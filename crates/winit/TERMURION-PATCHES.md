# Termurion patches to winit 0.30.13

This is winit 0.30.13 from crates.io (Apache-2.0, see LICENSE), used through `[patch.crates-io]`
in Termurion's Cargo.toml. The `examples/`, `tests/` and `docs/` directories and the
dev-dependencies are left out. Each changed site carries a `// Termurion patch:` comment. Changes:

1. Retina custom cursors on macOS. Stock winit gives a custom cursor's `NSImage` one point per
   image pixel, so an image drawn at 2x shows twice as big on a Retina screen. `CursorImage` has
   a `scale` (device pixels per point, 1.0 by default), `platform::macos` has
   `CustomCursorSourceExtMacOS::with_scale`, and `platform_impl/macos/cursor.rs` divides the
   image size and the hot spot by it. Other platforms ignore it.
2. System DLLs load from System32 only. `platform_impl/windows/util.rs` (`get_function_impl`)
   and `dark_mode.rs` (uxtheme.dll) call `LoadLibraryExW` with `LOAD_LIBRARY_SEARCH_SYSTEM32`
   instead of a bare `LoadLibraryA`, as Termurion's DLL rule asks (`src/dll.rs`).

To update winit, copy the new version here and apply these changes again.
