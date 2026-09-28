//! The AI settings' stored key names, iTerm2's AI enums with their raw values and display
//! names, the default chat permissions, and whether AI features are allowed at all. Pure.

use crate::globals::Globals;

/// The administrator's switch (Advanced, `ADV:924`).
pub const ADMIN: &str = "GenerativeAIAllowed";
/// iTerm2's secure `EnableAI` and `AICompletions`, ordinary settings on Windows (ruling 2).
pub const ENABLE_AI: &str = "EnableAI";
pub const AI_COMPLETIONS: &str = "AICompletions";
pub const VENDOR: &str = "AIVendor";
pub const USE_RECOMMENDED: &str = "UseRecommendedAIModel";
pub const MODEL: &str = "AiModel";
pub const TOKEN_LIMIT: &str = "AiMaxTokens";
pub const RESPONSE_TOKEN_LIMIT: &str = "AiResponseMaxTokens";
pub const TIMEOUT: &str = "AI Timeout";
pub const URL: &str = "AitermURL";
pub const USE_LEGACY_API: &str = "AitermUseLegacyAPI";
pub const API: &str = "AITermAPI";
pub const FEATURE_FILE_SEARCH: &str = "AIFeatureHostedFileSearch";
/// iTerm2's spelling (`P:254`).
pub const FEATURE_CODE_INTERPRETER: &str = "AIFeatureHostedCodeInterpeter";
pub const FEATURE_WEB_SEARCH: &str = "AIFeatureHostedWebSearch";
pub const FEATURE_FUNCTION_CALLING: &str = "AIFeatureFunctionCalling";
pub const FEATURE_STREAMING: &str = "AIFeatureStreamingResponses";
pub const VECTOR_STORE: &str = "AIVectorStore";
pub const MANUAL_MODELS: &str = "AIManualModelConfigurations";
pub const ECONOMY_MODEL: &str = "AIEconomyModelName";
pub const OLLAMA_REGULAR: &str = "AIOllamaRegularModel";
pub const OLLAMA_ECONOMY: &str = "AIOllamaEconomyModel";
pub const SAFETY_CHECK: &str = "AI Safety Check";
pub const CUSTOM_HEADERS_ENABLED: &str = "AICustomHeadersEnabled";
pub const CUSTOM_HEADERS: &str = "AICustomHeaders";
pub const ZERO_DATA_RETENTION: &str = "AIZeroDataRetention";
pub const MODEL_UPDATES: &str = "AIModelUpdatesEnabled";
/// The hosts a person allowed requests to (Tandem's, ruling 1); not a schema row, so import
/// never brings it.
pub const CONSENT: &str = "NoSyncAIRequestConsent";

/// iTerm2's `iTermAIAPI`, the wire dialect of a model (`PH:80-89`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Api {
    Completions,
    ChatCompletions,
    Responses,
    Gemini,
    EarlyO1,
    Llama,
    DeepSeek,
    Anthropic,
    AppleIntelligence,
}

impl Api {
    const ALL: [Api; 9] = [
        Api::Completions,
        Api::ChatCompletions,
        Api::Responses,
        Api::Gemini,
        Api::EarlyO1,
        Api::Llama,
        Api::DeepSeek,
        Api::Anthropic,
        Api::AppleIntelligence,
    ];

    /// The model editor's API popup order (`G:746-768`); Apple Intelligence is never offered.
    pub const EDITOR_ORDER: [Api; 8] = [Api::Responses, Api::ChatCompletions, Api::Anthropic, Api::Gemini, Api::DeepSeek, Api::Llama, Api::Completions, Api::EarlyO1];

    pub fn raw(self) -> i64 {
        self as i64
    }

    pub fn from_raw(v: i64) -> Option<Api> {
        usize::try_from(v).ok().and_then(|i| Api::ALL.get(i).copied())
    }

    /// An out-of-range value reads as Chat Completions, as iTerm2's display does (`G:114-141`).
    pub fn from_raw_or_chat(v: i64) -> Api {
        Api::from_raw(v).unwrap_or(Api::ChatCompletions)
    }

    /// The display name (`G:114-141`).
    pub fn title(self) -> &'static str {
        match self {
            Api::Completions => "Completions",
            Api::ChatCompletions => "Chat Completions",
            Api::Responses => "Responses",
            Api::Gemini => "Google Gemini",
            Api::EarlyO1 => "Chat Completions (Early O1)",
            Api::Llama => "Ollama",
            Api::DeepSeek => "DeepSeek",
            Api::Anthropic => "Anthropic",
            Api::AppleIntelligence => "Apple Intelligence",
        }
    }
}

/// iTerm2's `iTermAIVendor` (`PH:92-98`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Vendor {
    DeepSeek,
    Gemini,
    OpenAI,
    Llama,
    Anthropic,
    Apple,
}

impl Vendor {
    const ALL: [Vendor; 6] = [Vendor::DeepSeek, Vendor::Gemini, Vendor::OpenAI, Vendor::Llama, Vendor::Anthropic, Vendor::Apple];

    /// The default-model popup's providers (`G:2649-2661`); Apple Intelligence is never offered.
    pub const POPUP_ORDER: [Vendor; 5] = [Vendor::OpenAI, Vendor::Anthropic, Vendor::Gemini, Vendor::DeepSeek, Vendor::Llama];
    /// The key dialog's fields (`G:3547-3626`).
    pub const KEY_ORDER: [Vendor; 4] = [Vendor::OpenAI, Vendor::Anthropic, Vendor::Gemini, Vendor::DeepSeek];

    pub fn raw(self) -> i64 {
        self as i64
    }

    pub fn from_raw(v: i64) -> Option<Vendor> {
        usize::try_from(v).ok().and_then(|i| Vendor::ALL.get(i).copied())
    }

    /// The provider name (`G:145-162`).
    pub fn provider_name(self) -> &'static str {
        match self {
            Vendor::DeepSeek => "DeepSeek",
            Vendor::Gemini => "Gemini",
            Vendor::OpenAI => "OpenAI",
            Vendor::Llama => "Ollama",
            Vendor::Anthropic => "Anthropic",
            Vendor::Apple => "Apple Intelligence",
        }
    }

    /// Whether a person can type an API key for the vendor (the key dialog's four fields).
    pub fn has_enterable_key(self) -> bool {
        Vendor::KEY_ORDER.contains(&self)
    }
}

/// iTerm2's `iTermAIPermission`, a default chat permission (`PH:15-18`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Ask,
    Allow,
    Never,
}

impl Permission {
    pub fn raw(self) -> i64 {
        self as i64
    }

    pub fn from_raw(v: i64) -> Option<Permission> {
        match v {
            0 => Some(Permission::Ask),
            1 => Some(Permission::Allow),
            2 => Some(Permission::Never),
            _ => None,
        }
    }
}

/// The default chat permissions: stored key and label, in the Features sub-tab's order
/// (`X:7583-7866`, `P:276-285`). "Control Terminal" keeps the stored key `AIPermissionTypeForYou`.
pub const PERMISSIONS: [(&str, &str); 8] = [
    ("AIPermissionCheckTerminalState", "Check Terminal State"),
    ("AIPermissionRunCommands", "Run Commands"),
    ("AIPermissionViewHistory", "View Contents"),
    ("AIPermissionWriteToClipboard", "Write to Clipboard"),
    ("AIPermissionTypeForYou", "Control Terminal"),
    ("AIPermissionViewManpages", "View Manpages"),
    ("AIPermissionWriteToFilesystem", "Write to Filesystem"),
    ("AIPermissionActInWebBrowser", "Act in Web Browser"),
];

/// AI features may be used: the administrator allows them and the person enabled them (`GK:106-110`).
pub fn allowed(g: &Globals) -> bool {
    g.bool(ADMIN) && g.bool(ENABLE_AI)
}

/// The switches that stand between a person and an AI request: the administrator's, EnableAI,
/// AICompletions, the consent list and the default chat permissions. The one list Toggle
/// Setting, SetPreference, Import from iTerm2 and every settings load (a custom folder or URL,
/// Import All) refuse, so none of them can bypass consent.
pub fn guards_consent(key: &str) -> bool {
    [ADMIN, ENABLE_AI, AI_COMPLETIONS, CONSENT].contains(&key) || PERMISSIONS.iter().any(|(k, _)| *k == key)
}

/// Whether Toggle Setting may flip this key: the consent switches (`guards_consent`) are never
/// offered and never flipped, so a key binding cannot bypass consent.
pub fn toggle_allowed(key: &str) -> bool {
    !guards_consent(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    #[test]
    fn ai_enums_follow_iterm2() {
        use Api::*;
        let apis = [Completions, ChatCompletions, Responses, Gemini, EarlyO1, Llama, DeepSeek, Anthropic, AppleIntelligence];
        for (i, a) in apis.iter().enumerate() {
            assert_eq!(a.raw(), i as i64, "{a:?}");
            assert_eq!(Api::from_raw(i as i64), Some(*a));
        }
        assert_eq!(Api::from_raw(9), None);
        assert_eq!(Api::from_raw(-1), None);
        assert_eq!(Api::from_raw_or_chat(999), ChatCompletions);
        assert_eq!(Api::from_raw_or_chat(7), Anthropic);
        let titles: Vec<&str> = apis.iter().map(|a| a.title()).collect();
        assert_eq!(
            titles,
            ["Completions", "Chat Completions", "Responses", "Google Gemini", "Chat Completions (Early O1)", "Ollama", "DeepSeek", "Anthropic", "Apple Intelligence"]
        );
        assert_eq!(Api::EDITOR_ORDER, [Responses, ChatCompletions, Anthropic, Gemini, DeepSeek, Llama, Completions, EarlyO1]);

        let vendors = [Vendor::DeepSeek, Vendor::Gemini, Vendor::OpenAI, Vendor::Llama, Vendor::Anthropic, Vendor::Apple];
        for (i, v) in vendors.iter().enumerate() {
            assert_eq!(v.raw(), i as i64, "{v:?}");
            assert_eq!(Vendor::from_raw(i as i64), Some(*v));
        }
        assert_eq!(Vendor::from_raw(6), None);
        assert_eq!(Vendor::from_raw(-1), None);
        let names: Vec<&str> = vendors.iter().map(|v| v.provider_name()).collect();
        assert_eq!(names, ["DeepSeek", "Gemini", "OpenAI", "Ollama", "Anthropic", "Apple Intelligence"]);
        let keyed: Vec<bool> = vendors.iter().map(|v| v.has_enterable_key()).collect();
        assert_eq!(keyed, [true, true, true, false, true, false]);
        assert_eq!(Vendor::POPUP_ORDER, [Vendor::OpenAI, Vendor::Anthropic, Vendor::Gemini, Vendor::DeepSeek, Vendor::Llama]);
        assert_eq!(Vendor::KEY_ORDER, [Vendor::OpenAI, Vendor::Anthropic, Vendor::Gemini, Vendor::DeepSeek]);

        for (p, raw) in [(Permission::Ask, 0), (Permission::Allow, 1), (Permission::Never, 2)] {
            assert_eq!(p.raw(), raw);
            assert_eq!(Permission::from_raw(raw), Some(p));
        }
        assert_eq!(Permission::from_raw(3), None);
        assert_eq!(
            PERMISSIONS,
            [
                ("AIPermissionCheckTerminalState", "Check Terminal State"),
                ("AIPermissionRunCommands", "Run Commands"),
                ("AIPermissionViewHistory", "View Contents"),
                ("AIPermissionWriteToClipboard", "Write to Clipboard"),
                ("AIPermissionTypeForYou", "Control Terminal"),
                ("AIPermissionViewManpages", "View Manpages"),
                ("AIPermissionWriteToFilesystem", "Write to Filesystem"),
                ("AIPermissionActInWebBrowser", "Act in Web Browser"),
            ]
        );

        // `GK:106-110`: the admin switch and EnableAI together.
        let mut g = Globals::empty(std::path::Path::new("ai-allowed-test.toml"));
        assert!(!allowed(&g));
        g.set(ENABLE_AI, Value::Boolean(true));
        assert!(allowed(&g));
        g.set(ADMIN, Value::Boolean(false));
        assert!(!allowed(&g));
    }

    #[test]
    fn toggle_setting_never_reaches_ai_consent() {
        for key in [ADMIN, ENABLE_AI, AI_COMPLETIONS, CONSENT] {
            assert!(!toggle_allowed(key), "{key}");
        }
        for (key, _) in PERMISSIONS {
            assert!(!toggle_allowed(key), "{key}");
        }
        for key in [SAFETY_CHECK, ZERO_DATA_RETENTION, FEATURE_STREAMING, "FocusFollowsMouse"] {
            assert!(toggle_allowed(key), "{key}");
        }
    }
}
