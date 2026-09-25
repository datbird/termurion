//! tmux control mode's first exchange (iTerm2's kickOffTmuxForRestoration and the
//! TmuxController replies it starts): the phony command, the option and UTF-8 checks, the
//! default terminal, the key list, and the version, read from `#{version}` or bracketed by
//! commands that fail in older versions.

use std::time::{Duration, Instant};

use crate::tmux_gateway::{Flags, Gateway, Tag};

/// Each handshake question; its reply comes back under `tag(ask)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Phony,
    Ping,
    AggressiveResize,
    Status,
    Utf8,
    DefaultTerminal,
    ListKeys,
    CopyMode,
    Version,
    Probe23,
    Probe22,
    Probe21,
    Probe18,
    ServerPid,
    SetTitles,
}

const ASKS: [Ask; 15] = [Ask::Phony, Ask::Ping, Ask::AggressiveResize, Ask::Status, Ask::Utf8, Ask::DefaultTerminal, Ask::ListKeys, Ask::CopyMode, Ask::Version, Ask::Probe23, Ask::Probe22, Ask::Probe21, Ask::Probe18, Ask::ServerPid, Ask::SetTitles];

/// Handshake tags sit below this; the rest of the tmux code numbers its own from here.
pub const FIRST_FREE_TAG: Tag = 100;

pub fn tag(ask: Ask) -> Tag {
    ASKS.iter().position(|a| *a == ask).unwrap() as Tag + 1
}

pub fn ask_of(tag: Tag) -> Option<Ask> {
    tag.checked_sub(1).and_then(|i| ASKS.get(i as usize)).copied()
}

/// What `#{version}` says (iTerm2's handleDisplayMessageVersion).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Version {
    /// The version: 2.9a is 2.91, 3.0a is 3.01. `tab_bug`: tmux next-3.4 printed `\t`
    /// in list-windows replies.
    Exact { number: f64, tab_bug: bool },
    /// Empty: the format came in 2.4, so the version is 2.3 or older.
    AtMost(f64),
    /// Not a number, or outside 2.4 to 10: the probes decide.
    Ignored,
}

pub fn parse_version(reply: &str) -> Version {
    match reply {
        "openbsd-7.1" => return parse_version("3.4"),
        "openbsd-6.8" => return parse_version("3.2"),
        "openbsd-6.7" => return parse_version("3.0"),
        "" => return Version::AtMost(2.3),
        _ => {}
    }
    if let Some(n) = reply.strip_prefix("openbsd-") {
        // iTerm2 reports 3.4 and then 3.2 for 7.1 and later; the minimum keeps 3.4.
        let v = if decimal(n).is_some_and(|n| n >= 7.1) { "3.4" } else { "3.2" };
        return parse_version(v);
    }
    let tab_bug = reply == "next-3.4";
    if let Some(rest) = reply.strip_prefix("next-") {
        return match parse_version(rest) {
            Version::Exact { number, .. } => Version::Exact { number, tab_bug },
            v => v,
        };
    }
    let reply = reply.strip_suffix("-rc").unwrap_or(reply);
    match decimal(reply) {
        Some(n) if (2.4..=10.0).contains(&n) => {}
        _ => return Version::Ignored,
    }
    // A letter is a dot release: 2.9a is 2.91 (tmux issue 1712).
    let number = match reply.char_indices().find(|(_, c)| c.is_ascii_lowercase()) {
        None => decimal(reply),
        Some((i, c)) => decimal(&format!("{}{}", &reply[..i], c as u32 - 'a' as u32 + 1)),
    };
    match number {
        Some(number) => Version::Exact { number, tab_bug },
        None => Version::Ignored,
    }
}

/// The leading decimal number of `s`, as NSDecimalNumber reads it (none: NaN).
fn decimal(s: &str) -> Option<f64> {
    let end = s.char_indices().find(|&(i, c)| !(c.is_ascii_digit() || (c == '.' && !s[..i].contains('.')))).map_or(s.len(), |(i, _)| i);
    s[..end].parse().ok()
}

/// Something the handshake needs the session to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// End tmux mode with this alert (title, text).
    Abort(String, String),
    /// The version is known (iTerm2's didGuessVersion).
    VersionKnown,
}

pub const PHONY_WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Handshake {
    pub kicked: bool,
    phony_since: Option<Instant>,
    pub status_bar: bool,
    pub default_terminal: Option<String>,
    /// list-keys, read by the key-binding import (8f).
    pub keys: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    detected: bool,
    pub known: bool,
    pub tab_bug: bool,
    pub server_pid: Option<u32>,
    pub set_titles: bool,
}

impl Handshake {
    /// The kick-off, in iTerm2's order; the phony command must be first.
    pub fn kick_off(&mut self, gw: &mut Gateway, now: Instant) {
        if self.kicked {
            return;
        }
        self.kicked = true;
        let t = Flags { tolerate: true, ..Default::default() };
        self.phony_since = Some(now);
        gw.send("phony-command", Some(tag(Ask::Phony)), t);
        gw.send("refresh-client -fpause-after=0,wait-exit", Some(tag(Ask::Ping)), t);
        gw.send("show-window-options -g aggressive-resize", Some(tag(Ask::AggressiveResize)), Flags::default());
        gw.send("show-option -g -v status", Some(tag(Ask::Status)), Flags::default());
        gw.send("list-sessions -F \"\t\"", Some(tag(Ask::Utf8)), t);
        gw.send("show-options -v -s default-terminal", Some(tag(Ask::DefaultTerminal)), t);
        gw.send("list-keys", Some(tag(Ask::ListKeys)), Flags::default());
        gw.send("copy-mode -q", None, t);
        // Each probe is its own list, as iTerm2 sends them.
        for (cmd, ask) in [
            ("display-message -p \"#{version}\"", Ask::Version),
            ("show-window-options pane-border-format", Ask::Probe23),
            ("list-windows -F \"#{socket_path}\"", Ask::Probe22),
            ("list-windows -F \"#{pid}\"", Ask::Probe21),
            ("show-options -g message-style", Ask::Probe18),
        ] {
            gw.send_list(vec![(cmd.into(), Some(tag(ask)), t)], false);
        }
    }

    /// The phony command is still unanswered after five seconds: "Slow tmux Response", once.
    pub fn phony_slow(&mut self, now: Instant) -> bool {
        if self.phony_since.is_some_and(|t| now.saturating_duration_since(t) >= PHONY_WAIT) {
            self.phony_since = None;
            return true;
        }
        false
    }

    /// When the phony command's wait runs out, while it is unanswered.
    pub fn phony_due(&self) -> Option<Instant> {
        self.phony_since.map(|t| t + PHONY_WAIT)
    }

    /// A reply to one of the handshake's commands (`None`: it failed).
    pub fn reply(&mut self, ask: Ask, text: Option<&str>, gw: &mut Gateway) -> Vec<Step> {
        let mut steps = Vec::new();
        match ask {
            Ask::Phony => self.phony_since = None,
            Ask::Ping | Ask::CopyMode => {}
            Ask::AggressiveResize => {
                for line in text.unwrap_or("").split('\n') {
                    if let [option, value] = line.split(' ').collect::<Vec<_>>()[..] {
                        if option == "aggressive-resize" && value == "on" {
                            steps.push(Step::Abort(
                                "Unsupported tmux option".into(),
                                "The \u{201c}aggressive-resize\u{201d} option is turned on in tmux. It is not compatible with the Tandem-tmux integration. Please disable it and try again.".into(),
                            ));
                            return steps;
                        }
                    }
                }
            }
            Ask::Status => self.status_bar = text == Some("on"),
            Ask::Utf8 => {
                if text.is_some_and(|t| t.contains('_')) {
                    steps.push(Step::Abort(
                        "UTF-8 Mode Not Detected".into(),
                        "tmux is not in UTF-8 mode. Please pass the -u command line argument to tmux or change your LANG environment variable to end with \u{201c}.UTF-8\u{201d}.".into(),
                    ));
                }
            }
            Ask::DefaultTerminal => {
                if let Some(t) = text.filter(|t| !t.is_empty()) {
                    self.default_terminal = Some(t.into());
                }
            }
            Ask::ListKeys => self.keys = text.unwrap_or("").into(),
            Ask::Version => match parse_version(text.unwrap_or("")) {
                // A failed reply reads as empty, as iTerm2's nil string does.
                Version::AtMost(n) => self.lower_max(n),
                Version::Ignored => {}
                Version::Exact { number, tab_bug } => {
                    self.tab_bug |= tab_bug;
                    self.raise_min(number, gw);
                    self.detected = true;
                    self.version_known(gw, &mut steps);
                }
            },
            _ if self.detected => {}
            Ask::Probe23 => match text {
                None => self.lower_max(2.2),
                Some(_) => self.raise_min(2.3, gw),
            },
            Ask::Probe22 => match text.filter(|t| !t.trim().is_empty()) {
                None => self.lower_max(2.1),
                Some(_) => self.raise_min(2.2, gw),
            },
            Ask::Probe21 => match text.filter(|t| !t.is_empty()) {
                None => self.lower_max(2.0),
                Some(_) => self.raise_min(2.1, gw),
            },
            Ask::Probe18 => {
                match text {
                    Some(_) => self.raise_min(1.9, gw),
                    None => self.lower_max(1.8),
                }
                // The oldest version iTerm2 supports: the version is as known as it gets.
                self.version_known(gw, &mut steps);
            }
            Ask::ServerPid => self.server_pid = text.and_then(|t| t.trim().parse().ok()).filter(|p| *p > 0),
            Ask::SetTitles => self.set_titles = text == Some("on"),
        }
        steps
    }

    pub fn at_least(&self, v: f64) -> bool {
        self.min.is_some_and(|m| m >= v)
    }

    fn raise_min(&mut self, v: f64, gw: &mut Gateway) {
        if self.min.is_none_or(|m| m < v) {
            self.min = Some(v);
        }
        gw.v3_2 = self.at_least(3.2);
    }

    fn lower_max(&mut self, v: f64) {
        if self.max.is_none_or(|m| m > v) {
            self.max = Some(v);
        }
    }

    /// iTerm2's didGuessVersion, less pause mode (milestone 8d): the server's pid (2.1+)
    /// and set-titles (2.9+).
    fn version_known(&mut self, gw: &mut Gateway, steps: &mut Vec<Step>) {
        if self.min.unwrap_or(0.0) >= 2.1 {
            gw.send("display-message -p \"#{pid}\"", Some(tag(Ask::ServerPid)), Flags::default());
        }
        if self.at_least(2.9) {
            gw.send_list(vec![("show-options -v -g set-titles".into(), Some(tag(Ask::SetTitles)), Flags::default())], false);
        }
        self.known = true;
        steps.push(Step::VersionKnown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_gateway::Event;

    #[test]
    fn versions_follow_iterm2() {
        let exact = |n| Version::Exact { number: n, tab_bug: false };
        assert_eq!(parse_version("3.4"), exact(3.4));
        assert_eq!(parse_version("3.0a"), exact(3.01));
        assert_eq!(parse_version("2.9a"), exact(2.91));
        assert_eq!(parse_version("3.5-rc"), exact(3.5));
        assert_eq!(parse_version("next-3.4"), Version::Exact { number: 3.4, tab_bug: true });
        assert_eq!(parse_version("next-3.5"), exact(3.5));
        assert_eq!(parse_version("openbsd-7.1"), exact(3.4));
        assert_eq!(parse_version("openbsd-7.4"), exact(3.4));
        assert_eq!(parse_version("openbsd-6.9"), exact(3.2));
        assert_eq!(parse_version("openbsd-6.8"), exact(3.2));
        assert_eq!(parse_version("openbsd-6.7"), exact(3.0));
        assert_eq!(parse_version(""), Version::AtMost(2.3));
        assert_eq!(parse_version("master"), Version::Ignored);
        assert_eq!(parse_version("1.8"), Version::Ignored);
        assert_eq!(parse_version("11.0"), Version::Ignored);
    }

    fn started() -> Gateway {
        let mut g = Gateway::new("\r");
        for l in ["%begin 1 1 0", "%end 1 1 0", "%session-changed $0 main"] {
            g.line(l.as_bytes());
        }
        g
    }

    /// Answers the next queued command with `reply` (None: an error).
    fn answer(g: &mut Gateway, n: u32, reply: Option<&str>) -> Vec<Event> {
        let mut ev = g.line(format!("%begin {n} {n} 1").as_bytes());
        if let Some(r) = reply {
            for l in r.split('\n') {
                ev.extend(g.line(l.as_bytes()));
            }
        }
        let end = if reply.is_some() { "end" } else { "error" };
        ev.extend(g.line(format!("%{end} {n} {n} 1").as_bytes()));
        ev
    }

    fn reply_of(ev: &[Event]) -> Option<(Tag, Option<String>)> {
        ev.iter().find_map(|e| if let Event::Reply { tag, ok, text, .. } = e { Some((*tag, ok.then(|| text.clone()))) } else { None })
    }

    #[test]
    fn handshake_checks_options() {
        let mut g = started();
        let mut h = Handshake::default();
        let t0 = Instant::now();
        h.kick_off(&mut g, t0);
        h.kick_off(&mut g, t0);
        let sent = String::from_utf8(g.take_writes()).unwrap();
        let lines: Vec<&str> = sent.split('\r').filter(|l| !l.is_empty()).collect();
        assert_eq!(
            lines,
            [
                "phony-command",
                "refresh-client -fpause-after=0,wait-exit",
                "show-window-options -g aggressive-resize",
                "show-option -g -v status",
                "list-sessions -F \"\t\"",
                "show-options -v -s default-terminal",
                "list-keys",
                "copy-mode -q",
                "display-message -p \"#{version}\"",
                "show-window-options pane-border-format",
                "list-windows -F \"#{socket_path}\"",
                "list-windows -F \"#{pid}\"",
                "show-options -g message-style",
            ],
            "once, phony first"
        );
        assert_eq!(h.phony_due(), Some(t0 + PHONY_WAIT));
        assert!(!h.phony_slow(t0 + Duration::from_secs(4)));
        assert!(h.phony_slow(t0 + PHONY_WAIT));
        assert!(!h.phony_slow(t0 + PHONY_WAIT * 2), "warned once");
        assert_eq!(h.phony_due(), None);

        let mut steps = Vec::new();
        let replies = [Some(""), Some(""), Some("aggressive-resize off"), Some("on"), Some("\t"), Some("tmux-256color"), Some("bind-key -T prefix c new-window"), None, Some("3.4")];
        for (i, r) in replies.iter().enumerate() {
            // copy-mode -q has no tag: no reply comes back for it.
            let Some((t, text)) = reply_of(&answer(&mut g, i as u32 + 10, *r)) else { continue };
            if let Some(ask) = ask_of(t) {
                steps.extend(h.reply(ask, text.as_deref(), &mut g));
            }
        }
        assert!(steps.is_empty() || steps == [Step::VersionKnown], "{steps:?}");
        assert!(h.status_bar);
        assert_eq!(h.default_terminal.as_deref(), Some("tmux-256color"));
        assert_eq!(h.keys, "bind-key -T prefix c new-window");
        assert_eq!((h.min, h.known), (Some(3.4), true));
        assert!(g.v3_2);
        let follow = String::from_utf8(g.take_writes()).unwrap();
        assert_eq!(follow, "display-message -p \"#{pid}\"\rshow-options -v -g set-titles\r");
        // Probes after an exact version change nothing.
        assert!(h.reply(Ask::Probe18, None, &mut g).is_empty());
        assert_eq!(h.max, None);

        let mut h = Handshake::default();
        let mut g = started();
        let bad = h.reply(Ask::AggressiveResize, Some("aggressive-resize on"), &mut g);
        assert!(matches!(&bad[..], [Step::Abort(t, m)] if t == "Unsupported tmux option" && m.contains("\u{201c}aggressive-resize\u{201d} option is turned on")));
        let bad = h.reply(Ask::Utf8, Some("_"), &mut g);
        assert!(matches!(&bad[..], [Step::Abort(t, _)] if t == "UTF-8 Mode Not Detected"));
    }

    #[test]
    fn old_servers_are_bracketed_by_probes() {
        let mut g = started();
        let mut h = Handshake::default();
        assert!(h.reply(Ask::Version, Some(""), &mut g).is_empty());
        assert_eq!(h.max, Some(2.3));
        h.reply(Ask::Probe23, None, &mut g);
        h.reply(Ask::Probe22, Some("/tmp/tmux-1000/default"), &mut g);
        h.reply(Ask::Probe21, Some("42"), &mut g);
        assert_eq!(h.reply(Ask::Probe18, Some("message-style bg=yellow"), &mut g), vec![Step::VersionKnown]);
        assert_eq!((h.min, h.max), (Some(2.2), Some(2.2)));
        assert!(!g.v3_2);
        assert_eq!(g.take_writes(), b"display-message -p \"#{pid}\"\r");
        assert_eq!(ask_of(FIRST_FREE_TAG), None);
    }
}
