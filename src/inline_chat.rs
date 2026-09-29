//! iTerm2's inline chat, the pure half: a pane's binding to a chat, the panel's width, how
//! Clippings and the chat share the gutter, the arrangement keys, the panel's toolbar and
//! its Switch menu (`PTYSession.swift:2554-2699`, `ChatViewController.swift:3703-3803`,
//! `InlineChatToolbarView.swift:30-110`).

use toml::Table;

use crate::layout::Rect;

pub const PANEL_ID: &str = "com.iterm2.inlineChat";
/// iTerm2's key for the panel's width (`iTermGutterPanelWidths.swift:18-47`).
pub const WIDTH_KEY: &str = "NoSyncGutterPanelWidth_com.iterm2.inlineChat";
pub const DEFAULT_WIDTH: f32 = 400.0;
/// The arrangement's keys (`PTYSession.m:345-346`).
pub const ID_KEY: &str = "Inline Chat ID";
pub const VISIBLE_KEY: &str = "Inline Chat Visible";
pub const TOOLBAR_H: f32 = 28.0;
const BUTTON: f32 = 24.0;
const PADDING: f32 = 8.0;
const SPACING: f32 = 6.0;

pub const TITLE: &str = crate::chat_toolbar::WINDOW_TITLE;
pub const NEW_CHAT_TIP: &str = crate::chat_toolbar::NEW_CHAT_TIP;
pub const SWITCH_TIP: &str = "Switch to another chat";
pub const INFO_TIP: &str = crate::chat_toolbar::SESSION_TIP;
pub const HIDE_TIP: &str = "Hide chat";
pub const UNTITLED: &str = "Untitled Chat";
pub const NO_CHATS: &str = "No Chats";
pub const TMUX_REFUSED: &str = "The inline chat is not shown in tmux panes: tmux sizes the whole window and the panel would take columns it does not know about.";

pub const NEW_GLYPH: char = '\u{E70F}';
pub const SWITCH_GLYPH: char = '\u{E8F2}';
pub const INFO_GLYPH: char = crate::chat_toolbar::INFO_GLYPH;
pub const HIDE_GLYPH: char = '\u{E711}';

/// A pane's inline chat: the bound chat (`inlineChatID`) and whether its panel shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub chat: Option<String>,
    pub visible: bool,
}

/// The panel's width in logical pixels: 0 unless a chat is bound, shown and still there
/// (`CVC:3890-3917`); else the stored width clamped, or the default.
pub fn width(b: &Binding, exists: bool, stored: Option<f32>) -> f32 {
    if b.chat.is_none() || !b.visible || !exists {
        return 0.0;
    }
    crate::clippings::clamp_width(stored.unwrap_or(DEFAULT_WIDTH))
}

/// What View > Show Inline Chat does (`toggleInlineChat`, `PSS:2586-2624`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// A chat is bound: show or hide its panel.
    Flip,
    /// Bind this chat (the most recent one linked to the session) and show it.
    Bind(String),
    /// Make a new unlinked chat about the session, bind and show it.
    Create,
    /// The AI gate said no.
    Refused,
}

pub fn toggle(b: &Binding, gate: bool, linked: Option<&str>) -> Toggle {
    if b.chat.is_some() {
        return Toggle::Flip;
    }
    if !gate {
        return Toggle::Refused;
    }
    match linked {
        Some(chat) => Toggle::Bind(chat.to_string()),
        None => Toggle::Create,
    }
}

/// A pane's terminal rectangle, then Clippings' panel and the chat's, the Clippings on the
/// left (`iTermRightGutterController.swift:240-275`). Together they take at most three
/// quarters of the pane, each shrunk in proportion (Decision).
pub fn split_gutter(pane: Rect, clip: f32, chat: f32) -> (Rect, Option<Rect>, Option<Rect>) {
    let (clip, chat) = (clip.max(0.0), chat.max(0.0));
    let total = clip + chat;
    if total <= 0.0 {
        return (pane, None, None);
    }
    let cap = pane.w * 0.75;
    let k = if total > cap { cap / total } else { 1.0 };
    let (clip, chat) = (clip * k, chat * k);
    let term = Rect { w: pane.w - clip - chat, ..pane };
    let clip_rect = (clip > 0.0).then_some(Rect { x: term.x + term.w, w: clip, ..pane });
    let chat_rect = (chat > 0.0).then_some(Rect { x: term.x + term.w + clip, w: chat, ..pane });
    (term, clip_rect, chat_rect)
}

/// The binding an arrangement restores (`restoreInlineChat`, `PSS:2626-2663`): with the chat
/// database not open it binds optimistically; open, only a chat that still exists.
pub fn restore(id: &str, visible: bool, db_open: bool, exists: bool) -> Option<Binding> {
    if db_open && !exists {
        return None;
    }
    Some(Binding { chat: Some(id.to_string()), visible })
}

/// The arrangement keys of a binding (`PSS` `arrangement`, `PTYSession.m:7459-7462`).
pub fn to_table(b: &Binding) -> Table {
    let mut t = Table::new();
    if let Some(chat) = &b.chat {
        t.insert(ID_KEY.into(), toml::Value::String(chat.clone()));
        t.insert(VISIBLE_KEY.into(), toml::Value::Boolean(b.visible));
    }
    t
}

/// The saved chat id and visibility; None without an id.
pub fn from_table(t: &Table) -> Option<(String, bool)> {
    let id = t.get(ID_KEY)?.as_str().filter(|id| !id.is_empty())?;
    Some((id.to_string(), t.get(VISIBLE_KEY).and_then(|v| v.as_bool()).unwrap_or(false)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarLayout {
    pub new: Rect,
    pub switch: Rect,
    pub title: Rect,
    pub info: Rect,
    pub hide: Rect,
}

/// The toolbar's row at the top of the panel (`ICT:30-110`): [new, switch, title, info, hide],
/// buttons 24 square, 8 of padding, 6 between, the title taking what is left.
pub fn toolbar(panel: Rect, scale: f32) -> ToolbarLayout {
    let (b, pad, gap) = (BUTTON * scale, PADDING * scale, SPACING * scale);
    let y = panel.y + (TOOLBAR_H * scale - b) / 2.0;
    let button = |x: f32| Rect { x, y, w: b, h: b };
    let new = button(panel.x + pad);
    let switch = button(new.x + b + gap);
    let hide = button(panel.x + panel.w - pad - b);
    let info = button(hide.x - b - gap);
    let left = switch.x + b + gap;
    let title = Rect { x: left, y, w: (info.x - gap - left).max(0.0), h: b };
    ToolbarLayout { new, switch, title, info, hide }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchItem {
    pub id: Option<String>,
    pub title: String,
    pub checked: bool,
    pub bold: bool,
    pub orchestrator: bool,
}

/// The Switch Chat menu (`CVC:3719-3767`): `chats` are (id, title, orchestrator, linked to
/// this session), most recent first.
pub fn switch_menu(chats: &[(String, String, bool, bool)], current: Option<&str>) -> Vec<SwitchItem> {
    if chats.is_empty() {
        return vec![SwitchItem { id: None, title: NO_CHATS.to_string(), checked: false, bold: false, orchestrator: false }];
    }
    chats
        .iter()
        .map(|(id, title, orchestrator, linked)| SwitchItem {
            id: Some(id.clone()),
            title: if title.is_empty() { UNTITLED.to_string() } else { title.clone() },
            checked: current == Some(id.as_str()),
            bold: *linked,
            orchestrator: *orchestrator,
        })
        .collect()
}

/// The four toolbar buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    New,
    Switch,
    Info,
    Hide,
}

impl Button {
    pub fn tip(self) -> &'static str {
        match self {
            Button::New => NEW_CHAT_TIP,
            Button::Switch => SWITCH_TIP,
            Button::Info => INFO_TIP,
            Button::Hide => HIDE_TIP,
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Button::New => NEW_GLYPH,
            Button::Switch => SWITCH_GLYPH,
            Button::Info => INFO_GLYPH,
            Button::Hide => HIDE_GLYPH,
        }
    }
}

/// Where a point in the panel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Button(Button),
    /// The left edge the width is dragged by.
    Divider,
    Body,
}

/// The rectangle under the toolbar.
pub fn body(panel: Rect, scale: f32) -> Rect {
    let h = (TOOLBAR_H * scale).min(panel.h);
    Rect { y: panel.y + h, h: panel.h - h, ..panel }
}

/// What (x, y) is over in the panel; None outside it. The divider is 4 pixels wide as
/// Clippings' is.
pub fn hit(panel: Rect, scale: f32, x: f32, y: f32) -> Option<Hit> {
    if !panel.contains(x, y) {
        return None;
    }
    if (Rect { w: 4.0 * scale, ..panel }).contains(x, y) {
        return Some(Hit::Divider);
    }
    let l = toolbar(panel, scale);
    for (b, r) in [(Button::New, l.new), (Button::Switch, l.switch), (Button::Info, l.info), (Button::Hide, l.hide)] {
        if r.contains(x, y) {
            return Some(Hit::Button(b));
        }
    }
    Some(Hit::Body)
}

/// `text` cut to at most `cols` characters, with an ellipsis where the tail was (the label's
/// `byTruncatingTail`).
pub fn truncate_tail(text: &str, cols: usize) -> String {
    if text.chars().count() <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(cols - 1).collect();
    out.push('\u{2026}');
    out
}

/// The Switch menu as muda draws it, which has no bold item: the chats linked to this
/// session first, a separator, then the rest (Decision). Either group may be empty.
pub fn switch_groups(items: Vec<SwitchItem>) -> (Vec<SwitchItem>, Vec<SwitchItem>) {
    items.into_iter().partition(|i| i.bold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn bound(visible: bool) -> Binding {
        Binding { chat: Some("c".into()), visible }
    }

    #[test]
    fn binding_follows_iterm2() {
        // A bound binding flips whatever the gate says.
        assert_eq!(toggle(&bound(true), false, None), Toggle::Flip);
        assert_eq!(toggle(&bound(false), true, Some("x")), Toggle::Flip);
        let none = Binding::default();
        assert_eq!(toggle(&none, false, Some("x")), Toggle::Refused);
        assert_eq!(toggle(&none, true, Some("c")), Toggle::Bind("c".into()));
        assert_eq!(toggle(&none, true, None), Toggle::Create);
        // Width: 0 unless bound, shown and existing.
        assert_eq!(width(&none, true, Some(500.0)), 0.0);
        assert_eq!(width(&bound(false), true, None), 0.0);
        assert_eq!(width(&bound(true), false, None), 0.0);
        assert_eq!(width(&bound(true), true, None), 400.0);
        assert_eq!(width(&bound(true), true, Some(100.0)), 225.0);
        assert_eq!(width(&bound(true), true, Some(5000.0)), 1200.0);
        assert_eq!(width(&bound(true), true, Some(640.0)), 640.0);
    }

    #[test]
    fn gutter_split_follows_iterm2() {
        let p = r(0.0, 10.0, 1000.0, 500.0);
        assert_eq!(split_gutter(p, 0.0, 0.0), (p, None, None));
        assert_eq!(split_gutter(p, 300.0, 0.0), (r(0.0, 10.0, 700.0, 500.0), Some(r(700.0, 10.0, 300.0, 500.0)), None));
        assert_eq!(split_gutter(p, 0.0, 400.0), (r(0.0, 10.0, 600.0, 500.0), None, Some(r(600.0, 10.0, 400.0, 500.0))));
        assert_eq!(
            split_gutter(p, 300.0, 400.0),
            (r(0.0, 10.0, 300.0, 500.0), Some(r(300.0, 10.0, 300.0, 500.0)), Some(r(600.0, 10.0, 400.0, 500.0)))
        );
        // Together at most three quarters, each shrunk in proportion.
        assert_eq!(
            split_gutter(p, 600.0, 600.0),
            (r(0.0, 10.0, 250.0, 500.0), Some(r(250.0, 10.0, 375.0, 500.0)), Some(r(625.0, 10.0, 375.0, 500.0)))
        );
        // One panel alone is capped the same way, and a pane not at x 0 keeps its origin.
        let q = r(100.0, 0.0, 400.0, 300.0);
        assert_eq!(split_gutter(q, 0.0, 1200.0), (r(100.0, 0.0, 100.0, 300.0), None, Some(r(200.0, 0.0, 300.0, 300.0))));
    }

    #[test]
    fn arrangement_keys_follow_iterm2() {
        let t = to_table(&bound(true));
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(ID_KEY).and_then(|v| v.as_str()), Some("c"));
        assert_eq!(t.get(VISIBLE_KEY).and_then(|v| v.as_bool()), Some(true));
        assert!(to_table(&Binding { chat: None, visible: true }).is_empty());
        assert_eq!(from_table(&t), Some(("c".to_string(), true)));
        assert_eq!(from_table(&to_table(&bound(false))), Some(("c".to_string(), false)));
        assert_eq!(from_table(&Table::new()), None);
        let mut only_visible = Table::new();
        only_visible.insert(VISIBLE_KEY.into(), toml::Value::Boolean(true));
        assert_eq!(from_table(&only_visible), None);
        // A missing visible flag reads as hidden, a non-string id as no id.
        let mut no_flag = Table::new();
        no_flag.insert(ID_KEY.into(), toml::Value::String("c".into()));
        assert_eq!(from_table(&no_flag), Some(("c".to_string(), false)));
        let mut bad = Table::new();
        bad.insert(ID_KEY.into(), toml::Value::Integer(4));
        assert_eq!(from_table(&bad), None);
        // Restore: the store not open binds optimistically; open, only a chat that is there.
        assert_eq!(restore("c", true, false, false), Some(bound(true)));
        assert_eq!(restore("c", true, true, false), None);
        assert_eq!(restore("c", false, true, true), Some(bound(false)));
    }

    #[test]
    fn hits_follow_the_toolbar() {
        let p = r(100.0, 50.0, 400.0, 300.0);
        let l = toolbar(p, 1.0);
        let mid = |b: Rect| (b.x + b.w / 2.0, b.y + b.h / 2.0);
        for (b, rect) in [(Button::New, l.new), (Button::Switch, l.switch), (Button::Info, l.info), (Button::Hide, l.hide)] {
            let (x, y) = mid(rect);
            assert_eq!(hit(p, 1.0, x, y), Some(Hit::Button(b)));
        }
        assert_eq!(hit(p, 1.0, 101.0, 200.0), Some(Hit::Divider));
        assert_eq!(hit(p, 1.0, 300.0, 200.0), Some(Hit::Body));
        assert_eq!(hit(p, 1.0, mid(l.title).0, mid(l.title).1), Some(Hit::Body));
        assert_eq!(hit(p, 1.0, 99.0, 200.0), None);
        assert_eq!(hit(p, 1.0, 300.0, 351.0), None);
        assert_eq!(body(p, 1.0), r(100.0, 78.0, 400.0, 272.0));
        assert_eq!(Button::Hide.tip(), "Hide chat");
    }

    #[test]
    fn a_long_title_is_cut_at_the_tail() {
        assert_eq!(truncate_tail("AI Chat", 20), "AI Chat");
        assert_eq!(truncate_tail("Chat about a long name", 8), "Chat ab\u{2026}");
        assert_eq!(truncate_tail("abc", 0), "");
        assert_eq!(truncate_tail("abcd", 1), "\u{2026}");
    }

    #[test]
    fn linked_chats_list_first() {
        let chats = vec![("a".to_string(), "A".to_string(), false, false), ("b".to_string(), "B".to_string(), false, true), ("c".to_string(), "C".to_string(), true, true)];
        let (here, rest) = switch_groups(switch_menu(&chats, None));
        assert_eq!(here.iter().map(|i| i.id.as_deref().unwrap()).collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(rest.iter().map(|i| i.id.as_deref().unwrap()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn switch_menu_follows_iterm2() {
        let chats = vec![("a".to_string(), "Alpha".to_string(), false, true), ("b".to_string(), String::new(), true, false)];
        let items = switch_menu(&chats, Some("b"));
        assert_eq!(
            items,
            vec![
                SwitchItem { id: Some("a".into()), title: "Alpha".into(), checked: false, bold: true, orchestrator: false },
                SwitchItem { id: Some("b".into()), title: "Untitled Chat".into(), checked: true, bold: false, orchestrator: true },
            ]
        );
        assert_eq!(switch_menu(&[], None), vec![SwitchItem { id: None, title: "No Chats".into(), checked: false, bold: false, orchestrator: false }]);
        // The toolbar of a 400-wide panel at the origin.
        let l = toolbar(r(0.0, 0.0, 400.0, 500.0), 1.0);
        assert_eq!(l.new, r(8.0, 2.0, 24.0, 24.0));
        assert_eq!(l.switch, r(38.0, 2.0, 24.0, 24.0));
        assert_eq!(l.hide, r(368.0, 2.0, 24.0, 24.0));
        assert_eq!(l.hide.x + l.hide.w, 392.0);
        assert_eq!(l.info.x, l.hide.x - 30.0);
        assert_eq!(l.info.w, 24.0);
        assert_eq!(l.title.x, l.switch.x + l.switch.w + 6.0);
        assert_eq!(l.title.x + l.title.w, l.info.x - 6.0);
        // Scaled and offset.
        let s = toolbar(r(100.0, 50.0, 800.0, 500.0), 2.0);
        assert_eq!(s.new, r(116.0, 54.0, 48.0, 48.0));
        assert_eq!(s.hide.x + s.hide.w, 100.0 + 800.0 - 16.0);
        // The texts.
        assert_eq!(NEW_CHAT_TIP, "New chat");
        assert_eq!(SWITCH_TIP, "Switch to another chat");
        assert_eq!(INFO_TIP, "Link or unlink terminal/browser session");
        assert_eq!(HIDE_TIP, "Hide chat");
        assert_eq!(TITLE, "AI Chat");
        assert!(TMUX_REFUSED.starts_with("The inline chat is not shown in tmux panes"));
        assert!(!TMUX_REFUSED.contains('\u{2014}'));
    }
}
