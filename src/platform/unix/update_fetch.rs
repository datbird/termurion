//! Fetching the release feed with the system curl: the one process this update path starts, in a
//! process group of its own that cancel and quit end. The watching, the caps and the worker are
//! `update_run`'s.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::update::{self, Fetch, Fetched};
use crate::update_run::{self, Ender};

pub use crate::update_run::{cancel_running, Checker};

/// Where curl may come from: the system's own folders only, never PATH (a folder the user can
/// write, such as Homebrew's, could hold a curl that trusts anything). macOS always has the first.
const CURL_PATHS: [&str; 2] = ["/usr/bin/curl", "/bin/curl"];

/// The system curl, or what is missing.
fn system_curl() -> Result<PathBuf, String> {
    CURL_PATHS.iter().map(PathBuf::from).find(|p| p.is_file()).ok_or_else(|| format!("curl was not found in {}", CURL_PATHS.join(" or ")))
}

/// What the shared filter keeps that only matters to Unix TLS: OpenSSL's and GnuTLS's own config
/// and where they load code from. Schannel on Windows reads none of these.
const UNIX_DROPPED: [&str; 5] = ["OPENSSL_CONF", "OPENSSL_CONF_INCLUDE", "OPENSSL_MODULES", "OPENSSL_ENGINES", "GNUTLS_SYSTEM_PRIORITY_FILE"];

/// curl's environment on Unix: the shared filter, then the TLS libraries' config.
fn unix_env(vars: Vec<(String, String)>) -> Vec<(String, String)> {
    vars.into_iter().filter(|(k, _)| !UNIX_DROPPED.iter().any(|d| k.eq_ignore_ascii_case(d))).collect()
}

/// curl's process group: its id is curl's pid. `lost` is set once something else reaped curl.
struct Group {
    pid: libc::pid_t,
    lost: AtomicBool,
}

impl Ender for Group {
    /// Asks with WNOWAIT, so curl stays a zombie and its group id is not reused before `end`.
    fn exited(&self, _child: &mut Child) -> std::io::Result<bool> {
        // SAFETY: a zeroed siginfo_t is valid; waitid only writes it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::waitid(libc::P_PID, self.pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            // Reaped by someone else (an inherited SIGCHLD ignore does that): the id may be reused.
            if e.raw_os_error() == Some(libc::ECHILD) {
                self.lost.store(true, Ordering::SeqCst);
            }
            return Err(e);
        }
        // Nothing to report leaves the zeroed signal number.
        Ok(info.si_signo != 0)
    }

    fn end(&self) {
        if self.lost.load(Ordering::SeqCst) {
            return;
        }
        // SAFETY: killpg on the group we made; curl is not reaped yet, so the id is still ours.
        unsafe { libc::killpg(self.pid, libc::SIGKILL) };
    }

    fn lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }
}

/// On Linux curl gets SIGKILL when the thread that started it ends, so a Termurion that is killed or
/// crashes takes curl with it. macOS has no such call; there only an orderly quit ends curl.
fn die_with_us(cmd: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: getpid before the fork; the child runs only prctl and getppid, both async-signal-safe.
        let parent = unsafe { libc::getpid() };
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The parent may have died before the call: then nobody would send the signal.
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// Runs `curl` on `url`: never through a shell, stdin empty, from `/`, in its own process group;
/// stdout capped, the time capped, ended on cancel. `proto` is `=https`.
fn fetch(curl: &Path, url: &str, proto: &str) -> Fetched {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let started = update_run::cancels();
    let args = update::fetch_args(url, update::current_version(), proto);
    let (out_r, out_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let (err_r, err_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let mut cmd = Command::new(curl);
    cmd.args(&args).env_clear().envs(unix_env(update_run::inherited_env())).stdin(Stdio::null()).stdout(out_w).stderr(err_w).process_group(0);
    // A folder that is always there and never a removable mount.
    cmd.current_dir("/");
    die_with_us(&mut cmd);
    // Under the spawn gate, so a descriptor another thread has just made is never inherited.
    let spawned = super::pty::spawn_gated(|| cmd.spawn());
    drop(cmd);
    let child = match spawned {
        Ok(c) => c,
        Err(e) => return Fetched::Transport(format!("Could not run curl: {e}")),
    };
    let Ok(pid) = libc::pid_t::try_from(child.id()) else {
        return Fetched::Transport("curl's process id is out of range".into());
    };
    update_run::finish(child, out_r, err_r, Arc::new(Group { pid, lost: AtomicBool::new(false) }), started)
}

/// The production fetch: the system curl, https only.
pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, url: &str) -> Fetched {
        match system_curl() {
            Ok(curl) => fetch(&curl, url, "=https"),
            Err(e) => Fetched::Transport(e),
        }
    }
}

/// The same for the loopback test: plain http allowed.
#[cfg(test)]
pub(crate) fn fetch_with(curl: &Path, url: &str) -> Fetched {
    fetch(curl, url, "=http")
}

/// The curl the loopback tests run.
#[cfg(test)]
pub(crate) fn test_curl() -> PathBuf {
    system_curl().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    // update_run's loopback tests cancel every running fetch; never overlap them.

    #[test]
    fn one_curl_start_and_no_download() {
        let src = include_str!("update_fetch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("Command::new(").count(), 1, "the one curl start");
        for banned in ["\"-o\"", "\"-O\"", "\"--output\"", "std::fs", "\"sh\"", "\"/bin/sh\"", ".arg(\"-c\")"] {
            assert!(!code.contains(banned), "the update path downloads and writes nothing: {banned}");
        }
    }

    #[test]
    fn curl_comes_from_the_system_folders_only() {
        assert!(CURL_PATHS.iter().all(|p| p.starts_with("/usr/bin/") || p.starts_with("/bin/")));
        assert!(system_curl().unwrap().starts_with("/"), "an absolute path, never a PATH lookup");
    }

    #[test]
    fn a_cancel_ends_what_curl_started() {
        // A stand-in for curl that leaves a child behind: the group kill ends both.
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (out_r, out_w) = std::io::pipe().unwrap();
        let (err_r, err_w) = std::io::pipe().unwrap();
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("/bin/sh").args(["-c", "sleep 30 & echo $!; wait"]).stdin(std::process::Stdio::null()).stdout(out_w).stderr(err_w).process_group(0).spawn().unwrap();
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        let started = update_run::cancels();
        let worker = std::thread::spawn(move || update_run::finish(child, out_r, err_r, Arc::new(Group { pid, lost: AtomicBool::new(false) }), started));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let begun = std::time::Instant::now();
        update_run::cancel_running();
        let got = worker.join().unwrap();
        assert!(begun.elapsed() < std::time::Duration::from_secs(5), "the group was ended, not waited out");
        assert_eq!(got, Fetched::Transport("cancelled".into()));
        // The pipes closed only because the left-behind sleep was ended too; check it is gone.
        // SAFETY: signal 0 only asks whether a process exists.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { libc::killpg(pid, 0) } == 0 && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(unsafe { libc::killpg(pid, 0) } < 0, "nothing of the group is left");
    }
    #[test]
    fn curl_never_reads_the_tls_library_config() {
        let vars = ["PATH", "HOME", "OPENSSL_CONF", "OPENSSL_CONF_INCLUDE", "OPENSSL_MODULES", "OPENSSL_ENGINES", "GNUTLS_SYSTEM_PRIORITY_FILE", "SSL_CERT_FILE", "HTTPS_PROXY"];
        let all: Vec<_> = vars.iter().map(|k| (k.to_string(), "x".to_string())).collect();
        let kept: Vec<String> = unix_env(update_run::curl_env(all)).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kept, ["PATH", "HOME", "HTTPS_PROXY"]);
        let src = include_str!("update_fetch.rs");
        assert!(src.contains(".envs(unix_env(update_run::inherited_env()))"), "the start uses both filters");
    }

    #[test]
    fn a_curl_reaped_elsewhere_is_never_signalled() {
        // Something else reaps the child first: its id is no longer ours.
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (out_r, out_w) = std::io::pipe().unwrap();
        let (err_r, err_w) = std::io::pipe().unwrap();
        let child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).stdin(std::process::Stdio::null()).stdout(out_w).stderr(err_w).spawn().unwrap();
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        let mut status = 0;
        // SAFETY: reaping our own child, as an outside reaper would.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        let group = Arc::new(Group { pid, lost: AtomicBool::new(false) });
        let got = update_run::finish(child, out_r, err_r, group.clone(), update_run::cancels());
        assert_eq!(got, Fetched::Transport("could not wait for curl".into()));
        assert!(group.lost(), "ECHILD marks it lost, so end and the reap skip it");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn curl_dies_with_the_thread_that_started_it() {
        // A child started from a thread that then ends gets SIGKILL, as curl does when Termurion dies.
        let mut child = std::thread::spawn(|| {
            let mut cmd = std::process::Command::new("/bin/sleep");
            cmd.arg("30").stdin(std::process::Stdio::null());
            die_with_us(&mut cmd);
            cmd.spawn().unwrap()
        })
        .join()
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break Some(s);
            }
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.and_then(|s| s.signal()), Some(libc::SIGKILL), "the child outlived its starter");
    }
    use crate::update_run::FETCH_SERIAL as SERIAL;
}
