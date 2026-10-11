//! The HTTP sign-in prompt of a browser pane (13x7 Task 9): a native Tandem window (never page
//! content), iTerm2's `ModalPasswordAlert` (`MPA:32-38`, `:111-127`, `:146-222`). The caller builds the
//! heading (`browser_basic_auth::prompt_heading`), so the real origin and the server's labelled realm
//! are the same text the coordinator was given. The window carries the one input guard
//! (`prompt_ui::InputGuard`): a page can raise it, so input is ignored for a moment after it appears
//! and after it regains the focus, and a prefilled prompt has no Enter default until the person has
//! typed in a field or clicked OK. The typed user name and password are `Secret`s (wiped on drop);
//! nothing here logs.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::browser_basic_auth as ba;
use crate::egui_host::EguiHost;
use crate::prompt_ui::InputGuard;
use crate::pwm::Secret;
use crate::secret_field::secret_field;

pub enum AuthAnswer {
    Ok { user: String, password: Secret, remember: bool },
    Cancel,
    /// The typed user name and password are kept (`MPA:117-122`).
    PasswordManager { user: String, password: Secret },
}

/// Which field takes the first focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstFocus {
    User,
    Password,
}

/// What the window shows.
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub buttons: Vec<&'static str>,
    pub remember: bool,
    pub first_focus: FirstFocus,
}

/// The buttons, the Remember checkbox and the first focus (`MPA:146-156`): the password when a user
/// name is prefilled, else the user name.
pub fn layout(offer_remember: bool, offer_password_manager: bool, user_prefilled: bool) -> Layout {
    let mut buttons = vec![ba::OK, ba::CANCEL];
    if offer_password_manager {
        buttons.push(ba::PASSWORD_MANAGER);
    }
    Layout { buttons, remember: offer_remember, first_focus: if user_prefilled { FirstFocus::Password } else { FirstFocus::User } }
}

/// Whether Enter answers OK: a prefilled prompt (a stored credential) needs the person to have typed
/// first, so a stray Enter can never send a stored password.
pub fn enter_answers(prefilled: bool, typed: bool) -> bool {
    !prefilled || typed
}

pub struct AuthWindow {
    host: EguiHost,
    text: String,
    user: Secret,
    password: Secret,
    remember: bool,
    layout: Layout,
    prefilled: bool,
    typed: bool,
    first: bool,
    guard: InputGuard,
    /// The password the window began with, to see typing.
    start: (usize, usize),
}

impl AuthWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        text: &str,
        user: &str,
        password: Option<&Secret>,
        offer_remember: bool,
        remember_by_default: bool,
        offer_password_manager: bool,
    ) -> Result<AuthWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Tandem", 420.0, 230.0)?;
        let layout = layout(offer_remember, offer_password_manager, !user.is_empty());
        let pw = password.map(Secret::duplicate).unwrap_or_else(|| Secret::new(String::new()));
        let start = (user.len(), pw.expose().len());
        let prefilled = !user.is_empty() || !pw.is_empty();
        Ok(AuthWindow {
            host,
            text: text.to_string(),
            user: Secret::new(user.to_string()),
            password: pw,
            remember: offer_remember && remember_by_default,
            layout,
            prefilled,
            typed: false,
            first: true,
            guard: InputGuard::new(std::time::Instant::now()),
            start,
        })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// The answer a close (X, Alt+F4) gives.
    pub fn close_answer(&self) -> AuthAnswer {
        AuthAnswer::Cancel
    }

    /// A guarded window drops a key, click, touch or input-method event before its guard is armed.
    pub fn event(&mut self, e: &WindowEvent) -> bool {
        let now = std::time::Instant::now();
        self.guard.window_event(e, now);
        if self.guard.drops(e, now) {
            return false;
        }
        self.host.event(e)
    }

    pub fn redraw(&mut self) -> Option<AuthAnswer> {
        let mut answer: Option<AuthAnswer> = None;
        let armed = self.guard.armed(std::time::Instant::now());
        let (text, user, password, remember, lay, prefilled, typed, first, start) = (&self.text, &mut self.user, &mut self.password, &mut self.remember, &self.layout, self.prefilled, &mut self.typed, &mut self.first, self.start);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(text.as_str());
                ui.add_space(6.0);
                let (uid, pid) = (egui::Id::new("basic-auth-user"), egui::Id::new("basic-auth-password"));
                let w = ui.available_width();
                secret_field(ui, uid, user, armed, Some(ba::USER_PLACEHOLDER), w, true);
                ui.add_space(4.0);
                secret_field(ui, pid, password, armed, Some(ba::PASSWORD_PLACEHOLDER), w, false);
                if *first {
                    ui.ctx().memory_mut(|m| m.request_focus(if lay.first_focus == FirstFocus::User { uid } else { pid }));
                    *first = false;
                }
                if (user.expose().len(), password.expose().len()) != start || ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_)))) {
                    *typed = true;
                }
                if lay.remember {
                    ui.add_space(4.0);
                    ui.add_enabled(armed, egui::Checkbox::new(remember, ba::REMEMBER));
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                ui.add_space(8.0);
                let done = |user: &Secret, password: &Secret, remember: bool| -> AuthAnswer {
                    AuthAnswer::Ok { user: user.expose().to_string(), password: password.duplicate(), remember }
                };
                ui.horizontal(|ui| {
                    for label in &lay.buttons {
                        let r = ui.add_enabled(armed, egui::Button::new(*label));
                        // Only a real pointer release over the button counts.
                        let clicked = r.clicked() && r.hovered() && ui.input(|i| i.pointer.primary_released() || i.pointer.any_released());
                        if !clicked {
                            continue;
                        }
                        answer = Some(match *label {
                            ba::OK => done(user, password, *remember),
                            ba::PASSWORD_MANAGER => AuthAnswer::PasswordManager { user: user.expose().to_string(), password: password.duplicate() },
                            _ => AuthAnswer::Cancel,
                        });
                    }
                });
                if answer.is_none() && armed && enter && enter_answers(prefilled, *typed) {
                    answer = Some(done(user, password, *remember));
                }
                if answer.is_none() && escape {
                    answer = Some(AuthAnswer::Cancel);
                }
                if !armed {
                    ui.label("The buttons turn on in a moment.");
                }
            });
        });
        answer
    }
}

impl Drop for AuthWindow {
    fn drop(&mut self) {
        // The Secrets wipe themselves on drop; nothing else holds a typed value.
        self.user.clear();
        self.password.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_window_follows_iterm2() {
        let l = layout(true, true, true);
        assert_eq!(l.buttons, vec!["OK", "Cancel", "Password Manager"]);
        assert!(l.remember);
        assert_eq!(l.first_focus, FirstFocus::Password, "a prefilled user name focuses the password");
        let l = layout(false, false, false);
        assert_eq!(l.buttons, vec!["OK", "Cancel"]);
        assert!(!l.remember);
        assert_eq!(l.first_focus, FirstFocus::User);
        assert_eq!(layout(true, false, false).buttons, vec!["OK", "Cancel"]);
    }

    #[test]
    fn a_prefilled_prompt_has_no_enter_default_and_the_guard_holds() {
        assert!(!enter_answers(true, false), "a stored credential is never sent by a stray Enter");
        assert!(enter_answers(true, true) && enter_answers(false, false));
        let src = include_str!("basic_auth_ui.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("InputGuard::new") && prod.contains("self.guard.drops(e, now)"), "the one input guard");
        assert!(prod.contains("secret_field(") && !prod.contains("TextEdit"), "secrets are edited in place, never in egui's text copies");
        assert!(!prod.contains("println!") && !prod.contains("log::") && !prod.contains("dlog"), "nothing is logged");
    }
}
