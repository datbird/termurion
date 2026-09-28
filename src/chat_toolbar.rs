//! The AI Chat window's toolbar and entry rules (`ChatToolbar.swift`, `ChatWindowController.swift`,
//! `ChatViewController.swift`): the popups' options and tooltips, the titles, which messages lock
//! the provider or leave a chat empty, and whether AI Chats shows an old chat or makes a new one.
//! Pure.

use crate::ai_catalog::{Effort, ServiceTier};
use crate::ai_settings::Vendor;
use crate::chat_message::{locks_provider, Content, Message};

pub const WINDOW_TITLE: &str = "AI Chat";
pub const NEW_CHAT_TITLE: &str = "New Chat";
/// The window's first size and its smallest (`CWC:348-401`).
pub const SIZE: (f64, f64) = (800.0, 600.0);
pub const MIN_SIZE: (f64, f64) = (400.0, 300.0);
/// The chat list's first width.
pub const SIDEBAR_WIDTH: f32 = 200.0;

pub const EFFORT_TIP: &str = "Select reasoning effort for models that support it";
pub const TIER_TIP: &str = "Select OpenAI service tier. Priority is faster; Flex is lower-cost and slower.";
pub const THINKING_TIP: &str = "Enable high-effort reasoning? Slower but may produce better results.";
pub const WEB_SEARCH_TIP: &str = "Allow AI to perform web search?";
pub const SESSION_TIP: &str = "Link or unlink terminal/browser session";

/// Segoe Fluent Icons glyphs of the toolbar's buttons.
pub const LIST_GLYPH: char = '\u{E700}';
pub const THINKING_GLYPH: char = '\u{EA80}';
pub const WEB_GLYPH: char = '\u{E774}';
pub const INFO_GLYPH: char = '\u{E946}';

/// The title of a chat opened from a session (`CWC:646-662`). A pane's title is set by any
/// program, so control characters and line breaks are dropped (ruling F20).
pub fn chat_about_title(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    format!("Chat about {}", clean.trim())
}

/// The provider popup's title of a vendor (`TB:57-72`).
pub fn vendor_title(v: Vendor) -> &'static str {
    match v {
        Vendor::OpenAI => "OpenAI",
        Vendor::Anthropic => "Anthropic",
        Vendor::Gemini => "Gemini",
        Vendor::DeepSeek => "DeepSeek",
        Vendor::Llama => "Llama (Local)",
        Vendor::Apple => "Apple",
    }
}

/// A vendor row's identifier.
pub fn vendor_id(v: Vendor) -> String {
    format!("vendor:{}", v.raw())
}

/// A manual model row's identifier.
pub fn manual_id(name: &str) -> String {
    format!("manual:{name}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderOption {
    Vendor { id: String, title: String },
    Manual { name: String },
    Separator,
}

impl ProviderOption {
    /// The identifier a pick reports; None for the separator.
    pub fn id(&self) -> Option<String> {
        match self {
            ProviderOption::Vendor { id, .. } => Some(id.clone()),
            ProviderOption::Manual { name } => Some(manual_id(name)),
            ProviderOption::Separator => None,
        }
    }
}

fn vendor_option(v: Vendor) -> ProviderOption {
    ProviderOption::Vendor { id: vendor_id(v), title: vendor_title(v).to_string() }
}

/// The provider popup's rows (`CVC:3253-3277`): the available vendors, the chat's own vendor
/// after them when it still has models, then a separator and every manual model.
pub fn provider_options(available: &[Vendor], current: Option<Vendor>, current_has_models: bool, manual: &[String]) -> Vec<ProviderOption> {
    let mut out: Vec<ProviderOption> = available.iter().copied().map(vendor_option).collect();
    if let Some(v) = current {
        if current_has_models && !available.contains(&v) {
            out.push(vendor_option(v));
        }
    }
    if !manual.is_empty() && !out.is_empty() {
        out.push(ProviderOption::Separator);
    }
    out.extend(manual.iter().map(|name| ProviderOption::Manual { name: name.clone() }));
    out
}

/// A popup as the toolbar shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Popup {
    pub hidden: bool,
    pub enabled: bool,
    pub tooltip: &'static str,
}

/// The provider popup (`TB:425-445`): hidden with one selectable row, disabled once locked.
pub fn provider_popup(options: &[ProviderOption], can_change: bool) -> Popup {
    let selectable = options.iter().filter(|o| !matches!(o, ProviderOption::Separator)).count();
    let enabled = !options.is_empty() && can_change;
    let tooltip = if enabled { "Select the AI provider for this chat before sending the first message." } else { "The provider is fixed after the first real message in a chat." };
    Popup { hidden: selectable <= 1, enabled, tooltip }
}

/// The model popup (`TB:447-482`).
pub fn model_popup(models: usize, can_change: bool) -> Popup {
    let tooltip = if !can_change {
        "The model is fixed after the chat starts."
    } else if models > 1 {
        "Select a model for this chat."
    } else {
        "Only one model is available for this chat."
    };
    Popup { hidden: models <= 1, enabled: can_change && models > 1, tooltip }
}

/// `TB:600-620`.
pub fn effort_title(e: Effort) -> String {
    let value = match e {
        Effort::None => "None",
        Effort::Minimal => "Minimal",
        Effort::Low => "Low",
        Effort::Medium => "Medium",
        Effort::High => "High",
        Effort::Xhigh => "XHigh",
    };
    format!("Effort: {value}")
}

pub fn tier_title(t: ServiceTier) -> String {
    let value = match t {
        ServiceTier::Auto => "Auto",
        ServiceTier::Default => "Standard",
        ServiceTier::Priority => "Priority (Fast)",
        ServiceTier::Flex => "Flex (Slow)",
    };
    format!("Tier: {value}")
}

/// Whether any message fixes the chat's provider (`CVC:807-819`).
pub fn provider_locked(messages: &[Message]) -> bool {
    messages.iter().any(|m| locks_provider(&m.content))
}

/// A chat with nothing but commands, permissions and watcher events (`CWC:302-319`).
pub fn trivially_empty(messages: &[Message]) -> bool {
    messages.iter().all(|m| matches!(m.content, Content::UserCommand(_) | Content::SetPermissions(_) | Content::WatcherEvent(_)))
}

/// What AI Chats does on opening (`CWC:322-337`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reuse {
    New { offer: Option<String> },
    Select(String),
}

/// A chat this young (seconds) is reused.
const YOUNG: f64 = 5.0 * 60.0;

/// With a current session, its last linked chat is shown, else a new chat offers the link;
/// without one, an empty or under-five-minutes-old most recent chat is shown, else a new one.
pub fn ai_chats_reuse(chat_count: usize, session: Option<&str>, linked_to_session: Option<&str>, selected_or_recent_empty: bool, most_recent: Option<(&str, f64)>) -> Reuse {
    let offer = session.map(str::to_string);
    if chat_count == 0 {
        return Reuse::New { offer };
    }
    if session.is_some() {
        return match linked_to_session {
            Some(chat) => Reuse::Select(chat.to_string()),
            None => Reuse::New { offer },
        };
    }
    match most_recent {
        Some((id, age)) if selected_or_recent_empty || age < YOUNG => Reuse::Select(id.to_string()),
        _ => Reuse::New { offer: None },
    }
}

/// The list toggle's hover text.
pub fn list_toggle_tip(visible: bool) -> &'static str {
    if visible {
        "Hide chat list"
    } else {
        "Show chat list"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_message::{ClientLocal, Participant, UserCommand};
    use serde_json::json;

    fn vendor(v: Vendor) -> ProviderOption {
        ProviderOption::Vendor { id: vendor_id(v), title: vendor_title(v).to_string() }
    }

    #[test]
    fn provider_options_follow_iterm2() {
        let manual = vec!["local-llm".to_string()];
        let base = provider_options(&[Vendor::OpenAI, Vendor::Anthropic], None, false, &manual);
        assert_eq!(base, vec![ProviderOption::Vendor { id: "vendor:2".into(), title: "OpenAI".into() }, ProviderOption::Vendor { id: "vendor:4".into(), title: "Anthropic".into() }, ProviderOption::Separator, ProviderOption::Manual { name: "local-llm".into() }]);
        // The chat's own vendor follows the available ones, before the separator.
        let with_current = provider_options(&[Vendor::OpenAI, Vendor::Anthropic], Some(Vendor::Gemini), true, &manual);
        assert_eq!(with_current[2], ProviderOption::Vendor { id: "vendor:1".into(), title: "Gemini".into() });
        assert_eq!(with_current[3], ProviderOption::Separator);
        // Without models it is left out; an available one is not repeated.
        assert_eq!(provider_options(&[Vendor::OpenAI, Vendor::Anthropic], Some(Vendor::Gemini), false, &manual), base);
        assert_eq!(provider_options(&[Vendor::OpenAI, Vendor::Anthropic], Some(Vendor::OpenAI), true, &manual), base);
        // No manual models, no separator; no vendor, no leading separator.
        assert_eq!(provider_options(&[Vendor::OpenAI], None, false, &[]), vec![vendor(Vendor::OpenAI)]);
        assert_eq!(provider_options(&[], None, false, &manual), vec![ProviderOption::Manual { name: "local-llm".into() }]);
        assert_eq!(vendor_title(Vendor::Llama), "Llama (Local)");
        assert_eq!(vendor_title(Vendor::Apple), "Apple");
        assert_eq!(vendor_title(Vendor::DeepSeek), "DeepSeek");
        assert_eq!(ProviderOption::Manual { name: "m".into() }.id(), Some("manual:m".to_string()));
        assert_eq!(vendor(Vendor::OpenAI).id(), Some("vendor:2".to_string()));
        assert_eq!(ProviderOption::Separator.id(), None);
    }

    #[test]
    fn toolbar_follows_iterm2() {
        let two = provider_options(&[Vendor::OpenAI, Vendor::Anthropic], None, false, &[]);
        assert_eq!(provider_popup(&two, true), Popup { hidden: false, enabled: true, tooltip: "Select the AI provider for this chat before sending the first message." });
        assert_eq!(provider_popup(&two, false), Popup { hidden: false, enabled: false, tooltip: "The provider is fixed after the first real message in a chat." });
        // A separator is not selectable.
        let one = vec![vendor(Vendor::OpenAI), ProviderOption::Separator];
        assert!(provider_popup(&one, true).hidden);
        assert!(!provider_popup(&[], true).enabled);
        assert_eq!(model_popup(3, true), Popup { hidden: false, enabled: true, tooltip: "Select a model for this chat." });
        assert_eq!(model_popup(3, false), Popup { hidden: false, enabled: false, tooltip: "The model is fixed after the chat starts." });
        assert_eq!(model_popup(1, true), Popup { hidden: true, enabled: false, tooltip: "Only one model is available for this chat." });
        let efforts = [Effort::None, Effort::Minimal, Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh];
        let titles: Vec<String> = efforts.into_iter().map(effort_title).collect();
        assert_eq!(titles, ["Effort: None", "Effort: Minimal", "Effort: Low", "Effort: Medium", "Effort: High", "Effort: XHigh"]);
        let tiers: Vec<String> = [ServiceTier::Auto, ServiceTier::Default, ServiceTier::Priority, ServiceTier::Flex].into_iter().map(tier_title).collect();
        assert_eq!(tiers, ["Tier: Auto", "Tier: Standard", "Tier: Priority (Fast)", "Tier: Flex (Slow)"]);
        assert_eq!(list_toggle_tip(true), "Hide chat list");
        assert_eq!(list_toggle_tip(false), "Show chat list");
        assert_eq!(EFFORT_TIP, "Select reasoning effort for models that support it");
        assert_eq!(TIER_TIP, "Select OpenAI service tier. Priority is faster; Flex is lower-cost and slower.");
        assert_eq!(THINKING_TIP, "Enable high-effort reasoning? Slower but may produce better results.");
        assert_eq!(WEB_SEARCH_TIP, "Allow AI to perform web search?");
        assert_eq!(SESSION_TIP, "Link or unlink terminal/browser session");
        assert_eq!((WINDOW_TITLE, NEW_CHAT_TITLE), ("AI Chat", "New Chat"));
        assert_eq!(chat_about_title("zsh"), "Chat about zsh");
        // A title any program can set: no control characters or line breaks.
        assert_eq!(chat_about_title("a\r\nb\u{1b}[31mc"), "Chat about ab[31mc");
    }

    fn msg(content: Content) -> Message {
        Message::new("c", Participant::User, content, 1.0)
    }

    #[test]
    fn provider_lock_follows_iterm2() {
        let mut messages = vec![
            msg(Content::SetPermissions(vec![])),
            msg(Content::ClientLocal(ClientLocal::Notice("n".into()))),
            msg(Content::RenameChat("t".into())),
            msg(Content::Commit("u".into())),
            msg(Content::UserCommand(UserCommand::Stop)),
            msg(Content::VectorStoreCreated("v".into())),
        ];
        assert!(!provider_locked(&messages));
        messages.push(msg(Content::PlainText { text: "hi".into(), context: None }));
        assert!(provider_locked(&messages));
        assert!(provider_locked(&[msg(Content::TerminalCommand(crate::chat_message::TerminalCommand { username: None, hostname: None, directory: None, command: "ls".into(), output: String::new(), exit_code: 0, url: String::new() }))]));

        let mut empty = vec![msg(Content::UserCommand(UserCommand::Stop)), msg(Content::SetPermissions(vec![])), msg(Content::WatcherEvent(json!({})))];
        assert!(trivially_empty(&empty));
        assert!(trivially_empty(&[]));
        empty.push(msg(Content::ClientLocal(ClientLocal::Notice("n".into()))));
        assert!(!trivially_empty(&empty));
    }

    #[test]
    fn reuse_follows_iterm2() {
        assert_eq!(ai_chats_reuse(0, Some("s"), None, false, None), Reuse::New { offer: Some("s".into()) });
        assert_eq!(ai_chats_reuse(0, None, None, false, None), Reuse::New { offer: None });
        assert_eq!(ai_chats_reuse(2, Some("s"), Some("c1"), false, None), Reuse::Select("c1".into()));
        assert_eq!(ai_chats_reuse(2, Some("s"), None, false, None), Reuse::New { offer: Some("s".into()) });
        assert_eq!(ai_chats_reuse(2, None, None, true, Some(("c2", 9999.0))), Reuse::Select("c2".into()));
        assert_eq!(ai_chats_reuse(2, None, None, false, Some(("c2", 120.0))), Reuse::Select("c2".into()));
        assert_eq!(ai_chats_reuse(2, None, None, false, Some(("c2", 300.0))), Reuse::New { offer: None }, "strictly under five minutes");
    }
}
