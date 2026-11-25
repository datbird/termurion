# Termurion patches to glyphon 0.12.0

This is glyphon 0.12.0 from crates.io (MIT OR Apache-2.0 OR Zlib, see the LICENSE files), used
through `[patch.crates-io]` in Termurion's Cargo.toml. The `examples/`, `benches/`, `samples/` and
`.github/` directories and the dev-dependencies are left out. Each changed site carries a
`// Termurion patch N:` comment. Changes:

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

9. A GPU-free core and a wgpu backend (`lib.rs`, `text_atlas.rs`, `text_render.rs`,
   `row_render.rs`, `cache.rs`, `viewport.rs`, `wgpu_backend.rs`, `core_tests.rs`, `Cargo.toml`).
   The crate root, `custom_glyph.rs`, `error.rs`, `text_atlas.rs`, `text_render.rs` and
   `row_render.rs` name no GPU type; `cache.rs`, `upload.rs`, `viewport.rs` and `wgpu_backend.rs`
   build only with the `wgpu` feature (on by default; `--no-default-features` builds the core
   alone). `TextAtlas::new(max_side)` keeps the packer, the LRU, the use counts, the mask filter
   and the re-rasterization on growth, and records each texture change as an `AtlasOp` (`Grow`
   remakes a texture at a new size, `Upload` writes a glyph's texels) that `take_ops` hands out
   in order. `TextRenderer::prepare_cpu` fills a `GlyphDraws` (the `GlyphToRender` instances,
   now public, with `glyph_bytes`); `RowRenderer::prepare_cpu` adds its slot, row id and table
   writes to a `RowDraws` (`RowWrite`s into one byte list, `clear_writes` empties it once applied,
   even after a failed prepare) and replaces its `RowBatch` list. It takes the rows as a
   `Clone` iterator and keeps its scratch sets, so a steady frame allocates nothing there;
   `release` lets go of every kept row's glyphs at once. The target size comes in as a
   `Resolution`. `wgpu_backend` has `GpuAtlas` (the two textures and their bind group; `apply`
   runs the ops with `write_texture` as before), `GpuText` (the vertex buffer and pipeline) and
   `GpuRows` (the instance, id and table buffers and the row pipeline), which upload and draw
   with the code of patches 6 to 8. The upstream wgpu entry points (`TextRenderer::prepare*`
   and `render`, `TextAtlas::new(device, ..)`) and the depth callback are gone (every caller drew
   at depth 0). `Params` moved to `viewport.rs`, `ROW_UNIFORM_SIZE` and `ROW_TABLE_CHUNK` to
   `row_render.rs`. The atlas tests (`text_atlas::tests`) now run on the CPU; the GPU tests are
   `wgpu_backend::row_tests` (the row tests of patches 6 to 8) and `wgpu_backend::atlas_tests`
   (`atlas_ops_rebuild_the_same_texels`: the ops replayed on a CPU image equal the textures read
   back, through growth and eviction; the atlas textures get `COPY_SRC` in glyphon's own tests
   only). `core_tests` scans the core files for the GPU API's name.

To update glyphon, copy the new version here and apply these changes again.
