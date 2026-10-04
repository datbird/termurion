//! A session's state, kind and shell rules (iTerm2's `WorkgroupIntrospection`), shared by the
//! Cockpit and the orchestration dispatcher.

use crate::tabstatus::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Working,
    Waiting,
    Unknown,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Waiting => "waiting",
            State::Unknown => "unknown",
        }
    }
}

/// The state a status states in words (lower-cased but not trimmed, as iTerm2 does).
fn spoken(st: &Status) -> Option<State> {
    match st.status.as_deref().map(str::to_lowercase).as_deref() {
        Some("idle") => Some(State::Idle),
        Some("working") => Some(State::Working),
        Some("waiting") => Some(State::Waiting),
        _ => None,
    }
}

/// An empty status is unknown; the three words; else working with an indicator, else idle.
pub fn state(st: &Status) -> State {
    if st.is_empty() {
        return State::Unknown;
    }
    spoken(st).unwrap_or(if st.indicator.is_some() { State::Working } else { State::Idle })
}

/// `state` without the fallback: unknown unless the status says the word.
pub fn reported_state(st: &Status) -> State {
    spoken(st).unwrap_or(State::Unknown)
}

pub fn status_source(st: &Status) -> &'static str {
    if spoken(st).is_some() { "reported" } else { "inferred" }
}

/// The Cockpit's keyword state: unknown reads as idle.
pub fn keyword_state(st: &Status) -> State {
    match state(st) {
        State::Unknown => State::Idle,
        other => other,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    ClaudeCode,
    Tui,
    Other,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "claude-code",
            Kind::Tui => "tui",
            Kind::Other => "other",
        }
    }
}

/// One rule for the kind: `ai_terminal::kind_of`, with "claude" found among the job's ancestors.
pub fn kind(ancestors: &[String], alt_screen: bool) -> Kind {
    let claude = ancestors.iter().any(|a| crate::proctree::same_job(a, "claude"));
    match crate::ai_terminal::kind_of(claude.then_some("claude"), alt_screen) {
        "claude-code" => Kind::ClaudeCode,
        "tui" => Kind::Tui,
        _ => Kind::Other,
    }
}

/// iTerm2's 17 shells plus Windows' cmd.
pub const SHELLS: [&str; 18] = ["bash", "zsh", "sh", "fish", "dash", "ash", "ksh", "mksh", "pdksh", "tcsh", "csh", "xonsh", "nu", "nushell", "elvish", "pwsh", "powershell", "cmd"];

/// Lower case, the login dash dropped, the file name only, no ".exe".
pub fn normalize_job(name: &str) -> String {
    let low = name.trim().to_lowercase();
    let base = low.trim_start_matches('-').rsplit(['/', '\\']).next().unwrap_or("");
    crate::proctree::strip_exe(base.trim_start_matches('-')).to_string()
}

/// Whether the foreground job is a shell; an unknown job is not.
pub fn is_shell(job: Option<&str>) -> bool {
    job.is_some_and(|j| SHELLS.contains(&normalize_job(j).as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(text: &str, indicator: bool) -> Status {
        Status { status: Some(text.into()), indicator: indicator.then(|| "#5f87ff".into()), ..Default::default() }
    }

    #[test]
    fn states_follow_iterm2() {
        assert_eq!(state(&Status::default()), State::Unknown);
        assert_eq!(state(&st("Working", false)), State::Working);
        assert_eq!(state(&st(" idle ", false)), State::Idle);
        assert_eq!(state(&st(" idle ", true)), State::Working);
        assert_eq!(reported_state(&st(" idle ", false)), State::Unknown);
        assert_eq!(state(&st("compiling", true)), State::Working);
        assert_eq!(state(&st("compiling", false)), State::Idle);
        assert_eq!(reported_state(&st("compiling", true)), State::Unknown);
        assert_eq!(status_source(&st("WAITING", false)), "reported");
        assert_eq!(status_source(&st("compiling", false)), "inferred");
        assert_eq!(status_source(&Status::default()), "inferred");
        assert_eq!(keyword_state(&Status::default()), State::Idle);
        assert_eq!(kind(&["claude".into()], false), Kind::ClaudeCode);
        assert_eq!(kind(&["node".into()], true), Kind::Tui);
        assert_eq!(kind(&[], false), Kind::Other);
        assert_eq!(Kind::ClaudeCode.word(), "claude-code");
        assert!(is_shell(Some("-bash")) && is_shell(Some("PWSH")) && is_shell(Some("cmd")));
        assert!(!is_shell(Some("vim")) && !is_shell(None));
        assert_eq!(SHELLS.len(), 18);
        assert_eq!(serde_json::to_string(&State::Waiting).unwrap(), "\"waiting\"");
    }
}
