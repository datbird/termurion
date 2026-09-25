# Tandem Milestone 8g: tmux Clipboard, Clients, Reports, and Tab Labels

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The tmux paste buffer mirrors to the Windows clipboard as iTerm2 does (TmuxSyncClipboard, and its "_Always"/"_Never" offer); with several clients on one tmux 3.6+ session exactly one Tandem answers OSC 4 color queries and OSC 52 clipboard reads (iTerm2's client tracker election and the get-clipboard/set-clipboard rules); on tmux 3.4+ each pane's foreground and background colors go to tmux with `refresh-client -r` so tmux answers OSC 10/11 itself; and a tmux tab's label follows iTerm2's rule (the set-titles string, else the session title with the pane title as its name, else the prefix and the window name).

**Architecture:** `tmux_clients.rs` is the pure part: the client tracker, the clipboard option levels and the OSC 52/OSC 4 duty rules, the tmux report escaping and `refresh-client -r` builder, the extraction of OSC 4 answers from a reply, and the tab label rule. The controller runs the tracker and the two session-level clipboard monitors (on the 8f monitor code), fetches paste buffers, and sends color reports. The app mirrors the buffer, shows the offer, lets OSC 4 answers out of tmux panes only when this client is responsible, gates OSC 52 reads the same way, and draws tmux tab labels by the rule.

**Tech Stack:** as milestone 8f.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8f-tmux-facts.md` sections 3, 4, 11, and 1.4 (tab titles, with `TAB:810-840` and `STF:276-290`). Rows: `03-features-and-menus.md` `:431` (client tracking), `:432` (OSC responder election), `:417`; `02-global-settings.md` `:172` (TmuxSyncClipboard), `:173` (Ask Each Time).

## Global Constraints

- Everything from milestones 0 to 8f still holds.
- Commands are iTerm2's: `display-message -p '#{client_name}'`, `list-clients -t '$S' -F '#{client_name}<TAB>#{client_control_mode}'`, `show-buffer -b bufferN`, `refresh-client -r "%P:<escaped>"`.
- Only `bufferN` names are fetched (the tmux proto already filters).
- A tmux pane never answers any report except OSC 4 (3.6+, responsible) and OSC 52 reads (by the rule); tmux answers the rest.
- The tracker is made once per connection with the first session (iTerm2's quirk); the clipboard monitors are made again on each session change.

## Review Focus

1. Two Tandem clients (control mode) and no plain client on tmux 3.6: only the one with the smaller client name answers. Test: Task 1 `the_smallest_control_client_answers`.
2. A plain terminal client attached too. Expect: no Tandem answers. Test: Task 1 same test.
3. tmux 3.7 with `set-clipboard on` and `get-clipboard request`: OSC 52 reads are answered; `get-clipboard off` and `set-clipboard on`: not. Test: Task 1 `clipboard_duty_follows_iterm2`.
4. A reply holding a DA answer and an OSC 4 answer: only the OSC 4 answer goes to tmux. Test: Task 1 `only_osc4_answers_leave_tmux_panes`.
5. `%paste-buffer-changed buffer0` with TmuxSyncClipboard off and never answered: the offer shows once; answering Never stops it for good. Test: Task 3 `paste_buffer_offer_is_answered_once` (app-level rule in `tmux_clients::offer_paste_sync`).

---

### Task 1: The clients model

**Files:** Create `src/tmux_clients.rs`; add it to `src/lib.rs`.

**Produces:** `struct Tracker { me, session }` with `new(me, session)`, `list_command() -> String`, `listed(&mut self, reply)`, `detached(&mut self, name)`, `responsible() -> bool`; `get_level(&str) -> u8` (both 3, request 2, buffer 1), `set_level(&str) -> u8` (on 2, external 1); `answers_osc4(version, responsible) -> bool`; `answers_osc52(version, responsible, get, set) -> bool`; `escape_for_tmux(&str) -> String`; `color_report(osc: u8, rgb: (u8, u8, u8)) -> String`; `report_command(pane, report) -> String`; `osc4_answers(reply: &[u8]) -> Vec<u8>`; `offer_paste_sync(sync_on: bool, answered: bool, bar_showing: bool) -> bool`; `tab_label(set_titles: Option<&str>, pane_title: &str, session_title: &str, window_name: &str, prefix: &str) -> String`.

- [ ] Failing tests: `the_smallest_control_client_answers`, `clipboard_duty_follows_iterm2`, `reports_escape_for_tmux` (`refresh-client -r "%3:\033]11;rgb:1e1e/1e1e/1e1e\033\\"`), `only_osc4_answers_leave_tmux_panes` (BEL and ST forms, a split `ESC ]4` at the end is dropped), `paste_buffer_offer_is_answered_once`, `tmux_tab_labels_follow_iterm2`.
- [ ] Implement. Commit `tmux clients model`.

### Task 2: The controller

**Files:** Modify `src/tmux_controller.rs`.

**Produces:** On `open_initial` with tmux 3.6+: `display-message -p '#{client_name}'` (once per connection) then the list; `%client-session-changed` relists, `%client-detached` removes; the session-level monitors `#{T:get-clipboard}` (3.7+) and `#{T:set-clipboard}` (3.6+), made again on each session change; `answers_osc4()`, `answers_osc52()`; `Action::PasteBuffer(String)` on `%paste-buffer-changed`; `fetch_buffer(name, gw)` sending `show-buffer -b name` whose non-empty reply is `Action::Clipboard(String)`; `color_reports(pane, fg, bg, gw)` sending both `refresh-client -r` commands on 3.4+.

- [ ] Failing tests (3.6 and 3.7 rigs): `clients_elect_one_responder`, `clipboard_levels_come_from_monitors`, `paste_buffers_are_fetched`, `colors_are_reported_on_34`.
- [ ] Implement. Commit `tmux clients in the controller`.

### Task 3: App wiring, parity, acceptance

**Files:** `src/pane.rs` (`TmuxIo::answers_osc4` cell; `pump` sends `osc4_answers` of a tmux pane's reply when it is set), `src/app/clipboard_ui.rs` (an OSC 52 read in a tmux pane is dropped unless `answers_osc52`), `src/app/tmux_tabs_ui.rs` (PasteBuffer: mirror or offer; Clipboard: set the Windows clipboard; the offer bar "The tmux paste buffer was updated. Would you like to mirror it to the local clipboard from now on?" [Always, Never]; `answers_osc4` pushed to the panes after each action batch; color reports when a pane opens), `src/app/mod.rs` (`refresh_panes` resends tmux color reports), `src/app/requests_ui.rs` (`tab_look` uses `tab_label` for tmux tabs), `schema/global.toml` (General > tmux `TmuxSyncClipboard` "Mirror tmux paste buffer to local clipboard", default false), parity rows, `docs/acceptance/m8g-checklist.md`.

- [ ] Wire. Commit `tmux clipboard, clients, reports, and tab labels` and `docs: m8g parity and checklist`.
