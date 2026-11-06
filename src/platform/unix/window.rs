//! Native window handles do not exist for an app window on Unix (winit gives no HWND), so the app
//! has none to pass in. These answer "nothing" for a handle they are handed anyway.

use winit::window::Window;

use super::super::window::{Flash, Show};
use crate::layout::Rect;

/// The Mac's title bar falls back to a font that has the character: the title as it is.
#[cfg(target_os = "macos")]
pub fn os_title(title: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(title)
}

/// `title` as the title bar can draw it: a character the font that draws the bar lacks is left
/// out (it would show as a box). Which font that is, `title_glyphs` works out once per run.
#[cfg(not(target_os = "macos"))]
pub fn os_title(title: &str) -> std::borrow::Cow<'_, str> {
    if title.is_ascii() {
        return std::borrow::Cow::Borrowed(title);
    }
    std::borrow::Cow::Owned(crate::wintitle::drawable_title(title, title_glyphs::covered))
}

/// What the title bar can draw. On a Wayland desktop without server-side decorations (GNOME)
/// winit draws the bar itself (sctk-adwaita) with one face, the `fc-match` of GNOME's
/// titlebar-font, and no fallback: only that face's characters draw. Elsewhere the window
/// manager draws it with Pango, which falls back across every installed font.
#[cfg(not(target_os = "macos"))]
mod title_glyphs {
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;

    /// Code point ranges, sorted and merged.
    #[derive(Debug, PartialEq)]
    pub struct Charset(Vec<(u32, u32)>);

    impl Charset {
        pub fn has(&self, c: char) -> bool {
            let c = c as u32;
            let at = self.0.partition_point(|(_, hi)| *hi < c);
            self.0.get(at).is_some_and(|(lo, _)| *lo <= c)
        }
    }

    /// fontconfig's `%{charset}` text ("20-7e a0-17f 2100"), one or many lines of it, as one set.
    pub fn parse_charset(text: &str) -> Charset {
        let hex = |t: &str| u32::from_str_radix(t, 16).ok();
        let mut ranges: Vec<(u32, u32)> = text
            .split_whitespace()
            .filter_map(|t| match t.split_once('-') {
                Some((a, b)) => Some((hex(a)?, hex(b)?)),
                None => hex(t).map(|n| (n, n)),
            })
            .filter(|(a, b)| a <= b)
            .collect();
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (lo, hi) in ranges {
            match merged.last_mut() {
                Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        Charset(merged)
    }

    /// The fontconfig pattern sctk-adwaita looks up for `gsettings get
    /// org.gnome.desktop.wm.preferences titlebar-font` ('Adwaita Sans Bold 11'): the last word is
    /// the size, the one before it the style, the rest the family. No answer: "sans-serif".
    pub fn titlebar_pattern(setting: Option<&str>) -> String {
        let conf = setting.map(|s| s.trim().trim_matches('\'')).unwrap_or_default();
        let (head, sized) = match conf.rsplit_once(' ') {
            Some((head, tail)) if tail.chars().all(|c| c.is_numeric()) => (head, true),
            _ => (conf, false),
        };
        let pattern = match head.rsplit_once(' ') {
            Some((name, style)) if !name.is_empty() && (sized || !style.is_empty()) => format!("{name}:{style}"),
            _ => head.to_string(),
        };
        if pattern.is_empty() { "sans-serif".to_string() } else { pattern }
    }

    fn run(cmd: &mut Command) -> Option<String> {
        let out = cmd.stderr(Stdio::null()).output().ok().filter(|o| o.status.success())?;
        String::from_utf8(out.stdout).ok()
    }

    /// The charset of the face `fc-match` picks for `pattern`, as sctk-adwaita opens it.
    pub fn face_charset(pattern: &str) -> Option<Charset> {
        run(Command::new("fc-match").arg("-f").arg("%{charset}").arg(pattern)).map(|t| parse_charset(&t)).filter(|c| !c.0.is_empty())
    }

    /// Every installed font's characters together.
    fn any_font_charset() -> Option<Charset> {
        run(Command::new("fc-list").arg("--format=%{charset}\n")).map(|t| parse_charset(&t)).filter(|c| !c.0.is_empty())
    }

    fn titlebar_setting() -> Option<String> {
        run(Command::new("gsettings").args(["get", "org.gnome.desktop.wm.preferences", "titlebar-font"]))
    }

    /// A Wayland compositor that offers server-side decorations draws the bar itself (KWin).
    fn server_decorations() -> bool {
        use wayland_client::globals::{registry_queue_init, GlobalListContents};
        use wayland_client::protocol::wl_registry;
        struct S;
        impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for S {
            fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &wayland_client::Connection, _: &wayland_client::QueueHandle<Self>) {}
        }
        let Ok(conn) = wayland_client::Connection::connect_to_env() else { return false };
        let Ok((globals, _queue)) = registry_queue_init::<S>(&conn) else { return false };
        globals.contents().with_list(|l| l.iter().any(|g| g.interface == "zxdg_decoration_manager_v1"))
    }

    /// The characters the bar can draw, worked out once.
    fn bar_charset() -> &'static Option<Charset> {
        static BAR: OnceLock<Option<Charset>> = OnceLock::new();
        BAR.get_or_init(|| {
            use super::super::global_keys::{session_kind, SessionKind};
            if session_kind() == SessionKind::Wayland && !server_decorations() {
                let pattern = titlebar_pattern(titlebar_setting().as_deref());
                // sctk-adwaita draws with its own Cantarell when fontconfig gives no face: Latin only.
                Some(face_charset(&pattern).unwrap_or_else(|| parse_charset("20-7e a0-24f")))
            } else {
                any_font_charset()
            }
        })
    }

    /// Unknown (no fontconfig answer at all): the character stays.
    pub fn covered(c: char) -> bool {
        bar_charset().as_ref().is_none_or(|cs| cs.has(c))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_charset_reads_like_fontconfig_writes_it() {
            let cs = parse_charset("20-7e a0-17f 2100-215f 2184\n2189 2180-2183");
            assert!(cs.has('A') && cs.has('\u{2100}') && cs.has('\u{2189}') && cs.has('\u{2181}'));
            assert!(!cs.has('\u{21a3}') && !cs.has('\u{1f}') && !cs.has('\u{2185}'));
            assert_eq!(parse_charset("20-22 23 24-30"), Charset(vec![(0x20, 0x30)]), "touching ranges merge");
        }

        #[test]
        fn the_pattern_is_sctk_adwaitas() {
            assert_eq!(titlebar_pattern(Some("'Adwaita Sans Bold 11'\n")), "Adwaita Sans:Bold");
            assert_eq!(titlebar_pattern(Some("'Noto Serif CJK HK Bold 12'")), "Noto Serif CJK HK:Bold");
            assert_eq!(titlebar_pattern(Some("'Cantarell 12'")), "Cantarell");
            assert_eq!(titlebar_pattern(None), "sans-serif");
            assert_eq!(titlebar_pattern(Some("")), "sans-serif");
        }

        /// The W541's title face: Noto Sans Bold has no U+21A3, though other fonts on a desktop
        /// may (DejaVu Sans), so the face alone decides. Skipped where fontconfig picks another file.
        #[test]
        fn noto_sans_bold_has_no_tmux_arrow() {
            let file = run(Command::new("fc-match").arg("-f").arg("%{file}").arg("Noto Sans:Bold")).unwrap_or_default();
            if !file.ends_with("NotoSans-Bold.ttf") {
                eprintln!("skipped: fontconfig gives {file:?} for Noto Sans:Bold");
                return;
            }
            let cs = face_charset("Noto Sans:Bold").expect("a charset");
            assert!(cs.has('A') && cs.has('\u{2100}'));
            assert!(!cs.has('\u{21a3}'), "the face that draws the bar lacks the arrow");
        }
    }
}

pub fn hwnd_of(_window: &Window) -> Option<isize> {
    None
}

/// The NSView on macOS. On Linux a per-window token: the X window id on X11, the winit window id
/// on Wayland (which has no number a client can read). Menus and the hotkey window key on it.
pub fn view_of(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[cfg(not(target_os = "macos"))]
    {
        use super::global_keys::{note_session, SessionKind};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) => note_session(SessionKind::X11),
            RawWindowHandle::Wayland(_) => note_session(SessionKind::Wayland),
            _ => {}
        }
    }
    match window.window_handle().ok()?.as_raw() {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Wayland(_) => Some(u64::from(window.id()) as isize),
        _ => None,
    }
}

/// Brings `window` front with the activation token a notification click carried (Wayland);
/// false when there is none to use or the window is not a Wayland one, and the caller raises it
/// the usual way. macOS has no tokens.
pub fn activate(window: &Window, token: Option<&str>) -> Result<bool, String> {
    #[cfg(not(target_os = "macos"))]
    if let Some(t) = token {
        return super::super::linux::activation::activate(window, t);
    }
    let _ = (window, token);
    Ok(false)
}

/// Lets go of the Wayland activation binding (macOS keeps nothing).
pub fn forget_activation() {
    #[cfg(not(target_os = "macos"))]
    super::super::linux::activation::forget();
}

/// winit has no global pointer query; callers with a window use its cursor events instead.
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

pub fn root_at(_x: i32, _y: i32) -> Option<isize> {
    None
}

pub fn screen_to_client(_hwnd: isize, _x: i32, _y: i32) -> Option<(i32, i32)> {
    None
}

/// macOS activates the app of a negative handle (another app) or makes our window key; X11 asks
/// the window manager to activate the window (the hotkey window hands focus back this way).
pub fn set_foreground(_hwnd: isize) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::hotkey::bring_front(_hwnd);
    #[cfg(not(target_os = "macos"))]
    crate::platform::linux::hotkey::activate(_hwnd)
}

pub fn is_foreground(_hwnd: isize) -> bool {
    false
}

pub fn is_minimized(_hwnd: isize) -> bool {
    false
}

pub fn is_maximized(_hwnd: isize) -> bool {
    false
}

pub fn show(_hwnd: isize, _how: Show) {}

pub fn tool_style(_hwnd: isize) -> bool {
    false
}

pub fn set_tool_style(_hwnd: isize, _want: bool) {}

pub fn set_overlay_style(_hwnd: isize) {}

/// The work area and whole area of the window's screen in pixels, top left origin (macOS only).
pub fn monitor_rects(_view: isize) -> Option<(Rect, Rect)> {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::hotkey::monitor_rects(_view);
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn placement(_hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    None
}

pub fn flash(_hwnd: isize, _how: Flash) {}

pub fn is_arranged(_hwnd: isize) -> bool {
    false
}
