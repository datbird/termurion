//! The URL handler command (`UAH:326-375`): a clicked URL goes to the person's own command
//! instead of the system. SECURITY (13p-r ruling): the command is split into a program and
//! arguments and started with one `CreateProcessW`, never through a shell, and the URL is
//! one argument of its own (`semantic::handler_plan`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::semantic::{self, Run, HANDLER_FAILED, HANDLER_TITLE, URL_TOKEN};

/// The banner when a session variable in the handler template would change how the line splits.
pub(super) const VARIABLE_REFUSED: &str = "Tandem did not run the URL handler command: a variable in it holds a quote or a control character.";

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

/// The template evaluated with `url` bound to `URL_TOKEN` and `background` to "1" or "0"
/// (`UAH:340-375`). A session variable the template names is refused when it holds a quote or a
/// control character: it would add arguments or end the line.
pub(super) fn evaluate_template(template: &str, background: bool, vars: &dyn Fn(&str) -> Option<String>, lax: bool) -> Result<String, String> {
    let bad = std::cell::Cell::new(false);
    let lookup = |n: &str| match n {
        "url" => Some(URL_TOKEN.to_string()),
        "background" => Some(if background { "1" } else { "0" }.to_string()),
        n => vars(n).inspect(|v| {
            if v.contains('"') || v.chars().any(char::is_control) {
                bad.set(true);
            }
        }),
    };
    let out = crate::interp::evaluate_lax(template, &lookup, &[], lax);
    if bad.get() {
        return Err(VARIABLE_REFUSED.to_string());
    }
    out
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
    let evaluated = evaluate_template(template, background, vars, lax)?;
    match semantic::handler_plan(&evaluated, url, resolve) {
        Ok(Run::Spawn { program, args }) => spawn(&program, &args).map_err(|_| HANDLER_FAILED.to_string()),
        Ok(_) => Err(HANDLER_FAILED.to_string()),
        Err(semantic::Refusal::Empty) => Ok(()),
        Err(refusal) => Err(refusal.text().to_string()),
    }
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
    pane_dir.filter(|d| drive(d) && may_probe(&d.to_string_lossy()) && d.is_dir()).map(Path::to_path_buf).or_else(home)
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
        let resolve = |name: &str| semantic::resolve_program(name, &path_var, &pathext, &|p| p.is_file());
        let mut child = None;
        let result = handle(&template, url, background, &lookup, lax, &resolve, &mut |program, args| {
            let mut cmd = std::process::Command::new(program);
            cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            if let Some(d) = &dir {
                cmd.current_dir(d);
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
            self.script_history.add_log(&d.id, &d.output, &stamp);
            self.script_history.stop(&d.id);
            if d.failed {
                self.banners.set("action", HANDLER_FAILED);
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
            "name" => Some("fine".to_string()),
            _ => None,
        };
        let (r, calls) = run(r"open.exe \(path) \(url)", "https://a/", false, &vars);
        assert!(calls.is_empty());
        assert_eq!(r, Err(VARIABLE_REFUSED.to_string()));
        let (r, calls) = run(r"open.exe \(name) \(url)", "https://a/", false, &vars);
        assert_eq!((r, calls[0].1.clone()), (Ok(()), vec!["fine".to_string(), "https://a/".to_string()]));
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
