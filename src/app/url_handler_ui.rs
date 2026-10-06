//! The URL handler command (`UAH:326-375`): a clicked URL goes to the person's own command
//! instead of the system. SECURITY (13p-r ruling): the command is split into a program and
//! arguments and started with one `CreateProcessW`, never through a shell, and the URL is
//! one argument of its own (`semantic::handler_plan`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::semantic::{self, Run, HANDLER_FAILED, HANDLER_TITLE, URL_TOKEN};

/// How much of a handler's output the Script Console keeps, per stream.
const OUTPUT_LIMIT: u64 = 64 * 1024;

/// A finished handler: its Script Console entry, its output and how it ended.
pub(super) struct HandlerDone {
    id: String,
    output: String,
    failed: bool,
}

/// The finished handlers, filled by their waiting threads and drained in `tick_triggers`.
pub(super) type HandlerInbox = Arc<Mutex<Vec<HandlerDone>>>;

/// The template evaluated with `url` bound to `URL_TOKEN`, `background` to "1" or "0"
/// (`UAH:340-375`), and each session variable it names to `var_token(n)`; the values, in
/// order, come back beside it for `handler_plan`, which puts each back as one argument after
/// the split and refuses what a command reader would act on.
pub(super) fn evaluate_template(template: &str, background: bool, vars: &dyn Fn(&str) -> Option<String>, lax: bool) -> Result<(String, Vec<String>), String> {
    let values = std::cell::RefCell::new(Vec::new());
    let lookup = |n: &str| match n {
        "url" => Some(URL_TOKEN.to_string()),
        "background" => Some(if background { "1" } else { "0" }.to_string()),
        n => vars(n).map(|v| {
            let mut values = values.borrow_mut();
            values.push(v);
            semantic::var_token(values.len() - 1)
        }),
    };
    let out = crate::interp::evaluate_lax(template, &lookup, &[], lax)?;
    Ok((out, values.into_inner()))
}

/// Evaluates, plans and starts the handler through `spawn`. Err is the banner text. An empty
/// result runs nothing (iTerm2 logs it); a refusal runs nothing; a failed start is
/// `HANDLER_FAILED`.
pub(super) fn handle(
    template: &str,
    url: &str,
    background: bool,
    vars: &dyn Fn(&str) -> Option<String>,
    lax: bool,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
    spawn: &mut dyn FnMut(&Path, &[String]) -> Result<(), String>,
) -> Result<(), String> {
    let (evaluated, values) = evaluate_template(template, background, vars, lax)?;
    match semantic::handler_plan(&evaluated, url, &values, resolve) {
        Ok(Run::Spawn { program, args }) => spawn(&program, &args).map_err(|_| HANDLER_FAILED.to_string()),
        Ok(_) => Err(HANDLER_FAILED.to_string()),
        Err(semantic::Refusal::Empty) => Ok(()),
        Err(refusal) => Err(refusal.text().to_string()),
    }
}

/// A finished handler's Script Console text and banner. Ruling: "URL Handler Command Failed"
/// is only for a program that cannot start (`handle`); a non-zero exit is noted in the Script
/// Console, never a banner (explorer.exe exits 1 after handing a URL off).
fn finished(d: &HandlerDone) -> (String, Option<&'static str>) {
    let mut log = d.output.clone();
    if d.failed {
        log.push_str("The handler exited with a non-zero status.\n");
    }
    (log, None)
}

/// Output text for the Script Console: controls other than newline and tab are shown as `?`.
fn clean_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().map(|c| if c.is_control() && !matches!(c, '\n' | '\t') { '?' } else { c }).collect()
}

/// The handler's working folder: the pane's when it is a local drive folder, else the home
/// folder; cmd.exe refuses a UNC current folder, and one is never used.
fn work_dir(pane_dir: Option<&Path>, may_probe: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let drive = |p: &Path| {
        let s = p.to_string_lossy();
        s.as_bytes().get(1) == Some(&b':') && s.as_bytes()[0].is_ascii_alphabetic() && !crate::scripts::is_network_path(&s)
    };
    let home = || std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).filter(|h| drive(h));
    pane_dir.filter(|d| drive(d) && may_probe(&d.to_string_lossy()) && crate::localfs::is_local_dir(d)).map(Path::to_path_buf).or_else(home)
}

impl App {
    /// Hands a URL to `UrlHandlerCommand`. False when none is set (the URL opens normally);
    /// true when the handler took it, started or refused (the banner says which).
    pub(super) fn url_handler(&mut self, pane: PaneId, url: &str, cwd: Option<&Path>, background: bool) -> bool {
        let template = self.globals.string("UrlHandlerCommand");
        if template.trim().is_empty() {
            return false;
        }
        let probe = self.path_probe();
        let dir = work_dir(cwd, &|p| crate::netpath::may_probe(p, &probe));
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        let vars = self.variables_with(pane, false);
        let lookup = |n: &str| crate::prompted::variable(n.strip_prefix("session.").unwrap_or(n), &vars);
        let path_var = std::env::var("PATH").unwrap_or_default();
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let resolve = |name: &str| semantic::resolve_program(name, &path_var, &pathext, &|p| crate::localfs::is_local_file(p));
        let mut child = None;
        let result = handle(&template, url, background, &lookup, lax, &resolve, &mut |program, args| {
            let mut cmd = std::process::Command::new(program);
            cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            if let Some(d) = dir.as_deref().and_then(|d| crate::localfs::LocalPath::semantic(&d.to_string_lossy(), &probe)) {
                crate::localfs::set_cwd(&mut cmd, &d);
            }
            child = Some(cmd.spawn().map_err(|e| e.to_string())?);
            Ok(())
        });
        match result {
            Ok(()) => {
                if let Some(c) = child {
                    self.watch_handler(c);
                }
            }
            Err(banner) => self.banners.set("action", banner),
        }
        true
    }

    /// A started handler: a Script Console entry titled "URL Handler", and a thread that collects
    /// its output, waits for it, and reports back through the inbox.
    fn watch_handler(&mut self, mut child: std::process::Child) {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let id = format!("url-handler-{}", SERIAL.fetch_add(1, Ordering::Relaxed));
        self.script_history.add(crate::script_history::Entry::new(&id, HANDLER_TITLE, api_ui::unix_secs()));
        if let Some(e) = self.script_history.find_mut(&id) {
            e.pids.push(child.id());
        }
        let (inbox, proxy) = (self.handler_inbox.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            use std::io::Read;
            let read = |r: Option<Box<dyn Read + Send>>| {
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    if let Some(mut r) = r {
                        let _ = (&mut r).take(OUTPUT_LIMIT).read_to_end(&mut buf);
                        // Keep draining so the handler never blocks on a full pipe.
                        let _ = std::io::copy(&mut r, &mut std::io::sink());
                    }
                    buf
                })
            };
            let out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
            let err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
            let status = child.wait();
            let mut output = clean_output(&out.join().unwrap_or_default());
            output.push_str(&clean_output(&err.join().unwrap_or_default()));
            let failed = !status.is_ok_and(|s| s.success());
            if let Ok(mut q) = inbox.lock() {
                q.push(HandlerDone { id, output, failed });
            }
            let _ = proxy.lock().map(|p| p.send_event(UserEvent::Wake));
        });
    }

    /// Finished handlers: their output into the Script Console entry, and a failure's banner.
    pub(super) fn handlers_finished(&mut self) {
        let done: Vec<HandlerDone> = self.handler_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if done.is_empty() {
            return;
        }
        let stamp = self.history_stamp();
        for d in done {
            let (log, banner) = finished(&d);
            self.script_history.add_log(&d.id, &log, &stamp);
            self.script_history.stop(&d.id);
            if let Some(b) = banner {
                self.banners.set("action", b);
            }
        }
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(n: &str) -> Option<PathBuf> {
        match n {
            "open.exe" => Some(PathBuf::from(r"C:\t\open.exe")),
            "h.cmd" => Some(PathBuf::from(r"C:\t\h.cmd")),
            "cmd.exe" => Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
            _ => None,
        }
    }

    type Calls = Vec<(PathBuf, Vec<String>)>;

    fn run(template: &str, url: &str, background: bool, vars: &dyn Fn(&str) -> Option<String>) -> (Result<(), String>, Calls) {
        let mut calls: Calls = Vec::new();
        let r = handle(template, url, background, vars, true, &resolve, &mut |p, a| {
            calls.push((p.to_path_buf(), a.to_vec()));
            Ok(())
        });
        (r, calls)
    }

    #[test]
    fn handler_spawns_without_a_shell() {
        let none = |_: &str| None;
        let evil = "https://x/%CMDCMDLINE%&calc";
        let (r, calls) = run(r"open.exe \(url)", evil, false, &none);
        assert_eq!(r, Ok(()));
        assert_eq!(calls, vec![(PathBuf::from(r"C:\t\open.exe"), vec![evil.to_string()])], "exactly one call, one argument");
        assert!(calls.iter().all(|(p, _)| !p.to_string_lossy().to_ascii_lowercase().contains("cmd.exe")));
        // A batch handler refuses the URL: no call, the banner is the refusal's text.
        let (r, calls) = run(r"h.cmd \(url)", evil, false, &none);
        assert!(calls.is_empty());
        assert_eq!(r, Err(semantic::Refusal::BatchUnsafe.text().to_string()));
        // cmd.exe itself is never a handler, whatever the URL.
        for url in [evil, "https://a/b"] {
            let (r, calls) = run(r#"cmd.exe /c start "" \(url)"#, url, false, &none);
            assert!(calls.is_empty());
            assert_eq!(r, Err(semantic::Refusal::Forwarder.text().to_string()));
        }
    }

    #[test]
    fn handler_template_binds_url_and_background() {
        let none = |_: &str| None;
        assert_eq!(run(r"open.exe --bg=\(background) \(url)", "https://a/", true, &none).1[0].1, ["--bg=1", "https://a/"]);
        assert_eq!(run(r"open.exe --bg=\(background)", "https://a/", false, &none).1[0].1, ["--bg=0", "https://a/"]);
        // An empty result runs nothing and shows nothing.
        let (r, calls) = run("", "https://a/", false, &none);
        assert_eq!((r, calls.len()), (Ok(()), 0));
        // The URL cannot choose the program.
        let (r, calls) = run(r"\(url)", "https://a/", false, &none);
        assert!(calls.is_empty());
        assert_eq!(r, Err(semantic::Refusal::UrlInProgram.text().to_string()));
        // A control character in the URL.
        let (r, calls) = run(r"open.exe \(url)", "https://a/\nx", false, &none);
        assert!(calls.is_empty());
        assert_eq!(r, Err(semantic::Refusal::ControlCharacter.text().to_string()));
    }

    #[test]
    fn handler_variables_cannot_split_the_line() {
        let vars = |n: &str| match n {
            "path" => Some(r#"C:\a" --evil "#.to_string()),
            "name" => Some("My Title  with spaces".to_string()),
            "hostname" => Some("C:\\t".to_string()),
            _ => None,
        };
        let (r, calls) = run(r"open.exe \(path) \(url)", "https://a/", false, &vars);
        assert!(calls.is_empty());
        assert_eq!(r, Err(semantic::Refusal::VariableUnsafe.text().to_string()));
        // A title with spaces stays one argument.
        let (r, calls) = run(r"open.exe \(name) \(url)", "https://a/", false, &vars);
        assert_eq!(r, Ok(()));
        assert_eq!(calls[0].1, vec!["My Title  with spaces".to_string(), "https://a/".to_string()]);
        // A variable never names the program.
        for t in [r"\(hostname)\open.exe \(url)", r"\(path) \(url)"] {
            let (r, calls) = run(t, "https://a/", false, &vars);
            assert!(calls.is_empty(), "{t}");
            assert_eq!(r, Err(semantic::Refusal::VariableInProgram.text().to_string()), "{t}");
        }
    }

    /// Ruling: the failure banner is only for a program that cannot start; a handler that
    /// started and exits non-zero (explorer.exe exits 1 after handing a URL off) shows none, and
    /// its Script Console entry says how it ended.
    #[test]
    fn a_nonzero_exit_is_not_a_failure_banner() {
        let done = |failed| HandlerDone { id: "url-handler-0".into(), output: "out\n".into(), failed };
        assert_eq!(finished(&done(true)), ("out\nThe handler exited with a non-zero status.\n".to_string(), None));
        assert_eq!(finished(&done(false)), ("out\n".to_string(), None));
    }

    #[test]
    fn a_failed_start_is_the_handler_failure() {
        let r = handle(r"open.exe \(url)", "https://a/", false, &|_| None, true, &resolve, &mut |_, _| Err("os error".into()));
        assert_eq!(r, Err(HANDLER_FAILED.to_string()));
        let r = handle(r"nothere.exe \(url)", "https://a/", false, &|_| None, true, &resolve, &mut |_, _| Ok(()));
        assert_eq!(r, Err(HANDLER_FAILED.to_string()), "an unresolved program is refused (P14)");
    }

    #[test]
    fn the_working_folder_is_a_local_drive_folder() {
        let allow = |_: &str| true;
        let used = |d: Option<PathBuf>| d.is_some_and(|d| d.to_string_lossy().starts_with(r"\\"));
        assert!(!used(work_dir(Some(Path::new(r"\\srv\s")), &allow)), "a UNC folder is never used");
        assert!(!used(work_dir(Some(Path::new(r"\\?\UNC\srv\s")), &allow)));
        let temp = std::env::temp_dir();
        assert_eq!(work_dir(Some(&temp), &allow), Some(temp.clone()));
        let refused = |_: &str| false;
        assert_ne!(work_dir(Some(&temp), &refused), Some(temp), "a folder the probe rule refuses is not used");
    }
}
