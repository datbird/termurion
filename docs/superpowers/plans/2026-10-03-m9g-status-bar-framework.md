# Tandem Milestone 9g: The Status Bar Framework and Its Setup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's status bar: a profile with "Status bar enabled" shows a bar of components, one per window (the current session's) at the top or bottom of the tab area (`StatusBarPosition`), or one per pane with "Separate status bars per pane"; the profile's `Status Bar Layout` (iTerm2's dictionary: components with their knobs, and the advanced configuration) is laid out by iTerm2's Stable or Tightly Packed algorithm with its margins, priorities, minimum and maximum widths, and spring constants; the bar draws its background, component background bands, separators, the edge separator, and auto-rainbow colors; an empty layout shows "Click here to configure status bar"; right-click gives Copy, Configure/Hide component, Configure Status Bar, Disable Status Bar, and checkbox knobs; Profiles > Session has "Status bar enabled" and "Configure Status Bar", which opens iTerm2's setup window (palette, Active Components with drag, drop, reorder and delete, Configure Component with every knob type, Auto-Rainbow, Advanced with colors, font, and the three checkboxes, live preview with Cancel restoring the layout). This milestone's components are the Spring (Empty Space) and the Fixed-size Spacer; the others arrive in milestone 9h.

**Architecture:** `statusbar.rs` is the pure model: the layout codec (unknown keys and component configurations kept), the advanced configuration, the base knobs (priority, compression resistance or size multiple, minimum and maximum width, shared colors and font), knob definitions by type, the component registry (descriptions, exemplars, knobs, the framework components), Hide, and auto-rainbow colors. `statusbar_layout.rs` is iTerm2's layout algorithms over plain item sizes, plus separator offsets and background bands. The app keeps the window's or each pane's bar area out of the terminal, draws it with the scene, routes clicks and the context menu, and the setup window is an egui window (`statusbar_setup_ui.rs`).

**Tech Stack:** as milestone 9f.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` section 1 and section 2.20, 2.21 (spacer, spring). Rows: `01-profile-settings.md` Show Status Bar, Status Bar Layout; `02-global-settings.md` StatusBarPosition, SeparateStatusBarsPerPane, statusBarHeight; `03-features-and-menus.md` the status bar rows for the framework, the layout algorithms, the setup UI, auto-rainbow, the context menu.

## Global Constraints

- Everything from milestones 0 to 9f still holds.
- Heights and widths are iTerm2's points times the window's scale: bar height `statusBarHeight` (21), margins 10 (left 6), icon 17, separator inset 3, fixed spacer default 5.
- Status bar text uses the prose font at 12 pt scaled; a layout or component font sets the size (the family stays the prose font).
- Colors are Tandem's `{r, g, b, a}` tables (the importer already turns iTerm2 color dictionaries into them).
- No macOS vibrancy: the bar's background is the tab bar's color unless the layout sets one.
- tmux tabs share the window's bar (never per pane); the tmux status bar components and `UseTmuxStatusBar` are milestone 9h.

## Review Focus

1. Stable with three text components and a narrow bar: the lowest priority goes first; springs before anything. Test: Task 2 `stable_layout_follows_iterm2`.
2. Tightly Packed with a spring between two components: the components get their preferred widths and the spring the rest. Test: Task 2 `tight_layout_follows_iterm2`.
3. A layout imported from iTerm2 with a component class Tandem does not know: kept in the file, skipped on screen. Test: Task 1 `layouts_round_trip_like_iterm2`.
4. Hide on a spring removes every spring. Test: Task 1.
5. Separators: none after the last component, none on a fixed spacer's right (iTerm2's dead branch), one per boundary. Test: Task 2 `separators_and_bands_follow_iterm2`.

---

### Task 1: The model

**Files:** Create `src/statusbar.rs`; add to `src/lib.rs`.

**Produces:** `Layout { components: Vec<Component>, advanced: Advanced, extra: toml::Table }`, `Component { class, knobs: toml::Table, config: toml::Table }`, `Advanced { separator_color, background_color, default_text_color, algorithm: Algorithm, remove_empty, edge_separator, font: Option<String>, rainbow: Rainbow }`; `Layout::from_value(Option<&Value>)`, `to_value()`; `Component` base readers `priority()`, `spring_constant(algorithm)`, `min_width_knob()`, `max_width_knob()`, `text_color()`, `background_color()`, `font()`; `KnobKind { Checkbox, Text, Double, Color, Action, Invocation, Font }`, `Knob { label, kind, key, placeholder, default }`; `Info { class, short, detail, exemplar, stretch, margins, internal }` with `registry() -> Vec<Info>`, `info(class)`, `knobs(class, algorithm) -> Vec<Knob>`; `hide_class(&mut Layout, class)`; `rainbow_colors(count, style, dark_background) -> Vec<(f32, f32, f32)>`.

- [ ] Failing tests `layouts_round_trip_like_iterm2`, `base_knobs_follow_iterm2`, `framework_components_follow_iterm2`, `rainbow_follows_iterm2`.
- [ ] Implement. Commit `status bar model`.

### Task 2: Layout algorithms

**Files:** Create `src/statusbar_layout.rs`; add to `src/lib.rs`.

**Produces:** `Item { min, max, preferred, stretch, spring: f32, margins, priority, kind: ItemKind { Normal, Spring, FixedSpacer }, icon, empty, mandatory }`; `Placed { index, x, width }`; `layout(items, width, algorithm, remove_empty) -> Vec<Placed>`; `separators(&[Placed], &[Item]) -> Vec<f32>`; `bands(&[Placed], &[Item], width) -> Vec<(f32, f32, usize)>`.

- [ ] Failing tests `tight_layout_follows_iterm2`, `stable_layout_follows_iterm2`, `separators_and_bands_follow_iterm2`.
- [ ] Implement. Commit `status bar layout algorithms`.

### Task 3: The bar in the app

**Files:** Create `src/app/statusbar_ui.rs`; modify `src/app/mod.rs` (`content_area` and pane rectangles leave the bars' room; drawing; clicks; right-click menu), `src/scene.rs` (`status_bar`), `schema/global.toml` (`StatusBarPosition`, `SeparateStatusBarsPerPane`, `statusBarHeight`, `UseTmuxStatusBar` saved for 9h), `src/settings_ui.rs` (Profiles > Session: "Status bar enabled" and "Configure Status Bar").

- [ ] Wire; a pure test `bar_rects_follow_the_position` in `statusbar_layout.rs` for where the shared and per-pane bars go. Commit `status bar`.

### Task 4: The setup window

**Files:** Create `src/statusbar_setup_ui.rs`; modify the app to open it from Profiles > Session and the context menu, apply every change live to the profile, and restore on Cancel.

- [ ] Wire; pure helpers `insert_component`, `move_component`, `knob_values` in `statusbar.rs` with test `setup_edits_follow_iterm2`. Commit `status bar setup` and `docs: m9g parity and checklist` (parity rows, `docs/acceptance/m9g-checklist.md`).

---

## Resume notes (facts gathered before Task 1, from the iTerm2 source)

- Nothing of Task 1 is written yet. The ledger at `.superpowers/sdd/2026-10-03-m9g-status-bar-framework/progress.md` has only the pre-flight line; start with `task-start PLAN 1`.
- Component configuration: `{"class": <ObjC name>, "configuration": {"knobs": {...}, "layout advanced configuration dictionary value": {...}}}`. iTerm2 drops a component with no `configuration`; Tandem keeps an unknown class in the file (the 9h classes) and skips it on screen.
- Computed default layout: `{"components": [], "advanced configuration": {"algorithm": 0, "remove empty components": false, "draw separator between status bar and terminal": false, "auto-rainbow style": 0}}`.
- Base knob keys and defaults (`StatusBar/Components/iTermStatusBarBaseComponent.m`): `"base: priority"` 5; `"base: compression resistance"` 1, read as `max(0.01, v)`; `"maxwidth"` default infinity, read as `max(24, v)`; `"minwidth"` 0. Base knobs: stretchable components get "Compression Resistance:" (Tightly Packed) or "Size Multiple:" (Stable), then "Priority:"; others only "Priority:". Min/max knobs are "Minimum Width:" then "Maximum Width:". Default knobs: `{compression resistance: 1, priority: 5}`.
- Spring (`iTermStatusBarSpringComponent`): keys `iTermStatusBarSpringComponentSpringConstantKey` (tight, default 0.01) and `iTermStatusBarSpringComponentSizeMultipleKey` (stable, default 1); spring constant `max(0.01, knob or 1)`; knobs [Compression Resistance: or Size Multiple: (Double), "Color" (Color, key `shared background color`)]; min 0, preferred infinity, stretch, no margins; short "Spring"/"Empty Space"; exemplar `║〜〜║` / seven spaces; details in section 1.5 of the research.
- Fixed-size Spacer (`iTermStatusBarFixedSpacerComponent`): key `iTermStatusBarFixedSpacerComponentWidthKnob`, width `knob or 5` (0 also means 5); knobs ["Color" (Color), "Width" (Double, 5), "Priority:"]; exemplar `╠══╣`; "Adds a fixed amount of space".
- Placeholder (`iTermStatusBarPlaceholderComponent`): "Click here to configure status bar", stretchable, internal, never empty, click opens the profile's status bar settings.
- Text measurement: `Renderer::text_width(text, prose, scale)` (src/render.rs) measures prose-font text; porthole_ui.rs shows how to call it through the GPU renderer.
- Colors: Tandem stores colors as `{r, g, b, a}` tables (src/import.rs converts iTerm2 color dictionaries).
- Profile settings UI: `Status Bar Layout` is a `dict` setting that settings_ui.rs shows as "N item(s)" (the Kind::Dict arm); Task 3 replaces that with "Status bar enabled" plus a "Configure Status Bar" button.
- Rendering: popup boxes go in `Frame.popups` with `scene::cut_out`; `overlay` draws after all text.
