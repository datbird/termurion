//! The caption area of Minimal and Compact windows (13e): the window keeps winit's decorations, so
//! Windows draws the frame, the shadow and the resize edges, and a subclass takes the title bar
//! away (`WM_NCCALCSIZE`) and answers what the strip under the pointer is (`WM_NCHITTEST`, with
//! `HTMAXBUTTON` so Windows 11's snap layouts flyout opens). The subclass runs inside winit's
//! window procedure, so it never touches `App`: each frame publishes the window's strip to a
//! thread-local map it reads, and button hovers, presses and releases go back as user events.

use super::*;
use crate::caption::{self, Button, Event, Hit, HoverTrack, KeyMenu, KeyMsg, Layout, NcButton, StripMenu};
use std::cell::RefCell;
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
    static WAKE: RefCell<Option<EventLoopProxy<UserEvent>>> = const { RefCell::new(None) };
}

fn tell(hwnd: isize, e: Event) {
    WAKE.with(|w| {
        if let Some(proxy) = w.borrow().as_ref() {
            let _ = proxy.send_event(UserEvent::Caption(hwnd, e));
        }
    });
}

/// Whether the window has the caption subclass.
fn installed(hwnd: isize) -> bool {
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
pub(super) fn keep_client(hwnd: isize, change: impl FnOnce()) {
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
pub(super) fn request_inner_size(window: &winit::window::Window, size: impl Into<winit::dpi::Size>) -> Option<winit::dpi::PhysicalSize<u32>> {
    let size: winit::dpi::PhysicalSize<u32> = size.into().to_physical(window.scale_factor());
    let removed = super::menu::hwnd_of(window).map_or(0, removed_by_subclass);
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
pub(super) fn dpi_size(hwnd: isize) -> Option<winit::dpi::PhysicalSize<u32>> {
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
fn frame_thickness(hwnd: isize) -> f32 {
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
pub(super) fn install(hwnd: isize, wake: EventLoopProxy<UserEvent>) {
    WAKE.with(|w| *w.borrow_mut() = Some(wake));
    // SAFETY: a plain query of this window.
    let dpi = unsafe { GetDpiForWindow(hwnd as HWND) };
    STATES.with(|s| s.borrow_mut().insert(hwnd, State { dpi, ..State::default() }));
    // SAFETY: the HWND is this window's; the proc only answers frame messages.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID, 0) };
}

/// The window leaves Compact: the subclass goes.
pub(super) fn remove(hwnd: isize) {
    STATES.with(|s| s.borrow_mut().remove(&hwnd));
    // SAFETY: removing this proc from the window it was set on.
    unsafe { RemoveWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID) };
}

/// The strip for the window's next hit tests: its layout in window pixels, the rects of what
/// stays client in it, and of its tabs among them (None layout: the window has no strip now).
pub(super) fn publish(hwnd: isize, layout: Option<Layout>, over: Vec<Rect>, cells: Vec<Rect>) {
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

impl App {
    /// The current window's caption strip as `caption::layout` places it, in window pixels; None
    /// in a window that is not Minimal or Compact. The window name is laid out when it shows and the tabs leave room.
    pub(super) fn caption_layout(&self) -> Option<Layout> {
        let bar_h = self.caption_strip()?;
        let g = self.gpu.as_ref()?;
        let scale = self.scale();
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        let number = |key: &str, default: f32| self.globals.number(key).map_or(default, |v| v as f32);
        // The two Windows keys apply while a top bar shows (their help says so).
        let (setting, extra) = if top_bar { (number("CompactTabBarStoplightButtonsWidth", 75.0), number("ExtraSpaceBeforeCompactTopTabBar", 0.0)) } else { (75.0, 0.0) };
        let thickness = self.hwnd().map_or((8.0 * scale).round(), frame_thickness);
        let maximized = g.window.is_maximized();
        let resize_top = caption::resize_top(thickness, maximized, g.window.is_resizable());
        let width = g.config.width as f32;
        let mut l = caption::layout(width, bar_h, scale, setting, extra, 0.0, self.caption_label_w, resize_top, maximized);
        // The window name beside the tabs takes the room the tabs leave (`RTV:590-620`).
        if self.caption_name_w > 0.0 {
            let prefs = self.tab_settings().prefs;
            let scrollable = prefs.scrollable.then_some(prefs.scrollable_width * scale);
            let allowance = caption::name_allowance(l.tabs.w, self.tabs.len(), prefs.min_width * scale, scrollable, scale) / scale;
            let name_w = caption::name_width(self.caption_name_w / scale, allowance) * scale;
            if name_w > 0.0 {
                l = caption::layout(width, bar_h, scale, setting, extra, name_w, self.caption_label_w, resize_top, maximized);
            }
        }
        // The native menu bar cannot sit under the strip: its button opens the menu instead.
        l = caption::with_menu_button(l, scale);
        // The edge strip drags the window (Task 7, F10): filled in here so `layout` keeps its arguments.
        // The bar's rect comes from `window_layout` and this strip, never `tab_bar_rect` (which asks
        // for this layout).
        l.drag_edge = self.edge_strip_px();
        if let Some(bar) = self.window_layout().0 {
            let position = self.tab_position();
            let bar = if position == TabPosition::Top { Rect { x: l.tabs.x, w: l.tabs.w, ..bar } } else { bar };
            let strip = l.drag_edge.min(bar.h);
            if strip > 0.0 && matches!(position, TabPosition::Top | TabPosition::Bottom) {
                let y = if position == TabPosition::Bottom { bar.y + bar.h - strip } else { bar.y };
                // The "+" and the overflow button keep their presses (`caption::edge_drag_regions`).
                l.drag_regions = caption::edge_drag_regions(Rect { x: bar.x, y, w: bar.w, h: strip }, &self.bar_button_rects(bar));
            }
        }
        Some(l)
    }

    /// The width in pixels the window switch label's slot takes in the caption: its text in the
    /// bar's font plus 8 points of padding, 0 without a label. Measured here (layout cannot,
    /// measuring needs the renderer) and kept in `caption_label_w`.
    pub(super) fn sync_caption_label(&mut self) {
        let label = self.caption_strip().and_then(|_| self.compact_caption().1);
        let name = self.caption_strip().and_then(|_| self.window_name());
        let scale = self.scale();
        let pt = crate::tab_theme::font_pt(self.resolved_style());
        let (w, name_w) = match self.gpu.as_mut() {
            Some(g) => {
                let row_scale = pt * scale / g.renderer.font_px().max(1.0);
                let w = label.map_or(0.0, |text| (g.renderer.prose_width(&text, false, row_scale) + 8.0 * scale).ceil());
                (w, name.map_or(0.0, |text| g.renderer.prose_width(&text, false, row_scale).ceil()))
            }
            None => (0.0, 0.0),
        };
        self.caption_label_w = w;
        self.caption_name_w = name_w;
    }

    /// The window name that shows beside the tabs (`ShowWindowNameBesideTabs`), None when the
    /// setting, the window or the bar rules it out. The custom name is Edit Window Title's format
    /// evaluated, and only while that format is set (`PT:13636-13639`); the automatic name is the
    /// window's title without its number.
    pub(super) fn window_name(&self) -> Option<String> {
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        let mode = caption::name_mode(self.globals.int("ShowWindowNameBesideTabs"));
        let focused = self.tabs.get(self.active).map(|t| t.focused);
        let custom = self.win_title.as_deref().filter(|f| !f.is_empty()).map(|f| self.window_title_text(f, focused));
        let automatic = if mode == caption::NameMode::Always && self.compact_window() { self.caption_base() } else { String::new() };
        caption::name_beside_tabs(mode, self.compact_window(), self.full_screen(), top_bar, custom.as_deref(), &automatic)
    }

    /// A horizontal bar's own buttons, the "+" and the overflow button, in window pixels for the
    /// bar at `bar`. Its scroll is `tab_scroll` (`bar_offset` would ask for the caption's layout,
    /// which calls this).
    fn bar_button_rects(&self, bar: Rect) -> Vec<Rect> {
        let scale = self.scale();
        let add = tab_layout::add_rect_px(&self.tab_bar, self.tab_scroll, (bar.x, bar.y), scale);
        let overflow = self.tab_bar.overflow.map(|r| {
            let px = tab_layout::to_px(r, scale);
            Rect { x: px.x + bar.x, y: px.y + bar.y, ..px }
        });
        add.into_iter().chain(overflow).collect()
    }

    /// The rects in the strip that stay client for hit tests: each shown tab, group chip and the
    /// bar's "+" and overflow buttons, window pixels; the first `.1` of them are iTerm2's cells
    /// for a double click, every one but the chips (`caption::strip_items`). Not the window
    /// name: its clicks are the strip's (`caption::strip_part`).
    fn caption_client_rects(&self) -> (Vec<Rect>, usize) {
        let Some(bar) = self.tab_bar_rect().filter(|_| self.tab_position() == TabPosition::Top) else { return (Vec::new(), 0) };
        let tabs = self.bar_tab_rects().into_iter().filter(|r| r.w > 0.0).collect();
        let chips = self.bar_pills(None).into_iter().map(|p| p.rect).collect();
        caption::strip_items(tabs, chips, self.bar_button_rects(bar))
    }

    /// Hands the current window's strip to its subclass after a frame.
    pub(super) fn publish_caption(&self) {
        if let Some(hwnd) = self.hwnd() {
            let layout = self.caption_layout();
            let (over, cells) = if layout.is_some() { self.caption_client_rects() } else { (Vec::new(), 0) };
            let cells = over[..cells].to_vec();
            publish(hwnd, layout, over, cells);
        }
    }

    /// The current window has the caption subclass exactly while it is a Minimal or Compact window.
    /// Every path that can change that ends in `apply_border`, which calls this: a new window,
    /// its style, full screen and the Theme.
    pub(super) fn refresh_caption_frame(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        // Keyed on the themed type alone, not `compact_window()`: in full screen winit's style has no
        // title bar and the subclass leaves the frame alone, so a round trip changes nothing.
        let want = self.compact_type();
        if want == installed(hwnd) {
            return;
        }
        let proxy = self.proxy.lock().unwrap().clone();
        // The menu bar goes or comes back inside the same measure, so the client keeps its size.
        keep_client(hwnd, || {
            if want {
                install(hwnd, proxy);
            } else {
                remove(hwnd);
            }
            self.sync_menu_hidden();
        });
        self.caption_hover = None;
        self.caption_pressed = None;
        if !self.tabs.is_empty() {
            self.relayout();
        }
        self.redraw();
    }

    /// The strip's material under the tab bar (the bar's own rect is drawn over it): the palette's
    /// bar and margin fill across the whole window width, so the room beside the tabs matches.
    pub(super) fn draw_caption_bar(&mut self, frame: &mut Frame) {
        let Some(l) = self.caption_layout() else { return };
        scene::caption_bar(frame, l.bar, &self.tab_palette());
    }

    /// The strip's own drawing over the tab bar: the window switch label, the window's title where
    /// no top bar shows, and the three window buttons.
    pub(super) fn draw_caption_content(&mut self, frame: &mut Frame) {
        let Some(l) = self.caption_layout() else { return };
        let pal = self.tab_palette();
        let scale = self.scale();
        let cell = self.cell();
        let font_px = self.gpu.as_ref().map_or(cell.1, |g| g.renderer.font_px()).max(1.0);
        let font_scale = scale / font_px;
        let (title, label) = self.compact_caption();
        if let (Some(s), Some(text)) = (l.shortcut, label) {
            scene::caption_shortcut(frame, &text, s, s.w - 8.0 * scale, &pal, scale, font_scale, cell.1);
        }
        if let (Some(n), Some(text)) = (l.name, self.window_name()) {
            let row_scale = pal.font_pt * scale / font_px;
            if let Some(g) = self.gpu.as_mut() {
                let mut measure = |t: &str| g.renderer.prose_width(t, false, row_scale);
                scene::caption_name(frame, &text, n, &mut measure, &pal, scale, font_scale, cell.1);
            }
        }
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        if !top_bar && !title.is_empty() {
            let row_scale = pal.font_pt * scale / font_px;
            let width = self.gpu.as_mut().map_or(0.0, |g| g.renderer.prose_width(&title, false, row_scale));
            scene::caption_title(frame, &title, l.tabs, width, &pal, scale, font_scale, cell.1);
        }
        if let Some(m) = l.menu {
            scene::caption_menu_button(frame, m, &pal, scale, font_scale, cell.1);
        }
        scene::caption_buttons(frame, &l.buttons, self.caption_hover, self.caption_pressed, l.maximized, &pal, scale, font_scale, cell.1);
    }

    /// A caption button was hovered, pressed or released (`UserEvent::Caption`): the state it
    /// draws, and a click's action.
    pub(super) fn caption_event(&mut self, hwnd: isize, e: Event, event_loop: &ActiveEventLoop) {
        let id = WindowId::from(hwnd as u64);
        if !self.enter(id) {
            return;
        }
        match e {
            Event::Menu(at_cursor) => return self.show_caption_menu(at_cursor),
            Event::RightClick(x, y) => return self.caption_right_click(x as f32, y as f32),
            // After the double click's messages are done, as iTerm2 defers it (`PT:1235-1241`).
            Event::EditName => return self.open_title_prompt(event_loop, false),
            Event::Hover(h) => {
                self.caption_hover = h;
                // A button held while the pointer left it is let go.
                if self.caption_pressed.is_some() && self.caption_pressed != h {
                    self.caption_pressed = None;
                }
            }
            Event::Down(b) => self.caption_pressed = Some(b),
            Event::Up(b) => {
                let click = caption::clicked(self.caption_pressed.take(), b);
                if let (Some(b), Some(g)) = (click, self.gpu.as_ref()) {
                    match b {
                        Button::Minimize => g.window.set_minimized(true),
                        Button::Maximize => g.window.set_maximized(!g.window.is_maximized()),
                        Button::Close => return self.close_window(id, event_loop),
                    }
                }
            }
        }
        self.redraw();
    }

    /// A right click where Windows answers caption, at the window point (`x`, `y`): the menu of
    /// the tab or group chip there, else the whole menu at the pointer (`caption::strip_right_click`).
    fn caption_right_click(&mut self, x: f32, y: f32) {
        match caption::strip_right_click(self.tab_bar_hit(x, y)) {
            StripMenu::Tab(i) => {
                if let Some(tab) = self.tabs.get(i).map(|t| t.id) {
                    self.show_tab_menu(tab);
                }
            }
            StripMenu::Group => match self.bar_chip_at(x, y) {
                Some(group) => self.show_group_menu(&group),
                None => self.show_caption_menu(true),
            },
            StripMenu::Main => self.show_caption_menu(true),
        }
    }

    /// Pops the whole menu: under the caption's menu button (Alt, F10, a click on it) or at the
    /// pointer (a right click on the strip, `PSM:3978-3982`). The window's menu is already
    /// initialized for this window, so its picks reach the usual handler.
    pub(super) fn show_caption_menu(&mut self, at_cursor: bool) {
        use muda::ContextMenu;
        let (Some(hwnd), Some(bar)) = (self.hwnd(), self.menu_bar.as_ref()) else { return };
        let at = if at_cursor { None } else { self.caption_layout().and_then(|l| l.menu).map(|m| muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(m.x as i32, (m.y + m.h) as i32))) };
        // SAFETY: the window's own menu and HWND.
        unsafe { bar.menu.show_context_menu_for_hwnd(hwnd, at) };
    }
}
