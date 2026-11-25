//! The About window: what Termurion is, which build, which system and renderer, and the repo.
//! One egui window on every OS, through `EguiHost`, like the Software Update window.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

pub const TITLE: &str = "About Termurion";
pub const LICENSE: &str = "GPL-3.0";
pub const OPEN_PROJECT: &str = "Open Project Page";
pub const COPY: &str = "Copy";
pub const OK: &str = "OK";

/// The short commit this build came from, or `unknown` (a source tarball has no git).
pub fn commit_of(raw: &str) -> &str {
    let t = raw.trim();
    if t.is_empty() { "unknown" } else { t }
}

/// `macOS arm64`, `Windows x86_64`, `Linux x86_64`.
pub fn system_line() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os} {arch}")
}

/// The renderer line, from the adapter the app already holds. The one place to change when the
/// Metal renderer replaces wgpu on macOS.
pub fn renderer_line(info: &wgpu::AdapterInfo) -> String {
    let api = match info.backend {
        wgpu::Backend::Dx12 => "DirectX 12".to_string(),
        other => format!("{other:?}"),
    };
    format!("wgpu {api}, {}", info.name)
}

/// The About lines, in order. Every line is plain text, so Copy sends them as they are.
pub fn lines(version: &str, commit: &str, date: &str, system: &str, renderer: &str) -> Vec<String> {
    vec![
        format!("Termurion {version}"),
        "iTerm2, ported to Windows, Linux and macOS.".to_string(),
        format!("Build: {} ({date})", commit_of(commit)),
        format!("System: {system}"),
        format!("Renderer: {renderer}"),
        format!("License: {LICENSE}"),
        crate::links::REPO.to_string(),
    ]
}

/// The lines for this build.
pub fn this_build(renderer: &str) -> Vec<String> {
    lines(crate::update::current_version(), env!("TERMURION_GIT_COMMIT"), env!("TERMURION_BUILD_DATE"), &system_line(), renderer)
}

/// What a button press asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    OpenProject,
    Copy,
    Close,
}

/// Enter closes the window only when no button has the keyboard; a focused button takes the
/// Enter itself (egui's own button activation).
pub fn enter_closes(button_focused: bool) -> bool {
    !button_focused
}

pub struct AboutWindow {
    host: EguiHost,
    lines: Vec<String>,
    choice: Option<Choice>,
}

impl AboutWindow {
    pub fn open(event_loop: &ActiveEventLoop, gpu: &crate::gpu_shared::GpuShared) -> Result<AboutWindow, String> {
        let lines = this_build(&gpu.renderer_name());
        let host = EguiHost::open(event_loop, gpu, TITLE, 460.0, 250.0)?;
        Ok(AboutWindow { host, lines, choice: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// All the text lines, for Copy.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CloseRequested) {
            self.choice = Some(Choice::Close);
            return true;
        }
        self.host.event(event)
    }

    pub fn take_choice(&mut self) -> Option<Choice> {
        self.choice.take()
    }

    pub fn redraw(&mut self) {
        let lines = self.lines.clone();
        let mut picked = None;
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                for (i, line) in lines.iter().enumerate() {
                    if i == 0 {
                        ui.label(egui::RichText::new(line.as_str()).strong().size(16.0));
                        ui.add_space(2.0);
                    } else {
                        ui.add(egui::Label::new(line.as_str()).selectable(true));
                    }
                }
                ui.add_space(10.0);
                let mut focused = false;
                ui.horizontal(|ui| {
                    let open = ui.button(OPEN_PROJECT);
                    if open.clicked() {
                        picked = Some(Choice::OpenProject);
                    }
                    let copy = ui.button(COPY);
                    if copy.clicked() {
                        picked = Some(Choice::Copy);
                    }
                    let ok = ui.button(OK);
                    if ok.clicked() {
                        picked = Some(Choice::Close);
                    }
                    focused = open.has_focus() || copy.has_focus() || ok.has_focus();
                });
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                if escape || (enter && enter_closes(focused)) {
                    picked = Some(Choice::Close);
                }
            });
        });
        if picked.is_some() {
            self.choice = picked;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        lines("1.2.3", "abc1234", "2026-11-23", "Linux x86_64", "wgpu Vulkan, Test GPU").join("\n")
    }

    /// About names the version and the project repository (not iTerm2's site).
    #[test]
    fn about_points_at_the_repo() {
        let t = sample();
        assert!(t.starts_with("Termurion 1.2.3\n"));
        assert!(t.ends_with(crate::links::REPO));
        assert!(!t.contains(&["iterm2", ".com"].concat()));
    }

    #[test]
    fn about_names_every_os_and_the_build() {
        let t = sample();
        assert!(!t.contains("for Windows"), "{t}");
        assert!(t.contains("Windows, Linux and macOS"));
        assert!(t.contains("Build: abc1234 (2026-11-23)"));
        assert!(t.contains("System: Linux x86_64"));
        assert!(t.contains("Renderer: wgpu Vulkan, Test GPU"));
        assert!(t.contains("License: GPL-3.0"));
    }

    #[test]
    fn the_commit_falls_back_to_unknown() {
        assert_eq!(commit_of(""), "unknown");
        assert_eq!(commit_of("  \n"), "unknown");
        assert_eq!(commit_of("abc1234\n"), "abc1234");
        assert!(lines("1", "", "2026-01-01", "x", "y").join("\n").contains("Build: unknown (2026-01-01)"));
    }

    #[test]
    fn the_build_values_are_baked_in() {
        let date = env!("TERMURION_BUILD_DATE");
        assert_eq!(date.len(), 10, "{date}");
        assert!(date.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() }));
        assert!(!commit_of(env!("TERMURION_GIT_COMMIT")).is_empty());
    }

    #[test]
    fn enter_leaves_a_focused_button_alone() {
        assert!(enter_closes(false));
        assert!(!enter_closes(true));
    }

    #[test]
    fn the_system_line_is_os_then_arch() {
        let s = system_line();
        assert!(s.contains(' ') && !s.contains("aarch64"), "{s}");
    }
}
