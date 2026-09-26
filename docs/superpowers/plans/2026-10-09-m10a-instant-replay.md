# Tandem Milestone 10a: Instant Replay

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope (read first):** research `.superpowers/research/m10-facts.md` sections 0 to 4, 10 (the `IRMemory` row), 11 (items 1 to 7, 10, 11) and 12 (the first three bullets), and nothing else: the per-session recorder (iTerm2's DVR: the circular byte budget, key and diff frames, the key-frame rule with its counter quirk, eviction of leading diff frames, decoders and their invalidation), recording one frame per refresh from the grid's damage, the `IRMemory` setting (General > Instant Replay, 4 MB, 0 to 1000, read when a session starts), the replay view (a read-only screen from the frame swapped in through `Pane::show_view`, the live screen keeps recording), entering and stepping (View > Start Instant Replay Win+Alt+B, key action 15 "Start Instant Replay", Left and Right with Shift for 15, Esc, other keys beep, keys never reach the session), what closes Instant Replay (another pane made active, split selection, closing the pane, the window closing) and what it refuses (splitting), the "Instant Replay Control" panel (slider, time labels, "Live View", "Now", tick marks, the Export\u{2026} / Set Start / Set End / Cancel range export, placement, alpha), Edit > Clear Instant Replay (Win+Alt+K), and Session > Log > Export Recording and Import Recording (`.itr`). The password manager is milestone 10b (`2026-10-09-m10b-password-manager.md`). Browser Instant Replay (`webInstantReplayFrameRate`) stays n/a: Tandem has no browser sessions.

**Goal:** every session keeps a rolling, memory-capped video of its screen, and the user can rewind it frame by frame or with a scrubber, export all or part of it to a file, load such a file back for playback, and clear it, with iTerm2's frame rules, keys, texts, sizes and quirks.

**Architecture:** pure models first, each a plain `pub mod` tested on Linux. `dvr.rs` is iTerm2's DVR: an exact emulation of `DVRBuffer`'s byte ring (positions and lengths only, so eviction happens at the same moments), frames that hold alacritty `Row<Cell>` lines (key frames all rows, diff frames only dirty rows), the encoder's rules and the decoder. `Screen::replay_capture` turns alacritty's own damage tracking (`Term::damage` / `reset_damage`, unused by Tandem so far) into a frame and its clean lines; no alacritty patch. `replay.rs` is the replay session (enter, step, seek, keys, the frame screen). `replay_panel.rs` is the panel's model (labels, slider, ticks, export states, placement). `recording.rs` is the `.itr` file. The app side: `Pane.dvr` and `Pane.replay` (per-pane state on the Pane), `src/app/replay_ui.rs` (commands, keys, recording tick, import and export), and `src/replay_window.rs`, an egui host window for the panel; the panel handle is a per-window App field and goes in `swap_live`.

**Tech Stack:** as milestone 9l. No new crates: `flate2` gzips, `serde_json` with alacritty's default `serde` feature writes rows, `rfd` shows the save and open dialogs, `GetDateFormatEx` / `GetTimeFormatEx` (`Win32_Globalization`, already enabled) format times, `SetWindowLongPtrW` with `WS_EX_NOACTIVATE` keeps the panel from taking activation.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m10-facts.md` sections 1 to 4 and the Resume notes below (the source wins where they differ). Rows: `03-features-and-menus.md` Edit > Clear Instant Replay, View > Start Instant Replay, Session > Log > Import Recording and Export Recording, the six Instant Replay rows; `02-global-settings.md` Instant Replay memory usage limit, Start Instant Replay (KEY_ACTION_IR_BACKWARD), webInstantReplayFrameRate; rows found by title.

## Global Constraints

- Everything from milestones 0 to 9l still holds. Read `.superpowers/sdd-templates/implementer-rules.md` first; its invariants bind every task. Precedents: `docs/superpowers/rulings/m9k-rulings.txt` and `m9l-rulings.txt` (or the 9l ledger): Tandem menus have no per-state disabling, so a disabled iTerm2 item does nothing; beeps are `MessageBeep(MB_OK)`; floating panels are egui host windows.
- Reuse, do not duplicate: `Pane::{show_view, end_view, own, own_mut}` is the view swap; `Screen::{with_look, splice_rows, term, term_mut}` builds the frame screen; `prompt_ui::ConfirmWindow` and `App::ask` show warnings; `rfd` dialogs as the screenshot panel uses them; `find_ui::beep`; `commands::DONE` and `keyaction::status` record what is done.
- No alacritty patch. Frames are the visible rows (`grid()[Line(0)..Line(rows - 1)]`, the active grid, display offset ignored) plus cursor and size; dirty lines are alacritty's damage (`TermDamage::Full` means none clean, as iTerm2's scrolled case).
- Byte accounting is iTerm2's with Tandem's cell: a line counts `size_of::<Cell>() * (cols + 1)` bytes, every line adds iTerm2's 4-byte metadata length, a diff line adds 5 bytes (type and length) and a diff metadata entry 5 more; line metadata payloads count 0 (Tandem keeps row data inside the row).
- Strings, sizes, orders and alerts are iTerm2's, verbatim (Resume notes), with "iTerm2" in user text replaced by "Tandem"; key names in text use Windows words. Characters outside ASCII are `\u{...}` escapes in code. iTerm2 bugs are kept, each with a checklist line: the first replay frame being the second newest, the key-frame counter counting only diff-eligible frames, diffs reserving the key-frame size, a stranded decoder after eviction (Right to the oldest, Left beeps), Clear Instant Replay during replay clearing nothing, a range export restamping every frame as a key frame, tick marks computed once per panel open.
- Win is Cmd and Alt is Option. New pure code is a plain `pub mod`; app code is `#[cfg(windows)]`. Tests run on EXTREME with `scripts/xt test --lib <filter>`. Do not launch the GUI.

## Review Focus

1. The byte ring's arithmetic: strict `>` comparisons, wrap to offset 0 only when `capacity - end < length`, deallocation from the front, `begin`/`end` after each step. Test: Task 1 `ring_follows_iterm2`.
2. The encoder: frames over half the capacity dropped, diff only with more than 80 % clean lines, same size, and less than half the capacity since the last key frame; the key frame at the 1st, 101st, 201st eligible frame; leading diff frames evicted after any freeing. Test: Task 1 `encoder_follows_iterm2`.
3. The decoder: `seek` finds the first frame at or after a time, `prev` false at the first key and when invalidated, `next` from an invalidated decoder goes to the first frame, a diff chain rebuilds the exact rows. Test: Task 1 `decoder_follows_iterm2`.
4. Recording: no frame without damage; full damage marks no line clean; one frame per pane per 16 ms at most; a replay view never records; the live screen keeps recording during replay. Test: Task 2 `capture_follows_iterm2`, `recording_follows_iterm2`.
5. Replay keys: Esc exits (closes the session for a loaded recording), Left and Right step one or 15 with Shift, everything else beeps, nothing reaches the session, Win chords still reach the menus. Test: Task 3 `replay_keys_follow_iterm2`.
6. Entering and stepping: nothing when nothing was recorded, forward from live beeps, the first entry shows the second newest frame, ends beep. Test: Task 3 `replay_steps_follow_iterm2`.
7. The panel's export states and labels: Export\u{2026} to Set Start (seek 0) to Set End (seek 1) to export, a beep and no change when the end is before the start, Cancel back with no export, "Live View" when live. Test: Task 4 `panel_follows_iterm2`.
8. The `.itr` file: gzip JSON, version 4, the profile without iTerm2's private keys and with a new Guid, every load failure's warning verbatim. Test: Task 6 `recording_file_follows_iterm2`.
9. What ends a replay: another pane made active, a tab whose pane is live, split selection, closing the pane or the window; splitting refused while the panel shows; the panel handle swapped per window. Test: Task 5 `replay_closers_follow_iterm2`.
10. Byte budget worked through: an 80 by 24 key frame is `24 * (line_bytes(80) + 4)`, a one-line diff `24 * 5 + line_bytes(80) + 24 * 5`, and a 4 MiB recorder drops any frame over 2 MiB. Test: Task 1 `bytes_follow_iterm2`.

---

### Task 1: The recorder (pure)

**Files:** Create `src/dvr.rs`; add to `src/lib.rs`.

**Interfaces:**
- Consumes: `alacritty_terminal::{grid::Row, term::cell::Cell}`.
- Produces: `pub const KEY_FRAME_EVERY: u32 = 100`; `pub const CLEAN_SHARE: f64 = 0.8`; `pub fn line_bytes(cols: usize) -> u64`; `pub fn key_bytes(cols: usize, rows: usize) -> u64`; `pub fn diff_bytes(cols: usize, dirty: usize, rows: usize) -> u64`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Kind { Key, Diff }`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Info { pub cols: usize, pub rows: usize, pub cursor: (usize, usize), pub timestamp: i64, pub kind: Kind }`; `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Ring { pub capacity: u64, pub begin: u64, pub end: u64, pub first: u64, pub next: u64, blocks: VecDeque<(u64, u64)> }` with `fn new(capacity: u64) -> Ring`, `fn has_space(&self, len: u64) -> bool`, `fn reserve(&mut self, len: u64) -> (u64, u64)` (scratch offset, blocks freed), `fn allocate(&mut self, scratch: u64, len: u64) -> u64` (the key), `fn deallocate(&mut self)`, `fn last(&self) -> Option<u64>`; `#[derive(Clone, Debug)] pub enum Lines { Key(Vec<Row<Cell>>), Diff(Vec<Option<Row<Cell>>>) }`; `#[derive(Clone, Debug)] pub struct Frame { pub info: Info, pub lines: Lines }`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Appended { ReadOnly, TooBig, Key, Diff }`; `#[derive(Clone, Debug)] pub struct Dvr { pub ring: Ring, pub read_only: bool, frames: VecDeque<Frame>, last: Option<Vec<Row<Cell>>>, last_info: Option<Info>, count: u32, since_key: u64 }` with `fn new(capacity: u64) -> Dvr`, `fn append(&mut self, rows: &[Row<Cell>], clean: &[bool], cursor: (usize, usize), now_us: i64) -> Appended`, `fn is_empty(&self) -> bool`, `fn first_timestamp(&self) -> i64`, `fn last_timestamp(&self) -> i64` (0 when empty), `fn first_timestamp_after(&self, t: i64) -> i64` (0 when none), `fn timestamps(&self) -> Vec<i64>`, `fn can_clear(&self, replaying: bool) -> bool`, `fn clear(&mut self)`, `fn copy_range(&self, from: i64, to: i64, now_us: &mut dyn FnMut() -> i64) -> Dvr`, `fn frame(&self, key: u64) -> Option<&Frame>`; `#[derive(Clone, Debug, Default)] pub struct Decoder { key: Option<u64>, pub rows: Vec<Row<Cell>>, pub info: Option<Info> }` with `fn seek(&mut self, d: &Dvr, t: i64) -> bool`, `fn next(&mut self, d: &Dvr) -> bool`, `fn prev(&mut self, d: &Dvr) -> bool`, `fn timestamp(&self) -> i64`, `fn key(&self, d: &Dvr) -> Option<u64>` (None once its key was evicted: iTerm2's `invalidateIndex:`).

- [ ] **Step 1: failing tests** in `dvr.rs` (rows built with `Row::new(Column(cols))` and a letter per row):
  - `ring_follows_iterm2` (Review Focus 1): `Ring::new(100)`: `has_space(99)` true, `(100)` false (strict); reserve and allocate 60 then 30 give keys 0, 1 at 0 and 60, `end` 90; reserving 20 frees key 0 (`begin` 60), wraps (`capacity - end` 10 < 20) to scratch 0; with `begin > end`, `has_space` is `begin - end > len`; `deallocate` sets `begin` to the freed block's end; `last()` None on a new ring.
  - `encoder_follows_iterm2` (Review Focus 2): with 50 rows, a frame with 41 clean lines after a same-size frame is a Diff, with 40 a Key; a size change is a Key; the first eligible frame is a Key and the next 99 eligible are Diffs, the 101st eligible a Key, while an ineligible frame between them does not advance the count; a frame larger than `capacity / 2` is `TooBig` and leaves the buffer alone; once `since_key` reaches `capacity / 2` the next frame is a Key; after freeing, a leading Diff is evicted until a Key leads; `read_only` gives `ReadOnly`; timestamps are `now_us` at commit.
  - `decoder_follows_iterm2` (Review Focus 3): after key A, diff (row 3 changed), diff (row 7 changed), `seek` to the third timestamp yields the rows of the third frame exactly and `info.cursor` of the third; `seek` past the last is false; `prev` at the first key is false, `next` past the last is false; evicting the decoder's frame makes `key` None, `next` then lands on the first frame and `prev` from None is false; `first_timestamp_after(t)` is the first timestamp greater than `t`, 0 after the last.
  - `bytes_follow_iterm2` (Review Focus 10): `line_bytes(80)` is `size_of::<Cell>() * 81`; `key_bytes(80, 24)` is `24 * (line_bytes(80) + 4)`; `diff_bytes(80, 1, 24)` is `24 * 5 + line_bytes(80) + 24 * 5`; `diff_bytes(80, 24, 24)` exceeds `key_bytes(80, 24)` (so an all-dirty diff falls back to a key frame); a frame of `capacity / 2 + 1` bytes is `TooBig`. Compute from `size_of::<Cell>()`; never hard-code 24.
  - `copy_range_follows_iterm2`: `copy_range(first, last)` of the whole range keeps every timestamp and kind; a partial range gives only Key frames from `seek(from)` while `timestamp <= to`, each stamped by `now_us` (kept); `clear` empties it, `can_clear(true)` and a read-only recorder's `can_clear(false)` are false.
- [ ] **Step 2:** run `scripts/xt test --lib dvr`; expect the new tests to fail.
- [ ] **Step 3:** implement per the Resume notes ("Recorder"). `append` computes the key-frame size, drops `TooBig`, reserves that size (evicting, then evicting leading Diffs), decides key or diff, falls back to a key when the diff would exceed the reservation, allocates the actual length, stores the frame, and remembers the full rows for the next diff. The decoder walks back to the nearest key frame and applies diffs forward (`DC:193-233`).
- [ ] **Step 4:** run the filter; all pass.
- [ ] **Step 5:** commit `instant replay recorder`.

### Task 2: Recording from the grid and the memory setting

**Files:** Modify `src/screen.rs` (`replay_capture`), `src/pane.rs` (`pub dvr: dvr::Dvr`, `replay_at: Option<Instant>`, `fn record_frame`), `src/app/mod.rs` (after the pump loop at `:2444`, `record_replay_frames`; `install_pane` at `:945` sizes `pane.dvr` from `IRMemory`), `schema/global.toml` (`IRMemory`).

**Interfaces:**
- Consumes: Task 1; `Term::damage`, `Term::reset_damage`, `Term::grid`.
- Produces: `pub struct Capture { pub rows: Vec<Row<Cell>>, pub clean: Vec<bool>, pub cursor: (usize, usize) }`; `Screen::replay_capture(&mut self) -> Option<Capture>` (None when nothing is damaged; damage reset either way); `pub const FRAME_GAP: Duration = Duration::from_millis(16)`; `pub fn capacity_for(megabytes: i64) -> u64` (clamped 0 to 1000, times 1024 * 1024); `Pane::record_frame(&mut self, now: Instant, now_us: i64) -> bool` (records from `own_mut()`, never from a view; at most one per `FRAME_GAP`, damage kept for the next one); `App::record_replay_frames(&mut self)`.

- [ ] **Step 1: failing tests**:
  - `screen.rs` `capture_follows_iterm2` (Review Focus 4): a new screen's first capture has every line dirty; a second with no output is None; writing "x" on row 2 gives a capture with row 2 (and the cursor rows) dirty and the rest clean; a scroll (output past the bottom) gives no clean line; the alternate screen's rows are captured while it is active; the cursor comes from the active grid.
  - `pane.rs` `recording_follows_iterm2` (Review Focus 4): `capacity_for(4)` is 4194304, `(-1)` 0, `(5000)` 1048576000; a pane with output records one frame, a second call within 16 ms records nothing and keeps the damage, after 16 ms it records; while `show_view` shows a view, `record_frame` still records the own screen's output and never the view; `capacity_for(0)` records nothing (every frame `TooBig`).
- [ ] **Step 2:** run `scripts/xt test --lib screen` and `pane`; expect failures.
- [ ] **Step 3:** implement. `IRMemory`: `tab = "General"`, `section = "Instant Replay"`, `label = "Memory usage per session (in MB)"`, `type = "int"`, `default = 4`, `min = 0`, `max = 1000`, help "Megabytes of memory each session keeps for Instant Replay. A change applies to sessions started after it." Every pane records, background tabs and tmux panes included (iTerm2's cadence runs for all sessions); a tmux gateway does not.
- [ ] **Step 4:** run both filters and the full `--lib`; all pass.
- [ ] **Step 5:** commit `instant replay recording and memory setting`.

### Task 3: The replay session (pure)

**Files:** Create `src/replay.rs`; add to `src/lib.rs`. Modify `src/pane.rs` (`replay: Option<replay::Replay>`).

**Interfaces:**
- Consumes: Tasks 1 and 2; `screen::{Screen, Look}`, `palette::Palette`.
- Produces: `#[derive(Clone, Debug)] pub struct Replay { pub decoder: Option<dvr::Decoder>, pub read_only: bool }`; `pub fn enter(d: &dvr::Dvr) -> Option<Replay>` (None when `last_timestamp() == 0`; else seeks the last timestamp); `pub fn step(r: &mut Replay, d: &dvr::Dvr, dir: i32) -> bool` (false: beep; a None decoder beeps); `pub fn timestamp_for_fraction(first: i64, last: i64, f: f32) -> i64`; `pub fn seek_fraction(r: &mut Replay, d: &dvr::Dvr, f: f32) -> i64`; `pub fn current(r: Option<&Replay>) -> i64` (-1 when live); `pub fn can_prev(r: Option<&Replay>, d: &dvr::Dvr) -> bool`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum IrKey { Exit, Step { dir: i32, count: u32 }, Beep, Pass }`; `pub fn ir_key(key: &keystroke::Key, shift: bool, win: bool) -> IrKey` (Win chords `Pass` to the menus); `pub fn frame_screen(frame: &dvr::Decoder, cols: usize, rows: usize, palette: Palette, look: Look) -> Screen` (the frame cropped or padded with blank rows to the pane's size, cursor placed and clamped); `pub const SHIFT_STEPS: u32 = 15`.

- [ ] **Step 1: failing tests** in `replay.rs`:
  - `replay_steps_follow_iterm2` (Review Focus 6): `enter` of an empty recorder is None; of three frames seeks the third, and the app's first `step(.., -1)` shows the second (kept, `PS:1470-1532`); `step(.., -1)` at the first frame is false; `step(.., 1)` at the last is false; `timestamp_for_fraction(1000, 2000, 0.25)` is 1250, truncated for fractions; `seek_fraction(.., 1.0)` lands on the last frame; `can_prev(None, ..)` true, at the first frame false; `current(None)` -1.
  - `replay_keys_follow_iterm2` (Review Focus 5): Esc `Exit`; Left `Step { dir: -1, count: 1 }`, Shift+Left count 15, Right dir 1; "a", Enter, Up `Beep`; Win+Alt+B and Win+W `Pass`.
  - `frame_screen_follows_the_pane`: a 3-row frame in a 5-row pane leaves rows 3 and 4 blank; a 10-column frame in a 6-column pane keeps columns 0 to 5; the cursor at (8, 1) clamps to column 5; the screen's text equals the frame's.
- [ ] **Step 2:** run `scripts/xt test --lib replay`; expect failures.
- [ ] **Step 3:** implement per the Resume notes ("Replay").
- [ ] **Step 4:** run the filter; all pass.
- [ ] **Step 5:** commit `instant replay session model`.

### Task 4: The panel model (pure)

**Files:** Create `src/replay_panel.rs`; add to `src/lib.rs`.

**Interfaces:**
- Consumes: `layout::Rect`.
- Produces: `pub const TITLE: &str = "Instant Replay Control"`; `pub const SIZE: (f32, f32) = (602.0, 43.0)`; `pub const MIN_WIDTH: f32 = 270.0`; `pub const ALPHA: f32 = 0.9`; `pub const LIVE: &str = "Live View"`; `pub const NOW: &str = "Now"`; `pub const EXPORT: &str = "Export\u{2026}"`; `pub const SET_START: &str = "Set Start"`; `pub const SET_END: &str = "Set End"`; `pub const CANCEL: &str = "Cancel"`; `pub fn time_label(ts_us: i64, today: (i32, u32), date: &dyn Fn(i64) -> String, time: &dyn Fn(i64) -> String, day_of: &dyn Fn(i64) -> (i32, u32)) -> String`; `pub fn slider(current: i64, first: i64, last: i64) -> Option<f32>` (1.0 when live, None when the range is not positive); `pub fn ticks(timestamps: &[i64], width: f32) -> Vec<f32>` (x per frame, `round(f * (width - 1))`, repeats dropped); `pub fn label_x(frac: f32, slider_x: f32, slider_w: f32, label_w: f32) -> f32`; `pub fn origin(size: (f32, f32), window: Rect, work: Rect, tabs_bottom: f32) -> (f32, f32)`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Export { Normal, SetStart, SetEnd { start: i64 } }`; `#[derive(Clone, Copy, Debug, PartialEq)] pub enum PanelAct { Seek(f32), Export { from: i64, to: i64 }, Beep }`; `#[derive(Clone, Debug)] pub struct Panel { pub export: Export, pub ticks_at_open: Vec<i64> }` with `fn export_button(&mut self, current: i64) -> Vec<PanelAct>`, `fn cancel(&mut self)`, `fn buttons(&self) -> (&'static str, Option<&'static str>)`, `fn band(&self, current: i64, first: i64, last: i64) -> Option<(f32, f32)>`.

- [ ] **Step 1: failing test** `panel_follows_iterm2` (Review Focus 7): Normal `buttons()` is `("Export\u{2026}", None)`; `export_button` gives `[Seek(0.0)]` and `("Set Start", Some("Cancel"))`; again at current 5000 gives `[Seek(1.0)]`, `("Set End", Some("Cancel"))`, band from 5000; again at 9000 gives `[Export { from: 5000, to: 9000 }]` and Normal; from Set End at 4000 gives `[Beep]` and stays in Set End; `cancel` from any state is Normal with no act; `slider(-1, ..)` Some(1.0), `(1500, 1000, 2000)` Some(0.5), `(1000, 1000, 1000)` None; `time_label` of today's time is the time only, of yesterday's "date time"; `ticks` over 3 timestamps at width 101 gives 0, 50, 100 and drops a repeated x; `label_x` clamps inside the slider; `origin` puts a 602-wide panel centered below a window with room under it, above when there is none below, and just above `tabs_bottom` inside when there is neither.
- [ ] **Step 2:** run `scripts/xt test --lib replay_panel`; expect failures.
- [ ] **Step 3:** implement per the Resume notes ("Panel"). Y grows downward on Windows: "below" is past the window's bottom edge.
- [ ] **Step 4:** run the filter; all pass.
- [ ] **Step 5:** commit `instant replay panel model`.

### Task 5: Instant Replay in the app

**Files:** Create `src/app/replay_ui.rs` (mod line in `src/app/mod.rs`), `src/replay_window.rs` (egui, `#[cfg(windows)]`, mod line in `src/lib.rs`). Modify `src/app/mod.rs` (`replay_window: Option<replay_window::ReplayWindow>` per window; `key` calls `replay_key` first at `:1505`; `split` at `:1162` does nothing while the panel is open; focus changes and `close_pane` at `:1187` close Instant Replay; the window event loop routes the panel's events), `src/app/windows_ui.rs` (`swap_live` swaps `replay_window`), `src/app/menu.rs` (`irPrev:`, `clearInstantReplay:`), `src/app/actions.rs` (action 15 runs `ir_advance(-1)`, ignored in a tmux gateway), `src/keyaction.rs` (15 Done), `src/commands.rs` (`irPrev:` and `clearInstantReplay:` in `DONE`; `known_statuses`), `src/app/copymode_ui.rs` (showing a frame leaves copy mode).

**Interfaces:**
- Consumes: Tasks 1 to 4; `Pane::{show_view, end_view}`, `EguiHost::open_with`, `find_ui::beep`, `App::{focused_id, focused_pane, redraw, is_tmux_gateway}`.
- Produces: pure (in `replay.rs`, tested on Linux) `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum ReplayEvent { OtherPaneActive, TabShown { replaying: bool }, SplitSelection, PaneClosed, WindowClosed, Split }` and `pub fn on_event(panel_open: bool, e: ReplayEvent) -> Close` with `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Close { Nothing, ClosePanel, OpenPanel, Refuse }`; `App::ir_advance(&mut self, dir: i32)`; `App::replay_session(&mut self, pane: PaneId) -> bool`; `App::show_frame(&mut self, pane: PaneId)`; `App::replay_key(&mut self, ev: &KeyEvent) -> bool`; `App::close_instant_replay(&mut self, or_terminate: bool) -> bool`; `App::end_replay(&mut self, pane: PaneId)`; `App::in_instant_replay(&self) -> bool`; `App::clear_instant_replay(&mut self)`; `App::replay_seek(&mut self, f: f32)`; `App::update_replay_panel(&mut self)`; in `replay_window`: `pub struct ReplayWindow` with `fn open(event_loop, instance, adapter, device, queue, at: (f32, f32), ticks: Vec<f32>) -> Result<ReplayWindow, String>`, `fn id(&self) -> WindowId`, `fn event(&mut self, e: &WindowEvent) -> bool`, `fn redraw(&mut self, view: &PanelView) -> Vec<replay_panel::PanelAct>`, `pub struct PanelView { pub earliest: String, pub current: String, pub slider: f32, pub band: Option<(f32, f32)>, pub buttons: (&'static str, Option<&'static str>) }`.

- [ ] **Step 1: failing tests**: `commands.rs` `known_statuses` asserts `irPrev:` "Start Instant Replay" and `clearInstantReplay:` "Clear Instant Replay" Done; `keyaction.rs` `statuses_split_now_from_later` has 15 Done (was `Later(10)`); `replay.rs` `replay_closers_follow_iterm2` (Review Focus 9): with the panel open, `OtherPaneActive`, `SplitSelection`, `PaneClosed` and `WindowClosed` give `ClosePanel`, `Split` gives `Refuse`, `TabShown { replaying: true }` gives `OpenPanel` and `{ replaying: false }` `ClosePanel`; with it closed every event gives `Nothing` except `TabShown { replaying: true }` (`OpenPanel`).
- [ ] **Step 2:** run `scripts/xt test --lib commands`, `keyaction` and `replay`; expect failures.
- [ ] **Step 3:** entering and leaving: `ir_advance(dir)` on a live pane beeps for `dir > 0`, else `replay_session` (nothing when `enter` is None) then `step`; on a replaying pane it steps (a beep at the ends); `show_frame` calls `frame_screen` at the pane's size (Decision: no window resize) and `show_view`, leaves copy mode, and redraws. `replay_session` opens the panel when it is not open. `replay_key` runs before every other key handler: with the panel open, `ir_key` decides (Step repeats `step` `count` times, Beep beeps, Exit calls `close_instant_replay(true)`), and every key except `Pass` is taken, releases paired, none reaching the session or a tool. `close_instant_replay(or_terminate)`: a live recording closes the panel; a read-only one with `or_terminate` closes that pane (Task 6), else nothing. Closing the panel (Esc, its close box) calls `end_replay` for the focused pane (`end_view`, `replay = None`). Making another pane active, split selection (move pane), closing the replaying pane, and closing the window close Instant Replay; splitting does nothing while it is open; selecting a tab whose focused pane replays opens the panel, else closes it. Arrangements and restoration save the pane's own screen (they already read `own()`).
- [ ] **Step 4:** the panel: an egui host window titled `TITLE`, `SIZE`, resizable in width only (at least `MIN_WIDTH`), never activated (`WS_EX_NOACTIVATE`), alpha 0.9 and 1 while the pointer is over it, placed by `origin`; the earliest label (`time_label` of the first timestamp), a continuous slider (dragging calls `replay_seek`, which enters replay first when live), the tick strip over it (ticks from `Panel.ticks_at_open`, white 50 % on a dark theme, black 50 % on light), a red 50 % band during Set End, the current label under the knob (`LIVE` when live), `NOW` at the right, the two buttons; Esc on its Cancel button cancels the export states. `update_replay_panel` runs after every step, seek, tab change and panel resize. `clear_instant_replay`: a replaying pane drops its decoder and clears nothing (kept); else the pane's recorder clears when `can_clear(false)`. Key action 15 runs `ir_advance(-1)` (so a bound key keeps stepping back in replay); `irPrev:` does nothing while the focused pane replays (iTerm2 disables it).
- [ ] **Step 5:** run `scripts/xt test --lib` and `scripts/xt build --release`; commit `instant replay in the app`.

### Task 6: Export Recording and Import Recording

**Files:** Create `src/recording.rs`; add to `src/lib.rs`. Modify `src/dvr.rs` (`to_json`, `from_json`), `src/pane.rs` (`Pane::recording(id: PaneId, cols: u16, rows: u16) -> Pane`, no pty), `src/app/replay_ui.rs` (`export_recording`, `import_recording`, the panel's range export), `src/app/menu.rs` (`exportRecording:`, `loadRecording:`), `src/commands.rs` (both in `DONE`; `known_statuses`).

**Interfaces:**
- Consumes: Tasks 1, 3, 5; `flate2`, `serde_json`, `rfd`, `profile::Store`, `uuid`.
- Produces: `pub const EXTENSION: &str = "itr"`; `pub const DEFAULT_NAME: &str = "Recording"`; `pub const VERSION: i64 = 4`; `pub const PRIVATE_KEYS: &[&str]`; `pub fn redact(profile: &serde_json::Map<String, serde_json::Value>, new_guid: &str) -> serde_json::Map<String, serde_json::Value>`; `pub fn encode(dvr: serde_json::Value, profile: serde_json::Map<String, serde_json::Value>) -> Vec<u8>`; `#[derive(Debug, PartialEq)] pub enum LoadError { Read(String), Decompress, Unarchive, Version, Missing, Corrupt }`; `pub fn decode(bytes: &[u8]) -> Result<(serde_json::Value, serde_json::Map<String, serde_json::Value>), LoadError>`; `pub fn warning(e: &LoadError) -> (&'static str /*heading*/, String /*text*/)`; `pub const SAVE_FAILED: &str = "The recording could not be saved."`; `pub const ENCODE_FAILED: &str = "Error encoding recording."`; `Dvr::to_json(&self, from: i64, to: i64, now_us: &mut dyn FnMut() -> i64) -> serde_json::Value`; `Dvr::from_json(v: &serde_json::Value) -> Option<Dvr>` (read-only); `App::export_recording(&mut self, pane: PaneId, from: i64, to: i64)`; `App::import_recording(&mut self, path: Option<std::path::PathBuf>)`.

- [ ] **Step 1: failing test** `recording.rs` `recording_file_follows_iterm2` (Review Focus 8): `decode(encode(d, p))` round-trips a three-frame recorder whose `from_json` rows equal the original's and is read-only; `redact` drops every `PRIVATE_KEYS` entry ("Name", "Command", "Working Directory", "Initial Text", "Triggers", "Key Bindings", the rest per the Resume notes) and sets "Guid"; bytes that are not gzip give `Decompress`; gzip of "x" gives `Unarchive`; version 5 gives `Version`; no "dvr" gives `Missing`; a bad capacity gives `Corrupt`; `warning(&Version)` is `("Can\u{2019}t Load Recording", "This recording is from a newer version of Tandem and cannot be replayed in this version.")`, `warning(&Corrupt)` `("Error Loading Recording", "The recording could not be loaded. It might be corrupted.")`, `warning(&Read("e"))` `("Could not read the file: its envelope was malformed.", "e")`. `known_statuses` asserts `exportRecording:` and `loadRecording:` Done.
- [ ] **Step 2:** run `scripts/xt test --lib recording` and `commands`; expect failures.
- [ ] **Step 3:** export: `rfd` save dialog (default name "Recording", filter "itr", the home folder), the whole range from the menu (nothing when the recorder is empty; Tandem menus do not disable), the panel's range from its Set End through the live pane's recorder; a write error shows `SAVE_FAILED` with the error text through `ConfirmWindow` (OK only). Import: `rfd` open dialog for `.itr`; a failure shows `warning` (OK only); success adds a session-local copy of the file's profile, opens a new tab at index 0 of the current window with `Pane::recording` at the recording's first frame size, installs the read-only recorder, and enters replay at once. Esc in that pane's replay closes its tab; the panel's close box closes that tab too (Decision: not the whole window).
- [ ] **Step 4:** run `scripts/xt test --lib` and the release build; commit `instant replay recordings`.

### Task 7: Parity, checklist, rulings

**Files:** Modify `docs/parity/03-features-and-menus.md`, `docs/parity/02-global-settings.md`, `docs/parity/00-roadmap.md` (the milestone 10a paragraph and counts); create `docs/acceptance/m10a-checklist.md`, `docs/superpowers/rulings/m10a-rulings.txt`.

- [ ] **Step 1:** parity, rows found by title: 03 Edit > Clear Instant Replay, View > Start Instant Replay, Session > Log > Import Recording and Export Recording (have); Time-travel scrollback, Step backward/forward, Scrub bar / seek, Export replay range, Clear Instant Replay buffer, Per-pane independent buffers (have, with the research section 0 corrections for Step and Export); 02 Instant Replay memory usage limit (have, key `IRMemory`), Start Instant Replay (have), webInstantReplayFrameRate (n/a: no browser sessions).
- [ ] **Step 2:** `m10a-checklist.md` in the m9l form (copy `tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, `OpenConsole.exe` together; setup: a profile with a key bound to Start Instant Replay, a second window): run `dir /s C:\Windows` and replay it; Win+Alt+B shows the second-newest frame; Left, Shift+Left, Right, Shift+Right, a letter beeps, Esc returns live with the live output current; the slider and its ticks; the labels ("Live View", the earliest time, "Now"); Export\u{2026}, Set Start, Set End, a reversed range beeps, Cancel; Session > Log > Export Recording and Import Recording (the tab at index 0, Esc closes it); Win+Alt+K clears (and does nothing useful during replay, kept); `IRMemory` 0 records nothing, a new session after a change uses the new size; splitting refused and switching panes closing replay; a tmux pane and a background tab record; one line per kept bug and ruling a person can see.
- [ ] **Step 3:** `m10a-rulings.txt` in the `Ruling: ... - cost if wrong: ...` form: every Decision below, then each task's rulings and deferred minors as they happen.
- [ ] **Step 4:** run `scripts/xt test --lib`; commit `docs: m10a parity, checklist, rulings`.

---

## Resume notes (facts gathered before Task 1, from the iTerm2 source)

- Nothing of Task 1 is written yet. Start the ledger at `.superpowers/sdd/2026-10-09-m10a-instant-replay/progress.md` with `task-start PLAN 1`. Paths below are under `~/gitrepos/iterm2-ref/sources/` (HEAD `64791fc`). `D` `DVR/DVR.m`, `B` `DVR/DVRBuffer.m`, `E` `DVR/DVREncoder.m`, `DC` `DVR/DVRDecoder.m`, `IRW` `DVR/iTermInstantReplayWindowController.m`, `IRX` `DVR/Base.lproj/InstantReplay.xib`, `PS` `PTYSession/PTYSession.m`, `PT` `TerminalView/PseudoTerminal.m`, `TAB` `TerminalView/PTYTab.m`, `S` `VT100Screen/VT100Screen.m`, `TV` `TerminalView/PTYTextView.m`, `RC` `Codecs/iTermRecordingCodec.m`, `MM` `MainMenu/Base.lproj/MainMenu.xib`.

### Tandem code these tasks build on (verified at `5104d73`)

- Views: `Pane.live` (`src/pane.rs:184`), `own` / `own_mut` (`:647,651`), `show_view` (`:656`), `end_view` (`:681`), `freeze` (`:667`); Filter and Zoom use them (`src/app/find_ui.rs:392,402,522`); `Pane::pump` (`src/pane.rs:302`, output into `live` or `screen` at `:322`); the pump loop (`src/app/mod.rs:2441-2457`); `install_pane` (`:945`), `start_pane_as` (`:901`).
- Screen: `Screen::with_look` (`src/screen.rs:326`), `term` / `term_mut` (`:418,422`), `splice_rows` (`:883`), `text_rows` (`:897`), `line_text` (`:1117`); alacritty damage `Term::damage` (`crates/alacritty_terminal/src/term/mod.rs:484`, `TermDamage` `:178`); `Row` Clone, serde and `PartialEq` (`crates/alacritty_terminal/src/grid/row.rs:15-17,57`); `Cell` (`crates/alacritty_terminal/src/term/cell.rs:145`); patches list `crates/alacritty_terminal/TANDEM-PATCHES.md` (14 patches, none needed here).
- Keys and commands: `App::key` (`src/app/mod.rs:1505`, `toolbelt_key` first at `:1507`), `dispatch` (`:1551`), `split` (`:1162`), `close_pane` (`:1187`), `new_tab` (`:1145`); `run_chord` (`src/app/menu.rs:296`), the selector match (`:584` area); `later` (`src/app/actions.rs:106`); key action 15 `Later(10)` (`src/keyaction.rs:380`, test `:438`), `schema/keymaps.toml:310` ("Start Instant Replay"); menus `clearInstantReplay:` (`schema/menus.toml:940`, Win+Alt+K), `irPrev:` (`:1159`, Win+Alt+B), `loadRecording:` (`:1323`), `exportRecording:` (`:1327`); `commands::owner` 10 for these (`src/commands.rs:112`), `DONE` (`:21`), `known_statuses` (`:248`).
- Windows and UI: `swap_live` (`src/app/windows_ui.rs:92`), `EguiHost::open_with` (`src/egui_host.rs:38`), the Make Screenshot panel as the floating-panel precedent (`src/shot_ui.rs`, `src/app/screenshot_ui.rs`), `beep` (`src/app/find_ui.rs:203`), `ConfirmWindow` (`src/prompt_ui.rs:172`), `App::ask` (`src/app/infobar_ui.rs:11`), `show_toast` (`src/app/view_ui.rs:97`); settings rows in `schema/global.toml` (int rows with `min`/`max` as at `:296-297`).

### Recorder

- `B:196-215` space test (strict `>`); `B:145-163` reserve (free from the front while no space; scratch at `end` when `begin <= end` and `capacity - end >= len`, else 0; at `end` when `begin > end`); `B:165-179` allocate (`end = pos + len`); `B:181-187` deallocate (`begin = pos + len` of the freed); `B:222-225` last key; `B:233-250` first timestamp after (insertion index of `t + 1`).
- `D:76-105` append: read-only ignored; `length > capacity / 2` dropped; reserve the full length; `E:133-151` evict leading diffs after freeing. `E:112-122` diff eligibility (clean `> 0.8 * height`, same length and size, `since_key < capacity / 2`); `E:124-130` `!eligible || count++ % 100 == 0`; `E:182-203` key frame (`since_key = 0`); `E:268-362` diff layout (per line 1 + 4 [+ line], per metadata 1 + 4 [+ payload]; over the reservation falls back to a key frame); `E:260` timestamp at commit (microseconds, `E:39-47`).
- `DC:84-96` seek (first `>= t`), `DC:114-138` next and prev, `DC:145-150` invalidation, `DC:193-233` rebuild from the nearest key frame. `D:107-119` clear (`!readOnly && no decoders`); `D:134-158` first and last timestamps, 0 when empty; `D:164-174` dictionary; `D:215-245` range copy (key frames, fresh timestamps); `D:176-213` load (versions 1 to 4, read-only).
- `S:121-122` capacity `IRMemory * 1024 * 1024` once per session; `TV:2880-2917` a frame per refresh when dirty, clean lines only when not scrolled; `S:683-699` frame from the current grid (cursor, width, height). `PS:1470-1480` a replay view records nothing; the live session keeps recording.

### Worked sizes (with a 24-byte `Cell`; the tests compute from `size_of::<Cell>()`)

- One 80-column line is 1944 bytes; an 80 by 24 key frame 46752 bytes; a 4 MiB recorder holds about 89 such key frames before the first eviction.
- A one-line diff of that frame is 2184 bytes; 99 of them after a key frame use about 216 KB, far under the `capacity / 2` key-frame trigger, so the 101st eligible frame (the counter) is what forces the next key frame.
- A 400 by 110 frame is 1059080 bytes (fits 4 MiB); 400 by 220 is 2118160 bytes, over half, so a very large window records nothing until `IRMemory` grows.
- `IRMemory` 0 gives capacity 0: every frame is `TooBig`, the menu command finds nothing recorded and does nothing.

### Replay

- `PT:10473-10484` irAdvance (forward from live beeps "no time travel allowed"); `PT:10589-10608` replaySession (nothing when `lastTimeStamp == 0`; synthetic session, panel opened); `TAB:1835-1843` setDvrInSession; `PS:1470-1480` setDvr seeks the last and shows it; `PS:1502-1532` stepping, beeps at the ends; `PS:15314-15375` setDvrFrame (copy mode off, window resized to the frame unless tmux, titles refreshed); `PS:1534-1542` seek at least; `PT:14732-14738` fraction to time.
- `PS:12795-12802,12855-12891` keys in replay (Esc, Left/Right, Shift 15, beep). `PT:10528-10539` close or terminate; `PT:10446-10459` leaving puts the live session back (`PT:10736-10765`, window frame restored); `IRW:143-145` panel close. Closers: `TAB:907-909` another active pane, `PT:2766-2770`, `PT:12223-12226`, `PT:4910-4914`; `PT:11124-11129` split refused; `PT:7520-7528` a tab change shows or hides the panel. `PT:13940-13943` Start Instant Replay enabled only when not replaying and `canInstantReplayPrev` (`PS:7909-7918`). `PS:12035-12042` key action 15 (not in a tmux gateway). `PT:13744-13746` with `PS:1490-1500` Clear (during replay: decoder dropped, nothing cleared).

### Panel

- `IRX:23-99` "Instant Replay Control", 602 by 43, utility, closable, resizable, hides on deactivate; slider mini, continuous, 0 to 1, value 1; labels "Time" (earliest), "Now", "Live View"; buttons "Export\u{2026}" and hidden "Cancel" (Esc). `IRW:80-86` never key; `IRW:13,106-113,139` alpha 0.9, 1 on hover, floating; `IRW:268-271` min width 270, fixed height.
- `IRW:301-362` update: live slider 1 and "Live View"; else time label and `(t - first) / (last - first)` when positive; earliest label = first timestamp's string, latest "Now"; slider starts after the earliest label with the same margin; events strip inset 5; current label centered on the knob and clamped. `IRW:279-299` time: medium time style, plus short date when not today. `IRW:27-78,160-172` ticks (1 px, `round(f * (width - 1))`, repeats skipped, white/black 50 %, computed at delegate set), band red 50 %. `IRW:201-260` export states. `PT:10491-10509` origin (below, else above, else inside above the tab view's bottom). `PT:10461-10466` slider seek enters replay first.

### Panel geometry (`IRX:26-99`, points; Tandem uses the same numbers as logical pixels)

- Content 602 by 43 inside a visual-effect background; y grows upward in the xib, so "y 35" is near the top of a 43-high panel.
- Earliest label: x 18, y 36, 62 by 14, mini system font, left aligned, clipped, initial text "Time".
- Slider: x 86, y 35, 389 by 15, mini, two tick marks drawn above (the ends), continuous, min 0, max 1, value 1.
- Events strip: x 88, y 44, 391 by 4, drawn over the slider track; re-laid out as `x = slider.x + 5`, `w = slider.w - 10` on each update.
- Latest label: x 481, y 36, 26 by 14, "Now". Current label: x 444, y 20, 63 by 14, justified, "Live View".
- First button: x 512, y 35, 71 by 16, mini push button "Export\u{2026}". Second: x 512, y 18, same size, "Cancel", hidden, Esc.
- Resizing: width only, at least 270; the slider, strip and labels keep their right margins (autoresizing: slider and strip width-sizable).

### Menus and key actions

- Edit > Clear Instant Replay, Opt+Cmd+K (`MM:1071-1074`, `clearInstantReplay:`); View > Start Instant Replay, Opt+Cmd+B (`MM:1308-1311`, `irPrev:`); Session > Log > Import Recording (`MM:1505-1509`, `loadRecording:`, app level) and Export Recording (`MM:1511-1515`, `exportRecording:`). Tandem: Win+Alt+K, Win+Alt+B, no keys.
- Key action "Start Instant Replay" is `KEY_ACTION_IR_BACKWARD` (Tandem code 15, `schema/keymaps.toml:310`); "Unsupported Command (deprecated)" is `KEY_ACTION_IR_FORWARD` and stays a no-op (`02-global-settings.md:376`, already have).
- There is no Step Forward menu item; stepping forward is Right in replay or the slider.

### Windows notes for the app

- The panel must not take activation from the terminal window: `WS_EX_NOACTIVATE` plus `WindowAttributes::with_active(false)`; clicks on its controls still work. It hides when Tandem is not the active app (iTerm2's hides-on-deactivate).
- Times: `GetDateFormatEx(LOCALE_NAME_USER_DEFAULT, DATE_SHORTDATE, ..)` and `GetTimeFormatEx(LOCALE_NAME_USER_DEFAULT, 0, ..)` (seconds included, the medium style); pure code takes them as closures so tests pass fixed strings.
- Dialogs: `rfd::FileDialog::new().set_file_name("Recording").add_filter("Instant Replay recording", &["itr"]).set_directory(home)`; the open dialog uses the same filter.
- A loaded recording's pane has no pty (`Pane::recording`), so `pump` returns false, it never exits, and closing it needs no confirmation (nothing runs).

### Recordings

- `RC:137-208` export: default name "Recording", `itr`, home folder; dictionary `{ dvr, profile, version: 4 }` gzipped; private keys removed and a new GUID; errors "The recording could not be saved." (heading, error text as title) and "Error encoding recording.". `RC:23-135` import: the warnings of research section 4; a new synthetic tab at index 0 in replay; read-only. `PT:14078-14081` Export enabled when not empty. `PT:10442-10444` the panel exports from the live session. `AD` (`AppKit/iTermApplicationDelegate.m:812-815,3743-3745`) Import is app level and opens `.itr` files.
- Private keys (`RC:158-173`), Tandem names: "Name", "Command", "Working Directory", "Automatically Log", "Archive", "Archive Directory", "Logging Style", "Description", "Initial Text", "Tags", "Title Components", "Original Guid", "AWDS Window Directory", "AWDS Tab Directory", "AWDS Pane Directory", "Log Directory", "Log Filename Format", "Show Status Bar", "Status Bar Layout", "Has Hotkey", "Triggers", "Triggers Use Interpolated Strings", "Smart Selection Actions Use Interpolated Strings", "Smart Selection Rules", "Semantic History", "Bound Hosts", "Dynamic Profile Parent Name", "Dynamic Profile Parent GUID", "Dynamic Profile Filename", "Dynamic Profile Rewritable", "Dynamic Profile", "Key Bindings" (check each against `ITAddressBookMgr.h` and `schema/profile.toml` in Task 6; a key Tandem lacks is still removed if present).

### Kept bugs (one checklist line each)

- First Start Instant Replay shows the second-newest frame.
- The key-frame counter advances only on diff-eligible frames.
- A diff reserves (and may evict for) the full key-frame size.
- A replay whose frame was evicted keeps showing it; Right jumps to the oldest frame, Left beeps.
- Clear Instant Replay during replay strands the replay and clears nothing.
- A range export stores every frame as a key frame stamped with the export time.
- The panel's tick marks do not update while it stays open.

### Corrections to the research (the source wins)

- The first replay frame is the second newest (setDvr shows the newest, then the step back), not the newest.
- Frames are recorded at display refresh from dirty lines, not per byte of output; a burst between refreshes is one frame.
- `IRMemory` is per session and read at session start; the parity text "reserved" is a cap, not an allocation.
- Clear Instant Replay during replay does not clear the recording.
- Research line numbers checked at `64791fc`.

### Decisions (each goes in `m10a-rulings.txt`)

- Frames are alacritty rows cloned from the active grid; dirty lines come from alacritty's damage tracking, read and reset only by the recorder; no alacritty patch. Cost: cursor-only moves count as damage (as iTerm2's dirty bits do).
- The byte ring is emulated exactly with Tandem's cell size, line metadata payloads counted 0. Cost: the number of frames a given `IRMemory` holds differs from iTerm2's by the cell-size ratio.
- Recording cadence: at most one frame per pane per 16 ms, from the pump pass (damage accumulates between frames). Cost: none visible.
- A frame of a different size than the pane is cropped or padded, and the window is not resized (iTerm2 resizes the window to the frame). Cost: a frame recorded in a larger pane shows cut off.
- The panel is an egui host window that never activates; Tandem's keys reach it only through the pane (as iTerm2's never-key panel). Cost: none.
- The `.itr` file is Tandem's own (gzip of JSON rows); iTerm2 recordings are not readable (NSKeyedArchiver and `screen_char_t` with process-local complex characters). Cost: no exchange of recordings with iTerm2.
- A loaded recording opens as a session-local profile copy in a tab at index 0; closing its panel closes that tab, not the whole window (iTerm2 closes the window). Cost: differs from iTerm2 in the user's favor.
- User text replaces "iTerm2" with "Tandem"; times use the Windows locale's short date and medium time formats.
- Win chords reach the menus during replay (iTerm2's key equivalents run before the session's keyDown); every other key is taken.
- The replay pane keeps the live pane's title and status bar; tmux panes record and replay like others (no window resize, so the tmux size lock is untouched).
- `IRMemory` is read when a pane is installed; a change applies to panes started later (iTerm2 reads it once per screen). Cost: none.
- Frame rows carry the Tandem row patches (marks, folds, notes, blocks, buttons, portholes, image tiles) because they are whole rows; an image tile whose image the store has since dropped draws blank in replay. Cost: a replayed inline image can be missing.
- Diff frames store cloned rows (no sharing between frames), so the byte budget is also the real memory bound, within the cell-size ratio. Cost: none.
- A tmux gateway pane has no recorder (it shows no screen); key action 15 there does nothing (`PS:12037-12039`). Cost: none.
- The recorder always reads the pane's own screen, so a Zoom or Filter view, the composer, and Make Screenshot's frozen copy never enter a recording. Cost: none.
- Parity rows are found by title.

### Pitfalls for the implementer

- Nothing else in Tandem may call `Term::reset_damage`, or recordings lose lines; grep for it in Task 2 and keep the recorder the only reader.
- `Pane::show_view` refuses while `frozen` (Make Screenshot); entering replay then does nothing but the panel still opens, which is harmless; do not unfreeze.
- A replay view is a full `Screen`: triggers, marks and shell integration must not run on it (it is never fed bytes, only spliced rows).
- The decoder holds rows, not keys into the recorder, so a `Pane` can drop its recorder (pane closed) while the panel still shows; close the panel first in `close_pane`.
- `swap_live` must swap `replay_window`, or a second window's panel steals the first's events.
- Keep `known_statuses` and `statuses_split_now_from_later` in step with `DONE` and `keyaction::status`.
