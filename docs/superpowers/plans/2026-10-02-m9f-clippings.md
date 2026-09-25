# Tandem Milestone 9f: Clippings

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's Clippings: each session keeps a list of clippings (a title and a Markdown detail) and an archive of up to 50 earlier lists; View > Show Clippings (Shift+Win+=, checked) shows them in a panel on the right of the session (300 px by default, its width dragged between 225 and 1200 and kept in `NoSyncGutterPanelWidth_com.iterm2.clippings`); the panel has Add (the "New Clipping" sheet), Remove, Archive, Send (the selected clippings as `**title**\ndetail`, joined by `clippingSeparator`, through the paste path), Copy, the archive arrows with "N/M", a close button, double-click to paste one detail, Delete to remove, Space for a preview, and drag to reorder; the clippings go into window arrangements and restored sessions under iTerm2's keys.

**Architecture:** `clippings.rs` is the pure model: the list, the archive and the viewed index, adding (which shows the panel), removing, archiving, stepping, moving, the enabling rules, the send and copy text, the width clamp, the arrangement codec, and the panel's layout (rows, heights, buttons) with its hit test. The app keeps one model per pane, takes the panel's width out of the pane's grid rectangle, draws the panel with the scene, routes pointer and key events to it, and opens the New Clipping sheet as an egui window like the other prompts.

**Tech Stack:** as milestone 9e (`pulldown-cmark` for the detail's plain text).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` section 5. Rows: `03-features-and-menus.md` View > Show Clippings; `02-global-settings.md` `clippingSeparator`.

## Global Constraints

- Everything from milestones 0 to 9e still holds.
- Arrangement keys: `Clippings` (array of `{type, title, detail}`), `Clippings Visible`, `Clippings Archive` (array of arrays), `Clippings View Index` (-1 is the live list).
- `clippingSeparator` is Vim syntax, default `\n--\n`.
- The detail is drawn as plain text from its Markdown (emphasis and code marks removed); rows cap at 100 px of detail.
- The `iterm2.add_clipping` and `iterm2.archive_clippings` functions come with the scripting API (milestone 11); until then the Add button is the only source.
- Workgroup peers do not exist in Tandem; each session owns its clippings.

## Review Focus

1. Send with two clippings, one with a blank title: `**A**\none` + separator + `two`. Test: Task 1 `clippings_format_like_iterm2`.
2. The 51st archive: the oldest goes, and viewing stays on a valid index. Test: Task 1 `archives_keep_fifty`.
3. While viewing an archive: Add, Remove, Archive are off; Send and Copy work on the archived rows. Test: Task 1 `buttons_follow_the_view`.
4. A pane with the panel shown is split: both halves size their grids without the panel's width. Test: Task 2 `panel_takes_its_width`.
5. An arrangement with a view index past the archive: the live list shows. Test: Task 1 `clippings_round_trip_arrangements`.

---

### Task 1: The model

**Files:** Create `src/clippings.rs`; add to `src/lib.rs`.

**Produces:** `Clipping { kind, title, detail }`; `Clippings { live, archive, view: Option<usize>, visible }` with `add(Clipping)`, `remove(&[usize])`, `archive_all()`, `step(back: bool)`, `shown() -> &[Clipping]`, `move_row(from, to)`, `enabled() -> Enabled { add, remove, archive, send, back, forward }` (given a selection count), `status() -> Option<(String /*N/M*/, String /*tooltip*/)>`, `to_value() -> toml::Table`, `from_value(&toml::Table) -> Clippings`; `format(&[&Clipping], separator: &str) -> String`; `plain(markdown) -> String`; `clamp_width(f32) -> f32`.

- [ ] Failing tests `clippings_format_like_iterm2`, `archives_keep_fifty`, `buttons_follow_the_view`, `clippings_round_trip_arrangements`.
- [ ] Implement. Commit `clippings model`.

### Task 2: The panel layout

**Files:** Modify `src/clippings.rs`.

**Produces:** `Layout { rows: Vec<Rect>, buttons: Vec<(Button, Rect)>, status: Option<Rect>, resize: Rect }`, `layout(panel: Rect, rows: &[(f32 title_h, f32 detail_h)], scroll: f32, scale: f32) -> Layout`, `enum Button { Add, Remove, Archive, Send, Copy, Back, Forward, Close }`, `enum Hit { Row(usize), Button(Button), Resize, Nothing }`, `hit(&Layout, x, y) -> Hit`, `split_rect(pane: Rect, visible: bool, width: f32) -> (Rect /*terminal*/, Option<Rect> /*panel*/)`.

- [ ] Failing tests `panel_takes_its_width`, `panel_hits_rows_and_buttons`.
- [ ] Implement. Commit `clippings panel layout`.

### Task 3: App wiring, parity, acceptance

**Files:** Create `src/app/clippings_ui.rs`; modify `src/app/menu.rs` (`visible` gives terminal rects), `src/scene.rs` (the panel), the pointer and key paths, `src/app/menu.rs` (`toggleShowClippings:` and its check), `src/commands.rs`, the arrangement and restore code (the four keys), `schema/global.toml` (`NoSyncGutterPanelWidth_com.iterm2.clippings`), parity rows, `docs/acceptance/m9f-checklist.md`.

- [ ] Wire. Commit `clippings panel` and `docs: m9f parity and checklist`.
