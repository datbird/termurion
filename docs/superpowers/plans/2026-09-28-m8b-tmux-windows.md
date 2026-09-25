# Tandem Milestone 8b: tmux Windows as Tabs and Windows

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After the 8a handshake, Tandem opens every tmux window of the attached session as Tandem windows or tabs (iTerm2's `openWindowsInitial`), with tmux's pane layout, each pane's history, alternate screen, cursor, modes, and pending output restored, live `%output` into the panes, typing sent as `send-keys`, and the live notifications (`%layout-change`, `%window-add`, `%window-close`, `%window-renamed`, `%session-changed`, `%window-pane-changed`, `%session-window-changed`, `%sessions-changed`) applied. Edits from Tandem (splits, divider drags, closes, renames) and sizes are 8c.

**Architecture:** Pure models: `tmux_layout.rs` (the layout grammar, coalescing, pane-border-status rows, checksum, conversion to Tandem's binary `Layout` with cell-exact ratios), `tmux_state.rs` (the list-panes state line and the escape sequences that put a `Screen` into that state; capture-pane history and pending output), `tmux_windows.rs` (the `openWindowsOfSize` list, the list-windows TSV with the tab-bug workaround, the hex option decoders, hidden and too-many rules, affinity classes, and the placement decision of research 3.1), and `tmux_keys.rs` (the send-keys encoder with iTerm2's classes, caps, and version rules). The window side: a tmux client pane is a `Pane` with `Io::Tmux` (no ConPTY: reads come from `%output`, writes become `send-keys`), and `app/tmux_ui.rs` grows a per-gateway controller (window ids to tabs, pane ids to panes) that opens tabs through the existing `NewWindow::Tabs` path.

**Tech Stack:** as milestone 8a.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8-tmux-facts.md` sections 2 (steps 4 to 8), 3.1, 3.2, 4.1, 4.2, 7, 9 (8b). Rows: `03-features-and-menus.md` `:417` (tmux -CC), `:419` (window to tab mapping), `:420` (layout sync), `:422` (rename sync, the tmux-to-Tandem half), `:424` (hidden windows, read side); `02-global-settings.md` `:162-167` (OpenTmuxWindowsIn and its phony keys, TmuxDashboardLimit's open rule, UseTmuxProfile read as TmuxUsesDedicatedProfile), `:940` (anonymousTmuxWindowsOpenInCurrentWindow), and the advanced title keys.

## Global Constraints

- Everything from milestones 0 to 8a still holds.
- Notifications stay dropped until the last command of the initial window-opening list is answered (8a's acceptNotifications); output that arrived in between comes from `capture-pane -P -C` (pending output), never from `%output`, so nothing prints twice.
- Commands and formats are iTerm2's, character for character (research 2.5, 2.6, 2.8, 4.1, 4.2).
- Placement follows research 3.1: `OpenTmuxWindowsIn` 0 native windows (default), 1 tabs in a new window, 2 tabs in the attaching window; `anonymousTmuxWindowsOpenInCurrentWindow` 0 New window, 1 Focused window (default), 2 Topmost session window; `TmuxDashboardLimit` 10: more windows than that opens none (the Dashboard itself is 8e; 8a shows the notification only).
- Titles: `tmuxTitlePrefix` (default U+21A3 and a space) before tab and window titles; with `tmuxIncludeClientNameInWindowTitle` (default on) the window title adds ` [<client name>]`.
- A tmux client pane never has a ConPTY; its `Screen` has tmux's pane size exactly (cols x rows from the layout), whatever the pixel rect.
- Typing in a tmux pane becomes `send -lt`, `send -H -t`, or `send -t ... 0x..` commands in one command list with errors tolerated, each command under 1024 bytes (research 4.2); key names (`'C-M-x'`) on tmux 3.2+ for keys that the pane's modifyOtherKeys mode reports.

## Review Focus

1. A layout string with `{` and `[` nested three deep, a same-orientation child that must be hoisted, and pane-border-status top. Expect: the Tandem layout's panes have tmux's exact cell sizes. Test: Task 1 `layouts_convert_cell_exact`.
2. `%output` for a pane that arrived before the initial list's last reply. Expect: dropped by the gateway; the pending-output capture supplies it once. Test: Task 4 `early_output_is_not_doubled`.
3. Typing a mix of letters, `;`, a tab, a newline, and `e` with an acute accent into a tmux pane on tmux 2.1, 2.2, and 3.4. Expect: iTerm2's classes and command splits per version. Test: Task 5 `send_keys_follow_iterm2`.
4. `%layout-change` that adds a pane and removes another in one window. Expect: the new pane opens with its history, the removed one closes, the others keep their screens. Test: Task 4 `layout_changes_add_and_remove_panes`.
5. 11 windows in the session with the default limit 10. Expect: none open and the notification "Too many tmux windows!" / "Use the tmux dashboard to select which to open." Test: Task 3 `too_many_windows_open_none`.

---

### Task 1: Layouts

**Files:** Create `src/tmux_layout.rs`; modify `src/lib.rs`, `src/tmux_probe.rs` (a second run that splits a window twice and sends `list-windows -F "#{window_layout}"`, saved as `tests/fixtures/tmux/wsl-3.4-layout.bin`: the 8a transcript has no layout).

**Interfaces:** `Node { Leaf { w, h, x, y, pane: u32 }, Split { axis: Axis, w, h, x, y, children: Vec<Node> } }`; `parse(&str) -> Option<Node>` (skip the 5-character checksum prefix; nil for 5 characters or less and for unbalanced brackets; a single leaf gets a root split; same-orientation children hoisted); `border_status(node, top: bool, bottom: bool) -> Node`; `checksum(text) -> String` (`{:04x},text`); `panes(&Node) -> Vec<u32>` in DFS order; `to_layout(&Node, ids: &HashMap<u32, PaneId>) -> Layout` (binary splits, ratio by cells so each pane's grid is tmux's); `sizes(&Node) -> Vec<(u32, u16, u16)>`.

- [ ] Failing tests `layouts_parse_like_iterm2` (research 3.2's example, the fixture's layout), `bad_layouts_are_none`, `layouts_convert_cell_exact`, `checksum_matches_tmux` (a checksum from the WSL transcript). Commit `tmux layouts`.

### Task 2: Client panes

**Files:** Create `src/tmux_state.rs`; modify `src/pane.rs` (`Io::Tmux`), `src/screen.rs` or `src/trigger_eval.rs` (apply state, replace history).

**Interfaces:** `PaneState::parse(line) -> Option<PaneState>` for the research 4.1 keys; `PaneState::sequences(&self) -> Vec<u8>` (alternate screen, cursor, scroll region, tab stops, insert, DECCKM, keypad, wrap, mouse modes and SGR format, bracketed paste, modifyOtherKeys); `history_bytes(capture: &[u8]) -> Vec<u8>`; `pending_bytes(capture: &[u8]) -> Vec<u8>` (octal escapes kept only for control codes); `Io::Tmux(TmuxIo { gateway: PaneId, pane: u32, inbox, outbox, ended })` with `try_read`, `write`, no-op `resize`, `exit_code` after detach, no pid; `Pane::tmux(id, gateway, pane, cols, rows)`.

- [ ] Failing tests `pane_state_parses`, `pane_state_restores_modes` (a Screen fed the sequences reports the modes and cursor), `pending_output_keeps_only_controls`, `tmux_io_queues`. Commit `tmux client panes`.

### Task 3: Opening windows

**Files:** Create `src/tmux_windows.rs`; modify `src/lib.rs`, `src/tmux_handshake.rs` (VersionKnown then `openWindowsInitial`), `schema/global.toml` (`OpenTmuxWindowsIn`, `TmuxDashboardLimit`, `TmuxUsesDedicatedProfile`, `anonymousTmuxWindowsOpenInCurrentWindow`, `tmuxTitlePrefix`, `tmuxIncludeClientNameInWindowTitle`).

**Interfaces:** `open_list(session, w, h, status_2_9) -> Vec<String>` (research 2.5); `Window { session, id, name, w, h, layout, flags, active, visible_layout, border_status }` and `parse_windows(tsv, tab_bug) -> Vec<Window>`; `decode_option(prefix, value) -> String` (hex with prefix, plain otherwise); `hidden(&str) -> BTreeSet<u32>`; `affinities(&str) -> Vec<Class>`; `initial(windows, hidden, limit) -> Initial { open: Vec<u32>, notify: Option<(title, text)> }`; `place(window, prefs, affinity, focused_window_has_other_tmux, initial) -> Place { NewWindow, TabIn(window key), TabInGateway }`; `capture_list(pane, max_history, version) -> Vec<String>` (research 2.8).

- [ ] Failing tests `open_list_follows_iterm2`, `windows_parse_with_the_tab_bug`, `options_decode`, `too_many_windows_open_none`, `placement_follows_iterm2`, `capture_list_by_version`. Commit `tmux window model`.

### Task 4: The controller in the window

**Files:** modify `src/app/tmux_ui.rs` (the controller: session, windows map, panes map, pending capture replies; `%output` routing; open tabs and windows; live notifications), `src/app/windows_ui.rs` (a tmux tab's window key), `src/app/mod.rs` (tmux tabs in `relayout`: grids from tmux sizes, no pty resize), title prefix and client name in `src/app/menu.rs` or the title code.

**Interfaces:** `Controller` (pure part in `tmux_windows.rs`): `on_layout(window, layout) -> Change { added: Vec<u32>, removed: Vec<u32> }`, `on_window_close`, `on_session_changed -> close all`; `App::tmux_open_window(gw, Window)`, `App::tmux_layout_changed`.

- [ ] Failing tests `early_output_is_not_doubled` (gateway plus controller: output before the initial list's reply is dropped, the pending capture is fed once), `layout_changes_add_and_remove_panes`, `session_change_closes_everything`. Commit `tmux windows open`.

### Task 5: Keys

**Files:** Create `src/tmux_keys.rs`; modify `src/app/broadcast_ui.rs` (`user_write` to a tmux pane goes through the encoder; mouse reports too; focus reports become `select-pane`), `src/app/keyproto_ui.rs` (key names on 3.2+).

**Interfaces:** `encode(text: &[u8], pane: u32, version: f64) -> Result<Vec<String>, char>` (Err: a supplementary-plane character on 2.2); `key_name(key, mods) -> Option<String>` for research 4.2's modified keys.

- [ ] Failing tests `send_keys_follow_iterm2`, `long_input_splits_under_1024`, `key_names_follow_iterm2`. Commit `tmux keys`.

### Task 6: The tmux profile

**Files:** modify `src/app/tmux_ui.rs`, `src/profile.rs` or the store (`TmuxUsesDedicatedProfile`: the "tmux" profile made on first use from the default profile, as iTerm2's `ProfileModel.tmuxProfile`).

- [ ] Failing test `tmux_profile_is_made_once`. Commit `tmux profile`.

### Task 7: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m8b-checklist.md`. Commit.
