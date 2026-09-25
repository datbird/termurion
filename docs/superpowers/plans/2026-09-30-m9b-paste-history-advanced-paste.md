# Tandem Milestone 9b: Paste History and Advanced Paste

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's paste history (every copy and paste, trimmed, deduplicated, at most `PasteHistoryMaxOptions` entries, saved to disk only with "Save copy/paste and command history to disk"), Session > Open Paste History... (Shift+Ctrl+Win+H here) as a filterable popup that pastes the chosen entry, and Edit > Paste Special > Advanced Paste... (the source list with history, the editable preview, every transform control with iTerm2's enabling rules and remembered defaults, the speed sliders, "Minimum duration", Paste and Cancel); the paste questions' "Advanced..." buttons open it.

**Architecture:** `paste_history.rs` is the pure store (entries, save rules, the "N lines, size, date" text, a JSON file). `popup_filter.rs` is iTerm2's popup matcher (a case-insensitive subsequence in the first 200 characters, newest first, the 4 s filter reset), shared with the later history popups. `advanced_paste.rs` is the form model: the initial state from the text and the settings, which controls are enabled, the preview (escape, base64, and regex applied), the paste-time job, the statistics and duration texts, and the logarithmic slider maps. The app has two egui windows (the popup, like Find Globally; the Advanced Paste form, like the tmux Dashboard).

**Tech Stack:** as milestone 9a.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m9-facts.md` sections 6.5, 6.10, 3.6 (common popup behavior). Rows: `03-features-and-menus.md` `:90` (Advanced Paste), `:169` (Open Paste History), `:590-600` (Paste special rows, the window half), `:601`, `:603`; `02-global-settings.md` `:38` (Save copy/paste and command history), `:263-272` (PasteSpecial* defaults), the paste-history Advanced rows (`pasteHistoryMaxOptions`, `includePasteHistoryInAdvancedPaste`, `advancedPasteWaitsForPromptByDefault`, `saveToPasteHistoryWhenSecureInputEnabled`).

## Global Constraints

- Everything from milestones 0 to 9a still holds.
- History is saved to `%APPDATA%\Tandem\pbhistory.json` only while `SavePasteHistory` is on; turning it off does not delete the file (iTerm2's).
- Advanced Paste's remembered settings use iTerm2's keys (`PasteSpecialChunkSize`, `PasteSpecialChunkDelay`, `NumberOfSpacesPerTab`, `TabTransform`, `EscapeShellCharsWithBackslash`, `ConvertUnicodePunctuation`, `ConvertDosNewlines`, `RemoveControlCodes`, `BracketedPasteMode`, `PasteSpecialUseRegexSubstitution`, `PasteSpecialRegex`, `PasteSpecialSubstitution`), each saved only when its control was enabled.
- Windows has no secure keyboard entry; history is always saved (the secure-input rule is a no-op).

## Review Focus

1. Copy "abc", copy "abc" again, copy "abcd": history holds one "abc" then "abcd"? No: iTerm2 removes an existing equal entry and a last entry that is a prefix, so it holds only "abcd". Test: Task 1 `history_saves_like_iterm2`.
2. The 21st entry with the default maximum: the oldest goes. Test: Task 1.
3. Filter "gco" against "git checkout main": it matches (subsequence). Test: Task 1 `popups_filter_like_iterm2`.
4. Advanced Paste on text without tabs: the tab controls are off; with Replace on and a regex, the preview shows the substitution and Paste sends the preview. Test: Task 2 `advanced_paste_follows_iterm2`.
5. "Minimum duration" for 100,000 characters at 1024 per 0.01 s: iTerm2's formula. Test: Task 2.

---

### Task 1: History and the popup filter

**Files:** Create `src/paste_history.rs`, `src/popup_filter.rs`; add to `src/lib.rs`.

**Produces:** `Entry { value, time }`; `History { entries, max }` with `save(&mut self, text, now) -> bool`, `newest_first()`, `to_json()`, `from_json()`, `clear()`; `time_text(value, time, now) -> String`; `popup_filter::matches(filter, value) -> Option<Vec<usize>>` (the matched char indices), `Filter` with `type_char(c, now)`, `backspace()`, `text()`.

- [ ] Failing tests `history_saves_like_iterm2`, `history_files_round_trip`, `popups_filter_like_iterm2`.
- [ ] Implement. Commit `paste history`.

### Task 2: The Advanced Paste form

**Files:** Create `src/advanced_paste.rs`.

**Produces:** `Form` (source index, preview text, tab transform and spaces, the checkboxes, regex fields, chunk and delay, wait), `Form::new(text, &Defaults, bracketed, shell_integration, at_prompt) -> Form`, `enabled(&self, text) -> Enabled`, `preview(text, &Form) -> String`, `job(&Form) -> (paste::Job, paste::Speed)`, `stats(preview) -> String`, `duration(len, Speed) -> String`, `slider_to_units(f32) -> usize`, `units_to_slider(usize) -> f32`, `slider_to_delay`, `delay_to_slider`, `Defaults` read and written by the app.

- [ ] Failing test `advanced_paste_follows_iterm2`.
- [ ] Implement. Commit `Advanced Paste form`.

### Task 3: App wiring, parity, acceptance

**Files:** Create `src/paste_history_ui.rs` (popup window), `src/advanced_paste_ui.rs` (form window), `src/app/history_ui.rs`; modify copy paths (record copies), `src/app/paste_ui.rs` (record pastes; "Advanced..." opens the form), `src/app/menu.rs` (`pasteOptions:`, `openPasteHistory:`), `src/commands.rs`, `schema/global.toml` (`SavePasteHistory`, `PasteHistoryMaxOptions`, `IncludePasteHistoryInAdvancedPaste`, `AdvancedPasteWaitsForPromptByDefault`, the PasteSpecial* keys), `schema/menus.toml` (shortcuts), parity rows, `docs/acceptance/m9b-checklist.md`.

- [ ] Wire. Commit `paste history and Advanced Paste` and `docs: m9b parity and checklist`.
