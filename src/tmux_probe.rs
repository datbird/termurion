//! The tmux control-mode probe: runs a real `tmux -CC` (WSL) through Termurion's ConPTY and
//! records the raw bytes, so the protocol code is tested against what ConPTY really delivers.
//! Run by hand: `cargo test --lib tmux_probe -- --ignored`; it writes tests/fixtures/tmux/.

#[cfg(test)]
mod tests {
    use crate::platform::pty::conpty::{self, Source, dev_vendor_dir};
    use crate::platform::pty::spawn;
    use std::time::Duration;

    fn capture(source: Source, name: &str) {
        let api = conpty::load(&source).unwrap();
        let p = spawn(&api, "wsl.exe -e tmux -L termurion-probe -f /dev/null -CC new-session", 80, 24).unwrap();
        let mut all = Vec::new();
        let step = |p: &crate::platform::pty::Pty, needle: &[u8], all: &mut Vec<u8>| match p.read_until(needle, Duration::from_secs(20)) {
            Ok(b) => all.extend(b),
            Err(b) => {
                all.extend(b);
                panic!("{name}: no {:?} in {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(all));
            }
        };
        step(&p, b"%session-changed", &mut all);
        // Let the rest of the start settle.
        while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
            all.extend(b);
        }
        let long = "x".repeat(300);
        p.write(format!("display-message -p \"{long}\"\r").as_bytes()).unwrap();
        let before = all.len();
        step(&p, long.as_bytes(), &mut all);
        if !all[before..].windows(4).any(|w| w == b"%end") {
            step(&p, b"%end", &mut all);
        }
        p.write(b"kill-server\r").unwrap();
        step(&p, b"%exit", &mut all);
        while let Some(b) = p.read_timeout(Duration::from_millis(1000)) {
            all.extend(b);
        }
        std::fs::create_dir_all("tests/fixtures/tmux").unwrap();
        std::fs::write(format!("tests/fixtures/tmux/{name}.bin"), &all).unwrap();
    }

    /// Two splits and the window's layout: `%layout-change` lines and a list-windows reply.
    fn capture_layout(name: &str) {
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let p = spawn(&api, "wsl.exe -e tmux -L termurion-layout -f /dev/null -CC new-session", 80, 24).unwrap();
        let mut all = Vec::new();
        let step = |p: &crate::platform::pty::Pty, needle: &[u8], all: &mut Vec<u8>| match p.read_until(needle, Duration::from_secs(20)) {
            Ok(b) => all.extend(b),
            Err(b) => {
                all.extend(b);
                panic!("{name}: no {:?} in {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(all));
            }
        };
        step(&p, b"%session-changed", &mut all);
        while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
            all.extend(b);
        }
        for cmd in ["split-window -h", "split-window -v", "list-windows -F \"#{window_layout}\""] {
            p.write(format!("{cmd}\r").as_bytes()).unwrap();
            step(&p, b"%end", &mut all);
            while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
                all.extend(b);
            }
        }
        p.write(b"kill-server\r").unwrap();
        step(&p, b"%exit", &mut all);
        while let Some(b) = p.read_timeout(Duration::from_millis(1000)) {
            all.extend(b);
        }
        std::fs::write(format!("tests/fixtures/tmux/{name}.bin"), &all).unwrap();
    }

    /// The Windows user and machine names as they appear in a prompt, replaced by letters of
    /// the same length, so a recorded fixture holds no personal names.
    fn scrub(bytes: &mut [u8]) {
        for var in ["USERNAME", "COMPUTERNAME"] {
            let Ok(name) = std::env::var(var) else { continue };
            let name = name.into_bytes();
            if name.len() < 3 {
                continue;
            }
            let fill: Vec<u8> = (0..name.len()).map(|i| b"user"[i % 4]).collect();
            let mut i = 0;
            while i + name.len() <= bytes.len() {
                if bytes[i..i + name.len()].eq_ignore_ascii_case(&name) {
                    bytes[i..i + name.len()].copy_from_slice(&fill);
                    i += name.len();
                } else {
                    i += 1;
                }
            }
        }
    }

    /// Ends a psmux probe server by its own `-L` name, never the default one.
    fn stop_psmux(server: &str, session: &str) {
        let _ = std::process::Command::new("psmux").args(["-L", server, "kill-session", "-t", session]).output();
        let _ = std::process::Command::new("psmux").args(["-L", server, "kill-server"]).output();
    }

    /// Records psmux through the shipped ConPTY: the start, a long reply, then the end.
    /// psmux sends no `%exit` after `kill-session`, so the end is a quiet period.
    #[test]
    #[ignore]
    fn record_psmux_transcripts() {
        let server = "termurion-test-rec";
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let p = spawn(&api, &format!("psmux.exe -L {server} -CC new-session -s {server}"), 80, 24).unwrap();
        let mut all = Vec::new();
        let step = |p: &crate::platform::pty::Pty, needle: &[u8], all: &mut Vec<u8>| match p.read_until(needle, Duration::from_secs(20)) {
            Ok(b) => all.extend(b),
            Err(b) => {
                all.extend(b);
                stop_psmux(server, server);
                panic!("psmux: no {:?} in {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(all));
            }
        };
        step(&p, b"%session-changed", &mut all);
        while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
            all.extend(b);
        }
        let long = "x".repeat(300);
        p.write(format!("display-message -p \"{long}\"\r").as_bytes()).unwrap();
        let before = all.len();
        step(&p, long.as_bytes(), &mut all);
        if !all[before..].windows(4).any(|w| w == b"%end") {
            step(&p, b"%end", &mut all);
        }
        for cmd in ["split-window -h", "split-window -v", "list-windows -F \"#{window_layout}\""] {
            p.write(format!("{cmd}\r").as_bytes()).unwrap();
            step(&p, b"%end", &mut all);
            while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
                all.extend(b);
            }
        }
        p.write(format!("kill-session -t {server}\r").as_bytes()).unwrap();
        while let Some(b) = p.read_timeout(Duration::from_secs(3)) {
            all.extend(b);
        }
        stop_psmux(server, server);
        scrub(&mut all);
        std::fs::create_dir_all("tests/fixtures/tmux").unwrap();
        std::fs::write("tests/fixtures/tmux/psmux-3.3.8-start.bin", &all).unwrap();
        println!("PSMUX-RECORD {} bytes, client exit {:?}", all.len(), p.exit_code());
    }

    /// The layout notices psmux sends for two splits and a `list-windows` reply.
    #[test]
    #[ignore]
    fn record_psmux_layout() {
        let server = "termurion-test-lay";
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let p = spawn(&api, &format!("psmux.exe -L {server} -CC new-session -s {server}"), 80, 24).unwrap();
        let mut all = Vec::new();
        let step = |p: &crate::platform::pty::Pty, needle: &[u8], all: &mut Vec<u8>| match p.read_until(needle, Duration::from_secs(20)) {
            Ok(b) => all.extend(b),
            Err(b) => {
                all.extend(b);
                stop_psmux(server, server);
                panic!("psmux layout: no {:?} in {:?}", String::from_utf8_lossy(needle), String::from_utf8_lossy(all));
            }
        };
        step(&p, b"%session-changed", &mut all);
        while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
            all.extend(b);
        }
        for cmd in ["split-window -h", "split-window -v", "list-windows -F \"#{window_layout}\""] {
            p.write(format!("{cmd}\r").as_bytes()).unwrap();
            step(&p, b"%end", &mut all);
            while let Some(b) = p.read_timeout(Duration::from_millis(500)) {
                all.extend(b);
            }
        }
        stop_psmux(server, server);
        while let Some(b) = p.read_timeout(Duration::from_secs(2)) {
            all.extend(b);
        }
        scrub(&mut all);
        std::fs::write("tests/fixtures/tmux/psmux-3.3.8-layout.bin", &all).unwrap();
        println!("PSMUX-RECORD {} bytes", all.len());
    }

    #[test]
    #[ignore]
    fn record_tmux_layout() {
        capture_layout("wsl-3.4-layout");
    }

    #[test]
    #[ignore]
    fn record_tmux_transcripts() {
        // The Windows copy of ConPTY passes nothing of `tmux -CC` (probe of 2026-09-28: only
        // its own mode sequences in 20 s), so tmux integration needs the shipped ConPTY.
        capture(Source::Shipped(dev_vendor_dir()), "wsl-3.4-shipped");
    }
}
