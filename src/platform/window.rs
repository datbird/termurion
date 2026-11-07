//! Raw native-handle window operations: the pointer, the window under a point, show and hide, the
//! tool-window style, monitors. A handle is the HWND on Windows. On Unix an app window has no such
//! handle (`hwnd_of` is None), so the app never reaches the rest with one; the Unix versions answer
//! None or false for a handle they are given anyway.

use winit::window::Window;

use crate::layout::Rect;

#[cfg(unix)]
use super::unix::window as imp;
#[cfg(windows)]
use super::windows::window as imp;

/// The app id: the macOS bundle id, the X11 window class and the Wayland app id, and the name of
/// the Linux `.desktop` file, so a launcher finds the running window.
pub const APP_ID: &str = "dev.termurion.Termurion";

/// Alerts and small dialogs draw their own frame (Linux: winit cannot take minimize and maximize
/// off a frame there); Windows and macOS keep the system's.
pub const DIALOGS_DRAW_OWN_FRAME: bool = cfg!(target_os = "linux");

/// A new window's attributes: every Termurion window starts here, so each carries the app id
/// and a dock groups them all under the one launcher.
pub fn attributes() -> winit::window::WindowAttributes {
    with_app_id(Window::default_attributes())
}

/// `attrs` carrying the app id where the window system takes one (X11 class, Wayland app id).
pub fn with_app_id(attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    #[cfg(all(unix, not(target_os = "macos")))]
    let attrs = {
        use winit::platform::wayland::WindowAttributesExtWayland;
        use winit::platform::x11::WindowAttributesExtX11;
        WindowAttributesExtX11::with_name(WindowAttributesExtWayland::with_name(attrs, APP_ID, "termurion"), APP_ID, "termurion")
    };
    attrs
}

/// How `show` changes a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Hide,
    Show,
    ShowNoActivate,
    ShowMaximized,
    ShowMinimizedNoActivate,
    /// Shown, with no activation, in whatever state it last had (`SW_SHOWNA`).
    ShowAsIs,
}

/// How `flash` calls for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flash {
    /// The taskbar button once.
    Once,
    /// The taskbar button until the window is in front.
    UntilFront,
    /// The button and the caption once.
    OnceAll,
    /// Stops a flash.
    Stop,
}

/// A window's native handle, when the OS gives it one.
/// `title` as the OS title bar can draw it. Linux leaves out a character no installed font has
/// (the desktop draws the bar with its own fonts); Windows and macOS fall back to a font that has it.
pub fn os_title(title: &str) -> std::borrow::Cow<'_, str> {
    imp::os_title(title)
}

/// Remembers each window's last title, so a title goes to the window system only when it changes
/// (Wayland sends every `set_title` to the compositor; the frame loop would repeat it each frame).
#[derive(Default)]
pub struct TitleCache(std::collections::HashMap<winit::window::WindowId, String>);

impl TitleCache {
    /// True when `title` differs from what `id` last had (and records it).
    pub fn changed(&mut self, id: winit::window::WindowId, title: &str) -> bool {
        if self.0.get(&id).is_some_and(|t| t == title) {
            return false;
        }
        self.0.insert(id, title.to_string());
        true
    }
}

/// Sets `window`'s title unless it already has that one.
pub fn set_title_if_changed(window: &Window, title: &str) {
    thread_local! {
        static LAST: std::cell::RefCell<TitleCache> = std::cell::RefCell::new(TitleCache::default());
    }
    if LAST.with(|l| l.borrow_mut().changed(window.id(), title)) {
        window.set_title(title);
    }
}

/// Starts, off the UI thread, what `os_title` needs to know (Linux: the title bar's font).
pub fn prepare_os_title(window: &Window) {
    imp::prepare_os_title(window)
}

pub fn hwnd_of(window: &Window) -> Option<isize> {
    imp::hwnd_of(window)
}

/// The native handle menus and the hotkey window take: the HWND on Windows (the same value
/// `hwnd_of` gives), the window's NSView on macOS, none on Linux. `hwnd_of` stays None on macOS so
/// no Windows-only path wakes up there.
pub fn view_of(window: &Window) -> Option<isize> {
    imp::view_of(window)
}

/// Brings `window` front with a notification click's activation token where the window system
/// asks for one (Wayland). Ok(false): nothing was done, raise it the usual way.
pub fn activate(window: &Window, token: Option<&str>) -> Result<bool, String> {
    imp::activate(window, token)
}

/// Lets go of what `activate` keeps on the window system's display, while the display lives.
pub fn forget_activation() {
    imp::forget_activation()
}

/// The pointer's screen point.
/// Flashes the window for attention. Unix has no handle to flash (the app never holds one).
pub fn flash(hwnd: isize, how: Flash) {
    imp::flash(hwnd, how)
}

/// The window is snapped to a half or a quarter of the screen (`IsWindowArranged`, Windows 10 and later).
pub fn is_arranged(hwnd: isize) -> bool {
    imp::is_arranged(hwnd)
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    imp::cursor_pos()
}

/// The top-level window under a screen point.
pub fn root_at(x: i32, y: i32) -> Option<isize> {
    imp::root_at(x, y)
}

/// A screen point in a window's client pixels.
pub fn screen_to_client(hwnd: isize, x: i32, y: i32) -> Option<(i32, i32)> {
    imp::screen_to_client(hwnd, x, y)
}

/// Asks the OS to put a window in front. False when it refused or cannot.
pub fn set_foreground(hwnd: isize) -> bool {
    imp::set_foreground(hwnd)
}

/// Whether a window is the foreground one.
pub fn is_foreground(hwnd: isize) -> bool {
    imp::is_foreground(hwnd)
}

pub fn is_minimized(hwnd: isize) -> bool {
    imp::is_minimized(hwnd)
}

pub fn is_maximized(hwnd: isize) -> bool {
    imp::is_maximized(hwnd)
}

/// Shows or hides a window.
pub fn show(hwnd: isize, how: Show) {
    imp::show(hwnd, how)
}

/// Whether a window has the tool-window style (no taskbar button, no Alt+Tab entry).
pub fn tool_style(hwnd: isize) -> bool {
    imp::tool_style(hwnd)
}

/// Sets or clears the tool-window style.
pub fn set_tool_style(hwnd: isize, want: bool) {
    imp::set_tool_style(hwnd, want)
}

/// Makes a popup click-through, never active, and out of the taskbar.
pub fn set_overlay_style(hwnd: isize) {
    imp::set_overlay_style(hwnd)
}

/// The work area and the whole of the monitor a window is on, in pixels.
pub fn monitor_rects(hwnd: isize) -> Option<(Rect, Rect)> {
    imp::monitor_rects(hwnd)
}

/// A window's restore placement: (show command, flags, normal rect as left/top/right/bottom, shift of
/// the work area from the monitor origin). The shift is zero for a tool window, whose placement is
/// in screen coordinates already.
pub fn placement(hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    imp::placement(hwnd)
}

#[cfg(all(test, target_os = "linux"))] // test: windows on a private X server are Linux's
mod tests {

    /// Every window is built from `attributes`, so none goes without the app id (on Wayland a
    /// dock would not group it with the launcher).
    #[test]
    fn every_window_carries_the_app_id() {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !p.ends_with("platform") {
                        walk(&p, out);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") && std::fs::read_to_string(&p).unwrap().contains(concat!("default_", "attributes()")) {
                    out.push(p.display().to_string());
                }
            }
        }
        let mut found = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "windows built without platform::window::attributes: {found:?}");
    }
    use std::io::Read;

    #[test]
    fn the_desktop_file_names_the_app_id() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/linux");
        let file = dir.join(format!("{}.desktop", super::APP_ID));
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let has = |line: &str| text.lines().any(|l| l == line);
        assert!(has(&format!("StartupWMClass={}", super::APP_ID)), "{text}");
        assert!(has("Exec=termurion %U"), "{text}");
        assert!(has("Categories=System;TerminalEmulator;"), "{text}");
    }

    use std::process::{Command, Stdio};

    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::platform::x11::EventLoopBuilderExtX11;
    use winit::window::{Window, WindowId};

    struct Two(Vec<Window>);

    impl ApplicationHandler for Two {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.0.is_empty() {
                for _ in 0..2 {
                    self.0.push(el.create_window(Window::default_attributes()).unwrap());
                }
            }
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    /// The part that needs a display: the parent test starts one and runs this in a child.
    #[test]
    #[ignore = "run by linux_views_are_distinct_per_window, which gives it an X server"]
    fn two_windows_on_the_display() {
        let mut el = EventLoop::builder().with_x11().with_any_thread(true).build().unwrap();
        let mut app = Two(Vec::new());
        el.pump_app_events(Some(std::time::Duration::from_millis(200)), &mut app);
        assert_eq!(app.0.len(), 2, "both windows exist");
        let a = super::view_of(&app.0[0]).expect("a view for the first");
        let b = super::view_of(&app.0[1]).expect("a view for the second");
        println!("VIEWS {a} {b}");
        assert_ne!(a, b, "one token per window");
        assert!(super::hwnd_of(&app.0[0]).is_none(), "no HWND on Linux");
    }

    #[test]
    fn linux_views_are_distinct_per_window() {
        // A private X server on a display it picks itself.
        let mut xvfb = Command::new("Xvfb")
            .args(["-displayfd", "1", "-screen", "0", "800x600x24", "-nolisten", "tcp"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Xvfb is installed (the Linux checks need it)");
        let mut num = String::new();
        let mut out = xvfb.stdout.take().unwrap();
        let mut byte = [0u8; 1];
        while out.read(&mut byte).unwrap_or(0) == 1 && byte[0] != b'\n' {
            num.push(byte[0] as char);
        }
        let me = std::env::current_exe().unwrap();
        let run = Command::new(me)
            .args(["--exact", "platform::window::tests::two_windows_on_the_display", "--ignored", "--nocapture", "--test-threads=1"])
            .env("DISPLAY", format!(":{}", num.trim()))
            .env_remove("WAYLAND_DISPLAY")
            .output();
        let _ = xvfb.kill();
        let _ = xvfb.wait();
        let run = run.unwrap();
        let text = String::from_utf8_lossy(&run.stdout).to_string() + &String::from_utf8_lossy(&run.stderr);
        assert!(run.status.success(), "{text}");
        assert!(text.contains("VIEWS "), "{text}");
    }
}

#[cfg(test)]
mod title_tests {
    /// A title goes out only when it changes, per window.
    #[test]
    fn a_title_is_sent_only_when_it_changes() {
        let mut c = super::TitleCache::default();
        let (a, b) = (winit::window::WindowId::from(1u64), winit::window::WindowId::from(2u64));
        assert!(c.changed(a, "x"));
        assert!(!c.changed(a, "x"), "the same title again: nothing sent");
        assert!(c.changed(b, "x"), "another window");
        assert!(c.changed(a, "y"));
        assert!(c.changed(a, "x"));
    }
}
