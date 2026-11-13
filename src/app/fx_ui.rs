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
        // A card with no second view (OpenGL) gets copy passes instead (`without_view_formats`).
        let views = render_fx_gpu::view_formats_supported(gpu.render_adapter());
        let raw = render_fx_gpu::raw_format(format);
        if views && raw != format && !gpu.config.view_formats.contains(&raw) {
            gpu.config.view_formats.push(raw);
            gpu.surface.configure(gpu.render_device(), &gpu.config);
        }
        let chain = if shaders.is_empty() {
            None
        } else {
            match Chain::new(gpu.render_device(), format, shaders, keep_alpha) {
                Ok(c) if views => Some(c),
                Ok(c) => Some(c.without_view_formats(gpu.render_device())),
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
            u.input_box = p.input_box.map_or([0.0; 4], |b| b.uniform());
            u.input_box_x = p.input_box.map_or([0.0; 4], |b| b.uniform_x());
            u.prompt_mark = p.uniform_mark();
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

/// Detection runs for a pane at most this often.
const DETECT_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// The foreground program of a local pane's shell. None when tmux or ssh sits in between,
/// where its name says nothing about what the screen shows.
fn fx_job(p: &crate::pane::Pane, gateway: bool) -> Option<String> {
    if gateway {
        return None;
    }
    let name = p.shell_pid().and_then(crate::platform::proc::foreground_job)?.name;
    let base = name.trim().trim_end_matches(".exe").to_ascii_lowercase();
    (!matches!(base.as_str(), "tmux" | "ssh" | "mosh" | "mosh-client" | "autossh")).then_some(name)
}

/// Picks the profile of every pane that got output or a key (or all of them when `all`), at
/// most four times a second each. The time a throttled pane is due again; None when nothing waits.
fn detect(store: &crate::fx_store::Store, panes: &mut HashMap<PaneId, crate::pane::Pane>, all: bool, now: Instant, job: &dyn Fn(PaneId, &crate::pane::Pane) -> Option<String>) -> Option<Instant> {
    let mut due: Option<Instant> = None;
    let ids: Vec<PaneId> = panes.keys().copied().collect();
    for id in ids {
        let Some(p) = panes.get(&id) else { continue };
        if !(all || p.fx_dirty) {
            continue;
        }
        if let Some(next) = p.fx_checked.map(|t| t + DETECT_EVERY).filter(|&n| n > now && !all) {
            due = Some(due.map_or(next, |d| d.min(next)));
            continue;
        }
        let picked = p.fx_pick.as_ref().and_then(|n| store.by_name(n));
        // A job is read only when Auto will use it: the process table is not free.
        let job = if picked.is_none() { job(id, p) } else { None };
        let Some(p) = panes.get_mut(&id) else { continue };
        p.fx_dirty = false;
        p.fx_checked = Some(now);
        p.fx_rules = match picked {
            Some(rules) => rules,
            None => {
                p.fx_memory.forget_cleared(&p.screen);
                p.fx_memory.choose(store, &p.screen, job.as_deref()).1
            }
        };
    }
    due
}

/// Moves every pick that names `old` to `new`; true when any moved.
fn rename_picks(panes: &mut HashMap<PaneId, crate::pane::Pane>, old: &str, new: &str) -> bool {
    let mut moved = false;
    for p in panes.values_mut().filter(|p| p.fx_pick.as_deref() == Some(old)) {
        p.fx_pick = Some(new.to_string());
        p.fx_dirty = true;
        p.fx_checked = None;
        moved = true;
    }
    moved
}

impl App {
    /// Looks at the FX profile folder (the store holds its own one second gate) and, when it
    /// changed, takes the new union and has every pane pick again. True when it changed.
    /// The FX Profiles settings page calls this each frame it is open.
    pub(super) fn fx_profiles_poll(&mut self) -> bool {
        if !self.fx_profiles.refresh(Instant::now()) {
            return false;
        }
        self.fx_find = self.fx_profiles.find_rules();
        true
    }

    /// A profile was renamed in the GUI: the panes that picked it follow, and pick again now.
    pub(super) fn rename_fx_profile(&mut self, old: &str, new: &str) {
        if rename_picks(&mut self.panes, old, new) {
            self.fx_scan = true;
        }
    }

    /// The menu's pick for a pane ("" is Auto): the pane runs detection again at once.
    pub(super) fn set_fx_pick(&mut self, id: PaneId, name: &str) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        p.fx_pick = (!name.is_empty()).then(|| name.to_string());
        p.fx_dirty = true;
        p.fx_checked = None;
        self.fx_scan = true;
    }

    /// Picks each pane's profile after output, a key or a focus move; the folder is looked at
    /// only on that activity. The time of the next look, None when nothing waits.
    pub(super) fn tick_fx_profiles(&mut self) -> Option<Instant> {
        let focused = self.tabs.get(self.active).map(|t| t.focused);
        if focused != self.fx_focus {
            self.fx_focus = focused;
            if let Some(p) = focused.and_then(|id| self.panes.get_mut(&id)) {
                p.fx_dirty = true;
            }
        }
        let active = std::mem::take(&mut self.fx_scan) || self.panes.values().any(|p| p.fx_dirty);
        if !active {
            return None;
        }
        let reloaded = self.fx_profiles_poll();
        let tmux = &self.tmux;
        detect(&self.fx_profiles, &mut self.panes, reloaded, Instant::now(), &|id, p| fx_job(p, tmux.contains_key(&id)))
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

    #[test]
    fn the_fx_tick_asks_for_no_wake_when_nothing_happened() {
        let dir = std::env::temp_dir().join(format!("fx_ui_tick_{}", std::process::id()));
        let store = crate::fx_store::Store::open(&dir);
        let mut panes = HashMap::new();
        let mut p = crate::pane::Pane::recording(1, 10, 3);
        p.fx_dirty = false;
        panes.insert(1, p);
        let now = Instant::now();
        let none = |_: PaneId, _: &crate::pane::Pane| None;
        assert_eq!(detect(&store, &mut panes, false, now, &none), None, "idle");
        panes.get_mut(&1).unwrap().fx_dirty = true;
        assert_eq!(detect(&store, &mut panes, false, now, &none), None, "ran, nothing left");
        panes.get_mut(&1).unwrap().fx_dirty = true;
        assert_eq!(detect(&store, &mut panes, false, now, &none), Some(now + DETECT_EVERY), "throttled, due again");
    }

    /// A full-width Codex screen: its band and prompt, the cursor in it.
    fn codex_screen() -> crate::screen::Screen {
        let line = "\u{2500}".repeat(60);
        let mut s = crate::screen::Screen::new(80, 24);
        s.feed(format!("\x1b[?25l\x1b[2J\x1b[2;1HOpenAI Codex\x1b[19;1H{line}\x1b[20;1H\x1b[48;2;44;44;44m{}\x1b[0m\x1b[?25h\x1b[20;4H", " ".repeat(60)).as_bytes());
        s
    }

    fn picked(pick: Option<&str>, tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("fx_ui_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::fx_store::Store::open(&dir);
        let mut p = crate::pane::Pane::recording(1, 80, 24);
        p.screen = codex_screen();
        p.fx_pick = pick.map(str::to_string);
        p.fx_dirty = true;
        let mut panes = HashMap::new();
        panes.insert(1, p);
        detect(&store, &mut panes, false, Instant::now(), &|_, _| None);
        panes[&1].fx_rules.name.clone()
    }

    #[test]
    fn picking_a_profile_overrides_detection() {
        assert_eq!(picked(None, "auto"), "Codex", "Auto detects");
        assert_eq!(picked(Some("Shell"), "pick"), "Shell");
    }

    #[test]
    fn a_pick_whose_profile_vanished_falls_back_to_auto() {
        assert_eq!(picked(Some("No Such Profile"), "gone"), "Codex");
    }

    #[test]
    fn renaming_a_profile_moves_the_picks() {
        let mut panes = HashMap::new();
        for (id, pick) in [(1, Some("Old")), (2, Some("Other")), (3, None)] {
            let mut p = crate::pane::Pane::recording(id, 10, 3);
            p.fx_pick = pick.map(str::to_string);
            panes.insert(id, p);
        }
        assert!(rename_picks(&mut panes, "Old", "New"));
        assert_eq!(panes[&1].fx_pick.as_deref(), Some("New"));
        assert_eq!(panes[&2].fx_pick.as_deref(), Some("Other"));
        assert_eq!(panes[&3].fx_pick, None);
        assert!(panes[&1].fx_dirty && panes[&1].fx_checked.is_none(), "detection runs at once");
        assert!(!rename_picks(&mut panes, "Old", "Newer"));
    }
}
