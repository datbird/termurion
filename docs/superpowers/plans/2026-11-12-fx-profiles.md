# FX Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the cursor effects' screen-reading rules into per-app profile files that ship for six apps and that users can make, edit and share.

**Architecture:** A new `fx_profile` module holds the rule data (`FxRules`), parsing and limits. `scene.rs` keeps its algorithms and takes an `&FxRules` instead of constants. A new `fx_store` module owns the `FXProfiles` folder (shipped files, reload by polling, errors). A new `fx_detect` module picks a profile per multiplexer region; the app stores the pick on each `Pane`. The GUI is a new Settings page plus a pane menu item.

**Tech Stack:** Rust, `toml` (already a dependency), egui (Settings), the existing render plugin shader patcher.

**Spec:** `docs/superpowers/specs/2026-11-12-fx-profiles-design.md`

## Global Constraints

- Release version 1.1.0 (owner's request). Baseline for parity: main at `8afb3f8d` (1.0.5 plus the pointer and per-pane done fixes).
- No new crate dependencies. Folder changes are found by polling, at most once a second.
- Shell profile values equal today's constants: line glyphs `─` `━`; line fill 80%; bottom line at most 2 rows under the cursor; band rows 2 to 12, at most half the screen, at least half the columns, grow tolerance 2 cells, edge glyphs `▀` `▄`; painted cursor mark reverse video; Enter bytes `\r`; done quiet 1200 ms; busy 1500 ms; OSC 133;D ends the wait.
- Limits: at most 64 profiles; quiet time 100 ms to 60 000 ms; busy time 0 to 60 000 ms; at most 32 glyphs per glyph list; each `text` match 1 to 200 characters, at most 16 of them; band rows 1 to 50; fill share 0.1 to 1.0. Out of range is an error, never a clamp.
- Folder: `FXProfiles` beside the settings file, the same way `render_fx::user_dir` places `RenderPlugins`.
- No em dashes and no emoji in code comments, UI text, docs or commit messages. No Claude attribution in commits.
- Comments: short and human, the repo's existing density.
- Every task ends with `cargo test --lib` green except the known failures listed in `docs/port/unix-test-gaps.md` and the three known Linux normalization failures (`normalization_form_reaches_the_drawn_text`, `offscreen_band_normalizes`, `text_leaves_the_grid_normalized`).

## Review Focus

- A user's profile with `text = ["$"]` or another very common string: it must not take over every pane. Strong matches need the text inside the region, and ties go to shipped profiles first (Task 4 test `a_common_text_match_loses_to_a_program_match_and_ties_go_shipped`).
- A file saved mid-write by an editor (empty or half a file): the profile is skipped with an error and comes back on the next good save, never a crash (Task 3 test `a_half_written_file_is_an_error_until_it_is_whole`).
- Two Termurion windows or a restart while the folder changes: shipped-file bookkeeping must not resurrect a profile the user deleted (Task 3 test `a_deleted_shipped_profile_stays_deleted_after_restart`).
- A per-pane pick naming a profile whose file was then renamed by hand outside the GUI: the pane falls back to Auto and the menu shows the old name as missing (Task 6 test `a_pick_whose_profile_vanished_falls_back_to_auto`).
- A huge screen (500 columns, 200 rows) with 64 profiles: detection stays within budget (Task 4 test `detection_with_64_profiles_fits_the_budget`, under 2 ms in a release build, 20 ms in debug).

---

### Task 1: Rule data and parsing

**Files:**
- Create: `src/fx_profile.rs`, `resources/fx_profiles/{shell,claude-code,codex,opencode,hermes,agy}.toml`
- Modify: `src/lib.rs` (module list)

**Interfaces:**
- Produces:
  - `pub struct FxRules { pub name: String, pub matching: Match, pub cursor: CursorRule, pub boxes: BoxRule, pub prompt_marks: Vec<char>, pub done: DoneRule }`
  - `pub struct Match { pub program: Vec<String>, pub text: Vec<String>, pub prompt: Vec<char>, pub shape: Option<BoxKindRule> }`
  - `pub enum CursorKind { Real, Painted, Either }`; `pub enum PaintedMark { Reverse, Background(Rgb) }`; `pub struct CursorRule { pub kind: CursorKind, pub mark: PaintedMark, pub echo_anchor: bool }`
  - `pub enum BoxKindRule { Lines, Band, Either, None }`; `pub struct BoxRule { pub kind: BoxKindRule, pub line_glyphs: Vec<char>, pub line_fill: f32, pub bottom_within: usize, pub band_rows: (usize, usize), pub band_max_screen: f32, pub band_min_width: f32, pub band_tolerance: usize, pub edge_glyphs: Vec<char> }`
  - `pub struct DoneRule { pub enter: Vec<u8>, pub quiet: Duration, pub busy: Duration, pub osc133: bool }`
  - `impl Default for FxRules` = the Shell values in Global Constraints (name "Shell", cursor Either, box Either).
  - `pub fn parse(text: &str) -> Result<(FxRules, Vec<String>), ParseError>`: missing keys from `FxRules::default()`; the `Vec<String>` is warnings (unknown keys). `pub struct ParseError { pub line: Option<usize>, pub message: String }`.
  - `pub fn to_toml(rules: &FxRules, keep: &toml::Table) -> String`: writes every key; unknown keys from `keep` are written back.
  - `pub const SHIPPED: &[(&str, &str)]`: (file name, `include_str!` text) for the six files.
  - `pub fn union(rules: &[&FxRules]) -> FxRules`: Shell's numbers with the union of every line glyph list and painted marks (used to find the cursor before a profile is chosen).

Key names in the files (snake_case TOML): `name`; `[match] program text prompt shape`; `[cursor] kind mark echo_anchor` (`mark = "reverse"` or `"#rrggbb"`); `[box] kind line_glyphs line_fill bottom_within band_rows band_max_screen band_min_width band_tolerance edge_glyphs`; `[prompt_mark] glyphs`; `[done] enter quiet_ms busy_ms osc133`. Glyph lists are strings of characters (`line_glyphs = "─━"`).

Shipped profiles (all other keys default): Claude Code: `text = ["Claude Code"]`, `prompt = "❯"`, cursor Either, box Lines, prompt marks `❯`. Codex: `text = ["OpenAI Codex"]`, `prompt = "›"`, box Band, marks `›`. opencode: `text = ["opencode"]`, box Band, no marks. Hermes: `text = ["Hermes Agent", "Hermes"]`, `prompt = "❯"`, box Lines, marks `❯`. agy: `text = ["Antigravity"]`, `prompt = ">"`, box Lines, marks `>`. Shell: `name = "Shell"`, no match keys, marks `❯>$%#›`. The builder checks every `text` against `tests/fixtures/agents/*.ansi` and adjusts it to text those screens really show.

- [ ] **Step 1: Write the failing tests** in `src/fx_profile.rs`:

```rust
#[test] fn an_empty_file_is_the_shell_rules() { let (r, w) = parse("").unwrap(); assert_eq!(r.done.quiet, Duration::from_millis(1200)); assert_eq!(r.boxes.line_glyphs, vec!['\u{2500}', '\u{2501}']); assert!(w.is_empty()); }
#[test] fn every_key_reads() { /* one file setting every key to a non-default value; assert each field */ }
#[test] fn out_of_range_is_an_error_with_its_line() { let e = parse("[done]\nquiet_ms = 50\n").unwrap_err(); assert_eq!(e.line, Some(2)); assert!(e.message.contains("quiet_ms")); }
#[test] fn unknown_keys_are_warnings_and_kept_on_save() { let t = "name = \"X\"\nfuture = 3\n"; let (r, w) = parse(t).unwrap(); assert_eq!(w, vec!["unknown key `future`".to_string()]); let keep: toml::Table = t.parse().unwrap(); assert!(to_toml(&r, &keep).contains("future = 3")); }
#[test] fn a_painted_mark_is_reverse_or_a_color() { /* "reverse" -> Reverse, "#2c2c2c" -> Background(rgb(44,44,44)), "blue" -> error */ }
#[test] fn every_shipped_profile_parses_without_warnings() { for (f, t) in SHIPPED { let (_, w) = parse(t).unwrap_or_else(|e| panic!("{f}: {}", e.message)); assert!(w.is_empty(), "{f}"); } }
#[test] fn round_trip_keeps_every_value() { for (_, t) in SHIPPED { let (r, _) = parse(t).unwrap(); assert_eq!(parse(&to_toml(&r, &toml::Table::new())).unwrap().0, r); } }
#[test] fn union_takes_every_line_glyph() { /* a rules set with line_glyphs "═" unioned with Shell holds ─ ━ ═ */ }
```

- [ ] **Step 2: Run** `cargo test --lib fx_profile` . Expected: FAIL, module missing.
- [ ] **Step 3: Implement** the types and functions above. Parse with `toml::from_str::<toml::Table>`, then read each key by hand so line numbers come from `toml`'s span on error and unknown keys are listed. `FxRules` derives `Clone, Debug, PartialEq`.
- [ ] **Step 4: Run** `cargo test --lib fx_profile`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: rule data, parsing and the six shipped files"`.

### Task 2: scene.rs reads its rules from FxRules (parity)

**Files:**
- Modify: `src/scene.rs` (`box_line`, `band_box`, `painted_cursor`, `output_watch`, the `input_box` closure in `pane_with`, `DrawPrefs`)
- Create: `tests/fixtures/tmux-agents/` (copy of the 35 `.raw` and `.panes` files from the session scratchpad `agents/` folder), `tests/fixtures/tmux-agents/expected.txt`

**Interfaces:**
- Consumes: `FxRules`, `fx_profile::union` (Task 1).
- Produces:
  - `fn box_line(term, display_offset, row, pane, rules: &BoxRule) -> bool`
  - `fn band_box(term, palette, display_offset, row, col, pane, rules: &BoxRule) -> Option<(usize, usize, usize, usize, bool)>`
  - `pub fn painted_cursor(screen: &Screen, rules: &FxRules) -> Option<(usize, usize, bool)>`
  - `pub fn output_watch(screen: &Screen, rules: &FxRules) -> Option<(usize, usize, usize, usize)>`
  - `pub fn effects_cursor(screen: &Screen, rules: &FxRules) -> Option<(usize, usize, bool)>` = `cursor_cell` or `painted_cursor`, honoring `rules.cursor.kind`.
  - `DrawPrefs.fx_rules: Option<std::sync::Arc<FxRules>>` (the region's chosen rules) and `DrawPrefs.fx_find: Option<std::sync::Arc<FxRules>>` (the union, for finding the cursor). `None` means `FxRules::default()`.

What stays fixed in code (spec "What stays fixed algorithm"): junction, corner and title rejection in `box_line`; the default-background and vim strip rules and the half-block logic in `band_box`; `pane_cols`, `border_run`, `pane_walled`; the status row and pane border rules in `output_watch` (its border glyphs stay `─` `━` plus junctions, not the profile's). `output_watch`'s input-box line check uses `rules.boxes`.

- [ ] **Step 1: Copy the fixtures and write the golden test first, against the unchanged code**: test `tmux_agent_layouts_match_the_recorded_baseline` feeds each `.raw` into a `Screen` sized from its `.panes` (columns = max left+width, rows = max top+height + 1) and writes one line per file: name, `output_watch`, `effects_cursor`, and `fx_of(...).input_box` rounded to whole pixels. With `TERMURION_BLESS=1` it writes `expected.txt`; otherwise it compares. Run once with bless at `8afb3f8d`'s code (before any change in this task), commit the expected file.
- [ ] **Step 2: Run** `cargo test --lib tmux_agent_layouts_match_the_recorded_baseline`. Expected: PASS (it was just blessed).
- [ ] **Step 3: Implement** the signature changes. Each constant listed in Global Constraints is read from the rules at its current use. `painted_cursor` accepts a cell whose mark matches `rules.cursor.mark` (`Background(c)` compares `palette.color(cell.bg) == c`). `CursorKind::Real` never calls `painted_cursor`; `Painted` ignores a shown cursor only when `cursor_cell` is None (a shown cursor still wins, as today). `BoxKindRule::Lines` skips `band_box`, `Band` skips line boxes, `None` gives no box, `Either` keeps today's order. Every existing caller passes `&FxRules::default()` for now (`broadcast_ui.rs` included).
- [ ] **Step 4: Run** `cargo test --lib` . Expected: the golden test and every existing scene and render test pass unchanged.
- [ ] **Step 5: Add** test `a_custom_profile_reads_a_box_shell_rules_do_not`: a screen with `═` rows above and below a `λ ` prompt and the cursor between; with `FxRules::default()` `fx_of(...).input_box` is None; with rules whose `line_glyphs = "═"` it is a Lines box at those rows. Run it. Expected: PASS.
- [ ] **Step 6: Commit** `git commit -m "Effects: screen rules come from FxRules; tmux agent layouts pinned"`.

### Task 3: The FXProfiles folder

**Files:**
- Create: `src/fx_store.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `fx_profile::{parse, SHIPPED, FxRules}`.
- Produces:
  - `pub fn user_dir() -> PathBuf` (`FXProfiles` beside the settings file).
  - `pub struct Loaded { pub rules: Arc<FxRules>, pub file: PathBuf, pub shipped: Option<&'static str>, pub changed: bool, pub newer_shipped: bool, pub warnings: Vec<String>, pub raw: toml::Table }`
  - `pub struct Store { pub profiles: Vec<Loaded>, pub errors: Vec<(PathBuf, ParseError)>, /* private: dir, last scan stamp */ }`
  - `impl Store { pub fn open(dir: &Path) -> Store; pub fn refresh(&mut self, now: Instant) -> bool /* true when anything changed; scans at most once per second */; pub fn shell(&self) -> Arc<FxRules>; pub fn by_name(&self, name: &str) -> Option<Arc<FxRules>>; pub fn find_rules(&self) -> Arc<FxRules> /* union */; pub fn restore_shipped(&mut self) -> io::Result<()>; pub fn reset(&mut self, name: &str) -> io::Result<()>; pub fn save(&mut self, rules: &FxRules, keep: &toml::Table, file: Option<&Path>) -> io::Result<PathBuf>; pub fn delete(&mut self, name: &str) -> io::Result<()>; pub fn import(&mut self, from: &Path, replace: bool) -> Result<String, ImportError>; pub fn export(&self, name: &str, to: &Path) -> io::Result<()> }`
  - `ImportError::{Exists(String), Invalid(ParseError), Io(io::Error)}`.

Shipped bookkeeping: a file `FXProfiles/.shipped.toml` records, per shipped file name, the hash (FNV-1a 64 over the bytes, as hex) of the copy Termurion last wrote and a `deleted` flag. On open: a shipped file that is absent and not marked deleted is written (first start); a present file whose hash equals the recorded one and whose embedded text differs is overwritten (update); a present file whose hash differs is the user's (`changed`, and `newer_shipped` when the embedded text differs from the recorded hash's source). A shipped file that was recorded and is now absent is marked `deleted` and never written again until `restore_shipped`. Load order: file name order; duplicate `name`: the later file is an error "duplicate name". More than 64 profiles: the rest are errors. Shell: if no loaded profile is named "Shell", the built-in default is used.

- [ ] **Step 1: Write the failing tests** (each in a temp dir):

```rust
#[test] fn first_open_writes_the_six_shipped_files() { /* 6 .toml + .shipped.toml exist; profiles.len() == 6 */ }
#[test] fn an_unchanged_shipped_file_is_updated() { /* record hash of old text, embedded text differs -> file rewritten */ }
#[test] fn a_changed_shipped_file_is_kept_and_flagged() { /* edit codex.toml; reopen; text kept; changed && newer_shipped when embedded differs */ }
#[test] fn a_deleted_shipped_profile_stays_deleted_after_restart() { /* delete hermes.toml; open twice; still absent; restore_shipped brings it back */ }
#[test] fn a_broken_file_is_an_error_and_others_load() { /* "name = [" -> errors has it with a line; the other 6 load */ }
#[test] fn a_half_written_file_is_an_error_until_it_is_whole() { /* write "" then "[done]\nquiet" -> error; then a valid file -> loads after refresh */ }
#[test] fn duplicate_names_keep_the_first_file() { /* a.toml and b.toml both name="X": a loads, b is an error mentioning duplicate */ }
#[test] fn the_65th_profile_is_an_error() {}
#[test] fn shell_is_there_even_without_its_file() { /* delete shell.toml: shell() == FxRules::default() */ }
#[test] fn refresh_sees_a_new_file_within_a_second() { /* refresh(t0); copy a file; refresh(t0 + 1.1 s) -> true and the profile loads; refresh(t0 + 0.5 s) does not scan */ }
#[test] fn import_refuses_an_existing_name_unless_replace() {}
#[test] fn export_writes_the_profile_file() {}
```

- [ ] **Step 2: Run** `cargo test --lib fx_store`. Expected: FAIL, module missing.
- [ ] **Step 3: Implement** `Store` as above. The scan compares each file's (name, length, mtime) list with the last scan's and re-parses only on a difference.
- [ ] **Step 4: Run** `cargo test --lib fx_store`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: the FXProfiles folder, shipped files and reload"`.

### Task 4: Detection per region

**Files:**
- Create: `src/fx_detect.rs`
- Modify: `src/pane.rs` (fields), `src/app/mod.rs` (after the output pump and on focus change), `src/scene.rs` (`DrawPrefs` filled from the pane), `src/lib.rs`

**Interfaces:**
- Consumes: `Store` (Task 3), `scene::{effects_cursor, output_watch}` (Task 2), `proctree::Job`.
- Produces:
  - `pub type Region = (usize, usize, usize, usize);`
  - `pub fn score(rules: &FxRules, screen: &Screen, region: Region, job: Option<&str>) -> (u8, u32)`: (strength 0 none, 1 shape, 2 prompt, 3 text, 4 program; and the hit count). `program` compares with `proctree::same_job` and is skipped when `job` is None. `text` searches only the region's visible rows. `prompt` looks left of the effects' cursor on its row, nearest non-space cell. `shape` asks Task 2's box finder with the profile's own box kind.
  - `#[derive(Default)] pub struct Memory { regions: Vec<(Region, String)> }` with `pub fn choose(&mut self, store: &Store, screen: &Screen, job: Option<&str>) -> (Region, Arc<FxRules>)`: finds the cursor with `store.find_rules()`, the region with `output_watch`, scores every profile (shipped first, then user profiles in name order; the highest strength, then the most hits, then list order), remembers a strength 3 or 4 winner for the region, and returns the remembered name while the region shows nothing stronger. Shell when nothing scores above 0.
  - `pub fn forget_cleared(&mut self, screen: &Screen)`: drops all memory when `Screen` reports an alternate screen exit, ED 2, ED 3 or RIS since the last call (add a counter `Screen.clears: u64` bumped by those, read and compared).
  - `Pane.fx_memory: fx_detect::Memory`, `Pane.fx_rules: Arc<FxRules>`, `Pane.fx_pick: Option<String>` (Task 6 fills the pick; here it is always None).
- The app runs `choose` for a pane when it got output or a key, at most 4 times a second per pane, and stores `fx_rules` (or the picked profile's rules when `fx_pick` names one that exists). The renderer passes `fx_rules` and `store.find_rules()` in `DrawPrefs`.

- [ ] **Step 1: Write the failing tests** in `src/fx_detect.rs` (screens from `tests/fixtures/agents/` and `tests/fixtures/tmux-agents/`, a `Store` over a temp dir with the shipped files):

```rust
#[test] fn each_recorded_agent_screen_picks_its_own_profile() { /* codex.ansi -> "Codex", opencode.ansi -> "opencode", hermes.ansi -> "Hermes", agy.ansi -> "agy" */ }
#[test] fn a_claude_code_box_picks_claude_code() { /* scene's stacked() screen -> "Claude Code" */ }
#[test] fn a_plain_shell_picks_shell() {}
#[test] fn two_tmux_panes_pick_per_region() { /* mx-codex.raw: Codex region picks Codex; same screen with the cursor moved into the Claude pane (a reverse cell) picks Claude Code */ }
#[test] fn a_banner_that_scrolled_away_keeps_its_profile() { /* choose on codex.ansi, then feed 40 lines of output; still "Codex" */ }
#[test] fn a_clear_forgets() { /* after \x1b[2J and forget_cleared: plain screen -> "Shell" */ }
#[test] fn a_common_text_match_loses_to_a_program_match_and_ties_go_shipped() { /* user profile text=["$"], a shell prompt with job "codex" -> Codex; without job, a tie between a user and a shipped profile at the same strength goes to the shipped one */ }
#[test] fn detection_with_64_profiles_fits_the_budget() { /* 500x200 screen, 64 profiles each with 16 text matches: one choose() under 20 ms in debug, 2 ms with cfg(not(debug_assertions)) */ }
```

- [ ] **Step 2: Run** `cargo test --lib fx_detect`. Expected: FAIL.
- [ ] **Step 3: Implement** `score`, `Memory`, the `Pane` fields, the app hook and the `DrawPrefs` fill.
- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS, and Task 2's golden test unchanged.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: detection per multiplexer region"`.

### Task 5: Done rules and the prompt mark

**Files:**
- Modify: `src/render_fx.rs` (`Signals`, `patch_input_box`), `src/render_fx_prefix.glsl` (`iPromptMark`), the Rust uniform struct and its fill in `src/render.rs`, `src/app/broadcast_ui.rs`, `src/app/requests_ui.rs`, `src/scene.rs` (`FxPane.prompt_mark`)

**Interfaces:**
- Consumes: `FxRules.done`, `FxRules.prompt_marks`, `Pane.fx_rules` (Task 4).
- Produces:
  - `Signals::enter(&mut self, now: Instant, done: &DoneRule)` stores the quiet and busy times for this wait; `tick` and `command_ended` use the stored values; `command_ended` does nothing when the stored `osc133` is false.
  - `user_write` starts a wait when `bytes == p.fx_rules.done.enter` (today `b"\r"`).
  - `FxPane.prompt_mark: Option<[f32; 4]>`: the mark cell (left x, top y, width, height in window pixels) of the nearest glyph from `prompt_marks` left of the effects' cursor on its row, within the region's columns, skipping spaces; for a wrapped prompt (cursor below the box's first text row) the first box column of the row under the top line.
  - Uniform `vec4 iPromptMark` appended after `iInputBoxX` in `Globals`: x, y (top, in the same y convention as `iInputBox`), z = 1 when set else 0, w = cell width.
  - `patch_input_box` adds at the start of `clMarkLeft`: when `iPromptMark.z > 0.5` and the mark's row is the cursor's row, return `iPromptMark.x`, and -1 when its row differs; at the start of `clChevron`: when set, return `abs(x - iPromptMark.x) < 0.5 * w`; at the start of `clMarkAbove`: when set and its row is above the cursor's, return the mark's top-left corner in the function's own coordinates. Unset leaves the pixel checks as they are.

- [ ] **Step 1: Write the failing tests**:
  - `render_fx`: `a_wait_keeps_its_own_done_times` (enter with quiet 3000 ms; output at 0 and 2000 ms; tick at 4100 ms does not fire; at 5100 ms fires); `osc133_off_ignores_command_ended`.
  - `scene`: `the_prompt_mark_is_found_from_the_profile` (`λ ` prompt with rules marks `λ` gives `Some` at column 0; Shell rules give None for `λ`); `codex_mark_is_its_chevron` (codex.ansi with the Codex profile: the `›` cell).
  - `render_fx`: `every_builtin_reads_the_prompt_mark_uniform` (every patched shipped shader contains `iPromptMark` inside `clMarkLeft` and `clChevron`, and still compiles with `compile_glsl`).
  - `render.rs` (GPU, next to `every_effect_lights_both_box_lines_on_enter`): `a_profile_mark_changes_its_look_in_every_effect` with a `λ` mark and a custom profile.
- [ ] **Step 2: Run** those tests. Expected: FAIL.
- [ ] **Step 3: Implement** the interfaces above. The std140 offset of `iPromptMark` follows the rules in the prefix file's comments; `the_uniform_block_has_no_scalar_array` and `layout_is_std140_as_ghostty_declares` must still pass.
- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS, including every existing render test.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: done timing and the prompt mark come from the profile"`.

### Task 6: Pick a profile per pane

**Files:**
- Modify: `src/app/ctx_menu_app.rs`, `src/ctx_menu.rs` (or the module that builds its rows), `src/arrangement.rs` (a restoration-only key), the code that writes and reads `STABLE_ID_KEY` (the same path), `src/app/mod.rs` (Store held by the app, refreshed in the tick)

**Interfaces:**
- Consumes: `Store` (Task 3), `Pane.fx_pick` (Task 4).
- Produces:
  - `pub const FX_PROFILE_KEY: &str = "Termurion FX Profile";` written by restoration snapshots only, read at launch restore. Absent means Auto.
  - Context menu item "FX Profile" with a submenu: "Auto" (checked when `fx_pick` is None), then every loaded profile name (checked when picked); a picked name that no longer loads shows as "<name> (missing)" checked, and the pane uses Auto.
  - `App::rename_fx_profile(&mut self, old: &str, new: &str)` moves every pane's pick.

- [ ] **Step 1: Write the failing tests**: `the_menu_lists_auto_and_every_profile`; `picking_a_profile_overrides_detection` (codex screen, pick "Shell": `fx_rules.name == "Shell"`); `a_pick_survives_restore` (snapshot table holds `FX_PROFILE_KEY = "Codex"`, restore sets `fx_pick`); `a_pick_whose_profile_vanished_falls_back_to_auto`; `renaming_a_profile_moves_the_picks`; `an_arrangement_never_carries_the_pick` (saving a named arrangement writes no `FX_PROFILE_KEY`).
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement** as above. The app owns one `Store`, opened at start, refreshed from the tick; a `true` from `refresh` re-runs detection for every pane.
- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: pick a profile per pane"`.

### Task 7: The FX Profiles settings page

**Files:**
- Create: `src/settings_fx_profiles.rs`
- Modify: `src/settings_ui.rs` (`Page::FxProfiles`, label "FX Profiles", placed after "Pointer" in the page bar), `src/lib.rs`

**Interfaces:**
- Consumes: `Store` (Task 3), `fx_detect::score` and `scene` finders (Tasks 2, 4) for the preview, `App::rename_fx_profile` (Task 6).
- Produces: `pub fn page(ui: &mut egui::Ui, store: &mut Store, state: &mut PageState, preview: Option<&Screen>) -> Vec<PageAction>`; `pub enum PageAction { Rename { old: String, new: String }, ShowFolder, Import, Export(String) }` (file dialogs and the folder open run in the app, through the existing dialog helpers).

Layout: a list on the left (name, then "shipped", "changed", "newer version available" or "error" marks); buttons New, Duplicate, Delete, Import, Export, Reset (enabled for shipped profiles only), Restore Shipped Profiles, Show Folder; the error list for files that failed (file, line, message). On the right, the editor form with sections Match, Cursor, Input Box, Prompt Mark, Done. Every field has hover text with its meaning and its Shell default. Save writes through `Store::save` and keeps unknown keys. Delete and an Import over an existing name ask first (the existing confirm dialog helper). New starts from the Shell values with a unique name ("New Profile", "New Profile 2", ...); Duplicate copies with the name "<name> copy".

Preview: under the form, a text summary for the focused pane computed from the form's unsaved values: "Match: text `OpenAI Codex` found" or "no match", "Cursor: row 27, column 4 (real)", "Input box: band, rows 25 to 27", "Prompt mark: `›` at row 26, column 2", or "not found" for each. The values come from calling the Task 2 and Task 4 functions with the unsaved `FxRules`, never from the loaded store.

- [ ] **Step 1: Write the failing tests** (pure functions under the page, no egui frame): `new_profile_names_are_unique`; `duplicate_copies_every_rule`; `reset_is_only_for_shipped`; `the_preview_reports_each_rule` (codex.ansi with the Codex rules gives the four lines above with real numbers; with box kind Lines it says "Input box: not found"); `saving_keeps_unknown_keys`.
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement** the page and the app wiring for its actions.
- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -m "FX profiles: the settings page"`.

### Task 8: Docs and version

**Files:**
- Create: `docs/help/fx-profiles.md`
- Modify: `docs/help/README.md` (link), `README.md` (one line in the render plugins section, version), `docs/ROADMAP.md` (release row), `docs/help/installing.md`, `Cargo.toml`, `Cargo.lock` (termurion entry only), `installer/termurion.iss` (version 1.1.0, the same 7 places 1.0.5 changed in `e452bb88`)

The help page covers: what a profile is (how the effects read an app, not which effects run); the folder per OS; the six shipped profiles; every key with its default and limit (from Global Constraints); Auto detection and the per-pane pick; the Shell fallback; a worked custom profile for a `═` box with a `λ` prompt; the preview; shipped-file updates, Reset and Restore.

- [ ] **Step 1: Write the doc and bump the version.** `grep -rn "1\.0\.5" README.md docs/help docs/ROADMAP.md installer Cargo.toml` shows no stale release number afterwards (history rows excepted).
- [ ] **Step 2: Run** `cargo test --lib` and `scripts/win-check`. Expected: PASS, 0 errors.
- [ ] **Step 3: Commit** `git commit -m "Version 1.1.0: FX profiles"`.

### Task 9: Whole-branch review, real-app checks, release

- [ ] **Step 1:** Opus whole-branch review of `8afb3f8d..HEAD` against the spec; fix every Important finding; re-review.
- [ ] **Step 2:** `scripts/check-unix --test` and `scripts/win-check`: only the known failures, 0 Windows errors.
- [ ] **Step 3:** funtimes GUI (`scripts/mac-dev-app`): replay each `tests/fixtures/agents/*.ansi` screen, confirm the effects ring the box; open FX Profiles, make a custom `λ` profile, confirm the preview and the effect; pick a profile from the pane menu; quit and relaunch, the pick holds. Look at every screenshot.
- [ ] **Step 4:** Linux AppImage on Xvfb: the stacked fake Claude panes check from 1.0.5, plus one custom profile copied into the folder by hand while Termurion runs (it loads within a second).
- [ ] **Step 5:** Release 1.1.0 the way 1.0.5 was released (Windows installer and portable, macOS zip, Linux AppImage, hashes checked after download), install on EXTREME, the work Mac and the W541, clean up.
