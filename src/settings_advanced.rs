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

/// The Smart Selection editor's strings (`PreferencePanel.xib`, `SmartSelectionController.m`).
pub const HEADING: &str = "Smart Selection Rules";
pub const LOAD_DEFAULTS: &str = "Load Defaults";
pub const NO_RULE: &str = "No rule selected";
pub const NAME: &str = "Name";
pub const REGEX: &str = "Regular expression";
pub const PRECISION: &str = "Precision";
pub const ACTIONS: &str = "Actions\u{2026}";
pub const SYNTAX_HELP: &str = "Syntax Help";
pub const OPEN_VISUALIZER: &str = "Open Regular Expression Visualizer";
pub const OPEN_VISUALIZATION: &str = "Open Regular Expression Visualization";
pub const CLOSE_VISUALIZATION: &str = "Close Regular Expression Visualization";
pub const PLAYGROUND_PROMPT: &str = "Click on text in playground to test rules";
pub const NO_MATCH: &str = "No match";
pub const ACTIONS_HEADING: &str = "Smart Selection Actions";
pub const ACTIONS_NOTE: &str = "Items are added to the context menu when you right click on text matching this smart selection.";
/// The precision popup's items, lowest first (`_precisionButton`).
pub const PRECISION_NAMES: [&str; 5] = ["Very Low", "Low", "Normal", "High", "Very High"];
/// What the playground shows before a click.
const PLAYGROUND_SAMPLE: &str = "https://example.com/docs/page.html?id=42\nC:\\Users\\name\\notes.txt:12\nname@example.com  /usr/local/bin/tool\n";

/// The row of a rule in the table says how precise it is (`precisionAttributedString:actionCount:`).
pub fn precision_phrase(precision_name: &str, actions: usize) -> String {
    let level = match precision_name {
        "very_low" => "Very low",
        "low" => "Low",
        "normal" => "Normal",
        "high" => "High",
        "very_high" => "Very high",
        _ => "Undefined",
    };
    if actions >= 1 { format!("{level} precision, {actions} actions") } else { format!("{level} precision") }
}

/// The precision popup's item for a stored precision (the popup lists the five in `smartsel::PRECISIONS` order).
pub fn precision_index(precision: f64) -> usize {
    smartsel::PRECISIONS.iter().position(|(n, _)| *n == smartsel::precision_name(precision)).unwrap_or(2)
}

/// A new rule (`defaultRule`): matches nothing, normal precision, no actions.
fn new_rule() -> Rule {
    Rule { regex: String::new(), precision: 1.0, notes: String::new(), actions: vec![] }
}

/// The built-in rules, which "Load Defaults" puts back (`SmartSelectionController defaultRules`).
pub fn default_rules() -> Vec<Rule> {
    crate::schema::find("Smart Selection Rules").and_then(|s| s.default.as_ref()).map(smartsel::rules_from).unwrap_or_default()
}

/// What the playground says for a click on `text` at char `index` (`updatePlayground`): the note of the
/// rule that wins there, with what it matched, or "No match".
pub fn playground_result(text: &str, index: usize, rules: &[Rule]) -> String {
    let mut line_start = 0;
    let mut line = text;
    for (n, l) in text.split('\n').enumerate() {
        let len = l.chars().count();
        if index <= line_start + len || n + 1 == text.split('\n').count() {
            line = l;
            break;
        }
        line_start += len + 1;
    }
    let col = index.saturating_sub(line_start).min(line.chars().count());
    let byte = line.char_indices().nth(col).map_or(line.len(), |(b, _)| b);
    match smartsel::select_at(line, byte, rules) {
        Some(f) => format!("{}: {}", rules[f.rule].notes, &line[f.range]),
        None => NO_MATCH.to_string(),
    }
}

/// The Smart Selection rules editor (iTerm2's "Smart Selection Rules" panel): the rule table with
/// add, remove and Load Defaults, the selected rule's Name, Regular expression and Precision with
/// Syntax Help, the regular expression visualizer and Actions\u{2026}, and the playground. Returns
/// true when the rules changed; `open_url` takes a page to open.
pub fn smart_selection_rules(ui: &mut egui::Ui, profile: &mut Profile, open_url: &mut Option<String>) -> bool {
    let mut rules = smartsel::rules(profile);
    let sel_id = egui::Id::new("smart-selection-rule");
    let viz_id = egui::Id::new("smart-selection-visualizer");
    let actions_id = egui::Id::new("smart-selection-actions-open");
    let play_id = egui::Id::new("smart-selection-playground");
    let result_id = egui::Id::new("smart-selection-playground-result");
    let mut selected: Option<usize> = ui.data(|d| d.get_temp(sel_id)).filter(|i: &usize| *i < rules.len());
    let mut changed = false;
    ui.strong(HEADING);
    egui::ScrollArea::vertical().id_salt("smart-rules-scroll").max_height(200.0).show(ui, |ui| {
        for (i, r) in rules.iter().enumerate() {
            let fill = if selected == Some(i) { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
            let frame = egui::Frame::new().fill(fill).inner_margin(egui::Margin::same(3)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !r.notes.trim().is_empty() {
                    ui.label(egui::RichText::new(&r.notes).strong());
                }
                if !r.regex.is_empty() {
                    ui.label(egui::RichText::new(format!("/{}/", r.regex)).monospace());
                }
                ui.label(egui::RichText::new(precision_phrase(smartsel::precision_name(r.precision), r.actions.len())).strong());
            });
            if ui.interact(frame.response.rect, ui.id().with(("smart-rule-row", i)), egui::Sense::click()).clicked() {
                selected = Some(i);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("+").clicked() {
            rules.push(new_rule());
            selected = Some(rules.len() - 1);
            changed = true;
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("\u{2212}")).clicked()
            && let Some(i) = selected.take()
        {
            rules.remove(i);
            changed = true;
        }
        if ui.button(LOAD_DEFAULTS).clicked() {
            rules = default_rules();
            selected = None;
            changed = true;
        }
    });
    ui.separator();
    match selected {
        None => {
            ui.weak(NO_RULE);
        }
        Some(i) => {
            let r = &mut rules[i];
            egui::Grid::new("smart-rule").num_columns(2).show(ui, |ui| {
                ui.label(NAME);
                changed |= ui.add(egui::TextEdit::singleline(&mut r.notes).desired_width(360.0)).changed();
                ui.end_row();
                ui.label(REGEX);
                changed |= ui.add(egui::TextEdit::singleline(&mut r.regex).code_editor().desired_width(360.0)).changed();
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    if ui.button(SYNTAX_HELP).clicked() {
                        *open_url = Some(crate::links::HELP_SMART_SELECTION.to_string());
                    }
                    let open = ui.data(|d| d.get_temp::<bool>(viz_id)).unwrap_or(false);
                    let first_use = ui.data(|d| d.get_temp::<bool>(viz_id)).is_none();
                    let title = if open { CLOSE_VISUALIZATION } else if first_use { OPEN_VISUALIZER } else { OPEN_VISUALIZATION };
                    if ui.button(title).clicked() {
                        ui.data_mut(|d| d.insert_temp(viz_id, !open));
                    }
                });
                ui.end_row();
                ui.label(PRECISION);
                ui.horizontal(|ui| {
                    let now = precision_index(r.precision);
                    egui::ComboBox::from_id_salt("smart-precision").selected_text(PRECISION_NAMES[now]).show_ui(ui, |ui| {
                        for (k, name) in PRECISION_NAMES.iter().enumerate() {
                            if ui.selectable_label(k == now, *name).clicked() && k != now {
                                r.precision = smartsel::PRECISIONS[k].1;
                                changed = true;
                            }
                        }
                    });
                    if ui.button(ACTIONS).clicked() {
                        let open = ui.data(|d| d.get_temp::<bool>(actions_id)).unwrap_or(false);
                        ui.data_mut(|d| d.insert_temp(actions_id, !open));
                    }
                });
                ui.end_row();
            });
            if let Err(e) = fancy_regex::Regex::new(&smartsel::to_rust_regex(&r.regex)) {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), format!("This expression does not compile, so the rule is skipped: {e}"));
            }
            if ui.data(|d| d.get_temp::<bool>(viz_id)).unwrap_or(false) {
                egui::ScrollArea::horizontal().id_salt("smart-visualizer").show(ui, |ui| crate::regex_outline::show(ui, &crate::regex_outline::parse(&r.regex)));
            }
            if ui.data(|d| d.get_temp::<bool>(actions_id)).unwrap_or(false) {
                ui.separator();
                ui.strong(ACTIONS_HEADING);
                ui.label(ACTIONS_NOTE);
                ui.label("Use \\0 for match, \\1\u{2026}\\9 for match groups, \\d for directory, \\u for user, \\h for host.");
                let mut remove = None;
                for (j, a) in r.actions.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= ui.add(egui::TextEdit::singleline(&mut a.title).hint_text("Enter Item Title").desired_width(120.0)).changed();
                        changed |= combo(ui, &format!("smart-action-{j}"), &RULE_ACTIONS, &mut a.action);
                        changed |= ui.add(egui::TextEdit::singleline(&mut a.parameter).hint_text("Parameter").desired_width(220.0)).changed();
                        if ui.button("\u{2212}").clicked() {
                            remove = Some(j);
                        }
                    });
                }
                if let Some(j) = remove {
                    r.actions.remove(j);
                    changed = true;
                }
                if ui.button("+").clicked() {
                    r.actions.push(RuleAction { title: "Open".into(), action: "openUrl:".into(), parameter: "\\0".into() });
                    changed = true;
                }
            }
        }
    }
    // The playground: click in the text to see which rule wins there.
    ui.separator();
    let mut sample: String = ui.data(|d| d.get_temp(play_id)).unwrap_or_else(|| PLAYGROUND_SAMPLE.to_string());
    let out = egui::TextEdit::multiline(&mut sample).code_editor().desired_rows(4).desired_width(f32::INFINITY).show(ui);
    if (out.response.clicked() || out.response.changed())
        && let Some(range) = out.cursor_range
    {
        let said = playground_result(&sample, range.primary.index.into(), &rules);
        ui.data_mut(|d| d.insert_temp(result_id, said));
    }
    let said: String = ui.data(|d| d.get_temp(result_id)).unwrap_or_else(|| PLAYGROUND_PROMPT.to_string());
    ui.label(egui::RichText::new(said).small());
    ui.data_mut(|d| {
        d.insert_temp(play_id, sample);
        match selected {
            Some(i) => {
                d.insert_temp(sel_id, i);
            }
            None => d.remove::<usize>(sel_id),
        }
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
    use crate::schema;

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

    #[test]
    fn smart_selection_editor_follows_iterm2() {
        assert_eq!(PRECISION_NAMES, ["Very Low", "Low", "Normal", "High", "Very High"]);
        assert_eq!(precision_phrase("normal", 0), "Normal precision");
        assert_eq!(precision_phrase("very_high", 3), "Very high precision, 3 actions");
        assert_eq!(precision_phrase("high", 1), "High precision, 1 actions", "iTerm2's text does not singularize");
        for (i, (name, value)) in smartsel::PRECISIONS.iter().enumerate() {
            assert_eq!(precision_index(*value), i, "{name}");
        }
        let defaults = default_rules();
        assert!(defaults.len() > 5 && defaults.iter().any(|r| r.notes == "Paths"));
    }

    #[test]
    fn the_playground_names_the_rule_that_wins_where_you_click() {
        let rules = default_rules();
        let text = "see https://example.com/a/b for more\nsecond line";
        let at = text.find("example").unwrap();
        let said = playground_result(text, at, &rules);
        assert!(said.contains("example.com/a/b"), "{said}");
        assert_eq!(playground_result("  ", 0, &[]), NO_MATCH);
        // A click on the second line reads that line, not the first.
        assert!(playground_result(text, text.find("second").unwrap() + 1, &rules).contains("second"));
    }

    /// Review Focus 10: the Apple-only rows are not offered and `aiTitle` is never set.
    #[test]
    fn apple_only_rows_are_hidden() {
        assert!(schema::find_global("aiGeneratedTabTitles").is_none());
        assert!(schema::find_global("logAITabTitleCorpus").is_none());
        let help = &schema::find("Title Components").unwrap().help;
        assert!(help.contains("not available on Windows"), "{help}");
        let out = crate::interp::evaluate("\\(session.aiTitle)", &|_| None, &[]);
        assert_eq!(out.as_deref(), Ok(""));
    }
}
