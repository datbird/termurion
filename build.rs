//! Copies the shipped ConPTY next to the built exe, where the app looks for it.
fn main() {
    // Load-time DLL safety, kept here as well as in .cargo/config.toml so `cargo install`, another
    // target or an env RUSTFLAGS cannot drop it: every import resolves in System32 only, and the C
    // runtime is linked in (no vcruntime140.dll lookup before main).
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows") && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc") {
        if !std::env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|v| v.split(',').any(|f| f == "crt-static")) {
            panic!("Tandem must be built with crt-static (-C target-feature=+crt-static): see .cargo/config.toml");
        }
        println!("cargo:rustc-link-arg=/DEPENDENTLOADFLAG:0x800");
    }
    println!("cargo:rerun-if-changed=vendor/conpty");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out; the exe is in target/<profile>.
    let profile_dir = out.ancestors().nth(3).unwrap();
    for file in ["conpty.dll", "OpenConsole.exe"] {
        let src = std::path::Path::new("vendor").join("conpty").join(file);
        if src.exists() {
            std::fs::copy(&src, profile_dir.join(file)).unwrap();
        } else {
            println!("cargo:warning=vendor/conpty/{file} is missing. Run scripts/fetch-conpty.ps1.");
        }
    }
}
