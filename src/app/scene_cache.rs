//! The frame a window built last, kept for the frames only an effect's animation wants. Those
//! draw it again and skip the scene build, as Ghostty draws its animation frames without
//! updating its cells. Anything else that asks the window to draw builds a new frame.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// A kept frame is built again once it is this old, whatever asked. A change that asked for no
/// frame of its own still shows within this time.
pub(super) const MAX_AGE: Duration = Duration::from_millis(500);

struct Kept<F> {
    frame: F,
    built: Instant,
    size: (u32, u32),
}

pub(super) struct SceneCache<F> {
    kept: Option<Kept<F>>,
    /// Something other than the animation asked for a frame since the last take.
    changed: Cell<bool>,
    /// The animation asked for a frame since the last take.
    fx_asked: bool,
}

impl<F> Default for SceneCache<F> {
    fn default() -> Self {
        SceneCache { kept: None, changed: Cell::new(false), fx_asked: false }
    }
}

impl<F> SceneCache<F> {
    /// A change, input or anything but the animation asked for a frame: the next one builds.
    pub(super) fn changed(&self) {
        self.changed.set(true);
    }

    /// The animation asked for a frame.
    pub(super) fn fx_asked(&mut self) {
        self.fx_asked = true;
    }

    /// The kept frame and when it was built, when only the animation asked for this frame, the
    /// window has the same size and the frame is younger than `MAX_AGE`. Else None, and the
    /// caller builds. Either way the kept frame and both requests are used up.
    pub(super) fn take(&mut self, size: (u32, u32), now: Instant) -> Option<(F, Instant)> {
        // Both are read every time: a short-circuit would leave a change standing after the
        // frame that showed it, and every later animation frame would build.
        let fx_asked = std::mem::take(&mut self.fx_asked);
        let changed = self.changed.replace(false);
        let fx_only = fx_asked && !changed;
        let kept = self.kept.take()?;
        (fx_only && kept.size == size && now.saturating_duration_since(kept.built) < MAX_AGE).then_some((kept.frame, kept.built))
    }

    /// Keeps the frame just drawn (`built` is when its scene was built, not when it was drawn).
    pub(super) fn keep(&mut self, frame: F, built: Instant, size: (u32, u32)) {
        self.kept = Some(Kept { frame, built, size });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(t0: Instant) -> SceneCache<u32> {
        let mut c = SceneCache::default();
        c.keep(7, t0, (800, 600));
        c
    }

    #[test]
    fn an_animation_frame_draws_the_kept_frame() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0 + Duration::from_millis(33)), Some((7, t0)));
    }

    #[test]
    fn any_other_request_builds() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), None);
        // Output and the animation in the same frame: the output wins.
        let mut c = kept(t0);
        c.fx_asked();
        c.changed();
        assert_eq!(c.take((800, 600), t0), None);
        // A frame nobody asked for (the system redrew the window) builds too.
        let mut c = kept(t0);
        assert_eq!(c.take((800, 600), t0), None);
    }

    #[test]
    fn a_new_size_or_an_old_frame_builds() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.fx_asked();
        assert_eq!(c.take((801, 600), t0), None);
        let mut c = kept(t0);
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0 + MAX_AGE), None);
    }

    #[test]
    fn each_take_uses_up_the_requests_and_the_frame() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), None);
        // The change was used by that build: the next animation frame may reuse what it kept.
        c.keep(8, t0, (800, 600));
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0), Some((8, t0)));
        // Taken: nothing is kept until the drawn frame is kept again.
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0), None);
    }

    /// Every request for a terminal window's frame goes through `Gpu::redraw` or
    /// `Gpu::redraw_fx`, so the cache hears it. A direct `window.request_redraw()` on a Gpu
    /// would let an animation frame draw over a change with the old frame.
    #[test]
    fn terminal_windows_redraw_only_through_the_gpu() {
        let files = [
            ("mod.rs", include_str!("mod.rs")),
            ("frames_ui.rs", include_str!("frames_ui.rs")),
            ("windows_ui.rs", include_str!("windows_ui.rs")),
            ("menu_bar_ui.rs", include_str!("menu_bar_ui.rs")),
            ("tab_drag_ui.rs", include_str!("tab_drag_ui.rs")),
            ("gpu_recovery_ui.rs", include_str!("gpu_recovery_ui.rs")),
            ("fx_ui.rs", include_str!("fx_ui.rs")),
            ("metrics_ui.rs", include_str!("metrics_ui.rs")),
        ];
        for (name, src) in files {
            for bad in ["g.window.request_redraw()", "gpu.window.request_redraw()"] {
                assert!(!src.contains(bad), "{name} calls {bad}; use redraw() or redraw_fx()");
            }
        }
    }
}
