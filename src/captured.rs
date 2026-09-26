//! The Capture Output trigger's store: what it saves on a command mark, and the rules the
//! Captured Output tool reads it by (iTerm2's `CapturedOutput`, `CaptureTrigger.m`,
//! `ToolCapturedOutputView.m`).

/// One capture: the whole matched line, its regex groups, the parameter with backreferences
/// replaced (the coprocess to run, if any), whether it is checked off, and where it lives.
#[derive(Clone, Debug, PartialEq)]
pub struct Captured {
    pub line: String,
    pub values: Vec<String>,
    pub command: Option<String>,
    pub state: bool,
    pub abs_line: i64,
    pub mark: u32,
}

/// Adds a capture, merging into the last entry when `canMergeFrom:` would (`CapturedOutput.m`,
/// `VT100ScreenMark.m:657-673`): same line, the new text starts with the old, same command,
/// state and mark.
pub fn add(list: &mut Vec<Captured>, c: Captured) {
    if let Some(last) = list.last_mut() {
        let can_merge = last.abs_line == c.abs_line && last.mark == c.mark && last.command == c.command && last.state == c.state && c.line.starts_with(last.line.as_str());
        if can_merge {
            *last = c;
            return;
        }
    }
    list.push(c);
}

/// The tool's row label: a check mark or a dash, then the line, no space (`CO:289-299`).
pub fn label(c: &Captured) -> String {
    let mark = if c.state { "\u{2714}" } else { "-" };
    format!("{mark}{}", c.line)
}

/// Indices of `list` whose label contains `query`, case-insensitively (`CO:169-177`).
pub fn filter(list: &[Captured], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    list.iter().enumerate().filter(|(_, c)| label(c).to_lowercase().contains(&q)).map(|(i, _)| i).collect()
}

/// What a Clear remembers (the tool's Clear button, or OSC 1337 ClearCapturedOutput): the mark
/// it was cleared for, the last entry's line at that moment, and the clear generation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cleared {
    pub mark: Option<u32>,
    pub abs_line: i64,
    pub clear_count: u32,
}

/// A Clear for `mark`: the highest line among its entries (or `i64::MIN` with none), tagged
/// with `clear_count`.
pub fn clear(list: &[Captured], mark: u32, clear_count: u32) -> Cleared {
    let abs_line = list.iter().filter(|c| c.mark == mark).map(|c| c.abs_line).max().unwrap_or(i64::MIN);
    Cleared { mark: Some(mark), abs_line, clear_count }
}

/// Entries of `list` for `mark` that should show: hidden up to `cleared`'s line when it is for
/// this mark; a `cleared.clear_count` above the caller's own `clear_count` means the caller's
/// view is stale, so nothing shows until it catches up.
pub fn visible(list: &[Captured], mark: u32, clear_count: u32, cleared: &Cleared) -> Vec<usize> {
    let for_this_mark = cleared.mark == Some(mark);
    if for_this_mark && cleared.clear_count > clear_count {
        return Vec::new();
    }
    list.iter().enumerate().filter(|(_, c)| c.mark == mark && !(for_this_mark && c.abs_line <= cleared.abs_line)).map(|(i, _)| i).collect()
}

/// The Capture Output trigger's warnings (`PS:25167-25233`).
pub const NOT_VISIBLE: &str = "A Capture Output trigger fired, but the Captured Output tool is not visible.";
pub const NO_SHELL_INTEGRATION: &str = "A Capture Output trigger fired, but Shell Integration is not installed.";
pub const SUPPRESS_KEY: &str = "NoSyncSuppressCaptureOutputToolNotVisibleWarning";
pub const SUPPRESS_SI_KEY: &str = "NoSyncSuppressCaptureOutputRequiresShellIntegrationWarning";
pub const HELP_URL: &str = "http://iterm2.com/captured_output.html";
pub const RUN_TITLE: &str = "Captured Output trigger";

#[cfg(test)]
mod tests {
    use super::*;

    fn c(line: &str, values: &[&str], command: Option<&str>, abs_line: i64, mark: u32) -> Captured {
        Captured { line: line.into(), values: values.iter().map(|s| s.to_string()).collect(), command: command.map(str::to_string), state: false, abs_line, mark }
    }

    #[test]
    fn capture_follows_iterm2() {
        let mut list = Vec::new();
        add(&mut list, c("error: x", &["x"], Some("run x"), 10, 1));
        assert_eq!(list.len(), 1);
        // Same line, same command, growing text (a progress line): merges into the last entry.
        add(&mut list, c("error: xy", &["xy"], Some("run x"), 10, 1));
        assert_eq!(list.len(), 1, "merged into the last entry");
        assert_eq!(list[0].line, "error: xy");
        assert_eq!(list[0].values, vec!["xy".to_string()]);
        // A different line appends.
        add(&mut list, c("error: z", &["z"], Some("run x"), 11, 1));
        assert_eq!(list.len(), 2);

        let cleared = clear(&list, 1, 1);
        assert_eq!(cleared, Cleared { mark: Some(1), abs_line: 11, clear_count: 1 });
        assert!(visible(&list, 1, 1, &cleared).is_empty(), "hides entries up to the cleared line");

        add(&mut list, c("error: later", &["later"], Some("run x"), 12, 1));
        assert_eq!(visible(&list, 1, 1, &cleared), vec![2], "shows later ones");

        // A different mark ignores the clear entirely.
        add(&mut list, c("other", &[], None, 5, 2));
        assert_eq!(visible(&list, 2, 0, &cleared), vec![3]);

        // A clear_count above the caller's own shows nothing.
        assert!(visible(&list, 1, 0, &cleared).is_empty(), "a clear_count above the view's shows nothing");
    }

    #[test]
    fn labels_follow_iterm2() {
        let done = Captured { line: "build ok".into(), values: vec![], command: None, state: true, abs_line: 0, mark: 1 };
        let pending = Captured { line: "error: x".into(), values: vec![], command: None, state: false, abs_line: 0, mark: 1 };
        assert_eq!(label(&done), "\u{2714}build ok");
        assert_eq!(label(&pending), "-error: x");
        let list = vec![done, pending];
        assert_eq!(filter(&list, "ERR"), vec![1]);
        assert_eq!(filter(&list, "-e"), vec![1], "matches the prefix too");
    }
}
