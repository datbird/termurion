<div align="center">

<img src="docs/assets/termurion/termurion-mark.png" width="128" alt="Termurion logo">

# Termurion

**iTerm2, on Windows. All of it.**

A native Windows terminal, written in Rust, that aims at full 1:1 parity with iTerm2:
the same settings, the same menus, the same mouse, the same tmux integration, the same Python API.

![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4?style=flat-square)
![Language](https://img.shields.io/badge/Rust-2024%20edition-B7410E?style=flat-square)
![Renderer](https://img.shields.io/badge/renderer-wgpu-5b8cff?style=flat-square)
![Status](https://img.shields.io/badge/status-beta-7a5cff?style=flat-square)

[Why Termurion](#why-termurion) ·
[Features](#features) ·
[Screenshots](#screenshots) ·
[How it compares](#how-it-compares) ·
[Architecture](#architecture) ·
[Building](#building) ·
[Credits](#credits-and-licenses)

</div>

<p align="center"><img src="docs/assets/screenshots/settings-profiles.png" width="900" alt="Termurion settings window open over a PowerShell session"></p>

---

## Why Termurion

If you live in iTerm2 on a Mac, every Windows terminal feels like a step down. Some have tabs and
splits. A few have a drop-down window. None of them has tmux control mode, triggers, the toolbelt,
Instant Replay, or iTerm2's mouse.

Termurion does not borrow iTerm2's look. It ports iTerm2's behavior. Every profile setting, global
setting, menu item, and feature in iTerm2 is a row in a parity inventory, and each milestone closes
rows until they read `have`. Where iTerm2 has a quirk, Termurion keeps it and writes it down.

- **Your Mac settings, as they are.** Termurion reads iTerm2's own plist. Profiles, colors, keys,
  triggers, and arrangements come across without a conversion step.
- **Your scripts, unchanged.** The stock `iterm2` pip package connects to Termurion and works.
- **Your muscle memory.** iTerm2's menu bar, key actions, and pointer actions, with Ctrl+Shift in place of Cmd (or Win, your pick).

## Features

<table>
<tr>
<td width="50%" valign="top">

### Terminal
- Native ConPTY sessions, with the current OpenConsole shipped beside the exe
- GPU renderer on wgpu, ligatures, box drawing, bidi, and true color
- Inline images: OSC 1337 File, sixel, the kitty graphics protocol, animated GIFs
- Every keyboard protocol: CSI u, modifyOtherKeys, and kitty flags with key releases
- Mouse reporting that works in vim, tmux, and htop, on WSL and native shells
- Sessions in `termurion-host.exe`, so shells outlive a crashed window

</td>
<td width="50%" valign="top">

### tmux integration
- `tmux -CC` control mode: tmux windows become real Termurion windows and tabs
- Pause and resume, the tmux dashboard, and option monitors
- Edits, moves, clipboard, client lists, and persistence
- The tmux status bar inside Termurion's own status bar

</td>
</tr>
<tr>
<td valign="top">

### Shell integration
- OSC 133 marks, OSC 7 directories, and the 1337 shell keys
- A PowerShell script, plus iTerm2's own installer for bash and zsh
- Command history, recent directories, prompt navigation, and alerts on finish
- Automatic Profile Switching by host, user, path, and job

</td>
<td valign="top">

### Triggers and automation
- Triggers in iTerm2's stored form, with every action iTerm2 lists
- Coprocesses, badges, Windows notifications, and idle and output alerts
- Broadcast input across panes, tabs, and windows
- Smart Selection and Semantic History, with all of their actions

</td>
</tr>
<tr>
<td valign="top">

### Windows and sessions
- Hotkey windows, including the drop-down style
- Window arrangements in iTerm2's plist form, plus restoration
- Every window style, Lock Size, Lock Layout, and Undo Close
- Tab groups, tab themes, pane title bars, transparency, and virtual desktops

</td>
<td valign="top">

### Tools
- The toolbelt with all twelve built-in tools
- Status bar with every component: clock, git, CPU, memory, network, and more
- The Composer, Open Quickly, Instant Replay, and Copy Mode
- Find, Find Globally, Filter, folds, annotations, and named marks

</td>
</tr>
<tr>
<td valign="top">

### Scripting
- iTerm2's Python API server, protocol 1.19, on the loopback only
- Script functions, the expression language, and the Scripts menu
- The Script Console and the Scripting Inspector
- `imgcat`, `it2dl`, `it2copy`, and the other utilities, built for Windows

</td>
<td valign="top">

### AI, SSH, and the browser
- AI chat, inline help, and the Cockpit, with Anthropic, Gemini, DeepSeek, and Ollama
- Claude Code integration through a `cc-status` hook
- iTerm2's SSH integration: the conductor, file transfers, and `ssh:` URLs
- A WebView2 browser pane with find, Copy Mode, marks, triggers, and reader mode
- A password manager on Windows Credential Manager, 1Password, and LastPass

</td>
</tr>
</table>

## Screenshots

<table>
<tr>
<td width="50%"><img src="docs/assets/screenshots/menu-shell.png" alt="The Shell menu: windows, tabs, splits, archives, workgroups, broadcast input, tmux and ssh"></td>
<td width="50%"><img src="docs/assets/screenshots/menu-view.png" alt="The View menu: toolbelt, annotations, composer, timestamps, Instant Replay"></td>
</tr>
<tr>
<td width="50%"><img src="docs/assets/screenshots/settings-appearance.png" alt="Settings, Appearance: tabs, panes and window options"></td>
<td width="50%"><img src="docs/assets/screenshots/settings-keys.png" alt="Settings, Keys: key bindings, shortcut key and modifier remapping"></td>
</tr>
</table>

iTerm2's menu bar, settings window and key bindings, on Windows. The Shortcut Key (iTerm2's Cmd) is Ctrl+Shift by default.

## How it compares

Termurion is the only Windows terminal we found that has the full iTerm2 set. The deciding feature
is native tmux control mode, which no other Windows terminal ships.

| | Hotkey window | Shell integration | Triggers | tmux `-CC` | Reads iTerm2 settings |
|---|:-:|:-:|:-:|:-:|:-:|
| Windows Terminal | Yes | Yes | No | No | No |
| WezTerm | With Lua | Yes | Partial | No | No |
| Tabby | Yes | Partial | No | No | No |
| Warp | Yes | Yes | No | No | No |
| **Termurion** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** |

## Architecture

```mermaid
flowchart LR
    subgraph Host["termurion-host.exe"]
        CP["ConPTY<br/>(OpenConsole)"] --> SH["Shells: pwsh, cmd, WSL, ssh"]
    end
    subgraph App["termurion.exe"]
        PTY["Own ConPTY layer"] --> EMU["alacritty_terminal<br/>(patched core)"]
        EMU --> R["wgpu renderer<br/>(glyphon text)"]
        UI["iTerm2 UI<br/>menus, toolbelt, status bar"] --> R
        TMUX["tmux gateway"] --> EMU
        API["Python API server<br/>:1912 loopback"] --> UI
        WV["WebView2<br/>browser pane"] --> UI
    end
    Host <--> PTY
    PL["iTerm2 plist"] --> UI
    Scripts["iterm2 pip package"] <--> API
```

- **Core.** `alacritty_terminal` 0.26 parses and stores the grid. Termurion carries a small set of
  patches for marks, row time, folds, image tiles, and blocks. Each patch is listed in
  `crates/alacritty_terminal/TERMURION-PATCHES.md`.
- **ConPTY.** Termurion has its own ConPTY code. The shipped `conpty.dll` and `OpenConsole.exe` load
  from beside the exe, never from the inbox console host by accident.
- **Safety.** Every program links the C runtime statically and loads its imports from System32
  only. Downloads, imports, and escape sequences that can act go through trust gates first.

## Building

Termurion builds on Windows with the MSVC Rust toolchain.

```powershell
# 1. Fetch the pinned ConPTY package (checked by SHA-256)
.\scripts\fetch-conpty.ps1

# 2. Build
cargo build --release

# 3. Run
.\target\release\termurion.exe
```

The build copies `conpty.dll` and `OpenConsole.exe` next to the exe. The browser pane needs the
WebView2 runtime, which Windows 10 and 11 normally have.

`.cargo/config.toml` sets `+crt-static`. The build stops if that flag is missing.

### Repository layout

| Path | What it holds |
|---|---|
| `src/` | The app, about 520 modules |
| `src/bin/` | `termurion`, `termurion-host`, `termurion-utils`, `termurion-git`, `cc-status`, and the probes |
| `crates/` | Patched copies of `alacritty_terminal`, `vte`, and `glyphon` |
| `resources/` | Shell integration, color presets, tab icons, terminfo, and AI prompts |
| `proto/`, `schema/` | iTerm2's API protocol and the settings schema |
| `docs/parity/` | The parity inventory: 1,797 rows across profile, global, and feature settings |
| `docs/acceptance/` | One hands-on checklist per milestone |
| `docs/superpowers/` | Specs, plans, and rulings for each milestone |
| `tests/` | Integration tests and fixtures |

## Credits and licenses

Termurion is free software under the GNU General Public License v3 or later. See [`LICENSE`](LICENSE).

Termurion stands on these projects:

- [iTerm2](https://github.com/gnachman/iTerm2) by George Nachman, the reference for every
  behavior. Files copied from iTerm2 (color presets, script templates, AI prompts, tips, and the
  utilities) are used under iTerm2's GPL v2 or later. See [`resources/LICENSE`](resources/LICENSE).
- [alacritty_terminal](https://github.com/alacritty/alacritty), [vte](https://github.com/alacritty/vte),
  and [glyphon](https://github.com/grovesNL/glyphon), under their own licenses in `crates/`.
- [Windows Terminal](https://github.com/microsoft/terminal) for ConPTY and OpenConsole.
  The notices are in `resources/conpty/`.

Termurion shows every bundled license at runtime, gathered at build time into one list.

<div align="center">
<sub>Built for people who miss iTerm2 every time they sit down at a Windows machine.</sub>
</div>
