//! The Settings window's search field and its results (`iTermPreferencesSearch.m`,
//! `iTermPreferencesSearchEngineResultsWindowController.m`): every setting the window shows is a
//! document named by its label; a query matches the documents holding every word of it (a word
//! is a prefix of one of the document's, as written or cut back to its stem), and the matches are
//! listed by name. Activating one opens its pane and tab and marks the control.

use std::sync::OnceLock;

use crate::schema::{self, Setting};
use crate::settings_ui::Page;

/// The most results the list shows before it scrolls (`MAX_ROWS`).
pub const MAX_ROWS: usize = 20;
/// The field's hint.
pub const HINT: &str = "Search";
/// How long a revealed control stays marked, in seconds (the scrim's fade).
pub const MARK_SECONDS: f64 = 1.6;

/// Where a document's control is.
#[derive(Clone, Debug, PartialEq)]
pub enum Where {
    /// A Profiles tab (an index into `schema::TABS`).
    ProfileTab(usize),
    /// A pane of the window.
    Pane(Page),
    /// The Advanced pane: the query that finds the row there.
    Advanced(String),
}

/// One searchable setting.
#[derive(Clone, Debug)]
pub struct Doc {
    /// What the list shows.
    pub name: String,
    /// The schema key the control edits (the mark follows it).
    pub key: String,
    pub at: Where,
    words: Vec<String>,
    stems: Vec<String>,
}

/// The words of a phrase: runs of letters and digits, lower case (`it_normalizedTokens`).
pub fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// A word cut back to its stem (`it_stem`, in a small way): common endings go, down to three letters.
pub fn stem(word: &str) -> String {
    for ending in ["ing", "ed", "es", "s"] {
        if let Some(root) = word.strip_suffix(ending)
            && root.chars().count() >= 3
        {
            return root.to_string();
        }
    }
    word.to_string()
}

fn doc(name: &str, key: &str, at: Where, more: &[&str]) -> Doc {
    let name = name.trim().trim_end_matches(':').trim().to_string();
    let mut words = tokens(&name);
    for m in more {
        words.extend(tokens(m));
    }
    words.sort();
    words.dedup();
    let stems = words.iter().map(|w| stem(w)).collect();
    Doc { name, key: key.to_string(), at, words, stems }
}

fn pane_of(s: &Setting) -> Option<Page> {
    match s.tab.as_str() {
        "General" => Some(Page::General),
        "Appearance" => Some(Page::Appearance),
        "Keys" => Some(Page::Keys),
        "Pointer" => Some(Page::Pointer),
        _ => None,
    }
}

/// Every searchable setting, built once.
pub fn documents() -> &'static [Doc] {
    static DOCS: OnceLock<Vec<Doc>> = OnceLock::new();
    DOCS.get_or_init(|| {
        let mut docs = Vec::new();
        for s in schema::profile() {
            let Some(tab) = schema::TABS.iter().position(|t| *t == s.tab) else { continue };
            if s.macos_only || s.label.is_empty() || s.key.starts_with("HotKey ") || matches!(s.key.as_str(), "Browser Zoom" | "Touch Bar Map" | "Title Function") {
                continue;
            }
            let (name, caption) = crate::settings_colors::search_name(s);
            docs.push(doc(&name, &s.key, Where::ProfileTab(tab), &[&s.section, caption]));
        }
        for s in schema::global() {
            if s.macos_only || s.label.is_empty() || crate::settings_keys::row_hidden(&s.key) {
                continue;
            }
            if let Some(page) = pane_of(s) {
                if s.key == "OpenNoWindowsAtStartup" {
                    continue;
                }
                docs.push(doc(&s.label, &s.key, Where::Pane(page), &[&s.section]));
            }
        }
        for r in crate::advanced_pane::rows() {
            if r.macos_only {
                continue;
            }
            docs.push(doc(r.label, r.key, Where::Advanced(r.label.to_string()), &[r.description]));
        }
        docs
    })
}

/// The documents matching `query`, by name (`documentsMatchingQuery:`): each word of the query must
/// begin a word of the document, or begin a stem of it. An empty query matches nothing. A setting
/// found twice (a name two panes share) is listed once per control.
pub fn search(query: &str) -> Vec<&'static Doc> {
    let raw = tokens(query);
    if raw.is_empty() {
        return Vec::new();
    }
    let stems: Vec<String> = raw.iter().map(|w| stem(w)).collect();
    let mut found: Vec<&'static Doc> = documents()
        .iter()
        .filter(|d| raw.iter().zip(&stems).all(|(w, st)| d.words.iter().any(|dw| dw.starts_with(w.as_str())) || d.stems.iter().any(|ds| ds.starts_with(st.as_str()))))
        .collect();
    found.sort_by_key(|d| d.name.to_lowercase());
    found
}

/// The control being marked after a reveal: its key and when the mark ends (egui time).
pub fn marked(ctx: &egui::Context) -> Option<(String, f64)> {
    ctx.data(|d| d.get_temp::<(String, f64)>(egui::Id::new("settings-search-mark")))
}

/// Marks `key`'s control for `MARK_SECONDS`.
pub fn mark(ctx: &egui::Context, key: &str) {
    let until = ctx.input(|i| i.time) + MARK_SECONDS;
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("settings-search-mark"), (key.to_string(), until)));
    ctx.request_repaint();
}

/// The row that draws `key`'s control when several keys share one drawing (the font block, the
/// window size line, the title popup, the hotkey recorders, the startup popup): the row the
/// marked control is framed at.
pub fn lead(key: &str) -> &str {
    if crate::settings_fonts::GROUPED.contains(&key) {
        return "Normal Font";
    }
    match key {
        "Rows" | "Width Percentage" | "Height Percentage" => "Columns",
        "Title Function" => "Title Components",
        "Session Hotkey" => "Has Hotkey",
        "OpenNoWindowsAtStartup" => "OpenArrangementAtStartup",
        "PrefsCustomFolder" => "LoadPrefsFromCustomFolder",
        "HotkeyChar" | "HotkeyModifiers" | "HotkeyCode" => "Hotkey",
        "OpenQuicklyHotkeyChar" | "OpenQuicklyHotkeyModifiers" => "OpenQuicklyHotkeyCode",
        k if k.starts_with("HotKey ") => "Has Hotkey",
        k => k,
    }
}

/// Frames the marked control for a loop that draws rows with functions of their own: `row` is
/// called as each row starts, and `finish` after the last; a row's frame is drawn when the next
/// starts (so a `continue` in the loop cannot skip it).
#[derive(Default)]
pub struct RowFlash {
    prev: Option<(String, f32)>,
}

impl RowFlash {
    pub fn row(&mut self, ui: &mut egui::Ui, key: &str) {
        self.finish(ui);
        if marked(ui.ctx()).is_some() {
            self.prev = Some((key.to_string(), ui.cursor().top()));
        }
    }

    pub fn finish(&mut self, ui: &mut egui::Ui) {
        if let Some((key, top)) = self.prev.take() {
            flash_if_marked(ui, &key, top);
        }
    }
}

/// Called after a row `key` drew from `top` down to the cursor: if it is the marked control, frames
/// it and scrolls it into view, and keeps the window drawing until the mark fades.
pub fn flash_if_marked(ui: &mut egui::Ui, key: &str, top: f32) {
    let Some((marked_key, until)) = marked(ui.ctx()) else { return };
    if lead(&marked_key) != lead(key) {
        return;
    }
    // A row drawn with no height (its rows are drawn with another's) has nothing to frame, and a
    // control that framed itself this pass is not framed again by its loop.
    let pass = ui.ctx().cumulative_pass_nr();
    let framed_id = egui::Id::new("settings-search-framed");
    if ui.cursor().top() <= top + 0.5 || ui.ctx().data(|d| d.get_temp::<u64>(framed_id)) == Some(pass) {
        return;
    }
    ui.ctx().data_mut(|d| d.insert_temp(framed_id, pass));
    let now = ui.ctx().input(|i| i.time);
    if now >= until {
        ui.ctx().data_mut(|d| d.remove::<(String, f64)>(egui::Id::new("settings-search-mark")));
        return;
    }
    let area = ui.min_rect();
    let rect = egui::Rect::from_min_max(egui::pos2(area.left(), top), egui::pos2(area.right(), ui.cursor().top().max(top + 4.0)));
    let strength = ((until - now) / MARK_SECONDS).clamp(0.0, 1.0) as f32;
    let color = ui.visuals().selection.bg_fill.gamma_multiply(strength);
    ui.painter().rect_stroke(rect.expand(2.0), 3.0, egui::Stroke::new(2.0, color), egui::StrokeKind::Outside);
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
    ui.ctx().request_repaint();
}

/// The state of the field and its list across frames.
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// The highlighted result (Up and Down move it).
    pub selected: usize,
}

/// Draws the field and, while it has focus with text in it, the results under it. The returned
/// document is the one to reveal (the field is then cleared).
pub fn field(ui: &mut egui::Ui, st: &mut SearchState) -> Option<&'static Doc> {
    let r = crate::settings_keys::field(ui, egui::TextEdit::singleline(&mut st.query).hint_text(HINT).desired_width(180.0));
    if r.changed() {
        st.selected = 0;
    }
    let results = if st.query.trim().is_empty() { Vec::new() } else { search(&st.query) };
    let mut open = None;
    if r.has_focus() && !st.query.trim().is_empty() {
        // Up and Down move through the results, Enter opens one (`doCommandBySelector:`).
        if !results.is_empty() {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                st.selected = (st.selected + 1).min(results.len() - 1);
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                st.selected = st.selected.saturating_sub(1);
            }
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            st.query.clear();
        }
    }
    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if enter {
        open = results.get(st.selected).copied();
    }
    // The list stays up while the field has focus or the pointer is over it.
    let list_id = egui::Id::new("settings-search-results");
    let over_list = ui.ctx().data(|d| d.get_temp::<bool>(list_id)).unwrap_or(false);
    if (r.has_focus() || over_list) && !results.is_empty() && open.is_none() {
        let inner = egui::Area::new(list_id).order(egui::Order::Foreground).fixed_pos(r.rect.left_bottom() + egui::vec2(0.0, 2.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(320.0);
                let row = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                egui::ScrollArea::vertical().max_height(row * MAX_ROWS as f32).show_rows(ui, row, results.len(), |ui, range| {
                    for i in range {
                        let label = ui.selectable_label(i == st.selected, results[i].name.as_str());
                        if label.clicked() {
                            open = Some(results[i]);
                        }
                    }
                });
            });
        });
        let hovering = inner.response.hovered() || inner.response.contains_pointer();
        ui.ctx().data_mut(|d| d.insert_temp(list_id, hovering));
    } else {
        ui.ctx().data_mut(|d| d.insert_temp(list_id, false));
    }
    if open.is_some() {
        st.query.clear();
        st.selected = 0;
        ui.ctx().data_mut(|d| d.insert_temp(list_id, false));
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a loop that draws `drawn` as it goes (a row of its own each) framed the marked `key`.
    fn framed(key: &str, drawn: &[&str]) -> bool {
        let ctx = egui::Context::default();
        mark(&ctx, key);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut framing = RowFlash::default();
            for k in drawn {
                framing.row(ui, k);
                if *k != "Rows" {
                    ui.label(*k);
                }
            }
            framing.finish(ui);
        });
        out.textures_delta.clear();
        ctx.data(|d| d.get_temp::<u64>(egui::Id::new("settings-search-framed"))).is_some()
    }

    #[test]
    fn a_control_drawn_by_a_function_of_its_own_is_framed_too() {
        assert!(framed("Columns", &["Window Type", "Columns", "Rows"]));
        assert!(framed("Rows", &["Window Type", "Columns", "Rows"]), "the size line is framed for its Rows field");
        assert!(framed("Non-ASCII Ligatures", &["Normal Font", "Non Ascii Font"]), "the font block is framed for a key it draws");
        assert!(!framed("Rows", &["Window Type", "Transparency"]), "a key no row draws is framed nowhere");
        assert_eq!((lead("HotKey Code"), lead("Title Function"), lead("Name")), ("Has Hotkey", "Title Components", "Name"));
    }

    fn names(q: &str) -> Vec<&'static str> {
        search(q).into_iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn words_are_lower_case_runs() {
        assert_eq!(tokens("Window restoration policy:"), ["window", "restoration", "policy"]);
        assert_eq!(tokens("  Ctrl-Click (semantic) "), ["ctrl", "click", "semantic"]);
        assert_eq!(stem("restoring"), "restor");
        assert_eq!(stem("tabs"), "tab");
        assert_eq!(stem("is"), "is", "a short word keeps its ending");
    }

    #[test]
    fn a_query_needs_every_word() {
        let hits = names("restoration policy");
        assert!(hits.contains(&"Window restoration policy"), "{hits:?}");
        assert!(names("restoration zzzzz").is_empty());
        assert!(names("   ").is_empty());
    }

    #[test]
    fn a_word_matches_by_prefix_and_by_stem() {
        assert!(names("restor").contains(&"Window restoration policy"));
        assert!(names("tab bar").iter().any(|n| n.contains("tab bar")), "{:?}", names("tab bar"));
        assert!(names("profiles").iter().any(|n| n.to_lowercase().contains("profile")), "stem of profiles finds profile");
    }

    #[test]
    fn results_are_listed_by_name() {
        let hits = names("font");
        let mut sorted = hits.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(hits, sorted);
        assert!(hits.contains(&"Font"), "{hits:?}");
    }

    #[test]
    fn every_document_is_somewhere() {
        for d in documents() {
            match &d.at {
                Where::ProfileTab(t) => assert!(*t < schema::TABS.len()),
                Where::Pane(_) => {}
                Where::Advanced(q) => assert!(!q.is_empty()),
            }
        }
        assert!(documents().len() > 400);
    }
}
