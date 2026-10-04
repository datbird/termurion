//! The AI Chat window (iTerm2's `ChatWindowController`): one egui window through `EguiHost`, the
//! tmux Dashboard's pattern, data in and events out. This file holds the shell, the toolbar, the chat list and the
//! conversation; the input and the find bar are filled by the tasks after it.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::ai_catalog::{Effort, ServiceTier};
use crate::chat_buttons::{self, Button};
use crate::chat_find::{self, Cursor, Mark, Match};
use crate::chat_input::{self, Enter};
use crate::chat_list::{self, Row, Selection};
use crate::chat_message::{Content, Message, Participant};
use crate::chat_search::{self, Hit};
use crate::chat_toolbar::{self as tb, Popup, ProviderOption};
use crate::chat_view::{self, Body, CodeBlock, Item, Part, Style};
use crate::egui_host::EguiHost;
use crate::mention::{Chip, Picker};
use crate::mention_picker_ui;
use crate::palette::{rgb, Palette};
use crate::porthole::detect::Language;
use crate::porthole::render::{Block, Doc, Face};

/// What the toolbar shows (`ChatToolbar`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolbarData {
    /// The label at the left: the chat's title, else "AI Chat".
    pub title: String,
    pub providers: Vec<ProviderOption>,
    pub provider: Popup,
    /// The identifier of the selected provider row.
    pub provider_selected: Option<String>,
    pub models: Vec<String>,
    pub model: Popup,
    pub model_selected: Option<String>,
    pub efforts: Vec<(Effort, String)>,
    pub effort: Option<Effort>,
    pub tiers: Vec<(ServiceTier, String)>,
    pub tier: Option<ServiceTier>,
    /// None hides the button; Some(on) shows it, tinted when on.
    pub thinking: Option<bool>,
    pub web_search: bool,
    pub web_search_enabled: bool,
}

/// What the window shows (the app fills it; the tasks after this one add fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatData {
    /// The shown chat's id.
    pub chat: Option<String>,
    pub toolbar: ToolbarData,
    /// The chat list's rows, most recent first.
    pub rows: Vec<Row>,
    /// The search query the app runs; the field follows it when the app clears it.
    pub query: String,
    /// The results so far, newest first (12c's order).
    pub search: Vec<Hit>,
    /// More results are still being found.
    pub searching: bool,
    /// The conversation of the shown chat: dates, messages and the typing row, in order.
    pub convo: Arc<Vec<RowData>>,
    pub input: InputData,
    /// The find bar, while it is open.
    pub find: Option<FindData>,
}

/// What the find bar shows (`ChatFindBarView`): the query and mode the app runs (the field
/// follows them when the app changes them), the counter, and every match with the current one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindData {
    pub query: String,
    /// The popup item (`chat_find::popup_index`).
    pub mode_index: usize,
    pub counter: String,
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    /// Changes when the app wants the field focused with its text selected.
    pub focus: u64,
}

/// What the input shows (`ChatInputView`): the text (the field follows it when the app changes
/// it), the pending attachments' names, and the button's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputData {
    pub text: String,
    pub pending: Vec<String>,
    pub placeholder: String,
    /// The agent is typing: the button stops it.
    pub stoppable: bool,
    pub send_enabled: bool,
    pub loading: bool,
    /// Changes when the app set the text and wants the field focused.
    pub focus: u64,
    /// The sessions picked with `@` (orchestration chats): a chip each, with its name.
    pub chips: Vec<(Chip, String)>,
    /// The `@` picker under the field, while it is open.
    pub picker: Option<Picker>,
}

/// One row of the conversation, made by the app from the chat's messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowData {
    pub item: Item,
    /// The message's id ("" for a date or the typing row).
    pub id: String,
    /// The person's own message (drawn on the right, in the accent color).
    pub user: bool,
    pub style: Style,
    pub body: Body,
    /// The short time drawn beside the bubble.
    pub time: String,
    pub editable: bool,
    /// The buttons under the text, and whether they can be pressed now (Task 5).
    pub buttons: Vec<Button>,
    pub enabled: bool,
    /// The Segoe glyph drawn before the text.
    pub glyph: Option<char>,
    /// Changes when the content does; keys the rendered markdown.
    pub key: u64,
    /// The `iterm2:` links the row draws and runs (rulings P9 and the final one on the
    /// attached-content link).
    pub in_app: InApp,
}

/// Which `iterm2:` links a message draws as links: an agent's explanation response its
/// annotation reveals, a person's explanation request its attached-content link, any other
/// message none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InApp {
    #[default]
    None,
    Annotations,
    Location,
}

impl InApp {
    pub fn of(m: &Message) -> InApp {
        match (&m.author, &m.content) {
            (Participant::Agent, Content::ExplanationResponse(_)) => InApp::Annotations,
            (Participant::User, Content::ExplanationRequest(_)) => InApp::Location,
            _ => InApp::None,
        }
    }

    /// The route `url` runs from such a message, if any.
    pub fn route(self, url: &str) -> Option<crate::chat_routes::Route> {
        match self {
            InApp::None => None,
            InApp::Annotations => crate::chat_routes::from_explanation_link(url),
            InApp::Location => crate::chat_routes::from_explanation_request_link(url),
        }
    }
}

impl RowData {
    /// A date row or the typing row.
    pub fn marker(item: Item) -> RowData {
        RowData { item, id: String::new(), user: false, style: Style::System, body: Body::Plain(String::new()), time: String::new(), editable: false, buttons: Vec::new(), enabled: false, glyph: None, key: 0, in_app: InApp::None }
    }
}

/// What the person did (later tasks add variants).
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// A provider row's identifier.
    Provider(String),
    Model(String),
    Effort(Effort),
    Tier(ServiceTier),
    ToggleThinking,
    ToggleWebSearch,
    /// The session button's bottom left, in screen pixels.
    SessionMenu((f32, f32)),
    /// The list's selection now shows this chat.
    Select(String),
    /// The header's "+" button.
    NewChat,
    /// The search field's text changed.
    Search(String),
    /// A result was clicked (its index in `ChatData::search`).
    ChooseHit(usize),
    /// Enter in the list: rename this chat.
    Rename(String),
    /// Delete in the list: delete these chats (after the question).
    Delete(Vec<String>),
    /// A right click: the pointer in screen pixels and the selected chats.
    ListMenu { at: (f32, f32), ids: Vec<String> },
    /// A right click on a message: the pointer in screen pixels and the message's id.
    MessageMenu { at: (f32, f32), id: String },
    /// Copy this text (a code block's, a code attachment's).
    CopyText(String),
    /// The message menu's verbs, by message id (they also arrive from its native menu).
    Edit(String),
    DeleteMessage(String),
    Fork(String),
    /// A markdown link was clicked; the app opens it only when `chat_view::web_link` allows.
    OpenUrl(String),
    /// An `iterm2:` link of an explanation response (an annotation) or request (the attached
    /// content) was clicked: the message and the URL; the app re-checks both (ruling P9).
    RevealInApp { message: String, url: String },
    /// A file item's verbs; the string is `chat_view::file_ref(message, attachment)`.
    OpenFile(String),
    RevealFile(String),
    /// A terminal command row was clicked: its `iterm2:reveal-mark` URL.
    RevealCommand(String),
    /// A button under a message was pressed (Task 5); the app re-checks it against the store.
    Button { message: String, button: String },
    /// The input's text changed.
    /// The input's text and whether the edit was only the person's key presses (no paste, undo or IME).
    InputChanged(String),
    /// The `@` picker: Up (true) or Down (false).
    MentionMove(bool),
    /// Return, Tab or a click (its item index) on the picker.
    MentionCommit(Option<usize>),
    /// Escape on the picker.
    MentionClose,
    /// A mention chip's close button (its index).
    RemoveMention(usize),
    /// Send, or Enter: the app sends the text and attachments (only while the agent is idle).
    Send,
    /// The stop glyph, or Enter while the agent types.
    Stop,
    /// The attach button, its bottom left in screen pixels: the app opens its two-item popup.
    Attach((f32, f32)),
    RemoveAttachment(usize),
    /// The find field's text or the mode item changed.
    FindQuery(String, usize),
    FindNext,
    FindPrevious,
    FindClose,
    /// A click in a bubble's text: Find Next and Previous start from here.
    FindClick(Cursor),
    /// The text selected in the conversation (asked for with `request_selection`).
    FindUseSelection(String),
    Close,
}

/// What the list remembers between frames: the selection, the chat it last showed, and the
/// icons decoded once, keyed by their bytes' hash (None when they would not decode).
#[derive(Default)]
struct ListState {
    /// The search field's text; results replace the list while it is not empty.
    query: String,
    /// The query the app last handed back, to notice when it clears the field.
    seen_query: String,
    selection: Selection,
    last_shown: Option<String>,
    textures: HashMap<u64, Option<egui::TextureHandle>>,
}

/// A markdown body rendered once for a content and a theme.
struct Rendered {
    doc: Doc,
    blocks: Vec<CodeBlock>,
    /// Where each block sits in `Doc::plain`, the offsets find works in.
    spans: Vec<std::ops::Range<usize>>,
}

/// The row being drawn: its place among the rows, the next findable text to claim, and the
/// find matches inside it (the same order as `chat_find::body_segments`).
#[derive(Default)]
struct RowFind {
    item: usize,
    segment: usize,
    open: bool,
    matches: Vec<Match>,
    current: Option<usize>,
}

impl RowFind {
    fn claim(&mut self) -> usize {
        self.segment += 1;
        self.segment - 1
    }
}

/// What the conversation remembers between frames: rendered markdown, the code block that
/// shows "Copied", and the message a search result asked to be centered.
#[derive(Default)]
struct ConvoState {
    docs: HashMap<(u64, usize, bool), Rc<Rendered>>,
    /// (message id, code block's first block, when it was copied).
    copied: Option<(String, usize, Instant)>,
    /// (chat, message) to scroll to the center once its row is laid out.
    reveal: Option<(Option<String>, String)>,
    /// A file or code item took this frame's right click, so the bubble's own menu stays shut.
    taken: bool,
    /// Messages whose buttons were pressed and are disabled at once, until the app answers
    /// (iTerm2's optimistic disable); the permissions bubble is re-tappable and never in it.
    pressed: std::collections::HashSet<String>,
    row: RowFind,
    /// The current find match last scrolled to, so a rescan that keeps it never scrolls again.
    find_seen: Option<(String, usize, std::ops::Range<usize>)>,
    /// A new current match is waiting for its text to be laid out, to scroll to the match itself.
    match_due: bool,
    /// The match itself was scrolled to this frame, so the row's centering must not override it.
    match_scrolled: bool,
    find: FindUi,
}

/// What the find bar remembers: the field's text, and the app's query and focus request seen.
#[derive(Default)]
struct FindUi {
    text: String,
    seen_query: String,
    seen_focus: u64,
}

/// What the input remembers between frames: the text being edited and the focus request seen.
#[derive(Default)]
struct InputUi {
    text: String,
    focus_seen: u64,
}

pub struct ChatWindow {
    host: EguiHost,
    data: ChatData,
    title: String,
    pub list_visible: bool,
    list: ListState,
    convo: ConvoState,
    input: InputUi,
}

impl ChatWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<ChatWindow, String> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(tb::WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(tb::SIZE.0, tb::SIZE.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(tb::MIN_SIZE.0, tb::MIN_SIZE.1));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(ChatWindow { host, data: ChatData::default(), title: tb::WINDOW_TITLE.to_string(), list_visible: true, list: ListState::default(), convo: ConvoState::default(), input: InputUi::default() })
    }

    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.host.follow_theme(forced);
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn hwnd(&self) -> Option<isize> {
        self.host.hwnd()
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    /// Centers the frame on a monitor (`CWC:348-401`).
    pub fn center_on(&self, monitor: &winit::monitor::MonitorHandle) {
        let (m, s) = (monitor.position(), monitor.size());
        let w = self.host.window.outer_size();
        let at = winit::dpi::PhysicalPosition::new(m.x + (s.width as i32 - w.width as i32) / 2, m.y + (s.height as i32 - w.height as i32) / 2);
        self.host.window.set_outer_position(at);
    }

    /// The OS title bar's text.
    pub fn set_title(&mut self, t: &str) {
        if self.title != t {
            self.title = t.to_string();
            self.host.window.set_title(t);
        }
    }

    /// What the window shows now.
    pub fn data(&self) -> &ChatData {
        &self.data
    }

    pub fn set_data(&mut self, d: ChatData) {
        // The field follows the app's text when the app changed it (a send, Edit, a chat switch).
        if d.input.text != self.input.text {
            self.input.text = d.input.text.clone();
        }
        self.data = d;
        self.host.request_redraw();
    }

    /// Centers a message's row once it is laid out (a chosen search result, `CVC:1069-1098`).
    /// Asks for the selected conversation text; it comes back as `FindUseSelection`.
    pub fn request_selection(&mut self) {
        self.host.probe_selection();
    }

    pub fn reveal(&mut self, message: &str) {
        self.convo.reveal = Some((self.data.chat.clone(), message.to_string()));
        self.host.request_redraw();
    }

    /// What the window needs the clock for: "Copied" goes after a second, and the typing row
    /// turns. Redraws when either is due; answers when to ask again.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let mut due = None;
        if let Some((_, _, at)) = &self.convo.copied {
            let end = *at + chat_view::COPIED_FOR;
            if now >= end {
                self.convo.copied = None;
                self.host.request_redraw();
            } else {
                due = Some(end);
            }
        }
        if self.data.convo.iter().any(|r| r.item == Item::Typing) {
            self.host.request_redraw();
            due = due.into_iter().chain(Some(now + std::time::Duration::from_millis(120))).min();
        }
        due
    }

    /// One frame. Every event of the frame, in order.
    pub fn redraw(&mut self) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let d = &self.data;
        let list_visible = &mut self.list_visible;
        let list = &mut self.list;
        let convo_state = &mut self.convo;
        let input_state = &mut self.input;
        self.host.paint(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            // Ctrl+W closes, as every other egui window of Tandem does.
            if ui.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.command) {
                out.push(ChatEvent::Close);
            }
            egui::Panel::top("chat_toolbar").show(ui, |ui| {
                ui.add_space(4.0);
                toolbar(ui, &d.toolbar, icons, list_visible, &mut out);
                ui.add_space(4.0);
            });
            if *list_visible {
                egui::Panel::left("chat_list").resizable(true).default_size(tb::SIDEBAR_WIDTH).min_size(100.0).show(ui, |ui| {
                    sidebar(ui, d, list, icons, &mut out);
                });
            }
            egui::Panel::bottom("chat_input").show(ui, |ui| {
                input_area(ui, d, input_state, icons, &mut out);
            });
            egui::CentralPanel::default().show(ui, |ui| {
                conversation(ui, d, convo_state, icons, &mut out);
                if let Some(f) = &d.find {
                    find_bar(ui, f, &mut convo_state.find, icons, &mut out);
                }
            });
        });
        if let Some(text) = self.host.probed.take() {
            out.push(ChatEvent::FindUseSelection(text));
        }
        out
    }
}

/// The input under the conversation (`ChatInputView`): the attachments' strip, then the attach
/// button, the field (about six lines, then it scrolls) with the hint at its right, and the send
/// glyph (the stop glyph while the agent types). Enter is decided before the editor sees it.
fn input_area(ui: &mut egui::Ui, d: &ChatData, st: &mut InputUi, icons: bool, out: &mut Vec<ChatEvent>) {
    let id = egui::Id::new("chat_input_field");
    let enabled = d.chat.is_some();
    ui.add_space(6.0);
    if !d.input.pending.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for (i, name) in d.input.pending.iter().enumerate() {
                ui.label(name);
                if glyph_button(ui, icons, '\u{E711}', "x", "Remove", true, false) {
                    out.push(ChatEvent::RemoveAttachment(i));
                }
                ui.add_space(6.0);
            }
        });
    }
    if !d.input.chips.is_empty() {
        if let Some(i) = mention_picker_ui::chip_strip(ui, &d.input.chips) {
            out.push(ChatEvent::RemoveMention(i));
        }
    }
    if d.input.focus != st.focus_seen {
        st.focus_seen = d.input.focus;
        ui.memory_mut(|m| m.request_focus(id));
    }
    // The picker owns Up, Down, Return, Tab and Escape while it is open (the keys are taken before
    // the editor sees them); a list with nothing in it owns none.
    let mut picked = false;
    if enabled && ui.memory(|m| m.has_focus(id)) && d.input.picker.as_ref().is_some_and(|p| !p.items.is_empty()) {
        let none = egui::Modifiers::NONE;
        let (up, down, enter, tab, esc) = ui.input_mut(|i| (i.consume_key(none, egui::Key::ArrowUp), i.consume_key(none, egui::Key::ArrowDown), i.consume_key(none, egui::Key::Enter), i.consume_key(none, egui::Key::Tab), i.consume_key(none, egui::Key::Escape)));
        if up {
            out.push(ChatEvent::MentionMove(true));
        }
        if down {
            out.push(ChatEvent::MentionMove(false));
        }
        if enter || tab {
            out.push(ChatEvent::MentionCommit(None));
            picked = true;
        } else if esc {
            out.push(ChatEvent::MentionClose);
        }
    }
    // Enter is taken before the editor sees it; Shift+Enter (and Enter with nothing to do) stays
    // in the editor. The events go out after the text's own change, so a send sees the text.
    let mut after = Vec::new();
    if enabled && !picked && ui.memory(|m| m.has_focus(id)) {
        let (pressed, mods) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers));
        if pressed && !mods.ctrl && !mods.alt && !mods.command {
            let send = chat_input::send_enabled(&st.text, d.input.loading, d.input.stoppable);
            let action = chat_input::on_enter(mods.shift, send, d.input.stoppable);
            match action {
                Enter::Send => after.push(ChatEvent::Send),
                Enter::StopThenSend => after.extend([ChatEvent::Stop, ChatEvent::Send]),
                Enter::Stop => after.push(ChatEvent::Stop),
                Enter::Newline | Enter::Nothing => {}
            }
            if !after.is_empty() {
                ui.input_mut(|i| i.consume_key(mods, egui::Key::Enter));
            }
        }
    }
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let cap = line * 6.0 + 8.0;
    let mut edited = None;
    ui.add_enabled_ui(enabled, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let (glyph, fallback, tip, stop) = if d.input.stoppable { (chat_input::STOP_GLYPH, "Stop", "Stop", true) } else { (chat_input::SEND_GLYPH, "Send", "Send", false) };
            if glyph_button(ui, icons, glyph, fallback, tip, d.input.send_enabled, false) {
                out.push(if stop { ChatEvent::Stop } else { ChatEvent::Send });
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let attach = glyph_response(ui, icons, '\u{E723}', "+", chat_input::ATTACH_TIP, true, false);
                if attach.clicked() {
                    out.push(ChatEvent::Attach(screen_pixels(ui, attach.rect.left_bottom())));
                }
                egui::ScrollArea::vertical().id_salt("chat_input_scroll").max_height(cap).auto_shrink([false, true]).show(ui, |ui| {
                    let field = egui::TextEdit::multiline(&mut st.text).id(id).hint_text(d.input.placeholder.as_str()).desired_rows(1).desired_width(f32::INFINITY).frame(egui::Frame::NONE);
                    edited = Some(field.show(ui));
                });
            });
        });
    });
    if let Some(o) = edited {
        if o.response.changed() {
            out.push(ChatEvent::InputChanged(st.text.clone()));
        }
        if let Some(p) = &d.input.picker {
            // Focus leaving the field closes the list, except for the press that picks from it.
            if !o.response.has_focus() && !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                out.push(ChatEvent::MentionClose);
            }
            if let Some(i) = mention_picker_ui::show_above(ui, p, o.response.rect.left_top() - egui::vec2(0.0, 4.0)) {
                out.push(ChatEvent::MentionCommit(Some(i)));
            }
        }
        // The hint, at the field's right, unless the text would reach it or the agent types.
        let font = egui::FontId::proportional(12.0);
        let hint_w = ui.painter().layout_no_wrap(chat_input::HINT.to_string(), font.clone(), egui::Color32::WHITE).size().x;
        let rect = o.response.rect;
        if !d.input.stoppable && !chat_input::hint_hidden(&st.text, o.galley.rows.len(), o.galley.rect.width(), hint_w, rect.width()) {
            let weak = ui.visuals().weak_text_color();
            ui.painter().text(egui::pos2(rect.right() - 12.0, rect.top() + line * 0.5 + 2.0), egui::Align2::RIGHT_CENTER, chat_input::HINT, font, weak);
        }
    }
    out.extend(after);
    ui.add_space(6.0);
}

/// The toolbar row, left to right (`CWC:505-521`): the title, provider, model, effort, tier,
/// thinking, web search, the session button and the chat list toggle.
fn toolbar(ui: &mut egui::Ui, t: &ToolbarData, icons: bool, list_visible: &mut bool, out: &mut Vec<ChatEvent>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&t.title).size(20.0).strong());
        if !t.provider.hidden {
            let shown = t.providers.iter().find(|o| o.id() == t.provider_selected).map(option_title).unwrap_or_default();
            let r = ui.add_enabled_ui(t.provider.enabled, |ui| {
                egui::ComboBox::from_id_salt("chat_provider")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for o in &t.providers {
                            match o.id() {
                                None => {
                                    ui.separator();
                                }
                                Some(id) => {
                                    if ui.selectable_label(t.provider_selected.as_deref() == Some(id.as_str()), option_title(o)).clicked() {
                                        out.push(ChatEvent::Provider(id));
                                    }
                                }
                            }
                        }
                    })
                    .response
            });
            r.inner.on_hover_text(t.provider.tooltip).on_disabled_hover_text(t.provider.tooltip);
        }
        if !t.model.hidden {
            let shown = t.model_selected.clone().unwrap_or_default();
            let r = ui.add_enabled_ui(t.model.enabled, |ui| {
                egui::ComboBox::from_id_salt("chat_model")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for m in &t.models {
                            if ui.selectable_label(t.model_selected.as_deref() == Some(m.as_str()), m).clicked() {
                                out.push(ChatEvent::Model(m.clone()));
                            }
                        }
                    })
                    .response
            });
            r.inner.on_hover_text(t.model.tooltip).on_disabled_hover_text(t.model.tooltip);
        }
        if !t.efforts.is_empty() {
            let shown = t.efforts.iter().find(|(e, _)| Some(*e) == t.effort).map(|(_, s)| s.clone()).unwrap_or_else(|| t.efforts[0].1.clone());
            egui::ComboBox::from_id_salt("chat_effort")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (e, title) in &t.efforts {
                        if ui.selectable_label(Some(*e) == t.effort, title).clicked() {
                            out.push(ChatEvent::Effort(*e));
                        }
                    }
                })
                .response
                .on_hover_text(tb::EFFORT_TIP);
        }
        if !t.tiers.is_empty() {
            let shown = t.tiers.iter().find(|(x, _)| Some(*x) == t.tier).map(|(_, s)| s.clone()).unwrap_or_else(|| t.tiers[0].1.clone());
            egui::ComboBox::from_id_salt("chat_tier")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (x, title) in &t.tiers {
                        if ui.selectable_label(Some(*x) == t.tier, title).clicked() {
                            out.push(ChatEvent::Tier(*x));
                        }
                    }
                })
                .response
                .on_hover_text(tb::TIER_TIP);
        }
        // The right side: the glyph buttons, right to left, so the list toggle sits last.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if glyph_button(ui, icons, tb::LIST_GLYPH, "List", tb::list_toggle_tip(*list_visible), true, false) {
                *list_visible = !*list_visible;
            }
            let r = glyph_response(ui, icons, tb::INFO_GLYPH, "Info", tb::SESSION_TIP, true, false);
            if r.clicked() {
                out.push(ChatEvent::SessionMenu(screen_pixels(ui, r.rect.left_bottom())));
            }
            if glyph_button(ui, icons, tb::WEB_GLYPH, "Web", tb::WEB_SEARCH_TIP, t.web_search_enabled, t.web_search && t.web_search_enabled) {
                out.push(ChatEvent::ToggleWebSearch);
            }
            if let Some(on) = t.thinking {
                if glyph_button(ui, icons, tb::THINKING_GLYPH, "Think", tb::THINKING_TIP, true, on) {
                    out.push(ChatEvent::ToggleThinking);
                }
            }
        });
    });
}

fn option_title(o: &ProviderOption) -> String {
    match o {
        ProviderOption::Vendor { title, .. } => title.clone(),
        ProviderOption::Manual { name } => name.clone(),
        ProviderOption::Separator => String::new(),
    }
}

fn glyph_response(ui: &mut egui::Ui, icons: bool, glyph: char, fallback: &str, tip: &str, enabled: bool, on: bool) -> egui::Response {
    let mut text = if icons { egui::RichText::new(glyph.to_string()).family(egui::FontFamily::Name("icons".into())).size(16.0) } else { egui::RichText::new(fallback) };
    if on {
        text = text.color(ui.visuals().hyperlink_color);
    }
    ui.add_enabled(enabled, egui::Button::new(text).frame(false)).on_hover_text(tip).on_disabled_hover_text(tip)
}

/// A glyph toggle (its name when the icon font is missing), tinted with the accent color when on.
fn glyph_button(ui: &mut egui::Ui, icons: bool, glyph: char, fallback: &str, tip: &str, enabled: bool, on: bool) -> bool {
    glyph_response(ui, icons, glyph, fallback, tip, enabled, on).clicked()
}

/// The find bar (`ChatFindBarView`): a floating box at the top of the conversation, at most
/// `chat_find::MAX_WIDTH` wide and 8 points down, with the field, the counter, previous and next,
/// the mode popup and a close glyph. Enter is Find Next (Shift+Enter Previous), Escape closes.
fn find_bar(ui: &mut egui::Ui, f: &FindData, st: &mut FindUi, icons: bool, out: &mut Vec<ChatEvent>) {
    // The field follows the app when it changed the query (a selection used for find).
    if st.seen_query != f.query {
        st.seen_query = f.query.clone();
        st.text = f.query.clone();
    }
    let area = ui.max_rect();
    let width = (area.width() - 16.0).clamp(160.0, chat_find::MAX_WIDTH);
    let pos = egui::pos2(area.center().x - width / 2.0, area.top() + 8.0);
    let focus_now = st.seen_focus != f.focus;
    st.seen_focus = f.focus;
    egui::Area::new(egui::Id::new("chat_find_bar")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.horizontal(|ui| {
                let field_id = egui::Id::new("chat_find_field");
                let mut mode = f.mode_index;
                let controls = 3.0 * 24.0 + 130.0 + 70.0;
                let field = ui.add(egui::TextEdit::singleline(&mut st.text).id(field_id).hint_text(chat_find::PLACEHOLDER).desired_width((ui.available_width() - controls).max(60.0)));
                if focus_now {
                    field.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field_id) {
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(st.text.chars().count()))));
                        state.store(ui.ctx(), field_id);
                    }
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                let mut query_changed = field.changed();
                if field.lost_focus() && enter {
                    out.push(if ui.input(|i| i.modifiers.shift) { ChatEvent::FindPrevious } else { ChatEvent::FindNext });
                    field.request_focus();
                }
                if (field.has_focus() || field.lost_focus()) && escape {
                    out.push(ChatEvent::FindClose);
                }
                ui.label(egui::RichText::new(&f.counter).size(12.0).color(ui.visuals().weak_text_color()));
                let steppable = !f.matches.is_empty();
                if glyph_button(ui, icons, '\u{E70E}', "Prev", "Find Previous", steppable, false) {
                    out.push(ChatEvent::FindPrevious);
                }
                if glyph_button(ui, icons, '\u{E70D}', "Next", "Find Next", steppable, false) {
                    out.push(ChatEvent::FindNext);
                }
                egui::ComboBox::from_id_salt("chat_find_mode").selected_text(chat_find::MODES[mode.min(chat_find::MODES.len() - 1)].0).width(120.0).show_ui(ui, |ui| {
                    for (i, (title, _)) in chat_find::MODES.iter().enumerate() {
                        if ui.selectable_label(mode == i, *title).clicked() {
                            mode = i;
                            query_changed = true;
                        }
                    }
                });
                if query_changed {
                    out.push(ChatEvent::FindQuery(st.text.clone(), mode));
                }
                if glyph_button(ui, icons, '\u{E711}', "Close", "Close", true, false) {
                    out.push(ChatEvent::FindClose);
                }
            });
        });
    });
}

/// A point of the window in screen pixels (where a native popup menu opens).
fn screen_pixels(ui: &egui::Ui, p: egui::Pos2) -> (f32, f32) {
    let scale = ui.ctx().pixels_per_point();
    let origin = ui.ctx().input(|i| i.viewport().inner_rect).map_or(egui::Pos2::ZERO, |r| r.min);
    ((origin.x + p.x) * scale, (origin.y + p.y) * scale)
}

const ROW_HEIGHT: f32 = 56.0;
/// The search field's hint (`CL:509-514`).
const SEARCH_HINT: &str = "Search";
/// The most a stored icon may cost to decode (all frames): it is drawn 32 points wide.
const ICON_BUDGET: usize = 8 << 20;

/// The chat list: the header (the search field and the new chat button), then the
/// rows in a scroll area (`ChatListViewController`). Keys count only while the list has focus.
fn sidebar(ui: &mut egui::Ui, d: &ChatData, st: &mut ListState, icons: bool, out: &mut Vec<ChatEvent>) {
    // The list follows the shown chat when something else changed it.
    if d.chat != st.last_shown {
        st.last_shown = d.chat.clone();
        if let Some(c) = &d.chat {
            if st.selection.first() != Some(c.as_str()) {
                st.selection.set(&d.rows, c);
            }
        }
    }
    st.selection.retain(&d.rows);
    // The app clears the search when another chat is chosen (`CL:315-322`).
    if d.query != st.seen_query {
        st.seen_query = d.query.clone();
        st.query = d.query.clone();
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if glyph_button(ui, icons, chat_list::NEW_CHAT_GLYPH, "+", tb::NEW_CHAT_TIP, true, false) {
                out.push(ChatEvent::NewChat);
            }
            let field = egui::TextEdit::singleline(&mut st.query).hint_text(SEARCH_HINT).desired_width(f32::INFINITY);
            if ui.add(field).changed() {
                out.push(ChatEvent::Search(st.query.clone()));
            }
        });
    });
    ui.separator();
    if !st.query.is_empty() {
        results(ui, d, out);
        return;
    }
    let list_id = egui::Id::new("chat_list_focus");
    // Present every frame, or the focus is dropped; senses no click, so rows keep theirs.
    ui.interact(ui.available_rect_before_wrap(), list_id, egui::Sense::focusable_noninteractive());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, row) in d.rows.iter().enumerate() {
            let selected = st.selection.ids.contains(&row.id);
            let resp = draw_row(ui, row, selected, icons, &mut st.textures);
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                st.selection.click(&d.rows, i, m.ctrl || m.command, m.shift);
                ui.memory_mut(|mem| mem.request_focus(list_id));
            }
            if resp.secondary_clicked() {
                st.selection.right_click(&d.rows, i);
                ui.memory_mut(|mem| mem.request_focus(list_id));
                let at = resp.interact_pointer_pos().unwrap_or(resp.rect.center());
                out.push(ChatEvent::ListMenu { at: screen_pixels(ui, at), ids: st.selection.ids.clone() });
            }
        }
    });
    // The list's selection shows its first chat (`CL:490-506`).
    if let Some(first) = st.selection.first().map(str::to_string) {
        if st.last_shown.as_deref() != Some(first.as_str()) {
            st.last_shown = Some(first.clone());
            out.push(ChatEvent::Select(first));
        }
    }
    // Delete and Forward Delete delete; Return and keypad Enter rename (`CL:47-58`).
    if ui.memory(|m| m.has_focus(list_id)) && !st.selection.ids.is_empty() {
        let (delete, rename) = ui.input(|i| (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Enter)));
        if delete {
            out.push(ChatEvent::Delete(st.selection.ids.clone()));
        } else if rename {
            if let Some(first) = st.selection.first() {
                out.push(ChatEvent::Rename(first.to_string()));
            }
        }
    }
}

/// The search results in the list's place: small bubbles, the person's in the accent color at
/// the right, the agent's gray at the left, each snippet with its matches strong (`CSR`).
fn results(ui: &mut egui::Ui, d: &ChatData, out: &mut Vec<ChatEvent>) {
    let visuals = ui.visuals().clone();
    let width = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, hit) in d.search.iter().enumerate() {
            let (fill, text, strong) = if hit.user {
                (visuals.selection.bg_fill, egui::Color32::WHITE, egui::Color32::WHITE)
            } else {
                (visuals.widgets.noninteractive.bg_fill, visuals.text_color(), visuals.strong_text_color())
            };
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = (width * 0.85 - 16.0).max(20.0);
            for (piece, bold) in chat_search::bold_runs(&hit.snippet) {
                let mut f = egui::TextFormat::simple(egui::FontId::proportional(12.0), if bold { strong } else { text });
                if bold {
                    f.underline = egui::Stroke::new(1.0, strong);
                }
                job.append(&piece, 0.0, f);
            }
            let galley = ui.painter().layout_job(job);
            let size = galley.size() + egui::vec2(16.0, 10.0);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, size.y + 4.0), egui::Sense::click());
            let x = if hit.user { rect.right() - size.x - 4.0 } else { rect.left() + 4.0 };
            let bubble = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), size);
            ui.painter().rect_filled(bubble, 10.0, if resp.hovered() { fill.gamma_multiply(0.85) } else { fill });
            ui.painter().galley(bubble.min + egui::vec2(8.0, 5.0), galley, text);
            if resp.clicked() {
                out.push(ChatEvent::ChooseHit(i));
            }
        }
    });
}

/// One row (`ChatCellView`): the round icon, the title (with the orchestrator glyph), the
/// snippet on one line and the age at the right of the title line.
fn draw_row(ui: &mut egui::Ui, row: &Row, selected: bool, icons: bool, textures: &mut HashMap<u64, Option<egui::TextureHandle>>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 4.0, ui.visuals().selection.bg_fill);
    } else if resp.hovered() {
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon_rect = egui::Rect::from_min_size(rect.min + egui::vec2(8.0, (ROW_HEIGHT - chat_list::ICON_SIZE) / 2.0), egui::Vec2::splat(chat_list::ICON_SIZE));
    let picture = row.icon.as_deref().and_then(|bytes| icon_texture(ui.ctx(), textures, bytes));
    match picture {
        Some(tex) => egui::Image::new((tex.id(), icon_rect.size())).corner_radius(chat_list::ICON_SIZE / 2.0).paint_at(ui, icon_rect),
        None => {
            painter.circle_filled(icon_rect.center(), chat_list::ICON_SIZE / 2.0, egui::Color32::from_gray(140));
            if icons {
                painter.text(icon_rect.center(), egui::Align2::CENTER_CENTER, chat_list::DEFAULT_ICON_GLYPH.to_string(), egui::FontId::new(chat_list::ICON_SIZE * 0.45, egui::FontFamily::Name("icons".into())), egui::Color32::WHITE);
            }
        }
    }
    let left = icon_rect.right() + 8.0;
    let weak = ui.visuals().weak_text_color();
    let age = painter.layout_no_wrap(row.age.clone(), egui::FontId::proportional(11.0), weak);
    let age_at = egui::pos2(rect.right() - 8.0 - age.size().x, rect.top() + 9.0);
    painter.galley(age_at, age, weak);
    let title_clip = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2((age_at.x - 6.0).max(left), rect.top() + ROW_HEIGHT / 2.0 + 2.0));
    let title_painter = painter.with_clip_rect(title_clip.intersect(rect));
    let mut x = left;
    if row.orchestrator && icons {
        let r = title_painter.text(egui::pos2(x, rect.top() + 8.0), egui::Align2::LEFT_TOP, chat_list::ORCHESTRATOR_GLYPH.to_string(), egui::FontId::new(13.0, egui::FontFamily::Name("icons".into())), ui.visuals().strong_text_color());
        x = r.right() + 4.0;
    }
    title_painter.text(egui::pos2(x, rect.top() + 8.0), egui::Align2::LEFT_TOP, &row.title, egui::FontId::proportional(13.0), ui.visuals().strong_text_color());
    let snippet_clip = egui::Rect::from_min_max(egui::pos2(left, rect.top() + ROW_HEIGHT / 2.0), egui::pos2(rect.right() - 8.0, rect.bottom()));
    painter.with_clip_rect(snippet_clip.intersect(rect)).text(egui::pos2(left, rect.top() + 30.0), egui::Align2::LEFT_TOP, &row.snippet, egui::FontId::proportional(12.0), weak);
    resp
}

/// The texture for a stored icon, decoded once per distinct bytes (ruling 12d-2).
fn icon_texture<'a>(ctx: &egui::Context, textures: &'a mut HashMap<u64, Option<egui::TextureHandle>>, bytes: &[u8]) -> Option<&'a egui::TextureHandle> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let key = h.finish();
    textures
        .entry(key)
        .or_insert_with(|| {
            let decoded = chat_list::icon_fits(bytes).then(|| crate::images::decode_within(bytes, ICON_BUDGET).ok()).flatten()?;
            let frame = decoded.frames.first()?;
            let image = egui::ColorImage::from_rgba_unmultiplied([decoded.width as usize, decoded.height as usize], &frame.rgba);
            Some(ctx.load_texture(format!("chat-icon-{key:x}"), image, egui::TextureOptions::LINEAR))
        })
        .as_ref()
}

/// The conversation's rows for a chat's messages (`ChatViewControllerModel` and the cells'
/// own texts): dates, visible messages, the typing row.
pub fn convo_rows(messages: &[Message], typing: bool, day: &dyn Fn(f64) -> (i32, u32, u32), label: &dyn Fn(f64) -> String, time: &dyn Fn(f64) -> String, for_message: &dyn Fn(&Message, bool) -> (Vec<Button>, bool)) -> Vec<RowData> {
    let visible = chat_view::visible(messages);
    let last = visible.last().map(|m| m.unique_id.clone());
    chat_view::items(&visible, typing, day, label)
        .into_iter()
        .map(|item| match item {
            Item::Message(i) => {
                let m = visible[i];
                let body = chat_view::body(&m.content);
                let (buttons, enabled) = for_message(m, last.as_deref() == Some(m.unique_id.as_str()));
                RowData {
                    buttons,
                    enabled,
                    item,
                    id: m.unique_id.clone(),
                    user: m.author == Participant::User,
                    style: chat_view::style(&m.content),
                    time: time(m.sent_date),
                    editable: chat_view::editable(m),
                    glyph: chat_view::glyph(&m.content),
                    key: chat_view::body_key(&m.unique_id, &body),
                    in_app: InApp::of(m),
                    body,
                }
            }
            marker => RowData::marker(marker),
        })
        .collect()
}

/// How a button under a message draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// Can be pressed.
    Live,
    /// Greyed: the message's buttons are not enabled, or were pressed a moment ago.
    Off,
    /// Greyed with the "comes in milestone 12f" tooltip.
    Later,
}

/// The look of `b` on `row` (`pressed` holds the messages pressed and awaiting the app).
fn button_look(row: &RowData, pressed: &std::collections::HashSet<String>, b: &Button) -> Look {
    if chat_buttons::is_orchestration(&b.id) {
        Look::Later
    } else if row.enabled && !pressed.contains(&row.id) {
        Look::Live
    } else {
        Look::Off
    }
}

/// Whether a press leaves the row's buttons live: the permissions bubble is tapped again and
/// again (`CVC:2770-2778`); every other bubble greys at once.
pub fn stays_live(row: &RowData) -> bool {
    !row.buttons.is_empty() && row.buttons.iter().all(|b| crate::ai_permissions::Category::parse(&b.id).is_some())
}

/// Forgets pressed messages whose buttons the app has since disabled or removed.
fn prune_pressed(pressed: &mut std::collections::HashSet<String>, rows: &[RowData]) {
    pressed.retain(|id| rows.iter().any(|r| r.id == *id && r.enabled));
}

/// The colors a row draws with.
struct Ink {
    text: egui::Color32,
    strong: egui::Color32,
    link: egui::Color32,
    muted: egui::Color32,
    code_bg: egui::Color32,
    warn: egui::Color32,
}

fn ink_for(v: &egui::Visuals, row: &RowData) -> Ink {
    let base = |text, strong, link| Ink { text, strong, link, muted: v.weak_text_color(), code_bg: egui::Color32::from_black_alpha(48), warn: v.warn_fg_color };
    if row.style == Style::System {
        let m = v.weak_text_color();
        Ink { text: m, strong: v.text_color(), link: v.hyperlink_color, ..base(m, m, m) }
    } else if row.user {
        // User text and links are white on the accent (`CVC:2760-2765`).
        Ink { muted: egui::Color32::from_white_alpha(190), ..base(egui::Color32::WHITE, egui::Color32::WHITE, egui::Color32::WHITE) }
    } else {
        base(v.text_color(), v.strong_text_color(), v.hyperlink_color)
    }
}

const BODY_SIZE: f32 = 14.0;
/// A button row's height in points (`CVC`'s 30-point buttons).
const BUTTON_HEIGHT: f32 = 30.0;
/// One terminal cell of a block's indent, in points.
const INDENT_STEP: f32 = 9.0;

/// The conversation in the central panel: it scrolls, follows the newest message while the
/// person has not scrolled up, and each chat keeps its own position.
fn conversation(ui: &mut egui::Ui, d: &ChatData, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) {
    st.taken = false;
    st.match_scrolled = false;
    prune_pressed(&mut st.pressed, &d.convo);
    if st.reveal.as_ref().is_some_and(|(chat, _)| *chat != d.chat) {
        st.reveal = None;
    }
    if st.docs.len() > 500 {
        st.docs.clear();
    }
    let dark = ui.visuals().dark_mode;
    let mut found = false;
    let find = d.find.as_ref();
    if let Some(cur) = find.and_then(|f| f.current.and_then(|i| f.matches.get(i))) {
        // A different current match scrolls its row into view (`CVC`'s reveal); the same one after a rescan does not.
        let id = (cur.message.clone(), cur.segment, cur.range.clone());
        if st.find_seen.as_ref() != Some(&id) {
            st.reveal = Some((d.chat.clone(), cur.message.clone()));
            st.match_due = true;
            st.find_seen = Some(id);
        }
    } else {
        st.find_seen = None;
        st.match_due = false;
    }
    egui::ScrollArea::vertical().id_salt(egui::Id::new(("chat_convo", &d.chat))).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.add_space(6.0);
        for (index, row) in d.convo.iter().enumerate() {
            st.row = row_find(find, index, &row.id);
            match &row.item {
                Item::Date(label) => {
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new(label).size(12.0).color(ui.visuals().weak_text_color())));
                    ui.add_space(4.0);
                }
                Item::Typing => {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.add(egui::Spinner::new().size(14.0));
                    });
                }
                Item::Message(_) => {
                    let rect = message_row(ui, row, st, icons, dark, out);
                    if st.reveal.as_ref().is_some_and(|(_, m)| *m == row.id) {
                        // The match itself is brought into view where its text was drawn (`CVC:3099-3101`);
                        // the row is centered only for a reveal that has no match (a search result).
                        if !st.match_scrolled {
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        }
                        found = true;
                        st.match_due = false;
                    }
                    ui.add_space(6.0);
                }
            }
        }
    });
    // Asked for once: the row is centered when found; a row that is not there is not waited for.
    if found || (st.reveal.is_some() && !d.convo.is_empty()) {
        st.reveal = None;
    }
}

/// One message: a bubble (the person's at the right, the agent's at the left) with its short
/// time beside it, or a full-width muted line for a system row. Answers the row's rectangle.
/// The find matches of one row (its message's), sliced out of the ordered list.
fn row_find(find: Option<&FindData>, item: usize, message: &str) -> RowFind {
    let Some(f) = find else { return RowFind::default() };
    let from = f.matches.partition_point(|m| m.item < item);
    let to = f.matches.partition_point(|m| m.item <= item);
    let matches: Vec<Match> = f.matches[from..to].iter().filter(|m| m.message == message).cloned().collect();
    let current = f.current.filter(|c| (from..to).contains(c)).and_then(|c| matches.iter().position(|m| *m == f.matches[c]));
    RowFind { item, segment: 0, open: true, matches, current }
}

fn message_row(ui: &mut egui::Ui, row: &RowData, st: &mut ConvoState, icons: bool, dark: bool, out: &mut Vec<ChatEvent>) -> egui::Rect {
    let ink = ink_for(ui.visuals(), row);
    let rect = if row.style == Style::System {
        ui.horizontal_top(|ui| {
            ui.add_space(8.0);
            row_content(ui, row, &ink, st, icons, dark, out);
        })
        .response
        .rect
    } else {
        let max_inner = (ui.available_width() * chat_view::MAX_BUBBLE - 24.0).max(60.0);
        let fill = if row.user { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.inactive.weak_bg_fill };
        let layout = if row.user { egui::Layout::right_to_left(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
        ui.with_layout(layout, |ui| {
            ui.add_space(8.0);
            let bubble = egui::Frame::new().fill(fill).corner_radius(10.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_max_width(max_inner);
                row_content(ui, row, &ink, st, icons, dark, out);
            });
            ui.label(egui::RichText::new(&row.time).size(11.0).color(ui.visuals().weak_text_color()));
            bubble.response.rect
        })
        .inner
    };
    // Right click on the message: its menu, unless a file or code item inside took the click.
    if !st.taken && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary)) && ui.rect_contains_pointer(rect) {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            out.push(ChatEvent::MessageMenu { at: screen_pixels(ui, p), id: row.id.clone() });
        }
    }
    rect
}

/// A row's glyph, then its body.
fn row_content(ui: &mut egui::Ui, row: &RowData, ink: &Ink, st: &mut ConvoState, icons: bool, dark: bool, out: &mut Vec<ChatEvent>) {
    ui.horizontal_top(|ui| {
        if let Some(g) = row.glyph {
            let color = if g == chat_view::WARNING_GLYPH { ink.warn } else { ink.muted };
            if icons {
                ui.label(egui::RichText::new(g.to_string()).family(egui::FontFamily::Name("icons".into())).size(15.0).color(color));
            } else if let Some(word) = glyph_fallback(g) {
                ui.label(egui::RichText::new(word).size(BODY_SIZE).color(color));
            }
        }
        ui.vertical(|ui| {
            draw_body(ui, &row.body, row, 0, ink, st, icons, dark, out);
            draw_buttons(ui, row, st, out);
        });
    });
}

/// The buttons under a bubble's text: full-width rows of 30 points with a separator above each,
/// destructive ones in the error color, greyed unless the row is enabled and not just pressed.
/// A press is reported at once and greys the row until the app answers (iTerm2's optimistic
/// disable), except in the permissions bubble, which is tapped again and again. The
/// orchestration ones show with a tooltip that they wait for milestone 12f.
fn draw_buttons(ui: &mut egui::Ui, row: &RowData, st: &mut ConvoState, out: &mut Vec<ChatEvent>) {
    if row.buttons.is_empty() {
        return;
    }
    let error = ui.visuals().error_fg_color;
    for b in &row.buttons {
        ui.separator();
        let look = button_look(row, &st.pressed, b);
        let mut label = egui::RichText::new(&b.title).size(BODY_SIZE);
        if b.destructive {
            label = label.color(error);
        }
        let width = ui.available_width();
        let mut r = ui.add_enabled(look == Look::Live, egui::Button::new(label).frame(false).min_size(egui::vec2(width, BUTTON_HEIGHT)));
        if look == Look::Later {
            r = r.on_disabled_hover_text(chat_buttons::ORCHESTRATION_LATER);
        }
        if look == Look::Live && r.clicked() {
            out.push(ChatEvent::Button { message: row.id.clone(), button: b.id.clone() });
            if !stays_live(row) {
                st.pressed.insert(row.id.clone());
            }
        }
    }
}

fn draw_body(ui: &mut egui::Ui, body: &Body, row: &RowData, part: usize, ink: &Ink, st: &mut ConvoState, icons: bool, dark: bool, out: &mut Vec<ChatEvent>) {
    match body {
        Body::Plain(t) => {
            leaf(ui, t, Face::Prose, ink, row, st, icons, out);
        }
        Body::Markdown(md) => markdown(ui, md, row, part, ink, st, icons, dark, out),
        Body::Command { command, url } => {
            ui.horizontal(|ui| {
                if icons {
                    ui.label(egui::RichText::new(chat_view::COMMAND_GLYPH.to_string()).family(egui::FontFamily::Name("icons".into())).size(15.0).color(ink.muted));
                }
                let r = leaf(ui, command, Face::Code, ink, row, st, icons, out);
                if r.clicked() {
                    out.push(ChatEvent::RevealCommand(url.clone()));
                }
            });
        }
        Body::Parts(parts) => {
            for (i, p) in parts.iter().enumerate() {
                match p {
                    Part::Text(b) => draw_body(ui, b, row, i + 1, ink, st, icons, dark, out),
                    Part::Status(s) => {
                        let muted = Ink { text: ink.muted, strong: ink.muted, link: ink.link, muted: ink.muted, code_bg: ink.code_bg, warn: ink.warn };
                        markdown(ui, s, row, i + 1, &muted, st, icons, dark, out);
                    }
                    Part::Code(code) => code_attachment(ui, code, row, i, ink, st, icons, out),
                    Part::File { id, name } => file_item(ui, name, Some(chat_view::file_ref(&row.id, id)), row, ink, st, icons, out),
                    Part::RemoteFile { name } => file_item(ui, name, None, row, ink, st, icons, out),
                }
            }
        }
    }
}

/// A code attachment: a box titled "Code Interpreter" with its own copy button
/// (`MultipartMessageCellView.swift:387-405`).
fn code_attachment(ui: &mut egui::Ui, code: &str, row: &RowData, part: usize, ink: &Ink, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) {
    let frame = egui::Frame::new().fill(ink.code_bg).corner_radius(6.0).inner_margin(egui::Margin::same(6));
    let shown = frame.show(ui, |ui| {
        ui.label(egui::RichText::new(chat_view::CODE_INTERPRETER).size(11.0).strong().color(ink.muted));
        leaf(ui, code, Face::Code, ink, row, st, icons, out);
    });
    let rect = shown.response.rect;
    copy_button(ui, rect, &row.id, usize::MAX - part, code, ink, st, icons, out);
}

/// The "Copied" slot of a code block: its markdown part and its first block, so two parts of one
/// message that both start with a code block never share one.
fn copy_slot(part: usize, first: usize) -> usize {
    // Parts number from 0 and a code attachment's slot counts down from usize::MAX.
    (part + 1) * 1_000_000 + first
}

/// The text drawn for a row's glyph when the icon font is missing: a flagged request must never
/// look like a safe one, so the warning glyph has a word, the others none.
fn glyph_fallback(g: char) -> Option<&'static str> {
    (g == chat_view::WARNING_GLYPH).then_some("Warning")
}

/// The copy glyph in the top right corner of `rect`, and "Copied" beside it for a second.
#[allow(clippy::too_many_arguments)]
fn copy_button(ui: &mut egui::Ui, rect: egui::Rect, message: &str, slot: usize, text: &str, ink: &Ink, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) {
    let button = egui::Rect::from_min_size(egui::pos2(rect.right() - 24.0, rect.top() + 2.0), egui::vec2(22.0, 20.0));
    let id = ui.id().with(("copy", message, slot));
    let resp = ui.interact(button, id, egui::Sense::click());
    let color = if resp.hovered() { ink.strong } else { ink.muted };
    let painter = ui.painter();
    if icons {
        painter.text(button.center(), egui::Align2::CENTER_CENTER, chat_view::COPY_GLYPH.to_string(), egui::FontId::new(14.0, egui::FontFamily::Name("icons".into())), color);
    } else {
        painter.text(button.center(), egui::Align2::CENTER_CENTER, "Copy", egui::FontId::proportional(11.0), color);
    }
    if st.copied.as_ref().is_some_and(|(m, s, at)| m == message && *s == slot && at.elapsed() < chat_view::COPIED_FOR) {
        painter.text(egui::pos2(button.left() - 4.0, button.center().y), egui::Align2::RIGHT_CENTER, chat_view::COPIED, egui::FontId::proportional(11.0), ink.muted);
    }
    if resp.on_hover_text("Copy").clicked() {
        st.copied = Some((message.to_string(), slot, Instant::now()));
        st.taken = true;
        out.push(ChatEvent::CopyText(text.to_string()));
    }
}

/// A file part: its glyph and name; right-click offers Reveal in Explorer, and Open only for an
/// inert type (ruling F19). A `fileID` attachment (`reference` None) has no bytes to write, so
/// its items are disabled.
#[allow(clippy::too_many_arguments)]
fn file_item(ui: &mut egui::Ui, name: &str, reference: Option<String>, _row: &RowData, ink: &Ink, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) {
    let resp = ui
        .horizontal(|ui| {
            if icons {
                ui.label(egui::RichText::new(chat_view::FILE_GLYPH.to_string()).family(egui::FontFamily::Name("icons".into())).size(15.0).color(ink.muted));
            }
            ui.add(egui::Label::new(egui::RichText::new(name).color(ink.text)).sense(egui::Sense::click()))
        })
        .inner;
    if resp.secondary_clicked() {
        st.taken = true;
    }
    let usable = reference.is_some() && chat_view::attachment_name(name).is_some();
    resp.context_menu(|ui| {
        if ui.add_enabled(usable, egui::Button::new(chat_view::REVEAL_TITLE)).clicked() {
            if let Some(r) = &reference {
                out.push(ChatEvent::RevealFile(r.clone()));
            }
            ui.close();
        }
        if chat_view::openable(name) && ui.add_enabled(usable, egui::Button::new(chat_view::OPEN_TITLE)).clicked() {
            if let Some(r) = &reference {
                out.push(ChatEvent::OpenFile(r.clone()));
            }
            ui.close();
        }
    });
}

/// A markdown body through `porthole::render`, block by block. Links are drawn as links only
/// when `chat_view::web_link` accepts them; a click posts `OpenUrl`, which the app checks again.
#[allow(clippy::too_many_arguments)]
fn markdown(ui: &mut egui::Ui, md: &str, row: &RowData, part: usize, ink: &Ink, st: &mut ConvoState, icons: bool, dark: bool, out: &mut Vec<ChatEvent>) {
    let key = (row.key, part, dark);
    let rendered = match st.docs.get(&key) {
        Some(r) => r.clone(),
        None => {
            let c = |c: egui::Color32| rgb(c.r(), c.g(), c.b());
            let palette = Palette { fg: c(ink.text), link: c(ink.link), ..Palette::default() };
            let doc = crate::porthole::render::render(md, &Language::Markdown, &palette, dark);
            let blocks = chat_view::code_blocks(&doc);
            let spans = chat_find::block_spans(&doc);
            let r = Rc::new(Rendered { doc, blocks, spans });
            st.docs.insert(key, r.clone());
            r
        }
    };
    let segment = st.row.claim();
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    for (i, block) in rendered.doc.blocks.iter().enumerate() {
        if block.rule {
            ui.add_space(3.0);
            ui.separator();
            continue;
        }
        if block.runs.is_empty() && !block.code_bg {
            ui.add_space(6.0);
            continue;
        }
        let copy = rendered.blocks.iter().find(|b| b.first == i);
        draw_block(ui, block, rendered.spans[i].start, segment, ink, copy, row, part, st, icons, out);
    }
    ui.spacing_mut().item_spacing.y = spacing;
}

/// A plain text (a message, a command, a code attachment) drawn as one block in its own face,
/// so it wraps, highlights and takes a click for find like markdown does.
#[allow(clippy::too_many_arguments)]
fn leaf(ui: &mut egui::Ui, text: &str, face: Face, ink: &Ink, row: &RowData, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) -> egui::Response {
    let segment = st.row.claim();
    let run = crate::porthole::render::Run { text: text.to_string(), color: rgb(ink.text.r(), ink.text.g(), ink.text.b()), bold: false, italic: false, underline: false, face, link: None };
    let block = Block { runs: vec![run], scale: 1.0, indent: 0.0, code_bg: false, rule: false };
    draw_block(ui, &block, 0, segment, ink, None, row, 0, st, icons, out)
}

/// One block: its runs as a wrapped text, code lines on a shaded band, a copy glyph at the
/// first line of a code block.
#[allow(clippy::too_many_arguments)]
fn draw_block(ui: &mut egui::Ui, block: &Block, base: usize, segment: usize, ink: &Ink, copy: Option<&CodeBlock>, row: &RowData, part: usize, st: &mut ConvoState, icons: bool, out: &mut Vec<ChatEvent>) -> egui::Response {
    let indent = block.indent * INDENT_STEP;
    let len: usize = block.runs.iter().map(|r| r.text.len()).sum();
    let marks = if st.row.open { chat_find::marks(&st.row.matches, st.row.current, &row.id, segment, &(base..base + len)) } else { Vec::new() };
    let (job, links) = block_job(block, ink, (ui.available_width() - indent).max(20.0), &marks, row.in_app);
    let galley = ui.painter().layout_job(job);
    let width = if block.code_bg { ui.available_width() } else { indent + galley.size().x };
    let height = galley.size().y.max(if block.code_bg { BODY_SIZE + 2.0 } else { 0.0 });
    // Every block can be selected and clicked (a click moves the find position).
    let mut sense = egui::Sense::click_and_drag();
    sense -= egui::Sense::FOCUSABLE;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    if block.code_bg {
        ui.painter().rect_filled(rect, 0.0, ink.code_bg);
    }
    let origin = rect.min + egui::vec2(indent + if block.code_bg { 4.0 } else { 0.0 }, 0.0);
    if st.match_due {
        if let Some((range, _)) = marks.iter().find(|(_, current)| *current) {
            let text: String = block.runs.iter().map(|r| r.text.as_str()).collect();
            ui.scroll_to_rect(match_rect(&galley, &text, range.clone(), origin), None);
            st.match_due = false;
            st.match_scrolled = true;
        }
    }
    egui::text_selection::LabelSelectionState::label_text_selection(ui, &resp, origin, galley.clone(), ink.text, egui::Stroke::NONE);
    if st.row.open && resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let chars = galley.cursor_from_pos(p - origin).index.0;
            let text: String = block.runs.iter().map(|r| r.text.as_str()).collect();
            let byte = text.char_indices().nth(chars).map_or(text.len(), |(b, _)| b);
            out.push(ChatEvent::FindClick(Cursor { item: st.row.item, segment, at: base + byte }));
        }
    }
    if !links.is_empty() {
        let at = resp.hover_pos().or_else(|| resp.interact_pointer_pos());
        let url = at.and_then(|p| {
            let index = galley.cursor_from_pos(p - origin).index;
            links.iter().find(|(from, to, _)| (*from..*to).contains(&index.0)).map(|(_, _, u)| u.clone())
        });
        if let (Some(u), true) = (&url, resp.hovered()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            // The text of a link can differ from its target; the target shows while pointing.
            resp.clone().on_hover_text(u);
        }
        if resp.clicked() {
            if let Some(u) = url {
                if row.in_app.route(&u).is_some() {
                    out.push(ChatEvent::RevealInApp { message: row.id.clone(), url: u });
                } else {
                    out.push(ChatEvent::OpenUrl(u));
                }
            }
        }
    }
    if let Some(cb) = copy {
        copy_button(ui, rect, &row.id, copy_slot(part, cb.first), &cb.text, ink, st, icons, out);
    }
    resp
}

/// The rectangle of a match inside a laid-out block, from its first character to its last, in
/// the block's own coordinates shifted by `origin` (`scrollMatchRangeToVisible`, `CVC:3099-3101`).
/// `range` is in bytes of the block's text.
fn match_rect(galley: &egui::Galley, text: &str, range: std::ops::Range<usize>, origin: egui::Pos2) -> egui::Rect {
    let at = |byte: usize| egui::text::CCursor::new(text[..byte.min(text.len())].chars().count());
    let (from, to) = (galley.pos_from_cursor(at(range.start)), galley.pos_from_cursor(at(range.end)));
    from.union(to).translate(origin.to_vec2())
}

/// The find highlight (the system's find yellow): the current match solid, the others at
/// `HIGHLIGHT_ALPHA`.
fn find_color(current: bool) -> egui::Color32 {
    let yellow = egui::Color32::from_rgb(255, 230, 0);
    if current { yellow } else { yellow.gamma_multiply(chat_find::HIGHLIGHT_ALPHA) }
}

/// A block's runs as one text job, and the character ranges that are links. Runs are cut at the
/// edges of the find `marks` (bytes into the block's text) and the marked pieces get the
/// highlight as a background.
fn block_job(block: &Block, ink: &Ink, wrap: f32, marks: &[Mark], in_app: InApp) -> (egui::text::LayoutJob, Vec<(usize, usize, String)>) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap;
    let (mut at, mut links, mut byte) = (0usize, Vec::new(), 0usize);
    let size = BODY_SIZE * block.scale;
    for run in &block.runs {
        // An explanation's annotation links and its request's attached-content link are the only
        // `iterm2:` links a bubble draws (ruling P9, final ruling).
        let link = run.link.as_deref().and_then(|u| chat_view::web_link(u).or_else(|| in_app.route(u).map(|_| u.to_string())));
        let font = match run.face {
            Face::Prose => egui::FontId::proportional(size),
            Face::Code => egui::FontId::monospace(size * 0.93),
        };
        // Code lines keep their highlight colors; egui has no bold face, so bold is the strong color.
        let color = if link.is_some() {
            ink.link
        } else if block.code_bg {
            egui::Color32::from_rgb(run.color.r, run.color.g, run.color.b)
        } else if run.bold || block.scale > 1.0 {
            ink.strong
        } else {
            ink.text
        };
        let mut format = egui::TextFormat::simple(font, color);
        format.italics = run.italic;
        if link.is_some() {
            format.underline = egui::Stroke::new(1.0, color);
        }
        for (piece, mark) in chat_find::cut_run(byte..byte + run.text.len(), marks) {
            let mut f = format.clone();
            if let Some(current) = mark {
                f.background = find_color(current);
                if current {
                    f.color = egui::Color32::BLACK;
                }
            }
            job.append(&run.text[piece.start - byte..piece.end - byte], 0.0, f);
        }
        byte += run.text.len();
        let n = run.text.chars().count();
        if let Some(url) = link {
            links.push((at, at + n, url));
        }
        at += n;
    }
    (job, links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_message::{Content, RemoteCommand};

    fn ink() -> Ink {
        let c = egui::Color32::WHITE;
        Ink { text: c, strong: c, link: c, muted: c, code_bg: c, warn: c }
    }

    #[test]
    fn conversation_rows_follow_the_messages() {
        use crate::ai_tools::Call;
        let d1 = 86_400.0 * 100.0 + 50.0;
        let d2 = d1 + 86_400.0;
        let user = Message::new("C", Participant::User, Content::PlainText { text: " hi".into(), context: Some("secret".into()) }, d1);
        let flagged = Message::new("C", Participant::Agent, Content::RemoteCommandRequest { payload: crate::chat_message::Payload::Classic(RemoteCommand { llm_message: serde_json::json!({}), call: Call::ExecuteCommand { command: "rm x".into() } }), safe: Some(false) }, d2);
        let hidden = Message::new("C", Participant::Agent, Content::RenameChat("t".into()), d2);
        let day = |t: f64| (2026, 9, (t / 86_400.0) as u32);
        let label = |t: f64| format!("day {}", (t / 86_400.0) as u32);
        let time = |t: f64| format!("t{}", t as u64 % 100);
        let rows = convo_rows(&[user.clone(), hidden, flagged.clone()], true, &day, &label, &time, &|_, _| (vec![], false));
        let items: Vec<&Item> = rows.iter().map(|r| &r.item).collect();
        assert_eq!(items, [&Item::Date("day 100".into()), &Item::Message(0), &Item::Date("day 101".into()), &Item::Message(1), &Item::Typing]);
        let (a, b) = (&rows[1], &rows[3]);
        assert_eq!((a.id.as_str(), a.user, a.style, a.editable, a.glyph, a.time.as_str()), (user.unique_id.as_str(), true, Style::Regular, true, None, "t50"));
        assert_eq!(a.body, Body::Plain("hi".into()));
        assert_eq!((b.id.as_str(), b.user, b.editable, b.glyph), (flagged.unique_id.as_str(), false, false, Some(chat_view::WARNING_GLYPH)));
        assert!(matches!(&b.body, Body::Markdown(t) if t.contains("rm x")));
        assert_ne!(a.key, b.key);
        assert_eq!(rows[0], RowData::marker(Item::Date("day 100".into())));
        assert_eq!(rows[4], RowData::marker(Item::Typing));
        // The same messages give the same keys, so a redraw reuses the rendered markdown.
        assert_eq!(convo_rows(&[user, flagged], false, &day, &label, &time, &|_, _| (vec![], false))[1].key, a.key);
    }

    #[test]
    fn buttons_ride_on_the_row_with_the_last_message_rule() {
        let mk = |text: &str| Message::new("C", Participant::Agent, Content::Markdown(text.into()), 100.0);
        let (first, second) = (mk("one"), mk("two"));
        let day = |_: f64| (2026, 9, 1);
        let seen = std::cell::RefCell::new(Vec::new());
        let for_message = |m: &Message, last: bool| {
            seen.borrow_mut().push((m.unique_id.clone(), last));
            (vec![Button { title: "Go".into(), destructive: false, id: "go".into() }], last)
        };
        let rows = convo_rows(&[first.clone(), second.clone()], false, &day, &|_| "d".into(), &|_| "t".into(), &for_message);
        let msgs: Vec<&RowData> = rows.iter().filter(|r| matches!(r.item, Item::Message(_))).collect();
        assert_eq!(msgs.iter().map(|r| (r.buttons.len(), r.enabled)).collect::<Vec<_>>(), [(1, false), (1, true)]);
        assert_eq!(*seen.borrow(), [(first.unique_id, false), (second.unique_id, true)]);
        assert!(rows.iter().filter(|r| !matches!(r.item, Item::Message(_))).all(|r| r.buttons.is_empty() && !r.enabled), "dates carry none");
    }

    fn row(id: &str, buttons: &[(&str, &str)], enabled: bool) -> RowData {
        let buttons = buttons.iter().map(|(title, id)| Button { title: title.to_string(), destructive: false, id: id.to_string() }).collect();
        RowData { id: id.into(), buttons, enabled, ..RowData::marker(Item::Message(0)) }
    }

    #[test]
    fn buttons_grey_when_pressed_or_disabled_or_waiting_for_12f() {
        let mut pressed = std::collections::HashSet::new();
        let request = row("R", &[("Allow Once", "allowOnce"), ("Always Deny", "denyAlways")], true);
        assert_eq!(button_look(&request, &pressed, &request.buttons[0]), Look::Live);
        assert!(!stays_live(&request));
        // A press greys the whole row at once, before the app answers.
        pressed.insert("R".to_string());
        assert!(request.buttons.iter().all(|b| button_look(&request, &pressed, b) == Look::Off));
        // The app disables the row (or it goes): the note is forgotten.
        prune_pressed(&mut pressed, &[row("R", &[("Allow Once", "allowOnce")], false)]);
        assert!(pressed.is_empty());
        pressed.insert("R".to_string());
        prune_pressed(&mut pressed, &[row("R", &[("Allow Once", "allowOnce")], true)]);
        assert!(pressed.contains("R"), "still enabled: still waiting for the answer");
        prune_pressed(&mut pressed, &[]);
        assert!(pressed.is_empty(), "the message is gone");
        // A row the app did not enable stays off.
        let off = row("R", &[("Link", "link")], false);
        assert_eq!(button_look(&off, &pressed, &off.buttons[0]), Look::Off);
        // The orchestration buttons show but wait for 12f, enabled row or not.
        let offer = row("O", &[("Link", "link"), ("Enable Orchestration", "orchestrate")], true);
        assert_eq!(button_look(&offer, &pressed, &offer.buttons[0]), Look::Live);
        assert_eq!(button_look(&offer, &pressed, &offer.buttons[1]), Look::Later);
        let revoke = row("V", &[("Revoke", "revokeOrchestrationPermission:s")], true);
        assert_eq!(button_look(&revoke, &pressed, &revoke.buttons[0]), Look::Later);
        // The permissions bubble is tapped again and again.
        let perms = row("P", &[("AI can Run Commands: Ask", "Run Commands"), ("AI can View Contents: Ask", "View Contents")], true);
        assert!(stays_live(&perms));
        assert!(!stays_live(&row("X", &[], true)), "no buttons, nothing to tap");
    }

    #[test]
    fn copied_slots_are_per_part_and_the_warning_has_a_word() {
        assert_ne!(copy_slot(1, 0), copy_slot(2, 0), "two parts starting with a code block");
        assert_ne!(copy_slot(1, 3), copy_slot(1, 4));
        assert_ne!(copy_slot(0, 5), usize::MAX - 0, "never a code attachment's slot");
        assert_eq!(glyph_fallback(chat_view::WARNING_GLYPH), Some("Warning"));
        assert_eq!(glyph_fallback(chat_view::TIMER_GLYPH), None);
    }

    #[test]
    fn only_web_and_mail_links_are_clickable() {
        // Rulings F18 and m10, at the drawing: the runs of a model-written link a person could click.
        let md = "[a](file:///C:/x.exe) [b](https://ok.example/p) [c](iterm2:reveal-mark?s=1) [d](\\\\host\\share) [e](mailto:x@y.example)";
        let doc = crate::porthole::render::render(md, &Language::Markdown, &Palette::default(), true);
        let (job, links) = block_job(&doc.blocks[0], &ink(), 300.0, &[], InApp::None);
        let urls: Vec<&str> = links.iter().map(|(_, _, u)| u.as_str()).collect();
        assert_eq!(urls, ["https://ok.example/p", "mailto:x@y.example"]);
        // The ranges cover exactly the link texts.
        let text: Vec<char> = job.text.chars().collect();
        let shown: Vec<String> = links.iter().map(|(from, to, _)| text[*from..*to].iter().collect()).collect();
        assert_eq!(shown, ["b", "e"]);
        // An explanation response also draws its annotation links, and no other `iterm2:` one.
        // An explanation request draws only its attached-content link.
        let md = "[a](iterm2:annotation?ann=A1&s=G1) [b](iterm2:reveal-mark?s=1&m=2) [c](iterm2:/compound-location?session=G1&sub=0,7,3,9)";
        let doc = crate::porthole::render::render(md, &Language::Markdown, &Palette::default(), true);
        let urls = |in_app: InApp| block_job(&doc.blocks[0], &ink(), 300.0, &[], in_app).1.into_iter().map(|(_, _, u)| u).collect::<Vec<_>>();
        assert!(urls(InApp::None).is_empty(), "any other message keeps them inert");
        assert_eq!(urls(InApp::Annotations), ["iterm2:annotation?ann=A1&s=G1"]);
        assert_eq!(urls(InApp::Location), ["iterm2:/compound-location?session=G1&sub=0,7,3,9"]);
        // Which messages draw which: by author and kind.
        let request = crate::ai_explain::request_content(&crate::ai_explain::Request { command: None, truncated: false, original: crate::ai_explain::Located { string: String::new(), coords: vec![] }, question: String::new(), subject: "s".into(), url: None, context: crate::ai_explain::Context { session_id: "G".into(), base_offset: 0 } });
        let m = |who: Participant, content: Content| Message::new("C", who, content, 1.0);
        assert_eq!(InApp::of(&m(Participant::User, Content::ExplanationRequest(request.clone()))), InApp::Location);
        assert_eq!(InApp::of(&m(Participant::Agent, Content::ExplanationRequest(request))), InApp::None, "only the person's request");
        assert_eq!(InApp::of(&m(Participant::Agent, Content::ExplanationResponse(serde_json::json!({})))), InApp::Annotations);
        assert_eq!(InApp::of(&m(Participant::User, Content::ExplanationResponse(serde_json::json!({})))), InApp::None);
        assert_eq!(InApp::of(&m(Participant::User, Content::Markdown("x".into()))), InApp::None);
    }

    #[test]
    fn a_find_match_in_a_tall_message_is_what_scrolls_into_view() {
        let text = "alpha\nbeta\ngamma\ndelta\nneedle one\nzeta\neta\ntheta\nneedle two";
        let ctx = egui::Context::default();
        let mut rects = Vec::new();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = 300.0;
            job.append(text, 0.0, egui::TextFormat::simple(egui::FontId::proportional(14.0), egui::Color32::WHITE));
            let galley = ui.painter().layout_job(job);
            let first = text.find("needle one").unwrap();
            let second = text.find("needle two").unwrap();
            let origin = egui::pos2(10.0, 100.0);
            rects.push(match_rect(&galley, text, 0..5, origin));
            rects.push(match_rect(&galley, text, first..first + 6, origin));
            rects.push(match_rect(&galley, text, second..second + 6, origin));
        });
        out.textures_delta.clear();
        let (a, one, two) = (rects[0], rects[1], rects[2]);
        assert!(a.min.y >= 100.0 && a.min.y < one.min.y && one.min.y < two.min.y, "each match sits on its own row: {rects:?}");
        assert!(one.width() > 10.0 && one.height() > 5.0, "the rectangle covers the matched characters: {one:?}");
        assert!(two.min.y - one.min.y >= 3.0 * 14.0, "four lines apart: {rects:?}");
    }
}
