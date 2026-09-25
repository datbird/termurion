//! Event triggers (iTerm2's match types 100 to 115): which events exist, their filters, and
//! the strings they capture.

use std::time::{Duration, Instant, SystemTime};

use crate::screen::Screen;
use crate::triggers;

/// iTerm2's events, in match-type order from 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    PromptDetected,
    CommandFinished,
    DirectoryChanged,
    HostChanged,
    UserChanged,
    Idle,
    ActivityAfterIdle,
    SessionEnded,
    BellReceived,
    LongRunningCommand,
    CustomEscapeSequence,
    NotificationPosted,
    ProgressBarChanged,
    JobStarted,
    JobEnded,
    VariableChanged,
}

impl EventKind {
    pub const ALL: [EventKind; 16] = [
        EventKind::PromptDetected, EventKind::CommandFinished, EventKind::DirectoryChanged, EventKind::HostChanged,
        EventKind::UserChanged, EventKind::Idle, EventKind::ActivityAfterIdle, EventKind::SessionEnded,
        EventKind::BellReceived, EventKind::LongRunningCommand, EventKind::CustomEscapeSequence, EventKind::NotificationPosted,
        EventKind::ProgressBarChanged, EventKind::JobStarted, EventKind::JobEnded, EventKind::VariableChanged,
    ];

    pub fn match_type(self) -> i64 {
        100 + Self::ALL.iter().position(|k| *k == self).unwrap_or(0) as i64
    }

    pub fn from_match_type(n: i64) -> Option<EventKind> {
        usize::try_from(n - 100).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    pub fn name(self) -> &'static str {
        match self {
            EventKind::PromptDetected => "Prompt Detected",
            EventKind::CommandFinished => "Command Finished",
            EventKind::DirectoryChanged => "Directory Changed",
            EventKind::HostChanged => "Host Changed",
            EventKind::UserChanged => "User Changed",
            EventKind::Idle => "Idle (Silence)",
            EventKind::ActivityAfterIdle => "Activity After Idle",
            EventKind::SessionEnded => "Session Ended",
            EventKind::BellReceived => "Bell Received",
            EventKind::LongRunningCommand => "Long-Running Command",
            EventKind::CustomEscapeSequence => "Custom Escape Sequence",
            EventKind::NotificationPosted => "Notification Posted",
            EventKind::ProgressBarChanged => "Progress Bar Changed",
            EventKind::JobStarted => "Job Started",
            EventKind::JobEnded => "Job Ended",
            EventKind::VariableChanged => "Variable Changed",
        }
    }

    /// The milestone that brings it, when it is not here yet.
    pub fn later(self) -> Option<u8> {
        matches!(self, EventKind::JobStarted | EventKind::JobEnded).then_some(6)
    }
}

/// Command Finished's exit code filter: `*` or empty any, `0`, `!0`, or a number; anything
/// else matches.
pub fn exit_code_matches(filter: Option<&str>, code: i32) -> bool {
    match filter.map(str::trim) {
        None | Some("*") | Some("") => true,
        Some("!0") => code != 0,
        Some(f) => f.parse::<i32>().map_or(true, |n| n == code),
    }
}

/// A filter pattern: no pattern matches; a bad pattern matches as a substring (or exactly).
pub fn regex_param_matches(pattern: Option<&str>, value: &str, exact_fallback: bool) -> bool {
    let Some(p) = pattern.filter(|p| !p.is_empty()) else { return true };
    match regex::Regex::new(p) {
        Ok(re) => re.is_match(value),
        Err(_) if exact_fallback => value == p,
        Err(_) => value.contains(p),
    }
}

/// What an event is about, for its filters.
#[derive(Clone, Debug, Default)]
pub struct EventFacts {
    pub exit_code: Option<i32>,
    /// The folder, host, user, command, message, or variable value.
    pub value: String,
    /// The variable's name or the sequence id.
    pub name: String,
    pub appeared: bool,
}

fn param<'a>(params: Option<&'a toml::Table>, key: &str) -> Option<&'a str> {
    params?.get(key)?.as_str()
}

/// A number parameter (`timeout`, `threshold`).
pub fn param_secs(params: Option<&toml::Table>, key: &str, default: f64) -> f64 {
    params.and_then(|p| p.get(key)).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(default)
}

fn user_name(n: &str) -> String {
    format!("user.{}", n.strip_prefix("user.").unwrap_or(n))
}

/// Whether a trigger's filters pass for an event.
pub fn passes(kind: EventKind, params: Option<&toml::Table>, f: &EventFacts) -> bool {
    match kind {
        EventKind::CommandFinished => exit_code_matches(param(params, "exitCodeFilter"), f.exit_code.unwrap_or(0)),
        EventKind::DirectoryChanged => regex_param_matches(param(params, "directoryRegex"), &f.value, false),
        EventKind::HostChanged => regex_param_matches(param(params, "hostRegex"), &f.value, false),
        EventKind::UserChanged => regex_param_matches(param(params, "userRegex"), &f.value, false),
        EventKind::LongRunningCommand => regex_param_matches(param(params, "commandRegex"), &f.value, false),
        EventKind::CustomEscapeSequence => regex_param_matches(param(params, "sequenceId"), &f.name, true),
        EventKind::NotificationPosted => regex_param_matches(param(params, "messageRegex"), &f.value, false),
        EventKind::ProgressBarChanged => match param(params, "progressBarFilter") {
            Some("appeared") => f.appeared,
            Some("disappeared") => !f.appeared,
            _ => true,
        },
        EventKind::VariableChanged => {
            param(params, "variableName").is_none_or(|n| n.is_empty() || user_name(n) == user_name(&f.name))
                && regex_param_matches(param(params, "variableValueRegex"), &f.value, false)
        }
        _ => true,
    }
}

/// Timer state an event trigger keeps (idle spells, the command it fired for).
#[derive(Clone, Debug, Default)]
pub struct EventState {
    /// The quiet spell for Idle and Activity After Idle has begun.
    pub quiet: bool,
    /// Long-Running Command fired for this command's mark.
    pub long_fired: Option<u32>,
}

impl Screen {
    /// Runs the triggers for an event. `captures` are `\0`, `\1`, and on.
    pub(crate) fn fire_event(&mut self, kind: EventKind, facts: &EventFacts, captures: &[String]) {
        let wanted: Vec<usize> = self
            .triggers
            .list
            .iter()
            .enumerate()
            .filter(|(_, c)| c.trigger.match_type == kind.match_type() && c.trigger.enabled && !c.off_here && kind.later().is_none())
            .filter(|(_, c)| passes(kind, c.trigger.extra.get("eventParams").and_then(|v| v.as_table()), facts))
            .map(|(i, _)| i)
            .collect();
        for i in wanted {
            if self.fire_trigger_with(i, captures) {
                break;
            }
        }
    }

    /// Output arrived: Activity After Idle fires for triggers whose quiet spell had begun,
    /// and every quiet spell ends.
    pub(crate) fn events_on_output(&mut self) {
        self.last_output = Some(Instant::now());
        let back: Vec<usize> = self
            .triggers
            .list
            .iter()
            .enumerate()
            .filter(|(_, c)| c.events.quiet && c.trigger.match_type == EventKind::ActivityAfterIdle.match_type() && c.trigger.enabled && !c.off_here)
            .map(|(i, _)| i)
            .collect();
        for c in self.triggers.list.iter_mut() {
            c.events.quiet = false;
        }
        for i in back {
            self.fire_trigger_with(i, &[]);
        }
    }

    /// Idle (Silence) after each trigger's timeout without output, and Long-Running Command.
    pub fn check_event_timers(&mut self, now: Instant) {
        let idle = [EventKind::Idle.match_type(), EventKind::ActivityAfterIdle.match_type()];
        let running = self.marks.last_command.and_then(|id| self.marks.records.get(&id).filter(|m| m.end.is_none()).map(|m| (id, m.command.clone(), m.start)));
        for i in 0..self.triggers.list.len() {
            let c = &self.triggers.list[i];
            if !c.trigger.enabled || c.off_here {
                continue;
            }
            let params = c.trigger.extra.get("eventParams").and_then(|v| v.as_table()).cloned();
            let t = c.trigger.match_type;
            if idle.contains(&t) && !c.events.quiet {
                let timeout = param_secs(params.as_ref(), "timeout", 30.0);
                if self.last_output.is_some_and(|l| now.saturating_duration_since(l) >= Duration::from_secs_f64(timeout)) {
                    self.triggers.list[i].events.quiet = true;
                    if t == EventKind::Idle.match_type() {
                        self.fire_trigger_with(i, &[format!("{}", timeout as i64)]);
                    }
                }
            }
            if t == EventKind::LongRunningCommand.match_type() {
                let Some((id, command, Some(start))) = running.clone() else { continue };
                let threshold = param_secs(params.as_ref(), "threshold", 60.0);
                let elapsed = SystemTime::now().duration_since(start).unwrap_or_default().as_secs_f64();
                let facts = EventFacts { value: command.clone(), ..Default::default() };
                if elapsed >= threshold && self.triggers.list[i].events.long_fired != Some(id) && passes(EventKind::LongRunningCommand, params.as_ref(), &facts) {
                    self.triggers.list[i].events.long_fired = Some(id);
                    self.fire_trigger_with(i, &[command, format!("{}", elapsed as i64)]);
                }
            }
        }
    }

    /// Whether any trigger waits on time (the window's timer checks back).
    pub fn has_timed_events(&self) -> bool {
        let timed = [EventKind::Idle.match_type(), EventKind::ActivityAfterIdle.match_type(), EventKind::LongRunningCommand.match_type()];
        self.triggers.list.iter().any(|c| c.trigger.enabled && timed.contains(&c.trigger.match_type))
    }

    /// A user variable set (SetUserVar, a trigger): Variable Changed when its value changed.
    pub(crate) fn set_user_var(&mut self, name: &str, value: String) {
        let changed = self.shell.vars.get(name) != Some(&value);
        self.shell.vars.insert(name.to_string(), value.clone());
        if changed {
            let full = format!("user.{name}");
            let facts = EventFacts { value: value.clone(), name: full.clone(), ..Default::default() };
            self.fire_event(EventKind::VariableChanged, &facts, &[value, full]);
        }
    }
}

impl Screen {
    /// Runs one trigger's action with captured strings (event triggers). True to stop the rest.
    pub(crate) fn fire_trigger_with(&mut self, i: usize, captures: &[String]) -> bool {
        let Some(c) = self.triggers.list.get(i) else { return false };
        let param = c.trigger.parameter.clone();
        let groups: Vec<Option<&str>> = captures.iter().map(|s| Some(s.as_str())).collect();
        let value = if self.triggers.prefs.interpolated && param.contains("\\(") {
            match crate::interp::evaluate(&param, &|n| self.trigger_var(n, 0), &groups) {
                Ok(v) => v,
                Err(e) => {
                    if self.triggers.list[i].error.as_ref() != Some(&e) {
                        self.requests.push(crate::screen::Request::Trigger(crate::trigger_eval::TriggerRequest::ParameterError { trigger: i, text: e.clone() }));
                        self.triggers.list[i].error = Some(e);
                    }
                    return false;
                }
            }
        } else {
            triggers::substitute(&param, &groups)
        };
        self.run_event_action(i, &value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Request, Screen};
    use crate::trigger_eval::{TriggerPrefs, TriggerRequest};
    use crate::triggers::Trigger;

    fn event(kind: EventKind, param: &str, params: &[(&str, toml::Value)]) -> Trigger {
        let mut extra = toml::Table::new();
        if !params.is_empty() {
            extra.insert("eventParams".into(), toml::Value::Table(params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()));
        }
        Trigger { regex: String::new(), action: "AlertTrigger".into(), parameter: param.into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: kind.match_type(), extra }
    }

    fn alerts(s: &mut Screen) -> Vec<String> {
        s.take_requests().into_iter().filter_map(|r| match r {
            Request::Trigger(TriggerRequest::Alert { text, .. }) => Some(text),
            _ => None,
        }).collect()
    }

    #[test]
    fn events_match_their_triggers() {
        let mut s = Screen::new(40, 6);
        s.set_triggers(vec![
            event(EventKind::CommandFinished, "code \\0", &[]),
            event(EventKind::DirectoryChanged, "dir \\0", &[]),
            event(EventKind::BellReceived, "bell", &[]),
            event(EventKind::NotificationPosted, "note \\0", &[]),
            event(EventKind::PromptDetected, "prompt", &[]),
        ], TriggerPrefs::default());
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;1\x07");
        assert_eq!(alerts(&mut s), vec!["prompt", "code 1"]);
        s.feed(b"\x1b]1337;CurrentDir=/tmp\x07\x07\x1b]9;hi\x07");
        assert_eq!(alerts(&mut s), vec!["dir /tmp", "bell", "note hi"]);
        s.feed(b"\x1b]1337;CurrentDir=/tmp\x07");
        assert!(alerts(&mut s).is_empty(), "the same folder is not a change");
        assert_eq!(EventKind::Idle.name(), "Idle (Silence)");
        assert_eq!(EventKind::from_match_type(113), Some(EventKind::JobStarted));
        assert_eq!(EventKind::JobStarted.later(), Some(6));
    }

    #[test]
    fn exit_code_filters() {
        assert!(exit_code_matches(None, 3));
        assert!(exit_code_matches(Some("*"), 3) && exit_code_matches(Some(""), 3));
        assert!(exit_code_matches(Some("0"), 0) && !exit_code_matches(Some("0"), 1));
        assert!(exit_code_matches(Some("!0"), 2) && !exit_code_matches(Some("!0"), 0));
        assert!(exit_code_matches(Some("3"), 3) && !exit_code_matches(Some("3"), 4));
        assert!(exit_code_matches(Some("abc"), 9), "not a number: matches");
    }

    #[test]
    fn regex_filters_fall_back_to_substring() {
        assert!(regex_param_matches(None, "anything", false));
        assert!(regex_param_matches(Some(""), "anything", false));
        assert!(regex_param_matches(Some("^/home/"), "/home/me", false));
        assert!(!regex_param_matches(Some("^/home/"), "/tmp", false));
        assert!(regex_param_matches(Some("a(b"), "xa(by", false), "a bad pattern is a substring");
        assert!(!regex_param_matches(Some("a(b"), "xa(by", true), "or exact, for sequence ids");
        assert!(regex_param_matches(Some("a(b"), "a(b", true));
    }

    #[test]
    fn idle_event_fires_once_per_quiet_spell() {
        let mut s = Screen::new(40, 4);
        s.set_triggers(vec![event(EventKind::Idle, "idle \\0", &[("timeout", toml::Value::Integer(1))]), event(EventKind::ActivityAfterIdle, "back", &[("timeout", toml::Value::Integer(1))])], TriggerPrefs::default());
        let t0 = std::time::Instant::now();
        s.feed(b"x");
        s.last_output = Some(t0);
        s.check_event_timers(t0 + std::time::Duration::from_millis(500));
        assert!(alerts(&mut s).is_empty());
        s.check_event_timers(t0 + std::time::Duration::from_millis(1500));
        assert_eq!(alerts(&mut s), vec!["idle 1"]);
        s.check_event_timers(t0 + std::time::Duration::from_secs(3));
        assert!(alerts(&mut s).is_empty(), "once per quiet spell");
        s.feed(b"y");
        assert_eq!(alerts(&mut s), vec!["back"], "output after the quiet spell");
        s.last_output = Some(t0 + std::time::Duration::from_secs(4));
        s.check_event_timers(t0 + std::time::Duration::from_secs(6));
        assert_eq!(alerts(&mut s), vec!["idle 1"], "a new quiet spell");
    }

    #[test]
    fn custom_sequence_needs_trust() {
        let mut s = Screen::new(40, 4);
        s.set_triggers(vec![event(EventKind::CustomEscapeSequence, "\\0 says \\1", &[("sequenceId", toml::Value::String("me".into()))])], TriggerPrefs::default());
        s.feed(b"\x1b]1337;Custom=id=me:hello\x07\x1b]1337;Custom=id=other:x\x07");
        assert_eq!(alerts(&mut s), vec!["me says hello"]);
        s.trusted = false;
        s.feed(b"\x1b]1337;Custom=id=me:hello\x07");
        assert!(alerts(&mut s).is_empty());
    }

    #[test]
    fn variable_and_progress_events() {
        let mut s = Screen::new(40, 4);
        s.set_triggers(vec![
            event(EventKind::VariableChanged, "\\1=\\0", &[("variableName", toml::Value::String("user.build".into()))]),
            event(EventKind::ProgressBarChanged, "bar \\0", &[("progressBarFilter", toml::Value::String("appeared".into()))]),
        ], TriggerPrefs::default());
        use base64::Engine;
        let v = base64::engine::general_purpose::STANDARD.encode("ok");
        s.feed(format!("\x1b]1337;SetUserVar=build={v}\x07\x1b]1337;SetUserVar=other={v}\x07\x1b]9;4;1;50\x07\x1b]9;4;0\x07").as_bytes());
        assert_eq!(alerts(&mut s), vec!["user.build=ok", "bar appeared"]);
    }
}
