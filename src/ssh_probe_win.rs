//! The SSH transport probe (milestone 14a Task 1): ssh.exe against an unprivileged sshd the probe
//! starts inside WSL on 127.0.0.1:22990, through each transport of ruling 1, recording the raw
//! bytes. Nothing is installed (ruling "14a probe"): a missing piece is BLOCKED. Run by hand:
//! `cargo test --lib ssh_probe_win::tests::ssh_transport_probe -- --ignored --nocapture`; it
//! writes tests/fixtures/ssh/. `pipes_investigation` is the measured matrix of pipe kinds and
//! console flags behind the named-pipes transport.

#[cfg(test)]
mod tests {
    use crate::conpty::{self, Api, Source, dev_vendor_dir};
    use crate::pty::{Pty, spawn_flags};
    use crate::ssh_probe::*;
    use crate::ssh_transport::{ORDER, Transport, conpty_flags, label};
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Output, Stdio};
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

    /// One ssh.exe session: in a pseudoconsole, or on pipes (stderr joined to stdout).
    enum Conn {
        Pty(Pty),
        Pipes(PipeRun),
    }

    impl Conn {
        fn start(t: Transport, api: &Api, args: &[String]) -> Result<Conn, String> {
            match (conpty_flags(t), t) {
                (Some(flags), _) => spawn_flags(api, &cmdline("ssh.exe", args), flags, 120, 40).map(Conn::Pty),
                // Anonymous pipes as std makes them, no console (the brief's pipes).
                (None, Transport::Pipes) => pipes_start(args, DETACHED_PROCESS, StdinKind::Anon, StdinKind::Anon).map(Conn::Pipes),
                // Overlapped named pipes both ways and a console with no window (the pipes investigation).
                (None, _) => pipes_start(args, CREATE_NO_WINDOW, StdinKind::NamedOverlapped, StdinKind::NamedOverlapped).map(Conn::Pipes),
            }
        }

        fn read(&self, wait: Duration) -> Option<Vec<u8>> {
            match self {
                Conn::Pty(p) => p.read_timeout(wait),
                Conn::Pipes(run) => match run.output.recv_timeout(wait) {
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
                Conn::Pipes(run) => {
                    let _ = run.stdin.write_all(bytes).and_then(|_| run.stdin.flush());
                }
            }
        }

        fn wait_exit(&mut self, wait: Duration) -> Option<u32> {
            match self {
                Conn::Pty(p) => p.wait_exit(wait),
                Conn::Pipes(run) => {
                    let end = Instant::now() + wait;
                    loop {
                        if let Ok(Some(s)) = run.child.try_wait() {
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

    impl Drop for PipeRun {
        fn drop(&mut self) {
            self.stdin = Box::new(std::io::sink());
            let _ = self.child.kill();
            let _ = self.child.wait();
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

    /// Steps (0) to (4): preflight, the user, the sshd and its check, the client key, remote.sh.
    /// Returns the cleanup guard, the key's path and the user.
    fn setup() -> (Cleanup, String, String) {
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
        (cleanup, key, user)
    }

    /// How ssh.exe's stdin is made for a pipes experiment.
    #[derive(Clone, Copy, Debug)]
    enum StdinKind {
        Anon,
        NamedSync,
        NamedOverlapped,
    }

    /// A named pipe: our write end and ssh's read end (overlapped or not).
    fn named_pipe(overlapped: bool, inbound: bool) -> (std::fs::File, std::os::windows::io::OwnedHandle) {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND};
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};
        let name: Vec<u16> = format!("\\\\.\\pipe\\tandem-ssh-probe-{}-{overlapped}-{inbound}", std::process::id()).encode_utf16().chain(Some(0)).collect();
        unsafe {
            let server = CreateNamedPipeW(name.as_ptr(), if inbound { PIPE_ACCESS_INBOUND } else { PIPE_ACCESS_OUTBOUND }, PIPE_TYPE_BYTE | PIPE_WAIT, 1, 65536, 65536, 0, std::ptr::null());
            assert_ne!(server, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
            let client = CreateFileW(name.as_ptr(), if inbound { GENERIC_WRITE } else { GENERIC_READ }, 0, std::ptr::null(), OPEN_EXISTING, if overlapped { FILE_FLAG_OVERLAPPED } else { 0 }, std::ptr::null_mut());
            assert_ne!(client, INVALID_HANDLE_VALUE, "CreateFileW (pipe client)");
            (std::fs::File::from_raw_handle(server), std::os::windows::io::OwnedHandle::from_raw_handle(client))
        }
    }

    struct PipeRun {
        child: Child,
        stdin: Box<dyn Write + Send>,
        output: Receiver<Vec<u8>>,
    }

    fn pipes_start(args: &[String], flags: u32, kind: StdinKind, out: StdinKind) -> Result<PipeRun, String> {
        let mut cmd = Command::new("ssh.exe");
        cmd.args(args).creation_flags(flags);
        let mut r: Box<dyn Read + Send> = match out {
            StdinKind::Anon => {
                let (r, w) = std::io::pipe().unwrap();
                let w2 = w.try_clone().unwrap();
                cmd.stdout(w).stderr(w2);
                Box::new(r)
            }
            StdinKind::NamedSync | StdinKind::NamedOverlapped => {
                let (server, client) = named_pipe(matches!(out, StdinKind::NamedOverlapped), true);
                let client2 = client.try_clone().unwrap();
                cmd.stdout(Stdio::from(client)).stderr(Stdio::from(client2));
                Box::new(server)
            }
        };
        let mut ours: Option<Box<dyn Write + Send>> = None;
        match kind {
            StdinKind::Anon => {
                cmd.stdin(Stdio::piped());
            }
            StdinKind::NamedSync | StdinKind::NamedOverlapped => {
                let (server, client) = named_pipe(matches!(kind, StdinKind::NamedOverlapped), false);
                cmd.stdin(Stdio::from(client));
                ours = Some(Box::new(server));
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn ssh.exe: {e}"))?;
        // The command holds our copies of ssh's pipe ends; without it only ssh.exe does.
        drop(cmd);
        let stdin: Box<dyn Write + Send> = match ours {
            Some(o) => o,
            None => Box::new(child.stdin.take().unwrap()),
        };
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(PipeRun { child, stdin, output })
    }

    fn pipe_read_until(run: &PipeRun, all: &mut Vec<u8>, needle: &[u8], wait: Duration) -> bool {
        let end = Instant::now() + wait;
        while count(all, needle) == 0 {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if let Ok(b) = run.output.recv_timeout(left.min(Duration::from_millis(50))) {
                all.extend(b);
            }
        }
        true
    }

    fn pipe_finish(mut run: PipeRun, all: &mut Vec<u8>, wait: Duration) -> Option<i32> {
        let end = Instant::now() + wait;
        let code = loop {
            while let Ok(b) = run.output.try_recv() {
                all.extend(b);
            }
            if let Ok(Some(s)) = run.child.try_wait() {
                break s.code();
            }
            if Instant::now() >= end {
                let _ = run.child.kill();
                let _ = run.child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        std::thread::sleep(Duration::from_millis(300));
        while let Ok(b) = run.output.try_recv() {
            all.extend(b);
        }
        code
    }

    fn show(bytes: &[u8]) -> String {
        let s = format!("{:?}", String::from_utf8_lossy(bytes));
        if s.len() > 700 { format!("{}...{}", &s[..300], &s[s.len() - 300..]) } else { s }
    }

    /// The ssh -vvv log lines worth reading (the log never holds a key).
    fn log_lines(path: &std::path::Path) -> String {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().filter(|l| !l.contains("Failed to open file") && !l.contains("failed to open file")).collect();
        let from = lines.iter().position(|l| l.contains("new session")).unwrap_or(0);
        lines[from..].iter().take(80).cloned().collect::<Vec<_>>().join("\n    ")
    }

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    /// Pipes investigation (after Robert's ruling to investigate): each variant of ssh.exe on
    /// pipes, with the remote logging exactly what reaches it. Run by hand:
    /// `cargo test --lib ssh_probe_win::tests::pipes_investigation -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn pipes_investigation() {
        let (cleanup, key, user) = setup();
        let log = cleanup.temp.join("ssh.log");
        let in_file = format!("{WSL_DIR}/in.bin");
        // The remote logs what it reads for 6 s, then reports its hex.
        let logger = format!("stty -echo -isig -icanon min 1 2>/dev/null; stty -a 2>&1 | head -1; printf 'READY\\n'; timeout --foreground 6 cat > {in_file}; printf 'SIZE %s GOT %s END\\n' \"$(wc -c < {in_file})\" \"$(head -c 64 {in_file} | od -An -tx1 | tr -d ' \\n')\"; exit 0");
        let ticker = "i=0; while [ $i -lt 4 ]; do printf 'tick%s\\n' $i; i=$((i+1)); sleep 1; done; exit 0".to_string();
        let base = |tflags: &[&str], extra_v: bool| {
            let mut a: Vec<String> = ["-F", "NUL", "-i", key.as_str(), "-p", "22990", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=NUL", "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"].map(String::from).to_vec();
            if extra_v {
                a.extend(["-vvv".to_string(), "-E".to_string(), log.to_string_lossy().into_owned()]);
            } else {
                a.extend(["-o".to_string(), "LogLevel=ERROR".to_string()]);
            }
            a.extend(tflags.iter().map(|s| s.to_string()));
            a.push(format!("{user}@127.0.0.1"));
            a
        };
        use StdinKind::{Anon, NamedOverlapped as Ovl, NamedSync as Sync};
        // Every variant measured (report "Pipes investigation"); stdin kind, then stdout kind.
        type Variant<'a> = (&'a str, Vec<&'a str>, u32, StdinKind, StdinKind, &'a str);
        let variants: Vec<Variant> = vec![
            ("ticker no-window -tt anon/anon", vec!["-t", "-t"], CREATE_NO_WINDOW, Anon, Anon, "ticker"),
            ("input no-window -tt anon/anon", vec!["-t", "-t"], CREATE_NO_WINDOW, Anon, Anon, "logger"),
            ("input detached -tt anon/anon", vec!["-t", "-t"], DETACHED_PROCESS, Anon, Anon, "logger"),
            ("input detached -tt anon/ovl", vec!["-t", "-t"], DETACHED_PROCESS, Anon, Ovl, "logger"),
            ("input no-window -tt sync-named/ovl", vec!["-t", "-t"], CREATE_NO_WINDOW, Sync, Ovl, "logger"),
            ("input detached -tt ovl/sync-named", vec!["-t", "-t"], DETACHED_PROCESS, Ovl, Sync, "logger"),
            ("input detached -tt ovl/anon", vec!["-t", "-t"], DETACHED_PROCESS, Ovl, Anon, "logger"),
            ("input no-window -tt ovl/ovl", vec!["-t", "-t"], CREATE_NO_WINDOW, Ovl, Ovl, "logger"),
            ("input detached -tt ovl/ovl", vec!["-t", "-t"], DETACHED_PROCESS, Ovl, Ovl, "logger"),
            ("input new-console -tt ovl/ovl", vec!["-t", "-t"], CREATE_NEW_CONSOLE, Ovl, Ovl, "logger"),
            ("input detached -t ovl/ovl", vec!["-t"], DETACHED_PROCESS, Ovl, Ovl, "logger"),
            ("input no-window -T ovl/ovl", vec!["-T"], CREATE_NO_WINDOW, Ovl, Ovl, "logger"),
            ("input no-window -tt ovl/ovl 8 KiB", vec!["-t", "-t"], CREATE_NO_WINDOW, Ovl, Ovl, "logger-big"),
            ("input detached -tt ovl/ovl no -E log", vec!["-t", "-t"], DETACHED_PROCESS, Ovl, Ovl, "logger-nolog"),
            ("input no-window -tt ovl/ovl no -E log", vec!["-t", "-t"], CREATE_NO_WINDOW, Ovl, Ovl, "logger-nolog"),
        ];
        for (name, tflags, flags, kind, out, what) in variants {
            let _ = wsl(&["-e", "rm", "-f", &in_file]);
            let _ = std::fs::remove_file(&log);
            let mut args = base(&tflags, !what.ends_with("nolog"));
            args.push(if what == "ticker" { ticker.clone() } else { logger.clone() });
            let started = Instant::now();
            let mut run = pipes_start(&args, flags, kind, out).unwrap();
            let mut all = Vec::new();
            let mut stamps = Vec::new();
            if what.starts_with("logger") {
                let ready = pipe_read_until(&run, &mut all, b"READY", secs(15));
                stamps.push(format!("ready {ready} at {:?}", started.elapsed()));
                let data: Vec<u8> = if what == "logger-big" { b"0123456789abcdef".repeat(512) } else { b"hello\x03\x1b\xc3\x28\xff\n".to_vec() };
                let w1 = run.stdin.write_all(&data).and_then(|_| run.stdin.flush());
                stamps.push(format!("write {} bytes {w1:?}", data.len()));
                if what == "logger-close" {
                    run.stdin = Box::new(std::io::sink());
                }
                pipe_read_until(&run, &mut all, b"END", secs(15));
            } else {
                // Ticks arrive once a second: note when each one lands.
                let end = Instant::now() + secs(8);
                while Instant::now() < end {
                    if let Ok(b) = run.output.recv_timeout(Duration::from_millis(50)) {
                        stamps.push(format!("{:?}: {}", started.elapsed(), show(&b)));
                        all.extend(b);
                    }
                    if count(&all, b"tick3") > 0 {
                        break;
                    }
                }
            }
            let code = pipe_finish(run, &mut all, secs(5));
            println!("VARIANT {name}: exit {code:?}");
            for st in &stamps {
                println!("  {st}");
            }
            println!("  output {}", show(&all));
            println!("  ssh log:\n    {}", log_lines(&log));
        }
        drop(cleanup);
    }

    #[test]
    #[ignore]
    fn ssh_transport_probe() {
        let (cleanup, key, user) = setup();

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
