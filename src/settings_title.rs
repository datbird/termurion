//! Profiles > General's Title popup: a menu of check items that build the profile's Title
//! Components, with the scripts' title providers after them (`ProfilesGeneralPreferencesViewController.m`
//! `updateTitleSettingsMenuForView:`, `toggleSelectedTitleComponentForView:`).

use toml::Value;

use crate::jobtitle::{CUSTOM, JOB, PROFILE_AND_SESSION, PROFILE_NAME, SESSION_NAME, COMMAND_LINE, HOST, PWD, SIZE, TTY, USER};
use crate::profile::Profile;
use crate::schema::Setting;

/// One line of the popup's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A title over a group, not a choice (iTerm2's disabled "Name" and "Foreground Job").
    Header(&'static str),
    Separator,
    /// A check item: the component it toggles and its title.
    Item(u32, &'static str),
}

/// The popup's items in the Profiles pane (`_titleSettings`, `PreferencePanel.xib`).
pub const MENU: [Entry; 14] = [
    Entry::Header("Name"),
    Entry::Item(SESSION_NAME, "Session Name"),
    Entry::Item(PROFILE_NAME, "Profile Name"),
    Entry::Item(PROFILE_AND_SESSION, "Profile & Session Name"),
    Entry::Separator,
    Entry::Header("Foreground Job"),
    Entry::Item(JOB, "Job Name"),
    Entry::Item(COMMAND_LINE, "Job Name with Arguments"),
    Entry::Separator,
    Entry::Item(SIZE, "Columns \u{d7} Rows"),
    Entry::Item(HOST, "Host"),
    Entry::Item(USER, "User"),
    Entry::Item(PWD, "PWD"),
    Entry::Item(TTY, "TTY"),
];

/// The popup's items while Settings edits one session (`_titleSettingsForEditCurrentSession`): no
/// command line and no size.
pub const SESSION_MENU: [Entry; 10] = [
    Entry::Header("Name"),
    Entry::Item(SESSION_NAME, "Session Name"),
    Entry::Item(PROFILE_NAME, "Profile Name"),
    Entry::Item(PROFILE_AND_SESSION, "Profile & Session Name"),
    Entry::Separator,
    Entry::Item(JOB, "Job"),
    Entry::Item(USER, "User"),
    Entry::Item(HOST, "Host"),
    Entry::Item(PWD, "PWD"),
    Entry::Item(TTY, "TTY"),
];

/// The components only one of which can be on (`iTermTitleComponentsNameGroup`; the AI name is not on Windows).
const NAME_GROUP: u32 = SESSION_NAME | PROFILE_NAME | PROFILE_AND_SESSION;

/// The Title Components after a menu item with component `tag` was chosen
/// (`toggleSelectedTitleComponentForView:`): the item toggles, Custom goes, a name replaces the
/// other names, and Job and Command Line exclude each other.
pub fn toggle(original: u32, tag: u32) -> u32 {
    let mut new = original;
    if tag == CUSTOM && original != CUSTOM {
        new = CUSTOM;
    } else {
        new &= !CUSTOM;
        new ^= tag;
    }
    if new & JOB != 0 && new & COMMAND_LINE != 0 {
        if tag == COMMAND_LINE {
            new ^= JOB;
        } else {
            new ^= COMMAND_LINE;
        }
    }
    if tag & NAME_GROUP != 0 {
        let toggled = (original & NAME_GROUP) ^ tag;
        new = (new & !NAME_GROUP) | (tag & toggled);
    }
    new
}

/// The Title row: "Title:" and the popup. A script's title function writes Title Function and
/// Custom (`PGP:1427-1437`). `providers` are the registered title functions as (display name,
/// unique id). True when the profile changed.
pub fn row(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting, providers: &[(String, String)], for_session: bool, browser: bool) -> bool {
    let components = profile.int("Title Components").clamp(0, i64::from(u32::MAX)) as u32;
    let function = crate::jobtitle::title_function(profile.get("Title Function").as_ref());
    let custom = components & CUSTOM != 0;
    let current = function.as_ref().map(|(_, id)| id.clone());
    let (text, items) = crate::settings_ui::title_choice(i64::from(components), function, providers);
    let menu: &[Entry] = if for_session { &SESSION_MENU } else { &MENU };
    let mut toggled = None;
    let mut picked = None;
    let mut popup_rect = None;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        let r = egui::ComboBox::from_id_salt(&s.key).selected_text(text).show_ui(ui, |ui| {
            for entry in menu {
                match *entry {
                    Entry::Header(t) => {
                        ui.add_enabled(false, egui::Label::new(t));
                    }
                    Entry::Separator => {
                        ui.separator();
                    }
                    Entry::Item(tag, title) => {
                        if crate::browser::component_hidden(tag, browser, components) {
                            continue;
                        }
                        let on = !custom && components & tag != 0;
                        if ui.selectable_label(on, format!("{}{title}", if on { "\u{2713} " } else { "    " })).clicked() {
                            toggled = Some(tag);
                        }
                    }
                }
            }
            // The registered title functions, and a saved one nothing registers (disabled).
            if items.len() > 1 {
                ui.separator();
            }
            for (label, id) in items.iter().skip(1) {
                let on = custom && current.as_deref() == Some(id.as_str());
                let enabled = providers.iter().any(|(_, p)| p == id);
                let shown = format!("{}{label}", if on { "\u{2713} " } else { "    " });
                if ui.add_enabled(enabled, egui::Button::selectable(on, shown)).clicked() {
                    picked = Some((label.clone(), id.clone()));
                }
            }
        });
        popup_rect = Some(r.response.rect);
    })
    .response
    .on_hover_text(&s.help);
    if let Some(rect) = popup_rect.filter(|_| crate::settings_keys::indicate_non_default() && !crate::settings_ui::profile_is_default(profile, s, &s.key)) {
        crate::settings_keys::non_default_bar(ui, rect, ui.is_enabled());
    }
    if let Some((name, id)) = picked {
        profile.set("Title Function", crate::jobtitle::title_function_value(&name, &id));
        profile.set("Title Components", Value::Integer(i64::from(CUSTOM)));
        return true;
    }
    if let Some(tag) = toggled {
        profile.set("Title Components", Value::Integer(i64::from(toggle(components, tag))));
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_follows_iterm2() {
        // A component toggles on and off.
        assert_eq!(toggle(0, HOST), HOST);
        assert_eq!(toggle(HOST | USER, HOST), USER);
        // Custom is exclusive: choosing a component leaves it.
        assert_eq!(toggle(CUSTOM, HOST), HOST);
        assert_eq!(toggle(HOST, CUSTOM), CUSTOM);
        // Job and Job with Arguments exclude each other, the one just chosen winning.
        assert_eq!(toggle(JOB, COMMAND_LINE), COMMAND_LINE);
        assert_eq!(toggle(COMMAND_LINE, JOB), JOB);
        // One name at a time; choosing the same one again clears it.
        assert_eq!(toggle(SESSION_NAME | HOST, PROFILE_NAME), PROFILE_NAME | HOST);
        assert_eq!(toggle(PROFILE_NAME | HOST, PROFILE_NAME), HOST);
        assert_eq!(toggle(0, PROFILE_AND_SESSION), PROFILE_AND_SESSION);
    }

    #[test]
    fn the_menu_is_iterm2s() {
        let titles: Vec<&str> = MENU.iter().filter_map(|e| if let Entry::Item(_, t) = e { Some(*t) } else { None }).collect();
        assert_eq!(titles, ["Session Name", "Profile Name", "Profile & Session Name", "Job Name", "Job Name with Arguments", "Columns \u{d7} Rows", "Host", "User", "PWD", "TTY"]);
        let tags: Vec<u32> = MENU.iter().filter_map(|e| if let Entry::Item(t, _) = e { Some(*t) } else { None }).collect();
        assert_eq!(tags, [1, 32, 64, 2, 512, 1024, 256, 128, 4, 8], "iTerm2's menu item tags");
        let session: Vec<u32> = SESSION_MENU.iter().filter_map(|e| if let Entry::Item(t, _) = e { Some(*t) } else { None }).collect();
        assert_eq!(session, [1, 32, 64, 2, 128, 256, 4, 8]);
    }
}
