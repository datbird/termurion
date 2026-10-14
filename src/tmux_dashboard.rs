//! The tmux Dashboard's model (iTerm2's TmuxDashboardController, TmuxSessionsTable, and
//! TmuxWindowsTable): rows, which buttons work, and the commands they send.

/// A session row: tmux's id and name.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    pub id: u32,
    pub name: String,
}

/// A window row: its id, name, and whether it shows in Termurion.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowRow {
    pub id: u32,
    pub name: String,
    pub open: bool,
}

/// "New Session", then "New Session 2", "New Session 3"... the first name no session has.
pub fn new_session_name(existing: &[SessionRow]) -> String {
    let name = |n: u32| if n == 0 { "New Session".to_string() } else { format!("New Session {}", n + 1) };
    let mut n = 0;
    while existing.iter().any(|s| s.name == name(n)) {
        n += 1;
    }
    name(n)
}

/// The windows the "Search Windows" field keeps (a case-insensitive part of the name).
pub fn filter<'a>(rows: &'a [WindowRow], query: &str) -> Vec<&'a WindowRow> {
    let q = query.to_lowercase();
    rows.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)).collect()
}

/// Which session buttons work: (Attach, Detach, -).
pub fn session_buttons(selected: Option<u32>, attached: Option<u32>) -> (bool, bool, bool) {
    match selected {
        None => (false, false, false),
        Some(s) => {
            let is = attached == Some(s);
            (!is, is, true)
        }
    }
}

/// psmux runs one session per server: the Dashboard cannot make or attach to another one
/// through a control-mode client (its `new-session` and `attach-session` are not there).
pub fn session_buttons_for(psmux: bool, attach: bool) -> (bool, bool) {
    (!psmux, attach && !psmux)
}

/// Which window buttons work, and the Open in Windows label.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowButtons {
    pub add: bool,
    pub remove: bool,
    pub open_in_tabs: bool,
    pub open_in_windows: bool,
    pub open_label: &'static str,
    pub hide: bool,
}

/// iTerm2's updateEnabledStateOfButtons: `session_selected`: a session row is selected;
/// `attached_selected`: it is the attached one; `shown`: rows after the filter.
pub fn window_buttons(session_selected: bool, attached_selected: bool, shown: usize, selected: &[&WindowRow]) -> WindowButtons {
    let any_open = selected.iter().any(|w| w.open);
    let all_open = !selected.is_empty() && selected.iter().all(|w| w.open);
    let open = attached_selected && !selected.is_empty() && !any_open;
    WindowButtons {
        add: session_selected && shown > 0,
        remove: session_selected && !selected.is_empty(),
        open_in_tabs: open,
        open_in_windows: open,
        open_label: if open && selected.len() == 1 { "Open in Window" } else { "Open in Windows" },
        hide: all_open,
    }
}

pub fn rename_session(session: u32, name: &str) -> String {
    format!("rename-session -t \"${session}\" \"{}\"", crate::tmux_persist::escape_quotes(name))
}

pub fn kill_session(session: u32) -> String {
    format!("kill-session -t \"${session}\"")
}

pub fn new_session(name: &str) -> String {
    format!("new-session -s \"{}\"", crate::tmux_persist::escape_quotes(name))
}

pub fn attach_session(session: u32) -> String {
    format!("attach-session -t \"${session}\"")
}

pub fn unlink_window(window: u32) -> String {
    format!("unlink-window -k -t @{window}")
}

pub fn list_windows(session: u32, detailed_format: &str) -> String {
    format!("list-windows -F {detailed_format} -t \"${session}\"")
}

pub fn list_sessions() -> &'static str {
    "list-sessions -F \"#{session_id} #{session_name}\""
}

/// The Dashboard's limit text (a person-facing name for TmuxDashboardLimit).
pub const LIMIT_LABEL: &str = "Don't open windows when attaching if there are at least this many windows:";
pub const HIDDEN_LABEL: &str = "Open dashboard when attaching if there are hidden windows";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_iterm2() {
        assert_eq!(rename_session(3, "a\"b"), "rename-session -t \"$3\" \"a\\\"b\"");
        assert_eq!(kill_session(3), "kill-session -t \"$3\"");
        assert_eq!(new_session("New Session"), "new-session -s \"New Session\"");
        assert_eq!(attach_session(2), "attach-session -t \"$2\"");
        assert_eq!(unlink_window(7), "unlink-window -k -t @7");
        assert_eq!(session_buttons_for(false, true), (true, true));
        assert_eq!(session_buttons_for(true, true), (false, false), "psmux: no new session, no attach");
        assert_eq!(list_windows(2, "\"F\""), "list-windows -F \"F\" -t \"$2\"");
        assert_eq!(list_sessions(), "list-sessions -F \"#{session_id} #{session_name}\"");
    }

    #[test]
    fn new_session_names_are_unique() {
        let s = |n: &str| SessionRow { id: 0, name: n.into() };
        assert_eq!(new_session_name(&[]), "New Session");
        assert_eq!(new_session_name(&[s("New Session")]), "New Session 2");
        assert_eq!(new_session_name(&[s("New Session"), s("New Session 2")]), "New Session 3");
        assert_eq!(new_session_name(&[s("New Session 2")]), "New Session");
    }

    #[test]
    fn buttons_follow_iterm2() {
        assert_eq!(session_buttons(None, Some(1)), (false, false, false));
        assert_eq!(session_buttons(Some(1), Some(1)), (false, true, true), "the attached session: Detach");
        assert_eq!(session_buttons(Some(2), Some(1)), (true, false, true));
        let open = WindowRow { id: 1, name: "a".into(), open: true };
        let hidden = WindowRow { id: 2, name: "b".into(), open: false };
        let b = window_buttons(true, true, 2, &[&hidden]);
        assert!(b.open_in_tabs && b.open_in_windows && !b.hide);
        assert_eq!(b.open_label, "Open in Window");
        let b = window_buttons(true, true, 2, &[&hidden, &open]);
        assert!(!b.open_in_tabs && !b.hide, "an open window in the selection");
        assert_eq!(b.open_label, "Open in Windows");
        assert!(window_buttons(true, true, 2, &[&open]).hide);
        assert!(!window_buttons(true, false, 2, &[&hidden]).open_in_tabs, "only the attached session's windows open");
        assert!(!window_buttons(true, true, 0, &[]).add, "iTerm2 needs a row shown for +");
    }

    #[test]
    fn window_filter_matches_names() {
        let rows = vec![WindowRow { id: 1, name: "Editor".into(), open: true }, WindowRow { id: 2, name: "logs".into(), open: false }];
        assert_eq!(filter(&rows, "ED").len(), 1);
        assert_eq!(filter(&rows, "").len(), 2);
        assert!(filter(&rows, "zz").is_empty());
    }
}
