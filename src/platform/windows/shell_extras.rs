//! The Jump List (`jumplist_win`), the taskbar badge (`taskbar_win`) and DLL hardening (`dll`).

/// The COM writer, except in a test build, which never writes the real Jump List.
pub fn jump_list(m: &crate::jumplist::Model, exe: &std::path::Path) -> Result<Vec<Vec<String>>, String> {
    #[cfg(not(test))]
    {
        crate::jumplist_win::commit(m, exe)
    }
    #[cfg(test)]
    {
        let _ = (m, exe);
        Ok(Vec::new())
    }
}

pub fn jump_list_clear() -> Result<(), String> {
    #[cfg(not(test))]
    {
        crate::jumplist_win::clear_recent_destinations()
    }
    #[cfg(test)]
    {
        Ok(())
    }
}

pub fn taskbar_progress(hwnd: isize, label: &str) -> Result<(), String> {
    crate::taskbar_win::set_badge(hwnd, label)
}

pub fn taskbar_watch(hwnd: isize) {
    crate::taskbar_win::watch(hwnd)
}

pub fn taskbar_clear_all() {
    crate::taskbar_win::clear_all()
}

pub fn dll_hardening() -> Result<(), String> {
    if crate::dll::harden_process() {
        Ok(())
    } else {
        Err("the DLL search is not fully restricted".to_string())
    }
}

pub fn opt_out_of_power_throttling() -> Result<(), String> {
    crate::power::opt_out_of_power_throttling()
}

pub fn register_open_with(exe: &std::path::Path) -> Result<(), String> {
    use crate::reg::Registry;
    let entries = crate::open_file::open_with_entries(exe);
    let mut reg = crate::reg::HkcuRegistry;
    if reg.get(&entries[1].0, &entries[1].1).as_deref() == Some(entries[1].2.as_str()) {
        return Ok(());
    }
    crate::reg::apply(&mut reg, &entries).map(|_| ())
}
