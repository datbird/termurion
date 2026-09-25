//! Terminal-initiated downloads (OSC 1337 File without inline=1): asked about, then saved
//! in the Downloads folder, as iTerm2 does.

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
fn ask(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

impl App {
    /// Asks about each file a pane was sent to save, and saves the allowed ones.
    pub(super) fn take_downloads(&mut self) {
        let waiting: Vec<crate::inline_file::Download> = self.panes.values_mut().flat_map(|p| std::mem::take(&mut p.own_mut().downloads)).collect();
        for d in waiting {
            let Some(dir) = downloads_dir() else {
                self.banners.set("action", "No Downloads folder: USERPROFILE is not set.");
                continue;
            };
            // The question shows the name the file will really get.
            let shown = crate::inline_file::unique_path(&dir, &d.name);
            let name = shown.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let question = format!("Allow the terminal to download \"{name}\" ({}) to your Downloads folder?", size_text(d.bytes.len()));
            if !ask(&question) {
                continue;
            }
            match crate::inline_file::save(&dir, &d.name, &d.bytes) {
                Ok(path) => self.banners.set("action", format!("Saved {}", path.display())),
                Err(e) => self.banners.set("action", format!("Could not save {name}: {e}")),
            }
        }
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
}
