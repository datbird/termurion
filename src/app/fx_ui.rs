//! Render plugins in a window: the chain for the focused pane's profile, its uniforms each frame,
//! and the redraws its animation asks for. The chain lives in the window's `WindowGpu`, so each
//! window has its own and a GPU reset drops it with the rest.

use super::*;
use crate::render_fx::{self, Animation, Clock, Uniforms};

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
    /// The window's chain was built (`WindowGpu::set_effect`).
    active: bool,
    /// Where the chain's effects can draw, as the chain was given them.
    terms: render_fx::RegionTerms,
    clock: Clock,
    uniforms: Uniforms,
    /// Last frame's uniforms: an animation frame also shades where last frame's effect drew.
    last_uniforms: Uniforms,
    /// Built with Live Lines on (the shaders animate any box lines they find themselves).
    live_lines: bool,
    animation: Animation,
    /// The first plugin's preset cursor color (cursor-fx effects are made for one).
    preset: Option<[f32; 3]>,
    /// When the animation's next frame is due ("Effects animation" caps the rate); the idle
    /// loop asks for it then (`App::fx_frame_tick`).
    pub(super) due: Option<Instant>,
    /// Display link ticks in a row that found no frame due (`fx_frame_tick`).
    empty_ticks: u8,
    /// The pane colors the palette and color uniforms were last filled from.
    colors: Option<PaneColors>,
    palette_fills: u64,
    /// `region`'s last answer, kept while its inputs are the same.
    rects: render_fx::RectsCache,
}

/// Every pane color the uniforms carry; a frame with the same ones leaves them as they are.
#[derive(Clone, Debug, PartialEq)]
struct PaneColors {
    bg: crate::palette::Rgb,
    fg: crate::palette::Rgb,
    cursor_color: crate::palette::Rgb,
    program_cursor: Option<crate::palette::Rgb>,
    cursor_text: crate::palette::Rgb,
    selection: crate::palette::Rgb,
    selected_text: Option<crate::palette::Rgb>,
    ansi: [crate::palette::Rgb; 16],
}

impl PaneColors {
    fn of(p: &crate::scene::FxPane) -> PaneColors {
        PaneColors { bg: p.bg, fg: p.fg, cursor_color: p.cursor_color, program_cursor: p.program_cursor, cursor_text: p.cursor_text, selection: p.selection, selected_text: p.selected_text, ansi: p.ansi }
    }
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
    /// Asks every window whose effect frame is due for it, and returns when the next one is
    /// due. A capped animation waits here instead of asking from inside its own frame.
    pub(super) fn fx_frame_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut wake = |gpu: &mut Gpu| {
            let period = gpu.win.display_period();
            let tick = gpu.win.take_display_tick();
            // A frame that found no free surface (with or without an effect) draws on this tick.
            if tick.owed {
                gpu.redraw();
            }
            let Some(fx) = gpu.fx.as_mut() else { return gpu.win.follow_display(None) };
            match fx.due {
                Some(t) if fx_due_now(t, now, period, tick.ticked) => {
                    fx.due = None;
                    fx.empty_ticks = 0;
                    gpu.redraw_fx();
                }
                Some(t) => {
                    fx.empty_ticks = 0;
                    let t = fx_backstop(t, period);
                    next = Some(next.map_or(t, |n| n.min(t)));
                }
                // Ticks with no frame due: the frame asked for has not drawn. One such tick is a
                // frame still on its way (under load); after two in a row it never drew (a hidden
                // window, a recovery, a lost surface), so the link stops until a frame paces the
                // effect again.
                None if tick.ticked => {
                    fx.empty_ticks = fx.empty_ticks.saturating_add(1);
                    if fx.empty_ticks >= 2 {
                        gpu.win.follow_display(None);
                    }
                }
                None => {}
            }
        };
        if let Some(g) = self.gpu.as_mut() {
            wake(g);
        }
        for (_, w) in self.windows.parked_mut() {
            if let Some(g) = w.gpu.as_mut() {
                wake(g);
            }
        }
        next
    }

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
        let keep_alpha = gpu.win.keeps_alpha();
        if gpu.fx.as_ref().map(|f| (&f.ids, f.keep_alpha)) == Some((&wanted, keep_alpha)) || (gpu.fx.is_none() && wanted.0.is_empty()) {
            return;
        }
        // The window shows the old effects' output, or none: the next frame draws whole.
        gpu.scene.invalidate();
        if wanted.0.is_empty() {
            gpu.fx = None;
            // No chain to build: this cannot fail.
            let _ = gpu.win.set_effect(None, None);
            self.banners.clear(BANNER);
            return;
        }
        let (plugins, _) = render_fx::discover(Some(&render_fx::user_dir()));
        let switches: Vec<(&str, bool)> = render_fx::SWITCHES.iter().zip(&wanted.1).filter_map(|((_, c, ..), on)| c.map(|c| (c, *on))).collect();
        let live_lines = render_fx::SWITCHES.iter().zip(&wanted.1).any(|((_, c, ..), on)| *c == Some("LINE_EFFECT") && *on);
        let signals = render_fx::SWITCHES.iter().zip(&wanted.1).find(|((_, c, ..), _)| c.is_none()).is_none_or(|(_, on)| *on);
        let crate::gpu_window::BuiltEffects { errors, active, terms, animations, preset } = crate::gpu_window::build_effects(&mut gpu.win, &plugins, &wanted.0, &switches);
        gpu.fx = Some(FxRuntime { ids: wanted, active, terms: terms.filter(|_| active), clock: Clock::default(), uniforms: Uniforms::default(), last_uniforms: Uniforms::default(), live_lines, animation: strongest(&animations), preset, keep_alpha, signals, due: None, empty_ticks: 0, colors: None, palette_fills: 0, rects: Default::default() });
        if errors.is_empty() {
            self.banners.clear(BANNER);
        } else {
            self.banners.set(BANNER, format!("Effects: {}", errors.join("; ")));
        }
        gpu.redraw();
        // Detection skips panes while no effect runs; it starts again now.
        for p in self.panes.values_mut() {
            p.fx_dirty = true;
            p.fx_checked = None;
        }
        self.fx_scan = true;
        // The frame about to be built must already read the right rules.
        self.tick_fx_profiles();
    }
}

/// Whether an effect frame due at `t` draws now. On the display link (ticks `period` apart),
/// it draws on the tick nearest `t` (`ticked`: this wake is a tick), or once the backstop is
/// reached (a covered window, a paused link); on the loop timer, at `t`.
pub(super) fn fx_due_now(t: Instant, now: Instant, period: Option<std::time::Duration>, ticked: bool) -> bool {
    match period {
        Some(p) => (ticked && t <= now + p / 2) || fx_backstop(t, period) <= now,
        None => t <= now,
    }
}

/// The loop timer for a frame due at `t`: on the display link only a backstop, a tick and a
/// half after it, so a tick (at most half a tick from `t`, or a tick after a frame due at once
/// under Full) always comes first.
pub(super) fn fx_backstop(t: Instant, period: Option<std::time::Duration>) -> Instant {
    period.map_or(t, |p| t + p + p / 2)
}

impl FxRuntime {
    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// The uniforms `prepare` packed for this frame.
    pub(super) fn uniforms(&self) -> &Uniforms {
        &self.uniforms
    }

    /// The cursor color the scene draws while the effects run (`DrawPrefs::fx_cursor`).
    pub(super) fn cursor_color(&self) -> Option<crate::palette::Rgb> {
        let c = self.preset.filter(|_| self.active())?;
        Some(crate::palette::rgb((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8))
    }

    /// Fills the uniforms for a frame of `size` from what the scene recorded.
    pub(super) fn prepare(&mut self, pane: Option<&crate::scene::FxPane>, signals: Option<&render_fx::Signals>, size: (u32, u32), focused: bool, now: Instant, pacing: render_fx::Pacing) {
        self.last_uniforms.clone_from(&self.uniforms);
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
        let before = self.clock.last_change();
        self.clock.set_pacing(pacing);
        self.clock.frame(u, now, size, cursor, focused);
        // What keeps an `always` animation from pausing shows in the debug log.
        if crate::dlog::is_on() && self.clock.last_change() != before {
            let c = cursor.unwrap_or(render_fx::Cursor { rect: [0.0; 4], color: [0.0; 4], style: 0, visible: false });
            let [x, y, w, h] = c.rect;
            let [r, g, b, _] = c.color;
            crate::dlog!("fx: idle clock restarted: cursor {} at {} {} {}x{}, color {} {} {}, focused {}", cursor.is_some(), x, y, w, h, r, g, b, focused);
        }
        if let Some(p) = pane {
            // The colors move rarely: refill them (and the 256-entry palette) only when they did.
            let seen = PaneColors::of(p);
            if self.colors.as_ref() != Some(&seen) {
                u.background = unit(p.bg);
                u.foreground = unit(p.fg);
                u.cursor_color = unit(p.program_cursor.unwrap_or(p.cursor_color));
                u.cursor_text = unit(p.cursor_text);
                u.selection_background = unit(p.selection);
                u.selection_foreground = unit(p.selected_text.unwrap_or(p.fg));
                for (i, slot) in u.palette.iter_mut().enumerate() {
                    *slot = if i < 16 { unit(p.ansi[i]) } else { xterm_256(i) };
                }
                self.colors = Some(seen);
                self.palette_fills += 1;
            }
            u.input_box = p.input_box.map_or([0.0; 4], |b| b.uniform());
            u.input_box_x = p.input_box.map_or([0.0; 4], |b| b.uniform_x());
            u.prompt_mark = p.uniform_mark();
        }
    }

    /// Where this frame's effects can draw, alone and with last frame's (`render_fx::frame_rects`;
    /// None runs the full pass). `cell` is the focused pane's cell.
    pub(super) fn region(&mut self, cell: (f32, f32), size: (u32, u32)) -> std::sync::Arc<render_fx::FrameRegion> {
        let Some(terms) = self.terms.as_deref().filter(|_| self.active) else { return self.rects.none() };
        self.rects.get(terms, &self.uniforms, &self.last_uniforms, cell, self.live_lines, size)
    }

    #[cfg(test)]
    fn bare() -> FxRuntime {
        FxRuntime { ids: Default::default(), signals: false, keep_alpha: false, active: false, terms: None, clock: Clock::default(), uniforms: Uniforms::default(), last_uniforms: Uniforms::default(), live_lines: false, animation: Animation::Off, preset: None, due: None, empty_ticks: 0, colors: None, palette_fills: 0, rects: Default::default() }
    }

    /// The display link's interval while it paces the frames (`Clock::set_link`).
    pub(super) fn set_link(&mut self, link: Option<std::time::Duration>) {
        self.clock.set_link(link);
    }

    /// When the animation's next frame is due under `pacing`; None when it needs none.
    pub(super) fn next_frame(&self, now: Instant, focused: bool, pacing: render_fx::Pacing) -> Option<Instant> {
        if !self.active() {
            return None;
        }
        self.clock.next_frame(self.animation, now, focused, pacing)
    }

    /// `next_frame` after a frame that was left out (`Clock::next_after_still`).
    pub(super) fn next_after_still(&self, now: Instant, focused: bool, pacing: render_fx::Pacing) -> Option<Instant> {
        if !self.active() {
            return None;
        }
        self.clock.next_after_still(self.animation, now, focused, pacing)
    }
}

/// Detection runs for a pane at most this often.
const DETECT_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// The foreground program of a local pane's shell, from the job watcher (no process table read
/// here). None when tmux or ssh sits in between, where its name says nothing about the screen.
fn fx_job(jobs: &crate::jobwatch::JobWatch, p: &crate::pane::Pane, gateway: bool) -> Option<String> {
    if gateway {
        return None;
    }
    let name = jobs.job(p.shell_pid()?).job?.name;
    let base = name.trim().trim_end_matches(".exe").to_ascii_lowercase();
    (!matches!(base.as_str(), "tmux" | "ssh" | "mosh" | "mosh-client" | "autossh")).then_some(name)
}

/// Picks the profile of every pane that got output or a key (or all of them when `all`), at
/// most four times a second each. Panes whose rules changed are added to `changed`. The time a throttled pane is due again; None when nothing waits.
fn detect(store: &crate::fx_store::Store, panes: &mut HashMap<PaneId, crate::pane::Pane>, all: bool, now: Instant, on: &dyn Fn(PaneId) -> bool, job: &dyn Fn(PaneId, &crate::pane::Pane) -> Option<String>, changed: &mut Vec<PaneId>) -> Option<Instant> {
    let mut due: Option<Instant> = None;
    let ids: Vec<PaneId> = panes.keys().copied().collect();
    for id in ids {
        let Some(p) = panes.get(&id) else { continue };
        if !(all || p.fx_dirty) {
            continue;
        }
        // No effect runs in this pane's window: nothing reads the rules. Turning effects on marks panes again.
        if !on(id) {
            if let Some(p) = panes.get_mut(&id) {
                p.fx_dirty = false;
            }
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
        let rules = match picked {
            Some(rules) => rules,
            None => {
                p.fx_memory.forget_cleared(&p.screen);
                p.fx_memory.choose(store, &p.screen, job.as_deref()).1
            }
        };
        if !Arc::ptr_eq(&rules, &p.fx_rules) {
            changed.push(id);
        }
        p.fx_rules = rules;
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

    /// The FX Profiles page may have changed the store (a save, delete, import, or the folder):
    /// when the profiles differ from what the panes picked from, the union is rebuilt and every
    /// pane picks again.
    fn fx_profiles_sync(&mut self) {
        self.fx_profiles_poll();
        let seen: Vec<usize> = self.fx_profiles.profiles.iter().map(|l| Arc::as_ptr(&l.rules) as usize).collect();
        if seen == self.fx_seen {
            return;
        }
        self.fx_seen = seen;
        self.fx_find = self.fx_profiles.find_rules();
        for p in self.panes.values_mut() {
            p.fx_dirty = true;
            p.fx_checked = None;
        }
        self.fx_scan = true;
    }

    /// What the FX Profiles page asked for: a rename the picks follow, the folder, import, export.
    pub(super) fn fx_page_actions(&mut self, actions: Vec<crate::settings_fx_profiles::PageAction>, open: bool) {
        use crate::settings_fx_profiles::PageAction;
        for a in actions {
            match a {
                PageAction::Rename { old, new } => self.rename_fx_profile(&old, &new),
                PageAction::ShowFolder => {
                    let dir = crate::fx_store::user_dir();
                    if let Ok(judged) = crate::localfs::judge_io(&dir) {
                        let _ = crate::localfs::create_dir_all(&judged);
                    }
                    crate::settings_effects::open_folder(&dir);
                }
                PageAction::Import => self.import_fx_profile(),
                PageAction::Export(name) => self.export_fx_profile(&name),
            }
        }
        if open {
            self.fx_profiles_sync();
        }
    }

    fn import_fx_profile(&mut self) {
        use crate::fx_store::ImportError;
        let dialog = rfd::FileDialog::new().set_title("Import FX Profile").add_filter("FX profile", &["toml"]);
        let Some(path) = crate::localfs::pick_file(dialog) else { return };
        let mut result = self.fx_profiles.import(path.path(), false);
        if let Err(ImportError::Exists(name)) = &result {
            if !super::close_ui::confirm(self.hwnd(), "Replace Profile?", &format!("A profile named {name} exists. Replace it with the imported file?")) {
                return;
            }
            result = self.fx_profiles.import(path.path(), true);
        }
        match result {
            Ok(name) => self.banners.set("window", format!("Imported the FX profile {name}.")),
            Err(ImportError::Exists(_)) => {}
            Err(ImportError::Invalid(e)) => self.banners.set("window", format!("Not an FX profile: {}{}", e.line.map_or(String::new(), |n| format!("line {n}: ")), e.message)),
            Err(ImportError::Io(e)) => self.banners.set("window", format!("Could not import: {e}")),
        }
    }

    fn export_fx_profile(&mut self, name: &str) {
        let dialog = rfd::FileDialog::new().set_title("Export FX Profile").set_file_name(format!("{name}.toml")).add_filter("FX profile", &["toml"]);
        let Some(path) = crate::localfs::save_file(dialog) else { return };
        match self.fx_profiles.export(name, path.path()) {
            Ok(()) => self.banners.set("window", format!("Exported the FX profile {name}.")),
            Err(e) => self.banners.set("window", format!("Could not export: {e}")),
        }
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

    /// The panes of the windows that run a render plugin effect right now.
    fn fx_live_panes(&self) -> std::collections::HashSet<PaneId> {
        let on = |g: &Option<super::Gpu>| g.as_ref().and_then(|g| g.fx.as_ref()).is_some_and(|f| f.active());
        let mut out = std::collections::HashSet::new();
        if on(&self.gpu) {
            self.tabs.iter().for_each(|t| out.extend(t.layout.panes()));
        }
        for (_, s) in self.windows.parked() {
            if on(&s.gpu) {
                s.tabs.iter().for_each(|t| out.extend(t.layout.panes()));
            }
        }
        out
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
        let jobs = &self.jobs;
        let live = self.fx_live_panes();
        let mut changed = Vec::new();
        let due = detect(&self.fx_profiles, &mut self.panes, reloaded, Instant::now(), &|id| live.contains(&id), &|id, p| fx_job(jobs, p, tmux.contains_key(&id)), &mut changed);
        // New rules change what the effects read: that pane's window draws again.
        for id in changed {
            self.redraw_pane_window(id);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use crate::test_tmp::TempDir;
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
        let _td = TempDir::new("fx_ui_tick_");
        let dir = _td.to_path_buf();
        let store = crate::fx_store::Store::open(&dir);
        let mut panes = HashMap::new();
        let mut p = crate::pane::Pane::recording(1, 10, 3);
        p.fx_dirty = false;
        panes.insert(1, p);
        let now = Instant::now();
        let none = |_: PaneId, _: &crate::pane::Pane| None;
        assert_eq!(detect(&store, &mut panes, false, now, &|_| true, &none, &mut Vec::new()), None, "idle");
        panes.get_mut(&1).unwrap().fx_dirty = true;
        assert_eq!(detect(&store, &mut panes, false, now, &|_| true, &none, &mut Vec::new()), None, "ran, nothing left");
        panes.get_mut(&1).unwrap().fx_dirty = true;
        assert_eq!(detect(&store, &mut panes, false, now, &|_| true, &none, &mut Vec::new()), Some(now + DETECT_EVERY), "throttled, due again");
    }

    #[test]
    fn a_redraw_is_asked_only_when_the_rules_change() {
        let _td = TempDir::new("fx_ui_chg_");
        let dir = _td.to_path_buf();
        let store = crate::fx_store::Store::open(&dir);
        let mut p = crate::pane::Pane::recording(1, 80, 24);
        p.screen = codex_screen();
        p.fx_dirty = true;
        let mut panes = HashMap::new();
        panes.insert(1, p);
        let none = |_: PaneId, _: &crate::pane::Pane| None;
        let mut changed = Vec::new();
        detect(&store, &mut panes, false, Instant::now(), &|_| true, &none, &mut changed);
        assert_eq!(changed, vec![1], "Shell to Codex");
        changed.clear();
        panes.get_mut(&1).unwrap().fx_dirty = true;
        panes.get_mut(&1).unwrap().fx_checked = None;
        detect(&store, &mut panes, false, Instant::now(), &|_| true, &none, &mut changed);
        assert!(changed.is_empty(), "same rules, no redraw");
    }

    #[test]
    fn detection_skips_a_pane_with_no_effect_on() {
        let _td = TempDir::new("fx_ui_off_");
        let dir = _td.to_path_buf();
        let store = crate::fx_store::Store::open(&dir);
        let mut p = crate::pane::Pane::recording(1, 80, 24);
        p.screen = codex_screen();
        p.fx_dirty = true;
        let mut panes = HashMap::new();
        panes.insert(1, p);
        let jobs_read = std::cell::Cell::new(0);
        let job = |_: PaneId, _: &crate::pane::Pane| {
            jobs_read.set(jobs_read.get() + 1);
            None
        };
        detect(&store, &mut panes, true, Instant::now(), &|_| false, &job, &mut Vec::new());
        assert_eq!(panes[&1].fx_rules.name, "Shell", "not picked while effects are off");
        assert!(!panes[&1].fx_dirty && panes[&1].fx_checked.is_none());
        assert_eq!(jobs_read.get(), 0);
        panes.get_mut(&1).unwrap().fx_dirty = true;
        detect(&store, &mut panes, false, Instant::now(), &|_| true, &job, &mut Vec::new());
        assert_eq!(panes[&1].fx_rules.name, "Codex", "picked once effects are on");
    }

    /// A full-width Codex screen: its band and prompt, the cursor in it.
    fn codex_screen() -> crate::screen::Screen {
        let line = "\u{2500}".repeat(60);
        let mut s = crate::screen::Screen::new(80, 24);
        s.feed(format!("\x1b[?25l\x1b[2J\x1b[2;1HOpenAI Codex\x1b[19;1H{line}\x1b[20;1H\x1b[48;2;44;44;44m{}\x1b[0m\x1b[?25h\x1b[20;4H", " ".repeat(60)).as_bytes());
        s
    }

    fn picked(pick: Option<&str>, tag: &str) -> String {
        let _td = TempDir::new(&format!("fx-ui-{tag}"));
        let dir = _td.to_path_buf();
        let store = crate::fx_store::Store::open(&dir);
        let mut p = crate::pane::Pane::recording(1, 80, 24);
        p.screen = codex_screen();
        p.fx_pick = pick.map(str::to_string);
        p.fx_dirty = true;
        let mut panes = HashMap::new();
        panes.insert(1, p);
        detect(&store, &mut panes, false, Instant::now(), &|_| true, &|_, _| None, &mut Vec::new());
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

    fn pane() -> crate::scene::FxPane {
        let c = |n: u8| crate::palette::rgb(n, n / 2, 255 - n);
        crate::scene::FxPane {
            cursor: Some(render_fx::Cursor { rect: [10.0, 20.0, 8.0, 16.0], color: [1.0; 4], style: 0, visible: true }),
            focused: true,
            bg: c(1),
            fg: c(200),
            cursor_color: c(90),
            program_cursor: None,
            cursor_text: c(3),
            selection: c(40),
            selected_text: None,
            ansi: std::array::from_fn(|i| c(i as u8 * 10)),
            input_box: None,
            prompt_mark: None,
        }
    }

    #[test]
    fn the_palette_is_filled_once_per_color_change() {
        let mut rt = FxRuntime::bare();
        let mut p = pane();
        let start = Instant::now();
        for i in 0..100 {
            rt.prepare(Some(&p), None, (800, 600), true, start + std::time::Duration::from_millis(16 * i), render_fx::Pacing::Full);
        }
        assert_eq!(rt.palette_fills, 1);
        let before = rt.uniforms.palette.clone();
        p.ansi[4] = crate::palette::rgb(9, 8, 7);
        rt.prepare(Some(&p), None, (800, 600), true, start + std::time::Duration::from_secs(3), render_fx::Pacing::Full);
        assert_eq!(rt.palette_fills, 2);
        assert_ne!(rt.uniforms.palette, before);
        assert_eq!(rt.uniforms.palette[4], unit(p.ansi[4]));
        assert_eq!(rt.uniforms.background, unit(p.bg));
        // A cursor or input box change is not a color change.
        p.input_box = None;
        p.prompt_mark = Some([1.0, 2.0, 3.0, 4.0]);
        rt.prepare(Some(&p), None, (800, 600), true, start + std::time::Duration::from_secs(4), render_fx::Pacing::Full);
        assert_eq!(rt.palette_fills, 2);
        assert_eq!(rt.uniforms.prompt_mark, p.uniform_mark(), "the per-frame fields still follow");
    }

    #[test]
    fn a_frame_with_no_region_allocates_nothing() {
        let mut rt = FxRuntime::bare();
        let _ = rt.region((10.0, 20.0), (800, 600));
        let (_, allocs) = crate::render_fx::counting::count(|| {
            for _ in 0..100 {
                let _ = rt.region((10.0, 20.0), (800, 600));
            }
        });
        assert_eq!(allocs, 0);
    }

    /// The main loop under a display link that fires every `link` (from `phase`, with a little
    /// jitter), with the loop's backstop timer, for `secs`, as `fx_frame_tick` and the frame's
    /// `pace_fx` run them under `pacing`: (frames drawn, loop wakes, frames the backstop drew).
    fn display_paced(pacing: render_fx::Pacing, link: std::time::Duration, phase: std::time::Duration, secs: u64) -> (u64, u64, u64) {
        let us = std::time::Duration::from_micros;
        let t0 = Instant::now();
        let end = t0 + std::time::Duration::from_secs(secs);
        let mut clock = render_fx::Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(pacing);
        clock.set_link(Some(link));
        clock.frame(&mut u, t0, (10, 10), None, true);
        let mut due = clock.next_frame(Animation::Always, t0, true, pacing).unwrap();
        let (mut frames, mut wakes, mut backstops) = (0, 0, 0);
        let mut k: u32 = 0;
        // Each tick up to 0.8 ms late.
        let tick_at = |k: u32| t0 + phase + link * k + us(u64::from(k.wrapping_mul(7919) % 800));
        loop {
            let tick = tick_at(k);
            let timer = fx_backstop(due, Some(link));
            let (now, ticked) = if tick <= timer { (tick, true) } else { (timer, false) };
            if now >= end {
                break;
            }
            wakes += 1;
            if ticked {
                k += 1;
            }
            if fx_due_now(due, now, Some(link), ticked) {
                clock.frame(&mut u, now, (10, 10), None, true);
                frames += 1;
                backstops += u64::from(!ticked);
                due = clock.next_frame(Animation::Always, now, true, pacing).unwrap();
            }
        }
        (frames, wakes, backstops)
    }

    #[test]
    fn display_paced_frames_land_on_the_links_ticks() {
        use render_fx::Pacing;
        let hz = |f: f64| std::time::Duration::from_secs_f64(1.0 / f);
        // (display, pacing, the link's rate: the effect's where the display divides into it).
        let cases = [
            (60.0, Pacing::Smooth, 60.0),
            (60.0, Pacing::Balanced, 30.0),
            (60.0, Pacing::Light, 15.0),
            (60.0, Pacing::Full, 60.0),
            (60.2, Pacing::Smooth, 60.2),
            (59.9, Pacing::Smooth, 59.9),
            (59.9, Pacing::Balanced, 59.9 / 2.0),
            (120.0, Pacing::Smooth, 60.0),
            (120.0, Pacing::Balanced, 30.0),
            (120.0, Pacing::Light, 15.0),
            (120.0, Pacing::Full, 120.0),
        ];
        for (display, pacing, rate) in cases {
            // The link's first tick comes within one interval.
            for phase in [0.0, 0.2, 0.5, 0.9].map(|f| hz(rate).mul_f64(f)) {
                let (frames, wakes, backstops) = display_paced(pacing, hz(rate), phase, 10);
                let fps = frames as f64 / 10.0;
                let name = format!("{display} Hz, {pacing:?}, phase {phase:?}");
                assert!((fps - rate).abs() <= rate * 0.01 + 0.2, "{name}: {fps} frames/s, want {rate}");
                assert_eq!(backstops, 0, "{name}: the backstop timer drew {backstops} frames");
                assert!(wakes <= frames + 1, "{name}: {wakes} wakes for {frames} frames");
            }
        }
        // A link that ticks at the display's rate whatever it is asked: the rate still holds.
        for (pacing, rate) in [(Pacing::Balanced, 30.0), (Pacing::Light, 15.0)] {
            let (frames, _, backstops) = display_paced(pacing, hz(60.0), std::time::Duration::from_millis(5), 10);
            assert!((frames as f64 / 10.0 - rate).abs() <= rate * 0.01 + 0.2, "{pacing:?} on a 60 Hz link: {frames}");
            assert_eq!(backstops, 0);
        }
    }

    #[test]
    fn a_display_paced_frame_waits_for_a_refresh_unless_one_is_overdue() {
        let p = std::time::Duration::from_micros(16_667);
        let t = Instant::now();
        assert!(!fx_due_now(t, t, Some(p), false), "not on a refresh");
        assert!(fx_due_now(t, t - p / 3, Some(p), true), "the refresh nearest it");
        assert!(!fx_due_now(t, t - p, Some(p), true), "a refresh too early");
        assert!(!fx_due_now(t, t + p, Some(p), false), "a tick late is not yet the backstop");
        assert!(fx_due_now(t, t + p + p / 2, Some(p), false), "a tick and a half overdue: the backstop");
        assert_eq!(fx_backstop(t, Some(p)), t + p + p / 2);
        assert_eq!(fx_backstop(t, None), t);
        assert!(fx_due_now(t, t, None, false), "the loop timer");
    }
}
