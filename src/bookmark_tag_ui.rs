//! Edit Bookmark (`TE:34-281`): a small always-on-top window with the bookmark's title and URL, a
//! Tags field with prefix completions, Delete Bookmark at the left, Cancel (Escape) and Save (Enter)
//! at the right. The window only collects what the person typed and answers a `TagAct`; the app
//! applies it (the one database owner, `Db`) and asks the Delete question.
//!
//! The title, URL and tags are untrusted text (pages and imported files wrote them): every one is
//! shown through `display_text`, cut, and truncated by the label; nothing is rendered as markup.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::browser_db_tags::{clean_tag, MAX_TAGS_PER};
use crate::egui_host::EguiHost;
use crate::layout::PaneId;

pub const TITLE: &str = "Edit Bookmark";
pub const TITLE_LABEL: &str = "Title:";
pub const URL_LABEL: &str = "URL:";
pub const TAGS_LABEL: &str = "Tags:";
pub const TAGS_PLACEHOLDER: &str = "Enter tags\u{2026}";
pub const DELETE: &str = "Delete Bookmark";
pub const SAVE: &str = "Save";
pub const CANCEL: &str = "Cancel";
pub const DELETE_CONFIRM: &str = "Are you sure you want to delete this bookmark?";
pub const DELETE_BUTTON: &str = "Delete";
pub const DELETED_TOAST: &str = "Bookmark Deleted";
pub const REMOVED_TOAST: &str = "Bookmark Removed";
pub const SIZE: (f32, f32) = (480.0, 200.0);

/// The longest title and URL the window shows (characters); the rest is cut.
const SHOWN_TITLE: usize = 200;
const SHOWN_URL: usize = 300;
/// The completion list shows at most this many tags.
const MAX_COMPLETIONS: usize = 6;
/// The field takes at most this many characters (32 tags of 64 and their commas).
const MAX_FIELD: usize = MAX_TAGS_PER * 66;

/// The tags a field's text holds: comma separated, each trimmed and cleaned, empties dropped, the
/// first occurrence kept (NSTokenField's comma tokenizing), at most `MAX_TAGS_PER`.
pub fn parse_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in text.split(',').take(MAX_TAGS_PER * 4) {
        if let Some(tag) = clean_tag(piece) {
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
        if out.len() == MAX_TAGS_PER {
            break;
        }
    }
    out
}

/// (removed, added) going from `old` to `new` (`TE:193-208`).
pub fn tag_diff(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let removed = old.iter().filter(|t| !new.contains(t)).cloned().collect();
    let added = new.iter().filter(|t| !old.contains(t)).cloned().collect();
    (removed, added)
}

/// The tags of `all` that start with `prefix`, ignoring case (`TE:263-268`).
pub fn completions(all: &[String], prefix: &str) -> Vec<String> {
    let p = prefix.to_lowercase();
    all.iter().filter(|t| t.to_lowercase().starts_with(&p)).cloned().collect()
}

/// What the person asked.
#[derive(Debug, PartialEq)]
pub enum TagAct {
    /// The tags to keep.
    Save(Vec<String>),
    Delete,
    Cancel,
}

/// The text after the last comma, trimmed: the token being typed.
fn current_token(text: &str) -> &str {
    text.rsplit(',').next().unwrap_or("").trim()
}

/// `text` with its last token replaced by `tag`, and a comma and space after it.
fn complete_token(text: &str, tag: &str) -> String {
    let head = match text.rfind(',') {
        Some(i) => format!("{}, ", text[..i].trim_end()),
        None => String::new(),
    };
    format!("{head}{tag}, ")
}

pub struct TagEditor {
    host: EguiHost,
    pane: PaneId,
    /// The bookmark's own URL (the database key); `shown` is the cut, cleaned text of it.
    url: String,
    shown: String,
    title: String,
    text: String,
    all: Vec<String>,
    first: bool,
}

impl TagEditor {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, pane: PaneId, url: String, title: Option<String>, tags: Vec<String>, all: Vec<String>, center: (f64, f64)) -> Result<TagEditor, String> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(f64::from(SIZE.0), f64::from(SIZE.1)))
            .with_resizable(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.follow_theme(None);
        // Centered on the browser window (`BVC:1442-1462`), in this window's own pixels.
        let scale = host.window.scale_factor();
        let (w, h) = (f64::from(SIZE.0) * scale, f64::from(SIZE.1) * scale);
        host.window.set_outer_position(winit::dpi::PhysicalPosition::new((center.0 - w / 2.0).round() as i32, (center.1 - h / 2.0).round() as i32));
        let shown = |s: &str, max: usize| crate::inert_files::display_text(s, max, false);
        let text = tags.iter().map(|t| format!("{t}, ")).collect::<String>();
        let all = all.iter().filter_map(|t| clean_tag(t)).take(1000).collect();
        Ok(TagEditor { host, pane, shown: shown(&url, SHOWN_URL), url, title: shown(title.as_deref().unwrap_or(""), SHOWN_TITLE), text, all, first: true })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// The bookmark's URL: the editor's identity.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn pane(&self) -> PaneId {
        self.pane
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// Feeds an event; a close, or a frame's click or key, may answer.
    pub fn event(&mut self, e: &WindowEvent) -> Option<TagAct> {
        match e {
            WindowEvent::CloseRequested => Some(TagAct::Cancel),
            WindowEvent::RedrawRequested => self.redraw(),
            other => {
                self.host.event(other);
                None
            }
        }
    }

    fn redraw(&mut self) -> Option<TagAct> {
        let mut out = None;
        let (title, url, text, all, first) = (&self.title, &self.shown, &mut self.text, &self.all, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let secondary = ui.visuals().weak_text_color();
                for (label, value) in [(TITLE_LABEL, title), (URL_LABEL, url)] {
                    ui.horizontal(|ui| {
                        ui.add_sized([50.0, 18.0], egui::Label::new(label));
                        ui.add(egui::Label::new(egui::RichText::new(value.as_str()).color(secondary)).truncate());
                    });
                }
                ui.horizontal(|ui| {
                    ui.add_sized([50.0, 18.0], egui::Label::new(TAGS_LABEL));
                    let field = ui.add(egui::TextEdit::singleline(text).hint_text(TAGS_PLACEHOLDER).char_limit(MAX_FIELD).desired_width(f32::INFINITY));
                    if *first {
                        field.request_focus();
                        *first = false;
                    }
                });
                // Completions for the token being typed, tags already in the field left out.
                let have = parse_tokens(text.rfind(',').map_or("", |i| &text[..i]));
                let prefix = current_token(text).to_string();
                if !prefix.is_empty() {
                    let mut pick = None;
                    for tag in completions(all, &prefix).into_iter().filter(|t| !have.contains(t)).take(MAX_COMPLETIONS) {
                        if ui.selectable_label(false, &tag).clicked() {
                            pick = Some(tag);
                        }
                    }
                    if let Some(tag) = pick {
                        *text = complete_token(text, &tag);
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(DELETE).clicked() {
                        out = Some(TagAct::Delete);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(SAVE).clicked() {
                            out = Some(TagAct::Save(parse_tokens(text)));
                        }
                        if ui.button(CANCEL).clicked() {
                            out = Some(TagAct::Cancel);
                        }
                    });
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = Some(TagAct::Cancel);
                } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    out = Some(TagAct::Save(parse_tokens(text)));
                }
            });
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tag_editor_follows_iterm2() {
        assert_eq!(parse_tokens(" rust, web ,,rust"), v(&["rust", "web"]));
        assert_eq!(tag_diff(&v(&["a", "b"]), &v(&["b", "c"])), (v(&["a"]), v(&["c"])));
        assert_eq!(completions(&v(&["Rust", "ruby", "web"]), "ru"), v(&["Rust", "ruby"]));
        // The texts equal the Resume notes.
        assert_eq!((TITLE, TITLE_LABEL, URL_LABEL, TAGS_LABEL), ("Edit Bookmark", "Title:", "URL:", "Tags:"));
        assert_eq!((DELETE, SAVE, CANCEL, DELETE_BUTTON), ("Delete Bookmark", "Save", "Cancel", "Delete"));
        assert_eq!(DELETE_CONFIRM, "Are you sure you want to delete this bookmark?");
        assert_eq!((DELETED_TOAST, REMOVED_TOAST, TAGS_PLACEHOLDER), ("Bookmark Deleted", "Bookmark Removed", "Enter tags\u{2026}"));
        assert_eq!(SIZE, (480.0, 200.0));
    }

    #[test]
    fn typed_tags_are_bounded_and_clean() {
        let many = (0..100).map(|i| format!("t{i}")).collect::<Vec<_>>().join(",");
        assert_eq!(parse_tokens(&many).len(), MAX_TAGS_PER);
        assert_eq!(parse_tokens(&"x".repeat(500))[0].chars().count(), crate::browser_db_tags::MAX_TAG);
        assert!(parse_tokens("\u{202e}, \u{200b} ,").is_empty());
        assert_eq!(parse_tokens("a\u{202e}b"), v(&["ab"]));
    }

    #[test]
    fn completing_replaces_the_token_being_typed() {
        assert_eq!(current_token("a, b, ru"), "ru");
        assert_eq!(complete_token("a, b, ru", "rust"), "a, b, rust, ");
        assert_eq!(complete_token("ru", "rust"), "rust, ");
        assert_eq!(current_token(""), "");
    }

    /// The window shows untrusted text through the one sanitizer and never as markup, and holds no
    /// database: the app applies the answer.
    #[test]
    fn the_window_shows_text_inert_and_owns_no_store() {
        let src = include_str!("bookmark_tag_ui.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("display_text(s, max, false)") || prod.contains("crate::inert_files::display_text"));
        assert!(!prod.contains("browser_db::Db") && !prod.contains("rusqlite") && !prod.contains("rlog!") && !prod.contains("dlog!"));
        assert!(prod.contains(".truncate()"));
    }
}
