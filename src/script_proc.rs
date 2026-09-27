//! One launched script's process (iTerm2's `NSTask` in `tryLaunchScript:`, `L:799-853`, and
//! `waitForTask:`, `L:941-1012`): hidden, stdin empty, stdout and stderr on one pipe, in a Job
//! Object so the script and whatever it starts end together (Terminate, and Tandem quitting).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::pty::Notify;
use crate::script_provision::KillJob;

/// What the reader thread sends: output in order, then the exit code once the output ended
/// and the process was waited for.
enum Msg {
    Output(Vec<u8>),
    Exit(u32),
}

/// Exit code for a process that could not be waited for (reads as killed, `exit_text`).
const LOST: u32 = u32::MAX;

pub struct ScriptProcess {
    pid: u32,
    job: KillJob,
    rx: Receiver<Msg>,
    /// Output taken off the channel by `exit_code` before `try_output` asked for it.
    stash: VecDeque<Vec<u8>>,
    exit: Option<u32>,
    /// The Script Console's decoder for this script's output.
    pub decoder: crate::script_launch::Decoder,
}

impl ScriptProcess {
    /// Starts `program` with `args`, exactly `env` as its environment and `cwd` (None: Tandem's
    /// own). `notify` wakes the app for each piece of output and at the end.
    pub fn start(program: &Path, args: &[String], env: &[(String, String)], cwd: Option<&Path>, notify: Notify) -> Result<ScriptProcess, String> {
        use std::io::Read;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        let job = KillJob::new()?;
        let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
        let err_writer = writer.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(err_writer)
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let spawned = cmd.spawn();
        // The pipe's write ends go with the command, so the read ends when the script does.
        drop(cmd);
        let mut child = spawned.map_err(|e| e.to_string())?;
        // In the job before it runs, so nothing it starts escapes the job.
        if let Err(e) = job.adopt(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let pid = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            // Read to the end of the output (every process holding the pipe gone), then wait,
            // as iTerm2 reads until end of file before `waitUntilExit`.
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(Msg::Output(buf[..n].to_vec())).is_err() {
                    break;
                }
                notify();
            }
            let code = child.wait().ok().and_then(|s| s.code()).map_or(LOST, |c| c as u32);
            let _ = tx.send(Msg::Exit(code));
            notify();
        });
        Ok(ScriptProcess { pid, job, rx, stash: VecDeque::new(), exit: None, decoder: Default::default() })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Takes one message off the channel, if one waits.
    fn receive(&mut self) -> Option<Vec<u8>> {
        if self.exit.is_some() {
            return None;
        }
        match self.rx.try_recv() {
            Ok(Msg::Output(b)) => Some(b),
            Ok(Msg::Exit(code)) => {
                self.exit = Some(code);
                None
            }
            Err(TryRecvError::Empty) => None,
            // The reader thread is gone without a word.
            Err(TryRecvError::Disconnected) => {
                self.exit = Some(LOST);
                None
            }
        }
    }

    /// Output that arrived, oldest first; None when none waits.
    pub fn try_output(&mut self) -> Option<Vec<u8>> {
        self.stash.pop_front().or_else(|| self.receive())
    }

    /// The exit code once the script ended and all its output was taken.
    pub fn exit_code(&mut self) -> Option<u32> {
        while let Some(b) = self.receive() {
            self.stash.push_back(b);
        }
        if self.stash.is_empty() { self.exit } else { None }
    }

    /// Ends the script and everything it started (`TerminateJobObject`, exit code 1).
    pub fn terminate(&mut self) {
        self.job.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn env_with(extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        env
    }

    /// Output and exit code within `limit`.
    fn run_out(p: &mut ScriptProcess, limit: Duration) -> (String, Option<u32>) {
        let deadline = Instant::now() + limit;
        let mut out = Vec::new();
        loop {
            while let Some(b) = p.try_output() {
                out.extend(b);
            }
            if let Some(code) = p.exit_code() {
                return (String::from_utf8_lossy(&out).into_owned(), Some(code));
            }
            if Instant::now() > deadline {
                return (String::from_utf8_lossy(&out).into_owned(), None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn script_process_runs_and_ends() {
        let quiet: Notify = Arc::new(|| {});
        let mut p = ScriptProcess::start(Path::new("cmd.exe"), &args(&["/c", "echo out& echo err 1>&2& exit 3"]), &env_with(&[]), None, quiet.clone()).unwrap();
        assert_ne!(p.pid(), 0);
        let (out, code) = run_out(&mut p, Duration::from_secs(20));
        assert!(out.contains("out") && out.contains("err"), "{out:?}");
        assert_eq!(code, Some(3));

        let mut p = ScriptProcess::start(Path::new("ping.exe"), &args(&["-n", "30", "127.0.0.1"]), &env_with(&[]), None, quiet.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        p.terminate();
        let (_, code) = run_out(&mut p, Duration::from_secs(5));
        assert_eq!(code, Some(1), "ended after {:?}", t.elapsed());

        let mut p = ScriptProcess::start(Path::new("cmd.exe"), &args(&["/c", "echo %TANDEM_T%"]), &env_with(&[("TANDEM_T", "1")]), None, quiet).unwrap();
        let (out, code) = run_out(&mut p, Duration::from_secs(20));
        assert_eq!((out.trim(), code), ("1", Some(0)));
    }

    /// 11a's server on loopback runs a basic script launched as the Scripts menu does
    /// (`ScriptProcess::start`, `launch_env`, a Single cookie); the stock `iterm2` package
    /// builds its App from the fixture model. Needs `target\api-venv` (made by 11a's
    /// `stock_iterm2_package_connects` instructions); never creates or downloads anything.
    /// Run with `scripts/xt test --lib stock_script -- --ignored`.
    #[test]
    #[ignore]
    fn stock_script_launches() {
        use crate::api_server::{CookieCheck, Event, Server};
        let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("api-venv");
        let python = crate::script_runtime::venv_python(&venv);
        if !python.is_file() {
            eprintln!("skipped: {} is missing. This test never installs anything.", python.display());
            return;
        }
        let jar = Arc::new(std::sync::Mutex::new(crate::api_auth::Jar::default()));
        let cookie = jar.lock().unwrap().mint(crate::api_auth::CookieUse::Single);
        let check: CookieCheck = {
            let jar = jar.clone();
            Arc::new(move |c: &str| jar.lock().unwrap().consume(c))
        };
        let me = std::process::id();
        let s = match Server::start(crate::api_http::PORT, Arc::new(move |_, _| Ok(me)), check, Arc::new(|| {})) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipped: port {} is taken: {e}", crate::api_http::PORT);
                return;
            }
        };
        let dir = std::env::temp_dir().join(format!("tandem-stock-script-{me}"));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("count_windows.py");
        std::fs::write(&script, "import iterm2\n\nasync def main(c):\n    print(len((await iterm2.async_get_app(c)).windows))\n\niterm2.run_until_complete(main)\n").unwrap();
        let base: Vec<(String, String)> = env_with(&[]);
        let key = crate::api_auth::fresh_key();
        let env = crate::script_launch::launch_env(base, &cookie, &key, Some(&venv), crate::script_runtime::certifi_path(&venv).is_file());
        let mut p = ScriptProcess::start(&python, &[script.to_string_lossy().into_owned()], &env, None, Arc::new(|| {})).unwrap();
        let model = crate::api_model::fixture();
        let mut subs = crate::api_dispatch::Subscriptions::default();
        let (mut out, mut rejected) = (Vec::new(), Vec::new());
        let deadline = Instant::now() + Duration::from_secs(60);
        let code = loop {
            while let Some(b) = p.try_output() {
                out.extend(b);
            }
            if let Some(code) = p.exit_code() {
                break Some(code);
            }
            if Instant::now() > deadline {
                p.terminate();
                break None;
            }
            match s.try_event() {
                Some(Event::Pending(pending)) => {
                    assert!(pending.upgrade.preauthorized, "the Single cookie authorizes the script");
                    assert_eq!(pending.upgrade.key, key);
                    s.verdict(pending.conn, true);
                }
                Some(Event::Message { conn, bytes }) => {
                    let Some(req) = crate::api_proto::decode(&bytes) else { continue };
                    let reply = match crate::api_dispatch::route(&mut subs, conn, req) {
                        crate::api_dispatch::Route::Reply(m) => m,
                        crate::api_dispatch::Route::App(req) => crate::api_model::answer(&req, &model).unwrap_or_else(|| crate::api_proto::malformed(&req)),
                    };
                    s.send(conn, crate::api_proto::encode(&reply));
                }
                Some(Event::Rejected { reason, .. }) => rejected.push(reason),
                Some(Event::Closed { .. }) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        let _ = std::fs::remove_dir_all(&dir);
        let out = String::from_utf8_lossy(&out).into_owned();
        eprintln!("stock_script_launches output: {out:?}, exit {code:?}");
        assert_eq!((out.trim(), code), ("2", Some(0)), "refusals: {rejected:?}");
    }
}
