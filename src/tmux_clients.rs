//! Other clients of a tmux session and the reports only one of them may answer (iTerm2's
//! iTermTmuxClientTracker and TmuxController's OSC 4/52 rules), tmux report forwarding
//! (`refresh-client -r`), the paste buffer offer, and a tmux tab's label.

use std::collections::BTreeMap;

/// The clients of one session: which is in control mode, and whether this one answers.
#[derive(Clone, Debug)]
pub struct Tracker {
    /// This client's `#{client_name}`.
    pub me: String,
    pub session: u32,
    clients: BTreeMap<String, bool>,
}

impl Tracker {
    pub fn new(me: &str, session: u32) -> Tracker {
        Tracker { me: me.to_string(), session, clients: BTreeMap::new() }
    }

    pub fn list_command(&self) -> String {
        format!("list-clients -t '${}' -F '#{{client_name}}\t#{{client_control_mode}}'", self.session)
    }

    /// The list-clients reply: the clients, rebuilt from scratch.
    pub fn listed(&mut self, reply: &str) {
        self.clients = reply
            .split('\n')
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.len() == 2).then(|| (f[0].to_string(), f[1] == "1"))
            })
            .collect();
    }

    pub fn detached(&mut self, name: &str) {
        self.clients.remove(name);
    }

    /// No plain client is attached and this is the control client with the smallest name.
    pub fn responsible(&self) -> bool {
        if self.clients.values().any(|control| !control) {
            return false;
        }
        // The map is sorted: the first name is the smallest.
        self.clients.keys().next() == Some(&self.me)
    }
}

/// `#{T:get-clipboard}` as a level: both 3, request 2, buffer 1, else 0.
pub fn get_level(value: &str) -> u8 {
    [("both", 3), ("request", 2), ("buffer", 1)].iter().find(|(p, _)| value.starts_with(p)).map_or(0, |(_, l)| *l)
}

/// `#{T:set-clipboard}` as a level: on 2, external 1, else 0.
pub fn set_level(value: &str) -> u8 {
    [("on", 2), ("external", 1)].iter().find(|(p, _)| value.starts_with(p)).map_or(0, |(_, l)| *l)
}

/// OSC 4 color queries in tmux panes: tmux 3.6 and later leaves them to one client.
pub fn answers_osc4(version: f64, responsible: bool) -> bool {
    version >= 3.6 && responsible
}

/// OSC 52 reads in tmux panes (iTerm2's shouldHandleOSC52Queries).
pub fn answers_osc52(version: f64, responsible: bool, get: u8, set: u8) -> bool {
    if !responsible {
        return false;
    }
    if version >= 3.7 {
        return get >= 2 || set < 2;
    }
    version >= 3.6 && set < 2
}

/// iTerm2's stringByEscapingForTmux: controls as `\ooo`, `\` and `"` with a backslash.
pub fn escape_for_tmux(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            c if (c as u32) < 32 => out.push_str(&format!("\\{:03o}", c as u32)),
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// An OSC 10 or 11 color report: 16 bits a channel, or 8 with `sixteen` off (iTerm2's
/// oscColorReport16Bits).
pub fn color_report(osc: u8, rgb: (u8, u8, u8), sixteen: bool) -> String {
    let (r, g, b) = rgb;
    if !sixteen {
        return format!("\x1b]{osc};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\");
    }
    format!("\x1b]{osc};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\")
}

pub fn report_command(pane: u32, report: &str) -> String {
    format!("refresh-client -r \"%{pane}:{}\"", escape_for_tmux(report))
}

/// The OSC 4 answers in a pane's reply bytes (everything else is tmux's to answer).
pub fn osc4_answers(reply: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(start) = reply[i..].windows(4).position(|w| w == b"\x1b]4;").map(|p| p + i) {
        let body = start + 4;
        let end = (body..reply.len()).find_map(|j| match reply[j] {
            0x07 => Some(j + 1),
            0x1b if reply.get(j + 1) == Some(&b'\\') => Some(j + 2),
            _ => None,
        });
        let Some(end) = end else { break };
        out.extend_from_slice(&reply[start..end]);
        i = end;
    }
    out
}

/// Whether the paste buffer offer shows: sync is off, never answered, and not showing.
pub fn offer_paste_sync(sync_on: bool, answered: bool, bar_showing: bool) -> bool {
    !sync_on && !answered && !bar_showing
}

/// A tmux tab's label (iTerm2's updateTabTitleForCurrentSessionName): the tab's title
/// override (Edit Tab Title, else the set-titles string) with the prefix, else the session
/// title once tmux gave a pane title, else prefix and window name.
pub fn tab_label(user: Option<&str>, set_titles: Option<&str>, pane_title: &str, session_title: &str, window_name: &str, prefix: &str) -> String {
    match user.or(set_titles).filter(|t| !t.is_empty()) {
        Some(t) if t.starts_with(prefix) => t.to_string(),
        Some(t) => format!("{prefix}{t}"),
        None if !pane_title.is_empty() => session_title.to_string(),
        None => format!("{prefix}{window_name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smallest_control_client_answers() {
        let mut t = Tracker::new("client-2", 4);
        assert_eq!(t.list_command(), "list-clients -t '$4' -F '#{client_name}\t#{client_control_mode}'");
        assert!(!t.responsible(), "nothing listed yet");
        t.listed("client-2\t1\nclient-9\t1\nbad line\n");
        assert!(t.responsible());
        t.listed("client-2\t1\nclient-1\t1");
        assert!(!t.responsible(), "a smaller control client answers");
        t.detached("client-1");
        assert!(t.responsible());
        t.listed("client-2\t1\n/dev/pts/3\t0");
        assert!(!t.responsible(), "a plain client answers for itself");
        t.listed("client-9\t1");
        assert!(!t.responsible(), "not listed");
    }

    #[test]
    fn clipboard_duty_follows_iterm2() {
        assert_eq!((get_level("both"), get_level("request"), get_level("buffer"), get_level("off")), (3, 2, 1, 0));
        assert_eq!((set_level("on"), set_level("external"), set_level("off")), (2, 1, 0));
        assert!(answers_osc52(3.7, true, 2, 2), "get-clipboard request");
        assert!(!answers_osc52(3.7, true, 0, 2), "get-clipboard off, set-clipboard on");
        assert!(answers_osc52(3.7, true, 0, 1));
        assert!(answers_osc52(3.6, true, 0, 0) && !answers_osc52(3.6, true, 3, 2));
        assert!(!answers_osc52(3.5, true, 3, 0), "older tmux: never");
        assert!(!answers_osc52(3.7, false, 3, 0));
        assert!(answers_osc4(3.6, true) && !answers_osc4(3.5, true) && !answers_osc4(3.6, false));
    }

    #[test]
    fn reports_escape_for_tmux() {
        let r = color_report(11, (0x1e, 0x1e, 0x1e), true);
        assert_eq!(r, "\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\");
        assert_eq!(report_command(3, &r), "refresh-client -r \"%3:\\033]11;rgb:1e1e/1e1e/1e1e\\033\\\\\"");
        assert_eq!(escape_for_tmux("a\"b\n"), "a\\\"b\\012");
    }

    #[test]
    fn only_osc4_answers_leave_tmux_panes() {
        let reply = b"\x1b[?62;c\x1b]4;1;rgb:cdcd/0000/0000\x07\x1b]11;rgb:0000/0000/0000\x1b\\\x1b]4;2;rgb:0000/cdcd/0000\x1b\\\x1b]4;3";
        assert_eq!(osc4_answers(reply), b"\x1b]4;1;rgb:cdcd/0000/0000\x07\x1b]4;2;rgb:0000/cdcd/0000\x1b\\");
        assert!(osc4_answers(b"\x1b[0n").is_empty());
    }

    #[test]
    fn paste_buffer_offer_is_answered_once() {
        assert!(offer_paste_sync(false, false, false));
        assert!(!offer_paste_sync(true, false, false), "already mirroring");
        assert!(!offer_paste_sync(false, true, false), "Never was chosen");
        assert!(!offer_paste_sync(false, false, true), "one bar at a time");
    }

    #[test]
    fn tmux_tab_labels_follow_iterm2() {
        assert_eq!(tab_label(None, Some("> vim in work"), "host", "host", "bash", "> "), "> vim in work");
        assert_eq!(tab_label(None, Some("vim"), "host", "host", "bash", "> "), "> vim");
        assert_eq!(tab_label(None, None, "host", "host (vim)", "bash", "> "), "host (vim)");
        assert_eq!(tab_label(None, None, "", "tmux", "bash", "> "), "> bash");
        assert_eq!(tab_label(None, Some(""), "", "tmux", "bash", "> "), "> bash");
        assert_eq!(tab_label(Some("logs"), Some("vim"), "host", "host", "bash", "> "), "> logs", "Edit Tab Title wins");
    }

    #[test]
    fn color_reports_follow_the_width_switch() {
        assert_eq!(color_report(11, (0x12, 0x34, 0x56), true), "\x1b]11;rgb:1212/3434/5656\x1b\\");
        assert_eq!(color_report(11, (0x12, 0x34, 0x56), false), "\x1b]11;rgb:12/34/56\x1b\\");
    }
}
