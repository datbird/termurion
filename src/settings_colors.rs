//! The Profiles > Colors tab laid out as iTerm2's (`PreferencePanel.xib`, `ProfilesColorsPreferencesViewController`):
//! a column of row headers at the left (Defaults:, Cursor:, Bold:, ANSI Colors:) and the wells,
//! switches and sliders at their right, with the captions the xib draws (Background, Foreground,
//! Normal, Bright, Transparent, Opaque). The rows are the schema's; each is drawn by the generic
//! control with its caption left to this grid.

use crate::profile::Profile;
use crate::schema::{self, Setting};

/// The width of the column of row headers, shared with the rows above the wells.
pub const HEADER_WIDTH: f32 = 130.0;

/// The caption under a Defaults, Cursor or Selection well pair, and the pair's second well.
const BACKGROUND: &str = "Background";
const FOREGROUND: &str = "Foreground";
/// The captions under the ANSI wells (`ProfilesColorsPreferencesViewController`'s two columns).
pub const NORMAL: &str = "Normal";
pub const BRIGHT: &str = "Bright";
/// The ends of the Faint text slider.
pub const TRANSPARENT: &str = "Transparent";
pub const OPAQUE: &str = "Opaque";

/// The key rows of the sixteen ANSI wells: the normal column and the bright column.
const ANSI: [(&str, &str); 8] = [
    ("Ansi 0 Color", "Ansi 8 Color"),
    ("Ansi 1 Color", "Ansi 9 Color"),
    ("Ansi 2 Color", "Ansi 10 Color"),
    ("Ansi 3 Color", "Ansi 11 Color"),
    ("Ansi 4 Color", "Ansi 12 Color"),
    ("Ansi 5 Color", "Ansi 13 Color"),
    ("Ansi 6 Color", "Ansi 14 Color"),
    ("Ansi 7 Color", "Ansi 15 Color"),
];

/// The wells whose iTerm2 label is the text of a neighbor (`colorWellRelatedViews`): the search lists them
/// as "<that text> color" (`ProfilesColors.ColorWellDisplayName`).
const RELATED_WELLS: [&str; 17] = [
    "Ansi 0 Color",
    "Ansi 1 Color",
    "Ansi 2 Color",
    "Ansi 3 Color",
    "Ansi 4 Color",
    "Ansi 5 Color",
    "Ansi 6 Color",
    "Ansi 7 Color",
    "Foreground Color",
    "Background Color",
    "Selection Color",
    "Selected Text Color",
    "Cursor Color",
    "Cursor Text Color",
    "IME Cursor Color",
    "Badge Color",
    "Ansi 8 Color",
];

/// The name the Settings search lists a Colors row under, and an extra word that finds it: the wells
/// iTerm2 names after a neighbor are "<text> color", and the grid's caption (Foreground, Background) is a word of it.
pub fn search_name(s: &Setting) -> (String, &'static str) {
    if s.tab != "Colors" || !RELATED_WELLS.contains(&s.key.as_str()) || s.key == "Ansi 8 Color" {
        return (s.label.clone(), "");
    }
    let caption = match s.key.as_str() {
        "Foreground Color" | "Cursor Text Color" | "Selected Text Color" => FOREGROUND,
        "Background Color" | "Cursor Color" | "Selection Color" => BACKGROUND,
        _ => "",
    };
    (format!("{} color", s.label.trim_end_matches(':')), caption)
}

fn row(key: &str) -> &'static Setting {
    schema::find(key).unwrap_or_else(|| panic!("the Colors tab names {key}, which is not a profile row"))
}

/// The header of a row (the schema label of the row iTerm2 hangs it on), right-aligned in the first column.
fn header(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !text.is_empty() {
            ui.label(text.trim());
        }
    });
}

struct Grid<'a> {
    profile: &'a mut Profile,
    dark: bool,
    committed: bool,
}

impl Grid<'_> {
    /// One control of the row `key`, enabled as its dependents allow (the Bold color follows its switch).
    fn put(&mut self, ui: &mut egui::Ui, key: &str) {
        let s = row(key);
        let enabled = crate::settings_ui::profile_row_enabled(self.profile, key, self.dark);
        ui.add_enabled_ui(enabled, |ui| {
            if crate::settings_ui::control_bare(ui, self.profile, s, self.dark) {
                self.committed = true;
            }
        });
    }

    /// A well and the caption after it.
    fn well(&mut self, ui: &mut egui::Ui, key: &str, caption: &str) {
        self.put(ui, key);
        ui.label(caption);
    }

    /// `header` and the controls the closure draws, left to right.
    fn line(&mut self, ui: &mut egui::Ui, head: &str, f: impl FnOnce(&mut Self, &mut egui::Ui)) {
        header(ui, head);
        ui.horizontal(|ui| f(self, ui));
        ui.end_row();
    }
}

/// Draws the tab's rows in the xib's order (top to bottom) and returns true when one committed a change.
pub fn body(ui: &mut egui::Ui, profile: &mut Profile, dark: bool) -> bool {
    let mut g = Grid { profile, dark, committed: false };
    let defaults = row("Foreground Color").label.clone();
    let cursor = row("Cursor Color").label.clone();
    let selection = row("Selection Color").label.clone();
    let background = row("Background Color").label.clone();
    let foreground = row("Cursor Text Color").label.clone();
    let ansi_head = row("Ansi 8 Color").label.clone();
    egui::Grid::new("colors-tab-grid").num_columns(2).spacing([8.0, 6.0]).min_col_width(HEADER_WIDTH).show(ui, |ui| {
        g.line(ui, &defaults, |g, ui| {
            g.well(ui, "Background Color", &background);
            g.well(ui, "Foreground Color", &foreground);
        });
        g.line(ui, &cursor, |g, ui| {
            g.well(ui, "Cursor Color", &background);
            g.well(ui, "Cursor Text Color", &foreground);
        });
        g.line(ui, "", |g, ui| g.put(ui, "Smart Cursor Color"));
        g.line(ui, &row("Cursor Boost").label.clone(), |g, ui| g.put(ui, "Cursor Boost"));
        g.line(ui, "", |g, ui| g.put(ui, "HDR Cursor"));
        g.line(ui, &row("Cursor Guide Color").label.clone(), |g, ui| {
            g.put(ui, "Cursor Guide Color");
            g.put(ui, "Use Cursor Guide");
        });
        g.line(ui, &row("IME Cursor Color").label.clone(), |g, ui| g.put(ui, "IME Cursor Color"));
        g.line(ui, "Bold:", |g, ui| g.put(ui, "Brighten Bold Text"));
        g.line(ui, "", |g, ui| {
            g.put(ui, "Bold Color");
            g.put(ui, "Use Bright Bold");
        });
        g.line(ui, &row("Underline Color").label.clone(), |g, ui| {
            g.put(ui, "Underline Color");
            g.put(ui, "Use Underline Color");
        });
        g.line(ui, &row("Link Color").label.clone(), |g, ui| g.put(ui, "Link Color"));
        g.line(ui, "Marks:", |g, ui| g.put(ui, "Use Theme Mark Colors"));
        g.line(ui, &row("Faint Text Alpha").label.clone(), |g, ui| {
            ui.vertical(|ui| {
                g.put(ui, "Faint Text Alpha");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(TRANSPARENT).small());
                    ui.add_space(60.0);
                    ui.label(egui::RichText::new(OPAQUE).small());
                });
            });
        });
        g.line(ui, &row("Minimum Contrast").label.clone(), |g, ui| g.put(ui, "Minimum Contrast"));
        g.line(ui, &row("Match Background Color").label.clone(), |g, ui| g.put(ui, "Match Background Color"));
        g.line(ui, &selection, |g, ui| {
            g.well(ui, "Selection Color", &background);
            g.well(ui, "Selected Text Color", &foreground);
        });
        g.line(ui, "", |g, ui| g.put(ui, "Use Selected Text Color"));
        for (i, (normal, bright)) in ANSI.iter().enumerate() {
            let name = row(normal).label.clone();
            g.line(ui, if i == 0 { &ansi_head } else { "" }, |g, ui| {
                ui.add_sized([70.0, 18.0], egui::Label::new(name));
                g.put(ui, normal);
                g.put(ui, bright);
            });
        }
        g.line(ui, "", |_, ui| {
            ui.add_space(70.0 + ui.spacing().item_spacing.x);
            ui.label(NORMAL);
            ui.add_space(8.0);
            ui.label(BRIGHT);
        });
        g.line(ui, "", |g, ui| g.put(ui, "Harmonize 256 Colors"));
        g.line(ui, &row("Badge Color").label.clone(), |g, ui| g.put(ui, "Badge Color"));
        g.line(ui, &row("Tab Color").label.clone(), |g, ui| {
            g.put(ui, "Tab Color");
            g.put(ui, "Use Tab Color");
        });
        g.line(ui, &row("Active Pane Border Color").label.clone(), |g, ui| {
            g.put(ui, "Active Pane Border Color");
            g.put(ui, "Use Active Pane Border");
        });
    });
    g.committed
}
