//! The git menu's Log popover (milestone 9i, `GIT:449-505`): a small frameless window at
//! the git component showing `git log`, 60 characters of the terminal font wide plus
//! iTerm2's margins, 272 points tall (`iTermTextPopoverViewController.xib`), dark or light
//! by the terminal's background. Like a transient popover it closes when it loses focus;
//! Esc closes it too.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;

/// iTerm2's `iTermTextPopoverViewControllerHorizontalMarginWidth`, in points.
const MARGIN_PT: f64 = 4.0;
/// The popover's height in the nib, in points.
const HEIGHT_PT: f64 = 272.0;
/// Characters of the terminal font across (`GIT:463`).
const COLUMNS: f64 = 60.0;

/// Where the popover goes, in screen pixels: its left at `x`; its bottom at `y` when
/// `above` (a bar at the bottom), else its top.
#[derive(Clone, Copy, Debug)]
pub struct LogAt {
    pub x: f64,
    pub y: f64,
    pub above: bool,
}

/// The terminal's font as the popover uses it: its cell width and size in pixels, and the
/// screen scale.
#[derive(Clone, Copy, Debug)]
pub struct LogFont {
    pub cell_w: f64,
    pub px: f32,
    pub scale: f64,
    /// The terminal's background is dark.
    pub dark: bool,
}

pub struct GitLogWindow {
    host: EguiHost,
    text: String,
    font: LogFont,
    /// It has had focus (a focus loss before that is the window being made).
    focused: bool,
}

impl GitLogWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        text: String,
        at: LogAt,
        font: LogFont,
    ) -> Result<GitLogWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let w = font.cell_w * COLUMNS + MARGIN_PT * 2.0 * font.scale;
        let h = HEIGHT_PT * font.scale;
        let y = if at.above { at.y - h } else { at.y };
        let attrs = Window::default_attributes()
            .with_title("git log")
            .with_decorations(false)
            .with_skip_taskbar(true)
            .with_inner_size(winit::dpi::PhysicalSize::new(w, h))
            .with_position(winit::dpi::PhysicalPosition::new(at.x, y));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.ctx().set_visuals(if font.dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        host.focus();
        Ok(GitLogWindow { host, text, font, focused: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// Handles one of its window's events. False when it should close: it lost focus, was
    /// asked to close, or Esc was pressed.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => false,
            WindowEvent::Focused(true) => {
                self.focused = true;
                true
            }
            WindowEvent::Focused(false) => !self.focused,
            WindowEvent::RedrawRequested => self.redraw(),
            other => {
                self.host.event(other);
                true
            }
        }
    }

    fn redraw(&mut self) -> bool {
        let mut open = true;
        let (text, font) = (&self.text, self.font);
        self.host.paint(|ui| {
            let size = font.px / ui.ctx().pixels_per_point();
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(MARGIN_PT as f32)).show(ui, |ui| {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    // A `&str` buffer: selectable and copyable, not editable.
                    let mut shown = text.as_str();
                    ui.add(egui::TextEdit::multiline(&mut shown).font(egui::FontId::monospace(size)).frame(egui::Frame::NONE).desired_width(f32::INFINITY));
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        });
        open
    }
}
