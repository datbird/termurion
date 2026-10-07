//! The overlays drawn above a browser page (milestone 13x2, Task 5): the hovered link's URL in the
//! pane's corner (`SV:1320-1400`, `SV:1937-1976`, `HLH`), the "Copied" toast (`WVA:193-196`) and
//! the helpers of the announcement bars. A child web view covers anything the scene draws, so the
//! tip and the toast are one small undecorated, non-activating popup window drawn by egui; they
//! are Tandem's own text and are never inside the page, which cannot restyle or forge them.
//! Page-derived text (the URL) passes `browser::display_url` first, so the real host shows.

use serde_json::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

/// iTerm2's message handler of the hover script (`HLH:13`).
pub const HOVER_HANDLER: &str = "iTermHoverLink";
/// The text's padding inside the tip, points (`SV:1323-1324`).
pub const PADDING: (f64, f64) = (8.0, 4.0);
/// The gap to the pane's edges, points (`SV:1389-1392`).
pub const INSET: f64 = 4.0;
/// The toast's text and how long it shows (`WVA:193-196`).
pub const COPIED: &str = "Copied";
pub const COPIED_SECS: f64 = 1.0;
/// The toast's type size, points (`WVA:195`); the tip's is a text field's default.
const TOAST_POINT: f32 = 12.0;
const TIP_POINT: f32 = 13.0;
/// How far outside the link's rectangle the real pointer may be and still count as over it (P14).
const SLOP: f64 = 4.0;
/// The most a toast's text shows (page-derived toast text is cut, P15).
pub const TOAST_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub enum Hover {
    /// The link's URL and its rectangle in the page's own pixels (x, y, width, height).
    Show { url: String, rect: (f64, f64, f64, f64) },
    Clear,
}

/// What a hover script message means (`HLH:34-71`). The session secret is the caller's check
/// (`browser_js::secret_ok`), so this reads only the shape.
pub fn hover_of(body: &Value) -> Option<Hover> {
    match body.get("type")?.as_str()? {
        "hover" => {
            let num = |k: &str| body.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
            Some(Hover::Show { url: body.get("url")?.as_str()?.to_string(), rect: (num("x")?, num("y")?, num("width")?, num("height")?) })
        }
        "clear" => Some(Hover::Clear),
        _ => None,
    }
}

/// What tells the page to forget its hovered link (`HLH:73-74`). Evaluated only in the main frame's
/// Tandem world, never in the page world: the secret-checking handler lives there.
pub fn clear_script(secret: &str) -> String {
    let secret: String = secret.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("(function() {{window.iTermHoverLinkHandler?.clearHover('{secret}');}})();")
}

type R = (f64, f64, f64, f64);

fn overlap(a: R, b: R) -> f64 {
    let w = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
    let h = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// Where the tip of `text_size` goes in `pane`: its four corners (bottom-left, bottom-right,
/// top-left, top-right, as iTerm2's list, with y growing downward), the one that covers `anchor`
/// least; a tie keeps the earlier one (`SV:1320-1340`, `:1384-1400`). The answer is the box with the
/// padding added.
pub fn place(text_size: (f64, f64), pane: R, anchor: R) -> R {
    let (w, h) = (text_size.0 + 2.0 * PADDING.0, text_size.1 + 2.0 * PADDING.1);
    let left = pane.0 + INSET;
    let right = (pane.0 + pane.2 - w - INSET).max(left);
    let top = pane.1 + INSET;
    let bottom = (pane.1 + pane.3 - h - INSET).max(top);
    let mut best = (left, bottom, w, h);
    let mut least = overlap(best, anchor);
    for (x, y) in [(right, bottom), (left, top), (right, top)] {
        let o = overlap((x, y, w, h), anchor);
        if o < least {
            best = (x, y, w, h);
            least = o;
        }
    }
    best
}

/// Whether the real pointer lies in the link's rectangle, give or take `SLOP` (P14). The hover
/// script is copied from iTerm2 and does not check `isTrusted`, so a page can claim any link under
/// the pointer; the pointer itself is the check.
pub fn pointer_in(pointer: (f64, f64), anchor: R) -> bool {
    pointer.0 >= anchor.0 - SLOP && pointer.0 < anchor.0 + anchor.2 + SLOP && pointer.1 >= anchor.1 - SLOP && pointer.1 < anchor.1 + anchor.3 + SLOP
}

/// The URL as the tip shows it: the real host (punycode for look-alikes), no hidden character, and
/// the end of the host kept when it must be cut to `cells` characters (the 13x URL display rule).
pub fn tip_text(url: &str, cells: usize) -> String {
    crate::browser::fit_address(&crate::browser::display_url(url), cells)
}

/// The address cut to what FITS `room` (points) by its measured width: `fit_address` keeps the end of
/// the host, and this narrows its character count until `measure` says the text is no wider than the
/// room (review I1: a page picks wide glyphs, so a per-character guess let the window clip the right
/// side, where the host end is). Always returns something, even with almost no room.
pub fn fit_measured(shown: &str, room: f32, measure: &dyn Fn(&str) -> f32) -> String {
    let mut cells = shown.chars().count();
    loop {
        let text = crate::browser::fit_address(shown, cells);
        let width = measure(&text);
        if width <= room || cells <= 1 {
            return text;
        }
        // Scale by the overshoot, and always step down at least one.
        let scaled = (cells as f32 * (room / width)).floor() as usize;
        cells = scaled.min(cells - 1).max(1);
    }
}

/// What a hover message asks of the tip window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Show,
    Clear,
}

/// What the app does with the window for one ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Create,
    Reuse,
    Hide,
    Skip,
}

/// Windows are made at most this often (review I2: a page cannot make Tandem churn windows).
pub const MIN_CREATE_GAP: std::time::Duration = std::time::Duration::from_millis(250);

/// The one tip window's life: it is made once and then hidden and reused; only the pane going, the
/// app losing the foreground or a failure drops it, and a new one is made no sooner than
/// `MIN_CREATE_GAP` after the last.
#[derive(Default)]
pub struct Lifecycle {
    exists: bool,
    last: Option<std::time::Instant>,
}

impl Lifecycle {
    pub fn show(&mut self, now: std::time::Instant) -> Step {
        if self.exists {
            Step::Reuse
        } else if self.last.is_some_and(|t| now.saturating_duration_since(t) < MIN_CREATE_GAP) {
            Step::Skip
        } else {
            self.exists = true;
            self.last = Some(now);
            Step::Create
        }
    }

    /// A Clear hides the window and keeps it.
    pub fn hide(&mut self) -> Step {
        if self.exists { Step::Hide } else { Step::Skip }
    }

    /// The window was dropped (or never opened).
    pub fn dropped(&mut self) {
        self.exists = false;
    }
}

/// The web view's top and height once a bar covers the top of the pane down to `bar_bottom`: it
/// gives way only for what the bar covers (P21).
pub fn below_bar(rest_y: f32, rest_h: f32, bar_bottom: f32) -> (f32, f32) {
    let cut = (bar_bottom - rest_y).max(0.0).min(rest_h);
    (rest_y + cut, rest_h - cut)
}

/// The index an announcement's answer is: the pressed button's place, None when it was closed
/// (iTerm2's -2 and -1, `PSB:436-440`).
pub fn bar_index(buttons: &[String], pressed: Option<&str>) -> Option<usize> {
    let p = pressed?;
    buttons.iter().position(|b| b == p)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Announce {
    /// A bar with the identifier is showing: answer nil at once, queue nothing (`PSB:422-424`).
    Refuse,
    /// An entry waits whose bar left by another road: it ends with nil first (`dismissAnnouncement`).
    EndStale,
    Queue,
}

pub fn announce_plan(showing: bool, stale_entry: bool) -> Announce {
    if showing {
        Announce::Refuse
    } else if stale_entry {
        Announce::EndStale
    } else {
        Announce::Queue
    }
}

/// Where the window goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    /// In `pane`, away from `anchor` (both in screen pixels).
    Link { pane: R, anchor: R },
    /// Centered above this screen point.
    Point((f64, f64)),
}

/// The tip and toast window: undecorated, no taskbar button, never active, never hit by the pointer.
pub struct TipWindow {
    host: EguiHost,
    text: String,
    /// Whether `text` is a link's address, fitted to the measured width at every paint.
    link: bool,
    point: f32,
    spot: Spot,
    /// The text's measured size in points, and whether the window must be placed again.
    fit: Option<(f32, f32)>,
    dirty: bool,
    shown: bool,
    /// `put` sized the window: it is shown from the paint that follows (never at its old size).
    show_next: bool,
    /// The pane whose hover or toast this is.
    pub owner: Option<u64>,
    /// When a toast ends.
    until: Option<std::time::Instant>,
}

impl TipWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, text: &str, at: R) -> Result<TipWindow, String> {
        let attrs = crate::status_popover_ui::popover_attributes("Tandem Link", 10.0, 10.0).with_active(false).with_resizable(false).with_window_level(winit::window::WindowLevel::AlwaysOnTop);
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        crate::open_quickly_ui::install_fonts(host.ctx());
        host.follow_theme(None);
        if let Some(h) = host.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT};
            unsafe { SetWindowLongPtrW(h as _, GWL_EXSTYLE, GetWindowLongPtrW(h as _, GWL_EXSTYLE) | (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize) };
        }
        let w = TipWindow { host, text: text.to_string(), link: true, point: TIP_POINT, spot: Spot::Link { pane: at, anchor: at }, fit: None, dirty: true, shown: false, show_next: false, owner: None, until: None };
        w.host.request_redraw();
        Ok(w)
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// The hovered link: its text, its anchor in screen pixels; `bound` says which pane it is in.
    pub fn set(&mut self, text: &str, at: R) {
        let pane = match self.spot {
            Spot::Link { pane, .. } => pane,
            Spot::Point(_) => at,
        };
        self.link = true;
        self.change(text, TIP_POINT, Spot::Link { pane, anchor: at }, None);
    }

    /// The pane's rectangle in screen pixels, the tip's room.
    pub fn bound(&mut self, pane: R) {
        if let Spot::Link { anchor, .. } = self.spot {
            if self.spot != (Spot::Link { pane, anchor }) {
                self.spot = Spot::Link { pane, anchor };
                self.dirty = true;
                self.host.request_redraw();
            }
        }
    }

    /// A toast: `text` centered above the screen point `at` for `secs`.
    pub fn toast(&mut self, text: &str, at: (f64, f64), secs: f64) {
        self.link = false;
        self.change(text, TOAST_POINT, Spot::Point(at), Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(secs.clamp(0.0, 30.0))));
    }

    /// Hides the window and keeps it for the next hover (nothing is destroyed on a Clear).
    pub fn hide(&mut self) {
        if let Some(hwnd) = self.host.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
            unsafe { ShowWindow(hwnd as _, SW_HIDE) };
        }
        self.shown = false;
        self.show_next = false;
        self.owner = None;
        self.until = None;
        // The next text is always a change, so the window is placed and shown again.
        self.text.clear();
        self.fit = None;
    }

    pub fn is_toast(&self) -> bool {
        self.until.is_some()
    }

    /// True once a toast's time is over.
    pub fn expired(&self, now: std::time::Instant) -> bool {
        self.until.is_some_and(|t| now >= t)
    }

    fn change(&mut self, text: &str, point: f32, spot: Spot, until: Option<std::time::Instant>) {
        if self.text != text || self.point != point || self.spot != spot {
            self.text = text.to_string();
            self.point = point;
            self.spot = spot;
            self.fit = None;
            self.dirty = true;
            self.host.request_redraw();
        }
        self.until = until;
    }

    pub fn event(&mut self, e: &WindowEvent) {
        if matches!(e, WindowEvent::RedrawRequested) {
            return self.redraw();
        }
        self.host.event(e);
    }

    fn redraw(&mut self) {
        let (text, point) = (self.text.clone(), self.point);
        // The room the text has inside the pane, in points (the window is never wider).
        let room = match self.spot {
            Spot::Link { pane, .. } if self.link => Some((pane.2 / self.host.window.scale_factor().max(0.1) - 2.0 * INSET - 2.0 * PADDING.0).max(1.0) as f32),
            _ => None,
        };
        let mut size = (0.0f32, 0.0f32);
        self.host.paint(|ui| {
            let color = ui.visuals().text_color();
            let fill = ui.visuals().window_fill();
            let line = ui.visuals().widgets.noninteractive.bg_stroke;
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter().rect_stroke(rect, 0.0, line, egui::StrokeKind::Inside);
            let font = egui::FontId::proportional(point);
            let shown = match room {
                Some(room) => fit_measured(&text, room, &|t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), color).size().x),
                None => text.clone(),
            };
            let galley = ui.painter().layout_no_wrap(shown, font, color);
            size = (galley.size().x, galley.size().y);
            ui.painter().galley(egui::pos2(PADDING.0 as f32, PADDING.1 as f32), galley, color);
        });
        if self.fit != Some(size) && size.0 > 0.0 {
            self.fit = Some(size);
            self.dirty = true;
        }
        if self.dirty && self.fit.is_some() {
            self.dirty = false;
            self.put();
        } else if self.show_next && !self.dirty {
            // This paint was made at the size `put` gave: now the window may show.
            if let Some(hwnd) = self.host.hwnd() {
                use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
                unsafe { ShowWindow(hwnd as _, SW_SHOWNOACTIVATE) };
            }
            self.shown = true;
            self.show_next = false;
        }
    }

    /// Sizes and places the window from the measured text, and shows it without taking the focus.
    fn put(&mut self) {
        let Some(fit) = self.fit else { return };
        let scale = self.host.window.scale_factor().max(0.1);
        let text = (f64::from(fit.0), f64::from(fit.1));
        let pts = |r: R| (r.0 / scale, r.1 / scale, r.2 / scale, r.3 / scale);
        let (x, y, w, h) = match self.spot {
            Spot::Link { pane, anchor } => {
                let (x, y, w, h) = place(text, pts(pane), pts(anchor));
                let room = (pane.2 / scale - 2.0 * INSET).max(1.0);
                (x, y, w.min(room), h)
            }
            Spot::Point(p) => {
                let (w, h) = (text.0 + 2.0 * PADDING.0, text.1 + 2.0 * PADDING.1);
                (p.0 / scale - w / 2.0, p.1 / scale - h - 2.0 * INSET, w, h)
            }
        };
        let _ = self.host.window.request_inner_size(winit::dpi::PhysicalSize::new((w * scale).ceil().max(1.0) as u32, (h * scale).ceil().max(1.0) as u32));
        self.host.window.set_outer_position(winit::dpi::PhysicalPosition::new((x * scale).round() as i32, (y * scale).round() as i32));
        if !self.shown {
            self.show_next = true;
        }
        self.host.request_redraw();
    }
}

/// The pointer in screen pixels, as the system has it now (a child web view takes the pointer away
/// from the window, so the window's own last position is stale over a page).
pub fn pointer_now() -> Option<(f64, f64)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((f64::from(p.x), f64::from(p.y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hover_follows_iterm2() {
        let hover = json!({"type": "hover", "url": "https://a", "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0, "sessionSecret": "S"});
        assert_eq!(hover_of(&hover), Some(Hover::Show { url: "https://a".into(), rect: (1.0, 2.0, 3.0, 4.0) }));
        assert_eq!(hover_of(&json!({"type": "clear", "sessionSecret": "S"})), Some(Hover::Clear));
        let mut no_width = hover.clone();
        no_width.as_object_mut().unwrap().remove("width");
        assert_eq!(hover_of(&no_width), None, "HLH:43-50 needs every number");
        assert_eq!(hover_of(&json!({"type": "wiggle", "sessionSecret": "S"})), None);
        assert_eq!(hover_of(&json!({"type": "hover", "url": 5, "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0})), None);
        assert_eq!(hover_of(&json!("hover")), None);
        assert_eq!(clear_script("S"), "(function() {window.iTermHoverLinkHandler?.clearHover('S');})();");
        assert_eq!(HOVER_HANDLER, "iTermHoverLink");
        assert_eq!((COPIED, COPIED_SECS), ("Copied", 1.0));
    }

    #[test]
    fn tip_place_follows_iterm2() {
        let pane = (0.0, 0.0, 800.0, 600.0);
        let far = (300.0, 100.0, 50.0, 20.0);
        assert_eq!(PADDING, (8.0, 4.0));
        assert_eq!(INSET, 4.0);
        // Bottom-left first: the Mac's minY 4 is the bottom, padding 8 and 4 added to the text.
        assert_eq!(place((100.0, 20.0), pane, far), (4.0, 568.0, 116.0, 28.0));
        // The anchor over the bottom-left corner: the bottom-right one.
        assert_eq!(place((100.0, 20.0), pane, (0.0, 560.0, 200.0, 40.0)), (680.0, 568.0, 116.0, 28.0));
        // Over both bottom corners: top-left.
        assert_eq!(place((100.0, 20.0), pane, (0.0, 560.0, 800.0, 40.0)), (4.0, 4.0, 116.0, 28.0));
        // Over every corner: all tie at their own overlap; the least wins, and ties keep the earlier.
        let all = (0.0, 0.0, 800.0, 600.0);
        assert_eq!(place((100.0, 20.0), pane, all), (4.0, 568.0, 116.0, 28.0));
        // A pane that does not start at the origin: the corners are in the pane.
        assert_eq!(place((100.0, 20.0), (50.0, 30.0, 800.0, 600.0), far), (54.0, 598.0, 116.0, 28.0));
        // A text wider than the pane never starts left of the pane.
        assert_eq!(place((900.0, 20.0), pane, far).0, 4.0);
    }

    #[test]
    fn the_real_pointer_must_be_over_the_link() {
        let anchor = (100.0, 100.0, 50.0, 20.0);
        assert!(pointer_in((120.0, 110.0), anchor));
        assert!(pointer_in((97.0, 110.0), anchor), "4 px of slack (P14)");
        assert!(pointer_in((153.9, 123.9), anchor));
        assert!(!pointer_in((95.0, 110.0), anchor));
        assert!(!pointer_in((120.0, 130.0), anchor), "a page that reports a rect elsewhere shows nothing");
        assert!(!pointer_in((f64::NAN, 110.0), anchor));
    }

    #[test]
    fn the_tip_text_keeps_the_host_end_visible() {
        let long = format!("https://user:pw@accounts.example.com/{}", "x".repeat(300));
        let shown = tip_text(&long, 40);
        assert!(shown.chars().count() <= 40 && shown.contains("accounts.example.com"), "{shown}");
        assert_eq!(tip_text("https://a.test/x", 40), "https://a.test/x");
        // A look-alike host shows as the host it is; hidden characters never show.
        assert_eq!(tip_text("https://\u{430}pple.com/\u{202e}x", 60), "https://xn--pple-43d.com/x");
        // The host that does not fit keeps its suffix and the START of the registrable label (cut from
        // the right, so no other brand's name forms; Task 6 round 2 new breakage).
        let host = format!("https://{}evil.test/", "a".repeat(100));
        let t = tip_text(&host, 30);
        assert!(t.ends_with("\u{2026}.test") && t.starts_with('a') && t.chars().count() <= 30, "{t}");
    }

    #[test]
    fn the_web_view_gives_way_to_the_bar_only() {
        // rest starts at 40 (below the toolbar); a bar over the pane at y 0 ends at 24: nothing to give.
        assert_eq!(below_bar(40.0, 400.0, 24.0), (40.0, 400.0));
        // A bar that ends at 56 covers 16 of the page.
        assert_eq!(below_bar(40.0, 400.0, 56.0), (56.0, 384.0));
        // A bar taller than the page leaves nothing, never a negative height.
        assert_eq!(below_bar(40.0, 10.0, 90.0), (50.0, 0.0));
    }

    #[test]
    fn answers_follow_iterm2() {
        let buttons = vec!["Allow _Once".to_string(), "_Deny Once".to_string()];
        assert_eq!(bar_index(&buttons, Some("_Deny Once")), Some(1));
        assert_eq!(bar_index(&buttons, Some("Allow _Once")), Some(0));
        assert_eq!(bar_index(&buttons, None), None, "closed is nil (PSB:436-440)");
        assert_eq!(bar_index(&buttons, Some("Other")), None);
        assert_eq!(announce_plan(true, false), Announce::Refuse, "a second bar with the identifier answers nil (PSB:422-424)");
        assert_eq!(announce_plan(true, true), Announce::Refuse);
        assert_eq!(announce_plan(false, true), Announce::EndStale, "an entry whose bar left by another road ends first");
        assert_eq!(announce_plan(false, false), Announce::Queue);
    }

    /// Review I1: the tip fits its text to the MEASURED width, so the end of the host is never clipped
    /// (a page chooses wide glyphs; a per-character guess let the window clip the right side).
    #[test]
    fn the_host_end_survives_wide_letters() {
        // 'W' and fullwidth W are wide, everything else is narrow, the ellipsis is narrow.
        let measure = |t: &str| t.chars().map(|c| if c == 'W' || c == '\u{ff37}' { 22.0 } else { 7.0 }).sum::<f32>();
        let urls = [
            "https://accounts.google.com.WWWWWWWWWWWWWWWWWWWWWWWWW.evil.test/",
            "https://accounts.google.com@WWWWWWWWWWWWWWWWWWWWWWWWW.evil.test/path?q=1",
            "https://\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}\u{ff37}@evil.test/p",
        ];
        for url in urls {
            let shown = crate::browser::display_url(url);
            for room in [90.0f32, 140.0, 220.0, 400.0] {
                let t = fit_measured(&shown, room, &measure);
                assert!(measure(&t) <= room, "{room}: {t} is {}", measure(&t));
                assert!(t.contains("evil.test") || t.ends_with("evil.test") || (room < 140.0 && t.ends_with(".test")), "{room}: the registrable host is shown: {t}");
            }
            // Room for all of it: nothing is cut.
            assert_eq!(fit_measured(&shown, 100_000.0, &measure), shown);
        }
        // Never an empty tip, and never a loop without end, however little room there is.
        assert!(!fit_measured("https://evil.test/", 1.0, &measure).is_empty());
    }

    /// Review I2: a page that alternates hover and clear messages makes at most one window, a Clear
    /// is queued like a Show (newest wins), and creation is rate-limited.
    #[test]
    fn a_flood_of_hovers_makes_one_window() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut life = Lifecycle::default();
        let mut made = 0;
        for i in 0..1000u32 {
            let now = t0 + Duration::from_micros(u64::from(i) * 100);
            // One message per tick, as a page can send; the newest queued ask wins.
            let ask = if i % 2 == 0 { Ask::Show } else { Ask::Clear };
            let mut next = Some(ask);
            if i % 3 == 0 {
                next = Some(Ask::Show);
            }
            match next.take() {
                Some(Ask::Show) => made += usize::from(life.show(now) == Step::Create),
                Some(Ask::Clear) => {
                    life.hide();
                }
                None => {}
            }
        }
        assert_eq!(made, 1, "one window, reused");
        // A Clear after a Show in the same drain wins; a window that was dropped is not remade at once.
        let mut next = Some(Ask::Show);
        next = Some(Ask::Clear).or(next);
        assert_eq!(next, Some(Ask::Clear));
        let mut life = Lifecycle::default();
        assert_eq!(life.show(t0), Step::Create);
        life.dropped();
        assert_eq!(life.show(t0 + Duration::from_millis(10)), Step::Skip, "creation is rate-limited");
        assert_eq!(life.show(t0 + Duration::from_millis(300)), Step::Create);
        assert_eq!(life.hide(), Step::Hide);
        assert_eq!(life.show(t0 + Duration::from_millis(301)), Step::Reuse);
    }

    /// Task 5 re-review minor: a reused window is shown only from a paint at its new size, never
    /// from `put` (which would show the new text at the old size for a frame).
    #[test]
    fn the_window_is_shown_after_the_resize_paint() {
        let src = include_str!("browser_tip.rs");
        let live = &src[..src.find("#[cfg(test)]").unwrap()];
        let put = &live[live.find("fn put(&mut self)").unwrap()..live.find("/// The pointer in screen pixels").unwrap()];
        assert!(put.contains("self.show_next = true") && !put.contains("ShowWindow"), "put only asks for the show");
        let redraw = &live[live.find("fn redraw(&mut self)").unwrap()..live.find("/// Sizes and places").unwrap()];
        assert!(redraw.contains("self.show_next && !self.dirty") && redraw.contains("SW_SHOWNOACTIVATE"), "the paint after the resize shows it");
        let hide = &live[live.find("pub fn hide(&mut self) {").unwrap()..live.find("pub fn is_toast").unwrap()];
        assert!(hide.contains("self.show_next = false"));
    }
}
