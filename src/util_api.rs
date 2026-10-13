//! it2api (milestone 11e): iTerm2's Python script, shipped byte for byte in
//! `resources/utilities/it2api` and written as `utilities\it2api.py` with its two "iTerm2"
//! texts changed, run by the `it2api` entry with an installed Python 3 (Decision: not a Rust
//! port). The Python runtime (`script_runtime`) also uses `python_candidates`.

use std::path::{Path, PathBuf};

use crate::util_cli::{Cmd, Io};

/// The SHA-256 of iTerm2's `OtherResources/Utilities/it2api` at `64791fc`.
pub const SCRIPT_SHA256: &str = "09aa8dd4f0c7a9abb2861f0d37358f16bcd33a69051d1c36735ab1ba625e04c3";

/// iTerm2's script as shipped.
pub fn vendored() -> &'static str {
    include_str!("../resources/utilities/it2api")
}

/// The script Termurion writes: `vendored()` with its two user texts naming iTerm2 (`:535`,
/// `:659`) naming Termurion; the `iterm2` module name and its install text stay.
pub fn script() -> String {
    vendored()
        .replacen("description='iTerm2 CLI'", "description='Termurion CLI'", 1)
        .replacen("(represented as a tab in iTerm2)", "(represented as a tab in Termurion)", 1)
}

/// The Pythons it2api may run, best first: the first `py.exe` on PATH as `py -3`, then each
/// `python.exe` and `python3.exe` on PATH in PATH order, skipping the WindowsApps aliases
/// (they open the Store instead of running Python). Only `util_cli::path_dirs` folders are
/// looked in (local drive paths, quoted ones too, with no link on the way to a network path):
/// never the working folder, a relative, drive-relative or rooted one, or a share; `is_file`
/// is never asked about any other, nor about a candidate that is a link to a share.
pub fn python_candidates(path_var: &str, is_file: &dyn Fn(&Path) -> bool) -> Vec<Vec<String>> {
    let dirs: Vec<PathBuf> = crate::util_cli::path_dirs(path_var).collect();
    let alias = |p: &Path| p.to_string_lossy().to_ascii_lowercase().replace('/', "\\").contains("\\windowsapps\\");
    let usable = |p: &Path| !alias(p) && crate::util_cli::may_open(p) && is_file(p);
    let mut out = Vec::new();
    if let Some(py) = dirs.iter().map(|d| d.join("py.exe")).find(|p| usable(p)) {
        out.push(vec![py.to_string_lossy().into_owned(), "-3".to_string()]);
    }
    for dir in &dirs {
        for name in ["python.exe", "python3.exe"] {
            let exe = dir.join(name);
            if usable(&exe) {
                out.push(vec![exe.to_string_lossy().into_owned()]);
            }
        }
    }
    out
}

/// Where the script is: beside the program (`utilities\it2api.exe`), else in the
/// `utilities` folder beside it (`termurion-utils.exe it2api` beside termurion.exe).
pub fn script_path(own_dir: &Path, is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let file = crate::utilities::API_SCRIPT;
    [own_dir.join(file), own_dir.join(crate::utilities::FOLDER).join(file)].into_iter().find(|p| is_file(p))
}

/// What it2api prints when no Python 3 is found.
pub fn no_python() -> String {
    "it2api needs Python 3. Install it, then run:\n\n    $ py -m pip install iterm2\n".to_string()
}

/// What it2api prints when `it2api.py` is found neither beside the program in `dir` nor in
/// `dir\utilities`.
pub fn no_script(dir: &Path) -> String {
    let file = crate::utilities::API_SCRIPT;
    let inside = dir.join(crate::utilities::FOLDER);
    format!("it2api: {file} is not in {} or {}. Start Termurion once to write it.\n", dir.display(), inside.display())
}

/// `it2api [arguments...]`: runs the script with the first Python found, the arguments after
/// it, its streams shared, and returns its exit code. Ctrl+C is left to the script.
pub fn it2api(args: &[String], io: &mut Io) -> i32 {
    let own_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let is_file = |p: &Path| p.is_absolute() && crate::localfs::is_local_file(p);
    launch(args, io, &own_dir, &is_file)
}

/// `it2api` with the program's folder and the file test given.
fn launch(args: &[String], io: &mut Io, own_dir: &Path, is_file: &dyn Fn(&Path) -> bool) -> i32 {
    let path_var = (io.env)("PATH").unwrap_or_default();
    let Some(mut argv) = python_candidates(&path_var, is_file).into_iter().next() else {
        let _ = io.err.write_all(no_python().as_bytes());
        return 1;
    };
    let Some(script) = script_path(own_dir, is_file) else {
        let _ = io.err.write_all(no_script(own_dir).as_bytes());
        return 1;
    };
    let program = argv.remove(0);
    argv.push(script.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    let cmd = Cmd { program, args: argv, cwd: None, stdin: None, inherit: true, quiet: false, env: Vec::new(), merge_stderr: false };
    match (io.run)(&cmd) {
        Ok(done) => done.code,
        Err(e) => {
            let _ = io.err.write_all(format!("it2api: {}: {e}\n", cmd.program).as_bytes());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util_cli::{Harness, Output};
    use std::path::Path;

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn script_follows_iterm2() {
        assert_eq!(sha256_hex(vendored().as_bytes()), SCRIPT_SHA256);
        let (old, new) = (vendored(), script());
        let (old_lines, new_lines): (Vec<&str>, Vec<&str>) = (old.split('\n').collect(), new.split('\n').collect());
        assert_eq!(old_lines.len(), new_lines.len());
        let changed: Vec<usize> = (0..old_lines.len()).filter(|&i| old_lines[i] != new_lines[i]).map(|i| i + 1).collect();
        assert_eq!(changed, vec![535, 659], "only the two texts naming iTerm2");
        assert!(new.contains("description='Termurion CLI'"));
        assert!(new.contains("(represented as a tab in Termurion)"));
        assert!(!new.contains("iTerm2"));
        assert!(new.contains("import iterm2"));
        assert!(new.contains("The current version of Python doesn't have the 'iterm2' module installed. Please run:"));
    }

    #[test]
    fn python_follows_windows() {
        let path = r"C:\Users\u\AppData\Local\Microsoft\WindowsApps;C:\Py312;C:\Windows";
        let files = [r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\python.exe", r"C:\Py312\python.exe", r"C:\Windows\py.exe"];
        let is_file = |p: &Path| files.iter().any(|f| Path::new(f) == p);
        assert_eq!(
            python_candidates(path, &is_file),
            vec![vec![r"C:\Windows\py.exe".to_string(), "-3".to_string()], vec![r"C:\Py312\python.exe".to_string()]]
        );
        assert!(python_candidates(path, &|_: &Path| false).is_empty());
        // A py.exe under WindowsApps is never a candidate either.
        let apps = r"C:\Users\u\AppData\Local\Microsoft\WindowsApps";
        assert!(python_candidates(apps, &|_: &Path| true).is_empty(), "nothing under WindowsApps");
        // The alias is recognized with forward slashes and in any case too.
        for apps in [r"C:/Users/u/AppData/Local/Microsoft/WindowsApps", r"C:/Users/u/AppData/Local/Microsoft/windowsapps/", r"C:\Users\u\AppData\Local\Microsoft\WINDOWSAPPS"] {
            assert!(python_candidates(apps, &|_: &Path| true).is_empty(), "nothing under {apps}");
        }
        // Only absolute, local PATH folders are ever looked in: never the working folder, a
        // relative, drive-relative or rooted folder, or a network one (the Script runtime's
        // `find_backend` uses this list too).
        let probed = std::cell::RefCell::new(Vec::new());
        let every = |p: &Path| {
            probed.borrow_mut().push(p.to_path_buf());
            true
        };
        let unsafe_path = r".;bin;C:rel;\rooted;\\host\s\py;//host/s/py;\\?\UNC\host\s\py;\\?\GLOBALROOT\Device\Mup\host\s;C:\Py312";
        assert_eq!(
            python_candidates(unsafe_path, &every),
            vec![
                vec![r"C:\Py312\py.exe".to_string(), "-3".to_string()],
                vec![r"C:\Py312\python.exe".to_string()],
                vec![r"C:\Py312\python3.exe".to_string()],
            ]
        );
        assert!(probed.borrow().iter().all(|p| p.starts_with(r"C:\Py312")), "probed {:?}", probed.borrow());

        assert_eq!(no_python(), "it2api needs Python 3. Install it, then run:\n\n    $ py -m pip install iterm2\n");
        let own = Path::new(r"C:\T\utilities");
        let script = r"C:\T\utilities\it2api.py";
        let args: Vec<String> = vec!["list-sessions".into()];

        // No Python.
        let mut h = Harness { env: [("PATH".to_string(), path.to_string())].into(), ..Default::default() };
        assert_eq!(h.with_io(&mut |io| launch(&args, io, own, &|_: &Path| false)), 1);
        assert_eq!(String::from_utf8(h.err.clone()).unwrap(), no_python());
        assert!(h.out.is_empty() && h.runs.is_empty());

        // A Python, no script.
        let mut h = Harness { env: [("PATH".to_string(), path.to_string())].into(), ..Default::default() };
        assert_eq!(h.with_io(&mut |io| launch(&args, io, own, &is_file)), 1);
        assert_eq!(String::from_utf8(h.err.clone()).unwrap(), no_script(own));
        assert!(h.runs.is_empty());

        // Both: the script runs with the arguments, streams shared, and its code is returned.
        let with_script = |p: &Path| is_file(p) || p == Path::new(script);
        let mut h = Harness { env: [("PATH".to_string(), path.to_string())].into(), ..Default::default() };
        h.answers.push_back(Ok(Output { code: 7, stdout: Vec::new() }));
        assert_eq!(h.with_io(&mut |io| launch(&args, io, own, &with_script)), 7);
        assert_eq!(h.runs.len(), 1);
        let c = &h.runs[0];
        let argv: Vec<&str> = std::iter::once(c.program.as_str()).chain(c.args.iter().map(String::as_str)).collect();
        assert_eq!(argv, vec![r"C:\Windows\py.exe", "-3", script, "list-sessions"]);
        assert!(c.inherit);
        assert!(h.err.is_empty() && h.out.is_empty());

        // A Python that cannot start: its error, exit 1.
        let mut h = Harness { env: [("PATH".to_string(), path.to_string())].into(), ..Default::default() };
        h.answers.push_back(Err("not found".into()));
        assert_eq!(h.with_io(&mut |io| launch(&args, io, own, &with_script)), 1);
        assert_eq!(String::from_utf8(h.err.clone()).unwrap(), "it2api: C:\\Windows\\py.exe: not found\n");
    }

    #[test]
    fn it2api_entry_needs_python() {
        // Through `util_cli::run`'s arm: no Python on PATH prints `no_python()`, exit 1.
        let mut h = Harness { env: [("PATH".to_string(), r"C:\termurion-no-such-folder".to_string())].into(), ..Default::default() };
        assert_eq!(h.run("it2api", &["list-sessions"]), 1);
        assert_eq!(String::from_utf8(h.err.clone()).unwrap(), no_python());
        assert!(h.out.is_empty() && h.runs.is_empty());
    }

    #[test]
    fn script_path_follows_the_program() {
        let own = Path::new(r"C:\T");
        let beside = Path::new(r"C:\T\it2api.py");
        let inside = Path::new(r"C:\T\utilities\it2api.py");
        assert_eq!(script_path(own, &|p: &Path| p == beside || p == inside), Some(beside.to_path_buf()));
        assert_eq!(script_path(own, &|p: &Path| p == inside), Some(inside.to_path_buf()));
        assert_eq!(script_path(own, &|_: &Path| false), None);
        let text = no_script(own);
        assert!(text.starts_with("it2api: it2api.py is not in C:\\T or C:\\T\\utilities."), "{text}");
        assert!(text.ends_with('\n'));
    }

    /// A Python with the stock `iterm2` package: this checkout's `target\\api-venv`, else the
    /// main checkout's (11a's), else `py -3`. Nothing is ever installed or downloaded here.
    #[cfg(windows)]
    fn python_with_iterm2() -> Option<(PathBuf, Vec<&'static str>)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let venv = |r: &Path| r.join("target").join("api-venv").join("Scripts").join("python.exe");
        let mut candidates = vec![(venv(root), vec![])];
        if let Some(parent) = root.parent() {
            candidates.push((venv(&parent.join("termurion")), vec![]));
        }
        candidates.push((PathBuf::from("py"), vec!["-3"]));
        candidates.into_iter().find(|(exe, args)| {
            std::process::Command::new(exe)
                .args(args)
                .args(["-c", "import iterm2"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// The shipped script, run by a real Python with the stock `iterm2` package, lists the
    /// fixture's sessions and its focus through 11a's server on port 1912. Run with
    /// `scripts/xt test --lib it2api_lists -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn it2api_lists_sessions() {
        use crate::api_server::{CookieCheck, Event, Server};
        use std::io::Read;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let Some((python, py_args)) = python_with_iterm2() else {
            eprintln!(
                "skipped: no Python with the iterm2 package. This test never installs anything; to run it, \
                 prepare target\\api-venv yourself (py -3 -m venv target\\api-venv, then its pip install iterm2 \"websockets<14\")."
            );
            return;
        };
        let jar = Arc::new(Mutex::new(crate::api_auth::Jar::default()));
        let cookie = jar.lock().unwrap().mint(crate::api_auth::CookieUse::Session);
        let check: CookieCheck = {
            let jar = jar.clone();
            Arc::new(move |c: &str| jar.lock().unwrap().consume(c))
        };
        let pid = std::process::id();
        let s = match Server::start(crate::api_http::PORT, Arc::new(move |_, _| Ok(pid)), check, Arc::new(|| {})) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipped: port {} is taken: {e}", crate::api_http::PORT);
                return;
            }
        };
        let dir = crate::util_cli::TempDir::new("termurion-it2api");
        let file = dir.join("it2api.py");
        std::fs::write(&file, script()).unwrap();

        /// The script's process, killed on every way out of the test (a failed assert included).
        struct Script(Option<std::process::Child>);
        impl Drop for Script {
            fn drop(&mut self) {
                if let Some(c) = self.0.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        }
        let model = crate::api_model::fixture();
        let run = |sub: &str| -> (Option<i32>, String, String) {
            let child = std::process::Command::new(&python)
                .args(&py_args)
                .arg(&file)
                .arg(sub)
                .env("ITERM2_COOKIE", &cookie)
                .env_remove("ITERM2_KEY")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut process = Script(Some(child));
            let child = process.0.as_mut().unwrap();
            // Both pipes are drained while the script runs, so a long output never blocks it.
            let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    if let Some(mut p) = pipe {
                        let _ = p.read_to_end(&mut bytes);
                    }
                    bytes
                })
            };
            let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
            let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
            let mut subs = crate::api_dispatch::Subscriptions::default();
            let deadline = Instant::now() + Duration::from_secs(60);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    break;
                }
                match s.try_event() {
                    Some(Event::Pending(p)) => s.verdict(p.conn, true),
                    Some(Event::Message { conn, bytes }) => {
                        let Some(req) = crate::api_proto::decode(&bytes) else { continue };
                        let reply = match crate::api_dispatch::route(&mut subs, conn, req) {
                            crate::api_dispatch::Route::Reply(m) => m,
                            crate::api_dispatch::Route::App(req) => crate::api_model::answer(&req, &model).unwrap_or_else(|| crate::api_proto::malformed(&req)),
                        };
                        s.send(conn, crate::api_proto::encode(&reply));
                    }
                    Some(Event::Rejected { reason, .. }) => eprintln!("refused: {reason:?}"),
                    Some(Event::Closed { .. }) => {}
                    None => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            let status = process.0.take().unwrap().wait().unwrap();
            let (out, err) = (stdout.join().unwrap(), stderr.join().unwrap());
            (status.code(), String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned())
        };

        let (code, out, err) = run("list-sessions");
        eprintln!("MEASURED list-sessions exit {code:?}\n{out}\nstderr: {err}");
        assert_eq!(code, Some(0), "stderr: {err}");
        for id in ["1", "2", "3", "4", "7", "8"] {
            assert!(out.contains(&format!("id={id}")), "session {id} listed: {out}");
        }
        assert!(out.contains("Buried sessions:"), "{out}");

        let (code, out, err) = run("show-focus");
        eprintln!("MEASURED show-focus exit {code:?}\n{out}\nstderr: {err}");
        assert_eq!(code, Some(0), "stderr: {err}");
        assert!(out.contains("Key window: pty-A"), "{out}");
    }
}
