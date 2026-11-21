# Termurion patches to cosmic-text 0.19.0

This is cosmic-text 0.19.0 from crates.io (MIT OR Apache-2.0, see the LICENSE files), used
through `[patch.crates-io]` in Termurion's Cargo.toml. The `benches/`, `sample/`, `screenshots/`,
`fonts/` and `tests/` directories, the `*.sh` scripts, and the `[[bench]]`, `[[test]]`,
`[workspace]`, `[profile.test]` and `[dev-dependencies]` sections of Cargo.toml are left out. Each
changed site carries a `// Termurion patch N:` comment. Seven upstream comments in `buffer.rs`
and `shape.rs` had an em dash, now a colon. Changes:

1. Vendored as is. The root `Cargo.toml` lists cosmic-text as a workspace member so
   `cargo test -p cosmic-text --lib` runs.
