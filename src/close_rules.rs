//! When closing asks first, and what it says: iTerm2's session, tab, and window close
//! questions and its quit question.


use crate::close_prompt::oxford;

/// What is closing, for the rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Tab,
    Window,
}

/// What the question's body names (iTerm2's iTermCloseSubject, without pinned tabs and
/// locked sessions, which Tandem does not have).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Session,
    Tab,
    MultiPaneTab,
    Window,
}

pub fn subject(kind: Kind, panes: usize) -> Subject {
    match kind {
        Kind::Session => Subject::Session,
        Kind::Tab if panes > 1 => Subject::MultiPaneTab,
        Kind::Tab => Subject::Tab,
        Kind::Window => Subject::Window,
    }
}

/// Whether a close asks first. `running` counts sessions whose shell has not exited;
/// `has_reason` is a Prompt Before Closing reason; `confirm_multiple` is OnlyWhenMoreTabs.
pub fn must_ask(kind: Kind, running: usize, has_reason: bool, confirm_multiple: bool) -> bool {
    let reason = running > 0 && has_reason;
    match kind {
        Kind::Session => reason,
        Kind::Tab | Kind::Window => reason || (running > 1 && confirm_multiple),
    }
}

/// Job names sorted without case; a repeated name reads `N instances of "name"`.
pub fn counted(names: &[String]) -> Vec<String> {
    let mut keys: Vec<&String> = Vec::new();
    for n in names {
        if !keys.contains(&n) {
            keys.push(n);
        }
    }
    keys.sort_by_key(|k| k.to_lowercase());
    keys.into_iter()
        .map(|k| {
            let c = names.iter().filter(|n| *n == k).count();
            if c > 1 { format!("{c} instances of \"{k}\"") } else { k.clone() }
        })
        .collect()
}

/// The question's body for the subject and its sorted job names.
pub fn body(subject: Subject, names: &[String]) -> String {
    let what = match subject {
        Subject::Session => "This session",
        Subject::Tab => "This tab",
        Subject::MultiPaneTab => "This multi-pane tab",
        Subject::Window => "This window",
    };
    match names.len() {
        0 => format!("{what} will be closed."),
        1 => format!("{what} is running {}.", names[0]),
        2..=10 => format!("{what} is running the following jobs: {}.", oxford(names)),
        n => format!("{what} is running the following jobs: {}, plus {} others.", oxford(names), n - 10),
    }
}

/// The tab to select after tab `closed` goes, leaving `remaining`; with
/// moveLeftAfterClosingTab, the one to the left of a closed active tab.
pub fn active_after_close(active: usize, closed: usize, remaining: usize, move_left: bool) -> usize {
    let a = if closed < active || (closed == active && move_left && active > 0) { active - 1 } else { active };
    a.min(remaining.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bodies_follow_iterm2() {
        assert_eq!(body(Subject::Tab, &n(&["vim"])), "This tab is running vim.");
        assert_eq!(body(Subject::MultiPaneTab, &n(&["a", "b", "c"])), "This multi-pane tab is running the following jobs: a, b, and c.");
        assert_eq!(body(Subject::Window, &[]), "This window will be closed.");
        assert_eq!(body(Subject::Session, &n(&["a", "b"])), "This session is running the following jobs: a and b.");
        let many: Vec<String> = (0..12).map(|i| format!("j{i:02}")).collect();
        assert_eq!(body(Subject::Tab, &many), format!("This tab is running the following jobs: {}, plus 2 others.", crate::close_prompt::oxford(&many)), "iTerm2 joins them all, then adds the count");
    }

    #[test]
    fn counted_names() {
        assert_eq!(counted(&n(&["vim", "Git", "vim", "bash"])), n(&["bash", "Git", "2 instances of \"vim\""]));
    }

    #[test]
    fn tab_close_asks_like_iterm2() {
        assert!(must_ask(Kind::Tab, 2, false, true), "two running sessions with Confirm closing multiple sessions");
        assert!(!must_ask(Kind::Tab, 2, false, false));
        assert!(!must_ask(Kind::Tab, 1, false, true), "one running session");
        assert!(must_ask(Kind::Tab, 1, true, false), "a Prompt Before Closing reason");
        assert!(!must_ask(Kind::Tab, 0, true, true), "nothing running");
        assert!(!must_ask(Kind::Session, 3, false, true), "a session asks only for a reason");
        assert!(must_ask(Kind::Session, 1, true, false));
        assert!(must_ask(Kind::Window, 2, false, true));
        assert_eq!(subject(Kind::Tab, 3), Subject::MultiPaneTab);
        assert_eq!(subject(Kind::Tab, 1), Subject::Tab);
    }

    #[test]
    fn move_left_after_close() {
        assert_eq!(active_after_close(2, 2, 3, false), 2, "the tab that took its place (the right one)");
        assert_eq!(active_after_close(2, 2, 3, true), 1, "moveLeftAfterClosingTab");
        assert_eq!(active_after_close(0, 0, 2, true), 0);
        assert_eq!(active_after_close(3, 1, 3, false), 2, "a tab before the active one closed");
        assert_eq!(active_after_close(2, 2, 2, false), 1, "the last tab closed");
    }
}
