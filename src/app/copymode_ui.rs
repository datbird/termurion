//! Copy Mode in the window: keys go to the Copy Mode engine, the selection and view follow
//! its cursor, and the cursor draws as iTerm2's flag.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use super::find_ui::{to_point, to_pos};
use crate::copymode::{self, Kind, Outcome, State};

/// The Copy Mode engine's name for a key: the character typed, "C-x" for Ctrl with a
/// letter, or the key's name. None for keys Copy Mode has no use for (they are dropped).
pub(super) fn key_name(key: &WKey, text: Option<&str>, mods: crate::mouse::Mods) -> Option<String> {
    let name = match key {
        WKey::Named(NamedKey::Space) if mods.ctrl => "C-Space",
        WKey::Named(NamedKey::Space) => " ",
        WKey::Named(NamedKey::Escape) => "Esc",
        WKey::Named(NamedKey::ArrowLeft) => "Left",
        WKey::Named(NamedKey::ArrowRight) => "Right",
        WKey::Named(NamedKey::ArrowUp) => "Up",
        WKey::Named(NamedKey::ArrowDown) => "Down",
        WKey::Named(NamedKey::Home) => "Home",
        WKey::Named(NamedKey::End) => "End",
        WKey::Named(NamedKey::PageUp) => "PageUp",
        WKey::Named(NamedKey::PageDown) => "PageDown",
        WKey::Named(_) => return None,
        WKey::Character(c) if mods.ctrl && !mods.alt => return Some(format!("C-{}", c.to_lowercase())),
        _ => return text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)).map(str::to_string),
    };
    Some(name.to_string())
}

impl App {
    /// Edit > Copy Mode and key action 73. Returns false for other selectors.
    pub(super) fn run_copymode_command(&mut self, selector: &str) -> bool {
        if selector != "toggleCopyMode:" {
            return false;
        }
        let Some(id) = self.focused_id() else { return true };
        if self.is_browser_pane(id) {
            // Copy mode of a web page (`PS:1569-1583`); the terminal's `copy_modes` stays terminal-only.
            self.browser_copy_mode_toggle(id);
            return true;
        }
        if self.copy_modes.remove(&id).is_none() {
            self.enter_copy_mode(id);
        }
        self.composer_follow_copy_mode(id);
        self.sync_checks();
        self.redraw();
        true
    }

    /// Copy Mode ends in pane `id`, if on (an Instant Replay frame shown, `PS:15314-15375`).
    pub(super) fn leave_copy_mode(&mut self, id: PaneId) {
        if self.copy_modes.remove(&id).is_some() {
            self.composer_follow_copy_mode(id);
            self.sync_checks();
        }
    }

    /// Copy Mode starts at the selection's end, else at the terminal cursor.
    fn enter_copy_mode(&mut self, id: PaneId) {
        let Some(p) = self.panes.get(&id) else { return };
        let term = p.screen.term();
        let at = match term.selection.as_ref().and_then(|s| s.to_range(term)) {
            Some(r) => to_pos(&p.screen, r.end),
            None => to_pos(&p.screen, term.grid().cursor.point),
        };
        self.copy_modes.insert(id, State::new(at));
        self.belt_yield_keys();
        self.composer_follow_copy_mode(id);
    }

    /// A key while Copy Mode is on in the focused pane, or a Shift+arrow that starts it.
    /// Returns true when Copy Mode took the key.
    pub(super) fn copy_mode_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let Some(id) = self.focused_id() else { return false };
        let name = key_name(&ev.logical_key, ev.text.as_deref(), self.mods);
        if !self.copy_modes.contains_key(&id) {
            let m = self.mods;
            let backward = name.as_deref().and_then(|n| copymode::auto_enter(n, m.shift, m.alt, m.ctrl, m.cmd));
            let selection = self.panes.get(&id).and_then(|p| {
                let term = p.screen.term();
                term.selection.as_ref().and_then(|s| s.to_range(term)).map(|r| (to_pos(&p.screen, r.start), to_pos(&p.screen, r.end)))
            });
            let (Some(backward), Some((a, b)), true) = (backward, selection, self.globals.bool("EnterCopyModeAutomatically")) else {
                return false;
            };
            let mut state = State::new(if backward { a } else { b });
            state.anchor = Some(if backward { b } else { a });
            self.copy_modes.insert(id, state);
            self.belt_yield_keys();
            self.composer_follow_copy_mode(id);
            self.sync_checks();
        }
        if let Some(n) = name {
            self.copy_mode_keys(id, &[n]);
        }
        true
    }

    /// Key action 73: runs a vim-style key string through Copy Mode, then leaves Copy
    /// Mode as it was.
    pub(super) fn copy_mode_commands(&mut self, keys: &str) {
        if let Some(id) = self.focused_id() {
            self.copy_mode_commands_in(id, keys);
        }
    }

    pub(super) fn copy_mode_commands_in(&mut self, id: PaneId, keys: &str) {
        if self.is_browser_pane(id) {
            return self.browser_copy_mode_commands(id, keys);
        }
        let keys = match copymode::parse_vim_keys(keys) {
            Ok(k) => k,
            Err(e) => return self.banners.set("action", e),
        };
        let was_on = self.copy_modes.contains_key(&id);
        if !was_on {
            self.enter_copy_mode(id);
        }
        self.copy_mode_keys(id, &keys);
        if !was_on {
            self.copy_modes.remove(&id);
        }
        self.composer_follow_copy_mode(id);
        self.redraw();
    }

    fn copy_mode_keys(&mut self, id: PaneId, keys: &[String]) {
        for key in keys {
            let (Some(mut state), Some(p)) = (self.copy_modes.remove(&id), self.panes.get_mut(&id)) else { return };
            let outcome = {
                let grid = p.screen.term().grid();
                let (hist, cols, lines) = (grid.history_size(), grid.columns(), grid.screen_lines());
                let line = |r: usize| {
                    let l = Line(r as i32 - hist as i32);
                    (0..cols).map(|c| grid[l][Column(c)].c).collect::<Vec<char>>()
                };
                let mut marks: Vec<usize> = p.screen.row_marks().iter().map(|(l, _)| (l + hist as i32) as usize).collect();
                marks.dedup();
                let ctx = copymode::Ctx { rows: hist + lines, cols, view_top: hist - grid.display_offset(), view_rows: lines, marks: &marks, line: &line };
                state.key(key, &ctx)
            };
            match outcome {
                Outcome::None => {}
                Outcome::Moved => {
                    let mode = match state.selection() {
                        Some((_, _, Kind::Line)) => crate::seltext::Mode::Line,
                        Some((_, _, Kind::Box)) => crate::seltext::Mode::Box,
                        _ => crate::seltext::Mode::Character,
                    };
                    let sel = state.selection().map(|(a, b, kind)| {
                        let ty = match kind {
                            Kind::Char => SelectionType::Simple,
                            Kind::Line => SelectionType::Lines,
                            Kind::Box => SelectionType::Block,
                        };
                        let mut sel = Selection::new(ty, to_point(&p.screen, a), Side::Left);
                        sel.update(to_point(&p.screen, b), Side::Right);
                        sel
                    });
                    p.set_selection(sel, mode);
                    let line = to_point(&p.screen, state.cursor).line.0;
                    super::find_ui::show_line(p, line);
                }
                Outcome::Scroll(n) => p.scroll(Scroll::Delta(-n)),
                Outcome::Copy => {
                    self.copy_selection(id);
                    self.sync_checks();
                    continue;
                }
                Outcome::Exit { clear } => {
                    if clear {
                        p.clear_selection();
                    }
                    self.sync_checks();
                    continue;
                }
                Outcome::Find => {
                    self.copy_modes.insert(id, state);
                    self.run_find_command("performFindPanelAction:", "Find...");
                    continue;
                }
            }
            self.copy_modes.insert(id, state);
        }
        // Copy Mode may have ended (y, Esc, q): the composer comes back.
        self.composer_follow_copy_mode(id);
        self.redraw();
    }

    /// Where the Copy Mode cursor of a pane is on screen, if it is.
    pub(super) fn copy_cursor_on_screen(&self, id: PaneId) -> Option<(usize, usize, bool)> {
        let state = self.copy_modes.get(&id)?;
        let p = self.panes.get(&id)?;
        let pt: Point = to_point(&p.screen, state.cursor);
        let grid = p.screen.term().grid();
        let row = pt.line.0 + grid.display_offset() as i32;
        (row >= 0 && (row as usize) < grid.screen_lines()).then_some((row as usize, state.cursor.col, state.anchor.is_some()))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::Mods;

    #[test]
    fn copy_keys_name_themselves() {
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..none };
        let c = |s: &str| WKey::Character(s.into());
        assert_eq!(key_name(&c("j"), Some("j"), none).as_deref(), Some("j"));
        assert_eq!(key_name(&c("V"), Some("V"), Mods { shift: true, ..none }).as_deref(), Some("V"));
        assert_eq!(key_name(&c("b"), Some("\\u{2}"), ctrl).as_deref(), Some("C-b"));
        assert_eq!(key_name(&WKey::Named(NamedKey::Space), Some(" "), ctrl).as_deref(), Some("C-Space"));
        assert_eq!(key_name(&WKey::Named(NamedKey::Space), Some(" "), none).as_deref(), Some(" "));
        assert_eq!(key_name(&WKey::Named(NamedKey::Escape), None, none).as_deref(), Some("Esc"));
        assert_eq!(key_name(&WKey::Named(NamedKey::ArrowLeft), None, none).as_deref(), Some("Left"));
        assert_eq!(key_name(&WKey::Named(NamedKey::PageDown), None, none).as_deref(), Some("PageDown"));
        assert_eq!(key_name(&WKey::Named(NamedKey::F5), None, none), None);
    }
}
