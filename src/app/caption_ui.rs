//! The caption area of Minimal and Compact windows (13e): the window keeps winit's decorations, so
//! Windows draws the frame, the shadow and the resize edges, and a subclass takes the title bar
//! away (`WM_NCCALCSIZE`) and answers what the strip under the pointer is (`WM_NCHITTEST`, with
//! `HTMAXBUTTON` so Windows 11's snap layouts flyout opens). The subclass runs inside winit's
//! window procedure, so it never touches `App`: each frame publishes the window's strip to a
//! thread-local map it reads, and button hovers, presses and releases go back as user events.

use super::*;
use crate::caption::{self, Button, Event, Hit, Layout};
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, IsIconic, IsZoomed, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE,
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
    /// The button the pointer is over (the app is told when it changes).
    hover: Option<Button>,
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

/// Measures the frame again (the title bar comes or goes) and keeps the client area the size it
/// was, so the sessions do not gain or lose the title bar's rows: the window's outer height moves
/// instead. A maximized or minimized window keeps its size.
fn frame_changed(hwnd: isize) {
    let before = sizes(hwnd);
    // SAFETY: the HWND is a live window of this thread; nothing moves, only the frame is measured again.
    unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
    let (Some((client, (w, h))), Some((now, _))) = (before, sizes(hwnd)) else { return };
    // SAFETY: state queries of a live window.
    let fixed = unsafe { IsZoomed(hwnd as HWND) != 0 || IsIconic(hwnd as HWND) != 0 };
    if fixed || now == client {
        return;
    }
    // SAFETY: as above; only the window's height changes.
    unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 0, 0, w, h - (now - client), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
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

/// Takes the title bar away from the window and answers its strip: subclass, then measure the
/// frame again.
pub(super) fn install(hwnd: isize, wake: EventLoopProxy<UserEvent>) {
    WAKE.with(|w| *w.borrow_mut() = Some(wake));
    STATES.with(|s| s.borrow_mut().insert(hwnd, State::default()));
    // SAFETY: the HWND is this window's; the proc only answers frame messages.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID, 0) };
    frame_changed(hwnd);
}

/// The window leaves Compact: the subclass goes and the frame is measured again.
pub(super) fn remove(hwnd: isize) {
    STATES.with(|s| s.borrow_mut().remove(&hwnd));
    // SAFETY: removing this proc from the window it was set on.
    unsafe { RemoveWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID) };
    frame_changed(hwnd);
}

/// The strip for the window's next hit tests: its layout in window pixels, and the rects of what
/// stays client in it (None layout: the window has no strip now).
pub(super) fn publish(hwnd: isize, layout: Option<Layout>, over: Vec<Rect>) {
    STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&hwnd) {
            st.layout = layout;
            st.over = over;
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
    let changed = STATES.with(|s| s.borrow_mut().get_mut(&(hwnd as isize)).is_some_and(|st| std::mem::replace(&mut st.hover, now) != now));
    if changed {
        tell(hwnd as isize, Event::Hover(now));
    }
    if now.is_some() {
        let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
        // SAFETY: the struct is fully set and lives for the call.
        unsafe { TrackMouseEvent(&mut track) };
    }
}

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
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
            let mut p = POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
            if unsafe { ScreenToClient(hwnd, &mut p) } == 0 {
                return answer;
            }
            let hit = STATES.with(|s| {
                let s = s.borrow();
                let st = s.get(&(hwnd as isize))?;
                let l = st.layout.as_ref()?;
                let (x, y) = (p.x as f32, p.y as f32);
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
        // handling (which would draw its buttons) is not run.
        WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK if button_of_code(wparam as u32).is_some() => {
            let b = button_of_code(wparam as u32).unwrap();
            match msg {
                WM_NCLBUTTONDOWN => tell(hwnd as isize, Event::Down(b)),
                WM_NCLBUTTONUP => tell(hwnd as isize, Event::Up(b)),
                _ => {}
            }
            0
        }
        WM_NCLBUTTONUP => {
            // Let go elsewhere: a held button is let go.
            tell(hwnd as isize, Event::Hover(None));
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
    /// in a window that is not Minimal or Compact. The window name (Task 8) is not laid out yet.
    pub(super) fn caption_layout(&self) -> Option<Layout> {
        let bar_h = self.caption_strip()?;
        let g = self.gpu.as_ref()?;
        let scale = self.scale();
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        let number = |key: &str, default: f32| self.globals.number(key).map_or(default, |v| v as f32);
        // The two Windows keys apply while a top bar shows (their help says so).
        let (setting, extra) = if top_bar { (number("CompactTabBarStoplightButtonsWidth", 75.0), number("ExtraSpaceBeforeCompactTopTabBar", 0.0)) } else { (75.0, 0.0) };
        let resize_top = self.hwnd().map_or((8.0 * scale).round(), frame_thickness);
        let maximized = g.window.is_maximized();
        let mut l = caption::layout(g.config.width as f32, bar_h, scale, setting, extra, 0.0, self.caption_label_w, if maximized { 0.0 } else { resize_top }, maximized);
        // The edge strip drags the window (Task 7, F10): filled in here so `layout` keeps its arguments.
        l.drag_edge = self.edge_strip_px();
        if let Some(bar) = self.tab_bar_rect() {
            let position = self.tab_position();
            let strip = l.drag_edge.min(bar.h);
            if strip > 0.0 {
                let y = if position == TabPosition::Bottom { bar.y + bar.h - strip } else { bar.y };
                if matches!(position, TabPosition::Top | TabPosition::Bottom) {
                    l.drag_regions.push(Rect { x: bar.x, y, w: bar.w, h: strip });
                }
            }
        }
        Some(l)
    }

    /// The width in pixels the window switch label's slot takes in the caption: its text in the
    /// bar's font plus 8 points of padding, 0 without a label. Measured here (layout cannot,
    /// measuring needs the renderer) and kept in `caption_label_w`.
    pub(super) fn sync_caption_label(&mut self) {
        let label = self.caption_strip().and_then(|_| self.compact_caption().1);
        let scale = self.scale();
        let pt = crate::tab_theme::font_pt(self.resolved_style());
        let w = match (label, self.gpu.as_mut()) {
            (Some(text), Some(g)) => {
                let row_scale = pt * scale / g.renderer.font_px().max(1.0);
                (g.renderer.prose_width(&text, false, row_scale) + 8.0 * scale).ceil()
            }
            _ => 0.0,
        };
        self.caption_label_w = w;
    }

    /// The rects in the strip that stay client for hit tests: each shown tab, group chip and the
    /// bar's "+" and overflow buttons, window pixels.
    fn caption_client_rects(&self) -> Vec<Rect> {
        let Some(bar) = self.tab_bar_rect().filter(|_| self.tab_position() == TabPosition::Top) else { return Vec::new() };
        let mut rects: Vec<Rect> = self.bar_tab_rects().into_iter().filter(|r| r.w > 0.0).collect();
        rects.extend(self.bar_pills(None).into_iter().map(|p| p.rect));
        let scale = self.scale();
        rects.extend(tab_layout::add_rect_px(&self.tab_bar, self.bar_offset(), (bar.x, bar.y), scale));
        rects.extend(self.tab_bar.overflow.map(|r| {
            let px = tab_layout::to_px(r, scale);
            Rect { x: px.x + bar.x, y: px.y + bar.y, ..px }
        }));
        rects
    }

    /// Hands the current window's strip to its subclass after a frame.
    pub(super) fn publish_caption(&self) {
        if let Some(hwnd) = self.hwnd() {
            let layout = self.caption_layout();
            let over = if layout.is_some() { self.caption_client_rects() } else { Vec::new() };
            publish(hwnd, layout, over);
        }
    }

    /// The current window has the caption subclass exactly while it is a Minimal or Compact window.
    /// Every path that can change that ends in `apply_border`, which calls this: a new window,
    /// its style, full screen and the Theme.
    pub(super) fn refresh_caption_frame(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let want = self.compact_window();
        if want == installed(hwnd) {
            return;
        }
        if want {
            let proxy = self.proxy.lock().unwrap().clone();
            install(hwnd, proxy);
        } else {
            remove(hwnd);
        }
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
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        if !top_bar && !title.is_empty() {
            let row_scale = pal.font_pt * scale / font_px;
            let width = self.gpu.as_mut().map_or(0.0, |g| g.renderer.prose_width(&title, false, row_scale));
            scene::caption_title(frame, &title, l.tabs, width, &pal, scale, font_scale, cell.1);
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
}
