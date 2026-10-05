//! Power state (13l): the Windows battery saver is iTerm2's Low Power Mode, the integrated GPU
//! is asked for once per run, and Disable App Nap opts out of EcoQoS.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// What the power line says: on AC (or unknown), and whether battery saver is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub on_ac: bool,
    pub battery_saver: bool,
}

impl State {
    /// iTerm2's `isLowPowerModeEnabled`: the OS switch, not the battery.
    pub fn low_power_mode(&self) -> bool {
        self.battery_saver
    }
}

/// `ACLineStatus` (1 on, 0 off, 255 unknown counts as on) and `SystemStatusFlag` (1 is battery saver on).
pub fn from_status(ac_line: u8, status_flag: u8) -> State {
    State { on_ac: ac_line != 0, battery_saver: status_flag == 1 }
}

/// `SessionView.m:862-895`: Prefer integrated takes the low-power device, else the system default.
pub fn adapter_preference(prefer_integrated: bool) -> wgpu::PowerPreference {
    if prefer_integrated { wgpu::PowerPreference::LowPower } else { wgpu::PowerPreference::None }
}

/// The GPU sheet enables "Prefer integrated" only when an integrated and a discrete GPU exist
/// (`iTermAdvancedGPUSettingsViewController.m:25-34`).
pub fn both_gpu_kinds(kinds: &[wgpu::DeviceType]) -> bool {
    kinds.contains(&wgpu::DeviceType::IntegratedGpu) && kinds.contains(&wgpu::DeviceType::DiscreteGpu)
}

/// The first answer is kept (iTerm2's `dispatch_once`).
pub fn chosen_preference_in(cell: &OnceLock<wgpu::PowerPreference>, prefer_integrated: bool) -> wgpu::PowerPreference {
    *cell.get_or_init(|| adapter_preference(prefer_integrated))
}

static CHOSEN: OnceLock<wgpu::PowerPreference> = OnceLock::new();

/// The preference for this run, fixed by the first caller (the first window's `init_gpu`).
pub fn chosen_preference(prefer_integrated: bool) -> wgpu::PowerPreference {
    chosen_preference_in(&CHOSEN, prefer_integrated)
}

/// The preference the first window used; later requests (the see-through instance) read it so
/// every window lands on one adapter. The system default before any window exists.
pub fn chosen() -> wgpu::PowerPreference {
    CHOSEN.get().copied().unwrap_or(wgpu::PowerPreference::None)
}

static BOTH_KINDS: AtomicBool = AtomicBool::new(false);

/// Set once by `init_gpu` (the settings window has no App).
pub fn set_both_gpu_kinds(on: bool) {
    BOTH_KINDS.store(on, Ordering::Relaxed);
}

pub fn both_gpu_kinds_known() -> bool {
    BOTH_KINDS.load(Ordering::Relaxed)
}

/// The current power line; None when Windows will not say.
#[cfg(windows)]
pub fn sample() -> Option<State> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    (unsafe { GetSystemPowerStatus(&mut s) } != 0).then(|| from_status(s.ACLineStatus, s.SystemStatusFlag))
}

/// Disable App Nap: opts the process out of EcoQoS execution-speed throttling. Where the call is
/// unavailable (Windows before 1709) the error is returned and the caller carries on quietly.
#[cfg(windows)]
pub fn opt_out_of_power_throttling() -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE,
    };
    let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED, StateMask: 0 };
    let ok = unsafe { SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &state as *const _ as *const _, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) };
    if ok != 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};

    /// Review Focus 3: the AC line and battery saver come from `SYSTEM_POWER_STATUS`; iTerm2's
    /// Low Power Mode is the OS switch, not the battery.
    #[test]
    fn power_state_follows_windows() {
        let s = from_status(1, 0);
        assert!(s.on_ac && !s.battery_saver && !s.low_power_mode());
        let s = from_status(0, 1);
        assert!(!s.on_ac && s.battery_saver && s.low_power_mode());
        assert!(from_status(255, 0).on_ac);
        let s = from_status(0, 0);
        assert!(!s.on_ac && !s.low_power_mode());
    }

    /// R3: `preferIntegratedGPU` asks for the low-power device, off asks for the system default;
    /// the sheet's row is live only with both kinds.
    #[test]
    fn adapter_preference_follows_iterm2() {
        assert_eq!(adapter_preference(true), wgpu::PowerPreference::LowPower);
        assert_eq!(adapter_preference(false), wgpu::PowerPreference::None);
        assert!(both_gpu_kinds(&[IntegratedGpu, DiscreteGpu]));
        assert!(!both_gpu_kinds(&[DiscreteGpu]));
        assert!(!both_gpu_kinds(&[IntegratedGpu, Cpu]));
    }

    /// The first answer is kept for the run (iTerm2's `dispatch_once`); the second window's and
    /// the see-through instance's request read the same one.
    #[test]
    fn preference_is_fixed_on_the_first_call() {
        let first = chosen_preference_in(&std::sync::OnceLock::new(), true);
        assert_eq!(first, wgpu::PowerPreference::LowPower);
        let cell = std::sync::OnceLock::new();
        assert_eq!(chosen_preference_in(&cell, false), wgpu::PowerPreference::None);
        assert_eq!(chosen_preference_in(&cell, true), wgpu::PowerPreference::None);
    }

    #[test]
    fn both_kinds_flag_is_false_until_set() {
        assert!(!both_gpu_kinds_known());
        set_both_gpu_kinds(true);
        assert!(both_gpu_kinds_known());
        set_both_gpu_kinds(false);
    }

    #[cfg(windows)]
    #[test]
    fn ecoqos_opt_out_runs() {
        assert_eq!(opt_out_of_power_throttling(), Ok(()));
        assert!(sample().is_some());
    }
}
