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
    /// Mid-command re-authentication (LastPass): see `Interactive`.
    pub interactive: Option<Interactive>,
}

impl Request {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Request {
        Request { program: program.into(), args, env: Vec::new(), stdin: None, timeout: None, interactive: None }
    }
}

/// A finished command: its exit code, stdout (a `Secret`: a CLI's stdout is often a password, a
/// session token, or JSON carrying one) and stderr as plain text. Callers only look into stderr
/// to tell failures apart (an expired session) and never show it for any command (`op`'s and
/// `lpass`'s authentication errors can quote the master password back): a failure shows one of
/// the fixed texts.
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

/// The exact error a `Runner` returns when the person canceled the re-authentication prompt
/// (the child was killed), a sentinel like `TIMED_OUT`.
pub const CANCELED: &str = "pwm_run: canceled";

/// The exact error a `Runner` returns when a command that was given the master password asked
/// for it again and then failed (the answer was wrong): the pipe was closed after the one
/// answer, so the child failed instead of waiting for more.
pub const WRONG_PASSWORD: &str = "pwm_run: wrong password";

/// One mid-command ask: the channel its answer goes back on, and a flag the runner sets when the
/// request has ended (the child exited, timed out or was canceled), so the asker can skip an
/// ask nobody is waiting for any more.
pub struct Ask {
    pub reply: std::sync::mpsc::Sender<Option<Secret>>,
    pub ended: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Mid-command re-authentication (`LastPassDataSource.swift:36-54`). While the child runs its
/// stderr is watched: a message `needs_login` accepts stops the watch (the caller maps it); the
/// first other message sends a reply channel on `ask` and waits for the person's answer (the
/// request's clock stops while the person answers: Decision, 13y2 Task 5 review I1), writes the
/// password plus a newline to the open stdin and closes it on `Some` (a second prompt then reads
/// end of file and the command fails: `WRONG_PASSWORD`), and kills the child with `CANCELED` on
/// `None`. A request that carries its own `stdin` data closes that pipe after the write as ever
/// and is not watched.
#[derive(Clone)]
pub struct Interactive {
    pub ask: std::sync::mpsc::Sender<Ask>,
    pub needs_login: fn(&str) -> bool,
}

/// What the stderr watcher decides for the text seen so far.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Ignore,
    Ask,
}

/// The most stderr text the watcher keeps to classify (untrusted output is capped).
pub(crate) const WATCH_CAP: usize = 64 * 1024;

/// The pure part of the watcher: it sees the accumulated stderr (so a message split over two
/// reads is still recognized) and asks at most once, never once a needs-login message is seen.
pub(crate) struct Watch {
    needs_login: fn(&str) -> bool,
    asked: bool,
    login_seen: bool,
    pub(crate) text: String,
}

impl Watch {
    pub(crate) fn new(needs_login: fn(&str) -> bool) -> Watch {
        Watch { needs_login, asked: false, login_seen: false, text: String::new() }
    }

    pub(crate) fn feed(&mut self, chunk: &str) -> Step {
        if chunk.is_empty() {
            return Step::Ignore;
        }
        self.text.push_str(chunk);
        if self.text.len() > WATCH_CAP {
            let mut cut = self.text.len() - WATCH_CAP;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
        if self.login_seen || (self.needs_login)(&self.text) {
            self.login_seen = true;
            return Step::Ignore;
        }
        if self.asked {
            return Step::Ignore;
        }
        self.asked = true;
        Step::Ask
    }
}

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
    use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
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
            // A request with its own stdin data closes the pipe after the write as ever; only
            // one without is watched for a mid-command password prompt (pipe kept open).
            let watched = r.interactive.clone().filter(|_| r.stdin.is_none());
            cmd.stdin(if r.stdin.is_some() || watched.is_some() { Stdio::piped() } else { Stdio::null() });
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
            let stdin_slot: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(if watched.is_some() { child.stdin.take() } else { None }));
            let canceled = Arc::new(AtomicBool::new(false));
            let exited = Arc::new(AtomicBool::new(false));
            // Set once the password was written and stdin closed; text after that is a re-prompt.
            let answered = Arc::new(AtomicBool::new(false));
            let reprompted = Arc::new(AtomicBool::new(false));
            let clock = Arc::new(Clock::default());
            let (tx_out, rx_out) = mpsc::channel();
            let (tx_err, rx_err) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stdout_pipe.read_to_end(&mut buf);
                let _ = tx_out.send(buf);
            });
            let (slot, cancel_flag, exit_flag, answered_flag, reprompt_flag, ask_clock) = (stdin_slot.clone(), canceled.clone(), exited.clone(), answered.clone(), reprompted.clone(), clock.clone());
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                match watched {
                    None => {
                        let _ = stderr_pipe.read_to_end(&mut buf);
                    }
                    Some(i) => {
                        let mut watch = Watch::new(i.needs_login);
                        let mut chunk = [0u8; 4096];
                        while let Ok(n) = stderr_pipe.read(&mut chunk) {
                            if n == 0 {
                                break;
                            }
                            if buf.len() < WATCH_CAP {
                                buf.extend_from_slice(&chunk[..n]);
                            }
                            // Text after the one answer is the child asking again (or failing).
                            if answered_flag.load(Ordering::SeqCst) {
                                reprompt_flag.store(true, Ordering::SeqCst);
                            }
                            // The text is only classified, never logged or shown.
                            if watch.feed(&String::from_utf8_lossy(&chunk[..n])) == Step::Ask && !exit_flag.load(Ordering::SeqCst) {
                                ask_clock.pause();
                                let answer = ask_for_password(&i, &exit_flag);
                                ask_clock.resume();
                                match answer {
                                    Some(password) => {
                                        write_password(&slot, &password);
                                        answered_flag.store(true, Ordering::SeqCst);
                                    }
                                    None => cancel_flag.store(true, Ordering::SeqCst),
                                }
                            }
                        }
                    }
                }
                let _ = tx_err.send(buf);
            });

            let status = if r.timeout.is_some() || r.interactive.is_some() {
                match wait_polling(&mut child, r.timeout, &canceled, &clock) {
                    Waited::Done(status) => status,
                    Waited::TimedOut | Waited::Canceled => {
                        let canceled = canceled.load(Ordering::SeqCst);
                        exited.store(true, Ordering::SeqCst);
                        let _ = child.kill();
                        let _ = child.wait();
                        stdin_slot.lock().unwrap().take();
                        return Err(if canceled { CANCELED } else { TIMED_OUT }.to_string());
                    }
                }
            } else {
                child.wait().map_err(|e| e.to_string())?
            };
            exited.store(true, Ordering::SeqCst);
            stdin_slot.lock().unwrap().take();
            // Bounded so a grandchild that inherited the pipe handles (one that did not
            // redirect its own stdio) cannot hang the worker forever even though the process
            // we started has already exited: a request with its own timeout gets whatever is
            // left of it (at least MIN_DRAIN), an untimed one gets a flat safety net.
            let drain = match r.timeout {
                Some(d) => (start + d + clock.paused()).saturating_duration_since(Instant::now()).max(MIN_DRAIN),
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
            let code = status.code().unwrap_or(-1);
            // The child asked again after its one answer and failed: the answer was wrong.
            if code != 0 && reprompted.load(Ordering::SeqCst) {
                return Err(WRONG_PASSWORD.to_string());
            }
            Ok(Output { code, stdout, stderr: String::from_utf8_lossy(&stderr_bytes).into_owned() })
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

    enum Waited {
        Done(ExitStatus),
        TimedOut,
        Canceled,
    }

    /// The request's clock stops while the person answers a prompt (a person cannot read an alert
    /// and type a master password inside `ls`'s 5 s), so the time spent asking is not counted.
    #[derive(Default)]
    struct Clock {
        total: Mutex<Duration>,
        since: Mutex<Option<Instant>>,
    }

    impl Clock {
        fn pause(&self) {
            *self.since.lock().unwrap() = Some(Instant::now());
        }

        fn resume(&self) {
            if let Some(t) = self.since.lock().unwrap().take() {
                *self.total.lock().unwrap() += t.elapsed();
            }
        }

        /// Time spent asking so far, including an ask still open.
        fn paused(&self) -> Duration {
            *self.total.lock().unwrap() + self.since.lock().unwrap().map(|t| t.elapsed()).unwrap_or_default()
        }
    }

    /// Polls the child until it exits, `timeout` passes (if any, not counting time spent asking)
    /// or the person cancels; on the last two the child is left running for the caller to kill.
    fn wait_polling(child: &mut Child, timeout: Option<Duration>, canceled: &AtomicBool, clock: &Clock) -> Waited {
        let start = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Waited::Done(status);
            }
            if canceled.load(Ordering::SeqCst) {
                return Waited::Canceled;
            }
            if timeout.is_some_and(|t| start.elapsed().saturating_sub(clock.paused()) >= t) {
                return Waited::TimedOut;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Asks the person (through the app's single asker, so two never open at once) and waits.
    /// The wait gives up when the request ends (`ended`), so a stale ask never holds the thread.
    fn ask_for_password(i: &Interactive, ended: &Arc<AtomicBool>) -> Option<Secret> {
        let (reply, answer) = mpsc::channel();
        i.ask.send(Ask { reply, ended: ended.clone() }).ok()?;
        loop {
            match answer.recv_timeout(POLL) {
                Ok(a) => return a,
                Err(mpsc::RecvTimeoutError::Timeout) if !ended.load(Ordering::SeqCst) => {}
                Err(_) => return None,
            }
        }
    }

    /// Writes `password + "\n"` to the child's open stdin and closes it (a re-prompt then reads
    /// end of file instead of hanging), then zeroes the copy.
    fn write_password(slot: &Mutex<Option<ChildStdin>>, password: &Secret) {
        let mut bytes = password.expose().as_bytes().to_vec();
        bytes.push(b'\n');
        if let Some(mut stdin) = slot.lock().unwrap().take() {
            let _ = stdin.write_all(&bytes);
            let _ = stdin.flush();
        }
        zero(&mut bytes);
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

        assert!(r.interactive.is_none());
        let o = Output { code: 0, stdout: Secret::new("ok".to_string()), stderr: String::new() };
        assert_eq!(o.code, 0);
        assert_eq!(o.stdout.expose(), "ok");
    }

    /// The pure watcher: a needs-login message never asks (it ends the watch), the first other
    /// message asks once, later ones never ask again, and a login message split over two reads
    /// is still seen.
    #[test]
    fn reauth_watch_asks_once_and_never_for_needs_login() {
        let lp: fn(&str) -> bool = |t| t.contains("lpass login");
        let mut w = Watch::new(lp);
        assert_eq!(w.feed("Please enter your master password: "), Step::Ask);
        assert_eq!(w.feed("again: "), Step::Ignore);
        let mut w = Watch::new(lp);
        assert_eq!(w.feed("Please run 'lpass login' first"), Step::Ignore);
        assert_eq!(w.feed("password: "), Step::Ignore);
        let mut w = Watch::new(lp);
        assert_eq!(w.feed(""), Step::Ignore);
        assert_eq!(w.feed(&"x".repeat(WATCH_CAP * 2)), Step::Ask);
        assert!(w.text.len() <= WATCH_CAP);
    }

    #[cfg(windows)]
    mod win_tests {
        use super::super::win::ProcessRunner;
        use super::*;
        use std::sync::mpsc;

        fn script(name: &str, body: &str) -> (std::path::PathBuf, String) {
            let dir = std::env::temp_dir().join(format!("tandem-pwm-run-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("t.cmd");
            std::fs::write(&file, body).unwrap();
            (dir, file.to_string_lossy().into_owned())
        }

        fn interactive(answer: Option<&'static str>) -> (Interactive, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
            let (ask, rx) = mpsc::channel::<Ask>();
            let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let c = count.clone();
            std::thread::spawn(move || {
                for ask in rx {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = ask.reply.send(answer.map(|a| Secret::new(a.to_string())));
                }
            });
            (Interactive { ask, needs_login: |t| t.contains("lpass login") }, count)
        }

        fn request(file: &str, i: Interactive) -> Request {
            let mut r = Request::new("cmd.exe", vec!["/d".to_string(), "/c".to_string(), file.to_string()]);
            r.interactive = Some(i);
            r.timeout = Some(Duration::from_secs(20));
            r
        }

        /// Review Focus 5: the password goes to the still-open stdin once, however often the
        /// child prompts; Cancel kills the child with `CANCELED`; a needs-login message never asks.
        #[test]
        fn lastpass_reauth_follows_iterm2() {
            let body = "@echo off\r\necho Enter master password: 1>&2\r\nset /p L=\r\necho Enter master password: 1>&2\r\necho %L%\r\n";
            let (dir, file) = script("ok", body);
            let (i, count) = interactive(Some("pw"));
            let out = ProcessRunner::new().run(request(&file, i)).unwrap();
            assert_eq!(out.stdout.expose().trim(), "pw");
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
            let _ = std::fs::remove_dir_all(dir);

            let (dir, file) = script("cancel", "@echo off\r\necho Enter master password: 1>&2\r\nset /p L=\r\n");
            let (i, _) = interactive(None);
            assert_eq!(ProcessRunner::new().run(request(&file, i)).unwrap_err(), CANCELED);
            let _ = std::fs::remove_dir_all(dir);

            let (dir, file) = script("login", "@echo off\r\necho Please run 'lpass login' first 1>&2\r\nexit /b 1\r\n");
            let (i, count) = interactive(Some("pw"));
            let out = ProcessRunner::new().run(request(&file, i)).unwrap();
            assert_eq!(out.code, 1);
            assert!(out.stderr.contains("lpass login"));
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
            let _ = std::fs::remove_dir_all(dir);
        }

        /// I1: the request's clock stops while the person answers (the ask takes 1.5 s against a
        /// 500 ms timeout and the command still finishes); the clock runs again after the answer.
        #[test]
        fn reauth_clock_stops_while_waiting_for_the_answer() {
            let (dir, file) = script("slow", "@echo off\r\necho Enter master password: 1>&2\r\nset /p L=\r\necho %L%\r\n");
            let (ask, rx) = mpsc::channel::<Ask>();
            std::thread::spawn(move || {
                for ask in rx {
                    std::thread::sleep(Duration::from_millis(1500));
                    let _ = ask.reply.send(Some(Secret::new("pw".to_string())));
                }
            });
            let mut r = request(&file, Interactive { ask, needs_login: |_| false });
            r.timeout = Some(Duration::from_millis(500));
            assert_eq!(ProcessRunner::new().run(r).unwrap().stdout.expose().trim(), "pw");
            let _ = std::fs::remove_dir_all(dir);

            // After the answer the clock runs: a child that then hangs times out.
            let (dir, file) = script("hang", "@echo off\r\necho Enter master password: 1>&2\r\nset /p L=\r\nping -n 30 127.0.0.1 >nul\r\n");
            let (i, _) = interactive(Some("pw"));
            let mut r = request(&file, i);
            r.timeout = Some(Duration::from_millis(800));
            assert_eq!(ProcessRunner::new().run(r).unwrap_err(), TIMED_OUT);
            let _ = std::fs::remove_dir_all(dir);
        }

        /// C1: a wrong password makes the child prompt a second time. Stdin was closed after the
        /// one answer, so it reads end of file and fails; the request ends with `WRONG_PASSWORD`
        /// (never a hang, even without a timeout) and the person is asked once.
        #[test]
        fn reauth_second_prompt_is_a_wrong_password() {
            let body = "@echo off\r\necho Enter master password: 1>&2\r\nset /p L=\r\necho Incorrect master password; please try again. 1>&2\r\necho Enter master password: 1>&2\r\nset L=\r\nset /p L=\r\nif \"%L%\"==\"\" exit /b 1\r\n";
            let (dir, file) = script("wrong", body);
            let (i, count) = interactive(Some("bad"));
            let mut r = request(&file, i);
            r.timeout = None;
            assert_eq!(ProcessRunner::new().run(r).unwrap_err(), WRONG_PASSWORD);
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
            // The next request asks afresh.
            let (i, count) = interactive(Some("bad"));
            let mut r = request(&file, i);
            r.timeout = None;
            let _ = ProcessRunner::new().run(r);
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
            let _ = std::fs::remove_dir_all(dir);
        }

        /// I2: the ask carries a flag that is set once the request ends; an ask nobody waits for
        /// is released at once.
        #[test]
        fn reauth_ask_is_released_when_the_request_ends() {
            let (dir, file) = script("stale", "@echo off\r\necho Enter master password: 1>&2\r\nexit /b 1\r\n");
            let (ask, rx) = mpsc::channel::<Ask>();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<std::sync::Arc<std::sync::atomic::AtomicBool>>::new()));
            let s = seen.clone();
            std::thread::spawn(move || {
                for ask in rx {
                    s.lock().unwrap().push(ask.ended.clone());
                    std::thread::sleep(Duration::from_secs(5));
                    drop(ask);
                }
            });
            let start = std::time::Instant::now();
            let out = ProcessRunner::new().run(request(&file, Interactive { ask, needs_login: |_| false }));
            assert!(out.is_ok() || out.is_err());
            assert!(start.elapsed() < Duration::from_secs(4), "{:?}", start.elapsed());
            for flag in seen.lock().unwrap().iter() {
                assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
            }
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
