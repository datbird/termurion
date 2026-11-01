# P0 Compile Everywhere Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `cargo check --all-targets` passes on Linux (x86_64) and macOS (arm64) with every module un-gated, and every Windows test still passes.

**Architecture:** A new `src/platform/` layer holds one file per facility (`platform/notify.rs`, `platform/pty.rs`, ...). Each file declares the facility's free functions and types once and picks a backend with `#[cfg]`: `platform/windows/` wraps today's Win32 code (moved, not rewritten), `platform/unix/` holds the shared Unix backend. In P0 a Unix backend returns `platform::Unsupported` unless a portable crate already does the job. App code calls `crate::platform::<facility>::...` and holds no `#[cfg(windows)]`.

**Tech Stack:** Rust 2024, winit 0.30, wgpu 30, egui 0.36, muda 0.20, rfd 0.17, arboard 3, windows / windows-sys (Windows only), libc (new, Unix only).

**Spec:** `docs/superpowers/specs/2026-10-31-macos-linux-port-design.md`

## Global Constraints

- Windows behavior unchanged. Every task keeps the Windows build and its tests green (`scripts/xt test --lib`).
- No second copy of any logic. Mac and Linux share `platform/unix/`; `platform/macos/` and `platform/linux/` are created only when a backend really differs (not in P0).
- No `#[cfg(windows)]` in `src/app/` or other app modules as a way to "make it compile". A cfg lives only in `src/platform/`, `src/lib.rs` (for `*_win` modules that stay Windows-only), `Cargo.toml`, and `src/bin/`.
- Unix stubs return `platform::Unsupported`, never a silent no-op, and the UI shows it where a feature is used.
- The embedded browser (`webhost_win.rs`) and UI Automation (`uia_win.rs`) stay Windows-only modules; their callers reach them through `platform::browser` and `platform::a11y`, which report `Unsupported` on Unix. (The spec named a cargo feature; a target cfg inside the platform layer gives the same result with no feature flag to forget. Flag to Robert.)
- Windows-only extras with no Unix meaning (jump lists, taskbar progress, DLL hardening, default terminal handoff, UNC rules) go behind platform calls that are `Ok(())` no-ops on Unix with a doc comment saying why. This is the one allowed no-op class.
- Builds: Linux checks run on sirtoolio (`~/.cargo`); macOS checks run on `ssh funtimes` (arm64, rustc 1.99.1); Windows runs through `scripts/xt`.
- Short human comments, no em dashes or emoji anywhere, no Claude attribution in commits.

## Review Focus

1. A Windows code path that moved behind `platform/` changes behavior (a dropped flag, a changed error string a test or the UI matches on). Expect: identical behavior; every move is cut and paste plus a re-export.
2. A Unix stub that returns `Ok` with nothing done for a feature that matters (PTY, clipboard, credentials, notifications). Expect: `Unsupported`, shown in the UI.
3. Linux passes but macOS fails (winit/muda/rfd/arboard differ on macOS, `libc` constants differ, `std::os::linux` used where `std::os::unix` is meant). Expect: every task checks both hosts.
4. `#[cfg(windows)]` creeping into `src/app/`. Expect: `scripts/cfg-audit` reports zero hits outside allowed paths.
5. Unix-side `cargo test --lib` panics at test start because a test assumes `C:\` paths. P0 does not fix these tests, but it records them; they must not be hidden with `#[cfg(windows)]` on the test without a note in `docs/port/unix-test-gaps.md`.

---

### Task 1: Platform skeleton, Unsupported, check scripts

**Files:**
- Create: `src/platform/mod.rs`, `src/platform/windows/mod.rs`, `src/platform/unix/mod.rs`
- Create: `scripts/check-unix` (bash: `cargo check --all-targets` locally, then rsync to funtimes and run the same there; prints `linux: N errors`, `macos: N errors`)
- Create: `scripts/cfg-audit` (bash: greps `cfg(windows)`/`cfg(not(windows))` outside the allowed paths, exits 1 on any hit, prints `file:line`)
- Modify: `src/lib.rs` (add `pub mod platform;`), `Cargo.toml` (add `libc = "0.2"` under `[target.'cfg(unix)'.dependencies]`)

**Interfaces:**
- Produces: `pub struct Unsupported { pub feature: &'static str }` in `platform/mod.rs`, with `Display` = `"{feature} is not available on {OS_NAME} yet"`, `impl std::error::Error`, and `impl From<Unsupported> for String`. `pub const OS_NAME: &str` = `"Windows"`, `"macOS"` or `"Linux"`. `pub fn unsupported<T>(feature: &'static str) -> Result<T, Unsupported>`.

- [ ] **Step 1: Write the failing tests** in `platform/mod.rs`:
  `unsupported_names_feature_and_os`: `Unsupported { feature: "Notifications" }.to_string() == format!("Notifications is not available on {} yet", OS_NAME)`; `unsupported_into_string`: `String::from(Unsupported{feature:"X"})` contains `"X is not available"`.
- [ ] **Step 2:** `cargo test --lib platform` on Linux fails to build (whole lib fails today); run `scripts/xt test --lib platform`, expect FAIL (no module).
- [ ] **Step 3:** Implement the types; write both scripts. `check-unix` takes `--linux-only` / `--mac-only`.
- [ ] **Step 4:** `scripts/xt test --lib platform` PASS. `scripts/check-unix` runs and prints counts (still non-zero). Record the baseline in `docs/port/p0-ledger.md`.
- [ ] **Step 5:** Commit `Port P0: platform layer skeleton and Unix check scripts`.

### Task 2: Un-gate the OS-free UI and render modules

Modules: `render`, `egui_host`, `settings_ui`, `prompt_ui`, `note_ui`, `git_log_ui`, `status_popover_ui`, `web_popover`, `cmdinfo_ui`, `shot_ui`, `global_find_ui`, `open_quickly_ui`, `ssh_panel_ui`, `chat_window_ui`, `ai_engage_ui`, `ai_key_ui`, `script_deps_ui`, `script_export_ui`, `script_new_ui`, `script_console_ui`, `script_inspector_ui`, `replay_window`. These hold 0 to 6 Windows lines each.

**Files:**
- Modify: `src/lib.rs` (drop their `#[cfg(windows)]`), each module's Windows lines
- Create: `platform/<facility>.rs` for each facility those lines need (expected: `shell_open` (open URL/file), `dpi`, `clipboard` if any reach Win32 directly)

**Interfaces:**
- Consumes: `Unsupported` (Task 1).
- Produces: `platform::shell_open::open(target: &str) -> Result<(), String>` (Windows: today's ShellExecute path moved as is; Unix: `open` on macOS, `xdg-open` on Linux, through `std::process::Command`, since this one is portable and trivial). Other facility signatures copy the Windows function they replace, with Windows-only types (`HWND`, `HANDLE`) swapped for `winit::window::Window` refs or `isize` handles as the existing code already does.

- [ ] **Step 1:** Test `shell_open_refuses_empty_target` (`open("")` is `Err`) in `platform/shell_open.rs`.
- [ ] **Step 2:** Un-gate the modules; move each Windows line behind its facility.
- [ ] **Step 3:** `scripts/check-unix`: no errors reported in these files (other files may still fail). `scripts/cfg-audit` clean for them.
- [ ] **Step 4:** `scripts/xt test --lib` full PASS.
- [ ] **Step 5:** Commit.

### Task 3: IPC and the session host (winpipe, host, host_client)

**Files:**
- Create: `platform/ipc.rs`, `platform/windows/ipc.rs` (today's `winpipe.rs` moves here; `winpipe.rs` becomes a re-export so old paths keep working, then callers move and it is deleted), `platform/unix/ipc.rs`
- Modify: `src/host.rs`, `src/host_client.rs`, `src/api_server.rs`, `src/ssh_secret_win.rs` callers

**Interfaces:**
- Produces: `platform::ipc::{Pipe, Listener, connect, connect_owned, ConnectError, host_pipe_name, secret_pipe_name, url_pipe_name, own_peer, thread_peer}` with today's signatures. Win32-only helpers (`file_owner_sid`, `set_file_owner_to_user`, `user_sid`, `elevated`, `process_token_facts`, `session_of`) stay in `platform/windows/ipc.rs` and are reached through `platform::ipc::peer_check(...)`-style calls only if a caller outside Windows code needs them. Unix: every function returns `Unsupported { feature: "Session host" }` (the Unix socket is P1).

- [ ] **Step 1:** Test `unix_ipc_connect_is_unsupported` (cfg(unix) test in `platform/unix/ipc.rs`): `connect("x", 0s)` error string contains `"Session host is not available"`.
- [ ] **Step 2:** Move, re-export, un-gate `host`/`host_client` users.
- [ ] **Step 3:** `scripts/check-unix`: none of these files error. `scripts/xt test --lib` PASS (host tests included).
- [ ] **Step 4:** Commit.

### Task 4: PTY, ConPTY, job and process info

Modules: `conpty`, `pty`, `jobwatch`, `coproc_win`, `metrics_win`, `git_run_win`, `script_proc`, `probe`, `util_console`, `pane`.

**Files:**
- Create: `platform/pty.rs` (+ windows/unix backends), `platform/proc.rs` (process tree, foreground job, image path, cpu/mem metrics)
- Modify: the modules above; `pane.rs` (5432 lines, 5 Windows lines) talks only to `platform::pty`

**Interfaces:**
- Produces: `platform::pty::{Pty, Read, Notify, Transport, spawn_transport}` with today's signatures from `pty.rs`; `from_handoff` and `console_mode*` stay Windows-only inside `platform/windows/pty.rs` and are called only from Windows-only handoff code. Unix `spawn_transport` returns `Unsupported { feature: "Local shells" }`. `platform::proc::{foreground_job(pid) -> Option<Job>, process_image(pid) -> Option<String>, children(pid) -> Vec<u32>}`; Unix returns `None`/empty with a `Unsupported`-logged note via `dlog` once (job info is cosmetic; Review Focus 2 does not apply to it).

- [ ] **Step 1:** Test `unix_spawn_reports_unsupported` (cfg(unix)).
- [ ] **Step 2:** Move and un-gate.
- [ ] **Step 3:** `scripts/check-unix` clean for these files; `scripts/xt test --lib` PASS.
- [ ] **Step 4:** Commit.

### Task 5: Desktop services (clipboard, notifications, credentials, keys, shell extras)

Modules: `winclip`, `winprint`, `notify`, `pwm_credman`, `pwm_ui`, `winkeys`, `jumplist_win`, `taskbar_win`, `menu_tip_win`, `uia_win`, `webhost_win`, `display_watch`, `dragtext_win`, `firstmouse_win`, `svg_raster_win`, `sigarchive_win`, `ssh_probe_win`, `ssh_secret_win`, `dynamic_profiles_scan`, `scripts_scan`, `update_fetch_win`, `locale_win`, `api_server`, `ai_winhttp`, `backdrop`.

**Files:**
- Create: `platform/{clipboard,print,notify,credentials,keys,shell_extras,a11y,browser,display,locale,http}.rs` with backends
- Modify: `src/lib.rs` (the `*_win` modules stay gated; the rest un-gate)

**Interfaces:**
- Produces, Unix P0 backends: `clipboard` = `arboard` text plus `Unsupported` for HTML/RTF styled copy; `notify::show(title, body)` = `Unsupported { feature: "Notifications" }`; `credentials::{read,write,delete}` = `Unsupported { feature: "Password manager" }`; `keys::install_hook` = `Ok(())` (Cmd/Super come from winit modifiers on Unix, spec "Keys"); `shell_extras::{jump_list, taskbar_progress, dll_hardening}` = `Ok(())` no-ops; `a11y::start` and `browser::*` = `Unsupported`; `http` = `Unsupported { feature: "AI chat" }` until P1 picks a client (`ai_winhttp` is WinHTTP); `locale::user_locale` = `$LANG` parse.

- [ ] **Step 1:** Tests: `unix_notify_unsupported`, `unix_credentials_unsupported`, `locale_from_lang_env` (`"en_US.UTF-8"` -> `"en-US"`).
- [ ] **Step 2:** Move and un-gate.
- [ ] **Step 3:** `scripts/check-unix` clean for these files; `scripts/xt test --lib` PASS.
- [ ] **Step 4:** Commit.

### Task 6: Un-gate `app/` part 1: dialogs, window chrome, sizing

Files with the most Windows lines: `msgbox`, `windows_ui`, `chrome_ui`, `caption_ui`, `transparency_ui`, `sizing_ui`, `tab_drag_ui`, `mouse_ui`, `disclosure_ui`, `presence_ui`, `pane_move_ui`, `app/mod.rs`.

**Files:**
- Create: `platform/window.rs` (+ backends): raw-handle ops today done on `HWND` (dark title bar, cloak, transparency/blur, z-order, flash, monitor info, cursor position)
- Create: `platform/dialog.rs`: `message(title, body, kind) -> Answer`; Windows = today's `msgbox.rs` path; Unix = `rfd::MessageDialog`
- Modify: `src/lib.rs` (`app` un-gated), the files above

**Interfaces:**
- Produces: `platform::window::{set_dark_title(&Window, bool), set_cloaked(&Window, bool), set_blur(&Window, f32), flash(&Window), monitor_work_area(&Window) -> Option<Rect>}` (exact set = what the call sites use; keep Windows signatures). Unix backends: winit equivalents where winit has them (`set_window_level`, `request_user_attention`, `set_blur`, monitor via `current_monitor`), else `Unsupported`, shown through `platform::dialog::message` only when the user asked for the feature.

- [ ] **Step 1:** Test `dialog_answer_maps_buttons` (pure mapping of `Kind` to button set, shared by both backends).
- [ ] **Step 2:** Move and un-gate `app`.
- [ ] **Step 3:** `scripts/check-unix`: errors remain only in `app/` files Task 7 owns. `scripts/cfg-audit` clean for Task 6 files. `scripts/xt test --lib` PASS.
- [ ] **Step 4:** Commit.

### Task 7: Un-gate `app/` part 2 and the binaries

The remaining `app/` files (`pwm_app`, `logout_ui`, `settings_export_ui`, `hotkey_ui`, `statusbar_content`, `semantic_ui`, `requests_ui`, `tmux_ui`, `scripts_app`, `files_ui`, `dynamic_profiles_app`, `triggers_ui`, `tmux_tabs_ui`, `ssh_panel_app`, `screenshot_ui`, `replay_ui`, `menu`, `dlog_ui`, and any left), plus `src/bin/*`.

**Files:**
- Create: `platform/hotkey.rs` (Windows = today's RegisterHotKey path; Unix = `Unsupported { feature: "Hotkey window" }`), others as call sites need
- Modify: `src/bin/termurion.rs`, `termurion-host.rs`, `termurion-git.rs`, `termurion-utils.rs`, `cc-status.rs`: on Unix they build and run; the host binary prints the `Unsupported` text for the session host and exits 1
- Create: `docs/port/unix-test-gaps.md`; modify `docs/ROADMAP.md` (P0 done, P1 next)

- [ ] **Step 1:** Move and un-gate.
- [ ] **Step 2:** `scripts/check-unix` prints `linux: 0 errors`, `macos: 0 errors`. `scripts/cfg-audit` exits 0.
- [ ] **Step 3:** `cargo test --lib --no-run` builds on Linux; run `cargo test --lib` once on Linux and list failing tests by cause in `docs/port/unix-test-gaps.md` (not fixed in P0).
- [ ] **Step 4:** `scripts/xt test --lib` full PASS and `scripts/xt build --release` succeeds.
- [ ] **Step 5:** On Linux under Xvfb, `cargo run --bin termurion` opens a window and shows the "Local shells is not available on Linux yet" message (screenshot to the ledger).
- [ ] **Step 6:** Commit, push.
