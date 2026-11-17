# Redraw Only Changed Rows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A screen change costs work in proportion to the rows it changed, not the whole window, with an Advanced setting (on by default) to go back to full rebuilds.

**Architecture:** `scene::pane_with` builds each grid row through one per-row function whose output (quads, text rows, box glyphs, underlines, strikes, images, hover cells) is kept in a per-pane `RowCache`, keyed by everything that row's output depends on and stored in row-local coordinates, so a row that only moved (scrolling output) is reused at its new `y`. The renderer keeps its shaped glyphon buffers by row content instead of by slot, so a scrolled row is not shaped again. A full rebuild every 0.5 s and a setting that turns reuse off are the safety nets; tests build every frame both ways and require the two `Frame`s to be equal.

**Tech Stack:** Rust 2024, alacritty_terminal (vendored in `crates/`), glyphon 0.12 (vendored in `crates/glyphon`), wgpu. Tests: `scripts/xt test --lib -- <filter>` (Windows, EXTREME), `scripts/check-unix --test <filter>` (Linux here, macOS on funtimes).

**Spec:** FIXES.md item 29 (`docs/FIXES.md`) and this conversation's design: reuse on by default; an Advanced switch turns it off for suspected drawing bugs; full rebuild every 0.5 s in both modes; tests compare a reused frame with a full frame.

## Global Constraints

- Setting key `RedrawOnlyChangedRows`, type bool, `default = true`, tab `Advanced`, section `Drawing`. Termurion-only (iTerm2 has no such key); mark it so the parity/ui_labels checks accept it, as `EffectsAnimation` was (1.1.4).
- Label: `Redraw only the rows that changed`. Help: `Termurion rebuilds only the rows whose text, colors or highlights changed, as other fast terminals do. Turn this off if text is drawn wrongly (leftover or stale characters) and report it.`
- With the setting off, output must be exactly what 1.1.6 draws (the full path).
- A reused row must make the `Frame` `==` the frame a full rebuild makes, for every input. No row may be reused when any input it depends on changed.
- Full rebuild (cache cleared) at least every `ROW_CACHE_MAX_AGE = 500 ms`.
- Code comments: short, plain, human sentences (house style). No em dashes, no emoji anywhere (code, docs, commits). No host or user names in commits. No AI attribution in commits.
- Every task ends green on Windows (`scripts/xt`) and compiles on Linux and macOS (`scripts/check-unix`).

## Review Focus

1. **Scrolling output (Claude streaming, `tail -f`).** Rows move up one line per frame: each must be reused at its new `y`, and the new bottom row built. Test: Task 3 `scrolled_rows_are_reused_at_their_new_place`.
2. **Selection, search matches, hover, labels, notes and the unfocused dim change a row without changing its cells.** Each must change the row key (or force a rebuild). Test: Task 3 `highlights_change_the_row` (one case per kind).
3. **The cursor row and its neighbors** (block cursor, Smart Cursor Color reads rows above and below, IME). These rows are never reused. Test: Task 3 `cursor_rows_are_always_built`.
4. **Animated images and image tiles** read the clock. Rows with an image are never reused. Test: Task 3 `image_rows_are_always_built`.
5. **A palette, font, size, profile or preference change** must rebuild every row. Test: Task 3 `context_change_rebuilds_every_row`.

---

## File Structure

- Create `src/scene_rows.rs`: `RowCache`, `RowKey`, `RowOut`, translation, size cap. Pure data, unit-tested.
- Modify `src/scene.rs`: split the per-row work of `pane_with` (cell looks 1622-1792 and the row loop 1794-1903) into `fn build_row(...) -> RowOut`; `pane_with` gains `rows: Option<&mut RowCache>`.
- Modify `src/pane.rs`: `Pane` owns a `RowCache`.
- Modify `src/app/mod.rs` (`build_frame`, near 6774): pass the pane's cache when `RedrawOnlyChangedRows` is on; clear caches on the 0.5 s timer and when the setting turns off.
- Modify `src/render.rs` (`draw_with`, 1571-1855): shaped buffers found by row content.
- Modify `schema/global.toml`, `src/settings_keys.rs`, `schema/ui_labels.toml` (or its exempt list), `docs/help/gpu-renderer.md`, `docs/FIXES.md`.

---

### Task 1: The setting

**Files:**
- Modify: `schema/global.toml` (next to `SolidUnderlines`, line ~4337), `src/settings_keys.rs` (~4676), the ui_labels exempt list used for `EffectsAnimation`, `docs/help/gpu-renderer.md`
- Modify: `src/scene.rs` `DrawPrefs` (404-440) and `DrawPrefs::from_globals` (454)

**Interfaces:**
- Produces: `DrawPrefs.reuse_rows: bool` (from `globals.bool("RedrawOnlyChangedRows")`; `DrawPrefs::default()` sets `false` so existing tests keep the full path).

- [ ] **Step 1: Write the failing test** in `src/scene.rs` tests: `reuse_rows_follows_its_setting`: `Globals::default()` gives `DrawPrefs::from_globals(..).reuse_rows == true`; after `globals.set("RedrawOnlyChangedRows", false)` it is `false`; `DrawPrefs::default().reuse_rows == false`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- reuse_rows_follows_its_setting`. Expected: FAIL (no field / unknown key).
- [ ] **Step 3: Add the schema entry** with the exact key, label, help and default from Global Constraints, the `settings_keys.rs` row, the exempt-list entry, the field and its read. Add a short "Redraw only the rows that changed" paragraph to `docs/help/gpu-renderer.md` with the help text.
- [ ] **Step 4: Run** the new test plus the schema and label suites: `scripts/xt test --lib -- reuse_rows settings_keys ui_labels globals`. Expected: PASS.
- [ ] **Step 5: Commit** `Settings: RedrawOnlyChangedRows (Advanced > Drawing), on by default`.

### Task 2: RowCache, RowKey, RowOut

**Files:**
- Create: `src/scene_rows.rs` (add `mod scene_rows;` in `src/lib.rs`)

**Interfaces:**
- Produces:
  - `pub struct RowOut { pub quads: Vec<Quad>, pub rows: Vec<TextRow>, pub glyphs: Vec<CellGlyph>, pub underlines: Vec<Quad>, pub strikes: Vec<Quad>, pub images: Vec<ImageQuad>, pub hovered: Vec<usize> }` with all `y` values relative to the row's top (`y = 0`); `hovered` holds columns.
  - `impl RowOut { pub fn place(&self, frame: &mut Frame, top: f32, row: usize, hovered: &mut Vec<(usize, usize)>) }`: appends each list to the matching `frame` Vec, adding `top` to every `y` (quad `rect.y`, `TextRow.y` and `clip.y`, glyph rect, image rect), and pushes `(row, col)` for each hovered column.
  - `#[derive(Clone, PartialEq, Eq, Hash)] pub struct RowKey { pub context: u64, pub cells: Vec<Cell>, pub extra: u64 }`: `context` hashes the pane-wide inputs, `cells` is the row's cells (`alacritty_terminal::term::cell::Cell` is `Hash + Eq`), `extra` hashes the row's own highlight data.
  - `pub struct RowCache { .. }` with `pub fn new() -> Self`, `pub fn get(&mut self, key: &RowKey) -> Option<&RowOut>`, `pub fn put(&mut self, key: RowKey, out: RowOut)`, `pub fn clear(&mut self)`, `pub fn due(&self, now: Instant) -> bool` (true when 500 ms have passed since the last `clear` or creation), `pub fn len(&self) -> usize`.
  - `pub const ROW_CACHE_MAX_AGE: Duration = Duration::from_millis(500);`
  - Size cap: at most `4 * rows on screen` entries, `ROW_CACHE_MIN_ENTRIES = 256` floor; when over, drop entries not used in the last frame (`pub fn end_frame(&mut self)` marks the frame boundary).

- [ ] **Step 1: Write the failing tests** in `src/scene_rows.rs`:
  - `a_row_is_found_by_its_key`: put then get with an equal key returns the stored `RowOut`; a key differing only in `context`, in one cell's `c`, or in `extra` returns `None`.
  - `place_moves_the_row_to_its_top`: a `RowOut` with one quad at `y = 0`, one `TextRow` at `y = 2` (clip `y = 0`) and one glyph, placed with `top = 40.0`, lands at 40, 42 (clip 40) and glyph `y + 40` in `frame`; `hovered` gets `(row, col)`.
  - `clear_and_age`: `due` is false right after `new`, true at `+500 ms`, false again right after `clear`.
  - `entries_not_used_last_frame_are_dropped_past_the_cap`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- scene_rows`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement** with a `HashMap<RowKey, (RowOut, u64 /* last frame used */)>` and a frame counter.
- [ ] **Step 4: Run** `scripts/xt test --lib -- scene_rows`. Expected: PASS.
- [ ] **Step 5: Commit** `Scene: a cache of built rows, keyed by what each row shows`.

### Task 3: pane_with builds rows through the cache

**Files:**
- Modify: `src/scene.rs` (`pane_with` 1533-2133; callers in `src/screenshot.rs:282-293`, scene tests)

**Interfaces:**
- Consumes: Task 1 `DrawPrefs.reuse_rows`; Task 2 `RowCache`, `RowKey`, `RowOut`, `RowOut::place`.
- Produces: `pub fn pane_with(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool, look: &CursorLook, draw: &DrawPrefs, highlights: Option<&Highlights>, face: u16, rows: Option<&mut RowCache>)`. With `None` or `draw.reuse_rows == false` it builds every row (today's output). Existing callers pass `None`.

Design decisions the implementer cannot make alone:
- **Order of work.** First move the per-row parts (cell looks 1622-1792 for one row, then the row loop body 1794-1903) into `fn build_row(...) -> RowOut` with no behavior change and run the full scene suite; only then add the cache. Two commits.
- **Iterate rows, not `display_iter`.** `build_row` reads `term.grid()[Line(row as i32 - display_offset)]` for viewport row `row`, so a cached row skips its cells entirely.
- **Context key** (`RowKey.context`): hash of `rect.x`, `rect.w`, `cell`, `face`, `focused`, every `DrawPrefs` field, every `CursorLook` field, `screen.palette` (all fields), `screen.blink_allowed`, `screen.normalization`, the bidi rules and alt-screen flag that `row_order` reads, `Highlights.color`, `.dim`, `.bg_alpha`, `.opaque_colors`, `.text_blink_on`, `.link`, `.key_cursor`, and `history`/`display_offset` only where they change how a cell maps to a buffer row for highlights (fold them into `extra` instead, see below). Floats hash by `to_bits()`.
- **Row key** (`RowKey.extra`): hash of the buffer row number and, for that buffer row only: the `matches` entries in it, `selections` runs in it (`extra_runs`), whether `content.selection` / `term.selection` covers any of its cells and which, `labels` and `notes` on it, the `hover` cell if on it, and the `command_region` / `shade` / `outline` rows if they touch it. Note: the buffer row (not the viewport row) goes in `extra` so a scrolled row keeps its key only when its highlights moved with it.
- **Never cached** (built every frame, not stored): the cursor row; the rows directly above and below the cursor when `look.smart` is on; any row with an image cell (`c.image()` or `screen.images` tile); rows under the IME popup; rows whose `row_order` is not the identity (bidi), unless the order is part of the key.
- **Pane-level work stays once per pane, after the rows**: labels, notes, hovered underline, cursor shapes, `frame.fx`, smear, IME, command region, dim. They read `hovered` (rebuilt from each placed row's `RowOut.hovered`).
- `frame.face_heights` push stays at the top of `pane_with`.

- [ ] **Step 1: Write the comparison helper and failing tests** in `src/scene.rs` tests. Helper `both_ways(script: &[&[u8]], step: impl Fn(&mut Screen, usize))`: feeds each byte chunk to two equal `Screen`s, after each chunk builds a `Frame` with `rows: None` and a `Frame` with a persistent `RowCache` and `reuse_rows = true`, and asserts `full == reused` with the chunk index in the message. Tests (each a `both_ways` script):
  - `scrolled_rows_are_reused_at_their_new_place`: 200 numbered lines into a 24-row screen, one per chunk; also asserts the cache served at least 20 rows on the last chunk (`RowCache::hits()` counter, add it in this task).
  - `highlights_change_the_row`: one case each for a selection made then cleared, a search match added, hover on a link, a label, a note, `dim.only_text` toggled, and the pane losing focus.
  - `cursor_rows_are_always_built`: cursor moves across rows with `look.smart = true` and a block cursor.
  - `image_rows_are_always_built`: a sixel or kitty image, then text below it scrolling.
  - `context_change_rebuilds_every_row`: palette change (`OSC 4`), cell size change, `face` change, a `DrawPrefs` flag change, between frames.
  - `wide_bidi_and_box_drawing_rows_match`: CJK wide characters, an RTL line, box-drawing lines (`tmux` borders), powerline symbols.
  - `the_tmux_agent_layouts_match_with_reuse`: the fixture bytes from `tmux_agent_layouts_match_the_recorded_baseline` through `both_ways`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- scene::tests`. Expected: the new tests FAIL to compile (`pane_with` has no `rows` argument).
- [ ] **Step 3: Refactor** into `build_row` (no cache), passing `None`; run `scripts/xt test --lib -- scene` and `scripts/check-unix --test scene`. Expected: every pre-existing scene test PASS. Commit `Scene: build each grid row in its own function`.
- [ ] **Step 4: Add the cache path** per the design decisions above.
- [ ] **Step 5: Run** `scripts/xt test --lib -- scene` and `scripts/check-unix --test scene`. Expected: PASS, including all new tests.
- [ ] **Step 6: Commit** `Scene: reuse rows whose cells and highlights did not change`.

### Task 4: Panes keep their cache; build_frame uses it

**Files:**
- Modify: `src/pane.rs` (`Pane`, line 260), `src/app/mod.rs` (`build_frame` ~6774 and the `pane_with` call it makes; `about_to_wait` wake list), `src/app/frames_ui.rs` if the wake fits there

**Interfaces:**
- Consumes: Task 3 `pane_with(.., rows: Option<&mut RowCache>)`, Task 2 `RowCache::{due, clear, end_frame}`, `ROW_CACHE_MAX_AGE`.
- Produces: `Pane.rows: RowCache` (new panes start empty). Nothing else.

- [ ] **Step 1: Write the failing test** in `src/app/mod.rs` tests (source-scan style used there): `build_frame_passes_the_pane_row_cache` asserts `build_frame` passes `Some(&mut ... .rows)` only when `reuse_rows` is set, and that the cache is cleared when `due` or when the setting is off. Plus a behavior test in `src/pane.rs` if a `Pane` can be made in tests: a new pane's `rows.len() == 0`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- build_frame_passes_the_pane_row_cache`. Expected: FAIL.
- [ ] **Step 3: Implement.** In `build_frame`, per pane: if `rows.due(now)` or `!draw.reuse_rows` then `rows.clear()`; call `pane_with(.., draw.reuse_rows.then_some(&mut pane.rows))`; after the frame, `rows.end_frame()`. The 0.5 s rebuild needs no wake of its own: it happens on the next frame that comes anyway.
- [ ] **Step 4: Run** `scripts/xt test --lib` (whole suite). Expected: PASS except the 3 known failures that also fail on 1.1.5 (`handoff_win::tests::handles_are_typed_before_use`, `platform::windows::pty::tests::a_close_never_happens_while_attached`, `util_query::tests::it2ul_follows_the_script`). `scripts/check-unix`. Expected: 0 errors.
- [ ] **Step 5: Commit** `Panes keep their built rows between frames`.

### Task 5: Renderer finds shaped rows by content

**Files:**
- Modify: `src/render.rs` (`draw_with` 1571-1855, the `self.buffers` slot cache at 1586-1593)

**Interfaces:**
- Consumes: nothing new from earlier tasks.
- Produces: `Renderer::shapes()` stays the counter of shaping passes (tests read it).

Design decision: replace "buffer N matches row N" with a map from the row's shaping inputs (`spans`, `clip.w`, `scale`, `face`, the `cluster_fixes` inputs) to a shaped `Buffer`, reused at any slot; buffers not used in the last frame are dropped. Position (`x`, `y`, `clip.x/y`) is not part of the key.

- [ ] **Step 1: Write the failing test** `scrolled_rows_are_not_shaped_again` in `src/render.rs` tests (use the `device()` helper and `split_levels_keeps_every_frame_row_in_its_slot` as the pattern): draw a frame of 20 different rows, then the same rows shifted up one slot with a new last row; assert `shapes()` grew by at most 1 (the new row) and the second frame's pixels equal a fresh renderer's pixels for the same frame (`draw_offscreen`).
- [ ] **Step 2: Run** `scripts/xt test --lib -- scrolled_rows_are_not_shaped_again`. Expected: FAIL (20 shapes).
- [ ] **Step 3: Implement** the content-keyed map.
- [ ] **Step 4: Run** `scripts/xt test --lib -- render` and `scripts/check-unix --test render`. Expected: PASS.
- [ ] **Step 5: Commit** `Renderer: a scrolled row keeps its shaped text`.

### Task 6: Proof, docs, version

**Files:**
- Modify: `docs/FIXES.md` (move 29 to Done with "Done in 1.1.7"), version files as in commit `52ef1443` (Cargo.toml, Cargo.lock, README.md, docs/ROADMAP.md, docs/help/README.md, docs/help/installing.md, installer/termurion.iss)

- [ ] **Step 1: Measure** on the work Mac with a funtimes-built app: Claude Code streaming in front, `top` and `sample` as on 2026-11-17 (1.1.6: 27 to 37% CPU, `build_frame` 57% and `draw_with` 41% of frame time). Record both numbers in FIXES 29.
- [ ] **Step 2: Look at the screen** on funtimes with his settings (Lightning on): typing, `ls -la` scrolling, a selection, a search, a resize, tmux borders. Any stale or missing text fails the task.
- [ ] **Step 3: Commit** the docs, then `Version 1.1.7`.
