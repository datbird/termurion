# Tandem Milestone 7c: Arrangements

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's window arrangements: Window > Arrangements (Save Window Arrangement, Save Current Window as Arrangement, Restore Window Arrangement and Restore Window Arrangement as Tabs submenus with the default on Shift+Cmd+R, Load Arrangement from File, Change Profile in Arrangement), the save dialog (default name, replace question, Save to File with Contents), the Settings > Arrangements tab (list, rename, delete, Set Default, a preview), Open default window arrangement at startup, and Duplicate Window with its splits and folders.

**Architecture:** `arrangement.rs` holds the data in iTerm2's key names (windows with frames, tabs with a `Root` tree of "Splitter" and "SessionView" nodes with pixel frames, sessions with Columns, Rows, the profile, the working directory, and optional contents) as `toml::Value` tables, and converts between iTerm2's n-ary splitters and Tandem's binary layout through a public `layout::Tree`. `arrangements.rs` is the named store (default rules, unique names). The window code snapshots windows into arrangements and rebuilds windows, tabs, and panes from them; files are iTerm2 plists (`.itermarrangement` / `.iterm2arrangement`) read and written with the `plist` crate.

**Tech Stack:** as milestone 7b (the `plist` crate is already a dependency).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7-iterm2-facts.md` section 4. Rows: `02-global-settings.md` `:521-524` (Arrangements tab), `:981-982`; `03-features-and-menus.md` `:264-269`, `:649-655`, `:43` (Duplicate Window).

## Global Constraints

- Everything from milestones 0 to 7b still holds.
- Storage: globals `Window Arrangements` (name to an array of window tables) and `Default Arrangement Name`; the first saved arrangement becomes the default; deleting the default picks a neighbor.
- The save dialog: "Name for saved window arrangement:", prefilled "Arrangement (<date>)" made unique by adding the time, then seconds; newlines become spaces; an existing name asks "Replace Existing Saved Window Arrangement?" / "There is an existing saved window arrangement with this name. Would you like to replace it with the current arrangement?" (Yes, No).
- A restored session starts its profile's shell with the saved folder as the previous session's folder (the profile's working-directory rule decides), unless the arrangement carries contents, which force the folder. A missing profile uses the default profile and says so once.
- Restore as tabs appends every saved window's tabs to the current window.
- A file with a Mark of the Web (the `Zone.Identifier` stream, Windows' quarantine) asks first: `Open the arrangement "<name>"?` listing the commands it would run, ending "Open it only if you trust where it came from."

## Review Focus

1. An iTerm2 arrangement with a three-way vertical splitter holding a horizontal splitter. Expect: Tandem builds the same panes with the same size shares. Test: Task 1 `n_ary_splitters_become_binary`.
2. Save then restore a window with splits. Expect: the same tree and the same share of each pane. Test: Task 1 `layouts_round_trip`.
3. A saved arrangement naming a profile that was deleted. Expect: the default profile, one notice. Test: Task 4 `missing_profiles_fall_back`.
4. Deleting the default arrangement among three. Expect: a neighbor becomes the default. Test: Task 2 `default_rules`.
5. A plist file that is not an arrangement (a dictionary, garbage). Expect: an error message, nothing opens. Test: Task 5 `bad_files_are_refused`.

---

### Task 1: The arrangement model

**Files:** Create `src/arrangement.rs`; modify `src/layout.rs` (`Tree<T>`, `Layout::tree`, `Layout::from_tree`).

**Interfaces:** `Tree<T> { Leaf(T), Split { axis, ratio, a, b } }`; `node(tree: &Tree<Value>, rect) -> Value` (a "Splitter" with n-ary `Subviews` merged from same-axis splits, or a "SessionView"); `tree_of(node: &Value) -> Result<Tree<Value>, String>` (n-ary to binary, ratios from frames); `session(...)`, `window(...)`, `tab(...)` builders; accessors for the keys Tandem reads.

- [ ] Failing tests `layouts_round_trip`, `n_ary_splitters_become_binary`, `sessions_carry_profile_and_folder`. Commit `Arrangement model`.

### Task 2: The arrangement store

**Files:** Create `src/arrangements.rs`.

**Interfaces:** `Store { names() sorted, get, save(name, windows) (first becomes default), delete(name) (default moves to a neighbor), rename, set_default, default_name }` over the two globals keys; `unique_name(base_date, taken) -> String`; `clean_name(s)`.

- [ ] Failing tests `default_rules`, `unique_names`, `rename_keeps_the_default`. Commit `Arrangement store`.

### Task 3: Saving

**Files:** Create `src/app/arrange_ui.rs`; modify `src/app/menu.rs`, `src/commands.rs`, `src/prompt_ui.rs` (a third button).

**Interfaces:** `App::snapshot_window() -> Value`, `snapshot_all() -> Vec<Value>` (by window number); the save dialog with OK, "Save to File with Contents...", Cancel; the replace question; a file save dialog (rfd) writing an XML plist with contents (each session's visible lines and history as text).

- [ ] Failing test `snapshot_keys_follow_iterm2` (pure builder over sample panes). Commit `Save arrangements`.

### Task 4: Restoring

**Files:** modify `src/app/arrange_ui.rs`, `src/app/windows_ui.rs` (`NewWindow::Arrangement`), `src/app/menu.rs` (the two submenus, rebuilt as arrangements change, the default on Shift+Cmd+R), `src/app/mod.rs` (startup), `src/startup.rs` use.

**Interfaces:** `restore(name, as_tabs)`; `build_tab(tab: &Value) -> Result<Tab, String>` (starts a pane per session: profile by Guid then by Name, else the default profile with a notice; folder rule as in Global Constraints; contents fed as history); window frame, style, title override, and locks from the window table; Duplicate Window = snapshot then restore.

- [ ] Failing tests `missing_profiles_fall_back`, `folders_follow_the_profile_rule`. Commit `Restore arrangements`.

### Task 5: Files and trust

**Files:** modify `src/arrangement.rs` (plist read and write), `src/app/arrange_ui.rs` (Load Arrangement from File, the trust question), `src/app/menu.rs`.

**Interfaces:** `from_plist(bytes) -> Result<Vec<Value>, String>` (an array of windows; iTerm2's files), `to_plist(windows) -> Vec<u8>`; `trust_text(name, windows) -> Option<String>` listing profiles' commands and initial text; `has_mark_of_the_web(path)`.

- [ ] Failing tests `plist_round_trip`, `bad_files_are_refused`, `trust_text_lists_commands`. Commit `Arrangement files`.

### Task 6: Settings > Arrangements and Change Profile in Arrangement

**Files:** Create `src/settings_arrangements.rs`; modify `src/settings_ui.rs` (the tab), `src/app/arrange_ui.rs`.

**Interfaces:** the list (default marked, rename in place), "-" deletes, "Set Default", a preview drawing windows, tabs, and splits as boxes; Change Profile in Arrangement: pick an arrangement and a profile, every session's profile becomes it.

- [ ] Failing test `change_profile_rewrites_every_session`. Commit `Arrangements settings`.

### Task 7: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m7c-checklist.md`. Commit.
