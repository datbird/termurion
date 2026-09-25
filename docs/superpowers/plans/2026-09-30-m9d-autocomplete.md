# Tandem Milestone 9d: Autocomplete and Auto Command Completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Session > Open Autocomplete... (Win+;) completes the word before the cursor from the words on screen and in scrollback, the paste history, and the shell history, ranked by iTerm2's scores (context similarity, result number, short-word and full-word penalties, repeated hits with a 0.8 decay, at most `AutocompleteMaxOptions`), with Tab for "more" and Shift+Tab for "less"; View > Auto Command Completion (Shift+Alt+Win+Y) shows the host's commands that start with what is typed at the prompt as you type, where Shift+Enter inserts the rest and Alt+Enter inserts it and runs it.

**Architecture:** `autocomplete.rs` is iTerm2's Autocomplete.m and PopupModel.m as pure code over lines of text: the query (prefix, context words, whitespace before the cursor), the backwards search for candidates on screen, the paste history and command candidates, the scores, and the capped model. `completion.rs` is the list's state and key rules for both kinds. The app draws the list inside the terminal at the cursor (a scene overlay; the terminal keeps the keyboard), routes keys to it while it shows, and opens, updates, and closes Auto Command Completion from the typed command.

**Tech Stack:** as milestone 9c.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` section 3.6 (Open Autocomplete, Auto Command Completion); iTerm2 source `sources/Popups/Autocomplete.m`, `PopupModel.m`, `PopupEntry.m`, `CommandHistoryPopup.m`. Rows: `03-features-and-menus.md` Session > Open Autocomplete..., View > Auto Command Completion; `02-global-settings.md` Auto Command History, `autocompleteMaxOptions`.

## Global Constraints

- Everything from milestones 0 to 9c still holds.
- Word characters are letters, digits, and the profile's "Characters considered part of a word for selection" (iTerm2's word separator set is their complement).
- The list is drawn in the terminal window; it never takes keyboard focus from the terminal.
- `AutocompleteMaxOptions` defaults to 20; `AutoCommandHistory` defaults off.

## Review Focus

1. "git sta" typed with "git status" on screen: candidate "tus" (a suffix, joining prefix). Test: Task 1 `screen_words_complete_the_prefix`.
2. "git " (a space before the cursor) with "git status" on screen: the next word "status". Test: Task 1.
3. The same candidate found three times: its score grows by 0.8, 0.64 of each new hit. Test: Task 1 `hits_add_with_decay`.
4. Tab on a candidate, then Shift+Tab: back to the first list. Test: Task 2 `completion_keys_follow_iterm2`.
5. Auto Command Completion with "gi" typed: Enter runs what was typed (the list closes), Shift+Enter inserts the rest, Alt+Enter inserts the rest and Enter. Test: Task 2.

---

### Task 1: The autocomplete model

**Files:** Create `src/autocomplete.rs`; add to `src/lib.rs`.

**Produces:** `Query { prefix, context, whitespace_before }`, `query_at(lines: &[Vec<char>], row, col, word: &dyn Fn(char) -> bool) -> Query`; `similarity(q, r) -> f64`; `score(n, q, r, joining, word_len) -> f64`; `Model { max }` with `add(value, prefix, score)` and `sorted() -> Vec<Candidate>`; `from_screen(lines, row, col, &Query, word, &mut Model)`, `from_pastes(&[(String, f64 age)], &Query, &mut Model)`, `from_commands(&[(String, usize uses, f64 age)], typed: &str, &Query, &mut Model)`.

- [ ] Failing tests `screen_words_complete_the_prefix`, `hits_add_with_decay`, `scores_follow_iterm2`, `pastes_and_commands_add_candidates`.
- [ ] Implement. Commit `autocomplete model`.

### Task 2: The completion list

**Files:** Create `src/completion.rs`.

**Produces:** `enum Kind { Autocomplete, Commands { typed } }`, `List { kind, rows: Vec<(String /*shown prefix*/, String /*value*/)>, selected, more: String, stack }`, `enum Key { Up, Down, Enter, ShiftEnter, AltEnter, Tab, ShiftTab, Esc, Other }`, `enum Reply { Insert(String), InsertAndRun(String), More(String), Less, Close, PassThrough, Handled }`, `List::key(&mut self, Key) -> Reply`.

- [ ] Failing test `completion_keys_follow_iterm2`.
- [ ] Implement. Commit `completion list`.

### Task 3: App wiring, parity, acceptance

**Files:** Create `src/app/completion_ui.rs`; modify `src/scene.rs` (the list overlay), the key path (keys go to the list first while it shows), `src/app/menu.rs` (`openAutocomplete:`, `toggleAutoCommandHistory:` with its check), `src/commands.rs`, `schema/global.toml` (`AutoCommandHistory`, `AutocompleteMaxOptions`), parity rows, `docs/acceptance/m9d-checklist.md`.

- [ ] Wire. Commit `autocomplete and auto command completion` and `docs: m9d parity and checklist`.
