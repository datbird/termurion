//! The toolbelt: iTerm2's panel of native tools at the right of a terminal window (View >
//! Toolbelt, Shift+Cmd+B). This module is the tool registry, the global tool list, the width
//! clamp, window growth on show and hide, and the arrangement keys. Split view math lives in
//! `toolbelt_layout`, the shared list widget in `tool_list`, each tool's rows in
//! `toolbelt_tools`; the app side wires them together in `src/app/toolbelt_ui.rs`.

use toml::{Table, Value};

/// The twelve built-in tools (9j's seven plus 9k's Captured Output, Session Status, Jobs,
/// Notes and Codecierge). Names double as identifiers (iTerm2's `TBV:33-55,95-106`); the
/// English display name equals the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolId {
    Actions,
    CapturedOutput,
    CommandHistory,
    NamedMarks,
    SessionStatus,
    RecentDirectories,
    Jobs,
    Notes,
    PasteHistory,
    Profiles,
    Snippets,
    Codecierge,
}

impl ToolId {
    /// iTerm2's registration order (`TBV:33-55`).
    pub const ALL: [ToolId; 12] = [
        ToolId::Actions,
        ToolId::CapturedOutput,
        ToolId::CommandHistory,
        ToolId::NamedMarks,
        ToolId::SessionStatus,
        ToolId::RecentDirectories,
        ToolId::Jobs,
        ToolId::Notes,
        ToolId::PasteHistory,
        ToolId::Profiles,
        ToolId::Snippets,
        ToolId::Codecierge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ToolId::Actions => "Actions",
            ToolId::CapturedOutput => "Captured Output",
            ToolId::CommandHistory => "Command History",
            ToolId::NamedMarks => "Named Marks",
            ToolId::SessionStatus => "Session Status",
            ToolId::RecentDirectories => "Recent Directories",
            ToolId::Jobs => "Jobs",
            ToolId::Notes => "Notes",
            ToolId::PasteHistory => "Paste History",
            ToolId::Profiles => "Profiles",
            ToolId::Snippets => "Snippets",
            ToolId::Codecierge => "Codecierge",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolId> {
        ToolId::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The wrapper's content minimum, iTerm2's per-tool minimum (Resume notes, "The seven
    /// tools", "Captured Output", "Session Status", "Jobs", "Notes and Codecierge");
    /// `toolbelt_layout::wrapper_minimum` adds the wrapper's own 31.
    pub fn minimum_height(self) -> f32 {
        match self {
            ToolId::Actions => 60.0,
            ToolId::CapturedOutput => 91.0,
            ToolId::CommandHistory => 88.0,
            ToolId::NamedMarks => 60.0,
            ToolId::SessionStatus => 36.0,
            ToolId::RecentDirectories => 88.0,
            ToolId::Jobs => 76.0,
            ToolId::Notes => 68.0,
            ToolId::PasteHistory => 60.0,
            ToolId::Profiles => 110.0,
            ToolId::Snippets => 87.0,
            ToolId::Codecierge => crate::codecierge::MIN_HEIGHT,
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

/// The stored names that are one of the built-in tools, in the order stored. A dynamic tool's
/// name draws nothing: its web view cannot be made without a WebView2 host, and iTerm2 adds no
/// wrapper for a tool whose view it could not make (`TBV:580-611`).
pub fn shown_tools(names: &[String]) -> Vec<ToolId> {
    names.iter().filter_map(|n| ToolId::from_name(n)).collect()
}

/// Toggles a tool in the stored name list: removes it if present, else appends it at the
/// bottom (`TBV:256-271,559-560`). Other names (someone else's tools) are left untouched.
pub fn toggle_name(names: &[String], tool: ToolId) -> Vec<String> {
    toggle_named(names, tool.name())
}

/// `toggle_name` for any tool name, built-in or dynamic.
pub fn toggle_named(names: &[String], name: &str) -> Vec<String> {
    if names.iter().any(|n| n == name) {
        names.iter().filter(|n| n.as_str() != name).cloned().collect()
    } else {
        let mut v = names.to_vec();
        v.push(name.to_string());
        v
    }
}

/// The global's key: the dynamic tools scripts registered, identifier to `{name, URL}`
/// (`TBV:87-89`).
pub const DYNAMIC_TOOLS_KEY: &str = "NoSyncDynamicTools";
const DYNAMIC_NAME: &str = "name";
const DYNAMIC_URL: &str = "URL";

/// A tool a script registered with RegisterTool: a web view in iTerm2 (`ToolWebView`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicTool {
    pub identifier: String,
    pub name: String,
    pub url: String,
}

/// `NoSyncDynamicTools` read back, in identifier order; entries without a name or URL are
/// skipped.
pub fn dynamic_tools(v: Option<&Value>) -> Vec<DynamicTool> {
    let Some(t) = v.and_then(Value::as_table) else { return Vec::new() };
    t.iter()
        .filter_map(|(id, e)| {
            let name = e.get(DYNAMIC_NAME)?.as_str()?;
            let url = e.get(DYNAMIC_URL)?.as_str()?;
            Some(DynamicTool { identifier: id.clone(), name: name.to_string(), url: url.to_string() })
        })
        .collect()
}

/// Whether NSURL finds a host in a URL (`URLWithString:`, RFC 3986), and the host as written
/// (not percent-decoded): an optional `scheme:`, then `//` and an authority, the host being
/// the authority without user info or port. None without an authority (`about:blank`,
/// `file:///x`, `example.com/x`) or with a space or control character inside it; elsewhere
/// NSURL encodes such characters, so they do not matter.
pub fn url_host(url: &str) -> Option<&str> {
    let scheme_ok = |scheme: &str| {
        let mut sc = scheme.chars();
        sc.next().is_some_and(|c| c.is_ascii_alphabetic()) && sc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    };
    // A network-path reference (`//host/x`) has no scheme.
    let rest = match url.split_once(':') {
        Some((scheme, rest)) if scheme_ok(scheme) => rest,
        _ => url,
    };
    let rest = rest.strip_prefix("//")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match hostport.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => hostport.split_once(':').map_or(hostport, |(h, _)| h),
    };
    (!host.is_empty()).then_some(host)
}

/// RegisterTool's status (`H:2216-2243`): a missing name, identifier or URL, or a URL without
/// a host, is REQUEST_MALFORMED; a built-in tool's name PERMISSION_DENIED; otherwise OK.
pub fn register_tool_status(r: &crate::api_proto::pb::RegisterToolRequest) -> crate::api_proto::pb::register_tool_response::Status {
    use crate::api_proto::pb::register_tool_response::Status;
    let (Some(name), Some(_), Some(url)) = (&r.name, &r.identifier, &r.url) else { return Status::RequestMalformed };
    if url_host(url).is_none() {
        Status::RequestMalformed
    } else if ToolId::from_name(name).is_some() {
        Status::PermissionDenied
    } else {
        Status::Ok
    }
}

/// `registerDynamicToolWithIdentifier:` (`TBV:154-186`) over the registry and the configured
/// names: the same name and URL again only shows the tool, when `reveal` and it is not
/// configured; otherwise the registry gets the entry, the identifier's old name is hidden when
/// shown, and the new name is shown when not. Returns the new registry when it changed, and
/// the new configured names.
pub fn register_dynamic(registry: Option<&Value>, names: &[String], identifier: &str, name: &str, url: &str, reveal: bool) -> (Option<Value>, Vec<String>) {
    let table = registry.and_then(Value::as_table);
    let old = table.and_then(|t| t.get(identifier));
    let old_name = old.and_then(|e| e.get(DYNAMIC_NAME)).and_then(Value::as_str);
    let old_url = old.and_then(|e| e.get(DYNAMIC_URL)).and_then(Value::as_str);
    let configured = |names: &[String], n: &str| names.iter().any(|x| x == n);
    if old_url == Some(url) && old_name == Some(name) {
        let names = if reveal && !configured(names, name) { toggle_named(names, name) } else { names.to_vec() };
        return (None, names);
    }
    let mut t = table.cloned().unwrap_or_default();
    let mut entry = Table::new();
    entry.insert(DYNAMIC_NAME.into(), Value::String(name.to_string()));
    entry.insert(DYNAMIC_URL.into(), Value::String(url.to_string()));
    t.insert(identifier.to_string(), Value::Table(entry));
    let mut names = names.to_vec();
    if let Some(old_name) = old_name.filter(|n| configured(&names, n)) {
        names = toggle_named(&names, old_name);
    }
    if !configured(&names, name) {
        names = toggle_named(&names, name);
    }
    (Some(Value::Table(t)), names)
}

/// The Toolbelt menu and the hamburger's list order: every built-in tool and every dynamic
/// one, sorted by display name as `localizedStandardCompare:` does (`TBV:227-254`): ignoring
/// case, runs of digits by their value ("Web 2" before "Web 10"); a dynamic name listed twice
/// appears once.
pub fn all_tool_names(dynamic: &[DynamicTool]) -> Vec<String> {
    let mut v: Vec<String> = ToolId::ALL.iter().map(|t| t.name().to_string()).collect();
    for d in dynamic {
        if !v.contains(&d.name) {
            v.push(d.name.clone());
        }
    }
    v.sort_by(|a, b| standard_cmp(a, b).then_with(|| a.cmp(b)));
    v
}

/// `localizedStandardCompare:` for plain names: case ignored, digit runs compared by value.
fn standard_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // Leading zeros do not count; then the longer run is the larger number.
            let x: String = a[si..i].iter().skip_while(|c| **c == '0').collect();
            let y: String = b[sj..j].iter().skip_while(|c| **c == '0').collect();
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
            if o != Ordering::Equal {
                return o;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

/// `configuredTools` (`TBV:192-205`): the stored names that are a registered tool (a built-in
/// or one of `dynamic`), in the order stored. A toggle or a registration starts from this, so
/// a name nothing registers any more is dropped when the list is next written.
pub fn configured_names(names: &[String], dynamic: &[DynamicTool]) -> Vec<String> {
    names.iter().filter(|n| ToolId::from_name(n).is_some() || dynamic.iter().any(|d| &d.name == *n)).cloned().collect()
}

/// The stored-name list as a TOML value, for writing back to the global.
pub fn names_value(names: &[String]) -> Value {
    Value::Array(names.iter().map(|n| Value::String(n.clone())).collect())
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

/// `finishToolbeltInitialization` (`PT:1061-1076`), run once after a new window is set up
/// (a restored window never runs it: its arrangement's key, absent reading as false, marks the
/// flag initialized, `PT:4201-4202`): hiding resizes unless the window's right edge is within
/// 10 points of the visible frame's right edge, either side (`fabs`).
pub fn initial_hiding_resizes(window_right: f32, work_right: f32) -> bool {
    (work_right - window_right).abs() >= 10.0
}

/// A new window that showed its toolbelt as it opened (Open Toolbelt): the deferred
/// `finishToolbeltInitialization` overwrites the flag the show set, measured on the frame
/// after it grew by `grow_by` (`PT:782-787`).
pub fn new_window_hiding_resizes(window_right: f32, grow_by: f32, work_right: f32) -> bool {
    initial_hiding_resizes(window_right + grow_by, work_right)
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
            (ToolId::CapturedOutput, "Captured Output"),
            (ToolId::CommandHistory, "Command History"),
            (ToolId::NamedMarks, "Named Marks"),
            (ToolId::SessionStatus, "Session Status"),
            (ToolId::RecentDirectories, "Recent Directories"),
            (ToolId::Jobs, "Jobs"),
            (ToolId::Notes, "Notes"),
            (ToolId::PasteHistory, "Paste History"),
            (ToolId::Profiles, "Profiles"),
            (ToolId::Snippets, "Snippets"),
            (ToolId::Codecierge, "Codecierge"),
        ];
        for (tool, name) in pairs {
            assert_eq!(tool.name(), name);
            assert_eq!(ToolId::from_name(name), Some(tool));
        }
        let heights = [60.0, 91.0, 88.0, 60.0, 36.0, 88.0, 76.0, 68.0, 60.0, 110.0, 87.0, 130.0];
        for (tool, h) in ToolId::ALL.into_iter().zip(heights) {
            assert_eq!(tool.minimum_height(), h, "{tool:?}");
        }

        assert_eq!(stored_names(None), vec!["Profiles".to_string()]);
        assert!(stored_names(Some(&Value::Array(vec![]))).is_empty());

        let names: Vec<String> = ["Notes", "Snippets", "Bogus", "Profiles"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shown_tools(&names), vec![ToolId::Notes, ToolId::Snippets, ToolId::Profiles]);

        let profiles_only = vec!["Profiles".to_string()];
        assert_eq!(toggle_name(&profiles_only, ToolId::Snippets), vec!["Profiles".to_string(), "Snippets".to_string()]);

        let notes_and_profiles = vec!["Notes".to_string(), "Profiles".to_string()];
        assert_eq!(toggle_name(&notes_and_profiles, ToolId::Profiles), vec!["Notes".to_string()]);

        let order: Vec<ToolId> = all_tool_names(&[]).iter().filter_map(|n| ToolId::from_name(n)).collect();
        assert_eq!(order.len(), 12);
        assert_eq!(
            order,
            vec![
                ToolId::Actions,
                ToolId::CapturedOutput,
                ToolId::Codecierge,
                ToolId::CommandHistory,
                ToolId::Jobs,
                ToolId::NamedMarks,
                ToolId::Notes,
                ToolId::PasteHistory,
                ToolId::Profiles,
                ToolId::RecentDirectories,
                ToolId::SessionStatus,
                ToolId::Snippets,
            ]
        );

        assert_eq!(NO_TOOLS, "No Tools enabled. Select them from the menu above.");
    }

    #[test]
    fn register_tool_follows_iterm2() {
        use crate::api_proto::pb;
        use pb::register_tool_response::Status;
        let req = |name: Option<&str>, identifier: Option<&str>, url: Option<&str>| pb::RegisterToolRequest {
            name: name.map(str::to_string),
            identifier: identifier.map(str::to_string),
            url: url.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(register_tool_status(&req(None, Some("id"), Some("https://example.com/x"))), Status::RequestMalformed);
        assert_eq!(register_tool_status(&req(Some("My Tool"), None, Some("https://example.com/x"))), Status::RequestMalformed);
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), None)), Status::RequestMalformed);
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), Some("file:///c/x"))), Status::RequestMalformed);
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), Some("not a url"))), Status::RequestMalformed);
        assert_eq!(register_tool_status(&req(Some("Profiles"), Some("id"), Some("https://example.com/x"))), Status::PermissionDenied);
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), Some("https://example.com/x"))), Status::Ok);

        assert_eq!(url_host("https://user@example.com:8080/p"), Some("example.com"));
        assert_eq!(url_host("https://example.com"), Some("example.com"));
        assert_eq!(url_host("http://localhost:9000?q#f"), Some("localhost"));
        assert_eq!(url_host("about:blank"), None);
        assert_eq!(url_host("file:///c/x"), None);
        assert_eq!(url_host("not a url"), None);
        assert_eq!(url_host("https://a b/"), None);
        assert_eq!(url_host("example.com/x"), None);
        // As NSURL: a network-path reference has a host, and a space outside the authority is
        // encoded, not refused; a percent-encoded host is given as written.
        assert_eq!(url_host("//example.com/x"), Some("example.com"));
        assert_eq!(url_host("https://example.com/a b"), Some("example.com"));
        assert_eq!(url_host("https://example.com/a\u{a0}b"), Some("example.com"));
        assert_eq!(url_host("http://%65xample.com"), Some("%65xample.com"));
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), Some("//example.com/x"))), Status::Ok);
        assert_eq!(register_tool_status(&req(Some("My Tool"), Some("id"), Some("https://example.com/a b"))), Status::Ok);
    }

    #[test]
    fn dynamic_tools_follow_iterm2() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        let profiles = s(&["Profiles"]);
        let (reg, names) = register_dynamic(None, &profiles, "id1", "Web", "https://a", false);
        let reg = reg.expect("a new registry");
        let expected: Value = toml::from_str::<Table>("[id1]\nname = \"Web\"\nURL = \"https://a\"\n").map(Value::Table).unwrap();
        assert_eq!(reg, expected);
        assert_eq!(names, s(&["Profiles", "Web"]));
        assert_eq!(dynamic_tools(Some(&reg)), vec![DynamicTool { identifier: "id1".into(), name: "Web".into(), url: "https://a".into() }]);
        assert!(dynamic_tools(None).is_empty());

        // The same name and URL again: nothing saved, the names as they were...
        assert_eq!(register_dynamic(Some(&reg), &names, "id1", "Web", "https://a", false), (None, names.clone()));
        assert_eq!(register_dynamic(Some(&reg), &profiles, "id1", "Web", "https://a", false), (None, profiles.clone()));
        // ...unless asked to reveal a tool that is not configured.
        assert_eq!(register_dynamic(Some(&reg), &profiles, "id1", "Web", "https://a", true), (None, s(&["Profiles", "Web"])));
        assert_eq!(register_dynamic(Some(&reg), &names, "id1", "Web", "https://a", true), (None, names.clone()));

        // A new name for the identifier: the old name hides, the new one shows.
        let shown = s(&["Web", "Profiles"]);
        let (reg2, names2) = register_dynamic(Some(&reg), &shown, "id1", "Web 2", "https://a", false);
        let reg2 = reg2.expect("a changed registry");
        assert_eq!(names2, s(&["Profiles", "Web 2"]));
        assert_eq!(dynamic_tools(Some(&reg2)), vec![DynamicTool { identifier: "id1".into(), name: "Web 2".into(), url: "https://a".into() }]);
        // A new URL under the same name: hidden and shown again, so it moves to the bottom.
        let (reg3, names3) = register_dynamic(Some(&reg), &shown, "id1", "Web", "https://b", false);
        assert!(reg3.is_some());
        assert_eq!(names3, s(&["Profiles", "Web"]));
        // A second identifier joins the first.
        let (reg4, _) = register_dynamic(Some(&reg), &names, "id2", "Other", "https://o", false);
        assert_eq!(dynamic_tools(reg4.as_ref()).len(), 2);

        let all = all_tool_names(&dynamic_tools(Some(&reg)));
        assert_eq!(all.len(), 13);
        let web = all.iter().position(|n| n == "Web").unwrap();
        assert_eq!(all[web - 1], "Snippets");
        assert_eq!(&all[..3], &s(&["Actions", "Captured Output", "Codecierge"])[..]);
        let lower = all_tool_names(&[DynamicTool { identifier: "x".into(), name: "codex".into(), url: "https://c".into() }]);
        assert_eq!(&lower[2..5], &s(&["Codecierge", "codex", "Command History"])[..]);
        // Numbers in names go by value, as localizedStandardCompare.
        let web = |n: &str| DynamicTool { identifier: n.into(), name: n.into(), url: "https://w".into() };
        let numbered: Vec<String> = all_tool_names(&[web("Web 10"), web("Web 2"), web("web 1")]).into_iter().filter(|n| n.to_lowercase().starts_with("web")).collect();
        assert_eq!(numbered, s(&["web 1", "Web 2", "Web 10"]));
        assert_eq!(standard_cmp("a02", "a2"), std::cmp::Ordering::Equal);
        assert_eq!(standard_cmp("a", "a1"), std::cmp::Ordering::Less);

        // Only registered tools stay configured (`configuredTools`): a stale "Web" left from a
        // removed registry is dropped, so registering "Web" again appends it at the bottom.
        let stored = s(&["Web", "Profiles", "Junk"]);
        assert_eq!(configured_names(&stored, &[]), s(&["Profiles"]));
        assert_eq!(configured_names(&stored, &dynamic_tools(Some(&reg))), s(&["Web", "Profiles"]));
        let (_, again) = register_dynamic(None, &configured_names(&stored, &[]), "id1", "Web", "https://a", false);
        assert_eq!(again, s(&["Profiles", "Web"]));

        assert!(toggle_named(&s(&["Web"]), "Web").is_empty());
        assert_eq!(toggle_named(&profiles, "Web"), s(&["Profiles", "Web"]));
        assert_eq!(DYNAMIC_TOOLS_KEY, "NoSyncDynamicTools");
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
        // A new window that opened its toolbelt: measured on the grown frame (`PT:1061-1076`
        // runs after the growth); `fabs`, so a window past the edge counts as far from it.
        assert!(new_window_hiding_resizes(1600.0, 250.0, 1900.0), "grown to 1850: 50 inside");
        assert!(!new_window_hiding_resizes(1650.0, 250.0, 1900.0), "grown to the edge");
        assert!(new_window_hiding_resizes(1860.0, 0.0, 1900.0), "40 inside, no growth");
        assert!(new_window_hiding_resizes(1900.0, 50.0, 1900.0), "50 past the edge");
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
