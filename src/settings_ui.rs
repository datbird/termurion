//! The settings window, laid out like iTerm2's profile editor: a profile list on the
//! left, the eight tabs across the top, and one control per schema setting.

use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::globals::Globals;
use crate::mouse::Mods;
use crate::profile::{self, Profile};
use crate::schema::{self, Kind, Setting};
use crate::settings_keys::{self, KeysState};
use crate::store::Store;

pub fn sections(tab: &str) -> Vec<(String, Vec<&'static Setting>)> {
    let mut out: Vec<(String, Vec<&'static Setting>)> = Vec::new();
    for s in schema::profile().iter().filter(|s| s.tab == tab) {
        match out.iter_mut().find(|(name, _)| *name == s.section) {
            Some((_, list)) => list.push(s),
            None => out.push((s.section.clone(), vec![s])),
        }
    }
    out
}

pub fn rgb_of(v: &Value) -> [u8; 3] {
    let t = v.as_table();
    let c = |k: &str| {
        let f = t.and_then(|t| t.get(k)).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
        (f.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [c("r"), c("g"), c("b")]
}

pub fn value_of_rgb(c: [u8; 3]) -> Value {
    profile::color_value(crate::palette::rgb(c[0], c[1], c[2]))
}

pub enum Edit {
    None,
    Changed(String),
    Imported(usize),
    Error(String),
    /// Open a session with this profile Guid.
    Open(String, How),
    /// A global setting changed (Keys or Pointer).
    GlobalsChanged,
    /// The snippets changed (Settings > Shortcuts).
    SnippetsChanged,
}

/// The settings window's panes, as iTerm2's toolbar lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Profiles,
    General,
    Keys,
    Shortcuts,
    Pointer,
    Advanced,
    Appearance,
    Arrangements,
}

/// Panes that arrive with later milestones.
const LATER_PAGES: [(&str, u8); 0] = [];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Tab,
    Window,
    SplitRight,
    SplitDown,
}

/// The settings editor, or iTerm2's Profiles window (Cmd+O): a list to start sessions from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Settings,
    Profiles,
}

pub struct SettingsWindow {
    host: crate::egui_host::EguiHost,
    selected: Option<String>,
    tab: usize,
    mode: Mode,
    page: Page,
    keys: KeysState,
    mods: Mods,
    /// The Automatic Profile Switching rule warning, until the next commit.
    aps_warning: Option<String>,
    arr: crate::settings_arrangements::State,
    shortcuts: crate::settings_shortcuts::State,
    /// Hotkey windows: the sheet, and registration failures from the app.
    pub hotkeys: crate::settings_hotkey::HotkeyUi,
    /// Profiles > Session's "Configure Status Bar" was pressed for this profile Guid: the
    /// status bar setup window opens for it (the app takes it after each frame).
    pub configure_status_bar: Option<String>,
    /// The window was opened from a tmux session (iTerm2's `_tmuxSession`, set by how the
    /// panel opened, `PreferencePanel.m:725-766`): its status bar controls are disabled.
    tmux_session: bool,
    /// `show_shortcuts_editor`'s sheet, opened on the next frame.
    open_editor: Option<(crate::settings_shortcuts::Tab, Option<usize>)>,
    /// The registered session title providers as (display name, unique id), for the Title
    /// Function popup; the app sets them when the window opens and when they change.
    pub title_providers: Vec<(String, String)>,
    /// Settings > General > AI: its sub-tab and whether it was just revealed.
    pub ai: crate::settings_ai::State,
}

/// Turning from page `from` to `to` applies a custom scripts folder typed and not yet applied
/// (iTerm2's `willDeselectTab`, `GP:3432-3434`), taking the note; staying on a page keeps it.
fn page_turn_applies(ctx: &egui::Context, from: Page, to: Page) -> bool {
    from != to && settings_keys::take_folder_pending(ctx)
}

impl SettingsWindow {
    /// Turns to one pane of the settings. True when a custom scripts folder typed on the
    /// page left is to be applied (`page_turn_applies`): the caller applies
    /// `Edit::GlobalsChanged`.
    #[must_use]
    pub fn show_page(&mut self, page: Page) -> bool {
        let apply = page_turn_applies(self.host.ctx(), self.page, page);
        self.page = page;
        apply
    }

    /// Shows Settings > General > AI on one of its sub-tabs (the gate's Reveal, Codecierge's
    /// Reveal Setting; iTerm2 opens Settings at `Enable AI`, `GK:61-63`, `CC:741-743`). It turns
    /// the page through `show_page`, so its answer is the same: true when a typed custom scripts
    /// folder is to be applied.
    #[must_use]
    pub fn show_ai(&mut self, sub: crate::settings_ai::SubTab) -> bool {
        let apply = self.show_page(Page::General);
        self.ai.sub = sub;
        self.ai.reveal = true;
        self.host.window.request_redraw();
        apply
    }

    /// Whether Settings > General > AI > General shows, where the model popup, the Ollama
    /// pickers and Manage AI Models read the discovered models.
    pub fn shows_ai_models(&self) -> bool {
        self.mode == Mode::Settings && self.page == Page::General && self.ai.sub == crate::settings_ai::SubTab::General
    }

    /// Whether a custom scripts folder was typed and not yet applied, clearing the note
    /// (`settings_keys::take_folder_pending`).
    pub fn take_folder_pending(&mut self) -> bool {
        settings_keys::take_folder_pending(self.host.ctx())
    }

    /// Shows one profile on one of its tabs (Profiles > Session for the status bar).
    pub fn show_profile_tab(&mut self, guid: &str, tab: &str) {
        self.page = Page::Profiles;
        self.selected = Some(guid.to_string());
        if let Some(i) = schema::TABS.iter().position(|t| *t == tab) {
            self.tab = i;
        }
        self.host.window.request_redraw();
        self.focus();
    }

    /// Shows Settings > Shortcuts on one of its tabs (the status bar menus' Edit Actions
    /// and Edit Snippets, iTerm2's `sessionEditActions` and `sessionEditSnippets`).
    pub fn show_shortcuts_tab(&mut self, tab: crate::settings_shortcuts::Tab) {
        self.page = Page::Shortcuts;
        self.shortcuts.show_tab(tab);
        self.host.window.request_redraw();
        self.focus();
    }

    /// Shows Settings > Shortcuts with a tab's editor open (the toolbelt's Add and Edit): a new
    /// snippet or action (`index` None), or the one at `index`. The sheet is filled from the
    /// snippets and the actions on the next frame, which has them.
    pub fn show_shortcuts_editor(&mut self, tab: crate::settings_shortcuts::Tab, index: Option<usize>) {
        self.page = Page::Shortcuts;
        self.shortcuts.show_tab(tab);
        self.open_editor = Some((tab, index));
        self.host.window.request_redraw();
        self.focus();
    }

    /// The Edit Action sheet is open.
    pub fn editing_action(&self) -> bool {
        self.open_editor.is_some_and(|(t, _)| t == crate::settings_shortcuts::Tab::Actions) || self.shortcuts.editing_action()
    }

    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, mode: Mode) -> Result<Self, String> {
        let (title, w, h) = match mode {
            Mode::Settings => ("Tandem Settings", 980.0, 720.0),
            Mode::Profiles => ("Profiles", 420.0, 460.0),
        };
        let host = crate::egui_host::EguiHost::open(event_loop, instance, adapter, device, queue, title, w, h)?;
        settings_keys::add_icon_font(host.ctx());
        if mode == Mode::Settings {
            if let Some(h) = host.hwnd() {
                crate::winkeys::set_settings_window(h);
            }
        }
        Ok(SettingsWindow {
            host,
            selected: None,
            tab: 0,
            mode,
            page: Page::Profiles,
            keys: KeysState::default(),
            mods: Mods::default(),
            aps_warning: None,
            arr: Default::default(),
            shortcuts: Default::default(),
            hotkeys: Default::default(),
            configure_status_bar: None,
            tmux_session: false,
            open_editor: None,
            title_providers: Vec::new(),
            ai: Default::default(),
        })
    }

    /// How the window was last opened: from a tmux session (Edit Session) or any other way.
    pub fn set_tmux_session(&mut self, tmux: bool) {
        self.tmux_session = tmux;
        self.host.window.request_redraw();
    }

    /// Shows this profile in the editor (Edit Session).
    pub fn select(&mut self, guid: &str) {
        self.selected = Some(guid.to_string());
        self.host.window.request_redraw();
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    /// A binding editor is waiting for a key press.
    pub fn recording(&self) -> bool {
        self.keys.recording()
    }

    /// A Win chord the keyboard hook took while recording.
    pub fn record_chord(&mut self, c: &crate::menus::Chord) {
        self.keys.deliver(crate::keyroute::press_from_chord(c).stroke);
        crate::winkeys::set_recording(self.keys.recording());
        self.host.window.request_redraw();
    }

    /// Feeds a window event to egui. Returns true when the window should redraw.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::ModifiersChanged(m) = event {
            let s = m.state();
            self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
        }
        if let WindowEvent::KeyboardInput { event: k, .. } = event {
            if self.keys.recording() {
                if k.state == winit::event::ElementState::Pressed {
                    if let Some(stroke) = crate::keystroke::from_key_event(k, self.mods) {
                        self.keys.deliver(stroke);
                        crate::winkeys::set_recording(self.keys.recording());
                    }
                }
                self.host.window.request_redraw();
                return true;
            }
        }
        self.host.event(event)
    }

    pub fn redraw(&mut self, store: &mut Store, globals: &mut Globals, snippets: &mut crate::snippets::Store) -> Edit {
        let mut edit = Edit::None;
        let selected = self.selected.get_or_insert_with(|| store.default_guid.clone()).clone();
        let mut tab = self.tab;
        let mut pick = None;
        let dark = self.host.window.theme().is_none_or(|t| t == winit::window::Theme::Dark);
        let mode = self.mode;
        let mut page = self.page;
        let keys = &mut self.keys;
        let aps_warning = &mut self.aps_warning;
        keys.leader = crate::keystroke::parse(&globals.string("Leader"));
        let profiles: Vec<(String, String)> = store.profiles.iter().map(|p| (p.guid(), p.name())).collect();
        let profile_tables: Vec<(String, String, toml::Table)> = store.profiles.iter().map(|p| (p.guid(), p.name(), p.values.clone().into_iter().collect())).collect();
        match self.open_editor.take() {
            Some((crate::settings_shortcuts::Tab::Snippets, index)) => self.shortcuts.edit_snippet(index, crate::settings_shortcuts::clipboard_text().as_deref()),
            Some((crate::settings_shortcuts::Tab::Actions, index)) => self.shortcuts.edit_action(index, &crate::user_actions::load(globals.get("Actions").as_ref())),
            None => {}
        }
        let arr = &mut self.arr;
        let shortcuts = &mut self.shortcuts;
        settings_keys::set_snippets(&snippets.snippets);
        let hk = &mut self.hotkeys;
        let configure_status_bar = &mut self.configure_status_bar;
        let tmux_session = self.tmux_session;
        let title_providers = &self.title_providers;
        let ai = &mut self.ai;
        let hotkey_prefs: Vec<(String, crate::hotkey::Prefs)> = store.profiles.iter().map(|p| (p.guid(), crate::hotkey::Prefs::from_profile(p))).collect();
        let rule_owners: Vec<crate::aps::Candidate> = store
            .profiles
            .iter()
            .map(|p| crate::aps::Candidate { guid: p.guid(), name: p.name(), rules: p.get("Bound Hosts").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default() })
            .collect();
        self.host.paint(|ui| {
            if mode == Mode::Settings {
                egui::Panel::top("pages").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (p, name) in [(Page::General, "General"), (Page::Appearance, "Appearance"), (Page::Profiles, "Profiles"), (Page::Keys, "Keys"), (Page::Shortcuts, "Shortcuts"), (Page::Pointer, "Pointer"), (Page::Arrangements, "Arrangements"), (Page::Advanced, "Advanced")] {
                            if ui.selectable_label(page == p, name).clicked() {
                                page = p;
                            }
                        }
                        for (name, m) in LATER_PAGES {
                            ui.add_enabled(false, egui::Button::new(name)).on_disabled_hover_text(format!("Comes in milestone {m}."));
                        }
                    });
                });
                if page != Page::Profiles {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let changed = match page {
                            Page::Keys => {
                                let mut c = settings_keys::keys_page(ui, globals, keys, &profiles);
                                match crate::settings_hotkey::keys_section(ui, globals, store, hk, keys) {
                                    crate::settings_hotkey::KeysEdit::None => {}
                                    crate::settings_hotkey::KeysEdit::Globals => c = true,
                                    crate::settings_hotkey::KeysEdit::Profile(g) => edit = Edit::Changed(g),
                                }
                                c
                            }
                            Page::General => settings_keys::schema_page_with(ui, globals, "General", &mut |ui, g| crate::settings_ai::section(ui, g, ai)),
                            Page::Advanced => settings_keys::schema_page(ui, globals, "Advanced"),
                            Page::Appearance => settings_keys::schema_page(ui, globals, "Appearance"),
                            Page::Arrangements => crate::settings_arrangements::page(ui, globals, &profile_tables, arr),
                            Page::Shortcuts => match crate::settings_shortcuts::page(ui, globals, snippets, shortcuts, &profiles) {
                                crate::settings_shortcuts::Changed::None => false,
                                crate::settings_shortcuts::Changed::Actions => true,
                                crate::settings_shortcuts::Changed::Snippets => {
                                    edit = Edit::SnippetsChanged;
                                    false
                                }
                            },
                            _ => settings_keys::pointer_page(ui, globals, keys, &profiles),
                        };
                        if changed {
                            edit = Edit::GlobalsChanged;
                        }
                    });
                    return;
                }
            }
            if mode == Mode::Profiles {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Profiles");
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for p in &store.profiles {
                            let r = ui.selectable_label(p.guid() == selected, p.name());
                            if r.clicked() {
                                pick = Some(p.guid());
                            }
                            if r.double_clicked() {
                                edit = Edit::Open(p.guid(), How::Tab);
                            }
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        for (label, how) in [("New Tab", How::Tab), ("New Window", How::Window), ("Split Right", How::SplitRight), ("Split Down", How::SplitDown)] {
                            if ui.button(label).clicked() {
                                edit = Edit::Open(selected.clone(), how);
                            }
                        }
                    });
                });
                return;
            }
            egui::Panel::left("profiles").default_size(220.0).show(ui, |ui| {
                ui.heading("Profiles");
                for p in &store.profiles {
                    let star = if p.guid() == store.default_guid { " (default)" } else { "" };
                    if ui.selectable_label(p.guid() == selected, format!("{}{star}", p.name())).clicked() {
                        pick = Some(p.guid());
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("+").on_hover_text("New profile").clicked() {
                        let p = Profile::new("New Profile");
                        pick = Some(p.guid());
                        store.add(p);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("Duplicate").clicked() {
                        pick = store.duplicate(&selected);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("-").on_hover_text("Delete profile").clicked() && store.delete(&selected) {
                        pick = Some(store.default_guid.clone());
                        edit = Edit::Changed(String::new());
                    }
                });
                if ui.button("Set as Default").clicked() {
                    store.default_guid = selected.clone();
                    edit = Edit::Changed(selected.clone());
                }
                if ui.button("Import from iTerm2...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("iTerm2 settings or Dynamic Profiles", &["plist", "json"]).pick_file() {
                        edit = match crate::import::from_file(&path) {
                            Ok(list) => {
                                let n = list.len();
                                for p in list {
                                    store.profiles.retain(|q| q.guid() != p.guid());
                                    store.add(p);
                                }
                                // A full settings plist also carries the global key and pointer settings.
                                let is_plist = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("plist"));
                                match is_plist.then(|| crate::import::globals_from_file(&path)) {
                                    Some(Err(e)) => Edit::Error(e),
                                    Some(Ok(values)) => {
                                        for (k, v) in values {
                                            globals.set(&k, v);
                                        }
                                        crate::ai_models::migrate_after_import(globals);
                                        Edit::Imported(n)
                                    }
                                    None => Edit::Imported(n),
                                }
                            }
                            Err(e) => Edit::Error(e),
                        };
                    }
                }
            });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, name) in schema::TABS.iter().enumerate() {
                        if ui.selectable_label(tab == i, *name).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.separator();
                let is_default = selected == store.default_guid;
                let Some(profile) = store.get_mut(&selected) else { return };
                if profile.bool("Use Separate Colors for Light and Dark Mode") && schema::TABS[tab] == "Colors" {
                    ui.label(format!("Editing the {} mode colors (Windows is in {} mode).", if dark { "Dark" } else { "Light" }, if dark { "dark" } else { "light" }));
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (section, list) in sections(schema::TABS[tab]) {
                        if !section.is_empty() && section != "Key Mappings" {
                            ui.add_space(8.0);
                            ui.strong(&section);
                        }
                        for s in list {
                            // The hotkey keys are one row and the Configure Hotkey Window sheet.
                            if s.key == "Has Hotkey" {
                                if crate::settings_hotkey::profile_row(ui, &selected, profile, hk, keys, &hotkey_prefs) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key.starts_with("HotKey ") {
                                continue;
                            }
                            if s.key == "Keyboard Map" {
                                if settings_keys::profile_key_mappings(ui, profile, keys, &profiles) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Bound Hosts" {
                                let others: Vec<crate::aps::Candidate> = rule_owners.iter().filter(|c| c.guid != selected).cloned().collect();
                                if crate::settings_lists::bound_hosts(ui, profile, &others, aps_warning) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            // iTerm2's "Configure Status Bar" button, enabled with the
                            // "Status bar enabled" checkbox above it; both disabled for a
                            // tmux session's own profile (PSV:569-573).
                            if s.key == "Show Status Bar" && !status_bar_controls(tmux_session, true).0 {
                                ui.add_enabled_ui(false, |ui| control(ui, profile, s, dark, is_default));
                                continue;
                            }
                            if s.key == "Status Bar Layout" {
                                let on = status_bar_controls(tmux_session, profile.bool("Show Status Bar")).1;
                                if ui.add_enabled(on, egui::Button::new("Configure Status Bar")).on_hover_text(&s.help).clicked() {
                                    *configure_status_bar = Some(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Title Function" {
                                if title_function_row(ui, profile, s, title_providers) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Jobs to Ignore" {
                                if crate::settings_lists::jobs_to_ignore(ui, profile) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Triggers" {
                                if crate::settings_triggers::editor(ui, profile) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Smart Selection Rules" || s.key == "Semantic History" {
                                let done = if s.key == "Smart Selection Rules" {
                                    crate::settings_advanced::smart_selection_rules(ui, profile)
                                } else {
                                    crate::settings_advanced::semantic_history(ui, profile)
                                };
                                if done {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if control(ui, profile, s, dark, is_default) {
                                edit = Edit::Changed(selected.clone());
                            }
                        }
                    }
                });
            });
        });
        if let Some(e) = self.ai.take_error() {
            edit = Edit::Error(e);
        }
        self.tab = tab;
        // Leaving a page applies a custom scripts folder typed there (iTerm2's
        // `willDeselectTab`, `GP:3432-3434`); the field is no longer drawn to lose the focus.
        if matches!(edit, Edit::None) && page_turn_applies(self.host.ctx(), self.page, page) {
            edit = Edit::GlobalsChanged;
        }
        self.page = page;
        crate::winkeys::set_recording(self.keys.recording());
        if let Some(g) = pick {
            self.selected = Some(g);
        }
        // A probe or a model fetch keeps drawing: its spinner turns and its answer shows at once.
        if self.ai.busy() {
            self.host.request_redraw();
        }
        edit
    }

    /// Draw again: something the window shows changed outside it (discovered Ollama models, a
    /// discovery answer).
    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }
}

/// The Title Function popup's first item: the Title Components choose the title.
pub const BUILT_IN_COMPONENTS: &str = "Built-in components";

/// The Title Function popup (`PGP:753-775`, `PGP:1507-1526`): the text it shows (with Custom
/// on, the saved function's display name) and its items as (label, unique id): "Built-in
/// components" first, then each registered provider, then the saved function when no
/// registered provider has its unique id (iTerm2 keeps it, disabled).
pub fn title_choice(components: i64, function: Option<(String, String)>, providers: &[(String, String)]) -> (String, Vec<(String, String)>) {
    let mut items = vec![(BUILT_IN_COMPONENTS.to_string(), String::new())];
    items.extend(providers.iter().cloned());
    let text = match &function {
        Some((name, _)) if components & i64::from(crate::jobtitle::CUSTOM) != 0 => name.clone(),
        _ => BUILT_IN_COMPONENTS.to_string(),
    };
    if let Some(f) = function.filter(|(_, id)| !providers.iter().any(|(_, p)| p == id)) {
        items.push(f);
    }
    (text, items)
}

/// The Title Function row: a popup of `title_choice`'s items in place of the list's "n
/// item(s)". A provider writes Title Function `[{value = display name}, {value = unique
/// id}]` (`jobtitle::title_function_value`) and Title Components Custom (`PGP:1427-1437`);
/// "Built-in components" writes Title Components Job and keeps the Title Function. A saved function with no registered provider shows
/// disabled. True when the profile changed.
fn title_function_row(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting, providers: &[(String, String)]) -> bool {
    use crate::jobtitle::{CUSTOM, JOB};
    let components = profile.int("Title Components");
    let function = crate::jobtitle::title_function(profile.get("Title Function").as_ref());
    let custom = components & i64::from(CUSTOM) != 0;
    let current = function.as_ref().map(|(_, id)| id.clone());
    let (text, items) = title_choice(components, function, providers);
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        egui::ComboBox::from_id_salt(&s.key).selected_text(text).show_ui(ui, |ui| {
            for (i, (label, id)) in items.iter().enumerate() {
                let (on, enabled) = if i == 0 { (!custom, true) } else { (custom && current.as_deref() == Some(id.as_str()), providers.iter().any(|(_, p)| p == id)) };
                if ui.add_enabled(enabled, egui::Button::selectable(on, label.as_str())).clicked() {
                    picked = Some(i);
                }
                if i == 0 && items.len() > 1 {
                    ui.separator();
                }
            }
        });
    })
    .response
    .on_hover_text(&s.help);
    match picked {
        None => false,
        Some(0) => {
            profile.set("Title Components", Value::Integer(i64::from(JOB)));
            true
        }
        Some(i) => {
            let (name, id) = items[i].clone();
            profile.set("Title Function", crate::jobtitle::title_function_value(&name, &id));
            profile.set("Title Components", Value::Integer(i64::from(CUSTOM)));
            true
        }
    }
}

/// updateStatusBarSettingsEnabled (`PSV:569-573`): whether "Status bar enabled" and
/// "Configure Status Bar" are enabled. Neither is when Settings was opened from a tmux
/// session, whatever profile it shows; Configure also needs "Status bar enabled" on.
pub fn status_bar_controls(opened_from_tmux: bool, show: bool) -> (bool, bool) {
    (!opened_from_tmux, !opened_from_tmux && show)
}

/// The key a control edits. With separate Light and Dark colors on, a color with
/// variants edits the variant for the current mode, as iTerm2's editor does.
pub fn edit_key(profile: &Profile, s: &Setting, dark: bool) -> String {
    if s.light_dark && profile.bool("Use Separate Colors for Light and Dark Mode") {
        format!("{} ({})", s.key, if dark { "Dark" } else { "Light" })
    } else {
        s.key.clone()
    }
}

/// Font settings follow the default profile until each pane can have its own renderer.
pub const FONT_KEYS: [&str; 5] = ["Normal Font", "Non Ascii Font", "Use Non-ASCII Font", "Horizontal Spacing", "Vertical Spacing"];

/// One control for one setting. Returns true when a change is committed: at once for
/// checkboxes, pickers, and colors; when a drag ends for sliders; when the field loses
/// focus (or Enter) for text. Uncommitted edits still update the profile in memory.
fn control(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting, dark: bool, is_default: bool) -> bool {
    let font_locked = !is_default && FONT_KEYS.contains(&s.key.as_str());
    let key = edit_key(profile, s, dark);
    let current = if key == s.key {
        profile.get(&s.key)
    } else {
        let stock = if dark { s.default_dark.clone() } else { s.default_light.clone() };
        profile.values.get(&key).cloned().or(stock).or_else(|| profile.get(&s.key))
    };
    let mut changed = None;
    let mut commit = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!s.macos_only && !font_locked, |ui| {
            match s.kind {
                Kind::Bool => {
                    let mut b = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    if ui.checkbox(&mut b, &s.label).changed() {
                        changed = Some(Value::Boolean(b));
                        commit = true;
                    }
                }
                Kind::Int => {
                    ui.label(&s.label);
                    let mut i = current.as_ref().and_then(|v| v.as_integer()).unwrap_or(0);
                    let mut w = egui::DragValue::new(&mut i);
                    if let (Some(a), Some(b)) = (s.min_f64(), s.max_f64()) {
                        w = w.range(a as i64..=b as i64);
                    }
                    let r = ui.add(w);
                    if r.changed() {
                        changed = Some(Value::Integer(i));
                    }
                    commit = r.drag_stopped() || (r.changed() && !r.dragged());
                }
                Kind::Float => {
                    ui.label(&s.label);
                    let mut f = current.as_ref().and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
                    let r = match (s.min_f64(), s.max_f64()) {
                        (Some(a), Some(b)) => ui.add(egui::Slider::new(&mut f, a..=b)),
                        _ => ui.add(egui::DragValue::new(&mut f).speed(0.01)),
                    };
                    if r.changed() {
                        changed = Some(Value::Float(f));
                    }
                    commit = r.drag_stopped() || (r.changed() && !r.dragged());
                }
                Kind::String | Kind::Font => {
                    ui.label(&s.label);
                    let mut text = current.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let r = ui.text_edit_singleline(&mut text);
                    if r.changed() {
                        changed = Some(Value::String(text));
                    }
                    commit = r.lost_focus();
                }
                Kind::Color => {
                    ui.label(&s.label);
                    let mut c = current.as_ref().map(rgb_of).unwrap_or([0, 0, 0]);
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        changed = Some(value_of_rgb(c));
                        commit = true;
                    }
                }
                Kind::Enum => {
                    ui.label(&s.label);
                    let now = current.clone();
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or("".to_string(), |c| c.label.clone());
                    egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
                        for c in &s.choices {
                            if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                changed = Some(c.value.clone());
                                commit = true;
                            }
                        }
                    });
                }
                Kind::List | Kind::Dict | Kind::Data => {
                    let n = match &current {
                        Some(Value::Array(a)) => a.len(),
                        Some(Value::Table(t)) => t.len(),
                        _ => 0,
                    };
                    ui.label(format!("{}: {n} item(s)", s.label)).on_hover_text("This editor arrives with its feature's milestone.");
                }
            }
        })
        .response
        .on_hover_text(if s.macos_only {
            format!("{} (macOS only, no Windows equivalent)", s.help)
        } else if font_locked {
            format!("{} Fonts follow the default profile for now; per-profile fonts come later.", s.help)
        } else {
            s.help.clone()
        });
    });
    if let Some(v) = changed {
        profile.set(&key, v);
    }
    commit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_the_page_applies_the_typed_folder() {
        let ctx = egui::Context::default();
        settings_keys::note_folder_pending(&ctx);
        // Staying on the page keeps the note for the page change or close that follows.
        assert!(!page_turn_applies(&ctx, Page::General, Page::General));
        assert!(page_turn_applies(&ctx, Page::General, Page::Keys));
        // Taken: once only.
        assert!(!page_turn_applies(&ctx, Page::Keys, Page::General));
        assert!(!settings_keys::take_folder_pending(&ctx));
    }

    #[test]
    fn every_tab_has_sections_in_schema_order() {
        for tab in crate::schema::TABS {
            let s = sections(tab);
            assert!(!s.is_empty(), "{tab}");
            let keys: Vec<&str> = s.iter().flat_map(|(_, v)| v.iter().map(|x| x.key.as_str())).collect();
            let schema: Vec<&str> = crate::schema::profile().iter().filter(|x| x.tab == tab).map(|x| x.key.as_str()).collect();
            let mut sorted = schema.clone();
            sorted.retain(|k| keys.contains(k));
            assert_eq!(keys.len(), schema.len(), "{tab} lost settings");
        }
    }

    #[test]
    fn color_edits_target_the_current_mode_variant() {
        let fg = crate::schema::find("Foreground Color").unwrap();
        let mut p = Profile::stock();
        assert_eq!(edit_key(&p, fg, true), "Foreground Color (Dark)");
        assert_eq!(edit_key(&p, fg, false), "Foreground Color (Light)");
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(false));
        assert_eq!(edit_key(&p, fg, true), "Foreground Color");
        let cols = crate::schema::find("Columns").unwrap();
        assert_eq!(edit_key(&Profile::default(), cols, true), "Columns");
    }

    #[test]
    fn colors_convert_both_ways() {
        let v = value_of_rgb([255, 128, 0]);
        assert_eq!(rgb_of(&v), [255, 128, 0]);
    }

    /// `PGP:753-775`, `PGP:1507-1526`: the Title Function popup lists the built-in
    /// components and then the providers; a saved function whose provider is not registered
    /// stays an item; Custom shows the function's display name.
    #[test]
    fn title_choice_follows_iterm2() {
        let providers = vec![("Hello".to_string(), "com.ex.t".to_string())];
        let item = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(title_choice(2, None, &providers), ("Built-in components".to_string(), vec![item("Built-in components", ""), item("Hello", "com.ex.t")]));
        assert_eq!(title_choice(16, Some(item("Hello", "com.ex.t")), &providers).0, "Hello");
        assert_eq!(title_choice(16, Some(item("Hello", "com.ex.t")), &providers).1.len(), 2, "the registered one is not listed twice");
        let (text, items) = title_choice(16, Some(item("Old", "com.old")), &providers);
        assert_eq!(text, "Old");
        assert_eq!(items, vec![item("Built-in components", ""), item("Hello", "com.ex.t"), item("Old", "com.old")]);
        assert_eq!(title_choice(2, Some(item("Old", "com.old")), &[]).1, vec![item("Built-in components", ""), item("Old", "com.old")], "kept while not Custom too");
        assert_eq!(title_choice(16, None, &providers).0, "Built-in components");
    }

    /// `PSV:569-573`: opened from a tmux session, both status bar controls are disabled.
    #[test]
    fn status_bar_settings_follow_psv() {
        assert_eq!(status_bar_controls(false, true), (true, true));
        assert_eq!(status_bar_controls(false, false), (true, false), "Configure needs the bar on");
        assert_eq!(status_bar_controls(true, true), (false, false));
        assert_eq!(status_bar_controls(true, false), (false, false));
    }
}
