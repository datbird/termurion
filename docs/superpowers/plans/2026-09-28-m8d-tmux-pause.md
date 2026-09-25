# Tandem Milestone 8d: tmux Pause Mode

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's tmux pause mode (tmux 3.2 and later): after the version is known Tandem sends `refresh-client -fpause-after=<TmuxPauseModeAgeLimit>`, reads `%extended-output` latencies into iTerm2's buffer-size monitor (a four-sample linear fit, a red zone at half the limit, five red ticks before a warning, hysteresis on the shown seconds), warns "This session will pause in about N seconds because it is buffering too much data." [Pause Settings], shows "tmux paused this session because too much output was buffered." or "Session paused." [Unpause, Settings] when a pane pauses, unpauses by recapturing the pane (history, alternate screen, state, pending output, `refresh-client -A '%P:continue'`), and offers Shell > tmux > Pause Pane with its check mark.

**Architecture:** `tmux_pause.rs` is the pure monitor (samples, fit, ttl, red-zone counting, the warning's hysteresis) and the command builders. `tmux_controller.rs` enables pause mode, feeds the monitor from `%extended-output`, handles `%pause`, and runs the unpause captures as an opener whose result replaces the pane's contents. The window shows the bars and the menu item.

**Tech Stack:** as milestone 8c.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8-tmux-facts.md` section 4.3 and 6.1 (Pausing), source `tmux/iTermTmuxBufferSizeMonitor.m`, `PTYSession.m` `tmuxControllerSessionSetTTL`, `setTmuxPaused`, `showTmuxPausedAnnouncement`. Rows: `03-features-and-menus.md` `:69` (Pause Pane), `:428` (pause/buffer management); `02-global-settings.md` `:169-171` (TmuxPauseModeAgeLimit, TmuxUnpauseAutomatically, TmuxWarnBeforePausing), the advanced `noSyncDontWarnAboutTmuxPause`.

## Global Constraints

- Everything from milestones 0 to 8c still holds.
- Texts and buttons are iTerm2's. The age limit defaults to 120 seconds (minimum 1); Unpause automatically defaults off; Warn before pausing defaults on.
- The monitor runs once a second; a sample is taken at most every 0.5 s; four samples are kept; samples older than 2 s reset the series.

## Review Focus

1. A pane whose output age grows steadily to the limit. Expect: a warning after five red ticks, its seconds counting down with iTerm2's hysteresis, then the pause bar when `%pause` comes. Test: Task 1 `warnings_follow_iterm2`.
2. `%pause` with Unpause automatically on. Expect: no bar; the pane unpauses after one second. Test: Task 2 `automatic_unpause_waits_a_second`.
3. Unpause of a pane in the alternate screen. Expect: the pane's contents are replaced from fresh captures, not appended. Test: Task 2 `unpause_replaces_contents`.
4. Pause Pane on a tmux older than 3.2 or with pause mode off. Expect: the menu item is disabled and nothing is sent. Test: Task 2 `pause_needs_pause_mode`.
5. The age limit changes while attached. Expect: `refresh-client -fpause-after=N` is sent again. Test: Task 2 `age_limit_changes_are_sent`.

---

### Task 1: The monitor

**Files:** Create `src/tmux_pause.rs`; modify `src/lib.rs`.

- [ ] Failing tests `series_fit_like_iterm2`, `warnings_follow_iterm2`, `commands_follow_iterm2`. Commit `tmux pause monitor`.

### Task 2: Pause in the controller

**Files:** modify `src/tmux_controller.rs`, `src/tmux_proto.rs` use.

- [ ] Failing tests `automatic_unpause_waits_a_second`, `unpause_replaces_contents`, `pause_needs_pause_mode`, `age_limit_changes_are_sent`. Commit `tmux pause in the controller`.

### Task 3: Bars and the menu

**Files:** modify `src/app/tmux_tabs_ui.rs`, `src/app/menu.rs` (Shell > tmux > Pause Pane with its check), `src/commands.rs`, `schema/global.toml` (`TmuxPauseModeAgeLimit`, `TmuxUnpauseAutomatically`, `TmuxWarnBeforePausing`, `noSyncDontWarnAboutTmuxPause`).

- [ ] Commit `tmux pause bars and menu`.

### Task 4: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m8d-checklist.md`. Commit.
