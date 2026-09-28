//! Settings > General > AI, drawn inside the settings window (egui): its three sub-tabs and
//! the state they keep. Not `#[cfg(windows)]`, so its pure helpers test anywhere;
//! `SettingsWindow::show_ai` lives in `settings_ui.rs`.

/// The AI section's sub-tabs (`X:7171-7866`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubTab {
    #[default]
    General,
    Prompts,
    Features,
}

/// What the AI section keeps between frames. Tasks 7 to 9 add private fields.
#[derive(Default)]
pub struct State {
    pub sub: SubTab,
    /// The section was revealed (the gate's Reveal, Codecierge's Reveal Setting): the next
    /// frame scrolls it into view.
    pub reveal: bool,
}
