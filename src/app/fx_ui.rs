//! Render plugins in a window: the chain for the focused pane's profile, its uniforms each frame,
//! and the redraws its animation asks for. The chain lives on the window's `Gpu`, so each window
//! has its own and a GPU reset drops it with the rest.

use super::*;
use crate::render_fx::{self, Animation, Clock, Uniforms};
use crate::render_fx_gpu::{self, Chain};

/// The banner key for plugin errors.
const BANNER: &str = "render-plugins";

pub(super) struct FxRuntime {
    /// The plugin ids this chain was built for (the profile's list when it was made), with the
    /// shared switches it was compiled with.
    ids: (Vec<String>, Vec<bool>),
    /// Termurion's own Enter and done signals are on ("Effects Signals").
    pub(super) signals: bool,
    /// Built for a transparent window (its last pass keeps the shaders' alpha).
    keep_alpha: bool,
    chain: Option<Chain>,
    clock: Clock,
    uniforms: Uniforms,
    animation: Animation,
    /// The first plugin's preset cursor color (cursor-fx effects are made for one).
    preset: Option<[f32; 3]>,
}

/// The strongest animation among the plugins: always over auto over off.
fn strongest(list: &[Animation]) -> Animation {
    if list.contains(&Animation::Always) {
        Animation::Always
    } else if list.contains(&Animation::Auto) {
        Animation::Auto
    } else {
        Animation::Off
    }
}

fn unit(c: crate::palette::Rgb) -> [f32; 3] {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0]
}

/// The xterm 256-color table after the 16 ANSI colors: the 6x6x6 cube, then 24 grays.
fn xterm_256(i: usize) -> [f32; 3] {
    if i < 232 {
        let n = i - 16;
        let level = |v: usize| if v == 0 { 0.0 } else { (55 + v * 40) as f32 / 255.0 };
        [level(n / 36), level((n / 6) % 6), level(n % 6)]
    } else {
        let g = (8 + (i - 232) * 10) as f32 / 255.0;
        [g, g, g]
    }
}

impl App {
    /// The plugin ids the focused pane's profile lists, in order, and its shared switches (in
    /// `render_fx::SWITCHES` order).
    fn fx_wanted(&self) -> (Vec<String>, Vec<bool>) {
        let Some(id) = self.tabs.get(self.active).map(|t| t.focused) else { return (Vec::new(), Vec::new()) };
        let profile = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile());
        (crate::settings_lists::strings(profile, render_fx::PROFILE_KEY), render_fx::SWITCHES.iter().map(|(k, ..)| profile.bool(k)).collect())
    }

    /// Makes the window's chain match the focused profile's list. A plugin that is missing or
    /// fails to compile is left out and named in a banner, never silently dropped.
    pub(super) fn fx_sync(&mut self) {
        let wanted = self.fx_wanted();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let keep_alpha = gpu.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
        if gpu.fx.as_ref().map(|f| (&f.ids, f.keep_alpha)) == Some((&wanted, keep_alpha)) || (gpu.fx.is_none() && wanted.0.is_empty()) {
            return;
        }
        if wanted.0.is_empty() {
            gpu.fx = None;
            self.banners.clear(BANNER);
            return;
        }
        let (plugins, _) = render_fx::discover(Some(&render_fx::user_dir()));
        let mut shaders = Vec::new();
        let mut errors = Vec::new();
        let mut animations = Vec::new();
        let mut preset = None;
        let switches: Vec<(&str, bool)> = render_fx::SWITCHES.iter().zip(&wanted.1).filter_map(|((_, c, ..), on)| c.map(|c| (c, *on))).collect();
        let signals = render_fx::SWITCHES.iter().zip(&wanted.1).find(|((_, c, ..), _)| c.is_none()).is_none_or(|(_, on)| *on);
        for id in &wanted.0 {
            let Some(p) = plugins.iter().find(|p| &p.id == id) else {
                errors.push(format!("{id}: no such effect"));
                continue;
            };
            match p.compile_with(&switches) {
                Ok(list) => {
                    shaders.extend(list);
                    animations.push(p.manifest.animation);
                    preset = preset.or_else(|| p.manifest.cursor_color.as_deref().and_then(render_fx::hex_color));
                }
                Err(e) => errors.push(e),
            }
        }
        let format = gpu.config.format;
        // The passes write the window through its non-sRGB view, so the surface must offer one.
        let raw = render_fx_gpu::raw_format(format);
        if raw != format && !gpu.config.view_formats.contains(&raw) {
            gpu.config.view_formats.push(raw);
            gpu.surface.configure(gpu.render_device(), &gpu.config);
        }
        let chain = if shaders.is_empty() {
            None
        } else {
            match Chain::new(gpu.render_device(), format, shaders, keep_alpha) {
                Ok(c) => Some(c),
                Err(e) => {
                    errors.push(format!("the graphics card refused an effect: {e}"));
                    None
                }
            }
        };
        gpu.fx = Some(FxRuntime { ids: wanted, chain, clock: Clock::default(), uniforms: Uniforms::default(), animation: strongest(&animations), preset, keep_alpha, signals });
        if errors.is_empty() {
            self.banners.clear(BANNER);
        } else {
            self.banners.set(BANNER, format!("Effects: {}", errors.join("; ")));
        }
        gpu.window.request_redraw();
    }
}

impl FxRuntime {
    pub(super) fn active(&self) -> bool {
        self.chain.is_some()
    }

    /// The cursor color the scene draws while the effects run (`DrawPrefs::fx_cursor`).
    pub(super) fn cursor_color(&self) -> Option<crate::palette::Rgb> {
        let c = self.preset.filter(|_| self.active())?;
        Some(crate::palette::rgb((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8))
    }

    /// Fills the uniforms for a frame of `size` from what the scene recorded.
    pub(super) fn prepare(&mut self, pane: Option<&crate::scene::FxPane>, signals: Option<&render_fx::Signals>, size: (u32, u32), focused: bool, now: Instant) {
        let u = &mut self.uniforms;
        let mut cursor = pane.and_then(|p| p.cursor);
        if let (Some(c), Some(p)) = (cursor.as_mut(), pane) {
            // The cursor color the effects see: a program's OSC 12 color wins (a hook script's
            // signal arrives that way), else the effect's preset (or the profile's) with
            // Termurion's own Enter and done signal bits.
            let rgb = match p.program_cursor {
                Some(c) => unit(c),
                None => {
                    let base = self.preset.map_or(p.cursor_color, |c| crate::palette::rgb((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8));
                    let [r, g, b] = signals.map_or([base.r, base.g, base.b], |s| s.apply([base.r, base.g, base.b], now));
                    unit(crate::palette::rgb(r, g, b))
                }
            };
            c.color = [rgb[0], rgb[1], rgb[2], 1.0];
        }
        self.clock.frame(u, now, size, cursor, focused);
        if let Some(p) = pane {
            u.background = unit(p.bg);
            u.foreground = unit(p.fg);
            u.cursor_color = unit(p.program_cursor.unwrap_or(p.cursor_color));
            u.cursor_text = unit(p.cursor_text);
            u.selection_background = unit(p.selection);
            u.selection_foreground = unit(p.selected_text.unwrap_or(p.fg));
            u.input_box = p.input_box.map_or([0.0; 4], |[t, b]| [t, b, 1.0, 0.0]);
            for (i, slot) in u.palette.iter_mut().enumerate() {
                *slot = if i < 16 { unit(p.ansi[i]) } else { xterm_256(i) };
            }
        }
    }

    pub(super) fn chain_mut(&mut self) -> Option<(&mut Chain, &Uniforms)> {
        self.chain.as_mut().map(|c| (c, &self.uniforms))
    }

    /// Whether the animation wants another frame now.
    pub(super) fn wants_frame(&self, now: Instant, focused: bool) -> bool {
        self.active() && self.clock.wants_frame(self.animation, now, focused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strongest_animation_wins() {
        assert_eq!(strongest(&[Animation::Off, Animation::Auto]), Animation::Auto);
        assert_eq!(strongest(&[Animation::Auto, Animation::Always]), Animation::Always);
        assert_eq!(strongest(&[]), Animation::Off);
    }

    #[test]
    fn the_256_table_is_xterms() {
        assert_eq!(xterm_256(16), [0.0, 0.0, 0.0]);
        assert_eq!(xterm_256(231), [1.0, 1.0, 1.0]);
        assert_eq!(xterm_256(196), [1.0, 0.0, 0.0]);
        assert_eq!(xterm_256(232), [8.0 / 255.0; 3]);
        assert_eq!(xterm_256(255), [238.0 / 255.0; 3]);
    }
}
