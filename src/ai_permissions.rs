//! The AI permissions: the categories (`RemoteCommand.Content.PermissionCategory`,
//! `AITerm/RemoteCommand.swift:202-320`), the per-chat overrides and their Swift JSON
//! (`RemoteCommandExecutor.swift`), the allowed set the model is told about, the gate a tool
//! call passes, and the texts around them (`ChatClient.swift:126-181`,
//! `ChatViewController.swift`). Pure.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// What a tool needs the person's permission for. The order is iTerm2's `allCases`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    CheckTerminalState,
    RunCommands,
    ViewContents,
    WriteToClipboard,
    ControlTerminal,
    ViewManpages,
    WriteToFilesystem,
    ActInWebBrowser,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::CheckTerminalState,
        Category::RunCommands,
        Category::ViewContents,
        Category::WriteToClipboard,
        Category::ControlTerminal,
        Category::ViewManpages,
        Category::WriteToFilesystem,
        Category::ActInWebBrowser,
    ];

    /// The persisted name (`rawValue`), used in `setPermissions` and the per-chat overrides.
    pub fn raw(self) -> &'static str {
        match self {
            Category::CheckTerminalState => "Check Terminal State",
            Category::RunCommands => "Run Commands",
            Category::ViewContents => "View Contents",
            Category::WriteToClipboard => "Write to the Clipboard",
            Category::ControlTerminal => "Control Terminal",
            Category::ViewManpages => "View Manpages",
            Category::WriteToFilesystem => "Write to the File System",
            Category::ActInWebBrowser => "Act in Web Browser",
        }
    }

    /// A persisted name back to its category; the two renamed categories keep their old names
    /// (`RC:217-236`).
    pub fn parse(raw: &str) -> Option<Category> {
        match raw {
            "View History" => Some(Category::ViewContents),
            "Type for You" => Some(Category::ControlTerminal),
            _ => Category::ALL.into_iter().find(|c| c.raw() == raw),
        }
    }

    /// The global setting holding the default (`userDefaultsKey`, `RemoteCommand+Mac.swift:102-113`);
    /// Control Terminal keeps its old key.
    pub fn global_key(self) -> &'static str {
        match self {
            Category::CheckTerminalState => "AIPermissionCheckTerminalState",
            Category::RunCommands => "AIPermissionRunCommands",
            Category::ViewContents => "AIPermissionViewHistory",
            Category::WriteToClipboard => "AIPermissionWriteToClipboard",
            Category::ControlTerminal => "AIPermissionTypeForYou",
            Category::ViewManpages => "AIPermissionViewManpages",
            Category::WriteToFilesystem => "AIPermissionWriteToFilesystem",
            Category::ActInWebBrowser => "AIPermissionActInWebBrowser",
        }
    }

    /// The person-facing title (`regularTitle`, `RC:275-294`).
    pub fn title(self) -> &'static str {
        match self {
            Category::CheckTerminalState => "AI can Check Terminal State",
            Category::RunCommands => "AI can Run Commands",
            Category::ViewContents => "AI can View Contents",
            Category::WriteToClipboard => "AI can Write to the Clipboard",
            Category::ControlTerminal => "AI can Control Terminal",
            Category::ViewManpages => "AI can View Manpages",
            Category::WriteToFilesystem => "AI can Write to the File System",
            Category::ActInWebBrowser => "AI can Act in Web Browser",
        }
    }

    /// Governed by the chat's browser session rather than its terminal session.
    pub fn is_browser(self) -> bool {
        self == Category::ActInWebBrowser
    }

    /// At Always, the context is sent with every message instead of asked for.
    pub fn autopopulated_when_always(self) -> bool {
        matches!(self, Category::CheckTerminalState | Category::ViewContents)
    }

    /// At Always, the category's tools are not offered (`suppressesOnRequestToolsWhenAlways`):
    /// the sent terminal state replaces them, while View Contents' tools reach past the screen.
    pub fn hides_tools_when_always(self) -> bool {
        self == Category::CheckTerminalState
    }

    pub fn autopopulation_title(self) -> Option<&'static str> {
        match self {
            Category::CheckTerminalState => Some("Provide Terminal State Automatically"),
            Category::ViewContents => Some("Provide Screen Contents Automatically"),
            _ => None,
        }
    }

    /// The confirmation shown before Always (`RC:260-270`), iTerm2's missing space kept.
    pub fn autopopulation_warning(self) -> Option<&'static str> {
        match self {
            Category::CheckTerminalState => Some("By setting this permission to \u{201c}Always Allow\u{201d}, terminal state will be sent automatically on every message you send in this chat.\nThis includes:\n \u{2022} The current or last command and its exit status\n \u{2022}The window size\n \u{2022} Your shell\n \u{2022} The current working directory, username, and hostname."),
            Category::ViewContents => Some("By setting this permission to \u{201c}Always Allow\u{201d}, the current visible screen of your terminal session will be sent automatically on every message you send in this chat."),
            _ => None,
        }
    }

    /// Whether the model is told about the category's tools (`categoryIsAllowed`, `RCE:78-95`).
    fn exposed(self, p: Permission) -> bool {
        match p {
            Permission::Always => !self.hides_tools_when_always(),
            Permission::Ask => true,
            Permission::Never => false,
        }
    }
}

/// A chat's permission for one category in one session (`RemoteCommandExecutor.Permission`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Always,
    Never,
    Ask,
}

impl Permission {
    /// The persisted name.
    pub fn raw(self) -> &'static str {
        match self {
            Permission::Always => "always",
            Permission::Never => "never",
            Permission::Ask => "ask",
        }
    }

    fn parse(raw: &str) -> Option<Permission> {
        [Permission::Always, Permission::Never, Permission::Ask].into_iter().find(|p| p.raw() == raw)
    }

    fn of(setting: crate::ai_settings::Permission) -> Permission {
        match setting {
            crate::ai_settings::Permission::Ask => Permission::Ask,
            crate::ai_settings::Permission::Allow => Permission::Always,
            crate::ai_settings::Permission::Never => Permission::Never,
        }
    }

    /// A global setting as a lookup and a new chat's defaults read it; an unknown value is Ask
    /// (`RCE:51-62`). Seeding (`RCE:150-153`) reads the same: its `?? .never` never applies,
    /// as `iTermAIPermission(rawValue:)` of an Objective-C enum never fails, and `.init(setting)`
    /// answers Ask for an unknown value (ruling).
    pub fn from_global(v: i64) -> Permission {
        crate::ai_settings::Permission::from_raw(v).map_or(Permission::Ask, Permission::of)
    }

    /// Whether a request parks for the person's answer, given the safety check's verdict
    /// (None: not checked) (`parksOnApproval`, `RCE:35-48`).
    pub fn parks(self, safe: Option<bool>) -> bool {
        match self {
            Permission::Ask => true,
            Permission::Always => safe == Some(false),
            Permission::Never => false,
        }
    }

    /// The Info menu's cycle (`ChatViewController.swift:1162-1167`).
    pub fn next(self) -> Permission {
        match self {
            Permission::Always => Permission::Never,
            Permission::Never => Permission::Ask,
            Permission::Ask => Permission::Always,
        }
    }
}

/// Whose permission: a chat, a session guid (the pane id in decimal) and a category
/// (`RemoteCommandExecutor.Key`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub chat: String,
    pub guid: String,
    pub category: Category,
}

/// The per-chat overrides of the global defaults (`RemoteCommandExecutor.storage`).
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    map: BTreeMap<Key, Permission>,
}

impl Overrides {
    /// The chat's permission in a session, else the global (`permission(chatID:inSessionGuid:category:)`).
    pub fn get(&self, chat: &str, guid: &str, c: Category, global: &dyn Fn(Category) -> i64) -> Permission {
        let key = Key { chat: chat.to_string(), guid: guid.to_string(), category: c };
        self.map.get(&key).copied().unwrap_or_else(|| Permission::from_global(global(c)))
    }

    pub fn set(&mut self, key: Key, p: Permission) {
        self.map.insert(key, p);
    }

    /// The chat's entries as Swift's `JSONEncoder` writes a `[Key: Permission]`: an array
    /// alternating key objects and values (`encodedPermissions`, `RCE:125-130`).
    pub fn encoded(&self, chat: &str) -> String {
        encode(self.map.iter().filter(|(k, _)| k.chat == chat))
    }

    /// Merges a chat's stored text, later entries winning; nothing when any entry fails, as
    /// the whole blob fails to decode (`load(encodedPermissions:)`, `RCE:171-180`). Only the
    /// entries of `chat` itself are taken: a row never grants anything to another chat
    /// (ruling; iTerm2 merges every entry of the text).
    pub fn merge_encoded(&mut self, chat: &str, text: &str) -> bool {
        match decode(text) {
            Some(map) => {
                self.map.extend(map.into_iter().filter(|(k, _)| k.chat == chat));
                true
            }
            None => false,
        }
    }

    /// The categories whose tools the model is told about (`allowedCategories(chatID:terminalGuid:browserGuid:)`,
    /// `RCE:98-123`): a category without its kind of session is Never.
    pub fn allowed(&self, chat: &str, terminal: Option<&str>, browser: Option<&str>, global: &dyn Fn(Category) -> i64) -> BTreeSet<Category> {
        Category::ALL
            .into_iter()
            .filter(|&c| {
                let guid = if c.is_browser() { browser } else { terminal };
                c.exposed(guid.map_or(Permission::Never, |g| self.get(chat, g, c, global)))
            })
            .collect()
    }
}

fn encode<'a>(entries: impl Iterator<Item = (&'a Key, &'a Permission)>) -> String {
    let mut out = Vec::new();
    for (k, p) in entries {
        let mut key = serde_json::Map::new();
        key.insert("chatID".into(), Value::from(k.chat.as_str()));
        key.insert("guid".into(), Value::from(k.guid.as_str()));
        key.insert("category".into(), Value::from(k.category.raw()));
        out.push(Value::Object(key));
        out.push(Value::from(p.raw()));
    }
    Value::Array(out).to_string()
}

/// A stored `[Key: Permission]` text (`permissionsDict(encoded:)`, `RCE:157-159`); None for
/// "" or any entry that does not decode.
pub fn decode(text: &str) -> Option<BTreeMap<Key, Permission>> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else { return None };
    if items.len() % 2 != 0 {
        return None;
    }
    let mut map = BTreeMap::new();
    for pair in items.chunks(2) {
        let field = |name: &str| pair[0].get(name).and_then(Value::as_str).map(str::to_string);
        let key = Key { chat: field("chatID")?, guid: field("guid")?, category: Category::parse(&field("category")?)? };
        map.insert(key, Permission::parse(pair[1].as_str()?)?);
    }
    Some(map)
}

/// A new chat's permissions from the globals (`defaultPermissions`, `RCE:132-155`): the
/// terminal categories under the terminal session, the browser one under the browser session.
pub fn defaults(chat: &str, terminal: Option<&str>, browser: Option<&str>, global: &dyn Fn(Category) -> i64) -> BTreeMap<Key, Permission> {
    Category::ALL
        .into_iter()
        .filter_map(|c| {
            let guid = if c.is_browser() { browser } else { terminal }?;
            Some((Key { chat: chat.to_string(), guid: guid.to_string(), category: c }, Permission::from_global(global(c))))
        })
        .collect()
}

/// A stored text with every entry moved to `chat`, so a fork holds the source's grants as its
/// own from fork time on (ruling); None when the text does not decode.
pub fn rekey(text: &str, chat: &str) -> Option<String> {
    let moved: BTreeMap<Key, Permission> = decode(text)?.into_iter().map(|(k, p)| (Key { chat: chat.to_string(), ..k }, p)).collect();
    Some(encode(moved.iter()))
}

/// A new chat's permissions: the given text when it decodes, else the defaults
/// (`ChatBroker.swift:89-92`).
pub fn seed(chat: &str, terminal: Option<&str>, browser: Option<&str>, permissions: &str, global: &dyn Fn(Category) -> i64) -> BTreeMap<Key, Permission> {
    decode(permissions).unwrap_or_else(|| defaults(chat, terminal, browser, global))
}

/// The categories a permission map exposes, whatever their chat (`allowedCategories(dict:)`).
pub fn allowed_of(map: &BTreeMap<Key, Permission>) -> BTreeSet<Category> {
    map.iter().filter(|(k, p)| k.category.exposed(**p)).map(|(k, _)| k.category).collect()
}

/// The set a permission change publishes: Check Terminal State leaves it while its state is
/// sent automatically (`publishUpdatedPermissions`, `ChatViewController.swift:1204-1220`).
pub fn published_set(allowed: &BTreeSet<Category>, check_state_always: bool) -> Vec<Category> {
    allowed.iter().copied().filter(|&c| !(check_state_always && c == Category::CheckTerminalState)).collect()
}

/// What happens to a tool call (`processRemoteCommandRequest`, `ChatClient.swift:126-181`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Runs now.
    Run,
    /// Waits for the person's answer.
    Park,
    /// Answered with `DENIED_MODEL`, never run.
    Deny,
    /// The chat has no live session of the needed kind: ask the person to pick one.
    NoSession,
}

pub fn gate(linked: bool, p: Permission, safe: Option<bool>) -> Gate {
    if !linked {
        Gate::NoSession
    } else if p == Permission::Never {
        Gate::Deny
    } else if p.parks(safe) {
        Gate::Park
    } else {
        Gate::Run
    }
}

/// One line of the Info menu's permissions (`ChatViewController.swift:2808-2833`).
pub fn summary_line(c: Category, p: Permission) -> String {
    let state = match p {
        Permission::Always if c.autopopulated_when_always() => "Provided automatically",
        Permission::Always => "Always",
        Permission::Never => "Never",
        Permission::Ask => "Ask",
    };
    format!("{}: {state}", c.title())
}

/// The model's answer to a call at Never (`ChatClient.swift:154-162`).
pub const DENIED_MODEL: &str = "The user denied permission to use function calling in this terminal session. Do not try again.";
pub const DENIED_NOTICE: &str = "AI will not execute this command.";
/// The model's answer when the person denies a parked call (`ChatViewController.swift:1889-1896`).
pub const DECLINED_MODEL: &str = "The user declined to allow function calling. Try to find another way to assist.";
/// The model's answer when the person declines to pick a session (`ChatViewController.swift:1776-1805`).
pub const PICKER_DECLINED_MODEL: &str = "The user declined to allow this function call to execute.";
/// A parked call answered with no linked session (`ChatViewController.swift:1833-1850`).
pub const NOT_LINKED_NOTICE: &str = "This chat is not linked to any terminal session.";
pub const NOT_LINKED_MODEL: &str = "The user did not link a terminal session to chat, so the function could not be run.";
pub const NOT_LINKED_USER_NOTICE: &str = "AI attempted to perform an action, but no terminal session is linked to this chat so it failed.";
/// The session picker's text (`ChatViewController.swift:2746-2749`).
pub const NEEDS_SESSION: &str = "The AI agent needs to run commands in a live terminal session, but none is attached to this chat.";
/// A parked call's scope note (`ChatViewController.swift:2634`).
pub const GRANT_SCOPE: &str = "*If you grant or deny permission, it affects only this chat conversation while linked to this particular terminal session. You can change permissions in the chat Info menu.*";
/// The flagged-command warning, without iTerm2's warning emoji (`ChatViewController.swift:2624-2629`).
pub const FLAGGED: &str = "**The AI safety check flagged this command as potentially dangerous. Review it with care.**\n\n";

pub fn grant_question(title: &str) -> String {
    format!("Would you like to grant this permission: **{title}**?")
}

/// A parked call's text (`ChatViewController.swift:2621-2637`): the warning when the check
/// flagged it, the tool's consent description, the question and the scope note.
pub fn parked_text(description: &str, title: &str, safe: Option<bool>) -> String {
    let warning = if safe == Some(false) { FLAGGED } else { "" };
    format!("{warning}{description}. {}\n\n{GRANT_SCOPE}", grant_question(title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_names_follow_iterm2() {
        let raws: Vec<&str> = Category::ALL.iter().map(|c| c.raw()).collect();
        assert_eq!(raws, ["Check Terminal State", "Run Commands", "View Contents", "Write to the Clipboard", "Control Terminal", "View Manpages", "Write to the File System", "Act in Web Browser"]);
        for c in Category::ALL {
            assert_eq!(Category::parse(c.raw()), Some(c));
        }
        assert_eq!(Category::parse("View History"), Some(Category::ViewContents));
        assert_eq!(Category::parse("Type for You"), Some(Category::ControlTerminal));
        assert_eq!(Category::parse("x"), None);
        assert_eq!(Category::parse("run commands"), None);
        let mut sorted = Category::ALL;
        sorted.sort();
        assert_eq!(sorted, Category::ALL);
    }

    use Category::*;

    fn every(v: i64) -> impl Fn(Category) -> i64 {
        move |_| v
    }

    fn key(chat: &str, guid: &str, category: Category) -> Key {
        Key { chat: chat.to_string(), guid: guid.to_string(), category }
    }

    const TERMINAL: [Category; 7] = [CheckTerminalState, RunCommands, ViewContents, WriteToClipboard, ControlTerminal, ViewManpages, WriteToFilesystem];

    #[test]
    fn categories_follow_iterm2() {
        let keys: Vec<&str> = Category::ALL.iter().map(|c| c.global_key()).collect();
        assert_eq!(keys, ["AIPermissionCheckTerminalState", "AIPermissionRunCommands", "AIPermissionViewHistory", "AIPermissionWriteToClipboard", "AIPermissionTypeForYou", "AIPermissionViewManpages", "AIPermissionWriteToFilesystem", "AIPermissionActInWebBrowser"]);
        // 12a's settings rows store the same keys, in the same order.
        let settings: Vec<&str> = crate::ai_settings::PERMISSIONS.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, settings);
        let titles: Vec<&str> = Category::ALL.iter().map(|c| c.title()).collect();
        assert_eq!(titles, ["AI can Check Terminal State", "AI can Run Commands", "AI can View Contents", "AI can Write to the Clipboard", "AI can Control Terminal", "AI can View Manpages", "AI can Write to the File System", "AI can Act in Web Browser"]);
        for c in Category::ALL {
            assert_eq!(c.is_browser(), c == ActInWebBrowser);
            assert_eq!(c.autopopulated_when_always(), matches!(c, CheckTerminalState | ViewContents));
            assert_eq!(c.hides_tools_when_always(), c == CheckTerminalState);
            assert_eq!(c.autopopulation_title().is_some(), c.autopopulated_when_always());
            assert_eq!(c.autopopulation_warning().is_some(), c.autopopulated_when_always());
        }
        assert_eq!(CheckTerminalState.autopopulation_title(), Some("Provide Terminal State Automatically"));
        assert_eq!(ViewContents.autopopulation_title(), Some("Provide Screen Contents Automatically"));
        assert_eq!(
            CheckTerminalState.autopopulation_warning(),
            Some("By setting this permission to \u{201c}Always Allow\u{201d}, terminal state will be sent automatically on every message you send in this chat.\nThis includes:\n \u{2022} The current or last command and its exit status\n \u{2022}The window size\n \u{2022} Your shell\n \u{2022} The current working directory, username, and hostname.")
        );
        assert_eq!(
            ViewContents.autopopulation_warning(),
            Some("By setting this permission to \u{201c}Always Allow\u{201d}, the current visible screen of your terminal session will be sent automatically on every message you send in this chat.")
        );
    }

    #[test]
    fn overrides_follow_iterm2() {
        let g = every(0);
        let mut o = Overrides::default();
        o.set(key("C", "7", ViewContents), Permission::Never);
        o.set(key("C", "7", RunCommands), Permission::Always);
        o.set(key("D", "7", RunCommands), Permission::Never);
        let text = o.encoded("C");
        assert_eq!(text, r#"[{"chatID":"C","guid":"7","category":"Run Commands"},"always",{"chatID":"C","guid":"7","category":"View Contents"},"never"]"#);
        assert_eq!(Overrides::default().encoded("C"), "[]");

        let mut back = Overrides::default();
        assert!(back.merge_encoded("C", &text));
        assert_eq!(back.get("C", "7", RunCommands, &g), Permission::Always);
        assert_eq!(back.get("C", "7", ViewContents, &g), Permission::Never);
        assert_eq!(back.get("D", "7", RunCommands, &g), Permission::Ask);

        // The legacy name, and a later merge winning over an earlier one.
        assert!(back.merge_encoded("C", r#"[{"chatID":"C","guid":"7","category":"View History"},"ask"]"#));
        assert_eq!(back.get("C", "7", ViewContents, &g), Permission::Ask);
        assert!(back.merge_encoded("C", r#"[{"chatID":"C","guid":"7","category":"Type for You"},"always"]"#));
        assert_eq!(back.get("C", "7", ControlTerminal, &g), Permission::Always);

        // A row grants nothing to another chat: its entries for "C" are ignored when merged as "D".
        let mut other = Overrides::default();
        assert!(other.merge_encoded("D", &text));
        assert_eq!(other.get("C", "7", RunCommands, &g), Permission::Ask);
        assert_eq!(other.encoded("C"), "[]");
        assert_eq!(other.encoded("D"), "[]");
        // A fork takes the source's entries as its own.
        let forked = rekey(&text, "F").unwrap();
        assert_eq!(forked, r#"[{"chatID":"F","guid":"7","category":"Run Commands"},"always",{"chatID":"F","guid":"7","category":"View Contents"},"never"]"#);
        assert!(other.merge_encoded("F", &forked));
        assert_eq!(other.get("F", "7", RunCommands, &g), Permission::Always);
        assert_eq!(other.get("C", "7", RunCommands, &g), Permission::Ask);
        assert_eq!(rekey("", "F"), None);
        assert_eq!(rekey("[]", "F").as_deref(), Some("[]"));

        // One bad entry spoils the whole blob.
        let mut none = Overrides::default();
        assert!(!none.merge_encoded("C", r#"[{"chatID":"C","guid":"7","category":"Run Commands"},"always",{"chatID":"C","guid":"7","category":"Fly"},"ask"]"#));
        assert!(!none.merge_encoded("C", r#"[{"chatID":"C","guid":"7","category":"Run Commands"},"sometimes"]"#));
        assert!(!none.merge_encoded("C", r#"[{"chatID":"C","guid":"7","category":"Run Commands"}]"#));
        assert!(!none.merge_encoded("C", r#"[{"guid":"7","category":"Run Commands"},"ask"]"#));
        assert!(!none.merge_encoded("C", r#"{"a":"ask"}"#));
        assert!(!none.merge_encoded("C", ""));
        assert!(!none.merge_encoded("C", "x"));
        assert_eq!(none.encoded("C"), "[]");
        assert!(none.merge_encoded("C", "[]"));

        // The global behind a key never set: 0 Ask, 1 Always, 2 Never, unknown Ask.
        let empty = Overrides::default();
        assert_eq!(empty.get("C", "7", RunCommands, &every(0)), Permission::Ask);
        assert_eq!(empty.get("C", "7", RunCommands, &every(1)), Permission::Always);
        assert_eq!(empty.get("C", "7", RunCommands, &every(2)), Permission::Never);
        assert_eq!(empty.get("C", "7", RunCommands, &every(7)), Permission::Ask);
        assert_eq!(empty.get("C", "7", RunCommands, &every(-1)), Permission::Ask);
        let per = |c: Category| if c == RunCommands { 1 } else { 2 };
        assert_eq!(empty.get("C", "7", RunCommands, &per), Permission::Always);
        assert_eq!(empty.get("C", "7", ViewContents, &per), Permission::Never);

        // A grant is for its session only: relinking starts from the globals.
        let mut one = Overrides::default();
        one.set(key("C", "7", RunCommands), Permission::Always);
        assert_eq!(one.get("C", "7", RunCommands, &g), Permission::Always);
        assert_eq!(one.get("C", "8", RunCommands, &g), Permission::Ask);
        assert_eq!(Permission::Always.raw(), "always");
        assert_eq!(Permission::Never.raw(), "never");
        assert_eq!(Permission::Ask.raw(), "ask");
    }

    #[test]
    fn defaults_follow_iterm2() {
        let d = defaults("C", Some("7"), None, &every(0));
        let expected: BTreeMap<Key, Permission> = TERMINAL.into_iter().map(|c| (key("C", "7", c), Permission::Ask)).collect();
        assert_eq!(d, expected);
        let per = |c: Category| match c {
            RunCommands => 1,
            ViewContents => 2,
            ControlTerminal => 7,
            _ => 0,
        };
        let d = defaults("C", Some("7"), None, &per);
        assert_eq!(d[&key("C", "7", RunCommands)], Permission::Always);
        assert_eq!(d[&key("C", "7", ViewContents)], Permission::Never);
        assert_eq!(d[&key("C", "7", ControlTerminal)], Permission::Ask, "an unknown global seeds Ask, as iTerm2 as built");
        assert_eq!(d[&key("C", "7", WriteToClipboard)], Permission::Ask);
        assert_eq!(Permission::from_global(7), Permission::Ask);
        for (v, p) in [(0, Permission::Ask), (1, Permission::Always), (2, Permission::Never)] {
            assert_eq!(Permission::from_global(v), p);
        }
        assert!(defaults("C", None, None, &every(0)).is_empty());
        let browser = defaults("C", None, Some("9"), &every(1));
        assert_eq!(browser.into_iter().collect::<Vec<_>>(), [(key("C", "9", ActInWebBrowser), Permission::Always)]);

        let mut map = BTreeMap::new();
        map.insert(key("C", "7", RunCommands), Permission::Ask);
        map.insert(key("C", "7", ViewContents), Permission::Always);
        map.insert(key("C", "7", WriteToClipboard), Permission::Never);
        map.insert(key("C", "7", CheckTerminalState), Permission::Always);
        assert_eq!(allowed_of(&map), BTreeSet::from([RunCommands, ViewContents]));
        map.insert(key("C", "7", CheckTerminalState), Permission::Ask);
        assert_eq!(allowed_of(&map), BTreeSet::from([CheckTerminalState, RunCommands, ViewContents]));
    }

    #[test]
    fn seed_follows_iterm2() {
        let g = every(0);
        assert_eq!(seed("C", Some("7"), None, "", &g), defaults("C", Some("7"), None, &g));
        let mut one = Overrides::default();
        one.set(key("C", "7", RunCommands), Permission::Always);
        let text = one.encoded("C");
        assert_eq!(seed("C", Some("7"), None, &text, &g), BTreeMap::from([(key("C", "7", RunCommands), Permission::Always)]));
        assert_eq!(seed("C", Some("7"), None, "x", &g), defaults("C", Some("7"), None, &g));
        let unknown = r#"[{"chatID":"C","guid":"7","category":"Fly"},"ask"]"#;
        assert_eq!(seed("C", Some("7"), None, unknown, &g), defaults("C", Some("7"), None, &g));
        let with_browser = seed("C", Some("7"), Some("9"), "", &|c| if c == ActInWebBrowser { 1 } else { 0 });
        assert_eq!(with_browser.len(), 8);
        assert_eq!(with_browser[&key("C", "9", ActInWebBrowser)], Permission::Always);
        assert!(!with_browser.contains_key(&key("C", "7", ActInWebBrowser)));
        assert_eq!(decode(""), None);
        assert_eq!(decode("[]"), Some(BTreeMap::new()));
        assert_eq!(decode(&text), Some(BTreeMap::from([(key("C", "7", RunCommands), Permission::Always)])));
    }

    #[test]
    fn allowed_set_follows_iterm2() {
        let g = every(0);
        let mut o = Overrides::default();
        assert_eq!(o.allowed("C", Some("7"), None, &g), BTreeSet::from(TERMINAL));
        o.set(key("C", "7", CheckTerminalState), Permission::Always);
        let mut without = BTreeSet::from(TERMINAL);
        without.remove(&CheckTerminalState);
        assert_eq!(o.allowed("C", Some("7"), None, &g), without);
        o.set(key("C", "7", ViewContents), Permission::Always);
        assert_eq!(o.allowed("C", Some("7"), None, &g), without);
        o.set(key("C", "7", RunCommands), Permission::Never);
        assert!(!o.allowed("C", Some("7"), None, &g).contains(&RunCommands));
        assert!(o.allowed("C", None, None, &g).is_empty());
        // The browser category needs a browser session.
        assert_eq!(o.allowed("C", None, Some("9"), &g), BTreeSet::from([ActInWebBrowser]));
        assert_eq!(o.allowed("C", None, Some("9"), &every(2)), BTreeSet::new());

        let all = BTreeSet::from(TERMINAL);
        assert_eq!(published_set(&all, true), [RunCommands, ViewContents, WriteToClipboard, ControlTerminal, ViewManpages, WriteToFilesystem]);
        assert_eq!(published_set(&all, false), TERMINAL);
        assert_eq!(published_set(&BTreeSet::new(), true), []);
    }

    #[test]
    fn gate_follows_iterm2() {
        for p in [Permission::Always, Permission::Never, Permission::Ask] {
            for safe in [None, Some(true), Some(false)] {
                assert_eq!(gate(false, p, safe), Gate::NoSession);
            }
        }
        for safe in [None, Some(true), Some(false)] {
            assert_eq!(gate(true, Permission::Never, safe), Gate::Deny);
            assert_eq!(gate(true, Permission::Ask, safe), Gate::Park);
        }
        assert_eq!(gate(true, Permission::Always, Some(false)), Gate::Park);
        assert_eq!(gate(true, Permission::Always, Some(true)), Gate::Run);
        assert_eq!(gate(true, Permission::Always, None), Gate::Run);
        assert!(Permission::Ask.parks(None));
        assert!(Permission::Ask.parks(Some(true)));
        assert!(Permission::Always.parks(Some(false)));
        assert!(!Permission::Always.parks(None));
        assert!(!Permission::Never.parks(Some(false)));
        assert_eq!(Permission::Always.next(), Permission::Never);
        assert_eq!(Permission::Never.next(), Permission::Ask);
        assert_eq!(Permission::Ask.next(), Permission::Always);
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(DENIED_MODEL, "The user denied permission to use function calling in this terminal session. Do not try again.");
        assert_eq!(DENIED_NOTICE, "AI will not execute this command.");
        assert_eq!(DECLINED_MODEL, "The user declined to allow function calling. Try to find another way to assist.");
        assert_eq!(PICKER_DECLINED_MODEL, "The user declined to allow this function call to execute.");
        assert_eq!(NOT_LINKED_NOTICE, "This chat is not linked to any terminal session.");
        assert_eq!(NOT_LINKED_MODEL, "The user did not link a terminal session to chat, so the function could not be run.");
        assert_eq!(NOT_LINKED_USER_NOTICE, "AI attempted to perform an action, but no terminal session is linked to this chat so it failed.");
        assert_eq!(NEEDS_SESSION, "The AI agent needs to run commands in a live terminal session, but none is attached to this chat.");
        assert_eq!(FLAGGED, "**The AI safety check flagged this command as potentially dangerous. Review it with care.**\n\n");
        assert_eq!(GRANT_SCOPE, "*If you grant or deny permission, it affects only this chat conversation while linked to this particular terminal session. You can change permissions in the chat Info menu.*");
        assert_eq!(grant_question("AI can Run Commands"), "Would you like to grant this permission: **AI can Run Commands**?");
        let body = "The AI Agent would like to execute `ls`. Would you like to grant this permission: **AI can Run Commands**?\n\n*If you grant or deny permission, it affects only this chat conversation while linked to this particular terminal session. You can change permissions in the chat Info menu.*";
        assert_eq!(parked_text("The AI Agent would like to execute `ls`", "AI can Run Commands", Some(false)), format!("{FLAGGED}{body}"));
        assert_eq!(parked_text("The AI Agent would like to execute `ls`", "AI can Run Commands", Some(true)), body);
        assert_eq!(parked_text("The AI Agent would like to execute `ls`", "AI can Run Commands", None), body);
        assert_eq!(summary_line(RunCommands, Permission::Always), "AI can Run Commands: Always");
        assert_eq!(summary_line(CheckTerminalState, Permission::Always), "AI can Check Terminal State: Provided automatically");
        assert_eq!(summary_line(ViewContents, Permission::Always), "AI can View Contents: Provided automatically");
        assert_eq!(summary_line(ViewContents, Permission::Ask), "AI can View Contents: Ask");
        assert_eq!(summary_line(WriteToClipboard, Permission::Never), "AI can Write to the Clipboard: Never");
    }
}
