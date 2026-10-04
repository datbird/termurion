//! Annotations in the window: the Marks and Annotations commands, Show Annotations, the
//! note window, and what the scene draws.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{show_line, to_pos};
use crate::find::{Match, Pos};
use crate::note_ui::{NoteEvent, NoteWindow};

impl App {
    /// The annotation commands. Returns false for other selectors.
    pub(super) fn run_note_command(&mut self, selector: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "addNoteAtCursor:" => self.note_at_cursor(id, event_loop),
            "annotateSelection:" => self.annotate_selection(id, event_loop),
            "nextAnnotation:" | "previousAnnotation:" => self.step_note(id, selector == "nextAnnotation:", event_loop),
            "editSessionNote:" => {
                if self.note_win.as_ref().is_some_and(|(_, p, n)| *p == id && n.is_none()) {
                    self.note_win = None;
                } else {
                    let text = self.panes.get(&id).map(|p| p.session_note.clone()).unwrap_or_default();
                    self.note_win = None;
                    let Some(g) = self.gpu.as_ref() else { return true };
                    match NoteWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Session Note", &text, false) {
                        Ok(w) => self.note_win = Some((w, id, None)),
                        Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                    }
                }
                self.sync_checks();
            }
            "showHideNotes:" => {
                let Some(p) = self.panes.get_mut(&id) else { return true };
                let any_shown = p.own().notes.by_id.values().any(|n| !n.hidden);
                for n in p.own_mut().notes.by_id.values_mut() {
                    n.hidden = any_shown;
                }
            }
            _ => return false,
        }
        self.redraw();
        true
    }

    /// Add Annotation at Cursor: the Smart Selection match at the cursor, else just
    /// before or after it, else the whole line.
    fn note_at_cursor(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        let Some(p) = self.panes.get(&id) else { return };
        let cursor = p.screen.term().grid().cursor.point;
        let cols = p.screen.term().grid().columns();
        let mut range = None;
        for dc in [0i32, -1, 1] {
            let col = cursor.column.0 as i32 + dc;
            if col < 0 || col as usize >= cols {
                continue;
            }
            let point = Point::new(cursor.line, Column(col as usize));
            if self.smart_select(id, point, false).is_some() {
                range = self.panes.get(&id).and_then(|p| {
                    let term = p.screen.term();
                    term.selection.as_ref().and_then(|s| s.to_range(term)).map(|r| (r.start, r.end))
                });
                break;
            }
        }
        let (a, b) = range.unwrap_or((Point::new(cursor.line, Column(0)), Point::new(cursor.line, Column(cols - 1))));
        if let Some(p) = self.panes.get_mut(&id) {
            p.screen.term_mut().selection = None;
        }
        self.add_and_open_note(id, a, b, event_loop);
    }

    fn annotate_selection(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        let Some(p) = self.panes.get(&id) else { return };
        let term = p.screen.term();
        let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) else {
            return self.banners.set("action", "Select the text to annotate first.");
        };
        self.add_and_open_note(id, r.start, r.end, event_loop);
    }

    fn add_and_open_note(&mut self, id: PaneId, a: Point, b: Point, event_loop: &ActiveEventLoop) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let note = p.screen.add_note(a.line.0, a.column.0, b.line.0, b.column.0, "", false);
        if note != 0 {
            self.open_note(id, note, event_loop);
        }
    }

    /// Next or Previous Annotation: scrolls to it and opens it.
    fn step_note(&mut self, id: PaneId, forward: bool, event_loop: &ActiveEventLoop) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let rows: Vec<i32> = p.screen.notes_in_order().iter().map(|(l, _, _)| *l).collect();
        let grid = p.screen.term().grid();
        let from = self.note_pos.get(&id).copied().unwrap_or(if forward { -(grid.history_size() as i32) - 1 } else { grid.screen_lines() as i32 });
        let Some(line) = crate::annotations::next_note(&rows, from, forward) else { return };
        show_line(p, line);
        let note = p.screen.notes_in_order().into_iter().find(|(l, _, _)| *l == line).map(|(_, n, _)| n);
        self.note_pos.insert(id, line);
        if let Some(n) = note {
            self.open_note(id, n, event_loop);
        }
    }

    pub(super) fn open_note(&mut self, pane: PaneId, note: u32, event_loop: &ActiveEventLoop) {
        let text = self.panes.get(&pane).and_then(|p| p.screen.notes.by_id.get(&note)).map(|n| n.text.clone()).unwrap_or_default();
        self.note_win = None;
        let Some(g) = self.gpu.as_ref() else { return };
        match NoteWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Annotation", &text, true) {
            Ok(w) => self.note_win = Some((w, pane, Some(note))),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// A plain click on annotated text opens its note. Returns true when it did.
    pub(super) fn click_note(&mut self, id: PaneId, point: Point, event_loop: &ActiveEventLoop) -> bool {
        let Some(note) = self.note_cells(id).into_iter().find(|(m, _, _)| {
            let at = self.panes.get(&id).map(|p| to_pos(&p.screen, point));
            at.is_some_and(|at| m.start <= at && at <= m.end)
        }) else {
            return false;
        };
        self.open_note(id, note.1, event_loop);
        true
    }

    /// Each note's cells, id, and text on show.
    pub(super) fn note_cells(&self, id: PaneId) -> Vec<(Match, u32, Option<String>)> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        // Notes belong to the main screen; a full-screen app's screen covers them.
        if p.screen.term().mode().contains(TermMode::ALT_SCREEN) {
            return Vec::new();
        }
        let grid = p.screen.term().primary_grid();
        let hist = grid.history_size() as i32;
        p.screen
            .notes_in_order()
            .into_iter()
            .map(|(line, nid, n)| {
                let row = (line + hist) as usize;
                let m = Match { start: Pos { row, col: n.start_col }, end: Pos { row: row + n.more_rows, col: n.end_col } };
                (m, nid, (!n.hidden && !n.text.is_empty()).then(|| n.text.clone()))
            })
            .collect()
    }

    /// Handles an event for the note window. False when `window` is not it.
    pub(super) fn note_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane, note)) = self.note_win.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let (pane, note) = (*pane, *note);
        match event {
            WindowEvent::CloseRequested => {
                self.note_win = None;
                self.sync_checks();
            }
            WindowEvent::RedrawRequested => match w.redraw() {
                NoteEvent::None => {}
                NoteEvent::Close => self.note_win = None,
                NoteEvent::Changed(text) => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        match note {
                            Some(n) => {
                                if let Some(entry) = p.screen.notes.by_id.get_mut(&n) {
                                    entry.text = text;
                                }
                            }
                            None => p.session_note = text,
                        }
                    }
                    // The Notes tool's Session mode shows the same note, in every window.
                    if note.is_none() {
                        self.tools_changed();
                        self.redraw_all();
                    }
                    self.redraw();
                }
                // Delete asks "Really remove annotation?" unless its answer is remembered.
                NoteEvent::RequestDelete => match self.warning_preempt(&crate::note_ui::remove_spec()) {
                    crate::warnings::Preempt::Answer(0) => self.remove_annotation(pane, note),
                    _ => {
                        if let Some((w, _, _)) = self.note_win.as_mut() {
                            w.confirm_delete();
                        }
                    }
                },
                NoteEvent::Delete { suppress } => {
                    self.warning_answered(&crate::note_ui::remove_spec(), 0, suppress);
                    self.remove_annotation(pane, note);
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Removes the annotation `note` of `pane` (confirmed or answered) and closes its window.
    fn remove_annotation(&mut self, pane: PaneId, note: Option<u32>) {
        if let (Some(p), Some(n)) = (self.panes.get_mut(&pane), note) {
            p.screen.remove_note(n);
        }
        self.note_win = None;
        self.redraw();
    }
}
