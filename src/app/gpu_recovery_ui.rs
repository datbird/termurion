//! Recovery after a lost graphics device (a driver update or reset, a timeout, a GPU removed, a laptop
//! switching GPUs): every window gets a new adapter, device, surface and renderer resources through the
//! same API order and GPU preference as start-up. The state machine is `gpu_pick::Recovery`.

use super::*;
use crate::gpu_pick::Action;

/// Moves one window onto new handles. Hands the window back unchanged, with the reason, when its new
/// surface cannot be made, so a failed attempt loses no window.
fn rebuild_window_gpu(old: Gpu, primary: &GpuHandles, alpha: Option<&AlphaGpu>) -> Result<Gpu, (Box<Gpu>, String)> {
    let target = if old.alpha.is_some() { alpha } else { None };
    if old.alpha.is_some() && target.is_none() {
        return Err((Box::new(old), "no see-through instance".to_string()));
    }
    let (instance, adapter, device, queue) = match target {
        Some(a) => (&a.instance, &a.adapter, &a.device, &a.queue),
        None => (&primary.instance, &primary.adapter, &primary.device, &primary.queue),
    };
    // Made before the old surface goes: a failure leaves the window as it was.
    let surface = match instance.create_surface(old.window.clone()) {
        Ok(s) => s,
        Err(e) => return Err((Box::new(old), format!("no surface: {e}"))),
    };
    let caps = surface.get_capabilities(adapter);
    let Some(format) = caps.formats.iter().copied().find(|f| f.is_srgb()).or_else(|| caps.formats.first().copied()) else {
        return Err((Box::new(old), "the surface offers no format".to_string()));
    };
    let Gpu { surface: dead, config: old_config, mut renderer, shape, window, alpha: old_alpha, .. } = old;
    // One swap chain per window: the old one goes before the new one is configured.
    drop(dead);
    let alpha_mode = if caps.alpha_modes.contains(&old_config.alpha_mode) { old_config.alpha_mode } else { crate::transparency::pick_alpha_mode(&caps.alpha_modes, false) };
    let config = wgpu::SurfaceConfiguration {
        format,
        alpha_mode,
        view_formats: if old_alpha.is_some() && format.is_srgb() { vec![format.remove_srgb_suffix()] } else { vec![] },
        ..old_config
    };
    surface.configure(device, &config);
    renderer.rebuild_gpu(device, queue, format);
    Ok(Gpu {
        surface,
        adapter: primary.adapter.clone(),
        device: primary.device.clone(),
        queue: primary.queue.clone(),
        config,
        renderer,
        instance: primary.instance.clone(),
        alpha: target.cloned(),
        shape,
        window,
    })
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
            crate::host::log(&format!("renderer: recovering after: {why}"));
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
        let order = gpu_pick::backend_order(cfg!(windows), forced.as_deref());
        let Some(&(backends, label)) = order.get(api) else {
            return self.gpu_recovery.failed(now);
        };
        match self.rebuild_all_gpus(backends) {
            Ok(summary) => {
                crate::host::log(&format!("renderer: recovered on {label} {summary}"));
                self.gpu_recovery.recovered(now);
                self.prefs_gpu = self.prefs_gpu.take().and(self.gpu.as_ref().map(|g| (g.instance.clone(), g.adapter.clone(), g.device.clone(), g.queue.clone())));
                self.redraw_every_window();
            }
            Err(e) => {
                crate::host::log(&format!("renderer: recovery on {label} failed: {e}"));
                self.gpu_recovery.failed(now);
            }
        }
    }

    fn rebuild_all_gpus(&mut self, backends: wgpu::Backends) -> Result<String, String> {
        let display = self.display_handle.clone().ok_or("no display handle")?;
        let probe = match self.gpu.as_ref() {
            Some(g) => g.window.clone(),
            None => self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.clone())).ok_or("no window")?,
        };
        // Only devices made from here on may report a loss.
        gpu_pick::next_generation();
        let (primary, probe_surface, facts) = make_handles(&probe, &display, power::chosen(), backends)?;
        drop(probe_surface);
        let info = primary.adapter.get_info();
        let needs_alpha = self.gpu.iter().any(|g| g.alpha.is_some()) || self.windows.parked().any(|(_, s)| s.gpu.as_ref().is_some_and(|g| g.alpha.is_some()));
        let alpha = if needs_alpha {
            let w = self.gpu.iter().chain(self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref())).find(|g| g.alpha.is_some()).map(|g| g.window.clone()).unwrap_or(probe);
            Some(transparency_ui::make_alpha_gpu(&display, &w)?)
        } else {
            None
        };
        let mut failure: Option<String> = None;
        let mut each = |slot: &mut Option<Gpu>| {
            if let Some(old) = slot.take() {
                match rebuild_window_gpu(old, &primary, alpha.as_ref()) {
                    Ok(g) => *slot = Some(g),
                    Err((g, e)) => {
                        *slot = Some(*g);
                        failure.get_or_insert(e);
                    }
                }
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

    fn redraw_every_window(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
        for (_, slot) in self.windows.parked() {
            if let Some(g) = &slot.gpu {
                g.window.request_redraw();
            }
        }
    }
}
