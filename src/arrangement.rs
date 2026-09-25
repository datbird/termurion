//! Window arrangements in iTerm2's form: windows, tabs with a tree of "Splitter" and
//! "SessionView" nodes (pixel frames, n-ary splitters), and sessions. Tandem's layouts are
//! binary; `node` and `tree_of` convert.


pub use toml::{Table, Value};

use crate::layout::{Axis, Tree};

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

fn node_at(t: &Tree<Value>, x: f64, y: f64, w: f64, h: f64) -> Value {
    let mut out = Table::new();
    match t {
        Tree::Leaf(session) => {
            out.insert("View Type".into(), Value::String("SessionView".into()));
            out.insert("frame".into(), frame(x, y, w, h));
            out.insert("Session".into(), session.clone());
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
                        let n = node_at(k, offset, 0.0, cw, h);
                        offset += cw;
                        n
                    } else {
                        let ch = (h * share).round();
                        let n = node_at(k, 0.0, offset, w, ch);
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
    node_at(t, 0.0, 0.0, w, h)
}

/// The layout a `Root` describes: n-ary splitters become nested binary splits, each share
/// from the subviews' frames. The leaves are the session tables.
pub fn tree_of(v: &Value) -> Result<Tree<Value>, String> {
    match v.get("View Type").and_then(|t| t.as_str()) {
        Some("SessionView") => v.get("Session").cloned().map(Tree::Leaf).ok_or_else(|| "a pane in the arrangement has no session".to_string()),
        Some("Splitter") => {
            let vertical = v.get("isVertical").and_then(|b| b.as_bool()).unwrap_or(false);
            let axis = if vertical { Axis::Row } else { Axis::Column };
            let subs = v.get("Subviews").and_then(|s| s.as_array()).ok_or_else(|| "a split in the arrangement has no panes".to_string())?;
            let mut kids: Vec<(Tree<Value>, f64)> = Vec::new();
            for s in subs {
                let (w, h) = frame_size(s);
                kids.push((tree_of(s)?, if vertical { w } else { h }.max(1.0)));
            }
            fn join(mut kids: Vec<(Tree<Value>, f64)>, axis: Axis) -> Option<Tree<Value>> {
                if kids.len() <= 1 {
                    return kids.pop().map(|k| k.0);
                }
                let total: f64 = kids.iter().map(|k| k.1).sum();
                let (first, size) = kids.remove(0);
                let rest = join(kids, axis)?;
                Some(Tree::Split { axis, ratio: (size / total) as f32, a: Box::new(first), b: Box::new(rest) })
            }
            join(kids, axis).ok_or_else(|| "a split in the arrangement has no panes".to_string())
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


fn to_plist_value(v: &Value) -> plist::Value {
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

/// A tab table: its `Root`, whether it is the selected tab, and its Edit Tab Title text.
pub fn tab(root: Value, active: bool, title: Option<&str>) -> Value {
    let mut t = Table::new();
    t.insert("Root".into(), root);
    t.insert("Is Active".into(), Value::Boolean(active));
    if let Some(x) = title {
        t.insert("Title Override".into(), Value::String(x.into()));
    }
    Value::Table(t)
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


/// The profile to start a session with: by Guid, then by Name. None: the default profile
/// (the caller says so).
pub fn resolve_profile(session: &Value, profiles: &[(String, String)]) -> Option<String> {
    let (guid, name, _) = profile_of(session)?;
    profiles.iter().find(|(g, _)| *g == guid && !guid.is_empty()).or_else(|| profiles.iter().find(|(_, n)| *n == name && !name.is_empty())).map(|(g, _)| g.clone())
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

pub fn folder_for(session: &Value) -> Folder {
    match session.get("Working Directory").and_then(|v| v.as_str()) {
        Some(d) if contents(session).is_some() => Folder::Forced(d.into()),
        Some(d) => Folder::Previous(d.into()),
        None => Folder::None,
    }
}

/// The text saved "with contents".
pub fn contents(session: &Value) -> Option<&str> {
    session.get("Tandem Contents").and_then(|v| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let t = tab(root, true, Some("build"));
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

    #[test]
    fn missing_profiles_fall_back() {
        let mut p = Table::new();
        p.insert("Guid".into(), Value::String("gone".into()));
        p.insert("Name".into(), Value::String("Build".into()));
        let v = session(80, 24, p, "s", None);
        let profiles = vec![("G1".to_string(), "Default".to_string()), ("G2".to_string(), "Build".to_string())];
        assert_eq!(resolve_profile(&v, &profiles), Some("G2".to_string()), "a profile found by name when its Guid is gone");
        assert_eq!(resolve_profile(&v, &profiles[..1]), None, "neither: the caller uses the default profile and says so");
        let mut q = Table::new();
        q.insert("Guid".into(), Value::String("G1".into()));
        assert_eq!(resolve_profile(&session(1, 1, q, "t", None), &profiles), Some("G1".to_string()));
    }

    #[test]
    fn folders_follow_the_profile_rule() {
        let plain = session(80, 24, Table::new(), "s", Some(r"C:\src"));
        assert_eq!(folder_for(&plain), Folder::Previous(r"C:\src".into()), "the profile's rule decides, with this as the previous folder");
        let mut with = plain.clone();
        with.as_table_mut().unwrap().insert("Tandem Contents".into(), Value::String("x".into()));
        assert_eq!(folder_for(&with), Folder::Forced(r"C:\src".into()), "contents force the folder");
        assert_eq!(folder_for(&session(80, 24, Table::new(), "s", None)), Folder::None);
        assert_eq!(contents(&with), Some("x"));
    }
}
