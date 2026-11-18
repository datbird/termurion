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
   so tests can force eviction at a small size. Tests: `text_atlas::tests` on a headless GPU
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

To update glyphon, copy the new version here and apply these changes again.
