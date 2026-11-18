# Draw Speed Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** For the same work, Termurion uses less CPU and GPU than iTerm2 and Ghostty, above all while Claude Code streams with the Lightning effect on, with the picture unchanged pixel for pixel.

**Architecture:** First stop frames nobody can see (the activity spinner, frames equal to the kept one), fix the update prompt's redraw loop, then cut per-wake overhead (settings index, alacritty damage for the row cache, shared row text). Then make animation-only frames cheap (kept bind groups, no frame work, the effect shader only inside a declared region). Then keep prepared rows on the GPU: glyphon gets a row renderer whose per-row instances live in slots of one GPU buffer, moved by a small per-row uniform table, with atlas use counts so a cached row's glyphs are never evicted; quads go to persistent buffers. Last, parse pty output in bounded slices so a flood keeps the screen live.

**Tech Stack:** Rust 2024, winit (vendored `crates/winit`), wgpu, glyphon 0.12 (vendored `crates/glyphon`), cosmic-text 0.19, alacritty_terminal (vendored). Tests: `scripts/check-unix --test <filter>` (Linux here plus macOS on funtimes), `scripts/xt test --lib -- <filter>` (Windows on EXTREME), `scripts/win-check` (Windows type check from Linux), `cargo test -p glyphon --lib` for the vendored glyphon.

**Spec:** `docs/superpowers/specs/2026-11-18-draw-speed.md` (binding). Measurements: `.superpowers/sdd/2026-11-18-draw-speed/phase1-report.md`. Benchmark: `scripts/draw-bench`.

**Owner rulings (decided, not open):**
1. Change 2's `region` key in `plugin.toml`: yes. Every shipped plugin gets it; a plugin without it keeps the full-screen pass.
2. Change 3: an activity indicator that is not on screen draws no frames. Approved.
3. Change 5: step 1 only (bounded parse slices). Step 2 (parser thread) and change 6 (damage regions) are out of scope.
4. In scope as its own task: the "Check for updates automatically?" prompt redraws nonstop (110% CPU) and sits behind the terminal window. Fix both.
5. Out of scope: Termurion's cell is wider than iTerm2's for Monaco 12 (1595 pt against 1567 pt for 220 columns). The final task reports it again; nothing changes for it.

## Global Constraints

- The exact iTerm2 look must not change: pixels identical. Glyphs stay swash through cosmic-text with Termurion's mask filters (`set_mask_filter`), the same atlas content and the same placement (`physical()` rounding, `left_align`, `cluster_fixes`).
- The golden fixture test `scene::agent_box_tests::pane_frames_match_the_recorded_baseline` passes with every file in `tests/fixtures/pane-frames` unchanged. Never run it with `TERMURION_BLESS`. Any change to a `Frame` type must keep its `Debug` output byte-identical.
- Pixel-equality tests (spec "What must not change") are required for changes 1 and 2: the new path against today's `draw_with` and today's full-screen chain, byte for byte, on every pane-frame fixture case and every recorded agent screen (`tests/fixtures/agents`).
- GPU tests run headless here: sirtoolio has no GPU but Mesa lavapipe (`/usr/share/vulkan/icd.d/lvp_icd.json`), and the existing render GPU tests (`render::tests::*`, `test_gpu::shared`) pass on it today. Pixel tests use `test_gpu::shared` (never a new `wgpu::Instance`; `gpu_tests_share_one_instance` enforces it). The same tests run on funtimes (Metal) through `scripts/check-unix --test` and on EXTREME (DX12) through `scripts/xt`.
- The OpenGL path keeps working: no view formats on Linux and Windows (`Chain::without_view_formats`, `Renderer::set_view_formats(false)`), and the real GL backend (W541). Every pixel test runs both with and without view formats. No new shader feature that GLES 3.0 lacks: no storage buffers in vertex shaders, no `first_instance` (draw a vertex buffer slice instead), uniform buffers with dynamic offsets are fine.
- Every render plugin (the 13 in `resources/render_plugins`, user plugins, Ghostty shaders) and every FX profile (`resources/fx_profiles`) keeps working.
- Short human comments in code, only where the why is not obvious (house style). No em dashes and no emoji anywhere: code, comments, docs, commit messages.
- No host or user names in commits. No Claude attribution and no Co-Authored-By lines in commits. Work on main, do not push. Do not bump the version or edit `docs/FIXES.md` (the controller does both).
- Known failures that are not regressions: Linux and macOS `scene` has 2 (`offscreen_band_normalizes`, `normalization_form_reaches_the_drawn_text`); Windows full suite has 3 (`handoff_win::tests::handles_are_typed_before_use`, `platform::windows::pty::tests::a_close_never_happens_while_attached`, `util_query::tests::it2ul_follows_the_script`). Run the full Windows suite with `--test-threads=1` (parallel runs have stalled on EXTREME). Never run two `scripts/xt` at once.
- Benchmarks run on funtimes only (`scripts/draw-bench`); never open a GUI or take a screenshot on the owner's Mac. To bench a build: `scripts/mac-bundle`, then `ssh funtimes 'rm -rf /Applications/Termurion.app && ditto ~/src/termurion/target/release/Termurion.app /Applications/Termurion.app'`.

## Review Focus

1. **Rows that move by a fractional amount** (a pane at a fractional `y`, the `fractional-top` fixture, a porthole heading at `scale` 1.25): cosmic-text's `physical()` truncates `y` and bins `x`, so a cached row moved by a non-whole amount must be prepared again, never shifted. Expected: identical pixels. Test: Task 13 `fractional_tops_match_the_full_path`.
2. **Atlas pressure while rows are cached** (a CJK or emoji flood that fills a small atlas, evicts and grows it while other rows hold glyphs): no glyph a drawn row needs is evicted or drawn from a stale place. Test: Task 13 `atlas_growth_and_eviction_mid_run_match`.
3. **The cursor jumps far between animation-only frames** (a `clear`, Claude Code redrawing its box at another row): the old glow must be erased, not left behind outside the new region. Test: Task 9 `a_cursor_jump_erases_the_old_glow`.
4. **The window changes size or scale between animation frames** (a resize, a move to another display): the kept effect output is invalid and the next frame runs the full pass. Test: Task 9 `a_new_size_runs_the_full_pass`.
5. **The shell exits while a flood is still unparsed**: every byte shows before the pane's end action runs. Test: Task 15 `exit_waits_for_unparsed_output`.

---

## File Structure

- `src/app/activity_ui.rs`, `src/scene.rs` (`spinner_turn`): spinner frames only when a spinner is on screen (Task 1).
- `src/app/scene_cache.rs`, `src/app/mod.rs` (`render_frame` 7284-7470): equal frames skip the GPU (Task 2); animation-only frames skip the frame work (Task 7).
- `src/egui_host.rs` (`paint_with`, `paint_dialog`), the launch-time ask opener in `src/app/` (Task 3).
- `src/schema.rs` (`find_global`), `src/globals.rs` (generation), create `src/app/frame_settings.rs` (Task 4).
- `src/screen.rs` (`replay_capture` 1237, damage), `src/scene_rows.rs`, `src/scene.rs` (`pane_with`), `src/app/mod.rs` (`build_frame`) (Task 5).
- `src/scene.rs` (`TextRow.spans`, new `Spans`), `src/scene_rows.rs` (`RowOut::place`), `src/render.rs` (shaped buffers) (Task 6).
- `src/render_fx_gpu.rs` (`Chain`), `src/render_fx.rs` (`Manifest`, region), `src/render.rs` (`draw_fx_for`), `src/app/fx_ui.rs`, `resources/render_plugins/*/plugin.toml`, `docs/render-plugins.md` (Tasks 7-9).
- Create `src/render_pixel_tests.rs` (cfg(test)): the pixel-equality harness every later render task uses (Task 10).
- `crates/glyphon/src/text_atlas.rs`, create `crates/glyphon/src/row_render.rs` and `crates/glyphon/src/row_shader.wgsl`, `crates/glyphon/src/lib.rs`, create `crates/glyphon/src/test_gpu.rs` (cfg(test)), `crates/glyphon/TERMURION-PATCHES.md` (Tasks 11-12).
- `src/render.rs` (`draw_with` 1586-1855) (Tasks 13-14).
- `src/pane.rs` (`pump` 749), `src/app/mod.rs` (the pump loop near 8116, `about_to_wait`, the `Notify` wake near 2205) (Task 15).

---

### Task 1: The activity spinner draws only when it is on screen (change 3)

**Files:**
- Modify: `src/app/activity_ui.rs` (`tick_activity` 60-103), `src/scene.rs` (`spinner_turn` 3819), `src/app/mod.rs` (App field)
- Test: `src/app/activity_ui.rs` tests, `src/scene.rs` tests

**Interfaces:**
- Produces: `pub fn spinner_step(ms_since_epoch: u128) -> u64` in `src/scene.rs` (`ms / 500`); `spinner_turn` becomes `(spinner_step(ms) % 8) as f32 / 8.0`.
- Produces: `pub(super) fn spin_draw(shown: bool, last: Option<u64>, step: u64) -> bool` in `activity_ui.rs`: `shown && last != Some(step)`.
- Produces: `App::spinner_shown(&self, now: Instant) -> bool`: true when some window shows a spinner: its tab bar is visible (`tab_position::bar_visible` with that window's full-screen state and tab count, flashing included) and one of its tabs has `tab_processing(any_processing(tab panes), foreground, HideActivityIndicator) == true`. Live window and parked windows both.
- Produces: `App.spin_step: Option<u64>` (the step last drawn).

- [ ] **Step 1: Write the failing tests.**
  - `spinner_draws_only_when_shown_and_stepped`: `!spin_draw(false, None, 5)`, `spin_draw(true, None, 5)`, `!spin_draw(true, Some(5), 5)`, `spin_draw(true, Some(5), 6)`.
  - `spinner_step_matches_the_drawn_turn` (scene tests): for `ms` in `[0, 499, 500, 3999, 4000]`, `(spinner_step(ms) % 8) as f32 / 8.0 == ((ms / 500) % 8) as f32 / 8.0`.
  - `tick_activity_uses_the_spinner_rule` (source scan, as `output_sources_draw_through_the_pacer` does): the body of `fn tick_activity` contains `spin_draw(` and `spinner_shown(` and no longer assigns `spinning` from `self.activity.values().any(`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test activity_ui spinner_step`. Expected: FAIL (functions missing).
- [ ] **Step 3: Implement.** In `tick_activity`: `let step = spinner_step(now_ms); let spinning = spin_draw(self.spinner_shown(now), self.spin_step, step);` and set `self.spin_step = Some(step)` when it draws, `None` when no spinner is shown. Keep `output_draw` and the paced path as they are.
- [ ] **Step 4: Run** `scripts/check-unix --test activity_ui spinner scene_cache`. Expected: PASS on linux and macos. `scripts/win-check`: `windows: 0 errors`.
- [ ] **Step 5: Audit the remaining frames on funtimes.** Install the build (Global Constraints), then count `Window::request_redraw` callers during stream20 and claude with dtrace as phase 1 did (report "Why there are more frames than changes"). Every caller left must name a change that is on screen; fix any that does not in this task. Then `scripts/draw-bench termurion termurion-fx`. Expected: stream20 at most 21 frames/s (1.1.8: 30.9), claude at most 16 (1.1.8: 28.2), CPU of both at or under 1.1.8's (19.9, 15.2).
- [ ] **Step 6: Commit** `Activity: the spinner draws frames only when a tab bar shows it`.

### Task 2: A built frame equal to the kept one skips the GPU (change 3)

**Files:**
- Modify: `src/app/scene_cache.rs`, `src/app/mod.rs` (`render_frame`)

**Interfaces:**
- Produces: `pub(super) enum Take<F> { Reuse(F, Instant), Build { last: Option<F> } }` and `SceneCache::take(&mut self, size, now) -> Take<F>` (replaces the `Option` return). `Reuse` keeps today's rule. `Build.last` is the kept frame only when `changed` was set (an app request; a frame nobody asked for, a system redraw, gets `None`) and the size is equal; it is used up either way.
- Produces: `pub(super) fn skip_gpu(same: bool, fx_active: bool, bar_busy: bool, alpha_changed: bool, fade: f32) -> bool` = `same && !fx_active && !bar_busy && !alpha_changed && fade >= 1.0`.
- Produces: `fn worth_comparing(frame: &Frame) -> bool` = `frame.images.is_empty() && frame.backgrounds.is_empty()` (image pixels compare by value and would cost more than the draw).

- [ ] **Step 1: Write the failing tests** in `scene_cache.rs`: `an_app_request_hands_back_the_last_frame` (`keep(7)`, `changed()`, `take` gives `Build { last: Some(7) }`), `a_system_redraw_never_hands_it_back` (no request: `Build { last: None }`), `a_new_size_hands_back_nothing`; existing tests adapted to `Take`. In `mod.rs` tests: `skip_gpu_needs_every_condition` (true only for `(true, false, false, false, 1.0)`; each single flip gives false).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test scene_cache skip_gpu`. Expected: FAIL.
- [ ] **Step 3: Implement.** In `render_frame`, after `build_frame`: `same = worth_comparing(&frame) && last.as_ref() == Some(&frame)`; when `skip_gpu(..)`, keep the frame (`gpu.scene.keep(frame, now, size)`) and return before `get_current_texture`. `alpha_changed` is whether `sync_surface_alpha` reconfigured this frame (make it return a bool).
- [ ] **Step 4: Run** `scripts/check-unix --test scene_cache skip_gpu render`. Expected: PASS. `scripts/win-check`: 0 errors.
- [ ] **Step 5: Commit** `Frames: a frame equal to the last one is not drawn again`.

### Task 3: The update prompt stops redrawing nonstop and opens in front

**Files:**
- Modify: `src/egui_host.rs` (`paint_with` 388-495, `paint_dialog` 310-380), the opener of launch-time asks (`update_app::permission_ask` is queued from `src/app/update_app.rs`; find where its window is opened)
- Test: `src/egui_host.rs` tests, the opener's tests

**Interfaces:**
- Produces: `pub enum Retry { Reconfigure, Later }` and `pub fn surface_retry(status: &wgpu::CurrentSurfaceTexture) -> Retry` in `egui_host.rs`: `Outdated`, `Suboptimal`, `Lost` give `Reconfigure` (configure and redraw, as today); `Timeout`, `Occluded`, `Validation` give `Later` (no configure, no redraw now; the window redraws on `WindowEvent::Occluded(false)` or after `RETRY_LATER = 250 ms` through `with_delayed`).
- Produces: `pub(super) fn ask_may_show(first_terminal_frame: bool) -> bool` for asks raised at launch.

- [ ] **Step 1: Reproduce on funtimes** (superpowers:systematic-debugging). Settings that ask: `SUHasLaunchedBefore = true` and no `SUEnableAutomaticChecks` in Termurion's settings.toml there; save and restore the file the way `scripts/draw-bench` does ("terminal settings, saved and put back") and check it byte-equal after. Launch, wait 10 s: `top -l 2 -pid <pid>` shows the CPU; `sample <pid> 5` shows the loop. Record which of the two reconfigure-and-redraw sites in `paint_with` fires (the size check against `config`, or the `get_current_texture` failure arm), with a temporary `dlog!` line removed before commit. Also record whether the prompt window was created before the terminal window and whether it is occluded (`WindowEvent::Occluded`).
- [ ] **Step 2: Write the failing tests:** `surface_errors_retry_by_kind` (each variant maps as in Interfaces); `a_hidden_dialog_does_not_redraw_at_once` (source scan of `paint_with`: the failure arm calls `surface_retry` and requests a redraw only for `Retry::Reconfigure`); `launch_asks_wait_for_the_first_terminal_frame` (`!ask_may_show(false)`, `ask_may_show(true)`). If Step 1 found the size check looping instead (a fit asking for a size the window never takes), add `a_fit_never_asks_the_same_size_twice` against `dialog_frame::DialogFit::step` and fix that cause too.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test egui_host dialog_frame update_app`. Expected: the new tests FAIL.
- [ ] **Step 4: Implement** the retry rule, the cause Step 1 found, and the order: a launch ask opens (or, if already open, is shown and `focus_window()`ed) only after the first terminal window presented its first frame.
- [ ] **Step 5: Run** `scripts/check-unix --test egui_host dialog_frame update_app prompt_ui`. Expected: PASS. `scripts/win-check`: 0 errors.
- [ ] **Step 6: Verify on funtimes:** the same launch shows the prompt in front of the terminal window (look at the screen through `funtimes-ui`) and Termurion's CPU with the prompt open stays under 3% over 10 s. Settings file restored byte-equal.
- [ ] **Step 7: Commit** `Update prompt: no redraw loop while hidden, and it opens in front`.

### Task 4: Settings reads cost a hash lookup, frame settings resolve once (change 4, piece 1)

**Files:**
- Modify: `src/schema.rs` (`find_global` 145), `src/globals.rs`, `src/cadence.rs` (`Prefs::from_globals` callers), `src/app/mod.rs` (`about_to_wait`, `division_height` 3911, `window_layout` 3358), `src/app/statusbar_ui.rs` (`content_area_for` 336)
- Create: `src/app/frame_settings.rs` (add `mod frame_settings;` in `src/app/mod.rs`)

**Interfaces:**
- Produces: `find_global` backed by `static INDEX: OnceLock<rustc_hash::FxHashMap<&'static str, usize>>` over `global()`; same signature.
- Produces: `Globals::generation(&self) -> u64`, bumped by `set`, `unset`, `load`, and any other method that writes `values`.
- Produces: `pub(super) struct FrameSettings { generation: u64, pub cadence: crate::cadence::Prefs, .. }` with one field per setting the frame loop reads in `about_to_wait`, `division_height`, `window_layout`, `content_area_for` and `render_frame`; `fn refresh(&mut self, g: &Globals)` re-reads only when `g.generation()` moved. `App.frame_settings: RefCell<FrameSettings>` and `App::fs(&self) -> std::cell::Ref<'_, FrameSettings>` (refreshes first).

- [ ] **Step 1: Write the failing tests:** `find_global_finds_every_key_and_no_other` (for every `s` in `global()`, `std::ptr::eq(find_global(&s.key).unwrap(), s)`; `find_global("NoSuchKey").is_none()`); `generation_moves_on_every_write` (`set`, `unset` each raise it; `get` does not); `frame_settings_follow_a_change` (`MaximumFrameRate` set to 30 shows in `fs().cadence.maximum_frame_rate` on the next read, no other call between).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test find_global generation_moves frame_settings`. Expected: FAIL.
- [ ] **Step 3: Implement**, moving each `self.globals.*` read in the five functions to a `FrameSettings` field.
- [ ] **Step 4: Run** `scripts/check-unix --test globals schema frame_settings cadence settings_keys`. Expected: PASS. `scripts/win-check`: 0 errors.
- [ ] **Step 5: Commit** `Settings: indexed lookups, and the frame loop reads resolved settings`.

### Task 5: The row cache skips rows alacritty did not damage (change 4, piece 2)

**Files:**
- Modify: `src/screen.rs` (`replay_capture` 1237-1290, the drop path near 1291, `term_mut`), `src/scene_rows.rs`, `src/scene.rs` (`pane_with`), `src/app/mod.rs` (`build_frame`)
- Test: `src/scene.rs` `row_reuse_tests`, `src/screen.rs` tests

**Interfaces:**
- Produces: `pub enum RowDamage { Full, Rows(Vec<bool>) }` (viewport rows) and `Screen::take_render_damage(&mut self) -> RowDamage`. One private `fn drain_damage(&mut self)` reads `term.damage()` into two accumulators (`replay_dirty`, `render_dirty`, OR-ed until their reader takes them) and resets alacritty's. `replay_capture` reads its own accumulator; its behavior is unchanged.
- `RowDamage::Full` when alacritty says full, on the first take, after a resize, when `display_offset` changed since the last take, and after any `term_mut()` call (a direct grid write outside the parser: `term_mut` sets `render_full`).
- Produces: `RowCache::set_damage(&mut self, d: RowDamage)` and a per-viewport-row table of last frame's `(context, extra, entry)`. In `pane_with`, a row that is clean, has the same `context` and `extra` as last frame at the same viewport row, and is not a never-cached row (cursor rows, image rows, bidi rows: the 1.1.7 rules) reuses that entry without hashing or comparing cells. Every other row takes today's path. The 0.5 s `due` clear clears the table too.
- Produces: `RowCache::compares(&self) -> u64` (rows that hashed and compared cells, for tests).
- `build_frame` calls `pane.rows.set_damage(pane.screen.take_render_damage())` before `pane_with` (only when the pane's cache is passed).

- [ ] **Step 1: Write the failing tests:** `undamaged_rows_skip_the_compare` (24-row screen, a frame, then `\x1b[6;1Hx` and a frame: `compares` grew by at most 1 plus the never-cached rows, and the frame equals a full build); `term_mut_marks_everything_dirty`; `scrolling_the_view_is_full_damage`; `replay_still_sees_damage_after_the_renderer_took_it` (feed a change, `take_render_damage`, then `replay_capture` still reports that row); every existing `both_ways` test unchanged and passing.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test row_reuse_tests replay`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test scene screen scene_rows replay pane_frames_match_the_recorded_baseline`. Expected: PASS apart from the 2 known normalization failures; golden fixtures untouched (`git status tests/fixtures` clean). `scripts/win-check`: 0 errors.
- [ ] **Step 5: Commit** `Rows: rows alacritty did not touch skip the compare`.

### Task 6: Row text is shared, not cloned (change 4, piece 3)

**Files:**
- Modify: `src/scene.rs` (`TextRow` 56, new `Spans`), every `spans: vec![..]` construction site (about 190, mostly `.into()`), `src/scene_rows.rs` (`RowOut::place`), `src/render.rs` (the shaped-buffer list in `draw_with`)

**Interfaces:**
- Produces: `pub struct Spans(std::sync::Arc<Vec<Span>>)` with `Clone`, `Default`, `PartialEq` (`Arc::ptr_eq` first, then content), `Deref<Target = Vec<Span>>`, `DerefMut` (through `Arc::make_mut`), `From<Vec<Span>>`, `FromIterator<Span>`, and a hand-written `Debug` that prints exactly what `Vec<Span>`'s `Debug` prints (the golden fixtures are `{:#?}` of `Frame`). `TextRow.spans: Spans`.
- The renderer keeps a row's `Spans` (an `Arc` clone) as its shape key instead of cloning the spans.

- [ ] **Step 1: Write the failing tests:** `spans_print_like_a_vec` (`format!("{:#?}", Spans::from(v.clone())) == format!("{:#?}", v)` for a two-span row, also `{:?}`); `placing_a_row_shares_its_text` (after `RowOut::place`, the frame row's spans and the cached row's spans are `Arc::ptr_eq`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test spans_print_like_a_vec placing_a_row`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test scene scene_rows render pane_frames_match_the_recorded_baseline`. Expected: PASS (2 known), fixtures untouched. `scripts/win-check`: 0 errors.
- [ ] **Step 5: Bench** `scripts/draw-bench --sample termurion termurion-fx` (Tasks 4-6 together). Expected: settings lookups under 20 samples, row hashing and compare under 50, malloc and free under 120 in the stream20 profile (phase 1: 94, 98, 217); CPU of stream20 and claude below Task 1's numbers. Record the table in the commit message body.
- [ ] **Step 6: Commit** `Rows: text is shared between the cache and the frame`.

### Task 7: Animation-only frames do no frame work and keep their bind groups (change 2, CPU)

**Files:**
- Modify: `src/render_fx_gpu.rs` (`Chain`, `frame_views` 276, `run` 324), `src/app/mod.rs` (`render_frame`), `src/app/scene_cache.rs`
- Test: `src/render_fx_gpu.rs` or `src/render.rs` tests (GPU, `test_gpu::shared`)

**Interfaces:**
- Consumes: Task 2 `Take<F>`.
- Produces: `Chain` keeps `binds: Vec<wgpu::BindGroup>` (one per pass) and the copy passes' bind groups, made when the targets or plain textures are made (`new`, `without_view_formats`, `frame_views` on a size change), never in `run`. `last_packed: Vec<Vec<u8>>`: a pass's uniforms are written only when the packed bytes differ. `pub fn run(&mut self, device, queue, uniforms, out)`.
- Produces: counters `Chain::bind_groups_made(&self) -> u64`, `Chain::uniform_writes(&self) -> u64`.
- Produces: `SceneCache::would_reuse(&self, size, now) -> bool` (the `Reuse` rule without using anything up). On a frame that will reuse, `render_frame` skips `fx_sync`, `publish_caption`, `caption_text` with the window menu text refresh, `sync_blur` and the title send.

- [ ] **Step 1: Write the failing tests:** `chain_keeps_its_bind_groups_between_frames` (three `draw_offscreen_fx` at one size: `bind_groups_made` unchanged after the first; a new size raises it); `unchanged_uniforms_are_not_written` (same `Uniforms` twice: `uniform_writes` grows by 0 the second time; `time` changed: by the number of passes with a layout); `kept_bind_groups_draw_the_same_pixels` (a 10-frame Lightning sequence through one chain equals the same frames each through a fresh chain, byte for byte, for `Chain::new` and `without_view_formats`); `an_animation_frame_skips_the_frame_work` (source scan: in `render_frame`, `fx_sync(`, `publish_caption(`, `caption_text(`, `sync_blur(`, `title_sent.send(` sit behind the `would_reuse` check).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test chain_keeps unchanged_uniforms kept_bind_groups an_animation_frame_skips`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test render render_fx scene_cache` and `scripts/xt test --lib -- render_fx render::tests`. Expected: PASS.
- [ ] **Step 5: Commit** `Effects: animation frames keep their bind groups and skip the frame work`.

### Task 8: The `region` key and its rectangles (change 2, region)

**Files:**
- Modify: `src/render_fx.rs` (`Manifest` 107, `parse_manifest` 118), `docs/render-plugins.md`, `resources/render_plugins/README.md` if it lists manifest keys

**Interfaces:**
- Produces: `Manifest.region: Option<Vec<RegionTerm>>` and `#[derive(Clone, Debug, PartialEq)] pub enum RegionTerm { Full, Cursor(f32), CursorPath(f32), PromptMark(f32), InputBox(f32), FullAfterChange(std::time::Duration) }`. Syntax in `plugin.toml`: `region = ["cursor:2.5", "cursor_path:2.5", "prompt_mark:1", "input_box:1", "full_after_change:600"]` or `["full"]`. The number is a pad in cell heights (none means 0), except `full_after_change`, which takes milliseconds and requires one. Errors: `plugin.toml: unknown region term "{t}"`, `plugin.toml: region term "{t}" needs a number`.
- Produces: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct PxRect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }` and `pub fn region_rects(terms: &[RegionTerm], u: &Uniforms, cell_h: f32, size: (u32, u32)) -> Option<Vec<PxRect>>`. `None` means the full screen. Window pixels, top-left origin; where a `Uniforms` field is in Ghostty's bottom-left convention, convert back with the mapping `Clock::frame` applied. Each term: `Cursor(p)` the current cursor rect grown by `p * cell_h`; `CursorPath(p)` the box around the previous and the current cursor (`current_cursor`, `previous_cursor`) grown by `p * cell_h`; `PromptMark(p)` the mark cell (`prompt_mark` x, y, cell width, `cell_h` tall) grown, nothing when not set; `InputBox(p)` for z = 1 the top and bottom lines (one cell tall each) across the `input_box_x` span or the full width when zeros, for z = 2 the band, grown, nothing when z = 0; `FullAfterChange(d)` the full screen while `u.time - u.time_cursor_change < d`; `Full` the full screen. Rects are rounded outward to whole pixels, clamped to `size`, overlapping ones merged; more than 8 become their bounding box. The caller unites this frame's rects with last frame's (Task 9).
- Produces: `pub fn chain_region(manifests: &[&Manifest]) -> Option<Vec<RegionTerm>>`: `None` if any plugin has no `region` or has `Full`; else all terms.
- `cell_h` is the renderer's default-face cell height (`Renderer.cell.1`); `FxPane` is not changed (it is part of `Frame`, whose `Debug` the golden fixtures pin).

- [ ] **Step 1: Write the failing tests** in `render_fx.rs`: `region_terms_parse` (the example above parses to the five terms with `Duration::from_millis(600)`); `a_bad_region_term_is_an_error` (`"glow"` and `"full_after_change"` give the two messages); `no_region_key_means_none`; `cursor_region_covers_the_cursor` (a `Cursor` at (100, 40, 10, 20) fed through `Clock::frame`, `cell_h` 20, `Cursor(1.0)`: one rect containing (80, 20, 50, 60)); `cursor_path_spans_both_cursors`; `input_box_lines_and_band`; `full_after_change_is_full_only_while_it_plays`; `a_chain_with_one_regionless_plugin_is_full`; `rects_are_clamped_and_merged` (a rect past the right edge is clamped; 9 disjoint rects give 1).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test render_fx`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement**, and document the key in `docs/render-plugins.md` (each term, the pad unit, and that a plugin without the key keeps the full pass).
- [ ] **Step 4: Run** `scripts/check-unix --test render_fx`. Expected: PASS.
- [ ] **Step 5: Commit** `Render plugins: a region key says where an effect draws`.

### Task 9: Animation-only frames shade only the region (change 2, region)

**Files:**
- Modify: `src/render_fx_gpu.rs`, `src/render.rs` (`draw_fx_for` 1350), `src/app/fx_ui.rs` (`fx_sync` keeps the chain's terms, `prepare` keeps last frame's uniforms), `src/app/mod.rs` (`render_frame` passes the region), `resources/render_plugins/*/plugin.toml` (all 13)
- Test: `src/render.rs` tests (GPU)

**Interfaces:**
- Consumes: Task 7 `Chain` (kept bind groups), Task 8 `region_rects`, `chain_region`, `PxRect`.
- Produces: `Chain::set_region_terms(&mut self, terms: Option<Vec<RegionTerm>>)`. With terms, the chain owns `kept_out` (its last output, raw format, window size) and the last pass writes `kept_out`, then a full-screen copy pass (`textureLoad`, no filtering, no color conversion) writes `out`. `pub fn run_in(&mut self, device, queue, uniforms, out, rects: Option<&[PxRect]>)`: `None` runs every pass full screen (cleared as today); `Some` runs each pass with `LoadOp::Load` and one draw per rect with `set_scissor_rect`, then the copy. `run` stays `run_in(.., None)`.
- The region path is used only when all hold: the frame is animation-only (`same_scene && frame_kept()`), the chain has terms, it has at most 2 passes, `kept_out` holds the last frame at this size, and the previous frame was drawn by this chain. Anything else runs the full pass.
- The scissor rects are this frame's `region_rects` united with last frame's (last frame's pixels outside the new region must go back to the plain picture).
- Produces: `Renderer::draw_fx_for(&mut self, frame, chain, uniforms, out, w, h, same_scene: bool, region: Option<&[PxRect]>)`.
- Every shipped `plugin.toml` gets a `region` line chosen by reading its shader (what the cursor, the bolt path, the prompt mark, the input box and the Enter surge can light, with their largest radius); a plugin whose test below cannot pass with bounded terms gets `region = ["full"]`.

- [ ] **Step 1: Write the failing tests** (lavapipe here; both `Chain::new` and `without_view_formats`; `keep_alpha` false and true):
  - `region_frames_match_full_frames_for_every_shipped_plugin`: for each `BUILTIN` plugin whose region is not full, on each agent screen (`agent_frame`), shots every 50 ms from 0 to 2000: frame 0 full, every later frame animation-only through `run_in(Some(rects))`; the cursor moves one cell at 300 ms, jumps 20 rows at 700 ms, Enter at 1000 ms (`Signals::enter`), typing (a cursor move per 50 ms) from 1200 to 1400 ms. Each shot byte-equal to a full pass with the same uniforms.
  - `a_cursor_jump_erases_the_old_glow`: the 700 ms shot alone, asserting the pixels around the old cursor equal the plain frame's where the full pass has them equal.
  - `a_new_size_runs_the_full_pass`: after a size change the next animation-only frame ignores the rects (equal to the full pass and `kept_out` remade).
  - `an_opaque_window_keeps_alpha_one`: `keep_alpha` false, every alpha byte 255 in region frames.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test region_frames a_cursor_jump a_new_size_runs an_opaque_window`. Expected: FAIL.
- [ ] **Step 3: Implement**, then set each plugin's `region` until the first test passes for it.
- [ ] **Step 4: Run** `scripts/check-unix --test render render_fx` and `scripts/xt test --lib -- render_fx render::tests`. Expected: PASS. Every FX profile still loads: `scripts/check-unix --test fx_profile fx_store`. Expected: PASS.
- [ ] **Step 5: Bench** `scripts/draw-bench termurion-fx`. Expected: idle CPU at most 5% (1.1.8: 8.8%), claude below Task 6's number. The GPU win cannot be measured in the VM (spec); say so in the commit body with the table.
- [ ] **Step 6: Commit** `Effects: animation frames shade only where the effect draws`.

### Task 10: Pixel-equality harness (change 1, first)

**Files:**
- Create: `src/render_pixel_tests.rs` (`#[cfg(test)] mod render_pixel_tests;` next to `mod render;`)
- Modify: `src/scene.rs` (`agent_box_tests`: move the case building of `pane_frames_match_the_recorded_baseline` into `pub(crate) fn baseline_cases() -> Vec<(String, Frame, bool)>`; the golden test formats the same cases exactly as before), `src/render.rs` (`agent_frame` becomes `pub(crate)` inside the tests module or moves to the harness; `test_gpu::shared_all` reads `TERMURION_TEST_BACKEND`)

**Interfaces:**
- Produces: `pub(crate) struct PixelCase { pub name: String, pub frame: Frame, pub size: (u32, u32) }`; `pub(crate) fn pixel_cases(r: &Renderer) -> Vec<PixelCase>`: every `baseline_cases()` frame at 808 x 486, every agent screen in `tests/fixtures/agents` (`.ansi` and `.raw`) through `agent_frame`, and a 300-frame `cjk_flood` sequence (rows of distinct CJK and emoji text scrolling, one new row per frame).
- Produces: `pub(crate) fn pixels(r: &mut Renderer, c: &PixelCase) -> Vec<u8>` (`draw_offscreen`) and `pub(crate) fn assert_same(name: &str, a: &[u8], b: &[u8], w: u32)` (on a mismatch reports the first differing pixel's x, y and both values, and the count).
- Produces: `TERMURION_TEST_BACKEND` = `gl`, `vulkan`, `metal` or `dx12` picks the shared instance's backends; unset keeps today's default.

- [ ] **Step 1: Write the tests:** `every_case_draws_the_same_after_other_frames` (each case on a fresh renderer equals the same case drawn by one renderer that drew every case before it, all cases twice); `the_scroll_matches_fresh_draws` (the `cjk_flood` sequence, every 10th frame against a fresh renderer); both with `set_view_formats(true)` and `(false)`. The golden test still passes.
- [ ] **Step 2: Run** `scripts/check-unix --test render_pixel_tests pane_frames_match_the_recorded_baseline`. Expected: PASS (this harness pins today's path; later tasks add the new path to it). Fixtures untouched.
- [ ] **Step 3: Check the OpenGL backend here:** `TERMURION_TEST_BACKEND=gl cargo test --lib -- render_pixel_tests`. If Mesa's EGL gives an adapter, it passes and every later pixel test step also runs it this way. If no GL adapter comes up (the shared device panics at `request_adapter`), record that in the commit body; then the GL runs in Tasks 13, 14 and 16 run on the W541 instead (`cargo test --lib -- render_pixel_tests` in its checkout with `TERMURION_TEST_BACKEND=gl`).
- [ ] **Step 4: Run** `scripts/check-unix --test render gpu_tests_share_one_instance`. Expected: PASS.
- [ ] **Step 5: Commit** `Tests: a pixel harness over every fixture and agent screen`.

### Task 11: Atlas use counts (change 1)

**Files:**
- Modify: `crates/glyphon/src/text_atlas.rs` (`InnerAtlas::try_allocate` 72-105, `grow`, `TextAtlas`), `crates/glyphon/src/lib.rs`, `crates/glyphon/TERMURION-PATCHES.md` (patches 4 and 5)
- Create: `crates/glyphon/src/test_gpu.rs` (`#[cfg(test)]`: one process-wide instance, device and queue in a `OnceLock`, never dropped)

**Interfaces:**
- Produces (patch 4): `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct GlyphKey(pub(crate) GlyphonCacheKey)`; `TextAtlas::hold(&mut self, key: GlyphKey)`, `TextAtlas::release(&mut self, key: GlyphKey)` (a use count per key; release at 0 does nothing); `TextAtlas::held(&self) -> usize` (keys with a count above 0); `TextAtlas::generation(&self) -> u64` (raised by every successful `grow`).
- Eviction (`try_allocate`): walk the LRU from the oldest end and evict the first entry that has an `atlas_id`, is not in `glyphs_in_use` and is not held; when none qualifies, return `None` (the caller grows). A held or in-use entry at the oldest end no longer ends the search.
- Produces (patch 5): `#[doc(hidden)] pub fn set_max_size(&mut self, px: u32)` caps both inner atlases' `max_texture_dimension_2d` (tests only; Termurion's tests use it in Task 13).

- [ ] **Step 1: Write the failing tests** in `text_atlas.rs` (custom glyphs through `TextRenderer::prepare_with_custom` with a solid rasterizer, atlas capped at 256 with `set_max_size(256)`): `held_glyphs_are_never_evicted` (glyph A prepared and held, then enough other 32x32 glyphs to force eviction: A is still cached at the same atlas position); `released_glyphs_can_be_evicted`; `eviction_skips_held_entries_at_the_lru_end` (the oldest entry held: a new allocation succeeds by evicting the next oldest, no `AtlasFull`); `growth_bumps_the_generation`.
- [ ] **Step 2: Run** `cargo test -p glyphon --lib`. Expected: FAIL.
- [ ] **Step 3: Implement**; record patches 4 and 5 in `TERMURION-PATCHES.md` with `// Termurion patch 4:` and `5:` comments at each site.
- [ ] **Step 4: Run** `cargo test -p glyphon --lib` (Linux), `ssh funtimes 'cd src/termurion && ~/.cargo/bin/cargo test -p glyphon --lib'` after `scripts/check-unix --mac-only` synced the tree, `scripts/xt test -p glyphon --lib`, and `scripts/check-unix --test render`. Expected: PASS everywhere.
- [ ] **Step 5: Commit** `glyphon: use counts keep held glyphs in the atlas`.

### Task 12: glyphon row renderer: per-row instances in slots (change 1)

**Files:**
- Create: `crates/glyphon/src/row_render.rs`, `crates/glyphon/src/row_shader.wgsl`
- Modify: `crates/glyphon/src/lib.rs` (export), `crates/glyphon/src/cache.rs` (the row pipeline), `crates/glyphon/src/text_render.rs` (share `prepare_glyph` and `GlyphToRender` building), `crates/glyphon/TERMURION-PATCHES.md` (patch 6)

**Interfaces:**
- Consumes: Task 11 `hold`, `release`, `generation`.
- Produces:
  - `pub struct RowArea<'a> { pub serial: u64, pub buffer: &'a Buffer, pub left: f32, pub top: f32, pub bounds: TextBounds, pub default_color: Color }`. `serial` names the buffer's content: the caller gives a new serial whenever it shapes a buffer again.
  - `pub struct RowRenderer`, `RowRenderer::new(atlas: &mut TextAtlas, device: &Device, multisample: MultisampleState) -> Self`, `fn prepare<'a>(&mut self, device: &Device, queue: &Queue, font_system: &mut FontSystem, atlas: &mut TextAtlas, viewport: &Viewport, rows: impl IntoIterator<Item = RowArea<'a>>, cache: &mut SwashCache) -> Result<(), PrepareError>`, `fn render(&self, atlas: &TextAtlas, viewport: &Viewport, pass: &mut RenderPass<'_>) -> Result<(), RenderError>`, `fn prepared_rows(&self) -> u64` (rows that ran `prepare_glyph`, a counter for tests), `fn clear(&mut self)`.
- Decisions the implementer cannot make alone:
  - **What a prepared row is.** For each row: its `GlyphToRender` list made exactly as `prepare_with_depth_and_custom` makes it today (every layout run, every glyph, through `physical((left, top), 1.0)` and `prepare_glyph`), but with no bounds clipping and no run culling, and the keys it holds. Clipping moves to the vertex shader.
  - **When a prepared row is reused.** Its entry key is `(serial, left.to_bits(), default_color)`, plus `top.to_bits()` unless the row is `y_exact`. A row is `y_exact` when every glyph's y term (`(glyph.y - glyph.font_size * glyph.y_offset)`, scale 1) is a whole number and both the prepared and the new `top` are at least 0; then `truncf(gy + top)` moves by exactly `trunc(top_new) - trunc(top_prepared)` and the row is drawn with that `dy`. Otherwise a moved row is prepared again. `left` never shifts (it is in the key).
  - **Lifetime.** Entries not drawn in a frame are dropped at the end of `prepare` and release their held keys. When `atlas.generation()` differs from the one the entries were made with, every entry is dropped first; when the atlas grows during a `prepare`, that `prepare` runs again for all rows (repeat until the generation is stable).
  - **Slots.** One instance buffer (`VERTEX | COPY_DST`); a row's instances take a slot of a power-of-two capacity (at least 64 instances), with a free list per capacity. The buffer doubles when full and every live row's instances are written again from their CPU copy. Only new rows' slots are written.
  - **Row table.** A uniform buffer with dynamic offsets, one entry per drawn row at stride `max(min_uniform_buffer_offset_alignment, 32)`: `struct RowUniform { offset: vec2<i32>, _pad: vec2<i32>, clip: vec4<i32> }` (`offset` = (0, dy); `clip` = the row's bounds clamped to the viewport exactly as `prepare_with_depth_and_custom` clamps them: left/top at least 0, right/bottom at most the resolution and at least left/top). The table is written only where an entry changed.
  - **Drawing.** Rows draw in the order given (the order `draw_with` passes them today), one draw each: `set_bind_group(2, rows, &[i * stride])`, `set_vertex_buffer(0, slot slice)`, `draw(0..4, 0..len)`. No `first_instance`, no storage buffers.
  - **Shader.** `row_shader.wgsl` is `shader.wgsl` with `vs_row`: add `offset` to `pos`, then apply the CPU clipping of `prepare_glyph` in `i32` (a glyph with `x > clip.max_x`, `x + w < clip.min_x`, `y > clip.max_y` or `y + h < clip.min_y` becomes a zero-size quad; the left and top cuts shift `uv` by the cut), then the same position and uv math, `@invariant` kept. `fs_main` is unchanged.

- [ ] **Step 1: Write the failing tests** in `row_render.rs` (`test_gpu` device, a `FontSystem` with the bundled test font Termurion's tests use or any system monospace, an offscreen `Rgba8UnormSrgb` target): `rows_draw_like_the_text_renderer` (10 rows with mixed bounds, some clipped on each side, drawn by `TextRenderer::prepare` and by `RowRenderer::prepare`: byte-equal); `a_moved_whole_row_is_not_prepared_again` (`y_exact` row moved by +20: `prepared_rows` unchanged, pixels equal to a fresh draw at the new place); `a_row_moved_by_half_a_pixel_is_prepared_again`; `rows_not_drawn_release_their_glyphs` (`atlas.held()` back to 0 after a frame with no rows); `growth_prepares_every_row_again`.
- [ ] **Step 2: Run** `cargo test -p glyphon --lib`. Expected: FAIL.
- [ ] **Step 3: Implement**; record patch 6 in `TERMURION-PATCHES.md`.
- [ ] **Step 4: Run** `cargo test -p glyphon --lib` on Linux, macOS (funtimes) and `scripts/xt test -p glyphon --lib`. Expected: PASS.
- [ ] **Step 5: Commit** `glyphon: a row renderer that keeps prepared rows on the GPU`.

### Task 13: draw_with draws rows through the row renderer (change 1)

**Files:**
- Modify: `src/render.rs` (`Renderer` fields, `draw_with` 1586-1745), `src/app/mod.rs` (the renderer follows `RedrawOnlyChangedRows`), `src/render_pixel_tests.rs`

**Interfaces:**
- Consumes: Task 12 `RowRenderer`, `RowArea`; Task 10 harness; Task 11 `set_max_size`.
- Produces: `Renderer::set_row_instances(&mut self, on: bool)` (on by default; with it off `draw_with` is exactly today's path through `self.text`), `Renderer::row_prepares(&self) -> u64`. The app calls `set_row_instances(globals.bool("RedrawOnlyChangedRows"))` wherever it applies that setting today, so the existing Advanced switch also turns this off (its help text already covers drawing bugs).
- Each shaped buffer in `self.buffers` carries a serial (a renderer counter, new each time a buffer is shaped or reshaped). The text rows (`all_rows`, double strikes included, same order) go to `self.rows.prepare`; box and block glyph areas stay on `self.text.prepare_with_custom` and are rendered after the rows, so the draw order is today's.

- [ ] **Step 1: Write the failing tests** in `render_pixel_tests.rs`: `row_instances_match_the_full_path_on_every_case` (each case drawn by a renderer with row instances on, after it drew every earlier case, equals a renderer with them off; view formats on and off); `scrolled_rows_reuse_their_instances` (20 rows, then the same rows one slot up with a new last row: `row_prepares` grows by at most 2); `fractional_tops_match_the_full_path` (rows at `y = n * 20 + 0.5` and a `scale` 1.25 row, moved each frame); `atlas_growth_and_eviction_mid_run_match` (`r.atlas.set_max_size(256)` on both renderers, the `cjk_flood` sequence, every frame equal); `the_switch_turns_the_row_path_off` (with `set_row_instances(false)`, `row_prepares` stays 0).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test render_pixel_tests`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test render render_pixel_tests scene pane_frames_match_the_recorded_baseline`, the GL run from Task 10 Step 3, and `scripts/xt test --lib -- render`. Expected: PASS (2 known in scene), fixtures untouched.
- [ ] **Step 5: Commit** `Renderer: unchanged rows keep their prepared glyphs on the GPU`.

### Task 14: Quads and images in persistent buffers (change 1)

**Files:**
- Modify: `src/render.rs` (`draw_with` 1747-1855: the five `create_buffer_init` calls and the fade cover)

**Interfaces:**
- Produces: `struct GpuVec { buf: wgpu::Buffer, cap: u64, last: Vec<u8>, written: u64 }` with `fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> Option<wgpu::BufferSlice<'_>>` (`None` for empty input): grows by doubling (min 4 KB) and then writes everything; else writes only the 4 KB chunks that differ from `last`. `Renderer` keeps one each for `under`, `over`, `pops`, `picture`, `backdrop` and `cover`. `fn bytes_written(&self) -> u64` sums them (tests).
- Decision: per-row quad slots (the spec's "same treatment through `RowOut`") are not built: `Frame` hands quads over as flat lists and one quad is 6 vertices, so after the glyphs the cost left is the upload, which the chunk diff makes follow the changed rows. A scroll still rewrites the quads; the per-row table stays a glyph feature.

- [ ] **Step 1: Write the failing tests:** `persistent_buffers_write_only_what_changed` (a frame, then the same frame with one quad's color changed: `bytes_written` grows by at most 4096 per changed list); `persistent_buffers_match_the_full_path` (harness: every case and the `cjk_flood` sequence equal to a fresh renderer); `a_bigger_list_grows_the_buffer` (a frame with 10x the quads draws right after a small one).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test persistent_buffers a_bigger_list`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test render render_pixel_tests pane_frames_match_the_recorded_baseline`, the GL run, `scripts/xt test --lib -- render`. Expected: PASS, fixtures untouched.
- [ ] **Step 5: Bench** `scripts/draw-bench --sample termurion termurion-fx`. Expected: glyph cache, `physical`, vertex building and uploads under 10% of stream20's busy samples (phase 1: 30%); claude with Lightning at or under Ghostty + Lightning's 6.0% CPU or within 1 point of it. Table in the commit body.
- [ ] **Step 6: Commit** `Renderer: quads and images live in persistent buffers`.

### Task 15: Output is parsed in bounded slices (change 5, step 1)

**Files:**
- Modify: `src/pane.rs` (`Io` 153-220, `pump` 749-806), `src/app/mod.rs` (the pump loop near 8116, `about_to_wait`, the `Notify` near 2205)
- Test: `src/pane.rs` tests, `src/app/mod.rs` tests

**Interfaces:**
- Produces: `pub const PARSE_SLICE: Duration = Duration::from_millis(4);`, `pub struct Pump { pub changed: bool, pub more: bool }`, `Pane::pump_until(&mut self, deadline: Instant) -> Pump` (replaces `pump`). It reads one chunk at a time and runs that chunk's `throughput.add`, `log_raw` and `hook_output` at once (the order of bytes is unchanged); after each chunk it stops when `Instant::now() >= deadline` with `more = true`, leaving the rest in the channel (a chunk is never split). The coprocess part and the exit check run only when `more` is false; `exited` is never set while output is unread.
- Produces: `#[cfg(test)] Io::Chunks(RefCell<VecDeque<Vec<u8>>>)` whose `try_read` hands one chunk per call.
- App: one deadline `now + PARSE_SLICE` per turn shared by all panes, starting at a pane that rotates each turn (`App.pump_from: usize`) so no pane starves; if any pane returned `more`, `App.parse_owed = true` and `about_to_wait` runs another slice (same function, `fn pump_panes(&mut self, event_loop)`) after the turn's redraws, with `ControlFlow::Poll` while owed. The `Notify` closure sends `UserEvent::Wake` only when `wake_queued.swap(true)` was false (`Arc<AtomicBool>`); the `Wake` handler stores false before pumping.

- [ ] **Step 1: Write the failing tests:** `a_slice_stops_at_its_deadline` (10 chunks of 64 KB, a deadline already past: one chunk parsed, `changed && more`; a far deadline then parses the other 9 and `more` is false); `slices_keep_the_bytes_in_order` (a screen fed through slices equals one fed in one go); `exit_waits_for_unparsed_output` (chunks pending and the shell exited: `exited` stays `None` until the last slice); `wakes_are_coalesced` (two notifies before the main thread runs send one event).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test a_slice_stops slices_keep exit_waits wakes_are_coalesced`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `scripts/check-unix --test pane session_end tmux ssh coprocess` and `scripts/xt test --lib -- pane session_end coprocess`. Expected: PASS.
- [ ] **Step 5: Bench** `scripts/draw-bench termurion`. Expected: flood at least 25 frames/s (1.1.8: 0.1) and at least 60 MB/s (1.1.8: 69.5).
- [ ] **Step 6: Commit** `Output: parse in slices so a flood keeps drawing`.

### Task 16: Whole verification

No version bump (the controller does it). Nothing new is written; a failure goes back to the task that owns it.

- [ ] **Step 1: Linux and macOS, full lib tests:** `scripts/check-unix --test`. Expected: `linux: P passed, 2 failed` and `macos: P passed, 2 failed`, the 2 known normalization tests only.
- [ ] **Step 2: glyphon:** `cargo test -p glyphon --lib` here, on funtimes, and `scripts/xt test -p glyphon --lib`. Expected: PASS.
- [ ] **Step 3: Windows:** `scripts/win-check` (`windows: 0 errors`), then `scripts/xt test --lib -- --test-threads=1`. Expected: only the 3 known failures.
- [ ] **Step 4: Golden fixtures:** `scripts/check-unix --test pane_frames_match_the_recorded_baseline` passes and `git status --porcelain tests/fixtures` prints nothing.
- [ ] **Step 5: OpenGL:** the pixel tests with `TERMURION_TEST_BACKEND=gl` (here, or on the W541 per Task 10 Step 3). Expected: PASS.
- [ ] **Step 6: Look at the screen on funtimes** (`funtimes-ui`) with Lightning on and with each of the other 12 plugins once: typing, `ls -la` scrolling, a selection, a search, a resize, tmux borders, Claude Code's input box, a CJK and emoji line, a sixel image. Any stale, missing or shifted pixel fails.
- [ ] **Step 7: Benchmark:** `scripts/mac-bundle`, install on funtimes, `scripts/draw-bench --passes 2`. Report the full table next to phase 1's. Expected (spec targets): Termurion claude with Lightning at or below Ghostty with the same shader (6.0%), stream20 and claude below Ghostty's CPU, idle Lightning at most Ghostty's 4.2%, flood live. Then give the owner the `sample` command for his own window (`sample <pid> 10 -file ~/termurion-draw.txt` while Claude streams with Lightning on); an agent opens nothing on his Mac.
- [ ] **Step 8: Report:** the table, any target missed with its profile, and the cell width difference against iTerm2 (out of scope, measured again in the same run: Termurion's window width for 220 columns against iTerm2's).
