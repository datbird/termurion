//! iTerm2's notifications: whether an alert posts (the profile's switches and filters, and
//! whether the user is looking), and its title and body.

use std::time::{Duration, Instant};

/// What an alert is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Bell,
    Idle,
    NewOutput,
    SessionEnded,
    /// OSC 9 or OSC 1337 Notification (its own title, when it gave one).
    Terminal { title: Option<String>, message: String },
    /// A Post Notification trigger's message.
    Trigger(String),
}

/// The profile's notification switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertPrefs {
    /// "Notification Center alerts".
    pub enabled: bool,
    pub bell: bool,
    pub idle: bool,
    pub new_output: bool,
    pub session_ended: bool,
    /// "Send escape sequence-generated alerts".
    pub terminal: bool,
    /// "Suppress alerts from active session".
    pub suppress_active: bool,
}

/// Whether an alert posts: notifications on, the filter on, not suppressed in the active
/// session, and (except for escape-sequence alerts) the user not looking at the session.
pub fn should_post(kind: &AlertKind, prefs: AlertPrefs, looking: bool, active: bool) -> bool {
    if !prefs.enabled || (prefs.suppress_active && active) {
        return false;
    }
    let filter = match kind {
        AlertKind::Bell => prefs.bell,
        AlertKind::Idle => prefs.idle,
        AlertKind::NewOutput => prefs.new_output,
        AlertKind::SessionEnded => prefs.session_ended,
        AlertKind::Terminal { .. } => return prefs.terminal,
        AlertKind::Trigger(_) => true,
    };
    filter && !looking
}

/// iTerm2's title and body. `simple` is simpleNotifications; `end_text` is
/// sessionEndMessageText.
pub fn text(kind: &AlertKind, session: &str, tab: usize, simple: bool, end_text: &str) -> (String, String) {
    match kind {
        AlertKind::Bell => ("Bell".into(), format!("Session {session} #{tab} just rang a bell!")),
        AlertKind::Idle => ("Idle".into(), format!("Session {session} in tab #{tab} became idle.")),
        AlertKind::NewOutput => ("New Output".into(), format!("New output was received in {session}, tab #{tab}.")),
        AlertKind::SessionEnded => {
            let title = if end_text.is_empty() { "Session Ended" } else { end_text };
            (title.into(), format!("Session \"{session}\" in tab #{tab} just terminated."))
        }
        AlertKind::Terminal { title: Some(t), message } => (t.clone(), message.clone()),
        AlertKind::Terminal { title: None, message } => ("Alert".into(), if simple { message.clone() } else { format!("Session {session} #{tab}: {message}") }),
        AlertKind::Trigger(m) => (m.clone(), format!("A trigger fired in session \u{201c}{session}\u{201d} in tab #{tab}.")),
    }
}

/// "Periodically send an ASCII code when idle": due once `period` has passed since the
/// last output and since the last code sent.
pub fn idle_code_due(last_output: Instant, last_sent: Option<Instant>, now: Instant, period: Duration) -> bool {
    let from = last_sent.map_or(last_output, |s| s.max(last_output));
    now.saturating_duration_since(from) >= period
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: AlertPrefs = AlertPrefs { enabled: true, bell: true, idle: true, new_output: true, session_ended: true, terminal: true, suppress_active: false };

    #[test]
    fn alert_texts_follow_iterm2() {
        let t = |k: &AlertKind| text(k, "bash", 2, false, "");
        assert_eq!(t(&AlertKind::Bell), ("Bell".into(), "Session bash #2 just rang a bell!".into()));
        assert_eq!(t(&AlertKind::Idle), ("Idle".into(), "Session bash in tab #2 became idle.".into()));
        assert_eq!(t(&AlertKind::NewOutput), ("New Output".into(), "New output was received in bash, tab #2.".into()));
        assert_eq!(t(&AlertKind::SessionEnded), ("Session Ended".into(), "Session \"bash\" in tab #2 just terminated.".into()));
        assert_eq!(text(&AlertKind::SessionEnded, "bash", 2, false, "Bye").0, "Bye", "sessionEndMessageText");
        assert_eq!(t(&AlertKind::Terminal { title: None, message: "hi".into() }), ("Alert".into(), "Session bash #2: hi".into()));
        assert_eq!(text(&AlertKind::Terminal { title: None, message: "hi".into() }, "bash", 2, true, "").1, "hi", "simpleNotifications");
        assert_eq!(t(&AlertKind::Terminal { title: Some("Build".into()), message: "done".into() }), ("Build".into(), "done".into()), "OSC 1337 Notification's own title");
        assert_eq!(t(&AlertKind::Trigger("done".into())), ("done".into(), "A trigger fired in session \u{201c}bash\u{201d} in tab #2.".into()));
    }

    #[test]
    fn terminal_alerts_follow_the_filters() {
        let k = AlertKind::Terminal { title: None, message: "x".into() };
        assert!(should_post(&k, ALL, true, true), "escape-sequence alerts post while looking");
        assert!(!should_post(&k, AlertPrefs { enabled: false, ..ALL }, false, false), "Notification Center alerts off");
        assert!(!should_post(&k, AlertPrefs { terminal: false, ..ALL }, false, false));
        assert!(!should_post(&k, AlertPrefs { suppress_active: true, ..ALL }, true, true), "suppressed in the active session");
        assert!(should_post(&k, AlertPrefs { suppress_active: true, ..ALL }, false, false));
    }

    #[test]
    fn looking_blocks_posting() {
        for k in [AlertKind::Bell, AlertKind::Idle, AlertKind::NewOutput, AlertKind::SessionEnded, AlertKind::Trigger("t".into())] {
            assert!(!should_post(&k, ALL, true, true), "{k:?} while looking");
            assert!(should_post(&k, ALL, false, false), "{k:?}");
        }
        assert!(!should_post(&AlertKind::Idle, AlertPrefs { idle: false, ..ALL }, false, false));
        assert!(!should_post(&AlertKind::Bell, AlertPrefs { bell: false, ..ALL }, false, false));
    }

    #[test]
    fn osc9_and_notification_parse() {
        use base64::Engine;
        let b = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let mut s = crate::screen::Screen::new(20, 4);
        s.feed(b"\x1b]9;hello there\x07\x1b]9;4;1;50\x07");
        s.feed(format!("\x1b]1337;Notification=title={};message={};subtitle={}\x07", b("Build"), b("done"), b("ok")).as_bytes());
        let got: Vec<crate::screen::Request> = s.take_requests().into_iter().filter(|r| matches!(r, crate::screen::Request::Notify { .. })).collect();
        assert_eq!(got, vec![
            crate::screen::Request::Notify { title: None, message: "hello there".into() },
            crate::screen::Request::Notify { title: Some("Build".into()), message: "ok\ndone".into() },
        ]);
    }

    #[test]
    fn idle_code_every_period() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let p = Duration::from_secs(60);
        assert!(!idle_code_due(t0, None, t0 + Duration::from_secs(59), p));
        assert!(idle_code_due(t0, None, t0 + Duration::from_secs(60), p));
        assert!(!idle_code_due(t0, Some(t0 + Duration::from_secs(60)), t0 + Duration::from_secs(100), p), "a minute after the last one sent");
        assert!(idle_code_due(t0, Some(t0 + Duration::from_secs(60)), t0 + Duration::from_secs(120), p));
    }
}
