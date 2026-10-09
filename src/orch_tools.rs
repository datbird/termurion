//! Orchestration's tools, the pure part (12f Task 10): iTerm2's declarations
//! (`OrchestratorToolDefinitions.swift`), the argument decoding, the errors and the result
//! envelope (`OrchestratorCommand.swift`, `OrchestratorError.swift`), the claims, the
//! `session_*` mirrors of 12c's tools and every text the dispatcher answers with
//! (`OrchestratorDispatcher.swift`). The app side is `app/orch_dispatch.rs`.

use serde_json::{json, Map, Value};

use crate::ai_permissions::Category;
use crate::ai_turn::ToolDecl;

/// The 14 orchestration tools in iTerm2's order (`TD:119-259`), without notify and
/// request_notification_permission (no companion phone). iTerm2's header says 12; it has 16.
pub const TOOLS: [&str; 14] = crate::orch::TOOL_NAMES;

const SESSION_GUID: &str = "The session_guid of the target session, copied verbatim from a role in the <workgroups> snapshot (its session_guid field).";

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn nullable(kind: &str, description: &str) -> Value {
    json!({"type": [kind, "null"], "description": description})
}

/// A nullable string with an enum; null is one of the values (`nullableString`).
fn nullable_enum(description: &str, values: &[&str]) -> Value {
    let mut e: Vec<Value> = values.iter().map(|v| Value::from(*v)).collect();
    e.push(Value::Null);
    json!({"type": ["string", "null"], "description": description, "enum": e})
}

/// `object(_:required:)`: every property required, nothing else allowed.
fn object(properties: &[(&str, Value)]) -> Value {
    let mut props = Map::new();
    for (k, v) in properties {
        props.insert(k.to_string(), v.clone());
    }
    let required: Vec<Value> = properties.iter().map(|(k, _)| Value::from(*k)).collect();
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

fn empty() -> Value {
    object(&[])
}

fn decl_of(name: &str, description: &str, parameters: Value) -> ToolDecl {
    ToolDecl { name: name.to_string(), description: description.to_string(), parameters }
}

/// Ruling P2: nothing survives a launch, and Stop or leaving the mode cancels everything.
const WATCH_PERSISTENCE: &str = "Watchers do not survive a Tandem restart, and pressing Stop, switching the chat's mode or turning AI features off cancels them (you get a notice after a restart); register again when you need one. Do NOT poll yourself.";

/// `registerWatchDescription` (`TD:100-113`) without the phone clause.
fn watch_description(target: &str, blocking: &str, form: &str, persistence: &str) -> String {
    format!("Register an async watcher on {target}. Supply exactly ONE of target_state or condition. {blocking} When the watch fires, Tandem delivers a `<status_update>...</status_update>` message into the chat as a separate turn; treat that as a system event from Tandem (not a new user request) and act on it directly, messaging the user only when a step advances, a task finishes, or you need a decision (not on every event). {form} Watchers are de-duplicated on (session, target_state, condition): registering the same watch twice returns the existing watcher_id. If the goal is already satisfied at registration time, the watcher fires immediately. {persistence}")
}

fn register_watch_description() -> String {
    let blocking = "This call returns immediately and does NOT block your turn.";
    let form = "Choosing the form: use target_state (idle/working/waiting) when the session's status_source is \u{201C}reported\u{201D} (see list_workgroups / get_state) \u{2014} the watch then fires on the program's own exact status transitions. When status_source is \u{201C}inferred\u{201D}, or when what you're waiting for isn't really an idle/working/waiting transition (e.g. \u{201C}emacs has exited and a shell prompt is showing\u{201D}, \u{201C}the build printed a success or failure line\u{201D}, \u{201C}a password prompt appeared\u{201D}), prefer condition: a plain-English description that an AI judge evaluates by periodically reading the session's screen. Inferred idle/busy is ambiguous (a program parked at a splash screen counts as \u{201C}idle\u{201D}), so a specific condition is far more accurate there. Screen-judged watches (all condition watches, and target_state watches on \u{201C}inferred\u{201D} sessions) keep watching on their own, polling less often the longer they run, and only time out (reason=\u{201C}watchTimedOut\u{201D}) after several hours; re-register then if you still need to wait.";
    watch_description("a session", blocking, form, WATCH_PERSISTENCE)
}

/// One orchestration tool's declaration (`TD:119-295`, "iTerm2" as "Tandem").
pub fn decl(name: &str) -> Option<ToolDecl> {
    let guid = || ("session_guid", string(SESSION_GUID));
    Some(match name {
        "list_workgroups" => decl_of(name, "List all active workgroups and their member sessions. Each session reports its role within the workgroup, current status (idle/working/waiting), status_source, and kind (claude-code/shell/tui/other). status_source says how much to trust `status`: \u{201C}reported\u{201D} means the program announces its own status (accurate, and state watchers fire on exact transitions); \u{201C}inferred\u{201D} means Tandem is guessing from indicators, so `status` is unreliable (e.g. a program waiting at a splash screen looks \u{201C}idle\u{201D}). screen_last_changed says how long ago the session's screen last changed; use it to tell stale output from fresh activity. provenance (on workgroups and sessions) says who created them, e.g. a workgroup entered by a trigger in some session, or a session created by an agent in another chat; a workgroup whose provenance traces to a different chat belongs to that chat's work, so don't drive it unless asked. Standalone sessions appear as single-session workgroups with a synthetic ID prefixed \u{201C}session:\u{201D}.", empty()),
        "get_state" => decl_of(name, "Get rich state for a single session, including the last cc-status detail message for Claude Code sessions. The status_source field says whether `status` is announced by the program itself (\u{201C}reported\u{201D}, trustworthy) or guessed from indicators (\u{201C}inferred\u{201D}, unreliable). screen_last_changed says how long ago the screen last changed, and provenance says who created the session when it wasn't the user.", object(&[guid()])),
        "get_screen_contents" => decl_of(name, "Get the visible contents of a session. For Claude Code sessions, returns a synthesized view of recent hook events (assistant messages, tool calls). For shell sessions, returns scrollback. For TUI sessions, returns a snapshot of the rendered screen with is_snapshot=true; there is no history beyond what's currently displayed. The result includes a kind field so you know how to read the text.\n\nThe result also reports `screen` (\u{201C}primary\u{201D} or \u{201C}alternate\u{201D}) and `mouse_reporting` (bool). On the primary screen the returned text is linear scrollback (real history; ask for more `lines` to see more). On the alternate screen (full-screen apps like vim/less/htop, and Claude Code) ONLY the current grid is returned and is_snapshot is true: older content isn't in scrollback. To view content that scrolled off, if `screen` is \u{201C}alternate\u{201D} and `mouse_reporting` is true, use the scroll_wheel tool (direction=\u{201C}up\u{201D}) and then call get_screen_contents again.\n\nThe text uses a few markup tokens (the angle brackets are U+27E8/U+27E9 and effectively never occur in real terminal output):\n\u{27E8}dim\u{27E9}\u{2026}\u{27E8}/dim\u{27E9} wraps faint/dimmed text. This is how shells and TUIs render inline suggestions and ghost completions (zsh-autosuggestions, fish autosuggest, Claude Code's suggested reply, etc.). \n\u{27E8}cursor\u{27E9} marks the text cursor's position. \n\u{27E8}image\u{27E9} stands in for an inline image. These tokens are inserted by Tandem and are not literally present on the screen.", object(&[guid(), ("lines", nullable("integer", "Number of trailing lines to return for shell sessions. Use null for the default 100. Ignored for tui/claude-code kinds."))])),
        "scroll_wheel" => decl_of(
            name,
            "Send mouse scroll-wheel events to a session to reveal content that's off-screen. This is only useful when get_screen_contents reports screen=\u{201C}alternate\u{201D} (a full-screen app like Claude Code, vim, less, htop): on the alternate screen only the current grid is returned, and older content isn't in scrollback, so scrolling the app's own viewport is the only way to see it. REQUIRES the session to have mouse reporting enabled (get_screen_contents reports mouse_reporting=true); if it's off, this tool errors and does nothing. Workflow: call get_screen_contents, and if you need to see earlier content and screen=\u{201C}alternate\u{201D} with mouse_reporting=true, call scroll_wheel with direction=\u{201C}up\u{201D}, then call get_screen_contents again to read the now-revealed lines. Repeat to page further. Not every alternate-screen app honors the scroll wheel (some ignore it, some scroll a pane you didn't intend); if the screen doesn't change after scrolling, stop and tell the user rather than scrolling blindly. On the primary screen (screen=\u{201C}primary\u{201D}) you don't need this at all: just ask get_screen_contents for more lines.",
            object(&[guid(), ("lines", json!({"type": "integer", "description": "Number of scroll-wheel notches to send. Each notch is one wheel event; how many text lines that moves depends on the app (often 1 or 3). Start small (e.g. 3) and re-read."})), ("direction", nullable_enum("\u{201C}up\u{201D} reveals OLDER content (the usual choice for paging back through history); \u{201C}down\u{201D} reveals NEWER content. Use null for the default \u{201C}up\u{201D}.", &["up", "down"]))]),
        ),
        "list_workgroup_clippings" => decl_of(name, "List the clippings posted to a workgroup. Clippings are structured snippets (type, title, detail) that the user or other agents have posted to the workgroup leader for shared reference. Optional type_filter limits to clippings of a particular type (e.g. \u{201C}Code Review Comment\u{201D}).", object(&[("workgroup_id", json!({"type": "string"})), ("type_filter", nullable("string", "Optional. Only return clippings whose type matches this string exactly. Use null for all types."))])),
        "send_text" => decl_of(
            name,
            "Type text into a session as if the user typed it. By default appends a newline. The text supports a small backslash-escape vocabulary so you can send control keys and special characters: \\\\ for a literal backslash, \\n for newline, \\r for carriage return, \\t for tab, and \\uXXXX (four hex digits, JSON-style) for any Unicode scalar. Use this to drive control keys: \\u0003 for Ctrl-C (prefer the dedicated interrupt tool), \\u0004 for Ctrl-D / EOF, \\u001a for Ctrl-Z, \\u000c for Ctrl-L, \\u001b for Escape, etc. When sending a standalone control key, pass append_newline=false so you don't tack an Enter onto it. You may combine control bytes with text and a trailing newline in a single call (e.g. \\u001b:q\\n to quit vim). Tandem detects embedded control bytes and sends the payload as a raw keystroke stream rather than a bracketed paste, so the control bytes are interpreted by the target program rather than inserted as literal data. The return value `{\"ack\":{}}` only confirms that the bytes were transmitted; it does NOT confirm the program behaved as you expected. For interactive TUIs (vim, emacs, less, htop, fzf, etc.) always follow up with `get_screen_contents` to verify the effect before telling the user it worked. Requires the user to have approved this chat to control the workgroup; the user is prompted inline on the first send to an unclaimed workgroup.",
            object(&[guid(), ("text", json!({"type": "string"})), ("append_newline", nullable("boolean", "Whether to append a newline to the text. Use null for the default true. Set to false when sending control keys (e.g. \\u001b for Escape) by themselves."))]),
        ),
        "interrupt" => decl_of(name, "Send SIGINT to the foreground process of a session, equivalent to the user pressing Ctrl-C. Requires workgroup claim.", object(&[guid()])),
        "add_workgroup_clipping" => decl_of(
            name,
            "Post a clipping to a workgroup so it appears alongside any user-posted clippings. Useful for surfacing structured findings (e.g. distilled code-review comments) for the user or other agents to consume. Requires workgroup claim.",
            object(&[("workgroup_id", json!({"type": "string"})), ("type", string("Short tag describing the clipping kind, e.g. \u{201C}Code Review Comment\u{201D} or \u{201C}Build Error\u{201D}.")), ("title", string("One-line headline.")), ("detail", string("Full description. Markdown is supported."))]),
        ),
        "start_code_review" => decl_of(
            name,
            "Run a Code Review in one call. The session_guid must name the Code Review role's session. The session can be in either of two ready states: (a) the prompt overlay is up (pending_action mentions \u{201C}Code Review prompt overlay\u{201D}), in which case the overlay is populated and the review program launches; or (b) the Code Review session already ran and is idle at its chat prompt, in which case the session is reloaded (killed and relaunched) to run a fresh review. Either way the call auto-registers a watcher for the session reaching idle, so you'll receive a status_update when the review completes. The call errors only if the session isn't the Code Review role, its program is busy, or it can't be reloaded. Pick the prompt by name from the saved prompts (see the system message for the list) via prompt_name, OR pass custom_prompt with free-form text, OR leave both nil to use the user's default prompt. Prefer this over send_text + register_watch for code reviews.",
            object(&[guid(), ("prompt_name", nullable("string", "Optional. Name of a saved prompt from the user's prompt list. Use null when not using a saved prompt.")), ("custom_prompt", nullable("string", "Optional. Free-form prompt text when no saved prompt fits. Use null to use the default prompt."))]),
        ),
        "start_session" => decl_of(
            name,
            "Spawn a new terminal session. Use window=\u{201C}new\u{201D} to open a new window, \u{201C}tab\u{201D} (default) to add a tab to the current window, or \u{201C}current\u{201D} to split the current pane vertically. The user is always prompted to approve the spawn (and the command being run, if any). Returns the session_guid of the new session so you can immediately drive it with the other tools (send_text, get_screen_contents, etc.).",
            object(&[
                ("profile", nullable("string", "Optional profile name. Use null to fall back to the default profile.")),
                ("command", nullable("string", "Optional command to run in the new session. Use null for the profile's default shell.")),
                ("cwd", nullable("string", "Optional working directory. Use null for the default.")),
                ("window", nullable_enum("Where to put the new session. \u{201C}new\u{201D} for a new window, \u{201C}current\u{201D} to split the current pane, \u{201C}tab\u{201D} (default) for a new tab in the current window. Use null for tab.", &["new", "current", "tab"])),
            ]),
        ),
        "register_watch" => decl_of(
            name,
            &register_watch_description(),
            object(&[guid(), ("target_state", nullable_enum("State to watch for. Must be a transition target: \u{201C}idle\u{201D}, \u{201C}working\u{201D}, or \u{201C}waiting\u{201D}. \u{201C}unknown\u{201D} is shown by get_state when a session has no tab status yet, but it is not a watchable transition. Mutually exclusive with condition: set exactly one and null the other.", &["idle", "working", "waiting", "unknown"])), ("condition", nullable("string", "Plain-English condition to watch for, judged by an AI reading the session's screen, e.g. \u{201C}emacs has exited and a shell prompt is showing\u{201D}. Describe what will be VISIBLE on screen when the condition holds. Mutually exclusive with target_state: set exactly one and null the other."))]),
        ),
        "register_timer" => decl_of(
            name,
            "Set an async timer that fires once at a wall-clock time. Supply exactly ONE of delay_seconds (fire this many seconds from now) or fire_at (an absolute ISO-8601 instant, e.g. \u{201C}2026-09-07T17:00:00-07:00\u{201D}). This call returns immediately and does NOT block your turn. When the timer fires, Tandem delivers a `<status_update reason=\u{201C}timerFired\u{201D}>...</status_update>` message into the chat as a separate turn; treat that as a system event from Tandem (not a new user request) and carry out whatever you scheduled. Put what you intend to do in note so you remember it when the timer fires (your own reasoning may be far up-thread by then). Use this instead of trying to sleep or busy-wait yourself, and do NOT poll the clock. Timers do not survive a Tandem restart, and pressing Stop, switching the chat's mode or turning AI features off cancels them. A timer reads nothing (no screen, no terminal state) and targets no session; to act on a session when it fires, use your normal tools in that follow-up turn.",
            object(&[("delay_seconds", nullable("number", "Seconds from now until the timer fires. Must be positive. Mutually exclusive with fire_at: set exactly one and null the other.")), ("fire_at", nullable("string", "Absolute time to fire, ISO 8601 with a timezone offset, e.g. \u{201C}2026-09-07T17:00:00-07:00\u{201D}. Must be in the future. Mutually exclusive with delay_seconds: set exactly one and null the other.")), ("note", nullable("string", "Optional reminder of what to do when the timer fires, echoed back to you verbatim. Use null if you don't need one."))]),
        ),
        "unregister_watch" => decl_of(name, "Cancel a registered watcher by its watcher_id (from register_watch or list_watches). No-op if the watcher has already fired or doesn't exist.", object(&[("watcher_id", json!({"type": "string"}))])),
        "list_watches" => decl_of(name, "List every async watcher currently registered for this chat.", empty()),
        _ => return None,
    })
}

/// What a mirror's description ends with (`OrchestrationToolProvider.swift:316-319`).
pub const MIRROR_SUFFIX: &str = " Targets the session with the supplied session_guid.";
/// A mirror's `session_guid` description (`TP:309-311`).
pub const MIRROR_GUID: &str = "GUID of the session to target. Pull session_guid values from list_workgroups output (each role's session_guid field).";
/// 12c's tools a mirror never offers (`TP:255-283`).
pub const NOT_MIRRORED: [&str; 3] = ["get_screen_contents", "insert_text_at_cursor", "delete_current_line"];

/// A `session_*` mirror of one of 12c's tools: its schema with a required `session_guid`.
pub fn mirror(decl: &ToolDecl) -> ToolDecl {
    let mut parameters = decl.parameters.clone();
    if let Some(o) = parameters.as_object_mut() {
        let props = o.entry("properties").or_insert_with(|| Value::Object(Map::new()));
        if let Some(p) = props.as_object_mut() {
            p.insert("session_guid".into(), json!({"type": "string", "description": MIRROR_GUID}));
        }
        let required = o.entry("required").or_insert_with(|| Value::Array(Vec::new()));
        if let Some(r) = required.as_array_mut() {
            if !r.iter().any(|v| v == "session_guid") {
                r.push("session_guid".into());
            }
        }
    }
    ToolDecl { name: format!("session_{}", decl.name), description: format!("{}{MIRROR_SUFFIX}", decl.description), parameters }
}

/// Every tool an orchestration chat carries: the 14, then a mirror of each of 12c's tools that
/// iTerm2 mirrors (`TP:246-289`), in 12c's order.
pub fn all_decls() -> Vec<ToolDecl> {
    let mut out: Vec<ToolDecl> = TOOLS.iter().filter_map(|n| decl(n)).collect();
    for name in crate::ai_tools::ALL_NAMES {
        if NOT_MIRRORED.contains(&name) {
            continue;
        }
        let d = ToolDecl { name: name.to_string(), description: crate::ai_tools::description(name).unwrap_or_default().to_string(), parameters: crate::ai_tools::schema(name).unwrap_or_default() };
        out.push(mirror(&d));
    }
    out
}

/// `OrchestratorError` (`OE:9-66`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrchError {
    UnknownTool(String),
    UnknownWorkgroup(String),
    UnknownSession(String),
    AmbiguousWorkgroup { name: String, ids: Vec<String> },
    PermissionDenied,
    TargetGone(String),
    Malformed(String),
    Timeout,
    Unsupported(String),
    NotImplemented(String),
    SafetyBlocked(String),
}

impl OrchError {
    pub fn code(&self) -> &'static str {
        match self {
            OrchError::UnknownTool(_) => "unknown_tool",
            OrchError::UnknownWorkgroup(_) => "unknown_workgroup",
            OrchError::UnknownSession(_) => "unknown_session",
            OrchError::AmbiguousWorkgroup { .. } => "ambiguous_workgroup",
            OrchError::PermissionDenied => "permission_denied",
            OrchError::TargetGone(_) => "target_no_longer_exists",
            OrchError::Malformed(_) => "malformed_args",
            OrchError::Timeout => "timeout",
            OrchError::Unsupported(_) => "unsupported",
            OrchError::NotImplemented(_) => "not_implemented",
            OrchError::SafetyBlocked(_) => "safety_blocked",
        }
    }

    pub fn message(&self) -> String {
        match self {
            OrchError::UnknownTool(n) => format!("No such tool: {n}"),
            OrchError::UnknownWorkgroup(id) => format!("No active workgroup matches \u{201C}{id}\u{201D}"),
            OrchError::UnknownSession(g) => format!("No active session has GUID {g}. Copy a session_guid verbatim from the <workgroups> snapshot."),
            OrchError::AmbiguousWorkgroup { name, ids } => format!("Workgroup name \u{201C}{name}\u{201D} matches {} active workgroups; use the workgroup_id instead. Candidates: {}", ids.len(), ids.join(", ")),
            OrchError::PermissionDenied => "User denied permission for this action.".to_string(),
            OrchError::TargetGone(g) => format!("Target session no longer exists: session_guid={g}"),
            OrchError::Malformed(r) => format!("Malformed arguments: {r}"),
            OrchError::Timeout => "The operation timed out.".to_string(),
            OrchError::Unsupported(r) => format!("Unsupported: {r}"),
            OrchError::NotImplemented(d) => format!("Not implemented yet: {d}"),
            OrchError::SafetyBlocked(r) => format!("This command was not run automatically: {r}"),
        }
    }
}

/// The result envelope's error (`OC:356-419`), its text inert for the prompt.
pub fn error_json(e: &OrchError) -> String {
    result_json("error", json!({"code": e.code(), "message": e.message()}))
}

/// `{"ack":true}` (the send_text description's `{"ack":{}}` is not what the encoder writes).
pub fn ack_json() -> String {
    result_json("ack", Value::Bool(true))
}

/// One envelope key and its payload, keys sorted and every `<`, `>`, `&` escaped
/// (`orch::model_json`): program text in a result can forge no tag.
pub fn result_json(key: &str, payload: Value) -> String {
    let mut m = Map::new();
    m.insert(key.to_string(), payload);
    crate::orch::model_json(&Value::Object(m))
}

/// iTerm2's escape vocabulary (`decodeAIBackslashEscapes`, 12c's copy), a bad escape malformed.
pub fn decode_escapes(text: &str) -> Result<String, OrchError> {
    crate::ai_actions::decode_escapes(text).map_err(|e| OrchError::Malformed(format!("text: {e}")))
}

fn malformed(field: &str, want: &str) -> OrchError {
    OrchError::Malformed(format!("{field} must be {want}"))
}

fn obj(args: &Value) -> Result<&Map<String, Value>, OrchError> {
    args.as_object().ok_or_else(|| OrchError::Malformed("the arguments must be a JSON object".to_string()))
}

/// A required string (Swift's synthesized `Decodable`: present and a string).
fn req_str(args: &Value, k: &str) -> Result<String, OrchError> {
    match obj(args)?.get(k) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(malformed(k, "a string")),
        None => Err(OrchError::Malformed(format!("missing {k}"))),
    }
}

/// An optional string: absent or null is None.
fn opt_str(args: &Value, k: &str) -> Result<Option<String>, OrchError> {
    match obj(args)?.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(malformed(k, "a string or null")),
    }
}

fn opt_bool(args: &Value, k: &str) -> Result<Option<bool>, OrchError> {
    match obj(args)?.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(malformed(k, "a boolean or null")),
    }
}

fn opt_int(args: &Value, k: &str) -> Result<Option<i64>, OrchError> {
    match obj(args)?.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| malformed(k, "an integer or null")),
    }
}

/// The session a tool names.
pub fn parse_guid(args: &Value) -> Result<String, OrchError> {
    req_str(args, "session_guid")
}

#[derive(Clone, Debug, PartialEq)]
pub struct SendText {
    pub guid: String,
    pub text: String,
    pub newline: bool,
}

/// `SendTextArgs`: `append_newline` null or absent is true. The text is still escaped.
pub fn parse_send_text(args: &Value) -> Result<SendText, OrchError> {
    Ok(SendText { guid: parse_guid(args)?, text: req_str(args, "text")?, newline: opt_bool(args, "append_newline")?.unwrap_or(true) })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenArgs {
    pub guid: String,
    pub lines: Option<i64>,
}

pub fn parse_get_screen_contents(args: &Value) -> Result<ScreenArgs, OrchError> {
    Ok(ScreenArgs { guid: parse_guid(args)?, lines: opt_int(args, "lines")? })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollWheel {
    pub guid: String,
    pub lines: i64,
    pub up: bool,
}

/// `ScrollWheelArgs`: `direction` null is up.
pub fn parse_scroll_wheel(args: &Value) -> Result<ScrollWheel, OrchError> {
    let lines = obj(args)?.get("lines").and_then(Value::as_i64).ok_or_else(|| malformed("lines", "an integer"))?;
    let up = match opt_str(args, "direction")?.as_deref() {
        None | Some("up") => true,
        Some("down") => false,
        Some(_) => return Err(malformed("direction", "\u{201C}up\u{201D}, \u{201C}down\u{201D} or null")),
    };
    Ok(ScrollWheel { guid: parse_guid(args)?, lines, up })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListClippings {
    pub workgroup_id: String,
    pub type_filter: Option<String>,
}

pub fn parse_list_workgroup_clippings(args: &Value) -> Result<ListClippings, OrchError> {
    Ok(ListClippings { workgroup_id: req_str(args, "workgroup_id")?, type_filter: opt_str(args, "type_filter")? })
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddClipping {
    pub workgroup_id: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
}

/// The longest clipping type (`DI:2938-2948`, the schema's "short tag").
pub const CLIPPING_TYPE_MAX: usize = 80;

/// `AddClippingArgs` and `doAddWorkgroupClipping`'s checks: the trimmed type is 1 to 80
/// characters.
pub fn parse_add_workgroup_clipping(args: &Value) -> Result<AddClipping, OrchError> {
    let kind = req_str(args, "type")?.trim().to_string();
    if kind.is_empty() {
        return Err(OrchError::Malformed("type must be a non-empty short tag describing the clipping kind.".to_string()));
    }
    let n = kind.chars().count();
    if n > CLIPPING_TYPE_MAX {
        return Err(OrchError::Malformed(format!("type must be 80 characters or fewer (got {n}).")));
    }
    Ok(AddClipping { workgroup_id: req_str(args, "workgroup_id")?, kind, title: req_str(args, "title")?, detail: req_str(args, "detail")? })
}

/// What a clipping tool names: a session by `session:<stable id>`, or a workgroup instance by its
/// own `wg-` id (`WorkgroupIntrospection.swift:366-389`). Whether either is live is the app's to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClippingTarget<'a> {
    Session(&'a str),
    Workgroup(&'a str),
}

pub fn clipping_target(workgroup_id: &str) -> Result<ClippingTarget<'_>, OrchError> {
    if let Some(id) = workgroup_id.strip_prefix(crate::orch::SYNTHETIC_PREFIX) {
        return Ok(ClippingTarget::Session(id));
    }
    if crate::workgroup_run::valid_instance_id(workgroup_id) {
        return Ok(ClippingTarget::Workgroup(workgroup_id));
    }
    Err(OrchError::UnknownWorkgroup(workgroup_id.to_string()))
}

/// `list_workgroup_clippings`' payload (`ClippingInfo`): the live list, filtered by exact type.
pub fn clippings_value(list: &[crate::clippings::Clipping], type_filter: Option<&str>) -> Value {
    let rows: Vec<Value> = list.iter().filter(|c| type_filter.is_none_or(|t| c.kind == t)).map(|c| json!({"type": c.kind, "title": c.title, "detail": c.detail})).collect();
    Value::Array(rows)
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartCodeReview {
    pub guid: String,
}

pub fn parse_start_code_review(args: &Value) -> Result<StartCodeReview, OrchError> {
    opt_str(args, "prompt_name")?;
    opt_str(args, "custom_prompt")?;
    Ok(StartCodeReview { guid: parse_guid(args)? })
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartSession {
    pub profile: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// "new", "current" or "tab" (null is "tab").
    pub window: Option<String>,
}

/// `StartSessionArgs`; an empty profile, command or cwd is none.
pub fn parse_start_session(args: &Value) -> Result<StartSession, OrchError> {
    let window = opt_str(args, "window")?;
    if window.as_deref().is_some_and(|w| !["new", "current", "tab"].contains(&w)) {
        return Err(malformed("window", "\u{201C}new\u{201D}, \u{201C}current\u{201D}, \u{201C}tab\u{201D} or null"));
    }
    let some = |v: Option<String>| v.filter(|s| !s.is_empty());
    Ok(StartSession { profile: some(opt_str(args, "profile")?), command: some(opt_str(args, "command")?), cwd: some(opt_str(args, "cwd")?), window })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegisterWatch {
    /// None in a session-bound chat, which names no session.
    pub guid: Option<String>,
    pub target_state: Option<crate::session_state::State>,
    /// Trimmed; empty is none.
    pub condition: Option<String>,
}

/// `RegisterWatchArgs` and `doRegisterWatch`'s first checks: exactly one of a target state and
/// a condition, and never \u{201C}unknown\u{201D}.
pub fn parse_register_watch(args: &Value) -> Result<RegisterWatch, OrchError> {
    let condition = opt_str(args, "condition")?.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    let target_state = match opt_str(args, "target_state")?.as_deref() {
        None => None,
        Some("idle") => Some(crate::session_state::State::Idle),
        Some("working") => Some(crate::session_state::State::Working),
        Some("waiting") => Some(crate::session_state::State::Waiting),
        Some("unknown") => None,
        Some(_) => return Err(malformed("target_state", "\u{201C}idle\u{201D}, \u{201C}working\u{201D}, \u{201C}waiting\u{201D} or null")),
    };
    let named_unknown = opt_str(args, "target_state")?.as_deref() == Some("unknown");
    if condition.is_some() == (target_state.is_some() || named_unknown) {
        return Err(OrchError::Malformed(crate::orch_watch::EXACTLY_ONE.to_string()));
    }
    if named_unknown {
        return Err(OrchError::Malformed(crate::orch_watch::UNKNOWN_STATE.to_string()));
    }
    Ok(RegisterWatch { guid: opt_str(args, "session_guid")?, target_state, condition })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegisterTimer {
    pub delay: Option<f64>,
    pub fire_at: Option<String>,
    /// Trimmed; empty is none.
    pub note: Option<String>,
}

pub fn parse_register_timer(args: &Value) -> Result<RegisterTimer, OrchError> {
    let delay = match obj(args)?.get("delay_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().ok_or_else(|| malformed("delay_seconds", "a number or null"))?),
    };
    let note = opt_str(args, "note")?.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    Ok(RegisterTimer { delay, fire_at: opt_str(args, "fire_at")?, note })
}

pub fn parse_unregister_watch(args: &Value) -> Result<String, OrchError> {
    req_str(args, "watcher_id")
}

/// `sessionBoundRegisterWatchDefinition` (`TD:306-320`): no session_guid, the linked terminal is
/// the target, and no \"unknown\" state.
fn session_bound_register_watch() -> ToolDecl {
    let blocking = "This normally returns immediately and does NOT block your turn; the one exception is a screen-reading watch on a session whose View Contents permission is set to Ask, where it first asks the user to approve repeated screen reads and waits for that answer before returning.";
    let form = "Choosing the form: use target_state (idle/working/waiting) to fire when the running program reports that transition. Use condition, a plain-English description an AI judge evaluates by periodically reading the session's screen, when what you're waiting for isn't an idle/working/waiting transition (e.g. \u{201C}emacs has exited and a shell prompt is showing\u{201D}, \u{201C}the build printed a success or failure line\u{201D}, \u{201C}a password prompt appeared\u{201D}); a specific condition is more accurate for those. Screen-judged watches keep watching on their own, polling less often the longer they run, and only time out (reason=\u{201C}watchTimedOut\u{201D}) after several hours; re-register then if you still need to wait.";
    let description = watch_description("the terminal session this chat is linked to", blocking, form, WATCH_PERSISTENCE);
    let parameters = object(&[
        ("target_state", nullable_enum("State to watch for. Must be a transition target: \u{201C}idle\u{201D}, \u{201C}working\u{201D}, or \u{201C}waiting\u{201D}. Mutually exclusive with condition: set exactly one and null the other.", &["idle", "working", "waiting"])),
        ("condition", nullable("string", "Plain-English condition to watch for, judged by an AI reading the session's screen, e.g. \u{201C}emacs has exited and a shell prompt is showing\u{201D}. Describe what will be VISIBLE on screen when the condition holds. Mutually exclusive with target_state: set exactly one and null the other.")),
    ]);
    decl_of("register_watch", &description, parameters)
}

/// The watch tools a session-bound chat is offered (`registerSessionBoundWatchTools`): the
/// timer, register_watch only when `watch` (a terminal is linked and a form is allowed), then
/// unregister_watch and list_watches.
pub fn session_bound_decls(watch: bool) -> Vec<ToolDecl> {
    let mut out: Vec<ToolDecl> = decl("register_timer").into_iter().collect();
    if watch {
        out.push(session_bound_register_watch());
    }
    out.extend(decl("unregister_watch"));
    out.extend(decl("list_watches"));
    out
}

/// The text and `append_newline` of every tool that types into a session's prompt line (ruling
/// R12f-1, `DI:1302-1325`): send_text's decoded text and its newline (default true),
/// session_execute_command's command and true. Every other tool is None.
pub fn typed_payload(name: &str, args: &Value) -> Result<Option<(String, bool)>, OrchError> {
    match name {
        "send_text" => {
            let s = parse_send_text(args)?;
            Ok(Some((decode_escapes(&s.text)?, s.newline)))
        }
        "session_execute_command" => Ok(Some((req_str(args, "command")?, true))),
        _ => Ok(None),
    }
}

/// How a write tool's claim is found (`OC:303-328`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimBy {
    Session,
    Workgroup,
}

pub fn claim_scope(tool: &str) -> Option<ClaimBy> {
    match tool {
        "send_text" | "interrupt" | "start_code_review" => Some(ClaimBy::Session),
        "add_workgroup_clipping" => Some(ClaimBy::Workgroup),
        _ => None,
    }
}

/// Whether a mirror of a tool in category `c` needs the session claimed (`DI:1468-1476`).
pub fn mirror_needs_claim(c: Category) -> bool {
    matches!(c, Category::RunCommands | Category::WriteToClipboard | Category::ControlTerminal | Category::WriteToFilesystem | Category::ActInWebBrowser)
}

/// A claim scope for a session: iTerm2's synthetic workgroup id.
pub fn session_scope(stable_id: &str) -> String {
    format!("{}{stable_id}", crate::orch::SYNTHETIC_PREFIX)
}

/// What a scope's claim needs now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Granted,
    Ask,
    /// Refused in this chat; asked again only once claimed (a mention grant).
    Denied,
}

pub fn claim_needed(claimed: &[String], scope: &str, denied: &[String]) -> Claim {
    if claimed.iter().any(|s| s == scope) {
        Claim::Granted
    } else if denied.iter().any(|s| s == scope) {
        Claim::Denied
    } else {
        Claim::Ask
    }
}

/// The session claim's summary (`DI:1640-1644`).
pub fn claim_text(name: &str) -> String {
    format!("The agent is asking to send keystrokes and interrupt running commands in session \u{201C}{name}\u{201D}. Approval is sticky for the rest of this chat. Deny to refuse this and future control of this session until you explicitly approve.")
}

/// The notice after Revoke (`DI:1806-1808`).
pub fn revoked_text(name: &str) -> String {
    format!("Revoked this chat\u{2019}s permission to control \u{201C}{name}\u{201D}. The agent will ask before its next action there.")
}

/// A mirror's answer when the claim was refused (`DI:1296`).
pub fn declined_session(guid: &str) -> String {
    format!("The user declined to allow this chat to control session {guid}.")
}

/// Where start_session puts the session, as the spawn bubble says it (`DI:1689-1696`).
pub fn placement(window: Option<&str>) -> &'static str {
    match window {
        Some("new") => "a new window",
        Some("current") => "a vertical split of the current pane",
        _ => "a new tab in the current window",
    }
}

/// The spawn bubble's summary (`DI:1697-1703`).
pub fn start_prompt(placement: &str, command: Option<&str>) -> String {
    match command {
        Some(c) => format!("The agent is asking to open {placement} and run \u{201C}{c}\u{201D}."),
        None => format!("The agent is asking to open {placement}."),
    }
}

/// A command-approval bubble's summary (`DI:2808-2816`).
pub fn safety_summary(action: &str, reason: &str, denied: bool) -> String {
    let lead = if denied { "The safety check flagged this as potentially dangerous.\n\n" } else { "" };
    format!("{lead}{action}\n\nWhy it needs review: {reason}")
}

/// `DI:2698`.
pub fn run_summary(command: &str) -> String {
    format!("The agent wants to run:\n\n`{command}`")
}

/// `DI:2772`: the keystroke with its control bytes shown as `\uXXXX`.
pub fn keystroke_summary(text: &str) -> String {
    format!("The agent wants to send this to the session's foreground program:\n\n`{}`", display_keystroke(text))
}

/// `TUISafetyPrompt.displayKeystroke`: control bytes and DEL as lowercase `\uXXXX`.
pub fn display_keystroke(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if (c as u32) < 0x20 || c == '\u{7f}' {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// start_code_review on anything but the Code Review role (`DI:3168`); Tandem has no
/// workgroups before milestone 13, so it is every target.
pub fn not_code_review(role: &str, wg: &str) -> String {
    format!("Target {role} in {wg} is not the Code Review role. start_code_review only targets the Code Review role; use send_text if you want to type text into another role.")
}

/// start_code_review on a review session that is not idle while no prompt overlay is up
/// (`DI:3184-3186`): restarting it would kill a review under way.
pub fn review_busy(role: &str, wg: &str, state: &str) -> String {
    format!("Target {role} in {wg} is busy (status: {state}). Wait until it returns to idle before starting a review.")
}

/// scroll_wheel without mouse reporting (`DI:1971-1975`).
pub fn no_mouse_reporting(role: &str, wg: &str) -> String {
    format!("Mouse reporting is not enabled in {role} ({wg}), so the scroll wheel can't be used. If this is the primary screen, just ask get_screen_contents for more lines instead.")
}

/// `DI:2987`.
pub fn no_profile(name: &str) -> String {
    format!("No profile named \u{201C}{name}\u{201D}.")
}

/// `DI:3034-3037`.
pub const NO_WINDOW_FOR_CURRENT: &str = "window=\u{201C}current\u{201D} splits the currently-focused terminal, but no terminal window is open. Use window=\u{201C}new\u{201D} or window=\u{201C}tab\u{201D} instead.";
/// `DI:3062-3063`.
pub const LAUNCH_FAILED: &str = "Tandem could not open a new session. The launch failed before the window was shown.";
/// Tandem's: a start directory on another computer is never used (ruling P15).
pub const NETWORK_CWD: &str = "cwd must be a folder on this computer; network paths are not allowed.";
/// Tandem's spawn cap (ruling P15).
pub const TOO_MANY_SPAWNS: &str = "This chat has started too many sessions; wait a minute or close some first.";
/// The provenance of a session the agent started (`DI:3106-3114`).
pub fn provenance(title: Option<&str>, chat: &str) -> String {
    match title.filter(|t| !t.is_empty()) {
        Some(t) => format!("Created by the agent in chat \u{201C}{t}\u{201D} ({chat})."),
        None => format!("Created by the agent in chat {chat}."),
    }
}

/// Tandem's (ruling P8): a session that cannot be typed into now.
pub fn not_typeable(guid: &str, why: &str) -> String {
    format!("Session {guid} cannot be controlled now: {why}.")
}
pub const BURIED: &str = "it is buried; ask the user to show it again first";
pub const AT_PASSWORD: &str = "it is waiting for a password, which only the user may type";
pub const GATEWAY: &str = "it is a tmux gateway, which takes no typing";

/// The restart bubble's summary (ruling P16: a restart always asks, per call).
pub fn restart_summary(name: &str) -> String {
    format!("The agent wants to restart session \u{201C}{name}\u{201D}. Every program running in it will be stopped.")
}

/// `DI:2810`, a refused approval of a requireApproval verdict.
pub const DECLINED_APPROVAL: &str = "the user declined to approve it.";
pub const FAILCLOSED_SUMMARY: &str = "The agent wants to run a command in the session.";

pub fn file_summary(filename: &str) -> String {
    format!("The agent wants to create the file `{filename}`.")
}
pub const FILE_TOO_LARGE: &str = "the file is too large to safety-check automatically";
/// `createFileMaxClassifiableContent`, in characters.
pub const CREATE_FILE_MAX: usize = 8000;

// The `session_*` mirrors' plain-text answers (`DI:1226-1390`).
pub fn unknown_session_tool(name: &str) -> String {
    format!("Error: unknown session tool {name}")
}
pub fn not_in_orchestration(name: &str) -> String {
    format!("Error: {name} is not available in orchestration mode; use send_text (which is safety-checked).")
}
pub fn use_workgroup_screen(name: &str) -> String {
    format!("Error: {name} is not available; use the workgroup-level get_screen_contents.")
}
pub const ARGS_NOT_OBJECT: &str = "Error: arguments not a JSON object";
pub fn args_error(e: &str) -> String {
    format!("Error decoding arguments: {e}")
}
pub const MISSING_GUID: &str = "Error: missing session_guid";
pub fn no_session(guid: &str) -> String {
    format!("Error: no session with GUID {guid}")
}
pub const NEEDS_APPROVAL: &str = "Not run automatically: this action needs manual approval first. Ask the user to run it themselves or to approve it.";
pub const SESSION_TIMEOUT: &str = "Error: session command timed out after 30s.";
pub const LEFT_ORCHESTRATION: &str = "This chat is no longer in orchestration mode.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(d: &ToolDecl) -> &Map<String, Value> {
        d.parameters["properties"].as_object().unwrap()
    }

    #[test]
    fn tool_decls_follow_iterm2() {
        assert_eq!(
            TOOLS,
            ["list_workgroups", "get_state", "get_screen_contents", "scroll_wheel", "list_workgroup_clippings", "send_text", "interrupt", "add_workgroup_clipping", "start_code_review", "start_session", "register_watch", "register_timer", "unregister_watch", "list_watches"]
        );
        for name in TOOLS {
            let d = decl(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(d.parameters["additionalProperties"], false, "{name}");
            let mut keys: Vec<&String> = props(&d).keys().collect();
            let mut req: Vec<&str> = d.parameters["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            keys.sort();
            req.sort();
            assert_eq!(keys, req, "{name}: every property required");
            assert!(!d.description.contains("iTerm2") && !d.description.contains('\u{2014}') || name == "register_watch", "{name}");
            assert!(!d.description.contains("12 orchestrator tools"));
        }
        assert!(decl("notify").is_none() && decl("request_notification_permission").is_none());
        let s = decl("send_text").unwrap();
        assert_eq!(props(&s)["session_guid"]["type"], "string");
        assert_eq!(props(&s)["text"], json!({"type": "string"}));
        assert_eq!(props(&s)["append_newline"]["type"], json!(["boolean", "null"]));
        assert!(s.description.starts_with("Type text into a session as if the user typed it.") && s.description.contains("Tandem detects embedded control bytes"));
        assert_eq!(props(&decl("scroll_wheel").unwrap())["direction"]["enum"], json!(["up", "down", null]));
        assert_eq!(props(&decl("start_session").unwrap())["window"]["enum"], json!(["new", "current", "tab", null]));
        let w = decl("register_watch").unwrap();
        assert_eq!(props(&w)["target_state"]["enum"], json!(["idle", "working", "waiting", "unknown", null]));
        assert!(!props(&w).contains_key("notify_user") && !props(&decl("register_timer").unwrap()).contains_key("notify_user"));
        assert!(w.description.starts_with("Register an async watcher on a session. Supply exactly ONE") && !w.description.contains("iPhone"));
        let exec = ToolDecl { name: "execute_command".into(), description: crate::ai_tools::description("execute_command").unwrap().into(), parameters: crate::ai_tools::schema("execute_command").unwrap() };
        let m = mirror(&exec);
        assert_eq!(m.name, "session_execute_command");
        assert!(m.description.ends_with(MIRROR_SUFFIX));
        assert!(m.parameters["required"].as_array().unwrap().contains(&json!("session_guid")));
        assert_eq!(m.parameters["properties"]["session_guid"]["description"], MIRROR_GUID);
        assert_eq!(NOT_MIRRORED, ["get_screen_contents", "insert_text_at_cursor", "delete_current_line"]);
        let all: Vec<String> = all_decls().into_iter().map(|d| d.name).collect();
        assert_eq!(all.len(), 14 + 26 - 3);
        assert!(all.contains(&"session_restart_session".to_string()) && all.contains(&"session_load_url".to_string()));
        assert!(!all.iter().any(|n| n == "session_insert_text_at_cursor" || n == "session_delete_current_line" || n == "session_get_screen_contents"));
    }

    #[test]
    fn watch_args_follow_iterm2() {
        use crate::session_state::State;
        let ok = |v: Value| parse_register_watch(&v).unwrap();
        assert_eq!(ok(json!({"session_guid": "ptys_0123456789ABW", "target_state": "idle", "condition": null})), RegisterWatch { guid: Some("ptys_0123456789ABW".into()), target_state: Some(State::Idle), condition: None });
        assert_eq!(ok(json!({"target_state": null, "condition": "  build done \n"})), RegisterWatch { guid: None, target_state: None, condition: Some("build done".into()) }, "a session-bound chat names no session; the condition is trimmed");
        assert_eq!(ok(json!({"target_state": "waiting", "condition": ""})).target_state, Some(State::Waiting), "an empty condition is none");
        let bad = |v: Value| match parse_register_watch(&v) {
            Err(OrchError::Malformed(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(bad(json!({"target_state": null, "condition": null})), crate::orch_watch::EXACTLY_ONE);
        assert_eq!(bad(json!({"target_state": "idle", "condition": "x"})), crate::orch_watch::EXACTLY_ONE);
        assert_eq!(bad(json!({"target_state": null, "condition": "   "})), crate::orch_watch::EXACTLY_ONE);
        assert_eq!(bad(json!({"target_state": "unknown", "condition": null})), crate::orch_watch::UNKNOWN_STATE);
        assert!(bad(json!({"target_state": "sleeping", "condition": null})).contains("target_state"));
        assert!(bad(json!({"target_state": 3, "condition": null})).contains("target_state"));
        assert!(parse_register_watch(&json!([1])).is_err());
        let t = parse_register_timer(&json!({"delay_seconds": 90, "fire_at": null, "note": "  ship it "})).unwrap();
        assert_eq!(t, RegisterTimer { delay: Some(90.0), fire_at: None, note: Some("ship it".into()) });
        let t = parse_register_timer(&json!({"delay_seconds": null, "fire_at": "2026-10-13T14:30:05Z", "note": "  "})).unwrap();
        assert_eq!(t, RegisterTimer { delay: None, fire_at: Some("2026-10-13T14:30:05Z".into()), note: None });
        assert!(parse_register_timer(&json!({"delay_seconds": "soon"})).is_err());
        assert_eq!(parse_unregister_watch(&json!({"watcher_id": "W1"})), Ok("W1".to_string()));
        assert!(parse_unregister_watch(&json!({})).is_err());
    }

    #[test]
    fn the_watch_descriptions_tell_the_truth() {
        // Ruling P2: nothing survives a launch, and Stop and the mode switch cancel everything.
        for d in [decl("register_watch").unwrap(), decl("register_timer").unwrap()].into_iter().chain(session_bound_decls(true)) {
            let low = d.description.to_lowercase();
            assert!(!low.contains("persist across") && !low.contains("saved across") && !low.contains("resume when you next"), "{}: {}", d.name, d.description);
        }
        assert!(decl("register_watch").unwrap().description.contains("do not survive a Tandem restart"));
        assert!(decl("register_timer").unwrap().description.contains("do not survive a Tandem restart"));
    }

    #[test]
    fn session_bound_watch_tools_follow_iterm2() {
        let names = |watch: bool| session_bound_decls(watch).into_iter().map(|d| d.name).collect::<Vec<_>>();
        assert_eq!(names(true), ["register_timer", "register_watch", "unregister_watch", "list_watches"]);
        assert_eq!(names(false), ["register_timer", "unregister_watch", "list_watches"]);
        let w = session_bound_decls(true).into_iter().find(|d| d.name == "register_watch").unwrap();
        assert!(w.description.starts_with("Register an async watcher on the terminal session this chat is linked to. Supply exactly ONE of target_state or condition."), "{}", w.description);
        assert!(w.description.contains("a screen-reading watch on a session whose View Contents permission is set to Ask"));
        assert!(!props(&w).contains_key("session_guid"), "the target is the linked session");
        assert_eq!(props(&w)["target_state"]["enum"], json!(["idle", "working", "waiting", null]), "no unknown");
        assert_eq!(w.parameters["required"], json!(["target_state", "condition"]));
        assert_eq!(w.parameters["additionalProperties"], false);
        assert!(!w.description.contains("iTerm2") && !w.description.contains("list_workgroups"));
        let t = session_bound_decls(false).into_iter().find(|d| d.name == "register_timer").unwrap();
        assert_eq!(t, decl("register_timer").unwrap());
    }

    #[test]
    fn args_follow_iterm2() {
        let s = parse_send_text(&json!({"session_guid": "ptys_0123456789ABW", "text": "ls", "append_newline": null})).unwrap();
        assert_eq!(s, SendText { guid: "ptys_0123456789ABW".into(), text: "ls".into(), newline: true });
        assert!(matches!(parse_send_text(&json!({"session_guid": "g"})), Err(OrchError::Malformed(_))));
        let cases = [
            (OrchError::UnknownTool("x".into()), "unknown_tool", "No such tool: x"),
            (OrchError::UnknownSession("g".into()), "unknown_session", "No active session has GUID g. Copy a session_guid verbatim from the <workgroups> snapshot."),
            (OrchError::UnknownWorkgroup("w".into()), "unknown_workgroup", "No active workgroup matches \u{201C}w\u{201D}"),
            (OrchError::PermissionDenied, "permission_denied", "User denied permission for this action."),
            (OrchError::TargetGone("g".into()), "target_no_longer_exists", "Target session no longer exists: session_guid=g"),
            (OrchError::Malformed("r".into()), "malformed_args", "Malformed arguments: r"),
            (OrchError::Timeout, "timeout", "The operation timed out."),
            (OrchError::Unsupported("r".into()), "unsupported", "Unsupported: r"),
            (OrchError::NotImplemented("d".into()), "not_implemented", "Not implemented yet: d"),
            (OrchError::SafetyBlocked("r".into()), "safety_blocked", "This command was not run automatically: r"),
            (OrchError::AmbiguousWorkgroup { name: "n".into(), ids: vec!["a".into(), "b".into()] }, "ambiguous_workgroup", "Workgroup name \u{201C}n\u{201D} matches 2 active workgroups; use the workgroup_id instead. Candidates: a, b"),
        ];
        for (e, code, message) in cases {
            assert_eq!((e.code(), e.message()), (code, message.to_string()));
        }
        assert_eq!(ack_json(), "{\n  \"ack\": true\n}");
        // Task 10 review Important 1 (`DI:3184-3186`).
        assert_eq!(review_busy("Review", "Feature", "working"), "Target Review in Feature is busy (status: working). Wait until it returns to idle before starting a review.");
        assert_eq!(not_code_review("default", "vim"), "Target default in vim is not the Code Review role. start_code_review only targets the Code Review role; use send_text if you want to type text into another role.");
        assert_eq!(placement(Some("new")), "a new window");
        assert_eq!((placement(Some("tab")), placement(None)), ("a new tab in the current window", "a new tab in the current window"));
        assert_eq!(placement(Some("current")), "a vertical split of the current pane");
        assert_eq!(start_prompt("a new window", Some("make")), "The agent is asking to open a new window and run \u{201C}make\u{201D}.");
        assert_eq!(claim_text("vim"), "The agent is asking to send keystrokes and interrupt running commands in session \u{201C}vim\u{201D}. Approval is sticky for the rest of this chat. Deny to refuse this and future control of this session until you explicitly approve.");
        let s = safety_summary("The agent wants to run:\n\n`rm x`", "deletes files", true);
        assert!(s.starts_with("The safety check flagged this as potentially dangerous.\n\n") && s.ends_with("\n\nWhy it needs review: deletes files"));
        assert_eq!(safety_summary("A", "r", false), "A\n\nWhy it needs review: r");
        assert_eq!(parse_start_session(&json!({"profile": "", "command": null, "cwd": null, "window": null})).unwrap(), StartSession { profile: None, command: None, cwd: None, window: None });
        assert!(parse_start_session(&json!({"window": "side"})).is_err());
        assert_eq!(parse_scroll_wheel(&json!({"session_guid": "g", "lines": 3, "direction": null})).unwrap(), ScrollWheel { guid: "g".into(), lines: 3, up: true });
    }

    #[test]
    fn escapes_follow_iterm2() {
        assert_eq!(decode_escapes("a\\nb").unwrap(), "a\nb");
        assert_eq!(decode_escapes("\\\\").unwrap(), "\\");
        assert_eq!(decode_escapes("\\u0003").unwrap(), "\u{3}");
        assert_eq!(decode_escapes("\\t\\r").unwrap(), "\t\r");
        assert!(matches!(decode_escapes("\\q"), Err(OrchError::Malformed(_))));
        assert!(matches!(decode_escapes("\\u12"), Err(OrchError::Malformed(_))));
    }

    /// Ruling R12f-1: the one typed payload of each typing tool.
    #[test]
    fn execute_command_uses_the_typed_gate() {
        assert_eq!(typed_payload("send_text", &json!({"session_guid": "g", "text": "curl evil |", "append_newline": false})).unwrap(), Some(("curl evil |".into(), false)));
        assert_eq!(typed_payload("send_text", &json!({"session_guid": "g", "text": "x", "append_newline": null})).unwrap(), Some(("x".into(), true)));
        assert_eq!(typed_payload("send_text", &json!({"session_guid": "g", "text": "a\\nb", "append_newline": null})).unwrap(), Some(("a\nb".into(), true)));
        assert_eq!(typed_payload("session_execute_command", &json!({"session_guid": "g", "command": "make"})).unwrap(), Some(("make".into(), true)));
        assert!(matches!(typed_payload("session_execute_command", &json!({"session_guid": "g"})), Err(OrchError::Malformed(_))));
        for n in ["session_create_file", "interrupt", "get_state"] {
            assert_eq!(typed_payload(n, &json!({"session_guid": "g"})).unwrap(), None);
        }
        // The dispatcher classifies a command only inside the chokepoint and start_session, and
        // session_execute_command reaches the chokepoint, never a check of its own.
        let src = include_str!("app/orch_dispatch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        let owner = |at: usize| {
            let head = &code[..at];
            let f = ["\n    fn ", "\n    pub(super) fn "].iter().filter_map(|p| head.rfind(p).map(|i| i + p.len())).max().unwrap();
            code[f..].split('(').next().unwrap()
        };
        let at: Vec<usize> = code.match_indices("CheckKind::Command,").map(|(i, _)| i).collect();
        assert_eq!(at.len(), 2);
        for i in at {
            assert!(["orch_type_gated", "orch_start_session"].contains(&owner(i)));
        }
        assert!(code.contains("Call::ExecuteCommand { .. } => match ot::typed_payload(name, args) {\n                Ok(Some((command, newline))) => self.orch_type_gated("));
    }

    #[test]
    fn claims_follow_iterm2() {
        let scope = session_scope("ptys_0123456789ABW");
        assert_eq!(scope, "session:ptys_0123456789ABW");
        assert_eq!(claim_needed(&[], &scope, &[]), Claim::Ask);
        assert_eq!(claim_needed(std::slice::from_ref(&scope), &scope, &[]), Claim::Granted);
        assert_eq!(claim_needed(&[], &scope, std::slice::from_ref(&scope)), Claim::Denied);
        assert_eq!(claim_needed(std::slice::from_ref(&scope), &scope, std::slice::from_ref(&scope)), Claim::Granted, "a later claim wins");
        assert_eq!((claim_scope("send_text"), claim_scope("interrupt"), claim_scope("start_code_review")), (Some(ClaimBy::Session), Some(ClaimBy::Session), Some(ClaimBy::Session)));
        assert_eq!(claim_scope("add_workgroup_clipping"), Some(ClaimBy::Workgroup));
        assert_eq!((claim_scope("get_state"), claim_scope("scroll_wheel"), claim_scope("start_session")), (None, None, None));
        for c in [Category::RunCommands, Category::WriteToClipboard, Category::ControlTerminal, Category::WriteToFilesystem, Category::ActInWebBrowser] {
            assert!(mirror_needs_claim(c));
        }
        for c in [Category::CheckTerminalState, Category::ViewContents, Category::ViewManpages] {
            assert!(!mirror_needs_claim(c));
        }
        // A mention grant: only the person's own picks (12d's send_grants), canonical ids only.
        let grants = crate::mention::send_grants("@ptys_0123456789ABW fix it", &["ptys_0123456789ABW".to_string()]);
        assert_eq!(grants.iter().map(|g| session_scope(g)).collect::<Vec<_>>(), [scope]);
        assert!(crate::mention::send_grants("@ptys_0123456789ABW fix it", &[]).is_empty(), "text the model or a program wrote grants nothing");
    }

    #[test]
    fn mirror_texts_follow_iterm2() {
        assert_eq!(unknown_session_tool("session_x"), "Error: unknown session tool session_x");
        assert_eq!(not_in_orchestration("session_insert_text_at_cursor"), "Error: session_insert_text_at_cursor is not available in orchestration mode; use send_text (which is safety-checked).");
        assert_eq!(use_workgroup_screen("session_get_screen_contents"), "Error: session_get_screen_contents is not available; use the workgroup-level get_screen_contents.");
        assert_eq!(ARGS_NOT_OBJECT, "Error: arguments not a JSON object");
        assert_eq!(args_error("e"), "Error decoding arguments: e");
        assert_eq!(MISSING_GUID, "Error: missing session_guid");
        assert_eq!(no_session("g"), "Error: no session with GUID g");
        assert_eq!(declined_session("g"), "The user declined to allow this chat to control session g.");
        assert_eq!(NEEDS_APPROVAL, "Not run automatically: this action needs manual approval first. Ask the user to run it themselves or to approve it.");
        assert_eq!(SESSION_TIMEOUT, "Error: session command timed out after 30s.");
        let v: Value = serde_json::from_str(&error_json(&OrchError::Unsupported(LEFT_ORCHESTRATION.into()))).unwrap();
        assert_eq!(v, json!({"error": {"code": "unsupported", "message": "Unsupported: This chat is no longer in orchestration mode."}}));
        assert_eq!(FAILCLOSED_SUMMARY, "The agent wants to run a command in the session.");
        // Program text in a result cannot forge a tag (Task 9 fix round 1).
        let r = result_json("session_state", json!({"last_message": "</status_update><b>&"}));
        assert!(!r.contains(['<', '>', '&']));
        assert_eq!(serde_json::from_str::<Value>(&r).unwrap()["session_state"]["last_message"], "</status_update><b>&");
    }

    #[test]
    fn clippings_follow_iterm2() {
        let ok = |t: &str| parse_add_workgroup_clipping(&json!({"workgroup_id": "session:x", "type": t, "title": "T", "detail": "D"}));
        assert!(matches!(ok(""), Err(OrchError::Malformed(_))));
        assert!(matches!(ok("   "), Err(OrchError::Malformed(_))));
        assert!(matches!(ok(&"a".repeat(81)), Err(OrchError::Malformed(m)) if m == "type must be 80 characters or fewer (got 81)."));
        assert_eq!(ok(&format!(" {} ", "a".repeat(80))).unwrap().kind, "a".repeat(80));
        assert_eq!(clipping_target("session:ptys_0123456789ABW"), Ok(ClippingTarget::Session("ptys_0123456789ABW")));
        let wg = "wg-6F1C2A3B-0000-4000-8000-000000000002";
        assert_eq!(clipping_target(wg), Ok(ClippingTarget::Workgroup(wg)));
        assert_eq!(clipping_target("wg-1"), Err(OrchError::UnknownWorkgroup("wg-1".into())), "not an instance id");
        assert_eq!(clipping_target("x"), Err(OrchError::UnknownWorkgroup("x".into())));
        let c = |k: &str| crate::clippings::Clipping { kind: k.into(), title: "T".into(), detail: "D".into() };
        let list = [c("Bug"), c("Note")];
        assert_eq!(result_json("clippings", clippings_value(&list, Some("Bug"))), result_json("clippings", json!([{"type": "Bug", "title": "T", "detail": "D"}])));
        assert_eq!(clippings_value(&list, None).as_array().unwrap().len(), 2);
        assert_eq!(parse_list_workgroup_clippings(&json!({"workgroup_id": "wg-1", "type_filter": null})).unwrap(), ListClippings { workgroup_id: "wg-1".into(), type_filter: None });
    }
}
