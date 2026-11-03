//! ShellExecute for a judged URL (moved from `app/semantic_ui.rs` as is).

/// Explorer.
pub const FILE_MANAGER: &str = "Explorer";

pub fn open_url(url: &str, background: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), show) };
    // Values of 32 or less are errors (SE_ERR_*).
    if r as isize > 32 { Ok(()) } else { Err(format!("ShellExecute error {}", r as isize)) }
}

/// Programs and scripts: the one dangerous-type list and the machine's PATHEXT.
pub fn runnable(path: &str) -> bool {
    crate::inert_files::high_risk(path)
}

/// `link_scheme::BLOCKED` is Windows' whole list.
pub fn scheme_refusal(_scheme: &str) -> Option<&'static str> {
    None
}

/// Only an empty host and `localhost` are local here.
pub fn is_this_host(_host: &str) -> bool {
    false
}

/// A decoded `file:` link path as a Windows path: "/C:/x" is "C:\x", every `/` a `\`.
pub fn file_url_os_path(path: String) -> Option<String> {
    let path = match path.strip_prefix('/') {
        Some(p) if p.as_bytes().get(1) == Some(&b':') => p.to_string(),
        _ => path,
    };
    Some(path.replace('/', "\\"))
}

/// ShellExecuteW "open" on a judged file.
pub fn open_file(path: &std::path::Path, background: bool) -> Result<(), String> {
    shell_verb("open", path, None, background)
}

/// ShellExecuteW `verb` on a judged file, with a judged working folder; the file is one argument.
pub fn shell_verb(verb: &str, path: &std::path::Path, cwd: Option<&std::path::Path>, background: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide(verb.as_ref()), wide(path.as_os_str()));
    let dir = cwd.map(|d| wide(d.as_os_str()));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()), show) };
    // Values of 32 or less are errors (SE_ERR_*).
    if r as isize > 32 { Ok(()) } else { Err(format!("ShellExecute error {}", r as isize)) }
}

/// Explorer's `/select,` argument for `path`. Callers hold paths with either slash; Explorer
/// wants backslashes, so the backend turns every `/` into `\` for all of them.
fn select_arg(path: &str) -> Option<String> {
    crate::semantic::explorer_select_arg(&path.replace('/', "\\"))
}

/// Explorer from the Windows folder (never from PATH) with the one select argument
/// (`semantic::explorer_select_arg`, which refuses quotes, control characters and network paths).
pub fn reveal(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let program = crate::script_provision::system_directory().and_then(|s| crate::semantic::explorer_in(&s)).ok_or("the Windows folder is unknown")?;
    let arg = select_arg(path).ok_or("Termurion does not show this path in Explorer")?;
    let mut cmd = std::process::Command::new(program);
    cmd.raw_arg(arg);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Shows the finished items together in Explorer: one path selected in its folder, several
/// selected together in their common folder (`OP:67-80`, with the arrived ones only).
pub fn reveal_all(paths: &[std::path::PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFindLastID, ILFree, SHOpenFolderAndSelectItems};
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
    match paths {
        [] => Ok(()),
        [one] => {
            // The same Explorer the Win-click's reveal runs, found through the system directory.
            let program = crate::script_provision::system_directory().and_then(|s| crate::semantic::explorer_in(&s)).ok_or("the Windows folder is unknown")?;
            let arg = select_arg(&one.to_string_lossy()).ok_or("the file's name cannot be shown in Explorer")?;
            let mut cmd = std::process::Command::new(program);
            cmd.raw_arg(arg);
            cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
        }
        many => {
            let folder = many[0].parent().ok_or("no folder")?;
            if many.iter().any(|p| p.parent() != Some(folder)) {
                return Err("the items are not in one folder".into());
            }
            // SAFETY: each list comes from ILCreateFromPathW and is freed once; the child ids
            // point into their lists, which outlive the call.
            unsafe {
                let parent = ILCreateFromPathW(wide(folder).as_ptr());
                if parent.is_null() {
                    return Err("the folder could not be found".into());
                }
                let lists: Vec<*mut _> = many.iter().map(|p| ILCreateFromPathW(wide(p).as_ptr())).filter(|l| !l.is_null()).collect();
                let children: Vec<*const _> = lists.iter().map(|l| ILFindLastID(*l) as *const _).collect();
                let mut hr = if children.is_empty() { -1 } else { SHOpenFolderAndSelectItems(parent, children.len() as u32, children.as_ptr(), 0) };
                // winit's drop target starts OLE on the UI thread; if COM is not up, start it once.
                if hr == 0x800401F0u32 as i32 {
                    windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32);
                    hr = SHOpenFolderAndSelectItems(parent, children.len() as u32, children.as_ptr(), 0);
                }
                for l in lists {
                    ILFree(l);
                }
                ILFree(parent);
                if hr < 0 { Err(format!("Explorer answered 0x{:08X}", hr as u32)) } else { Ok(()) }
            }
        }
    }
}

/// ShellExecuteW with the one file argument, never a shell string.
pub fn open_scheme(encoded_url: &str, background: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(encoded_url));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), show) };
    if r as isize > 32 { Ok(()) } else { Err(format!("ShellExecute error {}", r as isize)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reveal caller's path reaches Explorer with backslashes (the backend owns the
    /// conversion, Task 6 ruling), and the select argument's own refusals still hold.
    #[test]
    fn reveal_turns_every_slash_into_a_backslash() {
        assert_eq!(select_arg("C:/Users/x/f.txt").as_deref(), Some(r#"/select,"C:\Users\x\f.txt""#));
        assert_eq!(select_arg(r"C:\Users\x/sub/f.txt").as_deref(), Some(r#"/select,"C:\Users\x\sub\f.txt""#));
        assert_eq!(select_arg(r"C:\a\b.txt").as_deref(), Some(r#"/select,"C:\a\b.txt""#));
        assert_eq!(select_arg("//server/share/f.txt"), None, "a network path, either slash");
        assert_eq!(select_arg("C:/a\"b"), None);
    }
}
