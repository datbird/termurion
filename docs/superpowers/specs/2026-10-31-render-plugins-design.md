# Render plugins: design

Date: 2026-10-31. Status: approved direction from Robert ("a new native feature built in for
plugins to be able to add render functions ... fork the one in the reddit and bring it into the
project so there are built in examples"). Details below marked **(call)** are my decisions,
made while he was away; he can change them.

## Intent

Termurion gets a native way for plugins to add render passes: shaders that run over the
finished terminal frame, every frame, and can animate. The first users are the 13 cursor
effects of `ghostty-cursor-fx` (MIT, copyright 2026 AONIR), which ship inside Termurion as the
built-in examples. iTerm2 has no such feature, so this is a Termurion extra, not a parity item.

The feature must work unchanged on the coming macOS and Linux builds. It uses only wgpu and
naga, which Termurion already ships, so it has no Windows code.

## Goals

- Run Ghostty-style custom shaders (Shadertoy GLSL plus Ghostty's cursor uniforms) with no
  edits. A shader written for Ghostty works in Termurion.
- Ship the 13 cursor-fx effects as built-in plugins, with the license and credit kept.
- Let a person turn plugins on per profile, chain several, and tune each effect's settings.
- Keep the Claude Code hooks of cursor-fx working: an "enter" effect when a prompt is sent and
  a "done" effect when Claude finishes.
- Zero cost when no plugin is on: no offscreen texture, no extra pass, no extra redraws.

## Non-goals (for this version)

- No plugin code beyond shaders (no scripting inside a render pass).
- No Python API calls to change plugins **(call: later, once the shape settles)**.
- No iChannel1-3 textures, no mouse uniform (Ghostty leaves them unused too).

## Anti-goals

- A plugin must never make the terminal unreadable for good: a shader that fails to compile
  is skipped and the error is shown in a pane bar. It is never hidden behind a fallback.
- No battery drain when idle: animation stops when nothing changes (see Animation).

## Plugin format

A plugin is a folder:

```
<name>/
  plugin.toml
  <shader>.glsl  (or .wgsl)
  LICENSE        (optional)
```

`plugin.toml`:

```toml
name = "Lightning"
description = "Sparks while typing; a surge along the input on Enter."
shaders = ["lightning-cursor.glsl"]   # run in order
animation = "auto"                    # auto | always | off
cursor_color = "#9fd0fe"              # optional: the preset cursor color (cursor-fx sets one)
author = "AONIR"
license = "MIT"
```

Folders are read from:

- Built in: `resources/render_plugins/` in the install (read only).
- The person's own: `<config dir>/RenderPlugins/` (`%APPDATA%\Termurion\RenderPlugins` on
  Windows; the macOS and Linux config dirs come from the port's paths layer). A folder there
  with the same name as a built-in one overrides it. Tuning a built-in effect copies it here
  first **(call)**.

## Shader languages

- **GLSL (Ghostty/Shadertoy)**: the file defines `void mainImage(out vec4 fragColor, in vec2
  fragCoord)`. Termurion adds a prefix that declares Ghostty's uniform block and `iChannel0`,
  and a suffix with `main()`. naga's GLSL front end compiles it. Measured 2026-10-31: all 13
  cursor-fx shaders parse and validate with naga 30.0.1.
  - naga has no combined `sampler2D`, so the prefix declares a texture and a sampler and
    `#define iChannel0 sampler2D(iChannel0_tex, iChannel0_smp)`.
  - `main()` goes after the body (naga rejects a call to a function defined later).
- **WGSL**: a fragment entry point `fs_main` with the same bind group (documented in
  `docs/render-plugins.md`). For people who prefer wgpu's own language.

## Uniforms

The same block, names and std140 layout as Ghostty's `shadertoy_prefix.glsl`:
`iResolution, iTime, iTimeDelta, iFrameRate, iFrame, iChannelTime, iChannelResolution, iMouse,
iDate, iSampleRate, iCurrentCursor, iPreviousCursor, iCurrentCursorColor,
iPreviousCursorColor, iCurrentCursorStyle, iPreviousCursorStyle, iCursorVisible,
iTimeCursorChange, iTimeFocus, iFocus, iPalette[256], iBackgroundColor, iForegroundColor,
iCursorColor, iCursorText, iSelectionForegroundColor, iSelectionBackgroundColor`.

Coordinates follow Ghostty's y-down convention (its Metal path), because wgpu is y-down:
`fragCoord` has its origin at the top left, `iCurrentCursor.xy` is the cursor's left edge and
its bottom (+Y) edge in pixels, `.zw` its width and height. `iChannel0` is the frame as drawn,
sampled through a non-sRGB view, so sampled colors and the color uniforms are in the same
(sRGB-encoded) space. Offsets in the uniform buffer come from naga's own layouter for the
prefix, not hand-counted, so the packing can never drift from the declaration.

The cursor values update when the cursor cell, shape or color changes, as in Ghostty:
`previous = current`, `iTimeCursorChange = iTime`. `iTimeFocus` is set when the pane's window
gains focus. One render pass covers the whole window, so the cursor uniforms are the focused
pane's cursor **(call: Ghostty has one surface per split; per-pane passes can come later)**.

## Pipeline

1. Plugins on for the active profile: the renderer draws the frame into an offscreen texture
   (same size and format as the surface, created once per size).
2. Each shader runs as a full-screen triangle. Shader N reads shader N-1's output as
   `iChannel0`; the first reads the frame. The last writes to the surface.
3. Fades keep working: they apply to the frame before the plugin chain.

## Animation

- `auto` **(default, call)**: keep redrawing at the display rate for 2 seconds after the last
  cursor change, focus change, key press or output, then stop. cursor-fx effects are all
  timed from those events, so they finish inside that window. Idle panes cost nothing.
- `always`: redraw every frame while the window is focused (Ghostty's
  `custom-shader-animation = true`).
- `off`: draw only when the terminal draws.

## Settings

- Profile key `Render Plugins` (list of plugin names, in order; empty by default), on the
  Profiles > Window tab, with an "Effects" list: checkbox, order, and a "Settings..." button.
- Settings for an effect come from `const <type> NAME = value; // help` lines at the top of its
  shader, as cursor-fx does. The editor shows a control per line and writes the value back
  into the person's copy, then reloads the plugin live.

## Claude Code and shell signals

cursor-fx signals "enter" and "done" by setting the cursor color with the lowest bit of blue
or green flipped (OSC 12), then resetting it (OSC 112). The shaders read those bits from
`iCurrentCursorColor`. Termurion already honors OSC 12 and 112, so this works as is, also
through ssh and tmux. Termurion ships the hook side as plain scripts, because the hook runs on
the host where Claude Code runs (often a remote Mac or Linux box, not where Termurion is):

- `resources/render_plugins/hooks/fx-signal.sh` (POSIX sh, writes to `/dev/tty`).
- `resources/render_plugins/hooks/fx-signal.ps1` (Windows).
- A sample `claude-code-hooks.json` that calls them from `UserPromptSubmit` and `Stop`.

## Credit

The cursor-fx shaders keep their MIT license text and copyright line in
`resources/render_plugins/cursor-fx/LICENSE`. The About box and `docs/render-plugins.md` credit
the project and link it.

## Left to the builder

Exact UI layout of the Effects list, the error bar wording, and test structure.
