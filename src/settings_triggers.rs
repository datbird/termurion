//! Profiles > Advanced > Triggers: iTerm2's list and detail editor.

use std::cell::Cell;

thread_local! {
    /// Which row the editor selects next time it draws: set by Add Trigger... and Edit
    /// Triggers (the last row, or the first).
    static FOCUS: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Add Trigger...: the editor shows the new (last) trigger.
pub fn focus_last() {
    FOCUS.with(|f| f.set(Some(true)));
}

/// Edit Triggers: the editor shows the first trigger.
pub fn focus_first() {
    FOCUS.with(|f| f.set(Some(false)));
}

/// The row the editor should select now, once.
pub fn take_focus(len: usize) -> Option<usize> {
    let f = FOCUS.with(|f| f.take())?;
    (len > 0).then(|| if f { len - 1 } else { 0 })
}

use alacritty_terminal::vte::ansi::Rgb;

use crate::profile::Profile;
use crate::triggers::{self, ActionInfo, Kind, Trigger};

/// How a trigger shows in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub detail: String,
    pub faint: bool,
}

/// An action's menu title, with the milestone that brings it when it is not here yet.
pub fn action_label(a: &ActionInfo) -> String {
    match a.later {
        Some(0) => format!("{} (later)", a.title),
        Some(m) => format!("{} (milestone {m})", a.title),
        None => a.title.to_string(),
    }
}

/// An event's menu title, with the milestone that brings it when it is not here yet.
pub fn event_label(e: crate::trigger_events::EventKind) -> String {
    match e.later() {
        Some(m) => format!("{} (milestone {m}c)", e.name()),
        None => e.name().to_string(),
    }
}

/// An event's filter fields (iTerm2's eventParams). True when one changed.
fn event_params(ui: &mut egui::Ui, kind: crate::trigger_events::EventKind, t: &mut Trigger) -> bool {
    use crate::trigger_events::EventKind as E;
    let mut params = t.extra.get("eventParams").and_then(|v| v.as_table()).cloned().unwrap_or_default();
    let mut changed = false;
    let text = |ui: &mut egui::Ui, key: &str, label: &str, hint: &str, params: &mut toml::Table| {
        let mut v = params.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
        ui.label(label);
        let c = ui.add(egui::TextEdit::singleline(&mut v).hint_text(hint).desired_width(300.0)).changed();
        ui.end_row();
        if c {
            params.insert(key.into(), v.into());
        }
        c
    };
    let number = |ui: &mut egui::Ui, key: &str, label: &str, default: f64, params: &mut toml::Table| {
        let mut v = crate::trigger_events::param_secs(Some(params), key, default);
        ui.label(label);
        let c = ui.add(egui::DragValue::new(&mut v).range(1.0..=86400.0).suffix(" s")).changed();
        ui.end_row();
        if c {
            params.insert(key.into(), toml::Value::Float(v));
        }
        c
    };
    match kind {
        E::CommandFinished => changed |= text(ui, "exitCodeFilter", "Exit code:", "* any, 0, !0, or a number", &mut params),
        E::DirectoryChanged => changed |= text(ui, "directoryRegex", "Directory:", "Regular expression (empty: any)", &mut params),
        E::HostChanged => changed |= text(ui, "hostRegex", "Host:", "Regular expression (empty: any)", &mut params),
        E::UserChanged => changed |= text(ui, "userRegex", "User:", "Regular expression (empty: any)", &mut params),
        E::Idle | E::ActivityAfterIdle => changed |= number(ui, "timeout", "Quiet for:", 30.0, &mut params),
        E::LongRunningCommand => {
            changed |= number(ui, "threshold", "Running for:", 60.0, &mut params);
            changed |= text(ui, "commandRegex", "Command:", "Regular expression (empty: any)", &mut params);
        }
        E::CustomEscapeSequence => changed |= text(ui, "sequenceId", "Sequence id:", "The id= the program sends", &mut params),
        E::NotificationPosted => changed |= text(ui, "messageRegex", "Message:", "Regular expression (empty: any)", &mut params),
        E::ProgressBarChanged => {
            let cur = params.get("progressBarFilter").and_then(|v| v.as_str()).unwrap_or("*").to_string();
            ui.label("When:");
            egui::ComboBox::from_id_salt("trigger-progress").selected_text(match cur.as_str() {
                "appeared" => "Appeared",
                "disappeared" => "Disappeared",
                _ => "Either",
            }).show_ui(ui, |ui| {
                for (label, v) in [("Either", "*"), ("Appeared", "appeared"), ("Disappeared", "disappeared")] {
                    if ui.selectable_label(cur == v, label).clicked() {
                        params.insert("progressBarFilter".into(), v.into());
                        changed = true;
                    }
                }
            });
            ui.end_row();
        }
        E::VariableChanged => {
            changed |= text(ui, "variableName", "Variable:", "user.name", &mut params);
            changed |= text(ui, "variableValueRegex", "Value:", "Regular expression (empty: any)", &mut params);
        }
        E::JobStarted | E::JobEnded => changed |= text(ui, "jobName", "Job:", "Program name", &mut params),
        E::PromptDetected | E::SessionEnded | E::BellReceived => {}
    }
    if changed {
        t.extra.insert("eventParams".into(), toml::Value::Table(params));
    }
    changed
}

/// iTerm2's list row: the name, then Instant, the job, `/regex/`, and the action with its
/// parameter; disabled rows faint.
pub fn row(t: &Trigger) -> Row {
    let mut detail = String::new();
    if t.instant {
        detail.push_str("Instant. ");
    }
    if !t.job.is_empty() {
        detail.push_str(&format!("Job: {} ", t.job));
    }
    let action = triggers::action_info(&t.action).map_or_else(|| format!("{} (not in Tandem)", t.action), action_label);
    match crate::trigger_events::EventKind::from_match_type(t.match_type) {
        Some(e) => detail.push_str(&format!("Event: {} {action}", event_label(e))),
        None => detail.push_str(&format!("/{}/ {action}", t.regex)),
    }
    if !t.parameter.is_empty() {
        detail.push(' ');
        detail.push_str(&t.parameter.replace('\u{1}', " "));
    }
    Row { name: t.name.clone(), detail, faint: !t.enabled }
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

pub fn two_colors(fg: Option<Rgb>, bg: Option<Rgb>) -> String {
    format!("{{{},{}}}", fg.map(hex).unwrap_or_default(), bg.map(hex).unwrap_or_default())
}

pub fn two_strings(a: &str, b: &str) -> String {
    format!("{a}\u{1}{b}")
}

pub fn split_two(v: &str) -> (String, String) {
    let (a, b) = v.split_once('\u{1}').unwrap_or((v, ""));
    (a.to_string(), b.to_string())
}

pub fn tab_status(dot: &str, text: &str, status: &str) -> String {
    format!("{dot}\u{1}{text}\u{1}{status}")
}

pub fn split_tab_status(v: &str) -> (String, String, String) {
    let mut p = v.splitn(3, '\u{1}');
    (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string())
}

pub fn setting(key: &str, on: bool) -> String {
    format!("{key}\u{1}{}", if on { 1 } else { 0 })
}

pub fn split_setting(v: &str) -> (String, bool) {
    let (k, on) = split_two(v);
    (k, on.trim() == "1")
}

fn to_color32(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

fn from_color32(c: egui::Color32) -> Rgb {
    Rgb { r: c.r(), g: c.g(), b: c.b() }
}

/// A color that may be unset: a check box, then the picker.
fn optional_color(ui: &mut egui::Ui, label: &str, color: &mut Option<Rgb>, default: Rgb) -> bool {
    let mut changed = false;
    let mut on = color.is_some();
    if ui.checkbox(&mut on, label).changed() {
        *color = on.then_some(color.unwrap_or(default));
        changed = true;
    }
    if let Some(c) = color.as_mut() {
        let mut c32 = to_color32(*c);
        if ui.color_edit_button_srgba(&mut c32).changed() {
            *c = from_color32(c32);
            changed = true;
        }
    }
    changed
}

/// The parameter widget for an action's kind. True when it changed.
fn parameter(ui: &mut egui::Ui, info: &ActionInfo, value: &mut String) -> bool {
    match info.kind {
        Kind::None => {
            ui.weak("No parameter");
            false
        }
        Kind::Text => ui.add(egui::TextEdit::singleline(value).hint_text(info.placeholder).desired_width(360.0)).changed(),
        Kind::Popup(choices) => {
            let current: i64 = value.trim().parse().unwrap_or(choices[0].1);
            let title = choices.iter().find(|(_, n)| *n == current).map_or(choices[0].0, |(t, _)| t);
            let mut changed = false;
            egui::ComboBox::from_id_salt("trigger-popup").selected_text(title).show_ui(ui, |ui| {
                for (t, n) in choices {
                    if ui.selectable_label(*n == current, *t).clicked() {
                        *value = n.to_string();
                        changed = true;
                    }
                }
            });
            changed
        }
        Kind::TwoColors => {
            let (mut fg, mut bg) = crate::trigger_eval::highlight_colors(value);
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= optional_color(ui, "Text color", &mut fg, Rgb { r: 255, g: 255, b: 255 });
                changed |= optional_color(ui, "Background color", &mut bg, Rgb { r: 255, g: 0, b: 0 });
            });
            if changed {
                *value = two_colors(fg, bg);
            }
            changed
        }
        Kind::TwoStrings => {
            let (mut a, mut b) = split_two(value);
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut a).hint_text("Variable name").desired_width(140.0)).changed();
                changed |= ui.add(egui::TextEdit::singleline(&mut b).hint_text(info.placeholder).desired_width(200.0)).changed();
            });
            if changed {
                *value = two_strings(&a, &b);
            }
            changed
        }
        Kind::TabStatus => {
            let (dot, text, mut status) = split_tab_status(value);
            let mut dot = crate::tabstatus::xterm_color(&dot);
            let mut text = crate::tabstatus::xterm_color(&text);
            let mut changed = false;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("trigger-tab-status").selected_text(match status.as_str() {
                    "wait" => "Waiting",
                    "idle" => "Idle",
                    _ => "Working",
                }).show_ui(ui, |ui| {
                    for (label, v) in [("Working", "work"), ("Waiting", "wait"), ("Idle", "idle")] {
                        if ui.selectable_label(status == v, label).clicked() {
                            status = v.to_string();
                            changed = true;
                        }
                    }
                });
                changed |= optional_color(ui, "Dot", &mut dot, Rgb { r: 0, g: 200, b: 0 });
                changed |= optional_color(ui, "Text", &mut text, Rgb { r: 255, g: 255, b: 255 });
            });
            if changed {
                *value = tab_status(&dot.map(hex).unwrap_or_default(), &text.map(hex).unwrap_or_default(), if status.is_empty() { "work" } else { &status });
            }
            changed
        }
        Kind::Setting => {
            let (mut key, mut on) = split_setting(value);
            let mut changed = false;
            ui.horizontal(|ui| {
                let label = crate::schema::find(&key).map_or(key.clone(), |s| s.label.clone());
                egui::ComboBox::from_id_salt("trigger-setting").selected_text(label).width(260.0).show_ui(ui, |ui| {
                    for s in crate::schema::profile().iter().filter(|s| s.kind == crate::schema::Kind::Bool) {
                        if ui.selectable_label(s.key == key, &s.label).clicked() {
                            key = s.key.clone();
                            changed = true;
                        }
                    }
                });
                changed |= ui.checkbox(&mut on, "On").changed();
            });
            if changed {
                *value = setting(&key, on);
            }
            changed
        }
    }
}

/// The Triggers editor: the list with + and -, and the selected trigger's fields. True when
/// the profile's triggers changed.
pub fn editor(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let mut list = profile.get("Triggers").map(|v| triggers::parse_list(&v)).unwrap_or_default();
    let interpolated = profile.bool("Triggers Use Interpolated Strings");
    let sel_id = egui::Id::new("trigger-selected");
    let mut selected: Option<usize> = take_focus(list.len()).or_else(|| ui.data(|d| d.get_temp(sel_id))).filter(|i: &usize| *i < list.len());
    let mut changed = false;
    ui.label("Triggers act on output that matches a regular expression.");
    egui::ScrollArea::vertical().max_height(220.0).id_salt("trigger-list").show(ui, |ui| {
        for (i, t) in list.iter().enumerate() {
            let r = row(t);
            let text = egui::RichText::new(format!("{}  {}", if r.name.is_empty() { "(no name)" } else { &r.name }, r.detail));
            let text = if r.faint { text.weak() } else { text };
            if ui.selectable_label(selected == Some(i), text).clicked() {
                selected = Some(i);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("+").clicked() {
            list.push(Trigger { regex: String::new(), action: "BounceTrigger".into(), parameter: "0".into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() });
            selected = Some(list.len() - 1);
            changed = true;
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("-")).clicked() {
            if let Some(i) = selected.take() {
                list.remove(i);
                changed = true;
            }
        }
    });
    match selected {
        None => {
            ui.weak("Select one trigger to view or edit");
        }
        Some(i) => {
            let t = &mut list[i];
            ui.separator();
            egui::Grid::new("trigger-detail").num_columns(2).show(ui, |ui| {
                ui.label("Name:");
                changed |= ui.text_edit_singleline(&mut t.name).changed();
                ui.end_row();
                ui.label("Match Against:");
                let event = crate::trigger_events::EventKind::from_match_type(t.match_type);
                egui::ComboBox::from_id_salt("trigger-match").selected_text(event.map_or("Regular Expression".to_string(), event_label)).width(260.0).show_ui(ui, |ui| {
                    if ui.selectable_label(event.is_none(), "Regular Expression").clicked() {
                        t.match_type = 0;
                        changed = true;
                    }
                    ui.separator();
                    let mut all = crate::trigger_events::EventKind::ALL.to_vec();
                    all.sort_by_key(|e| e.name());
                    for e in all {
                        if ui.selectable_label(event == Some(e), event_label(e)).clicked() {
                            t.match_type = e.match_type();
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                match event {
                    Some(e) => changed |= event_params(ui, e, t),
                    None => {
                        ui.label("Regular Expression:");
                        changed |= ui.add(egui::TextEdit::singleline(&mut t.regex).code_editor().desired_width(360.0)).changed();
                        ui.end_row();
                    }
                }
                ui.label("Job:");
                changed |= ui.add(egui::TextEdit::singleline(&mut t.job).hint_text("Trigger enabled only for this job (e.g., emacs)").desired_width(360.0)).changed();
                ui.end_row();
                ui.label("Action:");
                let current = triggers::action_info(&t.action).map_or_else(|| t.action.clone(), action_label);
                egui::ComboBox::from_id_salt("trigger-action").selected_text(current).width(260.0).show_ui(ui, |ui| {
                    for a in triggers::CATALOG {
                        if ui.selectable_label(triggers::action_info(&t.action).is_some_and(|c| c.class == a.class), action_label(a)).clicked() {
                            t.action = a.class.to_string();
                            t.parameter = match a.kind {
                                Kind::Popup(p) => p[0].1.to_string(),
                                _ => String::new(),
                            };
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Parameters:");
                match triggers::action_info(&t.action) {
                    Some(info) => changed |= parameter(ui, info, &mut t.parameter),
                    None => changed |= ui.text_edit_singleline(&mut t.parameter).changed(),
                }
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut t.instant, "Instant").changed();
                    changed |= ui.checkbox(&mut t.enabled, "Enabled").changed();
                });
                ui.end_row();
            });
            if let (0, Err(e)) = (t.match_type, fancy_regex::Regex::new(&t.regex)) {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), format!("This expression does not compile, so the trigger never fires: {e}"));
            }
            // An interpolated parameter that cannot be evaluated (iTerm2: Error in Trigger Parameter).
            if interpolated && t.parameter.contains("\\(") {
                if let Err(e) = crate::interp::evaluate(&t.parameter, &|_| None, &[]) {
                    ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), format!("Error in Trigger Parameter: {e}"));
                }
            }
            if !t.job.is_empty() {
                ui.weak("Job triggers fire once Tandem knows the program in front (milestone 6c).");
            }
        }
    }
    ui.data_mut(|d| match selected {
        Some(i) => {
            d.insert_temp(sel_id, i);
        }
        None => d.remove::<usize>(sel_id),
    });
    if changed {
        profile.set("Triggers", triggers::to_value(&list));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::Trigger;
    use alacritty_terminal::vte::ansi::Rgb;

    fn trig(name: &str, regex: &str, action: &str, parameter: &str) -> Trigger {
        Trigger { regex: regex.into(), action: action.into(), parameter: parameter.into(), instant: false, enabled: true, name: name.into(), job: String::new(), match_type: 0, extra: toml::Table::new() }
    }

    #[test]
    fn editor_rows_follow_iterm2() {
        let mut t = trig("errors", "err", "HighlightTrigger", "{#ff0000,}");
        let r = row(&t);
        assert_eq!((r.name.as_str(), r.detail.as_str(), r.faint), ("errors", "/err/ Highlight Text\u{2026} {#ff0000,}", false));
        t.job = "vim".into();
        t.instant = true;
        t.enabled = false;
        let r = row(&t);
        assert_eq!(r.detail, "Instant. Job: vim /err/ Highlight Text\u{2026} {#ff0000,}");
        assert!(r.faint, "disabled rows are faint");
        assert_eq!(row(&trig("", "x", "CaptureTrigger", "")).detail, "/x/ Capture Output (milestone 9)");
        assert_eq!(row(&trig("", "x", "NoSuch", "")).detail, "/x/ NoSuch (not in Tandem)");
        assert_eq!(action_label(crate::triggers::action_info("iTermEnterWorkgroupTrigger").unwrap()), "Enter Workgroup\u{2026} (later)");
        let mut ev = trig("", "", "BellTrigger", "");
        ev.match_type = 105;
        assert_eq!(row(&ev).detail, "Event: Idle (Silence) Ring Bell");
        ev.match_type = 113;
        assert_eq!(row(&ev).detail, "Event: Job Started (milestone 6c) Ring Bell");
    }

    #[test]
    fn parameter_encodings_round_trip() {
        let red = Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(two_colors(Some(red), None), "{#ff0000,}");
        assert_eq!(crate::trigger_eval::highlight_colors(&two_colors(Some(red), Some(red))), (Some(red), Some(red)));
        assert_eq!(two_strings("name", "value"), "name\u{1}value");
        assert_eq!(split_two("name\u{1}value"), ("name".to_string(), "value".to_string()));
        assert_eq!(split_two("only"), ("only".to_string(), String::new()));
        assert_eq!(tab_status("#00ff00", "#ffffff", "work"), "#00ff00\u{1}#ffffff\u{1}work");
        assert_eq!(split_tab_status("#00ff00\u{1}#ffffff\u{1}work"), ("#00ff00".to_string(), "#ffffff".to_string(), "work".to_string()));
        assert_eq!(setting("Silence Bell", true), "Silence Bell\u{1}1");
        assert_eq!(split_setting("Silence Bell\u{1}0"), ("Silence Bell".to_string(), false));
    }
}
