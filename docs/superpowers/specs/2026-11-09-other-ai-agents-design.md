# Render plugin effects in other AI coding agents

Date: 2026-11-09. Status: design, waiting for the owner's approval.

## Intent

The render plugin effects have Claude Code features: the input box lines light up, the prompt mark changes its look, Enter and done send a surge, and (since 279fe9f3) a painted cursor is followed. The owner wants the same effects in Codex, opencode, Hermes Agent and agy (Antigravity CLI). He can test only agy now. Testers may test the others later, so the design must work from what the tools really draw, and tests must pin that.

## What each tool draws

Recorded on 2026-11-09 in a private tmux (100x30, `TERM_PROGRAM=iTerm.app`, pane background `#101010` so Codex knows it and shades its box). The scrubbed screens are in `agent-screens/` next to this file, as `tmux capture-pane -e -N` output (trailing cells kept, so band widths are real). `NAME.ansi` is the screen with `hello` typed; `NAME-after.ansi` is the screen about 12 seconds after Enter (Codex and Hermes show an API error, opencode shows an answer from its free default model). `cursors.txt` holds each screen's cursor, 0-based. Home paths, the account name and request IDs were replaced, so rows that show a path are shorter than the real ones. The agy screen is the older capture without `-N`; its line box does not depend on trailing cells.

| Tool (version) | Real cursor | Input box | Prompt mark | Alt screen | Cursor (x, y) |
|---|---|---|---|---|---|
| Claude Code 2.1.319 | hidden on some accounts, a reverse-video cell painted instead | full-width `─` line above and below | `❯` | yes | |
| Hermes Agent 0.19.0 | shown | full-width `─` line above and below | `❯` | no | 7, 28 |
| agy 1.5.3 | shown | full-width `─` line above and below | `>` | no | 7, 8 |
| Codex 0.167.0 | shown | no lines: a 2-row band with background `#2c2c2c` across the full width, at the bottom; sent prompts in the history get the same band, without the cursor | bold `›` (U+203A) | yes | 7, 26 |
| opencode 1.19.5 | shown | no lines: a left bar `┃` in the accent color, a panel with background `#1e1e1e`, and a bottom edge of `▀` in the panel color, about 75% of the width on the home screen and nearly full width after Enter; the whole screen is painted `#0a0a0a` around it | none | yes | 21, 15 |

Codex shades its box only when the terminal answers the OSC 11 background query. Termurion answers it (`Event::ColorRequest`, `src/screen.rs`).

## How the effects find things now

- The cursor: `cursor_cell`, then `painted_cursor` (`src/scene.rs`). Generic for a shown cursor.
- Enter and done: `render_fx::Signals`. Enter is the Enter key. Done is the output going quiet after Enter, or OSC 133;D. Generic: all four tools animate a spinner or status while they work.
- The input box, two places:
  - Rust: `box_line` (a row whose cells are mostly a horizontal line glyph) and the `input_box` closure in `pane_with` give `iInputBox` (top and bottom line y, z = 1).
  - Each of the 13 shaders: `clLineNear` and `linesLook` find the lines by their pixels, and need a line just below the cursor.
- The prompt mark: each shader's `clMarkLeft` and `clChevron` find a chevron-shaped glyph (`❯` or `>`) left of the cursor, by its pixels.

## Result per tool with no change

- Hermes and agy: the same shape as Claude Code. All effects should already work. agy's `>` passes `clChevron` by its comment. Nothing pins this.
- Codex: cursor effects and Enter/done work. No box lines light. The `›` mark is smaller than `❯` and may fail `clChevron` (unknown).
- opencode: cursor effects and Enter/done work. No box lines light. No prompt mark exists.

## Goals

1. Every effect that works in Claude Code works in the four tools, as far as each screen allows.
2. Detection stays by screen shape. Termurion never checks a program's name (tmux hides it, and new tools appear).
3. Tests built from the recorded screens prove each tool's result, so a tester only needs to confirm.
4. A short tester checklist per tool.

## Non-goals

- Per-tool settings or a tool picker.
- Renaming the "Effects Claude ..." setting keys (saved profiles use them). Their descriptions may say "Claude Code and similar agents".
- A prompt mark for opencode (it draws none).

## Anti-goals (a failure even if the agents work)

- A shell, vim, less, htop, mc or a tmux status bar gets box effects it did not get before. In particular vim's `cursorline` (one shaded row around the cursor) and a full-width selection bar are not input boxes.
- The line-box tools (Claude Code, Hermes, agy) look different from today.
- A frame gets slower in a way the person can see.

## Design

### 1. Shaded box ("band box") in Rust

Add a second box shape next to `box_line` in `src/scene.rs`:

- "Different" means different from the cells around the band, not from the terminal's default background. opencode paints the whole screen `#0a0a0a`, and vim or mc color schemes paint full screens, so a check against the default background would match them.
- The cursor cell's background covers one run of at least half the columns on the cursor's row, with the cursor inside the run.
- The band grows up and down over rows whose same-color run covers the same columns (a few cells of difference allowed).
- The band is at least 2 rows and at most 12 rows high, and at most half the screen. One row is vim's `cursorline` or a selection bar, never a box. A taller block is a color scheme or a full-screen program.
- The row above the band and the row below it (or the screen edge) have a different background over the band's columns. A full-screen color fails this.
- An opencode-style edge row (`▀` or `▄` in the band color on the surrounding background, over the band's columns) belongs to the band as a half row, so the bottom edge is the middle of that row.
- A band without the cursor is not the input box (Codex's sent prompts in the history).
- A line box wins when both shapes match.

`iInputBox` keeps its meaning for a line box (z = 1). For a band box: x and y are the band's top and bottom edge in pixels, z = 2. A new uniform `vec4 iInputBoxX` holds the band's left and right edge in pixels (zero for a line box). It goes at the end of `Globals` in `src/render_fx_prefix.glsl` and in the Rust uniform struct, with the std140 rules that file's comments describe.

`painted_cursor` also accepts a band box, so a future tool that paints its cursor in a band works the same as Claude Code. With the real cursor hidden there is no cursor row to start from, so the band is searched around each reverse-video cell, and the cursor is the one such cell inside exactly one band.

### 2. Shaders

`render_fx::patch_input_box` already rewrites `boxLineY`, `clLineNear` and `boxLines` in every shader to return the `iInputBox` lines when `iInputBox.z > 0.5`. A band (z = 2) goes through the same patch, so its edges reach every shader as the box lines with no hand edits to the 13 files. Extend that patch, and add helpers to `src/render_fx_prefix.glsl`, for what a band needs on top:

- `LINE_EFFECT` and the Enter surge run along the band's top and bottom edges as they run along a line today, limited to the columns between `iInputBoxX.x` and `iInputBoxX.y` (opencode's panel is not full width).
- `linesLook`'s early exits skip background pixels and rows that are not a line across the window. For a band, these exits must not drop the edge pixels.
- `LINE_TINT`: a band has no line ink to tint, so the effect draws a thin line (about the thickness of a `─` glyph) in the effect's line color on each edge. The band then reads like Claude Code's colored box lines.
- The places in `src/render.rs` that build `iInputBox` as `[t, b, 1, 0]` from `FxPane.input_box` carry the kind and the x edges. `FxPane.input_box` becomes a small struct (top, bottom, kind, left, right).

`clChevron` must accept Codex's bold `›`. If it does not, loosen it only as far as `›` needs, and keep the letters, digits and signs its comment says it refuses.

### 3. Tests

- Fixture tests in `src/scene.rs`: feed each recorded screen from `agent-screens/`, before and after Enter, into a `Screen` (the capture text with cursor moves added, the cursor from `cursors.txt`), then check the box `pane_with` reports: Hermes and agy a line box around the cursor, Codex and opencode a band box with the right edges. On `codex-after`, the sent prompt's band in the history is not chosen. One opencode test repaints its `#0a0a0a` screen color as another color, to prove the band is found against its surroundings.
- Anti-goal tests: a vim-style one-row `cursorline`, a full-width selection bar with the cursor in it, a tmux status row, a full-screen color scheme with the cursor in it, a 13-row color block, and a plain shell prompt give no box.
- Render tests in `src/render.rs`, after `every_effect_lights_both_box_lines_on_enter`: with every effect on, the Codex and opencode screens light both band edges on Enter, and the Codex `›` and agy `>` marks change their look. "Lights" and "change their look" mean the same pixel check the existing test uses: the pixels differ from the frame with the effect off.
- The fixtures move to `tests/fixtures/agents/` (or the place the tests already use for fixtures) and the copy here stays as the record.

### 4. Tester checklist

Add a short section to the render plugins docs: for each tool, start it in Termurion with Shockwave on, type a word, press Enter, wait for the answer, and confirm the cursor ring, the box lines (or band edges), the prompt mark (not opencode) and the done surge.

## Left to the builder

- The exact band tolerances, if the anti-goal tests and the fixtures all pass.
- The helper names and how much shader code each plugin shares.
- How the band edge line is drawn (thickness, softness), if it looks like the line tint on Claude Code's lines.
- Whether the tester checklist covers the working state (the spinner) or a test does; the fixtures cover the screen at rest before and after Enter.
