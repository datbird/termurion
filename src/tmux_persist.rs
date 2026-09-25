//! What Tandem keeps in the tmux session (iTerm2's session options: `@hidden` and
//! `@affinities`, hex with a prefix), and the questions for closing tmux tabs and windows.

use std::collections::BTreeSet;

/// iTerm2's encodedString: the prefix, then lowercase hex of the UTF-8.
pub fn encode_option(text: &str, prefix: &str) -> String {
    let hex: String = text.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}

/// iTerm2's stringByEscapingQuotes: backslashes double, then quotes get a backslash.
pub fn escape_quotes(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn hidden_command(session: u32, hidden: &BTreeSet<u32>) -> String {
    let list: Vec<String> = hidden.iter().map(u32::to_string).collect();
    format!("set -t ${session} @hidden \"{}\"", encode_option(&list.join(","), "i_"))
}

/// One Tandem window's tmux windows (in tab order), its name (`pty-...`), and whether it
/// is full screen.
pub struct Group {
    pub windows: Vec<u32>,
    pub terminal: String,
    pub full_screen: bool,
}

/// The affinities text: each group `w1,w2,pty-...;options`, separated by spaces.
pub fn affinities_value(groups: &[Group]) -> String {
    groups
        .iter()
        .filter(|g| !g.windows.is_empty())
        .map(|g| {
            let mut siblings: Vec<String> = g.windows.iter().map(u32::to_string).collect();
            siblings.push(g.terminal.clone());
            let options = if g.full_screen { "WindowStyle=FullScreen" } else { "" };
            format!("{};{options}", siblings.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn affinities_command(session: u32, value: &str) -> String {
    format!("set -t ${session} @affinities \"{}\"", encode_option(&escape_quotes(value), "a_"))
}

/// Closing one tmux tab while the window has others of the same session:
/// (text, buttons), buttons in iTerm2's order.
pub fn close_tab_question() -> (&'static str, [&'static str; 3]) {
    ("Kill tmux window, terminating its jobs, or hide it? Hidden windows may be restored from the tmux dashboard.", ["Hide", "Cancel", "Kill"])
}

/// Closing a window (or its last tmux tab) with `n` attached tmux tabs; None for none.
pub fn close_window_question(n: usize) -> Option<(&'static str, [&'static str; 4])> {
    let text = match n {
        0 => return None,
        1 => "Kill window and its jobs, hide window from view, or detach from tmux session?\n\nHidden windows may be restored from the tmux dashboard.",
        _ => "Kill all tmux windows and their jobs, hide windows from view, or detach from tmux session?\n\nHidden windows may be restored from the tmux dashboard.",
    };
    Some((text, ["Hide", "Detach tmux Session", "Kill", "Cancel"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_windows::{Affinities, decode_option, hidden};

    #[test]
    fn options_encode_round_trip() {
        assert_eq!(encode_option("1,5", "i_"), "i_312c35");
        assert_eq!(decode_option(&encode_option("h\u{e9}", "o_"), "o_"), "h\u{e9}");
        let set: BTreeSet<u32> = [5, 1].into();
        let cmd = hidden_command(2, &set);
        assert_eq!(cmd, "set -t $2 @hidden \"i_312c35\"");
        let value = cmd.split('"').nth(1).unwrap();
        assert_eq!(hidden(value), set, "8b reads what 8c writes");
        let groups = [Group { windows: vec![1, 2], terminal: "pty-A".into(), full_screen: false }, Group { windows: vec![3], terminal: "pty-B".into(), full_screen: true }, Group { windows: vec![], terminal: "pty-C".into(), full_screen: false }];
        let v = affinities_value(&groups);
        assert_eq!(v, "1,2,pty-A; 3,pty-B;WindowStyle=FullScreen");
        let cmd = affinities_command(0, &v);
        let a = Affinities::parse(cmd.split('"').nth(1).unwrap());
        assert!(a.class_of("1").contains("2") && a.class_of("3").contains("pty-B"));
        assert_eq!(escape_quotes("a\"b\\"), "a\\\"b\\\\");
    }

    #[test]
    fn close_questions_follow_iterm2() {
        assert_eq!(close_tab_question().1, ["Hide", "Cancel", "Kill"]);
        assert!(close_tab_question().0.starts_with("Kill tmux window, terminating its jobs, or hide it?"));
        assert_eq!(close_window_question(0), None);
        assert!(close_window_question(1).unwrap().0.starts_with("Kill window and its jobs"));
        assert!(close_window_question(2).unwrap().0.starts_with("Kill all tmux windows"));
        assert_eq!(close_window_question(1).unwrap().1, ["Hide", "Detach tmux Session", "Kill", "Cancel"]);
    }
}
