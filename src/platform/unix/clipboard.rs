//! Unix clipboard: plain text and HTML through arboard, and on Linux the primary selection.

use super::super::Unsupported;

/// What a copy puts on the clipboard.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    Plain,
    /// The HTML document, set beside the plain text.
    Html(String),
    Refuse(Unsupported),
}

/// Decides a copy from what the caller has. RTF alone is refused: arboard has no RTF flavor.
pub fn plan(_text: &str, html: &str, rtf: &str) -> Plan {
    if !html.is_empty() {
        return Plan::Html(html_part(html).unwrap_or(html).to_string());
    }
    if !rtf.is_empty() {
        return Plan::Refuse(Unsupported { feature: "Copy with styles as RTF" });
    }
    Plan::Plain
}

/// The document part of Windows "HTML Format" text, read from its `StartHTML:` and `EndHTML:` offsets.
pub fn html_part(cf_html: &str) -> Option<&str> {
    let field = |name: &str| -> Option<usize> {
        let at = cf_html.find(name)? + name.len();
        let digits: String = cf_html[at..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let start = field("StartHTML:")?;
    let end = field("EndHTML:").filter(|e| *e >= start).unwrap_or(cf_html.len()).min(cf_html.len());
    cf_html.get(start..end).filter(|d| !d.is_empty())
}

/// Plain text, or HTML with the plain text beside it. A refused copy leaves the old clipboard alone.
pub fn set(text: &str, html: &str, rtf: &str) -> Result<(), String> {
    let plan = plan(text, html, rtf);
    if let Plan::Refuse(u) = plan {
        return Err(u.into());
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("could not open the clipboard ({e})"))?;
    match plan {
        Plan::Html(h) => cb.set_html(h, Some(text.to_string())),
        _ => cb.set_text(text.to_string()),
    }
    .map_err(|e| format!("could not set the clipboard ({e})"))
}

/// No file list on the clipboard is read on Unix yet, so there is none to report.
pub fn files() -> Vec<std::path::PathBuf> {
    Vec::new()
}

/// Macs have no primary selection: nothing to set (a call with no meaning here).
#[cfg(target_os = "macos")]
pub fn set_selection(_text: &str) -> Result<(), String> {
    Ok(())
}

/// Macs have no primary selection: nothing to read.
#[cfg(target_os = "macos")]
pub fn request_selection(done: Box<dyn FnOnce(Option<String>) + Send>) {
    done(None);
}

/// One thread owns the clipboard handle for good: X11 serves a selection only while its owner lives.
#[cfg(not(target_os = "macos"))]
mod primary {
    use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
    use std::sync::{OnceLock, mpsc};

    enum Job {
        Set(String),
        Get(Box<dyn FnOnce(Option<String>) + Send>),
    }

    fn worker() -> &'static mpsc::Sender<Job> {
        static TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
        TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                let mut cb: Option<Clipboard> = None;
                for job in rx {
                    if cb.is_none() {
                        cb = Clipboard::new().ok();
                    }
                    match job {
                        Job::Set(text) => {
                            let ok = cb.as_mut().is_some_and(|c| c.set().clipboard(LinuxClipboardKind::Primary).text(text).is_ok());
                            if !ok {
                                crate::dlog!("The primary selection could not be set");
                            }
                        }
                        Job::Get(done) => done(cb.as_mut().and_then(|c| c.get().clipboard(LinuxClipboardKind::Primary).text().ok())),
                    }
                }
            });
            tx
        })
    }

    pub fn set(text: &str) -> Result<(), String> {
        worker().send(Job::Set(text.to_string())).map_err(|_| "the selection worker has stopped".to_string())
    }

    pub fn request(done: Box<dyn FnOnce(Option<String>) + Send>) {
        if let Err(mpsc::SendError(Job::Get(done))) = worker().send(Job::Get(done)) {
            done(None);
        }
    }
}

/// Puts `text` in the PRIMARY selection. The worker does the X work, so this returns at once.
#[cfg(not(target_os = "macos"))]
pub fn set_selection(text: &str) -> Result<(), String> {
    primary::set(text)
}

/// Reads the PRIMARY selection on the worker and calls `done` there (None when it is empty or unreadable).
#[cfg(not(target_os = "macos"))]
pub fn request_selection(done: Box<dyn FnOnce(Option<String>) + Send>) {
    primary::request(done);
}

/// A secret copy kept out of clipboard-manager history: the KDE password hint on Linux, the
/// concealed-type mark on macOS.
pub fn set_private_text(cb: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    #[cfg(target_os = "macos")]
    {
        use arboard::SetExtApple;
        cb.set().exclude_from_history().text(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use arboard::SetExtLinux;
        cb.set().exclude_from_history().text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_part_finds_the_document() {
        let doc = crate::styled_copy::cf_html("<b>x</b>");
        assert!(html_part(&doc).unwrap().starts_with("<html>"));
        assert!(html_part("junk").is_none());
    }

    #[test]
    fn styled_copy_plans() {
        assert_eq!(plan("a", "", ""), Plan::Plain);
        let doc = crate::styled_copy::cf_html("<b>a</b>");
        match plan("a", &doc, "{\\rtf1}") {
            Plan::Html(h) => assert!(h.starts_with("<html>") && h.contains("<b>a</b>"), "{h}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(plan("a", "", "{\\rtf1}"), Plan::Refuse(Unsupported { feature: "Copy with styles as RTF" }));
    }

    #[test]
    fn unix_clipboard_files_empty() {
        assert!(files().is_empty());
    }
}
