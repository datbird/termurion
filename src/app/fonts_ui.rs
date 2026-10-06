//! Each pane draws with its own profile's fonts (milestone 13n Task 1). The renderer keeps a
//! table of faces (`render::Renderer::face`); a pane's face comes from its profile
//! (`App::face_key`), and its cell size from its face (`App::pane_cell`). Window chrome keeps
//! the default face and `App::cell`. iTerm2 draws every session with its own font table
//! (`PTYSession.m` `setFontTable:horizontalSpacing:verticalSpacing:`), the cell being
//! `PTYTextView.m:227-233`.

use std::collections::HashMap;

use super::*;
use crate::render::{FaceKey, Renderer};

/// The face key of a profile: its Normal Font at `scale`, the
/// Non-ASCII Font when the profile uses one, and the spacings. `resolve` turns an iTerm2 font
/// name into an installed family (the renderer's memoized lookup, or the identity).
pub fn face_key_for(p: &Profile, scale: f32, resolve: &dyn Fn(&str) -> String) -> FaceKey {
    let normal = p.font("Normal Font");
    let alt = p.bool("Use Non-ASCII Font").then(|| resolve(&p.font("Non Ascii Font").family));
    let px = normal.size_pt * 4.0 / 3.0 * scale;
    let mut key = FaceKey::new(&resolve(&normal.family), alt.as_deref(), px, p.float("Horizontal Spacing") as f32, p.float("Vertical Spacing") as f32);
    // Use Bold Font and Use Italic Font (`ASB:773-778`): a face that ignores a switch is its own face.
    (key.use_bold, key.use_italic) = (p.bool("Use Bold Font"), p.bool("Use Italic Font"));
    // ASCII Ligatures and Non-ASCII Ligatures decide what the pane's text shapes with.
    (key.ascii_ligatures, key.non_ascii_ligatures) = (p.bool("ASCII Ligatures"), p.bool("Non-ASCII Ligatures"));
    // ASCII Anti Aliased, Non-ASCII Anti Aliased and Thin Strokes decide how the pane's glyph masks are drawn.
    (key.ascii_aa, key.non_ascii_aa, key.thin) = (p.bool("ASCII Anti Aliased"), p.bool("Non-ASCII Anti Aliased"), p.int("Thin Strokes").clamp(0, 4) as u8);
    // Draw Powerline Glyphs chooses whether the pane's Powerline symbols are Tandem's.
    key.powerline = p.bool("Draw Powerline Glyphs");
    // Special Font Config: the table itself, normalized, so two spellings are one face and a value
    // that does not parse or is out of bounds (a file, the API) is no table (`font_table::Config::parse`).
    key.table = crate::font_table::normalized(&p.string(SPECIAL_FONT_CONFIG)).map(std::sync::Arc::from);
    key
}

/// The profile key of the per-Unicode-range font table (`Special Font Config`).
pub const SPECIAL_FONT_CONFIG: &str = crate::font_table::KEY;

/// The stored key View > Size Changes Update Profile toggles (`kPreferenceKeySizeChangesAffectProfile`).
pub const SIZE_CHANGES_AFFECT_PROFILE: &str = "Size Changes Affect Profile";

/// Adds to `targets` every pane of `all` that shares a tmux window (gateway, window) with one of
/// them: tmux sizes the window's panes together, so they take one text size. `window_of` names a
/// pane's tmux window, None for a pane that is not tmux's.
fn widen_to_tmux_windows(targets: &mut Vec<PaneId>, all: &[PaneId], window_of: &dyn Fn(PaneId) -> Option<(PaneId, u32)>) {
    let windows: Vec<(PaneId, u32)> = targets.iter().filter_map(|p| window_of(*p)).collect();
    for &p in all {
        if !targets.contains(&p) && window_of(p).is_some_and(|w| windows.contains(&w)) {
            targets.push(p);
        }
    }
}

/// The cell under a point of a pane whose text area starts at `origin`, and which half of the
/// cell it is in: the right side once the pointer is within `fraction` of the cell's right edge
/// (`FractionOfCharacterSelectingNextNeighbor`).
pub fn pointer_cell(cell: (f32, f32), origin: (f32, f32), x: f32, y: f32, cols: u32, rows: u32, fraction: f32) -> (u32, u32, Side) {
    let (cw, ch) = cell;
    let (col, row) = mouse::cell_at((x - origin.0) as f64, (y - origin.1) as f64, cw as f64, ch as f64, cols, rows);
    let side = crate::seltext::side_for(((x - origin.0) - col as f32 * cw) / cw, fraction);
    (col, row, side)
}

/// The values `changeFontSizeDirection:` writes to a session's profile (`PTYSession.m:8297-8337`):
/// a nonzero `dir` (points) grows the pane's current Normal Font and Non Ascii Font by `dir` points and
/// leaves the spacings alone; 0 takes the original (saved) profile's fonts and spacings.
pub fn size_values(current: &Profile, original: &Profile, dir: f32) -> Vec<(String, Option<toml::Value>)> {
    if dir != 0.0 {
        let mut values: Vec<(String, Option<toml::Value>)> = ["Normal Font", "Non Ascii Font"].into_iter().map(|k| (k.to_string(), Some(toml::Value::String(profile::grow_font_value(&current.string(k), dir))))).collect();
        // The sized entries of the Special Font Config grow with the fonts; a table with none
        // stays as it is and takes no override.
        let table = current.string(SPECIAL_FONT_CONFIG);
        let grown = crate::font_table::grown_value(&table, f64::from(dir));
        if grown != table {
            values.push((SPECIAL_FONT_CONFIG.to_string(), Some(toml::Value::String(grown))));
        }
        return values;
    }
    ["Normal Font", "Non Ascii Font", "Horizontal Spacing", "Vertical Spacing", SPECIAL_FONT_CONFIG].into_iter().map(|k| (k.to_string(), original.values.get(k).cloned())).collect()
}

/// The panes a size change reaches (`sessionsToAdjustFontSize`, `AFS:55-67`): the focused pane,
/// or, with `FontChangeAffectsBroadcastingSessions` on and the focused pane receiving
/// broadcast input, every pane of `all` that receives it.
pub fn adjust_targets(focused: PaneId, all: &[PaneId], key_on: bool, receives: &dyn Fn(PaneId) -> bool) -> Vec<PaneId> {
    if key_on && receives(focused) {
        return all.iter().copied().filter(|p| receives(*p)).collect();
    }
    vec![focused]
}

/// The zoom an Automatic Profile Switch keeps (`PTYSession.m:20830-20895`): how many points the
/// session's Normal Font is over its original profile's, 0 when the key is off.
pub fn zoom_delta(current: &Profile, original: &Profile, keep: bool) -> f32 {
    if keep { profile::font_delta(&current.string("Normal Font"), &original.string("Normal Font")) } else { 0.0 }
}

/// Grows the saved profile `guid`'s Normal Font and Non Ascii Font by `delta` (`AFS:90-98`,
/// "Size Changes Affect Profile"), so a new session opens at the new size.
pub fn grow_saved_profile(store: &mut Store, guid: &str, delta: i32) {
    let Some(p) = store.get_mut(guid) else { return };
    for k in ["Normal Font", "Non Ascii Font"] {
        let grown = profile::grow_font_value(&p.string(k), delta as f32);
        p.set(k, toml::Value::String(grown));
    }
    let table = p.string(SPECIAL_FONT_CONFIG);
    let grown = crate::font_table::grown_value(&table, f64::from(delta));
    if grown != table {
        p.set(SPECIAL_FONT_CONFIG, toml::Value::String(grown));
    }
}

/// The face and cell of every pane of a window, measured ahead of the frame: `pane_cell` only
/// reads it, so a new pane is sized at its own cell before it ever draws.
#[derive(Default)]
pub struct PaneFaces {
    /// The raw profile values each pane's key was built from, with the key: a pane whose values
    /// are unchanged keeps its key (no font parse, no lookup) on the frame's second call.
    raw: HashMap<PaneId, (RawFont, FaceKey)>,
    by_pane: HashMap<PaneId, (FaceKey, u16, (f32, f32))>,
    /// The cell each pane's grid was last sized at (`mark_laid`): a pane whose cell moved since
    /// (a font edit in its profile, a scale change) needs its grid again (`stale`).
    laid: HashMap<PaneId, (f32, f32)>,
}

/// What a face key is built from, as the profile stores it, plus the window's scale
/// (`face_key_for`'s inputs).
#[derive(Clone, Debug, PartialEq)]
pub struct RawFont {
    normal: String,
    alt: Option<String>,
    spacing: (u64, u64),
    scale: u32,
    bold: bool,
    italic: bool,
    ligatures: (bool, bool),
    anti_alias: (bool, bool),
    thin: i64,
    powerline: bool,
    /// The stored Special Font Config text.
    table: String,
}

impl RawFont {
    fn of(p: &Profile, scale: f32) -> RawFont {
        RawFont {
            normal: p.string("Normal Font"),
            alt: p.bool("Use Non-ASCII Font").then(|| p.string("Non Ascii Font")),
            spacing: (p.float("Horizontal Spacing").to_bits(), p.float("Vertical Spacing").to_bits()),
            scale: scale.to_bits(),
            bold: p.bool("Use Bold Font"),
            italic: p.bool("Use Italic Font"),
            ligatures: (p.bool("ASCII Ligatures"), p.bool("Non-ASCII Ligatures")),
            anti_alias: (p.bool("ASCII Anti Aliased"), p.bool("Non-ASCII Anti Aliased")),
            thin: p.int("Thin Strokes"),
            powerline: p.bool("Draw Powerline Glyphs"),
            table: p.string(SPECIAL_FONT_CONFIG),
        }
    }
}

impl RawFont {
    /// Whether `RawFont::of(p, scale)` would equal `self`, compared in place: the frame asks this
    /// of every pane, and only a miss builds (and copies) a new one.
    fn matches(&self, p: &Profile, scale: f32) -> bool {
        self.normal == p.str_ref("Normal Font")
            && match &self.alt {
                Some(alt) => p.bool("Use Non-ASCII Font") && alt == p.str_ref("Non Ascii Font"),
                None => !p.bool("Use Non-ASCII Font"),
            }
            && self.spacing == (p.float("Horizontal Spacing").to_bits(), p.float("Vertical Spacing").to_bits())
            && self.scale == scale.to_bits()
            && self.bold == p.bool("Use Bold Font")
            && self.italic == p.bool("Use Italic Font")
            && self.ligatures == (p.bool("ASCII Ligatures"), p.bool("Non-ASCII Ligatures"))
            && self.anti_alias == (p.bool("ASCII Anti Aliased"), p.bool("Non-ASCII Anti Aliased"))
            && self.thin == p.int("Thin Strokes")
            && self.powerline == p.bool("Draw Powerline Glyphs")
            && self.table == p.str_ref(SPECIAL_FONT_CONFIG)
    }
}

impl PaneFaces {
    /// The key cached for `pane` when its raw values still match profile `p` at `scale`.
    fn cached(&self, pane: PaneId, p: &Profile, scale: f32) -> Option<FaceKey> {
        self.raw.get(&pane).filter(|(r, _)| r.matches(p, scale)).map(|(_, k)| k.clone())
    }

    /// Measures the face of each pane in `panes` (on first use only: the renderer remembers a
    /// key), refreshes the cells (a scale change re-measures in place) and forgets the rest.
    pub fn ensure(&mut self, renderer: &mut Renderer, panes: impl IntoIterator<Item = (PaneId, FaceKey)>) {
        let mut seen = std::collections::HashSet::new();
        for (pane, key) in panes {
            seen.insert(pane);
            match self.by_pane.get_mut(&pane) {
                Some(e) if e.0 == key => e.2 = renderer.face_cell(e.1),
                _ => {
                    let id = renderer.face(&key);
                    self.by_pane.insert(pane, (key, id, renderer.face_cell(id)));
                }
            }
        }
        self.by_pane.retain(|p, _| seen.contains(p));
        self.raw.retain(|p, _| seen.contains(p));
        self.laid.retain(|p, _| seen.contains(p));
    }

    /// The panes were just sized at their current cells.
    pub fn mark_laid(&mut self) {
        self.laid = self.by_pane.iter().map(|(p, e)| (*p, e.2)).collect();
    }

    /// A pane has a cell its grid was not sized at.
    pub fn stale(&self) -> bool {
        self.by_pane.iter().any(|(p, e)| self.laid.get(p) != Some(&e.2))
    }

    pub fn face(&self, pane: PaneId) -> Option<u16> {
        self.by_pane.get(&pane).map(|e| e.1)
    }

    pub fn cell(&self, pane: PaneId) -> Option<(f32, f32)> {
        self.by_pane.get(&pane).map(|e| e.2)
    }
}

impl App {
    /// The face key of `p` in this window (`face_key_for`, with the renderer's memoized lookup).
    pub(super) fn face_key(&self, p: &Profile) -> FaceKey {
        let renderer = self.gpu.as_ref().map(|g| &g.renderer);
        face_key_for(p, self.scale(), &|name| renderer.map_or_else(|| name.to_string(), |r| r.resolve_family(name)))
    }

    /// Measures the face of every pane of this window, at the window's scale. Called at the top
    /// of every frame, after a profile edit, and before any code sizes a new pane, so that
    /// `pane_cell` (which only reads) is never stale; a face is measured once per key. A pane
    /// whose profile values are unchanged keeps its cached key and copies nothing (the Special
    /// Font Config text can be 256 KiB); only a changed pane builds a new `RawFont` and key.
    pub(super) fn ensure_faces(&mut self) {
        if self.gpu.is_none() {
            return;
        }
        let scale = self.scale();
        let built: Vec<(PaneId, Option<RawFont>, FaceKey)> = self
            .tabs
            .iter()
            .flat_map(|t| t.layout.panes())
            .filter_map(|id| {
                let pane = self.panes.get(&id)?;
                let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
                Some(match self.pane_faces.cached(id, p, scale) {
                    Some(key) => (id, None, key),
                    None => (id, Some(RawFont::of(p, scale)), self.face_key(p)),
                })
            })
            .collect();
        let mut keys = Vec::with_capacity(built.len());
        for (id, raw, key) in built {
            if let Some(raw) = raw {
                self.pane_faces.raw.insert(id, (raw, key.clone()));
            }
            keys.push((id, key));
        }
        let Some(g) = self.gpu.as_mut() else { return };
        g.renderer.set_scale(scale);
        self.pane_faces.ensure(&mut g.renderer, keys);
    }

    /// The face of the pane's profile (the default face before the pane is measured).
    pub(super) fn pane_face(&mut self, pane: PaneId) -> u16 {
        if self.pane_faces.face(pane).is_none() {
            self.ensure_faces();
        }
        self.pane_faces.face(pane).unwrap_or(crate::render::DEFAULT_FACE)
    }

    /// The cell a pane's text is laid out in: its own face's, else its tab's focused pane's,
    /// else the default cell (before the GPU exists or for an unknown pane). Only reads:
    /// `ensure_faces` measures.
    pub(super) fn pane_cell(&self, pane: PaneId) -> (f32, f32) {
        self.pane_faces.cell(pane).or_else(|| self.tab_focused_cell(pane)).unwrap_or_else(|| self.cell())
    }

    /// The font's own cell of a pane (without spacings): what the cursor is measured against.
    pub(super) fn pane_base_cell(&self, pane: PaneId) -> (f32, f32) {
        match (self.pane_faces.face(pane), self.gpu.as_ref()) {
            (Some(id), Some(g)) => g.renderer.face_base(id),
            _ => self.base_cell(),
        }
    }

    /// The cell of the focused pane of the current tab (window sizing and snapping follow it).
    pub(super) fn focused_cell(&self) -> (f32, f32) {
        self.tabs.get(self.active).map_or_else(|| self.cell(), |t| self.pane_cell(t.focused))
    }

    /// The cell of the pane under the pointer, else the focused pane's: a line of the wheel.
    pub(super) fn pointer_pane_cell(&self) -> (f32, f32) {
        self.pane_at(self.cursor.0, self.cursor.1).map_or_else(|| self.focused_cell(), |(id, _)| self.pane_cell(id))
    }

    fn tab_focused_cell(&self, pane: PaneId) -> Option<(f32, f32)> {
        let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&pane))?;
        self.pane_faces.cell(tab.focused)
    }

    /// The columns and rows a pane's rectangle holds at the pane's own cell.
    pub(super) fn grid_size_for(&self, pane: PaneId, rect: Rect) -> (u16, u16) {
        // Its scrollbar and its Adjacent timestamp column are out (13m's room), at its own cell.
        self.grid_size_in(rect, self.pane_cell(pane), self.pane_room(pane))
    }

    /// The cell a new pane of profile `p` will have: its face measured now.
    pub(super) fn profile_cell(&mut self, p: &Profile) -> (f32, f32) {
        let key = self.face_key(p);
        let scale = self.scale();
        let Some(g) = self.gpu.as_mut() else { return self.cell() };
        g.renderer.set_scale(scale);
        let id = g.renderer.face(&key);
        g.renderer.face_cell(id)
    }

    /// The panes Make Text Bigger, Smaller and Normal Size act on (`sessionsToAdjustFontSize`,
    /// `AFS:55-67`): the focused pane, or its broadcast group with
    /// `FontChangeAffectsBroadcastingSessions`. A tmux pane takes its whole tmux window along
    /// (their sizes are tmux's, so the window's panes share one text size).
    pub(super) fn sessions_to_adjust(&self) -> Vec<PaneId> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        let all: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let view = self.broadcast_view();
        let mut targets = adjust_targets(tab.focused, &all, self.globals.bool("FontChangeAffectsBroadcastingSessions"), &|p| self.broadcast.receives(&view, p));
        widen_to_tmux_windows(&mut targets, &all, &|p| self.tmux_of(p).map(|(gateway, _, window)| (gateway, window)));
        targets
    }

    /// `changeFontSizeDirection:` (`PTYSession.m:8297-8337`) on one pane: a nonzero `dir` grows its
    /// fonts by `dir` points, 0 restores its saved profile's fonts and spacings. The pane's
    /// profile becomes a private copy holding them. Writes only: the caller refreshes and lays
    /// out once for every pane it changed.
    pub(super) fn change_font_size(&mut self, pane: PaneId, dir: i32) {
        self.write_font_size(pane, dir as f32);
    }

    /// `change_font_size` with a fractional delta (an Automatic Profile Switch keeps one).
    pub(super) fn write_font_size(&mut self, pane: PaneId, delta: f32) {
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        // Make Text Normal Size on a pane with no copy has nothing to restore (`dir || isDivorced`).
        if delta == 0.0 && !self.store.is_session(&guid) {
            return;
        }
        let Some(own) = self.divorce(pane) else { return };
        let original = self.store.get(&self.store.shared_guid(&own)).unwrap_or_else(|| self.store.default_profile()).clone();
        let Some(current) = self.store.get(&own).cloned() else { return };
        self.store.set_session(&own, &size_values(&current, &original, delta));
    }

    /// Make Text Bigger (1) and Make Text Smaller (-1) from the menu and the pinch
    /// (`iTermAdjustFontSizeHelper` `biggerFont:`, `AFS:22-110`): the pane or its broadcast group
    /// changes, and with "Size Changes Affect Profile" their saved profiles too.
    pub(super) fn adjust_font_size(&mut self, delta: i32) {
        self.resize_text(delta);
    }

    /// Make Text Normal Size: the pane or its broadcast group takes its saved profile's fonts
    /// again. (Restore Text and Session Size does the same until 13f's window sizing restores
    /// the rows and columns.)
    pub(super) fn return_to_default_size(&mut self) {
        self.resize_text(0);
    }

    fn resize_text(&mut self, dir: i32) {
        let targets = self.sessions_to_adjust();
        let Some(focused) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        let (cell, grid) = (self.pane_cell(focused), self.grid_size_for(focused, self.content_area()));
        for pane in &targets {
            self.change_font_size(*pane, dir);
        }
        if dir != 0 && self.globals.bool(SIZE_CHANGES_AFFECT_PROFILE) {
            let saved: std::collections::BTreeSet<String> = targets.iter().filter_map(|p| self.panes.get(p)).map(|p| self.store.shared_guid(&p.profile)).collect();
            for guid in saved {
                grow_saved_profile(&mut self.store, &guid, dir);
            }
            self.save_due = Some(Instant::now() + SAVE_DELAY);
        }
        // Every copy is written: one refresh measures the new faces, one layout resizes the
        // panes whose cell moved (a tmux pane's grid stays tmux's; its report follows).
        self.refresh_panes();
        self.relayout();
        // The window keeps the tab's grid at the new cell (`reallyChangeSessionFontSize:adjustWindow:`,
        // `TAB:2189-2205`); no fit when the focused pane's cell did not change.
        if self.pane_cell(focused) != cell && self.globals.bool("AdjustWindowForFontSizeChange") && self.window_can_fit() {
            self.fit_window_to_grid(grid);
        }
    }

    /// The underline numbers and scale of a pane's face for `scene::pane_with`.
    pub(super) fn draw_prefs_for(&self, face: u16) -> scene::DrawPrefs {
        let metrics = self.gpu.as_ref().map(|g| g.renderer.face_underline(face)).unwrap_or(scene::DrawPrefs::default().metrics);
        let mut prefs = scene::DrawPrefs::from_globals(&self.globals, metrics, self.scale());
        if let Some(g) = self.gpu.as_ref() {
            (prefs.ligatures, prefs.alt_level) = g.renderer.face_ligatures(face, prefs.ligatures.prefer_speed);
            prefs.masks = g.renderer.face_masks(face, prefs.masks.retina, prefs.masks.force_aa_on_retina);
            prefs.powerline = g.renderer.face_powerline(face);
            prefs.table = g.renderer.face_table(face);
        }
        prefs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(values: &[(&str, toml::Value)]) -> Profile {
        let mut p = Profile::new("x");
        for (k, v) in values {
            p.set(k, v.clone());
        }
        p
    }

    fn store_with(font: &str) -> (Store, String) {
        let mut store = Store::fresh(std::path::Path::new("none.toml"));
        let mut p = Profile::new("Work");
        p.set("Normal Font", font.into());
        p.set("Non Ascii Font", font.into());
        let guid = p.guid();
        store.add(p);
        (store, guid)
    }

    fn apply(store: &mut Store, guid: &str, dir: i32) -> String {
        let new = store.divorce(guid);
        let values = size_values(store.get(&new).unwrap(), store.get(&store.shared_guid(&new)).unwrap(), dir as f32);
        store.set_session(&new, &values);
        new
    }

    /// Review Focus 5 (R3: the focused session only, was every pane).
    #[test]
    fn size_commands_follow_iterm2() {
        let (mut store, saved) = store_with("Consolas 12");
        for k in ["Horizontal Spacing", "Vertical Spacing"] {
            store.get_mut(&saved).unwrap().set(k, 1.2.into());
        }
        // Two panes on one saved profile; the first one grows.
        let first = apply(&mut store, &saved, 1);
        assert_ne!(first, saved, "the pane has a copy of its own");
        let copy = store.get(&first).unwrap();
        assert_eq!(copy.string("Normal Font"), "Consolas 13");
        assert_eq!(copy.string("Non Ascii Font"), "Consolas 13", "the non-ASCII font grew too");
        assert_eq!(copy.float("Horizontal Spacing"), 1.2, "spacing is kept");
        let second = store.get(&saved).unwrap();
        assert_eq!(second.string("Normal Font"), "Consolas 12", "the other pane keeps the saved profile");
        // A second step grows from the copy.
        let again = apply(&mut store, &first, -1);
        assert_eq!(again, first);
        assert_eq!(store.get(&first).unwrap().string("Normal Font"), "Consolas 12");
        // Zoom the copy and give it other spacing, then Make Text Normal Size.
        apply(&mut store, &first, 3);
        store.set_session(&first, &[("Horizontal Spacing".into(), Some(1.5.into())), ("Vertical Spacing".into(), Some(0.9.into()))]);
        let restored = size_values(store.get(&first).unwrap(), store.get(&saved).unwrap(), 0.0);
        store.set_session(&first, &restored);
        let copy = store.get(&first).unwrap();
        assert_eq!(copy.string("Normal Font"), "Consolas 12");
        assert_eq!(copy.string("Non Ascii Font"), "Consolas 12");
        assert_eq!((copy.float("Horizontal Spacing"), copy.float("Vertical Spacing")), (1.2, 1.2));
        assert!(store.sessions.iter().all(|s| !s.overridden.contains("Normal Font")), "back to the saved size overrides nothing");
        // The drawn range holds: no press is hidden at either end.
        let (mut store, saved) = store_with("Consolas 4");
        let low = apply(&mut store, &saved, -1);
        assert_eq!(store.get(&low).unwrap().string("Normal Font"), "Consolas 4");
        let (mut store, saved) = store_with("Consolas 200");
        let high = apply(&mut store, &saved, -1);
        assert_eq!(store.get(&high).unwrap().font("Normal Font").size_pt, 127.0, "a size above the drawn range steps from 128");
    }

    #[test]
    fn broadcast_sizes_follow_iterm2() {
        let (a, b, c, outside) = (1, 2, 3, 9);
        let all = [a, b, c, outside];
        let receives = |p: PaneId| p != outside;
        assert_eq!(adjust_targets(a, &all, false, &receives), vec![a], "the key off: the focused pane only");
        assert_eq!(adjust_targets(a, &all, true, &receives), vec![a, b, c], "the key on: every receiving pane");
        assert_eq!(adjust_targets(outside, &all, true, &receives), vec![outside], "a pane outside the broadcast grows alone");
    }

    #[test]
    fn profile_sizes_follow_iterm2() {
        let (mut store, saved) = store_with("Consolas 12");
        let pane = apply(&mut store, &saved, 1);
        let original = store.shared_guid(&pane);
        grow_saved_profile(&mut store, &original, 1);
        let p = store.get(&saved).unwrap();
        assert_eq!(p.string("Normal Font"), "Consolas 13", "a new session opens at the new size");
        assert_eq!(p.string("Non Ascii Font"), "Consolas 13");
        // The menu item shows the check.
        let menu = include_str!("menu.rs");
        assert!(menu.contains("\"toggleSizeChangesAffectProfile:\""), "the toggle has an arm and a check");
        assert!(menu.contains("check(\"Size Changes Update Profile\", self.globals.bool(fonts_ui::SIZE_CHANGES_AFFECT_PROFILE))"));
        assert_eq!(SIZE_CHANGES_AFFECT_PROFILE, "Size Changes Affect Profile");
        let key = crate::schema::find_global("Size Changes Affect Profile").expect("the stored key");
        assert_eq!(key.default, Some(toml::Value::Boolean(false)));
    }

    #[test]
    fn aps_keeps_the_zoom() {
        let (mut store, a) = store_with("Consolas 12");
        let mut other = Profile::new("Other");
        other.set("Normal Font", "Consolas 10".into());
        other.set("Non Ascii Font", "Consolas 10".into());
        let b = other.guid();
        store.add(other);
        let pane = apply(&mut store, &a, 2);
        assert_eq!(store.get(&pane).unwrap().string("Normal Font"), "Consolas 14");
        let delta = |keep: bool| zoom_delta(store.get(&pane).unwrap(), store.get(&store.shared_guid(&pane)).unwrap(), keep);
        assert_eq!(delta(true), 2.0);
        assert_eq!(delta(false), 0.0, "the key off keeps nothing");
        // The switch: the copy goes, the pane takes B, then the delta is applied once.
        let kept = delta(true);
        let new = store.divorce(&b);
        let values = size_values(store.get(&new).unwrap(), store.get(&b).unwrap(), kept);
        store.set_session(&new, &values);
        assert_eq!(store.get(&new).unwrap().string("Normal Font"), "Consolas 12", "10 and the kept 2");
        assert_eq!(store.get(&b).unwrap().string("Normal Font"), "Consolas 10", "the saved profile is untouched");
        let source = include_str!("aps_ui.rs");
        let (zoom, remarry) = (source.find("zoom_delta(").expect("measured"), source.find("self.remarry(").unwrap());
        assert!(zoom < remarry, "measured before the switch");
        assert!(source[remarry..].contains("write_font_size("), "re-applied after it");
    }

    #[test]
    fn face_key_follows_profile() {
        let same = |s: &str| s.to_string();
        let p = profile(&[
            ("Normal Font", "Cascadia Mono 14".into()),
            ("Use Non-ASCII Font", true.into()),
            ("Non Ascii Font", "MS Gothic 12".into()),
            ("Horizontal Spacing", 1.1.into()),
            ("Vertical Spacing", 1.0.into()),
        ]);
        let k = face_key_for(&p, 1.5, &same);
        assert_eq!(k.family, "Cascadia Mono");
        assert_eq!(k.alt_family.as_deref(), Some("MS Gothic"));
        assert_eq!(k.px(), 28.0);
        assert_eq!((k.h_bits, k.v_bits), (1.1f32.to_bits(), 1.0f32.to_bits()));
        let off = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Non-ASCII Font", false.into())]);
        assert_eq!(face_key_for(&off, 1.0, &same).alt_family, None);
        // A size outside the drawn range is drawn at the range's end.
        let huge = profile(&[("Normal Font", "Cascadia Mono 500".into())]);
        assert_eq!(face_key_for(&huge, 1.0, &same).px(), crate::render::quantize_px(128.0 * 4.0 / 3.0));
        // Names go through the resolver.
        let up = |s: &str| s.to_uppercase();
        assert_eq!(face_key_for(&p, 1.0, &up).family, "CASCADIA MONO");
    }

    #[test]
    fn bold_and_italic_switches_make_a_new_key() {
        let same = |s: &str| s.to_string();
        let on = profile(&[("Normal Font", "Cascadia Mono 14".into())]);
        let no_bold = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Bold Font", false.into())]);
        let no_italic = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Italic Font", false.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, &same);
        assert!(key(&on).use_bold && key(&on).use_italic, "iTerm2's defaults are on");
        assert!(!key(&no_bold).use_bold && key(&no_bold).use_italic);
        assert!(key(&no_italic).use_bold && !key(&no_italic).use_italic);
        assert_ne!(key(&on), key(&no_bold));
        assert_ne!(key(&on), key(&no_italic));
        // The per-pane cache is keyed on RawFont: a switch change must change it, or a stale key is served.
        assert_ne!(RawFont::of(&on, 1.0), RawFont::of(&no_bold, 1.0));
        assert_ne!(RawFont::of(&on, 1.0), RawFont::of(&no_italic, 1.0));
    }

    #[test]
    fn ligature_switches_make_a_new_key() {
        let same = |s: &str| s.to_string();
        let base = [("Normal Font", toml::Value::from("Cascadia Code 14"))];
        let off = profile(&base);
        let ascii = profile(&[base[0].clone(), ("ASCII Ligatures", true.into())]);
        let non_ascii = profile(&[base[0].clone(), ("Non-ASCII Ligatures", true.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, &same);
        assert!(!key(&off).ascii_ligatures && !key(&off).non_ascii_ligatures, "iTerm2's defaults are off");
        assert!(key(&ascii).ascii_ligatures && !key(&ascii).non_ascii_ligatures);
        assert!(!key(&non_ascii).ascii_ligatures && key(&non_ascii).non_ascii_ligatures);
        assert_ne!(key(&off), key(&ascii));
        assert_ne!(key(&off), key(&non_ascii));
        assert_ne!(RawFont::of(&off, 1.0), RawFont::of(&ascii, 1.0));
        assert_ne!(RawFont::of(&off, 1.0), RawFont::of(&non_ascii, 1.0));
    }

    #[test]
    fn anti_alias_switches_are_part_of_the_face() {
        let same = |s: &str| s.to_string();
        let base = [("Normal Font", toml::Value::from("Cascadia Mono 14"))];
        let stock = profile(&base);
        let ascii_off = profile(&[base[0].clone(), ("ASCII Anti Aliased", false.into())]);
        let non_ascii_off = profile(&[base[0].clone(), ("Non-ASCII Anti Aliased", false.into())]);
        let thin_never = profile(&[base[0].clone(), ("Thin Strokes", 0.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, &same);
        // The stock profile is anti-aliased with Thin Strokes at Retina only.
        assert_eq!((key(&stock).ascii_aa, key(&stock).non_ascii_aa, key(&stock).thin), (true, true, 4));
        assert!(!key(&ascii_off).ascii_aa && key(&ascii_off).non_ascii_aa);
        assert!(key(&non_ascii_off).ascii_aa && !key(&non_ascii_off).non_ascii_aa);
        assert_eq!(key(&thin_never).thin, 0);
        for other in [&ascii_off, &non_ascii_off, &thin_never] {
            assert_ne!(key(&stock), key(other));
            // The per-pane key cache is keyed on RawFont: a change must change it, or a stale key is served.
            assert_ne!(RawFont::of(&stock, 1.0), RawFont::of(other, 1.0));
        }
    }

    #[test]
    fn powerline_switch_is_part_of_the_face() {
        let same = |s: &str| s.to_string();
        let base = [("Normal Font", toml::Value::from("Cascadia Mono 14"))];
        let off = profile(&base);
        let on = profile(&[base[0].clone(), ("Draw Powerline Glyphs", true.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, &same);
        assert!(!key(&off).powerline && key(&on).powerline, "iTerm2's default is off");
        assert_ne!(key(&off), key(&on));
        // The per-pane key cache is keyed on RawFont: a change must change it, or a stale key is served.
        assert_ne!(RawFont::of(&off, 1.0), RawFont::of(&on, 1.0));
    }

    /// 13n Task 9: the Special Font Config is part of the face, as the table itself (P5).
    #[test]
    fn special_font_config_is_part_of_the_face() {
        let same = |s: &str| s.to_string();
        let one = r#"{"entries":[{"start":57344,"count":16,"fontName":"Consolas"}],"version":2}"#;
        let reordered = r#"{"version":2,"entries":[{"fontName":"Consolas","count":16,"start":57344}]}"#;
        let other = r#"{"entries":[{"start":57344,"count":17,"fontName":"Consolas"}],"version":2}"#;
        let base = [("Normal Font", toml::Value::from("Cascadia Mono 14"))];
        let none = profile(&base);
        let with = |text: &str| profile(&[base[0].clone(), ("Special Font Config", text.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, &same);
        assert_eq!(key(&none).table, None, "no table, no key text");
        assert_eq!(key(&with("")).table, None);
        assert_eq!(key(&with("not json")).table, None, "a table that does not parse is no table");
        assert_eq!(key(&with(r#"{"entries":[{"start":-1,"count":999999999999,"fontName":"A"}],"version":2}"#)).table, None, "nor one out of bounds");
        assert!(key(&with(one)).table.is_some());
        assert_eq!(key(&with(one)), key(&with(reordered)), "one table, one face, whatever the spelling");
        assert_ne!(key(&with(one)), key(&none));
        assert_ne!(key(&with(one)), key(&with(other)), "the text and not a hash of it decides");
        // The per-pane key cache is keyed on RawFont: a change must change it, or a stale key is served.
        assert_ne!(RawFont::of(&none, 1.0), RawFont::of(&with(one), 1.0));
        assert_ne!(RawFont::of(&with(one), 1.0), RawFont::of(&with(other), 1.0));
    }

    /// Make Text Bigger and Smaller grow the sized entries of the Special Font Config with the
    /// fonts (`AFS:90-98`, `PTYSession.m:8297-8337`); Normal Size takes the saved table back.
    #[test]
    fn special_font_config_sizes_grow_with_the_text() {
        let table = |size: f64| crate::font_table::Config::with_entries(vec![crate::font_table::Entry { start: 0xE000, count: 16, destination: None, font_name: "Consolas".into(), point_size: Some(size) }]).string();
        let (mut store, saved) = store_with("Consolas 12");
        store.get_mut(&saved).unwrap().set("Special Font Config", table(12.0).into());
        let first = apply(&mut store, &saved, 1);
        assert_eq!(store.get(&first).unwrap().string("Special Font Config"), table(13.0));
        assert_eq!(store.get(&saved).unwrap().string("Special Font Config"), table(12.0), "the saved profile is untouched");
        apply(&mut store, &first, -3);
        assert_eq!(store.get(&first).unwrap().string("Special Font Config"), table(10.0));
        // Normal Size restores the saved profile's table.
        let restored = size_values(store.get(&first).unwrap(), store.get(&saved).unwrap(), 0.0);
        store.set_session(&first, &restored);
        assert_eq!(store.get(&first).unwrap().string("Special Font Config"), table(12.0));
        assert!(store.sessions.iter().all(|s| !s.overridden.contains("Special Font Config")), "back to the saved size overrides nothing");
        // A profile with no table, or an unsized one, gets no override for it.
        let (mut plain, saved) = store_with("Consolas 12");
        let copy = apply(&mut plain, &saved, 1);
        assert!(!plain.sessions.iter().any(|s| s.profile.guid() == copy && s.overridden.contains("Special Font Config")));
        // The size steps stay in the drawn range.
        let (mut low, saved) = store_with("Consolas 12");
        low.get_mut(&saved).unwrap().set("Special Font Config", table(4.0).into());
        let c = apply(&mut low, &saved, -1);
        assert_eq!(low.get(&c).unwrap().string("Special Font Config"), table(4.0));
        // Size Changes Affect Profile grows the saved profile's table too.
        let (mut store, saved) = store_with("Consolas 12");
        store.get_mut(&saved).unwrap().set("Special Font Config", table(12.0).into());
        grow_saved_profile(&mut store, &saved, 2);
        assert_eq!(store.get(&saved).unwrap().string("Special Font Config"), table(14.0));
        // An APS switch keeps the zoom on the new profile's table as well.
        let values = size_values(store.get(&saved).unwrap(), store.get(&saved).unwrap(), 2.0);
        assert!(values.iter().any(|(k, v)| k == "Special Font Config" && v.as_ref().and_then(|v| v.as_str()) == Some(table(16.0).as_str())));
    }

    /// P16: no installed-font scan on the frame path, a pointer move or a cell lookup.
    #[test]
    fn no_font_scan_in_build_frame() {
        let source = include_str!("mod.rs");
        let needle = |s: &str| source.find(s).unwrap_or_else(|| panic!("{s} exists"));
        let body = |from: &str| {
            let start = needle(from);
            let end = source[start + 10..].find("\n    fn ").or_else(|| source[start + 10..].find("\n    pub(super) fn ")).map_or(source.len(), |e| e + start + 10);
            &source[start..end]
        };
        let (slow, scan) = (concat!(".fa", "ces()"), concat!(".resolve_", "family("));
        for name in ["fn build_frame(", "fn cell_at("] {
            let b = body(name);
            assert!(!b.contains(slow) && !b.contains(scan), "{name} scans fonts");
        }
        let fonts = include_str!("fonts_ui.rs");
        let pane_cell = &fonts[fonts.find("fn pane_cell(").unwrap()..];
        let pane_cell = &pane_cell[..pane_cell.find("\n    }").unwrap()];
        assert!(!pane_cell.contains(slow) && !pane_cell.contains(scan));
    }

    #[test]
    fn pane_cell_is_warm_before_the_first_frame() {
        let (device, queue) = crate::render::test_gpu::shared();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let mut faces = PaneFaces::default();
        let default = FaceKey::new("Cascadia Mono", None, 16.0, 1.0, 1.0);
        let big = FaceKey::new("Cascadia Mono", None, 32.0, 1.0, 1.0);
        r.set_fonts("Cascadia Mono", None, 16.0, 1.0, 1.0);
        faces.ensure(&mut r, [(1, default.clone())]);
        assert_eq!((faces.face(1), faces.cell(1)), (Some(crate::render::DEFAULT_FACE), Some(r.cell)));
        // A split of another profile: its own cell as soon as ensure ran, no frame between.
        faces.ensure(&mut r, [(1, default.clone()), (2, big.clone())]);
        let own = faces.cell(2).unwrap();
        assert!(own.0 > r.cell.0 * 1.8 && own.1 > r.cell.1 * 1.8, "{own:?} {:?}", r.cell);
        assert_eq!(faces.cell(2), Some(r.face_cell(faces.face(2).unwrap())));
        // A pane whose cell moved after its grid was sized is stale until the next layout.
        faces.mark_laid();
        assert!(!faces.stale());
        faces.ensure(&mut r, [(1, default.clone()), (2, big.clone()), (3, FaceKey::new("Cascadia Mono", None, 20.0, 1.0, 1.0))]);
        assert!(faces.stale(), "a new pane has not been laid out");
        faces.mark_laid();
        faces.ensure(&mut r, [(1, default.clone()), (2, FaceKey::new("Cascadia Mono", None, 24.0, 1.0, 1.0)), (3, FaceKey::new("Cascadia Mono", None, 20.0, 1.0, 1.0))]);
        assert!(faces.stale(), "pane 2's profile changed its font");
        faces.mark_laid();
        assert!(!faces.stale());
        // A pane that is gone is forgotten; an unknown one has no cell.
        faces.ensure(&mut r, [(2, big)]);
        assert_eq!(faces.cell(1), None);
        // A scale change re-measures in place: the same ids, the cells follow.
        let id = faces.face(2).unwrap();
        r.set_scale(2.0);
        faces.ensure(&mut r, [(2, FaceKey::new("Cascadia Mono", None, 64.0, 1.0, 1.0))]);
        assert_eq!(faces.face(2), Some(id));
        assert!(faces.cell(2).unwrap().0 > own.0 * 1.8);
    }

    /// 13n final 5: a size change on one tmux pane reaches every pane of its tmux window (and no
    /// pane of another window, another gateway or no tmux at all).
    #[test]
    fn a_size_change_reaches_every_pane_of_a_tmux_window() {
        // Panes 1, 2 and 3 are tmux window 7 of gateway 100; 4 is window 8; 5 is window 7 of gateway 200; 6 is no tmux pane.
        let window_of = |p: PaneId| match p {
            1..=3 => Some((100, 7)),
            4 => Some((100, 8)),
            5 => Some((200, 7)),
            _ => None,
        };
        let all = [1, 2, 3, 4, 5, 6];
        let mut targets = vec![2];
        widen_to_tmux_windows(&mut targets, &all, &window_of);
        targets.sort();
        assert_eq!(targets, vec![1, 2, 3], "the focused pane's tmux window, nothing else");
        let mut plain = vec![6];
        widen_to_tmux_windows(&mut plain, &all, &window_of);
        assert_eq!(plain, vec![6], "a pane that is not tmux's takes nothing along");
        let mut group = vec![1, 4];
        widen_to_tmux_windows(&mut group, &all, &window_of);
        group.sort();
        assert_eq!(group, vec![1, 2, 3, 4], "each target's window is widened, no pane twice");
    }

    /// 13n final 5: the per-frame check compares borrowed values (no 256 KiB copy of the table per
    /// pane per frame) and agrees with comparing two built `RawFont`s.
    #[test]
    fn a_pane_whose_values_are_unchanged_matches_without_building() {
        let stock = Profile::new("A");
        let mut table = stock.clone();
        table.set(SPECIAL_FONT_CONFIG, toml::Value::String(crate::font_table::Config::with_entries(vec![crate::font_table::Entry { start: 0xE000, count: 16, destination: None, font_name: "Consolas".into(), point_size: None }]).string()));
        let mut bold = stock.clone();
        bold.set("Use Bold Font", toml::Value::Boolean(!stock.bool("Use Bold Font")));
        let mut alt = stock.clone();
        alt.set("Use Non-ASCII Font", toml::Value::Boolean(!stock.bool("Use Non-ASCII Font")));
        let mut spaced = stock.clone();
        spaced.set("Horizontal Spacing", toml::Value::Float(1.5));
        let mut font = stock.clone();
        font.set("Normal Font", toml::Value::String("Consolas 20".into()));
        for (a, b) in [(&stock, &stock), (&stock, &table), (&table, &table), (&table, &stock), (&stock, &bold), (&stock, &alt), (&stock, &spaced), (&stock, &font)] {
            let raw = RawFont::of(a, 1.0);
            assert_eq!(raw.matches(b, 1.0), raw == RawFont::of(b, 1.0), "{:?}", raw);
        }
        assert!(RawFont::of(&stock, 1.0).matches(&stock, 1.0));
        assert!(!RawFont::of(&stock, 1.0).matches(&stock, 2.0), "a scale change is a change");
        // The borrowed read is the same text as the owned one.
        assert_eq!(table.str_ref(SPECIAL_FONT_CONFIG), table.string(SPECIAL_FONT_CONFIG));
        assert_eq!(stock.str_ref("Normal Font"), stock.string("Normal Font"));
    }

    #[test]
    fn pointer_cells_follow_the_pane() {
        let origin = (100.0, 40.0);
        // x = 45 into the text: column 4 where cells are 10 wide, column 5 where they are 8.
        assert_eq!(pointer_cell((10.0, 20.0), origin, 145.0, 41.0, 80, 24, 0.5).0, 4);
        assert_eq!(pointer_cell((8.0, 16.0), origin, 145.0, 41.0, 80, 24, 0.5).0, 5);
        // Rows, and the half of the cell.
        let (_, row, side) = pointer_cell((10.0, 20.0), origin, 147.0, 100.0, 80, 24, 0.5);
        assert_eq!((row, side), (3, Side::Right));
        assert_eq!(pointer_cell((10.0, 20.0), origin, 143.0, 100.0, 80, 24, 0.5).2, Side::Left);
        // iTerm2's 0.35: the last 0.35 of the cell picks the next one (R3).
        assert_eq!(pointer_cell((10.0, 20.0), origin, 147.0, 100.0, 80, 24, 0.35).2, Side::Right);
        assert_eq!(pointer_cell((10.0, 20.0), origin, 144.0, 100.0, 80, 24, 0.35).2, Side::Left);
    }
}
