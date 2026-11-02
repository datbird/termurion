//! Opening a link with the OS default app. The one place a URL reaches the shell.

#[cfg(unix)]
use super::unix::shell_open as imp;
#[cfg(windows)]
use super::windows::shell_open as imp;

/// Opens a URL with its default app. Only a URL (`semantic::shell_url`): a path, a drive path or a
/// file link is refused, so no caller hands the shell an unjudged file; a file opens through
/// `localfs::open_with_shell` with a judged `LocalPath`.
pub fn open(target: &str) -> Result<(), String> {
    open_with(target, false)
}

/// As `open`; `background` asks the app not to take the focus.
pub fn open_with(target: &str, background: bool) -> Result<(), String> {
    let Some(url) = crate::semantic::shell_url(target) else { return Err("Termurion opens only links here, not paths.".to_string()) };
    imp::open_url(&url, background)
}

/// Hands an already judged and percent-encoded link of another app's scheme to the app the OS
/// registered for it (`background` asks it not to take the focus). Only `semantic_ui::shell_open_scheme`
/// calls it, after `link_scheme::screen`.
pub fn open_scheme(encoded_url: &str, background: bool) -> Result<(), String> {
    imp::open_scheme(encoded_url, background)
}

/// Opens a judged file or folder with its default app (`background` asks it not to take the focus). Only
/// `localfs::open_with_shell` calls it, with a `LocalPath`.
pub fn open_file(path: &std::path::Path, background: bool) -> Result<(), String> {
    imp::open_file(path, background)
}

/// ShellExecute `verb` ("open", "edit") on a judged file, with a judged working folder. Only
/// `localfs::shell_verb` calls it.
#[cfg(windows)]
pub fn shell_verb(verb: &str, path: &std::path::Path, cwd: Option<&std::path::Path>, background: bool) -> Result<(), String> {
    imp::shell_verb(verb, path, cwd, background)
}

/// Shows `path` selected in the file manager (Explorer on Windows); nothing runs it. The caller passes
/// a path Termurion made or judged.
pub fn reveal(path: &str) -> Result<(), String> {
    imp::reveal(path)
}

/// Shows the finished items together in the file manager: one path selected in its folder, several
/// selected together in their common folder. Nothing runs them; the caller passes paths Termurion made.
pub fn reveal_all(paths: &[std::path::PathBuf]) -> Result<(), String> {
    imp::reveal_all(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_open_refuses_empty_target() {
        assert!(open("").is_err());
    }

    #[test]
    fn shell_open_refuses_a_path() {
        assert!(open("/etc/passwd").is_err());
        assert!(open(r"C:\x.txt").is_err());
    }
}
