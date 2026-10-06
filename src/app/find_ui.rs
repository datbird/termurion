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

/// The longest query the shared find query takes (`iTermFindPasteboard.m:43-47`).
pub(super) const BOARD_MAX: usize = 10 * 1024;

/// `FindDelaySeconds`: how long iTerm2 waits before searching a query of two characters or
/// fewer. Clamped to 0..=10 and NaN reads as the default, so a hand-edited value cannot panic
/// `Duration::from_secs_f64` (P6).
pub(super) fn find_delay(g: &crate::globals::Globals) -> Duration {
    let secs = g.number("FindDelaySeconds").filter(|v| !v.is_nan()).unwrap_or(1.0);
    Duration::from_secs_f64(secs.clamp(0.0, 10.0))
}

/// Whether Enter, with or without Shift, searches up: with `SwapFindNextPrevious` on Enter goes
/// up and Shift+Enter down; with it off the reverse (`iTermFindDriver.m:258-277`).
pub(super) fn step_direction(enter_shift: bool, swap: bool) -> bool {
    enter_shift != swap
}

/// Sets the shared query. `always` is Use Selection for Find and auto search
/// (`setStringValueUnconditionally`); else only with `SynchronizeQueryWithFindPasteboard` on.
/// A string over `BOARD_MAX` characters is refused. True when the query was set.
pub(super) fn board_set(board: &mut String, sync: bool, text: &str, always: bool) -> bool {
    if (!sync && !always) || text.chars().count() > BOARD_MAX {
        return false;
    }
    *board = text.to_string();
    true
}

/// Loads the shared query into a bar that is not already showing it (`iTermFindDriver.m:440-455`):
/// only with the sync key on, and a quiet search follows (B5). True when the bar took it.
pub(super) fn board_load(board: &str, sync: bool, f: &mut FindState, now: Instant) -> bool {
    if !sync || board.is_empty() || f.query == board {
        return false;
    }
    f.set_query(board, now);
    f.quiet = true;
    true
}
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

/// What must happen before Find Next or Find Previous steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreStep {
    /// Nothing waits: step.
    Step,
    /// A new query: search it; that search picks the match, so do not step after.
    Fresh,
    /// New output: search again keeping the current match, then step.
    Refresh,
}

/// Where a key goes while the find bar has the keyboard.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum BarRoute {
    Key(BarKey),
    /// Paste the clipboard into the query.
    Paste,
    /// The bar keeps the key and does nothing with it.
    Swallow,
    /// The key goes on to key routing (Ctrl+Tab).
    Pass,
}

/// The find bar's handling of a key.
pub(super) fn route_key(key: &WKey, text: Option<&str>, mods: crate::mouse::Mods) -> BarRoute {
    let printable = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    match key {
        WKey::Named(NamedKey::Enter) if mods.alt => BarRoute::Key(BarKey::PasteMatch),
        WKey::Named(NamedKey::Enter) if mods.shift => BarRoute::Key(BarKey::Previous),
        WKey::Named(NamedKey::Enter) => BarRoute::Key(BarKey::Next),
        WKey::Named(NamedKey::Escape) => BarRoute::Key(BarKey::Close),
        WKey::Named(NamedKey::Backspace) => BarRoute::Key(BarKey::Backspace),
        // Ctrl+Tab still cycles tabs.
        WKey::Named(NamedKey::Tab) if mods.ctrl => BarRoute::Pass,
        WKey::Named(NamedKey::Tab) => BarRoute::Key(BarKey::Grow),
        WKey::Character(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case("v") => BarRoute::Paste,
        // Plain typing, and AltGr (which arrives as Ctrl+Alt) typing a character.
        _ if printable.is_some() && mods.ctrl == mods.alt => BarRoute::Key(BarKey::Text(printable.unwrap_or_default().to_string())),
        _ => BarRoute::Swallow,
    }
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
    /// The bar is Filter's: the query picks the lines the pane shows.
    pub filter: bool,
    /// The lines the Filter view shows.
    pub shown: Vec<String>,
    /// The pending search comes from the shared query, not the person's typing: it fills the
    /// results and highlights but selects nothing and scrolls nowhere (B5).
    pub quiet: bool,
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
        self.quiet = false;
        self.pending = true;
        self.fresh = true;
        self.changed_at = Some(now);
    }

    /// New output arrived: search again soon, keeping the current match.
    pub fn output_changed(&mut self, now: Instant) {
        if self.open && (!self.query.is_empty() || self.filter) && !self.pending {
            self.pending = true;
            self.fresh = false;
            self.changed_at = Some(now);
        }
    }

    pub fn before_step(&self) -> PreStep {
        match (self.pending, self.fresh) {
            (true, true) => PreStep::Fresh,
            (true, false) => PreStep::Refresh,
            (false, _) if self.results.matches.is_empty() => PreStep::Fresh,
            (false, _) => PreStep::Step,
        }
    }

    /// When the pending search runs, if one waits.
    pub fn due(&self, short_delay: Duration) -> Option<Instant> {
        if !self.pending {
            return None;
        }
        let at = self.changed_at?;
        Some(if !self.fresh {
            at + OUTPUT_DELAY
        } else if !self.query.is_empty() && self.query.chars().count() <= 2 {
            at + short_delay
        } else {
            at
        })
    }

    /// Takes the pending search if it is due now.
    pub fn take_due(&mut self, now: Instant, short_delay: Duration) -> Option<bool> {
        if self.due(short_delay).is_some_and(|d| now >= d) {
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
        self.filter = false;
        self.shown.clear();
        self.pending = false;
        self.error = None;
        self.results = Results::default();
    }
}

pub(super) fn beep() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
    }
}

/// Menu item ids of the find bar's mode menu.
const MODE_ID: &str = "tandem.findmode.";
/// The status bar Search Tool's "Erase Search History" item.
const ERASE_ID: &str = "tandem.finderase";

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
    reveal_as(pane, m, crate::seltext::Mode::Character);
}

/// `reveal`, the selection made in `mode`.
pub(super) fn reveal_as(pane: &mut crate::pane::Pane, m: &Match, mode: crate::seltext::Mode) {
    let (a, b) = (to_point(&pane.screen, m.start), to_point(&pane.screen, m.end));
    let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
    sel.update(b, Side::Right);
    pane.set_selection(Some(sel), mode);
    show_line(pane, a.line.0);
}

/// Select Matches (`TVA:2801-2823`): the selection becomes one Character sub per match, the
/// last the live one, with no copy. None with no matches, which changes nothing; else whether
/// the matches passed `MAX_SUBS` and the rest were left out.
pub(super) fn select_all_matches(screen: &mut crate::screen::Screen, matches: &[Match]) -> Option<bool> {
    if matches.is_empty() {
        return None;
    }
    let subs = matches
        .iter()
        .take(crate::seltext::MAX_SUBS + 1)
        .map(|m| crate::seltext::Sub { start: m.start, end: m.end, mode: crate::seltext::Mode::Character, window: None, connected: false })
        .collect();
    Some(screen.set_subs(subs))
}

pub(super) fn show_line(pane: &mut crate::pane::Pane, line: i32) {
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
    pub(super) fn typing_in_find(&self) -> Option<PaneId> {
        let id = self.focused_id()?;
        self.finds.get(&id).filter(|f| f.open && f.typing).map(|_| id)
    }

    /// A key while the find bar has the keyboard. Returns false for keys the bar leaves
    /// to the terminal (chords with Ctrl or Alt it has no use for).
    pub(super) fn find_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(id) = self.typing_in_find() else { return false };
        let key = match route_key(&ev.logical_key, ev.text.as_deref(), self.mods) {
            BarRoute::Key(k) => k,
            BarRoute::Paste => {
                self.paste_into_find(id);
                return true;
            }
            BarRoute::Swallow => return true,
            BarRoute::Pass => return false,
        };
        let now = Instant::now();
        let Some(f) = self.finds.get_mut(&id) else { return false };
        let was_filter = f.filter;
        let outcome = f.key(key, now);
        if outcome == Outcome::Nothing {
            self.share_query(id);
        }
        match outcome {
            Outcome::Nothing => {}
            Outcome::Close => {
                if was_filter {
                    self.end_view(id);
                }
            }
            // The bar reports Return as "up"; the swap key decides what that means.
            Outcome::Step { up } => {
                let up = step_direction(!up, self.globals.bool("SwapFindNextPrevious"));
                self.find_step(id, up);
            }
            Outcome::Grow => self.find_grow(id),
            Outcome::PasteMatch => {
                let text = self.panes.get(&id).and_then(|p| p.screen.selection_text());
                if let (Some(t), Some(p)) = (text, self.panes.get_mut(&id)) {
                    p.scroll(Scroll::Bottom);
                    self.user_paste(id, &t, crate::broadcast::Write::Paste);
                }
            }
        }
        self.redraw();
        true
    }

    /// Text the IME commits while the find bar has the keyboard: typed into the query, as
    /// `find_key` types a printable key.
    pub(super) fn find_type_text(&mut self, id: PaneId, text: &str) {
        if let Some(f) = self.finds.get_mut(&id) {
            f.key(BarKey::Text(text.to_string()), Instant::now());
        }
        self.share_query(id);
        self.redraw();
    }

    /// Puts bar `id`'s query on the shared query after the person edited it (not a Filter bar's).
    fn share_query(&mut self, id: PaneId) {
        let Some(q) = self.finds.get(&id).filter(|f| !f.filter).map(|f| f.query.clone()) else { return };
        self.set_find_pasteboard(&q, Some(id));
    }

    /// Sets the shared find query with `SynchronizeQueryWithFindPasteboard` on (`from` is the
    /// bar that was edited, which already shows it). The key window's focused bar loads it.
    pub(super) fn set_find_pasteboard(&mut self, text: &str, from: Option<PaneId>) {
        if board_set(&mut self.find_pasteboard, self.globals.bool("SynchronizeQueryWithFindPasteboard"), text, false) {
            self.after_board_set(from);
        }
    }

    /// `set_find_pasteboard` for Use Selection for Find and auto search: even with sync off.
    pub(super) fn set_find_pasteboard_always(&mut self, text: &str, from: Option<PaneId>) {
        if board_set(&mut self.find_pasteboard, self.globals.bool("SynchronizeQueryWithFindPasteboard"), text, true) {
            self.after_board_set(from);
        }
    }

    fn after_board_set(&mut self, from: Option<PaneId>) {
        if let Some(id) = self.focused_id().filter(|id| Some(*id) != from) {
            self.load_find_pasteboard(id);
        }
    }

    /// The focused bar in the key window takes the shared query when it differs
    /// (`iTermFindDriver.m:440-455`); its search runs quiet (B5). Other bars wait for focus.
    pub(super) fn load_find_pasteboard(&mut self, id: PaneId) {
        if !self.focused || self.focused_id() != Some(id) {
            return;
        }
        let sync = self.globals.bool("SynchronizeQueryWithFindPasteboard");
        if let Some(f) = self.finds.get_mut(&id).filter(|f| f.open && !f.filter) {
            board_load(&self.find_pasteboard, sync, f, Instant::now());
        }
    }

    /// A pane or window that gained the keyboard loads the shared query into its bar.
    fn follow_focus(&mut self) {
        let now = self.focused.then(|| self.focused_id()).flatten();
        if now != self.find_focus_seen {
            self.find_focus_seen = now;
            if let Some(id) = now {
                self.load_find_pasteboard(id);
            }
        }
    }

    /// Pastes the clipboard into the find query, on one line.
    fn paste_into_find(&mut self, id: PaneId) {
        let text = match self.clipboard.as_mut().map(|cb| cb.get_text()) {
            Some(Ok(t)) => t.replace(['\r', '\n'], " "),
            Some(Err(e)) => return self.banners.set("clipboard", format!("Paste failed: {e}")),
            None => return,
        };
        if let Some(f) = self.finds.get_mut(&id) {
            f.key(BarKey::Text(text), Instant::now());
        }
        self.share_query(id);
        self.redraw();
    }

    /// Runs the searches whose time has come. Returns when the next one is due.
    pub(super) fn run_due_searches(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let delay = find_delay(&self.globals);
        self.follow_focus();
        let due: Vec<(PaneId, bool, bool)> =
            self.finds.iter_mut().filter_map(|(id, f)| f.take_due(now, delay).map(|fresh| (*id, fresh, std::mem::take(&mut f.quiet)))).collect();
        for (id, fresh, quiet) in due {
            if self.finds.get(&id).is_some_and(|f| f.filter) {
                self.refilter(id, fresh);
            } else {
                self.search_as(id, fresh, quiet);
            }
        }
        self.finds.values().filter_map(|f| f.due(delay)).min()
    }

    /// Searches a pane for its query. A new query makes the nearest match at or above the
    /// last one (else the bottom of the view) current; new output keeps the current one.
    fn search(&mut self, id: PaneId, fresh: bool) {
        self.search_as(id, fresh, false);
    }

    /// `search`; `quiet` fills the results and highlights but selects nothing and scrolls nowhere
    /// (B5: a query that came from the shared query or a selection must not move that selection).
    fn search_as(&mut self, id: PaneId, fresh: bool, quiet: bool) {
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
                if let Some(m) = f.results.current.and_then(|i| f.results.matches.get(i)).filter(|_| !quiet) {
                    reveal(p, &m.clone());
                }
            }
            Ok(matches) => {
                f.error = None;
                // The selection moves with the text, so it finds the current match again
                // after lines drop off the top of a full scrollback.
                let term = p.screen.term();
                let anchor = f.results.current.and(term.selection.as_ref().and_then(|s| s.to_range(term))).map(|r| to_pos(&p.screen, r.start));
                f.results.update(matches, anchor);
            }
        }
        self.redraw();
    }

    /// Filter: the pane shows the lines of its own screen that match the query. An empty
    /// query shows the pane's own screen. New output adds its matching lines to the view,
    /// which keeps its scroll position and selection.
    fn refilter(&mut self, id: PaneId, fresh: bool) {
        let mode = self.find_mode();
        let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) else { return };
        // Make Screenshot's frozen copy is not filtered, nor a replay's frame.
        if p.frozen || p.replay.is_some() {
            return;
        }
        if f.query.is_empty() {
            f.error = None;
            f.shown.clear();
            p.end_view();
            return self.redraw();
        }
        match crate::synthetic::filter_lines(p.own(), &find::pattern(&f.query, mode)) {
            Err(e) => f.error = Some(e),
            Ok(lines) => {
                f.error = None;
                let tail = (!fresh && p.live.is_some()).then(|| crate::synthetic::appended(&f.shown, &lines)).flatten();
                match tail {
                    Some(tail) => {
                        let mut bytes = String::new();
                        for (i, l) in tail.iter().enumerate() {
                            if i > 0 || !f.shown.is_empty() {
                                bytes.push_str("\r\n");
                            }
                            bytes.push_str(l);
                        }
                        p.screen.feed(bytes.as_bytes());
                    }
                    None => {
                        let view = crate::synthetic::view(&lines, p.own());
                        p.show_view(view);
                    }
                }
                f.shown = lines;
            }
        }
        self.redraw();
    }

    /// Back to the pane's own screen from Filter or Zoom.
    pub(super) fn end_view(&mut self, id: PaneId) {
        // The screenshot's frozen copy stays until its panel closes, and a replay's frame
        // until the replay ends (`end_replay`).
        if self.shot.as_ref().is_some_and(|s| s.pane == id) || self.panes.get(&id).is_some_and(|p| p.replay.is_some()) {
            return;
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.end_view();
        }
        if let Some(f) = self.finds.get_mut(&id).filter(|f| f.filter) {
            f.close();
        }
        self.redraw();
    }

    /// Find Next (`up`) or Find Previous, with the pane's query, opening nothing.
    pub(super) fn find_step(&mut self, id: PaneId, up: bool) {
        let pre = self.finds.get(&id).map_or(PreStep::Fresh, FindState::before_step);
        if let Some(f) = self.finds.get_mut(&id) {
            f.pending = false;
        }
        match pre {
            PreStep::Step => {}
            PreStep::Refresh => self.search(id, false),
            PreStep::Fresh => {
                self.search(id, true);
                // The fresh search already made a match current.
                if self.finds.get(&id).is_some_and(|f| f.results.current.is_some()) {
                    return;
                }
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
        self.share_query(id);
        self.search(id, true);
    }

    /// The find bar's menu commands. Returns false for other selectors.
    pub(super) fn run_find_command(&mut self, selector: &str, label: &str) -> bool {
        // Win+V while the bar has the keyboard pastes into the query, as Cmd+V does in iTerm2.
        if let (Some(id), "paste:") = (self.typing_in_find(), selector) {
            self.paste_into_find(id);
            return true;
        }
        let Some(id) = self.focused_id() else { return true };
        let now = Instant::now();
        match (selector, label) {
            ("performFindPanelAction:", "Find...") => {
                if self.finds.get(&id).is_some_and(|f| f.filter) {
                    self.end_view(id);
                }
                self.belt_yield_keys();
                let f = self.finds.entry(id).or_default();
                f.open();
                f.typing = true;
                self.load_find_pasteboard(id);
                if let Some(f) = self.finds.get_mut(&id).filter(|f| !f.query.is_empty()) {
                    f.set_query(&f.query.clone(), now);
                }
            }
            ("performFindPanelAction:", "Use Selection for Find") => {
                let Some(text) = self.panes.get(&id).and_then(|p| p.screen.selection_text()) else { return true };
                let query = if matches!(self.find_mode(), find::Mode::RegexSensitive | find::Mode::RegexInsensitive) { regex::escape(&text) } else { text };
                let f = self.finds.entry(id).or_default();
                f.set_query(&query, now);
                // Use Selection for Find shares the query even with sync off.
                self.set_find_pasteboard_always(&query, Some(id));
            }
            ("findNext:", _) => self.find_step(id, step_direction(false, self.globals.bool("SwapFindNextPrevious"))),
            ("findPrevious:", _) => self.find_step(id, step_direction(true, self.globals.bool("SwapFindNextPrevious"))),
            ("clearFindString:", _) => {
                let f = self.finds.entry(id).or_default();
                f.set_query("", now);
                f.results = Results::default();
                self.share_query(id);
            }
            ("filter:", _) => {
                self.belt_yield_keys();
                let f = self.finds.entry(id).or_default();
                f.results = Results::default();
                f.open();
                f.typing = true;
                f.filter = true;
                f.set_query("", now);
            }
            ("zoomOnSelection:", _) => {
                let Some(p) = self.panes.get_mut(&id) else { return true };
                let term = p.screen.term();
                let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) else { return true };
                let (a, b) = (to_pos(&p.screen, r.start), to_pos(&p.screen, r.end));
                let lines = crate::synthetic::zoom_lines(&p.screen, a.row, b.row);
                let view = crate::synthetic::view(&lines, &p.screen);
                p.show_view(view);
            }
            ("zoomOut:", _) => self.end_view(id),
            ("performFindPanelAction:", "Select Matches") => self.select_matches(id),
            ("openSelection:", _) => {
                match self.panes.get(&id).and_then(|p| p.screen.selection_text()).filter(|t| !t.trim().is_empty()) {
                    Some(text) => self.open_text(id, text.trim()),
                    None => beep(),
                }
            }
            ("findUrls:", _) => self.reveal_shortcuts("urls"),
            ("smartSelectAllVisible:", _) => self.reveal_shortcuts("smart"),
            ("revealContentNavigationShortcuts:", _) => self.reveal_shortcuts("pick"),
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
        if let Some(id) = self.focused_id() {
            self.find_regex_in(id, regex);
        }
    }

    pub(super) fn find_regex_in(&mut self, id: PaneId, regex: &str) {
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

    /// The mode menu's items, with `title` for each mode; the find bar's and the status bar
    /// Search Tool's add a separator and "Erase Search History"
    /// (`iTermMiniSearchFieldViewController.xib`).
    fn find_mode_menu(&mut self, title: fn(find::Mode) -> &'static str, erase: bool) -> muda::Menu {
        let current = self.find_mode();
        let menu = muda::Menu::new();
        let mut results = Vec::new();
        for m in find::Mode::ALL {
            results.push(menu.append(&muda::CheckMenuItem::with_id(format!("{MODE_ID}{}", m as i64), title(m), true, m == current, None)));
        }
        if erase {
            results.push(menu.append(&muda::PredefinedMenuItem::separator()));
            results.push(menu.append(&muda::MenuItem::with_id(ERASE_ID, "Erase Search History", true, None)));
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Find mode menu: {e}"));
        }
        menu
    }

    /// A status bar field's magnifier: the mode menu at the field, with "Erase Search
    /// History" for the Search Tool (`search`) and not the Filter Tool.
    pub(super) fn show_bar_mode_menu(&mut self, search: bool, at: super::statusbar_content::MenuAt) {
        let menu = self.find_mode_menu(find::Mode::menu_title, search);
        self.show_menu_at(menu, at);
    }

    /// Opens the find bar's mode menu under the mode label.
    pub(super) fn show_find_mode_menu(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        // As the bar's fields: Search has "Erase Search History", Filter does not.
        let search = !self.focused_id().and_then(|id| self.finds.get(&id)).is_some_and(|f| f.filter);
        let menu = self.find_mode_menu(find::Mode::label, search);
        let at = self.find_mode_rect.map(|r| muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(r.x as i32, (r.y + r.h) as i32)));
        use muda::ContextMenu;
        unsafe { menu.show_context_menu_for_hwnd(hwnd, at) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// A pick from the mode menu. Returns false for other menu ids.
    pub(super) fn find_mode_picked(&mut self, id: &str) -> bool {
        // `eraseSearchHistory`: iTerm2 clears the search history and the find pasteboard
        // every session shares; Tandem's stand-in is each pane's kept query, so every
        // pane's goes. A pane filtering keeps its filter (iTerm2's filter is its own field).
        if id == ERASE_ID {
            let now = Instant::now();
            for f in self.finds.values_mut().filter(|f| !(f.open && f.filter)) {
                f.set_query("", now);
                f.results = Results::default();
            }
            // iTerm2 clears the find pasteboard with the history.
            self.find_pasteboard.clear();
            self.redraw_all();
            return true;
        }
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

impl App {
    /// A click in a status bar field's text (the bar's pane is already the focused one):
    /// it takes the keyboard. A Search Tool opens find as Find does when find is closed or
    /// Filter has the bar; a Filter Tool turns Filter on when it is off.
    pub(super) fn bar_field_focus(&mut self, pane: PaneId, kind: crate::statusbar_components::Kind) {
        use crate::statusbar_components::Kind;
        let filter = kind == Kind::FilterTool;
        match self.finds.get_mut(&pane) {
            Some(f) if f.open && f.filter == filter => {
                f.typing = true;
                self.belt_yield_keys();
            }
            _ if filter => {
                self.run_find_command("filter:", "");
            }
            _ => {
                self.run_find_command("performFindPanelAction:", "Find...");
            }
        }
        self.redraw();
    }

    /// A status bar field's close glyph: a temporary Search Tool closes find, as Esc does;
    /// the Filter Tool turns Filter off (`closeFilterComponent`).
    pub(super) fn bar_field_close(&mut self, pane: PaneId, kind: crate::statusbar_components::Kind) {
        use crate::statusbar_components::Kind;
        match kind {
            Kind::FilterTool => self.end_view(pane),
            _ => {
                if let Some(f) = self.finds.get_mut(&pane).filter(|f| !f.filter) {
                    f.close();
                }
            }
        }
        self.redraw();
    }

    /// Find Globally: the window, or its focus when it is open.
    pub(super) fn open_global_find(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.global_find {
            return w.focus();
        }
        let query = self.focused_id().and_then(|id| self.finds.get(&id)).map(|f| f.query.clone()).unwrap_or_default();
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::global_find_ui::GlobalFindWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &query) {
            Ok(w) => self.global_find = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Handles an event for the Find Globally window. False when `window` is not it.
    pub(super) fn global_find_event(&mut self, window: winit::window::WindowId, event: &winit::event::WindowEvent) -> bool {
        use crate::global_find_ui::{GlobalEvent, Hit, group, snippet};
        let Some(w) = self.global_find.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            winit::event::WindowEvent::CloseRequested => self.global_find = None,
            winit::event::WindowEvent::RedrawRequested => match w.redraw() {
                GlobalEvent::None => {}
                GlobalEvent::Close => self.global_find = None,
                GlobalEvent::Search(q) => {
                    let pattern = find::pattern(&q, self.find_mode());
                    let mut hits = Vec::new();
                    let mut error = None;
                    // Every window's sessions, window by window.
                    let windows = self.registry.order().len();
                    'panes: for (win, n, panes) in self.all_tab_panes() {
                        for id in panes {
                            if hits.len() >= crate::global_find_ui::MAX_HITS {
                                break 'panes;
                            }
                            let Some(p) = self.panes.get(&id) else { continue };
                            let name = p.own().title().unwrap_or("Shell");
                            let title = if windows > 1 { format!("Window {}, Tab {}: {name}", win + 1, n + 1) } else { format!("Tab {}: {name}", n + 1) };
                            match find::find_hits(&p.screen, &pattern) {
                                Ok(found) => hits.extend(found.into_iter().map(|(m, line, r)| {
                                    let (snippet, mark) = snippet(&line, r);
                                    Hit { pane: id, title: title.clone(), m, snippet, mark }
                                })),
                                Err(e) => error = Some(e),
                            }
                        }
                    }
                    if hits.len() >= crate::global_find_ui::MAX_HITS {
                        hits.truncate(crate::global_find_ui::MAX_HITS);
                        error = error.or(Some(format!("Showing the first {} matches.", crate::global_find_ui::MAX_HITS)));
                    }
                    if let Some(w) = self.global_find.as_mut() {
                        w.set_results(group(hits), error);
                    }
                }
                GlobalEvent::Pick(id, m) => {
                    self.enter_pane_window(id);
                    if let Some(w) = self.current_window() {
                        self.last_window = Some(w);
                    }
                    if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
                        self.select_tab(t);
                        self.tabs[t].focused = id;
                        self.tabs[t].zoomed = None;
                    }
                    if let Some(p) = self.panes.get_mut(&id) {
                        reveal(p, &m);
                    }
                    self.bring_forward();
                    self.relayout();
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Select Matches (`TVA:2801-2823`): one selection per match, none copied (iTerm2 does not),
    /// the first one scrolled into view.
    fn select_matches(&mut self, id: PaneId) {
        let (Some(f), Some(p)) = (self.finds.get(&id), self.panes.get_mut(&id)) else { return };
        let Some(first) = f.results.matches.first().copied() else { return };
        let Some(capped) = select_all_matches(&mut p.screen, &f.results.matches) else { return };
        show_line(p, to_point(&p.screen, first.start).line.0);
        if capped {
            self.banners.set("select-matches", format!("Selected the first {} matches", crate::seltext::MAX_SUBS));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Select Matches makes one selection per match and copies nothing (iTerm2 does not).
    #[test]
    fn select_matches_makes_selections() {
        let mut s = crate::screen::Screen::new(10, 5);
        s.feed(b"abcd\r\n\r\n\r\nxxcd");
        let m = |r1, c1, r2, c2| Match { start: Pos { row: r1, col: c1 }, end: Pos { row: r2, col: c2 } };
        // Nothing to select: the selection stays as it was.
        s.set_selection(None, crate::seltext::Mode::Character);
        assert_eq!(select_all_matches(&mut s, &[]), None);
        assert!(s.subs().is_empty());
        assert_eq!(select_all_matches(&mut s, &[m(0, 0, 0, 1), m(3, 2, 3, 3)]), Some(false));
        let sub = |r, c1, c2| crate::seltext::Sub { start: Pos { row: r, col: c1 }, end: Pos { row: r, col: c2 }, mode: crate::seltext::Mode::Character, window: None, connected: false };
        assert_eq!(s.subs(), vec![sub(0, 0, 1), sub(3, 2, 3)]);
        assert_eq!(s.selection_text().as_deref(), Some("ab\ncd"));
        // The command copies nothing: no clipboard in its body.
        let body = include_str!("find_ui.rs").split("fn select_matches(&mut self").nth(1).unwrap().split("\n    }\n").next().unwrap();
        assert!(!body.contains("clipboard") && !body.contains("set_text") && body.contains("select_all_matches"));
        // A match already on screen is not scrolled to (B5: select_matches reveals the first only).
        assert!(body.contains("show_line"));
        assert!(!body.contains("reveal("));
    }

    const ONE: Duration = Duration::from_secs(1);

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
        assert_eq!(f.due(ONE), Some(t + Duration::from_secs(1)));
        assert_eq!(f.take_due(t + Duration::from_millis(500), ONE), None);
        assert_eq!(f.take_due(t + Duration::from_secs(1), ONE), Some(true));
        assert!(!f.pending);
        let mut long = typed("abc", t);
        assert_eq!(long.take_due(t, ONE), Some(true));
        long.key(BarKey::Backspace, t);
        assert_eq!(long.query, "ab");
        assert_eq!(long.due(ONE), Some(t + Duration::from_secs(1)));
        // An empty query clears the matches at once.
        let mut empty = typed("a", t);
        empty.key(BarKey::Backspace, t);
        assert_eq!(empty.due(ONE), Some(t));
    }

    /// `FindDelaySeconds` is the wait for a query of two characters or fewer (Review Focus 5).
    #[test]
    fn find_delay_follows_the_key() {
        let t = Instant::now();
        let f = typed("ab", t);
        assert_eq!(f.due(Duration::from_secs_f64(1.0)), Some(t + Duration::from_secs(1)));
        assert_eq!(f.due(Duration::from_secs_f64(0.25)), Some(t + Duration::from_millis(250)));
        assert_eq!(typed("abc", t).due(Duration::from_secs_f64(1.0)), Some(t));
        // The key's value, clamped (P6): negative, NaN and huge values cannot panic.
        let mut g = crate::globals::Globals::empty(std::path::Path::new("find-test.toml"));
        assert_eq!(find_delay(&g), Duration::from_secs(1));
        g.set("FindDelaySeconds", toml::Value::Float(0.25));
        assert_eq!(find_delay(&g), Duration::from_millis(250));
        g.set("FindDelaySeconds", toml::Value::Float(-3.0));
        assert_eq!(find_delay(&g), Duration::ZERO);
        g.set("FindDelaySeconds", toml::Value::Float(f64::NAN));
        assert_eq!(find_delay(&g), Duration::from_secs(1));
        g.set("FindDelaySeconds", toml::Value::Float(1e300));
        assert_eq!(find_delay(&g), Duration::from_secs(10));
    }

    /// Enter searches up with the swap on, down with it off; Shift+Enter the reverse
    /// (`PseudoTerminal.m:15327-15341`).
    #[test]
    fn find_direction_follows_the_key() {
        assert!(step_direction(false, true));
        assert!(!step_direction(true, true));
        assert!(!step_direction(false, false));
        assert!(step_direction(true, false));
        // The Find Next menu item is the unshifted one, Find Previous the shifted one.
        let body = include_str!("find_ui.rs").split("(\"findNext:\", _) =>").nth(1).unwrap().split("(\"clearFindString:\"").next().unwrap();
        assert!(body.contains("step_direction(false,") && body.contains("step_direction(true,"));
    }

    /// One query in every bar with the sync key on; the bar that becomes the key window's focused
    /// one takes it; Use Selection for Find sets it with sync off too; a long string is refused
    /// (`iTermFindPasteboard.m:43-47`).
    #[test]
    fn shared_query_follows_iterm2() {
        let t = Instant::now();
        let mut board = String::new();
        let mut a = typed("abc", t);
        let mut b = typed("zzz", t);
        assert!(board_set(&mut board, true, &a.query, false));
        assert_eq!(board, "abc");
        assert!(board_load(&board, true, &mut b, t));
        assert_eq!(b.query, "abc");
        assert!(b.quiet && b.pending && b.fresh, "a quiet search follows");
        // Already showing it: nothing to do.
        assert!(!board_load(&board, true, &mut b, t));
        // The typing bar does not reload what it just set.
        assert!(!board_load(&board, true, &mut a, t));
        // Sync off: typing shares nothing and a bar keeps its own query.
        let mut c = typed("own", t);
        assert!(!board_set(&mut board, false, "def", false));
        assert_eq!(board, "abc");
        assert!(!board_load(&board, false, &mut c, t));
        assert_eq!(c.query, "own");
        // Use Selection for Find sets it even with sync off.
        assert!(board_set(&mut board, false, "sel", true));
        assert_eq!(board, "sel");
        // Over 10240 characters is refused, at the limit it is taken.
        let long = "x".repeat(BOARD_MAX + 1);
        assert!(!board_set(&mut board, true, &long, false));
        assert!(!board_set(&mut board, true, &long, true));
        assert_eq!(board, "sel");
        assert!(board_set(&mut board, true, &"x".repeat(BOARD_MAX), false));
        // An empty shared query never wipes a bar's own (nothing was shared yet).
        let mut d = typed("keep", t);
        assert!(!board_load("", true, &mut d, t));
        assert_eq!(d.query, "keep");
        // The person's own typing is not quiet.
        d.key(BarKey::Text("x".into()), t);
        assert!(!d.quiet);
    }

    #[test]
    fn output_searches_again_without_moving_the_current_match() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        f.take_due(t, ONE);
        f.output_changed(t);
        assert_eq!(f.due(ONE), Some(t + OUTPUT_DELAY));
        assert_eq!(f.take_due(t + OUTPUT_DELAY, ONE), Some(false));
        let mut none = FindState::default();
        none.output_changed(t);
        assert_eq!(none.due(ONE), None);
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

    #[test]
    fn a_closed_bar_does_not_search_on_output() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        f.take_due(t, ONE);
        f.key(BarKey::Close, t);
        f.output_changed(t);
        assert_eq!(f.due(ONE), None);
    }

    #[test]
    fn find_next_during_output_searches_then_steps() {
        let t = Instant::now();
        let mut f = typed("abc", t);
        assert_eq!(f.before_step(), PreStep::Fresh);
        f.take_due(t, ONE);
        f.results.matches.push(crate::find::Match { start: Default::default(), end: Default::default() });
        assert_eq!(f.before_step(), PreStep::Step);
        f.output_changed(t);
        assert_eq!(f.before_step(), PreStep::Refresh);
        let empty = FindState::default();
        assert_eq!(empty.before_step(), PreStep::Fresh);
    }

    #[test]
    fn ctrl_chords_stay_in_the_bar() {
        use crate::mouse::Mods;
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..none };
        let c = |s: &str| WKey::Character(s.into());
        assert_eq!(route_key(&c("c"), Some("\u{3}"), ctrl), BarRoute::Swallow);
        assert_eq!(route_key(&c("d"), Some("\u{4}"), ctrl), BarRoute::Swallow);
        assert_eq!(route_key(&c("v"), Some("\u{16}"), ctrl), BarRoute::Paste);
        assert_eq!(route_key(&WKey::Named(NamedKey::Tab), None, ctrl), BarRoute::Pass);
        assert_eq!(route_key(&WKey::Named(NamedKey::Tab), None, none), BarRoute::Key(BarKey::Grow));
        assert_eq!(route_key(&c("x"), Some("x"), none), BarRoute::Key(BarKey::Text("x".into())));
        let altgr = Mods { ctrl: true, alt: true, ..none };
        assert_eq!(route_key(&c("@"), Some("@"), altgr), BarRoute::Key(BarKey::Text("@".into())));
        assert_eq!(route_key(&WKey::Named(NamedKey::Enter), None, Mods { alt: true, ..none }), BarRoute::Key(BarKey::PasteMatch));
        assert_eq!(route_key(&WKey::Named(NamedKey::ArrowUp), None, none), BarRoute::Swallow);
        assert_eq!(route_key(&c("b"), Some("b"), Mods { alt: true, ..none }), BarRoute::Swallow);
    }
}
