//! The Instant Replay Control panel (`IRX`, `IRW`): the earliest time, a continuous slider
//! with the recording's tick marks over it, the current time under the knob, "Now", and the
//! Export and Cancel buttons. It never takes activation from the terminal window (iTerm2's
//! never-key panel) and resizes in width only. It is opaque: iTerm2's 0.9 alpha (1 under
//! the pointer) would need a layered window, which wgpu's swap chains do not draw into
//! reliably.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;
use crate::replay_panel::{self, Export, Panel, PanelAct};

/// What the panel shows, built by the app from the focused pane at each redraw.
pub struct PanelView {
    pub earliest: String,
    pub current: String,
    pub slider: f32,
    pub band: Option<(f32, f32)>,
    pub buttons: (&'static str, Option<&'static str>),
    /// The replay decoder's timestamp (-1 while live), what Set Start and Set End read.
    pub current_ts: i64,
}

/// Content geometry from `IRX:26-99`, turned into y-down content coordinates (the xib's
/// background view reaches 14 points above the content, into the transparent title bar).
const LABEL_X: f32 = 18.0;
const LABEL_Y: f32 = 7.0;
/// The margin between the earliest label and the slider (`86 - 18 - 62`).
const LABEL_GAP: f32 = 6.0;
const SLIDER_Y: f32 = 7.0;
const SLIDER_H: f32 = 15.0;
/// The slider's right edge at the xib's width (`86 + 389`); it keeps its right margin.
const SLIDER_RIGHT: f32 = 475.0;
const STRIP_Y: f32 = 9.0;
const STRIP_H: f32 = 4.0;
const NOW_X: f32 = 481.0;
const CURRENT_Y: f32 = 23.0;
const BUTTON_X: f32 = 512.0;
const BUTTON_Y: [f32; 2] = [6.0, 23.0];
const BUTTON_SIZE: (f32, f32) = (71.0, 16.0);
/// `miniSystem`.
const FONT: f32 = 10.0;
const KNOB_R: f32 = 5.0;

pub struct ReplayWindow {
    host: EguiHost,
    /// The export steps and the tick marks fixed at open (the kept bug).
    pub panel: Panel,
    /// The slider's value, kept when the range is not positive (`IRW:317-321`).
    pub slider: f32,
    shown: bool,
}

impl ReplayWindow {
    /// A hidden panel; `place` puts it where `replay_panel::origin` says and shows it. The
    /// origin needs the panel's frame size (its title bar included, as iTerm2's
    /// `window.frame.size`), which only exists once the window does.
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ticks: Vec<i64>,
        forced: Option<bool>,
    ) -> Result<ReplayWindow, String> {
        let (w, h) = replay_panel::SIZE;
        let attrs = Window::default_attributes()
            .with_title(replay_panel::TITLE)
            .with_resizable(true)
            .with_enabled_buttons(winit::window::WindowButtons::CLOSE)
            .with_active(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64))
            .with_min_inner_size(winit::dpi::LogicalSize::new(replay_panel::MIN_WIDTH as f64, h as f64))
            .with_max_inner_size(winit::dpi::LogicalSize::new(100_000.0, h as f64));
        let attrs = crate::platform::window_style::skip_taskbar(attrs);
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.follow_theme(forced);
        crate::platform::window_style::no_activate(&host.window);
        Ok(ReplayWindow { host, panel: Panel { export: Export::Normal, ticks_at_open: ticks }, slider: 1.0, shown: false })
    }

    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.host.follow_theme(forced);
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// The frame's size in pixels, title bar included.
    pub fn outer_size(&self) -> (f32, f32) {
        let s = self.host.window.outer_size();
        (s.width as f32, s.height as f32)
    }

    /// Moves the frame's top left to `at` (pixels) and shows the panel without taking the
    /// focus.
    pub fn place(&mut self, at: (f32, f32)) {
        self.host.window.set_outer_position(winit::dpi::PhysicalPosition::new(at.0 as f64, at.1 as f64));
        self.shown = false;
        self.set_shown(true);
    }

    /// Hidden while Termurion is not the active app (`hidesOnDeactivate`).
    pub fn set_shown(&mut self, on: bool) {
        if on == self.shown {
            return;
        }
        self.shown = on;
        if !on {
            return self.host.window.set_visible(false);
        }
        crate::platform::window_style::show_no_activate(&self.host.window);
        self.host.request_redraw();
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    /// Feeds a window event to egui. True when the panel should redraw.
    pub fn event(&mut self, e: &WindowEvent) -> bool {
        self.host.event(e)
    }

    /// Draws the panel from `view`; returns what the slider and the buttons asked for.
    pub fn redraw(&mut self, view: &PanelView) -> Vec<PanelAct> {
        let mut acts = Vec::new();
        let mut cancelled = false;
        let Self { host, panel, slider, .. } = self;
        *slider = view.slider;
        host.paint(|ui| {
            let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                let full = ui.max_rect();
                let o = full.min;
                let at = |x: f32, y: f32| egui::pos2(o.x + x, o.y + y);
                let text = ui.visuals().text_color();
                let font = egui::FontId::proportional(FONT);
                let painter = ui.painter().clone();

                // The earliest label sizes to fit, and the slider starts the same margin
                // after it (`IRW:325-336`); the slider's right edge follows the width.
                let earliest = painter.layout_no_wrap(view.earliest.clone(), font.clone(), text);
                let slider_x = LABEL_X + earliest.size().x + LABEL_GAP;
                painter.galley(at(LABEL_X, LABEL_Y), earliest, text);
                let slider_w = (SLIDER_RIGHT + (full.width() - replay_panel::SIZE.0) - slider_x).max(0.0);
                let now = painter.layout_no_wrap(replay_panel::NOW.to_string(), font.clone(), text);
                painter.galley(at(NOW_X, LABEL_Y), now, text);

                // The slider: continuous, 0 to 1, two tick marks above its ends.
                let rect = egui::Rect::from_min_size(at(slider_x, SLIDER_Y), egui::vec2(slider_w, SLIDER_H));
                let resp = ui.interact(rect, ui.id().with("ir-slider"), egui::Sense::click_and_drag());
                let travel = (slider_w - 2.0 * KNOB_R).max(1.0);
                if (resp.dragged() || resp.clicked())
                    && let Some(p) = resp.interact_pointer_pos() {
                        let f = ((p.x - rect.left() - KNOB_R) / travel).clamp(0.0, 1.0);
                        if f != *slider {
                            *slider = f;
                            acts.push(PanelAct::Seek(f));
                        }
                    }
                let track_y = rect.center().y + 2.0;
                let track = egui::Rect::from_min_max(egui::pos2(rect.left() + KNOB_R, track_y - 1.5), egui::pos2(rect.right() - KNOB_R, track_y + 1.5));
                painter.rect_filled(track, 1.5, ui.visuals().widgets.inactive.bg_fill);
                for x in [track.left(), track.right()] {
                    painter.rect_filled(egui::Rect::from_min_size(egui::pos2(x.round(), rect.top()), egui::vec2(1.0, 2.0)), 0.0, ui.visuals().weak_text_color());
                }

                // The events strip over the track: the range band, then the ticks
                // (`IRW:27-78`), 5 in from each end of the slider.
                let strip = egui::Rect::from_min_size(at(slider_x + 5.0, STRIP_Y), egui::vec2((slider_w - 10.0).max(0.0), STRIP_H));
                let x_of = |f: f32| (f * (strip.width() - 1.0)).round();
                if let Some((s, e)) = view.band.filter(|(s, e)| e > s) {
                    let band = egui::Rect::from_min_max(egui::pos2(strip.left() + x_of(s), strip.top()), egui::pos2(strip.left() + x_of(e), strip.bottom()));
                    painter.rect_filled(band, 0.0, egui::Color32::from_rgba_unmultiplied(255, 0, 0, 128));
                }
                let tick = if ui.visuals().dark_mode { egui::Color32::from_white_alpha(128) } else { egui::Color32::from_black_alpha(128) };
                for x in replay_panel::ticks(&panel.ticks_at_open, strip.width()) {
                    painter.rect_filled(egui::Rect::from_min_size(egui::pos2(strip.left() + x, strip.top()), egui::vec2(1.0, STRIP_H)), 0.0, tick);
                }

                let knob = egui::pos2(rect.left() + KNOB_R + *slider * travel, track_y);
                painter.circle(knob, KNOB_R, ui.visuals().widgets.active.fg_stroke.color, ui.visuals().widgets.inactive.bg_stroke);

                // The current label, centered on the knob and kept over the slider.
                let current = painter.layout_no_wrap(view.current.clone(), font.clone(), text);
                let cx = replay_panel::label_x(*slider, slider_x, slider_w, current.size().x);
                painter.galley(at(cx, CURRENT_Y), current, text);

                let (first, second) = view.buttons;
                let button = |ui: &mut egui::Ui, y: f32, label: &str| {
                    let r = egui::Rect::from_min_size(at(BUTTON_X, y), egui::vec2(BUTTON_SIZE.0, BUTTON_SIZE.1));
                    ui.put(r, egui::Button::new(egui::RichText::new(label).size(FONT))).clicked()
                };
                if button(ui, BUTTON_Y[0], first) {
                    acts.extend(panel.export_button(view.current_ts));
                }
                if let Some(label) = second
                    && button(ui, BUTTON_Y[1], label) {
                        panel.cancel();
                        cancelled = true;
                    }
            });
        });
        // Cancel hides itself and the band: one more frame shows it.
        if cancelled {
            host.request_redraw();
        }
        acts
    }
}
