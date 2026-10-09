//! Find on a web page (milestone 13x3, Task 1): iTerm2's `custom-find.js` and the commands it takes
//! (`iTermBrowserFindManager.swift`). The script runs only in Tandem's isolated world; this module
//! assembles it, names its four search modes and builds the JSON commands the app hands it. Pure:
//! the page, the secret's use and every result are the app's and the next tasks' business.

use serde_json::{Value, json};

use crate::browser_js::{self, Inject, UserScript, World};
use crate::find::Mode;

/// The message handler the find script posts to (`FM:68`).
pub const HANDLER: &str = "iTermCustomFind";
/// The user script's identifier (`BM:403`).
pub const SCRIPT_ID: &str = "iTermCustomFind";

/// The expression `browser_js_call` runs with the argument `command` (`FM:230`), in the main frame's
/// Tandem world: the page's world has no `iTermCustomFind`.
pub const COMMAND_JS: &str = "return window.iTermCustomFind && await window.iTermCustomFind.handleCommand(command);";

/// The thirteen files copied from `FindOnPage/` (`graph-discovery-api.js`, which `custom-find.js`
/// also includes, is 13x2's). Each is registered in `browser_js::resource`.
pub const FILES: &[&str] = &[
    "custom-find.js",
    "find-api.js",
    "find-constants.js",
    "find-engine.js",
    "find-global-click.js",
    "find-highlighter.js",
    "find-iframe-segment.js",
    "find-match.js",
    "find-nav.js",
    "find-security.js",
    "find-segment.js",
    "find-text-segment.js",
    "find-utils.js",
];

/// `iTermBrowserFindMode` (`FM:15-20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindMode {
    CaseSensitive,
    CaseInsensitive,
    CaseSensitiveRegex,
    CaseInsensitiveRegex,
}

impl FindMode {
    /// The raw value the script's `VALID_SEARCH_MODES` lists (`FC:15`).
    pub fn raw(self) -> &'static str {
        match self {
            FindMode::CaseSensitive => "caseSensitive",
            FindMode::CaseInsensitive => "caseInsensitive",
            FindMode::CaseSensitiveRegex => "caseSensitiveRegex",
            FindMode::CaseInsensitiveRegex => "caseInsensitiveRegex",
        }
    }
}

/// Unicode category Lu or Lt: `CharacterSet.uppercaseLetters`. Rust's `is_uppercase` is Lu plus
/// Other_Uppercase (symbols such as a Roman numeral) and leaves out titlecase (Lt).
fn uppercase_letter(c: char) -> bool {
    let other_uppercase = matches!(c, '\u{2160}'..='\u{216f}' | '\u{24b6}'..='\u{24cf}' | '\u{1f130}'..='\u{1f149}' | '\u{1f150}'..='\u{1f169}' | '\u{1f170}'..='\u{1f189}');
    let titlecase = matches!(c, '\u{1c5}' | '\u{1c8}' | '\u{1cb}' | '\u{1f2}' | '\u{1f88}'..='\u{1f8f}' | '\u{1f98}'..='\u{1f9f}' | '\u{1fa8}'..='\u{1faf}' | '\u{1fbc}' | '\u{1fcc}' | '\u{1ffc}');
    titlecase || (c.is_uppercase() && !other_uppercase)
}

/// `iTermFindMode.browserFindMode(query:)` (`PB:535-551`): smart case is case sensitive when the
/// query holds an uppercase letter.
pub fn browser_mode(mode: Mode, query: &str) -> FindMode {
    match mode {
        Mode::SmartCase if query.chars().any(uppercase_letter) => FindMode::CaseSensitive,
        Mode::SmartCase => FindMode::CaseInsensitive,
        Mode::CaseSensitive => FindMode::CaseSensitive,
        Mode::CaseInsensitive => FindMode::CaseInsensitive,
        Mode::RegexSensitive => FindMode::CaseSensitiveRegex,
        Mode::RegexInsensitive => FindMode::CaseInsensitiveRegex,
    }
}

/// The name of an include still in `text` (the template loader leaves a missing one in place in a
/// release build).
fn missing_include(text: &str) -> Option<String> {
    let at = text.find("{{INCLUDE:")? + "{{INCLUDE:".len();
    let end = text[at..].find("}}").map_or(text.len(), |e| at + e);
    Some(text[at..end].chars().take(64).collect())
}

/// `FM:356-367`: `custom-find.js` with its includes expanded, then the keys. The secret goes into a
/// JS string literal, so it must be plain letters and digits (the view's is 32 hex characters).
pub fn find_script(secret: &str) -> Result<String, String> {
    if secret.is_empty() || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("the find secret is not plain letters and digits".into());
    }
    let src = browser_js::resource("custom-find.js").ok_or("custom-find.js is not embedded")?;
    let script = browser_js::fill_template(src, &[("SECRET", secret), ("SCROLL_BEHAVIOR", "smooth"), ("TEST_FUNCTIONS", ""), ("TEST_IMPLS", ""), ("TEST_FREEZE", "")]);
    match missing_include(&script) {
        Some(name) => Err(format!("find script include not found: {name}")),
        None => Ok(script),
    }
}

/// The find script as a user script: the isolated world, every frame, at document start (`BM:397-408`).
/// An `Err` means the pane gets no find script (find then finds nothing).
pub fn user_script(secret: &str) -> Result<UserScript, String> {
    find_script(secret).map(|code| UserScript { id: SCRIPT_ID, code, inject: Inject::DocumentStart, main_frame_only: false, world: World::Client })
}

/// `FM:223-227`: the command with the instance and the secret added last, so `extra` cannot set them.
pub fn command(action: &str, extra: Value, instance: &str, secret: &str) -> Value {
    let mut o = match extra {
        Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    o.insert("action".into(), json!(action));
    o.insert("instanceId".into(), json!(instance));
    o.insert("sessionSecret".into(), json!(secret));
    Value::Object(o)
}

/// The default find state's instance id (`FM:339`).
pub fn default_instance(uuid: &str) -> String {
    format!("main_{uuid}")
}

/// The global find state's instance id (`FM:340`).
pub fn global_instance(uuid: &str) -> String {
    format!("global_{uuid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIRTEEN: [&str; 13] = [
        "custom-find.js",
        "find-api.js",
        "find-constants.js",
        "find-engine.js",
        "find-global-click.js",
        "find-highlighter.js",
        "find-iframe-segment.js",
        "find-match.js",
        "find-nav.js",
        "find-security.js",
        "find-segment.js",
        "find-text-segment.js",
        "find-utils.js",
    ];

    /// A file's text as it sits in the script: its two slots filled.
    fn filled(name: &str, secret: &str) -> String {
        browser_js::resource(name).unwrap().replace("{{SECRET}}", secret).replace("{{SCROLL_BEHAVIOR}}", "smooth")
    }

    #[test]
    fn browser_modes_follow_iterm2() {
        use FindMode::*;
        assert_eq!(browser_mode(Mode::SmartCase, "rust"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "Rust"), CaseSensitive);
        // Unicode uppercase counts (`CharacterSet.uppercaseLetters`), titlecase too (Lt).
        assert_eq!(browser_mode(Mode::SmartCase, "\u{c9}t\u{e9}"), CaseSensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "a\u{1c5}"), CaseSensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "123"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::SmartCase, ""), CaseInsensitive);
        // Other_Uppercase symbols (a Roman numeral, a circled letter) are not letters of category Lu.
        assert_eq!(browser_mode(Mode::SmartCase, "\u{2160}\u{24b6}"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::CaseSensitive, "abc"), CaseSensitive);
        assert_eq!(browser_mode(Mode::CaseInsensitive, "ABC"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::RegexSensitive, "abc"), CaseSensitiveRegex);
        assert_eq!(browser_mode(Mode::RegexInsensitive, "ABC"), CaseInsensitiveRegex);
        let constants = browser_js::resource("find-constants.js").expect("find-constants.js is embedded");
        let list = constants.lines().find(|l| l.starts_with("const VALID_SEARCH_MODES")).expect("VALID_SEARCH_MODES");
        for (mode, raw) in [(CaseSensitive, "caseSensitive"), (CaseInsensitive, "caseInsensitive"), (CaseSensitiveRegex, "caseSensitiveRegex"), (CaseInsensitiveRegex, "caseInsensitiveRegex")] {
            assert_eq!(mode.raw(), raw);
            assert!(list.contains(&format!("'{raw}'")), "{raw} is in {list}");
        }
    }

    #[test]
    fn includes_follow_iterm2() {
        let custom = browser_js::resource("custom-find.js").expect("custom-find.js is embedded");
        let names: Vec<&str> = custom.split("{{INCLUDE:").skip(1).map(|t| t.split("}}").next().unwrap()).collect();
        assert_eq!(
            names,
            ["graph-discovery-api.js", "find-security.js", "find-constants.js", "find-utils.js", "find-global-click.js", "find-segment.js", "find-text-segment.js", "find-iframe-segment.js", "find-match.js", "find-engine.js", "find-api.js", "find-highlighter.js", "find-nav.js"],
            "CF:5-17"
        );
        for n in &names {
            assert!(browser_js::resource(n).is_some(), "{n} is registered");
        }
        let script = find_script("s").expect("the script assembles");
        assert!(!script.contains("{{INCLUDE:"));
        let at = |n: &str| script.find(&filled(n, "s")).unwrap_or_else(|| panic!("{n} is in the script"));
        assert!(at("graph-discovery-api.js") < at("find-nav.js"));
        for pair in names.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{} before {}", pair[0], pair[1]);
        }
        for f in FILES {
            assert!(browser_js::resource(f).is_some(), "{f} is registered");
        }
    }

    #[test]
    fn find_script_follows_iterm2() {
        let s = find_script("abc").expect("the script assembles");
        assert!(s.contains("const sessionSecret = \"abc\";"), "FC:7");
        assert!(s.contains("behavior: 'smooth'"), "FE:1313");
        assert!(!s.contains("{{"), "no slot is left");
        for f in ["find-engine.js", "find-nav.js"] {
            assert!(s.contains(&filled(f, "abc")), "{f}");
        }
        assert_eq!(s.matches("Object.defineProperty(window, 'iTermCustomFind'").count(), 1, "FA:308");
        assert_eq!(FILES.len(), 13);
        let mut want = THIRTEEN.to_vec();
        want.sort();
        let mut have = FILES.to_vec();
        have.sort();
        assert_eq!(have, want);
        let u = user_script("abc").expect("the user script");
        assert_eq!((u.id, u.world, u.inject, u.main_frame_only), (SCRIPT_ID, World::Client, Inject::DocumentStart, false));
        assert_eq!(u.code, s);
        // A leftover include is an error naming it, never a silent gap.
        assert_eq!(missing_include("a {{INCLUDE:nope.js}} b"), Some("nope.js".to_string()));
        assert_eq!(missing_include("a b"), None);
    }

    #[test]
    fn commands_follow_iterm2() {
        let c = command("startFind", json!({"searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive"}), "main_U", "s");
        let o = c.as_object().expect("an object");
        let mut keys: Vec<&str> = o.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["action", "contextLength", "instanceId", "searchMode", "searchTerm", "sessionSecret"]);
        assert_eq!(c, json!({"action": "startFind", "searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive", "instanceId": "main_U", "sessionSecret": "s"}));
        assert_eq!(command("findNext", json!({}), "main_U", "s")["action"], "findNext");
        // The caller's extra cannot override what the manager sets last (`FM:223-227`).
        let c = command("findNext", json!({"instanceId": "x", "sessionSecret": "y", "action": "z"}), "main_U", "s");
        assert_eq!((c["action"].as_str(), c["instanceId"].as_str(), c["sessionSecret"].as_str()), (Some("findNext"), Some("main_U"), Some("s")));
        let constants = browser_js::resource("find-constants.js").unwrap();
        let valid = constants.lines().find(|l| l.starts_with("const VALID_ACTIONS")).expect("VALID_ACTIONS");
        for a in ["startFind", "findNext", "findPrevious", "clearFind", "reveal", "hideResults", "showResults", "startNavigationShortcuts", "clearNavigationShortcuts"] {
            assert!(valid.contains(&format!("'{a}'")), "{a} is valid");
        }
        let uuid = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(uuid.len(), 36);
        assert_eq!(default_instance("U"), "main_U");
        assert_eq!(global_instance("U"), "global_U");
        assert!(default_instance(uuid).len() <= 50 && global_instance(uuid).len() <= 50, "FU:36-45 keeps 50");
        assert!(COMMAND_JS.contains("window.iTermCustomFind") && COMMAND_JS.contains("handleCommand(command)"));
    }
}
