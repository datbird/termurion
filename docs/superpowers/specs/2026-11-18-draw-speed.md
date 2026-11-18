# Draw speed: use less CPU and GPU than iTerm2 and Ghostty

Date: 2026-11-18. Status: design (phase 1 of the draw-speed work), waiting for the owner's
approval. No product code changed in this phase.

## Intent

The owner's goal: for the same work, Termurion uses less CPU and GPU than iTerm2, Ghostty and the
other terminals on his Mac, above all while Claude Code streams in a big window with the Lightning
effect on. "Do what it takes."

Known before this phase (his Mac, 1.1.8, window 3236x1762 pt on Retina, Lightning on, Claude
streaming): about 50% of a core. A 10 s `sample` put 1080 of the main thread's 4048 samples in
`draw_fx_for` (about 1250 counting the other paths into `Renderer::draw_with`), 387 in
`build_frame`, 106 in `pane_with`; the top of stack was glyphon's glyph cache (`HashMap` insert
124, `lru` get 79), the row hashes (`hash_parts` 101), `write_buffer` 157 and
`LayoutGlyph::physical` 54.

## How it was measured

`scripts/draw-bench` (committed with this spec) runs on funtimes, the macOS 26.6 VM (4 cores,
paravirtual GPU, a 1728x1084 screen at scale 1). For each terminal it writes the same output
straight to the terminal's tty for 20 s while the terminal is the front app, and measures over
exactly those seconds:

- CPU of the terminal's processes (Termurion with `termurion-host`, iTerm2 with `iTermServer`)
  per wall second, so 100% is one core;
- CPU of WindowServer, which composites every frame a terminal presents;
- frames presented: dtrace counts `-[CAMetalLayer nextDrawable]` (Termurion, iTerm2) or
  `-[CALayer setContents:]` (Ghostty draws to IOSurfaces);
- bytes the workload got into the tty (only the flood is limited by the terminal);
- the CPU of every other process (noise). A run with more than a quarter of a core of noise, or
  one where the terminal lost the front (the iMessage bridge pulls Messages forward at second 38
  of each minute), runs again; no run starts while the VM builds or runs a simulator.

GPU busy is not measured: the paravirtual GPU reports no utilization and `powermetrics` has no
sampler for it. WindowServer's CPU and the frame count stand in for it, and the shader's cost is
worked out per pixel below.

Workloads: **idle** (a prompt, cursor still), **stream20** (20 new lines a second),
**claude** (Claude Code's bottom area redrawn 10 times a second: a spinner row and a 3-row input
box with the cursor in it, and a new line of answer above them every other tick), **flood** (a
1 MB block of colored text written as fast as the terminal takes it). Window: 220 columns as tall
as the screen allows (Termurion 1595x977 pt, 56 rows; Ghostty 1548x977, 58 rows; iTerm2 1567x977,
55 rows), Monaco 12, cursor not blinking. Termurion 1.1.8, Ghostty 1.4.0, iTerm2 3.7.7. Lightning
in Termurion is its built-in plugin; in Ghostty it is the same GLSL file as a `custom-shader` with
`custom-shader-animation = always` (iTerm2 has no shaders).

## Results

Median of 2 passes (they agreed within 1.5 points), `target/draw-bench/run4`:

| terminal | workload | CPU % | WindowServer % | frames/s | MB/s |
|---|---|---|---|---|---|
| Termurion | idle | 2.1 | 1.5 | 2.0 | |
| Termurion | stream20 | 19.9 | 9.9 | 30.9 | |
| Termurion | claude | 15.2 | 10.6 | 28.2 | |
| Termurion | flood | 259.7 | 0.0 | 0.1 | 69.5 |
| Termurion + Lightning | idle | 8.8 | 13.2 | 27.2 | |
| Termurion + Lightning | stream20 | 21.2 | 10.1 | 31.0 | |
| Termurion + Lightning | claude | 17.4 | 12.1 | 28.8 | |
| Termurion + Lightning | flood | 258.1 | 0.1 | 0.2 | 70.0 |
| Ghostty | idle | 0.5 | 1.3 | 1.7 | |
| Ghostty | stream20 | 9.1 | 13.0 | 19.9 | |
| Ghostty | claude | 6.7 | 10.8 | 13.2 | |
| Ghostty | flood | 233.6 | 10.3 | 59.4 | 115.0 |
| Ghostty + Lightning | idle | 4.2 | 14.8 | 60.0 | |
| Ghostty + Lightning | stream20 | 9.3 | 14.4 | 60.0 | |
| Ghostty + Lightning | claude | 6.0 | 14.7 | 60.0 | |
| Ghostty + Lightning | flood | 235.2 | 10.3 | 60.0 | 115.6 |
| iTerm2 | idle | 2.0 | 0.8 | 1.2 | |
| iTerm2 | stream20 | 26.7 | 9.3 | 19.9 | |
| iTerm2 | claude | 23.1 | 10.1 | 10.0 | |
| iTerm2 | flood | 185.4 | 3.2 | 18.4 | 18.9 |

What it says:

- Termurion already beats iTerm2 on stream20 and claude, and loses to Ghostty by 2.2x to 2.3x on
  CPU. With Lightning it is 2.3x to 2.9x Ghostty with the same shader, although Ghostty draws
  twice as many frames (60 against 30).
- Termurion draws more frames than the output asks for: 31 a second for 20 changes, 28 for about
  15. Ghostty draws one frame per change (19.9, 13.2).
- An animation-only frame costs Termurion about 2.7 ms of CPU ((8.8 - 2.1)% for 25 more frames a
  second) and Ghostty about 0.6 ms ((4.2 - 0.5)% for 58 more).
- In a flood the screen freezes: 0.1 frames a second for the whole 20 s, while Ghostty takes 65%
  more bytes and still draws 60 frames a second.

## Where the time goes

`sample` of Termurion's main thread over the 20 s runs (`target/draw-bench/sample1`; the sampler
took about 1.7 ms per sample in the VM; counts are inclusive). The other threads (pty socket
reader, Metal's queues, Core Animation) together stay under 2% of a core outside the flood, and
`termurion-host` used 0.3% during stream20.

| main thread | stream20 | claude | Lightning stream20 | Lightning claude | Lightning idle (12 s) |
|---|---|---|---|---|---|
| busy (not waiting) | 2102 | 1632 | 2490 | 1694 | 452 |
| `render_frame` | 1664 | 1221 | 1982 | 1245 | 261 |
| `build_frame` | 305 | 366 | 351 | 348 | 17 |
| of it `pane_with` | 189 | 220 | 223 | 212 | 5 |
| `draw_with` (incl. glyphon prepare, inlined) | 1206 | 717 | 1323 | 638 | 12 |
| of it glyphon glyph cache (`lru` get, in-use insert) | 289 | 111 | 330 | 128 | |
| of it `LayoutGlyph::physical` | 68 | 28 | 70 | 31 | |
| of it shaping | 127 | 34 | 163 | 28 | |
| buffer uploads (`write_buffer`, `create_buffer_init`) | 144 | 125 | 172 | 158 | 24 |
| `Chain::run` (Lightning's passes) | | | 115 | 103 | 154 |
| queue submit | 137 | 174 | 159 | 148 | 99 |
| present + `nextDrawable` | 172 | 151 | 161 | 134 | 61 |
| row cache hashing and compare | 98 | 120 | 109 | 103 | 6 |
| settings lookups (`globals::default_of`) | 94 | 96 | 120 | 116 | 28 |
| `about_to_wait` (the event loop's idle work) | 159 | 169 | 213 | 210 | 82 |
| malloc and free | 217 | 109 | 259 | 135 | 26 |

Flood: the main thread spent 11899 of 14386 samples in alacritty's parser (`advance`), so no
frame could be drawn; a second thread spent its time reading the socket from `termurion-host`.

Why there are more frames than changes: dtrace on `Window::request_redraw` during stream20
showed most requests coming from `paced_draw` inside `about_to_wait`, which is
`tick_activity` (`src/app/activity_ui.rs`): while a session is "processing" (output in the last
2 s) the activity indicator's spinner asks for a paced frame on every turn of the event loop,
whether or not the indicator is on screen. With one tab there is no tab bar, so nothing changes,
but each such frame is a full one (`Gpu::redraw` marks the scene changed). One-variable check:
with `HideActivityIndicator = true`, stream20 drew 19.9 frames a second (from 30.9) at 17.9% CPU
(from 19.9%) and 9.0% WindowServer, and claude drew 10.1 (from 28.2) at 13.1% CPU (from 15.2%)
and 5.8% WindowServer (from 10.6%).

The Lightning shader on his Mac: the window is 3236x1762 pt, 6472x3524 = 22.8 million pixels.
At 30 animation frames a second the full-screen pass shades 684 million pixels a second, 14.6
times what the VM's window shades, plus the copy of the frame texture. The shader's visible
output while idle is a few cells around the cursor, the prompt mark and the input box lines.

## Changes, ranked by measured win

Ranked for the owner's case (Claude streaming with Lightning in a big Retina window). "Main
thread" shares are of the busy samples above; on his window there are about 3.8 times as many
cells (449x103 against 220x56), so every per-glyph cost weighs more there than in the VM.

### 1. Keep prepared rows on the GPU (a row instance cache in glyphon)

Today every frame that is not animation-only walks every glyph of every visible row through
glyphon's `prepare`: `physical()` rounding, an `lru` lookup and an in-use `HashSet` insert per
glyph, a vertex pushed per glyph, and the whole vertex array written to the GPU. 1.1.7's row cache
saved the shaping; it did not save this. Measured: glyph cache, `physical()`, vertex building
(`draw_with`'s own samples) and the uploads are 640 of 2102 busy samples in stream20 (30%), about
25% in claude, and on his Mac `write_buffer` 157 + cache 203 + `physical` 54 of 4048.

Change: glyphon (vendored in `crates/glyphon`) gets a per-row prepared form: for each shaped row
buffer it keeps the row's `GlyphToRender` instances in row-relative coordinates and the atlas
entries they use. A row whose buffer was reused (1.1.7 already finds it by content) reuses its
instances; only new rows run `prepare_glyph`. Instances live in one GPU buffer in row slots; a
frame writes only the changed slots and a small per-row table of (slot, x, y, clip) that the
vertex shader adds, so a scroll rewrites 56 table entries, not 10,000 glyphs. The atlas keeps a
use count per glyph from the rows that hold it, instead of clearing `glyphs_in_use` every frame
(`TextAtlas::trim`), so a glyph a cached row needs is never evicted; growth re-rasterizes as
today and marks every row's instances stale. Quads (backgrounds, underlines, cursor) get the same
treatment through the row cache's `RowOut`, and the five `create_buffer_init` calls per frame
become persistent buffers written in place.

This is the cell-grid renderer Ghostty, Alacritty and WezTerm use, built on the rasterizer
Termurion already has, so the pixels stay the same: same swash masks, same mask filters, same
placement. A full new renderer (own atlas, own shaping per cell) would gain little more, because
shaping is already cached per row (127 samples of 2102), and it would put the exact iTerm2 look
at risk.

Expected win: 25-30% of the main thread in the VM, more on his window; draw cost then scales
with changed rows, not with the window. Size: large (glyphon patch about 400 lines, `draw_with`
rework, tests). Risk: medium; atlas growth and eviction are where bugs would hide. Guard: a
pixel test that draws every pane-frame fixture and every recorded agent screen with both paths
and compares bytes, plus a test that grows the atlas mid-run.

### 2. Make an animation-only frame cheap, and shade only where the effect is

Measured: Lightning at an idle prompt costs 8.8% CPU against Ghostty's 4.2% with the same shader
at twice the frame rate: 2.7 ms against 0.6 ms a frame. Of the 452 busy samples, `Chain::run`
was 154 (its queue submit 99), `about_to_wait` 82, settings lookups 28; `build_frame` was only
17, so 1.1.6's kept scene works and the rest is overhead around the passes.

CPU: `Chain::run` creates a bind group per pass per frame and writes every uniform block each
time; keep the bind groups (they change only with the textures) and write uniforms only when
they changed. Skip the frame-independent work `render_frame` does before drawing on an
animation-only frame (`fx_sync`, caption publishing, menu text, `sync_blur`, title), which
belongs to built frames only.

GPU: on an animation-only frame the picture under the effect did not change, so the chain can
keep its last output in a texture it owns and run the shader only inside the effect's region,
with a scissor, then copy to the drawable. Lightning's region is known from its uniforms: the
cursor rect grown by its glow and sparks (2.3 cell heights), the prompt mark, the input box
lines, the bolt's segment from the previous cursor, the surge band while it plays. A plugin
declares its region in `plugin.toml` (new key, for example `region = ["cursor:2.5",
"prompt_mark", "input_box", "cursor_path"]`); a plugin without it keeps the full-screen pass, so
Ghostty shaders still work. Idle, that is a few thousand pixels instead of 22.8 million on his
window. A built frame still runs the full pass.

Expected win: CPU of an idle Lightning prompt from 8.8% to the 3-4% range (at or under Ghostty),
the shader's GPU work while idle or typing down by over 95% on Retina. Size: medium. Risk: low
for the CPU part; medium for the region (a wrong region clips the effect: test each effect
state against a full-screen render of the same frame).

### 3. Draw a frame only when something on screen changed

Measured above: the activity spinner's ticks ask for full frames while output flows, though no
tab bar shows it. Fixing it took claude from 28.2 to 10.1 frames a second, CPU 15.2% to 13.1%,
WindowServer 10.6% to 5.8%; stream20 from 30.9 to 19.9 frames, 19.9% to 17.9% CPU.

Change: `tick_activity` asks for a frame only when an indicator is drawn somewhere (a visible
tab bar or a title that shows it), and then at the spinner's own step rate, as iTerm2 redraws its
tab bar. The same audit for every `redraw()` caller: each one must name a change that is on
screen. A built frame that would equal the kept one (same scene) skips the GPU entirely; cheap
to check once the row cache keys every row.

Frame pacing proper is already iTerm2's (at most 60 a second for light output, 30 over 10 KB/s,
never above the display); once frames follow changes, claude draws 10 to 15 a second like
Ghostty and iTerm2. Lowering the rate further while streaming (say 20 a second after a second
of steady output) saves little after this fix (stream20 already sits at 20) and makes text
visibly steppier; not proposed.

Expected win: 10-15% CPU and about 45% of WindowServer's work in the Claude case (measured).
Size: small. Risk: low (a missed redraw shows as a stale indicator; covered by the existing
activity tests plus one for "no tab bar, no frame").

### 4. Cut the per-wake overhead

Measured: settings lookups (`globals::default_of`, reached from `about_to_wait`,
`cadence::Prefs::from_globals`, `division_height`, `window_layout`, `content_area_for`) are 94 to
120 busy samples (5-7%); `about_to_wait` as a whole 159 to 213 (8-12%); the row cache hashes and
compares every row's cells every built frame (98-120, 5-7%); malloc and free 109-259 (7-10%,
much of it `RowOut::place` cloning every `TextRow` with its span strings each frame).

Changes: resolve the settings the frame loop reads into a struct when settings change (the
defaults table is searched on every read today); let the row cache skip rows alacritty did not
damage (alacritty's damage is read and reset by Instant Replay alone today,
`Screen::replay_capture`; a per-pane dirty bitmap filled at each read gives the renderer its own
view), keeping the full compare for damaged rows and the 0.5 s full rebuild; share row text
between the cache and the frame (`Arc` spans) instead of cloning it.

Expected win: 15-20% of the main thread together. Size: small to medium, in independent
pieces. Risk: low (damage misses would show as stale rows; the 0.5 s rebuild bounds them and the
`both_ways` tests cover the cache).

### 5. Parse output off the main thread

Measured: in a flood the main thread is 100% in alacritty's parser and draws nothing for 20 s;
Ghostty parses on a per-surface IO thread, takes 115 MB/s and draws at 60 frames a second.

Change, in two steps. First, small: bound the parse work per event-loop turn (a byte or time
budget, about 4 ms) and let a due frame draw between slices, so the screen stays live at the
pacer's 30 frames a second. Second, large: move `Term::advance` for each pane to its own thread
behind a lock, as Alacritty does, with the main thread taking the lock only to read damaged rows
for a frame. The second step touches everything that reads or writes the terminal on the main
thread today (triggers, shell integration, tmux, Instant Replay, selection, the AI features),
so it needs its own design.

Expected win: the flood screen live (step 1); throughput toward Ghostty's and a main thread
free for frames in heavy streams (step 2). No change in the Claude case, where parsing is not
in the profile. Size: small, then large. Risk: low, then high.

### 6. Present less: damage regions

With 1 and 3 done, a built frame's CPU cost follows the changed rows, but the GPU still redraws
and WindowServer still composites the whole window. Keeping the last terminal picture in a
texture (the chain already does this for effects) and redrawing only the damaged row bands with a
scissor, then copying, cuts the GPU's fragment work for a spinner frame to a few rows. On macOS a
CAMetalLayer always presents the whole drawable, so WindowServer's share does not shrink; it
would only with a separate small layer for the animated region, which is a later idea, not
proposed now. Expected win: GPU only, not measurable in the VM. Size: medium. Risk: medium (an
underestimated damage region leaves stale pixels).

## What must not change

- The look. Glyph rasterization stays swash through cosmic-text with Termurion's mask filters
  (`set_mask_filter`), the same atlas content and the same pixel placement (`physical()`
  rounding, `left_align`, `cluster_fixes`). The golden pane frames (`tests/fixtures/pane-frames`)
  check the scene, not pixels; they must still pass unchanged, because every change above sits
  below the scene or leaves its output equal. Changes 1, 2 and 6 add pixel tests that compare the
  new path with today's `draw_with` and full-screen chain byte for byte, on every fixture and on
  the recorded agent screens (`tests/fixtures/agents`).
- Every feature that draws: ligatures, Special Font Config faces, box and block glyphs, images,
  backgrounds, badges, porthole prose, popups, the fade, underline gaps, double strike,
  transparency, the Linux menu bar strip, all render plugins and FX profiles.
- The OpenGL path (no view formats) on Linux and the W541, and Windows.

## Order of work

3 first (small, measured, removes the noise frames from every later measurement), then 4 (small
pieces), then 2, then 1 (the big one, on a quieter baseline), then 5 step 1. 5 step 2 and 6 wait
for new measurements after the rest. After each step: `scripts/draw-bench` on funtimes, the same
table, and one measurement on his Mac's real window (he runs `sample`; no GUI is opened there by
an agent).

Rough expectation for claude with Lightning in the VM: 17.4% now; 3 brings it near 15%, 4 near
12.5%, 1 near 9%, 2 removes most of the 30 animation frames' overhead between changes, ending
around Ghostty's 6%. Beating Ghostty clearly needs all of 1 to 4; none of them alone does it.

## Open questions for the owner

1. Change 2 adds a `region` key to `plugin.toml`. Shipped plugins would get it; Ghostty shaders
   brought in by users keep the full pass. Fine?
2. Change 3 stops frames for an activity indicator that is not on screen. Any place you rely on
   the spinner turning where Termurion has no tab bar?
3. Change 5 step 2 (parsing on a thread) is the largest and riskiest; do it only if the flood
   matters to you beyond a live screen.
