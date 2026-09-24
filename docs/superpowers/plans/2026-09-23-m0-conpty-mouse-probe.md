# Tandem Milestone 0: ConPTY and Mouse Probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that mouse input survives ConPTY for every mouse mode, before any window or renderer exists.

**Architecture:** A Rust library crate with four modules: a pure mouse encoder, a ConPTY loader, a pty wrapper, and a screen that runs app output through `alacritty_terminal`. A `probe` binary drives real apps (a Windows console app, a raw WSL program, vim, tmux, htop) under both the shipped ConPTY and the Windows copy, and prints a pass/fail table. These modules are the real Tandem modules. Milestone 1 builds the window on top of them.

**Tech Stack:** Rust 2024 edition (rustc 1.98.1 on EXTREME, MSVC), `alacritty_terminal` 0.26.0, `windows-sys` 0.59, ConPTY NuGet package 1.24.260710001, Python 3.12 in WSL.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`

**Scope:** This plan covers milestone 0 only. Each later milestone gets its own plan after this one reports, because the probe results can change the design of milestones 1 to 5.

## Where things run

- The repo is `~/gitrepos/tandem` on **sirtoolio**. Edit and commit there.
- sirtoolio has no Rust. **EXTREME** (Windows, `ssh extreme`, PowerShell) builds and runs everything.
- `scripts/xt <cargo args>` copies the repo to `C:\Users\datbi\src\tandem` over the sshfs mount at `/home/tbird/aiplayground/mount/extreme`, then runs `cargo <args>` there. Example: `scripts/xt test --lib mouse`.
- WSL on EXTREME is the distro `Ubuntu-Personal` (python3 3.12, vim, tmux 3.4). htop is installed in Task 7.

## Global Constraints

- Target: Windows only, `x86_64-pc-windows-msvc`. Every build and test runs on EXTREME through `scripts/xt`.
- `alacritty_terminal = "=0.26.0"`, used unmodified.
- `windows-sys = "0.59"`, the same version `alacritty_terminal` 0.26.0 uses.
- ConPTY: `Microsoft.Windows.Console.ConPTY` 1.24.260710001, nupkg SHA256 `175640566A3B59C4B132070EE96C2C77E5AB7EDD2E92732A5EB3610BBF63D90E`.
- Loading the shipped ConPTY never falls back silently. A missing `conpty.dll` or `OpenConsole.exe` is an error that names the file.
- No code copied from Windows Terminal or iTerm2.
- Commits: plain messages, no Claude attribution lines of any kind. Work on `main`.
- `.ps1` files are ASCII only. Docs have no emoji and no em dashes.
- The probe never hides a failure. A failed scenario prints what the terminal saw.

## Review Focus

1. A click past column 223 in legacy (non-SGR) mouse mode cannot be encoded. Expect: no bytes sent, not wrapped garbage. Test: Task 1, `legacy_past_223_is_dropped`.
2. A drag that leaves the window gives negative or oversize pixel positions. Expect: the cell clamps to the grid edge. Test: Task 1, `cell_at_clamps_outside_the_grid`.
3. An app sends a query (device attributes) at startup and waits. Expect: the terminal answers, so the app does not hang. Test: Task 4, `answers_device_attributes`, and `Session::pump_until` writes replies (Task 5).
4. A pane closes while its child still runs, or after the child exits. Expect: closing returns within 5 s, no deadlock. Tests: Task 3, `drop_returns_while_child_runs` and `drop_returns_after_child_exits`.
5. The shipped ConPTY folder has `conpty.dll` but not `OpenConsole.exe`. Expect: an error naming `OpenConsole.exe`, because ConPTY would otherwise quietly use the Windows copy. Test: Task 2, `missing_openconsole_is_an_error`.

---

### Task 1: Scaffold, sync script, and mouse encoder

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `src/lib.rs`, `scripts/xt`, `src/mouse.rs` (tests inline)

**Interfaces:**
- Consumes: nothing.
- Produces (in `tandem::mouse`):
  - `enum Button { Left, Middle, Right, WheelUp, WheelDown, None }` (`None` = motion with no button held)
  - `enum Action { Press, Release, Motion }`
  - `struct Mods { shift: bool, alt: bool, ctrl: bool }` (Default)
  - `enum Tracking { Off, Click, Drag, Any }` (Default = Off; Click = DEC 1000, Drag = 1002, Any = 1003)
  - `struct Modes { tracking: Tracking, sgr: bool }` (Default)
  - `struct Event { button: Button, action: Action, mods: Mods, col: u32, row: u32 }` (0-based cells)
  - `fn encode(modes: Modes, ev: Event) -> Option<Vec<u8>>`
  - `fn app_owns_mouse(modes: Modes, mods: Mods) -> bool`
  - `fn cell_at(x: f64, y: f64, cell_w: f64, cell_h: f64, cols: u32, rows: u32) -> (u32, u32)`
  - All types derive `Clone, Copy, Debug, PartialEq, Eq`.

- [ ] **Step 1: Create the GitHub repo and push the existing spec commit**

```bash
cd ~/gitrepos/tandem
gh-datbird repo create datbird/tandem --private
git remote add origin git@github-datbird:datbird/tandem.git
git push -u origin main
```

Expected: the repo exists and `main` holds the spec commits.

- [ ] **Step 2: Write the scaffold files**

`Cargo.toml`:

```toml
[package]
name = "tandem"
version = "0.0.1"
edition = "2024"
publish = false

[dependencies]
alacritty_terminal = "=0.26.0"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Storage_FileSystem",
    "Win32_System_Console",
    "Win32_System_IO",
    "Win32_System_LibraryLoader",
    "Win32_System_Pipes",
    "Win32_System_Threading",
] }
```

`.gitignore`:

```
/target
/vendor
```

`src/lib.rs`:

```rust
//! Tandem: an iTerm2-style terminal for Windows.

pub mod mouse;
```

`scripts/xt` (then `chmod +x scripts/xt`):

```bash
#!/usr/bin/env bash
# Copies this repo to EXTREME and runs cargo there.
# Usage: scripts/xt test --lib mouse
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
mount_root=/home/tbird/aiplayground/mount/extreme
dest="$mount_root/c/Users/datbi/src/tandem"
win='C:\Users\datbi\src\tandem'
if ! mountpoint -q "$mount_root"; then
    echo "xt: $mount_root is not mounted. Run: systemctl --user restart sshfs-extreme" >&2
    exit 2
fi
mkdir -p "$dest"
# -rt, not -a: the Windows side cannot take Linux owners or modes.
# target/ and vendor/ live only on EXTREME, and excluded paths survive --delete.
rsync -rt --delete --exclude target/ --exclude vendor/ --exclude .git/ "$repo/" "$dest/"
ssh extreme "cd '$win'; cargo $*; exit \$LASTEXITCODE"
```

- [ ] **Step 3: Write the failing tests**

`src/mouse.rs`, tests only for now (the module body comes in Step 5):

```rust
//! Encodes mouse events into the bytes a terminal app expects.

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(button: Button, action: Action, col: u32, row: u32) -> Event {
        Event { button, action, mods: Mods::default(), col, row }
    }

    const SGR_CLICK: Modes = Modes { tracking: Tracking::Click, sgr: true };
    const SGR_DRAG: Modes = Modes { tracking: Tracking::Drag, sgr: true };
    const SGR_ANY: Modes = Modes { tracking: Tracking::Any, sgr: true };
    const LEGACY_CLICK: Modes = Modes { tracking: Tracking::Click, sgr: false };

    #[test]
    fn off_sends_nothing() {
        assert_eq!(encode(Modes::default(), ev(Button::Left, Action::Press, 0, 0)), None);
    }

    #[test]
    fn sgr_press_and_release() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Press, 0, 0)).unwrap(), b"\x1b[<0;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Release, 0, 0)).unwrap(), b"\x1b[<0;1;1m");
        assert_eq!(encode(SGR_CLICK, ev(Button::Right, Action::Press, 9, 4)).unwrap(), b"\x1b[<2;10;5M");
    }

    #[test]
    fn legacy_press_and_release() {
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 4, 9)).unwrap(), vec![0x1b, b'[', b'M', 32, 37, 42]);
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Release, 4, 9)).unwrap(), vec![0x1b, b'[', b'M', 35, 37, 42]);
    }

    #[test]
    fn click_mode_ignores_motion() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Motion, 1, 1)), None);
    }

    #[test]
    fn drag_mode_reports_drags_only() {
        assert_eq!(encode(SGR_DRAG, ev(Button::Left, Action::Motion, 1, 1)).unwrap(), b"\x1b[<32;2;2M");
        assert_eq!(encode(SGR_DRAG, ev(Button::None, Action::Motion, 1, 1)), None);
    }

    #[test]
    fn any_mode_reports_plain_motion() {
        assert_eq!(encode(SGR_ANY, ev(Button::None, Action::Motion, 1, 1)).unwrap(), b"\x1b[<35;2;2M");
    }

    #[test]
    fn wheel_has_no_release() {
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelUp, Action::Press, 0, 0)).unwrap(), b"\x1b[<64;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelDown, Action::Press, 0, 0)).unwrap(), b"\x1b[<65;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelUp, Action::Release, 0, 0)), None);
    }

    #[test]
    fn modifiers_add_bits() {
        let e = Event { mods: Mods { shift: true, alt: false, ctrl: true }, ..ev(Button::Left, Action::Press, 0, 0) };
        assert_eq!(encode(SGR_CLICK, e).unwrap(), b"\x1b[<20;1;1M");
    }

    #[test]
    fn legacy_release_keeps_modifiers() {
        let e = Event { mods: Mods { shift: true, alt: false, ctrl: false }, ..ev(Button::Left, Action::Release, 0, 0) };
        assert_eq!(encode(LEGACY_CLICK, e).unwrap(), vec![0x1b, b'[', b'M', 32 + 3 + 4, 33, 33]);
    }

    #[test]
    fn legacy_past_223_is_dropped() {
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 223, 0)), None);
        assert!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 222, 0)).is_some());
    }

    #[test]
    fn sgr_handles_wide_screens() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Press, 299, 0)).unwrap(), b"\x1b[<0;300;1M");
    }

    #[test]
    fn alt_takes_the_mouse_back() {
        let none = Mods::default();
        let alt = Mods { alt: true, ..none };
        assert!(!app_owns_mouse(Modes::default(), none));
        assert!(app_owns_mouse(SGR_CLICK, none));
        assert!(!app_owns_mouse(SGR_CLICK, alt));
    }

    #[test]
    fn cell_at_maps_pixels() {
        assert_eq!(cell_at(25.0, 41.0, 10.0, 20.0, 80, 24), (2, 2));
    }

    #[test]
    fn cell_at_clamps_outside_the_grid() {
        assert_eq!(cell_at(-50.0, -1.0, 10.0, 20.0, 80, 24), (0, 0));
        assert_eq!(cell_at(5000.0, 9000.0, 10.0, 20.0, 80, 24), (79, 23));
    }
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `scripts/xt test --lib mouse`
Expected: compile errors such as `cannot find type Event in this scope`.

- [ ] **Step 5: Write the encoder**

Put this above the `#[cfg(test)]` block in `src/mouse.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Which DEC mouse tracking mode the app turned on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tracking {
    #[default]
    Off,
    /// DEC 1000: presses and releases.
    Click,
    /// DEC 1002: also motion while a button is held.
    Drag,
    /// DEC 1003: all motion.
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub tracking: Tracking,
    /// DEC 1006: SGR encoding.
    pub sgr: bool,
}

/// A mouse event at a 0-based cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub button: Button,
    pub action: Action,
    pub mods: Mods,
    pub col: u32,
    pub row: u32,
}

/// True when mouse events go to the app. Alt always takes the mouse back for
/// selection, like Option in iTerm2.
pub fn app_owns_mouse(modes: Modes, mods: Mods) -> bool {
    modes.tracking != Tracking::Off && !mods.alt
}

/// Returns the bytes to send the app for `ev`, or None when the app did not ask for it.
pub fn encode(modes: Modes, ev: Event) -> Option<Vec<u8>> {
    let wheel = matches!(ev.button, Button::WheelUp | Button::WheelDown);
    let reported = match (modes.tracking, ev.action) {
        (Tracking::Off, _) => false,
        (_, Action::Press) => ev.button != Button::None,
        (_, Action::Release) => !wheel && ev.button != Button::None,
        (Tracking::Click, Action::Motion) => false,
        (Tracking::Drag, Action::Motion) => !wheel && ev.button != Button::None,
        (Tracking::Any, Action::Motion) => !wheel,
    };
    if !reported {
        return None;
    }

    let button: u32 = match ev.button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    let mods = 4 * ev.mods.shift as u32 + 8 * ev.mods.alt as u32 + 16 * ev.mods.ctrl as u32;
    let motion = if ev.action == Action::Motion { 32 } else { 0 };
    let (x, y) = (ev.col + 1, ev.row + 1);

    if modes.sgr {
        let end = if ev.action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{x};{y}{end}", button + mods + motion).into_bytes());
    }

    // Legacy encoding: each value is one byte offset by 32, so 223 is the largest
    // position, and a release does not say which button.
    if x > 223 || y > 223 {
        return None;
    }
    let cb = if ev.action == Action::Release { 3 + mods } else { button + mods + motion };
    Some(vec![0x1b, b'[', b'M', (32 + cb) as u8, (32 + x) as u8, (32 + y) as u8])
}

/// Maps a pixel position to a cell, clamped to the grid so drags outside the
/// window still land on an edge cell.
pub fn cell_at(x: f64, y: f64, cell_w: f64, cell_h: f64, cols: u32, rows: u32) -> (u32, u32) {
    let col = (x / cell_w).floor().clamp(0.0, (cols - 1) as f64) as u32;
    let row = (y / cell_h).floor().clamp(0.0, (rows - 1) as f64) as u32;
    (col, row)
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `scripts/xt test --lib mouse`
Expected: `test result: ok. 14 passed`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml .gitignore scripts/xt src/lib.rs src/mouse.rs
git commit -m "Add scaffold, EXTREME sync script, and mouse encoder"
git push
```

---

### Task 2: ConPTY loader

**Files:**
- Create: `scripts/fetch-conpty.ps1`, `src/conpty.rs` (tests inline)
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces (in `tandem::conpty`, Windows only):
  - `enum Source { Shipped(PathBuf), Inbox }` with `fn label(&self) -> &'static str` ("shipped" or "inbox")
  - `type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;`
  - `type ResizeFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;`
  - `type CloseFn = unsafe extern "system" fn(HPCON);`
  - `#[derive(Clone)] struct Api { pub source: Source, pub create: CreateFn, pub resize: ResizeFn, pub close: CloseFn }`
  - `fn load(want: &Source) -> Result<Api, String>`
  - `fn dev_vendor_dir() -> PathBuf` (the repo's `vendor/conpty`)

- [ ] **Step 1: Write the fetch script**

`scripts/fetch-conpty.ps1` (ASCII only):

```powershell
# Downloads the pinned ConPTY package and copies the x64 files to vendor\conpty.
$ErrorActionPreference = 'Stop'
$version = '1.24.260710001'
$sha256 = '175640566A3B59C4B132070EE96C2C77E5AB7EDD2E92732A5EB3610BBF63D90E'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\conpty'
$tmp = Join-Path $env:TEMP "conpty-$version"
# Expand-Archive in PowerShell 5.1 only accepts a .zip name.
$pkg = "$tmp.zip"
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$version/microsoft.windows.console.conpty.$version.nupkg"
Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $pkg
$got = (Get-FileHash -Algorithm SHA256 $pkg).Hash
if ($got -ne $sha256) { throw "ConPTY package hash mismatch: got $got, want $sha256" }
if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
Expand-Archive -Path $pkg -DestinationPath $tmp
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $tmp 'runtimes\win-x64\native\conpty.dll') $dest -Force
Copy-Item (Join-Path $tmp 'build\native\runtimes\x64\OpenConsole.exe') $dest -Force
Remove-Item -Recurse -Force $tmp, $pkg
Write-Output "ConPTY $version installed in $dest"
```

- [ ] **Step 2: Sync and run it on EXTREME**

```bash
scripts/xt --version
ssh extreme 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\tandem\scripts\fetch-conpty.ps1'
```

Expected: `ConPTY 1.24.260710001 installed in C:\Users\datbi\src\tandem\vendor\conpty`.

- [ ] **Step 3: Write the failing tests**

`src/conpty.rs`, tests only for now:

```rust
//! Loads the ConPTY functions, from the shipped conpty.dll or from Windows.

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inbox_loads() {
        assert_eq!(load(&Source::Inbox).unwrap().source, Source::Inbox);
    }

    #[test]
    fn shipped_loads_from_vendor() {
        let dir = dev_vendor_dir();
        assert_eq!(load(&Source::Shipped(dir.clone())).unwrap().source, Source::Shipped(dir));
    }

    #[test]
    fn missing_dll_is_an_error() {
        let err = load(&Source::Shipped(scratch("nodll"))).err().unwrap();
        assert!(err.contains("conpty.dll not found"), "{err}");
    }

    #[test]
    fn missing_openconsole_is_an_error() {
        let dir = scratch("noexe");
        std::fs::copy(dev_vendor_dir().join("conpty.dll"), dir.join("conpty.dll")).unwrap();
        let err = load(&Source::Shipped(dir)).err().unwrap();
        assert!(err.contains("OpenConsole.exe not found"), "{err}");
    }

    #[test]
    fn labels() {
        assert_eq!(Source::Inbox.label(), "inbox");
        assert_eq!(Source::Shipped(PathBuf::new()).label(), "shipped");
    }
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod conpty;
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `scripts/xt test --lib conpty`
Expected: compile errors such as `cannot find function load in this scope`.

- [ ] **Step 5: Write the loader**

Put this above the `#[cfg(test)]` block in `src/conpty.rs`:

```rust
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::core::HRESULT;

pub type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;
pub type ResizeFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
pub type CloseFn = unsafe extern "system" fn(HPCON);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// conpty.dll and OpenConsole.exe in this folder.
    Shipped(PathBuf),
    /// The copy built into Windows.
    Inbox,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Shipped(_) => "shipped",
            Source::Inbox => "inbox",
        }
    }
}

#[derive(Clone)]
pub struct Api {
    pub source: Source,
    pub create: CreateFn,
    pub resize: ResizeFn,
    pub close: CloseFn,
}

pub fn load(want: &Source) -> Result<Api, String> {
    match want {
        Source::Inbox => Ok(Api {
            source: Source::Inbox,
            create: CreatePseudoConsole,
            resize: ResizePseudoConsole,
            close: ClosePseudoConsole,
        }),
        Source::Shipped(dir) => load_shipped(dir),
    }
}

fn load_shipped(dir: &Path) -> Result<Api, String> {
    let dll = dir.join("conpty.dll");
    // conpty.dll starts OpenConsole.exe from its own folder. Without the exe, ConPTY
    // quietly uses the Windows copy, so a missing exe is an error here.
    for file in [dll.clone(), dir.join("OpenConsole.exe")] {
        if !file.is_file() {
            return Err(format!("{} not found", file.display()));
        }
    }
    let wide: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = LoadLibraryW(wide.as_ptr());
        if module.is_null() {
            return Err(format!("LoadLibraryW({}) failed: {}", dll.display(), std::io::Error::last_os_error()));
        }
        let get = |name: &str| {
            let cname = format!("{name}\0");
            GetProcAddress(module, cname.as_ptr()).ok_or_else(|| format!("{} has no {name}", dll.display()))
        };
        let create = get("ConptyCreatePseudoConsole")?;
        let resize = get("ConptyResizePseudoConsole")?;
        let close = get("ConptyClosePseudoConsole")?;
        type Raw = unsafe extern "system" fn() -> isize;
        Ok(Api {
            source: Source::Shipped(dir.to_path_buf()),
            create: std::mem::transmute::<Raw, CreateFn>(create),
            resize: std::mem::transmute::<Raw, ResizeFn>(resize),
            close: std::mem::transmute::<Raw, CloseFn>(close),
        })
    }
}

/// The repo's vendor\conpty folder, filled by scripts\fetch-conpty.ps1.
pub fn dev_vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("conpty")
}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `scripts/xt test --lib conpty`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 7: Commit**

```bash
git add scripts/fetch-conpty.ps1 src/conpty.rs src/lib.rs
git commit -m "Add ConPTY loader that refuses a partial shipped copy"
git push
```

---

### Task 3: Pty

**Files:**
- Create: `src/pty.rs` (tests inline)
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `tandem::conpty::{Api, Source, load, dev_vendor_dir}`.
- Produces (in `tandem::pty`, Windows only):
  - `struct Pty` (not `Send`)
  - `fn spawn(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String>`
  - `Pty::write(&self, bytes: &[u8]) -> Result<(), String>`
  - `Pty::resize(&self, cols: u16, rows: u16) -> Result<(), String>`
  - `Pty::read_timeout(&self, wait: Duration) -> Option<Vec<u8>>` (next output chunk)
  - `Pty::read_until(&self, needle: &[u8], wait: Duration) -> Result<Vec<u8>, Vec<u8>>` (Ok on a hit, Err with everything read on timeout)
  - `Pty::wait_exit(&self, wait: Duration) -> Option<u32>` (exit code)
  - Dropping a `Pty` closes the pseudoconsole, which ends the child.

- [ ] **Step 1: Write the failing tests**

`src/pty.rs`, tests only for now:

```rust
//! A child process running inside a ConPTY pseudoconsole.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conpty::{self, Source, dev_vendor_dir};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn apis() -> Vec<Api> {
        vec![
            conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap(),
            conpty::load(&Source::Inbox).unwrap(),
        ]
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn runs_a_command_and_reads_output() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c echo tandem-ok", 80, 24).unwrap();
            p.read_until(b"tandem-ok", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            assert_eq!(p.wait_exit(secs(10)), Some(0), "{}", api.source.label());
        }
    }

    #[test]
    fn reports_exit_code() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 7", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(7), "{}", api.source.label());
        }
    }

    #[test]
    fn input_reaches_the_child() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            // cmd prints abc123 only when it runs the echo. The typed line shows abc^123.
            p.write(b"echo abc^123\r").unwrap();
            p.read_until(b"abc123", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            p.write(b"exit 3\r").unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(3), "{}", api.source.label());
        }
    }

    #[test]
    fn resize_succeeds() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            p.resize(120, 40).unwrap();
        }
    }

    #[test]
    fn bad_command_is_an_error() {
        let err = spawn(&apis()[0], "no-such-program-xyz.exe", 80, 24).err().unwrap();
        assert!(err.contains("CreateProcessW"), "{err}");
    }

    #[test]
    fn drop_returns_while_child_runs() {
        for api in apis() {
            let p = spawn(&api, "ping -n 30 127.0.0.1", 80, 24).unwrap();
            p.read_until(b"Pinging", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }

    #[test]
    fn drop_returns_after_child_exits() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 0", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(0));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod pty;
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib pty`
Expected: compile errors such as `cannot find function spawn in this scope`.

- [ ] **Step 3: Write the pty**

Put this above the `#[cfg(test)]` block in `src/pty.rs`:

```rust
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{COORD, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::conpty::Api;

pub struct Pty {
    api: Api,
    hpc: HPCON,
    input: HANDLE,
    process: HANDLE,
    output: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: cols as i16, Y: rows as i16 }
}

pub fn spawn(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String> {
    let (hpc, input, output, process) = unsafe { start(api, cmdline, cols, rows)? };
    let (rx, reader) = start_reader(output);
    Ok(Pty { api: api.clone(), hpc, input, process, output: rx, reader: Some(reader) })
}

/// Creates the pseudoconsole and the child. Returns (pseudoconsole, our input
/// write end, our output read end, child process).
unsafe fn start(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<(HPCON, HANDLE, HANDLE, HANDLE), String> {
    unsafe {
        let (mut in_read, mut in_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        let (mut out_read, mut out_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut in_read, &mut in_write, null(), 0) == 0 {
            return Err(os_err("CreatePipe (input)"));
        }
        if CreatePipe(&mut out_read, &mut out_write, null(), 0) == 0 {
            CloseHandle(in_read);
            CloseHandle(in_write);
            return Err(os_err("CreatePipe (output)"));
        }

        let mut hpc: HPCON = 0;
        let hr = (api.create)(coord(cols, rows), in_read, out_write, 0, &mut hpc);
        // ConPTY keeps its own copies of these two ends.
        CloseHandle(in_read);
        CloseHandle(out_write);
        if hr != 0 {
            CloseHandle(in_write);
            CloseHandle(out_read);
            return Err(format!("CreatePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        let fail = |msg: String| {
            (api.close)(hpc);
            CloseHandle(in_write);
            CloseHandle(out_read);
            Err(msg)
        };

        let mut attr_size = 0usize;
        // This first call fails on purpose. It only reports the size to allocate.
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut attr_size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        // The attribute value is the HPCON itself, not a pointer to it.
        if UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpc as *const core::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }

        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // Null std handles with this flag, so the child cannot inherit ours.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut cmd: Vec<u16> = OsStr::new(cmdline).encode_wide().chain(Some(0)).collect();
        let ok = CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err(&format!("CreateProcessW ({cmdline})")));
        }
        CloseHandle(pi.hThread);
        Ok((hpc, in_write, out_read, pi.hProcess))
    }
}

/// Drains ConPTY output on its own thread, so ConPTY never blocks on a full pipe.
fn start_reader(output: HANDLE) -> (Receiver<Vec<u8>>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    // HANDLE is a raw pointer and not Send. The thread owns this handle from here on.
    let raw = output as usize;
    let reader = std::thread::spawn(move || {
        let output = raw as HANDLE;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut n = 0u32;
            let ok = unsafe { ReadFile(output, buf.as_mut_ptr(), buf.len() as u32, &mut n, null_mut()) };
            if ok == 0 || n == 0 || tx.send(buf[..n as usize].to_vec()).is_err() {
                break;
            }
        }
        unsafe { CloseHandle(output) };
    });
    (rx, reader)
}

impl Pty {
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut done = 0;
        while done < bytes.len() {
            let mut n = 0u32;
            let rest = &bytes[done..];
            let ok = unsafe { WriteFile(self.input, rest.as_ptr(), rest.len() as u32, &mut n, null_mut()) };
            if ok == 0 {
                return Err(os_err("WriteFile"));
            }
            done += n as usize;
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let hr = unsafe { (self.api.resize)(self.hpc, coord(cols, rows)) };
        if hr != 0 {
            return Err(format!("ResizePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        Ok(())
    }

    /// Returns the next chunk of output, or None if nothing came within `wait`.
    pub fn read_timeout(&self, wait: Duration) -> Option<Vec<u8>> {
        match self.output.recv_timeout(wait) {
            Ok(chunk) => Some(chunk),
            Err(RecvTimeoutError::Timeout) => None,
            // The output pipe closed. Sleep, so callers that poll do not spin.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    /// Reads until `needle` appears. Ok with everything read on a hit, Err with
    /// everything read on timeout.
    pub fn read_until(&self, needle: &[u8], wait: Duration) -> Result<Vec<u8>, Vec<u8>> {
        let end = Instant::now() + wait;
        let mut all = Vec::new();
        loop {
            if all.windows(needle.len()).any(|w| w == needle) {
                return Ok(all);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(all);
            }
            if let Some(chunk) = self.read_timeout(left.min(Duration::from_millis(50))) {
                all.extend(chunk);
            }
        }
    }

    /// Waits for the child to exit and returns its exit code.
    pub fn wait_exit(&self, wait: Duration) -> Option<u32> {
        if unsafe { WaitForSingleObject(self.process, wait.as_millis() as u32) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return None;
        }
        Some(code)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Closing the pseudoconsole ends the child and closes the output pipe. The
        // reader thread keeps draining meanwhile, so the close cannot block on a full pipe.
        unsafe {
            (self.api.close)(self.hpc);
            CloseHandle(self.input);
            CloseHandle(self.process);
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib pty`
Expected: `test result: ok. 7 passed`.

If a `drop_returns_*` test fails for `inbox` only, that is a probe finding about the Windows copy, not a bug to hide. Record the output in the task report and keep going. If it fails for `shipped`, stop and report.

- [ ] **Step 5: Commit**

```bash
git add src/pty.rs src/lib.rs
git commit -m "Add ConPTY pty with a draining reader thread"
git push
```

---

### Task 4: Screen

**Files:**
- Create: `src/screen.rs` (tests inline)
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `tandem::mouse::{Modes, Tracking}`, `alacritty_terminal` 0.26.0.
- Produces (in `tandem::screen`):
  - `struct Screen`
  - `Screen::new(cols: usize, rows: usize) -> Screen`
  - `Screen::feed(&mut self, bytes: &[u8]) -> Vec<u8>` (returns reply bytes the terminal must write back to the pty)
  - `Screen::mouse_modes(&self) -> Modes`
  - `Screen::text(&self) -> String` (visible rows, `\n` after each)

- [ ] **Step 1: Write the failing tests**

`src/screen.rs`, tests only for now:

```rust
//! Runs app output through the terminal core and collects the replies the app expects.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_drag_and_sgr() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(s.mouse_modes(), Modes { tracking: Tracking::Drag, sgr: true });
    }

    #[test]
    fn any_motion_wins() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1000h\x1b[?1003h");
        assert_eq!(s.mouse_modes().tracking, Tracking::Any);
    }

    #[test]
    fn reset_turns_tracking_off() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1002l");
        assert_eq!(s.mouse_modes().tracking, Tracking::Off);
    }

    #[test]
    fn answers_device_attributes() {
        let mut s = Screen::new(80, 24);
        let reply = s.feed(b"\x1b[c");
        assert!(reply.starts_with(b"\x1b[?"), "{:?}", String::from_utf8_lossy(&reply));
    }

    #[test]
    fn text_shows_output() {
        let mut s = Screen::new(80, 24);
        s.feed(b"hello\r\nworld");
        let text = s.text();
        assert!(text.contains("hello") && text.contains("world"), "{text}");
    }
}
```

Add to `src/lib.rs`:

```rust
pub mod screen;
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib screen`
Expected: compile errors such as `cannot find struct Screen in this scope`.

- [ ] **Step 3: Write the screen**

Put this above the `#[cfg(test)]` block in `src/screen.rs`:

```rust
use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::Processor;

use crate::mouse::{Modes, Tracking};

#[derive(Clone)]
struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

pub struct Screen {
    term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (tx, events) = mpsc::channel();
        let term = Term::new(Config::default(), &TermSize::new(cols, rows), Listener(tx));
        Self { term, parser: Processor::new(), events }
    }

    /// Feeds app output. Returns the bytes the terminal must send back, such as
    /// answers to device queries. An app that gets no answer may hang.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, bytes);
        let mut reply = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let Event::PtyWrite(text) = event {
                reply.extend_from_slice(text.as_bytes());
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

Run: `scripts/xt test --lib screen`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/screen.rs src/lib.rs
git commit -m "Add screen wrapper over alacritty_terminal"
git push
```

---

### Task 5: Probe harness, Windows console scenario, probe binary

**Files:**
- Create: `src/probe.rs`, `src/bin/probe.rs`, `src/bin/mouse_echo_win.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `tandem::conpty::{Api, Source, load, dev_vendor_dir}`, `tandem::pty::{spawn, Pty}`, `tandem::screen::Screen`, `tandem::mouse::*`.
- Produces (in `tandem::probe`, Windows only):
  - `struct Session { pub pty: Pty, pub screen: Screen }` with `start(api: &Api, cmdline: &str) -> Result<Session, String>`, `pump_until(&mut self, wait: Duration, done: impl FnMut(&Screen) -> bool) -> bool`, `send(&self, bytes: &[u8]) -> Result<(), String>`
  - `struct Outcome { pub name: String, pub conpty: &'static str, pub pass: bool, pub detail: String }`
  - `fn run_all(api: &Api, filter: &str) -> Vec<Outcome>`
  - `fn wsl_path(path: &Path) -> String`
  - Constants `COLS: u16 = 200`, `ROWS: u16 = 50`, `DISTRO: &str = "Ubuntu-Personal"`

- [ ] **Step 1: Write the failing test for `wsl_path`**

`src/probe.rs`:

```rust
//! Drives real apps under ConPTY and checks that mouse input reaches them.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_path_maps_drive_letters() {
        assert_eq!(
            wsl_path(Path::new(r"C:\Users\datbi\src\tandem\probe\x.py")),
            "/mnt/c/Users/datbi/src/tandem/probe/x.py"
        );
    }
}
```

Add to `src/lib.rs`:

```rust
#[cfg(windows)]
pub mod probe;
```

- [ ] **Step 2: Run the test to see it fail**

Run: `scripts/xt test --lib probe`
Expected: compile error `cannot find function wsl_path in this scope`.

- [ ] **Step 3: Write the harness and the Windows console scenario**

Put this above the `#[cfg(test)]` block in `src/probe.rs`:

```rust
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::conpty::Api;
use crate::mouse::{self, Action, Button, Event, Modes, Mods, Tracking};
use crate::pty::{self, Pty};
use crate::screen::Screen;

pub const COLS: u16 = 200;
pub const ROWS: u16 = 50;
pub const DISTRO: &str = "Ubuntu-Personal";

pub struct Session {
    pub pty: Pty,
    pub screen: Screen,
}

impl Session {
    pub fn start(api: &Api, cmdline: &str) -> Result<Session, String> {
        Ok(Session { pty: pty::spawn(api, cmdline, COLS, ROWS)?, screen: Screen::new(COLS as usize, ROWS as usize) })
    }

    /// Feeds output into the screen until `done` is true or time runs out. Writes
    /// the screen's query replies back to the app, so apps that ask do not hang.
    pub fn pump_until(&mut self, wait: Duration, mut done: impl FnMut(&Screen) -> bool) -> bool {
        let end = Instant::now() + wait;
        loop {
            if done(&self.screen) {
                return true;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if let Some(chunk) = self.pty.read_timeout(left.min(Duration::from_millis(50))) {
                let reply = self.screen.feed(&chunk);
                if !reply.is_empty() {
                    let _ = self.pty.write(&reply);
                }
            }
        }
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), String> {
        self.pty.write(bytes)
    }
}

pub struct Outcome {
    pub name: String,
    pub conpty: &'static str,
    pub pass: bool,
    pub detail: String,
}

type Scenario = Box<dyn Fn(&Api) -> Result<String, String>>;

/// Runs every scenario whose name contains `filter`. Ok(detail) passes, Err(detail) fails.
pub fn run_all(api: &Api, filter: &str) -> Vec<Outcome> {
    let mut list: Vec<(&str, Scenario)> = vec![("win-console", Box::new(win_console_mouse))];
    list.retain(|(name, _)| name.contains(filter));
    list.iter()
        .map(|(name, run)| {
            let result = run(api);
            Outcome {
                name: name.to_string(),
                conpty: api.source.label(),
                pass: result.is_ok(),
                detail: result.unwrap_or_else(|e| e),
            }
        })
        .collect()
}

pub fn wsl_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) => format!("/mnt/{}/{rest}", drive.to_lowercase()),
        None => s,
    }
}

fn repo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn press_release(s: &Session, modes: Modes, col: u32, row: u32) -> Result<(), String> {
    for action in [Action::Press, Action::Release] {
        let ev = Event { button: Button::Left, action, mods: Mods::default(), col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            s.send(&bytes)?;
        }
    }
    Ok(())
}

/// A Windows console app reads mouse input with ReadConsoleInputW. ConPTY must ask
/// the terminal for mouse reports, then turn the reports into console events.
fn win_console_mouse(api: &Api) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("mouse_echo_win.exe");
    if !exe.is_file() {
        return Err(format!("{} is not built. Run cargo build --bins first.", exe.display()));
    }
    let mut s = Session::start(api, &format!("\"{}\"", exe.display()))?;
    let asked = s.pump_until(Duration::from_secs(5), |sc| {
        sc.text().contains("READY") && sc.mouse_modes().tracking != Tracking::Off
    });
    if !s.screen.text().contains("READY") {
        return Err(format!("helper never printed READY. Screen: {:?}", s.screen.text().trim_end()));
    }
    let modes = s.screen.mouse_modes();
    // If ConPTY never asked, still send SGR clicks to learn whether they get through.
    let send_modes = if asked { modes } else { Modes { tracking: Tracking::Click, sgr: true } };
    press_release(&s, send_modes, 10, 5)?;
    let got = s.pump_until(Duration::from_secs(5), |sc| sc.text().contains("MOUSE x=10 y=5 buttons=1"));
    match (asked, got) {
        (true, true) => Ok(format!("modes {modes:?}")),
        (false, true) => Err("click arrived, but ConPTY never asked the terminal for mouse input".into()),
        (true, false) => Err(format!("ConPTY asked for {modes:?}, but the click never arrived")),
        (false, false) => Err("ConPTY never asked for mouse input, and the click never arrived".into()),
    }
}
```

`src/bin/mouse_echo_win.rs`:

```rust
//! Probe helper: prints the first two console mouse events it receives, then exits.

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_MOUSE_INPUT, ENABLE_WINDOW_INPUT, GetStdHandle, INPUT_RECORD,
        MOUSE_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        // ENABLE_EXTENDED_FLAGS without ENABLE_QUICK_EDIT_MODE turns QuickEdit off.
        // QuickEdit would eat the mouse input.
        if SetConsoleMode(input, ENABLE_EXTENDED_FLAGS | ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT) == 0 {
            eprintln!("SetConsoleMode failed: {}", std::io::Error::last_os_error());
            std::process::exit(2);
        }
        println!("READY");
        let mut seen = 0;
        while seen < 2 {
            let mut rec: INPUT_RECORD = std::mem::zeroed();
            let mut n = 0u32;
            if ReadConsoleInputW(input, &mut rec, 1, &mut n) == 0 {
                eprintln!("ReadConsoleInputW failed: {}", std::io::Error::last_os_error());
                std::process::exit(2);
            }
            if n == 1 && rec.EventType as u32 == MOUSE_EVENT {
                let m = rec.Event.MouseEvent;
                println!(
                    "MOUSE x={} y={} buttons={} flags={}",
                    m.dwMousePosition.X, m.dwMousePosition.Y, m.dwButtonState, m.dwEventFlags
                );
                seen += 1;
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {}
```

`src/bin/probe.rs`:

```rust
//! Runs the mouse probe under the shipped ConPTY and the Windows copy, and prints
//! a Markdown table. Exits 1 if any scenario fails under the shipped ConPTY.
//! Usage: probe [name filter]

#[cfg(windows)]
fn main() {
    use tandem::conpty::{self, Source, dev_vendor_dir};

    let filter = std::env::args().nth(1).unwrap_or_default();
    let mut shipped_failed = false;
    println!("| Scenario | ConPTY | Result | Detail |");
    println!("|---|---|---|---|");
    for source in [Source::Shipped(dev_vendor_dir()), Source::Inbox] {
        let shipped = matches!(source, Source::Shipped(_));
        let api = match conpty::load(&source) {
            Ok(api) => api,
            Err(e) => {
                println!("| load | {} | FAIL | {} |", source.label(), e.replace('|', "/"));
                shipped_failed |= shipped;
                continue;
            }
        };
        for o in tandem::probe::run_all(&api, &filter) {
            let result = if o.pass { "pass" } else { "FAIL" };
            let detail = o.detail.replace('|', "/").replace('\n', " ");
            println!("| {} | {} | {result} | {detail} |", o.name, o.conpty);
            shipped_failed |= shipped && !o.pass;
        }
    }
    std::process::exit(if shipped_failed { 1 } else { 0 });
}

#[cfg(not(windows))]
fn main() {}
```

- [ ] **Step 4: Run the unit test to see it pass**

Run: `scripts/xt test --lib probe`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Run the scenario**

```bash
scripts/xt build --release --bins
scripts/xt run --release --bin probe -- win-console
```

Expected: a two-row table (shipped, inbox). A FAIL row is a finding, not a crash. Copy the table into the task report.

- [ ] **Step 6: Commit**

```bash
git add src/probe.rs src/bin/probe.rs src/bin/mouse_echo_win.rs src/lib.rs
git commit -m "Add probe harness and Windows console mouse scenario"
git push
```

---

### Task 6: WSL raw mouse scenarios

**Files:**
- Create: `probe/mouse_echo.py`
- Modify: `src/probe.rs` (add `hex`, `wsl_raw_mouse`, and five entries in `run_all`)

**Interfaces:**
- Consumes: everything Task 5 produces.
- Produces: scenarios `wsl-1000-sgr`, `wsl-1002-sgr`, `wsl-1003-sgr`, `wsl-1000-legacy`, `wsl-1000-legacy-col150`.

- [ ] **Step 1: Write the WSL helper**

`probe/mouse_echo.py`:

```python
#!/usr/bin/env python3
"""Probe helper: turns on the given DEC modes, then prints each escape sequence it
receives as a hex line. Quits on 'q' or after 5 s with no input."""
import os
import select
import sys
import termios
import tty

ESC = b"\x1b"
modes = sys.argv[1:]
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
tty.setraw(fd)
try:
    for m in modes:
        os.write(1, f"\x1b[?{m}h".encode())
    os.write(1, b"READY\r\n")
    idle = 0
    while idle < 50:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            idle += 1
            continue
        idle = 0
        data = os.read(fd, 1024)
        if data == b"q":
            break
        # One line per sequence, so several events in one read stay separate.
        for part in data.split(ESC):
            if part:
                os.write(1, f"GOT {(ESC + part).hex()}\r\n".encode())
finally:
    for m in modes:
        os.write(1, f"\x1b[?{m}l".encode())
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)
```

- [ ] **Step 2: Write the failing unit test for `hex`**

Add inside `mod tests` in `src/probe.rs`:

```rust
    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(b"\x1b[<0;1;1M"), "1b5b3c303b313b314d");
    }
```

Run: `scripts/xt test --lib probe`
Expected: compile error `cannot find function hex in this scope`.

- [ ] **Step 3: Write `hex` and the scenario**

Add to `src/probe.rs`, below `press_release`:

```rust
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A raw WSL program turns on `dec` modes and prints what it receives. Every event
/// the encoder produces must arrive byte for byte.
fn wsl_raw_mouse(api: &Api, dec: &[&str], want: Modes, col: u32, row: u32) -> Result<String, String> {
    let script = wsl_path(&repo_dir().join("probe").join("mouse_echo.py"));
    let mut s = Session::start(api, &format!("wsl.exe -d {DISTRO} -e python3 {script} {}", dec.join(" ")))?;
    if !s.pump_until(Duration::from_secs(10), |sc| sc.text().contains("READY") && sc.mouse_modes() == want) {
        return Err(format!(
            "terminal saw modes {:?}, wanted {want:?}. Screen: {:?}",
            s.screen.mouse_modes(),
            s.screen.text().trim_end()
        ));
    }
    let none = Mods::default();
    let events = [
        Event { button: Button::Left, action: Action::Press, mods: none, col, row },
        Event { button: Button::Left, action: Action::Motion, mods: none, col: col + 1, row },
        Event { button: Button::None, action: Action::Motion, mods: none, col: col + 2, row },
        Event { button: Button::Left, action: Action::Release, mods: none, col: col + 1, row },
        Event { button: Button::WheelUp, action: Action::Press, mods: none, col, row },
    ];
    let mut expected = Vec::new();
    for ev in events {
        if let Some(bytes) = mouse::encode(want, ev) {
            s.send(&bytes)?;
            expected.push(format!("GOT {}", hex(&bytes)));
        }
    }
    let all = s.pump_until(Duration::from_secs(5), |sc| {
        let text = sc.text();
        expected.iter().all(|e| text.contains(e.as_str()))
    });
    s.send(b"q")?;
    if all {
        return Ok(format!("{} events arrived intact", expected.len()));
    }
    let text = s.screen.text();
    let missing: Vec<&String> = expected.iter().filter(|e| !text.contains(e.as_str())).collect();
    let got: Vec<&str> = text.lines().map(str::trim_end).filter(|l| l.starts_with("GOT")).collect();
    Err(format!("missing {missing:?}; the app got {got:?}"))
}
```

In `run_all`, replace the `let mut list` line with:

```rust
    let sgr = |tracking| Modes { tracking, sgr: true };
    let legacy = Modes { tracking: Tracking::Click, sgr: false };
    let mut list: Vec<(&str, Scenario)> = vec![
        ("win-console", Box::new(win_console_mouse)),
        ("wsl-1000-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1000", "1006"], sgr(Tracking::Click), 10, 5))),
        ("wsl-1002-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1002", "1006"], sgr(Tracking::Drag), 10, 5))),
        ("wsl-1003-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1003", "1006"], sgr(Tracking::Any), 10, 5))),
        ("wsl-1000-legacy", Box::new(move |a| wsl_raw_mouse(a, &["1000"], legacy, 10, 5))),
        // Legacy bytes past 127 are not valid UTF-8. This row measures what ConPTY does with them.
        ("wsl-1000-legacy-col150", Box::new(move |a| wsl_raw_mouse(a, &["1000"], legacy, 150, 5))),
    ];
```

- [ ] **Step 4: Run the unit test to see it pass**

Run: `scripts/xt test --lib probe`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Run the scenarios**

```bash
scripts/xt build --release --bins
scripts/xt run --release --bin probe -- wsl-
```

Expected: ten rows (five scenarios, two ConPTY copies). Copy the table into the task report. `wsl-1000-legacy-col150` may fail on either copy. That row exists to measure it.

- [ ] **Step 6: Commit**

```bash
git add probe/mouse_echo.py src/probe.rs
git commit -m "Add WSL raw mouse scenarios for every tracking mode"
git push
```

---

### Task 7: vim, tmux, and htop scenarios

**Files:**
- Create: `probe/vim_target.txt`
- Modify: `src/probe.rs` (add `wsl`, `vim_click`, `tmux_wheel`, `htop_mouse`, three entries in `run_all`)

**Interfaces:**
- Consumes: everything Task 5 and Task 6 produce.
- Produces: scenarios `vim-click`, `tmux-wheel`, `htop`.

- [ ] **Step 1: Install htop in WSL**

```bash
ssh extreme 'wsl -d Ubuntu-Personal -u root -e apt-get install -y htop'
```

Expected: `htop` installs, or apt says it is already the newest version.

- [ ] **Step 2: Write the vim target file**

`probe/vim_target.txt`:

```
line 01 alpha
line 02 bravo
line 03 charlie
line 04 delta
line 05 echo
line 06 foxtrot
line 07 golf
line 08 hotel
line 09 india
line 10 juliet
```

- [ ] **Step 3: Write the scenarios**

Add to `src/probe.rs`, below `wsl_raw_mouse`:

```rust
/// Runs a command in WSL outside ConPTY and returns its stdout.
fn wsl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("wsl.exe")
        .args(["-d", DISTRO, "-e"])
        .args(args)
        .output()
        .map_err(|e| format!("wsl.exe: {e}"))?;
    if !out.status.success() {
        return Err(format!("wsl {args:?} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A click in vim moves the cursor. vim then writes the cursor position to a file.
fn vim_click(api: &Api) -> Result<String, String> {
    wsl(&["rm", "-f", "/tmp/tandem_vim_pos"])?;
    let file = wsl_path(&repo_dir().join("probe").join("vim_target.txt"));
    let mut s = Session::start(
        api,
        &format!("wsl.exe -d {DISTRO} -e vim -u NONE -N -c \"set mouse=a ttymouse=sgr\" {file}"),
    )?;
    if !s.pump_until(Duration::from_secs(10), |sc| {
        sc.text().contains("line 01") && sc.mouse_modes().tracking != Tracking::Off
    }) {
        return Err(format!("vim never turned on mouse tracking; modes {:?}", s.screen.mouse_modes()));
    }
    let modes = s.screen.mouse_modes();
    // Column 4, row 2 (0-based) is line 3, column 5 in vim.
    press_release(&s, modes, 4, 2)?;
    std::thread::sleep(Duration::from_millis(300));
    s.send(b":call writefile([line('.') . ',' . col('.')], '/tmp/tandem_vim_pos')\r:qa!\r")?;
    s.pty.wait_exit(Duration::from_secs(5)).ok_or("vim did not exit")?;
    let pos = wsl(&["cat", "/tmp/tandem_vim_pos"])?;
    if pos.trim() == "3,5" {
        Ok(format!("cursor moved to 3,5; modes {modes:?}"))
    } else {
        Err(format!("cursor at {:?}, wanted 3,5; modes {modes:?}", pos.trim()))
    }
}

/// With tmux mouse on, a wheel-up over a pane enters copy mode.
fn tmux_wheel(api: &Api) -> Result<String, String> {
    // The probe uses its own tmux socket. No server running is the normal case here.
    let _ = wsl(&["tmux", "-L", "tandemprobe", "kill-server"]);
    let mut s = Session::start(
        api,
        &format!("wsl.exe -d {DISTRO} -e tmux -L tandemprobe -f /dev/null new-session -s probe ; set -g mouse on"),
    )?;
    let on = s.pump_until(Duration::from_secs(10), |sc| sc.mouse_modes().tracking != Tracking::Off);
    let modes = s.screen.mouse_modes();
    let result = if !on {
        Err(format!("tmux never turned on mouse tracking; modes {modes:?}"))
    } else {
        let ev = Event { button: Button::WheelUp, action: Action::Press, mods: Mods::default(), col: 10, row: 10 };
        s.send(&mouse::encode(modes, ev).ok_or("the encoder dropped the wheel event")?)?;
        let end = Instant::now() + Duration::from_secs(3);
        let mut in_mode = String::new();
        while Instant::now() < end {
            in_mode = wsl(&["tmux", "-L", "tandemprobe", "display", "-p", "-t", "probe", "#{pane_in_mode}"])?;
            if in_mode.trim() == "1" {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if in_mode.trim() == "1" {
            Ok(format!("wheel entered copy mode; modes {modes:?}"))
        } else {
            Err(format!("pane_in_mode is {:?} after the wheel; modes {modes:?}", in_mode.trim()))
        }
    };
    let _ = wsl(&["tmux", "-L", "tandemprobe", "kill-server"]);
    result
}

/// htop turns on mouse tracking when it starts.
fn htop_mouse(api: &Api) -> Result<String, String> {
    wsl(&["which", "htop"]).map_err(|_| "htop is not installed in WSL (Task 7 step 1)".to_string())?;
    let mut s = Session::start(api, &format!("wsl.exe -d {DISTRO} -e htop"))?;
    let on = s.pump_until(Duration::from_secs(10), |sc| sc.mouse_modes().tracking != Tracking::Off);
    let modes = s.screen.mouse_modes();
    s.send(b"q")?;
    s.pty.wait_exit(Duration::from_secs(5)).ok_or("htop did not exit on q")?;
    if on { Ok(format!("modes {modes:?}")) } else { Err("htop never turned on mouse tracking".into()) }
}
```

Append to the `vec![...]` in `run_all`, after the `wsl-1000-legacy-col150` entry:

```rust
        ("vim-click", Box::new(vim_click)),
        ("tmux-wheel", Box::new(tmux_wheel)),
        ("htop", Box::new(htop_mouse)),
```

- [ ] **Step 4: Run the scenarios**

```bash
scripts/xt build --release --bins
scripts/xt run --release --bin probe -- vim
scripts/xt run --release --bin probe -- tmux
scripts/xt run --release --bin probe -- htop
```

Expected: two rows each. Copy the tables into the task report.

- [ ] **Step 5: Commit**

```bash
git add probe/vim_target.txt src/probe.rs
git commit -m "Add vim, tmux, and htop mouse scenarios"
git push
```

---

### Task 8: Full run and results

**Files:**
- Create: `docs/probe/2026-09-23-m0-results.md` (use the actual run date in the name)
- Modify: `docs/superpowers/specs/2026-09-23-tandem-design.md` (only if a result changes the design)

- [ ] **Step 1: Run every test and the full probe**

```bash
scripts/xt test
scripts/xt build --release --bins
scripts/xt run --release --bin probe > /tmp/claude-probe-table.md; echo "exit $?"
```

Expected: all unit tests pass. The probe exits 0 if every shipped row passes.

- [ ] **Step 2: Write the results file**

`docs/probe/<date>-m0-results.md` holds:

1. A one-line heading: `# Milestone 0 probe results, <date>`.
2. The machine: `EXTREME, Windows build <from [Environment]::OSVersion>, ConPTY package 1.24.260710001`.
3. The table, pasted verbatim from `/tmp/claude-probe-table.md`.
4. One short paragraph per FAIL row: what failed, under which ConPTY copy, and what that means for milestone 1.

- [ ] **Step 3: Decide**

- Every shipped row passes: milestone 0 is done. The shipped ConPTY carries the mouse, and milestone 1 can build on these modules.
- Any shipped row fails: **stop.** Do not patch around it in the encoder or the probe. Report the failing rows and the screen text to Robert, and wait for his decision. A fix is a design change, and it goes into the spec first.
- Inbox-only failures: record them. They support the spec's choice to ship ConPTY, and they need no action.

- [ ] **Step 4: Commit**

```bash
git add docs/probe/
git commit -m "Record milestone 0 probe results"
git push
```
