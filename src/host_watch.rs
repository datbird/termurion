//! What the session host does when its Termurion goes away. A clean quit or detach says
//! goodbye first and the host just waits for the next launch. A client that vanishes (the pipe
//! closes with no goodbye: a crash, a kill) leaves live sessions with no app, so the host
//! reopens Termurion after a short wait, at most once per 5 minutes, and ends the sessions if
//! nobody connects within an hour. This is the pure decision logic; `host.rs` feeds it events
//! and carries out the actions.

use std::time::{Duration, Instant};

/// Wait this long after a vanish for a client to come back by itself before reopening.
pub const REOPEN_AFTER: Duration = Duration::from_secs(3);
/// At most one automatic restart in this time.
pub const RESTART_EVERY: Duration = Duration::from_secs(5 * 60);
/// With no client for this long after a vanish, the sessions end.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The attached client is gone. `clean`: it said goodbye (or ended every session) first.
    /// `live` is the host's count of running sessions.
    ClientGone { clean: bool, live: usize },
    /// A client completed the handshake.
    Connected,
    /// Time passed; `live` is the host's count of running sessions.
    Tick { live: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A line for host.log.
    Log(String),
    /// Start termurion.exe.
    Reopen,
    /// End every session, as a quit that kills jobs does; the host then exits when idle.
    EndSessions,
}

#[derive(Default)]
pub struct Watch {
    /// When the current vanish happened, while no client has come back.
    vanished: Option<Instant>,
    /// The reopen decision for this vanish is made (restart or skipped).
    decided: bool,
    last_restart: Option<Instant>,
}

impl Watch {
    pub fn on(&mut self, now: Instant, ev: Event) -> Vec<Action> {
        let mut out = Vec::new();
        match ev {
            Event::Connected => *self = Watch { last_restart: self.last_restart, ..Watch::default() },
            Event::ClientGone { clean, live } => {
                if clean || live == 0 {
                    return out;
                }
                self.vanished = Some(now);
                self.decided = false;
                out.push(Action::Log(format!("host: client vanished, {live} sessions")));
            }
            Event::Tick { live } => {
                let Some(at) = self.vanished else { return out };
                if live == 0 {
                    // Nothing left to keep: the host's idle exit takes over.
                    self.vanished = None;
                    return out;
                }
                let waited = now.saturating_duration_since(at);
                if !self.decided && waited >= REOPEN_AFTER {
                    self.decided = true;
                    match self.last_restart.map(|r| at.saturating_duration_since(r)).filter(|ago| *ago < RESTART_EVERY) {
                        Some(ago) => out.push(Action::Log(format!("host: restart skipped, last was {}s ago", ago.as_secs()))),
                        None => {
                            self.last_restart = Some(now);
                            out.push(Action::Log("host: reopening termurion.exe".into()));
                            out.push(Action::Reopen);
                        }
                    }
                }
                if waited >= GIVE_UP_AFTER {
                    self.vanished = None;
                    out.push(Action::Log(format!("host: no client for 1 h, ending {live} sessions")));
                    out.push(Action::EndSessions);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn has(a: &[Action], x: &Action) -> bool {
        a.contains(x)
    }

    #[test]
    fn a_clean_quit_never_restarts() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        assert!(w.on(t0, Event::ClientGone { clean: true, live: 3 }).is_empty());
        for n in [1, 3, 10, 4000] {
            assert!(w.on(t0 + s(n), Event::Tick { live: 3 }).is_empty());
        }
    }

    #[test]
    fn a_vanish_restarts_after_three_seconds() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        let a = w.on(t0, Event::ClientGone { clean: false, live: 2 });
        assert_eq!(a, vec![Action::Log("host: client vanished, 2 sessions".into())]);
        assert!(w.on(t0 + s(2), Event::Tick { live: 2 }).is_empty());
        let a = w.on(t0 + s(3), Event::Tick { live: 2 });
        assert_eq!(a, vec![Action::Log("host: reopening termurion.exe".into()), Action::Reopen]);
        assert!(w.on(t0 + s(4), Event::Tick { live: 2 }).is_empty(), "one restart per vanish");
    }

    #[test]
    fn a_vanish_with_no_live_sessions_does_nothing() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        assert!(w.on(t0, Event::ClientGone { clean: false, live: 0 }).is_empty());
        assert!(w.on(t0 + s(10), Event::Tick { live: 0 }).is_empty());
    }

    #[test]
    fn a_reconnect_inside_three_seconds_cancels() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        w.on(t0, Event::ClientGone { clean: false, live: 1 });
        assert!(w.on(t0 + s(2), Event::Connected).is_empty());
        assert!(w.on(t0 + s(5), Event::Tick { live: 1 }).is_empty());
    }

    #[test]
    fn a_second_vanish_within_five_minutes_is_skipped() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        w.on(t0, Event::ClientGone { clean: false, live: 1 });
        assert!(has(&w.on(t0 + s(3), Event::Tick { live: 1 }), &Action::Reopen));
        w.on(t0 + s(10), Event::Connected);
        let t1 = t0 + s(100);
        w.on(t1, Event::ClientGone { clean: false, live: 1 });
        let a = w.on(t1 + s(3), Event::Tick { live: 1 });
        assert!(!has(&a, &Action::Reopen));
        assert_eq!(a, vec![Action::Log("host: restart skipped, last was 97s ago".into())]);
        // Once five minutes passed since the restart, a vanish restarts again.
        w.on(t1 + s(20), Event::Connected);
        let t2 = t0 + s(3 + 300);
        w.on(t2, Event::ClientGone { clean: false, live: 1 });
        assert!(has(&w.on(t2 + s(3), Event::Tick { live: 1 }), &Action::Reopen));
    }

    #[test]
    fn an_hour_without_a_client_ends_the_sessions() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        w.on(t0, Event::ClientGone { clean: false, live: 4 });
        w.on(t0 + s(3), Event::Tick { live: 4 });
        assert!(w.on(t0 + s(3599), Event::Tick { live: 4 }).is_empty());
        let a = w.on(t0 + s(3600), Event::Tick { live: 4 });
        assert_eq!(a, vec![Action::Log("host: no client for 1 h, ending 4 sessions".into()), Action::EndSessions]);
        assert!(w.on(t0 + s(3700), Event::Tick { live: 4 }).is_empty(), "ends once");
    }

    #[test]
    fn a_reconnect_cancels_the_hour() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        w.on(t0, Event::ClientGone { clean: false, live: 4 });
        w.on(t0 + s(3), Event::Tick { live: 4 });
        w.on(t0 + s(1800), Event::Connected);
        assert!(w.on(t0 + s(3600), Event::Tick { live: 4 }).is_empty());
        assert!(w.on(t0 + s(9000), Event::Tick { live: 4 }).is_empty());
    }

    #[test]
    fn the_hour_ends_even_when_the_restart_was_skipped() {
        let (mut w, t0) = (Watch::default(), Instant::now());
        w.on(t0, Event::ClientGone { clean: false, live: 1 });
        w.on(t0 + s(3), Event::Tick { live: 1 });
        w.on(t0 + s(4), Event::Connected);
        let t1 = t0 + s(60);
        w.on(t1, Event::ClientGone { clean: false, live: 1 });
        w.on(t1 + s(3), Event::Tick { live: 1 });
        assert!(has(&w.on(t1 + s(3600), Event::Tick { live: 1 }), &Action::EndSessions));
    }
}
