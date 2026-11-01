//! Unix clipboard: plain text through arboard. A styled copy (HTML, RTF) is not offered yet.

use super::super::Unsupported;

/// Plain text only. A styled copy is refused before the clipboard is touched, so the old contents stay.
pub fn set(text: &str, html: &str, rtf: &str) -> Result<(), String> {
    if !html.is_empty() || !rtf.is_empty() {
        return Err(Unsupported { feature: "Copy with styles" }.into());
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("could not open the clipboard ({e})"))?;
    cb.set_text(text.to_string()).map_err(|e| format!("could not set the clipboard ({e})"))
}

/// No file list on the clipboard is read on Unix yet, so there is none to report.
pub fn files() -> Vec<std::path::PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_styled_copy_unsupported() {
        let e = set("a", "<b>a</b>", "").unwrap_err();
        assert!(e.contains("Copy with styles is not available"), "{e}");
        assert!(set("a", "", "x").is_err());
    }

    #[test]
    fn unix_clipboard_files_empty() {
        assert!(files().is_empty());
    }
}
