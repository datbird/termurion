//! Commands for changes made in Tandem (iTerm2's TmuxController edits and
//! iTermInitialDirectory+Tmux), exactly as iTerm2 writes them.

/// tmux resizes a pane from a side: R and L for a vertical divider, D and U for a
/// horizontal one; `cells` is how far the divider moved (plus is right or down).
pub fn resize_pane(pane: u32, cells: i32, horizontal: bool) -> String {
    let dir = match (horizontal, cells > 0) {
        (true, true) => "R",
        (true, false) => "L",
        (false, true) => "D",
        (false, false) => "U",
    };
    format!("resize-pane -{dir} -t \"%{pane}\" {}", cells.unsigned_abs())
}

/// Where a new tmux pane or window starts (the profile's Initial Directory).
#[derive(Clone, Debug, PartialEq)]
pub enum Dir {
    Home,
    /// Reuse the directory of the current pane (tmux 1.9 and later).
    Recycle,
    Custom(String),
}

/// iTerm2 escapes a custom directory's single quotes twice (once when it evaluates it,
/// once when it adds `-c`); kept as it is.
fn dir_arg(dir: &Dir, recycling: bool) -> Option<String> {
    let d = match dir {
        Dir::Home => return None,
        Dir::Recycle if recycling => "#{pane_current_path}".to_string(),
        Dir::Recycle => return None,
        Dir::Custom(d) => d.replace('\'', "\\'"),
    };
    Some(format!("-c '{}'", d.replace('\'', "\\'")))
}

fn with_dir(mut args: Vec<String>, dir: &Dir, recycling: bool) -> String {
    if let Some(a) = dir_arg(dir, recycling) {
        args.push(a);
    }
    args.join(" ")
}

/// `split-window`: `side_by_side` is iTerm2's "vertical" split (`-h`).
pub fn split_window(pane: u32, side_by_side: bool, dir: &Dir, recycling: bool) -> String {
    with_dir(vec!["split-window".into(), if side_by_side { "-h" } else { "-v" }.into(), "-t".into(), format!("\"%{pane}\"")], dir, recycling)
}

pub fn list_panes(pane: u32) -> String {
    format!("list-panes -t %{pane} -F '#{{pane_id}}'")
}

/// The pane a split made: in the second list and not the first (-1 when none).
pub fn split_result(before: &str, after: &str) -> i64 {
    let before: Vec<&str> = before.split('\n').collect();
    let Some(new) = after.split('\n').find(|p| !before.contains(p)) else { return -1 };
    match new.strip_prefix('%') {
        Some(n) => n.bytes().take_while(u8::is_ascii_digit).fold(0i64, |v, d| v * 10 + (d - b'0') as i64),
        None => -1,
    }
}

/// `new-window` (New Tmux Tab and New Tmux Window); `session` places it after the session's
/// current window (the Dashboard's add).
pub fn new_window(session: Option<u32>, dir: &Dir, recycling: bool) -> String {
    let mut args: Vec<String> = vec!["new-window".into(), "-PF '#{window_id}'".into()];
    if let Some(s) = session {
        args.extend(["-a".into(), "-t".into(), format!("\"${s}:+\"")]);
    }
    with_dir(args, dir, recycling)
}

pub fn kill_pane(pane: u32) -> String {
    format!("kill-pane -t \"%{pane}\"")
}

pub fn kill_window(window: u32) -> String {
    format!("kill-window -t @{window}")
}

/// Newlines become spaces and backslashes double; quotes are left alone (iTerm2's
/// stringByEscapingBackslashesAndRemovingNewlines).
pub fn escape_name(name: &str) -> String {
    name.replace('\n', " ").replace('\\', "\\\\")
}

pub fn rename_window(window: u32, session: Option<u32>, name: &str) -> String {
    match session {
        Some(s) => format!("rename-window -t \"${s}:@{window}\" \"{}\"", escape_name(name)),
        None => format!("rename-window -t @{window} \"{}\"", escape_name(name)),
    }
}

/// Pane titles need tmux 2.6.
pub fn rename_pane(pane: u32, title: &str, version: Option<f64>) -> Option<String> {
    version.is_some_and(|v| v >= 2.6).then(|| format!("select-pane -t %{pane} -T \"{}\"", escape_name(title)))
}

pub fn zoom(pane: u32) -> String {
    format!("resize-pane -Z -t \"%{pane}\"")
}

pub fn swap_panes(a: u32, b: u32) -> String {
    format!("swap-pane -s \"%{a}\" -t \"%{b}\"")
}

pub fn move_pane(src: u32, dest: u32, side_by_side: bool, before: bool) -> String {
    format!("move-pane -s \"%{src}\" -t \"%{dest}\" {}{}", if side_by_side { "-h" } else { "-v" }, if before { " -b" } else { "" })
}

/// Tab reorder: tmux's order (a `list-windows -F "#{window_id}"` reply) to the wanted order
/// of those windows, as iTerm2's swaps.
pub fn reorder(current_reply: &str, wanted: &[u32]) -> Vec<String> {
    let mut current: Vec<u32> = current_reply.split('\n').filter_map(|w| w.strip_prefix('@')).map(crate::tmux_windows::window_id).filter(|w| wanted.contains(w)).collect();
    let desired: Vec<u32> = wanted.iter().copied().filter(|w| current.contains(w)).collect();
    let mut out = Vec::new();
    for i in 0..current.len() {
        if current[i] != desired[i] {
            let Some(j) = current.iter().position(|w| *w == desired[i]) else { continue };
            out.push(format!("swap-window -s @{} -t @{}", current[i], current[j]));
            current.swap(i, j);
        }
    }
    out
}

/// New Tab or New Window while a tmux pane is current (iTerm2's possiblyTmuxValueForWindow):
/// (heading, text, buttons).
pub fn new_kind_question(window: bool) -> (&'static str, &'static str, [&'static str; 3]) {
    if window {
        ("What kind of window do you want to open?", "The current session is a tmux session. Would you like to create a new tmux window or use the default profile?", ["New tmux Window", "Use Default Profile", "Cancel"])
    } else {
        ("What kind of tab do you want to open?", "The current session is a tmux session. Would you like to create a new tmux tab or use the default profile?", ["New tmux Tab", "Use Default Profile", "Cancel"])
    }
}

/// The Advanced switches that silence the question: either one on makes a tmux tab or
/// window without asking.
pub fn skip_new_kind_question(legacy: bool, specific: bool) -> bool {
    legacy || specific
}

/// The key that remembers the answer; the legacy key wins once it holds one.
pub fn new_kind_key(window: bool, legacy_set: bool) -> &'static str {
    if legacy_set {
        "NoSyncNewWindowOrTabFromTmuxOpensTmux"
    } else if window {
        "NoSyncNewWindowFromTmuxOpensTmux"
    } else {
        "NoSyncNewTabFromTmuxOpensTmux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_iterm2() {
        assert_eq!(resize_pane(3, 5, true), "resize-pane -R -t \"%3\" 5");
        assert_eq!(resize_pane(3, -2, true), "resize-pane -L -t \"%3\" 2");
        assert_eq!(resize_pane(3, 4, false), "resize-pane -D -t \"%3\" 4");
        assert_eq!(resize_pane(3, -1, false), "resize-pane -U -t \"%3\" 1");
        assert_eq!(split_window(4, true, &Dir::Home, true), "split-window -h -t \"%4\"");
        assert_eq!(split_window(4, false, &Dir::Recycle, true), "split-window -v -t \"%4\" -c '#{pane_current_path}'");
        assert_eq!(list_panes(4), "list-panes -t %4 -F '#{pane_id}'");
        assert_eq!(new_window(None, &Dir::Home, true), "new-window -PF '#{window_id}'");
        assert_eq!(new_window(Some(2), &Dir::Home, true), "new-window -PF '#{window_id}' -a -t \"$2:+\"");
        assert_eq!(kill_pane(4), "kill-pane -t \"%4\"");
        assert_eq!(kill_window(1), "kill-window -t @1");
        assert_eq!(zoom(4), "resize-pane -Z -t \"%4\"");
        assert_eq!(swap_panes(1, 2), "swap-pane -s \"%1\" -t \"%2\"");
        assert_eq!(move_pane(1, 2, true, true), "move-pane -s \"%1\" -t \"%2\" -h -b");
        assert_eq!(move_pane(1, 2, false, false), "move-pane -s \"%1\" -t \"%2\" -v");
        assert_eq!(rename_pane(4, "t", Some(2.5)), None, "tmux 2.6 and later");
        assert_eq!(rename_pane(4, "t", Some(3.4)).as_deref(), Some("select-pane -t %4 -T \"t\""));
        assert_eq!(reorder("@0\n@1\n@2\n@9", &[2, 0, 1]), ["swap-window -s @0 -t @2", "swap-window -s @1 -t @0"]);
        assert!(reorder("@0\n@1", &[0, 1]).is_empty());
    }

    #[test]
    fn new_tab_question_follows_iterm2() {
        assert_eq!(
            new_kind_question(false),
            ("What kind of tab do you want to open?", "The current session is a tmux session. Would you like to create a new tmux tab or use the default profile?", ["New tmux Tab", "Use Default Profile", "Cancel"])
        );
        assert_eq!(new_kind_question(true).2[0], "New tmux Window");
        assert_eq!(new_kind_key(true, false), "NoSyncNewWindowFromTmuxOpensTmux");
        assert_eq!(new_kind_key(false, false), "NoSyncNewTabFromTmuxOpensTmux");
        assert_eq!(new_kind_key(false, true), "NoSyncNewWindowOrTabFromTmuxOpensTmux", "the legacy key wins once it is set");
    }

    #[test]
    fn silenced_questions_make_tmux_tabs() {
        assert!(!skip_new_kind_question(false, false), "ask");
        assert!(skip_new_kind_question(true, false), "the legacy switch covers both");
        assert!(skip_new_kind_question(false, true));
    }

    #[test]
    fn names_escape_like_iterm2() {
        assert_eq!(rename_window(3, None, "a\"b\\c\nd$e"), "rename-window -t @3 \"a\"b\\\\c d$e\"");
        assert_eq!(rename_window(3, Some(1), "x"), "rename-window -t \"$1:@3\" \"x\"");
    }

    #[test]
    fn split_finds_the_new_pane() {
        assert_eq!(split_result("%1\n%2", "%1\n%2\n%7"), 7);
        assert_eq!(split_result("%1", "%1"), -1);
        assert_eq!(split_result("%1", "%1\nx"), -1);
    }

    #[test]
    fn directories_follow_iterm2() {
        assert_eq!(split_window(1, true, &Dir::Recycle, false), "split-window -h -t \"%1\"", "tmux 1.8 cannot reuse");
        assert_eq!(split_window(1, true, &Dir::Custom("/home/bob's".into()), true), "split-window -h -t \"%1\" -c '/home/bob\\\\'s'", "escaped twice, as iTerm2");
    }
}
