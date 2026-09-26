//! The status bar graphs' Windows samplers (milestone 9i, Task 3): `Samplers` over Win32.
//! CPU from `GetSystemTimes`, memory from `GlobalMemoryStatusEx`, network from
//! `GetIfTable2`, power from `GetSystemPowerStatus` and `CallNtPowerInformation`. Each call
//! takes microseconds; `src/app/metrics_ui.rs` makes them on `statusbar_metrics::Meters`'
//! schedule (at most once a second each), never per frame.

use crate::statusbar_metrics::{CpuTicks, NetCounters, PowerState, Samplers};
use std::collections::BTreeSet;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2};
use windows_sys::Win32::System::Power::{CallNtPowerInformation, GetSystemPowerStatus, SystemBatteryState, SYSTEM_BATTERY_STATE, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// `IF_TYPE_SOFTWARE_LOOPBACK`: the `lo` interfaces iTerm2 skips (`NETS:98-172`).
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// `IF_TYPE_TUNNEL`: Windows' analog of `utun`.
const IF_TYPE_TUNNEL: u32 = 131;
/// `InterfaceAndOperStatusFlags.FilterInterface`: an NDIS filter's row repeats its
/// adapter's counters.
const FILTER_INTERFACE_BIT: u8 = 1 << 1;
/// `BatteryFlag` values that mean no battery: 128 "no system battery", 255 "unknown".
const NO_BATTERY_FLAGS: [u8; 2] = [128, 255];
/// `BatteryLifePercent` and `BatteryLifeTime` when unknown.
const UNKNOWN_PERCENT: u8 = 255;
const UNKNOWN_TIME: u32 = 0xFFFF_FFFF;

/// The Win32 samplers. Stateless: the meters keep the history.
#[derive(Debug, Default)]
pub struct WinSamplers;

fn ticks(t: FILETIME) -> f64 {
    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64
}

fn power_status() -> Option<SYSTEM_POWER_STATUS> {
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    (unsafe { GetSystemPowerStatus(&mut s) } != 0).then_some(s)
}

/// This computer has a battery (`BAT:59-79`: read once per process).
pub fn has_battery() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| power_status().is_some_and(|s| !NO_BATTERY_FLAGS.contains(&s.BatteryFlag)))
}

/// A row Network Throughput counts: not loopback, not an NDIS filter's copy of its
/// adapter, and with `exclude_tunnels` not a tunnel.
fn counted(row: &MIB_IF_ROW2, exclude_tunnels: bool) -> bool {
    if row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE_BIT != 0 {
        return false;
    }
    !(exclude_tunnels && (row.Type == IF_TYPE_TUNNEL || row.TunnelType != 0))
}

impl Samplers for WinSamplers {
    /// Kernel time includes idle time, so the total is kernel plus user.
    fn cpu(&mut self) -> Option<CpuTicks> {
        let (mut idle, mut kernel, mut user): (FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        Some(CpuTicks { idle: ticks(idle), total: ticks(kernel) + ticks(user) })
    }

    fn memory(&mut self) -> Option<(u64, u64)> {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
            return None;
        }
        Some((m.ullTotalPhys.saturating_sub(m.ullAvailPhys), m.ullTotalPhys))
    }

    /// The kept rows' byte counters, summed; the interface set is each kept row's LUID and
    /// hardware address, so an adapter coming or going is seen (`NETS:104-114`).
    fn network(&mut self, exclude_tunnels: bool) -> Option<NetCounters> {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        let err = unsafe { GetIfTable2(&mut table) };
        if err != 0 || table.is_null() {
            if !table.is_null() {
                unsafe { FreeMibTable(table as *const _) };
            }
            return None;
        }
        let mut out = NetCounters { down: 0.0, up: 0.0, interfaces: BTreeSet::new() };
        let n = unsafe { (*table).NumEntries } as usize;
        let rows = unsafe { std::slice::from_raw_parts(std::ptr::addr_of!((*table).Table) as *const MIB_IF_ROW2, n) };
        for row in rows.iter().filter(|r| counted(r, exclude_tunnels)) {
            out.down += row.InOctets as f64;
            out.up += row.OutOctets as f64;
            let mut key = unsafe { row.InterfaceLuid.Value }.to_le_bytes().to_vec();
            let len = (row.PhysicalAddressLength as usize).min(row.PhysicalAddress.len());
            key.extend_from_slice(&row.PhysicalAddress[..len]);
            out.interfaces.insert(key);
        }
        unsafe { FreeMibTable(table as *const _) };
        Some(out)
    }

    /// No battery is no state (`PWR:148-212`: no source). The percentage is 100 when
    /// Windows does not know it; the time is to full while charging (from the battery's
    /// rate), else the time left, -1 when unknown.
    fn power(&mut self) -> Option<PowerState> {
        let s = power_status().filter(|s| !NO_BATTERY_FLAGS.contains(&s.BatteryFlag))?;
        let percentage = if s.BatteryLifePercent == UNKNOWN_PERCENT { 100 } else { i32::from(s.BatteryLifePercent) };
        let mut b: SYSTEM_BATTERY_STATE = unsafe { std::mem::zeroed() };
        let status = unsafe { CallNtPowerInformation(SystemBatteryState, std::ptr::null(), 0, &mut b as *mut _ as *mut _, std::mem::size_of::<SYSTEM_BATTERY_STATE>() as u32) };
        let charging = status == 0 && b.Charging != 0;
        let time = if charging {
            // `Rate` is signed milliwatts: positive while charging.
            let rate = b.Rate as i32;
            if rate > 0 { f64::from(b.MaxCapacity.saturating_sub(b.RemainingCapacity)) * 3600.0 / f64::from(rate) } else { -1.0 }
        } else if s.BatteryLifeTime == UNKNOWN_TIME {
            -1.0
        } else {
            f64::from(s.BatteryLifeTime)
        };
        Some(PowerState { percentage: Some(percentage), charging, time })
    }

    fn on_ac(&mut self) -> bool {
        power_status().is_some_and(|s| s.ACLineStatus == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samplers_read_this_machine() {
        let mut s = WinSamplers;
        let cpu = s.cpu().expect("GetSystemTimes");
        assert!(cpu.total > cpu.idle, "{cpu:?}");
        let (_, total) = s.memory().expect("GlobalMemoryStatusEx");
        assert!(total > 0);
        assert!(s.network(true).is_some());
        assert!(s.network(false).is_some());
        let _ = s.power();
        let _ = s.on_ac();
        let _ = has_battery();
    }
}
