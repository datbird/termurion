//! Win32 calls of the hotkey window.

/// The hotkey window works on Windows.
pub fn available() -> Result<(), super::super::Unsupported> {
    Ok(())
}

pub fn cloak(hwnd: isize, on: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let v: i32 = on as i32;
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&v as *const i32).cast(), 4) };
}

/// Sets the window's opacity for a fade: `WS_EX_LAYERED` is added for the animation and
/// removed again at full opacity, so the swap chain keeps its normal path outside a fade.
/// False when Windows refused the attribute (the fade then stops).
pub fn set_alpha(hwnd: isize, alpha: f32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, IsWindow, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW, WS_EX_LAYERED};
    unsafe {
        if IsWindow(hwnd as _) == 0 {
            return false;
        }
        let ex = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let layered = ex & WS_EX_LAYERED as isize != 0;
        if alpha >= 1.0 {
            if layered {
                SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
            }
            return true;
        }
        if !layered {
            SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        }
        SetLayeredWindowAttributes(hwnd as _, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, LWA_ALPHA) != 0
    }
}

pub fn foreground() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// Windows follows the desktop through `desktops::follows`, so there is nothing to set.
pub fn join_active_space(_hwnd: isize) {}

/// No per-window state is kept here.
pub fn forget(_view: isize) {}

/// A window of this process (a terminal window, Settings, a dialog).
pub fn ours(hwnd: isize) -> bool {
    let mut pid = 0u32;
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd as _, &mut pid) };
    pid == std::process::id()
}

/// The handle still names a window.
pub fn is_window(hwnd: isize) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd as _) != 0 }
}

/// Puts the window on top of the others without activating it; a minimized one is restored the
/// same way.
pub fn order_front(hwnd: isize, minimized: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOP, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow};
    if minimized {
        unsafe { ShowWindow(hwnd as _, SW_SHOWNOACTIVATE) };
    }
    unsafe { SetWindowPos(hwnd as _, HWND_TOP, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn window_alpha_adds_and_removes_the_layered_style() {
        unsafe {
            let class: Vec<u16> = "TermurionAlphaTest\0".encode_utf16().collect();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() });
            // A tool window that is never shown, off every monitor.
            let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), class.as_ptr(), WS_POPUP, -30000, -30000, 16, 16, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            assert!(!h.is_null());
            let layered = || GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_LAYERED as isize != 0;
            assert!(!layered());
            assert!(set_alpha(h as isize, 0.5));
            assert!(layered(), "a fade adds the layered style");
            let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
            GetLayeredWindowAttributes(h, &mut key, &mut alpha, &mut flags);
            assert_eq!((alpha, flags), (128, LWA_ALPHA));
            // Later steps of a fade are the same call; a desktop-less ssh session (0) refuses a second
            // `SetLayeredWindowAttributes` on a window (error 31), so they are checked on the checklist.
            assert!(set_alpha(h as isize, 1.0));
            assert!(!layered(), "full opacity removes the style");
            assert!(set_alpha(h as isize, 1.0), "removing twice is fine");
            DestroyWindow(h);
            assert!(!set_alpha(h as isize, 0.5), "a destroyed window refuses");
        }
    }
}
