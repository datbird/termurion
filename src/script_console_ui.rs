//! The Script Console window (iTerm2's `iTermScriptConsole` and its xib): the scripts, the
//! connections and the app-wide log as rows under an All/Running control; the selected row's
//! Logs or Calls, narrowed by the Filter; and the Inspector, Terminate, Restart and Clear
//! Terminated buttons.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::script_history::{Entry, History};

/// The window's texts (`API/Base.lproj/iTermScriptConsole.xib:29-189`) and the buttons'
/// tooltips (`SC:151-154`).
pub const TITLE: &str = "Script Console";
pub const NAME: &str = "Name";
pub const STARTED: &str = "Started";
pub const ALL: &str = "All";
pub const RUNNING: &str = "Running";
pub const LOGS: &str = "Logs";
pub const CALLS: &str = "Calls";
pub const FILTER: &str = "Filter";
pub const SCROLL: &str = "Scroll to bottom on update";
pub const INSPECTOR: &str = "Inspector";
pub const TERMINATE: &str = "Terminate";
pub const RESTART: &str = "Restart";
pub const CLEAR_TERMINATED: &str = "Clear Terminated";

/// Segoe Fluent Icons glyphs for iTerm2's SF Symbols (scope, stop circle, arrow clockwise,
/// trash): Zoom, Stop, Refresh and Delete.
const INSPECTOR_GLYPH: char = '\u{E71E}';
const TERMINATE_GLYPH: char = '\u{E71A}';
const RESTART_GLYPH: char = '\u{E72C}';
const CLEAR_GLYPH: char = '\u{E74D}';

/// One row of the table: the entry's id, its Name column and its Started column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub started: String,
}

/// The table's rows (`SC:185-206`): every entry, or the running ones; a stopped entry's name
/// in parentheses; the start time as `started` writes it.
pub fn rows(h: &History, running_only: bool, started: &dyn Fn(u64) -> String) -> Vec<Row> {
    h.filtered(running_only).into_iter().map(|e| Row { id: e.id.clone(), name: e.shown_name(), started: started(e.started) }).collect()
}

/// Which buttons are enabled (`SC:526-543`): Terminate for a running entry with exactly one
/// process, Restart for an entry that can be relaunched, Clear Terminated whenever any entry
/// has stopped (whatever is selected).
pub fn buttons(entry: Option<&Entry>, any_stopped: bool) -> (bool, bool, bool) {
    match entry {
        Some(e) => (e.running && e.only_pid().is_some(), e.relaunch.is_some(), any_stopped),
        None => (false, false, any_stopped),
    }
}

/// `revealTailOfHistoryEntry:` (`SC:264-280`): the entry's row among the shown ids; when the
/// Running filter hides it, its row among all ids and true (the filter switches to All);
/// None when it is gone.
pub fn reveal_target(filtered: &[String], all: &[String], id: &str) -> (Option<usize>, bool) {
    if let Some(i) = filtered.iter().position(|f| f == id) {
        return (Some(i), false);
    }
    match all.iter().position(|a| a == id) {
        Some(i) => (Some(i), true),
        None => (None, false),
    }
}

/// The tab over the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Logs,
    Calls,
}

/// What the person did in the console.
#[derive(Clone, Debug, PartialEq)]
pub enum ConsoleEvent {
    None,
    /// A row was selected (or the selection went).
    Select(Option<String>),
    Terminate(String),
    Restart(String),
    ClearTerminated,
    Inspector,
    /// Ctrl+W or Escape (`closeCurrentSession:`, `cancel:`).
    Close,
}

/// What the console shows (the app builds it from the history and the window's choices).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConsoleData {
    pub rows: Vec<Row>,
    /// The selected entry's log lines through the Filter.
    pub logs: String,
    /// The selected entry's calls through the Filter.
    pub calls: String,
    /// Terminate, Restart and Clear Terminated enabled.
    pub buttons: (bool, bool, bool),
}

pub struct ConsoleWindow {
    host: EguiHost,
    pub data: ConsoleData,
    /// The All/Running control on Running.
    pub running_only: bool,
    /// The selected row's entry id.
    pub selected: Option<String>,
    pub tab: Tab,
    pub filter: String,
    /// "Scroll to bottom on update" (on at first, as the xib has it).
    pub scroll_to_bottom: bool,
    /// The history generation the data was built from.
    pub seen: Option<u64>,
    /// The choices (`choices`) the data was built for.
    pub built_for: Option<(bool, Option<String>, String)>,
    /// The selection and filter the data was built for: new text with the same ones is new
    /// lines, which scroll to the end.
    shown_for: (Option<String>, String),
    /// Scroll the text to its end on the next paint.
    scroll_pending: bool,
}

impl ConsoleWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<ConsoleWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, TITLE, 900.0, 560.0)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(ConsoleWindow {
            host,
            data: ConsoleData::default(),
            running_only: false,
            selected: None,
            tab: Tab::Logs,
            filter: String::new(),
            scroll_to_bottom: true,
            seen: None,
            built_for: None,
            shown_for: (None, String::new()),
            scroll_pending: false,
        })
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

    /// New data. A selected entry no longer listed is unselected. The text scrolls to its end
    /// when "Scroll to bottom on update" is on and the tab that shows got new lines, or
    /// another row was selected (`SC:315-399`); a changed Filter keeps the place.
    pub fn set_data(&mut self, d: ConsoleData) {
        if self.selected.as_ref().is_some_and(|s| !d.rows.iter().any(|r| r.id == *s)) {
            self.selected = None;
        }
        let key = (self.selected.clone(), self.filter.clone());
        let changed = match self.tab {
            Tab::Logs => d.logs != self.data.logs,
            Tab::Calls => d.calls != self.data.calls,
        };
        if self.scroll_to_bottom && changed && (self.shown_for.0 != key.0 || self.shown_for.1 == key.1) {
            self.scroll_pending = true;
        }
        self.shown_for = key;
        self.data = d;
        self.host.request_redraw();
    }

    /// What the data depends on: Running, the selected row and the Filter.
    pub fn choices(&self) -> (bool, Option<String>, String) {
        (self.running_only, self.selected.clone(), self.filter.clone())
    }

    /// The text goes to its end on the next paint (a reveal, `scrollToEndOfDocument:`).
    pub fn scroll_to_end(&mut self) {
        self.scroll_pending = true;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> ConsoleEvent {
        let mut out = ConsoleEvent::None;
        // A jump to the end is clamped as it paints: the next paint shows it.
        let jumped = self.scroll_pending;
        let d = &self.data;
        let (running_only, selected, tab, filter, scroll_to_bottom, scroll_pending) = (&mut self.running_only, &mut self.selected, &mut self.tab, &mut self.filter, &mut self.scroll_to_bottom, &mut self.scroll_pending);
        self.host.paint(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || (i.key_pressed(egui::Key::W) && i.modifiers.command)) {
                out = ConsoleEvent::Close;
            }
            egui::Panel::bottom("buttons").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (terminate, restart, clear) = d.buttons;
                    if button(ui, icons, INSPECTOR_GLYPH, INSPECTOR, true) {
                        out = ConsoleEvent::Inspector;
                    }
                    let id = selected.clone().unwrap_or_default();
                    if button(ui, icons, TERMINATE_GLYPH, TERMINATE, terminate) {
                        out = ConsoleEvent::Terminate(id.clone());
                    }
                    if button(ui, icons, RESTART_GLYPH, RESTART, restart) {
                        out = ConsoleEvent::Restart(id);
                    }
                    if button(ui, icons, CLEAR_GLYPH, CLEAR_TERMINATED, clear) {
                        out = ConsoleEvent::ClearTerminated;
                    }
                });
                ui.add_space(2.0);
            });
            egui::Panel::left("entries").default_size(320.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(running_only, false, ALL);
                    ui.selectable_value(running_only, true, RUNNING);
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("rows").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("table").num_columns(2).striped(true).spacing([12.0, 2.0]).show(ui, |ui| {
                        ui.strong(NAME);
                        ui.strong(STARTED);
                        ui.end_row();
                        for r in &d.rows {
                            let on = selected.as_deref() == Some(r.id.as_str());
                            let a = ui.selectable_label(on, &r.name).on_hover_text(&r.name);
                            let b = ui.selectable_label(on, &r.started);
                            if (a.clicked() || b.clicked()) && !on {
                                *selected = Some(r.id.clone());
                                out = ConsoleEvent::Select(Some(r.id.clone()));
                            }
                            ui.end_row();
                        }
                    });
                });
            });
            egui::CentralPanel::default().show(ui, |ui| {
                egui::Panel::top("tabs").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(tab, Tab::Logs, LOGS);
                        ui.selectable_value(tab, Tab::Calls, CALLS);
                    });
                });
                egui::Panel::bottom("filter").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(FILTER);
                        ui.add(egui::TextEdit::singleline(filter).desired_width(260.0));
                        ui.checkbox(scroll_to_bottom, SCROLL);
                    });
                });
                let text = match tab {
                    Tab::Logs => d.logs.as_str(),
                    Tab::Calls => d.calls.as_str(),
                };
                let mut area = egui::ScrollArea::both().id_salt(("text", *tab == Tab::Logs)).auto_shrink([false, false]);
                if std::mem::take(scroll_pending) {
                    // Clamped to the end by the scroll area.
                    area = area.vertical_scroll_offset(f32::MAX);
                }
                area.show(ui, |ui| {
                    // Lines are never wrapped: the text scrolls sideways (`SC:79-85`).
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                        let job = egui::text::LayoutJob::simple(buf.as_str().to_owned(), egui::FontId::monospace(12.0), ui.visuals().text_color(), f32::INFINITY);
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let mut shown = text;
                    ui.add(egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace).frame(egui::Frame::NONE).desired_width(f32::INFINITY).layouter(&mut layouter));
                });
            });
        });
        // A new selection or control shows its data at once (the app builds it next).
        if jumped || !matches!(out, ConsoleEvent::None) {
            self.host.request_redraw();
        }
        out
    }
}

/// A bottom button: its glyph when the icon font is there (else its name), with its tooltip.
fn button(ui: &mut egui::Ui, icons: bool, glyph: char, tip: &str, enabled: bool) -> bool {
    let text = if icons { egui::RichText::new(glyph.to_string()).family(egui::FontFamily::Name("icons".into())).size(16.0) } else { egui::RichText::new(tip) };
    ui.add_enabled(enabled, egui::Button::new(text)).on_hover_text(tip).on_disabled_hover_text(tip).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script_history::Relaunch;
    use std::path::PathBuf;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn console_rows_follow_iterm2() {
        let mut h = History::new(false, 10);
        h.add(Entry::new("k", "tool", 20));
        let mut done = Entry::new("d", "done", 30);
        done.running = false;
        h.add(done);
        let started = |s: u64| format!("t{s}");
        let row = |id: &str, name: &str, started: &str| Row { id: id.into(), name: name.into(), started: started.into() };
        assert_eq!(rows(&h, false, &started), vec![row("iTerm2", "Tandem App", "t10"), row("k", "tool", "t20"), row("d", "(done)", "t30")]);
        assert_eq!(rows(&h, true, &started), vec![row("iTerm2", "Tandem App", "t10"), row("k", "tool", "t20")]);
    }

    #[test]
    fn console_buttons_follow_iterm2() {
        let relaunch = Some(Relaunch { path: PathBuf::from(r"C:\S\tool.py"), args: Vec::new() });
        let mut running = Entry::new("k", "tool", 0);
        running.pids = vec![42];
        running.relaunch = relaunch.clone();
        assert_eq!(buttons(Some(&running), false), (true, true, false));
        assert_eq!(buttons(Some(&running), true), (true, true, true));
        // The REPL: running, no process of its own to end and no relaunch.
        let repl = Entry::new("r", "REPL", 0);
        assert_eq!(buttons(Some(&repl), false), (false, false, false));
        assert_eq!(buttons(Some(&repl), true), (false, false, true));
        // Two processes: Terminate cannot pick one (`SH:173-178`).
        let mut two = running.clone();
        two.pids = vec![1, 2];
        assert_eq!(buttons(Some(&two), false), (false, true, false));
        let mut stopped = running.clone();
        stopped.running = false;
        assert_eq!(buttons(Some(&stopped), true), (false, true, true));
        assert_eq!(buttons(None, true), (false, false, true));
        assert_eq!(buttons(None, false), (false, false, false));
        assert_eq!((TITLE, NAME, STARTED, ALL, RUNNING, LOGS, CALLS, FILTER), ("Script Console", "Name", "Started", "All", "Running", "Logs", "Calls", "Filter"));
        assert_eq!((SCROLL, INSPECTOR, TERMINATE, RESTART, CLEAR_TERMINATED), ("Scroll to bottom on update", "Inspector", "Terminate", "Restart", "Clear Terminated"));
    }

    #[test]
    fn reveal_follows_iterm2() {
        assert_eq!(reveal_target(&v(&["a", "b"]), &v(&["a", "b", "c"]), "b"), (Some(1), false));
        // Hidden by Running: the filter switches to All (`SC:264-280`).
        assert_eq!(reveal_target(&v(&["a"]), &v(&["a", "c"]), "c"), (Some(1), true));
        assert_eq!(reveal_target(&v(&["a"]), &v(&["a", "c"]), "x"), (None, false));
    }
}
