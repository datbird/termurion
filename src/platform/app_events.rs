//! Events the OS sends the whole app: a quit from the Dock or the system (macOS asks the app
//! before it ends), a Dock click with no window, and hiding the app. Windows has its own shutdown
//! and launch-pipe paths and Linux has none of these, so those backends do nothing.

#[cfg(target_os = "macos")]
use super::macos::app_events as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::unix::app_events as imp;
#[cfg(windows)]
use super::windows::app_events as imp;

/// What the system asked of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// Quit (Dock menu, Cmd-Q from the system, logout). Answer it with `answer_terminate`.
    Terminate,
    /// The Dock icon was clicked.
    Reopen,
}

/// Whether the system can hide the app and the other apps.
pub const HAS_APP_HIDING: bool = imp::HAS_APP_HIDING;

/// Starts delivering system events to `sink`. Call once, on the main thread.
pub fn install(sink: Box<dyn Fn(AppEvent) + Send + Sync>) {
    imp::install(sink)
}

/// Tells the system whether the quit it asked about goes ahead. Only the first answer after a
/// Terminate counts; with none waiting it does nothing.
pub fn answer_terminate(quit: bool) {
    imp::answer_terminate(quit)
}

/// Hides this app. False where the OS has no such thing.
pub fn hide_app() -> bool {
    imp::hide_app()
}

/// Hides every other app. False where the OS has no such thing.
pub fn hide_others() -> bool {
    imp::hide_others()
}

/// Shows every hidden app again. False where the OS has no such thing.
pub fn show_all() -> bool {
    imp::show_all()
}

/// A quit the system asked about, waiting for its one answer.
#[derive(Default)]
pub struct Terminate {
    waiting: bool,
}

impl Terminate {
    /// The system asked.
    pub fn ask(&mut self) {
        self.waiting = true;
    }

    /// The reply to send: Some only for the first answer after an ask.
    pub fn answer(&mut self, quit: bool) -> Option<bool> {
        std::mem::take(&mut self.waiting).then_some(quit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review Focus 4: the system gets one reply per question, never two and never one unasked.
    #[test]
    fn a_terminate_is_answered_once() {
        let mut t = Terminate::default();
        assert_eq!(t.answer(true), None, "nothing was asked");
        t.ask();
        assert_eq!(t.answer(false), Some(false));
        assert_eq!(t.answer(true), None, "already answered");
        t.ask();
        assert_eq!(t.answer(true), Some(true), "a new question gets a new answer");
    }
}
