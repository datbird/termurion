//! The mark commands (Edit > Marks and Annotations, Clear to Last Mark, the two Select
//! commands), mark alerts, and the Set Named Mark prompt.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;

use super::App;
use crate::layout::PaneId;
use crate::browser_marks::AddPath;
use crate::marks;
use crate::prompt_ui::{PromptResult, PromptWindow};

/// What the one Set Named Mark window is for (review M9): a terminal pane's add (`Terminal(None)`) or
/// rename (`Terminal(Some(mark))`), or a browser pane's request.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PromptFor {
    Terminal(Option<u32>),
    Browser(super::browser_marks_app::MarkPrompt),
}

/// Mark commands this module runs.
pub(super) const SELECTORS: [&str; 13] = [
    "saveScrollPosition:",
    "addNamedMark:",
    "jumpToSavedScrollPosition:",
    "nextMark:",
    "previousMark:",
    "clearToLastMark:",
    "selectOutputOfLastCommand:",
    "selectCurrentCommand:",
    "toggleAlertOnNextMark:",
    "toggleAlertOnMarksInOffscreenSessions:",
    "enableMarkAlertShowsModalAlert:",
    "enableMarkAlertPostsNotification:",
    "enableMarkAlertRingsBell:",
];

/// Menu items with a check mark: (selector, Mark Alert Action value or "").
pub(super) const CHECKS: [(&str, &str); 5] = [
    ("toggleAlertOnNextMark:", ""),
    ("toggleAlertOnMarksInOffscreenSessions:", ""),
    ("enableMarkAlertShowsModalAlert:", "Modal Alert"),
    ("enableMarkAlertPostsNotification:", "Post Notification"),
    ("enableMarkAlertRingsBell:", "Ring Bell"),
];

/// Menu ids of a browser session's marks: this prefix and the mark's guid.
pub(super) const BROWSER_MARK_ID: &str = "termurion.browsermark.";

/// Most browser marks Edit > Named Marks lists (the cache can hold a million).
const BROWSER_MENU_MARKS: usize = 200;

/// Edit > Named Marks for a browser session: each cached mark's guid and menu text, cache order kept
/// (`AD:4310-4316`), "Unnamed Mark" for an empty name, names cleaned by Termurion's sanitizer and capped.
pub(super) fn browser_mark_items(marks: &[crate::browser_marks::Mark]) -> Vec<(String, String)> {
    marks
        .iter()
        .take(BROWSER_MENU_MARKS)
        .map(|m| {
            let name = crate::browser_marks::rename_text(&m.name);
            (m.guid.clone(), if name.trim().is_empty() { "Unnamed Mark".to_string() } else { name })
        })
        .collect()
}

/// Edit > Named Marks: each named mark's id and menu text, top first ("Unnamed Mark" for
/// an empty name).
pub(super) fn named_marks(marks: &[(i32, crate::marks::Mark)]) -> Vec<(u32, String)> {
    let mut named: Vec<&(i32, crate::marks::Mark)> = marks.iter().filter(|(_, m)| m.name.is_some()).collect();
    named.sort_by_key(|(l, _)| *l);
    named
        .into_iter()
        .map(|(_, m)| {
            let name = m.name.clone().unwrap_or_default();
            (m.id, if name.is_empty() { "Unnamed Mark".to_string() } else { name })
        })
        .collect()
}

/// Menu item ids of Edit > Named Marks.
pub(super) const NAMED_MARK_ID: &str = "termurion.namedmark.";

/// The display offset that puts grid line `line` at the top of the view (or centers it).
pub(super) fn offset_for(line: i32, rows: usize, history: usize, center: bool) -> usize {
    let top = if center { line - rows as i32 / 2 } else { line };
    (-top).clamp(0, history as i32) as usize
}

impl App {
    fn focused_id_for_marks(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Scrolls pane `id` so grid line `line` is at the top (or centered).
    pub(super) fn show_line(&mut self, id: PaneId, line: i32, center: bool) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let grid = p.screen.term().grid();
        let want = offset_for(line, grid.screen_lines(), grid.history_size(), center) as i32;
        let now = grid.display_offset() as i32;
        p.scroll(Scroll::Delta(want - now));
    }

    pub(super) fn select(&mut self, id: PaneId, (a, b): (Point, Point)) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
        sel.update(b, Side::Right);
        p.set_selection(Some(sel), crate::seltext::Mode::Character);
    }

    fn top_visible_line(&self, id: PaneId) -> i32 {
        self.panes.get(&id).map_or(0, |p| -(p.screen.term().grid().display_offset() as i32))
    }

    pub(super) fn step_mark(&mut self, id: PaneId, forward: bool) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let marks = p.screen.marks_in_order();
        let lines: Vec<i32> = marks.iter().map(|(l, _)| *l).collect();
        let from = self.mark_pos.get(&id).and_then(|m| marks.iter().find(|(_, x)| x.id == *m)).map(|(l, _)| *l);
        let Some(line) = marks::next_mark(&lines, from, forward) else { return };
        let mark = marks.iter().find(|(l, _)| *l == line).map(|(_, m)| m.clone()).unwrap();
        self.mark_pos.insert(id, mark.id);
        if mark.command.is_empty() {
            self.show_line(id, line, true);
        } else {
            // A command mark goes to the top, with its command selected.
            self.show_line(id, line, false);
            let range = mark.command_range.and_then(|(a, b)| {
                let p = self.panes.get(&id)?;
                Some((p.screen.resolve(a)?, p.screen.resolve(b)?))
            });
            if let Some((a, b)) = range {
                let b = if b.column.0 > 0 { Point::new(b.line, b.column - 1) } else { b };
                self.select(id, (a, b));
            }
        }
    }

    /// Runs one mark command. False when `selector` is not one.
    pub(super) fn run_mark_command(&mut self, selector: &str, event_loop: &ActiveEventLoop) -> bool {
        if !SELECTORS.contains(&selector) {
            return false;
        }
        let Some(id) = self.focused_id_for_marks() else { return true };
        // On a browser pane, Edit > Mark and Set Named Mark both name a spot on the page
        // (`PS:16755-16771`, `PT:2650-2661`); the Edit menu's gate applies.
        if matches!(selector, "saveScrollPosition:" | "addNamedMark:") && self.is_browser_pane(id) {
            self.browser_add_named_mark(id, None, AddPath::Menu);
            return true;
        }
        match selector {
            "saveScrollPosition:" => {
                let line = self.top_visible_line(id);
                if let Some(p) = self.panes.get_mut(&id) {
                    let m = p.screen.set_manual_mark(line, None);
                    self.mark_pos.insert(id, m);
                }
            }
            "addNamedMark:" => self.open_named_mark_prompt(id, event_loop),
            "jumpToSavedScrollPosition:" => {
                let last = self.panes.get_mut(&id).and_then(|p| p.screen.marks_in_order().last().map(|(l, m)| (*l, m.id)));
                if let Some((line, m)) = last {
                    self.mark_pos.insert(id, m);
                    self.show_line(id, line, false);
                }
            }
            "nextMark:" => self.step_mark(id, true),
            "previousMark:" => self.step_mark(id, false),
            "clearToLastMark:" => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.clear_to_last_mark();
                    p.scroll(Scroll::Bottom);
                }
            }
            "selectOutputOfLastCommand:" => {
                if let Some(r) = self.panes.get(&id).and_then(|p| p.screen.last_output_range()) {
                    self.select(id, r);
                    self.copy_on_selection(id);
                }
            }
            "selectCurrentCommand:" => {
                if let Some(r) = self.panes.get(&id).and_then(|p| p.screen.last_command_range()) {
                    self.select(id, r);
                    self.copy_on_selection(id);
                }
            }
            "toggleAlertOnNextMark:" => {
                if let Some(p) = self.panes.get_mut(&id) {
                    let marks = &mut p.own_mut().marks;
                    marks.alert_next = !marks.alert_next;
                }
            }
            "toggleAlertOnMarksInOffscreenSessions:" => {
                let now = self.globals.bool("Alert On Marks in Offscreen Sessions");
                self.globals.set("Alert On Marks in Offscreen Sessions", toml::Value::Boolean(!now));
                self.save_globals();
            }
            other => {
                if let Some((_, value)) = CHECKS.iter().find(|(s, v)| *s == other && !v.is_empty()) {
                    self.globals.set("Mark Alert Action", toml::Value::String(value.to_string()));
                    self.save_globals();
                }
            }
        }
        true
    }

    /// Add Named Mark's prompt, named with the time.
    pub(super) fn open_named_mark_prompt(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        self.open_mark_prompt(id, None, event_loop);
    }

    /// The same prompt renaming mark `mark`, its name filled in (the toolbelt's Edit,
    /// `PT:15741-15752`).
    pub(super) fn open_rename_mark_prompt(&mut self, id: PaneId, mark: u32, name: &str, event_loop: &ActiveEventLoop) {
        self.open_mark_prompt(id, Some((mark, name)), event_loop);
    }

    fn open_mark_prompt(&mut self, id: PaneId, rename: Option<(u32, &str)>, event_loop: &ActiveEventLoop) {
        if let Some((w, ..)) = &self.prompt {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let stamp = crate::prompt_ui::local_stamp();
        let text = rename.map_or(stamp.as_str(), |(_, name)| name);
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Set Named Mark", "Enter Mark Name", text) {
            Ok(w) => self.prompt = Some((w, id, PromptFor::Terminal(rename.map(|(m, _)| m)))),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Handles an event for the Set Named Mark window. False when `window` is not it.
    pub(super) fn prompt_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let guarded = self.mark_input_guarded(event);
        let Some((w, pane, target)) = self.prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let (pane, target) = (*pane, target.clone());
        if matches!(target, PromptFor::Browser(_)) {
            super::browser_marks_app::mark_window_refocused(&mut self.mark_guard, event);
        }
        match event {
            WindowEvent::CloseRequested => self.prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.prompt = None,
                PromptResult::Ok(name) => {
                    self.prompt = None;
                    // An empty name cancels, as in iTerm2 (`BookmarkDialogViewController.swift:33-37`).
                    if !name.trim().is_empty() {
                        let rename = match target {
                            PromptFor::Terminal(rename) => rename,
                            PromptFor::Browser(prompt) => return { self.browser_mark_prompt_done(pane, prompt, name); true },
                        };
                        match rename {
                            // The note "Named Mark <name>" follows the name (`PS:16775-16822`).
                            Some(mark) => {
                                if let Some(p) = self.panes.get_mut(&pane) {
                                    p.screen.rename_named_mark(mark, Some(name));
                                }
                            }
                            None => {
                                let line = self.top_visible_line(pane);
                                if let Some(p) = self.panes.get_mut(&pane) {
                                    let m = p.screen.set_named_mark(line, &name);
                                    self.mark_pos.insert(pane, m);
                                }
                            }
                        }
                        self.redraw();
                    }
                }
            },
            other => {
                // A browser mark window just opened takes no key or click yet.
                if !(guarded && matches!(target, PromptFor::Browser(_))) {
                    w.event(other);
                }
            }
        }
        true
    }

    /// After new output: a new mark resets mark navigation, and alerts when asked to.
    pub(super) fn check_marks(&mut self) {
        // The composer's queue: one command per new mark (`PS:16613-16617`), then what the
        // prompt state machine asks of the Auto Composer.
        self.drain_composer_queues();
        self.check_composer_events();
        let active: Vec<PaneId> = self.tabs.get(self.active).map(|t| t.layout.panes()).unwrap_or_default();
        let offscreen = self.globals.bool("Alert On Marks in Offscreen Sessions");
        let mut alerts = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            let screen = p.own_mut();
            if !screen.take_new_mark() {
                continue;
            }
            if let Some((_, m)) = screen.marks_in_order().last() {
                self.mark_pos.insert(*id, m.id);
            }
            if std::mem::take(&mut screen.marks.alert_next) || (offscreen && !active.contains(id)) {
                let title = screen.title().unwrap_or("Shell").to_string();
                alerts.push((title, screen.previous_code().unwrap_or(0) != 0));
            }
        }
        for (title, failed) in alerts {
            self.mark_alert(&title, failed);
        }
    }

    fn mark_alert(&mut self, title: &str, failed: bool) {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONHAND, MB_OK};
        match self.globals.string("Mark Alert Action").as_str() {
            "Modal Alert" => super::menu::info_box(&format!("Mark set in session \"{title}\".")),
            "Ring Bell" => {
                // The named Windows sound through the one judge, else the system beeps (the
                // stand-ins for iTerm2's bundled mp3s, `PTYSession.m:16657-16667`); never more
                // often than the beep floor.
                let named = crate::bell_badge::sound_for(failed, &self.globals.string("SuccessSound"), &self.globals.string("ErrorSound"));
                if self.bell_limiter.beep(std::time::Instant::now()) {
                    let outcome = named.and_then(|n| crate::taskbar_win::play_named(&n));
                    if crate::bell_badge::beeps_after(outcome) {
                        unsafe {
                            MessageBeep(if failed { MB_ICONHAND } else { MB_OK });
                        }
                    }
                }
            }
            _ => {
                let result = self.hwnd().ok_or_else(|| "no window".to_string()).and_then(|h| crate::notify::show(h, "Mark Set", &format!("Session {title} had a mark set.")));
                if let Err(e) = result {
                    self.banners.set("notify", format!("Notification failed: {e}"));
                }
            }
        }
    }
}

impl App {
    /// Rebuilds Edit > Named Marks from the focused pane's named marks, when they changed.
    pub(super) fn refresh_named_marks(&mut self) {
        // The buffer scan runs only when the focused pane or its named marks changed.
        let focused = self.focused_id();
        // A browser session lists its store's whole cache; the key is the rows themselves.
        let browser: Option<Vec<(String, String)>> = focused.filter(|&id| self.is_browser_pane(id)).map(|id| {
            let dev_null = self.panes.get(&id).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
            browser_mark_items(self.browser_marks_cache(dev_null))
        });
        let key: (Option<PaneId>, Vec<(u32, Option<String>)>) = (
            focused,
            match &browser {
                Some(rows) => rows.iter().enumerate().map(|(i, (g, t))| (i as u32, Some(format!("{g}\u{0}{t}")))).collect(),
                None => focused
                    .and_then(|id| self.panes.get(&id))
                    .map(|p| p.screen.marks.records.values().filter(|m| m.name.is_some()).map(|m| (m.id, m.name.clone())).collect())
                    .unwrap_or_default(),
            },
        );
        if self.menu_bar.as_ref().is_some_and(|b| b.named_key == key) {
            return;
        }
        let browser_rows = browser.unwrap_or_default();
        let browser_pane = !browser_rows.is_empty() || focused.is_some_and(|id| self.is_browser_pane(id));
        let items = if browser_pane { Vec::new() } else { focused.and_then(|id| self.panes.get_mut(&id)).map(|p| named_marks(&p.screen.marks_in_order())).unwrap_or_default() };
        let Some(bar) = &mut self.menu_bar else { return };
        bar.named_key = key;
        if bar.named_items == items && bar.named_browser == browser_rows {
            return;
        }
        let Some(sub) = &bar.named else { return };
        while sub.remove_at(0).is_some() {}
        for (id, label) in &items {
            let entry = muda::MenuItem::with_id(format!("{NAMED_MARK_ID}{id}"), label.replace('&', "&&"), true, None);
            if let Err(e) = sub.append(&entry) {
                self.banners.set("menu", format!("Named Marks menu: {e}"));
            }
        }
        for (guid, label) in &browser_rows {
            let entry = muda::MenuItem::with_id(format!("{BROWSER_MARK_ID}{guid}"), label.replace('&', "&&"), true, None);
            if let Err(e) = sub.append(&entry) {
                self.banners.set("menu", format!("Named Marks menu: {e}"));
            }
        }
        bar.named_items = items;
        bar.named_browser = browser_rows;
    }

    /// A pick from Edit > Named Marks. Returns false for other menu ids.
    pub(super) fn named_mark_picked(&mut self, menu_id: &str) -> bool {
        if let Some(guid) = menu_id.strip_prefix(BROWSER_MARK_ID) {
            // The only way in is Task 5's reveal (load gate, epoch-bound); a terminal pane ignores it.
            if let Some(id) = self.focused_id().filter(|&id| self.is_browser_pane(id)) {
                self.browser_reveal_named_mark(id, guid);
            }
            return true;
        }
        let Some(mark) = menu_id.strip_prefix(NAMED_MARK_ID).and_then(|n| n.parse::<u32>().ok()) else { return false };
        let Some(id) = self.focused_id() else { return true };
        let line = self.panes.get_mut(&id).and_then(|p| p.screen.marks_in_order().into_iter().find(|(_, m)| m.id == mark).map(|(l, _)| l));
        if let Some(line) = line {
            self.show_line(id, line, false);
            self.mark_pos.insert(id, mark);
        }
        self.redraw();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmark(guid: &str, name: &str) -> crate::browser_marks::Mark {
        crate::browser_marks::Mark { guid: guid.into(), url: "https://a.b/".into(), name: name.into(), sort: 0 }
    }

    #[test]
    fn browser_mark_menu_follows_iterm2() {
        let cache = vec![bmark("g2", ""), bmark("g1", "a")];
        assert_eq!(browser_mark_items(&cache), vec![("g2".to_string(), "Unnamed Mark".to_string()), ("g1".to_string(), "a".to_string())], "cache order kept");
        // Names are cleaned (bidi, controls) and capped; the list is bounded.
        let shown = browser_mark_items(&[bmark("g", "a\u{202e}b\u{7}c")]);
        assert_eq!(shown[0].1, "abc");
        let long = browser_mark_items(&[bmark("g", &"x".repeat(5000))]);
        assert_eq!(long[0].1.chars().count(), crate::browser_marks::MAX_NAME);
        let many: Vec<_> = (0..1000).map(|i| bmark(&format!("g{i}"), "n")).collect();
        assert_eq!(browser_mark_items(&many).len(), BROWSER_MENU_MARKS);
        // A pick reveals only through Task 5's reveal, on a browser pane; the prefixes never collide.
        assert!(!BROWSER_MARK_ID.starts_with(NAMED_MARK_ID) && !NAMED_MARK_ID.starts_with(BROWSER_MARK_ID));
        let src = include_str!("marks_ui.rs");
        let pick = &src[src.find("pub(super) fn named_mark_picked").unwrap()..];
        let pick = &pick[..pick.find("let Some(mark) =").unwrap()];
        assert!(pick.contains("strip_prefix(BROWSER_MARK_ID)") && pick.contains("is_browser_pane(id)") && pick.contains("self.browser_reveal_named_mark(id, guid)"));
    }

    #[test]
    fn named_marks_sort_by_row() {
        use crate::marks::{Kind, Mark};
        let mark = |id: u32, name: Option<&str>| Mark {
            id,
            kind: Kind::Manual,
            command: String::new(),
            code: None,
            start: None,
            end: None,
            aid: None,
            name: name.map(str::to_string),
            command_range: None,
            captured: Vec::new(),
            prompt_end: None,
            output_start: None,
            directory: None,
            host: None,
            user: None,
            line_style: false,
            clear_count: 0,
        };
        let marks = vec![(5, mark(1, Some("b"))), (-3, mark(2, Some(""))), (0, mark(3, None)), (2, mark(4, Some("a")))];
        assert_eq!(named_marks(&marks), vec![(2, "Unnamed Mark".to_string()), (4, "a".to_string()), (1, "b".to_string())]);
    }

    #[test]
    fn marks_scroll_to_the_top_or_the_middle() {
        assert_eq!(offset_for(-30, 20, 100, false), 30);
        assert_eq!(offset_for(-30, 20, 100, true), 40);
        assert_eq!(offset_for(5, 20, 100, false), 0);
        assert_eq!(offset_for(-300, 20, 100, false), 100);
    }
}
