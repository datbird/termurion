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
    /// The renderer face and cell the picture was drawn in: a zoom, a font edit or a pane moved
    /// to another window changes them, and the old picture is then not shown.
    pub face: u16,
    pub cell: (f32, f32),
    pub quads: Vec<scene::Quad>,
    pub images: Vec<scene::ImageQuad>,
    pub rows: Vec<scene::TextRow>,
    pub overlay: Vec<scene::Quad>,
    /// Underline and strikethrough strokes (they draw after the text, so they are part of the picture).
    pub underlines: Vec<scene::Quad>,
    pub strikes: Vec<scene::Quad>,
    /// Box and block glyphs (they draw with the text, so a hold must keep them).
    pub glyphs: Vec<scene::CellGlyph>,
}

/// Where a pane's own part of the frame starts: the length of each list before it draws.
#[derive(Clone, Copy, Debug)]
pub struct Marks {
    quads: usize,
    images: usize,
    rows: usize,
    overlay: usize,
    underlines: usize,
    strikes: usize,
    glyphs: usize,
}

impl Marks {
    pub fn of(frame: &scene::Frame) -> Marks {
        Marks { quads: frame.quads.len(), images: frame.images.len(), rows: frame.rows.len(), overlay: frame.overlay.len(), underlines: frame.underlines.len(), strikes: frame.strikes.len(), glyphs: frame.glyphs.len() }
    }
}

/// The oldest a held picture may be, measured back from the moment the cursor was hidden: the
/// longest frame period (an idle pane draws once a second). A pane in a background tab or a
/// hidden window keeps a picture from its last visit, which must never be shown.
pub const FRESH: Duration = Duration::from_secs(1);

impl Held {
    /// The pane's slice of `frame`: everything drawn since `from`.
    pub fn slice(frame: &scene::Frame, from: &Marks, since: Instant, rect: Rect, alt_screen: bool, face: u16, cell: (f32, f32)) -> Held {
        Held {
            since,
            rect,
            alt_screen,
            face,
            cell,
            quads: frame.quads[from.quads..].to_vec(),
            images: frame.images[from.images..].to_vec(),
            rows: frame.rows[from.rows..].to_vec(),
            overlay: frame.overlay[from.overlay..].to_vec(),
            underlines: frame.underlines[from.underlines..].to_vec(),
            strikes: frame.strikes[from.strikes..].to_vec(),
            glyphs: frame.glyphs[from.glyphs.min(frame.glyphs.len())..].to_vec(),
        }
    }

    /// Draws the held picture into `frame` again.
    pub fn replay(&self, frame: &mut scene::Frame) {
        if self.face != crate::render::DEFAULT_FACE && !frame.face_heights.iter().any(|(f, _)| *f == self.face) {
            frame.face_heights.push((self.face, self.cell.1));
        }
        frame.quads.extend(self.quads.iter().copied());
        frame.images.extend(self.images.iter().cloned());
        frame.rows.extend(self.rows.iter().cloned());
        frame.overlay.extend(self.overlay.iter().copied());
        frame.underlines.extend(self.underlines.iter().copied());
        frame.strikes.extend(self.strikes.iter().copied());
        frame.glyphs.extend(self.glyphs.iter().cloned());
    }

    /// Whether this picture may stand in for the pane now: drawn for this rect, screen, face and
    /// cell, and not older than `FRESH` before the cursor was hidden.
    pub fn usable(&self, rect: Rect, alt_screen: bool, face: u16, cell: (f32, f32), hidden_at: Option<Instant>) -> bool {
        self.rect == rect && self.alt_screen == alt_screen && self.face == face && self.cell == cell && hidden_at.is_some_and(|t| t.saturating_duration_since(self.since) <= FRESH)
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
        Held { since, rect: r, alt_screen: alt, face: 0, cell: (8.0, 16.0), quads: vec![], images: vec![], rows: vec![], overlay: vec![], underlines: vec![], strikes: vec![], glyphs: vec![] }
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
        let cell = (8.0, 16.0);
        assert!(h.usable(rect(500.0), false, 0, cell, hidden));
        assert!(!h.usable(rect(400.0), false, 0, cell, hidden), "a resize ends the hold");
        assert!(!h.usable(rect(500.0), true, 0, cell, hidden), "an alt screen switch ends the hold");
        let old = held(t, rect(500.0), false);
        assert!(!old.usable(rect(500.0), false, 0, cell, Some(t + FRESH + Duration::from_millis(1))), "a picture from a past visit is not shown");
        assert!(!h.usable(rect(500.0), false, 0, cell, None));
        // 13n final 5: a per-pane zoom (another cell), a font edit or a move to another window
        // (another face) ends the hold too.
        assert!(!h.usable(rect(500.0), false, 0, (16.0, 32.0), hidden), "a zoom ends the hold");
        assert!(!h.usable(rect(500.0), false, 3, cell, hidden), "another face ends the hold");
    }

    /// B1(c): a Reduce Flicker hold keeps the pane's box glyphs, and only its own.
    #[test]
    fn held_slice_keeps_box_glyphs() {
        let g = |x: f32| scene::CellGlyph { c: '\u{2500}', rect: Rect { x, y: 0.0, w: 10.0, h: 20.0 }, color: crate::palette::rgb(1, 2, 3), face: 0, clip: rect(100.0) };
        let mut frame = scene::Frame::default();
        frame.glyphs.push(g(500.0));
        let from = Marks::of(&frame);
        frame.glyphs.push(g(0.0));
        frame.glyphs.push(g(10.0));
        let t = Instant::now();
        let held = Held::slice(&frame, &from, t, rect(100.0), false, 0, (8.0, 16.0));
        assert_eq!(held.glyphs, vec![g(0.0), g(10.0)], "the slice has the pane's glyphs, not the ones drawn before it");
        let mut next = scene::Frame::default();
        held.replay(&mut next);
        assert_eq!(next.glyphs, held.glyphs, "a hold shows the box glyphs again");
    }
}
