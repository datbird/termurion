# Tandem Milestone 8e: tmux Dashboard, Burying, Settings, and tmux Keys

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's tmux Dashboard (Shell > tmux > Dashboard, Shift+Ctrl+Win+Return): a "tmux Dashboard" window with the connection popup, the sessions table (the attached one checked, rename, +, -, Attach, Detach), the windows table (search, rename, open ones full and hidden ones dimmed, +, -, Open in Tabs, Open in Windows, Hide, double-click), the "Don't open windows when attaching if there are at least this many windows:" limit, and "Open dashboard when attaching if there are hidden windows"; Session > Bury Session and Buried Sessions (deferred from M7) with AutoHideTmuxClientSession burying the gateway and its first-time notification; the tmux settings rows; key action 65 "Send tmux Command"; and tmux's prefix-table bindings as Leader bindings.

**Architecture:** `tmux_dashboard.rs` is the pure model (session and window rows, the button rules, iTerm2's command strings, the new session name, the window filter). `tmux_dashboard_ui.rs` is the egui window, like Find Globally. `app/tmux_dashboard_app.rs` connects it to the controllers. Burying keeps a pane alive outside every tab (`App::buried`). `tmux_prefix.rs` turns `list-keys` lines into bindings.

**Tech Stack:** as milestone 8d.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8-tmux-facts.md` sections 5.1, 5.2, 5.3, 6, 7 (prefix bindings). Rows: `03-features-and-menus.md` `:70` (Dashboard), `:230-231` (Bury Session, Buried Sessions), `:373` (OSC 1337 Disinter), `:418` (session dashboard), `:424` (hidden windows); `02-global-settings.md` `:165-166` (TmuxDashboardLimit, AutoHideTmuxClientSession), `:425` (Send tmux Command).

## Global Constraints

- Everything from milestones 0 to 8d still holds.
- Texts, buttons, and commands are iTerm2's (research 5.2, 5.3).
- The windows list of a session is read with `list-windows -F <detailed> -t "$S"`, 1.5 s after it is asked for, once per request.

## Review Focus

1. Two tmux connections, the Dashboard on the second: its actions go to the connection the popup names. Test: Task 1 `commands_follow_iterm2` plus the checklist.
2. New Session when "New Session" and "New Session 2" exist. Expect: "New Session 3". Test: Task 1 `new_session_names_are_unique`.
3. Open in Tabs with an open window selected. Expect: the button is off. Test: Task 1 `buttons_follow_iterm2`.
4. Burying the gateway, then detaching. Expect: the gateway comes back before "Detached" prints. Test: Task 3 `buried_gateways_return_on_detach`.
5. `list-keys` with `bind-key -T prefix C-c new-window` and `bind-key -T prefix : command-prompt`. Expect: Leader, Ctrl+C sends `new-window`; the command prompt is not bound. Test: Task 4 `prefix_bindings_follow_iterm2`.

---

### Task 1: The dashboard model

**Files:** Create `src/tmux_dashboard.rs`.

- [ ] Failing tests `commands_follow_iterm2`, `new_session_names_are_unique`, `buttons_follow_iterm2`, `window_filter_matches_names`. Commit `tmux dashboard model`.

### Task 2: The Dashboard window

**Files:** Create `src/tmux_dashboard_ui.rs`, `src/app/tmux_dashboard_app.rs`; modify `src/tmux_controller.rs` (sessions list refresh, list windows of a session, open a hidden window, rename, kill, new, attach, unlink), `src/app/menu.rs` (openDashboard:), `src/commands.rs`, `schema/global.toml` (`openTmuxDashboardIfHiddenWindows`).

- [ ] Failing test `dashboard_lists_follow_the_session` (controller). Commit `tmux dashboard`.

### Task 3: Burying

**Files:** modify `src/app/mod.rs` and a new `src/app/bury_ui.rs` (Session > Bury Session, Session > Buried Sessions submenu, restore), `src/app/tmux_ui.rs` (AutoHideTmuxClientSession, "Session Buried" once with `NoSyncAutoBurialReveal`, restore on disconnect), OSC 1337 Disinter.

- [ ] Failing test `buried_gateways_return_on_detach` (pure bury list model). Commit `Bury sessions`.

### Task 4: tmux keys

**Files:** Create `src/tmux_prefix.rs`; modify key action 65 (`src/app/actions.rs`), the key map lookup for tmux panes.

- [ ] Failing test `prefix_bindings_follow_iterm2`. Commit `tmux keys: Send tmux Command and prefix bindings`.

### Task 5: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m8e-checklist.md`. Commit.
