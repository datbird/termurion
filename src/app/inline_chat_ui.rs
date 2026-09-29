//! The inline chat in the app: a pane's binding to a chat, View > Show Inline Chat, the panel
//! at the right of the pane (its toolbar, its divider, the Switch menu), and the binding's
//! place in arrangements (`PTYSession.swift:2554-2699`, `ChatViewController.swift:3703-3803`).
//! Until Task 10 the panel draws its toolbar and an empty band.

use super::*;
use crate::chat_toolbar as tb;
use crate::inline_chat::{self as ic, Binding, Button as IcButton, Toggle};
use crate::layout::visible_rects;

/// One pane's inline chat. App-wide, keyed by pane (as `clips`).
#[derive(Default)]
pub(super) struct InlineState {
    pub(super) binding: Binding,
    /// The conversation's scroll, whether it stays at the bottom, whether the panel has the
    /// keyboard, and the input: Task 10 uses these.
    #[allow(dead_code)]
    pub(super) scroll: f32,
    #[allow(dead_code)]
    pub(super) pinned: bool,
    pub(super) keys: bool,
    #[allow(dead_code)]
    pub(super) draft: crate::chat_input::Draft,
    #[allow(dead_code)]
    pub(super) edit: crate::text_edit::TextEdit,
    #[allow(dead_code)]
    pub(super) wrap: crate::text_edit::WrapCache,
    /// The toolbar button under the pointer.
    hover: Option<IcButton>,
    /// The bound chat's title as the toolbar shows it, read at most once a second.
    title: std::cell::RefCell<Option<(Instant, String)>>,
}

/// A rectangle of a panel this window drew, for a press on it. Task 10 adds more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PanelHit {
    New(PaneId, Rect),
    Switch(PaneId, Rect),
    Info(PaneId, Rect),
    Hide(PaneId, Rect),
    Divider(PaneId, Rect),
    Body(PaneId, Rect),
}

impl PanelHit {
    fn rect(&self) -> Rect {
        match *self {
            PanelHit::New(_, r) | PanelHit::Switch(_, r) | PanelHit::Info(_, r) | PanelHit::Hide(_, r) | PanelHit::Divider(_, r) | PanelHit::Body(_, r) => r,
        }
    }

    fn button(&self) -> Option<(PaneId, IcButton)> {
        match *self {
            PanelHit::New(p, _) => Some((p, IcButton::New)),
            PanelHit::Switch(p, _) => Some((p, IcButton::Switch)),
            PanelHit::Info(p, _) => Some((p, IcButton::Info)),
            PanelHit::Hide(p, _) => Some((p, IcButton::Hide)),
            PanelHit::Divider(..) | PanelHit::Body(..) => None,
        }
    }
}

/// The ids of the Switch menu's items: `aichat:inline:<pane>:<chat>`.
const SWITCH_ID: &str = "aichat:inline:";

fn switch_id(pane: PaneId, chat: &str) -> String {
    format!("{SWITCH_ID}{pane}:{chat}")
}

fn parse_switch_id(id: &str) -> Option<(PaneId, &str)> {
    let (pane, chat) = id.strip_prefix(SWITCH_ID)?.split_once(':')?;
    Some((pane.parse().ok()?, chat)).filter(|(_, c)| !c.is_empty())
}

impl App {
    /// The panel's stored width in logical pixels, when one was dragged.
    fn inline_stored_width(&self) -> Option<f32> {
        self.globals.get(ic::WIDTH_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map(|f| f as f32)
    }

    /// Whether a chat is still in the store. With the store not open yet nothing says it is
    /// gone (restore binds without opening it, ruling F21); a read error keeps the panel.
    fn inline_chat_exists(&self, chat: &str) -> bool {
        self.chat_db.as_ref().is_none_or(|db| !matches!(db.chat(chat), Ok(None)))
    }

    /// The panel's width in logical pixels: 0 without a shown, existing chat, and in a tmux
    /// pane, whose size tmux locks (the panel is refused there).
    pub(super) fn inline_width(&self, id: PaneId) -> f32 {
        let Some(s) = self.inline.get(&id) else { return 0.0 };
        if self.tmux_of(id).is_some() {
            return 0.0;
        }
        let exists = s.binding.chat.as_deref().is_some_and(|c| self.inline_chat_exists(c));
        ic::width(&s.binding, exists, self.inline_stored_width())
    }

    /// The panels of the current tab: (pane, panel rectangle).
    fn inline_panels(&self) -> Vec<(PaneId, Rect)> {
        if self.inline.is_empty() {
            return Vec::new();
        }
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), GAP).into_iter().filter_map(|(id, r)| self.gutter_split(id, self.pane_bar_split(tab, id, r).0).2.map(|p| (id, p))).collect()
    }

    /// Whether the focused pane's inline chat shows (the View menu's check mark).
    pub(super) fn inline_shown(&self) -> bool {
        self.focused_id().is_some_and(|id| self.inline_width(id) > 0.0)
    }

    /// The most recent chat linked to the pane, opening the store if need be.
    fn inline_linked_chat(&mut self, pane: PaneId) -> Option<String> {
        let reference = self.session_ref(pane)?;
        let chats = self.chat_db()?.chats().ok()?;
        crate::chat_list::last_linked(&chats, &reference).map(|c| c.id.clone())
    }

    /// View > Show Inline Chat (`toggleInlineChat`, `PSS:2586-2624`): the focused pane. It
    /// never opens the chat window.
    pub(super) fn toggle_inline_chat(&mut self) {
        let Some(pane) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        let binding = self.inline.get(&pane).map(|s| s.binding.clone()).unwrap_or_default();
        let showing = binding.chat.is_some() && binding.visible;
        if !showing && self.tmux_of(pane).is_some() {
            return self.banners.set("inline_chat", ic::TMUX_REFUSED);
        }
        // A bound chat flips with no gate; after a failed database open nothing more happens
        // (12c checklist row 42).
        let gate = binding.chat.is_some() || (!self.chat_db_failed && self.ai_check(false));
        let linked = if gate && binding.chat.is_none() { self.inline_linked_chat(pane) } else { None };
        match ic::toggle(&binding, gate, linked.as_deref()) {
            Toggle::Flip => {
                if let Some(chat) = binding.chat.as_deref() {
                    self.bind_inline(pane, chat, !binding.visible);
                }
            }
            Toggle::Bind(chat) => self.bind_inline(pane, &chat, true),
            Toggle::Create => {
                if let Some(chat) = self.create_inline_chat(pane) {
                    self.bind_inline(pane, &chat, true);
                }
            }
            Toggle::Refused => {}
        }
    }

    /// A new chat for the pane's panel (`createInlineChat`, `PSS:2665-2699`): "Chat about
    /// <name>", not linked, with an offerLink bubble to link it. None when the pane has no
    /// reference to offer (the earlier run's links could not be cleared) or the store is closed.
    fn create_inline_chat(&mut self, pane: PaneId) -> Option<String> {
        let Some(guid) = self.session_ref(pane) else {
            if let Some(text) = super::chat_app::links_off_text(self.chat_links_off) {
                self.banners.set("chat_links", text.to_string());
            }
            return None;
        };
        let name = tb::drop_unsafe(&self.window_title(pane));
        let name = if name.trim().is_empty() { "Unnamed session".to_string() } else { name };
        let id = self.chat_create(&tb::chat_about_title(&name), None, None, "", vec![])?;
        self.client_local(&id, crate::chat_message::ClientLocal::OfferLink { terminal: true, guid, name: Some(name) });
        Some(id)
    }

    /// Binds the pane's panel to `chat` and shows or hides it (`inlineChatID`,
    /// `inlineChatVisible`): the pane's columns change, so its window lays out again. A tmux
    /// pane refuses a shown panel, as Clippings does.
    pub(super) fn bind_inline(&mut self, pane: PaneId, chat: &str, visible: bool) {
        if visible && self.tmux_of(pane).is_some() {
            return self.banners.set("inline_chat", ic::TMUX_REFUSED);
        }
        // The pane may sit in another window than the current one.
        if !self.enter_pane_window(pane) {
            return;
        }
        let s = self.inline.entry(pane).or_default();
        s.binding = Binding { chat: Some(chat.to_string()), visible };
        if !visible {
            s.keys = false;
        }
        self.mark_state_dirty();
        self.relayout();
        self.sync_checks();
    }

    /// Clears the bindings whose chat is gone, once the store says so (not during layout,
    /// `CVC:3901-3913`): the panel goes and the pane's columns come back.
    pub(super) fn inline_sweep(&mut self) {
        let Some(db) = self.chat_db.as_ref() else { return };
        let gone: Vec<PaneId> = self.inline.iter().filter(|(_, s)| s.binding.chat.as_deref().is_some_and(|c| matches!(db.chat(c), Ok(None)))).map(|(id, _)| *id).collect();
        if gone.is_empty() {
            return;
        }
        for id in gone {
            self.inline.remove(&id);
        }
        self.mark_state_dirty();
        self.relayout_changed_windows();
        self.sync_checks();
        self.redraw_all();
    }

    /// The toolbar's title: the bound chat's title, else "AI Chat".
    fn inline_title(&self, s: &InlineState) -> String {
        let Some(chat) = s.binding.chat.as_deref() else { return ic::TITLE.to_string() };
        let mut cache = s.title.borrow_mut();
        if let Some((at, title)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(1) {
                return title.clone();
            }
        }
        let title = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()).map(|c| c.title).filter(|t| !t.trim().is_empty()).map(|t| tb::drop_unsafe(&t)).unwrap_or_else(|| ic::TITLE.to_string());
        *cache = Some((Instant::now(), title.clone()));
        title
    }

    /// Draws the current tab's panels and answers their hit rectangles (this window's).
    pub(super) fn draw_inline_chats(&self, frame: &mut scene::Frame, cell: (f32, f32)) -> Vec<PanelHit> {
        let mut hits = Vec::new();
        let scale = self.scale();
        for (id, panel) in self.inline_panels() {
            let Some(s) = self.inline.get(&id) else { continue };
            let l = ic::toolbar(panel, scale);
            let text = match s.hover {
                Some(b) => b.tip().to_string(),
                None => self.inline_title(s),
            };
            let cols = (l.title.w / cell.0).floor().max(0.0) as usize;
            let title = ic::truncate_tail(&text, cols);
            scene::inline_chat_panel(frame, &scene::InlineDraw { panel, layout: &l, title: &title, hover: s.hover }, cell, scale);
            hits.push(PanelHit::Divider(id, Rect { w: 4.0 * scale, ..panel }));
            hits.push(PanelHit::New(id, l.new));
            hits.push(PanelHit::Switch(id, l.switch));
            hits.push(PanelHit::Info(id, l.info));
            hits.push(PanelHit::Hide(id, l.hide));
            hits.push(PanelHit::Body(id, panel));
        }
        hits
    }

    /// A press. True when it was in a panel and the panel took it: a left press; other buttons
    /// over a panel are swallowed by `inline_split_hit`. A left press elsewhere gives the
    /// keyboard back to the terminal.
    pub(super) fn inline_press(&mut self, button: MouseButton, x: f32, y: f32, _event_loop: &ActiveEventLoop) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let Some(hit) = self.inline_hits.iter().copied().find(|h| h.rect().contains(x, y)) else {
            for s in self.inline.values_mut() {
                s.keys = false;
            }
            return false;
        };
        let (PanelHit::New(pane, r) | PanelHit::Switch(pane, r) | PanelHit::Info(pane, r) | PanelHit::Hide(pane, r) | PanelHit::Divider(pane, r) | PanelHit::Body(pane, r)) = hit;
        // The panel's session becomes the focused one.
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        self.refresh_route();
        match hit {
            PanelHit::Divider(..) => {
                let start_w = crate::clippings::clamp_width(self.inline_stored_width().unwrap_or(ic::DEFAULT_WIDTH));
                self.drag = Drag::Inline { pane, start_x: x, start_w };
            }
            PanelHit::New(..) => self.inline_new_chat(pane),
            PanelHit::Switch(..) => self.show_switch_menu(pane, (r.x, r.y + r.h)),
            PanelHit::Info(..) => {
                if let Some(chat) = self.inline.get(&pane).and_then(|s| s.binding.chat.clone()) {
                    self.show_session_menu(&chat, Some(pane), (r.x, r.y + r.h));
                }
            }
            PanelHit::Hide(..) => {
                if let Some(chat) = self.inline.get(&pane).and_then(|s| s.binding.chat.clone()) {
                    self.bind_inline(pane, &chat, false);
                }
            }
            PanelHit::Body(..) => {
                if let Some(s) = self.inline.get_mut(&pane) {
                    s.keys = true;
                }
            }
        }
        self.redraw();
        true
    }

    /// The panel's "new chat" button (`CVC:3703-3717`): the gate, then a new chat shown.
    fn inline_new_chat(&mut self, pane: PaneId) {
        if self.chat_db_failed || !self.ai_check(false) {
            return;
        }
        if let Some(chat) = self.create_inline_chat(pane) {
            self.bind_inline(pane, &chat, true);
        }
    }

    /// The Switch button's menu (`CVC:3719-3767`): every chat, the current one checked, the
    /// chats linked to this session first under a separator (muda has no bold item). A pick
    /// arrives as `aichat:inline:<pane>:<chat>` and lives until then (`popup_menu`).
    fn show_switch_menu(&mut self, pane: PaneId, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let reference = self.session_ref(pane);
        let current = self.inline.get(&pane).and_then(|s| s.binding.chat.clone());
        let Some(chats) = self.chat_db().and_then(|db| db.chats().ok()) else { return };
        let rows: Vec<(String, String, bool, bool)> = chats.iter().map(|c| (c.id.clone(), c.title.clone(), c.orchestration_enabled, reference.is_some() && c.terminal_session_guid == reference)).collect();
        let (here, rest) = ic::switch_groups(ic::switch_menu(&rows, current.as_deref()));
        let menu = muda::Menu::new();
        let mut results = Vec::new();
        for (n, items) in [&here, &rest].into_iter().enumerate() {
            if n == 1 && !here.is_empty() && !rest.is_empty() {
                results.push(menu.append(&muda::PredefinedMenuItem::separator()));
            }
            for i in items {
                let title = tb::drop_unsafe(&i.title).replace('&', "&&");
                let title = if i.orchestrator { format!("{} {title}", crate::chat_list::ORCHESTRATOR_GLYPH) } else { title };
                results.push(match &i.id {
                    None => menu.append(&muda::MenuItem::new(title, false, None)),
                    Some(chat) => menu.append(&muda::CheckMenuItem::with_id(switch_id(pane, chat), title, true, i.checked, None)),
                });
            }
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Chat menu: {e}"));
            return;
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the Switch menu: binds the pane's panel to the chat, if the chat is still
    /// there (`CVC:3769-3786`). False for other ids.
    pub(super) fn inline_menu_picked(&mut self, id: &str) -> bool {
        if !id.starts_with(SWITCH_ID) {
            return false;
        }
        let Some((pane, chat)) = parse_switch_id(id) else { return true };
        if self.panes.contains_key(&pane) && self.chat_db.is_some() && self.inline_chat_exists(chat) {
            self.bind_inline(pane, chat, true);
        }
        true
    }

    /// Pointer motion: the button under the pointer (its tip shows), and a width drag.
    pub(super) fn inline_move(&mut self, x: f32, y: f32) {
        if self.inline.is_empty() {
            return;
        }
        let over = self.inline_hits.iter().find(|h| h.rect().contains(x, y)).and_then(PanelHit::button);
        let mut changed = false;
        for (id, s) in self.inline.iter_mut() {
            let now = over.filter(|(p, _)| p == id).map(|(_, b)| b);
            if s.hover != now {
                s.hover = now;
                changed = true;
            }
        }
        if changed {
            self.redraw();
        }
        if let Drag::Inline { pane, start_x, start_w } = self.drag {
            if !self.inline.contains_key(&pane) {
                return;
            }
            let w = crate::clippings::clamp_width(start_w + (start_x - x) / self.scale());
            self.globals.set(ic::WIDTH_KEY, toml::Value::Float(f64::from(w.round())));
            self.relayout();
        }
    }

    /// The width drag ends: the width is kept for every panel.
    pub(super) fn inline_release(&mut self) {
        self.save_globals();
    }

    /// Whether (x, y) is in one of the current tab's panels.
    pub(super) fn inline_split_hit(&self, x: f32, y: f32) -> bool {
        self.inline_panels().iter().any(|(_, p)| p.contains(x, y))
    }

    /// The session keys an arrangement keeps for a pane.
    pub(super) fn inline_table(&self, id: PaneId) -> toml::Table {
        self.inline.get(&id).map(|s| ic::to_table(&s.binding)).unwrap_or_default()
    }

    /// A restored session's inline chat, with no gate and without opening the store
    /// (`PSS:2626-2663`).
    pub(super) fn restore_inline(&mut self, id: PaneId, session: &toml::Value) {
        let Some((chat, visible)) = session.as_table().and_then(ic::from_table) else { return };
        let exists = self.chat_db.as_ref().is_some_and(|db| matches!(db.chat(&chat), Ok(Some(_))));
        if let Some(binding) = ic::restore(&chat, visible, self.chat_db.is_some(), exists) {
            self.inline.insert(id, InlineState { binding, ..InlineState::default() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_ids_round_trip_and_refuse_the_rest() {
        assert_eq!(parse_switch_id(&switch_id(7, "abc-1")), Some((7, "abc-1")));
        assert_eq!(parse_switch_id("aichat:inline:x:abc"), None);
        assert_eq!(parse_switch_id("aichat:inline:7:"), None);
        assert_eq!(parse_switch_id("aichat:menu:7:abc"), None);
    }
}
