//! Snippets and actions in the app: the snippets file, Edit > Snippets and Edit > Actions,
//! sending a snippet (Alt: Advanced Paste), and the apply modes of key actions.

use super::*;
use crate::keyaction::{self, Binding};
use crate::snippets::{self, MenuEntry, Store};
use crate::user_actions;

const SNIPPET_ID: &str = "snippet-send:";
/// Edit > Actions' item ids, one per action by its place in `Actions`; the status bar's
/// Actions Menu uses them too, so a pick runs the same way.
pub(super) const ACTION_ID: &str = "action-apply:";

pub(super) fn snippets_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Termurion").join("snippets.json")
}

/// The snippets file as it is now (a missing file is an empty list).
pub(super) fn load_snippets() -> Store {
    Store::from_read(std::fs::read_to_string(snippets_path()).map_err(|e| e.kind()))
}

/// A menu that takes items: Edit > Snippets (a submenu) or a status bar menu.
pub(super) trait Parent {
    fn add(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()>;
}

impl Parent for muda::Submenu {
    fn add(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()> {
        self.append(item)
    }
}

impl Parent for muda::Menu {
    fn add(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()> {
        self.append(item)
    }
}

fn append(parent: &dyn Parent, item: &dyn muda::IsMenuItem, errors: &mut Vec<String>) {
    if let Err(e) = parent.add(item) {
        errors.push(e.to_string());
    }
}

/// Builds entries into `parent` up to the end of the current folder.
fn build(parent: &dyn Parent, entries: &[MenuEntry], at: &mut usize, errors: &mut Vec<String>) {
    while *at < entries.len() {
        let e = &entries[*at];
        *at += 1;
        match e {
            MenuEntry::Close => return,
            MenuEntry::Open(name) => {
                let sub = muda::Submenu::new(name.replace('&', "&&"), true);
                build(&sub, entries, at, errors);
                append(parent, &sub, errors);
            }
            MenuEntry::Line => append(parent, &muda::PredefinedMenuItem::separator(), errors),
            MenuEntry::Item(guid, label) => append(parent, &muda::MenuItem::with_id(format!("{SNIPPET_ID}{guid}"), label.replace('&', "&&"), true, None), errors),
        }
    }
}

/// The snippets menu's items: the disabled Option hint, a separator, and the tag tree.
fn fill_snippets(parent: &dyn Parent, entries: &[MenuEntry], errors: &mut Vec<String>) {
    append(parent, &muda::MenuItem::new("Press Option to Edit Before Sending", false, None), errors);
    append(parent, &muda::PredefinedMenuItem::separator(), errors);
    build(parent, entries, &mut 0, errors);
}

impl App {
    /// The focused session's profile "Snippets Filter".
    pub(super) fn snippets_filter(&self) -> Vec<String> {
        let Some(p) = self.focused_id().and_then(|id| self.panes.get(&id)).and_then(|p| self.store.get(&p.profile)) else { return Vec::new() };
        p.get("Snippets Filter").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())).unwrap_or_default()
    }

    /// Edit > Snippets' entries: the snippets the focused session's filter lets through,
    /// as a tag tree.
    fn snippet_entries(&self) -> Vec<MenuEntry> {
        let s = &self.snippets.snippets;
        snippets::menu_entries(s, &snippets::filtered(s, &self.snippets_filter()))
    }

    /// Edit > Snippets' items into `parent`: the disabled Option hint, a separator, and the
    /// tag tree. The status bar's Snippets Menu opens with the same items.
    pub(super) fn append_snippet_items(&self, parent: &dyn Parent, errors: &mut Vec<String>) {
        fill_snippets(parent, &self.snippet_entries(), errors);
    }

    /// Edit > Snippets and Edit > Actions, rebuilt when what they list changes.
    pub(super) fn refresh_snippet_menus(&mut self) {
        let entries = self.snippet_entries();
        let actions: Vec<(String, String)> = user_actions::load(self.globals.get("Actions").as_ref()).iter().enumerate().map(|(i, a)| (format!("{ACTION_ID}{i}"), a.title.clone())).collect();
        let mut errors = Vec::new();
        let Some(bar) = &mut self.menu_bar else { return };
        if let Some(sub) = bar.snippets.as_ref().filter(|_| bar.snippet_items.as_ref() != Some(&entries)) {
            while sub.remove_at(0).is_some() {}
            fill_snippets(sub, &entries, &mut errors);
            bar.snippet_items = Some(entries);
        }
        if let Some(sub) = bar.actions.as_ref().filter(|_| bar.action_items != actions) {
            while sub.remove_at(0).is_some() {}
            for (id, title) in &actions {
                append(sub, &muda::MenuItem::with_id(id.clone(), title.replace('&', "&&"), true, None), &mut errors);
            }
            bar.action_items = actions;
        }
        if let Some(e) = errors.first() {
            self.banners.set("menu", format!("Snippets menu: {e}"));
        }
    }

    /// Sends the snippet `guid` to the focused session, or with `option` (Alt held) opens
    /// Advanced Paste on it (`sendSnippet:`; the menus and Open Quickly share it).
    pub(super) fn send_snippet(&mut self, guid: &str, option: bool, event_loop: &ActiveEventLoop) {
        let Some(s) = self.snippets.snippets.iter().find(|s| s.guid == guid).cloned() else { return };
        let text = keyaction::expand(&s.value, s.escaping);
        match self.focused_id() {
            // iTerm2 puts the escaped text on the pasteboard and opens Advanced Paste on it.
            Some(pane) if option => {
                if let Some(cb) = self.clipboard.as_mut() {
                    if let Err(e) = cb.set_text(text) {
                        return self.banners.set("clipboard", format!("Could not copy: {e}"));
                    }
                }
                self.open_advanced_paste(event_loop, pane, None);
            }
            Some(_) => self.send_to_focused(text.as_bytes()),
            None => {}
        }
    }

    /// A pick from Edit > Snippets or Edit > Actions. False for other menu ids.
    pub(super) fn snippet_or_action_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        if let Some(guid) = id.strip_prefix(SNIPPET_ID) {
            let option = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
            self.send_snippet(guid, option, event_loop);
            return true;
        }
        if let Some(n) = id.strip_prefix(ACTION_ID).and_then(|n| n.parse::<usize>().ok()) {
            if let Some(a) = user_actions::load(self.globals.get("Actions").as_ref()).get(n) {
                let b = a.binding();
                self.run_key_action(&b, None, None, event_loop);
            }
            return true;
        }
        false
    }

    /// What a text-sending key action writes, or None for other actions (and a deleted
    /// snippet).
    pub(super) fn action_text(&self, b: &Binding) -> Option<Vec<u8>> {
        match b.action {
            10 => Some([b"\x1b".as_slice(), b.text.as_bytes()].concat()),
            11 => Some(keyaction::hex_bytes(&b.text)),
            12 => Some(keyaction::expand(&b.text, b.escaping()).into_bytes()),
            38 | 76 => Some(keyaction::expand(&b.text, b.vim_escaping()).into_bytes()),
            // A snippet is sent with its own escaping, never the binding's.
            63 => self.snippets.find_key(&b.text).map(|s| keyaction::expand(&s.value, s.escaping).into_bytes()),
            _ => None,
        }
    }

    /// The sessions an apply mode names, from the focused one.
    fn apply_targets(&self, mode: i64, current: PaneId) -> Vec<PaneId> {
        let window: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let mut all = window.clone();
        for (_, s) in self.windows.parked() {
            all.extend(s.tabs.iter().flat_map(|t| t.layout.panes()));
        }
        let tab = self.tabs.get(self.active).map(|t| t.layout.panes()).unwrap_or_default();
        let broadcast = self.broadcast.domain_panes(&self.broadcast_view());
        user_actions::targets(mode, current, &all, &window, &tab, &broadcast)
    }

    /// A key action with an apply mode other than the current session: it runs once in
    /// each target session. True when it ran.
    pub(super) fn run_applied(&mut self, b: &Binding) -> bool {
        if b.apply_mode == 0 || !user_actions::shows_apply_mode(b.action) || !user_actions::fans_out(b.action) {
            return false;
        }
        // With no session, Invoke Script Function runs once in the app's scope, whatever the
        // apply mode (`+[PTYSession performKeyBindingAction:event:]`, `PS:11799-11813`).
        let Some(current) = self.focused_id() else { return b.action != 60 };
        let text = self.action_text(b);
        for t in self.apply_targets(b.apply_mode, current) {
            if self.panes.get(&t).is_none_or(|p| p.exited.is_some()) {
                continue;
            }
            match b.action {
                35 => self.start_coprocess(t, &b.text, false, crate::coprocess::Entry::KeyBinding),
                36 => self.find_regex_in(t, &b.text),
                37 => self.set_profile_on(t, &b.text),
                40 => self.load_preset_action(t, &b.text),
                41 | 42 => self.paste_special_in(t, b),
                73 => self.copy_mode_commands_in(t, &b.text),
                // Invoke Script Function: once in each target session's scope.
                60 => {
                    let scope = Self::session_or_app(Some(t));
                    self.invoke(&b.text, scope, crate::script_eval::Timeout::Never, true, super::script_ui::Origin::Report(super::script_ui::KEY_BINDING.into()));
                }
                // The text actions: each target gets the text once (its own broadcasting
                // does not repeat it).
                _ => {
                    let Some(bytes) = text.as_ref().filter(|x| !x.is_empty()) else { continue };
                    if let Some(p) = self.panes.get_mut(&t) {
                        p.scroll(Scroll::Bottom);
                    }
                    self.user_write(t, bytes, crate::broadcast::Write::SendTextLocal);
                }
            }
        }
        true
    }

    /// Saves the snippets file after an edit (never an empty list over a broken file).
    pub(super) fn save_snippets(&mut self) {
        let Some(text) = self.snippets.file_text() else { return };
        let path = snippets_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match crate::shell_history::write_whole(&path, &text) {
            Ok(()) => self.snippets.broken = false,
            Err(e) => self.banners.set("snippets", format!("Problem Saving Snippets: There was a problem saving snippets to \u{201c}{}\u{201d}. The error was: {e}", path.display())),
        }
        self.refresh_snippet_menus();
        // The Snippets tool shows them.
        self.tools_changed();
    }
}
