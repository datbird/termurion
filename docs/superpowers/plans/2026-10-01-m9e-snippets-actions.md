# Tandem Milestone 9e: Snippets and Actions

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's snippets and actions: snippets (title, value, GUID, tags, escaping) kept in `%APPDATA%\Tandem\snippets.json`, actions (a key action with its parameter, escaping, and apply mode) kept in the `Actions` setting; Edit > Snippets (the disabled "Press Option to Edit Before Sending" item, only snippets holding every tag of the session profile's "Snippets Filter", untagged first, then the `/` tag tree) sends the snippet with its own escaping, or with Alt held opens Advanced Paste on it; Edit > Actions applies an action to the sessions its apply mode names; key action 63 "Send Snippet" works and its editor picks a snippet; Settings > Shortcuts has the Actions and Snippets tabs with iTerm2's editors, search, context menu, drag reorder, and `.it2snippets`/`.it2actions` import and export.

**Architecture:** `snippets.rs` is the pure snippet model: the dictionary codec with iTerm2's version migration and fallback GUID, display titles, the action key, the store file (a present but broken file is never overwritten with an empty list), the tag filter, the menu tree, the search matcher, the import and export codec, and a new snippet's defaults. `user_actions.rs` is the action model: its codec in the `Actions` setting, its display text, and the apply-mode target rule. The app loads the store, builds both Edit submenus like the Profiles menu, sends snippets and applies actions, and the Settings window gains the Shortcuts page.

**Tech Stack:** as milestone 9d (`sha2` is new, for iTerm2's fallback GUID).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` sections 3.0 and 4. Rows: `03-features-and-menus.md` Edit > Snippets, Edit > Actions; `02-global-settings.md` Actions, Snippets, Send Snippet...; `01-profile-settings.md` Snippets Filter.

## Global Constraints

- Everything from milestones 0 to 9d still holds.
- Snippets file: `%APPDATA%\Tandem\snippets.json` holding `{"snippets": [ {title, value, guid, tags, version, escaping}, ... ]}` (iTerm2's dictionary keys). New snippets are version 2.
- Escaping migration: version 0 is Compatibility, 1 is Common, 2 and up reads `escaping`.
- Sending a snippet uses the snippet's escaping, never the binding's; the text is not interpolated.
- Clippings are milestone 9f; the Snippets and Actions toolbelt tools come with the toolbelt; the Snippets Menu and Actions Menu status bar components come with the status bar.
- Where iTerm2 has a bug (the menu tree drop, the editor fall-through, the Actions editor resetting escaping), Tandem does what iTerm2 means, and the ledger rules it.

## Review Focus

1. Snippets with tags `a/b` and `a`: both appear in the menu (iTerm2 drops the second). Test: Task 1 `menu_tree_follows_iterm2`.
2. A snippets file that fails to parse: the menu is empty and the file is left as it was on the next save of an empty list. Test: Task 1 `store_never_clobbers_a_broken_file`.
3. A key binding holding the legacy title string instead of `{"guid":...}`: it still sends. Test: Task 1 `action_keys_match_guid_or_title`.
4. An action with apply mode "all sessions in tab" on a split tab: the text goes to each pane of the tab once. Test: Task 2 `apply_modes_pick_targets`.
5. Search `tag:git -title:old ^ls`: iTerm2's operators. Test: Task 1 `search_follows_iterm2`.

---

### Task 1: The snippet model

**Files:** Create `src/snippets.rs`; add to `src/lib.rs`; `sha2` in `Cargo.toml`.

**Produces:** `Snippet { title, value, guid, tags: Vec<String>, escaping: Escaping }` with `from_json(&serde_json::Value, index) -> Option<Snippet>`, `to_json()`, `display_title()`, `action_key() -> String` (`{"guid":"..."}`), `matches_key(&str) -> bool`; `Store { snippets, broken: bool }` with `from_file_text(Option<&str>) -> Store`, `file_text(&self) -> Option<String>` (None when broken and empty), `find_key(&str) -> Option<&Snippet>`; `filtered(&[Snippet], filter_tags) -> Vec<usize>`; `enum MenuNode { Snippet(usize), Folder(String, Vec<MenuNode>) }`, `menu_tree(&[Snippet], &[usize]) -> (Vec<usize> /*untagged*/, Vec<MenuNode>)`; `search(&[Snippet], query, extra_tags) -> Vec<usize>`; `import(text) -> Result<Vec<Snippet>, String>`, `export(&[&Snippet]) -> String`; `new_snippet(clipboard: Option<&str>) -> Snippet`; `ellipsize(s, limit)`.

- [ ] Failing tests `snippets_decode_like_iterm2`, `action_keys_match_guid_or_title`, `store_never_clobbers_a_broken_file`, `menu_tree_follows_iterm2`, `search_follows_iterm2`, `import_and_export_round_trip`.
- [ ] Implement. Commit `snippet model`.

### Task 2: The action model

**Files:** Create `src/user_actions.rs`; add to `src/lib.rs`.

**Produces:** `UserAction { action: i64, title, parameter, escaping: Escaping, apply_mode: i64 }` with `from_value(&toml::Value)`, `to_value()`, `binding() -> keyaction::Binding`; `load(Option<&toml::Value>) -> Vec<UserAction>`, `store(&[UserAction]) -> toml::Value`; `import(text)`/`export(&[&UserAction])` (JSON array, iTerm2's keys); `APPLY_MODES: [(i64, &str); 6]`; `targets<T: Copy + Eq>(mode, current: T, all, window, tab, broadcast: &[T]) -> Vec<T>`; `description(action, parameter, snippet_title: Option<&str>) -> String` ("Send Snippet “x”", "Send Deleted Snippet (no action)").

- [ ] Failing tests `actions_decode_like_iterm2`, `apply_modes_pick_targets`, `descriptions_follow_iterm2`.
- [ ] Implement. Commit `action model`.

### Task 3: App wiring, Settings > Shortcuts, parity, acceptance

**Files:** Create `src/app/snippets_ui.rs`, `src/settings_shortcuts.rs`; modify `src/app/menu.rs` and `src/app/windows_ui.rs` (the two dynamic submenus), `src/app/actions.rs` (63, apply modes), `src/keyaction.rs` (63 done), `src/settings_keys.rs` (the snippet picker for 63), `src/settings_ui.rs` (Page::Shortcuts), `src/commands.rs`, parity rows, `docs/acceptance/m9e-checklist.md`.

- [ ] Wire. Commit `snippets and actions` and `docs: m9e parity and checklist`.
