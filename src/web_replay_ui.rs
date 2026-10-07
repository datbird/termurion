//! The browser Instant Replay playback window (milestone 13x, Task 10; `VPW`): the frames at their
//! capture times, Play Backward, Play / Pause, a scrubber, the time and Reveal in Explorer. The
//! frames are in memory (the caller's copy of the ring); nothing here touches a disk. Arrow keys step
//! one frame, Space plays or pauses, Escape closes (`VPW:7-12`).

use std::time::{Duration, Instant};

use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;
use crate::web_replay::{self, Clock};

/// What the window asks the app for.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayAct {
    /// Save the movie and show it in Explorer.
    Reveal,
    /// The window closed.
    Closed,
}

/// The controls' height in points (`controlsHeight`, `VPW:29`).
const CONTROLS: f64 = 60.0;
/// The window never opens smaller or larger than this (points).
const MIN_SIZE: (f64, f64) = (480.0, 300.0);
const MAX_SIZE: (f64, f64) = (1600.0, 1000.0);

pub struct PlaybackWindow {
    host: EguiHost,
    frames: Vec<(Duration, Vec<u8>)>,
    marks: Vec<f64>,
    clock: Clock,
    /// The frame the texture shows and the texture itself.
    shown: Option<usize>,
    texture: Option<egui::TextureHandle>,
    last: Instant,
    /// A save is running: Reveal waits for it.
    pub saving: bool,
}

impl PlaybackWindow {
    /// Opens the window on `frames` (oldest first) whose pictures are `size` pixels; it shows the
    /// last frame, paused (`seekToLastFrame`). Err when there is nothing to show or no window.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frames: Vec<(Duration, Vec<u8>)>,
        size: (u32, u32),
    ) -> Result<PlaybackWindow, String> {
        if frames.is_empty() {
            return Err("nothing was recorded".to_string());
        }
        let (w, h) = (f64::from(size.0.max(1)).clamp(MIN_SIZE.0, MAX_SIZE.0), f64::from(size.1.max(1)).clamp(MIN_SIZE.1, MAX_SIZE.1 - CONTROLS));
        let attrs = Window::default_attributes()
            .with_title(web_replay::WINDOW_TITLE)
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(w, h + CONTROLS));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.track_taskbar();
        let marks = web_replay::timeline(&frames);
        let clock = Clock::at_end(marks.last().copied().unwrap_or(0.0));
        host.request_redraw();
        Ok(PlaybackWindow { host, frames, marks, clock, shown: None, texture: None, last: Instant::now(), saving: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// The frames and their picture size, for saving the movie.
    pub fn movie_input(&self) -> (Vec<(Duration, Vec<u8>)>, (u32, u32)) {
        let size = self.frames.last().and_then(|f| web_replay::jpeg_size(&f.1)).unwrap_or((0, 0));
        (self.frames.clone(), size)
    }

    /// Follows a forced appearance (`tab_theme::forced_dark`).
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.host.follow_theme(forced);
    }

    /// One window event.
    pub fn event(&mut self, e: &WindowEvent) -> Option<ReplayAct> {
        match e {
            WindowEvent::CloseRequested => return Some(ReplayAct::Closed),
            WindowEvent::RedrawRequested => return self.redraw(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => return Some(ReplayAct::Closed),
                    Key::Named(NamedKey::ArrowLeft) => self.clock.step(&self.marks, -1),
                    Key::Named(NamedKey::ArrowRight) => self.clock.step(&self.marks, 1),
                    Key::Named(NamedKey::Space) => self.clock.forward(),
                    _ => return None,
                }
                self.last = Instant::now();
                self.host.request_redraw();
                return None;
            }
            _ => {}
        }
        self.host.event(e);
        None
    }

    /// Decodes frame `i` into the texture when it is not the one shown. A frame that does not decode
    /// leaves the last picture.
    fn load(&mut self, ctx: &egui::Context, i: usize) {
        if self.shown == Some(i) {
            return;
        }
        self.shown = Some(i);
        let bytes = &self.frames[i].1;
        // The header's size first: a hostile size is never decoded.
        if web_replay::jpeg_size(bytes).filter(|(w, h)| u64::from(*w) * u64::from(*h) <= 64 * 1024 * 1024).is_none() {
            return;
        }
        let Ok(img) = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg) else { return };
        let rgba = img.to_rgba8();
        // The decoded size, not the header's: a JPEG whose first frame header disagrees with the
        // decoder's output would fail egui's size check (Task 10 M4).
        let (dw, dh) = rgba.dimensions();
        let image = egui::ColorImage::from_rgba_unmultiplied([dw as usize, dh as usize], rgba.as_raw());
        self.texture = Some(ctx.load_texture("web-replay", image, egui::TextureOptions::LINEAR));
    }

    fn redraw(&mut self) -> Option<ReplayAct> {
        let now = Instant::now();
        self.clock.tick(now.duration_since(self.last).as_secs_f64());
        self.last = now;
        let index = self.clock.index(&self.marks);
        let ctx = self.host.ctx().clone();
        self.load(&ctx, index);
        let mut act = None;
        let Self { host, clock, texture, saving, .. } = self;
        let end = clock.end;
        host.paint(|ui| {
            egui::Panel::bottom("web-replay-controls").exact_size(CONTROLS as f32).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let back = if clock.dir < 0 { web_replay::PAUSE } else { web_replay::PLAY_BACKWARD };
                    if ui.button(back).clicked() {
                        clock.backward();
                    }
                    let fwd = if clock.dir > 0 { web_replay::PAUSE } else { web_replay::PLAY };
                    if ui.button(fwd).clicked() {
                        clock.forward();
                    }
                    let label = format!("{} / {}", web_replay::format_time(clock.pos), web_replay::format_time(end));
                    if ui.add_enabled(!*saving, egui::Button::new(web_replay::REVEAL)).clicked() {
                        act = Some(ReplayAct::Reveal);
                    }
                    ui.label(label);
                    let mut pos = clock.pos;
                    ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(40.0);
                    if ui.add(egui::Slider::new(&mut pos, 0.0..=end.max(f64::MIN_POSITIVE)).show_value(false)).changed() {
                        clock.seek(pos);
                    }
                });
            });
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(t) = texture {
                    let avail = ui.available_size();
                    let s = t.size_vec2();
                    let k = (avail.x / s.x).min(avail.y / s.y).min(1.0).max(0.0);
                    ui.centered_and_justified(|ui| {
                        ui.image((t.id(), s * k));
                    });
                }
            });
        });
        if self.clock.dir != 0 {
            self.host.request_redraw();
        }
        act
    }
}
