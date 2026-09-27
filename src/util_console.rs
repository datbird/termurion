//! The utilities' real console (milestone 11e): `/dev/tty` as `CONIN$` / `CONOUT$`, VT
//! output on for a console stdout, the scripts' `stty` modes with every changed mode put
//! back on every exit (normal, Ctrl+C, panic), replies read as bytes from
//! `ReadConsoleInputW`, the program runner, and `tandem-utils.exe`'s `real_main`.

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, CloseHandle, FALSE, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile};
use windows_sys::Win32::System::Console::{
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, FOCUS_EVENT, FlushConsoleInputBuffer, GetConsoleMode, GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
};
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

use crate::util_cli::{self, Cmd, FOCUS_IN, Io, KeyText, Mode, Output, Tty};

/// Every console mode a utility changed, oldest first, as (handle, mode before), so the
/// Ctrl+C handler and the panic hook can put them back. Handles are kept as integers.
static SAVED: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

fn write_handle(h: HANDLE, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let mut n = 0u32;
        let len = rest.len().min(1 << 20) as u32;
        if unsafe { WriteFile(h, rest.as_ptr(), len, &mut n, std::ptr::null_mut()) } == 0 || n == 0 {
            return;
        }
        rest = &rest[n as usize..];
    }
}

/// What the Ctrl+C handler writes to stdout before the process ends (it2copy's
/// `trap clean_up INT` closing an open OSC).
static PENDING_ST: Mutex<Option<&'static str>> = Mutex::new(None);

fn remember(h: HANDLE, mode: u32) {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if !saved.iter().any(|(k, _)| *k == h as usize) {
        saved.push((h as usize, mode));
    }
}

/// Puts back the mode saved for `h` and forgets it.
fn restore_one(h: HANDLE) {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = saved.iter().position(|(k, _)| *k == h as usize) {
        let (_, mode) = saved.remove(i);
        unsafe { SetConsoleMode(h, mode) };
    }
}

/// Puts back every saved mode, newest first (two handles on one console restore to the
/// oldest mode).
fn restore_all() {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    for (h, mode) in saved.drain(..).rev() {
        unsafe { SetConsoleMode(h as HANDLE, mode) };
    }
}

fn is_console_handle(h: HANDLE) -> bool {
    let mut mode = 0u32;
    !h.is_null() && h != INVALID_HANDLE_VALUE && unsafe { GetConsoleMode(h, &mut mode) } != 0
}

/// Turns on VT processing for a console output handle, remembering the mode before for the
/// restore. Returns that mode, or None when `handle` is not a console.
pub fn vt_output(handle: HANDLE) -> Option<u32> {
    let mut mode = 0u32;
    if handle.is_null() || handle == INVALID_HANDLE_VALUE || unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return None;
    }
    remember(handle, mode);
    unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
    Some(mode)
}

/// Sets (or clears) what the Ctrl+C handler writes before the process ends.
pub fn set_pending_st(s: Option<&'static str>) {
    *PENDING_ST.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

unsafe extern "system" fn ctrl_restore(_event: u32) -> BOOL {
    // The ST first, while VT output is still on, so it closes the OSC as a sequence.
    if let Some(st) = *PENDING_ST.lock().unwrap_or_else(|e| e.into_inner()) {
        write_handle(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }, st.as_bytes());
    }
    restore_all();
    // Not handled: the default handler still ends the process, as SIGINT ends the script.
    FALSE
}

fn install_ctrl_restore() {
    unsafe { SetConsoleCtrlHandler(Some(ctrl_restore), TRUE) };
}

fn open_console(name: &str) -> HANDLE {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let h = unsafe {
        CreateFileW(wide.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    if h == INVALID_HANDLE_VALUE { std::ptr::null_mut() } else { h }
}

/// `/dev/tty`.
pub struct Console {
    conin: HANDLE,
    conout: HANDLE,
    /// The input mode before `set_mode`.
    saved: Option<u32>,
    /// Bytes decoded from input records and not read yet.
    queue: VecDeque<u8>,
    keys: KeyText,
    /// A reply is awaited (a mode is set): a focus-in record reads as `ESC [ I`.
    awaiting: bool,
}

impl Console {
    /// Opens `CONIN$` and `CONOUT$`; without a console both handles are null and
    /// `is_console` is false.
    pub fn open() -> Console {
        Console { conin: open_console("CONIN$"), conout: open_console("CONOUT$"), saved: None, queue: VecDeque::new(), keys: KeyText::default(), awaiting: false }
    }

    /// Decodes input records into `queue`.
    fn take_records(&mut self, records: &[INPUT_RECORD]) {
        let mut bytes = Vec::new();
        for r in records {
            let kind = u32::from(r.EventType);
            if kind == KEY_EVENT {
                let k = unsafe { r.Event.KeyEvent };
                let unit = unsafe { k.uChar.UnicodeChar };
                if k.bKeyDown != 0 && unit != 0 {
                    for _ in 0..k.wRepeatCount.max(1) {
                        self.keys.push(unit, &mut bytes);
                    }
                }
            } else if kind == FOCUS_EVENT && unsafe { r.Event.FocusEvent.bSetFocus } != 0 && self.awaiting {
                bytes.extend_from_slice(FOCUS_IN);
            }
        }
        self.queue.extend(bytes);
    }

    /// Input ended (the wait ran out or reading failed): a high surrogate still waiting for
    /// its pair becomes U+FFFD, else nothing was read.
    fn ran_out(&mut self) -> Option<u8> {
        let mut bytes = Vec::new();
        self.keys.flush(&mut bytes);
        self.queue.extend(bytes);
        self.queue.pop_front()
    }
}

impl Tty for Console {
    fn write(&mut self, bytes: &[u8]) {
        if self.conout.is_null() {
            return;
        }
        let known = SAVED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(k, _)| *k == self.conout as usize);
        if !known {
            vt_output(self.conout);
        }
        write_handle(self.conout, bytes);
    }

    fn read(&mut self, wait: Option<Duration>) -> Option<u8> {
        if self.conin.is_null() {
            return None;
        }
        let end = wait.map(|w| Instant::now() + w);
        loop {
            if let Some(b) = self.queue.pop_front() {
                return Some(b);
            }
            let ms = match end {
                None => INFINITE,
                Some(end) => end.saturating_duration_since(Instant::now()).as_millis().min(u128::from(INFINITE - 1)) as u32,
            };
            if unsafe { WaitForSingleObject(self.conin, ms) } != WAIT_OBJECT_0 {
                return self.ran_out();
            }
            let mut records: [INPUT_RECORD; 64] = unsafe { std::mem::zeroed() };
            let mut n = 0u32;
            if unsafe { ReadConsoleInputW(self.conin, records.as_mut_ptr(), records.len() as u32, &mut n) } == 0 {
                return self.ran_out();
            }
            self.take_records(&records[..n as usize]);
            if self.queue.is_empty() && end.is_some_and(|e| Instant::now() >= e) {
                return self.ran_out();
            }
        }
    }

    fn set_mode(&mut self, m: Mode) {
        let before = match self.saved {
            Some(s) => s,
            None => {
                let mut mode = 0u32;
                if self.conin.is_null() || unsafe { GetConsoleMode(self.conin, &mut mode) } == 0 {
                    return;
                }
                remember(self.conin, mode);
                self.saved = Some(mode);
                mode
            }
        };
        unsafe { SetConsoleMode(self.conin, util_cli::input_mode(m, before)) };
        self.awaiting = true;
    }

    fn discard_input(&mut self) {
        if !self.conin.is_null() {
            unsafe { FlushConsoleInputBuffer(self.conin) };
        }
        self.queue.clear();
    }

    fn restore(&mut self) {
        if self.saved.take().is_some() {
            restore_one(self.conin);
        }
        if !self.conout.is_null() {
            restore_one(self.conout);
        }
        self.awaiting = false;
    }

    fn is_console(&self) -> bool {
        is_console_handle(self.conin)
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.restore();
        for h in [self.conin, self.conout] {
            if !h.is_null() {
                unsafe { CloseHandle(h) };
            }
        }
    }
}

/// Runs `c` and waits for it: its output captured (standard error shared) unless it
/// inherits the utility's streams; with no `stdin` bytes it reads nothing.
pub fn run_command(c: &Cmd) -> Result<Output, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(&c.program);
    cmd.args(&c.args);
    if let Some(dir) = &c.cwd {
        cmd.current_dir(dir);
    }
    if c.inherit {
        let status = cmd.status().map_err(|e| e.to_string())?;
        return Ok(Output { code: status.code().unwrap_or(1), stdout: Vec::new() });
    }
    cmd.stdout(Stdio::piped()).stderr(if c.quiet { Stdio::null() } else { Stdio::inherit() });
    cmd.stdin(if c.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let feeder = match (c.stdin.clone(), child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => Some(std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        })),
        _ => None,
    };
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if let Some(f) = feeder {
        let _ = f.join();
    }
    Ok(Output { code: out.status.code().unwrap_or(1), stdout: out.stdout })
}

/// `builtin command -V <program>`: a path naming a file, or a file of that name (with a
/// PATHEXT extension or none) in a PATH folder. Network folders and PATH entries that are
/// not absolute (the current folder) are never probed.
pub fn on_path(program: &str) -> bool {
    let local = |p: &Path| {
        let s = p.to_string_lossy();
        !(s.starts_with("\\\\") || s.starts_with("//")) && p.is_file()
    };
    if program.contains(['\\', '/']) {
        return local(Path::new(program));
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(';').map(str::trim).filter(|d| Path::new(d).is_absolute()).any(|dir| {
        let base = Path::new(dir).join(program);
        local(&base) || exts.split(';').filter(|e| !e.is_empty()).any(|e| local(&PathBuf::from(format!("{}{e}", base.display()))))
    })
}

/// `$RANDOM`: 0 to 32767, from the standard library's per-process random hash keys.
fn random_15() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    static STATE: std::sync::OnceLock<std::collections::hash_map::RandomState> = std::sync::OnceLock::new();
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = STATE.get_or_init(Default::default).build_hasher();
    h.write_u64(COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    (h.finish() & 0x7FFF) as u32
}

/// Runs `f` over the real streams and console: VT output on for a console stdout, every
/// changed console mode put back on Ctrl+C, on a panic and when `f` returns.
fn with_console_io(f: &mut dyn FnMut(&mut Io) -> i32) -> i32 {
    install_ctrl_restore();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_all();
        previous(info);
    }));
    let stdout_tty = std::io::stdout().is_terminal();
    if stdout_tty {
        vt_output(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) });
    }
    let code = {
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        let mut input = std::io::stdin();
        let mut tty = Console::open();
        let env = |k: &str| std::env::var(k).ok();
        let mut random = random_15;
        let mut run = run_command;
        let mut io = Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
            stdin_tty: std::io::stdin().is_terminal(),
            stdout_tty,
            tty: &mut tty,
            env: &env,
            cwd: std::env::current_dir().unwrap_or_default(),
            random: &mut random,
            run: &mut run,
            have: &on_path,
        };
        let code = f(&mut io);
        let _ = io.out.flush();
        let _ = io.err.flush();
        code
    };
    restore_all();
    code
}

/// `tandem-utils.exe`: picks the utility from the program's name or first argument, runs
/// it over the real console, and returns its exit code.
pub fn real_main() -> i32 {
    let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    match util_cli::pick(&args) {
        Ok((name, rest)) => with_console_io(&mut |io| util_cli::run(name, &rest, io)),
        Err(usage) => {
            let mut err = std::io::stderr();
            let _ = err.write_all(usage.as_bytes());
            let _ = err.flush();
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conpty::{self, Source};
    use crate::osc1337::Attention;
    use crate::pty::{self, Read};
    use crate::screen::{Request, Screen};
    use crate::util_osc::{b64, seq};

    fn shipped() -> conpty::Api {
        conpty::load(&Source::Shipped(conpty::dev_vendor_dir())).unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn mode_of(h: HANDLE) -> u32 {
        let mut m = 0u32;
        unsafe { GetConsoleMode(h, &mut m) };
        m
    }

    /// Reads for `wait` and returns every byte read.
    fn read_for(c: &mut Console, wait: Duration) -> Vec<u8> {
        let end = Instant::now() + wait;
        let mut got = Vec::new();
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return got;
            }
            if let Some(b) = c.read(Some(left)) {
                got.push(b);
            }
        }
    }

    fn say(io: &mut Io, line: &str) {
        let _ = io.out.write_all(format!("{line}\r\n").as_bytes());
        let _ = io.out.flush();
    }

    /// `probe:out`: an attention request, then a 64 KB clipboard write, on stdout.
    fn probe_out(io: &mut Io) -> i32 {
        let _ = io.out.write_all(seq(None, "1337;RequestAttention=once").as_bytes());
        let _ = io.out.write_all(seq(None, &format!("1337;Copy=:{}", b64(&[b'q'; 48 * 1024]))).as_bytes());
        let _ = io.out.flush();
        0
    }

    /// `probe:in`: raw mode, then 2 s of input read back as hex.
    fn probe_in(io: &mut Io) -> i32 {
        let mut c = Console::open();
        let before = mode_of(c.conin);
        say(io, &format!("MODES {before:x}"));
        c.set_mode(Mode::Raw);
        say(io, &format!("MODE {:x}", mode_of(c.conin)));
        say(io, "READY");
        let got = read_for(&mut c, Duration::from_secs(2));
        say(io, &format!("GOT {}", hex(&got)));
        c.restore();
        say(io, &format!("AFTER {:x}", mode_of(c.conin)));
        0
    }

    /// `probe:dsr`: DSR 5 on stdout, then 1 s of input read back as hex.
    fn probe_dsr(io: &mut Io) -> i32 {
        let mut c = Console::open();
        c.set_mode(Mode::Raw);
        let _ = io.out.write_all(b"\x1b[5n");
        let _ = io.out.flush();
        let got = read_for(&mut c, Duration::from_secs(1));
        c.restore();
        say(io, &format!("GOT {}", hex(&got)));
        0
    }

    /// The child half of the ConPTY tests: `TANDEM_UTILITY_CHILD` holds, in hex, a name and
    /// its arguments separated by U+001F; runs that probe or utility over the real console and
    /// exits with its code (so libtest's summary never decides it).
    #[test]
    #[ignore]
    fn utility_child() {
        let Ok(hexed) = std::env::var("TANDEM_UTILITY_CHILD") else {
            return;
        };
        let value = String::from_utf8(unhex(&hexed)).unwrap();
        let mut parts = value.split('\u{1f}').map(str::to_string);
        let name = parts.next().unwrap_or_default();
        let args: Vec<String> = parts.collect();
        let code = with_console_io(&mut |io| match name.as_str() {
            "probe:out" => probe_out(io),
            "probe:in" => probe_in(io),
            "probe:dsr" => probe_dsr(io),
            other => util_cli::run(other, &args, io),
        });
        std::process::exit(code);
    }

    /// Runs `child` (a name and arguments for `utility_child`) in the shipped ConPTY, feeding
    /// its output to a `Screen` until it exits (60 s cap). With `replies` the screen's answers
    /// go back to the child; `answer` may add bytes for each request, `watch` for the screen
    /// after each chunk. Returns the exit code, every byte read, and the screen. The child's
    /// words travel in hex inside cmd's quoted `set "NAME=value"`, so `&`, `^`, `%` and `"`
    /// pass untouched.
    fn drive(
        child: &[&str],
        replies: bool,
        answer: &mut dyn FnMut(&Request, &mut Vec<u8>),
        watch: &mut dyn FnMut(&Screen, &mut Vec<u8>),
    ) -> (Option<u32>, Vec<u8>, Screen) {
        let exe = std::env::current_exe().unwrap();
        let cmdline = format!(
            "cmd.exe /c set \"TANDEM_UTILITY_CHILD={}\"&& \"{}\" --exact util_console::tests::utility_child --ignored --nocapture --test-threads=1",
            hex(child.join("\u{1f}").as_bytes()),
            exe.display()
        );
        let pty = pty::spawn(&shipped(), &cmdline, 200, 50).unwrap();
        let mut screen = Screen::new(200, 50);
        let mut all = Vec::new();
        let end = Instant::now() + Duration::from_secs(60);
        let mut exited_at: Option<Instant> = None;
        loop {
            let mut back = Vec::new();
            match pty.try_read() {
                Read::Data(d) => {
                    let reply = screen.feed(&d);
                    all.extend_from_slice(&d);
                    if replies {
                        back.extend(reply);
                        back.extend(screen.take_replies());
                    }
                    for r in screen.take_requests() {
                        answer(&r, &mut back);
                    }
                    watch(&screen, &mut back);
                }
                Read::Empty => std::thread::sleep(Duration::from_millis(10)),
                Read::Closed => break,
            }
            if !back.is_empty() {
                let _ = pty.write(&back);
            }
            // After the exit, a moment for the last output still in the pipe.
            match exited_at {
                None if pty.exit_code().is_some() => exited_at = Some(Instant::now()),
                Some(t) if t.elapsed() > Duration::from_millis(500) => break,
                _ => {}
            }
            assert!(Instant::now() < end, "the child {child:?} did not finish within 60 s; output {:?}", String::from_utf8_lossy(&all));
        }
        (pty.exit_code(), all, screen)
    }

    fn run_in_conpty(child: &[&str], answer: &mut dyn FnMut(&Request, &mut Vec<u8>)) -> (Option<u32>, Vec<u8>, Screen) {
        drive(child, true, answer, &mut |_, _| {})
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// The word after the word `key` on the screen (libtest's "test ... " may start its line).
    fn field(screen: &Screen, key: &str) -> Option<String> {
        screen.text().lines().find_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            words.iter().position(|w| *w == key).map(|i| words.get(i + 1).map_or(String::new(), |w| w.to_string()))
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
    }

    #[test]
    fn conpty_carries_utility_output() {
        let mut requests = Vec::new();
        let (code, bytes, _) = run_in_conpty(&["probe:out"], &mut |r, _| requests.push(r.clone()));
        assert_eq!(code, Some(0));
        let attention = seq(None, "1337;RequestAttention=once");
        let copy = seq(None, &format!("1337;Copy=:{}", b64(&[b'q'; 48 * 1024])));
        assert!(contains(&bytes, attention.as_bytes()), "the attention request arrives unchanged");
        assert!(contains(&bytes, copy.as_bytes()), "the 64 KB clipboard write arrives contiguous and unchanged");
        let wanted = vec![Request::Attention(Attention::Once), Request::ClipboardWrite("q".repeat(48 * 1024))];
        let seen: Vec<Request> = requests.into_iter().filter(|r| matches!(r, Request::Attention(_) | Request::ClipboardWrite(_))).collect();
        assert_eq!(seen, wanted);
    }

    #[test]
    fn imgcat_through_conpty() {
        // 200x200 pseudo-random pixels: the PNG cannot compress below 100 KB.
        let mut seed = 0x2545_F491_u32;
        let img = image::RgbImage::from_fn(200, 200, |_, _| {
            let mut px = [0u8; 3];
            for c in &mut px {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *c = seed as u8;
            }
            image::Rgb(px)
        });
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        assert!(png.len() > 100 * 1024, "{} bytes", png.len());
        // cmd's special characters in the path (review: the child's words are carried in hex).
        let path = std::env::temp_dir().join(format!("tandem imgcat 100% & ^ (conpty) {}.png", std::process::id()));
        std::fs::write(&path, &png).unwrap();
        let (code, _, screen) = run_in_conpty(&["imgcat", &path.to_string_lossy()], &mut |_, _| {});
        let _ = std::fs::remove_file(&path);
        assert_eq!(code, Some(0));
        assert_eq!(screen.images.by_id.len(), 1);
        let shown = screen.images.by_id.values().next().unwrap();
        assert_eq!((shown.width_px, shown.height_px), (200, 200));
        assert!(!shown.broken);
    }

    /// A measurement (pre-flight ruling): run by hand with
    /// `cargo test --lib util_console::tests::conpty_carries_replies -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn conpty_carries_replies() {
        const REPLY: &[u8] = b"\x1b[ITERM2 3.7.0n\x1b[0n";
        let mut sent = false;
        let (code, bytes, screen) = drive(&["probe:in"], true, &mut |_, _| {}, &mut |s, back| {
            if !sent && field(s, "READY").is_some() {
                sent = true;
                back.extend_from_slice(REPLY);
            }
        });
        assert_eq!(code, Some(0));
        let before = u32::from_str_radix(&field(&screen, "MODES").expect("MODES"), 16).unwrap();
        let mode = u32::from_str_radix(&field(&screen, "MODE").expect("MODE"), 16).unwrap();
        let after = u32::from_str_radix(&field(&screen, "AFTER").expect("AFTER"), 16).unwrap();
        let got = unhex(&field(&screen, "GOT").unwrap_or_default());
        println!("MEASURED probe:in modes before {before:#x}, raw {mode:#x}, after {after:#x}, got {:?}", String::from_utf8_lossy(&got));
        assert_eq!(util_cli::restore_reply_prefix(&got), REPLY, "the reply, its swallowed ESC [ I put back");
        println!("MEASURED child wrote ?1004l: {}", contains(&bytes, b"\x1b[?1004l"));
        assert!(!contains(&bytes, b"\x1b[?1004l"), "raw mode leaves the pane's focus reporting alone");
        assert_eq!(mode, util_cli::input_mode(Mode::Raw, before));
        assert_eq!(after, before);

        let (code, _, screen) = drive(&["probe:dsr"], false, &mut |_, _| {}, &mut |_, _| {});
        assert_eq!(code, Some(0));
        let got = unhex(&field(&screen, "GOT").unwrap_or_default());
        println!("MEASURED dsr answered by ConPTY: {} (got {:?})", !got.is_empty(), String::from_utf8_lossy(&got));
    }
}
