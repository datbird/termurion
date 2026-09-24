//! Copies the shipped ConPTY next to the built exe, where the app looks for it.
fn main() {
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
