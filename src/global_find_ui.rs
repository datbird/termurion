//! iTerm2's Find Globally: one query over every session, results grouped by session.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::find::{Match, Mode, Pos};

/// The stored mode (`iTermUserDefaults.globalSearchMode`).
pub const MODE_KEY: &str = "NoSyncGlobalSearchMode";

/// The match-mode popup's items in order (`iTermGlobalSearch.xib:151-157`); a separator stands
/// after the first and after the third.
pub const MODE_ITEMS: [(Mode, &str); 5] = [
    (Mode::SmartCase, "Substring \u{2014} Automatic"),
    (Mode::CaseSensitive, "Substring \u{2014} Case Sensitive"),
    (Mode::CaseInsensitive, "Substring \u{2014} Case Insensitive"),
    (Mode::RegexSensitive, "Regex \u{2014} Case Sensitive"),
    (Mode::RegexInsensitive, "Regex \u{2014} Case Insensitive"),
];

/// The popup's text for a mode.
pub fn mode_title(mode: Mode) -> &'static str {
    MODE_ITEMS.iter().find(|(m, _)| *m == mode).map_or(MODE_ITEMS[0].1, |(_, t)| t)
}

/// One match in one session: where it is, and a snippet of its line with the match marked.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub pane: u64,
    pub title: String,
    pub m: Match,
    pub snippet: String,
    /// The match's byte range in `snippet`.
    pub mark: std::ops::Range<usize>,
    /// A browser pane's match: the page's identifier, handed back to reveal it. `m` is a placeholder.
    pub web: Option<serde_json::Value>,
}

/// A pane that closed or whose page was replaced: its delivered rows go (both lists).
pub fn drop_pane_rows(hits: &mut Vec<Hit>, web: &mut Vec<Hit>, pane: u64) {
    hits.retain(|h| h.pane != pane);
    web.retain(|h| h.pane != pane);
}

/// Longest snippet shown, in characters on each side of the match.
const CONTEXT: usize = 40;

/// A snippet of `line` around the byte range `r`, with the range in the snippet.
pub fn snippet(line: &str, r: std::ops::Range<usize>) -> (String, std::ops::Range<usize>) {
    let start = line[..r.start].char_indices().rev().nth(CONTEXT - 1).map_or(0, |(i, _)| i);
    let end = line[r.end..].char_indices().nth(CONTEXT).map_or(line.len(), |(i, _)| r.end + i);
    (line[start..end].to_string(), r.start - start..r.end - start)
}

/// Most results listed; past this the list says how many it left out.
pub const MAX_HITS: usize = 1000;

/// Find Globally searches as you type from three characters; a shorter query waits for Return.
pub fn should_search(query: &str, enter: bool) -> bool {
    let n = query.chars().count();
    n >= 3 || (enter && n > 0)
}

/// Hits grouped by session, in the order the sessions first appear.
pub fn group(hits: Vec<Hit>) -> Vec<(u64, String, Vec<Hit>)> {
    let mut out: Vec<(u64, String, Vec<Hit>)> = Vec::new();
    for h in hits {
        match out.iter_mut().find(|(p, _, _)| *p == h.pane) {
            Some((_, _, list)) => list.push(h),
            None => out.push((h.pane, h.title.clone(), vec![h])),
        }
    }
    out
}

/// The row handler: a browser hit reveals through its identifier, a terminal hit through its match.
pub fn pick_of(h: &Hit) -> GlobalEvent {
    match &h.web {
        Some(identifier) => GlobalEvent::PickWeb(h.pane, identifier.clone()),
        None => GlobalEvent::Pick(h.pane, h.m),
    }
}

/// Longest page string kept in a row, in characters (the model's own bound, `browser_find::MAX_TEXT`).
const SHOWN: usize = crate::browser_find::MAX_TEXT;

/// A browser pane's result as a row (`GSEC:457-470`): the snippet from the match's context
/// (`browser_find::snippet`), every page string made inert first (`inert_files::display_text`:
/// the egui window shows page text). The identifier goes back to the page unchanged.
pub fn web_hit(pane: u64, title: &str, r: &crate::browser_find::FindResult) -> Hit {
    let inert = |s: &Option<String>| s.as_deref().map(|s| crate::inert_files::display_text(s, SHOWN, true));
    let safe = crate::browser_find::FindResult { index: r.index, identifier: r.identifier.clone(), text: inert(&r.text), before: inert(&r.before), after: inert(&r.after) };
    let (snippet, mark) = crate::browser_find::snippet(&safe);
    Hit { pane, title: crate::inert_files::display_text(title, 200, false), m: Match { start: Pos::default(), end: Pos::default() }, snippet, mark, web: Some(r.identifier.clone()) }
}

/// Terminal hits then browser hits grouped by session, at most `MAX_HITS`; the text says so when
/// the list was cut.
pub fn merge(term: &[Hit], web: &[Hit]) -> (Vec<(u64, String, Vec<Hit>)>, Option<String>) {
    let cut = term.len() + web.len() > MAX_HITS;
    let all: Vec<Hit> = term.iter().chain(web).take(MAX_HITS).cloned().collect();
    (group(all), cut.then(|| format!("Showing the first {MAX_HITS} matches.")))
}

pub enum GlobalEvent {
    None,
    /// The query or the mode changed.
    Search(String),
    /// The popup picked another match mode: the search runs again with it.
    Mode(Mode),
    /// A result was clicked: its session and match.
    Pick(u64, Match),
    /// A browser pane's result was clicked: its pane and the page's match identifier.
    PickWeb(u64, serde_json::Value),
    Close,
}

pub struct GlobalFindWindow {
    host: EguiHost,
    query: String,
    groups: Vec<(u64, String, Vec<Hit>)>,
    error: Option<String>,
    first: bool,
    mode: Mode,
}

impl GlobalFindWindow {
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        query: &str,
        mode: Mode,
    ) -> Result<GlobalFindWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Find Globally", 560.0, 420.0)?;
        Ok(GlobalFindWindow { host, query: query.into(), groups: Vec::new(), error: None, first: true, mode })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// The text in the search field.
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    pub fn set_results(&mut self, groups: Vec<(u64, String, Vec<Hit>)>, error: Option<String>) {
        self.groups = groups;
        self.error = error;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> GlobalEvent {
        let mut out = GlobalEvent::None;
        let (query, groups, error, first, mode) = (&mut self.query, &self.groups, &self.error, &mut self.first, &mut self.mode);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let before = *mode;
                egui::ComboBox::from_id_salt("global-find-mode").selected_text(mode_title(*mode)).width(213.0).show_ui(ui, |ui| {
                    for (i, (m, title)) in MODE_ITEMS.iter().enumerate() {
                        ui.selectable_value(mode, *m, *title);
                        if i == 0 || i == 2 {
                            ui.separator();
                        }
                    }
                });
                if *mode != before {
                    out = GlobalEvent::Mode(*mode);
                }
                let r = ui.add(egui::TextEdit::singleline(query).hint_text("Search all sessions").desired_width(f32::INFINITY));
                if *first {
                    r.request_focus();
                    *first = false;
                    if !query.is_empty() {
                        out = GlobalEvent::Search(query.clone());
                    }
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (r.changed() || enter) && should_search(query, enter) {
                    out = GlobalEvent::Search(query.clone());
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = GlobalEvent::Close;
                }
                if let Some(e) = error {
                    ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x6b), e);
                }
                let total: usize = groups.iter().map(|(_, _, h)| h.len()).sum();
                ui.label(format!("{total} matches in {} sessions", groups.len()));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (_, title, hits) in groups {
                        egui::CollapsingHeader::new(format!("{title} ({})", hits.len())).default_open(true).show(ui, |ui| {
                            for h in hits {
                                let mut job = egui::text::LayoutJob::default();
                                let plain = egui::TextFormat::default();
                                let marked = egui::TextFormat { background: egui::Color32::from_rgb(0xff, 0xff, 0x00), color: egui::Color32::BLACK, ..Default::default() };
                                job.append(&h.snippet[..h.mark.start], 0.0, plain.clone());
                                job.append(&h.snippet[h.mark.clone()], 0.0, marked);
                                job.append(&h.snippet[h.mark.end..], 0.0, plain);
                                if ui.add(egui::Button::new(job).frame(false)).clicked() {
                                    out = pick_of(h);
                                }
                            }
                        });
                    }
                });
            });
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Pos;

    fn hit(pane: u64, row: usize) -> Hit {
        let m = Match { start: Pos { row, col: 0 }, end: Pos { row, col: 1 } };
        Hit { pane, title: format!("s{pane}"), m, snippet: "ab".into(), mark: 0..2, web: None }
    }

    #[test]
    fn global_results_group_by_session() {
        let g = group(vec![hit(2, 0), hit(1, 0), hit(2, 5), hit(1, 3)]);
        let shape: Vec<(u64, String, Vec<usize>)> = g.into_iter().map(|(p, t, h)| (p, t, h.iter().map(|h| h.m.start.row).collect())).collect();
        assert_eq!(shape, vec![(2, "s2".into(), vec![0, 5]), (1, "s1".into(), vec![0, 3])]);
    }

    #[test]
    fn snippets_are_cut_around_the_match() {
        let line = format!("{}needle{}", "x".repeat(100), "y".repeat(100));
        let (s, r) = snippet(&line, 100..106);
        assert_eq!(&s[r.clone()], "needle");
        assert_eq!(s.chars().count(), 40 + 6 + 40);
        let (s, r) = snippet("a needle b", 2..8);
        assert_eq!((s.as_str(), r), ("a needle b", 2..8));
        let (s, r) = snippet("\u{4e2d}\u{6587}needle", 6..12);
        assert_eq!(&s[r], "needle");
    }

    #[test]
    fn a_gone_pane_loses_its_rows() {
        let hit = |pane: u64| Hit { pane, title: String::new(), m: Match { start: Pos::default(), end: Pos::default() }, snippet: String::new(), mark: 0..0, web: None };
        let (mut hits, mut web) = (vec![hit(1), hit(2)], vec![hit(2), hit(3)]);
        drop_pane_rows(&mut hits, &mut web, 2);
        assert_eq!(hits.iter().map(|h| h.pane).collect::<Vec<_>>(), vec![1]);
        assert_eq!(web.iter().map(|h| h.pane).collect::<Vec<_>>(), vec![3]);
    }

    fn result(before: &str, text: &str, after: &str) -> crate::browser_find::FindResult {
        crate::browser_find::FindResult { index: 3, identifier: serde_json::json!({"index": 3}), text: Some(text.into()), before: Some(before.into()), after: Some(after.into()) }
    }

    /// Review Focus 6: a page match is a row with iTerm2's snippet and a way back to the page.
    #[test]
    fn global_hits_follow_iterm2() {
        let h = web_hit(7, "Tab 1: Web", &result("see ", "rust", " now"));
        assert_eq!((h.pane, h.title.as_str(), h.snippet.as_str(), h.mark.clone()), (7, "Tab 1: Web", "see rust now", 4..8));
        assert_eq!(h.web, Some(serde_json::json!({"index": 3})));
        // Page strings are inert: bidi and control characters are gone, the mark still fits.
        let h = web_hit(7, "T\u{202e}itle\u{7}", &result("a\u{202e}b\n", "r\u{200b}ust", "\u{1b}c"));
        assert_eq!(h.title, "Title");
        assert_eq!(h.snippet, "ab rustc");
        assert_eq!(&h.snippet[h.mark.clone()], "rust");
        // Merge: sessions in the order given, terminal first.
        let g = merge(&[hit(1, 0), hit(1, 4)], &[web_hit(7, "w", &result("", "x", ""))]);
        let shape: Vec<(u64, usize)> = g.0.iter().map(|(p, _, h)| (*p, h.len())).collect();
        assert_eq!(shape, vec![(1, 2), (7, 1)]);
        assert_eq!(g.1, None);
        // The cap, with iTerm2's text.
        let many: Vec<Hit> = (0..MAX_HITS + 1).map(|i| hit(1, i)).collect();
        let (g, text) = merge(&many, &[]);
        assert_eq!(g.iter().map(|(_, _, h)| h.len()).sum::<usize>(), MAX_HITS);
        assert_eq!(text, Some(format!("Showing the first {MAX_HITS} matches.")));
        let (g, text) = merge(&many[..MAX_HITS - 1], &[web_hit(7, "w", &result("", "x", ""))]);
        assert_eq!((g.iter().map(|(_, _, h)| h.len()).sum::<usize>(), text), (MAX_HITS, None), "exactly the cap is not cut");
        // The row handler.
        assert!(matches!(pick_of(&web_hit(7, "w", &result("", "x", ""))), GlobalEvent::PickWeb(7, v) if v == serde_json::json!({"index": 3})));
        assert!(matches!(pick_of(&hit(2, 5)), GlobalEvent::Pick(2, m) if m.start.row == 5));
    }

    #[test]
    fn the_mode_popup_follows_the_xib() {
        let titles: Vec<&str> = MODE_ITEMS.iter().map(|(_, t)| *t).collect();
        assert_eq!(titles, ["Substring \u{2014} Automatic", "Substring \u{2014} Case Sensitive", "Substring \u{2014} Case Insensitive", "Regex \u{2014} Case Sensitive", "Regex \u{2014} Case Insensitive"]);
        // The item tags are `iTermFindMode` values.
        assert_eq!(MODE_ITEMS.map(|(m, _)| m as i64), [0, 1, 2, 3, 4]);
        assert_eq!(mode_title(Mode::RegexInsensitive), "Regex \u{2014} Case Insensitive");
    }

    #[test]
    fn short_queries_wait_for_return() {
        assert!(!should_search("ab", false));
        assert!(should_search("ab", true));
        assert!(should_search("abc", false));
        assert!(!should_search("", true));
    }
}
