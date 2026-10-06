//! The Software Update window (M13v2 Task 2): Sparkle's informational alert, through `EguiHost`.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::update::{self, Release};

/// What the window shows (`UpToDate`'s bool: running a newer version than the newest release).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Checking,
    Found(Release),
    UpToDate(String, bool),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Skip,
    Later,
    LearnMore,
    Ok,
    Cancel,
}

/// The buttons, left to right.
pub fn buttons(view: &View) -> Vec<&'static str> {
    match view {
        View::Checking => vec!["Cancel"],
        View::Found(_) => vec![update::SKIP, update::REMIND, update::LEARN_MORE],
        View::UpToDate(..) | View::Error(_) => vec!["OK"],
    }
}

fn choice_of(view: &View, label: &str) -> Choice {
    match (view, label) {
        (_, "Cancel") => Choice::Cancel,
        (_, update::SKIP) => Choice::Skip,
        (_, update::REMIND) => Choice::Later,
        (_, update::LEARN_MORE) => Choice::LearnMore,
        _ => Choice::Ok,
    }
}

/// What the close box and Escape answer.
pub fn close_choice(view: &View) -> Choice {
    match view {
        View::Checking => Choice::Cancel,
        View::Found(_) => Choice::Later,
        _ => Choice::Ok,
    }
}

/// What Enter answers: the main button.
pub fn enter_choice(view: &View) -> Choice {
    match view {
        View::Found(_) => Choice::LearnMore,
        other => close_choice(other),
    }
}

/// The heading and the text under it.
pub fn heading_and_text(view: &View, current: &str) -> (String, String) {
    match view {
        View::Checking => (update::CHECKING.to_string(), String::new()),
        View::Found(r) => (update::UPDATE_HEADING.to_string(), update::update_text(&r.version, current)),
        View::UpToDate(newest, newer) => (update::UP_TO_DATE_TITLE.to_string(), update::up_to_date_text(newest, newer.then_some(current))),
        View::Error(text) => (update::ERROR_TITLE.to_string(), text.clone()),
    }
}

fn size_of(view: &View) -> (f64, f64) {
    match view {
        View::Found(_) => (520.0, 420.0),
        _ => (440.0, 150.0),
    }
}

pub struct UpdateWindow {
    host: EguiHost,
    view: View,
    choice: Option<Choice>,
}

impl UpdateWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, view: View) -> Result<UpdateWindow, String> {
        let (w, h) = size_of(&view);
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, update::TITLE, w, h)?;
        Ok(UpdateWindow { host, view, choice: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn set_view(&mut self, view: View) {
        let (w, h) = size_of(&view);
        if size_of(&self.view) != (w, h) {
            let _ = self.host.window.request_inner_size(winit::dpi::LogicalSize::new(w, h));
        }
        self.view = view;
        self.choice = None;
        self.host.request_redraw();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CloseRequested) {
            self.choice = Some(close_choice(&self.view));
            return true;
        }
        self.host.event(event)
    }

    pub fn take_choice(&mut self) -> Option<Choice> {
        self.choice.take()
    }

    pub fn redraw(&mut self) {
        let (heading, text) = heading_and_text(&self.view, update::current_version());
        let labels = buttons(&self.view);
        let view = self.view.clone();
        let mut picked = None;
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(egui::RichText::new(heading.as_str()).strong().size(16.0));
                if !text.is_empty() {
                    ui.add_space(4.0);
                    ui.label(text.as_str());
                }
                if let View::Found(r) = &view {
                    ui.add_space(8.0);
                    ui.label(update::RELEASE_NOTES);
                    let room = (ui.available_height() - 44.0).max(60.0);
                    egui::ScrollArea::vertical().max_height(room).auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::Label::new(r.notes.as_str()).selectable(true));
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for label in &labels {
                        if ui.button(*label).clicked() {
                            picked = Some(choice_of(&view, label));
                        }
                    }
                });
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if enter {
                    picked = Some(enter_choice(&view));
                } else if escape {
                    picked = Some(close_choice(&view));
                }
            });
        });
        if picked.is_some() {
            self.choice = picked;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Release;

    fn release() -> Release {
        Release { version: "9.9.9".into(), name: "Nine".into(), notes: "notes".into(), url: crate::update::RELEASE_PAGE.into(), prerelease: false }
    }

    #[test]
    fn buttons_follow_sparkle() {
        assert_eq!(buttons(&View::Checking), ["Cancel"]);
        assert_eq!(buttons(&View::Found(release())), ["Skip This Version", "Remind Me Later", "Learn More\u{2026}"]);
        assert_eq!(buttons(&View::UpToDate("1.0.0".into(), false)), ["OK"]);
        assert_eq!(buttons(&View::UpToDate("1.0.0".into(), true)), ["OK"]);
        assert_eq!(buttons(&View::Error("x".into())), ["OK"]);
    }

    #[test]
    fn closing_and_keys_follow_sparkle() {
        // The window's close box: Cancel while checking, Remind Me Later on an update (Sparkle),
        // OK otherwise; Enter takes the main button.
        assert_eq!(close_choice(&View::Checking), Choice::Cancel);
        assert_eq!(close_choice(&View::Found(release())), Choice::Later);
        assert_eq!(close_choice(&View::Error("x".into())), Choice::Ok);
        assert_eq!(enter_choice(&View::Found(release())), Choice::LearnMore);
        assert_eq!(enter_choice(&View::Checking), Choice::Cancel);
        assert_eq!(enter_choice(&View::UpToDate("1".into(), false)), Choice::Ok);
    }

    #[test]
    fn texts_follow_the_view() {
        let (heading, text) = heading_and_text(&View::UpToDate("0.0.5".into(), true), "0.0.9");
        assert_eq!(heading, "You\u{2019}re up-to-date!");
        assert_eq!(text, "Tandem 0.0.5 is currently the newest version available.\n(You are currently running version 0.0.9.)");
        let (heading, text) = heading_and_text(&View::Found(release()), "0.0.2");
        assert_eq!(heading, "A new version of Tandem is available!");
        assert!(text.starts_with("Tandem 9.9.9 is now available (you have 0.0.2)."));
        assert_eq!(heading_and_text(&View::Checking, "0.0.2").0, "Checking for updates\u{2026}");
        assert_eq!(heading_and_text(&View::Error(crate::update::RETRIEVE_ERROR.into()), "1").0, "Update Error!");
    }
}
