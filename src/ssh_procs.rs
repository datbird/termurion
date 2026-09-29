//! The remote process list (iTerm2's `SSHProcessInfoProvider`, `PIP`): the rows framer.py's
//! autopoll sends (`ps -eo pid,ppid,stat,lstart,command` edits and the CPU value), turned into
//! `proctree` values for the job, the close prompt and the CPU graph. A remote's names are
//! hostile text: every command is cleaned and capped as it is read.

use crate::proctree::{Job, Proc};
use crate::statusbar_metrics::Series;
use std::collections::{BTreeMap, BTreeSet};

/// The most rows kept (a real host has a few thousand; a hostile one fabricates them).
pub const MAX_ROWS: usize = 20_000;

/// The most command characters kept (a hostile remote can send a line of any length).
pub const COMMAND_MAX: usize = 512;

/// The most characters of a program name shown.
pub const NAME_MAX: usize = 64;

/// One `ps` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: i32,
    pub ppid: i32,
    pub fg: bool,
    pub start: Option<i64>,
    /// The command with controls, bidi and line-break characters made spaces, capped at
    /// `COMMAND_MAX` characters.
    pub command: String,
}

/// Whether a character may not reach a title, a prompt or a graph: controls, the bidi
/// controls and isolates, the zero width and line and paragraph separators.
fn hostile(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            // Format characters (general category Cf): invisible, they pad or spoof a name.
            '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{890}'..='\u{891}' | '\u{8e2}' | '\u{180e}'
                | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}'..='\u{e007f}'
        )
}

/// A remote's text made safe: hostile characters become spaces, the ends are trimmed and
/// the length is capped.
fn clean(text: &str, cap: usize) -> String {
    let spaced: String = text.chars().take(cap).map(|c| if hostile(c) { ' ' } else { c }).collect();
    spaced.trim().to_string()
}

/// The next whitespace-separated word and what follows it (which starts at the whitespace).
fn word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    text.split_at(end)
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

impl Row {
    /// PSRow.init (`PIP:35-94`): pid, ppid, stat, an lstart of five words (letters, letters,
    /// digits, hh:mm:ss, digits), then the command. `fg` is a "+" in the stat. An lstart that
    /// does not read as a date leaves `start` empty; a line of another shape is no row.
    pub fn parse(line: &str) -> Option<Row> {
        let (pid, rest) = word(line);
        let (ppid, rest) = word(rest);
        let (stat, rest) = word(rest);
        let (dow, rest) = word(rest);
        let (month, rest) = word(rest);
        let (dom, rest) = word(rest);
        let (time, rest) = word(rest);
        let (year, rest) = word(rest);
        let letters = |t: &str| !t.is_empty() && t.chars().all(char::is_alphabetic);
        let clock: Vec<&str> = time.split(':').collect();
        // The regex needs whitespace after the year, so a row with nothing after it is no row.
        if !digits(pid) || !digits(ppid) || stat.is_empty() || !letters(dow) || !letters(month) || !digits(dom) || clock.len() != 3 || !clock.iter().all(|t| digits(t)) || !digits(year) || rest.is_empty() {
            return None;
        }
        let start = lstart_seconds(&format!("{dow} {month} {dom} {time} {year}"));
        Some(Row { pid: pid.parse().ok()?, ppid: ppid.parse().ok()?, fg: stat.contains('+'), start, command: clean(rest, COMMAND_MAX) })
    }

    /// nameOfProcess (`PIP:326-335`): the last path component of the command's first shell
    /// word (`ssh_args::shell_words` is componentsInShellCommand); a login shell keeps its
    /// leading "-". None for an empty command.
    pub fn name(&self) -> Option<String> {
        let words = crate::ssh_args::shell_words(&self.command);
        let first = words.first()?;
        let last = first.trim_end_matches('/').rsplit('/').next().filter(|l| !l.is_empty()).unwrap_or("/");
        Some(clean(last, NAME_MAX)).filter(|n| !n.is_empty())
    }
}

/// The seconds since the epoch of an `lstart` ("Mon Sep 27 10:00:00 2026", the format
/// "E MMM d HH:mm:ss yyyy"), read as UTC: only differences between rows matter.
pub fn lstart_seconds(text: &str) -> Option<i64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let [dow, month, dom, time, year] = words[..] else { return None };
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if !DAYS.contains(&dow) {
        return None;
    }
    let month = MONTHS.iter().position(|m| *m == month)? as i64 + 1;
    let (dom, year): (i64, i64) = (dom.parse().ok()?, year.parse().ok()?);
    let clock: Vec<i64> = time.split(':').map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let [h, m, s] = clock[..] else { return None };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=9999).contains(&year) || !(1..=in_month).contains(&dom) || h > 23 || m > 59 || s > 59 {
        return None;
    }
    // days_from_civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + dom - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

/// The remote's processes under one framed shell.
pub struct Provider {
    root: i32,
    rows: BTreeMap<i32, Row>,
    tracked: BTreeSet<i32>,
    cpu: Option<f64>,
    /// The CPU value once a second while one is known (the session's CPU publisher,
    /// `PIP:120-135`).
    pub cpu_series: Series,
}

impl Provider {
    pub fn new(root: i32) -> Provider {
        Provider { root, rows: BTreeMap::new(), tracked: BTreeSet::new(), cpu: None, cpu_series: Series::new() }
    }

    /// handle (`PIP:212-238`): the categories `$begin ps` (edits: `+row`, `~row`, `-pid`) and
    /// `$begin cpu` (`=<value>`); empty lines and lines before any `$begin` are ignored, and
    /// a category's lines from every block count. True when the process rows changed.
    pub fn handle(&mut self, output: &str) -> bool {
        #[derive(PartialEq)]
        enum Cat {
            None,
            Ps,
            Cpu,
            Other,
        }
        let mut cat = Cat::None;
        let mut cpu_line: Option<&str> = None;
        let mut changed = false;
        for line in output.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix("$begin ") {
                cat = match name {
                    "ps" => Cat::Ps,
                    "cpu" => Cat::Cpu,
                    _ => Cat::Other,
                };
                continue;
            }
            match cat {
                Cat::Ps => changed |= self.edit(line),
                // Only the first cpu line counts (`PIP:240-243`).
                Cat::Cpu => cpu_line = cpu_line.or(Some(line)),
                Cat::None | Cat::Other => {}
            }
        }
        if let Some(line) = cpu_line {
            self.read_cpu(line);
        }
        changed
    }

    /// One `ProcessEdit` (`PIP:12-33`, `PIP:268-280`): a line that is none is skipped.
    fn edit(&mut self, line: &str) -> bool {
        let mut chars = line.chars();
        let rest = chars.as_str().get(1..).unwrap_or("");
        match chars.next() {
            Some('+' | '~') => {
                let Some(row) = Row::parse(rest) else { return false };
                if self.rows.len() >= MAX_ROWS && !self.rows.contains_key(&row.pid) {
                    return false;
                }
                self.rows.insert(row.pid, row.clone()).as_ref() != Some(&row)
            }
            Some('-') => rest.parse::<i32>().is_ok_and(|pid| self.rows.remove(&pid).is_some()),
            _ => false,
        }
    }

    /// handleCPU (`PIP:240-266`): "=<percent>" with an optional "%"; another line, or a value
    /// that is no finite number, leaves the last one. The value is kept from 0 to 1.
    fn read_cpu(&mut self, line: &str) {
        let Some(value) = line.strip_prefix('=') else { return };
        let value = value.trim();
        let value = value.strip_suffix('%').unwrap_or(value);
        if let Some(percent) = value.parse::<f64>().ok().filter(|v| v.is_finite()) {
            self.cpu = Some((percent / 100.0).clamp(0.0, 1.0));
        }
    }

    /// `register(trackedPID:)`: iTerm2 keeps the set to cache each one's deepest job.
    pub fn track(&mut self, pid: i32) {
        self.tracked.insert(pid);
    }

    pub fn untrack(&mut self, pid: i32) {
        self.tracked.remove(&pid);
    }

    pub fn tracked(&self) -> Vec<i32> {
        self.tracked.iter().copied().collect()
    }

    /// The rows as `proctree` processes (F23: pids are the positive ones). With `all` false
    /// only the root and the foreground rows stay (Decision): the job is the deepest
    /// foreground program, and a background command is never it.
    fn procs_of(&self, all: bool) -> Vec<Proc> {
        self.rows
            .values()
            .filter(|r| all || r.fg || r.pid == self.root)
            .filter_map(|r| {
                Some(Proc {
                    pid: u32::try_from(r.pid).ok()?,
                    ppid: u32::try_from(r.ppid).ok()?,
                    name: r.name().unwrap_or_default(),
                    created: r.start.map_or(0, |t| t.max(0) as u64),
                    gui: false,
                    cmdline: Some(r.command.clone()),
                })
            })
            .collect()
    }

    /// The root and the foreground rows below it, for the job.
    pub fn procs(&self) -> Vec<Proc> {
        self.procs_of(false)
    }

    /// The job of the shell `pid` and the names between it and the shell (lowercased, the
    /// ancestors of `proctree`); the job's name is the remote's own spelling (F23).
    pub fn job(&self, pid: i32) -> (Option<Job>, Vec<String>) {
        let Ok(root) = u32::try_from(pid) else { return (None, Vec::new()) };
        let procs = self.procs();
        let job = crate::proctree::display_job(&procs, root).map(|j| match self.rows.get(&(j.pid as i32)).and_then(Row::name) {
            Some(name) => Job { name, ..j },
            None => j,
        });
        (job, crate::proctree::ancestors(&procs, root))
    }

    /// Every program under the shell `pid` (not the shell), as the remote spells it: what the
    /// close prompt names (iTerm2's `descendants`).
    /// A row with no usable name (an empty or all-invisible command) is no program.
    pub fn descendant_names(&self, pid: i32) -> Vec<String> {
        let Ok(root) = u32::try_from(pid) else { return Vec::new() };
        crate::proctree::descendants(&self.procs_of(true), root).into_iter().filter(|p| !p.name.is_empty()).map(|p| p.name.clone()).collect()
    }

    pub fn cpu(&self) -> Option<f64> {
        self.cpu
    }

    /// The once-a-second publish: nothing until a value is known. True when one was pushed.
    pub fn tick(&mut self) -> bool {
        match self.cpu {
            Some(v) => {
                self.cpu_series.push(v);
                true
            }
            None => false,
        }
    }

    pub fn rows(&self) -> &BTreeMap<i32, Row> {
        &self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_args::shell_words;

    #[test]
    fn ps_rows_follow_iterm2() {
        let r = Row::parse("  4242     1 S+   Mon Sep 27 10:00:00 2026 -zsh").unwrap();
        assert_eq!((r.pid, r.ppid, r.fg, r.command.as_str()), (4242, 1, true, "-zsh"));
        assert_eq!(r.start, lstart_seconds("Mon Sep 27 10:00:00 2026"));
        assert!(r.start.is_some());
        let r = Row::parse("7 1 Ss Tue Feb 29 00:00:00 2000 /usr/sbin/sshd -D").unwrap();
        assert!(!r.fg);
        assert_eq!(r.command, "/usr/sbin/sshd -D");
        assert_eq!(r.name().as_deref(), Some("sshd"));
        assert_eq!(lstart_seconds("Tue Feb 29 00:00:00 2000"), Some(951782400));
        assert_eq!(lstart_seconds("Wed Dec 31 23:59:59 1969"), Some(-1));
        assert_eq!(Row::parse("x 1 S Mon Sep 27 10:00:00 2026 a"), None);
        assert_eq!(Row::parse("1 1 S yesterday a"), None);
        // A date that does not read leaves the row with no start (the regex passes, the
        // formatter fails); a non-date shape is no row.
        let r = Row::parse("5 1 S Mon Foo 27 10:00:00 2026 a").unwrap();
        assert_eq!(r.start, None);
        assert_eq!(lstart_seconds("Mon Feb 30 00:00:00 2001"), None);
        // No command at all is an empty command; odd spacing and a defunct name are kept.
        assert_eq!(Row::parse("9 1 Z Mon Sep 27 10:00:00 2026 ").unwrap().command, "");
        assert_eq!(Row::parse("9 1 Z Mon Sep 27 10:00:00 2026 [sh] <defunct> x").unwrap().command, "[sh] <defunct> x");
        assert_eq!(shell_words("'/usr/bin/my vim' -c \"set nu\" a\\ b"), ["/usr/bin/my vim", "-c", "set nu", "a b"]);
        let r = Row::parse("3 1 S+ Mon Sep 27 10:00:00 2026 '/usr/bin/my vim' -c \"set nu\" a\\ b").unwrap();
        assert_eq!(r.name().as_deref(), Some("my vim"));
        // Hostile text: controls, bidi and line breaks never reach a name, and it is capped.
        let r = Row::parse("4 1 S Mon Sep 27 10:00:00 2026 ev\u{1b}[31mil\u{202e}x\u{2028}y\u{7}z").unwrap();
        assert!(r.command.chars().all(|c| !c.is_control() && !"\u{202e}\u{2028}".contains(c)), "{:?}", r.command);
        let long = format!("6 1 S Mon Sep 27 10:00:00 2026 {}", "a".repeat(10_000));
        let r = Row::parse(&long).unwrap();
        assert!(r.command.chars().count() <= 512, "the command is capped");
        assert!(r.name().unwrap().chars().count() <= 64, "the name is capped");
    }

    const T0: &str = "Mon Sep 27 10:00:00 2026";

    #[test]
    fn provider_follows_iterm2() {
        let mut p = Provider::new(100);
        p.track(100);
        assert_eq!(p.tracked(), [100]);
        assert_eq!(p.tick_probe(), None);
        p.handle(&format!("$begin ps\n+100 1 Ss {T0} -bash\n+200 100 S+ Mon Sep 27 10:01:00 2026 vim /etc/hosts\n$begin cpu\n=12.5\n"));
        let (job, ancestors) = p.job(100);
        assert_eq!(job, Some(Job { name: "vim".into(), title: "vim".into(), cmdline: "vim /etc/hosts".into(), pid: 200 }));
        assert_eq!(ancestors, ["vim"]);
        assert_eq!(p.descendant_names(100), ["vim"]);
        assert_eq!(p.cpu(), Some(0.125));
        // Edits: "~" replaces a row, "-" removes one (`PIP:268-280`).
        p.handle("$begin ps\n~200 100 S+ Mon Sep 27 10:01:00 2026 vim /tmp/x\n-100\n");
        assert_eq!(p.rows().keys().copied().collect::<Vec<_>>(), [200]);
        assert_eq!(p.rows()[&200].command, "vim /tmp/x");
        // CPU: "%" optional; anything else leaves the value (`PIP:240-266`).
        p.handle("$begin cpu\n=37%\n");
        assert_eq!(p.cpu(), Some(0.37));
        p.handle("$begin cpu\n12\n");
        assert_eq!(p.cpu(), Some(0.37));
        p.handle("$begin cpu\n=x\n");
        assert_eq!(p.cpu(), Some(0.37));
        // Empty lines and lines before any "$begin" are ignored (`PIP:212-238`).
        let mut q = Provider::new(1);
        q.handle(&format!("\n+1 0 Ss {T0} init\n\n$begin cpu\n\n=5\n"));
        assert!(q.rows().is_empty());
        assert_eq!(q.cpu(), Some(0.05));
        // Only the change is reported.
        let mut q = Provider::new(1);
        assert!(q.handle(&format!("$begin ps\n+1 0 Ss {T0} init\n")));
        assert!(!q.handle(&format!("$begin ps\n~1 0 Ss {T0} init\n")));
        assert!(!q.handle("$begin cpu\n=1\n"));
        // tick pushes the value once one exists.
        let mut p = Provider::new(100);
        p.tick();
        assert!(p.cpu_series.values().is_empty());
        p.handle("$begin cpu\n=37%\n");
        p.tick();
        assert_eq!(p.cpu_series.values(), [0.37]);
    }

    impl Provider {
        fn tick_probe(&self) -> Option<f64> {
            self.cpu_series.last()
        }
    }

    #[test]
    fn provider_only_shows_the_root_and_foreground_rows() {
        let mut p = Provider::new(100);
        p.handle(&format!("$begin ps\n+100 1 Ss {T0} -bash\n+150 100 S {T0} sleep 99\n+200 100 S+ {T0} vim a\n+300 1 S+ {T0} other\n"));
        let names: Vec<String> = p.procs().iter().map(|q| q.name.clone()).collect();
        assert_eq!(names, ["-bash", "vim", "other"], "a background row is not in procs");
        // The close prompt names every program under the shell (iTerm2's descendants).
        assert_eq!(p.descendant_names(100), ["sleep", "vim"]);
        assert_eq!(p.job(100).0.unwrap().name, "vim");
        assert_eq!(p.job(999), (None, Vec::new()));
    }

    #[test]
    fn a_huge_remote_list_closes_quickly_and_briefly() {
        // A hostile remote lists 20000 distinct programs under the shell: reading them for the
        // close prompt stays fast and the question stays short.
        use crate::close_rules::{body, counted, Subject};
        let mut p = Provider::new(1);
        let mut text = format!("$begin ps\n+1 0 Ss {T0} -zsh\n");
        for i in 2..=MAX_ROWS {
            text.push_str(&format!("+{i} 1 S {T0} /bin/p{i}{}\n", "x".repeat(58)));
        }
        p.handle(&text);
        assert_eq!(p.rows().len(), MAX_ROWS);
        let t = std::time::Instant::now();
        let names = p.descendant_names(1);
        let question = body(Subject::Tab, &counted(&names));
        let took = t.elapsed();
        assert_eq!(names.len(), MAX_ROWS - 1);
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        assert!(question.len() < 4096, "{} bytes", question.len());
        assert!(question.ends_with(&format!("plus {} others.", MAX_ROWS - 1 - crate::close_rules::NAMES_SHOWN)), "{}", &question[question.len() - 40..]);
    }

    #[test]
    fn invisible_and_blank_names_never_count() {
        // Format characters (soft hyphen, Mongolian vowel separator, interlinear annotation,
        // tags, Arabic number signs) never pad or spoof a name.
        for c in ['\u{ad}', '\u{180e}', '\u{fff9}', '\u{fffb}', '\u{e0001}', '\u{e0041}', '\u{e007f}', '\u{600}', '\u{605}', '\u{6dd}', '\u{70f}', '\u{8e2}', '\u{110bd}', '\u{1d173}', '\u{13430}', '\u{1bca0}'] {
            let r = Row::parse(&format!("4 1 S {T0} vi{c}m")).unwrap();
            // The character becomes a space, which ends the program's word.
            assert_eq!(r.name().as_deref(), Some("vi"), "{:?}", c);
            assert_eq!(r.command, "vi m");
        }
        // A row with no command is kept, but a blank name is no program for the close prompt.
        let mut p = Provider::new(1);
        p.handle(&format!("$begin ps\n+1 0 Ss {T0} -zsh\n+2 1 Z {T0} \n+3 1 S {T0} \u{200b}\n+4 1 S {T0} vim\n"));
        assert_eq!(p.rows().len(), 4);
        assert_eq!(p.descendant_names(1), ["vim"]);
    }

    #[test]
    fn hostile_rows_are_bounded() {
        let mut p = Provider::new(1);
        let mut text = String::from("$begin ps\n");
        for i in 0..(MAX_ROWS + 500) {
            text.push_str(&format!("+{} 1 S Mon Sep 27 10:00:00 2026 x\n", i + 1));
        }
        p.handle(&text);
        assert_eq!(p.rows().len(), MAX_ROWS);
        // A cpu value out of range is a rejected value, never a NaN or a huge graph.
        p.handle("$begin cpu\n=nan\n");
        assert_eq!(p.cpu(), None);
        p.handle("$begin cpu\n=1e300\n");
        assert_eq!(p.cpu(), Some(1.0));
        p.handle("$begin cpu\n=-5\n");
        assert_eq!(p.cpu(), Some(0.0));
    }
}
