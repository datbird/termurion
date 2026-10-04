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

/// Side margin of a row, padding inside a bubble, the gap between rows and the list's own
/// padding, in points (`MCV:58-60`, `RegularMessageCellView.swift:43-57`).
pub const SIDE: f32 = 8.0;
pub const BUBBLE_PAD_X: f32 = 10.0;
pub const BUBBLE_PAD_Y: f32 = 6.0;
pub const ROW_GAP: f32 = 6.0;
pub const LIST_PAD: f32 = 6.0;
pub const BUTTON_H: f32 = 30.0;
/// The input's outer margin, its vertical padding and the most lines it grows to before it
/// scrolls (Decision).
pub const INPUT_MARGIN: f32 = 8.0;
pub const INPUT_PAD_V: f32 = 6.0;
pub const INPUT_LINES_MAX: usize = 6;

/// Where one conversation row sits, from the list's top left (`x` from the panel's left edge, `y`
/// from the content's top): its bubble (zero width for a date or the typing row), its text, its
/// buttons stacked under the text across the bubble, and whose it is.
#[derive(Clone, Debug, PartialEq)]
pub struct RowBox {
    pub item: usize,
    pub bubble: Rect,
    pub text: Rect,
    pub buttons: Vec<Rect>,
    pub user: bool,
}

/// The width a bubble holds at most: 70 percent of the panel (`MCV:58-60`).
fn bubble_max(panel_w: f32) -> f32 {
    panel_w * crate::chat_view::MAX_BUBBLE
}

/// The width text wraps at inside a bubble: 70 percent of the panel less the bubble's padding.
pub fn text_width(panel_w: f32, scale: f32) -> f32 {
    (bubble_max(panel_w) - 2.0 * BUBBLE_PAD_X * scale).max(20.0 * scale)
}

/// The boxes of the rows and the content's height. `rows` are (the person's, the text's height at
/// `text_width`, its natural width, the number of buttons, a date or the typing row). A date or
/// the typing row has no bubble and sits centered (the single flag of the brief; the typing dots
/// are centered as well, Decision).
pub fn row_boxes(rows: &[(bool, f32, f32, usize, bool)], panel_w: f32, scale: f32) -> (Vec<RowBox>, f32) {
    if rows.is_empty() {
        return (Vec::new(), 0.0);
    }
    let (side, pad_x, pad_y, gap, button_h) = (SIDE * scale, BUBBLE_PAD_X * scale, BUBBLE_PAD_Y * scale, ROW_GAP * scale, BUTTON_H * scale);
    let (mut y, mut out) = (LIST_PAD * scale, Vec::with_capacity(rows.len()));
    for (item, &(user, text_h, natural_w, buttons, marker)) in rows.iter().enumerate() {
        if marker {
            let w = natural_w.min(panel_w - 2.0 * side).max(0.0);
            let text = Rect { x: (panel_w - w) / 2.0, y, w, h: text_h };
            out.push(RowBox { item, bubble: Rect { x: text.x, y, w: 0.0, h: 0.0 }, text, buttons: Vec::new(), user });
            y += text_h + gap;
            continue;
        }
        let text_w = natural_w.min(text_width(panel_w, scale)).max(0.0);
        let bubble_w = if buttons > 0 { bubble_max(panel_w) } else { text_w + 2.0 * pad_x };
        let x = if user { panel_w - side - bubble_w } else { side };
        let bubble_h = pad_y + text_h + pad_y + buttons as f32 * button_h;
        let text = Rect { x: x + pad_x, y: y + pad_y, w: text_w, h: text_h };
        let first = text.y + text_h + pad_y;
        let buttons = (0..buttons).map(|i| Rect { x, y: first + i as f32 * button_h, w: bubble_w, h: button_h }).collect();
        out.push(RowBox { item, bubble: Rect { x, y, w: bubble_w, h: bubble_h }, text, buttons, user });
        y += bubble_h + gap;
    }
    (out, y - gap + LIST_PAD * scale)
}

/// The scroll after a wheel turn: `dy` is in lines and already carries the app's three lines
/// a notch (`WHEEL_LINES`), so a notch moves three lines, as the ruling and Clippings say.
pub fn wheel_scroll(scroll: f32, dy: f32, line_h: f32, max: f32) -> f32 {
    (scroll - dy * line_h).clamp(0.0, max)
}

/// The scroll after the content or the view changed: pinned to the bottom until the person
/// scrolls up, else kept inside the content.
pub fn scroll_after(scroll: f32, content_h: f32, view_h: f32, pinned: bool) -> f32 {
    let max = (content_h - view_h).max(0.0);
    if pinned { max } else { scroll.clamp(0.0, max) }
}

/// The input box at the panel's bottom: it grows with its lines, up to six.
pub fn input_rect(panel: Rect, lines: usize, line_h: f32, scale: f32) -> Rect {
    let (margin, pad) = (INPUT_MARGIN * scale, INPUT_PAD_V * scale);
    let h = lines.clamp(1, INPUT_LINES_MAX) as f32 * line_h + 2.0 * pad;
    Rect { x: panel.x + margin, y: panel.y + panel.h - margin - h, w: (panel.w - 2.0 * margin).max(0.0), h }
}

/// The parts of the input box: the attach glyph at its left, the send or stop glyph at its right
/// (both at the bottom, so they stay put while the box grows upward) and the text between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputParts {
    pub attach: Rect,
    pub text: Rect,
    pub send: Rect,
}

pub fn input_parts(input: Rect, scale: f32) -> InputParts {
    let (pad, inset) = (INPUT_PAD_V * scale, 4.0 * scale);
    let g = (24.0 * scale).min((input.h - 2.0).max(1.0));
    let y = input.y + input.h - g - ((input.h - g) / 2.0).min(3.0 * scale);
    let attach = Rect { x: input.x + inset, y, w: g, h: g };
    let send = Rect { x: input.x + input.w - inset - g, y, w: g, h: g };
    let left = attach.x + g + inset;
    InputParts { attach, text: Rect { x: left, y: input.y + pad, w: (send.x - inset - left).max(0.0), h: (input.h - 2.0 * pad).max(0.0) }, send }
}

/// What Enter does in the panel's input: Task 7's rules over the panel's own draft.
pub fn panel_enter(draft: &crate::chat_input::Draft, shift: bool, typing: bool) -> crate::chat_input::Enter {
    crate::chat_input::on_enter(shift, crate::chat_input::send_enabled(&draft.text, false, false), typing)
}

/// The send or stop glyph and whether it can be pressed (`IN:394-411`).
pub fn send_state(draft: &crate::chat_input::Draft, loading: bool, typing: bool) -> (char, bool) {
    (if typing { crate::chat_input::STOP_GLYPH } else { crate::chat_input::SEND_GLYPH }, crate::chat_input::send_enabled(&draft.text, loading, typing))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_wheel_notch_scrolls_three_lines() {
        // The app hands over dy = notches * 3 lines; one notch up from line 30 is line 27.
        assert_eq!(wheel_scroll(300.0, 3.0, 10.0, 1000.0), 270.0);
        assert_eq!(wheel_scroll(300.0, -3.0, 10.0, 1000.0), 330.0);
        assert_eq!(wheel_scroll(10.0, 3.0, 10.0, 1000.0), 0.0, "clamped at the top");
        assert_eq!(wheel_scroll(990.0, -3.0, 10.0, 1000.0), 1000.0, "clamped at the bottom");
    }

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

    fn box_top(b: &RowBox) -> f32 {
        if b.bubble.w > 0.0 { b.bubble.y } else { b.text.y }
    }

    fn box_bottom(b: &RowBox) -> f32 {
        if b.bubble.w > 0.0 { b.bubble.y + b.bubble.h } else { b.text.y + b.text.h }
    }

    #[test]
    fn panel_layout_follows_iterm2() {
        // user 100 x 20; agent 500 wide, 60 high; agent with 4 buttons; a date; the typing row.
        let rows = [(true, 20.0, 100.0, 0, false), (false, 60.0, 500.0, 0, false), (false, 40.0, 150.0, 4, false), (false, 14.0, 60.0, 0, true), (false, 18.0, 30.0, 0, true)];
        let (boxes, content) = row_boxes(&rows, 400.0, 1.0);
        assert_eq!(boxes.len(), 5);
        let user = &boxes[0];
        assert!(user.user && user.bubble.w <= 280.0 && user.bubble.w == 100.0 + 2.0 * BUBBLE_PAD_X, "{user:?}");
        assert_eq!(user.bubble.x + user.bubble.w, 400.0 - SIDE, "the person's bubble is right-aligned");
        assert_eq!((user.text.w, user.text.h), (100.0, 20.0));
        assert_eq!((user.text.x, user.text.y), (user.bubble.x + BUBBLE_PAD_X, user.bubble.y + BUBBLE_PAD_Y));
        assert_eq!(user.bubble.h, 20.0 + 2.0 * BUBBLE_PAD_Y);
        let agent = &boxes[1];
        assert!(!agent.user);
        assert_eq!((agent.bubble.x, agent.bubble.w), (SIDE, 280.0), "at most 70 percent of the panel");
        assert_eq!(text_width(400.0, 1.0), 280.0 - 2.0 * BUBBLE_PAD_X);
        assert_eq!(agent.text.w, text_width(400.0, 1.0), "wrapped at the text width");
        let b = &boxes[2];
        assert_eq!(b.buttons.len(), 4);
        for (i, r) in b.buttons.iter().enumerate() {
            assert_eq!((r.x, r.w, r.h), (b.bubble.x, b.bubble.w, BUTTON_H), "a full-bubble-width row of 30 points");
            assert_eq!(r.y, b.text.y + b.text.h + BUBBLE_PAD_Y + i as f32 * BUTTON_H);
        }
        assert_eq!(b.bubble.h, BUBBLE_PAD_Y + 40.0 + BUBBLE_PAD_Y + 4.0 * BUTTON_H);
        assert_eq!(b.bubble.w, 280.0, "a bubble with buttons takes its full width");
        let date = &boxes[3];
        assert_eq!(date.bubble.w, 0.0, "a date row has no bubble");
        assert_eq!(date.text.x + date.text.w / 2.0, 200.0, "and is centered");
        let typing = &boxes[4];
        assert_eq!((typing.bubble.w, typing.text.x + typing.text.w / 2.0), (0.0, 200.0));
        // The content is the rows, the gaps between them, and the list's padding at both ends.
        assert_eq!(box_top(&boxes[0]), LIST_PAD);
        for pair in boxes.windows(2) {
            assert_eq!(box_top(&pair[1]) - box_bottom(&pair[0]), ROW_GAP);
        }
        assert_eq!(content, box_bottom(&boxes[4]) + LIST_PAD);
        // Scaled: every length doubles, and an empty list has no height.
        let (twice, content2) = row_boxes(&[(true, 40.0, 200.0, 0, false)], 800.0, 2.0);
        assert_eq!((twice[0].bubble.w, twice[0].bubble.x + twice[0].bubble.w), (200.0 + 4.0 * BUBBLE_PAD_X, 800.0 - 2.0 * SIDE));
        assert_eq!(content2, 2.0 * (2.0 * LIST_PAD) + twice[0].bubble.h);
        assert_eq!(row_boxes(&[], 400.0, 1.0), (vec![], 0.0));
        // Scrolling: pinned to the bottom until the person scrolls up; never past the ends.
        assert_eq!(scroll_after(0.0, 1000.0, 300.0, true), 700.0);
        assert_eq!(scroll_after(200.0, 1000.0, 300.0, false), 200.0);
        assert_eq!(scroll_after(900.0, 500.0, 300.0, false), 200.0);
        assert_eq!(scroll_after(50.0, 200.0, 300.0, true), 0.0);
        assert_eq!(scroll_after(-5.0, 1000.0, 300.0, false), 0.0);
    }

    #[test]
    fn panel_input_follows_iterm2() {
        use crate::chat_input::{Draft, Enter, SEND_GLYPH, STOP_GLYPH};
        let panel = r(10.0, 20.0, 400.0, 600.0);
        let one = input_rect(panel, 1, 18.0, 1.0);
        assert_eq!(one.h, 18.0 + 2.0 * INPUT_PAD_V, "one line and its padding");
        assert_eq!(one.y + one.h, 20.0 + 600.0 - INPUT_MARGIN, "at the panel's bottom");
        assert_eq!((one.x, one.w), (10.0 + INPUT_MARGIN, 400.0 - 2.0 * INPUT_MARGIN));
        assert_eq!(input_rect(panel, 3, 18.0, 1.0).h, 54.0 + 2.0 * INPUT_PAD_V, "three lines");
        let ten = input_rect(panel, 10, 18.0, 1.0);
        assert_eq!(ten.h, 6.0 * 18.0 + 2.0 * INPUT_PAD_V, "capped at six lines");
        assert_eq!(ten.y + ten.h, one.y + one.h, "growing upward");
        assert_eq!(input_rect(panel, 0, 18.0, 1.0), one, "an empty field is one line");
        assert_eq!(input_rect(panel, 1, 36.0, 2.0).h, 2.0 * one.h);
        // The glyphs sit in the box's bottom corners and the text between them.
        let parts = input_parts(ten, 1.0);
        assert!(parts.attach.x >= ten.x && parts.attach.y + parts.attach.h <= ten.y + ten.h && parts.send.x + parts.send.w <= ten.x + ten.w);
        assert_eq!(parts.attach.y, parts.send.y);
        assert!(parts.attach.x + parts.attach.w <= parts.text.x && parts.text.x + parts.text.w <= parts.send.x, "{parts:?}");
        assert_eq!((parts.text.y, parts.text.h), (ten.y + INPUT_PAD_V, 6.0 * 18.0));
        assert_eq!(input_parts(input_rect(panel, 1, 18.0, 1.0), 1.0).text.w, parts.text.w, "the text's width does not depend on the lines");
        // Enter and the send glyph are Task 7's rules over the panel's draft.
        let typed = Draft { text: "hello".into(), pending: vec![], ..Default::default() };
        let blank = Draft { text: " \n ".into(), pending: vec![], ..Default::default() };
        assert_eq!(panel_enter(&typed, false, false), Enter::Send);
        assert_eq!(panel_enter(&typed, false, true), Enter::StopThenSend);
        assert_eq!(panel_enter(&typed, true, false), Enter::Newline);
        assert_eq!(panel_enter(&blank, false, false), Enter::Nothing, "whitespace never sends");
        assert_eq!(panel_enter(&blank, false, true), Enter::Stop);
        assert_eq!(send_state(&typed, false, false), (SEND_GLYPH, true));
        assert_eq!(send_state(&blank, false, false), (SEND_GLYPH, false));
        assert_eq!(send_state(&typed, true, false), (SEND_GLYPH, false), "an attachment still loading blocks it");
        assert_eq!(send_state(&blank, false, true), (STOP_GLYPH, true), "a typing agent: Stop is always enabled");
    }
}
