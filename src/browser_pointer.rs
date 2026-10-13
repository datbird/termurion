//! Pointer bindings in a page (milestone 13x2, Task 7). Pure. Web content is untrusted: a press
//! reaches Tandem only from Tandem's own gate script (`resources/browser/pointer-gate.js`), which
//! runs in the isolated world and ignores every event whose `isTrusted` is false, so a page that
//! dispatches a synthetic press runs no binding. This module builds the table that script reads,
//! reads its messages, and classifies what a binding does in a page (`PAP`, `BVC:1474-1600`).

use serde_json::Value;

use crate::pointer::{self, Down, Mods, Prefs};

/// The handler name of the gate script's messages.
pub const GATE_HANDLER: &str = "termurionPointer";

/// The most a coordinate of a press may be (page pixels); a larger value is not a press.
const MAX_COORD: f64 = 10_000_000.0;

/// The right button's number in iTerm2's numbering (0 left, 1 right, 2 middle).
const RIGHT: u8 = 1;

/// JavaScript's button (0 left, 1 middle, 2 right) as iTerm2's (0, 2, 1); 255 for any other.
pub fn button_of_js(js_button: i64) -> u8 {
    match js_button {
        0 => 0,
        1 => 2,
        2 => 1,
        // Back, Forward and beyond keep their numbers (iTerm2's otherMouseDown buttons 3+).
        3..=31 => js_button as u8,
        _ => 255,
    }
}

/// What a pointer action does in a page (`PAP:13-173`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerAct {
    /// Open the link under the pointer, in place or in a new tab (`BVC:1478-1481`).
    OpenLink { new_tab: bool },
    SmartSelect,
    ContextMenu,
    ExtendSelection,
    QuickLook,
    CopyLinkAddress,
    PasteClipboard,
    PasteSelection,
    CopyOrPaste,
    SendText,
    /// The session's own: tabs, windows, panes, profiles, move pane, menu items, script functions.
    Session,
    Nothing,
}

/// The action of a binding in a page. Send Escape Sequence and Ignore do nothing (`PAP:87-88`);
/// the paste actions ignore their configuration (`PAP:13-17`).
pub fn classify(action: &str) -> PointerAct {
    use PointerAct::*;
    match action {
        "kOpenTargetPointerAction" => OpenLink { new_tab: false },
        "kOpenTargetInBackgroundPointerAction" => OpenLink { new_tab: true },
        "kSmartSelectionPointerAction" | "kSmartSelectionIgnoringNewlinesPointerAction" => SmartSelect,
        "kContextMenuPointerAction" => ContextMenu,
        "kExtendSelectionPointerAction" => ExtendSelection,
        "kQuickLookAction" => QuickLook,
        "kCopyLinkAddressPointerAction" => CopyLinkAddress,
        "kPasteFromClipboardPointerAction" => PasteClipboard,
        "kPasteFromSelectionPointerAction" => PasteSelection,
        "kCopyOrPastePointerAction" => CopyOrPaste,
        "kSendTextPointerAction" | "kSendHexCodePointerAction" => SendText,
        "kNextTabPointerAction"
        | "kPrevTabPointerAction"
        | "kNextWindowPointerAction"
        | "kPrevWindowPointerAction"
        | "kMovePanePointerAction"
        | "kSelectPaneLeftPointerAction"
        | "kSelectPaneRightPointerAction"
        | "kSelectPaneAbovePointerAction"
        | "kSelectPaneBelowPointerAction"
        | "kNewWindowWithProfilePointerAction"
        | "kNewTabWithProfilePointerAction"
        | "kNewVerticalSplitWithProfilePointerAction"
        | "kNewHorizontalSplitWithProfilePointerAction"
        | "kSelectMenuItemPointerAction"
        | "kInvokeScriptFunction"
        | "kSelectNextPanePointerAction"
        | "kSelectPreviousPanePointerAction" => Session,
        _ => Nothing,
    }
}

/// The keys of the pointer map the page must yield to Tandem (`BWV:459-490`, `:604-631`): every
/// binding on the right or the middle button (or another) that `pointer::on_down` takes with no
/// app reporting, plus plain Ctrl single left when it stands for a right click and the right click is
/// bound. Left clicks stay the page's (`BWV:383-433`). The right button's own Open Context Menu is
/// left to the page: WebView2 shows its own menu with Tandem's items (Task 3), which is that action.
pub fn gate_keys(map: &toml::Table, prefs: &Prefs) -> Vec<String> {
    let mut keys = Vec::new();
    for (k, v) in map {
        let Some((button, clicks, mods)) = pointer::parse_key(k) else { continue };
        if !(1..=31).contains(&button) || !(1..=4).contains(&clicks) {
            continue;
        }
        if pointer::on_down(button, clicks, mods, false, false, map, prefs) != Down::Bound {
            continue;
        }
        let own_menu = button == RIGHT && pointer::PointerBinding::from_value(v).is_some_and(|b| classify(&b.action) == PointerAct::ContextMenu);
        if !own_menu {
            keys.push(pointer::key(button, clicks, mods));
        }
    }
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    if pointer::emulates_right_click(0, 1, ctrl, false, prefs) && pointer::lookup(map, 0, 1, ctrl, false, false, prefs).is_some() {
        keys.push(pointer::key(0, 1, ctrl));
    }
    keys.sort();
    keys.dedup();
    keys
}

/// The key of a press as the page side builds it (the same text `pointer::key` gives).
pub fn js_press_key(button: u8, clicks: u8, mods: &Mods) -> String {
    pointer::key(button, clicks, *mods)
}

/// A message of the gate script (page coordinates, the viewport's own).
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    Down { button: u8, clicks: u8, mods: Mods, x: f64, y: f64 },
    Up { button: u8, clicks: u8, mods: Mods, x: f64, y: f64 },
    /// The fourth click (`BWV:414-433`).
    SmartSelect { x: f64, y: f64 },
    /// A single click with only Win held (`BWV:528-546`).
    SmartAction { x: f64, y: f64 },
    Focus,
}

fn mods_of(bits: i64) -> Option<Mods> {
    (0..=15).contains(&bits).then_some(Mods { shift: bits & 1 != 0, ctrl: bits & 2 != 0, alt: bits & 4 != 0, cmd: bits & 8 != 0 })
}

fn coord(body: &Value, key: &str) -> Option<f64> {
    body.get(key)?.as_f64().filter(|v| v.is_finite() && v.abs() <= MAX_COORD)
}

/// The press a message of the gate script is, with the JavaScript button as iTerm2's. Anything that
/// is not a well-formed press is None.
pub fn press_of(body: &Value) -> Option<Press> {
    match body.get("type")?.as_str()? {
        t @ ("down" | "up") => {
            let button = button_of_js(body.get("button")?.as_i64()?);
            let clicks = u8::try_from(body.get("clicks")?.as_i64()?).ok().filter(|c| (1..=4).contains(c))?;
            let mods = mods_of(body.get("mods")?.as_i64()?)?;
            let (x, y) = (coord(body, "x")?, coord(body, "y")?);
            if button == 255 {
                return None;
            }
            Some(if t == "down" { Press::Down { button, clicks, mods, x, y } } else { Press::Up { button, clicks, mods, x, y } })
        }
        "smart" => Some(Press::SmartSelect { x: coord(body, "x")?, y: coord(body, "y")? }),
        "smartAction" => Some(Press::SmartAction { x: coord(body, "x")?, y: coord(body, "y")? }),
        "focus" => Some(Press::Focus),
        _ => None,
    }
}

/// The expression that sets the gate script's table in a world (the script reads it at press time).
/// `four_click_smart` is iTerm2's fourth-click Smart Selection (always on there), `focus_on_right_or_middle`
/// the preference of that name.
pub fn gate_update(keys: &[String], four_click_smart: bool, focus_on_right_or_middle: bool) -> String {
    let list = serde_json::to_string(keys).unwrap_or_else(|_| "[]".into());
    format!("__termurionGate = Object.freeze({{keys: {list}, fourClick: {four_click_smart}, focus: {focus_on_right_or_middle}}});")
}

/// `pointer-gate.js` (Tandem's), with its `{{SECRET}}` key.
pub fn gate_script() -> String {
    crate::browser_js::resource("pointer-gate.js").unwrap_or_default().to_string()
}

/// The gate as a Client script: the table first, then the listeners.
pub fn user_script(secret: &str, keys: &[String], four_click_smart: bool, focus_on_right_or_middle: bool) -> crate::browser_js::UserScript {
    let body = crate::browser_js::fill_template(&gate_script(), &[("SECRET", secret)]);
    crate::browser_js::UserScript {
        id: "pointer-gate",
        code: format!("{}\n{}", gate_update(keys, four_click_smart, focus_on_right_or_middle), body),
        inject: crate::browser_js::Inject::DocumentStart,
        // Frames: presses in an iframe stay the page's (their coordinates are the frame's).
        main_frame_only: true,
        world: crate::browser_js::World::Client,
    }
}

/// The link `open-link-at-point.js` found: its `url` (an object with none is no link). Bounded to a
/// URL's length and free of control characters; page-chosen text, judged by `nav_gate` before use.
pub fn link_of(v: &Value) -> Option<String> {
    let url = v.get("url")?.as_str()?;
    (!url.is_empty() && url.len() <= crate::webhost::MAX_URL && !url.chars().any(char::is_control)).then(|| url.to_string())
}

/// The first URL `urls-at-point.js` found, bounded as `link_of`.
pub fn first_url_of(v: &Value) -> Option<String> {
    link_of(&serde_json::json!({ "url": v.as_array()?.first()?.as_str()? }))
}

/// A number as a script template substitutes it.
pub fn num(v: f64) -> String {
    format!("{v}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointer::{self, Mods, Prefs};
    use serde_json::json;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };
    const P: Prefs = Prefs { pass_on_control_click: false, report_right_click: false };

    fn map(pairs: &[(&str, &str)]) -> toml::Table {
        let mut t = toml::Table::new();
        for (k, action) in pairs {
            let mut b = toml::Table::new();
            b.insert("Action".into(), toml::Value::String((*action).into()));
            t.insert((*k).into(), toml::Value::Table(b));
        }
        t
    }

    /// `PAP:13-173`, `BVC:1474-1600`: what each pointer action does in a page.
    #[test]
    fn pointer_actions_follow_iterm2() {
        use PointerAct::*;
        assert_eq!(classify("kOpenTargetPointerAction"), OpenLink { new_tab: false });
        assert_eq!(classify("kOpenTargetInBackgroundPointerAction"), OpenLink { new_tab: true });
        assert_eq!(classify("kSmartSelectionPointerAction"), SmartSelect);
        assert_eq!(classify("kSmartSelectionIgnoringNewlinesPointerAction"), SmartSelect);
        assert_eq!(classify("kContextMenuPointerAction"), ContextMenu);
        assert_eq!(classify("kExtendSelectionPointerAction"), ExtendSelection);
        assert_eq!(classify("kQuickLookAction"), QuickLook);
        assert_eq!(classify("kCopyLinkAddressPointerAction"), CopyLinkAddress);
        assert_eq!(classify("kPasteFromClipboardPointerAction"), PasteClipboard);
        assert_eq!(classify("kPasteFromSelectionPointerAction"), PasteSelection);
        assert_eq!(classify("kCopyOrPastePointerAction"), CopyOrPaste);
        assert_eq!(classify("kSendTextPointerAction"), SendText);
        assert_eq!(classify("kSendHexCodePointerAction"), SendText);
        // Escape sequences are ignored in a page (`PAP:87-88`), as is Ignore and anything unknown.
        assert_eq!(classify("kSendEscapeSequencePointerAction"), Nothing);
        assert_eq!(classify("kIgnoreAction"), Nothing);
        assert_eq!(classify("kNoSuchAction"), Nothing);
        // The session's own actions go to the session.
        for a in [
            "kNextTabPointerAction",
            "kPrevTabPointerAction",
            "kNextWindowPointerAction",
            "kPrevWindowPointerAction",
            "kMovePanePointerAction",
            "kSelectPaneLeftPointerAction",
            "kSelectPaneRightPointerAction",
            "kSelectPaneAbovePointerAction",
            "kSelectPaneBelowPointerAction",
            "kNewWindowWithProfilePointerAction",
            "kNewTabWithProfilePointerAction",
            "kNewVerticalSplitWithProfilePointerAction",
            "kNewHorizontalSplitWithProfilePointerAction",
            "kSelectMenuItemPointerAction",
            "kInvokeScriptFunction",
            "kSelectNextPanePointerAction",
            "kSelectPreviousPanePointerAction",
        ] {
            assert_eq!(classify(a), Session, "{a}");
        }
        // Every action Tandem lists is classified (none falls through by accident).
        for (a, _, _) in pointer::ACTIONS.iter() {
            let _ = classify(a);
        }
        assert_eq!(button_of_js(0), 0);
        assert_eq!(button_of_js(1), 2);
        assert_eq!(button_of_js(2), 1);
        // The mouse's Back and Forward buttons and beyond keep their numbers (iTerm2's otherMouseDown, `BWV:476-487`).
        assert_eq!(button_of_js(3), 3);
        assert_eq!(button_of_js(4), 4);
        assert_eq!(button_of_js(31), 31);
        assert_eq!(button_of_js(32), 255);
        assert_eq!(button_of_js(-1), 255);
    }

    /// `BWV:383-433`, `BWV:459-490`: only the presses a binding claims are taken from the page.
    #[test]
    fn pointer_gate_follows_iterm2() {
        // Middle click to Next Tab, right click to Paste: exactly those two keys, as `pointer::key`.
        let m = map(&[("Button,2,1,,", "kNextTabPointerAction"), ("Button,1,1,,", "kPasteFromClipboardPointerAction")]);
        let ctrl = Mods { ctrl: true, ..NONE };
        // (Right click is bound here, so plain Ctrl single left is added too: see below.)
        let mut keys: Vec<String> = gate_keys(&m, &P).into_iter().filter(|k| *k != pointer::key(0, 1, ctrl)).collect();
        keys.sort();
        let mut want = vec![pointer::key(2, 1, NONE), pointer::key(1, 1, NONE)];
        want.sort();
        assert_eq!(keys, want);
        // Right click bound: plain Ctrl single left stands for it (`pointer::emulates_right_click`).
        assert!(gate_keys(&m, &P).contains(&pointer::key(0, 1, ctrl)), "ctrl-click is a right click here");
        // Left clicks stay native in a page: a left double click alone yields nothing.
        assert!(gate_keys(&map(&[("Button,0,2,,", "kSmartSelectionPointerAction")]), &P).is_empty());
        // With nothing on the right button, Ctrl-click is not taken.
        assert!(gate_keys(&map(&[("Button,2,1,,", "kNextTabPointerAction")]), &P).iter().all(|k| *k != pointer::key(0, 1, ctrl)));
        // Open Context Menu on the right button stays the page's own menu (Task 3's), not taken.
        let menu = map(&[("Button,1,1,,", "kContextMenuPointerAction")]);
        let k = gate_keys(&menu, &P);
        assert!(!k.contains(&pointer::key(1, 1, NONE)), "the page's own right click is Task 3's menu");
        assert!(k.contains(&pointer::key(0, 1, ctrl)), "ctrl-click still stands for it");
        // A binding on button 4 (the mouse's Back button) is taken from the page like the middle button's.
        let back = map(&[("Button,3,1,,", "kNextTabPointerAction"), ("Button,4,2,c,", "kNextTabPointerAction")]);
        assert_eq!(gate_keys(&back, &P), { let mut v = vec![pointer::key(3, 1, NONE), pointer::key(4, 2, Mods { cmd: true, ..NONE })]; v.sort(); v });
        let b3 = press_of(&json!({"type": "down", "button": 3, "clicks": 1, "mods": 0, "x": 1.0, "y": 2.0}));
        assert_eq!(b3, Some(Press::Down { button: 3, clicks: 1, mods: NONE, x: 1.0, y: 2.0 }));
        let gate = include_str!("../resources/browser/pointer-gate.js");
        assert!(gate.contains("js >= 3 && js <= 31 ? js : -1"), "the script's buttonOf carries buttons 3 and up");
        // Gestures and junk keys are not clicks.
        assert!(gate_keys(&map(&[("Gesture,ThreeFingerSwipeUp,,", "kNextWindowPointerAction"), ("nonsense", "kNextTabPointerAction")]), &P).is_empty());
        // The press message, with Task 6's mods bits (1 Shift, 2 Ctrl, 4 Alt, 8 Win) and the page's own button
        // numbers (0 left, 1 middle, 2 right), which iTerm2's numbering (0, 2, 1) replaces.
        let down = press_of(&json!({"type": "down", "button": 1, "clicks": 1, "mods": 4, "x": 3.0, "y": 4.0, "s": "x"}));
        assert_eq!(down, Some(Press::Down { button: 2, clicks: 1, mods: Mods { alt: true, ..NONE }, x: 3.0, y: 4.0 }));
        let up = press_of(&json!({"type": "up", "button": 2, "clicks": 2, "mods": 15, "x": 1.5, "y": 2.5}));
        assert_eq!(up, Some(Press::Up { button: 1, clicks: 2, mods: Mods { shift: true, ctrl: true, alt: true, cmd: true }, x: 1.5, y: 2.5 }));
        assert_eq!(press_of(&json!({"type": "smart", "x": 1.0, "y": 2.0})), Some(Press::SmartSelect { x: 1.0, y: 2.0 }));
        assert_eq!(press_of(&json!({"type": "smartAction", "x": 1.0, "y": 2.0})), Some(Press::SmartAction { x: 1.0, y: 2.0 }));
        assert_eq!(press_of(&json!({"type": "focus"})), Some(Press::Focus));
        // Anything else is nothing: unknown type, missing or absurd values, a button Tandem has no key for.
        for bad in [
            json!({"type": "nope"}),
            json!({"type": "down", "button": 1, "clicks": 1, "mods": 0, "x": 1.0}),
            json!({"type": "down", "button": 40, "clicks": 1, "mods": 0, "x": 1.0, "y": 1.0}),
            json!({"type": "down", "button": 1, "clicks": 0, "mods": 0, "x": 1.0, "y": 1.0}),
            json!({"type": "down", "button": 1, "clicks": 9, "mods": 0, "x": 1.0, "y": 1.0}),
            json!({"type": "down", "button": 1, "clicks": 1, "mods": 99, "x": 1.0, "y": 1.0}),
            json!({"type": "down", "button": 1, "clicks": 1, "mods": 0, "x": 1e12, "y": 1.0}),
            json!({"type": "smart", "x": "1", "y": 2.0}),
            json!("down"),
        ] {
            assert_eq!(press_of(&bad), None, "{bad}");
        }
        // What the point scripts answer.
        assert_eq!(link_of(&json!({"url": "https://a.test/x", "target": "_self"})), Some("https://a.test/x".to_string()));
        assert_eq!(link_of(&json!({})), None);
        assert_eq!(link_of(&json!({"url": "a\nb"})), None);
        assert_eq!(link_of(&json!({"url": 5})), None);
        assert_eq!(first_url_of(&json!(["https://a.test/i.png", "https://b"])), Some("https://a.test/i.png".to_string()));
        assert_eq!(first_url_of(&json!([])), None);
        // The expression a live world is given, and the key the page side builds.
        let e = gate_update(&["Button,2,1,,".to_string(), "Button,1,1,^,".to_string()], true, false);
        assert!(e.starts_with("__termurionGate = Object.freeze({keys: [\"Button,2,1,,\",\"Button,1,1,^,\"], fourClick: true, focus: false});"), "{e}");
        assert!(gate_update(&[], false, true).contains("fourClick: false, focus: true"));
        assert_eq!(js_press_key(2, 1, &Mods { ctrl: true, ..NONE }), "Button,2,1,^,");
        assert_eq!(js_press_key(2, 1, &Mods { ctrl: true, ..NONE }), pointer::key(2, 1, Mods { ctrl: true, ..NONE }));
        assert_eq!(js_press_key(0, 1, &Mods { cmd: true, alt: true, shift: true, ctrl: true }), "Button,0,1,cos^,");
        // The gate script: Tandem's, trusted events only, one place that listens.
        let s = gate_script();
        assert!(s.contains("Tandem's own script") && s.contains(GATE_HANDLER));
        assert_eq!(s.matches("addEventListener(").count(), 1, "one listener helper");
        let at = s.find("addEventListener(").unwrap();
        assert!(s[at..at + 160].contains("if (!e.isTrusted)"), "the check is at the top of the listener");
        for ev in ["mousedown", "mouseup", "auxclick", "contextmenu", "click"] {
            assert!(s.contains(&format!("\"{ev}\"")), "{ev}");
        }
        // The table is read at press time from the world-local object, never baked into the script.
        assert!(s.contains("__termurionGate") && !s.contains("{{KEYS}}") && !s.contains("{{FOCUS}}"));
        // iTerm2's rules for the four-click and the Win-click.
        assert!(s.contains("smartAction") && s.contains("\"smart\""));
        // The wire: a press never reports itself as the page's.
        assert!(s.contains("sessionSecret"));
    }

    /// Deferred minor 212 (final review minor 13): with Focus on Right or Middle Click on, every
    /// non-left press focuses the pane BEFORE its binding runs (`rightMouseDown` and
    /// `otherMouseDown` call `focusOnRightOrMiddleClickIfNeeded` first, `BWV:459-488`), bound or not.
    #[test]
    fn a_bound_right_or_middle_press_focuses_first() {
        let s = gate_script();
        let down = &s[s.find("function onDown(").unwrap()..s.find("function onUp(").unwrap()];
        let focus = down.find("post({ type: \"focus\" })").expect("onDown posts focus");
        let bound = down.find("if (g.keys.indexOf(keyOf(b, clicks, mods)) >= 0)").unwrap();
        assert!(focus < bound, "focus comes before the binding");
        assert!(down.contains("if (g.focus && b >= 1)"), "right, middle and the other buttons (otherMouseDown)");
    }
}
