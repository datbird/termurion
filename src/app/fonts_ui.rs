//! Each pane draws with its own profile's fonts (milestone 13n Task 1). The renderer keeps a
//! table of faces (`render::Renderer::face`); a pane's face comes from its profile
//! (`App::face_key`), and its cell size from its face (`App::pane_cell`). Window chrome keeps
//! the default face and `App::cell`. iTerm2 draws every session with its own font table
//! (`PTYSession.m` `setFontTable:horizontalSpacing:verticalSpacing:`), the cell being
//! `PTYTextView.m:227-233`.

use std::collections::HashMap;

use super::*;
use crate::render::{FaceKey, Renderer};

/// The face key of a profile: its Normal Font (plus the shared font offset) at `scale`, the
/// Non-ASCII Font when the profile uses one, and the spacings. `resolve` turns an iTerm2 font
/// name into an installed family (the renderer's memoized lookup, or the identity).
pub fn face_key_for(p: &Profile, scale: f32, offset: f32, resolve: &dyn Fn(&str) -> String) -> FaceKey {
    let mut normal = p.font("Normal Font");
    normal.size_pt = (normal.size_pt + offset).clamp(4.0, 128.0);
    let alt = p.bool("Use Non-ASCII Font").then(|| resolve(&p.font("Non Ascii Font").family));
    let px = normal.size_pt * 4.0 / 3.0 * scale;
    let mut key = FaceKey::new(&resolve(&normal.family), alt.as_deref(), px, p.float("Horizontal Spacing") as f32, p.float("Vertical Spacing") as f32);
    // Use Bold Font and Use Italic Font (`ASB:773-778`): a face that ignores a switch is its own face.
    (key.use_bold, key.use_italic) = (p.bool("Use Bold Font"), p.bool("Use Italic Font"));
    // ASCII Ligatures and Non-ASCII Ligatures decide what the pane's text shapes with.
    (key.ascii_ligatures, key.non_ascii_ligatures) = (p.bool("ASCII Ligatures"), p.bool("Non-ASCII Ligatures"));
    // ASCII Anti Aliased, Non-ASCII Anti Aliased and Thin Strokes decide how the pane's glyph masks are drawn.
    (key.ascii_aa, key.non_ascii_aa, key.thin) = (p.bool("ASCII Anti Aliased"), p.bool("Non-ASCII Anti Aliased"), p.int("Thin Strokes").clamp(0, 4) as u8);
    key
}

/// The columns and rows `rect` (width, height) holds at `cell`, with the margins taken out
/// (`margins::grid_size`): the one path a pane's pixels become a grid.
pub fn grid_for(rect: (f32, f32), cell: (f32, f32), margins: (f32, f32)) -> (u16, u16) {
    margins::grid_size(rect, cell, margins)
}

/// The cell under a point of a pane whose text area starts at `origin`, and which half of the
/// cell it is in.
pub fn pointer_cell(cell: (f32, f32), origin: (f32, f32), x: f32, y: f32, cols: u32, rows: u32) -> (u32, u32, Side) {
    let (cw, ch) = cell;
    let (col, row) = mouse::cell_at((x - origin.0) as f64, (y - origin.1) as f64, cw as f64, ch as f64, cols, rows);
    let side = if (x - origin.0) - col as f32 * cw > cw / 2.0 { Side::Right } else { Side::Left };
    (col, row, side)
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

/// What a face key is built from, as the profile stores it, plus the window's scale and the
/// shared offset (`face_key_for`'s inputs).
#[derive(Clone, Debug, PartialEq)]
pub struct RawFont {
    normal: String,
    alt: Option<String>,
    spacing: (u64, u64),
    scale: u32,
    offset: u32,
    bold: bool,
    italic: bool,
    ligatures: (bool, bool),
    anti_alias: (bool, bool),
    thin: i64,
}

impl RawFont {
    fn of(p: &Profile, scale: f32, offset: f32) -> RawFont {
        RawFont {
            normal: p.string("Normal Font"),
            alt: p.bool("Use Non-ASCII Font").then(|| p.string("Non Ascii Font")),
            spacing: (p.float("Horizontal Spacing").to_bits(), p.float("Vertical Spacing").to_bits()),
            scale: scale.to_bits(),
            offset: offset.to_bits(),
            bold: p.bool("Use Bold Font"),
            italic: p.bool("Use Italic Font"),
            ligatures: (p.bool("ASCII Ligatures"), p.bool("Non-ASCII Ligatures")),
            anti_alias: (p.bool("ASCII Anti Aliased"), p.bool("Non-ASCII Anti Aliased")),
            thin: p.int("Thin Strokes"),
        }
    }
}

impl PaneFaces {
    /// The key cached for `pane` when its raw values are still `raw`.
    fn cached(&self, pane: PaneId, raw: &RawFont) -> Option<FaceKey> {
        self.raw.get(&pane).filter(|(r, _)| r == raw).map(|(_, k)| k.clone())
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
        face_key_for(p, self.scale(), self.font_offset, &|name| renderer.map_or_else(|| name.to_string(), |r| r.resolve_family(name)))
    }

    /// The face key of the profile `pane` runs (the default profile when it is gone).
    /// Returns the raw values with it, and reuses the cached key while they are unchanged.
    fn pane_face_key(&self, id: PaneId, pane: &Pane) -> (RawFont, FaceKey) {
        let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
        let raw = RawFont::of(p, self.scale(), self.font_offset);
        let key = self.pane_faces.cached(id, &raw).unwrap_or_else(|| self.face_key(p));
        (raw, key)
    }

    /// Measures the face of every pane of this window, at the window's scale. Called at the top
    /// of every frame, after a profile edit, and before any code sizes a new pane, so that
    /// `pane_cell` (which only reads) is never stale; a face is measured once per key.
    pub(super) fn ensure_faces(&mut self) {
        if self.gpu.is_none() {
            return;
        }
        let scale = self.scale();
        let built: Vec<(PaneId, RawFont, FaceKey)> = self
            .tabs
            .iter()
            .flat_map(|t| t.layout.panes())
            .filter_map(|id| {
                let (raw, key) = self.pane_face_key(id, self.panes.get(&id)?);
                Some((id, raw, key))
            })
            .collect();
        let mut keys = Vec::with_capacity(built.len());
        for (id, raw, key) in built {
            self.pane_faces.raw.insert(id, (raw, key.clone()));
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

    fn tab_focused_cell(&self, pane: PaneId) -> Option<(f32, f32)> {
        let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&pane))?;
        self.pane_faces.cell(tab.focused)
    }

    /// The columns and rows a pane's rectangle holds at the pane's own cell.
    pub(super) fn grid_size_for(&self, pane: PaneId, rect: Rect) -> (u16, u16) {
        grid_for((rect.w, rect.h), self.pane_cell(pane), self.margins())
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

    /// The underline numbers and scale of a pane's face for `scene::pane_with`.
    pub(super) fn draw_prefs_for(&self, face: u16) -> scene::DrawPrefs {
        let metrics = self.gpu.as_ref().map(|g| g.renderer.face_underline(face)).unwrap_or(scene::DrawPrefs::default().metrics);
        let mut prefs = scene::DrawPrefs::from_globals(&self.globals, metrics, self.scale());
        if let Some(g) = self.gpu.as_ref() {
            (prefs.ligatures, prefs.alt_level) = g.renderer.face_ligatures(face, prefs.ligatures.prefer_speed);
            prefs.masks = g.renderer.face_masks(face, prefs.masks.retina, prefs.masks.force_aa_on_retina);
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
        let k = face_key_for(&p, 1.5, 0.0, &same);
        assert_eq!(k.family, "Cascadia Mono");
        assert_eq!(k.alt_family.as_deref(), Some("MS Gothic"));
        assert_eq!(k.px(), 28.0);
        assert_eq!((k.h_bits, k.v_bits), (1.1f32.to_bits(), 1.0f32.to_bits()));
        let off = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Non-ASCII Font", false.into())]);
        assert_eq!(face_key_for(&off, 1.0, 0.0, &same).alt_family, None);
        // The shared font offset grows every face, kept inside the drawn range.
        assert_eq!(face_key_for(&off, 1.0, 2.0, &same).px(), crate::render::quantize_px(16.0 * 4.0 / 3.0));
        assert_eq!(face_key_for(&off, 1.0, 500.0, &same).px(), crate::render::quantize_px(128.0 * 4.0 / 3.0));
        // Names go through the resolver.
        let up = |s: &str| s.to_uppercase();
        assert_eq!(face_key_for(&p, 1.0, 0.0, &up).family, "CASCADIA MONO");
    }

    #[test]
    fn bold_and_italic_switches_make_a_new_key() {
        let same = |s: &str| s.to_string();
        let on = profile(&[("Normal Font", "Cascadia Mono 14".into())]);
        let no_bold = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Bold Font", false.into())]);
        let no_italic = profile(&[("Normal Font", "Cascadia Mono 14".into()), ("Use Italic Font", false.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, 0.0, &same);
        assert!(key(&on).use_bold && key(&on).use_italic, "iTerm2's defaults are on");
        assert!(!key(&no_bold).use_bold && key(&no_bold).use_italic);
        assert!(key(&no_italic).use_bold && !key(&no_italic).use_italic);
        assert_ne!(key(&on), key(&no_bold));
        assert_ne!(key(&on), key(&no_italic));
        // The per-pane cache is keyed on RawFont: a switch change must change it, or a stale key is served.
        assert_ne!(RawFont::of(&on, 1.0, 0.0), RawFont::of(&no_bold, 1.0, 0.0));
        assert_ne!(RawFont::of(&on, 1.0, 0.0), RawFont::of(&no_italic, 1.0, 0.0));
    }

    #[test]
    fn ligature_switches_make_a_new_key() {
        let same = |s: &str| s.to_string();
        let base = [("Normal Font", toml::Value::from("Cascadia Code 14"))];
        let off = profile(&base);
        let ascii = profile(&[base[0].clone(), ("ASCII Ligatures", true.into())]);
        let non_ascii = profile(&[base[0].clone(), ("Non-ASCII Ligatures", true.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, 0.0, &same);
        assert!(!key(&off).ascii_ligatures && !key(&off).non_ascii_ligatures, "iTerm2's defaults are off");
        assert!(key(&ascii).ascii_ligatures && !key(&ascii).non_ascii_ligatures);
        assert!(!key(&non_ascii).ascii_ligatures && key(&non_ascii).non_ascii_ligatures);
        assert_ne!(key(&off), key(&ascii));
        assert_ne!(key(&off), key(&non_ascii));
        assert_ne!(RawFont::of(&off, 1.0, 0.0), RawFont::of(&ascii, 1.0, 0.0));
        assert_ne!(RawFont::of(&off, 1.0, 0.0), RawFont::of(&non_ascii, 1.0, 0.0));
    }

    #[test]
    fn anti_alias_switches_are_part_of_the_face() {
        let same = |s: &str| s.to_string();
        let base = [("Normal Font", toml::Value::from("Cascadia Mono 14"))];
        let stock = profile(&base);
        let ascii_off = profile(&[base[0].clone(), ("ASCII Anti Aliased", false.into())]);
        let non_ascii_off = profile(&[base[0].clone(), ("Non-ASCII Anti Aliased", false.into())]);
        let thin_never = profile(&[base[0].clone(), ("Thin Strokes", 0.into())]);
        let key = |p: &Profile| face_key_for(p, 1.0, 0.0, &same);
        // The stock profile is anti-aliased with Thin Strokes at Retina only.
        assert_eq!((key(&stock).ascii_aa, key(&stock).non_ascii_aa, key(&stock).thin), (true, true, 4));
        assert!(!key(&ascii_off).ascii_aa && key(&ascii_off).non_ascii_aa);
        assert!(key(&non_ascii_off).ascii_aa && !key(&non_ascii_off).non_ascii_aa);
        assert_eq!(key(&thin_never).thin, 0);
        for other in [&ascii_off, &non_ascii_off, &thin_never] {
            assert_ne!(key(&stock), key(other));
            // The per-pane key cache is keyed on RawFont: a change must change it, or a stale key is served.
            assert_ne!(RawFont::of(&stock, 1.0, 0.0), RawFont::of(other, 1.0, 0.0));
        }
    }

    #[test]
    fn pane_cell_is_warm_before_the_first_frame() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
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

    #[test]
    fn pane_cells_follow_faces() {
        // 13l's default margins at scale 1: 5 points a side and 2 above and below.
        let margins = (margins::px(5, 1.0), margins::px(2, 1.0));
        assert_eq!(grid_for((1000.0, 500.0), (10.0, 20.0), margins), (99, 24));
        assert_eq!(grid_for((1000.0, 500.0), (8.0, 16.0), margins), (123, 31));
    }

    #[test]
    fn pointer_cells_follow_the_pane() {
        let origin = (100.0, 40.0);
        // x = 45 into the text: column 4 where cells are 10 wide, column 5 where they are 8.
        assert_eq!(pointer_cell((10.0, 20.0), origin, 145.0, 41.0, 80, 24).0, 4);
        assert_eq!(pointer_cell((8.0, 16.0), origin, 145.0, 41.0, 80, 24).0, 5);
        // Rows, and the half of the cell.
        let (_, row, side) = pointer_cell((10.0, 20.0), origin, 147.0, 100.0, 80, 24);
        assert_eq!((row, side), (3, Side::Right));
        assert_eq!(pointer_cell((10.0, 20.0), origin, 143.0, 100.0, 80, 24).2, Side::Left);
    }
}
