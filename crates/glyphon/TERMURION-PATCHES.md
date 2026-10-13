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

To update glyphon, copy the new version here and apply these changes again.
