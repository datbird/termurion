//! Prompt Before Closing: sessions whose profile asks, or that run jobs besides the
//! ignore list, need an OK before they close.

use super::*;
use crate::close_prompt;

/// What is closing, for the question's title.
pub(super) enum Closing {
    Session(PaneId),
    Tab(usize),
    Window,
}

impl App {
    /// Why closing these panes needs a confirmation (one line per reason).
    fn close_reasons(&self, panes: &[PaneId]) -> Vec<String> {
        let mut out = Vec::new();
        for id in panes {
            let Some(p) = self.panes.get(id) else { continue };
            if p.exited.is_some() {
                continue;
            }
            let Some(profile) = self.store.get(&p.profile) else { continue };
            let mode = profile.int("Prompt Before Closing 2");
            let jobs = if mode == close_prompt::IF_JOBS {
                match p.shell_pid().map(crate::jobwatch::descendants_now) {
                    Some(Ok(procs)) => procs.into_iter().map(|q| q.name).collect(),
                    // A tree that cannot be read is a reason to ask, with the error.
                    Some(Err(e)) => {
                        out.push(format!("Tandem could not read the programs running in a session ({e})."));
                        continue;
                    }
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            if let Some(r) = close_prompt::close_reason(mode, &close_prompt::ignore_list(profile), &profile.name(), &jobs) {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        out
    }

    /// True when the panes may close: nothing to ask about, or the user chose OK.
    pub(super) fn confirm_close(&self, what: Closing) -> bool {
        let (title, panes) = match what {
            Closing::Session(id) => (close_prompt::session_title(&self.session_title(id)), vec![id]),
            Closing::Tab(i) => (close_prompt::tab_title(i + 1), self.tabs.get(i).map(|t| t.layout.panes()).unwrap_or_default()),
            Closing::Window => (close_prompt::window_title(self.window_number() + 1), self.tabs.iter().flat_map(|t| t.layout.panes()).collect()),
        };
        let reasons = self.close_reasons(&panes);
        if reasons.is_empty() {
            return true;
        }
        confirm(self.hwnd(), &title, &close_prompt::message(&reasons))
    }
}

/// An OK and Cancel question; true for OK.
fn confirm(owner: Option<isize>, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_OKCANCEL | MB_ICONWARNING) == IDOK }
}
