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

/// Shows `path` selected in the file manager (Explorer on Windows); nothing runs it. The caller passes
/// a path Termurion made or judged. Unix reports Unsupported until a file manager call exists (P1).
pub fn reveal(path: &str) -> Result<(), String> {
    imp::reveal(path)
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
