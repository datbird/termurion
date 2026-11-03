//! The shell's virtual desktops: the `IVirtualDesktopManager` and the registry list Explorer keeps.

use crate::desktops::{DesktopId, ids_from_registry, REG_CURRENT, REG_IDS, REG_PATH};
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
