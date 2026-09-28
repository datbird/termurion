//! The "Script Dependency Editor" window (iTerm2's `iTermDependencyEditorWindowController.xib`):
//! the Script popup; for a full environment the Python Version popup, the Package Name /
//! Installed Version table, Add, Remove and Check for Update; for a basic script the upgrade
//! text and Upgrade; OK closes.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::script_deps as sd;

/// What the person did in the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum DepsEvent {
    None,
    /// A script picked in the Script popup.
    Script(usize),
    /// A Python version picked that differs from the environment's.
    Version(String),
    Add,
    /// Remove, and Check for Update, on a table row.
    Remove(usize),
    CheckForUpdate(usize),
    Upgrade,
    Close,
}

/// What the editor shows (the app fills it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepsData {
    /// The Script popup's titles and the one selected.
    pub scripts: Vec<String>,
    pub selected: usize,
    /// Whether the selected script is a full environment.
    pub full: bool,
    /// The Python Version popup's items and the environment's version.
    pub versions: Vec<String>,
    pub version: String,
    /// The table: each dependency and its installed version.
    pub packages: Vec<(String, String)>,
    /// A rebuild runs: the controls are off (`setEditingControlsEnabled:`, `DE:53-60`).
    pub busy: bool,
}

pub struct DepsWindow {
    host: EguiHost,
    data: DepsData,
    /// The table's selected row (`tableViewSelectionDidChange:`, `DE:781-785`).
    row: Option<usize>,
}

impl DepsWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<DepsWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, sd::TITLE, 520.0, 420.0)?;
        Ok(DepsWindow { host, data: DepsData::default(), row: None })
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

    /// New data; another script, or a table that changed, clears the row selection
    /// (`didSelectScriptAtIndex:` turns Remove and Check for Update off, `DE:182-184`).
    pub fn set_data(&mut self, data: DepsData) {
        if data.selected != self.data.selected || data.scripts != self.data.scripts || data.packages.iter().map(|p| &p.0).ne(self.data.packages.iter().map(|p| &p.0)) {
            self.row = None;
        }
        self.data = data;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> DepsEvent {
        let mut out = DepsEvent::None;
        let before = self.row;
        let (d, row) = (&self.data, &mut self.row);
        let enabled = !d.busy;
        self.host.paint(|ui| {
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || (i.key_pressed(egui::Key::W) && i.modifiers.command)) {
                out = DepsEvent::Close;
            }
            egui::Panel::bottom("ok").show(ui, |ui| {
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::api_auth::OK).clicked() {
                        out = DepsEvent::Close;
                    }
                });
                ui.add_space(4.0);
            });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(sd::SCRIPT_LABEL);
                    let current = d.scripts.get(d.selected).cloned().unwrap_or_default();
                    ui.add_enabled_ui(enabled, |ui| {
                        egui::ComboBox::from_id_salt("script").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                            for (i, s) in d.scripts.iter().enumerate() {
                                if ui.selectable_label(i == d.selected, s).clicked() && i != d.selected {
                                    out = DepsEvent::Script(i);
                                }
                            }
                        });
                    });
                });
                ui.separator();
                if d.scripts.is_empty() {
                    return;
                }
                if !d.full {
                    ui.label(sd::UPGRADE_TEXT_BASIC);
                    ui.add_space(8.0);
                    if ui.add_enabled(enabled, egui::Button::new(sd::UPGRADE)).clicked() {
                        out = DepsEvent::Upgrade;
                    }
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label(sd::PYTHON_LABEL);
                    ui.add_enabled_ui(enabled, |ui| {
                        egui::ComboBox::from_id_salt("python").selected_text(d.version.as_str()).show_ui(ui, |ui| {
                            for v in &d.versions {
                                if ui.selectable_label(*v == d.version, v).clicked() && *v != d.version {
                                    out = DepsEvent::Version(v.clone());
                                }
                            }
                        });
                    });
                });
                ui.add_space(4.0);
                let height = (ui.available_height() - 36.0).max(80.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_height(height);
                    egui::Grid::new("head").num_columns(2).min_col_width(ui.available_width() / 2.0 - 8.0).show(ui, |ui| {
                        ui.strong(sd::PACKAGE_COLUMN);
                        ui.strong(sd::VERSION_COLUMN);
                        ui.end_row();
                    });
                    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(height - 24.0).show(ui, |ui| {
                        egui::Grid::new("packages").num_columns(2).min_col_width(ui.available_width() / 2.0 - 8.0).striped(true).show(ui, |ui| {
                            for (i, (name, version)) in d.packages.iter().enumerate() {
                                let on = *row == Some(i);
                                let a = ui.add_enabled(enabled, egui::Button::selectable(on, name.as_str()));
                                let b = ui.add_enabled(enabled, egui::Button::selectable(on, version.as_str()));
                                if a.clicked() || b.clicked() {
                                    *row = if on { None } else { Some(i) };
                                }
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(enabled, egui::Button::new(sd::ADD)).clicked() {
                        out = DepsEvent::Add;
                    }
                    let chosen = row.filter(|r| *r < d.packages.len());
                    if ui.add_enabled(enabled && chosen.is_some(), egui::Button::new(sd::REMOVE)).clicked() {
                        out = DepsEvent::Remove(chosen.unwrap_or(0));
                    }
                    if ui.add_enabled(enabled && chosen.is_some(), egui::Button::new(sd::CHECK_FOR_UPDATE)).clicked() {
                        out = DepsEvent::CheckForUpdate(chosen.unwrap_or(0));
                    }
                });
            });
        });
        if self.row != before {
            self.host.request_redraw();
        }
        out
    }
}
