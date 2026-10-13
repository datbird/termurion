//! The Tip of the Day card (M13v2 Task 4): iTerm2's tip window through `EguiHost`
//! (`iTermTipWindowController.m`, `iTermTipCardViewController.m`).

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;
use crate::tips::Tip;

/// What a card button does (`TW:165-325`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    LearnMore,
    Dismiss,
    ToggleOptions,
    ShowLater,
    Share,
    ToggleFrequency,
    ToggleEnabled,
    Next,
    Previous,
}

/// The card's width in logical pixels (`kWindowWidth`) and its distance from the work area's
/// top left (`kWindowLeftMargin`, `kWindowTopMargin`).
pub const WIDTH: f64 = 400.0;
pub const MARGIN: f32 = 8.0;
/// The signature line under the body (`iTermTipCardViewController.m:191`).
pub const SIGNATURE: &str = "Termurion tip of the day";
pub const TITLE: &str = "Termurion Tip of the Day";

/// The buttons, top to bottom (`addButtonsToCard:expanded:`): Learn More only with an address, the
/// options toggle third, and the collapsible ones after it only while expanded. The next and
/// previous tips are left out when there is none.
pub fn card_buttons(has_url: bool, expanded: bool, frequency_high: bool, disabled: bool, has_next: bool, has_prev: bool) -> Vec<(&'static str, Action)> {
    let mut out = Vec::new();
    if has_url {
        out.push(("Learn More", Action::LearnMore));
    }
    out.push(("Dismiss Tip", Action::Dismiss));
    out.push((if expanded { "Fewer Options" } else { "More Options" }, Action::ToggleOptions));
    if expanded {
        out.push(("Show This Later", Action::ShowLater));
        out.push(("Share", Action::Share));
        out.push((if frequency_high { "Show Tips Weekly" } else { "Show Tips Daily" }, Action::ToggleFrequency));
        out.push((if disabled { "Enable Tips" } else { "Disable Tips" }, Action::ToggleEnabled));
        if has_next {
            out.push(("Show Next Tip", Action::Next));
        }
        if has_prev {
            out.push(("Show Previous Tip", Action::Previous));
        }
    }
    out
}

/// A work area as (x, y, width, height) in screen pixels.
pub type Work = (f32, f32, f32, f32);

/// The card's top left: the work area's top left, 8 in, plus the saved offset.
pub fn window_origin(work: Work, offset: (f32, f32)) -> (f32, f32) {
    (work.0 + MARGIN + offset.0, work.1 + MARGIN + offset.1)
}

/// The offset a card at `origin` is saved as (`saveWindowPosition`): the inverse of `window_origin`.
pub fn offset_of(work: Work, origin: (f32, f32)) -> (f32, f32) {
    (origin.0 - work.0 - MARGIN, origin.1 - work.1 - MARGIN)
}

/// `NSStringFromSize`: "{w, h}".
pub fn offset_string(offset: (f32, f32)) -> String {
    format!("{{{}, {}}}", offset.0, offset.1)
}

/// `NSSizeFromString`, exactly two finite numbers; anything else is None (stored text is read
/// defensively).
pub fn parse_offset(text: &str) -> Option<(f32, f32)> {
    let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut parts = inner.split(',');
    let w: f32 = parts.next()?.trim().parse().ok()?;
    let h: f32 = parts.next()?.trim().parse().ok()?;
    (parts.next().is_none() && w.is_finite() && h.is_finite()).then_some((w, h))
}

/// Keeps a card of `size` on the work area (its top left wins when it is too small), so a hand
/// edited offset cannot put it off screen.
pub fn clamp_origin(origin: (f32, f32), size: (f32, f32), work: Work) -> (f32, f32) {
    let x = origin.0.min(work.0 + work.2 - size.0).max(work.0);
    let y = origin.1.min(work.1 + work.3 - size.1).max(work.1);
    (x, y)
}

/// What Share copies: "Title\n\nBody", then "\n\nURL" when the tip has one.
pub fn share_text(tip: &Tip) -> String {
    let mut text = format!("{}\n\n{}", tip.title, tip.body);
    if let Some(url) = &tip.url {
        text.push_str("\n\n");
        text.push_str(url);
    }
    text
}

/// The card: always on top, no taskbar button, never takes the focus when it opens.
pub struct TipWindow {
    host: EguiHost,
    tip: Tip,
    buttons: Vec<(&'static str, Action)>,
    expanded: bool,
    work: Work,
    action: Option<Action>,
    /// Height the content asked for, in logical pixels; the window follows it.
    fit: f32,
    /// A drag is running (`drag_window` was called and the pointer has not come back).
    dragging: bool,
    last_pos: Option<(f32, f32)>,
    moved: Option<(f32, f32)>,
}

impl TipWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tip: &Tip,
        buttons: Vec<(&'static str, Action)>,
        work: Work,
        offset: (f32, f32),
    ) -> Result<TipWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let start_h = 220.0_f64;
        let attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_decorations(false)
            .with_resizable(false)
            .with_skip_taskbar(true)
            .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
            // A tip never takes the focus from what the person is typing into.
            .with_active(false)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(WIDTH, start_h));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        let size = (WIDTH as f32 * host.window.scale_factor() as f32, start_h as f32 * host.window.scale_factor() as f32);
        let (x, y) = clamp_origin(window_origin(work, offset), size, work);
        host.window.set_outer_position(PhysicalPosition::new(x as f64, y as f64));
        host.window.set_visible(true);
        host.request_redraw();
        Ok(TipWindow { host, tip: tip.clone(), buttons, expanded: false, work, action: None, fit: start_h as f32, dragging: false, last_pos: None, moved: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Shows another tip (Next, Previous) with the buttons for it.
    pub fn set_tip(&mut self, tip: &Tip, buttons: Vec<(&'static str, Action)>) {
        self.tip = tip.clone();
        self.set_buttons(buttons);
    }

    pub fn set_buttons(&mut self, buttons: Vec<(&'static str, Action)>) {
        self.buttons = buttons;
        self.action = None;
        self.host.request_redraw();
    }

    pub fn tip(&self) -> &Tip {
        &self.tip
    }

    pub fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }

    /// Where the card was dropped, as the offset to save (`saveWindowPosition`), once a drag ends.
    pub fn moved_to(&mut self) -> Option<(f32, f32)> {
        self.moved.take()
    }

    /// Feeds a window event; Esc and the close box answer Dismiss.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => {
                self.action = Some(Action::Dismiss);
                return true;
            }
            WindowEvent::Moved(p) if self.dragging => self.last_pos = Some((p.x as f32, p.y as f32)),
            // The pointer is back after the move loop: the drag is over.
            WindowEvent::CursorMoved { .. } | WindowEvent::CursorEntered { .. } | WindowEvent::MouseInput { state: ElementState::Released, .. } if self.dragging => {
                if let Some(p) = self.last_pos.take() {
                    self.moved = Some(offset_of(self.work, p));
                    self.dragging = false;
                }
            }
            _ => {}
        }
        self.host.event(event)
    }

    pub fn redraw(&mut self) {
        let tip = self.tip.clone();
        let buttons = self.buttons.clone();
        let mut picked = None;
        let mut content_h = 0.0f32;
        let mut drag = false;
        self.host.paint(|ui| {
            let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill).stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(14);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                // The whole card drags, as iTerm2's does; the buttons sit on top of it.
                let rect = ui.max_rect();
                let back = ui.interact(rect, ui.id().with("drag"), egui::Sense::click_and_drag());
                if back.drag_started() {
                    drag = true;
                }
                ui.label(egui::RichText::new(tip.title.as_str()).strong().size(20.0));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(tip.body.as_str()).size(15.0));
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(egui::RichText::new(SIGNATURE).italics().small().weak());
                });
                ui.add_space(8.0);
                for (label, action) in &buttons {
                    if ui.add_sized([ui.available_width(), 26.0], egui::Button::new(*label)).clicked() {
                        picked = Some(action.clone());
                    }
                }
                content_h = ui.min_rect().height();
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                picked = Some(Action::Dismiss);
            }
        });
        if drag {
            self.dragging = true;
            self.last_pos = None;
            let _ = self.host.window.drag_window();
        }
        if picked.is_some() {
            self.action = picked;
        }
        // The window follows its content (the card grows with More Options and longer tips).
        let want = (content_h + 28.0).ceil();
        if content_h > 0.0 && (want - self.fit).abs() >= 1.0 {
            self.fit = want;
            let _ = self.host.window.request_inner_size(LogicalSize::new(WIDTH, want as f64));
            let s = self.host.window.scale_factor() as f32;
            if let Ok(p) = self.host.window.outer_position() {
                let (x, y) = clamp_origin((p.x as f32, p.y as f32), (WIDTH as f32 * s, want * s as f32), self.work);
                self.host.window.set_outer_position(PhysicalPosition::new(x as f64, y as f64));
            }
            self.host.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(v: &[(&'static str, Action)]) -> Vec<&'static str> {
        v.iter().map(|(l, _)| *l).collect()
    }

    #[test]
    fn card_buttons_follow_iterm2() {
        use Action::*;
        let collapsed = card_buttons(true, false, true, false, true, true);
        assert_eq!(collapsed, vec![("Learn More", LearnMore), ("Dismiss Tip", Dismiss), ("More Options", ToggleOptions)]);
        let open = card_buttons(true, true, true, false, true, true);
        assert_eq!(
            open,
            vec![
                ("Learn More", LearnMore),
                ("Dismiss Tip", Dismiss),
                ("Fewer Options", ToggleOptions),
                ("Show This Later", ShowLater),
                ("Share", Share),
                ("Show Tips Weekly", ToggleFrequency),
                ("Disable Tips", ToggleEnabled),
                ("Show Next Tip", Next),
                ("Show Previous Tip", Previous),
            ]
        );
        assert!(!labels(&card_buttons(false, true, true, false, true, true)).contains(&"Learn More"));
        assert!(labels(&card_buttons(true, true, false, false, true, true)).contains(&"Show Tips Daily"));
        assert!(labels(&card_buttons(true, true, true, true, true, true)).contains(&"Enable Tips"));
        let no_next = labels(&card_buttons(true, true, true, false, false, true));
        assert!(!no_next.contains(&"Show Next Tip") && no_next.contains(&"Show Previous Tip"));
        let no_prev = labels(&card_buttons(true, true, true, false, true, false));
        assert!(no_prev.contains(&"Show Next Tip") && !no_prev.contains(&"Show Previous Tip"));
    }

    #[test]
    fn window_origin_adds_the_margin_and_the_offset() {
        assert_eq!(window_origin((0.0, 0.0, 1920.0, 1040.0), (0.0, 0.0)), (8.0, 8.0));
        assert_eq!(window_origin((0.0, 0.0, 1920.0, 1040.0), (100.0, 50.0)), (108.0, 58.0));
        assert_eq!(window_origin((1920.0, 30.0, 1920.0, 1040.0), (0.0, 0.0)), (1928.0, 38.0));
    }

    #[test]
    fn offsets_are_stored_as_iterm2_writes_them() {
        assert_eq!(offset_string((100.0, 50.0)), "{100, 50}");
        assert_eq!(parse_offset("{100, 50}"), Some((100.0, 50.0)));
        assert_eq!(parse_offset("{-4.5, 0}"), Some((-4.5, 0.0)));
        assert_eq!(parse_offset("garbage"), None);
        assert_eq!(parse_offset("{nan, 1}"), None);
        assert_eq!(parse_offset("{1, 2, 3}"), None);
    }

    #[test]
    fn the_card_stays_on_the_work_area() {
        let work = (0.0, 0.0, 1920.0, 1040.0);
        assert_eq!(clamp_origin((108.0, 58.0), (400.0, 300.0), work), (108.0, 58.0));
        assert_eq!(clamp_origin((5000.0, 5000.0), (400.0, 300.0), work), (1520.0, 740.0));
        assert_eq!(clamp_origin((-900.0, -900.0), (400.0, 300.0), work), (0.0, 0.0));
        // Bigger than the area: the top left wins.
        assert_eq!(clamp_origin((50.0, 50.0), (400.0, 300.0), (10.0, 20.0, 200.0, 100.0)), (10.0, 20.0));
    }

    #[test]
    fn share_text_is_title_body_and_address() {
        let tip = crate::tips::Tip { key: "1".into(), title: "T".into(), body: "B".into(), url: None };
        assert_eq!(share_text(&tip), "T\n\nB");
        let with = crate::tips::Tip { url: Some("https://github.com/datbird/termurion/x".into()), ..tip };
        assert_eq!(share_text(&with), "T\n\nB\n\nhttps://github.com/datbird/termurion/x");
    }
}
