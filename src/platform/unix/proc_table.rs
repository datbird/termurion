//! The Unix process table: the kernel's own reader per OS (`/proc` on Linux, `libproc` on macOS)
//! under one shape. Only the pieces both share live here.

use std::time::Duration;

#[cfg(target_os = "linux")]
pub use crate::platform::linux::proc_table::{open, rows, Opened};
#[cfg(target_os = "macos")]
pub use crate::platform::macos::proc_table::{open, rows, Opened};

/// Sleeps `gap`. Unix cannot wait on a process that is not our child, so a job's end is seen at
/// the next look, at most `jobwatch::FAST` later.
pub fn wait_any(_pids: &[u32], gap: Duration) {
    std::thread::sleep(gap);
}

/// argv as one line: words joined by spaces, a word with a blank in double quotes.
pub fn join_argv<S: AsRef<str>>(args: &[S]) -> String {
    let words: Vec<String> = args
        .iter()
        .map(|a| {
            let a = a.as_ref();
            if a.contains(char::is_whitespace) { format!("\"{a}\"") } else { a.to_string() }
        })
        .collect();
    words.join(" ")
}

/// Splits a NUL-separated argument block into its words, lossily.
pub fn split_nul(block: &[u8]) -> Vec<String> {
    let block = block.strip_suffix(&[0]).unwrap_or(block);
    if block.is_empty() {
        return Vec::new();
    }
    block.split(|&b| b == 0).map(|w| String::from_utf8_lossy(w).into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_joined_with_quotes_for_blanks() {
        assert_eq!(join_argv(&["vim", "a b.txt", "-u"]), "vim \"a b.txt\" -u");
        assert_eq!(join_argv::<&str>(&[]), "");
        assert_eq!(split_nul(b"sh\0-c\0sleep 5\0"), vec!["sh", "-c", "sleep 5"]);
        assert!(split_nul(b"").is_empty());
    }

    #[test]
    fn the_unix_wait_is_a_plain_sleep() {
        let t = std::time::Instant::now();
        wait_any(&[std::process::id()], Duration::from_millis(60));
        assert!(t.elapsed() >= Duration::from_millis(55));
    }
}
