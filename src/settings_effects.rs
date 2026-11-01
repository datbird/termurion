//! Profiles > Window > Effects: which render plugins run, in what order (`render_fx`).

use crate::profile::Profile;
use crate::render_fx::{self, PROFILE_KEY};

/// `list` with `id` turned on (added at the end) or off.
pub fn toggled(list: &[String], id: &str, on: bool) -> Vec<String> {
    let mut out: Vec<String> = list.iter().filter(|x| *x != id).cloned().collect();
    if on {
        out.push(id.to_string());
    }
    out
}

/// `list` with item `i` moved one place up or down (unchanged at an end).
pub fn moved(list: &[String], i: usize, up: bool) -> Vec<String> {
    let mut out = list.to_vec();
    let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < list.len()) };
    if let Some(j) = j.filter(|_| i < list.len()) {
        out.swap(i, j);
    }
    out
}

/// The editor. True when the profile changed.
pub fn editor(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let (plugins, errors) = render_fx::discover(Some(&render_fx::user_dir()));
    let mut list = crate::settings_lists::strings(profile, PROFILE_KEY);
    let mut changed = None;
    ui.label("Effects:").on_hover_text("Shaders that run over the whole window each frame. Checked effects run in the order listed.");
    // The running ones first, in their order, with up and down.
    for (i, id) in list.iter().enumerate() {
        let title = plugins.iter().find(|p| &p.id == id).map_or_else(|| format!("{id} (not found)"), |p| p.title().to_string());
        ui.horizontal(|ui| {
            let mut on = true;
            if ui.checkbox(&mut on, title).changed() {
                changed = Some(toggled(&list, id, false));
            }
            if ui.add_enabled(i > 0, egui::Button::new("\u{2191}").small()).on_hover_text("Run earlier").clicked() {
                changed = Some(moved(&list, i, true));
            }
            if ui.add_enabled(i + 1 < list.len(), egui::Button::new("\u{2193}").small()).on_hover_text("Run later").clicked() {
                changed = Some(moved(&list, i, false));
            }
        });
    }
    for p in plugins.iter().filter(|p| !list.contains(&p.id)) {
        let mut on = false;
        let label = if p.builtin() { p.title().to_string() } else { format!("{} (yours)", p.title()) };
        if ui.checkbox(&mut on, label).on_hover_text(&p.manifest.description).changed() {
            changed = Some(toggled(&list, &p.id, true));
        }
    }
    for (id, e) in &errors {
        ui.colored_label(ui.visuals().warn_fg_color, format!("{id}: {e}"));
    }
    ui.horizontal(|ui| {
        if ui.button("Open Effects Folder").on_hover_text("Your own effects go here, one folder each with a plugin.toml.").clicked() {
            let dir = render_fx::user_dir();
            let _ = std::fs::create_dir_all(&dir);
            open_folder(&dir);
        }
    });
    ui.label(egui::RichText::new("The built-in cursor effects are from ghostty-cursor-fx by AONIR (MIT).").small().weak());
    if let Some(new) = changed {
        list = new;
        crate::settings_lists::store(profile, PROFILE_KEY, list);
        return true;
    }
    false
}

/// Shows a folder in the file manager.
fn open_folder(dir: &std::path::Path) {
    #[cfg(windows)]
    let program = std::env::var_os("SystemRoot").map(|r| std::path::PathBuf::from(r).join("System32")).and_then(|s| crate::semantic::explorer_in(&s));
    #[cfg(target_os = "macos")]
    let program = Some(std::path::PathBuf::from("/usr/bin/open"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = Some(std::path::PathBuf::from("xdg-open"));
    if let Some(p) = program {
        let _ = std::process::Command::new(p).arg(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn toggling_adds_at_the_end_and_removes() {
        assert_eq!(toggled(&v(&["a", "b"]), "c", true), v(&["a", "b", "c"]));
        assert_eq!(toggled(&v(&["a", "b"]), "a", false), v(&["b"]));
        assert_eq!(toggled(&v(&["a"]), "a", true), v(&["a"]), "no duplicate");
    }

    #[test]
    fn moving_stops_at_the_ends() {
        assert_eq!(moved(&v(&["a", "b", "c"]), 1, true), v(&["b", "a", "c"]));
        assert_eq!(moved(&v(&["a", "b", "c"]), 1, false), v(&["a", "c", "b"]));
        assert_eq!(moved(&v(&["a", "b"]), 0, true), v(&["a", "b"]));
        assert_eq!(moved(&v(&["a", "b"]), 1, false), v(&["a", "b"]));
    }
}
