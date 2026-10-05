//! Reduce Flicker (iTerm2's temporary double buffer): while a program hides the cursor, the
//! pane keeps showing its last drawn picture for up to `HOLD`, so a half-drawn update is not
//! seen. Pure model; the app keeps the pictures (`Pane.held`) and draws them.

use crate::layout::{PaneId, Rect};
use crate::scene;
use std::time::{Duration, Instant};

/// How long a held picture may last (`kTimeToKeepSavedGrid`, iTermTemporaryDoubleBufferedGridController.m).
pub const HOLD: Duration = Duration::from_millis(200);

/// A pane's last live picture: its slice of the frame.
#[derive(Clone, Debug)]
pub struct Held {
    /// When the picture was drawn.
    pub since: Instant,
    /// The pane's rect and screen when it was drawn: a picture of another geometry or screen
    /// is not shown (iTerm2 resets the saved grid on a resize and on an alt screen switch).
    pub rect: Rect,
    pub alt_screen: bool,
    pub quads: Vec<scene::Quad>,
    pub images: Vec<scene::ImageQuad>,
    pub rows: Vec<scene::TextRow>,
    pub overlay: Vec<scene::Quad>,
    /// Underline and strikethrough strokes (they draw after the text, so they are part of the picture).
    pub underlines: Vec<scene::Quad>,
    pub strikes: Vec<scene::Quad>,
}

/// The oldest a held picture may be, measured back from the moment the cursor was hidden: the
/// longest frame period (an idle pane draws once a second). A pane in a background tab or a
/// hidden window keeps a picture from its last visit, which must never be shown.
pub const FRESH: Duration = Duration::from_secs(1);

impl Held {
    /// Whether this picture may stand in for the pane now: drawn for this rect and screen, and
    /// not older than `FRESH` before the cursor was hidden.
    pub fn usable(&self, rect: Rect, alt_screen: bool, hidden_at: Option<Instant>) -> bool {
        self.rect == rect && self.alt_screen == alt_screen && hidden_at.is_some_and(|t| t.saturating_duration_since(self.since) <= FRESH)
    }
}

/// Whether the pane shows its held picture now: Reduce Flicker is on, the cursor has been
/// hidden, and less than `HOLD` has passed since.
pub fn holding(enabled: bool, hidden_at: Option<Instant>, now: Instant) -> bool {
    enabled && hidden_at.is_some_and(|t| now.saturating_duration_since(t) < HOLD)
}

/// Ends the holds that are due at `now`; returns those panes, so each one's own window can be
/// asked for a frame (a pane of a parked window is released too).
pub fn release_holds<'a>(panes: impl Iterator<Item = (PaneId, &'a mut Option<Instant>)>, now: Instant) -> Vec<PaneId> {
    let mut out = Vec::new();
    for (id, until) in panes {
        if until.is_some_and(|t| t <= now) {
            *until = None;
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_hold_follows_iterm2() {
        let t = Instant::now();
        assert!(holding(true, Some(t), t + Duration::from_millis(150)));
        assert!(!holding(true, Some(t), t + Duration::from_millis(200)));
        assert!(!holding(true, Some(t), t + Duration::from_millis(900)));
        assert!(!holding(false, Some(t), t));
        assert!(!holding(true, None, t));
    }

    fn rect(w: f32) -> Rect {
        Rect { x: 0.0, y: 0.0, w, h: 100.0 }
    }

    fn held(since: Instant, r: Rect, alt: bool) -> Held {
        Held { since, rect: r, alt_screen: alt, quads: vec![], images: vec![], rows: vec![], overlay: vec![], underlines: vec![], strikes: vec![] }
    }

    /// Final review 1: every due hold is released and named, the others stay.
    #[test]
    fn release_holds_names_the_panes_whose_hold_ended() {
        let t = Instant::now();
        let (mut a, mut b, mut c) = (Some(t), Some(t + Duration::from_millis(150)), None);
        let released = release_holds([(1, &mut a), (2, &mut b), (3, &mut c)].into_iter(), t + Duration::from_millis(10));
        assert_eq!(released, vec![1]);
        assert_eq!((a, b.is_some(), c), (None, true, None));
    }

    /// Final review 2: a held picture is shown only for its own geometry and screen, and only
    /// when it is recent at the time the cursor was hidden.
    #[test]
    fn a_held_picture_is_used_only_while_it_still_fits() {
        let t = Instant::now();
        let hidden = Some(t + Duration::from_millis(30));
        let h = held(t, rect(500.0), false);
        assert!(h.usable(rect(500.0), false, hidden));
        assert!(!h.usable(rect(400.0), false, hidden), "a resize ends the hold");
        assert!(!h.usable(rect(500.0), true, hidden), "an alt screen switch ends the hold");
        let old = held(t, rect(500.0), false);
        assert!(!old.usable(rect(500.0), false, Some(t + FRESH + Duration::from_millis(1))), "a picture from a past visit is not shown");
        assert!(!h.usable(rect(500.0), false, None));
    }
}
