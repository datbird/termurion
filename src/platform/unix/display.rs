//! Windows-only extras: the display-change subclass and the DWM backdrop have no Unix meaning.
//! winit raises its own events for monitor changes, and the compositor draws any blur.

/// winit reports display changes on Unix, so there is no hook to register.
pub fn on_change(_f: Box<dyn Fn() + Send + Sync>) {}

/// No subclass is installed; false says so.
pub fn watch(_hwnd: isize) -> bool {
    false
}

/// No backdrop is requested; false says it was not applied.
pub fn set_blur(_hwnd: isize, _on: bool) -> bool {
    false
}

/// The window manager places a sheet; there is no owner client rect to read.
pub fn sheet_origin(_owner: isize, _width: u32) -> Option<(i32, i32)> {
    None
}

/// winit lists monitors per window, and the app holds no window here: the height is unknown, so 0.
pub fn primary_height() -> f64 {
    0.0
}

/// No monitor list without a window yet (winit's `available_monitors` needs one).
pub fn work_areas() -> Vec<(f64, f64, f64, f64)> {
    Vec::new()
}
