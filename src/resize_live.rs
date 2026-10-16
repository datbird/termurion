//! Live resize guard: a real shell behind the shipped ConPTY, fed through the same `Screen` a
//! pane uses and resized the way a pane resizes (the screen, then the pty). Once the shell is
//! idle, the rows of the grid must be the rows of ConPTY's own screen (read back from its
//! console), and after the shell redraws its prompt the cursors must agree too.
//!
//! What the tests rest on (ConPTY 1.25.260930003, scripts/fetch-conpty.ps1):
//! - ConPTY keeps no scrollback and keeps the top of its screen where it is: rows added by a
//!   resize are blank rows under the cursor, lines that merge or wrap move the text and the cursor
//!   within the screen, and nothing is pulled back from history. The grid does the same
//!   (alacritty_terminal patch 34), or the prompt sits on another row than the one the shell
//!   addresses and the echo of the next key lands in the middle of the output.
//! - Since 1.25 ConPTY asks the terminal for its cursor (ESC [ 6 n) the first time a program reads
//!   the screen after a resize, and takes the answer as its own cursor
//!   (microsoft/terminal#18725, PR #19535). The terminal must answer every such query, in order
//!   with the output; `Screen::feed` does. 1.24 sends no query.
//! - Nobody can align the two reflows exactly (xterm.js tried and reverted it, PR 5321 and 5358;
//!   Windows Terminal reflows its own buffer and guesses the ConPTY viewport top in
//!   `Terminal::UserResize`), so the guard compares what the user sees: rows, cursor and echo.
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
        fn console(&mut self) -> Result<(Vec<String>, (i32, i32)), String> {
            // ConPTY 1.25 asks the terminal for its cursor the first time a program reads the
            // screen, and waits for the answer: read it on another thread while this one pumps.
            let pid = self.pty.pid();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(read_console(pid));
            });
            loop {
                self.pump();
                if let Ok(v) = rx.recv_timeout(Duration::from_millis(5)) {
                    return v;
                }
            }
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

        /// Types "ps -" at the idle prompt: the echo must land on the one prompt row the grid
        /// shows, which is the row of the grid's cursor. Then clears the line again.
        fn check_echo(&mut self, what: &str) {
            self.pty.write(b"ps -").unwrap();
            self.settle();
            let rows = self.grid_rows();
            let (_, cr) = self.cursor();
            let echo: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].starts_with("PS ") && rows[*i].ends_with("ps -")).collect();
            let ok = echo == [cr as usize];
            println!("LIVE {what}: echo {} (cursor row {cr}, echo rows {echo:?})", if ok { "ok" } else { "MISMATCH" });
            if !ok {
                let holding: Vec<(usize, String)> = (0..rows.len()).filter(|i| rows[*i].contains("ps -")).map(|i| (i, rows[i].clone())).collect();
                println!("LIVE   cursor row {cr} holds {:?}; rows with the echo: {holding:?}", rows.get(cr as usize));
                println!("LIVE   bytes since the last resize: {}", self.escaped_trace());
                if let Ok((crows, at)) = self.console() {
                    println!("LIVE   ConPTY cursor {at:?}");
                    for i in 0..rows.len().min(crows.len()) {
                        if rows[i] != crows[i] {
                            println!("LIVE   row {i}: grid {:?} / ConPTY {:?}", rows[i], crows[i]);
                        }
                    }
                }
                self.failures.push(format!("{what}: the cursor is on row {cr} but the echo is on rows {echo:?}"));
            }
            // Esc clears the line in PSReadLine; the plain line editor takes Esc as well.
            self.pty.write(b"\x1b").unwrap();
            self.settle();
        }

        fn finish(self) {
            assert!(self.failures.is_empty(), "{:#?}", self.failures);
        }
    }

    /// ConPTY's own screen: the rows of the window and the cursor as (column, window row).
    fn read_console(pid: u32) -> Result<(Vec<String>, (i32, i32)), String> {
            use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
            use windows_sys::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, COORD, FreeConsole, GetConsoleScreenBufferInfo, ReadConsoleOutputCharacterW};
            // The test process has a console of its own; attaching to the shell's needs none.
            unsafe { FreeConsole() };
            crate::pty::attached_unless(pid, &std::sync::atomic::AtomicBool::new(false), || unsafe {
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

    /// A seeded walk of resizes, listings, history recalls and resizes with output in flight. At
    /// every idle point the grid shows ConPTY's rows; at the end, after a command, the cursors
    /// agree and sit on the prompt.
    fn walk(seed: u64, plain: bool, in_flight: bool) -> Rig {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut next = move |n: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % n
        };
        let mut r = Rig::powershell();
        if plain {
            // The shell's own line editor echoes at the console's cursor; PSReadLine draws at
            // the row it remembered from before a resize, which is its bug, not the terminal's.
            r.type_line("Remove-Module PSReadLine");
            r.settle();
        }
        for step in 0..24 {
            match next(6) {
                0 => r.type_line(DIR),
                1 => r.type_line("echo hi"),
                2 => {
                    r.pty.write(b"\x1b[A").unwrap();
                    r.settle();
                    r.pty.write(b"\x1b").unwrap();
                    r.settle();
                }
                3 if in_flight => {
                    // Output still arriving while the size changes.
                    r.type_line("1..600 | % { \"row $_\" }");
                    std::thread::sleep(Duration::from_millis(next(120)));
                    r.pump();
                    r.resize(50 + next(100) as u16, 6 + next(50) as u16);
                }
                _ => {}
            }
            if !in_flight {
                // Whatever the step typed has run and the prompt is back.
                r.settle();
            }
            for _ in 0..(1 + next(3)) {
                r.resize(50 + next(100) as u16, 6 + next(50) as u16);
                if next(2) == 0 {
                    std::thread::sleep(Duration::from_millis(next(60)));
                    r.pump();
                }
            }
            r.settle();
            r.check_echo(&format!("seed {seed} step {step}"));
        }
        r.type_line("echo done");
        r.check(&format!("seed {seed} after a command"), true);
        println!("LIVE SUMMARY seed {seed}: {} problems", r.failures.len());
        r
    }

    /// Resizes and listings at an idle prompt, with PSReadLine: no output is
    /// on its way when a size goes out. The grid and ConPTY agree at every step.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn random_walk_idle() {
        let mut bad = Vec::new();
        for seed in [7u64, 11, 13, 17, 19, 23] {
            bad.extend(walk(seed, false, false).failures);
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// The same with a long listing still arriving when the size changes. The shell and ConPTY
    /// each hold geometry from before the resize, so a keystroke typed while a size changes can
    /// still land wrong (1 of 6 seeds in the last measured run, 0 in the one before): the
    /// bound is 2.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn random_walk_in_flight() {
        let mut total = 0;
        for seed in [7u64, 11, 13, 17, 19, 23] {
            total += walk(seed, false, true).failures.len();
        }
        println!("LIVE TOTAL in flight: {total} problems over 6 seeds");
        assert!(total <= 2, "{total} problems over 6 seeds");
    }

    /// A drag: a new size every 16 ms for about 200 ms while a listing is still being printed.
    fn drag_with_output() -> usize {
        let mut x = 99u64;
        let mut next = move |n: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % n
        };
        let mut r = Rig::powershell();
        for round in 0..8 {
            r.type_line("1..600 | % { \"row $_\" }");
            std::thread::sleep(Duration::from_millis(40));
            let (mut c, mut rw) = (60 + next(60) as i32, 10 + next(40) as i32);
            for _ in 0..12 {
                c = (c + next(9) as i32 - 4).clamp(50, 140);
                rw = (rw + next(7) as i32 - 3).clamp(6, 56);
                r.resize(c as u16, rw as u16);
                std::thread::sleep(Duration::from_millis(16));
                r.pump();
            }
            r.settle();
            r.check_echo(&format!("drag round {round}"));
        }
        r.failures.len()
    }

    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn a_drag_with_output_in_flight() {
        let problems = drag_with_output();
        println!("LIVE DRAG: {problems} problems");
        assert_eq!(problems, 0);
    }

    /// ConPTY 1.25 sends DA1 and cursor queries and takes the answers from the terminal. A pane
    /// nobody is attached to (a kept session whose Termurion left) answers none, and must not
    /// stall: output and input still go through at once.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and cmd"]
    fn unanswered_queries_do_not_stall_the_shell() {
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let pty = spawn_notify(&api, "cmd.exe /q /k", None, &[], 80, 24, std::sync::Arc::new(|| {})).unwrap();
        pty.read_until(b"termurion>", Duration::from_secs(5)).unwrap_or_else(|got| panic!("no prompt: {:?}", String::from_utf8_lossy(&got)));
        pty.resize(100, 30).unwrap();
        let t = Instant::now();
        pty.write(b"echo abc^123\r").unwrap();
        pty.read_until(b"abc123", Duration::from_secs(5)).expect("the echo of a command");
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
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
