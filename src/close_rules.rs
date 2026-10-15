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

/// What the question's body names (iTerm2's iTermCloseSubject, without pinned tabs, which
/// ask through their own reason).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Session,
    Tab,
    MultiPaneTab,
    Window,
    LockedSession,
    TabLocked,
    MultiPaneTabLocked,
    WindowLocked,
}

/// `any_locked`: a pane among those closing is locked (`PT:2165-2310`).
pub fn subject(kind: Kind, panes: usize, any_locked: bool) -> Subject {
    match (kind, any_locked) {
        (Kind::Session, false) => Subject::Session,
        (Kind::Session, true) => Subject::LockedSession,
        (Kind::Tab, false) if panes > 1 => Subject::MultiPaneTab,
        (Kind::Tab, false) => Subject::Tab,
        (Kind::Tab, true) if panes > 1 => Subject::MultiPaneTabLocked,
        (Kind::Tab, true) => Subject::TabLocked,
        (Kind::Window, false) => Subject::Window,
        (Kind::Window, true) => Subject::WindowLocked,
    }
}

/// A locked pane's reason to ask before closing (`iTermPromptOnCloseReason.m:237-239`).
pub const LOCKED_REASON: &str = "This pane is locked.";

/// Whether a close asks first. `running` counts sessions whose shell has not exited;
/// `has_reason` is a Prompt Before Closing reason; `confirm_multiple` is OnlyWhenMoreTabs.
pub fn must_ask(kind: Kind, running: usize, has_reason: bool, confirm_multiple: bool) -> bool {
    let reason = running > 0 && has_reason;
    match kind {
        Kind::Session => reason,
        Kind::Tab | Kind::Window => reason || (running > 1 && confirm_multiple),
    }
}

/// The most job names the question lists (Termurion-side guard: iTerm2 lists them all).
pub const NAMES_SHOWN: usize = 32;

/// Job names sorted without case; a repeated name reads `N instances of "name"`.
pub fn counted(names: &[String]) -> Vec<String> {
    // Each name with its first place and its count, in one pass (a remote can name 20000).
    let mut seen: std::collections::HashMap<&String, (usize, usize)> = std::collections::HashMap::new();
    for (i, n) in names.iter().enumerate() {
        seen.entry(n).or_insert((i, 0)).1 += 1;
    }
    let mut keys: Vec<(&String, usize, usize)> = seen.into_iter().map(|(k, (first, c))| (k, first, c)).collect();
    // Sorted without case; names equal without case keep the order they came in.
    keys.sort_by_cached_key(|(k, first, _)| (k.to_lowercase(), *first));
    keys.into_iter().map(|(k, _, c)| if c > 1 { format!("{c} instances of \"{k}\"") } else { k.clone() }).collect()
}

/// The question's body for the subject and its sorted job names.
pub fn body(subject: Subject, names: &[String]) -> String {
    let what = match subject {
        Subject::Session => "This session",
        Subject::Tab => "This tab",
        Subject::MultiPaneTab => "This multi-pane tab",
        Subject::Window => "This window",
        Subject::LockedSession => "This locked session",
        Subject::TabLocked => "This tab (with a locked session)",
        Subject::MultiPaneTabLocked => "This multi-pane tab (with locked sessions)",
        Subject::WindowLocked => "This window (with locked sessions)",
    };
    match names.len() {
        0 => format!("{what} will be closed."),
        1 => format!("{what} is running {}.", names[0]),
        2..=10 => format!("{what} is running the following jobs: {}.", oxford(names)),
        n if n <= NAMES_SHOWN => format!("{what} is running the following jobs: {}, plus {} others.", oxford(names), n - 10),
        // Termurion-side guard: the first NAMES_SHOWN, then the true count of the rest.
        n => format!("{what} is running the following jobs: {}, plus {} others.", names[..NAMES_SHOWN].join(", "), n - NAMES_SHOWN),
    }
}

/// The body with extra notes after it (iTerm2's additionalMessage), such as a process tree
/// that could not be read.
pub fn with_notes(body: String, notes: &[String]) -> String {
    if notes.is_empty() { body } else { format!("{body}\n\n{}", notes.join("\n")) }
}

/// The tab to select after tab `closed` goes, leaving `remaining`; with
/// moveLeftAfterClosingTab, the one to the left of a closed active tab.
pub fn active_after_close(active: usize, closed: usize, remaining: usize, move_left: bool) -> usize {
    let a = if closed < active || (closed == active && move_left && active > 0) { active - 1 } else { active };
    a.min(remaining.saturating_sub(1))
}


pub const CONFIRM_QUIT: &str = "\u{201c}Settings > General > Closing > Confirm Quit Termurion\u{201d} is enabled and there is at least one terminal window.";
pub const CONFIRM_QUIT_NO_WINDOWS: &str = "\u{201c}Settings > General > Closing > Confirm Quit Termurion\u{201d} and \u{201c}Even if there are no windows\u{201d} is enabled.";
pub const CONFIRM_MULTIPLE: &str = "\u{201c}Settings > General > Closing > Confirm closing multiple sessions\u{201d} is enabled.";

/// The Closing settings a quit reads.
#[derive(Clone, Copy, Debug)]
pub struct QuitPrefs {
    /// PromptOnQuit.
    pub prompt_on_quit: bool,
    /// PromptOnQuitEvenIfThereAreNoWindows.
    pub even_if_no_windows: bool,
    /// OnlyWhenMoreTabs.
    pub confirm_multiple: bool,
}

/// One reason a quit asks: its priority and kind (iTerm2's reason classes).
#[derive(Clone, Debug, PartialEq)]
pub struct Reason {
    pub priority: u32,
    pub kind: ReasonKind,
    pub text: String,
}

/// Prompt Before Closing's two kinds carry the Settings footer; the plain messages do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonKind {
    Blocked,
    Always,
    Message,
}

const PROFILE_FOOTER: &str = "You can change this setting in Settings > Profiles > Session";

/// A Prompt Before Closing reason's kind and priority, from its text.
fn profile_reason(text: &str) -> Reason {
    if text == LOCKED_REASON {
        Reason { priority: 75, kind: ReasonKind::Message, text: text.into() }
    } else if text.ends_with("always requires confirmation.") {
        Reason { priority: 50, kind: ReasonKind::Always, text: text.into() }
    } else if text.starts_with("A session with profile") {
        Reason { priority: 25, kind: ReasonKind::Blocked, text: text.into() }
    } else {
        Reason { priority: 25, kind: ReasonKind::Message, text: text.into() }
    }
}

/// Why quitting asks: each profile's Prompt Before Closing reasons, more than one session
/// (Confirm closing multiple sessions, 90), and Confirm Quit (100). `last_window_closed`
/// is a quit that closing the last window caused (no Confirm Quit reason then).
pub fn quit_reasons(profile_reasons: &[String], windows: usize, sessions: usize, p: QuitPrefs, last_window_closed: bool) -> Vec<Reason> {
    let msg = |priority, text: &str| Reason { priority, kind: ReasonKind::Message, text: text.into() };
    let mut out: Vec<Reason> = profile_reasons.iter().map(|t| profile_reason(t)).collect();
    if !last_window_closed && p.prompt_on_quit {
        if windows > 0 {
            out.push(msg(100, CONFIRM_QUIT));
        } else if p.even_if_no_windows {
            out.push(msg(100, CONFIRM_QUIT_NO_WINDOWS));
        }
    }
    if p.confirm_multiple && sessions > 1 {
        out.push(msg(90, CONFIRM_MULTIPLE));
    }
    out
}

/// The quit question's text, formatted as iTerm2's compound reason: lowest priority first,
/// a repeated reason once, each with a bullet, and the Settings footer after each Prompt
/// Before Closing group (iTerm2 shows this in a disclosure).
/// Whether Quit leaves the shells running in the session host for the next launch
/// (iTerm2's shouldLeaveSessionsRunningOnQuit, without the update-restart case).
pub fn keep_jobs(run_in_servers: bool, restore_contents: bool, system_policy: bool, kill_on_quit: bool) -> bool {
    run_in_servers && restore_contents && system_policy && !kill_on_quit
}

pub fn quit_body(keep: bool) -> &'static str {
    if keep { "Sessions will be restored automatically when Termurion is relaunched." } else { "All sessions will be closed." }
}

/// The quit question as two parts: what happens to the sessions, and the reasons the disclosure
/// "Why am I being prompted?" opens (`iTermApplicationDelegate.m:1043-1057`).
pub fn quit_parts(reasons: &[Reason], keep: bool) -> (String, String) {
    let mut sorted: Vec<&Reason> = reasons.iter().collect();
    sorted.sort_by_key(|r| r.priority);
    sorted.dedup_by(|a, b| a.text == b.text);
    let footer = |k: ReasonKind| matches!(k, ReasonKind::Blocked | ReasonKind::Always).then_some(PROFILE_FOOTER);
    let mut lines: Vec<String> = Vec::new();
    let mut previous = sorted.first().map(|r| r.kind);
    for r in &sorted {
        if Some(r.kind) != previous {
            if let Some(f) = previous.and_then(footer) {
                lines.push(f.to_string());
                lines.push(String::new());
            }
        }
        lines.push(format!("\u{2022} {}", r.text));
        previous = Some(r.kind);
    }
    if let Some(f) = previous.and_then(footer) {
        lines.push(f.to_string());
    }
    (quit_body(keep).to_string(), format!("You are being prompted because:\n\n{}", lines.join("\n")))
}

/// The quit question as one text (the plain box's, when the disclosure dialog cannot open).
pub fn quit_text(reasons: &[Reason], keep: bool) -> String {
    let (body, why) = quit_parts(reasons, keep);
    format!("{body}\n\n{why}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quit_body_follows_kept_jobs() {
        assert!(keep_jobs(true, true, true, false));
        assert!(!keep_jobs(true, true, true, true), "Quit ends jobs by default");
        assert!(!keep_jobs(false, true, true, false), "no host");
        assert!(!keep_jobs(true, false, true, false), "no contents restored");
        assert!(!keep_jobs(true, true, false, false), "another startup policy");
        assert_eq!(quit_body(true), "Sessions will be restored automatically when Termurion is relaunched.");
        assert_eq!(quit_body(false), "All sessions will be closed.");
        let r = [Reason { kind: ReasonKind::Message, priority: 100, text: "x".into() }];
        assert!(quit_text(&r, true).starts_with("Sessions will be restored automatically"));
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
        assert_eq!(subject(Kind::Tab, 3, false), Subject::MultiPaneTab);
        assert_eq!(subject(Kind::Tab, 1, false), Subject::Tab);
    }

    #[test]
    fn move_left_after_close() {
        assert_eq!(active_after_close(2, 2, 3, false), 2, "the tab that took its place (the right one)");
        assert_eq!(active_after_close(2, 2, 3, true), 1, "MoveLeftAfterClosingTab");
        assert_eq!(active_after_close(0, 0, 2, true), 0);
        assert_eq!(active_after_close(3, 1, 3, false), 2, "a tab before the active one closed");
        assert_eq!(active_after_close(2, 2, 2, false), 1, "the last tab closed");
    }

    #[test]
    fn quit_asks_for_the_right_reasons() {
        let q = |profile: &[&str], windows, sessions, on, even, multi, last| quit_reasons(&n(profile), windows, sessions, QuitPrefs { prompt_on_quit: on, even_if_no_windows: even, confirm_multiple: multi }, last);
        assert!(q(&[], 1, 1, false, false, true, false).is_empty(), "Confirm Quit off, one session: no question");
        assert_eq!(quit_text(&q(&[], 1, 1, true, false, true, false), false), format!("All sessions will be closed.\n\nYou are being prompted because:\n\n\u{2022} {CONFIRM_QUIT}"));
        assert!(q(&[], 0, 0, true, false, true, false).is_empty(), "no windows");
        assert_eq!(q(&[], 0, 0, true, true, true, false).len(), 1);
        assert!(q(&[], 1, 1, true, false, true, true).is_empty(), "closing the last window skips Confirm Quit");
        let always = "The profile \u{201c}P\u{201d} always requires confirmation.";
        let blocked = "A session with profile \u{201c}P\u{201d} is running vim.";
        let r = q(&[always, blocked, always], 2, 3, true, false, true, false);
        let footer = "You can change this setting in Settings > Profiles > Session";
        let expect = format!("\u{2022} {blocked}\n{footer}\n\n\u{2022} {always}\n{footer}\n\n\u{2022} {CONFIRM_MULTIPLE}\n\u{2022} {CONFIRM_QUIT}");
        assert_eq!(quit_text(&r, false), format!("All sessions will be closed.\n\nYou are being prompted because:\n\n{expect}"), "iTerm2's order (lowest priority first), one of each, bullets, and the profile footer");
    }

    #[test]
    fn notes_follow_the_body() {
        assert_eq!(with_notes("This tab will be closed.".into(), &[]), "This tab will be closed.");
        assert_eq!(with_notes("This tab will be closed.".into(), &["Termurion could not read the programs (x).".into()]), "This tab will be closed.\n\nTermurion could not read the programs (x).");
    }

    #[test]
    fn lock_texts_follow_iterm2() {
        assert_eq!(subject(Kind::Session, 1, true), Subject::LockedSession);
        assert_eq!(subject(Kind::Tab, 1, true), Subject::TabLocked);
        assert_eq!(subject(Kind::Tab, 3, true), Subject::MultiPaneTabLocked);
        assert_eq!(subject(Kind::Window, 4, true), Subject::WindowLocked);
        assert_eq!(subject(Kind::Session, 1, false), Subject::Session);
        assert_eq!(subject(Kind::Tab, 3, false), Subject::MultiPaneTab);
        assert_eq!(subject(Kind::Window, 4, false), Subject::Window);
        assert_eq!(body(Subject::LockedSession, &[]), "This locked session will be closed.");
        assert_eq!(body(Subject::LockedSession, &n(&["vim"])), "This locked session is running vim.");
        assert_eq!(body(Subject::TabLocked, &n(&["vim"])), "This tab (with a locked session) is running vim.");
        assert_eq!(body(Subject::MultiPaneTabLocked, &n(&["a", "b"])), "This multi-pane tab (with locked sessions) is running the following jobs: a and b.");
        assert_eq!(body(Subject::WindowLocked, &[]), "This window (with locked sessions) will be closed.");
        let r = quit_reasons(&[LOCKED_REASON.to_string()], 1, 1, QuitPrefs { prompt_on_quit: false, even_if_no_windows: false, confirm_multiple: false }, false);
        assert_eq!(r, vec![Reason { priority: 75, kind: ReasonKind::Message, text: "This pane is locked.".into() }]);
    }
}
