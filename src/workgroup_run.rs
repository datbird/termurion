//! Entering a workgroup (milestone 13y1, Task 3): the pure side. Dispositions and refusals, the
//! spawn plan, the teardown order, the close warning, provenance, the trust question and how a
//! member's command line is made. The app side is `app/workgroup_app.rs`.
//!
//! Citations: `WC` `iTermWorkgroupController.swift`, `WI` `iTermWorkgroupInstance.swift`, `WSS`
//! `WorkgroupSessionSpawner.swift`, `WSC` `iTermWorkgroupSessionConfig.swift`, `WMN`
//! `WorkgroupMenu.swift`, all under `~/gitrepos/iterm2-ref/sources/Workgroups`.

use crate::inert_files::display_text;
use crate::profile::Profile;
use crate::workgroup::{self as wg, Kind, SessionConfig, Source, SplitSettings, Workgroup};

/// Marks a workgroup instance id (`WI`'s `instanceIDPrefix`): ITERM_WORKGROUP_ID, logs, saved
/// arrangements and mentions all carry it.
pub const INSTANCE_PREFIX: &str = "wg-";

/// The most members a person enters without being asked (the "above 8 asks" rule, B8).
pub const ASK_ABOVE: usize = 8;

/// A fresh instance id: the prefix and an upper-case UUID (`WI:mintInstanceUniqueIdentifier`).
pub fn mint_instance_id() -> String {
    format!("{INSTANCE_PREFIX}{}", uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase())
}

/// Whether `s` is an id `mint_instance_id` could have made: the only value that may reach a
/// session's environment as ITERM_WORKGROUP_ID (B1).
pub fn valid_instance_id(s: &str) -> bool {
    s.strip_prefix(INSTANCE_PREFIX).is_some_and(|r| r.len() == 36 && r.chars().all(|c| c.is_ascii_digit() || matches!(c, 'A'..='F' | '-')))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The session is being restored into a workgroup.
    Restoring,
    /// No such workgroup, or one with no root.
    Unusable,
    /// The session is a non-leader member of this instance: only a leader switches workgroups.
    NonLeader(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    AlreadyEntered,
    Refused(Refusal),
    /// Go ahead; `exit_first` is the instance the leader is switching away from.
    Proceed { exit_first: Option<String> },
}

/// What entering would do (`WC:enterDisposition`, in its order). `current` is (instance id, its
/// workgroup id, this session is its leader).
pub fn disposition(workgroup: Option<&Workgroup>, current: Option<(&str, &str, bool)>, restoring: bool) -> Disposition {
    let id = workgroup.map(|w| w.unique_identifier.as_str());
    if let Some((instance, of, leader)) = current {
        if Some(of) == id {
            return Disposition::AlreadyEntered;
        }
        if !leader {
            return Disposition::Refused(Refusal::NonLeader(instance.to_string()));
        }
    }
    if restoring {
        return Disposition::Refused(Refusal::Restoring);
    }
    match workgroup {
        Some(w) if w.root().is_some() => Disposition::Proceed { exit_first: current.map(|(i, ..)| i.to_string()) },
        _ => Disposition::Refused(Refusal::Unusable),
    }
}

/// The menu and triggers enter only from a session in no workgroup and only when entering would
/// go ahead (`WC:canEnterFromUI`, `WC:145-150`).
pub fn can_enter_from_ui(in_workgroup: bool, d: &Disposition) -> bool {
    !in_workgroup && !matches!(d, Disposition::Refused(_))
}

/// Who started an entry; the provenance text records it (`WC:9-22`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    Trigger,
    BrowserTrigger,
    Menu,
    Onboarding,
}

impl Mechanism {
    pub fn provenance(self, session: &str) -> String {
        match self {
            Mechanism::Trigger => format!("A trigger in session {session} entered this workgroup."),
            Mechanism::BrowserTrigger => format!("A browser trigger in session {session} entered this workgroup."),
            Mechanism::Menu => format!("The user entered this workgroup from the Workgroups menu in session {session}."),
            Mechanism::Onboarding => format!("The user entered this workgroup by accepting the Claude Code onboarding prompt in session {session}."),
        }
    }

    fn from_trigger(self) -> bool {
        matches!(self, Mechanism::Trigger | Mechanism::BrowserTrigger)
    }
}

/// One session to start, in the order iTerm2 starts them (`WI:811-900`, `WSS:60-140`).
#[derive(Clone, Debug, PartialEq)]
pub enum SpawnStep {
    Peer { config: String },
    Split { config: String, parent: String, vertical: bool, before: bool, fraction: f64 },
    Tab { config: String, parent: String },
}

/// The peers of the root first, then the splits and tabs depth first in stored order; a host's own
/// peers follow the host.
pub fn spawn_plan(w: &Workgroup) -> Vec<SpawnStep> {
    fn walk(w: &Workgroup, parent: &str, out: &mut Vec<SpawnStep>) {
        let kids = w.children(parent);
        for k in kids.iter().filter(|k| matches!(k.kind, Kind::Peer)) {
            out.push(SpawnStep::Peer { config: k.unique_identifier.clone() });
        }
        for k in kids {
            let config = k.unique_identifier.clone();
            match &k.kind {
                Kind::Split(s) => {
                    let (vertical, before) = split_arguments(s);
                    out.push(SpawnStep::Split { config: config.clone(), parent: parent.to_string(), vertical, before, fraction: s.location });
                }
                Kind::Tab => out.push(SpawnStep::Tab { config: config.clone(), parent: parent.to_string() }),
                Kind::Root | Kind::Peer => continue,
            }
            walk(w, &config, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = w.root() {
        walk(w, &root.unique_identifier, &mut out);
    }
    out
}

/// (vertical, before) of a split (`WSS:173-200`): a vertical divider puts the panes side by side,
/// and the leading or top side puts the new pane first.
pub fn split_arguments(s: &SplitSettings) -> (bool, bool) {
    (s.orientation == wg::Orientation::Vertical, s.side == wg::Side::LeadingOrTop)
}

/// The non-peer members in the order they end (spawn order; the leader is never in it).
pub fn teardown_order(non_peers: &[String]) -> Vec<String> {
    non_peers.to_vec()
}

/// Why a workgroup member cannot be buried (None for any other session). One predicate for every
/// bury entry: the menu, the API, it2 and the keyboard all reach `bury_pane`.
pub fn bury_refusal(is_member: bool) -> Option<&'static str> {
    is_member.then_some("A session in a workgroup cannot be buried. Exit the workgroup first (Shell > Workgroups > Exit Workgroup).")
}

/// What a teardown does with the leader (`WI:666-790`, `restoreMainLeaderToPaneIfNeeded`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderFate {
    /// Put back in the slot the shown peer holds, then left running.
    SwapBack,
    /// Closed with the rest: its slot is the closing member's own, or nowhere to put it.
    Close,
    /// Left as it is (it is the one ending, or nothing was shown in its place).
    Stay,
}

/// The leader's fate when the teardown was started by `ending` (None: Exit Workgroup), with `shown`
/// the member in the leader's slot. A closing member that is the shown one is the pane whose own tab
/// or window is closing, so the leader is never revealed into it (iTerm2's guard
/// `activeSession !== terminating`); it goes with the group instead of being stranded hidden.
pub fn leader_fate(ending: Option<crate::layout::PaneId>, leader: crate::layout::PaneId, shown: Option<crate::layout::PaneId>) -> LeaderFate {
    match (ending, shown) {
        (Some(e), _) if e == leader => LeaderFate::Stay,
        (Some(e), Some(s)) if e == s && s != leader => LeaderFate::Close,
        (_, Some(s)) if s != leader => LeaderFate::SwapBack,
        _ => LeaderFate::Stay,
    }
}

/// The close confirmation's extra line (`WI:171-195`): each group is (workgroup name, live members
/// that would close beyond those already closing).
pub fn close_warning(groups: &[(String, usize)]) -> Option<String> {
    let groups: Vec<&(String, usize)> = groups.iter().filter(|g| g.1 > 0).collect();
    let extra: usize = groups.iter().map(|g| g.1).sum();
    match groups.as_slice() {
        [] => None,
        [one] => Some(format!("This will also exit the workgroup \u{201c}{}\u{201d} and close {extra} other sessions.", display_text(&one.0, 128, false))),
        many => Some(format!("This will also exit {} workgroups and close {extra} other sessions.", many.len())),
    }
}

/// Whether entering asks the person first. Only the person's own workgroup entered by the person
/// runs unasked (the trust ruling); an import of any kind, a trigger, and a workgroup of more than
/// eight members ask (P3, B8).
pub fn needs_ask(source: Source, how: Mechanism, members: usize) -> bool {
    !source.is_person() || how.from_trigger() || members > ASK_ABOVE
}

/// The entry question's buttons, left to right: Return presses the first, so the default is Cancel.
pub const ASK_LABELS: [&str; 2] = [crate::api_auth::CANCEL, "Enter"];
/// The button that enters (by an explicit click or its access key only).
pub const ASK_ENTER: usize = 1;

/// The question's heading and body: who is asking, and every command exactly as it would run, made
/// inert (no bidi override can disguise one).
/// Whole commands only: None when the text would pass `ASK_ROOM` (the question cannot show it all,
/// so it is not offered; Trust it in Settings after reading it there).
pub fn ask_text(w: &Workgroup, how: Mechanism) -> Option<(String, String)> {
    let heading = format!("Enter the workgroup \u{201c}{}\u{201d}?", display_text(wg::display_name(w), 128, false));
    let mut body = String::new();
    body.push_str(match how {
        Mechanism::Trigger | Mechanism::BrowserTrigger => "A trigger wants to enter this workgroup.",
        _ => "Entering starts a session for each member.",
    });
    body.push_str(&format!(" It has {} members. These commands will run:\n", w.sessions.len()));
    for s in &w.sessions {
        body.push_str(&format!("\n{}: ", role(s)));
        match (s.command.is_empty(), s.url_string.is_empty()) {
            (true, true) => body.push_str("the profile's own shell"),
            (true, false) => body.push_str(&format!("opens {}", visible_runs(&display_text(&s.url_string, usize::MAX, false)))),
            _ => body.push_str(&visible_runs(&display_text(&s.command, usize::MAX, false))),
        }
    }
    (body.chars().count() <= ASK_ROOM).then_some((heading, body))
}

/// The most characters the question's body holds, every command whole.
pub const ASK_ROOM: usize = 6000;

/// `text` with every run of four or more blanks written as "[N spaces]", so padding cannot push
/// part of a command out of sight.
fn visible_runs(text: &str) -> String {
    let mut out = String::new();
    let mut run = 0usize;
    let flush = |out: &mut String, run: &mut usize| {
        match *run {
            0 => {}
            1..=3 => out.push_str(&" ".repeat(*run)),
            n => out.push_str(&format!(" [{n} spaces] ")),
        }
        *run = 0;
    };
    for c in text.chars() {
        if c == ' ' || c == '\t' {
            run += 1;
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    out
}

fn role(s: &SessionConfig) -> String {
    let name = display_text(&s.display_name, 64, false);
    if !name.is_empty() {
        return name;
    }
    match s.kind {
        Kind::Root => "Main session".into(),
        Kind::Peer => "Peer".into(),
        Kind::Split(_) => "Split".into(),
        Kind::Tab => "Tab".into(),
    }
}

/// A member profile: a copy with the member's URL as `Initial URL` when the profile is a Browser
/// one and the URL is one a saved arrangement may keep (S1); anything else is the profile as is.
pub fn member_profile(profile: &Profile, url_string: &str) -> Profile {
    let mut p = profile.clone();
    if crate::browser::is_browser(&p.string("Custom Command")) {
        if let Some(url) = (!url_string.is_empty()).then(|| crate::browser::arrangement_url(url_string)).flatten() {
            p.set("Initial URL", toml::Value::String(url));
        }
    }
    p
}

/// How a member starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No command: the profile's own shell.
    Profile,
    /// A command with no expression, started as a profile's command is.
    Line(String),
    /// A command that held `\(gitBase)`: the person's template split into arguments first, then the
    /// base put inside its own token, so a value is always exactly one argument (B4).
    Argv(Vec<String>),
}

/// The longest base ref a command takes.
const MAX_BASE: usize = 256;

/// A base ref a command may take (B4): not empty, not an option, no control character, not blank.
pub fn base_ok(base: &str) -> bool {
    !base.trim().is_empty() && !base.starts_with('-') && base.chars().count() <= MAX_BASE && !base.chars().any(char::is_control)
}

/// The plan for a member's command with `git_base` for `\(gitBase)`. An error is the banner text.
pub fn command_plan(template: &str, git_base: &str) -> Result<Plan, String> {
    if template.trim().is_empty() {
        return Ok(Plan::Profile);
    }
    if !template.contains("\\(") {
        return Ok(Plan::Line(template.to_string()));
    }
    if template.contains("\\(gitBase)") && !base_ok(git_base) {
        return Err("The base for this workgroup's command is not a usable name, so nothing started.".to_string());
    }
    let tokens: Vec<String> = crate::semantic::split_command_line(template).into_iter().map(|t| t.replace("\\(gitBase)", git_base)).collect();
    if tokens.is_empty() || tokens.iter().any(|t| t.contains("\\(")) {
        return Err("A workgroup command uses an expression Tandem does not run here, so nothing started.".to_string());
    }
    Ok(Plan::Argv(tokens))
}

/// The command line for a program Tandem resolved and its arguments, made with CreateProcess's
/// rules. A batch file never gets an argument cmd.exe would read as syntax.
pub fn argv_line(program: &std::path::Path, args: &[String]) -> Result<String, String> {
    let ext = program.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "cmd" | "bat") && args.iter().any(|a| crate::semantic::batch_unsafe(a)) {
        return Err(crate::semantic::CMD_REFUSED.to_string());
    }
    let mut line = String::new();
    crate::ssh_probe::quote_arg(&program.to_string_lossy(), &mut line);
    for a in args {
        line.push(' ');
        crate::ssh_probe::quote_arg(a, &mut line);
    }
    Ok(line)
}

/// A Shell > Workgroups entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
}

/// The id prefix of an entry's menu id.
pub const MENU_ID: &str = "workgroup:";

/// The entries above the separator (`WMN:40-110`): "Untitled" for an empty name, disabled with no
/// current session or in any workgroup, the current workgroup's checked. `current` is the focused
/// session's instance.
pub fn menu_entries(list: &[Workgroup], current: Option<(&str, &str, bool)>, has_session: bool, restoring: bool) -> Vec<MenuEntry> {
    list.iter()
        .map(|w| {
            let shown = display_text(&w.name, 128, false);
            let label = if shown.is_empty() { "Untitled".to_string() } else { shown };
            let d = disposition(Some(w), current, restoring);
            MenuEntry { id: format!("{MENU_ID}{}", w.unique_identifier), label: label.replace('&', "&&"), enabled: has_session && can_enter_from_ui(current.is_some(), &d), checked: current.is_some_and(|(_, of, _)| of == w.unique_identifier) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// No path can bury a workgroup member: a buried member is a pane in `App.buried` that the group
    /// still owns (B2), and a buried shown peer takes the group's slot out of every layout.
    #[test]
    fn no_member_can_be_buried() {
        use super::bury_refusal;
        assert_eq!(bury_refusal(false), None);
        assert!(bury_refusal(true).is_some_and(|t| t.contains("workgroup")));
        let bury = include_str!("app/bury_ui.rs");
        let f = &bury[bury.find("pub(super) fn bury_pane").unwrap()..];
        let head = &f[..f.find("enter_pane_window").unwrap()];
        assert!(head.contains("crate::workgroup_run::bury_refusal(self.workgroup_of(id).is_some())"), "the one predicate is the first thing every bury entry meets");
        // Every entry point reaches bury_pane (the menu, the API, it2, the keyboard and the auto-bury).
        for (file, src) in [("menu.rs", include_str!("app/menu.rs")), ("api_requests.rs", include_str!("app/api_requests.rs"))] {
            assert!(src.contains("self.bury_pane("), "{file} buries through bury_pane");
        }
        let mut direct = Vec::new();
        for (file, src) in [("menu.rs", include_str!("app/menu.rs")), ("api_requests.rs", include_str!("app/api_requests.rs")), ("git_ui.rs", include_str!("app/git_ui.rs")), ("bury_ui.rs", include_str!("app/bury_ui.rs"))] {
            if src.contains("Burial::bury(") || src.contains(".buried.bury(") {
                direct.push(file);
            }
        }
        assert_eq!(direct, ["bury_ui.rs"], "only bury_pane inserts into the buried store");
    }

    #[test]
    fn leader_fate_follows_iterm2() {
        use super::{leader_fate, LeaderFate::*};
        assert_eq!(leader_fate(None, 1, Some(2)), SwapBack, "Exit Workgroup puts the leader back");
        assert_eq!(leader_fate(Some(3), 1, Some(2)), SwapBack, "another member ending");
        assert_eq!(leader_fate(Some(2), 1, Some(2)), Close, "the shown peer closing takes the leader with it");
        assert_eq!(leader_fate(Some(1), 1, Some(2)), Stay, "the leader is the one ending");
        assert_eq!(leader_fate(None, 1, Some(1)), Stay);
        assert_eq!(leader_fate(Some(2), 1, None), Stay);
        // The one teardown asks the rule and never strands a leader that is in no layout.
        let app = include_str!("app/workgroup_app.rs");
        assert!(app.contains("run::leader_fate(skip, inst.leader, shown)"));
        assert!(app.contains("run::LeaderFate::Close"));
        assert!(app.contains("never left hidden"), "a swap that failed still closes the leader");
    }

    use super::*;
    use crate::workgroup::{self as wg, Kind, Mode, Orientation, Side, SplitSettings, Workgroup};

    fn with_split(side: Side, orientation: Orientation, location: f64) -> Workgroup {
        let mut w = wg::presets::coding_agent_plus_diff();
        let root = w.root().unwrap().unique_identifier.clone();
        let mut split = w.sessions[1].clone();
        split.unique_identifier = "S1".into();
        split.parent_id = Some(root);
        split.kind = Kind::Split(SplitSettings { orientation, side, location });
        split.mode = Mode::Regular;
        let mut tab = split.clone();
        tab.unique_identifier = "T1".into();
        tab.parent_id = Some("S1".into());
        tab.kind = Kind::Tab;
        w.sessions = vec![w.sessions[0].clone(), split, tab];
        w
    }

    #[test]
    fn dispositions_follow_iterm2() {
        let w = wg::presets::coding_agent_plus_diff();
        let id = w.unique_identifier.as_str();
        assert_eq!(disposition(Some(&w), None, false), Disposition::Proceed { exit_first: None });
        assert_eq!(disposition(Some(&w), Some(("wg-1", id, true)), false), Disposition::AlreadyEntered);
        assert_eq!(disposition(Some(&w), Some(("wg-1", "other", false)), false), Disposition::Refused(Refusal::NonLeader("wg-1".into())));
        assert_eq!(disposition(Some(&w), Some(("wg-1", "other", true)), false), Disposition::Proceed { exit_first: Some("wg-1".into()) });
        assert_eq!(disposition(Some(&w), None, true), Disposition::Refused(Refusal::Restoring));
        assert_eq!(disposition(None, None, false), Disposition::Refused(Refusal::Unusable));
        let mut empty = w.clone();
        empty.sessions.clear();
        assert_eq!(disposition(Some(&empty), None, false), Disposition::Refused(Refusal::Unusable));
        // WC:145-150: the menu refuses while in any workgroup.
        let go = disposition(Some(&w), None, false);
        assert!(can_enter_from_ui(false, &go));
        assert!(!can_enter_from_ui(true, &go));
        assert!(!can_enter_from_ui(false, &Disposition::Refused(Refusal::Restoring)));
        assert!(can_enter_from_ui(false, &Disposition::AlreadyEntered));
    }

    #[test]
    fn spawn_plan_follows_iterm2() {
        let cc = wg::claude_code::config();
        assert_eq!(spawn_plan(&cc), vec![SpawnStep::Peer { config: wg::claude_code::DIFF.into() }, SpawnStep::Peer { config: wg::claude_code::REVIEW.into() }]);
        let w = with_split(Side::LeadingOrTop, Orientation::Vertical, 0.3);
        assert_eq!(
            spawn_plan(&w),
            vec![SpawnStep::Split { config: "S1".into(), parent: w.sessions[0].unique_identifier.clone(), vertical: true, before: true, fraction: 0.3 }, SpawnStep::Tab { config: "T1".into(), parent: "S1".into() }]
        );
        let s = SplitSettings { orientation: Orientation::Horizontal, side: Side::TrailingOrBottom, location: 0.5 };
        assert_eq!(split_arguments(&s), (false, false));
        let s = SplitSettings { orientation: Orientation::Vertical, side: Side::TrailingOrBottom, location: 0.5 };
        assert_eq!(split_arguments(&s), (true, false));
        let (a, b) = (mint_instance_id(), mint_instance_id());
        assert!(a.starts_with(INSTANCE_PREFIX));
        assert_ne!(a, b);
        assert!(valid_instance_id(&a));
        assert!(!valid_instance_id("wg-../x"));
        assert!(!valid_instance_id("wg-abc"));
    }

    #[test]
    fn teardown_follows_iterm2() {
        assert_eq!(teardown_order(&["a".to_string(), "b".to_string()]), ["a", "b"]);
        assert!(teardown_order(&[]).is_empty());
    }

    #[test]
    fn close_warning_follows_iterm2() {
        assert_eq!(close_warning(&[("Claude Code".into(), 2)]).as_deref(), Some("This will also exit the workgroup \u{201c}Claude Code\u{201d} and close 2 other sessions."));
        assert_eq!(close_warning(&[("a".into(), 1), ("b".into(), 3)]).as_deref(), Some("This will also exit 2 workgroups and close 4 other sessions."));
        assert_eq!(close_warning(&[]), None);
        assert_eq!(close_warning(&[("a".into(), 0)]), None);
        assert_eq!(Mechanism::Menu.provenance("ptys_1"), "The user entered this workgroup from the Workgroups menu in session ptys_1.");
        assert_eq!(Mechanism::Trigger.provenance("g"), "A trigger in session g entered this workgroup.");
        assert_eq!(Mechanism::BrowserTrigger.provenance("g"), "A browser trigger in session g entered this workgroup.");
        assert_eq!(Mechanism::Onboarding.provenance("g"), "The user entered this workgroup by accepting the Claude Code onboarding prompt in session g.");
    }

    #[test]
    fn member_profile_follows_iterm2() {
        use crate::profile::Profile;
        let mut browser = Profile::new("b");
        browser.set("Custom Command", toml::Value::String("Browser".into()));
        browser.set("Initial URL", toml::Value::String("https://own.example/".into()));
        assert_eq!(member_profile(&browser, "https://a.b").string("Initial URL"), "https://a.b");
        assert_eq!(member_profile(&browser, "").string("Initial URL"), "https://own.example/");
        // S1: only a URL the arrangement rules keep; a refused one leaves the profile's own.
        assert_eq!(member_profile(&browser, "javascript:alert(1)").string("Initial URL"), "https://own.example/");
        let mut term = Profile::new("t");
        term.set("Custom Command", toml::Value::String("No".into()));
        assert_eq!(member_profile(&term, "https://a.b"), term);
    }

    #[test]
    fn commands_are_argv_never_quoted_text() {
        // No template: the profile's own shell. No expression: as written, like a profile's command.
        assert_eq!(command_plan("", "HEAD"), Ok(Plan::Profile));
        assert_eq!(command_plan("   ", "HEAD"), Ok(Plan::Profile));
        assert_eq!(command_plan("claude --resume", "HEAD"), Ok(Plan::Line("claude --resume".into())));
        // The base goes inside its own token: one argument whatever it holds.
        assert_eq!(command_plan("git diff \\(gitBase)", "HEAD"), Ok(Plan::Argv(vec!["git".into(), "diff".into(), "HEAD".into()])));
        assert_eq!(command_plan("git diff --x=\\(gitBase) -- a", "main..dev"), Ok(Plan::Argv(vec!["git".into(), "diff".into(), "--x=main..dev".into(), "--".into(), "a".into()])));
        assert_eq!(command_plan("git diff \\(gitBase)", "a b"), Ok(Plan::Argv(vec!["git".into(), "diff".into(), "a b".into()])));
        // B4's value rules: refused, nothing starts.
        for bad in ["", "-x", "--output=a", "a\nb", "a\u{3}b", "   ", &"x".repeat(257)] {
            assert!(command_plan("git diff \\(gitBase)", bad).is_err(), "{bad:?}");
        }
        // An expression Tandem does not run here is refused whole, never half substituted.
        assert!(command_plan("echo \\(user.x)", "HEAD").is_err());
        // The line is made with CreateProcess rules; a batch target refuses unsafe arguments.
        let line = argv_line(std::path::Path::new("C:\\Program Files\\Git\\git.exe"), &["diff".into(), "a b".into(), "x\"y".into()]).unwrap();
        assert_eq!(crate::semantic::split_command_line(&line), ["C:\\Program Files\\Git\\git.exe", "diff", "a b", "x\"y"]);
        assert!(argv_line(std::path::Path::new("C:\\t\\x.cmd"), &["50%".into()]).is_err());
        assert!(argv_line(std::path::Path::new("C:\\t\\x.exe"), &["50%".into()]).is_ok());
    }

    #[test]
    fn workgroup_values_use_the_one_builder() {
        let src = include_str!("workgroup_run.rs");
        let (quote_fn, posix) = (format!("fn {}(", "quote"), format!("{}\\{}{}", "'", "'", "'"));
        assert!(!src.contains(&quote_fn), "no second quoter (B3)");
        assert!(!src.contains(&posix), "no hand-written POSIX quoting (B3)");
    }

    #[test]
    fn untrusted_entries_ask_first() {
        use crate::workgroup::Source;
        // The person's own workgroup from the menu runs as iTerm2 does.
        assert!(!needs_ask(Source::Person, Mechanism::Menu, 3));
        assert!(!needs_ask(Source::Person, Mechanism::Onboarding, 3));
        // An import (any import path) and a trigger always ask.
        for s in [Source::Import, Source::RemotePrefs, Source::DynamicProfile, Source::Stored] {
            assert!(needs_ask(s, Mechanism::Menu, 2), "{s:?}");
        }
        assert!(needs_ask(Source::Person, Mechanism::Trigger, 2));
        assert!(needs_ask(Source::Person, Mechanism::BrowserTrigger, 2));
        // Above eight members the person is asked too (B8).
        assert!(needs_ask(Source::Person, Mechanism::Menu, 9));
        assert!(!needs_ask(Source::Person, Mechanism::Menu, 8));
    }

    #[test]
    fn the_question_shows_the_exact_commands() {
        let mut w = wg::presets::coding_agent_plus_diff();
        w.name = "Evil\u{202e}gp".into();
        w.sessions[0].command = "claude --x".into();
        w.sessions[1].display_name = "Diff\nRow".into();
        let (heading, body) = ask_text(&w, Mechanism::Trigger).unwrap();
        assert!(heading.contains("Enter the workgroup"), "{heading}");
        assert!(!heading.contains('\u{202e}') && !body.contains('\u{202e}'), "text a file supplies is made inert");
        assert!(body.contains("claude --x"), "{body}");
        assert!(body.contains("git diff \\(gitBase)"), "{body}");
        assert!(body.contains("trigger"), "says who asked: {body}");
        let (_, menu) = ask_text(&w, Mechanism::Menu).unwrap();
        assert!(!menu.contains("trigger"));
    }

    /// No silent cut: a long command is shown to its last character, a long run of spaces is made
    /// visible, and a workgroup too long to show in full cannot be entered from the question.
    #[test]
    fn the_question_never_hides_command_text() {
        let mut w = wg::presets::coding_agent_plus_diff();
        let long = format!("cmd /c echo ok{}& calc.exe TAIL", " ".repeat(3900));
        assert!(long.len() < 4096);
        w.sessions[1].command = long;
        let (_, body) = ask_text(&w, Mechanism::Trigger).unwrap();
        assert!(body.contains("& calc.exe TAIL"), "the tail is shown");
        assert!(body.contains("3900 spaces"), "a whitespace run is visible: {}", &body[..200.min(body.len())]);
        assert!(!body.contains("      "), "no run of blanks is left to hide text");
        // Over the room the question has, Enter is not offered.
        let mut big = wg::presets::coding_agent_plus_diff_plus_code_review(None, None);
        for s in &mut big.sessions {
            s.command = "x".repeat(4000);
        }
        assert!(ask_text(&big, Mechanism::Trigger).is_none());
    }

    #[test]
    fn workgroups_menu_follows_iterm2() {
        let a = wg::presets::coding_agent_plus_diff();
        let mut b = wg::presets::coding_agent_plus_diff();
        b.name = String::new();
        let mut c = wg::presets::coding_agent_plus_diff();
        c.name = "R&D".into();
        let list = [a.clone(), b.clone(), c];
        let m = menu_entries(&list, None, true, false);
        assert_eq!(m.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Coding Agent + Diff", "Untitled", "R&&D"]);
        assert!(m.iter().all(|e| e.enabled && !e.checked));
        assert_eq!(m[0].id, format!("workgroup:{}", a.unique_identifier));
        // No current session: every entry disabled.
        assert!(menu_entries(&list, None, false, false).iter().all(|e| !e.enabled));
        // In a workgroup: disabled, the current one checked.
        let cur = Some(("wg-1", a.unique_identifier.as_str(), true));
        let m = menu_entries(&list, cur, true, false);
        assert!(m.iter().all(|e| !e.enabled));
        assert_eq!(m.iter().map(|e| e.checked).collect::<Vec<_>>(), [true, false, false]);
        // Restoring refuses.
        assert!(menu_entries(&list, None, true, true).iter().all(|e| !e.enabled));
        // None listed: no entries (the separator hides).
        assert!(menu_entries(&[], None, true, false).is_empty());
        // Labels are the names made inert and cut.
        let mut long = a.clone();
        long.name = "x".repeat(300);
        assert_eq!(menu_entries(&[long], None, true, false)[0].label.chars().count() <= 129, true);
    }
}
