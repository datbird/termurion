//! The tools a chat's model can call (`RemoteCommand.Content`, `AITerm/RemoteCommand.swift`).
//! Pure. This part holds the calls, their Swift JSON form and their status lines; Task 4 adds
//! the registry (names, descriptions, schemas, argument parsing).

use serde_json::{Map, Value};

use crate::chat_message::{case, one_key};

/// One tool call with its arguments (`RC:104-196`), in iTerm2's `allCases` order.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    IsAtPrompt,
    ExecuteCommand { command: String },
    GetLastExitStatus,
    GetCommandHistory { limit: i64 },
    GetLastCommand,
    GetCommandBeforeCursor,
    SearchCommandHistory { query: String },
    GetCommandOutput { id: String },
    GetScreenContents { lines: i64 },
    GetTerminalSize,
    GetShellType,
    DetectSshSession,
    GetRemoteHostname,
    GetUserIdentity,
    GetCurrentDirectory,
    SetClipboard { text: String },
    InsertTextAtCursor { text: String },
    DeleteCurrentLine,
    GetManPage { cmd: String },
    CreateFile { filename: String, content: String },
    FindOnPage { query: String },
    LoadUrl { url: String },
    WebSearch { query: String },
    GetUrl,
    ReadWebPage { starting_line_number: i64, number_of_lines: i64 },
    RestartSession,
}

impl Call {
    /// Swift's case name, the key of the call's JSON object.
    fn case_name(&self) -> &'static str {
        match self {
            Call::IsAtPrompt => "isAtPrompt",
            Call::ExecuteCommand { .. } => "executeCommand",
            Call::GetLastExitStatus => "getLastExitStatus",
            Call::GetCommandHistory { .. } => "getCommandHistory",
            Call::GetLastCommand => "getLastCommand",
            Call::GetCommandBeforeCursor => "getCommandBeforeCursor",
            Call::SearchCommandHistory { .. } => "searchCommandHistory",
            Call::GetCommandOutput { .. } => "getCommandOutput",
            Call::GetScreenContents { .. } => "getScreenContents",
            Call::GetTerminalSize => "getTerminalSize",
            Call::GetShellType => "getShellType",
            Call::DetectSshSession => "detectSSHSession",
            Call::GetRemoteHostname => "getRemoteHostname",
            Call::GetUserIdentity => "getUserIdentity",
            Call::GetCurrentDirectory => "getCurrentDirectory",
            Call::SetClipboard { .. } => "setClipboard",
            Call::InsertTextAtCursor { .. } => "insertTextAtCursor",
            Call::DeleteCurrentLine => "deleteCurrentLine",
            Call::GetManPage { .. } => "getManPage",
            Call::CreateFile { .. } => "createFile",
            Call::FindOnPage { .. } => "searchBrowser",
            Call::LoadUrl { .. } => "loadURL",
            Call::WebSearch { .. } => "webSearch",
            Call::GetUrl => "getURL",
            Call::ReadWebPage { .. } => "readWebPage",
            Call::RestartSession => "restartSession",
        }
    }

    /// Swift's synthesized form: `{"<case>": {"_0": {<argument struct>}}}`.
    pub fn encode(&self) -> Value {
        let mut args = Map::new();
        let mut put = |key: &str, value: Value| {
            args.insert(key.to_string(), value);
        };
        match self {
            Call::ExecuteCommand { command } => put("command", command.as_str().into()),
            Call::GetCommandHistory { limit } => put("limit", (*limit).into()),
            Call::SearchCommandHistory { query } | Call::FindOnPage { query } | Call::WebSearch { query } => put("query", query.as_str().into()),
            Call::GetCommandOutput { id } => put("id", id.as_str().into()),
            Call::GetScreenContents { lines } => put("lines", (*lines).into()),
            Call::SetClipboard { text } | Call::InsertTextAtCursor { text } => put("text", text.as_str().into()),
            Call::GetManPage { cmd } => put("cmd", cmd.as_str().into()),
            Call::CreateFile { filename, content } => {
                put("filename", filename.as_str().into());
                put("content", content.as_str().into());
            }
            Call::LoadUrl { url } => put("url", url.as_str().into()),
            Call::ReadWebPage { starting_line_number, number_of_lines } => {
                put("startingLineNumber", (*starting_line_number).into());
                put("numberOfLines", (*number_of_lines).into());
            }
            _ => {}
        }
        case(self.case_name(), case("_0", Value::Object(args)))
    }

    /// Reads Swift's form. A missing or null argument takes the Swift struct's default ("", 0,
    /// or 100 for `limit`); an argument of the wrong type, an unknown case or a malformed
    /// object gives None.
    pub fn decode(v: &Value) -> Option<Call> {
        let (name, body) = one_key(v)?;
        let args = body.as_object()?.get("_0")?.as_object()?;
        let text = |key: &str| match args.get(key) {
            None | Some(Value::Null) => Some(String::new()),
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => None,
        };
        let number = |key: &str, default: i64| match args.get(key) {
            None | Some(Value::Null) => Some(default),
            Some(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64)),
        };
        Some(match name {
            "isAtPrompt" => Call::IsAtPrompt,
            "executeCommand" => Call::ExecuteCommand { command: text("command")? },
            "getLastExitStatus" => Call::GetLastExitStatus,
            "getCommandHistory" => Call::GetCommandHistory { limit: number("limit", 100)? },
            "getLastCommand" => Call::GetLastCommand,
            "getCommandBeforeCursor" => Call::GetCommandBeforeCursor,
            "searchCommandHistory" => Call::SearchCommandHistory { query: text("query")? },
            "getCommandOutput" => Call::GetCommandOutput { id: text("id")? },
            "getScreenContents" => Call::GetScreenContents { lines: number("lines", 0)? },
            "getTerminalSize" => Call::GetTerminalSize,
            "getShellType" => Call::GetShellType,
            "detectSSHSession" => Call::DetectSshSession,
            "getRemoteHostname" => Call::GetRemoteHostname,
            "getUserIdentity" => Call::GetUserIdentity,
            "getCurrentDirectory" => Call::GetCurrentDirectory,
            "setClipboard" => Call::SetClipboard { text: text("text")? },
            "insertTextAtCursor" => Call::InsertTextAtCursor { text: text("text")? },
            "deleteCurrentLine" => Call::DeleteCurrentLine,
            "getManPage" => Call::GetManPage { cmd: text("cmd")? },
            "createFile" => Call::CreateFile { filename: text("filename")?, content: text("content")? },
            "searchBrowser" => Call::FindOnPage { query: text("query")? },
            "loadURL" => Call::LoadUrl { url: text("url")? },
            "webSearch" => Call::WebSearch { query: text("query")? },
            "getURL" => Call::GetUrl,
            "readWebPage" => Call::ReadWebPage { starting_line_number: number("startingLineNumber", 0)?, number_of_lines: number("numberOfLines", 0)? },
            "restartSession" => Call::RestartSession,
            _ => return None,
        })
    }

    /// The status line shown for the call (`RC:404-459`).
    pub fn markdown_description(&self) -> String {
        let code = |s: &str| truncate32(&escape_code(s));
        match self {
            Call::IsAtPrompt => "Checking if you\u{2019}re at a shell prompt".to_string(),
            Call::ExecuteCommand { command } => format!("Executing `{}`", code(command)),
            Call::GetLastExitStatus => "Checking the exit status of the last command".to_string(),
            Call::GetCommandHistory { .. } => "Reviewing the history of commands you have run in this session".to_string(),
            Call::GetLastCommand => "Viewing the last command you ran in this session".to_string(),
            Call::GetCommandBeforeCursor => "Reading your current command prompt".to_string(),
            Call::SearchCommandHistory { .. } => "Searching the history of commands you have run in this session".to_string(),
            Call::GetCommandOutput { .. } => "Fetching the output of a previously run command".to_string(),
            Call::GetScreenContents { .. } => "Reading the visible screen".to_string(),
            Call::GetTerminalSize => "Querying the size of your terminal window".to_string(),
            Call::GetShellType => "Determining which shell you use".to_string(),
            Call::DetectSshSession => "Checking if you are using SSH".to_string(),
            Call::GetRemoteHostname => "Getting the current host name of this terminal session".to_string(),
            Call::GetUserIdentity => "Checking your username".to_string(),
            Call::GetCurrentDirectory => "Discovering your current directory".to_string(),
            Call::SetClipboard { .. } => "Pasting to the clipboard".to_string(),
            Call::InsertTextAtCursor { text } => format!("Typing `{}` into the current session", code(text)),
            Call::DeleteCurrentLine => "Erasing the current command line".to_string(),
            Call::GetManPage { cmd } => format!("Checking the manpage for `{}`", code(cmd)),
            Call::CreateFile { filename, .. } => format!("Creating {filename}"),
            Call::FindOnPage { query } => format!("Search in browser for {query}"),
            Call::LoadUrl { url } => format!("Navigate to {url}"),
            Call::WebSearch { query } => format!("Search the web for \u{201c}{query}\u{201d}"),
            Call::GetUrl => "Get the current URL".to_string(),
            Call::ReadWebPage { .. } => "View the current web page".to_string(),
            Call::RestartSession => "Restarting this session".to_string(),
        }
    }
}

/// `truncatedWithTrailingEllipsis(to: 32)`.
pub fn truncate32(s: &str) -> String {
    truncated(s, 32)
}

/// `truncatedWithTrailingEllipsis(to: max)` (`Categories/String+iTerm.swift:69-74`): a text
/// longer than `max` characters keeps `max - 1` and gains "\u{2026}". Counts `char`s, not
/// grapheme clusters (Decision).
pub(crate) fn truncated(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// `escapedForMarkdownCode`: each backtick gains a backslash (`AITerm/TerminalCommand.swift`).
pub fn escape_code(s: &str) -> String {
    s.replace('`', "\\`")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_follows_iterm2() {
        assert_eq!(Call::ExecuteCommand { command: "a`b".into() }.markdown_description(), "Executing `a\\`b`");
        let long = "x".repeat(40);
        assert_eq!(Call::ExecuteCommand { command: long }.markdown_description(), format!("Executing `{}\u{2026}`", "x".repeat(31)));
        assert_eq!(Call::ExecuteCommand { command: "y".repeat(32) }.markdown_description(), format!("Executing `{}`", "y".repeat(32)));
        assert_eq!(Call::InsertTextAtCursor { text: "..".into() }.markdown_description(), "Typing `..` into the current session");
        assert_eq!(Call::GetManPage { cmd: "ls".into() }.markdown_description(), "Checking the manpage for `ls`");
        assert_eq!(Call::CreateFile { filename: "f.txt".into(), content: "c".into() }.markdown_description(), "Creating f.txt");
        assert_eq!(Call::RestartSession.markdown_description(), "Restarting this session");
        assert_eq!(Call::GetScreenContents { lines: 0 }.markdown_description(), "Reading the visible screen");
        assert_eq!(Call::IsAtPrompt.markdown_description(), "Checking if you\u{2019}re at a shell prompt");
        assert_eq!(Call::WebSearch { query: "q".into() }.markdown_description(), "Search the web for \u{201c}q\u{201d}");
        assert_eq!(Call::GetCurrentDirectory.markdown_description(), "Discovering your current directory");
    }

    #[test]
    fn truncation_follows_iterm2() {
        assert_eq!(truncate32("abc"), "abc");
        assert_eq!(truncate32(&"\u{e9}".repeat(32)), "\u{e9}".repeat(32));
        assert_eq!(truncate32(&"\u{e9}".repeat(33)), format!("{}\u{2026}", "\u{e9}".repeat(31)));
        assert_eq!(escape_code("a`b``"), "a\\`b\\`\\`");
        assert_eq!(truncated("abcdef", 3), "ab\u{2026}");
        assert_eq!(truncated("abc", 3), "abc");
    }
}
