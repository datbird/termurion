//! The windows iTerm2 shows while something long runs: the Python Runtime Installer, "Hang
//! tight! Installing a new Python environment…", and the Importing window. Each is a `View`
//! with a status line the app updates; the work itself is the app's.

use crate::panel_ui::{Act, View};

/// `iTermOptionalComponentDownloadWindowController.xib:21`, `:33`, `:51`.
pub const RUNTIME_TITLE: &str = "Python Runtime Installer";
pub const RUNTIME_LABEL: &str = "Downloading optional components…";
/// `iTermOptionalComponentDownloadWindowController.m:417` (the label until the first phase).
pub const RUNTIME_INITIALIZING: &str = "Initializing…";
/// `iTermOptionalComponentDownloadWindowController.m:477`, `:480`.
pub const RUNTIME_CANCELED: &str = "Canceled";
pub const RUNTIME_FAILED: &str = "Download Failed";
pub const CANCEL: &str = "Cancel";
/// `iTermBuildingScriptWindowController.xib:27`.
pub const BUILDING_LABEL: &str = "Hang tight! Installing a new Python environment…";
/// `ImportingWindowController.xib:18`, `:43`, `:30`; `AppDelegate.swift` sets the first status.
pub const IMPORTING_TITLE: &str = "Importing…";
pub const IMPORTING_LABEL: &str = "Importing settings and data…";
pub const IMPORTING_STATUS_FIRST: &str = "Starting";

/// The Python Runtime Installer.
pub struct RuntimeInstallerView {
    label: String,
    progress: String,
    canceled: bool,
}

impl RuntimeInstallerView {
    pub fn new() -> RuntimeInstallerView {
        RuntimeInstallerView { label: RUNTIME_INITIALIZING.to_string(), progress: String::new(), canceled: false }
    }
}

impl Default for RuntimeInstallerView {
    fn default() -> Self {
        Self::new()
    }
}

impl View for RuntimeInstallerView {
    fn title(&self) -> String {
        RUNTIME_TITLE.to_string()
    }

    fn size(&self) -> (f64, f64) {
        (382.0, 142.0)
    }

    fn animating(&self) -> bool {
        !self.canceled
    }

    /// The first line of the job's output turns "Initializing…" into the installer's own label;
    /// later lines are the progress text.
    fn status(&mut self, line: &str) {
        if self.label == RUNTIME_INITIALIZING {
            self.label = RUNTIME_LABEL.to_string();
        }
        self.progress = line.chars().take(120).collect();
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        let mut acts = Vec::new();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(48.0));
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.label(&self.label);
                ui.add_space(6.0);
                ui.label(egui::RichText::new(if self.canceled { RUNTIME_CANCELED } else { &self.progress }).small());
            });
        });
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
            if crate::panel_ui::button(ui, CANCEL).clicked() {
                self.canceled = true;
                acts.push(Act::CancelWork);
            }
        });
        acts
    }

    /// The close box is the Cancel button (the title-bar close box is removed in iTerm2,
    /// `:407-410`, because it would bypass Cancel).
    fn on_close(&mut self) -> Vec<Act> {
        self.canceled = true;
        vec![Act::CancelWork]
    }
}

/// "Hang tight! Installing a new Python environment…" with a spinner.
pub struct BuildingScriptView;

impl View for BuildingScriptView {
    fn title(&self) -> String {
        "Termurion".to_string()
    }

    fn size(&self) -> (f64, f64) {
        (341.0, 97.0)
    }

    fn animating(&self) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        ui.vertical_centered(|ui| {
            ui.label(BUILDING_LABEL);
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(32.0));
        });
        Vec::new()
    }

    /// There is no close box to answer: the window ends with the work.
    fn on_close(&mut self) -> Vec<Act> {
        Vec::new()
    }
}

/// "Importing…": the label, the status line, a spinner.
pub struct ImportingView {
    status: String,
}

impl ImportingView {
    pub fn new() -> ImportingView {
        ImportingView { status: IMPORTING_STATUS_FIRST.to_string() }
    }
}

impl Default for ImportingView {
    fn default() -> Self {
        Self::new()
    }
}

impl View for ImportingView {
    fn title(&self) -> String {
        IMPORTING_TITLE.to_string()
    }

    fn size(&self) -> (f64, f64) {
        (360.0, 130.0)
    }

    fn animating(&self) -> bool {
        true
    }

    fn status(&mut self, line: &str) {
        self.status = line.trim_end_matches(['\r', '\n']).to_string();
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        ui.vertical_centered(|ui| {
            ui.label(IMPORTING_LABEL);
            ui.add_space(8.0);
            ui.add(egui::Spinner::new().size(32.0));
            ui.add_space(8.0);
            ui.label(&self.status);
        });
        Vec::new()
    }

    /// The import cannot be taken back half way: the window stays until it is done.
    fn on_close(&mut self) -> Vec<Act> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_starts_initializing_and_names_itself_on_the_first_line() {
        let mut v = RuntimeInstallerView::new();
        assert_eq!(v.label, "Initializing…");
        v.status("Downloading uv");
        assert_eq!((v.label.as_str(), v.progress.as_str()), ("Downloading optional components…", "Downloading uv"));
        assert_eq!(v.on_close(), vec![Act::CancelWork], "the close box is Cancel");
    }

    #[test]
    fn importing_shows_the_latest_status_line() {
        let mut v = ImportingView::new();
        assert_eq!(v.status, "Starting");
        v.status("Extracting Archive\n");
        assert_eq!(v.status, "Extracting Archive");
        assert!(v.on_close().is_empty());
    }
}

/// `SplitPanel.m:30`, `:32`; `SplitPanel.xib:47`, `:61`.
pub const SPLIT_VERTICAL_PROMPT: &str = "Split current pane vertically with profile:";
pub const SPLIT_HORIZONTAL_PROMPT: &str = "Split current pane horizontally with profile:";
pub const SPLIT: &str = "Split";

/// Split Vertically… / Split Horizontally…: pick the profile for the new pane
/// (`openSplitVerticallySheet:`, `SplitPanel`). Double-click splits at once
/// (`profileTableRowSelected:`).
pub struct SplitPanelView {
    vertical: bool,
    profiles: Vec<(String, String)>,
    selected: Option<usize>,
}

impl SplitPanelView {
    /// `profiles` are (guid, name) in the order the Profiles list shows them.
    pub fn new(vertical: bool, profiles: Vec<(String, String)>) -> SplitPanelView {
        SplitPanelView { vertical, profiles, selected: None }
    }

    pub fn prompt(vertical: bool) -> &'static str {
        if vertical { SPLIT_VERTICAL_PROMPT } else { SPLIT_HORIZONTAL_PROMPT }
    }

    /// What Split asks for with the current pick, if any.
    fn split(&self, i: Option<usize>) -> Option<Act> {
        let guid = i.or(self.selected).and_then(|i| self.profiles.get(i)).map(|p| p.0.clone())?;
        Some(Act::Split { profile: Some(guid), vertical: self.vertical })
    }
}

impl View for SplitPanelView {
    fn title(&self) -> String {
        Self::prompt(self.vertical).trim_end_matches(':').to_string()
    }

    fn size(&self) -> (f64, f64) {
        (464.0, 378.0)
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        let mut acts = Vec::new();
        ui.label(Self::prompt(self.vertical));
        ui.add_space(6.0);
        let mut double = None;
        egui::ScrollArea::vertical().max_height(ui.available_height() - 52.0).auto_shrink([false, false]).show(ui, |ui| {
            for (i, (_, name)) in self.profiles.iter().enumerate() {
                let r = ui.selectable_label(self.selected == Some(i), name);
                if r.clicked() {
                    self.selected = Some(i);
                }
                if r.double_clicked() {
                    double = Some(i);
                }
            }
        });
        if let Some(a) = double.and_then(|i| self.split(Some(i))) {
            acts.push(a);
        }
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::panel_ui::button_if(ui, self.selected.is_some(), SPLIT).clicked() {
                acts.extend(self.split(None));
            }
            if crate::panel_ui::button(ui, "Cancel").clicked() {
                acts.push(Act::Close);
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            acts.push(Act::Close);
        }
        acts
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn split_needs_a_pick_and_asks_for_the_direction() {
        let mut v = SplitPanelView::new(true, vec![("A".into(), "Default".into()), ("B".into(), "Other".into())]);
        assert_eq!(v.split(None), None, "the Split button is disabled until a profile is picked");
        v.selected = Some(1);
        assert_eq!(v.split(None), Some(Act::Split { profile: Some("B".into()), vertical: true }));
        assert_eq!(v.split(Some(0)), Some(Act::Split { profile: Some("A".into()), vertical: true }), "a double click splits with its row");
        assert_eq!(SplitPanelView::prompt(false), "Split current pane horizontally with profile:");
    }
}
