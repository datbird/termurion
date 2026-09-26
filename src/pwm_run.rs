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
                    for b in bytes.iter_mut() {
                        // SAFETY: a plain byte write into our own buffer.
                        unsafe { std::ptr::write_volatile(b, 0) };
                    }
                    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
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
                let mut buf = String::new();
                let _ = stderr_pipe.read_to_string(&mut buf);
                let _ = tx_err.send(buf);
            });

            let status = match r.timeout {
                Some(d) => match wait_with_timeout(&mut child, d) {
                    Some(status) => status,
                    None => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!("{} timed out", r.program));
                    }
                },
                None => child.wait().map_err(|e| e.to_string())?,
            };
            let stdout_bytes = rx_out.recv().unwrap_or_default();
            let stderr = rx_err.recv().unwrap_or_default();
            Ok(Output { code: status.code().unwrap_or(-1), stdout: Secret::new(String::from_utf8_lossy(&stdout_bytes).into_owned()), stderr })
        }
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
