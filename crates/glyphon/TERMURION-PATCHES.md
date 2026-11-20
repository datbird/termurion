# Termurion patches to glyphon 0.12.0

This is glyphon 0.12.0 from crates.io (MIT OR Apache-2.0 OR Zlib, see the LICENSE files), used
through `[patch.crates-io]` in Termurion's Cargo.toml. The `examples/`, `benches/`, `samples/` and
`.github/` directories and the dev-dependencies are left out. Each changed site carries a
`// Termurion patch:` comment. Changes:

1. Mask filter for aliased and thin-stroke text. `lib.rs` has `set_mask_filter(fn(u32, &mut [u8]))`
   (a process-wide `OnceLock`; a second call is ignored) and `filter_text_mask`, which runs the
   filter over a text glyph's alpha mask when `CacheKey.flags` holds bit 8 or bit 9 (Termurion's
   `FLAG_ALIASED` and `FLAG_THIN`). It is called at both places that turn a text glyph's
   `SwashImage` into atlas data: `text_render.rs` (the first upload, after
   `get_image_uncached`) and `text_atlas.rs` (the re-rasterization of every cached glyph when an
   atlas grows). Color glyphs are never filtered. The flags are part of the cache key, so each
   mode is cached apart.
2. `text_atlas.rs`: the re-rasterization of a cached text glyph skips a glyph that no longer
   rasterizes instead of unwrapping (a font that fails to load must not panic the renderer).
3. `text_atlas.rs`: `TextAtlas::mask_size()` returns the side of the mask atlas texture (Termurion's
   atlas growth test reads it).
4. Use counts in the atlas (`text_atlas.rs`, `lib.rs`). `GlyphKey` names a cached glyph;
   `TextAtlas::hold` and `release` keep a use count per key (a release at 0 does nothing), `held()`
   counts the keys above 0, and `generation()` is raised by every successful grow. `InnerAtlas::try_allocate`
   walks the LRU from the oldest end and evicts the first entry that is neither in use nor held (a
   zero-size entry takes no atlas space, so dropping it is free); a busy entry at the oldest end no
   longer ends the search (the caller grows when none qualifies). Both inner atlases keep the counts,
   since a key may sit in either.
5. `text_atlas.rs`: `set_max_size(px)` (doc hidden) caps both atlases' `max_texture_dimension_2d`
   so tests can force eviction at a small size, and `evictions()` (doc hidden) counts the entries
   evicted so a test can prove it did. Tests: `text_atlas::tests` on a headless GPU
   (`test_gpu.rs`, one process-wide device). The root `Cargo.toml` lists glyphon as a workspace member
   so `cargo test -p glyphon --lib` runs, and glyphon has `pollster` and `wgpu` (default backends) as
   dev-dependencies.
6. Row renderer (`row_render.rs`, `row_shader.wgsl`, `cache.rs`, `text_render.rs`). `RowRenderer`
   keeps each `RowArea`'s prepared glyph instances on the GPU, so a row that did not change runs no
   `prepare_glyph`. A row is made as `TextRenderer` makes a text area at scale 1, but with no bounds
   clipping; it holds its glyph keys (patch 4) while kept. Each frame it culls layout runs with
   `TextRenderer`'s own test (line box against the unclamped bounds) and draws the instances of the
   runs left, so a glyph overhanging into bounds its line misses never draws. It is reused under
   `(serial, left, default_color)` plus `top`, or at any `top` when every glyph's y term is whole,
   both tops are at least 0 and each term's truncation moves by the same `dy`. Rows not in a frame
   are dropped and release their keys before any new row is prepared, and an `AtlasFull` drops every
   kept row the pass did not reach and tries once more, so the last frame never pins the atlas full; a new atlas generation drops every row, and a
   `prepare` that grows the atlas runs again. Instances live in power-of-two slots of one vertex
   buffer; a row table (a uniform buffer with dynamic offsets) gives each draw its `dy` and clip
   rectangle, and `vs_row` clips in the same integer steps as `prepare_glyph`, so the pixels match
   `TextRenderer` byte for byte. No storage buffers and no `first_instance` (GLES 3.0). `cache.rs`
   holds the row pipeline; `text_render.rs` shares `prepare_glyph`, `text_glyph_image` and the
   bounds clamp (`GlyphBounds::clamped`). Tests: `row_render::tests`.
7. Upload belt (`upload.rs`, `row_render.rs`, `text_render.rs`). Every `Queue::write_buffer` makes a
   staging buffer that the next submit frees. `UploadBelt` keeps mapped staging chunks (256 KB, a
   bigger write gets its own), records each write as a copy into one encoder, and `submit` sends
   that encoder ahead of the caller's command buffers in one submission, then maps the chunks
   again with `map_async` (a free chunk comes back on a later poll or submit; at most 8 are kept
   free, and of those bigger than 256 KB only the largest; a write takes the smallest free chunk
   that fits). `buffers_made()` counts the chunks made. `RowRenderer::prepare` takes the belt for its
   instance and row table writes, and `TextRenderer::prepare_with_custom_uploads` writes its
   vertices through it (the upstream `prepare*` methods still use the queue). A grown buffer is
   dropped, not destroyed, since a copy recorded in the same frame may target it. Tests:
   `row_render::tests` (the rig submits the belt after each prepare).
8. Batched row draws (`row_render.rs`, `row_shader.wgsl`, `cache.rs`). Each kept row has a stable
   id (a free list, lowest first) and the row table holds one `RowUniform` per id, in 512-row
   chunks of 16384 bytes (`RowTable`, the GLES 3.0 uniform block limit) picked by a dynamic offset;
   an id's entry is written only when its offset or clip changed. A second instance-rate vertex
   buffer gives every instance its row id (`@location(6)`, read as `rows[id % 512]`). Taking a
   slot writes the whole slot: the row's instances, then zero instances (`dim` 0, which `vs_row`
   culls) with the row's id, so a short row in a long row's old slot draws nothing past its end.
   `render` walks the rows in `prepare` order and joins a row to the current draw only when its
   slot starts where the last one ends (only padding between) and its id is in the same chunk, so
   the draw order and the pixels are unchanged. Free slots are taken lowest first per class, so a
   new screen lays out in draw order. An entry drawn twice in one frame with another offset or clip
   is prepared again as its own entry (one table entry per id). `draws_issued()` counts the draws
   of the last `prepare`. Tests: `row_render::tests`.

To update glyphon, copy the new version here and apply these changes again.
