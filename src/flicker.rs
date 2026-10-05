//! Reduce Flicker (iTerm2's temporary double buffer): while a program hides the cursor, the
//! pane keeps showing its last drawn picture for up to `HOLD`, so a half-drawn update is not
//! seen. Pure model; the app keeps the pictures (`Pane.held`) and draws them.

use crate::scene;
use std::time::{Duration, Instant};

/// How long a held picture may last (`kTimeToKeepSavedGrid`, iTermTemporaryDoubleBufferedGridController.m).
pub const HOLD: Duration = Duration::from_millis(200);

/// A pane's last live picture: its slice of the frame.
#[derive(Clone, Debug)]
pub struct Held {
    pub since: Instant,
    pub quads: Vec<scene::Quad>,
    pub images: Vec<scene::ImageQuad>,
    pub rows: Vec<scene::TextRow>,
    pub overlay: Vec<scene::Quad>,
    /// Underline and strikethrough strokes (they draw after the text, so they are part of the picture).
    pub underlines: Vec<scene::Quad>,
    pub strikes: Vec<scene::Quad>,
}

/// Whether the pane shows its held picture now: Reduce Flicker is on, the cursor has been
/// hidden, and less than `HOLD` has passed since.
pub fn holding(enabled: bool, hidden_at: Option<Instant>, now: Instant) -> bool {
    enabled && hidden_at.is_some_and(|t| now.saturating_duration_since(t) < HOLD)
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
}
