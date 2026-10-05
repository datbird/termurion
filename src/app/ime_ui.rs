//! IME composition (13k Task 9): the person's uncommitted text is kept on the focused pane and
//! drawn at the cursor; it reaches the program only when committed. The candidate window follows
//! the caret.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::Ime;

use crate::ime::{self, Step};

impl App {
    /// One `WindowEvent::Ime`. Composition is the focused pane's; a commit is typed input.
    pub(super) fn ime_event(&mut self, ime: Ime) {
        let Some(id) = self.focused_id() else { return };
        match ime::step(&ime) {
            Step::Ignore => {}
            Step::Clear => self.ime_clear(id),
            Step::Store(p) => {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.preedit = Some(p);
                }
                self.redraw();
            }
            Step::Commit(text) => {
                self.ime_clear(id);
                // Typed input: broadcast input and the composer see it as the person's typing.
                self.user_write(id, text.as_bytes(), crate::broadcast::Write::Key);
            }
        }
    }

    /// Drops `pane`'s composition without committing it.
    fn ime_clear(&mut self, pane: PaneId) {
        if self.panes.get_mut(&pane).is_some_and(|p| p.preedit.take().is_some()) {
            self.redraw();
        }
    }

    /// Ends every composition (focus lost, a pane switch, Esc), including the system IME's own:
    /// turning the IME off and on again makes Windows cancel its composition.
    pub(super) fn ime_cancel(&mut self) {
        let mut any = false;
        for p in self.panes.values_mut() {
            any |= p.preedit.take().is_some();
        }
        if any {
            if let Some(g) = &self.gpu {
                g.window.set_ime_allowed(false);
                g.window.set_ime_allowed(true);
            }
            self.redraw();
        }
    }

    /// True while the focused pane has a composition: its keys are the IME's. Esc cancels it
    /// first, as Windows IMEs expect.
    pub(super) fn ime_holds_key(&mut self, ev: &KeyEvent) -> bool {
        let composing = self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| ime::holds_keys(p.preedit.as_ref()));
        if composing && ev.state == ElementState::Pressed && ev.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) {
            self.ime_cancel();
        }
        composing
    }

    /// A composition belongs to the focused pane of the key window: any other pane's is
    /// dropped (a pane or tab switch loses a half-typed composition, uncommitted).
    pub(super) fn ime_drop_unfocused(&mut self) {
        let owner = self.focused.then(|| self.focused_id()).flatten();
        if self.panes.iter().any(|(id, p)| p.preedit.is_some() && Some(*id) != owner) {
            for (id, p) in self.panes.iter_mut() {
                if Some(*id) != owner {
                    p.preedit = None;
                }
            }
            if let Some(g) = &self.gpu {
                g.window.set_ime_allowed(false);
                g.window.set_ime_allowed(true);
            }
        }
    }

    /// What the focused pane's composition looks like on the grid, for `cursor_look_for`.
    pub(super) fn ime_drawn(&self, pane: PaneId) -> Option<ime::Drawn> {
        let p = self.panes.get(&pane)?;
        let pre = p.preedit.as_ref().filter(|p| !p.text.is_empty())?;
        let (row, col, _) = scene::cursor_cell(&p.screen)?;
        let cols = p.screen.term().columns();
        let (pieces, caret) = ime::layout(pre, (row, col), cols);
        let dark = self.dark();
        let color = self.store.get(&p.profile).map_or(crate::palette::rgb(255, 255, 0), |pr| pr.color_for("IME Cursor Color", dark));
        // The caret color goes through Cursor Boost's mute like any processed background.
        let boost = self.store.get(&p.profile).map_or(0.0, |pr| pr.float_for("Cursor Boost", dark).clamp(0.0, 0.8) as f32);
        Some(ime::Drawn { pieces, caret, caret_color: crate::cursor_look::mute(color, p.screen.palette.bg, boost) })
    }

    /// Puts the IME's candidate window at the caret (at the cursor when nothing is composing),
    /// in physical pixels of the window's client area. Called after each frame; the animated
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
            Some(d) => ime::caret_rect(d.caret, (inner.x, inner.y), cell, geometry.base_cell.1, p.screen.term().columns(), scale),
            None => match scene::cursor_cell(&p.screen) {
                Some((row, col, wide)) => crate::cursor_look::box_rect(&geometry, (inner.x, inner.y), cell, row, col, wide),
                None => return,
            },
        };
        if let Some(g) = &self.gpu {
            g.window.set_ime_cursor_area(PhysicalPosition::new(r.x as f64, r.y as f64), PhysicalSize::new(r.w.max(1.0) as f64, r.h.max(1.0) as f64));
        }
    }
}
