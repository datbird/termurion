# macOS and Linux ports: design

Date: 2026-10-31. Robert's direction: "I want a linux and mac port of termurion", Mac first,
"with the side thought that we will soon roll out a linux version as well, so that any work
that can be avoided doubling should be done". He was away when this was written, so the parts
marked **(call)** are my decisions for him to review.

## Intent

One Termurion on Windows, macOS and Linux, from one source tree. Same features, same settings
format, same iTerm2-compatible keys. On the Mac it is an iTerm2-like app he controls (and the
home of the render plugins); on Linux it is the same app.

## Goals

- macOS (Apple silicon first) build that runs his daily setup: profiles, tabs, splits, tmux
  integration, triggers, shell integration, render plugins, ssh.
- Linux (x86_64, X11 and Wayland through winit) build right after, sharing every Unix part.
- Windows behavior unchanged. Every step keeps the Windows build and its tests green.

## Non-goals (first release of each port)

- The embedded browser (`webhost_win.rs`, WebView2) and UI Automation accessibility: behind a
  cargo feature, off on Unix at first **(call)**. They come back later through `wry` and
  `accesskit`.
- Windows-only extras with no Unix meaning: jump lists, taskbar progress, DLL hardening,
  default terminal handoff, UNC path rules.

## Anti-goals

- No second copy of any logic. If Mac and Linux need the same thing, it lives once in
  `platform/unix`.
- No `#[cfg(windows)]` sprinkled through `app/` as a way to "make it compile". Platform calls go
  through the platform layer, so the app code reads the same on every OS.

## Measured starting point (2026-10-31)

- 496k lines in `src/`. About 65% of the files have no OS calls at all.
- `src/lib.rs` gates 59 modules with `#[cfg(windows)]` (about 172k lines), including all of
  `app/` (128k lines). So today a Unix build has an empty `main`.
- `windows_sys` appears on 555 lines in 127 files, `windows::` on 282 lines in 76 files.
- The cross-platform crates are already the base: winit 0.30, wgpu 30, egui 0.36, glyphon,
  muda, rfd, arboard, alacritty_terminal. Windows crates are already target-gated in
  `Cargo.toml`.

## Shape: one platform layer

```
src/platform/
  mod.rs        traits + the `current()` re-exports used by the app
  windows/      the existing Win32 code, moved behind the traits
  unix/         shared by macOS and Linux (written once)
  macos/        macOS-only pieces
  linux/        Linux-only pieces
```

| Facility | Shared Unix (written once) | macOS only | Linux only |
|---|---|---|---|
| PTY | `openpty` + `fork`/`exec`, `TIOCSWINSZ`, `SIGCHLD` | | |
| Session host daemon | Unix domain socket in the runtime dir, mode 0600, peer uid check; same `host_proto` | launchd agent **(call)** | systemd user unit or plain double fork **(call)** |
| Foreground job, process tree | `tcgetpgrp` on the pty | `libproc` (`proc_pidinfo`, `proc_pidpath`) | `/proc` |
| Paths | XDG / home logic | `~/Library/Application Support/Termurion` | `$XDG_CONFIG_HOME/termurion` |
| Clipboard | `arboard` (already used) | | primary selection on X11 and Wayland |
| Open URL / file | | `open` | `xdg-open` |
| Credentials | `keyring` crate | Keychain, Touch ID gate | Secret Service |
| Notifications | | `UNUserNotificationCenter` | `notify-rust` (D-Bus) |
| Global hotkey, hotkey window | `global-hotkey` crate | Carbon hotkeys inside it | X11 grab; Wayland portal later |
| Menus | `muda` (already used) | `init_for_nsapp`, the real menu bar | egui-drawn menu bar **(call: muda needs GTK, which fights winit)** |
| Crash reports | signal handler writes the log | | |
| Printing | render to PDF | print panel later | `lp` |
| Install | | `.app` bundle, ad hoc signed, then notarized later | AppImage first **(call)** |

Keys: on macOS, Cmd is the real Cmd key, so the Win-key hook (`winkeys.rs`) is not needed and
the iTerm2 key tables apply as they are. On Linux, Super plays the Cmd role, as on Windows,
but without a low-level hook at first **(call)**: winit reports Super with its modifiers.

Settings: on macOS, Termurion can read iTerm2's live plist from
`~/Library/Preferences/com.googlecode.iterm2.plist` (the existing two-way plist code applies).

## Order of work

1. **P0 Compile everywhere.** Add `platform/` with traits and Windows impls that wrap today's
   code. Un-gate `app/` and the gated modules one at a time; each Windows call site moves
   behind a trait. Unix impls start as clear `Unsupported` errors, shown in the UI where a
   feature is used, never silent. Exit: `cargo check` passes on the Linux and macOS build hosts, and all Windows tests pass.
2. **P1 Unix core** (shared): PTY, host daemon over a Unix socket, job tracking, paths,
   clipboard, open URL. Exit: on Linux and on the macOS VM, a window opens, runs zsh/bash, tabs
   and splits work, tmux `-CC` attaches, restore after restart works.
3. **P2 macOS.** Menu bar, Cmd keys, hotkey window, notifications, Keychain, `.app` bundle,
   iTerm2 plist import. Exit: Robert runs it on the Mac.
4. **P3 Linux.** Menu bar (egui), notifications, Secret Service, AppImage. Exit: runs on a Linux desktop or VM.
5. **Later:** browser (`wry`), accessibility (`accesskit`), printing panels, notarization.

## Build and test hosts

- Linux: a Linux build host with rustup. `cargo check` and the pure
  test suite run locally; GUI checks run under Xvfb.
- macOS: a macOS VM (Tart) with rustup, not a daily-use Mac.
- Windows: the Windows build host through `scripts/xt`, as today.
- CI **(call)**: a GitHub Actions matrix (Windows, macOS, Ubuntu) running `cargo check` and the
  pure tests on push. It is not a scheduled job.

## Left to the builder

Trait method names, the split of each module, how `Unsupported` is shown per feature.
