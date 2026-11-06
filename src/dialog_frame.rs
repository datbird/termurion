//! Alerts and small dialogs draw their own frame on Linux. winit 0.30 ignores enabled buttons
//! there, and GNOME's frame on Wayland (sctk-adwaita) always has minimize and maximize, so such a
//! window has no window-manager frame: it draws a title row with the window's name and a close
//! button, a press on the row moves it, and its height follows what it shows. Windows and macOS
//! keep their own frames: there `show` draws only the panel the dialog always had.

/// Whether dialogs draw their own frame on this OS.
pub const OWN_FRAME: bool = crate::platform::window::DIALOGS_DRAW_OWN_FRAME;

/// The panel's margin on each side; the window adds it around the content.
pub const MARGIN: f32 = 8.0;

/// A dialog's window: on Linux a fixed size and no frame; elsewhere as any small window.
pub fn attributes(title: &str, w: f64, h: f64) -> winit::window::WindowAttributes {
    let attrs = crate::platform::window::attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(w, h));
    if OWN_FRAME { attrs.with_resizable(false).with_decorations(false) } else { attrs }
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
pub fn show(ui: &mut egui::Ui, title: &str, own_frame: bool, body: &mut dyn FnMut(&mut egui::Ui)) -> Framed {
    let mut out = Framed::default();
    let frame = if own_frame { egui::Frame::central_panel(ui.style()).inner_margin(MARGIN).stroke(ui.visuals().window_stroke) } else { egui::Frame::central_panel(ui.style()) };
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        // Measured in its own column: the panel's ui is as tall as the window, whatever it holds.
        let column = ui.vertical(|ui| {
            if own_frame {
                ui.horizontal(|ui| {
                    let name = ui.add(egui::Label::new(egui::RichText::new(title).strong()).sense(egui::Sense::click_and_drag()));
                    // Only the press starts a move: during the move the window manager has the
                    // pointer, and egui never hears the release.
                    out.drag = name.drag_started_by(egui::PointerButton::Primary);
                    // One row tall (the row's available height is the whole panel's): the close
                    // button at the right, and the rest of the row moves the window too.
                    let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        out.close = ui.add(egui::Button::new("\u{2715}").frame(false)).on_hover_text("Close").clicked();
                        let rest = ui.allocate_response(egui::vec2(ui.available_width(), row.y), egui::Sense::drag());
                        out.drag |= rest.drag_started_by(egui::PointerButton::Primary);
                    });
                });
                ui.separator();
            }
            body(ui);
        });
        out.height = column.response.rect.height() + 2.0 * MARGIN;
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

#[cfg(test)]
mod tests {
    use super::*;

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
                    show(ui, "Check for updates automatically?", own, &mut |ui| {
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
                    h = show(ui, "T", true, &mut |ui| {
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
                if show(ui, "T", true, &mut |ui| {
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
        assert_eq!(a.resizable, !cfg!(target_os = "linux"));
        assert_eq!(OWN_FRAME, cfg!(target_os = "linux"));
    }
}
