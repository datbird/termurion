//! Settings > FX Profiles: the profile list, an editor for the selected one, and a live preview
//! of what its unsaved rules find on the focused pane.

use crate::fx_profile::{self, BoxKindRule, CursorKind, FxRules, PaintedMark};
use crate::fx_store::{Loaded, Store};
use crate::screen::Screen;
use std::path::PathBuf;
use std::sync::Arc;

/// What the page asks the app to do (file dialogs and the folder open run there).
#[derive(Debug, PartialEq)]
pub enum PageAction {
    Rename { old: String, new: String },
    ShowFolder,
    Import,
    Export(String),
}

#[derive(Default)]
pub struct PageState {
    /// The selected profile's file (names change, files do not).
    selected: Option<PathBuf>,
    form: Option<Form>,
    /// The last thing that went wrong, shown under the buttons.
    note: Option<String>,
}

const MAX_NAME: usize = 64;

/// The editor's copy of one profile: the rules plus text buffers for the list fields.
pub struct Form {
    file: PathBuf,
    /// The profile as it was loaded, to tell an edit from none and to notice a hand edit.
    base: Arc<FxRules>,
    /// The file's own table, so keys this version does not know survive a save.
    keep: toml::Table,
    from: String,
    draft: FxRules,
    name: String,
    programs: String,
    texts: String,
    prompt: String,
    marks: String,
    line_glyphs: String,
    edge_glyphs: String,
    prompt_marks: String,
    enter: String,
    quiet: u64,
    busy: u64,
}

/// Glyphs as the field shows them. The settings font has no box drawing or block glyphs, so
/// those show as `U+2500` codes; anything else shows as itself.
fn glyphs(v: &[char]) -> String {
    let coded = |c: &char| ('\u{2500}'..='\u{259f}').contains(c);
    if !v.iter().any(coded) {
        return v.iter().collect();
    }
    v.iter().map(|c| if coded(c) { format!("U+{:04X}", *c as u32) } else { c.to_string() }).collect::<Vec<_>>().join(" ")
}

/// The glyphs a field says: plain characters, and `U+XXXX` codes (separated by spaces).
fn parse_glyphs(s: &str) -> Vec<char> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        let code = tok.strip_prefix("U+").or_else(|| tok.strip_prefix("u+")).filter(|h| (4..=6).contains(&h.len())).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32);
        match code {
            Some(c) => out.push(c),
            None => out.extend(tok.chars()),
        }
    }
    out
}

fn lines(v: &[String]) -> String {
    v.join("\n")
}

fn mark_text(m: &PaintedMark) -> String {
    match m {
        PaintedMark::Reverse => "reverse".into(),
        PaintedMark::Background(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
    }
}

/// Control characters of the Enter text shown as escapes, so the field stays one readable line.
fn escape(bytes: &[u8]) -> String {
    let mut o = String::new();
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\r' => o.push_str("\\r"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\\' => o.push_str("\\\\"),
            c if c.is_control() => o.push_str(&format!("\\x{:02x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> Result<Vec<u8>, String> {
    let mut o = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('r') => o.push('\r'),
            Some('n') => o.push('\n'),
            Some('t') => o.push('\t'),
            Some('\\') => o.push('\\'),
            Some('x') => {
                let hex: String = it.by_ref().take(2).collect();
                let n = u8::from_str_radix(&hex, 16).map_err(|_| "Enter: \\x needs two hex digits".to_string())?;
                o.push(char::from(n));
            }
            _ => return Err("Enter: use \\r, \\n, \\t, \\\\ or \\xNN".into()),
        }
    }
    Ok(o.into_bytes())
}

impl Form {
    fn from_loaded(l: &Loaded) -> Form {
        let r = &*l.rules;
        Form {
            file: l.file.clone(),
            base: l.rules.clone(),
            keep: l.raw.clone(),
            from: r.name.clone(),
            draft: r.clone(),
            name: r.name.clone(),
            programs: lines(&r.matching.program),
            texts: lines(&r.matching.text),
            prompt: glyphs(&r.matching.prompt),
            marks: r.cursor.marks.iter().map(mark_text).collect::<Vec<_>>().join("\n"),
            line_glyphs: glyphs(&r.boxes.line_glyphs),
            edge_glyphs: glyphs(&r.boxes.edge_glyphs),
            prompt_marks: glyphs(&r.prompt_marks),
            enter: escape(&r.done.enter),
            quiet: r.done.quiet.as_millis() as u64,
            busy: r.done.busy.as_millis() as u64,
        }
    }

    /// The rules the fields say, checked by the same parser a load uses. Err is the reason.
    fn build(&self, others: &[String]) -> Result<FxRules, String> {
        let list = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect::<Vec<_>>();
        let chars = parse_glyphs;
        let mut r = self.draft.clone();
        r.name = self.name.trim().to_string();
        valid_name(&r.name, others)?;
        r.matching.program = list(&self.programs);
        r.matching.text = list(&self.texts);
        r.matching.prompt = chars(&self.prompt);
        r.cursor.marks = list(&self.marks).iter().map(|m| fx_profile::parse_mark(m).ok_or_else(|| format!("cursor mark `{m}` must be reverse or a color like #2c2c2c"))).collect::<Result<_, _>>()?;
        r.boxes.line_glyphs = chars(&self.line_glyphs);
        r.boxes.edge_glyphs = chars(&self.edge_glyphs);
        r.prompt_marks = chars(&self.prompt_marks);
        r.done.enter = unescape(&self.enter)?;
        r.done.quiet = std::time::Duration::from_millis(self.quiet);
        r.done.busy = std::time::Duration::from_millis(self.busy);
        // The load's own checks (list sizes, ranges) are the rulebook.
        fx_profile::parse(&fx_profile::to_toml(&r, &self.keep)).map(|(p, _)| p).map_err(|e| e.message)
    }
}

/// A profile name: 1 to 64 characters, no control characters, not one another profile has.
fn valid_name(name: &str, others: &[String]) -> Result<(), String> {
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME {
        return Err(format!("A name is 1 to {MAX_NAME} characters."));
    }
    if name.chars().any(char::is_control) {
        return Err("A name has no control characters.".into());
    }
    if others.iter().any(|o| o == name) {
        return Err(format!("A profile named {name} exists."));
    }
    Ok(())
}

/// `base`, or `base 2`, `base 3` ... the first one not taken.
fn unique(taken: &[String], base: &str) -> String {
    let base: String = base.chars().take(MAX_NAME - 4).collect();
    let mut n = 1;
    loop {
        let name = if n == 1 { base.clone() } else { format!("{base} {n}") };
        if !taken.contains(&name) {
            return name;
        }
        n += 1;
    }
}

fn names(store: &Store) -> Vec<String> {
    store.profiles.iter().map(|p| p.rules.name.clone()).collect()
}

/// The Shell values under a fresh name.
fn new_rules(store: &Store) -> FxRules {
    FxRules { name: unique(&names(store), "New Profile"), ..(*store.shell()).clone() }
}

fn duplicate_rules(store: &Store, name: &str) -> Option<FxRules> {
    let from = store.by_name(name)?;
    Some(FxRules { name: unique(&names(store), &format!("{name} copy")), ..(*from).clone() })
}

fn can_reset(store: &Store, name: &str) -> bool {
    store.profiles.iter().any(|p| p.rules.name == name && p.shipped.is_some())
}

/// Writes the form's profile back to its file. Some((old, new)) when it was renamed.
fn save_form(store: &mut Store, form: &mut Form) -> Result<Option<(String, String)>, String> {
    let others: Vec<String> = store.profiles.iter().filter(|p| p.file != form.file).map(|p| p.rules.name.clone()).collect();
    let rules = form.build(&others)?;
    store.save(&rules, &form.keep, Some(&form.file)).map_err(|e| e.to_string())?;
    // The form now stands on what the store reloaded, so it is clean and sees later edits.
    if let Some(l) = store.profiles.iter().find(|p| p.file == form.file) {
        form.base = l.rules.clone();
        form.keep = l.raw.clone();
    }
    let old = std::mem::replace(&mut form.from, rules.name.clone());
    Ok((old != rules.name).then_some((old, rules.name)))
}

/// The lines under the form: what each rule finds on this screen.
fn preview(screen: &Screen, rules: &FxRules) -> Vec<String> {
    use crate::{fx_detect, scene};
    use alacritty_terminal::grid::Dimensions;
    let (rows, cols) = (screen.term().screen_lines(), screen.term().columns());
    let region = scene::output_watch(screen, rules).unwrap_or((0, rows.saturating_sub(1), 0, cols.saturating_sub(1)));
    let m = match fx_detect::score(rules, screen, region, None).0 {
        0 => "no match".to_string(),
        3 => {
            // The strength says a text hit; trying them one at a time says which.
            let hit = rules.matching.text.iter().find(|t| {
                let one = FxRules { matching: fx_profile::Match { text: vec![(*t).clone()], ..Default::default() }, ..rules.clone() };
                fx_detect::score(&one, screen, region, None).0 == 3
            });
            hit.map_or_else(|| "text found".to_string(), |t| format!("text `{t}` found"))
        }
        2 => "prompt found".to_string(),
        _ => "box shape found".to_string(),
    };
    let found = scene::fx_found(screen, rules);
    let real = scene::fx_found(screen, &FxRules { cursor: fx_profile::CursorRule { kind: CursorKind::Real, ..rules.cursor.clone() }, ..rules.clone() }).cursor.is_some();
    let cursor = found.cursor.map_or_else(|| "not found".to_string(), |(r, c, _)| format!("row {}, column {} ({})", r + 1, c + 1, if real { "real" } else { "painted" }));
    let input_box = found.input_box.map_or_else(
        || "not found".to_string(),
        |(k, top, bottom)| format!("{}, rows {} to {}", if k == scene::BoxKind::Band { "band" } else { "lines" }, top + 1, bottom + 1),
    );
    let mark = found.prompt_mark.map_or_else(|| "not found".to_string(), |(r, c, g)| format!("`{g}` at row {}, column {}", r + 1, c + 1));
    vec![format!("Match: {m}"), format!("Cursor: {cursor}"), format!("Input box: {input_box}"), format!("Prompt mark: {mark}")]
}

fn confirm(title: &str, text: &str) -> bool {
    crate::platform::dialog::message(None, title, text, crate::platform::dialog::Kind::ConfirmOkCancel) == crate::platform::dialog::Answer::Ok
}

fn mark_of(l: &Loaded) -> &'static str {
    if l.newer_shipped {
        "newer version available"
    } else if l.changed {
        "changed"
    } else if l.shipped.is_some() {
        "shipped"
    } else {
        ""
    }
}

pub fn page(ui: &mut egui::Ui, store: &mut Store, state: &mut PageState, screen: Option<&Screen>) -> Vec<PageAction> {
    let mut actions = Vec::new();
    // The folder is polled by the app each frame; keep frames coming while the page is open.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    // A hand edit or a reset shows in the form while it holds no edits of its own.
    if let Some(sel) = state.selected.clone() {
        match store.profiles.iter().find(|p| p.file == sel) {
            None => {
                state.selected = None;
                state.form = None;
            }
            Some(l) => {
                let stale = state.form.as_ref().is_none_or(|f| f.file != sel || (!Arc::ptr_eq(&f.base, &l.rules) && f.build(&[]).ok().as_ref() == Some(&*f.base)));
                if stale {
                    state.form = Some(Form::from_loaded(l));
                }
            }
        }
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(250.0);
                list(ui, store, state, &mut actions);
            });
            ui.separator();
            ui.vertical(|ui| editor(ui, store, state, screen, &mut actions));
        });
    });
    actions
}

fn list(ui: &mut egui::Ui, store: &mut Store, state: &mut PageState, actions: &mut Vec<PageAction>) {
    ui.strong("Profiles");
    let mut pick = None;
    for l in &store.profiles {
        let mark = mark_of(l);
        let text = if mark.is_empty() { l.rules.name.clone() } else { format!("{}  ({mark})", l.rules.name) };
        if ui.selectable_label(state.selected.as_ref() == Some(&l.file), text).clicked() {
            pick = Some(l.file.clone());
        }
    }
    for (path, e) in &store.errors {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let at = e.line.map_or(String::new(), |n| format!(" line {n}"));
        ui.colored_label(ui.visuals().error_fg_color, format!("error: {file}{at}: {}", e.message));
    }
    if pick.is_some() {
        state.selected = pick;
        state.form = None;
        state.note = None;
    }
    let name = state.selected.as_ref().and_then(|f| store.profiles.iter().find(|p| &p.file == f)).map(|p| p.rules.name.clone());
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("New").on_hover_text("A new profile with the Shell values.").clicked() {
            let r = new_rules(store);
            match store.save(&r, &toml::Table::new(), None) {
                Ok(file) => {
                    state.selected = Some(file);
                    state.form = None;
                    state.note = None;
                }
                Err(e) => state.note = Some(e.to_string()),
            }
        }
        if ui.add_enabled(name.is_some(), egui::Button::new("Duplicate")).on_hover_text("Copy the selected profile.").clicked() {
            if let Some(r) = name.as_deref().and_then(|n| duplicate_rules(store, n)) {
                let keep = state.selected.as_ref().and_then(|f| store.profiles.iter().find(|p| &p.file == f)).map(|p| p.raw.clone()).unwrap_or_default();
                match store.save(&r, &keep, None) {
                    Ok(file) => {
                        state.selected = Some(file);
                        state.form = None;
                        state.note = None;
                    }
                    Err(e) => state.note = Some(e.to_string()),
                }
            }
        }
        if ui.add_enabled(name.as_deref().is_some_and(|n| n != "Shell"), egui::Button::new("Delete")).on_hover_text("Remove the selected profile's file. The Shell profile stays.").clicked() {
            if let Some(n) = &name {
                if confirm("Delete Profile?", &format!("Delete the FX profile {n}?")) {
                    match store.delete(n) {
                        Ok(()) => {
                            state.selected = None;
                            state.form = None;
                            state.note = None;
                        }
                        Err(e) => state.note = Some(e.to_string()),
                    }
                }
            }
        }
        if ui.button("Import").on_hover_text("Copy a profile file into the folder.").clicked() {
            actions.push(PageAction::Import);
        }
        if ui.add_enabled(name.is_some(), egui::Button::new("Export")).on_hover_text("Save the selected profile to a file.").clicked() {
            actions.extend(name.clone().map(PageAction::Export));
        }
        let shipped = name.as_deref().is_some_and(|n| can_reset(store, n));
        if ui.add_enabled(shipped, egui::Button::new("Reset")).on_hover_text("Put a shipped profile back to the text Termurion ships.").clicked() {
            if let Some(n) = &name {
                if confirm("Reset Profile?", &format!("Put {n} back to its shipped rules? Your edits to it are lost.")) {
                    state.note = store.reset(n).err().map(|e| e.to_string());
                    state.form = None;
                }
            }
        }
        if ui.button("Restore Shipped Profiles").on_hover_text("Write back any shipped profile file that is gone.").clicked() {
            state.note = store.restore_shipped().err().map(|e| e.to_string());
        }
        if ui.button("Show Folder").on_hover_text("Open the profiles folder.").clicked() {
            actions.push(PageAction::ShowFolder);
        }
    });
    if let Some(n) = &state.note {
        ui.colored_label(ui.visuals().error_fg_color, n);
    }
}

/// A labeled row whose label and control both carry the hover text.
fn row(ui: &mut egui::Ui, label: &str, hover: &str, add: impl FnOnce(&mut egui::Ui) -> egui::Response) {
    ui.label(label).on_hover_text(hover);
    add(ui).on_hover_text(hover);
    ui.end_row();
}

fn text_row(ui: &mut egui::Ui, label: &str, hover: &str, text: &mut String, multi: bool) {
    row(ui, label, hover, |ui| if multi { ui.add(egui::TextEdit::multiline(text).desired_rows(2).desired_width(280.0)) } else { ui.add(egui::TextEdit::singleline(text).desired_width(280.0)) });
}

fn box_kind_name(k: BoxKindRule) -> &'static str {
    match k {
        BoxKindRule::Lines => "lines",
        BoxKindRule::Band => "band",
        BoxKindRule::Either => "either",
        BoxKindRule::None => "none",
    }
}

fn editor(ui: &mut egui::Ui, store: &mut Store, state: &mut PageState, screen: Option<&Screen>, actions: &mut Vec<PageAction>) {
    let Some(mut owned) = state.form.take() else {
        ui.label("Pick a profile to edit it, or press New.");
        return;
    };
    let form = &mut owned;
    let d = FxRules::default();
    let others: Vec<String> = store.profiles.iter().filter(|p| p.file != form.file).map(|p| p.rules.name.clone()).collect();
    ui.strong("Match");
    egui::Grid::new("fx_match").num_columns(2).min_col_width(170.0).show(ui, |ui| {
        if form.from == "Shell" {
            row(ui, "Name", "The Shell profile is the fallback for everything, so its name stays.", |ui| ui.add_enabled(false, egui::TextEdit::singleline(&mut form.name).desired_width(280.0)));
        } else {
            text_row(ui, "Name", "The name the menu and the list show: 1 to 64 characters, one of a kind. Shell default: Shell.", &mut form.name, false);
        }
        text_row(ui, "Programs", "Program names (one per line) that make Auto pick this profile when one runs in the pane. Shell default: none.", &mut form.programs, true);
        text_row(ui, "Texts", "Texts (one per line) that make Auto pick this profile when they show on the screen. Shell default: none.", &mut form.texts, true);
        text_row(ui, "Prompt glyphs", "Prompt glyphs that make Auto pick this profile when one starts the cursor's row. Shell default: none.", &mut form.prompt, false);
        row(ui, "Box shape", "A box shape on screen that makes Auto pick this profile. Shell default: not used.", |ui| {
            egui::ComboBox::from_id_salt("fx_shape")
                .selected_text(form.draft.matching.shape.map_or("not used", box_kind_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.draft.matching.shape, None, "not used");
                    for k in [BoxKindRule::Lines, BoxKindRule::Band, BoxKindRule::Either, BoxKindRule::None] {
                        ui.selectable_value(&mut form.draft.matching.shape, Some(k), box_kind_name(k));
                    }
                })
                .response
        });
    });
    ui.strong("Cursor");
    egui::Grid::new("fx_cursor").num_columns(2).min_col_width(170.0).show(ui, |ui| {
        row(ui, "Kind", "Which cursor the effects follow: the real one, one the app paints, or either. Shell default: either.", |ui| {
            egui::ComboBox::from_id_salt("fx_ckind")
                .selected_text(match form.draft.cursor.kind {
                    CursorKind::Real => "real",
                    CursorKind::Painted => "painted",
                    CursorKind::Either => "either",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.draft.cursor.kind, CursorKind::Real, "real");
                    ui.selectable_value(&mut form.draft.cursor.kind, CursorKind::Painted, "painted");
                    ui.selectable_value(&mut form.draft.cursor.kind, CursorKind::Either, "either");
                })
                .response
        });
        text_row(ui, "Painted marks", "How an app paints its cursor, one per line: reverse, or a background color like #2c2c2c. Shell default: reverse.", &mut form.marks, true);
        row(ui, "Echo anchor", "Keep the effect on the typed text while it echoes. Shell default: on.", |ui| ui.checkbox(&mut form.draft.cursor.echo_anchor, ""));
    });
    ui.strong("Input Box");
    egui::Grid::new("fx_box").num_columns(2).min_col_width(170.0).show(ui, |ui| {
        row(ui, "Kind", "The box around the input: lines above and below, a shaded band, either, or none. Shell default: either.", |ui| {
            egui::ComboBox::from_id_salt("fx_bkind")
                .selected_text(box_kind_name(form.draft.boxes.kind))
                .show_ui(ui, |ui| {
                    for k in [BoxKindRule::Lines, BoxKindRule::Band, BoxKindRule::Either, BoxKindRule::None] {
                        ui.selectable_value(&mut form.draft.boxes.kind, k, box_kind_name(k));
                    }
                })
                .response
        });
        text_row(ui, "Line glyphs", "Characters a box line is drawn with; write box glyphs as codes like U+2500. Shell default: U+2500 U+2501.", &mut form.line_glyphs, false);
        row(ui, "Line fill", &format!("How much of a row a box line must cover, 0.1 to 1. Shell default: {}.", d.boxes.line_fill), |ui| ui.add(egui::DragValue::new(&mut form.draft.boxes.line_fill).range(0.1..=1.0).speed(0.01)));
        row(ui, "Bottom within", &format!("How many rows below the cursor the bottom line may be, 0 to 50. Shell default: {}.", d.boxes.bottom_within), |ui| ui.add(egui::DragValue::new(&mut form.draft.boxes.bottom_within).range(0..=50)));
        row(ui, "Band rows", &format!("The fewest and most rows a band has, each 1 to 50. Shell default: {} to {}.", d.boxes.band_rows.0, d.boxes.band_rows.1), |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut form.draft.boxes.band_rows.0).range(1..=50));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut form.draft.boxes.band_rows.1).range(1..=50)).on_hover_text("The most rows a band has.")
            })
            .inner
        });
        row(ui, "Band max screen", &format!("The most of the screen's height a band may fill, 0 to 1. Shell default: {}.", d.boxes.band_max_screen), |ui| ui.add(egui::DragValue::new(&mut form.draft.boxes.band_max_screen).range(0.1..=1.0).speed(0.01)));
        row(ui, "Band min width", &format!("The least of the width a band must cover, 0 to 1. Shell default: {}.", d.boxes.band_min_width), |ui| ui.add(egui::DragValue::new(&mut form.draft.boxes.band_min_width).range(0.1..=1.0).speed(0.01)));
        row(ui, "Band tolerance", &format!("How many rows may differ in color inside a band, 0 to 50. Shell default: {}.", d.boxes.band_tolerance), |ui| ui.add(egui::DragValue::new(&mut form.draft.boxes.band_tolerance).range(0..=20)));
        text_row(ui, "Edge glyphs", "Half block characters that end a band as an edge row; write them as codes like U+2580. Shell default: U+2580.", &mut form.edge_glyphs, false);
    });
    ui.strong("Prompt Mark");
    egui::Grid::new("fx_mark").num_columns(2).min_col_width(170.0).show(ui, |ui| {
        text_row(ui, "Glyphs", "Characters that count as the prompt mark at the start of the input. Shell default: \u{276f}>\u{203a}.", &mut form.prompt_marks, false);
    });
    ui.strong("Done");
    egui::Grid::new("fx_done").num_columns(2).min_col_width(170.0).show(ui, |ui| {
        text_row(ui, "Enter", "What the Enter key sends, with \\r, \\n, \\t, \\\\ and \\xNN. Shell default: \\r.", &mut form.enter, false);
        row(ui, "Quiet (ms)", &format!("How long the output must stay still before the answer counts as done, 100 to 60000. Shell default: {}.", d.done.quiet.as_millis()), |ui| ui.add(egui::DragValue::new(&mut form.quiet).range(100..=60_000)));
        row(ui, "Busy (ms)", &format!("How long output after Enter must last before the program counts as busy, 0 to 60000. Shell default: {}.", d.done.busy.as_millis()), |ui| ui.add(egui::DragValue::new(&mut form.busy).range(0..=60_000)));
        row(ui, "Shell marks (OSC 133)", "Believe the shell's own prompt and command marks. Shell default: on.", |ui| ui.checkbox(&mut form.draft.done.osc133, ""));
    });
    let built = form.build(&others);
    let dirty = built.as_ref().map_or(true, |r| *r != *form.base);
    ui.add_space(6.0);
    let mut revert = false;
    ui.horizontal(|ui| {
        if ui.add_enabled(built.is_ok() && dirty, egui::Button::new("Save")).on_hover_text("Write the profile's file; the folder watcher reloads it.").clicked() {
            match save_form(store, form) {
                Ok(renamed) => {
                    state.note = None;
                    if let Some((old, new)) = renamed {
                        actions.push(PageAction::Rename { old, new });
                    }
                }
                Err(e) => state.note = Some(e),
            }
        }
        revert = ui.add_enabled(dirty, egui::Button::new("Revert")).on_hover_text("Throw away the unsaved edits.").clicked();
    });
    match &built {
        Err(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        Ok(rules) => {
            ui.add_space(6.0);
            ui.strong("Preview");
            match screen {
                Some(s) => {
                    for l in preview(s, rules) {
                        ui.label(l);
                    }
                }
                None => {
                    ui.label("No pane to read.");
                }
            }
        }
    }
    if !revert {
        state.form = Some(owned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx_profile::{BoxKindRule, SHIPPED};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fx_page_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn agent(name: &str) -> Screen {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agents/");
        let text = std::fs::read_to_string(format!("{dir}{name}.ansi")).unwrap();
        let cursors = std::fs::read_to_string(format!("{dir}cursors.txt")).unwrap();
        let line = cursors.lines().find(|l| l.split(' ').next() == Some(name)).unwrap();
        let num = |key: &str| -> usize { line.split(' ').find_map(|w| w.strip_prefix(key)).unwrap().parse().unwrap() };
        let rows: Vec<String> = text.trim_end_matches('\n').split('\n').map(|r| format!("\x1b[0m{r}")).collect();
        let mut s = Screen::new(100, 30);
        s.feed(format!("\x1b[2J\x1b[H{}\x1b[{};{}H", rows.join("\r\n"), num("y=") + 1, num("x=") + 1).as_bytes());
        s
    }

    fn codex() -> FxRules {
        let text = SHIPPED.iter().find(|(f, _)| *f == "codex.toml").unwrap().1;
        crate::fx_profile::parse(text).unwrap().0
    }

    #[test]
    fn new_profile_names_are_unique() {
        let mut store = Store::open(&tmp("new"));
        let first = new_rules(&store);
        assert_eq!(first.name, "New Profile");
        assert_eq!(first.boxes, store.shell().boxes, "starts from the Shell values");
        store.save(&first, &toml::Table::new(), None).unwrap();
        let second = new_rules(&store);
        assert_eq!(second.name, "New Profile 2");
    }

    #[test]
    fn duplicate_copies_every_rule() {
        let store = Store::open(&tmp("dup"));
        let copy = duplicate_rules(&store, "Codex").expect("codex ships");
        assert_eq!(copy.name, "Codex copy");
        let mut same = copy.clone();
        same.name = "Codex".into();
        assert_eq!(same, *store.by_name("Codex").unwrap());
    }

    #[test]
    fn reset_is_only_for_shipped() {
        let mut store = Store::open(&tmp("reset"));
        let mine = FxRules { name: "Mine".into(), ..FxRules::default() };
        store.save(&mine, &toml::Table::new(), None).unwrap();
        assert!(can_reset(&store, "Codex"));
        assert!(!can_reset(&store, "Mine"));
        assert!(!can_reset(&store, "Nothing"));
    }

    #[test]
    fn the_preview_reports_each_rule() {
        let s = agent("codex");
        let p = preview(&s, &codex());
        assert_eq!(p[0], "Match: text `OpenAI Codex` found");
        assert!(p[1].starts_with("Cursor: row 27, column 8 (real)"), "{p:?}");
        assert_eq!(p[2], "Input box: band, rows 26 to 27");
        assert_eq!(p[3], "Prompt mark: `\u{203a}` at row 27, column 1");
        let mut lines = codex();
        lines.boxes.kind = BoxKindRule::Lines;
        assert_eq!(preview(&s, &lines)[2], "Input box: not found");
        let mut other = codex();
        other.matching.text = vec!["nothing like it".into()];
        other.matching.prompt.clear();
        assert_eq!(preview(&s, &other)[0], "Match: no match");
    }

    #[test]
    fn a_saved_form_is_clean_and_glyph_codes_parse() {
        let mut store = Store::open(&tmp("clean"));
        let l = store.profiles.iter().find(|p| p.rules.name == "Codex").unwrap();
        let mut form = Form::from_loaded(l);
        form.name = "Codex 2".into();
        form.line_glyphs = "U+2550 -".into();
        save_form(&mut store, &mut form).unwrap();
        assert_eq!(form.build(&[]).unwrap(), *form.base, "clean after the save");
        let l = store.profiles.iter().find(|p| p.file == form.file).unwrap();
        assert!(Arc::ptr_eq(&form.base, &l.rules));
        assert_eq!(parse_glyphs("U+2500 u+2501 \u{203a}>"), vec!['\u{2500}', '\u{2501}', '\u{203a}', '>']);
        assert_eq!(glyphs(&['\u{2500}', '>']), "U+2500 >");
        assert_eq!(parse_glyphs(&glyphs(&['\u{2580}'])), vec!['\u{2580}']);
    }

    #[test]
    fn saving_keeps_unknown_keys() {
        let dir = tmp("keep");
        let mut store = Store::open(&dir);
        let text = "name = \"Mine\"\nfuture = 7\n\n[match]\ntext = [\"hello\"]\nnote = \"x\"\n";
        std::fs::write(dir.join("mine.toml"), text).unwrap();
        store.refresh(std::time::Instant::now() + std::time::Duration::from_secs(5));
        let loaded = store.profiles.iter().find(|p| p.rules.name == "Mine").expect("loaded");
        let mut form = Form::from_loaded(loaded);
        form.name = "Mine 2".into();
        let renamed = save_form(&mut store, &mut form).unwrap();
        assert_eq!(renamed, Some(("Mine".into(), "Mine 2".into())));
        let back = std::fs::read_to_string(dir.join("mine.toml")).unwrap();
        assert!(back.contains("future = 7") && back.contains("note = \"x\"") && back.contains("Mine 2"), "{back}");
    }
}
