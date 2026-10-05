//! iTerm2's pointer bindings: the "Button,1,1,," keys, the actions, and the rules for
//! which click runs a binding, goes to a mouse-reporting app, or does the native thing
//! (PointerController.m and PTYMouseHandler.m).

use toml::Value;

use crate::commands::Status;
use crate::keyaction::Arg;
pub use crate::mouse::Mods;

/// The 32 pointer actions: iTerm2's constant, its menu name, and its argument.
pub const ACTIONS: [(&str, &str, Arg); 32] = [
    ("kIgnoreAction", "Ignore", Arg::None),
    ("kInvokeScriptFunction", "Invoke Script Function...", Arg::Other),
    ("kPasteFromClipboardPointerAction", "Paste from Clipboard...", Arg::Other),
    ("kPasteFromSelectionPointerAction", "Paste from Selection...", Arg::Other),
    ("kExtendSelectionPointerAction", "Extend Selection", Arg::None),
    ("kOpenTargetPointerAction", "Open URL/Semantic History", Arg::None),
    ("kOpenTargetInBackgroundPointerAction", "Open URL in background", Arg::None),
    ("kSmartSelectionPointerAction", "Smart Selection", Arg::None),
    ("kSmartSelectionIgnoringNewlinesPointerAction", "Smart Selection Ignoring Newlines", Arg::None),
    ("kContextMenuPointerAction", "Open Context Menu", Arg::None),
    ("kNextTabPointerAction", "Next Tab", Arg::None),
    ("kPrevTabPointerAction", "Previous Tab", Arg::None),
    ("kNextWindowPointerAction", "Next Window", Arg::None),
    ("kPrevWindowPointerAction", "Previous Window", Arg::None),
    ("kMovePanePointerAction", "Move Pane", Arg::None),
    ("kSendEscapeSequencePointerAction", "Send Escape Sequence...", Arg::Escape),
    ("kSendHexCodePointerAction", "Send Hex Code...", Arg::Hex),
    ("kSendTextPointerAction", "Send Text...", Arg::Text),
    ("kSelectPaneLeftPointerAction", "Select Pane Left", Arg::None),
    ("kSelectPaneRightPointerAction", "Select Pane Right", Arg::None),
    ("kSelectPaneAbovePointerAction", "Select Pane Above", Arg::None),
    ("kSelectPaneBelowPointerAction", "Select Pane Below", Arg::None),
    ("kNewWindowWithProfilePointerAction", "New Window With Profile...", Arg::Profile),
    ("kNewTabWithProfilePointerAction", "New Tab With Profile...", Arg::Profile),
    ("kNewVerticalSplitWithProfilePointerAction", "New Vertical Split With Profile...", Arg::Profile),
    ("kNewHorizontalSplitWithProfilePointerAction", "New Horizontal Split With Profile...", Arg::Profile),
    ("kQuickLookAction", "QuickLook", Arg::None),
    ("kSelectMenuItemPointerAction", "Select Menu Item", Arg::MenuItem),
    ("kCopyLinkAddressPointerAction", "Copy Link Address", Arg::None),
    ("kSelectNextPanePointerAction", "Select Next Pane", Arg::None),
    ("kSelectPreviousPanePointerAction", "Select Previous Pane", Arg::None),
    ("kCopyOrPastePointerAction", "Copy or Paste", Arg::None),
];

pub fn name(action: &str) -> &str {
    ACTIONS.iter().find(|a| a.0 == action).map_or(action, |a| a.1)
}

pub fn arg(action: &str) -> Arg {
    ACTIONS.iter().find(|a| a.0 == action).map_or(Arg::None, |a| a.2)
}

/// Whether a pointer binding works now, or the milestone that brings it. A paste with
/// a paste-special configuration waits for Paste Special (milestone 9).
pub fn status(b: &PointerBinding) -> Status {
    match b.action.as_str() {
        "kOpenTargetInBackgroundPointerAction" => Status::Partial("Asks the app not to take the focus; a browser may still come to the front."),
        "kQuickLookAction" => Status::Partial("Opens the target with its default app, since Windows has no Quick Look."),
        _ => Status::Done,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointerBinding {
    pub action: String,
    pub argument: String,
    pub version: i64,
}

impl PointerBinding {
    /// A new binding, with Common escaping for Send Text (version 1), as iTerm2 saves them.
    pub fn new(action: &str, argument: &str) -> PointerBinding {
        PointerBinding { action: action.into(), argument: argument.into(), version: 1 }
    }

    pub fn from_value(v: &Value) -> Option<PointerBinding> {
        let t = v.as_table()?;
        Some(PointerBinding {
            action: t.get("Action")?.as_str()?.to_string(),
            argument: t.get("Argument").and_then(Value::as_str).unwrap_or_default().to_string(),
            version: t.get("Version").and_then(Value::as_integer).unwrap_or(0),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut t = toml::Table::new();
        t.insert("Action".into(), Value::String(self.action.clone()));
        if !self.argument.is_empty() {
            t.insert("Argument".into(), Value::String(self.argument.clone()));
        }
        if self.version != 0 {
            t.insert("Version".into(), Value::Integer(self.version));
        }
        Value::Table(t)
    }

    /// Send Text escaping: version 0 is Compatibility, 1 is Common.
    pub fn escaping(&self) -> crate::keyaction::Escaping {
        if self.version == 0 { crate::keyaction::Escaping::Compatibility } else { crate::keyaction::Escaping::Common }
    }
}

fn mod_chars(m: Mods) -> String {
    [(m.cmd, 'c'), (m.alt, 'o'), (m.shift, 's'), (m.ctrl, '^')].iter().filter(|(on, _)| *on).map(|(_, c)| *c).collect()
}

/// "Button,<button>,<clicks>,<mods>," with mods in the order c o s ^.
pub fn key(button: u8, clicks: u8, m: Mods) -> String {
    format!("Button,{button},{clicks},{},", mod_chars(m))
}

pub fn parse_key(k: &str) -> Option<(u8, u8, Mods)> {
    let mut parts = k.split(',');
    if parts.next()? != "Button" {
        return None;
    }
    let button = parts.next()?.parse().ok()?;
    let clicks = parts.next()?.parse().ok()?;
    let mods = parts.next().unwrap_or("");
    let m = Mods { cmd: mods.contains('c'), alt: mods.contains('o'), shift: mods.contains('s'), ctrl: mods.contains('^') };
    Some((button, clicks, m))
}

/// "Right button, double click, Win+Shift": how a key reads in the bindings table.
pub fn key_label(k: &str) -> String {
    if let Some(g) = k.strip_prefix("Gesture,") {
        return g.split(',').next().unwrap_or(g).to_string();
    }
    let Some((button, clicks, m)) = parse_key(k) else { return k.to_string() };
    let button = match button {
        0 => "Left button".to_string(),
        1 => "Right button".to_string(),
        2 => "Middle button".to_string(),
        n => format!("Button #{}", n + 1),
    };
    let clicks = ["single click", "double click", "triple click", "quad click"].get(clicks.saturating_sub(1) as usize).copied().unwrap_or("(error)");
    let flags = crate::keystroke::from_mods(m);
    let mods = crate::keystroke::Keystroke::new(0, flags, None, 0).label();
    let mods = mods.trim_end_matches("+0x0").trim_end_matches("0x0");
    if mods.is_empty() { format!("{button}, {clicks}") } else { format!("{button}, {clicks}, {mods}") }
}

pub struct Prefs {
    /// "Ctrl-click reported to apps, does not open menu" (PassOnControlClick).
    pub pass_on_control_click: bool,
    /// "Right click reported to apps, does not open menu" (ReportRightClick).
    pub report_right_click: bool,
}

/// A plain Ctrl single click stands for a right click, unless it goes to a tracking app.
pub fn emulates_right_click(button: u8, clicks: u8, m: Mods, reportable: bool, p: &Prefs) -> bool {
    clicks == 1 && button == 0 && !(p.pass_on_control_click && reportable) && m.ctrl && !m.shift && !m.alt && !m.cmd
}

/// The binding for a click (PointerController actionForEvent:). While the app tracks the
/// mouse, Alt (which keeps the click from the app) is not part of the lookup.
pub fn lookup(map: &toml::Table, button: u8, clicks: u8, m: Mods, reportable: bool, app_tracking: bool, p: &Prefs) -> Option<PointerBinding> {
    let (button, clicks, m) = if emulates_right_click(button, clicks, m, reportable, p) {
        (1, 1, Mods::default())
    } else {
        if reportable && button == 1 && clicks == 1 && p.report_right_click {
            return None;
        }
        (button, clicks, Mods { alt: m.alt && !app_tracking, ..m })
    };
    map.get(&key(button, clicks, m)).and_then(PointerBinding::from_value)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Down {
    /// A binding runs on release; nothing else happens now.
    Bound,
    /// The app gets the press.
    Report,
    /// Tandem's own handling: selection for left, the context menu for right.
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Up {
    Perform(PointerBinding),
    Report,
    Native,
}

/// A press (PTYMouseHandler mouseDown, rightMouseDown, otherMouseDown).
pub fn on_down(button: u8, clicks: u8, m: Mods, reportable: bool, app_tracking: bool, map: &toml::Table, p: &Prefs) -> Down {
    let bound = || lookup(map, button, clicks, m, reportable, app_tracking, p).is_some();
    match button {
        0 if emulates_right_click(button, clicks, m, reportable, p) => Down::Bound,
        0 => Down::Native,
        1 if bound() => Down::Bound,
        _ if reportable => Down::Report,
        1 => Down::Native,
        _ if bound() => Down::Bound,
        _ => Down::Native,
    }
}

/// A release (PTYMouseHandler mouseUp, rightMouseUp, otherMouseUp).
pub fn on_up(button: u8, clicks: u8, m: Mods, reportable: bool, app_tracking: bool, map: &toml::Table, p: &Prefs) -> Up {
    let bound = || lookup(map, button, clicks, m, reportable, app_tracking, p);
    match button {
        0 if emulates_right_click(button, clicks, m, reportable, p) => bound().map_or(Up::Native, Up::Perform),
        0 => Up::Native,
        1 => {
            if !(p.report_right_click && reportable) {
                if let Some(b) = bound() {
                    return Up::Perform(b);
                }
            }
            if reportable { Up::Report } else { Up::Native }
        }
        _ if reportable => Up::Report,
        _ => bound().map_or(Up::Native, Up::Perform),
    }
}

/// Click counting for pointer bindings: a new button starts over at one, and the count
/// never passes the one the multi-click timer gives (a slow click is a single click).
#[derive(Default)]
pub struct Clicks {
    button: Option<u8>,
    count: u8,
}

impl Clicks {
    pub fn press(&mut self, button: u8, os_count: u8) -> u8 {
        self.count = if self.button == Some(button) { self.count.saturating_add(1) } else { 1 };
        self.count = self.count.min(os_count.max(1));
        self.button = Some(button);
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_actions_are_done() {
        use crate::commands::Status;
        for a in ["kNextWindowPointerAction", "kPrevWindowPointerAction", "kNewWindowWithProfilePointerAction", "kMovePanePointerAction"] {
            assert_eq!(status(&PointerBinding::new(a, "")), Status::Done, "{a}");
        }
        for a in ["kQuickLookAction", "kOpenTargetInBackgroundPointerAction"] {
            assert!(matches!(status(&PointerBinding::new(a, "")), Status::Partial(_)), "{a}");
        }
    }

    fn defaults() -> toml::Table {
        crate::globals::keymaps::factory_pointer().clone()
    }
    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const P: Prefs = Prefs { pass_on_control_click: false, report_right_click: false };

    #[test]
    fn keys_match_iterm2() {
        assert_eq!(key(1, 1, NONE), "Button,1,1,,");
        assert_eq!(key(0, 2, Mods { cmd: true, shift: true, ..NONE }), "Button,0,2,cs,");
        assert_eq!(key(2, 1, Mods { alt: true, ctrl: true, ..NONE }), "Button,2,1,o^,");
        assert_eq!(parse_key("Button,0,2,cs,"), Some((0, 2, Mods { cmd: true, shift: true, ..NONE })));
        assert_eq!(parse_key("Gesture,ThreeFingerSwipeUp,,"), None);
    }

    #[test]
    fn ctrl_click_emulates_a_right_click() {
        assert!(emulates_right_click(0, 1, CTRL, false, &P));
        assert!(!emulates_right_click(0, 1, Mods { shift: true, ..CTRL }, false, &P));
        assert!(!emulates_right_click(0, 2, CTRL, false, &P));
        let bypass = Prefs { pass_on_control_click: true, ..P };
        assert!(!emulates_right_click(0, 1, CTRL, true, &bypass));
        assert!(emulates_right_click(0, 1, CTRL, false, &bypass));
        assert_eq!(lookup(&defaults(), 0, 1, CTRL, false, false, &P).unwrap().action, "kContextMenuPointerAction");
    }

    #[test]
    fn right_click_binding_beats_reporting_unless_bypassed() {
        assert_eq!(on_down(1, 1, NONE, true, true, &defaults(), &P), Down::Bound);
        assert!(matches!(on_up(1, 1, NONE, true, true, &defaults(), &P), Up::Perform(_)));
        let bypass = Prefs { report_right_click: true, ..P };
        assert_eq!(on_down(1, 1, NONE, true, true, &defaults(), &bypass), Down::Report);
        assert_eq!(on_up(1, 1, NONE, true, true, &defaults(), &bypass), Up::Report);
        assert_eq!(on_up(1, 1, NONE, false, false, &toml::Table::new(), &P), Up::Native);
    }

    #[test]
    fn middle_click_goes_to_a_tracking_app_first() {
        assert_eq!(on_down(2, 1, NONE, true, true, &defaults(), &P), Down::Report);
        assert_eq!(on_up(2, 1, NONE, true, true, &defaults(), &P), Up::Report);
        assert!(matches!(on_up(2, 1, NONE, false, false, &defaults(), &P), Up::Perform(b) if b.action == "kPasteFromClipboardPointerAction"));
    }

    #[test]
    fn option_is_ignored_while_the_app_tracks_the_mouse() {
        let alt = Mods { alt: true, ..NONE };
        // Alt keeps the click from the app, and the lookup drops Alt, so Alt+middle still pastes.
        assert!(matches!(on_up(2, 1, alt, false, true, &defaults(), &P), Up::Perform(_)));
        assert_eq!(on_up(2, 1, alt, false, false, &defaults(), &P), Up::Native);
    }

    #[test]
    fn a_plain_left_click_is_never_a_binding() {
        let mut map = toml::Table::new();
        map.insert("Button,0,1,,".into(), PointerBinding::new("kIgnoreAction", "").to_value());
        assert_eq!(on_down(0, 1, NONE, false, false, &map, &P), Down::Native);
    }

    #[test]
    fn clicks_reset_when_the_button_changes() {
        let mut c = Clicks::default();
        assert_eq!(c.press(0, 1), 1);
        assert_eq!(c.press(0, 2), 2);
        assert_eq!(c.press(1, 3), 1);
        // The OS count caps ours: a slow second click is a single click.
        assert_eq!(c.press(1, 1), 1);
    }

    #[test]
    fn keys_read_like_iterm2_rows() {
        assert_eq!(key_label("Button,1,1,,"), "Right button, single click");
        assert_eq!(key_label("Button,0,2,cs,"), "Left button, double click, Win+Shift");
        assert_eq!(key_label("Button,4,1,,"), "Button #5, single click");
        assert_eq!(key_label("Gesture,ThreeFingerSwipeUp,,"), "ThreeFingerSwipeUp");
    }

    #[test]
    fn thirty_two_actions_with_names() {
        assert_eq!(ACTIONS.len(), 32);
        assert_eq!(name("kContextMenuPointerAction"), "Open Context Menu");
        assert_eq!(status(&PointerBinding::new("kOpenTargetPointerAction", "")), Status::Done);
        assert_eq!(status(&PointerBinding::new("kInvokeScriptFunction", "f()")), Status::Done, "11c");
        assert_eq!(status(&PointerBinding::new("kPasteFromClipboardPointerAction", "")), Status::Done);
        assert_eq!(status(&PointerBinding::new("kPasteFromClipboardPointerAction", "x")), Status::Done, "9a");
    }

    /// B7: a click the profile's gate refuses is not "reportable" to a bound pointer action nor
    /// to the frustration detector.
    #[test]
    fn a_refused_click_is_not_reportable_to_its_readers() {
        use crate::mouse::{should_report, Gates, Kind, Modes, Tracking};
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        let gates = Gates { clicks: false, wheel: true, horizontal: true, first_mouse_blocks: false, ui_over: false, view_focused: true };
        let reportable = should_report(modes, crate::mouse::Mods::default(), Kind::Click, gates);
        assert!(!reportable);
        assert_eq!(on_down(2, 1, NONE, true, true, &defaults(), &P), Down::Report);
        assert_eq!(on_down(2, 1, NONE, reportable, true, &defaults(), &P), Down::Bound, "the bound action runs");
        let mut d = crate::mouse_frustration::Detector::default();
        d.mouse_down(1, reportable);
        d.mouse_dragged(reportable);
        d.mouse_up(0, reportable);
        assert!(!d.copy(), "no reported drag, no offer");
    }
}
