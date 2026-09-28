//! iTerm2 > Suppressed Alerts (iTerm2's `iTermSuppressedAlertsWindowController`, `SAW`): the
//! warnings being answered automatically, un-suppressing them, and the "Always show alerts
//! with remembered selections" mode; and the menu item's title (`iTermApplicationDelegate.m:548-566`).

use std::collections::BTreeSet;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::warnings;

pub const TITLE: &str = "Suppressed Alerts";
/// The text at the top (`SAW:75`).
pub const EXPLANATION: &str = "These alerts are being answered automatically because you chose to remember your response. Select one and click Un-suppress to be asked again.";
/// Over the table when nothing is suppressed (`SAW:139-150`).
pub const EMPTY: &str = "No alerts are currently being suppressed.";
pub const ALERT_COLUMN: &str = "Alert";
pub const RESPONSE_COLUMN: &str = "Automatic Response";
pub const WHEN_COLUMN: &str = "Last Suppressed";
pub const UNSUPPRESS: &str = "Un-suppress";
pub const UNSUPPRESS_ALL: &str = "Un-suppress All";
pub const DONE_BUTTON: &str = "Done";
pub const SHOW_REMEMBERED: &str = "Always show alerts with remembered selections";
pub const SHOW_REMEMBERED_TIP: &str = "When you check \u{201c}Remember my choice\u{201d} or \u{201c}Suppress this message permanently\u{201d} in an alert, Tandem stops showing it and reuses your saved response. Turn this on to show those alerts again so you can see them or choose differently. It stays on until you turn it off.";

/// The panel's size and least size in points (`SAW:29-35`).
const SIZE: (f64, f64) = (640.0, 380.0);
const MIN_SIZE: (f64, f64) = (480.0, 260.0);
/// The table's fixed columns and least row height (`SAW:98-137`).
const RESPONSE_WIDTH: f32 = 150.0;
const WHEN_WIDTH: f32 = 130.0;
const ALERT_MIN_WIDTH: f32 = 120.0;
const ROW_H: f32 = 22.0;

/// iTerm2 > Suppressed Alerts' title: the count and how long ago the newest was answered
/// (`newest_age` seconds), or the plain title when nothing is suppressed.
pub fn menu_title(count: usize, newest_age: Option<f64>) -> String {
    if count == 0 {
        return format!("{TITLE}\u{2026}");
    }
    let counted = if count == 1 { "1 Alert Suppressed".to_string() } else { format!("{count} Alerts Suppressed") };
    match newest_age {
        Some(age) => format!("{counted} ({} ago)\u{2026}", warnings::compact_date_difference(age)),
        None => format!("{counted}\u{2026}"),
    }
}

/// The Alert column: the heading on its own line, then the text (`SAW:348-355`).
pub fn alert_text(heading: Option<&str>, title: &str) -> String {
    let heading = heading.unwrap_or("");
    if !heading.is_empty() && !title.is_empty() {
        format!("{heading}\n{title}")
    } else if heading.is_empty() {
        title.to_string()
    } else {
        heading.to_string()
    }
}

/// One table row: the warning's identifier and what the columns and the hover show.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub alert: String,
    pub response: String,
    pub when: String,
    /// The warning's text (`SAW:419-429`).
    pub tooltip: String,
}

/// The rows of `entries` (newest first, as `Catalog::current` gives them) at `now`, Unix
/// seconds.
pub fn rows(entries: &[warnings::Entry], now: f64) -> Vec<Row> {
    entries
        .iter()
        .map(|e| Row { id: e.id.clone(), alert: alert_text(e.heading.as_deref(), &e.title), response: e.label.clone(), when: warnings::date_difference(now - e.last, true), tooltip: e.title.clone() })
        .collect()
}

/// A click on row `index`: alone it selects that row; with Ctrl it toggles it; with Shift it
/// selects from the last row clicked to it (with Ctrl too, added to the selection).
/// `selected` and `anchor` hold identifiers, so they survive the rows changing.
pub fn click(rows: &[Row], selected: &mut BTreeSet<String>, anchor: &mut Option<String>, index: usize, ctrl: bool, shift: bool) {
    let Some(row) = rows.get(index) else { return };
    let from = anchor.as_ref().and_then(|a| rows.iter().position(|r| r.id == *a));
    match (shift, from) {
        (true, Some(from)) => {
            if !ctrl {
                selected.clear();
            }
            let (a, b) = if from <= index { (from, index) } else { (index, from) };
            selected.extend(rows[a..=b].iter().map(|r| r.id.clone()));
            return;
        }
        _ if ctrl => {
            if !selected.remove(&row.id) {
                selected.insert(row.id.clone());
            }
        }
        _ => {
            selected.clear();
            selected.insert(row.id.clone());
        }
    }
    *anchor = Some(row.id.clone());
}

/// What the person did in the window.
#[derive(Clone, Debug, PartialEq)]
pub enum SuppressedEvent {
    None,
    /// Un-suppress (the selected rows) or a double-click (that row): the identifiers.
    Unsuppress(Vec<String>),
    UnsuppressAll,
    /// The "Always show alerts with remembered selections" checkbox, now on or off.
    ShowRemembered(bool),
    /// Done, Enter, Escape, or the window closed.
    Close,
}

pub struct SuppressedWindow {
    host: EguiHost,
    rows: Vec<Row>,
    selected: BTreeSet<String>,
    anchor: Option<String>,
    show_remembered: bool,
}

impl SuppressedWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, rows: Vec<Row>, show_remembered: bool) -> Result<SuppressedWindow, String> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(SIZE.0, SIZE.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1));
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        Ok(SuppressedWindow { host, rows, selected: BTreeSet::new(), anchor: None, show_remembered })
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

    /// The rows now; the selection is kept by identifier (`SAW` `reload`).
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.selected.retain(|id| rows.iter().any(|r| r.id == *id));
        if self.anchor.as_ref().is_some_and(|a| !rows.iter().any(|r| r.id == *a)) {
            self.anchor = None;
        }
        self.rows = rows;
        self.host.request_redraw();
    }

    pub fn redraw(&mut self) -> SuppressedEvent {
        let mut out = SuppressedEvent::None;
        let before = (self.selected.clone(), self.show_remembered);
        let (rows, selected, anchor, show_remembered) = (&self.rows, &mut self.selected, &mut self.anchor, &mut self.show_remembered);
        self.host.paint(|ui| {
            if ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)) {
                out = SuppressedEvent::Close;
            }
            egui::Panel::top("explanation").show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(EXPLANATION).small());
                ui.add_space(4.0);
            });
            egui::Panel::bottom("buttons").show(ui, |ui| {
                ui.add_space(8.0);
                if ui.checkbox(show_remembered, SHOW_REMEMBERED).on_hover_text(SHOW_REMEMBERED_TIP).changed() {
                    out = SuppressedEvent::ShowRemembered(*show_remembered);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!rows.is_empty(), egui::Button::new(UNSUPPRESS_ALL)).clicked() {
                        out = SuppressedEvent::UnsuppressAll;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(DONE_BUTTON).clicked() {
                            out = SuppressedEvent::Close;
                        }
                        if ui.add_enabled(!selected.is_empty(), egui::Button::new(UNSUPPRESS)).clicked() {
                            out = SuppressedEvent::Unsuppress(rows.iter().filter(|r| selected.contains(&r.id)).map(|r| r.id.clone()).collect());
                        }
                    });
                });
                ui.add_space(8.0);
            });
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.allocate_rect(rect, egui::Sense::hover());
                if let Some(e) = table(ui, rect, rows, selected, anchor) {
                    out = e;
                }
            });
        });
        if (&self.selected, self.show_remembered) != (&before.0, before.1) {
            self.host.request_redraw();
        }
        out
    }
}

/// The striped table: the column headers, a row per warning as tall as its wrapped Alert
/// text, several rows selectable, a double-click un-suppressing the row clicked, and the empty
/// text over it when there are none.
fn table(ui: &mut egui::Ui, rect: egui::Rect, rows: &[Row], selected: &mut BTreeSet<String>, anchor: &mut Option<String>) -> Option<SuppressedEvent> {
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 0.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    let alert_w = (rect.width() - RESPONSE_WIDTH - WHEN_WIDTH).max(ALERT_MIN_WIDTH);
    let widths = [alert_w, RESPONSE_WIDTH, WHEN_WIDTH];
    let col_x = |c: usize| rect.left() + widths[..c].iter().sum::<f32>();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), ROW_H));
    painter.rect_filled(header, 0.0, visuals.faint_bg_color);
    for (c, title) in [ALERT_COLUMN, RESPONSE_COLUMN, WHEN_COLUMN].into_iter().enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(col_x(c), rect.top()), egui::vec2(widths[c], ROW_H));
        ui.painter_at(cell).text(cell.left_center() + egui::vec2(5.0, 0.0), egui::Align2::LEFT_CENTER, title, font.clone(), visuals.weak_text_color());
        if c > 0 {
            painter.vline(cell.left(), header.y_range(), visuals.widgets.noninteractive.bg_stroke);
        }
    }
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
    if rows.is_empty() {
        painter.text(body.center(), egui::Align2::CENTER_CENTER, EMPTY, font, visuals.weak_text_color());
        return None;
    }
    let mut out = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(1.0, 0.0))));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        for (i, row) in rows.iter().enumerate() {
            let on = selected.contains(&row.id);
            let color = if on { visuals.selection.stroke.color } else { visuals.text_color() };
            let alert = ui.painter().layout(row.alert.clone(), font.clone(), color, (widths[0] - 10.0).max(20.0));
            let h = (alert.size().y + 6.0).max(ROW_H);
            let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
            let fill = if on {
                Some(visuals.selection.bg_fill)
            } else if i % 2 == 1 {
                Some(visuals.faint_bg_color)
            } else {
                None
            };
            if let Some(f) = fill {
                ui.painter().rect_filled(r, 0.0, f);
            }
            let cell = |c: usize| egui::Rect::from_min_size(egui::pos2(col_x(c), r.top()), egui::vec2(widths[c], h));
            ui.painter_at(cell(0)).galley(cell(0).min + egui::vec2(5.0, 3.0), alert, color);
            for (c, s) in [(1, &row.response), (2, &row.when)] {
                ui.painter_at(cell(c).shrink2(egui::vec2(4.0, 0.0))).text(cell(c).left_center() + egui::vec2(5.0, 0.0), egui::Align2::LEFT_CENTER, s, font.clone(), color);
            }
            let resp = resp.on_hover_text(&row.tooltip);
            if resp.clicked() {
                let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                click(rows, selected, anchor, i, ctrl, shift);
            }
            if resp.double_clicked() {
                out = Some(SuppressedEvent::Unsuppress(vec![row.id.clone()]));
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warnings::Entry;

    fn entry(id: &str, heading: Option<&str>, title: &str, label: &str, last: f64) -> Entry {
        Entry { id: id.into(), title: title.into(), heading: heading.map(str::to_string), label: label.into(), last, count: 1, episode: None }
    }

    #[test]
    fn suppressed_window_follows_iterm2() {
        assert_eq!(menu_title(0, None), "Suppressed Alerts\u{2026}");
        assert_eq!(menu_title(1, Some(30.0)), "1 Alert Suppressed (< 1 min ago)\u{2026}");
        assert_eq!(menu_title(3, Some(7200.0)), "3 Alerts Suppressed (2 hrs ago)\u{2026}");
        assert_eq!(menu_title(2, None), "2 Alerts Suppressed\u{2026}");
        assert_eq!(alert_text(Some("H"), "T"), "H\nT");
        assert_eq!(alert_text(None, "T"), "T");
        assert_eq!(alert_text(Some("H"), ""), "H");
        let now = 100_000.0;
        let entries = [entry("NoSyncA", Some("Restart session?"), "Running jobs will be killed.", "OK", now - 10.0), entry("NoSyncB", None, "Paste it?", "Paste", now - 7200.0)];
        let got = rows(&entries, now);
        assert_eq!(
            got,
            vec![
                Row { id: "NoSyncA".into(), alert: "Restart session?\nRunning jobs will be killed.".into(), response: "OK".into(), when: "moments ago".into(), tooltip: "Running jobs will be killed.".into() },
                Row { id: "NoSyncB".into(), alert: "Paste it?".into(), response: "Paste".into(), when: "2 hours ago".into(), tooltip: "Paste it?".into() },
            ]
        );
    }

    /// Windows list clicks: alone selects one, Ctrl toggles, Shift spans from the last click.
    #[test]
    fn selection_follows_clicks() {
        let now = 1000.0;
        let entries: Vec<Entry> = ["a", "b", "c", "d"].iter().map(|id| entry(id, None, "t", "OK", now)).collect();
        let rows = rows(&entries, now);
        let (mut sel, mut anchor) = (BTreeSet::new(), None);
        let ids = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>();
        click(&rows, &mut sel, &mut anchor, 1, false, false);
        assert_eq!(ids(&sel), ["b"]);
        click(&rows, &mut sel, &mut anchor, 3, true, false);
        assert_eq!(ids(&sel), ["b", "d"]);
        click(&rows, &mut sel, &mut anchor, 3, true, false);
        assert_eq!(ids(&sel), ["b"], "Ctrl again deselects");
        click(&rows, &mut sel, &mut anchor, 0, false, false);
        click(&rows, &mut sel, &mut anchor, 2, false, true);
        assert_eq!(ids(&sel), ["a", "b", "c"]);
        click(&rows, &mut sel, &mut anchor, 1, false, true);
        assert_eq!(ids(&sel), ["a", "b"], "the span starts at the last plain click");
        click(&rows, &mut sel, &mut anchor, 9, false, false);
        assert_eq!(ids(&sel), ["a", "b"], "no such row");
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(TITLE, "Suppressed Alerts");
        assert_eq!(EXPLANATION, "These alerts are being answered automatically because you chose to remember your response. Select one and click Un-suppress to be asked again.");
        assert_eq!(EMPTY, "No alerts are currently being suppressed.");
        assert_eq!(SHOW_REMEMBERED, "Always show alerts with remembered selections");
        assert_eq!(
            SHOW_REMEMBERED_TIP,
            "When you check \u{201c}Remember my choice\u{201d} or \u{201c}Suppress this message permanently\u{201d} in an alert, Tandem stops showing it and reuses your saved response. Turn this on to show those alerts again so you can see them or choose differently. It stays on until you turn it off."
        );
        assert_eq!((ALERT_COLUMN, RESPONSE_COLUMN, WHEN_COLUMN), ("Alert", "Automatic Response", "Last Suppressed"));
        assert_eq!((UNSUPPRESS, UNSUPPRESS_ALL, DONE_BUTTON), ("Un-suppress", "Un-suppress All", "Done"));
    }
}
