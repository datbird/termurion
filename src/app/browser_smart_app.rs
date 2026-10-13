//! Smart Selection actions from a page's menu (milestone 13x2, Task 4). Web content is untrusted:
//! the selection reaches here as the menu's own snapshot (WebView2's bounded selection), an action
//! runs only from the person's own pick on Termurion's menu, and every value a page's text puts into a
//! command or a path goes through the one builder (`smartsel::parameter`, whose refusals are the
//! 13p-r rules), then an ask that shows the whole text, then the terminal's own sinks. A URL opens
//! only through `browser_page_open`; a program is revealed, never run (`open_file_run`).

use super::*;
use crate::browser_ctx::{self, SmartPlan};
use crate::layout::PaneId;

/// What nothing runs for when the text cannot be shown whole.
pub(super) const TOO_LONG: &str = "Termurion did not run this: its text is too long or holds characters that cannot be shown, so it could not be shown in full first.";

impl App {
    /// The page's host (`BCM:217-224`): what `\h` means in a page's action; empty with none.
    fn browser_page_host(&self, pane: PaneId) -> String {
        self.panes.get(&pane).and_then(|p| p.browser_io()).and_then(|io| crate::toolbelt::url_host(&io.url).map(str::to_string)).unwrap_or_default()
    }

    /// Runs one Smart Selection action the person picked from the page menu (`BWV:554-602`).
    pub(super) fn browser_smart_action(&mut self, pane: PaneId, action: &crate::smartsel::RuleAction, captures: &[String], event_loop: &ActiveEventLoop) {
        let host = self.browser_page_host(pane);
        match browser_ctx::smart_plan(action, captures, &host) {
            SmartPlan::Nothing => {}
            SmartPlan::Refused(why) => {
                self.banners.set("action", if why.is_empty() { "The action's parameter could not be evaluated.".to_string() } else { why });
                self.redraw();
            }
            SmartPlan::Copy(text) => {
                self.set_clipboard_text(text.clone());
                self.remember_paste(&text);
            }
            SmartPlan::OpenUrl(url) => self.browser_page_open(pane, &url, super::browser_scripts_app::PageOpen::Tab, event_loop),
            SmartPlan::OpenFile(file) => {
                // The plan first: a network path, a link or a control character is a banner, not a question.
                if let crate::semantic::Run::Blocked(why) = crate::semantic::open_file_run(&file, None, false) {
                    self.banners.set("action", why);
                    return self.redraw();
                }
                if !browser_ctx::shown_whole(&file) {
                    self.banners.set("action", TOO_LONG);
                    return self.redraw();
                }
                let ask = super::scripts_alerts::ask(browser_ctx::OPEN_FILE_HEADING, &browser_ctx::open_file_text(&file), &["OK", crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::BrowserOpenFile { pane, file });
                self.ask_scripts(ask);
            }
            SmartPlan::Run(line) => {
                if !browser_ctx::shown_whole(&line) {
                    self.banners.set("action", TOO_LONG);
                    return self.redraw();
                }
                let ask = super::scripts_alerts::ask(browser_ctx::RUN_HEADING, &browser_ctx::run_text(&line), &["OK", crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::BrowserRun { pane, command: line });
                self.ask_scripts(ask);
            }
        }
    }

    /// OK on "Open file?": the file opens as `open_file_run` plans it (a program is revealed).
    pub(super) fn browser_open_file_confirmed(&mut self, pane: PaneId, file: &str) {
        let run = crate::semantic::open_file_run(file, None, false);
        self.execute_with(pane, run, None, super::semantic_ui::Exec { background: false, from_file: true, event_loop: None });
    }

    /// OK on "Run command?": queued for the next tick, which has the event loop a window needs.
    pub(super) fn browser_run_confirmed(&mut self, pane: PaneId, command: String) {
        self.browser_runs.push((pane, command));
        self.redraw();
    }

    /// Runs the confirmed commands in single-use windows, as the terminal's Run Command in Window.
    pub(super) fn browser_runs_tick(&mut self, event_loop: &ActiveEventLoop) {
        for (pane, command) in std::mem::take(&mut self.browser_runs) {
            let run = crate::semantic::run_in_window(&command);
            self.execute_with(pane, run, None, super::semantic_ui::Exec { background: false, from_file: false, event_loop: Some(event_loop) });
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_smart_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// B5 (a class): a page's command reaches `execute_with` only after the one parameter builder
    /// and an ask; nothing here builds a command line by itself.
    #[test]
    fn a_page_command_goes_through_the_one_path() {
        let p = prod();
        let act = &p[p.find("pub(super) fn browser_smart_action").unwrap()..p.find("pub(super) fn browser_open_file_confirmed").unwrap()];
        assert!(act.contains("smart_plan(") && !act.contains("execute"), "the action only plans and asks");
        assert!(act.find("shown_whole(&line)").unwrap() < act.find("Then::BrowserRun").unwrap(), "the whole line is shown first");
        assert!(act.contains("browser_page_open("), "a URL goes through the one funnel");
        for banned in ["Command::new(", "cmd_command_line(", "build_cmd_line(", "Escape::", "guard_open(", "shell_open(", "Run::Command"] {
            assert!(!p.contains(banned), "{banned}");
        }
        let tick = &p[p.find("pub(super) fn browser_runs_tick").unwrap()..];
        assert!(tick.contains("run_in_window(&command)") && tick.contains("Some(event_loop)"));
        // The answers are the person's: only OK queues or opens.
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::BrowserRun { .. } | Then::BrowserOpenFile { .. } => Answer::Button(1)"), "closing the window is Cancel");
        assert!(alerts.contains("if index == 0 {\n                    self.browser_run_confirmed(pane, command);"));
    }
}
