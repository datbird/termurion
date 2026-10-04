//! The remote file panel's window (milestone 14d, `SSHFilePanel.xib`): a sidebar of connected
//! hosts, a toolbar with Back, Forward, the path and the search field, the file table, and the
//! Open, Cancel and "Use System Panel" buttons, drawn through `EguiHost`. It shows the model
//! (`ssh_panel::Panel`) and reports what the person did; the app acts on it. Every name in it
//! came from a remote host and goes through `shown_name` (P14).

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::inert_files::shown_name;
use crate::ssh_panel::{self as m, Column, Panel};

/// What the person did in the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelEvent {
    None,
    /// A host in the sidebar (an index into `Panel::hosts`).
    Host(usize),
    Back,
    Forward,
    Up,
    Home,
    Refresh,
    ToggleHidden,
    /// A path to go to as it is (a completion that was picked).
    GoTo(String),
    /// A path typed, to be expanded (`~`, relative, `..`) and gone to.
    GoToTyped(String),
    /// Go to Folder's text changed: its completions are wanted (Tandem's).
    Complete(String),
    Search(String),
    Sort(Column),
    Click { path: String, ctrl: bool, shift: bool },
    DoubleClick(String),
    SystemPanel,
    Open,
    Cancel,
}

pub struct SshPanelWindow {
    host: EguiHost,
    local_user: String,
    /// The path field's text, and the panel path it was last set from.
    path_text: String,
    path_shown: Option<String>,
    search: String,
    /// Go to Folder's dialog, with its text, while it is open.
    goto: Option<String>,
}

/// The table's column shares of its width.
const SHARES: [f32; 4] = [0.44, 0.24, 0.12, 0.20];
const ROW: f32 = 20.0;

impl SshPanelWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<SshPanelWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, m::TITLE, 860.0, 520.0)?;
        Ok(SshPanelWindow { host, local_user: crate::ssh_args::local_user(), path_text: String::new(), path_shown: None, search: String::new(), goto: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    pub fn event(&mut self, e: &WindowEvent) -> bool {
        self.host.event(e)
    }

    /// Draws a frame of `panel` and reports what the person did in it.
    pub fn redraw(&mut self, panel: &Panel) -> PanelEvent {
        let mut out = PanelEvent::None;
        if self.path_shown != panel.path {
            self.path_text = panel.path.as_deref().map(shown_name).unwrap_or_default();
            self.path_shown = panel.path.clone();
        }
        let (path_text, search, goto, user) = (&mut self.path_text, &mut self.search, &mut self.goto, &self.local_user);
        self.host.paint(|ui| {
            let (mut open_goto, enter, esc) = ui.input(|i| {
                let c = i.modifiers.command;
                (c && i.modifiers.shift && i.key_pressed(egui::Key::G), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
            });
            ui.input(|i| {
                let c = i.modifiers.command;
                if c && i.modifiers.shift && i.key_pressed(egui::Key::H) {
                    out = PanelEvent::Home;
                } else if c && i.modifiers.shift && i.key_pressed(egui::Key::Period) {
                    out = PanelEvent::ToggleHidden;
                } else if c && i.key_pressed(egui::Key::ArrowUp) {
                    out = PanelEvent::Up;
                } else if c && i.key_pressed(egui::Key::ArrowLeft) {
                    out = PanelEvent::Back;
                } else if c && i.key_pressed(egui::Key::ArrowRight) {
                    out = PanelEvent::Forward;
                } else if c && !i.modifiers.shift && i.key_pressed(egui::Key::R) {
                    out = PanelEvent::Refresh;
                } else if c && i.key_pressed(egui::Key::W) {
                    out = PanelEvent::Cancel;
                }
            });
            let typing = ui.ctx().memory(|mem| mem.focused().is_some());
            if esc {
                if goto.is_some() {
                    *goto = None;
                } else {
                    out = PanelEvent::Cancel;
                }
            } else if enter && !typing && goto.is_none() && panel.open_enabled() {
                out = PanelEvent::Open;
            }

            egui::Panel::top("toolbar").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(panel.can_back(), egui::Button::new(m::BACK)).clicked() {
                        out = PanelEvent::Back;
                    }
                    if ui.add_enabled(panel.can_forward(), egui::Button::new(m::FORWARD)).clicked() {
                        out = PanelEvent::Forward;
                    }
                    let width = (ui.available_width() - 200.0).max(80.0);
                    let r = ui.add_enabled(panel.path.is_some(), egui::TextEdit::singleline(path_text).desired_width(width).hint_text(m::PATH_PLACEHOLDER));
                    if r.lost_focus() && enter {
                        out = PanelEvent::GoToTyped(path_text.clone());
                    }
                    let s = ui.add(egui::TextEdit::singleline(search).desired_width(ui.available_width()).hint_text(m::SEARCH));
                    if s.lost_focus() && enter {
                        out = PanelEvent::Search(search.clone());
                    }
                });
                ui.add_space(4.0);
            });

            egui::Panel::bottom("buttons").show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(m::USE_SYSTEM_PANEL).clicked() {
                        out = PanelEvent::SystemPanel;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(panel.open_enabled(), egui::Button::new(m::OPEN)).clicked() {
                            out = PanelEvent::Open;
                        }
                        if ui.button(m::CANCEL).clicked() {
                            out = PanelEvent::Cancel;
                        }
                    });
                });
                ui.add_space(6.0);
            });

            egui::Panel::left("hosts").default_size(170.0).show(ui, |ui| {
                ui.add_space(4.0);
                ui.strong(m::CONNECTED_HOSTS);
                ui.separator();
                for (i, h) in panel.hosts.iter().enumerate() {
                    let name = shown_name(&h.display_name(user));
                    if ui.selectable_label(panel.host.as_ref() == Some(h), name).clicked() {
                        out = PanelEvent::Host(i);
                    }
                }
            });

            egui::CentralPanel::default().show(ui, |ui| {
                let width = ui.available_width();
                let edges: Vec<f32> = SHARES.iter().scan(0.0, |x, s| { let at = *x; *x += s * width; Some(at) }).collect();
                // The headers sort.
                ui.horizontal(|ui| {
                    for (i, (col, title)) in [(Column::Name, m::COLUMN_NAME), (Column::Date, m::COLUMN_DATE), (Column::Size, m::COLUMN_SIZE), (Column::Kind, m::COLUMN_KIND)].into_iter().enumerate() {
                        let w = SHARES[i] * width - ui.spacing().item_spacing.x;
                        if ui.add_sized([w.max(10.0), ROW], egui::Button::new(egui::RichText::new(title).strong()).frame(false)).clicked() {
                            out = PanelEvent::Sort(col);
                        }
                    }
                });
                ui.separator();
                if panel.loading {
                    ui.weak("\u{2026}");
                }
                if let Some(e) = &panel.error {
                    ui.colored_label(ui.visuals().error_fg_color, shown_name(e));
                }
                if let Some(n) = &panel.notice {
                    ui.weak(shown_name(n));
                }
                let rows = panel.visible();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows.len(), |ui, range| {
                    for f in &rows[range] {
                        let selected = panel.is_selected(&f.absolute_path);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click());
                        if selected {
                            ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                        }
                        let color = if selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
                        let cells = [shown_name(&f.name()), crate::remote_file::date_text(f.unix_mtime()), crate::remote_file::size_text(f.size()), shown_name(&crate::remote_file::kind_text(f))];
                        for (i, text) in cells.iter().enumerate() {
                            let cell = egui::Rect::from_min_max(egui::pos2(rect.left() + edges[i], rect.top()), egui::pos2((rect.left() + edges[i] + SHARES[i] * width - 6.0).min(rect.right()), rect.bottom()));
                            ui.painter().with_clip_rect(cell).text(egui::pos2(cell.left() + 4.0, cell.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), color);
                        }
                        if resp.double_clicked() {
                            out = PanelEvent::DoubleClick(f.absolute_path.clone());
                        } else if resp.clicked() {
                            let mods = ui.input(|i| i.modifiers);
                            out = PanelEvent::Click { path: f.absolute_path.clone(), ctrl: mods.command, shift: mods.shift };
                        }
                        resp.context_menu(|ui| {
                            if ui.add(egui::Button::new(m::GO_TO_FOLDER).shortcut_text("Ctrl+Shift+G")).clicked() {
                                open_goto = true;
                                ui.close();
                            }
                            if ui.add(egui::Button::new(m::GO_TO_HOME).shortcut_text("Ctrl+Shift+H")).clicked() {
                                out = PanelEvent::Home;
                                ui.close();
                            }
                            if ui.add(egui::Button::new(m::GO_UP).shortcut_text("Ctrl+Up")).clicked() {
                                out = PanelEvent::Up;
                                ui.close();
                            }
                            if ui.add(egui::Button::new(m::REFRESH).shortcut_text("Ctrl+R")).clicked() {
                                out = PanelEvent::Refresh;
                                ui.close();
                            }
                            let mut shown = panel.show_hidden;
                            if ui.checkbox(&mut shown, m::SHOW_HIDDEN).clicked() {
                                out = PanelEvent::ToggleHidden;
                                ui.close();
                            }
                        });
                    }
                });
            });

            if open_goto && goto.is_none() {
                *goto = Some(path_text.clone());
                out = PanelEvent::Complete(path_text.clone());
            }
            if let Some(text) = goto.as_mut() {
                let title = m::folder_on(&panel.host.as_ref().map(|h| shown_name(&h.display_name(user))).unwrap_or_default());
                let ctx = ui.ctx().clone();
                let mut close = false;
                egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                    ui.label(m::GO_TO_TITLE);
                    ui.weak(m::GO_TO_INFO);
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(360.0).hint_text(m::PATH_PLACEHOLDER));
                    r.request_focus();
                    if r.changed() {
                        out = PanelEvent::Complete(text.clone());
                    }
                    if r.lost_focus() && enter {
                        out = PanelEvent::GoToTyped(text.clone());
                        close = true;
                    }
                    if panel.completing {
                        ui.weak(m::LOADING_COMPLETIONS);
                    }
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for c in panel.completion_list() {
                            if ui.selectable_label(false, shown_name(c)).clicked() {
                                out = PanelEvent::GoTo(c.clone());
                                close = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(m::GO).clicked() {
                            out = PanelEvent::GoToTyped(text.clone());
                            close = true;
                        }
                        if ui.button(m::CANCEL).clicked() {
                            close = true;
                        }
                    });
                });
                if close {
                    *goto = None;
                }
            }
        });
        out
    }
}
