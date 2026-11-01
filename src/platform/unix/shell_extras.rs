//! Windows-only extras: a Jump List, a taskbar badge and DLL search hardening have no Unix meaning,
//! so each call succeeds without doing anything.

pub fn jump_list(_m: &crate::jumplist::Model, _exe: &std::path::Path) -> Result<Vec<Vec<String>>, String> {
    Ok(Vec::new())
}

pub fn jump_list_clear() -> Result<(), String> {
    Ok(())
}

pub fn taskbar_progress(_hwnd: isize, _label: &str) -> Result<(), String> {
    Ok(())
}

pub fn taskbar_watch(_hwnd: isize) {}

pub fn taskbar_clear_all() {}

pub fn dll_hardening() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_shell_extras_are_noops() {
        assert!(jump_list_clear().is_ok());
        assert!(taskbar_progress(1, "3").is_ok());
        assert!(dll_hardening().is_ok());
        taskbar_watch(1);
        taskbar_clear_all();
    }
}
