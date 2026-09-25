//! The "tmux Dashboard" window (iTerm2's TmuxDashboard.xib): the connection popup, the
//! sessions table, the windows table, and the attach settings.

use std::collections::BTreeSet;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::tmux_dashboard::{self as model, SessionRow, WindowRow};

/// What the person did in the Dashboard.
#[derive(Clone, Debug, PartialEq)]
pub enum DashEvent {
    None,
    Connection(usize),
    Session(u32),
    Attach(u32),
    Detach,
    AddSession(String),
    RemoveSession(u32),
    RenameSession(u32, String),
    /// + in the windows table: a new window in this session.
    AddWindow(u32),
    RemoveWindows(Vec<u32>),
    OpenInTabs(Vec<u32>),
    OpenInWindows(Vec<u32>),
    Hide(Vec<u32>),
    RenameWindow(u32, u32, String),
    /// A double-click: reveal an open window, open a hidden one in a window.
    Reveal(u32, bool),
    Limit(i64),
    OpenIfHidden(bool),
    Close,
}

/// What the Dashboard shows (the app fills it each pass).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashData {
    pub connections: Vec<String>,
    pub connection: usize,
    pub sessions: Vec<SessionRow>,
    pub attached: Option<u32>,
    pub windows: Vec<WindowRow>,
    pub limit: i64,
    pub open_if_hidden: bool,
}

pub struct TmuxDashboardWindow {
    host: EguiHost,
    pub data: DashData,
    pub selected_session: Option<u32>,
    selected_windows: BTreeSet<u32>,
    search: String,
    editing_session: Option<(u32, String)>,
    editing_window: Option<(u32, String)>,
}

impl TmuxDashboardWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<TmuxDashboardWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "tmux Dashboard", 720.0, 460.0)?;
        Ok(TmuxDashboardWindow { host, data: DashData::default(), selected_session: None, selected_windows: BTreeSet::new(), search: String::new(), editing_session: None, editing_window: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    pub fn set_data(&mut self, data: DashData) {
        if self.selected_session.is_none_or(|s| !data.sessions.iter().any(|r| r.id == s)) {
            self.selected_session = data.attached.or(data.sessions.first().map(|s| s.id));
        }
        self.selected_windows.retain(|w| data.windows.iter().any(|r| r.id == *w));
        self.data = data;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> DashEvent {
        let mut out = DashEvent::None;
        let d = &self.data;
        let (sel_s, sel_w, search, edit_s, edit_w) = (&mut self.selected_session, &mut self.selected_windows, &mut self.search, &mut self.editing_session, &mut self.editing_window);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if ui.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.command) {
                    out = DashEvent::Close;
                }
                // The connections popup.
                let current = d.connections.get(d.connection).cloned().unwrap_or_else(|| "No tmux connection".into());
                egui::ComboBox::from_id_salt("conn").selected_text(current).show_ui(ui, |ui| {
                    for (i, c) in d.connections.iter().enumerate() {
                        if ui.selectable_label(i == d.connection, c).clicked() {
                            out = DashEvent::Connection(i);
                        }
                    }
                });
                ui.separator();
                ui.columns(2, |cols| {
                    // Sessions.
                    let ui = &mut cols[0];
                    ui.strong("Session");
                    egui::ScrollArea::vertical().id_salt("sessions").max_height(280.0).show(ui, |ui| {
                        for s in &d.sessions {
                            let mark = if d.attached == Some(s.id) { "\u{2713} " } else { "   " };
                            match edit_s {
                                Some((id, text)) if *id == s.id => {
                                    let r = ui.text_edit_singleline(text);
                                    if r.lost_focus() {
                                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                                            out = DashEvent::RenameSession(s.id, text.trim().to_string());
                                        }
                                        *edit_s = None;
                                    } else {
                                        r.request_focus();
                                    }
                                }
                                _ => {
                                    let r = ui.selectable_label(*sel_s == Some(s.id), format!("{mark}{}", s.name));
                                    if r.clicked() {
                                        *sel_s = Some(s.id);
                                        sel_w.clear();
                                        out = DashEvent::Session(s.id);
                                    }
                                    if r.double_clicked() {
                                        *edit_s = Some((s.id, s.name.clone()));
                                    }
                                }
                            }
                        }
                    });
                    let (attach, detach, remove) = model::session_buttons(*sel_s, d.attached);
                    ui.horizontal(|ui| {
                        if ui.button("+").clicked() {
                            out = DashEvent::AddSession(model::new_session_name(&d.sessions));
                        }
                        if ui.add_enabled(remove, egui::Button::new("-")).clicked() {
                            out = DashEvent::RemoveSession(sel_s.unwrap_or(0));
                        }
                        if ui.add_enabled(attach, egui::Button::new("Attach")).clicked() {
                            out = DashEvent::Attach(sel_s.unwrap_or(0));
                        }
                        if ui.add_enabled(detach, egui::Button::new("Detach")).clicked() {
                            out = DashEvent::Detach;
                        }
                    });
                    // Windows.
                    let ui = &mut cols[1];
                    ui.add(egui::TextEdit::singleline(search).hint_text("Search Windows").desired_width(f32::INFINITY));
                    ui.strong("Window");
                    let shown = model::filter(&d.windows, search);
                    egui::ScrollArea::vertical().id_salt("windows").max_height(250.0).show(ui, |ui| {
                        for w in &shown {
                            match edit_w {
                                Some((id, text)) if *id == w.id => {
                                    let r = ui.text_edit_singleline(text);
                                    if r.lost_focus() {
                                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                                            out = DashEvent::RenameWindow(sel_s.unwrap_or(0), w.id, text.trim().to_string());
                                        }
                                        *edit_w = None;
                                    } else {
                                        r.request_focus();
                                    }
                                }
                                _ => {
                                    // Hidden windows are dimmed (iTerm2 draws them at half alpha).
                                    let text = if w.open { egui::RichText::new(&w.name) } else { egui::RichText::new(&w.name).weak() };
                                    let r = ui.selectable_label(sel_w.contains(&w.id), text);
                                    if r.clicked() {
                                        if !ui.input(|i| i.modifiers.command || i.modifiers.shift) {
                                            sel_w.clear();
                                        }
                                        if !sel_w.insert(w.id) {
                                            sel_w.remove(&w.id);
                                        }
                                    }
                                    if r.double_clicked() {
                                        out = DashEvent::Reveal(w.id, w.open);
                                    }
                                    if r.secondary_clicked() {
                                        *edit_w = Some((w.id, w.name.clone()));
                                    }
                                }
                            }
                        }
                    });
                    let selected: Vec<&WindowRow> = d.windows.iter().filter(|w| sel_w.contains(&w.id)).collect();
                    let ids: Vec<u32> = selected.iter().map(|w| w.id).collect();
                    let b = model::window_buttons(sel_s.is_some(), sel_s.is_some() && *sel_s == d.attached, shown.len(), &selected);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(b.add, egui::Button::new("+")).clicked() {
                            out = DashEvent::AddWindow(sel_s.unwrap_or(0));
                        }
                        if ui.add_enabled(b.remove, egui::Button::new("-")).clicked() {
                            out = DashEvent::RemoveWindows(ids.clone());
                        }
                        if ui.add_enabled(b.open_in_tabs, egui::Button::new("Open in Tabs")).clicked() {
                            out = DashEvent::OpenInTabs(ids.clone());
                        }
                        if ui.add_enabled(b.open_in_windows, egui::Button::new(b.open_label)).clicked() {
                            out = DashEvent::OpenInWindows(ids.clone());
                        }
                        if ui.add_enabled(b.hide, egui::Button::new("Hide")).clicked() {
                            out = DashEvent::Hide(ids.clone());
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(model::LIMIT_LABEL);
                    let mut n = d.limit;
                    if ui.add(egui::DragValue::new(&mut n).range(1..=1000)).changed() {
                        out = DashEvent::Limit(n);
                    }
                });
                let mut on = d.open_if_hidden;
                if ui.checkbox(&mut on, model::HIDDEN_LABEL).changed() {
                    out = DashEvent::OpenIfHidden(on);
                }
            });
        });
        out
    }
}
