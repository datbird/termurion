//! The Crash Reporter (iTerm2's `UKCrashReporter`): at the next launch after a crash left a new
//! minidump (`crashdump.rs`), a window offers to send a crash log to the developers.
//!
//! Windows decisions (ruling W1): Send opens a new issue on the project's GitHub repository with
//! the log text in its body, private paths removed (the dump itself holds memory and is never
//! sent); Discard deletes the dump; Remind Me Later (and the close box) keeps it for the next
//! launch. The report is built here from the dump's name and `host.log`; the window is a `View`.

use std::path::{Path, PathBuf};

use crate::panel_ui::{Act, View};

/// The stored time of the last reported crash, seconds since 1970 (`UKCrashReporterLastCrashReportDate`).
pub const LAST_REPORT_KEY: &str = "UKCrashReporterLastCrashReportDate";

/// The window's title (`UKCrashReporter.xib:24`).
pub const TITLE: &str = "Crash Reporter";
/// The tab names (`UKCrashReporter.xib:50`, `:204`).
pub const TAB_DESCRIPTION: &str = "Description";
pub const TAB_CRASH_LOG: &str = "Crash Log";
/// The buttons (`UKCrashReporter.xib:365-390`).
pub const SEND: &str = "Send";
pub const REMIND: &str = "Remind Me Later";
pub const DISCARD: &str = "Discard";
/// The explanation (`UKCrashReporter.xib:38`), `{APPNAME}` filled with the app's name. Windows
/// has Microsoft's reporting where the Mac has Apple's (ruling W1).
pub const EXPLANATION: &str = "It appears that {APPNAME} crashed the last time it was used. Do you want to send a crash log to the developers?\n\nCrash logs from Microsoft are not sent to other developers, so this is the only way the developers of {APPNAME} can find out about and fix this crash.";
/// The note under the log (`UKCrashReporter.xib:352`; Termurion sends no settings, ruling W1).
pub const LOG_NOTE: &str = "You can delete any information you do not want transmitted, but this will make it more difficult to detect the bug. By default, the crash log as well as what PC you are using is transmitted.";
/// The message template (`UKCrashReporter.m:awakeFromNib`, `CrashReporter.FeedbackMessageTemplate`).
pub const MESSAGE_TEMPLATE: &str = "Hi Termurion Developers,\n\nhere\u{2019}s what I\u{2019}d like to tell you:\n\n\t...\n\n-- {USERNAME}\n\nReply to me at {EMAIL}";
pub const MISSING_EMAIL: &str = "<Your E-Mail Address here>";
/// The log's own header, as iTerm2 appends it to a crash log (`UKCrashReporter.m`).
pub const ERROR_LOG_HEADER: &str = "~~ Error Logs ~~\n";
/// The separator between the message and the log (`sendCrashReport:`).
pub const SEPARATOR: &str = "\n==========\n";
/// The issue's title.
pub const ISSUE_TITLE: &str = "Crash report";

/// The app's name inside the explanation.
pub fn explanation() -> String {
    EXPLANATION.replace("{APPNAME}", "Termurion")
}

/// The message box prefilled for `user` (`awakeFromNib`).
pub fn message(user: &str) -> String {
    MESSAGE_TEMPLATE.replace("{USERNAME}", user).replace("{EMAIL}", MISSING_EMAIL)
}

/// A dump's file name taken apart: the program, its process id and the time (`crashdump::dump_name`).
pub fn parse_dump_name(file: &str) -> Option<(String, u32, u64)> {
    let stem = file.strip_suffix(".dmp").or_else(|| file.strip_suffix(".DMP"))?;
    let (rest, secs) = stem.rsplit_once('-')?;
    let (exe, pid) = rest.rsplit_once('-')?;
    Some((exe.to_string(), pid.parse().ok()?, secs.parse().ok()?))
}

/// The newest dump in `dir` that was written after `last_reported` (seconds since 1970), with its
/// time. Only the app's own dumps count: the session host's crash is told by the app that
/// restarted it (`host_watch.rs`) and reported the same way when it is the newest.
pub fn pending(dir: &Path, last_reported: f64) -> Option<(PathBuf, u64)> {
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dmp")))
        .filter_map(|e| {
            let secs = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            Some((e.path(), secs))
        })
        .filter(|(_, secs)| (*secs as f64) > last_reported)
        .max_by_key(|(p, secs)| (*secs, p.clone()))
}

/// Private paths out of `text`: each folder in `folders` (its name, what stands for it) in either
/// slash direction and any case, then any other `X:\Users\<name>` home.
pub fn scrub(text: &str, folders: &[(String, &str)]) -> String {
    let mut out = text.to_string();
    let mut sorted: Vec<&(String, &str)> = folders.iter().filter(|(f, _)| f.len() > 3).collect();
    sorted.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
    for (folder, stand_in) in sorted {
        for variant in [folder.clone(), folder.replace('\\', "/")] {
            if let Ok(re) = regex::Regex::new(&format!("(?i){}", regex::escape(&variant))) {
                out = re.replace_all(&out, regex::NoExpand(stand_in)).into_owned();
            }
        }
    }
    let homes = regex::Regex::new(r#"(?i)[A-Z]:[\\/]Users[\\/][^\\/\s"']+"#).expect("home pattern");
    homes.replace_all(&out, regex::NoExpand("%USERPROFILE%")).into_owned()
}

/// The folders to hide on this machine.
pub fn private_folders() -> Vec<(String, &'static str)> {
    let mut v = Vec::new();
    for (var, stand_in) in [("LOCALAPPDATA", "%LOCALAPPDATA%"), ("APPDATA", "%APPDATA%"), ("TEMP", "%TEMP%"), ("USERPROFILE", "%USERPROFILE%")] {
        if let Some(p) = std::env::var_os(var) {
            v.push((p.to_string_lossy().into_owned(), stand_in));
        }
    }
    v
}

/// The lines of `host.log` that belong to the crash: every line of the crashed process (`[pid]`),
/// the last `max` of them, then the crash line itself if it was written by another process.
pub fn log_lines(log: &str, pid: u32, max: usize) -> Vec<String> {
    let tag = format!("[{pid}] ");
    let own: Vec<&str> = log.lines().filter(|l| l.split_once(' ').is_some_and(|(_, rest)| rest.starts_with(&tag))).collect();
    let skip = own.len().saturating_sub(max);
    own.into_iter().skip(skip).map(str::to_string).collect()
}

/// The Crash Log tab's text: the version and machine lines, the crash line and the process's log
/// (`UKCrashReporterCheckForCrash`'s final report).
pub fn report_text(version: &str, windows: &str, cpus: usize, dump_file: &str, log: &str) -> String {
    let (exe, pid) = parse_dump_name(dump_file).map_or((String::new(), 0), |(e, p, _)| (e, p));
    let lines = log_lines(log, pid, 200);
    let mut s = format!("Version: {version}\nWindows: {windows}\nCPUs: {cpus}\nProgram: {exe}\nDump: {dump_file}\n\n");
    s.push_str(ERROR_LOG_HEADER);
    for l in lines {
        s.push_str(&l);
        s.push('\n');
    }
    s
}

/// "Windows 11 24H2 (build 26100)" from the registry's version keys; "unknown" when unreadable.
#[cfg(windows)]
pub fn windows_version() -> String {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let read = |name: &str| -> Option<String> {
        let (key, name) = (wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), wide(name));
        let mut buf = vec![0u16; 128];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: both names are NUL-terminated and the buffer is as long as `bytes` says.
        let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    };
    match (read("DisplayVersion"), read("CurrentBuildNumber")) {
        (Some(v), Some(b)) => format!("{v} (build {b})"),
        (None, Some(b)) => format!("build {b}"),
        _ => "unknown".to_string(),
    }
}

#[cfg(not(windows))]
pub fn windows_version() -> String {
    "unknown".to_string()
}

/// The longest issue link opened: browsers and GitHub refuse far longer ones.
pub const MAX_URL: usize = 7000;

fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The new-issue link for `body`, cut to fit `MAX_URL`: the message and the start of the log are
/// kept, the cut is said in the text.
pub fn issue_url(title: &str, body: &str) -> String {
    let base = format!("{}/new?title={}&body=", crate::links::ISSUES, encode(title));
    let note = "\n\n(The log was cut to fit in a link.)";
    let mut text = body.to_string();
    let mut cut = false;
    loop {
        let shown = if cut { format!("{text}{note}") } else { text.clone() };
        let url = format!("{base}{}", encode(&shown));
        if url.len() <= MAX_URL || text.is_empty() {
            return url;
        }
        // Drop a tenth of what is left, on a character boundary.
        let mut keep = text.len() - (text.len() / 10).max(1);
        while !text.is_char_boundary(keep) {
            keep -= 1;
        }
        text.truncate(keep);
        cut = true;
    }
}

/// What Send posts (`sendCrashReport:`): the message, the separator, the log.
pub fn body(message: &str, log: &str) -> String {
    format!("{message}{SEPARATOR}{log}")
}

/// Which tab the window shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Description,
    Log,
}

pub struct CrashView {
    message: String,
    log: String,
    tab: Tab,
}

impl CrashView {
    pub fn new(message: String, log: String) -> CrashView {
        CrashView { message, log, tab: Tab::Description }
    }
}

impl View for CrashView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn size(&self) -> (f64, f64) {
        (592.0, 560.0)
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        let mut acts = Vec::new();
        ui.label(explanation());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Description, TAB_DESCRIPTION);
            ui.selectable_value(&mut self.tab, Tab::Log, TAB_CRASH_LOG);
        });
        ui.separator();
        let room = (ui.available_height() - 80.0).max(120.0);
        egui::ScrollArea::vertical().max_height(room).show(ui, |ui| match self.tab {
            Tab::Description => {
                ui.add(egui::TextEdit::multiline(&mut self.message).desired_width(f32::INFINITY).desired_rows(14));
            }
            Tab::Log => {
                ui.add(egui::TextEdit::multiline(&mut self.log).desired_width(f32::INFINITY).desired_rows(14).font(egui::TextStyle::Monospace));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(LOG_NOTE).small());
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if crate::panel_ui::button(ui, DISCARD).clicked() {
                acts.push(Act::CrashDiscard);
            }
            if crate::panel_ui::button(ui, REMIND).clicked() {
                acts.push(Act::CrashLater);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::panel_ui::button(ui, SEND).clicked() {
                    acts.push(Act::CrashSend { title: ISSUE_TITLE.to_string(), body: body(&self.message, &self.log) });
                }
            });
        });
        acts
    }

    /// The close box is Remind Me Later: nothing is lost by closing (`remindMeLater:`).
    fn on_close(&mut self) -> Vec<Act> {
        vec![Act::CrashLater]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_name_comes_apart() {
        assert_eq!(parse_dump_name("termurion-1234-1760000000.dmp"), Some(("termurion".into(), 1234, 1_760_000_000)));
        assert_eq!(parse_dump_name("termurion-host-9-5.dmp"), Some(("termurion-host".into(), 9, 5)));
        assert_eq!(parse_dump_name("note.txt"), None);
        assert_eq!(parse_dump_name("x-y-z.dmp"), None);
    }

    #[test]
    fn the_newest_unreported_dump_is_pending() {
        let d = std::env::temp_dir().join(format!("termurion-crashrep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let now = std::time::SystemTime::now();
        for (name, age) in [("termurion-1-1.dmp", 300u64), ("termurion-2-2.dmp", 100), ("termurion-3-3.dmp", 200)] {
            let f = std::fs::File::create(d.join(name)).unwrap();
            f.set_modified(now - std::time::Duration::from_secs(age)).unwrap();
        }
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        let (p, _) = pending(&d, 0.0).unwrap();
        assert_eq!(p.file_name().unwrap(), "termurion-2-2.dmp");
        let secs = |age: u64| (now - std::time::Duration::from_secs(age)).duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64;
        assert!(pending(&d, secs(50)).is_none(), "a crash older than the last report is not offered again");
        assert_eq!(pending(&d, secs(150)).unwrap().0.file_name().unwrap(), "termurion-2-2.dmp");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn private_paths_are_removed() {
        let text = r"crash: termurion code 0xc0000005 at icu.dll+0x1a, dump C:\Users\Someone\AppData\Local\Termurion\crashes\termurion-1-2.dmp and c:/users/someone/x";
        let out = scrub(text, &[(r"C:\Users\Someone\AppData\Local".to_string(), "%LOCALAPPDATA%")]);
        assert!(out.contains(r"%LOCALAPPDATA%\Termurion\crashes\termurion-1-2.dmp"), "{out}");
        assert!(!out.to_lowercase().contains("someone"), "{out}");
        let other = scrub(r"D:\Users\Other Name\x", &[]);
        assert!(!other.contains("Other"), "{other}");
    }

    #[test]
    fn the_log_is_the_crashed_programs_lines() {
        let log = "10 [7] start\n11 [8] other\n12 [7] crash: termurion code 0x1\n";
        assert_eq!(log_lines(log, 7, 10), vec!["10 [7] start", "12 [7] crash: termurion code 0x1"]);
        assert_eq!(log_lines(log, 7, 1), vec!["12 [7] crash: termurion code 0x1"]);
        let t = report_text("1.2.3", "10.0.26100", 8, "termurion-7-5.dmp", log);
        assert!(t.starts_with("Version: 1.2.3\nWindows: 10.0.26100\nCPUs: 8\n") && t.contains("~~ Error Logs ~~\n10 [7] start\n"));
    }

    #[test]
    fn the_issue_link_fits_and_says_when_it_cut() {
        let short = issue_url("Crash report", "a b&c");
        assert_eq!(short, format!("{}/new?title=Crash%20report&body=a%20b%26c", crate::links::ISSUES));
        let long = issue_url("Crash report", &"x".repeat(40_000));
        assert!(long.len() <= MAX_URL, "{}", long.len());
        assert!(long.contains("cut"), "the cut is said");
        assert!(issue_url("t", &"\u{e9}".repeat(40_000)).len() <= MAX_URL);
    }

    #[test]
    fn the_texts_name_termurion() {
        assert!(explanation().starts_with("It appears that Termurion crashed the last time it was used."));
        assert!(!explanation().contains("{APPNAME}"));
        assert!(message("Pat").contains("-- Pat") && message("Pat").contains(MISSING_EMAIL));
    }
}
