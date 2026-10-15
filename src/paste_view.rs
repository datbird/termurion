//! The paste progress view (`PasteView.xib`, `MinimalPasteView.xib`, `PasteViewController.m`,
//! `iTermPasteHelper.m`): "Pasting…", the progress bar, Cancel, and while a paste waits for the
//! shell prompt the keyboard button that lets keystrokes through to the terminal instead of
//! queueing them, with its hint. Pure: the texts and the rules; the drawing is `scene::paste_view`.

use std::time::{Duration, Instant};

/// `PasteView.xib:25`; the label of an upload is "Sending…" (`PasteViewController.m:97`).
pub const PASTING: &str = "Pasting\u{2026}";
pub const SENDING: &str = "Sending\u{2026}";
/// `PasteViewController.m:139`, `:143-144`, `:218`.
pub const KEYSTROKES_ACCESSIBILITY: &str = "Send keystrokes to terminal";
pub const KEYSTROKES_TOOLTIP: &str = "Type directly to the terminal (for example to answer a password prompt) instead of queueing your keystrokes until the paste finishes.";
pub const QUEUED_HINT: &str = "Typing is queued while pasting. Click the keyboard to toggle queueing.";
/// How long the hint stays (`PasteViewController.m:250`).
pub const HINT_SECONDS: f64 = 4.0;
/// The widest the hint's callout is (`PasteViewController.m:215`).
pub const HINT_MAX_WIDTH: f32 = 220.0;
/// The gap between the keyboard button and Cancel (`kKeystrokePassthroughButtonGap`).
pub const BUTTON_GAP: f32 = 2.0;

/// One pane's view state: each paste starts with keystrokes queued (`:702`).
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub passthrough: bool,
    hint_since: Option<Instant>,
}

impl State {
    /// The keyboard button was clicked: the hint goes, and the new state is returned.
    pub fn toggle(&mut self) -> bool {
        self.hint_since = None;
        self.passthrough = !self.passthrough;
        self.passthrough
    }

    /// Typing while the paste waits for the prompt and keystrokes are queued shows the hint
    /// (`enqueueEvent:`, `iTermPasteHelper.m:570-575`).
    pub fn typed(&mut self, now: Instant, waiting_for_prompt: bool) {
        if waiting_for_prompt && !self.passthrough {
            self.hint_since = Some(now);
        }
    }

    pub fn hint_shown(&self, now: Instant) -> bool {
        self.hint_since.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs_f64(HINT_SECONDS))
    }

    /// When the hint goes, for the redraw.
    pub fn hint_until(&self) -> Option<Instant> {
        self.hint_since.map(|t| t + Duration::from_secs_f64(HINT_SECONDS))
    }

    /// The paste stopped waiting: the hint has nothing to point at (`setWaitingForPrompt:NO`).
    pub fn stopped_waiting(&mut self) {
        self.hint_since = None;
    }
}

/// Whether a key aborts a paste that waits for the prompt (`eventAbortsPasteWaitingForPrompt:`,
/// `PTYSession.m:11628-11650`): Esc or Ctrl+C, unless keystrokes pass through.
pub fn aborts(waiting_for_prompt: bool, passthrough: bool, escape: bool, ctrl_c: bool) -> bool {
    waiting_for_prompt && !passthrough && (escape || ctrl_c)
}

/// Whether a typed key is held until the paste ends (`PTYTextView.m:7510`).
pub fn queues_keys(pasting: bool, passthrough: bool) -> bool {
    pasting && !passthrough
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_iterm2s() {
        assert_eq!(PASTING, "Pasting\u{2026}");
        assert_eq!(SENDING, "Sending\u{2026}");
        assert_eq!(QUEUED_HINT, "Typing is queued while pasting. Click the keyboard to toggle queueing.");
        assert_eq!(KEYSTROKES_ACCESSIBILITY, "Send keystrokes to terminal");
        assert!(KEYSTROKES_TOOLTIP.starts_with("Type directly to the terminal (for example to answer a password prompt)"));
    }

    #[test]
    fn the_hint_shows_four_seconds_only_while_waiting_and_queueing() {
        let t0 = Instant::now();
        let mut s = State::default();
        s.typed(t0, false);
        assert!(!s.hint_shown(t0), "no hint when not waiting for the prompt");
        s.typed(t0, true);
        assert!(s.hint_shown(t0 + Duration::from_secs(3)));
        assert!(!s.hint_shown(t0 + Duration::from_secs(4)));
        s.typed(t0, true);
        assert!(s.toggle(), "on");
        assert!(!s.hint_shown(t0), "engaging the control removes the hint");
        s.typed(t0, true);
        assert!(!s.hint_shown(t0), "passthrough keystrokes are not held, so no hint");
        assert!(!s.toggle());
        s.typed(t0, true);
        s.stopped_waiting();
        assert!(!s.hint_shown(t0));
    }

    #[test]
    fn escape_and_ctrl_c_abort_a_waiting_paste_unless_keys_pass_through() {
        assert!(aborts(true, false, true, false) && aborts(true, false, false, true));
        assert!(!aborts(true, true, true, false), "passthrough: Esc reaches the shell");
        assert!(!aborts(false, false, true, false), "only while waiting for the prompt");
        assert!(queues_keys(true, false) && !queues_keys(true, true) && !queues_keys(false, false));
    }
}
