# Tandem Milestone 7b: Closing, Quitting, Startup, and Undo Close

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's closing and startup rules: the session, tab, and window close questions with their exact texts (Confirm closing multiple sessions, Prompt Before Closing, the job lists), the quit question with its reasons ("Quit Tandem?", Confirm Quit, Even if there are no windows), Quit when all windows are closed, Undo Close (Shift+Cmd+T) with Session Close Undo Timeout, `moveLeftAfterClosingTab`, and the Startup settings (Open profiles window, the window restoration policy, Always open at least one terminal window).

**Architecture:** `close_rules.rs` decides whether a close asks and builds iTerm2's texts (the subject bodies from `closeConfirmationBodyForSubject`, the quit reasons from `iTermPromptOnCloseReason`); `undo_close.rs` keeps the undo stack (entries for a session, a tab, or a window, each with a deadline); `startup.rs` turns the Startup settings into the launch actions. The window code asks through the existing MessageBox and keeps closed panes alive, paused, in the undo stack until their deadline.

**Tech Stack:** as milestone 7a.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7-iterm2-facts.md` section 2, plus `PseudoTerminal.m:2155-2360` (close bodies), `:2360-2402` (tab rule), `:2737-2760` (session rule), `:4771-4795` and `:14305-14313` (window rule), `iTermApplicationDelegate.m:994-1063` (quit), `iTermPromptOnCloseReason.m:221-231`. Rows: `02-global-settings.md` `:15-29`, `:549`, `:699`, `:726`; `01-profile-settings.md` `:247`; `03-features-and-menus.md` `:54`.

## Global Constraints

- Everything from milestones 0 to 7a still holds.
- A session close with one pane in its tab is a tab close. A session close asks only for a Prompt Before Closing reason; a tab or window close also asks when more than one running session closes and `OnlyWhenMoreTabs` (Confirm closing multiple sessions, default on) is on.
- Titles: `Close session "N"?` (curly quotes), `Close tab #N?`, `Close Window #N?`. Bodies: "This session/tab/multi-pane tab/window is running X." / "... is running the following jobs: a, b, and c." / "... the following jobs: <ten>, plus N others." / "This ... will be closed." Repeated names read `2 instances of "vim"`; names sort case-insensitively.
- Quit: title "Quit Tandem?", body "All sessions will be closed.", then "You are being prompted because:" and the reasons (Prompt Before Closing reasons per window; `"Settings > General > Closing > Confirm Quit Tandem" is enabled and there is at least one terminal window.` when PromptOnQuit (default on) and a window exists, or the "Even if there are no windows" form; `"Settings > General > Closing > Confirm closing multiple sessions" is enabled.` for more than one session). A quit caused by closing the last window skips the Confirm Quit reason.
- Undo Close: Shift+Cmd+T; the profile's Session Close Undo Timeout (default 5 s, 0 turns it off); a closed tab or window is one entry; exited sessions are not undoable; the undo keeps the shell running, not drawn, until the deadline.
- `moveLeftAfterClosingTab` (default off) selects the tab to the left after a close.

## Review Focus

1. Closing a tab of three panes where one shell has exited. Expect: "more than one running session" counts two, and the body lists the jobs of the running ones. Test: Task 1 `tab_close_asks_like_iterm2`.
2. Undo Close after the deadline passed. Expect: nothing comes back and the shells were ended. Test: Task 3 `entries_expire`.
3. Undo Close of a tab whose window is gone. Expect: it comes back in a new window. Test: Task 3 `undo_targets`.
4. Quit with Confirm Quit off and one session. Expect: no question. Test: Task 2 `quit_reasons`.
5. A window closed while its undo entry waits, then Tandem quits. Expect: the waiting shells end with Tandem (no stray processes). Test: Task 3 `quit_drops_the_stack`.

---

### Task 1: Close rules

**Files:** Create `src/close_rules.rs`; modify `src/app/close_ui.rs`, `src/app/mod.rs` (active tab after a close), `schema/global.toml` (`OnlyWhenMoreTabs`, `moveLeftAfterClosingTab`).

**Interfaces:** `Subject { Session, Tab, MultiPaneTab, Window }`; `body(subject, names) -> String`; `counted(names) -> Vec<String>`; `must_ask(kind, running, has_reason, confirm_multiple) -> bool`; `active_after_close(active, closed, remaining, move_left) -> usize`.

- [ ] Failing tests `bodies_follow_iterm2`, `counted_names`, `tab_close_asks_like_iterm2`, `move_left_after_close`. Commit `Close questions like iTerm2`.

### Task 2: Quit

**Files:** modify `src/close_rules.rs` (`quit_reasons`, `quit_text`), `src/app/close_ui.rs`, `src/app/menu.rs`, `src/app/mod.rs` (the last window), `schema/global.toml` (`PromptOnQuit`, `PromptOnQuitEvenIfThereAreNoWindows`, `QuitWhenAllWindowsClosed`, `NeverBlockSystemShutdown`).

**Interfaces:** `quit_reasons(profile_reasons, windows, sessions, prompt_on_quit, even_if_none, confirm_multiple, last_window_closed) -> Vec<String>`; `quit_text(reasons) -> String`.

- [ ] Failing test `quit_reasons`. Commit `Quit like iTerm2`.

### Task 3: Undo Close

**Files:** Create `src/undo_close.rs`, `src/app/undo_ui.rs`; modify `src/app/mod.rs` (close paths hand panes to the stack), `src/app/menu.rs` (Shell > Undo Close), `src/commands.rs`.

**Interfaces:** `Stack<T>` with `push(entry, deadline)`, `pop(now) -> Option<T>`, `expire(now) -> Vec<T>`, `clear() -> Vec<T>`; `Entry { Session { pane, tab_id, window }, Tab { tab, index, window }, Window { tabs, number } }`; `target(entry, windows_alive, tab_alive) -> Target { SameTab, NewTab, SameWindow, NewWindow }`.

- [ ] Failing tests `entries_expire`, `undo_pops_the_newest`, `undo_targets`, `quit_drops_the_stack`. Commit `Undo Close`.

### Task 4: Startup

**Files:** Create `src/startup.rs`; modify `src/app/mod.rs` (`resumed`), `schema/global.toml` (`OpenBookmark`, `OpenArrangementAtStartup`, `OpenNoWindowsAtStartup`, `AlwaysOpenWindowAtStartup`, `openNewWindowAtStartup`).

**Interfaces:** `plan(prefs: StartupPrefs, has_default_arrangement, has_hotkey_windows) -> Actions { profiles_window, arrangement, window }` with iTerm2's order and suppressions; the settings help texts.

- [ ] Failing test `startup_follows_the_policy`. Commit `Startup settings`.

### Task 5: Parity and acceptance

- [ ] Update the rows, recount, write `docs/acceptance/m7b-checklist.md`. Commit.
