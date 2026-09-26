//! The one native list widget every toolbelt tool draws with: a search field, a row
//! selection, a bottom button strip, and this widget's own layout, hit test, tail truncation
//! and key routing. Pure model, like `toolbelt` and `toolbelt_layout`; the app owns the Win32
//! double-click timer (`GetDoubleClickTime`, matching `clippings_ui::double_click_ms`),
//! clipboard access and drawing. Each of the seven tools (`toolbelt_tools`) wires its own rows,
//! filter and pick rules on top of this.

use crate::layout::Rect;

/// A key a focused field or list can act on, already decoded from the raw key event (`Paste`
/// is Ctrl+V, already recognized as such before it reaches `Field::key` or `takes_key`).
#[derive(Clone, Debug, PartialEq)]
pub enum ListKey {
    Text(String),
    Paste,
    Backspace,
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Tab,
    Delete,
}

/// Which part of a tool has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Field,
    List,
}

/// A tool's search field. Only the text is modeled; the caret always sits at the end (none of
/// the seven tools' fields need mid-string editing to satisfy this task).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub text: String,
}

/// What a key did to a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldOut {
    Changed,
    Enter,
    /// Give the keyboard back to the terminal (Esc, `find_ui::route_key`'s `Close` idiom).
    GiveBack,
    Up,
    Down,
    /// Tab: move the keyboard to the list.
    ToList,
    /// The key was not for the field (backspace on empty text, or an arrow the field does not
    /// send to the list); nothing changed.
    Kept,
}

impl Field {
    /// One key while the field has focus (`PLV:972-1006`, `iTermSearchField.m:40-66`). Esc
    /// clears non-empty text only where `esc_clears` (Profiles' search field); on empty text,
    /// or wherever `esc_clears` is off, Esc gives the keyboard back. Up and Down move the list
    /// only where `arrows_to_list` (on for Snippets, Command History and Recent Directories;
    /// off for Profiles, `iTermSearchField.m:40-66`).
    pub fn key(&mut self, k: &ListKey, esc_clears: bool, arrows_to_list: bool) -> FieldOut {
        match k {
            ListKey::Text(s) => {
                self.text.push_str(s);
                FieldOut::Changed
            }
            ListKey::Backspace => {
                if self.text.pop().is_some() {
                    FieldOut::Changed
                } else {
                    FieldOut::Kept
                }
            }
            ListKey::Escape if esc_clears && !self.text.is_empty() => {
                self.text.clear();
                FieldOut::Changed
            }
            ListKey::Escape => FieldOut::GiveBack,
            ListKey::Up if arrows_to_list => FieldOut::Up,
            ListKey::Down if arrows_to_list => FieldOut::Down,
            ListKey::Enter => FieldOut::Enter,
            ListKey::Tab => FieldOut::ToList,
            _ => FieldOut::Kept,
        }
    }
}

/// A list's row selection: which rows are selected, click and double-click handling (Ctrl
/// toggles a row, Shift extends from the last plain click, Windows list convention), arrow
/// stepping and staying valid as the row count changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub rows: Vec<usize>,
    /// The last plain click (no Ctrl, no Shift), Shift extends from here.
    anchor: Option<usize>,
    /// The last click of any kind, for double-click detection.
    last: Option<(usize, u128)>,
}

impl Selection {
    /// A click on `row`. Ctrl toggles it in or out of the selection; Shift selects the range
    /// from the last plain click's row to this one; a plain click selects only this row and
    /// becomes the new Shift anchor. `multi` false (a single-selection tool) ignores Ctrl and
    /// Shift, always taking the plain-click path. Returns true when this click lands on the
    /// same row as the previous click within `double_ms` (the Windows double-click time).
    pub fn click(&mut self, row: usize, ctrl: bool, shift: bool, multi: bool, now_ms: u128, double_ms: u128) -> bool {
        let double = self.last.is_some_and(|(r, t)| r == row && now_ms.saturating_sub(t) <= double_ms);
        self.last = Some((row, now_ms));

        if shift && multi {
            let anchor = self.anchor.unwrap_or(row);
            let (lo, hi) = (anchor.min(row), anchor.max(row));
            self.rows = (lo..=hi).collect();
        } else if ctrl && multi {
            if let Some(pos) = self.rows.iter().position(|&r| r == row) {
                self.rows.remove(pos);
            } else {
                self.rows.push(row);
                self.rows.sort_unstable();
            }
        } else {
            self.rows = vec![row];
            self.anchor = Some(row);
        }
        double
    }

    /// Up (`down` false) or Down moves a single-row selection by one, clamped to `[0, n - 1]`.
    /// From no selection, Down starts at the top (row 0) and Up at the bottom (row `n - 1`).
    pub fn step(&mut self, down: bool, n: usize) {
        if n == 0 {
            self.rows.clear();
            self.anchor = None;
            return;
        }
        let next = match self.rows.first().copied() {
            None => {
                if down {
                    0
                } else {
                    n - 1
                }
            }
            Some(r) => {
                if down {
                    (r + 1).min(n - 1)
                } else {
                    r.saturating_sub(1)
                }
            }
        };
        self.rows = vec![next];
        self.anchor = Some(next);
    }

    /// Drops rows that no longer exist, for a list that shrank to `n` rows.
    pub fn retain(&mut self, n: usize) {
        self.rows.retain(|&r| r < n);
        if self.anchor.is_some_and(|a| a >= n) {
            self.anchor = None;
        }
        if self.last.is_some_and(|(r, _)| r >= n) {
            self.last = None;
        }
    }

    /// Selects row 0 when nothing is selected and the list is not empty (Profiles: "each
    /// change selects row 0 when none").
    pub fn first_if_none(&mut self, n: usize) {
        if self.rows.is_empty() && n > 0 {
            self.rows = vec![0];
            self.anchor = Some(0);
        }
    }
}

/// A button in a tool's bottom strip (or, for Profiles' Open, its popup).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolButton {
    Send,
    AdvancedPaste,
    Apply,
    Open,
    Add,
    Remove,
    Edit,
    Trash,
    Help,
}

impl ToolButton {
    /// The Segoe Fluent Icons glyph (Decisions, m9j resume notes). Send, Apply and Open share
    /// the play glyph.
    pub fn glyph(self) -> char {
        match self {
            ToolButton::Send | ToolButton::Apply | ToolButton::Open => '\u{E768}',
            ToolButton::AdvancedPaste => '\u{E77F}',
            ToolButton::Add => '\u{E710}',
            ToolButton::Remove => '\u{E738}',
            ToolButton::Edit => '\u{E70F}',
            ToolButton::Trash => '\u{E74D}',
            ToolButton::Help => '\u{E897}',
        }
    }

    /// The on-screen text (no tooltips, the 9j ruling: buttons read their label directly).
    /// Paste History's Clear button is `Trash` here but reads "Clear" on screen.
    pub fn label(self) -> &'static str {
        match self {
            ToolButton::Send => "Send",
            ToolButton::AdvancedPaste => "Advanced Paste",
            ToolButton::Apply => "Apply",
            ToolButton::Open => "Open",
            ToolButton::Add => "Add",
            ToolButton::Remove => "Remove",
            ToolButton::Edit => "Edit",
            ToolButton::Trash => "Clear",
            ToolButton::Help => "Help",
        }
    }
}

/// A field's height, points (brief, Step 3).
pub const FIELD_H: f32 = 22.0;
/// The bottom button strip's height, points (ToolProfiles' popup row height, brief Step 3).
pub const STRIP_H: f32 = 26.0;
/// The gap between the field, the list and the strip, points (brief Step 3).
pub const GAP: f32 = 4.0;
/// A strip button's width, points (brief Step 3: "buttons 22 wide").
pub const BTN_W: f32 = 22.0;

/// What a tool needs drawn: an optional search field, an optional row of top buttons, and the
/// bottom strip's left group (left-aligned, left to right) and right group (laid out right to
/// left, so the first entry ends up at the panel's right edge and later entries read to its
/// left; e.g. `[Add, Remove, Edit]` reads on screen, left to right, Edit, Remove, Add).
/// `popup_w` is the width of a tool's own popup menu (0 for none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListSpec {
    pub field: bool,
    pub top: Vec<ToolButton>,
    pub left: Vec<ToolButton>,
    pub right: Vec<ToolButton>,
    pub popup_w: f32,
}

/// Where everything in a tool's list widget is. Every field is already in pixels; `layout`
/// only scales its own point constants (`FIELD_H`, `STRIP_H`, `GAP`, `BTN_W`) by `scale`, the
/// same convention as `toolbelt_layout::layout`.
#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    pub field: Option<Rect>,
    pub top: Vec<(ToolButton, Rect)>,
    pub list: Rect,
    pub rows: Vec<Rect>,
    pub buttons: Vec<(ToolButton, Rect)>,
    pub popup: Option<Rect>,
    pub content_h: f32,
}

/// The list widget's layout (`NSTableView+iTerm.m:36-89,65-69`, the seven tools' own button
/// rows in the Resume notes). `area` is the tool wrapper's content rectangle, already in
/// pixels (the app's own room for it, already scaled, matching how `toolbelt_layout::layout`
/// and `clippings::layout` take an already-pixel rectangle). `row_heights` and `scroll` are
/// pixels too, already at the render scale (each row's height comes from measured text at the
/// current font, like `clippings::layout`'s rows; unlike `toolbelt_layout::Split::heights`,
/// which are points because they come from the app's own point-based model, there is nothing
/// here to scale a second time). `scale` only stretches this module's own point constants.
pub fn layout(area: Rect, spec: &ListSpec, row_heights: &[f32], scroll: f32, scale: f32) -> ListLayout {
    let field_h = FIELD_H * scale;
    let strip_h = STRIP_H * scale;
    let gap = GAP * scale;
    let btn_w = BTN_W * scale;

    let mut y = area.y;
    let mut top = Vec::new();
    if !spec.top.is_empty() {
        let mut x = area.x;
        for &b in &spec.top {
            top.push((b, Rect { x, y, w: btn_w, h: field_h }));
            x += btn_w;
        }
        y += field_h + gap;
    }

    let field = if spec.field {
        let r = Rect { x: area.x, y, w: area.w, h: field_h };
        y += field_h + gap;
        Some(r)
    } else {
        None
    };

    let has_strip = !spec.left.is_empty() || !spec.right.is_empty();
    let strip_h_used = if has_strip { strip_h } else { 0.0 };
    let list = Rect { x: area.x, y, w: area.w, h: (area.y + area.h - strip_h_used - y).max(0.0) };

    let mut ry = list.y - scroll;
    let mut rows = Vec::with_capacity(row_heights.len());
    for &h in row_heights {
        rows.push(Rect { x: list.x, y: ry, w: list.w, h });
        ry += h;
    }
    let content_h: f32 = row_heights.iter().sum();

    let mut buttons = Vec::new();
    if has_strip {
        let strip_y = area.y + area.h - strip_h;
        let mut lx = area.x;
        for &b in &spec.left {
            buttons.push((b, Rect { x: lx, y: strip_y, w: btn_w, h: strip_h }));
            lx += btn_w;
        }
        let mut rx = area.x + area.w;
        for &b in &spec.right {
            rx -= btn_w;
            buttons.push((b, Rect { x: rx, y: strip_y, w: btn_w, h: strip_h }));
        }
    }

    // The popup's own geometry is not specified by the source for this task (it belongs to
    // whichever tool needs one, e.g. Profiles' Open); placed right-aligned at the list's top,
    // one strip tall, as a placeholder a later task can override. See the report.
    let popup = if spec.popup_w > 0.0 {
        let w = spec.popup_w * scale;
        Some(Rect { x: (area.x + area.w - w).max(area.x), y: list.y, w, h: strip_h.min(list.h) })
    } else {
        None
    };

    ListLayout { field, top, list, rows, buttons, popup, content_h }
}

/// What a point in a tool's list widget is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListHit {
    Field,
    Button(ToolButton),
    Popup,
    Row(usize),
    /// Inside the list but not on a row (empty space below the last row).
    Empty,
    Nothing,
}

pub fn hit(l: &ListLayout, x: f32, y: f32) -> ListHit {
    if let Some(r) = l.field {
        if r.contains(x, y) {
            return ListHit::Field;
        }
    }
    if let Some(r) = l.popup {
        if r.contains(x, y) {
            return ListHit::Popup;
        }
    }
    if let Some((b, _)) = l.top.iter().find(|(_, r)| r.contains(x, y)) {
        return ListHit::Button(*b);
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return ListHit::Button(*b);
    }
    if !l.list.contains(x, y) {
        return ListHit::Nothing;
    }
    match l.rows.iter().position(|r| r.contains(x, y)) {
        Some(i) => ListHit::Row(i),
        None => ListHit::Empty,
    }
}

/// The scroll that shows the last row (the list scrolls to the end unless focused, `NSTableView
/// +iTerm.m`).
pub fn end_scroll(l: &ListLayout) -> f32 {
    (l.content_h - l.list.h).max(0.0)
}

/// The scroll kept inside the list.
pub fn clamp_scroll(scroll: f32, l: &ListLayout) -> f32 {
    scroll.clamp(0.0, end_scroll(l))
}

/// Tail truncation with an ellipsis (`NSStringITerm.m`'s trimming, `NSTableView+iTerm.m`'s
/// cell truncation): `text` unchanged when it already fits `max_w`; otherwise the longest
/// prefix that, with a trailing `\u{2026}`, still fits, measured by `width`.
pub fn truncate_tail(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> String {
    if width(text) <= max_w {
        return text.to_string();
    }
    const ELLIPSIS: char = '\u{2026}';
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let candidate: String = chars[..n].iter().collect::<String>() + &ELLIPSIS.to_string();
        if width(&candidate) <= max_w {
            return candidate;
        }
    }
    ELLIPSIS.to_string()
}

/// Whether a key while a tool has `focus` is taken by it (Review Focus 4): nothing is taken
/// with no focus. A Win chord always passes through (Win is Cmd, never claimed here); an Alt
/// chord always passes through; a Ctrl chord passes through too, except Ctrl+V into a field
/// (`ListKey::Paste`, the one exception the Decisions call out), which the field takes. Every
/// other key while a tool has the keyboard is swallowed, whether the field or the list has it,
/// so nothing reaches the shell and no key release reaches a kitty-mode app.
pub fn takes_key(focus: Option<Focus>, key: &ListKey, ctrl: bool, alt: bool, win: bool) -> bool {
    let Some(focus) = focus else { return false };
    if win || alt {
        return false;
    }
    if ctrl {
        return matches!(key, ListKey::Paste) && matches!(focus, Focus::Field);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_go_to_a_focused_tool_only() {
        assert!(!takes_key(None, &ListKey::Text("a".into()), false, false, false));
        assert!(takes_key(Some(Focus::Field), &ListKey::Text("a".into()), false, false, false));
        // Swallowed: nothing reaches the shell while the list has the keyboard either.
        assert!(takes_key(Some(Focus::List), &ListKey::Text("a".into()), false, false, false));

        for focus in [Focus::Field, Focus::List] {
            assert!(!takes_key(Some(focus), &ListKey::Text("c".into()), true, false, false), "Ctrl+C");
            assert!(!takes_key(Some(focus), &ListKey::Text("x".into()), false, true, false), "Alt+x");
            assert!(!takes_key(Some(focus), &ListKey::Text("a".into()), false, false, true), "Win+anything");
        }

        // The one exception: Ctrl+V into a field.
        assert!(takes_key(Some(Focus::Field), &ListKey::Paste, true, false, false));
    }

    #[test]
    fn field_and_list_keys_follow_iterm2() {
        let mut f = Field::default();
        assert_eq!(f.key(&ListKey::Text("a".into()), false, false), FieldOut::Changed);
        assert_eq!(f.key(&ListKey::Text("b".into()), false, false), FieldOut::Changed);
        assert_eq!(f.text, "ab");
        assert_eq!(f.key(&ListKey::Backspace, false, false), FieldOut::Changed);
        assert_eq!(f.text, "a");

        // Esc with esc_clears (Profiles) on non-empty text clears it.
        assert_eq!(f.key(&ListKey::Escape, true, false), FieldOut::Changed);
        assert_eq!(f.text, "");
        // Esc with esc_clears on empty text gives the keyboard back.
        assert_eq!(f.key(&ListKey::Escape, true, false), FieldOut::GiveBack);
        // Esc without esc_clears always gives it back.
        f.text = "x".into();
        assert_eq!(f.key(&ListKey::Escape, false, false), FieldOut::GiveBack);
        assert_eq!(f.text, "x");

        // Up and Down move the list only where arrows_to_list; Profiles has it off.
        assert_eq!(f.key(&ListKey::Up, false, false), FieldOut::Kept);
        assert_eq!(f.key(&ListKey::Down, false, false), FieldOut::Kept);
        assert_eq!(f.key(&ListKey::Up, false, true), FieldOut::Up);
        assert_eq!(f.key(&ListKey::Down, false, true), FieldOut::Down);

        assert_eq!(f.key(&ListKey::Enter, false, false), FieldOut::Enter);
        assert_eq!(f.key(&ListKey::Tab, false, false), FieldOut::ToList);

        // Selection.
        let mut s = Selection::default();
        assert!(!s.click(2, false, false, true, 0, 500));
        assert_eq!(s.rows, vec![2]);
        assert!(!s.click(5, false, true, true, 100, 500));
        assert_eq!(s.rows, vec![2, 3, 4, 5]);
        assert!(!s.click(3, true, false, true, 200, 500));
        assert_eq!(s.rows, vec![2, 4, 5]);

        // A second click on the same row within double_ms is a double-click; after it, not.
        assert!(!s.click(4, false, false, true, 300, 500));
        assert!(s.click(4, false, false, true, 350, 500));
        assert!(!s.click(4, false, false, true, 900, 500));

        // multi false keeps one row even with Ctrl and Shift held.
        let mut single = Selection::default();
        single.click(2, false, false, true, 0, 500);
        assert!(!single.click(5, true, true, false, 100, 500));
        assert_eq!(single.rows, vec![5]);

        let mut step = Selection::default();
        step.step(true, 4);
        assert_eq!(step.rows, vec![0]);
        let mut step2 = Selection::default();
        step2.rows = vec![3];
        step2.step(true, 4);
        assert_eq!(step2.rows, vec![3]);

        let mut ret = Selection::default();
        ret.rows = vec![0, 1, 2, 3];
        ret.retain(2);
        assert_eq!(ret.rows, vec![0, 1]);
    }

    #[test]
    fn list_layout_follows_iterm2() {
        let area = Rect { x: 0.0, y: 0.0, w: 242.0, h: 300.0 };
        let spec = ListSpec { field: true, top: vec![], left: vec![], right: vec![ToolButton::Add, ToolButton::Remove, ToolButton::Edit], popup_w: 0.0 };
        let row_heights = [20.0_f32, 20.0, 20.0];
        let l = layout(area, &spec, &row_heights, 0.0, 1.0);

        assert_eq!(l.field, Some(Rect { x: 0.0, y: 0.0, w: 242.0, h: 22.0 }));
        assert_eq!(l.list, Rect { x: 0.0, y: 26.0, w: 242.0, h: 248.0 });

        // Right buttons are laid out right to left, so on screen (left to right) they read
        // Edit, Remove, Add, with Add flush against the right edge.
        let mut by_x = l.buttons.clone();
        by_x.sort_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap());
        assert_eq!(by_x.iter().map(|b| b.0).collect::<Vec<_>>(), vec![ToolButton::Edit, ToolButton::Remove, ToolButton::Add]);
        let add = by_x[2].1;
        assert_eq!(add.x + add.w, area.w);
        assert_eq!(add.w, 22.0);

        // Rows stack from the list's top, minus scroll.
        assert_eq!(l.rows[0], Rect { x: 0.0, y: 26.0, w: 242.0, h: 20.0 });
        assert_eq!(l.rows[1], Rect { x: 0.0, y: 46.0, w: 242.0, h: 20.0 });
        assert_eq!(l.rows[2], Rect { x: 0.0, y: 66.0, w: 242.0, h: 20.0 });
        assert_eq!(l.content_h, 60.0);
        assert_eq!(end_scroll(&l), 0.0);

        let scrolled = layout(area, &spec, &[20.0; 20], 30.0, 1.0);
        assert_eq!(scrolled.rows[0].y, 26.0 - 30.0);
        assert_eq!(end_scroll(&scrolled), scrolled.content_h - scrolled.list.h);
        assert_eq!(clamp_scroll(10_000.0, &scrolled), end_scroll(&scrolled));
        assert_eq!(clamp_scroll(-5.0, &scrolled), 0.0);

        // hit finds each part.
        assert_eq!(hit(&l, 100.0, 10.0), ListHit::Field);
        assert_eq!(hit(&l, 100.0, 30.0), ListHit::Row(0));
        let edit = by_x[0].1;
        assert_eq!(hit(&l, edit.x + 1.0, edit.y + 1.0), ListHit::Button(ToolButton::Edit));
        assert_eq!(hit(&l, 100.0, 200.0), ListHit::Empty);
        assert_eq!(hit(&l, 100.0, 299.0), ListHit::Nothing);

        let spec_popup = ListSpec { field: true, top: vec![], left: vec![], right: vec![], popup_w: 100.0 };
        let lp = layout(area, &spec_popup, &row_heights, 0.0, 1.0);
        let popup = lp.popup.expect("popup present when popup_w > 0");
        assert_eq!(hit(&lp, popup.x + 1.0, popup.y + 1.0), ListHit::Popup);

        // Scale doubles the layout's own point constants; row_heights are already pixels, so
        // they pass straight through untouched (they come from measured text at the render
        // scale, like clippings::layout's rows).
        let area2 = Rect { x: 0.0, y: 0.0, w: 484.0, h: 600.0 };
        let l2 = layout(area2, &spec, &row_heights, 0.0, 2.0);
        assert_eq!(l2.field, Some(Rect { x: 0.0, y: 0.0, w: 484.0, h: 44.0 }));
        assert_eq!(l2.list, Rect { x: 0.0, y: 52.0, w: 484.0, h: 496.0 });
        assert_eq!(l2.rows[0], Rect { x: 0.0, y: 52.0, w: 484.0, h: 20.0 });
        let add2 = l2.buttons.iter().find(|(b, _)| *b == ToolButton::Add).unwrap().1;
        assert_eq!(add2.w, 44.0);
        assert_eq!(add2.x + add2.w, area2.w);

        // Tail truncation with a fixed-width measure.
        let w = 10.0_f32;
        let width = |s: &str| s.chars().count() as f32 * w;
        assert_eq!(truncate_tail("abcdef", 4.0 * w, &width), "abc\u{2026}");
        assert_eq!(truncate_tail("abc", 4.0 * w, &width), "abc");

        assert_eq!(ToolButton::Edit.label(), "Edit");
        assert_eq!(ToolButton::Trash.label(), "Clear");
    }
}
