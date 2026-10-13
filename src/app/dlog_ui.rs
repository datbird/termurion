//! Toggle Debug Logging (`DL:596-624`), its start at launch (`AD:1309-1324`, `:1356-1358`) and
//! the header and footer facts (`DL:98-150`). The file is written on a worker thread: a log of
//! up to 200 MB never stalls a frame, and the alert naming the file shows when it is done.

use super::scripts_alerts::{Then, ask};
use super::*;
use crate::dlog;
use crate::warnings::Kind;

/// Reads a text value of `HKEY_LOCAL_MACHINE` (a normal user may read these).
fn hklm_string(key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (key, name) = (wide(key), wide(name));
    let mut buf = [0u16; 256];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both names are NUL-terminated; `buf` has `bytes` bytes.
    let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let text = String::from_utf16_lossy(&buf[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The local time as "YYYY-MM-DD HH:MM:SS".
fn local_date() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: a zeroed out-structure the call fills.
    let t = unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// What Save Retrospective Debug Logs writes (`AD:3355-3357`): the header then the ring, no
/// footer; None when the ring is empty.
pub(super) fn retrospective_text(header: &str, ring: &str) -> Option<String> {
    (!ring.is_empty()).then(|| format!("{header}{ring}"))
}

impl App {
    /// Session > Toggle Debug Logging: off asks first, then starts; on stops and saves.
    pub(super) fn toggle_debug_logging(&mut self) {
        if dlog::is_on() {
            return self.stop_debug_logging(true);
        }
        self.ask_scripts(ask(dlog::ENABLED_HEADING, dlog::ENABLED_TEXT, &["OK"], None, Kind::Persistent, Then::DebugLogStart));
    }

    /// The "Debug Logging Enabled" alert was answered (or closed): logging starts.
    pub(super) fn debug_log_start_answered(&mut self) {
        if !dlog::is_on() {
            self.start_debug_logging();
        }
    }

    /// Termurion > Save Retrospective Debug Logs (`AD:3345-3373`): the recent low-frequency log,
    /// kept while debug logging was off, saved where the person picks.
    pub(super) fn save_retrospective_logs(&mut self) {
        let Some(text) = retrospective_text(&self.debug_log_header(), &dlog::ring_text()) else {
            return self.ask_scripts(ask(dlog::NO_RING_HEADING, dlog::NO_RING_TEXT, &["OK"], None, Kind::Persistent, Then::Acknowledged));
        };
        let Some(path) = crate::localfs::save_file(rfd::FileDialog::new().set_title(dlog::RING_SAVE_TITLE).set_file_name(dlog::RING_FILE)) else { return };
        if let Err(e) = crate::localfs::write(&path, text) {
            self.ask_scripts(ask(dlog::RING_SAVE_FAILED_HEADING, &dlog::ring_save_failed(&e.to_string()), &["OK"], None, Kind::Persistent, Then::Acknowledged));
        }
    }

    /// Starts the log with the header now. `LogToSyslog` is read as logging starts.
    fn start_debug_logging(&mut self) {
        dlog::set_syslog(self.globals.bool("LogToSyslog"));
        let header = self.debug_log_header();
        dlog::start_logging(header, self.globals.bool("AppendToExistingDebugLog"));
    }

    /// Stops and writes on a worker thread; `announce` shows "Debug Logging Stopped" when the
    /// file is written (a failure is always shown, `DL:228-234`).
    fn stop_debug_logging(&mut self, announce: bool) {
        let footer = self.debug_log_footer();
        let Some((header, body, append)) = dlog::stop() else { return };
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("debug-log-save".into()).spawn(move || {
            let result = dlog::save(&header, &body, &footer, append);
            let _ = proxy.lock().map(|p| p.send_event(UserEvent::DebugLogSaved(result, announce)));
        });
        if let Err(e) = spawned {
            self.debug_log_saved(Err(e.to_string()), announce);
        }
    }

    /// The worker finished: the alert names the file, or says why it was not written.
    pub(super) fn debug_log_saved(&mut self, result: Result<PathBuf, String>, announce: bool) {
        match result {
            Ok(path) if announce => self.ask_scripts(ask(dlog::STOPPED_HEADING, &dlog::stopped_text(&path), &["OK"], None, Kind::Persistent, Then::Acknowledged)),
            Ok(_) => {}
            Err(reason) => self.ask_scripts(ask(dlog::SAVE_PROBLEM_HEADING, &dlog::save_failed_text(&reason), &["OK"], None, Kind::Persistent, Then::Acknowledged)),
        }
    }

    /// `StartDebugLoggingAutomatically`: once per launch a notification says so, then logging
    /// starts without a question (`AD:1309-1324`).
    pub(super) fn start_debug_logging_automatically(&mut self) {
        if !self.globals.bool("StartDebugLoggingAutomatically") || self.debug_notice_shown || dlog::is_on() {
            return;
        }
        self.debug_notice_shown = true;
        match crate::notify::show_with(dlog::AUTO_TITLE, dlog::AUTO_DETAIL, self.globals.bool("SilentUserNotifications")) {
            Ok(()) => {
                self.last_notified_debug = Some(crate::notify::shown_count());
                self.last_notified = None;
                self.last_notified_script = None;
            }
            Err(e) => self.banners.set("notify", format!("Could not show a notification: {e}")),
        }
        self.start_debug_logging();
    }

    /// A click on that notification is iTerm2's "Disable" action: the key goes off and logging
    /// stops, saving silently.
    pub(super) fn disable_debug_logging_from_notice(&mut self) {
        self.last_notified_debug = None;
        self.globals.set("StartDebugLoggingAutomatically", toml::Value::Boolean(false));
        self.save_globals();
        self.stop_debug_logging(false);
    }

    /// What the header and the footer say (`DL:98-150`): Termurion's version, the machine, the
    /// monitors, Windows and the windows with counts. No title, folder, command or user name (P3).
    pub(super) fn debug_facts(&self) -> dlog::Facts {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let cpu = hklm_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default();
        let product = hklm_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName").unwrap_or_else(|| "Windows".into());
        let build = hklm_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild").unwrap_or_default();
        let screens = self
            .gpu
            .as_ref()
            .map(|g| g.window.available_monitors().map(|m| format!("{},{} {}x{} @ {:.2}", m.position().x, m.position().y, m.size().width, m.size().height, m.scale_factor())).collect())
            .unwrap_or_default();
        let current = self.windows.current();
        let mut windows = Vec::new();
        let mut order = Vec::new();
        for k in self.registry.order() {
            let Some(number) = self.registry.number(k) else { continue };
            let tabs = if current == Some(k) { Some(&self.tabs) } else { self.windows.parked().find(|(kk, _)| **kk == k).map(|(_, s)| &s.tabs) };
            if let Some(tabs) = tabs {
                windows.push(dlog::WindowFact { number, tabs: tabs.len(), panes: tabs.iter().map(|t| t.layout.panes().len()).sum() });
            }
            order.push(number);
        }
        dlog::Facts {
            version: env!("CARGO_PKG_VERSION").to_string(),
            date: local_date(),
            unix,
            machine: format!("{} {}", std::env::consts::ARCH, cpu).trim().to_string(),
            screens,
            os: format!("{product} build {build}").trim().to_string(),
            key_window: Some(self.window_number()).filter(|n| *n > 0),
            windows,
            order,
        }
    }

    pub(super) fn debug_log_header(&self) -> String {
        dlog::header(&self.debug_facts())
    }

    pub(super) fn debug_log_footer(&self) -> String {
        dlog::footer(&self.debug_facts())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AD:3345-3373`: an empty ring is nothing to save; else the header then the ring, no footer.
    #[test]
    fn retrospective_save_follows_iterm2() {
        assert_eq!(retrospective_text("HEAD\n", ""), None);
        assert_eq!(retrospective_text("HEAD\n", "one\ntwo\n"), Some("HEAD\none\ntwo\n".to_string()));
        assert!(!retrospective_text("H\n", "x\n").unwrap().contains("FOOTER"));
        let src = include_str!("dlog_ui.rs");
        assert!(src.contains("dlog::NO_RING_HEADING") && src.contains("dlog::RING_SAVE_TITLE") && src.contains("dlog::RING_FILE") && src.contains("dlog::RING_SAVE_FAILED_HEADING"));
    }
}
