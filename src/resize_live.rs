//! Live resize guard: a real shell behind the shipped ConPTY, fed through the same `Screen` a
//! pane uses and resized the way a pane resizes (the screen, then the pty). Once the shell is
//! idle, the rows of the grid must be the rows of ConPTY's own screen (read back from its
//! console), and after the shell redraws its prompt the cursors must agree too.
//!
//! Every test is ignored:
//! `scripts/xt test --lib resize_live -- --ignored --nocapture --test-threads=1`.

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;

    use crate::conpty::{self, Source, dev_vendor_dir};
    use crate::pty::{Pty, Read, spawn_notify};
    use crate::screen::Screen;

    const POWERSHELL: &str = "powershell.exe -NoLogo -NoProfile";

    struct Rig {
        pty: Pty,
        screen: Screen,
        failures: Vec<String>,
        /// Every byte the shell wrote, with a marker where each resize happened.
        trace: Vec<u8>,
    }

    impl Rig {
        fn new(cmd: &str, cols: u16, rows: u16) -> Rig {
            let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
            let pty = spawn_notify(&api, cmd, None, &[], cols, rows, std::sync::Arc::new(|| {})).unwrap();
            let mut screen = Screen::new(cols as usize, rows as usize);
            screen.pty_backed = true;
            let mut r = Rig { pty, screen, failures: Vec::new(), trace: Vec::new() };
            r.settle();
            r
        }

        fn powershell() -> Rig {
            Rig::new(POWERSHELL, 100, 30)
        }

        /// Moves everything the shell wrote into the screen, answering its queries.
        fn pump(&mut self) -> bool {
            let mut any = false;
            while let Read::Data(d) = self.pty.try_read() {
                any = true;
                self.trace.extend_from_slice(&d);
                let reply = self.screen.feed(&d);
                if !reply.is_empty() {
                    let _ = self.pty.write(&reply);
                }
            }
            any
        }

        /// Pumps until the shell has been quiet for 900 ms (at most 30 s).
        fn settle(&mut self) {
            let end = Instant::now() + Duration::from_secs(30);
            let mut last = Instant::now();
            while Instant::now() < end {
                if self.pump() {
                    last = Instant::now();
                } else if last.elapsed() >= Duration::from_millis(900) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// What a pane does: the screen first, then the pty.
        fn resize(&mut self, cols: u16, rows: u16) {
            self.trace.extend_from_slice(format!("<<RESIZE {cols}x{rows}>>").as_bytes());
            self.screen.resize(cols as usize, rows as usize);
            self.pty.resize(cols, rows).unwrap();
        }

        fn type_line(&mut self, s: &str) {
            self.pty.write(s.as_bytes()).unwrap();
            self.pty.write(b"\r").unwrap();
        }

        fn cursor(&self) -> (i32, i32) {
            let p = self.screen.term().grid().cursor.point;
            (p.column.0 as i32, p.line.0)
        }

        /// The grid's rows, one string each (a wrapped line is as many rows as it takes).
        fn grid_rows(&self) -> Vec<String> {
            let grid = self.screen.term().grid();
            (0..grid.screen_lines() as i32)
                .map(|l| {
                    let row = &grid[Line(l)];
                    let text: String = (0..grid.columns()).filter(|c| !row[Column(*c)].flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)).map(|c| row[Column(c)].c).collect();
                    text.trim_end().to_string()
                })
                .collect()
        }

        /// ConPTY's own screen, read from the shell's console: its rows and its cursor as
        /// (column, row in the window).
        fn console(&self) -> Result<(Vec<String>, (i32, i32)), String> {
            use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
            use windows_sys::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, COORD, FreeConsole, GetConsoleScreenBufferInfo, ReadConsoleOutputCharacterW};
            // The test process has a console of its own; attaching to the shell's needs none.
            unsafe { FreeConsole() };
            crate::pty::attached_unless(self.pty.pid(), &std::sync::atomic::AtomicBool::new(false), || unsafe {
                let name: Vec<u16> = "CONOUT$".encode_utf16().chain(Some(0)).collect();
                let h = CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut());
                if h == INVALID_HANDLE_VALUE {
                    return Err("CONOUT$ could not be opened".to_string());
                }
                let mut i: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
                if GetConsoleScreenBufferInfo(h, &mut i) == 0 {
                    CloseHandle(h);
                    return Err("GetConsoleScreenBufferInfo failed".to_string());
                }
                let mut rows = Vec::new();
                for y in i.srWindow.Top..=i.srWindow.Bottom {
                    let mut buf = vec![0u16; i.dwSize.X as usize];
                    let mut n = 0u32;
                    ReadConsoleOutputCharacterW(h, buf.as_mut_ptr(), buf.len() as u32, COORD { X: 0, Y: y }, &mut n);
                    rows.push(String::from_utf16_lossy(&buf[..n as usize]).trim_end().to_string());
                }
                CloseHandle(h);
                Ok((rows, (i.dwCursorPosition.X as i32, (i.dwCursorPosition.Y - i.srWindow.Top) as i32)))
            })
        }

        fn escaped_trace(&self) -> String {
            let from = self.trace.windows(9).rposition(|w| w == b"<<RESIZE ").unwrap_or(0);
            self.trace[from.saturating_sub(200)..]
                .iter()
                .take(1500)
                .map(|&b| match b {
                    0x1b => "<ESC>".to_string(),
                    b'\r' => "<CR>".into(),
                    b'\n' => "<LF>".into(),
                    32..=126 => (b as char).to_string(),
                    _ => format!("<{b:02x}>"),
                })
                .collect()
        }

        /// Once idle: the grid shows what ConPTY's screen shows. With `cursor`, the cursors
        /// agree and sit on the prompt line too (the shell has redrawn since the last resize).
        fn check(&mut self, what: &str, cursor: bool) {
            self.settle();
            let (rows, at) = match self.console() {
                Ok(v) => v,
                Err(e) => {
                    self.failures.push(format!("{what}: {e}"));
                    return;
                }
            };
            let grid = self.grid_rows();
            let mut problems = Vec::new();
            if grid.len() != rows.len() {
                problems.push(format!("the grid has {} rows, ConPTY {}", grid.len(), rows.len()));
            }
            let differing: Vec<usize> = (0..grid.len().min(rows.len())).filter(|i| grid[*i] != rows[*i]).collect();
            if !differing.is_empty() {
                problems.push(format!("rows differ: {differing:?}"));
            }
            if cursor {
                let here = self.cursor();
                if here != at {
                    problems.push(format!("the grid's cursor is at {here:?}, ConPTY's at {at:?}"));
                }
                let line = grid.get(at.1.max(0) as usize).cloned().unwrap_or_default();
                if !(line.starts_with("PS ") && line.ends_with('>')) {
                    problems.push(format!("ConPTY's cursor row is not a prompt: {line:?}"));
                }
            }
            println!("LIVE {what}: {} (grid cursor {:?}, ConPTY {at:?})", if problems.is_empty() { "ok" } else { "MISMATCH" }, self.cursor());
            if !problems.is_empty() {
                for i in &differing {
                    println!("LIVE   row {i}: grid {:?} / ConPTY {:?}", grid[*i], rows[*i]);
                }
                println!("LIVE   bytes since the last resize: {}", self.escaped_trace());
                self.failures.push(format!("{what}: {}", problems.join("; ")));
            }
        }

        fn finish(self) {
            assert!(self.failures.is_empty(), "{:#?}", self.failures);
        }
    }

    const DIR: &str = "dir C:\\Windows\\System32 | select -first 300";

    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn steps_after_output() {
        let mut r = Rig::powershell();
        r.check("start", true);
        r.type_line("1..200");
        r.check("after 200 lines", true);
        for (c, rw) in [(100, 54), (100, 20), (140, 40), (80, 24), (100, 54), (100, 30), (60, 10), (120, 50)] {
            r.resize(c, rw);
            r.check(&format!("resize {c}x{rw}"), false);
        }
        r.type_line("1..200");
        r.check("lines again", true);
        r.finish();
    }

    /// The report: the window grew, and ConPTY kept the prompt on its row while the grid pulled
    /// scrollback down and put it on the bottom row. A later redraw then landed mid-output.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn quiet_row_growth_keeps_the_prompt_row() {
        for (c, rw) in [(100, 54), (82, 36), (100, 31), (130, 45)] {
            let mut r = Rig::powershell();
            r.type_line("1..100 | % { \"row $_\" }");
            r.check("setup", true);
            let before = r.cursor();
            r.resize(c, rw);
            r.check(&format!("grow to {c}x{rw}"), true);
            assert_eq!(r.cursor(), (before.0, before.1), "the cursor row does not move when rows are added below it");
            r.finish();
        }
    }

    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn rapid_resizes() {
        let mut r = Rig::powershell();
        r.type_line("1..200");
        r.check("after 200 lines", true);
        let sizes = [(100, 54), (101, 40), (99, 12), (130, 54), (90, 20), (100, 54), (100, 33), (100, 54)];
        for round in 0..3 {
            for (c, rw) in sizes {
                r.resize(c, rw);
                std::thread::sleep(Duration::from_millis(15));
                r.pump();
            }
            r.check(&format!("rapid round {round}"), false);
        }
        r.finish();
    }

    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn resize_during_output() {
        let mut r = Rig::powershell();
        r.type_line("1..4000 | % { \"line $_\" }");
        for (c, rw) in [(100, 54), (100, 20), (120, 40), (100, 54), (80, 30)] {
            for _ in 0..8 {
                r.pump();
                std::thread::sleep(Duration::from_millis(10));
            }
            r.resize(c, rw);
        }
        r.check("resize during output", false);
        r.resize(100, 54);
        r.check("after", false);
        r.finish();
    }

    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn resize_with_a_short_screen_then_scroll() {
        let mut r = Rig::powershell();
        for (c, rw) in [(100, 54), (100, 40), (130, 54), (100, 12), (100, 54)] {
            r.resize(c, rw);
            r.check(&format!("fresh resize {c}x{rw}"), true);
        }
        r.type_line(DIR);
        r.check("dir after early resizes", true);
        r.type_line("cls");
        r.type_line("echo a");
        r.type_line("echo b");
        r.check("two short commands", true);
        for (c, rw) in [(100, 20), (100, 54), (90, 8), (110, 54), (100, 30)] {
            r.resize(c, rw);
            r.check(&format!("short resize {c}x{rw}"), false);
        }
        r.type_line(DIR);
        r.check("dir after short resizes", true);
        r.finish();
    }

    /// Up after a resize and a scrolled listing redraws on the prompt row.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn history_recall_after_resize() {
        let mut r = Rig::powershell();
        r.type_line("cd .");
        r.resize(100, 54);
        r.check("grown", true);
        r.type_line(DIR);
        r.check("dir", true);
        r.pty.write(b"\x1b[A").unwrap();
        r.check("up", false);
        let (_, row) = r.cursor();
        let text = r.grid_rows()[row as usize].clone();
        if !(text.starts_with("PS ") && text.contains("dir")) {
            r.failures.push(format!("up: the cursor row {row} holds {text:?}"));
        }
        r.finish();
    }

    /// The field case: a listing that scrolled, the prompt on the bottom row, a resize (taller,
    /// shorter, wider, narrower, and several in a row), then typing. The echo lands on the
    /// prompt, where the grid has it, and the grid shows the rows ConPTY shows.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn typing_after_a_resize_lands_at_the_prompt() {
        let sizes: [&[(u16, u16)]; 9] = [&[(100, 54)], &[(100, 12)], &[(70, 30)], &[(140, 30)], &[(120, 50), (80, 20)], &[(60, 40)], &[(60, 40), (130, 25)], &[(60, 30), (130, 30)], &[(60, 40), (130, 25), (100, 54)]];
        let mut bad = Vec::new();
        for seq in sizes {
            let mut r = Rig::powershell();
            r.type_line(DIR);
            r.check("listing", true);
            for (c, rw) in seq {
                r.resize(*c, *rw);
            }
            r.settle();
            r.pty.write(b"ps -").unwrap();
            r.check(&format!("typed after {seq:?}"), false);
            let rows = r.grid_rows();
            let (_, cr) = r.cursor();
            let echo_rows: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].starts_with("PS ") && rows[*i].ends_with("ps -")).collect();
            if echo_rows != [cr as usize] {
                r.failures.push(format!("typed after {seq:?}: the cursor is on row {cr} but the echo is on {echo_rows:?}"));
            }
            bad.extend(r.failures.clone());
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    #[ignore = "live: needs the shipped ConPTY and cmd"]
    fn cmd_control() {
        let mut r = Rig::new("cmd.exe /q /k prompt PS $P$G", 100, 30);
        r.type_line("for /l %i in (1,1,200) do @echo %i");
        r.check("cmd lines", true);
        r.resize(100, 54);
        r.check("cmd 54", true);
        r.resize(100, 20);
        r.check("cmd 20", true);
        r.finish();
    }
}
