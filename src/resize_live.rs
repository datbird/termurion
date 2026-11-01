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

    use crate::platform::pty::conpty::{self, Source, dev_vendor_dir};
    use crate::platform::pty::{Pty, Read, spawn_notify};
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
                if live_opt("LIVE_FULL").is_some() {
                    for i in 0..grid.len().min(rows.len()) {
                        println!("LIVE   full {i}: grid {:?} / ConPTY {:?}", grid[i], rows[i]);
                    }
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
            crate::platform::pty::attached_unless(pid, &std::sync::atomic::AtomicBool::new(false), || unsafe {
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

    /// A debugging switch: a `KEY=value` line in `live_debug.txt` beside Cargo.toml (the tests run on
    /// another machine than the one that starts them, so the environment does not reach them).
    fn live_opt(key: &str) -> Option<String> {
        let text = std::fs::read_to_string("live_debug.txt").ok()?;
        text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=').map(|v| v.trim().to_string()))
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

    // ---- Resizes while a program holds the alternate screen (ESC [ ? 1049 h) ----

    /// A stand-in for tmux: enters the alternate screen, draws every row (CUP), a status line on
    /// the last row and a scroll region above it, redraws whenever the console size changes, and
    /// leaves the alternate screen and prints three lines when it reads a q.
    const ALT_CHILD: &str = r#"$e = [char]27
$o = [Console]::Out
$o.Write("$e[?1049h")
$w = 0; $h = 0
while ($true) {
    $nw = [Console]::WindowWidth; $nh = [Console]::WindowHeight
    if ($nw -ne $w -or $nh -ne $h) {
        $w = $nw; $h = $nh
        $sb = New-Object System.Text.StringBuilder
        [void]$sb.Append("$e[r$e[2J")
        for ($r = 1; $r -lt $h; $r++) { [void]$sb.Append("$e[$r;1Hrow $r of $h, $w wide") }
        [void]$sb.Append("$e[$h;1H$e[7m" + ("[funtimes] 0:pwsh*".PadRight($w - 1)) + "$e[0m")
        [void]$sb.Append("$e[1;$($h - 1)r$e[$($h - 1);5H")
        $o.Write($sb.ToString())
    }
    if ([Console]::KeyAvailable) { if ([Console]::ReadKey($true).KeyChar -eq 'q') { break } }
    Start-Sleep -Milliseconds 40
}
$o.Write("$e[r$e[?1049l")
$o.Write("[detached (from session funtimes)]`r`nlogout`r`nConnection closed.`r`n")
"#;

    /// The stand-in's script on disk, and the command that runs it.
    fn alt_child_cmd() -> String {
        let path = std::env::temp_dir().join(format!("termurion-altchild-{}.ps1", std::process::id()));
        std::fs::write(&path, ALT_CHILD).unwrap();
        format!("powershell.exe -NoLogo -NoProfile -File \"{}\"", path.display())
    }

    impl Rig {
        fn in_alt(&self) -> bool {
            self.screen.alt_screen()
        }

        /// Waits (pumping) until `f` is true, at most 30 s.
        fn wait_for(&mut self, what: &str, f: impl Fn(&Rig) -> bool) {
            let end = Instant::now() + Duration::from_secs(30);
            while Instant::now() < end {
                self.pump();
                if f(self) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            self.failures.push(format!("timed out waiting for {what}"));
        }

        /// Defines `a` as the stand-in, so the line that starts it is short and never wraps.
        fn define_alt(&mut self) -> String {
            let cmd = alt_child_cmd();
            self.type_line(&format!("function a {{ {cmd} }}"));
            self.settle();
            "a".to_string()
        }

        fn enter_alt(&mut self, cmd: &str) {
            self.type_line(cmd);
            self.wait_for("the alternate screen", |r| r.in_alt());
            self.settle();
        }

        /// The stand-in leaves the alternate screen and exits; the prompt comes back.
        fn leave_alt(&mut self) {
            self.pty.write(b"q").unwrap();
            self.wait_for("the main screen", |r| !r.in_alt());
            self.settle();
        }
    }

    /// Resizes one after the other; with `check_alt` the alternate screen is compared with
    /// ConPTY's at each idle point.
    fn alt_resizes(r: &mut Rig, sizes: &[(u16, u16)], check_alt: bool) {
        for (c, rw) in sizes {
            r.resize(*c, *rw);
            if check_alt {
                r.settle();
                r.check(&format!("in the alternate screen at {c}x{rw}"), false);
            }
        }
        r.settle();
    }

    const BIG: (u16, u16) = (272, 70);
    const SMALL: (u16, u16) = (80, 23);

    /// The report, with a local stand-in: a prompt far down a long main screen, the alternate
    /// screen, the window shrunk a lot and grown again, drag steps, then the program leaves.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn alt_screen_resized_then_left() {
        let mut r = Rig::powershell();
        let child = r.define_alt();
        r.resize(BIG.0, BIG.1);
        r.settle();
        r.type_line(DIR);
        r.type_line("1..150 | % { \"scrollback $_\" }");
        r.type_line(DIR);
        r.check("before", true);
        r.enter_alt(&child);
        let steps = [SMALL, BIG, (80, 22), (120, 30), (205, 45), (138, 42), (146, 42), (90, 12), SMALL, BIG];
        alt_resizes(&mut r, &steps, true);
        r.leave_alt();
        r.check("after leaving the alternate screen", true);
        r.check_echo("typing after leaving");
        r.type_line("echo done");
        r.check("a command after", true);
        r.finish();
    }

    /// The same with the sizes arriving in a fast drag (no idle time between steps).
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn alt_screen_drag_then_left() {
        let mut r = Rig::powershell();
        let child = r.define_alt();
        r.resize(BIG.0, BIG.1);
        r.settle();
        r.type_line(DIR);
        r.type_line(DIR);
        r.enter_alt(&child);
        let mut c = 272i32;
        let mut rw = 70i32;
        for i in 0..60 {
            c = if i < 30 { c - 6 } else { c + 6 };
            rw = if i < 30 { rw - 1 } else { rw + 1 };
            r.resize(c as u16, rw as u16);
            std::thread::sleep(Duration::from_millis(16));
            r.pump();
        }
        r.settle();
        r.leave_alt();
        r.check("after a drag in the alternate screen", true);
        r.check_echo("typing after the drag");
        r.finish();
    }

    /// Resizes with the main screen showing, the program in the alternate screen, and a resize
    /// as the program leaves, all with a prompt far down.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn alt_screen_resize_as_it_leaves() {
        let mut r = Rig::powershell();
        let child = r.define_alt();
        r.resize(BIG.0, BIG.1);
        r.settle();
        r.type_line(DIR);
        r.enter_alt(&child);
        r.resize(SMALL.0, SMALL.1);
        r.settle();
        r.pty.write(b"q").unwrap();
        r.resize(BIG.0, BIG.1);
        r.wait_for("the main screen", |r| !r.in_alt());
        r.settle();
        r.check("resized as it left", true);
        r.check_echo("typing after");
        r.finish();
    }

    /// The real thing when WSL has tmux: a shell in WSL, tmux on its own socket, resizes, detach,
    /// exit, and the prompt of the first shell.
    #[test]
    #[ignore = "live: needs the shipped ConPTY, powershell and tmux in WSL"]
    fn tmux_in_wsl_resized_then_detached() {
        let have = std::process::Command::new("wsl.exe").args(["-e", "tmux", "-V"]).output().map(|o| o.status.success()).unwrap_or(false);
        if !have {
            println!("LIVE tmux in WSL: skipped, tmux is not available in WSL");
            return;
        }
        let mut r = Rig::powershell();
        r.resize(BIG.0, BIG.1);
        r.settle();
        r.type_line(DIR);
        r.type_line("1..100 | % { \"scrollback $_\" }");
        r.check("before", true);
        r.type_line("wsl.exe");
        r.settle();
        r.type_line("tmux -L termurion_live -f /dev/null new-session -s funtimes");
        r.wait_for("tmux", |r| r.in_alt());
        r.settle();
        r.type_line("seq 1 300");
        r.settle();
        let steps = [SMALL, BIG, (80, 22), (120, 30), (205, 45), (138, 42), (146, 42), SMALL, BIG];
        alt_resizes(&mut r, &steps, false);
        r.type_line("tmux detach-client");
        r.wait_for("tmux to detach", |r| !r.in_alt());
        if !r.failures.is_empty() {
            println!("LIVE   bytes at the end: {}", r.escaped_trace());
            println!("LIVE   grid: {:?}", r.grid_rows());
        }
        r.settle();
        r.type_line("tmux -L termurion_live kill-server 2>/dev/null; exit");
        r.settle();
        r.check("PowerShell after tmux", true);
        r.check_echo("typing after tmux");
        r.type_line("exit");
        r.finish();
    }

    /// A seeded walk that enters and leaves the alternate screen at random points among resizes
    /// (some right after the program starts, some while it draws), with listings between. At the
    /// end the program is gone and the cursors agree.
    fn alt_walk(seed: u64, early_resizes: bool) -> Rig {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut next = move |n: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % n
        };
        let mut r = Rig::powershell();
        let child = r.define_alt();
        r.resize(BIG.0, BIG.1);
        r.settle();
        r.type_line(DIR);
        for step in 0..14 {
            let alt = r.in_alt();
            if live_opt("LIVE_FULL").is_some() {
                println!("LIVE   step {step}: alt {alt}, cursor {:?}, size {}x{}", r.cursor(), r.screen.term().grid().columns(), r.screen.term().grid().screen_lines());
            }
            match next(4) {
                0 if !alt => {
                    r.type_line(&child);
                    // Resizes may arrive before the program has drawn anything.
                    if early_resizes && next(2) == 0 {
                        std::thread::sleep(Duration::from_millis(next(400)));
                    } else {
                        r.wait_for("the alternate screen", |r| r.in_alt());
                    }
                }
                1 if alt => {
                    r.pty.write(b"q").unwrap();
                    if next(2) == 0 {
                        r.resize(95 + next(177) as u16, 6 + next(60) as u16);
                    }
                    r.wait_for("the main screen", |r| !r.in_alt());
                    r.settle();
                    r.check_echo(&format!("seed {seed} step {step} after leaving"));
                }
                2 if !alt => r.type_line(DIR),
                _ => {}
            }
            for _ in 0..(1 + next(4)) {
                r.resize(95 + next(177) as u16, 6 + next(60) as u16);
                if next(2) == 0 {
                    std::thread::sleep(Duration::from_millis(next(80)));
                    r.pump();
                }
            }
            r.settle();
            if !r.in_alt() {
                if live_opt("LIVE_FULL").is_some() {
                    r.check(&format!("seed {seed} step {step} rows"), false);
                }
                r.check_echo(&format!("seed {seed} step {step}"));
            }
        }
        if r.in_alt() {
            r.leave_alt();
        }
        r.settle();
        r.type_line("echo done");
        r.check(&format!("seed {seed} after a command"), true);
        println!("LIVE SUMMARY alt seed {seed}: {} problems", r.failures.len());
        r
    }

    fn alt_seeds() -> Vec<u64> {
        match live_opt("LIVE_SEED") {
            Some(v) => v.split(',').map(|s| s.parse().unwrap()).collect(),
            None => vec![3, 5, 7, 11, 13, 17],
        }
    }

    /// Every resize comes once the program has the alternate screen up (or the shell is idle).
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn random_walk_alt_screen() {
        let mut bad = Vec::new();
        for seed in alt_seeds() {
            bad.extend(alt_walk(seed, false).failures);
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// Also resizes while the program is still starting (the shell has run the command and the
    /// alternate screen is not up yet): the same in-flight bound as `random_walk_in_flight`.
    #[test]
    #[ignore = "live: needs the shipped ConPTY and powershell"]
    fn random_walk_alt_screen_while_starting() {
        let mut total = 0;
        for seed in alt_seeds() {
            total += alt_walk(seed, true).failures.len();
        }
        println!("LIVE TOTAL alt, resizes while starting: {total} problems");
        assert!(total <= 2, "{total} problems");
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
