//! The Cockpit window (`CW`, `CX`): an egui window over `EguiHost`, always on top and off
//! the taskbar. Unlike the Instant Replay panel it takes the focus (iTerm2's
//! `becomesKeyOnlyIfNeeded = false`), so the command bar can type. Top to bottom it holds the
//! toolbar row, the filter bar, the list, a draggable divider, the command bar and the tip
//! line; the command bar fills in with a later task.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::cockpit::{self, Act, Frame, Tint, View};
use crate::cockpit_rows::{self, Row, RowKey};
use crate::egui_host::EguiHost;

/// The divider's height in points.
const DIVIDER: f32 = 6.0;
/// A show that just took the focus is not hidden again for this long: the focus moves over a
/// few passes, and `cockpit_follows_app` would otherwise see another app in front.
const SETTLE: Duration = Duration::from_millis(600);

/// The toolbar's glyphs (Segoe Fluent Icons) for iTerm2's SF Symbols: gear, bell, and one per
/// group mode; without the font a mode shows its label.
const GEAR: char = '\u{E713}';
const BELL: char = '\u{E7ED}';
const MODE_GLYPHS: [char; 3] = ['\u{E8FD}', '\u{E737}', '\u{E716}'];
/// The toolbar row's height (`CX`: the buttons are 24 high).
const TOOLBAR_H: f32 = 24.0;

/// A toolbar button: the glyph (or `fallback` text without the icon font), `width` wide.
fn tool_button(ui: &mut egui::Ui, glyph: char, fallback: &str, width: f32, on: bool, enabled: bool, tip: &str, a11y: &str) -> egui::Response {
    let text = crate::settings_keys::icon_text(ui.ctx(), glyph).unwrap_or_else(|| egui::RichText::new(fallback));
    let button = egui::Button::new(text).selected(on).min_size(egui::vec2(width, TOOLBAR_H));
    let r = ui.add_enabled(enabled, button).on_hover_text(tip);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, a11y));
    r
}

/// The toolbar row: the gear, the Group control, the bell and the search field (`CX:111-148`).
fn toolbar(ui: &mut egui::Ui, view: &mut View, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        let gear = tool_button(ui, GEAR, "Settings", cockpit::SETTINGS_W, false, cockpit::settings_enabled(view.mode), cockpit::SETTINGS_A11Y, cockpit::SETTINGS_A11Y);
        view.gear_rect = Some((gear.rect.min.x, gear.rect.min.y, gear.rect.width(), gear.rect.height()));
        if gear.clicked() {
            acts.push(Act::Settings);
        }
        // Three toggle buttons share the Group control's width.
        let each = cockpit::GROUP_W / 3.0;
        ui.spacing_mut().item_spacing.x = 0.0;
        for (mode, glyph) in cockpit::MODES.into_iter().zip(MODE_GLYPHS) {
            let r = tool_button(ui, glyph, mode.label(), each, view.mode == mode, true, mode.tooltip(), mode.label());
            if r.clicked() && view.mode != mode {
                view.mode = mode;
                acts.push(Act::Group(mode));
            }
        }
        ui.spacing_mut().item_spacing.x = ui.style().spacing.item_spacing.x;
        let armed = view.selection_armed;
        let a11y = if armed { cockpit::NOTIFY_ARMED_A11Y } else { cockpit::NOTIFY_A11Y };
        let bell = tool_button(ui, BELL, "Notify", cockpit::NOTIFY_W, armed, cockpit::notify_enabled(view.selection_kind.as_ref()), cockpit::notify_tooltip(armed), a11y);
        if bell.clicked() {
            acts.push(Act::Notify);
        }
        let mut text = view.search.clone();
        let field = egui::TextEdit::singleline(&mut text).hint_text("Search").desired_width(f32::INFINITY);
        if ui.add(field).changed() {
            view.search = text.clone();
            acts.push(Act::Search(text));
        }
    });
}

/// How often the list is rebuilt while the window shows (a status changes with no event for it).
const REFRESH: Duration = Duration::from_millis(500);
/// The room each level of the outline indents, and a disclosure triangle's box.
const INDENT: f32 = 16.0;

fn color(t: Tint, ui: &egui::Ui) -> egui::Color32 {
    match t {
        Tint::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
        Tint::Secondary => ui.visuals().weak_text_color(),
    }
}

/// The glyph of a row's kind, if it has one.
fn kind_glyph(key: &RowKey) -> Option<char> {
    match key {
        RowKey::Window(_) => Some(cockpit::WINDOW_GLYPH),
        RowKey::Tab(_) => Some(cockpit::TAB_GLYPH),
        RowKey::Session(_) => Some(cockpit::SESSION_GLYPH),
        RowKey::BuriedRoot => Some(cockpit::BURIED_GLYPH),
        RowKey::Workgroup(_) => Some(cockpit::WORKGROUP_GLYPH),
        RowKey::Group { .. } => None,
    }
}

fn row_height(row: &Row) -> f32 {
    if matches!(row.key, RowKey::Session(_)) && row.detail.as_deref().is_some_and(|d| !cockpit::detail_spans(d).is_empty()) { cockpit::ROW_HEIGHT_DETAIL } else { cockpit::ROW_HEIGHT }
}

/// The detail line: the runs of `detail_spans` in a small font, one row, clipped.
fn detail_job(ui: &egui::Ui, detail: &str, width: f32) -> egui::text::LayoutJob {
    let small = egui::TextStyle::Small.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('\u{2026}') };
    for s in cockpit::detail_spans(detail) {
        let font = if s.code { egui::FontId::monospace(small.size) } else { small.clone() };
        let col = if s.bold { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
        let mut f = egui::TextFormat::simple(font, col);
        f.italics = s.italic;
        if s.code {
            f.background = ui.visuals().code_bg_color;
        }
        job.append(&s.text, 0.0, f);
    }
    job
}

/// One row of the outline; the acts it asks for go to `acts`.
fn draw_row(ui: &mut egui::Ui, view: &View, depth: usize, row: &Row, expanded: bool, visible: &[RowKey], acts: &mut Vec<Act>) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, row_height(row)), egui::Sense::click());
    let selected = view.selected.contains(&row.key);
    let painter = ui.painter().with_clip_rect(rect);
    if selected {
        painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
    } else if resp.hovered() && row.key.selectable() {
        painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let text = if selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
    let weak = if selected { text } else { ui.visuals().weak_text_color() };
    let top = rect.min.y + cockpit::ROW_HEIGHT / 2.0;
    let mut x = rect.min.x + 6.0 + depth as f32 * INDENT;
    // The disclosure triangle.
    let tri = egui::Rect::from_min_size(egui::pos2(x, rect.min.y), egui::vec2(INDENT, cockpit::ROW_HEIGHT));
    if !row.children.is_empty() {
        let c = tri.center();
        let pts = if expanded { vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(4.0, -2.0), c + egui::vec2(0.0, 3.0)] } else { vec![c + egui::vec2(-2.0, -4.0), c + egui::vec2(-2.0, 4.0), c + egui::vec2(3.0, 0.0)] };
        painter.add(egui::Shape::convex_polygon(pts, weak, egui::Stroke::NONE));
    }
    x += INDENT;
    let icons = crate::settings_keys::has_icon_font(ui.ctx());
    if let (Some(g), true) = (kind_glyph(&row.key), icons) {
        let font = egui::FontId::new(14.0, crate::settings_keys::icon_family());
        let r = painter.text(egui::pos2(x, top), egui::Align2::LEFT_CENTER, g.to_string(), font, weak);
        x = r.max.x + 6.0;
    }
    // The right side: the bell, then the status word (a session only).
    let mut right = rect.max.x - 8.0;
    if row.armed && icons {
        let font = egui::FontId::new(14.0, crate::settings_keys::icon_family());
        let r = painter.text(egui::pos2(right, top), egui::Align2::RIGHT_CENTER, cockpit::BELL_ARMED_GLYPH.to_string(), font, ui.visuals().hyperlink_color);
        right = r.min.x - 6.0;
    }
    if let Some(status) = &row.status {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let tint = if selected { text } else { color(row.tint, ui) };
        let r = painter.text(egui::pos2(right, top), egui::Align2::RIGHT_CENTER, crate::inert_files::shown_name(status), font, tint);
        right = r.min.x - 8.0;
    }
    let title = painter.with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(right.max(x), rect.max.y)));
    title.text(egui::pos2(x, top), egui::Align2::LEFT_CENTER, &row.title, egui::TextStyle::Body.resolve(ui.style()), text);
    if row_height(row) > cockpit::ROW_HEIGHT {
        if let Some(d) = &row.detail {
            let job = detail_job(ui, d, (rect.max.x - 8.0 - x).max(10.0));
            let galley = ui.painter().layout_job(job);
            painter.galley(egui::pos2(x, rect.min.y + cockpit::ROW_HEIGHT - 4.0), galley, weak);
        }
    }
    let on_triangle = resp.interact_pointer_pos().is_some_and(|p| tri.contains(p)) && !row.children.is_empty();
    if resp.double_clicked() && !on_triangle {
        acts.push(Act::Jump(row.key.clone()));
    } else if resp.clicked() {
        if on_triangle {
            acts.push(Act::Toggle(row.key.clone()));
        } else if row.key.selectable() {
            let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            acts.push(Act::Select(cockpit::select(visible, &view.selected, &row.key, ctrl, shift)));
        }
    }
}

/// The list: the outline from `rows`, collapsed rows hiding their children (a search shows all).
fn outline(ui: &mut egui::Ui, view: &View, rows: &[Row], acts: &mut Vec<Act>) {
    let flat = cockpit_rows::flatten(rows, &view.collapsed, !view.search.is_empty());
    let visible: Vec<RowKey> = flat.iter().map(|(_, r)| r.key.clone()).collect();
    ui.spacing_mut().item_spacing.y = 0.0;
    for (depth, row) in flat {
        let open = cockpit_rows::expanded(&row.key, &view.collapsed, !view.search.is_empty());
        draw_row(ui, view, depth, row, open, &visible, acts);
    }
}

/// The filter bar: the items as a segmented row when they fit, else a popup (`CW:362-415`).
fn filter_bar(ui: &mut egui::Ui, view: &View, items: &[(Option<String>, String)], acts: &mut Vec<Act>) {
    let width = ui.available_width();
    ui.allocate_ui(egui::vec2(width, cockpit::FILTER_BAR_HEIGHT), |ui| {
        ui.horizontal_centered(|ui| {
            let labels: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
            let font = egui::TextStyle::Button.resolve(ui.style());
            let char_px = ui.painter().layout_no_wrap("n".into(), font, egui::Color32::WHITE).size().x;
            let current = items.iter().find(|(k, _)| *k == view.status_filter).map_or("", |(_, l)| l.as_str());
            if cockpit::filter_fits(&labels, char_px, width) {
                for (key, label) in items {
                    if ui.selectable_label(*key == view.status_filter, label).clicked() && *key != view.status_filter {
                        acts.push(Act::StatusFilter(key.clone()));
                    }
                }
            } else {
                egui::ComboBox::from_id_salt("cockpit-filter").selected_text(current).width(cockpit::FILTER_POPUP_MIN.max(width * 0.4)).show_ui(ui, |ui| {
                    for (key, label) in items {
                        if ui.selectable_label(*key == view.status_filter, label).clicked() && *key != view.status_filter {
                            acts.push(Act::StatusFilter(key.clone()));
                        }
                    }
                });
            }
        });
    });
}

pub struct CockpitWindow {
    host: EguiHost,
    shown: bool,
    shown_at: Option<Instant>,
    /// The frame last reported, so a redraw reports only a real move or resize.
    reported: Frame,
    /// The command bar's text.
    command: String,
    /// When the list was last drawn: it is rebuilt every `REFRESH` while the window shows.
    painted: Instant,
}

impl CockpitWindow {
    /// A hidden window at `frame`; `show_and_focus` shows it.
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: Frame,
        dark: bool,
    ) -> Result<CockpitWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let min_h = cockpit::FILTER_BAR_HEIGHT + cockpit::LIST_MIN + cockpit::COMPOSER_MIN + 40.0;
        let attrs = Window::default_attributes()
            .with_title(cockpit::TITLE)
            .with_resizable(true)
            .with_enabled_buttons(winit::window::WindowButtons::CLOSE)
            .with_skip_taskbar(true)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            .with_visible(false)
            .with_position(winit::dpi::PhysicalPosition::new(frame.x as f64, frame.y as f64))
            .with_inner_size(winit::dpi::PhysicalSize::new(frame.w as f64, frame.h as f64))
            .with_min_inner_size(winit::dpi::LogicalSize::new(400.0, min_h as f64));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.follow_theme(Some(dark));
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(CockpitWindow { host, shown: false, shown_at: None, reported: frame, command: String::new(), painted: Instant::now() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn hwnd(&self) -> Option<isize> {
        self.host.hwnd()
    }

    /// Follows a forced appearance, or Windows' when `None`.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.host.follow_theme(forced);
    }

    /// Shows the window and takes the focus (`showAndFocusCommand`).
    pub fn show_and_focus(&mut self) {
        self.shown = true;
        self.shown_at = Some(Instant::now());
        self.host.window.set_visible(true);
        self.host.focus();
        self.host.request_redraw();
    }

    /// Hidden while no window of Tandem is in front (`hidesOnDeactivate`, `CX:26`); it comes
    /// back without taking the focus from the window Tandem returned to.
    pub fn set_shown(&mut self, on: bool) {
        if on == self.shown {
            return;
        }
        if !on && self.shown_at.is_some_and(|t| t.elapsed() < SETTLE) {
            return;
        }
        self.shown = on;
        if !on {
            return self.host.window.set_visible(false);
        }
        if let Some(h) = self.host.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
            unsafe { ShowWindow(h as _, SW_SHOWNOACTIVATE) };
        }
        self.host.request_redraw();
    }

    /// The window's frame now: the outer top left and the content size, in pixels.
    pub fn frame(&self) -> Frame {
        let p = self.host.window.outer_position().map(|p| (p.x as f32, p.y as f32)).unwrap_or((self.reported.x, self.reported.y));
        let s = self.host.window.inner_size();
        Frame { x: p.0, y: p.1, w: s.width as f32, h: s.height as f32 }
    }

    /// Where the settings popover opens: below the gear. A popover's bottom centers on its point,
    /// so the point is the popover's height under the gear's bottom edge (screen pixels).
    pub fn gear_anchor(&self, view: &View) -> Option<(f64, f64)> {
        let (x, y, w, h) = view.gear_rect?;
        let origin = self.host.window.inner_position().ok()?;
        let scale = self.host.window.scale_factor();
        Some((origin.x as f64 + f64::from(x + w / 2.0) * scale, origin.y as f64 + f64::from(y + h) * scale + crate::status_popover_ui::SETTINGS_H * scale))
    }

    /// Asks for a redraw when the list is due, and says when it is next due (None while hidden).
    pub fn refresh(&mut self, now: Instant) -> Option<Instant> {
        if !self.shown {
            return None;
        }
        let due = self.painted + REFRESH;
        if now >= due {
            self.painted = now;
            self.host.request_redraw();
            return Some(now + REFRESH);
        }
        Some(due)
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    /// Feeds a window event to egui. True when the window should redraw.
    pub fn event(&mut self, e: &WindowEvent) -> bool {
        if matches!(e, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
            self.host.request_redraw();
        }
        self.host.event(e)
    }

    /// Draws the window from `view`; returns what it asks of the app.
    pub fn redraw(&mut self, view: &mut View, rows: &[Row], items: &[(Option<String>, String)]) -> Vec<Act> {
        let mut acts = Vec::new();
        let now = Instant::now();
        self.painted = now;
        let Self { host, command, .. } = self;
        host.paint(|ui| {
            // Ctrl+W closes, as every other egui window of Tandem does.
            if ui.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.command) {
                acts.push(Act::Closed);
            }
            egui::Panel::bottom("cockpit_tip").show(ui, |ui| {
                let (text, error) = view.tip.current(now);
                let color = if error { egui::Color32::from_rgb(0xd0, 0x30, 0x30) } else { ui.visuals().weak_text_color() };
                ui.label(egui::RichText::new(text).small().color(color));
            });
            egui::CentralPanel::default().show(ui, |ui| {
                toolbar(ui, view, &mut acts);
                filter_bar(ui, view, items, &mut acts);
                let total = ui.available_height();
                let line = ui.text_style_height(&egui::TextStyle::Monospace);
                let composer = cockpit::composer_height(line);
                let list_h = cockpit::split(view.list_height, total - DIVIDER, composer);

                let width = ui.available_width();
                ui.allocate_ui(egui::vec2(width, list_h), |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| outline(ui, view, rows, &mut acts));
                });

                // The divider drags the list's height.
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, DIVIDER), egui::Sense::drag());
                let color = if resp.hovered() || resp.dragged() { ui.visuals().widgets.hovered.bg_stroke.color } else { ui.visuals().widgets.noninteractive.bg_stroke.color };
                ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, color));
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if resp.dragged() {
                    let h = cockpit::split(Some(list_h + resp.drag_delta().y), total - DIVIDER, composer);
                    if h != list_h {
                        view.list_height = Some(h);
                        acts.push(Act::Split(h));
                    }
                }

                // The command bar (Task 6 sends from it).
                let id = ui.id().with("cockpit-command");
                let size = ui.available_size();
                let field = egui::TextEdit::multiline(command).id(id).hint_text(cockpit::PLACEHOLDER).desired_width(f32::INFINITY).min_size(size);
                ui.add_sized(size, field);
                if std::mem::take(&mut view.focus_command) {
                    ui.memory_mut(|m| m.request_focus(id));
                }
            });
        });
        let frame = self.frame();
        if frame != self.reported && self.host.window.is_minimized() != Some(true) {
            self.reported = frame;
            acts.push(Act::Moved(frame));
        }
        acts
    }
}
