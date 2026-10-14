# Tandem Milestone 13i: Pane navigation, dividers, dimming, locks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope (read first):** research `.superpowers/research/m13-facts.md` sections 2 (the `02:1079` default), 5 (the 13i list) and Appendix A rows `01:69`, `01:70`, `01:259`, `02:247`, `02:248`, `02:550`, `02:732`, `02:879`, `02:927`, `02:928`, `02:1079`, `03:293`; rulings `.superpowers/research/m13-rulings.md` (R2, R3, and the 13g-i rulings: inactive panes dim toward middle gray, Next/Previous Pane and pane numbers in on-screen reading order, default divider width 1 point scaled, Lock Split Pane Width applies on window resize only, 13i after 13h, and the eight-pane ordinal 9 that 13h produces); the review `.superpowers/research/m13ghi-plan-review.md` (13i approved; its minors are applied here); the 13h plan `docs/superpowers/plans/2026-10-15-m13h-pane-title-bars.md` (built first; this plan fills its lock hooks); and the Resume notes below, and nothing else: the order Next Pane, Previous Pane, pane number shortcuts, pane ordinals and swap-with-next use (iTerm2's geometric reading order, or the time of last use with NavigatePanesInReadingOrder off); directional pane navigation choosing the most recently active adjacent pane, with WrapFocus; focus of a new split under focus follows mouse; splits that carry the session's own (divorced) profile; divider width and color; the dimming model (Dim inactive split panes, Dimming amount, Dim background windows scaling, Dimming affects only text); the active pane border; Lock Pane (Default Pane Locked, the context menu's lock items, the title bar's lock mark, close confirmation, move refusal, arrangements); Lock Split Pane Width. Title bars and moving panes are 13h; tab styles (and so the divider's theme color) are 13e.

**Goal:** a person moves between split panes exactly as in iTerm2 (reading order or last use, most recent neighbor, wrapping at the edge), sees dividers, dimming and the active pane border as their settings say, splits keep a session's local profile changes, and locked panes resist closing, moving and window resizes as iTerm2's do.

**Architecture:** pure models first, each a plain `pub mod` tested on the Windows test machine. `pane_order.rs` is the pane order model: reading order from pane rectangles, last-use order from activity stamps, stepping through an order. `layout.rs` gains iTerm2's adjacency (projection, nothing between) and wraparound, the Row-parent width pin, and a stack test. `dim.rs` is the dimming model: which panes dim, how much, and the color math for Dim Only Text. `session_profile.rs` gains `inherit`. `close_rules.rs` gains the locked subjects and reason. The app side is `src/app/pane_nav_ui.rs` (ordered panes, activity, directional picks, the one set of callers every menu, key, pointer and API path shares), `src/app/pane_lock_ui.rs` (Lock Pane, Lock Split Pane Width) and changes in `mod.rs` (split focus and profile, the gap, the dim quad, the border). Per-pane state (activity stamp, lock, pinned width) lives on the `Pane`; the app-wide `pane_seen` map is keyed by tab id; nothing new is per window, so nothing goes in `swap_live`.

**Tech Stack:** as milestones 11a to 13h. No new crates and no new features.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m13-facts.md` and the Resume notes below (the source wins where they differ; see Corrections). Rows: `01-profile-settings.md` `01:69`, `01:70`, `01:259`; `02-global-settings.md` `02:247`, `02:248`, `02:550`, `02:732`, `02:879`, `02:927`, `02:928`, `02:1079`; `03-features-and-menus.md` `03:293`.

## Global Constraints

- Everything from milestones 0 to 13h still holds. Read `.superpowers/sdd-templates/implementer-rules.md` first; its invariants bind every task (tmux panes keep tmux's sizes, per-pane state on the Pane, divorced profiles through `Store::get` and `shared_guid`). Precedents: `docs/superpowers/rulings/m11c-rulings.txt`, `m13h-rulings.txt` once written.
- Build order (13g-i rulings): 13a, 13e and 13h land before this plan (13i after 13h). 13y2 (Pass B's gaps sweep) consumes this plan's replacement for `Highlights.undimmed`: `scene::Highlights { .., pub dim: crate::dim::Dim }` with `dim::Dim { amount, only_text }`, `Dim::NONE` and `Dim::LEGACY` (Task 7, R13xy-12); do not rename them. From 13e (`docs/superpowers/plans/2026-10-15-m13e-tab-themes.md` Tasks 4 and 5): `scene::dividers(frame, dividers, color)`, `App::divider_color(&self) -> Rgb` (the theme's divider color), `tab_theme::{toward_gray, perceived, dimming, DIMMING_AMOUNT}` and `App::tab_palette`; this plan adds the custom divider color inside 13e's `divider_color` and replaces `DIMMING_AMOUNT` with the `SplitPaneDimmingAmount` key. Where one of them landed under another name, use the landed name and never add a parallel path.
- 13h is built first. This plan uses 13h's `hexcolor::parse`, `pane_title::{ordinals, Parts}` (`ordinals(8)` is `[1, 2, 3, 4, 5, 6, 7, 9]`: a tab with exactly eight panes gives the eighth pane ordinal 9, `PTYTab.m:680-691`, 13g-i review ruling; the title bar numbers, `pane_by_number` and the Status tool all take this function's answer), `App::{pane_move_refused, show_context_menu_on}`, `pane_title_ui::TitleAction::ToggleLock`, `CtxItem`, and replaces 13h's placeholders: the `Hit::Lock => {}` arm, `Parts { locked: false }`, and the Lock Layout-only `pane_move_refused`.
- 13a adds the Advanced pane's text-field controls for string, int and float keys (facts section 3); until 13a is built, the Advanced keys this plan adds are stored and read but show no control. This plan adds only the Appearance float slider (Task 1).
- Ruling R3 binds three default fixes here, each its own step with a test and a rulings line: splits use the session's divorced profile (`02:1079`, Task 5); Next Pane, Previous Pane and pane numbers follow iTerm2's geometric reading order, not Tandem's tree order (Task 2); inactive panes dim toward middle gray as iTerm2's color map does, not toward the pane's background (Task 7).
- New Advanced keys are stored under iTerm2's user-defaults key, the DEFINE name with its first letter capitalized (`stringByCapitalizingFirstLetter`, `iTermAdvancedSettingsModel.m:105,112`; 13a ruling and R13xy-2): `NavigatePanesInReadingOrder`, `WrapFocus`, `FocusNewSplitPaneWithFocusFollowsMouse`, `SplitPaneColor`, `SplitPaneDividerWidth`, `UseDivorcedProfileToSplit`. None of them exists in `schema/global.toml` yet, so none is an existing lower-case key that stays; plist import matches either spelling (13a).
- Ruling R2 does not apply: every key here is read.
- User text is iTerm2's verbatim with "iTerm2" replaced by "Tandem" and Mac key words by Windows ones (Resume notes "Texts"). Characters outside ASCII are `\u{...}` escapes in code and `\uXXXX` in TOML. Pure code is a plain `pub mod`; Win32 code is `#[cfg(windows)]`. Tests run on the Windows test machine with `scripts/xt test --lib <filter>`. Do not launch the GUI.

## Review Focus

1. Pane order: nested splits where tree order and reading order differ, a side-by-side root (columns first), last-use order with NavigatePanesInReadingOrder off, one pane, a zoomed tab, pane number 9 as the last pane, stepping that does not reorder last use. Test: Task 2 `reading_order_follows_iterm2`, `activity_order_follows_iterm2`; Task 3 `ordered_panes_drive_every_path`.
2. Directional navigation: several adjacent panes (the most recently active wins), a pane behind a nearer one is not adjacent, the 1-point overlap margin, wrapping to the far edge with WrapFocus on and stopping with it off, swaps by direction using the same pick. Test: Task 4 `adjacency_follows_iterm2`, `wraparound_follows_iterm2`, `direction_pick_follows_iterm2`.
3. Splits: the new pane gets focus unless focus follows mouse is on and the Advanced key is off; a split of a pane that is not the active one (the API) leaves focus alone; a divorced session's split gets its own copy of the local changes, a plain session's split gets the saved profile, the key off gives the saved profile. Test: Task 5 `split_focus_follows_iterm2`, `split_profile_follows_iterm2`, `inherit_keeps_the_overrides`.
4. Dimming: amounts for inactive panes, background windows and both (1.5 times, capped at 0.9), broadcast exceptions, Dim inactive split panes off, the gray target, Dim Only Text leaving the default background alone. Test: Task 7 `dim_amounts_follow_iterm2`, `dim_colors_follow_iterm2`, `only_text_follows_iterm2`.
5. Locks: a locked pane asks before closing with iTerm2's locked texts, refuses Move Session to Split Pane, drags and Move Session to Window or Tab, shows its lock mark, survives an arrangement, starts locked from Default Pane Locked; Lock Split Pane Width is offered only for a non-tmux pane in a side-by-side split and holds that pane's width through window resizes. Test: Task 9 `lock_texts_follow_iterm2`, `lock_items_follow_iterm2`, `lock_refuses_moves`; Task 10 `width_pins_follow_iterm2`.

---

### Task 1: The settings rows

**Files:** Modify `schema/global.toml` (the rows of Resume notes "Settings rows"; `DimBackgroundWindows` `:500` moves from section "Windows" to "Dimming"), `src/settings_keys.rs` (`global_control` `:581` gains a `Kind::Float` arm drawn as a slider when the setting has both `min` and `max`; `row_enabled` `:731`: `DimOnlyText` and `SplitPaneDimmingAmount` follow `DimInactiveSplitPanes || DimBackgroundWindows`).

**Interfaces:**
- Consumes: `schema::{Setting, Kind}` (`src/schema.rs:10-50`: `min`, `max`); `Globals::{number, set}` (`src/globals.rs:101-113`).
- Produces: every key of Resume notes "Settings rows" readable through `globals.bool` or `globals.number`; the float slider (`egui::Slider::new(&mut v, min..=max)`, stored as a TOML float).

- [ ] **Step 1: failing tests** in `settings_keys.rs`:
  - `navigation_rows_follow_iterm2`: `NavigatePanesInReadingOrder` (Advanced, "Tabs", bool, true), `WrapFocus` (Advanced, "General", bool, true), `FocusNewSplitPaneWithFocusFollowsMouse` (Advanced, "Session", bool, true), `SplitPaneColor` (Advanced, "Windows", string, ""), `SplitPaneDividerWidth` (Advanced, "Windows", float, 0.0), `UseDivorcedProfileToSplit` (Advanced, "Experimental Features", bool, true), each with Resume notes' label.
  - `dimming_rows_follow_iterm2`: `DimInactiveSplitPanes` (Appearance, "Dimming", bool, true, label "Dim inactive split panes"), `DimOnlyText` (Appearance, "Dimming", bool, false, label "Dimming affects only text, not background."), `SplitPaneDimmingAmount` (Appearance, "Dimming", float, 0.4, min 0.01, max 0.6, label "Dimming amount:"), `DimBackgroundWindows` now in section "Dimming"; `row_enabled(g, "SplitPaneDimmingAmount")` is false with both dim switches off and true with either on.
- [ ] **Step 2:** run `scripts/xt test --lib settings_keys`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Settings rows". The slider shows two decimals.
- [ ] **Step 4:** run the filter, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: navigation, divider and dimming settings`.

### Task 2: The pane order model

**Files:** Create `src/pane_order.rs` (pure); add it to `src/lib.rs`.

**Interfaces:**
- Consumes: `crate::layout::{Rect, PaneId}`.
- Produces: `pub fn reading_order(rects: &[(PaneId, Rect)], side_by_side_root: bool) -> Vec<PaneId>` (`TAB:1052-1080`: a stacked root, or one pane, sorts top to bottom then left to right; a side-by-side root sorts left to right then top to bottom; coordinates within 1 of each other count as equal; ties keep input order); `pub fn activity_order(panes: &[PaneId], activity: &dyn Fn(PaneId) -> u64) -> Vec<PaneId>` (ascending stamp, ties in input order); `pub fn step(order: &[PaneId], from: PaneId, offset: isize) -> Option<PaneId>` (`TAB:1124-1143`, wrapping; None when `from` is not in `order`); `pub fn by_number(order: &[PaneId], digit: u8) -> Option<PaneId>` (`iTermApplication.m:464-481`: 9 is the last, 1 to 8 by position, else None).

- [ ] **Step 1: failing tests** in `pane_order.rs`:
  - `reading_order_follows_iterm2` (Review Focus 1): for `Column[Row[Column[1, 2], 3], 4]` in a 200 x 100 area at ratio 0.5 (rects 1 `(0, 0, 100, 25)`, 2 `(0, 25, 100, 25)`, 3 `(100, 0, 100, 50)`, 4 `(0, 50, 200, 50)`), `reading_order(.., false)` is `[1, 3, 2, 4]` (tree order would be `[1, 2, 3, 4]`); `Row[Column[1, 2], 3]` (1 `(0, 0, 100, 50)`, 2 `(0, 50, 100, 50)`, 3 `(100, 0, 100, 100)`) with `true` is `[1, 2, 3]` and with `false` would be `[1, 3, 2]`; rects whose y differ by 0.5 sort by x.
  - `activity_order_follows_iterm2`: panes `[1, 2, 3]` with stamps 1:4, 2:9, 3:2 give `[3, 1, 2]`; `step(&[3, 1, 2], 2, 1)` is 3; `step(.., 2, -1)` is 1; `step(&[5], 5, 1)` is 5; `step(.., 9, 1)` is None; `by_number(&[1, 2, 3], 9)` is 3, `by_number(.., 2)` is 2, `by_number(.., 4)` is None, `by_number(.., 0)` None.
- [ ] **Step 2:** run `scripts/xt test --lib pane_order`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Pane order".
- [ ] **Step 4:** run the filter and the full `--lib`; all pass.
- [ ] **Step 5:** commit `panes: the pane order model`.

### Task 3: Ordered panes and last use in the app

**Files:** Create `src/app/pane_nav_ui.rs` (declare in `src/app/mod.rs`). Modify `src/pane.rs` (`pub activity: u64` on `Pane`, 0 at start), `src/app/mod.rs` (app-wide `activity_seq: u64`, `pane_seen: HashMap<u64, PaneId>`; `about_to_wait` `:2839` calls `sync_pane_activity`; `Route::SelectPane` near `:2077` uses `pane_by_number`), `src/app/actions.rs` (`step_pane` `:120` becomes `pub(super)` so `menu.rs` can call it, and it and the swap arm `67 | 68` `:365` use `pane_in_order`), `src/app/menu.rs` (the `"nextPane:" | "previousPane:"` arm `:718` calls `step_pane`), `src/app/pane_title_ui.rs` (13h's ordinals come from `ordered_panes`), `src/app/toolbelt_more_ui.rs` (`status_rows` `:639` takes its pane ordinal from `pane_title::ordinals` over `ordered_panes`, `ToolStatus.swift:1000`).

**Interfaces:**
- Consumes: Task 2's `pane_order::*`; Task 1's `NavigatePanesInReadingOrder`; 13h's `pane_title::ordinals`; `Layout::{rects, tree}`; `App::content_area_for`.
- Produces in `pane_nav_ui`: `impl App { pub(super) fn ordered_panes(&self, tab: &Tab) -> Vec<PaneId>; pub(super) fn pane_in_order(&self, tab: &Tab, offset: isize) -> Option<PaneId>; pub(super) fn pane_by_number(&self, tab: &Tab, digit: u8) -> Option<PaneId>; pub(super) fn sync_pane_activity(&mut self); pub(super) fn focus_quietly(&mut self, tab: usize, pane: PaneId) }` (`focus_quietly` sets the tab's focus, clears `zoomed` as today, and records `pane_seen` so no stamp moves: iTerm2's `updateActivityCounter:NO`).

- [ ] **Step 1: failing tests** in `pane_nav_ui.rs`, on the pure `pub(super) fn activity_step(seen: Option<PaneId>, focused: PaneId, seq: u64) -> Option<[(PaneId, u64); 2]>` that `sync_pane_activity` applies:
  - `ordered_panes_drive_every_path` (Review Focus 1): `activity_step(Some(1), 2, 7)` is `Some([(1, 8), (2, 9)])` (the old pane and the new one both take fresh stamps, `TAB:907-914`); `activity_step(Some(2), 2, 7)` is None; `activity_step(None, 2, 7)` is `Some([(2, 8), (2, 9)])` collapsed by the caller to one stamp of 9. The pure `pub(super) fn number_index(digit: u8, panes: usize) -> Option<usize>` that `pane_by_number` uses follows the ordinals: with 8 panes digit 8 and digit 9 both give index 7 (the eighth pane shows 9 in its title bar, `iTermApplication.m:464-481`: 9 is the last pane), digit 1 gives 0; with 10 panes digit 9 gives index 9 and digit 8 index 7; with 3 panes digit 5 gives None and digit 9 gives index 2. The pure `pub(super) fn order_source(reading: bool) -> &'static str` names "reading" or "activity" for the rulings test; with the key on the order is `reading_order` over `layout.rects(content_area_for(tab), gap)` with `side_by_side_root` true when the root split is `Axis::Row`, with it off `activity_order` over `layout.panes()`.
- [ ] **Step 2:** run `scripts/xt test --lib pane_nav_ui`; expect failures.
- [ ] **Step 3:** implement. `sync_pane_activity` walks the current window's tabs and applies `activity_step` for any tab whose focus differs from `pane_seen` (every click, directional pick, new split, API select stamps, as `setActiveSession:` does); `step_pane`, the pane number route and swap-with-next move focus through `focus_quietly` (`TAB:1146-1170`, `iTermApplication.m:467`). Replace the menu's own Next Pane code with `step_pane`, so the menu and the key action agree. A zoomed tab steps through the full order and shows the new pane zoomed off (Tandem's existing `zoomed = None`; iTerm2 swaps the maximized view, Decision). `pane_seen` entries for closed tabs are dropped in `close_pane`.
- [ ] **Step 4:** run the filter, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: reading order and last use`.

### Task 4: Directional navigation and WrapFocus

**Files:** Modify `src/layout.rs` (`adjacent`, `wraparound`; `neighbor` `:362` is removed once its callers move), `src/app/pane_nav_ui.rs` (`pane_in_direction`), `src/app/actions.rs` (`select_pane_dir` `:110` becomes `pub(super)` for `menu.rs`, and it and the swap arm `53..=56` `:359` use `pane_in_direction`), `src/app/menu.rs` (the `selectPane*` arm `:701-715`), `src/app/builtin_ui.rs` (`select_pane_in_direction` `:445-455`).

**Interfaces:**
- Consumes: `Layout::rects`, `Dir`, `Rect`; Task 3's `Pane::activity`; Task 1's `WrapFocus`.
- Produces in `layout`: `pub fn adjacent(&self, from: PaneId, dir: Dir, area: Rect, gap: f32, margin: f32) -> Vec<PaneId>`; `pub fn wraparound(&self, from: PaneId, dir: Dir, area: Rect, gap: f32, margin: f32) -> Vec<PaneId>`; `pub fn most_recent(candidates: &[PaneId], activity: &dyn Fn(PaneId) -> u64) -> Option<PaneId>` (highest stamp; ties keep the first).
- Produces in `pane_nav_ui`: `pub(super) fn pane_in_direction(&self, tab: &Tab, dir: Dir) -> Option<PaneId>` (`most_recent(adjacent)`, else with WrapFocus `most_recent(wraparound)`, else None; `margin` is 1 point times the scale).

- [ ] **Step 1: failing tests** in `layout.rs`:
  - `adjacency_follows_iterm2` (Review Focus 2, `TAB:1454-1531`): `Row[1, Column[2, 3]]` in 200 x 100, gap 0, margin 1: `adjacent(1, Right)` is `[2, 3]`; `adjacent(2, Left)` is `[1]`; `adjacent(2, Down)` is `[3]`; `adjacent(3, Left)` is `[1]`; for `Row[1, Row[2, 3]]` (three columns), `adjacent(1, Right)` is `[2]` (2 sits between 1 and 3); a pane that overlaps by less than the margin (a 0.5-pixel sliver) is not in the projection.
  - `wraparound_follows_iterm2` (`TAB:1543-1567`): `wraparound(2, Right)` in `Row[1, Column[2, 3]]` is `[1]`; `wraparound(3, Down)` is `[2]` (1 does not overlap 3's columns by the margin); `wraparound(1, Left)` in three columns is `[3]`; with one pane every list is empty.
  - `direction_pick_follows_iterm2`: `most_recent(&[2, 3], stamps 2:5, 3:9)` is 3; `most_recent(&[], ..)` is None; the pure `pub(super) fn pick_direction(adjacent: &[PaneId], wrap: &[PaneId], wrap_focus: bool, activity: &dyn Fn(PaneId) -> u64) -> Option<PaneId>` in `pane_nav_ui.rs` returns the adjacent pick when there is one, the wrap pick only with `wrap_focus`, else None.
- [ ] **Step 2:** run `scripts/xt test --lib layout` and `pane_nav_ui`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Directional navigation": swap x and y for Up and Down, mirror for Left and Up (`after` false), exactly as `TAB:1454-1531`. Every caller of `neighbor` (select pane by direction from the menu, key actions 18 to 21, pointer actions, the swap-by-direction key actions 53 to 56 (`PT:12039-12065`), the builtin `select_pane_in_direction`) calls `pane_in_direction`; the directional selects stamp activity (they go through `setActiveSession:`, so they do not use `focus_quietly`).
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: most recent neighbor and WrapFocus`.

### Task 5: Split focus and the divorced profile

**Files:** Modify `src/session_profile.rs` (`inherit`), `src/app/pane_nav_ui.rs` (`split_guid`), `src/app/mod.rs` (`split` `:1601`: the new pane's focus), `src/app/api_requests.rs` (the API split near `:632`: the same focus rule), `src/app/menu.rs` (`"splitVertically:"`, `"splitHorizontally:"` and the other split-with-current-profile arms near `:515` use `split_guid` instead of `focused_guid` `:369`).

**Interfaces:**
- Consumes: Task 1's `FocusNewSplitPaneWithFocusFollowsMouse`, `UseDivorcedProfileToSplit`; `FocusFollowsMouse` (`schema/global.toml:226`); `Store::{sessions, shared_guid}`; `session_profile::SessionProfile`.
- Produces in `session_profile`: `pub fn inherit(from: &SessionProfile, new_guid: String) -> SessionProfile` (the same values, `Original Guid` and `overridden`, with `new_guid` as its Guid: iTerm2's `inheritDivorceFrom:`, `PS:8497-8511`).
- Produces in `pane_nav_ui`: `pub(super) fn focus_new_split(focus_follows_mouse: bool, key: bool, target_is_active: bool) -> bool` (pure, `PT:11365-11370`); `#[derive(Debug, PartialEq, Eq)] pub(super) enum SplitFrom { Shared(String), Inherit(String) }`; `pub(super) fn split_from(use_divorced: bool, pane_guid: &str, is_session_copy: bool, shared_guid: &str) -> SplitFrom` (pure, `PS:2915-2945`); `impl App { pub(super) fn split_guid(&mut self) -> Option<String> }` (for `Inherit`, pushes `inherit` of the focused pane's copy under a fresh upper-case UUID and returns it).

- [ ] **Step 1: failing tests:**
  - `session_profile.rs` `inherit_keeps_the_overrides` (Review Focus 3): a copy of "P" with "Font" overridden, inherited as "NEW", has Guid "NEW", `original_guid()` "P", `overridden` `{"Font"}` and the same "Font" value; merging a changed shared profile afterwards keeps the override.
  - `pane_nav_ui.rs` `split_focus_follows_iterm2`: `focus_new_split(false, true, true)` true; `(true, true, true)` true; `(true, false, true)` false; `(false, false, true)` true; `(false, true, false)` false (a split of a pane that is not the active one never takes focus).
  - `split_profile_follows_iterm2` (ruling R3, `02:1079`): `split_from(true, "C", true, "P")` is `Inherit("C")`; `split_from(true, "P", false, "P")` is `Shared("P")`; `split_from(false, "C", true, "P")` is `Shared("P")` (today's behavior only with the key off).
- [ ] **Step 2:** run `scripts/xt test --lib session_profile` and `pane_nav_ui`; expect failures.
- [ ] **Step 3:** implement. `split` sets `tab.focused = id` only when `focus_new_split(FocusFollowsMouse, FocusNewSplitPaneWithFocusFollowsMouse, target == tab.focused)`, and the API split does the same with its target; tmux splits are unchanged (tmux chooses). The menu's split-with-current-profile arms call `split_guid()`; new tabs, new windows and Edit Session keep `focused_guid` (iTerm2 uses `profileForSplit` only for splits). A session copy made for a split is dropped with its pane like any copy (`forget_session_profile`).
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: split focus and the divorced profile`.

### Task 6: Divider width and color

**Files:** Modify `src/app/mod.rs` (`GAP` `:45` becomes `fn gap(&self) -> f32`; every use in `src/app/mod.rs`, `menu.rs`, `statusbar_ui.rs`, `clippings_ui.rs`, `api_requests.rs` calls it; 13e's `App::divider_color` answers a valid `SplitPaneColor` first), `src/app/pane_nav_ui.rs` (`divider_px`, `divider_rgb`).

**Interfaces:**
- Consumes: Task 1's `SplitPaneDividerWidth`, `SplitPaneColor`; 13h's `hexcolor::parse`; 13e's `App::divider_color`, `scene::dividers`; `App::scale`.
- Produces: `pub(super) fn divider_px(width_pt: f64, scale: f32) -> f32` (pure: a width above 0 capped at 5 points, else 1 point, times the scale, rounded, at least 1 pixel); `pub(super) fn divider_rgb(custom: &str, theme: Rgb) -> Rgb` (pure); `impl App { fn gap(&self) -> f32 }`.

- [ ] **Step 1: failing tests:**
  - `pane_nav_ui.rs` `divider_follows_iterm2` (`PTYSplitView.m:102-119`): `divider_px(0.0, 1.0)` is 1.0; `(0.0, 1.5)` is 2.0; `(3.0, 1.0)` is 3.0; `(9.0, 1.0)` is 5.0; `(-2.0, 1.0)` is 1.0; `(2.5, 2.0)` is 5.0; `divider_rgb("#ff0000", rgb(170, 170, 170))` is `rgb(255, 0, 0)`; `divider_rgb("", t)`, `divider_rgb("ff0000", t)` and `divider_rgb("#zzz", t)` are the theme color `t` (`SPV:110-119`: only a valid `#` color overrides the window's decoration control color, which 13e computes).
- [ ] **Step 2:** run `scripts/xt test --lib pane_nav_ui`; expect failures.
- [ ] **Step 3:** implement. 13e's `App::divider_color` returns `divider_rgb(SplitPaneColor, <its theme color>)`. The divider grab zone (`src/app/mod.rs:2193`, 3 pixels times the scale) is measured from the new divider rect. tmux tabs use the same gap (tmux's dividers are a cell, but Tandem already draws them with its gap; unchanged). A Settings change of either key relays out every window (the room changes).
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: divider width and color`.

### Task 7: The dimming model

**Files:** Create `src/dim.rs` (pure); add it to `src/lib.rs`. Modify `src/tab_theme.rs` and `src/app/mod.rs` (13e's `DIMMING_AMOUNT` uses read `SplitPaneDimmingAmount`; 13e's `App::divider_color` passes its `invisible` background only when `DimInactiveSplitPanes` is on and `DimOnlyText` off; 13e's `tab_palette` dimming follows `dim::amount`), `src/scene.rs` (`Highlights::undimmed` `:184` becomes `pub dim: dim::Dim`; `pane_with` `:317` draws its dim quad only when `highlights` is None, else applies Dim Only Text to text and non-default backgrounds), `src/app/mod.rs` (the render loop computes each pane's `Dim` and lays the gray quad over the whole pane rect), `src/app/pane_title_ui.rs` (13h's `dimmed_for` goes; `Parts { dimmed, dim_amount, dim_only_text, dim_inactive }` come from this model: dimmed when the amount is above 0), the None-passing preview paths `scene::pane` (`src/scene.rs:201`) and `src/screenshot.rs:265-276` (they pass None, unchanged).

**Interfaces:**
- Consumes: Task 1's `DimInactiveSplitPanes`, `SplitPaneDimmingAmount`, `DimOnlyText`, `DimBackgroundWindows`; 13e's `tab_theme::{toward_gray, perceived, DIMMING_AMOUNT}`; `broadcast::{receives, should_broadcast_to}`.
- Produces in `scene`: `Highlights { .., pub dim: crate::dim::Dim }` replacing `pub undimmed: bool` (the name 13y2 consumes, R13xy-12; 13g adds `bg_alpha` and `opaque_colors` to the same struct, keep both).
- Produces in `dim`: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct Dim { pub amount: f32, pub only_text: bool }` with `pub const NONE: Dim` (amount 0) and `pub const LEGACY: Dim` (amount 0.4, the None path); `pub fn inactive_split(dim_inactive: bool, is_active: bool, receives: bool, current_receives: bool) -> bool` (`PT:12402-12422`); `pub fn background(dim_background_windows: bool, window_focused: bool) -> bool` (`SV:1497-1508`); `pub fn amount(inactive_split: bool, background: bool, base: f32) -> f32` (`SV:1449-1469`); `pub const GRAY: f32 = 0.5`; `pub fn text_color(c: Rgb, d: Dim, bg_brightness: f32) -> Rgb`; `pub fn bg_color(c: Rgb, default_bg: Rgb, d: Dim, bg_brightness: f32) -> Rgb`.

- [ ] **Step 1: failing tests** in `dim.rs`:
  - `dim_amounts_follow_iterm2` (Review Focus 4): `amount(true, false, 0.4)` 0.4; `amount(true, true, 0.4)` 0.6 within 1e-6; `amount(false, true, 0.4)` 0.4; `amount(false, false, 0.4)` 0; `amount(true, true, 0.6)` 0.9 (the cap); `amount(true, false, 1.5)` 0.9; `inactive_split(true, false, false, false)` true; `inactive_split(true, true, ..)` false; `inactive_split(false, false, ..)` false; `inactive_split(true, false, true, true)` false (broadcasting to it and to the current session); `inactive_split(true, false, true, false)` true; `background(false, false)` false; `background(true, false)` true; `background(true, true)` false.
  - `dim_colors_follow_iterm2` (ruling R3, `ExternalSessionState/iTermColorMap.m:299-306`, `:343-360`): 13e's `toward_gray(rgb(255, 255, 255), 0.4, 0.5)` is `rgb(204, 204, 204)` and `toward_gray(rgb(0, 0, 0), 0.4, 0.5)` is `rgb(51, 51, 51)` (the average `text_color` and `bg_color` use); with `only_text` false, `text_color` and `bg_color` leave colors alone (the gray quad at `Dim.amount` does the averaging).
  - `only_text_follows_iterm2` (`iTermColorMap.m:529-560`): `Dim { amount: 0.4, only_text: true }` on a black default background (brightness 0): `text_color(rgb(255, 255, 255), d, 0.0)` is `rgb(153, 153, 153)`; `bg_color(rgb(255, 0, 0), rgb(0, 0, 0), d, 0.0)` is `rgb(153, 0, 0)`; `bg_color(rgb(1, 1, 1), rgb(0, 0, 0), d, 0.0)` is `rgb(1, 1, 1)` (within 0.01 of the default counts as the default); amount 0 leaves everything.
  - `scene.rs` `dim_quads_follow_the_dim`: `pane_with` with `Highlights { dim: Dim::NONE, .. }` pushes no dim quad; with `dim.only_text` true it pushes no quad and a white cell's text row carries the dimmed color; with `highlights` None and `focused` false it pushes today's quad (the screenshot path, unchanged until 13y).
- [ ] **Step 2:** run `scripts/xt test --lib dim` and `scene`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Dimming". The render loop computes `inactive_split(DimInactiveSplitPanes, id == tab.focused, receives, current_receives)`, `background(DimBackgroundWindows, self.focused)`, `amount(.., SplitPaneDimmingAmount)` and passes `Dim { amount, only_text: DimOnlyText }`; without `only_text` and with an amount above 0 it lays `rgba(rgb(128, 128, 128), amount)` in `overlay` over the pane rect from `visible` (margins included; the title bar keeps its own dimmed colors). Update every `Highlights` constructor (the compiler lists them, tests included) from `undimmed: b` to `dim: if b { Dim::NONE } else { Dim::LEGACY }`.
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: the dimming model`.

### Task 8: The active pane border

**Files:** Modify `src/app/pane_nav_ui.rs` (`border_edges`, `draw_active_border`), `src/app/mod.rs` (the render loop calls `draw_active_border` after the pane's content).

**Interfaces:**
- Consumes: `Profile::{bool_for, color_for}` (`src/profile.rs:112`, `:145`), the profile keys "Use Active Pane Border" and "Active Pane Border Color" (`schema/profile.toml:707-732`, light and dark variants); `App::dark`; `scene::{Quad, rgba}`.
- Produces: `pub(super) fn border_edges(r: Rect, w: f32) -> [Rect; 4]` (pure: top, bottom, left, right, inside `r`); `pub(super) fn border_alpha(window_focused: bool) -> f32` (1.0, or 0.5 in a window without focus, `SV:2582-2586`); `pub(super) fn draw_active_border(&self, frame: &mut scene::Frame, tab: &Tab, id: PaneId, rect: Rect)`.

- [ ] **Step 1: failing tests** in `pane_nav_ui.rs`:
  - `active_border_follows_iterm2`: `border_edges((0, 0, 100, 50), 2.0)` is `[(0, 0, 100, 2), (0, 48, 100, 2), (0, 0, 2, 50), (98, 0, 2, 50)]`; `border_alpha(true)` 1.0, `border_alpha(false)` 0.5; the pure `pub(super) fn shows_border(use_border: bool, is_active: bool) -> bool` is true only for the active pane with the setting on, also when it is the only pane (`SV:2563-2570`).
- [ ] **Step 2:** run `scripts/xt test --lib pane_nav_ui`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Active pane border": 2 points times the scale, in `overlay`, around the terminal area (the pane rect from `visible`, below any title bar and above a bottom status bar), square corners. The color's alpha from the profile is not kept (`color_for` returns RGB; Decision).
- [ ] **Step 4:** run the filter, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: the active pane border`.

### Task 9: Lock Pane

**Files:** Create `src/app/pane_lock_ui.rs` (declare in `src/app/mod.rs`). Modify `src/pane.rs` (`pub locked: bool`), `src/app/mod.rs` (`start_pane_as` `:1210` sets `locked` from "Default Pane Locked"; `restart_pane` `:1145` keeps it), `src/close_rules.rs` (locked subjects, `LOCKED_REASON`, `profile_reason` priority), `src/app/close_ui.rs` (`close_reasons` `:18` adds the reason; `confirm_close` `:66` passes `any_locked`), `src/app/semantic_ui.rs` (`CtxItem::{LockPane(bool), LockAll, UnlockAll}` and their picks; `show_context_menu_on` adds them after Toggle Broadcasting Input), `src/app/menu.rs` (`show_context_menu_with` shows `LockPane` as a check item; `"moveSessionToWindow:"` and `"moveSessionToTab:"` `:433-435` refuse a locked pane), `src/app/pane_move_ui.rs` (13h's `pane_move_refused` adds the pane lock), `src/app/pane_title_ui.rs` (13h's `Parts.locked` and `geometry(.., locked)` read the pane; the `Hit::Lock` arm calls `toggle_pane_lock`), `src/app/arrange_ui.rs` (`session_value` `:39` writes "Locked"; the restore near `:299` reads it).

**Interfaces:**
- Consumes: 13h's `pane_title::geometry`, `TitleAction::ToggleLock`, `pane_move_refused`, `show_context_menu_on`; `close_rules::{body, subject, Subject, must_ask, quit_reasons}`; `Profile::bool`.
- Produces in `pane_lock_ui`: `impl App { pub(super) fn toggle_pane_lock(&mut self, pane: PaneId); pub(super) fn lock_all_in_tab(&mut self, pane: PaneId, lock: bool) }`; `pub(super) fn lock_items(tab_panes: usize, locked: bool, all_locked: bool, any_locked: bool) -> Vec<(&'static str, LockItem)>` (pure; `pub(super) enum LockItem { Pane(bool), All, UnlockAll }`).
- Produces in `close_rules`: `Subject::{LockedSession, TabLocked, MultiPaneTabLocked, WindowLocked}`; `pub fn subject(kind: Kind, panes: usize, any_locked: bool) -> Subject`; `pub const LOCKED_REASON: &str = "This pane is locked."`.

- [ ] **Step 1: failing tests:**
  - `close_rules.rs` `lock_texts_follow_iterm2` (Review Focus 5, `PT:2165-2310`, `iTermPromptOnCloseReason.m:237-239`): `subject(Kind::Session, 1, true)` LockedSession; `(Kind::Tab, 1, true)` TabLocked; `(Kind::Tab, 3, true)` MultiPaneTabLocked; `(Kind::Window, 4, true)` WindowLocked; `any_locked` false gives today's subjects; `body(LockedSession, &[])` "This locked session will be closed."; `body(TabLocked, &["vim".to_string()])` "This tab (with a locked session) is running vim." (`body` takes `&[String]`); `body(MultiPaneTabLocked, &["a".to_string(), "b".to_string()])` "This multi-pane tab (with locked sessions) is running the following jobs: a and b."; `body(WindowLocked, &[])` "This window (with locked sessions) will be closed."; `quit_reasons(&[LOCKED_REASON.into()], ..)` holds a `Message` reason of priority 75 with that text.
  - `pane_lock_ui.rs` `lock_items_follow_iterm2` (`CMH:671-713`): `lock_items(1, false, false, false)` is `[("Lock Pane", Pane(false))]`; `(3, false, false, false)` adds `("Lock All Panes in Tab", All)`; `(3, true, false, true)` is Lock Pane checked, Lock All Panes in Tab, Unlock All Panes in Tab (iTerm2's Alt alternate shown as its own item, Decision); `(3, true, true, true)` is Lock Pane checked and Unlock All Panes in Tab only.
  - `lock_refuses_moves`: the pure `pub(super) fn move_refused(layout_locked: bool, pane_locked: bool) -> bool` (13h's `pane_move_refused` calls it) is true when either is set (`MPC:69-93`, `:128-135`, `:313-322`); the pure `pub(super) fn close_reason_for(exited: bool, locked: bool) -> Option<&'static str>` is `Some(LOCKED_REASON)` only for a live locked pane (`PS:3068-3078`).
- [ ] **Step 2:** run `scripts/xt test --lib close_rules` and `pane_lock_ui`; expect failures.
- [ ] **Step 3:** implement per Resume notes "Locks". A locked pane's close always asks (the reason makes `must_ask` true while its shell runs) and names the locked subject; the context menu's Lock Pane toggles, Lock All and Unlock All act on every pane of the clicked pane's tab; the title bar shows the lock mark for a locked pane and a left click on it toggles (`STV:202-212`); Move Session to Split Pane, Swap With Session\u{2026}, title bar drags and Move Session to Window or Tab refuse a locked pane with a beep (iTerm2 disables the items; Tandem's menu items are static, Decision); key-action swaps (53 to 56, 67, 68) are not refused (`PT:12039-12065` do not check the lock). "Locked" is written for every session and read back when restoring an arrangement or saved state; a session that starts from a profile takes "Default Pane Locked" once (`PS:6211-6213`); Restart Session keeps the lock.
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: Lock Pane`.

### Task 10: Lock Split Pane Width

**Files:** Modify `src/layout.rs` (`has_row_ancestor`, `pin_width`), `src/pane.rs` (`pub pinned_width: Option<f32>`, points), `src/app/pane_lock_ui.rs` (`toggle_width_lock`, `width_lock_allowed`, `apply_width_pins`), `src/app/menu.rs` (`"toggleLockSplitPaneWidth:"` arm), `src/app/seltools_ui.rs` (`CHECKED` `:14` gains `"toggleLockSplitPaneWidth:"`; `is_checked` `:75` reads the focused pane), `src/app/mod.rs` (`WindowEvent::Resized` `:3079` calls `apply_width_pins` before `relayout`), `src/commands.rs` (`DONE` gains `"toggleLockSplitPaneWidth:"`).

**Interfaces:**
- Consumes: `Layout::{tree, rects, set_ratio}`, `Tree`, `Axis`; `App::{tmux_of, scale, gap, content_area_for}`.
- Produces in `layout`: `pub fn has_row_ancestor(tree: &Tree<PaneId>, pane: PaneId) -> bool`; `pub fn pin_width(&mut self, pane: PaneId, width: f32, area: Rect, gap: f32) -> bool` (when the pane's direct parent split is `Axis::Row` and the pane is wider than `width`, sets that split's ratio so the pane is `width` wide; false otherwise).
- Produces in `pane_lock_ui`: `pub(super) fn width_lock_allowed(tmux: bool, tab_panes: usize, row_ancestor: bool) -> bool` (pure, `PS:21736-21754`); `impl App { pub(super) fn toggle_width_lock(&mut self); pub(super) fn apply_width_pins(&mut self) }`.

- [ ] **Step 1: failing tests:**
  - `layout.rs` `width_pins_follow_iterm2` (Review Focus 5, `TAB:6596-6640`): `Row[1, 2]` in 400 x 100, gap 0: `pin_width(1, 120.0, ..)` is true and pane 1 is 120 wide; `pin_width(2, 100.0, ..)` makes pane 2 100 wide; pinning 1 to 300 when it is 200 wide is false (a pin only caps); `Column[1, 2]` gives false; in `Row[1, Column[2, 3]]` pinning 2 gives false (its direct parent is a Column split: iTerm2 caps only a session view that is a direct child of a vertical split); `has_row_ancestor(Row[1, Column[2, 3]], 3)` true; `has_row_ancestor(Column[1, 2], 1)` false.
  - `pane_lock_ui.rs` `width_lock_offers_follow_iterm2`: `width_lock_allowed(false, 2, true)` true; `(true, 2, true)` false (tmux); `(false, 1, true)` false (solo); `(false, 3, false)` false (no side-by-side split).
- [ ] **Step 2:** run `scripts/xt test --lib layout` and `pane_lock_ui`; expect failures.
- [ ] **Step 3:** implement. The menu item toggles the focused pane: allowed, it stores the pane's current width in points (`rect.w / scale`) or clears it (`PS:21756-21764`); not allowed, nothing happens. `apply_width_pins` runs only on a window resize (iTerm2 applies the pin in `resizeSubviewsWithOldSize:`), for every tab of the window, pinned panes in layout order; a divider drag is free to change a pinned pane (Decision). Tandem has no escape sequence that resizes a pane's columns, so iTerm2's refusal in `screenSetSize:` has nothing to guard (Decision). The check mark follows the focused pane.
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `panes: Lock Split Pane Width`.

### Task 11: Parity, checklist, rulings

**Files:** Modify `docs/parity/01-profile-settings.md`, `docs/parity/02-global-settings.md`, `docs/parity/03-features-and-menus.md`, `docs/parity/00-roadmap.md` (the milestone 13 paragraph: 13i done); create `docs/acceptance/m13i-checklist.md`, `docs/superpowers/rulings/m13i-rulings.txt`.

- [ ] **Step 1:** parity: `01:69` Use Active Pane Border and `01:70` Active Pane Border Color (have; remove "Tandem dims inactive panes instead" and the milestone 2 note; Windows note: square corners); `01:259` Default Pane Locked (have: Lock Pane in the context menu and the title bar mark); `02:247` DimOnlyText, `02:248` Dimming amount (have; the real key is `SplitPaneDimmingAmount`, range 0.01 to 0.6); `02:249` Dim inactive split panes (have; now in the schema); `02:212` Dim background windows (have; section Dimming); `02:550`, `02:732`, `02:879`, `02:927`, `02:928`, `02:1079` (have); `03:293` Lock Split Pane Width (have; remove "(milestone 7)"; describe it as capping the pane's width through window resizes).
- [ ] **Step 2:** `m13i-checklist.md` in the m10b form (copy `tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, `OpenConsole.exe` together): with a top row of two panes over a wide bottom pane, Next Pane visits them in reading order and with NavigatePanesInReadingOrder off in the order last used; Alt+Number (pane shortcut) and the title bar numbers agree; Select Pane Right from a pane with two neighbors goes to the one used last, and at the right edge wraps to the left (with WrapFocus off it stops); a new split under focus follows mouse keeps focus when FocusNewSplitPaneWithFocusFollowsMouse is off; a session whose font was changed by a script splits with that font (and with UseDivorcedProfileToSplit off with the saved profile's); SplitPaneDividerWidth 4 and SplitPaneColor #ff0000 change the dividers; inactive panes dim toward gray, the Dimming amount slider changes it, Dim only text keeps the background, Dim inactive split panes off stops it, Dim background windows makes an unfocused window's inactive panes dim more; Border around active pane draws a 2-point border that fades when the window loses focus; Lock Pane from the context menu shows the lock mark, closing asks "This locked session will be closed.", Move Session to Split Pane and a title bar drag refuse with a beep, the mark's click unlocks; Lock All and Unlock All Panes in Tab; Default Pane Locked on a profile; a saved and restored arrangement keeps a lock; Lock Split Pane Width keeps a pane's width while the window grows; one line per kept iTerm2 quirk and ruling a person can see.
- [ ] **Step 3:** `m13i-rulings.txt` in the `Ruling: ... - cost if wrong: ...` form: every Decision below, then each task's rulings and deferred minors as they happen.
- [ ] **Step 4:** run `scripts/xt test --lib`; commit `docs: m13i parity, checklist, rulings`.

---

## Resume notes (facts gathered before Task 1)

- Nothing of 13i is written; 13h must be built first. Start the ledger at `.superpowers/sdd/2026-10-15-m13i-pane-navigation/progress.md` with `task-start PLAN 1` (create the folder). iTerm2 paths are under `~/gitrepos/iterm2-ref/sources/` (HEAD `64791fc`). `TAB` `TerminalView/PTYTab.m`, `PT` `TerminalView/PseudoTerminal.m`, `PS` `PTYSession/PTYSession.m`, `SV` `TerminalView/SessionView.m`, `STV` `TerminalView/SessionTitleView.m`, `SPV` `TerminalView/PTYSplitView.m`, `CM` `ExternalSessionState/iTermColorMap.m`, `CMH` `ContextMenu/iTermTextViewContextMenuHelper.m`, `MPC` `TerminalView/MovePaneController.m`, `AS` `Settings/iTermAdvancedSettingsModel.m`, `PREF` `Settings/iTermPreferences.m`, `APV` `Settings/AppearancePreferencesViewController.m`, `APP` `AppKit/iTermApplication.m`.

### Tandem code these tasks build on (verified at `5269076`)

- Layout: `Layout::{rects, tree, panes, next, neighbor, set_ratio, split, swap}`, `Tree`, `Axis::{Row, Column}` (Row is side by side), `Dir`, `split_rect` (`src/layout.rs`; `next` `:335`, `set_ratio` `:343`, `neighbor` `:362` nearest by center, `visible_rects`); `GAP` (`src/app/mod.rs:45`, 1 pixel, 10 uses in `mod.rs`, `menu.rs`, `statusbar_ui.rs`, `clippings_ui.rs`, `api_requests.rs`); divider drag and grab (`src/app/mod.rs:2193`, `:2399`).
- Navigation callers: `select_pane_dir` (`src/app/actions.rs:110`), `step_pane` (`:120`), `swap_with` (`:136`), key actions 18 to 21 (`:298-301`), 30 and 31 (`:335-336`), 53 to 56 and 67, 68 (`:359-368`), pointer actions (`:496-509`); the menu's `selectPane*` (`src/app/menu.rs:701-715`) and `nextPane:`/`previousPane:` (`:718-725`, its own code); the builtin `select_pane_in_direction` (`src/app/builtin_ui.rs:445-455`); `Route::SelectPane` (`src/app/mod.rs` near `:2077`, `layout.panes()`, 9 the last); `status_rows` ordinals (`src/app/toolbelt_more_ui.rs:639-655`, position plus 1); 13h's ordinals in `pane_title_ui`.
- Splits: `split` (`src/app/mod.rs:1601`, always focuses the new pane), the API split (`src/app/api_requests.rs` near `:610-636`, `tab.focused = id` at `:634`), `focused_guid` (`src/app/menu.rs:369`, the saved profile), the split menu arms (`:515` on); `Store::{get, shared_guid, sessions}` (`src/store.rs:97-110`), `session_profile::{divorce, SessionProfile}` (`src/session_profile.rs:16-70`), `App::{divorce, forget_session_profile}` (`src/app/session_ui.rs`), `api_session_copy` (`src/app/api_requests.rs:445`); `FocusFollowsMouse` (`schema/global.toml:226`, read at `src/app/mod.rs:2414`).
- Dimming: `DIM_ALPHA` (`src/scene.rs:151`), `Highlights { undimmed }` (`:172-188`), the dim quad over the inner rect (`:550-551`), `pane_with` (`:317`); the render loop's `undimmed` rule (`src/app/mod.rs` near `:2537`: an unfocused window's focused pane without `DimBackgroundWindows`, or a broadcast receiver while broadcasting to the focused pane); `DimBackgroundWindows` (`schema/global.toml:500`); no `DimInactiveSplitPanes`, `DimOnlyText` or `SplitPaneDimmingAmount` key; `screenshot_ui::render_lines` and the Open Quickly preview pass no `Highlights` (13y's logged gap).
- Dividers: `scene::dividers` (`src/scene.rs:923`), `DIVIDER` `#4a4a4a` (`:140`); after 13e, `dividers` takes a color and `App::divider_color` gives the theme's.
- Profiles: "Use Active Pane Border" and "Active Pane Border Color" (`schema/profile.toml:707-732`, light and dark variants), "Default Pane Locked" (`:2463`); `Profile::{bool, bool_for, color_for}` (`src/profile.rs:91`, `:145`, `:112`).
- Close: `close_ui::{Closing, confirm_close, close_reasons}` (`src/app/close_ui.rs:8`, `:66`, `:18`), `close_rules::{Kind, Subject, subject, must_ask, body, quit_reasons, profile_reason}` (`src/close_rules.rs:8-152`), `close_prompt::oxford`.
- Menus and checks: `seltools_ui::{CHECKED, is_checked}` (`src/app/seltools_ui.rs:14`, `:75`), `sync_checks` (`src/app/menu.rs:271`), `schema/menus.toml:2097-2100` (Lock Split Pane Width), `commands::owner` (`LockSplitPaneWidth` milestone 7 at `src/commands.rs:131`), `moveSessionToWindow:`/`moveSessionToTab:` (`src/app/menu.rs:433-435`).
- Arrangements: `session_value` (`src/app/arrange_ui.rs:39`), the restore loop (`:287-299`, `restore_clippings` at `:299`).
- Settings: `global_control` (`src/settings_keys.rs:581`, Bool and Enum only), `row_enabled` (`:731`), `schema_page` (`:842`), `Globals::number` (`src/globals.rs:110`).
- Loop: `about_to_wait` (`src/app/mod.rs:2839`), `WindowEvent::Resized` (`:3079`), `start_pane_as` (`:1210`), `restart_pane` (`:1145`).

### Pane order (TAB, APP)

- `TAB:1052-1088` `orderedSessions`: with NavigatePanesInReadingOrder (default YES) the sessions sorted by their root-relative origins (flipped coordinates, y down): when the root split is not vertical (stacked), by y then x ("true reading order"); when it is vertical (side by side), by x then y; origins within 1 point are equal. Maximized, the order is the snapshot taken when maximizing (`TAB:5638`), the same order since the layout does not change. With the key NO, ascending `activityCounter`.
- `TAB:902-914`: `setActiveSession:` gives the old and the new active session fresh counters, unless `updateActivityCounter:NO`. `TAB:1124-1170`: Next and Previous (`nextSession`, `previousSession`) step through `orderedSessions` by plus or minus 1 with wrap and activate with `updateActivityCounter:NO`. `APP:464-481`: the pane number shortcut picks from `orderedSessions` (9 the last) the same way. `TAB:678-691`: title bar ordinals come from `orderedSessions`; `ToolStatus.swift:1000-1009` shows them.
- `TAB:1117-1122`: swap with next or previous pane uses `sessionInDirection:` over the same order.

### Directional navigation (TAB, PT)

- `TAB:1454-1487` projection: B is in A's projection looking right (`after`) when B's top and bottom overlap A's by more than a 1-point safety margin (`bBottom >= aTop + 1 && bTop + 1 < aBottom`) and B's left is at or past A's right; looking left, the x values are negated; up and down swap x and y first.
- `TAB:1489-1531`: B is adjacent when no C in the projection sits between A and B (C overlaps both on the cross axis and lies between them).
- `TAB:1541-1567`: the adjacent session with the highest activity counter; with none and WrapFocus (default YES), the sessions in the projection looking the other way with the minimum "left" (the far edge), the one with the highest counter; with WrapFocus NO, nil.
- `PT:11987-12037`: Select Pane Left/Right/Above/Below activate with `setActiveSession:` (the counter moves). `PT:12039-12065`: swap pane by direction uses the same pick and `swapSession:withSession:`.

### Splits (PT, PS)

- `PT:11364-11370`: after a split, when the target was the active session, the new session becomes active unless focus follows mouse is on and FocusNewSplitPaneWithFocusFollowsMouse (default YES) is off. `PT:11376-11385`: when the target session is divorced, the new session inherits its divorce (`PS:8497-8511`: the overridden fields copied).
- `PS:2915-2945` `profileForSplit`: with UseDivorcedProfileToSplit (default YES) and a divorced session, the session's own profile; else the original profile looked up by Guid, then the session's profile, then the default. `PT:11581-11598`: the split menu items (Split Vertically/Horizontally with Current Profile) use it.

### Dimming (SV, PT, CM, PREF, APV)

- `PREF:134-142`, `:837-845`: `DimOnlyText` NO, `SplitPaneDimmingAmount` 0.4, `DimInactiveSplitPanes` YES, `DimBackgroundWindows` NO. `APV:351-374`: the Dimming box on the Appearance pane; the slider runs 0.01 to 0.6 (`Settings/Base.lproj/PreferencePanel.xib`, tooltip "Inactive sessions get dimmed to help find the active session.").
- `PT:12402-12422`: a session dims as an inactive split when Dim inactive split panes is on, it is not its tab's active session, and not (input is broadcast to it and to the current session). `SV:1497-1508`: every session of a window dims as a background window when Dim background windows is on and the window is not key. `SV:1449-1469`: the amount is 0, 1 or 1.5 times the dimming amount for none, one or both reasons, clamped to 0 to 0.9.
- `CM:299-306`: averaging is `a (1 - t) + b t` per channel. `CM:343-360`, `:452-462`: text colors average toward gray 0.5, or toward the default background's perceived brightness with Dim Only Text. `CM:466-560`: background colors average toward 0.5 when not Dim Only Text (the default background included); with Dim Only Text only non-default backgrounds (more than 0.01 away in any channel) dim, toward the background's brightness. Tandem's quad of gray at the amount is the same average for every color (in linear light instead of sRGB, Decision).

### Active pane border (SV, PS)

- `PS:5809-5812`: the light and dark variants of "Use Active Pane Border" and "Active Pane Border Color". `SV:2563-2600`: shown on the tab's active session when on (a lone pane too); color at half alpha in a window that is not key; framed around the scroll view (the terminal area); corners rounded where they meet the window's rounded corners. `TerminalView/iTermActivePaneBorderView.swift:21-74`: a 2-point stroke inset by half its width.

### Locks (PS, PT, CMH, MPC, STV)

- `PS:6211-6213`: a session takes "Default Pane Locked" the first time its profile loads. `PS:1920`, `:7325`: arrangements save and restore "Locked". `PS:13516-13527`: setting the lock updates the title bar's lock button.
- `PS:3068-3078` `promptOnCloseReason`: an exited session has none; a locked one gives "This pane is locked." (priority 75, `AppShutdown/iTermPromptOnCloseReason.m:237-239`), before the profile's rule. `PT:2371-2400`, `:2745-2757`, `:14304-14312`: the close question names the locked subjects; `PT:2165-2310` their texts.
- `CMH:671-713`: after Toggle Broadcasting Input: "Lock Pane" (checked when locked); with several panes "Unlock All Panes in Tab" when all are locked, else "Lock All Panes in Tab" and, when any is locked, "Unlock All Panes in Tab" as its Option alternate. `TAB:5589-5617`: lock and unlock every session of the tab.
- `MPC:69-93`, `:128-135`, `:313-322`: move, swap, new-window moves and drags refuse a locked session; `PT:13883-13893`: Move Session to Window and to Tab are disabled for a locked current session. `STV:86-102`, `:202-212`: the lock button (tooltip "This pane is locked. It cannot be moved, swapped, or dragged, and closing it requires confirmation. Right-click to unlock.") toggles the lock.
- `PS:21736-21764`, `PTYTextView+ARC.m:216-231`: Lock Split Pane Width is allowed for a non-tmux session in a tab with several sessions that has a vertical (side-by-side) split above it; locking stores the view's width, unlocking clears it. `TAB:6596-6640`: during a resize, a session view that is a direct child of a vertical split is capped at its stored width. `PS:15737-15740`, `:15792-15794`: a width-locked session refuses escape-sequence resizes.

### Settings rows

- `NavigatePanesInReadingOrder`: `tab = "Advanced"`, `section = "Tabs"`, bool, default true, label and help "Next Pane and Previous Pane commands use reading order, not the time of last use." (`AS:311`).
- `WrapFocus`: Advanced, "General", bool, true, "Should split pane navigation by direction wrap around?" (`AS:510`).
- `FocusNewSplitPaneWithFocusFollowsMouse`: Advanced, "Session", bool, true, "When focus follows mouse is enabled, should new split panes automatically be focused?" (`AS:667`).
- `SplitPaneColor`: Advanced, "Windows", string, "", label "Custom color for split pane dividers. Leave empty to use default color.", help that plus "\nThis should be an HTML-style color, like #aabbcc." (`AS:715`).
- `SplitPaneDividerWidth`: Advanced, "Windows", float, 0.0, label and help "Custom width for split pane dividers in points (1-5). Set to 0 to use default width." (`AS:716`).
- `UseDivorcedProfileToSplit`: Advanced, "Experimental Features", bool, true, "When splitting a pane, use the profile with local modifications, not the backing profile." (`AS:881`).
- `DimInactiveSplitPanes`: `tab = "Appearance"`, `section = "Dimming"`, bool, true, label "Dim inactive split panes", help "Split panes without the keyboard focus are dimmed." (Tandem's).
- `DimOnlyText`: Appearance, "Dimming", bool, false, label "Dimming affects only text, not background.", help "Dimming changes the text and colored cells, not the default background." (Tandem's).
- `SplitPaneDimmingAmount`: Appearance, "Dimming", float, 0.4, `min = 0.01`, `max = 0.6`, label "Dimming amount:", help "Inactive sessions get dimmed to help find the active session." (the slider's tooltip).
- `DimBackgroundWindows`: unchanged except `section = "Dimming"`.

### Texts (Tandem's, `\u{...}` as written)

- Context menu: "Lock Pane", "Lock All Panes in Tab", "Unlock All Panes in Tab" (`CMH:673`, `:691`, `:698`).
- Close: `LOCKED_REASON` "This pane is locked."; bodies "This locked session is running %@." / "...the following jobs: %@." / "...: %1$@, plus %2$@." / "This locked session will be closed."; "This tab (with a locked session) ..."; "This multi-pane tab (with locked sessions) ..."; "This window (with locked sessions) ..." (`PT:2165-2310`), with Tandem's existing "plus N others" form.

### Corrections to the research (the source wins)

- `02:248`'s key is `SplitPaneDimmingAmount` on the Appearance pane (range 0.01 to 0.6), not an Advanced setting, and `02:247` DimOnlyText is on the Appearance pane too; iTerm2 dims by averaging colors toward middle gray, not by fading toward the background (Tandem's quad color is a default mismatch the facts did not list; ruling R3 fixes it).
- `02:550` "hard-coded at iTerm2's default" is not quite true: Tandem steps in layout tree order, which differs from iTerm2's geometric reading order for nested splits such as a split top row over a full-width bottom pane (ruling R3 fixes it). The pane number shortcut, the title bar ordinals and the Status tool's shortcuts use the same order in iTerm2.
- `02:249` Dim inactive split panes is marked have, but no key exists, so it cannot be turned off; this plan adds it.
- `02:879` also applies to the API's split: a split of a pane that is not the active one never takes focus (`PT:11365`).
- `03:293` Lock Split Pane Width caps the pane's width during window resizes (a maximum); the pane still shrinks with a smaller window.
- Key-action swaps by direction or order do not check the pane lock in iTerm2 (`PT:12039-12065`, `TAB:1117-1122`); only the move controller does.
- Research line numbers checked at `64791fc`.

### Decisions (each goes in `m13i-rulings.txt`)

- Next Pane, Previous Pane, pane numbers, title bar ordinals and the Status tool's shortcuts follow iTerm2's geometric reading order (ruling R3). Cost: some nested layouts step in a new order.
- Activity stamps are one app-wide sequence on each `Pane` (iTerm2 counts per tab; only the order inside a tab is compared, so the result is the same). Cost: none.
- Focus changes are stamped by comparing each tab's focus once per loop pass, and quiet moves (Next, Previous, pane numbers, swap with next) record themselves so they are not stamped. Cost: two focus changes inside one pass stamp only the last.
- A zoomed tab steps through every pane and unzooms, as Tandem does today (iTerm2 swaps the maximized pane). Cost: a zoomed tab unzooms on Next Pane.
- The directional margin is 1 point times the scale. Cost: none.
- The default divider is 1 point (scaled, rounded, at least 1 pixel), where Tandem drew 1 pixel; a valid `SplitPaneColor` overrides 13e's theme color. Cost: 2-pixel dividers at 150 percent and above.
- Inactive panes dim with a gray quad at the computed amount over the whole pane (margins included), averaging in linear light (ruling R3). Cost: slightly different midtones from iTerm2's sRGB average.
- The Advanced string and float keys show no control until 13a adds text fields. Cost: they are edited in the file until then.
- `DimBackgroundWindows` moves to the Dimming section, as iTerm2's Appearance pane groups it. Cost: none.
- The active pane border has square corners and ignores the color's alpha. Cost: a different look at rounded window corners.
- Locked panes refuse Move Session to Window and to Tab with a beep, since Tandem's menu items are not validated. Cost: the items look enabled.
- The context menu shows Unlock All Panes in Tab as its own item when some panes are locked (iTerm2 hides it behind Option). Cost: one more item.
- Restart Session keeps the lock. Cost: none known.
- Width pins apply only on a window resize; a divider drag may widen a pinned pane. Cost: a drag can undo the pin until the next resize.
- Tandem has no escape sequence that resizes a pane, so the width lock's refusal there is not needed. Cost: none.
- Parity rows are found by title.

### Pitfalls for the implementer

- `GAP` becomes `self.gap()`: change every use, including the API's split rects and the clippings panel, or layouts disagree between drawing and hit testing.
- `Highlights { undimmed }` is replaced; the None-passing paths (`scene::pane` and `src/screenshot.rs:265-276`) must keep today's dim until 13y2, so do not change their calls.
- `step_pane` and `select_pane_dir` are private in `src/app/actions.rs` today; make them `pub(super)` (Tasks 3 and 4 list it) rather than copying their bodies into `menu.rs`.
- The dim quad and the border go in `overlay` (no text); never put them in `popups`, which cut text out.
- Do not stamp activity in `focus_quietly`, or Previous Pane stops toggling between the last two panes.
- `pane_seen` is keyed by tab id; a tab moved to another window keeps its id, so the entry stays valid; drop entries when tabs close.
- A split's session copy must be pushed to `store.sessions` before the pane starts, and `forget_session_profile` drops it with the pane.
- The lock must not stop tmux commands that close a pane from tmux's side (a `%window-close` or kill from tmux is not a user close).
