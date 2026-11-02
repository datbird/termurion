//! Windows folders.

/// `%USERPROFILE%` when it is not empty.
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}

/// The app's folder name under the base.
pub const APP_FOLDER: &str = "Termurion";

/// `%APPDATA%`, else the temp folder.
pub fn config_base() -> std::path::PathBuf {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// `%LOCALAPPDATA%`, else the temp folder.
pub fn local_base() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// A reported directory as a Windows path: "/C:/Users/rob" (OSC 7 URL form) or "C:\Users\rob".
/// None for a path that is not a Windows path (a remote Unix one).
pub fn local_path(reported: &str) -> Option<std::path::PathBuf> {
    let s = reported.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(reported);
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| std::path::PathBuf::from(s.replace('/', "\\")))
}
