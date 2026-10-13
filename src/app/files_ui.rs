//! Terminal-initiated downloads (OSC 1337 File without inline=1): asked about with iTerm2's
//! warnings, then saved in the Downloads folder, as iTerm2 does; and inline files (inline=1),
//! asked about before they show while the pane's output waits.

use super::*;

/// iTerm2's size text for a download.
pub(super) fn size_text(n: usize) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} bytes"),
    }
}

/// The Downloads folder and why a setting was passed over: `DownloadsDirectory` when it is a
/// local absolute folder Termurion can write, else Windows' Downloads folder (`FOLDERID_Downloads`,
/// where a redirection puts it) on the same terms, else the profile folder
/// (`NSFileManager+iTerm.m:297-313`). A folder on a share is never used (the path judge): it
/// would leak the person's credentials, and `DownloadsDirectory` can arrive in an imported
/// settings file. The note names what was refused, for a banner.
pub(super) struct Downloads {
    pub dir: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

/// The Downloads folder chosen for one DownloadsDirectory value, for the run (final review item 17):
/// the folders are probed again only when the setting changes, and the banner is raised once per change.
#[derive(Default)]
pub(super) struct DownloadsCache {
    key: Option<String>,
    dir: Option<std::path::PathBuf>,
}

impl DownloadsCache {
    /// The folder for `setting`, from `compute` only when the setting changed; the second value is
    /// the banner to set (Some(None) clears it) when it was computed now, None when it was cached.
    pub(super) fn get(&mut self, setting: &str, compute: impl FnOnce(&str) -> Downloads) -> (Option<std::path::PathBuf>, Option<Option<String>>) {
        if self.key.as_deref() == Some(setting) {
            return (self.dir.clone(), None);
        }
        let d = compute(setting);
        self.key = Some(setting.to_string());
        self.dir = d.dir.clone();
        (d.dir, Some(d.note))
    }
}

/// A folder the judge accepts and a probe-free test file can be made in (made and removed).
fn writable_folder(dir: &std::path::Path) -> bool {
    let Some(d) = crate::localfs::judge(dir) else { return false };
    if !dir.is_absolute() || !crate::localfs::is_dir(&d) {
        return false;
    }
    let probe = dir.join(format!(".Termurion.{}", uuid::Uuid::new_v4().to_string().to_uppercase()));
    let Some(p) = crate::localfs::judge(&probe) else { return false };
    let made = crate::localfs::open_custom(&p, |o| {
        o.write(true).create_new(true);
    })
    .is_ok();
    if made {
        let _ = crate::localfs::remove_file(&p);
    }
    made
}

/// The folder Windows keeps Downloads in for this user.
#[cfg(windows)]
fn known_downloads() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};
    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: `out` receives a CoTaskMem string that is freed below.
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
    unsafe { CoTaskMemFree(out as _) };
    Some(path.into())
}

#[cfg(not(windows))]
fn known_downloads() -> Option<std::path::PathBuf> {
    None
}

pub(super) fn downloads(override_path: &str) -> Downloads {
    let known = known_downloads().or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join("Downloads")));
    let mut notes = Vec::new();
    // The setting is probed once: its answer serves the note and the choice.
    let override_ok = !override_path.is_empty() && writable_folder(std::path::Path::new(override_path));
    if !override_path.is_empty() && !override_ok {
        notes.push("the Downloads folder in Settings is not a local folder Termurion can write");
    }
    let writable = |p: &std::path::Path| if !override_path.is_empty() && p == std::path::Path::new(override_path) { override_ok } else { writable_folder(p) };
    let chosen = crate::resident::downloads_dir(override_path, known.clone(), &writable);
    if chosen.is_none() && known.is_some() {
        notes.push("Windows' Downloads folder is not a local folder Termurion can write");
    }
    let dir = chosen.or_else(|| std::env::var_os("USERPROFILE").map(std::path::PathBuf::from));
    let note = (!notes.is_empty()).then(|| format!("Downloads go to {}: {}.", dir.as_ref().map_or("nowhere".to_string(), |d| d.display().to_string()), notes.join("; ")));
    Downloads { dir, note }
}

#[cfg(windows)]
/// A message box with an OK button.
pub(super) fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Termurion").as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// A yes/no question in a message box.
pub(super) fn ask(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Termurion").as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

/// Larger than this, a download allowed by a remembered answer asks again
/// (`VT100ScreenBigFileDownloadThreshold`, `VT100Screen.m:53`).
pub(super) const BIG_DOWNLOAD: usize = 1 << 30;

/// The download question's identifier (`PS:19782`).
const DOWNLOAD_ID: &str = "NoSyncSuppressDownloadConfirmation";

/// A permanently silenceable question with Yes-like and No-like buttons and no Cancel.
fn yes_no_spec(id: &str, heading: &str, text: String, labels: [&str; 2]) -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(id.to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: heading.to_string(),
        text,
        labels: labels.map(str::to_string).to_vec(),
        cancel: None,
        never_remember: Vec::new(),
    }
}

/// A name the terminal sent, as a question (and the Suppressed Alerts catalog, which keeps the
/// text) shows it: control, bidirectional formatting, line and paragraph separator and
/// zero-width characters dropped, and at most `SHOWN_NAME` characters, the rest an ellipsis.
pub(super) fn shown_name(name: &str) -> String {
    crate::inert_files::shown_name(name)
}

/// "Allow Terminal-Initiated Download?" (or Display, for an `inline` file) for the file `name`
/// of `size` bytes: Yes or No (`screenConfirmDownloadAllowed:`, `PS:19781-19811`).
pub(super) fn download_spec(name: &str, size: usize, inline: bool) -> crate::warnings::Spec {
    let name = shown_name(name);
    let size = size_text(size);
    let (heading, text) = if inline {
        ("Allow Terminal-Initiated Display?", format!("The terminal has initiated display of a file named \u{201c}{name}\u{201d} of size {size}. Allow it?"))
    } else {
        ("Allow Terminal-Initiated Download?", format!("The terminal has initiated transfer of a file named \u{201c}{name}\u{201d} of size {size}. Download it?"))
    };
    yes_no_spec(DOWNLOAD_ID, heading, text, ["Yes", "No"])
}

/// "Allow Large File Download?" for the file `name`: Allow or Deny
/// (`screenConfirmDownloadNamed:canExceedSize:`, `PS:19772-19785`).
pub(super) fn big_download_spec(name: &str) -> crate::warnings::Spec {
    let name = shown_name(name);
    let text = format!("The download \u{201c}{name}\u{201d} is larger than {}. Continue?", size_text(BIG_DOWNLOAD));
    yes_no_spec("NoSyncAllowBigDownload", "Allow Large File Download?", text, ["Allow", "Deny"])
}

/// Whether an allowed download asks the large file question next: only when its question was
/// silenced as it was asked (a shown one already asked the person) and the file is larger
/// than `BIG_DOWNLOAD` (`PS:19812-19817`).
pub(super) fn needs_big_question(was_silenced: bool, allowed: bool, size: usize) -> bool {
    was_silenced && allowed && size > BIG_DOWNLOAD
}

/// The name a download will be saved under in `dir` (the question shows it).
fn saved_name(dir: &std::path::Path, name: &str) -> String {
    crate::inline_file::unique_path(dir, name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Whether a download named `name` would be refused when saved in `dir` (`inert_files`): asked
/// before the question, so the person is not asked to allow what cannot be saved.
pub(super) fn download_refused(dir: &std::path::Path, name: &str) -> bool {
    crate::inert_files::refused_write(&crate::inline_file::unique_path(dir, name).to_string_lossy())
}

/// The pane and inline file a question is about.
fn inline_ask_of(then: &super::scripts_alerts::Then) -> Option<(PaneId, u64)> {
    use super::scripts_alerts::Then;
    match then {
        Then::Inline { pane, serial, .. } | Then::BigInline(pane, serial) => Some((*pane, *serial)),
        _ => None,
    }
}

/// The pane a download question (or its large file question) is about.
fn download_ask_of(then: &super::scripts_alerts::Then) -> Option<PaneId> {
    use super::scripts_alerts::Then;
    match then {
        Then::Download(pane, ..) | Then::BigDownload(pane, _) => Some(*pane),
        _ => None,
    }
}

impl App {
    /// Asks about each file a pane was sent to save (through the alert queue); the allowed
    /// ones are saved (`download_answered`). A pane has one download question at a time: a
    /// file sent while its question is queued or showing is refused, as No.
    pub(super) fn take_downloads(&mut self) {
        let waiting: Vec<(PaneId, crate::inline_file::Download)> = self.panes.iter_mut().flat_map(|(id, p)| std::mem::take(&mut p.own_mut().downloads).into_iter().map(move |d| (*id, d))).collect();
        for (pane, d) in waiting {
            if self.alert_waits(|then| download_ask_of(then) == Some(pane)) {
                continue;
            }
            let Some(dir) = self.downloads_folder() else {
                self.banners.set("action", "No Downloads folder: USERPROFILE is not set.");
                continue;
            };
            // Whether the question was silenced before it is asked (`wasSilenced`): a shown
            // question whose box is checked is not.
            let was_silenced = crate::warnings::is_silenced(&self.globals, DOWNLOAD_ID, super::warnings_app::unix_now());
            // A name that cannot be saved is refused now, not after the person allowed it.
            if download_refused(&dir, &d.name) {
                self.banners.set("action", format!("Could not save {}: files of this type are not allowed.", shown_name(&saved_name(&dir, &d.name))));
                continue;
            }
            let spec = download_spec(&saved_name(&dir, &d.name), d.bytes.len(), false);
            self.ask_scripts(super::scripts_alerts::Ask::new(spec, super::scripts_alerts::Then::Download(pane, d, was_silenced)));
        }
        self.take_inline_asks();
    }

    /// Asks about each inline file a pane's output waits on ("Allow Terminal-Initiated
    /// Display?", `PS:19785-19820`), pausing the pane's reading until the answer. A remembered
    /// answer releases the pane at once, and its held output may wait on the next inline file:
    /// that one is asked here in turn, not by recursion. A pane whose file the screen dropped
    /// (too much output waited behind it) reads on.
    fn take_inline_asks(&mut self) {
        self.inline_asking = true;
        for p in self.panes.values_mut() {
            p.resume_after_inline();
        }
        self.close_dropped_inline_questions();
        loop {
            let waiting: Vec<(PaneId, u64, String, usize)> = self
                .panes
                .iter_mut()
                .filter_map(|(id, p)| {
                    let s = p.own_mut();
                    let (name, size) = s.inline_waiting()?;
                    Some((*id, s.inline_serial()?, name, size))
                })
                .collect();
            if waiting.is_empty() {
                break;
            }
            for (pane, serial, name, size) in waiting {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.pause_for_inline();
                }
                let was_silenced = crate::warnings::is_silenced(&self.globals, DOWNLOAD_ID, super::warnings_app::unix_now());
                let spec = download_spec(&name, size, true);
                self.ask_scripts(super::scripts_alerts::Ask::new(spec, super::scripts_alerts::Then::Inline { pane, serial, name, size, was_silenced }));
            }
        }
        self.inline_asking = false;
    }

    /// Questions about inline files the screen dropped (too much output waited behind them)
    /// close unanswered, as `Answer::Closed`: nothing is remembered or recorded, and the pane
    /// already reads on. A closed pane's (Undo Close may bring it back) stay.
    fn close_dropped_inline_questions(&mut self) {
        let panes = &self.panes;
        let dropped = |then: &super::scripts_alerts::Then| inline_ask_of(then).is_some_and(|(pane, serial)| panes.get(&pane).is_some_and(|p| p.own().inline_serial() != Some(serial)));
        self.scripts_alerts.retain(|a| !dropped(&a.then));
        if self.scripts_alert.as_ref().is_some_and(|(_, a)| dropped(&a.then)) {
            // The window closes as it is dropped.
            self.scripts_alert = None;
        }
    }

    /// Whether a question about `pane`'s inline file `serial` (display or large file) is
    /// queued or showing.
    pub(super) fn inline_question_waits(&self, pane: PaneId, serial: u64) -> bool {
        self.alert_waits(|then| inline_ask_of(then) == Some((pane, serial)))
    }

    /// A restarted pane's questions about its old screen's inline files close unanswered, as
    /// `close_dropped_inline_questions` closes them: the new screen numbers its files from 1
    /// again, so an old question could otherwise answer a new file.
    pub(super) fn close_inline_questions_of(&mut self, pane: PaneId) {
        let about = |then: &super::scripts_alerts::Then| inline_ask_of(then).is_some_and(|(p, _)| p == pane);
        self.scripts_alerts.retain(|a| !about(&a.then));
        if self.scripts_alert.as_ref().is_some_and(|(_, a)| about(&a.then)) {
            self.scripts_alert = None;
        }
    }

    /// The display question answered: Yes (0) shows the file, or first asks the large file
    /// question when the answer was remembered and the file is large (`PS:19812-19817`).
    pub(super) fn inline_answered(&mut self, pane: PaneId, serial: u64, name: &str, size: usize, was_silenced: bool, index: usize) {
        let allowed = index == 0;
        if needs_big_question(was_silenced, allowed, size) {
            self.ask_scripts(super::scripts_alerts::Ask::new(big_download_spec(name), super::scripts_alerts::Then::BigInline(pane, serial)));
        } else {
            self.inline_released(pane, serial, allowed);
        }
    }

    /// The pane's waiting inline file number `serial` shows (`allow`) or is dropped, and its
    /// held output is read; outside `take_inline_asks` what that output asked for is taken now.
    pub(super) fn inline_released(&mut self, pane: PaneId, serial: u64, allow: bool) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.inline_answered(serial, allow);
        if !self.inline_asking {
            self.after_output(&[pane], Instant::now());
        }
        self.redraw();
    }

    /// The folder downloads go to now (`DownloadsDirectory`, then Windows' Downloads folder, then
    /// the profile folder); a setting or a redirection that was refused shows one banner. Probed once
    /// per run and again only when the setting changes (`DownloadsCache`, P19).
    pub(super) fn downloads_folder(&mut self) -> Option<std::path::PathBuf> {
        let (dir, banner) = self.downloads_cache.get(&self.globals.string("DownloadsDirectory"), downloads);
        match banner {
            Some(Some(note)) => self.banners.set("downloads", note),
            Some(None) => self.banners.clear("downloads"),
            None => {}
        }
        dir
    }

    /// The download question answered: Yes (0) saves the file, or first asks the large file
    /// question when the answer was remembered and the file is large.
    pub(super) fn download_answered(&mut self, pane: PaneId, d: crate::inline_file::Download, was_silenced: bool, index: usize) {
        let allowed = index == 0;
        if needs_big_question(was_silenced, allowed, d.bytes.len()) {
            let name = self.downloads_folder().map_or_else(|| d.name.clone(), |dir| saved_name(&dir, &d.name));
            self.ask_scripts(super::scripts_alerts::Ask::new(big_download_spec(&name), super::scripts_alerts::Then::BigDownload(pane, d)));
        } else if allowed {
            self.save_download(d);
        }
    }

    /// Saves an allowed download in the Downloads folder.
    pub(super) fn save_download(&mut self, d: crate::inline_file::Download) {
        let Some(dir) = self.downloads_folder() else {
            self.banners.set("action", "No Downloads folder: USERPROFILE is not set.");
            return;
        };
        match crate::inline_file::save(&dir, &d.name, &d.bytes) {
            Ok(path) => self.banners.set("action", format!("Saved {}", path.display())),
            Err(e) => self.banners.set("action", format!("Could not save {}: {e}", saved_name(&dir, &d.name))),
        }
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inert_files::SHOWN_NAME;

    /// Final review item 17 (Task 8 M3, P19): the Downloads folder is probed once per run, again only
    /// when DownloadsDirectory changes, and its banner is raised once per change, not per download.
    #[test]
    fn the_downloads_folder_is_probed_once_per_setting() {
        let probes = std::cell::Cell::new(0);
        let compute = |s: &str| {
            probes.set(probes.get() + 1);
            Downloads { dir: Some(std::path::PathBuf::from(format!("C:\\{s}"))), note: Some(format!("note {s}")) }
        };
        let mut c = DownloadsCache::default();
        let (dir, banner) = c.get("a", compute);
        assert_eq!((dir.as_deref(), banner), (Some(std::path::Path::new("C:\\a")), Some(Some("note a".to_string()))));
        let (dir, banner) = c.get("a", compute);
        assert_eq!((dir.as_deref(), banner), (Some(std::path::Path::new("C:\\a")), None), "cached: no probe, the banner is not raised again");
        assert_eq!(probes.get(), 1);
        let (_, banner) = c.get("b", compute);
        assert_eq!((banner, probes.get()), (Some(Some("note b".to_string())), 2), "a changed setting probes again");
        let src = include_str!("files_ui.rs").split("\n#[cfg(test)]").next().unwrap();
        assert!(!src.contains("pub(super) fn downloads_dir("), "every caller goes through the cache");
    }

    /// Only the inline file questions name a pane and file.
    #[test]
    fn inline_questions_name_their_file() {
        use super::super::scripts_alerts::Then;
        assert_eq!(inline_ask_of(&Then::Inline { pane: 3, serial: 7, name: "a.png".into(), size: 5, was_silenced: false }), Some((3, 7)));
        assert_eq!(inline_ask_of(&Then::BigInline(3, 8)), Some((3, 8)));
        assert_eq!(inline_ask_of(&Then::Nothing), None);
    }

    /// A download whose name cannot be saved is refused before the question is asked.
    #[test]
    fn a_refused_download_is_not_asked_about() {
        let dir = std::path::Path::new("C:\\dl");
        for bad in ["x.url", "X.LNK", "desktop.ini", "a.scf", "..\\evil.scf"] {
            assert!(download_refused(dir, bad), "{bad:?}");
        }
        for fine in ["a.txt", "photo.png", "con", "a.ini", "a:b.txt"] {
            assert!(!download_refused(dir, fine), "{fine:?} (a device name is renamed and a colon replaced, not refused)");
        }
    }

    /// Only the download questions name a pane: its one question at a time.
    #[test]
    fn download_questions_name_their_pane() {
        use super::super::scripts_alerts::Then;
        let d = || crate::inline_file::Download { name: "a".into(), bytes: Vec::new() };
        assert_eq!(download_ask_of(&Then::Download(4, d(), false)), Some(4));
        assert_eq!(download_ask_of(&Then::BigDownload(5, d())), Some(5));
        assert_eq!(download_ask_of(&Then::BigInline(3, 8)), None);
        assert_eq!(download_ask_of(&Then::Nothing), None);
    }

    /// A name the terminal sent cannot break the question's text or flood it.
    #[test]
    fn sent_names_are_shown_safely() {
        assert_eq!(shown_name("a.png"), "a.png");
        assert_eq!(shown_name("a\nb\r\u{7}c.png"), "abc.png");
        assert_eq!(shown_name("evil\u{202e}gnp.exe"), "evilgnp.exe");
        assert_eq!(shown_name("\u{2066}x\u{2069}\u{200f}"), "x");
        assert_eq!(shown_name("a\u{2028}b\u{2029}c.png"), "abc.png", "no new line in the question");
        assert_eq!(shown_name("\u{feff}a\u{200b}b\u{200c}\u{200d}c\u{2060}.png"), "abc.png");
        let long = "x".repeat(5000);
        let shown = shown_name(&long);
        assert_eq!(shown.chars().count(), SHOWN_NAME + 1);
        assert!(shown.ends_with('\u{2026}'));
        let s = download_spec("a\u{202e}b", 3, true);
        assert!(s.text.contains("\u{201c}ab\u{201d}"), "{}", s.text);
        assert!(big_download_spec("a\nb").text.contains("\u{201c}ab\u{201d}"));
    }

    #[test]
    fn sizes_read_like_iterm2() {
        assert_eq!(size_text(5), "5 bytes");
        assert_eq!(size_text(2048), "2.0 KB");
        assert_eq!(size_text(3 << 20), "3.0 MB");
    }

    /// `screenConfirmDownloadAllowed:` and `screenConfirmDownloadNamed:canExceedSize:`
    /// (`PS:19772-19820`).
    #[test]
    fn download_confirmation_follows_iterm2() {
        use crate::warnings::Kind;
        let s = download_spec("a.png", 2048, false);
        assert_eq!(s.heading, "Allow Terminal-Initiated Download?");
        assert_eq!(s.text, "The terminal has initiated transfer of a file named \u{201c}a.png\u{201d} of size 2.0 KB. Download it?");
        assert_eq!(s.labels, ["Yes", "No"]);
        assert_eq!(s.id.as_deref(), Some("NoSyncSuppressDownloadConfirmation"));
        assert_eq!((s.kind, s.cancel), (Kind::Permanent, None));
        let s = download_spec("a.png", 2048, true);
        assert_eq!(s.heading, "Allow Terminal-Initiated Display?");
        assert_eq!(s.text, "The terminal has initiated display of a file named \u{201c}a.png\u{201d} of size 2.0 KB. Allow it?");
        let b = big_download_spec("a.iso");
        assert_eq!(b.heading, "Allow Large File Download?");
        assert_eq!(b.text, "The download \u{201c}a.iso\u{201d} is larger than 1.0 GB. Continue?");
        assert_eq!(b.labels, ["Allow", "Deny"]);
        assert_eq!(b.id.as_deref(), Some("NoSyncAllowBigDownload"));
        assert_eq!((b.kind, b.cancel), (Kind::Permanent, None));
        assert_eq!(BIG_DOWNLOAD, 1 << 30);
        assert!(needs_big_question(true, true, BIG_DOWNLOAD + 1));
        assert!(!needs_big_question(true, true, BIG_DOWNLOAD), "only larger than the limit");
        assert!(!needs_big_question(false, true, BIG_DOWNLOAD + 1), "a shown question already asked");
        assert!(!needs_big_question(true, false, BIG_DOWNLOAD + 1), "No");
    }
}
