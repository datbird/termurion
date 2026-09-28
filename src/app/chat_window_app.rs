//! The AI Chat window in the app (`ChatWindowController.swift`): its entry points (Window > AI
//! Chats, Session > Open AI Chat...), the toolbar's data and picks, the thin publishers every
//! chat feature uses, and the window's events. The chat list, conversation, input and find are
//! filled by the tasks after this one; the window is app-wide (one per app, iTerm2's singleton).

use super::*;
use crate::ai_catalog::{Effort, Feature, ServiceTier};
use crate::ai_settings::Vendor;
use crate::chat_agent::{self, EFFORT_KEY, HIGH_EFFORT_KEY, TIER_KEY, WEB_SEARCH_KEY};
use crate::chat_message::{ClientLocal, Content, Message, Participant};
use crate::chat_store::{Chat, Store};
use crate::chat_list::{self, Row};
use crate::chat_toolbar::{self as tb, Reuse};
use crate::chat_window_ui::{ChatData, ChatEvent, ChatWindow, ToolbarData};

/// `update_chat` from a row read just now, changing only what `f` names (ruling F6): the row's
/// permissions, model and every other column a window or list read earlier are never written
/// back from that older copy. False when the row is gone or the write failed.
pub(super) fn update_chat_in(store: &mut Store, id: &str, f: impl FnOnce(&mut Chat)) -> bool {
    let Ok(Some(mut row)) = store.chat(id) else { return false };
    f(&mut row);
    store.update_chat(&row).is_ok()
}

/// The list menu's ids (picked through `run_menu_id`).
const RENAME_ID: &str = "aichat:rename";
const DELETE_ID: &str = "aichat:delete";

/// What a chord does in the chat window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatChord {
    Close,
    Focus,
}

/// Win+W closes (`CWC:481-501`); Shift+Win+Y focuses (Window > AI Chats).
pub(super) fn chat_chord(c: &crate::menus::Chord) -> Option<ChatChord> {
    match c.key {
        crate::menus::ChordKey::Char('w') if crate::winkeys::chat_chord(c) => Some(ChatChord::Close),
        crate::menus::ChordKey::Char('y') if crate::winkeys::chat_chord(c) => Some(ChatChord::Focus),
        _ => None,
    }
}

impl App {
    /// The focused pane, when it is a live one.
    fn chat_focused_pane(&self) -> Option<PaneId> {
        self.focused_id().filter(|id| self.panes.contains_key(id))
    }

    /// Opens the window, or focuses it, centered on the monitor of the current terminal window.
    fn chat_window_show(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if let Some(w) = &self.chat_window {
            w.focus();
            return true;
        }
        let Some(g) = self.gpu.as_ref() else { return false };
        match ChatWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
            Ok(w) => {
                w.follow_theme(crate::tab_theme::forced_dark(self.tab_style()));
                if let Some(m) = g.window.current_monitor() {
                    w.center_on(&m);
                }
                if let Some(hwnd) = w.hwnd() {
                    crate::winkeys::add_chat_window(hwnd);
                }
                self.chat_window = Some(w);
            }
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                return false;
            }
        }
        if self.chat_sub.is_none() {
            self.chat_sub = Some(self.chat_broker.subscribe(None));
        }
        self.chat_toolbar_dirty = true;
        self.chat_snippets.clear();
        self.chat_rows_dirty = true;
        self.chat_window_refresh();
        true
    }

    /// The gate, the database and the window, in iTerm2's order (`showChatWindow`,
    /// `CWC:250-258`). After a failed open nothing more happens (checklist row 42, ruling F38).
    fn chat_window_enter(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.chat_db_failed || !self.ai_check(false) || self.chat_db().is_none() {
            return false;
        }
        self.chat_window_show(event_loop)
    }

    /// Window > AI Chats (`createNewChatIfNeeded`, `CWC:322-337`).
    pub(super) fn open_ai_chats(&mut self, event_loop: &ActiveEventLoop) {
        if !self.chat_window_enter(event_loop) {
            return;
        }
        let session = self.chat_focused_pane();
        let session_ref = session.and_then(|p| self.session_ref(p));
        let chats = self.chat_db.as_ref().and_then(|db| db.chats().ok()).unwrap_or_default();
        let linked = session_ref.as_deref().and_then(|r| chats.iter().find(|c| c.terminal_session_guid.as_deref() == Some(r)));
        // The shown chat, else the most recent one.
        let recent_id = self.chat_shown.clone().or_else(|| chats.first().map(|c| c.id.clone()));
        let empty = recent_id.and_then(|id| self.chat_db.as_ref()?.messages(&id).ok()).is_some_and(|m| tb::trivially_empty(&m));
        let now = super::warnings_app::unix_now();
        let most_recent = chats.first().map(|c| (c.id.as_str(), now - c.last_modified_date));
        match tb::ai_chats_reuse(chats.len(), session_ref.as_deref(), linked.map(|c| c.id.as_str()), empty, most_recent) {
            Reuse::New { offer } => self.create_window_chat(offer.and(session)),
            Reuse::Select(id) => self.show_chat(Some(id)),
        }
    }

    /// Session > Open AI Chat... (`CWC:646-662`): the focused session's last linked chat, else
    /// a new one about it.
    pub(super) fn open_ai_chat(&mut self, event_loop: &ActiveEventLoop) {
        if !self.chat_window_enter(event_loop) {
            return;
        }
        let Some(pane) = self.chat_focused_pane() else { return };
        let Some(reference) = self.session_ref(pane) else { return };
        let last = self.chat_db.as_ref().and_then(|db| db.chats().ok()).and_then(|chats| chats.into_iter().find(|c| c.terminal_session_guid.as_deref() == Some(reference.as_str())));
        let id = match last {
            Some(c) => Some(c.id),
            None => {
                let name = self.window_title(pane);
                let name = if name.trim().is_empty() { "Unnamed session".to_string() } else { name };
                self.chat_create(&tb::chat_about_title(&name), Some(&reference), None, "", vec![])
            }
        };
        if let Some(id) = id {
            self.show_chat(Some(id));
        }
    }

    /// `createNewChat(offerGuid:)` (`CWC:415-434`): a new chat that offers to link the live
    /// session it came from, else to enable orchestration.
    pub(super) fn create_window_chat(&mut self, offer: Option<PaneId>) {
        let Some(id) = self.chat_create(tb::NEW_CHAT_TITLE, None, None, "", vec![]) else { return };
        self.show_chat(Some(id.clone()));
        match offer.and_then(|p| self.session_ref(p).map(|guid| (p, guid))) {
            Some((pane, guid)) => {
                let name = Some(self.window_title(pane)).filter(|n| !n.trim().is_empty());
                self.client_local(&id, ClientLocal::OfferLink { terminal: true, guid, name });
            }
            None => self.client_local(&id, ClientLocal::OfferOrchestration),
        }
    }

    /// Shows `chat` (or none): the title, the toolbar and the data follow.
    pub(super) fn show_chat(&mut self, chat: Option<String>) {
        self.chat_shown = chat;
        self.chat_toolbar_dirty = true;
        self.chat_rows_dirty = true;
        self.chat_window_refresh();
    }

    fn chat_window_title(&self) -> String {
        self.chat_shown.as_deref().and_then(|id| self.chat_db.as_ref()?.chat(id).ok().flatten()).map(|c| c.title).filter(|t| !t.is_empty()).unwrap_or_else(|| tb::WINDOW_TITLE.to_string())
    }

    /// Rebuilds what the window shows and hands it over when it changed. The toolbar's model
    /// list and key state are computed only when marked dirty (ruling F17), never per chunk.
    fn chat_window_refresh(&mut self) {
        if self.chat_window.is_none() {
            return;
        }
        if self.chat_toolbar_dirty {
            self.chat_toolbar_dirty = false;
            let shown = self.chat_shown.clone();
            self.chat_toolbar = self.toolbar_data(shown.as_deref());
        }
        if self.chat_rows_dirty || self.chat_rows_built.is_none() {
            self.chat_rows_build();
        }
        let title = self.chat_window_title();
        self.chat_toolbar.title = title.clone();
        let data = ChatData { chat: self.chat_shown.clone(), toolbar: self.chat_toolbar.clone(), rows: self.chat_rows.clone() };
        if let Some(w) = self.chat_window.as_mut() {
            w.set_title(&title);
            if w.data() != &data {
                w.set_data(data);
            }
        }
    }

    /// The toolbar for `chat` (`ChatToolbar`, `CVC:3240-3345`).
    pub(super) fn toolbar_data(&mut self, chat: Option<&str>) -> ToolbarData {
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let row: Option<Chat> = chat.and_then(|id| self.chat_db.as_ref()?.chat(id).ok().flatten());
        let locked = chat.and_then(|id| self.chat_db.as_ref()?.messages(id).ok()).is_some_and(|m| tb::provider_locked(&m));
        let g = &self.globals;
        let manual = crate::ai_model::manual_models(g, catalog, &found);
        let model = row.as_ref().and_then(|c| c.model_name.as_deref()).and_then(|n| crate::ai_model::named(n, g, catalog, &found)).or_else(|| crate::ai_model::configured(g, catalog, &found));
        let is_manual = model.as_ref().is_some_and(|m| manual.iter().any(|x| x.name == m.name));

        // `providerIsAvailable` (`CVC:722-749`): Ollama always; the others need models and a key.
        let available: Vec<Vendor> = {
            let mut keys = crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner());
            let mut store = crate::ai_keys::system_store();
            Vendor::POPUP_ORDER.into_iter().filter(|&v| v == Vendor::Llama || (crate::ai_model::recommended(v, g, catalog, &found).is_some() && keys.is_set(v, &mut *store))).collect()
        };
        let current = model.as_ref().filter(|_| !is_manual).map(|m| m.vendor);
        let current_has_models = current.is_some_and(|v| v == Vendor::Llama || catalog.models.iter().any(|m| m.vendor == v));
        let manual_names: Vec<String> = manual.iter().map(|m| m.name.clone()).collect();
        let providers = tb::provider_options(&available, current, current_has_models, &manual_names);
        let provider = tb::provider_popup(&providers, chat.is_some() && !locked);
        let provider_selected = model.as_ref().map(|m| if is_manual { tb::manual_id(&m.name) } else { tb::vendor_id(m.vendor) });

        let mut models: Vec<String> = match &model {
            None => Vec::new(),
            Some(m) if is_manual => vec![m.name.clone()],
            Some(m) if m.vendor == Vendor::Llama => found.default_models().iter().map(|x| x.name.clone()).collect(),
            Some(m) => catalog.models.iter().filter(|x| x.vendor == m.vendor).map(|x| x.name.clone()).collect(),
        };
        if let Some(m) = &model {
            if !models.contains(&m.name) {
                models.insert(0, m.name.clone());
            }
        }
        let model_popup = tb::model_popup(models.len(), chat.is_some());

        let has = |f: Feature| model.as_ref().is_some_and(|m| crate::ai_model::has(m, f));
        let thinking = has(Feature::ConfigurableThinking).then(|| g.bool(HIGH_EFFORT_KEY));
        let web_search_enabled = has(Feature::HostedWebSearch);
        let efforts: Vec<(Effort, String)> = model.as_ref().map(|m| m.reasoning_efforts.iter().map(|&e| (e, tb::effort_title(e))).collect()).unwrap_or_default();
        let tiers: Vec<(ServiceTier, String)> = model.as_ref().map(|m| m.service_tiers.iter().map(|&t| (t, tb::tier_title(t))).collect()).unwrap_or_default();
        // The saved pick when the model takes it, else the thinking default, else the first.
        let effort = model.as_ref().and_then(|m| {
            chat_agent::effort_parse(&g.string(EFFORT_KEY))
                .filter(|e| m.reasoning_efforts.contains(e))
                .or_else(|| thinking.map(|on| if on { m.thinking_on_effort } else { m.thinking_off_effort }).filter(|e| m.reasoning_efforts.contains(e)))
                .or_else(|| m.reasoning_efforts.first().copied())
        });
        let tier = model.as_ref().and_then(|m| chat_agent::tier_parse(&g.string(TIER_KEY)).filter(|t| m.service_tiers.contains(t)).or_else(|| m.service_tiers.first().copied()));
        ToolbarData {
            title: String::new(),
            providers,
            provider,
            provider_selected,
            models,
            model: model_popup,
            model_selected: model.as_ref().map(|m| m.name.clone()),
            efforts,
            effort,
            tiers,
            tier,
            thinking,
            web_search: web_search_enabled && g.bool(WEB_SEARCH_KEY),
            web_search_enabled,
        }
    }

    /// A provider pick: binds the chat to that vendor's recommended model, else its first
    /// catalog model; a manual row binds that model (`CVC:3516-3537`). Refused once the
    /// provider is locked.
    fn chat_pick_provider(&mut self, id: &str) {
        let Some(chat) = self.chat_shown.clone() else { return };
        if self.chat_db.as_ref().and_then(|db| db.messages(&chat).ok()).is_some_and(|m| tb::provider_locked(&m)) {
            return;
        }
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let name = if let Some(raw) = id.strip_prefix("vendor:") {
            let Some(v) = raw.parse::<i64>().ok().and_then(Vendor::from_raw) else { return };
            crate::ai_model::recommended(v, &self.globals, catalog, &found).or_else(|| catalog.models.iter().find(|m| m.vendor == v).cloned()).map(|m| m.name)
        } else if let Some(n) = id.strip_prefix("manual:") {
            crate::ai_model::manual_models(&self.globals, catalog, &found).into_iter().find(|m| m.name == n).map(|m| m.name)
        } else {
            None
        };
        self.chat_bind_model(&chat, name);
    }

    /// A model pick within the provider (the model may change mid-chat, `CVC:3286-3293`).
    fn chat_pick_model(&mut self, name: &str) {
        let Some(chat) = self.chat_shown.clone() else { return };
        if !self.chat_toolbar.models.iter().any(|m| m == name) {
            return;
        }
        self.chat_bind_model(&chat, Some(name.to_string()));
    }

    /// `update_chat_in` over the chat database (ruling F6): the row is read again just before
    /// it is written, so only what `f` names changes.
    pub(super) fn chat_update(&mut self, id: &str, f: impl FnOnce(&mut Chat)) -> bool {
        self.chat_db.as_mut().is_some_and(|db| update_chat_in(db, id, f))
    }

    fn chat_bind_model(&mut self, chat: &str, name: Option<String>) {
        let Some(name) = name else { return };
        self.chat_update(chat, |c| c.model_name = Some(name));
        self.chat_toolbar_dirty = true;
        self.chat_window_refresh();
    }

    /// The model the toolbar's effort, tier and thinking picks apply to.
    fn chat_toolbar_model(&mut self) -> Option<crate::ai_catalog::Model> {
        let name = self.chat_toolbar.model_selected.clone()?;
        let found = self.ai_found();
        crate::ai_model::named(&name, &self.globals, crate::ai_catalog::bundled(), &found)
    }

    /// The effort, tier, thinking and web search picks fill 12c's four per-user globals; the
    /// next turn reads them (`chat_agent::turn_options`, ruling I-3).
    fn chat_toolbar_pick(&mut self, e: &ChatEvent) {
        let Some(model) = self.chat_toolbar_model() else { return };
        match e {
            ChatEvent::Effort(x) if model.reasoning_efforts.contains(x) => {
                self.globals.set(EFFORT_KEY, toml::Value::String(chat_agent::effort_raw(*x).to_string()));
                self.globals.set(HIGH_EFFORT_KEY, toml::Value::Boolean(*x != model.thinking_off_effort));
            }
            ChatEvent::Tier(t) if model.service_tiers.contains(t) => {
                self.globals.set(TIER_KEY, toml::Value::String(chat_agent::tier_raw(*t).to_string()));
            }
            ChatEvent::ToggleThinking if crate::ai_model::has(&model, Feature::ConfigurableThinking) => {
                let on = !self.globals.bool(HIGH_EFFORT_KEY);
                self.globals.set(HIGH_EFFORT_KEY, toml::Value::Boolean(on));
                if !model.reasoning_efforts.is_empty() {
                    let want = if on { model.thinking_on_effort } else { model.thinking_off_effort };
                    let pick = if model.reasoning_efforts.contains(&want) { Some(want) } else { model.reasoning_efforts.first().copied() };
                    if let Some(p) = pick {
                        self.globals.set(EFFORT_KEY, toml::Value::String(chat_agent::effort_raw(p).to_string()));
                    }
                }
            }
            ChatEvent::ToggleWebSearch if crate::ai_model::has(&model, Feature::HostedWebSearch) => {
                let on = !self.globals.bool(WEB_SEARCH_KEY);
                self.globals.set(WEB_SEARCH_KEY, toml::Value::Boolean(on));
            }
            _ => return,
        }
        self.save_globals();
        self.chat_toolbar_dirty = true;
        self.chat_window_refresh();
    }

    /// The list's rows, most recent first (`ChatListModel`): each chat's snippet is read once and
    /// kept until a message of it changes.
    fn chat_rows_build(&mut self) {
        self.chat_rows_dirty = false;
        self.chat_rows_built = Some(Instant::now());
        let Some(db) = self.chat_db.as_ref() else {
            self.chat_rows.clear();
            return;
        };
        let chats = db.chats().unwrap_or_default();
        let cache = std::cell::RefCell::new(std::mem::take(&mut self.chat_snippets));
        let snippet = |id: &str| -> String {
            if let Some(s) = cache.borrow().get(id) {
                return s.clone();
            }
            let s = db.messages(id).map(|m| chat_list::row_snippet(&m)).unwrap_or_default();
            cache.borrow_mut().insert(id.to_string(), s.clone());
            s
        };
        let broker = &self.chat_broker;
        let rows: Vec<Row> = chat_list::rows(&chats, &snippet, &|id| broker.is_typing(id, Participant::Agent), super::warnings_app::unix_now());
        self.chat_rows = rows;
        self.chat_snippets = cache.into_inner();
    }

    /// What the broker delivered changes the list: a whole message or a removal drops that
    /// chat's kept snippet, and any of them, or a typing or turn change, builds the rows again.
    fn chat_note_updates(&mut self, updates: &[(String, crate::chat_broker::Update)]) {
        use crate::chat_broker::Update;
        for (chat, u) in updates {
            match u {
                Update::Delivery { partial: true, .. } => {}
                Update::Delivery { .. } | Update::Removed { .. } => {
                    self.chat_snippets.remove(chat);
                    self.chat_rows_dirty = true;
                }
                Update::Typing { .. } | Update::Turn(_) => self.chat_rows_dirty = true,
            }
        }
    }

    /// Each pass, with or without the window: every subscription is drained (the one that
    /// hears all chats mints titles, ruling F14), title runs are polled, and the window is
    /// refreshed when something changed. The toolbar is rebuilt only for a whole message or a
    /// removal in the shown chat (a streamed chunk cannot change it). Answers the next wake-up:
    /// the ages' minute, and soon again while a title run is going.
    pub(super) fn chat_window_tick(&mut self) -> Option<Instant> {
        if self.chat_db.is_some() && self.chat_mint_sub.is_none() {
            self.chat_mint_sub = Some(self.chat_broker.subscribe(None));
        }
        let mut changed = false;
        if let Some(sub) = self.chat_mint_sub {
            let updates = self.chat_broker.drain(sub);
            self.chat_note_updates(&updates);
            let ended: Vec<String> = updates.iter().filter(|(_, u)| matches!(u, crate::chat_broker::Update::Turn(crate::chat_message::TurnEvent::Ended))).map(|(c, _)| c.clone()).collect();
            for chat in ended {
                self.mint_title(&chat);
            }
        }
        self.title_tick();
        if let Some(sub) = self.chat_sub {
            let updates = self.chat_broker.drain(sub);
            if !updates.is_empty() {
                changed = true;
                let shown = self.chat_shown.as_deref();
                if updates.iter().any(|(chat, u)| Some(chat.as_str()) == shown && matches!(u, crate::chat_broker::Update::Delivery { partial: false, .. } | crate::chat_broker::Update::Removed { .. })) {
                    self.chat_toolbar_dirty = true;
                }
            }
        }
        let mut due = (!self.title_runs.is_empty()).then(|| Instant::now() + std::time::Duration::from_millis(250));
        if self.chat_window.is_some() {
            let minute = self.chat_rows_built.map(|b| b + std::time::Duration::from_secs(60));
            if minute.is_some_and(|t| Instant::now() >= t) {
                self.chat_rows_dirty = true;
            }
            if changed || self.chat_rows_dirty {
                self.chat_window_refresh();
            }
            let minute = self.chat_rows_built.map(|b| b + std::time::Duration::from_secs(60));
            due = [due, minute].into_iter().flatten().min();
        }
        due
    }

    /// Stops the chat's streaming bubble (filled by Task 6; ruling F24).
    fn stop_streaming(&mut self, _chat: &str) {}

    /// Closing, or Close (Win+W): streaming stops, the subscription ends, the window goes
    /// (`CWC:481-501`). Nothing restores it at relaunch.
    pub(super) fn close_chat_window(&mut self) {
        if let Some(chat) = self.chat_shown.clone() {
            self.stop_streaming(&chat);
        }
        if let Some(sub) = self.chat_sub.take() {
            self.chat_broker.unsubscribe(sub);
        }
        if let Some(hwnd) = self.chat_window.as_ref().and_then(|w| w.hwnd()) {
            crate::winkeys::remove_chat_window(hwnd);
        }
        self.chat_window = None;
        self.chat_shown = None;
    }

    /// Win+W and Shift+Win+Y from the key hook with the chat window in front. True when taken;
    /// asked before the terminal's own chords, as `pwm_take_chord` is.
    pub(super) fn chat_take_chord(&mut self, c: &crate::menus::Chord, _event_loop: &ActiveEventLoop) -> bool {
        let Some(chord) = chat_chord(c) else { return false };
        let Some(hwnd) = self.chat_window.as_ref().and_then(|w| w.hwnd()) else { return false };
        // The window in front when the hook took the chord, not now.
        if crate::winkeys::chord_foreground() != hwnd {
            return false;
        }
        match chord {
            ChatChord::Close => self.close_chat_window(),
            ChatChord::Focus => {
                if let Some(w) = &self.chat_window {
                    w.focus();
                }
            }
        }
        true
    }

    /// Events for the chat window. False when `window` is not it.
    pub(super) fn chat_window_event(&mut self, window: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(w) = self.chat_window.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.close_chat_window(),
            // Back from Settings: a key or a model may have changed meanwhile (ruling F17).
            WindowEvent::Focused(true) => {
                w.event(event);
                self.chat_toolbar_dirty = true;
                self.chat_window_refresh();
            }
            WindowEvent::RedrawRequested => {
                for e in w.redraw() {
                    if self.chat_window.is_some() {
                        self.chat_window_act(e, event_loop);
                    }
                }
            }
            other => {
                w.event(other);
            }
        }
        true
    }

    fn chat_window_act(&mut self, e: ChatEvent, event_loop: &ActiveEventLoop) {
        match e {
            ChatEvent::Close => self.close_chat_window(),
            ChatEvent::Provider(id) => self.chat_pick_provider(&id),
            ChatEvent::Model(name) => self.chat_pick_model(&name),
            ChatEvent::Effort(_) | ChatEvent::Tier(_) | ChatEvent::ToggleThinking | ChatEvent::ToggleWebSearch => self.chat_toolbar_pick(&e),
            // The Chat Info menu: Task 6.
            ChatEvent::SessionMenu(_) => {}
            ChatEvent::Select(id) => self.show_chat(Some(id)),
            ChatEvent::NewChat => self.create_window_chat(None),
            ChatEvent::Rename(id) => self.rename_chat(&id, event_loop),
            ChatEvent::Delete(ids) => self.ask_ai(chat_list::delete_alert(ids)),
            ChatEvent::ListMenu { at, ids } => self.chat_list_menu(at, ids),
        }
    }

    /// The list's right-click menu at `at` (screen pixels) for the selected chats (`CL:393-414`);
    /// its picks come back through `run_menu_id` as `chat_list_picked`.
    fn chat_list_menu(&mut self, at: (f32, f32), ids: Vec<String>) {
        use muda::ContextMenu;
        let Some(hwnd) = self.chat_window.as_ref().and_then(|w| w.hwnd()) else { return };
        let menu = muda::Menu::new();
        let mut results = Vec::new();
        for entry in chat_list::context_menu(ids.len()) {
            results.push(match entry {
                None => menu.append(&muda::PredefinedMenuItem::separator()),
                Some(title) => {
                    let id = if title == chat_list::RENAME_TITLE { RENAME_ID } else { DELETE_ID };
                    menu.append(&muda::MenuItem::with_id(id, title, true, None))
                }
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Chat menu: {e}"));
            return;
        }
        self.chat_menu_ids = ids;
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// A pick from the list's menu. False for other menu ids.
    pub(super) fn chat_list_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        match id {
            RENAME_ID => {
                if let Some(chat) = self.chat_menu_ids.first().cloned() {
                    self.rename_chat(&chat, event_loop);
                }
                true
            }
            DELETE_ID => {
                let ids = std::mem::take(&mut self.chat_menu_ids);
                if !ids.is_empty() {
                    self.ask_ai(chat_list::delete_alert(ids));
                }
                true
            }
            _ => false,
        }
    }

    /// The rename window (`CWC:774-817`): the current title selected, OK reads "Rename".
    pub(super) fn rename_chat(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        if let Some((w, _)) = &self.chat_prompt {
            return w.focus();
        }
        let Some(title) = self.chat_db.as_ref().and_then(|db| db.chat(id).ok().flatten()).map(|c| c.title) else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::prompt_ui::PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, chat_list::RENAME_TITLE, chat_list::RENAME_PROMPT, &title) {
            Ok(w) => self.chat_prompt = Some((w.with_ok_label(chat_list::RENAME_BUTTON).with_selected(), id.to_string())),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the rename window. False when `window` is not it.
    pub(super) fn chat_prompt_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::PromptResult;
        let Some((w, _)) = self.chat_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.chat_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.chat_prompt = None,
                PromptResult::Ok(text) => {
                    if let Some((_, chat)) = self.chat_prompt.take() {
                        self.chat_renamed_to(&chat, &text);
                    }
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// OK in the rename window: a blank or unchanged name does nothing; else the rename is
    /// published from the agent side (12c's broker turns it into `Store::rename`), and a title
    /// that did not change asks `Rename Failed`.
    fn chat_renamed_to(&mut self, chat: &str, typed: &str) {
        let old = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()).map(|c| c.title);
        let Some(old) = old else { return self.ask_ai(chat_list::rename_failed_alert()) };
        let Some(title) = chat_list::rename_result(&old, typed) else { return };
        let m = Message::new(chat, Participant::Agent, Content::RenameChat(title.clone()), super::warnings_app::unix_now());
        let published = self.chat_publish(chat, m, false).is_ok();
        let now = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()).map(|c| c.title);
        if !published || now.as_deref() != Some(title.as_str()) {
            self.ask_ai(chat_list::rename_failed_alert());
        }
        self.chat_rows_dirty = true;
        self.chat_window_refresh();
    }

    /// Delete's answer (`CWC:844-872`): each chat once. A chat's title run is cancelled first
    /// (ruling F16), then its rows go; the window then shows the first remaining chat when the
    /// shown one went (`CWC:757-778`).
    pub(super) fn delete_chats(&mut self, ids: Vec<String>) {
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = ids.into_iter().filter(|i| seen.insert(i.clone())).collect();
        chat_list::cancel_runs(&mut self.title_runs, &ids, &mut |(mut runner, _)| {
            let _ = runner.cancel();
        });
        for id in &ids {
            self.chat_delete(id);
            self.ai_watermarks.remove(id);
            self.chat_snippets.remove(id);
        }
        let first = self.chat_db.as_ref().and_then(|db| db.chats().ok()).and_then(|chats| chats.first().map(|c| c.id.clone()));
        self.chat_rows_dirty = true;
        match chat_list::after_delete(self.chat_shown.as_deref(), &ids, first.as_deref()) {
            Some(next) => self.show_chat(next),
            None => self.chat_window_refresh(),
        }
    }

    /// After a chat's first turn: names it (`ChatAgent.requestRenaming`). The request is built
    /// only with the gate open for the chat model's own endpoint (`title_request`, ruling
    /// R12cd-1) and never asks for a key or shows an alert: the turn before it already did.
    /// Runs on the app-wide subscription, so a chat used only in a panel is named too (F14).
    pub(super) fn mint_title(&mut self, chat: &str) {
        if self.title_runs.contains_key(chat) {
            return;
        }
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let Some(db) = self.chat_db.as_ref() else { return };
        let (Ok(Some(row)), Ok(log)) = (db.chat(chat), db.messages(chat)) else { return };
        let g = &self.globals;
        let model = row.model_name.as_deref().and_then(|n| crate::ai_model::named(n, g, catalog, &found)).or_else(|| crate::ai_model::configured(g, catalog, &found));
        let Some(model) = model else { return };
        let Some((endpoint, messages)) = chat_list::title_request(g, &model, &log) else { return };
        // The proof that the gate is open right now, before anything is sent (ruling F15).
        if crate::chat_gate::pass(g, &endpoint).is_none() {
            return;
        }
        let vendor = model.vendor;
        let input = crate::ai_turn::TurnInput {
            model,
            messages,
            tools: Vec::new(),
            stream: false,
            hosted: crate::ai_turn::Hosted::default(),
            previous_response_id: None,
            // `newConversation.shouldThink = false` (`ChatAgent.swift:1885`).
            should_think: Some(false),
            effort: None,
            tier: None,
            volatile: Box::new(|| None),
            frozen: None,
            wire: crate::ai_turn::Wire::from_globals(g),
            legacy_url_pref: g.string(crate::ai_settings::URL),
        };
        let key = crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(vendor, &mut *crate::ai_keys::system_store());
        let mut runner = crate::ai_turn::Runner::new(crate::ai_turn::Turn::new(input), self.ai_transport.clone(), crate::ai_transport::options_from(g), self.notify.clone());
        let outs = runner.start(key.as_ref());
        match chat_list::title_outcome(&outs) {
            None => {
                self.title_runs.insert(chat.to_string(), (runner, endpoint));
            }
            Some(title) => self.title_done(chat, title),
        }
    }

    /// Polls the title runs: a run whose gate closed meanwhile (AI turned off, the admin
    /// switch, that host's consent forgotten) is cancelled and dropped first; the rest read what
    /// arrived, each send spending a fresh proof of the gate (ruling F15).
    pub(super) fn title_tick(&mut self) {
        if self.title_runs.is_empty() {
            return;
        }
        let g = &self.globals;
        let mut done: Vec<(String, Option<String>)> = Vec::new();
        self.title_runs.retain(|chat, (runner, endpoint)| {
            if !crate::chat_gate::gate_open(g, endpoint) {
                let _ = runner.cancel();
                return false;
            }
            let outs = runner.poll_with(&mut || crate::chat_gate::pass(g, endpoint));
            match chat_list::title_outcome(&outs) {
                None => true,
                Some(title) => {
                    done.push((chat.clone(), title));
                    false
                }
            }
        });
        for (chat, title) in done {
            self.title_done(&chat, title);
        }
    }

    /// A title run ended: a usable title is published from the agent side to a chat that still
    /// has its row; anything else leaves the chat's name (`ChatAgent.swift:1886-1906`).
    fn title_done(&mut self, chat: &str, title: Option<String>) {
        let Some(title) = title else { return };
        if !self.chat_db.as_ref().is_some_and(|db| db.chat(chat).ok().flatten().is_some()) {
            return;
        }
        let m = Message::new(chat, Participant::Agent, Content::RenameChat(title), super::warnings_app::unix_now());
        if let Err(e) = self.chat_publish(chat, m, false) {
            crate::ai_wire_log::console(self.globals.bool("aiChatVerboseConsoleLogging"), &format!("renameChat failed: {e}"));
        }
    }

    /// A message from the person, published in `chat`.
    #[allow(dead_code)] // Used by the tasks after Task 1.
    pub(super) fn publish_user(&mut self, chat: &str, c: Content) {
        let m = Message::new(chat, Participant::User, c, super::warnings_app::unix_now());
        let _ = self.chat_publish(chat, m, false);
    }

    /// A message from the agent, published in `chat`.
    #[allow(dead_code)] // Used by the tasks after Task 1.
    pub(super) fn publish_agent(&mut self, chat: &str, c: Content) {
        let m = Message::new(chat, Participant::Agent, c, super::warnings_app::unix_now());
        let _ = self.chat_publish(chat, m, false);
    }

    /// A notice in the conversation (`publishNotice`).
    #[allow(dead_code)] // Used by the tasks after Task 1.
    pub(super) fn notice(&mut self, chat: &str, text: &str) {
        self.client_local(chat, ClientLocal::Notice(text.to_string()));
    }

    /// An agent-authored client-local message.
    pub(super) fn client_local(&mut self, chat: &str, cl: ClientLocal) {
        self.publish_agent(chat, Content::ClientLocal(cl));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::{Chord, ChordKey};

    fn chord(c: char, shift: bool) -> Chord {
        Chord { key: ChordKey::Char(c), cmd: true, alt: false, shift, ctrl: false }
    }

    #[test]
    fn chat_chords_are_win_w_and_shift_win_y() {
        assert_eq!(chat_chord(&chord('w', false)), Some(ChatChord::Close));
        assert_eq!(chat_chord(&chord('y', true)), Some(ChatChord::Focus));
        assert_eq!(chat_chord(&chord('y', false)), None);
        assert_eq!(chat_chord(&chord('w', true)), None);
        assert_eq!(chat_chord(&Chord { ctrl: true, ..chord('w', false) }), None);
        assert_eq!(chat_chord(&Chord { alt: true, ..chord('w', false) }), None);
        assert_eq!(chat_chord(&Chord { cmd: false, ..chord('w', false) }), None);
    }

    #[test]
    fn a_pick_keeps_a_newer_permission() {
        let mut store = Store::open_in_memory().unwrap();
        let mut chat = Chat::new("T", 1.0);
        chat.permissions = "[]".into();
        store.add_chat(&chat).unwrap();
        // The window read the row, then a permission change was written, then a model pick lands.
        let read_earlier = store.chat(&chat.id).unwrap().unwrap();
        let mut changed = read_earlier.clone();
        changed.permissions = "[\"answered\"]".into();
        store.update_chat(&changed).unwrap();
        assert!(update_chat_in(&mut store, &chat.id, |c| c.model_name = Some("gpt".into())));
        let now = store.chat(&chat.id).unwrap().unwrap();
        assert_eq!(now.model_name.as_deref(), Some("gpt"));
        assert_eq!(now.permissions, "[\"answered\"]", "the pick did not revert the permission");
        assert!(!update_chat_in(&mut store, "gone", |c| c.model_name = None));
    }
}
