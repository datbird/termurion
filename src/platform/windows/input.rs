//! Win32 input preferences.

pub fn alt_down() -> bool {
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) < 0 }
}

pub fn double_click_ms() -> u128 {
    u128::from(unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() })
}

/// "Show animations in Windows"; a failed query leaves the default: animations on.
pub fn animations_on() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut on: i32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
    on != 0
}
