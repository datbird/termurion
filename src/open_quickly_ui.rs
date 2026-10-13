//! View > Open Quickly's windows (`iTermOpenQuicklyWindowController.m`, `WC`, and its xib):
//! the palette, a borderless 488-point window 170 points below the top of the screen with the
//! search field and the ranked rows, and the session preview beside it
//! (`iTermSessionPreviewPanel.m`).

use std::ops::Range;

use crate::layout::Rect;

/// The palette's width, points (the xib).
pub const WIDTH: f32 = 488.0;
/// Above the field, the field, below it (`kMarginAboveField`, the xib, `kMarginBelowField`).
const ABOVE_FIELD: f32 = 12.0;
const FIELD_H: f32 = 26.0;
const BELOW_FIELD: f32 = 9.0;
/// Everything but the table (`nonTableSpace`).
const NON_TABLE: f32 = ABOVE_FIELD + FIELD_H + BELOW_FIELD;
/// A row: 44 high, with no intercell spacing (the xib's 2 is set to zero at run time,
/// `WC:91`: "Using intercell spacing clips the selection rect").
pub const ROW_H: f32 = 44.0;
/// Below the last row, and the room the palette leaves above and below it on the screen.
const BELOW_ROWS: f32 = 10.0;
pub const ABOVE_WINDOW: f32 = 170.0;

/// The preview: its width, inset, label block (title 18, gap 2, detail 14, gap 8), the
/// image's height with no image, and its gap from the palette (`iTermSessionPreviewPanel.m`).
const PREVIEW_W: f32 = 400.0;
const PREVIEW_INSET: f32 = 12.0;
const PREVIEW_TITLE_H: f32 = 18.0;
const PREVIEW_DETAIL_H: f32 = 14.0;
const PREVIEW_LABEL_GAP: f32 = 2.0;
const PREVIEW_BELOW_LABELS: f32 = 8.0;
const PREVIEW_LABELS: f32 = PREVIEW_TITLE_H + PREVIEW_LABEL_GAP + PREVIEW_DETAIL_H + PREVIEW_BELOW_LABELS;
const PREVIEW_NO_IMAGE_H: f32 = 240.0;
const PREVIEW_GAP: f32 = 8.0;

/// A key the palette acts on (`WC:601-676`, `iTermOpenQuicklyTextField.m`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteKey {
    Up,
    Down,
    Open { alt: bool },
    Close,
}

/// The palette's action for a key, `key` named as `Up`, `Down`, `Enter`, `Escape`, or a
/// letter. Arrows move only without modifiers; Control+P and Control+N move like them
/// (`moveUp:`/`moveDown:`); Return opens, with Option (Alt) only when Alt is the one modifier
/// held; Esc closes. Everything else goes to the field.
pub fn palette_key(key: &str, ctrl: bool, alt: bool, shift: bool, win: bool) -> Option<PaletteKey> {
    let none = !(ctrl || alt || shift || win);
    let ctrl_only = ctrl && !(alt || shift || win);
    match key {
        "Escape" => Some(PaletteKey::Close),
        "Enter" => Some(PaletteKey::Open { alt: alt && !(ctrl || shift || win) }),
        "Up" if none => Some(PaletteKey::Up),
        "Down" if none => Some(PaletteKey::Down),
        "P" if ctrl_only => Some(PaletteKey::Up),
        "N" if ctrl_only => Some(PaletteKey::Down),
        _ => None,
    }
}

/// The row a move selects among `n` rows: nothing selected goes to the first row; moves wrap.
pub fn wrap_step(selected: Option<usize>, n: usize, down: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(match selected {
        None => 0,
        Some(i) if down => (i + 1) % n,
        Some(0) => n - 1,
        Some(i) => (i - 1).min(n - 1),
    })
}

/// The palette's height in points for `rows` results on a screen `screen_h` points tall
/// (`WC`, `frame`): the field's space, then as many rows as fit in the screen less 170 above
/// and below, to the bottom of the last visible row, and 10 more.
pub fn window_height(rows: usize, screen_h: f32) -> f32 {
    let max_h = screen_h - ABOVE_WINDOW * 2.0;
    let fit = ((max_h - NON_TABLE) / ROW_H).floor().max(0.0) as usize;
    let n = rows.min(fit);
    if n == 0 {
        return NON_TABLE;
    }
    NON_TABLE + n as f32 * ROW_H + BELOW_ROWS
}

/// Where the preview goes, in points, for the palette at `parent` on a screen whose work area
/// is `work`, with a snapshot `image` (width, height) or none (`windowFrameForParentFrame:`):
/// 400 wide, 8 right of the palette, else 8 left of it, else against the work area's right
/// edge; its top at the palette's top, kept on the work area; as tall as the labels and the
/// image at 376 wide, from 200 to the work area's height less 24 (never under 300).
pub fn preview_rect(parent: Rect, work: Rect, image: Option<(u32, u32)>) -> Rect {
    let inner_w = PREVIEW_W - 2.0 * PREVIEW_INSET;
    let image_h = match image {
        Some((w, h)) if w > 0 => inner_w * (h as f32 / w as f32),
        _ => PREVIEW_NO_IMAGE_H,
    };
    let max_h = (work.h - 24.0).max(300.0);
    let h = (PREVIEW_INSET + PREVIEW_LABELS + image_h + PREVIEW_INSET).max(200.0).min(max_h);
    let mut x = parent.x + parent.w + PREVIEW_GAP;
    if x + PREVIEW_W > work.x + work.w {
        let left = parent.x - PREVIEW_GAP - PREVIEW_W;
        x = if left >= work.x { left } else { work.x + work.w - PREVIEW_W };
    }
    x = x.max(work.x);
    // On the work area, its top winning over its bottom (the last clamp in the source).
    let y = parent.y.min(work.y + work.h - h).max(work.y);
    Rect { x, y, w: PREVIEW_W, h }
}

/// The preview's detail (`previewDetailForSession:`): the tab's number, and the window's title
/// when it has one.
pub fn preview_detail(tab: usize, window_title: &str) -> String {
    if window_title.is_empty() {
        format!("Tab {tab}")
    } else {
        format!("Tab {tab} \u{00B7} {window_title}")
    }
}

/// `text` cut into runs, each with whether it is bold, from the char indexes `bold` (the
/// matcher's indexes are chars, not bytes).
pub fn bold_runs(text: &str, bold: &[Range<usize>]) -> Vec<(Range<usize>, bool)> {
    let mut out: Vec<(Range<usize>, bool)> = Vec::new();
    for (ci, (bi, c)) in text.char_indices().enumerate() {
        let b = bold.iter().any(|r| r.contains(&ci));
        let end = bi + c.len_utf8();
        match out.last_mut() {
            Some((r, was)) if *was == b => r.end = end,
            _ => out.push((bi..end, b)),
        }
    }
    out
}

/// A window's features (`addWindowLocationToItems:withMatcher:`): with two or more monitors
/// "On" its monitor's name (or "Offscreen"), then "Miniaturized", "Full screen", and a hotkey
/// window's "Hotkey" text. Windows has no Spaces, so never "On other Space".
pub fn window_features(monitors: usize, monitor: Option<&str>, minimized: bool, full_screen: bool, hotkey: Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    if monitors > 1 {
        out.push(monitor.map_or_else(|| "Offscreen".to_string(), |m| format!("On {m}")));
    }
    if minimized {
        out.push("Miniaturized".to_string());
    }
    if full_screen {
        out.push("Full screen".to_string());
    }
    out.extend(hotkey);
    out
}

/// A hotkey window's feature: "Hotkey" and its first shortcut, else its double-tap modifier
/// (0 Control, 1 Shift, 2 Option, 3 Command) named twice with nothing between
/// (`Hotkey %1$@%2$@`, kept).
pub fn hotkey_feature(first: Option<String>, double_tap: Option<u8>) -> Option<String> {
    if let Some(s) = first {
        return Some(format!("Hotkey {s}"));
    }
    let name = match double_tap? {
        0 => "Ctrl",
        1 => "Shift",
        2 => "Alt",
        3 => "Win",
        _ => return None,
    };
    Some(format!("Hotkey {name}{name}"))
}

// -------------------------------------------------------------------------------------------
// The windows.
// -------------------------------------------------------------------------------------------

use std::sync::OnceLock;
use std::time::Instant;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;

/// One row as the palette draws it: its icon glyph, and its title and detail with the char
/// indexes (title) and char ranges (detail) to bold.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteRow {
    pub icon: char,
    /// A color preset's swatch: the glyph in this foreground on this background, in place of
    /// the plain icon (`OQM:624-626`).
    pub swatch: Option<([u8; 3], [u8; 3])>,
    pub title: String,
    pub title_bold: Vec<usize>,
    /// The title is a URL: fitted to the title width by the one shortener (`open_quickly::url_title_fit`),
    /// never cut at its tail.
    pub title_url: bool,
    pub detail: String,
    pub detail_bold: Vec<Range<usize>>,
}

/// What the palette asks of the app after a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    None,
    /// The query changed (`controlTextDidChange:`).
    Query(String),
    /// Another row was selected (the preview follows it).
    Selected(usize),
    /// Return, Option+Return or a double-click on a row (`openSelectedRow`).
    Open { row: usize, alt: bool },
    Close,
}

/// How long the palette waits for the focus before closing.
const NO_FOCUS_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Segoe Fluent Icons' Search and Cancel: the magnifier and the x.
const LOUPE: char = '\u{E721}';
const CLOSE_GLYPH: char = '\u{E711}';

fn icons() -> egui::FontFamily {
    egui::FontFamily::Name("icons".into())
}

/// The width a row's title and detail lines are laid out in (points).
const TITLE_WIDTH: f32 = 412.0;

fn bold() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

fn light() -> egui::FontFamily {
    egui::FontFamily::Name("light".into())
}

/// Windows' own fonts for the palette: Segoe UI, its bold and light faces, and the icon font
/// (Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on Windows 10, same code points). Read
/// once; a face that is missing leaves its family on egui's own font.
fn system_fonts() -> &'static Vec<(&'static str, Vec<u8>)> {
    static FONTS: OnceLock<Vec<(&'static str, Vec<u8>)>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        let mut out = Vec::new();
        for (family, files) in [("regular", &["segoeui.ttf"][..]), ("bold", &["segoeuib.ttf"][..]), ("light", &["segoeuil.ttf"][..]), ("icons", &["SegoeIcons.ttf", "segmdl2.ttf"][..])] {
            if let Some(bytes) = files.iter().find_map(|f| std::fs::read(dir.join(f)).ok()) {
                out.push((family, bytes));
            }
        }
        out
    })
}

pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let base = defs.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    let mut have = |face: &str| -> Option<String> {
        let (_, bytes) = system_fonts().iter().find(|(f, _)| *f == face)?;
        let name = format!("termurion-{face}");
        defs.font_data.entry(name.clone()).or_insert_with(|| std::sync::Arc::new(egui::FontData::from_owned(bytes.clone())));
        Some(name)
    };
    // Bold and light fall back to the regular face, then egui's own.
    let family = |faces: &[Option<String>]| -> Vec<String> { faces.iter().flatten().cloned().chain(base.iter().cloned()).collect() };
    let (regular, b, l, i) = (have("regular"), have("bold"), have("light"), have("icons"));
    let families = [
        (egui::FontFamily::Proportional, family(std::slice::from_ref(&regular))),
        (bold(), family(&[b, regular.clone()])),
        (light(), family(&[l, regular])),
        (icons(), family(&[i])),
    ];
    for (f, list) in families {
        defs.families.insert(f, list);
    }
    ctx.set_fonts(defs);
}

/// The Windows accent color (the DWM colorization color), for the selection
/// (`isEmphasized`: the selection is always in the accent color).
fn accent() -> egui::Color32 {
    let (mut color, mut opaque) = (0u32, 0i32);
    let ok = unsafe { windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut color, &mut opaque) } >= 0;
    if !ok {
        return egui::Color32::from_rgb(0x00, 0x67, 0xC0);
    }
    egui::Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

/// Rounded corners (Windows 11 rounds a borderless window only when asked).
fn round_corners(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    let v = DWMWCP_ROUND;
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&v as *const i32).cast(), 4) };
}

/// A height moving to another over the Open Quickly animation duration.
struct Anim {
    from: f32,
    to: f32,
    start: Instant,
    secs: f32,
}

impl Anim {
    fn now(&self) -> f32 {
        let t = if self.secs <= 0.0 { 1.0 } else { (self.start.elapsed().as_secs_f32() / self.secs).min(1.0) };
        // AppKit's ease-in-out.
        let e = t * t * (3.0 - 2.0 * t);
        self.from + (self.to - self.from) * e
    }

    fn done(&self) -> bool {
        self.secs <= 0.0 || self.start.elapsed().as_secs_f32() >= self.secs
    }
}

/// The Open Quickly palette.
pub struct OpenQuicklyWindow {
    host: EguiHost,
    query: String,
    rows: Vec<PaletteRow>,
    selected: Option<usize>,
    /// Scroll the selected row into view on the next frame.
    reveal: bool,
    /// The next frame focuses the field with its text selected (the query kept from last time
    /// is selected on a re-open, `selectText:`), or with the caret at its end (a tip).
    select_all: bool,
    caret_to_end: bool,
    /// It has had focus (a focus loss before that is the window being made), and when it was
    /// shown: one that never gets the focus closes after `NO_FOCUS_LIMIT`.
    focused: bool,
    opened: Instant,
    /// The screen's height in points, and the palette's top left in pixels.
    screen_h: f32,
    origin: (f64, f64),
    anim: Anim,
    duration: f32,
    /// The height last asked of the window, in points.
    applied: f32,
    accent: egui::Color32,
}

impl OpenQuicklyWindow {
    /// Opens the palette on the monitor `monitor` (pixels): centered, its top 170 points below
    /// the monitor's top, with just the field until rows arrive; `query` is kept from the last
    /// time and selected; its height moves over `duration` seconds (0 for no animation).
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        monitor: Rect,
        query: &str,
        dark: bool,
        duration: f32,
    ) -> Result<OpenQuicklyWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let attrs = Window::default_attributes()
            .with_title("Open Quickly")
            .with_decorations(false)
            .with_resizable(false)
            .with_skip_taskbar(true)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_visible(false)
            .with_position(winit::dpi::PhysicalPosition::new(monitor.x as f64, monitor.y as f64))
            .with_inner_size(winit::dpi::LogicalSize::new(WIDTH as f64, NON_TABLE as f64));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        let scale = host.window.scale_factor();
        install_fonts(host.ctx());
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        let x = monitor.x as f64 + ((monitor.w as f64 - WIDTH as f64 * scale) / 2.0).floor();
        let y = monitor.y as f64 + ABOVE_WINDOW as f64 * scale;
        host.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        if let Some(h) = host.hwnd() {
            round_corners(h);
        }
        host.window.set_visible(true);
        host.focus();
        let screen_h = (monitor.h as f64 / scale) as f32;
        let anim = Anim { from: NON_TABLE, to: NON_TABLE, start: Instant::now(), secs: 0.0 };
        Ok(OpenQuicklyWindow {
            host,
            query: query.to_string(),
            rows: Vec::new(),
            selected: None,
            reveal: false,
            select_all: true,
            caret_to_end: false,
            focused: false,
            opened: Instant::now(),
            screen_h,
            origin: (x, y),
            anim,
            duration: duration.max(0.0),
            applied: NON_TABLE,
            accent: accent(),
        })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.window.set_minimized(false);
        self.host.focus();
    }

    /// While it has never had the focus: when it gives up and closes (Windows may refuse to
    /// bring it forward, or a click elsewhere may come first, and then Esc cannot reach it).
    pub fn no_focus_deadline(&self) -> Option<Instant> {
        (!self.focused).then(|| self.opened + NO_FOCUS_LIMIT)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Puts `q` in the field with the caret at its end (a tip's `"/x "`).
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.caret_to_end = true;
        self.host.request_redraw();
    }

    /// New results: the first row selected and scrolled to, the height moving to fit them
    /// (`update`).
    pub fn set_items(&mut self, rows: Vec<PaletteRow>) {
        self.selected = if rows.is_empty() { None } else { Some(0) };
        self.reveal = true;
        let to = window_height(rows.len(), self.screen_h);
        self.rows = rows;
        if to != self.anim.to {
            self.anim = Anim { from: self.anim.now(), to, start: Instant::now(), secs: self.duration };
        }
        self.host.request_redraw();
    }

    /// Where the palette ends up (its height when the animation is done), in pixels: what the
    /// preview is placed beside (`repositionForParentFrame:` takes the target frame).
    pub fn target_rect(&self) -> Rect {
        let scale = self.host.window.scale_factor() as f32;
        Rect { x: self.origin.0 as f32, y: self.origin.1 as f32, w: WIDTH * scale, h: self.anim.to * scale }
    }

    pub fn scale(&self) -> f64 {
        self.host.window.scale_factor()
    }

    /// Handles one of its window's events. False when it should close: it lost focus after
    /// having it (`windowDidResignKey:`), or was asked to close.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => false,
            WindowEvent::Focused(true) => {
                self.focused = true;
                self.host.event(event);
                true
            }
            WindowEvent::Focused(false) => !self.focused,
            other => {
                self.host.event(other);
                true
            }
        }
    }

    pub fn redraw(&mut self) -> PaletteEvent {
        // The height, animated.
        let h = self.anim.now();
        if (h - self.applied).abs() > 0.01 {
            let _ = self.host.window.request_inner_size(winit::dpi::LogicalSize::new(WIDTH as f64, h as f64));
            self.applied = h;
        }
        if !self.anim.done() {
            self.host.request_redraw();
        }

        let mut out = PaletteEvent::None;
        let before = self.query.clone();
        let accent = self.accent;
        let n = self.rows.len();
        let (query, rows, selected, reveal, select_all, caret_to_end) = (&mut self.query, &self.rows, &mut self.selected, &mut self.reveal, &mut self.select_all, &mut self.caret_to_end);
        self.host.paint(|ui| {
            // The palette's keys never reach the field (Review Focus 6).
            let mut keys = Vec::new();
            ui.ctx().input_mut(|i| {
                i.events.retain(|e| {
                    let egui::Event::Key { key, pressed, modifiers, .. } = e else { return true };
                    let name = match key {
                        egui::Key::ArrowUp => "Up",
                        egui::Key::ArrowDown => "Down",
                        egui::Key::Enter => "Enter",
                        egui::Key::Escape => "Escape",
                        egui::Key::P => "P",
                        egui::Key::N => "N",
                        _ => return true,
                    };
                    match palette_key(name, modifiers.ctrl, modifiers.alt, modifiers.shift, false) {
                        Some(k) => {
                            if *pressed {
                                keys.push(k);
                            }
                            false
                        }
                        None => true,
                    }
                });
            });
            for k in keys {
                match k {
                    PaletteKey::Up | PaletteKey::Down => {
                        *selected = wrap_step(*selected, n, k == PaletteKey::Down);
                        *reveal = true;
                        if let (Some(i), PaletteEvent::None | PaletteEvent::Selected(_)) = (*selected, &out) {
                            out = PaletteEvent::Selected(i);
                        }
                    }
                    PaletteKey::Open { alt } => {
                        if let Some(row) = *selected {
                            out = PaletteEvent::Open { row, alt };
                        }
                    }
                    PaletteKey::Close => out = PaletteEvent::Close,
                }
            }

            let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                let full = ui.max_rect();
                let (x0, y0) = (full.min.x, full.min.y);
                let text = ui.visuals().text_color();
                let weak = ui.visuals().weak_text_color();

                // The field: the magnifier at the left, the x at the right once there is a query.
                let loupe = egui::Rect::from_min_size(egui::pos2(x0 + 13.0, y0 + ABOVE_FIELD + 2.0), egui::vec2(22.0, 22.0));
                ui.painter().text(loupe.center(), egui::Align2::CENTER_CENTER, LOUPE, egui::FontId::new(16.0, icons()), weak);
                let field_rect = egui::Rect::from_min_size(egui::pos2(x0 + 43.0, y0 + ABOVE_FIELD), egui::vec2(410.0, FIELD_H));
                let field_id = egui::Id::new("open-quickly-field");
                if *select_all || *caret_to_end {
                    let len = query.chars().count();
                    let mut st = egui::text_edit::TextEditState::load(ui.ctx(), field_id).unwrap_or_default();
                    let from = if *select_all { 0 } else { len };
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(from), egui::text::CCursor::new(len))));
                    st.store(ui.ctx(), field_id);
                    *select_all = false;
                    *caret_to_end = false;
                }
                let field = ui.put(
                    field_rect,
                    egui::TextEdit::singleline(query).id(field_id).frame(egui::Frame::NONE).font(egui::FontId::new(20.0, light())).hint_text(crate::open_quickly::PLACEHOLDER).desired_width(field_rect.width()),
                );
                // The field keeps the keyboard; a click on a row does not take it away.
                if !field.has_focus() {
                    field.request_focus();
                }
                if !query.is_empty() {
                    let x_rect = egui::Rect::from_min_size(egui::pos2(x0 + 456.0, y0 + ABOVE_FIELD + 2.5), egui::vec2(21.0, 21.0));
                    let x = ui.put(x_rect, egui::Button::new(egui::RichText::new(CLOSE_GLYPH.to_string()).family(icons()).size(11.0).color(weak)).frame(false));
                    // iTerm2's x closes the window rather than clearing the field (kept).
                    if x.clicked() {
                        out = PaletteEvent::Close;
                    }
                }

                // The rows.
                let table = egui::Rect::from_min_max(egui::pos2(x0, y0 + NON_TABLE), egui::pos2(full.max.x, full.max.y - BELOW_ROWS));
                if rows.is_empty() || table.height() <= 0.0 {
                    return;
                }
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(table));
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    for (i, row) in rows.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(WIDTH, ROW_H), egui::Sense::click());
                        // The cell view: 10 in from each side.
                        let cell = egui::Rect::from_min_size(egui::pos2(r.min.x + 10.0, r.min.y), egui::vec2(WIDTH - 20.0, ROW_H));
                        let chosen = *selected == Some(i);
                        let (title_color, detail_color) = if chosen { (egui::Color32::WHITE, egui::Color32::from_white_alpha(200)) } else { (text, weak) };
                        if chosen {
                            ui.painter().rect_filled(cell, 5.0, accent);
                        }
                        let icon = egui::Rect::from_min_size(egui::pos2(cell.min.x + 9.0, cell.min.y + 2.0), egui::vec2(40.0, 40.0));
                        match row.swatch {
                            Some((fg, bg)) => {
                                let tile = icon.shrink(4.0);
                                ui.painter().rect_filled(tile, 6.0, egui::Color32::from_rgb(bg[0], bg[1], bg[2]));
                                ui.painter().rect_stroke(tile, 6.0, egui::Stroke::new(1.0, weak), egui::StrokeKind::Inside);
                                ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, row.icon, egui::FontId::new(20.0, icons()), egui::Color32::from_rgb(fg[0], fg[1], fg[2]));
                            }
                            None => {
                                ui.painter().text(icon.center(), egui::Align2::CENTER_CENTER, row.icon, egui::FontId::new(24.0, icons()), title_color);
                            }
                        }
                        // A URL title is fitted to the title width by the one shortener, measured in
                        // the row's own runs, so the tail truncation below never cuts its host.
                        let (title_text, title_idx) = if row.title_url {
                            let measure = |t: &str| {
                                let runs: Vec<Range<usize>> = if t == row.title { row.title_bold.iter().map(|&c| c..c + 1).collect() } else { Vec::new() };
                                ui.painter().layout_job(runs_job(t, &runs, 13.0, title_color, f32::INFINITY)).size().x
                            };
                            crate::open_quickly::url_title_fit(&row.title, &row.title_bold, TITLE_WIDTH, &measure)
                        } else {
                            (row.title.clone(), row.title_bold.clone())
                        };
                        let title_bold: Vec<Range<usize>> = title_idx.iter().map(|&c| c..c + 1).collect();
                        let title = runs_job(&title_text, &title_bold, 13.0, title_color, TITLE_WIDTH);
                        let detail = runs_job(&row.detail, &row.detail_bold, 11.0, detail_color, TITLE_WIDTH);
                        let title = ui.painter().layout_job(title);
                        let detail = ui.painter().layout_job(detail);
                        ui.painter().galley(egui::pos2(cell.min.x + 56.0, cell.min.y + 5.0), title, title_color);
                        ui.painter().galley(egui::pos2(cell.min.x + 56.0, cell.min.y + 25.0), detail, detail_color);
                        if resp.double_clicked() {
                            let alt = ui.input(|inp| inp.modifiers.alt);
                            out = PaletteEvent::Open { row: i, alt };
                        } else if resp.clicked() && *selected != Some(i) {
                            *selected = Some(i);
                            if !matches!(out, PaletteEvent::Close | PaletteEvent::Open { .. }) {
                                out = PaletteEvent::Selected(i);
                            }
                        }
                        if *reveal && chosen {
                            resp.scroll_to_me(None);
                        }
                    }
                });
                *reveal = false;
            });
        });
        // A new query wins over a move in the same frame: the rows are about to change.
        if self.query != before && !matches!(out, PaletteEvent::Close | PaletteEvent::Open { .. }) {
            out = PaletteEvent::Query(self.query.clone());
        }
        out
    }
}

/// A one-line layout of `text` at `size` points with the char ranges `bold` in the bold face,
/// cut with an ellipsis at `width` (`NSLineBreakByTruncatingTail`).
fn runs_job(text: &str, bold_ranges: &[Range<usize>], size: f32, color: egui::Color32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (r, b) in bold_runs(text, bold_ranges) {
        let family = if b { bold() } else { egui::FontFamily::Proportional };
        job.append(&text[r], 0.0, egui::TextFormat { font_id: egui::FontId::new(size, family), color, ..Default::default() });
    }
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('\u{2026}') };
    job
}

/// The session preview beside the palette: the session's name, its tab, and a picture of its
/// screen. It never takes the focus (a non-activating panel).
pub struct PreviewWindow {
    host: EguiHost,
    title: String,
    detail: String,
    /// The pane shown, and its picture's texture.
    pane: Option<u64>,
    texture: Option<egui::TextureHandle>,
    shown: bool,
}

impl PreviewWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, dark: bool) -> Result<PreviewWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let attrs = Window::default_attributes()
            .with_title("Open Quickly Preview")
            .with_decorations(false)
            .with_resizable(false)
            .with_skip_taskbar(true)
            .with_active(false)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(PREVIEW_W as f64, 306.0));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        install_fonts(host.ctx());
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        if let Some(h) = host.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE};
            unsafe { SetWindowLongPtrW(h as _, GWL_EXSTYLE, GetWindowLongPtrW(h as _, GWL_EXSTYLE) | WS_EX_NOACTIVATE as isize) };
            round_corners(h);
        }
        Ok(PreviewWindow { host, title: String::new(), detail: String::new(), pane: None, texture: None, shown: false })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// Shows `pane` with its title and detail; `image` is its screen as RGBA (width, height,
    /// pixels), or None.
    pub fn set(&mut self, pane: u64, title: &str, detail: &str, image: Option<(u32, u32, &[u8])>) {
        self.title = title.to_string();
        self.detail = detail.to_string();
        if self.pane != Some(pane) {
            self.pane = Some(pane);
            self.texture = image.map(|(w, h, px)| {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], px);
                self.host.ctx().load_texture("open-quickly-preview", img, egui::TextureOptions::LINEAR)
            });
        }
        self.host.request_redraw();
    }

    /// Puts it at `r` (pixels) and shows it without taking the focus.
    pub fn place(&mut self, r: Rect) {
        self.host.window.set_outer_position(winit::dpi::PhysicalPosition::new(r.x as f64, r.y as f64));
        let _ = self.host.window.request_inner_size(winit::dpi::PhysicalSize::new(r.w.round() as u32, r.h.round() as u32));
        if !self.shown {
            if let Some(h) = self.host.hwnd() {
                use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
                unsafe { ShowWindow(h as _, SW_SHOWNOACTIVATE) };
            }
            self.shown = true;
        }
        self.host.request_redraw();
    }

    /// Hides it (a row with no session).
    pub fn hide(&mut self) {
        if self.shown {
            self.host.window.set_visible(false);
            self.shown = false;
        }
    }

    pub fn event(&mut self, event: &WindowEvent) {
        if let WindowEvent::RedrawRequested = event {
            return self.redraw();
        }
        self.host.event(event);
    }

    fn redraw(&mut self) {
        let (title, detail, texture) = (&self.title, &self.detail, &self.texture);
        self.host.paint(|ui| {
            let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                let full = ui.max_rect();
                let inner_w = full.width() - 2.0 * PREVIEW_INSET;
                let (x, y) = (full.min.x + PREVIEW_INSET, full.min.y + PREVIEW_INSET);
                let text = ui.visuals().text_color();
                let weak = ui.visuals().weak_text_color();
                // The title is bold throughout.
                let whole = 0..title.chars().count();
                let t = runs_job(title, std::slice::from_ref(&whole), 13.0, text, inner_w);
                let t = ui.painter().layout_job(t);
                ui.painter().galley(egui::pos2(x, y), t, text);
                let d = runs_job(detail, &[], 11.0, weak, inner_w);
                let d = ui.painter().layout_job(d);
                ui.painter().galley(egui::pos2(x, y + PREVIEW_TITLE_H + PREVIEW_LABEL_GAP), d, weak);
                let image_y = y + PREVIEW_LABELS;
                let avail = egui::vec2(inner_w, (full.max.y - PREVIEW_INSET - image_y).max(0.0));
                if let Some(tex) = texture {
                    // Scaled proportionally to fit, flush with the labels (`NSImageAlignTop`).
                    let size = tex.size_vec2();
                    let k = (avail.x / size.x).min(avail.y / size.y);
                    let shown = size * k;
                    let r = egui::Rect::from_min_size(egui::pos2(x + (avail.x - shown.x) / 2.0, image_y), shown);
                    ui.put(r, egui::Image::new(tex).fit_to_exact_size(shown).corner_radius(4.0));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn palette_keys_follow_iterm2() {
        use PaletteKey::*;
        assert_eq!(palette_key("Up", false, false, false, false), Some(Up));
        assert_eq!(palette_key("Down", false, false, false, false), Some(Down));
        assert_eq!(palette_key("P", true, false, false, false), Some(Up));
        assert_eq!(palette_key("N", true, false, false, false), Some(Down));
        assert_eq!(palette_key("Enter", false, false, false, false), Some(Open { alt: false }));
        assert_eq!(palette_key("Enter", false, true, false, false), Some(Open { alt: true }));
        assert_eq!(palette_key("Enter", false, true, true, false), Some(Open { alt: false }), "Alt must be the only modifier");
        assert_eq!(palette_key("Enter", false, false, true, false), Some(Open { alt: false }));
        assert_eq!(palette_key("Enter", true, false, false, false), Some(Open { alt: false }));
        assert_eq!(palette_key("Escape", false, false, false, false), Some(Close));
        assert_eq!(palette_key("Escape", true, true, true, false), Some(Close));
        // Arrows with a modifier and plain letters belong to the field.
        assert_eq!(palette_key("Up", false, false, true, false), None);
        assert_eq!(palette_key("P", false, false, false, false), None);
        assert_eq!(palette_key("N", false, false, true, false), None);
        assert_eq!(palette_key("A", true, false, false, false), None);

        assert_eq!(wrap_step(Some(4), 5, true), Some(0));
        assert_eq!(wrap_step(Some(0), 5, false), Some(4));
        assert_eq!(wrap_step(Some(1), 5, true), Some(2));
        assert_eq!(wrap_step(Some(3), 5, false), Some(2));
        assert_eq!(wrap_step(None, 5, true), Some(0));
        assert_eq!(wrap_step(None, 5, false), Some(0));
        assert_eq!(wrap_step(None, 0, true), None);
        assert_eq!(wrap_step(Some(2), 0, true), None);
    }

    #[test]
    fn palette_height_follows_iterm2() {
        // Field margins 12 + 26 + 9 = 47; rows 44 with no spacing (WC:91); 10 below the last.
        assert_eq!(window_height(0, 1080.0), 47.0);
        assert_eq!(window_height(3, 1080.0), 47.0 + 3.0 * 44.0 + 10.0);
        let cap = ((1080.0 - 340.0 - 47.0) / 44.0f32).floor();
        assert_eq!(window_height(1000, 1080.0), 47.0 + cap * 44.0 + 10.0);
        assert_eq!(window_height(cap as usize, 1080.0), window_height(1000, 1080.0));
        // A screen too short for one row shows the field alone.
        assert_eq!(window_height(5, 380.0), 47.0);
    }

    #[test]
    fn preview_follows_iterm2() {
        let work = rect(0.0, 0.0, 1920.0, 1040.0);
        let parent = rect(716.0, 170.0, 488.0, 300.0);
        // Right of the palette, 8 away, 400 wide, tops aligned; 240 tall for no image.
        assert_eq!(preview_rect(parent, work, None), rect(716.0 + 488.0 + 8.0, 170.0, 400.0, 12.0 + 42.0 + 240.0 + 12.0));
        // An image sets the height by its aspect over 376.
        let r = preview_rect(parent, work, Some((800, 400)));
        assert_eq!(r.h, 12.0 + 42.0 + 188.0 + 12.0);
        // At least 200; at most max(work height - 24, 300).
        assert_eq!(preview_rect(parent, work, Some((4000, 100))).h, 200.0);
        assert_eq!(preview_rect(parent, work, Some((100, 4000))).h, 1040.0 - 24.0);
        let short = rect(0.0, 0.0, 1920.0, 200.0);
        assert_eq!(preview_rect(rect(716.0, 0.0, 488.0, 47.0), short, Some((100, 4000))).h, 300.0);
        // No room on the right: the left, 8 away.
        let right = rect(1400.0, 170.0, 488.0, 300.0);
        assert_eq!(preview_rect(right, work, None).x, 1400.0 - 8.0 - 400.0);
        // No room on either side: pinned to the work area's right edge.
        let narrow = rect(0.0, 0.0, 900.0, 1040.0);
        assert_eq!(preview_rect(rect(206.0, 170.0, 488.0, 300.0), narrow, None).x, 900.0 - 400.0);
        // A monitor to the right of another: work areas need not start at 0.
        let second = rect(1920.0, 0.0, 1920.0, 1040.0);
        assert_eq!(preview_rect(rect(1920.0 + 716.0, 170.0, 488.0, 300.0), second, None).x, 1920.0 + 716.0 + 488.0 + 8.0);
        // Kept on the work area vertically.
        let low = preview_rect(rect(716.0, 900.0, 488.0, 47.0), work, None);
        assert_eq!(low.y, 1040.0 - low.h);
    }

    #[test]
    fn window_features_follow_iterm2() {
        assert_eq!(window_features(1, Some("DISPLAY1"), false, false, None), Vec::<String>::new());
        assert_eq!(window_features(2, Some("DISPLAY2"), true, true, Some("Hotkey Alt+Space".into())), vec!["On DISPLAY2", "Miniaturized", "Full screen", "Hotkey Alt+Space"]);
        assert_eq!(window_features(2, None, false, false, None), vec!["Offscreen"]);
        assert_eq!(hotkey_feature(Some("Win+F12".into()), Some(2)), Some("Hotkey Win+F12".into()));
        assert_eq!(hotkey_feature(None, Some(2)), Some("Hotkey AltAlt".into()));
        assert_eq!(hotkey_feature(None, Some(0)), Some("Hotkey CtrlCtrl".into()));
        assert_eq!(hotkey_feature(None, None), None);
    }

    #[test]
    fn preview_detail_names_the_tab_and_window() {
        assert_eq!(preview_detail(3, ""), "Tab 3");
        assert_eq!(preview_detail(1, "vim"), "Tab 1 \u{00B7} vim");
    }

    #[test]
    fn bold_char_indexes_map_to_byte_ranges() {
        assert_eq!(bold_runs("abc", &[1..2]), vec![(0..1, false), (1..2, true), (2..3, false)]);
        assert_eq!(bold_runs("abc", &[]), vec![(0..3, false)]);
        assert_eq!(bold_runs("", &[]), Vec::<(Range<usize>, bool)>::new());
        // Multi-byte chars: the char index 1 is the second char, 3 bytes in.
        let s = "\u{4e2d}\u{6587}x";
        assert_eq!(bold_runs(s, &[1..2]), vec![(0..3, false), (3..6, true), (6..7, false)]);
        // Adjacent and overlapping ranges merge; ranges past the end are clipped.
        assert_eq!(bold_runs("abcd", &[0..1, 1..2, 1..3, 9..12]), vec![(0..3, true), (3..4, false)]);
    }
}
