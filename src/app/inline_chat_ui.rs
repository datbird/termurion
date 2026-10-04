//! The inline chat in the app: a pane's binding to a chat, View > Show Inline Chat, the panel
//! at the right of the pane (its toolbar, its divider, the Switch menu), its conversation and
//! its input, and the binding's place in arrangements (`PTYSession.swift:2554-2699`,
//! `ChatViewController.swift:3703-3803`). The conversation is drawn by the scene renderer from
//! the same models the window uses (`chat_convo_rows`, `chat_view::body_doc`, `chat_buttons`);
//! the input is Task 7's rules over the Notes tool's editor.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use winit::keyboard::{Key as WKey, NamedKey};

use super::chat_window_app::{apply_update, ChatView, PANEL_ATTACH_ID};
use super::*;
use crate::chat_broker::SubId;
use crate::chat_message::{Message, Participant};
use crate::chat_toolbar as tb;
use crate::chat_view::{self, Item, Style};
use crate::chat_window_ui::RowData;
use crate::inline_chat::{self as ic, Binding, Button as IcButton, Toggle};
use crate::layout::visible_rects;
use crate::palette::{rgb, Palette};
use crate::porthole::render::VisualLine;
use crate::text_edit::{self, EditKey, EditOut};

/// One conversation row laid out at the panel's text width: its lines, its size, and the copy
/// glyph of each code block (the block's index, where its first line starts, what Copy takes).
pub(super) struct RowLayout {
    lines: Vec<VisualLine>,
    h: f32,
    w: f32,
    copies: Vec<(usize, f32, String)>,
    /// Each button's title width in the prose font.
    titles: Vec<f32>,
}

/// What the last frame laid out for one panel: the geometry presses and drawing read.
pub(super) struct PanelView {
    panel: Rect,
    /// The conversation's visible area.
    area: Rect,
    boxes: Vec<ic::RowBox>,
    rows: Arc<Vec<RowData>>,
    laid: Vec<Option<Rc<RowLayout>>>,
    scroll: f32,
    content_h: f32,
    input: Rect,
    parts: ic::InputParts,
    strip: Option<Rect>,
    line_h: f32,
    text_scale: f32,
    /// The input's visible lines (x, y, text), the selection, the caret, and the index of the
    /// first visible line.
    lines: Vec<(f32, f32, String)>,
    selection: Vec<Rect>,
    caret: Option<Rect>,
    first_line: usize,
    typing: bool,
}

/// One pane's inline chat. App-wide, keyed by pane (as `clips`).
#[derive(Default)]
pub(super) struct InlineState {
    pub(super) binding: Binding,
    /// The conversation's scroll in pixels, and whether the person scrolled up: until then it
    /// stays at the bottom.
    pub(super) scroll: f32,
    pub(super) unpinned: bool,
    /// Whether the panel has the keyboard (a press in the input gives it, a press in the
    /// terminal takes it back).
    pub(super) keys: bool,
    /// The input: its draft (text and attachments) and the editor holding the text.
    pub(super) draft: crate::chat_input::Draft,
    pub(super) edit: crate::text_edit::TextEdit,
    pub(super) wrap: crate::text_edit::WrapCache,
    /// The broker subscription of the shown chat, held while the panel is shown.
    sub: Option<SubId>,
    /// The bound chat's messages and the rows made from them.
    pub(super) msgs: Vec<Message>,
    msgs_chat: Option<String>,
    dirty: bool,
    convo: Arc<Vec<RowData>>,
    buttons_at: Option<Instant>,
    orchestrating: bool,
    /// Messages whose buttons were pressed and are greyed until the app answers.
    pressed: HashSet<String>,
    /// The code block just copied: (message id, block, when).
    copied: Option<(String, usize, Instant)>,
    /// Rows laid out, kept while the width, the look and the content stay.
    laid: HashMap<(u64, u32, bool, bool), Rc<RowLayout>>,
    laid_key: (u32, bool),
    /// The first visible line of a long input.
    input_first: usize,
    view: Option<PanelView>,
    /// The toolbar button under the pointer.
    hover: Option<IcButton>,
    /// The bound chat's title as the toolbar shows it, read at most once a second.
    title: std::cell::RefCell<Option<(Instant, String)>>,
}

/// A rectangle of a panel this window drew, for a press on it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PanelHit {
    New(PaneId, Rect),
    Switch(PaneId, Rect),
    Info(PaneId, Rect),
    Hide(PaneId, Rect),
    Divider(PaneId, Rect),
    Body(PaneId, Rect),
    /// A button under a message: the message id and the button id.
    Button(PaneId, String, String, Rect),
    Input(PaneId, Rect),
    Send(PaneId, Rect),
    Attach(PaneId, Rect),
    /// A bubble: its message id (a right click opens the message menu).
    Bubble(PaneId, String, Rect),
    /// A code block's copy glyph: the message id and the block.
    Copy(PaneId, String, usize, Rect),
}

impl PanelHit {
    fn rect(&self) -> Rect {
        match self {
            PanelHit::New(_, r) | PanelHit::Switch(_, r) | PanelHit::Info(_, r) | PanelHit::Hide(_, r) | PanelHit::Divider(_, r) | PanelHit::Body(_, r) | PanelHit::Input(_, r) | PanelHit::Send(_, r) | PanelHit::Attach(_, r) => *r,
            PanelHit::Button(_, _, _, r) | PanelHit::Bubble(_, _, r) | PanelHit::Copy(_, _, _, r) => *r,
        }
    }

    fn button(&self) -> Option<(PaneId, IcButton)> {
        match *self {
            PanelHit::New(p, _) => Some((p, IcButton::New)),
            PanelHit::Switch(p, _) => Some((p, IcButton::Switch)),
            PanelHit::Info(p, _) => Some((p, IcButton::Info)),
            PanelHit::Hide(p, _) => Some((p, IcButton::Hide)),
            _ => None,
        }
    }

    fn pane(&self) -> PaneId {
        match self {
            PanelHit::New(p, _) | PanelHit::Switch(p, _) | PanelHit::Info(p, _) | PanelHit::Hide(p, _) | PanelHit::Divider(p, _) | PanelHit::Body(p, _) | PanelHit::Input(p, _) | PanelHit::Send(p, _) | PanelHit::Attach(p, _) => *p,
            PanelHit::Button(p, ..) | PanelHit::Bubble(p, ..) | PanelHit::Copy(p, ..) => *p,
        }
    }
}

/// The ids of the Switch menu's items: `aichat:inline:<pane>:<chat>`.
const SWITCH_ID: &str = "aichat:inline:";

fn switch_id(pane: PaneId, chat: &str) -> String {
    format!("{SWITCH_ID}{pane}:{chat}")
}

fn parse_switch_id(id: &str) -> Option<(PaneId, &str)> {
    let (pane, chat) = id.strip_prefix(SWITCH_ID)?.split_once(':')?;
    Some((pane.parse().ok()?, chat)).filter(|(_, c)| !c.is_empty())
}

impl App {
    /// The panel's stored width in logical pixels, when one was dragged.
    fn inline_stored_width(&self) -> Option<f32> {
        self.globals.get(ic::WIDTH_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map(|f| f as f32)
    }

    /// Whether a chat is still in the store. With the store not open yet nothing says it is
    /// gone (restore binds without opening it, ruling F21); a read error keeps the panel.
    fn inline_chat_exists(&self, chat: &str) -> bool {
        self.chat_db.as_ref().is_none_or(|db| !matches!(db.chat(chat), Ok(None)))
    }

    /// The panel's width in logical pixels: 0 without a shown, existing chat, and in a tmux
    /// pane, whose size tmux locks (the panel is refused there).
    pub(super) fn inline_width(&self, id: PaneId) -> f32 {
        let Some(s) = self.inline.get(&id) else { return 0.0 };
        if self.tmux_of(id).is_some() {
            return 0.0;
        }
        let exists = s.binding.chat.as_deref().is_some_and(|c| self.inline_chat_exists(c));
        ic::width(&s.binding, exists, self.inline_stored_width())
    }

    /// The panels of the current tab: (pane, panel rectangle).
    fn inline_panels(&self) -> Vec<(PaneId, Rect)> {
        if self.inline.is_empty() {
            return Vec::new();
        }
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), GAP).into_iter().filter_map(|(id, r)| self.gutter_split(id, self.pane_bar_split(tab, id, r).0).2.map(|p| (id, p))).collect()
    }

    /// Whether the focused pane's inline chat shows (the View menu's check mark).
    pub(super) fn inline_shown(&self) -> bool {
        self.focused_id().is_some_and(|id| self.inline_width(id) > 0.0)
    }

    /// The most recent chat linked to the pane, opening the store if need be.
    fn inline_linked_chat(&mut self, pane: PaneId) -> Option<String> {
        let reference = self.session_ref(pane)?;
        let chats = self.chat_db()?.chats().ok()?;
        crate::chat_list::last_linked(&chats, &reference).map(|c| c.id.clone())
    }

    /// View > Show Inline Chat (`toggleInlineChat`, `PSS:2586-2624`): the focused pane. It
    /// never opens the chat window.
    pub(super) fn toggle_inline_chat(&mut self) {
        let Some(pane) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        let binding = self.inline.get(&pane).map(|s| s.binding.clone()).unwrap_or_default();
        let showing = binding.chat.is_some() && binding.visible;
        if !showing && self.tmux_of(pane).is_some() {
            return self.banners.set("inline_chat", ic::TMUX_REFUSED);
        }
        // A bound chat flips with no gate; after a failed database open nothing more happens
        // (12c checklist row 42).
        let gate = binding.chat.is_some() || (!self.chat_db_failed && self.ai_check(false));
        let linked = if gate && binding.chat.is_none() { self.inline_linked_chat(pane) } else { None };
        match ic::toggle(&binding, gate, linked.as_deref()) {
            Toggle::Flip => {
                if let Some(chat) = binding.chat.as_deref() {
                    self.bind_inline(pane, chat, !binding.visible);
                }
            }
            Toggle::Bind(chat) => self.bind_inline(pane, &chat, true),
            Toggle::Create => {
                if let Some(chat) = self.create_inline_chat(pane) {
                    self.bind_inline(pane, &chat, true);
                }
            }
            Toggle::Refused => {}
        }
    }

    /// A new chat for the pane's panel (`createInlineChat`, `PSS:2665-2699`): "Chat about
    /// <name>", not linked, with an offerLink bubble to link it. None when the pane has no
    /// reference to offer (the earlier run's links could not be cleared) or the store is closed.
    fn create_inline_chat(&mut self, pane: PaneId) -> Option<String> {
        let Some(guid) = self.session_ref(pane) else {
            if let Some(text) = super::chat_app::links_off_text(self.chat_links_off) {
                self.banners.set("chat_links", text.to_string());
            }
            return None;
        };
        let name = tb::drop_unsafe(&self.window_title(pane));
        let name = if name.trim().is_empty() { "Unnamed session".to_string() } else { name };
        let id = self.chat_create(&tb::chat_about_title(&name), None, None, "", vec![])?;
        self.client_local(&id, crate::chat_message::ClientLocal::OfferLink { terminal: true, guid, name: Some(name) });
        Some(id)
    }

    /// Binds the pane's panel to `chat` and shows or hides it (`inlineChatID`,
    /// `inlineChatVisible`): the pane's columns change, so its window lays out again. A tmux
    /// pane refuses a shown panel, as Clippings does.
    pub(super) fn bind_inline(&mut self, pane: PaneId, chat: &str, visible: bool) {
        if visible && self.tmux_of(pane).is_some() {
            return self.banners.set("inline_chat", ic::TMUX_REFUSED);
        }
        // The pane may sit in another window than the current one.
        if !self.enter_pane_window(pane) {
            return;
        }
        // Hiding the panel or showing another chat in it detaches the old one: its subscription goes
        // and its streaming stops (`detach`, `CVC:3649-3669`).
        let old = self.inline.get(&pane).and_then(|s| s.binding.chat.clone());
        if !visible || old.as_deref() != Some(chat) {
            self.inline_detach(pane);
        }
        let s = self.inline.entry(pane).or_default();
        s.binding = Binding { chat: Some(chat.to_string()), visible };
        if !visible {
            s.keys = false;
        }
        self.mark_state_dirty();
        self.relayout();
        self.sync_checks();
    }

    /// Clears the bindings whose chat is gone, once the store says so (not during layout,
    /// `CVC:3901-3913`): the panel goes and the pane's columns come back.
    pub(super) fn inline_sweep(&mut self) {
        let Some(db) = self.chat_db.as_ref() else { return };
        let gone: Vec<PaneId> = self.inline.iter().filter(|(_, s)| s.binding.chat.as_deref().is_some_and(|c| matches!(db.chat(c), Ok(None)))).map(|(id, _)| *id).collect();
        if gone.is_empty() {
            return;
        }
        for id in gone {
            self.inline_drop(id);
        }
        self.mark_state_dirty();
        self.relayout_changed_windows();
        self.sync_checks();
        self.redraw_all();
    }

    /// The toolbar's title: the bound chat's title, else "AI Chat".
    fn inline_title(&self, s: &InlineState) -> String {
        let Some(chat) = s.binding.chat.as_deref() else { return ic::TITLE.to_string() };
        let mut cache = s.title.borrow_mut();
        if let Some((at, title)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(1) {
                return title.clone();
            }
        }
        let title = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()).map(|c| c.title).filter(|t| !t.trim().is_empty()).map(|t| tb::drop_unsafe(&t)).unwrap_or_else(|| ic::TITLE.to_string());
        *cache = Some((Instant::now(), title.clone()));
        title
    }

    /// Draws the current tab's panels from what `prepare_inline_chats` laid out, and answers
    /// their hit rectangles (this window's).
    pub(super) fn draw_inline_chats(&self, frame: &mut scene::Frame, cell: (f32, f32)) -> Vec<PanelHit> {
        let mut hits = Vec::new();
        let scale = self.scale();
        for (id, panel) in self.inline_panels() {
            let Some(s) = self.inline.get(&id) else { continue };
            let l = ic::toolbar(panel, scale);
            let text = match s.hover {
                Some(b) => b.tip().to_string(),
                None => self.inline_title(s),
            };
            let cols = (l.title.w / cell.0).floor().max(0.0) as usize;
            let title = ic::truncate_tail(&text, cols);
            scene::inline_chat_panel(frame, &scene::InlineDraw { panel, layout: &l, title: &title, hover: s.hover }, cell, scale);
            hits.push(PanelHit::Divider(id, Rect { w: 4.0 * scale, ..panel }));
            hits.push(PanelHit::New(id, l.new));
            hits.push(PanelHit::Switch(id, l.switch));
            hits.push(PanelHit::Info(id, l.info));
            hits.push(PanelHit::Hide(id, l.hide));
            if let Some(v) = s.view.as_ref().filter(|v| v.panel == panel) {
                self.draw_inline_body(frame, id, s, v, cell, scale, &mut hits);
            }
            hits.push(PanelHit::Body(id, panel));
        }
        hits
    }

    /// One panel's conversation and input, and the rectangles they answer presses with.
    #[allow(clippy::too_many_arguments)]
    fn draw_inline_body(&self, frame: &mut scene::Frame, id: PaneId, s: &InlineState, v: &PanelView, cell: (f32, f32), scale: f32, hits: &mut Vec<PanelHit>) {
        let top = v.area.y - v.scroll;
        let moved = |r: Rect| Rect { x: v.panel.x + r.x, y: top + r.y, ..r };
        let mut rows = Vec::new();
        let mut buttons_hits = Vec::new();
        let mut copy_hits = Vec::new();
        let mut bubble_hits = Vec::new();
        for (b, row) in v.boxes.iter().zip(v.rows.iter()) {
            let (bubble, text) = (moved(b.bubble), moved(b.text));
            // Rows wholly above or below the view are not drawn.
            let (y0, y1) = (if b.bubble.w > 0.0 { bubble.y } else { text.y }, if b.bubble.w > 0.0 { bubble.y + bubble.h } else { text.y + text.h });
            if y1 < v.area.y || y0 > v.area.y + v.area.h {
                continue;
            }
            let (date, typing) = match &row.item {
                Item::Date(label) => (Some(label.as_str()), false),
                Item::Typing => (None, true),
                Item::Message(_) => (None, false),
            };
            let laid = v.laid.get(b.item).and_then(Option::as_ref);
            let lines: &[VisualLine] = laid.map_or(&[], |l| l.lines.as_slice());
            let g = cell.1;
            let copies: Vec<(usize, Rect)> = laid.map_or(Vec::new(), |l| l.copies.iter().map(|(block, y, _)| (*block, Rect { x: text.x + text.w - g - 2.0 * scale, y: text.y + y, w: g, h: g })).collect());
            let copied = s.copied.as_ref().filter(|(m, _, at)| *m == row.id && at.elapsed() < chat_view::COPIED_FOR).and_then(|(_, block, _)| copies.iter().find(|(b, _)| b == block)).map(|(_, r)| Rect { x: r.x - 60.0 * scale, w: 56.0 * scale, ..*r });
            let live = row.enabled && !s.pressed.contains(&row.id);
            let titles: Vec<f32> = laid.map_or(Vec::new(), |l| l.titles.clone());
            let button_rects: Vec<(Rect, &str, f32, bool, bool)> = b
                .buttons
                .iter()
                .zip(&row.buttons)
                .enumerate()
                .map(|(i, (r, btn))| (moved(*r), btn.title.as_str(), titles.get(i).copied().unwrap_or(0.0), btn.destructive, live && !crate::chat_buttons::is_orchestration(&btn.id)))
                .collect();
            for ((r, _, _, _, on), btn) in button_rects.iter().zip(&row.buttons) {
                if let (true, Some(c)) = (*on, clip_to(*r, v.area)) {
                    buttons_hits.push(PanelHit::Button(id, row.id.clone(), btn.id.clone(), c));
                }
            }
            for (block, r) in &copies {
                if let Some(c) = clip_to(*r, v.area) {
                    copy_hits.push(PanelHit::Copy(id, row.id.clone(), *block, c));
                }
            }
            if matches!(row.item, Item::Message(_)) {
                if let Some(c) = clip_to(bubble, v.area) {
                    bubble_hits.push(PanelHit::Bubble(id, row.id.clone(), c));
                }
            }
            rows.push(scene::InlineRowDraw { bubble, text, user: b.user, lines, date, typing, buttons: button_rects, copies: copies.iter().map(|(_, r)| *r).collect(), copied });
        }
        let status = s.copied.as_ref().filter(|(_, _, at)| at.elapsed() < chat_view::COPIED_FOR).map(|_| chat_view::COPIED.to_string());
        let pending: Vec<String> = s.draft.pending.iter().map(crate::chat_input::Pending::label).collect();
        let strip_text = if status.is_some() { status } else if pending.is_empty() { None } else { Some(pending.join(", ")) };
        let (glyph, send_on) = ic::send_state(&s.draft, false, v.typing);
        let placeholder = crate::chat_input::placeholder(s.orchestrating);
        let draw = scene::InlineBodyDraw {
            area: v.area,
            rows,
            strip: v.strip.zip(strip_text.as_deref()),
            input: scene::InlineInputDraw {
                rect: v.input,
                attach: v.parts.attach,
                send: v.parts.send,
                send_glyph: glyph,
                send_enabled: send_on,
                clip: v.parts.text,
                lines: &v.lines,
                selection: &v.selection,
                caret: v.caret.filter(|_| s.keys),
                placeholder: s.edit.text.is_empty().then_some(placeholder),
                focused: s.keys,
                text_scale: v.text_scale,
            },
        };
        scene::inline_chat_body(frame, &draw, cell, scale);
        // Specific rectangles before the whole body, buttons before the bubbles they sit in.
        hits.extend(copy_hits);
        hits.extend(buttons_hits);
        hits.extend(bubble_hits);
        hits.push(PanelHit::Attach(id, v.parts.attach));
        hits.push(PanelHit::Send(id, v.parts.send));
        hits.push(PanelHit::Input(id, v.input));
    }

    /// A press. True when it was in a panel and the panel took it: a left press, and a right
    /// press on a bubble (its message menu); other buttons over a panel are swallowed by
    /// `inline_split_hit`. A left press elsewhere gives the keyboard back to the terminal.
    pub(super) fn inline_press(&mut self, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(hit) = self.inline_hits.iter().find(|h| h.rect().contains(x, y)).cloned() else {
            if button == MouseButton::Left {
                for s in self.inline.values_mut() {
                    s.keys = false;
                }
            }
            return false;
        };
        let pane = hit.pane();
        if button == MouseButton::Right {
            if let PanelHit::Bubble(_, id, _) = &hit {
                self.inline_message_menu(pane, id, (x, y));
                return true;
            }
            return false;
        }
        if button != MouseButton::Left {
            return false;
        }
        let r = hit.rect();
        // The panel's session becomes the focused one, and only this panel may hold the keys.
        for (p, s) in self.inline.iter_mut() {
            if *p != pane {
                s.keys = false;
            }
        }
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        self.refresh_route();
        match hit {
            PanelHit::Divider(..) => {
                let start_w = crate::clippings::clamp_width(self.inline_stored_width().unwrap_or(ic::DEFAULT_WIDTH));
                self.drag = Drag::Inline { pane, start_x: x, start_w };
            }
            PanelHit::New(..) => self.inline_new_chat(pane),
            PanelHit::Switch(..) => self.show_switch_menu(pane, (r.x, r.y + r.h)),
            PanelHit::Info(..) => {
                if let Some(chat) = self.inline.get(&pane).and_then(|s| s.binding.chat.clone()) {
                    self.show_session_menu(&chat, Some(pane), (r.x, r.y + r.h));
                }
            }
            PanelHit::Hide(..) => {
                if let Some(chat) = self.inline.get(&pane).and_then(|s| s.binding.chat.clone()) {
                    self.bind_inline(pane, &chat, false);
                }
            }
            PanelHit::Button(_, message, button, _) => self.inline_button(pane, &message, &button, event_loop),
            PanelHit::Copy(_, message, block, _) => self.inline_copy(pane, &message, block),
            PanelHit::Attach(..) => self.inline_attach_menu(pane, (r.x, r.y + r.h)),
            PanelHit::Send(..) => {
                if self.inline.get(&pane).and_then(|s| s.view.as_ref()).is_some_and(|v| v.typing) {
                    self.stop_chat_for(ChatView::Panel(pane));
                } else {
                    self.send_input_for(ChatView::Panel(pane));
                }
            }
            PanelHit::Input(..) => self.inline_input_press(pane, x, y),
            PanelHit::Body(..) | PanelHit::Bubble(..) => {
                if let Some(s) = self.inline.get_mut(&pane) {
                    s.keys = true;
                }
            }
        }
        self.redraw();
        true
    }

    /// A button under a message: greyed at once (the permissions bubble is tapped again and
    /// again), then answered through `chat_button_pressed`, which re-checks it against the store.
    fn inline_button(&mut self, pane: PaneId, message: &str, button: &str, event_loop: &ActiveEventLoop) {
        let Some(chat) = self.view_chat(ChatView::Panel(pane)) else { return };
        if let Some(s) = self.inline.get_mut(&pane) {
            if !s.convo.iter().any(|r| r.id == message && crate::chat_window_ui::stays_live(r)) {
                s.pressed.insert(message.to_string());
            }
        }
        self.chat_button_pressed(&chat, message, button, event_loop);
        if let Some(s) = self.inline.get_mut(&pane) {
            s.dirty = true;
        }
    }

    /// A code block's copy glyph: its text on the clipboard and "Copied" in the strip for a second.
    fn inline_copy(&mut self, pane: PaneId, message: &str, block: usize) {
        let text = self.inline.get(&pane).and_then(|s| s.view.as_ref()).and_then(|v| {
            let at = v.rows.iter().position(|r| r.id == message)?;
            v.laid.get(at)?.as_ref()?.copies.iter().find(|(b, _, _)| *b == block).map(|(_, _, t)| t.clone())
        });
        let Some(text) = text else { return };
        self.chat_copy(text);
        if let Some(s) = self.inline.get_mut(&pane) {
            s.copied = Some((message.to_string(), block, Instant::now()));
        }
    }

    /// The right-click menu of a bubble: Task 4's items with the pane in their ids, so Edit refills
    /// this panel's input and Fork opens the chat window on the new chat.
    fn inline_message_menu(&mut self, pane: PaneId, id: &str, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let view = ChatView::Panel(pane);
        let Some(editable) = self.chat_visible_message(view, id).map(chat_view::editable) else { return };
        let menu = muda::Menu::new();
        for title in chat_view::message_menu(editable) {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(chat_view::menu_id(&title.to_lowercase(), Some(pane), id), title, true, None)) {
                self.banners.set("menu", format!("Chat menu: {e}"));
                return;
            }
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// The attach button's popup: files or a folder, as the window's.
    fn inline_attach_menu(&mut self, pane: PaneId, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let menu = muda::Menu::new();
        for (kind, title) in [("files", crate::chat_input::ATTACH_FILES), ("folder", crate::chat_input::ATTACH_FOLDER)] {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(format!("{PANEL_ATTACH_ID}{kind}:{pane}"), title, true, None)) {
                self.banners.set("menu", format!("Attach menu: {e}"));
                return;
            }
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// A press in the input: the panel takes the keyboard and the caret goes where the pointer is
    /// (Shift extends the selection, a double press selects a word).
    fn inline_input_press(&mut self, pane: PaneId, x: f32, y: f32) {
        let Some((parts, first, line_h)) = self.inline.get(&pane).and_then(|s| s.view.as_ref()).map(|v| (v.parts, v.first_line, v.line_h)) else { return };
        let scale = self.scale();
        let shift = self.mods.shift;
        let Some(g) = self.gpu.as_mut() else { return };
        let Some(s) = self.inline.get_mut(&pane) else { return };
        s.keys = true;
        let renderer = RefCell::new(&mut g.renderer);
        let measure = |t: &str| renderer.borrow_mut().toolbelt_width(&t.replace('\t', "    "), scale);
        let font = renderer.borrow().toolbelt_font_key(scale);
        let lines = s.wrap.wrap(&s.edit.text, parts.text.w, font, &measure);
        let line = (first + (((y - parts.text.y) / line_h.max(1.0)).floor().max(0.0) as usize)).min(lines.len().saturating_sub(1));
        let at = text_edit::index_at(&s.edit.text, &lines, line, x - parts.text.x, &measure);
        s.edit.click(at, text_edit::ends_line(&lines, line, at), shift, super::toolbelt_tools_ui::now_ms(), super::clippings_ui::double_click_ms());
    }

    /// The wheel over a panel scrolls its conversation and unpins it until it is back at the
    /// bottom. True when it was over a panel.
    pub(super) fn inline_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.cursor;
        let Some((id, _)) = self.inline_panels().into_iter().find(|(_, p)| p.contains(x, y)) else { return false };
        let (_, ch) = self.cell();
        let Some(s) = self.inline.get_mut(&id) else { return false };
        let Some(v) = s.view.as_ref() else { return true };
        let max = (v.content_h - v.area.h).max(0.0);
        s.scroll = (v.scroll - dy * ch * 3.0).clamp(0.0, max);
        s.unpinned = s.scroll < max - 1.0;
        self.redraw();
        true
    }

    /// Keys while a panel has the keyboard: Enter by Task 7's rules, the rest through the editor as
    /// the Notes tool routes them (Ctrl chords, Paste from the clipboard); Escape gives the keyboard
    /// back to the terminal; Win chords are not taken (they stay Tandem's).
    pub(super) fn inline_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(pane) = self.focused_id() else { return false };
        if !self.inline.get(&pane).is_some_and(|s| s.keys && s.binding.visible && s.view.is_some()) {
            return false;
        }
        let mods = self.mods;
        if super::toolbelt_tools_ui::is_modifier(&ev.logical_key) {
            return true;
        }
        if ev.logical_key == WKey::Named(NamedKey::Escape) && !(mods.ctrl || mods.alt || mods.cmd) {
            if let Some(s) = self.inline.get_mut(&pane) {
                s.keys = false;
            }
            self.redraw();
            return true;
        }
        let view = ChatView::Panel(pane);
        if ev.logical_key == WKey::Named(NamedKey::Enter) && !(mods.ctrl || mods.alt || mods.cmd) {
            let typing = self.inline.get(&pane).and_then(|s| s.view.as_ref()).is_some_and(|v| v.typing);
            let draft = self.inline.get(&pane).map(|s| s.draft.clone()).unwrap_or_default();
            use crate::chat_input::Enter;
            match ic::panel_enter(&draft, mods.shift, typing) {
                Enter::Send => self.send_input_for(view),
                Enter::StopThenSend => {
                    self.stop_chat_for(view);
                    self.send_input_for(view);
                }
                Enter::Stop => self.stop_chat_for(view),
                // The editor's own newline: Shift+Enter, and Enter when nothing can be sent.
                Enter::Newline | Enter::Nothing => return self.inline_edit_key(pane, EditKey::Enter),
            }
            self.redraw();
            return true;
        }
        let Some((key, ctrl, alt)) = super::toolbelt_more_ui::edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return !(mods.ctrl || mods.alt || mods.cmd) };
        if !text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let key = match key {
            EditKey::Paste(_) => EditKey::Paste(self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default()),
            k => k,
        };
        self.inline_edit_key(pane, key)
    }

    /// One editing key for a panel's input.
    fn inline_edit_key(&mut self, pane: PaneId, key: EditKey) -> bool {
        let scale = self.scale();
        let shift = self.mods.shift;
        let Some(parts) = self.inline.get(&pane).and_then(|s| s.view.as_ref()).map(|v| v.parts) else { return false };
        let cut = key == EditKey::Cut;
        let Some(g) = self.gpu.as_mut() else { return false };
        let Some(s) = self.inline.get_mut(&pane) else { return false };
        let renderer = RefCell::new(&mut g.renderer);
        let measure = |t: &str| renderer.borrow_mut().toolbelt_width(&t.replace('\t', "    "), scale);
        let font = renderer.borrow().toolbelt_font_key(scale);
        let lines = s.wrap.wrap(&s.edit.text, parts.text.w, font, &measure);
        s.edit.page_lines = ic::INPUT_LINES_MAX;
        let out = s.edit.key(key, shift, &lines, &measure);
        if matches!(out, EditOut::Changed) || (cut && matches!(out, EditOut::Copied(_))) {
            s.draft.text = s.edit.text.clone();
        }
        if let EditOut::Copied(text) = out {
            if let Some(cb) = self.clipboard.as_mut() {
                if let Err(e) = cb.set_text(text) {
                    self.banners.set("clipboard", format!("Could not copy: {e}"));
                }
            }
        }
        self.redraw();
        true
    }

    /// The panel's "new chat" button (`CVC:3703-3717`): the gate, then a new chat shown.
    fn inline_new_chat(&mut self, pane: PaneId) {
        if self.chat_db_failed || !self.ai_check(false) {
            return;
        }
        if let Some(chat) = self.create_inline_chat(pane) {
            self.bind_inline(pane, &chat, true);
        }
    }

    /// The Switch button's menu (`CVC:3719-3767`): every chat, the current one checked, the
    /// chats linked to this session first under a separator (muda has no bold item). A pick
    /// arrives as `aichat:inline:<pane>:<chat>` and lives until then (`popup_menu`).
    fn show_switch_menu(&mut self, pane: PaneId, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let reference = self.session_ref(pane);
        let current = self.inline.get(&pane).and_then(|s| s.binding.chat.clone());
        let Some(chats) = self.chat_db().and_then(|db| db.chats().ok()) else { return };
        let rows: Vec<(String, String, bool, bool)> = chats.iter().map(|c| (c.id.clone(), c.title.clone(), c.orchestration_enabled, reference.is_some() && c.terminal_session_guid == reference)).collect();
        let (here, rest) = ic::switch_groups(ic::switch_menu(&rows, current.as_deref()));
        let menu = muda::Menu::new();
        let mut results = Vec::new();
        for (n, items) in [&here, &rest].into_iter().enumerate() {
            if n == 1 && !here.is_empty() && !rest.is_empty() {
                results.push(menu.append(&muda::PredefinedMenuItem::separator()));
            }
            for i in items {
                let title = tb::drop_unsafe(&i.title).replace('&', "&&");
                let title = if i.orchestrator { format!("{} {title}", crate::chat_list::ORCHESTRATOR_GLYPH) } else { title };
                results.push(match &i.id {
                    None => menu.append(&muda::MenuItem::new(title, false, None)),
                    Some(chat) => menu.append(&muda::CheckMenuItem::with_id(switch_id(pane, chat), title, true, i.checked, None)),
                });
            }
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Chat menu: {e}"));
            return;
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the Switch menu: binds the pane's panel to the chat, if the chat is still
    /// there (`CVC:3769-3786`). False for other ids.
    pub(super) fn inline_menu_picked(&mut self, id: &str) -> bool {
        if let Some(rest) = id.strip_prefix(PANEL_ATTACH_ID) {
            let Some((kind, pane)) = rest.split_once(':').and_then(|(k, p)| Some((k, p.parse::<PaneId>().ok()?))) else { return true };
            let dialog = rfd::FileDialog::new();
            let paths = match kind {
                "files" => dialog.pick_files(),
                "folder" => dialog.pick_folders(),
                _ => None,
            };
            if let Some(paths) = paths {
                self.chat_attach(ChatView::Panel(pane), &paths);
                self.redraw();
            }
            return true;
        }
        if !id.starts_with(SWITCH_ID) {
            return false;
        }
        let Some((pane, chat)) = parse_switch_id(id) else { return true };
        if self.panes.contains_key(&pane) && self.chat_db.is_some() && self.inline_chat_exists(chat) {
            self.bind_inline(pane, chat, true);
        }
        true
    }

    /// Pointer motion: the button under the pointer (its tip shows), and a width drag.
    pub(super) fn inline_move(&mut self, x: f32, y: f32) {
        if self.inline.is_empty() {
            return;
        }
        let over = self.inline_hits.iter().find(|h| h.rect().contains(x, y)).and_then(PanelHit::button);
        let mut changed = false;
        for (id, s) in self.inline.iter_mut() {
            let now = over.filter(|(p, _)| p == id).map(|(_, b)| b);
            if s.hover != now {
                s.hover = now;
                changed = true;
            }
        }
        if changed {
            self.redraw();
        }
        if let Drag::Inline { pane, start_x, start_w } = self.drag {
            if !self.inline.contains_key(&pane) {
                return;
            }
            let w = crate::clippings::clamp_width(start_w + (start_x - x) / self.scale());
            self.globals.set(ic::WIDTH_KEY, toml::Value::Float(f64::from(w.round())));
            self.relayout();
        }
    }

    /// The width drag ends: the width is kept for every panel.
    pub(super) fn inline_release(&mut self) {
        self.save_globals();
    }

    /// Whether (x, y) is in one of the current tab's panels.
    pub(super) fn inline_split_hit(&self, x: f32, y: f32) -> bool {
        self.inline_panels().iter().any(|(_, p)| p.contains(x, y))
    }

    /// The session keys an arrangement keeps for a pane.
    pub(super) fn inline_table(&self, id: PaneId) -> toml::Table {
        self.inline.get(&id).map(|s| ic::to_table(&s.binding)).unwrap_or_default()
    }

    /// A restored session's inline chat, with no gate and without opening the store
    /// (`PSS:2626-2663`).
    pub(super) fn restore_inline(&mut self, id: PaneId, session: &toml::Value) {
        let Some((chat, visible)) = session.as_table().and_then(ic::from_table) else { return };
        let exists = self.chat_db.as_ref().is_some_and(|db| matches!(db.chat(&chat), Ok(Some(_))));
        if let Some(binding) = ic::restore(&chat, visible, self.chat_db.is_some(), exists) {
            self.inline.insert(id, InlineState { binding, ..InlineState::default() });
        }
    }
}

/// `r` cut to `clip`; None when nothing is left.
fn clip_to(r: Rect, clip: Rect) -> Option<Rect> {
    let (x0, y0) = (r.x.max(clip.x), r.y.max(clip.y));
    let (x1, y1) = ((r.x + r.w).min(clip.x + clip.w), (r.y + r.h).min(clip.y + clip.h));
    (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

impl App {
    /// The panel's chat gets its broker subscription and its messages once it shows, and loses them
    /// when it is hidden, unbound or its pane closes (`detach`, `CVC:3649-3669`).
    fn inline_attach(&mut self, pane: PaneId) {
        let Some(chat) = self.inline.get(&pane).filter(|s| s.binding.visible).and_then(|s| s.binding.chat.clone()) else { return };
        // A restored panel opens the store when it first draws (F21); a failed open is not retried.
        if self.chat_db.is_none() && !self.chat_db_failed {
            let _ = self.chat_db();
        }
        if self.inline.get(&pane).is_some_and(|s| s.sub.is_none()) {
            let sub = self.chat_broker.subscribe(Some(&chat));
            if let Some(s) = self.inline.get_mut(&pane) {
                s.sub = Some(sub);
            }
        }
        if self.inline.get(&pane).is_some_and(|s| s.msgs_chat.as_deref() != Some(chat.as_str())) {
            let msgs = match self.chat_db.as_ref().map(|db| db.messages(&chat)) {
                Some(Ok(m)) => m,
                Some(Err(_)) => {
                    self.banners.set("chat_read", "The chat could not be read.".to_string());
                    Vec::new()
                }
                None => Vec::new(),
            };
            let orchestrating = self.chat_db.as_ref().and_then(|db| db.chat(&chat).ok().flatten()).is_some_and(|c| c.orchestration_enabled);
            if let Some(s) = self.inline.get_mut(&pane) {
                s.msgs = msgs;
                s.msgs_chat = Some(chat);
                s.orchestrating = orchestrating;
                s.dirty = true;
            }
        }
    }

    /// The panel's subscription ends and its streaming stops, unless the chat window shows the same
    /// chat (streaming is per chat, app-wide).
    pub(super) fn inline_detach(&mut self, pane: PaneId) {
        let Some(s) = self.inline.get_mut(&pane) else { return };
        let sub = s.sub.take();
        let chat = s.binding.chat.clone();
        s.msgs = Vec::new();
        s.msgs_chat = None;
        s.convo = Arc::default();
        s.view = None;
        s.laid.clear();
        s.pressed.clear();
        if let Some(sub) = sub {
            self.chat_broker.unsubscribe(sub);
        }
        if let Some(chat) = chat.filter(|c| self.chat_shown.as_deref() != Some(c.as_str())) {
            self.stop_streaming(&chat);
        }
    }

    /// The pane's panel is gone (its pane closed, its chat was deleted, a restore failed).
    pub(super) fn inline_drop(&mut self, pane: PaneId) {
        self.inline_detach(pane);
        self.inline.remove(&pane);
    }

    /// The panels' subscriptions are drained: a delivery changes the kept messages and redraws only
    /// the windows showing that chat's panels. The next wake-up (the end of a "Copied").
    pub(super) fn inline_tick(&mut self) -> Option<Instant> {
        let panes: Vec<PaneId> = self.inline.iter().filter(|(_, s)| s.sub.is_some()).map(|(p, _)| *p).collect();
        let mut touched = Vec::new();
        for pane in panes {
            let Some(sub) = self.inline.get(&pane).and_then(|s| s.sub) else { continue };
            let updates = self.chat_broker.drain(sub);
            if updates.is_empty() {
                continue;
            }
            let db = self.chat_db.as_ref();
            let reload = |id: &str| db.and_then(|db| db.message(id).ok().flatten());
            let Some(s) = self.inline.get_mut(&pane) else { continue };
            let mut changed = false;
            for (_, u) in &updates {
                changed |= apply_update(&mut s.msgs, u, &reload) | matches!(u, crate::chat_broker::Update::Typing { .. } | crate::chat_broker::Update::Turn(_));
            }
            if changed {
                s.dirty = true;
                touched.push(pane);
            }
        }
        // A "Copied" that ran its second is cleared, and the panel draws without it.
        for (pane, s) in self.inline.iter_mut() {
            if s.copied.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= chat_view::COPIED_FOR) {
                s.copied = None;
                touched.push(*pane);
            }
        }
        for pane in touched {
            self.redraw_pane_window(pane);
        }
        let now = Instant::now();
        self.inline.values().filter_map(|s| s.copied.as_ref().map(|(_, _, at)| *at + chat_view::COPIED_FOR)).filter(|t| *t > now).min()
    }

    /// Asks the window that holds `pane` to draw again.
    fn redraw_pane_window(&self, pane: PaneId) {
        if self.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
            return self.redraw();
        }
        for (_, slot) in self.windows.parked() {
            if slot.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
                if let Some(g) = &slot.gpu {
                    g.window.request_redraw();
                }
            }
        }
    }

    /// Edit puts the person's text back in this panel's input and gives it the keyboard.
    pub(super) fn inline_set_input(&mut self, pane: PaneId, text: &str) {
        if let Some(s) = self.inline.get_mut(&pane) {
            s.draft.text = text.to_string();
            s.edit.load(text);
            s.keys = true;
        }
        self.redraw();
    }

    /// A sent message clears the input and brings the conversation to its bottom.
    pub(super) fn inline_sent(&mut self, pane: PaneId) {
        if let Some(s) = self.inline.get_mut(&pane) {
            s.draft = crate::chat_input::Draft::default();
            s.edit.load("");
            s.unpinned = false;
            s.dirty = true;
        }
        self.redraw();
    }

    /// Lays out every shown panel of the current tab for this frame: its rows from the kept
    /// messages (made by the window's own `chat_convo_rows`), each at the bubble's text width with
    /// the porthole's layout, the boxes, the scroll, and the input with its lines, selection and
    /// caret. Drawing and presses read the result (`PanelView`).
    pub(super) fn prepare_inline_chats(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let panels = self.inline_panels();
        let (scale, cell, dark) = (self.scale(), self.cell(), self.dark());
        let m = self.list_metrics();
        let base = Palette::default();
        for (pane, panel) in panels {
            self.inline_attach(pane);
            let Some(chat) = self.inline.get(&pane).and_then(|s| s.binding.chat.clone()) else { continue };
            // Rows from the kept messages when they changed; the buttons again from the live state
            // twice a second (a pick began, a permission changed).
            let refresh = self.inline.get(&pane).is_some_and(|s| s.dirty || s.buttons_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)));
            if refresh {
                let msgs = self.inline.get(&pane).map(|s| s.msgs.clone()).unwrap_or_default();
                let rows = self.chat_convo_rows(&chat, &msgs);
                if let Some(s) = self.inline.get_mut(&pane) {
                    if s.dirty || *s.convo != rows {
                        s.convo = Arc::new(rows);
                    }
                    s.dirty = false;
                    s.buttons_at = Some(Instant::now());
                    // A press greys a row only until the app has answered: a row no longer enabled is forgotten.
                    let convo = s.convo.clone();
                    s.pressed.retain(|id| convo.iter().any(|r| r.id == *id && r.enabled));
                }
            }
            let Some(g) = self.gpu.as_mut() else { continue };
            let Some(s) = self.inline.get_mut(&pane) else { continue };
            let typing = self.chat_broker.is_typing(&chat, Participant::Agent);
            let renderer = RefCell::new(&mut g.renderer);
            let toolbelt = |t: &str| renderer.borrow_mut().toolbelt_width(&t.replace('\t', "    "), scale);
            let font = renderer.borrow().toolbelt_font_key(scale);
            let (line_h, text_scale) = (m.line.max(1.0), m.text_scale);
            let tb_h = ic::TOOLBAR_H * scale;
            let body = Rect { y: panel.y + tb_h.min(panel.h), h: (panel.h - tb_h).max(0.0), ..panel };
            // The input first: its lines decide its height, and so the conversation's room.
            let text_w = ic::input_parts(ic::input_rect(panel, 1, line_h, scale), scale).text.w;
            let lines = s.wrap.wrap(&s.edit.text, text_w, font, &toolbelt);
            let input = ic::input_rect(panel, lines.len(), line_h, scale);
            let parts = ic::input_parts(input, scale);
            s.edit.page_lines = ic::INPUT_LINES_MAX;
            let (caret_line, caret_x) = s.edit.caret_line(&lines, &toolbelt);
            let shown = ic::INPUT_LINES_MAX.min(lines.len().max(1));
            if caret_line < s.input_first {
                s.input_first = caret_line;
            } else if caret_line >= s.input_first + shown {
                s.input_first = caret_line + 1 - shown;
            }
            s.input_first = s.input_first.min(lines.len().saturating_sub(shown));
            let first = s.input_first;
            let y_of = |i: usize| parts.text.y + (i as f32 - first as f32) * line_h;
            let text = &s.edit.text;
            let drawn: Vec<(f32, f32, String)> = lines.iter().enumerate().skip(first).take(shown).map(|(i, v)| (parts.text.x, y_of(i), text[v.start..v.end].replace('\t', "    "))).collect();
            let mut selection = Vec::new();
            if let Some(r) = s.edit.selection() {
                let space = toolbelt(" ");
                for (i, v) in lines.iter().enumerate().skip(first).take(shown) {
                    if r.end < v.start || r.start > v.end || (r.end == v.start && v.start != v.end) {
                        continue;
                    }
                    let (a, b) = (r.start.max(v.start), r.end.min(v.end));
                    let x0 = toolbelt(&text[v.start..a]);
                    let x1 = toolbelt(&text[v.start..b]) + if r.end > v.end && lines.get(i + 1).is_some_and(|n| n.start > v.end) { space } else { 0.0 };
                    if x1 > x0 {
                        selection.push(Rect { x: parts.text.x + x0, y: y_of(i), w: x1 - x0, h: line_h });
                    }
                }
            }
            let caret = (caret_line >= first && caret_line < first + shown).then(|| Rect { x: parts.text.x + caret_x.min((parts.text.w - 1.0).max(0.0)), y: y_of(caret_line), w: 1.0, h: line_h });
            // The strip above the input: "Copied" or the pending attachments' names.
            let has_strip = s.copied.as_ref().is_some_and(|(_, _, at)| at.elapsed() < chat_view::COPIED_FOR) || !s.draft.pending.is_empty();
            let strip_h = if has_strip { line_h } else { 0.0 };
            let strip = has_strip.then(|| Rect { x: input.x, y: input.y - strip_h - 2.0 * scale, w: input.w, h: strip_h });
            let below = strip.map_or(input.y, |r| r.y) - 4.0 * scale;
            let area = Rect { x: panel.x, y: body.y, w: panel.w, h: (below - body.y).max(0.0) };
            // The conversation at the bubble's text width.
            let tw = ic::text_width(panel.w, scale);
            if s.laid_key != (scale.to_bits(), dark) {
                s.laid.clear();
                s.laid_key = (scale.to_bits(), dark);
            }
            if s.laid.len() > 400 {
                s.laid.clear();
            }
            let convo = s.convo.clone();
            let mut laid: Vec<Option<Rc<RowLayout>>> = Vec::with_capacity(convo.len());
            let mut inputs = Vec::with_capacity(convo.len());
            for row in convo.iter() {
                match &row.item {
                    Item::Date(label) => {
                        laid.push(None);
                        inputs.push((false, cell.1 * 0.85, label.chars().count() as f32 * cell.0 * 0.9, 0, true));
                    }
                    Item::Typing => {
                        laid.push(None);
                        inputs.push((false, cell.1, 5.0 * 4.0 * scale, 0, true));
                    }
                    Item::Message(_) => {
                        let system = row.style == Style::System;
                        let key = (row.key, tw.to_bits(), row.user, system);
                        let lay = match s.laid.get(&key) {
                            Some(l) => l.clone(),
                            None => {
                                let (fg, muted) = if row.user { (rgb(255, 255, 255), rgb(220, 230, 255)) } else { (rgb(0xc7, 0xc7, 0xc7), rgb(0x8a, 0x8a, 0x8a)) };
                                let palette = Palette { fg: if system { muted } else { fg }, link: if row.user { rgb(255, 255, 255) } else { base.link }, ..base.clone() };
                                let doc = chat_view::body_doc(&row.body, &palette, muted, dark);
                                let layout = {
                                    let mut r = renderer.borrow_mut();
                                    super::porthole_ui::lay_doc(&mut r, &doc, false, tw, cell.1)
                                };
                                let copies = chat_view::code_blocks(&doc).into_iter().filter_map(|b| layout.lines.iter().find(|l| l.block == b.first).map(|l| (b.first, l.y, b.text))).collect();
                                let titles = row.buttons.iter().map(|b| renderer.borrow_mut().text_width(&b.title, true, 1.0)).collect();
                                let l = Rc::new(RowLayout { h: layout.height, w: layout.width, lines: layout.lines, copies, titles });
                                s.laid.insert(key, l.clone());
                                l
                            }
                        };
                        inputs.push((row.user, lay.h, lay.w, row.buttons.len(), false));
                        laid.push(Some(lay));
                    }
                }
            }
            let (boxes, content_h) = ic::row_boxes(&inputs, panel.w, scale);
            let scroll = ic::scroll_after(s.scroll, content_h, area.h, !s.unpinned);
            s.scroll = scroll;
            s.view = Some(PanelView { panel, area, boxes, rows: convo, laid, scroll, content_h, input, parts, strip, line_h, text_scale, lines: drawn, selection, caret, first_line: first, typing });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_ids_round_trip_and_refuse_the_rest() {
        assert_eq!(parse_switch_id(&switch_id(7, "abc-1")), Some((7, "abc-1")));
        assert_eq!(parse_switch_id("aichat:inline:x:abc"), None);
        assert_eq!(parse_switch_id("aichat:inline:7:"), None);
        assert_eq!(parse_switch_id("aichat:menu:7:abc"), None);
    }
}
