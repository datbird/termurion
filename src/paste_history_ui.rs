//! Session > Open Paste History...: iTerm2's popup of earlier copies and pastes, newest
//! first, filtered by typing (a subsequence of each entry), Enter pastes the chosen one.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::popup_filter::{self, Filter};

pub enum HistEvent {
    None,
    /// Paste this entry (its index in the rows given).
    Pick(usize),
    Close,
}

pub struct PasteHistoryWindow {
    host: EguiHost,
    /// (value, "N lines, size, date"), newest first.
    rows: Vec<(String, String)>,
    filter: Filter,
    selected: usize,
    started: std::time::Instant,
}

impl PasteHistoryWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, rows: Vec<(String, String)>) -> Result<PasteHistoryWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Paste History", 716.0, 270.0)?;
        Ok(PasteHistoryWindow { host, rows, filter: Filter::default(), selected: 0, started: std::time::Instant::now() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn set_rows(&mut self, rows: Vec<(String, String)>) {
        self.rows = rows;
        self.host.request_redraw();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    /// The rows the filter keeps: (index into the rows, matched character positions).
    fn shown(&self) -> Vec<(usize, Vec<usize>)> {
        self.rows.iter().enumerate().filter_map(|(i, (v, _))| popup_filter::matches(self.filter.text(), v).map(|m| (i, m))).collect()
    }

    pub fn redraw(&mut self) -> HistEvent {
        let mut out = HistEvent::None;
        let shown = self.shown();
        let now = self.started.elapsed().as_secs_f64();
        let (filter, selected, rows) = (&mut self.filter, &mut self.selected, &self.rows);
        self.host.paint(|ui| {
            // Typing filters; Backspace clears; arrows move; Enter pastes; Esc closes.
            let (typed, keys) = ui.input(|i| {
                let typed: String = i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None }).collect();
                (typed, (i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)))
            });
            for c in typed.chars() {
                filter.type_char(c, now);
                *selected = 0;
            }
            let (back, up, down, enter, esc) = keys;
            if back {
                filter.backspace();
            }
            if up {
                *selected = selected.saturating_sub(1);
            }
            if down && *selected + 1 < shown.len() {
                *selected += 1;
            }
            if esc {
                out = HistEvent::Close;
            }
            if enter {
                if let Some((i, _)) = shown.get(*selected) {
                    out = HistEvent::Pick(*i);
                }
            }
            egui::CentralPanel::default().show(ui, |ui| {
                if !filter.text().is_empty() {
                    ui.label(format!("Filter: {}", filter.text()));
                }
                egui::Grid::new("history").num_columns(2).striped(true).show(ui, |ui| {
                    ui.strong("Value");
                    ui.strong("Time");
                    ui.end_row();
                    for (n, (i, hits)) in shown.iter().enumerate() {
                        let (value, when) = &rows[*i];
                        // Matched characters are bold (iTerm2 draws them so).
                        let mut job = egui::text::LayoutJob::default();
                        for (k, c) in value.chars().take(80).map(|c| if c == '\n' { ' ' } else { c }).enumerate() {
                            let mut f = egui::TextFormat::default();
                            if hits.contains(&k) {
                                f.color = ui.visuals().strong_text_color();
                            }
                            job.append(&c.to_string(), 0.0, f);
                        }
                        if ui.selectable_label(n == *selected, job).clicked() {
                            out = HistEvent::Pick(*i);
                        }
                        ui.label(when);
                        ui.end_row();
                    }
                });
            });
        });
        out
    }
}
