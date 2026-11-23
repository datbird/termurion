# Cheap effects and full Metal on macOS: design

Date: 2026-11-23. Target release: 1.1.12 on Windows, macOS and Linux.

## Intent

An effect must be cheap enough that a person turns it on just for fun and
gives up nothing that matters. The owner's words: "at the heart I want this
to be really performant", and "so reasonable that if someone wants to use
just for fun they are not compromising in any major way performance."

## Where we are (measured on funtimes, 1.1.11, 2 passes)

All 13 effects cost about the same. The shader is not the cost. The fixed
work around each animation frame is the cost.

| config | idle CPU % | stream20 CPU % | claude CPU % | idle frames/s |
|---|---|---|---|---|
| Ghostty, its effect | 5.5 | 11.8 | 7.3 | 60.0 |
| Termurion, no effect | 0.7 | 12.6 | 7.5 | (no animation) |
| Termurion, 13 effects (range) | 8.4 to 9.6 | 12.6 to 16.8 | 11.3 to 12.8 | 27.4 to 27.9 |

CPU per animation frame: Termurion about 3.2 ms, Ghostty about 0.9 ms.
Termurion also draws only about 28 frames/s where Ghostty draws 60.

A profile shows the time goes to:

1. wgpu-core work on each submit (validation, tracking, encoder setup).
2. Copy passes around the effect (scene to effect input, effect output to
   the surface).
3. `App::render` bookkeeping that runs even when only the effect moved and
   no cell changed.

## Goals

1. Every row of the table above meets or beats Ghostty, with and without an
   effect, on macOS. On a row where a doable major change can beat Ghostty
   by a clear margin, take it.
2. Effects animate at the display rate (60 or 120 Hz), not about 28 frames/s,
   and still cost less CPU than now.
3. Windows and Linux get the shared effect fixes and a measured gain.
4. One matching 1.1.12 release on all 3 OSes, installed on EXTREME, W541 and
   the owner's Mac.
5. Docs reflect reality (see Documentation).

## Non-goals

- New effects, or new effect settings.
- Metal on Windows or Linux. They keep wgpu.
- Changes to the flood, parse or text paths that 1.1.11 shipped.

## Anti-goals (a failure even if it works)

- Any pixel change in any effect, or in plain text. Output stays
  byte-equal to 1.1.11.
- Any timing change: an effect keeps its speed, phase and look.
- A silent fallback. If Metal fails to start, the user sees an error. The
  app never quietly drops to another path.
- Faster numbers bought by a lower frame rate, a smaller surface or
  skipped frames the eye can see.

## Decisions the owner made

- macOS uses Metal for everything: text, cells, cursor, effects and
  present. ("everything on macOS, go full Metal")
- Windows and Linux are included, with matching releases on all 3 OSes.
- Target: the Ghostty numbers, unless a doable major change does better.
- The README is updated to reflect reality and spotlights the many days of
  optimization, performance and accurate-rendering work. It links a new
  `docs/PERFORMANCE.md` with tables that compare terminals and name no
  hardware.

## How (decided parts)

### macOS: full Metal

- A Metal renderer replaces wgpu on macOS for the whole frame.
- Effect GLSL keeps its source. naga translates it to MSL at build or load
  time, the same translation wgpu uses now. This keeps pixels byte-equal.
- Text: the glyph atlas and cell layout stay the same data. Only the
  submission layer changes.
- During the move, the wgpu path stays on macOS behind a hidden switch, so
  every bench and pixel test can run A/B. The switch and the wgpu macOS path
  are removed before 1.1.12 ships.
- A Metal start failure shows an error dialog with the reason.

### All OSes: shared effect fixes

- Draw the effect straight into the surface where the effect allows it, and
  cut the copy passes.
- An animation-only frame (no cell changed) skips the `App::render`
  bookkeeping and reuses the last frame's cell data.
- Remove whatever holds effects near 28 frames/s, so they follow the
  display rate under "Match the display refresh rate".

## Proof

- Pixel tests: every effect and plain text, compared byte for byte against
  1.1.11 output, on each OS (GL path on Linux CI, Metal on funtimes).
- Bench: `scripts/draw-bench` on funtimes with configs termurion, all 13
  `termurion-fx-*`, ghostty, ghostty-fx and iterm2, workloads idle,
  stream20, claude and flood. The 13-effect support in the scratchpad copy
  goes into the repo script.
- Windows and Linux: the same workloads, timed against the comparison
  terminals below.
- The owner's Mac: the 120 Hz check repeats (it was 25.3% with Lightning
  on 1.1.11).

## Documentation

- `README.md`: renderer badge and feature list match reality (Metal on
  macOS, wgpu on Windows and Linux). A short section spotlights the
  performance, memory and accurate-rendering work, with a link to
  `docs/PERFORMANCE.md`.
- `docs/PERFORMANCE.md` (new): one table per OS and workload, with and
  without an effect, CPU %, frames/s, MB/s and memory, Termurion against the
  other terminals. No hardware names. It also shows the Termurion history
  from 1.1.0 to 1.1.12 where we have numbers.
- `docs/FIXES.md`, `docs/ROADMAP.md`, help docs and installer: the usual
  version and entry updates.

## Builder's calls (marked as mine, the owner can change them)

- Comparison terminals. macOS: Ghostty and iTerm2. Windows: Windows Terminal
  and WezTerm. Linux: Ghostty, Alacritty and the desktop terminal (Ptyxis on
  the W541).
- PERFORMANCE.md includes the 1.1.0 to 1.1.12 history.
- naga MSL translation runs at build time if that is simpler and keeps
  plugin loading working for user plugins. Otherwise at load time.

## Left to the builder

- The Metal crate (metal-rs, objc2-metal or similar) and module layout.
- The order of work, as long as each step keeps pixels byte-equal and the
  A/B switch works until the end.
- How the animation-only frame is detected.
