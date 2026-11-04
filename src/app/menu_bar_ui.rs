//! The menu bar drawn in the terminal window (Linux). `menu_overlay` owns the state and the
//! layout; this is the window side: the strip the layout keeps free, the events that reach the
//! overlay first, and drawing its egui output on the frame after the terminal.

use super::*;
use crate::menu_overlay::{Consumed, Overlay};

/// One window's overlay and the egui parts that draw it.
pub(super) struct BarUi {
    overlay: Overlay,
    ctx: egui::Context,
    renderer: Option<egui_wgpu::Renderer>,
    dark: Option<bool>,
    /// A pop-up asked for, drawn by the next frame that gets to the surface.
    pending: Option<crate::platform::chrome::QueuedPopup>,
}

impl Default for BarUi {
    fn default() -> Self {
        let ctx = egui::Context::default();
        crate::egui_host::install_menu_fonts(&ctx);
        BarUi { overlay: Overlay::new(), ctx, renderer: None, dark: None, pending: None }
    }
}

/// What a frame hands the overlay.
pub(super) struct BarFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    pub scale: f32,
    pub dark: bool,
}

impl BarUi {
    /// Something of the bar's is to be drawn: a menu is open or a pop-up waits.
    pub(super) fn busy(&self) -> bool {
        self.overlay.is_open() || self.pending.is_some() || self.overlay.has_choice()
    }

    pub(super) fn queue(&mut self, popup: crate::platform::chrome::QueuedPopup) {
        self.pending = Some(popup);
    }

    /// Draws the bar (when `bar` is given) and any open menu over `frame.target`. Returns the id of
    /// an item chosen since the last frame.
    pub(super) fn draw(&mut self, frame: &BarFrame, bar: Option<&muda::Menu>) -> Option<String> {
        self.overlay.set_screen(frame.size.0 as f32, frame.size.1 as f32, frame.scale);
        if self.dark != Some(frame.dark) {
            self.dark = Some(frame.dark);
            self.overlay.set_dark(&self.ctx, frame.dark);
        }
        if let Some(p) = self.pending.take() {
            match p.at {
                Some((x, y)) => self.overlay.open_popup(p.items, egui::pos2(x / frame.scale, y / frame.scale)),
                None => self.overlay.open_popup_at_pointer(p.items),
            }
        }
        let chosen = self.overlay.paint(&self.ctx, bar);
        let Some(mut out) = self.overlay.take_output() else { return chosen };
        let renderer = self.renderer.get_or_insert_with(|| egui_wgpu::Renderer::new(frame.device, frame.format, egui_wgpu::RendererOptions::default()));
        let jobs = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [frame.size.0, frame.size.1], pixels_per_point: out.pixels_per_point };
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(frame.device, frame.queue, *id, delta);
            }
        }
        let mut encoder = frame.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("menu bar") });
        let extra = renderer.update_buffers(frame.device, frame.queue, &mut encoder, &jobs, &screen);
        {
            // Loads what the terminal drew: the overlay goes over it.
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu bar"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: frame.target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, &jobs, &screen);
        }
        frame.queue.submit(extra.into_iter().chain(Some(encoder.finish())));
        for id in &out.textures_delta.free {
            renderer.free_texture(id);
        }
        out.textures_delta.clear();
        chosen
    }
}

impl App {
    /// The strip the in-window bar takes at the top of this window, in pixels; None when no bar
    /// shows (another OS, no menus built, hidden, a Minimal or Compact window, full screen).
    pub(super) fn bar_strip(&self) -> Option<f32> {
        (crate::platform::chrome::IN_WINDOW_BAR && self.menu_bar.is_some() && !self.want_menu_hidden() && !self.full_screen()).then(|| Overlay::bar_height(self.scale()))
    }

    /// Gives a window event to the overlay first while it is open or can be under the pointer.
    /// True when the event is the menu's and goes no further.
    pub(super) fn bar_event(&mut self, event: &WindowEvent) -> bool {
        if !crate::platform::chrome::IN_WINDOW_BAR {
            return false;
        }
        let shown = self.bar_strip().is_some();
        let Some(g) = self.gpu.as_mut() else { return false };
        if !shown && !g.bar.overlay.is_open() {
            return false;
        }
        let ctx = g.bar.ctx.clone();
        let taken = g.bar.overlay.event(&ctx, event) == Consumed::Yes;
        let dirty = g.bar.overlay.take_dirty();
        if taken || dirty {
            g.window.request_redraw();
        }
        taken
    }

    /// Sends a chosen item's id the way a native menu does.
    pub(super) fn bar_choice(&self, id: String) {
        let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::Menu(id)));
    }
}

#[cfg(test)]
#[cfg(not(target_os = "macos"))] // test: muda menus are the main thread's on macOS
mod tests {
    use super::*;
    use muda::{Menu, MenuItem};
    use winit::event::{DeviceId, ElementState, MouseButton};

    fn mouse(ui: &mut BarUi, e: WindowEvent) {
        let ctx = ui.ctx.clone();
        ui.overlay.event(&ctx, &e);
    }

    #[test]
    fn a_pop_up_choice_with_no_bar_is_delivered_once() {
        let menu = Menu::new();
        menu.append(&MenuItem::with_id("pick", "Pick", true, None)).unwrap();
        let mut ui = BarUi::default();
        ui.overlay.set_screen(400.0, 300.0, 1.0);
        ui.queue(crate::platform::chrome::QueuedPopup { items: menu.items(), at: Some((10.0, 10.0)) });
        let ctx = ui.ctx.clone();
        // What `draw` does with the queue, without a GPU.
        let p = ui.pending.take().unwrap();
        ui.overlay.open_popup(p.items, egui::pos2(10.0, 10.0));
        assert_eq!(ui.overlay.paint(&ctx, None), None);
        let at = ui.overlay.row_rect(0, "Pick").unwrap().center();
        let id = DeviceId::dummy();
        mouse(&mut ui, WindowEvent::CursorMoved { device_id: id, position: winit::dpi::PhysicalPosition::new(at.x as f64, at.y as f64) });
        mouse(&mut ui, WindowEvent::MouseInput { device_id: id, state: ElementState::Pressed, button: MouseButton::Left });
        mouse(&mut ui, WindowEvent::MouseInput { device_id: id, state: ElementState::Released, button: MouseButton::Left });
        assert!(!ui.overlay.is_open(), "the choice closed the menu");
        assert!(ui.busy(), "the frame that delivers the choice must still run");
        assert_eq!(ui.overlay.paint(&ctx, None).as_deref(), Some("pick"));
        assert!(!ui.busy());
        assert_eq!(ui.overlay.paint(&ctx, None), None, "delivered exactly once");
    }
}
