//! Opening a link of another app's scheme (`vscode:`, `slack:`; TWEAKS 6). The one caller is
//! `execute_with`'s `Run::OpenScheme`; `link_scheme` decides, this asks once per scheme (Always,
//! Once, Never), remembers Always and Never in the settings, and launches through `shell_open_scheme`.

use super::*;

/// The registered app for one scheme and the files it names, read from the system registry on demand
/// (nothing is read until `link_scheme::plan` asks).
struct Live {
    reg: Box<dyn crate::url_schemes::SchemeRegistry>,
    scheme: String,
}

impl Live {
    fn new(scheme: &str) -> Self {
        Live { reg: crate::url_schemes::system(), scheme: scheme.to_string() }
    }
}

impl crate::link_scheme::Source for Live {
    fn name(&self) -> Option<String> {
        self.reg.handler_name(&self.scheme)
    }
    fn command(&self) -> Option<String> {
        self.reg.handler_command(&self.scheme)
    }
    fn exe(&self) -> Option<String> {
        self.reg.handler_exe(&self.scheme)
    }
    fn is_local_drive(&self, exe: &str) -> bool {
        self.reg.is_local_drive(exe)
    }
    fn long_path(&self, exe: &str) -> String {
        self.reg.long_path(exe)
    }
    fn version_names(&self, exe: &str) -> Option<Vec<String>> {
        self.reg.version_names(exe)
    }
    fn trusted_folders(&self) -> crate::link_scheme::TrustedFolders {
        self.reg.trusted_folders()
    }
}

impl App {
    /// A terminal link of another app's scheme was clicked: refused, opened (Always), or asked.
    pub(super) fn open_scheme_link(&mut self, url: &str, background: bool) {
        let Some(scheme) = crate::link_scheme::scheme_of(url) else { return };
        let plan = crate::link_scheme::plan(url, &Live::new(&scheme), &self.globals.string(crate::link_scheme::ALWAYS_KEY), &self.globals.string(crate::link_scheme::NEVER_KEY));
        match plan {
            crate::link_scheme::Plan::Refuse(why) => self.banners.set("action", why),
            crate::link_scheme::Plan::Open => self.launch_scheme(url, background),
            crate::link_scheme::Plan::Ask { app } => {
                // One question per scheme at a time.
                let waiting = self.alert_waits(|t| matches!(t, super::scripts_alerts::Then::LinkScheme { scheme: s, .. } if *s == scheme));
                if !waiting {
                    let then = super::scripts_alerts::Then::LinkScheme { url: url.to_string(), scheme: scheme.clone(), background };
                    self.ask_scripts(super::scripts_alerts::Ask::new(crate::link_scheme::question(&scheme, &app), then));
                }
            }
        }
        self.redraw();
    }

    /// The answer: Always (0) and Never (2) are remembered for the scheme; Always and Once open.
    pub(super) fn link_scheme_answered(&mut self, url: &str, scheme: &str, background: bool, index: usize) {
        match index {
            0 | 1 => {
                if index == 0 {
                    self.remember_scheme(crate::link_scheme::ALWAYS_KEY, scheme);
                }
                // Judged again at the answer: the registered app may have changed since the question.
                match crate::link_scheme::plan(url, &Live::new(scheme), scheme, "") {
                    crate::link_scheme::Plan::Open => self.launch_scheme(url, background),
                    crate::link_scheme::Plan::Refuse(why) => self.banners.set("action", why),
                    crate::link_scheme::Plan::Ask { .. } => {}
                }
            }
            2 => self.remember_scheme(crate::link_scheme::NEVER_KEY, scheme),
            _ => {}
        }
        self.redraw();
    }

    /// Adds `scheme` to the list setting `key` (the one settings writer: `save_globals`).
    fn remember_scheme(&mut self, key: &str, scheme: &str) {
        let list = crate::link_scheme::added(&self.globals.string(key), scheme);
        self.globals.set(key, toml::Value::String(list));
        self.save_globals();
    }

    fn launch_scheme(&mut self, url: &str, background: bool) {
        if let Err(e) = super::semantic_ui::shell_open_scheme(url, background) {
            self.banners.set("action", format!("Could not open the link: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    /// TWEAKS 6 wiring: the one caller, the input guard, the settings writer, a closed window opens nothing.
    #[test]
    fn the_scheme_road_is_wired() {
        let sem = include_str!("semantic_ui.rs");
        let exec = &sem[sem.find("pub(super) fn execute_with").unwrap()..];
        assert!(exec.contains("Run::OpenScheme(url) => self.open_scheme_link(&url, background)"));
        let open = &sem[sem.find("pub(super) fn shell_open_scheme").unwrap()..];
        let open = &open[..open.find("\n}\n").unwrap()];
        assert!(open.contains("link_scheme::scheme_of(") && open.contains("link_scheme::screen(") && open.contains("encode_url_chars("), "the sink judges the link again");
        assert!(open.contains("ShellExecuteW") && !open.contains("cmd") && !open.contains("Command::new"), "never a shell string");
        assert!(sem.matches("shell_open_scheme(").count() >= 2 && include_str!("link_scheme_app.rs").matches("shell_open_scheme(").count() >= 1);
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::BrowserPermission(_) | Then::BrowserClearData { .. } | Then::BrowserExtension { .. } | Then::ConfirmSigned | Then::AllowAutoLaunch(_) | Then::LinkScheme { .. }"), "input guard");
        assert!(alerts.contains("Then::LinkScheme { url, scheme, background } => self.link_scheme_answered(&url, &scheme, background, index)"));
        assert!(!alerts.contains("Then::LinkScheme { .. } => Answer::Button"), "a closed window answers nothing");
        let me = include_str!("link_scheme_app.rs");
        assert!(me.contains("self.save_globals()"), "one settings writer");
    }
}
