//! The find bar: iTerm2's search as you type in the focused pane, the match count, and
//! moving between matches.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use crate::find::{self, Match, Pos, Results};

/// iTerm2 waits this long before searching for a query of two characters or fewer.
const SHORT_QUERY_DELAY: Duration = Duration::from_secs(1);
/// New output searches again after this pause, so a stream of output searches rarely.
const OUTPUT_DELAY: Duration = Duration::from_millis(150);

/// A key the find bar takes while it is open.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum BarKey {
    Text(String),
    Backspace,
    /// Return: Find Next, which goes up.
    Next,
    /// Shift+Return.
    Previous,
    Close,
    /// Tab: grow the selection by a word and search for it.
    Grow,
    /// Alt+Return: paste the current match into the session.
    PasteMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Nothing,
    Step { up: bool },
    Close,
    Grow,
    PasteMatch,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FindState {
    pub open: bool,
    pub query: String,
    pub results: Results,
    /// The query is not a valid regular expression.
    pub error: Option<String>,
    /// A search waits to run: the query changed, or the output did.
    pub pending: bool,
    /// The pending search is for a new query, which starts at the bottom of the view.
    pub fresh: bool,
    pub changed_at: Option<Instant>,
    /// Keys go to the bar, not the terminal. A click in the pane takes them back.
    pub typing: bool,
}

impl FindState {
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Replaces the query, as Use Selection for Find does.
    pub fn set_query(&mut self, query: &str, now: Instant) {
        self.query = query.to_string();
        self.edited(now);
    }

    fn edited(&mut self, now: Instant) {
        self.pending = true;
        self.fresh = true;
        self.changed_at = Some(now);
    }

    /// New output arrived: search again soon, keeping the current match.
    pub fn output_changed(&mut self, now: Instant) {
        if !self.query.is_empty() && !self.pending {
            self.pending = true;
            self.fresh = false;
            self.changed_at = Some(now);
        }
    }

    /// When the pending search runs, if one waits.
    pub fn due(&self) -> Option<Instant> {
        if !self.pending {
            return None;
        }
        let at = self.changed_at?;
        Some(if !self.fresh {
            at + OUTPUT_DELAY
        } else if !self.query.is_empty() && self.query.chars().count() <= 2 {
            at + SHORT_QUERY_DELAY
        } else {
            at
        })
    }

    /// Takes the pending search if it is due now.
    pub fn take_due(&mut self, now: Instant) -> Option<bool> {
        if self.due().is_some_and(|d| now >= d) {
            self.pending = false;
            Some(self.fresh)
        } else {
            None
        }
    }

    pub fn key(&mut self, key: BarKey, now: Instant) -> Outcome {
        match key {
            BarKey::Text(t) => {
                self.query.push_str(&t);
                self.edited(now);
                Outcome::Nothing
            }
            BarKey::Backspace => {
                if self.query.pop().is_some() {
                    self.edited(now);
                }
                Outcome::Nothing
            }
            BarKey::Next => Outcome::Step { up: true },
            BarKey::Previous => Outcome::Step { up: false },
            BarKey::Close => {
                self.close();
                Outcome::Close
            }
            BarKey::Grow => Outcome::Grow,
            BarKey::PasteMatch => Outcome::PasteMatch,
        }
    }

    /// Closes the bar. The query stays for the next Find, as in iTerm2; the matches go.
    pub fn close(&mut self) {
        self.open = false;
        self.typing = false;
        self.pending = false;
        self.error = None;
        self.results = Results::default();
    }
}

/// Menu item ids of the find bar's mode menu.
const MODE_ID: &str = "tandem.findmode.";

pub(super) fn to_point(screen: &crate::screen::Screen, p: Pos) -> Point {
    let history = screen.term().grid().history_size() as i32;
    Point::new(Line(p.row as i32 - history), Column(p.col))
}

pub(super) fn to_pos(screen: &crate::screen::Screen, p: Point) -> Pos {
    let history = screen.term().grid().history_size() as i32;
    Pos { row: (p.line.0 + history).max(0) as usize, col: p.column.0 }
}

/// The last cell on screen, where a new search starts looking up from.
fn view_bottom(screen: &crate::screen::Screen) -> Pos {
    let grid = screen.term().grid();
    let line = grid.screen_lines() as i32 - 1 - grid.display_offset() as i32;
    to_pos(screen, Point::new(Line(line), Column(grid.columns() - 1)))
}

/// Selects `m` and scrolls it into the middle of the view when it is off screen.
pub(super) fn reveal(pane: &mut crate::pane::Pane, m: &Match) {
    let (a, b) = (to_point(&pane.screen, m.start), to_point(&pane.screen, m.end));
    let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
    sel.update(b, Side::Right);
    pane.screen.term_mut().selection = Some(sel);
    show_line(pane, a.line.0);
}

fn show_line(pane: &mut crate::pane::Pane, line: i32) {
    let grid = pane.screen.term().grid();
    let (lines, offset, history) = (grid.screen_lines() as i32, grid.display_offset() as i32, grid.history_size() as i32);
    if line >= -offset && line < -offset + lines {
        return;
    }
    let target = (lines / 2 - line).clamp(0, history);
    pane.scroll(Scroll::Delta(target - offset));
}

impl App {
    pub(super) fn find_mode(&self) -> find::Mode {
        find::Mode::from_i64(self.globals.int("findMode_iTerm"))
    }

    /// The focused pane's find bar, when it is open and has the keyboard.
    fn typing_in_find(&self) -> Option<PaneId> {
        let id = self.focused_id()?;
        self.finds.get(&id).filter(|f| f.open && f.typing).map(|_| id)
    }

    /// A key while the find bar has the keyboard. Returns false for keys the bar leaves
    /// to the terminal (chords with Ctrl or Alt it has no use for).
    pub(super) fn find_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(id) = self.typing_in_find() else { return false };
        let m = self.mods;
        let key = match &ev.logical_key {
            WKey::Named(NamedKey::Enter) if m.alt => BarKey::PasteMatch,
            WKey::Named(NamedKey::Enter) if m.shift => BarKey::Previous,
            WKey::Named(NamedKey::Enter) => BarKey::Next,
            WKey::Named(NamedKey::Escape) => BarKey::Close,
            WKey::Named(NamedKey::Backspace) => BarKey::Backspace,
            WKey::Named(NamedKey::Tab) if !m.ctrl => BarKey::Grow,
            WKey::Character(c) if m.ctrl && !m.alt && c.eq_ignore_ascii_case("v") => {
                match self.clipboard.as_mut().map(|cb| cb.get_text()) {
                    Some(Ok(t)) => BarKey::Text(t.replace(['\r', '\n'], " ")),
                    Some(Err(e)) => {
                        self.banners.set("clipboard", format!("Paste failed: {e}"));
                        return true;
                    }
                    None => return true,
                }
            }
            // AltGr arrives as Ctrl+Alt and types text.
            _ if (!m.ctrl || m.alt) && ev.text.as_deref().is_some_and(|t| !t.chars().any(char::is_control)) => {
                BarKey::Text(ev.text.as_deref().unwrap_or_default().to_string())
            }
            WKey::Named(_) if !m.ctrl && !m.alt => return true,
            _ => return false,
        };
        let now = Instant::now();
        let Some(f) = self.finds.get_mut(&id) else { return false };
        match f.key(key, now) {
            Outcome::Nothing | Outcome::Close => {}
            Outcome::Step { up } => self.find_step(id, up),
            Outcome::Grow => self.find_grow(id),
            Outcome::PasteMatch => {
                let text = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string());
                if let (Some(t), Some(p)) = (text, self.panes.get_mut(&id)) {
                    p.scroll(Scroll::Bottom);
                    p.paste(&t);
                }
            }
        }
        self.redraw();
        true
    }

    /// Runs the searches whose time has come. Returns when the next one is due.
    pub(super) fn run_due_searches(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let due: Vec<(PaneId, bool)> =
            self.finds.iter_mut().filter_map(|(id, f)| f.take_due(now).map(|fresh| (*id, fresh))).collect();
        for (id, fresh) in due {
            self.search(id, fresh);
        }
        self.finds.values().filter_map(FindState::due).min()
    }

    /// Searches a pane for its query. A new query makes the nearest match at or above the
    /// last one (else the bottom of the view) current; new output keeps the current one.
    fn search(&mut self, id: PaneId, fresh: bool) {
        let mode = self.find_mode();
        let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) else { return };
        match find::find_all(&p.screen, &find::pattern(&f.query, mode)) {
            Err(e) => {
                f.error = Some(e);
                f.results = Results::default();
            }
            Ok(matches) if fresh => {
                f.error = None;
                let from = f.results.current.and_then(|i| f.results.matches.get(i)).map_or_else(|| view_bottom(&p.screen), |m| m.start);
                f.results = Results { matches, current: None };
                f.results.start_at(from);
                if let Some(m) = f.results.current.and_then(|i| f.results.matches.get(i)) {
                    reveal(p, &m.clone());
                }
            }
            Ok(matches) => {
                f.error = None;
                f.results.update(matches);
            }
        }
        self.redraw();
    }

    /// Find Next (`up`) or Find Previous, with the pane's query, opening nothing.
    pub(super) fn find_step(&mut self, id: PaneId, up: bool) {
        if self.finds.get(&id).is_some_and(|f| f.results.matches.is_empty() || f.pending) {
            if let Some(f) = self.finds.get_mut(&id) {
                f.pending = false;
            }
            self.search(id, true);
            // The fresh search already made a match current.
            if self.finds.get(&id).is_some_and(|f| f.results.current.is_some()) {
                return;
            }
        }
        let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) else { return };
        let (m, wrapped) = f.results.step(up);
        if let Some(m) = m.copied() {
            reveal(p, &m);
        }
        if wrapped {
            self.banners.set("action", "Search wrapped.");
        }
        self.redraw();
    }

    /// Tab in the find bar: the current match grows by a word to the right, and that
    /// text becomes the query.
    fn find_grow(&mut self, id: PaneId) {
        let Some(p) = self.panes.get(&id) else { return };
        let term = p.screen.term();
        let Some(range) = term.selection.as_ref().and_then(|s| s.to_range(term)) else { return };
        let grid = term.grid();
        let last = Point::new(Line(grid.screen_lines() as i32 - 1), Column(grid.columns() - 1));
        let mut next = range.end;
        next = if next.column.0 + 1 < grid.columns() { Point::new(next.line, next.column + 1) } else { Point::new(next.line + 1, Column(0)) };
        if next > last {
            return;
        }
        // Past the spaces after the match, to the end of the next word.
        while next < last && term.grid()[next.line][next.column].c == ' ' {
            next = if next.column.0 + 1 < grid.columns() { Point::new(next.line, next.column + 1) } else { Point::new(next.line + 1, Column(0)) };
        }
        let end = term.semantic_search_right(next);
        let text = term.bounds_to_string(range.start, end);
        let query = if matches!(self.find_mode(), find::Mode::RegexSensitive | find::Mode::RegexInsensitive) { regex::escape(&text) } else { text };
        if let Some(f) = self.finds.get_mut(&id) {
            f.set_query(&query, Instant::now());
            f.pending = false;
        }
        self.search(id, true);
    }

    /// The find bar's menu commands. Returns false for other selectors.
    pub(super) fn run_find_command(&mut self, selector: &str, label: &str) -> bool {
        let Some(id) = self.focused_id() else { return true };
        let now = Instant::now();
        match (selector, label) {
            ("performFindPanelAction:", "Find...") => {
                let f = self.finds.entry(id).or_default();
                f.open();
                f.typing = true;
                if !f.query.is_empty() {
                    f.set_query(&f.query.clone(), now);
                }
            }
            ("performFindPanelAction:", "Use Selection for Find") => {
                let Some(text) = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()) else { return true };
                let query = if matches!(self.find_mode(), find::Mode::RegexSensitive | find::Mode::RegexInsensitive) { regex::escape(&text) } else { text };
                let f = self.finds.entry(id).or_default();
                f.set_query(&query, now);
            }
            ("findNext:", _) => self.find_step(id, true),
            ("findPrevious:", _) => self.find_step(id, false),
            ("clearFindString:", _) => {
                let f = self.finds.entry(id).or_default();
                f.set_query("", now);
                f.results = Results::default();
            }
            ("jumpToSelection:", _) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    let term = p.screen.term();
                    if let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) {
                        show_line(p, r.start.line.0);
                    }
                }
            }
            _ => return false,
        }
        self.redraw();
        true
    }

    /// Key action 36, Find Regular Expression: the next match up of `regex`, case
    /// sensitive, without touching the find bar's query.
    pub(super) fn find_regex(&mut self, regex: &str) {
        let Some(id) = self.focused_id() else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        match find::find_all(&p.screen, regex) {
            Err(e) => self.banners.set("action", format!("Find Regular Expression: {e}")),
            Ok(matches) => {
                let term = p.screen.term();
                let from = match term.selection.as_ref().and_then(|s| s.to_range(term)) {
                    Some(r) => to_pos(&p.screen, r.start),
                    None => view_bottom(&p.screen),
                };
                let mut r = Results { matches, current: None };
                r.start_at(from);
                // A match at the selection itself is the one already found: go past it.
                if r.current.and_then(|i| r.matches.get(i)).is_some_and(|m| m.start == from) {
                    r.step(true);
                }
                match r.current.and_then(|i| r.matches.get(i)) {
                    Some(m) => reveal(p, &m.clone()),
                    None => self.banners.set("action", "No match."),
                }
            }
        }
        self.redraw();
    }

    /// Opens the find bar's mode menu under the mode label.
    pub(super) fn show_find_mode_menu(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let current = self.find_mode();
        let menu = muda::Menu::new();
        for m in find::Mode::ALL {
            let item = muda::CheckMenuItem::with_id(format!("{MODE_ID}{}", m as i64), m.label(), true, m == current, None);
            if let Err(e) = menu.append(&item) {
                self.banners.set("menu", format!("Find mode menu: {e}"));
            }
        }
        let at = self.find_mode_rect.map(|r| muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(r.x as i32, (r.y + r.h) as i32)));
        use muda::ContextMenu;
        unsafe { menu.show_context_menu_for_hwnd(hwnd, at) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// A pick from the mode menu. Returns false for other menu ids.
    pub(super) fn find_mode_picked(&mut self, id: &str) -> bool {
        let Some(n) = id.strip_prefix(MODE_ID).and_then(|n| n.parse::<i64>().ok()) else { return false };
        self.globals.set("findMode_iTerm", toml::Value::Integer(n));
        self.save_globals();
        let now = Instant::now();
        for f in self.finds.values_mut().filter(|f| !f.query.is_empty()) {
            let q = f.query.clone();
            f.set_query(&q, now);
        }
        self.redraw();
        true
    }

    /// A left press: on the mode label opens the mode menu; in the bar gives it the
    /// keyboard; elsewhere in the pane gives the keyboard back to the terminal.
    /// Returns true when the bar took the press.
    pub(super) fn find_press(&mut self, x: f32, y: f32) -> bool {
        let Some(id) = self.focused_id() else { return false };
        if !self.finds.get(&id).is_some_and(|f| f.open) {
            return false;
        }
        if self.find_mode_rect.is_some_and(|r| r.contains(x, y)) {
            self.show_find_mode_menu();
            return true;
        }
        let in_bar = self.find_bar_rect.is_some_and(|r| r.contains(x, y));
        if let Some(f) = self.finds.get_mut(&id) {
            f.typing = in_bar;
        }
        self.redraw();
        in_bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(q: &str, now: Instant) -> FindState {
        let mut f = FindState::default();
        f.open();
        for c in q.chars() {
            f.key(BarKey::Text(c.to_string()), now);
        }
        f
    }

    #[test]
    fn short_queries_wait_a_second() {
        let t = Instant::now();
        let mut f = typed("ab", t);
        assert_eq!(f.query, "ab");
        assert_eq!(f.due(), Some(t + Duration::from_secs(1)));
        assert_eq!(f.take_due(t + Duration::from_millis(500)), None);
        assert_eq!(f.take_due(t + Duration::from_secs(1)), Some(true));
        assert!(!f.pending);
        let mut long = typed("abc", t);
        assert_eq!(long.take_due(t), Some(true));
        long.key(BarKey::Backspace, t);
        assert_eq!(long.query, "ab");
        assert_eq!(long.due(), Some(t + Duration::from_secs(1)));
        // An empty query clears the matches at once.
        let mut empty = typed("a", t);
        empty.key(BarKey::Backspace, t);
        assert_eq!(empty.due(), Some(t));
    }

    #[test]
    fn output_searches_again_without_moving_the_current_match() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        f.take_due(t);
        f.output_changed(t);
        assert_eq!(f.due(), Some(t + OUTPUT_DELAY));
        assert_eq!(f.take_due(t + OUTPUT_DELAY), Some(false));
        let mut none = FindState::default();
        none.output_changed(t);
        assert_eq!(none.due(), None);
    }

    #[test]
    fn escape_closes_and_clears() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        f.results.matches.push(crate::find::Match { start: Default::default(), end: Default::default() });
        f.results.current = Some(0);
        assert_eq!(f.key(BarKey::Close, t), Outcome::Close);
        assert!(!f.open);
        assert!(f.results.matches.is_empty());
        assert_eq!(f.results.current, None);
        assert!(!f.pending);
        assert_eq!(f.query, "abc");
    }

    #[test]
    fn return_goes_up_and_shift_return_down() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        assert_eq!(f.key(BarKey::Next, t), Outcome::Step { up: true });
        assert_eq!(f.key(BarKey::Previous, t), Outcome::Step { up: false });
        assert_eq!(f.key(BarKey::Grow, t), Outcome::Grow);
        assert_eq!(f.key(BarKey::PasteMatch, t), Outcome::PasteMatch);
        assert_eq!(f.key(BarKey::Text("d".into()), t), Outcome::Nothing);
        assert_eq!(f.query, "abcd");
    }
}
