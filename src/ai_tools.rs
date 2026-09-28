//! The tools a chat's model can call (`RemoteCommand.Content`, `AITerm/RemoteCommand.swift`).
//! Pure: the calls, their Swift JSON form, status lines and consent texts, and the registry the
//! model sees (names, descriptions, schemas, argument parsing).

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::ai_permissions::Category;
use crate::ai_turn::ToolDecl;
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
            Call::CreateFile { filename, .. } => format!("Creating {}", shown(filename)),
            Call::FindOnPage { query } => format!("Search in browser for {}", shown(query)),
            Call::LoadUrl { url } => format!("Navigate to {}", shown(url)),
            Call::WebSearch { query } => format!("Search the web for \u{201c}{}\u{201d}", shown(query)),
            Call::GetUrl => "Get the current URL".to_string(),
            Call::ReadWebPage { .. } => "View the current web page".to_string(),
            Call::RestartSession => "Restarting this session".to_string(),
        }
    }

    /// The call's place in `allCases` (and so in `TOOLS` and `ALL_NAMES`).
    fn index(&self) -> usize {
        match self {
            Call::IsAtPrompt => 0,
            Call::ExecuteCommand { .. } => 1,
            Call::GetLastExitStatus => 2,
            Call::GetCommandHistory { .. } => 3,
            Call::GetLastCommand => 4,
            Call::GetCommandBeforeCursor => 5,
            Call::SearchCommandHistory { .. } => 6,
            Call::GetCommandOutput { .. } => 7,
            Call::GetScreenContents { .. } => 8,
            Call::GetTerminalSize => 9,
            Call::GetShellType => 10,
            Call::DetectSshSession => 11,
            Call::GetRemoteHostname => 12,
            Call::GetUserIdentity => 13,
            Call::GetCurrentDirectory => 14,
            Call::SetClipboard { .. } => 15,
            Call::InsertTextAtCursor { .. } => 16,
            Call::DeleteCurrentLine => 17,
            Call::GetManPage { .. } => 18,
            Call::CreateFile { .. } => 19,
            Call::FindOnPage { .. } => 20,
            Call::LoadUrl { .. } => 21,
            Call::WebSearch { .. } => 22,
            Call::GetUrl => 23,
            Call::ReadWebPage { .. } => 24,
            Call::RestartSession => 25,
        }
    }

    /// The function name the model calls (`functionName`).
    pub fn name(&self) -> &'static str {
        TOOLS[self.index()].name
    }

    /// The permission that governs the call (`permissionCategory`, `RC:124-145`).
    pub fn category(&self) -> Category {
        TOOLS[self.index()].category
    }

    /// Only `execute_command` goes through the safety check (`needsSafetyCheck`); iTerm2 leaves
    /// `create_file` out on purpose, the person seeing it in the chat first.
    pub fn needs_safety_check(&self) -> bool {
        matches!(self, Call::ExecuteCommand { .. })
    }

    /// The text of a parked request (`permissionDescription`). Only the typed text is cut at 32;
    /// a command, man page name or file name is shown whole. Every model-written field goes
    /// through `consent_field` (ruling, a deviation from iTerm2, which escapes only the
    /// backticks of three fields): the model cannot end the code span, start a new line, or add
    /// markdown of its own to the text the person answers.
    pub fn consent_description(&self) -> String {
        let prefix = "The AI Agent would like to";
        match self {
            Call::IsAtPrompt => format!("{prefix} check if you\u{2019}re at a shell prompt"),
            Call::ExecuteCommand { command } => format!("{prefix} execute {}", consent_field(command, Field::Code)),
            Call::GetLastExitStatus => format!("{prefix} check the exit status of the last command"),
            Call::GetCommandHistory { .. } => format!("{prefix} review the history of commands you have run in this session"),
            Call::GetLastCommand => format!("{prefix} view the last command you ran in this session"),
            Call::GetCommandBeforeCursor => format!("{prefix} read your current command prompt"),
            Call::SearchCommandHistory { .. } => format!("{prefix} search the history of commands you have run in this session"),
            Call::GetCommandOutput { .. } => format!("{prefix} fetch the output of a previously run command"),
            Call::GetScreenContents { .. } => format!("{prefix} read the visible screen of your terminal session"),
            Call::GetTerminalSize => format!("{prefix} query the size of your terminal window"),
            Call::GetShellType => format!("{prefix} determine which shell you use"),
            Call::DetectSshSession => format!("{prefix} check if you are using SSH"),
            Call::GetRemoteHostname => format!("{prefix} get the current host name of this terminal session"),
            Call::GetUserIdentity => format!("{prefix} check your username"),
            Call::GetCurrentDirectory => format!("{prefix} know your current directory"),
            Call::SetClipboard { .. } => format!("{prefix} paste to the clipboard"),
            Call::InsertTextAtCursor { text } => format!("{prefix} type {} into the current session", consent_field(&truncate32(&shown(text)), Field::Code)),
            Call::DeleteCurrentLine => format!("{prefix} erase the current command line"),
            Call::GetManPage { cmd } => format!("{prefix} check the manpage for {}", consent_field(cmd, Field::Code)),
            Call::CreateFile { filename, .. } => format!("{prefix} create a file named {}", consent_field(filename, Field::Code)),
            // The browser texts are iTerm2's as written, lower-case "agent" and "write to" included.
            Call::FindOnPage { query } => format!("The AI agent would like to search the current web page for \u{201c}{}\u{201d}", consent_field(query, Field::Text)),
            Call::LoadUrl { url } => format!("The AI agent would like to navigate to {}", consent_field(url, Field::Text)),
            Call::WebSearch { query } => format!("The AI agent would like to write to search the web for \u{201c}{}\u{201d}", consent_field(query, Field::Text)),
            Call::GetUrl => "The AI agent would like to write to get the current URL".to_string(),
            Call::ReadWebPage { .. } => "The AI agent would like to write to view the current web page".to_string(),
            Call::RestartSession => format!("{prefix} restart this session, which kills any running jobs"),
        }
    }

    /// The notice published while the call runs (`performRemoteCommand`): the status line and an
    /// ellipsis, for every tool but `execute_command` (`shouldPublishNotice`).
    pub fn status_notice(&self) -> Option<String> {
        match self {
            Call::ExecuteCommand { .. } => None,
            _ => Some(format!("{}\u{2026}", self.markdown_description())),
        }
    }
}

/// An argument's JSON type (`JSONSchema.extractFieldType`: Swift `Int` is "number").
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
}

/// One argument: its Swift label (the JSON key), type and description (`argDescriptions`).
struct Arg {
    label: &'static str,
    kind: Kind,
    description: &'static str,
}

/// One tool as declared to the model (`RemoteCommandToolProvider.define`).
struct Tool {
    name: &'static str,
    category: Category,
    description: &'static str,
    args: &'static [Arg],
}

const fn text(label: &'static str, description: &'static str) -> Arg {
    Arg { label, kind: Kind::Text, description }
}

const fn number(label: &'static str, description: &'static str) -> Arg {
    Arg { label, kind: Kind::Number, description }
}

/// Every tool in `allCases` order (`RC:141-168`), with `functionName`, `permissionCategory`,
/// `functionDescription` and `argDescriptions` (`RemoteCommand.swift`), the arguments in their
/// structs' declaration order. The texts are iTerm2's with "iTerm2" as "Tandem" and "Finder" as
/// "Explorer".
const TOOLS: [Tool; 26] = {
    use Category::*;
    [
        Tool { name: "is_at_prompt", category: CheckTerminalState, description: "Returns true if the terminal is at the command prompt, allowing safe command injection.", args: &[] },
        Tool {
            name: "execute_command",
            category: RunCommands,
            description: concat!(
                "Runs a shell command and returns its output once the command finishes. ",
                "It blocks until the command exits, so an interactive or full-screen (TUI) ",
                "program (for example vim, less, top, an ssh session, a REPL, or claude) does ",
                "not return until it exits or the user cancels it. It is the only tool that ",
                "submits a command line, so start any program with it, an interactive one ",
                "included, and then drive the running program with insert_text_at_cursor."
            ),
            args: &[text("command", "The command to run")],
        },
        Tool { name: "get_last_exit_status", category: CheckTerminalState, description: "Retrieves the exit status of the last executed command.", args: &[] },
        Tool { name: "get_command_history", category: ViewContents, description: "Returns the recent command history.", args: &[number("limit", "Maximum number of history items to return.")] },
        Tool { name: "get_last_command", category: ViewContents, description: "Retrieves the most recent command.", args: &[] },
        Tool { name: "get_command_before_cursor", category: ViewContents, description: "Returns the current partially typed command before the cursor.", args: &[] },
        Tool { name: "search_command_history", category: ViewContents, description: "Searches history for commands matching a query.", args: &[text("query", "Search query for filtering command history.")] },
        Tool {
            name: "get_command_output",
            category: ViewContents,
            description: "Returns the output of a previous command by its unique identifier.",
            args: &[text("id", "Unique identifier of the command whose output is requested.")],
        },
        Tool {
            name: "get_screen_contents",
            category: ViewContents,
            description: "Returns the visible contents of this terminal session. For a normal shell the text is linear scrollback (real history; ask for more `lines` to see further back). For a full-screen application (vim, less, htop, a REPL, etc.) the text is only the current rendered screen: there is no scrollback or history beyond what is displayed. The result reports a `kind` field and an `is_snapshot` flag (true means do not assume any history is present) alongside the raw `text`. The text uses a few markup tokens (the angle brackets are U+27E8/U+27E9 and effectively never occur in real terminal output): \u{27E8}dim\u{27E9}\u{2026}\u{27E8}/dim\u{27E9} wraps faint/dimmed text (how shells and TUIs render inline suggestions and ghost completions); \u{27E8}cursor\u{27E9} marks the text cursor's position; \u{27E8}image\u{27E9} stands in for an inline image. These tokens are inserted by Tandem and are not literally present on the screen.",
            args: &[number(
                "lines",
                "Number of trailing lines to return when the visible screen is a normal shell (the primary screen). Use 0 for the default of 100. Ignored for a full-screen application, where only the current screen is available.",
            )],
        },
        Tool { name: "get_terminal_size", category: CheckTerminalState, description: "Returns (columns, rows) of the terminal window.", args: &[] },
        Tool { name: "get_shell_type", category: CheckTerminalState, description: "Detects the shell in use (e.g., bash, fish, xonsh, zsh).", args: &[] },
        Tool { name: "detect_ssh_session", category: CheckTerminalState, description: "Returns true if the user is SSH\u{2019}ed into a remote host.", args: &[] },
        Tool { name: "get_remote_hostname", category: CheckTerminalState, description: "Returns the remote hostname if in an SSH session.", args: &[] },
        Tool { name: "get_user_identity", category: CheckTerminalState, description: "Returns the logged-in user\u{2019}s username.", args: &[] },
        Tool { name: "get_current_directory", category: CheckTerminalState, description: "Returns the current directory.", args: &[] },
        Tool { name: "set_clipboard", category: WriteToClipboard, description: "Copies text to the clipboard.", args: &[text("text", "The text to copy to the clipboard.")] },
        Tool {
            name: "insert_text_at_cursor",
            category: ControlTerminal,
            description: concat!(
                "Inserts text into the terminal input at the cursor position, as if typed, ",
                "without submitting it: the text may not contain a line break or a control key ",
                "other than Tab (a refused text types nothing). To run a command, use ",
                "execute_command. Use this to interact with a running interactive or ",
                "full-screen (TUI) program - choose a menu option, or send keystrokes to a ",
                "running app - since unlike execute_command it does not wait for the program ",
                "to finish."
            ),
            args: &[text(
                "text",
                "The text to insert at the cursor position. Supports a small backslash-escape vocabulary so you can send special characters: \\\\ for a literal backslash, \\t for tab, and \\uXXXX (four hex digits, JSON-style) for any Unicode scalar. Only Tab (\\t) is typed among the control keys: a line break or any other control character (\\n, \\r, Ctrl+C, Escape, Ctrl-D, Ctrl-Z, arrow keys and the like) is refused and nothing is typed, because this tool never submits a line. Consider whether execute_command would be a better choice, especially when running a command at the shell prompt since insert_text_at_cursor does not return the output to you.",
            )],
        },
        Tool { name: "delete_current_line", category: ControlTerminal, description: "Clears the current command line input (only at the prompt).", args: &[] },
        Tool { name: "get_man_page", category: ViewManpages, description: "Returns the content of a command's man page.", args: &[text("cmd", "The command whose man page content is requested.")] },
        Tool {
            name: "create_file",
            category: WriteToFilesystem,
            description: "Creates a file containing a specified string on the user's computer and then reveals it in Explorer.",
            args: &[
                text("filename", "The name of the file you wish to create. It will be replaced if it already exists."),
                text("content", "The content that will be written to the file."),
            ],
        },
        Tool {
            name: "find_on_page",
            category: ActInWebBrowser,
            description: "Searches the current web page in the associated web browser (after converting to markdown format) for a substring.",
            args: &[text("query", "The text to search for on the current page. Ensure you know which web page is currently loaded before using this.")],
        },
        Tool { name: "load_url", category: ActInWebBrowser, description: "Loads the specified URL in the associated web browser", args: &[text("url", "The URL to load. Must use https scheme.")] },
        Tool {
            name: "web_search_in_browser",
            category: ActInWebBrowser,
            description: "Performs a web search using the currently configured search engine in the associated web browser",
            args: &[text("query", "The web search query")],
        },
        Tool { name: "get_current_url", category: ActInWebBrowser, description: "Returns the current URL of the associated web browser", args: &[] },
        Tool {
            name: "read_web_page_section",
            category: ActInWebBrowser,
            description: "Returns some of the content (in markdown format) of the page visible in the associated web browser.",
            args: &[number("startingLineNumber", "The line number to start reading at."), number("numberOfLines", "The number of lines to return.")],
        },
        Tool {
            name: "restart_session",
            category: ControlTerminal,
            description: "Restarts this terminal session: terminates any running jobs and relaunches the session\u{2019}s command, equivalent to the Session > Restart Session menu item. Use this to recover a hung or misconfigured session. This is disruptive - every running process in the session is killed - so only use it when the user has asked to restart or when the session is unusable.",
            args: &[],
        },
    ]
};

/// Every function name in `allCases` order.
pub const ALL_NAMES: [&str; 26] = {
    let mut names = [""; 26];
    let mut i = 0;
    while i < TOOLS.len() {
        names[i] = TOOLS[i].name;
        i += 1;
    }
    names
};

fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The tool's `functionDescription`.
pub fn description(name: &str) -> Option<&'static str> {
    tool(name).map(|t| t.description)
}

/// The tool's argument descriptions, in declaration order; empty for no arguments or no tool.
pub fn arg_descriptions(name: &str) -> Vec<(&'static str, &'static str)> {
    tool(name).map(|t| t.args.iter().map(|a| (a.label, a.description)).collect()).unwrap_or_default()
}

/// `JSONSchema(for:descriptions:)` (`AITerm/JSONSchema.swift`): an object whose properties are
/// every argument (`{"type", "description"}`), each one required (strict mode), keys in the
/// order Swift's encoder writes them.
pub fn schema(name: &str) -> Option<Value> {
    let tool = tool(name)?;
    let mut properties = Map::new();
    for arg in tool.args {
        let kind = match arg.kind {
            Kind::Text => "string",
            Kind::Number => "number",
        };
        let mut property = Map::new();
        property.insert("type".into(), kind.into());
        property.insert("description".into(), arg.description.into());
        properties.insert(arg.label.into(), Value::Object(property));
    }
    let mut out = Map::new();
    out.insert("type".into(), "object".into());
    out.insert("properties".into(), Value::Object(properties));
    out.insert("required".into(), tool.args.iter().map(|a| Value::from(a.label)).collect());
    Some(Value::Object(out))
}

/// The model's call to a `Call` (`LLM.Function.invoke`: Swift's `JSONDecoder` over the argument
/// struct). Every key must be present with its type, as the synthesized `Decodable` requires
/// (a struct's default values are not used); a number must be whole; unknown keys are ignored.
/// Null arguments read as `{}` (iTerm2 turns Anthropic's empty argument text into `{}`).
pub fn parse(name: &str, args: &Value) -> Result<Call, String> {
    let Some(tool) = tool(name) else {
        return Err(crate::ai_turn::UNKNOWN_FUNCTION.to_string());
    };
    let invalid = |why: String| format!("Invalid arguments for {name}: {why}");
    let empty = Map::new();
    let object = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(invalid("the arguments must be an object".into())),
    };
    let mut values = Vec::new();
    for arg in tool.args {
        let Some(v) = object.get(arg.label) else {
            return Err(invalid(format!("{} is required", arg.label)));
        };
        let checked = match arg.kind {
            Kind::Text => v.as_str().map(Value::from).ok_or_else(|| format!("{} must be a string", arg.label)),
            Kind::Number if !v.is_number() => Err(format!("{} must be a number", arg.label)),
            Kind::Number => whole(v).map(Value::from).ok_or_else(|| format!("{} must be a whole number", arg.label)),
        };
        values.push(checked.map_err(invalid)?);
    }
    let s = |i: usize| values[i].as_str().unwrap_or_default().to_string();
    let n = |i: usize| values[i].as_i64().unwrap_or_default();
    Ok(match name {
        "is_at_prompt" => Call::IsAtPrompt,
        "execute_command" => Call::ExecuteCommand { command: s(0) },
        "get_last_exit_status" => Call::GetLastExitStatus,
        "get_command_history" => Call::GetCommandHistory { limit: n(0) },
        "get_last_command" => Call::GetLastCommand,
        "get_command_before_cursor" => Call::GetCommandBeforeCursor,
        "search_command_history" => Call::SearchCommandHistory { query: s(0) },
        "get_command_output" => Call::GetCommandOutput { id: s(0) },
        "get_screen_contents" => Call::GetScreenContents { lines: n(0) },
        "get_terminal_size" => Call::GetTerminalSize,
        "get_shell_type" => Call::GetShellType,
        "detect_ssh_session" => Call::DetectSshSession,
        "get_remote_hostname" => Call::GetRemoteHostname,
        "get_user_identity" => Call::GetUserIdentity,
        "get_current_directory" => Call::GetCurrentDirectory,
        "set_clipboard" => Call::SetClipboard { text: s(0) },
        "insert_text_at_cursor" => Call::InsertTextAtCursor { text: s(0) },
        "delete_current_line" => Call::DeleteCurrentLine,
        "get_man_page" => Call::GetManPage { cmd: s(0) },
        "create_file" => Call::CreateFile { filename: s(0), content: s(1) },
        "find_on_page" => Call::FindOnPage { query: s(0) },
        "load_url" => Call::LoadUrl { url: s(0) },
        "web_search_in_browser" => Call::WebSearch { query: s(0) },
        "get_current_url" => Call::GetUrl,
        "read_web_page_section" => Call::ReadWebPage { starting_line_number: n(0), number_of_lines: n(1) },
        "restart_session" => Call::RestartSession,
        // A tool added to `TOOLS` without an arm here is unknown, never another tool.
        _ => return Err(crate::ai_turn::UNKNOWN_FUNCTION.to_string()),
    })
}

/// A JSON number as Swift's `Int` reads it: whole and in range (5.0 is 5, 5.5 fails).
fn whole(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64))
}

/// The declarations for the model: every tool whose category is allowed, in `allCases` order
/// (`RemoteCommandToolProvider.registerTools`). A browser tool is declared whenever Act in Web
/// Browser is allowed, which the allowed set grants only with a linked browser session.
pub fn tool_defs(allowed: &BTreeSet<Category>) -> Vec<ToolDecl> {
    TOOLS
        .iter()
        .filter(|t| allowed.contains(&t.category))
        .map(|t| ToolDecl { name: t.name.to_string(), description: t.description.to_string(), parameters: schema(t.name).unwrap_or_default() })
        .collect()
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

/// How a model-written field sits in a parked request's markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// A code span (a command, typed text, a man page or file name).
    Code,
    /// Running text (a query, a URL).
    Text,
}

/// The one escaping rule for model-written text in a permission request (ruling, a deviation
/// from iTerm2's backtick escape): line breaks and other control characters become visible
/// escapes (`shown`), never a new line; a code span is fenced with one more backtick than the
/// longest run inside it (a backslash does not escape in a code span), padded with a space when
/// the text starts or ends with a backtick or a space; running text has every ASCII
/// punctuation character backslash-escaped, so it can open no markdown of its own.
pub fn consent_field(s: &str, field: Field) -> String {
    let s = shown(s);
    match field {
        Field::Code => {
            let (mut longest, mut run) = (0, 0);
            for c in s.chars() {
                run = if c == '`' { run + 1 } else { 0 };
                longest = longest.max(run);
            }
            let fence = "`".repeat(longest + 1);
            let pad = if s.starts_with(['`', ' ']) || s.ends_with(['`', ' ']) { " " } else { "" };
            format!("{fence}{pad}{s}{pad}{fence}")
        }
        Field::Text => {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                if c.is_ascii_punctuation() {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        }
    }
}

/// Control characters as visible escapes: CR `\r`, LF `\n`, tab `\t`, any other C0 or C1
/// control, DEL and the line and paragraph separators as `\uXXXX`.
fn shown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
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

    use serde_json::json;

    /// One call of each tool, in `allCases` order, with its arguments' default values.
    fn prototypes() -> Vec<Call> {
        let s = String::new;
        vec![
            Call::IsAtPrompt,
            Call::ExecuteCommand { command: s() },
            Call::GetLastExitStatus,
            Call::GetCommandHistory { limit: 100 },
            Call::GetLastCommand,
            Call::GetCommandBeforeCursor,
            Call::SearchCommandHistory { query: s() },
            Call::GetCommandOutput { id: s() },
            Call::GetScreenContents { lines: 0 },
            Call::GetTerminalSize,
            Call::GetShellType,
            Call::DetectSshSession,
            Call::GetRemoteHostname,
            Call::GetUserIdentity,
            Call::GetCurrentDirectory,
            Call::SetClipboard { text: s() },
            Call::InsertTextAtCursor { text: s() },
            Call::DeleteCurrentLine,
            Call::GetManPage { cmd: s() },
            Call::CreateFile { filename: s(), content: s() },
            Call::FindOnPage { query: s() },
            Call::LoadUrl { url: s() },
            Call::WebSearch { query: s() },
            Call::GetUrl,
            Call::ReadWebPage { starting_line_number: 0, number_of_lines: 0 },
            Call::RestartSession,
        ]
    }

    const BROWSER: [&str; 5] = ["find_on_page", "load_url", "web_search_in_browser", "get_current_url", "read_web_page_section"];

    #[test]
    fn names_follow_iterm2() {
        assert_eq!(
            ALL_NAMES,
            [
                "is_at_prompt", "execute_command", "get_last_exit_status", "get_command_history", "get_last_command", "get_command_before_cursor",
                "search_command_history", "get_command_output", "get_screen_contents", "get_terminal_size", "get_shell_type", "detect_ssh_session",
                "get_remote_hostname", "get_user_identity", "get_current_directory", "set_clipboard", "insert_text_at_cursor", "delete_current_line",
                "get_man_page", "create_file", "find_on_page", "load_url", "web_search_in_browser", "get_current_url", "read_web_page_section",
                "restart_session",
            ]
        );
        use Category::*;
        let categories = [
            CheckTerminalState, RunCommands, CheckTerminalState, ViewContents, ViewContents, ViewContents, ViewContents, ViewContents, ViewContents,
            CheckTerminalState, CheckTerminalState, CheckTerminalState, CheckTerminalState, CheckTerminalState, CheckTerminalState, WriteToClipboard,
            ControlTerminal, ControlTerminal, ViewManpages, WriteToFilesystem, ActInWebBrowser, ActInWebBrowser, ActInWebBrowser, ActInWebBrowser,
            ActInWebBrowser, ControlTerminal,
        ];
        let calls = prototypes();
        assert_eq!(calls.len(), 26);
        for (i, call) in calls.iter().enumerate() {
            assert_eq!(call.name(), ALL_NAMES[i]);
            assert_eq!(call.category(), categories[i], "{}", ALL_NAMES[i]);
            assert_eq!(call.needs_safety_check(), matches!(call, Call::ExecuteCommand { .. }), "{}", ALL_NAMES[i]);
        }
    }

    #[test]
    fn descriptions_follow_iterm2() {
        assert_eq!(description("is_at_prompt"), Some("Returns true if the terminal is at the command prompt, allowing safe command injection."));
        assert_eq!(
            description("execute_command"),
            Some(
                "Runs a shell command and returns its output once the command finishes. It blocks until the command exits, so an interactive or full-screen (TUI) program (for example vim, less, top, an ssh session, a REPL, or claude) does not return until it exits or the user cancels it. It is the only tool that submits a command line, so start any program with it, an interactive one included, and then drive the running program with insert_text_at_cursor."
            )
        );
        // Ruling R12cd-2's wording: the insert tool never submits a line (a deviation from iTerm2).
        assert_eq!(
            description("insert_text_at_cursor"),
            Some(
                "Inserts text into the terminal input at the cursor position, as if typed, without submitting it: the text may not contain a line break or a control key other than Tab (a refused text types nothing). To run a command, use execute_command. Use this to interact with a running interactive or full-screen (TUI) program - choose a menu option, or send keystrokes to a running app - since unlike execute_command it does not wait for the program to finish."
            )
        );
        assert_eq!(description("create_file"), Some("Creates a file containing a specified string on the user's computer and then reveals it in Explorer."));
        let screen = description("get_screen_contents").unwrap();
        assert!(screen.ends_with("These tokens are inserted by Tandem and are not literally present on the screen."));
        assert!(screen.contains("\u{27E8}dim\u{27E9}\u{2026}\u{27E8}/dim\u{27E9}"));
        assert!(description("restart_session").unwrap().starts_with("Restarts this terminal session: terminates any running jobs"));
        assert_eq!(description("detect_ssh_session"), Some("Returns true if the user is SSH\u{2019}ed into a remote host."));
        for name in ALL_NAMES {
            assert!(description(name).is_some_and(|d| !d.contains("iTerm2") && !d.contains("Finder")), "{name}");
        }
        assert_eq!(description("nope"), None);
        assert_eq!(
            arg_descriptions("get_screen_contents"),
            [("lines", "Number of trailing lines to return when the visible screen is a normal shell (the primary screen). Use 0 for the default of 100. Ignored for a full-screen application, where only the current screen is available.")]
        );
        assert_eq!(
            arg_descriptions("create_file"),
            [("filename", "The name of the file you wish to create. It will be replaced if it already exists."), ("content", "The content that will be written to the file.")]
        );
        assert_eq!(arg_descriptions("read_web_page_section"), [("startingLineNumber", "The line number to start reading at."), ("numberOfLines", "The number of lines to return.")]);
        assert_eq!(
            arg_descriptions("insert_text_at_cursor"),
            [("text", "The text to insert at the cursor position. Supports a small backslash-escape vocabulary so you can send special characters: \\\\ for a literal backslash, \\t for tab, and \\uXXXX (four hex digits, JSON-style) for any Unicode scalar. Only Tab (\\t) is typed among the control keys: a line break or any other control character (\\n, \\r, Ctrl+C, Escape, Ctrl-D, Ctrl-Z, arrow keys and the like) is refused and nothing is typed, because this tool never submits a line. Consider whether execute_command would be a better choice, especially when running a command at the shell prompt since insert_text_at_cursor does not return the output to you.")]
        );
        for name in ["execute_command", "insert_text_at_cursor"] {
            let d = description(name).unwrap();
            assert!(!d.contains("newline to submit") && !d.contains("plus a newline") && !d.contains("insert_text_at_cursor instead"), "{name}");
        }
        assert!(arg_descriptions("is_at_prompt").is_empty());
        assert!(arg_descriptions("nope").is_empty());
    }

    #[test]
    fn schemas_follow_iterm2() {
        let history = schema("get_command_history").unwrap();
        assert_eq!(history, json!({"type":"object","properties":{"limit":{"type":"number","description":"Maximum number of history items to return."}},"required":["limit"]}));
        assert_eq!(serde_json::to_string(&history).unwrap(), r#"{"type":"object","properties":{"limit":{"type":"number","description":"Maximum number of history items to return."}},"required":["limit"]}"#);
        assert_eq!(schema("is_at_prompt"), Some(json!({"type":"object","properties":{},"required":[]})));
        let file = schema("create_file").unwrap();
        assert_eq!(file["properties"]["filename"]["type"], "string");
        assert_eq!(file["properties"]["content"]["type"], "string");
        assert_eq!(file["required"], json!(["filename", "content"]));
        let keys: Vec<&String> = file["properties"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["filename", "content"]);
        let page = schema("read_web_page_section").unwrap();
        assert_eq!(page["properties"]["startingLineNumber"]["type"], "number");
        assert_eq!(page["properties"]["numberOfLines"]["type"], "number");
        assert_eq!(page["required"], json!(["startingLineNumber", "numberOfLines"]));
        assert_eq!(schema("nope"), None);
        for name in ALL_NAMES {
            let s = schema(name).unwrap();
            let args = arg_descriptions(name);
            assert_eq!(s["properties"].as_object().unwrap().len(), args.len(), "{name}");
            for (label, text) in args {
                assert_eq!(s["properties"][label]["description"], text, "{name}");
            }
        }
    }

    #[test]
    fn registry_follows_the_allowed_set() {
        let names = |set: &BTreeSet<Category>| tool_defs(set).into_iter().map(|d| d.name).collect::<Vec<_>>();
        let run = tool_defs(&BTreeSet::from([Category::RunCommands]));
        assert_eq!(run.len(), 1);
        assert_eq!(run[0].name, "execute_command");
        assert_eq!(run[0].description, description("execute_command").unwrap());
        assert_eq!(run[0].parameters, schema("execute_command").unwrap());
        let terminal: BTreeSet<Category> = Category::ALL.into_iter().filter(|c| *c != Category::ActInWebBrowser).collect();
        let expected: Vec<&str> = ALL_NAMES.into_iter().filter(|n| !BROWSER.contains(n)).collect();
        assert_eq!(expected.len(), 21);
        assert_eq!(names(&terminal), expected);
        assert_eq!(names(&BTreeSet::from([Category::ActInWebBrowser])), BROWSER);
        assert_eq!(names(&BTreeSet::from(Category::ALL)), ALL_NAMES);
        assert!(tool_defs(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn parse_follows_iterm2() {
        assert_eq!(parse("execute_command", &json!({"command":"ls"})), Ok(Call::ExecuteCommand { command: "ls".into() }));
        assert_eq!(parse("get_command_history", &json!({"limit": 5.0})), Ok(Call::GetCommandHistory { limit: 5 }));
        assert_eq!(parse("get_command_history", &json!({"limit": 7})), Ok(Call::GetCommandHistory { limit: 7 }));
        assert_eq!(parse("get_command_history", &json!({"limit": 5.5})), Err("Invalid arguments for get_command_history: limit must be a whole number".to_string()));
        // Swift's synthesized Decodable needs every key, default values notwithstanding.
        assert_eq!(parse("get_command_history", &json!({})), Err("Invalid arguments for get_command_history: limit is required".to_string()));
        assert_eq!(parse("get_screen_contents", &json!({"lines": null})), Err("Invalid arguments for get_screen_contents: lines must be a number".to_string()));
        assert_eq!(parse("get_screen_contents", &json!({"lines": 30})), Ok(Call::GetScreenContents { lines: 30 }));
        assert_eq!(parse("execute_command", &json!({"command": 5})), Err("Invalid arguments for execute_command: command must be a string".to_string()));
        assert_eq!(parse("execute_command", &json!(["ls"])), Err("Invalid arguments for execute_command: the arguments must be an object".to_string()));
        assert_eq!(parse("nope", &json!({})), Err("There is no registered function by that name. Try again.".to_string()));
        assert_eq!(parse("is_at_prompt", &json!({"extra": 1})), Ok(Call::IsAtPrompt));
        assert_eq!(parse("is_at_prompt", &Value::Null), Ok(Call::IsAtPrompt));
        assert_eq!(
            parse("create_file", &json!({"filename": "a.txt", "content": "hi"})),
            Ok(Call::CreateFile { filename: "a.txt".into(), content: "hi".into() })
        );
        assert_eq!(
            parse("read_web_page_section", &json!({"startingLineNumber": 3, "numberOfLines": 4})),
            Ok(Call::ReadWebPage { starting_line_number: 3, number_of_lines: 4 })
        );
        for (i, name) in ALL_NAMES.into_iter().enumerate() {
            let args: Map<String, Value> = schema(name).unwrap()["properties"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, p)| (k.clone(), if p["type"] == "number" { json!(if k == "limit" { 100 } else { 0 }) } else { json!("") }))
                .collect();
            assert_eq!(parse(name, &Value::Object(args)), Ok(prototypes()[i].clone()), "{name}");
        }
    }

    #[test]
    fn consent_follows_iterm2() {
        assert_eq!(Call::ExecuteCommand { command: "ls".into() }.consent_description(), "The AI Agent would like to execute `ls`");
        // `permissionDescription` escapes the command but never truncates it (`RemoteCommand.swift`).
        let long = "x".repeat(40);
        assert_eq!(Call::ExecuteCommand { command: format!("{long}`") }.consent_description(), format!("The AI Agent would like to execute `` {long}` ``"));
        assert_eq!(Call::InsertTextAtCursor { text: long.clone() }.consent_description(), format!("The AI Agent would like to type `{}\u{2026}` into the current session", "x".repeat(31)));
        assert_eq!(Call::RestartSession.consent_description(), "The AI Agent would like to restart this session, which kills any running jobs");
        assert_eq!(Call::IsAtPrompt.consent_description(), "The AI Agent would like to check if you\u{2019}re at a shell prompt");
        assert_eq!(Call::GetManPage { cmd: "a`b".into() }.consent_description(), "The AI Agent would like to check the manpage for ``a`b``");
        assert_eq!(Call::CreateFile { filename: "f`.txt".into(), content: String::new() }.consent_description(), "The AI Agent would like to create a file named ``f`.txt``");
        assert_eq!(Call::WebSearch { query: "q".into() }.consent_description(), "The AI agent would like to write to search the web for \u{201c}q\u{201d}");
        assert_eq!(Call::GetUrl.consent_description(), "The AI agent would like to write to get the current URL");
        assert_eq!(Call::GetCurrentDirectory.consent_description(), "The AI Agent would like to know your current directory");
        assert_eq!(Call::ExecuteCommand { command: "ls".into() }.status_notice(), None);
        assert_eq!(Call::GetScreenContents { lines: 0 }.status_notice(), Some("Reading the visible screen\u{2026}".to_string()));
        assert_eq!(Call::CreateFile { filename: "f".into(), content: String::new() }.status_notice(), Some("Creating f\u{2026}".to_string()));
        for call in prototypes() {
            assert!(call.consent_description().starts_with("The AI "), "{}", call.name());
            assert_eq!(call.status_notice().is_none(), matches!(call, Call::ExecuteCommand { .. }));
        }
    }

    #[test]
    fn status_lines_stay_on_one_line() {
        for call in [Call::CreateFile { filename: "a\nb".into(), content: String::new() }, Call::FindOnPage { query: "a\nb".into() }, Call::LoadUrl { url: "a\r\nb".into() }, Call::WebSearch { query: "a\nb".into() }] {
            let line = call.markdown_description();
            assert!(!line.contains(['\n', '\r']), "{}", call.name());
            assert!(line.contains("a\\n") || line.contains("a\\r"), "{line}");
        }
    }

    #[test]
    fn consent_fields_are_escaped() {
        // A line break never starts a new line, and the model's markdown stays inside the span.
        let file = Call::CreateFile { filename: "a\n\n**Allow?**".into(), content: String::new() }.consent_description();
        assert_eq!(file, "The AI Agent would like to create a file named `a\\n\\n**Allow?**`");
        assert!(!file.contains('\n'));
        // A fence longer than any backtick run inside; padded where the text ends with one.
        let cmd = Call::ExecuteCommand { command: "echo ```x```".into() }.consent_description();
        assert_eq!(cmd, "The AI Agent would like to execute ```` echo ```x``` ````");
        assert_eq!(Call::ExecuteCommand { command: "ls\r\n\u{1b}[2J\u{2028}".into() }.consent_description(), "The AI Agent would like to execute `ls\\r\\n\\u001B[2J\\u2028`");
        assert_eq!(Call::ExecuteCommand { command: " x".into() }.consent_description(), "The AI Agent would like to execute `  x `");
        // The typed text is cut after its escapes are shown, so a cut never splits the fence.
        let typed = Call::InsertTextAtCursor { text: format!("{}\n", "`".repeat(40)) }.consent_description();
        assert_eq!(typed, format!("The AI Agent would like to type {f} {t}\u{2026} {f} into the current session", f = "`".repeat(32), t = "`".repeat(31)));
        // Running text: every ASCII punctuation mark is escaped, so no markdown opens.
        assert_eq!(
            Call::WebSearch { query: "**x**\n\n[a](b)".into() }.consent_description(),
            "The AI agent would like to write to search the web for \u{201c}\\*\\*x\\*\\*\\\\n\\\\n\\[a\\]\\(b\\)\u{201d}"
        );
        assert_eq!(Call::LoadUrl { url: "https://a.b/c".into() }.consent_description(), "The AI agent would like to navigate to https\\:\\/\\/a\\.b\\/c");
        assert_eq!(consent_field("", Field::Code), "``");
        for call in prototypes() {
            assert!(!call.consent_description().contains('\n'), "{}", call.name());
        }
    }
}
