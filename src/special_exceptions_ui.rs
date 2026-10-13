//! Manage Special Exceptions (iTerm2's `SpecialExceptionsWindowController.swift`, `SE`, and its
//! xib): the window that edits a profile's Special Font Config. A table of the code point ranges
//! and the fonts that draw them; "+" and "Add Preset..." and "-" and "Edit..." change it through
//! the "Edit Special Exception" sheet, Import and Export read and write a JSON file, OK keeps the
//! table and Cancel drops the edits. The table comes from a profile, a file or the API, so every
//! entry goes through `font_table`'s bounds, and the installed fonts are listed once when the
//! window opens (never per frame).

use crate::font_table::{self, Check, Config, Entry};

/// The most installed families the font list holds.
pub const MAX_FONTS: usize = 4096;
/// The most font names the picker lists at once (the filter narrows the rest).
const MAX_SHOWN: usize = 200;
/// The largest file Import reads.
const MAX_FILE: u64 = font_table::MAX_TEXT as u64;
/// The explanation under the title (`SpecialExceptionsWindowController.xib`).
pub const EXPLANATION: &str = "Special Exceptions override fonts for specific Unicode ranges or scripts.";
pub const TITLE: &str = "Manage Special Exceptions";
pub const SHEET_TITLE: &str = "Edit Special Exception";
pub const UNREADABLE_NOTICE: &str = "The stored Special Font Config could not be read; it is kept unchanged until you edit it.";
pub const ADD_PRESET: &str = "Add Preset\u{2026}";
pub const INSTALL_NERD: &str = "Install Nerd Font Bundle\u{2026}";

pub enum Outcome {
    Ok(Config),
    /// OK on a stored value the window could not read, with nothing edited: the stored text stays.
    Keep,
    Cancel,
}

/// The "Edit Special Exception" sheet's fields, as text (`SpecialExceptionEntryEditorWindowController`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    /// The row being edited; None when adding.
    pub row: Option<usize>,
    pub start: String,
    pub end: String,
    pub has_destination: bool,
    pub destination: String,
    pub font: String,
    /// The font list's filter text.
    pub filter: String,
}

impl Sheet {
    pub fn blank() -> Sheet {
        Sheet::default()
    }

    /// The fields an entry fills (`copyEntryToControls`): the code points of the font, and the
    /// ones the program sends when there is a destination.
    pub fn for_entry(row: Option<usize>, e: &Entry) -> Sheet {
        Sheet {
            row,
            start: font_table::format_code_point(e.start),
            end: font_table::format_code_point(e.start.saturating_add(e.count) - 1),
            has_destination: e.destination.is_some(),
            destination: e.destination.map(font_table::format_code_point).unwrap_or_default(),
            font: e.font_name.clone(),
            filter: String::new(),
        }
    }

    /// A preset's range, no font yet.
    pub fn for_preset(i: usize) -> Option<Sheet> {
        let (_, first, last) = *font_table::PRESETS.get(i)?;
        Some(Sheet { start: font_table::format_code_point(first), end: font_table::format_code_point(last), ..Sheet::blank() })
    }

    /// What the fields say (`checkedRange`): an unreadable destination first, then the range.
    pub fn check(&self, others: &[Entry]) -> Check {
        let destination = self.has_destination.then(|| font_table::parse_code_point(&self.destination));
        if destination == Some(None) {
            return Check::InvalidDestination;
        }
        font_table::check(font_table::parse_code_point(&self.start), font_table::parse_code_point(&self.end), destination.flatten(), others)
    }

    /// The installed spelling of the chosen font, when it is installed.
    fn chosen<'a>(&self, fonts: &'a [String]) -> Option<&'a str> {
        fonts.iter().find(|f| f.eq_ignore_ascii_case(self.font.trim())).map(String::as_str)
    }

    /// The entry the fields make, when the range is valid and a font is chosen
    /// (`copyControlsToEntryIfValid`). iTerm2's editor leaves the point size out.
    pub fn entry(&self, others: &[Entry], fonts: &[String]) -> Option<Entry> {
        if self.check(others) != Check::Valid {
            return None;
        }
        let font = self.chosen(fonts)?;
        let (start, end) = (font_table::parse_code_point(&self.start)?, font_table::parse_code_point(&self.end)?);
        let destination = if self.has_destination { Some(font_table::parse_code_point(&self.destination)?) } else { None };
        Some(Entry { start, count: end - start + 1, destination, font_name: font.to_string(), point_size: None })
    }

    /// The preview's text (`updatePreview`): iTerm2's words for a refusal, "No font selected." when
    /// the range is fine and the font is not, else the run's characters.
    pub fn preview(&self, others: &[Entry], fonts: &[String]) -> String {
        let check = self.check(others);
        if let Some(text) = font_table::check_text(&check) {
            return text.to_string();
        }
        if check != Check::Valid {
            return String::new();
        }
        if self.chosen(fonts).is_none() {
            return font_table::NO_FONT.to_string();
        }
        match (font_table::parse_code_point(&self.start), font_table::parse_code_point(&self.end)) {
            (Some(start), Some(end)) => font_table::preview_text(start, end - start + 1),
            _ => String::new(),
        }
    }
}

/// The font list: each installed family once (ignoring case), sorted, capped.
fn font_list(mut fonts: Vec<String>) -> Vec<String> {
    fonts.retain(|f| !f.is_empty());
    fonts.sort_by_cached_key(|f| (f.to_lowercase(), f.clone()));
    fonts.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    fonts.truncate(MAX_FONTS);
    fonts
}

pub struct Window {
    config: Config,
    fonts: Vec<String>,
    selected: Option<usize>,
    sheet: Option<Sheet>,
    /// A problem to show (the heading is `font_table::IMPORT_HEADING`).
    message: Option<String>,
    /// The stored value could not be read (the table shown is empty, not the stored one).
    unreadable: bool,
    /// An explicit edit (add, edit, delete, import, preset) was made.
    edited: bool,
}

impl Window {
    /// The window for `config`; `fonts` is the installed families, listed once here.
    pub fn open(config: Config, fonts: Vec<String>) -> Window {
        Window { config, fonts: font_list(fonts), selected: None, sheet: None, message: None, unreadable: false, edited: false }
    }

    /// The window for a stored value that does not parse: an empty table and a notice, and the
    /// stored text is kept unless the person edits.
    pub fn open_unreadable(fonts: Vec<String>) -> Window {
        Window { unreadable: true, ..Window::open(Config::with_entries(Vec::new()), fonts) }
    }

    /// The notice shown over an unreadable stored value, until the person edits.
    pub fn notice(&self) -> Option<&'static str> {
        (self.unreadable && !self.edited).then_some(UNREADABLE_NOTICE)
    }

    /// What OK does: keeps an unreadable stored value that was not edited, else hands back the table.
    pub fn finish(&self) -> Outcome {
        if self.unreadable && !self.edited {
            Outcome::Keep
        } else {
            Outcome::Ok(self.config.clone())
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn fonts(&self) -> &[String] {
        &self.fonts
    }

    pub fn sheet(&self) -> Option<&Sheet> {
        self.sheet.as_ref()
    }

    pub fn sheet_mut(&mut self) -> Option<&mut Sheet> {
        self.sheet.as_mut()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn select(&mut self, row: Option<usize>) {
        self.selected = row.filter(|r| *r < self.config.entries.len());
    }

    /// "+": a blank sheet.
    pub fn begin_add(&mut self) {
        self.sheet = Some(Sheet::blank());
    }

    /// "Add Preset...": a sheet with the preset's range (index into `font_table::PRESETS`).
    pub fn begin_preset(&mut self, i: usize) {
        if let Some(sheet) = Sheet::for_preset(i) {
            self.sheet = Some(sheet);
        }
    }

    /// Edit: a sheet with the selected row.
    pub fn begin_edit(&mut self) {
        if let Some(row) = self.selected {
            if let Some(e) = self.config.entries.get(row) {
                self.sheet = Some(Sheet::for_entry(Some(row), e));
            }
        }
    }

    /// The entries a sheet's range is checked against: all but the row being edited
    /// (`editorRangeIsValid`).
    fn others(&self, row: Option<usize>) -> Vec<Entry> {
        self.config.entries.iter().enumerate().filter(|(i, _)| Some(*i) != row).map(|(_, e)| e.clone()).collect()
    }

    /// The sheet's OK: false (and the sheet stays) while its fields are not valid.
    pub fn accept_sheet(&mut self) -> bool {
        let Some(sheet) = self.sheet.as_ref() else { return false };
        let others = self.others(sheet.row);
        let Some(entry) = sheet.entry(&others, &self.fonts) else { return false };
        let mut entries = others;
        entries.push(entry);
        self.config = Config::with_entries(entries);
        self.selected = None;
        self.sheet = None;
        self.edited = true;
        true
    }

    pub fn cancel_sheet(&mut self) {
        self.sheet = None;
    }

    /// "-": the selected row goes.
    pub fn delete_selected(&mut self) {
        if let Some(row) = self.selected.filter(|r| *r < self.config.entries.len()) {
            self.config.entries.remove(row);
            self.edited = true;
        }
        self.selected = None;
    }

    /// Import: the file's table replaces this one, or a message says what is wrong.
    pub fn import_text(&mut self, text: &str) {
        let fonts = &self.fonts;
        let installed = |name: &str| fonts.iter().any(|f| f.eq_ignore_ascii_case(name));
        match font_table::import(text, &installed) {
            Ok(config) => {
                self.config = config;
                self.selected = None;
                self.message = None;
                self.edited = true;
            }
            Err(e) => self.message = Some(e),
        }
    }

    /// Export: the table as the JSON a file holds.
    pub fn export_text(&self) -> String {
        self.config.string()
    }

    /// Reads a picked file for Import.
    fn import_file(&mut self, path: &std::path::Path) {
        let path = match crate::localfs::judge_io(path) {
            Ok(p) => p,
            Err(e) => return self.message = Some(e.to_string()),
        };
        match crate::localfs::stat(&path) {
            Ok(m) if m.len <= MAX_FILE => match crate::localfs::read_to_string(&path) {
                Ok(text) => self.import_text(&text),
                Err(e) => self.message = Some(e.to_string()),
            },
            Ok(_) => self.message = Some(font_table::IMPORT_MALFORMED.to_string()),
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Writes a picked file for Export.
    fn export_file(&mut self, path: &std::path::Path) {
        if let Err(e) = crate::localfs::judge_io(path).and_then(|p| crate::localfs::write(&p, self.export_text())) {
            self.message = Some(e.to_string());
        }
    }

    /// One frame of the window. OK or Cancel (or Escape) ends it.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
        let ctx = ui.ctx().clone();
        let mut out = None;
        let modal = self.sheet.is_some() || self.message.is_some();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_enabled_ui(!modal, |ui| {
                ui.label(egui::RichText::new(EXPLANATION).small());
                if let Some(notice) = self.notice() {
                    ui.colored_label(ui.visuals().warn_fg_color, notice);
                }
                ui.add_space(4.0);
                let room = (ui.available_height() - 84.0).max(80.0);
                let mut edit = false;
                egui::ScrollArea::vertical().max_height(room).auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("special-exceptions").striped(true).num_columns(2).min_col_width(160.0).show(ui, |ui| {
                        ui.strong("Code points");
                        ui.strong("Font");
                        ui.end_row();
                        for (i, e) in self.config.entries.iter().enumerate() {
                            let r = ui.selectable_label(self.selected == Some(i), font_table::row_range_text(e));
                            ui.label(&e.font_name);
                            ui.end_row();
                            if r.clicked() {
                                self.selected = Some(i);
                            }
                            if r.double_clicked() {
                                self.selected = Some(i);
                                edit = true;
                            }
                        }
                    });
                });
                if edit {
                    self.begin_edit();
                }
                ui.horizontal(|ui| {
                    if ui.button("+").clicked() {
                        self.begin_add();
                    }
                    let mut preset = None;
                    ui.menu_button(ADD_PRESET, |ui| {
                        let mut order: Vec<usize> = (0..font_table::PRESETS.len()).collect();
                        order.sort_by_key(|i| font_table::PRESETS[*i].0.to_lowercase());
                        for i in order {
                            if ui.button(font_table::PRESETS[i].0).clicked() {
                                preset = Some(i);
                                ui.close();
                            }
                        }
                    });
                    if let Some(i) = preset {
                        self.begin_preset(i);
                    }
                    if ui.add_enabled(self.selected.is_some(), egui::Button::new("-")).clicked() {
                        self.delete_selected();
                    }
                    if ui.add_enabled(self.selected.is_some(), egui::Button::new("Edit\u{2026}")).clicked() {
                        self.begin_edit();
                    }
                    ui.separator();
                    if ui.button("Import\u{2026}").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("Special Exceptions", &["json", "itse"]).pick_file() {
                            self.import_file(&path);
                        }
                    }
                    if ui.button("Export\u{2026}").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("Special Exceptions", &["json", "itse"]).set_file_name("special-exceptions.json").save_file() {
                            self.export_file(&path);
                        }
                    }
                });
                // Termurion does not download fonts (Decision): the button is there, disabled.
                ui.add_enabled(false, egui::Button::new(INSTALL_NERD)).on_disabled_hover_text(font_table::NERD_DISABLED);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        out = Some(self.finish());
                    }
                    if ui.button("Cancel").clicked() {
                        out = Some(Outcome::Cancel);
                    }
                });
            });
        });
        if self.sheet.is_some() {
            self.sheet_ui(&ctx);
        }
        if let Some(text) = self.message.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("special-exceptions-message")).show(&ctx, |ui| {
                ui.set_max_width(380.0);
                ui.heading(font_table::IMPORT_HEADING);
                ui.label(&text);
                ui.add_space(6.0);
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
            if close {
                self.message = None;
            }
        }
        if out.is_none() && !modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Some(Outcome::Cancel);
        }
        out
    }

    /// The "Edit Special Exception" sheet.
    fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(mut sheet) = self.sheet.take() else { return };
        let others = self.others(sheet.row);
        let (mut ok, mut cancel) = (false, false);
        egui::Modal::new(egui::Id::new("special-exceptions-sheet")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(SHEET_TITLE);
            ui.horizontal(|ui| {
                ui.label("Use Code Points");
                ui.add(egui::TextEdit::singleline(&mut sheet.start).hint_text("U+xxx").desired_width(100.0));
                ui.label("to");
                ui.add(egui::TextEdit::singleline(&mut sheet.end).hint_text("U+xxx").desired_width(100.0));
            });
            ui.horizontal(|ui| {
                ui.label("Of Font:");
                ui.add(egui::TextEdit::singleline(&mut sheet.font).desired_width(200.0));
                ui.menu_button("\u{25be}", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut sheet.filter).hint_text("Filter").desired_width(220.0));
                    let needle = sheet.filter.to_lowercase();
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        let mut shown = 0;
                        for f in self.fonts.iter().filter(|f| needle.is_empty() || f.to_lowercase().contains(&needle)) {
                            if shown == MAX_SHOWN {
                                break;
                            }
                            shown += 1;
                            if ui.selectable_label(sheet.font.eq_ignore_ascii_case(f), f).clicked() {
                                sheet.font = f.clone();
                                ui.close();
                            }
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut sheet.has_destination, "For characters starting at");
                ui.add_enabled(sheet.has_destination, egui::TextEdit::singleline(&mut sheet.destination).hint_text("U+xxx").desired_width(100.0));
            });
            ui.separator();
            let preview = sheet.preview(&others, &self.fonts);
            egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                ui.label(preview);
            });
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.add_enabled(sheet.entry(&others, &self.fonts).is_some(), egui::Button::new("OK")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        self.sheet = Some(sheet);
        if ok {
            self.accept_sheet();
        } else if cancel {
            self.cancel_sheet();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Vec<String> {
        vec!["Consolas".into(), "Meiryo".into(), "Symbols Nerd Font Mono".into()]
    }

    fn typed(w: &mut Window, start: &str, end: &str, font: &str) {
        let s = w.sheet_mut().expect("a sheet is open");
        (s.start, s.end, s.font) = (start.into(), end.into(), font.into());
    }

    #[test]
    fn the_edit_sheet_follows_iterm2() {
        // An entry fills the fields as `copyEntryToControls` does.
        let e = Entry { start: 0xE000, count: 0x1900, destination: None, font_name: "Consolas".into(), point_size: None };
        let s = Sheet::for_entry(Some(0), &e);
        assert_eq!((s.start.as_str(), s.end.as_str(), s.font.as_str(), s.has_destination), ("U+e000", "U+f8ff", "Consolas", false));
        let d = Entry { start: 0x41, count: 26, destination: Some(0x10000), font_name: "Consolas".into(), point_size: None };
        let s = Sheet::for_entry(None, &d);
        assert_eq!((s.start.as_str(), s.end.as_str(), s.has_destination, s.destination.as_str()), ("U+41", "U+5a", true, "U+10000"));
        // The text of each refusal, and a valid range builds the entry.
        let mut s = Sheet { start: "0x41".into(), end: "0x50".into(), font: "Consolas".into(), ..Sheet::blank() };
        assert_eq!(s.check(&[]), Check::Ascii);
        assert_eq!(s.preview(&[], &fonts()), font_table::ASCII_ERROR);
        assert_eq!(s.entry(&[], &fonts()), None);
        s.start = "U+100".into();
        s.end = "".into();
        assert_eq!(s.check(&[]), Check::Incomplete);
        assert_eq!(s.preview(&[], &fonts()), "");
        (s.start, s.end) = ("U+200".into(), "U+100".into());
        assert_eq!(s.preview(&[], &fonts()), font_table::INVALID_RANGE);
        (s.start, s.end) = ("U+100".into(), "U+110001".into());
        assert_eq!(s.preview(&[], &fonts()), font_table::LIMIT_TOO_LARGE);
        (s.start, s.end) = ("U+100".into(), "U+1ff".into());
        assert_eq!(s.check(&[]), Check::Valid);
        assert_eq!(s.entry(&[], &fonts()), Some(Entry { start: 0x100, count: 0x100, destination: None, font_name: "Consolas".into(), point_size: None }));
        let taken = [Entry { start: 0x1f0, count: 4, destination: None, font_name: "Meiryo".into(), point_size: None }];
        assert_eq!(s.preview(&taken, &fonts()), font_table::RANGE_TAKEN);
        assert_eq!(s.entry(&taken, &fonts()), None);
        // No font, or one that is not installed, cannot be kept; the preview says so.
        s.font = String::new();
        assert_eq!(s.preview(&[], &fonts()), font_table::NO_FONT);
        assert_eq!(s.entry(&[], &fonts()), None);
        s.font = "Nonesuch".into();
        assert_eq!(s.entry(&[], &fonts()), None);
        assert_eq!(s.preview(&[], &fonts()), font_table::NO_FONT);
        s.font = "consolas".into();
        assert_eq!(s.entry(&[], &fonts()).map(|e| e.font_name), Some("Consolas".into()), "the installed spelling is kept");
        // A destination: an unreadable one is invalid; a good one makes the entry remap.
        s.has_destination = true;
        s.destination = "zz".into();
        assert_eq!(s.check(&[]), Check::InvalidDestination);
        assert_eq!(s.preview(&[], &fonts()), font_table::INVALID_DESTINATION);
        s.destination = "U+10000".into();
        assert_eq!(s.check(&[]), Check::Valid);
        assert_eq!(s.entry(&[], &fonts()).and_then(|e| e.destination), Some(0x10000));
        s.destination = "U+41".into();
        assert_eq!(s.check(&[]), Check::Ascii, "ASCII is judged by where the program sends the code points");
        // A valid preview is the run's characters.
        (s.start, s.end, s.has_destination) = ("U+3b1".into(), "U+3b3".into(), false);
        assert_eq!(s.preview(&[], &fonts()), "\u{3b1}\u{3b2}\u{3b3}");
    }

    #[test]
    fn the_window_edits_the_table_as_iterm2() {
        let mut w = Window::open(Config::with_entries(Vec::new()), fonts());
        // Add: a sheet opens blank; OK is refused until the fields are valid.
        w.begin_add();
        assert_eq!(w.sheet().map(|s| s.start.as_str()), Some(""));
        assert!(!w.accept_sheet(), "nothing typed");
        assert!(w.sheet().is_some(), "the sheet stays open");
        typed(&mut w, "U+4e00", "U+9fff", "Meiryo");
        assert!(w.accept_sheet());
        assert!(w.sheet().is_none());
        // A second entry sorts before the first, by the code points the program sends.
        w.begin_add();
        typed(&mut w, "U+e000", "U+e0ff", "Symbols Nerd Font Mono");
        assert!(w.accept_sheet());
        w.begin_add();
        typed(&mut w, "U+400", "U+4ff", "Consolas");
        assert!(w.accept_sheet());
        let starts = |w: &Window| w.config().entries.iter().map(|e| e.range().start).collect::<Vec<_>>();
        assert_eq!(starts(&w), vec![0x400, 0x4E00, 0xE000]);
        // An overlapping range is refused, with the entry kept as it was.
        w.begin_add();
        typed(&mut w, "U+4f00", "U+5000", "Consolas");
        assert!(!w.accept_sheet());
        w.cancel_sheet();
        assert_eq!(w.config().entries.len(), 3);
        // Edit: the row's own range is not "taken"; OK replaces the row.
        w.select(Some(1));
        w.begin_edit();
        assert_eq!(w.sheet().map(|s| (s.row, s.start.as_str(), s.font.as_str())), Some((Some(1), "U+4e00", "Meiryo")));
        typed(&mut w, "U+4e00", "U+9fef", "Meiryo");
        assert!(w.accept_sheet());
        assert_eq!(w.config().entries[1].count, 0x9fef - 0x4e00 + 1);
        // Delete the selected row.
        w.select(Some(0));
        w.delete_selected();
        assert_eq!(starts(&w), vec![0x4E00, 0xE000]);
        // With nothing selected, edit and delete do nothing.
        w.select(None);
        w.begin_edit();
        assert!(w.sheet().is_none());
        w.delete_selected();
        assert_eq!(w.config().entries.len(), 2);
        // A preset fills the range; the font is still to choose.
        w.begin_preset(3);
        assert_eq!(w.sheet().map(|s| (s.start.as_str(), s.end.as_str(), s.font.as_str())), Some(("U+600", "U+6ff", "")));
        assert!(!w.accept_sheet(), "no font yet");
        w.sheet_mut().unwrap().font = "Meiryo".into();
        assert!(w.accept_sheet());
        assert_eq!(w.config().entries.len(), 3);
        w.begin_preset(99);
        assert!(w.sheet().is_none(), "a preset that is not one opens nothing");
    }

    #[test]
    fn import_and_export_follow_iterm2() {
        let mut w = Window::open(Config::with_entries(Vec::new()), fonts());
        let good = r#"{"entries":[{"start":57344,"count":6400,"fontName":"Symbols Nerd Font Mono"}],"version":2}"#;
        w.import_text(good);
        assert!(w.message().is_none());
        assert_eq!(w.config().entries.len(), 1);
        assert_eq!(Config::parse(&w.export_text()).as_ref(), Some(w.config()), "export reads back");
        // Malformed, newer, and a font that is not installed: each is refused with iTerm2's text.
        w.import_text("{");
        assert_eq!(w.message(), Some(font_table::IMPORT_MALFORMED));
        w.import_text(r#"{"entries":[{"start":4096,"count":1,"fontName":"A","bogus":true}],"version":3}"#);
        assert_eq!(w.message(), Some(font_table::IMPORT_NEWER), "an unknown field is ignored, the version is not");
        w.import_text(r#"{"entries":[{"start":4096,"count":1,"fontName":"Gone"},{"start":4200,"count":1,"fontName":"Gone2"}],"version":2}"#);
        assert_eq!(w.message(), Some("You must install the following fonts to use the exceptions in this file:\n\nGone\nGone2"));
        assert_eq!(w.config().entries.len(), 1, "a refused file changes nothing");
        // An entry outside the bounds is malformed, never a panic.
        w.import_text(r#"{"entries":[{"start":-5,"count":9223372036854775807,"fontName":"Consolas"}],"version":2}"#);
        assert_eq!(w.message(), Some(font_table::IMPORT_MALFORMED));
        // A good file replaces the table and clears the message.
        w.import_text(r#"{"entries":[{"start":4096,"count":16,"fontName":"consolas"}],"version":2}"#);
        assert!(w.message().is_none());
        assert_eq!(w.config().entries[0].range(), 4096..4112);
    }

    /// 13n final 3: a table imported from iTerm2 (an overlap, an ASCII start) opens whole in the
    /// editor and OK hands every entry back, so it never wipes what it did not show.
    #[test]
    fn an_imported_overlapping_table_opens_whole_and_ok_keeps_it() {
        let text = r#"{"entries":[{"start":57344,"count":16,"fontName":"A"},{"start":57352,"count":16,"fontName":"B"},{"start":65,"count":4,"fontName":"C"}],"version":2}"#;
        let config = Config::parse(text).expect("kept");
        let mut w = Window::open(config, fonts());
        assert_eq!(w.config().entries.len(), 3, "every row is shown");
        // A new row still may not overlap, or start in ASCII.
        w.begin_add();
        let sheet = w.sheet_mut().unwrap();
        sheet.start = "U+e004".into();
        sheet.end = "U+e005".into();
        sheet.font = "Meiryo".into();
        assert!(!w.accept_sheet(), "the editor still refuses an overlap for a new row");
        // Editing a row that still overlaps another is refused and changes nothing.
        w.select(Some(0));
        w.begin_edit();
        w.sheet_mut().unwrap().font = "Meiryo".into();
        assert!(!w.accept_sheet());
        assert_eq!(w.config().entries.len(), 3, "a refused edit never drops an entry");
        // Deleting one row keeps the others as they were, overlapping or not.
        w.select(Some(2));
        w.delete_selected();
        assert_eq!(w.config().entries.len(), 2);
        assert_eq!(Config::parse(&w.config().string()).map(|c| c.entries.len()), Some(2));
    }

    /// 13n final residual: a stored value the editor cannot read opens with a notice, and OK
    /// with no edit keeps it; an explicit edit replaces it.
    #[test]
    fn an_unreadable_stored_value_is_kept_until_edited() {
        let mut w = Window::open_unreadable(fonts());
        assert_eq!(w.notice(), Some(UNREADABLE_NOTICE));
        assert!(matches!(w.finish(), Outcome::Keep), "OK with no edit keeps the stored text");
        // A refused sheet is not an edit.
        w.begin_add();
        assert!(!w.accept_sheet());
        assert!(matches!(w.finish(), Outcome::Keep));
        // An accepted add replaces the stored value.
        typed(&mut w, "U+4e00", "U+9fff", "Meiryo");
        assert!(w.accept_sheet());
        assert!(w.notice().is_none());
        assert!(matches!(w.finish(), Outcome::Ok(c) if c.entries.len() == 1));
        // An import also counts as an edit.
        let mut w = Window::open_unreadable(fonts());
        w.import_text(r#"{"entries":[{"start":57344,"count":16,"fontName":"Consolas"}],"version":2}"#);
        assert!(matches!(w.finish(), Outcome::Ok(c) if c.entries.len() == 1));
        // A readable (or empty) stored table always hands back its table.
        assert!(matches!(Window::open(Config::with_entries(Vec::new()), fonts()).finish(), Outcome::Ok(_)));
    }

    #[test]
    fn the_font_list_is_built_once_and_capped() {
        let many: Vec<String> = (0..10_000).map(|i| format!("Font {}", i % 6000)).collect();
        let w = Window::open(Config::with_entries(Vec::new()), many.into_iter().chain(["font 5".to_string()]).collect());
        assert_eq!(w.fonts().len(), super::MAX_FONTS, "deduplicated ignoring case, then capped");
        let sorted = w.fonts().windows(2).all(|p| p[0].to_lowercase() <= p[1].to_lowercase());
        assert!(sorted);
        // The window never asks the renderer for fonts itself.
        let source = include_str!("special_exceptions_ui.rs");
        let needle = concat!("fa", "ces()");
        assert!(!source.split("#[cfg(test)]").next().unwrap().contains(needle));
    }

    /// The window's frames run with the sheet and the message open: no panic, no id clash, and
    /// Escape cancels only when nothing is open over it.
    #[test]
    fn the_window_draws_every_state() {
        let ctx = egui::Context::default();
        let mut w = Window::open(Config::parse(r#"{"entries":[{"start":57344,"count":6400,"fontName":"Consolas"}],"version":2}"#).unwrap(), fonts());
        let frame = |w: &mut Window, escape: bool| {
            let mut input = egui::RawInput::default();
            if escape {
                input.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
            }
            let mut out = None;
            // Nothing paints these frames: the texture deltas are dropped on purpose.
            ctx.run_ui(input, |ui| out = w.ui(ui)).textures_delta.clear();
            out
        };
        assert!(frame(&mut w, false).is_none());
        w.select(Some(0));
        w.begin_edit();
        assert!(frame(&mut w, false).is_none(), "the sheet draws");
        assert!(frame(&mut w, true).is_none(), "Escape closes the sheet, not the window");
        assert!(w.sheet().is_none());
        w.import_text("{");
        assert!(frame(&mut w, false).is_none(), "the message draws");
        w.begin_preset(0);
        assert!(frame(&mut w, false).is_none());
        w.cancel_sheet();
        w.message = None;
        assert!(matches!(frame(&mut w, true), Some(Outcome::Cancel)), "Escape cancels the window");
    }
}
