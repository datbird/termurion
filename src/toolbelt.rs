//! The toolbelt: iTerm2's panel of native tools at the right of a terminal window (View >
//! Toolbelt, Shift+Cmd+B). This module is the tool registry, the global tool list, the width
//! clamp, window growth on show and hide, and the arrangement keys. Split view math lives in
//! `toolbelt_layout`, the shared list widget in `tool_list`, each tool's rows in
//! `toolbelt_tools`; the app side wires them together in `src/app/toolbelt_ui.rs`.

use toml::{Table, Value};

/// The seven tools 9j shows. Names double as identifiers (iTerm2's `TBV:33-55,95-106`); the
/// English display name equals the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolId {
    Actions,
    CommandHistory,
    NamedMarks,
    PasteHistory,
    Profiles,
    RecentDirectories,
    Snippets,
}

impl ToolId {
    pub const ALL: [ToolId; 7] = [
        ToolId::Actions,
        ToolId::CommandHistory,
        ToolId::NamedMarks,
        ToolId::PasteHistory,
        ToolId::Profiles,
        ToolId::RecentDirectories,
        ToolId::Snippets,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ToolId::Actions => "Actions",
            ToolId::CommandHistory => "Command History",
            ToolId::NamedMarks => "Named Marks",
            ToolId::PasteHistory => "Paste History",
            ToolId::Profiles => "Profiles",
            ToolId::RecentDirectories => "Recent Directories",
            ToolId::Snippets => "Snippets",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolId> {
        ToolId::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The wrapper's content minimum, iTerm2's per-tool minimum (Resume notes, "The seven
    /// tools"); `toolbelt_layout::wrapper_minimum` adds the wrapper's own 31.
    pub fn minimum_height(self) -> f32 {
        match self {
            ToolId::Actions => 60.0,
            ToolId::CommandHistory => 88.0,
            ToolId::NamedMarks => 60.0,
            ToolId::PasteHistory => 60.0,
            ToolId::Profiles => 110.0,
            ToolId::RecentDirectories => 88.0,
            ToolId::Snippets => 87.0,
        }
    }
}

/// The global's key: which tools are configured, in wrapper order (`TBV:86`).
pub const TOOLS_KEY: &str = "ToolbeltTools";
/// The global's key: the last divider proportions, applied to new windows (`TBV:844-947`).
pub const PROPORTIONS_KEY: &str = "NoSyncToolbeltProportions";
/// The global's key: the width a newly shown toolbelt starts at (parity row 02:258; the name
/// is not "DefaultToolbeltWidth").
pub const DEFAULT_WIDTH_KEY: &str = "Default Toolbelt Width";
pub const DEFAULT_WIDTH: f32 = 250.0;
/// The empty-panel label (`TBV:343`).
pub const NO_TOOLS: &str = "No Tools enabled. Select them from the menu above.";

/// `ToolbeltTools` as stored strings, in order. Absent reads as `["Profiles"]`
/// (`TBV:192-204,287-290`); an empty array reads as no tools. Unlike iTerm2, unknown names are
/// kept rather than dropped, so a name from a tool 9j does not show survives a toggle
/// (a ruling; `shown_tools` is what the panel actually draws).
pub fn stored_names(v: Option<&Value>) -> Vec<String> {
    match v {
        None => vec!["Profiles".to_string()],
        Some(v) => v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
    }
}

/// The stored names that are one of 9j's seven tools, in the order stored.
pub fn shown_tools(names: &[String]) -> Vec<ToolId> {
    names.iter().filter_map(|n| ToolId::from_name(n)).collect()
}

/// Toggles a tool in the stored name list: removes it if present, else appends it at the
/// bottom (`TBV:256-271,559-560`). Other names (someone else's tools) are left untouched.
pub fn toggle_name(names: &[String], tool: ToolId) -> Vec<String> {
    let name = tool.name();
    if names.iter().any(|n| n == name) {
        names.iter().filter(|n| n.as_str() != name).cloned().collect()
    } else {
        let mut v = names.to_vec();
        v.push(name.to_string());
        v
    }
}

/// The stored-name list as a TOML value, for writing back to the global.
pub fn names_value(names: &[String]) -> Value {
    Value::Array(names.iter().map(|n| Value::String(n.clone())).collect())
}

/// The Toolbelt menu and the hamburger's list order: every tool, sorted by display name
/// ignoring case (`localizedStandardCompare:`, `TBV:227-254`).
pub fn menu_order() -> Vec<ToolId> {
    let mut v = ToolId::ALL.to_vec();
    v.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
    v
}

/// The toolbelt's width limit: at most half the window's content width, at least the smaller
/// of 100 and 5% of it (`RTV:72-74,1483-1489`).
pub fn clamp_width(width: f32, view_w: f32) -> f32 {
    let upper = width.min(view_w * 0.5);
    let lower = 100.0_f32.min(view_w * 0.05);
    upper.max(lower)
}

/// What showing the toolbelt does to the window: `grow_by` added to the window's width (0 when
/// the window does not grow), the toolbelt's resulting `width`, and whether the window grew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Growth {
    pub grow_by: f32,
    pub width: f32,
    pub resized: bool,
}

/// Growing the window to fit a newly shown toolbelt (`PT:1286-1315`). Only a window that can
/// grow (Normal or No Title Bar, not maximized, full screen or size-locked) is offered growth;
/// others give the toolbelt content room instead (`grow_by` 0, `width` unchanged).
/// With no overage past `work_right` the window grows by the full `width`. With an overage,
/// when `width` less the overage still clears 100 the toolbelt shrinks to that (capped at half
/// the view) and the window grows by the shrunk width; otherwise the window does not grow at
/// all and the toolbelt takes its room from the content instead.
pub fn show_growth(width: f32, view_w: f32, window_right: f32, work_right: f32, can_grow: bool) -> Growth {
    if !can_grow {
        return Growth { grow_by: 0.0, width, resized: false };
    }
    let overage = (window_right + width) - work_right;
    if overage <= 0.0 {
        return Growth { grow_by: width, width, resized: true };
    }
    let reduced = width - overage;
    if reduced > 100.0 {
        let w = reduced.min(view_w * 0.5);
        Growth { grow_by: w, width: w, resized: true }
    } else {
        Growth { grow_by: 0.0, width, resized: false }
    }
}

/// Shrinking the window when the toolbelt hides (`PT:1317-1346`): the amount to shrink by, or
/// 0 when either the window never grew for it (`resized`) or it cannot grow now (`can_grow`).
pub fn hide_shrink(width: f32, resized: bool, can_grow: bool) -> f32 {
    if resized && can_grow { width } else { 0.0 }
}

/// A restored window with the toolbelt shown and no saved "Hiding Toolbelt Should Resize
/// Window" flag: the flag is set when the window's right edge sits 10 points or more inside
/// the visible frame's right edge (`PT:1061-1076`).
pub fn initial_hiding_resizes(window_right: f32, work_right: f32) -> bool {
    work_right - window_right >= 10.0
}

/// Whether the toolbelt opens itself on a tab's arrival: the tab count reached 1 from 0, the
/// profile's "Open Toolbelt" key is on, and it is not already shown (`PT:8200-8207`).
pub fn opens_on_first_tab(tabs_before: usize, tabs_after: usize, open_toolbelt: bool, shown: bool) -> bool {
    tabs_before == 0 && tabs_after == 1 && open_toolbelt && !shown
}

/// The toolbelt's arrangement facts for one window (`PT:196-201,4579-4589,4196-4202`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arranged {
    pub has: bool,
    pub hiding_resizes: Option<bool>,
    pub proportions: Option<Value>,
    pub states: Option<Table>,
}

/// Writes the toolbelt's arrangement keys into a window table. "Toolbelt Proportions" and
/// "Toolbelt" are omitted rather than written nil.
pub fn write_arrangement(t: &mut Table, a: &Arranged) {
    t.insert("Has Toolbelt".into(), Value::Boolean(a.has));
    if let Some(h) = a.hiding_resizes {
        t.insert("Hiding Toolbelt Should Resize Window".into(), Value::Boolean(h));
    }
    if let Some(p) = &a.proportions {
        t.insert("Toolbelt Proportions".into(), p.clone());
    }
    if let Some(s) = &a.states {
        t.insert("Toolbelt".into(), Value::Table(s.clone()));
    }
}

/// Reads the toolbelt's arrangement keys from a window value. Missing keys read as `has`
/// false and everything else `None`.
pub fn read_arrangement(w: &Value) -> Arranged {
    Arranged {
        has: w.get("Has Toolbelt").and_then(Value::as_bool).unwrap_or(false),
        hiding_resizes: w.get("Hiding Toolbelt Should Resize Window").and_then(Value::as_bool),
        proportions: w.get("Toolbelt Proportions").cloned(),
        states: w.get("Toolbelt").and_then(Value::as_table).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_follow_iterm2() {
        let pairs = [
            (ToolId::Actions, "Actions"),
            (ToolId::CommandHistory, "Command History"),
            (ToolId::NamedMarks, "Named Marks"),
            (ToolId::PasteHistory, "Paste History"),
            (ToolId::Profiles, "Profiles"),
            (ToolId::RecentDirectories, "Recent Directories"),
            (ToolId::Snippets, "Snippets"),
        ];
        for (tool, name) in pairs {
            assert_eq!(tool.name(), name);
            assert_eq!(ToolId::from_name(name), Some(tool));
        }
        let heights = [60.0, 88.0, 60.0, 60.0, 110.0, 88.0, 87.0];
        for (tool, h) in ToolId::ALL.into_iter().zip(heights) {
            assert_eq!(tool.minimum_height(), h, "{tool:?}");
        }

        assert_eq!(stored_names(None), vec!["Profiles".to_string()]);
        assert!(stored_names(Some(&Value::Array(vec![]))).is_empty());

        let names: Vec<String> = ["Notes", "Snippets", "Bogus", "Profiles"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shown_tools(&names), vec![ToolId::Snippets, ToolId::Profiles]);

        let profiles_only = vec!["Profiles".to_string()];
        assert_eq!(toggle_name(&profiles_only, ToolId::Snippets), vec!["Profiles".to_string(), "Snippets".to_string()]);

        let notes_and_profiles = vec!["Notes".to_string(), "Profiles".to_string()];
        assert_eq!(toggle_name(&notes_and_profiles, ToolId::Profiles), vec!["Notes".to_string()]);

        assert_eq!(
            menu_order(),
            vec![ToolId::Actions, ToolId::CommandHistory, ToolId::NamedMarks, ToolId::PasteHistory, ToolId::Profiles, ToolId::RecentDirectories, ToolId::Snippets]
        );

        assert_eq!(NO_TOOLS, "No Tools enabled. Select them from the menu above.");
    }

    #[test]
    fn width_and_growth_follow_iterm2() {
        assert_eq!(clamp_width(250.0, 1000.0), 250.0);
        assert_eq!(clamp_width(700.0, 1000.0), 500.0);
        assert_eq!(clamp_width(20.0, 1000.0), 50.0);
        assert_eq!(clamp_width(20.0, 3000.0), 100.0);
        assert_eq!(clamp_width(80.0, 1000.0), 80.0);

        let g = show_growth(250.0, 800.0, 1600.0, 1900.0, true);
        assert_eq!(g, Growth { grow_by: 250.0, width: 250.0, resized: true });

        let g = show_growth(250.0, 800.0, 1700.0, 1900.0, true);
        assert_eq!(g, Growth { grow_by: 200.0, width: 200.0, resized: true });

        let g = show_growth(250.0, 800.0, 1860.0, 1900.0, true);
        assert_eq!(g, Growth { grow_by: 0.0, width: 250.0, resized: false });

        let g = show_growth(250.0, 800.0, 1600.0, 1900.0, false);
        assert_eq!(g, Growth { grow_by: 0.0, width: 250.0, resized: false });

        assert_eq!(hide_shrink(250.0, true, true), 250.0);
        assert_eq!(hide_shrink(250.0, false, true), 0.0);
        assert_eq!(hide_shrink(250.0, true, false), 0.0);

        assert!(!initial_hiding_resizes(1895.0, 1900.0));
        assert!(initial_hiding_resizes(1880.0, 1900.0));
    }

    #[test]
    fn open_toolbelt_and_arrangements_follow_iterm2() {
        assert!(opens_on_first_tab(0, 1, true, false));
        assert!(!opens_on_first_tab(1, 2, true, false));
        assert!(!opens_on_first_tab(0, 1, false, false));
        assert!(!opens_on_first_tab(0, 1, true, true));

        let a = Arranged { has: true, hiding_resizes: Some(false), proportions: None, states: None };
        let mut t = Table::new();
        write_arrangement(&mut t, &a);
        assert_eq!(t.get("Has Toolbelt"), Some(&Value::Boolean(true)));
        assert_eq!(t.get("Hiding Toolbelt Should Resize Window"), Some(&Value::Boolean(false)));
        assert!(t.get("Toolbelt Proportions").is_none());
        assert!(t.get("Toolbelt").is_none());
        assert_eq!(read_arrangement(&Value::Table(t)), a);

        let read = read_arrangement(&Value::Table(Table::new()));
        assert!(!read.has);
        assert_eq!(read.hiding_resizes, None);
    }
}
