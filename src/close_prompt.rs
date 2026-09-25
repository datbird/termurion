//! Prompt Before Closing: whether closing a session needs a confirmation, and iTerm2's
//! text for it (`iTermPromptOnCloseReason`).


use crate::proctree::{same_job, strip_exe};

/// `Prompt Before Closing 2`: never, always, or when jobs besides the ignore list run.
pub const NEVER: i64 = 0;
pub const ALWAYS: i64 = 1;
pub const IF_JOBS: i64 = 2;

/// iTerm2 lists at most this many jobs by name.
const LISTED: usize = 3;

/// Console hosts ConPTY starts: never a reason to ask.
const BUILT_IN: [&str; 2] = ["conhost", "OpenConsole"];

const FOOTER: &str = "You can change this setting in Settings > Profiles > Session";

/// "a", "a and b", "a, b, and c".
pub fn oxford(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Why closing a session with this profile needs a confirmation, or None. `jobs` are the
/// image names of every process under the shell (not the shell).
pub fn close_reason(mode: i64, ignore: &[String], profile: &str, jobs: &[String]) -> Option<String> {
    match mode {
        ALWAYS => Some(format!("The profile \u{201c}{profile}\u{201d} always requires confirmation.")),
        IF_JOBS => {
            let running: Vec<String> = jobs
                .iter()
                .filter(|j| !BUILT_IN.iter().any(|b| same_job(b, j)) && !ignore.iter().any(|i| same_job(i, j)))
                .map(|j| strip_exe(j).to_string())
                .collect();
            if running.is_empty() {
                return None;
            }
            Some(if running.len() <= LISTED {
                format!("A session with profile \u{201c}{profile}\u{201d} is running {}.", oxford(&running))
            } else {
                let more = running.len() - LISTED;
                format!("A session with profile \u{201c}{profile}\u{201d} is running {}, and {more} other {}.", running[..LISTED].join(", "), if more == 1 { "job" } else { "jobs" })
            })
        }
        _ => None,
    }
}

/// The confirmation's text: each reason on its own line, then the footer.
pub fn message(reasons: &[String]) -> String {
    format!("{}\n\n{FOOTER}", reasons.join("\n"))
}

/// The profile's Jobs to Ignore.
pub fn ignore_list(p: &crate::profile::Profile) -> Vec<String> {
    p.get("Jobs to Ignore").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
}

pub fn session_title(name: &str) -> String {
    format!("Close session \u{201c}{name}\u{201d}?")
}

pub fn tab_title(number: usize) -> String {
    format!("Close tab #{number}?")
}

pub fn window_title(number: usize) -> String {
    format!("Close Window #{number}?")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn close_reason_texts() {
        let ignore = names(&["ssh"]);
        assert_eq!(close_reason(IF_JOBS, &ignore, "Default", &names(&["vim.exe"])).as_deref(), Some("A session with profile \u{201c}Default\u{201d} is running vim."));
        assert_eq!(close_reason(IF_JOBS, &ignore, "P", &names(&["a", "b"])).as_deref(), Some("A session with profile \u{201c}P\u{201d} is running a and b."));
        assert_eq!(close_reason(IF_JOBS, &ignore, "P", &names(&["a", "b", "c"])).as_deref(), Some("A session with profile \u{201c}P\u{201d} is running a, b, and c."));
        assert_eq!(close_reason(IF_JOBS, &ignore, "P", &names(&["a", "b", "c", "d"])).as_deref(), Some("A session with profile \u{201c}P\u{201d} is running a, b, c, and 1 other job."));
        assert_eq!(close_reason(IF_JOBS, &ignore, "P", &names(&["a", "b", "c", "d", "e"])).as_deref(), Some("A session with profile \u{201c}P\u{201d} is running a, b, c, and 2 other jobs."));
        let msg = message(&["one".to_string(), "two".to_string()]);
        assert_eq!(msg, "one\ntwo\n\nYou can change this setting in Settings > Profiles > Session");
        assert_eq!(session_title("build"), "Close session \u{201c}build\u{201d}?");
        assert_eq!(tab_title(2), "Close tab #2?");
        assert_eq!(window_title(1), "Close Window #1?");
    }

    #[test]
    fn ignore_list_matches_names() {
        let ignore = names(&["SSH", "plink.exe"]);
        assert_eq!(close_reason(IF_JOBS, &ignore, "P", &names(&["ssh.exe", "plink.exe", "conhost.exe", "OpenConsole.exe"])), None);
        assert!(close_reason(IF_JOBS, &ignore, "P", &names(&["ssh.exe", "git.exe"])).unwrap().ends_with("running git."));
    }

    #[test]
    fn never_and_always() {
        let jobs = names(&["vim"]);
        assert_eq!(close_reason(NEVER, &[], "P", &jobs), None);
        assert_eq!(close_reason(ALWAYS, &[], "P", &[]).as_deref(), Some("The profile \u{201c}P\u{201d} always requires confirmation."));
        assert_eq!(close_reason(IF_JOBS, &[], "P", &[]), None, "only the shell");
        assert_eq!(close_reason(7, &[], "P", &jobs), None, "an unknown mode is Never");
    }

    #[test]
    fn jobs_to_ignore_seeds_remote_logins() {
        let p = crate::profile::Profile::new("x");
        assert_eq!(ignore_list(&p), names(&["ssh", "telnet", "plink"]));
        assert_eq!(crate::profile::Profile::new("x").int("Prompt Before Closing 2"), NEVER);
    }
}
