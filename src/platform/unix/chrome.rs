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

/// Menus open where the toolkit puts them; there is nothing to lift.
pub struct Lift;

impl Lift {
    pub fn new(_hwnd: isize, _x: f32, _y: f32) -> Lift {
        Lift
    }
}

/// No pop-up without a native handle; the app holds none on Unix (`window::hwnd_of` is None).
pub fn popup<M: muda::ContextMenu + ?Sized>(_menu: &M, _hwnd: isize, _position: Option<muda::dpi::Position>) -> bool {
    false
}

/// The menu bar is not attached on Unix yet: the earlier bar is dropped and the new one is not shown.
pub fn replace_menu_bar(earlier: Option<muda::Menu>, _menu: &muda::Menu, _hwnd: isize, _hide: bool) -> muda::Result<()> {
    drop(earlier);
    Ok(())
}

/// A Windows-only extra: menus here are the toolkit's own.
pub fn install_menu_tips(_hwnd: isize, _delay: std::time::Duration) {}
