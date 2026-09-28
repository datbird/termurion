//! `execute_command`'s plan and output extraction (`PTYSession.swift:511-595`). Where the
//! session is at a prompt and shell integration has been used, the command is typed and its
//! output is the text up to the next prompt (the prompt path). Otherwise a per-shell marker is
//! appended and the output is the text between the marker's two uuids (the marker path).
//! Pure: the App side (`ai_exec_app`) reads the screen and writes the bytes.

use crate::ai_shell::ShellKind;
use crate::ai_terminal::Outcome;

/// How a command's end is found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Path {
    /// The next prompt mark.
    Prompt,
    /// `-- FINISHED <uuid>` at a line start.
    Marker { uuid: String },
}

/// What to type and how to find the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub path: Path,
    pub bytes: Vec<u8>,
}

/// The text after the command that prints the marker (ruling 9). cmd's `echo` prints the rest
/// of the line verbatim; PowerShell's `Write-Output` prints the quoted text; POSIX and Unknown
/// use iTerm2's own (`PS:552`). In each echo of the command line the uuid is not at a line
/// start after "-- FINISHED ", so only the printed marker line matches `marker_line_seen`.
pub fn marker_suffix(kind: ShellKind, uuid: &str) -> String {
    match kind {
        ShellKind::Cmd => format!(" & echo -- FINISHED {uuid}"),
        ShellKind::PowerShell => format!("; Write-Output \"-- FINISHED {uuid}\""),
        ShellKind::Posix | ShellKind::Unknown => format!(";echo '-- FINISHED' {uuid}"),
    }
}

/// The answer for an `execute_command` whose text holds a control key the allow-list refuses,
/// None when it may go on to the check (Task 7 finding 1, ruling R12cd-2 widened). The same
/// allow-list as `insert_text_at_cursor` (`ai_actions::has_refused_control`) plus CR and LF,
/// so a multi-line command is checked whole; Ctrl+R, Ctrl+P, Ctrl+Y, Esc-prefixed keys and C1
/// controls would make the shell run a history line the check never saw. Called before the
/// check and again before any write (`App::ai_exec_start`).
pub const REFUSED_MODEL: &str = "Error: execute_command cannot run a command that holds a control character other than Tab and newlines. Send the command as plain text.";
pub const REFUSED_NOTICE: &str = "AI tried to run a command holding a control character, so nothing was run.";

pub fn refusal(command: &str) -> Option<Outcome> {
    crate::ai_actions::has_refused_control(command, true)
        .then(|| Outcome { model: REFUSED_MODEL.to_string(), notice: Some(REFUSED_NOTICE.to_string()) })
}

/// The bytes to type: the prompt path when shell integration was ever used and the session is
/// at a prompt (`PS:511-516`), else the marker path.
pub fn plan(command: &str, kind: ShellKind, ever_used: bool, at_prompt: bool, uuid: &str) -> Plan {
    if ever_used && at_prompt {
        Plan { path: Path::Prompt, bytes: format!("{command}\r").into_bytes() }
    } else {
        Plan { path: Path::Marker { uuid: uuid.to_string() }, bytes: format!("{command}{}\r", marker_suffix(kind, uuid)).into_bytes() }
    }
}

/// What the marker path found in the text from the command's start line to the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerFound {
    /// The marker line has not printed yet.
    Waiting,
    /// The text between the two uuids.
    Output(String),
    /// The marker printed but the echoed command line is gone (the start fell off the
    /// scrollback): everything, marker removed.
    Lost(String),
    /// Nothing before the marker.
    Empty,
}

const FINISHED: &str = "-- FINISHED ";

/// iTerm2's `^-- FINISHED <uuid>` on any line: a line start only, so the echoed command line
/// (which holds the uuid after other text) never counts.
pub fn marker_line_seen(text: &str, uuid: &str) -> bool {
    let line = format!("{FINISHED}{uuid}");
    text.lines().any(|l| l.starts_with(&line))
}

/// The output of a marker-path command (`PS:541-576`). Two uuids: the text between them with
/// a trailing "-- FINISHED " removed. One (the echoed line scrolled away): the text with the
/// marker removed. PowerShell's echoed line ends the uuid with its closing quote, dropped.
pub fn marker_output(text: &str, uuid: &str) -> MarkerFound {
    if !marker_line_seen(text, uuid) {
        return MarkerFound::Waiting;
    }
    let at: Vec<usize> = text.match_indices(uuid).map(|(i, _)| i).collect();
    if at.len() == 2 {
        let between = &text[at[0] + uuid.len()..at[1]];
        let between = between.strip_prefix('"').unwrap_or(between);
        return MarkerFound::Output(between.strip_suffix(FINISHED).unwrap_or(between).to_string());
    }
    let marker = format!("{FINISHED}{uuid}");
    let before = text.find(&marker).map_or("", |i| &text[..i]);
    if before.trim().is_empty() {
        return MarkerFound::Empty;
    }
    MarkerFound::Lost(text.replace(&marker, "").replace(uuid, ""))
}

pub fn ran(command: &str) -> String {
    format!("Ran {command}")
}

pub fn lost_notice(command: &str) -> String {
    format!("Ran {command} but its output could not be located. Did you run out of scrollback history lines?")
}

pub fn unavailable_notice(command: &str) -> String {
    format!("Ran {command} but its output was unavailable. Did you run out of scrollback history lines?")
}

pub const UNAVAILABLE_MODEL: &str = "The executeCommand function call failed because the output of the command is not available. The buffer may have been cleared or some other technical issue occurred.";

pub const CANCELED_MODEL: &str = "The function call was canceled by user request.";

/// A command that is running on a pane.
#[derive(Clone, Debug)]
pub struct Running {
    pub chat: String,
    pub request_id: String,
    pub command: String,
    pub path: Path,
    /// The cursor's absolute line before the command was typed.
    pub start_line: i64,
    /// `prompts_seen` before the command was typed.
    pub prompts_before: usize,
}

/// The outcome of a command that ended with its output text.
pub fn output_outcome(command: &str, text: String) -> Outcome {
    Outcome { model: text, notice: Some(ran(command)) }
}

pub fn lost_outcome(command: &str, text: String) -> Outcome {
    Outcome { model: text, notice: Some(lost_notice(command)) }
}

/// The output was not available (the start line was dropped, or nothing came before the marker).
pub fn unavailable_outcome(command: &str) -> Outcome {
    Outcome { model: UNAVAILABLE_MODEL.to_string(), notice: Some(unavailable_notice(command)) }
}

/// A cancelled command: the model is told, the chat shows no notice.
pub fn canceled_outcome() -> Outcome {
    Outcome { model: CANCELED_MODEL.to_string(), notice: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;

    const U: &str = "U";

    #[test]
    fn exec_refuses_control_keys() {
        for bad in ["echo\u{12}", "\u{3}", "\u{7f}", "echo \u{7f}\u{7f}\u{7f}\u{7f}\u{7f}rm -rf ~", "\u{10}", "\u{19}", "a\u{1b}.", "\u{1b}b", "x\u{9b}A", "\u{f}", "a\u{0}", "a\u{18}\u{5}"] {
            let o = refusal(bad).unwrap_or_else(|| panic!("{bad:?} must be refused"));
            assert_eq!((o.model.as_str(), o.notice.as_deref()), (REFUSED_MODEL, Some(REFUSED_NOTICE)), "{bad:?}");
            assert!(!o.model.contains("insert_text_at_cursor") && !o.model.contains("line break") && !o.model.contains("use execute_command"), "own text: {}", o.model);
            assert!(!REFUSED_NOTICE.contains("type"), "{REFUSED_NOTICE}");
        }
        for ok in ["dir", "echo a\necho b", "echo a\r\necho b", "a\tb", "echo \u{e9}\u{4e16}"] {
            assert_eq!(refusal(ok), None, "{ok:?}");
        }
    }

    #[test]
    fn exec_plan_follows_shells() {
        let p = plan("dir", ShellKind::Cmd, true, true, U);
        assert_eq!(p, Plan { path: Path::Prompt, bytes: b"dir\r".to_vec() });
        let p = plan("dir", ShellKind::Cmd, false, true, U);
        assert_eq!(p, Plan { path: Path::Marker { uuid: U.into() }, bytes: b"dir & echo -- FINISHED U\r".to_vec() });
        assert_eq!(plan("dir", ShellKind::Cmd, true, false, U).path, Path::Marker { uuid: U.into() }, "not at a prompt");
        assert_eq!(plan("ls", ShellKind::PowerShell, true, false, U).bytes, b"ls; Write-Output \"-- FINISHED U\"\r".to_vec());
        assert_eq!(plan("ls", ShellKind::Posix, false, false, U).bytes, b"ls;echo '-- FINISHED' U\r".to_vec());
        assert_eq!(plan("ls", ShellKind::Unknown, false, false, U).bytes, b"ls;echo '-- FINISHED' U\r".to_vec());
        assert_eq!(marker_suffix(ShellKind::Cmd, "U"), " & echo -- FINISHED U");
    }

    #[test]
    fn marker_output_follows_iterm2() {
        let t = "C:\\> dir & echo -- FINISHED U\na\nb\n-- FINISHED U\nC:\\>";
        assert_eq!(marker_output(t, U), MarkerFound::Output("\na\nb\n".into()));
        assert_eq!(marker_output("C:\\> dir & echo -- FINISHED U\na", U), MarkerFound::Waiting);
        assert_eq!(marker_output("a\nb\n-- FINISHED U\nC:\\>", U), MarkerFound::Lost("a\nb\n\nC:\\>".into()));
        assert_eq!(marker_output("-- FINISHED U", U), MarkerFound::Empty);
        assert!(marker_line_seen("x\n-- FINISHED U\n", U));
        assert!(!marker_line_seen("x -- FINISHED U", U), "a line start only");
        // PowerShell's echo has the quote before the dash and after the uuid.
        let ps = "PS> ls; Write-Output \"-- FINISHED U\"\na\n-- FINISHED U\nPS>";
        assert!(marker_line_seen(ps, U));
        assert!(!marker_line_seen("PS> ls; Write-Output \"-- FINISHED U\"", U));
        assert_eq!(marker_output(ps, U), MarkerFound::Output("\na\n".into()));
        // A hostile line that names another uuid is not the marker.
        assert_eq!(marker_output("$ x\n-- FINISHED V\n", U), MarkerFound::Waiting);
    }

    #[test]
    fn marker_needs_a_real_line_start() {
        // A row that wraps into the marker text is one line: its start is not the marker's.
        let mut s = Screen::new(20, 5);
        s.feed(b"aaaaaaaaaaaaaaaaaaaa-- FINISHED U\r\n");
        let c = s.abs_cursor_line();
        assert!(!marker_line_seen(&s.abs_text(c - 2, c).unwrap(), U));
        // A real line break before it: the marker line counts.
        let mut t = Screen::new(20, 5);
        t.feed(b"aaaaaaaaaaaaaaaaaaaa\r\n-- FINISHED U\r\n");
        let c = t.abs_cursor_line();
        assert!(marker_line_seen(&t.abs_text(c - 2, c).unwrap(), U));
    }

    #[test]
    fn prompt_output_follows_iterm2() {
        assert_eq!(ran("ls"), "Ran ls");
        assert_eq!(lost_notice("ls"), "Ran ls but its output could not be located. Did you run out of scrollback history lines?");
        assert_eq!(unavailable_notice("ls"), "Ran ls but its output was unavailable. Did you run out of scrollback history lines?");
        assert_eq!(UNAVAILABLE_MODEL, "The executeCommand function call failed because the output of the command is not available. The buffer may have been cleared or some other technical issue occurred.");
        assert_eq!(CANCELED_MODEL, "The function call was canceled by user request.");
        assert_eq!(canceled_outcome().notice, None);
    }
}
