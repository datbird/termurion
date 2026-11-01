//! Which graphics API and which GPU the terminal windows draw with.
//!
//! The API: DirectX 12 on Windows (the one Windows Terminal draws through, and the one the see-through
//! windows already need), then Vulkan, then OpenGL, each tried only when the one before it could not
//! make an adapter, a surface or a device. The GPU: iTerm2's "Prefer integrated to discrete GPU"
//! (`preferIntegratedGPU`, default on; `SessionView.m:862-895`) asks the low-power device, off asks
//! the high-performance one. All of this is decided here over plain lists, so it is tested without a GPU.

use wgpu::{Backend, Backends, DeviceType, PowerPreference};

/// The environment variable that puts one API first (`dx12`, `vulkan` or `gl`), for measuring and for
/// a driver that misbehaves. The others still follow as fallbacks.
pub const FORCE_ENV: &str = "TERMURION_RENDERER";

/// What the choice needs to know about one adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub backend: Backend,
    pub name: String,
    pub kind: DeviceType,
}

impl Facts {
    pub fn of(info: &wgpu::AdapterInfo) -> Facts {
        Facts { backend: info.backend, name: info.name.clone(), kind: info.device_type }
    }
}

/// The APIs to try, first to last: DirectX 12, Vulkan, OpenGL on Windows; the plain default order elsewhere.
/// `forced` (the value of `FORCE_ENV`) moves one API to the front.
pub fn backend_order(windows: bool, forced: Option<&str>) -> Vec<(Backends, &'static str)> {
    let mut order: Vec<(Backends, &'static str)> =
        if windows { vec![(Backends::DX12, "DirectX 12"), (Backends::VULKAN, "Vulkan"), (Backends::GL, "OpenGL")] } else { vec![(Backends::PRIMARY, "Vulkan or Metal"), (Backends::GL, "OpenGL")] };
    let want = match forced.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("dx12" | "d3d12" | "directx") => Some(Backends::DX12),
        Some("vulkan" | "vk") => Some(Backends::VULKAN),
        Some("gl" | "opengl") => Some(Backends::GL),
        _ => None,
    };
    if let Some(i) = want.and_then(|w| order.iter().position(|(b, _)| *b == w)) {
        let first = order.remove(i);
        order.insert(0, first);
    }
    order
}

/// Prefer integrated: the low-power GPU. Off: the high-performance one (the system default would
/// leave the pick to the enumeration order, which is not a decision).
pub fn preference(prefer_integrated: bool) -> PowerPreference {
    if prefer_integrated { PowerPreference::LowPower } else { PowerPreference::HighPerformance }
}

/// How well a device kind suits a preference; lower is better. A software adapter is always last.
fn rank(kind: DeviceType, pref: PowerPreference) -> u8 {
    match (pref, kind) {
        (PowerPreference::LowPower, DeviceType::IntegratedGpu) => 0,
        (PowerPreference::LowPower, DeviceType::DiscreteGpu) => 1,
        (PowerPreference::LowPower, DeviceType::VirtualGpu) => 2,
        (_, DeviceType::DiscreteGpu) => 0,
        (_, DeviceType::IntegratedGpu) => 1,
        (_, DeviceType::VirtualGpu) => 2,
        (_, DeviceType::Other) => 3,
        (_, DeviceType::Cpu) => 4,
    }
}

/// The adapter a preference should land on: the best-ranked, the first of equals (the enumeration
/// order is the system's own: DXGI lists the high-performance adapter first).
pub fn expected_pick(list: &[Facts], pref: PowerPreference) -> Option<usize> {
    (0..list.len()).min_by_key(|&i| (rank(list[i].kind, pref), i))
}

/// Whether the adapter that was handed out is as good as the list allows for this preference.
pub fn honored(list: &[Facts], pref: PowerPreference, chosen: &Facts) -> bool {
    match expected_pick(list, pref) {
        Some(i) => rank(chosen.kind, pref) <= rank(list[i].kind, pref),
        None => true,
    }
}

/// The sentence host.log gives for the pick.
pub fn why(pref: PowerPreference, list: &[Facts], chosen: &Facts) -> String {
    let asked = match pref {
        PowerPreference::LowPower => "Prefer integrated to discrete GPU is on: asked for the low-power GPU",
        PowerPreference::HighPerformance => "Prefer integrated to discrete GPU is off: asked for the high-performance GPU",
        PowerPreference::None => "no preference: the system default",
    };
    let only = list.iter().filter(|f| f.kind != DeviceType::Cpu).count() <= 1;
    let note = if only {
        "only one GPU to choose from"
    } else if honored(list, pref, chosen) {
        "honored"
    } else {
        "NOT honored (the adapter that can present to the window was another kind)"
    };
    format!("{asked}; got {:?}; {note}", chosen.kind)
}

/// One line per adapter, for the single listing in host.log.
pub fn describe(list: &[Facts]) -> String {
    list.iter().map(|f| format!("{:?} {} ({:?})", f.backend, f.name, f.kind)).collect::<Vec<_>>().join("; ")
}

/// The iTerm2 setting only matters when an integrated and a discrete GPU both exist.
pub fn both_kinds(list: &[Facts]) -> bool {
    list.iter().any(|f| f.kind == DeviceType::IntegratedGpu) && list.iter().any(|f| f.kind == DeviceType::DiscreteGpu)
}

/// Whether a device-lost report is a real loss. A device that was dropped on purpose (a recovery
/// replacing it) reports "Dropped" and is not one.
pub fn loss_is_real(reason: &str) -> bool {
    !reason.eq_ignore_ascii_case("dropped")
}

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The device generation: a device made now reports its loss only while it is still the newest.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static LOST: Mutex<Option<String>> = Mutex::new(None);
static WAKE: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Gives the loss reporter a way to wake the event loop (a loss arrives on a driver thread).
pub fn set_wake(f: Arc<dyn Fn() + Send + Sync>) {
    let _ = WAKE.set(f);
}

/// Starts a new device generation; call once when the run's devices are made again; `current_generation` serves devices made later.
pub fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// The generation devices made now belong to (a second device made later for the same run, such as
/// the see-through instance's, shares it).
pub fn current_generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

fn report_lost(generation: u64, text: String) {
    if generation != GENERATION.load(Ordering::SeqCst) {
        return;
    }
    crate::host_base::log(&format!("renderer: {text}"));
    let mut slot = LOST.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(text);
    }
    drop(slot);
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// Reports `device`'s loss (and an uncaptured error that says the device is lost) for `take_lost`.
/// Other uncaptured errors are logged, not fatal: a dead device must not abort the program.
pub fn watch(device: &wgpu::Device, generation: u64) {
    device.set_device_lost_callback(move |reason, message| {
        let reason = format!("{reason:?}");
        if loss_is_real(&reason) {
            report_lost(generation, format!("device lost ({reason}): {message}"));
        }
    });
    device.on_uncaptured_error(Arc::new(move |e| {
        let text = e.to_string();
        if text.to_ascii_lowercase().contains("lost") {
            report_lost(generation, format!("device lost (error): {text}"));
        } else {
            crate::host_base::log(&format!("renderer: GPU error: {text}"));
        }
    }));
}

/// Whether a loss has been reported and not yet taken.
pub fn lost_pending() -> bool {
    LOST.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Takes the pending device-lost report, if any.
pub fn take_lost() -> Option<String> {
    LOST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Tries per API before the next one, and the delay after a failure.
pub const MAX_TRIES: u32 = 3;
const BACKOFF_MS: u64 = 250;
const BACKOFF_CAP_MS: u64 = 4000;
/// A loss this soon after a recovery counts as the same trouble: the next try moves down the API order.
pub const STABLE: std::time::Duration = std::time::Duration::from_secs(60);

pub fn backoff(failures: u32) -> std::time::Duration {
    std::time::Duration::from_millis((BACKOFF_MS << failures.min(8)).min(BACKOFF_CAP_MS))
}

/// What the loss handler should do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Idle,
    /// Nothing to do before this moment (the backoff).
    WaitUntil(std::time::Instant),
    /// Rebuild on API number `api` of the order (an index into `backend_order`).
    Attempt { api: usize },
    /// Every API failed its tries: stop, and tell the person.
    GaveUp,
}

/// The recovery state machine, over plain values (no GPU, no clock of its own).
#[derive(Clone, Debug)]
pub struct Recovery {
    apis: usize,
    api: usize,
    failures: u32,
    pending: bool,
    gave_up: bool,
    due: Option<std::time::Instant>,
    recovered_at: Option<std::time::Instant>,
}

impl Recovery {
    pub fn new(apis: usize) -> Recovery {
        Recovery { apis: apis.max(1), api: 0, failures: 0, pending: false, gave_up: false, due: None, recovered_at: None }
    }

    /// The device was lost. A first loss starts again at the top of the order; a loss soon after a
    /// recovery moves to the next API, so a flapping driver cannot loop on one.
    pub fn lost(&mut self, now: std::time::Instant) {
        if self.gave_up {
            return;
        }
        let quick = self.recovered_at.is_some_and(|t| now.saturating_duration_since(t) < STABLE);
        if quick {
            self.api += 1;
        } else {
            self.api = 0;
        }
        self.failures = 0;
        self.pending = true;
        self.due = None;
        if self.api >= self.apis {
            self.gave_up = true;
            self.pending = false;
        }
    }

    pub fn poll(&self, now: std::time::Instant) -> Action {
        if self.gave_up {
            return Action::GaveUp;
        }
        if !self.pending {
            return Action::Idle;
        }
        match self.due {
            Some(t) if t > now => Action::WaitUntil(t),
            _ => Action::Attempt { api: self.api },
        }
    }

    /// The attempt failed: wait, try again, or move down the order.
    pub fn failed(&mut self, now: std::time::Instant) {
        self.failures += 1;
        if self.failures >= MAX_TRIES {
            self.api += 1;
            self.failures = 0;
            if self.api >= self.apis {
                self.gave_up = true;
                self.pending = false;
                return;
            }
        }
        self.due = Some(now + backoff(self.failures));
    }

    pub fn recovered(&mut self, now: std::time::Instant) {
        self.pending = false;
        self.failures = 0;
        self.due = None;
        self.recovered_at = Some(now);
    }

    pub fn gave_up(&self) -> bool {
        self.gave_up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(backend: Backend, name: &str, kind: DeviceType) -> Facts {
        Facts { backend, name: name.into(), kind }
    }
    fn desktop() -> Vec<Facts> {
        vec![
            f(Backend::Dx12, "NVIDIA GeForce RTX 4080 SUPER", DeviceType::DiscreteGpu),
            f(Backend::Dx12, "Intel(R) UHD Graphics 770", DeviceType::IntegratedGpu),
            f(Backend::Dx12, "Microsoft Basic Render Driver", DeviceType::Cpu),
        ]
    }

    #[test]
    fn windows_tries_dx12_then_vulkan_then_gl() {
        let names: Vec<_> = backend_order(true, None).iter().map(|b| b.1).collect();
        assert_eq!(names, ["DirectX 12", "Vulkan", "OpenGL"]);
    }

    #[test]
    fn a_forced_api_goes_first_and_the_rest_follow() {
        let order = |s| backend_order(true, Some(s)).iter().map(|b| b.1).collect::<Vec<_>>();
        assert_eq!(order("vulkan"), ["Vulkan", "DirectX 12", "OpenGL"]);
        assert_eq!(order(" GL "), ["OpenGL", "DirectX 12", "Vulkan"]);
        assert_eq!(order("dx12"), ["DirectX 12", "Vulkan", "OpenGL"]);
        assert_eq!(order("nonsense"), ["DirectX 12", "Vulkan", "OpenGL"], "an unknown word changes nothing");
    }

    #[test]
    fn other_systems_keep_the_default_order() {
        assert_eq!(backend_order(false, None)[0].0, Backends::PRIMARY);
        assert_eq!(backend_order(false, Some("dx12")).len(), 2, "no DirectX 12 off Windows: nothing to move");
    }

    #[test]
    fn the_setting_maps_to_a_power_preference() {
        assert_eq!(preference(true), PowerPreference::LowPower);
        assert_eq!(preference(false), PowerPreference::HighPerformance);
    }

    #[test]
    fn prefer_integrated_picks_the_integrated_gpu() {
        let l = desktop();
        assert_eq!(expected_pick(&l, preference(true)), Some(1));
        assert_eq!(expected_pick(&l, preference(false)), Some(0));
    }

    #[test]
    fn the_enumeration_order_breaks_ties() {
        let l = vec![f(Backend::Dx12, "A", DeviceType::DiscreteGpu), f(Backend::Dx12, "B", DeviceType::DiscreteGpu)];
        assert_eq!(expected_pick(&l, PowerPreference::HighPerformance), Some(0));
    }

    #[test]
    fn software_is_the_last_resort() {
        let l = vec![f(Backend::Dx12, "Basic", DeviceType::Cpu), f(Backend::Gl, "GL", DeviceType::Other)];
        assert_eq!(expected_pick(&l, PowerPreference::LowPower), Some(1));
        assert_eq!(expected_pick(&[], PowerPreference::LowPower), None);
    }

    #[test]
    fn a_discrete_only_machine_ignores_the_integrated_ask() {
        let l = vec![f(Backend::Dx12, "RTX", DeviceType::DiscreteGpu)];
        assert_eq!(expected_pick(&l, PowerPreference::LowPower), Some(0));
        assert!(!both_kinds(&l));
        assert!(both_kinds(&desktop()));
    }

    #[test]
    fn honored_flags_the_wrong_kind() {
        let l = desktop();
        assert!(honored(&l, PowerPreference::LowPower, &l[1]));
        assert!(!honored(&l, PowerPreference::LowPower, &l[0]));
        assert!(honored(&l, PowerPreference::HighPerformance, &l[0]));
        assert!(!honored(&l, PowerPreference::HighPerformance, &l[1]));
        assert!(!honored(&l, PowerPreference::HighPerformance, &l[2]));
    }

    #[test]
    fn the_log_sentence_says_why() {
        let l = desktop();
        let s = why(preference(true), &l, &l[1]);
        assert!(s.contains("is on") && s.contains("low-power") && s.contains("honored") && !s.contains("NOT"), "{s}");
        let s = why(preference(true), &l, &l[0]);
        assert!(s.contains("NOT honored"), "{s}");
        let one = vec![l[0].clone()];
        assert!(why(preference(false), &one, &one[0]).contains("only one GPU"));
        assert!(describe(&l).contains("Dx12 Intel(R) UHD Graphics 770 (IntegratedGpu)"));
    }

    #[test]
    fn a_dropped_device_is_not_a_loss() {
        assert!(!loss_is_real("Dropped"));
        assert!(loss_is_real("Unknown"));
        assert!(loss_is_real("Destroyed"));
    }

    #[test]
    fn backoff_doubles_then_caps() {
        let ms = |n| backoff(n).as_millis();
        assert_eq!((ms(0), ms(1), ms(2), ms(3), ms(4)), (250, 500, 1000, 2000, 4000));
        assert_eq!(ms(30), 4000);
    }

    #[test]
    fn a_loss_is_retried_at_once_then_recovers() {
        let t0 = std::time::Instant::now();
        let mut r = Recovery::new(3);
        assert_eq!(r.poll(t0), Action::Idle);
        r.lost(t0);
        assert_eq!(r.poll(t0), Action::Attempt { api: 0 });
        r.recovered(t0);
        assert_eq!(r.poll(t0), Action::Idle);
    }

    #[test]
    fn failures_back_off_then_move_down_the_order() {
        let t0 = std::time::Instant::now();
        let mut r = Recovery::new(2);
        r.lost(t0);
        r.failed(t0);
        assert_eq!(r.poll(t0), Action::WaitUntil(t0 + backoff(1)));
        assert_eq!(r.poll(t0 + backoff(1)), Action::Attempt { api: 0 });
        r.failed(t0);
        r.failed(t0);
        assert_eq!(r.poll(t0 + std::time::Duration::from_secs(10)), Action::Attempt { api: 1 }, "three failures: the next API");
        for _ in 0..3 {
            r.failed(t0);
        }
        assert!(r.gave_up());
        assert_eq!(r.poll(t0), Action::GaveUp);
        r.lost(t0);
        assert_eq!(r.poll(t0), Action::GaveUp, "giving up is final for the run");
    }

    #[test]
    fn a_quick_second_loss_moves_to_the_next_api() {
        let t0 = std::time::Instant::now();
        let mut r = Recovery::new(3);
        r.lost(t0);
        r.recovered(t0);
        r.lost(t0 + std::time::Duration::from_secs(5));
        assert_eq!(r.poll(t0), Action::Attempt { api: 1 });
        r.recovered(t0 + std::time::Duration::from_secs(5));
        r.lost(t0 + std::time::Duration::from_secs(6));
        assert_eq!(r.poll(t0), Action::Attempt { api: 2 });
        r.recovered(t0 + std::time::Duration::from_secs(6));
        r.lost(t0 + std::time::Duration::from_secs(7));
        assert_eq!(r.poll(t0), Action::GaveUp, "a loop of quick losses ends");
    }

    #[test]
    fn a_loss_after_a_stable_minute_starts_over_at_the_top() {
        let t0 = std::time::Instant::now();
        let mut r = Recovery::new(3);
        r.lost(t0);
        r.lost(t0);
        r.recovered(t0);
        r.lost(t0 + STABLE + std::time::Duration::from_secs(1));
        assert_eq!(r.poll(t0), Action::Attempt { api: 0 });
    }
}
