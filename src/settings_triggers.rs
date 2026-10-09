//! Profiles > Advanced > Triggers: iTerm2's list and detail editor.

use std::cell::Cell;

thread_local! {
    /// Which row the editor selects next time it draws: set by Add Trigger... and Edit
    /// Triggers (the last row, or the first).
    static FOCUS: Cell<Option<bool>> = const { Cell::new(None) };
    /// The alerts Import and Export raised this frame (heading, text), taken by the Settings window
    /// as `Edit::Alert`, one per frame.
    static ALERTS: std::cell::RefCell<std::collections::VecDeque<(String, String)>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    /// A file Export just wrote, for the Settings window to show in Explorer.
    static REVEAL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The next alert the editor raised, if any.
pub fn take_alert() -> Option<(String, String)> {
    ALERTS.with(|a| a.borrow_mut().pop_front())
}

/// Whether an alert waits for a frame to carry it.
pub fn alerts_pending() -> bool {
    ALERTS.with(|a| !a.borrow().is_empty())
}

/// The file an Export wrote, to show in Explorer.
pub fn take_reveal() -> Option<String> {
    REVEAL.with(|r| r.borrow_mut().take())
}

thread_local! {
    /// The workgroups the Enter Workgroup pop-up lists, (id, name), decoded from the stored value
    /// the last time it changed (the Settings window draws every frame).
    static WORKGROUPS: std::cell::RefCell<(String, Vec<(String, String)>)> = const { std::cell::RefCell::new((String::new(), Vec::new())) };
}

/// The configured workgroups as (id, name), from the stored "Workgroups" value (`EWT:availableWorkgroups`).
/// A value that does not decode lists none, as the model does.
pub fn workgroup_choices(raw: &str) -> Vec<(String, String)> {
    WORKGROUPS.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != raw {
            let list = if raw.trim().is_empty() { Vec::new() } else { crate::workgroup::decode(raw).unwrap_or_default() };
            *c = (raw.to_string(), list.into_iter().map(|w| (w.unique_identifier, w.name)).collect());
        }
        c.1.clone()
    })
}

/// Most alerts one Import raises (a folder of broken files must not queue windows forever).
const MAX_ALERTS: usize = 8;

fn push_alert(heading: &str, text: String) {
    ALERTS.with(|a| {
        let mut a = a.borrow_mut();
        if a.len() < MAX_ALERTS {
            a.push_back((heading.to_string(), text));
        }
    });
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
        E::JobStarted | E::JobEnded => changed |= text(ui, "jobName", "Job:", "claude", &mut params),
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
    row_with(t, &[])
}

/// `row`, with the configured workgroups so an Enter Workgroup trigger shows the workgroup's name
/// (`EWT:description`) and not its id.
pub fn row_with(t: &Trigger, workgroups: &[(String, String)]) -> Row {
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
    if t.action == "iTermEnterWorkgroupTrigger" {
        // The stored id is never shown: the label is the workgroup's name, "(unset)" or "(missing)".
        detail.push(' ');
        detail.push_str(&triggers::enter_workgroup_label(&t.parameter, workgroups));
    } else if !t.parameter.is_empty() {
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
fn parameter(ui: &mut egui::Ui, info: &ActionInfo, value: &mut String, workgroups: &[(String, String)]) -> bool {
    match info.kind {
        Kind::Workgroup => {
            // The pop-up of the configured workgroups by name; an empty parameter shows (and
            // enters) the first, as the pop-up does (`EWT:effectiveID`).
            let shown = triggers::enter_workgroup_label(value, workgroups);
            let current = triggers::effective_workgroup(value, workgroups);
            let mut changed = false;
            egui::ComboBox::from_id_salt("trigger-workgroup").selected_text(shown).width(360.0).show_ui(ui, |ui| {
                for (id, name) in triggers::sorted_workgroups(workgroups) {
                    let label = crate::inert_files::display_text(if name.is_empty() { "Untitled" } else { &name }, 128, false);
                    if ui.selectable_label(current.as_deref() == Some(id.as_str()), label).clicked() {
                        *value = id;
                        changed = true;
                    }
                }
            });
            changed
        }
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
        Kind::Accounts => {
            // The password manager's cached display strings, never fetched here (iTerm2's
            // pop-up of `cachedCombinedAccountNames`). The placeholder stands for no account.
            let choices = crate::pwm_prompt::trigger_choices(&crate::pwm_prompt::cached_names(), value);
            let shown = crate::pwm_prompt::trigger_param(value).unwrap_or_else(|| crate::pwm_prompt::UNLOCK.to_string());
            let mut changed = false;
            egui::ComboBox::from_id_salt("trigger-accounts").selected_text(shown.clone()).width(360.0).show_ui(ui, |ui| {
                for name in choices {
                    if ui.selectable_label(name == shown, &name).clicked() {
                        *value = name;
                        changed = true;
                    }
                }
            });
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

/// The editor's Import (`TriggerController.m:1171-1203`): each file's text parsed in order and its
/// triggers merged into the stored list; each failure is an alert (heading, text), and a file that
/// failed adds nothing. `files` are (shown name, the text or the reason it could not be read).
pub fn import_into(stored: Option<&toml::Value>, files: &[(String, Result<String, String>)]) -> (toml::Value, Vec<(String, String)>) {
    use crate::trigger_file as tf;
    let mut list = stored.cloned().unwrap_or_else(|| toml::Value::Array(Vec::new()));
    let mut alerts = Vec::new();
    for (name, text) in files {
        match text {
            Err(e) => alerts.push((tf::IMPORT_FAILED_HEADING.to_string(), tf::load_error_text(name, e))),
            Ok(text) => match tf::parse(text, name) {
                Ok(imported) => {
                    list = tf::add_triggers(Some(&list), &imported);
                    let off = tf::off_count(&imported);
                    if off > 0 {
                        alerts.push((tf::IMPORTED_OFF_HEADING.to_string(), tf::off_text(off)));
                    }
                }
                Err(e) => alerts.push((e.heading.to_string(), e.text)),
            },
        }
    }
    (list, alerts)
}

/// The text of a trigger file the person picked: at most `MAX_FILE` bytes, read through the judge's
/// type, valid UTF-8 (`TriggerController.m:580-600`). The error is the reason, for "While loading".
pub fn read_text(p: &crate::localfs::LocalPath) -> Result<String, String> {
    use std::io::Read;
    let file = crate::localfs::open(p).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(crate::trigger_file::MAX_FILE as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > crate::trigger_file::MAX_FILE {
        return Err("the file is too large for a trigger file".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not valid UTF-8".to_string())
}

/// Writes an exported file: a new file readable by this user alone, made beside the target and moved
/// over it, so an existing file (or a link planted there) is replaced as itself, never written
/// through. The path is the person's pick.
pub fn write_export(path: &crate::localfs::LocalPath, text: &str) -> std::io::Result<()> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "this is not a file name");
    let name = path.path().file_name().ok_or_else(bad)?.to_os_string();
    let folder = path.parent().ok_or_else(bad)?;
    let target = folder.unfollowed(&name).ok_or_else(bad)?;
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut temp_name = std::ffi::OsString::from(".");
        temp_name.push(&name);
        temp_name.push(format!(".{}.tmp", std::process::id()));
        let temp = folder.unfollowed(&temp_name).ok_or_else(bad)?;
        match crate::localfs::remove_unfollowed(&temp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let written = crate::localfs::create_owner_only(&temp).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()));
        let moved = written.and_then(|()| crate::localfs::rename_unfollowed(&temp, &target));
        if moved.is_err() {
            let _ = crate::localfs::remove_unfollowed(&temp);
        }
        moved
    }
    #[cfg(not(windows))]
    {
        let _ = target;
        crate::localfs::write(path, text)
    }
}

/// "Export to File" (`TriggerController.m:1139-1169`): the trigger as a one-element array in a file
/// the person names. A failure is an "Export Failed" alert; a file written is shown in Explorer.
fn export_one(t: &Trigger) {
    use crate::trigger_file as tf;
    let mut table = t.to_table();
    for k in ["name", "job"] {
        if table.get(k).and_then(|v| v.as_str()).is_some_and(str::is_empty) {
            table.remove(k);
        }
    }
    let text = match tf::export_file_text(&table) {
        Ok(text) => text,
        Err(e) => return push_alert(tf::EXPORT_FAILED_HEADING, e),
    };
    let dialog = rfd::FileDialog::new().set_title(tf::EXPORT_TITLE).set_file_name(format!("triggers.{}", tf::EXTENSION)).add_filter("iTerm2 triggers", &[tf::EXTENSION]);
    let Some(path) = crate::localfs::save_file(dialog) else { return };
    let shown = crate::inert_files::shown_name(&path.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
    match write_export(&path, &text) {
        Ok(()) => REVEAL.with(|r| *r.borrow_mut() = Some(path.path().to_string_lossy().into_owned())),
        Err(e) => push_alert(tf::EXPORT_FAILED_HEADING, tf::save_error_text(&shown, &e.to_string())),
    }
}

/// The Import button (`TriggerController.m:1171-1203`): files the person picks, each read and merged
/// into the profile's list. True when the list changed.
fn import_button(profile: &mut Profile) -> bool {
    use crate::trigger_file as tf;
    let dialog = rfd::FileDialog::new().set_title(tf::IMPORT_TITLE).add_filter("iTerm2 triggers", &[tf::EXTENSION]);
    let files: Vec<(String, Result<String, String>)> = crate::localfs::pick_files(dialog)
        .iter()
        .map(|p| (crate::inert_files::shown_name(&p.path().file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())), read_text(p)))
        .collect();
    let stored = profile.get("Triggers");
    let (merged, alerts) = import_into(stored.as_ref(), &files);
    for (heading, text) in alerts {
        push_alert(&heading, text);
    }
    let changed = stored.as_ref() != Some(&merged) && !(stored.is_none() && merged.as_array().is_some_and(Vec::is_empty));
    if changed {
        profile.set("Triggers", merged.clone());
        let dropped = tf::dropped_by_limit(&merged, profile.get("Triggers").as_ref());
        if dropped > 0 {
            push_alert(tf::NOT_IMPORTED_HEADING, tf::not_kept_text(dropped));
        }
    }
    changed
}

/// The Triggers editor: the list with + and -, and the selected trigger's fields. True when
/// the profile's triggers changed.
pub fn editor(ui: &mut egui::Ui, profile: &mut Profile, workgroups: &[(String, String)]) -> bool {
    let mut list = profile.get("Triggers").map(|v| triggers::parse_list(&v)).unwrap_or_default();
    let interpolated = profile.bool("Triggers Use Interpolated Strings");
    let sel_id = egui::Id::new("trigger-selected");
    let mut selected: Option<usize> = take_focus(list.len()).or_else(|| ui.data(|d| d.get_temp(sel_id))).filter(|i: &usize| *i < list.len());
    let mut changed = false;
    let mut imported = false;
    ui.label("Triggers act on output that matches a regular expression.");
    egui::ScrollArea::vertical().max_height(220.0).id_salt("trigger-list").show(ui, |ui| {
        for (i, t) in list.iter().enumerate() {
            let r = row_with(t, workgroups);
            let text = egui::RichText::new(format!("{}  {}", if r.name.is_empty() { "(no name)" } else { &r.name }, r.detail));
            let text = if r.faint { text.weak() } else { text };
            let response = ui.selectable_label(selected == Some(i), text);
            if response.clicked() || response.secondary_clicked() {
                selected = Some(i);
            }
            // The row's menu (`TriggerController.m:1133-1135`).
            response.context_menu(|ui| {
                if ui.button(crate::trigger_file::EXPORT_TITLE).clicked() {
                    export_one(t);
                    ui.close();
                }
            });
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
        if ui.button(crate::trigger_file::IMPORT_TITLE).clicked() && import_button(profile) {
            // The list changed under the editor: it reads the stored one again next frame.
            imported = true;
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
                    // The workgroup actions need a live session: Session Ended is not offered for them.
                    let mut all = triggers::allowed_events(&t.action);
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
                // Job Started and Job Ended name their job above (iTerm2 disables this field).
                let job_event = matches!(event, Some(crate::trigger_events::EventKind::JobStarted | crate::trigger_events::EventKind::JobEnded));
                let hint = if job_event { "Set job above" } else { "Trigger enabled only for this job (e.g., emacs)" };
                changed |= ui.add_enabled(!job_event, egui::TextEdit::singleline(&mut t.job).hint_text(hint).desired_width(360.0)).changed();
                ui.end_row();
                ui.label("Action:");
                let current = triggers::action_info(&t.action).map_or_else(|| t.action.clone(), action_label);
                egui::ComboBox::from_id_salt("trigger-action").selected_text(current).width(260.0).show_ui(ui, |ui| {
                    for a in triggers::CATALOG {
                        if ui.selectable_label(triggers::action_info(&t.action).is_some_and(|c| c.class == a.class), action_label(a)).clicked() {
                            t.action = a.class.to_string();
                            t.parameter = match a.kind {
                                Kind::Popup(p) => p[0].1.to_string(),
                                // The first workgroup by name: what the pop-up shows now.
                                Kind::Workgroup => triggers::effective_workgroup("", workgroups).unwrap_or_default(),
                                _ => String::new(),
                            };
                            // A match the new action cannot use (Session Ended for a workgroup action) goes.
                            if !triggers::allows_match_type(&t.action, t.match_type) {
                                t.match_type = 0;
                            }
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Parameters:");
                match triggers::action_info(&t.action) {
                    Some(info) => changed |= parameter(ui, info, &mut t.parameter, workgroups),
                    None => changed |= ui.text_edit_singleline(&mut t.parameter).changed(),
                }
                ui.end_row();
                // Exit Workgroup's own option (`hasLeaderOnlyOption`): kept in the event params, only when on.
                if triggers::action_info(&t.action).is_some_and(|a| a.leader_only_option) {
                    ui.label("");
                    let mut params = t.extra.get("eventParams").and_then(|v| v.as_table()).cloned().unwrap_or_default();
                    let mut on = params.get(triggers::LEADER_ONLY_KEY).and_then(toml::Value::as_bool).unwrap_or(false);
                    let r = ui.checkbox(&mut on, "Only on the workgroup leader").on_hover_text("When set, the trigger fires only for the workgroup leader (the session that originally entered the workgroup). Peers such as the Code Review and Diff sessions do not, so reloading or quitting one keeps the workgroup open.");
                    if r.changed() {
                        if on {
                            params.insert(triggers::LEADER_ONLY_KEY.into(), toml::Value::Boolean(true));
                        } else {
                            params.remove(triggers::LEADER_ONLY_KEY);
                        }
                        t.extra.insert("eventParams".into(), toml::Value::Table(params));
                        changed = true;
                    }
                    ui.end_row();
                }
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
                if let Err(e) = crate::interp::check(&t.parameter) {
                    ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), format!("Error in Trigger Parameter: {e}"));
                }
            }
        }
    }
    ui.data_mut(|d| match selected {
        Some(i) => {
            d.insert_temp(sel_id, i);
        }
        None => d.remove::<usize>(sel_id),
    });
    if changed && !imported {
        // Entries the list could not read stay in place; untouched triggers stay as stored.
        let stored = profile.get("Triggers");
        profile.set("Triggers", triggers::merge_list(stored.as_ref(), &list));
    }
    changed || imported
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
        assert_eq!(row(&trig("", "x", "CaptureTrigger", "")).detail, "/x/ Capture Output");
        assert_eq!(row(&trig("", "x", "NoSuch", "")).detail, "/x/ NoSuch (not in Tandem)");
        assert_eq!(action_label(crate::triggers::action_info("iTermEnterWorkgroupTrigger").unwrap()), "Enter Workgroup\u{2026}");
        let ew = trig("", "x", "iTermEnterWorkgroupTrigger", "gid");
        assert_eq!(row_with(&ew, &[("gid".into(), "Chat".into())]).detail, "/x/ Enter Workgroup\u{2026} Chat", "the workgroup's name, never its id");
        assert_eq!(row_with(&ew, &[]).detail, "/x/ Enter Workgroup\u{2026} (missing)");
        let mut ev = trig("", "", "BellTrigger", "");
        ev.match_type = 105;
        assert_eq!(row(&ev).detail, "Event: Idle (Silence) Ring Bell");
        ev.match_type = 113;
        assert_eq!(row(&ev).detail, "Event: Job Started Ring Bell");
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

    /// T9 m3: the editor's Import says how many triggers came in turned off.
    #[test]
    fn editor_import_says_how_many_came_in_off() {
        let run = toml::Table::from_iter([("action".to_string(), "ScriptTrigger".into()), ("regex".to_string(), "x".into()), ("parameter".to_string(), "echo".into())]);
        let c = crate::trigger_file::canonical(&run).unwrap();
        let files = vec![("x.it2triggers".to_string(), Ok(crate::trigger_file::export_text(std::slice::from_ref(&c))))];
        let (merged, alerts) = import_into(None, &files);
        assert_eq!(merged.as_array().unwrap()[0].as_table().unwrap().get("disabled").and_then(toml::Value::as_bool), Some(true));
        assert_eq!(alerts, vec![(crate::trigger_file::IMPORTED_OFF_HEADING.to_string(), crate::trigger_file::off_text(1))]);
    }

    #[test]
    fn editor_import_merges_files_and_reports_failures() {
        let alert = |r: &str| toml::Table::from_iter([("action".to_string(), "AlertTrigger".into()), ("regex".to_string(), r.into()), ("parameter".to_string(), "hi".into())]);
        let a = crate::trigger_file::canonical(&alert("a")).unwrap();
        let c = crate::trigger_file::canonical(&alert("c")).unwrap();
        let stored = toml::Value::Array(vec![a.clone().into()]);
        let files = vec![("x.it2triggers".to_string(), Ok(crate::trigger_file::export_text(std::slice::from_ref(&c)))), ("y.it2triggers".to_string(), Err("denied".to_string()))];
        let (merged, alerts) = import_into(Some(&stored), &files);
        assert_eq!(merged, toml::Value::Array(vec![a.into(), c.into()]));
        assert_eq!(alerts, vec![("Import Failed".to_string(), "While loading y.it2triggers: denied".to_string())]);
        let bad = vec![("z.it2triggers".to_string(), Ok("5".to_string()))];
        let (kept, alerts) = import_into(Some(&stored), &bad);
        assert_eq!(kept, stored, "a file that failed adds nothing");
        assert_eq!(alerts, vec![("Import Failed".to_string(), "Malformed file at z.it2triggers".to_string())]);
    }

    #[cfg(windows)]
    #[test]
    fn export_writes_owner_only_and_replaces() {
        let dir = std::env::temp_dir().join(format!("tandem-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = crate::localfs::LocalPath::judge(dir.join("t.it2triggers")).expect("a local path");
        write_export(&file, "[1]").unwrap();
        write_export(&file, "[2]").unwrap();
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), "[2]");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "no temporary file stays: {names:?}");
        let sddl = crate::localfs::dacl_sddl(&file.parent().unwrap().unfollowed("t.it2triggers").unwrap()).unwrap();
        assert!(sddl.contains("D:P(A;;FA;;;"), "owner-only: {sddl}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
