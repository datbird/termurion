//! Window chrome on Unix. DWM has no Unix meaning (the window manager draws border and corners);
//! the native menu bar is not attached yet and says so.

use crate::platform::Unsupported;

/// What the menu bar says when it cannot attach.
fn menu_unsupported() -> String {
    Unsupported { feature: "The native menu bar" }.to_string()
}

pub fn set_border(_hwnd: isize, _color: u32) {}

pub fn set_corners(_hwnd: isize, _preference: i32) {}

pub fn invisible_top_bottom(_hwnd: isize) -> (f32, f32) {
    (0.0, 0.0)
}

pub fn hide_menu(_menu: &muda::Menu, _hwnd: isize) -> Result<(), String> {
    Err(menu_unsupported())
}

pub fn show_menu(_menu: &muda::Menu, _hwnd: isize) -> Result<(), String> {
    Err(menu_unsupported())
}

pub fn menu_attached(_hwnd: isize) -> bool {
    false
}

pub fn apply_menu_theme(_hwnd: isize, _forced: Option<bool>, _high_contrast: bool, _bar: Option<&muda::Menu>) -> Result<(), String> {
    Ok(())
}

/// No subclass: winit tells the app about a theme change itself.
pub fn guard_theme(_hwnd: isize, _wake: Box<dyn Fn()>) {}

pub fn theme_reset_handled() {}

pub fn shift_down() -> bool {
    false
}
