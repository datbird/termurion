# Tandem Milestone 6a: Triggers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's triggers: the profile's `Triggers` list (regex, action, parameter, Instant, Enabled, name, job), evaluated on each completed logical line of output and, for Instant triggers, on the line being written every 0.5 s; plain and interpolated parameter substitution; every terminal action iTerm2 lists (with the ones owned by later milestones listed and marked unavailable); the Triggers editor in Settings; and Session > Triggers (Add Trigger..., Edit Triggers, Enable Triggers in Interactive Apps, Enable All, Disable All).

**Architecture:** A pure `triggers.rs` holds the model (parsed from the profile's list of tables), the action catalog (iTerm2's class names, titles, parameter kinds, placeholders, idempotency), and plain substitution. A pure `interp.rs` evaluates iTerm2 interpolated strings (`\(path)`, `\(matches[n])`, string literals) against a variable lookup and refuses function calls. `trigger_eval.rs` extends `Screen`: it counts lines scrolled off (alacritty_terminal patch 13), finds completed logical lines after each feed, matches enabled triggers, and applies the grid actions itself (highlight, style, hyperlink, annotate, marks, fold, prompt, directory, host, inject, user variable, stop); window actions become `Request::Trigger(...)` for `app/triggers_ui.rs`. The editor is `settings_triggers.rs`, an egui master and detail list in Profiles > Advanced.

**Tech Stack:** as milestone 5f; `fancy-regex` (already a dependency) for ICU-like patterns.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `01-profile-settings.md` Triggers section; `03-features-and-menus.md` Session > Triggers and the triggers subsystem rows. iTerm2 facts: `.superpowers/research/m6-iterm2-facts.md` sections 1 to 3.

## Global Constraints

- Everything from milestones 0 to 5 still holds.
- Profile keys and stored forms are iTerm2's (`Triggers` array of tables with `regex`, `action` (the class name), `parameter`, `partial`, `disabled`, `name`, `job`, `matchType`), so imported iTerm2 profiles work unchanged. Unknown action class names are kept and shown as unavailable, never dropped.
- Regex: fancy-regex, case sensitive unless the pattern says `(?i)`; an invalid pattern never fires and the editor shows the error.
- Plain substitution replaces `\0` to `\8` (iTerm2 never replaces `\9`), then `\a \b \e \n \r \t \\` and `\xHH`; a backslash sequence counts only after an even number of backslashes.
- Triggers see the logical line (soft wraps joined, at most 3 screen rows before and after the row, iTerm2's triggerRadius), read back from the grid.
- A full check runs when a line ends; Instant triggers run on the unfinished line at most every 0.5 s and fire once per line (Highlight Text may fire again).
- While the alternate screen is up, triggers run only when the profile's "Enable triggers in interactive apps" is on.
- A trigger with a `job` fires only when the foreground job is known to match; Tandem learns the foreground job in milestone 6c, so until then such triggers do not fire (ruling in the ledger).
- Actions owned by later milestones stay in the list, marked "(milestone N)", and do nothing: Capture Output (9), Invoke Script Function (11), Open Password Manager (10), Enter Workgroup and Exit Workgroup (not in the roadmap yet). Ring Bell and Post Notification call 6b's bell and notification code when it lands; in 6a Ring Bell beeps and Post Notification shows a bar. Run Coprocess and Run Silent Coprocess wait for 6c.
- Run Command runs `cmd.exe /d /c <command>` hidden, in Tandem's working directory, at most 16 at once (more wait), output discarded; a failure shows "Run Command Trigger Failed" in a bar.
- Show Alert is a bar with "OK" and "Disable This Alert" (session only), once per second per trigger. Bounce Dock Icon flashes the taskbar button (FlashWindowEx: until activated, or once).

## Review Focus

1. Output of 100000 short lines in one read with 20 triggers. Expect: every completed line is checked once, no line twice; time stays linear. Test: Task 3 `each_line_is_checked_once`.
2. A trigger whose regex matches the empty string, or matches every character. Expect: no endless loop, no action for an empty match (iTerm2 skips empty ranges for cell actions). Test: Task 3 `empty_matches_do_nothing`.
3. A Highlight Text trigger on a line that scrolled off before the check ran. Expect: nothing is changed, no panic. Test: Task 3 `lines_gone_before_the_check`.
4. Send Text or Inject Data that prints text matching its own trigger. Expect: no runaway loop (Inject Data text goes through the parser once; a trigger does not fire again on the line it is already processing). Test: Task 4 `injected_text_does_not_refire_forever`.
5. An interpolated parameter with a function call, an unknown variable, or unbalanced parentheses. Expect: the call is refused with a message, unknown variables are empty, bad syntax leaves the text as written and shows the error once. Test: Task 2 `interpolation_refuses_calls_and_bad_syntax`.

---

### Task 1: Trigger model and plain substitution

**Files:** Create `src/triggers.rs`; modify `src/lib.rs`.

**Interfaces:** `Trigger { regex: String, action: String, parameter: String, instant: bool, enabled: bool, name: String, job: String, match_type: i64, extra: toml::Table }` with `from_table` and `to_table` (unknown keys kept); `parse_list(&toml::Value) -> Vec<Trigger>`; `Kind { None, Text, Popup(&'static [(&'static str, i64)]), TwoColors, TwoStrings, TabStatus, Setting }`; `ActionInfo { class, title, kind, placeholder, idempotent, multi_fire, later: Option<u8> }`; `CATALOG: &[ActionInfo]` in iTerm2's menu order with iTerm2's class names and synonyms (`GrowlTrigger`); `action_info(class) -> Option<&ActionInfo>`; `substitute(param, captures: &[Option<&str>]) -> String`; `escape_regex(text) -> String` (Add Trigger...: ICU metacharacters `\*?+[(){}^$|.]-&` escaped).

- [ ] Failing tests: `triggers_round_trip_with_unknown_keys`, `catalog_follows_iterm2`, `substitution_follows_iterm2` (`\9` stays, `\\1` stays literal, escapes and `\xHH`), `add_trigger_escapes_the_selection`. Commit `Add the trigger model`.

### Task 2: Interpolated strings

**Files:** Create `src/interp.rs`.

**Interfaces:** `evaluate(template, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>]) -> Result<String, String>`: literal text passes through with the plain escapes; `\(expr)` where `expr` is a dotted path (`session.hostname`, `user.x`, `hostname` meaning `session.hostname`), `matches[n]`, a quoted string, or `a ?? b` (first non-empty); a call like `f(x)` is refused ("Function calls are not allowed here."); unknown paths give ""; unbalanced `\(` is an error naming the column. `has_calls(template) -> bool` (SetBadgeFormat's check, used in 6b).

- [ ] Failing tests: `paths_matches_and_literals`, `interpolation_refuses_calls_and_bad_syntax`, `fallback_operator`. Commit `Add interpolated strings`.

### Task 3: Evaluating triggers (alacritty patch 13)

**Files:** `crates/alacritty_terminal` (`Term.scrolled_off: u64`, counted when a row leaves the top of the main screen's history or the screen), `TANDEM-PATCHES.md`; create `src/trigger_eval.rs`; modify `src/screen.rs`.

**Interfaces:** `Screen::set_triggers(list, TriggerPrefs { interactive: bool, interpolated: bool })` (compiles once; bad patterns kept with their error); `Screen::trigger_errors() -> Vec<(usize, String)>`; after each feed, every logical line whose last row ended since the last check is matched (lines already gone are skipped); `Screen::check_instant_triggers(now)` (0.5 s limit, the cursor's line, once per line per trigger unless multi-fire); `Screen::app_vars: HashMap<String, String>` (variables the window owns, pushed by the app: `session.name`, `session.profileName`, `tab.title`, ...) and the screen's own (`session.hostname`, `session.username`, `session.path`, `user.*`, `line`); matches over all non-empty ranges; `Stop Processing Triggers` ends the list for that line.

- [ ] Failing tests: `each_line_is_checked_once`, `wrapped_lines_are_one_logical_line`, `instant_triggers_fire_once_per_line`, `empty_matches_do_nothing`, `lines_gone_before_the_check`, `alt_screen_needs_the_profile_switch`, `job_triggers_wait_for_6c`. Commit `Evaluate triggers`.

### Task 4: Grid actions

**Files:** Modify `src/trigger_eval.rs`, `src/screen.rs` (`Request::Trigger`).

**Interfaces:** in the screen: Highlight Text (`{#fg,#bg}` either side empty, legacy numeric presets, default white on red; the matched cells' colors), Highlight Line (the whole logical line, at most 10 screen widths), Change Style (SGR parameters applied to the matched cells), Make Hyperlink (an OSC 8 link on the matched cells; empty or unparsable URL ignored), Annotate (a note over the match, not on the alternate screen), Set Mark (Keep Scrolling / Stop Scrolling), Set Named Mark, Fold to Named Mark, Prompt Detected (a prompt mark unless the line has one), Report Directory, Report User & Host, Inject Data (the text through the parser as output, once, with triggers off while it runs), Set User Variable (`name<U+1>value`, a `.` in the name refused, `user.` stripped), Stop Processing Triggers. Requests to the window: `Trigger::{Alert { trigger, text }, Notify(text), Bell, Bounce { until_active }, SendText(text), RunCommand(cmd), SetTitle(text), TabStatus { status, dot, text }, BufferInput(bool), ProfileSetting { key, on }, Coprocess { cmd, silent }, Unavailable(title, milestone)}`.

- [ ] Failing tests: `highlight_text_colors_the_match`, `highlight_line_colors_the_line`, `change_style_applies_sgr`, `hyperlink_trigger_links_cells`, `annotate_and_marks`, `fold_to_named_mark`, `prompt_detected_marks_once`, `report_directory_and_host`, `injected_text_does_not_refire_forever`, `user_variable_names`. Commit `Add the grid trigger actions`.

### Task 5: Window actions and menus

**Files:** Create `src/app/triggers_ui.rs`; modify `src/app/requests_ui.rs`, `src/app/menu.rs`, `src/commands.rs`, `src/app/mod.rs`.

**Interfaces:** the app pushes the profile's triggers to each pane's screen (on start, on profile change, on Edit Session), refreshes `app_vars` when titles change, and calls `check_instant_triggers` from its timer; requests: Show Alert (bar "OK" / "Disable This Alert", 1 s per trigger), Post Notification (a bar until 6b), Ring Bell (MessageBeep until 6b), Bounce (FlashWindowEx), Send Text (as typed, not broadcast), Run Command (`cmd.exe /d /c`, hidden, 16 at once), Set Title (the session name), Set Tab Status (5c's tab status), Buffer Input (5c's), Set Profile Setting (a session profile value), Coprocess and Unavailable (a bar naming the milestone, once per trigger). Session > Triggers: Add Trigger... (Settings opens on the new trigger, regex = the escaped selection, name = the selection), Edit Triggers, Enable Triggers in Interactive Apps (check mark), Enable All, Disable All (session profile values).

- [ ] Pure tests: `run_command_pool_waits_past_16`, `alert_rate_limit_per_trigger`, `enable_all_rewrites_disabled`. Commit `Run window trigger actions and the Triggers menu`.

### Task 6: The Triggers editor

**Files:** Create `src/settings_triggers.rs`; modify `src/settings_ui.rs`.

**Interfaces:** Profiles > Advanced > Triggers: a list (bold name, `/regex/`, action and parameter; Instant rows marked with a lightning glyph; disabled rows faint) with + and -; the detail pane: Name, Regular Expression (the compile error under it), Job, Action (a combo in iTerm2's order, later-milestone actions labeled "(milestone N)"), Parameters (text, popup, two color pickers, two strings, tab status, or a setting picker, by the action's kind), Instant, Enabled; "Use interpolated strings for parameters" and "Enable triggers in interactive apps" check boxes. Edits save to the profile like other settings.

- [ ] Pure tests: `editor_rows_follow_iterm2`, `parameter_encodings_round_trip` (`{#fg,#bg}`, `name<U+1>value`, `dot<U+1>color<U+1>status`, `key<U+1>1`). Commit `Add the Triggers editor`.

### Task 7: Parity and acceptance

- [ ] Update the rows 6a closes, split milestone 6 in the roadmap into 6a (triggers), 6b (bell, notifications, idle and new output, badges), 6c (Automatic Profile Switching, coprocesses, foreground job), recount, write `docs/acceptance/m6a-checklist.md`. Commit.
