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

/// The Downloads folder (the user's profile when there is none).
fn downloads_dir() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE")?);
    let d = home.join("Downloads");
    Some(if d.is_dir() { d } else { home })
}

#[cfg(windows)]
/// A message box with an OK button.
pub(super) fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// A yes/no question in a message box.
pub(super) fn ask(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
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

/// "Allow Terminal-Initiated Download?" (or Display, for an `inline` file) for the file `name`
/// of `size` bytes: Yes or No (`screenConfirmDownloadAllowed:`, `PS:19781-19811`).
pub(super) fn download_spec(name: &str, size: usize, inline: bool) -> crate::warnings::Spec {
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

impl App {
    /// Asks about each file a pane was sent to save (through the alert queue); the allowed
    /// ones are saved (`download_answered`).
    pub(super) fn take_downloads(&mut self) {
        let waiting: Vec<crate::inline_file::Download> = self.panes.values_mut().flat_map(|p| std::mem::take(&mut p.own_mut().downloads)).collect();
        for d in waiting {
            let Some(dir) = downloads_dir() else {
                self.banners.set("action", "No Downloads folder: USERPROFILE is not set.");
                continue;
            };
            // Whether the question was silenced before it is asked (`wasSilenced`): a shown
            // question whose box is checked is not.
            let was_silenced = crate::warnings::is_silenced(&self.globals, DOWNLOAD_ID, super::warnings_app::unix_now());
            let spec = download_spec(&saved_name(&dir, &d.name), d.bytes.len(), false);
            self.ask_scripts(super::scripts_alerts::Ask::new(spec, super::scripts_alerts::Then::Download(d, was_silenced)));
        }
        self.take_inline_asks();
    }

    /// Asks about each inline file a pane's output waits on ("Allow Terminal-Initiated
    /// Display?", `PS:19785-19820`), pausing the pane's reading until the answer. A remembered
    /// answer releases the pane at once, and its held output may wait on the next inline file:
    /// that one is asked here in turn, not by recursion.
    fn take_inline_asks(&mut self) {
        self.inline_asking = true;
        loop {
            let waiting: Vec<(PaneId, String, usize)> = self.panes.iter_mut().filter_map(|(id, p)| p.own_mut().inline_waiting().map(|(name, size)| (*id, name, size))).collect();
            if waiting.is_empty() {
                break;
            }
            for (pane, name, size) in waiting {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.set_paused(true);
                }
                let was_silenced = crate::warnings::is_silenced(&self.globals, DOWNLOAD_ID, super::warnings_app::unix_now());
                let spec = download_spec(&name, size, true);
                self.ask_scripts(super::scripts_alerts::Ask::new(spec, super::scripts_alerts::Then::Inline { pane, name, size, was_silenced }));
            }
        }
        self.inline_asking = false;
    }

    /// The display question answered: Yes (0) shows the file, or first asks the large file
    /// question when the answer was remembered and the file is large (`PS:19812-19817`).
    pub(super) fn inline_answered(&mut self, pane: PaneId, name: &str, size: usize, was_silenced: bool, index: usize) {
        let allowed = index == 0;
        if needs_big_question(was_silenced, allowed, size) {
            self.ask_scripts(super::scripts_alerts::Ask::new(big_download_spec(name), super::scripts_alerts::Then::BigInline(pane)));
        } else {
            self.inline_released(pane, allowed);
        }
    }

    /// The pane's waiting inline file shows (`allow`) or is dropped, and its held output is
    /// read; outside `take_inline_asks` what that output asked for is taken now.
    pub(super) fn inline_released(&mut self, pane: PaneId, allow: bool) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.inline_answered(allow);
        if !self.inline_asking {
            self.after_output(&[pane], Instant::now());
        }
        self.redraw();
    }

    /// The download question answered: Yes (0) saves the file, or first asks the large file
    /// question when the answer was remembered and the file is large.
    pub(super) fn download_answered(&mut self, d: crate::inline_file::Download, was_silenced: bool, index: usize) {
        let allowed = index == 0;
        if needs_big_question(was_silenced, allowed, d.bytes.len()) {
            let name = downloads_dir().map_or_else(|| d.name.clone(), |dir| saved_name(&dir, &d.name));
            self.ask_scripts(super::scripts_alerts::Ask::new(big_download_spec(&name), super::scripts_alerts::Then::BigDownload(d)));
        } else if allowed {
            self.save_download(d);
        }
    }

    /// Saves an allowed download in the Downloads folder.
    pub(super) fn save_download(&mut self, d: crate::inline_file::Download) {
        let Some(dir) = downloads_dir() else {
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
