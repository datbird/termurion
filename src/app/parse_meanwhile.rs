//! Output parsed on a second thread while the main thread draws a frame (Parse While Drawing).
//! `render_frame` lends the panes to one scoped worker for the length of the draw; the work
//! that follows output (`App::pump_tail`) runs on the main thread after it, as after a slice.

use crate::layout::PaneId;
use crate::pane::{Pane, Pump};

// The worker borrows the panes across a thread, so a pane must stay `Send`.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<crate::pane::Pane>();
};

/// The settings each pane takes before its output is read.
#[derive(Clone, Copy, Debug)]
pub(super) struct PumpPrep {
    pub auto_composer: bool,
    pub smart_logging: bool,
    pub always_line_marks: bool,
}

/// The prompt state machine follows View > Auto Composer before it reads output (the Task 3
/// contract: every pane, new ones included).
pub(super) fn prep_for_pump(p: &mut Pane, prep: &PumpPrep) {
    p.own_mut().prompt_state.enabled = prep.auto_composer;
    p.log_compact = (prep.smart_logging, prep.auto_composer);
    // New prompt marks are line-style with Auto Composer or AlwaysUseLineStyleMarks
    // (`S:18934`); marks already made keep their style.
    p.screen.line_style_marks = prep.auto_composer || prep.always_line_marks;
}

/// What one turn of parsing did, for `App::pump_tail`.
#[derive(Debug, Default)]
pub(super) struct Parsed {
    /// Each pane parsed, once, in the order it was parsed.
    pub results: Vec<(PaneId, Pump)>,
    /// Output is still unread.
    pub owed: bool,
    /// The first pane in the rotation that ran out of time (counted from `start`).
    pub first_more: Option<usize>,
    /// Where the rotation started (`App::pump_from`).
    pub start: usize,
}

impl Parsed {
    /// Records pane number `k` of the rotation.
    pub fn note(&mut self, k: usize, id: PaneId, r: Pump) {
        self.owed |= r.more;
        if r.more && self.first_more.is_none() {
            self.first_more = Some(k);
        }
        self.results.push((id, r));
    }
}
