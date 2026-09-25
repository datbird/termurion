# Tandem Milestone 5f: Portholes (natively rendered documents in the grid)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's portholes: a document shown rendered in place of grid rows (markdown, JSON, or syntax-highlighted code), made by OSC 1337 Block `render=1`, OSC 1337 File= `inline=1` with a text payload, and Edit > Replace Selection > Render Selection Natively; with iTerm2's chrome (border, close button, language list, Wide), close to restore the raw rows, selection and Copy inside the porthole, block copy of the raw rows, Find over the rendered text, and link clicks that ask first.

**Architecture:** A pure `porthole/detect.rs` resolves the language (type hint, file name, the markdown and JSON sniffs, then a best-effort detector). `porthole/render.rs` turns text plus a language into a `Doc` of styled lines (pulldown-cmark for markdown, serde_json with key order kept for JSON, syntect for code) and lays it out at a pixel width into visual lines. `Screen` reserves the rows: the fold engine's splice swaps the source rows for N blank rows tagged with a porthole id (alacritty_terminal patch 12, a Row `porthole` slot) and keeps the raw rows to put back on close. The scene draws each visible porthole over its rows in the normal wgpu pass (no renderer switch, unlike iTerm2). The app handles the controls, selection, copy, find, and links.

**Tech Stack:** as milestone 5e, plus `pulldown-cmark` (markdown), `syntect` with `default-fancy` (pure Rust regex, no oniguruma), and serde_json's `preserve_order` feature.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` Edit > Replace Selection > Render Selection Natively, OSC 1337 Block (`render=1`), OSC 1337 File= text documents; `02-global-settings.md` defaultWideMode, nativeRenderingCSSLight, nativeRenderingCSSDark. iTerm2 facts: `.superpowers/research/m5f-iterm2-facts.md`.

## Global Constraints

- Everything from milestones 0 to 5e still holds.
- iTerm2's behavior is the target, with the fixes the facts file lists under "Fix, don't copy": a type hint that matches nothing still lets the sniffs run; JSON keeps key order and prints `true`/`false`; relative markdown links resolve against the first row's working directory; a selection ending at column 0 does not take that row.
- Rows reserved = `ceil((content height + 16 px) / cell height)`, at least 1. The count changes on a language change and on Wide, not on window resize or font change (iTerm2); the text re-wraps inside the fixed rows and is clipped at the bottom.
- Portholes live on the main screen only (the alternate screen hides them because it hides the main grid). Folds carry them (a folded porthole's rows go into the fold and come back with it).
- Markdown prose uses Segoe UI Variable (Segoe UI when missing), code and JSON the profile's font. Heading scales 2.0, 1.5, 1.3, 1.0, 0.8, 0.7. Every color comes from the profile: text in the default foreground; code colors map highlight classes to ANSI colors as iTerm2's dark and light CSS templates do (chosen by background brightness).
- Chrome: 1 px border in the text color at 50%, radius 4 (square corners allowed), background = the default background moved 20% toward grey 50%, a close button (xmark in a circle, text color at 50%) at the top right with a 2 px margin, the language name as a button to its left, and a "Wide" check box left of that.
- No CSV renderer and no size cap (iTerm2 has neither).
- Porthole content is kept per session (not iTerm2's global registry). Saving it with arrangements waits for milestone 7.

## Review Focus

1. A porthole whose rows scroll off the top of the scrollback. Expect: the porthole and its saved rows are dropped; no panic; close on a gone porthole does nothing. Test: Task 3 `portholes_pruned_with_scrollback`.
2. A 5000-line markdown document. Expect: rendering stays under a second, rows reserved match the laid-out height, drawing only lays out the visible slice. Test: Task 2 `layout_is_bounded_by_the_view`.
3. File= `inline=1` with bytes that are not UTF-8 and no image type. Expect: the image path (a broken-image placeholder), no porthole, no panic. Test: Task 5 `file_text_needs_utf8`.
4. Render Selection over a range that overlaps a porthole, or while the selection is still being dragged. Expect: the command is off. Test: Task 5 `render_selection_enablement`.
5. Close after the pane was resized narrower. Expect: the raw rows come back re-wrapped to the new width; the cursor keeps its row. Test: Task 3 `close_restores_rows_at_the_new_width`.

---

### Task 1: Language detection

**Files:** Create `src/porthole/mod.rs`, `src/porthole/detect.rs`; modify `src/lib.rs`, `Cargo.toml` (`syntect` with `default-features = false, features = ["default-fancy"]`).

**Interfaces:** `Language { Markdown, Json, Plain, Code(String /*syntect syntax name*/) }`; `languages() -> Vec<(display name, Language, extensions)>` (Markdown, JSON, Plain text, then every syntect default syntax); `for_type_hint(hint) -> Option<Vec<Language>>` (`.ext`, an exact MIME from a small table (`text/markdown`, `application/json`, `text/plain`, `text/x-python`, `application/javascript`, `text/html`, `text/css`, `application/xml`, `text/x-shellscript`), a display name, or a short name, case-insensitive); `for_path(name) -> Vec<Language>` (every dotted suffix); `detect(text, hint, filename) -> Detected { language: Option<Language>, candidates: Vec<Language> }` in iTerm2's order (hint with one match wins; hint with several gives candidates; a hint with none falls through; then the filename; then `^# .` is markdown; then JSON (fragments allowed); then `guess(text, candidates)`); `guess` = a shebang or first-line match (syntect `find_syntax_by_first_line`), then a keyword score over a fixed table of 20 common languages, else Plain.

- [ ] Failing tests: `type_hints_follow_iterm2`, `filename_suffixes`, `sniffs_markdown_then_json`, `empty_hint_still_sniffs`, `guess_uses_shebang_and_keywords`. Commit `Add porthole language detection`.

### Task 2: Rendering and layout

**Files:** Create `src/porthole/render.rs`; modify `Cargo.toml` (`pulldown-cmark`, serde_json `preserve_order`).

**Interfaces:** `Face { Prose, Code }`; `Run { text, color: Rgb, bold, italic, underline, face, link: Option<String> }`; `Block { runs: Vec<Run>, scale: f32, indent: f32 /*cells*/, code_bg: bool }`; `Doc { blocks: Vec<Block> }`; `render(text, &Language, palette, dark) -> Doc` (markdown: headings with scales, emphasis, strong, inline code and fenced code in the code face (fenced code highlighted by its info string), lists with bullets and numbers, block quotes indented, links with their target, rules as a line of U+2500, tables as aligned code-face rows; JSON: 4-space pretty print, key order kept, strings green, numbers red, null grey, punctuation at 75%, a parse error in red; code: syntect scopes mapped to ANSI colors per the dark or light template); `layout(doc, width_px, measure: &dyn Fn(&str, Face, f32) -> f32, wide: bool) -> Layout { lines: Vec<VisualLine { runs, y, h }>, height, width }` (word wrap to the width unless wide; a visual line's height = cell height x scale); `Layout::visible(y0, y1) -> &[VisualLine]`.

- [ ] Failing tests: `markdown_blocks_and_styles`, `json_keeps_order_and_bools`, `json_error_shows_red`, `code_colors_from_palette`, `wraps_to_width_unless_wide`, `layout_is_bounded_by_the_view`. Commit `Render porthole documents`.

### Task 3: Reserving rows (alacritty patch 12)

**Files:** `crates/alacritty_terminal` (Row `porthole` slot: `porthole()`, `set_porthole()`, cleared by `reset`, merged by `absorb_tags`, kept by split rows), `TANDEM-PATCHES.md`; create `src/porthole/store.rs`; modify `src/screen.rs`.

**Interfaces:** `Porthole { id, text, language: Option<Language>, candidates, filename, base_dir, wide, rows: u32, saved: Vec<Row<Cell>>, doc: Doc }`; `Screen::render_range(first, last, source: PortholeSource { hint, filename, wide }, rows_for: impl Fn(&Porthole) -> u32) -> Option<u32>` (main screen only; widens to whole rows; a last row at column 0 of a selection is the caller's to drop; text = the rows' text, soft wraps joined, no trailing newline; when the cursor is inside the range, a line feed first moves it below so the range never holds the cursor; the rows become `rows_for` blank hard-wrapped rows tagged with the id; marks, notes, and buttons inside go with the saved rows); `Screen::close_porthole(id) -> bool` (the saved rows re-wrapped to the current width take the blank rows' place); `Screen::porthole_rows(id) -> Option<(i32, i32)>`; `Screen::set_porthole_rows(id, n)` (language change, Wide); `prune_tags` drops portholes whose rows are gone.

- [ ] Failing tests: `render_range_reserves_tagged_rows`, `cursor_moves_below`, `close_restores_rows_at_the_new_width`, `portholes_pruned_with_scrollback`, `folds_carry_portholes`, `alt_screen_refuses`. Commit `Reserve rows for portholes`.

### Task 4: Drawing and chrome

**Files:** Modify `src/scene.rs`, `src/render.rs` (a `Span` face for prose and a per-row scale), create `src/app/porthole_ui.rs`, modify `src/app/mod.rs`.

**Interfaces:** `scene::porthole(frame, rect, visible: &[VisualLine], chrome: Chrome { fg, bg, language: &str, wide, scroll_x, hover }, cell)` (background, border, text rows clipped to the rect, the close button, the language button, the Wide box; returns the control rects); a porthole at `x = inner.x`, `y = first_row*cell_h + 4`, `w = inner.w`, `h = rows*cell_h - 8`, clipped to the pane; `App` lays out each porthole's `Doc` once per width and font (cached by id, width, font size) and draws only the visible lines.

- [ ] Scene test `porthole_chrome_and_clip`. Commit `Draw portholes`.

### Task 5: Making portholes

**Files:** Modify `src/marks.rs` (Block `render=1` at end, `type` from start), `src/inline_file.rs` (File= `inline=1`: iTerm2's smellsLikeImage rules; valid UTF-8 text becomes a porthole with `type`, `filename`, `mode=wide`; invalid UTF-8 goes to the image path), `schema/menus.toml` and `src/commands.rs` (Edit > Replace Selection > Render Selection Natively, Win+Alt+E, `renderSelection:`), `schema/global.toml` (`defaultWideMode` bool false, Advanced > General).

**Interfaces:** `inline_file::smells_like_image(type, filename) -> bool`; `porthole::selection_can_render(sel, dragging, overlaps) -> bool` (one selection, not being dragged, not over a porthole); the Block path runs before later bytes are parsed (the rows exist at once).

- [ ] Failing tests: `block_render_makes_a_porthole`, `file_text_needs_utf8`, `smells_like_image_follows_iterm2`, `render_selection_enablement`, `selection_ending_at_column_0`. Commit `Make portholes from blocks, files, and selections`.

### Task 6: Controls, selection, copy, find, links

**Files:** Modify `src/app/porthole_ui.rs`, `src/find.rs`, `src/app/blocks_ui.rs`.

**Interfaces:** close button restores the rows; the language button opens a popup menu (candidates sorted, a separator, then all languages), a choice re-renders and changes the reserved rows; Wide toggles no-wrap with a horizontal offset moved by Shift+wheel and horizontal wheel; a drag inside a porthole selects rendered text (exclusive: it clears the terminal selection and other portholes' selections, and a terminal selection clears it); Copy, Copy with Styles (the runs as styled rows, through 5c's writer), Copy with Control Sequences act on it; block copy over a porthole row copies its saved raw rows; Find searches rendered text too and marks matches yellow with black text; a link click asks in a bar ("Open URL <url>?" or "Open file at <path>?", Yes / Always / No; Always stores `NoSyncOpenURLFromMarkdownLink` or `NoSyncOpenFileFromMarkdownLink`), file links resolve against `base_dir` and must pass `prompted::local_path_ok`, URLs pass `prompted::open_url`'s scheme rules.

- [ ] Failing tests: `selection_is_exclusive`, `porthole_copy_uses_rendered_text`, `block_copy_uses_raw_rows`, `find_sees_rendered_text`, `links_resolve_and_ask`. Commit `Add porthole controls, selection, copy, find, and links`.

### Task 7: Highlight themes from CSS

**Files:** Modify `src/porthole/render.rs`, `schema/global.toml` (`nativeRenderingCSSLight`, `nativeRenderingCSSDark`, strings, Advanced > General).

**Interfaces:** `theme_from_css(css, palette) -> Theme` (iTerm2's `${fg}`, `${bg}`, `${black}` to `${brightWhite}` placeholders; rules of the form `.hljs-<class>[, ...] { color: #hex; font-weight: bold; font-style: italic }`; anything else ignored); the class names map to syntect scopes by a fixed table; a missing or unreadable file uses the built-in template and shows the error in a bar once.

- [ ] Failing tests: `css_placeholders_and_rules`, `bad_css_falls_to_template`. Commit `Read porthole highlight themes from CSS`.

### Task 8: Parity and acceptance

- [ ] Update the rows 5f closes, mark milestone 5 done in the roadmap, recount, write `docs/acceptance/m5f-checklist.md`. Commit.
