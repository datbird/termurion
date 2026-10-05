//! Explain Output with AI, the App side (iTerm2's `PTYSession.m:21211-21312`, `PSS:185-316`,
//! `ChatClient.swift:59-83`, `:259-470`): the menu item, the hand-off to the chat window, the
//! explanation processor the chat engine runs over each message, the hidden annotations it
//! places, and the reveal. The rules themselves are `ai_explain`'s.
//!
//! Nothing here starts a runner: the request goes through 12c's chat turn, and the gate runs
//! right before it is published (ruling R12e-1).

use std::collections::HashMap;

use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use winit::event_loop::ActiveEventLoop;

use super::ai_app::KeyThen;
use super::chat_app::pane_for_ref;
use super::{App, PaneId};
use crate::ai_explain::{self as ex, ExplainStep, Request, Response, Rows, Source, Update};
use crate::ai_gate::{Alert, Then};
use crate::api_buffer::{facts, grid_line, Facts};
use crate::chat_gate::endpoint_of;
use crate::chat_message::{new_id, Content, Message, Participant};
use crate::pane::Pane;
use crate::pwm::Secret;

/// A terminal grid as `ai_explain` reads it.
struct GridRows<'a>(&'a Grid<Cell>);

impl Rows for GridRows<'_> {
    fn width(&self) -> usize {
        self.0.columns()
    }

    fn row(&self, line: i32) -> Option<(Vec<char>, bool)> {
        if line < -(self.0.history_size() as i32) || line >= self.0.screen_lines() as i32 {
            return None;
        }
        let row = &self.0[Line(line)];
        let cells = (0..self.0.columns())
            .map(|c| {
                let cell = &row[Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    '\0'
                } else {
                    cell.c
                }
            })
            .collect();
        Some((cells, row[Column(self.0.columns() - 1)].flags.contains(Flags::WRAPLINE)))
    }
}

/// The cells of an attached-content range (`iTermSubSelection initWithCompactString:`): start
/// and end as (x, absolute line), the end exclusive, as the first and last grid point
/// (inclusive). None once a line left the history, below the screen, or for an empty range.
pub(super) fn location_points(f: Facts, start: (usize, i64), end: (usize, i64)) -> Option<(Point, Point)> {
    let cols = usize::try_from(f.cols).ok().filter(|c| *c > 0)?;
    // An end at x 0 is the end of the line before it.
    let (end_x, end_line) = match end.0 {
        0 => (cols - 1, end.1 - 1),
        x => (x - 1, end.1),
    };
    let (first, last) = (grid_line(start.1, f)?, grid_line(end_line, f)?);
    if (end_line, end_x) < (start.1, start.0) {
        return None;
    }
    Some((Point::new(Line(first), Column(start.0.min(cols - 1))), Point::new(Line(last), Column(end_x.min(cols - 1)))))
}

/// The explanation processor (`ChatClient.swift:59-83`, `:298-437`) over one message, App-free:
/// `marginal` places an update's annotations and answers its markdown, None when the session is
/// gone (the message is then dropped). Every message that is not an agent's explanation
/// response, or an append or commit for one, passes through unchanged.
pub(super) fn process(message: Message, target: Option<&Message>, marginal: &mut dyn FnMut(&Response, Option<&Update>) -> Option<String>) -> Option<Message> {
    if message.author != Participant::Agent {
        return Some(message);
    }
    match &message.content {
        // The whole answer, or a stream's first message: the update goes, the markdown grows.
        Content::ExplanationResponse(v) => {
            let Some((response, update, markdown)) = ex::parse_response_content(v) else { return Some(message) };
            let added = marginal(&response, update.as_ref())?;
            Some(Message { content: Content::ExplanationResponse(ex::response_content(&response, None, &format!("{markdown}{added}"))), ..message })
        }
        Content::Append { text, uuid: _ } => {
            let Some((target, mut response, markdown)) = explanation_of(target) else { return Some(message) };
            let mut update = response.append(text, false);
            update.message_id = Some(target.unique_id.clone());
            let added = marginal(&response, Some(&update))?;
            Some(grown(message, &response, &update, &format!("{markdown}{added}")))
        }
        Content::Commit(_) => {
            let Some((target, response, markdown)) = explanation_of(target) else { return Some(message) };
            let update = Update::new(true, Some(target.unique_id.clone()));
            let added = marginal(&response, Some(&update))?;
            Some(grown(message, &response, &update, &format!("{markdown}{added}")))
        }
        _ => Some(message),
    }
}

/// The stored row an append or commit targets, when it holds an explanation response.
fn explanation_of(target: Option<&Message>) -> Option<(&Message, Response, String)> {
    let target = target?;
    let Content::ExplanationResponse(v) = &target.content else { return None };
    let (response, _, markdown) = ex::parse_response_content(v)?;
    Some((target, response, markdown))
}

/// The new message (a fresh id) that carries the grown response; its update names the row it
/// replaces.
fn grown(message: Message, response: &Response, update: &Update, markdown: &str) -> Message {
    Message { unique_id: new_id(), content: Content::ExplanationResponse(ex::response_content(response, Some(update), markdown)), ..message }
}

/// The marginal markdown of an update (`markdownForExplanationResponse`, `ChatClient.swift:437-470`),
/// placing each annotation that has cells as a hidden note on the request's pane
/// (`PSS:185-215`). None when that session is gone.
pub(super) fn marginal_in(panes: &mut HashMap<PaneId, Pane>, notes: &mut HashMap<String, (PaneId, u32)>, links_off: bool, response: &Response, update: Option<&Update>) -> Option<String> {
    let session = &response.request.context.session_id;
    let pane = pane_for_ref(panes, session).filter(|_| !links_off)?;
    let (markdown, placed) = marginal_on(panes.get_mut(&pane)?.own_mut(), response, update);
    notes.extend(placed.into_iter().map(|(id, note)| (id, (pane, note))));
    Some(markdown)
}

/// The same on one screen: the markdown and the (annotation id, note) of each note placed.
fn marginal_on(screen: &mut crate::screen::Screen, response: &Response, update: Option<&Update>) -> (String, Vec<(String, u32)>) {
    let session = &response.request.context.session_id;
    let value = update.cloned().unwrap_or_else(|| Update::full(response));
    let f = facts(screen);
    let width = usize::try_from(f.cols).unwrap_or(0);
    let base = response.request.context.base_offset;
    let (mut bullets, mut placed) = (Vec::new(), Vec::new());
    for a in &value.annotations {
        let Some(((start_col, start_y), (end_col, end_y))) = ex::cells(&response.request.original, a, width) else { continue };
        // A line lost from the history gets no note and no bullet (`PSS:193-196`).
        let (Some(first), Some(last)) = (grid_line(base + i64::from(start_y), f), grid_line(base + i64::from(end_y), f)) else { continue };
        // The cells' end is exclusive, a note's inclusive.
        let note = screen.add_note(first, start_col, last, end_col.saturating_sub(1), &a.note, true);
        if note == 0 {
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        bullets.push(ex::bullet(&ex::annotation_url(&id, session), &a.annotated_text, &a.note));
        placed.push((id, note));
    }
    (ex::markdown(&bullets, value.is_final, !response.annotations.is_empty(), value.main_response.as_deref()), placed)
}

/// What Explain reads from a pane: the source, the first and last cell (inclusive) of its text
/// and whether the output was cut.
struct Found {
    source: Source,
    start: Point,
    end: Point,
    truncated: bool,
}

/// Whether the screen has a selection, without building its text (`PTA:172-174`): the item
/// follows the selection, so this runs every pass of the event loop.
pub(super) fn has_selection(screen: &crate::screen::Screen) -> bool {
    screen.term().selection.as_ref().is_some_and(|s| !s.is_empty())
}

/// The enabled state to apply to the menu items now, or None when they already show it.
pub(super) fn explain_refresh(shown: Option<bool>, now: bool) -> Option<bool> {
    (shown != Some(now)).then_some(now)
}

impl App {
    /// Whether Explain Output is enabled: a selection or a command mark in the focused pane
    /// (`PTA:172-174`). The item is not tied to the gate.
    pub(super) fn can_explain(&self) -> bool {
        let Some(p) = self.focused_id().and_then(|id| self.panes.get(&id)) else { return false };
        let own = p.own();
        ex::can_explain_with(has_selection(own), own.last_command_mark().is_some())
    }

    /// The Explain item follows the selection as it changes (`PTA:172-174`): checked every pass
    /// of the event loop, cheaply, and the bars are touched only on a change.
    pub(super) fn explain_follows(&mut self) {
        let now = self.can_explain();
        let Some(on) = explain_refresh(self.explain_shown, now) else { return };
        self.explain_shown = Some(on);
        for bar in self.menu_bars_mut() {
            if let Some(item) = &bar.explain_ai {
                item.set_enabled(on);
            }
        }
    }

    /// The selection, else the last command's output (`PS:21211-21258`); None with neither.
    fn explain_found(&mut self, pane: PaneId) -> Option<Found> {
        let screen = self.panes.get_mut(&pane)?.own_mut();
        if let Some(text) = screen.selection_text().filter(|t| !t.is_empty()) {
            let term = screen.term();
            let range = term.selection.as_ref().and_then(|s| s.to_range(term))?;
            let marks = screen.marks_in_order();
            // The command mark a point follows: the nearest prompt mark above it (`screenMarkBefore`).
            let before = |p: Point| marks.iter().filter(|(line, m)| *line <= p.line.0 && m.kind == crate::marks::Kind::Prompt).map(|(_, m)| m.id).next_back();
            let ends = (before(range.start), before(range.end));
            return Some(Found { source: Source::Selection { text, marks: ends }, start: range.start, end: range.end, truncated: false });
        }
        let mark = screen.last_command_mark()?;
        let (id, command) = (mark.id, mark.command.clone());
        let (start, end) = screen.last_output_range()?;
        let (first, truncated) = ex::capped(i64::from(start.line.0), i64::from(end.line.0));
        let start = Point::new(Line(first as i32), start.column);
        let first_line = command.lines().next().unwrap_or("").to_string();
        Some(Found { source: Source::LastCommand { mark: id, first_line, full: command }, start, end, truncated })
    }

    /// The request for what `found` holds, read now from the pane's grid (`PSS:287-316`).
    fn explain_request(&mut self, pane: PaneId, found: &Found) -> Option<Request> {
        let session = self.session_ref(pane)?;
        let screen = self.panes.get_mut(&pane)?.own_mut();
        let marks = screen.marks_in_order();
        let full = |id: u32| marks.iter().find(|(_, m)| m.id == id).map(|(_, m)| m.command.clone()).filter(|c| !c.is_empty());
        let f = facts(screen);
        let term = screen.term();
        let grid = if matches!(found.source, Source::Selection { .. }) { term.grid() } else { term.primary_grid() };
        let end = (found.end.column.0 + 1, found.end.line.0);
        let original = ex::located(&GridRows(grid), (found.start.column.0, found.start.line.0), end);
        let abs = |line: i32| f.above + i64::from(line);
        let url = ex::selection_url(&session, (found.start.column.0, abs(found.start.line.0)), (end.0, abs(end.1)));
        Some(Request {
            command: ex::command_for(&found.source, &full),
            truncated: found.truncated,
            original,
            question: String::new(),
            subject: ex::subject(&found.source),
            url: Some(url),
            context: ex::Context { session_id: session, base_offset: f.above },
        })
    }

    /// Edit > Explain Output with AI (`PS:21302-21312`, `PSS:287-316`, `CCL:259-292`). The gate
    /// runs first, for the configured model's own endpoint (ruling R12e-1); a closed gate
    /// creates, publishes and asks for nothing.
    pub(super) fn explain_output(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.focused_id() else { return };
        if self.ai_explain_waiting.is_some() {
            return;
        }
        let Some(found) = self.explain_found(pane) else { return };
        let model = self.ai_model_now();
        let endpoint = model.as_ref().map(endpoint_of);
        let open = self.ai_check_endpoint(endpoint.as_ref(), false);
        if ex::explain_step(open, true, true) == ExplainStep::Stop {
            return;
        }
        let Some(request) = self.explain_request(pane, &found) else {
            if let Some(text) = super::chat_app::links_off_text(self.chat_links_off) {
                self.banners.set("chat_links", text.to_string());
            }
            return;
        };
        let title = ex::title(&found.source);
        // The window, the gate for the default model and the database (`chat_window_enter`; it
        // shows its own alert after a failed open).
        if !self.chat_window_enter(event_loop) {
            return;
        }
        let (chat_open, has_key) = (self.chat_db.is_some(), self.ai_has_key());
        match ex::explain_step(open, chat_open, has_key) {
            ExplainStep::Stop => {}
            ExplainStep::NoChat => self.ask_ai(Alert { heading: ex::NO_CHAT_HEADING.to_string(), text: ex::NO_CHAT_TEXT.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None }),
            ExplainStep::AskKey => {
                let found = self.ai_found();
                let vendor = crate::ai_models::effective_vendor(&self.globals, crate::ai_catalog::bundled(), &found);
                self.ai_explain_waiting = Some((request, title));
                self.ask_ai_key(vendor, endpoint.as_ref(), KeyThen::Explain);
            }
            ExplainStep::Send => self.explain_send(&request, &title),
        }
    }

    /// The key prompt's answer (`KeyThen::Explain`): none closes the chat window (`CCL:268-277`);
    /// a key sends the held request after the gate ran again, as it may have closed meanwhile.
    pub(super) fn explain_key_given(&mut self, key: Option<Secret>) {
        let held = self.ai_explain_waiting.take();
        let Some((request, title)) = held else { return };
        if key.is_none() {
            return self.close_chat_window();
        }
        self.explain_send(&request, &title);
    }

    /// Creates the chat and publishes the request (`CCL:259-292`), the gate having just been
    /// run again for the model's own endpoint (ruling R12e-1).
    fn explain_send(&mut self, request: &Request, title: &str) {
        let model = self.ai_model_now();
        let endpoint = model.as_ref().map(endpoint_of);
        if ex::explain_step(self.ai_check_endpoint(endpoint.as_ref(), false), true, true) == ExplainStep::Stop {
            return;
        }
        let guid = request.context.session_id.clone();
        let Some(chat) = self.chat_create(title, Some(&guid), None, "", vec![]) else { return };
        let message = Message::new(&chat, Participant::User, Content::ExplanationRequest(ex::request_content(request)), super::warnings_app::unix_now());
        if let Err(e) = self.chat_publish(&chat, message, false) {
            self.banners.set("chat_link", e);
        }
        self.show_chat(Some(chat));
    }

    /// An annotation link was clicked (`PSS:216-234`): scrolls to the note and opens it as Next
    /// Annotation does (Tandem has no highlight flash). Acts only for an id this app placed, on
    /// the pane the link's session names (ruling P9).
    pub(super) fn reveal_ai_annotation(&mut self, session: &str, id: &str, event_loop: &ActiveEventLoop) {
        let Some(&(pane, note)) = self.ai_annotations.get(id) else { return };
        if self.ref_pane(session) != Some(pane) {
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let Some(line) = p.own().notes_in_order().into_iter().find(|(_, n, _)| *n == note).map(|(l, _, _)| l) else { return };
        self.reveal_pane(pane);
        if let Some(p) = self.panes.get_mut(&pane) {
            super::find_ui::show_line(p, line);
        }
        self.open_note(pane, note, event_loop);
        self.redraw();
    }

    /// The attached-content link of an explanation request (`revealLocationFromURL:`,
    /// `iTermApplicationDelegate.m:2182-2210`): the session's pane comes forward, the range is
    /// selected and scrolled into view. Nothing for a session that is gone or a range whose
    /// lines left the history.
    pub(super) fn reveal_ai_location(&mut self, session: &str, start: (usize, i64), end: (usize, i64)) {
        let Some(pane) = self.ref_pane(session) else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let Some((a, b)) = location_points(facts(p.own()), start, end) else { return };
        let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
        sel.update(b, Side::Right);
        p.own_mut().term_mut().selection = Some(sel);
        self.reveal_pane(pane);
        if let Some(p) = self.panes.get_mut(&pane) {
            super::find_ui::show_line(p, a.line.0);
        }
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attached_range_selects_its_cells() {
        // 10 lines lost, 5 kept in the history, a 24 by 80 screen: absolute 10 is grid -5.
        let f = Facts { above: 15, history: 5, rows: 24, cols: 80 };
        let p = |line: i32, col: usize| Point::new(Line(line), Column(col));
        assert_eq!(location_points(f, (0, 15), (3, 16)), Some((p(0, 0), p(1, 2))), "the end is exclusive");
        assert_eq!(location_points(f, (2, 10), (0, 12)), Some((p(-5, 2), p(-4, 79))), "an end at x 0 ends the line before");
        assert_eq!(location_points(f, (0, 9), (3, 16)), None, "a line lost from the history");
        assert_eq!(location_points(f, (0, 15), (3, 39)), None, "below the screen");
        assert_eq!(location_points(f, (4, 15), (4, 15)), None, "empty");
        assert_eq!(location_points(f, (100, 15), (200, 15)), Some((p(0, 79), p(0, 79))), "columns kept on the line");
    }
    use crate::ai_explain::{Annotation, Context, Located};
    use crate::chat_message::Content;

    fn request() -> Request {
        Request {
            command: None,
            truncated: false,
            original: Located { string: "ab".into(), coords: vec![(0, 0), (1, 0)] },
            question: String::new(),
            subject: "s".into(),
            url: None,
            context: Context { session_id: "G".into(), base_offset: 0 },
        }
    }

    fn agent(content: Content) -> Message {
        Message::new("c", Participant::Agent, content, 1.0)
    }

    #[test]
    fn the_processor_clears_the_first_update_and_names_the_row_it_replaces() {
        let mut seen = Vec::new();
        let mut marginal = |r: &Response, u: Option<&Update>| {
            seen.push(u.map(|u| (u.is_final, u.message_id.clone())));
            Some(format!("<{}>", r.raw))
        };
        // The stream's first message: its update names its own id and is cleared.
        let first = agent(Content::ExplanationResponse(ex::response_content(&Response::with_text(request(), "", false), Some(&Update::new(false, Some("SELF".into()))), "")));
        let stored = process(first, None, &mut marginal).unwrap();
        let Content::ExplanationResponse(v) = &stored.content else { panic!() };
        let (_, update, markdown) = ex::parse_response_content(v).unwrap();
        assert_eq!((update, markdown.as_str()), (None, "<>"));

        // An append for it becomes a new message whose update names the stored row.
        let append = agent(Content::Append { text: "<response>hi".into(), uuid: stored.unique_id.clone() });
        let grown = process(append, Some(&stored), &mut marginal).unwrap();
        assert_ne!(grown.unique_id, stored.unique_id);
        let Content::ExplanationResponse(v) = &grown.content else { panic!() };
        let (response, update, markdown) = ex::parse_response_content(v).unwrap();
        assert_eq!(update.unwrap().message_id.as_deref(), Some(stored.unique_id.as_str()));
        assert_eq!(markdown, "<><<response>hi>");
        assert_eq!(response.raw, "<response>hi");

        // The commit is final and names it too.
        let row = Message { unique_id: stored.unique_id.clone(), ..grown };
        let committed = process(agent(Content::Commit(stored.unique_id.clone())), Some(&row), &mut marginal).unwrap();
        let Content::ExplanationResponse(v) = &committed.content else { panic!() };
        let update = ex::parse_response_content(v).unwrap().1.unwrap();
        assert!(update.is_final);
        assert_eq!(update.message_id.as_deref(), Some(stored.unique_id.as_str()));
        assert_eq!(seen[0], Some((false, Some("SELF".to_string()))));
    }

    #[test]
    fn the_processor_passes_everything_else_and_drops_a_gone_session() {
        let mut gone = |_: &Response, _: Option<&Update>| None;
        let plain = agent(Content::Markdown("hi".into()));
        assert_eq!(process(plain.clone(), None, &mut gone), Some(plain));
        let user = Message::new("c", Participant::User, Content::Append { text: "x".into(), uuid: "u".into() }, 1.0);
        assert_eq!(process(user.clone(), None, &mut gone), Some(user));
        // An append for a row that is not an explanation is the broker's.
        let text = agent(Content::Markdown("t".into()));
        let append = agent(Content::Append { text: "x".into(), uuid: text.unique_id.clone() });
        assert_eq!(process(append.clone(), Some(&text), &mut gone), Some(append));
        // A response whose session is gone is dropped.
        let whole = agent(Content::ExplanationResponse(ex::response_content(&Response::with_text(request(), "<response>m</response>", true), None, "")));
        assert_eq!(process(whole, None, &mut gone), None);
    }
    #[test]
    fn explain_follows_the_selection() {
        let mut screen = crate::screen::Screen::new(10, 3);
        screen.feed(b"hello");
        assert!(!has_selection(&screen));
        // A click with no drag selects nothing.
        screen.term_mut().selection = Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(1)), Side::Left));
        assert!(!has_selection(&screen));
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(3)), Side::Right);
        screen.term_mut().selection = Some(sel);
        assert!(has_selection(&screen));
        // The item is touched only when its state changes.
        assert_eq!(explain_refresh(None, false), Some(false));
        assert_eq!(explain_refresh(Some(false), true), Some(true));
        assert_eq!(explain_refresh(Some(true), true), None);
    }

    #[test]
    fn annotations_are_placed_hidden_on_the_cells_they_name() {
        let mut screen = crate::screen::Screen::new(10, 3);
        screen.feed(b"error: text busy");
        let original = ex::located(&GridRows(screen.term().grid()), (0, 0), (6, 1));
        assert_eq!(original.string, "error: text busy");
        let mut response = Response::with_text(Request { original, ..request() }, "", true);
        response.annotations.push(Annotation { line: 0, utf16_offset_in_line: 7, utf16_length: 4, note: "The binary is running.".into(), annotated_text: "text busy".into() });
        let (markdown, placed) = marginal_on(&mut screen, &response, None);
        assert_eq!(placed.len(), 1);
        let (id, note) = &placed[0];
        let url = ex::annotation_url(id, "G");
        assert_eq!(markdown, ex::markdown(&[ex::bullet(&url, "text busy", "The binary is running.")], true, true, None));
        // Hidden, from (7, 0) across the wrap to the inclusive (0, 1).
        let n = &screen.notes.by_id[note];
        assert_eq!((n.start_col, n.more_rows, n.end_col, n.hidden, n.text.as_str()), (7, 1, 0, true, "The binary is running."));
        // A line lost from the history gets no note and no bullet, and no hint line either.
        let mut lost = response.clone();
        lost.request.context.base_offset = -5;
        let (markdown, placed) = marginal_on(&mut screen, &lost, None);
        assert!(placed.is_empty());
        assert_eq!(markdown, ex::markdown(&[], true, true, None));
    }
    #[test]
    fn the_gate_runs_before_anything_is_created_or_published() {
        let production = include_str!("ai_explain_app.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let body = |name: &str| production.split(&format!("fn {name}(")).nth(1).unwrap().split("\n    }\n").next().unwrap().to_string();
        let first = |text: &str, needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} missing"));
        let output = body("explain_output");
        assert!(first(&output, "ai_check_endpoint") < first(&output, "chat_window_enter"));
        assert!(first(&output, "ai_check_endpoint") < first(&output, "explain_send"));
        let send = body("explain_send");
        assert!(first(&send, "ai_check_endpoint") < first(&send, "chat_create"));
        assert!(first(&send, "ai_check_endpoint") < first(&send, "chat_publish"));
    }
}
