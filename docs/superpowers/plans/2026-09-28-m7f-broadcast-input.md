# Tandem Milestone 7f: Broadcast Input

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's broadcast input: Shell > Broadcast Input (Send Input to Current Session Only, All Panes in All Tabs, All Panes in Current Tab, Toggle Broadcast Input to Current Session, Show Background Pattern Indicator, Current Session is Broadcast Source), the context menu's Toggle Broadcasting Input, the warning with "Suppress this message permanently", typed input, pastes, drops, and key actions copied to every session of the domain as the sender's bytes, "Send Text without Broadcasting", the red stripe pattern and the top-right broadcast indicators, and the broadcast-aware Clear Buffer, Reset, and Scroll to End.

**Architecture:** `broadcast.rs` ports iTerm2's `iTermBroadcastInputHelper` as a pure model per window (raw mode and the mode the menu sees, per-tab flags, custom domains, sources, and every transition). The window keeps it in its `WinSlot` (swapped in `swap_live`). One entry point, `App::user_write(sender, bytes)`, sends user input to the sender's domain; every typed-input path goes through it and the local-only paths (mouse and focus reports, triggers, uploads) do not. Drawing adds a generated 80x80 stripe tile as a repeating image over cell backgrounds and under text, and the broadcast glyphs to the top-right indicators.

**Tech Stack:** as milestone 7e.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7f-broadcast-facts.md` (all sections; it corrects `m7-iterm2-facts.md` section 6). Rows: `03-features-and-menus.md` `:58-63`, `:747-755`, `:733`; `02-global-settings.md` `:436`, `:807`, `:899`, `:967`, `:986`.

## Global Constraints

- Everything from milestones 0 to 7e still holds.
- Menu items, chords, check marks, and enabled rules exactly as the research's table 2.1 (Cmd is Win, Opt is Alt); the context menu item "Toggle Broadcasting Input" after the Edit Session group, acting on the clicked pane.
- The warning shows only when the menu's mode goes from off to on: its title text from research section 3 (pane counts), OK and Cancel, "Suppress this message permanently" stored as `NoSyncSuppressBroadcastInputWarning`.
- Copied: key presses and releases as the sender's encoded bytes, pastes (bracketed by the sender's mode), drops, Send Text, Escape Sequence, Hex Code, vim text, backspace actions, alt-click arrows. Never copied: mouse reports, focus reports, trigger Send Text, uploads, Buffer Input release, "Send Text without Broadcasting" (action 76).
- Stripes: off by default (`Use background pattern indicator`), red (255, 0, 0) at alpha 64, "/" bands at 45 degrees, 40 px on and 40 px off per row, drawn on every pane that receives. Indicators: sends, and receive only, hidden by `disableTopRightIndicators`.
- Broadcast state is not saved in arrangements or restoration.

## Review Focus

1. Current Tab broadcast in tab A, then switch to tab B and choose Send Input to Current Session Only. Expect: iTerm2's quirk, nothing changes and A keeps broadcasting inside itself. Test: Task 1 `quirks_follow_iterm2`.
2. A paste into a broadcasting pane while one target has bracketed paste on and another does not. Expect: every target gets the sender's bytes (bracketed by the sender's mode). Test: Task 2 `pastes_carry_the_senders_bytes`.
3. A target pane closes or moves to another window while it is in a custom domain. Expect: it drops out; a domain left with one live pane keeps working like iTerm2 (only itself). Test: Task 1 `targets_skip_gone_panes`.
4. Typing into a pane that is in a domain with a different pane as the broadcast source. Expect: its input stays local. Test: Task 1 `sources_limit_senders`.
5. A mouse report or focus report in a broadcasting pane. Expect: it goes to that pane only. Test: Task 2 `local_writes_stay_local` (the routing table).

---

### Task 1: The broadcast model

**Files:** Create `src/broadcast.rs`; modify `src/lib.rs`.

**Interfaces:** `Mode { Off, AllPanes, AllTabs, Custom }`; `Broadcast { raw, domains: Vec<BTreeSet<u64>>, sources: BTreeSet<u64> }`; a `View` of the window (`tabs: Vec<(tab id, flag, Vec<pane>)>`, current tab, current pane, exited panes); `mode(view)`, `domain_containing(view, pane)`, `current_domain(view)`, `should_broadcast_to(view, pane, sender)`, `set_mode(view, mode) -> Change { Asks(title) | Done }` with `confirm_set_mode`, `toggle_session(view, pane)`, `toggle_source(view, pane)`, `targets(view, sender) -> Vec<u64>`, `receives(view, pane)`, `sends(view, pane)`, `warning_title(view, mode)`; tab flags are returned as edits for the caller to apply.

- [ ] Failing tests `modes_toggle_like_iterm2`, `toggle_session_builds_domains`, `sources_limit_senders`, `quirks_follow_iterm2`, `targets_skip_gone_panes`, `warning_titles_count_panes`. Commit `Broadcast model`.

### Task 2: Routing

**Files:** modify `src/app/mod.rs` (`user_write`; `to_terminal`, alt-click arrows), `src/app/keyproto_ui.rs`, `src/app/actions.rs` (`send_to_focused` through `user_write`; action 76 local; Scroll to End on every target), `src/pane.rs` (`paste_payload` public), paste and drop sites, `src/app/windows_ui.rs` (`WinSlot.broadcast`, `swap_live`), `struct Tab` (`broadcasting`).

**Interfaces:** `App::user_write(sender, bytes)`; `App::user_paste(sender, text)`; `broadcast::Route { Broadcast, Local }` and `route(kind) -> Route` over the write kinds (key, release, paste, drop, send-text, no-broadcast text, mouse report, focus report, trigger, upload, buffered release).

- [ ] Failing tests `local_writes_stay_local`, `pastes_carry_the_senders_bytes`. Commit `Broadcast typed input`.

### Task 3: Menus, warnings, and side commands

**Files:** modify `src/app/menu.rs` (the six items, checks, enabled rules), `src/commands.rs`, `src/app/seltools_ui.rs`, `src/app/semantic_ui.rs` (context menu), `src/app/broadcast_ui.rs` (new: the warning with the suppress box, Clear Buffer and Reset in all broadcast sessions with their three-button questions and silence keys), `schema/global.toml` (`NoSyncSuppressBroadcastInputWarning`, `NoSyncClearAllBroadcast`, `NoSyncResetAllBroadcast`, `Use background pattern indicator`).

**Interfaces:** `broadcast::clear_question(n) -> (heading, text, buttons)`, `reset_question(n)`.

- [ ] Failing test `clear_and_reset_questions`. Commit `Broadcast menus`.

### Task 4: Indicators

**Files:** modify `src/scene.rs` (`stripe_tile(scale) -> (w, h, rgba)`, several top-right glyphs in order), `src/app/mod.rs` (the stripe image per receiving pane after `pane_with`; broadcast panes undimmed with the current one), `src/app/coprocess_ui.rs` (indicators).

- [ ] Failing tests `stripe_tile_matches_iterm2`, `indicators_keep_iterm2_order`. Commit `Broadcast indicators`.

### Task 5: Parity and acceptance

- [ ] Update rows (and correct `03:58-62`, `03:733`, `02:436`), recount, write `docs/acceptance/m7f-checklist.md`. Commit.
