//! Coprocesses in the window: Session > Run Coprocess and Stop Coprocess, the trigger,
//! key, Smart Selection, Semantic History, and file-drop starts, the indicator, and the
//! banners.

use super::*;
use crate::coprocess::{self, Entry, Refusal};
use crate::prompt_ui::{PromptResult, PromptWindow};

const STDERR_BAR: &str = "coprocess-stderr";
const TRIGGER_BAR: &str = "coprocess-trigger";

fn string_list(v: Option<toml::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
}

impl App {
    /// Starts a coprocess in a pane, or says why not.
    pub(super) fn start_coprocess(&mut self, pane: PaneId, command: &str, mute: bool, entry: Entry) {
        self.start_coprocess_titled(pane, command, mute, entry, None);
    }

    /// `start_coprocess`, with the refusal's title for a start that is not a Run Coprocess
    /// trigger (Captured Output's "Captured Output trigger", `PS:25097`).
    pub(super) fn start_coprocess_titled(&mut self, pane: PaneId, command: &str, mute: bool, entry: Entry, title: Option<&str>) {
        let running = self.panes.get(&pane).is_some_and(|p| p.coprocess_command().is_some());
        match coprocess::start_policy(running, entry) {
            Ok(()) => {}
            Err(Refusal::Quiet) => return,
            Err(Refusal::TriggerWarning) => {
                if !self.globals.bool(coprocess::SUPPRESS_TRIGGER_WARNING_KEY) && self.trigger_told.insert((pane, TRIGGER_BAR.to_string())) {
                    let text = match title {
                        Some(t) => format!("{t}: Can't run two coprocesses at once."),
                        None => coprocess::trigger_warning(if mute { "Run Silent Coprocess\u{2026}" } else { "Run Coprocess\u{2026}" }),
                    };
                    self.ask(pane, TRIGGER_BAR, &text, &["Silence Warning"], false);
                }
                return;
            }
            Err(Refusal::Banner) => {
                self.banners.set("coprocess", "A coprocess is already running in this session. Stop it first (Session > Stop Coprocess).");
                return self.redraw();
            }
        }
        let cwd = self.pane_dir(pane).filter(|d| crate::localfs::is_dir(d)).or_else(|| std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)).unwrap_or_else(std::env::temp_dir);
        let notify = self.notify.clone();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        match p.run_coprocess(command, mute, &cwd, notify) {
            Ok(()) => {
                self.banners.clear("coprocess");
                let mut mru = string_list(self.globals.get(coprocess::MRU_KEY));
                coprocess::mru_add(&mut mru, command);
                self.globals.set(coprocess::MRU_KEY, toml::Value::Array(mru.into_iter().map(toml::Value::String).collect()));
                self.save_globals();
            }
            Err(e) => self.banners.set("coprocess", format!("Failed to launch coprocess. ({e})")),
        }
        self.redraw();
    }

    pub(super) fn stop_coprocess(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.stop_coprocess();
        }
        self.redraw();
    }

    /// Session > Run Coprocess...: the command window, for the focused pane.
    pub(super) fn open_coprocess_prompt(&mut self, event_loop: &ActiveEventLoop) {
        if let Some((w, _)) = &self.co_prompt {
            return w.focus();
        }
        let Some(id) = self.focused_id() else { return };
        // The menu item is off while one runs.
        if self.panes.get(&id).is_none_or(|p| p.coprocess_command().is_some()) {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let mru = string_list(self.globals.get(coprocess::MRU_KEY));
        let ignored = string_list(self.globals.get(coprocess::IGNORE_ERRORS_KEY));
        let first = mru.first().cloned().unwrap_or_default();
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Run Coprocess", "Coprocess command to run:", &first) {
            Ok(w) => self.co_prompt = Some((w.with_choices(mru).with_check("Silently ignore errors from this command", ignored).refuse_blank(), id)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the Run Coprocess window. False when `window` is not it.
    pub(super) fn coprocess_prompt_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane)) = self.co_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let pane = *pane;
        match event {
            WindowEvent::CloseRequested => self.co_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.co_prompt = None,
                PromptResult::Ok(command) => {
                    let ignore = w.checked();
                    self.co_prompt = None;
                    let command = command.trim().to_string();
                    let mut list = string_list(self.globals.get(coprocess::IGNORE_ERRORS_KEY));
                    list.retain(|c| *c != command);
                    if ignore {
                        list.push(command.clone());
                    }
                    self.globals.set(coprocess::IGNORE_ERRORS_KEY, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
                    self.save_globals();
                    self.start_coprocess(pane, &command, false, Entry::Menu);
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Coprocesses that ended: stderr output shows a banner unless the command's errors
    /// are ignored; lost input is reported once.
    pub(super) fn check_coprocesses(&mut self) {
        let ignored = string_list(self.globals.get(coprocess::IGNORE_ERRORS_KEY));
        let mut ended = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            if p.coprocess_dropped() > 0 {
                self.banners.set("coprocess-input", "A coprocess is not reading its input: terminal output it missed was dropped.");
            }
            if let Some(end) = p.take_coprocess_end() {
                ended.push((*id, end));
            }
        }
        for (id, (command, stderr)) in ended {
            if !stderr.trim().is_empty() && !ignored.contains(&command) {
                self.coprocess_errors.insert(id, (command.clone(), stderr));
                self.ask(id, STDERR_BAR, &coprocess::stderr_banner(&command), &["View Errors", "Ignore Errors from This Command"], false);
            }
            self.redraw();
        }
    }

    /// The coprocess bars' answers. False for other bars.
    pub(super) fn coprocess_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        match id {
            TRIGGER_BAR => {
                if button == Some("Silence Warning") {
                    self.globals.set(coprocess::SUPPRESS_TRIGGER_WARNING_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                }
                true
            }
            STDERR_BAR => {
                let Some((command, stderr)) = self.coprocess_errors.remove(&pane) else { return true };
                match button {
                    Some("View Errors") => {
                        let path = std::env::temp_dir().join(format!("coprocess-stderr.{}.txt", std::process::id()));
                        match std::fs::write(&path, stderr) {
                            Ok(()) => {
                                if let Err(e) = std::process::Command::new("notepad.exe").arg(&path).spawn() {
                                    self.banners.set("coprocess", format!("Could not open {}: {e}", path.display()));
                                }
                            }
                            Err(e) => self.banners.set("coprocess", format!("Could not write {}: {e}", path.display())),
                        }
                    }
                    Some("Ignore Errors from This Command") => {
                        let mut list = string_list(self.globals.get(coprocess::IGNORE_ERRORS_KEY));
                        if !list.contains(&command) {
                            list.push(command);
                        }
                        self.globals.set(coprocess::IGNORE_ERRORS_KEY, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
                        self.save_globals();
                    }
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }

    /// The pane's top-right indicators: broadcast (sends, or receive only), the coprocess, and
    /// the pin of a pinned hotkey window, and the AI chat's linked and streaming glyphs. Answers each AI glyph's rectangle, which the
    /// frame keeps for a press (the draw loop holds a borrow of the tab).
    pub(super) fn draw_coprocess_indicator(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32), broadcast: (bool, bool)) -> Vec<(PaneId, Rect, scene::IndicatorKind)> {
        if self.globals.bool("disableTopRightIndicators") {
            return Vec::new();
        }
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let (linked, streaming) = self.chat_pane_state(id);
        let shown = scene::indicators(broadcast.0, broadcast.1, p.coprocess_command().is_some(), linked, streaming, self.in_pinned_hotkey_window());
        if shown.is_empty() {
            return Vec::new();
        }
        let glyphs: Vec<char> = shown.iter().map(|(g, _)| *g).collect();
        let rects = scene::top_right_indicators(frame, rect, &glyphs, p.own().palette.fg, cell);
        shown.iter().zip(rects).filter(|((_, k), _)| *k != scene::IndicatorKind::Other).map(|((_, k), r)| (id, r, *k)).collect()
    }

    /// Copy Interpolated String (key action 72): the text with the session's variables.
    pub(super) fn copy_interpolated(&mut self, template: &str) {
        let Some(id) = self.focused_id() else { return };
        let vars = self.variables(id);
        let text = match crate::interp::evaluate(template, &|n| crate::prompted::variable(n.strip_prefix("session.").unwrap_or(n), &vars), &[]) {
            Ok(t) => t,
            Err(e) => return self.banners.set("action", format!("Copy Interpolated String: {e}")),
        };
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    /// Files dropped on the window: a silent coprocess when `fileDropCoprocess` is set,
    /// else the paths are pasted (quoted when they hold a space).
    pub(super) fn flush_dropped(&mut self) {
        self.dnd_flush();
        if self.dropped.is_empty() {
            return;
        }
        let dropped = std::mem::take(&mut self.dropped);
        let pane = dropped[0].0;
        // The drop's own window decides where a broadcast paste goes.
        self.enter_pane_window(pane);
        let files: Vec<String> = dropped.into_iter().map(|(_, f)| f).collect();
        // Alt held over a session that can transfer: the files are offered to the remote
        // working directory instead (before the coprocess and the paste).
        if self.ssh_drop(pane, files.clone()) {
            return;
        }
        // Dropped names are typed text or a coprocess's input: never at a password prompt or into
        // an ended session (13q/13r gates).
        if let Some(why) = self.panes.get(&pane).and_then(|p| drop_refusal(p.exited.is_some(), p.ended, p.at_password_prompt())) {
            return self.banners.set("action", why);
        }
        match coprocess::drop_command(&self.globals.string("fileDropCoprocess"), &files) {
            Some(Ok(cmd)) => self.start_coprocess(pane, &cmd, true, Entry::FileDrop),
            // A dropped name the one cmd.exe builder refuses: nothing runs, and the banner says why.
            Some(Err(why)) => self.banners.set("action", why),
            None => {
                // The names pass 13r's one guard for terminal writes (B5): a refused name types
                // nothing and the banner says why.
                let shell = self.composer_shell(pane);
                match coprocess::drop_text(&files, self.globals.bool("WrapDroppedFilenamesInQuotesWhenPasting"), shell.as_deref()) {
                    Ok(text) => self.user_paste(pane, &text, crate::broadcast::Write::Drop),
                    Err(why) => self.banners.set("action", why),
                }
            }
        }
    }
}

/// Why dropped names are not written to a pane: its session has ended, or it waits at a
/// password prompt.
pub(super) fn drop_refusal(exited: bool, ended: bool, password: bool) -> Option<&'static str> {
    if exited || ended {
        Some(super::cmdinfo_app::ENDED)
    } else if password {
        Some(super::cmdinfo_app::PASSWORD_PROMPT)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 8 fix round 1, item 2: a dropped name is never typed at a password prompt or into an
    /// ended session (13q/13r gates), whichever route carries it.
    #[test]
    fn dropped_text_refuses_a_password_prompt_and_an_ended_pane() {
        assert_eq!(drop_refusal(false, false, false), None);
        assert_eq!(drop_refusal(false, true, false), Some(super::super::cmdinfo_app::ENDED));
        assert_eq!(drop_refusal(true, false, false), Some(super::super::cmdinfo_app::ENDED));
        assert_eq!(drop_refusal(false, false, true), Some(super::super::cmdinfo_app::PASSWORD_PROMPT));
        let src = include_str!("coprocess_ui.rs");
        let body = &src[src.find("fn flush_dropped").unwrap()..];
        let (gate, coproc, paste) = (body.find("drop_refusal(").expect("asks the gate"), body.find("start_coprocess(").unwrap(), body.find("user_paste(").unwrap());
        assert!(gate < coproc && gate < paste, "the gate comes before either route");
    }
}
