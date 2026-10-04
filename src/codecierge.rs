//! Codecierge's per-session model, texts and views (`CG` = `Toolbelt/ToolCodecierge.swift`): the
//! stage a session is in, what each stage shows, and the transcript's markdown sections. Pure;
//! the App draws the views and runs the conversation. The conversation itself (prompts, feed,
//! functions) is Task 7's.

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
            .finish()
    }
}

impl Session {
    pub fn new(guid: &str, allowed: bool, has_key: bool) -> Session {
        Session { guid: guid.to_string(), stage: initial_stage(allowed, has_key), goal: String::new(), auto: true, auto_box: false, transcript: String::new(), busy: false, commands: 0, reply: String::new() }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gate::{check_endpoint, Check};
    use crate::ai_models::Endpoint;
    use crate::ai_settings::{Api, Vendor, ADMIN, ENABLE_AI};
    use crate::globals::Globals;
    use std::path::Path;
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
}
