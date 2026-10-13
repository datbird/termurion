//! The origin secret pipe: `\\.\pipe\termurion-ssh-secrets-<SID>[-admin]` hands each client a
//! fresh secret (iTerm2's unix socket, `SS:31-73`), and `fetch` is it2ssh's side (`IS:44-56`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ssh_secret::{valid, Secrets, SECRET_BYTES};
use crate::winpipe::{Listener, Pipe};

/// How long one accept waits before the thread looks at its stop flag.
const ACCEPT_WAIT: Duration = Duration::from_millis(250);
/// The most bytes `fetch` reads; a secret is 32.
const READ_LIMIT: usize = 64;

/// The serving thread; dropping it stops and joins the thread.
pub struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Serves on this user's secret pipe (`winpipe::secret_pipe_name`).
    pub fn start(secrets: Arc<Mutex<Secrets>>) -> Result<Server, String> {
        Server::start_on(&crate::winpipe::secret_pipe_name()?, secrets)
    }

    /// Serves on `name`. Fails when the name is taken (another Termurion of this user serves it).
    pub fn start_on(name: &str, secrets: Arc<Mutex<Secrets>>) -> Result<Server, String> {
        let mut listener = Listener::bind(name)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ssh-secrets".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept(ACCEPT_WAIT) {
                        Ok(Some(pipe)) => serve(&pipe, &secrets),
                        Ok(None) => {}
                        // The waiting instance broke: make a new one and wait a beat, so an
                        // accept that keeps failing never spins the loop.
                        Err(_) => {
                            let _ = listener.renew();
                            std::thread::sleep(ACCEPT_WAIT);
                        }
                    }
                }
            })
            .map_err(|e| format!("could not start the SSH secret thread: {e}"))?;
        Ok(Server { stop, thread: Some(thread) })
    }
}

/// Hands one client a fresh secret and closes (`SS:62-73`). Nothing is written when the
/// system random source fails.
fn serve(pipe: &Pipe, secrets: &Mutex<Secrets>) {
    let mut bytes = [0u8; SECRET_BYTES];
    if getrandom::fill(&mut bytes).is_ok() {
        let secret = secrets.lock().unwrap_or_else(std::sync::PoisonError::into_inner).mint(bytes);
        let _ = pipe.write_all(secret.as_bytes());
    }
    pipe.shutdown();
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A secret from this user's Termurion at this elevation. None when none serves one (iTerm2's
/// "none" off macOS, `IS:44-56`).
pub fn fetch(wait: Duration) -> Option<String> {
    fetch_from(&crate::winpipe::secret_pipe_name().ok()?, wait)
}

/// A secret from the pipe `name`: None when nothing listens, when the server is another user
/// or runs at another elevation, or when what it sent (up to its close, at most 64 bytes)
/// is not a secret. `wait` bounds the connect and, separately, the read.
pub fn fetch_from(name: &str, wait: Duration) -> Option<String> {
    let pipe = Arc::new(crate::winpipe::connect(name, wait).ok()??);
    pipe.server_is_this_user().ok()?;
    // A server that never closes must not hold the caller: the pipe is shut after `wait`.
    let (done, ended) = std::sync::mpsc::channel::<()>();
    let watched = pipe.clone();
    let watch = std::thread::spawn(move || {
        if ended.recv_timeout(wait) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            watched.shutdown();
        }
    });
    let mut buf = [0u8; READ_LIMIT];
    let mut n = 0;
    while n < READ_LIMIT {
        match pipe.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    drop(done);
    let _ = watch.join();
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    valid(text).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_secret::valid;

    #[test]
    fn secret_pipe_round_trip() {
        let pid = std::process::id();
        let s = Arc::new(Mutex::new(Secrets::default()));
        let name = format!(r"\\.\pipe\termurion-ssh-secrets-test-{pid}");
        let server = Server::start_on(&name, s.clone()).unwrap();
        let first = fetch_from(&name, Duration::from_secs(2)).expect("a secret");
        let second = fetch_from(&name, Duration::from_secs(2)).expect("a second secret");
        assert!(valid(&first) && valid(&second));
        assert!(first != second, "each client gets a fresh secret");
        assert!(s.lock().unwrap().check(&first));
        assert!(!s.lock().unwrap().check(&first), "a secret is used once");
        assert!(s.lock().unwrap().check(&second));
        assert!(fetch_from(&format!(r"\\.\pipe\termurion-ssh-secrets-missing-{pid}"), Duration::from_millis(100)).is_none());
        drop(server);
        assert!(fetch_from(&name, Duration::from_millis(100)).is_none(), "nothing serves after the drop");
    }

    /// A server that answers `reply` to one client and holds the pipe open until `release`.
    fn fake_server(name: &str, reply: &'static [u8], release: std::sync::mpsc::Receiver<()>) -> JoinHandle<()> {
        let mut listener = Listener::bind(name).unwrap();
        std::thread::spawn(move || {
            if let Ok(Some(pipe)) = listener.accept(Duration::from_secs(5)) {
                let _ = pipe.write_all(reply);
                let _ = release.recv();
                pipe.shutdown();
            }
        })
    }

    #[test]
    fn fetch_refuses_what_is_not_a_secret() {
        let pid = std::process::id();
        let replies: [&'static [u8]; 3] = [b"0123456789abcdef0123456789abcdef0", b"0123456789ABCDEF0123456789ABCDEF", b""];
        for (i, reply) in replies.into_iter().enumerate() {
            let name = format!(r"\\.\pipe\termurion-ssh-secrets-refuse-{pid}-{i}");
            let (release, held) = std::sync::mpsc::channel();
            let server = fake_server(&name, reply, held);
            drop(release);
            assert!(fetch_from(&name, Duration::from_secs(2)).is_none(), "reply {i}");
            server.join().unwrap();
        }
    }

    #[test]
    fn fetch_gives_up_on_a_server_that_never_closes() {
        let name = format!(r"\\.\pipe\termurion-ssh-secrets-silent-{}", std::process::id());
        let (release, held) = std::sync::mpsc::channel();
        let server = fake_server(&name, b"", held);
        let start = std::time::Instant::now();
        assert!(fetch_from(&name, Duration::from_millis(300)).is_none());
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(250) && took < Duration::from_secs(3), "{took:?}");
        release.send(()).unwrap();
        server.join().unwrap();
    }
}
