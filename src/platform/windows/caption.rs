//! The caption area of Minimal and Compact windows (13e): the window keeps winit's decorations, so
//! Windows draws the frame, the shadow and the resize edges, and a subclass takes the title bar
//! away (`WM_NCCALCSIZE`) and answers what the strip under the pointer is (`WM_NCHITTEST`, with
//! `HTMAXBUTTON` so Windows 11's snap layouts flyout opens). The subclass runs inside winit's
//! window procedure, so it never touches `App`: each frame publishes the window's strip to a
//! thread-local map it reads, and button hovers, presses and releases go back through `wake`.

use crate::caption::{self, Button, Event, Hit, HoverTrack, KeyMenu, KeyMsg, Layout, NcButton};
use crate::layout::Rect;
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, VK_CONTROL, VK_SHIFT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetWindowLongPtrW, GetWindowRect, HTCAPTION, WM_DPICHANGED, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_NCMBUTTONDOWN,
    WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WM_NCXBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WS_CAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, IsIconic, IsZoomed, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
    WM_NCMOUSEMOVE,
};

/// Distinct from the sizing (`0x7473_697A`), Theme guard (`0x7468_656D`) and status bar subclasses.
const SUBCLASS_ID: usize = 0x7463_6170;

/// What a Minimal or Compact window's last frame published, and what its subclass tracks.
#[derive(Default)]
struct State {
    layout: Option<Layout>,
    /// The tab bar's tabs and buttons in the strip: the points that stay client.
    over: Vec<Rect>,
    /// The tabs and the bar's buttons (no chips): iTerm2's cells, for a double click.
    cells: Vec<Rect>,
    /// The button the pointer is over (the app is told when it changes).
    hover: HoverTrack,
    /// Alt pressed alone, and F10 (the menu bar is detached, so Windows sends no `SC_KEYMENU`).
    keys: KeyMenu,
    /// The window's DPI, as of its last `WM_DPICHANGED`.
    dpi: u32,
    /// A DPI change winit is about to size the window for: the client before it (pixels), the
    /// old DPI and the new; taken by the `ScaleFactorChanged` that follows (`dpi_size`).
    dpi_from: Option<((u32, u32), u32, u32)>,
}

thread_local! {
    /// Each caption window's state, read by `subclass`; a window is in the map while it has one.
    static STATES: RefCell<HashMap<isize, State>> = RefCell::new(HashMap::new());
    /// The way back to the app from `subclass`, which never touches `App`.
    static WAKE: RefCell<Option<Box<dyn Fn(isize, Event)>>> = const { RefCell::new(None) };
}

fn tell(hwnd: isize, e: Event) {
    WAKE.with(|w| {
        if let Some(wake) = w.borrow().as_ref() {
            wake(hwnd, e);
        }
    });
}

/// Whether the window has the caption subclass.
pub fn installed(hwnd: isize) -> bool {
    STATES.with(|s| s.borrow().contains_key(&hwnd))
}

/// The window's client height and its whole size, in pixels.
fn sizes(hwnd: isize) -> Option<(i32, (i32, i32))> {
    let (mut client, mut outer) = (RECT { left: 0, top: 0, right: 0, bottom: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 });
    // SAFETY: the HWND is a live window and both RECTs are ours.
    let got = unsafe { GetClientRect(hwnd as HWND, &mut client) != 0 && GetWindowRect(hwnd as HWND, &mut outer) != 0 };
    got.then_some((client.bottom - client.top, (outer.right - outer.left, outer.bottom - outer.top)))
}

/// Runs `change` (which alters the frame: the subclass, the menu bar), measures the frame again,
/// and keeps the client area the size it was, in either direction, so the sessions never gain or
/// lose the caption's or the menu bar's rows: the window's outer height moves instead. A maximized
/// or minimized window keeps its size.
pub fn keep_client(hwnd: isize, change: impl FnOnce()) {
    let before = sizes(hwnd);
    change();
    // SAFETY: the HWND is a live window of this thread; nothing moves, only the frame is measured again.
    unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
    let (Some((client, (w, h))), Some((now, _))) = (before, sizes(hwnd)) else { return };
    // SAFETY: state queries of a live window.
    let fixed = unsafe { IsZoomed(hwnd as HWND) != 0 || IsIconic(hwnd as HWND) != 0 };
    let target = caption::kept_outer_height(h, client, now);
    if fixed || target == h {
        return;
    }
    // SAFETY: as above; only the window's height changes.
    unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 0, 0, w, target, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
}

/// The height in pixels the caption subclass takes off the window: what `AdjustWindowRectExForDpi`
/// (which winit sizes the outer frame with) adds for the title bar and top border. 0 without the
/// subclass or while the window has no title bar (full screen).
fn removed_by_subclass(hwnd: isize) -> i32 {
    // SAFETY: plain queries of a live window and a RECT of ours.
    unsafe {
        let style = GetWindowLongPtrW(hwnd as HWND, GWL_STYLE) as u32;
        if !installed(hwnd) || style & WS_CAPTION != WS_CAPTION {
            return 0;
        }
        let ex = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE) as u32;
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if AdjustWindowRectExForDpi(&mut r, style, 0, ex, GetDpiForWindow(hwnd as HWND)) == 0 {
            return 0;
        }
        -r.top
    }
}

/// `Window::request_inner_size` for a terminal window: winit sizes the outer frame for a style that
/// still has its title bar and the caption subclass removes it, so the height asked for is
/// reduced by that (`caption::corrected_inner`). Every size request of a terminal window goes here.
pub fn request_inner_size(window: &winit::window::Window, size: impl Into<winit::dpi::Size>) -> Option<winit::dpi::PhysicalSize<u32>> {
    let size: winit::dpi::PhysicalSize<u32> = size.into().to_physical(window.scale_factor());
    let removed = crate::platform::window::hwnd_of(window).map_or(0, removed_by_subclass);
    window.request_inner_size(winit::dpi::PhysicalSize::new(size.width, caption::corrected_inner(size.height, removed)))
}

/// The client size for `ScaleFactorChanged`'s `inner_size_writer`: winit sends the event from
/// inside `WM_DPICHANGED` before it sizes the window (`event_loop.rs:2283-2291`, then `:2295-2400`)
/// for a style that still has its title bar, so the size it would set, the old client scaled, is
/// handed back less the title bar the subclass removes (`caption::dpi_inner`). The old client was
/// noted by the subclass before winit ran, so a buffered event (dispatched after winit's resize)
/// gets the same answer. None leaves winit's size: no subclass, no title bar (full screen), or a
/// maximized or minimized window. winit keeps a maximized window's client and frames it for the
/// title bar, which leaves it too tall; changing the size here would un-maximize it (winit clears
/// `MAXIMIZED` for a new size), so the subclass fits it to its monitor after (`fit_maximized`).
pub fn dpi_size(hwnd: isize) -> Option<winit::dpi::PhysicalSize<u32>> {
    let (old, from, to) = STATES.with(|s| s.borrow_mut().get_mut(&hwnd)?.dpi_from.take())?;
    let removed = removed_by_subclass(hwnd);
    if removed == 0 {
        return None;
    }
    let (w, h) = caption::dpi_inner(old, from, to, removed);
    Some(winit::dpi::PhysicalSize::new(w, h))
}

/// The frame's thickness along the top in pixels at the window's DPI (`SM_CYSIZEFRAME` plus
/// `SM_CXPADDEDBORDER`): the resize border a maximized window overhangs the monitor by.
pub fn frame_thickness(hwnd: isize) -> f32 {
    // SAFETY: plain metric queries.
    unsafe {
        let dpi = GetDpiForWindow(hwnd as HWND);
        (GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)) as f32
    }
}

/// A maximized caption window after winit's `WM_DPICHANGED`: winit kept its client and framed it
/// for a style with a title bar, one title bar (less the frame's overhang) taller than the
/// monitor, so it is set back to the work area overhung by the frame (`caption::maximized_outer`).
/// It stays maximized: only its rect moves.
fn fit_maximized(hwnd: HWND) {
    // SAFETY: state and monitor queries of this live window, a MONITORINFO of ours, and a move of
    // the window's own rect.
    unsafe {
        if IsZoomed(hwnd) == 0 || removed_by_subclass(hwnd as isize) == 0 {
            return;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) == 0 {
            return;
        }
        let w = info.rcWork;
        let (l, t, r, b) = caption::maximized_outer((w.left, w.top, w.right, w.bottom), frame_thickness(hwnd as isize).round() as i32);
        SetWindowPos(hwnd, std::ptr::null_mut(), l, t, r - l, b - t, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Takes the title bar away from the window: subclass it. The caller keeps the client size
/// (`keep_client`) around this and the menu bar's change.
pub fn install(hwnd: isize, wake: Box<dyn Fn(isize, Event)>) {
    WAKE.with(|w| *w.borrow_mut() = Some(wake));
    // SAFETY: a plain query of this window.
    let dpi = unsafe { GetDpiForWindow(hwnd as HWND) };
    STATES.with(|s| s.borrow_mut().insert(hwnd, State { dpi, ..State::default() }));
    // SAFETY: the HWND is this window's; the proc only answers frame messages.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID, 0) };
}

/// The window leaves Compact: the subclass goes.
pub fn remove(hwnd: isize) {
    STATES.with(|s| s.borrow_mut().remove(&hwnd));
    // SAFETY: removing this proc from the window it was set on.
    unsafe { RemoveWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID) };
}

/// The strip for the window's next hit tests: its layout in window pixels, the rects of what
/// stays client in it, and of its tabs among them (None layout: the window has no strip now).
pub fn publish(hwnd: isize, layout: Option<Layout>, over: Vec<Rect>, cells: Vec<Rect>) {
    STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&hwnd) {
            st.layout = layout;
            st.over = over;
            st.cells = cells;
        }
    });
}

fn hit_code(h: Hit) -> LRESULT {
    (match h {
        Hit::Client => HTCLIENT,
        Hit::Caption => HTCAPTION,
        Hit::Top => HTTOP,
        Hit::Min => HTMINBUTTON,
        Hit::Max => HTMAXBUTTON,
        Hit::Close => HTCLOSE,
    }) as LRESULT
}

fn button_of_code(code: u32) -> Option<Button> {
    match code {
        HTMINBUTTON => Some(Button::Minimize),
        HTMAXBUTTON => Some(Button::Maximize),
        HTCLOSE => Some(Button::Close),
        _ => None,
    }
}

/// The hover moved: the app is told once per change, and Windows tells us when the pointer leaves.
fn hover_to(hwnd: HWND, now: Option<Button>) {
    if let Some(e) = STATES.with(|s| s.borrow_mut().get_mut(&(hwnd as isize)).and_then(|st| st.hover.to(now))) {
        tell(hwnd as isize, e);
    }
    if now.is_some() {
        let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
        // SAFETY: the struct is fully set and lives for the call.
        unsafe { TrackMouseEvent(&mut track) };
    }
}

/// The key menu's view of a window message: keys, mouse presses (a click between Alt's press and
/// release, as in an Alt+drag, cancels it) and the focus leaving.
fn key_msg(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<KeyMsg> {
    let vk = wparam as u16;
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyMsg::Down { vk, repeat: lparam & (1 << 30) != 0, sys: msg == WM_SYSKEYDOWN }),
        WM_KEYUP | WM_SYSKEYUP => Some(KeyMsg::Up { vk, sys: msg == WM_SYSKEYUP }),
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN | WM_NCXBUTTONDOWN => Some(KeyMsg::Pointer),
        WM_KILLFOCUS => Some(KeyMsg::FocusLost),
        _ => None,
    }
}

/// The client point of a non-client mouse message (lparam holds screen coordinates).
fn client_point(hwnd: HWND, lparam: LPARAM) -> Option<(i32, i32)> {
    let mut p = POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
    // SAFETY: the HWND is live and the POINT is ours.
    (unsafe { ScreenToClient(hwnd, &mut p) } != 0).then_some((p.x, p.y))
}

/// What a double click at the screen point in lparam does (`caption::strip_double_click`); the
/// title bar's own without a strip.
fn strip_double_click(hwnd: HWND, lparam: LPARAM) -> caption::StripDoubleClick {
    let title_bar = caption::StripDoubleClick::TitleBar;
    let Some((x, y)) = client_point(hwnd, lparam) else { return title_bar };
    let (x, y) = (x as f32, y as f32);
    STATES.with(|s| {
        let s = s.borrow();
        let Some(st) = s.get(&(hwnd as isize)) else { return title_bar };
        st.layout.as_ref().map_or(title_bar, |l| caption::strip_double_click(l, x, y, st.cells.iter().any(|r| r.contains(x, y))))
    })
}

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // Alt alone and F10 open the caption's menu: seen here before winit, which never hands them to
    // Windows' own menu handling while muda has the menu bar detached. The message goes on to
    // winit either way (the terminal still sees F10, the app Alt's release).
    if let Some(m) = key_msg(msg, wparam, lparam) {
        // SAFETY: plain keyboard state queries on the window's thread.
        let shift_or_ctrl = unsafe { GetKeyState(VK_SHIFT as i32) < 0 || GetKeyState(VK_CONTROL as i32) < 0 };
        let open = STATES.with(|s| s.borrow_mut().get_mut(&(hwnd as isize)).is_some_and(|st| st.keys.feed(m, shift_or_ctrl) && st.layout.as_ref().is_some_and(|l| l.menu.is_some())));
        if open {
            tell(hwnd as isize, Event::Menu(false));
        }
    }
    match msg {
        // The DPI changes: the client before winit sizes the window, for `dpi_size`.
        WM_DPICHANGED => {
            let to = (wparam & 0xFFFF) as u32;
            let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            // SAFETY: state queries of this live window and a RECT of ours.
            let sized = unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 && GetClientRect(hwnd, &mut client) != 0 };
            STATES.with(|s| {
                if let Some(st) = s.borrow_mut().get_mut(&(hwnd as isize)) {
                    let from = std::mem::replace(&mut st.dpi, to);
                    let old = ((client.right - client.left).max(0) as u32, (client.bottom - client.top).max(0) as u32);
                    st.dpi_from = (sized && from != to).then_some((old, from, to));
                }
            });
            let answer = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            fit_maximized(hwnd);
            answer
        }
        // Without a title bar (full screen) there is nothing to take away.
        WM_NCCALCSIZE if unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CAPTION != WS_CAPTION => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
        WM_NCCALCSIZE => {
            // The top of the window as proposed, before the default takes the title bar off it.
            let top = if wparam != 0 {
                // SAFETY: with a true wparam the system passes an NCCALCSIZE_PARAMS in lparam.
                unsafe { (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0].top }
            } else {
                // SAFETY: with a false wparam lparam is the window's RECT.
                unsafe { (*(lparam as *mut RECT)).top }
            };
            let answer = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            // No caption: the client starts at the window's top (the side and bottom frame stay). A
            // maximized window overhangs the monitor by its frame, which is added back.
            let overhang = if unsafe { IsZoomed(hwnd) } != 0 { frame_thickness(hwnd as isize).round() as i32 } else { 0 };
            if wparam != 0 {
                unsafe { (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0].top = top + overhang };
            } else {
                unsafe { (*(lparam as *mut RECT)).top = top + overhang };
            }
            answer
        }
        WM_NCHITTEST => {
            // The side and bottom edges are the system's; only a client answer is ours.
            let answer = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if answer != HTCLIENT as LRESULT {
                return answer;
            }
            let Some((px, py)) = client_point(hwnd, lparam) else { return answer };
            let hit = STATES.with(|s| {
                let s = s.borrow();
                let st = s.get(&(hwnd as isize))?;
                let l = st.layout.as_ref()?;
                let (x, y) = (px as f32, py as f32);
                Some(caption::hit(l, x, y, st.over.iter().any(|r| r.contains(x, y))))
            });
            hit.map_or(answer, hit_code)
        }
        WM_NCMOUSEMOVE => {
            hover_to(hwnd, button_of_code(wparam as u32));
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCMOUSELEAVE => {
            hover_to(hwnd, None);
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        // The buttons are ours: the press and release are told to the app, and Windows' own
        // handling (which would draw its buttons) is not run. A double click's second press is a
        // press (`caption::button_event`).
        WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK if button_of_code(wparam as u32).is_some() => {
            let b = button_of_code(wparam as u32).unwrap();
            let m = match msg {
                WM_NCLBUTTONDOWN => NcButton::Down,
                WM_NCLBUTTONUP => NcButton::Up,
                _ => NcButton::DoubleClick,
            };
            tell(hwnd as isize, caption::button_event(m, b));
            0
        }
        // Windows' own keyboard menu (Alt or F10 alone; with no menu bar, the system menu's
        // invisible mode that eats the next key) never starts: the key menu above opened ours.
        // Alt+Space carries its character and stays the system menu.
        WM_SYSCOMMAND if caption::is_key_menu(wparam, lparam) && STATES.with(|s| s.borrow().get(&(hwnd as isize)).is_some_and(|st| st.layout.as_ref().is_some_and(|l| l.menu.is_some()))) => 0,
        // A right click where Windows answers caption, not its system menu: the app gives the menu
        // of the tab or chip under it (the edge strip), else the whole menu (`PSM:3961-3985`).
        WM_NCRBUTTONUP if wparam as u32 == HTCAPTION => {
            if let Some((x, y)) = client_point(hwnd, lparam) {
                tell(hwnd as isize, Event::RightClick(x, y));
            }
            0
        }
        // A double click on the window name edits the window title, over a tab (the edge strip)
        // it does nothing, elsewhere on the strip it stays the title bar's
        // (`caption::strip_double_click`).
        WM_NCLBUTTONDBLCLK if wparam as u32 == HTCAPTION => match strip_double_click(hwnd, lparam) {
            caption::StripDoubleClick::EditName => {
                tell(hwnd as isize, Event::EditName);
                0
            }
            caption::StripDoubleClick::Nothing => 0,
            caption::StripDoubleClick::TitleBar => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
        },
        WM_NCLBUTTONUP => {
            // Let go elsewhere: no hover, and a held button is let go.
            if let Some(e) = STATES.with(|s| s.borrow_mut().get_mut(&(hwnd as isize)).map(|st| st.hover.release_elsewhere())) {
                tell(hwnd as isize, e);
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            STATES.with(|s| s.borrow_mut().remove(&(hwnd as isize)));
            unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

