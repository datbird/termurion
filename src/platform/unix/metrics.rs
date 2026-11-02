//! No machine meter is read on Unix yet: every sampler answers none, so the status bar shows
//! nothing for them, and the debug log says so once.

use crate::statusbar_metrics::{CpuTicks, NetCounters, PowerState, Samplers};

fn note() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("Machine meters are not available on this platform yet");
    }
}

/// There is no battery reading, so there is no battery meter.
pub fn has_battery() -> bool {
    false
}

pub struct SystemSamplers;

impl Samplers for SystemSamplers {
    fn cpu(&mut self) -> Option<CpuTicks> {
        note();
        None
    }

    fn memory(&mut self) -> Option<(u64, u64)> {
        note();
        None
    }

    fn network(&mut self, _exclude_tunnels: bool) -> Option<NetCounters> {
        note();
        None
    }

    fn power(&mut self) -> Option<PowerState> {
        note();
        None
    }

    fn on_ac(&mut self) -> bool {
        true
    }
}
