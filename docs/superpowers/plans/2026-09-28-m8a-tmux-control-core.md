# Tandem Milestone 8a: tmux Control-Mode Core

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tandem recognizes `tmux -CC` in any pane, turns that pane into iTerm2's tmux gateway (the banner, the esc/X/L/C keys, logging, "Detaching...", "Detached"), speaks the control-mode protocol (commands with matched replies, every notification parsed), and runs iTerm2's initial handshake (version, aggressive-resize and UTF-8 checks, the session list). tmux windows do not open yet (8b).

**Architecture:** Three pure models and a small window part. `tmux_hook.rs` is a per-pane byte state machine that finds `ESC P 1000 p` across chunks, then splits the stream into raw lines until `%exit` (iTerm2's VT100TmuxParser, with the response-block and tmux 1.8 rules). `tmux_proto.rs` decodes `%output` octal escapes and parses every notification line (with iTerm2's malformed texts). `tmux_gateway.rs` is the command queue: ids from `%begin`, lists joined by "; ", flags (tolerate errors, wants data, laggy duplicate), the write gate until `%session-changed`, the handshake commands, and version parsing. `Pane::pump` runs the hook before `Screen::feed`; the gateway pane keeps its screen and prints iTerm2's texts into it.

**Tech Stack:** as milestone 7f.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m8-tmux-facts.md` sections 1, 2, 8, 9 (8a). Rows: `03-features-and-menus.md` `:414-417` (tmux -CC control mode) and the gateway rows; `01-profile-settings.md` Tmux Newline.

## Global Constraints

- Everything from milestones 0 to 7f still holds.
- Texts exactly as research 1.2, 1.3, 2 ("** tmux mode started **" and the Command Menu, "Detaching...", "Detached", "tmux logging on/off", "Exiting tmux mode, but tmux client may still be running.", "tmux failed with error: ...", "Unrecognized command from tmux. Did your ssh session die? The command was:", "tmux Reported a Problem", "Unsupported tmux option" and its text, "UTF-8 Mode Not Detected" and its text, "Slow tmux Response" and its text, "Force Detach?").
- On entering tmux mode Tandem writes a bare Ctrl+C (0x03) first; nothing else is written until tmux sends `%session-changed` (or the first reply completes on tmux 1.8).
- Commands end with "\r", or "\n" with the profile's Tmux Newline.
- Recorded transcripts from a real tmux 3.4 (WSL on the Windows test machine, through Tandem's ConPTY) are test fixtures under `tests/fixtures/tmux/`.

## Review Focus

1. `ESC P 1000 p` split across two ConPTY reads (for example `ESC P 10` then `00p`). Expect: detected; the bytes before it reach the screen, nothing after it does. Test: Task 2 `hook_survives_chunk_splits`.
2. A command reply whose text contains a line starting `%exit` (for example `display-message -p "%exit"`). Expect: not an exit. Test: Task 2 `exit_inside_a_reply_is_text`.
3. `%output` with bytes that are not UTF-8 and octal escapes split by `\r`. Expect: the exact bytes. Test: Task 3 `octal_decoding_follows_iterm2`.
4. A command list where the first command fails. Expect: the rest of the list fails the same way, and a tolerated error does not abort. Test: Task 4 `list_errors_fail_the_rest`.
5. tmux versions `3.4`, `3.0a`, `next-3.4`, `openbsd-7.1`, empty. Expect: iTerm2's version table. Test: Task 5 `versions_follow_iterm2`.

---

### Task 1: The ConPTY probe and fixtures

**Files:** Create `src/tmux_probe.rs` (an ignored test that runs `wsl.exe -e tmux -L tandem-probe -f /dev/null -CC new-session` in a ConPTY with each ConPTY source, sends `display-message -p` with a 300-character text and `kill-server`, and writes the raw reads); `tests/fixtures/tmux/wsl-3.4-shipped.bin`, `wsl-3.4-inbox.bin`.

- [ ] Run it on the Windows test machine, check `ESC P 1000 p`, unwrapped long lines, `%exit` and `ESC \`; commit the fixtures and the findings in the ledger. Commit `tmux probe`.

### Task 2: The hook and line reader

**Files:** Create `src/tmux_hook.rs`.

**Interfaces:** `Hook::feed(&mut self, bytes) -> Vec<Out>` with `Out::Screen(Vec<u8>)`, `Out::Enter`, `Out::Line(Vec<u8>)`, `Out::Exit(Vec<u8>)`; the response-block ids, the 1.8 rule, recovery mode.

- [ ] Failing tests `hook_finds_the_start`, `hook_survives_chunk_splits`, `lines_drop_carriage_returns`, `exit_inside_a_reply_is_text`, `transcripts_parse` (the fixtures). Commit `tmux hook`.

### Task 3: Protocol lines

**Files:** Create `src/tmux_proto.rs`.

**Interfaces:** `decode_output(&[u8]) -> Vec<u8>`; `Note` for every notification in research 1.4 (`Output { pane, data }`, `ExtendedOutput { pane, latency, data }`, `LayoutChange { window, layout, visible, zoomed }`, `WindowAdd`, `WindowClose`, `WindowRenamed`, `SessionChanged`, `SessionRenamed`, `SessionsChanged`, `WindowPaneChanged`, `SessionWindowChanged`, `PasteBufferChanged`, `Pause`, `Continue`, `SubscriptionChanged`, `ClientSessionChanged`, `ClientDetached`, `Begin`, `End`, `Error`, `Exit`, `Other`); `parse(line) -> Result<Note, String>` with the malformed texts.

- [ ] Failing tests `octal_decoding_follows_iterm2`, `notifications_parse`, `malformed_lines_say_why`. Commit `tmux protocol`.

### Task 4: The gateway

**Files:** Create `src/tmux_gateway.rs`.

**Interfaces:** `Gateway { newline }` with `send(cmd, Flags, Tag) `, `send_list(cmds, initial)`, `take_writes() -> Vec<u8>`, `line(bytes) -> Vec<Event>` (`Reply { tag, ok, text, data }`, `Note`, `Abort(msg)`, `Initialized`, `InitialFailed(text)`, `Stray(text)`, `Exit(reason)`), the write gate, acceptNotifications, laggy-duplicate detection (`hung(now) -> bool`).

- [ ] Failing tests `replies_match_by_begin`, `writes_wait_for_session_changed`, `list_errors_fail_the_rest`, `notifications_wait_for_the_initial_reply`, `unrecognized_text_aborts`. Commit `tmux gateway`.

### Task 5: Handshake and the gateway session

**Files:** Create `src/app/tmux_ui.rs`; modify `src/pane.rs` (the hook in `pump`), `src/tmux_gateway.rs` (kick-off list, `parse_version`, the checks), `src/app/mod.rs`, `src/app/menu.rs` (gateway keys), `schema/global.toml` (`NoSyncTmuxHung` and the tmux warning keys used here).

**Interfaces:** `parse_version(&str) -> Version`; `Handshake` state; `App::tmux_line`, gateway key handling (esc, X, L, C).

- [ ] Failing tests `versions_follow_iterm2`, `handshake_checks_options`. Commit `tmux gateway session`.

### Task 6: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m8a-checklist.md`. Commit.
