//! The Cockpit window (`CW`, `CX`): an egui window over `EguiHost`, always on top and off
//! the taskbar. Unlike the Instant Replay panel it takes the focus (iTerm2's
//! `becomesKeyOnlyIfNeeded = false`), so the command bar can type. Top to bottom it holds the
//! toolbar row, the filter bar, the list, a draggable divider, the command bar and the tip
//! line; the rows, filters and command bar fill in with the later tasks.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::cockpit::{self, Act, Frame, View};
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

pub struct CockpitWindow {
    host: EguiHost,
    shown: bool,
    shown_at: Option<Instant>,
    /// The frame last reported, so a redraw reports only a real move or resize.
    reported: Frame,
    /// The command bar's text.
    command: String,
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
        Ok(CockpitWindow { host, shown: false, shown_at: None, reported: frame, command: String::new() })
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
    pub fn redraw(&mut self, view: &mut View) -> Vec<Act> {
        let mut acts = Vec::new();
        let now = Instant::now();
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
                // The filter bar comes with the filters (Task 4); it keeps its height now so
                // the split matches iTerm2's.
                ui.allocate_space(egui::vec2(ui.available_width(), cockpit::FILTER_BAR_HEIGHT));
                let total = ui.available_height();
                let line = ui.text_style_height(&egui::TextStyle::Monospace);
                let composer = cockpit::composer_height(line);
                let list_h = cockpit::split(view.list_height, total - DIVIDER, composer);

                // The list: empty until Task 2 builds the rows.
                let width = ui.available_width();
                ui.allocate_ui(egui::vec2(width, list_h), |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |_ui| {});
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
