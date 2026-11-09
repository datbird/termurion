# Render plugin effects in other AI agents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The render plugin effects find the input box of Codex and opencode (a shaded band) as they find Claude Code's line box, and tests built from real recorded screens pin all four agents.

**Architecture:** `src/scene.rs` gains a band box detector next to `box_line`; `FxPane.input_box` becomes a struct carrying the box kind and x edges; a new uniform `iInputBoxX` carries the x edges; `render_fx::patch_input_box` (which already feeds grid box lines to all 13 shaders) gains the band handling, so the 13 shader files need no hand edits unless a task says so.

**Tech Stack:** Rust, wgpu, naga GLSL.

**Spec:** `docs/superpowers/specs/2026-11-09-other-ai-agents-design.md` (read it whole; the recorded screens are in `docs/superpowers/specs/agent-screens/`, cursors in `cursors.txt` there).

## Global Constraints

- Detection is by screen shape only. Never check a program's name.
- Claude Code, Hermes and agy (line boxes) must render exactly as before: the existing render plugin tests stay green.
- Comments are short, plain English. No em dashes, no emoji in code, comments, docs or commits.
- Commits: no Claude attribution, no Co-Authored-By line, no host or user names.
- `source ~/.cargo/env` before cargo. Never run tests against the real keyring, never set `TERMURION_PAM_TEST`, run GUI tests only on their own Xvfb display, kill only PIDs you started, leave nothing running, write nothing into the real `~/.local/share`.
- The full Linux suite has 152 documented failures. A task passes when its own tests pass and the failure count does not grow. Three tests (`platform::linux::secrets::tests::a_dismissed_prompt...`, `a_hung_service...`, `trigger_eval::tests::banner_lines_skip_triggers`) flake under the parallel run; rerun them alone before counting them.

## Review Focus

1. vim with `cursorline`, mc, htop, less and a tmux status bar must get no box (one-row bands, full-screen color, bands without the cursor).
2. Codex's sent prompts in the history share the composer's color; only the band with the cursor counts.
3. opencode paints the whole screen `#0a0a0a`, so "different" must mean different from the band's surroundings.
4. A line box and a band box both present: the line box wins.
5. Band edges must not light outside the band's columns (opencode's panel is not full width).

---

### Task 1: Band box detection and plumbing

**Files:**
- Modify: `src/scene.rs` (`box_line` area near line 647, the `input_box` closure near line 1581, `FxPane.input_box` near line 149, tests)
- Modify: `src/render_fx.rs` (`Uniforms.input_box` near line 372, new `input_box_x`, the bytes writer near line 451)
- Modify: `src/render_fx_prefix.glsl` (new `vec4 iInputBoxX;` after `iInputBox`, end of `Globals`)
- Modify: `src/render.rs` (the four `u.input_box = ...` sites near lines 2998, 3124, 3209, 3270), `src/app/fx_ui.rs` (line 164)
- Create: `tests/fixtures/agents/` with copies of the `.ansi` files and `cursors.txt` from the spec folder

**Interfaces:**
- Produces in `src/scene.rs`:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub enum BoxKind { Lines, Band }
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub struct InputBox { pub top: f32, pub bottom: f32, pub kind: BoxKind, pub left: f32, pub right: f32 }
  impl InputBox {
      /// iInputBox: top, bottom, z = 1 lines or 2 band, 0.
      pub fn uniform(&self) -> [f32; 4];
      /// iInputBoxX: left and right edge in pixels for a band, zeros for lines.
      pub fn uniform_x(&self) -> [f32; 4];
  }
  /// The band box around the cursor cell: (top row, bottom row, first col, last col, bottom is a half-block edge row).
  fn band_box(term: &alacritty_terminal::Term<crate::screen::Listener>, display_offset: usize, row: usize, col: usize) -> Option<(usize, usize, usize, usize, bool)>;
  ```
  `FxPane.input_box: Option<InputBox>`. For `Lines`, `left`/`right` are 0 and `top`/`bottom` keep today's values (middle of each line row). For `Band`, `top` is the top pixel of the first band row, `bottom` is the bottom pixel of the last band row or the middle of a `▀`/`▄` edge row, `left`/`right` the pixel edges of the band's columns.
- `Uniforms` gains `pub input_box_x: [f32; 4]`, written as `iInputBoxX`.

- [ ] **Step 1:** Copy the fixtures to `tests/fixtures/agents/`. Write a test helper that loads `NAME.ansi`, feeds it into a `Screen::new(100, 30)` (rows joined with `\r\n`, starting with `\x1b[2J\x1b[H`, a reset `\x1b[0m` at each row start), then moves the cursor to the `cursors.txt` position, and returns the `FxPane` from `pane_with` with an fx cursor set (follow `painted_cursor_tests::frame_of`).
- [ ] **Step 2: Write the failing tests** in a new `agent_box_tests` module:
  - `hermes_and_agy_get_a_line_box`: `hermes`, `hermes-after`, `agy` give `kind == Lines`.
  - `codex_gets_a_band_box_with_the_cursor`: `codex` and `codex-after` give `Band`; on `codex-after` the band's top row is the row above the cursor row's band, not the history band at rows 12-13.
  - `opencode_gets_a_band_box_with_its_edge`: `opencode` and `opencode-after` give `Band`, the bottom edge at the middle of the `▀` row, left/right at the panel's columns.
  - `opencode_band_is_found_on_any_screen_color`: replace `48;2;10;10;10` with `48;2;60;20;20` in `opencode`, still `Band` with the same edges.
  - `no_box_for_other_screens`: each gives `input_box == None`: a one-row full-width shaded row with the cursor in it (vim `cursorline`); the whole screen one background color with the cursor in it; a 13-row shaded block with the cursor in it; a shaded tmux status row on the last row with the cursor on it; a plain `$ ` prompt.
  - `a_line_box_wins_over_a_band`: a Claude-style line box whose middle rows are also shaded gives `Lines`.
- [ ] **Step 3:** Run `cargo test agent_box_tests`. Expected: compile errors or failures.
- [ ] **Step 4: Implement** `band_box` per the spec's band rules (run of one background color at least half the columns with the cursor inside, rows grow up and down while the run covers the same columns within 2 cells, 2 to 12 rows and at most half the screen, the row above and below differ over the band's columns or are the screen edge, an edge row of `▀`/`▄` whose fg is the band color joins as a half row, colors compared as resolved RGB so palette and truecolor match). Line box first, then band. Plumb `InputBox` through all sites listed above; `iInputBoxX` goes at the end of `Globals`.
- [ ] **Step 5:** Run `cargo test agent_box_tests painted_cursor_tests render_fx`. Expected: PASS. Then the full suite; failure count does not grow.
- [ ] **Step 6: Commit** `Render plugins: find a shaded input box (Codex, opencode)`.

### Task 2: Painted cursor in a band

**Files:** Modify `src/scene.rs` (`painted_cursor`, near line 656; `painted_cursor_tests`).

**Interfaces:** Consumes `band_box` from Task 1. `painted_cursor` keeps its signature.

- [ ] **Step 1: Write the failing tests** in `painted_cursor_tests`:
  - `a_hidden_cursor_follows_a_painted_cell_in_a_band`: the `codex` fixture with `\x1b[?25l` and one `\x1b[7m \x1b[0m` cell after `hello` gives the fx cursor at that cell and a `Band` box.
  - `two_painted_cells_in_a_band_give_no_cursor`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3: Implement:** keep the line box path; if it finds nothing, for each reverse-video cell on screen try `band_box` at that cell; the cursor is the one reverse cell inside exactly one band (none when two cells or two bands match).
- [ ] **Step 4:** Run `cargo test painted_cursor_tests agent_box_tests`. Expected: PASS.
- [ ] **Step 5: Commit** `Render plugins: follow a painted cursor in a shaded box`.

### Task 3: Shaders light a band's edges

**Files:**
- Modify: `src/render_fx.rs` (`patch_input_box` near line 198, its test near line 948)
- Modify: `src/render_fx_prefix.glsl` (band helpers)
- Modify: the 13 `resources/render_plugins/*/*-cursor.glsl` only if a patch cannot reach what is needed, or for `clChevron` (Step 6)
- Test: `src/render.rs` (after `every_effect_lights_both_box_lines_on_enter`, near line 3232)

**Interfaces:** Consumes `InputBox::uniform()` / `uniform_x()` and `iInputBoxX`.

- [ ] **Step 1:** Run `cargo test every_effect_lights_both_box_lines_on_enter` on this host first and record whether it passes. If it fails before any change, find out why (fonts, GPU) and say so in the report; the new tests below follow whatever makes it run.
- [ ] **Step 2: Write the failing tests** in `src/render.rs`, built like `every_effect_lights_both_box_lines_on_enter` but from the Task 1 fixtures:
  - `every_effect_lights_a_bands_edges_on_enter`: `codex` and `opencode-after`, every built-in plugin, Enter at 1000 ms: pixels near the band's top edge and bottom edge differ from the plain frame (same threshold as the existing test); pixels in band rows away from both edges by more than half a row do not differ more than they do with no box (`iInputBox` zeros).
  - `a_band_lights_only_inside_its_columns`: `opencode` (the 75% panel): no lit pixels on the edge rows left of the panel's left edge or right of its right edge.
  - `line_tint_draws_a_bands_edges`: LINE_TINT on, no signal: both edge rows show the effect's line color inside the band's columns.
  - `prompt_marks_of_codex_and_agy_change_their_look`: with PROMPT_EFFECT on, the `›` cell of `codex` and the `>` cell of `agy` differ from the plain frame, for every built-in plugin that has PROMPT_EFFECT.
  - Extend `the_input_box_patch_reaches_every_builtin` for the new band patch text.
- [ ] **Step 3:** Run them. Expected: FAIL (band edges unlit; `›` may already pass, which is fine).
- [ ] **Step 4: Implement** in `patch_input_box` and the prefix: for `iInputBox.z > 1.5`, the patched `clLineNear`/`boxLineY`/`boxLines` answer the band edges only for x inside `iInputBoxX.x..iInputBoxX.y` (pixel coordinates follow `Y_DOWN` as the existing patch does); `linesLook`'s two early exits are skipped for a band; the line tint for a band draws a thin line (about a `─` glyph's thickness, soft edges) in the effect's line color on each edge instead of tinting ink. Patch by matching each shader's lines, as the existing patch does.
- [ ] **Step 5:** Run the Step 2 tests plus `every_effect_lights_both_box_lines_on_enter` and the `render_fx` tests. Expected: PASS, and the line box test unchanged.
- [ ] **Step 6:** If `prompt_marks_of_codex_and_agy_change_their_look` still fails for `›`, loosen `clChevron` only as far as `›` needs, in every shader that has it, and keep refusing letters, digits, `-`, `*`, `$`. Rerun.
- [ ] **Step 7: Commit** `Render plugins: light a shaded input box's edges`.

### Task 4: Labels and tester checklist

**Files:** Modify `src/render_fx.rs` (lines 26-31), `resources/render_plugins/README.md`, `src/ui_labels.rs` only if a test ties to the group text.

- [ ] **Step 1:** Keep the setting keys. Change the group label `"Claude Code"` (last field, lines 26-31) to `"Claude Code and other agents"`, and the descriptions: Done `"Done surge (an agent finished, a long command ended)"`, Line Color `"Input box lines (or a shaded box's edges) in the effect's color"`, Live Lines `"Input box lines (or a shaded box's edges) always alive"`, Prompt `"Prompt mark (\u{276f}, \u{203a}, >) look"`. Update any test that pins the old text.
- [ ] **Step 2:** Add a `## Testing with AI agents` section to `resources/render_plugins/README.md`: for Claude Code, Codex, opencode, Hermes Agent and agy, start the agent in Termurion with Shockwave on, type a word, press Enter, wait for the answer, and confirm: the cursor ring; both box lines (Claude Code, Hermes, agy) or both edges of the shaded box (Codex, opencode) light on Enter and take the effect's color; the prompt mark changes look (not opencode); the done surge when the answer is in. Name the versions recorded in the spec.
- [ ] **Step 3:** Run `cargo test render_fx ui_labels`. Expected: PASS.
- [ ] **Step 4: Commit** `Render plugins: name other agents in the effect settings, add a tester checklist`.

### After the tasks (controller)

- Final review on Opus, then a Mac build with `scripts/mac-bundle`, a Tart (funtimes) check with his old config: Shockwave on, a replayed Codex and opencode screen, LOOK at the screen; then the zip to his Downloads under a new name.
