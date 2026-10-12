//! Dark menus: the menu bar and its drop-down menus follow Windows' app mode (Settings >
//! Personalization > Colors > "Choose your default app mode"), or a fixed Theme's mode.
//!
//! muda paints the bar itself when told Dark; the drop-downs are the system's popups, which go
//! dark only through uxtheme's undocumented exports (looked up by ordinal in `win`, and skipped
//! when missing, so menus stay light). The decisions are pure and tested here.

/// The process app mode uxtheme's `SetPreferredAppMode` takes (ordinal 135, build 18362 on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum AppMode {
    ForceDark = 2,
    ForceLight = 3,
}

/// Which uxtheme call sets the app mode on this Windows build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// Before Windows 10 1809 (build 17763): no dark menus.
    None,
    /// Build 17763: ordinal 135 is `AllowDarkModeForApp(BOOL)`.
    AllowDarkModeForApp,
    /// Build 18362 on: ordinal 135 is `SetPreferredAppMode(PreferredAppMode)`.
    SetPreferredAppMode,
}

/// The uxtheme call a build has; an unknown build has none.
pub fn api_for_build(build: Option<u32>) -> Api {
    match build {
        Some(b) if b >= 18362 => Api::SetPreferredAppMode,
        Some(b) if b >= 17763 => Api::AllowDarkModeForApp,
        _ => Api::None,
    }
}

/// Windows' app mode from `AppsUseLightTheme` (a DWORD under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`): 0 is dark; a missing
/// value or any other is light.
pub fn dark_from_registry(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

/// Whether the menus are dark: a fixed Theme's mode, else Windows' app mode; high contrast
/// always keeps the system's (light-path) menus, which follow the contrast theme.
pub fn menu_dark(forced: Option<bool>, system_dark: bool, high_contrast: bool) -> bool {
    !high_contrast && forced.unwrap_or(system_dark)
}

/// The app mode for a menu decision.
pub fn app_mode(dark: bool) -> AppMode {
    if dark { AppMode::ForceDark } else { AppMode::ForceLight }
}

/// Whether a `WM_SETTINGCHANGE` names the app mode change (its lParam string).
pub fn is_color_set_change(area: &str) -> bool {
    area == "ImmersiveColorSet"
}

/// A `CurrentBuildNumber` string as a number.
pub fn parse_build(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

#[cfg(windows)]
pub mod win {
    //! The undocumented uxtheme exports, by ordinal, and the registry reads. Every call does
    //! nothing when an export is missing (the menus stay light).

    use super::{Api, AppMode};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};

    const REFRESH_IMMERSIVE_COLOR_POLICY_STATE: usize = 104;
    const ALLOW_DARK_MODE_FOR_WINDOW: usize = 133;
    const SET_PREFERRED_APP_MODE: usize = 135;
    const FLUSH_MENU_THEMES: usize = 136;

    struct Exports {
        api: Api,
        refresh: Option<unsafe extern "system" fn()>,
        allow_window: Option<unsafe extern "system" fn(HWND, i32) -> i32>,
        /// Ordinal 135: `SetPreferredAppMode(i32) -> i32` or, on 17763, `AllowDarkModeForApp(BOOL) -> BOOL`;
        /// both take and return one 32-bit value.
        set_mode: Option<unsafe extern "system" fn(i32) -> i32>,
        flush: Option<unsafe extern "system" fn()>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn exports() -> &'static Exports {
        static E: OnceLock<Exports> = OnceLock::new();
        E.get_or_init(|| {
            let none = Exports { api: Api::None, refresh: None, allow_window: None, set_mode: None, flush: None };
            let api = super::api_for_build(build());
            if api == Api::None {
                return none;
            }
            // SAFETY: loading a system DLL by name; it stays loaded for the process.
            let lib = unsafe { LoadLibraryW(wide("uxtheme.dll").as_ptr()) };
            if lib.is_null() {
                return none;
            }
            // SAFETY: an ordinal lookup (MAKEINTRESOURCE); a missing export gives None. Each
            // export's signature is the one win32-darkmode, muda and winit use.
            unsafe {
                let get = |ord: usize| GetProcAddress(lib, ord as *const u8);
                Exports {
                    api,
                    refresh: get(REFRESH_IMMERSIVE_COLOR_POLICY_STATE).map(|f| std::mem::transmute(f)),
                    allow_window: get(ALLOW_DARK_MODE_FOR_WINDOW).map(|f| std::mem::transmute(f)),
                    set_mode: get(SET_PREFERRED_APP_MODE).map(|f| std::mem::transmute(f)),
                    flush: get(FLUSH_MENU_THEMES).map(|f| std::mem::transmute(f)),
                }
            }
        })
    }

    fn reg_dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
        let (key, name) = (wide(key), wide(name));
        let mut data = 0u32;
        let mut bytes = 4u32;
        // SAFETY: a DWORD read into a 4-byte buffer.
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut bytes) };
        (rc == ERROR_SUCCESS).then_some(data)
    }

    /// Windows' build number (`CurrentBuildNumber`), None when unreadable.
    fn build() -> Option<u32> {
        let (key, name) = (wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), wide("CurrentBuildNumber"));
        let mut buf = [0u16; 32];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: a string read into a buffer of `bytes` bytes.
        let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        super::parse_build(&String::from_utf16_lossy(&buf[..end]))
    }

    /// Windows' app mode now (light when unreadable).
    pub fn system_dark() -> bool {
        super::dark_from_registry(reg_dword(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "AppsUseLightTheme"))
    }

    /// Sets the process app mode the drop-down menus draw in and lets this window draw dark, then
    /// flushes the cached menu themes so the next popup uses the mode.
    pub fn apply(hwnd: isize, mode: AppMode) {
        let e = exports();
        let dark = mode == AppMode::ForceDark;
        // SAFETY: each export was found by its ordinal and takes the arguments given here.
        unsafe {
            if let Some(f) = e.allow_window {
                f(hwnd as HWND, i32::from(dark));
            }
            if let Some(f) = e.set_mode {
                match e.api {
                    Api::SetPreferredAppMode => f(mode as i32),
                    Api::AllowDarkModeForApp => f(i32::from(dark)),
                    Api::None => 0,
                };
            }
            if let Some(f) = e.refresh {
                f();
            }
            if let Some(f) = e.flush {
                f();
            }
        }
    }

    /// The area a `WM_SETTINGCHANGE` names (its lParam, a string or null).
    ///
    /// # Safety
    /// `lparam` is the lParam of a `WM_SETTINGCHANGE` the system delivered.
    pub unsafe fn setting_area(lparam: isize) -> Option<String> {
        if lparam == 0 {
            return None;
        }
        let p = lparam as *const u16;
        let mut n = 0usize;
        // SAFETY: a null-terminated wide string; capped so a bad pointer cannot run on.
        while n < 256 && unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_zero_is_dark_and_anything_else_light() {
        assert!(dark_from_registry(Some(0)));
        assert!(!dark_from_registry(Some(1)));
        assert!(!dark_from_registry(Some(7)));
        // Missing or unreadable: fail safe to light.
        assert!(!dark_from_registry(None));
    }

    #[test]
    fn a_fixed_theme_wins_over_windows() {
        assert!(menu_dark(Some(true), false, false));
        assert!(!menu_dark(Some(false), true, false));
    }

    #[test]
    fn no_fixed_theme_follows_windows() {
        assert!(menu_dark(None, true, false));
        assert!(!menu_dark(None, false, false));
    }

    #[test]
    fn high_contrast_keeps_the_system_menus() {
        assert!(!menu_dark(None, true, true));
        assert!(!menu_dark(Some(true), true, true));
    }

    #[test]
    fn app_mode_forces_the_decision() {
        assert_eq!(app_mode(true), AppMode::ForceDark);
        assert_eq!(app_mode(false), AppMode::ForceLight);
        assert_eq!(AppMode::ForceDark as i32, 2);
        assert_eq!(AppMode::ForceLight as i32, 3);
    }

    #[test]
    fn the_build_picks_the_call() {
        assert_eq!(api_for_build(None), Api::None);
        assert_eq!(api_for_build(Some(17134)), Api::None);
        assert_eq!(api_for_build(Some(17763)), Api::AllowDarkModeForApp);
        assert_eq!(api_for_build(Some(18362)), Api::SetPreferredAppMode);
        assert_eq!(api_for_build(Some(26100)), Api::SetPreferredAppMode);
    }

    #[test]
    fn only_the_color_set_broadcast_counts() {
        assert!(is_color_set_change("ImmersiveColorSet"));
        assert!(!is_color_set_change("WindowsThemeElement"));
        assert!(!is_color_set_change(""));
    }

    #[test]
    fn builds_parse() {
        assert_eq!(parse_build("26100"), Some(26100));
        assert_eq!(parse_build(" 19045 "), Some(19045));
        assert_eq!(parse_build("x"), None);
    }
}
