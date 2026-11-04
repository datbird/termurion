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
        // "HTML Format" text with a header we cannot read is never offered as it is: plain text instead.
        return match (html_part(html), html.contains("StartHTML:")) {
            (Some(doc), _) => Plan::Html(doc.to_string()),
            (None, true) => Plan::Plain,
            (None, false) => Plan::Html(html.to_string()),
        };
    }
    if !rtf.is_empty() {
        return Plan::Refuse(Unsupported { feature: "Copy with styles as RTF", plural: false });
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
/// The queue is bounded: only the newest set waits, and a few reads, so a stuck X connect cannot pile up text.
#[cfg(not(target_os = "macos"))]
mod primary {
    use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
    use std::sync::{Condvar, Mutex, OnceLock};

    type Done = Box<dyn FnOnce(Option<String>) + Send>;
    const MAX_READS: usize = 4;

    #[derive(Default)]
    struct Queue {
        set: Option<String>,
        reads: Vec<Done>,
    }

    fn queue() -> &'static (Mutex<Queue>, Condvar) {
        static Q: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
        Q.get_or_init(|| {
            std::thread::spawn(|| {
                let (lock, cv) = queue();
                let mut cb: Option<Clipboard> = None;
                loop {
                    let work = {
                        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while q.set.is_none() && q.reads.is_empty() {
                            q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                        std::mem::take(&mut *q)
                    };
                    if cb.is_none() {
                        cb = Clipboard::new().ok();
                    }
                    if let Some(text) = work.set {
                        let ok = cb.as_mut().is_some_and(|c| c.set().clipboard(LinuxClipboardKind::Primary).text(text).is_ok());
                        if !ok {
                            crate::dlog!("The primary selection could not be set");
                        }
                    }
                    for done in work.reads {
                        done(cb.as_mut().and_then(|c| c.get().clipboard(LinuxClipboardKind::Primary).text().ok()));
                    }
                }
            });
            (Mutex::new(Queue::default()), Condvar::new())
        })
    }

    pub fn set(text: &str) -> Result<(), String> {
        let (lock, cv) = queue();
        lock.lock().unwrap_or_else(|e| e.into_inner()).set = Some(text.to_string());
        cv.notify_one();
        Ok(())
    }

    pub fn request(done: Done) {
        let (lock, cv) = queue();
        let full = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            if q.reads.len() >= MAX_READS { Some(done) } else { q.reads.push(done); None }
        };
        match full {
            Some(done) => done(None),
            None => cv.notify_one(),
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
        // A header that cannot be read never reaches the clipboard as HTML.
        assert_eq!(plan("a", "Version:0.9\r\nStartHTML:xx\r\n<b>a</b>", ""), Plan::Plain);
        assert_eq!(plan("a", "<b>a</b>", ""), Plan::Html("<b>a</b>".into()));
        assert_eq!(plan("a", "", "{\\rtf1}"), Plan::Refuse(Unsupported { feature: "Copy with styles as RTF", plural: false }));
    }

    #[test]
    fn unix_clipboard_files_empty() {
        assert!(files().is_empty());
    }
}
