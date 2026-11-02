//! ShellExecute for a judged URL (moved from `app/semantic_ui.rs` as is).

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

/// Explorer from the Windows folder (never from PATH) with the one select argument
/// (`semantic::explorer_select_arg`, which refuses quotes, control characters and network paths).
pub fn reveal(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let program = crate::script_provision::system_directory().and_then(|s| crate::semantic::explorer_in(&s)).ok_or("the Windows folder is unknown")?;
    let arg = crate::semantic::explorer_select_arg(path).ok_or("Termurion does not show this path in Explorer")?;
    let mut cmd = std::process::Command::new(program);
    cmd.raw_arg(arg);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}
