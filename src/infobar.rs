//! iTerm2's announcement bar: a strip across the top of a pane that tells the user
//! something or asks, with buttons on the right and a close box. Each pane keeps a queue;
//! the front bar shows.

use crate::layout::Rect;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    /// Which question this is; a second bar with the same id does not queue.
    pub id: String,
    pub text: String,
    pub buttons: Vec<String>,
    /// An important bar stays until answered; any other is dismissed by a key press.
    pub important: bool,
}

impl Bar {
    /// A button's title as shown: `_X` marks Alt+X and shows as X.
    pub fn label(&self, i: usize) -> String {
        self.buttons.get(i).map(|b| b.replacen('_', "", 1)).unwrap_or_default()
    }

    /// The button Alt+`letter` presses.
    pub fn shortcut(&self, letter: char) -> Option<usize> {
        let want = letter.to_ascii_uppercase();
        self.buttons.iter().position(|b| b.split_once('_').and_then(|(_, rest)| rest.chars().next()).is_some_and(|c| c.to_ascii_uppercase() == want))
    }

    /// The text as shown: a bar a key dismisses says so.
    pub fn shown_text(&self) -> String {
        if self.important { self.text.clone() } else { format!("{} Press any key to dismiss this message.", self.text) }
    }
}

#[derive(Default)]
pub struct Bars {
    queue: std::collections::VecDeque<Bar>,
}

impl Bars {
    /// Queues a bar. A waiting bar with the same id is replaced (its request is dropped)
    /// and returned.
    pub fn push(&mut self, bar: Bar) -> Option<Bar> {
        if let Some(old) = self.queue.iter_mut().find(|b| b.id == bar.id) {
            return Some(std::mem::replace(old, bar));
        }
        self.queue.push_back(bar);
        None
    }

    pub fn front(&self) -> Option<&Bar> {
        self.queue.front()
    }

    pub fn pop(&mut self) -> Option<Bar> {
        self.queue.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// Where the bar goes in a pane: its strip, and a rectangle per button (in button order,
/// right-aligned before the close box), and the close box.
pub struct Layout {
    pub bar: Rect,
    pub buttons: Vec<Rect>,
    pub close: Rect,
}

pub fn layout(bar: &Bar, area: Rect, cell: (f32, f32)) -> Layout {
    let h = cell.1 + 8.0;
    let strip = Rect { x: area.x, y: area.y, w: area.w, h };
    let inner = Rect { x: 0.0, y: area.y + 2.0, w: 0.0, h: h - 4.0 };
    let close = Rect { x: area.x + area.w - 2.0 * cell.0, w: 2.0 * cell.0, ..inner };
    let mut right = close.x - cell.0;
    let mut buttons: Vec<Rect> = bar
        .buttons
        .iter()
        .enumerate()
        .rev()
        .map(|(i, _)| {
            let w = (bar.label(i).chars().count() as f32 + 2.0) * cell.0;
            right -= w;
            let r = Rect { x: right, w, ..inner };
            right -= cell.0;
            r
        })
        .collect();
    buttons.reverse();
    Layout { bar: strip, buttons, close }
}

/// What a click at (x, y) does: `Some(Some(i))` presses button i, `Some(None)` closes
/// the bar, `None` is not on a button.
pub fn hit(bar: &Bar, area: Rect, cell: (f32, f32), x: f32, y: f32) -> Option<Option<usize>> {
    let l = layout(bar, area, cell);
    if l.close.contains(x, y) {
        return Some(None);
    }
    l.buttons.iter().position(|r| r.contains(x, y)).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(id: &str, buttons: &[&str]) -> Bar {
        Bar { id: id.into(), text: "Share clipboard contents with app in terminal?".into(), buttons: buttons.iter().map(|b| b.to_string()).collect(), important: true }
    }

    const AREA: Rect = Rect { x: 100.0, y: 50.0, w: 800.0, h: 400.0 };
    const CELL: (f32, f32) = (8.0, 16.0);

    #[test]
    fn same_id_replaces_the_bar() {
        let mut bars = Bars::default();
        assert_eq!(bars.push(bar("paste", &["Just Once"])), None);
        assert_eq!(bars.push(bar("denied", &["OK"])), None);
        let replaced = bars.push(Bar { text: "newer".into(), ..bar("paste", &["Again"]) });
        assert_eq!(replaced.map(|b| b.buttons), Some(vec!["Just Once".to_string()]), "the older request is dropped");
        assert_eq!(bars.front().map(|b| b.text.as_str()), Some("newer"), "in the old one's place");
        bars.pop();
        assert_eq!(bars.front().map(|b| b.id.as_str()), Some("denied"));
        bars.pop();
        assert!(bars.is_empty());
    }

    #[test]
    fn alt_letter_presses_a_button() {
        let b = Bar { important: false, ..bar("p", &["Always _Allow", "Always _Deny", "Never"]) };
        assert_eq!(b.label(0), "Always Allow");
        assert_eq!(b.shortcut('d'), Some(1));
        assert_eq!(b.shortcut('A'), Some(0));
        assert_eq!(b.shortcut('n'), None, "no underscore, no shortcut");
        assert_eq!(b.shown_text(), "Share clipboard contents with app in terminal? Press any key to dismiss this message.");
        assert_eq!(bar("p", &[]).shown_text(), "Share clipboard contents with app in terminal?", "important bars stay until answered");
        let l = layout(&b, AREA, CELL);
        assert_eq!(l.buttons[0].w, (12.0 + 2.0) * 8.0, "the shown title's width");
    }

    #[test]
    fn buttons_lay_out_right_to_left() {
        let b = bar("paste", &["Just Once", "Always", "Never"]);
        let l = layout(&b, AREA, CELL);
        assert_eq!(l.bar, Rect { x: 100.0, y: 50.0, w: 800.0, h: 24.0 });
        assert_eq!(l.close.x + l.close.w, 900.0, "close box at the right edge");
        assert_eq!(l.buttons.len(), 3);
        assert!(l.buttons[0].x < l.buttons[1].x && l.buttons[1].x < l.buttons[2].x, "in order");
        assert!(l.buttons[2].x + l.buttons[2].w <= l.close.x, "before the close box");
        assert_eq!(l.buttons[0].w, (9.0 + 2.0) * 8.0, "label width plus a cell each side");
    }

    #[test]
    fn clicks_hit_buttons_and_close() {
        let b = bar("paste", &["Just Once", "Always"]);
        let l = layout(&b, AREA, CELL);
        let mid = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (x, y) = mid(l.buttons[1]);
        assert_eq!(hit(&b, AREA, CELL, x, y), Some(Some(1)));
        let (x, y) = mid(l.close);
        assert_eq!(hit(&b, AREA, CELL, x, y), Some(None));
        assert_eq!(hit(&b, AREA, CELL, 110.0, 60.0), None, "on the text");
        assert_eq!(hit(&b, AREA, CELL, 110.0, 200.0), None, "below the bar");
    }
}
