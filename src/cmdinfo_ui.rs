//! The Command Info window (`CommandInfoViewController`): a small frameless window at the click
//! with the command's fields and buttons. It closes on Esc or when it loses the focus (a click
//! outside), as a transient popover does. It only draws and reports a button as an `Act`;
//! `app/cmdinfo_app` does what the button means.

use std::time::{Duration, Instant, SystemTime};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::cmdinfo::{self, Info};
use crate::egui_host::EguiHost;
use crate::layout::{PaneId, Rect};

/// What a button asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    WriteCommand,
    CopyCommand,
    AddSnippet,
    CopyUrl,
    WriteDirectory,
    CopyDirectory,
    CopyOutput,
    SelectOutput,
    Composer,
}

/// What a frame or an event asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdInfoEvent {
    None,
    Close,
    Act(Act),
}

/// A toast under a button.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Toast {
    Copied { x: f32, y: f32 },
    SnippetAdded,
}

/// The gap between redraws while the running time ticks (P15: the cadence, never a free run).
const FRAME_GAP: Duration = Duration::from_millis(33);
/// The label column's width, points.
const LABEL_W: f32 = 96.0;
/// A button's size, points.
const BUTTON: [f32; 2] = [28.0, 24.0];

pub struct CmdInfoWindow {
    host: EguiHost,
    pub pane: PaneId,
    info: Info,
    /// "Started at <time> on <date>", with a start time only.
    started: Option<String>,
    /// The time the running row counts to.
    now: SystemTime,
    focused: bool,
    share_open: bool,
    /// The share menu's open state the window was last sized for.
    sized_for: bool,
    toast: Option<(Toast, Instant)>,
    next_frame: Instant,
}

impl CmdInfoWindow {
    /// Opens the window for a click at `at` (screen pixels), kept inside `work`.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pane: PaneId,
        info: Info,
        started: Option<String>,
        at: (f64, f64),
        work: Option<Rect>,
        dark: bool,
    ) -> Result<CmdInfoWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let h = cmdinfo::height(started.is_some(), false);
        let attrs = Window::default_attributes()
            .with_title("Command Info")
            .with_decorations(false)
            .with_skip_taskbar(true)
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(cmdinfo::WIDTH, h));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        let scale = host.window.scale_factor();
        let (x, y) = cmdinfo::place(at, cmdinfo::WIDTH * scale, h * scale, work, scale);
        host.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        host.window.set_visible(true);
        host.focus();
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(CmdInfoWindow { host, pane, info, started, now: SystemTime::now(), focused: false, share_open: false, sized_for: false, toast: None, next_frame: Instant::now() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn mark(&self) -> u32 {
        self.info.mark
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Whether the running row still ticks.
    pub fn running(&self) -> bool {
        self.info.running
    }

    /// The command ended (or the program with it) while the window was open: the fields as
    /// they are now, the output as it is now.
    pub fn refresh(&mut self, code: Option<i32>, end: Option<SystemTime>, running: bool, output: Option<(u64, usize)>) {
        self.info.code = code;
        self.info.end = end;
        self.info.running = running;
        if let Some((bytes, lines)) = output {
            self.info.output_bytes = bytes;
            self.info.output_lines = lines;
        }
        self.host.request_redraw();
    }

    /// The "Snippet Added" toast (`CIV:340`).
    pub fn toast_snippet_added(&mut self) {
        self.toast = Some((Toast::SnippetAdded, Instant::now()));
        self.share_open = false;
        self.host.request_redraw();
    }

    /// Asks for the next frame while the time ticks or a toast fades; the time the next one is
    /// due, or None when nothing moves (the window is idle, or minimized).
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        if self.host.window.is_minimized() == Some(true) {
            return None;
        }
        if self.toast.is_some_and(|(t, at)| toast_gone(t, at, now)) {
            self.toast = None;
            self.host.request_redraw();
        }
        if !self.info.running && self.toast.is_none() {
            return None;
        }
        if now >= self.next_frame {
            self.now = SystemTime::now();
            self.host.request_redraw();
            self.next_frame = now + FRAME_GAP;
        }
        Some(self.next_frame)
    }

    /// Handles one of its window's events.
    pub fn event(&mut self, event: &WindowEvent) -> CmdInfoEvent {
        if let Some(close) = crate::status_popover_ui::closes(event, &mut self.focused, false) {
            return if close { CmdInfoEvent::Close } else { CmdInfoEvent::None };
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.redraw();
        }
        self.host.event(event);
        CmdInfoEvent::None
    }

    fn redraw(&mut self) -> CmdInfoEvent {
        if self.sized_for != self.share_open {
            self.sized_for = self.share_open;
            let _ = self.host.window.request_inner_size(winit::dpi::LogicalSize::new(cmdinfo::WIDTH, cmdinfo::height(self.started.is_some(), self.share_open)));
        }
        let mut out = CmdInfoEvent::None;
        let (info, started, now, toast) = (&self.info, &self.started, self.now, self.toast);
        let share_open = &mut self.share_open;
        let mut new_toast = None;
        self.host.paint(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(cmdinfo::MARGIN as f32)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // Command.
                text_row(ui, cmdinfo::LABEL_COMMAND, &cmdinfo::shown(&info.command), None, |_| {});
                // Directory, with its two buttons (disabled with no directory).
                let has_dir = info.directory.is_some();
                text_row(ui, cmdinfo::LABEL_DIRECTORY, &cmdinfo::shown(info.directory.as_deref().unwrap_or("")), None, |ui| {
                    let copy = icon_button(ui, '\u{E8C8}', "Copy", cmdinfo::TIP_COPY_DIRECTORY, has_dir);
                    if copy.clicked() {
                        out = CmdInfoEvent::Act(Act::CopyDirectory);
                        new_toast = Some(Toast::Copied { x: copy.rect.center().x, y: copy.rect.top() });
                    }
                    if icon_button(ui, '\u{E724}', "Write", cmdinfo::TIP_SEND_DIRECTORY, has_dir).clicked() {
                        out = CmdInfoEvent::Act(Act::WriteDirectory);
                    }
                });
                // Return code: red with a stop sign when it failed, a check when it did not.
                let (code, glyph, red) = info.code_shown();
                text_row(ui, cmdinfo::LABEL_CODE, &code, Some((glyph, red)), |_| {});
                text_row(ui, cmdinfo::LABEL_RUNNING, &info.running_time(now), None, |_| {});
                // Output, with Copy output and Select output.
                text_row(ui, cmdinfo::LABEL_OUTPUT, &cmdinfo::output_text(info.output_bytes, info.output_lines), None, |ui| {
                    if icon_button(ui, '\u{E8B3}', "Select", cmdinfo::TIP_SELECT_OUTPUT, true).clicked() {
                        out = CmdInfoEvent::Act(Act::SelectOutput);
                    }
                    let copy = icon_button(ui, '\u{E8C8}', "Copy", cmdinfo::TIP_COPY_OUTPUT, true);
                    if copy.clicked() {
                        out = CmdInfoEvent::Act(Act::CopyOutput);
                        new_toast = Some(Toast::Copied { x: copy.rect.center().x, y: copy.rect.top() });
                    }
                });
                if let Some(text) = started {
                    text_row(ui, "", text, None, |_| {});
                }
                // The command's buttons: write, copy, share, composer.
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if icon_button(ui, '\u{E724}', "Write", cmdinfo::TIP_SEND_COMMAND, true).clicked() {
                        out = CmdInfoEvent::Act(Act::WriteCommand);
                    }
                    let copy = icon_button(ui, '\u{E8C8}', "Copy", cmdinfo::TIP_COPY_COMMAND, true);
                    if copy.clicked() {
                        out = CmdInfoEvent::Act(Act::CopyCommand);
                        new_toast = Some(Toast::Copied { x: copy.rect.center().x, y: copy.rect.top() });
                    }
                    if icon_button(ui, '\u{E72D}', "Share", cmdinfo::TIP_SHARE, true).clicked() {
                        *share_open = !*share_open;
                    }
                    if icon_button(ui, '\u{E70F}', "Composer", cmdinfo::TIP_COMPOSER, true).clicked() {
                        out = CmdInfoEvent::Act(Act::Composer);
                    }
                });
                // The share menu's items.
                if *share_open {
                    ui.add_space(4.0);
                    if ui.add(egui::Button::new(cmdinfo::ADD_SNIPPET).min_size(egui::vec2(ui.available_width(), 24.0))).clicked() {
                        out = CmdInfoEvent::Act(Act::AddSnippet);
                    }
                    if ui.add(egui::Button::new(cmdinfo::COPY_URL).min_size(egui::vec2(ui.available_width(), 24.0))).clicked() {
                        out = CmdInfoEvent::Act(Act::CopyUrl);
                        *share_open = false;
                        new_toast = Some(Toast::Copied { x: ui.max_rect().center().x, y: ui.max_rect().bottom() });
                    }
                }
            });
            if let Some((t, age)) = toast.map(|(t, at)| (t, at.elapsed().as_secs_f32())) {
                draw_toast(ui, t, age);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = CmdInfoEvent::Close;
            }
        });
        if let Some(t) = new_toast {
            self.toast = Some((t, Instant::now()));
            self.next_frame = Instant::now();
        }
        out
    }
}

/// Whether a toast that began at `at` is gone by `now`.
fn toast_gone(t: Toast, at: Instant, now: Instant) -> bool {
    let hold = match t {
        Toast::Copied { .. } => cmdinfo::COPIED_HOLD,
        Toast::SnippetAdded => cmdinfo::SNIPPET_HOLD,
    };
    cmdinfo::toast_alpha(now.saturating_duration_since(at).as_secs_f32(), hold).is_none()
}

/// A glyph of the icon font as a button, its word without it.
fn icon_button(ui: &mut egui::Ui, glyph: char, fallback: &str, tip: &str, enabled: bool) -> egui::Response {
    let text = crate::settings_keys::icon_text(ui.ctx(), glyph).unwrap_or_else(|| egui::RichText::new(fallback));
    let r = ui.add_enabled(enabled, egui::Button::new(text).min_size(egui::vec2(BUTTON[0], BUTTON[1])));
    if enabled { r.on_hover_text(tip) } else { r.on_disabled_hover_text(tip) }
}

/// One row: the label, the value (one line, cut to the room the buttons leave; a tooltip holds
/// it whole) and the buttons at the right.
fn text_row(ui: &mut egui::Ui, label: &str, value: &str, code: Option<(Option<char>, bool)>, buttons: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(cmdinfo::ROW_H as f32);
        ui.add_sized([LABEL_W, cmdinfo::ROW_H as f32], egui::Label::new(egui::RichText::new(label).weak()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            buttons(ui);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let mut text = egui::RichText::new(value);
                if code.is_some_and(|(_, red)| red) {
                    text = text.color(egui::Color32::from_rgb(220, 50, 47));
                }
                // Selectable, so a value can be copied; cut to one line.
                let r = ui.add(egui::Label::new(text).truncate().selectable(true));
                if value.chars().count() > 40 {
                    r.on_hover_text(value);
                }
                if let Some((Some(glyph), red)) = code {
                    let mut g = crate::settings_keys::icon_text(ui.ctx(), glyph).unwrap_or_else(|| egui::RichText::new(if red { "!" } else { "ok" }));
                    if red {
                        g = g.color(egui::Color32::from_rgb(220, 50, 47));
                    }
                    ui.label(g);
                }
            });
        });
    });
}

/// "Copied" under its button, or "Snippet Added" at the bottom, holding and then fading.
fn draw_toast(ui: &egui::Ui, toast: Toast, age: f32) {
    let (text, hold, at, width) = match toast {
        Toast::Copied { x, y } => (cmdinfo::COPIED, cmdinfo::COPIED_HOLD, egui::pos2(x, y - 2.0), 64.0),
        Toast::SnippetAdded => (cmdinfo::SNIPPET_ADDED, cmdinfo::SNIPPET_HOLD, egui::pos2(ui.max_rect().center().x, ui.max_rect().bottom() - 6.0), 108.0),
    };
    let Some(alpha) = cmdinfo::toast_alpha(age, hold) else { return };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("cmdinfo-toast")));
    let rect = egui::Align2::CENTER_BOTTOM.anchor_size(at, egui::vec2(width, 22.0));
    painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha((alpha * 255.0) as u8));
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.0), egui::Color32::WHITE.gamma_multiply(alpha / cmdinfo::TOAST_ALPHA));
}
