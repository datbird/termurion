# Beat Ghostty (1.1.11) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Termurion at or below Ghostty's CPU in every `scripts/draw-bench` workload, with and without Lightning, and at or above Ghostty's flood frame rate and throughput, with every pixel unchanged.

**Architecture:** Cut the costs the Task 17 profile left, smallest risk first: menu sync only when output moved something a menu shows, per-wake ticks that skip when nothing is due, cheaper changed-row building, cheaper effect frames, a cheaper output path per byte, rows drawn in a few batched draws, effect shading limited to its region on built frames, and last, parsing while the frame is drawn and presented (a scoped worker thread over the panes, the drawing over the GPU; the compiler proves they share nothing).

**Tech Stack:** Rust 2024, winit (vendored), wgpu, glyphon 0.12 (vendored `crates/glyphon`), cosmic-text 0.19, alacritty_terminal (vendored), muda menus. Tests: `scripts/check-unix --test <filter>` (Linux here and macOS on funtimes), `scripts/xt test --lib -- <filter>` (Windows on EXTREME), `scripts/win-check`, `cargo test -p glyphon --lib`, `TERMURION_TEST_BACKEND=gl cargo test --lib render_pixel_tests`.

**Spec:** `docs/superpowers/specs/2026-11-20-beat-ghostty.md` (targets and the flood design, binding) and `docs/superpowers/specs/2026-11-18-draw-speed.md` ("What must not change", binding). Last profile: `.superpowers/sdd/2026-11-18-draw-speed/task-17-report.md`. Benchmark: `scripts/draw-bench`.

## Global Constraints

- The exact iTerm2 look must not change: pixels identical. Glyphs stay swash through cosmic-text with Termurion's mask filters (`set_mask_filter`), the same atlas content and the same placement (`physical()` rounding, `left_align`, `cluster_fixes`).
- The golden fixture test `scene::agent_box_tests::pane_frames_match_the_recorded_baseline` passes with every file in `tests/fixtures/pane-frames` unchanged (`git status --porcelain tests/fixtures` prints nothing). Never run it with `TERMURION_BLESS`. `Frame`'s `Debug` output stays byte-identical.
- Render changes are proven with the pixel harness (`src/render_pixel_tests.rs`, `crates/glyphon/src/row_render.rs` tests): the new path against today's, byte for byte, on every pane-frame case and agent screen, with view formats on and off, and once with `TERMURION_TEST_BACKEND=gl`.
- GPU tests use `test_gpu::shared` (never a new `wgpu::Instance`). Lavapipe here, Metal on funtimes, DX12 on EXTREME (high-performance adapter).
- GLES 3.0 rule: no storage buffers in vertex shaders, no `first_instance`; vertex buffer slices, instance-rate vertex attributes and dynamic uniform offsets are fine; a uniform block is at most 16384 bytes. The GL path without view formats (`Chain::without_view_formats`, `Renderer::set_view_formats(false)`) keeps working, on Linux and the W541.
- Every render plugin in `resources/render_plugins` (13), user plugins, Ghostty shaders and every FX profile in `resources/fx_profiles` keeps working.
- New dependencies: none, except `memchr` as a direct dependency at the version already in `Cargo.lock` (Task 5 only).
- Short human comments in code, only where the why is not obvious. No em dashes and no emoji anywhere: code, comments, docs, commit messages.
- No host or user names in commits. No attribution or Co-Authored-By lines. Work on main, commit each fix on its own, do not push. Do not bump the version and do not edit `docs/FIXES.md` (the controller does both).
- Known failures that are not regressions: Linux full suite, exactly the 151 in `.superpowers/sdd/2026-11-18-draw-speed/linux-baseline-failures.txt`; macOS adds `pane::tests::the_pwsh_script_*` (no pwsh on funtimes); Windows full suite has 3 (`handoff_win::tests::handles_are_typed_before_use`, `platform::windows::pty::tests::a_close_never_happens_while_attached`, `util_query::tests::it2ul_follows_the_script`). `trigger_eval` timing tests flake about 1 in 3 on this host (rerun alone). Run the full Windows suite with `--test-threads=1`. Never run two `scripts/xt` at once.
- Benchmarks run on funtimes only; never open a GUI or take a screenshot on the owner's Mac. To bench a build: `scripts/mac-bundle`; on funtimes `mv /Applications/Termurion.app ~/Termurion-release.app` (note its `shasum` of the binary), `ditto ~/src/termurion/target/release/Termurion.app /Applications/Termurion.app`, run `scripts/draw-bench`, then put the release bundle back and check the hash. The bench is slow; wait for it, never kill it. Leave no Termurion running on funtimes.
- **Profile command** (every task's Step 1): `scripts/draw-bench --sample --out target/draw-bench/<task>-before termurion termurion-fx`. It writes `sample-<config>-<workload>.txt` for stream20, claude and flood (Task 2 adds idle with `--sample-idle`). Read the main thread's call tree (inclusive counts, 1 ms samples) as the Task 17 report does, and the other threads for the flood. Record the sites and counts in the task's report before changing code.
- **Bench step** (every task's last step before the final commit): `scripts/draw-bench --passes 2 --out target/draw-bench/<task>-after termurion termurion-fx ghostty ghostty-fx`; put the CPU, fps and MB/s table against 1.1.10 (spec table) in the commit body.

## Review Focus

1. **One read changes a menu fact and prints text** (a title, a DEC mode, a kitty keyboard push, a named mark or a download inside a block of plain output), in a pane that is focused or not: the menu shows the new state after that read, on the macOS bar and the parked windows' bars. Test: Task 1 `every_menu_fact_moves_the_epoch`.
2. **A slot reused by a shorter row** (a long row dropped, a short one prepared into its slot, then batched with its neighbors): the old row's instances past the new length must draw nothing. Test: Task 6 `padding_draws_nothing`.
3. **The shell exits, or the window is resized, while the worker parses** (`cat bigfile; exit` with Parse While Drawing on): every byte shows before the end action runs, and a resize never meets a parse in progress. Test: Task 8 `an_exit_seen_by_the_worker_waits_for_the_tail`.
4. **A trigger fires on output the worker parses** (highlight, alert, inject data, set mark): same screen and same queued requests as the serial path. Test: Task 8 `the_worker_parses_like_the_slices`.
5. **Cells whose text is longer than one char** (combining marks, NFD under a normalization setting, a ZWJ emoji family over 8 bytes, VS16, a mirrored bidi character): the frame is the same as before. Test: Task 3 `cell_text_acts_like_a_string`.

---

## File Structure

- Create `src/app/menu_inputs.rs`: the output menu key (Task 1). Modify `src/screen.rs` (`MenuFacts`, `menu_epoch`), `src/app/mod.rs` (`after_output`).
- `src/app/mod.rs` (`about_to_wait` 8088-8365) and the tick functions the profile names; `scripts/draw-bench` (`--sample-idle`) (Task 2).
- `src/scene.rs` (`CellLook` 569, `row_looks` 1623, `build_row` 1803, `pane_with` 2047), create `src/cell_text.rs` (Task 3).
- `src/app/mod.rs` (`render_frame` 7525-7790), `src/app/fx_ui.rs`, `src/render_fx_gpu.rs`, `src/render.rs` (`draw_fx_for` 1493) (Tasks 4, 7).
- `src/pane.rs` (`pump_until` 794, `hook_output` 878, `route_output` 939), `src/host_client.rs` (`read_loop` 136), `src/platform/unix/pty.rs` (reader near 418), `src/ssh_hook.rs`, `src/tmux_hook.rs` (Task 5).
- `crates/glyphon/src/row_render.rs`, `crates/glyphon/src/row_shader.wgsl`, `crates/glyphon/src/cache.rs` (row pipeline layout), `crates/glyphon/TERMURION-PATCHES.md` (patch 8) (Task 6).
- `resources/render_plugins/lightning/plugin.toml` (Task 7, only if its region test allows).
- Create `src/app/parse_meanwhile.rs`; modify `src/app/mod.rs` (`pump_panes` 518, `render_frame`, the `RedrawRequested` arm), `src/cadence.rs`, `schema/global.toml` (Task 8).

---

### Task 1: Menu checks sync only when output moved something a menu shows (low risk)

**Files:**
- Create: `src/app/menu_inputs.rs` (`mod menu_inputs;` in `src/app/mod.rs`)
- Modify: `src/screen.rs` (`feed` 1453), `src/app/mod.rs` (`after_output` 5824, App fields), `src/app/menu.rs` (`sync_checks` 778, read only for the audit)
- Test: `src/screen.rs` tests, `src/app/menu_inputs.rs` tests

**Interfaces:**
- Produces: `#[derive(Clone, PartialEq, Debug, Default)] pub struct MenuFacts` in `src/screen.rs`, one field per thing a menu reads from a `Screen` (audit in Step 1), and `Screen::menu_facts(&self) -> MenuFacts`.
- Produces: `Screen::menu_epoch(&self) -> u64`. `feed` takes `menu_facts()` before and after `feed_untimed` and adds 1 when they differ. No other writer bumps it.
- Produces: `#[derive(Clone, PartialEq, Debug)] pub(super) struct OutputMenuKey { focused: Option<PaneId>, epochs: u64, exited: Option<bool>, logging: Option<bool>, selection: bool, transfers: u64, menus_dirty: bool }` and `App::output_menu_key(&self) -> OutputMenuKey`. `epochs` is the wrapping sum over all panes of `own().menu_epoch()` and `screen.menu_epoch()`; `exited` and `logging` are the focused pane's `ended || exited.is_some()` and `log.is_some()`; `selection` is `has_selection(own)` of the focused pane; `transfers` is a counter `Transfers::generation(&self) -> u64` raised by every change to its rows.
- Produces: `App.menu_key: Option<OutputMenuKey>`; `after_output` calls `sync_checks()` only when `Some(self.output_menu_key()) != self.menu_key`, then stores the key. Every other `sync_checks()` caller is unchanged and also stores the key after it runs.

- [ ] **Step 1: Profile and audit.** Profile command (Global Constraints). Then list, in the task report, every input `sync_checks` and each function it calls reads (`refresh_named_marks`, `refresh_window_list`, `refresh_transfer_menus`, `refresh_workgroups_menu`, `refresh_triggers_menu`, `can_explain`, `sync_move_session_items`, `TOGGLES`, `KEY_MODES`, `KITTY_FLAGS`, `LEVELS`, the marks checks and the rest). For each: can pty output change it? If yes, it goes in `MenuFacts` (Screen state: DEC mode bits, keyboard mode and kitty flags, level, literal controls, title, current directory and remote host, named marks and the last command mark, alert-next, downloads and `file_aborted`, tab color, user variables a workgroup fact reads) or in `OutputMenuKey`. If no, name the writer and show it already calls `sync_checks`, `relayout` or sets `menus_dirty`; add the call where it is missing. The window list's titles already refresh from `render_frame` (`window_menu_text`); say so.
- [ ] **Step 2: Write the failing tests.**
  - `every_menu_fact_moves_the_epoch` (screen tests): for each sequence, a fresh 80x24 `Screen` fed `b"plain text\r\n"` first; then the sequence inside plain text (`b"abc" + seq + b"def"`) raises `menu_epoch()` by 1: `\x1b]0;title\x07`, `\x1b]7;file://h/tmp\x07`, `\x1b[?1h`, `\x1b[?2004h`, `\x1b[>1u`, `\x1b]1337;SetMark\x07`, `\x1b]133;A\x07\x1b]133;C\x07\x1b]133;D;0\x07`, `\x1b]6;1;bg;red;brightness;255\x07`, `\x1b]1337;File=name=eA==;size=1:eA==\x07`, plus one per further fact Step 1 found.
  - `plain_output_keeps_the_epoch`: 1000 lines of colored text, `\r`, `\x1b[K`, cursor moves: `menu_epoch()` unchanged.
  - `after_output_syncs_only_on_a_new_key` (source scan of `after_output`'s body): it contains `output_menu_key()` and `self.sync_checks()` appears only inside the `!=` branch.
  - `the_key_follows_focus_and_dirty` (pure, in `menu_inputs.rs`): two keys differing in only `focused`, only `menus_dirty`, only `selection` compare unequal.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test every_menu_fact plain_output_keeps after_output_syncs the_key_follows`. Expected: FAIL (missing items).
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** `scripts/check-unix --test screen menu marks transfers app::` and `scripts/xt test --lib -- screen menu marks transfers`. Expected: PASS apart from the baseline. `scripts/win-check`: `windows: 0 errors`.
- [ ] **Step 6: Check the macOS bar on funtimes** through `funtimes-ui`: in Termurion there, `printf '\e[?1h'` then open Edit (or the menu holding the Terminal State toggles): Application Cursor Keys is checked; `printf '\e[?1l'`: unchecked. Same for a kitty push (`printf '\e[>1u'`) and a named mark (`printf '\e]1337;SetMark\a'` then the Named Marks list).
- [ ] **Step 7: Bench** (Global Constraints). Expected: `sync_checks` under 5 samples in the claude profile (Task 17: 43), claude and stream20 CPU below 1.1.10's.
- [ ] **Step 8: Commit** `Menus: output syncs the check marks only when it moved one`.

### Task 2: Per-wake ticks skip when nothing is due (low risk)

**Files:**
- Modify: `scripts/draw-bench` (`--sample-idle`: line 296 also samples idle when set), `src/app/mod.rs` (`about_to_wait`), the tick functions Step 1 names (Task 17 named `hotkey_ui::termurion_in_front`, `tick_status_bars`, `chat_tick`, `tick_fx_profiles`, `sync_menu_hidden`)
- Test: each tick's own module tests

**Interfaces:**
- Produces: `scripts/draw-bench --sample-idle` (implies `--sample`, adds `sample-<config>-idle.txt`).
- Produces, per tick fixed: a gate that returns before any work when the tick's state says nothing can be due (a stored `Option<Instant>` due time, or a dirty flag set by the writer of its inputs), keeping its return value (`Option<Instant>`) exactly what it was. Name each gate `<tick>_idle(&self) -> bool` or keep the due time in a field named `<tick>_due`.

- [ ] **Step 1: Profile** with `scripts/draw-bench --sample --sample-idle --out target/draw-bench/t2-before termurion termurion-fx`. Under `about_to_wait`, list every callee with 2 or more inclusive samples in idle, stream20 or claude, and for each the work it does when it has nothing to do (an OS call, an allocation, a walk over panes or windows, a settings read).
- [ ] **Step 2: Write the failing tests**, two per tick fixed: `<tick>_does_nothing_when_nothing_is_due` (a counter of the work it skips, or a pure gate function, shows no work on a call with nothing due) and `<tick>_still_acts_when_due` (the same tick with its input changed, or its time come, does the work and returns the same due time as before the change). For `termurion_in_front` the state may be cached only if it is refreshed by every event that can change it (`WindowEvent::Focused` on any of our windows and the app activation events in `platform::app_events`); otherwise leave it.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test <the new test names>`. Expected: FAIL.
- [ ] **Step 4: Implement** the gates.
- [ ] **Step 5: Run** `scripts/check-unix --test app:: hotkey statusbar chat fx_profile` and `scripts/xt test --lib -- app:: hotkey statusbar`. Expected: PASS apart from the baseline. `scripts/win-check`: 0 errors.
- [ ] **Step 6: Bench.** Expected: `about_to_wait` under 60 samples in claude (Task 17: 118); idle at or below Ghostty's.
- [ ] **Step 7: Commit** one commit per tick, `Idle loop: <tick> skips when nothing is due`, and the script change as `Bench: sample the idle prompt too`.

### Task 3: A changed row is cheaper to build (low risk)

**Files:**
- Create: `src/cell_text.rs` (`mod cell_text;` in `src/lib.rs`, next to `mod scene;`)
- Modify: `src/scene.rs` (`CellLook.text`, `row_looks`, `build_row`, `pane_with`'s `orders` Vec at 2131), `src/scene_rows.rs` (`RowCache` keeps the scratch)
- Test: `src/cell_text.rs` tests, `src/scene.rs` `row_reuse_tests`

**Interfaces:**
- Produces: `pub struct CellText` holding up to 8 bytes inline and a `String` beyond that, with `new() -> Self`, `push(&mut self, c: char)`, `extend<I: IntoIterator<Item = char>>(&mut self, it: I)`, `as_str(&self) -> &str`, `is_empty`, `is_ascii`, `chars`, `replace_first(&mut self, with: char)` (what the bidi mirror does), and `normalize(&mut self, form: crate::encoding::Form)` that leaves ASCII untouched and otherwise runs `normalize_in_place` on a `String`. `CellLook.text: CellText`; every reader uses `as_str()`.
- Produces: `RowCache::scratch(&mut self) -> &mut Vec<CellLook>` (cleared, capacity kept), used by `build_row` through `RowCtx`, and the `orders` Vec kept in the `RowCache` between frames. A pane built without a cache keeps today's allocations.

- [ ] **Step 1: Profile.** Record `build_frame`, `pane_with`, `build_row`, `row_looks` and the malloc and free under them (Task 17: 213, 161, 99, 42).
- [ ] **Step 2: Write the failing tests.**
  - `cell_text_acts_like_a_string`: for `"a"`, `"é"`, `"e\u{301}"`, `"\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}"` (25 bytes), `"\u{2764}\u{fe0f}"`, `"("` mirrored to `")"`, and each `encoding::Form`: the same pushes and calls on a `String` and a `CellText` give the same `as_str()`.
  - `rows_build_the_same_frames_with_the_scratch`: every `baseline_cases()` frame built twice through one `RowCache` (scratch reused) and once without a cache: `format!("{:#?}")` equal.
  - The golden test and every `both_ways` test unchanged.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test cell_text rows_build_the_same`. Expected: FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** `scripts/check-unix --test scene scene_rows bidi pane_frames_match_the_recorded_baseline` and `scripts/xt test --lib -- scene scene_rows`. Expected: PASS apart from the baseline; `tests/fixtures` untouched.
- [ ] **Step 6: Bench.** Expected: `row_looks` under 20 samples in claude, `build_frame` under 150.
- [ ] **Step 7: Commit** `Rows: cell text stays inline and row building reuses its buffers`.

### Task 4: An effect frame costs less CPU (low to medium risk)

**Files:**
- Modify: `src/app/mod.rs` (`render_frame`), `src/app/fx_ui.rs` (`prepare`, `region`), `src/render_fx_gpu.rs` (`run_with` 501), `src/render.rs` (`draw_fx_for`)
- Test: `src/app/scene_cache.rs` tests (source scans), `src/render.rs` GPU tests

**Interfaces:**
- Consumes: Task 2's `--sample-idle`.
- Produces: on a frame that will reuse the scene (`will_reuse`), `render_frame` also skips every call Step 1 shows doing frame work that cannot change on an animation frame (candidates: `check_composer_fields`, `sync_menu_hidden`, `publish_snap`, `sync_surface_alpha` when the alpha did not change, the `fx_cell` and `fx_signals` reads, texture view creation that can be kept per surface texture size).
- Produces: `Chain::skips(&self) -> u64` and, in `render_frame`, a frame is left out (no `get_current_texture`) only when the scene is the same and the packed uniform bytes of every pass equal last frame's. Lightning never meets this while animating (spec fact 1); it covers paused and unfocused effects whose uniforms stand still.

- [ ] **Step 1: Profile** `scripts/draw-bench --sample --sample-idle --out target/draw-bench/t4-before termurion-fx`. In idle, split the main thread per animation frame: `render_frame` and each callee, `get_current_texture`, `Chain::run_with`, `present`, `about_to_wait`. Divide by frames drawn. Record ms per frame against Ghostty's 0.6.
- [ ] **Step 2: Write the failing tests:** `an_animation_frame_skips_the_frame_work` extended (source scan: each call Step 1 moved sits behind `will_reuse`); `a_still_effect_frame_is_not_drawn` (two frames with the same scene and the same `Uniforms` bytes: the second is skipped, `skips()` is 1; a changed `time` draws); `kept_bind_groups_draw_the_same_pixels` and `region_frames_match_full_frames_for_every_shipped_plugin` pass unchanged.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test an_animation_frame_skips a_still_effect_frame`. Expected: FAIL.
- [ ] **Step 4: Implement**, one site at a time, rerunning the region and pixel tests after each.
- [ ] **Step 5: Run** `scripts/check-unix --test render render_fx scene_cache fx_` and `scripts/xt test --lib -- render_fx render::tests`. Expected: PASS.
- [ ] **Step 6: Bench.** Expected: termurion-fx idle at or below ghostty-fx (4.3 in 1.1.9's run); report ms per animation frame.
- [ ] **Step 7: Commit** one per site, `Effects: an animation frame skips <what>`.

### Task 5: The output path costs less per byte (low to medium risk)

**Files:**
- Modify: `Cargo.toml` (`memchr`, the version in `Cargo.lock`), `src/pane.rs` (`hook_output` 878, `route_output` 939, `log_raw`, `tee_raw`, `decode_output`, `watch_stuck`), `src/ssh_hook.rs`, `src/tmux_hook.rs`, `src/host_client.rs` (`read_loop` 136-232), `src/platform/unix/pty.rs` (reader near 418), `src/screen.rs` (the OSC pre-scan in `feed_chunk`)
- Test: each module's tests

**Interfaces:**
- Produces: no new public API. Each pass over a chunk that today walks every byte (the `windows(start.len()).position` scan for the tmux start at `hook_output`, the SSH hook's scan for its DCS, the OSC scanner, the stuck watch, the cooked log when no log is open, decoding when the encoding is UTF-8) either uses `memchr`/`memchr::memmem` for its trigger byte or is skipped when it has nothing to do. Copies of a chunk (`bytes[skip..].to_vec()` in `read_loop`, `ssh_hook.take_held()` concatenation, `decode_output` returning a new Vec for UTF-8) become moves where the bytes are not changed.

- [ ] **Step 1: Profile the flood** (`sample-termurion-flood.txt`, every thread). Record per thread: main, the host reader thread, Metal, and `termurion-host`'s CPU (`ssh funtimes 'ps -o pid,%cpu,comm -ax | grep termurion'` during a flood). Record parse samples per MB and each pass and copy above with its samples. State which thread is at 100%.
- [ ] **Step 2: Write the failing tests**, one per changed pass, each comparing the new pass with the old one kept in the test: `hook_scan_finds_the_start_like_before` (random chunks of 1 to 70000 bytes with the tmux start placed at every offset of the first 64, across a chunk boundary, and absent: same cut), `ssh_hook_feed_matches_the_byte_walk`, `osc_prescan_matches_the_byte_walk`, `utf8_decode_moves_the_bytes` (`decode_output` of a UTF-8 chunk returns the same allocation: pointer equal). Each fails first by running the new function name before it exists.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test hook_scan ssh_hook_feed osc_prescan utf8_decode`. Expected: FAIL.
- [ ] **Step 4: Implement**, one pass per commit.
- [ ] **Step 5: Run** `scripts/check-unix --test pane screen ssh tmux session_log coprocess host_client` and `scripts/xt test --lib -- pane screen ssh tmux session_log coprocess`. Expected: PASS apart from the baseline. `scripts/win-check`: 0 errors.
- [ ] **Step 6: Bench** `termurion`. Expected: flood MB/s up from 68.5 with CPU per MB/s at or under 3.69; report parse samples per MB before and after.
- [ ] **Step 7: Commit** one per pass, `Output: <pass> scans with memchr` or `Output: <copy> moves the bytes`.

### Task 6: Rows draw in a few batched draws (medium risk)

**Files:**
- Modify: `crates/glyphon/src/row_render.rs` (`RowRenderer`, `alloc` 356, `drop_entries` 388, `write_table` 412, `render` 452), `crates/glyphon/src/row_shader.wgsl`, `crates/glyphon/src/cache.rs` (row pipeline: second vertex buffer, table bind group layout), `crates/glyphon/TERMURION-PATCHES.md` (patch 8)
- Test: `crates/glyphon/src/row_render.rs` tests, `src/render_pixel_tests.rs`

**Interfaces:**
- Consumes: patch 6 and 7 (`RowRenderer`, `UploadBelt`).
- Produces: `pub fn draws_issued(&self) -> u64` on `RowRenderer` (draw calls recorded by the last `render`, for tests).
- Decisions:
  - **Row ids.** Each `Entry` gets a stable `id: u32` from a free list (lowest first) when it is made and gives it back when dropped. The table holds one `RowUniform` per id (not per draw): `struct RowTable { rows: array<RowUniform, 512> }` (512 x 32 = 16384 bytes). An id's entry is written only when that row's `(offset, clip)` changed. Ids of 512 and up use the next 16384-byte chunk through the dynamic offset; the shader reads `rows[id % 512]`.
  - **Id per instance.** A second vertex buffer, instance rate, one `u32` per instance at the same index as the instance buffer, `@location(6) row_id: u32`. A slot's ids are written with its instances.
  - **Padding.** When an entry takes a slot, the whole slot (its class capacity) is written: the row's instances, then zero instances (`dim = 0`) with the row's id. `vs_row` treats `dim == 0` as culled (a zero-size quad), which changes no real glyph: a glyph is never prepared with a zero size.
  - **Batching keeps the order.** `render` walks the rows in the order `prepare` was given them and extends the current batch with the next row only when its slot starts where the batch's last slot ends (its full capacity) and its id is in the same 512-chunk; otherwise it ends the batch. One batch is `set_bind_group(2, table, &[chunk * 16384])` when the chunk changed, two `set_vertex_buffer` slices over the batch's slots, and `draw(0..4, 0..instances)`. The rows draw in exactly today's order, so overlapping glyphs blend as today.
  - **Slots in draw order.** New slots come from the free list of their class when one exists, else from `end`, as today; a fresh screen is laid out in draw order, and a scroll leaves two batches (the kept run and the reused slot).

- [ ] **Step 1: Profile.** Record `CommandEncoder::finish` and the render pass recording under `draw_with` in stream20 and claude (Task 17: 111 and 49).
- [ ] **Step 2: Write the failing tests** in `row_render.rs`: `batched_rows_draw_like_one_draw_per_row` (the 10-row mixed-bounds case of `rows_draw_like_the_text_renderer` and the 56-row screen: pixels equal to `TextRenderer`); `a_scrolled_screen_draws_in_few_batches` (56 rows, then the same rows one up plus a new last row: `draws_issued() <= 3`); `padding_draws_nothing` (a 60-glyph row dropped, a 3-glyph row prepared into its slot, drawn between neighbors: pixels equal to `TextRenderer`); `rows_past_one_table_chunk_draw_right` (600 one-glyph rows: pixels equal, `draws_issued() >= 2`); and in `render_pixel_tests.rs` the existing `row_instances_match_the_full_path_on_every_case`, `scrolled_rows_reuse_their_instances`, `fractional_tops_match_the_full_path`, `atlas_growth_and_eviction_mid_run_match` unchanged.
- [ ] **Step 3: Run** `cargo test -p glyphon --lib`. Expected: the new tests FAIL.
- [ ] **Step 4: Implement**; record patch 8 in `TERMURION-PATCHES.md` with `// Termurion patch 8:` at each site.
- [ ] **Step 5: Run** `cargo test -p glyphon --lib` here, on funtimes (after `scripts/check-unix --mac-only` synced the tree, `ssh funtimes 'cd src/termurion && ~/.cargo/bin/cargo test -p glyphon --lib'`) and `scripts/xt test -p glyphon --lib`; `scripts/check-unix --test render render_pixel_tests pane_frames_match_the_recorded_baseline`; `TERMURION_TEST_BACKEND=gl cargo test --lib render_pixel_tests`; `scripts/xt test --lib -- render`. Expected: PASS everywhere, fixtures untouched.
- [ ] **Step 6: Bench.** Expected: `CommandEncoder::finish` under half of Step 1's in both workloads; stream20 and claude CPU at or below Ghostty's.
- [ ] **Step 7: Commit** `glyphon: rows draw in batches with a row id per glyph`.

### Task 7: A built effect frame shades only the effect's region (medium risk)

**Files:**
- Modify: `src/render.rs` (`draw_fx_for` 1493: the region also on built frames), `src/render_fx_gpu.rs` (`run_with`: a built frame first copies the new plain frame into `kept_out`, then shades this frame's rects, then copies out), `resources/render_plugins/lightning/plugin.toml` (only as Step 4 allows)
- Test: `src/render.rs` GPU tests

**Interfaces:**
- Consumes: Task 9 of the draw-speed plan (`region_rects`, `PxRect`, `run_with(.., rects)`, `keep_output`).
- Produces: `draw_fx_for` passes the region whenever it is bounded, built or not. On a built frame the rects are this frame's only (everything outside them is the new plain frame); on an animation-only frame they stay this frame's united with last frame's.

- [ ] **Step 1: Write the failing tests** (both `Chain::new` and `without_view_formats`, `keep_alpha` false and true): `built_region_frames_match_full_frames` (for each shipped plugin with a bounded region, on each agent screen, the draw-speed sequence of shots, but every shot a built frame with the next agent screen's rows changed: byte-equal to the full pass); `a_built_frame_after_an_animation_frame_matches` (animation-only, built, animation-only in a row: each equal to the full pass).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test built_region_frames a_built_frame_after`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Lightning's region.** Lightning declares `full_after_change:1000`. Replace it with bounded terms read from its shader (the bolt's `reach` along `cursor_path`, the surge's `LINE_REACH` band on the input box lines, the sent row) only if `region_frames_match_full_frames_for_every_shipped_plugin` and Step 1's tests pass with them; otherwise keep the line and say why in the commit body.
- [ ] **Step 5: Run** `scripts/check-unix --test render render_fx fx_profile fx_store` and `scripts/xt test --lib -- render_fx render::tests`. Expected: PASS.
- [ ] **Step 6: Bench.** The shading cut is GPU work the VM does not show as CPU (draw-speed spec); report WindowServer % next to CPU and check termurion-fx stream20 and claude did not rise.
- [ ] **Step 7: Commit** `Effects: built frames shade only where the effect draws`.

### Task 8: Output is parsed while the frame is drawn (high risk)

#### Design

- **Thread model.** No new long-lived thread. In `render_frame`, after `build_frame` and after the last read of `self.panes` (`fx_cell`, `fx_signals`), `std::thread::scope` spawns one worker that gets `&mut self.panes` and a `ParseJob`; the main thread keeps `self.gpu` and the frame and runs `get_current_texture`, the draw, the menu bar strip and `present` as today. Both borrows are disjoint fields of `App`, so the compiler proves the worker and the drawing share nothing. The worker stops when the main thread sets `stop` after `present`; the scope then joins it.
- **Locking.** None. The worker owns the panes for the scope; no main-thread code that reads a pane can run inside it (it would not compile).
- **What the worker does.** For each eligible pane, in `pump_from` rotation, `prep_for_pump` (the per-pane settings `pump_panes` sets today) and `pump_until(Instant::now() + PARSE_SLICE)`, round after round, until `stop` is set or no pane has `more`. A chunk is never split (64 KB at most, about 1 ms), so the main thread waits at most one chunk at the join.
- **Eligible panes.** `Io::Local` or `Io::Hosted`, not a gateway, `!tmux_hook.active()`, `!ssh_hook.active()`, `ssh_q` and `tmux_q` empty, no conductor, no coprocess. Others are parsed by the main thread's slices as today.
- **What the draw reads.** The `Frame` (owned, built before the scope), the fx uniforms and signals (read before), and `gpu`. Nothing of a pane.
- **After the scope.** `render_frame` returns the worker's `Parsed`; the `RedrawRequested` arm passes it to `pump_tail(parsed, event_loop)`, the tail of `pump_panes` moved into its own function (outputs, `fx_signal.output`, `stuck_tick`, `api_output`, `api_session_events`, `record_replay_frames`, `handle_ended`, `check_coprocesses`, `after_output`, `pty_wake.turn_ended`, the rotation). `pump_panes` runs its loop and then the same `pump_tail`. So every frame is built only after the tail of all output parsed before it, as today.
- **Resize.** `relayout` and `Pane::resize` run only on the main thread outside the scope; the worker never meets a resize.
- **Exit.** `pump_until` sets `exited` only when no output is unread, on whichever thread ran it; `handle_ended` runs in the tail on the main thread.
- **Instant Replay.** Damage accumulates in the `Screen` during the worker's parse; `record_replay_frames` captures in the tail as it does after a slice.
- **Triggers.** They run inside `Screen::feed` on the worker; their actions on the screen happen there, and their requests wait in the screen's queue for `after_output` (`take_requests`) as today. Step 1 audits that nothing reachable from `pump_until` calls an OS UI API (AppKit, muda, winit, notifications, the clipboard); any such call makes that pane ineligible or is deferred into the queue.
- **Coprocesses, tmux integration, SSH integration.** Their panes are not eligible.
- **The API.** `api_output` and `api_session_events` run in the tail; API reads of a screen happen on the main thread outside the scope.
- **Fallback.** Advanced setting `ParseWhileDrawing` (default true). Off: no scope, `render_frame` returns no `Parsed`, and `cadence` keeps the 30 fps flood rate: exactly 1.1.10's path.
- **Flood frame rate.** With the setting on, `cadence::interval` uses `maximum_frame_rate` where it uses `metal_slow_frame_rate` today (spec fact 2).

**Files:**
- Create: `src/app/parse_meanwhile.rs` (`mod parse_meanwhile;`)
- Modify: `src/app/mod.rs` (`pump_panes` 518-605 split into the loop and `pump_tail`, `render_frame`, the `RedrawRequested` arm, `about_to_wait`'s owed check), `src/app/frame_settings.rs` (`parse_while_drawing`), `src/cadence.rs` (`Prefs.parse_while_drawing`, `interval`), `schema/global.toml` (the setting, after `RedrawOnlyChangedRows`, tab Advanced, section Drawing)
- Test: `src/app/parse_meanwhile.rs` tests, `src/cadence.rs` tests, `src/app/mod.rs` source scans

**Interfaces:**
- Produces: `pub(super) struct PumpPrep { pub auto_composer: bool, pub smart_logging: bool, pub always_line_marks: bool }` and `pub(super) fn prep_for_pump(p: &mut Pane, prep: &PumpPrep)`.
- Produces: `pub(super) fn eligible(p: &Pane) -> bool` (the rule above).
- Produces: `pub(super) struct Parsed { pub results: Vec<(PaneId, Pump)>, pub owed: bool, pub first_more: Option<usize>, pub start: usize }` and `pub(super) fn parse_meanwhile(panes: &mut HashMap<PaneId, Pane>, order: &[PaneId], prep: &PumpPrep, stop: &AtomicBool) -> Parsed`.
- Produces: `App::pump_tail(&mut self, parsed: Parsed, event_loop: &ActiveEventLoop)`; `render_frame(&mut self, timing: bool) -> Option<Parsed>`.
- Produces: `cadence::Prefs.parse_while_drawing: bool` (read from `ParseWhileDrawing`); setting help text: "Parse output while a frame is drawn\nTermurion reads program output on a second thread while the window draws, so a flood of output keeps drawing at the full frame rate. Turn this off if output shows late or out of order and report it."
- Produces: `const _: fn() = || { fn send<T: Send>() {} send::<crate::pane::Pane>(); };` in `parse_meanwhile.rs`.

- [ ] **Step 1: Audit.** Add the `Send` assertion and run `scripts/check-unix --linux-only` and `scripts/win-check` (Linux passes at 05ce84ac; if Windows does not, stop and report which field is not `Send`). Walk `pump_until`'s call graph (`hook_output`, `route_output`, `Screen::feed`, triggers, `log_raw`, `reply`, the coprocess tee) and list every `crate::platform::` call and every `thread_local!` it reaches, with why each is safe on another thread; write the list into the task report.
- [ ] **Step 2: Write the failing tests** (in `parse_meanwhile.rs`, with `Io::Chunks` panes):
  - `the_worker_parses_like_the_slices`: two panes, each with a trigger profile (highlight, set mark, alert) and 200 chunks of colored text, OSC titles and marks; one copy parsed by `parse_meanwhile` on a scoped thread with `stop` set after 1 ms and rounds repeated until caught up, the other by `pump_until` slices on this thread: `format!("{:?}")` of each screen's grid, marks, title and queued requests equal.
  - `stop_ends_the_parse_after_one_chunk`: `stop` already set: each eligible pane parsed at most one chunk, `owed` true.
  - `only_plain_panes_are_eligible`: `eligible` false for a gateway, a pane with an active tmux hook, an active SSH hook, a conductor, a coprocess, a browser pane; true for a plain `Io::Chunks` pane.
  - `an_exit_seen_by_the_worker_waits_for_the_tail`: chunks pending and the shell exited: `exited` stays `None` while `owed`; after the last round it is set and `handle_ended` has not run (it runs only in `pump_tail`).
  - `the_scope_reads_no_pane` (source scan of `render_frame`): between `thread::scope(` and its end, no `self.panes`, `focused_pane(`, `pane_cell(` or `self.tabs`.
  - `the_switch_restores_the_old_path` (source scan): `parse_meanwhile(` is reached only under `parse_while_drawing`, and `pump_panes` ends in `pump_tail(`.
  - `a_flood_draws_at_the_full_rate_only_with_the_switch` (cadence): throughput 50_000, visible: 15_667 us with `parse_while_drawing` true, 32_333 us with it false; every other existing cadence test unchanged.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test parse_meanwhile a_flood_draws_at_the_full_rate`. Expected: FAIL.
- [ ] **Step 4: Implement**, first `pump_tail` and `prep_for_pump` with no behavior change (run `scripts/check-unix --test pane session_end tmux ssh coprocess app::`, PASS, commit `Output: the work after a parse is one function`), then the worker, the setting and the cadence rule.
- [ ] **Step 5: Run** `scripts/check-unix --test pane session_end tmux ssh coprocess app:: cadence trigger replay dvr` and `scripts/xt test --lib -- pane session_end coprocess app:: cadence trigger`. Expected: PASS apart from the baseline. `scripts/win-check`: 0 errors.
- [ ] **Step 6: Hand checks on funtimes** (through `funtimes-ui`, Parse While Drawing on): `cat` of a 200 MB file then `exit` in the same command (the pane's end action runs after the last line shows); a window resize during `yes | head -c 500000000`; a tmux integration session (`tmux -CC`) with output in two panes; a trigger that highlights `ERROR` in a flood; the same with the setting off. Record what each showed.
- [ ] **Step 7: Bench** `termurion termurion-fx ghostty ghostty-fx`, both with the setting on and off. Expected with it on: flood fps at or above Ghostty's, MB/s at or above Ghostty's, CPU per MB/s at or under 3.69; with it off: 1.1.10's flood within noise.
- [ ] **Step 8: Commit** `Output: parse while the frame is drawn`.

### Task 8b: The frame rate follows the display (low to medium risk)

Owner ruling 2026-11-19: draw up to the display's own refresh rate, as Ghostty does (60, 120, 144 Hz). Windows battery saver keeps iTerm2's 30 fps.

**Files:**
- Modify: `src/cadence.rs` (`State.display_hz`, `Prefs.match_display`, `interval`, `shortest_interval`), the window code that builds each window's `State` list (find it from `window_interval`'s callers), `src/app/frame_settings.rs`, `schema/global.toml` (setting `MatchDisplayRefreshRate`, default true, tab Advanced, section Drawing, after `ParseWhileDrawing`)
- Test: `src/cadence.rs` tests, a source scan in the window code

**Interfaces:**
- Produces: `State.display_hz: Option<f64>`: the refresh rate of the monitor the window is on, from winit `current_monitor()?.refresh_rate_millihertz() / 1000`, read when the window is made and again on `WindowEvent::Moved`, `ScaleFactorChanged` and `Resized` (a move to another monitor); `None` when the OS does not report it.
- Produces: `Prefs.match_display: bool` (from `MatchDisplayRefreshRate`). When true and `display_hz` is `Some(hz)`, every rate in `interval` that is 60 fps by default (`maximum_frame_rate`, `active_update_cadence`, and the flood rate when Task 8's `parse_while_drawing` gives `maximum_frame_rate`) becomes `hz` clamped to 30..=240, but only while that setting still has its default value of 60. A value the user set by hand wins. `low_power_frame_rate` (30) and `metal_slow_frame_rate` with Parse While Drawing off are unchanged. `shortest_interval` follows the same rule.
- Setting help text: "Match the display refresh rate\nTermurion draws up to the refresh rate of the display the window is on (for example 120 frames a second on a ProMotion display) instead of 60. Battery saver still draws at most 30."

- [ ] **Step 1: Write the failing tests** (cadence): `a_120_hz_display_draws_at_120` (light output, `display_hz` 120: the 120 fps gap from `interval`'s formula); `a_flood_on_120_hz_draws_at_120_with_parse_while_drawing`; `battery_saver_keeps_30_on_any_display`; `a_hand_set_maximum_wins` (`maximum_frame_rate` 45, display 120: 45); `no_display_rate_keeps_60`; `the_switch_off_keeps_60` (`match_display` false, display 120: 60); and a source scan that `display_hz` is refreshed on `Moved`, `ScaleFactorChanged` and `Resized`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test cadence`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.** Check the surface's present mode on each OS does not hold the rate at 60 (Fifo follows the display; say what each backend uses).
- [ ] **Step 4: Run** `scripts/check-unix --test cadence app::` and `scripts/xt test --lib -- cadence app::`. Expected: PASS apart from the baseline. `scripts/win-check`: 0 errors.
- [ ] **Step 5: Bench** `termurion termurion-fx ghostty ghostty-fx` and record the funtimes display rate the app read. Expected: no CPU rise on a 60 Hz display.
- [ ] **Step 6: Commit** `Frames: draw up to the display refresh rate`.

### Task 9: Whole verification

No version bump. Nothing new is written here; a failure goes back to the task that owns it.

- [ ] **Step 1:** `scripts/check-unix --test`. Expected: Linux exactly the baseline list, macOS the same plus the pwsh tests.
- [ ] **Step 2:** `cargo test -p glyphon --lib` here, on funtimes and with `scripts/xt test -p glyphon --lib`. Expected: PASS.
- [ ] **Step 3:** `scripts/win-check` (0 errors), then `scripts/xt test --lib -- --test-threads=1`. Expected: only the 3 known failures.
- [ ] **Step 4:** `scripts/check-unix --test pane_frames_match_the_recorded_baseline` passes and `git status --porcelain tests/fixtures` prints nothing.
- [ ] **Step 5:** `TERMURION_TEST_BACKEND=gl cargo test --lib render_pixel_tests` here, and the same on the W541 in its checkout. Expected: PASS.
- [ ] **Step 6: Look at the screen on funtimes** with Lightning and each other plugin once: typing, `ls -la` scrolling, a selection, a search, a resize, tmux borders, Claude Code's input box, a CJK and emoji line, a sixel image, the Edit and View menus' check marks after `printf '\e[?1h'`. Any stale, missing or shifted pixel, or a wrong check mark, fails.
- [ ] **Step 7: Benchmark:** `scripts/draw-bench --passes 2 --out target/draw-bench/final termurion termurion-fx ghostty ghostty-fx iterm2`. Report the full table next to the spec's 1.1.10 table and mark each of the spec's four targets met or missed; a missed one gets its profile (`--sample`).
- [ ] **Step 8: Report** the table, the targets, and any switch a user may need (`ParseWhileDrawing`).
