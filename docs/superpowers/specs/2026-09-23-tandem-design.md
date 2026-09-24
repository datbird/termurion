# Tandem: an iTerm2-style terminal for Windows

Status: draft for review, 2026-09-23. "Tandem" is a working name.

## 1. Goal

Robert uses iTerm2 on his Mac. He wants his Windows terminal to look and behave the same,
so he does not switch habits between machines. He tried Ghostty and does not like it as much.

Tandem is a new Windows-only terminal app. It copies iTerm2 behavior and reads iTerm2's
own settings file. iTerm2 stays the terminal on the Mac.

### Success criteria

1. A profile, color, or font change in iTerm2 on the Mac shows up in Tandem without manual steps.
2. The mouse works the iTerm2 way in plain shells, tmux, vim, and htop. No lost clicks, no stuck selections.
3. `tmux -CC` shows tmux windows and panes as native Tandem tabs and splits.
4. One global key shows and hides a drop-down Tandem window.
5. iTerm2 triggers and shell integration work: marks, current directory, inline images, badges.

### Not in scope

- A Mac or Linux build. iTerm2 already covers the Mac.
- iTerm2's Python API, status bar, toolbelt, Instant Replay, AI features, and password manager.
- A full settings editor. iTerm2 on the Mac is where settings get edited (see section 4).
- Any code from Windows Terminal. The one exception is the ConPTY DLL (section 3.1).

## 2. Architecture

Tandem is one Rust program with five parts. Each part has one job and a small interface.

| Part | Job | Built on |
|---|---|---|
| `pty` | Starts shells and moves bytes to and from them | ConPTY |
| `core` | Parses escape codes, holds the screen and scrollback | `alacritty_terminal` crate |
| `render` | Draws cells, cursor, selection, and images on the GPU | wgpu and glyphon (cosmic-text) |
| `ui` | Windows, tabs, splits, mouse, keys, hotkey window | winit plus direct Win32 calls |
| `settings` | Reads iTerm2's settings file and Windows overrides | `plist` crate |

Two feature parts sit on top of `core` and `ui`:

| Part | Job |
|---|---|
| `integrate` | Triggers, OSC 133 marks, OSC 1337 (images, badges, user vars), OSC 7 current directory |
| `tmuxcc` | The tmux control-mode client |

### Data flow

```
shell <-> ConPTY <-> pty --bytes--> core --screen--> render --> window
                                     |                  ^
                                     +--> integrate ----+  (marks, highlights, images)
ui (mouse, keys) --encoded input--> pty
settings --profiles--> ui, render, integrate (live reload on file change)
tmuxcc: one pty carries the control channel, tmuxcc drives many core instances
```

Each tab or split pane owns one `core` instance. In normal mode each `core` has its own `pty`.
In tmux mode, one `pty` runs `tmux -CC` and `tmuxcc` feeds every pane's `core` from it.

## 3. Components

### 3.1 pty

- Uses ConPTY through the `windows` crate.
- Ships `conpty.dll` and `OpenConsole.exe` from Microsoft's ConPTY NuGet package (MIT) beside the exe.
  Pinned version: `Microsoft.Windows.Console.ConPTY` 1.24.260710001.
  The copy of ConPTY built into Windows lags behind and has a history of mouse input bugs.
  The shipped copy gets the newest fixes. (Current ConPTY has no separate "passthrough mode" flag.
  Its only creation flags are cursor inheritance and glyph width.)
- `conpty.dll` starts `OpenConsole.exe` from its own folder. If that exe is missing, ConPTY quietly uses
  the Windows copy. Tandem checks that both files exist before it loads the DLL.
- Tandem has its own ConPTY code, not the one in `alacritty_terminal`. The Alacritty code falls back
  to the Windows copy without saying so, which breaks the next rule.
- If the shipped DLL fails to load, Tandem says so in a visible banner, then uses the built-in ConPTY.
  It does not switch without telling you.
- Shells come from the profile's Windows override (section 4.2). Default: `pwsh.exe`.

### 3.2 core

- `alacritty_terminal` (Apache-2.0) provides the parser, grid, scrollback, and selection model.
- Tandem adds parsing for sequences Alacritty ignores: OSC 1337, OSC 133, and the iTerm2 image protocol.
  These go through a hook ahead of the core parser, so the Alacritty crate stays unmodified.
- If an upstream change blocks the hook, we vendor the crate and record why in the repo.

### 3.3 render

- wgpu on the DX12 backend draws a grid of cells from a glyph atlas.
- glyphon (cosmic-text and swash) shapes and rasterizes glyphs, with font fallback and emoji. It uses
  grayscale smoothing, which is closer to iTerm2's CoreText look than ClearType. (Changed from DirectWrite
  in milestone 1; DirectWrite stays an option if text looks wrong on Windows.)
- Supports iTerm2 profile visuals: ANSI and bright colors, cursor style and color, bold and italic fonts,
  line spacing, window transparency, and blur (DWM acrylic).
- Draws inline images from `core` image cells.

### 3.4 ui

Window, tabs, and splits:

- Tabs and a tab bar drawn by Tandem, styled like iTerm2's minimal and compact themes.
- Vertical and horizontal splits with draggable dividers. Dimming of inactive panes, as in iTerm2.

Keys:

- iTerm2 uses Cmd for app shortcuts. Tandem maps Cmd to **Ctrl+Shift** by default
  (for example Cmd+T becomes Ctrl+Shift+T, Cmd+D becomes Ctrl+Shift+D).
  Plain Ctrl keys stay with the shell. A Windows override can change the mapping.
- Option maps to Alt. The profile's "Left/Right Option key sends" setting applies to Alt.
- The profile's `Keyboard Map` entries apply, translated with the same modifier map.

Mouse (the top-priority feature):

| iTerm2 on Mac | Tandem on Windows |
|---|---|
| Drag selects text | Same |
| Option-drag selects even when the app holds the mouse | Alt-drag |
| Cmd-Option-drag selects a rectangle | Ctrl-Alt-drag |
| Double-click word, triple-click line, quad-click smart selection | Same, using the profile's smart selection rules |
| Cmd-click opens a URL or file path | Ctrl-click |
| Copy on selection (setting) | Same setting, honored |
| Middle-click pastes | Same |
| Wheel sends arrow keys in the alternate screen (setting) | Same setting, honored |
| Focus follows mouse (setting) | Same setting, honored |
| Right-click menu | Copy, Paste, Split, Open link |

- When an app turns on mouse reporting (modes 1000, 1002, 1003, with SGR 1006), clicks go to the app,
  except Alt-drag, which always selects.
- Every mouse event is encoded by one function with unit tests for each mode.

Hotkey window:

- `RegisterHotKey` claims the global key from the iTerm2 hotkey profile.
- The window is borderless and always on top. It slides in from the edge set in the profile.
- It hides when it loses focus, unless pinned. It floats over full-screen apps.
- If another program already owns the key, Tandem shows an error with the key name. It does not pick another key.

### 3.5 settings

See section 4.

### 3.6 integrate

Triggers:

- Reads the profile's `Triggers` list from the iTerm2 settings file.
- Runs each regex against each line when the line completes, with a per-trigger "instant" flag honored.
- Supported actions: Highlight Text, Post Notification (Windows toast), Set Mark, Run Command,
  Send Text, Open URL, Set User Variable. A trigger with any other action shows once in a warning list.
  It is not silently dropped.

Shell integration:

- OSC 133 A/B/C/D: prompt and command marks. Jump between prompts, select a command's output.
- OSC 1337: `CurrentDir`, `RemoteHost`, `SetUserVar`, `SetBadgeFormat`, `SetMark`, `File=` (inline images).
- OSC 7: current directory.
- Tandem ships a PowerShell integration script. iTerm2's own bash, zsh, and fish scripts
  work as they are inside WSL and over SSH.

### 3.7 tmuxcc

- Starts `tmux -CC` (or `ssh host -t tmux -CC`) in a pty and reads the control protocol:
  `%begin`/`%end`/`%error`, `%output`, `%window-add`, `%window-close`, `%layout-change`,
  `%session-changed`, `%exit`.
- Each tmux window becomes a Tandem tab. Each tmux pane becomes a Tandem split, placed from the layout string.
- Keys in a pane go out as `send-keys`. Resizing a Tandem split sends `resize-pane`.
- On attach, Tandem reads pane history with `capture-pane` so scrollback is present.
- Target: real tmux in WSL and over SSH. psmux (the tmux on EXTREME) may not support control mode.
  We check that at milestone 5. It is not a requirement.
- A protocol error shows the raw text in the tab and a clear message. Tandem does not guess.

## 4. Settings

### 4.1 Source of truth: iTerm2 on the Mac

iTerm2 can load and save its settings in a chosen folder
(Settings > General > Settings > "Load settings from a custom folder or URL").
We point that at a folder both machines sync. Tandem watches
`com.googlecode.iterm2.plist` in that folder and reloads on change.

Tandem reads these parts of the file:

- `New Bookmarks`: the profiles, by `Guid`. Colors (`Ansi 0 Color` and the rest), fonts (`Normal Font`,
  `Non Ascii Font`), cursor, transparency, blur, `Keyboard Map`, `Triggers`, `Smart Selection Rules`,
  and the mouse options.
- The default profile and the hotkey window profile.
- Global keys and mouse options.

Tandem never writes this file.

The sync transport is Syncthing by default, since it already runs on his machines.
The Mac is a work machine, so this needs his check (section 8).

### 4.2 Windows overrides

Some profile values cannot carry over, such as `/bin/zsh` or a font that is not installed on Windows.
A small file, `%APPDATA%\Tandem\overrides.toml`, fixes those per profile `Guid`:

```toml
[profile."5C9C1A5E-...-GUID"]
command = "pwsh.exe -NoLogo"
font = "MesloLGS NF"

[keys]
cmd = "ctrl+shift"
```

### 4.3 Errors

- A bad or half-written plist: Tandem keeps the last good settings and shows a banner with the parse error.
- A font that is not installed and has no override: a banner names the font. Tandem uses Consolas until it is fixed.
- Every banner stays until the problem is fixed or dismissed. Nothing fails silently.

## 5. Build and test

### Where things run

- The code lives in `~/gitrepos/tandem` on sirtoolio, in a private `datbird` repo.
- EXTREME builds and runs it (Rust stable, MSVC toolchain, already installed).
- The build pulls from the repo on EXTREME and runs `cargo build`.

### Tests

- Unit tests: mouse encoding for each mode, OSC 1337 and OSC 133 parsing, key mapping,
  the tmux control-mode parser (from recorded tmux sessions), and plist import (from a copy of his real settings file).
- ConPTY tests on EXTREME: start a shell, send input, check the screen contents.
- Acceptance: a checklist run side by side with iTerm2 on the Mac, one row per item in section 1.

## 6. Milestones

Risk goes first. Each milestone ends with something he can run.

| # | Milestone | Done when |
|---|---|---|
| 0 | ConPTY and mouse probe | A scripted probe (no window) runs a console app, tmux, vim, and htop under ConPTY, and every mouse mode passes with the shipped ConPTY |
| 1 | Terminal basics | Tabs, splits, renderer, keys, copy and paste |
| 2 | iTerm2 settings | Profiles, colors, and fonts load from the synced plist and reload live |
| 3 | Hotkey window | The global key shows and hides the drop-down window |
| 4 | Triggers and shell integration | Triggers, marks, badges, and inline images work |
| 5 | tmux -CC | tmux windows and panes appear as native tabs and splits |

## 7. Licensing

- `alacritty_terminal` is Apache-2.0. The ConPTY DLL is MIT. Both allow a private or public app.
- iTerm2 is GPL. Tandem reads iTerm2's file format and copies its behavior, but no iTerm2 code.
  If we copy iTerm2 code later (for example the default smart selection rules), Tandem becomes GPL.

## 8. Open items for review

1. **Settings sync.** Syncthing between the work Mac and EXTREME. Is that allowed on the work Mac,
   or should the sync go another way (for example a private git repo)?
2. **Cmd mapping.** Cmd becomes Ctrl+Shift. Bumbles shares one keyboard between the Mac and Windows,
   so a different mapping may fit his hands better.
3. **Name.** "Tandem" is a working name.
