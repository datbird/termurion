//! The Make Screenshot panel: a minimap with the chosen line range, a preview, the line
//! fields, the Redact and Highlight annotations, the redaction method, and Cancel, Copy to
//! Clipboard, Save As..., and Save.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::screenshot::{Kind, Method, Panel};

/// What the user did in the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShotEvent {
    None,
    /// The range, method, or annotations changed: the preview needs drawing again.
    Changed,
    Annotate(Kind),
    Copy,
    SaveAs,
    Save,
    Cancel,
}

#[derive(Clone, Copy)]
enum Drag {
    Top,
    Bottom,
    Move(isize),
}

pub struct ShotWindow {
    host: EguiHost,
    pub panel: Panel,
    /// The pane has a selection to annotate.
    pub can_annotate: bool,
    /// How many files a save writes (Copy is off past one).
    pub parts: usize,
    preview: Option<egui::TextureHandle>,
    preview_size: (f32, f32),
    minimap: Option<egui::TextureHandle>,
    new_preview: Option<egui::ColorImage>,
    new_minimap: Option<egui::ColorImage>,
    selected: Option<usize>,
    copied_until: Option<Instant>,
    drag: Option<Drag>,
}

impl ShotWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        panel: Panel,
    ) -> Result<ShotWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Make Screenshot", 612.0, 720.0)?;
        Ok(ShotWindow {
            host,
            panel,
            can_annotate: false,
            parts: 1,
            preview: None,
            preview_size: (0.0, 0.0),
            minimap: None,
            new_preview: None,
            new_minimap: None,
            selected: None,
            copied_until: None,
            drag: None,
        })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn window(&self) -> &winit::window::Window {
        &self.host.window
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    /// A new preview image (RGBA), shown fitted into the preview box.
    pub fn set_preview(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.new_preview = Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba));
        self.preview_size = (w as f32, h as f32);
        self.host.request_redraw();
    }

    pub fn set_minimap(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.new_minimap = Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba));
        self.host.request_redraw();
    }

    /// Copy to Clipboard reads "Copied!" for a moment.
    pub fn copied(&mut self) {
        self.copied_until = Some(Instant::now() + Duration::from_millis(1200));
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> ShotEvent {
        let mut out = ShotEvent::None;
        let Self { host, panel, can_annotate, parts, preview, preview_size, minimap, new_preview, new_minimap, selected, copied_until, drag } = self;
        let copied = copied_until.is_some_and(|t| Instant::now() < t);
        host.paint(|ui| {
            if let Some(img) = new_preview.take() {
                *preview = Some(ui.ctx().load_texture("preview", img, egui::TextureOptions::LINEAR));
            }
            if let Some(img) = new_minimap.take() {
                *minimap = Some(ui.ctx().load_texture("minimap", img, egui::TextureOptions::LINEAR));
            }
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if minimap_view(ui, panel, minimap.as_ref(), drag) {
                        out = ShotEvent::Changed;
                    }
                    egui::ScrollArea::both().max_height(300.0).show(ui, |ui| {
                        if let Some(tex) = preview.as_ref() {
                            let (w, h) = *preview_size;
                            let ppp = ui.ctx().pixels_per_point();
                            let fit = (400.0 / (w / ppp)).min(300.0 / (h / ppp)).min(1.0);
                            let size = egui::vec2(w / ppp * fit, h / ppp * fit);
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), size)));
                        } else {
                            ui.label("Drawing...");
                        }
                    });
                });
                if *parts > 1 {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Large screenshot will be saved as {parts} files."));
                }
                ui.horizontal(|ui| {
                    ui.label("Line range:");
                    let (mut a, mut b) = (panel.range.0 + 1, panel.range.1 + 1);
                    let total = panel.total.max(1);
                    let r1 = ui.add(egui::DragValue::new(&mut a).range(1..=total));
                    ui.label("to");
                    let r2 = ui.add(egui::DragValue::new(&mut b).range(1..=total));
                    if r1.changed() || r2.changed() {
                        panel.set_range_1based(a, b);
                    }
                    if r1.drag_stopped() || r1.lost_focus() || r2.drag_stopped() || r2.lost_focus() {
                        out = ShotEvent::Changed;
                    }
                });
                ui.add_space(18.0);
                ui.strong("Annotations:");
                ui.weak("Select text in the terminal, then click a button below.");
                ui.horizontal(|ui| {
                    if ui.add_enabled(*can_annotate, egui::Button::new("Redact Selection")).clicked() {
                        out = ShotEvent::Annotate(Kind::Redact);
                    }
                    if ui.add_enabled(*can_annotate, egui::Button::new("Highlight Selection")).clicked() {
                        out = ShotEvent::Annotate(Kind::Highlight);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!panel.annotations.is_empty(), egui::Button::new("Clear All")).clicked() {
                            panel.annotations.clear();
                            *selected = None;
                            out = ShotEvent::Changed;
                        }
                        if ui.add_enabled(selected.is_some(), egui::Button::new("-")).clicked() {
                            if let Some(i) = selected.take() {
                                if i < panel.annotations.len() {
                                    panel.annotations.remove(i);
                                }
                            }
                            out = ShotEvent::Changed;
                        }
                    });
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_height(100.0);
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(100.0).show(ui, |ui| {
                        if panel.annotations.is_empty() {
                            ui.centered_and_justified(|ui| ui.weak("No annotations. Select text and click a button above."));
                        }
                        for (i, a) in panel.annotations.iter().enumerate() {
                            if ui.selectable_label(*selected == Some(i), &a.label).clicked() {
                                *selected = Some(i);
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("Redaction method:");
                    let blur = matches!(panel.method, Method::Blur(_));
                    if ui.selectable_label(blur, "Blur").clicked() && !blur {
                        panel.method = Method::Blur(8.0);
                        out = ShotEvent::Changed;
                    }
                    if ui.selectable_label(!blur, "Solid Color").clicked() && blur {
                        panel.method = Method::Solid([0, 0, 0]);
                        out = ShotEvent::Changed;
                    }
                });
                ui.horizontal(|ui| match &mut panel.method {
                    Method::Blur(r) => {
                        ui.label("Blur radius:");
                        let s = ui.add(egui::Slider::new(r, 1.0..=200.0).integer());
                        if s.drag_stopped() || (s.changed() && !s.dragged()) {
                            out = ShotEvent::Changed;
                        }
                    }
                    Method::Solid(c) => {
                        ui.label("Fill color:");
                        if ui.color_edit_button_srgb(c).changed() {
                            out = ShotEvent::Changed;
                        }
                    }
                });
                ui.add_space(18.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        out = ShotEvent::Save;
                    }
                    if ui.button("Save As...").clicked() {
                        out = ShotEvent::SaveAs;
                    }
                    let label = if copied { "Copied!" } else { "Copy to Clipboard" };
                    if ui.add_enabled(*parts <= 1, egui::Button::new(label)).clicked() {
                        out = ShotEvent::Copy;
                    }
                    if ui.button("Cancel").clicked() {
                        out = ShotEvent::Cancel;
                    }
                });
            });
        });
        if copied {
            self.host.request_redraw();
        }
        out
    }
}

/// The minimap with the range drawn on it; dragging the ends or the middle moves the range.
/// True when a drag ended (the preview needs drawing again).
fn minimap_view(ui: &mut egui::Ui, panel: &mut Panel, tex: Option<&egui::TextureHandle>, drag: &mut Option<Drag>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(60.0, 300.0), egui::Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let inner = rect.shrink(2.0);
    if let Some(t) = tex {
        p.image(t.id(), inner, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    let total = panel.total.max(1) as f32;
    let y_of = |line: usize| inner.top() + line as f32 / total * inner.height();
    let line_of = |y: f32| (((y - inner.top()) / inner.height() * total).floor().max(0.0) as usize).min(panel.total.saturating_sub(1));
    let (top, bottom) = (y_of(panel.range.0), y_of(panel.range.1 + 1));
    let dim = egui::Color32::from_black_alpha(128);
    p.rect_filled(egui::Rect::from_min_max(inner.min, egui::pos2(inner.right(), top)), 0.0, dim);
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(inner.left(), bottom), inner.max), 0.0, dim);
    let accent = ui.visuals().selection.bg_fill;
    let range = egui::Rect::from_min_max(egui::pos2(inner.left(), top), egui::pos2(inner.right(), bottom));
    p.rect_stroke(range, 0.0, egui::Stroke::new(2.0, accent), egui::StrokeKind::Outside);
    for y in [top, bottom] {
        p.rect_filled(egui::Rect::from_center_size(egui::pos2(inner.center().x, y), egui::vec2(inner.width(), 4.0)), 1.0, accent);
    }
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *drag = if (pos.y - top).abs() <= 12.0 {
                Some(Drag::Top)
            } else if (pos.y - bottom).abs() <= 12.0 {
                Some(Drag::Bottom)
            } else if pos.y > top && pos.y < bottom {
                Some(Drag::Move(line_of(pos.y) as isize - panel.range.0 as isize))
            } else {
                None
            };
        }
    }
    if let (Some(d), Some(pos)) = (*drag, resp.interact_pointer_pos()) {
        let at = line_of(pos.y);
        let (s, e) = panel.range;
        let last = panel.total.saturating_sub(1);
        panel.range = match d {
            // Handles cannot make a one-line range; the fields can.
            Drag::Top => (at.min(e.saturating_sub(1)), e),
            Drag::Bottom => (s, at.max(s + 1).min(last)),
            Drag::Move(off) => {
                let len = e - s;
                let start = (at as isize - off).clamp(0, (last - len) as isize) as usize;
                (start, start + len)
            }
        };
    }
    if resp.drag_stopped() && drag.take().is_some() {
        return true;
    }
    false
}
