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
    pub shown: Vec<crate::synthetic::Styled>,
    /// The pending search comes from the shared query, not the person's typing: it fills the
    /// results and highlights but selects nothing and scrolls nowhere (B5).
    pub quiet: bool,
    /// Filter reads a big buffer a chunk at a time: the run under way (P7).
    pub run: Option<crate::synthetic::Chunked>,
    /// The run's fraction read, for the bar's progress line; None when no run is under way.
    pub progress: Option<f32>,
    /// When the run reads its next chunk (`FAST_GAP` after the last).
    pub next_chunk: Option<Instant>,
    /// The run started for a new query, so its result replaces the view instead of extending it.
    pub run_fresh: bool,
    /// Where the last finished run stopped: output resumes there instead of rereading the buffer.
    pub tail: Option<crate::synthetic::Tail>,
    /// Lines the last finished run read (a work counter for tests).
    pub work: usize,
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
        self.run = None;
        self.progress = None;
        self.next_chunk = None;
        self.tail = None;
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

/// One search of `p` for `f`'s query (`search_as`'s body, apart from the redraw): a fresh one
/// replaces the results (quiet: no select, no scroll, no current match), a repeat updates them.
fn search_pane(p: &mut crate::pane::Pane, f: &mut FindState, mode: find::Mode, fresh: bool, quiet: bool) {
    // A selected command restricts the search to its rows, in the pane's own screen only (a Filter
    // or Zoom view has other rows).
    let region = if p.live.is_none() { p.search_region() } else { None };
    match find::find_all_in(&p.screen, &find::pattern(&f.query, mode), region) {
        Err(e) => {
            f.error = Some(e);
            f.results = Results::default();
        }
        Ok(matches) if fresh => {
            f.error = None;
            let from = f.results.current.and_then(|i| f.results.matches.get(i)).map_or_else(|| view_bottom(&p.screen), |m| m.start);
            fresh_results(p, &mut f.results, matches, from, quiet);
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
}

/// Whether the bar says "Searching only selected command": the selected command's region exists
/// and the search uses it (Find skips it in a Zoom view, which has other rows; Filter reads the
/// own screen and keeps it).
pub(super) fn find_restricted(region: bool, filter: bool, in_view: bool) -> bool {
    region && (filter || !in_view)
}

/// The results of a fresh search. A loud one (the person's own typing, Enter, Find...) makes the
/// nearest match at or above `from` current and selects and shows it; a quiet one (B5: the
/// shared query, auto search) keeps the matches and highlights only, so no match is current (the
/// bar shows a count, not "i/N", `iTermSearchFieldCell.m:299`) and the selection that caused the
/// search stays where it is.
fn fresh_results(pane: &mut crate::pane::Pane, results: &mut Results, matches: Vec<Match>, from: Pos, quiet: bool) {
    *results = Results { matches, current: None };
    if quiet {
        return;
    }
    results.start_at(from);
    if let Some(m) = results.current.and_then(|i| results.matches.get(i)).copied() {
        reveal(pane, &m);
    }
}

/// Filter's start (`refilter`'s body): an empty query shows the pane's own screen; a new query
/// starts a run; new output during a run starts nothing, since the run reads to the buffer's end at
/// each step, and output after a run resumes where it stopped (P7). The first chunk is read at
/// once, so a small buffer is done before this returns.
fn filter_start(p: &mut crate::pane::Pane, f: &mut FindState, mode: find::Mode, fresh: bool, now: Instant) {
    // Make Screenshot's frozen copy is not filtered, nor a replay's frame.
    if p.frozen || p.replay.is_some() {
        return;
    }
    if f.query.is_empty() {
        f.error = None;
        f.run = None;
        f.progress = None;
        f.tail = None;
        f.shown.clear();
        p.end_view();
        return;
    }
    // The run under way reads the new output itself (it reads to the buffer's end at each step).
    if f.run.is_some() && !fresh {
        return;
    }
    let region = p.search_region();
    let pattern = find::pattern(&f.query, mode);
    // Output after a finished run: read only the new end, and extend the view (a whole run only
    // for a new query, a command's rows, or a changed width).
    let resumed = (!fresh && region.is_none() && p.live.is_some()).then(|| f.tail.as_ref().and_then(|t| crate::synthetic::Chunked::resume(p.own(), &pattern, t))).flatten();
    let started = match resumed {
        Some(run) => Ok(run),
        None => crate::synthetic::Chunked::start_in(p.own(), &pattern, region),
    };
    match started {
        Err(e) => {
            f.error = Some(e);
            f.run = None;
            f.progress = None;
        }
        Ok(run) => {
            f.error = None;
            f.run = Some(run);
            f.run_fresh = fresh;
            f.tail = None;
            filter_step(p, f, now);
        }
    }
}

/// One chunk of Filter's run. At its end the lines go to the view: a repeat that only grew adds
/// its new lines to it (which keeps its scroll position and selection), anything else replaces it.
pub(super) fn filter_step(p: &mut crate::pane::Pane, f: &mut FindState, now: Instant) {
    let Some(run) = f.run.as_mut() else { return };
    let Some(lines) = run.step(p.own()) else {
        f.progress = Some(run.progress());
        f.next_chunk = Some(now + crate::cadence::FAST_GAP);
        return;
    };
    let (base, tail, read) = (run.base(), run.tail(), run.lines_read());
    f.run = None;
    f.progress = None;
    f.next_chunk = None;
    f.work = read;
    f.tail = tail;
    // The view's lines from `base` on are this run's to replace: a repeat that only grew adds its
    // new lines (which keeps the scroll position and selection), anything else replaces the view.
    let base = base.min(f.shown.len());
    let old = f.shown.split_off(base);
    let grown = (!f.run_fresh && p.live.is_some()).then(|| crate::synthetic::appended(&old, &lines)).flatten();
    let cap = {
        use alacritty_terminal::grid::Dimensions;
        let grid = p.own().term().grid();
        grid.history_size() + grid.screen_lines()
    };
    match grown {
        Some(tail) => {
            let mut bytes = String::new();
            for (i, l) in tail.iter().enumerate() {
                if i > 0 || base + old.len() > 0 {
                    bytes.push_str("\r\n");
                }
                bytes.push_str(&l.sgr);
            }
            p.screen.feed(bytes.as_bytes());
            f.shown.extend(lines);
            trim_shown(f, cap);
        }
        None => {
            f.shown.extend(lines);
            trim_shown(f, cap);
            let view = crate::synthetic::view(&f.shown, p.own());
            p.show_view(view);
        }
    }
}

/// Cuts Filter's lines from the front to `cap`, the own screen's line count (it cannot hold more
/// matches than that), and moves the resume point with them: memory stays bounded under steady output.
fn trim_shown(f: &mut FindState, cap: usize) {
    let cut = f.shown.len().saturating_sub(cap);
    if cut > 0 {
        f.shown.drain(..cut);
        f.tail = f.tail.take().map(|t| t.trimmed(cut));
    }
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
        // A browser pane: Return searches forward, Esc clears the page, Tab and Alt+Return do nothing.
        if self.is_browser_pane(id) {
            match super::browser_find_app::bar_act(outcome) {
                super::browser_find_app::BarAct::Nothing => {}
                super::browser_find_app::BarAct::Step { forward } => self.browser_find_step(id, forward),
                super::browser_find_app::BarAct::Close => self.browser_find_closed(id),
            }
            self.redraw();
            return true;
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
    pub(super) fn share_query(&mut self, id: PaneId) {
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
        // Filter's chunks: one per `FAST_GAP` while a run is under way, no wake when none is.
        let runs: Vec<PaneId> = self.finds.iter().filter(|(_, f)| f.run.is_some() && f.next_chunk.is_some_and(|t| now >= t)).map(|(id, _)| *id).collect();
        for id in runs {
            if let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) {
                filter_step(p, f, now);
            } else if let Some(f) = self.finds.get_mut(&id) {
                f.run = None;
                f.progress = None;
            }
            self.redraw();
        }
        self.finds.values().flat_map(|f| [f.due(delay), f.run.as_ref().and(f.next_chunk)]).flatten().min()
    }

    /// Searches a pane for its query. A new query makes the nearest match at or above the
    /// last one (else the bottom of the view) current; new output keeps the current one.
    fn search(&mut self, id: PaneId, fresh: bool) {
        self.search_as(id, fresh, false);
    }

    /// `search`; `quiet` fills the results and highlights but selects nothing and scrolls nowhere
    /// (B5: a query that came from the shared query or a selection must not move that selection).
    fn search_as(&mut self, id: PaneId, fresh: bool, quiet: bool) {
        // A browser pane's page is searched by the page's own engine; a fresh query is forced.
        if self.is_browser_pane(id) {
            return self.browser_find_search(id, fresh);
        }
        let mode = self.find_mode();
        let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) else { return };
        search_pane(p, f, mode, fresh, quiet);
        self.redraw();
    }

    /// Filter: the pane shows the lines of its own screen that match the query. An empty
    /// query shows the pane's own screen. New output adds its matching lines to the view,
    /// which keeps its scroll position and selection.
    fn refilter(&mut self, id: PaneId, fresh: bool) {
        let mode = self.find_mode();
        let (Some(f), Some(p)) = (self.finds.get_mut(&id), self.panes.get_mut(&id)) else { return };
        filter_start(p, f, mode, fresh, Instant::now());
        self.redraw();
    }

    /// The selected command changed (`updateSearchRange`): an open bar looks in the new rows. A
    /// search is quiet, so the selection the click made stays.
    pub(super) fn find_range_changed(&mut self, id: PaneId) {
        let Some(filter) = self.finds.get(&id).filter(|f| f.open).map(|f| f.filter) else { return };
        if filter {
            self.refilter(id, true);
        } else {
            self.search_as(id, true, true);
        }
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
        // The terminal's "up" is the browser's Find Previous.
        if self.is_browser_pane(id) {
            return self.browser_find_step(id, !up);
        }
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
        // The find family on a browser pane is the browser's (`browser_find::route`); the terminal's
        // arms below read a screen it does not have.
        if self.is_browser_pane(id) && crate::browser_find::is_find_family(selector) {
            return self.browser_find_command(id, selector, label);
        }
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
        if self.is_browser_pane(id) {
            return self.browser_find_regex(id, regex);
        }
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
        let mut was_open = false;
        match self.finds.get_mut(&pane) {
            Some(f) if f.open && f.filter == filter => {
                f.typing = true;
                was_open = true;
                self.belt_yield_keys();
            }
            _ if filter => {
                self.run_find_command("filter:", "");
            }
            _ => {
                self.run_find_command("performFindPanelAction:", "Find...");
            }
        }
        // On a browser pane the bar (now typing) takes the keyboard from the address field and the page.
        if was_open && self.is_browser_pane(pane) {
            self.browser_find_take_keyboard(pane);
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
                    if self.is_browser_pane(pane) {
                        self.browser_find_closed(pane);
                    }
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
        use crate::global_find_ui::{GlobalEvent, Hit, snippet};
        let Some(w) = self.global_find.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            winit::event::WindowEvent::CloseRequested => self.global_closed(),
            winit::event::WindowEvent::RedrawRequested => match w.redraw() {
                GlobalEvent::None => {}
                GlobalEvent::Close => self.global_closed(),
                GlobalEvent::Search(q) => {
                    let pattern = find::pattern(&q, self.find_mode());
                    let mut hits = Vec::new();
                    let mut error = None;
                    // Every window's sessions, window by window. A browser pane has no screen: its
                    // page answers below (`browser_global_start`).
                    let windows = self.registry.order().len();
                    'panes: for (win, n, panes) in self.all_tab_panes() {
                        for id in panes {
                            if hits.len() > crate::global_find_ui::MAX_HITS {
                                break 'panes;
                            }
                            let Some(p) = self.panes.get(&id) else { continue };
                            if p.browser_io().is_some() {
                                continue;
                            }
                            let name = p.own().title().unwrap_or("Shell");
                            let title = global_row_title(windows, win, n, name);
                            match find::find_hits(&p.screen, &pattern) {
                                Ok(found) => hits.extend(found.into_iter().map(|(m, line, r)| {
                                    let (snippet, mark) = snippet(&line, r);
                                    Hit { pane: id, title: title.clone(), m, snippet, mark, web: None }
                                })),
                                Err(e) => error = Some(e),
                            }
                        }
                    }
                    self.global_hits = hits;
                    self.global_web.clear();
                    self.global_error = error;
                    self.global_generation += 1;
                    self.global_show();
                    self.browser_global_start(&q);
                }
                GlobalEvent::PickWeb(id, identifier) => {
                    self.global_focus_pane(id);
                    self.browser_global_reveal(id, &identifier);
                    self.bring_forward();
                    self.relayout();
                }
                GlobalEvent::Pick(id, m) => {
                    self.global_focus_pane(id);
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

    /// The row title of pane `pane` in Find Globally, as the terminal rows have it.
    pub(super) fn global_title(&self, pane: PaneId) -> Option<String> {
        let windows = self.registry.order().len();
        let (win, n, _) = self.all_tab_panes().into_iter().find(|(_, _, panes)| panes.contains(&pane))?;
        Some(global_row_title(windows, win, n, self.panes.get(&pane)?.own().title().unwrap_or("Shell")))
    }

    /// Brings pane `id` to the front and focuses it, as a Find Globally pick does (the terminal's
    /// reveal and the browser's both follow).
    fn global_focus_pane(&mut self, id: PaneId) {
        self.enter_pane_window(id);
        if let Some(w) = self.current_window() {
            self.last_window = Some(w);
        }
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            self.select_tab(t);
            self.tabs[t].focused = id;
            self.tabs[t].zoomed = None;
        }
    }

    /// The Find Globally list: the terminal hits and the browser hits that have arrived, capped.
    pub(super) fn global_show(&mut self) {
        let (groups, cut) = crate::global_find_ui::merge(&self.global_hits, &self.global_web);
        let error = self.global_error.clone().or(cut);
        if let Some(w) = self.global_find.as_mut() {
            w.set_results(groups, error);
        }
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

/// A Find Globally row title: "Tab 2: name", with the window number when there are several.
fn global_row_title(windows: usize, win: usize, tab: usize, name: &str) -> String {
    if windows > 1 { format!("Window {}, Tab {}: {name}", win + 1, tab + 1) } else { format!("Tab {}: {name}", tab + 1) }
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

    /// A pane whose text has "abc" three times, with the second one selected.
    fn selected_pane() -> (crate::pane::Pane, Vec<Match>) {
        let mut p = crate::pane::Pane::recording(1, 20, 5);
        p.screen.feed(b"abc abc\r\nabc");
        let (a, b) = (Point::new(Line(0), Column(4)), Point::new(Line(0), Column(6)));
        let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
        sel.update(b, Side::Right);
        p.set_selection(Some(sel), crate::seltext::Mode::Character);
        let matches = find::find_all(&p.screen, "abc").unwrap();
        assert_eq!(matches.len(), 3);
        (p, matches)
    }

    /// Review Focus 8: three commands, "abc" on rows 1, 4 and 8; the second command's region is
    /// rows 3 to 6.
    fn commands_pane() -> crate::pane::Pane {
        let mut p = crate::pane::Pane::recording(1, 30, 12);
        p.screen.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07abc\r\nxx\r\n\x1b]133;D;0\x07");
        p.screen.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two\r\n\x1b]133;C\x07abc\r\nyy\r\nzz\r\n\x1b]133;D;0\x07");
        p.screen.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07three\r\n\x1b]133;C\x07abc");
        p
    }

    /// `PS:21819-21834`, `TV:3793-3814`: with a command selected the search, Select All and Filter
    /// work inside its rows; the bar says so; deselecting brings the rest back.
    #[test]
    fn restricted_find_follows_iterm2() {
        let mut p = commands_pane();
        let rows = |p: &crate::pane::Pane, region| find::find_all_in(&p.screen, "abc", region).unwrap().iter().map(|m| m.start.row).collect::<Vec<_>>();
        assert_eq!(rows(&p, None), [1, 4, 8]);
        let second = p.screen.prompt_at_or_before(4);
        assert!(second.is_some());
        p.selected_command = second;
        assert_eq!(p.search_region(), Some((3, 6)));
        // The bar finds only row 4.
        let mut f = typed("abc", Instant::now());
        search_pane(&mut p, &mut f, find::Mode::SmartCase, true, true);
        assert_eq!(f.results.matches.iter().map(|m| m.start.row).collect::<Vec<_>>(), [4]);
        assert_eq!(crate::cmdsel::NOTICE, "Searching only selected command");
        let m = include_str!("mod.rs");
        assert!(m.contains("notice: restricted.then_some(crate::cmdsel::NOTICE)"));
        // Select All takes rows 3 to 6.
        assert_eq!(crate::cmdsel::select_all_lines(p.search_region(), 0, 12), (3, 6));
        let menu = include_str!("menu.rs");
        assert!(menu.contains("crate::cmdsel::select_all_lines(region, "));
        // Filter shows only the region's matching lines.
        let region = p.search_region();
        assert_eq!(crate::synthetic::filter_lines_in(p.own(), "abc", region).unwrap().iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["abc"]);
        assert_eq!(crate::synthetic::filter_lines(p.own(), "abc").unwrap().len(), 3);
        // Deselecting restores all three.
        p.selected_command = None;
        assert_eq!(p.search_region(), None);
        search_pane(&mut p, &mut f, find::Mode::SmartCase, true, true);
        assert_eq!(f.results.matches.len(), 3);
    }

    /// A Filter or Zoom view has no marks: its search is not restricted, and the bar says nothing.
    #[test]
    fn a_view_is_never_restricted() {
        let mut p = commands_pane();
        p.selected_command = p.screen.prompt_at_or_before(4);
        let line = crate::synthetic::Styled { text: "abc".into(), sgr: "abc".into() };
        let view = crate::synthetic::view(&[line.clone(), line], p.own());
        p.show_view(view);
        assert_eq!(p.selected_region(), None);
        let mut f = typed("abc", Instant::now());
        search_pane(&mut p, &mut f, find::Mode::SmartCase, true, true);
        assert_eq!(f.results.matches.len(), 2, "the view's own lines, unrestricted");
        // Filter reads the own screen, so the region still applies to it.
        assert_eq!(p.search_region(), Some((3, 6)));
    }

    /// The alternate screen shows no command's rows: no restriction while it does; a vanished mark
    /// ends the selection.
    #[test]
    fn the_region_ends_with_the_alternate_screen_and_the_mark() {
        let mut p = commands_pane();
        p.selected_command = p.screen.prompt_at_or_before(4);
        assert!(p.selected_region().is_some());
        p.screen.feed(b"\x1b[?1049h");
        assert_eq!(p.selected_region(), None);
        assert!(p.selected_command.is_some(), "the selection waits for the main screen");
        p.screen.feed(b"\x1b[?1049l");
        assert_eq!(p.selected_region(), Some((3, 6)));
        p.selected_command = Some(9999);
        assert_eq!(p.selected_region(), None);
        assert_eq!(p.selected_command, None, "a mark that is gone ends it");
    }

    /// A line-style mark starts the search range one row higher (`PS:21826-21829`).
    #[test]
    fn a_line_style_mark_searches_one_row_higher() {
        let mut p = crate::pane::Pane::recording(1, 30, 12);
        p.screen.line_style_marks = true;
        p.screen.feed(b"x\r\n\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07abc");
        p.selected_command = p.screen.prompt_at_or_before(2);
        assert_eq!(p.selected_region(), Some((1, 11)));
        assert_eq!(p.search_region(), Some((0, 11)));
    }

    /// Final review minor 5: a Filter view has no marks, so the line-style row comes from the own
    /// screen's record, not the view's.
    #[test]
    fn a_line_style_region_survives_a_view() {
        let mut p = crate::pane::Pane::recording(1, 30, 12);
        p.screen.line_style_marks = true;
        p.screen.feed(b"x\r\n\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07abc");
        p.selected_command = p.screen.prompt_at_or_before(2);
        let line = crate::synthetic::Styled { text: "abc".into(), sgr: "abc".into() };
        let view = crate::synthetic::view(&[line], p.own());
        p.show_view(view);
        assert_eq!(p.search_region(), Some((0, 11)), "one row higher, as without the view");
    }

    /// Final review minor 6: the bar says it searches only the selected command when the search
    /// does: Find skips the region in a Zoom view, Filter reads the own screen so it keeps it.
    #[test]
    fn the_restricted_notice_matches_what_find_does() {
        assert!(find_restricted(true, false, false));
        assert!(!find_restricted(true, false, true), "a Find inside a view is not restricted");
        assert!(find_restricted(true, true, true), "Filter reads the own screen");
        assert!(!find_restricted(false, true, false));
        let m = include_str!("mod.rs");
        assert!(m.contains("find_ui::find_restricted("), "the bar asks the predicate");
    }

    fn selection_start(p: &crate::pane::Pane) -> Pos {
        p.screen.subs()[0].start
    }

    /// B5: a search the shared query or auto search caused fills the results and highlights but
    /// leaves the selection that triggered it, and no match is current; the person's own search
    /// selects the nearest match.
    #[test]
    fn auto_search_keeps_the_selection() {
        let (mut p, matches) = selected_pane();
        let before = selection_start(&p);
        let mut results = Results::default();
        let bottom = view_bottom(&p.screen);
        fresh_results(&mut p, &mut results, matches.clone(), bottom, true);
        assert_eq!(selection_start(&p), before, "the selection stays");
        assert_eq!(results.matches.len(), 3);
        assert_eq!(results.current, None);
        assert_eq!(results.label(true), "3");
        fresh_results(&mut p, &mut results, matches, bottom, false);
        assert_eq!(results.current, Some(2));
        assert_eq!(selection_start(&p), Pos { row: 1, col: 0 }, "a loud search selects the match");
    }

    /// B5 through the search itself (the flag `search_as` passes), not only `fresh_results`.
    #[test]
    fn search_pane_honors_the_quiet_flag() {
        let (mut p, _) = selected_pane();
        let before = selection_start(&p);
        let mut f = typed("abc", Instant::now());
        search_pane(&mut p, &mut f, find::Mode::SmartCase, true, true);
        assert_eq!(f.results.matches.len(), 3);
        assert_eq!(f.results.current, None, "a quiet search has no current match");
        assert_eq!(selection_start(&p), before, "and leaves the selection");
        search_pane(&mut p, &mut f, find::Mode::SmartCase, true, false);
        assert_eq!(f.results.current, Some(2));
        assert_eq!(selection_start(&p), Pos { row: 1, col: 0 }, "a loud search selects the match");
    }

    #[test]
    fn loaded_query_does_not_move_the_selection() {
        let (mut p, matches) = selected_pane();
        let before = selection_start(&p);
        let t = Instant::now();
        let mut bar = typed("zzz", t);
        assert!(board_load("abc", true, &mut bar, t));
        let quiet = std::mem::take(&mut bar.quiet);
        assert!(quiet, "a loaded query searches quietly");
        let bottom = view_bottom(&p.screen);
        fresh_results(&mut p, &mut bar.results, matches, bottom, quiet);
        assert_eq!(selection_start(&p), before);
        assert_eq!(bar.results.current, None);
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

    /// iTerm2's `setFilterProgress:`: the bar shows how much of a big filter is read, and nothing
    /// once it is done; new output during the run does not restart it (P7).
    #[test]
    fn filter_progress_shows() {
        let mut p = crate::pane::Pane::recording(1, 40, 24);
        p.screen = crate::screen::Screen::with_scrollback(40, 24, 50_100);
        let mut text = String::new();
        for i in 0..50_000 {
            text.push_str(&format!("line {i}{}\r\n", if i % 10_000 == 5 { " needle" } else { "" }));
        }
        p.screen.feed(text.as_bytes());
        let t = Instant::now();
        let mut f = typed("needle", t);
        filter_start(&mut p, &mut f, find::Mode::SmartCase, true, t);
        assert!(f.run.is_some(), "a 50 000 line buffer takes several chunks");
        let first = f.progress.expect("the bar shows the run");
        assert!((first - 0.4).abs() < 0.02, "{first}");
        assert!(p.live.is_none(), "nothing replaces the view before the run ends");
        // The wake is a chunk away; with no run there is none.
        assert_eq!(f.run.as_ref().and(f.next_chunk), Some(t + crate::cadence::FAST_GAP));
        // Output during the run does not restart it.
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        assert_eq!(f.progress, Some(first));
        filter_step(&mut p, &mut f, t);
        assert!((f.progress.unwrap() - 0.8).abs() < 0.02);
        filter_step(&mut p, &mut f, t);
        // Done: the lines show, with their colors' codes fed; no second pass starts.
        assert!(p.live.is_some());
        assert_eq!(f.shown.len(), 5);
        assert_eq!((f.run.is_none(), f.progress, f.next_chunk), (true, None, None));
        // A closed bar holds no run.
        filter_start(&mut p, &mut f, find::Mode::SmartCase, true, t);
        f.close();
        assert_eq!((f.run.is_none(), f.progress), (true, None));
    }

    /// Final review fix 3: Filter never rereads the buffer from the start for output. During a run
    /// the run itself reads to the buffer's end at each step, so output ticks start no second
    /// pass and the run still ends; after it, output starts a run that resumes where the last one
    /// stopped and reads only the new tail, and idle output leaves no run.
    #[test]
    fn filter_output_reads_only_the_tail() {
        let mut p = crate::pane::Pane::recording(1, 40, 24);
        p.screen = crate::screen::Screen::with_scrollback(40, 24, 50_100);
        let mut text = String::new();
        for i in 0..50_000 {
            text.push_str(&format!("line {i}{}\r\n", if i % 10_000 == 5 { " needle" } else { "" }));
        }
        p.screen.feed(text.as_bytes());
        let t = Instant::now();
        let mut f = typed("needle", t);
        filter_start(&mut p, &mut f, find::Mode::SmartCase, true, t);
        // Steady output during the run: every tick asks again, none starts a second pass.
        for n in 0..20 {
            p.screen.feed(format!("tick {n} needle\r\n").as_bytes());
            filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        }
        let mut steps = 0;
        while f.run.is_some() {
            filter_step(&mut p, &mut f, t);
            steps += 1;
            assert!(steps <= 4, "the run ends after its own chunks, output does not re-arm it");
        }
        assert_eq!(f.shown.len(), 25, "the 5 old needles and the 20 that arrived during the run");
        // Output after the run: a run over the tail only, not the 50 000 lines again.
        p.own_mut().feed(b"after one needle\r\nafter two\r\nafter three needle\r\n");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        let work = f.run.as_ref().map_or(0, |r| r.lines_read()) + f.work;
        assert!(work < 100, "{work} lines read for 3 new lines");
        while f.run.is_some() {
            filter_step(&mut p, &mut f, t);
        }
        assert_eq!(f.shown.len(), 27);
        assert_eq!(f.shown.last().map(|l| l.text.as_str()), Some("after three needle"));
        // Idle: nothing re-arms.
        assert_eq!((f.run.is_none(), f.progress, f.next_chunk), (true, None, None));
        // A line that is still being typed is read again whole once it ends.
        p.own_mut().feed(b"partial nee");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        assert_eq!(f.shown.len(), 27);
        p.own_mut().feed(b"dle done\r\n");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        assert_eq!(f.shown.len(), 28);
        assert_eq!(f.shown.last().map(|l| l.text.as_str()), Some("partial needle done"));
    }

    /// Final re-review: under steady output Filter's lines stay within the own screen's
    /// scrollback, and every pass (a partial line that changes its match included) reads only the tail.
    #[test]
    fn filter_shown_stays_within_the_scrollback() {
        use alacritty_terminal::grid::Dimensions;
        let mut p = crate::pane::Pane::recording(1, 40, 24);
        p.screen = crate::screen::Screen::with_scrollback(40, 24, 200);
        let t = Instant::now();
        let mut f = typed("needle", t);
        p.screen.feed(b"first needle\r\n");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, true, t);
        let cap = |p: &crate::pane::Pane| p.own().term().grid().history_size() + p.own().term().grid().screen_lines();
        let mut worst = 0;
        for i in 0..600 {
            p.own_mut().feed(format!("m {i} needle\r\n").as_bytes());
            filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
            while f.run.is_some() {
                filter_step(&mut p, &mut f, t);
            }
            worst = worst.max(f.work);
            assert!(f.shown.len() <= cap(&p), "{} shown lines at {i}, the scrollback holds {}", f.shown.len(), cap(&p));
        }
        assert!(worst < 10, "a pass read {worst} lines");
        assert_eq!(f.shown.last().map(|l| l.text.as_str()), Some("m 599 needle"));
        // A last line that grows and changes its match rebuilds from the kept lines only.
        p.own_mut().feed(b"part");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        p.own_mut().feed(b"ial needle\r\n");
        filter_start(&mut p, &mut f, find::Mode::SmartCase, false, t);
        while f.run.is_some() {
            filter_step(&mut p, &mut f, t);
        }
        assert!(f.shown.len() <= cap(&p));
        assert!(f.work < 10, "{} lines read", f.work);
        assert_eq!(f.shown.last().map(|l| l.text.as_str()), Some("partial needle"));
        assert_eq!(f.shown[f.shown.len() - 2].text, "m 599 needle");
    }

    /// The bar draws the run's fraction as a line in the selection color, and none when done.
    #[test]
    fn filter_progress_draws() {
        let pal = crate::palette::Palette::default();
        let bar = |progress| scene::FindBar { query: "q", count: "Filter", mode: "Smart Case", error: false, notice: None, progress, selection: pal.selection };
        let area = Rect { x: 0.0, y: 0.0, w: 600.0, h: 300.0 };
        let mut none = scene::Frame::default();
        scene::find_bar(&mut none, &bar(None), area, (10.0, 20.0));
        let mut half = scene::Frame::default();
        scene::find_bar(&mut half, &bar(Some(0.5)), area, (10.0, 20.0));
        assert_eq!(half.quads.len(), none.quads.len() + 1);
        let line = &half.quads[1];
        let box_ = &half.quads[0].rect;
        assert_eq!((line.rect.h, line.color), (2.0, scene::rgba(pal.selection, 1.0)));
        assert!((line.rect.w - box_.w * 0.5).abs() < 0.01);
        assert!((line.rect.y - (box_.y + box_.h - 2.0)).abs() < 0.01);
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
