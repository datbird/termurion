//! The SSH transport probe (milestone 14a Task 1): ssh.exe against an unprivileged sshd the probe
//! starts inside WSL on 127.0.0.1:22990, through each transport of ruling 1, recording the raw
//! bytes. Nothing is installed (ruling "14a probe"): a missing piece is BLOCKED. Run by hand:
//! `cargo test --lib ssh_probe_win -- --ignored --nocapture`; it writes tests/fixtures/ssh/.

#[cfg(test)]
mod tests {
    use crate::conpty::{self, Api, Source, dev_vendor_dir};
    use crate::pty::{Pty, spawn_flags};
    use crate::ssh_probe::*;
    use crate::ssh_transport::{ORDER, Transport, conpty_flags, label};
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, Command, Output, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SIZE_HEAD: &[u8] = b"\x1b]134;:size ";
    const GOT_HEAD: &[u8] = b"\x1b]134;:got ";

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn quiet(program: &str) -> Command {
        let mut c = Command::new(program);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    }

    /// `wsl.exe <args>` with no input.
    fn wsl(args: &[&str]) -> Option<Output> {
        quiet("wsl.exe").args(args).stdin(Stdio::null()).output().ok()
    }

    /// `wsl.exe -e <args>` with `input` on stdin (scripts never go on the command line).
    fn wsl_fed(args: &[&str], input: &[u8]) -> Option<Output> {
        let mut child = quiet("wsl.exe").arg("-e").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let _ = stdin.write_all(input);
        drop(stdin);
        child.wait_with_output().ok()
    }

    fn ok(out: &Option<Output>) -> bool {
        out.as_ref().is_some_and(|o| o.status.success())
    }

    fn stdout(out: &Option<Output>) -> String {
        out.as_ref().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    }

    /// Stops the probe's sshd and removes both folders on every way out, a failed assertion
    /// included; the WSL keepalive (so the distro does not stop under sshd) ends last.
    struct Cleanup {
        temp: PathBuf,
        keepalive: Option<Child>,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = wsl(&["-e", "sh", "-c", SSHD_STOP]);
            let _ = std::fs::remove_dir_all(&self.temp);
            if let Some(mut k) = self.keepalive.take() {
                let _ = k.kill();
                let _ = k.wait();
            }
        }
    }

    fn blocked(reason: &str, cleanup: Cleanup) -> ! {
        println!("BLOCKED {reason}");
        drop(cleanup);
        panic!("BLOCKED {reason}");
    }

    /// One ssh.exe session: in a pseudoconsole, or on plain pipes (stderr joined to stdout).
    enum Conn {
        Pty(Pty),
        Pipes { child: Child, stdin: Option<ChildStdin>, output: Receiver<Vec<u8>> },
    }

    impl Conn {
        fn start(t: Transport, api: &Api, args: &[String]) -> Result<Conn, String> {
            match conpty_flags(t) {
                Some(flags) => spawn_flags(api, &cmdline("ssh.exe", args), flags, 120, 40).map(Conn::Pty),
                None => {
                    let (mut r, w) = std::io::pipe().map_err(|e| e.to_string())?;
                    let w2 = w.try_clone().map_err(|e| e.to_string())?;
                    let mut cmd = Command::new("ssh.exe");
                    cmd.args(args).stdin(Stdio::piped()).stdout(w).stderr(w2).creation_flags(DETACHED_PROCESS);
                    let mut child = cmd.spawn().map_err(|e| format!("spawn ssh.exe: {e}"))?;
                    // The command holds the pipe's write ends; without it only ssh.exe does.
                    drop(cmd);
                    let stdin = child.stdin.take();
                    let (tx, output) = mpsc::channel();
                    std::thread::spawn(move || {
                        let mut buf = vec![0u8; 64 * 1024];
                        while let Ok(n) = r.read(&mut buf) {
                            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    });
                    Ok(Conn::Pipes { child, stdin, output })
                }
            }
        }

        fn read(&self, wait: Duration) -> Option<Vec<u8>> {
            match self {
                Conn::Pty(p) => p.read_timeout(wait),
                Conn::Pipes { output, .. } => match output.recv_timeout(wait) {
                    Ok(b) => Some(b),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(wait);
                        None
                    }
                },
            }
        }

        fn write(&mut self, bytes: &[u8]) {
            match self {
                Conn::Pty(p) => {
                    let _ = p.write(bytes);
                }
                Conn::Pipes { stdin, .. } => {
                    if let Some(s) = stdin {
                        let _ = s.write_all(bytes).and_then(|_| s.flush());
                    }
                }
            }
        }

        fn wait_exit(&mut self, wait: Duration) -> Option<u32> {
            match self {
                Conn::Pty(p) => p.wait_exit(wait),
                Conn::Pipes { child, .. } => {
                    let end = Instant::now() + wait;
                    loop {
                        if let Ok(Some(s)) = child.try_wait() {
                            return s.code().map(|c| c as u32);
                        }
                        if Instant::now() >= end {
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
    }

    impl Drop for Conn {
        fn drop(&mut self) {
            if let Conn::Pipes { child, stdin, .. } = self {
                stdin.take();
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    /// Reads into `all` until `needle` has appeared `n` times, at most `wait`.
    fn read_until_count(conn: &Conn, all: &mut Vec<u8>, needle: &[u8], n: usize, wait: Duration) -> bool {
        let end = Instant::now() + wait;
        while count(all, needle) < n {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if let Some(b) = conn.read(left.min(Duration::from_millis(50))) {
                all.extend(b);
            }
        }
        true
    }

    fn drain(conn: &Conn, all: &mut Vec<u8>) {
        while let Some(b) = conn.read(Duration::from_millis(500)) {
            all.extend(b);
        }
    }

    struct Measured {
        raw: Vec<u8>,
        argv: Vec<u8>,
        create: String,
        exit: Option<u32>,
        notes: Vec<String>,
    }

    fn measure(t: Transport, api: &Api, key: &str, user: &str) -> Measured {
        let remote = vec!["sh".to_string(), format!("{WSL_DIR}/remote.sh")];
        let mut m = Measured { raw: Vec::new(), argv: Vec::new(), create: "ok".into(), exit: None, notes: Vec::new() };
        let mut conn = match Conn::start(t, api, &ssh_args(t, key, user, &remote)) {
            Ok(c) => c,
            Err(e) => {
                m.create = e;
                return m;
            }
        };
        let all = &mut m.raw;
        if !read_until_count(&conn, all, SIZE_HEAD, 1, secs(20)) {
            m.notes.push("no first size report within 20 s".into());
        }
        if let Conn::Pty(p) = &conn
            && let Err(e) = p.resize(100, 30)
        {
            m.notes.push(format!("resize: {e}"));
        }
        conn.write(b"size\n");
        if !read_until_count(&conn, all, SIZE_HEAD, 2, secs(20)) {
            m.notes.push("no second size report within 20 s".into());
        }
        conn.write(BOOTSTRAP);
        if !read_until_count(&conn, all, GOT_HEAD, 1, secs(20)) {
            m.notes.push("no got report within 20 s".into());
        }
        m.exit = conn.wait_exit(secs(10));
        drain(&conn, all);
        drop(conn);

        match Conn::start(t, api, &ssh_args(t, key, user, &argv_args())) {
            Ok(conn) => {
                if !read_until_count(&conn, &mut m.argv, ARGV_SEEN, 1, secs(20)) {
                    m.notes.push("argv: no marker within 20 s".into());
                }
                drain(&conn, &mut m.argv);
            }
            Err(e) => m.notes.push(format!("argv create: {e}")),
        }
        m
    }

    #[test]
    #[ignore]
    fn ssh_transport_probe() {
        let temp = std::env::temp_dir().join("tandem-ssh-probe");
        let mut cleanup = Cleanup { temp: temp.clone(), keepalive: None };

        // (0) Preflight: nothing is installed; any miss is BLOCKED.
        let wsl_ok = ok(&wsl(&["-e", "sh", "-c", "true"]));
        let found = if wsl_ok { String::from_utf8_lossy(&wsl_fed(&["sh"], PRECHECK.as_bytes()).map(|o| o.stdout).unwrap_or_default()).into_owned() } else { String::new() };
        let sshd = found.lines().any(|l| l.trim() == "/usr/sbin/sshd");
        let keygen = found.lines().any(|l| l.trim().ends_with("/ssh-keygen"));
        let ssh_exe = ok(&quiet("where").arg("ssh.exe").stdin(Stdio::null()).output().ok());
        if let Some(reason) = blocked_reason(wsl_ok, sshd, keygen, ssh_exe, true, true) {
            blocked(reason, cleanup);
        }

        // (1) The WSL user, the only one an unprivileged sshd can log in.
        let user = stdout(&wsl(&["-e", "sh", "-c", "id -un"])).trim().to_string();
        cleanup.keepalive = quiet("wsl.exe").args(["-e", "sleep", "900"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();

        // (2) The sshd, then the setup check.
        let setup = wsl_fed(&["sh"], sshd_setup(&user).as_bytes());
        let pid_file = format!("{WSL_DIR}/sshd.pid");
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], SSHD_PORT));
        let end = Instant::now() + secs(5);
        let (mut pid, mut port) = (false, false);
        while !(pid && port) && Instant::now() < end {
            pid = pid || ok(&wsl(&["-e", "test", "-f", &pid_file]));
            port = port || std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok();
            if !(pid && port) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        if let Some(reason) = blocked_reason(true, true, true, true, pid, port) {
            let err = setup.as_ref().map(|o| String::from_utf8_lossy(&o.stderr).into_owned()).unwrap_or_default();
            println!("setup stderr: {}", err.trim());
            println!("sshd.log tail: {}", stdout(&wsl(&["-e", "tail", "-n", "20", &format!("{WSL_DIR}/sshd.log")])).trim());
            blocked(reason, cleanup);
        }

        // (3) The client key, in a folder only this user (and SYSTEM, Administrators) can read.
        std::fs::create_dir_all(&temp).unwrap();
        let key = temp.join("client_key");
        let bytes = wsl(&["-e", "cat", &format!("{WSL_DIR}/client_key")]).map(|o| o.stdout).unwrap_or_default();
        assert!(!bytes.is_empty(), "the client key could not be read from WSL");
        std::fs::write(&key, bytes).unwrap();
        let key = key.to_string_lossy().into_owned();
        // %TEMP% can carry more than the user's ACL (EXTREME's grants CodexSandboxUsers), which
        // OpenSSH for Windows refuses ("UNPROTECTED PRIVATE KEY FILE"): only this user keeps access.
        // USERDOMAIN reads WORKGROUP in an ssh session there, so the bare name is used.
        let me = format!("{}:F", std::env::var("USERNAME").unwrap_or_default());
        let acl = quiet("icacls.exe").args([key.as_str(), "/inheritance:r", "/grant:r", me.as_str()]).stdin(Stdio::null()).output().ok();
        assert!(ok(&acl), "icacls could not restrict the client key: {} {}", stdout(&acl).trim(), acl.as_ref().map(|o| String::from_utf8_lossy(&o.stderr).into_owned()).unwrap_or_default().trim());

        // (4) The fake remote.
        let wrote = wsl_fed(&["sh", "-c", &format!("cat > {WSL_DIR}/remote.sh")], remote_script().as_bytes());
        assert!(ok(&wrote), "remote.sh was not written");

        // (5) to (8) Each transport, its fixtures and its line.
        let shipped = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        std::fs::create_dir_all("tests/fixtures/ssh").unwrap();
        let mut results = Vec::new();
        for t in ORDER {
            let m = measure(t, &shipped, &key, &user);
            std::fs::write(format!("tests/fixtures/ssh/{}.bin", label(t)), &m.raw).unwrap();
            std::fs::write(format!("tests/fixtures/ssh/argv-{}.bin", label(t)), &m.argv).unwrap();
            let v = check(&m.raw);
            for n in &m.notes {
                println!("NOTE {}: {n}", label(t));
            }
            println!("MEASURED {}: {v:?} argv {} create {} exit {:?}", label(t), argv_ok(&m.argv), m.create, m.exit);
            results.push((t, v));
        }
        println!("CHOSEN {}", choose(&results).map_or("none", label));
        // (9) Cleanup's Drop stops sshd and removes both folders.
        drop(cleanup);
    }
}
