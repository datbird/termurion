//! Profiles > Advanced > Triggers: iTerm2's list and detail editor.

use std::cell::Cell;

thread_local! {
    /// Which row the editor selects next time it draws: set by Add Trigger... and Edit
    /// Triggers (the last row, or the first).
    static FOCUS: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Add Trigger...: the editor shows the new (last) trigger.
pub fn focus_last() {
    FOCUS.with(|f| f.set(Some(true)));
}

/// Edit Triggers: the editor shows the first trigger.
pub fn focus_first() {
    FOCUS.with(|f| f.set(Some(false)));
}

/// The row the editor should select now, once.
pub fn take_focus(len: usize) -> Option<usize> {
    let f = FOCUS.with(|f| f.take())?;
    (len > 0).then(|| if f { len - 1 } else { 0 })
}
