//! Session Status' settings popover (`StatusPrioritySettings.swift`, `SPS:197-365`): the
//! priority patterns with their Notify boxes, reordered by dragging and edited in place, + and
//! -, and the two checkboxes; `InfoPopover`, a plain two-column popover (Jobs' Inspect); and
//! `TextPopover`, a script component's popover of text.
//! Both are small frameless egui windows that close when they lose focus (a transient
//! popover) or on Esc, as the git Log popover does.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;
use crate::layout::Rect;
use crate::status_tool::{self, Pattern};

/// The settings popover's size, points (`SPS`, `contentSize`).
const SETTINGS_W: f64 = 280.0;
pub const SETTINGS_H: f64 = 322.0;
/// Its margin, and the Notify column's width, points (`SPS:273-279`, `notifyColumnWidth`).
const MARGIN: f32 = 10.0;
const NOTIFY_W: f32 = 44.0;
/// The information popover's width, and a line's height, points.
const INFO_W: f64 = 320.0;
const INFO_LINE: f64 = 20.0;

/// What the settings popover asks of the app after a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum PopoverEvent {
    None,
    /// The patterns as edited (a pattern's text, a Notify box, a drag, or -).
    Patterns(Vec<Pattern>),
    Merge(bool),
    Subtitle(bool),
    /// + was pressed: the app asks for the new pattern (`NEW_PATTERN`).
    AddPattern,
    Close,
}

/// A frameless, taskbar-less window of `w` by `h` points, its bottom centered on `at` (screen
/// pixels) as a popover above its button (`preferredEdge: .maxY`), kept on `work`, the work
/// area of the button's monitor (`place`).
#[allow(clippy::too_many_arguments)]
fn popover_window(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, title: &str, w: f64, h: f64, at: (f64, f64), work: Option<Rect>) -> Result<EguiHost, String> {
    use winit::platform::windows::WindowAttributesExtWindows;
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_decorations(false)
        .with_skip_taskbar(true)
        .with_visible(false)
        .with_inner_size(winit::dpi::LogicalSize::new(w, h));
    let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
    let scale = host.window.scale_factor();
    let (pw, ph) = (w * scale, h * scale);
    let (x, y) = place(at, pw, ph, work);
    host.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    host.window.set_visible(true);
    host.focus();
    Ok(host)
}

/// The content size of a script's popover in points: the size asked for, at least one,
/// kept inside `work` (the monitor's work area, screen pixels) and the largest surface the
/// device draws (`max_px`), at `scale` pixels per point. A script gives any 32-bit size.
pub fn popover_size(size: (f64, f64), work: Option<Rect>, scale: f64, max_px: f64) -> (f64, f64) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let cap = |v: f64, room: Option<f32>| {
        let most = room.map_or(max_px, |r| f64::from(r).min(max_px)) / scale;
        v.min(most).max(1.0)
    };
    (cap(size.0, work.map(|r| r.w)), cap(size.1, work.map(|r| r.h)))
}

/// Where a popover of `pw` by `ph` pixels goes: its bottom centered on `at`, moved inside
/// `work` (the monitor's work area, screen pixels; its top-left edge wins when it is too
/// small), or kept off negative coordinates without one.
fn place(at: (f64, f64), pw: f64, ph: f64, work: Option<Rect>) -> (f64, f64) {
    let (x, y) = (at.0 - pw / 2.0, at.1 - ph);
    match work {
        Some(r) => {
            let (l, t) = (f64::from(r.x), f64::from(r.y));
            let (rr, b) = (l + f64::from(r.w), t + f64::from(r.h));
            (x.min(rr - pw).max(l), y.min(b - ph).max(t))
        }
        None => (x.max(0.0), y.max(0.0)),
    }
}

/// Whether a popover event closes it: its own close, or a focus loss after it had focus (the
/// first loss can be the window being made).
pub(crate) fn closes(event: &WindowEvent, focused: &mut bool, hold: bool) -> Option<bool> {
    match event {
        WindowEvent::CloseRequested => Some(true),
        WindowEvent::Focused(true) => {
            *focused = true;
            Some(false)
        }
        WindowEvent::Focused(false) => Some(*focused && !hold),
        _ => None,
    }
}

pub struct StatusPopover {
    host: EguiHost,
    patterns: Vec<Pattern>,
    merge: bool,
    subtitle: bool,
    /// The row last clicked or edited: what - removes.
    selected: Option<usize>,
    /// A pattern's text changed and is not yet sent (sent when its field lets go).
    dirty: bool,
    focused: bool,
    /// Kept open through a focus loss: the new-pattern prompt has the focus.
    pub hold: bool,
}

impl StatusPopover {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        patterns: Vec<Pattern>,
        merge: bool,
        subtitle: bool,
        at: (f64, f64),
        work: Option<Rect>,
        dark: bool,
    ) -> Result<StatusPopover, String> {
        let host = popover_window(event_loop, instance, adapter, device, queue, "Session Status", SETTINGS_W, SETTINGS_H, at, work)?;
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        Ok(StatusPopover { host, patterns, merge, subtitle, selected: None, dirty: false, focused: false, hold: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
        self.host.request_redraw();
    }

    /// The patterns changed elsewhere (a pattern the prompt added): shown as they are now.
    pub fn set_patterns(&mut self, patterns: Vec<Pattern>) {
        self.patterns = patterns;
        self.selected = None;
        self.dirty = false;
        self.host.request_redraw();
    }

    /// A pattern edit not yet sent (its field still has the focus): the patterns with it, once.
    /// The app reads this before the popover closes or its patterns are replaced, so a focus
    /// loss or a + answer does not drop the edit.
    pub fn pending_patterns(&mut self) -> Option<Vec<Pattern>> {
        std::mem::take(&mut self.dirty).then(|| self.patterns.clone())
    }

    /// Handles one of its window's events.
    pub fn event(&mut self, event: &WindowEvent) -> PopoverEvent {
        if let Some(close) = closes(event, &mut self.focused, self.hold) {
            return if close { PopoverEvent::Close } else { PopoverEvent::None };
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.redraw();
        }
        self.host.event(event);
        PopoverEvent::None
    }

    /// Draws a frame: the label, the table, + and -, the checkboxes (bottom up, `SPS:230-310`).
    pub fn redraw(&mut self) -> PopoverEvent {
        let mut out = PopoverEvent::None;
        let (patterns, merge, subtitle, selected, dirty) = (&mut self.patterns, &mut self.merge, &mut self.subtitle, &mut self.selected, &mut self.dirty);
        self.host.paint(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(MARGIN)).show(ui, |ui| {
                ui.label(egui::RichText::new(status_tool::SETTINGS_TEXT).small().weak());
                ui.add_space(4.0);
                // The table takes what the controls below leave.
                let controls = 24.0 + 3.0 * 8.0 + 2.0 * 18.0;
                let table_h = (ui.available_height() - controls).max(40.0);
                let text_w = (ui.available_width() - NOTIFY_W - 24.0).max(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.add_sized([text_w, 16.0], egui::Label::new(egui::RichText::new("Pattern").strong()));
                    ui.add_sized([NOTIFY_W, 16.0], egui::Label::new(egui::RichText::new("Notify").strong()));
                });
                let mut moved: Option<(usize, usize)> = None;
                let mut changed = false;
                egui::ScrollArea::vertical().max_height(table_h).auto_shrink([false, false]).show(ui, |ui| {
                    for (i, p) in patterns.iter_mut().enumerate() {
                        let row = ui.horizontal(|ui| {
                            // The grip drags the row to a new place (`supportsReorder`).
                            ui.dnd_drag_source(egui::Id::new(("status-pattern", i)), i, |ui| {
                                ui.label("::");
                            });
                            let r = ui.add(egui::TextEdit::singleline(&mut p.pattern).desired_width(text_w));
                            if r.gained_focus() || r.clicked() {
                                *selected = Some(i);
                            }
                            if r.changed() {
                                *dirty = true;
                            }
                            // Click to edit: the edit lands when the field lets go.
                            if r.lost_focus() && *dirty {
                                *dirty = false;
                                changed = true;
                            }
                            ui.add_space(((NOTIFY_W - 18.0) / 2.0).max(0.0));
                            if ui.checkbox(&mut p.notify, "").changed() {
                                *selected = Some(i);
                                changed = true;
                            }
                        });
                        if let Some(from) = row.response.dnd_release_payload::<usize>() {
                            moved = Some((*from, i));
                        }
                    }
                });
                if let Some((from, to)) = moved.filter(|(a, b)| a != b) {
                    let item = patterns.remove(from);
                    patterns.insert(to, item);
                    *selected = Some(to);
                    changed = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("+").clicked() {
                        out = PopoverEvent::AddPattern;
                    }
                    let can_remove = selected.is_some_and(|i| i < patterns.len());
                    if ui.add_enabled(can_remove, egui::Button::new("\u{2212}")).clicked() {
                        if let Some(i) = selected.take() {
                            patterns.remove(i);
                            *dirty = false;
                            changed = true;
                        }
                    }
                });
                if changed {
                    out = PopoverEvent::Patterns(patterns.clone());
                }
                ui.add_space(8.0);
                if ui.checkbox(merge, status_tool::MERGE_LABEL.0).on_hover_text(status_tool::MERGE_LABEL.1).changed() {
                    out = PopoverEvent::Merge(*merge);
                }
                ui.add_space(8.0);
                if ui.checkbox(subtitle, status_tool::SUBTITLE_LABEL).changed() {
                    out = PopoverEvent::Subtitle(*subtitle);
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = PopoverEvent::Close;
            }
        });
        out
    }
}

/// A plain popover of label and value lines (Jobs' Inspect).
pub struct InfoPopover {
    host: EguiHost,
    lines: Vec<(String, String)>,
    focused: bool,
}

impl InfoPopover {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        lines: Vec<(String, String)>,
        at: (f64, f64),
        work: Option<Rect>,
        dark: bool,
    ) -> Result<InfoPopover, String> {
        let h = lines.len().max(1) as f64 * INFO_LINE + 2.0 * f64::from(MARGIN);
        let host = popover_window(event_loop, instance, adapter, device, queue, "Info", INFO_W, h, at, work)?;
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        Ok(InfoPopover { host, lines, focused: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// Handles one of its window's events. False when it should close.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let Some(close) = closes(event, &mut self.focused, false) {
            return !close;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.redraw();
        }
        self.host.event(event);
        true
    }

    fn redraw(&mut self) -> bool {
        let mut open = true;
        let lines = &self.lines;
        self.host.paint(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(MARGIN)).show(ui, |ui| {
                egui::Grid::new("info-popover").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for (label, value) in lines {
                        ui.label(egui::RichText::new(label.as_str()).weak());
                        // Selectable, so a value can be copied.
                        ui.add(egui::Label::new(value.as_str()).selectable(true));
                        ui.end_row();
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        });
        open
    }
}

/// The popover a script opens from its status bar component (StatusBarComponentRequest,
/// `BASE:363-389`): iTerm2 shows the HTML in a web view; Tandem shows its text
/// (`script_statusbar::html_text`), selectable and scrolling, in a content area of the
/// requested size. It closes when it loses focus (a semitransient popover) or on Esc.
pub struct TextPopover {
    host: EguiHost,
    text: String,
    focused: bool,
}

impl TextPopover {
    /// `size`: the content area in points (at least one); `at`: the point its bottom centers
    /// on, screen pixels; `work`: the work area of that point's monitor.
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, text: &str, size: (f64, f64), at: (f64, f64), work: Option<Rect>) -> Result<TextPopover, String> {
        let (w, h) = (size.0.max(1.0), size.1.max(1.0));
        let host = popover_window(event_loop, instance, adapter, device, queue, "Popover", w, h, at, work)?;
        Ok(TextPopover { host, text: text.to_string(), focused: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// Handles one of its window's events. False when it should close.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let Some(close) = closes(event, &mut self.focused, false) {
            return !close;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.redraw();
        }
        self.host.event(event);
        true
    }

    fn redraw(&mut self) -> bool {
        let mut open = true;
        let text = &self.text;
        self.host.paint(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(MARGIN)).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(text.as_str()).selectable(true).wrap());
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        });
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popover_stays_on_its_buttons_monitor() {
        // A second monitor to the right of the first, its work area 1920..3840 by 0..1040.
        let work = Some(Rect { x: 1920.0, y: 0.0, w: 1920.0, h: 1040.0 });
        // Above the button, centered, when it fits.
        assert_eq!(place((2500.0, 600.0), 200.0, 300.0, work), (2400.0, 300.0));
        // Near the monitor's left edge it stays on that monitor, not on the first.
        assert_eq!(place((1950.0, 600.0), 200.0, 300.0, work), (1920.0, 300.0));
        // Near its right edge and its top.
        assert_eq!(place((3830.0, 100.0), 200.0, 300.0, work), (3640.0, 0.0));
        // Too small: the top-left edge wins.
        assert_eq!(place((2000.0, 100.0), 4000.0, 300.0, work), (1920.0, 0.0));
        // No monitor known: only kept off negative coordinates.
        assert_eq!(place((50.0, 100.0), 200.0, 300.0, None), (0.0, 0.0));
    }

    /// Review finding 4: a script's popover size is kept inside the work area and the
    /// device's largest surface, and at least one point.
    #[test]
    fn popover_size_is_bounded() {
        let work = Some(Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
        assert_eq!(popover_size((300.0, 200.0), work, 1.0, 8192.0), (300.0, 200.0));
        assert_eq!(popover_size((2147483647.0, 2147483647.0), work, 1.0, 8192.0), (1920.0, 1040.0));
        assert_eq!(popover_size((5000.0, 5000.0), work, 2.0, 8192.0), (960.0, 520.0), "points at 2 pixels each");
        assert_eq!(popover_size((0.0, -5.0), work, 1.0, 8192.0), (1.0, 1.0));
        assert_eq!(popover_size((f64::NAN, 1e12), None, 1.0, 8192.0), (8192.0, 8192.0), "no monitor: the device limit");
        assert_eq!(popover_size((100.0, 100.0), work, 0.0, 8192.0), (100.0, 100.0), "a bad scale reads as 1");
    }
}
