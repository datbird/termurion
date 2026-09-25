//! iTerm2's Find Globally: one query over every session, results grouped by session.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::find::Match;

/// One match in one session: where it is, and a snippet of its line with the match marked.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub pane: u64,
    pub title: String,
    pub m: Match,
    pub snippet: String,
    /// The match's byte range in `snippet`.
    pub mark: std::ops::Range<usize>,
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

pub enum GlobalEvent {
    None,
    /// The query or the mode changed.
    Search(String),
    /// A result was clicked: its session and match.
    Pick(u64, Match),
    Close,
}

pub struct GlobalFindWindow {
    host: EguiHost,
    query: String,
    groups: Vec<(u64, String, Vec<Hit>)>,
    error: Option<String>,
    first: bool,
}

impl GlobalFindWindow {
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        query: &str,
    ) -> Result<GlobalFindWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Find Globally", 560.0, 420.0)?;
        Ok(GlobalFindWindow { host, query: query.into(), groups: Vec::new(), error: None, first: true })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
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
        let (query, groups, error, first) = (&mut self.query, &self.groups, &self.error, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
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
                                    out = GlobalEvent::Pick(h.pane, h.m);
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
        Hit { pane, title: format!("s{pane}"), m, snippet: "ab".into(), mark: 0..2 }
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
    fn short_queries_wait_for_return() {
        assert!(!should_search("ab", false));
        assert!(should_search("ab", true));
        assert!(should_search("abc", false));
        assert!(!should_search("", true));
    }
}
