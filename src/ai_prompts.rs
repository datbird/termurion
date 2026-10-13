//! The AI prompts of Settings > General > AI > Prompts: iTerm2's default texts (bundled
//! under `resources/ai/prompts/`, "iTerm2" read as "Termurion"), the required-variable warning
//! and the template evaluator.

/// The prompts the Prompts sub-tab edits (`PH:101-111`, raw 0 to 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Prompt {
    #[default]
    EngageAi,
    AiChat,
    ReadOnlyTerminal,
    ReadWriteTerminal,
    Browser,
    ReadOnlyTerminalBrowser,
    ReadWriteTerminalBrowser,
    Orchestration,
    CodeReviewSystem,
    ChatIcon,
}

impl Prompt {
    /// The stored key (`P:229-241`).
    pub fn key(self) -> &'static str {
        match self {
            Prompt::EngageAi => "AI Prompt",
            Prompt::AiChat => "AI Prompt for AI Chat with no function calling",
            Prompt::ReadOnlyTerminal => "AI Prompt for AI Chat with ReadOnlyTerminal",
            Prompt::ReadWriteTerminal => "AI Prompt for AI Chat with ReadWriteTerminal",
            Prompt::Browser => "AI Prompt for AI Chat with Browser",
            Prompt::ReadOnlyTerminalBrowser => "AI Prompt for AI Chat with ReadOnlyTerminalBrowser",
            Prompt::ReadWriteTerminalBrowser => "AI Prompt for AI Chat with ReadWriteTerminalBrowser",
            Prompt::Orchestration => "AI Prompt for AI Chat Orchestration",
            Prompt::CodeReviewSystem => "AI Prompt for Code Review System",
            Prompt::ChatIcon => "AI Prompt for Chat Icon",
        }
    }

    /// The selector's item (`X:7525-7541`; iTerm2's "Artifical" kept).
    pub fn title(self) -> &'static str {
        match self {
            Prompt::EngageAi => "Edit > Engage Artifical Intelligence",
            Prompt::AiChat => "AI Chat (no function calling)",
            Prompt::ReadOnlyTerminal => "AI Chat + Read-Only Terminal Access",
            Prompt::ReadWriteTerminal => "AI Chat + Full Terminal Access",
            Prompt::Browser => "AI Chat + Browser Access",
            Prompt::ReadOnlyTerminalBrowser => "AI Chat + Read-Only Terminal + Browser Access",
            Prompt::ReadWriteTerminalBrowser => "AI Chat + Full Terminal + Browser Access",
            Prompt::Orchestration => "AI Chat + Orchestration",
            Prompt::CodeReviewSystem => "Code Review System Prompt",
            Prompt::ChatIcon => "Chat List Icon",
        }
    }

    /// The `ai` variable the prompt must contain and what replaces it (`G:2585-2625`).
    pub fn required(self) -> Option<(&'static str, &'static str)> {
        match self {
            Prompt::EngageAi => Some(("prompt", "The query you enter will replace it when speaking to the AI. For example: \u{201c}Write a unix command to \\(ai.prompt).\u{201d}")),
            Prompt::ChatIcon => Some(("subject", "The chat\u{2019}s title will replace it when speaking to the AI.")),
            _ => None,
        }
    }
}

/// The selector's items, a separator between each group (`X:7525-7541`).
pub const SELECTOR: [&[Prompt]; 8] = [
    &[Prompt::EngageAi],
    &[Prompt::AiChat],
    &[Prompt::ReadOnlyTerminal, Prompt::ReadWriteTerminal],
    &[Prompt::Browser],
    &[Prompt::ReadOnlyTerminalBrowser, Prompt::ReadWriteTerminalBrowser],
    &[Prompt::Orchestration],
    &[Prompt::CodeReviewSystem],
    &[Prompt::ChatIcon],
];

/// The Code Review prompt: not in the selector, edited where code reviews start (12f).
pub const CODE_REVIEW_KEY: &str = "AI Prompt for Code Review";

/// What the `uname` variable reads on Windows (ruling 9), for 12e's local sessions.
pub const WINDOWS_UNAME: &str = "Windows_NT";

/// The "?" button's text (`G:4173-4181`), Windows key words.
pub const HELP_MD: &str = include_str!("../resources/ai/ai-prompt-help.md");

/// The Prompts sub-tab's note (`X:7471-7580`).
pub const COST_NOTE: &str = "Your AI provider may charge for each token used.";

/// iTerm2's default for a prompt key (`P:288-496`); None for every other key.
pub fn default_text(key: &str) -> Option<&'static str> {
    Some(match key {
        "AI Prompt" => include_str!("../resources/ai/prompts/engage.txt"),
        "AI Prompt for AI Chat with no function calling" => include_str!("../resources/ai/prompts/chat.txt"),
        "AI Prompt for AI Chat with ReadOnlyTerminal" => include_str!("../resources/ai/prompts/chat-read-only-terminal.txt"),
        "AI Prompt for AI Chat with ReadWriteTerminal" => include_str!("../resources/ai/prompts/chat-read-write-terminal.txt"),
        "AI Prompt for AI Chat with Browser" => include_str!("../resources/ai/prompts/chat-browser.txt"),
        "AI Prompt for AI Chat with ReadOnlyTerminalBrowser" => include_str!("../resources/ai/prompts/chat-read-only-terminal-browser.txt"),
        "AI Prompt for AI Chat with ReadWriteTerminalBrowser" => include_str!("../resources/ai/prompts/chat-read-write-terminal-browser.txt"),
        "AI Prompt for AI Chat Orchestration" => include_str!("../resources/ai/prompts/orchestration.txt"),
        CODE_REVIEW_KEY => include_str!("../resources/ai/prompts/code-review.txt"),
        "AI Prompt for Code Review System" => include_str!("../resources/ai/prompts/code-review-system.txt"),
        "AI Prompt for Chat Icon" => include_str!("../resources/ai/prompts/chat-icon.txt"),
        _ => return None,
    })
}

/// The warning glyph's hover text when `text` lacks the prompt's required variable (`G:3436-3454`).
pub fn warning(prompt: Prompt, text: &str) -> Option<String> {
    let (name, explanation) = prompt.required()?;
    let variable = format!("\\(ai.{name})");
    (!text.contains(&variable)).then(|| format!("The prompt must contain the substring {variable}. {explanation}"))
}

/// Evaluates a prompt template synchronously, with no calls (`AIPromptTemplateEvaluator.swift:33-80`):
/// each `ai` pair is `\(ai.<name>)`, every other variable comes from `lookup`. A syntax error,
/// an unclosed `\(` included, fails.
pub fn evaluate(template: &str, ai: &[(&str, &str)], lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    crate::interp::check(template)?;
    let scoped = |name: &str| match name.strip_prefix("ai.") {
        Some(rest) => ai.iter().find(|(k, _)| *k == rest).map(|(_, v)| v.to_string()),
        None => lookup(name),
    };
    crate::interp::evaluate(template, &scoped, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use std::path::Path;
    use toml::Value;

    fn sha256(text: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    const ALL: [Prompt; 10] = [
        Prompt::EngageAi,
        Prompt::AiChat,
        Prompt::ReadOnlyTerminal,
        Prompt::ReadWriteTerminal,
        Prompt::Browser,
        Prompt::ReadOnlyTerminalBrowser,
        Prompt::ReadWriteTerminalBrowser,
        Prompt::Orchestration,
        Prompt::CodeReviewSystem,
        Prompt::ChatIcon,
    ];

    #[test]
    fn prompt_defaults_are_iterm2s() {
        let table: [(&str, usize, &str); 11] = [
            ("AI Prompt", 237, "4e2dcab7333f5fc3af3bcbed8412fbebe82c8b0f994627b82181cf71fd593843"),
            ("AI Prompt for AI Chat with no function calling", 845, "c38da3d8d1a44f7d5b95ae9e3e2a20e72e6f7f66c698ebb692ccdc97e64e984e"),
            ("AI Prompt for AI Chat with ReadOnlyTerminal", 1288, "fa45e25dd9f720e8fb5c07614d1dd191c99b886028cacb7bc021cbf8c01e862d"),
            ("AI Prompt for AI Chat with ReadWriteTerminal", 1160, "48c33567db13ec3981cf37554baa74c9520224a6e1fde974a1fabf055df3fb08"),
            ("AI Prompt for AI Chat with Browser", 1148, "9670f3c9088a521faf8bd5c3b0c8ed3f80f525b1374797374e3a3b683f09426a"),
            ("AI Prompt for AI Chat with ReadOnlyTerminalBrowser", 1587, "2e936c33b5c58d0e7f66aafffbb2f2500e93124446bb475c13046332a2e53fa3"),
            ("AI Prompt for AI Chat with ReadWriteTerminalBrowser", 1438, "6608c7b0caaf73005ca0b7cc10ea5dfef7e563bc329b296ef654947d3586b4c6"),
            ("AI Prompt for AI Chat Orchestration", 11732, "a0415880cf7213635518f75576821d631a2c6df8756c1828f599659387ffb95e"),
            ("AI Prompt for Code Review", 1092, "337c39e056c530733ab7bc1399b691001dc2faab9d889cbc3370f52aa3ea39d7"),
            ("AI Prompt for Code Review System", 2364, "9566e3c5a985a9e1e7fb243362009e58313c5a0ca8993bb73ceb192f27bacd3f"),
            ("AI Prompt for Chat Icon", 524, "6fe319a2620139a0fcc8093f4b1bd6d7c8287eb30e42ee5e66688e68c34c0dad"),
        ];
        for (key, len, digest) in table {
            let text = default_text(key).unwrap_or_else(|| panic!("no default for {key}"));
            assert_eq!(text.len(), len, "{key}");
            assert_eq!(sha256(text), digest, "{key}");
            assert!(!text.contains("iTerm2"), "{key}");
        }
        let engage = default_text("AI Prompt").unwrap();
        assert!(engage.starts_with("Return a command suitable for copy/pasting into \\(shell) on \\(uname)."));
        assert!(engage.ends_with("It must do this: \\(ai.prompt)"));
        let g = Globals::empty(Path::new("ai-prompts-test.toml"));
        assert_eq!(g.string("AI Prompt for Chat Icon"), include_str!("../resources/ai/prompts/chat-icon.txt"));
        assert_eq!(default_text("AI Prompt for Code Review System"), Some(include_str!("../resources/ai/prompts/code-review-system.txt")));
        assert_eq!(default_text("AiModel"), None);
    }

    #[test]
    fn prompt_selector_follows_iterm2() {
        let flat: Vec<Prompt> = SELECTOR.iter().flat_map(|g| g.iter().copied()).collect();
        assert_eq!(flat, ALL);
        let groups: Vec<Vec<Prompt>> = SELECTOR.iter().map(|g| g.to_vec()).collect();
        use Prompt::*;
        assert_eq!(
            groups,
            [
                vec![EngageAi],
                vec![AiChat],
                vec![ReadOnlyTerminal, ReadWriteTerminal],
                vec![Browser],
                vec![ReadOnlyTerminalBrowser, ReadWriteTerminalBrowser],
                vec![Orchestration],
                vec![CodeReviewSystem],
                vec![ChatIcon],
            ],
            "X:7525-7541: a separator between each group"
        );
        let titles: Vec<&str> = ALL.iter().map(|p| p.title()).collect();
        assert_eq!(
            titles,
            [
                "Edit > Engage Artifical Intelligence",
                "AI Chat (no function calling)",
                "AI Chat + Read-Only Terminal Access",
                "AI Chat + Full Terminal Access",
                "AI Chat + Browser Access",
                "AI Chat + Read-Only Terminal + Browser Access",
                "AI Chat + Full Terminal + Browser Access",
                "AI Chat + Orchestration",
                "Code Review System Prompt",
                "Chat List Icon",
            ]
        );
        let keys: Vec<&str> = ALL.iter().map(|p| p.key()).collect();
        assert_eq!(
            keys,
            [
                "AI Prompt",
                "AI Prompt for AI Chat with no function calling",
                "AI Prompt for AI Chat with ReadOnlyTerminal",
                "AI Prompt for AI Chat with ReadWriteTerminal",
                "AI Prompt for AI Chat with Browser",
                "AI Prompt for AI Chat with ReadOnlyTerminalBrowser",
                "AI Prompt for AI Chat with ReadWriteTerminalBrowser",
                "AI Prompt for AI Chat Orchestration",
                "AI Prompt for Code Review System",
                "AI Prompt for Chat Icon",
            ]
        );
        assert_eq!(CODE_REVIEW_KEY, "AI Prompt for Code Review");
        for key in keys.iter().copied().chain([CODE_REVIEW_KEY]) {
            let s = crate::schema::find_global(key).unwrap_or_else(|| panic!("no schema row for {key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("General", "AI", crate::schema::Kind::String), "{key}");
            assert_eq!(s.default, None, "{key}: the text lives in the resource file");
        }
        assert_eq!(crate::schema::find_global(CODE_REVIEW_KEY).unwrap().label, "Code Review Prompt");
        assert_eq!(crate::schema::find_global("AI Prompt").unwrap().help, "Edit > Engage Artifical Intelligence\nEdited in Settings > General > AI > Prompts.");
    }

    #[test]
    fn required_variable_follows_iterm2() {
        assert_eq!(
            warning(Prompt::EngageAi, "hi").as_deref(),
            Some("The prompt must contain the substring \\(ai.prompt). The query you enter will replace it when speaking to the AI. For example: \u{201c}Write a unix command to \\(ai.prompt).\u{201d}")
        );
        assert_eq!(warning(Prompt::EngageAi, "x \\(ai.prompt)"), None);
        assert_eq!(
            warning(Prompt::ChatIcon, "").as_deref(),
            Some("The prompt must contain the substring \\(ai.subject). The chat\u{2019}s title will replace it when speaking to the AI.")
        );
        assert_eq!(warning(Prompt::AiChat, ""), None);
        assert_eq!(Prompt::EngageAi.required().map(|r| r.0), Some("prompt"));
        assert_eq!(Prompt::ChatIcon.required().map(|r| r.0), Some("subject"));
        for p in ALL.iter().filter(|p| !matches!(p, Prompt::EngageAi | Prompt::ChatIcon)) {
            assert_eq!(p.required(), None, "{p:?}");
            assert_eq!(warning(*p, default_text(p.key()).unwrap()), None, "{p:?}");
        }
        assert_eq!(warning(Prompt::EngageAi, default_text("AI Prompt").unwrap()), None, "the default satisfies its own rule");
        assert_eq!(warning(Prompt::ChatIcon, default_text("AI Prompt for Chat Icon").unwrap()), None);
    }

    #[test]
    fn reset_restores_the_default() {
        let mut g = Globals::empty(Path::new("ai-prompts-reset.toml"));
        g.set("AI Prompt", Value::String("x".into()));
        assert_eq!(g.string("AI Prompt"), "x");
        g.set("AI Prompt", Value::String(default_text("AI Prompt").unwrap().into()));
        assert!(!g.values.contains_key("AI Prompt"), "a value equal to the default is not stored");
        assert_eq!(g.string("AI Prompt"), default_text("AI Prompt").unwrap());
    }

    #[test]
    fn prompt_evaluation_follows_iterm2() {
        let lookup = |k: &str| match k {
            "session.shell" | "shell" => Some("pwsh".to_string()),
            "session.uname" | "uname" => Some(WINDOWS_UNAME.to_string()),
            _ => None,
        };
        assert_eq!(evaluate("Into \\(shell) on \\(uname): \\(ai.prompt)", &[("prompt", "list files")], &lookup), Ok("Into pwsh on Windows_NT: list files".to_string()));
        let unset = crate::interp::evaluate("[\\(ai.subject)]", &|_| None, &[]);
        assert_eq!(evaluate("[\\(ai.subject)]", &[], &lookup), unset);
        assert_eq!(unset, Ok("[]".to_string()), "an unset variable is empty (lax)");
        assert_eq!(evaluate("[\\(ai.subject)]", &[("subject", "cats")], &|_| Some("caller".into())), Ok("[cats]".to_string()), "the ai frame wins over the caller's scope");
        assert!(evaluate("\\(ai.prompt", &[("prompt", "x")], &lookup).is_err(), "an unclosed interpolation fails");
        assert!(evaluate("\\(f(1))", &[], &lookup).is_err(), "synchronous: no calls");
        assert_eq!(WINDOWS_UNAME, "Windows_NT");
    }

    #[test]
    fn help_text_follows_windows() {
        assert_eq!(HELP_MD.len(), 2880);
        assert_eq!(sha256(HELP_MD), "f64f3b4cea64d0507b240446f2cb99bd7760e9544a35fea4f3bdc92960301d13");
        assert!(HELP_MD.contains("(Win+Y)") && HELP_MD.contains("(Shift+Win+.)"));
        assert!(!HELP_MD.contains("iTerm2"));
        assert_eq!(COST_NOTE, "Your AI provider may charge for each token used.");
    }
}
