//! Key binding and pointer binding actions: what each iTerm2 action does in the app.

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Side;
use winit::event_loop::ActiveEventLoop;
use winit::window::Fullscreen;

use super::App;
use crate::commands::Status;
use crate::keyaction::{self, Binding};
use crate::layout::{Axis, Dir, PaneId};
use crate::menus::{self, Chord};
use crate::pointer::{self, PointerBinding};

/// Arrow keys that move the cursor from `cursor` to `target` (column, row), as iTerm2's
/// Alt-click does (PTYTextView placeCursorOnCurrentLineWithEvent:). Without vertical
/// moves the cursor walks the text in reading order.
pub(super) fn option_click_arrows(cursor: (u32, u32), target: (u32, u32), cols: u32, vertical_ok: bool, app_cursor: bool) -> Vec<u8> {
    let arrow = |c: char| if app_cursor { format!("\x1bO{c}") } else { format!("\x1b[{c}") };
    let (mut cx, mut cy) = (cursor.0 as i64, cursor.1 as i64);
    let (tx, ty) = (target.0 as i64, target.1 as i64);
    let mut out = String::new();
    if !vertical_ok {
        let steps = (ty * cols as i64 + tx) - (cy * cols as i64 + cx);
        out = arrow(if steps > 0 { 'C' } else { 'D' }).repeat(steps.unsigned_abs() as usize);
        return out.into_bytes();
    }
    if cx > tx {
        out.push_str(&arrow('D').repeat((cx - tx) as usize));
        cx = tx;
    }
    while cy != ty {
        out.push_str(&arrow(if cy > ty { 'A' } else { 'B' }));
        cy += if cy > ty { -1 } else { 1 };
    }
    if cx != tx {
        out.push_str(&arrow(if tx > cx { 'C' } else { 'D' }).repeat((tx - cx).unsigned_abs() as usize));
    }
    out.into_bytes()
}

/// Which pane a pointer action works on. iTerm2 runs it in the clicked session, whether
/// or not the click focused it (Focus window after right or middle click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// Text in or out of the clicked pane; focus stays where it was.
    Clicked,
    /// The context menu's commands act on the focused pane, so the clicked pane takes focus.
    FocusClicked,
    /// Tab and pane navigation, new tabs and splits: from the focused pane, as usual.
    Focused,
}

pub(super) fn pointer_target(action: &str) -> Target {
    match action {
        "kContextMenuPointerAction" | "kSelectMenuItemPointerAction" => Target::FocusClicked,
        "kPasteFromClipboardPointerAction" | "kPasteFromSelectionPointerAction" | "kExtendSelectionPointerAction"
        | "kSendEscapeSequencePointerAction" | "kSendHexCodePointerAction" | "kSendTextPointerAction" | "kCopyOrPastePointerAction" => Target::Clicked,
        _ => Target::Focused,
    }
}

/// Toggle Setting's parameter: {"key": ..., "isProfile": ..., "label": ...}.
pub(super) fn toggle_target(json: &str) -> Option<(String, bool)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some((v.get("key")?.as_str()?.to_string(), v.get("isProfile").and_then(|b| b.as_bool()).unwrap_or(false)))
}

/// Select Menu Item's parameter: "title" or "title\nidentifier" for key bindings, and
/// "identifier\ntitle" for pointer bindings.
pub(super) fn menu_item_parts(text: &str, pointer: bool) -> (String, Option<String>) {
    match text.split_once('\n') {
        Some((a, b)) if pointer => (b.to_string(), Some(a.to_string())),
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (text.to_string(), None),
    }
}

fn find_menu_item(text: &str, pointer: bool) -> Option<&'static menus::Item> {
    let (title, id) = menu_item_parts(text, pointer);
    id.and_then(|id| menus::by_id(&id))
        .or_else(|| menus::items().iter().find(|i| !i.separator && !i.submenu && i.label() == title))
}

impl App {
    fn focused_pane_id(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Bytes an action sends, as if typed.
    pub(super) fn send_to_focused(&mut self, bytes: &[u8]) {
        self.send_focused_as(bytes, crate::broadcast::Write::SendText);
    }

    fn send_focused_as(&mut self, bytes: &[u8], what: crate::broadcast::Write) {
        if bytes.is_empty() {
            return;
        }
        let Some(id) = self.focused_id() else { return };
        if let Some(p) = self.panes.get_mut(&id).filter(|p| p.exited.is_none()) {
            p.scroll(Scroll::Bottom);
            self.user_write(id, bytes, what);
        }
    }

    fn later(&mut self, name: &str, milestone: u8) {
        self.banners.set("action", format!("{name} comes in milestone {milestone}."));
    }

    fn select_pane_dir(&mut self, dir: Dir) {
        let area = self.content_area();
        if let Some(t) = self.tabs.get_mut(self.active) {
            if let Some(id) = t.layout.neighbor(t.focused, dir, area) {
                t.focused = id;
                t.zoomed = None;
            }
        }
    }

    fn step_pane(&mut self, forward: bool) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            if let Some(id) = t.layout.next(t.focused, forward) {
                t.focused = id;
                t.zoomed = None;
            }
        }
    }

    fn step_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n > 0 {
            self.active = if forward { (self.active + 1) % n } else { (self.active + n - 1) % n };
        }
    }

    fn swap_with(&mut self, other: Option<PaneId>) {
        if let (Some(f), Some(o)) = (self.tabs.get(self.active).map(|t| t.focused), other) {
            if self.tmux_swap(f, o) {
                return;
            }
        }
        if let (Some(t), Some(o)) = (self.tabs.get_mut(self.active), other) {
            t.layout.swap(t.focused, o);
        }
    }

    fn grow_focused(&mut self, axis: Axis, grow: bool) {
        // One cell's share of the tab, the nearest thing to iTerm2's one-cell step.
        let area = self.content_area();
        let (cw, ch) = self.cell();
        let step = match axis {
            Axis::Row => cw / area.w.max(1.0),
            Axis::Column => ch / area.h.max(1.0),
        };
        if let Some(t) = self.tabs.get_mut(self.active) {
            let f = t.focused;
            t.layout.grow(f, axis, if grow { step } else { -step });
        }
    }

    fn open_with_profile(&mut self, guid: &str, split: Option<Axis>) {
        if self.store.get(guid).is_none() {
            return self.banners.set("action", format!("No profile with Guid {guid}."));
        }
        match split {
            Some(axis) => self.split(axis, Some(guid)),
            None => self.new_tab(Some(guid)),
        }
    }

    fn toggle_setting(&mut self, json: &str) {
        let Some((key, is_profile)) = toggle_target(json) else { return };
        // A hand-edited or imported binding never flips an AI consent switch.
        if !crate::ai_settings::toggle_allowed(&key) {
            return;
        }
        if is_profile {
            let Some(guid) = self.focused_pane_id().and_then(|id| self.panes.get(&id)).map(|p| self.store.shared_guid(&p.profile)) else { return };
            if let Some(p) = self.store.get_mut(&guid) {
                let now = p.bool(&key);
                p.set(&key, toml::Value::Boolean(!now));
            }
            self.save_due = Some(std::time::Instant::now());
            self.refresh_panes();
        } else {
            let now = self.globals.bool(&key);
            self.globals.set(&key, toml::Value::Boolean(!now));
            self.save_globals();
        }
    }

    fn set_profile(&mut self, guid: &str) {
        if let Some(id) = self.focused_pane_id() {
            self.set_profile_on(id, guid);
        }
    }

    pub(super) fn set_profile_on(&mut self, id: PaneId, guid: &str) {
        let Some(p) = self.store.get(guid).cloned() else {
            return self.banners.set("action", format!("No profile with Guid {guid}."));
        };
        // A new profile ends any session copy, as SetProfile does.
        self.remarry(id, &p.guid());
    }

    /// Paste... (41) and Paste from Selection... (42) with their configuration, into `id`.
    pub(super) fn paste_special_in(&mut self, id: PaneId, b: &Binding) {
        let text = if b.action == 41 { self.clipboard_text() } else { self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()) };
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            self.paste_with_config(id, &t, &b.text);
        }
    }

    fn copy_or(&mut self, otherwise: impl FnOnce(&mut Self)) {
        let Some(id) = self.focused_pane_id() else { return };
        let has = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()).is_some_and(|s| !s.is_empty());
        if has { self.copy_selection(id) } else { otherwise(self) }
    }

    fn clipboard_has_text(&mut self) -> bool {
        self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).is_some_and(|t| !t.is_empty())
    }

    /// Runs one key binding action. `fallback` is what the key would have sent; `chord` is
    /// the Win chord it came from, if any.
    pub(super) fn run_key_action(&mut self, b: &Binding, fallback: Option<Vec<u8>>, chord: Option<&Chord>, event_loop: &ActiveEventLoop) {
        let name = keyaction::info(b.action).map_or("This action", |a| a.name.as_str());
        if let Status::Later(m) = keyaction::status(b.action) {
            return self.later(name, m);
        }
        if self.run_applied(b) {
            return;
        }
        let window = self.gpu.as_ref().map(|g| g.window.clone());
        match b.action {
            35 => {
                if let Some(id) = self.focused_id() {
                    self.start_coprocess(id, &b.text, false, crate::coprocess::Entry::KeyBinding);
                }
            }
            72 => self.copy_interpolated(&b.text),
            // Invoke Script Function (`PS:12220-12224`, `PS:11799-11813`): the focused
            // session's scope, else the app's, no timeout.
            60 => {
                let scope = Self::session_or_app(self.focused_id());
                self.invoke(&b.text, scope, crate::script_eval::Timeout::Never, true, super::script_ui::Origin::Report(super::script_ui::KEY_BINDING.into()));
            }
            // Toggle Pin Hotkey Window: the session's profile (its saved original) flips.
            43 => {
                if let Some(g) = self.focused_guid() {
                    let original = self.store.shared_guid(&g);
                    self.toggle_pin_hotkey(&original);
                }
            }
            0 => self.step_tab(true),
            2 => self.step_tab(false),
            // Scroll to End: every broadcast session (iTerm2's broadcastScrollToEnd).
            4 => {
                if let Some(id) = self.focused_id() {
                    let domain = self.broadcast.domain_panes(&self.broadcast_view());
                    let all = if domain.contains(&id) { domain } else { vec![id] };
                    for t in all {
                        if let Some(p) = self.panes.get_mut(&t) {
                            p.scroll(Scroll::Bottom);
                        }
                    }
                }
            }
            5 => self.focused_pane().map_or((), |p| p.scroll(Scroll::Top)),
            6 => self.focused_pane().map_or((), |p| p.scroll(Scroll::Delta(-1))),
            7 => self.focused_pane().map_or((), |p| p.scroll(Scroll::Delta(1))),
            8 => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageDown)),
            9 => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageUp)),
            10 => self.send_to_focused(&[b"\x1b".as_slice(), b.text.as_bytes()].concat()),
            11 => self.send_to_focused(&keyaction::hex_bytes(&b.text)),
            12 => self.send_to_focused(keyaction::expand(&b.text, b.escaping()).as_bytes()),
            // Send Snippet: nothing when the snippet was deleted.
            63 => {
                if let Some(bytes) = self.action_text(b) {
                    self.send_to_focused(&bytes);
                }
            }
            // Compose... (`PS:11996-12003`): ignored on an ended session or a tmux gateway.
            64 => {
                if let Some(id) = self.focused_id() {
                    self.composer_show_with(id, &b.text);
                }
            }
            38 => self.send_to_focused(keyaction::expand(&b.text, b.vim_escaping()).as_bytes()),
            // Send Text without Broadcasting.
            76 => self.send_focused_as(keyaction::expand(&b.text, b.vim_escaping()).as_bytes(), crate::broadcast::Write::SendTextLocal),
            // Start Instant Replay (`PS:12035-12042`): not in a tmux gateway. Once replaying,
            // only a Win chord bound to it steps back further (the panel takes every other key,
            // and those beep).
            15 => {
                if !self.focused_id().is_some_and(|id| self.is_tmux_gateway(id)) {
                    self.ir_advance(-1, event_loop);
                }
            }
            16 => self.send_to_focused(&[0x08]),
            17 => self.send_to_focused(&[0x7f]),
            18 => self.select_pane_dir(Dir::Left),
            19 => self.select_pane_dir(Dir::Right),
            20 => self.select_pane_dir(Dir::Up),
            21 => self.select_pane_dir(Dir::Down),
            23 => {
                if let Some(w) = &window {
                    w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
                }
            }
            25 => match find_menu_item(&b.text, false) {
                Some(item) => self.run_command(item, event_loop),
                None => self.banners.set("action", format!("No menu item {:?}.", b.text)),
            },
            1 => self.step_window(true),
            3 => self.step_window(false),
            26 => {
                if self.store.get(&b.text).is_some() {
                    self.pending_windows.push(super::windows_ui::NewWindow::Profile(Some(b.text.clone())));
                } else {
                    self.banners.set("action", format!("No profile with Guid {}.", b.text));
                }
            }
            27 => self.open_with_profile(&b.text, None),
            36 => self.find_regex(&b.text),
            73 => self.copy_mode_commands(&b.text),
            45..=48 => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.move_selection_end(id, b.action, &b.text);
                }
            }
            57 | 58 => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.find_step(id, b.action == 58);
                }
            }
            28 => self.open_with_profile(&b.text, Some(Axis::Column)),
            29 => self.open_with_profile(&b.text, Some(Axis::Row)),
            30 => self.step_pane(true),
            31 => self.step_pane(false),
            32 | 39 => {
                if let Some(id) = self.mru.step(b.action == 32) {
                    if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                        self.active = i;
                    }
                    self.mru_mods = Some(self.mods);
                }
            }
            33 | 34 if !self.layout_allowed(crate::winmodel::LayoutOp::ReorderTab) => {}
            33 | 34 => {
                let n = self.tabs.len();
                if n > 1 {
                    let to = if b.action == 34 { (self.active + 1) % n } else { (self.active + n - 1) % n };
                    self.tabs.swap(self.active, to);
                    self.active = to;
                }
            }
            37 => self.set_profile(&b.text),
            49 => self.grow_focused(Axis::Column, false),
            50 => self.grow_focused(Axis::Column, true),
            51 => self.grow_focused(Axis::Row, false),
            52 => self.grow_focused(Axis::Row, true),
            53..=56 => {
                let dir = [Dir::Left, Dir::Right, Dir::Up, Dir::Down][(b.action - 53) as usize];
                let area = self.content_area();
                let other = self.tabs.get(self.active).and_then(|t| t.layout.neighbor(t.focused, dir, area));
                self.swap_with(other);
            }
            67 | 68 => {
                let other = self.tabs.get(self.active).and_then(|t| t.layout.next(t.focused, b.action == 67));
                self.swap_with(other);
            }
            59 => {
                if let Some(id) = self.focused_pane_id() {
                    let now = self.mouse_allowed(id);
                    self.mouse_override.insert(id, !now);
                }
            }
            61 => {
                let g = self.focused_pane_id().and_then(|id| self.panes.get(&id)).map(|p| self.store.shared_guid(&p.profile));
                self.new_tab(g.as_deref());
            }
            65 => self.send_tmux_command(&b.text),
            // Paste... and Paste from Selection... with their paste configuration.
            41 | 42 => {
                if let Some(id) = self.focused_id() {
                    self.paste_special_in(id, b);
                }
            }
            66 => {
                for step in keyaction::sequence(&b.text) {
                    // A sequence never runs another sequence (iTerm2's editor does not allow one).
                    if step.action != 66 {
                        self.run_key_action(&step, fallback.clone(), chord, event_loop);
                    }
                }
            }
            69 => {
                // A key bound to Copy or Send ^C is a copy key to the frustration detector,
                // whichever it does: after a reported drag there is nothing selected.
                if let Some(id) = self.focused_pane_id() {
                    self.frustration_copy(id);
                }
                self.copy_or(|app| app.send_to_focused(fallback.as_deref().unwrap_or_default()));
            }
            70 => {
                if self.clipboard_has_text() {
                    self.paste();
                } else {
                    self.send_to_focused(fallback.as_deref().unwrap_or_default());
                }
            }
            74 => {
                if let Some(item) = chord.and_then(crate::commands::command_for_chord) {
                    self.run_command(item, event_loop);
                }
            }
            75 => self.toggle_setting(&b.text),
            71 => {
                if let Some(p) = self.focused_pane() {
                    let marks = &mut p.own_mut().marks;
                    marks.alert_next = !marks.alert_next;
                }
            }
            _ => {}
        }
    }

    /// Runs one pointer binding at `cell` of `pane`.
    pub(super) fn run_pointer_action(&mut self, b: &PointerBinding, pane: PaneId, cell: (u32, u32), event_loop: &ActiveEventLoop) {
        if let Status::Later(m) = pointer::status(b) {
            return self.later(pointer::name(&b.action), m);
        }
        let target = pointer_target(&b.action);
        let before = self.focused_pane_id();
        if target != Target::Focused {
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.focused = pane;
            }
        }
        self.run_pointer_action_here(b, pane, cell, event_loop);
        // Text actions leave focus where it was; the click itself did not move it.
        if target == Target::Clicked {
            if let (Some(t), Some(prev)) = (self.tabs.get_mut(self.active), before) {
                if t.layout.panes().contains(&prev) {
                    t.focused = prev;
                }
            }
        }
    }

    fn run_pointer_action_here(&mut self, b: &PointerBinding, pane: PaneId, cell: (u32, u32), event_loop: &ActiveEventLoop) {
        let text = |b: &PointerBinding| keyaction::expand(&b.argument, b.escaping());
        match b.action.as_str() {
            // With an argument: iTerm2's paste configuration (Paste from Clipboard...).
            "kPasteFromClipboardPointerAction" if !b.argument.is_empty() => {
                if let (Some(t), Some(id)) = (self.clipboard_text(), self.focused_id()) {
                    self.paste_with_config(id, &t, &b.argument);
                }
            }
            "kPasteFromClipboardPointerAction" => self.paste(),
            // `textViewInvokeScriptFunction:` (`PS:13431-13433`): the clicked session's scope.
            "kInvokeScriptFunction" => {
                let scope = Self::session_or_app(Some(pane));
                self.invoke(&b.argument, scope, crate::script_eval::Timeout::Never, true, super::script_ui::Origin::Report(super::script_ui::POINTER_ACTION.into()));
            }
            "kPasteFromSelectionPointerAction" => {
                let sel = self.panes.get(&pane).and_then(|p| p.screen.term().selection_to_string());
                if let (Some(s), Some(id)) = (sel, self.focused_id()) {
                    if b.argument.is_empty() {
                        self.user_paste(id, &s, crate::broadcast::Write::Paste);
                    } else {
                        self.paste_with_config(id, &s, &b.argument);
                    }
                }
            }
            "kExtendSelectionPointerAction" => {
                let point = self.grid_point(pane, cell.0, cell.1);
                if let Some(sel) = self.panes.get_mut(&pane).and_then(|p| p.screen.term_mut().selection.as_mut()) {
                    sel.update(point, Side::Right);
                    self.copy_selection(pane);
                }
            }
            "kContextMenuPointerAction" => {
                let point = self.grid_point(pane, cell.0, cell.1);
                self.show_context_menu_at(Some((pane, point)));
            }
            "kOpenTargetPointerAction" | "kQuickLookAction" | "kOpenTargetInBackgroundPointerAction" => {
                let point = self.grid_point(pane, cell.0, cell.1);
                self.open_target(pane, point, b.action == "kOpenTargetInBackgroundPointerAction");
            }
            "kSmartSelectionPointerAction" | "kSmartSelectionIgnoringNewlinesPointerAction" => {
                let point = self.grid_point(pane, cell.0, cell.1);
                if self.smart_select(pane, point, b.action == "kSmartSelectionIgnoringNewlinesPointerAction").is_some() {
                    self.copy_selection(pane);
                }
            }
            "kCopyLinkAddressPointerAction" => {
                let point = self.grid_point(pane, cell.0, cell.1);
                self.copy_link_at(pane, point);
            }
            "kNextTabPointerAction" => self.step_tab(true),
            "kPrevTabPointerAction" => self.step_tab(false),
            "kSendEscapeSequencePointerAction" => self.send_to_focused(&[b"\x1b".as_slice(), b.argument.as_bytes()].concat()),
            "kSendHexCodePointerAction" => self.send_to_focused(&keyaction::hex_bytes(&b.argument)),
            "kSendTextPointerAction" => self.send_to_focused(text(b).as_bytes()),
            "kSelectPaneLeftPointerAction" => self.select_pane_dir(Dir::Left),
            "kSelectPaneRightPointerAction" => self.select_pane_dir(Dir::Right),
            "kSelectPaneAbovePointerAction" => self.select_pane_dir(Dir::Up),
            "kSelectPaneBelowPointerAction" => self.select_pane_dir(Dir::Down),
            "kNewTabWithProfilePointerAction" => self.open_with_profile(&b.argument, None),
            "kNewVerticalSplitWithProfilePointerAction" => self.open_with_profile(&b.argument, Some(Axis::Row)),
            "kNewHorizontalSplitWithProfilePointerAction" => self.open_with_profile(&b.argument, Some(Axis::Column)),
            "kSelectMenuItemPointerAction" => {
                if let Some(item) = find_menu_item(&b.argument, true) {
                    self.run_command(item, event_loop);
                }
            }
            "kSelectNextPanePointerAction" => self.step_pane(true),
            "kSelectPreviousPanePointerAction" => self.step_pane(false),
            "kCopyOrPastePointerAction" => self.copy_or(|app| app.paste()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_click_moves_along_the_line() {
        assert_eq!(option_click_arrows((2, 5), (5, 5), 80, false, false), b"\x1b[C\x1b[C\x1b[C");
        assert_eq!(option_click_arrows((5, 5), (3, 5), 80, false, true), b"\x1bOD\x1bOD");
        assert!(option_click_arrows((5, 5), (5, 5), 80, false, false).is_empty());
    }

    #[test]
    fn vertical_moves_only_in_the_alternate_screen() {
        // Without vertical moves, the cursor walks the wrapped text: 80 - 5 + 2 steps right.
        assert_eq!(option_click_arrows((5, 1), (2, 2), 80, false, false).len(), 77 * 3);
        assert_eq!(option_click_arrows((5, 1), (2, 2), 80, true, false), b"\x1b[D\x1b[D\x1b[D\x1b[B");
        assert_eq!(option_click_arrows((1, 3), (4, 1), 80, true, false), b"\x1b[A\x1b[A\x1b[C\x1b[C\x1b[C");
    }

    #[test]
    fn pointer_actions_act_on_the_clicked_pane() {
        assert_eq!(pointer_target("kPasteFromClipboardPointerAction"), Target::Clicked);
        assert_eq!(pointer_target("kSendTextPointerAction"), Target::Clicked);
        assert_eq!(pointer_target("kCopyOrPastePointerAction"), Target::Clicked);
        assert_eq!(pointer_target("kContextMenuPointerAction"), Target::FocusClicked);
        assert_eq!(pointer_target("kNextTabPointerAction"), Target::Focused);
    }

    #[test]
    fn toggle_setting_reads_its_json() {
        assert_eq!(toggle_target(r#"{"key":"FocusFollowsMouse","isProfile":false,"label":"x"}"#), Some(("FocusFollowsMouse".to_string(), false)));
        assert_eq!(toggle_target("junk"), None);
    }

    #[test]
    fn menu_item_parameters_name_a_title_and_maybe_an_id() {
        assert_eq!(menu_item_parts("Paste", false), ("Paste".to_string(), None));
        assert_eq!(menu_item_parts("Paste\nEdit > Paste", false), ("Paste".to_string(), Some("Edit > Paste".to_string())));
        // Pointer bindings store the identifier first.
        assert_eq!(menu_item_parts("Edit > Paste\nPaste", true), ("Paste".to_string(), Some("Edit > Paste".to_string())));
    }
}
