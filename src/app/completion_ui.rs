//! Session > Open Autocomplete... and View > Auto Command Completion: a list at the cursor
//! the terminal keeps typing through (keys it knows go to it first).

use super::*;
use crate::autocomplete::{self, Model};
use crate::broadcast::Write;
use crate::completion::{Key as CKey, Kind, List, Reply, Row};

/// Rows looked at for Autocomplete (the rest of the scrollback is not searched).
const ROWS: usize = 5000;
const AUTO_FOOTER: &str = "Press Shift+Enter or Alt+Enter to send command.";

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The word characters Settings > General > Selection names (letters and digits count too).
fn word_chars_from(g: &Globals) -> String {
    g.string("WordCharacters")
}

impl App {
    /// The word characters of Settings > General > Selection (letters and digits count too).
    fn word_chars(&self, _pane: PaneId) -> String {
        word_chars_from(&self.globals)
    }

    /// Session > Open Autocomplete... (again while it shows: "more", as iTerm2).
    pub(super) fn open_autocomplete(&mut self) {
        let Some(pane) = self.focused_id() else { return };
        if let Some((p, list)) = self.completion.as_mut()
            && *p == pane && list.kind == Kind::Autocomplete {
                if list.key(CKey::Tab) == Reply::Search {
                    self.autocomplete_search(false);
                }
                return self.redraw();
            }
        let extra = self.word_chars(pane);
        let word = move |c: char| c.is_alphanumeric() || extra.contains(c);
        let Some((lines, row, col)) = self.panes.get(&pane).map(|p| p.own().char_rows(ROWS)) else { return };
        let q = autocomplete::query_at(&lines, row, col, &word);
        self.completion = Some((pane, List::new(Kind::Autocomplete, q, Vec::new())));
        self.belt_yield_keys();
        self.autocomplete_search(true);
    }

    /// The candidates for the list's query: the paste history, the screen, and (`first`:
    /// when the list opens, as iTerm2) the shell history.
    fn autocomplete_search(&mut self, first: bool) {
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
        if let Some(typed) = self.panes.get(&pane).and_then(|p| p.own().typed_command()).filter(|_| first) {
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

    /// Text the IME committed while a completion list is open: a typed character that reaches
    /// the terminal the way `completion_key`'s `Other` key does (the list follows the typing, or
    /// closes). The text itself is written by the caller.
    pub(super) fn completion_typed_text(&mut self) {
        let Some((pane, list)) = self.completion.as_mut() else { return };
        let pane = *pane;
        let reply = list.key(CKey::Other);
        if let Kind::Commands { typed } = &list.kind
            && !matches!(reply, Reply::Pass | Reply::Handled | Reply::Search) {
                let typed = typed.clone();
                self.auto.1.dismissed(&typed);
            }
        if matches!(reply, Reply::CloseAndPass) && self.focused_id() == Some(pane) {
            self.completion = None;
        }
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
        // A closed Auto Command Completion list stays closed until the typed text changes.
        if let Kind::Commands { typed } = &list.kind
            && !matches!(reply, Reply::Pass | Reply::Handled | Reply::Search) {
                let typed = typed.clone();
                self.auto.1.dismissed(&typed);
            }
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
            Reply::Navigate(path) => {
                self.completion = None;
                self.navigator_chosen(pane, &path);
                true
            }
            Reply::Search => {
                self.autocomplete_search(false);
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

    /// View > Auto Command Completion: the list of history commands starting with what is
    /// typed at the prompt (timing in `completion::Auto`). When the next check is due.
    pub(super) fn auto_command_tick(&mut self) -> Option<Instant> {
        use crate::completion::AutoDo;
        if !self.globals.bool("AutoCommandHistory") {
            return None;
        }
        let pane = self.focused_id()?;
        if self.auto.0 != Some(pane) {
            self.auto = (Some(pane), crate::completion::Auto::default());
        }
        let typed = self.panes.get(&pane).and_then(|p| p.own().typed_command()).map(|t| t.trim_start().to_string());
        let other = self.completion.as_ref().is_some_and(|(p, l)| *p == pane && l.kind == Kind::Autocomplete);
        let now = self.started.elapsed().as_secs_f64();
        match self.auto.1.step(typed.as_deref(), now, other) {
            AutoDo::Nothing => None,
            AutoDo::WakeAt(t) => Some(self.started + std::time::Duration::from_secs_f64(t)),
            AutoDo::Hide => {
                if self.completion.as_ref().is_some_and(|(_, l)| matches!(l.kind, Kind::Commands { .. })) {
                    self.completion = None;
                    self.redraw();
                }
                None
            }
            AutoDo::Show(typed) => {
                let host = self.history_host_pub(pane);
                let (unix, powers) = (unix_now(), self.powers_pub());
                let rows: Vec<Row> = self.shell_history_pub().commands(&host, &typed, unix, powers).into_iter().map(|(c, _)| Row { shown: String::new(), value: c }).collect();
                // No match, or only what is typed (ignoring case): nothing to offer.
                let any = !(rows.is_empty() || (rows.len() == 1 && rows[0].value.eq_ignore_ascii_case(&typed)));
                self.auto.1.showed(&typed, any);
                if any {
                    let selected = self.completion.as_ref().filter(|(p, _)| *p == pane).map_or(0, |(_, l)| l.selected);
                    let mut list = List::new(Kind::Commands { typed }, autocomplete::Query::default(), rows);
                    list.selected = selected.min(list.rows.len() - 1);
                    self.completion = Some((pane, list));
                } else if self.completion.as_ref().is_some_and(|(_, l)| matches!(l.kind, Kind::Commands { .. })) {
                    self.completion = None;
                }
                self.redraw();
                None
            }
        }
    }

    /// The list's rows and box in `pane`'s rectangle `rect` (the pane's whole rectangle; the
    /// text sits inside its inset): the rows, the selection, the footer, where the box goes and
    /// the bounds it is kept in. The draw and the hit rectangle both come from here.
    pub(super) fn completion_box(&self, pane: PaneId, rect: Rect, cell: (f32, f32)) -> Option<(Vec<String>, usize, Option<&'static str>, (f32, f32), Rect)> {
        let rect = self.text_rect(pane, rect);
        let (p, list) = self.completion.as_ref().filter(|(p, _)| *p == pane)?;
        let s = self.panes.get(p).map(|x| x.own())?;
        let cursor = s.term().grid().cursor.point;
        let at = (rect.x + cursor.column.0 as f32 * cell.0, rect.y + (cursor.line.0 as f32 + 1.0) * cell.1);
        let rows: Vec<String> = list.rows.iter().map(|r| format!("{}{}", r.shown, r.value)).collect();
        let footer = matches!(list.kind, Kind::Commands { .. }).then_some(AUTO_FOOTER);
        Some((rows, list.selected, footer, at, rect))
    }

    /// Draws the list at the cursor of its pane and returns the box it drew.
    pub(super) fn draw_completion(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) -> Option<Rect> {
        let (rows, selected, footer, at, bounds) = self.completion_box(pane, rect, cell)?;
        scene::completion_list(frame, &rows, selected, footer, at, bounds, cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocomplete_reads_word_characters() {
        let mut g = Globals::empty(std::path::Path::new("word-chars-test.toml"));
        assert_eq!(word_chars_from(&g), "/-+\\~_.");
        g.set("WordCharacters", "x".into());
        assert_eq!(word_chars_from(&g), "x");
    }
}
