# Tandem Milestone 7a: Many Windows

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tandem opens more than one terminal window, as iTerm2 does: window numbers (lowest free, shown in the title), New Window and New Window with Current Profile, Prevent Opening in a Tab and `openProfilesInNewWindow`, switching by number and Next/Previous Window, Move Tab to New Window, Merge All Windows, Duplicate Window, every window style (the 20 `iTermWindowType` ids with iTerm2's frame math), new-window placement (the four `WindowPlacement` policies, saved slots, smart layout, the Screen setting), Lock Size and Lock Layout, Resize Window by one cell, Edit Window Title, and closing one window at a time.

**Architecture:** A window's own state (its surface and renderer, tabs, active tab, number, style, locks, title override, pointer and hover state) lives in a `WinSlot`. `App` keeps the fields it has today for the current window and a map of the other windows' slots; `App::enter(window_id)` swaps the named window's slot into those fields before an event is handled or a frame is drawn. The ~150 places that read `self.tabs`, `self.active`, and `self.gpu` keep working on "the current window". Panes stay app-wide, so moving a tab between windows moves only the `Tab` value. Pure models decide the rules: `winmodel.rs` (numbers, cycling, titles, open-target choice, locks), `frames.rs` (a port of `FrameCanonicalizer` for all styles), and `placement.rs` (the four placement policies, saved slots, smart layout, screens).

**Tech Stack:** as milestone 6; windows share one wgpu instance, adapter, device, and queue, with a surface and renderer per window.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7-iterm2-facts.md` sections 0, 1, 7, 8, and 9 (7a). Inventory rows: `01-profile-settings.md` Window tab (`:14`, `:146-175`); `02-global-settings.md` `:130-134`, `:207` Show window number, `:212` Dim background windows, key actions Next/Previous Window and New Window with Profile, `openProfilesInNewWindow`; `03-features-and-menus.md` `:38-39`, `:43`, `:52`, `:200-202`, `:251-252`, `:270-271`, `:276`, `:294-317`.

## Global Constraints

- Everything from milestones 0 to 6 still holds.
- Window numbers are the lowest free integer from 0, shown as number + 1; the title ends with " - Alt+Win+N" style shortcut text only in the Window menu. Titles with "Show window number" (`WindowNumber`, default on) start "N. ".
- New Window is Cmd+N (default profile); New Window with Current Profile is Opt+Cmd+Shift+N. "Always open in a new window, never in a tab" (`Prevent Opening in a Tab`) turns a new-tab request into a new window.
- Switch by number: the `SwitchWindowModifier` (default Cmd+Opt) plus 1 to 9. Next and Previous Window cycle by number and wrap.
- Window styles use iTerm2's ids and names (0 Normal, 1 Full Screen, 2/9 Top, 5/8 Bottom, 6/10 Left, 7/11 Right, 12 No Title Bar, 15 Maximized, 17 Centered, 19 Centered No Title Bar; 13, 16, 18 map to 0, 15, 17); edge styles use the percent or cell sizes as `FC` does; a negative percent means cells.
- Placement default is 3 ("Remember the position of previously closed windows and use size from profile"); saved state is per profile slot "<guid> <n>"; iTerm2 never cascades.
- Closing a window closes only that window (Cmd+Shift+W, the close box); the last window's close exits Tandem (the full closing rules come in 7b).
- Lock Size and Lock Layout are exclusive; Lock Layout forbids adding, closing, reordering, or moving tabs and panes.

## Review Focus

1. An event for window B while window A is current (a key, a resize, a redraw, a pane wake). Expect: B's state handles it, A's state is untouched, and A still draws right afterwards. Test: Task 1 `slots_swap_cleanly` (model of the swap over two slots).
2. Closing the window that holds the focused pane of a notification or a coprocess. Expect: panes close, their jobs and coprocesses end, other windows keep working. Test: Task 1 `closing_one_window_keeps_the_others` (registry model).
3. Window numbers after closing window 1 of 3 and opening a new one. Expect: the new one takes number 1 (shown "2"). Test: Task 1 `numbers_reuse_the_lowest_free`.
4. An edge style at 50 percent on a monitor whose work area starts at a negative x. Expect: the frame sits inside that monitor's work area. Test: Task 4 `edge_frames_follow_the_work_area`.
5. Placement with a saved slot on a monitor that is gone. Expect: the window lands on a present monitor (`restoreWindowsWithinScreens`). Test: Task 5 `saved_frames_stay_on_screen`.

---

### Task 1: Window slots and the registry

**Files:** Create `src/winmodel.rs`; modify `src/app/mod.rs` (`WinSlot`, `enter`, event routing by `WindowId`, `resumed`, redraw of every window), `src/winkeys.rs` (a set of Tandem window handles), `src/app/menu.rs` (a menu bar per window), `src/app/close_ui.rs` (the real window number).

**Interfaces:** `Registry { numbers: BTreeMap<WindowId-like u64, usize> }` with `allocate() -> usize` (lowest free), `release(n)`, `order()`, `next(current)` and `previous(current)` (by number, wrapping); `title(base, number, show_number) -> String`; `App::enter(id)`, `App::open_window(guid, how) -> Result<WindowId, String>`, `App::close_window(id)`; `winkeys::add_window(hwnd)`, `remove_window(hwnd)`.

- [ ] Failing tests: `numbers_reuse_the_lowest_free`, `next_and_previous_wrap`, `titles_carry_the_number`, `slots_swap_cleanly`, `closing_one_window_keeps_the_others`. Commit `Open many windows`.

### Task 2: New window commands

**Files:** modify `src/winmodel.rs` (`open_target`), `src/app/menu.rs`, `src/app/actions.rs` (New Window with Profile), `src/commands.rs` (move newWindow out of PARTIAL), `docs/parity/03-features-and-menus.md` shortcut texts.

**Interfaces:** `open_target(request: Request { NewTab, NewWindow, ProfileMenu { option } }, prevent_tab: bool, profiles_in_new_window: bool) -> Target { Tab, Window }`.

- [ ] Failing test `open_target_follows_iterm2`. Commit `New window commands`.

### Task 3: Switching and moving

**Files:** modify `src/app/mod.rs`, `src/app/menu.rs`, `src/app/actions.rs`, `src/winkeys.rs` (switch-by-number chords).

**Interfaces:** switch by number with the `SwitchWindowModifier`; key actions Next Window (1) and Previous Window (3); Move Tab to New Window; Merge All Windows (every other window's tabs move here, in number order); Duplicate Window (the same tabs and splits with the same profiles, new shells in the same folders); Move Session to Window.

- [ ] Failing tests `switch_chord_targets_a_number`, `merge_order_follows_numbers`. Commit `Switch and move between windows`.

### Task 4: Window styles

**Files:** Create `src/frames.rs`; modify `src/app/menu.rs` (the full Window Style menu), `src/app/mod.rs` (apply a style: decorations, maximize, borderless full screen, frames), `schema/profile.toml` (the style list and the units rule).

**Interfaces:** `frame(style, percent: (f64, f64), cells: (u16, u16), cell_px: (f32, f32), chrome: (f32, f32), work: Rect, screen: Rect) -> Frame { rect, decorations, maximized, fullscreen }`; `unthemed(id)`; `store_id(style, width_units_percent, height_units_percent)`.

- [ ] Failing tests `edge_frames_follow_the_work_area`, `percent_and_cells`, `centered_and_maximized`, `theme_ids_map_back`. Commit `Window styles`.

### Task 5: Placement

**Files:** Create `src/placement.rs`; modify `src/app/mod.rs` (save the frame on close into globals, place on open), `schema/global.toml` (`WindowPlacement`, `AdjustWindowForFontSizeChange`).

**Interfaces:** `Slots` (per profile, lowest free slot number, frames with monitor names); `place(policy, profile_guid, slots, monitors, others: &[Rect], preferred: Size, screen_pref) -> Rect`; `smart(area, size, others) -> Point` (50-pixel steps, overlap plus centre distance); `clamp_to_monitors(rect, monitors)`.

- [ ] Failing tests `policies_follow_iterm2`, `smart_layout_avoids_overlap`, `saved_frames_stay_on_screen`, `screen_preference_moves_the_frame`. Commit `Place new windows`.

### Task 6: Window settings and menus

**Files:** modify `src/winmodel.rs` (locks), `src/app/menu.rs`, `src/app/mod.rs`, `src/app/requests_ui.rs` (titles), `src/settings_ui.rs` if a window title prompt is needed.

**Interfaces:** `Locks::toggle_size`, `toggle_layout` (exclusive), `forbids(op: LayoutOp) -> bool`; Resize Window Decrease/Increase Height and Width by one cell (Opt+Cmd+Up/Down for height); Edit Window Title and Edit Tab Title (a one-line prompt; an empty title clears the override); Hide After Opening (minimize); Lock Window Size Automatically; Show window number; Dim background windows (the unfocused window's panes dim like inactive panes).

- [ ] Failing tests `locks_are_exclusive`, `lock_layout_forbids_moves`. Commit `Window settings`.

### Task 7: Parity and acceptance

- [ ] Update the rows 7a closes, recount, write `docs/acceptance/m7a-checklist.md`. Commit.
