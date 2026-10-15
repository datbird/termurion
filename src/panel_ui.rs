//! The small windows iTerm2 builds from xibs (Crash Reporter, What's New, Importing, the shell
//! integration installer, Split Panel, the Python installers): one egui window around a `View`.
//! A view draws a frame and answers what the person did as `Act`s; `app/panels_app` does the
//! work, so a view stays free of the app's state and can be tested alone.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

/// What a view asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Closes this window.
    Close,
    /// Opens a web link in the default browser (judged again by the app).
    OpenUrl(String),
    /// Crash Reporter, Send: open a new issue with the log text (private paths already removed),
    /// then remember this crash as reported.
    CrashSend { title: String, body: String },
    /// Crash Reporter, Discard: delete the dump and remember it as handled.
    CrashDiscard,
    /// Crash Reporter, Remind Me Later (and the window's close box): keep the dump, ask again next launch.
    CrashLater,
    /// The shell integration installer: type this text into the session it was opened for.
    SendText(String),
    /// The shell integration installer: copy this text.
    CopyText(String),
    /// The shell integration installer, PowerShell: save the script and type the command that loads it.
    InstallPwsh,
    /// The installer's "Also install Termurion Utilities" box.
    SetInstallUtilities(bool),
    /// Cancels what an installer window waits for.
    CancelWork,
    /// Split Panel: split the pane with this profile (None: the pane's own) in this direction.
    Split { profile: Option<String>, vertical: bool },
    /// What's New: a page's "Try it now!" link (`iterm2whatsnew:<route>`).
    WhatsNewLink(String),
}

/// One window's contents.
pub trait View {
    fn title(&self) -> String;
    /// The window's inner size in points.
    fn size(&self) -> (f64, f64);
    /// Draws one frame.
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act>;
    /// Whether the view needs another frame at once (a spinner turns, a worker is waited for).
    fn animating(&self) -> bool {
        false
    }
    /// The window's close box: what that answers.
    fn on_close(&mut self) -> Vec<Act> {
        vec![Act::Close]
    }
    /// A line of status from the app (the installer's progress, the importer's current step).
    fn status(&mut self, _line: &str) {}
    /// Whether the view waits for the terminal's answer to text it typed (the shell installer).
    fn expecting(&self) -> bool {
        false
    }
    /// Lines the terminal printed since the view last typed; what the view does next.
    fn output(&mut self, _lines: &[String]) -> Vec<Act> {
        Vec::new()
    }
}

/// Which window a panel is, so the app finds the one it must update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Crash,
    WhatsNew,
    Importing,
    PythonRuntime,
    BuildingScript,
    ShellIntegration,
    SplitPanel,
}

pub struct Panel {
    host: EguiHost,
    pub kind: Kind,
    pub view: Box<dyn View>,
    /// The pane a pane-bound window (Split Panel, the installer) works on.
    pub pane: Option<u64>,
}

impl Panel {
    pub fn open(event_loop: &ActiveEventLoop, gpu: (&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue), kind: Kind, view: Box<dyn View>, pane: Option<u64>, dark: bool) -> Result<Panel, String> {
        let (w, h) = view.size();
        let host = EguiHost::open(event_loop, gpu.0, gpu.1, gpu.2, gpu.3, &view.title(), w, h)?;
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        host.focus();
        Ok(Panel { host, kind, view, pane })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    /// Whether the view wants frames at the animation cadence (`App::panels_tick` asks for them,
    /// so a spinner never runs the loop free).
    pub fn animating(&self) -> bool {
        self.view.animating()
    }

    /// A window event. `Some(acts)` for a close box or a frame; None for events egui took.
    pub fn event(&mut self, event: &WindowEvent) -> Option<Vec<Act>> {
        match event {
            WindowEvent::CloseRequested => Some(self.view.on_close()),
            WindowEvent::RedrawRequested => {
                let mut acts = Vec::new();
                let view = &mut self.view;
                self.host.paint(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| acts = view.ui(ui));
                });
                Some(acts)
            }
            other => {
                self.host.event(other);
                None
            }
        }
    }
}

/// A link-looking button: text the person can click, drawn as a link.
pub fn link(ui: &mut egui::Ui, text: &str, url: &str, acts: &mut Vec<Act>) {
    if ui.link(text).on_hover_text(url).clicked() {
        acts.push(Act::OpenUrl(url.to_string()));
    }
}

/// A push button of the width iTerm2's xibs use.
pub fn button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).min_size(egui::vec2(76.0, 24.0)))
}

/// Same, greyed out when `enabled` is false.
pub fn button_if(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(text).min_size(egui::vec2(76.0, 24.0)))
}
