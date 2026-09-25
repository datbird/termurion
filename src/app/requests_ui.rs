//! What output asks of the window: focus, attention, frames, the cursor guide, the tab color.

use std::time::{Duration, Instant};

use winit::dpi::{PhysicalPosition, PhysicalSize};

use super::*;
use crate::osc1337::{self, Attention};
use crate::screen::Request;

/// How long RequestAttention's pane flash and fireworks last.
pub(super) const FLASH: Duration = Duration::from_millis(300);
pub(super) const FIREWORKS: Duration = Duration::from_millis(1200);

impl App {
    pub(super) fn window_request(&mut self, pane: PaneId, r: Request) {
        match r {
            Request::Trigger(t) => self.trigger_request(pane, t),
            Request::Bell => self.ring_bell(pane),
            Request::Notify { title, message } => self.terminal_alert(pane, title, message),
            Request::SetBadge(t) => self.set_badge(pane, Some(t)),
            Request::BadgeRefused => self.set_badge(pane, None),
            // Progress Bar Changed event triggers arrive in task 5 of milestone 6b.
            Request::Progress(_) => {}
            Request::CursorGuide(on) => {
                self.show_guide.insert(pane, on);
                self.redraw();
            }
            Request::Attention(a) => self.attention(pane, a),
            Request::Disinter => self.disinter(pane),
            Request::StealFocus => {
                let allow = files_ui::ask("A control sequence attempted to activate a session. Allow it?");
                self.steal_asked = Some(Instant::now());
                if allow {
                    self.reveal(pane);
                }
            }
            Request::ReportCellSize => {
                let scale = self.scale() as f64;
                let (w, h) = self.cell();
                let reply = format!(
                    "\x1b]1337;ReportCellSize={};{};{}\x1b\\",
                    osc1337::number(h as f64 / scale),
                    osc1337::number(w as f64 / scale),
                    osc1337::number(scale)
                );
                self.reply(pane, reply.as_bytes());
            }
            Request::ReportWindowFrame => {
                let Some((x, y, w, h)) = self.window_points() else { return };
                let n = osc1337::number;
                let reply = format!("\x1b]1337;WindowFrame={};{};{};{}\x1b\\", n(x), n(y), n(w), n(h));
                self.reply(pane, reply.as_bytes());
            }
            Request::ReportScreenFrames => {
                let scale = self.scale() as f64;
                let primary = primary_height();
                let n = osc1337::number;
                let frames: Vec<String> = work_areas()
                    .into_iter()
                    .map(|(x, y, w, h)| {
                        let (x, y, w, h) = osc1337::to_points(x, y, w, h, primary, scale);
                        format!("{},{},{},{}", n(x), n(y), n(w), n(h))
                    })
                    .collect();
                let reply = format!("\x1b]1337;ScreenFrames={}\x1b\\", frames.join(";"));
                self.reply(pane, reply.as_bytes());
            }
            Request::SetWindowFrame(x, y, w, h) => self.set_window_frame(pane, x, y, w, h),
            Request::TabColor(c) => {
                match c {
                    Some(c) => self.tab_colors.insert(pane, c),
                    None => self.tab_colors.remove(&pane),
                };
                self.redraw();
            }
            Request::SetProfile(name) => self.set_profile_request(pane, name),
            Request::SetProfileProperty(pairs) => self.set_property_request(pane, pairs),
            Request::SetBackgroundImageFile(value) => self.set_background_request(pane, value),
            Request::OpenUrl(value) => self.open_url_request(pane, value),
            Request::ReportVariable(value) => self.report_variable_request(pane, value),
            Request::RequestUpload(value) => self.upload_request(pane, value),
            Request::ClipboardWrite(_) | Request::ClipboardRead(_) => {}
        }
    }

    fn reply(&mut self, pane: PaneId, bytes: &[u8]) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.reply(bytes);
        }
    }

    fn attention(&mut self, pane: PaneId, a: Attention) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_STOP, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
        let flash = |flags: u32, count: u32, hwnd: isize| {
            let info = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: hwnd as _, dwFlags: flags, uCount: count, dwTimeout: 0 };
            unsafe { FlashWindowEx(&info) };
        };
        match a {
            Attention::Once | Attention::Start | Attention::Stop => {
                let Some(hwnd) = self.hwnd() else { return };
                match a {
                    Attention::Once => flash(FLASHW_TRAY, 1, hwnd),
                    Attention::Start => flash(FLASHW_TRAY | FLASHW_TIMERNOFG, 0, hwnd),
                    _ => flash(FLASHW_STOP, 0, hwnd),
                }
            }
            Attention::Flash => {
                self.flashes.insert(pane, Instant::now());
                self.redraw();
            }
            Attention::Fireworks => {
                let at = self.panes.get(&pane).map(|p| {
                    let g = p.own().term().grid();
                    (g.cursor.point.column.0, (g.cursor.point.line.0 + g.display_offset() as i32).max(0) as usize)
                });
                if let Some(at) = at {
                    self.fireworks.insert(pane, (Instant::now(), at));
                    self.redraw();
                }
            }
        }
    }

    /// StealFocus: the pane's tab comes forward, the pane takes focus, and the window comes
    /// to the front (Windows may only flash its taskbar button instead).
    fn reveal(&mut self, pane: PaneId) {
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) {
            self.active = t;
            self.tabs[t].focused = pane;
        }
        self.bring_forward();
        if let Some(hwnd) = self.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
            if unsafe { SetForegroundWindow(hwnd as _) } == 0 {
                self.attention(pane, Attention::Once);
            }
        }
        self.relayout();
    }

    /// The window's frame in AppKit points (y up from the bottom of the primary monitor).
    fn window_points(&self) -> Option<(f64, f64, f64, f64)> {
        let g = self.gpu.as_ref()?;
        let pos = g.window.outer_position().ok()?;
        let size = g.window.outer_size();
        Some(osc1337::to_points(pos.x as f64, pos.y as f64, size.width as f64, size.height as f64, primary_height(), g.window.scale_factor()))
    }

    fn set_window_frame(&mut self, pane: PaneId, x: f64, y: f64, w: f64, h: f64) {
        let refused = self.profile_of(pane).is_some_and(|p| p.bool("Disable Window Resizing"));
        let Some(g) = &self.gpu else { return };
        if refused || g.window.fullscreen().is_some() {
            return;
        }
        let (x, y, w, h) = osc1337::from_points(x, y, w, h, primary_height(), g.window.scale_factor());
        // The frame includes the title bar and borders; the inner size leaves them out.
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let (dw, dh) = (outer.width.saturating_sub(inner.width) as f64, outer.height.saturating_sub(inner.height) as f64);
        g.window.set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
        let _ = g.window.request_inner_size(PhysicalSize::new((w - dw).max(1.0).round() as u32, (h - dh).max(1.0).round() as u32));
    }

    /// A tab's title, status, and colors for the tab bar.
    pub(super) fn tab_look(&self, t: &Tab) -> scene::TabLook {
        use crate::tabstatus::{self, xterm_color};
        let title = self.tmux_tab_label(t.focused).or_else(|| t.title.clone()).unwrap_or_else(|| self.session_title(t.focused));
        let panes = t.layout.panes();
        let focused = self.panes.get(&t.focused).map(|p| &p.own().tab_status);
        let others: Vec<&tabstatus::Status> = panes.iter().filter(|id| **id != t.focused).filter_map(|id| self.panes.get(id)).map(|p| &p.own().tab_status).collect();
        let st = tabstatus::aggregate(focused, &others, &tabstatus::PRIORITIES);
        let show_text = self.globals.bool("ShowSessionStatusInTabSubtitle");
        let status = st.filter(|_| show_text).and_then(|s| {
            let text = s.status.clone()?;
            let color = s.status_color.as_deref().and_then(xterm_color).unwrap_or(rgb_of(0xc7, 0xc7, 0xc7));
            Some((text, color))
        });
        let dot = st.and_then(|s| s.indicator.as_deref()).and_then(xterm_color);
        let color = self.tab_colors.get(&t.focused).copied();
        let dead = self.panes.get(&t.focused).is_some_and(|p| p.exited.is_some());
        let bell = panes.iter().any(|id| self.bell_icons.contains(id));
        let (new_output, idle) = self.activity_icons(&panes);
        let icon = scene::tab_icon(dot.is_some(), dead, bell, new_output, idle);
        scene::TabLook { title, status, dot, color, icon }
    }

    /// Forgets pane flashes and fireworks that are over.
    pub(super) fn prune_attention(&mut self) {
        let now = Instant::now();
        self.flashes.retain(|_, t| now - *t < FLASH);
        self.fireworks.retain(|_, (t, _)| now - *t < FIREWORKS);
    }

    /// Draws RequestAttention's pane flash and fireworks; says when to draw again.
    pub(super) fn draw_attention(&self, frame: &mut Frame, pane: PaneId, inner: Rect, cell: (f32, f32)) -> Option<Instant> {
        let now = Instant::now();
        let mut next = None;
        if let Some(t) = self.flashes.get(&pane).copied() {
            let age = now - t;
            if age < FLASH {
                scene::pane_flash(frame, inner, 1.0 - age.as_secs_f32() / FLASH.as_secs_f32());
                next = Some(now + Duration::from_millis(16));
            }
        }
        if let Some((t, (col, row))) = self.fireworks.get(&pane).copied() {
            let age = now - t;
            if age < FIREWORKS {
                let center = (inner.x + (col as f32 + 0.5) * cell.0, inner.y + (row as f32 + 0.5) * cell.1);
                scene::fireworks(frame, center, age.as_secs_f32() / FIREWORKS.as_secs_f32(), cell);
                next = Some(now + Duration::from_millis(16));
            }
        }
        next
    }
}

fn rgb_of(r: u8, g: u8, b: u8) -> alacritty_terminal::vte::ansi::Rgb {
    alacritty_terminal::vte::ansi::Rgb { r, g, b }
}

/// The primary monitor's height in pixels (it sits at the virtual screen's origin).
fn primary_height() -> f64 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CYSCREEN};
    unsafe { GetSystemMetrics(SM_CYSCREEN) as f64 }
}

/// Each monitor's work area (without the taskbar) in virtual-screen pixels.
pub(super) fn work_areas() -> Vec<(f64, f64, f64, f64)> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> windows_sys::Win32::Foundation::BOOL {
        let out = unsafe { &mut *(data as *mut Vec<(f64, f64, f64, f64)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut info) } != 0 {
            let r = info.rcWork;
            out.push((r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64));
        }
        1
    }
    let mut out: Vec<(f64, f64, f64, f64)> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut out as *mut _ as LPARAM) };
    out
}
