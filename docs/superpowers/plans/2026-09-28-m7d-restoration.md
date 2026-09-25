# Tandem Milestone 7d: Session Restoration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's session restoration: every window, tab, split, and session's contents come back at the next launch (policy "Use System Window Restoration Setting"), with the "Session Contents Restored on <date>" banner and a new shell in the saved folder; with `runJobsInServers` the shells run in a separate `tandem-host.exe`, so after a crash or force quit Tandem reattaches to the still-running jobs; the quit question says whether sessions will be restored; unclaimed host sessions are adopted into one new window.

**Architecture:** `restore_state.rs` is the saved state (the 7c arrangement of every window with contents, plus a host session id and output offset per session), its debounce, its atomic file write, and the banner. The contents are styled text limited to SGR sequences (`sgr_safe`), so colors come back and nothing else in the saved bytes can act. `host_proto.rs` is the host protocol (length-prefixed frames) and the per-session output ring with absolute byte offsets. `host.rs` (run by `src/bin/tandem-host.rs`) owns every ConPTY through today's `pty.rs`, serves one UI at a time over `\\.\pipe\tandem-host-<user SID>` with a DACL for that user, keeps reading output while no UI is attached, and exits when detached with no sessions. `host_client.rs` gives the UI a `HostedPty` with the same methods as `Pty`; `Pane` holds either one (`pane::Io`). At reattach the UI restores the saved contents, asks the host for output since the saved offset, and forces a repaint when the ring dropped bytes.

**Tech Stack:** as milestone 7c; the `plist` crate for the state file (binary plist); `windows-sys` named pipes, security descriptors, and `MoveFileExW`.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7d-restoration-facts.md` (all sections) and `m7-iterm2-facts.md` section 5. Rows: `03-features-and-menus.md` `:639-644`; `02-global-settings.md` `:645`, `:660`, `:863`, `:867`, `:877`, `:904`, and `killJobsInServersOnQuit`.

## Global Constraints

- Everything from milestones 0 to 7c still holds.
- Settings (Advanced, iTerm2 keys, defaults, help texts from the research file section 7): `runJobsInServers` (on, restart needed), `killJobsInServersOnQuit` (on), `showSessionRestoredBanner` (on), `timeoutForDaemonAttachment` (10 s), `restoreWindowContents` (on), `maxHistoryLinesToRestore` (20000), `restoreWindowsWithinScreens` (on). "iTerm2" in help texts reads "Tandem"; the macOS System Settings sentence in `restoreWindowContents` is dropped.
- Policy 0 (neither `OpenArrangementAtStartup` nor `OpenNoWindowsAtStartup`) restores the saved windows at launch. Windows has no `NSQuitAlwaysKeepsWindows`, so policy 0 always restores.
- The state is saved when Tandem loses focus (every window unfocused), 5 s after the last change but at most once per 30 s, and synchronously at quit before any window closes. Closing every window and then quitting saves a state with no windows.
- The banner: `Session Contents Restored on <date>` (restore time, the user's locale, medium date and short time), one full-width line in the system-message grays (background gray = floor((B + 0.15 s) 255), text gray = floor((B + 0.4 s) 255), s = -1 when the profile background brightness B > 0.5, else +1), after the contents, only when no job was reattached, only with `showSessionRestoredBanner`. Triggers do not see it.
- Quit question body: `Sessions will be restored automatically when Tandem is relaunched.` when jobs will be kept (`runJobsInServers`, `restoreWindowContents`, policy 0, and `killJobsInServersOnQuit` off), else `All sessions will be closed.` With defaults Quit saves the state, then the host ends every session.
- Orphans: every live host session no restored pane claims goes as a tab into ONE new window with the default profile, not activated, each with the bar `This already-running session was restored but its contents were not saved.` and a repaint (a resize to one column wider, then back).
- The host accepts one UI at a time; a second UI is refused and runs its sessions in-process with a banner saying why. The pipe's DACL grants only the current user; the UI checks that the pipe's server process runs as the same user before it sends anything.

## Review Focus

1. Tandem is killed (Task Manager) while `ping -t` runs in a split pane. Expect: the next launch shows the same windows and splits, the ping still running in the same pane, and no lost or doubled lines between the saved contents and the replay. Test: Task 3 `ring_since_offsets` and Task 6 `reattach_plan_matches_by_id`.
2. The host's ring for a session overflows while no UI is attached (a program printing 10 MB). Expect: the replay starts at the oldest kept byte, the pane is told bytes were dropped, and the screen is repainted, not garbled. Test: Task 3 `ring_drops_the_oldest_and_says_so`.
3. A state file that is truncated, from an older version, or not a state file. Expect: nothing is restored, one banner says why, the file is kept as `state.bad` for inspection, and Tandem opens a normal window. Test: Task 1 `bad_state_is_refused`.
4. A second Tandem process starts while the first is attached to the host. Expect: the host refuses it cleanly; the second UI runs its shells in-process and says so; the first UI's sessions are untouched. Test: Task 4 `second_client_is_refused`.
5. Saved contents holding escape sequences other than SGR (an OSC 52 clipboard write, a title change, DECSET 1049). Expect: only colors and attributes survive; nothing reaches the clipboard, title, or modes. Test: Task 1 `only_sgr_survives`.

---

### Task 1: The restoration state model

**Files:** Create `src/restore_state.rs`; modify `src/styled_copy.rs` (a whole-buffer SGR text), `src/lib.rs`.

**Interfaces:** `State { version, host: Option<String>, windows: Vec<Value> }`; `encode(&State) -> Vec<u8>` (binary plist), `decode(&[u8]) -> Result<State, String>`; `write_atomic(path, bytes) -> Result<(), String>` (temp file then `MoveFileExW` replace); `Debounce { dirty_at, last_write }` with `mark(now)`, `due(now) -> bool` (5 s quiet, 30 s floor), `written(now)`; `sgr_safe(text) -> String` (keeps printable text, CR, LF, TAB, and `ESC [ <digits;:> m`; drops every other control and sequence); `cap_lines(text, n)`; `banner_text(date) -> String`; `banner_grays(bg: Rgb) -> (u8, u8)`; per-session keys `Host Session` (`{Id, Instance, Offset}`).

- [ ] Failing tests `state_round_trip`, `bad_state_is_refused`, `debounce_waits_and_floors`, `only_sgr_survives`, `contents_are_capped`, `banner_grays_follow_the_background`, `atomic_write_replaces`. Commit `Restoration state`.

### Task 2: Save and restore without the host

**Files:** Create `src/app/restore_ui.rs`; modify `src/app/mod.rs` (dirty marks on output and layout changes, focus loss, quit, startup policy 0), `src/app/arrange_ui.rs` (contents as SGR text; the host keys), `src/screen.rs` (`feed_restored`: SGR-safe text without triggers, then the banner as cells), `src/placement.rs` (`within_screens`: iTerm2's shift of a partly off-screen frame into the monitors' bounding box, keeping today's zero-overlap move), `schema/global.toml` (the seven keys), `src/startup.rs`.

**Interfaces:** `App::save_state(now, force)`, `App::restore_state() -> bool`; `Screen::feed_restored(text, banner: Option<(String, (u8, u8))>)`; `placement::within_screens(rect, monitors) -> Rect`.

- [ ] Failing tests `restored_contents_keep_colors_and_fire_nothing`, `banner_is_one_full_line`, `frames_shift_into_the_screens`, `policy_zero_restores`. Commit `Save and restore windows`.

### Task 3: The host protocol and the output ring

**Files:** Create `src/host_proto.rs`; modify `src/lib.rs`.

**Interfaces:** `Msg` enum: `Hello { version }`, `Welcome { version, host_pid, instance, sessions: Vec<Live> }`, `Refused { reason }`, `Launch { tag, cmdline, cwd, env, cols, rows }`, `Launched { tag, id, pid }`, `LaunchFailed { tag, error }`, `Attach { id, since }`, `Attached { id, from, dropped }`, `Write { id, bytes }`, `Resize { id, cols, rows }`, `Clear { id }`, `Pause { id, paused }`, `Output { id, offset, bytes }`, `Pending { id, bytes }`, `Exited { id, code }`, `Forget { id }`, `Kill { id }`, `KillAll`; `Live { id, pid, cmdline, cwd, cols, rows, exited: Option<u32>, end: u64 }`; `encode(&Msg) -> Vec<u8>` (u32 length, then fields), `Decoder::push(bytes)` and `next() -> Option<Result<Msg, String>>` with a 16 MB frame cap; `Ring { cap }` with `push(bytes)`, `end() -> u64`, `since(offset) -> (u64 start, bool dropped, Vec<u8>)`.

- [ ] Failing tests `messages_round_trip`, `partial_frames_wait`, `oversized_frames_are_refused`, `ring_since_offsets`, `ring_drops_the_oldest_and_says_so`. Commit `Host protocol`.

### Task 4: The host process

**Files:** Create `src/host.rs`, `src/bin/tandem-host.rs`, `src/pipe_sec.rs` (the per-user DACL, the SID string, the server-owner check); modify `src/lib.rs`.

**Interfaces:** `host::run(pipe_name, api) -> Result<(), String>` (accept loop: one client, a second gets `Refused`; per session a pty, a ring of 4 MB, and a pump thread that pushes output into the ring and forwards it while attached; the host exits when no client is attached and no session is live or unforgotten, after a 5 s grace); `pipe_sec::pipe_name() -> Result<String, String>`, `pipe_sec::same_user(pipe) -> Result<(), String>`.

- [ ] Failing tests (run a host in a test thread on a unique pipe name): `host_launches_and_echoes`, `output_waits_while_detached`, `second_client_is_refused`, `exited_sessions_wait_for_forget`. Commit `Host process`.

### Task 5: Hosted sessions

**Files:** Create `src/host_client.rs`; modify `src/pane.rs` (`Io { Local(Pty), Hosted(HostedPty) }`), `src/app/mod.rs` (the host connection at launch when `runJobsInServers`; start `tandem-host.exe` beside `tandem.exe` detached when the pipe is absent; a banner and in-process shells when it cannot).

**Interfaces:** `Client::connect(pipe, timeout) -> Result<(Client, Welcome), String>`, `Client::launch(...) -> Result<HostedPty, String>`, `Client::attach(id, since, notify) -> Result<(HostedPty, Attached), String>`; `HostedPty` methods as `Pty` (write, resize, clear, try_read, pid, set_paused, pending, exit_code) plus `id()` and `offset()`; dropping a `HostedPty` sends `Kill` then `Forget` unless `detach()` was called.

- [ ] Failing tests `hosted_pane_runs_a_shell`, `hosted_exit_code_arrives`, `drop_kills_the_session`. Commit `Run shells in the host`.

### Task 6: Reattach and orphans

**Files:** Create `src/reattach.rs`; modify `src/app/restore_ui.rs`, `src/app/windows_ui.rs` (`NewWindow::Orphans`), `src/app/mod.rs` (the attach timeout).

**Interfaces:** `plan(saved: &[(PaneKey, HostKey)], instance, live: &[Live]) -> Plan { attach: Vec<(PaneKey, u64 id, u64 since)>, fresh: Vec<PaneKey>, orphans: Vec<u64>, ended: Vec<(PaneKey, u32)> }` (match by instance and id, then pid); `jiggle(cols, rows) -> [(u16, u16); 2]`.

- [ ] Failing tests `reattach_plan_matches_by_id`, `other_instances_are_orphans`, `ended_sessions_show_as_ended`, `jiggle_widens_then_restores`. Commit `Reattach to running shells`.

### Task 7: Quit semantics, parity, and acceptance

**Files:** modify `src/close_rules.rs` (`quit_body(keep_jobs)`, `keep_jobs(...)`), `src/app/close_ui.rs`, `src/app/mod.rs` (quit: save, then `KillAll` unless jobs are kept, then detach), the parity rows, `docs/acceptance/m7d-checklist.md`.

**Interfaces:** `keep_jobs(run_in_servers, restore_contents, policy, kill_on_quit) -> bool`; `quit_body(keep: bool) -> &'static str`.

- [ ] Failing test `quit_body_follows_kept_jobs`. Update rows, recount, write the checklist. Commit.
