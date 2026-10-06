//! Prompt Before Closing: sessions whose profile asks, or that run jobs besides the
//! ignore list, need an OK before they close.

use super::*;
use crate::close_prompt;

/// What is closing, for the question's title.
pub(super) enum Closing {
    Session(PaneId),
    Tab(usize),
    Window,
    /// Quit: every session in every window. (A Windows shutdown asks the same through
    /// `resident_ui::shutdown_query`, on a thread of its own.)
    App,
}

impl App {
    /// Why closing these panes needs a confirmation (one line per reason).
    fn close_reasons(&self, panes: &[PaneId]) -> Vec<String> {
        let mut out = Vec::new();
        for id in panes {
            let Some(p) = self.panes.get(id) else { continue };
            if let Some(reason) = super::pane_lock_ui::close_reason_for(p.exited.is_some(), p.locked) {
                if !out.iter().any(|r| r == reason) {
                    out.push(reason.to_string());
                }
            }
            // What an exited shell left running ends with the pane (13q B7).
            let left = p.live_survivors();
            if !left.is_empty() {
                let reason = format!("{} still running after the session ended: {}.", if left.len() == 1 { "A program is".to_string() } else { format!("{} programs are", left.len()) }, left.join(", "));
                if !out.contains(&reason) {
                    out.push(reason);
                }
            }
            if !super::pane_lock_ui::profile_reasons_apply(p.exited.is_some(), p.locked) {
                continue;
            }
            let Some(profile) = self.store.get(&p.profile) else { continue };
            let mode = profile.int("Prompt Before Closing 2");
            // A framing session's programs are on its remote (a list that has not arrived is
            // not an error: the local tree answers).
            let listed = if mode == close_prompt::IF_JOBS {
                p.remote_process_names().map(Ok).or_else(|| p.shell_pid().map(|pid| crate::jobwatch::descendants_now(pid).map(|procs| procs.into_iter().map(|q| q.name).collect())))
            } else {
                None
            };
            let jobs = match listed {
                Some(Ok(names)) => names,
                // A tree that cannot be read is a reason to ask, with the error.
                Some(Err(e)) => {
                    out.push(format!("Tandem could not read the programs running in a session ({e})."));
                    continue;
                }
                None => Vec::new(),
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
            let Some(p) = self.panes.get(id) else { continue };
            // An ended pane's running programs are the survivors of its shell.
            if p.exited.is_some() {
                out.extend(p.live_survivors());
                continue;
            }
            if let Some(names) = p.remote_process_names() {
                out.extend(names);
                continue;
            }
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
        // A pinned tab always asks, and a window names each of its pinned tabs
        // (`PT:2366-2369`, `PT:4739-4741`).
        let (kind, title, panes, pinned) = match what {
            Closing::Session(id) => (Kind::Session, close_prompt::session_title(&self.session_title(id)), vec![id], Vec::new()),
            Closing::Tab(i) => {
                let pinned = if self.tabs.get(i).is_some_and(|t| t.pinned) { vec![close_prompt::pinned_reason(i + 1)] } else { Vec::new() };
                (Kind::Tab, close_prompt::tab_title(i + 1), self.tabs.get(i).map(|t| t.layout.panes()).unwrap_or_default(), pinned)
            }
            Closing::Window => {
                (Kind::Window, close_prompt::window_title(self.window_number() + 1), self.tabs.iter().flat_map(|t| t.layout.panes()).collect(), pinned_reasons(&self.tabs))
            }
            Closing::App => {
                let reasons = self.quit_reasons_now();
                if reasons.is_empty() {
                    return true;
                }
                return confirm(self.hwnd(), "Quit Tandem?", &close_rules::quit_text(&reasons, self.keep_jobs_on_quit()));
            }
        };
        let running = panes.iter().filter(|id| self.panes.get(id).is_some_and(|p| p.exited.is_none() || !p.live_survivors().is_empty())).count();
        let has_reason = !self.close_reasons(&panes).is_empty();
        if !close_rules::must_ask(kind, running, has_reason, self.globals.bool("OnlyWhenMoreTabs")) && pinned.is_empty() {
            return true;
        }
        let (names, notes) = self.job_names(&panes);
        let notes: Vec<String> = pinned.into_iter().chain(notes).collect();
        let body = close_rules::with_notes(close_rules::body(close_rules::subject(kind, panes.len(), panes.iter().any(|id| self.panes.get(id).is_some_and(|p| p.locked))), &close_rules::counted(&names)), &notes);
        confirm(self.hwnd(), &title, &body)
    }

    /// Why a quit asks (and so a shutdown): the sessions' reasons, the Closing settings and every
    /// window's pinned tabs. Reads only.
    pub(super) fn quit_reasons_now(&self) -> Vec<crate::close_rules::Reason> {
        use crate::close_rules;
        let panes: Vec<PaneId> = self.panes.keys().copied().collect();
        let prefs = close_rules::QuitPrefs {
            prompt_on_quit: self.globals.bool("PromptOnQuit"),
            even_if_no_windows: self.globals.bool("PromptOnQuitEvenIfThereAreNoWindows"),
            confirm_multiple: self.globals.bool("OnlyWhenMoreTabs"),
        };
        let mut reasons = close_rules::quit_reasons(&self.close_reasons(&panes), self.registry.order().len(), panes.len(), prefs, false);
        // Every window's pinned tabs, window by window in tab order (`POC:241-244`).
        reasons.extend(self.pinned_reasons_everywhere().into_iter().map(|text| close_rules::Reason { priority: close_prompt::PINNED_PRIORITY, kind: close_rules::ReasonKind::Message, text }));
        reasons
    }

    /// "Tab #N is pinned." for each pinned tab of every window, the windows in number order.
    fn pinned_reasons_everywhere(&self) -> Vec<String> {
        let current = self.windows.current();
        self.registry
            .order()
            .into_iter()
            .flat_map(|k| {
                if Some(k) == current {
                    pinned_reasons(&self.tabs)
                } else {
                    self.windows.parked().find(|(p, _)| **p == k).map(|(_, s)| pinned_reasons(&s.tabs)).unwrap_or_default()
                }
            })
            .collect()
    }
}

/// "Tab #N is pinned." for each pinned tab, in tab order.
fn pinned_reasons(tabs: &[Tab]) -> Vec<String> {
    tabs.iter().enumerate().filter(|(_, t)| t.pinned).map(|(i, _)| close_prompt::pinned_reason(i + 1)).collect()
}

/// The shutdown's quit question, shown by a thread that owns no window: an OK and Cancel box on
/// top and in front (no owner, so no window of the app thread is tied to it); true for OK.
pub(super) fn confirm_on_top(title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide(title).as_ptr(), MB_OKCANCEL | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST) == IDOK }
}

/// An OK and Cancel question; true for OK.
pub(super) fn confirm(owner: Option<isize>, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_OKCANCEL | MB_ICONWARNING) == IDOK }
}
