//! The Scripting Inspector (iTerm2's `iTermScriptInspector.xib`, opened by the Script
//! Console's Inspector button): the sessions, tabs and windows as an outline, the variables of
//! the selected one, and the registered functions with their roles and scripts.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::api_proto::pb::rpc_registration_request::Role;
use crate::egui_host::EguiHost;

/// The window's texts (`API/Base.lproj/iTermScriptInspector.xib`).
pub const TITLE: &str = "Scripting Inspector";
pub const OUTLINE: &str = "Sessions, Tabs, and Windows";
pub const VARIABLES: &str = "Variables";
pub const PATH: &str = "Path";
pub const VALUE: &str = "Value";
pub const FUNCTIONS: &str = "Registered Functions";
pub const SIGNATURE: &str = "Signature";
pub const ROLE: &str = "Role";
pub const SCRIPT: &str = "Script";
/// The outline's row menu, and the variables' (`iTermScriptInspector.xib:468-500`).
pub const REVEAL: &str = "Reveal";
pub const COPY_ID: &str = "Copy ID";
pub const COPY_VALUE: &str = "Copy Value";
pub const COPY_PATH: &str = "Copy Path";
/// The outline's root ("iTerm2") and the buried sessions' row
/// (`iTermSessionTabWindowOutlineDelegate.m:151`, `:212`).
pub const ROOT: &str = "Termurion";
pub const BURIED: &str = "Buried Sessions";
/// A function whose connection has no Script Console entry
/// (`iTermRegisteredFunctionsTableViewDelegate.m:69`).
pub const UNKNOWN: &str = "Unknown";

/// Segoe Fluent Icons glyphs for the rows, in place of iTerm2's emoji (Decision): a window,
/// a tab and a command prompt for a session.
const WINDOW_GLYPH: char = '\u{E737}';
const TAB_GLYPH: char = '\u{E8A5}';
const SESSION_GLYPH: char = '\u{E756}';

/// What the outline shows: each window (its `pty-` id, its title, and its tabs, each with
/// its id, label and sessions by id and name), then the buried sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineInput {
    pub windows: Vec<(String, String, Vec<(u64, String, Vec<(u64, String)>)>)>,
    pub buried: Vec<(u64, String)>,
}

/// An outline row's object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Object {
    App,
    Window(String),
    Tab(u64),
    Session(u64),
    Buried,
}

impl Object {
    /// What Copy ID copies (`copyIdentifier:`): a window's `pty-` id, a tab's or session's
    /// number. The app's identifier is NSNull and the buried row's is empty in iTerm2: none.
    pub fn identifier(&self) -> Option<String> {
        match self {
            Object::Window(id) => Some(id.clone()),
            Object::Tab(id) | Object::Session(id) => Some(id.to_string()),
            Object::App | Object::Buried => None,
        }
    }
}

fn session_row(depth: usize, id: u64, name: &str) -> (usize, String, Object) {
    (depth, format!("{SESSION_GLYPH} {name} ({id})"), Object::Session(id))
}

/// The outline, fully expanded (`iTermSessionTabWindowOutlineDelegate.m:56-212`): the root,
/// each window "title (id)", its tabs "label (id)", their sessions "name (id)", then "Buried
/// Sessions" with the buried ones when there are any.
pub fn outline(i: &OutlineInput) -> Vec<(usize, String, Object)> {
    let mut out = vec![(0, ROOT.to_string(), Object::App)];
    for (id, title, tabs) in &i.windows {
        out.push((1, format!("{WINDOW_GLYPH} {title} ({id})"), Object::Window(id.clone())));
        for (tab, label, sessions) in tabs {
            out.push((2, format!("{TAB_GLYPH} {label} ({tab})"), Object::Tab(*tab)));
            out.extend(sessions.iter().map(|(s, name)| session_row(3, *s, name)));
        }
    }
    if !i.buried.is_empty() {
        out.push((1, BURIED.to_string(), Object::Buried));
        out.extend(i.buried.iter().map(|(s, name)| session_row(2, *s, name)));
    }
    out
}

/// A registration's role as the functions table names it
/// (`iTermRegisteredFunctionsTableViewDelegate.m:52-67`); an unknown one reads "RPC".
pub fn role_name(role: i32) -> &'static str {
    match Role::try_from(role) {
        Ok(Role::SessionTitle) => "Title Provider",
        Ok(Role::StatusBarComponent) => "Status Bar Component",
        Ok(Role::ContextMenu) => "Context Menu Provider",
        Ok(Role::Generic) | Err(_) => "RPC",
    }
}

/// The functions table (`iTermRegisteredFunctionsTableViewDelegate.m:50-72`): each
/// registration's signature, role and script (the name `script_name` gives its connection,
/// "Unknown" when that is empty), sorted by signature (iTerm2 keeps its dictionary's order;
/// Decision).
pub fn function_rows(regs: &[(String, u64, i32)], script_name: &dyn Fn(u64) -> String) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = regs
        .iter()
        .map(|(sig, conn, role)| {
            let name = script_name(*conn);
            (sig.clone(), role_name(*role).to_string(), if name.is_empty() { UNKNOWN.to_string() } else { name })
        })
        .collect();
    out.sort();
    out
}

/// The variables table's rows: each path and its value, strings as they are and other values
/// as JSON, in path order.
pub fn variable_rows(values: &crate::api_vars::Values) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), text)
        })
        .collect()
}

/// What the Inspector shows (the app builds it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectorData {
    pub outline: Vec<(usize, String, Object)>,
    pub variables: Vec<(String, String)>,
    pub functions: Vec<(String, String, String)>,
}

/// What the person did in the Inspector.
#[derive(Clone, Debug, PartialEq)]
pub enum InspectorEvent {
    None,
    /// An outline row was selected: its variables are wanted.
    Select(Object),
    /// Reveal in an outline row's menu.
    Reveal(Object),
    /// Copy ID, Copy Value or Copy Path: this text to the clipboard.
    Copy(String),
    /// Ctrl+W or Escape (`closeCurrentSession:`).
    Close,
}

pub struct InspectorWindow {
    host: EguiHost,
    pub data: InspectorData,
    /// The selected outline row's object.
    pub selected: Option<Object>,
}

impl InspectorWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<InspectorWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, TITLE, 760.0, 600.0)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(InspectorWindow { host, data: InspectorData::default(), selected: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    /// New data (a reload keeps the selected object when it is still there,
    /// `selectObjectEquivalentTo:`).
    pub fn set_data(&mut self, data: InspectorData) {
        if self.selected.as_ref().is_some_and(|s| !data.outline.iter().any(|(_, _, o)| o == s)) {
            self.selected = None;
        }
        self.data = data;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> InspectorEvent {
        let mut out = InspectorEvent::None;
        let (d, selected) = (&self.data, &mut self.selected);
        self.host.paint(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            let family = if icons { egui::FontFamily::Name("icons".into()) } else { egui::FontFamily::Proportional };
            egui::CentralPanel::default().show(ui, |ui| {
                if ui.input(|i| i.key_pressed(egui::Key::Escape) || (i.key_pressed(egui::Key::W) && i.modifiers.command)) {
                    out = InspectorEvent::Close;
                }
                let top = (ui.available_height() - 200.0).max(160.0);
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.strong(OUTLINE);
                    egui::ScrollArea::both().id_salt("outline").max_height(top).auto_shrink([false, false]).show(ui, |ui| {
                        for (depth, label, object) in &d.outline {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0 * *depth as f32);
                                let text = row_text(label, family.clone());
                                let r = ui.selectable_label(selected.as_ref() == Some(object), text);
                                if r.clicked() {
                                    *selected = Some(object.clone());
                                    out = InspectorEvent::Select(object.clone());
                                }
                                r.context_menu(|ui| {
                                    if ui.button(REVEAL).clicked() {
                                        out = InspectorEvent::Reveal(object.clone());
                                        ui.close();
                                    }
                                    if ui.add_enabled(object.identifier().is_some(), egui::Button::new(COPY_ID)).clicked() {
                                        out = InspectorEvent::Copy(object.identifier().unwrap_or_default());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                    let ui = &mut cols[1];
                    ui.strong(VARIABLES);
                    egui::ScrollArea::both().id_salt("variables").max_height(top).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Grid::new("vars").num_columns(2).striped(true).spacing([12.0, 2.0]).show(ui, |ui| {
                            ui.strong(PATH);
                            ui.strong(VALUE);
                            ui.end_row();
                            for (path, value) in &d.variables {
                                let r = ui.label(path).union(ui.label(value));
                                r.context_menu(|ui| {
                                    if ui.button(COPY_VALUE).clicked() {
                                        out = InspectorEvent::Copy(value.clone());
                                        ui.close();
                                    }
                                    if ui.button(COPY_PATH).clicked() {
                                        out = InspectorEvent::Copy(path.clone());
                                        ui.close();
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.separator();
                ui.strong(FUNCTIONS);
                egui::ScrollArea::both().id_salt("functions").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("fns").num_columns(3).striped(true).spacing([16.0, 2.0]).show(ui, |ui| {
                        ui.strong(SIGNATURE);
                        ui.strong(ROLE);
                        ui.strong(SCRIPT);
                        ui.end_row();
                        for (sig, role, script) in &d.functions {
                            ui.label(sig);
                            ui.label(role);
                            ui.label(script);
                            ui.end_row();
                        }
                    });
                });
            });
        });
        out
    }
}

/// A row's text: its leading glyph in the icon font (when there is one), the rest as text.
fn row_text(label: &str, icons: egui::FontFamily) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let (glyph, rest) = match label.chars().next() {
        Some(c) if [WINDOW_GLYPH, TAB_GLYPH, SESSION_GLYPH].contains(&c) => (Some(c), &label[c.len_utf8()..]),
        _ => (None, label),
    };
    let body = egui::TextFormat { font_id: egui::FontId::proportional(14.0), ..Default::default() };
    if let Some(c) = glyph {
        if icons != egui::FontFamily::Proportional {
            job.append(&c.to_string(), 0.0, egui::TextFormat { font_id: egui::FontId::new(14.0, icons), ..Default::default() });
        }
    }
    job.append(rest, 0.0, body);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspector_rows_follow_iterm2() {
        let input = OutlineInput { windows: vec![("pty-A".into(), "bash".into(), vec![(5, "Tab 1".into(), vec![(1, "bash".into()), (2, "vim".into())])])], buried: vec![(8, "top".into())] };
        let row = |d: usize, l: &str, o: Object| (d, l.to_string(), o);
        assert_eq!(
            outline(&input),
            vec![
                row(0, "Termurion", Object::App),
                row(1, "\u{E737} bash (pty-A)", Object::Window("pty-A".into())),
                row(2, "\u{E8A5} Tab 1 (5)", Object::Tab(5)),
                row(3, "\u{E756} bash (1)", Object::Session(1)),
                row(3, "\u{E756} vim (2)", Object::Session(2)),
                row(1, "Buried Sessions", Object::Buried),
                row(2, "\u{E756} top (8)", Object::Session(8)),
            ]
        );
        // No buried row without buried sessions.
        let bare = OutlineInput { buried: Vec::new(), ..input };
        assert!(!outline(&bare).iter().any(|(_, _, o)| *o == Object::Buried));
        assert_eq!(outline(&bare).len(), 5);

        assert_eq!(role_name(Role::Generic as i32), "RPC");
        assert_eq!(role_name(Role::SessionTitle as i32), "Title Provider");
        assert_eq!(role_name(Role::StatusBarComponent as i32), "Status Bar Component");
        assert_eq!(role_name(Role::ContextMenu as i32), "Context Menu Provider");
        assert_eq!(role_name(99), "RPC");

        let regs = vec![("z(a)".to_string(), 1, Role::Generic as i32), ("a()".to_string(), 9, Role::SessionTitle as i32)];
        let name = |c: u64| if c == 1 { "tool".to_string() } else { String::new() };
        assert_eq!(
            function_rows(&regs, &name),
            vec![("a()".to_string(), "Title Provider".to_string(), "Unknown".to_string()), ("z(a)".to_string(), "RPC".to_string(), "tool".to_string())]
        );

        // Copy ID: the object's id; none for the root and the buried row.
        assert_eq!(Object::Window("pty-A".into()).identifier().as_deref(), Some("pty-A"));
        assert_eq!(Object::Session(3).identifier().as_deref(), Some("3"));
        assert_eq!((Object::App.identifier(), Object::Buried.identifier()), (None, None));
        let mut v = crate::api_vars::Values::new();
        v.insert("columns".into(), serde_json::json!(80));
        v.insert("name".into(), serde_json::json!("bash"));
        assert_eq!(variable_rows(&v), vec![("columns".to_string(), "80".to_string()), ("name".to_string(), "bash".to_string())]);
        assert_eq!((TITLE, OUTLINE, VARIABLES, FUNCTIONS), ("Scripting Inspector", "Sessions, Tabs, and Windows", "Variables", "Registered Functions"));
        assert_eq!((PATH, VALUE, SIGNATURE, ROLE, SCRIPT), ("Path", "Value", "Signature", "Role", "Script"));
        assert_eq!((REVEAL, COPY_ID, COPY_VALUE, COPY_PATH), ("Reveal", "Copy ID", "Copy Value", "Copy Path"));
    }
}
