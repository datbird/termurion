//! What each content component on a status bar shows (milestone 9h): a component and its
//! session become a `Content` (width variants, copy text, empty, icon, click), which
//! `statusbar_ui` measures, lays out and draws. Text components here: Clock, Host Name,
//! User Name, Job Name, Current Directory (and its click menu), Interpolated String.
//!
//! Citations use `.superpowers/research/m9-facts.md` section 2's abbreviations (`TXT` =
//! `iTermStatusBarTextComponent.m`, `VAR` = `iTermStatusBarVariableBaseComponent.m`, `JOB`
//! = `iTermStatusBarJobComponent.m`, `CLK` = `iTermStatusBarClockComponent.m`, `SWS` =
//! `iTermStatusBarSwiftyStringComponent.m`).

use super::statusbar_ui::Click;
use super::*;
use crate::broadcast::Write;
use crate::statusbar::Component;
use crate::statusbar_components::{self as sbc, Kind};
use crate::statusbar_text as sbt;

const FORMAT_KEY: &str = "format";
const LOCALIZE_KEY: &str = "localize";
/// `CLK`: the format when the knob is absent.
const DEFAULT_CLOCK_FORMAT: &str = "M/dd h:mm";
const ABBREVIATE_LOCALHOST_KEY: &str = "abbreviate-localhost";
const EXPRESSION_KEY: &str = "expression";
/// `SWS:130`: what an Interpolated String shows when its expression fails.
pub(super) const LADYBUG: &str = "\u{1F41E}";

/// What one content component shows and does now.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Content {
    /// Width variants (`stringVariants`), in the component's own order; the widest one
    /// that fits is drawn (`TXT:178-229`).
    pub(super) variants: Vec<String>,
    /// User Name: the minimum width is the narrower of the full name and this; the full
    /// name is always drawn (`VAR:234-247`).
    pub(super) min_text: Option<String>,
    /// The right-click Copy text when it is not the widest variant (Current Directory's
    /// raw path, `VAR:314-316`).
    pub(super) copy: Option<String>,
    /// `statusBarComponentIsEmpty`.
    pub(super) empty: bool,
    /// A Segoe Fluent Icons glyph in a 17 point column at the left.
    pub(super) icon: Option<char>,
    /// The text sits at the right of the component's room.
    pub(super) right_align: bool,
    /// A component sized by numbers, not text: (minimum, preferred) points.
    pub(super) fixed: Option<(f32, f32)>,
    pub(super) click: Click,
}

impl Content {
    /// A text component with these variants: empty when the widest is "" (`TXT`); no
    /// variants at all is one empty variant.
    pub(super) fn text(variants: Vec<String>) -> Content {
        let variants = if variants.is_empty() { vec![String::new()] } else { variants };
        let empty = variants.iter().all(|v| v.is_empty());
        Content { variants, min_text: None, copy: None, empty, icon: None, right_align: false, fixed: None, click: Click::None }
    }
}

/// A knob's string value, else its default.
fn knob_str(c: &Component, kind: Kind, key: &str) -> Option<String> {
    c.knobs.get(key).or(sbc::default_knobs(kind).get(key)).and_then(toml::Value::as_str).map(str::to_string)
}

/// A knob's checkbox value, else `default`.
fn knob_bool(c: &Component, key: &str, default: bool) -> bool {
    c.knobs.get(key).and_then(toml::Value::as_bool).unwrap_or(default)
}

/// `JOB:109-149`: the deepest job first, then its parents. Tandem's ancestors are the job
/// and its parents below the shell, deepest first, so the job's own name comes first and
/// the ancestors after their first entry follow. Empty before the session has a job.
pub(super) fn job_chain(job: Option<&str>, ancestors: &[String]) -> Vec<String> {
    let Some(name) = job else { return Vec::new() };
    std::iter::once(name.to_string()).chain(ancestors.iter().skip(1).cloned()).collect()
}

/// The last component of a path, either separator (`lastPathComponent`).
pub(super) fn folder_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or(trimmed).to_string()
}

/// A recent directory's menu title (`VAR:361-369`): starred ones are marked.
pub(super) fn dir_title(path: &str, starred: bool) -> String {
    if starred { format!("\u{2605} {path}") } else { path.to_string() }
}

/// `SWS:132`: the message a click on a failed Interpolated String shows.
pub(super) fn evaluation_error(expression: &str, error: &str) -> String {
    format!("Error while evaluating \u{201C}{expression}\u{201D}:\n{error}")
}

/// A reported path as shown and copied: the Windows form of an OSC 7 path (`/C:/x`),
/// else as reported.
fn shown_path(raw: &str) -> String {
    crate::profile::local_path(raw).map_or_else(|| raw.to_string(), |p| p.to_string_lossy().into_owned())
}

/// The names this computer answers to, for `is_local_host`: localhost, the computer name
/// and its full DNS name.
pub(super) fn local_names() -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = vec!["localhost".to_string()];
        names.extend(std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()));
        let full = super::aps_ui::local_host();
        if !full.is_empty() {
            names.push(full);
        }
        names
    })
}

/// `session.isLocalhost`: a session with no reported host is on this computer (iTerm2
/// starts every session with the local host).
pub(super) fn is_localhost(host: Option<&str>) -> bool {
    host.is_none_or(|h| sbt::is_local_host(h, local_names()))
}

/// The local time, broken down.
#[cfg(windows)]
fn local_civil() -> sbt::Civil {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    sbt::Civil {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        weekday: t.wDayOfWeek as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
        millis: t.wMilliseconds as u32,
    }
}

/// `dateFormatFromTemplate:options:0 locale:currentLocale` (`CLK`): ICU's best pattern for
/// the template in the user's locale, or the template itself when ICU fails. Cached per
/// template.
#[cfg(windows)]
fn best_pattern(template: &str) -> String {
    use windows_sys::Win32::Globalization::{U_ZERO_ERROR, udatpg_close, udatpg_getBestPattern, udatpg_open};
    thread_local! {
        static PATTERNS: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::new());
    }
    if let Some(p) = PATTERNS.with(|m| m.borrow().get(template).cloned()) {
        return p;
    }
    let found = (|| {
        let mut err = U_ZERO_ERROR;
        // A null locale is ICU's default, the user's.
        let g = unsafe { udatpg_open(std::ptr::null(), &mut err) };
        if g.is_null() || err > U_ZERO_ERROR {
            return None;
        }
        let skeleton: Vec<u16> = template.encode_utf16().collect();
        let mut buf = vec![0u16; 256];
        let n = unsafe { udatpg_getBestPattern(g, skeleton.as_ptr(), skeleton.len() as i32, buf.as_mut_ptr(), buf.len() as i32, &mut err) };
        unsafe { udatpg_close(g) };
        (err <= U_ZERO_ERROR && n > 0 && (n as usize) <= buf.len()).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    })();
    let pattern = found.unwrap_or_else(|| template.to_string());
    PATTERNS.with(|m| m.borrow_mut().insert(template.to_string(), pattern.clone()));
    pattern
}

/// The Clock's text for its knobs (`CLK`): the format, through ICU when localized.
#[cfg(windows)]
pub(super) fn clock_text(format: &str, localize: bool) -> String {
    let pattern = if localize { best_pattern(format) } else { format.to_string() };
    sbt::format_tr35(&pattern, &local_civil())
}

/// The next whole second of the wall clock (the Clock's cadence is 1 s).
pub(super) fn next_whole_second(now: Instant) -> Instant {
    let into = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    now + std::time::Duration::from_millis(u64::from(1000 - into.min(999)))
}

impl App {
    /// What a content component shows for a session now.
    pub(super) fn content(&self, pane: PaneId, kind: Kind, c: &Component) -> Content {
        // The session's `hostname`, `username` and `path` as `variables` reports them, read
        // directly: this runs every frame, and `variables` copies the selection.
        let shell = self.panes.get(&pane).map(|p| &p.own().shell);
        let reported_host = shell.and_then(|s| s.host.clone());
        let mut content = match kind {
            // `CLK`: one variant; the SSH knobs wait for milestone 14.
            Kind::Clock => {
                let format = knob_str(c, kind, FORMAT_KEY).unwrap_or_else(|| DEFAULT_CLOCK_FORMAT.to_string());
                Content::text(vec![clock_text(&format, knob_bool(c, LOCALIZE_KEY, true))])
            }
            // `VAR:169-202`: the replacement alone on this computer, else the host's
            // variants. The local host name stands in before shell integration reports one.
            Kind::HostName => {
                let local = is_localhost(reported_host.as_deref());
                let host = reported_host.unwrap_or_else(super::aps_ui::local_host);
                let replacement = knob_str(c, kind, ABBREVIATE_LOCALHOST_KEY).unwrap_or_default();
                if !replacement.is_empty() && local { Content::text(vec![replacement]) } else { Content::text(sbt::host_variants(&host)) }
            }
            // `VAR:234-247`: the full name, always; this computer's user before shell
            // integration reports one.
            Kind::UserName => {
                let user = shell.and_then(|s| s.user.clone()).unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_default());
                let min_text = sbt::user_min_text(&user);
                Content { min_text: (!min_text.is_empty()).then_some(min_text), ..Content::text(vec![user]) }
            }
            // `JOB:85-149`: [""] before the first job; U+2026 when every name is empty.
            Kind::JobName => {
                let chain = shell.map_or_else(Vec::new, |s| job_chain(s.job.as_ref().map(|j| j.name.as_str()), &s.job_ancestors));
                Content::text(sbt::job_variants(&chain))
            }
            // `VAR:252-345`: home as `~`, compressed from the first component; empty when
            // blank; Copy is the whole path; a click opens the directory menu.
            Kind::CurrentDirectory => {
                // A tmux pane's own directory stands in until shell integration reports one.
                let tmux_cwd = || self.panes.get(&pane).and_then(|p| p.tmux_io()).map(|io| io.info.borrow().cwd.clone());
                let raw = shell.and_then(|s| s.cwd.clone()).or_else(tmux_cwd).unwrap_or_default();
                let path = shown_path(&raw);
                let home = std::env::var("USERPROFILE").unwrap_or_default();
                let display = sbt::display_dir(&path, &home);
                let blank = raw.trim().is_empty();
                Content {
                    copy: (!blank).then(|| path.clone()),
                    empty: blank,
                    click: Click::Menu(Kind::CurrentDirectory),
                    ..Content::text(if blank { Vec::new() } else { sbt::dir_variants(&display) })
                }
            }
            // `SWS`: one variant, the value; a failed expression shows the ladybug and a
            // click shows why.
            Kind::InterpolatedString => {
                let expression = knob_str(c, kind, EXPRESSION_KEY).unwrap_or_default();
                let vars = self.variables(pane);
                let lookup = |n: &str| crate::prompted::variable(n.strip_prefix("session.").unwrap_or(n), &vars);
                match crate::interp::evaluate(&expression, &lookup, &[]) {
                    Ok(value) => Content::text(vec![value]),
                    Err(e) => Content { click: Click::Error(evaluation_error(&expression, &e)), ..Content::text(vec![LADYBUG.to_string()]) },
                }
            }
            // Tasks 4 and 5 fill these in; `Piece::of` does not place them yet.
            Kind::CustomAction
            | Kind::ActionsMenu
            | Kind::SnippetsMenu
            | Kind::TriggersMenu
            | Kind::SearchTool
            | Kind::FilterTool
            | Kind::Composer
            | Kind::CallScriptFunction
            | Kind::TmuxLeft
            | Kind::TmuxRight
            | Kind::Progress => Content::text(Vec::new()),
        };
        content.icon = sbc::icon(kind);
        content
    }

    /// Current Directory's click (`VAR:355-413`): the host's ten best recent directories
    /// (picking one types `cd <path>`), then, with a known path, Copy Path, Copy Folder
    /// Name, Show in Explorer, New Session Here in New Window and in New Tab. `at` is the
    /// component's corner in window pixels.
    pub(super) fn show_dir_menu(&mut self, pane: PaneId, path: Option<String>, at: (f32, f32)) {
        use muda::{ContextMenu, Menu, MenuItem, PredefinedMenuItem};
        let Some(hwnd) = self.hwnd() else { return };
        let host = self.history_host_pub(pane);
        let dirs: Vec<crate::shell_history::Dir> = self.shell_history_pub().dirs(&host).into_iter().take(10).collect();
        if dirs.is_empty() && path.is_none() {
            return;
        }
        let menu = Menu::new();
        let mut results = Vec::new();
        for (n, d) in dirs.iter().enumerate() {
            results.push(menu.append(&MenuItem::with_id(format!("statusbar:dir:{n}"), dir_title(&d.path, d.starred).replace('&', "&&"), true, None)));
        }
        if path.is_some() {
            results.push(menu.append(&PredefinedMenuItem::separator()));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-copy-path", "Copy Path", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-copy-name", "Copy Folder Name", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-reveal", "Show in Explorer", true, None)));
            results.push(menu.append(&PredefinedMenuItem::separator()));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-new-window", "New Session Here in New Window", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-new-tab", "New Session Here in New Tab", true, None)));
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        let choices = dirs.into_iter().map(|d| d.path).collect();
        self.status_menu = Some(super::statusbar_ui::StatusMenu::for_component(pane, Kind::CurrentDirectory.class(), choices, path));
        let position = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(position)) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the directory menu (`VAR:415-495`). True when `what` was one.
    pub(super) fn dir_menu_picked(&mut self, what: &str, pane: PaneId, choices: &[String], path: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        if let Some(n) = what.strip_prefix("dir:") {
            // `cd <path>`, shell-escaped, with no newline.
            let Some(dir) = n.parse::<usize>().ok().and_then(|n| choices.get(n)) else { return true };
            let shell = self.panes.get(&pane).and_then(|p| p.own().shell.shell.clone());
            let text = format!("cd {}", crate::shell_history::insertable_dir(dir, shell.as_deref()));
            self.user_write(pane, text.as_bytes(), Write::SendText);
            return true;
        }
        let Some(path) = path else { return what.starts_with("dir-") };
        match what {
            "dir-copy-path" => self.copy_status_text(path.to_string()),
            "dir-copy-name" => self.copy_status_text(folder_name(path)),
            // "Reveal in Finder": Explorer opens the folder; nothing runs.
            "dir-reveal" => match crate::profile::local_path(path).filter(|p| p.is_dir()) {
                Some(dir) => {
                    if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
                        self.banners.set("action", format!("Could not show {path}: {e}"));
                    }
                }
                None => self.banners.set("action", format!("{path} is not a folder on this computer.")),
            },
            // The default profile, started in the path.
            "dir-new-window" | "dir-new-tab" => {
                self.start_dir_override = Some(crate::arrangement::Folder::Forced(path.to_string()));
                if what == "dir-new-window" {
                    self.new_window(event_loop, None);
                } else {
                    self.new_tab(None);
                }
                self.start_dir_override = None;
                self.redraw();
            }
            _ => return false,
        }
        true
    }

    /// Puts text on the clipboard for a status bar item.
    pub(super) fn copy_status_text(&mut self, text: String) {
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(err) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Could not copy: {err}"));
            }
        }
    }

    /// The Clock's cadence (`CLK`, 1 s): while any session's bar shows a Clock, every
    /// window redraws on each whole second. When to come back.
    pub(super) fn tick_status_bars(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if let Some(due) = self.status_clock_due {
            if now < due {
                return Some(due);
            }
        }
        let was_due = self.status_clock_due.take().is_some();
        if !self.any_clock_shown() {
            return None;
        }
        if was_due {
            self.redraw();
            self.redraw_all();
        }
        let next = next_whole_second(now);
        self.status_clock_due = Some(next);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `JOB:109-149` on Tandem's ancestors: the job's own name, then its parents below the
    /// shell; nothing before the first job. `JOB:92-99`: a job with an empty name still
    /// shows U+2026.
    #[test]
    fn job_chain_follows_iterm2() {
        let anc = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(job_chain(None, &[]), Vec::<String>::new());
        assert_eq!(sbt::job_variants(&job_chain(None, &[])), vec![String::new()]);
        assert_eq!(job_chain(Some("pwsh"), &[]), vec!["pwsh"]);
        assert_eq!(job_chain(Some("Vim"), &anc(&["vim", "bash"])), vec!["Vim", "bash"]);
        assert_eq!(sbt::job_variants(&job_chain(Some("vim"), &anc(&["vim", "bash"]))), vec!["vim", "vim \u{25C2} bash"]);
        assert_eq!(sbt::job_variants(&job_chain(Some(""), &[])), vec!["\u{2026}"]);
    }

    #[test]
    fn directory_menu_text_follows_iterm2() {
        assert_eq!(dir_title(r"C:\src", true), "\u{2605} C:\\src");
        assert_eq!(dir_title(r"C:\src", false), r"C:\src");
        assert_eq!(folder_name(r"C:\Users\rob\src\"), "src");
        assert_eq!(folder_name("/home/rob/tandem"), "tandem");
        assert_eq!(folder_name(r"C:\"), "C:");
        assert_eq!(shown_path("/C:/Users/rob"), r"C:\Users\rob");
        assert_eq!(shown_path("/home/rob"), "/home/rob");
        assert_eq!(evaluation_error("\\(x", "bad"), "Error while evaluating \u{201C}\\(x\u{201D}:\nbad");
    }

    #[test]
    fn text_content_is_empty_by_its_widest_variant() {
        assert!(Content::text(Vec::new()).empty);
        assert_eq!(Content::text(Vec::new()).variants, vec![String::new()]);
        assert!(!Content::text(vec![String::new(), "a".into()]).empty);
    }

    /// ICU answers for the default template (Windows 10 1903 and later), and the Clock
    /// renders it; an unlocalized format is used as given.
    #[cfg(windows)]
    #[test]
    fn clock_pattern_comes_from_icu() {
        let pattern = best_pattern(DEFAULT_CLOCK_FORMAT);
        assert!(pattern.contains('M') && pattern.contains("mm"), "{pattern}");
        assert!(clock_text(DEFAULT_CLOCK_FORMAT, true).chars().any(|c| c.is_ascii_digit()));
        assert_eq!(clock_text("'x'", false), "x");
    }

    #[test]
    fn clock_wakes_on_whole_seconds() {
        let now = Instant::now();
        let next = next_whole_second(now);
        assert!(next > now && next <= now + std::time::Duration::from_millis(1000));
    }
}
