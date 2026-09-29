//! One egui window: its surface, egui state, and renderer. The settings window and the
//! prompt window draw through it.

use std::sync::Arc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

pub struct EguiHost {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    pub window: Arc<Window>,
    /// Set to read the selected label text on the next frame (see `paint`).
    probe: bool,
    /// What the last probe found.
    pub probed: Option<String>,
}

impl EguiHost {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        w: f64,
        h: f64,
    ) -> Result<EguiHost, String> {
        let attrs = Window::default_attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(w, h));
        Self::open_with(event_loop, instance, adapter, device, queue, attrs)
    }

    /// A window made from `attrs` (a popover's: no frame, placed, no taskbar button).
    pub fn open_with(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        attrs: winit::window::WindowAttributes,
    ) -> Result<EguiHost, String> {
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
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(device.limits().max_texture_dimension_2d as usize));
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Ok(EguiHost { surface, config, device: device.clone(), queue: queue.clone(), ctx, state, renderer, window, probe: false, probed: None })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`: the
    /// window's theme, and the egui visuals to match. winit does not track a forced theme
    /// (`Window::theme` stays Windows' mode), so the mode is the forced one, else the window's.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.window.set_theme(crate::tab_theme::window_theme(forced));
        let dark = forced.unwrap_or_else(|| self.window.theme().is_none_or(|t| t == winit::window::Theme::Dark));
        self.ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
    }

    /// The window's Win32 handle.
    pub fn hwnd(&self) -> Option<isize> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match self.window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
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

    /// Asks for the text selected in the labels: the next frame copies it into `probed`
    /// instead of the clipboard.
    pub fn probe_selection(&mut self) {
        self.probe = true;
        self.window.request_redraw();
    }

    /// Runs one egui frame with `f` and draws it.
    pub fn paint(&mut self, f: impl FnMut(&mut egui::Ui)) {
        let mut input = self.state.take_egui_input(&self.window);
        let probing = std::mem::take(&mut self.probe);
        if probing {
            input.events.push(egui::Event::Copy);
        }
        let mut out = self.ctx.run_ui(input, f);
        if probing {
            // The copy is ours: taken out of the frame's output so the clipboard is untouched.
            let mut found = String::new();
            out.platform_output.commands.retain(|c| match c {
                egui::OutputCommand::CopyText(t) => {
                    found.push_str(t);
                    false
                }
                _ => true,
            });
            self.probed = Some(found);
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
                return;
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
    }
}
