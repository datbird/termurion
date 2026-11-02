//! Windows folders.

/// `%USERPROFILE%` when it is not empty.
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}
