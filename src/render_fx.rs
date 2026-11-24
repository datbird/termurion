//! Render plugins (pure part): shaders that run over the finished frame. A plugin is a folder
//! with `plugin.toml` and Ghostty-style GLSL or WGSL. This module finds plugins, compiles their
//! shaders with naga, lays out Ghostty's uniform block and keeps its cursor and time rules.
//! No OS calls, so it is the same on every platform. Design:
//! `docs/superpowers/specs/2026-10-31-render-plugins-design.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::fx_profile::DoneRule;

use wgpu::naga;

/// Ghostty's uniform block and `iChannel0`, ready for naga.
pub const PREFIX: &str = include_str!("render_fx_prefix.glsl");

/// The profile key: plugin folder names, in the order they run.
pub const PROFILE_KEY: &str = "Render Plugins";

/// The user's own plugins, under the config folder.
pub const USER_FOLDER: &str = "RenderPlugins";

/// The switches every built-in effect shares, as (profile key, shader `const bool`, label,
/// default, group). Termurion sets them in each shader before it compiles it, so one profile
/// setting drives every effect. `None` as the constant is Termurion's own signals.
pub const SWITCHES: &[(&str, Option<&str>, &str, bool, &str)] = &[
    ("Effects Claude Enter", Some("ENTER_EFFECT"), "Enter surge (a prompt sent, a command run)", true, "Claude Code and other agents"),
    ("Effects Claude Done", Some("DONE_EFFECT"), "Done surge (an agent finished, a long command ended)", true, "Claude Code and other agents"),
    ("Effects Claude Line Color", Some("LINE_TINT"), "Input box lines (or a shaded box's edges) in the effect's color", true, "Claude Code and other agents"),
    ("Effects Claude Live Lines", Some("LINE_EFFECT"), "Input box lines (or a shaded box's edges) always alive", false, "Claude Code and other agents"),
    ("Effects Claude Prompt", Some("PROMPT_EFFECT"), "Prompt mark (\u{276f}, \u{203a}, >) look", true, "Claude Code and other agents"),
    ("Effects Signals", None, "Termurion sends the Enter and done signals itself (no hooks needed)", true, "Claude Code and other agents"),
    ("Effects Jumps", Some("JUMP_EFFECTS"), "Bolt when the cursor jumps along its row", true, "General"),
    ("Effects Idle", Some("IDLE_EFFECTS"), "Idle animation", true, "General"),
];

/// `text` with each `const bool` switch set, where the shader has it.
pub fn with_switches(text: &str, on: &[(&str, bool)]) -> String {
    on.iter().fold(text.to_string(), |t, (name, v)| set_setting(&t, name, if *v { "true" } else { "false" }).unwrap_or(t))
}

/// The user's own plugins: `RenderPlugins` beside the settings file.
pub fn user_dir() -> PathBuf {
    crate::globals::default_path().parent().map_or_else(|| PathBuf::from(USER_FOLDER), |d| d.join(USER_FOLDER))
}

/// How long `auto` animation runs after the last change.
pub const AUTO_WINDOW: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Animation {
    /// Animate for `AUTO_WINDOW` after a change.
    #[default]
    Auto,
    /// Animate every frame while the window is focused.
    Always,
    /// Draw only when the terminal draws.
    Off,
}

/// A capped frame is admitted this much before its gap is up, so a loop that wakes on the
/// vsync just short of the gap (16.667 ms against two 120 Hz vsyncs) still lands on it.
const FRAME_SLACK: Duration = Duration::from_millis(2);

/// How hard animated effects may drive the GPU ("Effects animation", General > GPU Renderer).
/// An `Always` effect redraws the whole window every frame, which keeps an older GPU busy with
/// nobody at the machine; Smooth, Balanced and Light cap the frame rate and pause it when idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pacing {
    /// As fast as the display refreshes, never paused (before 1.1.4).
    Full,
    /// Up to 30 frames a second; pauses after 2 minutes with no change.
    Balanced,
    /// Up to 15 frames a second; pauses after 30 seconds. For older hardware.
    Light,
    /// Up to 60 frames a second, even on a faster screen; pauses after 2 minutes with no change.
    #[default]
    Smooth,
}

impl Pacing {
    /// The `EffectsAnimation` setting's value: 0 Full, 1 Balanced, 2 Light, 3 Smooth.
    pub fn from_setting(v: i64) -> Pacing {
        match v {
            0 => Pacing::Full,
            1 => Pacing::Balanced,
            2 => Pacing::Light,
            _ => Pacing::Smooth,
        }
    }

    /// The shortest time between two animation frames; None draws as fast as the GPU allows.
    pub fn frame_gap(self) -> Option<Duration> {
        match self {
            Pacing::Full => None,
            Pacing::Balanced => Some(Duration::from_micros(33_333)),
            Pacing::Light => Some(Duration::from_micros(66_667)),
            Pacing::Smooth => Some(Duration::from_micros(16_667)),
        }
    }

    /// How long an `Always` animation runs on with no cursor change (a key, output, a signal)
    /// and no focus change. None never pauses.
    pub fn idle_pause(self) -> Option<Duration> {
        match self {
            Pacing::Full => None,
            Pacing::Balanced => Some(Duration::from_secs(120)),
            Pacing::Light => Some(Duration::from_secs(30)),
            Pacing::Smooth => Some(Duration::from_secs(120)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub shaders: Vec<String>,
    pub animation: Animation,
    /// The cursor color the effect was made for (cursor-fx presets set one).
    pub cursor_color: Option<String>,
    pub author: String,
    pub license: String,
    /// Where the effect can draw (`region` in plugin.toml); None means anywhere, the full pass.
    pub region: Option<Vec<RegionTerm>>,
}

/// One place an effect can light. The number is a pad in cell heights.
#[derive(Clone, Debug, PartialEq)]
pub enum RegionTerm {
    Full,
    Cursor(f32),
    CursorPath(f32),
    PromptMark(f32),
    InputBox(f32),
    /// The whole window for this long after the cursor changes.
    FullAfterChange(Duration),
    /// Looks outside the other terms that follow the terminal's picture and the cursor, never
    /// the time (Cyber tints sent prompts anywhere): a frame with a new picture runs the full
    /// pass. A cursor change is covered only by `FullAfterChange` beside it.
    StillLooks,
}

fn parse_region(v: &toml::Value) -> Result<Vec<RegionTerm>, String> {
    // A region that cannot be read is an error: dropping a term would shade too little.
    let items: Vec<&str> = match v {
        toml::Value::Array(a) => a.iter().map(|x| x.as_str().ok_or("plugin.toml: region terms must be strings")).collect::<Result<_, _>>()?,
        toml::Value::String(one) => vec![one.as_str()],
        _ => return Err("plugin.toml: region must be a list of terms".into()),
    };
    if items.is_empty() {
        return Err("plugin.toml: region lists no term".into());
    }
    items
        .into_iter()
        .map(|t| {
            let (name, num) = match t.split_once(':') {
                Some((n, x)) => (n.trim(), Some(x.trim())),
                None => (t.trim(), None),
            };
            let pad = || match num {
                None => Ok(0.0),
                Some(x) => x.parse::<f32>().ok().filter(|p| p.is_finite() && *p >= 0.0).ok_or(format!("plugin.toml: region term \"{t}\" needs a number")),
            };
            Ok(match name {
                "full" => RegionTerm::Full,
                "still_looks" => RegionTerm::StillLooks,
                "cursor" => RegionTerm::Cursor(pad()?),
                "cursor_path" => RegionTerm::CursorPath(pad()?),
                "prompt_mark" => RegionTerm::PromptMark(pad()?),
                "input_box" => RegionTerm::InputBox(pad()?),
                "full_after_change" => {
                    let ms = num.and_then(|x| x.parse::<u64>().ok()).ok_or(format!("plugin.toml: region term \"{t}\" needs a number"))?;
                    RegionTerm::FullAfterChange(Duration::from_millis(ms))
                }
                _ => return Err(format!("plugin.toml: unknown region term \"{t}\"")),
            })
        })
        .collect()
}

/// A rectangle in window pixels, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The terms of a chain of plugins: None (the full screen) if any plugin has no region or
/// asks for `Full`, else every term.
pub fn chain_region(manifests: &[&Manifest]) -> Option<Vec<RegionTerm>> {
    let mut all = Vec::new();
    for m in manifests {
        let terms = m.region.as_ref()?;
        if terms.contains(&RegionTerm::Full) {
            return None;
        }
        all.extend(terms.iter().cloned());
    }
    Some(all)
}

/// The pixels an effect can change this frame, or None for the whole screen. `cell_h` is the
/// unit the pads are in (`pad_unit`). The caller unites this with last frame's rects.
pub fn region_rects(terms: &[RegionTerm], u: &Uniforms, cell_h: f32, size: (u32, u32)) -> Option<Vec<PxRect>> {
    let (sw, sh) = (size.0 as f32, size.1 as f32);
    // (left, top, right, bottom) in float pixels, grown by pad cells
    let mut boxes: Vec<[f32; 4]> = Vec::new();
    // Cursors are stored y-down with y the bottom edge (see `Clock::frame`).
    let cursor = |c: [f32; 4]| [c[0], c[1] - c[3], c[0] + c[2], c[1]];
    let grow = |b: [f32; 4], p: f32| {
        let g = p * cell_h;
        [b[0] - g, b[1] - g, b[2] + g, b[3] + g]
    };
    for t in terms {
        match t {
            RegionTerm::Full => return None,
            // An animation-only frame keeps those looks from the frame before.
            RegionTerm::StillLooks => {}
            RegionTerm::FullAfterChange(d) => {
                if u.time - u.time_cursor_change < d.as_secs_f32() {
                    return None;
                }
            }
            RegionTerm::Cursor(p) => boxes.push(grow(cursor(u.current_cursor), *p)),
            RegionTerm::CursorPath(p) => {
                let (a, b) = (cursor(u.previous_cursor), cursor(u.current_cursor));
                boxes.push(grow([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])], *p));
            }
            RegionTerm::PromptMark(p) => {
                let m = u.prompt_mark;
                if m[2] > 0.5 {
                    boxes.push(grow([m[0], m[1], m[0] + m[3], m[1] + cell_h], *p));
                } else {
                    boxes.push(mark_search_box(u, cell_h));
                }
            }
            RegionTerm::InputBox(p) => {
                let (top, bottom, z) = (u.input_box[0], u.input_box[1], u.input_box[2]);
                if z < 0.5 {
                    boxes.push(line_search_band(u, cell_h, sw));
                }
                let (l, r) = if u.input_box_x[1] > 0.0 { (u.input_box_x[0], u.input_box_x[1]) } else { (0.0, sw) };
                if z > 1.5 {
                    boxes.push(grow([l, top, r, bottom], *p));
                } else if z > 0.5 {
                    // A line's value is its row's middle.
                    for y in [top, bottom] {
                        boxes.push(grow([l, y - cell_h / 2.0, r, y + cell_h / 2.0], *p));
                    }
                }
            }
        }
    }
    Some(round_and_merge(boxes, (sw, sh)))
}

/// Where the built-ins' linesLook tints box lines it finds itself (Termurion found no box, so
/// `iInputBox` is zero): any line across the window from 6.5 cursor heights above the cursor's
/// top to 2.6 below it (`p.y < cur.y - 2.6 * h || p.y > cur.y + 6.5 * h` in y-up pixels, `cur.y`
/// the top). A cell height more on each side, and `unit` (`pad_unit`, at least the cursor's
/// height) for `h`.
fn line_search_band(u: &Uniforms, unit: f32, width: f32) -> [f32; 4] {
    let top = u.current_cursor[1] - u.current_cursor[3];
    [0.0, top - 7.5 * unit, width, top + 3.6 * unit]
}

/// Where the built-ins' promptLook restyles a mark it finds itself (Termurion found none, so
/// `iPromptMark` is zero): nothing right of `9 * w + 0.4 * h` from the window's left edge; on
/// the cursor's row within 0.9 h of its middle, or on a wrapped prompt's first row up to 10.5 h
/// above the cursor's top. `w` is a cell (`cellWOf`, at most the cursor's width or half of
/// `unit`), `h` the cursor's height (at most `unit`); a cell more on each side.
fn mark_search_box(u: &Uniforms, unit: f32) -> [f32; 4] {
    let c = u.current_cursor;
    let top = c[1] - c[3];
    let w = c[2].max(0.5 * unit);
    [0.0, top - 11.5 * unit, 10.0 * w + 0.4 * unit, top + 2.4 * unit]
}

/// The unit region pads are in: the built-ins size their looks in the cursor's
/// `cellH = max(cursor height, 2 * cellWOf(cursor))`, where `cellWOf` halves a block cursor only
/// when it is less than 1.4 times as tall as wide (a wide character). Taken for the current and
/// the previous cursor (both with the current style, as the shaders do), and never below the
/// cell's own bound `max(cell height, 2 * cell width)` (a hidden or unset cursor).
pub fn pad_unit((cw, ch): (f32, f32), u: &Uniforms) -> f32 {
    let cell_h = |c: [f32; 4]| {
        let (w, h) = (c[2], c[3]);
        let one = if u.current_cursor_style == 0 && h < 1.4 * w { 0.5 * w } else { w };
        h.max(2.0 * one)
    };
    ch.max(2.0 * cw).max(cell_h(u.current_cursor)).max(cell_h(u.previous_cursor))
}

/// One frame's own region (`region_rects` with pads in `pad_unit`), or None for the full
/// pass. With Live Lines on and no input box found by Termurion the shaders look for box lines
/// themselves, anywhere around the cursor, and animate them: that frame is full.
pub fn own_region(terms: &[RegionTerm], u: &Uniforms, cell: (f32, f32), live_lines: bool, size: (u32, u32)) -> Option<Vec<PxRect>> {
    if live_lines && u.input_box[2] < 0.5 {
        return None;
    }
    region_rects(terms, u, pad_unit(cell, u), size)
}

/// The rects an animation frame shades: this frame's region united with last frame's, whose
/// light outside this frame's must go back to the plain picture. None (the full pass) when
/// either frame is full; an empty list when neither can draw anything.
pub fn frame_region(terms: &[RegionTerm], now: &Uniforms, last: &Uniforms, cell: (f32, f32), live_lines: bool, size: (u32, u32)) -> Option<Vec<PxRect>> {
    let a = own_region(terms, now, cell, live_lines, size)?;
    let b = own_region(terms, last, cell, live_lines, size)?;
    let boxes = a.iter().chain(&b).map(|r| [r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32]).collect();
    Some(round_and_merge(boxes, (size.0 as f32, size.1 as f32)))
}

/// The rects a frame may shade (`frame_rects`). None in either runs the full pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameRegion {
    /// This frame's own region: a frame drawn anew shades it over its new picture.
    pub own: Option<Vec<PxRect>>,
    /// `own` united with last frame's (`frame_region`): an animation-only frame shades it.
    pub with_last: Option<Vec<PxRect>>,
}

/// `own_region` and `frame_region` of one frame.
pub fn frame_rects(terms: &[RegionTerm], now: &Uniforms, last: &Uniforms, cell: (f32, f32), live_lines: bool, size: (u32, u32)) -> FrameRegion {
    FrameRegion { own: own_region(terms, now, cell, live_lines, size), with_last: frame_region(terms, now, last, cell, live_lines, size) }
}

/// What `region_rects` reads of one frame's uniforms, apart from the clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RegionInputs {
    current: [f32; 4],
    previous: [f32; 4],
    style: i32,
    mark: [f32; 4],
    input_box: [f32; 4],
    input_box_x: [f32; 4],
}

impl RegionInputs {
    fn of(u: &Uniforms) -> RegionInputs {
        RegionInputs { current: u.current_cursor, previous: u.previous_cursor, style: u.current_cursor_style, mark: u.prompt_mark, input_box: u.input_box, input_box_x: u.input_box_x }
    }
}

/// Per term: a `FullAfterChange` still inside its window. The clock reaches the rects only
/// through these, so an animation frame that moves just time repeats the answer.
fn recent_of(terms: &[RegionTerm], u: &Uniforms) -> Vec<bool> {
    terms.iter().map(|t| recent_term(t, u)).collect()
}

fn recent_term(t: &RegionTerm, u: &Uniforms) -> bool {
    matches!(t, RegionTerm::FullAfterChange(d) if u.time - u.time_cursor_change < d.as_secs_f32())
}

fn recent_same(terms: &[RegionTerm], u: &Uniforms, seen: &[bool]) -> bool {
    seen.len() == terms.len() && terms.iter().zip(seen).all(|(t, &s)| recent_term(t, u) == s)
}

/// What `RectsCache` keyed its answer on.
#[derive(Debug)]
struct RectsKey {
    terms: Vec<RegionTerm>,
    now: RegionInputs,
    last: RegionInputs,
    recent_now: Vec<bool>,
    recent_last: Vec<bool>,
    cell: (f32, f32),
    live_lines: bool,
    size: (u32, u32),
}

/// `frame_rects` with a memory: the same inputs give the last answer again, with no work and no
/// allocation. Most animation frames move only the clock, which the rects do not read.
#[derive(Debug, Default)]
pub struct RectsCache {
    key: Option<RectsKey>,
    region: std::sync::Arc<FrameRegion>,
    /// The answer for a chain with no region terms, made once.
    none: std::sync::Arc<FrameRegion>,
    computes: u64,
}

impl RectsCache {
    pub fn get(&mut self, terms: &[RegionTerm], now: &Uniforms, last: &Uniforms, cell: (f32, f32), live_lines: bool, size: (u32, u32)) -> std::sync::Arc<FrameRegion> {
        let (a, b) = (RegionInputs::of(now), RegionInputs::of(last));
        let same = self.key.as_ref().is_some_and(|k| {
            k.terms == terms && k.now == a && k.last == b && k.cell == cell && k.live_lines == live_lines && k.size == size && recent_same(terms, now, &k.recent_now) && recent_same(terms, last, &k.recent_last)
        });
        if !same {
            self.region = std::sync::Arc::new(frame_rects(terms, now, last, cell, live_lines, size));
            self.key = Some(RectsKey { terms: terms.to_vec(), now: a, last: b, recent_now: recent_of(terms, now), recent_last: recent_of(terms, last), cell, live_lines, size });
            self.computes += 1;
        }
        self.region.clone()
    }

    /// The answer when the chain has no region terms (the full pass): one shared value.
    pub fn none(&self) -> std::sync::Arc<FrameRegion> {
        self.none.clone()
    }

    /// How many times the rects were computed rather than reused.
    pub fn computes(&self) -> u64 {
        self.computes
    }
}

/// Rounds boxes outward to whole pixels, clamps them to the window, merges overlapping ones and
/// collapses more than 8 into their bounding box.
fn round_and_merge(boxes: Vec<[f32; 4]>, (sw, sh): (f32, f32)) -> Vec<PxRect> {
    let mut rects: Vec<[f32; 4]> = boxes
        .into_iter()
        .map(|b| [b[0].floor().clamp(0.0, sw), b[1].floor().clamp(0.0, sh), b[2].ceil().clamp(0.0, sw), b[3].ceil().clamp(0.0, sh)])
        .filter(|b| b[2] > b[0] && b[3] > b[1])
        .collect();
    'again: loop {
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                if a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3] {
                    rects[i] = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                    rects.remove(j);
                    continue 'again;
                }
            }
        }
        break;
    }
    if rects.len() > 8 {
        let all = rects.iter().fold([f32::MAX, f32::MAX, 0.0, 0.0], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
        rects = vec![all];
    }
    rects.into_iter().map(|b| PxRect { x: b[0] as u32, y: b[1] as u32, w: (b[2] - b[0]) as u32, h: (b[3] - b[1]) as u32 }).collect()
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let v: toml::Table = text.parse().map_err(|e| format!("plugin.toml: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let shaders: Vec<String> = match v.get("shaders") {
        // A shader name that cannot be read is an error: dropping it would load a plugin missing a pass.
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or("plugin.toml: `shaders` entries must be file names"))
            .collect::<Result<_, _>>()?,
        Some(toml::Value::String(one)) => vec![one.clone()],
        Some(_) => return Err("plugin.toml: `shaders` must be a list of file names".into()),
        None => vec![],
    };
    if shaders.is_empty() {
        return Err("plugin.toml: `shaders` lists no shader file".into());
    }
    let animation = match s("animation").as_str() {
        "" | "auto" => Animation::Auto,
        "always" => Animation::Always,
        "off" => Animation::Off,
        other => return Err(format!("plugin.toml: animation must be auto, always or off, not {other:?}")),
    };
    let cursor_color = Some(s("cursor_color")).filter(|c| !c.is_empty());
    let region = v.get("region").map(parse_region).transpose()?;
    Ok(Manifest { name: s("name"), description: s("description"), shaders, animation, cursor_color, author: s("author"), license: s("license"), region })
}

/// Where a plugin's files are.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Compiled into Termurion: (file, text) pairs.
    Builtin(&'static [(&'static str, &'static str)]),
    /// A folder on disk.
    Folder(PathBuf),
}

/// A plugin. `id` is its folder name, the name a profile lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Plugin {
    pub id: String,
    pub manifest: Manifest,
    pub origin: Origin,
}

impl Plugin {
    /// The name to show: the manifest's, else the folder's.
    pub fn title(&self) -> &str {
        if self.manifest.name.is_empty() { &self.id } else { &self.manifest.name }
    }

    pub fn builtin(&self) -> bool {
        matches!(self.origin, Origin::Builtin(_))
    }

    /// The text of one of its shader files.
    pub fn shader_text(&self, file: &str) -> Result<String, String> {
        match &self.origin {
            Origin::Builtin(files) => files.iter().find(|(n, _)| *n == file).map(|(_, t)| t.to_string()).ok_or_else(|| format!("{}: no shader {file}", self.id)),
            Origin::Folder(dir) => {
                // A manifest names files inside its own folder only.
                if file.contains(['/', '\\']) || file.contains("..") {
                    return Err(format!("{}: {file:?} is not a file in the plugin folder", self.id));
                }
                std::fs::read_to_string(dir.join(file)).map_err(|e| format!("{}: {file}: {e}", self.id))
            }
        }
    }

    /// Every shader, compiled, in the order the manifest lists them.
    pub fn compile(&self) -> Result<Vec<Compiled>, String> {
        self.compile_with(&[])
    }

    /// `compile` with the shared switches set (`with_switches`).
    pub fn compile_with(&self, switches: &[(&str, bool)]) -> Result<Vec<Compiled>, String> {
        self.manifest.shaders.iter().map(|f| self.shader_text(f).and_then(|text| compile_named(f, &with_switches(&text, switches)).map_err(|e| format!("{} ({f}):\n{e}", self.title())))).collect()
    }
}

/// Every plugin: the built-in ones, then the user's folder, sorted by title. A user folder with
/// a built-in one's name replaces it. A folder whose manifest is broken comes back as an error
/// beside its name, so the UI can say why it is missing.
pub fn discover(user: Option<&Path>) -> (Vec<Plugin>, Vec<(String, String)>) {
    discover_in(crate::render_fx_builtin::BUILTIN, user)
}

fn discover_in(builtin: &[crate::render_fx_builtin::Builtin], user: Option<&Path>) -> (Vec<Plugin>, Vec<(String, String)>) {
    let mut found: HashMap<String, Plugin> = HashMap::new();
    let mut errors = Vec::new();
    for (id, manifest, files) in builtin {
        match parse_manifest(manifest) {
            Ok(manifest) => {
                found.insert(id.to_string(), Plugin { id: id.to_string(), manifest, origin: Origin::Builtin(files) });
            }
            Err(e) => errors.push((id.to_string(), e)),
        }
    }
    if let Some(Ok(entries)) = user.map(std::fs::read_dir) {
        for e in entries.flatten() {
            let dir = e.path();
            let manifest_path = dir.join("plugin.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let id = e.file_name().to_string_lossy().to_string();
            match std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string()).and_then(|t| parse_manifest(&t)) {
                Ok(mut manifest) => {
                    // Only the bundled effects are proven to draw nothing outside their region
                    // on a frame with a new picture (the region pixel tests). A user's region
                    // keeps the 1.1.10 meaning: such a frame runs the full pass.
                    if let Some(terms) = manifest.region.as_mut().filter(|t| !t.contains(&RegionTerm::StillLooks)) {
                        terms.push(RegionTerm::StillLooks);
                    }
                    found.insert(id.clone(), Plugin { id, manifest, origin: Origin::Folder(dir) });
                }
                Err(err) => errors.push((id, err)),
            }
        }
    }
    let mut list: Vec<Plugin> = found.into_values().collect();
    list.sort_by(|a, b| a.title().to_lowercase().cmp(&b.title().to_lowercase()));
    (list, errors)
}

/// The GLSL naga compiles: the prefix, the body (with `patch_input_box`) with its own line
/// numbers, then `main()`. `main()` goes last because naga rejects a call to a function defined
/// later.
pub fn wrap_glsl(body: &str) -> String {
    let body = patch_input_box(body);
    format!("{PREFIX}\n#line 1\n{body}\nvoid main() {{ mainImage(_fragColor, gl_FragCoord.xy); }}\n")
}

/// Termurion's patch for cursor-fx-style shaders (each line stays one line, so error line numbers
/// still match the file): they look for Claude Code's input box lines by scanning pixels near the
/// cursor, at most 4 rows up, so a box that grows with a long prompt loses its top line. When
/// Termurion found a box in the grid (`iInputBox`), it patches:
///
/// - Box lines: `boxLineY`, `clLineNear` and `boxLines` answer with the box's top and bottom line
///   (a band's edges, only between its left and right edge, `iInputBoxX`), and the "how far above
///   the cursor" bounds reach the top line.
/// - The below bound: "a box line just below the cursor" and "a box around the cursor" checks
///   count a band as a box, and linesLook's row bounds follow a band's edges.
/// - lineTint for a band (`iInputBox.z` 2): it has no line ink, so linesLook's quick exits are
///   skipped and LINE_TINT draws a thin line on each edge. Hologram's panel and Cyber's rain keep
///   out of a band's text.
/// - The mark functions (`clMarkInk`, `clChevron`, `clMarkLeft`): their background tests also
///   take a band's shade as background.
/// - mainImage: reads that shade once per pixel, only in a shader that has a mark function.
///
/// Every patch keys on a built-in's own function or line, so a shader without them (a plain
/// Ghostty shader) is left as it is.
pub fn patch_input_box(body: &str) -> String {
    let up = |v: &str| format!("(Y_DOWN ? iResolution.y - {v} : {v})");
    let (top, bottom) = (up("iInputBox.x"), up("iInputBox.y"));
    let outside = |x: &str| format!("iInputBox.z > 0.5 && iInputBoxX.y > 0.0 && ({x} < iInputBoxX.x || {x} > iInputBoxX.y)");
    let nearest = |at: &str, x: &str| {
        // Inside a band, more than half a row from both edges, is its text: no line there.
        format!(" if (iInputBox.z > 0.5) {{ float tA = {top}, tB = {bottom}; if ({}) return -1.0; float tN = abs({at} - tA) < abs({at} - tB) ? tA : tB; if (iInputBox.z > 1.5 && ({at} - tA) * ({at} - tB) < 0.0 && abs({at} - tN) > 0.5 * iCurrentCursor.w) return -1.0; return abs({at} - tN) <= reach ? tN : -1.0; }}", outside(x))
    };
    // A band's line: halfway from the background beside the edge to the text color (a box
    // glyph's grey), tinted as lineTint tints a line, all in the screen texture's color space.
    let band_tint = format!(
        " if (iInputBox.z > 1.5) {{ vec3 tBg = screenAt(vec2(p.x, lineY + (lineY > 0.5 * ({top} + {bottom}) ? 4.0 : -4.0)) / res).rgb; bool tLin = distance(tBg, sRGBToLinear(iBackgroundColor)) < distance(tBg, iBackgroundColor); return termurionBandLine(p, col, lineY, mix(tBg, tLin ? sRGBToLinear(iForegroundColor) : iForegroundColor, 0.5), tLin ? sRGBToLinear(iCurrentCursorColor.rgb) : iCurrentCursorColor.rgb, LINE_COLOR); }}"
    );
    let rows_exit = "if (p.y < cur.y - 2.6 * h || p.y > cur.y + 6.5 * h || (p.y > cur.y - h && p.y < cur.y)) return col;";
    let band_rows = format!("if (iInputBox.z > 1.5 ? min(abs(p.y - {top}), abs(p.y - {bottom})) > 0.5 * h : (p.y < cur.y - 2.6 * h || p.y > cur.y + 6.5 * h || (p.y > cur.y - h && p.y < cur.y))) return col;");
    // The prompt mark's tests compare with the background. In a band the band's shade (read once
    // per pixel in mainImage, just inside its top left corner) counts as background too: Codex
    // leaves the mark's cell unshaded. At z <= 1.5 the helper ignores `band`, which is safe only
    // because every patched call passes the background there (termurionMarkBg is iBackgroundColor).
    let mark_bg = format!(" termurionMarkBg = iInputBox.z > 1.5 ? screenAt(vec2(iInputBoxX.x + 1.0, {top} + (Y_DOWN ? -2.0 : 2.0)) / iResolution.xy).rgb : iBackgroundColor;");
    let mark_dist = "float termurionMarkDist(vec3 s, vec3 band) { float d = colorDist(s, iBackgroundColor); return iInputBox.z > 1.5 ? min(d, distance(s, band)) : d; } ";
    let mark_fns = ["float clMarkInk(vec2 at, vec2 m, float w, float h, vec2 res)", "bool clChevron(float x, float w, vec4 cur, vec2 res)", "float clMarkLeft(vec4 cur, vec2 res)"];
    let (mut in_mark, mut have_mark_dist) = (false, false);
    let mut out = String::with_capacity(body.len() + 1024);
    for line in body.split_inclusive('\n') {
        let (text, end) = match line.strip_suffix('\n') {
            Some(t) => (t, "\n"),
            None => (line, ""),
        };
        let trimmed = text.trim_end();
        let mut text = text.to_string();
        if in_mark {
            // A function ends at the first line starting with `}` in column 0 (its body is
            // indented), a trailing comment after it or not.
            in_mark = !text.starts_with('}');
            text = text.replace("colorDist(", "termurionMarkDist(").replace("iBackgroundColor", "termurionMarkBg");
        } else if mark_fns.iter().any(|f| trimmed.starts_with(f)) && trimmed.ends_with('{') {
            in_mark = true;
            // The profile's own mark, when Rust found one (zero: the pixel checks below run).
            if trimmed.starts_with("bool clChevron(") {
                text.push_str(" if (iPromptMark.z > 0.5) return abs(x - iPromptMark.x) < 0.5 * w;");
            } else if trimmed.starts_with("float clMarkLeft(") {
                text.push_str(&format!(" if (iPromptMark.z > 0.5) return abs({} - cur.y) < 0.5 * cur.w ? iPromptMark.x : -1.0;", up("iPromptMark.y")));
            }
            // The helper goes on the line of whichever comes first, so line numbers stay.
            if !have_mark_dist {
                text.insert_str(0, mark_dist);
                have_mark_dist = true;
            }
        } else if trimmed.starts_with("vec2 clMarkAbove(vec4 cur, vec2 res)") && trimmed.ends_with('{') {
            // A wrapped prompt: the mark sits on a row above the cursor's.
            text.push_str(&format!(" if (iPromptMark.z > 0.5) {{ float mT = {}; return mT > cur.y + 0.5 * cur.w ? vec2(iPromptMark.x, mT) : vec2(-1.0); }}", up("iPromptMark.y")));
        } else if have_mark_dist && trimmed.starts_with("void mainImage(out vec4 fragColor, in vec2 fragCoord)") && trimmed.ends_with('{') {
            // Only after a mark function: screenAt and Y_DOWN are the built-ins' own, so a
            // plain Ghostty shader would not compile with this line.
            text.push_str(&mark_bg);
        } else if trimmed.starts_with("float boxLineY(vec2 p, vec2 center, vec4 cur, vec2 res, float reach)") && trimmed.ends_with('{') {
            text.push_str(&nearest("p.y", "p.x"));
        } else if trimmed.starts_with("float clLineNear(float y0, float reach, vec2 res)") && trimmed.ends_with('{') {
            // No x here, so it gates on the pixel being drawn (gl_FragCoord.x), not the column
            // the caller asks about.
            text.push_str(&nearest("y0", "gl_FragCoord.x"));
        } else if trimmed.starts_with("vec2 boxLines(vec4 cur, vec2 center, float cellW, float cellH, vec2 res)") && trimmed.ends_with('{') {
            text.push_str(&format!(" if ({}) return vec2(-1.0); if (iInputBox.z > 0.5) return vec2({top}, {bottom});", outside("gl_FragCoord.x")));
        } else if trimmed.starts_with("vec3 lineTint(vec2 p, vec3 col, float lineY, vec2 res)") && trimmed.ends_with('{') {
            text.push_str(&band_tint);
        } else if trimmed.trim_start() == rows_exit {
            text = text.replace(rows_exit, &band_rows);
        } else {
            // linesLook's quick exits for pixels that are no line ink: a band's edges have none.
            text = text.replace("if (!look && ", "if (!look && iInputBox.z < 1.5 && ").replace("if (!LINE_EFFECT && colorDist", "if (!LINE_EFFECT && iInputBox.z < 1.5 && colorDist");
            // A band's middle holds the typed text: Hologram's panel and Cyber's rain stay out of it.
            text = text.replace("bool inBox = p.y >= y0 && p.y <= y1;", "bool inBox = iInputBox.z < 1.5 && p.y >= y0 && p.y <= y1;").replace("yBot = bot + 3.0;", "yBot = iInputBox.z > 1.5 ? top + 3.0 : bot + 3.0;");
        }
        // "A box line just below the cursor" and "a box around the cursor": a band is a box
        // wherever its edges are.
        let text = below_check_re().replace_all(&text, "(iInputBox.z > 1.5 ? 0.0 : $0)");
        let text = around_check_re().replace_all(&text, "(iInputBox.z > 1.5 ? 0.0 : $0)");
        let line = bounds_re().replace_all(&text, |c: &regex::Captures| {
            let unit = if c[2].contains("cur.w") { "cur.w" } else { "cellH" };
            let m = &c[2];
            if let Some(name) = m.strip_prefix("(4.0 + ").and_then(|r| r.strip_suffix(") * cellH")) {
                format!("{}(termurionAbove(cellH) + {name} * cellH)", &c[1])
            } else if m == "4.5 * cellH" {
                format!("{}(termurionAbove(cellH) + 0.5 * cellH)", &c[1])
            } else {
                format!("{}termurionAbove({unit})", &c[1])
            }
        });
        // A "rows above the cursor" limit written as a sum (Embers' Enter: 4 rows plus 1.5 for
        // its rising embers) reaches the box top the same way, keeping what it adds past 4 rows.
        let line = above_re().replace_all(&line, |c: &regex::Captures| {
            let rows: f32 = c[2].parse().unwrap_or(0.0);
            if rows < 4.0 { c[0].to_string() } else { format!("{}termurionAbove(cellH) + {:.1} * cellH", &c[1], rows - 4.0) }
        });
        // "Rows below the cursor" limits reach a band's bottom edge, however far down it is.
        out.push_str(&below_re().replace_all(&line, |c: &regex::Captures| match (c.get(2), c.get(3), c.get(4)) {
            (Some(rows), _, _) => format!("{}termurionBelow(cellH, {})", &c[1], rows.as_str()),
            (_, Some(rows), Some(name)) => format!("{}(termurionBelow(cellH, {}) + {} * cellH)", &c[1], rows.as_str(), name.as_str()),
            _ => c[0].to_string(),
        }));
        out.push_str(end);
    }
    out
}

fn bounds_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(center\.y\s*[<>]\s*)(\(4\.0 \+ [A-Z_]+\) \* cellH|4\.5 \* cellH|4\.0 \* cellH|4\.0 \* cur\.w)").unwrap())
}

fn above_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(p\.y\s*>\s*center\.y\s*\+\s*)(\d+(?:\.\d+)?) \* cellH").unwrap())
}

fn below_check_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"clLineNear\(cur\.y - 1\.7 \* (h|cur\.w), 0\.7 \* (h|cur\.w), res\)").unwrap())
}

fn around_check_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"boxLineY\(center [+-] vec2\(0\.0, 0\.5 \* cellH\), center, cur, res, 0\.9 \* cellH\)").unwrap())
}

fn below_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(center\.y - p\.y\s*[<>]\s*|p\.y\s*<\s*center\.y\s*-\s*)(?:(\d+\.\d+) \* cellH|\((\d+\.\d+) \+ ([A-Z_]+)\) \* cellH)").unwrap())
}

/// A shader compiled and validated.
pub struct Compiled {
    pub module: naga::Module,
    pub info: naga::valid::ModuleInfo,
    /// The fragment entry point's name.
    pub entry: String,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiled").field("entry", &self.entry).finish()
    }
}

/// Lines of the wrapped GLSL before the body's line 1.
fn prefix_lines() -> u32 {
    wrap_glsl("").split("#line 1\n").next().unwrap_or("").matches('\n').count() as u32 + 1
}

/// "line N: message" with N counted in the person's file, for each span naga reports.
fn located(src: &str, first_line: u32, spans: impl Iterator<Item = (naga::Span, String)>, fallback: String) -> String {
    let lines: Vec<String> = spans
        .map(|(span, msg)| {
            let at = span.location(src).line_number;
            if at > first_line { format!("line {}: {msg}", at - first_line) } else { msg }
        })
        .collect();
    if lines.is_empty() { fallback } else { lines.join("\n") }
}

fn validate(module: naga::Module, src: &str, entry: &str, first_line: u32) -> Result<Compiled, String> {
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| located(src, first_line, e.spans().map(|(s, m)| (*s, format!("{m}: {}", e.as_inner()))), format!("{}", e.as_inner())))?;
    if !module.entry_points.iter().any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment) {
        return Err(format!("no fragment entry point named {entry}"));
    }
    Ok(Compiled { module, info, entry: entry.to_string() })
}

/// Compiles a Ghostty/Shadertoy GLSL body (`mainImage`).
pub fn compile_glsl(body: &str) -> Result<Compiled, String> {
    let src = wrap_glsl(body);
    let mut fe = naga::front::glsl::Frontend::default();
    let first = prefix_lines();
    let module = fe
        .parse(&naga::front::glsl::Options::from(naga::ShaderStage::Fragment), &src)
        .map_err(|e| located(&src, first, e.errors.iter().map(|x| (x.meta, x.kind.to_string())), e.emit_to_string(&src)))?;
    validate(module, &src, "main", first)
}

/// Compiles a WGSL shader with a `fs_main` fragment entry point and the same bindings.
pub fn compile_wgsl(src: &str) -> Result<Compiled, String> {
    let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
    validate(module, src, "fs_main", 0)
}

/// Compiles a shader by its file name's extension: `.wgsl` is WGSL, anything else GLSL.
pub fn compile_named(file: &str, text: &str) -> Result<Compiled, String> {
    if file.to_ascii_lowercase().ends_with(".wgsl") { compile_wgsl(text) } else { compile_glsl(text) }
}

/// Where each uniform lives in the buffer, read from naga's IR (the GLSL front end lays the
/// block out by std140), so the packing always matches the declaration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub size: u32,
    /// Member name -> (offset, array stride; 0 for a plain value).
    pub fields: HashMap<String, (u32, u32)>,
}

/// The members an animation frame moves: the clock, the frame count, the cursor and focus.
const CLOCK_FIELDS: &[(&str, u32)] = &[
    ("iTime", 4),
    ("iTimeDelta", 4),
    ("iFrame", 4),
    ("iChannelTime", 16),
    ("iCurrentCursor", 16),
    ("iPreviousCursor", 16),
    ("iCurrentCursorColor", 16),
    ("iPreviousCursorColor", 16),
    ("iCurrentCursorStyle", 4),
    ("iPreviousCursorStyle", 4),
    ("iCursorVisible", 4),
    ("iTimeCursorChange", 4),
    ("iTimeFocus", 4),
    ("iFocus", 4),
];

impl Layout {
    /// The byte range holding iTime, iTimeDelta, iFrame, iChannelTime and the cursor and focus
    /// members (4-byte aligned): all an animation frame changes, so a frame writes just this.
    /// Empty when the block has none of them.
    pub fn clock_span(&self) -> std::ops::Range<usize> {
        let mut span: Option<(u32, u32)> = None;
        for (name, len) in CLOCK_FIELDS {
            if let Some(&(off, _)) = self.fields.get(*name) {
                let (a, b) = span.unwrap_or((off, off + len));
                span = Some((a.min(off), b.max(off + len)));
            }
        }
        match span {
            Some((a, b)) => (a & !3) as usize..((b + 3) & !3).min(self.size) as usize,
            None => 0..0,
        }
    }
}

/// The layout of the uniform block at group 0, binding 1. None when the shader has none (a
/// shader may ignore every uniform).
pub fn uniform_layout(module: &naga::Module) -> Option<Layout> {
    let var = module.global_variables.iter().map(|(_, v)| v).find(|v| {
        v.space == naga::AddressSpace::Uniform && v.binding.as_ref().is_some_and(|b| b.group == 0 && b.binding == 1)
    })?;
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).ok()?;
    let size = layouter[var.ty].size;
    let mut fields = HashMap::new();
    if let naga::TypeInner::Struct { members, .. } = &module.types[var.ty].inner {
        for m in members {
            let stride = match module.types[m.ty].inner {
                naga::TypeInner::Array { stride, .. } => stride,
                _ => 0,
            };
            if let Some(name) = &m.name {
                fields.insert(name.clone(), (m.offset, stride));
            }
        }
    }
    Some(Layout { size, fields })
}

/// Ghostty's custom shader uniforms (`shadertoy.zig`), filled each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Uniforms {
    pub resolution: [f32; 3],
    pub time: f32,
    pub time_delta: f32,
    pub frame_rate: f32,
    pub frame: i32,
    pub date: [f32; 4],
    pub current_cursor: [f32; 4],
    pub previous_cursor: [f32; 4],
    pub current_cursor_color: [f32; 4],
    pub previous_cursor_color: [f32; 4],
    pub current_cursor_style: i32,
    pub previous_cursor_style: i32,
    pub cursor_visible: i32,
    pub time_cursor_change: f32,
    pub time_focus: f32,
    pub focus: i32,
    pub palette: Vec<[f32; 3]>,
    pub background: [f32; 3],
    pub foreground: [f32; 3],
    pub cursor_color: [f32; 3],
    pub cursor_text: [f32; 3],
    pub selection_foreground: [f32; 3],
    pub selection_background: [f32; 3],
    /// Termurion's `iInputBox`: the input box's top and bottom (pixels from the top), and z:
    /// 1 for a line box, 2 for a shaded band, 0 when there is none.
    pub input_box: [f32; 4],
    /// `iInputBoxX`: a band box's left and right edge in pixels (also a line box in a split pane), zeros for a full-width line box.
    pub input_box_x: [f32; 4],
    /// `iPromptMark`: the profile's prompt mark cell (left, top, set, cell width), zeros when none.
    pub prompt_mark: [f32; 4],
}

impl Default for Uniforms {
    fn default() -> Self {
        Uniforms {
            resolution: [0.0, 0.0, 1.0],
            time: 0.0,
            time_delta: 0.0,
            frame_rate: 60.0,
            frame: 0,
            date: [0.0; 4],
            current_cursor: [0.0; 4],
            previous_cursor: [0.0; 4],
            current_cursor_color: [0.0; 4],
            previous_cursor_color: [0.0; 4],
            current_cursor_style: 0,
            previous_cursor_style: 0,
            cursor_visible: 0,
            time_cursor_change: 0.0,
            time_focus: 0.0,
            // Ghostty assumes focus at first.
            focus: 1,
            palette: vec![[0.0; 3]; 256],
            background: [0.0; 3],
            foreground: [1.0; 3],
            cursor_color: [1.0; 3],
            cursor_text: [0.0; 3],
            selection_foreground: [0.0; 3],
            selection_background: [1.0; 3],
            input_box: [0.0; 4],
            input_box_x: [0.0; 4],
            prompt_mark: [0.0; 4],
        }
    }
}

fn put(buf: &mut [u8], at: u32, bytes: &[u8]) {
    let at = at as usize;
    if let Some(dst) = buf.get_mut(at..at + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

/// Writes `v` as little-endian floats at `at`; skipped whole when it does not fit.
fn put_floats(buf: &mut [u8], at: u32, v: &[f32]) {
    let at = at as usize;
    if let Some(dst) = buf.get_mut(at..at + 4 * v.len()) {
        for (c, f) in dst.chunks_exact_mut(4).zip(v) {
            c.copy_from_slice(&f.to_le_bytes());
        }
    }
}

fn put_i32(buf: &mut [u8], at: u32, v: i32) {
    put(buf, at, &v.to_le_bytes());
}

impl Uniforms {
    /// The uniform buffer's bytes for `layout`. Members the layout lacks are skipped.
    pub fn pack(&self, layout: &Layout) -> Vec<u8> {
        let mut buf = Vec::new();
        self.pack_into(layout, &mut buf);
        buf
    }

    /// `pack` into `out`, which is cleared and reused: a warm buffer allocates nothing.
    pub fn pack_into(&self, layout: &Layout, out: &mut Vec<u8>) {
        out.clear();
        out.resize(layout.size as usize, 0);
        let buf = out.as_mut_slice();
        let at = |name: &str| layout.fields.get(name).map(|&(off, _)| off);
        let floats = |buf: &mut [u8], name: &str, v: &[f32]| {
            if let Some(off) = at(name) {
                put_floats(buf, off, v);
            }
        };
        let int = |buf: &mut [u8], name: &str, v: i32| {
            if let Some(off) = at(name) {
                put_i32(buf, off, v);
            }
        };
        floats(buf, "iResolution", &self.resolution);
        floats(buf, "iTime", &[self.time]);
        floats(buf, "iTimeDelta", &[self.time_delta]);
        floats(buf, "iFrameRate", &[self.frame_rate]);
        int(buf, "iFrame", self.frame);
        floats(buf, "iMouse", &[0.0; 4]);
        floats(buf, "iDate", &self.date);
        floats(buf, "iSampleRate", &[44100.0]);
        floats(buf, "iCurrentCursor", &self.current_cursor);
        floats(buf, "iPreviousCursor", &self.previous_cursor);
        floats(buf, "iCurrentCursorColor", &self.current_cursor_color);
        floats(buf, "iPreviousCursorColor", &self.previous_cursor_color);
        int(buf, "iCurrentCursorStyle", self.current_cursor_style);
        int(buf, "iPreviousCursorStyle", self.previous_cursor_style);
        int(buf, "iCursorVisible", self.cursor_visible);
        floats(buf, "iTimeCursorChange", &[self.time_cursor_change]);
        floats(buf, "iTimeFocus", &[self.time_focus]);
        int(buf, "iFocus", self.focus);
        floats(buf, "iBackgroundColor", &self.background);
        floats(buf, "iForegroundColor", &self.foreground);
        floats(buf, "iCursorColor", &self.cursor_color);
        floats(buf, "iCursorText", &self.cursor_text);
        floats(buf, "iSelectionForegroundColor", &self.selection_foreground);
        floats(buf, "iSelectionBackgroundColor", &self.selection_background);
        floats(buf, "iInputBox", &self.input_box);
        floats(buf, "iInputBoxX", &self.input_box_x);
        floats(buf, "iPromptMark", &self.prompt_mark);
        // A vec4 here (see the prefix): channel 0 runs on the frame's clock.
        floats(buf, "iChannelTime", &[self.time, 0.0, 0.0, 0.0]);
        // Arrays: element i at offset + i * stride.
        if let Some(&(off, stride)) = layout.fields.get("iChannelResolution") {
            put_floats(buf, off, &[self.resolution[0], self.resolution[1], 1.0]);
            for i in 1..4 {
                put_floats(buf, off + i * stride, &[0.0; 3]);
            }
        }
        if let Some(&(off, stride)) = layout.fields.get("iPalette") {
            for (i, c) in self.palette.iter().take(256).enumerate() {
                put_floats(buf, off + i as u32 * stride, c);
            }
        }
    }
}

/// Ghostty's cursor shape numbers (`CURSORSTYLE_*` in the prefix).
pub mod style {
    pub const BLOCK: i32 = 0;
    pub const BLOCK_HOLLOW: i32 = 1;
    pub const BAR: i32 = 2;
    pub const UNDERLINE: i32 = 3;
}

/// The cursor as drawn this frame, in window pixels with a top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cursor {
    /// Left, top, width, height.
    pub rect: [f32; 4],
    /// sRGB 0-1, alpha last.
    pub color: [f32; 4],
    pub style: i32,
    pub visible: bool,
}

/// Keeps Ghostty's per-frame rules (`updateCustomShaderUniformsForFrame`): time since the first
/// frame, the frame count, the previous cursor when the cursor moves or changes color, and the
/// time focus was last gained. Also remembers the last change, for `auto` animation.
#[derive(Clone, Debug)]
pub struct Clock {
    first: Option<Instant>,
    last: Option<Instant>,
    focused: Option<bool>,
    last_change: Option<Instant>,
    /// When the next capped frame is due; it moves by whole gaps so the rate keeps its phase on
    /// a display whose period does not divide the gap.
    next_due: Option<Instant>,
    /// The frame gap of the current pacing (`set_pacing`), which `frame` advances `next_due` by.
    gap: Option<Duration>,
}

impl Default for Clock {
    fn default() -> Self {
        Clock { first: None, last: None, focused: None, last_change: None, next_due: None, gap: None }
    }
}

impl Clock {
    /// The pacing the next `frame` calls run under; set before them each frame, so a change of
    /// the setting applies at once.
    pub fn set_pacing(&mut self, pacing: Pacing) {
        self.gap = pacing.frame_gap();
    }

    /// Updates `u` for a frame drawn at `now`.
    pub fn frame(&mut self, u: &mut Uniforms, now: Instant, size: (u32, u32), cursor: Option<Cursor>, focused: bool) {
        let first = *self.first.get_or_insert(now);
        let last = self.last.unwrap_or(now);
        self.last = Some(now);
        if let Some(gap) = self.gap {
            // Only a frame the pacing admits moves the phase; a redraw for content or a resize
            // in between leaves it. A pause or stall (over a gap behind) restarts it, no burst.
            let due = match self.next_due {
                None => now + gap,
                Some(d) if now + FRAME_SLACK >= d => {
                    let n = d + gap;
                    if now.saturating_duration_since(n) > gap { now + gap } else { n }
                }
                Some(d) => d,
            };
            // A frame admitted early (within the slack) must not pull the phase early with it.
            self.next_due = Some(due.min(now + gap + FRAME_SLACK));
        }
        u.time = now.saturating_duration_since(first).as_secs_f32();
        u.time_delta = now.saturating_duration_since(last).as_secs_f32();
        u.frame += 1;
        u.resolution = [size.0 as f32, size.1 as f32, 1.0];
        if let Some(c) = cursor {
            // Ghostty's y-down form: x is the left edge, y the bottom (+Y) edge.
            let rect = [c.rect[0], c.rect[1] + c.rect[3], c.rect[2], c.rect[3]];
            if rect != u.current_cursor || c.color != u.current_cursor_color {
                u.previous_cursor = u.current_cursor;
                u.previous_cursor_color = u.current_cursor_color;
                u.current_cursor = rect;
                u.current_cursor_color = c.color;
                u.time_cursor_change = u.time;
                self.last_change = Some(now);
            }
            u.previous_cursor_style = u.current_cursor_style;
            u.current_cursor_style = c.style;
            u.cursor_visible = c.visible as i32;
        } else {
            // A hidden cursor keeps its last place, as in Ghostty.
            u.cursor_visible = 0;
        }
        u.focus = focused as i32;
        if self.focused != Some(focused) {
            if focused && self.focused.is_some() {
                u.time_focus = u.time;
            }
            self.focused = Some(focused);
            self.last_change = Some(now);
        }
    }

    /// When the cursor, its color or the focus last changed (the idle pause counts from it).
    pub fn last_change(&self) -> Option<Instant> {
        self.last_change
    }

    /// Something the effects react to happened (a key, output, a click).
    pub fn poke(&mut self, now: Instant) {
        self.last_change = Some(now);
    }

    /// When the animation's next frame is due under `pacing`, or None when it needs none: not
    /// animating, or an `Always` animation paused for idleness. A time at or before `now` means
    /// draw at once. The idle clock is the last change (`frame` sets it when the cursor moves or
    /// changes color, and on a focus change), else the first frame.
    pub fn next_frame(&self, animation: Animation, now: Instant, focused: bool, pacing: Pacing) -> Option<Instant> {
        if !self.wants_frame(animation, now, focused) {
            return None;
        }
        if animation == Animation::Always
            && let Some(limit) = pacing.idle_pause()
            && now.saturating_duration_since(self.last_change.or(self.first).unwrap_or(now)) >= limit
        {
            return None;
        }
        Some(match (pacing.frame_gap(), self.next_due) {
            (Some(_), Some(due)) => (due - FRAME_SLACK).max(now),
            _ => now,
        })
    }

    /// `next_frame` for a frame that was left out: with no blocking acquire or present to slow
    /// it, a frame due at once (Full pacing) would spin the loop, so the next one waits a frame
    /// gap (Full: 60 frames a second).
    pub fn next_after_still(&self, animation: Animation, now: Instant, focused: bool, pacing: Pacing) -> Option<Instant> {
        let wait = match pacing.frame_gap() {
            Some(gap) => gap - FRAME_SLACK,
            None => Duration::from_micros(16_667),
        };
        self.next_frame(animation, now, focused, pacing).map(|t| t.max(now + wait))
    }

    /// Whether another frame should be drawn now only for the animation.
    pub fn wants_frame(&self, animation: Animation, now: Instant, focused: bool) -> bool {
        match animation {
            Animation::Off => false,
            Animation::Always => focused,
            Animation::Auto => self.last_change.is_some_and(|t| now.saturating_duration_since(t) < AUTO_WINDOW),
        }
    }
}

/// How long a signal stays on (cursor-fx's SIGNAL_TIME, longer than the longest Enter effect).
pub const SIGNAL_TIME: Duration = Duration::from_millis(1900);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A prompt was sent or a command run.
    Enter,
    /// The answer arrived (Claude finished, a long command ended).
    Done,
}

/// The Enter and done signals cursor-fx's shaders read, made by Termurion itself so nothing has
/// to be installed on the host the program runs on: Enter when the person presses Enter, done
/// when output that kept coming after it goes quiet (Claude Code's spinner draws until the
/// answer is in) or a shell reports the command ended. They reach the shaders as cursor-fx's
/// invisible cursor color bits (`apply`), exactly as its hook script sets them over OSC 12.
#[derive(Clone, Debug, Default)]
pub struct Signals {
    on: Option<(Signal, Instant)>,
    parity: bool,
    /// Since Enter: (when, the last output after it).
    waiting: Option<(Instant, Option<Instant>)>,
    /// The done times of the profile active at Enter, fixed for this wait.
    quiet: Duration,
    busy: Duration,
    osc133: bool,
}

impl Signals {
    fn fire(&mut self, s: Signal, now: Instant) {
        self.parity = !self.parity;
        self.on = Some((s, now));
    }

    /// Enter was pressed; `done` is the active profile's rule, kept for this wait.
    pub fn enter(&mut self, now: Instant, done: &DoneRule) {
        self.fire(Signal::Enter, now);
        self.waiting = Some((now, None));
        (self.quiet, self.busy, self.osc133) = (done.quiet, done.busy, done.osc133);
    }

    /// The pane printed something.
    pub fn output(&mut self, now: Instant) {
        if let Some((_, last)) = self.waiting.as_mut() {
            *last = Some(now);
        }
    }

    /// A shell reported the command ended (OSC 133;D): done now, if it ran long enough.
    pub fn command_ended(&mut self, now: Instant) {
        if !self.osc133 {
            return;
        }
        if let Some((since, _)) = self.waiting.take()
            && now.saturating_duration_since(since) >= self.busy
        {
            self.fire(Signal::Done, now);
        }
    }

    /// Fires done once the output after Enter has gone quiet. True when it fired.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some((since, Some(last))) = self.waiting else { return false };
        if now.saturating_duration_since(last) < self.quiet {
            return false;
        }
        self.waiting = None;
        if last.saturating_duration_since(since) >= self.busy {
            self.fire(Signal::Done, now);
            return true;
        }
        false
    }

    /// The signal on now, if any.
    pub fn current(&self, now: Instant) -> Option<Signal> {
        self.on.filter(|(_, at)| now.saturating_duration_since(*at) < SIGNAL_TIME).map(|(s, _)| s)
    }

    /// `rgb` with the signal's bits (`fx-signal.sh`): Enter sets blue's lowest bit and clears
    /// green's, done the other way round; red's lowest bit flips with every signal, so a new one
    /// is always a color change. No signal leaves the color as it is.
    pub fn apply(&self, rgb: [u8; 3], now: Instant) -> [u8; 3] {
        let Some(s) = self.current(now) else { return rgb };
        let [r, g, b] = rgb;
        let r = (r & !1) | self.parity as u8;
        match s {
            Signal::Enter => [r, g & !1, b | 1],
            Signal::Done => [r, g | 1, b & !1],
        }
    }
}

/// A tunable value: a `const <type> NAME = value; // help` line near the top of a shader (the
/// form cursor-fx uses).
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub line: usize,
    pub ty: String,
    pub name: String,
    pub value: String,
    pub help: String,
}

fn setting_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^(const\s+(bool|float|int|vec2|vec3)\s+([A-Z][A-Z0-9_]*)\s*=\s*)([^;]+?)(\s*;\s*(?://\s*(.*))?)$").unwrap())
}

pub fn settings(src: &str) -> Vec<Setting> {
    src.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let c = setting_re().captures(l.trim_end())?;
            Some(Setting { line: i, ty: c[2].to_string(), name: c[3].to_string(), value: c[4].trim().to_string(), help: c.get(6).map(|m| m.as_str().trim().to_string()).unwrap_or_default() })
        })
        .collect()
}

/// `src` with setting `name` set to `value`; the rest of the file is kept byte for byte.
pub fn set_setting(src: &str, name: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.contains([';', '\n', '/']) {
        return Err(format!("{name}: {value:?} is not a value"));
    }
    let mut done = false;
    let out: Vec<String> = src
        .split_inclusive('\n')
        .map(|line| {
            let (body, end) = match line.strip_suffix('\n') {
                Some(b) => (b.strip_suffix('\r').unwrap_or(b), &line[b.strip_suffix('\r').unwrap_or(b).len()..]),
                None => (line, ""),
            };
            if !done
                && let Some(c) = setting_re().captures(body)
                && &c[3] == name
            {
                done = true;
                return format!("{}{}{}{}", &c[1], value, &c[5], end);
            }
            line.to_string()
        })
        .collect();
    if done { Ok(out.concat()) } else { Err(format!("no setting named {name}")) }
}

/// `#rrggbb` to sRGB 0-1.
pub fn hex_color(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([((n >> 16) & 255) as f32 / 255.0, ((n >> 8) & 255) as f32 / 255.0, (n & 255) as f32 / 255.0])
}

#[cfg(test)]
mod tests {
    use crate::test_tmp::TempDir;
    use super::*;

    #[test]
    fn manifest_reads_every_key() {
        let m = parse_manifest("name = \"Neon\"\nshaders = [\"a.glsl\", \"b.wgsl\"]\nanimation = \"always\"\ncursor_color = \"#ff00ff\"\nlicense = \"MIT\"").unwrap();
        assert_eq!(m.name, "Neon");
        assert_eq!(m.shaders, ["a.glsl", "b.wgsl"]);
        assert_eq!(m.animation, Animation::Always);
        assert_eq!(m.cursor_color.as_deref(), Some("#ff00ff"));
    }

    fn region_manifest(r: &str) -> Manifest {
        parse_manifest(&format!("shaders = \"a.glsl\"\nregion = {r}")).unwrap()
    }

    /// Uniforms after one frame with the cursor at (x, y, w, h), top-left.
    fn cursor_uniforms(rect: [f32; 4]) -> Uniforms {
        let mut u = Uniforms::default();
        let mut clock = Clock::default();
        let c = Cursor { rect, color: [1.0; 4], style: 0, visible: true };
        clock.frame(&mut u, Instant::now(), (800, 600), Some(c), true);
        u
    }

    fn covers(rects: &[PxRect], (x, y, w, h): (u32, u32, u32, u32)) -> bool {
        rects.iter().any(|r| r.x <= x && r.y <= y && r.x + r.w >= x + w && r.y + r.h >= y + h)
    }

    #[test]
    fn region_terms_parse() {
        let m = region_manifest("[\"cursor:2.5\", \"cursor_path:2.5\", \"prompt_mark:1\", \"input_box:1\", \"full_after_change:600\"]");
        assert_eq!(m.region.unwrap(), [RegionTerm::Cursor(2.5), RegionTerm::CursorPath(2.5), RegionTerm::PromptMark(1.0), RegionTerm::InputBox(1.0), RegionTerm::FullAfterChange(Duration::from_millis(600))]);
        assert_eq!(region_manifest("[\"full\"]").region.unwrap(), [RegionTerm::Full]);
        assert_eq!(region_manifest("[\"cursor\", \"still_looks\"]").region.unwrap(), [RegionTerm::Cursor(0.0), RegionTerm::StillLooks]);
        assert_eq!(region_manifest("[\"cursor\"]").region.unwrap(), [RegionTerm::Cursor(0.0)]);
    }

    #[test]
    fn a_bad_region_term_is_an_error() {
        assert_eq!(parse_manifest("shaders = \"a.glsl\"\nregion = [\"glow\"]").unwrap_err(), "plugin.toml: unknown region term \"glow\"");
        assert_eq!(parse_manifest("shaders = \"a.glsl\"\nregion = [\"full_after_change\"]").unwrap_err(), "plugin.toml: region term \"full_after_change\" needs a number");
    }

    #[test]
    fn a_region_that_cannot_be_read_is_an_error_never_an_empty_one() {
        let err = |r: &str| parse_manifest(&format!("shaders = \"a.glsl\"\nregion = {r}")).unwrap_err();
        assert_eq!(err("[\"cursor\", 5]"), "plugin.toml: region terms must be strings");
        assert_eq!(err("[5]"), "plugin.toml: region terms must be strings");
        assert_eq!(err("[\"cursor\", \"glow\"]"), "plugin.toml: unknown region term \"glow\"");
        assert_eq!(err("[]"), "plugin.toml: region lists no term");
        assert_eq!(err("\"\""), "plugin.toml: unknown region term \"\"");
        assert_eq!(err("7"), "plugin.toml: region must be a list of terms");
    }

    #[test]
    fn a_shader_name_that_cannot_be_read_is_an_error() {
        let err = |v: &str| parse_manifest(&format!("shaders = {v}")).unwrap_err();
        assert_eq!(err("[\"a.glsl\", 5]"), "plugin.toml: `shaders` entries must be file names");
        assert_eq!(err("7"), "plugin.toml: `shaders` must be a list of file names");
        assert_eq!(err("[]"), "plugin.toml: `shaders` lists no shader file");
        assert_eq!(parse_manifest("shaders = [\"a.glsl\", \"b.glsl\"]").unwrap().shaders, ["a.glsl", "b.glsl"]);
    }

    #[test]
    fn every_shipped_plugin_region_parses() {
        for (id, manifest, _) in crate::render_fx_builtin::BUILTIN {
            let m = parse_manifest(manifest).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(m.region.as_ref().is_none_or(|r| !r.is_empty()), "{id}");
        }
    }

    #[test]
    fn prompt_mark_rects() {
        let mut u = cursor_uniforms([200.0, 300.0, 10.0, 20.0]);
        let t = [RegionTerm::PromptMark(0.5)];
        // No mark found: where the shaders look for one, the first 10 cells of the cursor's row
        // and of the rows above it (the cursor's top is 300).
        assert_eq!(region_rects(&t, &u, 20.0, (800, 600)).unwrap(), [PxRect { x: 0, y: 70, w: 108, h: 278 }]);
        u.prompt_mark = [40.0, 100.0, 1.0, 10.0];
        assert_eq!(region_rects(&t, &u, 20.0, (800, 600)).unwrap(), [PxRect { x: 30, y: 90, w: 30, h: 40 }]);
    }

    #[test]
    fn an_unset_region_is_the_full_pass() {
        let m = parse_manifest("shaders = \"a.glsl\"").unwrap();
        assert_eq!(chain_region(&[&m]), None);
        assert_eq!(chain_region(&[]), Some(vec![]));
    }

    #[test]
    fn the_pad_follows_the_cell_height_and_scale() {
        let u = cursor_uniforms([100.0, 100.0, 10.0, 20.0]);
        let t = [RegionTerm::Cursor(1.0)];
        assert_eq!(region_rects(&t, &u, 30.0, (800, 600)).unwrap(), [PxRect { x: 70, y: 70, w: 70, h: 80 }]);
        // Scale 2: the cursor and the cell are twice as big, so the rect is too.
        let u = cursor_uniforms([200.0, 200.0, 20.0, 40.0]);
        assert_eq!(region_rects(&t, &u, 40.0, (1600, 1200)).unwrap(), [PxRect { x: 160, y: 160, w: 100, h: 120 }]);
    }

    #[test]
    fn fractions_round_outward() {
        assert_eq!(round_and_merge(vec![[10.4, 20.6, 30.2, 40.1]], (800.0, 600.0)), [PxRect { x: 10, y: 20, w: 21, h: 21 }]);
    }

    #[test]
    fn no_region_key_means_none() {
        assert_eq!(parse_manifest("shaders = \"a.glsl\"").unwrap().region, None);
    }

    #[test]
    fn cursor_region_covers_the_cursor() {
        let u = cursor_uniforms([100.0, 40.0, 10.0, 20.0]);
        let r = region_rects(&[RegionTerm::Cursor(1.0)], &u, 20.0, (800, 600)).unwrap();
        assert_eq!(r.len(), 1);
        assert!(covers(&r, (80, 20, 50, 60)), "{r:?}");
    }

    #[test]
    fn cursor_path_spans_both_cursors() {
        let mut u = cursor_uniforms([100.0, 40.0, 10.0, 20.0]);
        let mut clock = Clock::default();
        let c = Cursor { rect: [300.0, 200.0, 10.0, 20.0], color: [1.0; 4], style: 0, visible: true };
        clock.frame(&mut u, Instant::now(), (800, 600), Some(c), true);
        let r = region_rects(&[RegionTerm::CursorPath(0.0)], &u, 20.0, (800, 600)).unwrap();
        assert_eq!(r, [PxRect { x: 100, y: 40, w: 210, h: 180 }]);
    }

    #[test]
    fn input_box_lines_and_band() {
        let mut u = cursor_uniforms([200.0, 400.0, 10.0, 20.0]);
        // No box found: where the shaders look for lines across the window, 7.5 cells above
        // the cursor's top (400) to 3.6 below it.
        assert_eq!(region_rects(&[RegionTerm::InputBox(1.0)], &u, 20.0, (800, 600)).unwrap(), [PxRect { x: 0, y: 250, w: 800, h: 222 }]);
        u.input_box = [100.0, 300.0, 1.0, 0.0];
        let r = region_rects(&[RegionTerm::InputBox(0.0)], &u, 20.0, (800, 600)).unwrap();
        assert_eq!(r, [PxRect { x: 0, y: 90, w: 800, h: 20 }, PxRect { x: 0, y: 290, w: 800, h: 20 }]);
        u.input_box = [100.0, 300.0, 2.0, 0.0];
        u.input_box_x = [50.0, 400.0, 0.0, 0.0];
        let r = region_rects(&[RegionTerm::InputBox(1.0)], &u, 20.0, (800, 600)).unwrap();
        assert_eq!(r, [PxRect { x: 30, y: 80, w: 390, h: 240 }]);
    }

    #[test]
    fn full_after_change_is_full_only_while_it_plays() {
        let mut u = Uniforms::default();
        u.time_cursor_change = 1.0;
        let t = [RegionTerm::FullAfterChange(Duration::from_millis(600))];
        u.time = 1.5;
        assert_eq!(region_rects(&t, &u, 20.0, (800, 600)), None);
        u.time = 1.7;
        assert_eq!(region_rects(&t, &u, 20.0, (800, 600)), Some(vec![]));
        assert_eq!(region_rects(&[RegionTerm::Full], &u, 20.0, (800, 600)), None);
    }

    #[test]
    fn a_chain_with_one_regionless_plugin_is_full() {
        let a = region_manifest("[\"cursor\"]");
        let b = parse_manifest("shaders = \"a.glsl\"").unwrap();
        let c = region_manifest("[\"input_box\"]");
        let f = region_manifest("[\"full\"]");
        assert_eq!(chain_region(&[&a, &b]), None);
        assert_eq!(chain_region(&[&a, &f]), None);
        assert_eq!(chain_region(&[&a, &c]), Some(vec![RegionTerm::Cursor(0.0), RegionTerm::InputBox(0.0)]));
    }

    #[test]
    fn a_frames_region_unites_it_with_last_frames() {
        let last = cursor_uniforms([100.0, 40.0, 10.0, 20.0]);
        let mut now = cursor_uniforms([100.0, 400.0, 10.0, 20.0]);
        // Long after the cursor changed.
        now.time = 10.0;
        let t = [RegionTerm::Cursor(0.0)];
        assert_eq!(frame_region(&t, &now, &last, (8.0, 20.0), false, (800, 600)).unwrap(), [PxRect { x: 100, y: 400, w: 10, h: 20 }, PxRect { x: 100, y: 40, w: 10, h: 20 }]);
        // Either frame full: the full pass.
        let mut playing = now.clone();
        playing.time_cursor_change = playing.time;
        let f = [RegionTerm::Cursor(0.0), RegionTerm::FullAfterChange(Duration::from_millis(600))];
        assert_eq!(frame_region(&f, &now, &playing, (8.0, 20.0), false, (800, 600)), None);
        assert_eq!(frame_region(&f, &playing, &now, (8.0, 20.0), false, (800, 600)), None);
        // Nothing to draw in either: nothing shaded, never the full pass.
        assert_eq!(frame_region(&[], &now, &last, (8.0, 20.0), false, (800, 600)), Some(vec![]));
        // No box found in either frame: each one's line search band, the whole width.
        assert_eq!(frame_region(&[RegionTerm::InputBox(1.0)], &now, &last, (8.0, 20.0), false, (800, 600)).unwrap(), [PxRect { x: 0, y: 250, w: 800, h: 222 }, PxRect { x: 0, y: 0, w: 800, h: 112 }]);
    }

    #[test]
    fn pads_are_in_the_shaders_cell_unit() {
        let none = Uniforms::default();
        assert_eq!(pad_unit((8.0, 20.0), &none), 20.0);
        // Horizontal Spacing 200%: the shaders' cellH is twice the cell width.
        assert_eq!(pad_unit((16.0, 20.0), &none), 32.0);
        // A block cursor on a wide character (two cells): halved, so the cell's own bound.
        let mut u = Uniforms { current_cursor: [0.0, 20.0, 16.0, 20.0], ..Default::default() };
        assert_eq!(pad_unit((8.0, 20.0), &u), 20.0);
        // An underline on a wide character is not halved: 2 x 16.
        u.current_cursor = [0.0, 20.0, 16.0, 2.0];
        u.current_cursor_style = 3;
        assert_eq!(pad_unit((8.0, 20.0), &u), 32.0);
        // A tall block (Vertical Spacing) on a wide character: 40 >= 1.4 x 16, not halved.
        u.current_cursor = [0.0, 40.0, 16.0, 40.0];
        u.current_cursor_style = 0;
        assert_eq!(pad_unit((8.0, 40.0), &u), 40.0);
        u.current_cursor = [0.0, 30.0, 24.0, 34.0];
        assert_eq!(pad_unit((12.0, 34.0), &u), 48.0);
        // The previous cursor counts too.
        u.current_cursor = [0.0, 20.0, 8.0, 20.0];
        u.previous_cursor = [0.0, 20.0, 16.0, 2.0];
        u.current_cursor_style = 3;
        assert_eq!(pad_unit((8.0, 20.0), &u), 32.0);
        let u = cursor_uniforms([100.0, 100.0, 16.0, 2.0]);
        assert_eq!(own_region(&[RegionTerm::Cursor(1.0)], &u, (16.0, 20.0), false, (800, 600)).unwrap(), [PxRect { x: 68, y: 68, w: 80, h: 66 }]);
    }

    #[test]
    fn live_lines_with_no_box_found_is_the_full_pass() {
        let mut u = cursor_uniforms([100.0, 100.0, 10.0, 20.0]);
        u.time = 10.0;
        let t = [RegionTerm::Cursor(1.0), RegionTerm::InputBox(1.0)];
        assert_eq!(own_region(&t, &u, (8.0, 20.0), true, (800, 600)), None);
        assert!(own_region(&t, &u, (8.0, 20.0), false, (800, 600)).is_some());
        u.input_box = [60.0, 200.0, 1.0, 0.0];
        assert!(own_region(&t, &u, (8.0, 20.0), true, (800, 600)).is_some());
        let last = u.clone();
        u.input_box = [0.0; 4];
        assert_eq!(frame_region(&t, &last, &u, (8.0, 20.0), true, (800, 600)), None, "last frame had no box");
    }

    #[test]
    fn rects_are_clamped_and_merged() {
        let u = cursor_uniforms([790.0, 40.0, 10.0, 20.0]);
        let r = region_rects(&[RegionTerm::Cursor(1.0)], &u, 20.0, (800, 600)).unwrap();
        assert_eq!(r, [PxRect { x: 770, y: 20, w: 30, h: 60 }]);
        let mut u = Uniforms::default();
        u.prompt_mark = [0.0, 0.0, 1.0, 10.0];
        u.current_cursor = [5.0, 20.0, 10.0, 20.0];
        let t = [RegionTerm::PromptMark(0.0), RegionTerm::Cursor(0.0)];
        assert_eq!(region_rects(&t, &u, 20.0, (800, 600)).unwrap().len(), 1);
        let nine: Vec<[f32; 4]> = (0..9).map(|i| [i as f32 * 20.0, 0.0, i as f32 * 20.0 + 10.0, 10.0]).collect();
        assert_eq!(round_and_merge(nine, (800.0, 600.0)), [PxRect { x: 0, y: 0, w: 170, h: 10 }]);
    }

    #[test]
    fn manifest_errors_are_said() {
        assert!(parse_manifest("name = \"x\"").unwrap_err().contains("no shader"));
        assert!(parse_manifest("shaders = \"a.glsl\"\nanimation = \"sometimes\"").unwrap_err().contains("auto, always or off"));
        assert_eq!(parse_manifest("shaders = \"a.glsl\"").unwrap().animation, Animation::Auto);
    }

    #[test]
    fn every_builtin_plugin_compiles() {
        let (list, errors) = discover(None);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(list.iter().filter(|p| p.builtin()).count(), 13, "the 13 cursor-fx effects");
        for p in &list {
            for c in p.compile().unwrap_or_else(|e| panic!("{e}")) {
                let layout = uniform_layout(&c.module).expect("Ghostty's block");
                assert!(layout.fields.contains_key("iCurrentCursor"));
            }
            assert!(p.manifest.cursor_color.as_deref().and_then(hex_color).is_some(), "{} has a preset color", p.id);
        }
    }

    #[test]
    fn compile_errors_name_the_body_line() {
        let e = compile_glsl("void mainImage(out vec4 c, in vec2 p) {\n  c = vec4(nope);\n}").unwrap_err();
        assert!(e.contains("nope"), "{e}");
        assert!(e.contains("line 2:"), "line 2 of the body: {e}");
    }

    #[test]
    fn wgsl_needs_fs_main() {
        let ok = "@fragment fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return vec4<f32>(1.0); }";
        assert!(compile_wgsl(ok).is_ok());
        assert!(compile_wgsl(&ok.replace("fs_main", "other")).unwrap_err().contains("fs_main"));
    }

    #[test]
    fn layout_is_std140_as_ghostty_declares() {
        let c = compile_glsl("void mainImage(out vec4 c, in vec2 p) { c = vec4(iTime) + vec4(iPalette[3], 1.0); }").unwrap();
        let l = uniform_layout(&c.module).unwrap();
        // vec3 iResolution at 0, float iTime packs into its tail at 12 (std140).
        assert_eq!(l.fields["iResolution"], (0, 0));
        assert_eq!(l.fields["iTime"], (12, 0));
        // std140 arrays have a 16 byte stride.
        assert_eq!(l.fields["iPalette"].1, 16);
        // iChannelTime is a vec4 right after iFrame's 16 byte slot, not an array.
        assert_eq!(l.fields["iChannelTime"], (32, 0));
        assert_eq!(l.size % 16, 0);
        let mut u = Uniforms { time: 2.5, ..Default::default() };
        u.palette[3] = [0.25, 0.5, 0.75];
        let b = u.pack(&l);
        assert_eq!(b.len(), l.size as usize);
        assert_eq!(f32::from_le_bytes(b[12..16].try_into().unwrap()), 2.5);
        let p3 = (l.fields["iPalette"].0 + 3 * 16) as usize;
        assert_eq!(f32::from_le_bytes(b[p3 + 4..p3 + 8].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(b[32..36].try_into().unwrap()), 2.5, "iChannelTime[0] is the clock");
    }

    /// naga's Metal output lays a scalar array out 4 bytes apart, while the buffer is packed with
    /// std140's 16, so every member after one read zeros on macOS and no effect drew. The block
    /// must hold no scalar array (vec3 and vec4 arrays are 16 apart in Metal too).
    #[test]
    fn the_uniform_block_has_no_scalar_array() {
        let c = compile_glsl("void mainImage(out vec4 c, in vec2 p) { c = vec4(iTime); }").unwrap();
        let m = &c.module;
        let var = m.global_variables.iter().map(|(_, v)| v).find(|v| v.space == naga::AddressSpace::Uniform).unwrap();
        let naga::TypeInner::Struct { members, .. } = &m.types[var.ty].inner else { panic!("the block is a struct") };
        let bad: Vec<_> = members
            .iter()
            .filter(|mem| matches!(m.types[mem.ty].inner, naga::TypeInner::Array { base, .. } if matches!(m.types[base].inner, naga::TypeInner::Scalar(_))))
            .map(|mem| mem.name.clone().unwrap_or_default())
            .collect();
        assert!(bad.is_empty(), "scalar arrays in the uniform block: {bad:?}");
    }

    #[test]
    fn cursor_moves_keep_the_previous_one_and_the_time() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        let cur = |x: f32| Some(Cursor { rect: [x, 20.0, 8.0, 16.0], color: [1.0; 4], style: style::BLOCK, visible: true });
        clock.frame(&mut u, t0, (800, 600), cur(10.0), true);
        assert_eq!(u.current_cursor, [10.0, 36.0, 8.0, 16.0], "y is the bottom edge (y-down)");
        clock.frame(&mut u, t0 + Duration::from_millis(500), (800, 600), cur(18.0), true);
        assert_eq!(u.previous_cursor[0], 10.0);
        assert_eq!(u.current_cursor[0], 18.0);
        assert!((u.time_cursor_change - 0.5).abs() < 1e-3);
        assert_eq!(u.frame, 2);
        // No move: the change time stays.
        clock.frame(&mut u, t0 + Duration::from_millis(900), (800, 600), cur(18.0), true);
        assert!((u.time_cursor_change - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_color_change_alone_counts_as_a_cursor_change() {
        // cursor-fx signals Enter and done through the cursor color's lowest bits.
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        let mut c = Cursor { rect: [0.0, 0.0, 8.0, 16.0], color: [0.5, 0.5, 0.5, 1.0], style: 0, visible: true };
        clock.frame(&mut u, t0, (10, 10), Some(c), true);
        c.color[2] += 1.0 / 255.0;
        clock.frame(&mut u, t0 + Duration::from_secs(1), (10, 10), Some(c), true);
        assert_eq!(u.previous_cursor_color, [0.5, 0.5, 0.5, 1.0]);
        assert!((u.time_cursor_change - 1.0).abs() < 1e-3);
    }

    #[test]
    fn focus_time_is_set_when_focus_comes_back() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.frame(&mut u, t0, (10, 10), None, true);
        assert_eq!(u.time_focus, 0.0, "the first frame is not a focus change");
        clock.frame(&mut u, t0 + Duration::from_secs(1), (10, 10), None, false);
        assert_eq!(u.focus, 0);
        clock.frame(&mut u, t0 + Duration::from_secs(3), (10, 10), None, true);
        assert!((u.time_focus - 3.0).abs() < 1e-3);
    }

    #[test]
    fn auto_animation_stops_after_its_window() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        assert!(!clock.wants_frame(Animation::Auto, t0, true), "nothing happened yet");
        clock.poke(t0);
        assert!(clock.wants_frame(Animation::Auto, t0 + Duration::from_millis(1900), true));
        assert!(!clock.wants_frame(Animation::Auto, t0 + AUTO_WINDOW, true));
        assert!(clock.wants_frame(Animation::Always, t0 + Duration::from_secs(60), true));
        assert!(!clock.wants_frame(Animation::Always, t0, false), "always stops while unfocused");
        assert!(!clock.wants_frame(Animation::Off, t0, true));
    }

    /// A frame left out has no acquire or present to slow it: under Full the next one still waits.
    #[test]
    fn a_skipped_frame_waits_a_frame_before_it_asks_again() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.frame(&mut u, t0, (10, 10), None, true);
        for pacing in [Pacing::Full, Pacing::Balanced, Pacing::Light, Pacing::Smooth] {
            let due = clock.next_after_still(Animation::Always, t0, true, pacing).expect("an always animation");
            assert!(due > t0, "{pacing:?} sets a due time after now");
        }
        assert_eq!(clock.next_after_still(Animation::Always, t0, true, Pacing::Full), Some(t0 + Duration::from_micros(16_667)));
        assert_eq!(clock.next_after_still(Animation::Always, t0, true, Pacing::Light), Some(t0 + Duration::from_micros(66_667) - FRAME_SLACK));
        assert_eq!(clock.next_after_still(Animation::Always, t0, true, Pacing::Smooth), Some(t0 + Duration::from_micros(16_667) - FRAME_SLACK));
        assert_eq!(clock.next_after_still(Animation::Off, t0, true, Pacing::Full), None);
    }

    #[test]
    fn pacing_caps_the_frame_rate() {
        let t0 = Instant::now();
        // A clock that has drawn its first frame at t0 under `pacing`.
        let drawn = |pacing: Pacing| {
            let mut clock = Clock::default();
            let mut u = Uniforms::default();
            clock.set_pacing(pacing);
            clock.frame(&mut u, t0, (10, 10), None, true);
            clock
        };
        let at = |us: u64| Some(t0 + Duration::from_micros(us) - FRAME_SLACK);
        assert_eq!(drawn(Pacing::Full).next_frame(Animation::Always, t0, true, Pacing::Full), Some(t0), "full draws at once");
        assert_eq!(drawn(Pacing::Balanced).next_frame(Animation::Always, t0, true, Pacing::Balanced), at(33_333));
        assert_eq!(drawn(Pacing::Light).next_frame(Animation::Always, t0, true, Pacing::Light), at(66_667));
        assert_eq!(drawn(Pacing::Smooth).next_frame(Animation::Always, t0, true, Pacing::Smooth), at(16_667));
        let clock = drawn(Pacing::Light);
        let late = t0 + Duration::from_millis(500);
        assert_eq!(clock.next_frame(Animation::Always, late, true, Pacing::Light), Some(late), "an overdue frame draws now");
        assert_eq!(clock.next_frame(Animation::Off, t0, true, Pacing::Light), None);
        assert_eq!(clock.next_frame(Animation::Always, t0, false, Pacing::Full), None, "unfocused draws nothing extra");
    }

    #[test]
    fn pacing_pauses_an_idle_always_animation() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.frame(&mut u, t0, (10, 10), None, true);
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert!(clock.next_frame(Animation::Always, at(29), true, Pacing::Light).is_some());
        assert_eq!(clock.next_frame(Animation::Always, at(30), true, Pacing::Light), None, "light pauses after 30 s");
        assert!(clock.next_frame(Animation::Always, at(119), true, Pacing::Balanced).is_some());
        assert_eq!(clock.next_frame(Animation::Always, at(120), true, Pacing::Balanced), None, "balanced pauses after 2 min");
        assert!(clock.next_frame(Animation::Always, at(3600), true, Pacing::Full).is_some(), "full never pauses");
        // A cursor that moves (a key, output) wakes it.
        let c = Cursor { rect: [1.0, 1.0, 1.0, 1.0], color: [1.0; 4], style: 0, visible: true };
        clock.frame(&mut u, at(200), (10, 10), Some(c), true);
        assert!(clock.next_frame(Animation::Always, at(201), true, Pacing::Light).is_some(), "a change wakes it");
    }

    #[test]
    fn pacing_reads_the_setting() {
        assert_eq!(Pacing::from_setting(0), Pacing::Full);
        assert_eq!(Pacing::from_setting(1), Pacing::Balanced);
        assert_eq!(Pacing::from_setting(2), Pacing::Light);
        assert_eq!(Pacing::from_setting(3), Pacing::Smooth);
        assert_eq!(Pacing::from_setting(9), Pacing::Smooth, "an unknown value takes the default");
        assert_eq!(Pacing::default(), Pacing::Smooth);
    }

    /// Frames drawn by an Always animation asked at each tick (ns from the start).
    fn drawn_at(pacing: Pacing, ticks: impl Iterator<Item = u64>) -> u32 {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(pacing);
        clock.frame(&mut u, t0, (10, 10), None, true);
        let mut drawn = 0;
        for ns in ticks {
            let now = t0 + Duration::from_nanos(ns);
            if clock.next_frame(Animation::Always, now, true, pacing).is_some_and(|due| due <= now) {
                clock.frame(&mut u, now, (10, 10), None, true);
                drawn += 1;
            }
        }
        drawn
    }

    /// Vsync ticks at `hz` for `secs`, each moved by a fixed pseudo-random offset within +-1 ms when `jitter`.
    fn vsyncs(hz: u64, secs: u64, jitter: bool) -> impl Iterator<Item = u64> {
        let mut x = 12345u64;
        (1..=hz * secs).map(move |i| {
            let at = i * 1_000_000_000 / hz;
            if !jitter {
                return at;
            }
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (at + (x >> 33) % 2_000_001).saturating_sub(1_000_000)
        })
    }

    #[test]
    fn capped_rates_average_their_target() {
        for hz in [60u64, 75, 120, 144, 165, 240] {
            for jitter in [false, true] {
                for (pacing, fps) in [(Pacing::Smooth, 60u32), (Pacing::Balanced, 30), (Pacing::Light, 15)] {
                    let drawn = drawn_at(pacing, vsyncs(hz, 2, jitter));
                    let (lo, hi) = (2 * fps - 2, 2 * fps + 2);
                    assert!((lo..=hi).contains(&drawn), "{pacing:?} at {hz} Hz (jitter {jitter}) drew {drawn} in 2 s");
                    if hz == 60 && !jitter && pacing == Pacing::Smooth {
                        assert_eq!(drawn, 120, "60 Hz draws every vsync");
                    }
                }
            }
        }
    }

    #[test]
    fn smooth_caps_at_sixty_on_a_fast_screen() {
        // A 120 Hz screen asks every 8.333 ms for one second.
        let drawn = drawn_at(Pacing::Smooth, (1..=120u64).map(|i| i * 8_333_333));
        assert!((59..=61).contains(&drawn), "drew {drawn} frames in a second");
    }

    #[test]
    fn smooth_never_passes_sixty_on_240hz() {
        let drawn = drawn_at(Pacing::Smooth, (1..=240u64).map(|i| i * 4_166_667));
        assert!(drawn <= 61, "drew {drawn} frames in a second");
    }

    #[test]
    fn a_stall_does_not_burst() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(Pacing::Smooth);
        clock.frame(&mut u, t0, (10, 10), None, true);
        let stall = t0 + Duration::from_millis(500);
        let mut drawn = 0;
        for i in 0..3u64 {
            let now = stall + Duration::from_nanos(i * 8_333_333);
            if clock.next_frame(Animation::Always, now, true, Pacing::Smooth).is_some_and(|due| due <= now) {
                clock.frame(&mut u, now, (10, 10), None, true);
                drawn += 1;
            }
        }
        assert!(drawn <= 2, "{drawn} frames in the first 20 ms after a stall");
    }

    #[test]
    fn content_redraws_do_not_push_the_phase() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(Pacing::Smooth);
        let gap = Pacing::Smooth.frame_gap().unwrap();
        for i in 0..20u64 {
            clock.frame(&mut u, t0 + Duration::from_millis(i), (10, 10), None, true);
        }
        let now = t0 + Duration::from_millis(19);
        let due = clock.next_frame(Animation::Always, now, true, Pacing::Smooth).unwrap();
        assert!(due <= now + gap, "next frame {:?} past now", due - now);
    }

    #[test]
    fn still_cycles_do_not_drift() {
        let mut now = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(Pacing::Smooth);
        let gap = Pacing::Smooth.frame_gap().unwrap();
        clock.frame(&mut u, now, (10, 10), None, true);
        for _ in 0..100 {
            now = clock.next_after_still(Animation::Always, now, true, Pacing::Smooth).unwrap();
            clock.frame(&mut u, now, (10, 10), None, true);
        }
        let due = clock.next_frame(Animation::Always, now, true, Pacing::Smooth).unwrap();
        assert!(due <= now + gap, "next frame {:?} past now", due - now);
    }

    #[test]
    fn the_first_frame_waits_its_gap() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.set_pacing(Pacing::Smooth);
        clock.frame(&mut u, t0, (10, 10), None, true);
        let due = clock.next_frame(Animation::Always, t0, true, Pacing::Smooth);
        assert_eq!(due, Some(t0 + Duration::from_micros(16_667) - FRAME_SLACK));
    }

    #[test]
    fn smooth_pauses_when_idle() {
        let t0 = Instant::now();
        let mut clock = Clock::default();
        let mut u = Uniforms::default();
        clock.frame(&mut u, t0, (10, 10), None, true);
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert!(clock.next_frame(Animation::Always, at(119), true, Pacing::Smooth).is_some());
        assert_eq!(clock.next_frame(Animation::Always, at(120), true, Pacing::Smooth), None, "smooth pauses after 2 min");
        let c = Cursor { rect: [1.0, 1.0, 1.0, 1.0], color: [1.0; 4], style: 0, visible: true };
        clock.frame(&mut u, at(200), (10, 10), Some(c), true);
        assert!(clock.next_frame(Animation::Always, at(201), true, Pacing::Smooth).is_some(), "a cursor change wakes it");
    }

    #[test]
    fn settings_read_and_write_in_place() {
        let src = "// head\nconst float SPEED = 1.5;  // how fast\r\nconst bool ON = true;\nconst vec3 TINT = vec3(1.0, 0.5, 0.0); // color\nfloat x = 1.0;\n";
        let s = settings(src);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].name.as_str(), s[0].value.as_str(), s[0].help.as_str(), s[0].line), ("SPEED", "1.5", "how fast", 1));
        assert_eq!(s[2].value, "vec3(1.0, 0.5, 0.0)");
        let out = set_setting(src, "SPEED", "2.0").unwrap();
        assert_eq!(out, src.replace("SPEED = 1.5;", "SPEED = 2.0;"));
        assert!(set_setting(src, "NOPE", "1").is_err());
        assert!(set_setting(src, "SPEED", "1; evil()").is_err());
    }

    #[test]
    fn discovery_lets_the_user_folder_replace_a_builtin() {
        let _td = TempDir::new("render_fx_test_");
        let tmp = _td.to_path_buf();
        let u = tmp.join("u");
        std::fs::create_dir_all(u.join("neon")).unwrap();
        std::fs::write(u.join("neon").join("plugin.toml"), "name = \"Mine\"\nshaders = \"n.glsl\"").unwrap();
        std::fs::write(u.join("neon").join("n.glsl"), "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }").unwrap();
        std::fs::create_dir_all(u.join("broken")).unwrap();
        std::fs::write(u.join("broken").join("plugin.toml"), "name = 1").unwrap();
        const BUILTIN: &[crate::render_fx_builtin::Builtin] = &[("neon", "name = \"Builtin\"\nshaders = \"n.glsl\"", &[("n.glsl", "")])];
        let (list, errors) = discover_in(BUILTIN, Some(&u));
        let compiled = list[0].compile();
        let escape = list[0].shader_text("../plugin.toml");
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].title(), list[0].builtin()), ("Mine", false));
        assert_eq!(compiled.map(|c| c.len()), Ok(1));
        assert!(escape.is_err(), "a manifest cannot reach outside its folder");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "broken");
    }

    fn done() -> DoneRule {
        crate::fx_profile::FxRules::default().done
    }

    #[test]
    fn a_wait_keeps_its_own_done_times() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Signals::default();
        s.enter(t0, &DoneRule { quiet: Duration::from_millis(3000), ..done() });
        s.output(ms(0));
        s.output(ms(2000));
        assert!(!s.tick(ms(4100)), "2100 ms of quiet is not 3000");
        assert!(s.tick(ms(5100)));
        // The next wait takes the rule it is given.
        s.enter(ms(6000), &done());
        s.output(ms(6000));
        s.output(ms(7600));
        assert!(s.tick(ms(8800)));
    }

    #[test]
    fn osc133_off_ignores_command_ended() {
        let t0 = Instant::now();
        let mut s = Signals::default();
        s.enter(t0, &DoneRule { osc133: false, ..done() });
        s.command_ended(t0 + Duration::from_secs(4));
        assert_eq!(s.current(t0 + Duration::from_secs(4)), None, "no done, and the Enter signal has run out");
        s.enter(t0, &done());
        s.command_ended(t0 + Duration::from_secs(4));
        assert_eq!(s.current(t0 + Duration::from_secs(4)), Some(Signal::Done));
    }

    #[test]
    fn enter_and_done_signals_follow_cursor_fx() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Signals::default();
        let base = [0x9f, 0xd0, 0xfe];
        assert_eq!(s.apply(base, t0), base, "no signal, no change");
        s.enter(t0, &done());
        assert_eq!(s.current(ms(100)), Some(Signal::Enter));
        assert_eq!(s.apply(base, ms(100)), [0x9f, 0xd0, 0xff], "Enter: blue odd, green even, red flipped");
        // The answer streams for 3 s, then goes quiet.
        for n in (200..3000).step_by(100) {
            s.output(ms(n));
            assert!(!s.tick(ms(n)));
        }
        assert!(!s.tick(ms(2900 + 1100)), "not quiet long enough yet");
        assert!(s.tick(ms(2900 + 1200)));
        assert_eq!(s.current(ms(4200)), Some(Signal::Done));
        assert_eq!(s.apply(base, ms(4200)), [0x9e, 0xd1, 0xfe], "done: green odd, blue even, red flipped back");
        assert_eq!(s.apply(base, ms(4100 + 1900)), base, "a signal ends after SIGNAL_TIME");
        assert!(!s.tick(ms(9000)), "done fires once per Enter");
    }

    #[test]
    fn a_quick_command_gets_no_done() {
        let t0 = Instant::now();
        let mut s = Signals::default();
        s.enter(t0, &done());
        s.output(t0 + Duration::from_millis(80));
        assert!(!s.tick(t0 + Duration::from_secs(5)));
        assert_eq!(s.current(t0 + Duration::from_secs(5)), None);
        // A shell's command-ended mark after a long run is done at once.
        s.enter(t0, &done());
        s.command_ended(t0 + Duration::from_secs(4));
        assert_eq!(s.current(t0 + Duration::from_secs(4)), Some(Signal::Done));
    }

    #[test]
    fn switches_are_set_in_every_builtin_that_has_them() {
        let (list, _) = discover(None);
        for p in &list {
            let text = p.shader_text(&p.manifest.shaders[0]).unwrap();
            let off = with_switches(&text, &[("ENTER_EFFECT", false), ("LINE_EFFECT", true), ("NO_SUCH", true)]);
            let s = settings(&off);
            let get = |n: &str| s.iter().find(|x| x.name == n).map(|x| x.value.clone());
            assert_eq!(get("ENTER_EFFECT").as_deref(), Some("false"), "{}", p.id);
            assert_eq!(get("LINE_EFFECT").as_deref(), Some("true"), "{}", p.id);
            assert!(p.compile_with(&[("ENTER_EFFECT", false), ("DONE_EFFECT", false)]).is_ok(), "{}", p.id);
        }
        for (key, constant, _, _, _) in SWITCHES {
            assert!(crate::schema::find(key).is_some(), "{key} is in the profile schema");
            if let Some(c) = constant {
                assert!(list.iter().all(|p| p.shader_text(&p.manifest.shaders[0]).unwrap().contains(&format!(" {c} "))), "{c} is in every built-in");
            }
        }
    }

    #[test]
    fn every_builtin_reads_the_prompt_mark_uniform() {
        // The text of the function that starts with `head`, up to the next column 0 `}`.
        let body_of = |src: &str, head: &str| -> String { src.lines().skip_while(|l| !l.starts_with(head)).take_while(|l| !l.starts_with('}')).collect::<Vec<_>>().join("\n") };
        for p in discover(None).0 {
            let patched = patch_input_box(&p.shader_text(&p.manifest.shaders[0]).unwrap());
            for head in ["float clMarkLeft(", "bool clChevron(", "vec2 clMarkAbove("] {
                assert!(body_of(&patched, head).contains("iPromptMark"), "{}: {head} reads iPromptMark", p.id);
            }
            p.compile().unwrap_or_else(|e| panic!("{}: {e}", p.id));
        }
    }

    #[test]
    fn the_input_box_patch_reaches_every_builtin() {
        for p in discover(None).0 {
            let text = p.shader_text(&p.manifest.shaders[0]).unwrap();
            let patched = patch_input_box(&text);
            assert_eq!(text.lines().count(), patched.lines().count(), "{}: line numbers stay", p.id);
            assert!(patched.contains("iInputBox.z > 0.5) { float tA"), "{}: boxLineY answers from the grid", p.id);
            assert!(!bounds_re().is_match(&patched), "{}: every 4-row bound reaches the box top", p.id);
            assert!(!above_re().captures_iter(&patched).any(|c| c[2].parse::<f32>().unwrap_or(0.0) >= 4.0), "{}: every summed bound reaches the box top", p.id);
            assert!(patched.matches("termurionAbove(").count() >= 1, "{}", p.id);
            // A band: its edges only between its left and right edge, linesLook's quick exits
            // skipped, and LINE_TINT drawing a line of its own.
            assert!(patched.matches("iInputBoxX.x").count() >= 2, "{}: boxLineY and clLineNear keep to the band's columns", p.id);
            assert!(patched.contains("return termurionBandLine("), "{}: lineTint draws a band's edges", p.id);
            for exit in ["if (!look && colorDist", "if (!look && (colorDist", "if (!LINE_EFFECT && colorDist"] {
                assert!(!patched.contains(exit), "{}: `{exit}` is skipped for a band", p.id);
            }
            assert!(!patched.contains("|| (p.y > cur.y - h && p.y < cur.y)) return col;"), "{}: linesLook's row bounds reach a band's edges", p.id);
            assert!(patched.contains("termurionBelow(cellH, "), "{}: the Enter look reaches a band's bottom edge", p.id);
            assert_eq!(patched.matches(" termurionMarkBg = ").count(), 1, "{}: mainImage reads a band's shade once", p.id);
            assert_eq!(patched.matches("termurionMarkBg)").count(), 4, "{}: the prompt mark's tests see a band's shade as background", p.id);
            assert!(patched.contains("float termurionMarkDist(") && !patched.contains("colorDist(screenAt(vec2(x + f * w, y) / res).rgb, iBackgroundColor)"), "{}: clChevron reads ink with it", p.id);
            assert!(patched.contains("(iInputBox.z > 1.5 ? 0.0 : clLineNear(cur.y - 1.7"), "{}: a band counts as a box below the cursor", p.id);
        }
        // A shader without the functions is untouched.
        let plain = "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }";
        assert_eq!(patch_input_box(plain), plain);
    }

    #[test]
    fn a_plain_ghostty_shader_is_left_as_it_is_and_compiles() {
        // A user's cursor shader in Ghostty's form: the standard mainImage line, none of the
        // built-ins' helpers (no clMark functions, no screenAt, no Y_DOWN).
        let ghostty = "float sdBox(vec2 p, vec2 b) {\n    vec2 d = abs(p) - b;\n    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);\n}\n\nvoid mainImage(out vec4 fragColor, in vec2 fragCoord) {\n    vec2 uv = fragCoord / iResolution.xy;\n    fragColor = texture(iChannel0, uv);\n    vec2 c = iCurrentCursor.xy + 0.5 * iCurrentCursor.zw;\n    float d = sdBox(fragCoord - c, 0.5 * iCurrentCursor.zw);\n    fragColor = mix(fragColor, iCurrentCursorColor, step(d, 0.0) * 0.5);\n}\n";
        assert_eq!(patch_input_box(ghostty), ghostty);
        if let Err(e) = compile_glsl(ghostty) {
            panic!("a plain Ghostty shader compiles: {e}");
        }
    }

    #[test]
    fn hex_colors_parse() {
        assert_eq!(hex_color("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(hex_color("ff8000"), None);
    }

    /// The packing as it was before `pack_into`, kept to prove the new one writes the same bytes.
    fn pack_reference(u: &Uniforms, layout: &Layout) -> Vec<u8> {
        fn fl(v: &[f32]) -> Vec<u8> {
            v.iter().flat_map(|f| f.to_le_bytes()).collect()
        }
        let mut buf = vec![0u8; layout.size as usize];
        let mut f = |name: &str, bytes: Vec<u8>| {
            if let Some(&(off, _)) = layout.fields.get(name) {
                put(&mut buf, off, &bytes);
            }
        };
        f("iResolution", fl(&u.resolution));
        f("iTime", fl(&[u.time]));
        f("iTimeDelta", fl(&[u.time_delta]));
        f("iFrameRate", fl(&[u.frame_rate]));
        f("iFrame", u.frame.to_le_bytes().to_vec());
        f("iMouse", fl(&[0.0; 4]));
        f("iDate", fl(&u.date));
        f("iSampleRate", fl(&[44100.0]));
        f("iCurrentCursor", fl(&u.current_cursor));
        f("iPreviousCursor", fl(&u.previous_cursor));
        f("iCurrentCursorColor", fl(&u.current_cursor_color));
        f("iPreviousCursorColor", fl(&u.previous_cursor_color));
        f("iCurrentCursorStyle", u.current_cursor_style.to_le_bytes().to_vec());
        f("iPreviousCursorStyle", u.previous_cursor_style.to_le_bytes().to_vec());
        f("iCursorVisible", u.cursor_visible.to_le_bytes().to_vec());
        f("iTimeCursorChange", fl(&[u.time_cursor_change]));
        f("iTimeFocus", fl(&[u.time_focus]));
        f("iFocus", u.focus.to_le_bytes().to_vec());
        f("iBackgroundColor", fl(&u.background));
        f("iForegroundColor", fl(&u.foreground));
        f("iCursorColor", fl(&u.cursor_color));
        f("iCursorText", fl(&u.cursor_text));
        f("iSelectionForegroundColor", fl(&u.selection_foreground));
        f("iSelectionBackgroundColor", fl(&u.selection_background));
        f("iInputBox", fl(&u.input_box));
        f("iInputBoxX", fl(&u.input_box_x));
        f("iPromptMark", fl(&u.prompt_mark));
        f("iChannelTime", fl(&[u.time, 0.0, 0.0, 0.0]));
        let mut array = |name: &str, items: Vec<Vec<u8>>| {
            if let Some(&(off, stride)) = layout.fields.get(name) {
                for (i, bytes) in items.iter().enumerate() {
                    put(&mut buf, off + i as u32 * stride, bytes);
                }
            }
        };
        array("iChannelResolution", (0..4).map(|i| fl(&if i == 0 { [u.resolution[0], u.resolution[1], 1.0] } else { [0.0; 3] })).collect());
        array("iPalette", u.palette.iter().take(256).map(|c| fl(c)).collect());
        buf
    }

    /// Every field a different number, so a swapped or skipped field shows.
    fn filled() -> Uniforms {
        let mut n = 0.5f32;
        let mut next = || {
            n += 1.25;
            n
        };
        let mut v4 = |next: &mut dyn FnMut() -> f32| [next(), next(), next(), next()];
        let mut u = Uniforms::default();
        u.resolution = [next(), next(), 1.0];
        u.time = next();
        u.time_delta = next();
        u.frame_rate = next();
        u.frame = 41;
        u.date = v4(&mut next);
        u.current_cursor = v4(&mut next);
        u.previous_cursor = v4(&mut next);
        u.current_cursor_color = v4(&mut next);
        u.previous_cursor_color = v4(&mut next);
        u.current_cursor_style = 2;
        u.previous_cursor_style = 3;
        u.cursor_visible = 1;
        u.time_cursor_change = next();
        u.time_focus = next();
        u.focus = 0;
        for (i, c) in u.palette.iter_mut().enumerate() {
            *c = [i as f32 / 255.0, 1.0 - i as f32 / 255.0, next()];
        }
        u.background = [next(), next(), next()];
        u.foreground = [next(), next(), next()];
        u.cursor_color = [next(), next(), next()];
        u.cursor_text = [next(), next(), next()];
        u.selection_foreground = [next(), next(), next()];
        u.selection_background = [next(), next(), next()];
        u.input_box = v4(&mut next);
        u.input_box_x = v4(&mut next);
        u.prompt_mark = v4(&mut next);
        u
    }

    fn builtin_layouts() -> Vec<(String, Layout)> {
        let (list, errors) = discover(None);
        assert!(errors.is_empty(), "{errors:?}");
        let mut out = Vec::new();
        for p in list.iter().filter(|p| p.builtin()) {
            for (i, c) in p.compile().unwrap().iter().enumerate() {
                if let Some(l) = uniform_layout(&c.module) {
                    out.push((format!("{} pass {i}", p.id), l));
                }
            }
        }
        assert!(out.len() >= 13);
        out
    }

    #[test]
    fn pack_into_matches_pack() {
        let u = filled();
        let mut out = vec![0xAAu8; 7];
        for (name, l) in builtin_layouts() {
            u.pack_into(&l, &mut out);
            assert_eq!(out, pack_reference(&u, &l), "{name}");
            assert_eq!(u.pack(&l), out, "{name}: pack");
        }
    }

    #[test]
    fn pack_into_allocates_nothing() {
        let u = filled();
        let layouts = builtin_layouts();
        let mut out = Vec::new();
        for (_, l) in &layouts {
            u.pack_into(l, &mut out);
        }
        let (_, allocs) = counting::count(|| {
            for _ in 0..100 {
                for (_, l) in &layouts {
                    u.pack_into(l, &mut out);
                }
            }
        });
        assert_eq!(allocs, 0, "a warm buffer packs without allocating");
    }

    /// A frame that moves only the clock and the cursor leaves every byte outside `clock_span`
    /// alone, so `run_with` can write just the span.
    #[test]
    fn the_clock_span_holds_what_an_animation_frame_changes() {
        let a = filled();
        let mut b = a.clone();
        b.time += 0.5;
        b.time_delta = 0.016;
        b.frame += 1;
        b.current_cursor = [9.0, 8.0, 7.0, 6.0];
        b.previous_cursor = [1.0, 2.0, 3.0, 4.0];
        b.current_cursor_color = [0.1, 0.2, 0.3, 0.4];
        b.previous_cursor_color = [0.5, 0.6, 0.7, 0.8];
        b.current_cursor_style = 0;
        b.previous_cursor_style = 1;
        b.cursor_visible = 0;
        b.time_cursor_change += 1.0;
        b.time_focus += 1.0;
        b.focus = 1;
        for (name, l) in builtin_layouts() {
            let (mut pa, mut pb) = (Vec::new(), Vec::new());
            a.pack_into(&l, &mut pa);
            b.pack_into(&l, &mut pb);
            let span = l.clock_span();
            assert!(span.end <= pa.len() && span.start % 4 == 0 && span.end % 4 == 0, "{name}: {span:?}");
            assert_eq!(pa[..span.start], pb[..span.start], "{name}: before the span");
            assert_eq!(pa[span.end..], pb[span.end..], "{name}: after the span");
            assert_ne!(pa[span.clone()], pb[span], "{name}: the span moved");
        }
    }

    /// More than 64 terms: the late one is in the key too.
    #[test]
    fn the_rects_key_covers_every_term() {
        let mut terms = vec![RegionTerm::Cursor(1.0); 69];
        terms.push(RegionTerm::FullAfterChange(Duration::from_millis(500)));
        let mut u = cursor_uniforms([200.0, 300.0, 10.0, 20.0]);
        u.time = 5.0;
        let last = u.clone();
        let mut cache = RectsCache::default();
        let outside = cache.get(&terms, &u, &last, (10.0, 20.0), false, (800, 600));
        assert!(outside.own.is_some());
        // The cursor just changed: inside the term's window the pass is full.
        let mut v = u.clone();
        v.time_cursor_change = v.time;
        let inside = cache.get(&terms, &v, &last, (10.0, 20.0), false, (800, 600));
        assert_eq!(cache.computes(), 2, "the late term flipped, so the key missed");
        assert_eq!(inside.own, None);
    }

    #[test]
    fn no_region_terms_share_one_answer() {
        let cache = RectsCache::default();
        let (_, allocs) = counting::count(|| {
            for _ in 0..100 {
                let _ = cache.none();
            }
        });
        assert_eq!(allocs, 0);
    }

    #[test]
    fn frame_rects_reuses_its_answer() {
        let terms = [RegionTerm::Cursor(1.0), RegionTerm::FullAfterChange(Duration::from_millis(500))];
        let mut u = cursor_uniforms([200.0, 300.0, 10.0, 20.0]);
        u.time = 5.0;
        let last = u.clone();
        let mut cache = RectsCache::default();
        let first = cache.get(&terms, &u, &last, (10.0, 20.0), false, (800, 600));
        assert_eq!(*first, frame_rects(&terms, &u, &last, (10.0, 20.0), false, (800, 600)));
        assert_eq!(cache.computes(), 1);
        // The clock moves every frame; the rects do not.
        let mut v = u.clone();
        v.time += 0.016;
        v.frame += 1;
        let again = cache.get(&terms, &v, &last, (10.0, 20.0), false, (800, 600));
        assert_eq!(*again, *first);
        assert_eq!(cache.computes(), 1, "the same inputs compute once");
        // A moved cursor is a new answer.
        let w = cursor_uniforms([400.0, 300.0, 10.0, 20.0]);
        let moved = cache.get(&terms, &w, &last, (10.0, 20.0), false, (800, 600));
        assert_eq!(*moved, frame_rects(&terms, &w, &last, (10.0, 20.0), false, (800, 600)));
        assert_eq!(cache.computes(), 2);
    }
}

#[cfg(test)]
/// A global allocator that counts allocations of the thread that armed it (a counter per
/// thread, so the other tests in this binary do not disturb it).
pub(crate) mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if ARMED.try_with(|a| a.get()).unwrap_or(false) {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            }
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            if ARMED.try_with(|a| a.get()).unwrap_or(false) {
                let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            }
            unsafe { System.realloc(p, l, n) }
        }
    }

    /// How many allocations `f` made on this thread.
    pub fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
        COUNT.with(|c| c.set(0));
        ARMED.with(|a| a.set(true));
        let r = f();
        ARMED.with(|a| a.set(false));
        (r, COUNT.with(|c| c.get()))
    }
}

#[cfg(test)]
// This applies to the whole lib test binary (one global allocator per binary, so no other
// test code may declare one).
#[global_allocator]
static ALLOC: counting::Counting = counting::Counting;
