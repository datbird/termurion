//! The browser's settings page (milestone 13x8 Task 5; `SH`, `SP`): the copied page as served, the
//! closed set of actions it may send, the link-opening preference, and the replies. Pure: the App
//! applies the effects (`app::browser_pages_app`), and a message is accepted only through
//! `browser_pages::route` (the message's source is the check, never a secret in the JSON; rulings
//! R13x8-4 and R13x8-6). Every value from the page is untrusted input: typed and bounded here, and
//! the App acts on a closed set of keys only (nothing the page sends names a settings key).

use crate::cooldown::Cooldown;
use serde_json::{Value, json};
use std::path::Path;

pub const SETTINGS_URL: &str = "iterm2-about:settings";
pub const HANDLER: &str = "iterm2-about:settings";
/// The page's name in `browser_pages::page_name`.
pub const PAGE: &str = "settings";

/// The longest string a message may carry (an engine or list URL; P13).
pub const MAX_VALUE: usize = 2048;
/// The longest extension id a message may carry (Task 10 holds the exact shape).
const MAX_ID: usize = 128;

/// What the page asks the Tandem side to do (`SH:117-187`).
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    ClearCookies,
    ClearAllData,
    SetAdblockEnabled(bool),
    SetAdblockUrl(String),
    SetSearchCommand(String),
    SetSearchSuggestUrl(String),
    ForceAdblockUpdate,
    GetAdblockSettings,
    GetSearchSettings,
    GetAdblockStats,
    SetProxyEnabled(bool),
    SetProxyHost(String),
    SetProxyPort(i64),
    GetProxySettings,
    GetExtensions,
    SetExtensionEnabled { id: String, enabled: bool },
    RevealExtensionsDirectory,
    OpenPasswordManager,
    SetLinkOpening(String),
    GetLinkOpening,
}

/// What the served page substitutes (`SH:61-66`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    pub adblock_enabled: bool,
    pub adblock_url: String,
    pub dev_null: bool,
    pub show_extensions: bool,
}

/// `&` and `"` as iTerm2 escapes them; `<` and `>` too, and `{` so no later fill can match inside
/// the value (R13x8-2 M2). The browser reads the entities back to the setting's own text.
fn attribute_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;").replace('{', "&#123;")
}

pub fn substitutions(s: &PageState, secret: &str) -> Vec<(&'static str, String)> {
    vec![
        ("ADBLOCK_ENABLED", if s.adblock_enabled { "checked" } else { "" }.to_string()),
        ("ADBLOCK_URL", attribute_text(&s.adblock_url)),
        ("SECRET", secret.to_string()),
        ("DEV_NULL_NOTE", if s.dev_null { "" } else { "display: none;" }.to_string()),
        ("SHOW_EXTENSIONS", if s.show_extensions { "" } else { "display: none;" }.to_string()),
    ]
}

fn string_value(map: &serde_json::Map<String, Value>) -> Option<String> {
    match map.get("value") {
        Some(Value::String(s)) if s.len() <= MAX_VALUE => Some(s.clone()),
        _ => None,
    }
}

fn bool_value(map: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    map.get(key)?.as_bool()
}

/// The action a message body names. iTerm2 refuses a message without its `sessionSecret`
/// (`SH:109-114`); Tandem keeps the field (the copied page sends it) but decides by the message's
/// source, so only its type is checked. A value of the wrong JSON type, a key outside the action's
/// own, or an unknown action is None (`as?` casts, `SH:132-187`).
pub fn parse(body: &Value) -> Option<Msg> {
    let map = body.as_object()?;
    match map.get("sessionSecret") {
        None => {}
        Some(Value::String(s)) if s.len() <= MAX_VALUE => {}
        _ => return None,
    }
    let action = map.get("action")?.as_str()?;
    let extra: &[&str] = match action {
        "setAdblockEnabled" | "setAdblockURL" | "setSearchCommand" | "setSearchSuggestURL" | "setProxyEnabled" | "setProxyHost" | "setProxyPort" | "setLinkOpeningPreference" => &["value"],
        "setExtensionEnabled" => &["extensionId", "enabled"],
        _ => &[],
    };
    if !map.keys().all(|k| k == "action" || k == "sessionSecret" || extra.contains(&k.as_str())) {
        return None;
    }
    Some(match action {
        "clearCookies" => Msg::ClearCookies,
        "clearAllData" => Msg::ClearAllData,
        "setAdblockEnabled" => Msg::SetAdblockEnabled(bool_value(map, "value")?),
        "setAdblockURL" => Msg::SetAdblockUrl(string_value(map)?),
        "setSearchCommand" => Msg::SetSearchCommand(string_value(map)?),
        "setSearchSuggestURL" => Msg::SetSearchSuggestUrl(string_value(map)?),
        "forceAdblockUpdate" => Msg::ForceAdblockUpdate,
        "getAdblockSettings" => Msg::GetAdblockSettings,
        "getSearchSettings" => Msg::GetSearchSettings,
        "getAdblockStats" => Msg::GetAdblockStats,
        "setProxyEnabled" => Msg::SetProxyEnabled(bool_value(map, "value")?),
        "setProxyHost" => Msg::SetProxyHost(string_value(map)?),
        // `as? Int`: a JSON integer only (a string, a float or a bool is not one).
        "setProxyPort" => Msg::SetProxyPort(map.get("value")?.as_i64()?),
        "getProxySettings" => Msg::GetProxySettings,
        "getExtensions" => Msg::GetExtensions,
        "setExtensionEnabled" => {
            let id = match map.get("extensionId") {
                Some(Value::String(s)) if !s.is_empty() && s.len() <= MAX_ID => s.clone(),
                _ => return None,
            };
            Msg::SetExtensionEnabled { id, enabled: bool_value(map, "enabled")? }
        }
        "revealExtensionsDirectory" => Msg::RevealExtensionsDirectory,
        "openPasswordManager" => Msg::OpenPasswordManager,
        "setLinkOpeningPreference" => Msg::SetLinkOpening(string_value(map)?),
        "getLinkOpeningPreference" => Msg::GetLinkOpening,
        _ => return None,
    })
}

// --- Replies: every argument is a JSON value, never text pasted into a script (preflight B9) ---

/// `showStatus(<message>, <kind>);` (`SH:302-307`). Both arguments are JSON strings.
pub fn status_script(message: &str, kind: &str) -> String {
    format!("showStatus({}, {});", Value::String(message.to_string()), Value::String(kind.to_string()))
}

/// `alert(<message>);` with the message as a JSON string (Decision: iTerm2 quotes with `'` unescaped,
/// `SH:223`).
pub fn alert_script(message: &str) -> String {
    format!("alert({});", Value::String(message.to_string()))
}

/// `updateLinkOpeningUI({"preference":"ask"|"builtin"|"default"});`
pub fn link_ui_script(p: LinkPref) -> String {
    let word = match p {
        LinkPref::Ask => "ask",
        LinkPref::Builtin => "builtin",
        LinkPref::Default => "default",
    };
    format!("updateLinkOpeningUI({});", json!({ "preference": word }))
}

pub const DATA_CLEARED: &str = "All website data has been cleared successfully!";
/// Shown when the web view could not clear its data (Decision: iTerm2's call always completes).
pub const DATA_CLEAR_FAILED: &str = "The website data could not be cleared.";

/// `SH:213-219`: the database could not be deleted, with its path when known.
pub fn db_delete_failed(path: Option<&Path>) -> String {
    match path {
        Some(p) => format!("The browser database could not be deleted. It is in {}", p.display()),
        None => "The browser database could not be deleted. Your application support folder could not be found.".to_string(),
    }
}

pub const LINK_ASK: &str = "Link opening preference set to always ask";
pub const LINK_BUILTIN: &str = "Links will always open in built-in browser";
pub const LINK_DEFAULT: &str = "Links will always open in default browser";
pub const LINK_INVALID: &str = "Invalid link opening preference";

/// Where a web link opens (`NoSyncOpenLinksInApp`, `SH:566-620`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkPref {
    Ask,
    Builtin,
    Default,
}

impl LinkPref {
    /// The text of the status line for this choice.
    pub fn text(self) -> &'static str {
        match self {
            LinkPref::Ask => LINK_ASK,
            LinkPref::Builtin => LINK_BUILTIN,
            LinkPref::Default => LINK_DEFAULT,
        }
    }
}

/// The two keys a link-opening choice may write, and nothing else: the silence flag and the saved
/// selection of the existing warning (`warnings::selection_key`). The setting only picks between
/// Tandem's own browser and the default browser for a web link; it names no scheme, program or path.
pub fn link_keys() -> [String; 2] {
    [crate::browser::OPEN_LINKS_ID.to_string(), crate::warnings::selection_key(crate::browser::OPEN_LINKS_ID)]
}

/// The silence flag and selection key to set or remove for `pref` (`SH:566-590`); None for a word
/// that is not one of the three.
pub fn link_pref_writes(pref: &str) -> Option<(LinkPref, Vec<(String, Option<toml::Value>)>)> {
    let [flag, selection] = link_keys();
    Some(match pref {
        "ask" => (LinkPref::Ask, vec![(flag, None), (selection, None)]),
        "builtin" => (LinkPref::Builtin, vec![(flag, Some(toml::Value::Boolean(true))), (selection, Some(toml::Value::Integer(1)))]),
        "default" => (LinkPref::Default, vec![(flag, Some(toml::Value::Boolean(true))), (selection, Some(toml::Value::Integer(0)))]),
        _ => return None,
    })
}

/// The preference the saved warning shows (`SH:593-610`): silenced with selection 1 is built-in,
/// with selection 0 default, anything else (not silenced, no selection, another value) is ask.
pub fn link_pref_of(silenced: bool, selection: Option<i64>) -> LinkPref {
    match (silenced, selection) {
        (true, Some(1)) => LinkPref::Builtin,
        (true, Some(0)) => LinkPref::Default,
        _ => LinkPref::Ask,
    }
}

// --- The copied page, adapted at load time (R13x3-1/-2 pattern) ---

/// The copy stays byte-identical on disk. The nonce policy forbids its bare `<script>` and every
/// inline `onclick` and `onchange`, which become `data-tandem-action` and `data-tandem-change`
/// attributes one delegated listener reads (trusted events only). The extension rows are built from
/// an untrusted manifest: the name, description and version go in by `textContent` and the id by a
/// data attribute, never through `innerHTML` or an inline handler (preflight B2). (name, iTerm2's
/// text, Tandem's text)
pub const PATCHES: [(&str, &str, &str); 24] = [
    ("SCRIPT", "    <script>\n        function clearCookies() {", "    <script nonce=\"{{NONCE}}\">\n        function clearCookies() {"),
    ("TANDEM", "    </script>\n</body>", "    </script>\n    <script nonce=\"{{NONCE}}\">\n        {{TANDEM_SCRIPT}}\n    </script>\n</body>"),
    ("CLEAR_COOKIES", "onclick=\"clearCookies()\"", "data-tandem-action=\"clearCookies\""),
    ("CLEAR_DATA", "onclick=\"clearAllData()\"", "data-tandem-action=\"clearAllData\""),
    ("PERMISSIONS", "onclick=\"openPermissions()\"", "data-tandem-action=\"openPermissions\""),
    ("PASSWORDS", "onclick=\"openPasswordManager()\"", "data-tandem-action=\"openPasswordManager\""),
    ("ADBLOCK_UPDATE", "onclick=\"forceAdblockUpdate()\"", "data-tandem-action=\"forceAdblockUpdate\""),
    ("EXT_SECTION", "onclick=\"toggleExtensionsSection()\"", "data-tandem-action=\"toggleExtensionsSection\""),
    ("EXT_REVEAL", "onclick=\"revealExtensionsDirectory()\"", "data-tandem-action=\"revealExtensionsDirectory\""),
    ("LINK_SELECT", "onchange=\"updateLinkOpeningPreference()\"", "data-tandem-change=\"updateLinkOpeningPreference\""),
    ("ENGINE_SELECT", "onchange=\"updateSearchEngine()\"", "data-tandem-change=\"updateSearchEngine\""),
    ("CUSTOM_SEARCH", "onchange=\"updateCustomSearchURL(this.value)\"", "data-tandem-change=\"updateCustomSearchURL\""),
    ("CUSTOM_SUGGEST", "onchange=\"updateCustomSuggestURL(this.value)\"", "data-tandem-change=\"updateCustomSuggestURL\""),
    ("ADBLOCK_TOGGLE", "onchange=\"toggleAdblock(this.checked)\"", "data-tandem-change=\"toggleAdblock\""),
    ("ADBLOCK_URL", "onchange=\"updateAdblockURL(this.value)\"", "data-tandem-change=\"updateAdblockURL\""),
    ("PROXY_TOGGLE", "onchange=\"toggleProxy(this.checked)\"", "data-tandem-change=\"toggleProxy\""),
    ("PROXY_HOST", "onchange=\"updateProxyHost(this.value)\"", "data-tandem-change=\"updateProxyHost\""),
    ("PROXY_PORT", "onchange=\"updateProxyPort(this.value)\"", "data-tandem-change=\"updateProxyPort\""),
    ("EXT_NAME", "<h4>${extension.name}</h4>", "<h4 data-tandem-text=\"name\"></h4>"),
    ("EXT_DESCRIPTION", "<p>${description}</p>", "<p data-tandem-text=\"description\"></p>"),
    ("EXT_TOGGLE", "onchange=\"toggleExtension('${extension.id}', this.checked)\"", "data-tandem-change=\"toggleExtension\""),
    ("EXT_VERSION", "<span>Version ${version}</span>", "<span data-tandem-text=\"version\"></span>"),
    (
        "EXT_FILL",
        "container.appendChild(extensionBox);",
        "for (const [k, v] of [['name', extension.name], ['description', description], ['version', 'Version ' + version]]) {\n                    extensionBox.querySelector('[data-tandem-text=\"' + k + '\"]').textContent = String(v);\n                }\n                extensionBox.querySelector('input[type=\"checkbox\"]').dataset.extensionId = String(extension.id);\n                container.appendChild(extensionBox);",
    ),
    ("EXT_FINDER", "Reveal in Finder", "Reveal in Explorer"),
];

/// Tandem's own script: one delegated listener for clicks and one for changes. An event a script
/// made up (`isTrusted` false) does nothing, so a setting changes only by the person's own click or
/// edit on the page's own control; the page's own functions then post the message. Nothing here
/// builds markup.
pub const TANDEM_SCRIPT: &str = "document.addEventListener('click', function (e) {
            if (!e.isTrusted || !e.target || !e.target.closest) { return; }
            var el = e.target.closest('[data-tandem-action]');
            if (!el || el.disabled) { return; }
            switch (el.getAttribute('data-tandem-action')) {
            case 'clearCookies': clearCookies(); break;
            case 'clearAllData': clearAllData(); break;
            case 'openPermissions': openPermissions(); break;
            case 'openPasswordManager': openPasswordManager(); break;
            case 'forceAdblockUpdate': forceAdblockUpdate(); break;
            case 'toggleExtensionsSection': toggleExtensionsSection(); break;
            case 'revealExtensionsDirectory': revealExtensionsDirectory(); break;
            }
        });
        document.addEventListener('change', function (e) {
            if (!e.isTrusted || !e.target || !e.target.closest) { return; }
            var el = e.target.closest('[data-tandem-change]');
            if (!el) { return; }
            switch (el.getAttribute('data-tandem-change')) {
            case 'updateLinkOpeningPreference': updateLinkOpeningPreference(); break;
            case 'updateSearchEngine': updateSearchEngine(); break;
            case 'updateCustomSearchURL': updateCustomSearchURL(el.value); break;
            case 'updateCustomSuggestURL': updateCustomSuggestURL(el.value); break;
            case 'toggleAdblock': toggleAdblock(el.checked); break;
            case 'updateAdblockURL': updateAdblockURL(el.value); break;
            case 'toggleProxy': toggleProxy(el.checked); break;
            case 'updateProxyHost': updateProxyHost(el.value); break;
            case 'updateProxyPort': updateProxyPort(el.value); break;
            case 'toggleExtension': toggleExtension(el.dataset.extensionId, el.checked); break;
            }
        });";

/// The page as served, or None: a patch missed (fail closed). The `SECRET` slot stays iTerm2's but
/// holds nothing: the host decides by the source.
pub fn document(state: &PageState, nonce: &str) -> Option<String> {
    use crate::browser_js::resource;
    match crate::view_source::apply_patches(resource("settings-page.html")?, &PATCHES) {
        Ok(html) => {
            let subs = substitutions(state, "");
            let mut refs: Vec<(&str, &str)> = subs.iter().map(|(k, v)| (*k, v.as_str())).collect();
            refs.push(("NONCE", nonce));
            refs.push(("TANDEM_SCRIPT", TANDEM_SCRIPT));
            Some(crate::browser_pages::page(&html, &refs))
        }
        Err(e) => {
            crate::rlog!("{}", crate::dlog::name(&e));
            None
        }
    }
}

/// Per-pane settings state (in the pane's `BrowserView`, dropped with its view).
pub struct State {
    /// A Clear All Data is waiting for the web view to finish; its `DataCleared` then erases the database.
    pub clearing: bool,
    limit: Cooldown<u8>,
}

impl Default for State {
    fn default() -> Self {
        State { clearing: false, limit: Cooldown::new(1000, 8) }
    }
}

impl State {
    /// Whether this message may act now: a clear, the password manager or a link choice at most once
    /// a second, so a page script cannot loop one (a read or a field is not limited).
    pub fn allow(&mut self, msg: &Msg, now_ms: u64) -> bool {
        let key = match msg {
            Msg::ClearCookies => 1,
            Msg::ClearAllData => 2,
            Msg::OpenPasswordManager => 3,
            Msg::SetLinkOpening(_) => 4,
            _ => return true,
        };
        if self.limit.active(&key, now_ms) {
            return false;
        }
        self.limit.start(key, now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: Value) -> Option<Msg> {
        parse(&v)
    }

    #[test]
    fn settings_privacy_follows_iterm2() {
        // The source decides, not a secret: the field is typed (a string or absent), never compared.
        assert_eq!(b(json!({"action": "clearCookies", "sessionSecret": "s"})), Some(Msg::ClearCookies));
        assert_eq!(b(json!({"action": "clearCookies"})), Some(Msg::ClearCookies));
        assert_eq!(b(json!({"action": "clearCookies", "sessionSecret": 5})), None);
        assert_eq!(b(json!({"action": "clearAllData"})), Some(Msg::ClearAllData));
        let s = "s";
        let cases = [
            (json!({"action": "setAdblockEnabled", "value": true, "sessionSecret": s}), Msg::SetAdblockEnabled(true)),
            (json!({"action": "setAdblockURL", "value": "https://a/l.json", "sessionSecret": s}), Msg::SetAdblockUrl("https://a/l.json".into())),
            (json!({"action": "setSearchCommand", "value": "https://a/?q=%@"}), Msg::SetSearchCommand("https://a/?q=%@".into())),
            (json!({"action": "setSearchSuggestURL", "value": ""}), Msg::SetSearchSuggestUrl(String::new())),
            (json!({"action": "forceAdblockUpdate"}), Msg::ForceAdblockUpdate),
            (json!({"action": "getAdblockSettings"}), Msg::GetAdblockSettings),
            (json!({"action": "getSearchSettings"}), Msg::GetSearchSettings),
            (json!({"action": "getAdblockStats"}), Msg::GetAdblockStats),
            (json!({"action": "setProxyEnabled", "value": false}), Msg::SetProxyEnabled(false)),
            (json!({"action": "setProxyHost", "value": "127.0.0.1"}), Msg::SetProxyHost("127.0.0.1".into())),
            (json!({"action": "setProxyPort", "value": 8118}), Msg::SetProxyPort(8118)),
            (json!({"action": "getProxySettings"}), Msg::GetProxySettings),
            (json!({"action": "getExtensions"}), Msg::GetExtensions),
            (json!({"action": "setExtensionEnabled", "extensionId": "abc", "enabled": true}), Msg::SetExtensionEnabled { id: "abc".into(), enabled: true }),
            (json!({"action": "revealExtensionsDirectory"}), Msg::RevealExtensionsDirectory),
            (json!({"action": "openPasswordManager"}), Msg::OpenPasswordManager),
            (json!({"action": "setLinkOpeningPreference", "value": "ask"}), Msg::SetLinkOpening("ask".into())),
            (json!({"action": "getLinkOpeningPreference"}), Msg::GetLinkOpening),
        ];
        for (body, want) in cases {
            assert_eq!(parse(&body), Some(want.clone()), "{body}");
        }
        // A wrong JSON type answers None (`as?`), as does a missing or surplus key.
        assert_eq!(b(json!({"action": "setProxyPort", "value": "8"})), None);
        assert_eq!(b(json!({"action": "setProxyPort", "value": 8.5})), None);
        assert_eq!(b(json!({"action": "setProxyPort", "value": true})), None);
        assert_eq!(b(json!({"action": "setAdblockEnabled", "value": "yes"})), None);
        assert_eq!(b(json!({"action": "setAdblockURL", "value": 3})), None);
        assert_eq!(b(json!({"action": "setAdblockURL"})), None);
        assert_eq!(b(json!({"action": "setExtensionEnabled", "extensionId": "abc"})), None);
        assert_eq!(b(json!({"action": "setExtensionEnabled", "enabled": true})), None);
        assert_eq!(b(json!({"action": "setExtensionEnabled", "extensionId": 7, "enabled": true})), None);
        assert_eq!(b(json!({"action": "clearCookies", "key": "BrowserProxyHost"})), None, "no extra key");
        assert_eq!(b(json!({"action": "setProxyHost", "value": "h", "key": "k"})), None);
        assert_eq!(b(json!({"action": "setGlobal", "value": "h"})), None);
        assert_eq!(b(json!({"action": 5})), None);
        assert_eq!(b(json!("clearCookies")), None);
        // Bounded: a string over the cap is refused whole.
        assert_eq!(b(json!({"action": "setAdblockURL", "value": "x".repeat(MAX_VALUE + 1)})), None);
        assert!(b(json!({"action": "setAdblockURL", "value": "x".repeat(MAX_VALUE)})).is_some());
        assert_eq!(b(json!({"action": "setExtensionEnabled", "extensionId": "a".repeat(MAX_ID + 1), "enabled": true})), None);

        let state = PageState { adblock_enabled: true, adblock_url: "a&b\"c".into(), dev_null: false, show_extensions: false };
        let subs = substitutions(&state, "sec");
        let get = |k: &str| subs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!((get("ADBLOCK_ENABLED").as_str(), get("ADBLOCK_URL").as_str(), get("SECRET").as_str()), ("checked", "a&amp;b&quot;c", "sec"));
        assert_eq!((get("DEV_NULL_NOTE").as_str(), get("SHOW_EXTENSIONS").as_str()), ("display: none;", "display: none;"));
        let on = substitutions(&PageState { adblock_enabled: false, adblock_url: "{{NONCE}}<x>".into(), dev_null: true, show_extensions: true }, "");
        let get = |k: &str| on.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!((get("ADBLOCK_ENABLED").as_str(), get("DEV_NULL_NOTE").as_str(), get("SHOW_EXTENSIONS").as_str()), ("", "", ""));
        assert_eq!(get("ADBLOCK_URL"), "&#123;&#123;NONCE}}&lt;x&gt;", "no slot or tag survives in a value");

        assert_eq!(DATA_CLEARED, "All website data has been cleared successfully!");
        assert_eq!(db_delete_failed(Some(Path::new(r"C:\a\browserdb-x.sqlite"))), r"The browser database could not be deleted. It is in C:\a\browserdb-x.sqlite");
        assert_eq!(db_delete_failed(None), "The browser database could not be deleted. Your application support folder could not be found.");
        // Replies carry values as JSON (preflight B9), never pasted text.
        assert_eq!(alert_script("It's C:\\a"), "alert(\"It's C:\\\\a\");");
        assert_eq!(status_script("Can't", "error"), "showStatus(\"Can't\", \"error\");");
        assert_eq!(status_script("a\"b\\c\nd</script>", "x"), "showStatus(\"a\\\"b\\\\c\\nd</script>\", \"x\");");
        assert_eq!(link_ui_script(LinkPref::Builtin), "updateLinkOpeningUI({\"preference\":\"builtin\"});");
        assert!(crate::browser_js::resource("settings-page.html").unwrap().contains("Reveal in Finder"), "the copy is iTerm2's");
        let served = document(&state, "n0nce").unwrap();
        assert!(served.contains("Reveal in Explorer") && !served.contains("Finder"));
    }

    #[test]
    fn link_opening_follows_iterm2() {
        let flag = crate::browser::OPEN_LINKS_ID;
        let sel = format!("{flag}_selection");
        assert_eq!(flag, "NoSyncOpenLinksInApp");
        assert_eq!(link_pref_writes("ask"), Some((LinkPref::Ask, vec![(flag.into(), None), (sel.clone(), None)])));
        assert_eq!(link_pref_writes("builtin"), Some((LinkPref::Builtin, vec![(flag.into(), Some(toml::Value::Boolean(true))), (sel.clone(), Some(toml::Value::Integer(1)))])));
        assert_eq!(link_pref_writes("default"), Some((LinkPref::Default, vec![(flag.into(), Some(toml::Value::Boolean(true))), (sel.clone(), Some(toml::Value::Integer(0)))])));
        for bad in ["x", "", "Ask", "ask ", "builtin\0", "file", "ssh:", "C:\\Windows\\system32\\cmd.exe"] {
            assert_eq!(link_pref_writes(bad), None, "{bad:?} writes nothing");
        }
        // A choice writes the two keys of the warning and nothing else, whatever the word.
        let keys = link_keys();
        for word in ["ask", "builtin", "default"] {
            for (k, v) in link_pref_writes(word).unwrap().1 {
                assert!(keys.contains(&k), "{k}");
                assert!(matches!(v, None | Some(toml::Value::Boolean(true)) | Some(toml::Value::Integer(0 | 1))));
            }
        }
        assert_eq!(link_pref_of(false, Some(1)), LinkPref::Ask);
        assert_eq!(link_pref_of(true, Some(1)), LinkPref::Builtin);
        assert_eq!(link_pref_of(true, Some(0)), LinkPref::Default);
        assert_eq!(link_pref_of(true, Some(2)), LinkPref::Ask);
        assert_eq!(link_pref_of(true, None), LinkPref::Ask);
        assert_eq!((LINK_ASK, LINK_BUILTIN, LINK_DEFAULT, LINK_INVALID), ("Link opening preference set to always ask", "Links will always open in built-in browser", "Links will always open in default browser", "Invalid link opening preference"));
        assert_eq!((LinkPref::Ask.text(), LinkPref::Builtin.text(), LinkPref::Default.text()), (LINK_ASK, LINK_BUILTIN, LINK_DEFAULT));
        // The round trip through the stored values.
        for word in ["ask", "builtin", "default"] {
            let (pref, writes) = link_pref_writes(word).unwrap();
            let silenced = writes.iter().any(|(k, v)| k == flag && v.is_some());
            let selection = writes.iter().find_map(|(k, v)| match (k == &sel, v) {
                (true, Some(toml::Value::Integer(n))) => Some(*n),
                _ => None,
            });
            assert_eq!(link_pref_of(silenced, selection), pref);
        }
    }

    #[test]
    fn the_page_is_a_patched_copy_under_the_nonce_policy() {
        let raw = crate::browser_js::resource("settings-page.html").unwrap();
        for (n, target, _) in PATCHES.iter() {
            assert_eq!(raw.matches(target).count(), 1, "{n}");
            assert!(crate::view_source::apply_patches(&format!("{raw}{target}"), &PATCHES).is_err(), "a second copy of {n} is a miss");
            assert!(crate::view_source::apply_patches(&raw.replace(target, "x"), &PATCHES).is_err(), "a missing {n} serves no page");
        }
        let state = PageState { adblock_enabled: false, adblock_url: "https://a/".into(), dev_null: false, show_extensions: false };
        let html = document(&state, "n0nce").expect("served");
        assert!(!html.contains("onclick=") && !html.contains("onchange=") && !html.contains("{{"), "no inline handler, no unfilled slot");
        assert_eq!(html.matches("<script").count(), 2);
        assert_eq!(html.matches("<script nonce=\"n0nce\">").count(), 2, "every script has the nonce");
        assert!(html.contains("const sessionSecret = \"\";") && html.contains("e.isTrusted"));
        // Manifest text never reaches markup (B2): nothing interpolates it into innerHTML.
        assert!(!html.contains("${extension.name}") && !html.contains("${description}") && !html.contains("${version}") && !html.contains("${extension.id}"));
        assert!(html.contains("textContent = String(v)") && html.contains("dataset.extensionId"));
        assert!(!html.contains("iTerm2"), "visible names are Tandem's");
        // Every inline handler of the copy is accounted for by a patch.
        let handlers = raw.matches("onclick=\"").count() + raw.matches("onchange=\"").count();
        assert_eq!(handlers, PATCHES.iter().filter(|(_, t, _)| t.starts_with("onclick=\"") || t.starts_with("onchange=\"")).count());
        // The dev-null note shows only in a dev-null pane.
        let dn = document(&PageState { dev_null: true, ..state.clone() }, "n").unwrap();
        assert!(dn.contains("dev-null-note\" style=\"\"") && html.contains("dev-null-note\" style=\"display: none;\""));
    }

    #[test]
    fn privileged_actions_are_rate_limited() {
        let mut s = State::default();
        assert!(s.allow(&Msg::ClearAllData, 10_000));
        assert!(!s.allow(&Msg::ClearAllData, 10_500), "a second clear within a second");
        assert!(s.allow(&Msg::ClearCookies, 10_500), "another action has its own window");
        assert!(s.allow(&Msg::OpenPasswordManager, 10_500));
        assert!(!s.allow(&Msg::OpenPasswordManager, 10_900));
        assert!(s.allow(&Msg::GetLinkOpening, 10_900) && s.allow(&Msg::GetLinkOpening, 10_901), "reads are not limited");
        assert!(s.allow(&Msg::ClearAllData, 11_100));
    }

    #[test]
    fn no_reply_pastes_text_into_a_script() {
        let src = include_str!("browser_settings_page.rs");
        let live = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!live.contains("'{}'") && !live.contains("\\'{"), "values go in as JSON strings");
    }
}
