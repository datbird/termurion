# Tandem Milestone 9a: The Paste Pipeline

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every paste goes through iTerm2's pipeline: the eight transforms in iTerm2's order (newlines, Unicode punctuation, unsafe control codes, zero-width format characters, tabs, shell escaping, regex substitution, base64), bracketing, the multi-line, one-line-with-newline, size, and tabs warnings, and chunked writing at iTerm2's speeds with a progress bar and Cancel; plus Edit > Paste Special (Paste Slowly, Paste Faster, Paste Slower and their Slowly variants, Paste File Base64-Encoded, the four warning toggles), key actions 41 and 42 and the pointer paste actions with an encoded paste configuration.

**Architecture:** `paste.rs` is the pure model: flags, the `sanitize` steps, bracketing, base64 lines, vim escapes, the speed math, the warning decision, and the JSON configuration codec with its summary text. The app keeps one paste job queue per pane: `user_paste` builds a job (flags from the entry point), asks the warning question when one applies, and a feeder writes chunks on the event loop tick, showing a "Pasting..." bar with Cancel for long pastes.

**Tech Stack:** as milestone 8h (Rust, winit, egui, `serde_json` for the configuration codec).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` sections 6.0 to 6.9 and 6.11, and 10 (9a). Rows: `03-features-and-menus.md` `:91-101`, `:590-600`, `:604`; `02-global-settings.md` `:123`, `:263-272`, `:403`, `:404`, `:467`, `:468`, `:654`, `:993-1015` (not paste history); `01-profile-settings.md` `:212`.

## Global Constraints

- Everything from milestones 0 to 8h still holds.
- Transform order and character sets are iTerm2's (facts 6.2). Chunks count UTF-16 units and never split a surrogate pair.
- Cmd+V (Ctrl+Shift+V here) uses newlines to CR, unsafe control codes removed, and bracketing when the terminal asks for it; 768 units every 0.01 s unless `QuickPasteBytesPerCall`/`QuickPasteDelayBetweenCalls` are set. Paste Slowly: 16 every 0.125 s (`SlowPaste*`).
- Warnings are bars with iTerm2's texts and buttons; "Remember my choice" is the bar's permanent silence key.
- Paste history (9b) and Advanced Paste (9b) are not in 9a; "Advanced..." buttons open nothing yet and say so in the ledger.

## Review Focus

1. A 2 MB paste. Expect: a "Pasting..." bar, chunks on the tick, Cancel stops it and the next queued paste runs. Test: Task 2 `chunks_never_split_surrogates` plus the checklist.
2. `"abc\n"` at a shell prompt. Expect: "OK to paste one line ending in a newline at shell prompt?" with "Paste Without Newline". Test: Task 2 `warnings_follow_iterm2`.
3. Tabs with bracketing off. Expect: "You're about to paste a string with tabs." Tabs with bracketing on: no question. Test: Task 2 `warnings_follow_iterm2`.
4. Shell escaping of `a b'c\`. Expect: `a\ b\'c\\`. With `EscapeWithQuotes`: `'a b'\''c\'`. Test: Task 1 `transforms_follow_iterm2`.
5. A key-binding paste configuration JSON from iTerm2 (missing keys). Expect: missing keys read as 0 or false. Test: Task 2 `configs_decode_like_iterm2`.

---

### Task 1: The transforms

**Files:** Create `src/paste.rs`; add to `src/lib.rs`.

**Produces:** flag constants (`ESCAPE`, `NEWLINES`, `CONTROLS`, `BRACKET`, `BASE64`, `PUNCTUATION`, `COMMANDS`, `REMOVE_NEWLINES`, `REGEX`, `NO_WARNINGS`); `enum Tabs { Keep, Spaces(usize), CtrlV }`; `struct Job { text, flags, tabs, regex, substitution }`; `struct Prefs { escape_with_quotes, strip_zero_width }`; `sanitize(&Job, &Prefs) -> String`; `bracket(&str, per_line: bool) -> String`; `base64_lines(&[u8]) -> String`; `expand_vim(&str) -> String`.

- [ ] Failing test `transforms_follow_iterm2` (each step and the order), `base64_lines_are_77_wide`, `vim_escapes_expand`, `brackets_wrap_lines`.
- [ ] Implement. Commit `paste transforms`.

### Task 2: Speeds, chunks, warnings, configurations

**Files:** Modify `src/paste.rs`.

**Produces:** `Speed { units, delay }` with `QUICK`, `SLOW`, `faster(Speed, factor) -> Speed` (clamps [1, 1048576] and [0.001, 10]), `rate_text(Speed) -> String` ("Pasting at up to %@/sec"); `next_chunk(text: &[u16], at: usize, units: usize, block_at_newline: bool) -> usize`; `shows_progress(len, Speed, commands) -> bool`; `enum Warning { Size(n), Lines { n, at_prompt }, OneLine { at_prompt }, Tabs }` with `text()` and `buttons()`, `check(text, at_prompt: Option<bool>, bracketed, flags, prefs: &WarnPrefs) -> Option<Warning>`; `Config` (every JSON key) with `decode(&str)`, `encode()`, `job(text)`, `summary()`.

- [ ] Failing tests `speeds_follow_iterm2`, `chunks_never_split_surrogates`, `warnings_follow_iterm2`, `configs_decode_like_iterm2`.
- [ ] Implement. Commit `paste speeds, warnings, and configurations`.

### Task 3: App wiring, menus, parity, acceptance

**Files:** Create `src/app/paste_ui.rs`; modify `src/app/broadcast_ui.rs` (`user_paste` builds a job), `src/app/mod.rs` (tick), `src/app/menu.rs` (Paste Special items by label), `src/commands.rs` (statuses), `src/keyaction.rs` (41, 42 done; 65 done, fixing 8e), `src/app/actions.rs` (41, 42), `src/pointer.rs` and the pointer dispatch (paste arguments), `schema/global.toml` (paste Advanced keys, `PasteTabToStringTabStopSize`, `WrapDroppedFilenamesInQuotesWhenPasting`), parity rows, `docs/acceptance/m9a-checklist.md`.

- [ ] Wire; a pure test `paste_jobs_queue_in_order` for the job queue in `src/paste.rs`. Commit `paste pipeline` and `docs: m9a parity and checklist`.
