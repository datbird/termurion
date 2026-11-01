//! Render plugins (pure part): shaders that run over the finished frame. A plugin is a folder
//! with `plugin.toml` and Ghostty-style GLSL or WGSL. This module finds plugins, compiles their
//! shaders with naga, lays out Ghostty's uniform block and keeps its cursor and time rules.
//! No OS calls, so it is the same on every platform. Design:
//! `docs/superpowers/specs/2026-10-31-render-plugins-design.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    ("Effects Claude Enter", Some("ENTER_EFFECT"), "Enter surge (a prompt sent, a command run)", true, "Claude Code"),
    ("Effects Claude Done", Some("DONE_EFFECT"), "Done surge (Claude finished, a long command ended)", true, "Claude Code"),
    ("Effects Claude Line Color", Some("LINE_TINT"), "Input box lines in the effect's color", true, "Claude Code"),
    ("Effects Claude Live Lines", Some("LINE_EFFECT"), "Input box lines always alive", false, "Claude Code"),
    ("Effects Claude Prompt", Some("PROMPT_EFFECT"), "Prompt mark (\u{276f}) look", true, "Claude Code"),
    ("Effects Signals", None, "Termurion sends the Enter and done signals itself (no hooks needed)", true, "Claude Code"),
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
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let v: toml::Table = text.parse().map_err(|e| format!("plugin.toml: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let shaders: Vec<String> = match v.get("shaders") {
        Some(toml::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(toml::Value::String(one)) => vec![one.clone()],
        _ => vec![],
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
    Ok(Manifest { name: s("name"), description: s("description"), shaders, animation, cursor_color, author: s("author"), license: s("license") })
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
                Ok(manifest) => {
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
/// Termurion found the box in the grid (`iInputBox`), `boxLineY`, `clLineNear` and `boxLines`
/// answer with its lines, and the "how far above the cursor" bounds reach the top line. Shaders
/// without these functions are left as they are.
pub fn patch_input_box(body: &str) -> String {
    let up = |v: &str| format!("(Y_DOWN ? iResolution.y - {v} : {v})");
    let (top, bottom) = (up("iInputBox.x"), up("iInputBox.y"));
    let nearest = |at: &str| format!(" if (iInputBox.z > 0.5) {{ float tA = {top}, tB = {bottom}; float tN = abs({at} - tA) < abs({at} - tB) ? tA : tB; return abs({at} - tN) <= reach ? tN : -1.0; }}");
    let mut out = String::with_capacity(body.len() + 1024);
    for line in body.split_inclusive('\n') {
        let (text, end) = match line.strip_suffix('\n') {
            Some(t) => (t, "\n"),
            None => (line, ""),
        };
        let trimmed = text.trim_end();
        let mut text = text.to_string();
        if trimmed.starts_with("float boxLineY(vec2 p, vec2 center, vec4 cur, vec2 res, float reach)") && trimmed.ends_with('{') {
            text.push_str(&nearest("p.y"));
        } else if trimmed.starts_with("float clLineNear(float y0, float reach, vec2 res)") && trimmed.ends_with('{') {
            text.push_str(&nearest("y0"));
        } else if trimmed.starts_with("vec2 boxLines(vec4 cur, vec2 center, float cellW, float cellH, vec2 res)") && trimmed.ends_with('{') {
            text.push_str(&format!(" if (iInputBox.z > 0.5) return vec2({top}, {bottom});"));
        }
        out.push_str(&bounds_re().replace_all(&text, |c: &regex::Captures| {
            let unit = if c[2].contains("cur.w") { "cur.w" } else { "cellH" };
            let m = &c[2];
            if let Some(name) = m.strip_prefix("(4.0 + ").and_then(|r| r.strip_suffix(") * cellH")) {
                format!("{}(termurionAbove(cellH) + {name} * cellH)", &c[1])
            } else if m == "4.5 * cellH" {
                format!("{}(termurionAbove(cellH) + 0.5 * cellH)", &c[1])
            } else {
                format!("{}termurionAbove({unit})", &c[1])
            }
        }));
        out.push_str(end);
    }
    out
}

fn bounds_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(center\.y\s*[<>]\s*)(\(4\.0 \+ [A-Z_]+\) \* cellH|4\.5 \* cellH|4\.0 \* cellH|4\.0 \* cur\.w)").unwrap())
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
    /// Termurion's `iInputBox`: the input box's top and bottom line (pixels from the top), and
    /// 1 when there is one.
    pub input_box: [f32; 4],
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
        }
    }
}

fn put(buf: &mut [u8], at: u32, bytes: &[u8]) {
    let at = at as usize;
    if let Some(dst) = buf.get_mut(at..at + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

impl Uniforms {
    /// The uniform buffer's bytes for `layout`. Members the layout lacks are skipped.
    pub fn pack(&self, layout: &Layout) -> Vec<u8> {
        let mut buf = vec![0u8; layout.size as usize];
        let mut f = |name: &str, bytes: Vec<u8>| {
            if let Some(&(off, _)) = layout.fields.get(name) {
                put(&mut buf, off, &bytes);
            }
        };
        f("iResolution", floats(&self.resolution));
        f("iTime", floats(&[self.time]));
        f("iTimeDelta", floats(&[self.time_delta]));
        f("iFrameRate", floats(&[self.frame_rate]));
        f("iFrame", self.frame.to_le_bytes().to_vec());
        f("iMouse", floats(&[0.0; 4]));
        f("iDate", floats(&self.date));
        f("iSampleRate", floats(&[44100.0]));
        f("iCurrentCursor", floats(&self.current_cursor));
        f("iPreviousCursor", floats(&self.previous_cursor));
        f("iCurrentCursorColor", floats(&self.current_cursor_color));
        f("iPreviousCursorColor", floats(&self.previous_cursor_color));
        f("iCurrentCursorStyle", self.current_cursor_style.to_le_bytes().to_vec());
        f("iPreviousCursorStyle", self.previous_cursor_style.to_le_bytes().to_vec());
        f("iCursorVisible", self.cursor_visible.to_le_bytes().to_vec());
        f("iTimeCursorChange", floats(&[self.time_cursor_change]));
        f("iTimeFocus", floats(&[self.time_focus]));
        f("iFocus", self.focus.to_le_bytes().to_vec());
        f("iBackgroundColor", floats(&self.background));
        f("iForegroundColor", floats(&self.foreground));
        f("iCursorColor", floats(&self.cursor_color));
        f("iCursorText", floats(&self.cursor_text));
        f("iSelectionForegroundColor", floats(&self.selection_foreground));
        f("iSelectionBackgroundColor", floats(&self.selection_background));
        f("iInputBox", floats(&self.input_box));
        // Arrays: element i at offset + i * stride.
        let mut array = |name: &str, items: Vec<Vec<u8>>| {
            if let Some(&(off, stride)) = layout.fields.get(name) {
                for (i, bytes) in items.iter().enumerate() {
                    put(&mut buf, off + i as u32 * stride, bytes);
                }
            }
        };
        array("iChannelTime", (0..4).map(|i| floats(&[if i == 0 { self.time } else { 0.0 }])).collect());
        array("iChannelResolution", (0..4).map(|i| floats(&if i == 0 { [self.resolution[0], self.resolution[1], 1.0] } else { [0.0; 3] })).collect());
        array("iPalette", self.palette.iter().take(256).map(|c| floats(c)).collect());
        buf
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
}

impl Default for Clock {
    fn default() -> Self {
        Clock { first: None, last: None, focused: None, last_change: None }
    }
}

impl Clock {
    /// Updates `u` for a frame drawn at `now`.
    pub fn frame(&mut self, u: &mut Uniforms, now: Instant, size: (u32, u32), cursor: Option<Cursor>, focused: bool) {
        let first = *self.first.get_or_insert(now);
        let last = self.last.unwrap_or(now);
        self.last = Some(now);
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

    /// Something the effects react to happened (a key, output, a click).
    pub fn poke(&mut self, now: Instant) {
        self.last_change = Some(now);
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
/// Quiet after a busy stretch that counts as "the answer arrived".
pub const DONE_QUIET: Duration = Duration::from_millis(1200);
/// Output must keep coming this long after Enter for a done signal: a quick shell command is
/// not an answer worth a surge (cursor-fx plays done only when a long command ends).
pub const DONE_BUSY: Duration = Duration::from_millis(1500);

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
}

impl Signals {
    fn fire(&mut self, s: Signal, now: Instant) {
        self.parity = !self.parity;
        self.on = Some((s, now));
    }

    /// Enter was pressed.
    pub fn enter(&mut self, now: Instant) {
        self.fire(Signal::Enter, now);
        self.waiting = Some((now, None));
    }

    /// The pane printed something.
    pub fn output(&mut self, now: Instant) {
        if let Some((_, last)) = self.waiting.as_mut() {
            *last = Some(now);
        }
    }

    /// A shell reported the command ended (OSC 133;D): done now, if it ran long enough.
    pub fn command_ended(&mut self, now: Instant) {
        if let Some((since, _)) = self.waiting.take()
            && now.saturating_duration_since(since) >= DONE_BUSY
        {
            self.fire(Signal::Done, now);
        }
    }

    /// Fires done once the output after Enter has gone quiet. True when it fired.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some((since, Some(last))) = self.waiting else { return false };
        if now.saturating_duration_since(last) < DONE_QUIET {
            return false;
        }
        self.waiting = None;
        if last.saturating_duration_since(since) >= DONE_BUSY {
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
    use super::*;

    #[test]
    fn manifest_reads_every_key() {
        let m = parse_manifest("name = \"Neon\"\nshaders = [\"a.glsl\", \"b.wgsl\"]\nanimation = \"always\"\ncursor_color = \"#ff00ff\"\nlicense = \"MIT\"").unwrap();
        assert_eq!(m.name, "Neon");
        assert_eq!(m.shaders, ["a.glsl", "b.wgsl"]);
        assert_eq!(m.animation, Animation::Always);
        assert_eq!(m.cursor_color.as_deref(), Some("#ff00ff"));
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
        assert_eq!(l.fields["iChannelTime"].1, 16);
        assert_eq!(l.fields["iPalette"].1, 16);
        assert_eq!(l.size % 16, 0);
        let mut u = Uniforms { time: 2.5, ..Default::default() };
        u.palette[3] = [0.25, 0.5, 0.75];
        let b = u.pack(&l);
        assert_eq!(b.len(), l.size as usize);
        assert_eq!(f32::from_le_bytes(b[12..16].try_into().unwrap()), 2.5);
        let p3 = (l.fields["iPalette"].0 + 3 * 16) as usize;
        assert_eq!(f32::from_le_bytes(b[p3 + 4..p3 + 8].try_into().unwrap()), 0.5);
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
        let tmp = std::env::temp_dir().join(format!("render_fx_test_{}", std::process::id()));
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
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].title(), list[0].builtin()), ("Mine", false));
        assert_eq!(compiled.map(|c| c.len()), Ok(1));
        assert!(escape.is_err(), "a manifest cannot reach outside its folder");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "broken");
    }

    #[test]
    fn enter_and_done_signals_follow_cursor_fx() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Signals::default();
        let base = [0x9f, 0xd0, 0xfe];
        assert_eq!(s.apply(base, t0), base, "no signal, no change");
        s.enter(t0);
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
        s.enter(t0);
        s.output(t0 + Duration::from_millis(80));
        assert!(!s.tick(t0 + Duration::from_secs(5)));
        assert_eq!(s.current(t0 + Duration::from_secs(5)), None);
        // A shell's command-ended mark after a long run is done at once.
        s.enter(t0);
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
    fn the_input_box_patch_reaches_every_builtin() {
        for p in discover(None).0 {
            let text = p.shader_text(&p.manifest.shaders[0]).unwrap();
            let patched = patch_input_box(&text);
            assert_eq!(text.lines().count(), patched.lines().count(), "{}: line numbers stay", p.id);
            assert!(patched.contains("iInputBox.z > 0.5) { float tA"), "{}: boxLineY answers from the grid", p.id);
            assert!(!bounds_re().is_match(&patched), "{}: every 4-row bound reaches the box top", p.id);
            assert!(patched.matches("termurionAbove(").count() >= 1, "{}", p.id);
        }
        // A shader without the functions is untouched.
        let plain = "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }";
        assert_eq!(patch_input_box(plain), plain);
    }

    #[test]
    fn hex_colors_parse() {
        assert_eq!(hex_color("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(hex_color("ff8000"), None);
    }
}
