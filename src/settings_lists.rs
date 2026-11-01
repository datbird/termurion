//! String-list editors: Automatic Profile Switching rules (Bound Hosts) and Jobs to Ignore.


use toml::Value;

use crate::aps::{self, Candidate, RuleProblem};
use crate::profile::Profile;

/// Empty and repeated rows removed; true when any went (the editor beeps, as iTerm2 does).
pub fn tidy(list: &[String]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    for r in list {
        let t = r.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    let dropped = out.len() != list.len();
    (out, dropped)
}

/// What happened to a list in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEvent {
    /// "+": an empty row to type in; it is kept until the row is left.
    Added,
    Removed,
    /// A row lost focus (or Enter): empty and repeated rows go, with a beep.
    Committed,
}

/// The list after an event, and whether to beep.
pub fn after(list: &[String], event: ListEvent) -> (Vec<String>, bool) {
    match event {
        ListEvent::Added => {
            let mut l = list.to_vec();
            l.push(String::new());
            (l, false)
        }
        ListEvent::Removed => (list.to_vec(), false),
        ListEvent::Committed => tidy(list),
    }
}

pub fn strings(profile: &Profile, key: &str) -> Vec<String> {
    profile.get(key).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
}

pub fn store(profile: &mut Profile, key: &str, list: Vec<String>) {
    profile.set(key, Value::Array(list.into_iter().map(Value::String).collect()));
}

fn beep() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
    }
}

/// Rows of text with add and remove: the event, and whether text changed.
fn rows(ui: &mut egui::Ui, id: &str, list: &mut Vec<String>, hint: &str) -> (Option<ListEvent>, bool) {
    let (mut event, mut edited) = (None, false);
    let mut remove = None;
    for (i, r) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let resp = crate::settings_keys::field(ui, egui::TextEdit::singleline(r).hint_text(hint).desired_width(320.0).id_salt((id, i)));
            edited |= resp.changed();
            if resp.lost_focus() {
                event = Some(ListEvent::Committed);
            }
            if ui.small_button("\u{2212}").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        list.remove(i);
        event = Some(ListEvent::Removed);
    }
    if ui.small_button("+").clicked() {
        event = Some(ListEvent::Added);
    }
    (event, edited)
}

/// Profiles > Advanced, Automatic Profile Switching. `others` are the other profiles and
/// their rules, for the "already bound" warning.
pub fn bound_hosts(ui: &mut egui::Ui, profile: &mut Profile, others: &[Candidate], warning: &mut Option<String>) -> bool {
    ui.label("Automatic Profile Switching:");
    ui.weak("Shell Integration must be installed to use this feature.");
    let mut list = strings(profile, "Bound Hosts");
    let (event, edited) = rows(ui, "bound-hosts", &mut list, "Enter a rule\u{2026}");
    ui.collapsing("Rule examples", |ui| {
        for (rule, what) in [("user@host:/path", "user, host, and folder"), ("user@", "a user on any host"), ("host", "a host name (* matches anything)"), ("/path", "a folder (C:\\path on Windows)"), ("&job", "a program that is running"), ("!rule", "sticky: stays when nothing else matches")] {
            ui.label(format!("{rule}    {what}"));
        }
    });
    if let Some(w) = warning.as_ref() {
        ui.colored_label(egui::Color32::from_rgb(0xff, 0xb0, 0x40), w.as_str());
    }
    let Some(event) = event else {
        if edited {
            store(profile, "Bound Hosts", list);
        }
        return false;
    };
    let (clean, beeped) = after(&list, event);
    if beeped {
        beep();
    }
    if event != ListEvent::Committed {
        store(profile, "Bound Hosts", clean);
        return true;
    }
    *warning = None;
    // A rule another profile has is taken out of this one, with iTerm2's warning.
    let mut kept = Vec::new();
    for r in clean {
        match aps::check_rule(&r, &kept, others) {
            Err(RuleProblem::Owned { message, .. }) => *warning = Some(format!("{message} It was removed from this profile.")),
            _ => kept.push(r),
        }
    }
    store(profile, "Bound Hosts", kept);
    true
}

/// A profile's string list as a token field with the row's label (Tags, Snippets Filter): the
/// label, the field, and the row's help as a tooltip. True when the profile changed.
pub fn tokens_row(ui: &mut egui::Ui, profile: &mut Profile, s: &crate::schema::Setting, hint: &str) -> bool {
    let mut tokens = strings(profile, &s.key);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        changed = token_field(ui, &s.key, &mut tokens, hint);
    })
    .response
    .on_hover_text(&s.help);
    if changed {
        store(profile, &s.key, tokens);
    }
    changed
}

/// Profiles > Session, Jobs to Ignore (used when Prompt Before Closing is "If there are
/// jobs besides").
pub fn jobs_to_ignore(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let on = profile.int("Prompt Before Closing 2") == crate::close_prompt::IF_JOBS;
    let mut list = strings(profile, "Jobs to Ignore");
    let mut result = (None, false);
    ui.label("Jobs to Ignore:");
    ui.add_enabled_ui(on, |ui| {
        result = rows(ui, "jobs-to-ignore", &mut list, "Job Name");
    });
    match result {
        (Some(e), _) => {
            store(profile, "Jobs to Ignore", after(&list, e).0);
            true
        }
        (None, true) => {
            store(profile, "Jobs to Ignore", list);
            false
        }
        (None, false) => false,
    }
}

/// The tokens after the text in a token field's box was committed (`NSTokenField`): the text is
/// split at commas, each piece trimmed, and the new ones that are not already tokens added in
/// order. Returns the tokens and whether any were added.
pub fn tokens_after(tokens: &[String], text: &str) -> (Vec<String>, bool) {
    let mut out = tokens.to_vec();
    let mut added = false;
    for piece in text.split(',') {
        let t = piece.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
            added = true;
        }
    }
    (out, added)
}

/// A token field (iTerm2's `NSTokenField`, Tags and Snippets Filter): the tokens as chips, each with
/// a remove button, and a box to type the next one; a comma, Enter or leaving the box makes the
/// typed text a token. True when the tokens changed.
pub fn token_field(ui: &mut egui::Ui, salt: &str, tokens: &mut Vec<String>, hint: &str) -> bool {
    let id = ui.id().with(("token-field", salt));
    let mut text: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let mut remove = None;
        for (i, t) in tokens.iter().enumerate() {
            ui.group(|ui| {
                ui.label(t.as_str());
                if ui.small_button("\u{2715}").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            tokens.remove(i);
            changed = true;
        }
        let r = crate::settings_keys::field(ui, egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(140.0).id_salt((salt, "box")));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if text.contains(',') || enter || (r.lost_focus() && !text.trim().is_empty()) {
            let (next, added) = tokens_after(tokens, &text);
            if added {
                *tokens = next;
                changed = true;
            }
            text.clear();
            if enter {
                r.request_focus();
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, text));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_tidied_on_commit() {
        let list = vec!["a".to_string(), " ".to_string(), "b".to_string(), "a".to_string()];
        assert_eq!(tidy(&list), (vec!["a".to_string(), "b".to_string()], true), "empty and repeated rows go, with a beep");
        assert_eq!(tidy(&["x".to_string()]), (vec!["x".to_string()], false));
    }

    #[test]
    fn a_new_row_stays_until_it_is_left() {
        let (list, beep) = after(&["a".to_string()], ListEvent::Added);
        assert_eq!((list, beep), (vec!["a".to_string(), String::new()], false), "+ adds an empty row to type in");
        let (list, beep) = after(&["a".to_string(), String::new()], ListEvent::Committed);
        assert_eq!((list, beep), (vec!["a".to_string()], true), "left empty: it goes, with a beep");
        let (list, _) = after(&["a".to_string(), "b".to_string()], ListEvent::Removed);
        assert_eq!(list, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn token_field_splits_at_commas_and_skips_repeats() {
        let have = vec!["work".to_string()];
        assert_eq!(tokens_after(&have, "work, home ,, a/b"), (vec!["work".into(), "home".into(), "a/b".into()], true));
        assert_eq!(tokens_after(&have, "  "), (have.clone(), false));
        assert_eq!(tokens_after(&have, "work"), (have, false), "a token already there is not added again");
    }
}
