# Tandem Milestone 9c: Shell History (Commands and Recent Directories)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's shell history (ShellHistoryController): every command shell integration reports is kept per host (`user@host`) with its uses, directory, and exit code, and every directory the shell reports is kept with its use count and time; commands rank by iTerm2's frecency, directories by iTerm2's order; data older than 90 days and directories past the newest 1000 go at start; with "Save copy/paste and command history to disk" it is saved. Session > Open Command History... (Shift+Win+;) lists the host's commands that start with what is typed at the prompt and inserts the rest; Session > Open Recent Directories... (Alt+Win+/) lists the host's directories and inserts the chosen one, shell-escaped, at a prompt; both show iTerm2's "About Shell Integration" alert before shell integration was ever used.

**Architecture:** `shell_history.rs` is the pure store (hosts, commands with uses, directories, frecency, ordering, limits, JSON). The app feeds it from each pane's `Shell` (new command records and directory changes), saves it, and opens a list popup (`list_popup_ui.rs`, the paste history popup made general: title, two columns, the shared filter).

**Tech Stack:** as milestone 9b.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` sections 3.6 (Command History, Recent Directories, the common popup), 3.7. Rows: `03-features-and-menus.md` Session > Open Command History..., Open Recent Directories..., the command history and recent directories features; `02-global-settings.md` `:142` (the command half), `commandHistoryUsePower`, `commandHistoryAgePower`.

## Global Constraints

- Everything from milestones 0 to 9b still holds.
- Frecency: `pow(log10(10 + uses), 3) / pow(log10(10 + age_seconds), 1)` (the powers are `CommandHistoryUsePower` and `CommandHistoryAgePower`).
- Directory order: starred first, then higher `floor(log2(uses))`, then newer last use, then more uses.
- Lookups return at most 200.
- The file is `%APPDATA%\Tandem\shell_history.json`, written only while `SavePasteHistory` is on.
- Open Autocomplete... and View > Auto Command Completion are milestone 9d.

## Review Focus

1. The same command run twice: one entry with two uses. Test: Task 1 `commands_are_kept_like_iterm2`.
2. Prefix "git c" with "git commit" used often long ago and "git checkout" once now: frecency decides. Test: Task 1.
3. A directory reported twice in a row: one use. Test: Task 1 `directories_are_kept_like_iterm2`.
4. 91-day-old uses and 1001 directories: pruned at start. Test: Task 1 `old_history_is_pruned`.
5. Open Command History with "gi" typed, choosing "git status": only "t status" is sent. Test: Task 1 `remainders_follow_the_typed_prefix`.

---

### Task 1: The store

**Files:** Create `src/shell_history.rs`; add to `src/lib.rs`.

**Produces:** `Store` with `record_command(host, command, dir, time)`, `record_code(host, command, code)`, `record_dir(host, path, time)`, `commands(host, prefix, now, powers) -> Vec<(String, f64 /*last use*/)>`, `dirs(host) -> Vec<Dir>`, `toggle_star(host, path)`, `prune(now)`, `erase_commands()`, `erase_dirs()`, `ever_used() -> bool`, `to_json()`, `from_json()`; `frecency(uses, age, powers) -> f64`; `remainder(command, typed) -> String`; `host_key(user, host) -> String`.

- [ ] Failing tests `commands_are_kept_like_iterm2`, `directories_are_kept_like_iterm2`, `old_history_is_pruned`, `remainders_follow_the_typed_prefix`, `history_round_trips`.
- [ ] Implement. Commit `shell history store`.

### Task 2: App wiring, popups, parity, acceptance

**Files:** Create `src/list_popup_ui.rs`, `src/app/shell_history_ui.rs`; modify `src/marks.rs` (the command text being typed), `src/app/mod.rs` (feed after output, load at start), `src/app/menu.rs` (`openCommandHistory:`, `openDirectories:`), `src/commands.rs`, `schema/global.toml` (`CommandHistoryUsePower` 3, `CommandHistoryAgePower` 1, `NoSyncCommandHistoryHasEverBeenUsed`), parity rows, `docs/acceptance/m9c-checklist.md`.

- [ ] Wire. Commit `shell history and its popups` and `docs: m9c parity and checklist`.
