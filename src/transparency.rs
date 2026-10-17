//! Window transparency rules (pure): per-pane alpha, the window toggle, the backdrop quads,
//! the premultiplied color, the surface alpha mode and the composited pixel.

use crate::layout::Rect;
use crate::scene::Quad;

/// The arrangement key that carries a window's Use Transparency state.
pub const USE_KEY: &str = "Use Transparency";
/// Global: drop transparency while a window is full screen.
pub const DISABLE_FULLSCREEN_KEY: &str = "DisableFullscreenTransparency";
/// Global: the focused window is always opaque.
pub const DISABLE_KEY_WINDOW_KEY: &str = "DisableTransparencyForKeyWindow";
pub const DEFAULT_BLUR_RADIUS: f64 = 2.0;
pub const MAX_BLUR_RADIUS: f64 = 64.0;
pub const LARGE_RADIUS_LIMIT: f64 = 26.0;

/// A pane's background alpha (PTYTextView.m:2726-2731): 1 in the key window when it is forced opaque,
/// else `1 - transparency` while the window uses transparency, else 1. Transparency is clamped to 0..=1.
pub fn pane_alpha(transparency: f64, window_uses: bool, key_window: bool, disable_for_key: bool) -> f32 {
    if key_window && disable_for_key || !window_uses {
        return 1.0;
    }
    (1.0 - transparency.clamp(0.0, 1.0)) as f32
}

/// A window's Use Transparency state and its full screen bookkeeping
/// (PseudoTerminal.m useTransparency_, oldUseTransparency_, restoreUseTransparency_).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinTransparency {
    pub on: bool,
    old: bool,
    restore: bool,
}

impl WinTransparency {
    pub fn new(initial: bool) -> Self {
        WinTransparency { on: initial, old: false, restore: false }
    }

    /// A window restored from an arrangement: the saved key wins only when present (PT:4189-4191).
    pub fn restored(saved: Option<bool>, initial: bool) -> Self {
        Self::new(saved.unwrap_or(initial))
    }

    /// A full screen window restored from an arrangement (PT:3656-3659).
    /// The saved key is applied on top when present (PT:4189-4191), keeping the bookkeeping.
    pub fn restored_fullscreen(saved: Option<bool>, disable_pref: bool) -> Self {
        let mut t = WinTransparency { on: !disable_pref, old: true, restore: true };
        if let Some(s) = saved {
            t.on = s;
        }
        t
    }

    /// The menu item and Win+U: flips the state and forgets any pending restore.
    pub fn toggle(&mut self) {
        self.on = !self.on;
        self.restore = false;
    }

    /// Runs before the window enters or leaves full screen (PseudoTerminal+WindowStyle.m:587-600).
    pub fn before_fullscreen_toggle(&mut self, full_screen_now: bool, disable_pref: bool) {
        if !full_screen_now && disable_pref {
            self.old = self.on;
            self.on = false;
            self.restore = true;
        } else if full_screen_now && self.restore {
            self.on = self.old;
            self.restore = false;
        } else {
            self.restore = false;
        }
    }
}

/// A new window's Use Transparency: the first profile's Initial Use Transparency (PT:864), the default
/// profile's when the window has none.
pub fn initial_use(first: Option<&crate::profile::Profile>, default: &crate::profile::Profile) -> bool {
    first.unwrap_or(default).bool("Initial Use Transparency")
}

/// The check marks of the two View items (AD:700-702, AD:580-584).
pub fn checks(window_on: bool, disable_for_key: bool) -> [(&'static str, bool); 2] {
    [("toggleUseTransparency:", window_on), ("toggleDisableTransparencyForActiveWindow:", disable_for_key)]
}

/// True when any pane draws below full opacity, so the surface must be premultiplied.
pub fn needs_alpha(alphas: &[f32]) -> bool {
    alphas.iter().any(|a| *a < 1.0)
}

/// The sRGB transfer function, decoding 0..=1 to linear light (the curve of `scene::srgb_to_linear`).
pub fn linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// A quad color whose pipeline blend leaves exactly premultiplied pixels in an sRGB target:
/// `ALPHA_BLENDING` writes `linear(c * a)` in linear space, which the target stores as `c * a`.
pub fn premultiplied(srgb: [u8; 3], alpha: f32) -> [f32; 4] {
    let a = alpha.clamp(0.0, 1.0);
    if a == 0.0 {
        return [0.0; 4];
    }
    let c = |v: u8| linear(v as f32 / 255.0 * a) / a;
    [c(srgb[0]), c(srgb[1]), c(srgb[2]), a]
}

/// The first layer of a see-through window: chrome strips around the content (opaque), then each pane's
/// margin rects at the pane's color. Zero-size strips are skipped. The grid itself is drawn by the pane's
/// own background runs, so every pixel gets exactly one background layer.
pub fn backdrop(window: Rect, content: Rect, chrome: [f32; 4], panes: &[(Vec<Rect>, [f32; 4])]) -> Vec<Quad> {
    let mut out = Vec::new();
    let mut strip = |rect: Rect| {
        if rect.w > 0.0 && rect.h > 0.0 {
            out.push(Quad { rect, color: chrome });
        }
    };
    strip(Rect { x: window.x, y: window.y, w: window.w, h: content.y - window.y });
    strip(Rect { x: window.x, y: content.y + content.h, w: window.w, h: window.y + window.h - (content.y + content.h) });
    strip(Rect { x: window.x, y: content.y, w: content.x - window.x, h: content.h });
    strip(Rect { x: content.x + content.w, y: content.y, w: window.x + window.w - (content.x + content.w), h: content.h });
    for (margins, color) in panes {
        for m in margins {
            if m.w > 0.0 && m.h > 0.0 {
                out.push(Quad { rect: *m, color: *color });
            }
        }
    }
    out
}

/// The glyph (Segoe Fluent warning) shown before the large radius warning; iTerm2's emoji is dropped.
pub const WARNING_GLYPH: char = '\u{E7BA}';
/// The note under the Transparency slider (PreferencePanel.xib).
pub const OVERRIDE_NOTICE: &str = "View > Use Transparency can override this setting temporarily per window.";
/// The warning under Blur Radius (PreferencePanel.xib).
pub const LARGE_RADIUS: &str = "Large radius may reduce performance";

/// One session's blur facts: its profile's Transparency, Blur and Blur Radius (None when unset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurInput {
    pub transparency: f64,
    pub blur: bool,
    pub radius: Option<f64>,
}

/// The tab blurs when any session has Transparency above 0 and Blur on while the window uses
/// transparency (PTYTab.m:3027-3037).
pub fn tab_blur(sessions: &[BlurInput], window_uses: bool) -> bool {
    window_uses && sessions.iter().any(|s| s.transparency > 0.0 && s.blur)
}

/// The blur radius: the average over the sessions that blur (a missing radius counts 2.0), capped at 64;
/// 2.0 when none blurs (PTYTab.m:3039-3055). Acrylic has no radius, so the value is only computed.
pub fn tab_blur_radius(sessions: &[BlurInput]) -> f64 {
    let blurred: Vec<f64> = sessions.iter().filter(|s| s.transparency > 0.0 && s.blur).map(|s| s.radius.unwrap_or(DEFAULT_BLUR_RADIUS)).collect();
    if blurred.is_empty() {
        return DEFAULT_BLUR_RADIUS;
    }
    (blurred.iter().sum::<f64>() / blurred.len() as f64).min(MAX_BLUR_RADIUS)
}

/// Whether a Window tab row is enabled (PWP:157-170, 179-189): Keep background colors opaque and Blur need
/// Transparency above 0; Blur Radius needs Blur as well.
pub fn row_enabled(key: &str, transparency: f64, blur: bool) -> bool {
    match key {
        "Only The Default BG Color Uses Transparency" | "Blur" => transparency > 0.0,
        "Blur Radius" => transparency > 0.0 && blur,
        _ => true,
    }
}

/// The large radius warning shows while Blur is on and the radius is above 26 (PWP:762-769).
pub fn radius_warning(blur: bool, radius: f64) -> bool {
    blur && radius > LARGE_RADIUS_LIMIT
}

/// The surface alpha mode: premultiplied when something is see-through and the surface offers it,
/// opaque when nothing is; otherwise the first mode offered.
pub fn pick_alpha_mode(modes: &[wgpu::CompositeAlphaMode], transparent: bool) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode as M;
    let want = if transparent { M::PreMultiplied } else { M::Opaque };
    if modes.contains(&want) {
        want
    } else {
        modes.first().copied().unwrap_or(M::Auto)
    }
}

/// True when a window showing these profiles (Transparency, Blur) needs an alpha surface. Only Transparency
/// above 0 counts: Blur alone never makes a pane see-through (pre-flight P5). The window's Use Transparency
/// toggle is ignored so Win+U never needs a recreate.
pub fn wants_alpha_surface(profiles: &[(f64, bool)]) -> bool {
    profiles.iter().any(|(t, _)| *t > 0.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceKind {
    #[default]
    Opaque,
    Alpha,
}

/// An opaque window that now needs alpha must be recreated; an alpha window never goes back
/// (it stays on the second instance with an opaque alpha mode).
pub fn needs_recreate(current: SurfaceKind, profiles: &[(f64, bool)]) -> bool {
    current == SurfaceKind::Opaque && wants_alpha_surface(profiles)
}

/// An opaque window is recreated only when it needs alpha and a try has not already failed for it.
pub fn recreate_due(current: SurfaceKind, profiles: &[(f64, bool)], failed: bool) -> bool {
    !failed && needs_recreate(current, profiles)
}

/// What a recreated window takes over from the one it replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carry {
    pub fullscreen: bool,
    pub on_top: bool,
    pub show: bool,
    pub activate: bool,
    /// The old window was minimized: the new one comes back minimized (shown without activation).
    pub minimized: bool,
    /// The old window was maximized (or minimized from maximized): the new one maximizes over its normal frame.
    pub maximized: bool,
}

/// `WINDOWPLACEMENT.showCmd` values that mean minimized, and `WPF_RESTORETOMAXIMIZED`.
const MINIMIZED_CMDS: [u32; 4] = [2, 6, 7, 11];
const SW_SHOWMAXIMIZED: u32 = 3;
const WPF_RESTORETOMAXIMIZED: u32 = 2;

/// A window's placement as `GetWindowPlacement` reports it: its normal (restore) frame as left, top, right,
/// bottom, in workspace coordinates unless the window is a tool window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The normal frame in screen coordinates.
    pub frame: [i32; 4],
    pub minimized: bool,
    pub maximized: bool,
}

/// Reads a `WINDOWPLACEMENT` (`show_cmd`, `flags`, normal rect) into the state a recreate carries.
/// `shift` is the workspace origin relative to the screen (taskbar on the top or left), added to the
/// frame; zero for a tool window, whose placement is already in screen coordinates.
pub fn placed(show_cmd: u32, flags: u32, normal: [i32; 4], shift: (i32, i32)) -> Placed {
    let minimized = MINIMIZED_CMDS.contains(&show_cmd);
    let maximized = show_cmd == SW_SHOWMAXIMIZED || (minimized && flags & WPF_RESTORETOMAXIMIZED != 0);
    Placed { frame: [normal[0] + shift.0, normal[1] + shift.1, normal[2] + shift.0, normal[3] + shift.1], minimized, maximized }
}

/// Full screen and the floating level ride along; the new window shows only if the old one was shown and
/// takes focus only if the old one had it.
pub fn carry(fullscreen: bool, accessory: bool, floating_hotkey: bool, visible: bool, focused: bool, placed: &Placed) -> Carry {
    Carry { fullscreen, on_top: accessory || floating_hotkey, show: visible, activate: visible && focused && !placed.minimized, minimized: placed.minimized, maximized: placed.maximized }
}

/// Takes a window that the old HWND owns out of its slot, so the slot is clear and it can open again.
pub fn drop_owned<T>(slot: &mut Option<T>) -> Option<T> {
    slot.take()
}

/// The pixel at (x, y) after drawing `quads` in order over nothing with the pipelines' `ALPHA_BLENDING`.
pub fn composite(quads: &[Quad], x: f32, y: f32) -> [f32; 4] {
    let mut px = [0.0f32; 4];
    for q in quads.iter().filter(|q| q.rect.contains(x, y)) {
        let a = q.color[3];
        for (p, c) in px.iter_mut().zip(q.color).take(3) {
            *p = c * a + *p * (1.0 - a);
        }
        px[3] = a + px[3] * (1.0 - a);
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recreate_carries_the_old_window() {
        let normal = placed(1, 0, [10, 20, 810, 620], (0, 0));
        // Full screen, floating and visibility ride along; activation only when the old window had focus.
        let c = |fullscreen, on_top, show, activate| Carry { fullscreen, on_top, show, activate, minimized: false, maximized: false };
        assert_eq!(carry(true, false, false, true, false, &normal), c(true, false, true, false));
        assert_eq!(carry(false, true, false, true, true, &normal), c(false, true, true, true));
        assert!(carry(false, false, true, true, false, &normal).on_top, "a shown floating hotkey window");
        let hidden = carry(false, false, false, false, true, &normal);
        assert!(!hidden.show && !hidden.activate, "a hidden window stays hidden and never takes focus");
    }

    #[test]
    fn recreate_keeps_minimized_and_maximized_placement() {
        // A minimized window reads its normal frame, never the (-32000, -32000) of its iconic rect.
        let min = placed(2, 0, [100, 50, 900, 650], (0, 0));
        assert_eq!(min, Placed { frame: [100, 50, 900, 650], minimized: true, maximized: false });
        let c = carry(false, false, false, true, true, &min);
        assert!(c.minimized && c.show && !c.activate, "comes back minimized and never takes focus");
        // Minimized from maximized restores to maximized.
        assert!(placed(7, 2, [0, 0, 10, 10], (0, 0)).maximized);
        // A maximized window keeps its restore rectangle.
        let max = placed(3, 0, [100, 50, 900, 650], (0, 0));
        assert!(max.maximized && !max.minimized);
        assert_eq!(max.frame, [100, 50, 900, 650]);
        assert!(carry(false, false, false, true, true, &max).maximized);
        // A taskbar on the top shifts workspace coordinates onto the screen.
        assert_eq!(placed(1, 0, [0, 0, 800, 600], (0, 40)).frame, [0, 40, 800, 640]);
        let n = placed(1, 0, [0, 0, 1, 1], (0, 0));
        assert!(!n.minimized && !n.maximized);
    }

    #[test]
    fn failed_alpha_window_is_not_retried() {
        let want = [(0.3, false)];
        assert!(recreate_due(SurfaceKind::Opaque, &want, false));
        assert!(!recreate_due(SurfaceKind::Opaque, &want, true));
        assert!(!recreate_due(SurfaceKind::Alpha, &want, false));
    }

    #[test]
    fn owned_windows_are_released_before_recreate() {
        let mut slot = Some(1);
        assert_eq!(drop_owned(&mut slot), Some(1));
        assert!(slot.is_none(), "the slot is clear so the window can reopen");
        assert_eq!(drop_owned(&mut slot), None);
    }

    #[test]
    fn transparency_checks_follow_iterm2() {
        assert_eq!(checks(true, false), [("toggleUseTransparency:", true), ("toggleDisableTransparencyForActiveWindow:", false)]);
        assert_eq!(checks(false, true), [("toggleUseTransparency:", false), ("toggleDisableTransparencyForActiveWindow:", true)]);
    }
    use crate::layout::Rect;
    use crate::scene::Quad;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn pane_alpha_follows_iterm2() {
        assert!((pane_alpha(0.3, true, false, false) - 0.7).abs() < 1e-6);
        assert_eq!(pane_alpha(0.3, true, true, true), 1.0);
        assert!((pane_alpha(0.3, true, true, false) - 0.7).abs() < 1e-6);
        assert_eq!(pane_alpha(0.3, false, false, false), 1.0);
        assert_eq!(pane_alpha(0.0, true, false, false), 1.0);
        assert_eq!(pane_alpha(1.5, true, false, false), 0.0);
        assert_eq!(pane_alpha(-0.2, true, false, false), 1.0);
    }

    #[test]
    fn window_toggle_follows_iterm2() {
        let mut w = WinTransparency::new(true);
        assert!(w.on);
        w.toggle();
        assert!(!w.on);
        assert!(!WinTransparency::restored(Some(false), true).on);
        assert!(WinTransparency::restored(None, true).on);
        assert!(WinTransparency::restored(Some(true), false).on);
    }

    #[test]
    fn fullscreen_restores_follow_iterm2() {
        let mut w = WinTransparency::new(true);
        w.before_fullscreen_toggle(false, true);
        assert!(!w.on);
        w.before_fullscreen_toggle(true, true);
        assert!(w.on);

        let mut w = WinTransparency::new(true);
        w.before_fullscreen_toggle(false, true);
        w.toggle();
        assert!(w.on);
        w.before_fullscreen_toggle(true, true);
        assert!(w.on);

        let mut w = WinTransparency::new(false);
        w.before_fullscreen_toggle(false, false);
        assert!(!w.on);
        w.before_fullscreen_toggle(true, false);
        assert!(!w.on);

        let mut w = WinTransparency::new(false);
        w.before_fullscreen_toggle(false, true);
        w.before_fullscreen_toggle(true, true);
        assert!(!w.on);

        let mut w = WinTransparency::restored_fullscreen(None, true);
        assert!(!w.on);
        w.before_fullscreen_toggle(true, true);
        assert!(w.on);
        assert!(WinTransparency::restored_fullscreen(None, false).on);
    }

    #[test]
    fn margins_surround_the_grid() {
        let pane = r(0.0, 30.0, 350.0, 570.0);
        let grid = r(4.0, 34.0, 340.0, 560.0);
        let m = crate::scene::margins(pane, grid);
        assert_eq!(m, vec![r(0.0, 30.0, 350.0, 4.0), r(0.0, 594.0, 350.0, 6.0), r(0.0, 34.0, 4.0, 560.0), r(344.0, 34.0, 6.0, 560.0)]);
        assert!(crate::scene::margins(pane, pane).is_empty());
    }

    #[test]
    fn backdrop_follows_iterm2() {
        let chrome = [0.1, 0.1, 0.1, 1.0];
        let a = [0.2, 0.2, 0.2, 0.7];
        let b = [0.3, 0.3, 0.3, 1.0];
        let a_margins = vec![r(0.0, 30.0, 350.0, 4.0), r(0.0, 594.0, 350.0, 6.0)];
        let b_margins = vec![r(351.0, 30.0, 349.0, 4.0)];
        let q = backdrop(r(0.0, 0.0, 800.0, 600.0), r(0.0, 30.0, 700.0, 570.0), chrome, &[(a_margins.clone(), a), (b_margins.clone(), b)]);
        let want = vec![
            Quad { rect: r(0.0, 0.0, 800.0, 30.0), color: chrome },
            Quad { rect: r(700.0, 30.0, 100.0, 570.0), color: chrome },
            Quad { rect: a_margins[0], color: a },
            Quad { rect: a_margins[1], color: a },
            Quad { rect: b_margins[0], color: b },
        ];
        assert_eq!(q, want);
        let none = backdrop(r(0.0, 0.0, 800.0, 600.0), r(0.0, 0.0, 800.0, 600.0), chrome, &[]);
        assert!(none.is_empty());
    }

    #[test]
    fn premultiplied_color_is_exact() {
        let c = premultiplied([255, 255, 255], 0.5);
        assert_eq!(c[3], 0.5);
        assert!((c[0] * 0.5 - linear(0.5)).abs() < 1e-4);
        assert_eq!(premultiplied([0, 0, 0], 0.3), [0.0, 0.0, 0.0, 0.3]);
        let o = premultiplied([51, 102, 204], 1.0);
        assert!((o[0] - linear(51.0 / 255.0)).abs() < 1e-6 && (o[1] - linear(102.0 / 255.0)).abs() < 1e-6 && (o[2] - linear(204.0 / 255.0)).abs() < 1e-6);
        assert_eq!(o[3], 1.0);
        assert_eq!(premultiplied([200, 100, 50], 0.0), [0.0; 4]);
    }

    #[test]
    fn alpha_mode_follows_need() {
        use wgpu::CompositeAlphaMode as M;
        let all = [M::Auto, M::Inherit, M::Opaque, M::PostMultiplied, M::PreMultiplied];
        assert_eq!(pick_alpha_mode(&all, true), M::PreMultiplied);
        assert_eq!(pick_alpha_mode(&all, false), M::Opaque);
        assert_eq!(pick_alpha_mode(&[M::Opaque], true), M::Opaque);
        assert_eq!(pick_alpha_mode(&[M::Opaque], false), M::Opaque);
        assert_eq!(pick_alpha_mode(&[M::Auto], true), M::Auto);
        assert_eq!(pick_alpha_mode(&[M::Auto], false), M::Auto);
        assert!(needs_alpha(&[1.0, 0.7]));
        assert!(!needs_alpha(&[1.0]));
        assert!(!needs_alpha(&[]));
    }

    #[test]
    fn alpha_window_decision_follows_profiles() {
        assert!(!wants_alpha_surface(&[(0.0, false)]));
        assert!(wants_alpha_surface(&[(0.0, false), (0.3, false)]));
        // Blur alone does not count (pre-flight P5): the tab blurs only above Transparency 0.
        assert!(!wants_alpha_surface(&[(0.0, true)]));
        assert!(!wants_alpha_surface(&[]));
        assert!(needs_recreate(SurfaceKind::Opaque, &[(0.3, false)]));
        assert!(!needs_recreate(SurfaceKind::Opaque, &[(0.0, false)]));
        assert!(!needs_recreate(SurfaceKind::Alpha, &[(0.0, false)]));
        assert!(!needs_recreate(SurfaceKind::Alpha, &[(0.3, true)]));
    }

    #[test]
    fn composite_blends_like_the_pipeline() {
        let q = |x, y, w, h, a| Quad { rect: r(x, y, w, h), color: [1.0, 0.0, 0.0, a] };
        let one = composite(&[q(0.0, 0.0, 10.0, 10.0, 0.7)], 5.0, 5.0);
        assert!((one[3] - 0.7).abs() < 1e-6);
        let two = composite(&[q(0.0, 0.0, 10.0, 10.0, 0.7), q(0.0, 0.0, 10.0, 10.0, 0.7)], 5.0, 5.0);
        assert!((two[3] - 0.91).abs() < 1e-6);
        assert_eq!(composite(&[q(0.0, 0.0, 10.0, 10.0, 0.7)], 50.0, 5.0), [0.0; 4]);
    }

    #[test]
    fn tab_blur_follows_iterm2() {
        let b = |transparency, blur, radius| BlurInput { transparency, blur, radius };
        assert!(tab_blur(&[b(0.3, true, None)], true));
        assert!(!tab_blur(&[b(0.3, true, None)], false));
        assert!(!tab_blur(&[b(0.0, true, None)], true));
        assert!(tab_blur(&[b(0.3, false, None), b(0.2, true, Some(10.0))], true));
    }

    #[test]
    fn blur_radius_follows_iterm2() {
        let b = |transparency, blur, radius| BlurInput { transparency, blur, radius };
        assert_eq!(tab_blur_radius(&[b(0.3, true, Some(10.0)), b(0.3, true, None)]), 6.0);
        assert_eq!(tab_blur_radius(&[b(0.3, false, Some(30.0))]), 2.0);
        assert_eq!(tab_blur_radius(&[b(0.3, true, Some(100.0))]), 64.0);
    }

    #[test]
    fn transparency_rows_follow_iterm2() {
        let keep = "Only The Default BG Color Uses Transparency";
        assert!(!row_enabled(keep, 0.0, false));
        assert!(row_enabled(keep, 0.1, false));
        assert!(!row_enabled("Blur", 0.0, true));
        assert!(row_enabled("Blur", 0.1, false));
        assert!(!row_enabled("Blur Radius", 0.1, false));
        assert!(row_enabled("Blur Radius", 0.1, true));
        assert!(!row_enabled("Blur Radius", 0.0, true));
        assert!(row_enabled("Transparency", 0.0, false));
        assert!(!radius_warning(true, 26.0));
        assert!(radius_warning(true, 26.5));
        assert!(!radius_warning(false, 40.0));
        assert_eq!(OVERRIDE_NOTICE, "View > Use Transparency can override this setting temporarily per window.");
        assert_eq!(LARGE_RADIUS, "Large radius may reduce performance");
        assert_eq!(WARNING_GLYPH, '\u{E7BA}');
    }

    #[test]
    fn dedicated_hotkey_window_is_transparent() {
        use crate::profile::Profile;
        let p = crate::hotkey::dedicated_profile(&Profile::default(), &[], &toml::Table::new(), "g");
        assert!((pane_alpha(p.float("Transparency"), p.bool("Initial Use Transparency"), false, false) - 0.7).abs() < 1e-6);
        assert!(tab_blur(&[BlurInput { transparency: 0.3, blur: p.bool("Blur"), radius: Some(p.float("Blur Radius")) }], true));
    }

    #[test]
    fn restored_fullscreen_applies_the_saved_key() {
        // PT:3656-3659 builds the state from the setting, then PT:4189-4191 applies the saved key.
        let w = WinTransparency::restored_fullscreen(Some(true), true);
        assert!(w.on);
        // The bookkeeping from the first step stays: leaving full screen still restores.
        let mut w = WinTransparency::restored_fullscreen(Some(false), false);
        assert!(!w.on);
        w.before_fullscreen_toggle(true, true);
        assert!(w.on, "old and restore come from the setting step");
        assert!(!WinTransparency::restored_fullscreen(Some(false), false).on);
    }

    #[test]
    fn launch_window_takes_initial_use_transparency() {
        use crate::profile::Profile;
        let mut off = Profile::default();
        off.set("Initial Use Transparency", toml::Value::Boolean(false));
        let on = Profile::default();
        assert!(!initial_use(Some(&off), &on));
        assert!(initial_use(None, &on));
        assert!(!initial_use(None, &off));
    }
}
