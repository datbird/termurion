//! A watched tmux format (iTerm2's iTermTmuxOptionMonitor): a subscription on tmux 3.2 and
//! later (`refresh-client -B`), and `display-message` polls, on events or a timer, before
//! that. The first value always comes from one poll.

use std::sync::atomic::{AtomicU64, Ordering};

/// Subscription ids count up across every tmux connection (iTerm2's static counter).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_subscription_id() -> String {
    format!("it2_{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// A format inside single quotes: `\` doubles, then `'` gets a backslash.
pub fn escape_format(format: &str) -> String {
    format.replace('\\', "\\\\").replace('\'', "\\'")
}

pub fn subscribe_command(id: &str, target: &str, format: &str) -> String {
    format!("refresh-client -B '{id}:{target}:{}'", escape_format(format))
}

pub fn unsubscribe_command(id: &str) -> String {
    format!("refresh-client -B '{id}'")
}

pub fn poll_command(target: &str, format: &str) -> String {
    format!("display-message -t '{target}' -p '{}'", escape_format(format))
}

/// A command a new monitor sends.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Subscribe(String),
    Poll(String),
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub target: String,
    pub format: String,
    /// The subscription's id (3.2 and later).
    pub id: Option<String>,
    /// tmux answered the subscription; `valid`: it took it.
    initialized: bool,
    valid: bool,
    /// A poll waits for its reply. A poll that failed leaves it set (iTerm2's didFetch).
    outstanding: bool,
    pub value: Option<String>,
    /// Polls on a timer (before 3.2 only): the interval and the next time.
    timer: Option<(f64, f64)>,
}

impl Monitor {
    /// A monitor of `format` on `target` (`%P`, `@W`, or empty for the session).
    pub fn new(target: &str, format: &str, subscribe: bool, timer: Option<f64>) -> (Monitor, Vec<Cmd>) {
        let id = subscribe.then(next_subscription_id);
        let mut m = Monitor { target: target.to_string(), format: format.to_string(), id, initialized: false, valid: false, outstanding: false, value: None, timer: timer.map(|i| (i, 0.0)) };
        let mut cmds = Vec::new();
        if let Some(id) = &m.id {
            cmds.push(Cmd::Subscribe(subscribe_command(id, target, format)));
        }
        cmds.extend(m.update_once().map(Cmd::Poll));
        (m, cmds)
    }

    /// iTerm2's updateOnce: the poll to send, if one may go.
    pub fn update_once(&mut self) -> Option<String> {
        if self.outstanding || self.valid {
            return None;
        }
        self.outstanding = true;
        Some(poll_command(&self.target, &self.format))
    }

    /// A poll's reply (None: an error). A new value comes back.
    pub fn fetched(&mut self, reply: Option<&str>) -> Option<String> {
        let value = reply?;
        self.outstanding = false;
        self.set(value)
    }

    /// tmux answered the subscription.
    pub fn subscribed(&mut self, ok: bool) {
        self.initialized = true;
        self.valid = ok;
    }

    /// `%subscription-changed`: a new value comes back.
    pub fn changed(&mut self, value: &str) -> Option<String> {
        self.set(value)
    }

    fn set(&mut self, value: &str) -> Option<String> {
        if self.value.as_deref() == Some(value) {
            return None;
        }
        self.value = Some(value.to_string());
        self.value.clone()
    }

    /// The command that ends the subscription, unless tmux refused it or there is none.
    pub fn unsubscribe(&self) -> Option<String> {
        let id = self.id.as_ref()?;
        if self.initialized && !self.valid {
            return None;
        }
        Some(unsubscribe_command(id))
    }

    /// When the timer polls next (None: no timer, or the subscription carries it).
    pub fn next_tick(&self) -> Option<f64> {
        let (_, next) = self.timer?;
        (!self.valid && !self.outstanding).then_some(next)
    }

    /// The timer's poll, when it is due.
    pub fn tick(&mut self, now: f64) -> Option<String> {
        let (interval, next) = self.timer?;
        if self.valid || now < next {
            return None;
        }
        self.timer = Some((interval, now + interval));
        self.update_once()
    }
}

/// When a tmux pane's working directory is asked for again (iTerm2's
/// iTermWorkingDirectoryPoller): line feeds at most once a second, and once a directory is
/// known only after a key press.
#[derive(Clone, Debug, Default)]
pub struct CwdGate {
    last: Option<f64>,
    known: bool,
    key: bool,
}

impl CwdGate {
    /// A line feed: true when the directory is asked for now.
    pub fn line_feed(&mut self, now: f64) -> bool {
        if self.last.is_some_and(|t| now - t < 1.0) || (self.known && !self.key) {
            return false;
        }
        self.last = Some(now);
        self.key = false;
        true
    }

    /// A key press: true when the directory is asked for now.
    pub fn key_pressed(&mut self, now: f64) -> bool {
        self.key = true;
        self.line_feed(now)
    }

    /// A directory came back.
    pub fn known(&mut self) {
        self.known = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_follow_iterm2() {
        assert_eq!(escape_format("a'b\\c"), "a\\'b\\\\c");
        assert_eq!(subscribe_command("it2_3", "%1", "#{pane_title}"), "refresh-client -B 'it2_3:%1:#{pane_title}'");
        assert_eq!(unsubscribe_command("it2_3"), "refresh-client -B 'it2_3'");
        assert_eq!(poll_command("%1", "#{pane_title}"), "display-message -t '%1' -p '#{pane_title}'");
        let (a, b) = (next_subscription_id(), next_subscription_id());
        assert!(a.starts_with("it2_") && b.starts_with("it2_") && a != b);
    }

    #[test]
    fn subscriptions_replace_polls() {
        let (mut m, cmds) = Monitor::new("%1", "#{pane_title}", true, None);
        let id = m.id.clone().unwrap();
        assert_eq!(cmds, vec![Cmd::Subscribe(format!("refresh-client -B '{id}:%1:#{{pane_title}}'")), Cmd::Poll("display-message -t '%1' -p '#{pane_title}'".into())]);
        assert_eq!(m.update_once(), None, "one poll at a time");
        assert_eq!(m.fetched(Some("a")), Some("a".into()));
        m.subscribed(true);
        assert_eq!(m.update_once(), None, "the subscription carries it");
        assert_eq!(m.changed("x"), Some("x".into()));
        assert_eq!(m.changed("x"), None, "unchanged");
        assert_eq!(m.unsubscribe(), Some(format!("refresh-client -B '{id}'")));
        let (mut r, _) = Monitor::new("%1", "#{pane_title}", true, None);
        r.subscribed(false);
        assert_eq!(r.fetched(Some("a")), Some("a".into()));
        assert!(r.update_once().is_some(), "a refused subscription keeps polls");
        assert_eq!(r.unsubscribe(), None);
        let (old, cmds) = Monitor::new("%1", "#{pane_title}", false, None);
        assert_eq!((old.id.clone(), cmds.len(), old.unsubscribe()), (None, 1, None));
    }

    #[test]
    fn a_failed_poll_stops_the_monitor() {
        let (mut m, _) = Monitor::new("%1", "#{pane_current_command}", false, Some(1.0));
        assert_eq!(m.fetched(None), None);
        assert_eq!(m.update_once(), None);
        assert_eq!(m.tick(100.0), None, "never again");
        let (mut t, _) = Monitor::new("%1", "#{pane_current_command}", false, Some(1.0));
        t.fetched(Some("bash"));
        assert!(t.tick(0.5).is_some(), "the first tick polls");
        t.fetched(Some("bash"));
        assert_eq!(t.tick(1.0), None, "a second between");
        assert!(t.tick(1.6).is_some());
    }

    #[test]
    fn working_directory_polls_are_rate_limited() {
        let mut g = CwdGate::default();
        assert!(g.line_feed(0.0));
        assert!(!g.line_feed(0.5), "once a second");
        assert!(g.line_feed(1.2), "no directory yet: line feeds poll");
        g.known();
        assert!(!g.line_feed(3.0), "known: only after a key press");
        assert!(g.key_pressed(3.5));
        assert!(!g.line_feed(3.6));
        assert!(!g.key_pressed(3.8), "still once a second");
        assert!(g.line_feed(4.6), "the key press still counts");
        assert!(!g.line_feed(6.0));
    }
}
