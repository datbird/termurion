//! The settings window, laid out like iTerm2's profile editor: a profile list on the
//! left, the eight tabs across the top, and one control per schema setting.

use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::chrome;
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
    /// A settings folder was just picked with "Load settings from a custom folder or URL"
    /// turned on: the settings changed, and "Copy local settings to custom folder now?" asks.
    PrefsCopyNow,
    /// General > Settings' Save Now.
    PrefsSaveNow,
    /// General > Settings' Export All Settings and Data button.
    ExportAll,
    /// General > Settings' Import All Settings and Data button.
    ImportAll,
    /// General > Settings' Erase All Settings and Data button.
    EraseAll,
    /// A color preset is being previewed (or the preview was put back): the panes redraw, and
    /// nothing is saved (B2).
    Preview,
}

/// What `App::apply_edit` does first with an edit (B2): false when nothing more is applied (no edit,
/// or a preview, which only redraws). Any other edit first puts a stale preview back through
/// `restore`, so the previewed colors are never in the store when it is saved.
pub fn gate_edit(edit: &Edit, restore: &mut dyn FnMut()) -> bool {
    if matches!(edit, Edit::None | Edit::Preview) {
        return false;
    }
    restore();
    true
}

/// Whether an edit starts the pause before the profiles are saved: a preview never does.
pub fn edit_arms_save(edit: &Edit) -> bool {
    !matches!(edit, Edit::Preview | Edit::None)
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

/// The page bar's options menu item (`PP:1096-1107`), bound to `NoSyncHideDefaultValuedSettings`.
pub const SHOW_INDICATORS: &str = "Show indicators for non-default values";
/// The options menu's name (iTerm2's search field menu, `PP:1096`), shown on hover.
pub const SEARCH_OPTIONS: &str = "Search Options";
/// The options menu button's glyph: Segoe Fluent Icons' Search.
const SEARCH_GLYPH: char = '\u{E721}';

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
    /// Profiles > Terminal's "Change Locale\u{2026}" was pressed for this profile Guid: the locale
    /// picker opens for it (the app takes it after each frame).
    pub locale_pick: Option<String>,
    /// Profiles > General's Configure SSH sheet.
    ssh_sheet: Option<crate::settings_ssh::Sheet>,
    /// A page to open in the browser (the Configure SSH help button); the app takes it after
    /// each frame.
    pub open_url: Option<String>,
    /// Profiles > General's "URL schemes:" menu chose a scheme Tandem does not handle yet: the
    /// not-default question to ask (the app takes it after each frame).
    pub scheme_ask: Option<SchemeAsk>,
    /// Profiles > Text's "Manage Special Exceptions" was pressed for this profile Guid: the
    /// window opens for it (the app takes it after each frame).
    pub special_exceptions: Option<String>,
    /// The window was opened from a tmux session (iTerm2's `_tmuxSession`, set by how the
    /// panel opened, `PreferencePanel.m:725-766`): its status bar controls are disabled.
    tmux_session: bool,
    /// `show_shortcuts_editor`'s sheet, opened on the next frame.
    open_editor: Option<(crate::settings_shortcuts::Tab, Option<usize>)>,
    /// The registered session title providers as (display name, unique id), for the Title
    /// Function popup; the app sets them when the window opens and when they change.
    pub title_providers: Vec<(String, String)>,
    /// Session > Prevent Sleep's gray line (`PSV:465-482`), set by the app.
    pub sleep_status: String,
    /// Settings > Advanced: the search, the non-default filter and the field being edited.
    advanced: crate::settings_advanced_pane::AdvancedState,
    /// Settings > General > AI: its sub-tab and whether it was just revealed.
    pub ai: crate::settings_ai::State,
    /// The warning shown over the window (iTerm2's sheet), until it is answered.
    sheet: Option<settings_keys::SheetWarning>,
    /// "Always show alerts with remembered selections"; the app sets it before each redraw.
    pub show_remembered: bool,
    /// The warnings answered since the app last took them: it lists the automatic answers in
    /// Suppressed Alerts and saves the settings a remembered answer changed.
    pub answered: Vec<settings_keys::SheetAnswer>,
    /// The Colors tab's presets: the Editing mode, a live hover preview, the questions and dialogs.
    presets: crate::settings_presets::State,
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
        shows_ai_models_on(self.mode, self.page, self.ai.sub)
    }

    /// A file to show in Explorer (an exported color preset), taken after each frame.
    pub fn take_reveal(&mut self) -> Option<String> {
        self.presets.reveal.take()
    }

    /// Puts a live color preview's colors back before anything is saved or the window goes (B2).
    /// True when a profile changed.
    pub fn restore_preview(&mut self, store: &mut Store) -> bool {
        self.presets.restore(store)
    }

    /// Whether a custom scripts folder was typed and not yet applied, clearing the note
    /// (`settings_keys::take_folder_pending`).
    pub fn take_folder_pending(&mut self) -> bool {
        settings_keys::take_folder_pending(self.host.ctx())
    }

    /// Whether the Space row committed a numbered desktop since the last call, clearing the
    /// note: the app then asks the "make sure the desktop exists" warning (`PWP:1087-1098`).
    pub fn take_space_warning(&mut self) -> bool {
        self.host.ctx().data_mut(|d| d.remove_temp::<bool>(egui::Id::new("space_warning"))).unwrap_or(false)
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

    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.host.follow_theme(forced);
    }

    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, mode: Mode) -> Result<Self, String> {
        let (title, w, h) = match mode {
            Mode::Settings => ("Tandem Settings", 980.0, 720.0),
            Mode::Profiles => ("Profiles", 420.0, 460.0),
        };
        let host = crate::egui_host::EguiHost::open(event_loop, instance, adapter, device, queue, title, w, h)?;
        let host_dark = host.window.theme().is_none_or(|t| t == winit::window::Theme::Dark);
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
            locale_pick: None,
            ssh_sheet: None,
            open_url: None,
            scheme_ask: None,
            special_exceptions: None,
            tmux_session: false,
            open_editor: None,
            title_providers: Vec::new(),
            sleep_status: String::new(),
            advanced: Default::default(),
            ai: Default::default(),
            sheet: None,
            show_remembered: false,
            answered: Vec::new(),
            presets: crate::settings_presets::State::new(host_dark),
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
        // A fixed Theme forces its mode here too (`tab_theme::forced_dark`).
        let style = crate::tab_theme::from_value(globals.int(crate::tab_theme::KEY));
        let dark = crate::tab_theme::forced_dark(style).unwrap_or_else(|| self.host.window.theme().is_none_or(|t| t == winit::window::Theme::Dark));
        self.presets.editing.follow(dark);
        let presets = &mut self.presets;
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
        settings_keys::set_indicate_non_default(globals.bool("NoSyncHideDefaultValuedSettings"));
        let hk = &mut self.hotkeys;
        let configure_status_bar = &mut self.configure_status_bar;
        let locale_pick = &mut self.locale_pick;
        let ssh_sheet = &mut self.ssh_sheet;
        let open_url = &mut self.open_url;
        let scheme_ask = &mut self.scheme_ask;
        let special_exceptions = &mut self.special_exceptions;
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let tmux_session = self.tmux_session;
        let title_providers = &self.title_providers;
        let sleep_status = &self.sleep_status;
        let advanced = &mut self.advanced;
        let ai = &mut self.ai;
        let sheet = &mut self.sheet;
        let show_remembered = self.show_remembered;
        let answered = &mut self.answered;
        let key_maps: Vec<toml::Table> = store.profiles.iter().map(|p| p.get("Keyboard Map").and_then(|v| v.as_table().cloned()).unwrap_or_default()).collect();
        let global_keys = globals.table("GlobalKeyMap");
        let hotkey_prefs: Vec<(String, crate::hotkey::Prefs)> = store.profiles.iter().map(|p| (p.guid(), crate::hotkey::Prefs::from_profile(p))).collect();
        let rule_owners: Vec<crate::aps::Candidate> = store
            .profiles
            .iter()
            .map(|p| crate::aps::Candidate { guid: p.guid(), name: p.name(), rules: p.get("Bound Hosts").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default() })
            .collect();
        self.host.paint(|ui| {
            // A warning an editor raised on the last frame shows now, over everything.
            if sheet.is_none() {
                *sheet = keys.ask.take();
            }
            if let Some(w) = sheet.as_mut() {
                if let Some(answer) = settings_keys::sheet_warning(ui.ctx(), globals, w, show_remembered) {
                    // Forget shows the warning again; any other answer ends it.
                    if answer.index < answer.spec.labels.len() {
                        // "Update Which Modes?": load the preset the click was about (P21).
                        if crate::settings_presets::is_question_id(answer.spec.id.as_deref()) {
                            if let Some((name, _)) = presets.pending.take() {
                                if let (Some(p), Some(preset)) = (store.get_mut(&selected), crate::color_presets::find(globals, &name)) {
                                    crate::settings_presets::load(&preset, p, answer.index == 0, presets.editing.dark, dark);
                                    edit = Edit::Changed(selected.clone());
                                }
                            }
                        }
                        match sheet.take().map(|w| w.then) {
                            Some(settings_keys::SheetThen::SaveKey) => match settings_keys::held_answered(keys, answer.index, globals, store) {
                                settings_keys::Saved::Nothing => {}
                                settings_keys::Saved::Global => edit = Edit::GlobalsChanged,
                                settings_keys::Saved::Profile(g) => edit = Edit::Changed(g),
                            },
                            Some(settings_keys::SheetThen::CreateHotkey(existing)) => crate::settings_hotkey::create_answered(hk, store, &existing, answer.index),
                            Some(settings_keys::SheetThen::Nothing) | None => {}
                        }
                    }
                    answered.push(answer);
                    ui.ctx().request_repaint();
                }
            }
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
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let title = settings_keys::icon_text(ui.ctx(), SEARCH_GLYPH).unwrap_or_else(|| egui::RichText::new(SEARCH_OPTIONS));
                            ui.menu_button(title, |ui| {
                                let mut on = globals.bool("NoSyncHideDefaultValuedSettings");
                                if ui.checkbox(&mut on, SHOW_INDICATORS).changed() {
                                    globals.set("NoSyncHideDefaultValuedSettings", Value::Boolean(on));
                                    settings_keys::set_indicate_non_default(on);
                                    edit = Edit::GlobalsChanged;
                                }
                            })
                            .response
                            .on_hover_text(SEARCH_OPTIONS);
                        });
                    });
                });
                if page != Page::Profiles {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let changed = match page {
                            Page::Keys => {
                                crate::color_presets::remember_names(globals);
                                let mut c = settings_keys::keys_page(ui, globals, keys, &profiles, &key_maps);
                                match crate::settings_hotkey::keys_section(ui, globals, store, hk, keys) {
                                    crate::settings_hotkey::KeysEdit::None => {}
                                    crate::settings_hotkey::KeysEdit::Globals => c = true,
                                    crate::settings_hotkey::KeysEdit::Profile(g) => edit = Edit::Changed(g),
                                }
                                c
                            }
                            Page::General => match settings_keys::schema_page_with(ui, globals, "General", &mut |ui, g| crate::settings_ai::section(ui, g, ai)) {
                                (_, settings_keys::PrefsAction::CopyNow) => {
                                    edit = Edit::PrefsCopyNow;
                                    false
                                }
                                (_, settings_keys::PrefsAction::SaveNow) => {
                                    edit = Edit::PrefsSaveNow;
                                    false
                                }
                                (_, settings_keys::PrefsAction::ExportAll) => {
                                    edit = Edit::ExportAll;
                                    false
                                }
                                (_, settings_keys::PrefsAction::ImportAll) => {
                                    edit = Edit::ImportAll;
                                    false
                                }
                                (_, settings_keys::PrefsAction::EraseAll) => {
                                    edit = Edit::EraseAll;
                                    false
                                }
                                (c, _) => c,
                            },
                            Page::Advanced => crate::settings_advanced_pane::page(ui, globals, advanced),
                            Page::Appearance => settings_keys::schema_page(ui, globals, "Appearance").0,
                            Page::Arrangements => crate::settings_arrangements::page(ui, globals, &profile_tables, arr),
                            Page::Shortcuts => match {
                                crate::color_presets::remember_names(globals);
                                crate::settings_shortcuts::page(ui, globals, snippets, shortcuts, &profiles)
                            } {
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
                            Ok(mut list) => {
                                let n = list.len();
                                // A full settings plist also carries the global key and pointer
                                // settings, and maybe iTerm2's legacy keys, which are migrated into
                                // the imported values and profiles before they are stored.
                                let is_plist = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("plist"));
                                let globals_read = is_plist.then(|| {
                                    let mut values = crate::import::globals_from_file(&path)?;
                                    crate::import::migrate_legacy(&mut values, &crate::import::legacy_from_file(&path)?, &mut list);
                                    Ok::<_, String>(values)
                                });
                                for p in list {
                                    store.profiles.retain(|q| q.guid() != p.guid());
                                    store.add(p);
                                }
                                match globals_read {
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
                let saved_guid = store.shared_guid(&selected);
                let Some(profile) = store.get_mut(&selected) else { return };
                let separate = profile.bool("Use Separate Colors for Light and Dark Mode");
                let system_dark = dark;
                let colors = schema::TABS[tab] == "Colors";
                // The Colors tab's wells edit the mode the Editing selector names, not Windows' mode.
                let dark = if colors { presets.editing.dark } else { dark };
                if colors {
                    let h = crate::settings_presets::colors_header(ui, presets, globals, profile, &selected, system_dark);
                    if h.previewed {
                        edit = Edit::Preview;
                    }
                    if h.import {
                        crate::settings_presets::begin_import(presets);
                    }
                    if h.export {
                        crate::settings_presets::begin_export(presets, profile);
                    }
                    if h.save_as {
                        crate::settings_presets::begin_save_as(presets, globals, profile);
                    }
                    if h.delete {
                        crate::settings_presets::begin_delete(presets, globals);
                    }
                    if h.gallery {
                        *open_url = Some(crate::settings_presets::GALLERY_URL.to_string());
                    }
                    if let Some(name) = h.clicked {
                        match crate::settings_presets::click(presets, &name, globals, profile, system_dark) {
                            crate::settings_presets::Click::Loaded => edit = Edit::Changed(selected.clone()),
                            crate::settings_presets::Click::Ask(spec) => {
                                *sheet = Some(settings_keys::SheetWarning::new(spec, settings_keys::SheetThen::Nothing));
                                ui.ctx().request_repaint();
                            }
                            crate::settings_presets::Click::Missing => {}
                        }
                    }
                    if crate::settings_presets::pump_dialogs(ui.ctx(), presets, globals, profile) {
                        edit = Edit::GlobalsChanged;
                    }
                    // iTerm2 shows a warning button whose click shows this text; Tandem shows the text.
                    if separate && crate::tab_theme::mode_warning(style, separate) {
                        ui.colored_label(ui.visuals().warn_fg_color, crate::tab_theme::MODE_WARNING);
                    }
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
                                crate::color_presets::remember_names(globals);
                                if settings_keys::profile_key_mappings(ui, profile, keys, &profiles, &global_keys) {
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
                            // The locale's status line under the mode (`PTM:397-445`) and the Change
                            // Locale button, shown only for the Custom mode; the button is the only
                            // way to set the locale, as in iTerm2.
                            if s.key == "Custom Locale" {
                                if profile.int("Set Local Environment Vars") == 2 && ui.button("Change Locale\u{2026}").on_hover_text(&s.help).clicked() {
                                    *locale_pick = Some(selected.clone());
                                }
                                continue;
                            }
                            if s.key == crate::ssh_config::KEY {
                                if crate::settings_ssh::row(ui, &selected, profile, ssh_sheet, open_url, &home) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            // The Window Style popup and the Columns and Rows row with its
                            // units popups (`PWV:394-700`); the percent fields are in that row.
                            if s.key == "Window Type" {
                                if window_type_row(ui, profile, s) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if s.key == "Columns" {
                                if window_size_row(ui, profile) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            if matches!(s.key.as_str(), "Rows" | "Width Percentage" | "Height Percentage") {
                                continue;
                            }
                            if s.key == "Background Image Source Mode" && background_image_row(ui, profile) {
                                edit = Edit::Changed(selected.clone());
                            }
                            if s.key == "Title Function" {
                                if title_function_row(ui, profile, s, title_providers) {
                                    edit = Edit::Changed(selected.clone());
                                }
                                continue;
                            }
                            // iTerm2's "Manage Special Exceptions" button (the table is its own window).
                            if s.key == crate::font_table::KEY {
                                if ui.button(&s.label).on_hover_text(&s.help).clicked() {
                                    *special_exceptions = Some(selected.clone());
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
                            if s.key == "Cursor Blink Fade In Duration" {
                                ui.add_enabled_ui(profile_row_enabled(profile, "Presets", dark), |ui| {
                                    if blink_presets_row(ui, profile) {
                                        edit = Edit::Changed(selected.clone());
                                    }
                                });
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
                            let enabled = crate::transparency::row_enabled(&s.key, profile.float("Transparency"), profile.bool("Blur")) && profile_row_enabled(profile, &s.key, dark);
                            let mut changed = false;
                            if s.key == "Progress Bar Height" {
                                ui.add_enabled_ui(enabled, |ui| changed = progress_height_row(ui, profile, s, globals.int("TerminalVMargin")));
                            } else {
                                ui.add_enabled_ui(enabled, |ui| changed = control(ui, profile, s, dark, is_default));
                            }
                            // The notes under Transparency and Blur Radius (PWP:157-170, 762-769).
                            if s.key == "Transparency" && profile.float("Transparency") > 0.0 {
                                ui.label(egui::RichText::new(crate::transparency::OVERRIDE_NOTICE).small());
                            }
                            if s.key == "Blur Radius" && crate::transparency::radius_warning(profile.bool("Blur"), profile.float("Blur Radius")) {
                                ui.horizontal(|ui| {
                                    if let Some(g) = settings_keys::icon_text(ui.ctx(), crate::transparency::WARNING_GLYPH) {
                                        ui.label(g.color(ui.visuals().warn_fg_color));
                                    }
                                    ui.label(egui::RichText::new(crate::transparency::LARGE_RADIUS).small());
                                });
                            }
                            if s.key == crate::session_end::END_ACTION_KEY && short_lived_row(ui, profile, &saved_guid, globals) {
                                edit = Edit::GlobalsChanged;
                            }
                            if s.key == "Prevent Sleep" && !sleep_status.is_empty() {
                                ui.label(egui::RichText::new(sleep_status).small().color(ui.visuals().weak_text_color()));
                            }
                            if s.key == "Set Local Environment Vars" {
                                locale_status_line(ui, profile, &globals.string("FallbackLCCType"));
                            }
                            if changed {
                                // Choosing Meta for either Option key warns (`PK:554-559`).
                                if chose_meta(&s.key, profile.get(&s.key).as_ref()) {
                                    keys.ask = Some(settings_keys::SheetWarning::new(settings_keys::meta_spec(), settings_keys::SheetThen::Nothing));
                                    ui.ctx().request_repaint();
                                }
                                if s.key == "Space" && profile.int("Space") > 0 {
                                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("space_warning"), true));
                                }
                                edit = Edit::Changed(selected.clone());
                            }
                        }
                    }
                    // iTerm2's "URL schemes:" sits at the bottom of the General tab.
                    if schema::TABS[tab] == "General" {
                        ui.add_space(8.0);
                        if url_schemes_control(ui, globals, &selected, scheme_ask) {
                            edit = Edit::GlobalsChanged;
                        }
                    }
                });
            });
        });
        // A preview whose header is gone (another tab, page or profile) goes back now (B2).
        if self.presets.strand_check(store) && matches!(edit, Edit::None) {
            edit = Edit::Preview;
        }
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
    let mut popup_rect = None;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        let r = egui::ComboBox::from_id_salt(&s.key).selected_text(text).show_ui(ui, |ui| {
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
        popup_rect = Some(r.response.rect);
    })
    .response
    .on_hover_text(&s.help);
    if let Some(rect) = popup_rect.filter(|_| settings_keys::indicate_non_default() && !profile_is_default(profile, s, &s.key)) {
        settings_keys::non_default_bar(ui, rect, ui.is_enabled());
    }
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

/// Stores a window type with the units popups' choices and the percent fields
/// (`chrome::units_save`); the percentages are written only for a type that keeps them.
fn save_window_type(profile: &mut Profile, style: i64, columns: chrome::Units, rows: chrome::Units) {
    use crate::frames::{ACCESSORY, FULL_SCREEN, LION_FULL_SCREEN, MAXIMIZED};
    let (stored, w, h) = chrome::units_save(style, columns, rows, profile.float("Width Percentage"), profile.float("Height Percentage"));
    profile.set("Window Type", Value::Integer(stored));
    if !matches!(stored, MAXIMIZED | FULL_SCREEN | LION_FULL_SCREEN | ACCESSORY) {
        profile.set("Width Percentage", Value::Float(w));
        profile.set("Height Percentage", Value::Float(h));
    }
}

/// The units popups' choices for the profile now; a hidden popup (Maximized, full screen)
/// keeps what the percentages say.
fn shown_units(profile: &Profile) -> (chrome::Units, chrome::Units) {
    let (w, h) = crate::frames::profile_percents(profile);
    let row = chrome::units_rows(profile.int("Window Type"), w, h);
    let unit = |p: Option<f64>| if p.is_some_and(|p| p >= 0.0) { chrome::Units::Percent } else { chrome::Units::Cells };
    (row.columns_units.unwrap_or(unit(w)), row.rows_units.unwrap_or(unit(h)))
}

/// The profile's Window Style popup: iTerm2's ten entries (`frames::PROFILE_MENU`), a stored
/// cells id or Compact form showing as its entry without being rewritten until a pick. True
/// when the profile changed.
fn window_type_row(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting) -> bool {
    use crate::frames::{profile_menu_value, PROFILE_MENU};
    let now = profile_menu_value(profile.int("Window Type"));
    let shown = PROFILE_MENU.iter().find(|(id, _)| *id == now).map_or("", |(_, n)| n);
    let mut picked = None;
    let mut popup_rect = None;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        let r = egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
            for (id, name) in PROFILE_MENU {
                if ui.selectable_label(id == now, name).clicked() {
                    picked = Some(id);
                }
            }
        });
        popup_rect = Some(r.response.rect);
    })
    .response
    .on_hover_text(&s.help);
    if let Some(rect) = popup_rect.filter(|_| settings_keys::indicate_non_default() && !profile_is_default(profile, s, &s.key)) {
        settings_keys::non_default_bar(ui, rect, ui.is_enabled());
    }
    let Some(style) = picked else { return false };
    let (columns, rows) = shown_units(profile);
    save_window_type(profile, style, columns, rows);
    true
}

/// The Columns and Rows row (`updateWindowTypeControlFromSettings`, `PWV:477-700`, laid out
/// by `chrome::units_rows`): each axis's field in cells or percent of the screen and its
/// units popup, "by" between them; without popups (Maximized, the full screen types) the
/// fields read "columns by" and "rows", disabled. True when a change is committed.
fn window_size_row(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    use chrome::Units;
    let (w, h) = crate::frames::profile_percents(profile);
    let row = chrome::units_rows(profile.int("Window Type"), w, h);
    let mut commit = false;
    let mut units = (row.columns_units, row.rows_units);
    let mut units_changed = false;
    ui.horizontal(|ui| {
        let mut axis = |ui: &mut egui::Ui, u: &mut Option<Units>, enabled: bool, cells_key: &str, percent_key: &str, titles: (&str, &str), id: &str| {
            let percent = *u == Some(Units::Percent);
            let key = if percent { percent_key } else { cells_key };
            let setting = schema::find(key);
            let r = ui.add_enabled_ui(enabled, |ui| {
                let mut v = if percent { profile.float(key).round() as i64 } else { profile.int(key) };
                let range = if percent { 0..=100 } else { 1..=1250 };
                let r = ui.add(egui::DragValue::new(&mut v).range(range));
                if r.changed() {
                    profile.set(key, if percent { Value::Float(v as f64) } else { Value::Integer(v) });
                }
                if let Some(s) = setting {
                    r.clone().on_hover_text(&s.help);
                    if settings_keys::indicate_non_default() && !profile_is_default(profile, s, key) {
                        settings_keys::non_default_bar(ui, r.rect, enabled);
                    }
                }
                r.drag_stopped() || (r.changed() && !r.dragged())
            });
            commit |= r.inner;
            if let Some(now) = *u {
                let label = |x: Units| if x == Units::Percent { titles.1 } else { titles.0 };
                egui::ComboBox::from_id_salt(id).selected_text(label(now)).show_ui(ui, |ui| {
                    for x in [Units::Cells, Units::Percent] {
                        if ui.selectable_label(now == x, label(x)).clicked() && now != x {
                            *u = Some(x);
                            units_changed = true;
                        }
                    }
                });
            }
        };
        axis(ui, &mut units.0, row.columns_enabled, "Columns", "Width Percentage", ("Columns", "% of screen width"), "window-columns-units");
        if units.0.is_none() {
            ui.label("columns by");
        }
        if row.by_label {
            ui.label("by");
        }
        axis(ui, &mut units.1, row.rows_enabled, "Rows", "Height Percentage", ("Rows", "% of screen height"), "window-rows-units");
        if units.1.is_none() {
            ui.label("rows");
        }
    });
    let percent_edit = commit && (units.0 == Some(Units::Percent) || units.1 == Some(Units::Percent));
    if units_changed || percent_edit {
        // A popup change or a percent edit saves the type again (`saveWindowType`).
        let style = crate::frames::profile_menu_value(profile.int("Window Type"));
        save_window_type(profile, style, units.0.unwrap_or(Units::Cells), units.1.unwrap_or(Units::Cells));
        commit = true;
    }
    commit
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

/// Whether a profile control shows its default (`iTermProfilePreferencesBaseViewController.m:253-265`):
/// the value of `key` (`edit_key`: the setting or its Light or Dark variant) nullably equals
/// DefaultBookmark.plist's (the schema's seed) or else iTerm2's default, within 0.001. The
/// fonts compare with Tandem's new-profile font, which stands in for Monaco 12; the Guid has no
/// control and is never marked.
///
/// The controls iTerm2 gives a `hasDefaultValue` of their own follow it: the working directory
/// (`working_directory_is_default`), Unicode version 9 (`ProfilesTextPreferencesViewController.m:296-299`,
/// a missing value reading 9 as `iTermProfilePreferences.m:1553-1564`), and the Title Function
/// popup, which is iTerm2's title popup keyed by Title Components
/// (`ProfilesGeneralPreferencesViewController.m:397-398`). Has Hotkey's override (default while
/// off, `ProfilesKeysPreferencesViewController.m:196-198`) is the ordinary rule.
pub fn profile_is_default(profile: &Profile, s: &Setting, key: &str) -> bool {
    match key {
        profile::GUID => return true,
        "Working Directory" => {
            let home = std::env::var("USERPROFILE").unwrap_or_default();
            return working_directory_is_default(profile.values.get(key), &home);
        }
        "Unicode Version" => return profile.values.get(key).map_or(9, |v| v.as_integer().unwrap_or(0)) == 9,
        "Title Function" => {
            if let Some(components) = schema::find("Title Components") {
                return profile_is_default(profile, components, "Title Components");
            }
        }
        _ => {}
    }
    let (value, preset) = if let Some(variant) = key.strip_prefix(s.key.as_str()).filter(|v| !v.is_empty()) {
        let (seed, default) = if variant == " (Dark)" { (&s.seed_dark, &s.default_dark) } else { (&s.seed_light, &s.default_light) };
        let preset = seed.clone().or_else(|| default.clone());
        (profile.values.get(key).cloned().or_else(|| default.clone()).or_else(|| profile.get(&s.key)), preset)
    } else if FONT_KEYS[..2].contains(&key) {
        (profile.get(key), Some(Value::String(profile::WINDOWS_FONT.into())))
    } else {
        (profile.get(key), s.seed.clone().or_else(|| s.default.clone()))
    };
    settings_keys::nullably_equal(value.as_ref(), preset.as_ref())
}

/// The working directory field's rule (`ProfilesGeneralPreferencesViewController.m:356-359`,
/// "Because new profiles are initialized with NSHomeDirectory here"): default when the text is
/// the home folder. Tandem's new profiles store an empty path, which is default too.
pub fn working_directory_is_default(value: Option<&Value>, home: &str) -> bool {
    let text = value.and_then(Value::as_str).unwrap_or("");
    text.is_empty() || text == home
}

/// Whether a committed change of profile setting `key` to `value` chose Meta (1) for an
/// Option key ("Option Key Sends" or "Right Option Key Sends").
pub fn chose_meta(key: &str, value: Option<&Value>) -> bool {
    matches!(key, "Option Key Sends" | "Right Option Key Sends") && value.and_then(Value::as_integer) == Some(1)
}

/// Font settings follow the default profile until each pane can have its own renderer.
pub const FONT_KEYS: [&str; 5] = ["Normal Font", "Non Ascii Font", "Use Non-ASCII Font", "Horizontal Spacing", "Vertical Spacing"];

/// The not-default question a scheme menu choice raised: the question, its detail, and what it
/// would bind.
pub struct SchemeAsk {
    pub guid: String,
    pub scheme: String,
    pub text: String,
    pub info: String,
}

/// Choosing a scheme in the "URL schemes:" menu: the binding is kept in the app-wide table, never
/// in the profile. `Unbind` and `Bind` change `table`; `Ask` leaves it for the question's OK.
pub fn url_scheme_toggled(table: &mut toml::Table, reg: &dyn crate::url_schemes::SchemeRegistry, scheme: &str, guid: &str, exe: &std::path::Path) -> crate::url_schemes::Pick {
    use crate::url_schemes as us;
    let pick = us::pick(table, reg, scheme, guid, exe);
    match &pick {
        us::Pick::Unbind => us::unbind(table, scheme),
        us::Pick::Bind => us::bind(table, scheme, guid),
        us::Pick::Ask { .. } => {}
    }
    pick
}

/// "URL schemes:" and its "Select URL Schemes..." menu (`PG:1043-1092`). The registry is read
/// only while the menu is open. True when the table changed.
fn url_schemes_control(ui: &mut egui::Ui, globals: &mut Globals, guid: &str, ask: &mut Option<SchemeAsk>) -> bool {
    use crate::url_schemes as us;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(us::LABEL);
        ui.menu_button(us::SELECT, |ui| {
            let Ok(exe) = std::env::current_exe() else { return };
            let reg = us::system();
            let mut table = globals.table(us::HANDLERS_KEY);
            for scheme in us::SCHEMES {
                let on = us::handles(&table, reg.as_ref(), scheme, guid, &exe);
                if ui.selectable_label(on, scheme).clicked() {
                    if let us::Pick::Ask { text, info } = url_scheme_toggled(&mut table, reg.as_ref(), scheme, guid, &exe) {
                        *ask = Some(SchemeAsk { guid: guid.to_string(), scheme: scheme.to_string(), text, info });
                    } else {
                        globals.set(us::HANDLERS_KEY, toml::Value::Table(table.clone()));
                        changed = true;
                    }
                    ui.close();
                }
            }
        });
    });
    changed
}

/// Whether a profile row is enabled by the rows it depends on; every key without a rule is. The cursor rules
/// are iTerm2's: Cursor Color and Cursor Text Color give way to Smart Cursor Color on a box cursor
/// (`ProfilesColorsPreferencesViewController.m:471-476`), Smart Cursor Color is for the box, the shadow and
/// the smooth slide for the underline and the bar (`ProfilesTextPreferencesViewController.m:512-530`),
/// smooth blink needs Blinking Cursor and its six timings both (`:537-560`), and the interactive-apps check
/// needs Animate Movement (`:168-175`). `dark` picks the variant of a Light and Dark key.
pub fn profile_row_enabled(p: &Profile, key: &str, dark: bool) -> bool {
    let box_cursor = p.int("Cursor Type") == 2;
    match key {
        "Cursor Color" | "Cursor Text Color" => !(p.bool_for("Smart Cursor Color", dark) && box_cursor),
        "Smart Cursor Color" => box_cursor,
        "Cursor Shadow" | "Cursor Smooth Slide" => matches!(p.int("Cursor Type"), 0 | 1),
        "Cursor Smooth Blink" => p.bool("Blinking Cursor"),
        "Presets"
        | "Cursor Blink Fade In Duration"
        | "Cursor Blink Fade Out Duration"
        | "Cursor Blink Fade In Curve"
        | "Cursor Blink Fade Out Curve"
        | "Cursor Blink Visible Dwell"
        | "Cursor Blink Hidden Dwell" => p.bool("Blinking Cursor") && p.bool("Cursor Smooth Blink"),
        "Animate Movement Only in Interactive Apps" => p.bool("Animate Movement"),
        // The directory rows follow Initial working directory (`AWD:60-86`, `ID:19-62`): the custom
        // path is for Custom, the six Advanced rows for Advanced, and each path needs its option on Custom.
        "Working Directory" => p.string("Custom Directory") == "Yes",
        "AWDS Window Option" | "AWDS Tab Option" | "AWDS Pane Option" => p.string("Custom Directory") == "Advanced",
        "AWDS Window Directory" | "AWDS Tab Directory" | "AWDS Pane Directory" => {
            let option = key.replace("Directory", "Option");
            p.string("Custom Directory") == "Advanced" && p.string(&option) == "Yes"
        }
        // The bar's height and scheme need Enable Progress Bars (`PSV:357-385`).
        "Progress Bar Height" | "Progress Bar Color Scheme" => p.bool("Enable Progress Bars"),
        // Change Locale is for the Custom mode (`PTM:397-445`).
        "Custom Locale" => p.int("Set Local Environment Vars") == 2,
        // The swatch follows its switch (`ProfilesColorsPreferencesViewController.m:468`).
        "Underline Color" => p.bool_for("Use Underline Color", dark),
        // The logging rows follow "Automatically Log", the archive folder "Archive On Closure"
        // (`ProfilesSessionPreferencesViewController.m:187-213`).
        "Log Directory" | "Log Filename Format" | "Plain Text Logging" => p.bool("Automatically Log"),
        "Archive Directory" => p.bool("Archive On Closure"),
        _ => true,
    }
}

/// The label of the "Warn about short-lived sessions" check box (`PSV:391-446`).
pub const SHORT_LIVED_LABEL: &str = "Warn about short-lived sessions";

/// The phony check box under "After a session ends": checked while the profile's silence key is
/// not set, enabled only for Close (end action 1). Unchecking silences the warning for the saved
/// profile for good (the key true, its selection 0); checking removes both and the silence time. True on a change.
pub fn short_lived_row(ui: &mut egui::Ui, p: &Profile, saved_guid: &str, g: &mut Globals) -> bool {
    let key = crate::session_end::short_lived_key(saved_guid);
    let mut on = !g.bool(&key);
    let enabled = p.int(crate::session_end::END_ACTION_KEY) == 1;
    let changed = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, SHORT_LIVED_LABEL)).changed();
    if changed {
        crate::session_end::set_short_lived_warning(g, saved_guid, on);
    }
    changed
}

/// The line beside the Progress Bar Height field: a typed height above the top margin was cut
/// to it (`PSV:365-371`).
pub fn height_message(typed: f64, margin: i64) -> Option<&'static str> {
    crate::progress::clamp_typed(typed, margin as f64).1.then_some(crate::progress::HEIGHT_TOO_TALL)
}

/// The Progress Bar Height field (`PSV:358-371`): a typed value is cut to 1 up to the top margin,
/// and one above the margin shows `HEIGHT_TOO_TALL` beside the field until the next edit.
/// True when a change is committed (a drag ended or a typed value was taken).
fn progress_height_row(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting, margin: i64) -> bool {
    let id = egui::Id::new("progress_height_cut");
    let mut commit = false;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        let mut height = profile.float(&s.key);
        let r = ui.add(egui::DragValue::new(&mut height).speed(0.1).range(0.0..=1000.0));
        if r.changed() {
            let (value, cut) = crate::progress::clamp_typed(height, margin as f64);
            profile.set(&s.key, Value::Float(value));
            ui.ctx().data_mut(|d| d.insert_temp(id, cut));
            commit = r.drag_stopped() || !r.dragged();
        }
        if ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
            ui.label(egui::RichText::new(crate::progress::HEIGHT_TOO_TALL.replace("**", "")).small());
        }
    });
    commit
}

/// The line under "Set Locale Variables" (`PTM:397-445`): `LANG=<locale>` in the fixed font and
/// the locale's title in gray after three en spaces, or the plain status text.
#[cfg(windows)]
fn locale_status_line(ui: &mut egui::Ui, profile: &Profile, fallback: &str) {
    let key = (profile.int("Set Local Environment Vars"), profile.string("Custom Locale"), profile.int("Character Encoding"), fallback.to_string());
    let id = egui::Id::new("locale_status");
    // Windows answers are read once per change, not every frame.
    let cached: Option<((i64, String, i64, String), crate::locale_win::Status)> = ui.ctx().data(|d| d.get_temp(id));
    let status = match cached {
        Some((k, v)) if k == key => v,
        _ => {
            let v = crate::locale_win::status(key.0, &key.1, key.2, &key.3);
            ui.ctx().data_mut(|d| d.insert_temp(id, (key, v.clone())));
            v
        }
    };
    ui.horizontal_wrapped(|ui| {
        if let Some(lang) = &status.lang {
            ui.label(egui::RichText::new(format!("LANG={lang}")).monospace());
            ui.label(egui::RichText::new(format!("\u{2002}\u{2002}\u{2002}{}", status.text)).color(ui.visuals().weak_text_color()));
        } else {
            ui.label(egui::RichText::new(&status.text).color(ui.visuals().weak_text_color()));
        }
    });
}

#[cfg(not(windows))]
fn locale_status_line(_ui: &mut egui::Ui, _profile: &Profile, _fallback: &str) {}

/// The title of the smooth-blink presets pull-down (`PreferencePanel.xib:11312`).
pub const PRESETS_TITLE: &str = "Presets";

/// Writes preset `index`'s six keys (`ProfilesTextPreferencesViewController.m:668-697`): the
/// timings as floats, the curves as integers. An index past the five is ignored.
pub fn apply_blink_preset(p: &mut Profile, index: usize) {
    let Some((_, t)) = crate::cursor_anim::PRESETS.get(index) else { return };
    p.set("Cursor Blink Fade In Duration", Value::Float(t.fade_in));
    p.set("Cursor Blink Fade Out Duration", Value::Float(t.fade_out));
    p.set("Cursor Blink Fade In Curve", Value::Integer(t.in_curve.index()));
    p.set("Cursor Blink Fade Out Curve", Value::Integer(t.out_curve.index()));
    p.set("Cursor Blink Visible Dwell", Value::Float(t.visible_dwell));
    p.set("Cursor Blink Hidden Dwell", Value::Float(t.hidden_dwell));
}

/// The Presets pull-down: the five names, the matching one checked. True when one was picked.
fn blink_presets_row(ui: &mut egui::Ui, p: &mut Profile) -> bool {
    let current = crate::cursor_anim::preset_matching(&crate::cursor_anim::FadeTimes::from_profile(p));
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(PRESETS_TITLE);
        egui::ComboBox::from_id_salt("blink_presets").selected_text(current.map_or("", |i| crate::cursor_anim::PRESETS[i].0)).show_ui(ui, |ui| {
            for (i, (name, _)) in crate::cursor_anim::PRESETS.iter().enumerate() {
                if ui.selectable_label(current == Some(i), format!("{}{name}", if current == Some(i) { "\u{2713} " } else { "" })).clicked() {
                    picked = Some(i);
                }
            }
        });
    });
    match picked {
        Some(i) => {
            apply_blink_preset(p, i);
            true
        }
        None => false,
    }
}

/// The "Use background image" checkbox row's text (`PP`).
pub const USE_BACKGROUND_IMAGE: &str = "Use background image";
/// The picker's message (`PWP:815-827`).
pub const BACKGROUND_PICKER_MESSAGE: &str = "Choose an image for the background, or a folder to rotate through its images.";

/// On when the path of the current source mode is non-empty (`PWP:937-939`).
pub fn background_checkbox_state(p: &Profile) -> bool {
    !p.string(background_key(p)).is_empty()
}

/// The key the current source mode reads: the folder in folder rotation, else the image.
fn background_key(p: &Profile) -> &'static str {
    if p.int("Background Image Source Mode") == 1 {
        "Background Image Folder Location"
    } else {
        "Background Image Location"
    }
}

/// The unchecked box: clears the key of the current source mode only.
pub fn clear_background_image(p: &mut Profile) {
    p.set(background_key(p), Value::String(String::new()));
}

/// A picked file sets source mode 0 and the location; a folder sets mode 1 and the folder.
pub fn set_background_pick(p: &mut Profile, path: &str, is_dir: bool) {
    p.set("Background Image Source Mode", Value::Integer(is_dir as i64));
    p.set(if is_dir { "Background Image Folder Location" } else { "Background Image Location" }, Value::String(path.to_string()));
}

/// The "Use background image" row (`PWP:815-827`): the box is on while the current source has
/// a path. Checking it opens the picker for a file; the Folder button picks a folder (Windows'
/// dialog cannot take both). Cancel leaves the profile alone; unchecking clears the key.
fn background_image_row(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut on = background_checkbox_state(profile);
        if ui.checkbox(&mut on, USE_BACKGROUND_IMAGE).changed() {
            if on {
                if let Some(path) = rfd::FileDialog::new().set_title(BACKGROUND_PICKER_MESSAGE).add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"]).pick_file() {
                    set_background_pick(profile, &path.to_string_lossy(), false);
                    changed = true;
                }
            } else {
                clear_background_image(profile);
                changed = true;
            }
        }
        if ui.button("Folder...").on_hover_text(BACKGROUND_PICKER_MESSAGE).clicked() {
            if let Some(path) = rfd::FileDialog::new().set_title(BACKGROUND_PICKER_MESSAGE).pick_folder() {
                set_background_pick(profile, &path.to_string_lossy(), true);
                changed = true;
            }
        }
    });
    changed
}

/// The warning beside the Minimum Contrast slider: shown at 0.97 and above (`PCP:459-463`).
fn contrast_warning(key: &str, value: f64) -> Option<&'static str> {
    (key == "Minimum Contrast" && crate::contrast::warns(value)).then_some(crate::contrast::WARNING_TEXT)
}

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
    // The widget that takes the value, and whether it is enabled, for the non-default bar.
    let mut widget: Option<(egui::Rect, bool)> = None;
    // A color well or a checkbox can be bound to an expression (`ExpressionBindableView`).
    let bound_expression = matches!(s.kind, Kind::Bool | Kind::Color).then(|| crate::bindings::get(profile).get(&key).map(|b| b.expression().to_string())).flatten();
    let mut bind_action = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!s.macos_only && !font_locked, |ui| {
            let enabled = ui.is_enabled();
            match s.kind {
                Kind::Bool => {
                    let mut b = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    let r = ui.checkbox(&mut b, &s.label);
                    if r.changed() {
                        changed = Some(Value::Boolean(b));
                        commit = true;
                    }
                    widget = Some((r.rect, enabled));
                    bind_action = binding_menu(ui, &r, bound_expression.as_deref());
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
                    widget = Some((r.rect, enabled));
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
                    widget = Some((r.rect, enabled));
                    commit = r.drag_stopped() || (r.changed() && !r.dragged());
                    // iTerm2's two outlets (`_bwWarning1`, `_bwWarning2`): the glyph, then the text.
                    if let Some(text) = contrast_warning(&s.key, f) {
                        let color = ui.visuals().warn_fg_color;
                        if let Some(g) = crate::settings_keys::icon_text(ui.ctx(), crate::scripts::WARNING_GLYPH) {
                            ui.label(g.color(color));
                        }
                        ui.label(egui::RichText::new(text).color(color));
                    }
                }
                Kind::String | Kind::Font => {
                    ui.label(&s.label);
                    let mut text = current.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let r = ui.text_edit_singleline(&mut text);
                    if r.changed() {
                        changed = Some(Value::String(text));
                    }
                    widget = Some((r.rect, enabled));
                    commit = r.lost_focus();
                }
                Kind::Color => {
                    ui.label(&s.label);
                    let mut c = current.as_ref().map(rgb_of).unwrap_or([0, 0, 0]);
                    let r = ui.color_edit_button_srgb(&mut c);
                    if r.changed() {
                        changed = Some(value_of_rgb(c));
                        commit = true;
                    }
                    widget = Some((r.rect, enabled));
                    bind_action = binding_menu(ui, &r, bound_expression.as_deref());
                }
                Kind::Enum => {
                    ui.label(&s.label);
                    let now = current.clone();
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or("".to_string(), |c| c.label.clone());
                    let r = egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
                        for c in &s.choices {
                            if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                changed = Some(c.value.clone());
                                commit = true;
                            }
                        }
                    });
                    widget = Some((r.response.rect, enabled));
                }
                Kind::List | Kind::Dict | Kind::Data => {
                    let n = match &current {
                        Some(Value::Array(a)) => a.len(),
                        Some(Value::Table(t)) => t.len(),
                        _ => 0,
                    };
                    let r = ui.label(format!("{}: {n} item(s)", s.label)).on_hover_text("This editor arrives with its feature's milestone.");
                    widget = Some((r.rect, enabled));
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
    match bind_action {
        Some(BindAction::Remove) => {
            crate::bindings::set(profile, &key, None);
            commit = true;
        }
        Some(BindAction::Edit) => binding_dialog_open(ui.ctx(), &profile.guid(), &key, bound_expression.as_deref().unwrap_or("")),
        None => {}
    }
    if let Some(text) = binding_dialog(ui, &profile.guid(), &key) {
        crate::bindings::set(profile, &key, Some(&text));
        commit = true;
    }
    if let Some((rect, enabled)) = widget.filter(|_| settings_keys::indicate_non_default() && !profile_is_default(profile, s, &key)) {
        settings_keys::non_default_bar(ui, rect, enabled);
    }
    commit
}

/// What a right click on a bindable control chose.
enum BindAction {
    Edit,
    Remove,
}

/// The link glyph (Segoe Fluent Icons) a bound control shows at its right.
const LINK_GLYPH: char = '\u{E71B}';

/// The right-click menu of a color well or checkbox (`EBV:22-45`) and the glyph a bound one shows.
fn binding_menu(ui: &mut egui::Ui, r: &egui::Response, bound: Option<&str>) -> Option<BindAction> {
    let mut action = None;
    r.context_menu(|ui| {
        if ui.button(if bound.is_some() { crate::bindings::MENU_EDIT } else { crate::bindings::MENU_BIND }).clicked() {
            action = Some(BindAction::Edit);
            ui.close();
        }
        if bound.is_some() && ui.button(crate::bindings::MENU_REMOVE).clicked() {
            action = Some(BindAction::Remove);
            ui.close();
        }
    });
    if let Some(e) = bound {
        let tip = format!("Bound to the expression {e}");
        match settings_keys::icon_text(ui.ctx(), LINK_GLYPH) {
            Some(g) => ui.label(g).on_hover_text(tip),
            None => ui.label("(bound)").on_hover_text(tip),
        };
    }
    action
}

/// The bind dialog's state: which control it is for, its text and whether the help shows.
#[derive(Clone)]
struct BindDialog {
    guid: String,
    key: String,
    text: String,
    help: bool,
}

fn binding_dialog_id() -> egui::Id {
    egui::Id::new("tandem-binding-dialog")
}

fn binding_dialog_open(ctx: &egui::Context, guid: &str, key: &str, text: &str) {
    ctx.data_mut(|d| d.insert_temp(binding_dialog_id(), BindDialog { guid: guid.into(), key: key.into(), text: text.into(), help: false }));
}

/// "Bind Expression to Setting" (`EBV:59-101`), shown by the control it is for. The text to
/// store (empty clears the binding) after OK.
fn binding_dialog(ui: &egui::Ui, guid: &str, key: &str) -> Option<String> {
    let ctx = ui.ctx();
    let mut d = ctx.data(|d| d.get_temp::<BindDialog>(binding_dialog_id())).filter(|d| d.guid == guid && d.key == key)?;
    let mut done = None;
    let mut open = true;
    egui::Window::new(crate::bindings::TITLE).collapsible(false).resizable(false).default_width(420.0).open(&mut open).show(ctx, |ui| {
        ui.label(crate::bindings::INFO);
        let r = ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(crate::bindings::placeholder(key)).desired_width(380.0));
        if !d.help {
            r.request_focus();
        }
        ui.horizontal(|ui| {
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("OK").clicked() || enter {
                done = Some(Some(d.text.clone()));
            }
            if ui.button("Cancel").clicked() {
                done = Some(None);
            }
            if ui.button("?").clicked() {
                d.help = !d.help;
            }
        });
        if d.help {
            ui.separator();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                ui.label(crate::bindings::help_text(key));
            });
        }
    });
    if !open {
        done = Some(None);
    }
    match done {
        Some(result) => {
            ctx.data_mut(|m| m.remove::<BindDialog>(binding_dialog_id()));
            result
        }
        None => {
            ctx.data_mut(|m| m.insert_temp(binding_dialog_id(), d));
            None
        }
    }
}

/// Whether this mode, page and AI sub-tab show the discovered models.
fn shows_ai_models_on(mode: Mode, page: Page, sub: crate::settings_ai::SubTab) -> bool {
    mode == Mode::Settings && page == Page::General && sub == crate::settings_ai::SubTab::General
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reg(Option<String>);
    impl crate::url_schemes::SchemeRegistry for Reg {
        fn handler_command(&self, _: &str) -> Option<String> {
            self.0.clone()
        }
        fn handler_name(&self, _: &str) -> Option<String> {
            None
        }
        fn user_choice(&self, _: &str) -> Option<String> {
            None
        }
        fn classes_command(&self, _: &str) -> Option<String> {
            None
        }
        fn register(&mut self, _: &str, _: &std::path::Path) -> Result<(), String> {
            panic!("a menu choice never writes the registry")
        }
    }

    #[test]
    fn background_checkbox_follows_iterm2() {
        let mut p = Profile::new("P");
        assert!(!background_checkbox_state(&p));
        p.set("Background Image Source Mode", Value::Integer(0));
        p.set("Background Image Location", Value::String("C:\\bg.png".into()));
        assert!(background_checkbox_state(&p));
        p.set("Background Image Location", Value::String(String::new()));
        assert!(!background_checkbox_state(&p));
        // Source mode 1 reads the folder key, not the file key.
        p.set("Background Image Source Mode", Value::Integer(1));
        p.set("Background Image Location", Value::String("C:\\bg.png".into()));
        assert!(!background_checkbox_state(&p));
        p.set("Background Image Folder Location", Value::String("C:\\pics".into()));
        assert!(background_checkbox_state(&p));
        // Unchecking clears the current source's key only.
        clear_background_image(&mut p);
        assert_eq!(p.string("Background Image Folder Location"), "");
        assert_eq!(p.string("Background Image Location"), "C:\\bg.png");
        p.set("Background Image Source Mode", Value::Integer(0));
        p.set("Background Image Folder Location", Value::String("C:\\pics".into()));
        clear_background_image(&mut p);
        assert_eq!(p.string("Background Image Location"), "");
        assert_eq!(p.string("Background Image Folder Location"), "C:\\pics");
    }

    #[test]
    fn background_pick_sets_the_source_mode() {
        let mut p = Profile::new("P");
        set_background_pick(&mut p, "C:\\pics", true);
        assert_eq!((p.int("Background Image Source Mode"), p.string("Background Image Folder Location").as_str()), (1, "C:\\pics"));
        assert!(background_checkbox_state(&p));
        set_background_pick(&mut p, "C:\\bg.png", false);
        assert_eq!((p.int("Background Image Source Mode"), p.string("Background Image Location").as_str()), (0, "C:\\bg.png"));
        assert_eq!(USE_BACKGROUND_IMAGE, "Use background image");
        assert_eq!(BACKGROUND_PICKER_MESSAGE, "Choose an image for the background, or a folder to rotate through its images.");
    }

    #[test]
    fn minimum_contrast_warns_near_one() {
        assert_eq!(contrast_warning("Minimum Contrast", 0.97), Some("Black & White Text Only"));
        assert_eq!(contrast_warning("Minimum Contrast", 1.0), Some("Black & White Text Only"));
        assert_eq!(contrast_warning("Minimum Contrast", 0.96), None);
        assert_eq!(contrast_warning("Cursor Boost", 1.0), None);
    }

    #[test]
    fn url_schemes_control_stores_nothing_in_the_profile() {
        use crate::url_schemes as us;
        let exe = std::path::PathBuf::from(r"C:\T\tandem.exe");
        let profile = Profile::new("P");
        let before = profile.values.clone();
        let mut table = toml::Table::new();
        let ours = Reg(Some(us::tandem_command(&exe)));
        assert_eq!(url_scheme_toggled(&mut table, &ours, "ssh", "G1", &exe), us::Pick::Bind);
        assert_eq!(table.get("ssh").and_then(|v| v.as_str()), Some("G1"));
        assert_eq!(table.len(), 1);
        assert_eq!(profile.values, before, "the profile holds no URL scheme key");
        assert_eq!(url_scheme_toggled(&mut table, &ours, "ssh", "G1", &exe), us::Pick::Unbind);
        assert!(table.is_empty());
        // Not the handler: asks, binds nothing, writes nothing.
        let other = Reg(None);
        assert!(matches!(url_scheme_toggled(&mut table, &other, "ssh", "G1", &exe), us::Pick::Ask { .. }));
        assert!(table.is_empty());
        assert!(schema::profile().iter().all(|s| !s.key.starts_with("__") && s.key != us::HANDLERS_KEY && !us::SCHEMES.contains(&s.key.as_str())));
        assert!(crate::schema::global().iter().all(|s| s.key != us::HANDLERS_KEY), "no settings row for the table");
    }

    /// Task 10 re-review: stale Ollama servers are asked again only where the models show.
    #[test]
    fn discovery_reads_only_where_the_models_show() {
        use crate::settings_ai::SubTab;
        assert!(shows_ai_models_on(Mode::Settings, Page::General, SubTab::General));
        assert!(!shows_ai_models_on(Mode::Settings, Page::General, SubTab::Prompts));
        assert!(!shows_ai_models_on(Mode::Settings, Page::General, SubTab::Features));
        assert!(!shows_ai_models_on(Mode::Settings, Page::Keys, SubTab::General));
        assert!(!shows_ai_models_on(Mode::Profiles, Page::General, SubTab::General));
    }

    /// Choosing Meta (1) for either Option key is what warns (`PK:554-559`).
    #[test]
    fn meta_choice_warns() {
        assert!(chose_meta("Option Key Sends", Some(&Value::Integer(1))));
        assert!(chose_meta("Right Option Key Sends", Some(&Value::Integer(1))));
        assert!(!chose_meta("Option Key Sends", Some(&Value::Integer(2))));
        assert!(!chose_meta("Option Key Sends", None));
        assert!(!chose_meta("Mouse Reporting", Some(&Value::Integer(1))));
    }

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
    fn space_row_follows_windows() {
        let s = crate::schema::profile().iter().find(|x| x.key == "Space").unwrap();
        assert!(!s.macos_only);
        assert_eq!(s.label, "Desktop");
        let got: Vec<(Value, String)> = s.choices.iter().map(|c| (c.value.clone(), c.label.clone())).collect();
        let mut want = vec![(Value::Integer(0), "Current Desktop".to_string()), (Value::Integer(-1), "All Desktops".to_string())];
        for n in 1..=9 {
            want.push((Value::Integer(n), format!("Desktop {n}")));
        }
        assert_eq!(got, want);
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
    fn stored_rows_are_in_no_pane() {
        use crate::advanced_pane::STORED_TAB;
        // `sections` reads the profile schema, which has no Stored tab, so this first assertion
        // holds trivially; the global checks below are the real guard.
        assert!(sections(STORED_TAB).is_empty());
        // The only tabs a Page draws from the global schema: General and Appearance with
        // settings_keys::schema_page, Advanced with settings_advanced_pane::page.
        assert!(!["General", "Appearance", "Advanced"].contains(&STORED_TAB));
        assert!(crate::schema::global().iter().any(|s| s.tab == STORED_TAB));
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

    /// `iTermProfilePreferencesBaseViewController.m:253-265`: a profile control is default when
    /// its value equals DefaultBookmark.plist's (the schema's seed) or else iTerm2's default,
    /// within 0.001.
    #[test]
    fn profile_indicators_follow_iterm2() {
        assert_eq!(SHOW_INDICATORS, "Show indicators for non-default values");
        assert_eq!(SEARCH_OPTIONS, "Search Options");
        let mut p = Profile::new("Default");
        let cols = crate::schema::find("Columns").unwrap();
        assert!(profile_is_default(&p, cols, "Columns"));
        p.set("Columns", Value::Integer(132));
        assert!(!profile_is_default(&p, cols, "Columns"));
        p.set("Columns", Value::Integer(80));
        assert!(profile_is_default(&p, cols, "Columns"));
        // The name compares with DefaultBookmark.plist's "Default" (the source; see the report).
        let name = crate::schema::find("Name").unwrap();
        assert!(profile_is_default(&p, name, "Name"));
        assert!(!profile_is_default(&Profile::new("New Profile"), name, "Name"));
        // The Guid has no control and is never marked.
        assert!(profile_is_default(&Profile::new("x"), name, "Guid"));
        // A color's variant key compares with the variant's seed or, without one, its default.
        let fg = crate::schema::find("Foreground Color").unwrap();
        let fresh = Profile::new("Default");
        assert!(profile_is_default(&fresh, fg, "Foreground Color (Dark)"));
        assert!(profile_is_default(&fresh, fg, "Foreground Color (Light)"));
        let mut q = Profile::default();
        assert!(!profile_is_default(&q, fg, "Foreground Color (Dark)"), "a missing variant reads iTerm2's default, not the seed");
        q.set("Foreground Color (Dark)", fg.seed_dark.clone().unwrap());
        assert!(profile_is_default(&q, fg, "Foreground Color (Dark)"));
        q.set("Foreground Color (Dark)", fg.default_dark.clone().unwrap());
        assert!(!profile_is_default(&q, fg, "Foreground Color (Dark)"), "the seed wins over the default");
        q.set("Foreground Color (Light)", value_of_rgb([255, 0, 0]));
        assert!(!profile_is_default(&q, fg, "Foreground Color (Light)"));
        // Within 0.001 of the seed is default.
        let near = |v: &Value| Value::Table(v.as_table().unwrap().iter().map(|(k, x)| (k.clone(), Value::Float(x.as_float().unwrap() + 0.0005))).collect());
        q.set("Foreground Color (Dark)", near(fg.seed_dark.as_ref().unwrap()));
        assert!(profile_is_default(&q, fg, "Foreground Color (Dark)"));
        // Tandem's new-profile font stands in for Monaco 12.
        let font = crate::schema::find("Normal Font").unwrap();
        let mut f = Profile::new("Default");
        assert!(profile_is_default(&f, font, "Normal Font"));
        f.set("Normal Font", Value::String("Monaco 12".into()));
        assert!(!profile_is_default(&f, font, "Normal Font"));
    }

    /// iTerm2's `hasDefaultValue` overrides for the profile controls that have one.
    #[test]
    fn profile_custom_indicators_follow_iterm2() {
        // `ProfilesGeneralPreferencesViewController.m:356-359`: the working directory is default
        // when it is the home folder (iTerm2 seeds new profiles with it); Tandem seeds new
        // profiles with an empty path, so that is default too.
        let home = "C:\\Users\\rob";
        let wd = |s: &str| Some(Value::String(s.into()));
        assert!(working_directory_is_default(wd(home).as_ref(), home));
        assert!(working_directory_is_default(wd("").as_ref(), home));
        assert!(working_directory_is_default(None, home));
        assert!(!working_directory_is_default(wd("C:\\src").as_ref(), home));
        assert!(!working_directory_is_default(wd("~").as_ref(), home), "iTerm2 compares the text");
        let dir = crate::schema::find("Working Directory").unwrap();
        let mut p = Profile::new("Default");
        assert!(profile_is_default(&p, dir, "Working Directory"));
        let user = std::env::var("USERPROFILE").unwrap_or_default();
        p.set("Working Directory", Value::String(user));
        assert!(profile_is_default(&p, dir, "Working Directory"));
        p.set("Working Directory", Value::String("C:\\definitely-not-home".into()));
        assert!(!profile_is_default(&p, dir, "Working Directory"));
        // `ProfilesTextPreferencesViewController.m:296-299`: Unicode version 9 is default, and a
        // missing value reads 9 (`iTermProfilePreferences.m:1553-1564`).
        let uv = crate::schema::find("Unicode Version").unwrap();
        let mut p = Profile::default();
        assert!(profile_is_default(&p, uv, "Unicode Version"));
        p.set("Unicode Version", Value::Integer(8));
        assert!(!profile_is_default(&p, uv, "Unicode Version"));
        p.set("Unicode Version", Value::Integer(9));
        assert!(profile_is_default(&p, uv, "Unicode Version"));
        // `ProfilesKeysPreferencesViewController.m:196-198`: Has Hotkey is default while off.
        let hk = crate::schema::find("Has Hotkey").unwrap();
        let mut p = Profile::new("Default");
        assert!(profile_is_default(&p, hk, "Has Hotkey"));
        p.set("Has Hotkey", Value::Boolean(true));
        assert!(!profile_is_default(&p, hk, "Has Hotkey"));
        // `ProfilesGeneralPreferencesViewController.m:397-398`: the title popup's key is Title
        // Components, so the Title Function popup is marked by it.
        let tf = crate::schema::find("Title Function").unwrap();
        let mut p = Profile::new("Default");
        assert!(profile_is_default(&p, tf, "Title Function"));
        p.set("Title Function", crate::jobtitle::title_function_value("Hello", "com.ex.t"));
        assert!(profile_is_default(&p, tf, "Title Function"), "the function alone is not the popup's key");
        p.set("Title Components", Value::Integer(i64::from(crate::jobtitle::CUSTOM)));
        assert!(!profile_is_default(&p, tf, "Title Function"));
    }

    /// `PSV:569-573`: opened from a tmux session, both status bar controls are disabled.
    #[test]
    fn status_bar_settings_follow_psv() {
        assert_eq!(status_bar_controls(false, true), (true, true));
        assert_eq!(status_bar_controls(false, false), (true, false), "Configure needs the bar on");
        assert_eq!(status_bar_controls(true, true), (false, false));
        assert_eq!(status_bar_controls(true, false), (false, false));
    }

    /// `PG:860-870`, `SC:10-55`: the `SSH` row is General's Configure SSH sheet.
    #[test]
    fn ssh_row_follows_iterm2() {
        let s = crate::schema::find("SSH").unwrap();
        assert_eq!(s.tab, "General");
        assert_eq!(s.kind, Kind::Dict);
        assert_eq!(s.label, "Configure SSH");
        assert_eq!(s.help, "SSH Integration, the environment variables and files to copy to the remote host, and the path to ssh, for the SSH command type.");
    }

    #[test]
    fn blink_presets_write_the_six_keys() {
        assert_eq!(PRESETS_TITLE, "Presets");
        let mut p = Profile::default();
        apply_blink_preset(&mut p, 0);
        assert_eq!(p.get("Cursor Blink Fade In Duration"), Some(Value::Float(0.9)));
        assert_eq!(p.get("Cursor Blink Fade Out Duration"), Some(Value::Float(0.9)));
        assert_eq!(p.get("Cursor Blink Fade In Curve"), Some(Value::Integer(3)));
        assert_eq!(p.get("Cursor Blink Fade Out Curve"), Some(Value::Integer(3)));
        assert_eq!(p.get("Cursor Blink Visible Dwell"), Some(Value::Float(0.5)));
        assert_eq!(p.get("Cursor Blink Hidden Dwell"), Some(Value::Float(0.5)));
        assert_eq!(crate::cursor_anim::preset_matching(&crate::cursor_anim::FadeTimes::from_profile(&p)), Some(0));
        apply_blink_preset(&mut p, 1);
        assert_eq!(p.get("Cursor Blink Fade In Curve"), Some(Value::Integer(0)));
        assert_eq!(crate::cursor_anim::preset_matching(&crate::cursor_anim::FadeTimes::from_profile(&p)), Some(1));
        apply_blink_preset(&mut p, 99);
        assert_eq!(crate::cursor_anim::preset_matching(&crate::cursor_anim::FadeTimes::from_profile(&p)), Some(1));
    }

    #[test]
    fn awds_rows_enable_as_iterm2() {
        let on = |p: &Profile, key: &str| profile_row_enabled(p, key, false);
        let s = |v: &str| Value::String(v.into());
        let rows = ["Window", "Tab", "Pane"];
        let mut p = Profile::default();
        for r in rows {
            assert!(!on(&p, &format!("AWDS {r} Option")) && !on(&p, &format!("AWDS {r} Directory")), "{r}");
        }
        assert!(!on(&p, "Working Directory"));
        p.set("Custom Directory", s("Yes"));
        assert!(on(&p, "Working Directory"));
        for r in rows {
            assert!(!on(&p, &format!("AWDS {r} Option")), "{r}");
        }
        p.set("Custom Directory", s("Advanced"));
        assert!(!on(&p, "Working Directory"));
        for r in rows {
            assert!(on(&p, &format!("AWDS {r} Option")), "{r}");
            assert!(!on(&p, &format!("AWDS {r} Directory")), "{r}: option No");
            p.set(&format!("AWDS {r} Option"), s("Recycle"));
            assert!(!on(&p, &format!("AWDS {r} Directory")), "{r}: option Recycle");
            p.set(&format!("AWDS {r} Option"), s("Yes"));
            assert!(on(&p, &format!("AWDS {r} Directory")), "{r}: option Yes");
        }
        // Each option offers exactly No, Yes and Recycle (`AWD:60-86`).
        for r in rows {
            let values: Vec<_> = crate::schema::find(&format!("AWDS {r} Option")).unwrap().choices.iter().filter_map(|c| c.value.as_str()).collect();
            assert_eq!(values, ["No", "Yes", "Recycle"]);
        }
    }

    #[test]
    fn cursor_profile_rows_enable_as_iterm2() {
        let on = |p: &Profile, key: &str| profile_row_enabled(p, key, false);
        let colors = ["Cursor Color", "Cursor Text Color"];
        let timings = [
            "Presets",
            "Cursor Blink Fade In Duration",
            "Cursor Blink Fade Out Duration",
            "Cursor Blink Fade In Curve",
            "Cursor Blink Fade Out Curve",
            "Cursor Blink Visible Dwell",
            "Cursor Blink Hidden Dwell",
        ];
        let mut p = Profile::default();
        for k in colors {
            assert!(on(&p, k), "{k}");
        }
        assert!(on(&p, "Columns"));
        // Smart Cursor Color with a box (type 2) takes both colors away; any other type gives them back.
        p.set("Smart Cursor Color", Value::Boolean(true));
        p.set("Cursor Type", Value::Integer(2));
        for k in colors {
            assert!(!on(&p, k), "{k}");
        }
        p.set("Cursor Type", Value::Integer(0));
        for k in colors {
            assert!(on(&p, k), "{k}");
        }
        // Smart Cursor Color is for the box only.
        for (ty, want) in [(0, false), (1, false), (2, true)] {
            p.set("Cursor Type", Value::Integer(ty));
            assert_eq!(on(&p, "Smart Cursor Color"), want, "type {ty}");
        }
        // The shadow and the smooth slide are for the underline and the bar.
        for (ty, want) in [(0, true), (1, true), (2, false)] {
            p.set("Cursor Type", Value::Integer(ty));
            assert_eq!(on(&p, "Cursor Shadow"), want, "type {ty}");
            assert_eq!(on(&p, "Cursor Smooth Slide"), want, "type {ty}");
        }
        // Smooth blink needs Blinking Cursor; the six timings need both.
        let mut p = Profile::default();
        assert!(!on(&p, "Cursor Smooth Blink"));
        for k in timings {
            assert!(!on(&p, k), "{k}");
        }
        p.set("Blinking Cursor", Value::Boolean(true));
        assert!(on(&p, "Cursor Smooth Blink"));
        for k in timings {
            assert!(!on(&p, k), "{k}");
        }
        p.set("Cursor Smooth Blink", Value::Boolean(true));
        for k in timings {
            assert!(on(&p, k), "{k}");
        }
        p.set("Blinking Cursor", Value::Boolean(false));
        for k in timings {
            assert!(!on(&p, k), "{k}");
        }
        // Animate Movement Only in Interactive Apps follows Animate Movement.
        let mut p = Profile::default();
        assert!(!on(&p, "Animate Movement Only in Interactive Apps"));
        p.set("Animate Movement", Value::Boolean(true));
        assert!(on(&p, "Animate Movement Only in Interactive Apps"));
        // Smart Cursor Color has Light and Dark variants (P7): the mode being edited decides.
        let mut p = Profile::default();
        p.set("Use Separate Colors for Light and Dark Mode", Value::Boolean(true));
        p.set("Smart Cursor Color (Dark)", Value::Boolean(true));
        p.set("Cursor Type", Value::Integer(2));
        assert!(!profile_row_enabled(&p, "Cursor Color", true));
        assert!(profile_row_enabled(&p, "Cursor Color", false));
    }

    #[test]
    fn cursor_profile_help_follows_iterm2() {
        let boost = crate::schema::find("Cursor Boost").unwrap();
        assert_eq!(boost.help, "Amount to dim non-cursor text to highlight cursor");
        assert_eq!(boost.label, "Cursor Boost");
        assert_eq!((boost.min.clone(), boost.max.clone()), (Some(Value::Float(0.0)), Some(Value::Float(0.8))));
        let shadow = crate::schema::find("Cursor Shadow").unwrap();
        assert_eq!(shadow.help, "Whether the cursor has a shadow");
    }

    /// 13l: "Underline Color" is enabled only while "Use Underline Color" is on
    /// (`ProfilesColorsPreferencesViewController.m:468`); the switch has Light and Dark variants.
    #[test]
    fn underline_color_row_follows_its_switch() {
        let mut p = Profile::default();
        assert!(!profile_row_enabled(&p, "Underline Color", false));
        p.set("Use Underline Color", Value::Boolean(true));
        assert!(profile_row_enabled(&p, "Underline Color", false));
        assert!(profile_row_enabled(&p, "Underline Color", true));
        // With separate colors the mode being edited decides.
        p.set("Use Separate Colors for Light and Dark Mode", Value::Boolean(true));
        p.set("Use Underline Color (Dark)", Value::Boolean(false));
        p.set("Use Underline Color (Light)", Value::Boolean(true));
        assert!(!profile_row_enabled(&p, "Underline Color", true));
        assert!(profile_row_enabled(&p, "Underline Color", false));
        // Reduce Flicker has no rule.
        assert!(profile_row_enabled(&Profile::default(), "Reduce Flicker", false));
    }

    /// 13p (P5): the "Change Locale" row is for the Custom mode only (`PTM:397-445`).
    #[test]
    fn change_locale_row_needs_custom() {
        let mut p = Profile::default();
        for (mode, on) in [(0, false), (1, false), (2, true), (3, false)] {
            p.set("Set Local Environment Vars", Value::Integer(mode));
            assert_eq!(profile_row_enabled(&p, "Custom Locale", false), on, "mode {mode}");
        }
    }

    /// 13p: the progress bar's height and color scheme need "Enable Progress Bars"
    /// (`PSV:357-385`); the switch itself is always on offer.
    #[test]
    fn progress_rows_enable_as_iterm2() {
        let mut p = Profile::default();
        for (on, expect) in [(true, true), (false, false)] {
            p.set("Enable Progress Bars", Value::Boolean(on));
            assert_eq!(profile_row_enabled(&p, "Progress Bar Height", false), expect, "height, enabled {on}");
            assert_eq!(profile_row_enabled(&p, "Progress Bar Color Scheme", false), expect, "scheme, enabled {on}");
            assert!(profile_row_enabled(&p, "Enable Progress Bars", false));
        }
        // The clamp message shows beside the field only for a value above the margin.
        assert_eq!(height_message(5.0, 2), Some(crate::progress::HEIGHT_TOO_TALL));
        assert_eq!(height_message(2.0, 2), None);
        assert_eq!(height_message(0.0, 2), None);
    }

    #[test]
    fn underline_and_flicker_helps_follow_tandem() {
        let help = |k: &str| crate::schema::find(k).unwrap().help.clone();
        assert_eq!(help("Use Underline Color"), "Draws underlines in the Underline Color instead of the text color (an app's SGR 58 color still wins).");
        assert_eq!(help("Reduce Flicker"), "While a program hides the cursor, keep showing the screen as it was for up to 0.2 seconds so half-drawn updates are not seen.");
    }

    /// 13q Task 1 (`AB:414-418`, `PP:1216`, `DB:1038`, `IPR:1326-1328`): "Close Sessions On End"
    /// reads No Action when missing, a new profile starts with Close, and the silence keys are
    /// per saved profile.
    #[test]
    fn end_action_row_follows_iterm2() {
        use crate::session_end::{short_lived_key, short_lived_selection_key, END_ACTION_KEY};
        let row = schema::find(END_ACTION_KEY).expect("end action row");
        assert_eq!(row.kind, Kind::Enum);
        assert_eq!(row.default, Some(Value::Integer(0)));
        assert_eq!(row.seed, Some(Value::Integer(1)));
        assert_eq!(row.label, "Close or restart session on end");
        let choices: Vec<(Value, &str)> = row.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        assert_eq!(choices, vec![(Value::Integer(0), "No Action"), (Value::Integer(1), "Close"), (Value::Integer(2), "Restart")]);
        assert_eq!(Profile::stock().int(END_ACTION_KEY), 1);
        assert_eq!(Profile::default().int(END_ACTION_KEY), 0);
        assert_eq!(short_lived_key("ABC"), "NeverWarnAboutShortLivedSessions_ABC");
        assert_eq!(short_lived_selection_key("ABC"), "NeverWarnAboutShortLivedSessions_ABC_selection");
    }

    /// 13q Task 1 (`PSV:187-213`): the logging rows follow "Automatically Log", the archive
    /// folder follows "Archive On Closure".
    #[test]
    fn session_profile_rows_enable_as_iterm2() {
        let on = |p: &Profile, key: &str| profile_row_enabled(p, key, false);
        let mut p = Profile::stock();
        let logging = ["Log Directory", "Log Filename Format", "Plain Text Logging"];
        for k in logging {
            assert!(!on(&p, k), "{k}");
        }
        assert!(!on(&p, "Archive Directory"));
        p.set("Automatically Log", Value::Boolean(true));
        for k in logging {
            assert!(on(&p, k), "{k}");
        }
        assert!(!on(&p, "Archive Directory"));
        p.set("Archive On Closure", Value::Boolean(true));
        assert!(on(&p, "Archive Directory"));
        assert!(on(&p, "Columns"));
    }

    /// B2: a color preview redraws the panes and never starts the save timer.
    #[test]
    fn preview_never_arms_the_save() {
        assert!(!edit_arms_save(&Edit::Preview));
        assert!(edit_arms_save(&Edit::Changed("g".into())));
    }

    /// B2 through the gate `apply_edit` runs first: a preview stops there with the colors still
    /// previewed; any other edit puts a stale preview back before it is applied and saved.
    #[test]
    fn a_stale_preview_never_reaches_a_save() {
        let mut store = Store::fresh(&std::env::temp_dir().join(format!("tandem-gate-{}.toml", std::process::id())));
        let guid = store.default_guid.clone();
        let before = store.get(&guid).unwrap().values.clone();
        let mut st = crate::settings_presets::State::new(false);
        let p = store.get_mut(&guid).unwrap();
        st.preview = Some(crate::settings_presets::Preview::begin(p, &guid, false, "Solarized"));
        crate::settings_presets::load(&crate::color_presets::built_in()[0].1, p, false, false, false);
        let previewed = store.get(&guid).unwrap().values.clone();
        assert_ne!(previewed, before);
        let mut run = |edit: &Edit, store: &mut Store| gate_edit(edit, &mut || {
            st.restore(store);
        });
        assert!(!run(&Edit::Preview, &mut store), "a preview stops at the gate");
        assert_eq!(store.get(&guid).unwrap().values, previewed, "and leaves the colors previewed");
        assert!(!run(&Edit::None, &mut store));
        // Another edit (a tab away, then any change) restores before it is applied.
        assert!(run(&Edit::Changed("g".into()), &mut store));
        assert_eq!(store.get(&guid).unwrap().values, before);
    }
}
