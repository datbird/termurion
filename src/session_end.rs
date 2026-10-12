//! What happens when a session ends (milestone 13q): the end action, the outcome of an exit and
//! of a tmux detach (`PTYSession.m:4747-4822`, `iTermNaggingController.m:866-886`), the end
//! message line, the silence keys of the short-lived warning, and every text.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;

/// The profile key of the end action (`ITAddressBookMgr.h:219`): 0 No Action, 1 Close, 2 Restart.
pub const END_ACTION_KEY: &str = "Close Sessions On End";

/// The prefix of the per-profile silence key of the short-lived warning (`iTermPreferences.m:1326`).
pub const SHORT_LIVED_PREFIX: &str = "NeverWarnAboutShortLivedSessions_";

/// The silence key for the profile whose saved Guid is `saved_guid`.
pub fn short_lived_key(saved_guid: &str) -> String {
    format!("{SHORT_LIVED_PREFIX}{saved_guid}")
}

/// The key that stores the answer of the silenced warning (`warnings::selection_key`).
pub fn short_lived_selection_key(saved_guid: &str) -> String {
    crate::warnings::selection_key(&short_lived_key(saved_guid))
}

/// What a profile does when its session ends ("Close Sessions On End", `ITAddressBookMgr.h:414`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndAction {
    NoAction,
    Close,
    Restart,
}

impl EndAction {
    /// 0 and any unknown value are No Action, 1 Close, 2 Restart.
    pub fn from_int(v: i64) -> EndAction {
        match v {
            1 => EndAction::Close,
            2 => EndAction::Restart,
            _ => EndAction::NoAction,
        }
    }
}

/// What the exit loop knows about a session that just ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub action: EndAction,
    /// The session has a program to start again (a tmux client has none).
    pub restartable: bool,
    /// A git window's single-use session that lived under the short-lived limit.
    pub short_lived_single_use: bool,
}

/// The message written into the pane's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Ended,
    Finished,
    TmuxDetached,
}

/// What the pane does after the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Keep { offer_restart: bool },
    RestartAfter(Duration),
    Finished { close: bool },
}

/// `brokenPipeWithError:` (`PS:4747-4822`). `sessionShouldAutoClose` is always true here
/// (Decision: Tandem has no split drag a close could break).
pub fn after_exit(e: Exit, suppress_offer: bool) -> (Message, Outcome) {
    if e.short_lived_single_use {
        // iTerm2 never restarts a single-use session (`PS:4785-4795`).
        return (Message::Finished, Outcome::Finished { close: e.action == EndAction::Close });
    }
    let outcome = match e.action {
        EndAction::Close => Outcome::Close,
        EndAction::Restart if e.restartable => Outcome::RestartAfter(RESTART_DELAY),
        EndAction::Restart => Outcome::Keep { offer_restart: false },
        EndAction::NoAction => Outcome::Keep { offer_restart: e.restartable && !suppress_offer },
    };
    (Message::Ended, outcome)
}

/// What a detached tmux window does (`tmuxDidDisconnect`, `PS:4697-4725`; `NC:866-886`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TmuxAfter {
    Close,
    Keep,
    Ask,
}

/// Close closes; otherwise the Advanced answer (`TmuxWindowsShouldCloseAfterDetach`), else ask.
pub fn after_tmux_detach(action: EndAction, close_after: Option<bool>) -> TmuxAfter {
    if action == EndAction::Close {
        return TmuxAfter::Close;
    }
    match close_after {
        Some(true) => TmuxAfter::Close,
        Some(false) => TmuxAfter::Keep,
        None => TmuxAfter::Ask,
    }
}

/// The Restart action waits this long (`PS:4807-4812`).
pub const RESTART_DELAY: Duration = Duration::from_secs(1);
/// iTerm2's divider blue (`PS:4514-4596`).
pub const DIVIDER_RGB: (u8, u8, u8) = (70, 83, 246);
/// The divider when the character is empty (Decision: iTerm2 draws an image).
pub const DEFAULT_DIVIDER: &str = "\u{2500}";

/// The panes whose timed restart came due, sorted by id, and the earliest deadline still
/// waiting (`PS:4856-4864`).
pub fn restart_due(now: Instant, pending: &HashMap<crate::layout::PaneId, Instant>) -> (Vec<crate::layout::PaneId>, Option<Instant>) {
    let mut due: Vec<crate::layout::PaneId> = pending.iter().filter(|(_, at)| **at <= now).map(|(id, _)| *id).collect();
    due.sort_unstable();
    let next = pending.values().filter(|at| **at > now).min().copied();
    (due, next)
}

/// Tandem's bound on the Restart action (13q P11; iTerm2 has none): at most this many timed
/// restarts of one pane within `STORM_WINDOW`.
pub const STORM_LIMIT: usize = 5;
pub const STORM_WINDOW: Duration = Duration::from_secs(10);
/// The bar shown when the bound stops a restart.
pub const KEEPS_ENDING: &str = "Session keeps ending. Restart it?";

/// Whether a pane that restarted at `history` may be restarted again at `now`.
pub fn restart_allowed(history: &[Instant], now: Instant) -> bool {
    history.iter().filter(|t| now.saturating_duration_since(**t) < STORM_WINDOW).count() < STORM_LIMIT
}

/// The words of the restart message (`PS:4779`): the custom end text replaces it as it
/// replaces every message (`PS:4525`), else the Advanced "restarted" text.
pub fn restarted_text(custom_end: &str, restarted: &str) -> String {
    if !custom_end.is_empty() {
        custom_end.to_string()
    } else if restarted.is_empty() {
        SESSION_RESTARTED_DEFAULT.to_string()
    } else {
        restarted.to_string()
    }
}

pub const SESSION_ENDED: &str = "Session Ended";
pub const TMUX_DETACHED: &str = "tmux detached";
pub const SESSION_RESTARTED_DEFAULT: &str = "Session Restarted";
pub const FINISHED_DEFAULT: &str = "Finished";
pub const RESTART_OFFER: &str = "Session ended (command exited). Restart it?";
pub const FAILED_TO_START: &str = "Session failed to start.";

/// The words of the bar an ended session offers (`sessionEndedWithExecFailure:`, `NC:318-327`): a
/// command that never started says so; otherwise the restart question.
pub fn offer_text(exec_failed: bool) -> &'static str {
    if exec_failed { FAILED_TO_START } else { RESTART_OFFER }
}
pub const RESTART: &str = "Restart";
pub const DONT_ASK_AGAIN: &str = "Don\u{2019}t Ask Again";
pub const TMUX_CLOSE_QUESTION: &str = "Close tmux windows after detaching?";
pub const ALWAYS: &str = "Always";
pub const NEVER: &str = "Never";
pub const RESTART_BAR: &str = "session-end-restart";
/// The bar of a pane that keeps ending (P11): its own, so its buttons never reach the restart
/// offer's "Don't Ask Again".
pub const STORM_BAR: &str = "session-end-storm";
pub const TMUX_CLOSE_BAR: &str = "session-end-tmux-close";

/// The words of the end message: the custom text (Advanced) when set, for every message as
/// iTerm2 does (`appendBrokenPipeMessage:`, `PS:4525`); else the message's own words, `finished`
/// being the Advanced "finished" text.
pub fn message_text(m: Message, globals_custom_end: &str, finished: &str) -> String {
    match m {
        _ if !globals_custom_end.is_empty() => globals_custom_end.to_string(),
        Message::Ended => SESSION_ENDED.to_string(),
        Message::Finished => finished.to_string(),
        Message::TmuxDetached => TMUX_DETACHED.to_string(),
    }
}

/// The whole divider line for a pane `width` cells wide: a run of `divider`, " text ", a run
/// that fills the line (`PS:4514-4596`). A divider's width is at least 1; an empty one is U+2500.
pub fn end_line_with(width: usize, text: &str, divider: &str, measure: impl Fn(&str) -> usize) -> String {
    let divider = if divider.is_empty() { DEFAULT_DIVIDER } else { divider };
    let padded = format!(" {text} ");
    let text_w = measure(&padded);
    let div_w = measure(divider).max(1);
    let left = (width.saturating_sub(text_w) / 2) / div_w;
    let used = left * div_w + text_w;
    let right = if width == 0 { 0 } else { ((width - used % width) % width) / div_w };
    format!("{}{padded}{}", divider.repeat(left), divider.repeat(right))
}

/// `end_line` measured by Unicode widths (the grid measures with its own rule: `end_line_with`).
pub fn end_line(width: usize, text: &str, divider: &str) -> String {
    end_line_with(width, text, divider, UnicodeWidthStr::width)
}

/// The heading of the short-lived warning: none is given, so `iTermWarning`'s "Warning"
/// (`iTermWarning.m:330`).
pub const SHORT_LIVED_HEADING: &str = "Warning";

/// Whether a session that lived `lived` ended too soon (`PS:3748`). A limit that is not a
/// finite, positive number is 0: nothing is short-lived.
pub fn short_lived(lived: Duration, limit_seconds: f64) -> bool {
    let limit = if limit_seconds.is_finite() && limit_seconds > 0.0 { limit_seconds } else { 0.0 };
    lived.as_secs_f64() < limit
}

/// The warning's text for the profile named `profile_name` (`PS:3805`).
pub fn short_lived_text(profile_name: &str) -> String {
    format!("A session ended very soon after starting. Check that the command in profile \u{201c}{profile_name}\u{201d} is correct.")
}

/// The warning as an alert: OK, permanently silenceable under the saved profile's key
/// (`iTermWarning` kind permanent, so the box reads "Suppress this message permanently").
pub fn short_lived_spec(saved_guid: &str, profile_name: &str) -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(short_lived_key(saved_guid)),
        kind: crate::warnings::Kind::Permanent,
        heading: SHORT_LIVED_HEADING.to_string(),
        text: short_lived_text(profile_name),
        labels: vec![crate::api_auth::OK.to_string()],
        cancel: None,
        never_remember: Vec::new(),
    }
}

/// What the close of a pane knows when it decides to warn.
#[derive(Clone, Copy, Debug)]
pub struct CloseFacts {
    pub exited: bool,
    pub single_use: bool,
    pub tmux: bool,
    pub read_only: bool,
    pub hold_open: bool,
    pub archived: bool,
    /// The program sent `ExecFailed`: the person must be able to read the error (`PS:3896`).
    pub exec_failed: bool,
    pub lived: Duration,
    pub limit_seconds: f64,
}

/// Whether closing the pane warns (`PS:3748-3822`, `PS:3887-3898`): an ended session that is not
/// single-use, tmux, a recording, a kept-open pane or an archive, and lived under the limit.
pub fn short_lived_due(f: CloseFacts) -> bool {
    f.exited && !f.single_use && !f.tmux && !f.read_only && !f.hold_open && !f.archived && !f.exec_failed && short_lived(f.lived, f.limit_seconds)
}

/// The write of the Settings check box "Warn about short-lived sessions" (`PSV:391-446`):
/// unchecked silences the saved profile's warning for good (key true, answer 0); checked forgets
/// the key, its answer and its silence time (`unsilenceIdentifier:`, `warnings::clear_saved`).
pub fn set_short_lived_warning(s: &mut dyn crate::warnings::Store, saved_guid: &str, warn: bool) {
    let key = short_lived_key(saved_guid);
    if warn {
        crate::warnings::clear_saved(s, &key);
    } else {
        s.set(&key, toml::Value::Boolean(true));
        s.set(&crate::warnings::selection_key(&key), toml::Value::Integer(0));
    }
}

/// How a restart was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// Session > Restart Session.
    Menu,
    /// The offer bar's Restart button.
    Bar,
    /// The Restart end action's timer.
    Timer,
    /// The object API's RestartSession and the AI tool.
    Api,
}

/// What a restart does first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestartStep {
    /// Restart now.
    Now,
    /// A running shell: "Restart session?" (`PT:2855`).
    AskRunningJobs,
    /// Programs the ended shell left still run, and the restart would end them: ask, naming them.
    AskSurvivors,
    /// The timer never ends them unasked: show the restart offer instead of restarting.
    Offer,
    /// The pane has no program of its own to start again (`isRestartable`): nothing happens.
    Refused,
}

/// The first step of a restart (13q B7, P8: a restart never silently ends running programs).
pub fn restart_step(via: Via, exited: bool, survivors: usize) -> RestartStep {
    if !exited {
        return if via == Via::Menu { RestartStep::AskRunningJobs } else { RestartStep::Now };
    }
    match (survivors, via) {
        (0, _) => RestartStep::Now,
        (_, Via::Timer) => RestartStep::Offer,
        _ => RestartStep::AskSurvivors,
    }
}

/// The first step of a restart of a pane that may have nothing to restart: a single-use window
/// (git, Run Command in Window), a kept-open pane, a tmux client or a recording is refused on
/// every route before any question is asked (13q x 13r merge).
pub fn restart_first(restartable: bool, via: Via, exited: bool, survivors: usize) -> RestartStep {
    if !restartable {
        return RestartStep::Refused;
    }
    restart_step(via, exited, survivors)
}

/// Which shell a "Restart session?" question was asked about: the pane's shell generation and
/// the programs an ended shell left (the question named them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskedAbout {
    pub generation: u64,
    pub survivors: Vec<String>,
}

/// What an answered OK does (a question can be answered long after it was asked).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerStep {
    /// The shell is the one asked about: restart.
    Restart,
    /// That shell is gone or changed (it exited and restarted, or left other programs): the
    /// answer is not for the pane as it is now, so the restart asks again about it.
    AskAgain,
}

pub fn answer_step(asked: &AskedAbout, now: &AskedAbout) -> AnswerStep {
    if asked == now {
        AnswerStep::Restart
    } else {
        AnswerStep::AskAgain
    }
}

/// The question asked before a restart ends the programs an ended shell left. Never silenceable.
pub fn restart_survivors_spec(names: &[String]) -> crate::warnings::Spec {
    let text = if names.len() == 1 {
        format!("1 program is still running: {}. Restarting ends it.", names[0])
    } else {
        format!("{} programs are still running: {}. Restarting ends them.", names.len(), names.join(", "))
    };
    crate::warnings::Spec {
        id: None,
        kind: crate::warnings::Kind::Persistent,
        heading: "Restart session?".to_string(),
        text,
        labels: vec![crate::api_auth::OK.to_string(), crate::api_auth::CANCEL.to_string()],
        cancel: None,
        never_remember: Vec::new(),
    }
}

/// Whether WM_ENDSESSION ends the hosted shells (`iTermController.m:1746-1751`): both
/// `KillSessionsOnLogout` and `runJobsInServers` on, and the session really ending.
pub fn kill_on_logout(key: bool, run_jobs_in_servers: bool, ending: bool) -> bool {
    key && run_jobs_in_servers && ending
}

/// The folder a restarted shell starts in (P13): the folder it first started in while that is
/// still a local folder (13p's probe: never a network path), else the profile's rule with no
/// previous session (a restart never reads OSC 7 or the focused pane; Recycle then is home).
pub fn restart_dir(start_dir: Option<&std::path::Path>, p: &crate::profile::Profile, is_dir: &dyn Fn(&std::path::Path) -> bool) -> (std::path::PathBuf, Option<String>) {
    match start_dir.filter(|d| crate::start_dir::probe_ok(d, is_dir)) {
        Some(d) => (d.to_path_buf(), None),
        None => crate::profile::start_dir_probing(p, None, is_dir),
    }
}

#[cfg(test)]
mod tests {
    /// An old "Restart session?" answer never ends a new shell: the shell it was asked about must
    /// still be the pane's, with the same programs left.
    #[test]
    fn a_stale_restart_answer_asks_again() {
        use super::*;
        let asked = AskedAbout { generation: 3, survivors: vec![] };
        assert_eq!(answer_step(&asked, &asked.clone()), AnswerStep::Restart, "the same shell");
        // Order one: the shell exited (generation 4), the timer restarted it (5).
        assert_eq!(answer_step(&asked, &AskedAbout { generation: 5, survivors: vec![] }), AnswerStep::AskAgain, "a new shell");
        // Order two: the shell exited and the person answers over the ended shell.
        assert_eq!(answer_step(&asked, &AskedAbout { generation: 4, survivors: vec![] }), AnswerStep::AskAgain, "the shell exited since the question");
        // Programs appeared (or went) after the survivors question named them.
        let named = AskedAbout { generation: 4, survivors: vec!["a.exe".into()] };
        assert_eq!(answer_step(&named, &named.clone()), AnswerStep::Restart);
        assert_eq!(answer_step(&named, &AskedAbout { generation: 4, survivors: vec!["a.exe".into(), "b.exe".into()] }), AnswerStep::AskAgain, "a program the question did not name");
    }

    use super::*;

    fn exit(action: EndAction, restartable: bool, single: bool) -> Exit {
        Exit { action, restartable, short_lived_single_use: single }
    }

    #[test]
    fn after_exit_follows_iterm2() {
        use EndAction::*;
        assert_eq!((EndAction::from_int(0), EndAction::from_int(1), EndAction::from_int(2), EndAction::from_int(7)), (NoAction, Close, Restart, NoAction));
        assert_eq!(after_exit(exit(Close, true, false), false), (Message::Ended, Outcome::Close));
        assert_eq!(after_exit(exit(NoAction, true, false), false), (Message::Ended, Outcome::Keep { offer_restart: true }));
        assert_eq!(after_exit(exit(NoAction, true, false), true), (Message::Ended, Outcome::Keep { offer_restart: false }));
        assert_eq!(after_exit(exit(NoAction, false, false), false), (Message::Ended, Outcome::Keep { offer_restart: false }));
        assert_eq!(after_exit(exit(Restart, true, false), false), (Message::Ended, Outcome::RestartAfter(Duration::from_secs(1))));
        assert_eq!(after_exit(exit(Restart, false, false), false), (Message::Ended, Outcome::Keep { offer_restart: false }));
        assert_eq!(after_exit(exit(Close, true, true), false), (Message::Finished, Outcome::Finished { close: true }));
        assert_eq!(after_exit(exit(NoAction, true, true), false), (Message::Finished, Outcome::Finished { close: false }));
        assert_eq!(after_exit(exit(Restart, true, true), false), (Message::Finished, Outcome::Finished { close: false }));
    }

    #[test]
    fn kill_on_logout_follows_iterm2() {
        assert!(kill_on_logout(true, true, true));
        assert!(!kill_on_logout(false, true, true), "the key is off");
        assert!(!kill_on_logout(true, false, true), "no jobs run in the host");
        assert!(!kill_on_logout(true, true, false), "the logoff was cancelled (wParam zero)");
    }

    #[test]
    fn tmux_detach_follows_iterm2() {
        use EndAction::*;
        assert_eq!(after_tmux_detach(Close, None), TmuxAfter::Close);
        assert_eq!(after_tmux_detach(Close, Some(false)), TmuxAfter::Close);
        assert_eq!(after_tmux_detach(NoAction, Some(true)), TmuxAfter::Close);
        assert_eq!(after_tmux_detach(NoAction, Some(false)), TmuxAfter::Keep);
        assert_eq!(after_tmux_detach(Restart, None), TmuxAfter::Ask);
        assert_eq!(after_tmux_detach(NoAction, None), TmuxAfter::Ask);
    }

    #[test]
    fn end_line_follows_iterm2() {
        assert_eq!(message_text(Message::Ended, "", "Finished"), "Session Ended");
        assert_eq!(message_text(Message::Ended, "Bye", "Finished"), "Bye");
        // iTerm2 applies the custom text inside appendBrokenPipeMessage: (PS:4525), so it
        // replaces every message; empty, each message has its own words.
        assert_eq!(message_text(Message::Finished, "Bye", "Done"), "Bye");
        assert_eq!(message_text(Message::TmuxDetached, "Bye", "Done"), "Bye");
        assert_eq!(message_text(Message::Finished, "", "Done"), "Done");
        assert_eq!(message_text(Message::TmuxDetached, "", "Done"), "tmux detached");
        assert_eq!(end_line(20, "Session Ended", "="), "== Session Ended ===");
        assert_eq!(end_line(20, "Session Ended", ""), "\u{2500}\u{2500} Session Ended \u{2500}\u{2500}\u{2500}");
        assert_eq!(end_line(20, "Session Ended", "\u{ff1d}"), "\u{ff1d} Session Ended \u{ff1d}");
        assert_eq!(end_line(10, "Session Ended", "="), " Session Ended =====");
        assert_eq!(end_line(15, "Session Ended", "="), " Session Ended ");
    }

    #[test]
    fn end_line_follows_the_grid_width() {
        // With Ambiguous Double Width each U+2500 takes two cells: 20 cells hold 2 + 3 copies
        // of a 2-cell divider around the 15-cell message, not 5 + 5 cells.
        let wide = |s: &str| s.chars().map(|c| if c == '\u{2500}' { 2 } else { 1 }).sum::<usize>();
        assert_eq!(end_line_with(20, "Session Ended", "", wide), "\u{2500} Session Ended \u{2500}");
        let narrow = |s: &str| s.chars().count();
        assert_eq!(end_line_with(20, "Session Ended", "", narrow), "\u{2500}\u{2500} Session Ended \u{2500}\u{2500}\u{2500}");
    }

    #[derive(Default)]
    struct MapStore(std::collections::BTreeMap<String, toml::Value>);
    impl crate::warnings::Store for MapStore {
        fn get(&self, key: &str) -> Option<toml::Value> {
            self.0.get(key).cloned()
        }
        fn set(&mut self, key: &str, v: toml::Value) {
            self.0.insert(key.to_string(), v);
        }
        fn remove(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    #[test]
    fn short_lived_follows_iterm2() {
        let s = Duration::from_secs;
        assert!(short_lived(s(2), 3.0));
        assert!(!short_lived(s(3), 3.0));
        assert!(!short_lived(s(2), 0.0));
        assert!(!short_lived(s(2), f64::NAN));
        assert!(!short_lived(s(2), f64::INFINITY));
        assert!(!short_lived(s(2), -1.0));
        assert_eq!(short_lived_text("Default"), "A session ended very soon after starting. Check that the command in profile \u{201c}Default\u{201d} is correct.");
        assert_eq!(SHORT_LIVED_HEADING, "Warning");
    }

    #[test]
    fn short_lived_key_follows_the_saved_profile() {
        use crate::warnings::{checkbox_label, preempt, Kind, Preempt};
        let spec = short_lived_spec("G", "Default");
        assert_eq!(spec.id.as_deref(), Some("NeverWarnAboutShortLivedSessions_G"));
        assert_eq!(spec.kind, Kind::Permanent);
        assert_eq!(spec.labels, vec!["OK".to_string()]);
        assert_eq!(spec.cancel, None);
        assert_eq!(spec.heading, "Warning");
        assert_eq!(checkbox_label(&spec), Some("Suppress this message permanently"));
        let mut store = MapStore::default();
        assert_eq!(preempt(&spec, &store, 0.0, false), Preempt::Show);
        use crate::warnings::Store;
        store.set("NeverWarnAboutShortLivedSessions_G", toml::Value::Boolean(true));
        store.set("NeverWarnAboutShortLivedSessions_G_selection", toml::Value::Integer(0));
        assert_eq!(preempt(&spec, &store, 0.0, false), Preempt::Answer(0));
    }

    #[test]
    fn short_lived_is_due_only_for_a_plain_ended_session() {
        let base = CloseFacts { exited: true, single_use: false, tmux: false, read_only: false, hold_open: false, archived: false, exec_failed: false, lived: Duration::from_secs(1), limit_seconds: 3.0 };
        assert!(short_lived_due(base));
        assert!(!short_lived_due(CloseFacts { exited: false, ..base }));
        assert!(!short_lived_due(CloseFacts { single_use: true, ..base }));
        assert!(!short_lived_due(CloseFacts { tmux: true, ..base }));
        assert!(!short_lived_due(CloseFacts { read_only: true, ..base }));
        assert!(!short_lived_due(CloseFacts { hold_open: true, ..base }));
        assert!(!short_lived_due(CloseFacts { archived: true, ..base }));
        assert!(!short_lived_due(CloseFacts { exec_failed: true, ..base }), "a failed exec keeps the error readable");
        assert!(!short_lived_due(CloseFacts { lived: Duration::from_secs(3), ..base }));
    }

    #[test]
    fn the_settings_check_box_writes_what_iterm2_does() {
        use crate::warnings::Store;
        use toml::Value;
        let mut store = MapStore::default();
        let key = "NeverWarnAboutShortLivedSessions_G";
        set_short_lived_warning(&mut store, "G", false);
        assert_eq!(store.get(key), Some(Value::Boolean(true)));
        assert_eq!(store.get(&format!("{key}_selection")), Some(Value::Integer(0)));
        // A stale silence time from an older run goes too when the box is checked again.
        store.set(&format!("{key}_SilenceUntil"), Value::Float(9.0));
        set_short_lived_warning(&mut store, "G", true);
        assert!(store.0.is_empty(), "{:?}", store.0);
        // Another profile's key is untouched.
        store.set("NeverWarnAboutShortLivedSessions_H", Value::Boolean(true));
        set_short_lived_warning(&mut store, "G", true);
        assert_eq!(store.get("NeverWarnAboutShortLivedSessions_H"), Some(Value::Boolean(true)));
    }

    #[test]
    fn every_restart_route_counts_the_survivors() {
        use RestartStep::*;
        use Via::*;
        // Nothing left: an ended pane restarts at once; a running shell asks from the menu only.
        for via in [Menu, Bar, Timer, Api] {
            assert_eq!(restart_step(via, true, 0), Now, "{via:?}");
        }
        assert_eq!(restart_step(Menu, false, 0), AskRunningJobs);
        assert_eq!(restart_step(Api, false, 0), Now);
        // Programs left: every route asks, except the timer, which shows the offer.
        assert_eq!(restart_step(Menu, true, 2), AskSurvivors);
        assert_eq!(restart_step(Bar, true, 1), AskSurvivors);
        assert_eq!(restart_step(Api, true, 1), AskSurvivors);
        assert_eq!(restart_step(Timer, true, 1), Offer);
    }

    #[test]
    fn the_survivors_question_cannot_be_silenced() {
        use crate::warnings::{checkbox_label, Kind};
        let spec = restart_survivors_spec(&["a.exe".to_string(), "b.exe".to_string()]);
        assert_eq!(spec.heading, "Restart session?");
        assert_eq!(spec.text, "2 programs are still running: a.exe, b.exe. Restarting ends them.");
        assert_eq!(spec.labels, vec!["OK".to_string(), "Cancel".to_string()]);
        assert_eq!((spec.id.clone(), spec.kind), (None, Kind::Persistent));
        assert_eq!(checkbox_label(&spec), None);
        assert_eq!(restart_survivors_spec(&["a.exe".to_string()]).text, "1 program is still running: a.exe. Restarting ends it.");
    }

    /// TWEAKS 4b: `ExecFailed` draws iTerm2's "Session failed to start." in place of the restart question.
    #[test]
    fn a_failed_exec_has_its_own_message() {
        assert_eq!(offer_text(true), "Session failed to start.");
        assert_eq!(offer_text(false), "Session ended (command exited). Restart it?");
    }

    #[test]
    fn restart_offer_follows_iterm2() {
        assert_eq!(RESTART_OFFER, "Session ended (command exited). Restart it?");
        assert_eq!(RESTART, "Restart");
        assert_eq!(DONT_ASK_AGAIN, "Don\u{2019}t Ask Again");
        assert_eq!(FAILED_TO_START, "Session failed to start.");
        assert_eq!(SESSION_RESTARTED_DEFAULT, "Session Restarted");
        assert_eq!(FINISHED_DEFAULT, "Finished");
    }

    #[test]
    fn restart_due_follows_iterm2() {
        let t0 = Instant::now();
        let mut pending: HashMap<crate::layout::PaneId, Instant> = HashMap::new();
        pending.insert(2, t0 + Duration::from_secs(1));
        pending.insert(1, t0 + Duration::from_millis(500));
        assert_eq!(restart_due(t0 + Duration::from_millis(600), &pending), (vec![1], Some(t0 + Duration::from_secs(1))));
        pending.remove(&1);
        assert_eq!(restart_due(t0 + Duration::from_secs(1), &pending), (vec![2], None));
        assert_eq!(restart_due(t0, &HashMap::new()), (vec![], None));
    }

    #[test]
    fn restart_storm_is_bounded() {
        let t0 = Instant::now();
        let five: Vec<Instant> = (0..5).map(|i| t0 + Duration::from_secs(i)).collect();
        assert!(restart_allowed(&five[..4], t0 + Duration::from_secs(5)));
        assert!(!restart_allowed(&five, t0 + Duration::from_secs(5)));
        assert!(restart_allowed(&five, t0 + Duration::from_secs(14)), "old restarts fall out of the window");
        assert_eq!(restarted_text("", "Back"), "Back");
        assert_eq!(restarted_text("", ""), "Session Restarted");
        assert_eq!(restarted_text("Bye", "Back"), "Bye");
    }
    /// P13 (13p seam S3): a restart reuses the pane's own start folder, never another pane's,
    /// and a gone or network folder falls back to the profile's rule without a previous session.
    #[test]
    fn restart_reuses_the_start_folder() {
        use std::path::{Path, PathBuf};
        let mut p = crate::profile::Profile::default();
        p.set("Custom Directory", toml::Value::String("Recycle".into()));
        let all = |_: &Path| true;
        assert_eq!(restart_dir(Some(Path::new("D:\\work")), &p, &all).0, PathBuf::from("D:\\work"));
        let home = crate::profile::home();
        assert_eq!(restart_dir(Some(Path::new("\\\\host\\share")), &p, &all).0, home, "a network folder is never probed or used");
        assert_eq!(restart_dir(Some(Path::new("D:\\gone")), &p, &|_| false).0, home, "a gone folder: Recycle with no previous session is home");
        assert_eq!(restart_dir(None, &p, &all).0, home);
        p.set("Custom Directory", toml::Value::String("Yes".into()));
        p.set("Working Directory", toml::Value::String("D:\\proj".into()));
        assert_eq!(restart_dir(Some(Path::new("D:\\gone")), &p, &|d| d == Path::new("D:\\proj")).0, PathBuf::from("D:\\proj"));
        let src = include_str!("app/mod.rs");
        let spawn = &src[src.find("    fn spawn_pane(").unwrap()..];
        let spawn = &spawn[..spawn.find("\n    }\n").unwrap()];
        assert!(spawn.contains("pane.start_dir = Some(cwd);"), "every spawned shell records its folder");
        assert!(spawn.contains("_ if restart_dir.is_some() => crate::session_end::restart_dir(restart_dir"), "a restart never reads the focused pane");
    }

    /// 13q x 13r merge: a pane with no program of its own to start again (a git or Run Command
    /// in Window single-use window, a kept-open pane, a tmux client, a recording) is refused on
    /// every route before any question: no "Restart session?" whose OK does nothing, no offer.
    #[test]
    fn a_pane_that_cannot_restart_is_refused_on_every_route() {
        use Via::*;
        for via in [Menu, Bar, Timer, Api] {
            for exited in [false, true] {
                for left in [0, 2] {
                    assert_eq!(restart_first(false, via, exited, left), RestartStep::Refused, "{via:?} {exited} {left}");
                    assert_eq!(restart_first(true, via, exited, left), restart_step(via, exited, left), "{via:?} {exited} {left}");
                }
            }
        }
    }
}
