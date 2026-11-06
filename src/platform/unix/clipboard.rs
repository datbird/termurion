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
    // The window's connection carries plain text only: styles are refused rather than dropped.
    #[cfg(not(target_os = "macos"))]
    if window::attached() {
        if let Plan::Html(_) = plan {
            return Err("Copy with styles needs the data-control protocol, which this Wayland desktop does not have; Copy still works".to_string());
        }
        window::store(text);
        return Ok(());
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

/// The Mac pasteboard needs no window, so arboard is the clipboard as it is.
#[cfg(target_os = "macos")]
pub type Clipboard = arboard::Clipboard;

/// Nothing to attach on macOS.
#[cfg(target_os = "macos")]
pub fn attach(_display: &dyn winit::raw_window_handle::HasDisplayHandle) {}

/// Nothing to let go of on macOS.
#[cfg(target_os = "macos")]
pub fn detach() {}

/// Which way the clipboard's text goes.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// arboard: X11, or Wayland's data-control protocol.
    Arboard,
    /// The window's own Wayland connection (wl_data_device), which every compositor has.
    Window,
}

/// A Wayland session without data-control (GNOME, weston) would leave arboard on X11, which the
/// window's paste never sees there (or, with no Xwayland, no clipboard at all).
#[cfg(not(target_os = "macos"))]
fn route(wayland: bool, data_control: bool) -> Route {
    if wayland && !data_control { Route::Window } else { Route::Arboard }
}

/// The clipboard through the window's Wayland connection, one for the process, from `attach` to `detach`.
#[cfg(not(target_os = "macos"))]
mod window {
    use std::sync::Mutex;

    static BOARD: Mutex<Option<smithay_clipboard::Clipboard>> = Mutex::new(None);

    fn board() -> std::sync::MutexGuard<'static, Option<smithay_clipboard::Clipboard>> {
        BOARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn attach(display: &dyn winit::raw_window_handle::HasDisplayHandle) {
        use winit::raw_window_handle::RawDisplayHandle;
        let Ok(handle) = display.display_handle() else { return };
        let RawDisplayHandle::Wayland(w) = handle.as_raw() else { return };
        let data_control = wl_clipboard_rs::utils::is_primary_selection_supported().is_ok();
        if super::route(true, data_control) != super::Route::Window {
            return;
        }
        crate::dlog!("No data-control protocol: copy and paste go through the window's Wayland connection");
        // SAFETY: the display is winit's and outlives this: `detach` drops the board in `exiting`, before the loop closes it.
        *board() = Some(unsafe { smithay_clipboard::Clipboard::new(w.display.as_ptr()) });
    }

    pub fn detach() {
        board().take();
    }

    pub fn attached() -> bool {
        board().is_some()
    }

    /// False when nothing is attached.
    pub fn store(text: &str) -> bool {
        board().as_ref().map(|b| b.store(text)).is_some()
    }

    pub fn store_primary(text: &str) -> bool {
        board().as_ref().map(|b| b.store_primary(text)).is_some()
    }

    /// None when nothing is attached. An empty clipboard reads as arboard's ContentNotAvailable.
    pub fn load() -> Option<Result<String, arboard::Error>> {
        board().as_ref().map(|b| b.load().map_err(to_arboard))
    }

    pub fn load_primary() -> Option<Option<String>> {
        board().as_ref().map(|b| b.load_primary().ok())
    }

    fn to_arboard(e: std::io::Error) -> arboard::Error {
        let empty = e.kind() == std::io::ErrorKind::NotFound || e.to_string() == "selection is empty";
        if empty { arboard::Error::ContentNotAvailable } else { arboard::Error::Unknown { description: e.to_string() } }
    }
}

/// Starts the window-connection clipboard where the session needs it (Wayland without data-control).
#[cfg(not(target_os = "macos"))]
pub fn attach(display: &dyn winit::raw_window_handle::HasDisplayHandle) {
    window::attach(display);
}

/// Ends it while winit's display still lives. Twice is safe.
#[cfg(not(target_os = "macos"))]
pub fn detach() {
    window::detach();
}

/// The app's clipboard: text through the window's connection when attached, else arboard.
/// Images (and text where nothing is attached) stay arboard's.
#[cfg(not(target_os = "macos"))]
pub struct Clipboard {
    arboard: Option<arboard::Clipboard>,
}

#[cfg(not(target_os = "macos"))]
impl Clipboard {
    /// Fails only when neither way works (no X server and no attached window).
    pub fn new() -> Result<Self, arboard::Error> {
        match arboard::Clipboard::new() {
            Ok(c) => Ok(Self { arboard: Some(c) }),
            Err(_) if window::attached() => Ok(Self { arboard: None }),
            Err(e) => Err(e),
        }
    }

    fn arboard(&mut self) -> Result<&mut arboard::Clipboard, arboard::Error> {
        self.arboard.as_mut().ok_or(arboard::Error::ClipboardNotSupported)
    }

    pub fn set_text<'a, T: Into<std::borrow::Cow<'a, str>>>(&mut self, text: T) -> Result<(), arboard::Error> {
        let text = text.into();
        if window::store(&text) {
            return Ok(());
        }
        self.arboard()?.set_text(text)
    }

    pub fn get_text(&mut self) -> Result<String, arboard::Error> {
        if let Some(got) = window::load() {
            return got;
        }
        self.arboard()?.get_text()
    }

    pub fn get_image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
        self.arboard()?.get_image()
    }

    pub fn set_image(&mut self, image: arboard::ImageData<'_>) -> Result<(), arboard::Error> {
        self.arboard()?.set_image(image)
    }
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
                    // The window's connection, when attached, holds the primary selection too.
                    if super::window::attached() {
                        if let Some(text) = &work.set {
                            super::window::store_primary(text);
                        }
                        for done in work.reads {
                            done(super::window::load_primary().flatten());
                        }
                        continue;
                    }
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
pub fn set_private_text(cb: &mut Clipboard, text: &str) -> Result<(), arboard::Error> {
    #[cfg(target_os = "macos")]
    {
        use arboard::SetExtApple;
        cb.set().exclude_from_history().text(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use arboard::SetExtLinux;
        // The window's connection offers plain text only, with no hint (a gap, docs/port/unix-test-gaps.md).
        if window::store(text) {
            return Ok(());
        }
        cb.arboard()?.set().exclude_from_history().text(text)
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

    // GNOME and weston have no data-control: arboard would fall back to X11, which a Wayland
    // window's paste never sees, so text goes through the window's own connection there.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn wayland_without_data_control_uses_the_window() {
        assert_eq!(route(true, false), Route::Window);
        assert_eq!(route(true, true), Route::Arboard);
        assert_eq!(route(false, false), Route::Arboard);
    }
}
