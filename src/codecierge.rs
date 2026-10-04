//! Codecierge's per-session model, texts and views (`CG` = `Toolbelt/ToolCodecierge.swift`): the
//! stage a session is in, what each stage shows, and the transcript's markdown sections. Pure;
//! the App draws the views and runs the conversation. The conversation itself (prompts, feed,
//! functions) and the Windows write plan are here too.

use std::path::{Path, PathBuf};

/// Codecierge's onboarding states (`CG:646-709`): the administrator's switch, then `EnableAI`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeciergeState {
    /// `GenerativeAIAllowed` false: Get Started is disabled.
    AdminDisabled,
    /// `EnableAI` false: Reveal Setting offers Settings.
    NeedsAi,
    Ready,
}

pub const ONBOARDING: &str = "Codecierge uses AI to help you in your terminal.";
pub const ADMIN_DISABLED: &str = "Generative AI has been disabled by the system administrator.";
pub const MUST_ENABLE: &str = "You must enable AI features to use Codecierge.";
pub const GET_STARTED: &str = "Get Started";
pub const REVEAL_SETTING: &str = "Reveal Setting";
pub const GOAL_PROMPT: &str = "What are you trying to do? I'll suggest commands and explain their output.";
pub const GOAL_PLACEHOLDER: &str = "I want to\u{2026}";
pub const AUTO_LABEL: &str = "Run commands automatically";
pub const START: &str = "Start";
pub const END_TASK: &str = "End Task";
pub const RESPONSE_PLACEHOLDER: &str = "Response";
pub const THINKING: &str = "Thinking\u{2026}";
pub const DANGER_HEADING: &str = "Danger!";
pub const DANGER_TEXT: &str = "This lets an AI completely control your computer. It could delete your files, do something stupid or dangerous, or lead to the downfall of humanity. Proceed with caution.";
pub const PRIVACY_HEADING: &str = "Privacy Warning";
pub const PRIVACY_TEXT: &str = "Everything that happens in your terminal while Codecierge is running will be sent to your AI provider. Don't send them confidential information!";
/// The global that remembers the Privacy Warning was accepted (`CG:455`).
pub const PRIVACY_KEY: &str = "NoSyncCodeciergeWarningAcknowledged";

/// The tool's minimum height, points (`CG`, min 130).
pub const MIN_HEIGHT: f32 = 130.0;
/// The suggestion view's opacity while the assistant works (`CG:914-1115`).
pub const BUSY_ALPHA: f32 = 0.75;

/// The onboarding state for the two switches.
pub fn onboarding(admin_allowed: bool, ai_enabled: bool) -> CodeciergeState {
    if !admin_allowed {
        CodeciergeState::AdminDisabled
    } else if !ai_enabled {
        CodeciergeState::NeedsAi
    } else {
        CodeciergeState::Ready
    }
}

/// What the onboarding view's label says for a state.
pub fn codecierge_text(s: CodeciergeState) -> &'static str {
    match s {
        CodeciergeState::AdminDisabled => ADMIN_DISABLED,
        CodeciergeState::NeedsAi => MUST_ENABLE,
        CodeciergeState::Ready => ONBOARDING,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Onboarding,
    GoalSetting,
    Running,
}

/// One terminal session's Codecierge (`CG:554-567`: created on first use, per session guid).
#[derive(Clone, PartialEq)]
pub struct Session {
    pub guid: String,
    pub stage: Stage,
    pub goal: String,
    /// iTerm2's `ghostRiding`: run commands automatically. True at creation until Start sets it
    /// from the checkbox (`CG:559-561`, `CG:455-457`).
    pub auto: bool,
    /// The goal view's checkbox.
    pub auto_box: bool,
    pub transcript: String,
    pub busy: bool,
    /// Commands run since the task began or the usage warning was last answered (Task 7).
    pub commands: u32,
    /// The reply field's text.
    pub reply: String,
    /// The conversation: the system message, then each exchange the model finished.
    pub messages: Vec<crate::ai_msg::Message>,
    /// Texts waiting to be sent, one request at a time, and whether the answer appends (a reply)
    /// or replaces the transcript (a feed or the first message), `CG:313-340`.
    pub queue: std::collections::VecDeque<(String, bool)>,
    /// The run in flight (the App's run id).
    pub request: Option<u64>,
    /// A function call waiting for the next finished command: its run id and call id.
    pub pending: Option<(u64, String)>,
    /// True when the pending call is a typed `write`, answered "Done" instead of the feed.
    pub pending_done: bool,
    /// The text the run in flight answers and whether its answer appends.
    pub sent: Option<(String, bool)>,
    /// The model's own function call, kept until it is answered so the conversation keeps call
    /// then result in order, as iTerm2's `AIConversation` does.
    pub call: Option<crate::ai_msg::Message>,
}

/// The goal, the transcript and the reply are what the person and the model said: never printed.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("guid", &self.guid)
            .field("stage", &self.stage)
            .field("auto", &self.auto)
            .field("auto_box", &self.auto_box)
            .field("busy", &self.busy)
            .field("commands", &self.commands)
            .field("goal_len", &self.goal.len())
            .field("transcript_len", &self.transcript.len())
            .field("reply_len", &self.reply.len())
            .field("messages", &self.messages.len())
            .field("queued", &self.queue.len())
            .field("request", &self.request)
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

impl Session {
    pub fn new(guid: &str, allowed: bool, has_key: bool) -> Session {
        Session { guid: guid.to_string(), stage: initial_stage(allowed, has_key), goal: String::new(), auto: true, auto_box: false, transcript: String::new(), busy: false, commands: 0, reply: String::new(), messages: Vec::new(), queue: std::collections::VecDeque::new(), request: None, pending: None, pending_done: false, sent: None, call: None }
    }

    /// A click on "Run commands automatically" (`CG:837-850`, final ruling): unticking is at
    /// once; ticking only asks Danger! (true) and the box stays unticked until it is answered
    /// OK, so a Start before the answer runs without auto-run.
    pub fn box_clicked(&mut self) -> bool {
        if self.auto_box {
            self.auto_box = false;
            return false;
        }
        true
    }

    /// Danger! answered with the button at `index`: only OK (0) ticks the box. Cancel, a close
    /// and Escape untick it and turn auto-run off at once for a session already running; OK
    /// never turns it on for one (Start reads the box).
    pub fn danger_answered(&mut self, index: usize) {
        self.auto_box = index == 0;
        if !self.auto_box {
            self.auto = false;
        }
    }

    /// Start's auto-run flag (`CG:455-457`): the ticked box, only while the model can call
    /// functions (the box is enabled only then, `CG:795-799`).
    pub fn begin_auto(&mut self, function_calling: bool) {
        self.auto = self.auto_box && function_calling;
    }
}

/// Danger!'s buttons; a close and Escape answer Cancel.
pub const DANGER_BUTTONS: [&str; 2] = ["OK", "Cancel"];
pub const DANGER_CLOSE: &str = "Cancel";

/// Whether the Privacy Warning's answer starts the session: only OK (`CG:455`).
pub fn privacy_starts(index: usize) -> bool {
    index == 0
}

/// The first stage (`CG:412-418`): goal setting only with a registration, which iTerm2 has
/// while the gate is open and a key is stored. `allowed` is the whole gate (consent included).
pub fn initial_stage(allowed: bool, has_key: bool) -> Stage {
    if allowed && has_key {
        Stage::GoalSetting
    } else {
        Stage::Onboarding
    }
}

/// What Get Started does (ruling R12e-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetStarted {
    /// The gate is closed: stay (12a's gate alert explains).
    Stay,
    /// The gate is open and no key is stored: ask for one.
    AskKey,
    Goal,
}

/// A closed gate stays in onboarding whatever the key (a stored key is not permission).
pub fn get_started(gate_open: bool, has_key: bool) -> GetStarted {
    match (gate_open, has_key) {
        (false, _) => GetStarted::Stay,
        (true, false) => GetStarted::AskKey,
        (true, true) => GetStarted::Goal,
    }
}

/// The suggestion view's goal line.
pub fn goal_label(goal: &str) -> String {
    format!("Goal: {goal}")
}

/// Start is enabled with a goal (`CG:745-912`).
pub fn start_enabled(goal: &str) -> bool {
    !goal.is_empty()
}

/// The auto-run checkbox is enabled only when the model has function calling (`CG:795-799`).
pub fn auto_enabled(function_calling: bool) -> bool {
    function_calling
}

/// The heading a transcript section carries (iTerm2's emoji dropped, Decision).
pub fn assistant(text: &str) -> String {
    format!("### Assistant\n{text}")
}

pub fn assistant_more(text: &str) -> String {
    format!("\n\n### Assistant\n{text}")
}

pub fn you(text: &str) -> String {
    format!("\n\n### You\n{text}\n")
}

/// A finished command: backticks escaped, then cut to 80 characters with an ellipsis.
pub fn terminal(command: &str) -> String {
    let escaped = command.replace('`', "\\`");
    let cut = if escaped.chars().count() > 80 { format!("{}\u{2026}", escaped.chars().take(79).collect::<String>()) } else { escaped };
    format!("\n\n### Terminal\n`{cut}`")
}

pub const ASSISTANT_THINKING: &str = "\n\n### Assistant\nThinking\u{2026}";

/// What the tool shows for a session.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum View {
    Onboarding(CodeciergeState),
    Goal { goal: String, auto_box: bool, auto_enabled: bool, start_enabled: bool },
    /// `goal` is the label to draw ("Goal: ...").
    Running { goal: String, transcript: String, busy: bool, reply: String },
}

pub fn view(s: &Session, admin_allowed: bool, ai_enabled: bool, function_calling: bool) -> View {
    match s.stage {
        Stage::Onboarding => View::Onboarding(onboarding(admin_allowed, ai_enabled)),
        Stage::GoalSetting => View::Goal { goal: s.goal.clone(), auto_box: s.auto_box, auto_enabled: auto_enabled(function_calling), start_enabled: start_enabled(&s.goal) },
        Stage::Running => View::Running { goal: goal_label(&s.goal), transcript: s.transcript.clone(), busy: s.busy, reply: s.reply.clone() },
    }
}

// ---- The conversation (`CG:62-370`) ----

pub const GHOST_RIDING_KEY: &str = "codeciergeGhostRidingPrompt";
pub const REGULAR_KEY: &str = "codeciergeRegularPrompt";
pub const WARNING_COUNT_KEY: &str = "codeciergeCommandWarningCount";

/// `codeciergeGhostRidingPrompt`'s default (`ADV:921`): the system prompt when commands run
/// automatically.
pub const GHOST_RIDING_DEFAULT: &str = "You operate a terminal emulator for me. My goal is $GOAL. $CONTEXT. Ask clarifying questions as needed and run commands using the execute function when you're ready. When I've reached my goal, remind me to click the End Task button.";
/// `codeciergeRegularPrompt`'s default (`ADV:923`), "You help a me" kept as iTerm2 has it.
pub const REGULAR_DEFAULT: &str = "You help a me in a terminal emulator. My goal is $GOAL. $CONTEXT. Start by suggesting a command. Don't overwhelm me with too much information: just go one step at a time. When I've reached my goal, remind me to click the End Task button.";

pub const GET_STARTED_MESSAGE: &str = "Tell me how to get started.";
pub const SESSION_GONE: &str = "The session no longer exists";
/// The answer to a call that arrives after Danger! was refused while the session ran.
pub const AUTO_OFF: &str = "Commands are no longer run automatically in this session. Tell the user what to run instead.";

pub const USAGE_HEADING: &str = "Codecierge Usage Warning";
pub const USAGE_TEXT: &str = "Your codecierge session has been going on for a long time. Are you still using it? If not, stop it to save money and privacy.";
pub const KEEP_GOING: &str = "Keep Going";
pub const STOP: &str = "Stop Codecierge";
pub const USAGE_ID: &str = "CodeciergeCommandWarning";

/// Why a file is not written (Tandem's, rulings R12e-2, R12e-6 and R12e-7).
pub const UNKNOWN_SHELL: &str = "Tandem cannot tell which shell this session runs, so Codecierge cannot write files in it.";
pub const NO_CWD: &str = "Tandem cannot write a relative path because this session's directory is unknown.";
pub const WRITE_NOT_RELATIVE: &str = "Codecierge can only write a relative file name inside this session's directory.";
pub const WRITE_PARENT: &str = "Codecierge cannot write outside this session's directory: \u{201c}..\u{201d} is not allowed.";
pub const WRITE_DEVICE: &str = "Codecierge cannot write to a device name such as CON or NUL.";
pub const WRITE_STREAM: &str = "Codecierge cannot write a file name that contains \u{201c}:\u{201d}.";
pub const WRITE_INVALID: &str = "Codecierge cannot write a file name with characters Windows does not allow.";
pub const WRITE_BAD_CWD: &str = "Codecierge cannot write here because this session's directory is not a folder on this computer.";
pub const WRITE_ESCAPES: &str = "Codecierge cannot write there: the path leads outside this session's directory.";
pub const WRITE_LINK: &str = "Codecierge cannot replace a link. Remove it or choose another name.";
pub const WRITE_HARDLINK: &str = "Codecierge cannot replace a file that has other hard links. Remove it or choose another name.";
/// 12c's text for an empty name, shared so the two writers answer alike.
const EMPTY_NAME: &str = "Error: the file name is empty.";

/// The context line of the system prompt (`CG:161-179`).
pub fn context(shell: Option<&str>, uname: Option<&str>) -> String {
    match (shell, uname) {
        (Some(s), Some(u)) => format!("The shell is {s} and the system's `uname` is {u}. "),
        (Some(s), None) => format!("The shell is {s}. "),
        (None, Some(u)) => format!("The system's `uname` is {u}. "),
        (None, None) => String::new(),
    }
}

/// The system message: `$GOAL`, then `$CONTEXT`, replaced (`CG:72-77`).
pub fn system_prompt(template: &str, goal: &str, context: &str) -> String {
    template.replace("$GOAL", goal).replace("$CONTEXT", context)
}

/// A finished command as the model is told about it (`CG:51-56`, `Command`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fed {
    pub user: Option<String>,
    pub host: Option<String>,
    pub directory: Option<String>,
    pub command: String,
    pub output: String,
    pub code: i32,
}

/// `user@host:directory%` as iTerm2 builds it (`CG:342-355`): the user and host only together,
/// the directory after a colon when something precedes it.
pub fn prompt_prefix(f: &Fed) -> String {
    let mut result = String::new();
    if let (Some(host), Some(user)) = (&f.host, &f.user) {
        result = format!("{user}@{host}");
    }
    if let Some(dir) = &f.directory {
        if result.is_empty() {
            result = dir.clone();
        } else {
            result.push(':');
            result.push_str(dir);
        }
    }
    result + "%"
}

/// The message a finished command becomes (`CG:357-370`).
pub fn feed(f: &Fed, auto: bool) -> String {
    let mut lines = vec![format!("{} {}", prompt_prefix(f), f.command), f.output.clone(), "-- end output --".to_string()];
    if f.code != 0 {
        lines.push(format!("The command failed with exit code {}.", f.code));
    }
    lines.push(if auto {
        "Briefly explain the output, especially if anything went wrong, and perform the next step.".to_string()
    } else {
        "Briefly explain the output, especially if anything went wrong, and suggest the next step. If there is another command to run, please state it.".to_string()
    });
    lines.join("\n")
}

/// `execute` and `write`, offered only when commands run automatically (`CG:80-146`).
pub fn functions() -> Vec<crate::ai_turn::ToolDecl> {
    use serde_json::json;
    vec![
        crate::ai_turn::ToolDecl {
            name: "execute".into(),
            description: "Execute a command at the shell. The output will be provided in a subsequent message from me.".into(),
            parameters: json!({"type": "object", "properties": {"command": {"type": "string", "description": "The command to run"}}, "required": ["command"]}),
        },
        crate::ai_turn::ToolDecl {
            name: "write".into(),
            description: "Write a file, replacing an existing file with the same name.".into(),
            parameters: json!({"type": "object", "properties": {"filename": {"type": "string", "description": "The filename to write to. It may be relative or absolute."}, "contents": {"type": "string", "description": "The contents of the file to be written."}}, "required": ["filename", "contents"]}),
        },
    ]
}

/// What the transcript says while a command runs (`CG:106`).
pub fn running(command: &str) -> String {
    format!("Running `{}`\n", command.replace('`', "\\`"))
}

/// What the transcript says while a file is written (`CG:140`).
pub fn writing(filename: &str) -> String {
    format!("Writing to {filename}\n")
}

pub fn error_text(e: &str) -> String {
    format!("There was an error: {e}")
}

/// What a finished run does to its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reaction {
    /// The gate refused the send, or a key was declined: the session goes back to onboarding
    /// (`CG:476-480`) instead of showing an error.
    StopToOnboarding,
    ShowError(String),
}

/// The text a request that is not sent answers (`ai_runs::NOT_ENABLED`, `ATO:478-492`).
const NOT_ENABLED: &str = "AI features are not enabled or the API key is missing.";

pub fn done_reaction(failure: &str) -> Reaction {
    if failure == NOT_ENABLED {
        return Reaction::StopToOnboarding;
    }
    Reaction::ShowError(error_text(failure))
}

/// The sessions a closed gate stops (`CG:476-480`): every one not already in onboarding.
pub fn to_stop<'a>(sessions: impl IntoIterator<Item = &'a Session>, gate_open: bool) -> Vec<String> {
    if gate_open {
        return Vec::new();
    }
    sessions.into_iter().filter(|s| s.stage != Stage::Onboarding).map(|s| s.guid.clone()).collect()
}

/// Whether the usage warning is due (`CG:233-266`).
pub fn usage_due(count: u32, limit: i64) -> bool {
    i64::from(count) >= limit
}

/// What `Session::stop` took: the run to cancel and the call to answer as canceled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stopped {
    pub cancel: Option<u64>,
    pub answer: Option<(u64, String)>,
}

impl Session {
    /// A pending call is answered with `text` (a feed, "Done" or a refusal): the exchange that
    /// led to it, the model's call and the result join the conversation in that order, and the
    /// final answer keeps the request's own append flag (`ToolCodecierge.swift`: the answer goes
    /// to the closure of the request that started the call).
    pub fn answer_call(&mut self, text: &str) {
        let append = self.sent.as_ref().is_some_and(|(_, a)| *a);
        if let Some((prev, _)) = self.sent.take() {
            if !prev.is_empty() {
                self.messages.push(crate::ai_msg::Message::text(crate::ai_msg::Role::User, &prev));
            }
        }
        if let Some(call) = self.call.take() {
            use crate::ai_msg::{Body, Message, Role};
            let (name, id) = match &call.body {
                Body::FunctionCall(c, id) => (c.name.clone().unwrap_or_default(), id.clone().or_else(|| c.id.clone().map(|i| crate::ai_msg::CallId { call_id: i.clone(), item_id: i }))),
                _ => (String::new(), None),
            };
            self.messages.push(call);
            self.messages.push(Message::new(Some(Role::Function), Body::FunctionOutput { name, output: text.to_string(), id }));
            // The result is in the conversation already: nothing more to add as a user text.
            self.sent = Some((String::new(), append));
        } else {
            self.sent = Some((text.to_string(), append));
        }
    }

    /// Ends the conversation's work (ruling R12e-3): the run in flight, the pending call and the
    /// queue are taken, the session is idle and in stage `to`.
    pub fn stop(&mut self, to: Stage) -> Stopped {
        let stopped = Stopped { cancel: self.request.take(), answer: self.pending.take() };
        self.pending_done = false;
        self.sent = None;
        self.call = None;
        self.queue.clear();
        self.busy = false;
        self.stage = to;
        stopped
    }
}

/// What a function call of the model does (ruling R12e-3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invoke {
    /// Nothing is typed, written or answered.
    Drop,
    Type(Vec<String>),
    Write(WritePlan),
    Answer(String),
}

/// A command or file text as typed input: line ends are Enter, other C0 controls are removed so
/// the model cannot send ^C, ^D or an escape sequence (ruling R12e-4).
fn typed_text(s: &str) -> String {
    let job = crate::paste::Job::new(s, crate::paste::NEWLINES | crate::paste::CONTROLS);
    let mut t = crate::paste::sanitize(&job, &crate::paste::Prefs::default());
    t.retain(|c| !matches!(c, '\u{c}' | '\u{7f}' | '\u{80}'..='\u{9f}'));
    t
}

/// Decides what a call of run `id` does. Only a Running, auto-run session whose current request
/// it is, with the gate open, acts at all.
#[allow(clippy::too_many_arguments)]
pub fn invoke_plan(s: &Session, id: u64, gate_open: bool, name: &str, arguments: &str, shell: crate::ai_shell::ShellKind, local: bool, cwd: Option<&str>, home: &Path) -> Invoke {
    if s.stage != Stage::Running || !gate_open || s.request != Some(id) {
        return Invoke::Drop;
    }
    // Auto-run was turned off while this request was out: the call is answered, never acted on,
    // so the run does not wait for a command that never comes.
    if !s.auto {
        return Invoke::Answer(AUTO_OFF.to_string());
    }
    let args: serde_json::Value = match serde_json::from_str(if arguments.trim().is_empty() { "{}" } else { arguments }) {
        Ok(v) => v,
        Err(e) => return Invoke::Answer(format!("Invalid arguments for {name}: {e}")),
    };
    let text = |key: &str| args.get(key).and_then(|v| v.as_str()).map(str::to_string).ok_or_else(|| format!("Invalid arguments for {name}: missing {key}"));
    match name {
        "execute" => match text("command") {
            Ok(c) => Invoke::Type(vec![format!("{}\r", typed_text(c.trim_end_matches(['\r', '\n'])))]),
            Err(e) => Invoke::Answer(e),
        },
        "write" => match (text("filename"), text("contents")) {
            (Ok(f), Ok(c)) => Invoke::Write(write_plan(shell, local, cwd, &f, &c, home)),
            (Err(e), _) | (_, Err(e)) => Invoke::Answer(e),
        },
        _ => Invoke::Answer(crate::ai_turn::UNKNOWN_FUNCTION.to_string()),
    }
}

// ---- Writing a file (rulings R12e-2, R12e-4, R12e-6, R12e-7) ----

/// How a `write` call is carried out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WritePlan {
    /// Tandem writes the file itself: `relative` under `cwd`, UTF-8, a final newline.
    Direct { cwd: PathBuf, relative: PathBuf, contents: String },
    /// The parts are typed at a POSIX shell: `cat >`, the contents, ^D.
    Typed(Vec<String>),
    Refuse(String),
}

/// Whether the session's shell runs on this computer (ruling R12e-6): the shell integration's
/// host is local and no `ssh` is among the foreground job's ancestors.
pub fn session_is_local(host_is_local: bool, ancestors: &[String]) -> bool {
    host_is_local && !ancestors.iter().any(|a| matches!(a.trim().to_ascii_lowercase().as_str(), "ssh" | "ssh.exe"))
}

/// A name a model gave, as a path relative to the session folder (ruling R12e-2), or why not.
pub fn relative_name(filename: &str) -> Result<PathBuf, String> {
    if filename.trim().is_empty() {
        return Err(EMPTY_NAME.to_string());
    }
    let is_sep = |c: char| c == '\\' || c == '/';
    let b = filename.as_bytes();
    // Rooted, UNC, `\\?\`, `\\.\`, `\??\` and `D:\x` are all outside the folder.
    if filename.starts_with(is_sep) || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && is_sep(b[2] as char)) {
        return Err(WRITE_NOT_RELATIVE.to_string());
    }
    if filename.contains(':') {
        return Err(WRITE_STREAM.to_string());
    }
    if filename.ends_with(is_sep) || filename.chars().any(|c| (c as u32) < 0x20 || matches!(c, '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(WRITE_INVALID.to_string());
    }
    let mut out = PathBuf::new();
    for part in filename.split(is_sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part.chars().all(|c| c == '.' || c == ' ') {
            return Err(WRITE_PARENT.to_string());
        }
        if crate::inert_files::is_device_name(part) {
            return Err(WRITE_DEVICE.to_string());
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return Err(WRITE_INVALID.to_string());
    }
    Ok(out)
}

/// The session folder: only an absolute local drive path (ruling R12e-2).
pub fn local_cwd(cwd: Option<&str>) -> Result<PathBuf, String> {
    let Some(c) = cwd.filter(|c| !c.is_empty()) else { return Err(NO_CWD.to_string()) };
    let b = c.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    if !drive || c[2..].contains(':') || c.chars().any(|ch| (ch as u32) < 0x20 || matches!(ch, '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(WRITE_BAD_CWD.to_string());
    }
    Ok(PathBuf::from(c.replace('/', "\\")))
}

/// The session folder and the relative name for a direct write.
pub fn direct_path(filename: &str, cwd: Option<&str>, home: &Path) -> Result<(PathBuf, PathBuf), String> {
    let relative = relative_name(filename)?;
    let cwd_path = local_cwd(cwd)?;
    // One join and one refusal rule for every model-named writer (`ai_actions::file_target`, which
    // runs `inert_files::refused_path`): a listing type or a `desktop.ini` never writes.
    match crate::ai_actions::file_target(&relative.to_string_lossy(), Some(&cwd_path.to_string_lossy()), true, home) {
        crate::ai_actions::FileTarget::Refused(why) => Err(why),
        crate::ai_actions::FileTarget::Local(p) if p.starts_with(&cwd_path) => Ok((cwd_path, relative)),
        crate::ai_actions::FileTarget::Local(_) => Err(WRITE_ESCAPES.to_string()),
    }
}

/// The components of a path, lower case, with a verbatim drive prefix read as the drive.
fn parts(p: &Path) -> Vec<String> {
    let text = p.to_string_lossy().replace('/', "\\");
    let text = text.strip_prefix(r"\\?\").filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(&text);
    text.split('\\').filter(|c| !c.is_empty()).map(str::to_lowercase).collect()
}

/// Whether `target` lies under `canonical_cwd`, component by component and ignoring case as NTFS
/// does: a prefix of the text is not a parent.
pub fn contained(canonical_cwd: &Path, canonical_target: &Path) -> bool {
    let (c, t) = (parts(canonical_cwd), parts(canonical_target));
    t.len() >= c.len() && t[..c.len()] == c[..]
}

/// What the opened handle says decides (ruling R12e-7): `Err((text, delete))`, `delete` when
/// this open created the file and it must be removed.
pub fn after_open(canonical_cwd: &Path, final_path: &Path, created: bool, reparse: bool, links: u32) -> Result<(), (String, bool)> {
    if reparse {
        return Err((WRITE_LINK.to_string(), created));
    }
    if links > 1 && !created {
        return Err((WRITE_HARDLINK.to_string(), false));
    }
    if !contained(canonical_cwd, final_path) {
        return Err((WRITE_ESCAPES.to_string(), created));
    }
    Ok(())
}

/// Writes `contents` to `relative` under `cwd` (ruling R12e-2): the folder and the existing
/// parent are canonicalized and the parent must lie inside, as a first and friendly refusal;
/// then `open_checked` decides on the handle it really opened. Parent folders are not created.
pub fn write_direct(cwd: &Path, relative: &Path, contents: &str) -> Result<(), String> {
    let canonical_cwd = std::fs::canonicalize(cwd).map_err(|_| WRITE_BAD_CWD.to_string())?;
    let full = cwd.join(relative);
    let (Some(parent), Some(name)) = (full.parent(), full.file_name()) else { return Err(WRITE_INVALID.to_string()) };
    let canonical_parent = std::fs::canonicalize(parent).map_err(|e| crate::ai_actions::create_failed(&e.to_string()))?;
    if !contained(&canonical_cwd, &canonical_parent) {
        return Err(WRITE_ESCAPES.to_string());
    }
    let target = canonical_parent.join(name);
    if reparse_point(&target) {
        return Err(WRITE_LINK.to_string());
    }
    open_checked(&canonical_cwd, &target, contents)
}

/// Whether `path` itself (not what it points to) is a link or a junction.
fn reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }
}

/// The handle first, the check second, the write last (ruling R12e-7).
#[cfg(windows)]
pub fn open_checked(canonical_cwd: &Path, full: &Path, contents: &str) -> Result<(), String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW, SetEndOfFile, SetFileInformationByHandle, SetFilePointerEx, WriteFile, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BEGIN,
        FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, OPEN_ALWAYS, VOLUME_NAME_DOS,
    };
    let wide: Vec<u16> = full.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path; the handle is closed on every path below.
    unsafe {
        let h = CreateFileW(wide.as_ptr(), GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES, FILE_SHARE_READ, std::ptr::null(), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut());
        if h == INVALID_HANDLE_VALUE {
            return Err(crate::ai_actions::create_failed(&std::io::Error::last_os_error().to_string()));
        }
        let created = GetLastError() != ERROR_ALREADY_EXISTS;
        let fail = |text: String, delete: bool| -> Result<(), String> {
            if delete {
                let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
                SetFileInformationByHandle(h, FileDispositionInfo, &info as *const _ as *const _, std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32);
            }
            CloseHandle(h);
            Err(text)
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(h, &mut info) == 0 {
            return fail(crate::ai_actions::create_failed(&std::io::Error::last_os_error().to_string()), created);
        }
        let mut buf = vec![0u16; 1024];
        let mut n = GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
        if n as usize >= buf.len() {
            buf = vec![0u16; n as usize + 1];
            n = GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
        }
        if n == 0 || n as usize >= buf.len() {
            return fail(WRITE_ESCAPES.to_string(), created);
        }
        let final_path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n as usize]));
        if let Err((text, delete)) = after_open(canonical_cwd, &final_path, created, info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0, info.nNumberOfLinks) {
            return fail(text, delete);
        }
        // Checked: now, and only now, the file is cut to length and written.
        let mut ok = SetFilePointerEx(h, 0, std::ptr::null_mut(), FILE_BEGIN) != 0 && SetEndOfFile(h) != 0;
        let bytes = contents.as_bytes();
        let mut done = 0usize;
        while ok && done < bytes.len() {
            let chunk = (bytes.len() - done).min(1 << 20) as u32;
            let mut wrote = 0u32;
            ok = WriteFile(h, bytes[done..].as_ptr(), chunk, &mut wrote, std::ptr::null_mut()) != 0 && wrote > 0;
            done += wrote as usize;
        }
        if !ok {
            let e = std::io::Error::last_os_error().to_string();
            CloseHandle(h);
            return Err(crate::ai_actions::create_failed(&e));
        }
        CloseHandle(h);
        // The Mark of the Web on the file the handle really named, as every model-named writer
        // does (`inert_files`, ruling P15).
        crate::inert_files::mark(&final_path);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn open_checked(_canonical_cwd: &Path, _full: &Path, _contents: &str) -> Result<(), String> {
    Err(WRITE_BAD_CWD.to_string())
}

/// POSIX single quotes, `'` as `'\''`, always quoted.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// How a `write` call is carried out in a session (rulings R12e-2, R12e-4, R12e-6).
pub fn write_plan(shell: crate::ai_shell::ShellKind, local: bool, cwd: Option<&str>, filename: &str, contents: &str, home: &Path) -> WritePlan {
    use crate::ai_shell::ShellKind;
    match shell {
        ShellKind::Unknown => WritePlan::Refuse(UNKNOWN_SHELL.to_string()),
        ShellKind::Cmd | ShellKind::PowerShell if local => match direct_path(filename, cwd, home) {
            Ok((cwd, relative)) => WritePlan::Direct { cwd, relative, contents: with_final_newline(contents) },
            Err(e) => WritePlan::Refuse(e),
        },
        _ => {
            if filename.trim().is_empty() {
                return WritePlan::Refuse(EMPTY_NAME.to_string());
            }
            if filename.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
                return WritePlan::Refuse(WRITE_INVALID.to_string());
            }
            let mut body = typed_text(contents);
            if !body.ends_with('\r') {
                body.push('\r');
            }
            WritePlan::Typed(vec![format!("cat > {}\r", sh_quote(filename)), body, "\u{4}".to_string()])
        }
    }
}

fn with_final_newline(contents: &str) -> String {
    if contents.ends_with('\n') { contents.to_string() } else { format!("{contents}\n") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gate::{check_endpoint, Check};
    use crate::ai_shell::ShellKind;
    use crate::ai_models::Endpoint;
    use crate::ai_settings::{Api, Vendor, ADMIN, ENABLE_AI};
    use crate::globals::Globals;
    use std::path::{Path, PathBuf};
    use toml::Value;

    #[test]
    fn codecierge_stages_follow_iterm2() {
        assert_eq!(initial_stage(true, true), Stage::GoalSetting);
        assert_eq!(initial_stage(true, false), Stage::Onboarding);
        assert_eq!(initial_stage(false, true), Stage::Onboarding);

        assert_eq!(onboarding(false, false), CodeciergeState::AdminDisabled);
        assert_eq!(onboarding(false, true), CodeciergeState::AdminDisabled);
        assert_eq!(onboarding(true, false), CodeciergeState::NeedsAi);
        assert_eq!(onboarding(true, true), CodeciergeState::Ready);
        assert_eq!(codecierge_text(CodeciergeState::AdminDisabled), "Generative AI has been disabled by the system administrator.");
        assert_eq!(codecierge_text(CodeciergeState::NeedsAi), "You must enable AI features to use Codecierge.");
        assert_eq!(codecierge_text(CodeciergeState::Ready), "Codecierge uses AI to help you in your terminal.");

        let s = Session::new("G", true, true);
        assert_eq!((s.stage, s.auto, s.auto_box), (Stage::GoalSetting, true, false));
        assert!(!start_enabled(""));
        assert!(start_enabled("x"));
        assert!(!auto_enabled(false));
        assert!(auto_enabled(true));

        let mut s = Session::new("G", true, true);
        s.goal = "deploy".into();
        assert_eq!(view(&s, true, true, false), View::Goal { goal: "deploy".into(), auto_box: false, auto_enabled: false, start_enabled: true });
        s.stage = Stage::Running;
        s.transcript = "t".into();
        match view(&s, true, true, true) {
            View::Running { goal, transcript, busy, reply } => assert_eq!((goal.as_str(), transcript.as_str(), busy, reply.as_str()), ("Goal: deploy", "t", false, "")),
            v => panic!("{v:?}"),
        }
        assert_eq!(goal_label("deploy"), "Goal: deploy");
        s.stage = Stage::Onboarding;
        assert_eq!(view(&s, true, false, true), View::Onboarding(CodeciergeState::NeedsAi));
    }

    /// A goal-setting session with a goal, as the person sees it before Start.
    fn goal_session() -> Session {
        let mut s = Session::new("G", true, true);
        s.goal = "deploy".into();
        s
    }

    /// Start as `App::codecierge_start` does it, with the model calling functions.
    fn start(s: &mut Session, id: u64) {
        s.begin_auto(true);
        s.stage = Stage::Running;
        s.request = Some(id);
    }

    fn execute(s: &Session, id: u64) -> Invoke {
        invoke_plan(s, id, true, "execute", "{\"command\":\"rm -rf ~\"}", ShellKind::Posix, true, None, home())
    }

    #[test]
    fn danger_tick_then_start_runs_without_auto() {
        // Bypass 1: tick the box, then Start before Danger! is answered.
        let mut s = goal_session();
        assert!(s.box_clicked(), "ticking asks Danger!");
        assert_eq!(view(&s, true, true, true), View::Goal { goal: "deploy".into(), auto_box: false, auto_enabled: true, start_enabled: true }, "the box shows unticked while Danger! is pending");
        start(&mut s, 7);
        assert!(!s.auto, "Start before the answer runs without auto-run");
        assert_eq!(execute(&s, 7), Invoke::Answer(AUTO_OFF.into()), "nothing is typed");
    }

    #[test]
    fn danger_ok_after_start_leaves_the_running_session_alone() {
        // Bypass 2: Start first, then OK: the running session keeps no auto-run; the box is
        // ticked for the next Start.
        let mut s = goal_session();
        assert!(s.box_clicked());
        start(&mut s, 7);
        s.danger_answered(0);
        assert!(!s.auto, "OK after Start does not arm the running session");
        assert!(s.auto_box, "OK ticks the box");
        assert_eq!(execute(&s, 7), Invoke::Answer(AUTO_OFF.into()));
    }

    #[test]
    fn danger_cancel_after_start_turns_auto_off_at_once() {
        // Bypass 3: auto-run on after OK, then a Cancel (or a close) arrives while it runs.
        let close = crate::prompt_ui::close_index_with(&DANGER_BUTTONS, Some(DANGER_CLOSE));
        assert_eq!(close, 1, "a close and Escape answer Cancel");
        for refusal in [1, close] {
            let mut s = goal_session();
            assert!(s.box_clicked());
            s.danger_answered(0);
            start(&mut s, 7);
            assert!(s.auto, "auto-run after OK");
            assert_eq!(execute(&s, 7), Invoke::Type(vec!["rm -rf ~\r".into()]));
            s.danger_answered(refusal);
            assert!(!s.auto && !s.auto_box, "Cancel turns auto-run off for the running session");
            assert_eq!(execute(&s, 7), Invoke::Answer(AUTO_OFF.into()), "a later call types nothing");
        }
    }

    #[test]
    fn danger_box_and_start_follow_the_rulings() {
        // Unticking is at once and asks nothing.
        let mut s = goal_session();
        assert!(s.box_clicked());
        s.danger_answered(0);
        assert!(s.auto_box);
        assert!(!s.box_clicked(), "unticking asks nothing");
        assert!(!s.auto_box);
        // Start needs function calling too (Task 6 minor).
        let mut s = goal_session();
        assert!(s.box_clicked());
        s.danger_answered(0);
        s.begin_auto(false);
        assert!(!s.auto, "no function calling, no auto-run");
        s.begin_auto(true);
        assert!(s.auto);
        // The Privacy Warning: only OK starts; Cancel, a close and Escape do not.
        assert!(privacy_starts(0));
        assert!(!privacy_starts(1));
        assert!(!privacy_starts(crate::prompt_ui::close_index_with(&["OK", "Cancel"], None)));
    }

    #[test]
    fn session_debug_prints_no_text() {
        let mut s = Session::new("G1", true, true);
        s.goal = "secret goal".into();
        s.transcript = "secret transcript".into();
        s.reply = "secret reply".into();
        let d = format!("{s:?}");
        assert!(d.contains("G1") && d.contains("GoalSetting"), "{d}");
        assert!(!d.contains("secret"), "{d}");
    }

    #[test]
    fn codecierge_transcript_follows_iterm2() {
        assert_eq!(assistant("Run ls"), "### Assistant\nRun ls");
        assert_eq!(assistant_more("Done"), "\n\n### Assistant\nDone");
        assert_eq!(you("why?"), "\n\n### You\nwhy?\n");
        assert_eq!(terminal("echo `x`"), "\n\n### Terminal\n`echo \\`x\\``");
        let long = "a".repeat(100);
        let want = format!("\n\n### Terminal\n`{}\u{2026}`", "a".repeat(79));
        assert_eq!(terminal(&long), want);
        assert_eq!(terminal(&"a".repeat(80)), format!("\n\n### Terminal\n`{}`", "a".repeat(80)));
        assert_eq!(ASSISTANT_THINKING, "\n\n### Assistant\nThinking\u{2026}");
    }

    #[test]
    fn get_started_is_gated() {
        assert_eq!(get_started(false, true), GetStarted::Stay);
        assert_eq!(get_started(false, false), GetStarted::Stay);
        assert_eq!(get_started(true, false), GetStarted::AskKey);
        assert_eq!(get_started(true, true), GetStarted::Goal);

        // AI on, admin on, a key stored, but no consent for the host: the gate is closed, so the
        // first stage is onboarding and Get Started stays.
        let mut g = Globals::empty(Path::new("cc-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(true));
        g.set(ADMIN, Value::Boolean(true));
        let ep = Endpoint { vendor: Vendor::OpenAI, model: "m".into(), url: "https://api.openai.com/v1/responses".into(), api: Api::Responses, custom_headers: vec![] };
        let open = check_endpoint(&g, Some(&ep)) == Check::Pass;
        assert!(!open);
        assert_eq!(initial_stage(open, true), Stage::Onboarding);
        assert_eq!(get_started(open, true), GetStarted::Stay);
        crate::ai_gate::grant(&mut g, "api.openai.com");
        let open = check_endpoint(&g, Some(&ep)) == Check::Pass;
        assert_eq!(initial_stage(open, true), Stage::GoalSetting);
    }

    fn home() -> &'static Path {
        Path::new(r"C:\Users\u")
    }

    fn running_session(request: Option<u64>) -> Session {
        let mut s = Session::new("G", true, true);
        s.stage = Stage::Running;
        s.auto = true;
        s.request = request;
        s
    }

    #[test]
    fn codecierge_prompts_follow_iterm2() {
        assert_eq!(context(Some("pwsh"), Some("Windows_NT")), "The shell is pwsh and the system's `uname` is Windows_NT. ");
        assert_eq!(context(Some("bash"), None), "The shell is bash. ");
        assert_eq!(context(None, Some("Linux")), "The system's `uname` is Linux. ");
        assert_eq!(context(None, None), "");
        assert_eq!(
            system_prompt(REGULAR_DEFAULT, "deploy the app", "The shell is pwsh. "),
            "You help a me in a terminal emulator. My goal is deploy the app. The shell is pwsh. . Start by suggesting a command. Don't overwhelm me with too much information: just go one step at a time. When I've reached my goal, remind me to click the End Task button."
        );
        assert_eq!(
            system_prompt(GHOST_RIDING_DEFAULT, "g", "c"),
            "You operate a terminal emulator for me. My goal is g. c. Ask clarifying questions as needed and run commands using the execute function when you're ready. When I've reached my goal, remind me to click the End Task button."
        );
        assert_eq!((GHOST_RIDING_KEY, REGULAR_KEY, WARNING_COUNT_KEY), ("codeciergeGhostRidingPrompt", "codeciergeRegularPrompt", "codeciergeCommandWarningCount"));
        assert_eq!((GET_STARTED_MESSAGE, SESSION_GONE), ("Tell me how to get started.", "The session no longer exists"));

        let f = functions();
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].name.as_str(), f[0].description.as_str()), ("execute", "Execute a command at the shell. The output will be provided in a subsequent message from me."));
        assert_eq!(f[0].parameters, serde_json::json!({"type": "object", "properties": {"command": {"type": "string", "description": "The command to run"}}, "required": ["command"]}));
        assert_eq!((f[1].name.as_str(), f[1].description.as_str()), ("write", "Write a file, replacing an existing file with the same name."));
        assert_eq!(
            f[1].parameters,
            serde_json::json!({"type": "object", "properties": {"filename": {"type": "string", "description": "The filename to write to. It may be relative or absolute."}, "contents": {"type": "string", "description": "The contents of the file to be written."}}, "required": ["filename", "contents"]})
        );
    }

    #[test]
    fn default_prompts_match_the_settings_rows() {
        for (key, default) in [(GHOST_RIDING_KEY, GHOST_RIDING_DEFAULT), (REGULAR_KEY, REGULAR_DEFAULT)] {
            let row = crate::schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(row.default, Some(toml::Value::String(default.to_string())), "{key}");
        }
    }

    fn fed(user: Option<&str>, host: Option<&str>, dir: Option<&str>, code: i32) -> Fed {
        Fed { user: user.map(String::from), host: host.map(String::from), directory: dir.map(String::from), command: "dir".into(), output: "a\nb".into(), code }
    }

    #[test]
    fn feed_follows_iterm2() {
        let ok = fed(Some("u"), Some("h"), Some("C:\\src"), 0);
        assert_eq!(feed(&ok, false), "u@h:C:\\src% dir\na\nb\n-- end output --\nBriefly explain the output, especially if anything went wrong, and suggest the next step. If there is another command to run, please state it.");
        let bad = fed(Some("u"), Some("h"), Some("C:\\src"), 2);
        assert_eq!(feed(&bad, true), "u@h:C:\\src% dir\na\nb\n-- end output --\nThe command failed with exit code 2.\nBriefly explain the output, especially if anything went wrong, and perform the next step.");
        assert_eq!(prompt_prefix(&fed(None, Some("h"), Some("C:\\src"), 0)), "C:\\src%");
        assert_eq!(prompt_prefix(&fed(Some("u"), None, Some("C:\\src"), 0)), "C:\\src%");
        assert_eq!(prompt_prefix(&fed(None, None, None, 0)), "%");
        assert_eq!(prompt_prefix(&fed(Some("u"), Some("h"), None, 0)), "u@h%");
        assert_eq!(running("ls `x`"), "Running `ls \\`x\\``\n");
        assert_eq!(writing("a.txt"), "Writing to a.txt\n");
        assert_eq!(error_text("timeout"), "There was an error: timeout");
    }

    #[test]
    fn write_plan_follows_windows() {
        let direct = |relative: &str| WritePlan::Direct { cwd: PathBuf::from(r"C:\src"), relative: PathBuf::from(relative), contents: "hi\n".into() };
        for kind in [ShellKind::PowerShell, ShellKind::Cmd] {
            assert_eq!(write_plan(kind, true, Some(r"C:\src"), r"notes\a.txt", "hi", home()), direct(r"notes\a.txt"));
            assert_eq!(write_plan(kind, true, Some(r"C:\src"), "notes/a.txt", "hi", home()), direct(r"notes\a.txt"));
            assert_eq!(write_plan(kind, true, Some(r"C:\src"), r".\a.txt", "hi", home()), direct("a.txt"));
            assert_eq!(write_plan(kind, true, Some(r"C:\src"), "a.txt", "hi\n", home()), direct("a.txt"));
            assert_eq!(write_plan(kind, true, None, "a.txt", "hi", home()), WritePlan::Refuse(NO_CWD.into()));
        }
        for local in [true, false] {
            assert_eq!(write_plan(ShellKind::Unknown, local, Some(r"C:\src"), "a.txt", "hi", home()), WritePlan::Refuse(UNKNOWN_SHELL.into()));
        }
    }

    #[test]
    fn write_refusals_follow_tandem() {
        let refused = |name: &str| write_plan(ShellKind::PowerShell, true, Some(r"C:\src"), name, "x", home());
        for kind in [ShellKind::PowerShell, ShellKind::Cmd] {
            let refused = |name: &str| write_plan(kind, true, Some(r"C:\src"), name, "x", home());
            for (names, text) in [
                (&[r"..\x", "../x", r"a\..\..\x", r"a\..\b", r"...\x", r".. \x"][..], WRITE_PARENT),
                (&[r"D:\x.txt", r"C:\src\x.txt", r"\x", "/x", r"\\h\s\x", "//h/s/x", r"\\?\C:\x", r"\\.\PhysicalDrive0", r"\??\C:\x"][..], WRITE_NOT_RELATIVE),
                (&["C:x", "a.txt:s", "a.txt::$DATA", r"a:b\c.txt"][..], WRITE_STREAM),
                (&["CON", "con", "nul.txt", r"sub\aux", "COM1", "com9.log", "LPT1", "PRN.", "CONIN$", "CONOUT$", "COM\u{b9}", "nul .txt"][..], WRITE_DEVICE),
                (&["a*.txt", "a?.txt", "a<b", "a|b", "a\"b", "a\u{1}b", "a.txt\n", r"notes\"][..], WRITE_INVALID),
            ] {
                for name in names {
                    assert_eq!(refused(name), WritePlan::Refuse(text.into()), "{name:?}");
                }
            }
            assert_eq!(refused(""), WritePlan::Refuse("Error: the file name is empty.".into()));
        }
        // The shell lists these types without opening them (`inert_files`), so they never write.
        for name in ["x.url", "a.lnk", "desktop.ini", r"sub\a.scf", "a.theme"] {
            assert!(matches!(refused(name), WritePlan::Refuse(_)), "{name}");
        }
        assert_eq!(refused("x.url"), WritePlan::Refuse("Error: files of this type are not allowed.".into()));
        for cwd in [r"\\h\s", "//h/s", r"\\?\C:\src", r"\\.\C:\src", "src", "C:src", "/home/u"] {
            assert_eq!(write_plan(ShellKind::PowerShell, true, Some(cwd), "a.txt", "x", home()), WritePlan::Refuse(WRITE_BAD_CWD.into()), "{cwd:?}");
        }
        assert!(!contained(Path::new(r"C:\src"), Path::new(r"C:\src2\a.txt")));
        assert!(contained(Path::new(r"C:\src"), Path::new(r"C:\SRC\a.txt")));
        assert!(!contained(Path::new(r"C:\src"), Path::new(r"C:\a.txt")));
        assert!(contained(Path::new(r"\\?\C:\src"), Path::new(r"C:\src\sub\a.txt")));
    }

    #[test]
    fn after_open_decides_from_the_handle() {
        let s = r"C:\s";
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\s\a.txt"), true, false, 1), Ok(()));
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\out\a.txt"), true, false, 1), Err((WRITE_ESCAPES.to_string(), true)));
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\out\a.txt"), false, false, 1), Err((WRITE_ESCAPES.to_string(), false)));
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\s\a.txt"), true, true, 1), Err((WRITE_LINK.to_string(), true)));
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\s\a.txt"), false, true, 1), Err((WRITE_LINK.to_string(), false)));
        assert_eq!(after_open(Path::new(s), Path::new(r"C:\s\a.txt"), false, false, 2), Err((WRITE_HARDLINK.to_string(), false)));
    }

    #[cfg(windows)]
    mod disk {
        use super::super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT: AtomicUsize = AtomicUsize::new(0);

        /// A fresh folder with `s` and `out` in it.
        pub fn scratch() -> (PathBuf, PathBuf, PathBuf) {
            let t = std::env::temp_dir().join(format!("tandem-cc-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
            let _ = std::fs::remove_dir_all(&t);
            std::fs::create_dir_all(t.join("s")).unwrap();
            std::fs::create_dir_all(t.join("out")).unwrap();
            (t.clone(), t.join("s"), t.join("out"))
        }

        pub fn junction(link: &Path, target: &Path) {
            let r = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(r.status.success(), "mklink: {}", String::from_utf8_lossy(&r.stdout));
        }

        /// Removes junctions first so the delete never reaches through one.
        pub fn clean(t: &Path) {
            for sub in [t.join("s"), t.join("out")] {
                if let Ok(rd) = std::fs::read_dir(&sub) {
                    for e in rd.flatten() {
                        if std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.file_type().is_symlink()) {
                            let _ = std::fs::remove_dir(e.path()).or_else(|_| std::fs::remove_file(e.path()));
                        }
                    }
                }
            }
            let _ = std::fs::remove_dir_all(t);
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_cannot_escape() {
        let (t, s, out) = disk::scratch();
        disk::junction(&s.join("j"), &out);
        assert_eq!(write_direct(&s, Path::new(r"j\x.txt"), "x"), Err(WRITE_ESCAPES.to_string()));
        assert!(!out.join("x.txt").exists(), "nothing reached the folder the junction names");
        assert_eq!(write_direct(&s, Path::new("a.txt"), "x"), Ok(()));
        assert_eq!(std::fs::read_to_string(s.join("a.txt")).unwrap(), "x");
        // A final junction is a link too.
        disk::junction(&s.join("l.txt"), &out);
        assert_eq!(write_direct(&s, Path::new("l.txt"), "x"), Err(WRITE_LINK.to_string()));
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        // A symlink to a file needs a privilege: skipped, and said so, when Windows refuses one.
        std::fs::write(out.join("target.txt"), "keep").unwrap();
        match std::os::windows::fs::symlink_file(out.join("target.txt"), s.join("sym.txt")) {
            Ok(()) => {
                assert_eq!(write_direct(&s, Path::new("sym.txt"), "x"), Err(WRITE_LINK.to_string()));
                assert_eq!(std::fs::read_to_string(out.join("target.txt")).unwrap(), "keep");
            }
            Err(e) => eprintln!("junction_cannot_escape: the file symlink half is skipped, Windows refused to make one: {e}"),
        }
        disk::clean(&t);
    }

    #[cfg(windows)]
    #[test]
    fn write_race_is_closed() {
        let (t, s, out) = disk::scratch();
        let canon = std::fs::canonicalize(&s).unwrap();
        disk::junction(&s.join("d"), &out);
        assert_eq!(open_checked(&canon, &s.join("d").join("x.txt"), "x"), Err(WRITE_ESCAPES.to_string()));
        assert!(!out.join("x.txt").exists(), "the file the open created is deleted by handle");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing else changed");
        std::fs::write(out.join("x.txt"), "old").unwrap();
        assert_eq!(open_checked(&canon, &s.join("d").join("x.txt"), "x"), Err(WRITE_ESCAPES.to_string()));
        assert_eq!(std::fs::read_to_string(out.join("x.txt")).unwrap(), "old", "an existing file is never truncated before the check");
        disk::clean(&t);
    }

    #[cfg(windows)]
    #[test]
    fn hard_link_is_refused() {
        let (t, s, out) = disk::scratch();
        std::fs::write(out.join("secret.txt"), "keep").unwrap();
        std::fs::hard_link(out.join("secret.txt"), s.join("h.txt")).unwrap();
        assert_eq!(write_direct(&s, Path::new("h.txt"), "x"), Err(WRITE_HARDLINK.to_string()));
        assert_eq!(std::fs::read_to_string(out.join("secret.txt")).unwrap(), "keep");
        std::fs::remove_file(s.join("h.txt")).unwrap();
        assert_eq!(write_direct(&s, Path::new("h.txt"), "x"), Ok(()));
        assert_eq!(std::fs::read_to_string(s.join("h.txt")).unwrap(), "x");
        disk::clean(&t);
    }

    #[cfg(windows)]
    #[test]
    fn direct_write_carries_the_mark_of_the_web() {
        let (t, s, _) = disk::scratch();
        assert_eq!(write_direct(&s, Path::new("m.txt"), "hello\n"), Ok(()));
        assert_eq!(std::fs::read_to_string(s.join("m.txt")).unwrap(), "hello\n");
        let stream = format!("{}:Zone.Identifier", s.join("m.txt").display());
        assert_eq!(std::fs::read_to_string(stream).unwrap(), crate::inert_files::ZONE_MARK);
        // Writing again replaces the contents and keeps the mark.
        assert_eq!(write_direct(&s, Path::new("m.txt"), "b"), Ok(()));
        assert_eq!(std::fs::read_to_string(s.join("m.txt")).unwrap(), "b");
        disk::clean(&t);
    }

    #[test]
    fn remote_session_never_writes_locally() {
        assert!(session_is_local(true, &["pwsh".into()]));
        assert!(!session_is_local(false, &["pwsh".into()]));
        assert!(!session_is_local(true, &["cmd".into(), "ssh.exe".into(), "pwsh".into()]));
        assert!(!session_is_local(true, &["ssh".into()]));
        for kind in [ShellKind::Cmd, ShellKind::PowerShell] {
            assert!(matches!(write_plan(kind, false, Some(r"C:\src"), "a.txt", "hi", home()), WritePlan::Typed(_)), "{kind:?}");
        }
        assert!(matches!(write_plan(ShellKind::Posix, false, Some("/home/u"), "a.txt", "hi", home()), WritePlan::Typed(_)));
    }

    #[test]
    fn typed_write_follows_tandem() {
        let typed = |name: &str, contents: &str| write_plan(ShellKind::Posix, true, Some("/home/u"), name, contents, home());
        let parts = |a: &str, b: &str| WritePlan::Typed(vec![a.into(), b.into(), "\u{4}".into()]);
        assert_eq!(typed("a.txt", "hi"), parts("cat > 'a.txt'\r", "hi\r"));
        assert_eq!(typed("a.txt", "hi\n"), parts("cat > 'a.txt'\r", "hi\r"));
        assert_eq!(typed("a.txt", "a\r\nb"), parts("cat > 'a.txt'\r", "a\rb\r"));
        assert_eq!(typed("a.txt", "x\u{3}y\u{4}z\u{1b}[1m"), parts("cat > 'a.txt'\r", "xyz[1m\r"));
        assert_eq!(typed("a.txt", "x\u{c}y\u{7f}z\tw"), parts("cat > 'a.txt'\r", "xyz\tw\r"));
        assert_eq!(typed("a b", "hi"), parts("cat > 'a b'\r", "hi\r"));
        assert_eq!(typed("it's", "hi"), parts("cat > 'it'\\''s'\r", "hi\r"));
        assert_eq!(typed("a; rm -rf ~", "hi"), parts("cat > 'a; rm -rf ~'\r", "hi\r"));
        assert_eq!(typed("$(id)", "hi"), parts("cat > '$(id)'\r", "hi\r"));
        assert_eq!(typed("a\nb", "hi"), WritePlan::Refuse(WRITE_INVALID.into()));
        assert_eq!(typed("a\rb", "hi"), WritePlan::Refuse(WRITE_INVALID.into()));
        assert_eq!(sh_quote(""), "''");
        // The only ^D is the last part.
        if let WritePlan::Typed(p) = typed("a", "x\u{4}y") {
            assert_eq!(p.iter().filter(|s| s.contains('\u{4}')).count(), 1);
            assert_eq!(p.last().map(String::as_str), Some("\u{4}"));
        }
    }

    fn busy_session() -> Session {
        let mut s = running_session(Some(7));
        s.pending = Some((7, "c1".into()));
        s.queue.push_back(("a".into(), false));
        s.queue.push_back(("b".into(), true));
        s.busy = true;
        s
    }

    #[test]
    fn stop_cancels_everything() {
        for to in [Stage::GoalSetting, Stage::Onboarding] {
            let mut s = busy_session();
            assert_eq!(s.stop(to), Stopped { cancel: Some(7), answer: Some((7, "c1".into())) });
            assert_eq!((s.request, s.pending.clone(), s.queue.len(), s.busy, s.stage), (None, None, 0, false, to));
            assert_eq!(s.stop(to), Stopped { cancel: None, answer: None });
        }
    }

    #[test]
    fn late_execute_never_types() {
        let exec = |s: &Session, id: u64, open: bool| invoke_plan(s, id, open, "execute", r#"{"command":"rm -rf ~"}"#, ShellKind::Posix, true, Some("/home/u"), home());
        let write = |s: &Session, id: u64, open: bool| invoke_plan(s, id, open, "write", r#"{"filename":"a.txt","contents":"x"}"#, ShellKind::Posix, true, Some("/home/u"), home());
        let s = busy_session();
        assert_eq!(exec(&s, 7, true), Invoke::Type(vec!["rm -rf ~\r".into()]));
        assert_eq!(write(&s, 7, true), Invoke::Write(WritePlan::Typed(vec!["cat > 'a.txt'\r".into(), "x\r".into(), "\u{4}".into()])));
        for to in [Stage::GoalSetting, Stage::Onboarding] {
            let mut s = busy_session();
            s.stop(to);
            assert_eq!(exec(&s, 7, true), Invoke::Drop, "after stop to {to:?}");
            assert_eq!(write(&s, 7, true), Invoke::Drop, "after stop to {to:?}");
        }
        let s = running_session(Some(8));
        assert_eq!(exec(&s, 7, true), Invoke::Drop, "an answer from a cancelled run");
        assert_eq!(write(&s, 7, true), Invoke::Drop);
        let s = busy_session();
        assert_eq!(exec(&s, 7, false), Invoke::Drop, "gate closed");
        assert_eq!(write(&s, 7, false), Invoke::Drop);
        let mut s = busy_session();
        s.auto = false;
        assert_eq!(exec(&s, 7, true), Invoke::Answer(AUTO_OFF.into()), "no auto-run: answered, nothing typed");
        assert_eq!(write(&s, 7, true), Invoke::Answer(AUTO_OFF.into()));
        // Other names and bad arguments answer the model.
        let s = busy_session();
        match invoke_plan(&s, 7, true, "nope", "{}", ShellKind::Posix, true, None, home()) {
            Invoke::Answer(t) => assert_eq!(t, crate::ai_turn::UNKNOWN_FUNCTION),
            i => panic!("{i:?}"),
        }
        assert!(matches!(invoke_plan(&s, 7, true, "execute", "{", ShellKind::Posix, true, None, home()), Invoke::Answer(_)));
        // The typed command has no unsafe control and one Enter.
        assert_eq!(invoke_plan(&s, 7, true, "execute", "{\"command\":\"ls\\u0003\\n\"}", ShellKind::Posix, true, None, home()), Invoke::Type(vec!["ls\r".into()]));
    }

    #[test]
    fn a_closed_gate_stops_every_running_session() {
        let mut a = Session::new("A", true, true);
        a.stage = Stage::Running;
        let mut b = Session::new("B", true, true);
        b.stage = Stage::GoalSetting;
        let c = Session::new("C", false, false);
        assert_eq!(c.stage, Stage::Onboarding);
        assert_eq!(to_stop([&a, &b, &c], false), ["A".to_string(), "B".to_string()]);
        assert!(to_stop([&a, &b, &c], true).is_empty());
    }

    #[test]
    fn a_refused_send_follows_the_gate() {
        // The gate's refusal and a declined key both end a run with this text.
        assert_eq!(done_reaction("AI features are not enabled or the API key is missing."), Reaction::StopToOnboarding);
        assert_eq!(done_reaction("timeout"), Reaction::ShowError("There was an error: timeout".into()));
    }

    #[test]
    fn usage_warning_follows_iterm2() {
        assert!(!usage_due(9, 10));
        assert!(usage_due(10, 10));
        assert!(usage_due(11, 10));
        assert_eq!((USAGE_HEADING, KEEP_GOING, STOP, USAGE_ID), ("Codecierge Usage Warning", "Keep Going", "Stop Codecierge", "CodeciergeCommandWarning"));
        assert_eq!(USAGE_TEXT, "Your codecierge session has been going on for a long time. Are you still using it? If not, stop it to save money and privacy.");
    }

    #[test]
    fn codecierge_warnings_follow_iterm2() {
        assert_eq!(DANGER_HEADING, "Danger!");
        assert_eq!(DANGER_TEXT, "This lets an AI completely control your computer. It could delete your files, do something stupid or dangerous, or lead to the downfall of humanity. Proceed with caution.");
        assert_eq!(PRIVACY_HEADING, "Privacy Warning");
        assert_eq!(PRIVACY_TEXT, "Everything that happens in your terminal while Codecierge is running will be sent to your AI provider. Don't send them confidential information!");
        assert_eq!(PRIVACY_KEY, "NoSyncCodeciergeWarningAcknowledged");
        assert_eq!(GOAL_PROMPT, "What are you trying to do? I'll suggest commands and explain their output.");
        assert_eq!(GOAL_PLACEHOLDER, "I want to\u{2026}");
        assert_eq!(AUTO_LABEL, "Run commands automatically");
        assert_eq!(START, "Start");
        assert_eq!(END_TASK, "End Task");
        assert_eq!(RESPONSE_PLACEHOLDER, "Response");
        assert_eq!((GET_STARTED, REVEAL_SETTING, THINKING), ("Get Started", "Reveal Setting", "Thinking\u{2026}"));
        assert_eq!((MIN_HEIGHT, BUSY_ALPHA), (130.0, 0.75));
    }

    /// Task 7 fix: a feed that answers a reply's `execute` keeps the reply's append flag, and
    /// later requests carry the model's call then its result.
    #[test]
    fn a_feed_answer_keeps_the_append_flag_and_the_call() {
        use crate::ai_msg::{Body, CallId, FunctionCall, Message, Role};
        for append in [true, false] {
            let mut s = running_session(Some(7));
            s.sent = Some(("why?".into(), append));
            let id = CallId { call_id: "c1".into(), item_id: "i1".into() };
            s.call = Some(Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall { name: Some("execute".into()), arguments: Some("{}".into()), ..Default::default() }, Some(id.clone()))));
            s.answer_call("feed");
            assert_eq!(s.sent, Some((String::new(), append)), "the answer keeps the request's own flag");
            let tail: Vec<&Message> = s.messages.iter().rev().take(3).collect();
            assert_eq!(tail[2], &Message::text(Role::User, "why?"));
            assert!(tail[1].function_call().is_some(), "the call comes before its result");
            assert_eq!(tail[0].body, Body::FunctionOutput { name: "execute".into(), output: "feed".into(), id: Some(id) });
        }
    }
}
