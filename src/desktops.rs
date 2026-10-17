//! Virtual desktops (13g): ids, the registry list, the restore target, and the saved-state key.
//! Pure here; `win` holds the `IVirtualDesktopManager` and the registry reads.

use toml::Value;

/// A desktop's id: the GUID's canonical value (Data1 in the top 32 bits).
pub type DesktopId = u128;

/// The saved-state key a window table carries (never an arrangement).
pub const STATE_KEY: &str = "Termurion Virtual Desktop";
/// The global setting that turns the restore on.
pub const RESTORE_KEY: &str = "RestoreWindowsToSameSpaces";
/// Explorer's own registry key (undocumented, read only).
pub const REG_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops";
pub const REG_IDS: &str = "VirtualDesktopIDs";
pub const REG_CURRENT: &str = "CurrentVirtualDesktop";

/// A GUID in memory layout (Data1 to Data3 little endian) as an id.
pub fn from_guid_bytes(b: &[u8; 16]) -> DesktopId {
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u128;
    let d2 = u16::from_le_bytes([b[4], b[5]]) as u128;
    let d3 = u16::from_le_bytes([b[6], b[7]]) as u128;
    let mut d4 = 0u128;
    for &x in &b[8..] {
        d4 = d4 << 8 | x as u128;
    }
    d1 << 96 | d2 << 80 | d3 << 64 | d4
}

/// The inverse of `from_guid_bytes`.
pub fn to_guid_bytes(id: DesktopId) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&((id >> 96) as u32).to_le_bytes());
    b[4..6].copy_from_slice(&((id >> 80) as u16).to_le_bytes());
    b[6..8].copy_from_slice(&((id >> 64) as u16).to_le_bytes());
    b[8..].copy_from_slice(&(id as u64).to_be_bytes());
    b
}

/// The ids in Explorer's `VirtualDesktopIDs` value: concatenated 16-byte GUIDs. Zero ids and a
/// short tail are skipped; nothing here panics on odd lengths.
pub fn ids_from_registry(bytes: &[u8]) -> Vec<DesktopId> {
    bytes
        .chunks_exact(16)
        .filter_map(|c| {
            let mut a = [0u8; 16];
            a.copy_from_slice(c);
            Some(from_guid_bytes(&a)).filter(|&id| id != 0)
        })
        .collect()
}

/// "{01020304-0506-0708-090A-0B0C0D0E0F10}".
pub fn to_text(id: DesktopId) -> String {
    format!("{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}", (id >> 96) as u32, (id >> 80) as u16, (id >> 64) as u16, (id >> 48) as u16, id as u64 & 0xFFFF_FFFF_FFFF)
}

/// The text form back, braces optional, either case.
pub fn from_text(s: &str) -> Option<DesktopId> {
    let s = s.trim();
    let s = s.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(s);
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    if parts.len() != 5 || parts.iter().zip(lens).any(|(p, n)| p.len() != n || !p.bytes().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    u128::from_str_radix(&parts.concat(), 16).ok()
}

/// The desktop a saved window goes back to: only one that still exists (`GP:3496`; iTerm2
/// ignores a space that is gone).
pub fn restore_target(saved: Option<&str>, desktops: &[DesktopId]) -> Option<DesktopId> {
    let id = from_text(saved?)?;
    desktops.contains(&id).then_some(id)
}

/// The row is enabled only under the system restore policy: no default arrangement and not
/// hotkey-only (`GP:3496-3506`; Termurion has no "close windows when quitting" switch).
pub fn restore_row_enabled(open_arrangement: bool, open_no_windows: bool) -> bool {
    !open_arrangement && !open_no_windows
}

/// Writes the window's desktop into a saved-state window table; None writes nothing.
pub fn stamp(window: &mut Value, id: Option<DesktopId>) {
    if let (Some(t), Some(id)) = (window.as_table_mut(), id) {
        t.insert(STATE_KEY.into(), Value::String(to_text(id)));
    }
}

/// The desktop text a saved window carries.
pub fn saved(window: &Value) -> Option<&str> {
    window.get(STATE_KEY)?.as_str()
}

/// Where a profile's Space opens a new window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Current,
    All,
    Desktop(DesktopId),
    Missing(i64),
}

/// The profile's Space against the desktop list: 0 current, -1 every desktop, 1 to 9 the n-th
/// desktop in Task View order (missing when there is none), anything else current
/// (`iTermController.m:1052-1061`).
pub fn space_target(space: i64, desktops: &[DesktopId]) -> Target {
    match space {
        -1 => Target::All,
        1..=9 => desktops.get((space - 1) as usize).map_or(Target::Missing(space), |d| Target::Desktop(*d)),
        _ => Target::Current,
    }
}

/// The notification title when a numbered desktop does not exist (`iTermMissionControlHacks.m:55-61`).
pub fn cant_switch(n: i64) -> String {
    format!("Can\u{2019}t switch to desktop {n}")
}

pub const CANT_SWITCH_FIX: &str = "That desktop does not exist. Add desktops in Task View (Win+Tab).";
pub const SPACES_WARNING_ID: &str = "NeverWarnAboutSpaces";
pub const SPACES_WARNING_HEADING: &str = "Warning";
pub const SPACES_WARNING: &str = "To have a new window open on a specific desktop, make sure that the desktop exists in Task View (Win+Tab).";
/// How often a following ("All Desktops") window is checked against the current desktop.
pub const FOLLOW_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How long after activating a window on another desktop to check that Windows switched to it
/// (iTerm2 sleeps 0.3 s after its Ctrl+number, `MCH:55-90`).
pub const SWITCH_RECHECK: std::time::Duration = std::time::Duration::from_millis(300);

/// Whether a window follows the current desktop for a profile's Space (All Desktops only).
pub fn follows(space: i64) -> bool {
    space == -1
}

/// Whether a profile's Space applies to a window: a hotkey window only follows All Desktops
/// (iTerm2 never switches a hotkey window's desktop), so a numbered Space does nothing for it,
/// not even the missing-desktop notice.
pub fn space_applies(hotkey: bool, space: i64) -> bool {
    space != 0 && (!hotkey || follows(space))
}

/// The switch failed when the window is still not on the current desktop; an unreadable
/// answer is not counted.
pub fn switch_failed(on_current: Option<bool>) -> bool {
    on_current == Some(false)
}

#[cfg(windows)]
pub mod win {
    use super::{DesktopId, ids_from_registry, REG_CURRENT, REG_IDS, REG_PATH};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    use windows::core::GUID;

    /// The shell's virtual desktop manager (documented, own windows only) and the registry list
    /// Explorer keeps (undocumented, read only; every failure is "no list" or "current").
    pub struct Manager {
        vdm: IVirtualDesktopManager,
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    fn guid_id(g: &GUID) -> DesktopId {
        g.to_u128()
    }

    /// A binary registry value under Explorer's desktops key. The size comes first; a value
    /// that grew between the two calls is tried once more.
    fn reg_binary(name: &str) -> Option<Vec<u8>> {
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (path, value) = (wide(REG_PATH), wide(name));
        for _ in 0..2 {
            let mut size = 0u32;
            let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), value.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
            if rc != 0 || size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), value.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
            match rc {
                0 => {
                    buf.truncate(size as usize);
                    return Some(buf);
                }
                234 => continue, // ERROR_MORE_DATA: the value grew
                _ => return None,
            }
        }
        None
    }

    impl Manager {
        /// None when the shell has no virtual desktop manager (no Explorer shell, an ssh logon):
        /// every desktop feature is then off.
        pub fn new() -> Option<Manager> {
            let make = || unsafe { CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL) };
            let vdm = match make() {
                Ok(v) => v,
                Err(e) if e.code().0 as u32 == 0x800401F0 => {
                    // CO_E_NOTINITIALIZED: winit's OLE was not up on this thread; start COM once.
                    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                    make().ok()?
                }
                Err(_) => return None,
            };
            Some(Manager { vdm })
        }

        /// The desktop the window is on; None for a window the shell does not know (or a
        /// cloaked one that fails).
        pub fn window_desktop(&self, hwnd_: isize) -> Option<DesktopId> {
            let g = unsafe { self.vdm.GetWindowDesktopId(hwnd(hwnd_)) }.ok()?;
            Some(guid_id(&g)).filter(|&id| id != 0)
        }

        pub fn on_current(&self, hwnd_: isize) -> Option<bool> {
            unsafe { self.vdm.IsWindowOnCurrentVirtualDesktop(hwnd(hwnd_)) }.ok().map(|b| b.as_bool())
        }

        /// Moves one of Termurion's own windows (the shell refuses foreign ones).
        pub fn move_window(&self, hwnd_: isize, id: DesktopId) -> Result<(), String> {
            let g = GUID::from_u128(id);
            unsafe { self.vdm.MoveWindowToDesktop(hwnd(hwnd_), &g) }.map_err(|e| e.to_string())
        }

        /// Task View order, from Explorer's registry value; empty when it is missing (one desktop,
        /// Windows 10) or odd.
        pub fn list(&self) -> Vec<DesktopId> {
            reg_binary(REG_IDS).map(|b| ids_from_registry(&b)).unwrap_or_default()
        }

        /// The current desktop: Explorer's value on Windows 11, else the foreground window's.
        pub fn current(&self) -> Option<DesktopId> {
            if let Some(b) = reg_binary(REG_CURRENT)
                && let Some(id) = ids_from_registry(&b).first() {
                    return Some(*id);
                }
            let fg = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            if fg.is_null() { None } else { self.window_desktop(fg as isize) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: [u8; 16] = [0x04, 0x03, 0x02, 0x01, 0x06, 0x05, 0x08, 0x07, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10];

    /// iTerm2 hotkey windows show where the person is: only All Desktops (-1) keeps one following,
    /// and a window that did not end up on the current desktop after the switch is reported.
    #[test]
    fn hotkey_and_switch_follow_iterm2() {
        assert!(follows(-1));
        assert!(!follows(0) && !follows(3));
        assert_eq!(SWITCH_RECHECK, std::time::Duration::from_millis(300));
        assert!(switch_failed(Some(false)));
        assert!(!switch_failed(Some(true)));
        assert!(!switch_failed(None), "an unreadable answer is not a failure");
    }

    #[test]
    fn space_target_follows_iterm2() {
        let d = [1u128, 2, 3];
        assert_eq!(space_target(0, &d), Target::Current);
        assert_eq!(space_target(-1, &d), Target::All);
        assert_eq!(space_target(1, &d), Target::Desktop(1));
        assert_eq!(space_target(3, &d), Target::Desktop(3));
        assert_eq!(space_target(4, &d), Target::Missing(4));
        assert_eq!(space_target(9, &d), Target::Missing(9));
        assert_eq!(space_target(10, &d), Target::Current);
        assert_eq!(space_target(-5, &d), Target::Current);
    }

    #[test]
    fn space_texts_follow_windows() {
        assert_eq!(cant_switch(4), "Can\u{2019}t switch to desktop 4");
        assert_eq!(CANT_SWITCH_FIX, "That desktop does not exist. Add desktops in Task View (Win+Tab).");
        assert_eq!(SPACES_WARNING, "To have a new window open on a specific desktop, make sure that the desktop exists in Task View (Win+Tab).");
        assert_eq!(SPACES_WARNING_ID, "NeverWarnAboutSpaces");
        assert_eq!(SPACES_WARNING_HEADING, "Warning");
        assert_eq!(FOLLOW_EVERY, std::time::Duration::from_secs(1));
    }

    #[test]
    fn desktop_ids_follow_windows() {
        let mut v = GUID.to_vec();
        v.extend([0u8; 16]);
        v.extend([1, 2, 3, 4, 5]);
        let ids = ids_from_registry(&v);
        assert_eq!(ids.len(), 1);
        assert_eq!(to_text(ids[0]), "{01020304-0506-0708-090A-0B0C0D0E0F10}");
        assert_eq!(to_guid_bytes(from_guid_bytes(&GUID)), GUID);
        assert!(ids_from_registry(&[]).is_empty());
        assert!(ids_from_registry(&[7; 15]).is_empty());
    }

    #[test]
    fn desktop_text_round_trips() {
        for x in [1u128, from_guid_bytes(&GUID), u128::MAX] {
            assert_eq!(from_text(&to_text(x)), Some(x));
        }
        assert_eq!(from_text("{nope}"), None);
        assert_eq!(from_text(&to_text(from_guid_bytes(&GUID)).to_lowercase()), Some(from_guid_bytes(&GUID)));
        assert_eq!(from_text(""), None);
    }

    #[test]
    fn restore_target_follows_iterm2() {
        let (a, b) = (from_guid_bytes(&GUID), 99u128);
        assert_eq!(restore_target(Some(&to_text(a)), &[a, b]), Some(a));
        assert_eq!(restore_target(Some(&to_text(a)), &[b]), None);
        assert_eq!(restore_target(None, &[a, b]), None);
        assert_eq!(restore_target(Some("garbage"), &[a, b]), None);
    }

    #[test]
    fn restore_row_follows_iterm2() {
        assert!(restore_row_enabled(false, false));
        assert!(!restore_row_enabled(true, false));
        assert!(!restore_row_enabled(false, true));
        assert!(!restore_row_enabled(true, true));
    }

    #[test]
    fn state_key_is_written_only_for_restoration() {
        let a = from_guid_bytes(&GUID);
        let mut w = Value::Table(Default::default());
        stamp(&mut w, Some(a));
        assert_eq!(w.get(STATE_KEY).and_then(|v| v.as_str()), Some(to_text(a).as_str()));
        assert_eq!(saved(&w), Some(to_text(a).as_str()));
        let mut w2 = Value::Table(Default::default());
        stamp(&mut w2, None);
        assert!(w2.get(STATE_KEY).is_none());
        assert_eq!(saved(&w2), None);
        let facts = crate::arrangement::WindowFacts { x: 0.0, y: 0.0, w: 800.0, h: 600.0, style: 0, selected_tab: 0, title: None, size_locked: false, layout_locked: false, percent: (100.0, 100.0), cells: (80, 25), use_transparency: None };
        let arranged = crate::arrangement::window(facts, Vec::new());
        assert!(arranged.get(STATE_KEY).is_none(), "Save Window Arrangement never writes the key");
    }

    #[cfg(windows)]
    #[test]
    fn manager_answers_for_no_window() {
        let Some(m) = win::Manager::new() else {
            eprintln!("virtual desktop manager unavailable in this session; skipped");
            return;
        };
        assert_eq!(m.window_desktop(0), None);
        eprintln!("desktops listed: {}", m.list().len());
    }

    #[test]
    fn hotkey_windows_skip_numbered_spaces() {
        assert!(space_applies(false, 3));
        assert!(space_applies(true, -1));
        assert!(!space_applies(true, 3));
        assert!(!space_applies(false, 0));
    }
}
