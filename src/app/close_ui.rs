//! Prompt Before Closing: sessions whose profile asks, or that run jobs besides the
//! ignore list, need an OK before they close.

use super::*;
use crate::close_prompt;

/// What is closing, for the question's title.
pub(super) enum Closing {
    Session(PaneId),
    Tab(usize),
    Window,
    /// Quit: every session in every window.
    App,
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

    /// The programs running under the panes' shells (not the shells), for the question, and
    /// a note for each shell whose programs could not be read.
    fn job_names(&self, panes: &[PaneId]) -> (Vec<String>, Vec<String>) {
        let (mut out, mut notes) = (Vec::new(), Vec::new());
        for id in panes {
            let Some(p) = self.panes.get(id).filter(|p| p.exited.is_none()) else { continue };
            match p.shell_pid().map(crate::jobwatch::descendants_now) {
                Some(Ok(procs)) => out.extend(procs.iter().map(|q| crate::proctree::strip_exe(&q.name).to_string()).filter(|n| !n.eq_ignore_ascii_case("conhost") && !n.eq_ignore_ascii_case("OpenConsole"))),
                Some(Err(e)) => notes.push(format!("Tandem could not read the programs running in a session ({e}).")),
                None => {}
            }
        }
        (out, notes)
    }

    /// True when the panes may close: nothing to ask about, or the user chose OK. A
    /// session alone in its tab closes as its tab (iTerm2).
    pub(super) fn confirm_close(&self, what: Closing) -> bool {
        use crate::close_rules::{self, Kind};
        let tab_of = |id: PaneId| self.tabs.iter().position(|t| t.layout.panes().contains(&id));
        let what = match what {
            Closing::Session(id) if tab_of(id).is_some_and(|i| self.tabs[i].layout.panes().len() == 1) => Closing::Tab(tab_of(id).unwrap_or(0)),
            w => w,
        };
        let (kind, title, panes) = match what {
            Closing::Session(id) => (Kind::Session, close_prompt::session_title(&self.session_title(id)), vec![id]),
            Closing::Tab(i) => (Kind::Tab, close_prompt::tab_title(i + 1), self.tabs.get(i).map(|t| t.layout.panes()).unwrap_or_default()),
            Closing::Window => (Kind::Window, close_prompt::window_title(self.window_number() + 1), self.tabs.iter().flat_map(|t| t.layout.panes()).collect()),
            Closing::App => {
                let panes: Vec<PaneId> = self.panes.keys().copied().collect();
                let prefs = close_rules::QuitPrefs {
                    prompt_on_quit: self.globals.bool("PromptOnQuit"),
                    even_if_no_windows: self.globals.bool("PromptOnQuitEvenIfThereAreNoWindows"),
                    confirm_multiple: self.globals.bool("OnlyWhenMoreTabs"),
                };
                let reasons = close_rules::quit_reasons(&self.close_reasons(&panes), self.registry.order().len(), panes.len(), prefs, false);
                if reasons.is_empty() {
                    return true;
                }
                return confirm(self.hwnd(), "Quit Tandem?", &close_rules::quit_text(&reasons, self.keep_jobs_on_quit()));
            }
        };
        let running = panes.iter().filter(|id| self.panes.get(id).is_some_and(|p| p.exited.is_none())).count();
        let has_reason = !self.close_reasons(&panes).is_empty();
        if !close_rules::must_ask(kind, running, has_reason, self.globals.bool("OnlyWhenMoreTabs")) {
            return true;
        }
        let (names, notes) = self.job_names(&panes);
        let body = close_rules::with_notes(close_rules::body(close_rules::subject(kind, panes.len()), &close_rules::counted(&names)), &notes);
        confirm(self.hwnd(), &title, &body)
    }
}

/// An OK and Cancel question; true for OK.
pub(super) fn confirm(owner: Option<isize>, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_OKCANCEL | MB_ICONWARNING) == IDOK }
}
