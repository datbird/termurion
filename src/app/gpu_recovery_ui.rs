//! Recovery after a lost graphics device (a driver update or reset, a timeout, a GPU removed, a laptop
//! switching GPUs): every window gets a new adapter, device, surface and renderer resources through the
//! same API order and GPU preference as start-up. The state machine is `gpu_pick::Recovery`.

use super::*;
use crate::gpu_pick::Action;

/// Moves one window onto new handles. Leaves the window as it was, and says why, when its new
/// surface cannot be made, so a failed attempt loses no window.
fn rebuild_window_gpu(g: &mut Gpu, primary: &GpuHandles, alpha: Option<&AlphaGpu>) -> Result<(), String> {
    g.win.rebuild(&primary.shared(), alpha.map(GpuHandles::shared).as_ref())?;
    after_rebuild(g);
    Ok(())
}

/// What a window drops once its GPU side was made again.
fn after_rebuild(g: &mut Gpu) {
    g.fx = None;
    g.bar = Default::default();
    g.layout = None;
    // The same window: its title is sent once more, which is harmless.
    g.title_sent = Default::default();
    g.scene = Default::default();
    g.painted_at = 0;
    g.display_hz = super::display_hz_of(&g.window);
}

/// `rebuild_window_gpu` for a window on Metal: there is no see-through instance to carry.
#[cfg(target_os = "macos")]
fn rebuild_metal_window(g: &mut Gpu, primary: &crate::gpu_shared::GpuShared) -> Result<(), String> {
    g.win.rebuild(primary, None)?;
    after_rebuild(g);
    Ok(())
}

impl App {
    /// True while the device is lost or being recovered: nothing draws.
    pub(super) fn gpu_recovery_blocks(&self) -> bool {
        gpu_pick::lost_pending() || !matches!(self.gpu_recovery.poll(Instant::now()), Action::Idle)
    }

    /// Runs the recovery state machine; returns when it next needs a look.
    pub(super) fn gpu_recovery_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if let Some(why) = gpu_pick::take_lost() {
            crate::host_base::log(&format!("renderer: recovering after: {why}"));
            self.gpu_recovery.lost(now);
        }
        match self.gpu_recovery.poll(now) {
            Action::Idle => None,
            Action::WaitUntil(t) => Some(t),
            Action::GaveUp => {
                self.banners.set("gpu", "The graphics device was lost and could not be recovered. Restart Termurion.");
                None
            }
            Action::Attempt { api } => {
                self.recover_gpu(api, now);
                match self.gpu_recovery.poll(Instant::now()) {
                    Action::WaitUntil(t) => Some(t),
                    Action::Attempt { .. } => Some(Instant::now()),
                    Action::GaveUp => {
                        self.banners.set("gpu", "The graphics device was lost and could not be recovered. Restart Termurion.");
                        None
                    }
                    Action::Idle => None,
                }
            }
        }
    }

    /// One attempt on API number `api` of the start-up order.
    fn recover_gpu(&mut self, api: usize, now: Instant) {
        let forced = std::env::var(gpu_pick::FORCE_ENV).ok();
        let order = gpu_pick::api_order(crate::platform::IS_WINDOWS, cfg!(target_os = "macos"), forced.as_deref());
        let Some(&(api, label)) = order.get(api) else {
            return self.gpu_recovery.failed(now);
        };
        let rebuilt = match api {
            gpu_pick::Api::Wgpu(backends) => self.rebuild_all_gpus(backends),
            gpu_pick::Api::Metal => self.rebuild_all_metal(),
        };
        match rebuilt {
            Ok(summary) => {
                crate::host_base::log(&format!("renderer: recovered on {label} {summary}"));
                self.gpu_recovery.recovered(now);
                self.prefs_gpu = self.prefs_gpu.take().and(self.gpu.as_ref().map(Gpu::shared));
                self.redraw_every_window();
            }
            Err(e) => {
                crate::host_base::log(&format!("renderer: recovery on {label} failed: {e}"));
                self.gpu_recovery.failed(now);
            }
        }
    }

    fn rebuild_all_gpus(&mut self, backends: wgpu::Backends) -> Result<String, String> {
        let display = self.display_handle.clone().ok_or("no display handle")?;
        let probe = match self.gpu.as_ref() {
            Some(g) => g.window.clone(),
            None => match self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.clone())) {
                Some(w) => w,
                // No terminal window (Wayland's resident one is gone): nothing to move, and the
                // next window makes fresh handles (the caller drops the kept ones; review I3).
                None => return Ok("with no terminal window: the next one gets new handles".to_string()),
            },
        };
        // Only devices made from here on may report a loss.
        gpu_pick::next_generation();
        let (mut primary, probe_surface, mut facts) = make_handles(&probe, &display, power::chosen(), backends)?;
        drop(probe_surface);
        // Start-up's OpenGL move again, so a recovery keeps the integrated GPU it chose.
        let forced = std::env::var(gpu_pick::FORCE_ENV).is_ok();
        if wants_gl_move(&gpu_pick::Facts::of(&primary.adapter.get_info()), &display, power::chosen(), forced) {
            match make_handles(&probe, &display, power::chosen(), wgpu::Backends::GL) {
                Ok((gl, s, f)) => {
                    drop(s);
                    (primary, facts) = (gl, f);
                }
                Err(e) => crate::host_base::log(&format!("renderer: OpenGL's integrated GPU failed in the recovery ({e}), staying on {:?}", primary.adapter.get_info().backend)),
            }
        }
        let info = primary.adapter.get_info();
        let needs_alpha = self.gpu.iter().any(|g| g.win.has_alpha()) || self.windows.parked().any(|(_, s)| s.gpu.as_ref().is_some_and(|g| g.win.has_alpha()));
        let alpha = if needs_alpha {
            let w = self.gpu.iter().chain(self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref())).find(|g| g.win.has_alpha()).map(|g| g.window.clone()).unwrap_or(probe);
            Some(transparency_ui::make_alpha_gpu(&display, &w)?)
        } else {
            None
        };
        let mut failure: Option<String> = None;
        let mut each = |slot: &mut Option<Gpu>| {
            if let Some(g) = slot.as_mut()
                && let Err(e) = rebuild_window_gpu(g, &primary, alpha.as_ref())
            {
                failure.get_or_insert(e);
            }
        };
        each(&mut self.gpu);
        for (_, slot) in self.windows.parked_mut() {
            each(&mut slot.gpu);
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if alpha.is_some() {
            self.alpha_gpu = alpha;
        }
        Ok(format!("{} ({})", info.name, gpu_pick::why(power::chosen(), &facts, &gpu_pick::Facts::of(&info))))
    }

    /// A new Metal device for every window (the native backend has no other API to move to).
    #[cfg(target_os = "macos")]
    fn rebuild_all_metal(&mut self) -> Result<String, String> {
        gpu_pick::next_generation();
        let shared = crate::metal_backend::MetalShared::new(power::chosen() == wgpu::PowerPreference::LowPower)?;
        let primary = crate::gpu_shared::GpuShared::Metal(shared.clone());
        let mut failure: Option<String> = None;
        let mut each = |slot: &mut Option<Gpu>| {
            if let Some(g) = slot.as_mut()
                && let Err(e) = rebuild_metal_window(g, &primary)
            {
                failure.get_or_insert(e);
            }
        };
        each(&mut self.gpu);
        for (_, slot) in self.windows.parked_mut() {
            each(&mut slot.gpu);
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(shared.name()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn rebuild_all_metal(&mut self) -> Result<String, String> {
        Err("Metal is macOS only".to_string())
    }

    fn redraw_every_window(&self) {
        if let Some(g) = &self.gpu {
            g.redraw();
        }
        for (_, slot) in self.windows.parked() {
            if let Some(g) = &slot.gpu {
                g.redraw();
            }
        }
    }
}
