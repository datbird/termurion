//! Trigger files on the app side: the open-file path and the profile chooser for `.it2triggers`
//! (`TriggerController.m:658-735`).

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::{ask, Then};
use super::{App, Edit};
use crate::egui_host::EguiHost;
use crate::open_file::{self, Facts};
use crate::profile_list_ui::{profile_list, ListState};
use crate::store::Store;
use crate::trigger_file as tf;
use crate::warnings::Kind as Warn;

/// Files waiting for the chooser (one shows at a time); later ones are dropped, as the open
/// questions are.
const MAX_WAITING: usize = 8;

/// What an opened file's parsed triggers lead to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    /// An empty list does nothing (`TriggerController.m:659-661`).
    Nothing,
    /// A file from the internet asks once first (13v1 P21).
    Ask,
    Choose,
}

pub(super) fn next_step(imported: &[toml::Table], marked: bool) -> Next {
    match (imported.is_empty(), marked) {
        (true, _) => Next::Nothing,
        (false, true) => Next::Ask,
        (false, false) => Next::Choose,
    }
}

/// `TriggerController.m:711-735`: each chosen profile gets the triggers it does not already have
/// (an unknown guid is skipped, `:719-721`); the guids of the profiles changed. The write goes
/// through `Profile::set`, the one write point (`triggers::clean`).
fn import_to(store: &mut Store, guids: &[String], imported: &[toml::Table]) -> (Vec<String>, usize) {
    let mut changed = Vec::new();
    let mut dropped = 0;
    for guid in guids {
        let Some(profile) = store.get_mut(guid) else { continue };
        let stored = profile.get("Triggers");
        let merged = tf::add_triggers(stored.as_ref(), imported);
        if stored.as_ref() != Some(&merged) {
            profile.set("Triggers", merged.clone());
            dropped = dropped.max(tf::dropped_by_limit(&merged, profile.get("Triggers").as_ref()));
            changed.push(guid.clone());
        }
    }
    (changed, dropped)
}

/// The profiles changed by `import_to` (their number).
#[cfg(test)]
pub(super) fn apply_import(store: &mut Store, guids: &[String], imported: &[toml::Table]) -> usize {
    import_to(store, guids, imported).0.len()
}

/// The chooser: `select_profiles_text` above the profile list (several rows may be chosen, none at
/// first), with OK and Cancel (`TriggerController.m:694-709`).
pub(super) struct ImportWindow {
    host: EguiHost,
    list: ListState,
    text: String,
    imported: Vec<toml::Table>,
}

enum Outcome {
    Ok,
    Cancel,
}

impl App {
    /// An opened `.it2triggers` file (`TriggerController.m:658-668`, `AD:820-822`). The path was
    /// judged and inspected on a worker; it is judged again and read here through the judge's type,
    /// at most `MAX_FILE` bytes (the file may have changed since), and parsed with bounds. Nothing
    /// is imported until the person picks profiles and presses OK.
    pub(super) fn import_triggers_file(&mut self, f: &Facts, event_loop: &ActiveEventLoop) {
        let shown = crate::inert_files::shown_name(&f.path.to_string_lossy());
        if !f.regular || f.len > tf::MAX_FILE as u64 {
            return self.open_file_banner(format!("Tandem did not open \u{201c}{shown}\u{201d}: it is not a file of a size it reads."));
        }
        let Some(path) = crate::localfs::LocalPath::judge(&f.path) else { return self.open_file_banner(open_file::NETWORK_BANNER.to_string()) };
        let ok = [crate::api_auth::OK];
        let text = match crate::settings_triggers::read_text(&path) {
            Ok(t) => t,
            Err(e) => return self.ask_scripts(ask(tf::IMPORT_FAILED_HEADING, &tf::load_error_text(&shown, &e), &ok, Some(tf::IMPORT_FAILED_ID), Warn::Persistent, Then::Nothing)),
        };
        let imported = match tf::parse(&text, &shown) {
            Ok(list) => list,
            Err(e) => return self.ask_scripts(ask(e.heading, &e.text, &ok, Some(e.identifier), Warn::Persistent, Then::Nothing)),
        };
        match next_step(&imported, f.marked) {
            Next::Nothing => {}
            Next::Ask => self.ask_scripts(ask("", tf::DOWNLOADED_TEXT, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, Warn::Persistent, Then::TriggerImportMarked(imported))),
            Next::Choose => {
                self.trigger_import_queue(imported);
                self.show_trigger_import(event_loop);
            }
        }
    }

    /// Lists a file's triggers for the chooser; it opens on the next turn of the open machinery.
    pub(super) fn trigger_import_queue(&mut self, imported: Vec<toml::Table>) {
        if self.trigger_imports.len() < MAX_WAITING {
            self.trigger_imports.push_back(imported);
        } else {
            self.open_files_ignored(1);
        }
    }

    /// Opens the chooser for the next waiting file, when none shows.
    pub(super) fn show_trigger_import(&mut self, event_loop: &ActiveEventLoop) {
        if self.trigger_import.is_some() {
            return;
        }
        // No window can open without the GPU: the waiting file stays queued, not dropped.
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(imported) = self.trigger_imports.pop_front() else { return };
        let text = tf::select_profiles_text(&imported);
        match EguiHost::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, tf::IMPORT_TITLE, 620.0, 700.0) {
            Ok(host) => {
                host.focus();
                self.trigger_import = Some(ImportWindow { host, list: ListState::default(), text, imported });
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// The chooser's events: OK imports into the chosen profiles, Cancel and closing import nothing.
    /// False for other windows.
    pub(super) fn trigger_import_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        if !self.trigger_import.as_ref().is_some_and(|w| w.host.id() == window) {
            return false;
        }
        let rows = crate::profile_list::rows(&self.store.profiles);
        let default_guid = self.store.default_guid.clone();
        let Some(w) = self.trigger_import.as_mut() else { return false };
        let outcome = match event {
            WindowEvent::CloseRequested => Some(Outcome::Cancel),
            WindowEvent::RedrawRequested => {
                let mut out = None;
                let (list, text) = (&mut w.list, &w.text);
                w.host.paint(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(190.0).id_salt("trigger-import-text").show(ui, |ui| {
                            ui.add(egui::Label::new(text.as_str()).wrap());
                        });
                        ui.add_space(6.0);
                        profile_list(ui, list, &rows, &default_guid, true, None);
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button(crate::api_auth::OK).clicked() {
                                out = Some(Outcome::Ok);
                            }
                            if ui.button(crate::api_auth::CANCEL).clicked() {
                                out = Some(Outcome::Cancel);
                            }
                        });
                    });
                    if out.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        out = Some(Outcome::Cancel);
                    }
                });
                out
            }
            other => {
                w.host.event(other);
                None
            }
        };
        match outcome {
            None => {}
            Some(Outcome::Cancel) => self.trigger_import = None,
            Some(Outcome::Ok) => {
                if let Some(w) = self.trigger_import.take() {
                    let (changed, dropped) = import_to(&mut self.store, &w.list.selection, &w.imported);
                    if dropped > 0 {
                        self.ask_scripts(ask(tf::NOT_IMPORTED_HEADING, &tf::not_kept_text(dropped), &[crate::api_auth::OK], None, Warn::Persistent, Then::Nothing));
                    }
                    if !changed.is_empty() {
                        for guid in &changed {
                            self.dynamic_edited(guid);
                        }
                        self.refresh_profiles_menu();
                        self.apply_edit(Edit::Imported(changed.len()));
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn trigger(regex: &str) -> toml::Table {
        crate::trigger_file::canonical(&toml::from_str::<toml::Table>(&format!("action = \"AlertTrigger\"\nregex = \"{regex}\"\nparameter = \"hi\"")).unwrap()).unwrap()
    }

    #[test]
    fn import_reaches_the_chosen_profiles() {
        let mut store = Store::fresh(std::path::Path::new("none.toml"));
        let (a, c) = (trigger("a"), trigger("c"));
        let mut g1 = crate::profile::Profile::new("One");
        g1.set(crate::profile::GUID, "G1".into());
        g1.set("Triggers", toml::Value::Array(vec![a.clone().into()]));
        let mut g2 = crate::profile::Profile::new("Two");
        g2.set(crate::profile::GUID, "G2".into());
        store.add(g1);
        store.add(g2);
        let changed = apply_import(&mut store, &["G2".to_string(), "GONE".to_string()], &[a.clone(), c.clone()]);
        assert_eq!(changed, 1);
        assert_eq!(store.get("G2").unwrap().get("Triggers"), Some(toml::Value::Array(vec![a.clone().into(), c.into()])));
        assert_eq!(store.get("G1").unwrap().get("Triggers"), Some(toml::Value::Array(vec![a.into()])));
    }

    /// T9 m2: a profile near the cap keeps what fits and the import says how many did not.
    #[test]
    fn import_into_a_nearly_full_profile_counts_the_dropped_tail() {
        let mut store = Store::fresh(std::path::Path::new("none.toml"));
        let mut g = crate::profile::Profile::new("Full");
        g.set(crate::profile::GUID, "G".into());
        g.set("Triggers", toml::Value::Array((0..255).map(|i| trigger(&format!("old{i}")).into()).collect()));
        store.add(g);
        let (changed, dropped) = import_to(&mut store, &["G".to_string()], &[trigger("n1"), trigger("n2"), trigger("n3")]);
        assert_eq!((changed.len(), dropped), (1, 2));
        assert_eq!(store.get("G").unwrap().get("Triggers").and_then(|v| v.as_array().map(Vec::len)), Some(256));
    }

    /// T9 m1: the chooser takes the waiting file off the queue only once a window can open.
    #[test]
    fn show_trigger_import_checks_the_gpu_before_taking_the_file() {
        let src = include_str!("trigger_file_ui.rs");
        let body = &src[src.find("pub(super) fn show_trigger_import(").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.find("self.gpu.as_ref()").unwrap() < body.find("pop_front()").unwrap(), "{body}");
    }

    #[test]
    fn marked_triggers_ask_once() {
        let one = vec![trigger("a")];
        assert_eq!(next_step(&[], true), Next::Nothing);
        assert_eq!(next_step(&[], false), Next::Nothing);
        assert_eq!(next_step(&one, true), Next::Ask);
        assert_eq!(next_step(&one, false), Next::Choose);
        // The question's OK goes straight to the chooser, which does not ask again; Cancel and a
        // closed window import nothing; another process raised it, so it is capped.
        let then = Then::TriggerImportMarked(one);
        assert!(crate::app::scripts_alerts::on_close(&then, 0) == crate::app::scripts_alerts::Answer::Button(1));
        let src = include_str!("scripts_alerts.rs");
        assert!(src.contains("Then::TriggerImportMarked(imported) => {\n                if index == 0 {\n                    self.trigger_import_queue(imported);"));
        assert!(src.contains("Then::OpenRenamedProfile { .. } | Then::TriggerImportMarked(_))"), "capped");
    }

    #[test]
    fn triggers_arm_uses_facts() {
        let open = include_str!("open_file_ui.rs");
        assert!(open.contains("Kind::Triggers => self.import_triggers_file(&f, event_loop),"));
        let own = include_str!("trigger_file_ui.rs");
        let live = &own[..own.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(live.contains("pub(super) fn import_triggers_file(&mut self, f: &Facts,"), "no Path entry point");
        assert!(live.contains("LocalPath::judge(&f.path)") && live.contains("read_text(&path)"), "judged again, read through the judge's type");
        assert!(!live.contains("std::fs::read") && !live.contains("fs::read_to_string"));
        assert!(live.contains("f.len > tf::MAX_FILE") && live.contains("f.marked"));
    }
}
