//! Screen readers in the window (13v2 Task 10): the window thread's side of the UI Automation
//! provider (`uia_win`). While a client listens (`Host::listening`), the current window's visible
//! panes are turned into `PaneSnapshot`s and published, `access::Tracker` says what to announce,
//! and a client's Select and ScrollIntoView come back as `UserEvent::Uia`. Without a client
//! nothing is built, nothing is tracked and no timer is armed. App-wide state only (never in
//! `swap_live`): the hosts are keyed by window, the trackers by pane.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};

use super::*;
use crate::access::{self, Event, Signal};
use crate::pane::Pane;
use crate::screen::Screen;
use crate::uia_win::{Host, PaneSnapshot, Request, WindowSnapshot};

/// What the screen reader side keeps of one pane while a client listens.
#[derive(Default)]
pub(super) struct AccessPane {
    tracker: access::Tracker,
    /// What the last publish was built from (None: the pane was not in the last one).
    signal: Option<Signal>,
    secure: bool,
    /// Where the prompt ended when the pane reached a password prompt: (absolute line, column).
    input_start: Option<(i64, usize)>,
    snap: Option<PaneSnapshot>,
    /// The absolute line of the published text's row 0 (the offsets a client sends refer to it).
    first_abs: i64,
}

/// The least time between two snapshot builds: one frame, as iTerm2 refreshes accessibility on its
/// cadence timer and Windows Terminal raises UIA events once per painted frame.
pub(super) const BUILD_GAP: Duration = Duration::from_millis(16);

/// None when a snapshot may be built now, else when the held-back build may run (every pty wake
/// inside the gap folds into that one build).
pub(super) fn build_wait(last: Option<Instant>, now: Instant, gap: Duration) -> Option<Instant> {
    last.map(|l| l + gap).filter(|d| *d > now)
}

/// The screen a client reads: the pane's own, never a view standing in for it.
pub(super) fn access_screen(p: &Pane) -> &Screen {
    p.own()
}

/// A view (Instant Replay, Filter, Zoom, a frozen screenshot) is shown in place of the pane's own
/// screen: what a client holds is not what the view draws, so requests that act on it are refused.
pub(super) fn view_shown(p: &Pane) -> bool {
    p.live.is_some() || p.replay.is_some()
}

/// The current window's title, rectangle and shown panes at the last publish.
pub(super) type AccessWindow = (String, (i32, i32, i32, i32), Vec<PaneId>);

/// The rows a snapshot covers (`PTYTextView.m:7474-7477`): the last `lines` lines of history plus
/// screen, capped to what exists. `lines` reads as `access::clamp_lines` does (P16).
pub(super) fn snapshot_rows(lines: usize, history: usize, screen: usize) -> usize {
    access::clamp_lines(i64::try_from(lines).unwrap_or(i64::MAX)).min(history.saturating_add(screen))
}

/// A range of text offsets as the grid cells a selection takes, (row, column) to (row, column)
/// inclusive: an empty range is one cell, a range ending on a newline ends at that row's last cell.
pub(super) fn selection_cells(t: &access::Text, start: usize, end: usize) -> ((usize, usize), (usize, usize)) {
    let len = t.text.len();
    // The cell of an offset, the end of the text being the last row's end (never a row below).
    let cell = |o: usize| if o >= len && len > 0 { access::cell_of(t, len - 1) } else { access::cell_of(t, o) };
    let s = start.min(len);
    if end <= s || s >= len {
        let c = cell(s);
        return (c, c);
    }
    let last = end.min(len) - 1;
    let mut a = cell(s);
    let mut b = cell(last);
    if t.text[last] == b'\n' as u16 {
        b.1 = b.1.saturating_sub(1);
        if last == s {
            a = b;
        }
    }
    (a, b)
}

/// Where a text sits in the viewport: for text row 0 on grid line `top` (a line above the screen is
/// negative), `total` rows, a `screen` lines tall viewport scrolled `offset` lines into history,
/// the first text row on screen, the viewport row it sits on, and how many text rows show.
pub(super) fn viewport(top: i64, total: usize, screen: usize, offset: usize) -> (usize, usize, usize) {
    let shift = top.saturating_add(i64::try_from(offset).unwrap_or(i64::MAX));
    let first = usize::try_from(shift.saturating_neg().max(0)).unwrap_or(usize::MAX).min(total);
    let row = shift.saturating_add(first as i64).max(0);
    if row >= screen as i64 || first >= total {
        return (first, usize::try_from(row).unwrap_or(screen).min(screen), 0);
    }
    (first, row as usize, (total - first).min(screen - row as usize))
}

/// A selection (grid (line, column) ends, inclusive) as text offsets over a text whose row 0 is on
/// grid line `top`; the part outside the text is cut off, and None when none of it is inside.
pub(super) fn selection_offsets(t: &access::Text, top: i64, sel: ((i32, usize), (i32, usize))) -> Option<(usize, usize)> {
    let rows = t.breaks.len() as i64;
    let ((l1, c1), (l2, c2)) = sel;
    let (r1, r2) = (i64::from(l1) - top, i64::from(l2) - top);
    if r2 < 0 || r1 >= rows {
        return None;
    }
    let start = if r1 < 0 { 0 } else { access::offset_of(t, r1 as usize, c1) };
    let end = if r2 >= rows {
        t.text.len()
    } else {
        let o = access::offset_of(t, r2 as usize, c2);
        // Past a row's text the selection holds blanks, not the row's newline.
        if t.text.get(o) == Some(&(b'\n' as u16)) && o > start { o } else { (o + 1).min(t.text.len()) }
    };
    Some((start, end.max(start)))
}

/// The cursor row as the tracker reads it: (column, cell text) for each cell.
pub(super) fn cursor_line(rows: &access::Rows) -> Vec<(usize, String)> {
    rows.rows.get(rows.cursor.0).map(|(cells, _)| cells.iter().cloned().enumerate().collect()).unwrap_or_default()
}

/// Whether UI Automation is told the focused element is the terminal pane (B7): not while a child
/// web view holds the keyboard, and not for a browser pane (WebView2's own tree answers there). The
/// browser pane flag is always false until browser panes exist (13x Task 5).
pub(super) fn access_target<T>(focused: bool, web_held: bool, pane_is_browser: bool, target: T) -> Option<T> {
    (focused && !web_held && !pane_is_browser).then_some(target)
}

impl App {
    /// Puts the screen reader's host on the current window (after its GPU is up): it answers
    /// `WM_GETOBJECT` from then on and builds nothing until a client listens.
    pub(super) fn access_install_host(&mut self) {
        let (Some(key), Some(hwnd)) = (self.windows.current(), self.hwnd()) else { return };
        if self.uia_hosts.contains_key(&key) {
            return;
        }
        let proxy = self.proxy.clone();
        let pending = self.access_attach.clone();
        // Runs on UI Automation's threads: it only posts (an Attach is posted once until handled).
        let send = Box::new(move |r: Request| {
            if r == Request::Attach && pending.swap(true, Ordering::SeqCst) {
                return;
            }
            let _ = proxy.lock().unwrap_or_else(|e| e.into_inner()).send_event(UserEvent::Uia(r));
        });
        match Host::install(hwnd, send) {
            Ok(h) => {
                self.uia_hosts.insert(key, h);
            }
            Err(e) => self.banners.set("access", format!("Screen reader support is off for this window: {e}")),
        }
    }

    /// A window's host goes with it, before the window is destroyed.
    pub(super) fn access_forget_window(&mut self, key: u64) {
        self.uia_hosts.remove(&key);
        if self.access_focus_sent.is_some_and(|(k, _)| k == key) {
            self.access_focus_sent = None;
        }
        self.access_window = None;
    }

    /// The window was made again on another handle (13g): the old host goes, the new one comes,
    /// and everything is published afresh.
    pub(super) fn access_reinstall_host(&mut self, old_key: u64) {
        self.access_forget_window(old_key);
        self.access_install_host();
        for st in self.access_trackers.values_mut() {
            st.signal = None;
        }
    }

    /// `about_to_wait`: while a client listens, publishes what changed and says what to announce;
    /// otherwise it forgets everything and arms nothing.
    pub(super) fn access_tick(&mut self) {
        if !Host::listening() {
            self.access_trackers.clear();
            self.access_window = None;
            self.access_focus_sent = None;
            self.access_due = None;
            self.access_built = None;
            self.access_hold = None;
            return;
        }
        let now = Instant::now();
        // Every pty wake inside a frame folds into the next frame's build.
        self.access_hold = build_wait(self.access_built, now, BUILD_GAP);
        if self.access_hold.is_none() {
            self.access_update(now);
        }
        self.access_raise_focus();
        self.access_settle(now);
    }

    /// The window or pane focus changed: the focused pane is announced to clients.
    pub(super) fn access_focus(&mut self) {
        if !Host::listening() {
            return;
        }
        // A focus change is a user action: it is not held back.
        self.access_update(Instant::now());
        self.access_raise_focus();
    }

    fn access_raise_focus(&mut self) {
        let Some(key) = self.windows.current() else { return };
        // The keyboard may be in a web view: WebView2 raises its own UI Automation focus then (B7).
        let web_held = self.web_focus_held.is_some() && self.web_focus_held == self.current_window();
        let target = access_target(self.focused, web_held, false, ()).and_then(|()| self.tabs.get(self.active).map(|t| (key, t.focused)));
        if target == self.access_focus_sent {
            return;
        }
        self.access_focus_sent = target;
        if let (Some((_, pane)), Some(h)) = (target, self.uia_hosts.get(&key)) {
            h.raise_focus(pane);
        }
    }

    /// The current window as the provider sees it, from what was built at the last publish.
    pub(super) fn access_snapshot(&self) -> WindowSnapshot {
        let Some((title, rect, ids)) = &self.access_window else { return WindowSnapshot::default() };
        WindowSnapshot { title: title.clone(), rect: *rect, panes: ids.iter().filter_map(|id| self.access_trackers.get(id).and_then(|s| s.snap.clone())).collect() }
    }

    /// Builds the panes whose signal changed, publishes, and raises their events.
    fn access_update(&mut self, now: Instant) {
        let Some(key) = self.windows.current() else { return };
        if !self.uia_hosts.contains_key(&key) {
            return;
        }
        let lines = self.globals.int("NumberOfLinesForAccessibility");
        let Some((title, (wx, wy), size)) = self.gpu.as_ref().map(|g| (g.window.title(), g.window.inner_position().map_or((0, 0), |p| (p.x, p.y)), g.window.inner_size())) else { return };
        let tab = self.tabs.get(self.active);
        let shown: Vec<(PaneId, crate::layout::Rect)> = tab.map(|t| self.visible(t)).unwrap_or_default().into_iter().filter(|(id, _)| self.panes.contains_key(id)).collect();
        let tab_focus = tab.map(|t| t.focused);
        let mut signals: Vec<(PaneId, Signal)> = Vec::with_capacity(shown.len());
        for (id, r) in &shown {
            let p = &self.panes[id];
            let screen = access_screen(p);
            let term = screen.term();
            let sel = term.selection.as_ref().and_then(|s| s.to_range(term)).map(|r| ((r.start.line.0, r.start.column.0), (r.end.line.0, r.end.column.0)));
            let (cw, ch) = self.pane_cell(*id);
            let rect = (wx + r.x as i32, wy + r.y as i32, wx + (r.x + r.w) as i32, wy + (r.y + r.h) as i32);
            signals.push((*id, Signal { tick: screen.tick_digest(), sel, offset: term.grid().display_offset(), focused: self.focused && tab_focus == Some(*id), rect, cell: (cw.round() as u32, ch.round() as u32) }));
        }
        let last: HashMap<PaneId, Signal> = self.access_trackers.iter().filter_map(|(id, s)| s.signal.clone().map(|g| (*id, g))).collect();
        let mut changed = access::changed_panes(&last, &signals);
        for (id, _) in &signals {
            if self.access_trackers.get(id).is_some_and(|s| s.secure != self.panes[id].text_is_secure()) && !changed.contains(id) {
                changed.push(*id);
            }
        }
        let ids: Vec<PaneId> = shown.iter().map(|(id, _)| *id).collect();
        let window = (title, (wx, wy, wx + size.width as i32, wy + size.height as i32), ids.clone());
        if changed.is_empty() && self.access_window.as_ref() == Some(&window) {
            return;
        }
        let mut raised: Vec<(PaneId, Vec<Event>)> = Vec::new();
        for id in changed {
            let Some((_, r)) = shown.iter().find(|(i, _)| *i == id) else { continue };
            let sig = signals.iter().find(|(i, _)| *i == id).map(|(_, s)| s.clone()).unwrap_or_default();
            let mut st = self.access_trackers.remove(&id).unwrap_or_default();
            let events = self.access_build(id, *r, &sig, lines, (wx, wy), now, &mut st);
            self.access_trackers.insert(id, st);
            raised.push((id, events));
        }
        // Panes that are not shown keep no text.
        for (id, st) in self.access_trackers.iter_mut() {
            if !ids.contains(id) {
                st.snap = None;
                st.signal = None;
                st.tracker = access::Tracker::default();
            }
        }
        self.access_trackers.retain(|id, _| self.panes.contains_key(id));
        self.access_window = Some(window);
        self.access_built = Some(now);
        let snapshot = self.access_snapshot();
        if let Some(h) = self.uia_hosts.get(&key) {
            h.publish(snapshot);
            for (id, events) in &raised {
                for e in events {
                    h.raise(*id, e);
                }
            }
        }
    }

    /// One pane's snapshot, stored in `st`, and the events its change calls for.
    #[allow(clippy::too_many_arguments)]
    fn access_build(&self, id: PaneId, rect: crate::layout::Rect, sig: &Signal, lines: i64, win: (i32, i32), now: Instant, st: &mut AccessPane) -> Vec<Event> {
        let p = &self.panes[&id];
        let screen = access_screen(p);
        let grid = screen.term().grid();
        let rows = screen.access_rows(snapshot_rows(lines as usize, grid.history_size(), grid.screen_lines()));
        let cursor_abs = screen.cursor_abs();
        let base = cursor_abs - i64::from(grid.cursor.point.line.0);
        let secure = p.text_is_secure();
        let output_changed = st.signal.as_ref().is_none_or(|g| g.tick != sig.tick);
        let sel_changed = st.signal.as_ref().is_none_or(|g| g.sel != sig.sel);
        let secured;
        let (used, events) = if secure {
            // Recorded only on seeing the pane reach the prompt: a pane first seen at one exposes nothing.
            if !st.secure && st.signal.is_some() {
                st.input_start = Some((cursor_abs, rows.cursor.1));
            } else if !st.secure {
                st.input_start = None;
            }
            secured = access::secure_rows(&rows, st.input_start);
            (&secured, st.tracker.refresh_secure())
        } else {
            st.input_start = None;
            let mut ev = Vec::new();
            if output_changed {
                ev = st.tracker.refresh(now, cursor_abs, rows.cursor.1 + 1, &cursor_line(&rows));
            }
            if sel_changed && !ev.contains(&Event::SelectionChanged) {
                ev.push(Event::SelectionChanged);
            }
            (&rows, ev)
        };
        st.secure = secure;
        let text = access::build(used);
        let top = used.first_line - base;
        let cursor = access::offset_of(&text, used.cursor.0, used.cursor.1);
        let selection = if secure { None } else { sig.sel.and_then(|s| selection_offsets(&text, top, s)) }.unwrap_or((cursor, cursor));
        let (first, row0, count) = viewport(top, text.breaks.len(), grid.screen_lines(), sig.offset);
        let (cw, ch) = self.pane_cell(id);
        let tr = self.text_rect(id, rect);
        st.first_abs = used.first_line;
        st.snap = Some(PaneSnapshot {
            id,
            text,
            selection,
            cursor,
            rect: sig.rect,
            cell: (cw, ch),
            origin: (win.0 + tr.x as i32, win.1 + (tr.y + row0 as f32 * ch) as i32),
            first_visible_row: first,
            visible_rows: count,
            focused: sig.focused,
            secure,
        });
        st.signal = Some(sig.clone());
        events
    }

    /// Reads the burst of output that has stopped, and arms the next settle (the one timer).
    fn access_settle(&mut self, now: Instant) {
        let Some(key) = self.windows.current() else { return };
        let mut raised: Vec<(PaneId, Vec<Event>)> = Vec::new();
        for (id, st) in self.access_trackers.iter_mut() {
            if st.tracker.due().is_some_and(|d| d <= now) {
                if let Some(p) = self.panes.get(id) {
                    // A pane that turned secure inside the build hold says nothing: drop the burst.
                    if p.text_is_secure() {
                        st.tracker = access::Tracker::default();
                        continue;
                    }
                    let ev = st.tracker.settle(now, &|a| access_screen(p).abs_line_text(a), access_screen(p).cursor_abs());
                    raised.push((*id, ev));
                }
            }
        }
        if let Some(h) = self.uia_hosts.get(&key) {
            for (id, events) in &raised {
                for e in events {
                    h.raise(*id, e);
                }
            }
        }
        self.access_due = self.access_trackers.values().filter_map(|s| s.tracker.due()).chain(self.access_hold).min();
    }

    /// A client's request, posted from UI Automation's threads (`UserEvent::Uia`). The text it
    /// refers to is the last published one; a pane at a password prompt or gone refuses.
    pub(super) fn uia_request(&mut self, r: Request) {
        match r {
            Request::Attach => {
                self.access_attach.store(false, Ordering::SeqCst);
            }
            Request::Select { pane, start, end } => {
                let Some(st) = self.access_trackers.get(&pane) else { return };
                let Some(snap) = st.snap.as_ref().filter(|s| !s.secure) else { return };
                let Some(p) = self.panes.get_mut(&pane).filter(|p| !p.text_is_secure() && !view_shown(p)) else { return };
                if start >= end.min(snap.text.text.len()) {
                    p.clear_selection();
                } else {
                    let base = access_screen(p).cursor_abs() - i64::from(access_screen(p).term().grid().cursor.point.line.0);
                    let ((r1, c1), (r2, c2)) = selection_cells(&snap.text, start, end);
                    let line = |row: usize| Line((st.first_abs + row as i64 - base).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                    let mut sel = Selection::new(SelectionType::Simple, Point::new(line(r1), Column(c1)), Side::Left);
                    sel.update(Point::new(line(r2), Column(c2)), Side::Right);
                    p.set_selection(Some(sel), crate::seltext::Mode::Character);
                }
                self.redraw();
            }
            Request::ScrollIntoView { pane, offset } => {
                let Some(st) = self.access_trackers.get(&pane) else { return };
                let Some(snap) = st.snap.as_ref().filter(|s| !s.secure) else { return };
                let row = access::cell_of(&snap.text, offset).0.min(snap.text.breaks.len().saturating_sub(1));
                let Some(p) = self.panes.get_mut(&pane).filter(|p| !p.text_is_secure() && !view_shown(p)) else { return };
                let base = access_screen(p).cursor_abs() - i64::from(access_screen(p).term().grid().cursor.point.line.0);
                let line = (st.first_abs + row as i64 - base).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                super::find_ui::show_line(p, line);
                self.redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::access_target;

    /// Preflight B7: UI Automation is told the terminal pane has focus only when the window is
    /// focused, no web view holds the keyboard and the pane is not a browser pane.
    #[test]
    fn access_skips_focus_for_a_browser_pane() {
        assert_eq!(access_target(true, false, false, 7), Some(7));
        assert_eq!(access_target(false, false, false, 7), None);
        assert_eq!(access_target(true, true, false, 7), None, "a web view holds the keyboard");
        assert_eq!(access_target(true, false, true, 7), None, "WebView2 raises its own focus for a browser pane");
    }

    /// Final review 3 and 4: a settle skips a pane that turned secure, and a pane that leaves the
    /// shown set forgets its tracker.
    #[test]
    fn trackers_follow_secure_and_shown() {
        let src = include_str!("access_app.rs");
        let from = |n: &str| { let at = src.find(n).unwrap_or_else(|| panic!("{n}")); &src[at..at + src[at..].find("\n    }\n").unwrap()] };
        assert!(from("fn access_settle(").contains("text_is_secure()"), "a secure pane is never settled");
        assert!(from("// Panes that are not shown keep no text.").contains("st.tracker = "), "a hidden pane's tracker is reset");
    }

    use super::*;

    fn text_of(spec: &[(&str, bool)]) -> access::Text {
        let rows = access::Rows { rows: spec.iter().map(|(s, w)| (s.chars().map(|c| c.to_string()).collect(), *w)).collect(), cursor: (0, 0), first_line: 0 };
        access::build(&rows)
    }

    /// Review Focus 5: Select on a text range selects those cells.
    #[test]
    fn select_reaches_the_pane() {
        let t = text_of(&[("ab", false), ("cd", false)]);
        assert_eq!(String::from_utf16(&t.text).unwrap(), "ab\ncd\n");
        // "cd": offsets 3..5 are cells (1, 0) to (1, 1), the end inclusive.
        assert_eq!(selection_cells(&t, 3, 5), ((1, 0), (1, 1)));
        // An empty range is one cell.
        assert_eq!(selection_cells(&t, 4, 4), ((1, 1), (1, 1)));
        // A range ending on a newline ends at that row's last cell.
        assert_eq!(selection_cells(&t, 3, 6), ((1, 0), (1, 1)));
        assert_eq!(selection_cells(&t, 0, 3), ((0, 0), (0, 1)));
        // Across rows.
        assert_eq!(selection_cells(&t, 1, 5), ((0, 1), (1, 1)));
        // A range past the text clamps to the end.
        assert_eq!(selection_cells(&t, 0, 900).0, (0, 0));
    }

    /// A progress bar that wakes the loop every millisecond builds at most once a frame.
    #[test]
    fn builds_are_one_a_frame() {
        let t0 = Instant::now();
        let (mut last, mut builds) = (None, 0);
        for ms in 0..1000u64 {
            let now = t0 + Duration::from_millis(ms);
            if build_wait(last, now, BUILD_GAP).is_none() {
                builds += 1;
                last = Some(now);
            }
        }
        assert!(builds <= 1000 / 16 + 1, "{builds} builds in a second");
        assert!(builds >= 50, "{builds} builds: the pane still follows the output");
        // Held back, the wait names the instant it ends; at that instant it builds.
        let last = Some(t0);
        let due = build_wait(last, t0 + Duration::from_millis(5), BUILD_GAP).unwrap();
        assert_eq!(due, t0 + BUILD_GAP);
        assert_eq!(build_wait(last, due, BUILD_GAP), None);
        assert_eq!(build_wait(None, t0, BUILD_GAP), None);
    }

    /// P12: with a view shown (Filter and Zoom show their view this way, Instant Replay and a frozen
    /// screenshot too), the model, the digest, the cursor and the settle read the own screen.
    #[test]
    fn the_own_screen_is_read_while_a_view_shows() {
        let mut views: Vec<(&str, Box<dyn Fn(&mut Pane, Screen)>)> = Vec::new();
        views.push(("view", Box::new(|p, v| p.show_view(v))));
        views.push(("freeze", Box::new(|p, v| p.freeze(v))));
        for (name, show) in &views {
            let mut p = Pane::recording(1, 20, 4);
            p.screen.feed(b"live secret line\r\nlive 2");
            let (digest, cursor, at) = (p.screen.tick_digest(), p.screen.cursor_abs(), p.screen.abs_line_text(p.screen.cursor_abs()));
            let mut v = Screen::new(30, 6);
            for chunk in [&b"VIEW frame\r\n"[..], b"\r\n", b"\r\n", b"more view text", b"!"] {
                v.feed(chunk);
            }
            assert_ne!(v.tick_digest(), digest, "{name}: the two differ");
            show(&mut p, v);
            assert!(view_shown(&p), "{name}: a view shows");
            let s = access_screen(&p);
            let rows = s.access_rows(100);
            let text: String = rows.rows.iter().map(|(c, _)| c.concat()).collect::<Vec<_>>().join("\n");
            assert!(text.contains("live secret line") && !text.contains("VIEW"), "{name}: {text}");
            assert_eq!((s.tick_digest(), s.cursor_abs(), s.abs_line_text(s.cursor_abs())), (digest, cursor, at), "{name}");
        }
        let p = Pane::recording(1, 20, 4);
        assert!(!view_shown(&p), "no view");
    }

    /// P12 in the wiring: nothing here reads `p.screen` (the shown view), and Select and
    /// ScrollIntoView are refused while a view shows.
    #[test]
    fn requests_refuse_while_a_view_shows() {
        let src = std::fs::read_to_string(format!("{}/src/app/access_app.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("p.screen"), "the shown view is read");
        let req = code.split("fn uia_request(").nth(1).unwrap();
        let (select, scroll) = req.split_once("Request::ScrollIntoView").unwrap();
        assert!(select.contains("view_shown("), "Select");
        assert!(scroll.contains("view_shown("), "ScrollIntoView");
    }

    #[test]
    fn lines_follow_iterm2() {
        assert_eq!(snapshot_rows(1000, 50000, 40), 1000);
        assert_eq!(snapshot_rows(1000, 10, 40), 50);
        // The setting is clamped as everywhere else (P16).
        assert_eq!(snapshot_rows(0, 50000, 40), 1000);
        assert_eq!(snapshot_rows(usize::MAX, usize::MAX, usize::MAX), access::MAX_LINES);
    }

    /// A scrolled view shows other rows of the text, and a short text sits on its own rows.
    #[test]
    fn the_viewport_follows_the_scroll() {
        // 50 rows, the last 10 are the screen (top line -40), live: rows 40..50 show on rows 0..10.
        assert_eq!(viewport(-40, 50, 10, 0), (40, 0, 10));
        // Scrolled 5 lines up: rows 35..45.
        assert_eq!(viewport(-40, 50, 10, 5), (35, 0, 10));
        // Scrolled to the top of the text: row 0 at the top row.
        assert_eq!(viewport(-40, 50, 10, 40), (0, 0, 10));
        // A text of 4 rows on lines 6..10 of a 10 line screen (a small setting): rows on 6..10.
        assert_eq!(viewport(6, 4, 10, 0), (0, 6, 4));
        // Scrolled 3 lines up the text moves down 3 rows, one falls off the bottom.
        assert_eq!(viewport(6, 4, 10, 3), (0, 9, 1));
        // Scrolled so far that none show.
        assert_eq!(viewport(6, 4, 10, 8).2, 0);
    }

    #[test]
    fn selections_become_offsets() {
        let t = text_of(&[("ab", false), ("cd", false)]);
        // Text row 0 is grid line -1: line -1 col 1 to line 0 col 1 is "b\ncd".
        assert_eq!(selection_offsets(&t, -1, ((-1, 1), (0, 1))), Some((1, 5)));
        // Wholly above the text: nothing. Partly above: cut at the start.
        assert_eq!(selection_offsets(&t, -1, ((-5, 0), (-3, 4))), None);
        assert_eq!(selection_offsets(&t, -1, ((-5, 0), (-1, 0))), Some((0, 1)));
        // Partly below: cut at the end of the text.
        assert_eq!(selection_offsets(&t, -1, ((0, 0), (9, 0))), Some((3, t.text.len())));
    }

    #[test]
    fn the_cursor_line_has_its_cells() {
        let rows = access::Rows { rows: vec![(vec!["a".into()], false), (vec!["$".into(), " ".into(), "x".into()], false)], cursor: (1, 3), first_line: 0 };
        assert_eq!(cursor_line(&rows), vec![(0, "$".to_string()), (1, " ".to_string()), (2, "x".to_string())]);
    }

    /// The wiring that keeps a screen reader's costs at zero and its hosts in step with windows.
    #[test]
    fn the_wiring_is_in_place() {
        let read = |f: &str| std::fs::read_to_string(format!("{}/src/app/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let body = |src: &str, name: &str, end: &str| src.split(name).nth(1).and_then(|r| r.split(end).next()).unwrap_or_default().to_string();
        let me = read("access_app.rs");
        let tick = body(&me, "pub(super) fn access_tick(", "\n    }\n");
        assert!(tick.find("Host::listening()").is_some_and(|l| l < tick.find("self.access_update(").unwrap_or(usize::MAX)), "nothing is built before a client listens");
        assert!(tick.contains("build_wait(self.access_built"), "builds are one a frame");
        let m = read("mod.rs");
        assert!(body(&m, "fn about_to_wait(", "ControlFlow::Wait),").contains("self.access_tick()"));
        assert!(m.contains("self.access_due") && m.contains("UserEvent::Uia(r)"));
        assert!(body(&read("windows_ui.rs"), "fn forget_window(", "\n    }\n").contains("access_forget_window"));
        assert!(body(&read("transparency_ui.rs"), "fn recreate_window_for_alpha", "\n    }\n").contains("access_reinstall_host"));
        assert!(body(&read("windows_ui.rs"), "fn open_window_with(", "self.registry.open(").contains("access_install_host"));
        assert!(body(&m, "fn teardown_process()", "\n}\n").contains("uia_win::disconnect_all"));
    }

    /// The 13v1 merge (S8 b): a hidden resident window has no screen reader provider (going resident
    /// forgets the window, and with it the host); the wake gives it back, and the next pass publishes.
    #[test]
    fn a_resident_window_has_no_provider_and_a_wake_publishes() {
        let read = |f: &str| std::fs::read_to_string(format!("{}/src/app/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let body = |src: &str, name: &str, end: &str| src.split(name).nth(1).and_then(|r| r.split(end).next()).unwrap_or_default().to_string();
        let r = read("resident_ui.rs");
        let go = body(&r, "pub(super) fn go_resident(&mut self)", "\n    }\n");
        assert!(go.find("self.forget_window(k)").is_some_and(|f| f < go.find("self.resident = true").unwrap_or(0)), "the host goes before the window is hidden");
        assert!(body(&read("windows_ui.rs"), "fn forget_window(", "\n    }\n").contains("access_forget_window"));
        let leave = body(&r, "pub(super) fn leave_resident_for(", "\n    }\n");
        assert!(leave.find("self.resident = false").is_some_and(|f| f < leave.find("self.access_install_host()").unwrap_or(0)), "the woken window gets its host back");
        // Every wake path leaves residence through that one function (open_window_with's resident
        // branch and wake_in_place).
        assert!(body(&r, "fn wake_in_place(", "\n    }\n").contains("self.leave_resident_for(false)"));
        assert!(body(&read("windows_ui.rs"), "fn open_window_with(", "self.install_sizing()").contains("None => self.leave_resident_for(hotkey)"));
        // The publish needs a host: nothing is built for a window without one.
        let update = body(&read("access_app.rs"), "fn access_update(&mut self", "let lines");
        assert!(update.contains("if !self.uia_hosts.contains_key(&key)"));
    }

    /// The 13v1 merge (S13): the one process teardown (13u S6, 13v1's every exit and logoff) ends
    /// 13v2's update check, Dynamic Profiles watch and screen reader providers beside 13v1's handoff.
    #[test]
    fn the_one_teardown_ends_the_update_check_the_watch_and_the_providers() {
        let m = std::fs::read_to_string(format!("{}/src/app/mod.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(m.matches("fn teardown_process()").count(), 1, "one teardown");
        let t = m.split("pub(crate) fn teardown_process()").nth(1).and_then(|r| r.split("\n}\n").next()).unwrap();
        for call in ["handoff_win::begin_closing()", "update_fetch_win::cancel_running()", "dynamic_profiles_scan::stop_all()", "uia_win::disconnect_all()"] {
            assert!(t.contains(call), "the teardown calls {call}");
        }
    }

}
