//! Session > Open Autocomplete... and View > Auto Command Completion: a list at the cursor
//! the terminal keeps typing through (keys it knows go to it first).

use super::*;
use crate::autocomplete::{self, Model};
use crate::broadcast::Write;
use crate::completion::{Key as CKey, Kind, List, Reply, Row};

/// Rows looked at for Autocomplete (the rest of the scrollback is not searched).
const ROWS: usize = 5000;
/// Auto Command Completion waits this long after typing (iTerm2's 0.2 s).
const AUTO_DELAY: std::time::Duration = std::time::Duration::from_millis(200);
const AUTO_FOOTER: &str = "Press Shift+Enter or Alt+Enter to send command.";

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    /// The profile's word characters (letters and digits count too).
    fn word_chars(&self, pane: PaneId) -> String {
        self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map(|pr| pr.string("Characters considered part of a word for selection")).unwrap_or_else(|| "/-+\\~_.".into())
    }

    /// Session > Open Autocomplete... (again while it shows: "more", as iTerm2).
    pub(super) fn open_autocomplete(&mut self) {
        let Some(pane) = self.focused_id() else { return };
        if let Some((p, list)) = self.completion.as_mut() {
            if *p == pane && list.kind == Kind::Autocomplete {
                if list.key(CKey::Tab) == Reply::Search {
                    self.autocomplete_search();
                }
                return self.redraw();
            }
        }
        let extra = self.word_chars(pane);
        let word = move |c: char| c.is_alphanumeric() || extra.contains(c);
        let Some((lines, row, col)) = self.panes.get(&pane).map(|p| p.own().char_rows(ROWS)) else { return };
        let q = autocomplete::query_at(&lines, row, col, &word);
        self.completion = Some((pane, List::new(Kind::Autocomplete, q, Vec::new())));
        self.autocomplete_search();
    }

    /// The candidates for the list's query: the paste history, the screen, the shell history.
    fn autocomplete_search(&mut self) {
        let Some((pane, list)) = self.completion.clone() else { return };
        let extra = self.word_chars(pane);
        let word = move |c: char| c.is_alphanumeric() || extra.contains(c);
        let now = unix_now();
        let max = self.globals.int("AutocompleteMaxOptions").max(1) as usize;
        let mut m = Model::new(max);
        let pastes: Vec<(String, f64)> = self.paste_history_entries().into_iter().map(|(v, t)| (v, now - t)).collect();
        autocomplete::from_pastes(&pastes, &list.query, &word, &mut m);
        if let Some((lines, row, col)) = self.panes.get(&pane).map(|p| p.own().char_rows(ROWS)) {
            autocomplete::from_screen(&lines, row, col, &list.query, &word, &mut m);
        }
        if let Some(typed) = self.panes.get(&pane).and_then(|p| p.own().typed_command()) {
            let host = self.history_host_pub(pane);
            let cmds: Vec<(String, usize, f64)> = self.shell_history_pub().command_stats(&host).into_iter().map(|(c, u, t)| (c, u, now - t)).collect();
            autocomplete::from_commands(&cmds, typed.trim_start(), &list.query, &word, &mut m);
        }
        let rows: Vec<Row> = m.sorted().into_iter().map(|c| Row { shown: c.prefix, value: c.value }).collect();
        // Nothing found: the list closes (iTerm2's).
        if rows.is_empty() {
            self.completion = None;
        } else if let Some((_, l)) = self.completion.as_mut() {
            l.rows = rows;
            l.selected = 0;
        }
        self.redraw();
    }

    /// A key while the list shows. True when the list used it.
    pub(super) fn completion_key(&mut self, ev: &KeyEvent) -> bool {
        use winit::keyboard::{Key as WKey, NamedKey};
        let Some((pane, _)) = self.completion.as_ref() else { return false };
        let pane = *pane;
        if self.focused_id() != Some(pane) {
            self.completion = None;
            return false;
        }
        let (shift, alt) = (self.mods.shift, self.mods.alt);
        let key = match &ev.logical_key {
            WKey::Named(NamedKey::ArrowUp) => CKey::Up,
            WKey::Named(NamedKey::ArrowDown) => CKey::Down,
            WKey::Named(NamedKey::Enter) if shift => CKey::ShiftEnter,
            WKey::Named(NamedKey::Enter) if alt => CKey::AltEnter,
            WKey::Named(NamedKey::Enter) => CKey::Enter,
            WKey::Named(NamedKey::Tab) if shift => CKey::ShiftTab,
            WKey::Named(NamedKey::Tab) => CKey::Tab,
            WKey::Named(NamedKey::Escape) => CKey::Esc,
            // Modifier keys alone change nothing.
            WKey::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super) => return false,
            _ => CKey::Other,
        };
        let Some((_, list)) = self.completion.as_mut() else { return false };
        let reply = list.key(key);
        self.redraw();
        match reply {
            Reply::Insert(text) => {
                self.completion = None;
                self.user_write(pane, text.as_bytes(), Write::SendText);
                true
            }
            Reply::InsertAndRun(text) => {
                self.completion = None;
                self.user_write(pane, format!("{text}\r").as_bytes(), Write::SendText);
                true
            }
            Reply::Search => {
                self.autocomplete_search();
                true
            }
            Reply::Close => {
                self.completion = None;
                true
            }
            Reply::CloseAndPass => {
                self.completion = None;
                false
            }
            Reply::Pass => false,
            Reply::Handled => true,
        }
    }

    /// View > Auto Command Completion: after typing at the prompt stops for 0.2 s, the
    /// host's commands that start with it show (and follow it). When the next check is due.
    pub(super) fn auto_command_tick(&mut self) -> Option<Instant> {
        if !self.globals.bool("AutoCommandHistory") {
            return None;
        }
        let pane = self.focused_id()?;
        let typed = self.panes.get(&pane).and_then(|p| p.own().typed_command()).map(|t| t.trim_start().to_string());
        let showing = self.completion.as_ref().filter(|(p, l)| *p == pane && matches!(l.kind, Kind::Commands { .. })).map(|(_, l)| l.kind.clone());
        let Some(typed) = typed.filter(|t| !t.is_empty()) else {
            if showing.is_some() {
                self.completion = None;
                self.redraw();
            }
            self.auto_typed = None;
            return None;
        };
        let now = Instant::now();
        match &self.auto_typed {
            Some((p, t, _)) if *p == pane && *t == typed => {}
            _ => {
                self.auto_typed = Some((pane, typed.clone(), now + AUTO_DELAY));
            }
        }
        let due = self.auto_typed.as_ref().map(|(_, _, d)| *d)?;
        if showing == Some(Kind::Commands { typed: typed.clone() }) {
            return None;
        }
        if now < due {
            return Some(due);
        }
        let host = self.history_host_pub(pane);
        let (unix, powers) = (unix_now(), self.powers_pub());
        let rows: Vec<Row> = self.shell_history_pub().commands(&host, &typed, unix, powers).into_iter().map(|(c, _)| Row { shown: String::new(), value: c }).collect();
        // No match, or only what is typed: nothing to offer.
        if rows.is_empty() || (rows.len() == 1 && rows[0].value == typed) {
            if showing.is_some() {
                self.completion = None;
                self.redraw();
            }
            return None;
        }
        let selected = self.completion.as_ref().filter(|(p, _)| *p == pane).map_or(0, |(_, l)| l.selected);
        let mut list = List::new(Kind::Commands { typed }, autocomplete::Query::default(), rows);
        list.selected = selected.min(list.rows.len() - 1);
        self.completion = Some((pane, list));
        self.redraw();
        None
    }

    /// Draws the list at the cursor of its pane.
    pub(super) fn draw_completion(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) {
        let Some((p, list)) = self.completion.as_ref().filter(|(p, _)| *p == pane) else { return };
        let Some(s) = self.panes.get(p).map(|x| x.own()) else { return };
        let cursor = s.term().grid().cursor.point;
        let at = (rect.x + cursor.column.0 as f32 * cell.0, rect.y + (cursor.line.0 as f32 + 1.0) * cell.1);
        let rows: Vec<String> = list.rows.iter().map(|r| format!("{}{}", r.shown, r.value)).collect();
        let footer = matches!(list.kind, Kind::Commands { .. }).then_some(AUTO_FOOTER);
        scene::completion_list(frame, &rows, list.selected, footer, at, rect, cell);
    }
}
