//! Code Review mode on the app side (milestone 13y1, Task 7): the prompt overlay a review member waits
//! behind, Start and Reload, the prompts pulldown and its two windows, the auto-send and auto-request
//! behaviors, and the system prompt's temp file. The pure side is `code_review`.
//!
//! A review member starts as an idle pane (no program) like a Diff member; Start restarts it into its own
//! command as an argument vector (B5): the prompt is one argument, the system prompt a file only the
//! person can read. Review text goes back into a coding agent's terminal only through the paste path,
//! bracketed, controls removed, never into a plain shell (B6).
//!
//! Citations: `PCR` `PTYSession+CodeReviewPrompt.swift`, `CRV` `CodeReviewPromptView.swift`, `WPP`
//! `iTermWorkgroupPeerPort.swift`, `WI` `iTermWorkgroupInstance.swift` under
//! `~/gitrepos/iterm2-ref/sources/Workgroups`.

use super::workgroup_app::MemberLaunch;
use super::*;
use crate::code_review::{self as cr, SavedPrompt};
use crate::code_review_ui::{ManagerEvent, ManagerWindow, NameEvent, NameWindow};
use crate::text_edit::{EditKey, TextEdit};
use crate::workgroup::Mode;
use std::collections::BTreeMap;

/// The prompt overlay of one review member: the multi-line field and the line it starts showing at.
pub(super) struct Overlay {
    pub edit: TextEdit,
    pub scroll: usize,
    /// The field has the keyboard (it takes it when the overlay opens and when it is clicked).
    pub focused: bool,
}

impl Overlay {
    pub(super) fn new(text: &str) -> Overlay {
        let mut edit = TextEdit::default();
        edit.load(text);
        Overlay { edit, scroll: 0, focused: true }
    }
}

/// The review state of one instance, by member config id.
#[derive(Default)]
pub(super) struct ReviewSide {
    pub overlays: BTreeMap<String, Overlay>,
    /// The prompt Start (or an auto-request) gave a member: read once by its launch.
    pub start: BTreeMap<String, String>,
    /// The prompt last submitted, which the overlay offers again (`codeReviewLastUsedPrompt`).
    pub last_prompt: BTreeMap<String, String>,
    /// The system prompt's temp file of each member (deleted at teardown and at the member's end).
    pub files: BTreeMap<String, std::path::PathBuf>,
    /// The coding agent in front of the main session when the last review started: an auto-send goes
    /// only while that same agent is still there (None: it was at a shell prompt, nothing is sent).
    pub agent: Option<cr::Agent>,
}

/// The windows the overlay's pulldown opens, and the text the name sheet will save.
#[derive(Default)]
pub(super) struct ReviewWindows {
    pub manager: Option<ManagerWindow>,
    pub name: Option<(NameWindow, String)>,
    /// The pane whose Prompts pulldown is open.
    pub menu_pane: Option<PaneId>,
}

/// The id prefix of the pulldown's native menu.
const MENU: &str = "review-prompt:";

/// Which toggle of the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Toggle {
    AutoSend,
    AutoRequest,
}

// --- Geometry ---

struct Geom {
    title: Rect,
    field: Rect,
    prompts: Rect,
    start: Rect,
}

fn geom(panel: Rect, cell: (f32, f32), s: f32) -> Geom {
    let (inset, gap, button_h) = (16.0 * s, 8.0 * s, 28.0 * s);
    let w = |t: &str, min: f32| (unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0 + 28.0 * s).max(min * s);
    let by = panel.y + panel.h - inset - button_h;
    let start_w = w(cr::START, 80.0);
    let start = Rect { x: panel.x + panel.w - inset - start_w, y: by, w: start_w, h: button_h };
    let prompts_w = w(cr::MENU_HEADER, 160.0);
    let prompts = Rect { x: start.x - 12.0 * s - prompts_w, y: by, w: prompts_w, h: button_h };
    let title = Rect { x: panel.x + inset, y: panel.y + inset, w: (panel.w - 2.0 * inset).max(0.0), h: cell.1 * 1.4 };
    let top = title.y + title.h + gap;
    let field = Rect { x: title.x, y: top, w: title.w, h: (by - gap - top).max(40.0 * s) };
    Geom { title, field, prompts, start }
}

/// The room a field's text has: its width less its padding, in pixels.
fn text_width(field: Rect, s: f32) -> f32 {
    (field.w - 12.0 * s).max(8.0)
}

/// Visual lines of the text at the field's width.
fn lines_of(text: &str, field: Rect, cw: f32, s: f32) -> Vec<crate::text_edit::VisualLine> {
    let measure = move |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cw;
    crate::text_edit::wrap(text, text_width(field, s), &measure)
}

fn visible_lines(field: Rect, ch: f32, s: f32) -> usize {
    (((field.h - 8.0 * s) / ch).floor() as usize).max(1)
}

impl App {
    // --- The overlay ---

    /// The text the overlay opens with: the prompt last used by this member, else the store's default
    /// (the last selected prompt, else the first).
    fn review_default_text(&self, last: Option<&String>) -> String {
        last.cloned().unwrap_or_else(|| cr::default_prompt_text(&cr::prompts_of(&self.globals), cr::last_selected(&self.globals).as_deref()))
    }

    /// Shows the overlay of review member `config` (a fresh entry, or Reload).
    pub(super) fn show_review_overlay(&mut self, instance: &str, config: &str) {
        self.persist_prompt_seed();
        let last = self.workgroups.get(instance).and_then(|i| i.review.last_prompt.get(config)).cloned();
        let text = self.review_default_text(last.as_ref());
        if let Some(inst) = self.workgroups.get_mut(instance) {
            inst.review.overlays.insert(config.to_string(), Overlay::new(&text));
        }
        self.redraw();
    }

    /// The entry's overlays: one per review member that started, each with the default text.
    pub(super) fn review_entry_overlays(&mut self, inst: &mut super::workgroup_app::Instance, configs: &[String]) {
        self.persist_prompt_seed();
        let text = self.review_default_text(None);
        for c in configs {
            inst.review.overlays.insert(c.clone(), Overlay::new(&text));
        }
    }

    /// Writes the seeded "Default" prompt when the stored value is missing or empty; a malformed value is
    /// left alone until the first edit (P10).
    fn persist_prompt_seed(&mut self) {
        let raw = self.globals.string(cr::KEY);
        if cr::should_persist_seed(Some(&raw)) {
            let (seed, _) = cr::load_prompts(None, &self.globals.string(crate::ai_prompts::CODE_REVIEW_KEY));
            self.store_prompts(seed);
        }
    }

    /// The one write point of the saved prompts (P10).
    pub(super) fn store_prompts(&mut self, list: Vec<SavedPrompt>) {
        let list = cr::bounded(list);
        self.globals.set(cr::KEY, toml::Value::String(cr::encode_prompts(&list)));
        self.save_globals();
    }

    fn set_last_selected(&mut self, uuid: &str) {
        self.globals.set(cr::LAST_KEY, toml::Value::String(uuid.to_string()));
        self.save_globals();
    }

    /// The shown members of the front tab that wear the overlay: (pane, terminal rectangle).
    fn review_panels(&self) -> Vec<(PaneId, Rect)> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        if self.workgroups.is_empty() {
            return Vec::new();
        }
        self.visible(tab).into_iter().filter(|(id, _)| self.review_overlay_of(*id).is_some()).collect()
    }

    /// The overlay `pane` wears, with its instance and config.
    fn review_overlay_of(&self, pane: PaneId) -> Option<(&Overlay, String, String)> {
        let (instance, config) = self.member_config(pane)?;
        let o = self.workgroups.get(&instance)?.review.overlays.get(&config)?;
        Some((o, instance, config))
    }

    /// Draws each overlay: the pane's text stops under it (`cut_out`), its boxes are popups and its words
    /// rows over them (P22).
    pub(super) fn draw_review_overlays(&mut self, frame: &mut scene::Frame, cell: (f32, f32)) {
        use scene::{Quad, Span, TextRow};
        let panels = self.review_panels();
        if panels.is_empty() {
            return;
        }
        let s = self.scale();
        let span = |text: String, color: crate::palette::Rgb, bold: bool| Span { text, color, bold, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let mix = |a: crate::palette::Rgb, b: crate::palette::Rgb, t: f32| {
            let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
            crate::palette::Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
        };
        let row = |x: f32, y: f32, spans: Vec<Span>, clip: Rect| TextRow { x, y, spans, clip, scale: 1.0, face: crate::render::DEFAULT_FACE };
        for (pane, panel) in panels {
            let Some(p) = self.panes.get(&pane) else { continue };
            let (bg, fg, selected) = (p.screen.palette.bg, p.screen.palette.fg, p.screen.palette.selection);
            let (back, field_bg, dim, line) = (mix(bg, fg, 0.04), mix(bg, fg, 0.08), mix(bg, fg, 0.6), mix(bg, fg, 0.3));
            let g = geom(panel, cell, s);
            let (instance, config) = match self.member_config(pane) {
                Some(m) => m,
                None => continue,
            };
            let visible = visible_lines(g.field, cell.1, s);
            // Keep the caret on screen.
            if let Some(o) = self.workgroups.get_mut(&instance).and_then(|i| i.review.overlays.get_mut(&config)) {
                let lines = lines_of(&o.edit.text, g.field, cell.0, s);
                let measure = |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0;
                let (caret_line, _) = o.edit.caret_line(&lines, &measure);
                o.edit.page_lines = visible;
                if caret_line < o.scroll {
                    o.scroll = caret_line;
                } else if caret_line >= o.scroll + visible {
                    o.scroll = caret_line + 1 - visible;
                }
                o.scroll = o.scroll.min(lines.len().saturating_sub(1));
            }
            let Some(o) = self.workgroups.get(&instance).and_then(|i| i.review.overlays.get(&config)) else { continue };
            scene::cut_out(frame, panel, cell.1);
            frame.popups.push(Quad { rect: panel, color: scene::rgba(back, 1.0) });
            frame.rows.push(row(g.title.x, g.title.y, vec![span(cr::TITLE.to_string(), fg, true)], g.title));
            frame.popups.push(Quad { rect: g.field, color: scene::rgba(field_bg, 1.0) });
            let t = s.max(1.0);
            let on = if o.focused { fg } else { line };
            for q in [Rect { h: t, ..g.field }, Rect { y: g.field.y + g.field.h - t, h: t, ..g.field }, Rect { w: t, ..g.field }, Rect { x: g.field.x + g.field.w - t, w: t, ..g.field }] {
                frame.popups.push(Quad { rect: q, color: scene::rgba(on, 1.0) });
            }
            let lines = lines_of(&o.edit.text, g.field, cell.0, s);
            let measure = |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0;
            let (x0, y0) = (g.field.x + 6.0 * s, g.field.y + 4.0 * s);
            let show = o.scroll..(o.scroll + visible).min(lines.len());
            if o.focused {
                for (i, x, w) in crate::text_edit::selection_spans(o.edit.selection(), &o.edit.text, &lines, show.clone(), &measure) {
                    frame.popups.push(Quad { rect: Rect { x: x0 + x, y: y0 + (i - o.scroll) as f32 * cell.1, w, h: cell.1 }, color: scene::rgba(selected, 1.0) });
                }
                let (cl, cx) = o.edit.caret_line(&lines, &measure);
                if show.contains(&cl) {
                    frame.popups.push(Quad { rect: Rect { x: x0 + cx, y: y0 + (cl - o.scroll) as f32 * cell.1, w: t, h: cell.1 }, color: scene::rgba(fg, 1.0) });
                }
            }
            for i in show {
                let l = &lines[i];
                let text = crate::inert_files::display_text(o.edit.text.get(l.start..l.end).unwrap_or(""), 4096, false);
                frame.rows.push(row(x0, y0 + (i - o.scroll) as f32 * cell.1, vec![span(text, fg, false)], g.field));
            }
            let button = |frame: &mut scene::Frame, r: Rect, label: &str, strong: bool| {
                frame.popups.push(Quad { rect: r, color: scene::rgba(mix(back, fg, 0.12), 1.0) });
                for q in [Rect { h: t, ..r }, Rect { y: r.y + r.h - t, h: t, ..r }, Rect { w: t, ..r }, Rect { x: r.x + r.w - t, w: t, ..r }] {
                    frame.popups.push(Quad { rect: q, color: scene::rgba(if strong { selected } else { line }, 1.0) });
                }
                let tw = unicode_width::UnicodeWidthStr::width(label) as f32 * cell.0;
                frame.rows.push(row(r.x + (r.w - tw) / 2.0, r.y + (r.h - cell.1) / 2.0, vec![span(label.to_string(), if strong { fg } else { dim }, false)], r));
            };
            button(frame, g.prompts, &format!("{} \u{25be}", cr::MENU_HEADER), false);
            button(frame, g.start, cr::START, true);
        }
    }

    /// A press on an overlay: Start, the Prompts pulldown, or a click in the field. Every press on the
    /// panel is taken (the terminal is not under it). True when it was on a panel.
    pub(super) fn review_overlay_press(&mut self, button: MouseButton, x: f32, y: f32) -> bool {
        let (s, cell) = (self.scale(), self.cell());
        let Some((pane, panel)) = self.review_panels().into_iter().find(|(_, r)| r.contains(x, y)) else { return false };
        if button != MouseButton::Left {
            return true;
        }
        let g = geom(panel, cell, s);
        let Some((_, instance, config)) = self.review_overlay_of(pane).map(|(o, i, c)| (o.focused, i, c)) else { return true };
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        if g.start.contains(x, y) {
            let text = self.workgroups.get(&instance).and_then(|i| i.review.overlays.get(&config)).map(|o| o.edit.text.clone()).unwrap_or_default();
            self.start_review(pane, &text);
        } else if g.prompts.contains(x, y) {
            self.open_prompts_menu(pane, super::statusbar_content::MenuAt { x: g.prompts.x, y: g.prompts.y + g.prompts.h, above: false });
        } else if g.field.contains(x, y) {
            let shift = self.mods.shift;
            let now = self.started.elapsed().as_millis();
            if let Some(o) = self.workgroups.get_mut(&instance).and_then(|i| i.review.overlays.get_mut(&config)) {
                o.focused = true;
                let lines = lines_of(&o.edit.text, g.field, cell.0, s);
                let line = (o.scroll + ((y - g.field.y - 4.0 * s) / cell.1).floor().max(0.0) as usize).min(lines.len().saturating_sub(1));
                let measure = |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0;
                let at = crate::text_edit::index_at(&o.edit.text, &lines, line, x - (g.field.x + 6.0 * s), &measure);
                let end = crate::text_edit::ends_line(&lines, line, at);
                o.edit.click(at, end, shift, now, 500);
            }
        }
        self.redraw();
        true
    }

    /// The focused pane's overlay field takes the keys. Chords with Alt or Win fall through (the peer and
    /// toolbar shortcuts fire while it has the keyboard); Shift+Return starts the review, Return is a new line.
    pub(super) fn review_overlay_key(&mut self, ev: &KeyEvent) -> bool {
        use winit::keyboard::{Key as WKey, NamedKey};
        let Some(pane) = self.focused_id() else { return false };
        let Some((focused, instance, config)) = self.review_overlay_of(pane).map(|(o, i, c)| (o.focused, i, c)) else { return false };
        let Some((_, panel)) = self.review_panels().into_iter().find(|(p, _)| *p == pane) else { return false };
        if !focused {
            return false;
        }
        if ev.state != ElementState::Pressed || super::toolbelt_tools_ui::is_modifier(&ev.logical_key) {
            return true;
        }
        let mods = self.mods;
        let plain = !(mods.ctrl || mods.alt || mods.cmd);
        if matches!(&ev.logical_key, WKey::Named(NamedKey::Enter)) && mods.shift && !mods.ctrl && !mods.alt && !mods.cmd {
            let text = self.workgroups.get(&instance).and_then(|i| i.review.overlays.get(&config)).map(|o| o.edit.text.clone()).unwrap_or_default();
            self.start_review(pane, &text);
            return true;
        }
        let Some((key, ctrl, alt)) = super::toolbelt_more_ui::edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return plain };
        if matches!(key, EditKey::Copy | EditKey::Cut | EditKey::Paste(_) | EditKey::Text(_) | EditKey::Enter | EditKey::Tab | EditKey::Backspace | EditKey::Delete | EditKey::Left | EditKey::Right | EditKey::Up | EditKey::Down | EditKey::Home | EditKey::End | EditKey::PageUp | EditKey::PageDown | EditKey::WordLeft | EditKey::WordRight | EditKey::DeleteWordBack | EditKey::DeleteWordForward | EditKey::TextStart | EditKey::TextEnd | EditKey::SelectAll | EditKey::Undo | EditKey::Redo) && !crate::text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let key = self.with_clipboard_text(key);
        let (s, cell) = (self.scale(), self.cell());
        let g = geom(panel, cell, s);
        let shift = mods.shift;
        let measure = move |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0;
        let mut copied = None;
        if let Some(o) = self.workgroups.get_mut(&instance).and_then(|i| i.review.overlays.get_mut(&config)) {
            // The prompt is bounded as it is typed (P10): nothing past 64 KiB goes in.
            let adds = match &key {
                EditKey::Text(t) | EditKey::Paste(t) => t.len(),
                EditKey::Enter | EditKey::Tab => 1,
                _ => 0,
            };
            if adds > 0 && o.edit.text.len() + adds > cr::MAX_TEXT {
                return true;
            }
            let lines = lines_of(&o.edit.text, g.field, cell.0, s);
            if let crate::text_edit::EditOut::Copied(t) = o.edit.key(key, shift, &lines, &measure) {
                copied = Some(t);
            }
        }
        if let Some(t) = copied {
            self.set_clipboard_text(t);
        }
        self.redraw();
        true
    }

    // --- The pulldown and its windows ---

    /// The Prompts pulldown (`CRV` `rebuildPromptMenu`): the saved prompts, a separator when there are any,
    /// Save Current as New (only with text) and Manage Prompts.
    fn open_prompts_menu(&mut self, pane: PaneId, at: super::statusbar_content::MenuAt) {
        use muda::{Menu, MenuItem, PredefinedMenuItem};
        let Some((o, _, _)) = self.review_overlay_of(pane) else { return };
        let can_save = !o.edit.text.trim().is_empty();
        let prompts = cr::prompts_of(&self.globals);
        let menu = Menu::new();
        for (i, p) in prompts.iter().enumerate() {
            let shown = crate::inert_files::display_text(&p.name, cr::MAX_NAME, false).replace('&', "&&");
            let _ = menu.append(&MenuItem::with_id(format!("{MENU}{i}"), if shown.is_empty() { cr::UNTITLED.to_string() } else { shown }, true, None));
        }
        if !prompts.is_empty() {
            let _ = menu.append(&PredefinedMenuItem::separator());
        }
        let _ = menu.append(&MenuItem::with_id(format!("{MENU}save"), cr::SAVE_AS_NEW, can_save, None));
        let _ = menu.append(&MenuItem::with_id(format!("{MENU}manage"), cr::MANAGE, true, None));
        self.workgroups.review.menu_pane = Some(pane);
        self.show_menu_at(menu, at);
    }

    /// A pick from the pulldown. False for other ids.
    pub(super) fn review_menu_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(rest) = id.strip_prefix(MENU) else { return false };
        let Some(pane) = self.workgroups.review.menu_pane.take() else { return true };
        let Some((instance, config)) = self.member_config(pane) else { return true };
        match rest {
            "save" => {
                let text = self.workgroups.get(&instance).and_then(|i| i.review.overlays.get(&config)).map(|o| o.edit.text.clone()).unwrap_or_default();
                if text.trim().is_empty() {
                    return true;
                }
                if let Some((w, _)) = &mut self.workgroups.review.name {
                    w.focus();
                    return true;
                }
                let Some(g) = self.gpu.as_ref() else { return true };
                match NameWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
                    Ok(w) => self.workgroups.review.name = Some((w, text)),
                    Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                }
            }
            "manage" => self.open_prompt_manager(event_loop),
            n => {
                let prompts = cr::prompts_of(&self.globals);
                if let Some(p) = n.parse::<usize>().ok().and_then(|i| prompts.get(i)) {
                    if let Some(o) = self.workgroups.get_mut(&instance).and_then(|i| i.review.overlays.get_mut(&config)) {
                        o.edit.load(&p.text);
                        o.scroll = 0;
                    }
                    let uuid = p.uuid.clone();
                    self.set_last_selected(&uuid);
                }
            }
        }
        self.redraw();
        true
    }

    fn open_prompt_manager(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.workgroups.review.manager {
            w.focus();
            return;
        }
        self.persist_prompt_seed();
        let prompts = cr::prompts_of(&self.globals);
        let last = cr::last_selected(&self.globals);
        let Some(g) = self.gpu.as_ref() else { return };
        match ManagerWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, prompts, last.as_deref()) {
            Ok(w) => self.workgroups.review.manager = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events of the manager and the name sheet. True when one was theirs.
    pub(super) fn review_window_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        if self.workgroups.review.manager.as_ref().is_some_and(|w| w.id() == window) {
            match event {
                WindowEvent::CloseRequested => self.workgroups.review.manager = None,
                WindowEvent::RedrawRequested => {
                    let out = self.workgroups.review.manager.as_mut().map(ManagerWindow::redraw);
                    match out {
                        Some(ManagerEvent::Changed(list)) => {
                            self.store_prompts(list);
                            self.redraw();
                        }
                        Some(ManagerEvent::Selected(uuid)) => self.set_last_selected(&uuid),
                        _ => {}
                    }
                }
                other => {
                    if let Some(w) = self.workgroups.review.manager.as_mut() {
                        w.event(other);
                    }
                }
            }
            return true;
        }
        if self.workgroups.review.name.as_ref().is_some_and(|(w, _)| w.id() == window) {
            match event {
                WindowEvent::CloseRequested => self.workgroups.review.name = None,
                WindowEvent::RedrawRequested => {
                    let out = self.workgroups.review.name.as_mut().map(|(w, _)| w.redraw());
                    match out {
                        Some(NameEvent::Cancel) => self.workgroups.review.name = None,
                        Some(NameEvent::Save(name)) => {
                            if let Some((_, text)) = self.workgroups.review.name.take() {
                                self.save_prompt_as_new(&name, &text);
                            }
                        }
                        _ => {}
                    }
                }
                other => {
                    if let Some((w, _)) = self.workgroups.review.name.as_mut() {
                        w.event(other);
                    }
                }
            }
            return true;
        }
        false
    }

    /// Save Current as New: the prompt joins the list and is the last selected (`CRV:saveAsNewMenuItem`).
    fn save_prompt_as_new(&mut self, name: &str, text: &str) {
        let mut list = cr::prompts_of(&self.globals);
        let uuid = cr::fresh_uuid();
        list.push(SavedPrompt { uuid: uuid.clone(), name: name.to_string(), text: text.to_string() });
        self.store_prompts(list);
        self.set_last_selected(&uuid);
        let stored = cr::prompts_of(&self.globals);
        if let Some(m) = self.workgroups.review.manager.as_mut() {
            m.reload(stored);
        }
    }

    // --- Start, Reload, request ---

    /// Start: the prompt is the member's one-shot, its program restarts into the review command, the
    /// overlay goes (`PCR` `presentCodeReviewPromptOverlay` onStart). A launch that is refused leaves the
    /// overlay where it was.
    pub(super) fn start_review(&mut self, pane: PaneId, prompt: &str) {
        let Some((instance, config)) = self.member_config(pane) else { return };
        if let Some(inst) = self.workgroups.get_mut(&instance) {
            inst.review.start.insert(config.clone(), prompt.to_string());
        }
        self.in_instance_window(&instance, |a| a.restart_launching(pane));
        let consumed = self.workgroups.get_mut(&instance).is_some_and(|inst| inst.review.start.remove(&config).is_none());
        let started = consumed && self.panes.get(&pane).is_some_and(|p| p.shell_pid().is_some());
        if started {
            let main = self.workgroups.get(&instance).map(|i| i.leader);
            let agent = main.and_then(|m| cr::agent_of(self.foreground(m).as_ref()));
            if let Some(inst) = self.workgroups.get_mut(&instance) {
                inst.review.last_prompt.insert(config.clone(), prompt.to_string());
                inst.review.overlays.remove(&config);
                inst.review.agent = agent;
            }
        }
        self.redraw();
    }

    /// Reload of a review member (`reloadCodeReviewPromptOverlay`): the overlay again, the program keeps
    /// running until Start.
    pub(super) fn review_reload(&mut self, pane: PaneId) {
        if let Some((instance, config)) = self.member_config(pane) {
            self.show_review_overlay(&instance, &config);
        }
    }

    /// A review member's launch (called by `workgroup_launch`): with a one-shot prompt it is the review
    /// command as arguments; without one the overlay shows and nothing starts.
    pub(super) fn review_launch(&mut self, instance: &str, config: &str, template: &str) -> MemberLaunch {
        let Some(prompt) = self.workgroups.get_mut(instance).and_then(|i| i.review.start.remove(config)) else {
            self.show_review_overlay(instance, config);
            return MemberLaunch::Refused;
        };
        match self.review_command(instance, config, template, &prompt) {
            Ok(line) => {
                // The one-shot is spent only by a launch that was made; the caller reads this to drop the overlay.
                MemberLaunch::Run { command: Some(line), env: vec![format!("ITERM_WORKGROUP_ID={instance}")] }
            }
            Err(why) => {
                self.banners.set("workgroup", why);
                // Not started: the prompt is given back so Start can tell, and the overlay stays.
                if let Some(inst) = self.workgroups.get_mut(instance) {
                    inst.review.start.insert(config.to_string(), prompt);
                }
                MemberLaunch::Refused
            }
        }
    }

    /// The command line of a review: the system prompt file, the settings file, the argument vector from
    /// the person's template, `claude.exe` only (a batch target is refused).
    fn review_command(&mut self, instance: &str, config: &str, template: &str, prompt: &str) -> Result<String, String> {
        let base = self.workgroups.get(instance).map(|i| i.git_base.clone()).unwrap_or_else(|| "HEAD".to_string());
        let key = crate::ai_prompts::Prompt::CodeReviewSystem.key();
        let stored = self.globals.string(key);
        let system = if stored.is_empty() { crate::ai_prompts::default_text(key).unwrap_or_default().to_string() } else { stored };
        let dir = review_tmp_dir().ok_or("Tandem could not find a local folder for the review's files, so the review did not start.")?;
        let file = self.write_review_file(instance, config, &dir, &system)?;
        let resources = write_review_settings(&dir)?;
        let made = cr::review_argv(template, prompt, &file, &resources, &base)?;
        if made.cut {
            self.banners.set("workgroup", "The prompt was longer than 16 KiB, so only its first 16 KiB was used.");
        }
        let path = std::env::var_os("PATH");
        let program = cr::review_program(&made.argv[0], &|p| crate::semantic::resolve_on_path(p, path.as_deref()))?;
        let line = crate::workgroup_run::argv_line(&program, &made.argv[1..])?;
        if !cr::line_fits(&line) {
            return Err("The review command is too long for Windows, so the review did not start.".to_string());
        }
        Ok(line)
    }

    /// The member's system prompt file: a new owner-only file with an unguessable name; the one this
    /// member made before is deleted.
    fn write_review_file(&mut self, instance: &str, config: &str, dir: &crate::localfs::LocalPath, text: &str) -> Result<std::path::PathBuf, String> {
        if let Some(old) = self.workgroups.get_mut(instance).and_then(|i| i.review.files.remove(config)) {
            remove_review_file(&old);
        }
        #[cfg(windows)]
        let made = cr::write_system_prompt(dir, text).map_err(|e| format!("Tandem could not write the review's system prompt: {e}"));
        #[cfg(not(windows))]
        let made: Result<std::path::PathBuf, String> = {
            let _ = (dir, text);
            Err("Code review runs on Windows only.".to_string())
        };
        let path = made?;
        if let Some(inst) = self.workgroups.get_mut(instance) {
            inst.review.files.insert(config.to_string(), path.clone());
        }
        Ok(path)
    }

    /// `requestCodeReviewFromSoleReviewSession` (`WI:640-656`): a review on the workgroup's one review
    /// member, started with the prompt its overlay shows (or the last one used, else the default), and the
    /// member shown without moving the window. False unless there is exactly one live review member that
    /// can start.
    pub(super) fn request_review(&mut self, instance: &str) -> bool {
        self.request_review_with(instance, None)
    }

    /// `request_review` with the prompt a start_code_review call chose (`DI:3195-3216`), which replaces
    /// the overlay's text; None keeps the review's own choice.
    pub(super) fn request_review_with(&mut self, instance: &str, chosen: Option<String>) -> bool {
        let Some(inst) = self.workgroups.get(instance) else { return false };
        let reviews: Vec<&crate::workgroup::SessionConfig> = inst.workgroup.sessions.iter().filter(|s| s.mode == Mode::CodeReview).collect();
        let [only] = reviews.as_slice() else { return false };
        let config = only.unique_identifier.clone();
        let Some(pane) = inst.pane_of(&config).or_else(|| inst.non_peers.iter().find(|(c, _)| *c == config).map(|(_, p)| *p)) else { return false };
        if !self.panes.contains_key(&pane) {
            return false;
        }
        let shown = inst.review.overlays.get(&config).map(|o| o.edit.text.clone());
        let last = inst.review.last_prompt.get(&config).cloned();
        if shown.is_none() && !self.restartable(pane) {
            return false;
        }
        let prompt = chosen.or(shown).unwrap_or_else(|| self.review_default_text(last.as_ref()));
        self.start_review(pane, &prompt);
        let started = self.workgroups.get(instance).is_some_and(|i| !i.review.overlays.contains_key(&config));
        if started {
            self.reveal_as_peer(pane);
        }
        started
    }

    /// A review member's program ended: its system prompt file goes.
    pub(super) fn code_review_member_ended(&mut self, pane: PaneId) {
        let Some((instance, config)) = self.member_config(pane) else { return };
        if let Some(old) = self.workgroups.get_mut(&instance).and_then(|i| i.review.files.remove(&config)) {
            remove_review_file(&old);
        }
    }

    /// A teardown: the files and the waiting Returns of the instance go.
    pub(super) fn code_review_teardown(&mut self, instance: &str, review: &mut ReviewSide) {
        for (_, f) in std::mem::take(&mut review.files) {
            remove_review_file(&f);
        }
        self.cancel_paste_after(instance);
    }

    // --- The toggles ---

    /// Whether the toggle is on for `pane`.
    pub(super) fn toggle_on(&self, pane: PaneId, which: Toggle) -> bool {
        self.panes.get(&pane).is_some_and(|p| if which == Toggle::AutoSend { p.auto_send_clippings } else { p.auto_request_review })
    }

    /// Auto-request works only with exactly one review session in the workgroup (the item says so).
    pub(super) fn auto_request_enabled(&self, pane: PaneId) -> bool {
        self.workgroup_of(pane).is_some_and(|i| i.workgroup.sessions.iter().filter(|s| s.mode == Mode::CodeReview).count() == 1)
    }

    /// A click on a toggle: it flips on the pane that owns the toolbar item and says so (`WAS`, `WAR`).
    pub(super) fn toggle_auto(&mut self, pane: PaneId, which: Toggle) {
        if which == Toggle::AutoRequest && !self.auto_request_enabled(pane) {
            self.banners.set("workgroup", "Auto-request a review when idle (needs exactly one code review session)");
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let flag = if which == Toggle::AutoSend { &mut p.auto_send_clippings } else { &mut p.auto_request_review };
        *flag = !*flag;
        let on = *flag;
        // Turned off: a Return already waiting for an auto-send goes too.
        if !on && which == Toggle::AutoSend {
            if let Some(i) = self.workgroup_of(pane).map(|i| i.id.clone()) {
                self.cancel_paste_after(&i);
            }
        }
        let text = match (which, on) {
            (Toggle::AutoSend, true) => "Auto-send clippings to the main session when idle: on",
            (Toggle::AutoSend, false) => "Auto-send clippings to the main session when idle: off",
            (Toggle::AutoRequest, true) => "Auto-request a review from the code review session when idle: on",
            (Toggle::AutoRequest, false) => "Auto-request a review from the code review session when idle: off",
        };
        self.banners.set("workgroup", text);
        self.mark_state_dirty();
        self.redraw();
    }

    // --- The idle transitions ---

    /// Status changes of the pass (the same list `tick_status_change` hands every consumer): a review
    /// member going working to idle with its toggle on sends its clippings to the main session; the main
    /// session doing so with its toggle on and one review session asks for a review (`WPP:216-345`).
    pub(super) fn code_review_status_changed(&mut self, changes: &[(PaneId, Option<String>, Option<String>)]) {
        if self.workgroups.is_empty() {
            return;
        }
        for (pane, before, after) in changes {
            let Some(instance) = self.panes.get(pane).and_then(|p| p.workgroup.clone()) else { continue };
            let (prev, now) = (before.as_deref().map(|s| cr::state_word(Some(s))), cr::state_word(after.as_deref()));
            let Some(inst) = self.workgroups.get(&instance) else { continue };
            let reviews = inst.workgroup.sessions.iter().filter(|s| s.mode == Mode::CodeReview).count();
            let mode = inst.config_of(*pane).map(str::to_string).or_else(|| inst.non_peers.iter().find(|(_, p)| p == pane).map(|(c, _)| c.clone())).and_then(|c| inst.workgroup.session(&c)).map(|s| s.mode);
            let is_main = inst.leader == *pane;
            let (auto_request, auto_send) = self.panes.get(pane).map_or((false, false), |p| (p.auto_request_review, p.auto_send_clippings));
            if is_main && cr::should_auto_request(prev, now, auto_request, reviews) {
                self.request_review(&instance);
            }
            if mode == Some(Mode::CodeReview) && !is_main {
                self.auto_send(&instance, prev, now, auto_send);
            }
        }
    }

    /// The main session's foreground job, as the job watcher reports it (a framing session's remote job
    /// included), with the command shell integration says is running there (text and mark).
    fn foreground(&self, pane: PaneId) -> Option<cr::Foreground> {
        let s = self.panes.get(&pane)?.own();
        let j = s.shell.job.as_ref()?;
        let running = s.command_running().then(|| s.last_command_mark().map(|m| (m.command.clone(), m.id))).flatten();
        Some(cr::Foreground { name: j.name.clone(), cmdline: j.cmdline.clone(), pid: j.pid, running })
    }

    /// What an auto-send of `bytes` (0 for its Return) into `main` for `instance` is judged on (B6): the
    /// pane's state, and whether the coding agent the review was started for is still in front of it.
    pub(super) fn review_send_target(&self, instance: &str, main: PaneId, bytes: usize) -> cr::SendTarget {
        let job = self.foreground(main);
        let started_for = self.workgroups.get(instance).and_then(|i| i.review.agent.as_ref());
        cr::SendTarget {
            alive: self.panes.get(&main).is_some_and(|p| p.exited.is_none() && p.replay.is_none()),
            password: self.panes.get(&main).is_some_and(|p| p.password_input),
            bracketed: self.paste_bracketed_pub(main),
            bytes,
            is_main: self.panes.get(&main).is_some_and(|p| p.workgroup.as_deref() == Some(instance)),
            shell: crate::session_state::is_shell(job.as_ref().map(|j| j.name.as_str())),
            agent: cr::agent_present(started_for, job.as_ref()),
        }
    }

    /// The auto-send (`maybeAutoSendClippings`): the group's clippings, pasted bracketed into the main
    /// session through the paste path, a lone Return after the delay measured from the end of the paste,
    /// the clippings archived, the main session shown. Refused (and said) unless the main session asked for
    /// bracketed paste, is alive, is not at a password prompt and still runs the coding agent the review was
    /// started for (never a shell prompt); the Return is judged again before it is written (B6).
    fn auto_send(&mut self, instance: &str, prev: Option<&str>, now: &str, toggle: bool) {
        let Some(inst) = self.workgroups.get(instance) else { return };
        let main = inst.leader;
        let busy = self.panes.get(&main).is_some_and(|p| crate::session_state::reported_state(&p.own().tab_status) == crate::session_state::State::Working);
        let home = self.clip_home(main);
        let separator = self.globals.string("ClippingSeparator");
        let list: Vec<String> = self.clips.get(&home).map(|c| c.model.live.iter().map(|k| crate::clippings::format(&[k], "")).collect()).unwrap_or_default();
        let Some(text) = cr::clippings_to_send(prev, now, Mode::CodeReview, toggle, busy, &list, &separator) else { return };
        if let Some(why) = cr::send_refusal(&self.review_send_target(instance, main, text.len())) {
            self.banners.set("workgroup", why);
            return;
        }
        self.paste_auto(main, cr::auto_job(&text));
        let delay = cr::submit_delay(self.globals.get("WorkgroupAutoSendSubmitDelay").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.501));
        self.paste_literal_after(main, "\r", delay, instance);
        // The sent clippings are history now: this is what ends the request and send loop. The archive
        // happens before the main session takes the group's slot (the clippings travel with it).
        if let Some(c) = self.clips.get_mut(&home) {
            if c.model.archive_all() {
                c.selected.clear();
                c.scroll = 0.0;
            }
        }
        self.mark_state_dirty();
        self.reveal_as_peer(main);
        self.redraw();
    }
}

// --- Files ---

/// `%LOCALAPPDATA%\Termurion\tmp`, judged and made; the files an earlier run left are swept once per process.
fn review_tmp_dir() -> Option<crate::localfs::LocalPath> {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    let local = std::env::var("LOCALAPPDATA").ok().filter(|t| !t.is_empty())?;
    let dir = crate::localfs::LocalPath::judge(local)?.join("Termurion")?.join("tmp")?;
    crate::localfs::create_dir_all(&dir).ok()?;
    SWEPT.call_once(|| cr::sweep(&dir));
    Some(dir)
}

/// The resources folder the review settings file lives in: `<tmp>\..\resources`, the file rewritten when it
/// differs and made owner-only. Returns the folder (`\(iterm2.appBundlePath)/Contents/Resources`).
fn write_review_settings(tmp: &crate::localfs::LocalPath) -> Result<std::path::PathBuf, String> {
    let bad = || "Tandem could not write the review's settings file, so the review did not start.".to_string();
    let dir = tmp.parent().and_then(|p| p.join("resources")).ok_or_else(bad)?;
    crate::localfs::create_dir_all(&dir).map_err(|_| bad())?;
    let entry = dir.unfollowed("code-review-settings.txt").ok_or_else(bad)?;
    let want = include_str!("../../resources/code-review-settings.txt");
    if dir.join("code-review-settings.txt").and_then(|p| crate::localfs::read(&p).ok()).is_some_and(|b| b == want.as_bytes()) {
        return Ok(dir.path().to_path_buf());
    }
    #[cfg(windows)]
    {
        use std::io::Write;
        let _ = crate::localfs::remove_unfollowed(&entry);
        let mut f = crate::localfs::create_owner_only(&entry).map_err(|_| bad())?;
        f.write_all(want.as_bytes()).and_then(|()| f.sync_all()).map_err(|_| bad())?;
    }
    Ok(dir.path().to_path_buf())
}

/// Removes a review file this run made (judged: it is in the tmp folder).
fn remove_review_file(path: &std::path::Path) {
    if let Some(dir) = path.parent().and_then(crate::localfs::LocalPath::judge) {
        if let Some(entry) = path.file_name().and_then(|n| dir.unfollowed(n)) {
            let _ = crate::localfs::remove_unfollowed(&entry);
        }
    }
}

#[cfg(test)]
mod tests {
    const THIS: &str = include_str!("code_review_app.rs");

    /// 13y1 Task 7 item 3 (B6): the auto-send's paste and its Return go to the main session only, never to
    /// its broadcast domain, so the checks made on the main session are the checks on every target.
    #[test]
    fn auto_send_writes_only_to_the_main_session() {
        use crate::broadcast::{self, Broadcast, Mode, Route, TabView, View};
        use std::collections::BTreeSet;
        let what = super::super::paste_ui::AUTO_WRITE;
        // Broadcast input is on for the main session's tab: the person's typing reaches every pane, the
        // auto-send's paste and Return reach the main session only.
        let b = Broadcast { raw: Mode::AllPanes, ..Default::default() };
        let v = View { tabs: vec![TabView { broadcasting: true, panes: vec![7, 8, 9] }], current_tab: 0, current: 7, exited: BTreeSet::new() };
        assert_eq!(broadcast::write_targets(&b, &v, 7, crate::broadcast::Write::Paste, &BTreeSet::new()), vec![7, 8, 9], "the control: broadcasting is on");
        assert_eq!(broadcast::write_targets(&b, &v, 7, what, &BTreeSet::new()), vec![7], "the auto-send goes to the main session alone");
        assert_eq!(broadcast::route(what), Route::Local);
        // Both writes of the auto-send use it, and a queue that holds one stays local until it drains.
        let paste = include_str!("paste_ui.rs");
        let auto = &paste[paste.find("pub(super) fn paste_auto").unwrap()..paste.find("pub(super) fn paste_literal_after").unwrap()];
        assert!(auto.contains("AUTO_WRITE") && !auto.contains("Write::Paste"), "{auto}");
        let tick = &paste[paste.find("fn paste_after_tick").unwrap()..paste.find("pub(super) fn paste_tick").unwrap()];
        assert!(tick.contains("self.user_write(l.pane, l.text.as_bytes(), AUTO_WRITE)") && !tick.contains("Write::Paste"), "{tick}");
        let run = &paste[paste.find("fn paste_run").unwrap()..paste.find("pub(super) fn paste_auto").unwrap()];
        assert!(run.contains("if broadcast::route(entry.1) != broadcast::Route::Local {"), "a person's paste joining the queue never makes the auto-send broadcast");
    }

    /// The auto-send goes only through the paste path, after the refusals, and its Return is a timer that
    /// belongs to the instance (B6).
    #[test]
    fn auto_send_return_cancelled_with_the_instance() {
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        let send = &live[live.find("fn auto_send(").unwrap()..live.find("// --- Files ---").unwrap()];
        assert!(send.find("cr::send_refusal(").unwrap() < send.find("self.paste_auto(").unwrap(), "refusals first");
        assert!(send.find("self.paste_auto(").unwrap() < send.find("self.paste_literal_after(").unwrap());
        assert!(send.find("archive_all()").unwrap() < send.find("self.reveal_as_peer(main)").unwrap(), "archive before the clippings travel with the slot");
        let app = include_str!("workgroup_app.rs");
        let teardown = &app[app.find("fn teardown_workgroup").unwrap()..app.find("/// Ends the members a teardown queued").unwrap()];
        assert!(teardown.contains("code_review_teardown("), "a teardown cancels the Return and removes the files");
        let paste = include_str!("paste_ui.rs");
        assert!(paste.contains("self.workgroups.get(&l.owner).is_some()") && paste.contains("p.workgroup.as_deref() == Some(l.owner.as_str())"), "a Return never fires into a pane that is no longer the instance's");
        assert!(live.contains("self.cancel_paste_after(&i)"), "turning the toggle off cancels a waiting Return");
    }

    /// Every review launch is argv from the person's template, the system prompt an owner-only file, and
    /// the command is one the entry approved (checked by `workgroup_launch` before `review_launch`).
    #[test]
    fn a_review_starts_from_an_argument_vector() {
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        let f = &live[live.find("fn review_command(").unwrap()..live.find("/// The member's system prompt file").unwrap()];
        for needed in ["cr::review_argv(", "argv_line(", "cr::line_fits(", "cr::review_program(", "write_review_file("] {
            assert!(f.contains(needed), "{needed}");
        }
        assert!(!live.contains("format!(\"claude"), "no hand-built command line");
        let app = include_str!("workgroup_app.rs");
        let launch = &app[app.find("pub(super) fn workgroup_launch").unwrap()..];
        let launch = &launch[..launch.find("\n    }\n").unwrap()];
        assert!(launch.find("approved.allows(").unwrap() < launch.find("review_launch(").unwrap(), "the approval comes first");
    }

    /// The overlay is drawn in the popups layer over a cut-out, and its keys let Alt and Win chords through.
    #[test]
    fn the_overlay_follows_the_layering_rules() {
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        assert!(live.contains("scene::cut_out(frame, panel, cell.1)") && live.contains("frame.popups.push"));
        assert!(live.contains("crate::text_edit::takes(ctrl, alt, mods.cmd, &key)"));
        let m = include_str!("mod.rs");
        assert!(m.contains("self.draw_review_overlays(&mut frame, cell);") && m.contains("self.review_overlay_press(button, x, y)") && m.contains("self.review_overlay_key(ev)"));
    }

    #[test]
    fn the_overlay_has_every_part_inside_its_panel() {
        use super::*;
        let panel = Rect { x: 100.0, y: 50.0, w: 700.0, h: 400.0 };
        let g = geom(panel, (8.0, 16.0), 1.0);
        for r in [g.title, g.field, g.prompts, g.start] {
            assert!(r.x >= panel.x && r.y >= panel.y && r.x + r.w <= panel.x + panel.w && r.y + r.h <= panel.y + panel.h, "{r:?}");
        }
        assert!(g.prompts.x + g.prompts.w < g.start.x, "the pulldown is left of Start");
        assert!(g.field.y + g.field.h <= g.start.y, "the field is above the buttons");
    }
}
