# Tandem Milestone 8h: tmux Pane Moves, Tab Colors, and Window Persistence

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tmux pane can leave its tab (Session > Move Session > Move Session to Window, and Move Session to Tab, which Tandem gains for every session) through tmux's `break-pane`; the profile's tab color colors a tab (Use Tab Color, Tab Color, light and dark), and tmux panes keep theirs in `@tab_colors`; and tmux keeps what iTerm2 keeps about Tandem's windows: `@per_window_settings` (size or full screen), `@per_tab_settings` (the tab's title override), `@origins` (window positions), and `@buried_indexes` (where a hidden window's tab was), each restored on the next attach.

**Architecture:** `tmux_persist.rs` gains the `w_`, `T_`, `o_`, `b_`, and `t_` encoders and decoders, byte for byte as iTerm2 (escape-before-hex where iTerm2 does it), and iTerm2's color `hexString`. The controller reads the five options in the attach list, answers `restore_of(window)` for the window side, saves them (each deduplicated against the last command), and sends `break-pane`. The app moves sessions, draws profile tab colors, saves state when windows move, resize, change full screen, or change tabs, and applies the restore when a tmux window opens.

**Tech Stack:** as milestone 8g.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8f-tmux-facts.md` sections 5, 6.2, 7, 12, 13. Rows: `03-features-and-menus.md` `:423` (per-window and per-tab persistence), `:428` (break pane), `:430` (tab color from tmux), `:429` (pane hotkeys, deferred), the Move Session menu rows; `01-profile-settings.md` `:65-66` (Use Tab Color, Tab Color); `02-global-settings.md` `:938` (disableTmuxWindowPositionRestoration), `:944` (rememberTmuxWindowSizes), `:936` (useBlackFillerColorForTmuxInFullScreen).

## Global Constraints

- Everything from milestones 0 to 8g still holds.
- Window keys in `@per_window_settings` and `@buried_indexes` are the same `pty-<key>` names the affinities use.
- `@origins` holds Tandem's own screen coordinates (Win32 top-left pixels); a restored origin is kept on a visible monitor.
- `size=` is written as `{W, H}` (iTerm2's NSStringFromSize text, integral values) in logical pixels of the outer window; `fs` when full screen.
- Restores follow iTerm2: size only with `rememberTmuxWindowSizes` and variable window sizes; origin unless `disableTmuxWindowPositionRestoration`.
- `@hotkeys` waits for session hotkeys (not built in Tandem); the row says so.

## Review Focus

1. A window with two tmux tabs moved on screen, detach, attach: the window opens where it was. Test: Task 2 `window_state_round_trips`.
2. `@origins` from iTerm2 with a point off every monitor: the window opens on the nearest monitor. Test: Task 4 checklist; the clamp rule in Task 1 `origins_decode_like_iterm2`.
3. Move Session to Window on a tmux pane: `break-pane -P -F "#{window_id}" -s "%P"`, and the new tmux window opens in a new Tandem window. Test: Task 2 `break_pane_places_the_new_window`.
4. `@tab_colors` holding `p3#ffff00000000` (iTerm2's red bug): the "Fix corrupted tab colors?" question. Test: Task 1 `tab_colors_decode_like_iterm2`.
5. The same save twice: one command. Test: Task 2 `window_state_round_trips`.

---

### Task 1: The option codecs

**Files:** Modify `src/tmux_persist.rs`.

**Produces:** `WindowSettings { size: Option<WinSize> }` with `enum WinSize { Size(i64, i64), FullScreen }`; `per_window_value(&[(String, WindowSettings)]) -> String` (plain), `parse_per_window(&str) -> HashMap<String, WindowSettings>`; `per_tab_value(&[(u32, String)])`, `parse_per_tab(&str) -> HashMap<u32, String>`; `origins_value(&[(Vec<u32>, (i32, i32))])`, `parse_origins(&str) -> HashMap<u32, (i32, i32)>`; `buried_value(&[(String, Vec<(u32, usize)>)])`, `parse_buried(&str) -> HashMap<String, Vec<(u32, usize)>>`; `tab_colors_value(&[(u32, Option<(u8, u8, u8)>)])`, `parse_tab_colors(&str) -> (HashMap<u32, Option<(u8, u8, u8)>>, bool /* pure red found */)`; `hex_string((u8, u8, u8)) -> String` (`#rrggbb`); `option_command(session, name, encoded) -> String`.

- [ ] Failing tests: `per_window_settings_round_trip`, `per_tab_settings_round_trip`, `origins_decode_like_iterm2`, `buried_indexes_round_trip`, `tab_colors_decode_like_iterm2`.
- [ ] Implement. Commit `tmux window options codecs`.

### Task 2: The controller

**Files:** Modify `src/tmux_controller.rs`.

**Produces:** the attach list's replies for `@buried_indexes`, `@per_window_settings`, `@per_tab_settings`, `@origins`, `@tab_colors` kept; `restore_of(window) -> Restore { settings: Option<WindowSettings>, origin: Option<(i32, i32)>, tab_title: Option<String>, tab_colors: HashMap<u32, ...> }`; `save_window_state(state: &WindowState, gw)` with `WindowState { groups: Vec<GroupState> }` sending only changed options; `break_pane(pane, sibling: Option<u32>, gw)` (`-s` on 2.1+, `-t` before) whose reply marks the new window intentional (a new Tandem window) or sets its affinity to the sibling; `buried(window, terminal, index)` and `unburied(window) -> Option<(String, usize)>`.

- [ ] Failing tests: `window_state_round_trips`, `break_pane_places_the_new_window`, `buried_windows_remember_their_tab`.
- [ ] Implement. Commit `tmux window state in the controller`.

### Task 3: Profile tab colors

**Files:** Modify `src/app/requests_ui.rs` (`tab_look`: the OSC tab color, else the profile's), a pure `profile_tab_color(profile, dark) -> Option<Rgb>` in `src/profile.rs`.

- [ ] Failing test `profile_tab_colors_follow_the_mode` (Use Tab Color off: None; on: the color; separate light and dark: the mode's variant).
- [ ] Implement. Commit `Profile tab colors`.

### Task 4: App wiring, parity, acceptance

**Files:** `src/app/windows_ui.rs` (Move Session to Tab for every session; Move Session to Window and to Tab on tmux panes send `break_pane`), `src/app/menu.rs`, `src/app/tmux_tabs_ui.rs` (save window state with the affinities; restore size, full screen, origin, tab title, tab colors on open; the red tab color question; buried indexes on hide and on reveal), `src/app/mod.rs` (window moved, resized, and full screen changes save the state of tmux windows), schema Advanced rows `disableTmuxWindowPositionRestoration`, `rememberTmuxWindowSizes` (exists), parity rows, `docs/acceptance/m8h-checklist.md`.

- [ ] Wire. Commit `tmux pane moves and window persistence` and `docs: m8h parity and checklist`.
