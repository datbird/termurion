//! iTerm2's Clippings: each session's list of notes (a title and a Markdown detail), the
//! archive of earlier lists, and the panel that shows them on the right of the session.

use toml::{Table, Value};

use crate::layout::Rect;

/// At most this many archived lists (iTerm2's maxClippingsArchiveEntries).
pub const MAX_ARCHIVE: usize = 50;
/// The panel's width: its default and limits, in logical pixels.
pub const DEFAULT_WIDTH: f32 = 300.0;
pub const MIN_WIDTH: f32 = 225.0;
pub const MAX_WIDTH: f32 = 1200.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clipping {
    /// iTerm2's `type`: kept, never shown.
    pub kind: String,
    pub title: String,
    pub detail: String,
}

/// Which panel buttons work now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enabled {
    pub add: bool,
    pub remove: bool,
    pub archive: bool,
    pub send: bool,
    pub copy: bool,
    pub back: bool,
    pub forward: bool,
}

/// One session's clippings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clippings {
    pub live: Vec<Clipping>,
    /// Oldest first.
    pub archive: Vec<Vec<Clipping>>,
    /// The archive entry shown, or None for the live list.
    pub view: Option<usize>,
    pub visible: bool,
}

impl Clippings {
    pub fn viewing_live(&self) -> bool {
        self.view.is_none_or(|i| i >= self.archive.len())
    }

    /// The list the panel shows.
    pub fn shown(&self) -> &[Clipping] {
        match self.view {
            Some(i) if i < self.archive.len() => &self.archive[i],
            _ => &self.live,
        }
    }

    /// A new clipping: the view goes back to the live list and the panel shows.
    pub fn add(&mut self, c: Clipping) {
        self.live.push(c);
        self.view = None;
        self.visible = true;
    }

    /// Removes rows of the live list (nothing while an archive shows).
    pub fn remove(&mut self, rows: &[usize]) {
        if !self.viewing_live() {
            return;
        }
        let mut rows = rows.to_vec();
        rows.sort_unstable();
        rows.dedup();
        for r in rows.into_iter().rev() {
            if r < self.live.len() {
                self.live.remove(r);
            }
        }
    }

    /// Moves the live list into the archive. False when it is empty.
    pub fn archive_all(&mut self) -> bool {
        if self.live.is_empty() {
            return false;
        }
        self.archive.push(std::mem::take(&mut self.live));
        if self.archive.len() > MAX_ARCHIVE {
            let extra = self.archive.len() - MAX_ARCHIVE;
            self.archive.drain(..extra);
        }
        self.view = None;
        true
    }

    /// The archive arrows: back to older lists, forward to newer ones and the live list.
    pub fn step(&mut self, back: bool) {
        let total = self.archive.len();
        let current = if self.viewing_live() { total } else { self.view.unwrap_or(total) };
        let target = if back { current.saturating_sub(1) } else { (current + 1).min(total) };
        self.view = (target < total).then_some(target);
    }

    /// Drag reorder of the live list: the row at `from` goes before the row at `to`
    /// (`to` may be the length, for the end). Returns the row's new index.
    pub fn move_row(&mut self, from: usize, to: usize) -> Option<usize> {
        if !self.viewing_live() || from >= self.live.len() || to > self.live.len() {
            return None;
        }
        let c = self.live.remove(from);
        let at = if to > from { to - 1 } else { to };
        self.live.insert(at, c);
        Some(at)
    }

    pub fn enabled(&self, selected: usize) -> Enabled {
        let live = self.viewing_live();
        let total = self.archive.len();
        let idx = if live { total } else { self.view.unwrap_or(total) };
        Enabled {
            add: live,
            remove: live && selected > 0,
            archive: live && !self.shown().is_empty(),
            send: selected > 0,
            copy: selected > 0,
            back: idx > 0,
            forward: idx < total,
        }
    }

    /// While an archive shows: "N/M" and its tooltip.
    pub fn status(&self) -> Option<(String, String)> {
        if self.viewing_live() {
            return None;
        }
        let (n, m) = (self.view.unwrap_or(0) + 1, self.archive.len());
        Some((format!("{n}/{m}"), format!("Viewing archived clippings ({n} of {m})")))
    }

    /// The session arrangement keys.
    pub fn to_table(&self) -> Table {
        let mut t = Table::new();
        if *self == Clippings::default() {
            return t;
        }
        t.insert("Clippings".into(), list_value(&self.live));
        t.insert("Clippings Visible".into(), Value::Boolean(self.visible));
        t.insert("Clippings Archive".into(), Value::Array(self.archive.iter().map(|l| list_value(l)).collect()));
        t.insert("Clippings View Index".into(), Value::Integer(self.view.map_or(-1, |i| i as i64)));
        t
    }

    pub fn from_table(t: &Table) -> Clippings {
        let live = t.get("Clippings").map(list_of).unwrap_or_default();
        let archive: Vec<Vec<Clipping>> = t.get("Clippings Archive").and_then(Value::as_array).map(|a| a.iter().map(list_of).collect()).unwrap_or_default();
        let index = t.get("Clippings View Index").and_then(Value::as_integer).unwrap_or(-1);
        let view = usize::try_from(index).ok().filter(|&i| i < archive.len());
        let visible = t.get("Clippings Visible").and_then(Value::as_bool).unwrap_or(false);
        Clippings { live, archive, view, visible }
    }
}

fn list_value(list: &[Clipping]) -> Value {
    Value::Array(
        list.iter()
            .map(|c| {
                let mut t = Table::new();
                t.insert("type".into(), Value::String(c.kind.clone()));
                t.insert("title".into(), Value::String(c.title.clone()));
                t.insert("detail".into(), Value::String(c.detail.clone()));
                Value::Table(t)
            })
            .collect(),
    )
}

/// A list of `{type, title, detail}`; an entry missing one of them is dropped (iTerm2's).
fn list_of(v: &Value) -> Vec<Clipping> {
    let Some(a) = v.as_array() else { return Vec::new() };
    a.iter()
        .filter_map(|x| {
            let t = x.as_table()?;
            let s = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_string);
            Some(Clipping { kind: s("type")?, title: s("title")?, detail: s("detail")? })
        })
        .collect()
}

/// Send and Copy: each clipping as `**title**\ndetail` (the detail alone when the title
/// is blank), joined by the separator in Vim syntax.
pub fn format(items: &[&Clipping], separator: &str) -> String {
    let sep = crate::keyaction::expand(separator, crate::keyaction::Escaping::Vim);
    items
        .iter()
        .map(|c| {
            let title = c.title.trim();
            if title.is_empty() { c.detail.clone() } else { format!("**{title}**\n{}", c.detail) }
        })
        .collect::<Vec<_>>()
        .join(&sep)
}

/// The detail's Markdown as the plain text the panel draws.
pub fn plain(markdown: &str) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};
    let mut out = String::new();
    for e in Parser::new(markdown) {
        match e {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::BlockQuote(_)) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    out.trim_end().to_string()
}

pub fn clamp_width(w: f32) -> f32 {
    w.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The panel's buttons, in the bottom strip's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Add,
    Remove,
    Archive,
    Send,
    Copy,
    Back,
    Forward,
    Close,
}

impl Button {
    /// iTerm2's tooltips.
    pub fn tooltip(self) -> &'static str {
        match self {
            Button::Add => "Add clipping",
            Button::Remove => "Remove selected clipping",
            Button::Archive => "Archive all clippings",
            Button::Send => "Send selected to terminal",
            Button::Copy => "Copy selected to pasteboard",
            Button::Back => "Show previous archived clippings",
            Button::Forward => "Show next archived clippings",
            Button::Close => "Hide clippings",
        }
    }
}

/// Where things are in a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The list's area (rows are clipped to it).
    pub list: Rect,
    /// Each row, scrolled; rows outside `list` are not drawn.
    pub rows: Vec<Rect>,
    pub buttons: Vec<(Button, Rect)>,
    /// Where "N/M" goes.
    pub status: Rect,
    /// The left edge the width is dragged by.
    pub resize: Rect,
    /// The rows' whole height (for scrolling).
    pub content_h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Row(usize),
    Button(Button),
    Resize,
    Nothing,
}

/// A pane's terminal rectangle and, when the panel shows, the panel's (on the right; it
/// takes at most three quarters of the pane).
pub fn split_rect(pane: Rect, visible: bool, width: f32) -> (Rect, Option<Rect>) {
    if !visible {
        return (pane, None);
    }
    let w = width.min(pane.w * 0.75);
    let term = Rect { w: pane.w - w, ..pane };
    (term, Some(Rect { x: pane.x + pane.w - w, w, ..pane }))
}

/// The panel's layout. `rows` are each row's (title, detail) text heights; `scroll` is
/// how far the list is scrolled down.
pub fn layout(panel: Rect, rows: &[(f32, f32)], scroll: f32, scale: f32) -> Layout {
    let strip_h = 28.0 * scale;
    let list = Rect { h: (panel.h - strip_h).max(0.0), ..panel };
    let (pad, gap) = (6.0 * scale, 2.0 * scale);
    let mut y = list.y - scroll;
    let rows: Vec<Rect> = rows
        .iter()
        .map(|&(title, detail)| {
            let h = pad * 2.0 + title + gap + detail;
            let r = Rect { x: panel.x, y, w: panel.w, h };
            y += h;
            r
        })
        .collect();
    let content_h = rows.iter().map(|r| r.h).sum();
    let b = 22.0 * scale;
    let by = list.y + list.h + (strip_h - b) / 2.0;
    let mut x = panel.x + pad;
    let mut buttons = Vec::new();
    for group in [&[Button::Add, Button::Remove, Button::Archive][..], &[Button::Send, Button::Copy], &[Button::Back, Button::Forward]] {
        for &button in group {
            buttons.push((button, Rect { x, y: by, w: b, h: b }));
            x += b;
        }
        x += pad;
    }
    let close_x = panel.x + panel.w - pad - b;
    let status = Rect { x, y: by, w: (close_x - x - pad).max(0.0), h: b };
    buttons.push((Button::Close, Rect { x: close_x, y: by, w: b, h: b }));
    let resize = Rect { x: panel.x, y: panel.y, w: 4.0 * scale, h: list.h };
    Layout { list, rows, buttons, status, resize, content_h }
}

pub fn hit(l: &Layout, x: f32, y: f32) -> Hit {
    if l.resize.contains(x, y) {
        return Hit::Resize;
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return Hit::Button(*b);
    }
    if !l.list.contains(x, y) {
        return Hit::Nothing;
    }
    l.rows.iter().position(|r| r.contains(x, y)).map_or(Hit::Nothing, Hit::Row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(title: &str, detail: &str) -> Clipping {
        Clipping { kind: String::new(), title: title.into(), detail: detail.into() }
    }

    #[test]
    fn clippings_format_like_iterm2() {
        let a = c("  A ", "one");
        let b = c(" ", "two");
        assert_eq!(format(&[&a, &b], r"\n--\n"), "**A**\none\n--\ntwo");
        assert_eq!(format(&[&b], r"\n--\n"), "two");
        assert_eq!(format(&[&a, &b], r"\t"), "**A**\none\ttwo");
        assert_eq!(plain("Use **bold** and `code`.\n\n- item one\n- item two"), "Use bold and code.\nitem one\nitem two");
        assert_eq!(clamp_width(100.0), MIN_WIDTH);
        assert_eq!(clamp_width(5000.0), MAX_WIDTH);
        assert_eq!(clamp_width(400.0), 400.0);
    }

    #[test]
    fn archives_keep_fifty() {
        let mut k = Clippings::default();
        assert!(!k.archive_all());
        k.add(c("a", "1"));
        assert!(k.visible);
        for i in 0..MAX_ARCHIVE {
            assert!(k.archive_all());
            k.add(c(&format!("x{i}"), ""));
        }
        assert!(k.archive_all());
        assert_eq!(k.archive.len(), MAX_ARCHIVE);
        // The first list ("a") went; the oldest now is "x0".
        assert_eq!(k.archive[0][0].title, "x0");
        assert!(k.live.is_empty() && k.viewing_live());
        // Stepping back from the live list goes to the newest archive, then older.
        k.step(true);
        assert_eq!(k.view, Some(MAX_ARCHIVE - 1));
        k.step(true);
        assert_eq!(k.view, Some(MAX_ARCHIVE - 2));
        k.step(false);
        k.step(false);
        assert_eq!(k.view, None);
        k.step(false);
        assert_eq!(k.view, None);
        // Adding while an archive shows goes back to the live list.
        k.view = Some(0);
        k.add(c("new", ""));
        assert!(k.viewing_live());
        assert_eq!(k.shown().len(), 1);
    }

    #[test]
    fn buttons_follow_the_view() {
        let mut k = Clippings::default();
        let e = k.enabled(0);
        assert!(e.add && !e.remove && !e.archive && !e.send && !e.copy && !e.back && !e.forward);
        k.add(c("a", "1"));
        k.add(c("b", "2"));
        let e = k.enabled(1);
        assert!(e.add && e.remove && e.archive && e.send && e.copy && !e.back && !e.forward);
        assert_eq!(k.status(), None);
        k.archive_all();
        k.add(c("c", "3"));
        k.step(true);
        let e = k.enabled(1);
        assert!(!e.add && !e.remove && !e.archive && e.send && e.copy && !e.back && e.forward);
        assert_eq!(k.status(), Some(("1/1".to_string(), "Viewing archived clippings (1 of 1)".to_string())));
        // Removing and moving do nothing on an archive.
        k.remove(&[0]);
        assert_eq!(k.move_row(0, 2), None);
        assert_eq!(k.shown().len(), 2);
        k.step(false);
        k.remove(&[0]);
        assert!(k.live.is_empty());
        let mut m = Clippings::default();
        for t in ["a", "b", "c"] {
            m.add(c(t, ""));
        }
        assert_eq!(m.move_row(0, 3), Some(2));
        assert_eq!(m.live.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
        assert_eq!(m.move_row(2, 0), Some(0));
        assert_eq!(m.live.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        m.remove(&[2, 0]);
        assert_eq!(m.live, vec![c("b", "")]);
    }

    #[test]
    fn clippings_round_trip_arrangements() {
        let mut k = Clippings::default();
        k.add(Clipping { kind: "review".into(), title: "t".into(), detail: "d".into() });
        k.archive_all();
        k.add(c("live", "x"));
        k.view = Some(0);
        let t = k.to_table();
        assert_eq!(t.get("Clippings View Index").and_then(Value::as_integer), Some(0));
        assert_eq!(t.get("Clippings Visible").and_then(Value::as_bool), Some(true));
        assert_eq!(Clippings::from_table(&t), k);
        // A view past the archive shows the live list; a bad entry is dropped.
        let mut t2 = k.to_table();
        t2.insert("Clippings View Index".into(), Value::Integer(7));
        let mut list = t2["Clippings"].as_array().unwrap().clone();
        list.push(Value::String("junk".into()));
        t2.insert("Clippings".into(), Value::Array(list));
        let back = Clippings::from_table(&t2);
        assert_eq!(back.view, None);
        assert_eq!(back.live.len(), 1);
        assert_eq!(Clippings::from_table(&Table::new()), Clippings::default());
        assert_eq!(Clippings::default().to_table(), Table::new());
    }

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn panel_takes_its_width() {
        let pane = r(100.0, 20.0, 1000.0, 600.0);
        assert_eq!(split_rect(pane, false, 300.0), (pane, None));
        assert_eq!(split_rect(pane, true, 300.0), (r(100.0, 20.0, 700.0, 600.0), Some(r(800.0, 20.0, 300.0, 600.0))));
        // A narrow pane keeps a quarter for the terminal.
        assert_eq!(split_rect(r(0.0, 0.0, 400.0, 100.0), true, 300.0), (r(0.0, 0.0, 100.0, 100.0), Some(r(100.0, 0.0, 300.0, 100.0))));
        // Two panes of one split: each takes its own panel.
        let (left, right) = (r(0.0, 0.0, 500.0, 300.0), r(504.0, 0.0, 496.0, 300.0));
        assert_eq!(split_rect(left, true, 225.0).0.w, 275.0);
        assert_eq!(split_rect(right, false, 225.0).0, right);
    }

    #[test]
    fn panel_hits_rows_and_buttons() {
        let panel = r(700.0, 0.0, 300.0, 400.0);
        let l = layout(panel, &[(16.0, 30.0), (16.0, 100.0)], 0.0, 1.0);
        // Rows: 6 padding, title, 2, detail, 6 padding.
        assert_eq!(l.rows[0], r(700.0, 0.0, 300.0, 60.0));
        assert_eq!(l.rows[1], r(700.0, 60.0, 300.0, 130.0));
        assert_eq!(l.content_h, 190.0);
        // The strip at the bottom holds the buttons, Close at the right.
        assert_eq!(l.list, r(700.0, 0.0, 300.0, 372.0));
        assert_eq!(l.buttons.iter().map(|b| b.0).collect::<Vec<_>>(), [Button::Add, Button::Remove, Button::Archive, Button::Send, Button::Copy, Button::Back, Button::Forward, Button::Close]);
        let close = l.buttons[7].1;
        assert!(close.x + close.w <= 1000.0 && close.x + close.w > 990.0);
        assert_eq!(hit(&l, 750.0, 70.0), Hit::Row(1));
        assert_eq!(hit(&l, 750.0, 300.0), Hit::Nothing);
        let add = l.buttons[0].1;
        assert_eq!(hit(&l, add.x + 2.0, add.y + 2.0), Hit::Button(Button::Add));
        assert_eq!(hit(&l, 701.0, 100.0), Hit::Resize);
        // Scrolled down 50: row 0 starts above the list; a row under the strip is not hit.
        let l = layout(panel, &[(16.0, 30.0), (16.0, 100.0), (16.0, 100.0), (16.0, 100.0)], 50.0, 1.0);
        assert_eq!(l.rows[0].y, -50.0);
        assert_eq!(hit(&l, 750.0, 5.0), Hit::Row(0));
        // The status area of the strip, over a row scrolled under it.
        assert_eq!(hit(&l, 920.0, 380.0), Hit::Nothing);
        // Scale doubles the fixed sizes.
        let l2 = layout(panel, &[(16.0, 30.0)], 0.0, 2.0);
        assert_eq!(l2.rows[0].h, 16.0 + 30.0 + 2.0 * 14.0);
        assert_eq!(l2.list.h, 400.0 - 56.0);
    }
}
