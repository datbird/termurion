# Tandem: an iTerm2-style terminal for Windows

Status: draft for review, 2026-09-23. "Tandem" is a working name.

## 1. Goal

Robert uses iTerm2 on his Mac. He wants a Windows terminal that matches iTerm2 one to one:
every feature he uses today, and the ones he does not use yet. He tried Ghostty and does not
like it as much. iTerm2 stays the terminal on the Mac.

Revised 2026-09-24. Robert: "I dont care if there is a mechanism to keep the settings synced.
Thats not the point. The point is being able to one-to-one match the current features and even
others I am not using yet." Settings sync is therefore not a goal.

### Success criteria

1. Every iTerm2 feature in `docs/parity/` has a Windows equivalent in Tandem, or a written
   reason why the feature cannot exist on Windows (for example the Touch Bar).
2. Someone who knows iTerm2 finds each setting, menu command, and shortcut where they expect it,
   with Cmd mapped as in section 3.4.
3. The mouse works the iTerm2 way in plain shells, tmux, vim, and htop.

### Scope

- The target is the whole feature list in `docs/parity/`: profiles and settings, menus, tmux
  integration, shell integration, triggers, status bar, toolbelt, Instant Replay, the password
  manager, Composer, the Python API, AI features, and the rest.
- Tandem has its own settings window, laid out like iTerm2's. Importing an iTerm2 settings file
  is an optional one-time convenience, not a sync.
- Not in scope: a Mac or Linux build, and code copied from Windows Terminal or iTerm2 (the
  ConPTY DLL is the one exception, section 3.1).

## 2. Architecture

Tandem is one Rust program with five parts. Each part has one job and a small interface.

| Part | Job | Built on |
|---|---|---|
| `pty` | Starts shells and moves bytes to and from them | ConPTY |
| `core` | Parses escape codes, holds the screen and scrollback | `alacritty_terminal` crate |
| `render` | Draws cells, cursor, selection, and images on the GPU | wgpu and glyphon (cosmic-text) |
| `ui` | Windows, tabs, splits, mouse, keys, hotkey window | winit plus direct Win32 calls |
| `settings` | Tandem's own profiles and settings, plus an optional iTerm2 import | TOML files, `plist` crate for import |

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

Superseded 2026-09-24. Tandem keeps its own profiles and global settings in `%APPDATA%\Tandem`,
edited in a settings window laid out like iTerm2's (Profiles, Appearance, Keys, Pointer,
Arrangements, Advanced). A one-time import can read an iTerm2 `com.googlecode.iterm2.plist` or
Dynamic Profiles JSON. The full list of settings to match is in `docs/parity/01-profile-settings.md`
and `docs/parity/02-global-settings.md`.

Errors in a settings file keep the last good settings and show a banner with the parse error.

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

Milestones 0 (ConPTY and mouse probe) and 1 (terminal basics) are done. The roadmap from
milestone 2 on is in `docs/parity/00-roadmap.md`, ordered from the feature inventory.

## 7. Licensing

- `alacritty_terminal` is Apache-2.0. The ConPTY DLL is MIT. Both allow a private or public app.
- iTerm2 is GPL. Tandem reads iTerm2's file format and copies its behavior, but no iTerm2 code.
  If we copy iTerm2 code later (for example the default smart selection rules), Tandem becomes GPL.

## 8. Open items for review

1. **Cmd mapping.** Cmd becomes Ctrl+Shift. Bumbles shares one keyboard between the Mac and Windows,
   so a different mapping may fit his hands better.
2. **Name.** "Tandem" is a working name.
