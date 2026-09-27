//! The Script Console's data (iTerm2's `iTermScriptHistory` and the connection handlers of
//! `iTermScriptConsole`): one entry per script, connection and app-wide log, each with
//! timestamped log lines, the API calls it made and received, and whether it runs.

use std::path::{Path, PathBuf};

use crate::stamps::Parts;

/// The app-wide entry's identifier (`SH:32-43`).
pub const GLOBAL_ID: &str = "iTerm2";
/// The app-wide entry's name ("iTerm2 App", `SH:37`).
pub const GLOBAL_NAME: &str = "Tandem App";
/// Automatic Profile Switching's entry (`SH:45-54`).
pub const APS_ID: &str = "__APS";
pub const APS_NAME: &str = "Automatic Profile Switching";
/// Log lines an entry keeps (`SH:293`).
pub const MAX_LOG_LINES: usize = 1000;
/// Calls an entry keeps (`SH:313-318`).
pub const MAX_CALLS: usize = 100;
/// The most of one call's text kept; the rest becomes "\u{2026}".
pub const MAX_CALL_BYTES: usize = 16384;
/// The longest log line kept whole: output with no newline is broken here, the rest going on
/// as a line of its own with the stamp, so an open line stays bounded (Decision; iTerm2 lets
/// it grow).
pub const MAX_LINE_BYTES: usize = 16384;

/// `line` in pieces of at most `MAX_LINE_BYTES` (whole characters), each after the first
/// starting with the stamp.
fn break_long(line: String, stamp: &str) -> Vec<String> {
    if line.len() <= MAX_LINE_BYTES {
        return vec![line];
    }
    let mut out = Vec::new();
    let mut rest = line;
    while rest.len() > MAX_LINE_BYTES {
        let mut end = MAX_LINE_BYTES;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let tail = format!("{stamp}: {}", &rest[end..]);
        rest.truncate(end);
        out.push(std::mem::replace(&mut rest, tail));
    }
    out.push(rest);
    out
}

/// The name of a connection with no advisory name (`SC:568-571`).
pub use crate::api_auth::UNKNOWN_NAME;
/// Logged when a connection ends (`SC:621`).
pub const CONNECTION_CLOSED: &str = "\nConnection closed.\n";
/// Logged when Terminate is pressed (`SH:191`).
pub const TERMINATE_PRESSED: &str = "\n*Terminate button pressed*\n";

/// Logged when a connection is accepted (`SC:609`).
pub fn connection_accepted(reason: &str) -> String {
    format!("Connection accepted: {reason}\n")
}

/// Which way an API message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// From the script to Tandem.
    In,
    /// From Tandem to the script.
    Out,
}

/// A call's text in the Calls tab (`SH:143-179`): the arrow, the time and the message text
/// with its newline, cut at `MAX_CALL_BYTES`.
pub fn call_text(dir: Dir, stamp: &str, text: &str) -> String {
    let arrow = match dir {
        Dir::In => "\u{2192}",
        Dir::Out => "\u{2190}",
    };
    let body = if text.len() > MAX_CALL_BYTES {
        let mut end = MAX_CALL_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\u{2026}", &text[..end])
    } else {
        text.to_string()
    };
    format!("Script {arrow} Tandem {stamp}:\n{body}\n\n")
}

/// Keeps what is written up to `cap` bytes of whole characters, then refuses more, so a
/// formatter writing into it stops early.
struct Capped {
    text: String,
    cap: usize,
}

impl std::fmt::Write for Capped {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for c in s.chars() {
            if self.text.len() + c.len_utf8() > self.cap {
                return Err(std::fmt::Error);
            }
            self.text.push(c);
        }
        Ok(())
    }
}

/// A message's pretty `Debug` text for the Calls tab, formatted only as far as `call_text`
/// keeps: a few bytes past `MAX_CALL_BYTES`, so `call_text` sees it is longer and cuts it
/// exactly as it would cut the whole text. A multi-megabyte message is never built in full.
pub fn debug_text(value: &dyn std::fmt::Debug) -> String {
    let mut out = Capped { text: String::new(), cap: MAX_CALL_BYTES + 4 };
    // An error only means the cap was reached.
    let _ = std::fmt::write(&mut out, format_args!("{value:#?}"));
    out.text
}

/// A log line's timestamp, "M/d HH:mm:ss.SSS".
pub fn stamp_log(p: &Parts, millis: u16) -> String {
    format!("{}/{} {:02}:{:02}:{:02}.{:03}", p.month, p.day, p.hour, p.minute, p.second, millis)
}

/// The Started column, "M/d HH:mm:ss".
pub fn stamp_started(p: &Parts) -> String {
    format!("{}/{} {:02}:{:02}:{:02}", p.month, p.day, p.hour, p.minute, p.second)
}

/// The lines the Filter keeps (`SC:421-445`): all when it is empty; else those containing it,
/// ignoring case unless it has an uppercase letter.
pub fn filter_lines<'a>(lines: impl Iterator<Item = &'a String>, filter: &str) -> Vec<&'a str> {
    if filter.is_empty() {
        return lines.map(String::as_str).collect();
    }
    if filter.chars().any(char::is_uppercase) {
        lines.filter(|l| l.contains(filter)).map(String::as_str).collect()
    } else {
        let f = filter.to_lowercase();
        lines.filter(|l| l.to_lowercase().contains(&f)).map(String::as_str).collect()
    }
}

/// An Automatic Profile Switching line with its session's name
/// (`AutomaticProfileSwitching/iTermAutomaticProfileSwitcher.m:31`).
pub fn aps_line(session: &str, line: &str) -> String {
    format!("[{session}] {line}\n")
}

/// How a launched script starts again (Restart).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relaunch {
    pub path: PathBuf,
    pub args: Vec<String>,
}

/// One row of the Script Console.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The connection key (a launched script's `ITERM2_KEY`), or `GLOBAL_ID` or `APS_ID`.
    pub id: String,
    pub name: String,
    /// What the script was launched from (a full environment's folder, else the script).
    pub full_path: Option<PathBuf>,
    /// The script that runs (a full environment's main .py).
    pub path: Option<PathBuf>,
    /// Unix seconds.
    pub started: u64,
    pub running: bool,
    pub terminated_by_user: bool,
    pub pids: Vec<u32>,
    pub relaunch: Option<Relaunch>,
    log_lines: Vec<String>,
    continues: bool,
    calls: Vec<String>,
}

impl Entry {
    /// A running entry started at `started` (`SH:95-128`).
    pub fn new(id: &str, name: &str, started: u64) -> Entry {
        Entry {
            id: id.to_string(),
            name: name.to_string(),
            full_path: None,
            path: None,
            started,
            running: true,
            terminated_by_user: false,
            pids: Vec::new(),
            relaunch: None,
            log_lines: Vec::new(),
            continues: false,
            calls: Vec::new(),
        }
    }

    pub fn log_lines(&self) -> &[String] {
        &self.log_lines
    }

    /// Whether the last line waits for more text (the last chunk had no final newline).
    pub fn continues(&self) -> bool {
        self.continues
    }

    pub fn calls(&self) -> impl Iterator<Item = &String> {
        self.calls.iter()
    }

    /// The process Terminate can end: only when there is exactly one (`SH:173-178`).
    pub fn only_pid(&self) -> Option<u32> {
        match self.pids.as_slice() {
            [p] if *p != 0 => Some(*p),
            _ => None,
        }
    }

    /// The Name column: in parentheses once stopped (`SC:188-196`).
    pub fn shown_name(&self) -> String {
        if self.running {
            self.name.clone()
        } else {
            format!("({})", self.name)
        }
    }

    /// The Logs tab's text.
    pub fn logs_text(&self) -> String {
        if self.log_lines.is_empty() {
            return String::new();
        }
        let mut s = self.log_lines.join("\n");
        if !self.continues {
            s.push('\n');
        }
        s
    }

    /// The Calls tab's text (`SC:434-436`).
    pub fn calls_text(&self) -> String {
        self.calls.join("\n")
    }

    /// iTerm2's `appendLogs` (`SH:223-311`): every line starts with the stamp; a chunk
    /// without a final newline leaves its last line open for the next chunk. A line past
    /// `MAX_LINE_BYTES` is broken.
    fn append_log(&mut self, text: &str, stamp: &str) {
        let continuation = self.continues;
        self.continues = !text.ends_with('\n');
        let body = text.strip_suffix('\n').unwrap_or(text);
        let mut out = body.replace('\n', &format!("\n{stamp}: "));
        if !continuation {
            out.insert_str(0, &format!("{stamp}: "));
        }
        let mut lines = out.split('\n').map(str::to_string);
        if continuation {
            if let Some(first) = lines.next() {
                match self.log_lines.pop() {
                    Some(mut last) => {
                        last.push_str(&first);
                        self.log_lines.extend(break_long(last, stamp));
                    }
                    None => self.log_lines.extend(break_long(first, stamp)),
                }
            }
        }
        for line in lines {
            self.log_lines.extend(break_long(line, stamp));
        }
        if self.log_lines.len() > MAX_LOG_LINES {
            let extra = self.log_lines.len() - MAX_LOG_LINES;
            self.log_lines.drain(..extra);
        }
    }
}

/// Every entry, in the order added, and a counter that changes with any change (so a window
/// knows when to redraw).
#[derive(Clone, Debug)]
pub struct History {
    entries: Vec<Entry>,
    generation: u64,
    /// The Automatic Profile Switching entry was added once (`_haveAddedAPSLoggingEntry`):
    /// once cleared it is not added again.
    aps_added: bool,
}

impl History {
    /// The Automatic Profile Switching entry first when its logging is on, then the app-wide
    /// one (`SH:340-351`).
    pub fn new(aps: bool, now: u64) -> History {
        let mut entries = Vec::new();
        if aps {
            entries.push(Entry::new(APS_ID, APS_NAME, now));
        }
        entries.push(Entry::new(GLOBAL_ID, GLOBAL_NAME, now));
        History { entries, generation: 0, aps_added: aps }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// All, or the running ones (`SC:200-206`).
    pub fn filtered(&self, running_only: bool) -> Vec<&Entry> {
        self.entries.iter().filter(|e| !running_only || e.running).collect()
    }

    pub fn add(&mut self, e: Entry) {
        self.entries.push(e);
        self.generation += 1;
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The entry to change; counts as a change.
    pub fn find_mut(&mut self, id: &str) -> Option<&mut Entry> {
        let e = self.entries.iter_mut().find(|e| e.id == id)?;
        self.generation += 1;
        Some(e)
    }

    /// Adds output to an entry's log; empty text and an unknown entry change nothing.
    pub fn add_log(&mut self, id: &str, text: &str, stamp: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(e) = self.find_mut(id) {
            e.append_log(text, stamp);
        }
    }

    /// Adds a call, keeping the newest `MAX_CALLS`.
    pub fn add_call(&mut self, id: &str, dir: Dir, text: &str, stamp: &str) {
        if let Some(e) = self.find_mut(id) {
            e.calls.push(call_text(dir, stamp, text));
            if e.calls.len() > MAX_CALLS {
                let extra = e.calls.len() - MAX_CALLS;
                e.calls.drain(..extra);
            }
        }
    }

    /// The entry stopped running (`SH:207-211`).
    pub fn stop(&mut self, id: &str) {
        if let Some(e) = self.find_mut(id) {
            e.running = false;
        }
    }

    /// Terminate (`SH:190-196`): the note in the log, terminated by the user, and the process
    /// to end, if there is exactly one. The entry stops when its process ends.
    pub fn kill(&mut self, id: &str, stamp: &str) -> Option<u32> {
        self.find(id)?;
        self.add_log(id, TERMINATE_PRESSED, stamp);
        let e = self.find_mut(id)?;
        e.terminated_by_user = true;
        e.only_pid()
    }

    /// The API stopped (`SH:182-188`): every running entry, the app-wide and Automatic Profile
    /// Switching ones too (each observes the stop, `SH:131-134`), is terminated by the user and
    /// stops; the processes to end are those of entries with exactly one.
    pub fn api_stopped(&mut self) -> Vec<u32> {
        let mut pids = Vec::new();
        for e in self.entries.iter_mut().filter(|e| e.running) {
            e.terminated_by_user = true;
            e.running = false;
            pids.extend(e.only_pid());
        }
        self.generation += 1;
        pids
    }

    /// Clear Terminated (`SH:413-424`).
    pub fn clear_terminated(&mut self) {
        let before = self.entries.len();
        self.entries.retain(|e| e.running);
        if self.entries.len() != before {
            self.generation += 1;
        }
    }

    /// The running entry of this script (`SH:432-437`).
    pub fn running_with_path(&self, p: &Path) -> Option<&Entry> {
        self.entries.iter().find(|e| e.running && e.path.as_deref() == Some(p))
    }

    /// The running entry launched from this path (`SH:439-444`).
    pub fn running_with_full_path(&self, p: &Path) -> Option<&Entry> {
        self.entries.iter().find(|e| e.running && e.full_path.as_deref() == Some(p))
    }

    /// Adds the Automatic Profile Switching entry at the end the first time it is needed
    /// (logging turned on after start, `SH:353-366`); never again once added, even after
    /// Clear Terminated removed it.
    pub fn ensure_aps(&mut self, now: u64) {
        if !self.aps_added {
            self.aps_added = true;
            self.add(Entry::new(APS_ID, APS_NAME, now));
        }
    }

    /// An accepted connection (`SC:591-614`): its entry (a launched script's, else a new one
    /// with this name) logs why.
    pub fn accepted(&mut self, key: &str, name: &str, reason: &str, stamp: &str, now: u64) {
        if self.find(key).is_none() {
            self.add(Entry::new(key, shown_or_unknown(name), now));
        }
        self.add_log(key, &connection_accepted(reason), stamp);
    }

    /// A refused connection (`SC:553-589`): its entry by key, else a new one (under `fresh`
    /// when there is no key) logs the reason and stops.
    pub fn rejected(&mut self, key: Option<&str>, name: &str, reason: &str, stamp: &str, fresh: &str, now: u64) {
        let id = key.unwrap_or(fresh);
        if self.find(id).is_none() {
            self.add(Entry::new(id, shown_or_unknown(name), now));
        }
        self.add_log(id, reason, stamp);
        self.stop(id);
    }

    /// A connection ended (`SC:616-623`); an unknown key changes nothing.
    pub fn closed(&mut self, key: &str, stamp: &str) {
        if self.find(key).is_none() {
            return;
        }
        self.add_log(key, CONNECTION_CLOSED, stamp);
        self.stop(key);
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

fn shown_or_unknown(name: &str) -> &str {
    if name.is_empty() {
        UNKNOWN_NAME
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(h: &History, id: &str) -> Vec<String> {
        h.find(id).unwrap().log_lines().to_vec()
    }

    fn launched(h: &mut History) {
        let mut e = Entry::new("k", "tool", 5);
        e.pids = vec![42];
        e.path = Some(PathBuf::from(r"C:\S\tool.py"));
        e.full_path = Some(PathBuf::from(r"C:\S\tool.py"));
        h.add(e);
    }

    #[test]
    fn log_lines_follow_iterm2() {
        let mut h = History::new(false, 0);
        h.add(Entry::new("k", "tool", 0));
        h.add_log("k", "a\nb\n", "S1");
        assert_eq!(lines(&h, "k"), ["S1: a", "S1: b"]);
        assert!(!h.find("k").unwrap().continues());
        assert_eq!(h.find("k").unwrap().logs_text(), "S1: a\nS1: b\n");
        h.add_log("k", "c", "S2");
        assert_eq!(lines(&h, "k"), ["S1: a", "S1: b", "S2: c"]);
        assert!(h.find("k").unwrap().continues());
        assert_eq!(h.find("k").unwrap().logs_text(), "S1: a\nS1: b\nS2: c");
        h.add_log("k", "d\n", "S3");
        assert_eq!(lines(&h, "k"), ["S1: a", "S1: b", "S2: cd"]);
        assert!(!h.find("k").unwrap().continues());
        h.add_log("k", "e\nf", "S4");
        assert_eq!(lines(&h, "k"), ["S1: a", "S1: b", "S2: cd", "S4: e", "S4: f"]);
        assert!(h.find("k").unwrap().continues());

        let g = h.generation();
        h.add_log("k", "", "S5");
        assert_eq!(h.generation(), g);
        assert_eq!(lines(&h, "k").len(), 5);

        h.add_log("k", "\n", "S6");
        let big: String = (0..1500).map(|i| format!("{i}\n")).collect();
        h.add_log("k", &big, "S7");
        let l = lines(&h, "k");
        assert_eq!(l.len(), MAX_LOG_LINES);
        assert_eq!(l[0], "S7: 500");
        assert_eq!(l[999], "S7: 1499");

        let g = h.generation();
        h.add_log("nobody", "x\n", "S8");
        assert_eq!(h.generation(), g);
        assert!(h.find("nobody").is_none());
    }

    #[test]
    fn long_lines_are_broken() {
        let mut h = History::new(false, 0);
        h.add(Entry::new("k", "tool", 0));
        // One line with no newline, longer than the limit: broken, the rest open.
        h.add_log("k", &"x".repeat(20000), "S1");
        let l = lines(&h, "k");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].len(), MAX_LINE_BYTES);
        assert_eq!(l[0], format!("S1: {}", "x".repeat(MAX_LINE_BYTES - 4)));
        assert_eq!(l[1], format!("S1: {}", "x".repeat(20000 - (MAX_LINE_BYTES - 4))));
        assert!(h.find("k").unwrap().continues());
        // More of the same line goes on the open piece and is broken again at the limit.
        h.add_log("k", &"y".repeat(20000), "S2");
        let l = lines(&h, "k");
        assert!(l.iter().all(|line| line.len() <= MAX_LINE_BYTES), "{:?}", l.iter().map(String::len).collect::<Vec<_>>());
        assert_eq!(l.len(), 3);
        assert!(l[2].starts_with("S2: "));
        let text: String = l.iter().map(|line| line.trim_start_matches("S1: ").trim_start_matches("S2: ")).collect();
        assert_eq!(text, format!("{}{}", "x".repeat(20000), "y".repeat(20000)));
        // A break never splits a character.
        h.add_log("k", "\n", "S3");
        // "S45: " is five bytes, so the limit falls inside a two-byte character.
        h.add_log("k", &"\u{e9}".repeat(10000), "S45");
        let l = lines(&h, "k");
        assert!(l.iter().all(|line| line.len() <= MAX_LINE_BYTES));
        assert_eq!(l[3].len(), MAX_LINE_BYTES - 1);
        assert_eq!(l[4], format!("S45: {}", "\u{e9}".repeat(10000 - (MAX_LINE_BYTES - 6) / 2)));
    }

    #[test]
    fn calls_follow_iterm2() {
        let mut h = History::new(false, 0);
        h.add(Entry::new("k", "tool", 0));
        h.add_call("k", Dir::In, "x", "S");
        h.add_call("k", Dir::Out, "x", "S");
        let calls: Vec<&String> = h.find("k").unwrap().calls().collect();
        assert_eq!(calls[0], "Script \u{2192} Tandem S:\nx\n\n");
        assert_eq!(calls[1], "Script \u{2190} Tandem S:\nx\n\n");
        assert_eq!(h.find("k").unwrap().calls_text(), "Script \u{2192} Tandem S:\nx\n\n\nScript \u{2190} Tandem S:\nx\n\n");

        for i in 0..101 {
            h.add_call("k", Dir::In, &format!("{i}"), "S");
        }
        let calls: Vec<&String> = h.find("k").unwrap().calls().collect();
        assert_eq!(calls.len(), MAX_CALLS);
        assert_eq!(calls[0], "Script \u{2192} Tandem S:\n1\n\n");
        assert_eq!(calls[99], "Script \u{2192} Tandem S:\n100\n\n");

        // Three-byte characters, so the limit falls inside one.
        let long = "\u{2192}".repeat(20000 / 3 + 1);
        assert!(long.len() >= 20000);
        h.add_call("k", Dir::In, &long, "S");
        let last = h.find("k").unwrap().calls().last().unwrap().clone();
        let head = "Script \u{2192} Tandem S:\n";
        let body = last.strip_prefix(head).unwrap().strip_suffix("\u{2026}\n\n").unwrap();
        assert!(body.len() <= MAX_CALL_BYTES && body.len() > MAX_CALL_BYTES - 3, "{}", body.len());
        assert!(long.starts_with(body));
    }

    #[test]
    fn entries_follow_iterm2() {
        let h = History::new(false, 0);
        assert_eq!(h.entries().len(), 1);
        let g = &h.entries()[0];
        assert_eq!((g.id.as_str(), g.name.as_str(), g.running), ("iTerm2", "Tandem App", true));
        assert_eq!(GLOBAL_NAME, "Tandem App");

        let h = History::new(true, 0);
        let ids: Vec<&str> = h.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["__APS", "iTerm2"]);
        assert_eq!(h.entries()[0].name, "Automatic Profile Switching");

        let mut h = History::new(false, 0);
        launched(&mut h);
        let tool = PathBuf::from(r"C:\S\tool.py");
        assert_eq!(h.find("k").unwrap().shown_name(), "tool");
        assert_eq!(h.find("k").unwrap().only_pid(), Some(42));
        assert_eq!(h.running_with_path(&tool).unwrap().id, "k");
        assert_eq!(h.running_with_full_path(&tool).unwrap().id, "k");
        assert_eq!(h.filtered(true).len(), 2);

        assert_eq!(h.kill("k", "S"), Some(42));
        let k = h.find("k").unwrap();
        assert!(k.terminated_by_user && k.running);
        assert_eq!(k.log_lines(), ["S: ", "S: *Terminate button pressed*"]);
        assert_eq!(h.kill("nobody", "S"), None);

        h.stop("k");
        assert_eq!(h.find("k").unwrap().shown_name(), "(tool)");
        assert!(h.running_with_path(&tool).is_none());
        assert!(h.running_with_full_path(&tool).is_none());
        let running: Vec<&str> = h.filtered(true).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(running, ["iTerm2"]);
        assert_eq!(h.filtered(false).len(), 2);

        h.clear_terminated();
        let ids: Vec<&str> = h.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["iTerm2"]);

        // API stop: one pid, two pids, none; the app-wide and APS entries stop too.
        let mut h = History::new(true, 0);
        launched(&mut h);
        let mut two = Entry::new("two", "two", 0);
        two.pids = vec![7, 8];
        h.add(two);
        h.add(Entry::new("conn", "\u{2248}c", 0));
        let mut done = Entry::new("done", "done", 0);
        done.pids = vec![9];
        done.running = false;
        h.add(done);
        let g = h.generation();
        assert_eq!(h.api_stopped(), vec![42]);
        assert!(h.generation() > g);
        for id in ["k", "two", "conn"] {
            let e = h.find(id).unwrap();
            assert!(!e.running && e.terminated_by_user, "{id}");
        }
        assert!(!h.find("done").unwrap().terminated_by_user);
        for id in [GLOBAL_ID, APS_ID] {
            let e = h.find(id).unwrap();
            assert!(!e.running && e.terminated_by_user, "{id}");
        }
        h.clear_terminated();
        assert!(h.entries().is_empty());
        // Cleared, the two are gone for good, as iTerm2's unlisted singletons.
        h.ensure_aps(5);
        let g = h.generation();
        h.add_log(GLOBAL_ID, "x\n", "S");
        h.add_log(APS_ID, "x\n", "S");
        assert!(h.entries().is_empty());
        assert_eq!(h.generation(), g);

        let mut h = History::new(false, 0);
        h.ensure_aps(3);
        h.ensure_aps(4);
        let ids: Vec<&str> = h.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["iTerm2", "__APS"]);
        assert_eq!(h.entries()[1].started, 3);
        h.find_mut(APS_ID).unwrap().running = false;
        h.clear_terminated();
        h.ensure_aps(5);
        let ids: Vec<&str> = h.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["iTerm2"], "added once only");
    }

    #[test]
    fn calls_are_formatted_only_as_far_as_kept() {
        // The same stored call as formatting the whole message and cutting it.
        let big = vec![7u8; 200_000];
        let whole = format!("{big:#?}");
        assert!(whole.len() > MAX_CALL_BYTES);
        assert_eq!(call_text(Dir::Out, "S", &debug_text(&big)), call_text(Dir::Out, "S", &whole));
        let small = vec![1u8, 2];
        assert_eq!(debug_text(&small), format!("{small:#?}"));
        // Three-byte characters around the cut.
        let arrows = "\u{2192}".repeat(MAX_CALL_BYTES);
        assert_eq!(call_text(Dir::In, "S", &debug_text(&arrows)), call_text(Dir::In, "S", &format!("{arrows:#?}")));

        // A multi-megabyte screenshot reply stays bounded.
        use crate::api_proto::pb;
        let png = vec![0u8; 8 << 20];
        let m = pb::ScreenshotResponse { png: Some(png), ..Default::default() };
        let text = debug_text(&m);
        assert!(text.len() <= MAX_CALL_BYTES + 4);
        let stored = call_text(Dir::Out, "S", &text);
        assert!(stored.len() <= MAX_CALL_BYTES + 64 && stored.ends_with("\u{2026}\n\n"));

        // A formatter that would write for ever is stopped.
        struct Endless;
        impl std::fmt::Debug for Endless {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                loop {
                    f.write_str("xxxxxxxx")?;
                }
            }
        }
        assert_eq!(debug_text(&Endless).len(), MAX_CALL_BYTES + 4);
    }

    #[test]
    fn connections_follow_iterm2() {
        let mut h = History::new(false, 0);
        launched(&mut h);
        h.accepted("k2", "\u{2248}demo.py", "Script launched by user action", "S", 9);
        let e = h.find("k2").unwrap();
        assert_eq!((e.name.as_str(), e.running, e.started), ("\u{2248}demo.py", true, 9));
        assert_eq!(e.log_lines(), ["S: Connection accepted: Script launched by user action"]);
        assert_eq!(connection_accepted("r"), "Connection accepted: r\n");

        h.accepted("k", "\u{2248}other", "r", "S", 0);
        assert_eq!(h.find("k").unwrap().name, "tool");
        assert_eq!(h.find("k").unwrap().log_lines(), ["S: Connection accepted: r"]);
        assert_eq!(h.entries().len(), 3);

        h.rejected(None, "\u{2248}x.py", "nope", "S", "F", 0);
        let f = h.find("F").unwrap();
        assert_eq!((f.name.as_str(), f.running), ("\u{2248}x.py", false));
        assert_eq!(f.log_lines(), ["S: nope"]);

        h.rejected(Some("k"), "\u{2248}x.py", "denied", "S", "G", 0);
        let k = h.find("k").unwrap();
        assert!(!k.running);
        assert_eq!(k.name, "tool");
        assert_eq!(k.log_lines().last().unwrap(), "S: denied");
        assert!(h.find("G").is_none());
        assert_eq!(h.entries().iter().filter(|e| e.id == "k").count(), 1);

        h.rejected(Some("new"), "", "why", "S", "H", 0);
        assert_eq!(h.find("new").unwrap().name, "Unknown");
        h.accepted("anon", "", "r", "S", 0);
        assert_eq!(h.find("anon").unwrap().name, UNKNOWN_NAME);

        h.closed("k2", "S");
        let e = h.find("k2").unwrap();
        assert!(!e.running);
        assert_eq!(e.log_lines()[1..], ["S: ", "S: Connection closed."]);
        let g = h.generation();
        let n = h.entries().len();
        h.closed("nobody", "S");
        assert_eq!((h.generation(), h.entries().len()), (g, n));
        assert_eq!(CONNECTION_CLOSED, "\nConnection closed.\n");
        assert_eq!(TERMINATE_PRESSED, "\n*Terminate button pressed*\n");
    }

    #[test]
    fn filters_follow_iterm2() {
        let l: Vec<String> = ["Alpha", "beta", "ALPHA"].iter().map(|s| s.to_string()).collect();
        assert_eq!(filter_lines(l.iter(), "alpha"), ["Alpha", "ALPHA"]);
        assert_eq!(filter_lines(l.iter(), "Alpha"), ["Alpha"]);
        assert_eq!(filter_lines(l.iter(), ""), ["Alpha", "beta", "ALPHA"]);
    }

    #[test]
    fn stamps_follow_the_form() {
        let p = Parts { month: 9, day: 7, hour: 14, minute: 3, second: 5, ..Default::default() };
        assert_eq!(stamp_log(&p, 42), "9/7 14:03:05.042");
        assert_eq!(stamp_started(&p), "9/7 14:03:05");
        assert_eq!(aps_line("bash", "host=x"), "[bash] host=x\n");
    }
}
