//! The profile Advanced tab's editors: Smart Selection rules and Semantic History.

use toml::Value;

use crate::profile::Profile;
use crate::smartsel::{self, Rule, RuleAction};

/// iTerm2's Smart Selection action selectors and their menu names.
pub const RULE_ACTIONS: [(&str, &str); 7] = [
    ("openFile:", "Open File"),
    ("openUrl:", "Open URL"),
    ("runCommand:", "Run Command"),
    ("runCoprocess:", "Run Coprocess"),
    ("sendText:", "Send Text"),
    ("runCommandInWindow:", "Run Command in Window"),
    ("copy:", "Copy"),
];

/// iTerm2's Semantic History actions and their menu names.
pub const HISTORY_ACTIONS: [(&str, &str); 7] = [
    ("best editor", "Open with default app"),
    ("editor", "Open with editor..."),
    ("url", "Open URL..."),
    ("command", "Run command..."),
    ("coprocess", "Run coprocess..."),
    ("raw command", "Always run command..."),
    ("send text", "Send text..."),
];

/// Rules as the profile stores them.
pub fn rules_to_value(rules: &[Rule]) -> Value {
    let rule = |r: &Rule| {
        let mut t = toml::Table::new();
        t.insert("regex".into(), Value::String(r.regex.clone()));
        t.insert("precision".into(), Value::String(smartsel::precision_name(r.precision).into()));
        t.insert("notes".into(), Value::String(r.notes.clone()));
        if !r.actions.is_empty() {
            let actions = r.actions.iter().map(|a| {
                let mut at = toml::Table::new();
                at.insert("title".into(), Value::String(a.title.clone()));
                at.insert("action".into(), Value::String(a.action.clone()));
                at.insert("parameter".into(), Value::String(a.parameter.clone()));
                Value::Table(at)
            });
            t.insert("actions".into(), Value::Array(actions.collect()));
        }
        Value::Table(t)
    };
    Value::Array(rules.iter().map(rule).collect())
}

fn combo(ui: &mut egui::Ui, id: &str, choices: &[(&str, &str)], value: &mut String) -> bool {
    let shown = choices.iter().find(|(v, _)| v == value).map_or(value.clone(), |(_, l)| l.to_string());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
        for (v, l) in choices {
            if ui.selectable_label(value == v, *l).clicked() && value != v {
                *value = v.to_string();
                changed = true;
            }
        }
    });
    changed
}

/// The Smart Selection rules editor. Returns true when the rules changed.
pub fn smart_selection_rules(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let mut rules = smartsel::rules(profile);
    let sel_id = egui::Id::new("smart-selection-rule");
    let mut selected: Option<usize> = ui.data(|d| d.get_temp(sel_id)).filter(|i: &usize| *i < rules.len());
    let mut changed = false;
    ui.label("Smart Selection rules: the best match at a click (precision times length) wins.");
    egui::Grid::new("smart-rules").striped(true).num_columns(3).show(ui, |ui| {
        ui.strong("Notes");
        ui.strong("Regular expression");
        ui.strong("Precision");
        ui.end_row();
        for (i, row) in rule_rows(&rules).iter().enumerate() {
            if ui.selectable_label(selected == Some(i), &row[0]).clicked() {
                selected = Some(i);
            }
            ui.monospace(&row[1]);
            ui.label(&row[2]);
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("+").clicked() {
            rules.push(Rule { regex: String::new(), precision: 1.0, notes: "New rule".into(), actions: vec![] });
            selected = Some(rules.len() - 1);
            changed = true;
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("-")).clicked() {
            if let Some(i) = selected.take() {
                rules.remove(i);
                changed = true;
            }
        }
    });
    if let Some(i) = selected {
        let r = &mut rules[i];
        ui.separator();
        egui::Grid::new("smart-rule").num_columns(2).show(ui, |ui| {
            ui.label("Notes");
            changed |= ui.text_edit_singleline(&mut r.notes).changed();
            ui.end_row();
            ui.label("Regular expression");
            changed |= ui.add(egui::TextEdit::singleline(&mut r.regex).code_editor().desired_width(360.0)).changed();
            ui.end_row();
            ui.label("Precision");
            let mut p = smartsel::precision_name(r.precision).to_string();
            let names: Vec<(&str, &str)> = smartsel::PRECISIONS.iter().map(|(n, _)| (*n, *n)).collect();
            if combo(ui, "smart-precision", &names, &mut p) {
                r.precision = smartsel::precision_of(&p);
                changed = true;
            }
            ui.end_row();
        });
        if let Err(e) = fancy_regex::Regex::new(&smartsel::to_rust_regex(&r.regex)) {
            ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), format!("This expression does not compile, so the rule is skipped: {e}"));
        }
        ui.label("Actions (\\0 is the match, \\1 to \\8 its groups, \\d the folder, \\h the host, \\u the user):");
        let mut remove = None;
        for (j, a) in r.actions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut a.title).hint_text("Title").desired_width(120.0)).changed();
                changed |= combo(ui, &format!("smart-action-{j}"), &RULE_ACTIONS, &mut a.action);
                changed |= ui.add(egui::TextEdit::singleline(&mut a.parameter).hint_text("Parameter").desired_width(220.0)).changed();
                if ui.button("-").clicked() {
                    remove = Some(j);
                }
            });
        }
        if let Some(j) = remove {
            r.actions.remove(j);
            changed = true;
        }
        if ui.button("Add action").clicked() {
            r.actions.push(RuleAction { title: "Open".into(), action: "openUrl:".into(), parameter: "\\0".into() });
            changed = true;
        }
    }
    ui.data_mut(|d| match selected {
        Some(i) => {
            d.insert_temp(sel_id, i);
        }
        None => d.remove::<usize>(sel_id),
    });
    if changed {
        profile.set("Smart Selection Rules", rules_to_value(&rules));
    }
    changed
}

/// The Semantic History editor. Returns true when it changed.
pub fn semantic_history(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let mut t = profile.get("Semantic History").and_then(|v| v.as_table().cloned()).unwrap_or_default();
    let get = |t: &toml::Table, k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let mut action = get(&t, "action");
    if action.is_empty() {
        action = "best editor".into();
    }
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Win-click on a file:");
        changed |= combo(ui, "semantic-action", &HISTORY_ACTIONS, &mut action);
    });
    let (key, hint) = match action.as_str() {
        "editor" => ("editor", "Editor program, for example C:\\Program Files\\Notepad++\\notepad++.exe"),
        "url" => ("text", "URL: \\1 file, \\2 line, \\3 text before, \\4 text after, \\5 folder"),
        "command" | "raw command" | "coprocess" => ("text", "Command for cmd.exe: \\1 file, \\2 line, \\3 before, \\4 after, \\5 folder"),
        "send text" => ("text", "Text to send: \\1 file, \\2 line, \\3 before, \\4 after, \\5 folder"),
        _ => ("", ""),
    };
    let mut value = get(&t, key);
    if !key.is_empty() {
        changed |= ui.add(egui::TextEdit::singleline(&mut value).hint_text(hint).desired_width(f32::INFINITY)).changed();
    }
    if changed {
        t.insert("action".into(), Value::String(action));
        if !key.is_empty() {
            t.insert(key.into(), Value::String(value));
        }
        profile.set("Semantic History", Value::Table(t));
    }
    changed
}

/// The rules table's rows: notes, regex, precision.
pub fn rule_rows(rules: &[Rule]) -> Vec<[String; 3]> {
    rules.iter().map(|r| [r.notes.clone(), r.regex.clone(), smartsel::precision_name(r.precision).to_string()]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_rows_round_trip() {
        let rules = vec![
            Rule { regex: "\\S+".into(), precision: 0.001, notes: "Word".into(), actions: vec![] },
            Rule {
                regex: "x(\\d+)".into(),
                precision: 1000.0,
                notes: "Ticket".into(),
                actions: vec![RuleAction { title: "Open".into(), action: "openUrl:".into(), parameter: "https://t/\\1".into() }],
            },
        ];
        let v = rules_to_value(&rules);
        assert_eq!(smartsel::rules_from(&v), rules);
        assert_eq!(rule_rows(&rules)[1], ["Ticket".to_string(), "x(\\d+)".to_string(), "high".to_string()]);
        let stored = v.as_array().unwrap()[0].as_table().unwrap().clone();
        assert!(!stored.contains_key("actions"), "a rule with no actions stores none, as iTerm2 does");
        assert_eq!(stored["precision"].as_str(), Some("low"));
    }
}
