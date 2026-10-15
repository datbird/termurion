//! Paste with files or an image on the clipboard instead of text (`iTermNonTextPasteHelper.swift`):
//! the question picks how, the answer pastes the path, the contents as base64, an archive of a
//! folder, or a text file's text. The words and the rules are `nontext_paste`.

use std::path::PathBuf;

use super::*;
use crate::nontext_paste as np;

/// What the open question is about.
pub(super) enum NonText {
    Files { paths: Vec<LocalPath>, actions: Vec<np::FileAction> },
    Image { png: Vec<u8>, actions: Vec<np::ImageAction> },
    /// The large confirmation: the base64 to paste when OK.
    Large(String),
}

use crate::localfs::LocalPath;

impl App {
    /// Paste found no text: files or an image on the clipboard get their question. False when the
    /// clipboard holds neither.
    pub(super) fn nontext_paste(&mut self, pane: PaneId) -> bool {
        let files = crate::winclip::files();
        if !files.is_empty() {
            self.nontext_files(pane, files);
            return true;
        }
        let image = self.clipboard.as_mut().and_then(|cb| cb.get_image().ok());
        if let Some(img) = image {
            let Some(rgba) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned()) else { return false };
            let mut png = Vec::new();
            if image::DynamicImage::ImageRgba8(rgba).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
                return false;
            }
            let actions = np::image_actions();
            let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
            let text = np::png_prompt(&np::byte_size(png.len() as u64));
            let id = np::image_identifier(&actions);
            self.nontext.insert(pane, NonText::Image { png, actions });
            self.ask_scripts(super::scripts_alerts::ask(np::IMAGE_HEADING, &text, &labels, Some(&id), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::NonText(pane)));
            return true;
        }
        false
    }

    fn nontext_files(&mut self, pane: PaneId, files: Vec<PathBuf>) {
        let judged: Vec<LocalPath> = files.iter().filter_map(crate::localfs::judge).filter(crate::localfs::exists).collect();
        if judged.is_empty() {
            return self.nontext_error(np::COPIED_FILE_MISSING.to_string());
        }
        if judged.len() < files.len() {
            self.nontext_error(np::some_missing(files.len() - judged.len()));
        }
        let single = judged.len() == 1;
        let is_dir = single && crate::localfs::is_dir(&judged[0]);
        let can_text = single && !is_dir && crate::localfs::open(&judged[0]).ok().is_some_and(|f| {
            use std::io::Read;
            let mut buf = Vec::new();
            f.take(np::UTF8_SAMPLE as u64 + 1).read_to_end(&mut buf).is_ok() && np::prefix_is_utf8(&buf)
        });
        let actions = np::file_actions(single, is_dir, can_text);
        let labels: Vec<&str> = actions.iter().map(|a| a.label()).collect();
        let names: Vec<String> = judged.iter().map(|p| p.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())).collect();
        let text = np::file_prompt(&names, is_dir);
        let id = np::file_identifier(&actions);
        let mut ask = super::scripts_alerts::ask(np::file_heading(single, is_dir), &text, &labels, Some(&id), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::NonText(pane));
        // A remembered Paste as Text would answer a list without it with the wrong button: never remembered.
        if let Some(i) = actions.iter().position(|a| *a == np::FileAction::AsText) {
            ask.spec.never_remember.push(i);
        }
        self.nontext.insert(pane, NonText::Files { paths: judged, actions });
        self.ask_scripts(ask);
    }

    /// `showError`: a warning box with OK, headed "Paste Failed".
    fn nontext_error(&mut self, text: String) {
        self.ask_scripts(super::scripts_alerts::ask(np::FAILED_HEADING, &text, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing));
    }

    /// The question's answer (None: dropped).
    pub(super) fn nontext_answered(&mut self, pane: PaneId, index: Option<usize>) {
        let Some(pending) = self.nontext.remove(&pane) else { return };
        let Some(index) = index else { return };
        match pending {
            NonText::Large(base64) => {
                if index == 0 {
                    self.user_paste(pane, &base64, crate::broadcast::Write::Paste);
                }
            }
            NonText::Files { paths, actions } => match actions.get(index).copied() {
                Some(np::FileAction::PastePath) | Some(np::FileAction::PastePaths) => {
                    let list: Vec<String> = paths.iter().map(|p| p.path().to_string_lossy().into_owned()).collect();
                    // The same text a drop makes, with the same guard for a name the shell cannot take.
                    let shell = self.composer_shell(pane);
                    match crate::coprocess::drop_text(&list, self.globals.bool("WrapDroppedFilenamesInQuotesWhenPasting"), shell.as_deref()) {
                        Ok(text) => self.user_paste(pane, &text, crate::broadcast::Write::Paste),
                        Err(why) => self.banners.set("action", why),
                    }
                }
                Some(np::FileAction::Base64) => {
                    let name = file_name(&paths[0]);
                    match crate::localfs::read(&paths[0]) {
                        Ok(bytes) => self.base64_with_confirmation(pane, crate::paste::base64_lines(&bytes)),
                        Err(_) => self.nontext_error(np::could_not_read(&name)),
                    }
                }
                Some(np::FileAction::Base64Archive) => {
                    let name = file_name(&paths[0]);
                    let mut tar = tar::Builder::new(Vec::new());
                    let made = crate::localfs::append_to_tar(&mut tar, std::path::Path::new(&name), &paths[0]).and_then(|_| tar.into_inner()).and_then(|raw| {
                        use std::io::Write;
                        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                        gz.write_all(&raw)?;
                        gz.finish()
                    });
                    match made {
                        Ok(bytes) => self.base64_with_confirmation(pane, crate::paste::base64_lines(&bytes)),
                        Err(e) => self.nontext_error(np::archive_failed(&name, &e.to_string())),
                    }
                }
                Some(np::FileAction::AsText) => {
                    let name = file_name(&paths[0]);
                    match crate::localfs::read(&paths[0]) {
                        Err(_) => self.nontext_error(np::could_not_read(&name)),
                        Ok(bytes) => match String::from_utf8(bytes) {
                            Ok(text) => self.user_paste(pane, &text, crate::broadcast::Write::Paste),
                            Err(_) => self.nontext_error(np::not_utf8(&name)),
                        },
                    }
                }
                Some(np::FileAction::Cancel) | None => {}
            },
            NonText::Image { png, actions } => match actions.get(index).copied() {
                Some(np::ImageAction::SaveTemp) => {
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let t = crate::stamps::local(secs);
                    let stamp = format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
                    let path = std::env::temp_dir().join(format!("pasted-image-{stamp}.png"));
                    let saved = crate::localfs::judge_io(&path).and_then(|lp| crate::localfs::write(&lp, &png).map(|_| lp));
                    match saved {
                        Ok(lp) => {
                            let list = [lp.path().to_string_lossy().into_owned()];
                            let shell = self.composer_shell(pane);
                            match crate::coprocess::drop_text(&list, self.globals.bool("WrapDroppedFilenamesInQuotesWhenPasting"), shell.as_deref()) {
                                Ok(text) => self.user_paste(pane, &text, crate::broadcast::Write::Paste),
                                Err(why) => self.banners.set("action", why),
                            }
                        }
                        Err(e) => self.nontext_error(np::image_not_saved(&e.to_string())),
                    }
                }
                Some(np::ImageAction::Base64) => self.base64_with_confirmation(pane, crate::paste::base64_lines(&png)),
                Some(np::ImageAction::Cancel) | None => {}
            },
        }
    }

    /// `pasteBase64WithConfirmationIfNeeded`: over 10,000 characters asks first.
    fn base64_with_confirmation(&mut self, pane: PaneId, base64: String) {
        if base64.chars().count() <= np::LARGE_PASTE_ABOVE {
            return self.user_paste(pane, &base64, crate::broadcast::Write::Paste);
        }
        let text = np::large_paste_text(base64.chars().count());
        self.nontext.insert(pane, NonText::Large(base64));
        self.ask_scripts(super::scripts_alerts::ask(np::LARGE_PASTE_HEADING, &text, &[crate::api_auth::OK, crate::api_auth::CANCEL], Some(np::LARGE_PASTE_ID), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::NonText(pane)));
    }
}

fn file_name(p: &LocalPath) -> String {
    p.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}
