//! The small xib-style windows (`panel_ui`): opening one, routing its events, and doing what its
//! buttons mean. The Crash Reporter at launch is the first.

use super::*;
use crate::crash_report::{self, CrashView};
use crate::panel_ui::{Act, Kind, Panel, View};
use crate::panel_views::{BuildingScriptView, ImportingView, RuntimeInstallerView};
use crate::whats_new::{self, WhatsNewView};

/// `iTermOpenWhatsNewURL`'s warning when a screenshot is asked for with no window
/// (`iTermOnboardingWindowController.m`, `Onboarding.NoTerminalWindow*`).
pub(super) const NO_WINDOW_HEADING: &str = "No Terminal Window";
pub(super) const NO_WINDOW_TEXT: &str = "You need an open terminal window to make a screenshot.";

/// The provisioner jobs that show a window while they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Long {
    /// Downloading the runtime: the Python Runtime Installer.
    Install,
    /// Building an environment: "Hang tight!".
    Build,
}

/// The most of `host.log` read for a crash report.
const LOG_READ: u64 = 2 * 1024 * 1024;

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    /// Opens a panel of `kind` (for `pane` when it works on one), or raises the one already open.
    pub(super) fn open_panel(&mut self, event_loop: &ActiveEventLoop, kind: Kind, pane: Option<PaneId>, view: Box<dyn View>) -> bool {
        if let Some(p) = self.panels.iter().find(|p| p.kind == kind && p.pane == pane) {
            p.focus();
            return false;
        }
        let dark = self.dark();
        let Some(g) = self.gpu.as_ref() else { return false };
        match Panel::open(event_loop, (&g.instance, &g.adapter, &g.device, &g.queue), kind, view, pane, dark) {
            Ok(p) => {
                self.panels.push(p);
                true
            }
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                false
            }
        }
    }

    /// A line for the panels of `kind` (the installer's progress, the importer's step).
    pub(super) fn panel_status(&mut self, kind: Kind, line: &str) {
        for p in self.panels.iter_mut().filter(|p| p.kind == kind) {
            p.view.status(line);
            p.request_redraw();
        }
    }

    /// Opens or closes the panel of `kind` to match `want`.
    fn sync_panel(&mut self, event_loop: &ActiveEventLoop, kind: Kind, want: bool, make: impl FnOnce() -> Box<dyn View>) {
        let open = self.panels.iter().any(|p| p.kind == kind);
        if want && !open {
            self.open_panel(event_loop, kind, None, make());
        } else if !want && open {
            self.close_panel(kind, None);
        }
    }

    /// The windows that follow work in progress, and the animation cadence of the ones that move
    /// (a spinner asks for a frame every 33 ms, never a free run). The time the next frame is due.
    pub(super) fn panels_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        self.import_stage_tick();
        let installing = self.provision_long.values().any(|k| *k == Long::Install);
        let building = !installing && self.provision_long.values().any(|k| *k == Long::Build);
        self.sync_panel(event_loop, Kind::PythonRuntime, installing, || Box::new(RuntimeInstallerView::new()));
        self.sync_panel(event_loop, Kind::BuildingScript, building, || Box::new(BuildingScriptView));
        let importing = self.import_stage.is_some();
        self.sync_panel(event_loop, Kind::Importing, importing, || Box::new(ImportingView::new()));
        self.regex_visualization(event_loop);
        let hint_until = self.paste_view_tick(event_loop);
        // The trigger check times of the session Edit Session points at, for the editor's CPU Time row.
        let editing = self.settings.as_ref().and_then(|s| s.edit_session);
        crate::settings_triggers::set_cpu(editing.and_then(|p| self.panes.get(&p)).map(|p| p.screen.trigger_cpu()).unwrap_or_default());
        if let Some(pane) = self.shell_wizard_due.take() {
            self.open_shell_wizard(event_loop, pane);
        }
        let watching = self.wizard_watch_tick(event_loop);
        let mut moving = false;
        for p in self.panels.iter().filter(|p| p.animating()) {
            p.request_redraw();
            moving = true;
        }
        let spin = moving.then(|| Instant::now() + Duration::from_millis(33));
        [spin, watching, hint_until].into_iter().flatten().min()
    }

    /// Types the installer's text into the session (the keyboard's Enter for each line end) and
    /// starts reading what the terminal answers after it.
    fn wizard_send(&mut self, pane: PaneId, text: &str) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.exited.is_some() || p.ended {
            return self.banners.set("action", super::cmdinfo_app::ENDED);
        }
        p.scroll(Scroll::Bottom);
        p.input(text.replace('\n', "\r").as_bytes());
        let at = p.screen.cursor_abs();
        self.wizard_watch.insert(pane, (at, at));
    }

    /// The terminal's answer to the installer's text: the lines printed since it typed, to the
    /// window (iTerm2's `iTermExpect`), polled every 100 ms while the window waits.
    fn wizard_watch_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let waiting: Vec<(Kind, PaneId)> = self.panels.iter().filter(|p| p.view.expecting()).filter_map(|p| Some((p.kind, p.pane?))).collect();
        let mut again = false;
        for (kind, pane) in waiting {
            again = true;
            let Some((sent, fed)) = self.wizard_watch.get(&pane).copied() else { continue };
            let Some(p) = self.panes.get(&pane) else { continue };
            let now = p.screen.cursor_abs();
            let lines: Vec<String> = (fed + 1..=now).filter_map(|a| p.screen.abs_line_text(a)).collect();
            self.wizard_watch.insert(pane, (sent, (now - 1).max(fed)));
            if lines.is_empty() {
                continue;
            }
            let acts = self.panels.iter_mut().find(|pn| pn.kind == kind && pn.pane == Some(pane)).map(|pn| pn.view.output(&lines)).unwrap_or_default();
            for act in acts {
                self.panel_act(event_loop, kind, Some(pane), act);
            }
        }
        again.then(|| Instant::now() + Duration::from_millis(100))
    }

    /// The trigger editor's visualization button: the diagram in a popover (iTerm2's
    /// `iTermRegexVisualizationViewController` in an NSPopover), or closed again. Without the web
    /// runtime the page opens in the browser, as Smart Selection's does.
    fn regex_visualization(&mut self, event_loop: &ActiveEventLoop) {
        use crate::web_popover::Kind as PopKind;
        let showing = matches!(&self.web_popover, Some((None, p)) if matches!(p.kind(), PopKind::Html { .. }));
        if let Some(regex) = crate::settings_triggers::take_visualization() {
            if regex.is_empty() && showing {
                self.web_popover = None;
            } else {
                let dark = self.dark();
                if self.web_runtime() == crate::webhost::Runtime::Missing {
                    if let Err(e) = crate::regex_railroad::open(&regex, dark) {
                        self.banners.set("action", format!("Could not show the visualization: {e}"));
                    }
                } else {
                    let html = crate::regex_railroad::page(&regex, dark);
                    let size = crate::regex_railroad::page_size(&html);
                    let at = self.pointer_screen();
                    self.open_web_popover(None, PopKind::Html { html }, size, at, event_loop);
                }
            }
        }
        let showing = matches!(&self.web_popover, Some((None, p)) if matches!(p.kind(), PopKind::Html { .. }));
        crate::settings_triggers::set_visualizing(showing);
    }

    pub(super) fn close_panel(&mut self, kind: Kind, pane: Option<PaneId>) {
        if let (Kind::ShellIntegration, Some(p)) = (kind, pane) {
            self.wizard_watch.remove(&p);
        }
        self.panels.retain(|p| !(p.kind == kind && p.pane == pane));
    }

    /// Routes a window event to a panel. False when `window` is not one.
    pub(super) fn panels_event(&mut self, event_loop: &ActiveEventLoop, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some(i) = self.panels.iter().position(|p| p.id() == window) else { return false };
        let (kind, pane) = (self.panels[i].kind, self.panels[i].pane);
        if let Some(acts) = self.panels[i].event(event) {
            for act in acts {
                self.panel_act(event_loop, kind, pane, act);
            }
        }
        true
    }

    /// What a panel's button means.
    fn panel_act(&mut self, event_loop: &ActiveEventLoop, kind: Kind, pane: Option<PaneId>, act: Act) {
        match act {
            Act::Close => self.close_panel(kind, pane),
            Act::OpenUrl(url) => {
                if let Err(e) = semantic_ui::shell_open(&url, false) {
                    self.banners.set("action", e);
                }
            }
            Act::CrashSend { title, body } => {
                let url = crash_report::issue_url(&title, &crash_report::scrub(&body, &crash_report::private_folders()));
                if let Err(e) = semantic_ui::shell_open(&url, false) {
                    return self.banners.set("action", format!("Couldn\u{2019}t submit crash report: {e}"));
                }
                self.crash_reported();
                self.close_panel(kind, pane);
            }
            Act::CrashDiscard => {
                if let Some(dump) = self.crash_dump.take() {
                    let _ = std::fs::remove_file(dump);
                }
                self.crash_reported();
                self.close_panel(kind, pane);
            }
            Act::Split { profile, vertical } => {
                self.close_panel(kind, pane);
                self.split(if vertical { Axis::Row } else { Axis::Column }, profile.as_deref());
            }
            Act::WhatsNewLink(route) => self.whats_new_link(event_loop, &route),
            Act::CancelWork if kind == Kind::ShellIntegration => self.close_panel(kind, pane),
            Act::CancelWork => {
                if let Some(p) = &self.provisioner {
                    p.cancel();
                }
            }
            Act::SendText(text) => {
                if let Some(pane) = pane {
                    self.wizard_send(pane, &text);
                }
            }
            Act::InstallPwsh => {
                if let (Some(pane), Some(command)) = (pane, self.write_pwsh_script()) {
                    self.wizard_send(pane, &command);
                }
            }
            Act::SetInstallUtilities(on) => {
                self.globals.set(super::menu::INSTALL_UTILITIES_KEY, toml::Value::Boolean(on));
                self.save_globals();
            }
            Act::CrashLater => {
                self.crash_dump = None;
                self.close_panel(kind, pane);
            }
            _ => {}
        }
    }

    /// Window > Split Horizontally… and Split Vertically… (`_openSplitSheetForVertical:`): the
    /// profile picker for the new pane.
    pub(super) fn open_split_panel(&mut self, event_loop: &ActiveEventLoop, vertical: bool) {
        let Some(pane) = self.focused_id() else { return };
        let profiles = self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect();
        self.open_panel(event_loop, Kind::SplitPanel, Some(pane), Box::new(crate::panel_views::SplitPanelView::new(vertical, profiles)));
    }

    /// A What's New page's link (`iTermOpenWhatsNewURL`).
    fn whats_new_link(&mut self, event_loop: &ActiveEventLoop, route: &str) {
        match route {
            "claudecode" => self.claude_install_due = true,
            "screenshot" => {
                if self.tabs.is_empty() {
                    self.ask_scripts(super::scripts_alerts::ask(NO_WINDOW_HEADING, NO_WINDOW_TEXT, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing));
                } else {
                    self.make_screenshot(event_loop);
                }
            }
            "orchestration" => {
                if !self.chat_window_enter(event_loop) {
                    return;
                }
                if let Some(id) = self.chat_create(crate::chat_toolbar::NEW_CHAT_TITLE, None, None, "", vec![]) {
                    self.show_chat(Some(id.clone()));
                    if let Err(e) = self.set_orchestration(&id, true) {
                        self.banners.set("chat_orchestration", e);
                    }
                }
            }
            _ => {}
        }
    }

    /// At launch: notes the version that ran, and opens What's New after an update, once.
    fn whats_new_launch(&mut self, event_loop: &ActiveEventLoop) {
        let current = crate::update::current_version();
        let previous = Some(self.globals.string(whats_new::LAST_VERSION_KEY)).filter(|v| !v.is_empty());
        let show = whats_new::should_show(previous.as_deref(), current, self.globals.bool(whats_new::SHOWN_KEY));
        self.globals.set(whats_new::LAST_VERSION_KEY, toml::Value::String(current.to_string()));
        if show {
            self.globals.set(whats_new::SHOWN_KEY, toml::Value::Boolean(true));
        }
        self.save_globals();
        if show {
            self.open_panel(event_loop, Kind::WhatsNew, None, Box::new(WhatsNewView::new(current)));
        }
    }

    /// Remembers that every crash up to now was answered (`UKCrashReporterLastCrashReportDate`).
    fn crash_reported(&mut self) {
        self.globals.set(crash_report::LAST_REPORT_KEY, toml::Value::Float(now_secs()));
        self.save_globals();
        self.crash_dump = None;
    }

    /// At the first tick with a window: a dump written since the last answer opens the Crash
    /// Reporter (`UKCrashReporterCheckForCrash`).
    pub(super) fn crash_check_tick(&mut self, event_loop: &ActiveEventLoop) {
        if self.crash_checked || self.gpu.is_none() {
            return;
        }
        self.crash_checked = true;
        self.whats_new_launch(event_loop);
        let last = self.globals.number(crash_report::LAST_REPORT_KEY).unwrap_or(0.0);
        let Some((dump, _)) = crash_report::pending(&crate::crashdump::crashes_dir(), last) else { return };
        let file = dump.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let log = read_tail(&crate::host::log_path(), LOG_READ);
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        let text = crash_report::report_text(crate::update::current_version(), &crash_report::windows_version(), cpus, &file, &log);
        let text = crash_report::scrub(&text, &crash_report::private_folders());
        let user = std::env::var("USERNAME").unwrap_or_default();
        let view = CrashView::new(crash_report::message(&user), text);
        self.crash_dump = Some(dump);
        self.open_panel(event_loop, Kind::Crash, None, Box::new(view));
    }
}

/// The last `max` bytes of a text file, from a line start; empty when it cannot be read.
fn read_tail(path: &std::path::Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    if len > max && f.seek(SeekFrom::Start(len - max)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = f.take(max).read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if len > max { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()) } else { text }
}
