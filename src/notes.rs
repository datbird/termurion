//! The Notes tool's store, pure rules: its two modes and when a change to the global note is
//! written (iTerm2's `Toolbelt/ToolNotes.m`, `TN`). The editor is `text_edit`; the app owns the
//! file under `%APPDATA%\Tandem`, the session note on the Pane and the drawing.

use std::time::{Duration, Instant};

/// Which note the tool edits (`TN:91-99`): the one global note, or the current session's note
/// (the one Edit Session Note shows). Global first; the choice is not saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Global,
    Session,
}

/// The mode control's segments, in order (`TN:94`).
pub const MODES: [(Mode, &str); 2] = [(Mode::Global, "Global"), (Mode::Session, "Session")];

/// The global note's file, plain UTF-8 text beside Tandem's other files (iTerm2 keeps
/// `notes.rtfd`; ruling: Tandem's editor is plain text only).
pub const FILE_NAME: &str = "notes.txt";

/// A global note this long or longer is written on a trailing debounce instead of on every
/// change (`TN:554`, `100 * 1024` characters).
pub const LARGE: usize = 100 * 1024;

/// The trailing debounce for a large global note (`TN:598`, 2.0 s after the last change).
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// When the global note is written: at once for a note under `LARGE`, else `DEBOUNCE` after
/// the last change (`TN:554-598`). `dirty` marks an armed write not yet made, which closing the
/// tool or switching its mode flushes.
#[derive(Clone, Debug, Default)]
pub struct Saver {
    due: Option<Instant>,
    dirty: bool,
}

impl Saver {
    /// The global note changed and is now `chars` long (`length`). True: write it now (this
    /// also disarms a pending write, the note shrank). False: a write is armed for
    /// `DEBOUNCE` from `now`, replacing any earlier one.
    pub fn changed(&mut self, chars: usize, now: Instant) -> bool {
        if chars < LARGE {
            self.due = None;
            self.dirty = false;
            true
        } else {
            self.due = Some(now + DEBOUNCE);
            self.dirty = true;
            false
        }
    }

    /// When the armed write is due, for the app's wake-up timer.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// True once, when the armed write is due at `now`: write the note.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(d) if now >= d => {
                self.due = None;
                self.dirty = false;
                true
            }
            _ => false,
        }
    }

    /// True when an armed write was still pending (the tool is closing or leaving Global
    /// mode): write the note now. Disarms it.
    pub fn flush(&mut self) -> bool {
        let dirty = self.dirty;
        self.due = None;
        self.dirty = false;
        dirty
    }
}

/// A note's length as `LARGE` counts it: `NSString` length, UTF-16 units (`TN:554` reads
/// `textStorage.length`).
pub fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_save_like_iterm2() {
        assert_eq!(MODES.map(|(_, name)| name), ["Global", "Session"]);
        assert_eq!(MODES[0].0, Mode::default());
        assert_eq!((FILE_NAME, LARGE, DEBOUNCE), ("notes.txt", 102_400, Duration::from_secs(2)));

        // Under LARGE characters every change writes at once and arms nothing.
        let t0 = Instant::now();
        let mut s = Saver::default();
        assert!(s.changed(LARGE - 1, t0));
        assert_eq!(s.due(), None);
        assert!(!s.take_due(t0 + Duration::from_secs(10)));
        assert!(!s.flush());

        // At LARGE or more: a trailing debounce, 2 s after the last change.
        assert!(!s.changed(LARGE, t0));
        assert_eq!(s.due(), Some(t0 + DEBOUNCE));
        let t1 = t0 + Duration::from_millis(1500);
        assert!(!s.changed(LARGE + 5, t1));
        assert_eq!(s.due(), Some(t1 + DEBOUNCE));
        assert!(!s.take_due(t0 + DEBOUNCE));
        assert!(s.take_due(t1 + DEBOUNCE));
        assert_eq!(s.due(), None);
        assert!(!s.take_due(t1 + DEBOUNCE * 2));

        // Shrinking under LARGE while a write is armed writes now and disarms it.
        assert!(!s.changed(LARGE, t0));
        assert!(s.changed(10, t1));
        assert_eq!(s.due(), None);

        // Closing or switching mode flushes an armed write once.
        assert!(!s.changed(LARGE, t0));
        assert!(s.flush());
        assert_eq!(s.due(), None);
        assert!(!s.flush());

        // Length counts like NSString: UTF-16 units, not bytes.
        assert_eq!(length("a\u{e9}"), 2);
        assert_eq!(length("\u{1F600}"), 2);
    }
}
