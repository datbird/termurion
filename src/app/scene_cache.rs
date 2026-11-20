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
    /// The cache's epoch when it was drawn (`invalidate`).
    epoch: u64,
}

pub(super) struct SceneCache<F> {
    kept: Option<Kept<F>>,
    /// Something other than the animation asked for a frame since the last take.
    changed: Cell<bool>,
    /// The animation asked for a frame since the last take.
    fx_asked: bool,
    /// Counts what makes the screen differ from the kept frame without the frame differing:
    /// a surface configure, a renderer setting, an effect or menu drawn, a window shown. A
    /// frame is handed back for comparing only while the epoch it was drawn in is current.
    epoch: Cell<u64>,
    /// The last renderer setting epoch seen (`sync_look`).
    look: Cell<u64>,
}

/// What `take` decides for a frame.
#[derive(Debug, PartialEq)]
pub(super) enum Take<F> {
    /// Draw this frame again, built at this time.
    Reuse(F, Instant),
    /// Build a new frame. `last` is the frame on screen when it can be compared with the new one.
    Build { last: Option<F> },
}

impl<F> Default for SceneCache<F> {
    fn default() -> Self {
        SceneCache { kept: None, changed: Cell::new(false), fx_asked: false, epoch: Cell::new(0), look: Cell::new(0) }
    }
}

impl<F> SceneCache<F> {
    /// A change, input or anything but the animation asked for a frame: the next one builds.
    pub(super) fn changed(&self) {
        self.changed.set(true);
    }

    /// The screen no longer shows exactly what the kept frame says (or may not): the next built
    /// frame draws, equal or not.
    pub(super) fn invalidate(&self) {
        self.epoch.set(self.epoch.get() + 1);
    }

    /// The renderer's setting epoch (`Renderer::look_epoch`): a change invalidates.
    pub(super) fn sync_look(&self, look: u64) {
        if self.look.replace(look) != look {
            self.invalidate();
        }
    }

    /// The animation asked for a frame.
    pub(super) fn fx_asked(&mut self) {
        self.fx_asked = true;
    }

    /// The screen shows the kept frame exactly: it was drawn in the current epoch, and
    /// nothing has been drawn over it or reconfigured since (`invalidate`).
    pub(super) fn shown(&self) -> bool {
        self.kept.as_ref().is_some_and(|k| k.epoch == self.epoch.get())
    }

    /// Whether `take` at this size and time would answer `Reuse`, using nothing up.
    pub(super) fn would_reuse(&self, size: (u32, u32), now: Instant) -> bool {
        self.fx_asked && !self.changed.get() && self.kept.as_ref().is_some_and(|k| k.size == size && now.saturating_duration_since(k.built) < MAX_AGE)
    }

    /// `Reuse` (the kept frame and when it was built) when only the animation asked for this
    /// frame, the window has the same size and the frame is younger than `MAX_AGE`. Else `Build`,
    /// carrying the kept frame only when an app request asked for this one at the same size and
    /// nothing was invalidated since it was drawn: the
    /// caller may find the new frame equal to it. Either way the kept frame and both requests are
    /// used up.
    pub(super) fn take(&mut self, size: (u32, u32), now: Instant) -> Take<F> {
        // Both are read every time: a short-circuit would leave a change standing after the
        // frame that showed it, and every later animation frame would build.
        let fx_asked = std::mem::take(&mut self.fx_asked);
        let changed = self.changed.replace(false);
        let fx_only = fx_asked && !changed;
        let Some(kept) = self.kept.take() else { return Take::Build { last: None } };
        if fx_only && kept.size == size && now.saturating_duration_since(kept.built) < MAX_AGE {
            return Take::Reuse(kept.frame, kept.built);
        }
        // A redraw nobody asked for (the system's) may show what the screen lost: it never skips.
        Take::Build { last: (changed && kept.size == size && kept.epoch == self.epoch.get()).then_some(kept.frame) }
    }

    /// Keeps the frame just drawn (`built` is when its scene was built, not when it was drawn).
    pub(super) fn keep(&mut self, frame: F, built: Instant, size: (u32, u32)) {
        self.kept = Some(Kept { frame, built, size, epoch: self.epoch.get() });
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
        assert_eq!(c.take((800, 600), t0 + Duration::from_millis(33)), Take::Reuse(7, t0));
    }

    #[test]
    fn a_frame_is_shown_until_something_invalidates_it() {
        let t0 = Instant::now();
        let mut c = SceneCache::<u32>::default();
        assert!(!c.shown(), "nothing drawn");
        c.keep(7, t0, (800, 600));
        assert!(c.shown());
        c.invalidate();
        assert!(!c.shown(), "the screen may differ from it");
        c.keep(7, t0, (800, 600));
        assert!(c.shown(), "drawn again");
    }

    #[test]
    fn would_reuse_matches_take_and_uses_nothing_up() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_millis(33);
        let mut c = kept(t0);
        assert!(!c.would_reuse((800, 600), later), "nothing asked");
        c.fx_asked();
        assert!(c.would_reuse((800, 600), later));
        assert!(c.would_reuse((800, 600), later), "asking twice changes nothing");
        assert!(!c.would_reuse((801, 600), later), "a new size");
        assert!(!c.would_reuse((800, 600), t0 + MAX_AGE), "an old frame");
        c.changed();
        assert!(!c.would_reuse((800, 600), later), "another request too");
        let mut c = kept(t0);
        c.fx_asked();
        assert!(c.would_reuse((800, 600), later));
        assert_eq!(c.take((800, 600), later), Take::Reuse(7, t0));
        c.fx_asked();
        assert!(!c.would_reuse((800, 600), later), "the frame was taken");
    }

    #[test]
    fn any_other_request_builds() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(7) });
        // Output and the animation in the same frame: the output wins.
        let mut c = kept(t0);
        c.fx_asked();
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(7) });
        // A frame nobody asked for (the system redrew the window) builds too.
        let mut c = kept(t0);
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None });
    }

    #[test]
    fn an_app_request_hands_back_the_last_frame() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(7) });
    }

    #[test]
    fn a_system_redraw_never_hands_it_back() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None });
    }

    #[test]
    fn a_new_size_hands_back_nothing() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((801, 600), t0), Take::Build { last: None });
    }

    #[test]
    fn a_new_size_or_an_old_frame_builds() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.fx_asked();
        assert_eq!(c.take((801, 600), t0), Take::Build { last: None });
        let mut c = kept(t0);
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0 + MAX_AGE), Take::Build { last: None });
    }

    #[test]
    fn each_take_uses_up_the_requests_and_the_frame() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(7) });
        // The change was used by that build: the next animation frame may reuse what it kept.
        c.keep(8, t0, (800, 600));
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0), Take::Reuse(8, t0));
        // Taken: nothing is kept until the drawn frame is kept again.
        c.fx_asked();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None });
    }

    #[test]
    fn a_reconfigure_at_the_same_size_hands_back_nothing() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.invalidate();
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None });
    }

    #[test]
    fn a_renderer_setting_change_hands_back_nothing() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.sync_look(0);
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(7) }, "same settings");
        c.keep(7, t0, (800, 600));
        c.sync_look(1);
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None }, "a setting changed");
    }

    #[test]
    fn a_frame_drawn_after_an_invalidate_is_comparable_again() {
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.invalidate();
        c.keep(9, t0, (800, 600));
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: Some(9) });
    }

    #[test]
    fn a_failed_draw_keeps_nothing() {
        // The caller keeps only a drawn frame: after a failure the next take has nothing.
        let t0 = Instant::now();
        let mut c = kept(t0);
        c.changed();
        let _ = c.take((800, 600), t0);
        c.invalidate();
        c.changed();
        assert_eq!(c.take((800, 600), t0), Take::Build { last: None });
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

    /// An animation frame has no frame work to do: these sit behind the `would_reuse` check.
    #[test]
    fn an_animation_frame_skips_the_frame_work() {
        let src = include_str!("mod.rs");
        let body = &src[src.find("fn render_frame(").unwrap()..];
        let body = &body[..body.find("\n/// iTerm2's button numbers").unwrap()];
        let at = |what: &str| body.find(what).unwrap_or_else(|| panic!("{what} in render_frame"));
        let ask = at("would_reuse(");
        for call in ["self.fx_sync()", "self.publish_caption()", "self.caption_text()", "self.sync_blur()", "title_sent.send(", "self.sync_menu_hidden()"] {
            let i = at(call);
            assert!(i > ask, "{call} comes after the would_reuse check");
            // The nearest line above that opens a block must be the will_reuse guard.
            let head = &body[..i];
            let guard = head.rfind("if !will_reuse {").into_iter().chain(head.rfind("if let Some(title) = &title {")).max();
            let guard = guard.unwrap_or_else(|| panic!("{call} sits in no guard"));
            assert!(!head[guard..].contains("\n        }\n"), "{call} is inside the guard, not after it");
        }
    }

    /// A still effect frame leaves the surface alone: the check comes before the drawable is
    /// asked for, after the effects' uniforms are packed, and every input that can change the
    /// picture without changing them is part of it.
    #[test]
    fn a_still_effect_frame_asks_for_no_drawable() {
        let src = include_str!("mod.rs");
        let body = &src[src.find("fn render_frame(").unwrap()..];
        let body = &body[..body.find("\n/// iTerm2's button numbers").unwrap()];
        let at = |what: &str| body.find(what).unwrap_or_else(|| panic!("{what} in render_frame"));
        let (prepare, still, acquire) = (at("fx.prepare("), at("skip_if_still("), at("get_current_texture()"));
        assert!(prepare < still && still < acquire, "prepare, then the still check, then the drawable");
        let check = &body[still.saturating_sub(300)..still];
        // A frame that gets no drawable still paces the next one.
        let acquire = &body[acquire..];
        let timeout = &acquire[acquire.find("Timeout | wgpu::CurrentSurfaceTexture::Occluded").unwrap()..];
        assert!(timeout[..timeout.find("return;").unwrap()].contains("pace_fx(fx_now, focused_window, fx_pacing, true)"));
        // Only an active chain may skip.
        assert!(body.contains("filter(|f| f.active()).and_then(|f| f.chain_mut())"));
        // A frame left out waits a frame gap before it asks again.
        assert!(body.contains("fx_pacing, true)"));
        // The menu bar's theme is read only when the bar draws.
        assert!(body.contains("let bar_dark = bar_draws && self.dark()"));
        // The sRGB view is made only when something reads it.
        assert!(body.contains("(!fx_raw || bar_draws).then("));
        for input in ["same_scene", "shown", "bar_busy", "alpha_changed", "frame.fade"] {
            assert!(check.contains(input), "the still check reads {input}");
        }
    }
}
