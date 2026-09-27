//! The "New Python Script" picker (iTerm2's iTermScriptTemplatePickerWindowController.xib):
//! the environment, then the template, each a selectable box. A full environment's PyPI
//! dependencies and Python version are asked here too, above the templates, since Windows'
//! save dialog takes no accessory view (Decision).

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::script_new::{self as sn, Env, Template};

/// What the person did in the picker.
#[derive(Clone, Debug, PartialEq)]
pub enum PickerEvent {
    None,
    Chosen { env: Env, template: Template, dependencies: Vec<String>, python: String },
    Cancel,
}

/// Segoe Fluent Icons glyphs for the boxes, in place of iTerm2's emoji (Decision): Emoji2,
/// Link, Repair and Robot.
const BASIC_GLYPH: char = '\u{E76E}';
const FULL_GLYPH: char = '\u{E71B}';
const SIMPLE_GLYPH: char = '\u{E90F}';
const DAEMON_GLYPH: char = '\u{E99A}';

pub struct PickerWindow {
    host: EguiHost,
    /// The environment chosen on the first page; None while it shows.
    env: Option<Env>,
    dependencies: String,
    versions: Vec<String>,
    version: String,
    default_version: String,
}

impl PickerWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, versions: Vec<String>, default_version: &str) -> Result<PickerWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, sn::PICKER_TITLE, 480.0, 400.0)?;
        crate::settings_keys::add_icon_font(host.ctx());
        let mut w = PickerWindow { host, env: None, dependencies: String::new(), versions: Vec::new(), version: default_version.to_string(), default_version: default_version.to_string() };
        w.set_versions(versions);
        Ok(w)
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

    /// The Python versions the popup offers (`M:1350-1375`): sorted by number; the default
    /// stays chosen when offered, else the newest. An empty list keeps the default alone.
    pub fn set_versions(&mut self, v: Vec<String>) {
        let mut v = v;
        if v.is_empty() {
            v = vec![self.default_version.clone()];
        }
        v.sort_by(|a, b| crate::script_runtime::dotted(a, b));
        v.dedup();
        self.version = if v.contains(&self.default_version) { self.default_version.clone() } else { v.last().cloned().unwrap_or_default() };
        self.versions = v;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> PickerEvent {
        let before = self.env;
        let mut out = PickerEvent::None;
        let (env, deps, versions, version) = (&mut self.env, &mut self.dependencies, &self.versions, &mut self.version);
        self.host.paint(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = PickerEvent::Cancel;
                }
                match *env {
                    None => {
                        ui.label(sn::ENV_PROMPT);
                        ui.add_space(6.0);
                        if choice(ui, icons, BASIC_GLYPH, sn::BASIC, sn::BASIC_TEXT) {
                            *env = Some(Env::Basic);
                        }
                        ui.add_space(6.0);
                        if choice(ui, icons, FULL_GLYPH, sn::FULL, sn::FULL_TEXT) {
                            *env = Some(Env::Full);
                        }
                    }
                    Some(chosen) => {
                        if chosen == Env::Full {
                            egui::Grid::new("full").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                                ui.label(sn::DEPS_LABEL);
                                ui.add(egui::TextEdit::singleline(deps).hint_text(sn::DEPS_PLACEHOLDER).desired_width(300.0));
                                ui.end_row();
                                ui.label(sn::VERSION_LABEL);
                                egui::ComboBox::from_id_salt("version").selected_text(version.as_str()).show_ui(ui, |ui| {
                                    for v in versions {
                                        ui.selectable_value(version, v.clone(), v.as_str());
                                    }
                                });
                                ui.end_row();
                            });
                            ui.add_space(8.0);
                        }
                        ui.label(sn::TEMPLATE_PROMPT);
                        ui.add_space(6.0);
                        let picked = if choice(ui, icons, SIMPLE_GLYPH, sn::SIMPLE, sn::SIMPLE_TEXT) {
                            Some(Template::Simple)
                        } else {
                            ui.add_space(6.0);
                            choice(ui, icons, DAEMON_GLYPH, sn::DAEMON, sn::DAEMON_TEXT).then_some(Template::Daemon)
                        };
                        if let Some(template) = picked {
                            let dependencies = if chosen == Env::Full { sn::parse_dependencies(deps) } else { Vec::new() };
                            out = PickerEvent::Chosen { env: chosen, template, dependencies, python: version.clone() };
                        }
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    if ui.button(crate::api_auth::CANCEL).clicked() {
                        out = PickerEvent::Cancel;
                    }
                });
            });
        });
        // The second page shows at once.
        if self.env != before {
            self.host.request_redraw();
        }
        out
    }
}

/// One selectable box (`iTermSelectableBox`): the glyph, the title and the text; true when
/// clicked.
fn choice(ui: &mut egui::Ui, icons: bool, glyph: char, title: &str, text: &str) -> bool {
    let inner = egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if icons {
                ui.label(egui::RichText::new(glyph.to_string()).family(egui::FontFamily::Name("icons".into())).size(40.0));
                ui.add_space(8.0);
            }
            ui.vertical(|ui| {
                ui.strong(title);
                ui.label(text);
            });
        });
    });
    let r = ui.interact(inner.response.rect, ui.id().with(title), egui::Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(inner.response.rect, 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.25));
    }
    r.clicked()
}
