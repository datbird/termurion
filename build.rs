//! Copies the shipped ConPTY next to the built exe, where the app looks for it, and gathers the
//! license files of every bundled crate into OUT_DIR/licenses.txt (`src/licenses.rs`).
use std::path::{Path, PathBuf};

fn main() {
    // Load-time DLL safety, kept here as well as in .cargo/config.toml so `cargo install`, another
    // target or an env RUSTFLAGS cannot drop it: every import resolves in System32 only, and the C
    // runtime is linked in (no vcruntime140.dll lookup before main).
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows") && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc") {
        if !std::env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|v| v.split(',').any(|f| f == "crt-static")) {
            panic!("Termurion must be built with crt-static (-C target-feature=+crt-static): see .cargo/config.toml");
        }
        println!("cargo:rustc-link-arg=/DEPENDENTLOADFLAG:0x800");
        #[cfg(windows)]
        embed_resource();
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
    write_licenses(&out);
}

/// One list entry, as `licenses::entry` writes it ("crate version/FILE", then the text).
fn entry(name: &str, version: &str, file: &str, text: &str) -> String {
    format!("{name} {version}/{file}\n{text}\n\n")
}

/// Every notice file under `resources/` (LICENSE*, LICENCE*, COPYING*, NOTICE*, in any folder), by
/// path ("resources/ssh/COPYING-iTerm2"); files with the same text share one entry that names
/// them all. These cover the iTerm2 files, the tab icons, the terminfo entries and the ConPTY pair.
fn resource_notices(root: &Path) -> String {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| ["LICENSE", "LICENCE", "COPYING", "NOTICE"].iter().any(|x| n.to_ascii_uppercase().starts_with(x))) {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for f in files {
        let Some(bytes) = std::fs::read(&f).ok().filter(|b| b.len() <= 256 * 1024) else { continue };
        let body = String::from_utf8_lossy(&bytes).trim().to_string();
        let name = f.to_string_lossy().replace('\\', "/");
        match groups.iter_mut().find(|(b, _)| *b == body) {
            Some((_, names)) => names.push(name),
            None => groups.push((body, vec![name])),
        }
    }
    groups.iter().map(|(body, names)| format!("{}\n{body}\n\n", names.join(", "))).collect()
}

/// The (name, version, from the registry) of every package in Cargo.lock's text.
fn packages(lock: &str) -> Vec<(String, String, bool)> {
    let mut v = Vec::new();
    let (mut name, mut version, mut registry) = (String::new(), String::new(), false);
    let mut flush = |name: &mut String, version: &mut String, registry: &mut bool| {
        if !name.is_empty() && !version.is_empty() {
            v.push((std::mem::take(name), std::mem::take(version), *registry));
        }
        name.clear();
        version.clear();
        *registry = false;
    };
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            flush(&mut name, &mut version, &mut registry);
        } else if let Some(r) = line.strip_prefix("name = ") {
            name = r.trim_matches('"').to_string();
        } else if let Some(r) = line.strip_prefix("version = ") {
            version = r.trim_matches('"').to_string();
        } else if let Some(r) = line.strip_prefix("source = ") {
            registry = r.trim_matches('"').starts_with("registry+");
        }
    }
    flush(&mut name, &mut version, &mut registry);
    v
}

/// The license-like files in a crate's top folder (LICENSE*, LICENCE*, COPYING*, NOTICE*), by name.
fn license_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| ["LICENSE", "LICENCE", "COPYING", "NOTICE"].iter().any(|p| n.to_ascii_uppercase().starts_with(p)))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            // A license is a few KiB; a larger file is not one.
            let bytes = std::fs::read(dir.join(&n)).ok().filter(|b| b.len() <= 256 * 1024)?;
            Some((n, String::from_utf8_lossy(&bytes).trim().to_string()))
        })
        .collect()
}

/// The `license` line of a crate's Cargo.toml, for a crate with no license file.
fn spdx_of(dir: &Path) -> Option<String> {
    let toml = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    toml.lines().map(str::trim).find_map(|l| l.strip_prefix("license = ").map(|r| r.trim_matches('"').to_string()))
}

/// Cargo's registry source folders: `$CARGO_HOME` else `%USERPROFILE%\.cargo`, then `registry/src/*`.
fn registry_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".cargo")));
    let Some(src) = home.map(|h| h.join("registry").join("src")) else { return Vec::new() };
    let mut roots: Vec<PathBuf> = std::fs::read_dir(src).into_iter().flatten().flatten().map(|e| e.path()).collect();
    roots.sort();
    roots
}

/// Gathers the list. Reads only Cargo.lock, the registry sources, the path crates under crates/
/// and resources/; a crate whose folder is absent (a dependency for another target that cargo
/// never downloaded here) is skipped without an error.
fn write_licenses(out: &Path) {
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=crates");
    println!("cargo:rerun-if-changed=resources");
    println!("cargo:rerun-if-env-changed=CARGO_HOME");
    let lock = std::fs::read_to_string("Cargo.lock").unwrap_or_default();
    let roots = registry_roots();
    let mut pkgs = packages(&lock);
    pkgs.sort();
    pkgs.dedup();
    let mut text = String::new();
    for (name, version, registry) in pkgs {
        let dir = if registry {
            roots.iter().map(|r| r.join(format!("{name}-{version}"))).find(|d| d.is_dir())
        } else {
            Some(Path::new("crates").join(&name)).filter(|d| d.is_dir())
        };
        let Some(dir) = dir else { continue };
        let files = license_files(&dir);
        if files.is_empty()
            && let Some(spdx) = spdx_of(&dir) {
                text.push_str(&entry(&name, &version, "license", &spdx));
            }
        for (file, body) in files {
            text.push_str(&entry(&name, &version, &file, &body));
        }
    }
    text.push_str(&resource_notices(Path::new("resources")));
    std::fs::write(out.join("licenses.txt"), text).unwrap();
}

/// The exe icon and version resource: the Termurion art, and the names Explorer and the taskbar show.
#[cfg(windows)]
fn embed_resource() {
    println!("cargo:rerun-if-changed=docs/assets/termurion/termurion.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("docs/assets/termurion/termurion.ico");
    res.set("ProductName", "Termurion");
    res.set("FileDescription", "Termurion");
    res.set("InternalName", "termurion");
    res.set("OriginalFilename", "termurion.exe");
    res.compile().expect("compile the Windows resource (needs the Windows SDK rc.exe)");
}
