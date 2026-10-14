# Tandem Milestone 1: Terminal Basics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Windows terminal window you can use every day: tabs, splits, a GPU renderer, keys, iTerm2-style mouse selection, and copy and paste.

**Architecture:** Pure logic lives in small modules with unit tests: palette, layout tree, keys and shortcuts, mouse decisions, and the scene (what to draw, as plain data). A `Pane` joins a `Pty` and a `Screen`. The renderer draws a scene with wgpu (rectangles) and glyphon (text), and is tested offscreen. `app.rs` wires winit events to all of this.

**Tech Stack:** Rust 2024, `alacritty_terminal` 0.26.0, `windows-sys` 0.59, `winit` 0.30, `wgpu` 30, `glyphon` 0.12 (cosmic-text 0.19), `arboard` 3, `pollster` 0.4, `bytemuck` 1.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inputs from milestone 0: `docs/probe/2026-09-24-m0-results.md`.

## Where things run

Same as milestone 0. Edit on the Linux dev machine in `~/gitrepos/tandem`. `scripts/xt <cargo args>` builds and tests on the Windows test machine. The GPU tests render offscreen, so they open no window. Robert may be using the Windows test machine. Nothing in Tasks 1 to 10 may open a window on his desktop.

## Global Constraints

- Everything from the milestone 0 plan still holds (Windows only, pinned crates, ConPTY 1.24.260710001, no silent fallbacks, no Claude attribution, work on `main`, ASCII `.ps1`, no emoji or em dashes in docs).
- New crates, pinned by major version: `winit = "0.30"`, `wgpu = "30"`, `glyphon = "0.12"`, `arboard = "3"`, `pollster = "0.4"`, `bytemuck = "1"`.
- Keys: iTerm2 Cmd is Ctrl+Shift, Cmd+Shift is Ctrl+Shift+Alt, Option is Alt (spec 3.4).
- Default font for this milestone: `Cascadia Mono`, 12 pt (16 px at 100% scale). Milestone 2 takes the font from the iTerm2 profile.
- Default colors: iTerm2's default profile. Milestone 2 takes them from the iTerm2 profile.
- Closing a pane never blocks the UI thread (milestone 0 finding).

## Review Focus

1. A pane is resized to 0 columns or rows (window minimized, pane squeezed). Expect: no crash; the pty keeps a size of at least 2x1. Test: Task 7 `resize_to_zero_keeps_a_minimum`.
2. A paste contains the bracketed-paste end marker. Expect: the marker is stripped, so the paste cannot break out and run commands. Test: Task 7 `bracketed_paste_strips_the_end_marker`.
3. A shell exits on its own. Expect: its pane closes, the last pane closing closes the tab, and the last tab closing exits the app. Test: Task 7 `exit_code_is_reported`, plus the close logic in Task 10.
4. A wide Unicode character or a combining mark is on screen. Expect: the scene keeps it in the row text and does not shift later cells. Test: Task 8 `wide_char_spacer_is_skipped`.
5. An app sets a very long title or many tabs are open. Expect: tab titles are cut to fit their tab. Test: Task 8 `tab_titles_are_cut_to_fit`.

---

### Task 1: Pty notify, non-blocking read, exit watch, size check

**Files:**
- Modify: `src/pty.rs`

**Interfaces:**
- Consumes: milestone 0 `pty.rs`.
- Produces (in `tandem::pty`):
  - `pub type Notify = Arc<dyn Fn() + Send + Sync>;`
  - `pub enum Read { Data(Vec<u8>), Empty, Closed }`
  - `pub fn spawn_notify(api: &Api, cmdline: &str, cols: u16, rows: u16, notify: Notify) -> Result<Pty, String>` (calls `notify` after each output chunk and once when the child exits)
  - `Pty::try_read(&self) -> Read`
  - `Pty::exit_code(&self) -> Option<u32>`
  - `spawn` and `resize` reject a size of 0 or above 32767 with an error that says "size".

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `src/pty.rs` (add `use std::sync::atomic::{AtomicUsize, Ordering};` and `use std::sync::Arc;` at the top of the test module):

```rust
    fn shipped() -> Api {
        conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap()
    }

    #[test]
    fn notify_fires_on_output_and_exit() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let p = spawn_notify(&shipped(), "cmd.exe /c echo notify-me", 80, 24, Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        assert_eq!(p.wait_exit(secs(10)), Some(0));
        let end = Instant::now() + secs(5);
        while count.load(Ordering::SeqCst) < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(count.load(Ordering::SeqCst) >= 2, "notify ran {} times", count.load(Ordering::SeqCst));
    }

    #[test]
    fn try_read_returns_output_without_blocking() {
        let p = spawn(&shipped(), "cmd.exe /q /k echo try-read", 80, 24).unwrap();
        let mut all = Vec::new();
        let end = Instant::now() + secs(10);
        while !text(&all).contains("try-read") && Instant::now() < end {
            match p.try_read() {
                Read::Data(d) => all.extend(d),
                Read::Empty => std::thread::sleep(Duration::from_millis(20)),
                Read::Closed => panic!("output closed early"),
            }
        }
        assert!(text(&all).contains("try-read"), "{:?}", text(&all));
    }

    #[test]
    fn exit_code_is_none_while_running() {
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert_eq!(p.exit_code(), None);
    }

    #[test]
    fn zero_size_is_an_error() {
        let err = spawn(&shipped(), "cmd.exe /q /k", 0, 24).err().unwrap();
        assert!(err.contains("size"), "{err}");
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert!(p.resize(80, 0).unwrap_err().contains("size"));
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib pty`
Expected: compile errors for `spawn_notify`, `Read`, `try_read`, `exit_code`.

- [ ] **Step 3: Write the code**

In `src/pty.rs`:

1. Imports. Change `use std::sync::mpsc::{self, Receiver, RecvTimeoutError};` to
   `use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};` and add `use std::sync::Arc;`.
   Add `DUPLICATE_SAME_ACCESS, DuplicateHandle` to the `windows_sys::Win32::Foundation` import.
   Add `GetCurrentProcess, INFINITE` to the `windows_sys::Win32::System::Threading` import.

2. Below the `CLOSE_WAIT` constant add:

```rust
/// Called from background threads when a pty has new output or its child exits.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The result of a non-blocking read.
pub enum Read {
    Data(Vec<u8>),
    Empty,
    /// The output pipe closed. ConPTY closes it only after the pseudoconsole closes,
    /// so a child exit shows up in `exit_code`, not here.
    Closed,
}

fn check_size(cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(format!("bad pty size {cols}x{rows}"));
    }
    Ok(())
}
```

3. Replace the `spawn` function with:

```rust
pub fn spawn(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String> {
    spawn_notify(api, cmdline, cols, rows, Arc::new(|| {}))
}

pub fn spawn_notify(api: &Api, cmdline: &str, cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    check_size(cols, rows)?;
    let (hpc, input, output, process) = unsafe { start(api, cmdline, cols, rows)? };
    let (rx, reader_done, reader) = start_reader(output, notify.clone());
    let pty = Pty { api: api.clone(), hpc, input, process, output: rx, reader: Some(reader), reader_done };
    watch_exit(pty.process, notify)?;
    Ok(pty)
}

/// Calls `notify` once the child exits. It waits on its own copy of the process
/// handle, so the Pty can close its copy at any time.
fn watch_exit(process: HANDLE, notify: Notify) -> Result<(), String> {
    let mut copy: HANDLE = null_mut();
    let ok = unsafe {
        DuplicateHandle(GetCurrentProcess(), process, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS)
    };
    if ok == 0 {
        return Err(os_err("DuplicateHandle"));
    }
    let raw = copy as usize;
    std::thread::spawn(move || {
        let copy = raw as HANDLE;
        unsafe {
            WaitForSingleObject(copy, INFINITE);
            CloseHandle(copy);
        }
        notify();
    });
    Ok(())
}
```

4. Change `start_reader` to take `notify: Notify`. After a successful `tx.send(...)` call `notify();`, and after `let _ = done_tx.send(());` call `notify();`. The loop body becomes:

```rust
            if ok == 0 || n == 0 || tx.send(buf[..n as usize].to_vec()).is_err() {
                break;
            }
            notify();
```

   In the test `reader_survives_a_zero_byte_write`, change the call to `start_reader(r, Arc::new(|| {}))`.

5. In `impl Pty`, add the size check to `resize` (first line: `check_size(cols, rows)?;`) and add:

```rust
    pub fn try_read(&self) -> Read {
        match self.output.try_recv() {
            Ok(chunk) => Read::Data(chunk),
            Err(TryRecvError::Empty) => Read::Empty,
            Err(TryRecvError::Disconnected) => Read::Closed,
        }
    }

    /// The child's exit code, or None while it runs.
    pub fn exit_code(&self) -> Option<u32> {
        self.wait_exit(Duration::ZERO)
    }
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib`
Expected: all tests pass (37 before, plus 4).

- [ ] **Step 5: Commit**

```bash
git add src/pty.rs
git commit -m "Add pty notify, non-blocking read, exit watch, and size check"
git push
```

---

### Task 2: Palette

**Files:**
- Create: `src/palette.rs`
- Modify: `src/lib.rs` (add `pub mod palette;`)

**Interfaces:**
- Produces (in `tandem::palette`):
  - `pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb` (Rgb is `alacritty_terminal::vte::ansi::Rgb`)
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct Palette { pub ansi: [Rgb; 16], pub fg, pub bg, pub bold, pub cursor, pub cursor_text, pub selection, pub selected_text: Rgb, pub bright_bold: bool }` with `Default` = iTerm2 defaults
  - `Palette::indexed(&self, i: u8) -> Rgb`, `Palette::color(&self, c: Color) -> Rgb`, `Palette::cell(&self, fg: Color, bg: Color, flags: Flags) -> (Rgb, Rgb)`

- [ ] **Step 1: Write the failing tests**

`src/palette.rs`:

```rust
//! Maps terminal colors to RGB, starting from iTerm2's default colors.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_red_is_itermish() {
        assert_eq!(Palette::default().color(Color::Named(NamedColor::Red)), rgb(0xc9, 0x1b, 0x00));
    }

    #[test]
    fn indexed_cube_and_grays() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), rgb(0, 0, 0));
        assert_eq!(p.indexed(196), rgb(255, 0, 0));
        assert_eq!(p.indexed(231), rgb(255, 255, 255));
        assert_eq!(p.indexed(232), rgb(8, 8, 8));
        assert_eq!(p.indexed(255), rgb(238, 238, 238));
    }

    #[test]
    fn dim_named_is_two_thirds() {
        assert_eq!(Palette::default().color(Color::Named(NamedColor::DimRed)), rgb(0x86, 0x12, 0x00));
    }

    #[test]
    fn bold_uses_bright_colors() {
        let p = Palette::default();
        let (fg, _) = p.cell(Color::Named(NamedColor::Red), Color::Named(NamedColor::Background), Flags::BOLD);
        assert_eq!(fg, p.ansi[9]);
        let (fg, _) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::BOLD);
        assert_eq!(fg, p.bold);
    }

    #[test]
    fn inverse_swaps_and_hidden_hides() {
        let p = Palette::default();
        let (fg, bg) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::INVERSE);
        assert_eq!((fg, bg), (p.bg, p.fg));
        let (fg, bg) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::HIDDEN);
        assert_eq!(fg, bg);
    }

    #[test]
    fn spec_colors_pass_through() {
        assert_eq!(Palette::default().color(Color::Spec(rgb(1, 2, 3))), rgb(1, 2, 3));
    }
}
```

Add `pub mod palette;` to `src/lib.rs` under `pub mod mouse;`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib palette`
Expected: compile errors (`Palette`, `rgb` not found).

- [ ] **Step 3: Write the palette**

Put above the tests in `src/palette.rs`:

```rust
use alacritty_terminal::term::cell::Flags;
pub use alacritty_terminal::vte::ansi::Rgb;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection: Rgb,
    pub selected_text: Rgb,
    /// iTerm2's "Use bright colors for bold text".
    pub bright_bold: bool,
}

impl Default for Palette {
    /// iTerm2's default profile colors.
    fn default() -> Self {
        Palette {
            ansi: [
                rgb(0x00, 0x00, 0x00), rgb(0xc9, 0x1b, 0x00), rgb(0x00, 0xc2, 0x00), rgb(0xc7, 0xc4, 0x00),
                rgb(0x02, 0x25, 0xc7), rgb(0xca, 0x30, 0xc7), rgb(0x00, 0xc5, 0xc7), rgb(0xc7, 0xc7, 0xc7),
                rgb(0x67, 0x67, 0x67), rgb(0xff, 0x6d, 0x67), rgb(0x5f, 0xf9, 0x67), rgb(0xfe, 0xfb, 0x67),
                rgb(0x68, 0x71, 0xff), rgb(0xff, 0x76, 0xff), rgb(0x5f, 0xfd, 0xff), rgb(0xff, 0xfe, 0xfe),
            ],
            fg: rgb(0xc7, 0xc7, 0xc7),
            bg: rgb(0x00, 0x00, 0x00),
            bold: rgb(0xff, 0xff, 0xff),
            cursor: rgb(0xc7, 0xc7, 0xc7),
            cursor_text: rgb(0x00, 0x00, 0x00),
            selection: rgb(0xc1, 0xde, 0xff),
            selected_text: rgb(0x00, 0x00, 0x00),
            bright_bold: true,
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    let d = |v: u8| (v as u16 * 2 / 3) as u8;
    rgb(d(c.r), d(c.g), d(c.b))
}

impl Palette {
    /// A 256-color index: 16 ANSI colors, a 6x6x6 cube, then 24 grays.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                rgb(v, v, v)
            }
        }
    }

    fn named(&self, n: NamedColor) -> Rgb {
        let i = n as usize;
        match n {
            NamedColor::Foreground => self.fg,
            NamedColor::Background => self.bg,
            NamedColor::Cursor => self.cursor,
            NamedColor::BrightForeground => self.bold,
            NamedColor::DimForeground => dim(self.fg),
            _ if i < 16 => self.ansi[i],
            _ => dim(self.ansi[i - NamedColor::DimBlack as usize]),
        }
    }

    pub fn color(&self, c: Color) -> Rgb {
        match c {
            Color::Spec(rgb) => rgb,
            Color::Indexed(i) => self.indexed(i),
            Color::Named(n) => self.named(n),
        }
    }

    /// The (foreground, background) a cell draws with.
    pub fn cell(&self, fg: Color, bg: Color, flags: Flags) -> (Rgb, Rgb) {
        let bold = flags.contains(Flags::BOLD);
        let mut f = match fg {
            Color::Named(NamedColor::Foreground) if bold => self.bold,
            Color::Named(n) if bold && self.bright_bold && (n as usize) < 8 => self.ansi[n as usize + 8],
            Color::Indexed(i) if bold && self.bright_bold && i < 8 => self.ansi[i as usize + 8],
            c => self.color(c),
        };
        if flags.contains(Flags::DIM) {
            f = dim(f);
        }
        let mut b = self.color(bg);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut f, &mut b);
        }
        if flags.contains(Flags::HIDDEN) {
            f = b;
        }
        (f, b)
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib palette`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/palette.rs src/lib.rs
git commit -m "Add palette with iTerm2 default colors"
git push
```

---

### Task 3: Screen answers queries, tracks title, resizes

**Files:**
- Modify: `src/screen.rs`

**Interfaces:**
- Consumes: `tandem::palette::Palette`.
- Produces (in `tandem::screen`):
  - `Screen::resize(&mut self, cols: usize, rows: usize)`
  - `Screen::term(&self) -> &Term<Listener>` and `Screen::term_mut(&mut self) -> &mut Term<Listener>` (make `Listener` `pub`)
  - `Screen::set_cell_px(&mut self, w: u16, h: u16)`
  - `Screen::title(&self) -> Option<&str>`
  - `Screen::palette` public field (`pub palette: Palette`)
  - `feed` also answers color queries (OSC 10/11/4) and size queries (CSI 14t)
  - `TermSize` from `term::test` is gone; a local `GridSize` implements `Dimensions`.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `src/screen.rs`:

```rust
    #[test]
    fn answers_background_color_query() {
        let mut s = Screen::new(80, 24);
        let reply = String::from_utf8(s.feed(b"\x1b]11;?\x07")).unwrap();
        assert!(reply.contains("rgb:0000/0000/0000"), "{reply:?}");
    }

    #[test]
    fn answers_text_area_size_query() {
        let mut s = Screen::new(80, 24);
        s.set_cell_px(9, 18);
        let reply = String::from_utf8(s.feed(b"\x1b[14t")).unwrap();
        assert!(reply.contains("4;432;720t"), "{reply:?}");
    }

    #[test]
    fn tracks_title() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b]0;hello\x07");
        assert_eq!(s.title(), Some("hello"));
    }

    #[test]
    fn resize_changes_the_grid() {
        let mut s = Screen::new(80, 24);
        s.resize(100, 30);
        assert_eq!((s.term().columns(), s.term().screen_lines()), (100, 30));
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib screen`
Expected: compile errors (`set_cell_px`, `title`, `resize`, `term` not found).

- [ ] **Step 3: Write the code**

Replace everything above `#[cfg(test)]` in `src/screen.rs` with:

```rust
use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;

use crate::mouse::{Modes, Tracking};
use crate::palette::Palette;

#[derive(Clone)]
pub struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Screen {
    term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
    pub palette: Palette,
    title: Option<String>,
    cell_px: (u16, u16),
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (tx, events) = mpsc::channel();
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Term::new(config, &GridSize { cols, rows }, Listener(tx));
        Self { term, parser: Processor::new(), events, palette: Palette::default(), title: None, cell_px: (0, 0) }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(GridSize { cols, rows });
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }

    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        self.cell_px = (w, h);
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feeds app output. Returns the bytes the terminal must send back, such as
    /// answers to device, color, and size queries. An app that gets no answer may hang.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, bytes);
        let mut reply = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::PtyWrite(text) => reply.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, answer) => {
                    let color = self.term.colors()[index].unwrap_or_else(|| match index {
                        0..=255 => self.palette.indexed(index as u8),
                        257 => self.palette.bg,
                        258 => self.palette.cursor,
                        _ => self.palette.fg,
                    });
                    reply.extend_from_slice(answer(color).as_bytes());
                }
                Event::TextAreaSizeRequest(answer) => {
                    let size = WindowSize {
                        num_lines: self.term.screen_lines() as u16,
                        num_cols: self.term.columns() as u16,
                        cell_width: self.cell_px.0,
                        cell_height: self.cell_px.1,
                    };
                    reply.extend_from_slice(answer(size).as_bytes());
                }
                Event::Title(title) => self.title = Some(title),
                Event::ResetTitle => self.title = None,
                _ => {}
            }
        }
        reply
    }

    pub fn mouse_modes(&self) -> Modes {
        let mode = self.term.mode();
        let tracking = if mode.contains(TermMode::MOUSE_MOTION) {
            Tracking::Any
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            Tracking::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            Tracking::Click
        } else {
            Tracking::Off
        };
        Modes { tracking, sgr: mode.contains(TermMode::SGR_MOUSE) }
    }

    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        for row in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                out.push(grid[Line(row as i32)][Column(col)].c);
            }
            out.push('\n');
        }
        out
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib`
Expected: all pass (screen adds 4).

- [ ] **Step 5: Commit**

```bash
git add src/screen.rs
git commit -m "Screen answers color and size queries, tracks the title, and resizes"
git push
```

---

### Task 4: Layout tree

**Files:**
- Create: `src/layout.rs`
- Modify: `src/lib.rs` (add `pub mod layout;`)

**Interfaces:**
- Produces (in `tandem::layout`):
  - `pub type PaneId = u64;`
  - `#[derive(Clone, Copy, Debug, PartialEq)] pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }` with `contains(&self, x: f32, y: f32) -> bool` and `inset(&self, by: f32) -> Rect`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Axis { Row, Column }` (Row = side by side)
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Dir { Left, Right, Up, Down }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct Divider { pub rect: Rect, pub path: Vec<bool>, pub axis: Axis, pub parent: Rect }`
  - `Layout::new(PaneId)`, `split(&mut self, target, Axis, new) -> bool`, `remove(&mut self, target) -> bool`, `panes(&self) -> Vec<PaneId>`, `rects(&self, area: Rect, gap: f32) -> Vec<(PaneId, Rect)>`, `dividers(&self, area: Rect, gap: f32) -> Vec<Divider>`, `set_ratio(&mut self, path: &[bool], ratio: f32)`, `neighbor(&self, from: PaneId, dir: Dir, area: Rect) -> Option<PaneId>`

- [ ] **Step 1: Write the failing tests**

`src/layout.rs`:

```rust
//! Split panes: a binary tree of panes, and the rectangles they get.

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 101.0, h: 51.0 };

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn one_pane_fills_the_area() {
        assert_eq!(Layout::new(1).rects(AREA, 1.0), vec![(1, AREA)]);
    }

    #[test]
    fn split_right_and_down() {
        let mut l = Layout::new(1);
        assert!(l.split(1, Axis::Row, 2));
        assert_eq!(l.rects(AREA, 1.0), vec![(1, r(0.0, 0.0, 50.0, 51.0)), (2, r(51.0, 0.0, 50.0, 51.0))]);
        assert!(l.split(2, Axis::Column, 3));
        assert_eq!(l.rects(AREA, 1.0)[1..], [(2, r(51.0, 0.0, 50.0, 25.0)), (3, r(51.0, 26.0, 50.0, 25.0))]);
        assert!(!l.split(9, Axis::Row, 4));
    }

    #[test]
    fn remove_gives_the_space_back() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert!(l.remove(2));
        assert_eq!(l.panes(), vec![1, 3]);
        assert_eq!(l.rects(AREA, 1.0)[1], (3, r(51.0, 0.0, 50.0, 51.0)));
        assert!(l.remove(3));
        assert_eq!(l.rects(AREA, 1.0), vec![(1, AREA)]);
    }

    #[test]
    fn cannot_remove_the_last_pane() {
        let mut l = Layout::new(1);
        assert!(!l.remove(1));
        assert!(!l.remove(7));
    }

    #[test]
    fn divider_drag_sets_the_ratio() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        let d = &l.dividers(AREA, 1.0)[0];
        assert_eq!((d.path.clone(), d.axis, d.rect), (vec![], Axis::Row, r(50.0, 0.0, 1.0, 51.0)));
        l.set_ratio(&[], 0.25);
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 25.0);
        l.set_ratio(&[], 0.0);
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 10.0);
    }

    #[test]
    fn neighbors_by_direction() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert_eq!(l.neighbor(3, Dir::Left, AREA), Some(1));
        assert_eq!(l.neighbor(2, Dir::Down, AREA), Some(3));
        assert_eq!(l.neighbor(3, Dir::Up, AREA), Some(2));
        assert_eq!(l.neighbor(1, Dir::Left, AREA), None);
    }

    #[test]
    fn rect_helpers() {
        assert!(r(0.0, 0.0, 10.0, 10.0).contains(9.9, 0.0));
        assert!(!r(0.0, 0.0, 10.0, 10.0).contains(10.0, 0.0));
        assert_eq!(r(0.0, 0.0, 10.0, 10.0).inset(2.0), r(2.0, 2.0, 6.0, 6.0));
    }
}
```

Add `pub mod layout;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib layout`
Expected: compile errors (`Layout`, `Rect` not found).

- [ ] **Step 3: Write the layout**

Put above the tests in `src/layout.rs`:

```rust
pub type PaneId = u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn inset(&self, by: f32) -> Rect {
        Rect { x: self.x + by, y: self.y + by, w: (self.w - 2.0 * by).max(0.0), h: (self.h - 2.0 * by).max(0.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side, split by a vertical line.
    Row,
    /// Stacked, split by a horizontal line.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    /// Which child to take at each split from the root: false = first, true = second.
    pub path: Vec<bool>,
    pub axis: Axis,
    /// The area of the split this divider belongs to.
    pub parent: Rect,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Leaf(PaneId),
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    root: Node,
}

/// Splits `r` into (first, second, divider).
fn split_rect(r: Rect, axis: Axis, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
    match axis {
        Axis::Row => {
            let aw = ((r.w - gap) * ratio).floor();
            (
                Rect { w: aw, ..r },
                Rect { x: r.x + aw + gap, w: r.w - aw - gap, ..r },
                Rect { x: r.x + aw, w: gap, ..r },
            )
        }
        Axis::Column => {
            let ah = ((r.h - gap) * ratio).floor();
            (
                Rect { h: ah, ..r },
                Rect { y: r.y + ah + gap, h: r.h - ah - gap, ..r },
                Rect { y: r.y + ah, h: gap, ..r },
            )
        }
    }
}

fn overlaps(a0: f32, alen: f32, b0: f32, blen: f32) -> bool {
    a0 < b0 + blen && b0 < a0 + alen
}

impl Layout {
    pub fn new(pane: PaneId) -> Self {
        Layout { root: Node::Leaf(pane) }
    }

    /// Splits `target`. `new` goes right of it (Row) or below it (Column).
    /// False if `target` is not in this layout.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        fn go(n: &mut Node, t: PaneId, axis: Axis, new: PaneId) -> bool {
            match n {
                Node::Leaf(id) if *id == t => {
                    *n = Node::Split { axis, ratio: 0.5, a: Box::new(Node::Leaf(t)), b: Box::new(Node::Leaf(new)) };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { a, b, .. } => go(a, t, axis, new) || go(b, t, axis, new),
            }
        }
        go(&mut self.root, target, axis, new)
    }

    /// Removes `target`, and its sibling takes its space. False if `target` is not
    /// here or is the last pane.
    pub fn remove(&mut self, target: PaneId) -> bool {
        fn go(n: &mut Node, t: PaneId) -> bool {
            let Node::Split { a, b, .. } = n else { return false };
            let keep = if **a == Node::Leaf(t) {
                std::mem::replace(&mut **b, Node::Leaf(0))
            } else if **b == Node::Leaf(t) {
                std::mem::replace(&mut **a, Node::Leaf(0))
            } else {
                return go(a, t) || go(b, t);
            };
            *n = keep;
            true
        }
        go(&mut self.root, target)
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.rects(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, 0.0).into_iter().map(|(id, _)| id).collect()
    }

    pub fn rects(&self, area: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
        fn go(n: &Node, r: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
            match n {
                Node::Leaf(id) => out.push((*id, r)),
                Node::Split { axis, ratio, a, b } => {
                    let (ra, rb, _) = split_rect(r, *axis, *ratio, gap);
                    go(a, ra, gap, out);
                    go(b, rb, gap, out);
                }
            }
        }
        let mut out = Vec::new();
        go(&self.root, area, gap, &mut out);
        out
    }

    pub fn dividers(&self, area: Rect, gap: f32) -> Vec<Divider> {
        fn go(n: &Node, r: Rect, gap: f32, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
            if let Node::Split { axis, ratio, a, b } = n {
                let (ra, rb, rd) = split_rect(r, *axis, *ratio, gap);
                out.push(Divider { rect: rd, path: path.clone(), axis: *axis, parent: r });
                path.push(false);
                go(a, ra, gap, path, out);
                path.pop();
                path.push(true);
                go(b, rb, gap, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(&self.root, area, gap, &mut Vec::new(), &mut out);
        out
    }

    /// Sets the split ratio at `path`, kept between 0.1 and 0.9.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        let mut n = &mut self.root;
        for &second in path {
            n = match n {
                Node::Split { a, b, .. } => {
                    if second {
                        &mut **b
                    } else {
                        &mut **a
                    }
                }
                Node::Leaf(_) => return,
            };
        }
        if let Node::Split { ratio: r, .. } = n {
            *r = ratio.clamp(0.1, 0.9);
        }
    }

    /// The nearest pane in direction `dir` from `from` that overlaps it on the other axis.
    pub fn neighbor(&self, from: PaneId, dir: Dir, area: Rect) -> Option<PaneId> {
        let rects = self.rects(area, 0.0);
        let (_, f) = *rects.iter().find(|(id, _)| *id == from)?;
        let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
        let dist = |r: &Rect| {
            let (dx, dy) = (r.x + r.w / 2.0 - cx, r.y + r.h / 2.0 - cy);
            dx * dx + dy * dy
        };
        rects
            .iter()
            .filter(|(id, r)| {
                *id != from
                    && match dir {
                        Dir::Left => r.x + r.w <= f.x + 0.5 && overlaps(r.y, r.h, f.y, f.h),
                        Dir::Right => r.x >= f.x + f.w - 0.5 && overlaps(r.y, r.h, f.y, f.h),
                        Dir::Up => r.y + r.h <= f.y + 0.5 && overlaps(r.x, r.w, f.x, f.w),
                        Dir::Down => r.y >= f.y + f.h - 0.5 && overlaps(r.x, r.w, f.x, f.w),
                    }
            })
            .min_by(|(_, a), (_, b)| dist(a).total_cmp(&dist(b)))
            .map(|(id, _)| *id)
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib layout`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/layout.rs src/lib.rs
git commit -m "Add split-pane layout tree"
git push
```

---

### Task 5: Keys and shortcuts

**Files:**
- Create: `src/keys.rs`
- Modify: `src/lib.rs` (add `pub mod keys;`)

**Interfaces:**
- Consumes: `tandem::mouse::Mods`, `tandem::layout::Dir`.
- Produces (in `tandem::keys`):
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub enum Key { Text(String), Enter, Backspace, Tab, Escape, Up, Down, Left, Right, Home, End, PageUp, PageDown, Insert, Delete, F(u8) }`
  - `pub fn encode(key: &Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>>`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Action { NewTab, ClosePane, SplitRight, SplitDown, NextPane, PrevPane, Focus(Dir), NextTab, PrevTab, SelectTab(usize), Copy, Paste, ScrollPageUp, ScrollPageDown, ScrollTop, ScrollBottom }`
  - `pub fn shortcut(key: &Key, mods: Mods) -> Option<Action>` (`key` is the key with no modifiers applied)

- [ ] **Step 1: Write the failing tests**

`src/keys.rs`:

```rust
//! Turns key presses into the bytes a terminal app expects, and maps iTerm2's
//! Cmd shortcuts (Cmd is Ctrl+Shift, Cmd+Shift is Ctrl+Shift+Alt).

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };
    const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true };
    const ALT: Mods = Mods { shift: false, alt: true, ctrl: false };
    const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false };
    const CMD: Mods = Mods { shift: true, alt: false, ctrl: true };
    const CMD_SHIFT: Mods = Mods { shift: true, alt: true, ctrl: true };

    fn t(s: &str) -> Key {
        Key::Text(s.into())
    }

    fn enc(k: Key, m: Mods) -> Vec<u8> {
        encode(&k, m, false).unwrap()
    }

    #[test]
    fn text_and_control_keys() {
        assert_eq!(enc(t("a"), NONE), b"a");
        assert_eq!(enc(t("c"), CTRL), b"\x03");
        assert_eq!(enc(t("["), CTRL), b"\x1b");
        assert_eq!(enc(t(" "), CTRL), b"\x00");
        assert_eq!(enc(t("x"), ALT), b"\x1bx");
        assert_eq!(enc(Key::Enter, NONE), b"\r");
        assert_eq!(enc(Key::Backspace, NONE), b"\x7f");
        assert_eq!(enc(Key::Backspace, CTRL), b"\x08");
        assert_eq!(enc(Key::Tab, SHIFT), b"\x1b[Z");
        assert_eq!(encode(&t(""), NONE, false), None);
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(enc(Key::Up, NONE), b"\x1b[A");
        assert_eq!(encode(&Key::Up, NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(enc(Key::Right, CTRL), b"\x1b[1;5C");
        assert_eq!(enc(Key::Home, SHIFT), b"\x1b[1;2H");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(enc(Key::PageUp, NONE), b"\x1b[5~");
        assert_eq!(enc(Key::Delete, CTRL), b"\x1b[3;5~");
        assert_eq!(enc(Key::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(4), SHIFT), b"\x1b[1;2S");
        assert_eq!(enc(Key::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE), b"\x1b[24~");
        assert_eq!(encode(&Key::F(13), NONE, false), None);
    }

    #[test]
    fn cmd_shortcuts() {
        assert_eq!(shortcut(&t("t"), CMD), Some(Action::NewTab));
        assert_eq!(shortcut(&t("T"), CMD), Some(Action::NewTab));
        assert_eq!(shortcut(&t("w"), CMD), Some(Action::ClosePane));
        assert_eq!(shortcut(&t("d"), CMD), Some(Action::SplitRight));
        assert_eq!(shortcut(&t("d"), CMD_SHIFT), Some(Action::SplitDown));
        assert_eq!(shortcut(&t("]"), CMD), Some(Action::NextPane));
        assert_eq!(shortcut(&t("]"), CMD_SHIFT), Some(Action::NextTab));
        assert_eq!(shortcut(&t("3"), CMD), Some(Action::SelectTab(2)));
        assert_eq!(shortcut(&t("c"), CMD), Some(Action::Copy));
        assert_eq!(shortcut(&t("v"), CMD), Some(Action::Paste));
        assert_eq!(shortcut(&Key::Left, CMD_SHIFT), Some(Action::Focus(Dir::Left)));
        assert_eq!(shortcut(&Key::Tab, CTRL), Some(Action::NextTab));
        assert_eq!(shortcut(&Key::PageUp, SHIFT), Some(Action::ScrollPageUp));
        assert_eq!(shortcut(&Key::Home, CMD), Some(Action::ScrollTop));
    }

    #[test]
    fn plain_keys_are_not_shortcuts() {
        assert_eq!(shortcut(&t("t"), CTRL), None);
        assert_eq!(shortcut(&t("c"), CTRL), None);
        assert_eq!(shortcut(&t("t"), NONE), None);
        assert_eq!(shortcut(&Key::PageUp, NONE), None);
    }
}
```

Add `pub mod keys;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib keys`
Expected: compile errors (`Key`, `encode`, `shortcut` not found).

- [ ] **Step 3: Write the keys module**

Put above the tests in `src/keys.rs`:

```rust
use crate::layout::Dir;
use crate::mouse::Mods;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Text(String),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

fn has_mods(m: Mods) -> bool {
    m.shift || m.alt || m.ctrl
}

/// xterm's modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn mod_param(m: Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

fn tilde(code: u8, mods: Mods) -> Vec<u8> {
    if has_mods(mods) {
        format!("\x1b[{code};{}~", mod_param(mods)).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// The bytes for a key press, or None if the key sends nothing.
pub fn encode(key: &Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let alt_prefix = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    Some(match key {
        Key::Text(s) if s.is_empty() => return None,
        Key::Text(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if mods.ctrl => alt_prefix(vec![ctrl_byte(c)?]),
                _ => alt_prefix(s.as_bytes().to_vec()),
            }
        }
        Key::Enter => alt_prefix(b"\r".to_vec()),
        Key::Backspace => alt_prefix(vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(b"\t".to_vec()),
        Key::Escape => alt_prefix(b"\x1b".to_vec()),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let c = match key {
                Key::Up => 'A',
                Key::Down => 'B',
                Key::Right => 'C',
                Key::Left => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if has_mods(mods) {
                format!("\x1b[1;{}{c}", mod_param(mods)).into_bytes()
            } else if app_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            }
        }
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::F(n @ 1..=4) => {
            let c = ['P', 'Q', 'R', 'S'][(*n - 1) as usize];
            if has_mods(mods) {
                format!("\x1b[1;{}{c}", mod_param(mods)).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(*n - 5) as usize], mods),
        Key::F(_) => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    ClosePane,
    SplitRight,
    SplitDown,
    NextPane,
    PrevPane,
    Focus(Dir),
    NextTab,
    PrevTab,
    SelectTab(usize),
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
}

/// The app shortcut for a key, if any. `key` must be the key with no modifiers
/// applied (so Ctrl+Shift+T arrives as "t" or "T").
pub fn shortcut(key: &Key, mods: Mods) -> Option<Action> {
    let cmd = mods.ctrl && mods.shift && !mods.alt;
    let cmd_shift = mods.ctrl && mods.shift && mods.alt;
    let shift_only = mods.shift && !mods.ctrl && !mods.alt;
    let text = match key {
        Key::Text(s) => s.to_lowercase(),
        _ => String::new(),
    };
    Some(match (key, text.as_str()) {
        (_, "t") if cmd => Action::NewTab,
        (_, "w") if cmd => Action::ClosePane,
        (_, "d") if cmd => Action::SplitRight,
        (_, "d") if cmd_shift => Action::SplitDown,
        (_, "]") if cmd => Action::NextPane,
        (_, "[") if cmd => Action::PrevPane,
        (_, "]") if cmd_shift => Action::NextTab,
        (_, "[") if cmd_shift => Action::PrevTab,
        (_, "c") if cmd => Action::Copy,
        (_, "v") if cmd => Action::Paste,
        (_, d) if cmd && d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9') => {
            Action::SelectTab((d.as_bytes()[0] - b'1') as usize)
        }
        (Key::Tab, _) if mods.ctrl && !mods.alt => {
            if mods.shift {
                Action::PrevTab
            } else {
                Action::NextTab
            }
        }
        (Key::Left, _) if cmd_shift => Action::Focus(Dir::Left),
        (Key::Right, _) if cmd_shift => Action::Focus(Dir::Right),
        (Key::Up, _) if cmd_shift => Action::Focus(Dir::Up),
        (Key::Down, _) if cmd_shift => Action::Focus(Dir::Down),
        (Key::PageUp, _) if shift_only => Action::ScrollPageUp,
        (Key::PageDown, _) if shift_only => Action::ScrollPageDown,
        (Key::Home, _) if cmd => Action::ScrollTop,
        (Key::End, _) if cmd => Action::ScrollBottom,
        _ => return None,
    })
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib keys`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/keys.rs src/lib.rs
git commit -m "Add key encoding and iTerm2-style shortcuts"
git push
```

---

### Task 6: Mouse decisions (clicks, selection kind, wheel)

**Files:**
- Create: `src/input.rs`
- Modify: `src/lib.rs` (add `pub mod input;`)

**Interfaces:**
- Consumes: `tandem::mouse::*`.
- Produces (in `tandem::input`):
  - `pub const MULTI_CLICK: Duration` (500 ms)
  - `#[derive(Default)] pub struct Clicks` with `press(&mut self, now: Instant, col: u32, row: u32) -> u8` (1 to 3, then back to 1)
  - `pub fn selection_type(clicks: u8, mods: Mods) -> SelectionType`
  - `#[derive(Debug, PartialEq, Eq)] pub enum Wheel { Send(Vec<u8>), Scroll(i32) }`
  - `pub fn wheel(modes: Modes, mods: Mods, alt_screen: bool, app_cursor: bool, lines: i32, col: u32, row: u32) -> Wheel` (positive lines = up)

- [ ] **Step 1: Write the failing tests**

`src/input.rs`:

```rust
//! Pure mouse decisions: click counting, the kind of selection, and what the wheel does.

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };

    #[test]
    fn clicks_count_up_to_three() {
        let mut c = Clicks::default();
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(c.press(t, 1, 1), 1);
        assert_eq!(c.press(t + ms(100), 1, 1), 2);
        assert_eq!(c.press(t + ms(200), 1, 1), 3);
        assert_eq!(c.press(t + ms(300), 1, 1), 1);
        assert_eq!(c.press(t + ms(400), 2, 1), 1);
        assert_eq!(c.press(t + ms(1500), 2, 1), 1);
    }

    #[test]
    fn selection_kinds() {
        assert_eq!(selection_type(1, NONE), SelectionType::Simple);
        assert_eq!(selection_type(2, NONE), SelectionType::Semantic);
        assert_eq!(selection_type(3, NONE), SelectionType::Lines);
        assert_eq!(selection_type(1, Mods { ctrl: true, alt: true, shift: false }), SelectionType::Block);
    }

    #[test]
    fn wheel_goes_to_a_tracking_app() {
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        assert_eq!(wheel(modes, NONE, false, false, 2, 2, 1), Wheel::Send(b"\x1b[<64;3;2M\x1b[<64;3;2M".to_vec()));
        assert_eq!(wheel(modes, NONE, false, false, -1, 0, 0), Wheel::Send(b"\x1b[<65;1;1M".to_vec()));
    }

    #[test]
    fn wheel_sends_arrows_in_the_alternate_screen() {
        let off = Modes::default();
        assert_eq!(wheel(off, NONE, true, false, 2, 0, 0), Wheel::Send(b"\x1b[A\x1b[A".to_vec()));
        assert_eq!(wheel(off, NONE, true, true, -1, 0, 0), Wheel::Send(b"\x1bOB".to_vec()));
    }

    #[test]
    fn wheel_scrolls_history_otherwise() {
        assert_eq!(wheel(Modes::default(), NONE, false, false, 3, 0, 0), Wheel::Scroll(3));
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        let alt = Mods { alt: true, ..NONE };
        assert_eq!(wheel(modes, alt, false, false, 3, 0, 0), Wheel::Scroll(3));
    }
}
```

Add `pub mod input;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib input`
Expected: compile errors (`Clicks`, `wheel` not found).

- [ ] **Step 3: Write the module**

Put above the tests in `src/input.rs`:

```rust
use std::time::{Duration, Instant};

pub use alacritty_terminal::selection::SelectionType;

use crate::mouse::{self, Action, Button, Event, Modes, Mods};

/// Windows' default double-click time.
pub const MULTI_CLICK: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Clicks {
    last: Option<(Instant, u32, u32)>,
    count: u8,
}

impl Clicks {
    /// Counts a press: 1, 2 or 3 for a single, double or triple click on one cell.
    pub fn press(&mut self, now: Instant, col: u32, row: u32) -> u8 {
        self.count = match self.last {
            Some((t, c, r)) if now.duration_since(t) <= MULTI_CLICK && (c, r) == (col, row) && self.count < 3 => {
                self.count + 1
            }
            _ => 1,
        };
        self.last = Some((now, col, row));
        self.count
    }
}

/// Double-click selects a word, triple-click a line, Ctrl+Alt-drag a rectangle.
pub fn selection_type(clicks: u8, mods: Mods) -> SelectionType {
    if mods.ctrl && mods.alt {
        return SelectionType::Block;
    }
    match clicks {
        2 => SelectionType::Semantic,
        3 => SelectionType::Lines,
        _ => SelectionType::Simple,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wheel {
    /// Bytes for the app.
    Send(Vec<u8>),
    /// Lines to scroll the history (positive is up).
    Scroll(i32),
}

/// What `lines` of wheel movement do (positive is up). A tracking app gets wheel
/// events. In the alternate screen without tracking, the wheel sends arrow keys,
/// as iTerm2 does by default. Otherwise it scrolls the history.
pub fn wheel(modes: Modes, mods: Mods, alt_screen: bool, app_cursor: bool, lines: i32, col: u32, row: u32) -> Wheel {
    let n = lines.unsigned_abs() as usize;
    if lines != 0 && mouse::app_owns_mouse(modes, mods) {
        let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
        let one = mouse::encode(modes, Event { button, action: Action::Press, mods, col, row }).unwrap_or_default();
        return Wheel::Send(one.repeat(n));
    }
    if lines != 0 && alt_screen {
        let arrow: &[u8] = match (lines > 0, app_cursor) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return Wheel::Send(arrow.repeat(n));
    }
    Wheel::Scroll(lines)
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib input`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/input.rs src/lib.rs
git commit -m "Add click counting, selection kinds, and wheel routing"
git push
```

---

### Task 7: Pane

**Files:**
- Create: `src/pane.rs`
- Modify: `src/lib.rs` (add `#[cfg(windows)] pub mod pane;`)

**Interfaces:**
- Consumes: `pty::{spawn_notify, Notify, Pty, Read}`, `Screen`, `layout::PaneId`, `conpty::Api`.
- Produces (in `tandem::pane`, Windows only):
  - `pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8>`
  - `pub struct Pane { pub id: PaneId, pub screen: Screen, pub exited: Option<u32>, pub error: Option<String>, .. }`
  - `Pane::start(id, api: &Api, cmdline: &str, cols: u16, rows: u16, notify: Notify) -> Result<Pane, String>`
  - `Pane::pump(&mut self) -> bool`, `input(&mut self, &[u8])`, `paste(&mut self, &str)`, `resize(&mut self, cols: u16, rows: u16)`, `scroll(&mut self, Scroll)`
  - Dropping a `Pane` closes its pty on another thread.

- [ ] **Step 1: Write the failing tests**

`src/pane.rs`:

```rust
//! One terminal pane: a shell in a pty, and its screen.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conpty::{self, Source, dev_vendor_dir};
    use std::time::{Duration, Instant};

    fn start(cmd: &str) -> Pane {
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        Pane::start(1, &api, cmd, 80, 24, Arc::new(|| {})).unwrap()
    }

    fn pump_until(p: &mut Pane, done: impl Fn(&Pane) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            p.pump();
            if done(p) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn plain_paste_uses_carriage_returns() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_wraps() {
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_the_end_marker() {
        assert_eq!(paste_bytes("a\x1b[201~rm -rf x\n", true), b"\x1b[200~arm -rf x\r\x1b[201~");
    }

    #[test]
    fn pump_shows_output() {
        let mut p = start("cmd.exe /q /k echo pane-ok");
        assert!(pump_until(&mut p, |p| p.screen.text().contains("pane-ok")), "{}", p.screen.text());
    }

    #[test]
    fn exit_code_is_reported() {
        let mut p = start("cmd.exe /c exit 4");
        assert!(pump_until(&mut p, |p| p.exited.is_some()));
        assert_eq!(p.exited, Some(4));
    }

    #[test]
    fn resize_to_zero_keeps_a_minimum() {
        let mut p = start("cmd.exe /q /k");
        p.resize(0, 0);
        assert_eq!(p.error, None);
        assert_eq!((p.screen.term().columns(), p.screen.term().screen_lines()), (2, 1));
    }

    #[test]
    fn drop_does_not_block() {
        let p = start("cmd.exe /q /k");
        let t = Instant::now();
        drop(p);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    }
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod pane;
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib pane`
Expected: compile errors (`Pane`, `paste_bytes` not found).

- [ ] **Step 3: Write the pane**

Put above the tests in `src/pane.rs`:

```rust
use std::sync::Arc;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;

use crate::conpty::Api;
use crate::layout::PaneId;
use crate::pty::{self, Notify, Pty, Read};
use crate::screen::Screen;

/// The bytes to send for a paste. Newlines become carriage returns, as a typed
/// Enter would. Bracketed paste wraps the text so the app knows it was pasted,
/// and drops any end marker inside it so a paste cannot break out and run commands.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text.into_bytes();
    }
    format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", "")).into_bytes()
}

pub struct Pane {
    pub id: PaneId,
    pub screen: Screen,
    pty: Option<Pty>,
    pub exited: Option<u32>,
    pub error: Option<String>,
}

impl Pane {
    pub fn start(id: PaneId, api: &Api, cmdline: &str, cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = pty::spawn_notify(api, cmdline, cols, rows, notify)?;
        Ok(Pane { id, screen: Screen::new(cols as usize, rows as usize), pty: Some(pty), exited: None, error: None })
    }

    /// Feeds all waiting output into the screen. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let mut changed = false;
        while let Read::Data(chunk) = pty.try_read() {
            changed = true;
            let reply = self.screen.feed(&chunk);
            if !reply.is_empty() {
                if let Err(e) = pty.write(&reply) {
                    self.error = Some(e);
                }
            }
        }
        if self.exited.is_none() {
            if let Some(code) = pty.exit_code() {
                self.exited = Some(code);
                changed = true;
            }
        }
        changed
    }

    pub fn input(&mut self, bytes: &[u8]) {
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.write(bytes) {
                self.error = Some(e);
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.screen.term().mode().contains(TermMode::BRACKETED_PASTE);
        self.input(&paste_bytes(text, bracketed));
    }

    /// Resizes the screen and the pty. A pane squeezed to nothing keeps 2x1.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (self.screen.term().columns(), self.screen.term().screen_lines()) == (cols as usize, rows as usize) {
            return;
        }
        self.screen.resize(cols as usize, rows as usize);
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.resize(cols, rows) {
                self.error = Some(e);
            }
        }
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.screen.term_mut().scroll_display(scroll);
    }
}

impl Drop for Pane {
    /// Closing a pty can take about 2 s on the Windows copy of ConPTY (milestone 0),
    /// so it happens on its own thread.
    fn drop(&mut self) {
        if let Some(pty) = self.pty.take() {
            std::thread::spawn(move || drop(pty));
        }
    }
}
```

Note: `use std::sync::Arc;` is used by the tests through `super::*`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib pane`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/pane.rs src/lib.rs
git commit -m "Add pane joining a pty and a screen"
git push
```

---

### Task 8: Scene (what to draw)

**Files:**
- Create: `src/scene.rs`
- Modify: `src/lib.rs` (add `pub mod scene;`)

**Interfaces:**
- Consumes: `Screen`, `Palette`, `layout::{Rect, Divider}`.
- Produces (in `tandem::scene`):
  - `#[derive(Clone, Copy, Debug, PartialEq)] pub struct Quad { pub rect: Rect, pub color: [f32; 4] }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct Span { pub text: String, pub color: Rgb, pub bold: bool, pub italic: bool }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct TextRow { pub x: f32, pub y: f32, pub spans: Vec<Span>, pub clip: Rect }`
  - `#[derive(Clone, Debug, Default, PartialEq)] pub struct Frame { pub clear: [f32; 4], pub quads: Vec<Quad>, pub rows: Vec<TextRow>, pub overlay: Vec<Quad> }` (draw order: quads, rows, overlay)
  - `pub fn rgba(c: Rgb, a: f32) -> [f32; 4]` (linear, for an sRGB target)
  - `pub fn pane(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool)`
  - `pub fn tab_bar(frame: &mut Frame, titles: &[String], active: usize, area: Rect, cell: (f32, f32)) -> Vec<Rect>`
  - `pub fn dividers(frame: &mut Frame, dividers: &[Divider])`
  - `pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32))`

- [ ] **Step 1: Write the failing tests**

`src/scene.rs`:

```rust
//! What to draw, as plain data: rectangles and rows of text. The app builds a
//! Frame from its panes, and the renderer draws it.

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};

    const CELL: (f32, f32) = (10.0, 20.0);
    const RECT: Rect = Rect { x: 0.0, y: 0.0, w: 800.0, h: 480.0 };

    fn screen(bytes: &[u8]) -> Screen {
        let mut s = Screen::new(80, 24);
        s.feed(bytes);
        s
    }

    fn row_text(f: &Frame, i: usize) -> String {
        f.rows[i].spans.iter().map(|s| s.text.as_str()).collect::<String>()
    }

    #[test]
    fn a_red_background_makes_a_quad() {
        let s = screen(b"\x1b[41mX");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true);
        let red = rgba(s.palette.ansi[1], 1.0);
        assert!(f.quads.iter().any(|q| q.color == red && q.rect == Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn rows_carry_the_text() {
        let s = screen(b"hello\r\nworld");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true);
        assert_eq!(row_text(&f, 0).trim_end(), "hello");
        assert_eq!(row_text(&f, 1).trim_end(), "world");
        assert_eq!((f.rows[1].x, f.rows[1].y), (0.0, 20.0));
    }

    #[test]
    fn focused_block_cursor_is_a_quad() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true);
        let cursor = rgba(s.palette.cursor, 1.0);
        assert!(f.quads.iter().any(|q| q.color == cursor && q.rect.x == 20.0 && q.rect.y == 0.0));
    }

    #[test]
    fn unfocused_pane_is_dimmed_and_cursor_hollow() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false);
        assert!(f.overlay.iter().any(|q| q.rect == RECT));
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn selection_uses_selection_colors() {
        let mut s = screen(b"hello");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true);
        let sel = rgba(s.palette.selection, 1.0);
        assert!(f.quads.iter().any(|q| q.color == sel && q.rect == Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn wide_char_spacer_is_skipped() {
        let s = screen("\u{4e2d}x".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true);
        assert!(row_text(&f, 0).starts_with("\u{4e2d}x"), "{:?}", row_text(&f, 0));
    }

    #[test]
    fn tab_titles_are_cut_to_fit() {
        let mut f = Frame::default();
        let titles = vec!["a very long title that cannot fit".to_string(), "b".to_string()];
        let rects = tab_bar(&mut f, &titles, 0, Rect { x: 0.0, y: 0.0, w: 200.0, h: 30.0 }, CELL);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].x, 100.0);
        let first: String = f.rows[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert!(first.chars().count() <= 8, "{first:?}");
    }
}
```

Add `pub mod scene;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib scene`
Expected: compile errors (`Frame`, `pane` not found).

- [ ] **Step 3: Write the scene**

Put above the tests in `src/scene.rs`:

```rust
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::CursorShape;

use crate::layout::{Divider, Rect};
use crate::palette::{Rgb, rgb};
use crate::screen::Screen;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub rect: Rect,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub x: f32,
    pub y: f32,
    pub spans: Vec<Span>,
    pub clip: Rect,
}

/// Draw order: `quads`, then `rows`, then `overlay`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: [f32; 4],
    pub quads: Vec<Quad>,
    pub rows: Vec<TextRow>,
    pub overlay: Vec<Quad>,
}

const TAB_BAR: Rgb = rgb(0x1e, 0x1e, 0x1e);
const TAB_ACTIVE: Rgb = rgb(0x3a, 0x3a, 0x3a);
const TAB_TEXT: Rgb = rgb(0xc7, 0xc7, 0xc7);
const TAB_TEXT_DIM: Rgb = rgb(0x8a, 0x8a, 0x8a);
const DIVIDER: Rgb = rgb(0x4a, 0x4a, 0x4a);
const BANNER: Rgb = rgb(0x7a, 0x1e, 0x1e);
/// How much an unfocused pane fades toward the background, as iTerm2 dims inactive splits.
const DIM_ALPHA: f32 = 0.4;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A color for the quad shader. The target is sRGB, so the shader works in linear light.
pub fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    [srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), a]
}

struct CellLook {
    text: String,
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
}

/// Draws one pane's visible screen into `rect`.
pub fn pane(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool) {
    let (cw, ch) = cell;
    let palette = &screen.palette;
    let term = screen.term();
    let (cols, lines) = (term.columns(), term.screen_lines());
    let content = term.renderable_content();
    let cursor = point_to_viewport(content.display_offset, content.cursor.point)
        .filter(|_| content.cursor.shape != CursorShape::Hidden)
        .map(|p| (p.line, p.column.0));
    let block_cursor = focused && content.cursor.shape == CursorShape::Block;

    let blank = || CellLook { text: " ".into(), fg: palette.fg, bg: palette.bg, bold: false, italic: false };
    let mut grid: Vec<Vec<CellLook>> = (0..lines).map(|_| (0..cols).map(|_| blank()).collect()).collect();
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(content.display_offset, indexed.point) else { continue };
        let (row, col) = (vp.line, vp.column.0);
        if row >= lines || col >= cols {
            continue;
        }
        let c = indexed.cell;
        let (mut fg, mut bg) = palette.cell(c.fg, c.bg, c.flags);
        if content.selection.as_ref().is_some_and(|s| s.contains(indexed.point)) {
            (fg, bg) = (palette.selected_text, palette.selection);
        }
        if block_cursor && cursor == Some((row, col)) {
            (fg, bg) = (palette.cursor_text, palette.cursor);
        }
        let mut text = String::new();
        if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            text.push(c.c);
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
        }
        grid[row][col] = CellLook {
            text,
            fg,
            bg,
            bold: c.flags.contains(Flags::BOLD),
            italic: c.flags.contains(Flags::ITALIC),
        };
    }

    for (row, cells) in grid.iter().enumerate() {
        let y = rect.y + row as f32 * ch;
        // Background runs.
        let mut col = 0;
        while col < cols {
            let bg = cells[col].bg;
            let start = col;
            while col < cols && cells[col].bg == bg {
                col += 1;
            }
            if bg != palette.bg {
                let r = Rect { x: rect.x + start as f32 * cw, y, w: (col - start) as f32 * cw, h: ch };
                frame.quads.push(Quad { rect: r, color: rgba(bg, 1.0) });
            }
        }
        // Text spans.
        let mut spans: Vec<Span> = Vec::new();
        for c in cells {
            match spans.last_mut() {
                Some(s) if s.color == c.fg && s.bold == c.bold && s.italic == c.italic => s.text.push_str(&c.text),
                _ => spans.push(Span { text: c.text.clone(), color: c.fg, bold: c.bold, italic: c.italic }),
            }
        }
        while spans.last().is_some_and(|s| s.text.trim_end().is_empty()) {
            spans.pop();
        }
        if !spans.is_empty() {
            frame.rows.push(TextRow { x: rect.x, y, spans, clip: rect });
        }
    }

    if let Some((row, col)) = cursor {
        let r = Rect { x: rect.x + col as f32 * cw, y: rect.y + row as f32 * ch, w: cw, h: ch };
        let color = rgba(palette.cursor, 1.0);
        let thin = 2.0_f32.min(cw);
        match content.cursor.shape {
            CursorShape::Beam => frame.overlay.push(Quad { rect: Rect { w: thin, ..r }, color }),
            CursorShape::Underline => frame.overlay.push(Quad { rect: Rect { y: r.y + r.h - thin, h: thin, ..r }, color }),
            _ if !block_cursor => {
                for edge in [
                    Rect { h: 1.0, ..r },
                    Rect { y: r.y + r.h - 1.0, h: 1.0, ..r },
                    Rect { w: 1.0, ..r },
                    Rect { x: r.x + r.w - 1.0, w: 1.0, ..r },
                ] {
                    frame.overlay.push(Quad { rect: edge, color });
                }
            }
            _ => {}
        }
    }

    if !focused {
        frame.overlay.push(Quad { rect, color: rgba(palette.bg, DIM_ALPHA) });
    }
}

/// Draws the tab bar across `area`. Returns each tab's rectangle, for clicks.
pub fn tab_bar(frame: &mut Frame, titles: &[String], active: usize, area: Rect, cell: (f32, f32)) -> Vec<Rect> {
    frame.quads.push(Quad { rect: area, color: rgba(TAB_BAR, 1.0) });
    let n = titles.len().max(1) as f32;
    let w = (area.w / n).floor();
    let mut rects = Vec::new();
    for (i, title) in titles.iter().enumerate() {
        let r = Rect { x: area.x + i as f32 * w, y: area.y, w, h: area.h };
        if i == active {
            frame.quads.push(Quad { rect: r, color: rgba(TAB_ACTIVE, 1.0) });
        }
        let fit = ((w / cell.0) as usize).saturating_sub(2);
        let text: String = title.chars().take(fit).collect();
        let color = if i == active { TAB_TEXT } else { TAB_TEXT_DIM };
        let y = r.y + ((r.h - cell.1) / 2.0).floor();
        frame.rows.push(TextRow { x: r.x + cell.0, y, spans: vec![Span { text, color, bold: false, italic: false }], clip: r });
        rects.push(r);
    }
    rects
}

pub fn dividers(frame: &mut Frame, dividers: &[Divider]) {
    for d in dividers {
        frame.overlay.push(Quad { rect: d.rect, color: rgba(DIVIDER, 1.0) });
    }
}

/// A one-line error bar along the bottom of `area`. Its box goes in `quads`, so its
/// text (in `rows`) draws on top.
pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) {
    let r = Rect { x: area.x, y: area.y + area.h - cell.1 - 8.0, w: area.w, h: cell.1 + 8.0 };
    frame.quads.push(Quad { rect: r, color: rgba(BANNER, 1.0) });
    let span = Span { text: text.to_string(), color: rgb(0xff, 0xff, 0xff), bold: false, italic: false };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span], clip: r });
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib scene`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/scene.rs src/lib.rs
git commit -m "Add scene building: cells, cursor, selection, tabs, dividers"
git push
```

---

### Task 9: Renderer (wgpu and glyphon), tested offscreen

**Files:**
- Create: `src/render.rs`, `src/quad.wgsl`, `build.rs`
- Modify: `Cargo.toml`, `src/lib.rs` (add `#[cfg(windows)] pub mod render;`)

**Interfaces:**
- Consumes: `scene::{Frame, Quad, TextRow}`.
- Produces (in `tandem::render`):
  - `pub struct Renderer` with `pub cell: (f32, f32)`
  - `Renderer::new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, family: &str, font_px: f32) -> Renderer`
  - `Renderer::set_font_px(&mut self, px: f32)`
  - `Renderer::draw(&mut self, frame: &Frame, view: &wgpu::TextureView, width: u32, height: u32) -> Result<(), String>`
  - `build.rs` copies `vendor/conpty/*` next to the built exe.

- [ ] **Step 1: Add dependencies and the build script**

Append to `[dependencies]` in `Cargo.toml`:

```toml
arboard = "3"
bytemuck = "1"
glyphon = "0.12"
pollster = "0.4"
wgpu = "30"
winit = "0.30"
```

Add `"Win32_UI_WindowsAndMessaging",` to the `windows-sys` features list.

`build.rs`:

```rust
//! Copies the shipped ConPTY next to the built exe, where the app looks for it.
fn main() {
    println!("cargo:rerun-if-changed=vendor/conpty");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out; the exe is in target/<profile>.
    let profile_dir = out.ancestors().nth(3).unwrap();
    for file in ["conpty.dll", "OpenConsole.exe"] {
        let src = std::path::Path::new("vendor").join("conpty").join(file);
        if src.exists() {
            std::fs::copy(&src, profile_dir.join(file)).unwrap();
        } else {
            println!("cargo:warning=vendor/conpty/{file} is missing. Run scripts/fetch-conpty.ps1.");
        }
    }
}
```

- [ ] **Step 2: Write the failing test**

`src/render.rs`:

```rust
//! Draws a scene Frame: rectangles with a small wgpu pipeline, text with glyphon.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use crate::palette::rgb;
    use crate::scene::{Quad, Span, TextRow, rgba};

    fn device() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
    }

    fn read(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().to_vec();
        data
    }

    #[test]
    fn draws_quads_and_text_offscreen() {
        let (device, queue) = device();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut r = Renderer::new(&device, &queue, format, "Cascadia Mono", 24.0);
        assert!(r.cell.0 > 5.0 && r.cell.1 > 10.0, "{:?}", r.cell);
        let (w, h) = (64u32, 32u32);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let frame = Frame {
            clear: rgba(rgb(0, 0, 0), 1.0),
            quads: vec![Quad { rect: Rect { x: 0.0, y: 0.0, w: 32.0, h: 32.0 }, color: rgba(rgb(255, 0, 0), 1.0) }],
            rows: vec![TextRow {
                x: 36.0,
                y: 0.0,
                spans: vec![Span { text: "\u{2588}".into(), color: rgb(255, 255, 255), bold: false, italic: false }],
                clip: Rect { x: 32.0, y: 0.0, w: 32.0, h: 32.0 },
            }],
            overlay: vec![],
        };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 3) as usize];
        assert_eq!(at(4, 4), &[255, 0, 0]);
        let white = (32..64).flat_map(|x| (0..32).map(move |y| (x, y))).any(|(x, y)| at(x, y)[0] > 200 && at(x, y)[2] > 200);
        assert!(white, "no text pixels drawn");
    }
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod render;
```

- [ ] **Step 3: Run the test to see it fail**

Run: `scripts/xt test --lib render`
Expected: compile errors (`Renderer` not found).

- [ ] **Step 4: Write the shader and the renderer**

`src/quad.wgsl`:

```wgsl
struct Screen {
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;

struct In {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(v: In) -> Out {
    var o: Out;
    let p = v.pos / screen.size * 2.0 - 1.0;
    o.pos = vec4<f32>(p.x, -p.y, 0.0, 1.0);
    o.color = v.color;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}
```

Put above the tests in `src/render.rs`:

```rust
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use wgpu::util::DeviceExt;

use crate::scene::{Frame, Quad, Span};

/// Rectangles: two triangles each, position and color per vertex.
struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl QuadPipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 6 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        QuadPipeline { pipeline, uniform, bind_group }
    }

    fn vertices(quads: &[Quad]) -> Vec<f32> {
        let mut v = Vec::with_capacity(quads.len() * 36);
        for q in quads {
            let (x0, y0, x1, y1) = (q.rect.x, q.rect.y, q.rect.x + q.rect.w, q.rect.y + q.rect.h);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x0, y1), (x1, y0), (x1, y1)] {
                v.extend_from_slice(&[x, y]);
                v.extend_from_slice(&q.color);
            }
        }
        v
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    quads: QuadPipeline,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text: TextRenderer,
    /// One text buffer per row slot, with the spans it last shaped.
    buffers: Vec<(Buffer, Vec<Span>, f32)>,
    family: String,
    font_px: f32,
    pub cell: (f32, f32),
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, family: &str, font_px: f32) -> Renderer {
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let mut r = Renderer {
            device: device.clone(),
            queue: queue.clone(),
            quads: QuadPipeline::new(device, format),
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
            atlas,
            viewport,
            text,
            buffers: Vec::new(),
            family: family.to_string(),
            font_px,
            cell: (0.0, 0.0),
        };
        r.set_font_px(font_px);
        r
    }

    /// Sets the font size and measures the cell: the advance of "M" by 1.2 lines.
    pub fn set_font_px(&mut self, px: f32) {
        self.font_px = px;
        let line = (px * 1.2).round();
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px, line));
        b.set_text("M", &Attrs::new().family(Family::Name(&self.family)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let w = b.layout_runs().next().and_then(|r| r.glyphs.first().map(|g| g.w)).unwrap_or(px * 0.6);
        self.cell = (w.round().max(1.0), line);
        self.buffers.clear();
    }

    pub fn draw(&mut self, frame: &Frame, view: &wgpu::TextureView, width: u32, height: u32) -> Result<(), String> {
        let (cw, ch) = self.cell;
        let metrics = Metrics::new(self.font_px, ch);
        while self.buffers.len() < frame.rows.len() {
            self.buffers.push((Buffer::new(&mut self.font_system, metrics), Vec::new(), 0.0));
        }
        for (slot, row) in self.buffers.iter_mut().zip(&frame.rows) {
            let (buffer, last, last_w) = slot;
            if *last == row.spans && *last_w == row.clip.w {
                continue;
            }
            buffer.set_metrics_and_size(metrics, Some(row.clip.w + cw), Some(ch));
            buffer.set_wrap(Wrap::None);
            buffer.set_monospace_width(Some(cw));
            let base = Attrs::new().family(Family::Name(&self.family));
            let spans = row.spans.iter().map(|s| {
                let attrs = base
                    .clone()
                    .color(Color::rgb(s.color.r, s.color.g, s.color.b))
                    .weight(if s.bold { Weight::BOLD } else { Weight::NORMAL })
                    .style(if s.italic { Style::Italic } else { Style::Normal });
                (s.text.as_str(), attrs)
            });
            buffer.set_rich_text(spans, &base, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            *last = row.spans.clone();
            *last_w = row.clip.w;
        }

        self.viewport.update(&self.queue, Resolution { width, height });
        let areas = self.buffers.iter().zip(&frame.rows).map(|((buffer, _, _), row)| TextArea {
            buffer,
            left: row.x,
            top: row.y,
            scale: 1.0,
            bounds: TextBounds {
                left: row.clip.x as i32,
                top: row.clip.y as i32,
                right: (row.clip.x + row.clip.w) as i32,
                bottom: (row.clip.y + row.clip.h) as i32,
            },
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &[],
        });
        self.text
            .prepare(&self.device, &self.queue, &mut self.font_system, &mut self.atlas, &self.viewport, areas, &mut self.swash)
            .map_err(|e| format!("text prepare: {e:?}"))?;

        self.queue.write_buffer(&self.quads.uniform, 0, bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]));
        let under = QuadPipeline::vertices(&frame.quads);
        let over = QuadPipeline::vertices(&frame.overlay);
        let make = |v: &[f32]| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let (under_buf, over_buf) = (make(&under), make(&over));

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let [r, g, b, a] = frame.clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.quads.pipeline);
            pass.set_bind_group(0, &self.quads.bind_group, &[]);
            if !under.is_empty() {
                pass.set_vertex_buffer(0, under_buf.slice(..));
                pass.draw(0..(under.len() / 6) as u32, 0..1);
            }
            self.text.render(&self.atlas, &self.viewport, &mut pass).map_err(|e| format!("text render: {e:?}"))?;
            if !over.is_empty() {
                pass.set_pipeline(&self.quads.pipeline);
                pass.set_bind_group(0, &self.quads.bind_group, &[]);
                pass.set_vertex_buffer(0, over_buf.slice(..));
                pass.draw(0..(over.len() / 6) as u32, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.atlas.trim();
        Ok(())
    }
}
```

- [ ] **Step 5: Run the test to see it pass**

Run: `scripts/xt test --lib render`
Expected: `1 passed`. Then run `scripts/xt test --lib` and expect everything to pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml build.rs src/quad.wgsl src/render.rs src/lib.rs
git commit -m "Add wgpu and glyphon renderer with an offscreen test"
git push
```

---

### Task 10: The app window

**Files:**
- Create: `src/app.rs`, `src/bin/tandem.rs`
- Modify: `src/lib.rs` (add `#[cfg(windows)] pub mod app;`)

**Interfaces:**
- Consumes: everything above.
- Produces: `tandem::app::run()`, and the `tandem` binary.

This task is glue between winit and the tested modules. Its test is a clean release build plus the pure-logic tests it relies on. Robert runs the acceptance checklist in Task 11.

- [ ] **Step 1: Write the app**

`src/app.rs`:

```rust
//! The Tandem window: tabs of split panes, drawn with wgpu.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::{TermMode, viewport_to_point};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::conpty::{self, Api, Source};
use crate::input::{self, Clicks, Wheel};
use crate::keys::{self, Action, Key};
use crate::layout::{Axis, Dir, Layout, PaneId, Rect};
use crate::mouse::{self, Button, Mods, Tracking};
use crate::pane::Pane;
use crate::pty::Notify;
use crate::render::Renderer;
use crate::scene::{self, Frame, rgba};

const FONT: &str = "Cascadia Mono";
/// 12 pt, iTerm2's default size, at 96 dpi.
const FONT_PX: f32 = 16.0;
/// Space between a pane's edge and its text, and the divider width, in pixels at 100%.
const PAD: f32 = 4.0;
const GAP: f32 = 1.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f32 = 3.0;

pub enum UserEvent {
    Wake,
}

struct Tab {
    layout: Layout,
    focused: PaneId,
}

enum Drag {
    None,
    Select { pane: PaneId },
    Divider { path: Vec<bool>, axis: Axis, parent: Rect },
    App { pane: PaneId, button: Button },
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    instance: wgpu::Instance,
    // Dropped last, after the surface.
    window: Arc<Window>,
}

pub struct App {
    notify: Notify,
    api: Api,
    shell: String,
    gpu: Option<Gpu>,
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: PaneId,
    mods: Mods,
    cursor: (f32, f32),
    drag: Drag,
    clicks: Clicks,
    wheel_acc: f32,
    focused: bool,
    tab_rects: Vec<Rect>,
    banner: Option<String>,
    clipboard: Option<arboard::Clipboard>,
}

/// pwsh.exe when it is on the PATH, else Windows PowerShell.
fn default_shell() -> String {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("pwsh.exe").is_file()))
        .unwrap_or(false);
    if found { "pwsh.exe -NoLogo".into() } else { "powershell.exe -NoLogo".into() }
}

fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONERROR) };
}

pub fn run() {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => return message_box(&format!("Could not start the event loop: {e}")),
    };
    let proxy: Mutex<EventLoopProxy<UserEvent>> = Mutex::new(event_loop.create_proxy());
    let notify: Notify = Arc::new(move || {
        let _ = proxy.lock().unwrap().send_event(UserEvent::Wake);
    });

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let (api, banner) = match conpty::load(&Source::Shipped(exe_dir)) {
        Ok(api) => (api, None),
        Err(e) => match conpty::load(&Source::Inbox) {
            Ok(api) => (api, Some(format!("Shipped ConPTY did not load ({e}). Using the Windows copy."))),
            Err(e2) => return message_box(&format!("No ConPTY: {e}; {e2}")),
        },
    };

    let mut app = App {
        notify,
        api,
        shell: default_shell(),
        gpu: None,
        tabs: Vec::new(),
        active: 0,
        panes: HashMap::new(),
        next_id: 1,
        mods: Mods::default(),
        cursor: (0.0, 0.0),
        drag: Drag::None,
        clicks: Clicks::default(),
        wheel_acc: 0.0,
        focused: true,
        tab_rects: Vec::new(),
        banner,
        clipboard: arboard::Clipboard::new().ok(),
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        message_box(&format!("Tandem stopped: {e}"));
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    fn cell(&self) -> (f32, f32) {
        self.gpu.as_ref().map_or((8.0, 16.0), |g| g.renderer.cell)
    }

    fn tab_height(&self) -> f32 {
        if self.tabs.len() > 1 { (self.cell().1 * 1.6).round() } else { 0.0 }
    }

    /// The area the active tab's panes share.
    fn content_area(&self) -> Rect {
        let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let top = self.tab_height();
        let bottom = if self.banner.is_some() { self.cell().1 + 8.0 } else { 0.0 };
        Rect { x: 0.0, y: top, w, h: (h - top - bottom).max(1.0) }
    }

    /// The text area inside a pane rectangle.
    fn inner(&self, rect: Rect) -> Rect {
        rect.inset(PAD * self.scale())
    }

    fn grid_size(&self, rect: Rect) -> (u16, u16) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        ((inner.w / cw).floor().max(2.0) as u16, (inner.h / ch).floor().max(1.0) as u16)
    }

    fn start_pane(&mut self, rect: Rect) -> Option<PaneId> {
        let (cols, rows) = self.grid_size(rect);
        let id = self.next_id;
        match Pane::start(id, &self.api, &self.shell, cols, rows, self.notify.clone()) {
            Ok(pane) => {
                self.next_id += 1;
                self.panes.insert(id, pane);
                Some(id)
            }
            Err(e) => {
                self.banner = Some(format!("Could not start {}: {e}", self.shell));
                None
            }
        }
    }

    fn new_tab(&mut self) {
        let area = self.content_area();
        if let Some(id) = self.start_pane(area) {
            self.tabs.push(Tab { layout: Layout::new(id), focused: id });
            self.active = self.tabs.len() - 1;
            self.relayout();
        }
    }

    fn split(&mut self, axis: Axis) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let target = tab.focused;
        let area = self.content_area();
        let Some((_, rect)) = tab.layout.rects(area, GAP).into_iter().find(|(id, _)| *id == target) else { return };
        let half = match axis {
            Axis::Row => Rect { w: rect.w / 2.0, ..rect },
            Axis::Column => Rect { h: rect.h / 2.0, ..rect },
        };
        if let Some(id) = self.start_pane(half) {
            let tab = &mut self.tabs[self.active];
            tab.layout.split(target, axis, id);
            tab.focused = id;
            self.relayout();
        }
    }

    /// Closes a pane. The last pane closes its tab, and the last tab exits.
    fn close_pane(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        self.panes.remove(&id);
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            let tab = &mut self.tabs[t];
            if tab.layout.remove(id) {
                if tab.focused == id {
                    tab.focused = tab.layout.panes()[0];
                }
            } else {
                self.tabs.remove(t);
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
            }
        }
        if self.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        self.relayout();
    }

    fn relayout(&mut self) {
        let area = self.content_area();
        let (cw, ch) = self.cell();
        let sizes: Vec<(PaneId, (u16, u16))> = self
            .tabs
            .iter()
            .flat_map(|t| t.layout.rects(area, GAP))
            .map(|(id, rect)| (id, self.grid_size(rect)))
            .collect();
        for (id, (cols, rows)) in sizes {
            if let Some(p) = self.panes.get_mut(&id) {
                p.resize(cols, rows);
                p.screen.set_cell_px(cw as u16, ch as u16);
            }
        }
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn focused_pane(&mut self) -> Option<&mut Pane> {
        let id = self.tabs.get(self.active)?.focused;
        self.panes.get_mut(&id)
    }

    fn pane_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect)> {
        let tab = self.tabs.get(self.active)?;
        tab.layout.rects(self.content_area(), GAP).into_iter().find(|(_, r)| r.contains(x, y))
    }

    /// The cell under (x, y) in pane `rect`, and which half of the cell it is in.
    fn cell_at(&self, id: PaneId, rect: Rect, x: f32, y: f32) -> (u32, u32, Side) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        let term = self.panes[&id].screen.term();
        let (col, row) = mouse::cell_at((x - inner.x) as f64, (y - inner.y) as f64, cw as f64, ch as f64, term.columns() as u32, term.screen_lines() as u32);
        let side = if (x - inner.x) - col as f32 * cw > cw / 2.0 { Side::Right } else { Side::Left };
        (col, row, side)
    }

    fn grid_point(&self, id: PaneId, col: u32, row: u32) -> Point {
        let offset = self.panes[&id].screen.term().grid().display_offset();
        viewport_to_point(offset, Point::new(row as usize, Column(col as usize)))
    }

    fn copy_selection(&mut self, id: PaneId) {
        let Some(text) = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()) else { return };
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banner = Some(format!("Copy failed: {e}"));
            }
        }
    }

    fn paste(&mut self) {
        let text = match self.clipboard.as_mut().map(|cb| cb.get_text()) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                self.banner = Some(format!("Paste failed: {e}"));
                return self.redraw();
            }
            None => return,
        };
        if let Some(p) = self.focused_pane() {
            p.paste(&text);
        }
    }

    fn action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let n = self.tabs.len();
        match action {
            Action::NewTab => self.new_tab(),
            Action::ClosePane => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.close_pane(id, event_loop);
                }
            }
            Action::SplitRight => self.split(Axis::Row),
            Action::SplitDown => self.split(Axis::Column),
            Action::NextPane | Action::PrevPane => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let panes = tab.layout.panes();
                    let i = panes.iter().position(|p| *p == tab.focused).unwrap_or(0);
                    let step = if action == Action::NextPane { 1 } else { panes.len() - 1 };
                    tab.focused = panes[(i + step) % panes.len()];
                }
            }
            Action::Focus(dir) => {
                let area = self.content_area();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(id) = tab.layout.neighbor(tab.focused, dir, area) {
                        tab.focused = id;
                    }
                }
            }
            Action::NextTab if n > 0 => self.active = (self.active + 1) % n,
            Action::PrevTab if n > 0 => self.active = (self.active + n - 1) % n,
            Action::SelectTab(i) if i < n => self.active = i,
            Action::Copy => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.copy_selection(id);
                }
            }
            Action::Paste => self.paste(),
            Action::ScrollPageUp => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageUp)),
            Action::ScrollPageDown => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageDown)),
            Action::ScrollTop => self.focused_pane().map_or((), |p| p.scroll(Scroll::Top)),
            Action::ScrollBottom => self.focused_pane().map_or((), |p| p.scroll(Scroll::Bottom)),
            _ => {}
        }
        self.relayout();
    }

    fn key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) {
        if ev.state != ElementState::Pressed {
            return;
        }
        if let Some(base) = to_key(&ev.key_without_modifiers(), None) {
            if let Some(action) = keys::shortcut(&base, self.mods) {
                return self.action(action, event_loop);
            }
        }
        let Some(key) = to_key(&ev.logical_key, ev.text.as_deref()) else { return };
        let mods = self.mods;
        let Some(pane) = self.focused_pane() else { return };
        let app_cursor = pane.screen.term().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::encode(&key, mods, app_cursor) {
            pane.scroll(Scroll::Bottom);
            pane.input(&bytes);
        }
    }

    fn mouse_press(&mut self, button: MouseButton) {
        let (x, y) = self.cursor;
        if let Some(i) = self.tab_rects.iter().position(|r| r.contains(x, y)) {
            self.active = i;
            return self.redraw();
        }
        if button == MouseButton::Left {
            let area = self.content_area();
            let grab = 3.0 * self.scale();
            if let Some(d) = self.tabs.get(self.active).and_then(|t| {
                t.layout.dividers(area, GAP).into_iter().find(|d| {
                    Rect { x: d.rect.x - grab, y: d.rect.y - grab, w: d.rect.w + 2.0 * grab, h: d.rect.h + 2.0 * grab }.contains(x, y)
                })
            }) {
                self.drag = Drag::Divider { path: d.path, axis: d.axis, parent: d.parent };
                return;
            }
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        self.tabs[self.active].focused = id;
        let (col, row, side) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.panes[&id].screen.mouse_modes();
        let mb = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
            _ => return,
        };
        if mouse::app_owns_mouse(modes, mods) {
            let ev = mouse::Event { button: mb, action: mouse::Action::Press, mods, col, row };
            if let Some(bytes) = mouse::encode(modes, ev) {
                self.panes.get_mut(&id).unwrap().input(&bytes);
            }
            self.drag = Drag::App { pane: id, button: mb };
            return self.redraw();
        }
        match mb {
            Button::Left => {
                let count = self.clicks.press(Instant::now(), col, row);
                let point = self.grid_point(id, col, row);
                let ty = input::selection_type(count, mods);
                self.panes.get_mut(&id).unwrap().screen.term_mut().selection = Some(Selection::new(ty, point, side));
                self.drag = Drag::Select { pane: id };
            }
            Button::Middle => self.paste(),
            _ => {}
        }
        self.redraw();
    }

    fn mouse_release(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Select { pane } => {
                let empty = self.panes.get(&pane).and_then(|p| p.screen.term().selection_to_string()).map_or(true, |s| s.is_empty());
                if empty {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.screen.term_mut().selection = None;
                    }
                } else {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_selection(pane);
                }
            }
            Drag::App { pane, button } => {
                let (x, y) = self.cursor;
                if let Some((_, rect)) = self.pane_at(x, y).filter(|(id, _)| *id == pane) {
                    let (col, row, _) = self.cell_at(pane, rect, x, y);
                    let p = self.panes.get_mut(&pane).unwrap();
                    let modes = p.screen.mouse_modes();
                    let ev = mouse::Event { button, action: mouse::Action::Release, mods: self.mods, col, row };
                    if let Some(bytes) = mouse::encode(modes, ev) {
                        p.input(&bytes);
                    }
                }
            }
            _ => {}
        }
        self.redraw();
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        match &self.drag {
            Drag::Select { pane } => {
                let pane = *pane;
                let area = self.content_area();
                let Some((_, rect)) = self.tabs[self.active].layout.rects(area, GAP).into_iter().find(|(id, _)| *id == pane) else { return };
                let (col, row, side) = self.cell_at(pane, rect, x, y);
                let point = self.grid_point(pane, col, row);
                if let Some(sel) = self.panes.get_mut(&pane).and_then(|p| p.screen.term_mut().selection.as_mut()) {
                    sel.update(point, side);
                }
                self.redraw();
            }
            Drag::Divider { path, axis, parent } => {
                let ratio = match axis {
                    Axis::Row => (x - parent.x) / parent.w,
                    Axis::Column => (y - parent.y) / parent.h,
                };
                let path = path.clone();
                self.tabs[self.active].layout.set_ratio(&path, ratio);
                self.relayout();
            }
            Drag::App { pane, button } => {
                let (pane, button) = (*pane, *button);
                self.send_motion(pane, Some(button), x, y);
            }
            Drag::None => {
                if let Some((id, _)) = self.pane_at(x, y) {
                    if self.panes[&id].screen.mouse_modes().tracking == Tracking::Any {
                        self.send_motion(id, None, x, y);
                    }
                }
            }
        }
    }

    fn send_motion(&mut self, pane: PaneId, button: Option<Button>, x: f32, y: f32) {
        let Some((_, rect)) = self.pane_at(x, y).filter(|(id, _)| *id == pane) else { return };
        let (col, row, _) = self.cell_at(pane, rect, x, y);
        let p = self.panes.get_mut(&pane).unwrap();
        let modes = p.screen.mouse_modes();
        let ev = mouse::Event { button: button.unwrap_or(Button::None), action: mouse::Action::Motion, mods: self.mods, col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            p.input(&bytes);
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let ch = self.cell().1;
        self.wheel_acc += match delta {
            MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINES,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / ch,
        };
        let lines = self.wheel_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel_acc -= lines as f32;
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let p = self.panes.get_mut(&id).unwrap();
        let mode = *p.screen.term().mode();
        let modes = p.screen.mouse_modes();
        match input::wheel(modes, mods, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR), lines, col, row) {
            Wheel::Send(bytes) => p.input(&bytes),
            Wheel::Scroll(n) => p.scroll(Scroll::Delta(n)),
        }
        self.redraw();
    }

    fn build_frame(&mut self) -> Frame {
        let cell = self.cell();
        let palette_bg = self.panes.values().next().map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let mut frame = Frame { clear: rgba(palette_bg, 1.0), ..Default::default() };
        let (w, _) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        self.tab_rects.clear();
        if self.tabs.len() > 1 {
            let titles: Vec<String> = self
                .tabs
                .iter()
                .map(|t| self.panes.get(&t.focused).and_then(|p| p.screen.title()).unwrap_or("Shell").to_string())
                .collect();
            let bar = Rect { x: 0.0, y: 0.0, w, h: self.tab_height() };
            self.tab_rects = scene::tab_bar(&mut frame, &titles, self.active, bar, cell);
        }
        let area = self.content_area();
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in tab.layout.rects(area, GAP) {
                if let Some(p) = self.panes.get(&id) {
                    scene::pane(&mut frame, &p.screen, self.inner(rect), cell, self.focused && id == tab.focused);
                }
            }
            scene::dividers(&mut frame, &tab.layout.dividers(area, GAP));
        }
        let error = self.banner.clone().or_else(|| self.panes.values().find_map(|p| p.error.clone()));
        if let Some(text) = error {
            let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
            scene::banner(&mut frame, &text, Rect { x: 0.0, y: 0.0, w, h }, cell);
        }
        frame
    }

    fn render(&mut self) {
        let frame = self.build_frame();
        let title = self
            .tabs
            .get(self.active)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.screen.title())
            .unwrap_or("Tandem")
            .to_string();
        let Some(gpu) = self.gpu.as_mut() else { return };
        gpu.window.set_title(&title);
        let texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(gpu.window.clone()) {
                    Ok(s) => gpu.surface = s,
                    Err(e) => self.banner = Some(format!("Lost the window surface: {e}")),
                }
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    gpu.window.request_redraw();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                self.banner = Some("GPU validation error".into());
                return;
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (w, h) = (gpu.config.width, gpu.config.height);
        if let Err(e) = gpu.renderer.draw(&frame, &view, w, h) {
            self.banner = Some(e);
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.queue.present(texture);
        }
    }
}

fn to_key(key: &WKey, text: Option<&str>) -> Option<Key> {
    Some(match key {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Text(" ".into()),
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => return None,
        },
        WKey::Character(s) => Key::Text(s.to_string()),
        _ => return text.map(|t| Key::Text(t.to_string())),
    })
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                message_box(&format!("Could not open a window: {e}"));
                return event_loop.exit();
            }
        };
        match init_gpu(window, event_loop) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(e) => {
                message_box(&format!("Could not start the GPU renderer: {e}"));
                return event_loop.exit();
            }
        }
        self.new_tab();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: UserEvent) {
        let mut changed = false;
        let mut exited = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            changed |= p.pump();
            if p.exited.is_some() {
                exited.push(*id);
            }
        }
        for id in exited {
            self.close_pane(id, event_loop);
        }
        if changed {
            self.redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.panes.clear();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(&g.device, &g.config);
                }
                self.relayout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(g) = self.gpu.as_mut() {
                    g.renderer.set_font_px(FONT_PX * scale_factor as f32);
                }
                self.relayout();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key() };
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => self.mouse_press(button),
            WindowEvent::MouseInput { state: ElementState::Released, .. } => self.mouse_release(),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn init_gpu(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
        event_loop.owned_display_handle(),
    )));
    let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&device, &config);
    let renderer = Renderer::new(&device, &queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, device, queue, config, renderer, instance, window })
}
```

`src/bin/tandem.rs`:

```rust
//! The Tandem terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    tandem::app::run();
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod app;
```

- [ ] **Step 2: Build it**

Run: `scripts/xt build --release --bin tandem`
Expected: builds with no errors and no warnings. Fix compile errors against the real APIs and ledger each change as a ruling.

- [ ] **Step 3: Run every test**

Run: `scripts/xt test --lib`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/bin/tandem.rs src/lib.rs
git commit -m "Add the Tandem window: tabs, splits, keys, mouse, copy and paste"
git push
```

---

### Task 11: Acceptance on Robert's desktop

**Files:**
- Create: `docs/acceptance/m1-checklist.md`

- [ ] **Step 1: Write the checklist**

`docs/acceptance/m1-checklist.md` lists each check, with the iTerm2 behavior it copies:

1. Start `%USERPROFILE%\src\tandem\target\release\tandem.exe`. A window opens with a PowerShell prompt.
2. Type `dir`, press Enter. Output appears. Colors look like iTerm2's defaults.
3. Ctrl+Shift+D splits right. Ctrl+Shift+Alt+D splits down. The inactive pane is dimmed.
4. Ctrl+Shift+] and Ctrl+Shift+[ move between panes. Ctrl+Shift+Alt+arrows move by direction.
5. Drag a divider. The panes resize and the text reflows.
6. Ctrl+Shift+T opens a tab. The tab bar appears. Click a tab to switch. Ctrl+Shift+1 selects the first tab.
7. Drag to select text. It is copied at once. Double-click selects a word, triple-click a line. Ctrl+Alt-drag selects a rectangle.
8. Middle-click pastes. Ctrl+Shift+V pastes.
9. `wsl` then `vim` with `:set mouse=a`: clicks move the cursor. Alt-drag still selects text.
10. `wsl` then `tmux` with mouse on: the wheel scrolls the tmux history; clicking a pane selects it.
11. `wsl` then `htop`: clicking a column header sorts by it.
12. Scroll the history with the wheel in a plain shell. Shift+PageUp and Shift+PageDown page through it.
13. Type `exit` in a pane. The pane closes. Closing the last pane closes the tab; the last tab closes the window.

- [ ] **Step 2: Hand it to Robert**

Ask Robert to run the checklist, because he is the user and he may be using the Windows test machine. Do not launch the window yourself. Record his results in the checklist file and commit.
