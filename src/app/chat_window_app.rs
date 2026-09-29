//! The AI Chat window in the app (`ChatWindowController.swift`): its entry points (Window > AI
//! Chats, Session > Open AI Chat...), the toolbar's data and picks, the thin publishers every
//! chat feature uses, and the window's events. The chat list, conversation, input and find are
//! filled by the tasks after this one; the window is app-wide (one per app, iTerm2's singleton).

use super::*;
use std::path::PathBuf;
use crate::ai_catalog::{Effort, Feature, ServiceTier};
use crate::ai_settings::Vendor;
use crate::chat_agent::{self, EFFORT_KEY, HIGH_EFFORT_KEY, TIER_KEY, WEB_SEARCH_KEY};
use crate::chat_message::{ClientLocal, Content, Message, Participant};
use crate::chat_store::{Chat, Store};
use crate::chat_list::{self, Row};
use crate::chat_toolbar::{self as tb, Reuse};
use crate::chat_view;
use crate::chat_input::{self, Draft};
use crate::chat_find::{self, Finder, Segments};
use crate::chat_window_ui::{convo_rows, ChatData, ChatEvent, ChatWindow, FindData, InputData, ToolbarData};
use crate::find::Mode;

/// `update_chat` from a row read just now, changing only what `f` names (ruling F6): the row's
/// permissions, model and every other column a window or list read earlier are never written
/// back from that older copy. False when the row is gone or the write failed.
pub(super) fn update_chat_in(store: &mut Store, id: &str, f: impl FnOnce(&mut Chat)) -> bool {
    let Ok(Some(mut row)) = store.chat(id) else { return false };
    f(&mut row);
    store.update_chat(&row).is_ok()
}

/// One broker update applied to the shown chat's kept messages; whether the rows must be built
/// again. A chunk, an attached file or a commit (`Append`, `AppendAttachment`, `Commit`) names a
/// stored message whose row the broker already changed, so `reload` reads that row again; a
/// whole message replaces its old copy or joins the end; a removal drops.
fn apply_update(msgs: &mut Vec<Message>, u: &crate::chat_broker::Update, reload: &dyn Fn(&str) -> Option<Message>) -> bool {
    use crate::chat_broker::Update;
    match u {
        Update::Delivery { message, .. } => match &message.content {
            Content::Append { uuid, .. } | Content::AppendAttachment { uuid, .. } | Content::Commit(uuid) => {
                let Some(at) = msgs.iter().position(|m| m.unique_id == *uuid) else { return false };
                let Some(fresh) = reload(uuid) else { return false };
                msgs[at] = fresh;
                true
            }
            _ => {
                match msgs.iter().position(|m| m.unique_id == message.unique_id) {
                    Some(at) => msgs[at] = message.clone(),
                    None => msgs.push(message.clone()),
                }
                true
            }
        },
        Update::Removed { ids, .. } => {
            let before = msgs.len();
            msgs.retain(|m| !ids.contains(&m.unique_id));
            msgs.len() != before
        }
        Update::Typing { .. } | Update::Turn(_) => false,
    }
}

/// The blobs of a fork's retained rounds (ruling F5): copied with fresh ids only when the
/// retained messages' references are exactly the source's first blobs, then each clone (found
/// through `ids`, old message id to clone id) points at its copy. Nothing is linked when
/// nothing was copied, so no clone keeps a reference into the source chat.
fn fork_blobs(store: &mut Store, source: &str, fork: &str, retained: &[Message], ids: &std::collections::HashMap<String, String>) -> Result<(), String> {
    let refs: Vec<String> = retained.iter().filter_map(|m| m.first_blob_ref.clone()).collect();
    let map = store.copy_blobs(source, fork, &refs)?;
    for m in retained {
        let (Some(new), Some(clone)) = (m.first_blob_ref.as_ref().and_then(|r| map.get(r)), ids.get(&m.unique_id)) else { continue };
        store.set_first_blob_ref(fork, clone, new)?;
    }
    Ok(())
}

/// The list menu's ids (picked through `run_menu_id`).
const RENAME_ID: &str = "aichat:rename";
const DELETE_ID: &str = "aichat:delete";
/// The attach popup's items.
const ATTACH_FILES_ID: &str = "aichat:attach:files";
const ATTACH_FOLDER_ID: &str = "aichat:attach:folder";
/// Most paths one window gathers from a drop before the next pass asks about them.
const DROPS_MAX: usize = 1000;

/// What a chord does in the chat window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatChord {
    Close,
    Focus,
    /// Win+F, Win+G, Shift+Win+G and Win+E: the find bar (`CVC:2917-2960`).
    Find,
    FindNext,
    FindPrevious,
    UseSelection,
}

/// Win+W closes (`CWC:481-501`); Shift+Win+Y focuses (Window > AI Chats); Win+F, Win+G,
/// Shift+Win+G and Win+E are the find chords.
pub(super) fn chat_chord(c: &crate::menus::Chord) -> Option<ChatChord> {
    match c.key {
        crate::menus::ChordKey::Char('w') if crate::winkeys::chat_chord(c) => Some(ChatChord::Close),
        crate::menus::ChordKey::Char('y') if crate::winkeys::chat_chord(c) => Some(ChatChord::Focus),
        crate::menus::ChordKey::Char('f') if crate::winkeys::chat_chord(c) => Some(ChatChord::Find),
        crate::menus::ChordKey::Char('g') if crate::winkeys::chat_chord(c) => Some(if c.shift { ChatChord::FindPrevious } else { ChatChord::FindNext }),
        crate::menus::ChordKey::Char('e') if crate::winkeys::chat_chord(c) => Some(ChatChord::UseSelection),
        _ => None,
    }
}

/// The chat window's find bar while it is open: the matches, the bar's mode, the findable
/// texts of each row (kept by the row's content key, so a streamed chunk renders one row again),
/// a counter that asks the field for focus, and whether the rows changed since the last scan.
#[derive(Default)]
pub(super) struct ChatFind {
    finder: Option<Finder>,
    mode: Mode,
    texts: std::collections::HashMap<u64, Vec<String>>,
    focus: u64,
    dirty: bool,
}

/// The plain text of a markdown source as the window renders it (the offsets find works in).
fn markdown_plain(md: &str) -> String {
    crate::porthole::render::render(md, &crate::porthole::detect::Language::Markdown, &crate::palette::Palette::default(), false).plain()
}

impl App {
    /// Win+F, or Use Selection for Find: opens the bar seeded with the last query (or `seed`),
    /// the field focused with its text selected, and searches (`CVC:2986-3006`).
    pub(super) fn chat_find_open(&mut self, seed: Option<String>) {
        if self.chat_shown.is_none() || self.chat_window.is_none() {
            return;
        }
        if let Some(q) = seed {
            self.chat_find_query = q;
        }
        // Rows changed while the bar was closed were never scanned: read every row again.
        self.chat_find.texts.clear();
        let mut finder = self.chat_find.finder.take().unwrap_or_default();
        let segments = self.chat_find_segments();
        finder.search(&self.chat_find_query.clone(), self.chat_find.mode, &segments);
        self.chat_find.finder = Some(finder);
        self.chat_find.focus += 1;
        self.chat_find.dirty = false;
        self.chat_window_refresh();
    }

    /// Win+G and Shift+Win+G (and the bar's arrows): the next or previous match.
    pub(super) fn chat_find_step(&mut self, forward: bool) {
        let Some(f) = self.chat_find.finder.as_mut() else { return };
        if forward {
            f.next();
        } else {
            f.previous();
        }
        self.chat_window_refresh();
    }

    /// The bar's field or mode changed.
    fn chat_find_query(&mut self, query: String, mode_index: usize) {
        if self.chat_find.finder.is_none() {
            return;
        }
        let mode = chat_find::MODES.get(mode_index).map_or(Mode::SmartCase, |m| m.1);
        self.chat_find_query = query;
        self.chat_find.mode = mode;
        let segments = self.chat_find_segments();
        if let Some(f) = self.chat_find.finder.as_mut() {
            f.search(&self.chat_find_query, mode, &segments);
        }
        self.chat_window_refresh();
    }

    /// Closes the bar and forgets its matches (`CVC:3041-3050`).
    pub(super) fn chat_find_close(&mut self) {
        if let Some(mut f) = self.chat_find.finder.take() {
            f.clear();
            self.chat_find.texts.clear();
            self.chat_window_refresh();
        }
    }

    /// The conversation changed: the same query again over the new rows, keeping the current
    /// match. Called once per refresh, so a burst of chunks is one scan.
    fn chat_find_rescan(&mut self) {
        if !std::mem::take(&mut self.chat_find.dirty) || self.chat_find.finder.is_none() {
            return;
        }
        let segments = self.chat_find_segments();
        if let Some(f) = self.chat_find.finder.as_mut() {
            f.rescan(&segments);
        }
    }

    /// Each message row's findable texts, as the window draws them.
    fn chat_find_segments(&mut self) -> Vec<Segments> {
        let mut found = Vec::new();
        let texts = &mut self.chat_find.texts;
        if texts.len() > 4000 {
            texts.clear();
        }
        for (i, row) in self.chat_convo.iter().enumerate() {
            if matches!(row.item, chat_view::Item::Message(_)) {
                let segs = texts.entry(row.key).or_insert_with(|| chat_find::body_segments(&row.body, &markdown_plain)).clone();
                found.push((row.id.clone(), i, segs));
            }
        }
        found
    }

    /// The find bar's data for the window.
    fn chat_find_data(&self) -> Option<FindData> {
        let f = self.chat_find.finder.as_ref()?;
        Some(FindData { query: f.query.clone(), mode_index: chat_find::popup_index(f.mode), counter: f.counter(), matches: f.matches.clone(), current: f.current, focus: self.chat_find.focus })
    }
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
        self.chat_rows_dirty = true;
        self.chat_window_refresh();
        true
    }

    /// The gate, the database and the window, in iTerm2's order (`showChatWindow`,
    /// `CWC:250-258`). After a failed open nothing more happens (checklist row 42, ruling F38).
    pub(super) fn chat_window_enter(&mut self, event_loop: &ActiveEventLoop) -> bool {
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
        let Some(reference) = self.session_ref(pane) else {
            if let Some(text) = super::chat_app::links_off_text(self.chat_links_off) {
                self.banners.set("chat_links", text.to_string());
            }
            return;
        };
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

    /// Shows `chat` (or none) and ends the search: a chat chosen from the list, "+", or any
    /// other selection clears it (`CL:315-322`).
    pub(super) fn show_chat(&mut self, chat: Option<String>) {
        self.chat_search.set_query("", Instant::now());
        self.show_chat_in_search(chat);
    }

    /// Shows `chat` and leaves the search as it is (a chosen result keeps search mode,
    /// `CL:522-527`): the title, the toolbar and the data follow.
    fn show_chat_in_search(&mut self, chat: Option<String>) {
        // Showing another chat ends the streaming of the one this view showed (`CVC:843-847`).
        if let Some(old) = self.chat_shown.clone().filter(|old| Some(old) != chat.as_ref()) {
            self.stop_streaming(&old);
        }
        // Showing another chat closes the find bar (`CVC:891-893`).
        if self.chat_shown != chat {
            self.chat_find_close();
        }
        self.chat_input_switch(chat.as_deref());
        self.chat_shown = chat;
        self.chat_load_messages();
        // A chat whose last state is Active with nothing streaming it was left on by an earlier
        // run: it says so (`CVC:1006-1011`).
        if let Some(shown) = self.chat_shown.clone() {
            if crate::chat_menu::stale_streaming(crate::chat_menu::last_streaming(&self.chat_msgs), self.streaming_chats.contains(&shown)) {
                self.client_local(&shown, ClientLocal::StreamingChanged(crate::chat_message::Streaming::StoppedAutomatically));
            }
        }
        self.chat_toolbar_dirty = true;
        self.chat_rows_dirty = true;
        self.chat_window_refresh();
    }

    fn chat_window_title(&self) -> String {
        self.chat_shown.as_deref().and_then(|id| self.chat_db.as_ref()?.chat(id).ok().flatten()).map(|c| c.title).filter(|t| !t.is_empty()).unwrap_or_else(|| tb::WINDOW_TITLE.to_string())
    }

    /// Rebuilds what the window shows and hands it over when it changed. The toolbar's model
    /// list and key state are computed only when marked dirty (ruling F17), never per chunk.
    pub(super) fn chat_window_refresh(&mut self) {
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
        if self.chat_convo_dirty {
            self.chat_convo_build();
        }
        self.chat_find_rescan();
        self.chat_buttons_refresh();
        let title = self.chat_window_title();
        self.chat_toolbar.title = title.clone();
        let data = ChatData {
            chat: self.chat_shown.clone(),
            toolbar: self.chat_toolbar.clone(),
            rows: self.chat_rows.clone(),
            query: self.chat_search.query.clone(),
            search: self.chat_search.hits().to_vec(),
            searching: self.chat_search.searching(),
            convo: self.chat_convo.clone(),
            input: self.chat_input_data(),
            find: self.chat_find_data(),
        };
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
        let read = |id: &str| db.messages(id).map(|m| chat_list::row_snippet(&m)).unwrap_or_default();
        let snippet = |id: &str| chat_list::cached_snippet(&mut cache.borrow_mut(), id, &read);
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
        let mut changed = false;
        self.inline_sweep();
        let dropped = self.chat_drops_flush();
        self.pick_tick();
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
                self.chat_apply_updates(&updates);
                let shown = self.chat_shown.as_deref();
                if updates.iter().any(|(chat, u)| Some(chat.as_str()) == shown && matches!(u, crate::chat_broker::Update::Delivery { partial: false, .. } | crate::chat_broker::Update::Removed { .. })) {
                    self.chat_toolbar_dirty = true;
                }
            }
        }
        let mut due = (!self.title_runs.is_empty()).then(|| Instant::now() + std::time::Duration::from_millis(250));
        if self.chat_window.is_some() {
            let (search_changed, search_due) = self.chat_search_tick();
            changed |= search_changed;
            due = [due, search_due].into_iter().flatten().min();
            let minute = self.chat_rows_built.map(|b| b + std::time::Duration::from_secs(60));
            if minute.is_some_and(|t| Instant::now() >= t) {
                self.chat_rows_dirty = true;
                // A day may have turned: the date rows' labels are relative to now.
                self.chat_convo_dirty = true;
            }
            if let Some(w) = self.chat_window.as_mut() {
                due = [due, w.tick(Instant::now())].into_iter().flatten().min();
            }
            if changed || self.chat_rows_dirty || self.chat_convo_dirty {
                self.chat_window_refresh();
            }
            let minute = self.chat_rows_built.map(|b| b + std::time::Duration::from_secs(60));
            due = [due, minute].into_iter().flatten().min();
        }
        // A drop question just queued opens on the next pass.
        if dropped {
            due = Some(Instant::now());
        }
        due
    }

    /// The search's turn: the store is asked once the person paused typing (ruling F28), the
    /// hits are made in slices of at most `SLICE`, and the next pass is asked for while any
    /// remain. Returns whether the window's data changed and when to run again.
    fn chat_search_tick(&mut self) -> (bool, Option<Instant>) {
        let Some(db) = self.chat_db.as_ref() else { return (false, None) };
        let mut search = |q: &str| db.search(q);
        let mut messages = |chat: &str| db.messages(chat);
        let t = self.chat_search.tick(Instant::now(), &mut search, &mut messages);
        if let Some(e) = t.error {
            // Store errors are logged, never shown (`chat_db`'s policy).
            crate::ai_wire_log::console(self.globals.bool("aiChatVerboseConsoleLogging"), &format!("chat search: {e}"));
            self.banners.set("chat_search", "The search could not be completed.".to_string());
        }
        (t.changed, t.next)
    }

    /// The search field changed: a new query drops the old work (`CSR:24-31`).
    fn chat_search_query(&mut self, query: &str) {
        if self.chat_search.set_query(query, Instant::now()) {
            self.chat_window_refresh();
        }
    }

    /// A result was clicked: its chat shows, search mode stays (`CL:522-527`), and the message
    /// is centered (`CWC:822-826`, `CVC:1069-1098`).
    fn chat_choose_hit(&mut self, index: usize) {
        let Some((chat, message)) = self.chat_search.hits().get(index).map(|h| (h.chat.clone(), h.message.clone())) else { return };
        self.show_chat_in_search(Some(chat));
        if let Some(w) = self.chat_window.as_mut() {
            w.reveal(&message);
        }
    }

    /// The shown chat's messages, read from the store (a failed read shows a fixed notice).
    fn chat_load_messages(&mut self) {
        self.chat_convo_dirty = true;
        self.chat_msgs = Vec::new();
        let Some(id) = self.chat_shown.clone() else { return };
        match self.chat_db.as_ref().map(|db| db.messages(&id)) {
            Some(Ok(m)) => self.chat_msgs = m,
            Some(Err(_)) => self.banners.set("chat_read", "The chat could not be read.".to_string()),
            None => {}
        }
    }

    /// The conversation's rows from the kept messages (dates in the person's locale).
    fn chat_convo_build(&mut self) {
        self.chat_convo_dirty = false;
        self.chat_find.dirty = true;
        let typing = self.chat_shown.as_deref().is_some_and(|c| self.chat_broker.is_typing(c, Participant::Agent));
        let local = |t: f64| crate::stamps::local(chat_view::epoch_secs(t));
        let now = crate::stamps::local(chat_view::epoch_secs(super::warnings_app::unix_now()));
        let day = |t: f64| {
            let p = local(t);
            (i32::from(p.year), u32::from(p.month), u32::from(p.day))
        };
        let label = |t: f64| chat_view::date_text(&local(t), &now);
        let time = |t: f64| crate::stamps::short_time(&local(t));
        let chat = self.chat_shown.clone().unwrap_or_default();
        let row = self.chat_db.as_ref().and_then(|db| db.chat(&chat).ok().flatten());
        let for_message = |m: &Message, last: bool| self.chat_row_buttons(&chat, row.as_ref(), &self.chat_msgs, m, last);
        self.chat_convo = std::sync::Arc::new(convo_rows(&self.chat_msgs, typing, &day, &label, &time, &for_message));
    }

    /// What the broker delivered for the shown chat, kept in `chat_msgs`. A chunk, an attached
    /// file or a commit (`Append`, `AppendAttachment`, `Commit`) names a stored message whose
    /// row the broker already changed, so that row is read again; a whole message replaces or
    /// joins; a removal drops.
    fn chat_apply_updates(&mut self, updates: &[(String, crate::chat_broker::Update)]) {
        let Some(shown) = self.chat_shown.clone() else { return };
        let db = self.chat_db.as_ref();
        let reload = |id: &str| db.and_then(|db| db.message(id).ok().flatten());
        let mut changed = false;
        for (_, u) in updates.iter().filter(|(chat, _)| *chat == shown) {
            // The typing row is part of the rows, though no message changes.
            changed |= apply_update(&mut self.chat_msgs, u, &reload) | matches!(u, crate::chat_broker::Update::Typing { .. });
        }
        self.chat_convo_dirty |= changed;
    }

    /// A visible message of the shown chat.
    fn chat_visible_message(&self, id: &str) -> Option<&Message> {
        chat_view::visible(&self.chat_msgs).into_iter().find(|m| m.unique_id == id)
    }

    /// The message menu at `at` (screen pixels), built from `chat_view::message_menu`; its picks
    /// come back through `run_menu_id` as `chat_message_picked`.
    fn chat_message_menu(&mut self, at: (f32, f32), id: String) {
        use muda::ContextMenu;
        let Some(hwnd) = self.chat_window.as_ref().and_then(|w| w.hwnd()) else { return };
        let Some(editable) = self.chat_visible_message(&id).map(chat_view::editable) else { return };
        let menu = muda::Menu::new();
        for title in chat_view::message_menu(editable) {
            let item = muda::MenuItem::with_id(format!("{}{}:{id}", chat_view::MENU_PREFIX, title.to_lowercase()), title, true, None);
            if let Err(e) = menu.append(&item) {
                self.banners.set("menu", format!("Chat menu: {e}"));
                return;
            }
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// A pick from the message menu. False for other menu ids.
    pub(super) fn chat_message_picked(&mut self, menu_id: &str) -> bool {
        let Some(rest) = menu_id.strip_prefix(chat_view::MENU_PREFIX) else { return false };
        let Some((verb, id)) = rest.split_once(':') else { return true };
        let id = id.to_string();
        match verb {
            "edit" => self.chat_edit(&id),
            "copy" => self.chat_copy_message(&id),
            "fork" => self.chat_fork(&id),
            "delete" => self.chat_delete_from(&id),
            _ => {}
        }
        true
    }

    /// Text on the clipboard.
    fn chat_copy(&mut self, text: String) {
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    /// Copy in the message menu: the rendered text (`Doc::plain` for markdown).
    fn chat_copy_message(&mut self, id: &str) {
        let Some(text) = self.chat_visible_message(id).map(|m| chat_view::copy_text(&chat_view::body(&m.content))) else { return };
        self.chat_copy(text);
    }

    /// The chat's ids from this message on (`messageIDs(fromIndex:)`), and the person's text when
    /// it is one of theirs.
    fn chat_from_message(&self, id: &str) -> Option<(String, Vec<String>, Option<String>)> {
        let chat = self.chat_shown.clone()?;
        let visible = chat_view::visible(&self.chat_msgs);
        let m = visible.iter().find(|m| m.unique_id == id)?;
        let text = match (&m.content, chat_view::editable(m)) {
            (Content::PlainText { text, .. }, true) => Some(text.clone()),
            _ => None,
        };
        Some((chat, chat_view::ids_from(&visible, id), text))
    }

    /// Edit (`CVC:1555-1571`): this message and every later one are removed through the broker
    /// and the text goes back to the input.
    fn chat_edit(&mut self, id: &str) {
        let Some((chat, ids, Some(text))) = self.chat_from_message(id) else { return };
        self.chat_remove(&chat, &ids);
        self.set_input(&chat, &text);
    }

    /// Delete (`CVC:1573-1584`): the same truncation, without the input.
    fn chat_delete_from(&mut self, id: &str) {
        let Some((chat, ids, _)) = self.chat_from_message(id) else { return };
        self.chat_remove(&chat, &ids);
    }

    /// Puts text in the shown chat's input (Edit, a fork's staged message): the text is replaced
    /// and kept from a selection offer. Another chat's input is not touched.
    pub(super) fn set_input(&mut self, chat: &str, text: &str) {
        if self.chat_shown.as_deref() != Some(chat) {
            return;
        }
        self.chat_input.set(text);
        self.chat_window_refresh();
    }

    /// A terminal selection offered to the shown chat's input (`offerSelectedText`): it replaces
    /// the input's text for as long as nobody has typed in it.
    pub(super) fn offer_text(&mut self, chat: &str, text: &str) {
        if self.chat_shown.as_deref() == Some(chat) && self.chat_input.offer(text) {
            self.chat_window_refresh();
        }
    }

    /// Fork (`CWC:876-990`): a new chat holding the messages before this one (renames dropped,
    /// each cloned with a fresh id), the source's terminal, browser, permissions and icon, and
    /// copies of its blobs for the retained rounds linked to the clones (ruling F5); the new
    /// chat shows and the chosen message is staged in its input.
    fn chat_fork(&mut self, id: &str) {
        let Some(source_id) = self.chat_shown.clone() else { return };
        let Some(db) = self.chat_db.as_ref() else { return };
        let (Ok(Some(source)), Ok(all)) = (db.chat(&source_id), db.messages(&source_id)) else { return };
        let Some(index) = all.iter().position(|m| m.unique_id == id) else { return };
        let retained = &all[..index];
        let mut ids = std::collections::HashMap::new();
        let clones: Vec<Message> = retained.iter().filter(|m| !matches!(m.content, Content::RenameChat(_))).map(|m| chat_view::clone_for_fork(m, &mut ids)).collect();
        let now = crate::stamps::local(chat_view::epoch_secs(super::warnings_app::unix_now()));
        let stamp = format!("{} {}", crate::stamps::short_date(&now), crate::stamps::short_time(&now));
        let title = chat_view::fork_title(&source.title, &stamp);
        let Some(fork) = self.chat_create(&title, source.terminal_session_guid.as_deref(), source.browser_session_guid.as_deref(), &source.permissions, clones) else { return };
        let verbose = self.globals.bool("aiChatVerboseConsoleLogging");
        // The icon: not fatal when it cannot be copied (`CWC:920-932`); the row is read again first.
        if let Some(icon) = source.icon.clone() {
            if !self.chat_update(&fork, |c| c.icon = Some(icon)) {
                crate::ai_wire_log::console(verbose, "fork: the icon could not be copied");
            }
        }
        if let Some(db) = self.chat_db.as_mut() {
            if let Err(e) = fork_blobs(db, &source_id, &fork, retained, &ids) {
                crate::ai_wire_log::console(verbose, &format!("fork: the blobs could not be copied: {e}"));
            }
        }
        self.show_chat(Some(fork.clone()));
        if let Content::PlainText { text, .. } = &all[index].content {
            self.set_input(&fork, text);
        }
    }

    /// A markdown link: opened only when it is a web or mail link (ruling F18); the drawing
    /// already refuses the rest, and this is the second check.
    fn chat_open_url(&mut self, url: &str) {
        let Some(url) = chat_view::web_link(url) else { return };
        if let Err(e) = super::semantic_ui::shell_open(&url, None, false) {
            self.banners.set("action", format!("Could not open {url}: {e}"));
        }
    }

    /// A terminal command row was clicked: only its reveal-mark route runs; any other route in
    /// the URL is ignored (minor m10).
    fn chat_reveal_command(&mut self, url: &str, event_loop: &ActiveEventLoop) {
        if let Some(route) = crate::chat_routes::from_command_cell(url) {
            self.run_chat_route(&route, event_loop);
        }
    }

    /// The file a file item names: written under the chat's temp folder through
    /// `chat_view::materialize` only (ruling R12cd-6), from the bytes in the message itself.
    fn chat_attachment_file(&self, reference: &str) -> Result<(PathBuf, String), String> {
        use crate::chat_json::Attachment;
        let (message, attachment) = chat_view::split_file_ref(reference).ok_or("not an attachment")?;
        let chat = self.chat_shown.as_deref().ok_or("no chat is shown")?;
        let m = self.chat_msgs.iter().find(|m| m.unique_id == message).ok_or("the message is gone")?;
        let Content::Multipart { parts, .. } = &m.content else { return Err("the message has no attachments".to_string()) };
        let found = parts.iter().find_map(|p| match p {
            crate::chat_message::Subpart::Attachment(v) => crate::chat_json::attachment(v).filter(|(id, _)| id == attachment).map(|(_, a)| a),
            _ => None,
        });
        let (name, bytes) = match found.ok_or("the attachment is gone")? {
            Attachment::File { name, content, .. } => (name, content),
            Attachment::Code(code) => (chat_view::CODE_FILE.to_string(), code.into_bytes()),
            _ => return Err("the attachment has no local bytes".to_string()),
        };
        let root = std::env::temp_dir().join(chat_view::ATTACHMENTS_DIR);
        chat_view::materialize(&root, chat, attachment, &name, &bytes).map(|p| (p, name))
    }

    /// Reveal in Explorer, or Open (only for an inert type, ruling F19; anything else is revealed).
    fn chat_file(&mut self, reference: &str, open: bool) {
        match self.chat_attachment_file(reference) {
            Ok((path, name)) => {
                let text = path.to_string_lossy().to_string();
                if open && chat_view::openable(&name) {
                    if let Err(e) = super::semantic_ui::shell_open(&text, None, false) {
                        self.banners.set("action", format!("Could not open {text}: {e}"));
                    }
                } else {
                    self.reveal_in_explorer(&text);
                }
            }
            Err(e) => {
                crate::ai_wire_log::console(self.globals.bool("aiChatVerboseConsoleLogging"), &format!("attachment: {e}"));
                self.banners.set("action", "The attachment could not be written.".to_string());
            }
        }
    }

    /// Closing, or Close (Win+W): streaming stops, the subscription ends, the window goes
    /// (`CWC:481-501`). Nothing restores it at relaunch.
    pub(super) fn close_chat_window(&mut self) {
        self.pick_window_closed();
        self.chat_drops.clear();
        self.chat_input_switch(None);
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
        self.chat_find = Default::default();
        self.chat_shown = None;
        self.chat_msgs.clear();
        self.chat_convo = Default::default();
        self.chat_convo_dirty = true;
        self.chat_search.set_query("", Instant::now());
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
            ChatChord::Find => self.chat_find_open(None),
            ChatChord::FindNext => self.chat_find_step(true),
            ChatChord::FindPrevious => self.chat_find_step(false),
            // The window answers with its selected text on its next frame.
            ChatChord::UseSelection => {
                if let Some(w) = self.chat_window.as_mut() {
                    w.request_selection();
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
            // One event per dropped path: they are gathered and asked about together (ruling F27).
            WindowEvent::DroppedFile(path) => {
                if self.chat_drops.len() < DROPS_MAX {
                    self.chat_drops.push(path.clone());
                }
            }
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
            ChatEvent::SessionMenu(at) => {
                if let Some(chat) = self.chat_shown.clone() {
                    self.show_session_menu(&chat, None, at);
                }
            }
            ChatEvent::Select(id) => self.show_chat(Some(id)),
            ChatEvent::NewChat => self.create_window_chat(None),
            ChatEvent::Search(q) => self.chat_search_query(&q),
            ChatEvent::ChooseHit(i) => self.chat_choose_hit(i),
            ChatEvent::Rename(id) => self.rename_chat(&id, event_loop),
            ChatEvent::Delete(ids) => self.ask_ai(chat_list::delete_alert(ids)),
            ChatEvent::ListMenu { at, ids } => self.chat_list_menu(at, ids),
            ChatEvent::MessageMenu { at, id } => self.chat_message_menu(at, id),
            ChatEvent::CopyText(text) => self.chat_copy(text),
            ChatEvent::Edit(id) => self.chat_edit(&id),
            ChatEvent::DeleteMessage(id) => self.chat_delete_from(&id),
            ChatEvent::Fork(id) => self.chat_fork(&id),
            ChatEvent::OpenUrl(url) => self.chat_open_url(&url),
            ChatEvent::OpenFile(r) => self.chat_file(&r, true),
            ChatEvent::RevealFile(r) => self.chat_file(&r, false),
            ChatEvent::RevealCommand(url) => self.chat_reveal_command(&url, event_loop),
            ChatEvent::InputChanged(text) => {
                self.chat_input.typed(text);
                self.chat_window_refresh();
            }
            ChatEvent::FindQuery(q, mode) => self.chat_find_query(q, mode),
            ChatEvent::FindNext => self.chat_find_step(true),
            ChatEvent::FindPrevious => self.chat_find_step(false),
            ChatEvent::FindClose => self.chat_find_close(),
            ChatEvent::FindClick(c) => {
                if let Some(f) = self.chat_find.finder.as_mut() {
                    f.set_cursor(Some(c));
                }
            }
            // A selection is text for the field; nothing selected changes nothing.
            ChatEvent::FindUseSelection(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    self.chat_find_open(Some(text.to_string()));
                }
            }
            ChatEvent::Send => self.send_input(),
            ChatEvent::Stop => self.stop_chat(),
            ChatEvent::Attach(at) => self.chat_attach_menu(at),
            ChatEvent::RemoveAttachment(i) => {
                self.chat_input.remove_attachment(i);
                self.chat_window_refresh();
            }
            ChatEvent::Button { message, button } => {
                if let Some(chat) = self.chat_shown.clone() {
                    self.chat_button_pressed(&chat, &message, &button, event_loop);
                }
            }
        }
    }

    /// The input's draft moves with the shown chat (`CVC:864-894`): the outgoing chat's text and
    /// attachments are kept, the incoming chat's come back. The same chat changes nothing.
    fn chat_input_switch(&mut self, to: Option<&str>) {
        let from = self.chat_shown.clone();
        if from.as_deref() == to {
            return;
        }
        let current = std::mem::take(&mut self.chat_input.draft);
        let next = self.chat_drafts.switch(from.as_deref(), current, to);
        self.chat_input.switched(next);
    }

    /// What the input shows now.
    fn chat_input_data(&self) -> InputData {
        let shown = self.chat_shown.as_deref();
        let stoppable = shown.is_some_and(|c| self.chat_broker.is_typing(c, Participant::Agent));
        let orchestrating = shown.and_then(|c| self.chat_db.as_ref()?.chat(c).ok().flatten()).is_some_and(|c| c.orchestration_enabled);
        let text = self.chat_input.draft.text.clone();
        InputData {
            send_enabled: chat_input::send_enabled(&text, false, stoppable),
            text,
            pending: self.chat_input.draft.pending.iter().map(chat_input::Pending::label).collect(),
            placeholder: chat_input::placeholder(orchestrating).to_string(),
            stoppable,
            loading: false,
            focus: self.chat_input.focus,
        }
    }

    /// The model the shown chat's attachments are checked against: its own, else the default.
    fn chat_input_model(&mut self, chat: &str) -> Option<crate::ai_catalog::Model> {
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let name = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()).and_then(|c| c.model_name);
        name.and_then(|n| crate::ai_model::named(&n, &self.globals, catalog, &found)).or_else(|| crate::ai_model::configured(&self.globals, catalog, &found))
    }

    /// Send (`sendButtonClicked`, `CVC:2269-2388`): the text and the pending files become one
    /// user message published to the chat; the agent adds the terminal state itself. Nothing
    /// goes while the agent types (the button is Stop then) or for whitespace-only text; the
    /// input clears only when the message was published.
    pub(super) fn send_input(&mut self) {
        let Some(chat) = self.chat_shown.clone() else { return };
        if self.chat_broker.is_typing(&chat, Participant::Agent) || !chat_input::send_enabled(&self.chat_input.draft.text, false, false) {
            return;
        }
        let model = self.chat_input_model(&chat);
        let accepts = |name: &str| model.as_ref().is_some_and(|m| crate::ai_model::accepts(m, crate::ai_files::upload_mime(name)));
        let budget = std::cell::RefCell::new(chat_input::Budget::default());
        let read = |p: &std::path::Path| chat_input::read_capped(p, chat_input::FILE_BYTES_MAX, &mut |n| budget.borrow_mut().take(n, chat_input::TOTAL_BYTES_MAX));
        let list = |p: &std::path::Path| chat_input::list_folder(p, &accepts, chat_input::FOLDER_FILES_MAX);
        let mime = |n: &str| crate::ai_files::upload_mime(n).to_string();
        let draft = self.chat_input.draft.clone();
        let (parts, skipped) = chat_input::attachments(&draft.pending, &read, &list, &mime);
        let vector_store = self.chat_db.as_ref().and_then(|db| db.chat(&chat).ok().flatten()).and_then(|c| c.vector_store);
        let Some(content) = chat_input::message_content(&draft.text, parts, vector_store) else { return };
        let m = Message::new(&chat, Participant::User, content, super::warnings_app::unix_now());
        // The store and the window are not held across the publish: it may deliver at once.
        match self.chat_publish(&chat, m, false) {
            Ok(()) => {
                self.chat_input.sent();
                if !skipped.is_empty() {
                    self.notice(&chat, &chat_input::skipped_notice(&skipped));
                }
            }
            Err(e) => crate::ai_wire_log::console(self.globals.bool("aiChatVerboseConsoleLogging"), &format!("send: {e}")),
        }
        self.chat_window_refresh();
    }

    /// Stop (`stopButtonClicked`, `CVC:2256-2267`): 12c's `chat_stop` for the shown chat. Not the
    /// streaming bubble's Stop.
    pub(super) fn stop_chat(&mut self) {
        if let Some(chat) = self.chat_shown.clone() {
            self.chat_stop(&chat);
        }
    }

    /// The attach button's popup at `at` (screen pixels): files or a folder, two dialogs because
    /// Windows has no one for both. Its pick returns through `run_menu_id`.
    fn chat_attach_menu(&mut self, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.chat_window.as_ref().and_then(|w| w.hwnd()) else { return };
        let menu = muda::Menu::new();
        for (id, title) in [(ATTACH_FILES_ID, chat_input::ATTACH_FILES), (ATTACH_FOLDER_ID, chat_input::ATTACH_FOLDER)] {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(id, title, true, None)) {
                self.banners.set("menu", format!("Attach menu: {e}"));
                return;
            }
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// Paths chosen in a dialog: the ones the model takes wait for the next send, the others
    /// are named in an alert (`addFiles`, `IN:532-560`).
    fn chat_attach(&mut self, paths: &[PathBuf]) {
        let Some(chat) = self.chat_shown.clone() else { return };
        let model = self.chat_input_model(&chat);
        let accepts = |name: &str| model.as_ref().is_some_and(|m| crate::ai_model::accepts(m, crate::ai_files::upload_mime(name)));
        let (ok, no) = chat_input::split_by_model(paths, &|p| p.is_dir(), &accepts);
        if !no.is_empty() {
            let names: Vec<String> = no.iter().map(|p| chat_input::file_name(p)).collect();
            let provider = model.as_ref().map(|m| crate::ai_model::display_name(m));
            self.ask_ai(chat_input::rejected_alert(&names, provider));
        }
        chat_input::add_paths(&mut self.chat_input.draft.pending, &ok);
        self.chat_window_refresh();
    }

    /// The paths dropped on the window since the last pass, as one question (ruling F27): with
    /// nothing the model takes they are inserted as text; else the drop question asks (and its
    /// silence may answer at once).
    fn chat_drops_flush(&mut self) -> bool {
        if self.chat_drops.is_empty() {
            return false;
        }
        let paths = std::mem::take(&mut self.chat_drops);
        let Some(chat) = self.chat_shown.clone() else { return false };
        let model = self.chat_input_model(&chat);
        let accepts = |name: &str| model.as_ref().is_some_and(|m| crate::ai_model::accepts(m, crate::ai_files::upload_mime(name)));
        let (supported, _) = chat_input::split_by_model(&paths, &|p| p.is_dir(), &accepts);
        if supported.is_empty() {
            self.chat_input.insert(&chat_input::inserted_paths(&paths));
            self.chat_window_refresh();
            return false;
        }
        self.ask_ai(chat_input::drop_alert(&chat, supported, paths));
        true
    }

    /// The drop question's answer (`IN:518-527`): only the first button attaches. It applies
    /// only while the chat it was asked for is still shown.
    pub(super) fn chat_drop_answered(&mut self, chat: &str, supported: &[PathBuf], all: &[PathBuf], index: usize) {
        if self.chat_shown.as_deref() != Some(chat) {
            return;
        }
        let (attach, insert) = chat_input::drop_result(chat_input::drop_action(index), supported, all);
        chat_input::add_paths(&mut self.chat_input.draft.pending, &attach);
        self.chat_input.insert(&chat_input::inserted_paths(&insert));
        self.chat_window_refresh();
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
            ATTACH_FILES_ID => {
                if let Some(paths) = rfd::FileDialog::new().pick_files() {
                    self.chat_attach(&paths);
                }
                true
            }
            ATTACH_FOLDER_ID => {
                if let Some(paths) = rfd::FileDialog::new().pick_folders() {
                    self.chat_attach(&paths);
                }
                true
            }
            _ => self.chat_message_picked(id),
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
            self.chat_drafts.forget(id);
            if self.chat_shown.as_deref() == Some(id.as_str()) {
                self.chat_input.switched(Draft::default());
            }
            self.streaming_chats.remove(id);
            self.chat_delete(id);
            self.ai_watermarks.remove(id);
            self.chat_snippets.remove(id);
        }
        self.chat_refresh();
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
    fn find_chords_are_win_f_g_shift_g_and_e() {
        assert_eq!(chat_chord(&chord('f', false)), Some(ChatChord::Find));
        assert_eq!(chat_chord(&chord('g', false)), Some(ChatChord::FindNext));
        assert_eq!(chat_chord(&chord('g', true)), Some(ChatChord::FindPrevious));
        assert_eq!(chat_chord(&chord('e', false)), Some(ChatChord::UseSelection));
        assert_eq!(chat_chord(&chord('f', true)), None);
        assert_eq!(chat_chord(&chord('e', true)), None);
        assert_eq!(chat_chord(&Chord { ctrl: true, ..chord('f', false) }), None);
        assert_eq!(chat_chord(&Chord { alt: true, ..chord('g', false) }), None);
        assert_eq!(chat_chord(&Chord { cmd: false, ..chord('f', false) }), None);
        // The hook takes exactly the chords the window answers.
        for (k, shift) in [('w', false), ('y', true), ('f', false), ('g', false), ('g', true), ('e', false)] {
            assert!(crate::winkeys::chat_chord(&chord(k, shift)), "{k} {shift}");
        }
        assert!(!crate::winkeys::chat_chord(&chord('f', true)));
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

    #[test]
    fn a_fork_links_its_clones_to_copied_blobs() {
        use crate::chat_store::BlobRow;
        let blob = |id: &str| BlobRow { seq: 0, blob_id: id.into(), chat_id: "S".into(), protocol: 5, role: "user".into(), payload: id.as_bytes().to_vec(), response_id: None, token_count: None, wire_format_version: 1 };
        let mut store = Store::open_in_memory().unwrap();
        let mut source = Chat::new("Source", 1.0);
        source.id = "S".into();
        source.blob_protocol = Some("5".into());
        store.add_chat(&source).unwrap();
        for id in ["X1", "X2", "X3"] {
            store.add_blob(&blob(id)).unwrap();
        }
        let user = |text: &str, at: f64, blob: Option<&str>| {
            let mut m = Message::new("S", Participant::User, Content::PlainText { text: text.into(), context: None }, at);
            m.first_blob_ref = blob.map(str::to_string);
            m
        };
        let build = |store: &mut Store, kept: &[Message]| {
            let fork = Chat::new("Fork", 2.0);
            store.add_chat(&fork).unwrap();
            let mut ids = std::collections::HashMap::new();
            for m in kept {
                let clone = chat_view::clone_for_fork(m, &mut ids);
                store.add_message(&Message { chat_id: fork.id.clone(), ..clone }).unwrap();
            }
            (fork.id, ids)
        };
        // The first two rounds are retained: their blobs are copied under new ids and the clones follow.
        let kept = [user("one", 1.0, Some("X1")), Message::new("S", Participant::Agent, Content::Markdown("ok".into()), 2.0), user("two", 3.0, Some("X2"))];
        let (fork, ids) = build(&mut store, &kept);
        fork_blobs(&mut store, "S", &fork, &kept, &ids).unwrap();
        let copies = store.blobs(&fork).unwrap();
        assert_eq!(copies.len(), 2);
        assert!(copies.iter().all(|b| !["X1", "X2", "X3"].contains(&b.blob_id.as_str())), "fresh blob ids");
        let stored = store.messages(&fork).unwrap();
        let refs: Vec<Option<String>> = stored.iter().map(|m| m.first_blob_ref.clone()).collect();
        assert_eq!(refs, vec![Some(copies[0].blob_id.clone()), None, Some(copies[1].blob_id.clone())]);
        assert_eq!(store.chat(&fork).unwrap().unwrap().blob_protocol.as_deref(), Some("5"));
        // The source is untouched.
        assert_eq!(store.blobs("S").unwrap().len(), 3);
        // A gap (the second round retained without the first) copies nothing and links nothing.
        let gap = [user("two", 3.0, Some("X2"))];
        let (fork, ids) = build(&mut store, &gap);
        fork_blobs(&mut store, "S", &fork, &gap, &ids).unwrap();
        assert!(store.blobs(&fork).unwrap().is_empty());
        assert_eq!(store.messages(&fork).unwrap()[0].first_blob_ref, None);
    }

    #[test]
    fn updates_keep_the_shown_messages_current() {
        use crate::chat_broker::Update;
        let agent = |text: &str| Message::new("C", Participant::Agent, Content::Markdown(text.into()), 1.0);
        let first = agent("Hel");
        let mut msgs = vec![first.clone()];
        // A chunk is a delta: the message is read again from the store, which already holds the text.
        let stored = Message { content: Content::Markdown("Hello".into()), ..first.clone() };
        let chunk = Update::Delivery { message: Message::new("C", Participant::Agent, Content::Append { text: "lo".into(), uuid: first.unique_id.clone() }, 2.0), partial: true };
        assert!(apply_update(&mut msgs, &chunk, &|id| (id == first.unique_id).then(|| stored.clone())));
        assert_eq!(msgs, vec![stored.clone()]);
        // A commit reads the message again too; one for a message not kept changes nothing.
        let commit = Update::Delivery { message: Message::new("C", Participant::Agent, Content::Commit(first.unique_id.clone()), 3.0), partial: false };
        assert!(apply_update(&mut msgs, &commit, &|_| Some(Message { content: Content::Markdown("Hello!".into()), ..first.clone() })));
        assert_eq!(msgs[0].content, Content::Markdown("Hello!".into()));
        let stray = Update::Delivery { message: Message::new("C", Participant::Agent, Content::Commit("other".into()), 3.0), partial: false };
        assert!(!apply_update(&mut msgs, &stray, &|_| None));
        // A whole message joins the end; delivered again it replaces its copy.
        let second = agent("next");
        let whole = |m: &Message| Update::Delivery { message: m.clone(), partial: false };
        assert!(apply_update(&mut msgs, &whole(&second), &|_| None));
        assert_eq!(msgs.len(), 2);
        let edited = Message { content: Content::Markdown("edited".into()), ..second.clone() };
        assert!(apply_update(&mut msgs, &whole(&edited), &|_| None));
        assert_eq!((msgs.len(), &msgs[1].content), (2, &Content::Markdown("edited".into())));
        // A removal drops the ids; typing and turn events change no message.
        assert!(apply_update(&mut msgs, &Update::Removed { ids: vec![first.unique_id.clone()], chat: "C".into() }, &|_| None));
        assert_eq!(msgs, vec![edited]);
        assert!(!apply_update(&mut msgs, &Update::Removed { ids: vec!["none".into()], chat: "C".into() }, &|_| None));
        assert!(!apply_update(&mut msgs, &Update::Typing { on: true, who: Participant::Agent }, &|_| None));
        assert!(!apply_update(&mut msgs, &Update::Turn(crate::chat_message::TurnEvent::Started), &|_| None));
    }
}
