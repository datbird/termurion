//! One launched script's process (iTerm2's `NSTask` in `tryLaunchScript:`, `L:799-853`, and
//! `waitForTask:`, `L:941-1012`): hidden, stdin empty, stdout and stderr on one pipe, in a Job
//! Object so Terminate and an API stop end the script and whatever it started. When the script
//! ends by itself the job is handed back (`into_job`), so its helpers keep running as in iTerm2
//! until they end or Termurion quits.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use crate::platform::pty::Notify;
use crate::script_provision::KillJob;

/// What the reader thread sends: output in order, then the exit code once the output ended
/// and the process was waited for.
enum Msg {
    Output(Vec<u8>),
    Exit(u32),
}

/// Exit code for a process that could not be waited for (reads as killed, `exit_text`).
const LOST: u32 = u32::MAX;

/// The exit code Terminate and an API stop give (`STATUS_CONTROL_C_EXIT`), which reads as
/// "killed by a signal" as iTerm2's SIGHUP and signal 9 do (`SH:182-212`).
pub const KILLED_CODE: u32 = 0xC000_013A;

/// Output chunks the reader may queue before it waits (the script then waits on its pipe, as
/// in a terminal), and each chunk's most bytes.
const QUEUED_CHUNKS: usize = 16;
const CHUNK: usize = 8192;

pub struct ScriptProcess {
    pid: u32,
    job: KillJob,
    rx: Receiver<Msg>,
    /// At most one chunk `exit_code` took off the channel before `take_output` asked for it.
    stash: VecDeque<Vec<u8>>,
    exit: Option<u32>,
    /// Set by the reader when it woke the app, cleared when output is taken: one wake for any
    /// number of chunks.
    woken: Arc<AtomicBool>,
    notify: Notify,
    /// The Script Console's decoder for this script's output.
    pub decoder: crate::script_launch::Decoder,
    /// The one-time cookie the script was given (set by the launcher), removed from the jar
    /// when the script ends; never logged.
    pub cookie: String,
}

impl ScriptProcess {
    /// Starts `program` with `args`, exactly `env` as its environment and `cwd` (None: Termurion's
    /// own). `notify` wakes the app when output waits and at the end.
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
            // CreateProcess opens the working folder: judged first, like any probe.
            let dir = crate::localfs::LocalPath::judge(dir).ok_or_else(|| crate::localfs::refused().to_string())?;
            crate::localfs::set_cwd(&mut cmd, &dir);
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
        let (tx, rx) = std::sync::mpsc::sync_channel(QUEUED_CHUNKS);
        let woken = Arc::new(AtomicBool::new(false));
        let (reader_woken, reader_notify) = (woken.clone(), notify.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; CHUNK];
            // Read to the end of the output (every process holding the pipe gone), then wait,
            // as iTerm2 reads until end of file before `waitUntilExit`.
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(Msg::Output(buf[..n].to_vec())).is_err() {
                    break;
                }
                if !reader_woken.swap(true, Ordering::AcqRel) {
                    reader_notify();
                }
            }
            let code = child.wait().ok().and_then(|s| s.code()).map_or(LOST, |c| c as u32);
            let _ = tx.send(Msg::Exit(code));
            reader_notify();
        });
        Ok(ScriptProcess { pid, job, rx, stash: VecDeque::new(), exit: None, woken, notify, decoder: Default::default(), cookie: String::new() })
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

    /// Output that arrived, oldest first, about `cap` bytes at most (whole chunks); when more
    /// may wait the app is woken again, so a script that never stops printing is read a piece
    /// per pass and the app stays responsive.
    pub fn take_output(&mut self, cap: usize) -> Vec<u8> {
        self.woken.store(false, Ordering::Release);
        let mut out = Vec::new();
        while out.len() < cap {
            match self.stash.pop_front().or_else(|| self.receive()) {
                Some(b) => out.extend(b),
                None => return out,
            }
        }
        (self.notify)();
        out
    }

    /// The exit code once the script ended and all its output was taken.
    pub fn exit_code(&mut self) -> Option<u32> {
        if self.stash.is_empty()
            && let Some(b) = self.receive() {
                self.stash.push_back(b);
            }
        if self.stash.is_empty() { self.exit } else { None }
    }

    /// Ends the script and everything it started (`TerminateJobObject` with `KILLED_CODE`).
    pub fn terminate(&mut self) {
        self.job.terminate_with(KILLED_CODE);
    }

    /// How many of the script's processes (it and what it started) still run.
    pub fn active_processes(&self) -> u32 {
        self.job.active_processes()
    }

    /// The script's job, for keeping what it started running after it ended by itself.
    pub(crate) fn into_job(self) -> KillJob {
        self.job
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
            out.extend(p.take_output(usize::MAX));
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

        // Terminate ends the script and what it started (a grandchild ping here), and reads
        // as killed.
        let mut p = ScriptProcess::start(Path::new("cmd.exe"), &args(&["/c", "start /b ping.exe -n 30 127.0.0.1 & ping.exe -n 30 127.0.0.1"]), &env_with(&[]), None, quiet.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while p.active_processes() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(p.active_processes() >= 3, "cmd and both pings: {}", p.active_processes());
        let t = Instant::now();
        p.terminate();
        let (_, code) = run_out(&mut p, Duration::from_secs(5));
        assert_eq!(code, Some(KILLED_CODE), "ended after {:?}", t.elapsed());
        assert_eq!(crate::script_launch::exit_text(KILLED_CODE).as_deref(), Some(crate::script_launch::KILLED));
        assert_eq!(p.active_processes(), 0, "the grandchild ended with the job");

        // Output is taken a piece at a time; a piece past the cap wakes the app again.
        let woke = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = woke.clone();
        let mut p = ScriptProcess::start(Path::new("cmd.exe"), &args(&["/c", "for /l %i in (1,1,400) do @echo 0123456789012345678901234567890123456789"]), &env_with(&[]), None, Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut total = 0;
        let mut pieces = 0;
        while p.exit_code().is_none() && Instant::now() < deadline {
            let piece = p.take_output(100);
            assert!(piece.len() < 100 + CHUNK, "{}", piece.len());
            total += piece.len();
            pieces += usize::from(!piece.is_empty());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(p.exit_code(), Some(0));
        assert!(total >= 400 * 40 && pieces > 1, "{total} bytes in {pieces} pieces");
        assert!(woke.load(std::sync::atomic::Ordering::SeqCst) >= 1);

        let mut p = ScriptProcess::start(Path::new("cmd.exe"), &args(&["/c", "echo %TERMURION_T%"]), &env_with(&[("TERMURION_T", "1")]), None, quiet).unwrap();
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
        let dir = std::env::temp_dir().join(format!("termurion-stock-script-{me}"));
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
            out.extend(p.take_output(usize::MAX));
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
