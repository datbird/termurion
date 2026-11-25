//! Termurion patch 9: the core is GPU-free. Every core file is scanned for the GPU API's name.

/// The files that make up the GPU-free core.
const CORE: [(&str, &str); 6] = [
    ("lib.rs", include_str!("lib.rs")),
    ("custom_glyph.rs", include_str!("custom_glyph.rs")),
    ("error.rs", include_str!("error.rs")),
    ("text_atlas.rs", include_str!("text_atlas.rs")),
    ("text_render.rs", include_str!("text_render.rs")),
    ("row_render.rs", include_str!("row_render.rs")),
];

#[test]
fn core_names_no_wgpu() {
    let gpu = concat!("wg", "pu");
    let gate = concat!("feature = \"", "wg", "pu\"");
    for (file, src) in CORE {
        // The crate root wires the backend in: a line gated on the feature, and the item after it.
        let mut gated = false;
        for (n, line) in src.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("#[cfg(") && line.contains(gate) {
                gated = true;
                continue;
            }
            assert!(gated || !line.contains(gpu), "{file}:{} names {gpu}: {line}", n + 1);
            gated = false;
        }
    }
}

/// Files that may name it: the backend and its tests. Any other file in `src` must be in `CORE`.
const BACKEND: [&str; 6] = ["cache.rs", "upload.rs", "viewport.rs", "wgpu_backend.rs", "test_gpu.rs", "core_tests.rs"];

#[test]
fn every_source_file_is_core_or_backend() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".rs") {
            assert!(CORE.iter().any(|(f, _)| *f == name) || BACKEND.contains(&name.as_str()), "{name} is neither core nor backend");
        }
    }
}
