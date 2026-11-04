# Termurion patches to cosmic-text 0.19.0

This is cosmic-text 0.19.0 from crates.io (MIT OR Apache-2.0, see the LICENSE files), used through
`[patch.crates-io]` in Termurion's Cargo.toml. Only `src/` and the licenses are kept; the tests,
benches and dev-dependencies are left out. The changed site carries a `// Termurion patch:` comment.

1. `src/shape.rs`, glyph layout: when a monospace width is set (the terminal's cell), a glyph of a
   proportional font (no monospace em width, which is also the case for most primary faces) advances a whole number of cells, at least one. Upstream
   leaves its advance as the font gives it, so a proportional fallback face drifts off the grid.

To update cosmic-text, copy the new version here and apply this change again.
