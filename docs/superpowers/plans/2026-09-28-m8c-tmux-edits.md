# Tandem Milestone 8c: tmux Edits, Sizes, Persistence, and Detach

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Changes made in Tandem reach tmux the way iTerm2 sends them: window and font sizes (`refresh-client -C`, per-window sizes on 3.4, `@iterm2_size`), divider drags (`resize-pane`), splits (`split-window` with the before/after pane diff), New Tmux Tab and New Tmux Window (menu, Win+T/Win+N with the "What kind of tab" question and its silence keys, the tab bar +), closing a pane, a tab, or a window (with iTerm2's Kill/Hide/Detach questions and silence keys), renames, zoom, swap, and tab reorder; the session options iTerm2 keeps (`@affinities`, `@hidden`, `@origins`, `@per_window_settings`, `@per_tab_settings`, `@buried_indexes`) are written on change; and Shell > tmux > Detach and Force Detach, the double-attach warning, and the detached panes' end follow iTerm2.

**Architecture:** Pure models: `tmux_sizes.rs` (fixed vs variable size mode, the client size as the minimum over tmux tabs, the per-window size commands by version, skipped sizes, the 2.9 status fix, the outstanding-resize counter), `tmux_edits.rs` (a builder for every row of research 3.4, the split pane diff, the escape rules for names and directories, the questions' texts and buttons), and `tmux_persist.rs` (encoders for the session options, the inverses of 8b's decoders). The window side adds the hooks: window resize and font change, divider release, split, new tab and window, close, rename, zoom, swap, tab move, detach.

**Tech Stack:** as milestone 8b.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8-tmux-facts.md` sections 1.5, 3.3, 3.4, 5.1, 9 (8c). Rows: `03-features-and-menus.md` `:65-68` (Detach, Force Detach, New Tmux Window, New Tmux Tab), `:420` (layout sync, the Tandem-to-tmux half), `:421` (detach and reattach), `:422` (rename sync), `:423` (per-window and per-tab settings), `:429` (break pane: 8f); `02-global-settings.md` the Tmux Integration advanced rows for sizes, detach, and the questions.

## Global Constraints

- Everything from milestones 0 to 8b still holds.
- Command strings, texts, buttons, and silence identifiers are iTerm2's (research 3.4 and 1.5), with "iTerm2" read as "Tandem".
- A divider drag moves nothing in tmux until the button comes up; the gateway holds tmux's lines while the drag runs (iTerm2's pauseTokenExecution), so layouts do not fight the drag.
- Sizes: tmux's divider is one cell; Tandem's gap counts as one cell in the size it reports. `disableTmuxWindowResizing` (default YES) keeps Tandem's window size and shrinks tmux's layout to it; `rememberTmuxWindowSizes` (default YES) restores window frames from `@per_window_settings`.
- Session options are written after every change that alters them, debounced so one burst of changes writes once.

## Review Focus

1. Dragging a divider in a tmux tab while tmux sends `%output` and a `%layout-change` from another client. Expect: the drag is not undone mid-drag; after release one `resize-pane` goes out and tmux's next layout wins. Test: Task 3 `drags_send_one_resize_on_release`.
2. Two tmux tabs in one window with different pane layouts on tmux 2.8 (fixed size). Expect: one client size, the smallest that fits both. Test: Task 1 `fixed_size_is_the_minimum`.
3. Closing the last tmux tab of a window when `ClosingTmuxWindowKillsTmuxWindows` is unset. Expect: the four-button question with iTerm2's text; Hide keeps the tmux window and records it in `@hidden`. Test: Task 4 `close_questions_follow_iterm2`.
4. A window name with `"`, `\`, a newline, and `$`. Expect: iTerm2's escaping in `rename-window`. Test: Task 2 `names_escape_like_iterm2`.
5. Esc in the gateway twice while tmux does not answer. Expect: "A previous detach request has not yet been honored. Force detach?" and OK forces it. Test: Task 5 `second_detach_offers_force`.

---

### Task 1: Sizes

**Files:** Create `src/tmux_sizes.rs`; modify `src/app/tmux_tabs_ui.rs` (window resize and font change hooks), `schema/global.toml` (`disableTmuxWindowResizing`, `rememberTmuxWindowSizes`, `tmuxVariableWindowSizesSupported`).

- [ ] Failing tests `fixed_size_is_the_minimum`, `variable_sizes_by_version`, `sizes_skip_unchanged_and_out_of_range`. Commit `tmux sizes`.

### Task 2: Edit commands

**Files:** Create `src/tmux_edits.rs`.

- [ ] Failing tests `commands_follow_iterm2` (every row of research 3.4), `names_escape_like_iterm2`, `split_finds_the_new_pane`, `directories_follow_iterm2`. Commit `tmux edit commands`.

### Task 3: Edits from the window

**Files:** modify `src/app/tmux_tabs_ui.rs`, `src/app/mod.rs` (divider release, split, close pane, new tab, rename, zoom, swap, tab move), `src/tmux_gateway.rs` (pause and resume line handling), `src/app/menu.rs` (Shell > tmux > New Tmux Window and New Tmux Tab), `schema/global.toml` (`NoSyncNewWindowOrTabFromTmuxOpensTmux`, `NoSyncNewWindowFromTmuxOpensTmux`, `NoSyncNewTabFromTmuxOpensTmux`).

- [ ] Failing tests `drags_send_one_resize_on_release`, `new_tab_question_follows_iterm2`. Commit `tmux edits from Tandem`.

### Task 4: Closing and persistence

**Files:** Create `src/tmux_persist.rs`; modify `src/app/tmux_tabs_ui.rs` (close tab and window questions, hide), `src/tmux_controller.rs` (save the options after changes), `schema/global.toml` (`ClosingTmuxTabKillsTmuxWindows`, `ClosingTmuxWindowKillsTmuxWindows`).

- [ ] Failing tests `close_questions_follow_iterm2`, `options_encode_round_trip`. Commit `tmux closing and session options`.

### Task 5: Detach

**Files:** modify `src/app/menu.rs` (Shell > tmux > Detach, Force Detach with iTerm2's enabled rules), `src/app/tmux_ui.rs` (requestDetach, double attach warning "Cannot Attach" [OK, Reveal, Force Detach Other], panes end with "tmux detached"), `schema/global.toml` (`tmuxWindowsShouldCloseAfterDetach`, `AlreadyAttachedToTmuxSession`).

- [ ] Failing tests `second_detach_offers_force`, `double_attach_is_found`. Commit `tmux detach`.

### Task 6: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m8c-checklist.md`. Commit.
