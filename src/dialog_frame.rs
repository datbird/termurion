//! Alerts and small dialogs draw their own frame on Linux. winit 0.30 ignores enabled buttons
//! there, and GNOME's frame on Wayland (sctk-adwaita) always has minimize and maximize, so such a
//! window has no window-manager frame: it draws a title row with the window's name and a close
//! button, a press on the row moves it, and its height follows what it shows. Windows and macOS
//! keep their own frames: there `show` draws only the panel the dialog always had.

/// Whether dialogs draw their own frame on this OS.
pub const OWN_FRAME: bool = crate::platform::window::DIALOGS_DRAW_OWN_FRAME;

/// Whether a dialog's window takes its content's height (Linux and macOS); macOS keeps the system's
/// title bar and fits only the content under it.
pub const FIT: bool = crate::platform::window::DIALOGS_FIT_CONTENT;

/// The panel's margin on each side; the window adds it around the content.
pub const MARGIN: f32 = 8.0;

/// The tallest a dialog's body may be, in points (a scrolling text stops growing there).
pub const BODY_CAP: f32 = 480.0;

/// A dialog's window: on Linux no frame; on Linux and macOS hidden until measured; on Windows as
/// any small window.
pub fn attributes(title: &str, w: f64, h: f64) -> winit::window::WindowAttributes {
    let attrs = crate::platform::window::attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(w, h));
    // Hidden until its first frame has measured it (X11; Wayland maps a window with its first
    // drawn frame anyway), and resizable until then, so its first commit names no size (GNOME
    // kept a fixed creation size and sent it back after a later fit). `paint_dialog` fixes the
    // size once measured.
    let attrs = if OWN_FRAME { attrs.with_decorations(false) } else { attrs };
    attrs.with_visible(!FIT)
}

/// The height to ask the window for after a frame whose content needs `content`, or None to keep
/// `last`: never under `floor` (a dialog whose drop-down list needs room below it), and no ask
/// for less than a point of change.
pub fn next_height(content: f32, last: f32, floor: f32) -> Option<f32> {
    let want = content.ceil().max(floor);
    (content > 0.0 && (want - last).abs() >= 1.0).then_some(want)
}

/// How a fitted dialog's window follows its content, frame by frame (Linux).
///
/// The first frame is laid out and measured but not shown: the window is asked for the measured
/// height before anything maps, so no size change races the compositor's first configures (on
/// GNOME 50.1 a change made just after the window mapped was answered with a configure back to
/// the creation size). After that, a fit asks again when the content changes, and when the
/// compositor answers with another height than the one asked, it asks once or twice more.
#[derive(Debug, Clone, PartialEq)]
pub struct DialogFit {
    /// The height asked for last, in points.
    pub fit: f32,
    /// The least height (room for a drop-down list).
    pub floor: f32,
    shown: bool,
    retries: u8,
}

/// What one frame of a fitted dialog does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitStep {
    /// Draw this frame to the screen (false only for the first, measuring frame).
    pub present: bool,
    /// Ask the window for this height (min, max and size).
    pub ask: Option<f32>,
}

/// How often a fit is asked again when the compositor answers with another height.
const FIT_RETRIES: u8 = 2;

impl DialogFit {
    pub fn new(start: f32) -> DialogFit {
        DialogFit { fit: start, floor: 0.0, shown: false, retries: 0 }
    }

    /// Whether the first, measuring frame is done (the dialog draws to the screen from now on).
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The step for a frame whose content needs `content` points, in a window `actual` points tall;
    /// `answered` is true when the window system sized the window since the last frame.
    pub fn step(&mut self, content: f32, actual: f32, answered: bool) -> FitStep {
        let present = std::mem::replace(&mut self.shown, true);
        if let Some(want) = next_height(content, self.fit, self.floor) {
            self.fit = want;
            self.retries = 0;
            return FitStep { present, ask: Some(want) };
        }
        // The window system answered with another height (GNOME's configure right after mapping,
        // a late X11 window manager): ask again, a bounded number of times. A frame that only
        // reads the old height before any answer came waits and spends nothing.
        if present && answered && (actual - self.fit).abs() >= 1.0 && self.retries < FIT_RETRIES {
            self.retries += 1;
            return FitStep { present, ask: Some(self.fit) };
        }
        FitStep { present, ask: None }
    }
}

/// What `paint_dialog` does to the window after a frame, in this order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WinOp {
    /// Fix the size (not resizable from now on).
    Fix,
    /// Min, max and size at this height.
    Size(f32),
    /// Show the window and ask for the first drawn frame.
    Show,
}

impl FitStep {
    /// The window changes for this step: after the measuring frame the size is fixed and set before
    /// the window shows; later, only an ask sets it.
    pub fn ops(&self, fit: f32) -> Vec<WinOp> {
        let first = !self.present;
        let mut ops = Vec::new();
        if first {
            ops.push(WinOp::Fix);
        }
        if first || self.ask.is_some() {
            ops.push(WinOp::Size(self.ask.unwrap_or(fit)));
        }
        if first {
            ops.push(WinOp::Show);
        }
        ops
    }
}

/// What a frame of the dialog asked for.
#[derive(Default, Debug, PartialEq)]
pub struct Framed {
    /// The close button: the window closes as if the window manager had closed it.
    pub close: bool,
    /// A press on the title row: the window follows the pointer.
    pub drag: bool,
    /// The height the window needs, in points (the content and the margins).
    pub height: f32,
}

/// Draws the dialog's panel and `body` in it; with `own_frame`, the title row first and a thin
/// edge around it all.
pub fn show(ui: &mut egui::Ui, title: &str, own_frame: bool, fit: bool, body: &mut dyn FnMut(&mut egui::Ui)) -> Framed {
    let mut out = Framed::default();
    let frame = if own_frame { egui::Frame::central_panel(ui.style()).inner_margin(MARGIN).stroke(ui.visuals().window_stroke) } else { egui::Frame::central_panel(ui.style()) };
    let margins = frame.total_margin().sum().y;
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        if !own_frame && !fit {
            // Windows: the panel the dialog always had, nothing measured.
            body(ui);
            return;
        }
        // Measured in its own column: the panel's ui is as tall as the window, whatever it holds.
        let column = ui.vertical(|ui| {
            // macOS: the system's title bar is above the content, so only the body is drawn here.
            if own_frame {
                ui.horizontal(|ui| {
                    // One row tall (the row's available height is the whole panel's). The close
                    // button's room is kept first, so a long name is cut short with an ellipsis.
                    let row_h = ui.spacing().interact_size.y;
                    let close_w = row_h + ui.spacing().item_spacing.x;
                    let name_w = (ui.available_width() - close_w).max(0.0);
                    ui.allocate_ui_with_layout(egui::vec2(name_w, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let name = ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate().sense(egui::Sense::click_and_drag()));
                        // Only the press starts a move: during the move the window manager has the
                        // pointer, and egui never hears the release.
                        out.drag = name.drag_started_by(egui::PointerButton::Primary);
                        // The rest of the name's room moves the window too.
                        let rest = ui.allocate_response(egui::vec2(ui.available_width(), row_h), egui::Sense::drag());
                        out.drag |= rest.drag_started_by(egui::PointerButton::Primary);
                    });
                    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), row_h), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        out.close = ui.add(egui::Button::new("\u{2715}").frame(false)).on_hover_text("Close").clicked();
                    });
                });
                ui.separator();
            }
            // The body lays out in a fixed budget, never in the window's height: what it draws
            // then does not depend on the last fit, so the fit settles in one step.
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), BODY_CAP), egui::Layout::top_down(egui::Align::Min), |ui| body(ui));
        });
        // The margins and the edge, top and bottom.
        out.height = column.response.rect.height() + margins;
    });
    out
}

type CloseSink = Box<dyn Fn(winit::window::WindowId) + Send + Sync>;
static CLOSE: std::sync::OnceLock<CloseSink> = std::sync::OnceLock::new();

/// Where a dialog's close button goes: the app hands the window a close request, as the window
/// manager's would be (so each dialog closes the way its own close handling says).
pub fn set_close_sink(sink: CloseSink) {
    let _ = CLOSE.set(sink);
}

pub fn request_close(id: winit::window::WindowId) {
    if let Some(sink) = CLOSE.get() {
        sink(id);
    }
}

/// Runs `frames` frames of a dialog as `paint_dialog` and the window system do: each frame's
/// window is as tall as the last frame asked for. The heights, frame by frame.
#[cfg(test)]
pub(crate) fn settle(width: f32, start: f32, frames: usize, body: &mut dyn FnMut(&mut egui::Ui)) -> Vec<f32> {
    settle_with(width, start, 0.0, frames, &mut |_| Vec::new(), body).0
}

/// `settle` for a dialog with a least height `floor`, with `events(frame)` as each frame's input.
/// Also gives back the last frame's output.
#[cfg(test)]
pub(crate) fn settle_with(width: f32, start: f32, floor: f32, frames: usize, events: &mut dyn FnMut(usize) -> Vec<egui::Event>, body: &mut dyn FnMut(&mut egui::Ui)) -> (Vec<f32>, egui::FullOutput) {
    let ctx = egui::Context::default();
    let mut h = start;
    let mut seen = Vec::new();
    let mut last = egui::FullOutput::default();
    for frame in 0..frames {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, h))), events: events(frame), ..Default::default() };
        let mut next = h;
        let mut out = ctx.run_ui(input, |ui| {
            // As the dialog is drawn here: its own title row on Linux, the body alone on macOS.
            let f = show(ui, "Termurion", OWN_FRAME, true, body);
            next = next_height(f.height, h, floor).unwrap_or(h);
        });
        out.textures_delta.clear();
        h = next;
        seen.push(h);
        last = out;
    }
    (seen, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor keeps the window from shrinking under it (room for a drop-down list); a change of
    /// under a point asks for nothing.
    #[test]
    fn the_next_height_keeps_the_floor() {
        assert_eq!(next_height(120.0, 200.0, 200.0), None, "at the floor already");
        assert_eq!(next_height(120.0, 300.0, 200.0), Some(200.0));
        assert_eq!(next_height(250.4, 200.0, 200.0), Some(251.0));
        assert_eq!(next_height(250.4, 251.0, 0.0), None);
        assert_eq!(next_height(0.0, 100.0, 0.0), None, "nothing measured yet");
    }

    /// A dialog with a drop-down list (Locale) keeps room for it: clicked open, the list shows
    /// about nine rows inside the window.
    #[test]
    fn a_dialog_with_a_list_shows_about_nine_rows() {
        let combo = std::cell::Cell::new(egui::Rect::NOTHING);
        let mut sel = 0;
        let (hs, out) = settle_with(480.0, 200.0, 200.0, 40, &mut |frame| {
            let at = combo.get().center();
            match frame {
                20 => vec![egui::Event::PointerMoved(at), egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
                21 => vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }],
                _ => Vec::new(),
            }
        }, &mut |ui| {
            ui.label("Choose the locale Termurion uses for this.");
            combo.set(egui::ComboBox::from_id_salt("locale_pick").width(ui.available_width() - 8.0).selected_text(format!("item {sel}")).show_ui(ui, |ui| {
                for i in 0..40 {
                    ui.selectable_value(&mut sel, i, format!("item {i}"));
                }
            }).response.rect);
            ui.horizontal(|ui| {
                let _ = ui.button("Cancel");
                let _ = ui.button("OK");
            });
        });
        let h = *hs.last().unwrap();
        // Each "item" text drawn whole inside its clip and the window (the combo's own label too).
        let rows = out.shapes.iter().filter(|c| match &c.shape {
            egui::Shape::Text(t) => t.galley.job.text.starts_with("item ") && c.clip_rect.contains(t.pos + egui::vec2(1.0, t.galley.size().y - 1.0)) && t.pos.y + t.galley.size().y <= h,
            _ => false,
        }).count();
        assert!(rows >= 9, "{rows} rows show in a {h} pt window: {hs:?}");
    }

    /// The first frame measures and asks, unseen; a compositor that answers with its own height is
    /// asked again a bounded number of times; a content change asks once.
    #[test]
    fn a_dialog_is_sized_before_it_shows_and_holds_its_fit() {
        let mut f = DialogFit::new(130.0);
        assert_eq!(f.step(90.0, 130.0, false), FitStep { present: false, ask: Some(90.0) }, "measured, asked, not drawn");
        assert_eq!(f.step(90.0, 90.0, true), FitStep { present: true, ask: None }, "drawn at its height");
        // GNOME answered with the creation size: ask again, twice at most.
        assert_eq!(f.step(90.0, 130.0, true), FitStep { present: true, ask: Some(90.0) });
        assert_eq!(f.step(90.0, 130.0, true), FitStep { present: true, ask: Some(90.0) });
        assert_eq!(f.step(90.0, 130.0, true), FitStep { present: true, ask: None }, "no endless asking");
        // New content (the wrong-password line) asks once for the new height.
        assert_eq!(f.step(108.0, 130.0, false), FitStep { present: true, ask: Some(108.0) });
        assert_eq!(f.step(108.0, 108.0, true), FitStep { present: true, ask: None });
        // A floor holds from the first frame.
        let mut g = DialogFit::new(150.0);
        g.floor = 200.0;
        assert_eq!(g.step(120.0, 150.0, false).ask, Some(200.0));
    }

    /// A frame that still reads the old height before the window system answered spends no retry,
    /// so a late wrong answer (an X11 window manager's ConfigureNotify) still gets its asks.
    #[test]
    fn only_a_real_answer_spends_a_retry() {
        let mut f = DialogFit::new(130.0);
        assert_eq!(f.step(105.0, 130.0, false).ask, Some(105.0));
        // Frames before the answer: the old height, nothing asked, nothing spent.
        for _ in 0..5 {
            assert_eq!(f.step(105.0, 130.0, false), FitStep { present: true, ask: None });
        }
        // The answer is the wrong height, late: asked again, twice, then no more.
        assert_eq!(f.step(105.0, 130.0, true).ask, Some(105.0));
        assert_eq!(f.step(105.0, 130.0, false).ask, None, "waits for the next answer");
        assert_eq!(f.step(105.0, 130.0, true).ask, Some(105.0));
        assert_eq!(f.step(105.0, 130.0, true).ask, None, "bounded");
        // A right answer asks nothing.
        let mut g = DialogFit::new(130.0);
        g.step(105.0, 130.0, false);
        assert_eq!(g.step(105.0, 105.0, true).ask, None);
    }

    /// The measuring frame is never drawn, and the window shows only after its size is fixed and
    /// set; later frames change the window only to ask.
    #[test]
    fn the_window_shows_after_its_size_is_set() {
        let mut f = DialogFit::new(130.0);
        assert!(!f.shown());
        let first = f.step(105.0, 130.0, false);
        assert!(!first.present, "the measuring frame is not drawn");
        assert_eq!(first.ops(f.fit), vec![WinOp::Fix, WinOp::Size(105.0), WinOp::Show]);
        assert!(f.shown());
        let next = f.step(105.0, 105.0, true);
        assert!(next.present);
        assert!(next.ops(f.fit).is_empty());
        assert_eq!(f.step(120.0, 105.0, false).ops(f.fit), vec![WinOp::Size(120.0)]);
        // An empty body still fixes, sizes and shows (no next_height change).
        let mut e = DialogFit::new(105.0);
        assert_eq!(e.step(105.0, 105.0, false).ops(e.fit), vec![WinOp::Fix, WinOp::Size(105.0), WinOp::Show]);
    }

    /// A title wider than the window still leaves the close button inside it.
    #[test]
    fn a_long_title_keeps_the_close_button_in_the_window() {
        let ctx = egui::Context::default();
        let title = "A Python API script named something long enough to fill the whole title row and then some more";
        let mut close_x = f32::MAX;
        for _ in 0..2 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 200.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                show(ui, title, true, true, &mut |ui| {
                    ui.label("x");
                });
            });
            out.textures_delta.clear();
            close_x = out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "\u{2715}" => Some(t.pos.x + t.galley.size().x),
                _ => None,
            }).unwrap_or(f32::MAX);
        }
        assert!(close_x <= 420.0, "the close glyph ends at {close_x}, past the 420 pt window");
    }

    /// The fitted height leaves the same room under the content as over it: the margin and the
    /// edge on both sides.
    #[test]
    fn the_fit_counts_the_edge_on_both_sides() {
        let mut bottom = 0.0;
        let hs = settle(360.0, 300.0, 6, &mut |ui| {
            bottom = ui.label("last line").rect.max.y;
        });
        let h = *hs.last().unwrap();
        assert!((h - bottom - (MARGIN + 1.0)).abs() <= 1.0, "window {h}, content ends at {bottom}");
    }

    fn texts(out: &egui::FullOutput) -> Vec<String> {
        out.shapes.iter().filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        }).collect()
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 400.0))), events, ..Default::default() }
    }

    /// The own frame is a title row with the name and one button, Close, over the body; without
    /// it only the body shows.
    #[test]
    fn the_own_frame_has_a_title_row_with_close() {
        let ctx = egui::Context::default();
        for own in [true, false] {
            let mut shown = Vec::new();
            for _ in 0..2 {
                let mut out = ctx.run_ui(input(Vec::new()), |ui| {
                    show(ui, "Check for updates automatically?", own, own, &mut |ui| {
                        ui.label("BODY");
                    });
                });
                out.textures_delta.clear();
                shown = texts(&out);
            }
            assert!(shown.iter().any(|t| t == "BODY"));
            assert_eq!(shown.iter().any(|t| t == "Check for updates automatically?") && shown.iter().any(|t| t == "\u{2715}"), own, "{own}: {shown:?}");
        }
    }

    /// The height is what the content needs: far less than the 400 pt window, and more with a
    /// taller body.
    #[test]
    fn the_height_follows_the_content() {
        let ctx = egui::Context::default();
        let measure = |lines: usize| {
            let mut h = 0.0;
            for _ in 0..2 {
                let mut out = ctx.run_ui(input(Vec::new()), |ui| {
                    h = show(ui, "T", true, true, &mut |ui| {
                        for i in 0..lines {
                            ui.label(format!("line {i}"));
                        }
                    })
                    .height;
                });
                out.textures_delta.clear();
            }
            h
        };
        let (short, tall) = (measure(1), measure(6));
        assert!(short > 2.0 * MARGIN && short < 120.0, "{short}");
        assert!(tall > short + 60.0, "{short} {tall}");
    }

    /// A press on the title row starts one window move; while the window manager moves the
    /// window egui never hears the release, so no later frame asks again.
    #[test]
    fn a_press_on_the_title_row_starts_one_move() {
        let ctx = egui::Context::default();
        let at = egui::pos2(20.0, 12.0);
        let mut moves = 0;
        for frame in 0..8 {
            let events = match frame {
                1 => vec![egui::Event::PointerMoved(at)],
                2 => vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
                3 => vec![egui::Event::PointerMoved(at + egui::vec2(30.0, 0.0))],
                // The window manager has the pointer now: egui hears nothing more.
                4 => vec![egui::Event::PointerGone],
                _ => Vec::new(),
            };
            let mut out = ctx.run_ui(input(events), |ui| {
                if show(ui, "T", true, true, &mut |ui| {
                    ui.label("x");
                })
                .drag
                {
                    moves += 1;
                }
            });
            out.textures_delta.clear();
        }
        assert_eq!(moves, 1, "one move for one press");
    }

    /// Every alert and small dialog goes through the one frame: opened with `open_dialog` (or made
    /// a dialog with `as_dialog`) and drawn with `paint_dialog`, no `paint` of its own.
    #[test]
    fn every_alert_and_small_dialog_uses_the_frame() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // (file, dialogs in it): the alerts and questions, the account password gate, the login
        // sheet, the AI key and engage questions, Save Review, New Clipping, Locale, the tag editor.
        for (file, n) in [("prompt_ui.rs", 2), ("pwm_ui.rs", 1), ("basic_auth_ui.rs", 1), ("ai_key_ui.rs", 1), ("ai_engage_ui.rs", 2), ("code_review_ui.rs", 1), ("clip_add_ui.rs", 1), ("app/locale_ui.rs", 1), ("bookmark_tag_ui.rs", 1)] {
            let text = std::fs::read_to_string(src.join(file)).unwrap();
            let prod = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
            let opened = prod.matches("EguiHost::open_dialog(").count() + prod.matches(".as_dialog(").count();
            assert_eq!((opened, prod.matches(".paint_dialog(").count()), (n, n), "{file}");
        }
    }

    /// Off Linux a dialog keeps its window manager's frame; on Linux it has none and a fixed size.
    #[test]
    fn only_linux_dialogs_lose_the_window_frame() {
        let a = attributes("T", 300.0, 200.0);
        assert_eq!(a.decorations, !cfg!(target_os = "linux"));
        assert!(a.resizable, "fixed only once measured (paint_dialog)");
        assert_eq!(OWN_FRAME, cfg!(target_os = "linux"));
    }

    /// macOS fits its dialogs as Linux does (K1, the Tart check: "See Tips of the Day?" kept a band
    /// under its buttons), with the system's title bar kept; Windows does not, yet.
    #[test]
    fn linux_and_macos_dialogs_fit_their_content() {
        assert_eq!(FIT, cfg!(any(target_os = "linux", target_os = "macos")));
        let a = attributes("T", 300.0, 200.0);
        assert_eq!(a.visible, !FIT, "hidden until measured");
        if cfg!(target_os = "macos") {
            assert!(a.decorations, "macOS keeps its title bar and window buttons");
        }
        let host = include_str!("egui_host.rs");
        assert!(host.contains("let fit = crate::dialog_frame::FIT && self.dialog.is_some();") && host.contains("if !fit {"), "paint_dialog fits wherever FIT");
        // Without the title row the body alone is measured: the margins and what it draws.
        let ctx = egui::Context::default();
        let mut h = 0.0;
        for _ in 0..2 {
            let mut out = ctx.run_ui(input(Vec::new()), |ui| {
                h = show(ui, "T", false, true, &mut |ui| {
                    ui.allocate_space(egui::vec2(10.0, 50.0));
                })
                .height;
            });
            out.textures_delta.clear();
        }
        let margins = egui::Frame::central_panel(&egui::Style::default()).total_margin().sum().y;
        assert!((h - (50.0 + margins)).abs() < 1.0, "{h}");
    }
}
