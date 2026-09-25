# Tandem Milestone 9h: Status Bar Content Components and the tmux Status Bar

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope split (read first):** the content components plus the tmux status bar are too big for one plan of about seven tasks. This plan is the first half, 9h. A later plan, 9i, covers the rest. These move to 9i, exactly:
- Battery Level, CPU Utilization, Memory Utilization, Network Throughput (research 2.3 to 2.6) and their registry entries, knobs and exemplars.
- The sparkline base (2.2.5): 60-sample history, the path geometry, the ceiling rule, the baseline, the left and right text, `animateGraphStatusBarComponents`.
- The Windows samplers: `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetIfTable2`, `GetSystemPowerStatus`, `excludeUtunFromNetworkUtilization`. `Cargo.toml` already enables `Win32_System_Threading` (GetSystemTimes) and `Win32_System_SystemInformation` (GlobalMemoryStatusEx). 9i adds `Win32_System_Power` (GetSystemPowerStatus, SYSTEM_POWER_STATUS) and `Win32_NetworkManagement_IpHelper` plus `Win32_NetworkManagement_Ndis` (GetIfTable2, FreeMibTable, MIB_IF_TABLE2; the NDIS feature gates the row type).
- git state (2.12): the attributed-text base, the git CLI worker with `gitTimeout` 4 and `gitSearchPath`, polling and bumps, the rendered string, the click menu, the single-use command windows, the remote `user.git*` variables.
- Rows 03:472-475, 03:481, 03:450; 02:744, 02:747, 02:761, 02:806.

Until 9i, a layout that names one of those five classes keeps it in the profile and skips it on screen (the 9g rule for classes Tandem does not show).

**Goal:** iTerm2's text-based status bar components on the 9g framework: Clock, Host Name, User Name, Job Name, Current Directory (with its click menu), Interpolated String, Custom Action, Actions Menu, Snippets Menu, Triggers Menu, Call Script Function (knobs, and the milestone 11 refusal on click), Search Tool and Filter Tool bound to the existing find bar and Filter, the Progress Indicator for slow pastes, and the Composer component as a stub; each text component gets shorter as the bar narrows by iTerm2's width variants; the temporary search, filter and progress components; the tmux status bar (`UseTmuxStatusBar`, `status-left` and `status-right` monitors on the 8f option monitors, the tmux left and right components); and "Save lines to scrollback when an app status bar is present" (`Scrollback With Status Bar`).

**Architecture:** `statusbar_components.rs` is the pure registry of the content classes: kinds, descriptions, knobs in iTerm2's order, default knobs, icons, exemplars, empty rules, the temporary components, and the tmux layout. `statusbar_text.rs` is pure text: every component's width variants, the widest-fitting pick, localhost detection, and a TR35 date formatter. The app side keeps `src/app/statusbar_ui.rs` (9g) as the bar and adds `src/app/statusbar_content.rs`, which turns a component and its session into a `Content` (variants, copy text, empty, icon, click). The tmux monitor pair lives in `tmux_controller.rs` beside the 8f monitors. The scrollback rule is one more vendored alacritty patch.

**Tech Stack:** as milestone 9g. No new crates. The ICU date-pattern call uses `windows-sys` `Win32_Globalization`, already enabled.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` sections 2.2.1 to 2.2.2, 2.7 to 2.11, 2.13 to 2.19, 2.22 to 2.25, 1.6, and the corrections in the Resume notes below (the source wins over the research where they differ). Rows: `03-features-and-menus.md` 476-480, 482-488, 491, 492, 425, 445; `01-profile-settings.md` 198; `02-global-settings.md` 168.

## Global Constraints

- Everything from milestones 0 to 9g still holds, including 9g's: bar height `statusBarHeight` (21) points times scale, margins 10 (left 6), icon 17, prose font at 12 pt unless a font knob or the layout sets one, colors as `{r, g, b, a}` tables, no vibrancy.
- Text widths come from `Renderer::text_width(text, true, scale)` in pixels, divided by the scale and rounded up to points. No NSTextField padding is added.
- An icon is a 17 pt column at the component's left, one Segoe Fluent Icons glyph (the scene's `icon: true` span) tinted with the component's text color. The icon adds 17 to the component's minimum and preferred width (the 9g `Item.icon` already does this).
- Knob keys, labels, defaults and strings are iTerm2's, verbatim (Resume notes). Characters outside ASCII are written in code as `\u{...}` escapes.
- iTerm2 bugs are kept and each is listed in the checklist (the 9c ruling): the Triggers Menu's inverted empty rule, User Name never shortening, the Directory component's dot variants, Composer having no compression knob.
- SSH integration is milestone 14: the Clock's two SSH knobs are shown and saved and have no effect. Script functions are milestone 11: Call Script Function refuses on click with "Script functions need the Python API, which arrives in milestone 11."; an Interpolated String with a function call shows `interp::evaluate`'s refusal as its error.
- The Composer component is a stub here: registry entry, knobs, sizes, an inert field. Its click says "The Composer arrives with its own milestone." The Composer milestone fills it in (typing, history, Return sends).
- tmux tabs share one bar per window (9g). The tmux monitors need tmux 2.9 or later.
- New pure code is a plain `pub mod` tested on Linux; app code stays `#[cfg(windows)]`. Tests run on EXTREME with `scripts/xt test --lib <filter>`.

## Review Focus

1. A bar too narrow for a Host Name's full name: the widest variant that fits is shown, and iTerm2's center-truncated first-label forms (with U+2026) come first in the list. Tests: Task 2 `hostname_variants_follow_iterm2`, `widest_fitting_variant_wins`.
2. Win+F in a session whose bar has no Search Tool: a temporary search field goes first on the bar and stays even when the bar is too narrow for everything; Esc removes it and the in-pane find bar is never drawn meanwhile. Test: Task 5 `temporary_search_stays_on_a_narrow_bar`.
3. tmux status text with style directives (`#[fg=red]`) and a `<'name' not ready>` placeholder: both are stripped, and the fast re-poll happens once per monitor pair, never again. Test: Task 6 `tmux_status_follows_iterm2`.
4. A tmux 2.8 server with "Status bar shows tmux status bar content" on: no status monitors, and the bar shows the profile's own components. Test: Task 6 `status_monitors_follow_iterm2`.
5. tmux run inside a plain pane (not control mode) with "Save lines to scrollback when an app status bar is present" off: lines scrolled under its status line do not reach scrollback; a full-screen scroll still does. Test: Task 7 `scroll_regions_follow_scrollback_with_status_bar`.

---

### Task 1: The content registry

**Files:** Create `src/statusbar_components.rs`; add to `src/lib.rs`. Modify `src/statusbar.rs`: `registry()` returns iTerm2's palette order with the content entries and the framework ones in place, `info()` and `knobs()` fall through to the new module, and add `exemplar()`. Update 9g's `framework_components_follow_iterm2` count (it asserts 3).

**Interfaces:**
- Consumes: `statusbar::{Info, Knob, KnobKind, Algorithm, Component, SPRING_CLASS, SPACER_CLASS, PLACEHOLDER_CLASS}`.
- Produces: class constants (`CLOCK_CLASS` and the rest, Resume notes table); `enum Kind { Clock, CustomAction, ActionsMenu, SnippetsMenu, TriggersMenu, HostName, UserName, JobName, CurrentDirectory, SearchTool, FilterTool, Composer, InterpolatedString, CallScriptFunction, TmuxLeft, TmuxRight, Progress }` with `Kind::of(class) -> Option<Kind>` and `Kind::class(self) -> &'static str`; `infos() -> Vec<Info>` (palette entries, then the internal ones); `knobs(kind, algorithm) -> Vec<Knob>`; `default_knobs(kind) -> toml::Table`; `icon(kind) -> Option<char>`; `exemplar(class, knobs: &toml::Table, clock_text: &str, user: &str, home: &str) -> String` (the caller formats the clock text, since the formatter arrives in Task 2); `statusbar::exemplar(class, knobs, clock_text, user, home)` wraps it and falls back to `Info::exemplar`.
- Where the 9g setup window draws `Info::exemplar` (palette and Active Components), it calls `statusbar::exemplar` instead.

- [ ] **Step 1: failing tests** in `statusbar_components.rs`:
  - `content_registry_follows_iterm2`: palette order is Clock, Custom Action, Actions Menu, Snippets Menu, Triggers Menu, Host Name, User Name, Job Name, Current Directory, Search Tool, Filter Tool, Composer, Fixed-size Spacer, Spring, Interpolated String, Call Script Function (9i inserts its five at iTerm2's positions); short and detailed descriptions verbatim; `stretch` true for all but Composer and Call Script Function; the Progress Indicator and tmux left and right are `internal`.
  - `content_knobs_follow_iterm2`: the knob matrix (research 2.25). Text components: "Text Color:", "Background Color:", "Custom Font", then "Compression Resistance:" (Tightly Packed) or "Size Multiple:" (Stable), "Priority:", "Minimum Width:", "Maximum Width:". Clock's four own knobs first. Host Name's "localhost replacement" first. Custom Action's "Action" (kind Action) first. Search and Filter only "Color" (key `shared text color`). Composer: "Icon Color:", "Background Color:", "Priority:", "Minimum Width:", "Maximum Width:". Call Script Function: "Label:", "Function call:" (kind Invocation), "Timeout (seconds):" (default 5), "Background Color:", "Text Color:", "Priority:". Minimum Width's default is the string "0", Maximum Width's is infinity.
  - `exemplars_follow_iterm2`: Clock shows the given clock text; Custom Action its action's title, else "Custom Action"; User Name the user; Current Directory the home; Interpolated String its expression, else `\(expression)`; Call Script Function its label, else `foo(bar: "baz")`; Search `"\u{1F50E} Search"`; Filter `"\u{2979} Filter"`; Composer `">_ [Command] \u{1F4AC}"`.
- [ ] **Step 2:** run `scripts/xt test --lib statusbar_components`; expect the three to fail.
- [ ] **Step 3:** implement. Default knobs: base `{base: compression resistance = 1, base: priority = 5}`; Clock adds `format = "M/dd h:mm"`; Job and Current Directory add `maxwidth = inf, minwidth = 0`; Call Script Function adds `timeout = 5`.
- [ ] **Step 4:** run `scripts/xt test --lib statusbar`; all pass (9g's updated count included).
- [ ] **Step 5:** commit `status bar content registry`.

### Task 2: Text variants and the clock

**Files:** Create `src/statusbar_text.rs`; add to `src/lib.rs`.

**Interfaces:**
- Produces:
  - `pick(widths: &[(String, f32)], available: f32) -> String`: the widest variant with `ceil(w) <= ceil(available)`, else `""`. `narrowest(widths) -> f32`, `widest(widths) -> f32`.
  - `host_variants(name) -> Vec<String>`, `display_dir(raw, home) -> String`, `dir_variants(display) -> Vec<String>`, `job_variants(chain: &[String]) -> Vec<String>`, `action_variants(title) -> Vec<String>`, `tmux_left_variants(value) -> Vec<String>`, `tmux_right_variants(value) -> Vec<String>`, `user_min_text(full) -> String`.
  - `is_local_host(host, local_names: &[String]) -> bool`.
  - `struct Civil { year: i32, month: u32, day: u32, weekday: u32 /*0 = Sunday*/, hour: u32, minute: u32, second: u32, millis: u32 }` and `format_tr35(pattern, &Civil) -> String`.

- [ ] **Step 1: failing tests:**
  - `widest_fitting_variant_wins`: three variants 40, 80, 120 wide; 100 gives the 80 one; 80.4 gives 80 (ceil on both sides); 30 gives `""`.
  - `hostname_variants_follow_iterm2`: `foobar.example.com` gives `fo\u{2026}ar`, `f\u{2026}ar`, `foobar`, `foobar.example.com`, `foobar.example.c`, `foobar.e.c`, `f.e.c`. A one-label name gives only itself. An empty name gives none.
  - `directory_variants_follow_iterm2`: home `C:\Users\rb` and raw `C:\Users\rb\src\iterm2\sources` display as `~\src\iterm2\sources`, then `~\s\iterm2\sources`, `~\s\i\sources`, `~\s\i\s`. A raw path with a dot in it also gets the first-label center forms first (iTerm2's shared base, kept). A forward-slash path splits on `/`.
  - `job_variants_follow_iterm2`: `["vim", "bash"]` gives `vim`, `vim \u{25C2} bash`; no job yet gives `[""]`; all names empty gives `["\u{2026}"]`.
  - `action_title_variants_follow_iterm2`: `Run my job` gives `Run`, `Run my`, `Run my job`; a one-word title gives itself (the research's "full title only if it ends in whitespace" is wrong, see Resume notes).
  - `tmux_variants_follow_iterm2`: left `a | b | c` gives `a | b | c`, `a | b`, `a`; right gives `a | b | c`, `b | c`, `c`.
  - `clock_formats_like_tr35`: `M/dd h:mm` at 2026-10-04 15:07 gives `10/04 3:07`; `yyyy-MM-dd HH:mm:ss` and `EEE MMM d` and `h:mm a` and a quoted literal `'at' h` and `''` for one quote.
  - `localhost_is_this_machine`: `localhost`, the computer name case-insensitively, and `name.local` forms match; another host does not.
- [ ] **Step 2:** run `scripts/xt test --lib statusbar_text`; expect failures.
- [ ] **Step 3:** implement. Names for `MMM`, `MMMM`, `EEE`, `EEEE`, `a` are English.
- [ ] **Step 4:** run and pass.
- [ ] **Step 5:** commit `status bar text variants and clock`.

### Task 3: Text components on the bar

**Files:** Create `src/app/statusbar_content.rs` (mod line in `src/app/mod.rs`). Modify `src/app/statusbar_ui.rs` (the 9g extension point), `src/scene.rs` (`BarText` gains `icon: bool`), `src/app/mod.rs` (`about_to_wait` folds in `tick_status_bars`), `src/app/prompted_ui.rs` (`session.isLocalhost`).

**Interfaces:**
- Consumes: Tasks 1 and 2; `App::variables(pane)`, `prompted::variable`, `interp::evaluate`, `shell_history_pub().dirs(host)`, `shell_history::insertable_dir`, `start_dir_override` with `arrangement::Folder::Forced`.
- Produces: `Piece::Content(Kind)` (`Piece::of` maps the content classes); `pub(super) struct Content { variants: Vec<String>, min_text: Option<String>, copy: Option<String>, empty: bool, icon: Option<char>, right_align: bool, fixed: Option<(f32, f32)> /*min, preferred points*/, click: Click }`; `App::content(pane, kind, &Component) -> Content`; `Click` gains `Menu(Kind)`, `Error(String)`, `Run`, `Field`, `Cancel`, `Refuse(&'static str)`; `Piece::item` takes the measured variant widths; `App::tick_status_bars() -> Option<Instant>`.

- [ ] **Step 1: failing test** `content_items_size_by_variants` in `statusbar_ui.rs`: variants 30 and 90 points wide make min 30 and preferred 90, stretchable; a Job with maxwidth 50 has preferred 50 (clamped); an icon marks `Item.icon`; a `fixed` content (125, 200) sizes to that; an empty-text content is `empty`.
- [ ] **Step 2:** implement `Content` for Clock (knobs `format`, `localize`; Windows `GetLocalTime` gives the `Civil`; with localize, `udatpg_getBestPattern` turns the template into a pattern, and when that fails the template is used as given; single variant; cadence 1 s), Host Name (`hostname`; the replacement knob when local), User Name (`username`; one variant, `min_text` = `user_min_text`), Job Name (the job's name then `job_ancestors` after it, up to the shell), Current Directory (`path`; copy = raw path; empty when blank), Interpolated String (`expression` through `interp::evaluate` with the session lookup; error text is `"\u{1F41E}"` and the click shows the error), the Placeholder as before. Drawing picks `statusbar_text::pick` for the placed width less the icon, right-aligns when asked, and draws the icon glyph.
- [ ] **Step 3:** the Current Directory click opens a menu at the component: up to 10 recent directories for the host by score (starred ones prefixed `"\u{2605} "`), picking one types `cd <insertable path>` with no newline; then, when a path is known, separator, "Copy Path", "Copy Folder Name", "Show in Explorer" (Windows for "Reveal in Finder"), separator, "New Session Here in New Window", "New Session Here in New Tab" (the default profile with `start_dir_override`). Menu ids `statusbar:dir:<n>` and friends go through `status_bar_picked`.
- [ ] **Step 4:** `tick_status_bars` returns the next whole second while any bar on screen shows a Clock. The right-click Copy uses `Content::copy` (the widest variant for text components, the raw path for the directory).
- [ ] **Step 5:** run `scripts/xt test --lib statusbar` and build; commit `status bar text components`.

### Task 4: Menu and button components

**Files:** Modify `src/app/statusbar_content.rs`, `src/app/statusbar_ui.rs`, `src/statusbar_components.rs` (pure helpers), `src/settings_ui.rs` (open Shortcuts on a tab).

**Interfaces:**
- Consumes: `user_actions::load(globals "Actions")`, `UserAction::from_value`, `UserAction::binding`, `run_key_action`, `snippets::menu_entries`, `snippets::filtered`, the `SNIPPET_ID` and `ACTION_ID` menu ids of `snippets_ui.rs` (their pick path is reused), `run_trigger_command_menu("addTrigger:" | "editTriggers:")`, `set_session_values`, `triggers::{parse_list, to_value, action_info}`.
- Produces: `statusbar_components::menu_empty(kind, actions: usize, snippets: usize, triggers: usize) -> bool`; `trigger_menu_title(action_title, regex) -> String`; `SettingsWindow::show_shortcuts_tab(settings_shortcuts::Tab)`.

- [ ] **Step 1: failing test** `menu_components_follow_iterm2`: Actions Menu is empty with no actions, Snippets Menu with no snippets, the Triggers Menu is empty when there ARE triggers (iTerm2's bug, kept); the trigger title drops a trailing U+2026 from the action title and joins with `" \u{2014} "` and the regex; Custom Action is never empty.
- [ ] **Step 2:** implement. Custom Action: text = the action's title through `interp::evaluate`, variants by `action_variants`, error text `"\u{1F41E} <error>"`, click runs the action's binding. Actions Menu: `"Perform Action\u{2026}"`, click opens one item per action (the Edit > Actions ids), separator, "Edit Actions\u{2026}". Snippets Menu: `"Send Snippet\u{2026}"`, the same menu as Edit > Snippets (disabled "Press Option to Edit Before Sending", separator, the tag tree), then separator and "Edit Snippets\u{2026}". Triggers Menu: `"Triggers\u{2026}"`, one checked item per trigger (toggling writes the session's own `Triggers` with that entry's `disabled` flipped), separator, "Add Trigger\u{2026}", "Edit Triggers\u{2026}". Call Script Function: its label drawn on a rounded box in its background color, width = label + 12 points, not stretchable, click shows the milestone 11 refusal as a banner.
- [ ] **Step 3:** menus open at the component's left edge, below the bar when it is at the top, above when at the bottom. The two Edit items open Settings > Shortcuts on the Actions or Snippets tab.
- [ ] **Step 4:** run `scripts/xt test --lib statusbar` and build; commit `status bar menu components`.

### Task 5: Search Tool, Filter Tool, Progress Indicator, Composer stub

**Files:** Modify `src/statusbar_components.rs`, `src/app/statusbar_content.rs`, `src/app/statusbar_ui.rs`, `src/app/find_ui.rs` (Find and Filter with a bar), `src/app/mod.rs` (`build_frame` skips the in-pane find bar for a pane whose bar hosts it), `src/app/paste_ui.rs` (the progress bar moves onto the bar), `src/scene.rs` (`StatusBarDraw` gains `boxes: &[(Rect, Rgb, f32)]` drawn after the bands).

**Interfaces:**
- Consumes: `FindState` (query, results, `typing`, `filter`, `open`, `close`), `find_step(id, up)`, `find_key`, `show_find_mode_menu`, `refilter`, the paste queues' `progress()` and `cancel()`, `statusbar_layout::Item.mandatory`.
- Produces: `temporary(kind) -> Component` (knobs `{"base: priority": inf, "search: temporary": true}` or `"filter: temporary"`); `with_temporaries(components: &[Component], left: Option<Component>, right: Option<Component>) -> Vec<Component>` (left first; before a right one a Spring with `base: compression resistance` 1); `App::bar_find_host(pane) -> Option<Kind>`.

- [ ] **Step 1: failing tests** `temporaries_follow_iterm2` (order, the spring's knob, the temporary keys) and `temporary_search_stays_on_a_narrow_bar` (in `statusbar_components.rs`, through `statusbar_layout::layout`: a priority-infinity item with three priority-5 items on a 150 point bar under both algorithms keeps the infinity item and drops the others).
- [ ] **Step 2:** Search Tool: min 125, preferred 200, stretches, no icon. Drawn as a box (1 point border in the text color at half alpha) holding a magnifier glyph (mode menu), the query or the placeholder "Search" at half alpha, `Results::label`, and two chevrons (the left one calls `find_step(id, false)`, the right one `find_step(id, true)`: Find Next goes up in Tandem as in iTerm2); a close glyph only when temporary. A click in the box sets `typing` so keys take the find path. Filter Tool: the same with "Filter", no "Erase Search History" in its mode menu, always a close glyph; closing clears the filter.
- [ ] **Step 3:** Find with a bar shown for the pane: a Search Tool in the layout becomes mandatory while find is open; otherwise the temporary one is inserted left. Esc or closing find removes it. `filter:` with a bar: a Filter Tool in the layout takes focus; otherwise a temporary right one. The mode menu adds "Erase Search History", which clears the pane's query.
- [ ] **Step 4:** Progress Indicator: while a pane's paste shows progress and that pane's bar is shown, it is the temporary right component (winning over a temporary Filter) instead of the "Pasting... N%" info bar; min 125, preferred 200, stretches; a track with the filled fraction and a cancel glyph whose click cancels the paste.
- [ ] **Step 5:** Composer stub: min 200, preferred infinity, not stretchable, icon, an inert field with the placeholder "Command"; its click shows the stub message.
- [ ] **Step 6:** run `scripts/xt test --lib statusbar` and the find tests; commit `status bar search, filter, progress`.

### Task 6: The tmux status bar

**Files:** Modify `src/tmux_monitor.rs`, `src/tmux_controller.rs`, `src/statusbar_components.rs`, `src/app/tmux_tabs_ui.rs`, `src/app/prompted_ui.rs`, `src/app/statusbar_ui.rs` (the effective layout), `src/settings_ui.rs`, `schema/global.toml` (UseTmuxStatusBar's help loses "Takes effect with milestone 9h's tmux status bar.").

**Interfaces:**
- Consumes: `Monitor::new`, `update_once`, `Controller::install`, `uninstall`, `poll`, `next_due`, `version_known`.
- Produces: `tmux_monitor::sanitize_status(raw) -> String`, `tmux_monitor::not_ready(raw) -> bool`, `Monitor::set_interval(secs)`; `enum Side { Left, Right }`, `Key::Status(u32, Side)`, `Req::StatusInterval`, `Controller::watch_status(window, on, gw)`, `Action::StatusLine { window: u32, side: Side, value: String }`; `statusbar_components::tmux_layout(&Layout) -> Layout` and `uses_tmux_layout(use_tmux: bool, profile_layout: &Layout) -> bool`; `App.tmux_status: HashMap<(PaneId /*gateway*/, u32), (String, String)>`; session variables `tmuxStatusLeft`, `tmuxStatusRight`; `SettingsWindow::set_tmux_profiles(HashSet<String>)`.

- [ ] **Step 1: failing tests:**
  - `tmux_status_follows_iterm2` (in `tmux_monitor.rs`): `#[fg=red]a#[default] | b` sanitizes to `a | b`; `x <'git' not ready> y` to `x  y`; `not_ready` sees `' not ready>`.
  - `status_monitors_follow_iterm2` (in `tmux_controller.rs`, with the 8f fake gateway): on 2.9 `watch_status(1, true)` sends `display-message -p "#{status-interval}"` once and installs `#{T:status-left}` and `#{T:status-right}` on `@1` (subscriptions on 3.2, timers before); a reply of `5` sets both timers to 5 s, `0` sets 1 s; a value with a not-ready part schedules one extra poll of both after 0.01 s and a second not-ready value schedules none; `watch_status(1, false)` uninstalls; on 2.8 nothing is sent.
  - `tmux_layout_follows_iterm2` (in `statusbar_components.rs`): the profile's advanced configuration is kept and the components become tmux left (`expression` = `\(tmuxStatusLeft)`), a Spring with no knobs, tmux right (`\(tmuxStatusRight)`); `uses_tmux_layout` is true when the pref is on or the profile layout has no components.
- [ ] **Step 2:** implement the controller side. Values go through `sanitize_status`; the accelerated poll is a controller deadline folded into `next_due` and fired from `poll`, used once per controller (iTerm2's `_accelerated` never resets).
- [ ] **Step 3:** app side. A tmux tab whose session profile has "Status bar enabled" watches its window; turning it off, closing the window, or a tab changing tmux window re-watches. `Action::StatusLine` updates `tmux_status` and redraws. The bar of a tmux tab uses `tmux_layout` when `uses_tmux_layout(globals UseTmuxStatusBar, profile layout)` and the server is 2.9 or later; the tmux left and right pieces are Interpolated Strings with the tmux variants, the right one right-aligned. The layout is not written to a session profile (Tandem computes it when drawing).
- [ ] **Step 4:** Settings: when the profile being edited is a tmux session's own profile, "Status bar enabled" and "Configure Status Bar" are disabled (`PSV:569-573`).
- [ ] **Step 5:** run `scripts/xt test --lib tmux` and `statusbar`; commit `tmux status bar`.

### Task 7: Scrollback with an app status bar, parity, checklist

**Files:** Modify `crates/alacritty_terminal/src/term/mod.rs` and `grid/mod.rs`, `crates/alacritty_terminal/TANDEM-PATCHES.md` (patch 14), `src/screen.rs`, the pane restyle path that reads profile keys (`install_pane` and profile change), parity rows, create `docs/acceptance/m9h-checklist.md`.

**Interfaces:**
- Produces: `Term::set_scrollback_with_region(bool)` (default true); `Screen::set_scrollback_with_status_bar(bool)`.

- [ ] **Step 1: failing test** `scroll_regions_follow_scrollback_with_status_bar` in `src/screen.rs`: a 10 by 5 screen with `ESC[1;4r` and six lines fed: with the flag on, history grows; with it off, history stays 0 and the fifth row is untouched; with no region, history grows either way; a region not at the top never saves.
- [ ] **Step 2:** patch 14: in `scroll_up_relative`, when the flag is off and the region starts at the top but is not the whole screen, the region scrolls in place like one below the top (no history). Record it in TANDEM-PATCHES.md.
- [ ] **Step 3:** the profile key `Scrollback With Status Bar` (default true) reaches each pane's screen on start and on profile change.
- [ ] **Step 4:** parity: 03:476-480, 482, 483, 485, 486, 487, 491 have; 03:484 have with the job tree popover noted for the Jobs tool; 03:488 partial (the Composer milestone); 03:492 partial (milestone 11); 03:425 have; 03:445 note the Interpolated String reads `user.*`; 01:198 have; 02:168 have. The checklist has setup steps and one numbered check per component, the tmux bar, the scrollback rule, and one line per kept iTerm2 bug.
- [ ] **Step 5:** run `scripts/xt test --lib`; commit `scrollback with an app status bar` and `docs: m9h parity and checklist`.

---

## Resume notes (facts gathered before Task 1, from the iTerm2 source)

- Nothing of Task 1 is written yet. Start the ledger at `.superpowers/sdd/2026-10-04-m9h-status-bar-components/progress.md` with `task-start PLAN 1`. 9g Task 4 (setup window) must be committed first; it edits `src/statusbar.rs` and `src/app/statusbar_ui.rs`.
- The 9g extension point: `Piece` in `src/app/statusbar_ui.rs` (`of`, `text`, `copyable`, `click`, `item`) and `place_bar`, which measures text with `Renderer::text_width` and builds `statusbar_layout::Item`s. `Item` already has `icon`, `empty`, `mandatory`; priority `f32::INFINITY` is preallocated by Stable.
- Classes (identifier `com.iterm2.<class>`): `iTermStatusBarClockComponent`, `iTermStatusBarActionComponent` (Custom Action), `iTermStatusBarActionMenuComponent`, `iTermStatusBarSnippetMenuComponent`, `iTermStatusBarTriggersComponent`, `iTermStatusBarHostnameComponent`, `iTermStatusBarUsernameComponent`, `iTermStatusBarJobComponent`, `iTermStatusBarWorkingDirectoryComponent`, `iTermStatusBarSearchFieldComponent`, `iTermStatusBarFilterComponent`, `iTermStatusBarComposerComponent`, `iTermStatusBarSwiftyStringComponent` (Interpolated String), `iTermStatusBarFunctionCallComponent`, internal `iTermStatusBarProgressComponent`, `iTermStatusBarTmuxLeftComponent`, `iTermStatusBarTmuxRightComponent`.
- Knob keys: Clock `format` (Text, placeholder "Date Format (Unicode TR 35)", default "M/dd h:mm", label "Date Format:"), `localize` (Checkbox "Localize Date Format", default true), `ssh_offset` ("Show server time in SSH integration?", true), `ssh_tz` ("Use server time zone in SSH integration?", true). Host `abbreviate-localhost` ("localhost replacement", placeholder "Enter replacement text for localhost", default ""). Custom Action `action` ("Action", kind Action, an iTerm2 action dictionary, read with `UserAction::from_value`). Interpolated `expression` ("String Value:", placeholder `String with \(expressions)`). Call Script `label` ("Label:", placeholder "Button Label"), `function invocation` ("Function call:", placeholder `foo(bar: "baz")`), `timeout` ("Timeout (seconds):", 5). Temporaries `search: temporary`, `filter: temporary`. Shared keys from 9g: `shared text color`, `shared background color`, `shared font`, `base: priority`, `base: compression resistance`, `minwidth`, `maxwidth`.
- Displayed text: Actions Menu "Perform Action\u{2026}" (exemplar "Action\u{2026}"), Snippets Menu "Send Snippet\u{2026}" (exemplar "Snippet\u{2026}"), Triggers Menu "Triggers\u{2026}", Placeholder "Click here to configure status bar". Job exemplar `vim \u{25C2} bash`, Host exemplar `example.com`, git (9i) `\u{2387} main`.
- Icons (Segoe Fluent Icons; check each renders on EXTREME and swap for a close glyph if not): Clock E823, Custom Action and Actions Menu E945, Snippets E8C6, Triggers E7C1, Host E774, User E77B, Job E756, Directory E8B7, Composer E70F, Progress E77F. Search, Filter, Interpolated String, Call Script Function and tmux have none. Field glyphs: magnifier E721, chevrons E76B and E76C, close E711.
- Text rule (`iTermStatusBarTextComponent.m:178-229`): preferred = widest variant's width, minimum = narrowest's, draw = widest with `ceil(w) <= ceil(available)`, else "". Empty when the widest variant is "". Copy = widest variant. Only Job, Current Directory and git clamp preferred by the min and max knobs; max applies to all as `MAX(24, maxwidth)`.
- Corrections to the research, from the source:
  - Custom Action variants (`iTermStatusBarActionComponent.m:24-40`): RegexKitLite's `enumerateStringsSeparatedByRegex` visits the words between `\h+` runs, so the variants are every word prefix including the full title.
  - Host variants (`iTermStatusBarVariableBaseComponent.m:102-125`): the center forms come from `byTruncatingComposedCharactersInCenter:n` (`NSStringITerm.m:1876-1890`): prefix `(len - n) / 2` characters, U+2026, then the rest of the suffix; n runs from 2 while `n < len - 2`. This `variantsOf:` is in the shared base, so Current Directory gets it too when its path has a dot (then its own `/` compression). Host compresses the LAST label that is longer than one character; Directory the FIRST component.
  - Triggers Menu item titles (`PTYSession.m:23172-23180`): `"<action title without a trailing U+2026> U+2014 <regex>"`, checked when not disabled. Toggling writes the session's own profile (`PTYSession.m:4506-4512`).
  - The Actions Menu adds its separator before "Edit Actions\u{2026}" even with no actions (`iTermStatusBarActionComponent.m:202-226`); the component is empty then, so it shows only with "Remove components with empty values" off.
- Clock (`iTermStatusBarClockComponent.m`): single variant; stretches; cadence 1 s; with localize the format is a template (`dateFormatFromTemplate:options:0 locale:currentLocale`). Windows: `udatpg_open` with the user locale and `udatpg_getBestPattern` (ICU in `Win32_Globalization`, in Windows 10 1903 and later).
- User Name (`VAR:234-247`): always shows the full name; minimum = min(width of full, width of first character + U+2026).
- Job chain (`JOB:85-149`): deepest job first, parents after, stop at the session's root process; `[""]` before the first value, `["\u{2026}"]` when every name is empty. Tandem: `Shell.job.name` then `Shell.job_ancestors` after the first entry (they are lowercased, deepest first; check in `jobwatch.rs` whether the shell itself is included). The job tree popover on click is left to the toolbelt Jobs tool.
- Current Directory: a leading home prefix becomes `~` (plain string prefix, so remote paths too). Windows home is `%USERPROFILE%`; compare case-insensitively in both slash forms.
- Interpolated String (`iTermStatusBarSwiftyStringComponent.m`): single variant, empty when the value is empty, error logs `Error while evaluating \u{201C}%1$@\u{201D}:\n%2$@`, shows U+1F41E, and a click shows that message under the heading "Error". Tandem shows it as an info bar with OK.
- Search and Filter (`iTermStatusBarSearchFieldComponent.m`, `StatusBarFilterComponent.swift`, `SessionView.m:713-829`): Find with a status bar and a Search Tool in the layout sets `mustShowSearchComponent` (mandatory); without one it inserts the temporary left component with priority infinity; hiding find clears both. Filter with no Filter Tool visible sets the temporary right component. Mode items: "Smart Case Substring", "Case-Sensitive Substring", "Case-Insensitive Substring", "Case-Sensitive Regex", "Case-Insensitive Regex", then "Erase Search History" (Search only).
- Temporary right component (`iTermStatusBarViewController.m:248-293`): a Spring with compression resistance 1 goes before it. The Progress Indicator (`iTermStatusBarProgressComponent.m`) is the temporary right component during a slow paste (`Pasting/iTermPasteViewManager.m:70-89`); min 125, preferred 200, stretches, icon StatusBarIconPaste.
- Call Script Function (`iTermStatusBarFunctionCallComponent.m`): a borderless rounded NSButton sized to its label, bezel = background knob, not stretchable, vertical offset 0.5.
- tmux (`iTermTmuxStatusBarMonitor.m`, `iTermStatusBarLayout+tmux.m`, `PTYSession.m:10077-10091`): tmux 2.9 or later only; interval starts at 1, `display-message -p "#{status-interval}"` then `max(1, value)`; formats `#{T:status-left}` and `#{T:status-right}` on `@<window>`; sanitize removes regexes `<'.*?' not ready>` and `#\[.*?\]`; the accelerated poll starts at 0.01 s, doubles, and is used once. Left variants drop trailing `" | "` parts, right variants drop leading ones and right-align. Existing 8f pieces: `Key` enum and `install` at `src/tmux_controller.rs:77-82, 704-722`; `Monitor::new(target, format, subscribe, timer)`.
- Scrollback rule (`VT100/VT100Grid.m:978-1030`): with a scroll region, a line leaving the top goes to scrollback only when the region's top is row 0, the flag is on, and there is no column region. alacritty today always saves when the region starts at the top (`grid/mod.rs:277`), which is the flag-on behavior. Profile key `Scrollback With Status Bar`, default true, at `schema/profile.toml:1848`.
- Settings > Shortcuts has the Actions and Snippets tabs (`src/settings_shortcuts.rs`, `Tab`); triggers open with `run_trigger_command_menu("editTriggers:")` and `("addTrigger:")` in `src/app/triggers_ui.rs`.
