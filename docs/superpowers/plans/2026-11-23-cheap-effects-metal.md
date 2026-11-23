# Cheap Effects and Full Metal on macOS (1.1.12) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An effect costs so little CPU per frame that it can be on just for fun: every effect row meets or beats Ghostty's effect row at the new 60 frames/s default, macOS draws everything through native Metal, and Windows and Linux get the shared savings.

**Architecture:** Phase A cuts the App-side work on frames where only the effect moves (all OSes) and adds the 60 frames/s "Smooth" choice. Phase B splits drawing into a backend-neutral draw list built on the CPU (`DrawList`) and a backend that runs it, with today's wgpu code as the first backend and no pixel change. Phase C adds a native Metal backend on macOS that runs the same `DrawList`, compiles every shader (WGSL and plugin GLSL) through naga to MSL with the options wgpu-hal uses, and also paints the egui windows. Phase D cuts the effect copy pass on all backends. Phase E removes wgpu from macOS, then measures, documents and releases.

**Tech Stack:** Rust 2024; wgpu 30.0.1 (Windows, Linux; macOS until Task 14); naga 30.0.1 direct (`wgsl-in`, `glsl-in`, `msl-out`); objc2 0.6, objc2-metal 0.3, objc2-quartz-core 0.3 (macOS); egui 0.36.2; vendored glyphon, cosmic-text, winit. Tests: `scripts/check-unix` (Linux here, macOS on funtimes), `scripts/xt test --lib -- <filter>` and `scripts/win-check` (Windows on EXTREME).

**Spec:** `docs/superpowers/specs/2026-11-23-cheap-effects-metal-design.md` (approved 2026-11-23). Code map: `.superpowers/sdd/2026-11-23-cheap-effects-metal/render-map.md` (copy of the research map; file:line facts for every step below).

## Global Constraints

- Pixels: no change to any effect or to plain text. Proof on wgpu: today's path against the new path, byte for byte, with `src/render_pixel_tests.rs` and the region tests in `src/render.rs`. Proof on Metal: wgpu-Metal against native Metal in one macOS process, exact; a difference of at most 1 step per channel is allowed only where the only cause is float rounding in the MSL compile, and each such case is listed by name in the task report. Any larger difference is a bug.
- Timing: an effect keeps its speed, phase and look (`Clock`, `Signals`, `iTime` unchanged).
- No silent fallback. On macOS, a Metal start failure shows an error dialog with the reason and the app does not try another API. GPU loss remakes the Metal device; if that fails, the existing "restart Termurion" banner shows.
- During Phases B to D, macOS keeps the wgpu path behind the hidden switch `TERMURION_RENDERER=wgpu` (the existing `gpu_pick::FORCE_ENV`). Default on macOS is Metal from Task 9 on. Task 14 removes the switch and wgpu from the macOS build.
- "Effects animation" choices after this plan, in this order in the schema: `0 Full` (display rate, never pauses), `1 Balanced` (30 frames/s, pauses after 120 s), `2 "Light (older hardware)"` (15 frames/s, pauses after 30 s), `3 Smooth` (60 frames/s even on a 120 Hz screen, pauses after 120 s). Default `3`. Users CAN still lower the rate (owner's rule); a saved value keeps its meaning.
- The golden fixture test `scene::agent_box_tests::pane_frames_match_the_recorded_baseline` passes with `tests/fixtures` unchanged (`git status --porcelain tests/fixtures` prints nothing). Never `TERMURION_BLESS`.
- GPU tests use the shared test device (`render::test_gpu`, never a new `wgpu::Instance`; the Metal twin `metal_backend::test_device()` is one per process too). GLES 3.0 rule: no storage buffers in vertex shaders, no `first_instance`, uniform blocks at most 16384 bytes; the GL path without view formats keeps working.
- All 13 plugins in `resources/render_plugins`, user plugins (`.glsl` and `.wgsl`), Ghostty shaders and every FX profile in `resources/fx_profiles` keep working unchanged. No plugin manifest change.
- New dependencies: only `naga` (direct, same version wgpu uses, one copy in `Cargo.lock`), `objc2-metal` and `objc2-quartz-core` (macOS only, versions already in `Cargo.lock` through wgpu-hal where possible).
- Vendored crate changes go in their `TERMURION-PATCHES.md` as the next patch number, with `// Termurion patch N:` at each site.
- Short human comments, only where the why is not obvious. No em dashes and no emoji anywhere: code, comments, docs, commit messages. No host or user names in commits. No attribution or Co-Authored-By lines. Work on main. One commit per Commit step. Do not push, do not bump the version, do not edit `docs/FIXES.md` (the controller does all three, in Task 16).
- Known failures that are not regressions: Linux full suite, exactly the list in `.superpowers/sdd/2026-11-18-draw-speed/linux-baseline-failures.txt`; macOS adds `pane::tests::the_pwsh_script_*`; Windows has 3 (`handoff_win::tests::handles_are_typed_before_use`, `platform::windows::pty::tests::a_close_never_happens_while_attached`, `util_query::tests::it2ul_follows_the_script`). Run the full Windows suite with `--test-threads=1`; never two `scripts/xt` at once.
- Benchmarks: the controller runs them, on funtimes, with the bench procedure in the 2026-11-20 beat-ghostty plan's Global Constraints. A task states its expected result and does not run the bench. Leave no Termurion running on funtimes. Never open a GUI on the owner's Mac.

## Review Focus

1. **A cell changes on the same wake as an effect frame** (a key echoes, output arrives, the window resizes, a setting changes) while the fx-only skip is on: the frame must be rebuilt, never reused. Test: Task 2 `an_input_wake_is_never_fx_only`.
2. **The window moves to another screen or changes scale while Metal draws** (Retina to a 1x external display, 120 Hz to 60 Hz): the drawable size, scale and the effect's `iResolution` follow on the next frame, no stretched or stale frame. Test: Task 9 `a_new_scale_resizes_the_drawable`.
3. **Glyph atlas growth or eviction in the middle of a frame on Metal** (a CJK flood, an emoji burst): the same pixels as wgpu-Metal. Test: Task 10 `metal_matches_wgpu_through_atlas_growth`.
4. **A user plugin that fails to compile, or compiles to MSL that Metal rejects**: the same error text and line number the wgpu path shows, and the window keeps drawing without the effect. Test: Task 11 `a_bad_plugin_reports_the_same_error_on_metal`.
5. **A Settings or prompt window opens while no terminal window exists** (resident mode, `prefs_gpu`): it draws on the shared Metal device. Test: Task 12 `egui_opens_with_no_terminal_window`.

---

## File Structure

- `src/render_fx.rs`: `Pacing::Smooth`, cheaper `Uniforms::pack_into` (Tasks 1, 3).
- `schema/global.toml`, `docs/help/gpu-renderer.md`, `docs/render-plugins.md`: the new choice (Task 1).
- `src/app/mod.rs`, `src/app/fx_ui.rs`, `src/app/fonts_ui.rs`, `src/app/statusbar_ui.rs`: the fx-only wake and frame (Task 2, 3).
- Create `src/draw_list.rs`: `DrawList` and its parts, no GPU types (Task 5).
- `src/render.rs`: the CPU half builds a `DrawList`; create `src/render_wgpu.rs` for the wgpu backend moved out of `render.rs` (Tasks 5, 6).
- `crates/glyphon/src/*`: GPU-free core plus a `wgpu` backend module (patch 9, Task 6).
- Create `src/gpu_shared.rs`: `GpuShared` handle replacing the four wgpu objects (Task 7).
- Create `src/gpu_window.rs`: `WindowGpu` per-window backend enum (Task 8).
- Create `src/metal_backend/` (macOS only): `mod.rs` (device, pick, loss), `surface.rs` (CAMetalLayer), `shaders.rs` (naga to MSL), `quads.rs`, `images.rs`, `text.rs`, `chain.rs`, `egui_painter.rs`, `readback.rs`, `tests.rs` (Tasks 9 to 12).
- `src/egui_host.rs`: draws through `GpuShared` (Tasks 7, 12).
- `scripts/draw-bench`: the 13 `termurion-fx-<id>` configs (Task 4). Create `scripts/draw-bench-linux` and `scripts/draw-bench-win.ps1` (Task 15).
- Create `docs/PERFORMANCE.md`; modify `README.md`, `docs/FIXES.md`, `docs/ROADMAP.md`, help docs, installer (Task 16).

---

## Phase A: cheaper effect frames on every OS

### Task 1: The "Smooth" choice, 60 frames/s, is the default

**Files:** Modify `src/render_fx.rs:62-105` (`Pacing`), `schema/global.toml:3222-3230`, `docs/help/gpu-renderer.md:29-38`, `docs/render-plugins.md` (if it names rates). Test: `src/render_fx.rs` tests.

**Interfaces:** Produces `Pacing::Smooth` (the new `#[default]`), `Pacing::from_setting(3) == Smooth`, `frame_gap(Smooth) == Some(Duration::from_micros(16_667))`, `idle_pause(Smooth) == Some(Duration::from_secs(120))`.

- [ ] **Step 1: Write the failing tests.** `pacing_reads_the_setting` gains `from_setting(3) == Pacing::Smooth` and `Pacing::default() == Pacing::Smooth`; keep `from_setting(1) == Balanced`, `from_setting(0) == Full`, `from_setting(2) == Light`, and an unknown value (`9`) maps to `Smooth`. New `smooth_caps_at_sixty_on_a_fast_screen`: a `Clock` with `Pacing::Smooth`, frames asked every 8.333 ms (120 Hz vsync) for 1 s, draws 60 (plus or minus 1). New `smooth_pauses_when_idle`: no cursor or focus change for 120 s pauses it; a cursor change wakes it.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test pacing smooth_`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add the variant, the schema choice `{ value = 3, label = "Smooth" }` as the last choice, `default = 3`, and the help text: Smooth "up to 60 frames a second, even on a faster screen; pauses after 2 minutes with no change (the default)". Update `docs/help/gpu-renderer.md` the same way, with Smooth listed first and "(the default)" moved to it, and Full now "as fast as the display refreshes".
- [ ] **Step 4: Run** `scripts/check-unix --linux-only --test pacing smooth_ auto_animation a_skipped_frame schema`. Expected: PASS.
- [ ] **Step 5: Commit** `Effects animation: Smooth, 60 frames a second, is the default`.

### Task 2: A wake that only moves the effect skips the App bookkeeping

**Files:** Modify `src/app/mod.rs` (`about_to_wait` 8220-8493, `RedrawRequested` 8895, `render_frame` 7594-7733), `src/app/fx_ui.rs` (`fx_frame_tick`), `src/app/fonts_ui.rs:248` (`ensure_faces`), `src/app/statusbar_ui.rs:350` (`room`). Create `src/app/fx_wake.rs` (pure). Test: `src/app/fx_wake.rs`.

**Interfaces:** Produces `pub struct FxWake { only_fx: bool }` with `fn fx_fired(&mut self)`, `fn other(&mut self)` (any input, user event, pty output, resize, focus, setting change, timer other than the fx timer), `fn take(&mut self) -> bool` (true only when the fx timer was the only reason since the last take). Produces `App::fx_only: FxWake`. Consumes `globals.generation()` for the settings check.

- [ ] **Step 1: Profile.** Controller runs `scripts/draw-bench --sample --out target/draw-bench/t2-before termurion-fx-lightning` with `DRAW_BENCH_WL=idle`; the task report lists the main-thread inclusive samples of `about_to_wait`, `apply_fonts`, `ensure_faces`, `room`, `publish_snap`, `check_composer_fields`, `note_frame`, `FxRuntime::prepare`, `Uniforms::pack`, `frame_rects`, and the wgpu submit.
- [ ] **Step 2: Write the failing tests** in `fx_wake.rs`: `only_the_fx_timer_is_fx_only` (`fx_fired`, `take` is true); `an_input_wake_is_never_fx_only` (`fx_fired`, `other`, `take` is false; `other`, `fx_fired`, `take` is false); `take_resets` (second `take` with no calls is false). Source-scan test `every_wake_source_marks_other`: in `src/app/mod.rs`, the bodies of `window_event`, `user_event` and `new_events` each call `self.fx_only.other()` before any other statement, except the `RedrawRequested` arm.
- [ ] **Step 3: Run** `scripts/check-unix --linux-only --test fx_wake every_wake_source`. Expected: FAIL.
- [ ] **Step 4: Implement.** `fx_frame_tick` calls `fx_fired` when it wakes a window. In `about_to_wait`, when `take()` is true, run only `fx_frame_tick`, `gpu_recovery_tick`, the cursor blink tick and the wake-time computation, then set the control flow as today. On `RedrawRequested` with the frame reusable (`scene.would_reuse`), skip `apply_fonts`, `ensure_faces` and the `room()` compare unless `globals.generation()` or the window size moved since the last full frame. In `render_frame` on the reuse path, skip `check_composer_fields` and `publish_snap`; keep `note_frame`, `Signals::tick` and `pace_fx`.
- [ ] **Step 5: Run** `scripts/check-unix --test fx_wake every_wake_source app:: render_fx` and `scripts/xt test --lib -- fx_wake app::`. Expected: PASS apart from the baseline.
- [ ] **Step 6: Commit** `Effects: a wake that only moves the effect skips the App bookkeeping`.

### Task 3: Effect uniforms cost almost nothing per frame

**Files:** Modify `src/render_fx.rs` (`Uniforms::pack` 809, `frame_rects` 318), `src/app/fx_ui.rs` (`FxRuntime::prepare`), `src/render_fx_gpu.rs` (`skip_if_still` 418, `run_with` 558). Test: `src/render_fx.rs`, `src/render.rs` region tests.

**Interfaces:** Produces `Uniforms::pack_into(&self, layout: &Layout, out: &mut Vec<u8>)` (reuses `out`, no other allocation) and `Layout::clock_span() -> std::ops::Range<usize>` (the byte range holding iTime, iTimeDelta, iFrame, iChannelTime, the cursor and focus fields). `FxRuntime::prepare` recomputes the palette and colors only when the pane's colors moved (`colors_epoch`). `frame_rects` reuses its two `Vec`s and returns the cached result when its inputs are equal.

- [ ] **Step 1: Write the failing tests.** `pack_into_matches_pack` (every field of a filled `Uniforms`, every built-in plugin's layout, bytes equal to the old `pack`); `pack_into_allocates_nothing` (after one warm call, a counting allocator in the test sees 0 allocations for 100 calls); `the_palette_is_filled_once_per_color_change` (`prepare` 100 times with the same pane: the palette fill counter is 1; change one ANSI color: 2); `frame_rects_reuses_its_answer` (same inputs twice: the rect lists are equal and the compute counter moved by 1).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test pack_into the_palette_is frame_rects_reuses`. Expected: FAIL.
- [ ] **Step 3: Implement.** `run_with` writes only `clock_span()` bytes when the rest equals `last_packed`; the whole buffer otherwise.
- [ ] **Step 4: Run** `scripts/check-unix --test render_fx region_ unchanged_uniforms a_still_effect render_pixel` and the same with `TERMURION_TEST_BACKEND=gl`, and `scripts/xt test --lib -- render_fx region_`. Expected: PASS.
- [ ] **Step 5: Commit** `Effects: uniforms pack into one buffer and the palette fills once per color change`.

### Task 4: Phase A bench gate

**Files:** Modify `scripts/draw-bench` (add `termurion-fx-<id>` configs: copy the three patched lines from the controller's `draw-bench-fx`, in `.superpowers/sdd/2026-11-23-cheap-effects-metal/draw-bench-fx`, and `DRAW_BENCH_WL`).

- [ ] **Step 1: Implement** the script change; `bash -n scripts/draw-bench` passes.
- [ ] **Step 2: Commit** `draw-bench: one config per effect`.
- [ ] **Step 3: Controller bench.** `scripts/draw-bench --passes 2 --out target/draw-bench/phase-a ghostty-fx termurion-fx-lightning termurion-fx-cyber termurion-fx-glitch termurion`. Expected: Smooth (60 frames/s) effect idle CPU below 1.1.11's Balanced 8.8%, and plain termurion rows unchanged. Record the table and the per-frame ms in the ledger. This number tells how much Phase C must still take off.

## Phase B: one draw list, two backends (no pixel change)

### Task 5: The CPU builds a `DrawList`; wgpu runs it

**Files:** Create `src/draw_list.rs`; modify `src/render.rs` (`draw_with` 1794-2132); create `src/render_wgpu.rs` (the GPU half of `draw_with` moves here). Test: `src/render_pixel_tests.rs`, `src/draw_list.rs`.

**Interfaces:** Produces, in `draw_list.rs` (no wgpu, no Metal types):
- `pub struct DrawList { pub clear: [f32; 4], pub size: (u32, u32), pub quad_uniform: [f32; 4], pub backdrop: Vec<ImageDraw>, pub under: Range<u32>, pub picture: Vec<ImageDraw>, pub pops: Range<u32>, pub rows: RowDraws, pub glyphs: GlyphDraws, pub over: Range<u32>, pub fade: Option<f32> }` where the ranges index the persistent quad buffers.
- `pub enum BufferWrite { Quads { which: QuadBuf, offset: u64, bytes: Vec<u8> }, ... }` and `pub struct Uploads { pub buffers: Vec<BufferWrite>, pub atlas: Vec<AtlasOp> }`.
- `Renderer::prepare(&mut self, frame: &Frame, width: u32, height: u32) -> (DrawList, Uploads)` (the CPU top of today's `draw_with`, cut at `viewport.update`, see render map section 7).
- `render_wgpu::WgpuDraw::run(&mut self, device, queue, list: &DrawList, up: Uploads, view, raw)`; `Renderer::draw_with` keeps its signature and calls `prepare` then `run`.

- [ ] **Step 1: Write the failing tests.** `draw_list_has_no_gpu_types` (source scan: `src/draw_list.rs` names neither `wgpu` nor `objc2`); `prepare_is_pure_cpu` (a `Renderer` made without a device can `prepare` every `baseline_cases()` frame; needs `Renderer::cpu_only(face, px)`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test draw_list_has prepare_is_pure`. Expected: FAIL.
- [ ] **Step 3: Implement.** Split the CPU half and the GPU half of `Renderer` along the cut line. The font and metrics API (about 70 methods) stays on `Renderer`.
- [ ] **Step 4: Run** `scripts/check-unix --test render_pixel render:: draw_list` and with `TERMURION_TEST_BACKEND=gl`; `scripts/check-unix --mac-only --test render_pixel render::`; `scripts/xt test --lib -- render_pixel render::`. Expected: PASS, every pixel case byte-equal.
- [ ] **Step 5: Commit** `Render: the CPU builds a draw list and the wgpu backend runs it`.

### Task 6: glyphon gets a GPU-free core

**Files:** Modify `crates/glyphon/src/{text_atlas.rs, text_render.rs, row_render.rs, cache.rs, upload.rs}`; create `crates/glyphon/src/wgpu_backend.rs`; `crates/glyphon/TERMURION-PATCHES.md` (patch 9). Test: `cargo test -p glyphon --lib`, `src/render_pixel_tests.rs`.

**Interfaces:** Produces `pub enum AtlasOp { Grow { kind: ContentType, size: u32 }, Upload { kind: ContentType, x: u32, y: u32, w: u32, h: u32, bytes: Vec<u8> } }` (the atlas packer, LRU, mask filter and re-raster stay in the core and emit these), `RowRenderer::prepare_cpu(...) -> RowDraws` and `TextRenderer::prepare_cpu(...) -> GlyphDraws` (the `GlyphToRender` instance streams, the row table bytes, the batch list). The wgpu module applies them with today's code. `DrawList.rows` and `.glyphs` (Task 5) take these types.

- [ ] **Step 1: Write the failing tests** (glyphon): `core_names_no_wgpu` (source scan of every core file); `atlas_ops_rebuild_the_same_texels` (apply the recorded `AtlasOp`s to a CPU image, compare with the texture read back from the wgpu atlas, through growth and eviction).
- [ ] **Step 2: Run** `cargo test -p glyphon --lib core_names atlas_ops`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p glyphon --lib`, the Task 5 Step 4 set on all three OSes and GL. Expected: PASS, byte-equal.
- [ ] **Step 5: Commit** `glyphon: a GPU-free core and a wgpu backend (patch 9)`.

### Task 7: One handle for the shared GPU

**Files:** Create `src/gpu_shared.rs`; modify `src/egui_host.rs` (`open`, `open_dialog`, `open_with`), `App::prefs_gpu` (`src/app/mod.rs:2181`) and the about 96 opener sites (`grep -n '&g.instance' src`). Test: `src/gpu_shared.rs`.

**Interfaces:** Produces `#[derive(Clone)] pub enum GpuShared { Wgpu { instance: wgpu::Instance, adapter: wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue } }` (Task 9 adds the `Metal` arm, `#[cfg(target_os = "macos")]`), `Gpu::shared(&self) -> GpuShared`, and `EguiHost::open*(shared: &GpuShared, ...)`.

- [ ] **Step 1: Write the failing test** `no_opener_takes_four_wgpu_objects` (source scan: no `EguiHost::open` call in `src/` passes `&g.instance` or a four-tuple).
- [ ] **Step 2: Run** it. Expected: FAIL.
- [ ] **Step 3: Implement** (mechanical).
- [ ] **Step 4: Run** `scripts/check-unix --test egui_host app::`, `scripts/check-unix --mac-only --test egui_host`, `scripts/win-check`. Expected: PASS, `windows: 0 errors`.
- [ ] **Step 5: Commit** `GPU: one shared handle for egui windows`.

### Task 8: One per-window backend object

**Files:** Create `src/gpu_window.rs`; modify `src/app/mod.rs` (`Gpu` 431-460, the `draw` closure 7735-7840, `init_gpu` 8960, `init_gpu_on` 9061), `src/app/windows_ui.rs:1283`, `src/app/gpu_recovery_ui.rs:9`, `src/app/screenshot_ui.rs:45`, `src/app/fx_ui.rs` (`FxRuntime.chain`). Test: `src/gpu_window.rs`, existing app and region tests.

**Interfaces:** Produces `pub enum WindowGpu { Wgpu(WgpuWindow) }` (Task 9 adds `Metal(MetalWindow)`) with:
- `fn resize(&mut self, w: u32, h: u32, scale: f64)`
- `fn render(&mut self, list: &DrawList, up: Uploads, fx: Option<FxFrame<'_>>) -> Result<Drawn, RenderError>` (acquire, draw, effect chain, present; `Drawn` carries the bar's item and whether the frame was kept)
- `fn capture(&mut self, list: &DrawList, up: Uploads, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String>` (RGBA)
- `fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String>` (replaces `fx_sync`'s direct `Chain::new`)
- `fn skip_if_still(&mut self, u: &Uniforms, eligible: bool) -> bool`
- `fn rebuild(&mut self, shared: &GpuShared) -> Result<(), String>`
- `pub struct FxFrame<'a> { pub uniforms: &'a Uniforms, pub region: Option<&'a FrameRegion>, pub same_scene: bool, pub keep_output: bool }`
- `pub enum RenderError { Skip, Outdated, Lost(String) }`.

- [ ] **Step 1: Write the failing test** `app_names_no_surface` (source scan: `src/app/mod.rs` has no `get_current_texture`, `surface.configure` or `create_view`).
- [ ] **Step 2: Run** it. Expected: FAIL.
- [ ] **Step 3: Implement.** Move the closure body and the chain ownership behind `WindowGpu`; `Gpu.config` reads become `WindowGpu` getters.
- [ ] **Step 4: Run** the full suites: `scripts/check-unix` (Linux and macOS), `scripts/xt test --lib -- --test-threads=1`, `scripts/win-check`, and the GL run of `render_pixel region_`. Expected: PASS apart from the baseline.
- [ ] **Step 5: Controller check** on funtimes and EXTREME: the app starts, an effect runs, resize, a screenshot, Settings opens. `scripts/draw-bench --passes 1 termurion termurion-fx-lightning` within 3% of Task 4.
- [ ] **Step 6: Commit** `GPU: one backend object per window`.

## Phase C: native Metal on macOS

### Task 9: Metal device, layer and present; quads and images

**Files:** Create `src/metal_backend/{mod.rs, surface.rs, shaders.rs, quads.rs, images.rs, readback.rs, tests.rs}`; modify `Cargo.toml` (`naga`, `objc2-metal`, `objc2-quartz-core` under macOS), `src/gpu_shared.rs`, `src/gpu_window.rs`, `src/gpu_pick.rs` (make `report_lost` `pub(crate)`; Metal branch in `backend_order`), `src/app/mod.rs` (`init_gpu`: Metal unless `TERMURION_RENDERER=wgpu`; error dialog on failure). Test: `src/metal_backend/tests.rs` (macOS), `src/metal_backend/shaders.rs` (all OSes).

**Interfaces:**
- `metal_backend::shaders::msl_options() -> naga::back::msl::Options` and `pipeline_options()`: copied from wgpu-hal 30.0.1 `metal/device.rs` (`lang_version`, bounds-check policies, binding map per stage, `fake_missing_bindings`, `zero_initialize_workgroup_memory`), with a comment naming that file and version.
- `shaders::to_msl(module: &naga::Module, info: &naga::valid::ModuleInfo, stage: naga::ShaderStage, entry: &str) -> Result<String, String>`.
- `MetalShared { device: Retained<ProtocolObject<dyn MTLDevice>>, queue: Retained<ProtocolObject<dyn MTLCommandQueue>> }` and `GpuShared::Metal(MetalShared)`.
- `MetalWindow` implements every `WindowGpu` method. Layer: `CAMetalLayer` on winit's `NSView`, `pixelFormat = BGRA8Unorm_sRGB`, `framebufferOnly = false`, `maximumDrawableCount = 2` (wgpu's `desired_maximum_frame_latency: 2`), `displaySyncEnabled = true`, opaque, `contentsScale` = the window's scale.
- `metal_backend::test_device() -> &'static MetalShared`; `readback::read_rgba(texture, w, h) -> Vec<u8>`.

- [ ] **Step 1: Write the failing tests.** All OSes: `quad_and_image_wgsl_translate_to_msl` (`src/quad.wgsl`, `src/image.wgsl` parse and `to_msl` succeeds for `vs` and `fs`). macOS: `metal_matches_wgpu_on_quads_and_images` (every `baseline_cases()` frame with its text rows removed, drawn by `WgpuWindow::capture` on the wgpu-Metal test device and by `MetalWindow::capture`: equal bytes under the Global Constraints rule); `a_new_scale_resizes_the_drawable` (resize from scale 2.0 to 1.0: the drawable size and `iResolution` follow on the next frame); `a_lost_device_is_reported` (a forced loss reaches `gpu_pick::take_lost`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test quad_and_image_wgsl` and `scripts/check-unix --mac-only --test metal_`. Expected: FAIL.
- [ ] **Step 3: Implement.** The startup error dialog uses the app's existing macOS alert path (`NSAlert` through objc2-app-kit) with the text `Termurion could not start Metal: <reason>`, then exits with status 1.
- [ ] **Step 4: Run** the Step 2 commands. Expected: PASS.
- [ ] **Step 5: Commit** `Metal: device, layer, present, quads and images`.

### Task 10: Metal draws text

**Files:** Create `src/metal_backend/text.rs`; modify `src/metal_backend/mod.rs`. Test: `src/metal_backend/tests.rs`.

**Interfaces:** Consumes `AtlasOp`, `RowDraws`, `GlyphDraws` (Task 6). Uses glyphon's `shader.wgsl` and `row_shader.wgsl` through `to_msl`. Atlas textures: mask `R8Unorm`, color `RGBA8Unorm_sRGB` (or `RGBA8Unorm` per `ColorMode`), Nearest sampler, as in `crates/glyphon/src/cache.rs`.

- [ ] **Step 1: Write the failing tests** (macOS): `metal_matches_wgpu_on_every_pixel_case` (all of `pixel_cases()`: baseline frames, agent screens, 300 `cjk_flood` frames); `metal_matches_wgpu_through_atlas_growth` (the atlas growth and eviction run from `render_pixel_tests.rs`); `metal_matches_wgpu_on_scrolled_rows` (row instance reuse case).
- [ ] **Step 2: Run** `scripts/check-unix --mac-only --test metal_matches`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** it. Expected: PASS.
- [ ] **Step 5: Commit** `Metal: text and rows`.

### Task 11: Metal runs the effect chain

**Files:** Create `src/metal_backend/chain.rs`. Test: `src/metal_backend/tests.rs`.

**Interfaces:** Consumes `Compiled` (naga module from `Plugin::compile_with`), `Layout`, `FrameRegion`, `Shade` semantics of `Chain::run_with` (render map section 3). The scene texture is `BGRA8Unorm_sRGB` with `MTLTextureUsagePixelFormatView`; the effect reads a `BGRA8Unorm` view and writes the drawable as `BGRA8Unorm`. Sampler Linear, ClampToEdge. The kept-to-drawable copy is a blit encoder copy. Exposes the counters the region tests assert (`uniform_writes`, `kept_made`, `skips`, `shaded_region`, `frame_kept`).

- [ ] **Step 1: Write the failing tests** (macOS): `metal_effects_match_wgpu_for_every_plugin` (the `region_shots` timeline of every shipped plugin, run on both backends, every frame compared); `a_bad_plugin_reports_the_same_error_on_metal` (a GLSL file with an error on line 7 gives the same message text and line on both; the window still draws without the effect); `a_user_wgsl_plugin_runs_on_metal`. All OSes: `every_plugin_translates_to_msl` (each built-in through `compile_with` with every switch combination, then `to_msl`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test every_plugin_translates` and `scripts/check-unix --mac-only --test metal_effects a_bad_plugin a_user_wgsl`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** them. Expected: PASS.
- [ ] **Step 5: Commit** `Metal: the effect chain`.

### Task 12: Metal paints the egui windows; screenshots and GPU capture

**Files:** Create `src/metal_backend/egui_painter.rs` (egui-wgpu 0.36.2's `egui.wgsl` copied into `src/metal_backend/egui.wgsl`, translated by `to_msl`); modify `src/egui_host.rs`, `src/app/gpu_capture.rs` (`MTLCaptureManager` on Metal), `src/app/menu_bar_ui.rs` (stays wgpu-only; it is Linux-only). Test: `src/metal_backend/tests.rs`.

**Interfaces:** `EguiPainter::paint(&mut self, shared: &MetalShared, drawable, jobs: &[egui::ClippedPrimitive], deltas: &egui::TexturesDelta, screen: ScreenDescriptor)` with the same blend, non-sRGB target format and scissor rules as egui-wgpu.

- [ ] **Step 1: Write the failing tests** (macOS): `metal_egui_matches_wgpu` (the Settings window, a prompt and a popover tessellated once, painted by egui-wgpu on wgpu-Metal and by `EguiPainter`: equal bytes under the rule); `egui_opens_with_no_terminal_window` (`prefs_gpu` holds `GpuShared::Metal` with no terminal windows; an `EguiHost` paints a frame); `screenshot_matches_wgpu` (`render_lines` on both backends).
- [ ] **Step 2: Run** `scripts/check-unix --mac-only --test metal_egui egui_opens screenshot_matches`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** them. Expected: PASS.
- [ ] **Step 5: Controller check** on funtimes through `funtimes-ui`: open Termurion, an effect runs, Settings, a prompt, the screenshot panel, resize, full screen, a move to the second display if present. Look at each screen.
- [ ] **Step 6: Commit** `Metal: egui windows, screenshots and GPU capture`.

## Phase D: the copy pass

### Task 13: One effect pass writes the window and the kept texture

**Files:** Modify `src/render_fx_gpu.rs` (`run_with`, `Kept`, `blit`), `src/render_fx.rs` (`wrap_glsl` gains a second output only for the region path), `src/metal_backend/chain.rs`. Test: `src/render.rs` region tests, `src/metal_backend/tests.rs`.

**Design:** On the region path with view formats, today's two render passes (scissored effect into `kept`, then full-screen `kept` to window) become one full-screen pass with two color attachments: the window view and `kept`. A pass cannot read and write one texture, so `kept` becomes a ping-pong pair (`kept[0]`, `kept[1]`, swapped each frame): the pass reads the last frame's `kept` and writes the window and the other `kept`. A fragment inside a shade rect runs `mainImage` and writes its color to both; outside it copies the last `kept` texel to both. Rects go to the shader as a uniform array `iShadeRects[16]` plus `iShadeCount`; more than 16 rects, GL without view formats, or 3+ passes keep today's path. On Metal, compare this against the blit copy from Task 11 and keep the faster one, recorded in the ledger.

- [ ] **Step 1: Write the failing tests.** `one_pass_region_frames_match_two_pass` (every shipped plugin's `region_shots` timeline, both ways, byte-equal, on all 4 `ChainKind`s that support it); `seventeen_rects_take_the_old_path`; `the_region_path_runs_one_pass` (pass counter is 1 per animation frame).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test one_pass_region seventeen_rects the_region_path`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the region tests on Linux (lavapipe and GL), macOS (wgpu-Metal and Metal) and Windows (`scripts/xt test --lib -- region_`). Expected: PASS.
- [ ] **Step 5: Controller bench** on funtimes and EXTREME: keep the change only if the effect idle CPU drops; otherwise revert and record the numbers.
- [ ] **Step 6: Commit** `Effects: one pass writes the window and the kept texture`.

## Phase E: macOS without wgpu; measure, document, release

### Task 14: wgpu leaves the macOS build

**Files:** `Cargo.toml` (`wgpu`, `egui-wgpu` move to `[target.'cfg(not(target_os = "macos"))'.dependencies]`), `src/gpu_window.rs`, `src/gpu_shared.rs`, `src/gpu_pick.rs`, every `#[cfg]` the build needs; tests that compare against wgpu-Metal become tests that compare against stored references.

**Design:** Before removal, Task 14 Step 1 records reference images on funtimes from the native Metal path for every case the `metal_*` tests cover, as `tests/fixtures/metal/*.png.zst` with an FNV hash index; the `metal_*` tests then compare native Metal against these files.

- [ ] **Step 1: Record** the references (`TERMURION_RECORD_METAL=1 scripts/check-unix --mac-only --test metal_`), commit them.
- [ ] **Step 2: Write the failing test** `macos_build_has_no_wgpu` (macOS: `cargo tree -e normal --target aarch64-apple-darwin -i wgpu` reports no path; run as a script test in `scripts/check-unix --mac-only`).
- [ ] **Step 3: Implement.** `TERMURION_RENDERER=wgpu` on macOS is gone; an unknown value prints a warning to host.log and is ignored.
- [ ] **Step 4: Run** the full suites on all three OSes and `scripts/mac-bundle`. Expected: PASS apart from the baseline; the bundle builds.
- [ ] **Step 5: Commit** `macOS: Metal only, wgpu leaves the build`.

### Task 15: Benches on all three OSes

**Files:** Create `scripts/draw-bench-linux` (W541: X11 or Wayland session; configs termurion, termurion-fx-<id>, ghostty, ghostty-fx, alacritty, ptyxis; same workloads and columns as `scripts/draw-bench`), `scripts/draw-bench-win.ps1` (EXTREME: termurion, termurion-fx-<id>, Windows Terminal, WezTerm; CPU from process counters, fps from Termurion's own frame log and PresentMon where available, MB/s from the flood timing).

- [ ] **Step 1: Implement** both scripts with a `--dry-run` that prints the plan of runs; `bash -n` and `pwsh -NoProfile -Command "[ScriptBlock]::Create((Get-Content -Raw scripts/draw-bench-win.ps1))"` pass.
- [ ] **Step 2: Commit** `Benches for Linux and Windows`.
- [ ] **Step 3: Controller runs** all three benches (2 passes each, every effect, every comparison terminal), plus the 120 Hz check on the owner's Mac by reading `top` over SSH only (no GUI driving). Expected: every macOS effect row at or below Ghostty's effect row at 60 frames/s; Linux and Windows effect rows lower than 1.1.11 on the same machine. If a macOS row misses, the controller profiles it and adds a task before Task 16.

### Task 16: Documentation and the 1.1.12 release

**Files:** Create `docs/PERFORMANCE.md`; modify `README.md` (badge `renderer-Metal%20%7C%20wgpu`, feature list, a "Performance and accuracy" section that spotlights the speed, memory and exact-rendering work from 1.1.0 to 1.1.12 and links `docs/PERFORMANCE.md`, the mermaid diagram's renderer node), `docs/FIXES.md` (entry 37), `docs/ROADMAP.md`, `docs/help/gpu-renderer.md` (Metal on macOS; the GPU-lost section for macOS), version 1.1.11 to 1.1.12 in `Cargo.toml`, `Cargo.lock`, `README.md`, `docs/help/README.md`, `docs/help/installing.md`, `installer/termurion.iss`.

**Design of PERFORMANCE.md:** No hardware names anywhere (no CPU, GPU, model, VM or host name). One section per OS. Per section, one table per workload (idle, stream20, claude, flood) with rows per terminal, with and without an effect, columns CPU %, frames/s, MB/s, memory MB. A short "How we measure" section (workloads, passes, what each column means) and a "History" table of Termurion's own numbers from 1.1.0 to 1.1.12 where measured (from `docs/FIXES.md` and `target/draw-bench`).

- [ ] **Step 1: Write** the docs. `grep -nE 'M[0-9] |Apple M|Ryzen|Intel|NVIDIA|GeForce|Radeon|ThinkPad|W541|EXTREME|funtimes|Tart' docs/PERFORMANCE.md` prints nothing. The style guard passes (no em dashes, no emoji).
- [ ] **Step 2: Release** per the owner's rule: build all three (`scripts/mac-bundle`, the Windows installer and portable zip on EXTREME, `scripts/linux-appimage`), check each on its OS by looking at the screen, tag `v1.1.12`, push, GitHub release with notes and SHA-256 sums through `gh-datbird`, install on EXTREME, the W541 and the owner's Mac.
- [ ] **Step 3: Commit** `Version 1.1.12` (body: `FIXES 37 done in 1.1.12:` then the measured before and after effect CPU and the words `Metal on macOS`).
