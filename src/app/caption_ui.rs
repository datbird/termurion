//! The caption area of Minimal and Compact windows (13e): what the strip shows and does. The Win32
//! subclass that answers the frame messages lives in `platform::caption`.

use super::*;
use crate::caption::{self, Button, Event, Layout, StripMenu};
use crate::platform::caption::{frame_thickness, install, installed, keep_client, publish, remove};

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
                install(hwnd, Box::new(move |h, e| {
                    let _ = proxy.send_event(UserEvent::Caption(h, e));
                }));
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
