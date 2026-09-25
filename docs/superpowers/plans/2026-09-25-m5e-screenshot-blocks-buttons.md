# Tandem Milestone 5e: Make Screenshot, and OSC 1337 Block, UpdateBlock, and Button

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's Session > Make Screenshot... (a frozen copy of the pane, a panel with a minimap and line range, Redact and Highlight annotations, blur or solid redaction, Copy to Clipboard, Save, Save As, the 30000 px split) and the OSC 1337 command-block sequences: `Block` (start and end, a gutter bar, hover buttons to copy and fold), `UpdateBlock` (fold, unfold), and `Button` (an inline copy button and custom buttons that report `CSI ? 1337 ; code ~`).

**Architecture:** A pure `screenshot.rs` decides the line range, annotation labels and rectangles, the parts a tall image splits into, and file names; `shotimage.rs` does the pixel work (blur and fill in rectangles, the highlight outline and glow, the minimap). The renderer draws a range of a frozen copy of the pane offscreen in batches and reads the pixels back. `app/screenshot_ui.rs` swaps the frozen copy into the pane (as Filter and Zoom views do) and runs the panel, an egui window. Blocks: an alacritty_terminal patch adds a per-row block slot stamped while a block is open (as the row time is); `blocks.rs` parses the sequences and finds a block's rows; the scene draws the gutter bar, the hover buttons, and inline buttons; folding uses Tandem's fold engine with iTerm2's summary.

**Tech Stack:** as milestone 5d.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` Session > Make Screenshot..., OSC 1337 Block / UpdateBlock, Button. iTerm2 facts: `m5d-iterm2-facts.md` sections 1 and 3.

## Global Constraints

- Everything from milestones 0 to 5d still holds.
- The screenshot is of a frozen copy: output keeps going to the live screen underneath; the copy shows in the pane with a camera sign until the panel closes.
- Output pixels: the pane's full width with its side padding, no top or bottom margin, the window's scale, opaque (no transparency), no cursor, selection, find highlights, marks, or timestamps; background images laid out over the whole image's height.
- `render=1` (rich documents in place of a block's lines) and Edit > Render Selection Natively wait for milestone 5f (portholes).

## Review Focus

1. A 20000-line buffer, the whole range chosen. Expect: the image splits into parts under 30000 px, Copy is off, Save writes every part; memory stays bounded by one batch. Test: Task 1 `parts_split_tall_images`.
2. A redaction that spans a render batch edge. Expect: every covered cell is blurred (a faint seam is acceptable, as in iTerm2). Test: Task 2 `blur_stays_inside_rects`.
3. A Block that never ends, and an id reused after its block ended. Expect: no panic; hover and copy use the newest block with that id. Test: Task 5 `blocks_tag_rows_and_reuse_ids`.
4. A block's rows scrolled out of the buffer while folded. Expect: unfold does nothing; no panic. Test: Task 6 `folded_block_that_scrolled_out`.
5. A custom button clicked when the terminal is untrusted. Expect: nothing is sent. Test: Task 7 `custom_button_clicks_report_when_trusted`.

---

### Task 1: Screenshot decisions

**Files:** Create `src/screenshot.rs`.

**Interfaces:** `clamp_range(start, end, total) -> (usize, usize)`; `label(kind: Redact | Highlight, lines: &[String]) -> String` (iTerm2's `N lines`, `N lines: “text”` cut to 17 plus `...` past 20, `N lines: “first 10”...“last 10”`); `rects(sel: Selection spans, range, cell, side_margin) -> Vec<Rect>` (one per line, box selections one rect, clipped to the range); `parts(lines, line_px, width_px, max_px = 30000) -> Vec<Range<usize>>` (a part at most `max_px` tall and `max_px^2 / 5` pixels); `file_name(local time) -> "Tandem-Screenshot-yyyy-MM-dd-HH-mm-ss.png"` (iTerm2's pattern with Tandem's name); `part_name(base, i, n)`.

- [ ] Failing tests: `range_clamps`, `labels_follow_iterm2`, `redaction_rects_per_line`, `parts_split_tall_images`, `names_have_the_time`. Commit `Add screenshot decisions`.

### Task 2: Screenshot pixels

**Files:** Create `src/shotimage.rs`.

**Interfaces:** `blur_rects(rgba, w, h, rects, radius)` (Gaussian over the whole image with clamped edges, copied into the rectangles only); `fill_rects(rgba, w, h, rects, color)`; `highlight(rgba, w, h, rects, dark_bg)` (a 3 px yellow outline around each group and a 20 px glow, white on a dark background and black on a light one, never inside the group); `minimap(lines: &[image rows], out_w, out_h)` (a line per output row, the middle line of each bucket past that).

- [ ] Failing tests: `blur_stays_inside_rects`, `solid_fill`, `highlight_leaves_the_inside`, `minimap_samples_lines`. Commit `Add screenshot image operations`.

### Task 3: Rendering a line range

**Files:** Modify `src/render.rs` (a public offscreen read-back), `src/scene.rs` (a screenshot mode that leaves out the cursor, selection, and highlights), create `src/app/screenshot_ui.rs`.

**Interfaces:** `Renderer::draw_offscreen(frame, w, h) -> Vec<u8>` (RGBA, opaque); `App::render_range(pane, first, last) -> Result<(u32, u32, Vec<u8>), String>` in batches of 1000 lines from the frozen copy, each over its slice of the background image.

- [ ] GPU test `renders_a_range_offscreen` (two batches join without a gap). Commit `Render a line range offscreen`.

### Task 4: The Make Screenshot panel

**Files:** Modify `src/app/screenshot_ui.rs`, `src/app/menu.rs`, `src/commands.rs`, `src/scene.rs` (the camera sign).

**Interfaces:** Session > Make Screenshot... (beeps with no session): the frozen copy replaces the pane's screen (with all scrollback, cursor hidden); an egui panel "Make Screenshot" beside the window with the minimap and range, line fields (1-based), "Annotations:" with Redact Selection and Highlight Selection (from the pane's selection; the selection clears), the list with remove and Clear All, "Redaction method:" Blur (radius 1-200, 8) or Solid Color (black), Cancel, Copy to Clipboard (PNG and DIB; "Copied!" for 1.2 s; off when the image splits), Save As... (PNG), Save (`screenshotSaveLocation`, else the Desktop; the panel closes; Explorer shows the file). Closing the panel puts the live screen back.

- [ ] Pure test `panel_state_follows_selection` (the annotation buttons need a selection; adding one clears it). Commit `Add Make Screenshot`.

### Task 5: Block model (alacritty patch 10)

**Files:** `crates/alacritty_terminal` (Row `block` slot, stamped by `write_at_cursor` and line feed while `Term.block` is set), create `src/blocks.rs`, modify `src/screen.rs`, `src/marks.rs`.

**Interfaces:** `parse(value) -> Vec<(String, String)>` (`;` pairs, first `=`, last duplicate wins, pairs without `=` dropped); `Block=id=..;attr=start[;type=..]` opens a block (ignored on the alternate screen) and gives it a number; `attr=end` closes it; `Screen::block_rows(id) -> Option<(i32, i32)>` (the newest block with that id, from its tagged rows); `UpdateBlock=id=..;action=fold|unfold`.

- [ ] Failing tests: `block_args_parse`, `blocks_tag_rows_and_reuse_ids`, `blocks_ignore_the_alternate_screen`. Commit `Add the block model`.

### Task 6: Block drawing, hover buttons, fold

**Files:** Modify `src/scene.rs`, `src/app/*`.

**Interfaces:** A gutter bar in the left padding on block rows (filled; an outline when folded; red on hover for blocks of 3 or more rows); on hover, a pill at the right edge with fold (rightmost, blocks of 3 or more rows) and copy, stuck to the top of the view when the block's top scrolled off; copy puts the block's plain text on the clipboard with a "Copied" toast; fold uses the fold engine; a click on a folded block's line unfolds it; UpdateBlock folds and unfolds.

- [ ] Failing tests: scene `block_rows_get_a_gutter_bar`, `hover_pill_sticks_to_the_top`, `folded_block_that_scrolled_out`. Commit `Draw blocks and their buttons`.

### Task 7: Inline buttons

**Files:** Modify `src/blocks.rs`, `src/screen.rs`, `src/scene.rs`, `src/app/*`.

**Interfaces:** `Button=type=copy;block=<id>` and `Button=type=custom;code=<n>;icon=<name>` put a button at the cursor and write 4 spaces; a custom button with no valid code or icon makes every custom button inert (drawn at 40%); icons map iTerm2's SF Symbol names to Segoe Fluent Icons (an unknown name draws nothing); a click on a copy button copies its block; on a custom button sends `ESC [ ? 1337 ; <code> ~` when trusted.

- [ ] Failing tests: `buttons_parse_and_write_spaces`, `invalid_custom_button_disables_all`, `custom_button_clicks_report_when_trusted`. Commit `Add inline buttons`.

### Task 8: Parity and acceptance

- [ ] Update the rows 5e closes, add milestone 5f (portholes: render=1, Render Selection Natively, File= text documents) to the roadmap, recount, write `docs/acceptance/m5e-checklist.md`. Commit.
