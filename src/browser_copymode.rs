//! Copy mode on a web page (milestone 13x3, Task 8): iTerm2's page cursor (`copy-mode.js`) and the
//! keys that drive it (`iTermBrowserCopyModeHandler.swift`, `iTermBrowserCopyModeState.swift`). The
//! scripts run only in Tandem's isolated world. The keys are the terminal's: `copymode::action_for`
//! is the one table, this module maps its actions to the page's API. Pure: the app runs the steps.

use serde_json::Value;

use crate::browser_js::{self, Inject, UserScript, World};
use crate::browser_world;
use crate::copymode::{self, Action};

/// The user script's identifier (`BM:347`).
pub const SCRIPT_ID: &str = "CopyMode";
/// Tandem's own handler for the keys script (not iTerm2's: iTerm2 reads keys natively, `BM:870-879`).
pub const KEY_HANDLER: &str = "tandemCopyModeKey";

/// The five files copied from `CopyMode/`; each is registered in `browser_js::resource`.
pub const FILES: &[&str] = &["copy-mode.js", "copy-mode-util.js", "copy-mode-cursor.js", "copy-mode-cursor-movement.js", "copy-mode-impl.js"];

/// The most times one counted action is repeated in one script (the count itself is capped at
/// 100000 by `copymode::accrue`).
pub const MAX_REPEAT: usize = 1000;

/// The page stops selecting (Quit, `CPH:441-444`); the selection itself stays (`CMI:75-88`).
pub const STOP_SELECTING_JS: &str = "window.iTerm2CopyMode.selecting = false;";

/// What a key does to the page and to Tandem.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Nothing,
    /// A statement list for the main frame's Tandem world (`browser_js_call`).
    Script(String),
    /// Copy the page's selection through Tandem's clipboard, then leave copy mode.
    Copy,
    Leave { stop_selecting: bool },
    /// Open the find bar.
    Find,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub selecting: bool,
    pub mode: u8,
    count: usize,
}

/// A secret goes into a JS string literal only when it is plain letters and digits (the view's is 32
/// hex characters): nothing it holds can end the literal.
fn plain(secret: &str) -> bool {
    !secret.is_empty() && secret.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `BCH:61-65`: `copy-mode.js` with its includes expanded and the secret filled. An include left in
/// place is an error, not a silent gap.
pub fn script(secret: &str) -> Result<String, String> {
    if !plain(secret) {
        return Err("the copy mode secret is not plain letters and digits".into());
    }
    let src = browser_js::resource("copy-mode.js").ok_or("copy-mode.js is not embedded")?;
    let out = browser_js::fill_template(src, &[("SECRET", secret)]);
    match out.find("{{") {
        Some(at) => Err(format!("copy mode script has an unfilled slot: {}", out[at..].chars().take(40).collect::<String>())),
        None => Ok(out),
    }
}

/// iTerm2's cursor script as a user script: the isolated world (`window.iTerm2CopyMode` does not
/// exist in the page's world), every frame, at document start (`BM:341-348`).
pub fn user_script(secret: &str) -> Result<UserScript, String> {
    script(secret).map(|code| UserScript { id: SCRIPT_ID, code, inject: Inject::DocumentStart, main_frame_only: false, world: World::Client })
}

/// Tandem's key listener, filled with the view's secret. A capture-phase `keydown` listener on
/// `window`: an event that is not trusted is ignored first of all; while the world-local switch is on
/// (only `window.__tandemCopyModeKeys(secret, on)` in this world sets it) every key without Win, and
/// not part of an IME composition, is kept from the page and reported to `KEY_HANDLER`.
const KEYS_JS: &str = r#"// Tandem's own script (not from iTerm2): the keys of copy mode in a web page.
(function() {
    'use strict';
    try {
        const SECRET = "{{SECRET}}";
        let on = false;
        // At most 200 posts a second.
        const win = { start: 0, count: 0 };
        function post(body) {
            const now = Date.now();
            if (now - win.start >= 1000) {
                win.start = now;
                win.count = 0;
            }
            if (win.count >= 200) {
                return;
            }
            win.count += 1;
            window.webkit.messageHandlers.tandemCopyModeKey.postMessage(body);
        }
        window.addEventListener('keydown', function(e) {
            if (!e.isTrusted) { return; }
            if (!on) { return; }
            if (e.metaKey) { return; }
            if (e.isComposing || e.keyCode === 229) { return; }
            if (e.key === 'F10' || (e.altKey && e.key === 'F4')) { return; }
            e.preventDefault();
            e.stopImmediatePropagation();
            post({ key: String(e.key).slice(0, 32), ctrl: e.ctrlKey === true, alt: e.altKey === true, shift: e.shiftKey === true });
        }, true);
        // The release of a key copy mode took is kept from the page too, so it cannot see which
        // keys were pressed. Same checks; a release is never reported.
        window.addEventListener('keyup', function(e) {
            if (!e.isTrusted) { return; }
            if (!on) { return; }
            if (e.metaKey) { return; }
            if (e.isComposing || e.keyCode === 229) { return; }
            if (e.key === 'F10' || (e.altKey && e.key === 'F4')) { return; }
            e.preventDefault();
            e.stopImmediatePropagation();
        }, true);
        Object.defineProperty(window, '__tandemCopyModeKeys', {
            value: function(secret, value) {
                if (secret !== SECRET) { return; }
                on = value === true;
            }
        });
    } catch (x) {}
})();
"#;

/// The key listener as a Client-world user script in every frame, at document start.
pub fn keys_script(secret: &str) -> Result<UserScript, String> {
    if !plain(secret) {
        return Err("the copy mode secret is not plain letters and digits".into());
    }
    let code = browser_js::fill_template(KEYS_JS, &[("SECRET", secret)]);
    Ok(UserScript { id: "TandemCopyModeKeys", code, inject: Inject::DocumentStart, main_frame_only: false, world: World::Client })
}

/// Turns the key listener on or off in one Tandem world (the app runs it in every live world and in
/// each new one). Nothing when the secret is not plain: no script is built from it.
pub fn keys_on_js(secret: &str, on: bool) -> String {
    if !plain(secret) {
        return String::new();
    }
    format!("window.__tandemCopyModeKeys && window.__tandemCopyModeKeys('{secret}', {on});")
}

/// The `Runtime.evaluate` parameters that tell a Tandem world made mid-session that copy mode is on
/// (`Runtime.executionContextCreated`'s JSON): the one composition the app sends for a late iframe
/// and the probe sends for the same event. None for a context that is not Tandem's isolated world
/// or a secret that is not plain.
pub fn new_context_params(secret: &str, context_json: &str) -> Option<serde_json::Value> {
    let js = keys_on_js(secret, true);
    if js.is_empty() {
        return None;
    }
    let unique = browser_world::isolated_unique(context_json)?;
    Some(browser_world::evaluate_params(&unique, &js))
}

/// `BCH:31-39`.
pub fn enable_js(secret: &str) -> String {
    if !plain(secret) {
        return String::new();
    }
    format!("window.iTerm2CopyMode.enable('{secret}');")
}

pub fn disable_js(secret: &str) -> String {
    if !plain(secret) {
        return String::new();
    }
    format!("window.iTerm2CopyMode.disable('{secret}');")
}

/// The page's `KeyboardEvent.key` as the copy-mode key names of `copymode_ui::key_name`: the
/// character typed, "C-x" for Ctrl with a letter, or the key's name. AltGr typing arrives as
/// Ctrl+Alt and gives the character itself. None for keys copy mode has no use for.
pub fn key_name(key: &str, ctrl: bool, alt: bool) -> Option<String> {
    let name = match key {
        " " if ctrl => "C-Space",
        " " => " ",
        "Escape" => "Esc",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "Home" | "End" | "PageUp" | "PageDown" => key,
        _ => {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            if c.is_control() {
                return None;
            }
            return Some(if ctrl && !alt { format!("C-{}", c.to_lowercase()) } else { c.to_string() });
        }
    };
    Some(name.to_string())
}

/// The three facts of a `KEY_HANDLER` body, or None for any other shape: `{key, ctrl, alt, shift}`
/// with a string key of at most 32 characters and three booleans (`shift` is read but copy mode's
/// names carry it in the character).
pub fn key_of(body: &Value) -> Option<(String, bool, bool)> {
    let o = body.as_object()?;
    let key = o.get("key")?.as_str().filter(|k| !k.is_empty() && k.chars().count() <= 32)?;
    let (ctrl, alt, _shift) = (o.get("ctrl")?.as_bool()?, o.get("alt")?.as_bool()?, o.get("shift")?.as_bool()?);
    Some((key.to_string(), ctrl, alt))
}

/// The same names from a virtual key and Tandem's modifier bits (the key gate, which sees keys before
/// the page): named keys by code, Ctrl with a letter as "C-x", else the character the layout types.
pub fn key_name_vk(vk: u32, mods: u8) -> Option<String> {
    use crate::browser_keys::{ALT, CTRL, WIN};
    if mods & WIN != 0 {
        return None;
    }
    let ctrl = mods & CTRL != 0;
    let alt = mods & ALT != 0;
    match vk {
        0x1B => return Some("Esc".into()),
        0x25 => return Some("Left".into()),
        0x26 => return Some("Up".into()),
        0x27 => return Some("Right".into()),
        0x28 => return Some("Down".into()),
        0x24 => return Some("Home".into()),
        0x23 => return Some("End".into()),
        0x21 => return Some("PageUp".into()),
        0x22 => return Some("PageDown".into()),
        0x20 => return Some(if ctrl { "C-Space" } else { " " }.into()),
        0x41..=0x5A if ctrl && !alt => return Some(format!("C-{}", (vk as u8 + 32) as char)),
        _ => {}
    }
    let text = crate::browser_keys::layout_text(vk, mods)?;
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    (!c.is_control()).then(|| c.to_string())
}

/// The page API name of an action (`BCMS:199-301`); None for an action that is not a page move
/// (copy, leave, find, the toggles, none).
pub fn method(a: Action) -> Option<&'static str> {
    Some(match a {
        Action::MoveBackwardWord => "moveBackwardWord",
        Action::MoveForwardWord => "moveForwardWord",
        Action::MoveBackwardBigWord => "moveBackwardBigWord",
        Action::MoveForwardBigWord => "moveForwardBigWord",
        Action::MoveLeft => "moveLeft",
        Action::MoveRight => "moveRight",
        Action::MoveUp => "moveUp",
        Action::MoveDown => "moveDown",
        Action::MoveToStartOfNextLine => "moveToStartOfNextLine",
        Action::PageUp => "pageUp",
        Action::PageDown => "pageDown",
        Action::PageUpHalfScreen => "pageUpHalfScreen",
        Action::PageDownHalfScreen => "pageDownHalfScreen",
        Action::PreviousMark => "previousMark",
        Action::NextMark => "nextMark",
        Action::MoveToStart => "moveToStart",
        Action::MoveToEnd => "moveToEnd",
        Action::MoveToStartOfIndentation => "moveToStartOfIndentation",
        Action::MoveToBottomOfVisibleArea => "moveToBottomOfVisibleArea",
        Action::MoveToMiddleOfVisibleArea => "moveToMiddleOfVisibleArea",
        Action::MoveToTopOfVisibleArea => "moveToTopOfVisibleArea",
        Action::MoveToStartOfLine | Action::MoveToStartOfLineOrAccrue0 => "moveToStartOfLine",
        Action::MoveToEndOfLine => "moveToEndOfLine",
        Action::Swap => "swap",
        Action::ScrollUp => "scrollUp",
        Action::ScrollDown => "scrollDown",
        _ => return None,
    })
}

impl Model {
    /// One key, as iTerm2's shared handler reads it (`CPH:377-484`): a count digit accrues, an action
    /// repeats count times (`CPH:347-371`) and then the page reveals its cursor (`BCH:92-99`, every
    /// browser move answers moved, `BCMS:177-194`). A secret that is not plain builds nothing.
    pub fn key(&mut self, key: &str, secret: &str) -> Step {
        if !plain(secret) {
            return Step::Nothing;
        }
        if copymode::accrue(&mut self.count, key) {
            return Step::Nothing;
        }
        let n = std::mem::take(&mut self.count).clamp(1, MAX_REPEAT);
        let action = copymode::action_for(key);
        let reveal = format!("iTerm2CopyMode.scrollCursorIntoView('{secret}');");
        let toggle = |this: &mut Model, mode: u8| {
            this.selecting = !this.selecting;
            this.mode = mode;
            format!("iTerm2CopyMode.selecting = {}; iTerm2CopyMode.mode = {};", this.selecting, mode)
        };
        let body = match action {
            Action::None => return Step::Nothing,
            Action::CopySelection => return Step::Copy,
            Action::ExitCopyMode => return Step::Leave { stop_selecting: false },
            Action::Quit => return Step::Leave { stop_selecting: true },
            Action::ShowFindPanel => return Step::Find,
            // The three toggles flip selecting and set the mode (0 character, 2 line, 4 box, `CMU:1-7`).
            Action::ToggleCharacterSelection => toggle(self, 0),
            Action::ToggleLineSelection => toggle(self, 2),
            Action::ToggleBoxSelection => toggle(self, 4),
            other => match method(other) {
                Some(m) => format!("iTerm2CopyMode.{m}('{secret}'); ").repeat(n),
                None => return Step::Nothing,
            },
        };
        Step::Script(format!("{body} {reveal}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script_of(s: Step) -> String {
        match s {
            Step::Script(t) => t,
            other => panic!("not a script: {other:?}"),
        }
    }

    /// The late-iframe switch (final wave): Tandem's isolated context gets the keys-on script by
    /// unique id; any other context, or a secret that is not plain, gets nothing.
    #[test]
    fn a_late_world_is_told_by_its_unique_id() {
        let ev = |name: &str, kind: &str| format!(r#"{{"context":{{"id":7,"name":"{name}","uniqueId":"u-9","auxData":{{"type":"{kind}"}}}}}}"#);
        let p = new_context_params("abc", &ev(browser_world::WORLD_NAME, "isolated")).expect("Tandem's world");
        assert_eq!(p["uniqueContextId"], "u-9");
        assert_eq!(p["expression"], keys_on_js("abc", true));
        assert!(new_context_params("abc", &ev("page", "default")).is_none(), "a page context");
        assert!(new_context_params("abc", &ev(browser_world::WORLD_NAME, "default")).is_none(), "named like Tandem's but not isolated");
        assert!(new_context_params("a'b", &ev(browser_world::WORLD_NAME, "isolated")).is_none(), "a secret that is not plain");
        assert!(new_context_params("abc", "not json").is_none());
    }

    /// 13x3 final wave (Task 8 minor 10): a huge count repeats MAX_REPEAT times, not 5000.
    #[test]
    fn a_count_is_capped_at_max_repeat() {
        let mut m = Model::default();
        for d in ["5", "0", "0", "0"] {
            assert_eq!(m.key(d, "s"), Step::Nothing);
        }
        let j = script_of(m.key("j", "s"));
        assert_eq!(j.matches("iTerm2CopyMode.moveDown('s')").count(), MAX_REPEAT);
        assert_eq!(j.matches("scrollCursorIntoView('s')").count(), 1);
    }

    #[test]
    fn browser_copy_steps_follow_iterm2() {
        let mut m = Model::default();
        let j = script_of(m.key("j", "s"));
        assert_eq!(j.matches("iTerm2CopyMode.moveDown('s')").count(), 1);
        assert!(j.find("moveDown('s')").unwrap() < j.find("iTerm2CopyMode.scrollCursorIntoView('s')").unwrap(), "every action reveals the cursor");
        assert_eq!(m.key("3", "s"), Step::Nothing);
        let w = script_of(m.key("w", "s"));
        assert_eq!(w.matches("iTerm2CopyMode.moveForwardWord('s')").count(), 3, "a count repeats the move");
        assert_eq!(w.matches("scrollCursorIntoView('s')").count(), 1, "and reveals once");
        let v = script_of(m.key("v", "s"));
        assert!(v.contains("iTerm2CopyMode.selecting = true") && v.contains("iTerm2CopyMode.mode = 0"));
        assert!(m.selecting && m.mode == 0);
        let big = script_of(m.key("V", "s"));
        assert!(big.contains("iTerm2CopyMode.mode = 2"));
        let boxed = script_of(m.key("C-v", "s"));
        assert!(boxed.contains("iTerm2CopyMode.mode = 4"));
        assert!(script_of(m.key("o", "s")).contains("iTerm2CopyMode.swap('s')"));
        assert!(script_of(m.key("C-y", "s")).contains("iTerm2CopyMode.scrollUp('s')"));
        assert!(script_of(m.key("C-e", "s")).contains("iTerm2CopyMode.scrollDown('s')"));
        assert!(script_of(m.key("[", "s")).contains("iTerm2CopyMode.previousMark('s')"));
        assert!(script_of(m.key("]", "s")).contains("iTerm2CopyMode.nextMark('s')"));
        assert_eq!(m.key("y", "s"), Step::Copy);
        assert_eq!(m.key("q", "s"), Step::Leave { stop_selecting: true });
        assert_eq!(m.key("Esc", "s"), Step::Leave { stop_selecting: true });
        assert_eq!(m.key("C-c", "s"), Step::Leave { stop_selecting: false });
        assert_eq!(m.key("/", "s"), Step::Find);
        assert_eq!(m.key("x", "s"), Step::Nothing);
        assert_eq!(method(Action::MoveToStartOfIndentation), Some("moveToStartOfIndentation"));
        assert_eq!(method(Action::Quit), None);
        // Toggling again turns selecting off.
        let mut t = Model::default();
        t.key("v", "s");
        let off = script_of(t.key("v", "s"));
        assert!(off.contains("iTerm2CopyMode.selecting = false") && !t.selecting);
        // Every action table entry that moves has a page method.
        for key in ["h", "l", "k", "j", "w", "W", "b", "B", "0", "^", "$", "g", "G", "H", "M", "L", "C-b", "C-f", "C-u", "C-d"] {
            assert!(method(copymode::action_for(key)).is_some(), "{key}");
        }
        // The page scripts.
        assert_eq!(enable_js("s"), "window.iTerm2CopyMode.enable('s');");
        assert_eq!(disable_js("s"), "window.iTerm2CopyMode.disable('s');");
        let full = script("s").unwrap();
        assert!(full.contains("const secret = 's';") && !full.contains("{{"));
        assert!(user_script("s").unwrap().world == World::Client && keys_script("s").unwrap().world == World::Client);
        assert!(!keys_script("s").unwrap().main_frame_only && !user_script("s").unwrap().main_frame_only);
        assert_eq!(keys_script("s").unwrap().inject, Inject::DocumentStart);
        let keys = keys_script("s").unwrap().code;
        let listener = &keys[keys.find("addEventListener('keydown'").unwrap()..];
        assert!(listener.find("isTrusted").unwrap() < listener.find("preventDefault").unwrap() && listener.find("isTrusted").unwrap() < listener.find("post(").unwrap());
        assert!(listener.contains("e.metaKey") && listener.contains("stopImmediatePropagation") && listener.contains("true)"), "capture phase");
        assert!(keys.contains(KEY_HANDLER));
        // The matching keyup is kept from the page too (Task 8 minor 5): same checks, never posts.
        let up = &keys[keys.find("addEventListener('keyup'").expect("a keyup listener")..];
        let up = &up[..up.find("}, true);").unwrap()];
        assert!(up.find("isTrusted").unwrap() < up.find("preventDefault").unwrap() && up.contains("stopImmediatePropagation") && up.contains("e.metaKey") && up.contains("!on"));
        assert!(!up.contains("post("), "a release is never reported");
        // Alt+F4 and F10 are the host's (the gate lets them through): neither listener touches them.
        let sys = "e.key === 'F10' || (e.altKey && e.key === 'F4')";
        assert!(listener.find(sys).unwrap() < listener.find("preventDefault").unwrap() && up.find(sys).unwrap() < up.find("preventDefault").unwrap());
        // A secret that is not plain letters and digits builds no script and no step.
        assert!(script("a'b").is_err() && user_script("").is_err() && keys_script("a b").is_err());
        assert_eq!(Model::default().key("j", "a'b"), Step::Nothing);
        assert!(!keys_on_js("s", true).is_empty() && keys_on_js("s", true).contains("true") && keys_on_js("s", false).contains("false"));
    }

    #[test]
    fn copy_key_names_follow_windows() {
        assert_eq!(key_name("Escape", false, false).as_deref(), Some("Esc"));
        for (k, n) in [("ArrowLeft", "Left"), ("ArrowRight", "Right"), ("ArrowUp", "Up"), ("ArrowDown", "Down"), ("Home", "Home"), ("End", "End"), ("PageUp", "PageUp"), ("PageDown", "PageDown")] {
            assert_eq!(key_name(k, false, false).as_deref(), Some(n), "{k}");
        }
        assert_eq!(key_name(" ", true, false).as_deref(), Some("C-Space"));
        assert_eq!(key_name(" ", false, false).as_deref(), Some(" "));
        assert_eq!(key_name("b", true, false).as_deref(), Some("C-b"));
        assert_eq!(key_name("B", true, false).as_deref(), Some("C-b"));
        assert_eq!(key_name("@", true, true).as_deref(), Some("@"), "AltGr arrives as Ctrl+Alt");
        assert_eq!(key_name("j", false, false).as_deref(), Some("j"));
        assert_eq!(key_name("Shift", false, false), None);
        assert_eq!(key_name("F5", false, false), None);
        assert_eq!(key_name("Tab", false, false), None);
        assert_eq!(key_name("", false, false), None);
    }

    #[test]
    fn keys_arrive_typed() {
        use serde_json::json;
        assert_eq!(key_of(&json!({"key": "j", "ctrl": false, "alt": false, "shift": false})), Some(("j".into(), false, false)));
        assert_eq!(key_of(&json!({"key": "b", "ctrl": true, "alt": false, "shift": true})), Some(("b".into(), true, false)));
        // Anything but the exact shape is dropped.
        assert_eq!(key_of(&json!({"key": "j", "ctrl": "no", "alt": false, "shift": false})), None);
        assert_eq!(key_of(&json!({"key": "j", "ctrl": false, "alt": false})), None);
        assert_eq!(key_of(&json!({"key": 5, "ctrl": false, "alt": false, "shift": false})), None);
        assert_eq!(key_of(&json!({"key": "x".repeat(33), "ctrl": false, "alt": false, "shift": false})), None);
        assert_eq!(key_of(&json!("j")), None);
        // The gate's names: named keys by code, Ctrl letters, Win is never copy mode's.
        use crate::browser_keys::{CTRL, WIN};
        assert_eq!(key_name_vk(0x1B, 0).as_deref(), Some("Esc"));
        assert_eq!(key_name_vk(0x25, 0).as_deref(), Some("Left"));
        assert_eq!(key_name_vk(0x22, 0).as_deref(), Some("PageDown"));
        assert_eq!(key_name_vk(0x20, CTRL).as_deref(), Some("C-Space"));
        assert_eq!(key_name_vk(0x20, 0).as_deref(), Some(" "));
        assert_eq!(key_name_vk(0x42, CTRL).as_deref(), Some("C-b"));
        assert_eq!(key_name_vk(0x42, WIN), None);
        assert_eq!(key_name_vk(0x70, 0), None, "F1 types nothing");
    }

    /// The key listener and the switch exist only in the isolated world: both scripts are Client,
    /// neither is ever a page-world script, and the page has no name to call (the property is the
    /// world's own and takes the secret).
    #[test]
    fn copy_mode_scripts_never_run_in_the_page() {
        for s in [user_script("abc").unwrap(), keys_script("abc").unwrap()] {
            assert_eq!(s.world, World::Client);
        }
        let k = keys_script("abc").unwrap().code;
        assert!(k.contains("if (secret !== SECRET) { return; }") && k.contains("const SECRET = \"abc\";"));
        assert!(k.contains("Object.defineProperty(window, '__tandemCopyModeKeys'"), "not writable, not enumerable, no way to swap it");
        assert!(!k.contains("localStorage") && !k.contains("document.cookie"));
    }
}
