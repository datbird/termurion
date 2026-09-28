//! The AI Chat window (iTerm2's `ChatWindowController`): one egui window through `EguiHost`, the
//! tmux Dashboard's pattern, data in and events out. This file holds the shell and the toolbar;
//! the chat list, the conversation and the input are filled by the tasks after it.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use std::collections::HashMap;

use crate::ai_catalog::{Effort, ServiceTier};
use crate::chat_list::{self, Row, Selection};
use crate::chat_toolbar::{self as tb, Popup, ProviderOption};
use crate::egui_host::EguiHost;

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
    /// Enter in the list: rename this chat.
    Rename(String),
    /// Delete in the list: delete these chats (after the question).
    Delete(Vec<String>),
    /// A right click: the pointer in screen pixels and the selected chats.
    ListMenu { at: (f32, f32), ids: Vec<String> },
    Close,
}

/// What the list remembers between frames: the selection, the chat it last showed, and the
/// icons decoded once, keyed by their bytes' hash (None when they would not decode).
#[derive(Default)]
struct ListState {
    selection: Selection,
    last_shown: Option<String>,
    textures: HashMap<u64, Option<egui::TextureHandle>>,
}

pub struct ChatWindow {
    host: EguiHost,
    data: ChatData,
    title: String,
    pub list_visible: bool,
    list: ListState,
}

impl ChatWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<ChatWindow, String> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(tb::WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(tb::SIZE.0, tb::SIZE.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(tb::MIN_SIZE.0, tb::MIN_SIZE.1));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(ChatWindow { host, data: ChatData::default(), title: tb::WINDOW_TITLE.to_string(), list_visible: true, list: ListState::default() })
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
        self.data = d;
        self.host.request_redraw();
    }

    /// One frame. Every event of the frame, in order.
    pub fn redraw(&mut self) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        let d = &self.data;
        let list_visible = &mut self.list_visible;
        let list = &mut self.list;
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
            egui::CentralPanel::default().show(ui, |_ui| {
                // The conversation, its input and the find bar: Tasks 4, 7 and 8.
            });
        });
        out
    }
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

/// A point of the window in screen pixels (where a native popup menu opens).
fn screen_pixels(ui: &egui::Ui, p: egui::Pos2) -> (f32, f32) {
    let scale = ui.ctx().pixels_per_point();
    let origin = ui.ctx().input(|i| i.viewport().inner_rect).map_or(egui::Pos2::ZERO, |r| r.min);
    ((origin.x + p.x) * scale, (origin.y + p.y) * scale)
}

const ROW_HEIGHT: f32 = 56.0;
/// Icons of a stored PNG above this many pixels a side draw as the default icon: a hostile
/// database must not make a huge texture for a 32-point circle.
const MAX_ICON_SIDE: u32 = 512;

/// The chat list: the header (the new chat button; Task 3 adds the search field), then the
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
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if glyph_button(ui, icons, chat_list::NEW_CHAT_GLYPH, "+", tb::NEW_CHAT_TIP, true, false) {
                out.push(ChatEvent::NewChat);
            }
        });
    });
    ui.separator();
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
            let decoded = crate::images::decode(bytes).ok().filter(|d| d.width <= MAX_ICON_SIDE && d.height <= MAX_ICON_SIDE)?;
            let frame = decoded.frames.first()?;
            let image = egui::ColorImage::from_rgba_unmultiplied([decoded.width as usize, decoded.height as usize], &frame.rgba);
            Some(ctx.load_texture(format!("chat-icon-{key:x}"), image, egui::TextureOptions::LINEAR))
        })
        .as_ref()
}
