//! Edit > Paste Special > Advanced Paste...: iTerm2's form (source, editable preview,
//! transforms, speed, "Wait for shell prompt before pasting each line", Paste, Cancel).

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::advanced_paste::{self as model, Form, Session};
use crate::egui_host::EguiHost;

pub enum AdvEvent {
    None,
    Paste,
    Cancel,
}

pub struct AdvancedPasteWindow {
    host: EguiHost,
    /// (label, text) of each source: history entries, then the clipboard.
    pub sources: Vec<(String, String)>,
    pub source: usize,
    pub form: Form,
    pub preview: String,
    pub session: Session,
    /// The remembered settings the form starts from (a new source starts from them again).
    defaults: crate::advanced_paste::Defaults,
}

impl AdvancedPasteWindow {
    #[allow(clippy::too_many_arguments)] // a parameter struct would only move the list; callers pass the pieces they already hold
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, sources: Vec<(String, String)>, source: usize, form: Form, session: Session, defaults: crate::advanced_paste::Defaults) -> Result<AdvancedPasteWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Advanced Paste", 520.0, 640.0)?;
        let text = sources.get(source).map(|s| s.1.clone()).unwrap_or_default();
        let preview = form.preview(&text);
        Ok(AdvancedPasteWindow { host, sources, source, form, preview, session, defaults })
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

    pub fn text(&self) -> String {
        self.sources.get(self.source).map(|s| s.1.clone()).unwrap_or_default()
    }

    pub fn redraw(&mut self) -> AdvEvent {
        let mut out = AdvEvent::None;
        let text = self.text();
        let en = self.form.enabled(&text, self.session);
        let before = self.form.clone();
        let before_source = self.source;
        let (form, preview, sources, source) = (&mut self.form, &mut self.preview, &self.sources, &mut self.source);
        self.host.paint(|ui| {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = AdvEvent::Cancel;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                out = AdvEvent::Paste;
            }
            egui::CentralPanel::default().show(ui, |ui| {
                let current = sources.get(*source).map(|s| s.0.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("source").width(470.0).selected_text(current).show_ui(ui, |ui| {
                    for (i, (label, _)) in sources.iter().enumerate() {
                        ui.selectable_value(source, i, label);
                    }
                });
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    // The preview can be edited; a transform change makes it again (iTerm2's).
                    ui.add(egui::TextEdit::multiline(preview).code_editor().desired_width(f32::INFINITY).desired_rows(6));
                });
                ui.label(model::stats(preview));
                ui.separator();
                ui.strong("Transforms");
                ui.add_enabled_ui(en.tabs, |ui| {
                    ui.radio_value(&mut form.tab_transform, 0, "Paste with tabs");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut form.tab_transform, 1, "Convert tabs to spaces:");
                        ui.add_enabled(en.spaces, egui::DragValue::new(&mut form.spaces).range(0..=100));
                    });
                    ui.radio_value(&mut form.tab_transform, 2, "Escape tabs with ^V");
                });
                ui.add_enabled(en.escape, egui::Checkbox::new(&mut form.escape, "Escape shell characters with \\"));
                ui.add_enabled(en.convert_newlines, egui::Checkbox::new(&mut form.convert_newlines, "Convert CRLF and LF to CR")).on_hover_text("Both DOS and UNIX newlines are generally not used as input. CR is standard.");
                ui.add_enabled(en.remove_newlines, egui::Checkbox::new(&mut form.remove_newlines, "Remove newlines")).on_hover_text("Both DOS and UNIX newlines are generally not used as input. CR is standard.");
                ui.checkbox(&mut form.base64, "Base-64 encode");
                ui.add_enabled(en.punctuation, egui::Checkbox::new(&mut form.punctuation, "Convert Unicode punctuation")).on_hover_text("Converts various forms of quotation marks and dashes to their ASCII equivalents.");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut form.use_regex, "Replace");
                    ui.add_enabled(form.use_regex, egui::TextEdit::singleline(&mut form.regex).hint_text("Regular Expression").desired_width(160.0));
                    ui.label("with");
                    ui.add_enabled(form.use_regex, egui::TextEdit::singleline(&mut form.substitution).hint_text("Substitution").desired_width(140.0));
                });
                ui.small("Use $1, $2, etc. for capture groups. Respects vim escape sequences. ICU regular expression syntax.");
                ui.add_enabled(en.remove_controls, egui::Checkbox::new(&mut form.remove_controls, "Remove unsafe control codes")).on_hover_text("Tab, line feed, form feed, and carriage return are the only \"safe\" control codes.");
                ui.add_enabled(en.bracket, egui::Checkbox::new(&mut form.bracket, "Bracketed paste mode"));
                ui.separator();
                ui.strong("Speed");
                let mut chunk = model::units_to_slider(form.chunk);
                ui.horizontal(|ui| {
                    ui.label("Chunk Size:");
                    if ui.add(egui::Slider::new(&mut chunk, 0.0..=1.0).show_value(false)).changed() {
                        form.chunk = model::slider_to_units(chunk);
                    }
                    ui.label(crate::paste::format_bytes(form.chunk as f64));
                });
                let mut delay = model::delay_to_slider(form.delay);
                ui.horizontal(|ui| {
                    ui.label("Interchunk Delay:");
                    if ui.add(egui::Slider::new(&mut delay, 0.0..=1.0).show_value(false)).changed() {
                        form.delay = model::slider_to_delay(delay);
                    }
                    ui.label(format!("{:.0} ms", form.delay * 1000.0));
                });
                ui.add_enabled(en.wait, egui::Checkbox::new(&mut form.wait, "Wait for shell prompt before pasting each line"));
                ui.small("Requires Shell Integration for prompt detection");
                ui.label(format!("Minimum duration: {}", model::duration(preview.encode_utf16().count(), crate::paste::Speed { units: form.chunk.max(1), delay: form.delay })));
                ui.horizontal(|ui| {
                    if ui.button("Paste").clicked() {
                        out = AdvEvent::Paste;
                    }
                    if ui.button("Cancel").clicked() {
                        out = AdvEvent::Cancel;
                    }
                });
            });
        });
        // A new source or transform shows a new preview (speed changes do not).
        let transforms_changed = { let mut b = before.clone(); b.chunk = self.form.chunk; b.delay = self.form.delay; b.wait = self.form.wait; b.bracket = self.form.bracket; b.tab_transform = self.form.tab_transform; b.spaces = self.form.spaces; b != self.form };
        // A new source sets every control again from its text (iTerm2's selectValueAtIndex:);
        // the speed stays.
        if self.source != before_source {
            let text = self.text();
            let (chunk, delay) = (self.form.chunk, self.form.delay);
            self.form = Form { chunk, delay, ..Form::new(&text, &self.defaults, self.session) };
        }
        if self.source != before_source || transforms_changed {
            let text = self.text();
            self.preview = self.form.preview(&text);
            self.host.request_redraw();
        }
        out
    }
}
