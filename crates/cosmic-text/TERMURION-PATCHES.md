# Termurion patches to cosmic-text 0.19.0

This is cosmic-text 0.19.0 from crates.io (MIT OR Apache-2.0, see the LICENSE files), used
through `[patch.crates-io]` in Termurion's Cargo.toml. The `benches/`, `sample/`, `screenshots/`,
`fonts/` and `tests/` directories, the `*.sh` scripts, and the `[[bench]]`, `[[test]]`,
`[workspace]`, `[profile.test]` and `[dev-dependencies]` sections of Cargo.toml are left out. Each
changed site carries a `// Termurion patch N:` comment. Seven upstream comments in `buffer.rs`
and `shape.rs` had an em dash, now a colon. Changes:

1. Vendored, with the `shape-run-cache` feature in `default` (Cargo.toml), so every build and
   cosmic-text's own tests have the run cache. The root `Cargo.toml` lists cosmic-text as a
   workspace member so `cargo test -p cosmic-text --lib` runs; its tests load the fonts in
   Termurion's `tests/fixtures/fonts` with `include_bytes!`.
2. `shape_run_cache.rs`, `shape.rs` (`shape_run_cached`): `ShapeRunKey` holds `rtl`, the run's
   direction. Without it the same text shaped in a right to left span and a left to right one
   (a `(12)` or `!?` inside Hebrew: mirrored and reversed glyphs) shared one entry and drew wrong.
   Test: `shape::run_cache_tests::the_same_text_in_each_direction_keeps_its_own_glyphs`.
3. `shape_run_cache.rs`, `shape.rs` (`shape_run_cached`): caps and a switch. `MAX_ENTRIES` (4096
   runs), `MAX_GLYPHS` (32768 glyphs across all runs) and `MAX_RUN_BYTES` (256: a longer run is
   shaped and never stored). An `insert` that would pass a cap first drops every run not used since
   the last `trim`, and stores nothing when that is not enough; `trim` keeps the glyph count.
   `set_enabled(false)` clears the cache and makes `shape_run_cached` call `shape_run` with
   nothing touched. `clear`, `len`, `is_empty`, `glyph_count`, and `stats` (hits and misses since
   creation or the last `clear`; a run over `MAX_RUN_BYTES` counts a miss, decided before the key
   is built). Tests: `shape_run_cache::tests`, `shape::run_cache_tests`.
4. `font/system.rs` (`db_mut`): also clears the run cache, as it clears the font match cache, so a
   cached run never names a fallback that a newly loaded font replaces. Test:
   `shape::run_cache_tests::loading_a_font_clears_the_cache`.
5. `Cargo.toml`, `src/edit/mod.rs`: the `vi` feature, its optional dependencies (`modit`,
   `syntect`, `cosmic_undo_2`), the docs.rs metadata that enabled it, and `src/edit/vi.rs` and
   `src/edit/syntect.rs` with their `cfg` lines are left out. Termurion never uses the editor, and
   a workspace member's optional dependencies are otherwise locked in `Cargo.lock`.
6. `shape_run_cache.rs`: two more limits on patch 3. `MAX_SPANS` (16384 attribute spans across all
   keys, `span_count()`): a run colored per character (lolcat, character-level highlighting) holds
   a span per character in its key, about 200 bytes each, so the spans have their own cap; at all
   caps the cache holds about 7.7 MB. And an eviction pass that frees nothing (every run was used
   this frame) remembers the age, so later misses in the same frame that do not fit are skipped without scanning
   the map again; `trim` and `clear` forget it. Tests: `the_cache_never_passes_its_caps`,
   `a_full_cache_skips_the_eviction_pass_until_the_next_trim`.
7. `lib.rs`: the crate doc example's text `"Hello, Rust! "` plus a crab emoji is written with the
   escape `\u{1F980}`, the same string with no emoji in the source.
