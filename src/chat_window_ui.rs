//! The AI Chat window (iTerm2's `ChatWindowController`): one egui window through `EguiHost`, the
//! tmux Dashboard's pattern, data in and events out. This file holds the shell and the toolbar;
//! the chat list, the conversation and the input are filled by the tasks after it.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::ai_catalog::{Effort, ServiceTier};
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
    Close,
}

pub struct ChatWindow {
    host: EguiHost,
    data: ChatData,
    title: String,
    pub list_visible: bool,
}

impl ChatWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<ChatWindow, String> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(tb::WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(tb::SIZE.0, tb::SIZE.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(tb::MIN_SIZE.0, tb::MIN_SIZE.1));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(ChatWindow { host, data: ChatData::default(), title: tb::WINDOW_TITLE.to_string(), list_visible: true })
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
        self.host.paint(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            egui::Panel::top("chat_toolbar").show(ui, |ui| {
                ui.add_space(4.0);
                toolbar(ui, &d.toolbar, icons, list_visible, &mut out);
                ui.add_space(4.0);
            });
            if *list_visible {
                egui::Panel::left("chat_list").resizable(true).default_size(tb::SIDEBAR_WIDTH).min_size(100.0).show(ui, |_ui| {
                    // The chat list: Task 2.
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
                let p = r.rect.left_bottom();
                let scale = ui.ctx().pixels_per_point();
                let origin = ui.ctx().input(|i| i.viewport().inner_rect).map_or(egui::Pos2::ZERO, |r| r.min);
                out.push(ChatEvent::SessionMenu(((origin.x + p.x) * scale, (origin.y + p.y) * scale)));
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
