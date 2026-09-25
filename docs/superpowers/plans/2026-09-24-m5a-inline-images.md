# Tandem Milestone 5a: Inline Images (OSC 1337 File, Sixel, Kitty Graphics)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's inline images: OSC 1337 `File=` and `MultipartFile=`/`FilePart=`/`FileEnd` (inline display with iTerm2's sizing, and downloads to the Downloads folder with iTerm2's confirmation), sixel graphics, and the kitty graphics protocol (transmit, display, delete, query, chunking, zlib), with animated GIFs and the broken-image placeholder.

**Architecture:** iTerm2 keeps an image in the grid as cells that each carry an image number and the cell's place in the image; Tandem does the same with an alacritty_terminal patch (an image tile in `CellExtra`), so images scroll, erase, and clear like text. `images.rs` decodes and sizes; `Screen::place_image` writes the tiles; `sixel.rs` and `kitty.rs` parse their protocols; the OSC scanner also captures DCS (sixel) and APC (kitty) strings. The renderer gets a textured-quad pipeline; the scene emits one textured quad per visible image cell.

**Tech Stack:** as milestone 4, plus the `image` crate (PNG, JPEG, GIF, BMP, WebP, TIFF, ICO) and `flate2` (kitty `o=z`).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` section 2 (OSC 1337 File, MultipartFile, FilePart, FileEnd; Sixel; Kitty graphics protocol).

## Global Constraints

- Everything from milestones 0 to 4 still holds. iTerm2 facts are the research in the ledger (VT100Terminal.m 3970-4315, VT100InlineImageHelper.m, ScreenChar.h, iTermImage+Sixel.m, KittyImageController.swift, KittyImageCommand.swift, TerminalFile.m).
- Sizing follows iTerm2: pixels to cells is `ceil(px / cell)`, percent is of the grid, auto keeps the aspect ratio; an image wider than the space left is shrunk to the full grid width; height is capped at 255 cells. After an inline image the cursor stays on its last row, moved right by its width.
- Copying a selection over an image gives no text for the image cells.
- A decode failure shows iTerm2's broken-image placeholder; nothing panics on bad input.

## Review Focus

1. A 20 MB base64 image streamed in 4 KB chunks. Expect: shown once complete, no stall per chunk. Test: Task 4 `a_large_file_streams_through_the_scanner`.
2. An image that scrolls into history and then the buffer is cleared. Expect: the image memory is freed. Test: Task 1 `unused_images_are_dropped`.
3. Resize narrower with an image on screen. Expect: no panic, tiles stay tiles. Test: Task 1 `image_rows_survive_reflow`.
4. A malformed sixel or kitty command (bad numbers, truncated base64, huge sizes). Expect: an error reply or nothing, never a panic or a huge allocation. Tests: Task 5 `bad_sixel_is_harmless`, Task 6 `bad_kitty_commands_reply_errors`.
5. Many images on screen at once (100 small ones). Expect: each texture uploads once, not every frame. Test: Task 3 `textures_upload_once`.

---

### Task 1: Image tiles in the grid (alacritty patches 7 and 8)

**Files:** `crates/alacritty_terminal/src/term/cell.rs`, `term/mod.rs`, `TANDEM-PATCHES.md`; create `src/images.rs` (the store); modify `src/screen.rs`.

**Interfaces:** `Tile { image: u32, x: u16, y: u16 }` in `CellExtra`, with `Cell::image() -> Option<Tile>` and `Cell::set_image(Option<Tile>)`; DA1 reports sixel (`CSI ? 62 ; 4 c`). `Store { images: HashMap<u32, Image> }` where `Image { width_px, height_px, cols, rows, frames: Vec<Frame { rgba: Arc<Vec<u8>>, delay_ms }>, broken, generation }`; `Screen::place_image(image, cols, rows, move_cursor: bool)` writes tiles at the cursor (scrolling as needed), then leaves the cursor as iTerm2 does; `Screen::prune_images()` drops images no cell uses (run with `prune_tags`).

- [ ] Failing tests: `placing_an_image_writes_tiles_and_moves_the_cursor`, `image_cells_copy_as_no_text`, `text_over_an_image_erases_its_tiles`, `image_rows_survive_reflow`, `unused_images_are_dropped`, `da1_reports_sixel`. Commit `Add image tiles to the grid`.

### Task 2: Decoding and sizing

**Files:** Modify `src/images.rs`, `Cargo.toml`.

**Interfaces:** `decode(bytes) -> Result<Decoded, String>` (all frames of a GIF with delays; others one frame; RGBA8); `Dim { Auto, Cells(u32), Pixels(u32), Percent(u32) }`; `size_in_cells(img_px: (u32, u32), w: Dim, h: Dim, keep_aspect: bool, cell_px: (f32, f32), grid: (usize, usize), cursor_col: usize) -> (usize, usize)`; `broken() -> Decoded` (a small placeholder image: a grey frame with a red cross).

- [ ] Failing tests: `decodes_png_and_gif_frames`, `sizes_follow_iterm2`, `wide_images_shrink_to_the_grid`, `height_caps_at_255`, `corrupt_bytes_decode_as_broken`. Commit `Add image decoding and sizing`.

### Task 3: Drawing images

**Files:** Create `src/image.wgsl`; modify `src/render.rs`, `src/scene.rs`.

**Interfaces:** `Frame.images: Vec<ImageQuad { rect, uv: [f32; 4], key: (u32, u64, usize) /*id, generation, frame*/, pixels: Arc<Vec<u8>>, size: (u32, u32) }>`, drawn after `quads` and before text. The renderer keeps one texture per key, uploads it the first frame it is seen, and drops textures no frame used for 120 frames. `scene::pane_with` emits a quad per visible tile (the frame chosen by wall-clock time for animations) and a placeholder for a broken image.

- [ ] Failing tests: scene `tiles_become_textured_quads`, GPU `draws_an_image_offscreen`, `textures_upload_once`. Commit `Draw inline images`.

### Task 4: OSC 1337 File and downloads

**Files:** Modify `src/osc.rs`, `src/marks.rs` (or a new `src/inline_file.rs`), `src/app/*`.

**Interfaces:** The scanner keeps `1337;File=`, `1337;MultipartFile=`, and `1337;FilePart=` payloads up to 512 MB; `FileArgs { name, size, width: Dim, height: Dim, keep_aspect, inline, kind }` parsed from iTerm2's grammar; multipart parts assemble until `FileEnd`. Inline: decode, size, place. Not inline: the app asks "Download NAME (SIZE) to Downloads?" (Yes / No), then writes a unique file name in the Downloads folder and shows where. Invalid base64 shows the broken image.

- [ ] Failing tests: `file_args_parse`, `multipart_files_assemble`, `a_large_file_streams_through_the_scanner`, `downloads_get_unique_names`. Commit `Add OSC 1337 File`.

### Task 5: Sixel

**Files:** Create `src/sixel.rs`; modify `src/osc.rs`, `src/screen.rs`.

**Interfaces:** The scanner captures `ESC P <params> q <data> ESC \`; `sixel::decode(params, data) -> Result<(w, h, rgba), String>` with the VT340 default palette, `#` color definitions (RGB and HLS), `!` repeats, `$` and `-`, raster attributes `"`; pixels are opaque as in iTerm2 (P2 ignored); limits of 10,000 x 10,000 pixels. The image is placed at the cursor (DECSDM off behavior: the cursor column returns to where it was).

- [ ] Failing tests: `decodes_a_small_sixel`, `repeat_and_newline`, `raster_attributes_set_the_size`, `bad_sixel_is_harmless`. Commit `Add sixel graphics`.

### Task 6: Kitty graphics protocol

**Files:** Create `src/kitty.rs`; modify `src/osc.rs`, `src/screen.rs`.

**Interfaces:** The scanner captures `ESC _ G <keys> ; <payload> ESC \`; `Command` parsed from the keys; actions t, T, p, d, q (a, f, c parsed and answered, not animated, as in iTerm2); formats 24, 32, 100; media d (direct), f (file), t (temp file, deleted after); s (shared memory) answers `EINVAL`; `o=z` inflates; `m=1` chunks; ids `i`, `I`, placements `p`; display with `c`/`r` (cells), `x`/`y`/`w`/`h` (source rectangle), `C=1` (cursor stays); `z` and pixel offsets are not supported (Partial); deletion `a`, `i`, `n`, `c`, `p`, `q`, `r`, `x`, `y`, `z`, upper or lower case alike; replies `ESC _ G i=..;OK ESC \` or `;CODE:message`, honoring `q=1`/`q=2`. Image data kept up to 320 MB, oldest dropped first.

- [ ] Failing tests: `kitty_keys_parse`, `transmit_and_display_places_tiles`, `chunks_and_zlib_assemble`, `deletes_by_id`, `bad_kitty_commands_reply_errors`, `quiet_levels`. Commit `Add the kitty graphics protocol`.

### Task 7: Animation, parity, acceptance

- [ ] Animated GIFs redraw while on screen (the next frame's time as the wake-up). Update the rows 5a closes, recount `00-roadmap.md`, write `docs/acceptance/m5a-checklist.md`. Commit.
