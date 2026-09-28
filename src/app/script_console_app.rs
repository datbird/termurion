//! The Script Console and the Scripting Inspector in the app: their data from the script
//! history, the windows and the function registry, and what their buttons do. The console
//! opens from Scripts > Manage > Console (Win+Alt+J) and at each reveal point: a failed
//! script's notification, the function error alert's "Reveal in Script Console".

use super::*;
use crate::api_vars::{Objects, Scope};
use crate::script_console_ui::{self as sc, ConsoleEvent, ConsoleWindow, Tab};
use crate::script_history::{filter_lines, stamp_started};
use crate::script_inspector_ui::{self as si, InspectorData, InspectorEvent, InspectorWindow, Object, OutlineInput};

impl App {
    /// Scripts > Manage > Console: opens the console, or brings it forward.
    pub(super) fn open_script_console(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.script_console {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match ConsoleWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
            Ok(w) => self.script_console = Some(w),
            Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
        }
        self.console_refresh();
    }

    /// `revealTailOfHistoryEntry:` (`SC:264-280`): the console comes forward; the entry's row
    /// is selected (the filter switched to All when Running hides it), Logs shows and scrolls
    /// to its end. A gone entry only brings the console forward.
    pub(super) fn reveal_in_console(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        self.open_script_console(event_loop);
        let h = &self.script_history;
        let Some(w) = self.script_console.as_mut() else { return };
        let all: Vec<String> = h.entries().iter().map(|e| e.id.clone()).collect();
        let filtered: Vec<String> = h.filtered(w.running_only).iter().map(|e| e.id.clone()).collect();
        let Some((running_only, selected, tab)) = sc::reveal_choices(&filtered, &all, id, w.running_only) else { return };
        w.running_only = running_only;
        w.selected = Some(selected);
        w.tab = tab;
        self.console_refresh();
        if let Some(w) = self.script_console.as_mut() {
            w.scroll_to_end();
        }
    }

    /// The console's data again: the rows and buttons, and the selected entry's text for the
    /// tab that shows, through the Filter (`SC:414-445`), built only when that entry, the tab
    /// or the Filter changed (`sc::text_key`).
    fn console_refresh(&mut self) {
        let h = &self.script_history;
        let Some(w) = self.script_console.as_mut() else { return };
        let rows = sc::rows(h, w.running_only, &|s| stamp_started(&crate::stamps::local(s)));
        let listed = w.selected.as_deref().filter(|id| rows.iter().any(|r| r.id == *id));
        let entry = listed.and_then(|id| h.find(id));
        let key = sc::text_key(h, listed, w.tab, &w.filter);
        let text = (w.text_for != key).then(|| match entry {
            None => String::new(),
            Some(e) => match (w.tab, w.filter.is_empty()) {
                (Tab::Logs, true) => e.logs_text(),
                (Tab::Calls, true) => e.calls_text(),
                (Tab::Logs, false) => filter_lines(e.log_lines().iter(), &w.filter).join("\n"),
                (Tab::Calls, false) => filter_lines(e.calls(), &w.filter).join("\n"),
            },
        });
        let any_stopped = h.entries().iter().any(|e| !e.running);
        let buttons = sc::buttons(entry, any_stopped);
        w.seen = Some(h.generation());
        w.refreshed = Some(Instant::now());
        w.built_for = Some(w.choices());
        w.text_for = key;
        w.set_data(rows, buttons, text);
    }

    /// Once per loop pass: a reveal asked for where no event loop was at hand (a notification
    /// click, the error alert's button) opens now, and the console follows the history, at
    /// most every `sc::REFRESH_EVERY` (the time it is due next is returned for the loop's
    /// wake).
    pub(super) fn script_console_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        if let Some(id) = self.script_console_reveal.take() {
            self.reveal_in_console(&id, event_loop);
        }
        let w = self.script_console.as_ref()?;
        if w.seen == Some(self.script_history.generation()) {
            return None;
        }
        let due = sc::refresh_due(w.refreshed, Instant::now());
        if due.is_none() {
            self.console_refresh();
        }
        due
    }

    /// Events for the console. False for other windows. Closing it forgets its choices, as
    /// iTerm2 clears the text and the selection (`SC:172-177`).
    pub(super) fn script_console_event(&mut self, window: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(w) = self.script_console.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConsoleEvent::Close,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConsoleEvent::None
            }
        };
        match answer {
            ConsoleEvent::None | ConsoleEvent::Select(_) => {}
            // `terminateScriptOnRow:` (`SC:216-222`): only a running entry with one process.
            ConsoleEvent::Terminate(id) => {
                if self.script_history.find(&id).is_some_and(|e| e.running && e.only_pid().is_some()) {
                    self.terminate_script(&id);
                }
            }
            ConsoleEvent::Restart(id) => self.relaunch_script(&id),
            ConsoleEvent::ClearTerminated => self.script_history.clear_terminated(),
            ConsoleEvent::Inspector => self.open_script_inspector(event_loop),
            ConsoleEvent::Close => {
                self.script_console = None;
                return true;
            }
        }
        // The person's choices (a row, the Filter, All or Running) show at once.
        if self.script_console.as_ref().is_some_and(|w| w.built_for != Some(w.choices()) || w.seen != Some(self.script_history.generation())) {
            self.console_refresh();
        }
        true
    }

    /// The Inspector button (`SC:246-253`): the Inspector opens, or reloads and comes forward.
    fn open_script_inspector(&mut self, event_loop: &ActiveEventLoop) {
        if self.script_inspector.is_none() {
            let Some(g) = self.gpu.as_ref() else { return };
            match InspectorWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
                Ok(w) => self.script_inspector = Some(w),
                Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
            }
        }
        self.inspector_reload();
        if let Some(w) = &self.script_inspector {
            w.focus();
        }
    }

    /// The Inspector's data now (`reload:`): the outline, the selected object's variables
    /// (kept when it is still there) and the registered functions.
    fn inspector_reload(&mut self) {
        let outline = si::outline(&self.inspector_outline());
        let selected = self.script_inspector.as_ref().and_then(|w| w.selected.clone()).filter(|s| outline.iter().any(|(_, _, o)| o == s));
        let variables = selected.as_ref().map(|o| self.inspector_variables(o)).unwrap_or_default();
        let regs: Vec<(String, u64, i32)> = self.api_fns.registrations().into_iter().map(|(sig, conn, r)| (sig, conn, r.role.unwrap_or(0))).collect();
        let name = |conn: u64| self.api_keys.get(&conn).and_then(|(key, _)| self.script_history.find(key)).map(|e| e.name.clone()).unwrap_or_default();
        let functions = si::function_rows(&regs, &name);
        if let Some(w) = self.script_inspector.as_mut() {
            w.set_data(InspectorData { outline, variables, functions });
        }
    }

    /// The windows by their number order, each with its title, tabs and sessions, and the
    /// buried sessions (`iTermOutlineRoot`). Each window is entered to read it.
    fn inspector_outline(&mut self) -> OutlineInput {
        let back = self.windows.current();
        let mut windows = Vec::new();
        for k in self.registry.order() {
            let Some(id) = self.window_id_of_key(k) else { continue };
            if !self.enter(id) {
                continue;
            }
            let guid = self.registry.guid(k).unwrap_or_default().to_string();
            let title = self.gpu.as_ref().map(|g| g.window.title()).unwrap_or_default();
            let tabs = self.tabs.iter().map(|t| (t.id, self.tab_look(t).title, t.layout.panes().into_iter().map(|p| (p, self.session_title(p))).collect())).collect();
            windows.push((guid, title, tabs));
        }
        // The window current before, or none again when none was.
        match back.and_then(|k| self.window_id_of_key(k)) {
            Some(b) => {
                self.enter(b);
            }
            None => self.leave_windows(),
        }
        OutlineInput { windows, buried: self.buried.items() }
    }

    /// An object's variables as the API reads them (11b's `scope_values` through
    /// `api_objects`); the buried row has none.
    fn inspector_variables(&self, o: &Object) -> Vec<(String, String)> {
        let objects = self.api_objects();
        let values = match o {
            Object::App => objects.values(Scope::App, ""),
            Object::Window(guid) => objects.values(Scope::Window, guid),
            Object::Tab(t) => objects.values(Scope::Tab, &t.to_string()),
            Object::Session(p) => objects.values(Scope::Session, &p.to_string()),
            Object::Buried => return Vec::new(),
        };
        si::variable_rows(&values)
    }

    /// Events for the Inspector. False for other windows.
    pub(super) fn script_inspector_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.script_inspector.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => InspectorEvent::Close,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                InspectorEvent::None
            }
        };
        match answer {
            InspectorEvent::None => {}
            // The buried row has no scope: the variables stay (`showVariablesForProxy:`).
            InspectorEvent::Select(Object::Buried) => {}
            InspectorEvent::Select(o) => {
                let variables = self.inspector_variables(&o);
                if let Some(w) = self.script_inspector.as_mut() {
                    let data = InspectorData { variables, ..w.data.clone() };
                    w.set_data(data);
                }
            }
            InspectorEvent::Reveal(o) => self.inspector_reveal(&o),
            InspectorEvent::Copy(text) => {
                if let Some(Err(e)) = self.clipboard.as_mut().map(|cb| cb.set_text(text)) {
                    self.banners.set("clipboard", format!("Could not copy: {e}"));
                }
            }
            InspectorEvent::Close => self.script_inspector = None,
        }
        true
    }

    /// Reveal (`reveal:`): a session comes forward in its tab and window (a buried one is
    /// restored first, as `PTYSession reveal` disinters it); a tab or a window brings forward
    /// its active session. The root and the buried row reveal nothing.
    fn inspector_reveal(&mut self, o: &Object) {
        let ids = self.api_ids();
        let pane = match o {
            Object::Session(p) => {
                if self.buried.contains(*p) {
                    self.disinter(*p);
                }
                Some(*p)
            }
            Object::Tab(t) => ids.tab_window(*t).and_then(|k| self.window_id_of_key(k)).filter(|id| self.enter(*id)).and_then(|_| self.tabs.iter().find(|x| x.id == *t).map(|x| x.focused)),
            Object::Window(guid) => ids.window_key(guid).and_then(|k| self.window_id_of_key(k)).filter(|id| self.enter(*id)).and_then(|_| self.tabs.get(self.active).map(|x| x.focused)),
            Object::App | Object::Buried => None,
        };
        if let Some(p) = pane.filter(|p| self.panes.contains_key(p)) {
            self.reveal_pane(p);
        }
    }
}
