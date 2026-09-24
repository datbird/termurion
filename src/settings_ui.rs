//! The settings window, laid out like iTerm2's profile editor: a profile list on the
//! left, the eight tabs across the top, and one control per schema setting.

use std::sync::Arc;

use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::profile::{self, Profile};
use crate::schema::{self, Kind, Setting};
use crate::store::Store;

pub fn sections(tab: &str) -> Vec<(String, Vec<&'static Setting>)> {
    let mut out: Vec<(String, Vec<&'static Setting>)> = Vec::new();
    for s in schema::profile().iter().filter(|s| s.tab == tab) {
        match out.iter_mut().find(|(name, _)| *name == s.section) {
            Some((_, list)) => list.push(s),
            None => out.push((s.section.clone(), vec![s])),
        }
    }
    out
}

pub fn rgb_of(v: &Value) -> [u8; 3] {
    let t = v.as_table();
    let c = |k: &str| {
        let f = t.and_then(|t| t.get(k)).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
        (f.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [c("r"), c("g"), c("b")]
}

pub fn value_of_rgb(c: [u8; 3]) -> Value {
    profile::color_value(crate::palette::rgb(c[0], c[1], c[2]))
}

pub enum Edit {
    None,
    Changed(String),
    Imported(usize),
    Error(String),
    /// Open a session with this profile Guid.
    Open(String, How),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Tab,
    Window,
    SplitRight,
    SplitDown,
}

/// The settings editor, or iTerm2's Profiles window (Cmd+O): a list to start sessions from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Settings,
    Profiles,
}

pub struct SettingsWindow {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    selected: Option<String>,
    tab: usize,
    mode: Mode,
    window: Arc<Window>,
}

impl SettingsWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, mode: Mode) -> Result<Self, String> {
        let (title, w, h) = match mode {
            Mode::Settings => ("Tandem Settings", 980.0, 720.0),
            Mode::Profiles => ("Profiles", 420.0, 460.0),
        };
        let attrs = Window::default_attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(w, h));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &config);
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, None);
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Ok(SettingsWindow { surface, config, device: device.clone(), queue: queue.clone(), ctx, state, renderer, selected: None, tab: 0, mode, window })
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Feeds a window event to egui. Returns true when the window should redraw.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(s) = event {
            if s.width > 0 && s.height > 0 {
                self.config.width = s.width;
                self.config.height = s.height;
                self.surface.configure(&self.device, &self.config);
            }
        }
        let r = self.state.on_window_event(&self.window, event);
        if r.repaint {
            self.window.request_redraw();
        }
        r.repaint
    }

    pub fn redraw(&mut self, store: &mut Store) -> Edit {
        let input = self.state.take_egui_input(&self.window);
        let mut edit = Edit::None;
        let selected = self.selected.get_or_insert_with(|| store.default_guid.clone()).clone();
        let mut tab = self.tab;
        let mut pick = None;
        let dark = self.window.theme().is_none_or(|t| t == winit::window::Theme::Dark);
        let mode = self.mode;
        let out = self.ctx.run_ui(input, |ui| {
            if mode == Mode::Profiles {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Profiles");
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for p in &store.profiles {
                            let r = ui.selectable_label(p.guid() == selected, p.name());
                            if r.clicked() {
                                pick = Some(p.guid());
                            }
                            if r.double_clicked() {
                                edit = Edit::Open(p.guid(), How::Tab);
                            }
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        for (label, how) in [("New Tab", How::Tab), ("New Window", How::Window), ("Split Right", How::SplitRight), ("Split Down", How::SplitDown)] {
                            if ui.button(label).clicked() {
                                edit = Edit::Open(selected.clone(), how);
                            }
                        }
                    });
                });
                return;
            }
            egui::Panel::left("profiles").default_size(220.0).show(ui, |ui| {
                ui.heading("Profiles");
                for p in &store.profiles {
                    let star = if p.guid() == store.default_guid { " (default)" } else { "" };
                    if ui.selectable_label(p.guid() == selected, format!("{}{star}", p.name())).clicked() {
                        pick = Some(p.guid());
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("+").on_hover_text("New profile").clicked() {
                        let p = Profile::new("New Profile");
                        pick = Some(p.guid());
                        store.add(p);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("Duplicate").clicked() {
                        pick = store.duplicate(&selected);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("-").on_hover_text("Delete profile").clicked() && store.delete(&selected) {
                        pick = Some(store.default_guid.clone());
                        edit = Edit::Changed(String::new());
                    }
                });
                if ui.button("Set as Default").clicked() {
                    store.default_guid = selected.clone();
                    edit = Edit::Changed(selected.clone());
                }
                if ui.button("Import from iTerm2...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("iTerm2 settings or Dynamic Profiles", &["plist", "json"]).pick_file() {
                        edit = match crate::import::from_file(&path) {
                            Ok(list) => {
                                let n = list.len();
                                for p in list {
                                    store.profiles.retain(|q| q.guid() != p.guid());
                                    store.add(p);
                                }
                                Edit::Imported(n)
                            }
                            Err(e) => Edit::Error(e),
                        };
                    }
                }
            });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, name) in schema::TABS.iter().enumerate() {
                        if ui.selectable_label(tab == i, *name).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.separator();
                let Some(profile) = store.get_mut(&selected) else { return };
                if profile.bool("Use Separate Colors for Light and Dark Mode") && schema::TABS[tab] == "Colors" {
                    ui.label(format!("Editing the {} mode colors (Windows is in {} mode).", if dark { "Dark" } else { "Light" }, if dark { "dark" } else { "light" }));
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (section, list) in sections(schema::TABS[tab]) {
                        if !section.is_empty() {
                            ui.add_space(8.0);
                            ui.strong(section);
                        }
                        for s in list {
                            if control(ui, profile, s, dark) {
                                edit = Edit::Changed(selected.clone());
                            }
                        }
                    }
                });
            });
        });
        self.tab = tab;
        if let Some(g) = pick {
            self.selected = Some(g);
        }
        self.state.handle_platform_output(&self.window, out.platform_output);
        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: out.pixels_per_point };
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            _ => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return edit;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("settings") });
        let extra = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        self.queue.submit(extra.into_iter().chain(Some(encoder.finish())));
        self.queue.present(frame);
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
        edit
    }
}

/// The key a control edits. With separate Light and Dark colors on, a color with
/// variants edits the variant for the current mode, as iTerm2's editor does.
pub fn edit_key(profile: &Profile, s: &Setting, dark: bool) -> String {
    if s.light_dark && profile.bool("Use Separate Colors for Light and Dark Mode") {
        format!("{} ({})", s.key, if dark { "Dark" } else { "Light" })
    } else {
        s.key.clone()
    }
}

/// One control for one setting. Returns true when the user changed it.
fn control(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting, dark: bool) -> bool {
    let key = edit_key(profile, s, dark);
    let current = if key == s.key {
        profile.get(&s.key)
    } else {
        let stock = if dark { s.default_dark.clone() } else { s.default_light.clone() };
        profile.values.get(&key).cloned().or(stock).or_else(|| profile.get(&s.key))
    };
    let mut changed = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!s.macos_only, |ui| {
            match s.kind {
                Kind::Bool => {
                    let mut b = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    if ui.checkbox(&mut b, &s.label).changed() {
                        changed = Some(Value::Boolean(b));
                    }
                }
                Kind::Int => {
                    ui.label(&s.label);
                    let mut i = current.as_ref().and_then(|v| v.as_integer()).unwrap_or(0);
                    let mut w = egui::DragValue::new(&mut i);
                    if let (Some(a), Some(b)) = (s.min_f64(), s.max_f64()) {
                        w = w.range(a as i64..=b as i64);
                    }
                    if ui.add(w).changed() {
                        changed = Some(Value::Integer(i));
                    }
                }
                Kind::Float => {
                    ui.label(&s.label);
                    let mut f = current.as_ref().and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
                    let r = match (s.min_f64(), s.max_f64()) {
                        (Some(a), Some(b)) => ui.add(egui::Slider::new(&mut f, a..=b)),
                        _ => ui.add(egui::DragValue::new(&mut f).speed(0.01)),
                    };
                    if r.changed() {
                        changed = Some(Value::Float(f));
                    }
                }
                Kind::String | Kind::Font => {
                    ui.label(&s.label);
                    let mut text = current.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if ui.text_edit_singleline(&mut text).changed() {
                        changed = Some(Value::String(text));
                    }
                }
                Kind::Color => {
                    ui.label(&s.label);
                    let mut c = current.as_ref().map(rgb_of).unwrap_or([0, 0, 0]);
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        changed = Some(value_of_rgb(c));
                    }
                }
                Kind::Enum => {
                    ui.label(&s.label);
                    let now = current.clone();
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or("".to_string(), |c| c.label.clone());
                    egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
                        for c in &s.choices {
                            if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                changed = Some(c.value.clone());
                            }
                        }
                    });
                }
                Kind::List | Kind::Dict | Kind::Data => {
                    let n = match &current {
                        Some(Value::Array(a)) => a.len(),
                        Some(Value::Table(t)) => t.len(),
                        _ => 0,
                    };
                    ui.label(format!("{}: {n} item(s)", s.label)).on_hover_text("This editor arrives with its feature's milestone.");
                }
            }
        })
        .response
        .on_hover_text(if s.macos_only { format!("{} (macOS only, no Windows equivalent)", s.help) } else { s.help.clone() });
    });
    match changed {
        Some(v) => {
            profile.set(&key, v);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_has_sections_in_schema_order() {
        for tab in crate::schema::TABS {
            let s = sections(tab);
            assert!(!s.is_empty(), "{tab}");
            let keys: Vec<&str> = s.iter().flat_map(|(_, v)| v.iter().map(|x| x.key.as_str())).collect();
            let schema: Vec<&str> = crate::schema::profile().iter().filter(|x| x.tab == tab).map(|x| x.key.as_str()).collect();
            let mut sorted = schema.clone();
            sorted.retain(|k| keys.contains(k));
            assert_eq!(keys.len(), schema.len(), "{tab} lost settings");
        }
    }

    #[test]
    fn color_edits_target_the_current_mode_variant() {
        let fg = crate::schema::find("Foreground Color").unwrap();
        let mut p = Profile::default();
        assert_eq!(edit_key(&p, fg, true), "Foreground Color (Dark)");
        assert_eq!(edit_key(&p, fg, false), "Foreground Color (Light)");
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(false));
        assert_eq!(edit_key(&p, fg, true), "Foreground Color");
        let cols = crate::schema::find("Columns").unwrap();
        assert_eq!(edit_key(&Profile::default(), cols, true), "Columns");
    }

    #[test]
    fn colors_convert_both_ways() {
        let v = value_of_rgb([255, 128, 0]);
        assert_eq!(rgb_of(&v), [255, 128, 0]);
    }

    #[test]
    fn cmd_comma_opens_settings() {
        use crate::keys::{Action, Key, shortcut};
        use crate::mouse::Mods;
        assert_eq!(shortcut(&Key::Text(",".into()), Mods { ctrl: true, shift: true, alt: false }), Some(Action::OpenSettings));
    }
}
