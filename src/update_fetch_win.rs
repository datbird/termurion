//! Fetching the release feed with curl.exe (M13v2 Task 2): the one process this update path
//! starts, in a job that ends it. The watching, the caps and the worker are `update_run`'s.

use std::path::Path;
use std::process::Child;
use std::sync::Arc;

use crate::script_provision::{KillJob, RealRunner};
use crate::update::{self, Fetch, Fetched};
use crate::update_run::{self, Ender};

pub use crate::update_run::{cancel_running, Checker};

/// A job handle is a kernel object, safe to use from any thread.
struct SharedJob(KillJob);
unsafe impl Send for SharedJob {}
unsafe impl Sync for SharedJob {}

impl Ender for SharedJob {
    fn exited(&self, child: &mut Child) -> std::io::Result<bool> {
        child.try_wait().map(|s| s.is_some())
    }

    fn end(&self) {
        self.0.terminate();
    }
}

/// Runs `curl` on `url`: hidden, from its own folder, never through a shell, stdin empty, in a
/// job that ends it; stdout capped, the time capped, ended on cancel. `proto` is `=https`.
fn fetch(curl: &Path, url: &str, proto: &str) -> Fetched {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    let started = update_run::cancels();
    let args = update::fetch_args(url, update::current_version(), proto);
    let job = match KillJob::new() {
        Ok(j) => Arc::new(SharedJob(j)),
        Err(e) => return Fetched::Transport(e),
    };
    let (out_r, out_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let (err_r, err_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let mut cmd = Command::new(curl);
    cmd.args(&args).env_clear().envs(update_run::inherited_env()).stdin(Stdio::null()).stdout(out_w).stderr(err_w).creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    // System32 as the working folder, through the one path judge (13v1) like every folder.
    if let Some(dir) = curl.parent().and_then(crate::localfs::judge) {
        crate::localfs::set_cwd(&mut cmd, &dir);
    }
    let spawned = cmd.spawn();
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Fetched::Transport(format!("Could not run curl: {e}")),
    };
    if let Err(e) = job.0.adopt(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Fetched::Transport(e);
    }
    update_run::finish(child, out_r, err_r, job, started)
}

/// The production fetch: `curl.exe` from System32, https only.
pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, url: &str) -> Fetched {
        match RealRunner::system_tool("curl.exe") {
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
pub(crate) fn test_curl() -> std::path::PathBuf {
    RealRunner::system_tool("curl.exe").unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_curl_start_and_no_download() {
        let src = include_str!("update_fetch_win.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("Command::new(").count(), 1, "the one curl.exe start");
        for banned in ["\"-o\"", "\"-O\"", "\"--output\"", "std::fs", "ShellExecute", "CreateProcess", ".arg(\"/c\")"] {
            assert!(!code.contains(banned), "the update path downloads and writes nothing: {banned}");
        }
    }
}
