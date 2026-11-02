//! The Linux process table, from `/proc`.

use crate::platform::proc_table::Row;
use crate::platform::unix::proc_table::{join_argv, split_nul};

/// The fields of `/proc/<pid>/stat` the job code uses.
#[derive(Debug, PartialEq)]
pub struct Stat {
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub pgid: i32,
    pub sid: i32,
    pub tpgid: i32,
    /// Start time in clock ticks since boot.
    pub start: u64,
}

/// Parses one `stat` line. The name sits in parentheses and may hold spaces and parentheses
/// itself, so the fields are counted from the last `)`.
pub fn parse_stat(line: &str) -> Option<Stat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = line[open + 1..close].to_string();
    let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // After the name: state ppid pgrp session tty_nr tpgid ... starttime is the 20th.
    Some(Stat {
        comm,
        state: rest.first()?.chars().next()?,
        ppid: rest.get(1)?.parse().ok()?,
        pgid: rest.get(2)?.parse().ok()?,
        sid: rest.get(3)?.parse().ok()?,
        tpgid: rest.get(5)?.parse().ok()?,
        start: rest.get(19).and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

fn stat_of(pid: u32) -> Option<Stat> {
    // Bytes, not a string: the kernel cuts the name at 15 bytes, which can split a character.
    parse_stat(&String::from_utf8_lossy(&std::fs::read(format!("/proc/{pid}/stat")).ok()?))
}

/// Every live process. A zombie is gone for our purposes, and a process whose group is not its
/// terminal's foreground group (or that has no terminal) is background.
pub fn rows() -> Result<Vec<Row>, String> {
    let dir = std::fs::read_dir("/proc").map_err(|e| format!("/proc: {e}"))?;
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        // The process may end between the listing and the read.
        let Some(s) = stat_of(pid) else { continue };
        if s.state == 'Z' {
            continue;
        }
        out.push(Row { pid, ppid: s.ppid, name: s.comm, background: s.tpgid != s.pgid });
    }
    Ok(out)
}

/// A process seen once: its start time is fixed at open, the rest is read when asked.
pub struct Opened {
    pid: u32,
    start: u64,
}

pub fn open(pid: u32) -> Option<Opened> {
    // A zombie is gone for our purposes.
    stat_of(pid).filter(|s| s.state != 'Z').map(|s| Opened { pid, start: s.start })
}

impl Opened {
    pub fn created(&self) -> u64 {
        self.start
    }

    pub fn image(&self) -> Option<String> {
        let link = std::fs::read_link(format!("/proc/{}/exe", self.pid)).ok()?;
        let path = link.to_string_lossy().into_owned();
        // A binary replaced while it runs reads "path (deleted)".
        Some(path.strip_suffix(" (deleted)").map_or(path.clone(), str::to_string))
    }

    pub fn cmdline(&self) -> Option<String> {
        let words = split_nul(&std::fs::read(format!("/proc/{}/cmdline", self.pid)).ok()?);
        (!words.is_empty()).then(|| join_argv(&words))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines_parse() {
        let s = parse_stat("123 (a) b) S 1 123 123 34816 200 4194304 100 0 0 0 1 2 0 0 20 0 1 0 98765 1000 100").unwrap();
        assert_eq!((s.comm.as_str(), s.ppid, s.pgid, s.sid, s.tpgid), ("a) b", 1, 123, 123, 200));
        assert_eq!((s.state, s.start), ('S', 98765));
        assert!(parse_stat("garbage").is_none());
        assert!(parse_stat("1 (x) S 1").is_none(), "too short to trust");
    }

    #[test]
    fn a_name_cut_inside_a_character_still_parses() {
        // The kernel keeps 15 bytes of the name; here the last character is cut after its first byte.
        let mut line = b"77 (".to_vec();
        line.extend_from_slice(&[0xC3, 0xA9, 0xC3, 0xA9, 0xC3]);
        line.extend_from_slice(b") S 1 77 77 34816 77 0 0 0 0 0 0 0 0 0 20 0 1 0 555 1 1");
        let s = parse_stat(&String::from_utf8_lossy(&line)).unwrap();
        assert_eq!((s.ppid, s.tpgid, s.start), (1, 77, 555));
        assert!(s.comm.starts_with("\u{e9}\u{e9}"));
    }

    #[test]
    fn our_own_row_and_facts_are_there() {
        let me = std::process::id();
        let all = rows().unwrap();
        let mine = all.iter().find(|r| r.pid == me).expect("our row");
        assert_eq!(mine.ppid, std::os::unix::process::parent_id());
        let o = open(me).unwrap();
        assert!(o.created() > 0);
        assert!(o.image().unwrap().starts_with('/'));
        assert!(o.cmdline().is_some());
        assert!(open(4_000_000).is_none());
    }
}
