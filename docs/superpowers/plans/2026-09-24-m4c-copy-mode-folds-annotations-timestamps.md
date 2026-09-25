# Tandem Milestone 4c: Copy Mode, Folds, Annotations, Timestamps, and the Edit Tools

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the rest of iTerm2's milestone 4 rows: Copy Mode (its key table, counts, the three selection kinds, Shift+arrow auto entry, key action 73), the selection move actions 45 to 48, Clear to Start of Selection, Selection Respects Soft Boundaries, Replace Selection (pretty JSON, Base64 encode and decode), Fold Selected Lines and Fold All, annotations (add, annotate selection, next and previous, show and hide, OSC 1337 AddAnnotation), the session note, per-line timestamps, the cursor guide, Find Cursor, and the Named Marks submenu.

**Architecture:** Two more alacritty_terminal patches: per-row `time`, `fold`, and `note` slots, and `Grid::splice` (replace a run of rows, returning the removed ones). Folding and Replace Selection edit the buffer through `Screen::splice`, as iTerm2 does (its `replaceRange:withLines:`). Pure modules decide: `copymode.rs` (keys to moves), `stamps.rs` (timestamp text), `fold.rs` (summary text), `annotations.rs` (the notes on a screen), `seltools.rs` (JSON, Base64, endpoint moves, soft dividers). The app draws and routes keys.

**Tech Stack:** as milestone 4b.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` Edit and View rows tagged milestone 4, Edit > Copy Mode, the annotations and fold rows, OSC 1337 AddAnnotation; `01-profile-settings.md` cursor guide and timestamps rows; `02-global-settings.md` Enter Copy Mode, EnterCopyModeAutomatically.

## Global Constraints

- Everything from milestones 0 to 4b still holds. iTerm2 facts are the research in the ledger (iTermCopyModeHandler.m, iTermCopyModeKeyBindings.dict, PTYTextView.swift 592-771 and 1152-1286, PTYAnnotation, iTermTimestampDrawHelper.m, iTermFindCursorView.m, SelectionReplacement.swift, iTermTextExtractor.m 2212-2253).
- Cmd is Win and Option is Alt. Menu chords come from `schema/menus.toml`.
- Nothing in this milestone sends text to the shell except Copy Mode's paste-free copy (none) and Replace Selection (none): replace and fold edit only Tandem's buffer, as in iTerm2.

## Review Focus

1. A fold, then output that scrolls the buffer, then unfold. Expect: the lines come back in place, marks on them intact. Test: Task 5 `unfold_restores_lines_after_scrolling`.
2. Copy Mode at the top and bottom of a 10,000-line buffer with counts (`999k`, `G`, `g`). Expect: the cursor stops at the ends, no panic. Test: Task 3 `moves_clamp_at_the_buffer_ends`.
3. Replace Selection over wide characters and wrapped lines. Expect: whole logical lines only, no split wide character. Test: Task 2 `replacement_needs_whole_lines`.
4. Timestamps after reflow (narrow then widen). Expect: each line keeps its own time. Test: Task 1 `row_slots_survive_reflow`.
5. An annotation on a line that scrolls into history and the note window. Expect: the underline stays on its text; Next Annotation finds it. Test: Task 6 `annotations_follow_their_rows`.

---

### Task 1: Row slots and splice (alacritty patches 5 and 6)

**Files:** `crates/alacritty_terminal/src/grid/row.rs`, `grid/resize.rs`, `grid/mod.rs`, `TANDEM-PATCHES.md`; `src/screen.rs`.

**Interfaces:** `Row::{time, set_time, fold, set_fold, note, set_note}` (u32, cleared by `reset`, moved on reflow like `mark`); `Grid::splice(first: Line, last: Line, with: Vec<Row<T>>) -> Vec<Row<T>>` (rows above slide down into the screen when fewer come back; the cursor's row keeps its place). `Screen::stamp_rows(now: u32)` after each feed marks every row the output touched; `Screen::row_time(line) -> u32`; `Screen::splice(first: i32, last: i32, lines: &[String]) -> Vec<Row<Cell>>` and `Screen::restore(at: i32, rows: Vec<Row<Cell>>)`.

- [ ] Failing tests: `output_stamps_the_rows_it_writes`, `row_slots_survive_reflow`, `splice_replaces_rows_and_keeps_the_cursor_row`. Implement. Commit `Add row time, fold, and note slots and Grid::splice`.

### Task 2: Selection tools

**Files:** Create `src/seltools.rs`; modify `src/app/*`, `src/commands.rs`, `src/keyaction.rs`.

**Interfaces:** `pretty_json(text) -> Result<String, String>`, `base64_encode(text)`, `base64_decode(text) -> Result<String, String>`; `eligible(sel_start_col, sel_end_is_line_end) -> bool` (whole lines from column 0); `move_endpoint(sel: (Pos, Pos), which: End, left: bool, n: usize, cols: usize) -> (Pos, Pos)`; `divider_columns(lines: &[String], row: usize) -> Vec<usize>` (`|`, U+2502, U+251C, U+2524 repeated in the same column for 8 rows). Commands: Replace Selection x3 (buffer splice, not sent to the shell), Render Selection Natively is Later(5); Clear to Start of Selection; Selection Respects Soft Boundaries (a global check item; word, smart, and drag selection stop at a divider); key actions 45 to 48 (parameter = count).

- [ ] Failing tests: `json_is_pretty_printed`, `base64_round_trips`, `replacement_needs_whole_lines`, `endpoint_moves_wrap_rows`, `dividers_need_eight_rows`. Commit `Add the selection tools`.

### Task 3: Copy Mode engine

**Files:** Create `src/copymode.rs`.

**Interfaces:** `State { cursor: Pos, anchor: Option<Pos>, kind: Kind (Char, Line, Box), count: usize }`; `enum Outcome { Moved, Copy, Exit, Find, None }`; `fn key(&mut self, k: &str, ctx: &Ctx) -> Outcome` where `Ctx` gives the buffer's rows, columns, top visible row, screen lines, mark rows, and line text for word moves. iTerm2's table: h j k l and arrows, w b W B, 0 ^ $, g G Home End, H M L, Ctrl+B F U D, Ctrl+Y E, [ ], v Space V Ctrl+V Ctrl+Space, o, y Ctrl+K, q Esc, Ctrl+C Ctrl+G, /, counts 1-9 (0 continues a count). `auto_enter(key, mods) -> Option<bool /*backward*/>` for Shift(+Alt/Ctrl/Win) with Left Up Home PageUp or Right Down End PageDown. `parse_vim_keys(s) -> Result<Vec<String>, String>` for key action 73 (`<C-v>`, `<Esc>`, `<CR>`, plain characters).

- [ ] Failing tests: `hjkl_and_counts_move`, `moves_clamp_at_the_buffer_ends`, `word_moves_follow_vi`, `v_V_and_ctrl_v_select`, `o_swaps_the_ends`, `y_copies_and_exits`, `shift_arrows_enter_copy_mode`, `vim_key_strings_parse`. Commit `Add the Copy Mode engine`.

### Task 4: Copy Mode in the window

**Files:** Create `src/app/copymode_ui.rs`; modify `src/app/mod.rs`, `src/scene.rs`, `src/commands.rs`, `src/keyaction.rs`, `schema/global.toml` (`EnterCopyModeAutomatically` bool true).

**Interfaces:** Edit > Copy Mode toggles; keys go to the engine while on; the view follows the cursor; the selection mirrors `State` (Box = alacritty Block); the cursor draws as iTerm2's flag (a triangle in the lower third of the cell, white, light blue while selecting). Key action 73 runs its key string through the engine. Shift+arrow with a selection enters Copy Mode when the setting is on.

- [ ] Scene test `copy_mode_cursor_is_a_flag`. Commit `Add Copy Mode`.

### Task 5: Folds

**Files:** Create `src/fold.rs`; modify `src/screen.rs`, `src/app/*`.

**Interfaces:** `summary(first: &str, last: &str, n: usize, cols: usize) -> String` (first line prefix, ` …N lines… `, last line prefix, fits `cols`); `fold_all_caption(first_cmd_time, first_cmd, last_cmd, n, cols)`. `Screen::fold(first, last) -> Option<u32>` (refused over the cursor row or the alternate screen), `Screen::unfold(id)`, `Screen::fold_at(line) -> Option<u32>`; the summary row draws faint italic. Edit > Fold Selected Lines toggles (unfolds when the selection holds a fold); Fold All folds everything above the cursor with the command caption; a click on a summary row unfolds; the right-click menu offers Unfold there.

- [ ] Failing tests: `summary_fits_the_width`, `fold_then_unfold_is_lossless`, `unfold_restores_lines_after_scrolling`, `fold_refuses_the_cursor_row`. Commit `Add folds`.

### Task 6: Annotations

**Files:** Create `src/annotations.rs`, `src/note_ui.rs`; modify `src/screen.rs`, `src/osc.rs`, `src/scene.rs`, `src/app/*`.

**Interfaces:** `Notes { by_id: BTreeMap<u32, Note { text, start_col, end_col, hidden }> }` on `Screen`, anchored by the row `note` slot; `Screen::add_note(line, cols, text, visible) -> u32`, `notes_in_order()`, `remove_note(id)`. Add Annotation at Cursor (the Smart Selection match at, before, or after the cursor, else the whole line), Annotate Selection, Next and Previous Annotation, Show Annotations (Win+\\: hide all when any shows, else show all), OSC 1337 `AddAnnotation=` and `AddHiddenAnnotation=` (text, or `length|text`, or `text|length|x|y` as iTerm2 parses them). Annotated text gets a 1 px yellow underline 2 px above the row bottom. A note opens in a small egui window with the text (edit saves; Delete asks "Really remove annotation?").

- [ ] Failing tests: `annotations_follow_their_rows`, `osc_add_annotation_forms`, `next_annotation_wraps`. Commit `Add annotations`.

### Task 7: Timestamps, cursor guide, Find Cursor

**Files:** Create `src/stamps.rs`; modify `src/scene.rs`, `src/app/*`, `src/commands.rs`.

**Interfaces:** `stamps::label(then, now) -> String` (under a day `HH:MM:SS`, under 6 days `Ddd HH:MM:SS`, under 180 days `Mmm D HH:MM:SS`, else `YYYY Mmm D HH:MM:SS`, local time); View > Show Timestamps (Win+Shift+E, per session, starts from the profile's Timestamps Visible) draws right-aligned labels over each row (Overlap), only while the pointer is over the right edge (Hover), or in a reserved right column (Adjacent). Cursor guide: the profile's Use Cursor Guide and Cursor Guide Color (with alpha) fill the cursor row; View > Show Cursor Guide toggles it per session. View > Find Cursor (Win+/): a dim overlay over the window with a clear circle (30 px radius) at the cursor, for 1 s after Win comes up.

- [ ] Failing tests: `labels_scale_with_age`, scene `cursor_guide_fills_the_cursor_row`, `find_cursor_leaves_a_hole`. Commit `Add timestamps, the cursor guide, and Find Cursor`.

### Task 8: Session note and Named Marks

**Files:** Modify `src/note_ui.rs`, `src/app/*`, `src/app/menu.rs`.

**Interfaces:** View > Show Session Note (Win+Alt+G): a per-pane egui window titled "Session Note" with the pane's note text (kept while Tandem runs; arrangements save it in milestone 7). Edit > Named Marks: the submenu is rebuilt from the focused pane's named marks when marks change; picking one scrolls to it ("Unnamed Mark" for an empty name).

- [ ] Pure test `named_marks_sort_by_row`. Commit `Add the session note and the Named Marks menu`.

### Task 9: Parity and acceptance

- [ ] Update the rows 4c closes, recount `00-roadmap.md`, write `docs/acceptance/m4c-checklist.md`. Commit.
