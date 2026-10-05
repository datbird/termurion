//! What happens when a session ends (milestone 13q): the end action, the outcome of an exit and
//! of a tmux detach (`PTYSession.m:4747-4822`, `iTermNaggingController.m:866-886`), the end
//! message line, the silence keys of the short-lived warning, and every text.

use std::time::Duration;

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

pub const SESSION_ENDED: &str = "Session Ended";
pub const TMUX_DETACHED: &str = "tmux detached";
pub const SESSION_RESTARTED_DEFAULT: &str = "Session Restarted";
pub const FINISHED_DEFAULT: &str = "Finished";
pub const RESTART_OFFER: &str = "Session ended (command exited). Restart it?";
pub const FAILED_TO_START: &str = "Session failed to start.";
pub const RESTART: &str = "Restart";
pub const DONT_ASK_AGAIN: &str = "Don\u{2019}t Ask Again";
pub const TMUX_CLOSE_QUESTION: &str = "Close tmux windows after detaching?";
pub const ALWAYS: &str = "Always";
pub const NEVER: &str = "Never";
pub const RESTART_BAR: &str = "session-end-restart";
pub const TMUX_CLOSE_BAR: &str = "session-end-tmux-close";

/// The words of the end message: the custom text (Advanced) for an ended session, `finished`
/// (the Advanced "finished" text) for a single-use one.
pub fn message_text(m: Message, globals_custom_end: &str, finished: &str) -> String {
    match m {
        Message::Ended if !globals_custom_end.is_empty() => globals_custom_end.to_string(),
        Message::Ended => SESSION_ENDED.to_string(),
        Message::Finished => finished.to_string(),
        Message::TmuxDetached => TMUX_DETACHED.to_string(),
    }
}

/// The whole divider line for a pane `width` cells wide: a run of `divider`, " text ", a run
/// that fills the line (`PS:4514-4596`). A divider's width is at least 1; an empty one is U+2500.
pub fn end_line(width: usize, text: &str, divider: &str) -> String {
    let divider = if divider.is_empty() { DEFAULT_DIVIDER } else { divider };
    let padded = format!(" {text} ");
    let text_w = padded.width();
    let div_w = divider.width().max(1);
    let left = (width.saturating_sub(text_w) / 2) / div_w;
    let used = left * div_w + text_w;
    let right = if width == 0 { 0 } else { ((width - used % width) % width) / div_w };
    format!("{}{padded}{}", divider.repeat(left), divider.repeat(right))
}

#[cfg(test)]
mod tests {
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
        assert_eq!(message_text(Message::Finished, "Bye", "Done"), "Done");
        assert_eq!(message_text(Message::TmuxDetached, "Bye", "Done"), "tmux detached");
        assert_eq!(end_line(20, "Session Ended", "="), "== Session Ended ===");
        assert_eq!(end_line(20, "Session Ended", ""), "\u{2500}\u{2500} Session Ended \u{2500}\u{2500}\u{2500}");
        assert_eq!(end_line(20, "Session Ended", "\u{ff1d}"), "\u{ff1d} Session Ended \u{ff1d}");
        assert_eq!(end_line(10, "Session Ended", "="), " Session Ended =====");
        assert_eq!(end_line(15, "Session Ended", "="), " Session Ended ");
    }
}
