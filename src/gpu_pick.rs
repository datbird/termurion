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
}
