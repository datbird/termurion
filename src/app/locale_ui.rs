//! The locale UI (`PTM:508-541`, `LP:66-245`, `PS:3370-3464`): the picker window, the warnings
//! and the start-time prompt.

use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::{self, Then};
use super::{App, Edit};
use crate::egui_host::EguiHost;
use crate::locale_guess::{self, Picker};
use crate::warnings::Kind;

/// What a locale question is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ask {
    /// The wrong-encoding warning for `locale` under profile `guid`; `from_settings` is "Keep
    /// stores the locale" (a pick made in Settings, or a remembered pick of the prompt). As a
    /// queued ask it reopens Settings' picker (Change Locale after such a pick).
    Mismatch { guid: String, locale: String, from_settings: bool },
    /// The start-time prompt for a profile whose automatic guess failed.
    Prompt { guid: String, profile_name: String, allow_remember: bool },
}

/// The profile's new "Set Local Environment Vars" and "Custom Locale" for an answer: `choice`
/// is the locale picked, None for "Use Minimal POSIX Locale". None when nothing is written
/// (`PS:3405-3464`: only a remembered pick of a profile that may remember).
pub(super) fn apply_answer(ask: &Ask, choice: Option<&str>, remember: bool) -> Option<(i64, Option<String>)> {
    match ask {
        Ask::Prompt { allow_remember, .. } => {
            if !remember || !allow_remember {
                return None;
            }
            Some(match choice {
                Some(l) => (2, Some(l.to_string())),
                None => (3, None),
            })
        }
        Ask::Mismatch { locale, from_settings, .. } => from_settings.then(|| (2, Some(locale.clone()))),
    }
}

/// What the picker window answered.
pub(super) enum Pick {
    Open,
    /// OK with a locale, and the remember box.
    Locale(String, bool),
    /// "Use Minimal POSIX Locale", and the remember box.
    Minimal(bool),
    Cancel,
}

/// What the open picker is for.
pub(super) enum Purpose {
    Prompt(Ask),
    /// Settings' Change Locale for the profile Guid.
    Settings(String),
}

/// The picker (`LP:125-195`): an egui window with the message, the locales in a combo (the
/// wrong-encoding ones below a disabled header), the remember box when allowed, OK and "Use
/// Minimal POSIX Locale" or Cancel.
pub(super) struct PickerWindow {
    host: EguiHost,
    purpose: Purpose,
    picker: Picker,
    selected: Option<(bool, usize)>,
    message: String,
    /// The remember box's label when it shows.
    remember_label: Option<String>,
    remember: bool,
    /// The second button is "Use Minimal POSIX Locale" (start) rather than Cancel (Settings).
    minimal: bool,
}

impl PickerWindow {
    #[allow(clippy::too_many_arguments)]
    fn open(event_loop: &ActiveEventLoop, gpu: (&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue), purpose: Purpose, picker: Picker, current: Option<&str>, message: &str, remember_label: Option<String>, minimal: bool) -> Result<PickerWindow, String> {
        let host = EguiHost::open(event_loop, gpu.0, gpu.1, gpu.2, gpu.3, "Locale", 480.0, if remember_label.is_some() { 230.0 } else { 200.0 })?;
        let selected = current.and_then(|c| picker.find(c)).or(picker.default);
        Ok(PickerWindow { host, purpose, picker, selected, message: message.to_string(), remember_label, remember: false, minimal })
    }

    fn id(&self) -> WindowId {
        self.host.id()
    }

    fn redraw(&mut self) -> Pick {
        let mut out = Pick::Open;
        let (picker, selected, remember, label) = (&self.picker, &mut self.selected, &mut self.remember, &self.remember_label);
        let (message, minimal) = (&self.message, self.minimal);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add(egui::Label::new(message.as_str()).wrap());
                ui.add_space(8.0);
                let shown = selected.and_then(|at| picker.entry(at)).map_or(String::new(), |e| e.title.clone());
                egui::ComboBox::from_id_salt("locale_pick").width(ui.available_width() - 8.0).selected_text(shown).show_ui(ui, |ui| {
                    for (i, e) in picker.good.iter().enumerate() {
                        if ui.selectable_label(*selected == Some((false, i)), &e.title).clicked() {
                            *selected = Some((false, i));
                        }
                    }
                    if !picker.bad.is_empty() {
                        ui.separator();
                        ui.add_enabled(false, egui::Label::new(locale_guess::WRONG_ENCODING_SECTION));
                        for (i, e) in picker.bad.iter().enumerate() {
                            if ui.selectable_label(*selected == Some((true, i)), &e.title).clicked() {
                                *selected = Some((true, i));
                            }
                        }
                    }
                });
                if let Some(label) = label {
                    ui.add_space(6.0);
                    ui.checkbox(remember, label.as_str());
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let chosen = selected.and_then(|at| picker.entry(at)).map(|e| e.locale.clone());
                    if ui.add_enabled(chosen.is_some(), egui::Button::new(crate::api_auth::OK)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) && chosen.is_some() {
                        out = Pick::Locale(chosen.clone().unwrap_or_default(), *remember);
                    }
                    if ui.button(if minimal { locale_guess::USE_MINIMAL } else { crate::api_auth::CANCEL }).clicked() {
                        out = if minimal { Pick::Minimal(*remember) } else { Pick::Cancel };
                    }
                });
                if !minimal && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = Pick::Cancel;
                }
            });
        });
        out
    }
}

impl App {
    /// The picker for a profile: Windows' specific locales as `ll_CC.UTF-8` and `ll_CC`, the
    /// wrong-encoding ones apart when the profile is UTF-8 (R6).
    fn locale_picker(&self, guid: &str) -> Picker {
        let encoding = self.store.get(guid).map_or(4, |p| p.int("Character Encoding"));
        let facts = crate::locale_win::facts(encoding, "");
        let system = crate::locale_win::system_locales();
        let (languages, countries) = crate::locale_win::names_for(&system);
        let language = |c: &str| languages.get(c).cloned();
        let country = |c: &str| countries.get(c).cloned();
        Picker::new(&locale_guess::picker_locales(&system), encoding == 4, &facts.preferred, (&language, &country))
    }

    /// Queues the start-time prompt for a profile, unless one is open or waiting for it.
    pub(super) fn queue_locale_prompt(&mut self, guid: String, profile_name: String, allow_remember: bool) {
        let same = |a: &Ask| matches!(a, Ask::Prompt { guid: g, .. } if *g == guid);
        let open = matches!(self.locale_window.as_ref().map(|w| &w.purpose), Some(Purpose::Prompt(a)) if same(a));
        if open || self.locale_asks.iter().any(same) {
            return;
        }
        self.locale_asks.push_back(Ask::Prompt { guid, profile_name, allow_remember });
    }

    /// Asks the wrong-encoding warning through the alert queue (a remembered answer is given at
    /// once), unless one for the profile already waits.
    pub(super) fn ask_locale_mismatch(&mut self, guid: String, locale: String, from_settings: bool) {
        if self.alert_waits(|t| matches!(t, Then::LocaleMismatch { guid: g, .. } if *g == guid)) {
            return;
        }
        let id = locale_guess::mismatch_id(&guid);
        let ask = scripts_alerts::ask(locale_guess::MISMATCH_HEADING, locale_guess::MISMATCH_TEXT, &[locale_guess::CHANGE_LOCALE, locale_guess::KEEP_LOCALE], Some(&id), Kind::Permanent, Then::LocaleMismatch { guid, locale, from_settings });
        self.ask_scripts(ask);
    }

    /// The warning's answer: Change Locale (0) asks for another locale (Settings' picker, or the
    /// prompt at a session start), Keep This Locale (1) stores it when it came from Settings.
    pub(super) fn locale_mismatch_answered(&mut self, guid: String, locale: String, from_settings: bool, index: usize) {
        if index == 0 {
            if from_settings {
                self.locale_asks.push_back(Ask::Mismatch { guid, locale, from_settings: true });
            } else {
                let Some(p) = self.store.get(&guid) else { return };
                let (name, allow) = (p.name(), !p.bool("Is Dynamic Profile"));
                self.queue_locale_prompt(guid, name, allow);
            }
            return;
        }
        let ask = Ask::Mismatch { guid, locale: locale.clone(), from_settings };
        self.locale_answered(ask, Some(locale), false);
    }

    /// Settings' "Change Locale": opens the picker with the profile's locale selected.
    pub(super) fn open_locale_settings(&mut self, guid: String) {
        let locale = self.store.get(&guid).map(|p| p.string("Custom Locale")).unwrap_or_default();
        self.locale_asks.push_back(Ask::Mismatch { guid, locale, from_settings: true });
    }

    /// Opens the next queued question when none shows (each pass of the event loop).
    pub(super) fn open_locale_asks(&mut self, event_loop: &ActiveEventLoop) {
        if self.locale_window.is_some() {
            return;
        }
        let gpu = self.gpu.as_ref().map(|g| (g.instance.clone(), g.adapter.clone(), g.device.clone(), g.queue.clone())).or_else(|| self.prefs_gpu.clone());
        let Some((instance, adapter, device, queue)) = gpu else { return };
        while let Some(ask) = self.locale_asks.pop_front() {
            let (guid, purpose, current, message, label, minimal) = match &ask {
                Ask::Mismatch { guid, locale, .. } => {
                    if self.store.get(guid).is_none() {
                        continue;
                    }
                    (guid.clone(), Purpose::Settings(guid.clone()), Some(locale.clone()), locale_guess::SELECT_MESSAGE, None, false)
                }
                Ask::Prompt { guid, profile_name, allow_remember } => {
                    if self.store.get(guid).is_none() {
                        continue;
                    }
                    (guid.clone(), Purpose::Prompt(ask.clone()), None, locale_guess::PROMPT_MESSAGE, allow_remember.then(|| locale_guess::save_to_profile(profile_name)), true)
                }
            };
            let picker = self.locale_picker(&guid);
            match PickerWindow::open(event_loop, (&instance, &adapter, &device, &queue), purpose, picker, current.as_deref(), message, label, minimal) {
                Ok(w) => self.locale_window = Some(w),
                Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
            }
            return;
        }
    }

    /// Events for the picker. False for other windows.
    pub(super) fn locale_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.locale_window.as_mut().filter(|w| w.id() == window) else { return false };
        let pick = match event {
            WindowEvent::CloseRequested => Pick::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.host.event(other);
                Pick::Open
            }
        };
        if matches!(pick, Pick::Open) {
            return true;
        }
        let Some(w) = self.locale_window.take() else { return true };
        match (w.purpose, pick) {
            (_, Pick::Cancel | Pick::Open) => {}
            (Purpose::Settings(guid), Pick::Locale(locale, _)) => self.locale_settings_picked(guid, locale),
            (Purpose::Settings(_), Pick::Minimal(_)) => {}
            (Purpose::Prompt(ask), Pick::Locale(locale, remember)) => {
                let Ask::Prompt { guid, .. } = &ask else { return true };
                let suspect = self.store.get(guid).is_some_and(|p| locale_guess::is_suspect(p.int("Character Encoding") == 4, &locale));
                if suspect {
                    // The pick is stored only when kept, and only if the person asked to remember.
                    let guid = guid.clone();
                    self.ask_locale_mismatch(guid, locale, remember);
                } else {
                    self.locale_answered(ask, Some(locale), remember);
                }
            }
            (Purpose::Prompt(ask), Pick::Minimal(remember)) => self.locale_answered(ask, None, remember),
        }
        true
    }

    /// OK in Settings' picker: a non-UTF-8 locale under a UTF-8 profile asks the warning first
    /// (Keep stores it), any other is stored.
    fn locale_settings_picked(&mut self, guid: String, locale: String) {
        let suspect = self.store.get(&guid).is_some_and(|p| locale_guess::is_suspect(p.int("Character Encoding") == 4, &locale));
        if suspect {
            self.ask_locale_mismatch(guid, locale, true);
        } else {
            self.locale_answered(Ask::Mismatch { guid, locale: locale.clone(), from_settings: true }, Some(locale), false);
        }
    }

    /// An answer applied: what `apply_answer` gives is written to the saved profile (the
    /// session's own copy is not a separate profile here; iTerm2 writes `originalProfile`).
    pub(super) fn locale_answered(&mut self, ask: Ask, choice: Option<String>, remember: bool) {
        let Some((mode, custom)) = apply_answer(&ask, choice.as_deref(), remember) else { return };
        let (Ask::Mismatch { guid, .. } | Ask::Prompt { guid, .. }) = &ask;
        let shared = self.store.shared_guid(guid);
        let Some(p) = self.store.get_mut(&shared) else { return };
        p.set("Set Local Environment Vars", Value::Integer(mode));
        if let Some(l) = custom {
            p.set("Custom Locale", Value::String(l));
        }
        self.save_now();
        self.apply_edit(Edit::Changed(shared));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(allow: bool) -> Ask {
        Ask::Prompt { guid: "G".into(), profile_name: "Work".into(), allow_remember: allow }
    }

    #[test]
    fn answers_follow_iterm2() {
        assert_eq!(apply_answer(&prompt(true), Some("fr_FR.UTF-8"), true), Some((2, Some("fr_FR.UTF-8".to_string()))));
        assert_eq!(apply_answer(&prompt(true), Some("fr_FR.UTF-8"), false), None);
        assert_eq!(apply_answer(&prompt(true), None, true), Some((3, None)));
        assert_eq!(apply_answer(&prompt(true), None, false), None);
        // A dynamic profile never remembers.
        assert_eq!(apply_answer(&prompt(false), Some("fr_FR.UTF-8"), true), None);
        let keep = Ask::Mismatch { guid: "G".into(), locale: "C".into(), from_settings: true };
        assert_eq!(apply_answer(&keep, Some("C"), false), Some((2, Some("C".to_string()))));
        // A custom locale already in the profile (session start) has nothing to store.
        let at_start = Ask::Mismatch { guid: "G".into(), locale: "C".into(), from_settings: false };
        assert_eq!(apply_answer(&at_start, Some("C"), false), None);
    }
}
