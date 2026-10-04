//! Window arrangements in iTerm2's form: windows, tabs with a tree of "Splitter" and
//! "SessionView" nodes (pixel frames, n-ary splitters), and sessions. Tandem's layouts are
//! binary; `node` and `tree_of` convert.


pub use toml::{Table, Value};

use crate::layout::{Axis, Tree};
use crate::palette::Rgb;

/// iTerm2's frame dictionary.
pub fn frame(x: f64, y: f64, w: f64, h: f64) -> Value {
    let mut t = Table::new();
    for (k, v) in [("x", x), ("y", y), ("width", w), ("height", h)] {
        t.insert(k.into(), Value::Float(v));
    }
    Value::Table(t)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)))
}

fn frame_size(v: &Value) -> (f64, f64) {
    let f = v.get("frame");
    (num(f.and_then(|f| f.get("width"))).unwrap_or(0.0), num(f.and_then(|f| f.get("height"))).unwrap_or(0.0))
}

/// The children of a run of same-axis splits, with their share of the whole.
fn flatten<'a>(t: &'a Tree<Value>, axis: Axis, share: f64, out: &mut Vec<(&'a Tree<Value>, f64)>) {
    match t {
        Tree::Split { axis: a2, ratio, a, b } if *a2 == axis => {
            flatten(a, axis, share * *ratio as f64, out);
            flatten(b, axis, share * (1.0 - *ratio as f64), out);
        }
        other => out.push((other, share)),
    }
}

fn node_at(t: &Tree<Value>, x: f64, y: f64, w: f64, h: f64, active: Option<usize>, index: &mut usize) -> Value {
    let mut out = Table::new();
    match t {
        Tree::Leaf(session) => {
            out.insert("View Type".into(), Value::String("SessionView".into()));
            out.insert("frame".into(), frame(x, y, w, h));
            out.insert("Session".into(), session.clone());
            // iTerm2 marks the active session on its view.
            if active == Some(*index) {
                out.insert("Is Active".into(), Value::Boolean(true));
            }
            *index += 1;
        }
        Tree::Split { axis, .. } => {
            let mut kids = Vec::new();
            flatten(t, *axis, 1.0, &mut kids);
            let vertical = *axis == Axis::Row;
            let mut offset = 0.0;
            let subviews = kids
                .into_iter()
                .map(|(k, share)| {
                    let v = if vertical {
                        let cw = (w * share).round();
                        let n = node_at(k, offset, 0.0, cw, h, active, index);
                        offset += cw;
                        n
                    } else {
                        let ch = (h * share).round();
                        let n = node_at(k, 0.0, offset, w, ch, active, index);
                        offset += ch;
                        n
                    };
                    v
                })
                .collect();
            out.insert("View Type".into(), Value::String("Splitter".into()));
            out.insert("isVertical".into(), Value::Boolean(vertical));
            out.insert("frame".into(), frame(x, y, w, h));
            out.insert("Subviews".into(), Value::Array(subviews));
        }
    }
    Value::Table(out)
}

/// A tab's `Root`: iTerm2's node tree for a layout whose leaves are session tables, laid
/// out in `w` by `h` pixels. Runs of same-axis splits become one n-ary splitter.
pub fn node(t: &Tree<Value>, w: f64, h: f64) -> Value {
    node_with_active(t, w, h, None)
}

/// `node`, with the `active`-th session (in reading order) marked active.
pub fn node_with_active(t: &Tree<Value>, w: f64, h: f64, active: Option<usize>) -> Value {
    node_at(t, 0.0, 0.0, w, h, active, &mut 0)
}

/// The active session's place in reading order.
pub fn active_leaf(root: &Value) -> Option<usize> {
    fn walk(v: &Value, i: &mut usize, depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        match v.get("View Type").and_then(|t| t.as_str()) {
            Some("SessionView") => {
                let here = *i;
                *i += 1;
                v.get("Is Active").and_then(|b| b.as_bool()).unwrap_or(false).then_some(here)
            }
            _ => v.get("Subviews").and_then(|s| s.as_array())?.iter().find_map(|s| walk(s, i, depth + 1)),
        }
    }
    walk(root, &mut 0, 0)
}

/// Splits and panes an arrangement may hold: far past any real tab, short of the stack.
const MAX_DEPTH: usize = 32;
const MAX_PANES: usize = 64;

/// Saved text as plain text: control characters (escape sequences' starts) go, so restored
/// contents cannot act on the terminal.
pub fn inert_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || matches!(c, '\r' | '\n' | '\t')).collect()
}

/// A folder as saved: a local folder reported by OSC 7 ("/C:/x") as a Windows path; other
/// paths as they are.
pub fn saved_folder(cwd: &str) -> String {
    crate::profile::local_path(cwd).map_or_else(|| cwd.to_string(), |p| p.display().to_string())
}

/// The layout a `Root` describes: n-ary splitters become nested binary splits, each share
/// from the subviews' frames. The leaves are the session tables. Too deep or too many panes
/// is refused.
pub fn tree_of(v: &Value) -> Result<Tree<Value>, String> {
    let mut panes = 0;
    tree_at(v, 0, &mut panes)
}

fn tree_at(v: &Value, depth: usize, panes: &mut usize) -> Result<Tree<Value>, String> {
    if depth > MAX_DEPTH {
        return Err("the arrangement's splits are nested too deeply".into());
    }
    match v.get("View Type").and_then(|t| t.as_str()) {
        Some("SessionView") => {
            *panes += 1;
            if *panes > MAX_PANES {
                return Err(format!("a tab in the arrangement has more than {MAX_PANES} panes"));
            }
            v.get("Session").cloned().map(Tree::Leaf).ok_or_else(|| "a pane in the arrangement has no session".to_string())
        }
        Some("Splitter") => {
            let vertical = v.get("isVertical").and_then(|b| b.as_bool()).unwrap_or(false);
            let axis = if vertical { Axis::Row } else { Axis::Column };
            let subs = v.get("Subviews").and_then(|s| s.as_array()).ok_or_else(|| "a split in the arrangement has no panes".to_string())?;
            if subs.len() > MAX_PANES {
                return Err(format!("a tab in the arrangement has more than {MAX_PANES} panes"));
            }
            let mut kids: Vec<(Tree<Value>, f64)> = Vec::new();
            for s in subs {
                let (w, h) = frame_size(s);
                let size = if vertical { w } else { h };
                kids.push((tree_at(s, depth + 1, panes)?, if size.is_finite() { size.max(1.0) } else { 1.0 }));
            }
            // Folded from the right: the first child and the rest, without recursion.
            let mut it = kids.into_iter().rev();
            let (mut acc, mut acc_size) = it.next().ok_or_else(|| "a split in the arrangement has no panes".to_string())?;
            for (t, size) in it {
                let total = size + acc_size;
                acc = Tree::Split { axis, ratio: (size / total) as f32, a: Box::new(t), b: Box::new(acc) };
                acc_size = total;
            }
            Ok(acc)
        }
        _ => Err("not an arrangement pane or split".to_string()),
    }
}

/// A session table: its size, its profile (the whole profile, as iTerm2 keeps it), its id,
/// and its folder.
pub fn session(cols: u16, rows: u16, profile: Table, guid: &str, cwd: Option<&str>) -> Value {
    let mut t = Table::new();
    t.insert("Columns".into(), Value::Integer(cols as i64));
    t.insert("Rows".into(), Value::Integer(rows as i64));
    t.insert("Bookmark".into(), Value::Table(profile));
    t.insert("Session GUID".into(), Value::String(guid.into()));
    if let Some(c) = cwd {
        t.insert("Working Directory".into(), Value::String(c.into()));
    }
    Value::Table(t)
}

/// A session's profile: its Guid, its Name, and the whole table.
pub fn profile_of(session: &Value) -> Option<(String, String, &Table)> {
    let b = session.get("Bookmark")?.as_table()?;
    let s = |k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some((s("Guid"), s("Name"), b))
}


pub(crate) fn to_plist_value(v: &Value) -> plist::Value {
    match v {
        Value::String(s) => plist::Value::String(s.clone()),
        Value::Integer(i) => plist::Value::Integer((*i).into()),
        Value::Float(f) => plist::Value::Real(*f),
        Value::Boolean(b) => plist::Value::Boolean(*b),
        Value::Datetime(d) => plist::Value::String(d.to_string()),
        Value::Array(a) => plist::Value::Array(a.iter().map(to_plist_value).collect()),
        Value::Table(t) => plist::Value::Dictionary(t.iter().map(|(k, v)| (k.clone(), to_plist_value(v))).collect()),
    }
}

/// An arrangement file: an XML plist array of windows (iTerm2's form).
pub fn to_plist(windows: &[Value]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    plist::Value::Array(windows.iter().map(to_plist_value).collect()).to_writer_xml(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// A tab's pinned flag (`TAB:99`).
pub const PINNED: &str = "Pinned";
/// A tab's group definition (`TAB:100-103`); only a grouped tab writes them.
pub const GROUP_ID: &str = "Tab Group ID";
pub const GROUP_NAME: &str = "Tab Group Name";
pub const GROUP_COLOR: &str = "Tab Group Color";
pub const GROUP_COLLAPSED: &str = "Tab Group Collapsed";

/// A tab table: its `Root`, whether it is the selected tab, its Edit Tab Title text, and
/// whether it is pinned and its group (`TAB:3973-3984`).
pub fn tab(root: Value, active: bool, title: Option<&str>, pinned: bool, group: Option<&crate::tab_groups::TabGroup>) -> Value {
    let mut t = Table::new();
    t.insert("Root".into(), root);
    t.insert("Is Active".into(), Value::Boolean(active));
    if let Some(x) = title {
        t.insert("Title Override".into(), Value::String(x.into()));
    }
    t.insert(PINNED.into(), Value::Boolean(pinned));
    if let Some(g) = group {
        t.insert(GROUP_ID.into(), Value::String(g.id.clone()));
        t.insert(GROUP_NAME.into(), Value::String(g.name.clone()));
        t.insert(GROUP_COLOR.into(), color_dict(g.color));
        t.insert(GROUP_COLLAPSED.into(), Value::Boolean(g.collapsed));
    }
    Value::Table(t)
}

/// A tab table's pinned flag and group (`TAB:3631-3638`). A group with no name reads "" and
/// one with no color the palette's blue, iTerm2's `tabGroupsInWindow` fallbacks
/// (`PT:9036-9038`); an empty id is no group.
pub fn tab_pin_group(t: &Value) -> (bool, Option<crate::tab_groups::TabGroup>) {
    let pinned = t.get(PINNED).and_then(|v| v.as_bool()).unwrap_or(false);
    let group = t.get(GROUP_ID).and_then(|v| v.as_str()).filter(|id| !id.is_empty()).map(|id| crate::tab_groups::TabGroup {
        id: id.to_string(),
        name: t.get(GROUP_NAME).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        color: t.get(GROUP_COLOR).and_then(color_of_dict).unwrap_or(crate::tab_groups::PALETTE[0]),
        collapsed: t.get(GROUP_COLLAPSED).and_then(|v| v.as_bool()).unwrap_or(false),
    });
    (pinned, group)
}

/// iTerm2's color dictionary (`dictionaryValue`, `NSColor+iTerm.m:826-838`), in sRGB.
pub fn color_dict(c: Rgb) -> Value {
    let mut t = Table::new();
    t.insert("Color Space".into(), Value::String("sRGB".into()));
    for (k, v) in [("Red Component", c.r), ("Green Component", c.g), ("Blue Component", c.b)] {
        t.insert(k.into(), Value::Float(v as f64 / 255.0));
    }
    t.insert("Alpha Component".into(), Value::Float(1.0));
    Value::Table(t)
}

/// A color dictionary's color, or None when a component is missing. Reading an arrangement
/// file turns iTerm2's dictionary into Tandem's r, g, b table (`import::from_plist`, which
/// also converts Display P3), so both forms are read.
pub fn color_of_dict(v: &Value) -> Option<Rgb> {
    let comp = |long: &str, short: &str| num(v.get(long).or_else(|| v.get(short)));
    let byte = |f: f64| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(crate::palette::rgb(byte(comp("Red Component", "r")?), byte(comp("Green Component", "g")?), byte(comp("Blue Component", "b")?)))
}

/// What a window table records.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowFacts {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub style: i64,
    pub selected_tab: usize,
    pub title: Option<String>,
    pub size_locked: bool,
    pub layout_locked: bool,
    pub percent: (f64, f64),
    pub cells: (u16, u16),
}

/// A window table in iTerm2's keys.
pub fn window(f: WindowFacts, tabs: Vec<Value>) -> Value {
    let mut t = Table::new();
    for (k, v) in [("X Origin", f.x), ("Y Origin", f.y), ("Width", f.w), ("Height", f.h)] {
        t.insert(k.into(), Value::Float(v));
    }
    t.insert("Window Type".into(), Value::Integer(f.style));
    t.insert("Selected Tab Index".into(), Value::Integer(f.selected_tab as i64));
    t.insert("Size Locked".into(), Value::Boolean(f.size_locked));
    t.insert("Layout Locked".into(), Value::Boolean(f.layout_locked));
    t.insert("Desired Screen Width Percentage".into(), Value::Float(f.percent.0));
    t.insert("Desired Screen Height Percentage".into(), Value::Float(f.percent.1));
    t.insert("Desired Columns".into(), Value::Integer(f.cells.0 as i64));
    t.insert("Desired Rows".into(), Value::Integer(f.cells.1 as i64));
    if let Some(x) = f.title {
        t.insert("Title Override".into(), Value::String(x));
    }
    t.insert("Tabs".into(), Value::Array(tabs));
    Value::Table(t)
}

/// A window table's facts, for restoring.
/// The frame a window is saved with: a minimized window (Windows parks it at -32000) has no
/// useful frame, so it is saved without one and restores where placement puts it.
pub fn saved_frame(pos: (i32, i32), size: (u32, u32), minimized: bool) -> (f64, f64, f64, f64) {
    // Windows parks a minimized window at -32000; no monitor reaches that far.
    if minimized || pos.0 <= -32000 || pos.1 <= -32000 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (pos.0 as f64, pos.1 as f64, size.0 as f64, size.1 as f64)
}

pub fn window_facts(w: &Value) -> WindowFacts {
    let f = |k: &str| num(w.get(k)).unwrap_or(0.0);
    let b = |k: &str| w.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    WindowFacts {
        x: f("X Origin"),
        y: f("Y Origin"),
        w: f("Width"),
        h: f("Height"),
        style: w.get("Window Type").and_then(|v| v.as_integer()).unwrap_or(0),
        selected_tab: w.get("Selected Tab Index").and_then(|v| v.as_integer()).unwrap_or(0).max(0) as usize,
        title: w.get("Title Override").and_then(|v| v.as_str()).map(str::to_string),
        size_locked: b("Size Locked"),
        layout_locked: b("Layout Locked"),
        percent: (num(w.get("Desired Screen Width Percentage")).unwrap_or(100.0), num(w.get("Desired Screen Height Percentage")).unwrap_or(100.0)),
        cells: (w.get("Desired Columns").and_then(|v| v.as_integer()).unwrap_or(80) as u16, w.get("Desired Rows").and_then(|v| v.as_integer()).unwrap_or(25) as u16),
    }
}

/// The date, the time to the minute, and the time to the second, for arrangement names.
pub fn date_names(t: (u16, u16, u16, u16, u16, u16)) -> (String, String, String) {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, mo, d, h, mi, s) = t;
    let month = MONTHS.get((mo as usize).saturating_sub(1)).copied().unwrap_or("");
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    (format!("{month} {d}, {y}"), format!("{h12}:{mi:02} {ampm}"), format!("{h12}:{mi:02}:{s:02} {ampm}"))
}


/// The profile a restored session starts with (`PS:2013-2029`, `PS:1960-1969`).
#[derive(Clone, Debug, PartialEq)]
pub enum ProfileChoice {
    /// The saved profile with this Guid.
    Saved(String),
    /// The arrangement's own copy, restored as a session copy.
    Embedded(Table),
    /// A missing profile whose copy has no Name or no Guid (iTerm2 3.5.7's corruption).
    Broken,
    /// No profile in the arrangement: the default one.
    Default,
}

/// The saved profile with the arrangement's Guid, else the arrangement's own copy. The
/// copy's Original Guid decides only whether the profile counts as missing, and only a
/// missing one whose copy lacks its Name or Guid is broken. `saved` is (Guid, Name) of
/// every saved profile.
pub fn choose_profile(session: &Value, saved: &[(String, String)]) -> ProfileChoice {
    let Some(copy) = session.get("Bookmark").and_then(|b| b.as_table()) else { return ProfileChoice::Default };
    let is_saved = |key: &str| copy.get(key).and_then(|v| v.as_str()).is_some_and(|g| saved.iter().any(|(s, _)| s == g));
    if is_saved("Guid") {
        return ProfileChoice::Saved(copy["Guid"].as_str().unwrap_or_default().to_string());
    }
    let missing = !is_saved(crate::session_profile::ORIGINAL_GUID);
    if missing && (copy.get("Name").is_none() || copy.get("Guid").is_none()) {
        return ProfileChoice::Broken;
    }
    ProfileChoice::Embedded(copy.clone())
}

/// A session's own id in the arrangement ("Session GUID").
pub fn session_guid(session: &Value) -> Option<String> {
    session.get("Session GUID").and_then(|v| v.as_str()).map(str::to_string)
}

/// Runs `f` on every session table in the windows.
fn each_session(windows: &mut [Value], f: &mut dyn FnMut(&mut Table)) {
    fn walk(node: &mut Value, f: &mut dyn FnMut(&mut Table)) {
        let Some(t) = node.as_table_mut() else { return };
        if let Some(Value::Table(s)) = t.get_mut("Session") {
            f(s);
        }
        if let Some(Value::Array(kids)) = t.get_mut("Subviews") {
            for k in kids {
                walk(k, f);
            }
        }
    }
    for w in windows {
        if let Some(Value::Array(tabs)) = w.as_table_mut().and_then(|t| t.get_mut("Tabs")) {
            for tab in tabs {
                if let Some(root) = tab.as_table_mut().and_then(|t| t.get_mut("Root")) {
                    walk(root, f);
                }
            }
        }
    }
}

/// The windows with `change` applied to the session whose id is `session_guid`, wherever
/// it is.
fn repaired(windows: &[Value], session_guid: &str, change: &dyn Fn(&mut Table)) -> Vec<Value> {
    let mut out = windows.to_vec();
    each_session(&mut out, &mut |s| {
        if s.get("Session GUID").and_then(|v| v.as_str()) == Some(session_guid) {
            change(s);
        }
    });
    out
}

/// Repair: the session's initial folder becomes `folder` (`iTermController.m:471-482`).
pub fn replace_folder(windows: &[Value], session_guid: &str, folder: &str) -> Vec<Value> {
    repaired(windows, session_guid, &|s| {
        s.insert("Working Directory".into(), Value::String(folder.into()));
    })
}

/// Assign Profile: the session's profile becomes `profile` (`PS:24104-24114`).
pub fn replace_profile(windows: &[Value], session_guid: &str, profile: &Table) -> Vec<Value> {
    repaired(windows, session_guid, &|s| {
        s.insert("Bookmark".into(), Value::Table(profile.clone()));
    })
}

/// A restored session's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Folder {
    /// Contents came with it: this folder, whatever the profile says.
    Forced(String),
    /// The profile's working-directory rule decides, with this as the previous folder.
    Previous(String),
    None,
}

/// A restored session's folder rule, with the folder as the Windows path it names (the OSC 7
/// form "/D:/x" is D:\x, "~" the home folder: `profile::local_folder`). Contents force only
/// such a path: another (WSL, MSYS, a remote host's) cannot be a Windows shell's folder, so
/// the profile's rule decides. An `ssh` session's "~" is the remote host's home, so it is
/// never this machine's.
pub fn folder_for(session: &Value, ssh: bool) -> Folder {
    let Some(d) = session.get("Working Directory").and_then(|v| v.as_str()) else { return Folder::None };
    let local = if ssh { crate::profile::local_path(d) } else { crate::profile::local_folder(d) };
    match local.map(|p| p.display().to_string()) {
        Some(local) if contents(session).is_some() || styled_contents(session).is_some() => Folder::Forced(local),
        Some(local) => Folder::Previous(local),
        None => Folder::Previous(d.into()),
    }
}

pub const CONTENTS_KEY: &str = "Tandem Contents";
/// Session restoration's contents: styled text (SGR sequences only).
pub const STYLED_KEY: &str = "Tandem Styled Contents";
/// Set on a session while it is restored: the banner line to draw under its contents.
pub const BANNER_KEY: &str = "Tandem Banner";
/// Session restoration: where the cursor was on the screen (`{Row, Column}`).
pub const CURSOR_KEY: &str = "Tandem Cursor";

/// A window that is a profile's hotkey window (iTerm2's keys).
pub const HOTKEY_WINDOW_KEY: &str = "Is Hotkey Window";
pub const HOTKEY_GUID_KEY: &str = "Hotkey Profile GUID";

/// The profile of a saved hotkey window.
pub fn hotkey_guid(window: &Value) -> Option<String> {
    let is = window.get(HOTKEY_WINDOW_KEY).and_then(|v| v.as_bool()).unwrap_or(false);
    is.then(|| window.get(HOTKEY_GUID_KEY).and_then(|v| v.as_str()).map(str::to_string)).flatten()
}

pub fn saved_cursor(session: &Value) -> Option<(usize, usize)> {
    let c = session.get(CURSOR_KEY)?;
    let n = |k: &str| c.get(k).and_then(|v| v.as_integer()).filter(|i| (0..100_000).contains(i)).map(|i| i as usize);
    Some((n("Row")?, n("Column")?))
}

/// Set on a session while it is restored: the host session to take over (`{Id, Pid, Since}`).
pub const ATTACH_KEY: &str = "Tandem Attach";

/// The text saved "with contents".
pub fn contents(session: &Value) -> Option<&str> {
    session.get(CONTENTS_KEY).and_then(|v| v.as_str())
}

pub fn styled_contents(session: &Value) -> Option<&str> {
    session.get(STYLED_KEY).and_then(|v| v.as_str())
}


/// An arrangement file (iTerm2's plist: an array of window dictionaries).
pub fn from_plist(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| format!("not an arrangement file: {e}"))?;
    let arr = root.as_array().ok_or("not an arrangement file: it holds no list of windows")?;
    let windows: Vec<Value> = arr.iter().filter_map(crate::import::from_plist).filter(|w| w.get("Tabs").and_then(|t| t.as_array()).is_some()).collect();
    if windows.is_empty() {
        return Err("not an arrangement file: it holds no windows".into());
    }
    Ok(windows)
}

/// How the trust question names the profile of a session with no Bookmark table.
const DEFAULT_PROFILE_NAME: &str = "Default profile";

/// The question before opening a downloaded arrangement file: what its profiles would run.
pub fn trust_text(name: &str, windows: &[Value]) -> String {
    let mut lines = vec![format!("Open the arrangement \u{201c}{name}\u{201d}?"), String::new()];
    for w in windows {
        for t in w.get("Tabs").and_then(|t| t.as_array()).into_iter().flatten() {
            let Some(tree) = t.get("Root").and_then(|r| tree_of(r).ok()) else { continue };
            for s in tree.leaves() {
                // Pending Jumps reach a remote `eval`: every hop is named, also for a session
                // with no Bookmark table, which restores with the default profile and takes its
                // hops when that profile is an SSH one (final review I3).
                let pname = profile_of(s).map_or_else(|| DEFAULT_PROFILE_NAME.to_string(), |(_, name, _)| name);
                for hop in s.get(crate::ssh_jumps::PENDING_JUMPS_KEY).map(crate::ssh_jumps::jumps_from).unwrap_or_default() {
                    lines.push(format!("Profile \u{201c}{pname}\u{201d} connects through: {}", hop.sshargs.trim_end()));
                }
                let Some((_, pname, prof)) = profile_of(s) else { continue };
                let get = |k: &str| prof.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                if get("Custom Command") == "Yes" && !get("Command").is_empty() {
                    lines.push(format!("Profile \u{201c}{pname}\u{201d} runs: {}", get("Command")));
                }
                if !get("Initial Text").is_empty() {
                    lines.push(format!("Profile \u{201c}{pname}\u{201d} types: {}", get("Initial Text")));
                }
                if prof.get("Triggers").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty()) {
                    lines.push(format!("Profile \u{201c}{pname}\u{201d} has triggers."));
                }
            }
        }
    }
    if lines.len() == 2 {
        lines.push("It came from another computer.".into());
    }
    lines.push(String::new());
    lines.push("Open it only if you trust where it came from.".into());
    lines.join("\n")
}

/// Windows marks a downloaded file with a Zone.Identifier stream (its quarantine flag).
pub fn has_mark_of_the_web(path: &std::path::Path) -> bool {
    let mut s = path.as_os_str().to_os_string();
    s.push(":Zone.Identifier");
    std::fs::metadata(std::path::PathBuf::from(s)).is_ok()
}


/// Change Profile in Arrangement: every session in every window gets this profile.
pub fn change_profile(windows: &mut [Value], profile: &Table) {
    each_session(windows, &mut |s| {
        s.insert("Bookmark".into(), Value::Table(profile.clone()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimized_windows_save_no_frame() {
        assert_eq!(saved_frame((10, 20), (800, 600), false), (10.0, 20.0, 800.0, 600.0));
        assert_eq!(saved_frame((-32000, -32000), (160, 28), true), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(saved_frame((-32000, -32000), (160, 28), false), (0.0, 0.0, 0.0, 0.0), "parked, even when the flag is not known");
    }
    use crate::layout::{Axis, Tree};

    fn s(name: &str) -> Value {
        let mut t = Table::new();
        t.insert("Name".into(), Value::String(name.into()));
        Value::Table(t)
    }

    fn leaf_names(t: &Tree<Value>) -> Vec<String> {
        match t {
            Tree::Leaf(v) => vec![v.get("Name").and_then(|n| n.as_str()).unwrap_or("").to_string()],
            Tree::Split { a, b, .. } => [leaf_names(a), leaf_names(b)].concat(),
        }
    }

    #[test]
    fn layouts_round_trip() {
        let tree = Tree::Split {
            axis: Axis::Row,
            ratio: 0.3,
            a: Box::new(Tree::Leaf(s("a"))),
            b: Box::new(Tree::Split { axis: Axis::Column, ratio: 0.5, a: Box::new(Tree::Leaf(s("b"))), b: Box::new(Tree::Leaf(s("c"))) }),
        };
        let root = node(&tree, 1000.0, 600.0);
        assert_eq!(root.get("View Type").and_then(|v| v.as_str()), Some("Splitter"));
        assert_eq!(root.get("isVertical").and_then(|v| v.as_bool()), Some(true), "side by side is a vertical splitter");
        let back = tree_of(&root).unwrap();
        assert_eq!(leaf_names(&back), vec!["a", "b", "c"]);
        let Tree::Split { axis, ratio, b, .. } = &back else { panic!("{back:?}") };
        assert_eq!(*axis, Axis::Row);
        assert!((ratio - 0.3).abs() < 0.01, "{ratio}");
        assert!(matches!(**b, Tree::Split { axis: Axis::Column, .. }));
    }

    #[test]
    fn n_ary_splitters_become_binary() {
        let view = |name: &str, w: f64, h: f64| {
            let mut t = Table::new();
            t.insert("View Type".into(), Value::String("SessionView".into()));
            t.insert("frame".into(), frame(0.0, 0.0, w, h));
            t.insert("Session".into(), s(name));
            Value::Table(t)
        };
        let mut inner = Table::new();
        inner.insert("View Type".into(), Value::String("Splitter".into()));
        inner.insert("isVertical".into(), Value::Boolean(false));
        inner.insert("frame".into(), frame(0.0, 0.0, 400.0, 600.0));
        inner.insert("Subviews".into(), Value::Array(vec![view("c", 400.0, 200.0), view("d", 400.0, 400.0)]));
        let mut root = Table::new();
        root.insert("View Type".into(), Value::String("Splitter".into()));
        root.insert("isVertical".into(), Value::Boolean(true));
        root.insert("Subviews".into(), Value::Array(vec![view("a", 300.0, 600.0), view("b", 300.0, 600.0), Value::Table(inner)]));
        let t = tree_of(&Value::Table(root)).unwrap();
        assert_eq!(leaf_names(&t), vec!["a", "b", "c", "d"]);
        let Tree::Split { axis: Axis::Row, ratio, b, .. } = &t else { panic!() };
        assert!((ratio - 0.3).abs() < 0.01, "a takes 300 of 1000: {ratio}");
        let Tree::Split { axis: Axis::Row, ratio: r2, b: rest, .. } = &**b else { panic!() };
        assert!((r2 - 300.0 / 700.0).abs() < 0.01, "{r2}");
        let Tree::Split { axis: Axis::Column, ratio: r3, .. } = &**rest else { panic!() };
        assert!((r3 - 1.0 / 3.0).abs() < 0.01, "{r3}");
        assert!(tree_of(&Value::String("x".into())).is_err());
    }

    #[test]
    fn sessions_carry_profile_and_folder() {
        let mut profile = Table::new();
        profile.insert("Guid".into(), Value::String("G".into()));
        profile.insert("Name".into(), Value::String("Build".into()));
        let v = session(80, 24, profile, "S1", Some(r"C:\src"));
        assert_eq!(v.get("Columns").and_then(|x| x.as_integer()), Some(80));
        assert_eq!(v.get("Rows").and_then(|x| x.as_integer()), Some(24));
        assert_eq!(v.get("Session GUID").and_then(|x| x.as_str()), Some("S1"));
        assert_eq!(v.get("Working Directory").and_then(|x| x.as_str()), Some(r"C:\src"));
        assert_eq!(profile_of(&v).map(|p| (p.0, p.1)), Some(("G".to_string(), "Build".to_string())));
    }

    #[test]
    fn snapshot_keys_follow_iterm2() {
        let root = node(&Tree::Leaf(s("a")), 800.0, 600.0);
        let t = tab(root, true, Some("build"), false, None);
        assert_eq!(t.get("Is Active").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(t.get("Title Override").and_then(|v| v.as_str()), Some("build"));
        assert!(t.get("Root").is_some());
        let w = window(WindowFacts { x: 10.0, y: 20.0, w: 800.0, h: 600.0, style: 0, selected_tab: 1, title: None, size_locked: true, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) }, vec![t.clone(), t]);
        for (k, v) in [("X Origin", 10.0), ("Y Origin", 20.0), ("Width", 800.0), ("Height", 600.0)] {
            assert_eq!(w.get(k).and_then(|x| x.as_float()), Some(v), "{k}");
        }
        assert_eq!(w.get("Window Type").and_then(|v| v.as_integer()), Some(0));
        assert_eq!(w.get("Selected Tab Index").and_then(|v| v.as_integer()), Some(1));
        assert_eq!(w.get("Size Locked").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(w.get("Tabs").and_then(|v| v.as_array()).map(Vec::len), Some(2));
        assert_eq!(date_names((2026, 9, 27, 14, 5, 33)), ("Sep 27, 2026".to_string(), "2:05 PM".to_string(), "2:05:33 PM".to_string()));
    }

    fn profile(pairs: &[(&str, &str)]) -> Table {
        pairs.iter().map(|(k, v)| (k.to_string(), Value::String(v.to_string()))).collect()
    }

    /// Review Focus 6 (R3; `PS:2013-2029`, `PS:1960-1969`).
    #[test]
    fn arrangement_profile_follows_iterm2() {
        let saved = vec![("G1".to_string(), "Default".to_string()), ("G2".to_string(), "Build".to_string())];
        let with = |t: Table| session(80, 24, t, "S1", None);
        assert_eq!(choose_profile(&with(profile(&[("Guid", "G2"), ("Name", "Build")])), &saved), ProfileChoice::Saved("G2".into()));
        let copy = profile(&[("Guid", "SESSION"), ("Original Guid", "G2"), ("Name", "Build")]);
        assert_eq!(choose_profile(&with(copy.clone()), &saved), ProfileChoice::Embedded(copy), "the Original Guid only decides whether it counts as missing");
        let whole = profile(&[("Guid", "gone"), ("Name", "Elsewhere"), ("Command", "cmd")]);
        assert_eq!(choose_profile(&with(whole.clone()), &saved), ProfileChoice::Embedded(whole), "a missing profile restores from the arrangement's own copy");
        assert_eq!(choose_profile(&with(profile(&[("Guid", "gone")])), &saved), ProfileChoice::Broken, "no Name");
        assert_eq!(choose_profile(&with(profile(&[("Name", "Build")])), &saved), ProfileChoice::Broken, "no Guid");
        assert_eq!(choose_profile(&with(Table::new()), &saved), ProfileChoice::Broken, "an empty copy");
        let mut bare = Table::new();
        bare.insert("Session GUID".into(), Value::String("S1".into()));
        assert_eq!(choose_profile(&Value::Table(bare), &saved), ProfileChoice::Default, "no Bookmark");
        let named = profile(&[("Guid", "other"), ("Name", "Build")]);
        assert_eq!(choose_profile(&with(named.clone()), &saved), ProfileChoice::Embedded(named), "a saved profile that only shares the Name is not used");
        assert_eq!(session_guid(&with(Table::new())), Some("S1".to_string()));
        assert_eq!(session_guid(&Value::Table(Table::new())), None);
    }

    /// Two windows: S1 beside S2, then S1 alone.
    fn two_windows() -> Vec<Value> {
        let facts = WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) };
        let s = |g: &str| session(80, 24, profile(&[("Guid", "P"), ("Name", "Plain")]), g, Some(r"C:\old"));
        let split = Tree::Split { axis: Axis::Column, ratio: 0.5, a: Box::new(Tree::Leaf(s("S1"))), b: Box::new(Tree::Leaf(s("S2"))) };
        vec![window(facts.clone(), vec![tab(node(&split, 800.0, 600.0), true, None, false, None)]), window(facts, vec![tab(node(&Tree::Leaf(s("S1")), 800.0, 600.0), true, None, false, None)])]
    }

    fn sessions_of(windows: &[Value]) -> Vec<Value> {
        windows.iter().flat_map(|w| w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default()).flat_map(|t| tree_of(t.get("Root").unwrap()).unwrap().leaves().into_iter().cloned().collect::<Vec<_>>()).collect()
    }

    /// Repair rewrites only that session (`iTermController.m:438-482`).
    #[test]
    fn repairs_follow_iterm2() {
        let windows = two_windows();
        let folder = |s: &Value| s.get("Working Directory").and_then(|v| v.as_str()).map(str::to_string);
        let fixed = replace_folder(&windows, "S1", r"D:\w");
        let got: Vec<(Option<String>, Option<String>)> = sessions_of(&fixed).iter().map(|s| (session_guid(s), folder(s))).collect();
        assert_eq!(got, vec![(Some("S1".into()), Some(r"D:\w".into())), (Some("S2".into()), Some(r"C:\old".into())), (Some("S1".into()), Some(r"D:\w".into()))]);
        let good = profile(&[("Guid", "G2"), ("Name", "Build")]);
        let fixed = replace_profile(&windows, "S1", &good);
        let got: Vec<Option<String>> = sessions_of(&fixed).iter().map(|s| profile_of(s).map(|p| p.0)).collect();
        assert_eq!(got, vec![Some("G2".into()), Some("P".into()), Some("G2".into())]);
        assert_eq!(sessions_of(&fixed).iter().map(folder).collect::<Vec<_>>(), vec![Some(r"C:\old".into()); 3], "only the profile changes");
        assert_eq!(replace_folder(&windows, "nobody", r"D:\w"), windows, "an unknown session changes nothing");
        assert_eq!(replace_profile(&windows, "nobody", &good), windows);
    }

    #[test]
    fn folders_follow_the_profile_rule() {
        let plain = session(80, 24, Table::new(), "s", Some(r"C:\src"));
        assert_eq!(folder_for(&plain, false), Folder::Previous(r"C:\src".into()), "the profile's rule decides, with this as the previous folder");
        let mut with = plain.clone();
        with.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&with, false), Folder::Forced(r"C:\src".into()), "contents force the folder");
        assert_eq!(folder_for(&session(80, 24, Table::new(), "s", None), false), Folder::None);
        // A folder that is not a Windows path (WSL, MSYS, a remote host's) cannot be a Windows
        // shell's folder: contents never force it, so no "does not exist" banner follows.
        let mut unix = session(80, 24, Table::new(), "s", Some("/home/rob/src"));
        unix.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&unix, false), Folder::Previous("/home/rob/src".into()));
        // The OSC 7 form and "~" are forced as the Windows paths they name, so an existing one
        // is not reported as missing.
        let mut osc7 = session(80, 24, Table::new(), "s", Some("/D:/x"));
        osc7.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&osc7, false), Folder::Forced(r"D:\x".into()));
        let mut tilde = session(80, 24, Table::new(), "s", Some("~/www"));
        tilde.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&tilde, false), Folder::Forced(crate::profile::home().join("www").display().to_string()));
        assert_eq!(folder_for(&session(80, 24, Table::new(), "s", Some("/D:/x")), false), Folder::Previous(r"D:\x".into()));
        assert_eq!(contents(&with), Some("x"));
    }

    /// An ssh session's "~" is the remote host's home: never this machine's, so it is not
    /// forced (and checked) here.
    #[test]
    fn an_ssh_sessions_home_is_remote() {
        let mut tilde = session(80, 24, Table::new(), "s", Some("~/www"));
        tilde.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&tilde, true), Folder::Previous("~/www".into()));
        assert_eq!(folder_for(&session(80, 24, Table::new(), "s", Some("~")), true), Folder::Previous("~".into()));
        // A drive-letter folder is still this machine's.
        let mut local = session(80, 24, Table::new(), "s", Some(r"C:\src"));
        local.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&local, true), Folder::Forced(r"C:\src".into()));
    }

    fn one_window(profile: Table) -> Vec<Value> {
        let tree = Tree::Leaf(session(80, 24, profile, "s", Some("C:\\")));
        vec![window(WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) }, vec![tab(node(&tree, 800.0, 600.0), true, None, false, None)])]
    }

    #[test]
    fn plist_round_trip() {
        let w = one_window(Table::new());
        let bytes = to_plist(&w).unwrap();
        let back = from_plist(&bytes).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(window_facts(&back[0]).cells, (80, 25));
        let root = back[0].get("Tabs").and_then(|t| t.as_array()).and_then(|t| t[0].get("Root")).unwrap();
        assert!(tree_of(root).is_ok());
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(from_plist(b"not a plist").is_err());
        let mut dict = Vec::new();
        plist::Value::Dictionary(Default::default()).to_writer_xml(&mut dict).unwrap();
        assert!(from_plist(&dict).is_err(), "a dictionary is not an arrangement");
        let mut empty = Vec::new();
        plist::Value::Array(vec![plist::Value::String("x".into())]).to_writer_xml(&mut empty).unwrap();
        assert!(from_plist(&empty).is_err(), "an array without windows");
    }

    #[test]
    fn trust_text_lists_commands() {
        let mut p = Table::new();
        p.insert("Name".into(), Value::String("Ops".into()));
        p.insert("Custom Command".into(), Value::String("Yes".into()));
        p.insert("Command".into(), Value::String("ssh prod".into()));
        p.insert("Initial Text".into(), Value::String("rm -rf /tmp/x".into()));
        let t = trust_text("prod.itermarrangement", &one_window(p));
        assert!(t.starts_with("Open the arrangement \u{201c}prod.itermarrangement\u{201d}?"), "{t}");
        assert!(t.contains("ssh prod") && t.contains("rm -rf /tmp/x") && t.contains("Ops"), "{t}");
        assert!(t.ends_with("Open it only if you trust where it came from."));
    }

    /// Pending Jumps are text an arrangement file sends to a remote `eval`, so the question
    /// names every hop before the file is trusted.
    #[test]
    fn trust_text_lists_pending_jumps() {
        let mut p = Table::new();
        p.insert("Name".into(), Value::String("Hosts".into()));
        p.insert("Custom Command".into(), Value::String("SSH".into()));
        let mut leaf = session(80, 24, p, "s", None);
        let hops = [("-p 22 bastion.lan ", Some("/srv")), ("evil.example ", None)].map(|(a, d)| crate::ssh_jumps::Reconnection { sshargs: a.into(), initial_directory: d.map(String::from), boolargs: "46".into() });
        if let Value::Table(t) = &mut leaf {
            t.insert(crate::ssh_jumps::PENDING_JUMPS_KEY.into(), crate::ssh_jumps::jumps_value(&hops));
        }
        let tree = Tree::Leaf(leaf);
        let windows = vec![window(WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) }, vec![tab(node(&tree, 800.0, 600.0), true, None, false, None)])];
        let t = trust_text("hops.itermarrangement", &windows);
        assert!(t.contains("Profile \u{201c}Hosts\u{201d} connects through: -p 22 bastion.lan"), "{t}");
        assert!(t.contains("Profile \u{201c}Hosts\u{201d} connects through: evil.example"), "{t}");
        assert!(!t.contains("It came from another computer."), "the hops are what it runs: {t}");
    }

    /// Final review I3: a session with no Bookmark table restores with the default profile,
    /// which may be an SSH one that takes its hops, so the question names them too.
    #[test]
    fn trust_text_names_hops_without_a_bookmark() {
        let mut leaf = session(80, 24, Table::new(), "s", None);
        let hops = [crate::ssh_jumps::Reconnection { sshargs: "-p 22 bastion.lan ".into(), initial_directory: None, boolargs: "46".into() }];
        if let Value::Table(t) = &mut leaf {
            t.remove("Bookmark");
            t.insert(crate::ssh_jumps::PENDING_JUMPS_KEY.into(), crate::ssh_jumps::jumps_value(&hops));
        }
        assert!(profile_of(&leaf).is_none());
        let tree = Tree::Leaf(leaf);
        let windows = vec![window(WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) }, vec![tab(node(&tree, 800.0, 600.0), true, None, false, None)])];
        let t = trust_text("hops.itermarrangement", &windows);
        assert!(t.contains("connects through: -p 22 bastion.lan"), "{t}");
        assert!(!t.contains("It came from another computer."), "{t}");
    }

    #[test]
    fn change_profile_rewrites_every_session() {
        let mut windows = one_window(Table::new());
        windows.extend(one_window(Table::new()));
        let mut p = Table::new();
        p.insert("Guid".into(), Value::String("NEW".into()));
        p.insert("Name".into(), Value::String("Fresh".into()));
        change_profile(&mut windows, &p);
        for w in &windows {
            let root = w.get("Tabs").and_then(|t| t.as_array()).and_then(|t| t[0].get("Root")).unwrap();
            for s in tree_of(root).unwrap().leaves() {
                assert_eq!(profile_of(s).map(|x| x.0), Some("NEW".to_string()));
            }
        }
    }

    fn view(name: &str) -> Value {
        let mut t = Table::new();
        t.insert("View Type".into(), Value::String("SessionView".into()));
        t.insert("frame".into(), frame(0.0, 0.0, 10.0, 10.0));
        t.insert("Session".into(), s(name));
        Value::Table(t)
    }

    #[test]
    fn a_wide_splitter_does_not_overflow() {
        let mut root = Table::new();
        root.insert("View Type".into(), Value::String("Splitter".into()));
        root.insert("isVertical".into(), Value::Boolean(true));
        root.insert("Subviews".into(), Value::Array((0..10_000).map(|i| view(&i.to_string())).collect()));
        assert!(tree_of(&Value::Table(root)).is_err(), "more panes than a tab can hold is refused, without recursing 10k deep");
    }

    #[test]
    fn deep_nesting_is_refused() {
        let mut v = view("x");
        for _ in 0..200 {
            let mut t = Table::new();
            t.insert("View Type".into(), Value::String("Splitter".into()));
            t.insert("isVertical".into(), Value::Boolean(false));
            t.insert("Subviews".into(), Value::Array(vec![v, view("y")]));
            v = Value::Table(t);
        }
        assert!(tree_of(&v).is_err());
    }

    #[test]
    fn restored_text_is_inert() {
        assert_eq!(inert_text("ok\x1b]52;c;SGVsbG8=\x07 done\r\nnext\ttab\x05"), "ok]52;c;SGVsbG8= done\r\nnext\ttab");
    }

    #[test]
    fn osc7_folders_are_saved_as_windows_paths() {
        assert_eq!(saved_folder("/C:/Users/me"), r"C:\Users\me");
        assert_eq!(saved_folder(r"C:\src"), r"C:\src");
        assert_eq!(saved_folder("/home/me"), "/home/me", "a remote POSIX path is kept");
    }

    #[test]
    fn the_active_pane_is_marked_on_its_view() {
        let tree = Tree::Split { axis: Axis::Row, ratio: 0.5, a: Box::new(Tree::Leaf(s("a"))), b: Box::new(Tree::Leaf(s("b"))) };
        let root = node_with_active(&tree, 100.0, 100.0, Some(1));
        assert_eq!(active_leaf(&root), Some(1));
        let subs = root.get("Subviews").and_then(|v| v.as_array()).unwrap();
        assert_eq!(subs[1].get("Is Active").and_then(|v| v.as_bool()), Some(true), "iTerm2 marks the SessionView");
    }

    #[test]
    fn tab_keeps_pin_and_group() {
        use crate::tab_groups::{TabGroup, PALETTE};
        let root = || node(&Tree::Leaf(s("a")), 800.0, 600.0);
        let g = TabGroup { id: "G-1".into(), name: "Build".into(), color: PALETTE[0], collapsed: true };
        let t = tab(root(), true, None, true, Some(&g));
        assert_eq!(t.get(PINNED).and_then(|v| v.as_bool()), Some(true));
        assert_eq!(t.get(GROUP_ID).and_then(|v| v.as_str()), Some("G-1"));
        assert_eq!(t.get(GROUP_NAME).and_then(|v| v.as_str()), Some("Build"));
        assert_eq!(t.get(GROUP_COLLAPSED).and_then(|v| v.as_bool()), Some(true));
        let c = t.get(GROUP_COLOR).expect("a color dictionary");
        let comp = |k: &str| c.get(k).and_then(|v| v.as_float());
        assert_eq!(comp("Red Component"), Some(0.0));
        assert_eq!(comp("Green Component"), Some(122.0 / 255.0));
        assert_eq!(comp("Blue Component"), Some(1.0));
        assert_eq!(comp("Alpha Component"), Some(1.0));
        assert_eq!(c.get("Color Space").and_then(|v| v.as_str()), Some("sRGB"));
        assert_eq!(tab_pin_group(&t), (true, Some(g.clone())));
        // An arrangement file holds it as a plist; reading one converts color dictionaries.
        let w = window(WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25) }, vec![t]);
        let back = from_plist(&to_plist(&[w]).unwrap()).unwrap();
        let t2 = &back[0].get("Tabs").and_then(|v| v.as_array()).unwrap()[0];
        assert_eq!(tab_pin_group(t2), (true, Some(g)), "through an arrangement file");

        let plain = tab(root(), false, None, false, None);
        assert_eq!(plain.get(PINNED).and_then(|v| v.as_bool()), Some(false));
        for k in [GROUP_ID, GROUP_NAME, GROUP_COLOR, GROUP_COLLAPSED] {
            assert!(plain.get(k).is_none(), "{k}");
        }
        assert_eq!(tab_pin_group(&plain), (false, None));

        // iTerm2's `tabGroupsInWindow` fallback: no name reads "", no color systemBlueColor.
        let mut bare = Table::new();
        bare.insert(GROUP_ID.into(), Value::String("G-2".into()));
        let want = TabGroup { id: "G-2".into(), name: String::new(), color: PALETTE[0], collapsed: false };
        assert_eq!(tab_pin_group(&Value::Table(bare)), (false, Some(want)));
        assert_eq!(color_of_dict(&color_dict(crate::palette::rgb(0x50, 0xaf, 0xbc))), Some(crate::palette::rgb(0x50, 0xaf, 0xbc)));
    }
}
