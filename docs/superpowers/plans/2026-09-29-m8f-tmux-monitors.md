# Tandem Milestone 8f: tmux Monitors, User Variables, and TERM

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** What tmux knows about each pane reaches Tandem the way iTerm2 gets it: option monitors (tmux 3.2 subscriptions with `refresh-client -B`, and `display-message` polls before 3.2) for the pane title, the foreground job, and the working directory, and for the window title when tmux has `set-titles on`; the values feed the session, tab, and window titles; OSC 1337 SetUserVar in a tmux pane is saved in the pane option `@uservars` and read back on attach; and a pane whose TERM is screen-like (a tmux pane takes tmux's `default-terminal`, else "screen") converts italics to reverse video as iTerm2 does.

**Architecture:** `tmux_monitor.rs` is the pure monitor (iTerm2's iTermTmuxOptionMonitor): the command strings, the escaping, the subscription and poll state, and the working-directory rate rule. The controller owns every monitor of its connection, keyed by what it watches, routes their replies and `%subscription-changed` lines, and emits `Action::PaneInfo` and `Action::WindowTitle`. `tmux_persist.rs` gains the `u_` user-variable encoding. `screenterm.rs` is the byte filter for screen-like TERM. The app stores the values on `TmuxIo` and uses them in `session_title` and the tab title.

**Tech Stack:** as milestone 8e.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8f-tmux-facts.md` sections 1, 2, 8, 9, 10, 12, 13 (and "Recommended split", 8f). Rows: `03-features-and-menus.md` `:426` (option monitor), `:427` (copy mode bridging), `:434` (local vs remote), `:435` (user variables), `:417` (the integration row); `02-global-settings.md` `:668-669` (italics), `:941` (pollForTmuxForegroundJob).

## Global Constraints

- Everything from milestones 0 to 8e still holds.
- Commands are iTerm2's byte for byte: `refresh-client -B '<id>:<target>:<format>'`, `refresh-client -B '<id>'`, `display-message -t '<target>' -p '<format>'`, `set -p -t %<pane> @uservars "<encoded>"`. Formats escape `\` as `\\`, then `'` as `\'`.
- Subscription ids are `it2_<n>` from one process-wide counter.
- A poll that errors stops that monitor for good (iTerm2's outstanding flag stays set).
- On tmux before 3.2 the pane monitors poll only on events: OSC 0/1/2 (title and job), a key press and line feeds (working directory, rate-limited), and a timer only for the job with `pollForTmuxForegroundJob`.
- The job monitor exists only when the profile's Icon is not "No Icon" (iTerm2 `PS:10098-10100`).
- tmux working directories are remote paths: they show in titles only; they never become the directory of a new local session.

## Review Focus

1. tmux 3.4: a pane's title changes in tmux. Expect: one subscription per pane monitor and `%subscription-changed` updates the title; no `display-message` after the first. Test: Task 2 `subscriptions_carry_pane_titles`.
2. tmux 3.1: the job monitor on, `pollForTmuxForegroundJob` off. Expect: one poll at install and one per OSC title change, no timer. Test: Task 2 `old_tmux_polls_on_events`.
3. A poll that errors. Expect: that monitor never polls again, others go on. Test: Task 1 `a_failed_poll_stops_the_monitor`.
4. A SetUserVar value with `=` and non-ASCII text, then a reattach. Expect: the same variables come back. Test: Task 3 `user_vars_round_trip`.
5. SGR `3;23;1` in a screen-like pane split across two reads. Expect: `7;27;1`; `3:1` (a sub-parameter) is kept. Test: Task 4 `italics_become_reverse_video`.

---

### Task 1: The monitor model

**Files:** Create `src/tmux_monitor.rs`; add `pub mod tmux_monitor;` to `src/lib.rs`.

**Produces:** `escape_format(&str) -> String`; `next_subscription_id() -> String`; `subscribe_command(id, target, format)`, `unsubscribe_command(id)`, `poll_command(target, format)`; `struct Monitor { target, format, id: Option<String>, initialized, valid, outstanding, value: Option<String>, timer: Option<f64> }` with `Monitor::new(target, format, subscribe: bool, timer: Option<f64>) -> (Monitor, Vec<Cmd>)` where `enum Cmd { Subscribe(String), Poll(String) }`, `update_once(&mut self) -> Option<String>` (the poll command, or None), `fetched(&mut self, Option<&str>) -> Option<String>` (a new value), `subscribed(&mut self, ok: bool)`, `changed(&mut self, value: &str) -> Option<String>`, `unsubscribe(&self) -> Option<String>`, `due(&self, now) -> bool`; `struct CwdGate` with `line_feed(now) -> bool`, `key_pressed()`, `known()`.

- [ ] Failing tests:
  - `commands_follow_iterm2`: `escape_format("a'b\\c")` is `a\'b\\\\c`; `subscribe_command("it2_3", "%1", "#{pane_title}")` is `refresh-client -B 'it2_3:%1:#{pane_title}'`; `poll_command("%1", "#{pane_title}")` is `display-message -t '%1' -p '#{pane_title}'`; two `next_subscription_id()` calls differ and start with `it2_`.
  - `subscriptions_replace_polls`: a subscribing monitor sends Subscribe then Poll; after `subscribed(true)` `update_once` is None; `changed("x")` gives Some("x"), again "x" gives None. `subscribed(false)` (refused) keeps polls working and `unsubscribe()` is None.
  - `a_failed_poll_stops_the_monitor`: `fetched(None)` then `update_once()` is None forever.
  - `working_directory_polls_are_rate_limited`: before a directory is known a line feed polls; then at most once a second and only after a key press.
- [ ] Implement. Commit `tmux monitor model`.

### Task 2: Monitors in the controller

**Files:** Modify `src/tmux_controller.rs`, `src/tmux_handshake.rs` (nothing new is sent; `set_titles` is read by the controller through `version_known`).

**Produces:** `Controller::watch_pane(pane, job: bool, gw)`, `unwatch_pane(pane, gw)` (called from `forget_pane`, which gains `gw`), `pane_event(pane, PaneEvent, gw)` with `enum PaneEvent { Title, Key, LineFeed }`, `set_titles: bool` (set by `version_known`), the window title monitor installed when a window opens and removed when it closes, `Action::PaneInfo { pane, what: Info, value }` with `enum Info { Title, Job, Cwd }`, `Action::WindowTitle(window, value)`; `Prefs::poll_job: bool`; `Req::Subscribed(Key)`, `Req::Fetched(Key)`; `%subscription-changed` routed by id. `poll(now, gw)` also runs job timers. Output notes count line feeds for `pane_event(LineFeed)` themselves.

- [ ] Failing tests:
  - `subscriptions_carry_pane_titles` (3.4 rig, `opened_34`): `watch_pane(0, true)` sends three `refresh-client -B` plus three `display-message` (title, job, cwd); answers give three `PaneInfo`; `%subscription-changed it2_N $0 @0 0 %0 : vim` gives `PaneInfo { Job, "vim" }`; `pane_event(Title)` sends nothing.
  - `old_tmux_polls_on_events` (3.1 rig): `watch_pane(0, true)` sends two polls (title, job) and one cwd poll; `pane_event(Title)` after the replies sends the title and job polls again; `poll()` a second later sends nothing; with `prefs.poll_job` it sends the job poll.
  - `window_titles_follow_set_titles` (3.4 rig with set_titles): the window opening subscribes `@0` to `#{T:set-titles-string}`; its value gives `WindowTitle(0, ..)`; `%window-close @0` unsubscribes.
- [ ] Implement. Commit `tmux monitors in the controller`.

### Task 3: User variables over tmux

**Files:** Modify `src/tmux_persist.rs` (`user_vars_value(&BTreeMap<String,String>) -> String`, `parse_user_vars(&str) -> BTreeMap<String,String>`, `user_vars_command(pane, &str)`), `src/tmux_controller.rs` (`Part::UserVars` in the capture list, `PaneOpen::user_vars`, `sync_user_vars(pane, &BTreeMap, gw)` that sends only on change and on 3.1+).

- [ ] Failing tests:
  - `user_vars_round_trip` (persist): `{"user.a": "x=y", "user.b": "é"}` encodes as `u_` + lowercase hex of `user.a=x=y\0user.b=é`; decode gives the map back; a value without `u_` is plain text; parts without `=` are skipped; an odd trailing hex digit is ignored.
  - `user_vars_are_saved_and_read` (controller, 3.1 rig): the capture reply for `@uservars` fills `PaneOpen::user_vars`; `sync_user_vars` sends `set -p -t %0 @uservars "u_..."` once, nothing for the same map; on a 3.0 controller nothing.
- [ ] Implement. Commit `tmux user variables`.

### Task 4: TERM and italics

**Files:** Create `src/screenterm.rs` (`screen_like(term, bugwards) -> bool`, `struct Filter { italics: bool }` with `feed(&mut self, &[u8]) -> Vec<u8>` holding a partial CSI across reads); modify `src/pane.rs` (an optional filter applied to screen bytes in `pump`), `schema/global.toml` (Advanced: `translateScreenToXterm` default true, `convertItalicsToReverseVideoForTmux` default true, `convertItalicsToReverseVideoForTmuxBugwardsCompatible` default false, `pollForTmuxForegroundJob` default false, with iTerm2's texts).

- [ ] Failing tests:
  - `screen_terms_are_found`: "screen-256color" yes; "tmux-256color" only with bugwards; "xterm-256color" no.
  - `italics_become_reverse_video`: `ESC[3;23;1m` becomes `ESC[7;27;1m`; split as `ESC[3` + `;23m` gives the same; `ESC[3:1m` kept; `ESC[?5h` and `ESC[?5l` dropped, `ESC[?5;1h` becomes `ESC[?1h`; with `italics: false` SGR stays but `?5` is still dropped; other bytes pass through.
- [ ] Implement. Commit `screen TERM: italics as reverse video`.

### Task 5: App wiring, parity, acceptance

**Files:** Modify `src/pane.rs` (`TmuxIo` gains `title`, `job`, `cwd` cells), `src/app/tmux_tabs_ui.rs` (watch panes on open with the profile's Icon, unwatch on forget, `PaneInfo` and `WindowTitle` actions, title change and key events, user variables applied on open and synced after output, the filter set from `default_terminal`), `src/app/jobs_ui.rs` (a tmux pane's title parts: the pane title as the session name when non-empty, the job, the directory), `src/app/tmux_ui.rs` (`Prefs::poll_job`), parity rows and `docs/acceptance/m8f-checklist.md`.

- [ ] Failing test `tmux_pane_titles_use_monitors` (jobtitle): `TitleParts::tmux_pane_title` non-empty replaces the session name part.
- [ ] Wire. Local vs remote: `serverIsLocal` stays false on Windows (a WSL tmux pid is never a Windows process); the row says so. Copy mode: `copy-mode -q` on attach is already sent (8a); the row says iTerm2 does no more. Commit `tmux monitors: titles, jobs, directories, user variables` and `docs: m8f parity and checklist`.
