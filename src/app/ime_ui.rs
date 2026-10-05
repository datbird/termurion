//! IME composition (13k Task 9): the person's uncommitted text is kept in the window's IME
//! state, owned by its focused pane, and drawn at the cursor; it reaches anything only when
//! committed, and then as typed text, through the owners a typed key goes to. The candidate
//! window follows the caret.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::Ime;

use crate::ime::{self, Dest, Owners, Step};
use crate::text_edit::EditKey;
use crate::tool_list::{Focus, ListKey};
use crate::toolbelt::ToolId;

impl App {
    /// One `WindowEvent::Ime`. A composition is the focused pane's; a commit is typed input.
    pub(super) fn ime_event(&mut self, ime: Ime, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused_id() else { return };
        match ime::step(&ime) {
            // Windows sends Enabled only when a new entry begins: a cancel stops waiting for
            // the stray commit of the composition it cancelled.
            Step::Begin => self.ime.begin(),
            Step::Clear => {
                if self.ime.preedit.is_some() {
                    self.ime.cancel();
                    self.redraw();
                }
            }
            Step::End => {
                self.ime.end();
                self.redraw();
            }
            Step::Store(p) => {
                self.ime.store(id, p);
                self.redraw();
            }
            Step::Commit(text) => {
                // Only to the pane that owns the composition, while it is still this window's
                // focused pane; after a cancel or a pane switch the text is dropped.
                if self.ime.commit(Some(id), self.focused) == Some(id) {
                    self.ime_type(id, &text, event_loop);
                }
                self.redraw();
            }
        }
    }

    /// Whether a toolbelt tool has the keyboard, as (a text entry, no text entry): the Notes
    /// editor, Codecierge's goal and reply and a tool's search field take typed text; a tool's
    /// list takes the key and does nothing with text. A tool that let go owns nothing.
    fn ime_belt_owner(&self) -> (bool, bool) {
        let Some((tool, focus)) = self.belt.focus else { return (false, false) };
        if !self.belt.visible || !self.belt.split.expanded(tool) {
            return (false, false);
        }
        let text = matches!(tool, ToolId::Notes | ToolId::Codecierge) || focus == Focus::Field;
        (text, !text)
    }

    /// Text into the toolbelt tool that has the keyboard, as its typed `Text` key. False when it
    /// has no text entry now.
    fn ime_tool_text(&mut self, text: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some((tool, _)) = self.belt.focus else { return false };
        match tool {
            ToolId::Notes => self.notes_edit_key(EditKey::Text(text.to_string())),
            ToolId::Codecierge => self.codecierge_edit_key(EditKey::Text(text.to_string())),
            // A search field is one line, as a paste into it is.
            _ => {
                self.tool_field_key(tool, ListKey::Text(text.replace(['\r', '\n'], " ")), event_loop);
                self.redraw();
                true
            }
        }
    }

    /// Committed text is typed text: it goes to whatever owns the keyboard, in `key`'s order; an
    /// owner with no text entry right now (a composer that let go) leaves it to the terminal, as
    /// `to_terminal` writes a key (the view returns to the bottom, a Filter or Zoom view ends, a
    /// frozen or replaying pane takes nothing).
    fn ime_type(&mut self, id: PaneId, text: &str, event_loop: &ActiveEventLoop) {
        let (composer, field) = self.composer_keys_owner();
        let (tool, belt) = self.ime_belt_owner();
        let owners = Owners {
            tool,
            belt,
            composer,
            field,
            inline: self.inline_has_keys(id),
            shortcuts: self.shortcuts.is_some(),
            find: self.typing_in_find().is_some(),
            copy_mode: self.copy_modes.contains_key(&id),
        };
        let first = ime::dest(&owners);
        let taken = match first {
            Dest::Tool => self.ime_tool_text(text, event_loop),
            Dest::Inline => self.inline_edit_key(id, EditKey::Text(text.to_string())),
            Dest::Composer | Dest::Field => self.composer_type_text(text),
            _ => false,
        };
        if taken {
            return;
        }
        // An owner that had no text entry after all hands the text to the terminal, as its key would.
        // The key would reach Clippings next: any other key gives the panel's keyboard back.
        if !owners.belt {
            if let Some(c) = self.clips.get_mut(&id).filter(|c| c.keys && c.model.visible) {
                c.keys = false;
                c.preview = false;
                self.redraw();
            }
        }
        match ime::after_refusal(first, false) {
            Dest::Drop => {}
            Dest::Find => self.find_type_text(id, text),
            _ => {
                self.completion_typed_text();
                if self.terminal_refuses_keys() {
                    return;
                }
                self.end_read_only_view();
                let Some(pane) = self.panes.get_mut(&id) else { return };
                pane.scroll(Scroll::Bottom);
                self.user_write(id, text.as_bytes(), crate::broadcast::Write::Key);
            }
        }
    }

    /// Ends this window's composition (focus lost, a pane switch, Esc), including the system
    /// IME's own: turning the IME off and on again makes Windows cancel it. Whatever it commits
    /// afterwards is dropped.
    pub(super) fn ime_cancel(&mut self) {
        if self.ime.preedit.is_none() {
            return;
        }
        self.ime.cancel();
        self.ime_reset_system();
        self.redraw();
    }

    fn ime_reset_system(&mut self) {
        self.ime.system_reset();
        if let Some(g) = &self.gpu {
            g.window.set_ime_allowed(false);
            g.window.set_ime_allowed(true);
        }
    }

    /// True while the focused pane has a composition: its key presses are the IME's. Esc
    /// cancels it first, as Windows IMEs expect. Releases pass, so a key held down from before
    /// the composition still reports its release.
    pub(super) fn ime_holds_key(&mut self, ev: &KeyEvent) -> bool {
        let composing = self.focused_id().is_some_and(|id| self.ime.of(id).is_some_and(|p| ime::holds_keys(Some(p))));
        if !composing || ev.state != ElementState::Pressed {
            return false;
        }
        if ev.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) {
            self.ime_cancel();
        }
        true
    }

    /// Run at each frame of this window: a composition belongs to the focused pane of the key
    /// window, so a pane or tab switch drops it, uncommitted. Only this window's own state is
    /// read, so another window's frames never touch it.
    pub(super) fn ime_sync(&mut self) {
        if self.ime.sync(self.focused_id(), self.focused) {
            self.ime_reset_system();
        }
    }

    /// What `pane`'s composition looks like on the grid, for `cursor_look_for`.
    pub(super) fn ime_drawn(&self, pane: PaneId) -> Option<ime::Drawn> {
        let p = self.panes.get(&pane)?;
        let pre = self.ime.of(pane).filter(|p| !p.text.is_empty())?;
        let start = scene::ime_cell(&p.screen);
        let cols = p.screen.term().columns();
        let (pieces, caret) = ime::layout(pre, start, cols);
        let dark = self.dark();
        let color = self.store.get(&p.profile).map_or(crate::palette::rgb(255, 255, 0), |pr| pr.color_for("IME Cursor Color", dark));
        // The caret color goes through Cursor Boost's mute like any processed background.
        let boost = self.store.get(&p.profile).map_or(0.0, |pr| pr.float_for("Cursor Boost", dark).clamp(0.0, 0.8) as f32);
        Some(ime::Drawn { pieces, caret, caret_color: crate::cursor_look::mute(color, p.screen.palette.bg, boost) })
    }

    /// Puts the IME's candidate window at the caret (at the cursor cell when nothing is
    /// composing), in physical pixels of the window's client area; set only when it changed.
    /// A hidden cursor or a scrolled view still has a cell (`scene::ime_cell`). The animated
    /// cursor's slide or smear is not followed, the cell is.
    pub(super) fn ime_area(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(id) = self.focused_id() else { return };
        let Some((_, rect)) = self.visible(tab).into_iter().find(|(pid, _)| *pid == id) else { return };
        let Some(p) = self.panes.get(&id) else { return };
        let (cell, scale) = (self.cell(), self.scale());
        let inner = self.inner(rect);
        let geometry = crate::cursor_look::Geometry::from_globals(&self.globals, scale, self.base_cell());
        let r = match self.ime_drawn(id) {
            Some(d) => ime::caret_rect((d.caret.0, p.screen.visual_col(d.caret.0, d.caret.1)), (inner.x, inner.y), cell, geometry.base_cell.1, p.screen.term().columns(), scale),
            None => {
                let (row, col) = scene::ime_cell(&p.screen);
                let col = p.screen.visual_col(row, col);
                let wide = scene::cursor_cell(&p.screen).is_some_and(|(_, _, w)| w);
                crate::cursor_look::box_rect(&geometry, (inner.x, inner.y), cell, row, col, wide)
            }
        };
        let area = [r.x as i32, r.y as i32, r.w.max(1.0) as i32, r.h.max(1.0) as i32];
        if self.ime.area == Some(area) {
            return;
        }
        self.ime.area = Some(area);
        if let Some(g) = &self.gpu {
            g.window.set_ime_cursor_area(PhysicalPosition::new(area[0] as f64, area[1] as f64), PhysicalSize::new(area[2] as f64, area[3] as f64));
        }
    }
}
