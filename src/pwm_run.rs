//! Running a password manager CLI (`op`, `lpass`) as a child process. `Request`, `Output` and
//! the `Runner` trait are pure (no Windows dependency), so `pwm_cli::OnePassword<R>` and
//! `pwm_cli::LastPass<R>` are testable against a fake `Runner`. The real child-process runner is
//! `#[cfg(windows)]`. A secret never reaches a command-line argument: it travels only as
//! `Request::stdin` or an entry in `Request::env` (the session token, `OP_SESSION_my`).

use std::time::Duration;

use crate::pwm::Secret;

/// One command to run: the program, its arguments (never a secret), environment variables (a
/// session token travels this way), data to write to stdin then close (a password or master
/// password, when the CLI takes one that way), and an optional timeout.
pub struct Request {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, Secret)>,
    pub stdin: Option<Secret>,
    pub timeout: Option<Duration>,
}

impl Request {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Request {
        Request { program: program.into(), args, env: Vec::new(), stdin: None, timeout: None }
    }
}

/// A finished command: its exit code, stdout (a `Secret`: a CLI's stdout is often a password, a
/// session token, or JSON carrying one) and stderr as plain text. Callers never show stderr for
/// a command that can carry a secret (`op`'s and `lpass`'s authentication errors can quote the
/// master password back), so it is kept as a bare `String` only for commands that cannot.
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: Secret,
    pub stderr: String,
}

/// Runs one `Request` to completion. Every call may block (a real process spawn and wait), so
/// only the password manager's worker thread calls it. `Send` so that thread can own one; a
/// test uses a fake that records the `Request`s it was given and returns canned `Output`s.
pub trait Runner: Send {
    fn run(&mut self, r: Request) -> Result<Output, String>;
}

/// The exact error every `Runner` returns when a request's own timeout elapses, so a caller can
/// tell a timeout apart from every other failure (a bad path, a real CLI error) without matching
/// on a formatted message, and map it to iTerm2's "Timeout" texts instead of a generic one.
pub const TIMED_OUT: &str = "pwm_run: timed out";

/// How long a command's output is given to finish arriving once the process has exited or been
/// killed, when the request itself set no timeout (a generous safety net: a normal command's
/// pipes close within milliseconds of exit).
const UNTIMED_DRAIN: Duration = Duration::from_secs(5);
/// The least time a timed-out request's own deadline still gives the drain, even if the deadline
/// has already passed by the time the child is confirmed dead.
const MIN_DRAIN: Duration = Duration::from_millis(200);

#[cfg(windows)]
mod win {
    use super::*;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    /// No console flashes for a CLI Tandem spawns on the user's behalf.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// How often a timed-out wait polls the child (`git_run_win.rs` uses the same idea with a
    /// Job Object; a plain `op`/`lpass` invocation never spawns children of its own, so a
    /// simple poll-and-kill is enough here).
    const POLL: Duration = Duration::from_millis(20);

    /// Runs `op` and `lpass` as real child processes.
    #[derive(Default)]
    pub struct ProcessRunner;

    impl ProcessRunner {
        pub fn new() -> ProcessRunner {
            ProcessRunner
        }
    }

    impl Runner for ProcessRunner {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            let start = Instant::now();
            let mut cmd = Command::new(&r.program);
            cmd.args(&r.args);
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd.stdin(if r.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            for (k, v) in &r.env {
                cmd.env(k, v.expose());
            }
            let mut child = cmd.spawn().map_err(|e| format!("{} could not start: {e}", r.program))?;

            if let Some(input) = r.stdin {
                let mut stdin = child.stdin.take().expect("piped stdin");
                std::thread::spawn(move || {
                    let mut bytes = input.expose().as_bytes().to_vec();
                    let _ = stdin.write_all(&bytes);
                    drop(stdin);
                    // The write leaves a copy of the secret in this buffer; zero it at once
                    // (the caller's own `Secret` is dropped, and zeroed, by its normal owner).
                    zero(&mut bytes);
                });
            }

            let mut stdout_pipe = child.stdout.take().expect("piped stdout");
            let mut stderr_pipe = child.stderr.take().expect("piped stderr");
            let (tx_out, rx_out) = mpsc::channel();
            let (tx_err, rx_err) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stdout_pipe.read_to_end(&mut buf);
                let _ = tx_out.send(buf);
            });
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr_pipe.read_to_end(&mut buf);
                let _ = tx_err.send(buf);
            });

            let status = match r.timeout {
                Some(d) => match wait_with_timeout(&mut child, d) {
                    Some(status) => status,
                    None => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(TIMED_OUT.to_string());
                    }
                },
                None => child.wait().map_err(|e| e.to_string())?,
            };
            // Bounded so a grandchild that inherited the pipe handles (one that did not
            // redirect its own stdio) cannot hang the worker forever even though the process
            // we started has already exited: a request with its own timeout gets whatever is
            // left of it (at least MIN_DRAIN), an untimed one gets a flat safety net.
            let drain = match r.timeout {
                Some(d) => (start + d).saturating_duration_since(Instant::now()).max(MIN_DRAIN),
                None => UNTIMED_DRAIN,
            };
            let stdout_bytes = rx_out.recv_timeout(drain).unwrap_or_default();
            let stderr_bytes = rx_err.recv_timeout(drain).unwrap_or_default();
            // `from_utf8` moves the buffer into the String at no cost when it is already valid
            // UTF-8 (the common case); only the rare invalid-UTF-8 path makes a lossy copy, and
            // that copy's source bytes are zeroed right after.
            let stdout = match String::from_utf8(stdout_bytes) {
                Ok(s) => Secret::new(s),
                Err(e) => {
                    let mut raw = e.into_bytes();
                    let lossy = Secret::new(String::from_utf8_lossy(&raw).into_owned());
                    zero(&mut raw);
                    lossy
                }
            };
            Ok(Output { code: status.code().unwrap_or(-1), stdout, stderr: String::from_utf8_lossy(&stderr_bytes).into_owned() })
        }
    }

    /// Overwrites a buffer with zeros so a secret's plaintext copy does not linger past its use.
    fn zero(bytes: &mut [u8]) {
        for b in bytes.iter_mut() {
            // SAFETY: a plain byte write into our own buffer.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }

    /// Polls the child until it exits or `timeout` passes; `None` on a timeout (the child is
    /// left running for the caller to kill).
    fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Some(status);
            }
            if start.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The real runner: an exit code and stdout come back, stdin reaches the child, and a
        /// timeout kills it and reports the `TIMED_OUT` sentinel (never a raw message a real
        /// CLI's own timeout text could be mistaken for). Uses only `cmd.exe` and `sort`, never
        /// a real `op` or `lpass`.
        #[test]
        fn process_runner_runs_reads_and_times_out() {
            let mut runner = ProcessRunner::new();
            let out = runner.run(Request::new("cmd.exe", vec!["/c".to_string(), "exit".to_string(), "3".to_string()])).unwrap();
            assert_eq!(out.code, 3);

            let mut echo = Request::new("cmd.exe", vec!["/c".to_string(), "sort".to_string()]);
            echo.stdin = Some(Secret::new("hello".to_string()));
            let out = runner.run(echo).unwrap();
            assert_eq!(out.stdout.expose().trim(), "hello");

            let mut hang = Request::new("cmd.exe", vec!["/d".to_string(), "/c".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()]);
            hang.timeout = Some(Duration::from_millis(300));
            let start = Instant::now();
            let err = runner.run(hang).unwrap_err();
            assert_eq!(err, TIMED_OUT);
            assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        }
    }
}

#[cfg(windows)]
pub use win::ProcessRunner;

#[cfg(test)]
mod tests {
    use super::*;

    /// The pure shapes compile and hold what they are given; the real process runner is
    /// `#[cfg(windows)]` and is exercised by the CLI sources' own tests through a fake `Runner`
    /// (this milestone's checklist covers the real child process by hand, as with Credential
    /// Manager in Task 3).
    #[test]
    fn request_and_output_are_plain_data() {
        let r = Request::new("op", vec!["item".to_string(), "list".to_string()]);
        assert_eq!(r.program, "op");
        assert_eq!(r.args, vec!["item".to_string(), "list".to_string()]);
        assert!(r.env.is_empty());
        assert!(r.stdin.is_none());
        assert!(r.timeout.is_none());

        let o = Output { code: 0, stdout: Secret::new("ok".to_string()), stderr: String::new() };
        assert_eq!(o.code, 0);
        assert_eq!(o.stdout.expose(), "ok");
    }
}
