# Tandem Milestone 4b: Find, Smart Selection, Semantic History, and Links

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's find bar (five modes, search as you type, Find Next going up, wrap, match highlighting, the i/N count), Use Selection for Find, Clear Find, Jump to Selection, Find Globally, Filter, Find URLs, Find All Smart Selection Matches, Pick Result to Open, Select Matches, Smart Selection (rules, precision, quad-click, actions), Semantic History (Win-click on files and URLs, the seven actions), OSC 8 links (dashed underline, Win-click, Copy Link Address), Open Selection, and Zoom In on Selection / Zoom Out.

**Architecture:** Pure modules decide: `find.rs` (modes, regex building, all matches, navigation, count), `smartsel.rs` (rules, scoring, action text), `semantic.rs` (what is under a Win-click: link, file with line and column, smart selection action, URL). The app draws the find bar and match highlights through `scene.rs`, keeps a per-pane find state, and shows a synthetic read-only screen for Filter and Zoom. Find Globally is an egui window on the 4a host.

**Tech Stack:** as milestone 4a. alacritty_terminal's `RegexSearch` / `RegexIter` does the matching.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory: `docs/parity/03-features-and-menus.md` (find; Edit and View menu rows owned by 4b), `docs/parity/01-profile-settings.md` (Smart Selection, Semantic History, Match Background Color, Link Color), `docs/parity/02-global-settings.md` (Pointer: Win-click opens filename/URL; General > Selection: Double-click performs smart selection).

## Global Constraints

- Everything from milestones 0 to 4a still holds. iTerm2 facts come from the research recorded in the ledger (iTermFindDriver.m, iTermFindOnPageHelper.m, iTermCoreSearch.m, SmartSelectionRules.plist, iTermTextExtractor.m 439-529, iTermURLActionFactory.m, iTermPathFinder.m, iTermPathCleaner.m 234-243, iTermSemanticHistoryController.m 768-946).
- Find Next goes up (toward older text) and Find Previous goes down, iTerm2's default (`swapFindNextPrevious` YES). Return is Find Next, Shift+Return Find Previous.
- The find mode is global (`findMode_iTerm`, default 0 smart case). Values 0 smart case, 1 case-sensitive substring, 2 case-insensitive substring, 3 case-sensitive regex, 4 case-insensitive regex never change.
- Cmd is Win. A Win-click must not open the Start menu (the hook's mask key is sent).
- Semantic History runs commands through `cmd.exe /c` (iTerm2 uses `/bin/sh -c`); Run Coprocess waits for milestone 6.

## Review Focus

1. A query typed while output streams in. Expect: matches stay right; the count updates. Test: Task 1 `new_output_keeps_matches_in_order`.
2. A regex that matches the empty string (`x*`). Expect: no hang, no zero-width matches. Test: Task 1 `empty_matches_are_skipped`.
3. A Win-click on `src/main.rs:12:5` printed by a compiler, with the working directory from shell integration. Expect: the file, line 12, column 5. Test: Task 3 `finds_a_path_with_line_and_column`.
4. A Win-click on text that is both a path and a URL-like word. Expect: iTerm2's order (link, file, smart selection action, URL). Test: Task 3 `resolution_follows_iterm2s_order`.
5. Filter over 100,000 lines of history. Expect: no freeze of the window. Test: Task 6 `filtering_a_large_buffer_is_quick` (under 500 ms for 100,000 lines).

---

### Task 1: Search engine

**Files:** Create `src/find.rs`.

**Interfaces:** `pub enum Mode { SmartCase = 0, CaseSensitive = 1, CaseInsensitive = 2, RegexSensitive = 3, RegexInsensitive = 4 }` with `from_i64`, `label`; `pub fn pattern(query: &str, mode: Mode) -> String` (substring modes escape the query; smart case is sensitive when the query has an uppercase letter; insensitive modes add `(?i)`); `pub fn find_all(screen: &Screen, pattern: &str) -> Result<Vec<Match>, String>` (`Match { start: Point, end: Point }`, top to bottom, zero-width matches skipped); `pub struct Results { pub matches: Vec<Match>, pub current: Option<usize> }` with `step(&mut self, up: bool) -> (Option<&Match>, bool /*wrapped*/)`, `start_at(from: Point)` (the nearest match at or above `from`), `label(&self) -> String` ("N" or "i/N", i counted from the bottom).

- [ ] Failing tests: `smart_case_follows_uppercase`, `substring_modes_escape_the_query`, `finds_every_match_top_to_bottom`, `empty_matches_are_skipped`, `find_next_goes_up_and_wraps` (from the bottom match, up to the top, then wraps to the bottom with `wrapped`), `the_count_counts_from_the_bottom`, `new_output_keeps_matches_in_order`. Implement. Commit `Add the find engine`.

### Task 2: Find bar

**Files:** Create `src/app/find_ui.rs`; modify `src/scene.rs`, `src/app/mod.rs`, `src/app/menu.rs`, `src/commands.rs`, `src/keyaction.rs`, `schema/global.toml` (`findMode_iTerm` int 0).

**Interfaces:** Per pane `FindState { open, query, results, typed_at: Instant }`. Keys while open go to the bar: text, Backspace, Ctrl+V, Return (Find Next = up), Shift+Return (down), Esc (close, clear highlights, back to the terminal), Tab (grow the selection by a word and load it), Alt+Return (copy and paste the selection into the session). Search as you type: queries of 2 characters or fewer wait 1 s (iTerm2 findDelaySeconds). A click on the mode label opens a menu of the five modes. The bar draws at the top right of the focused pane: query, cursor, the i/N count, and the mode. All visible matches draw with the profile's Match Background Color (text black or white by its brightness); the current match is the selection; the view scrolls to center a match that is not visible. Commands: `performFindPanelAction:` Find... (open), Use Selection for Find (selected text, regex-escaped in regex modes), Select Matches (see Task 8); `findNext:`, `findPrevious:`, `clearFindString:`, `jumpToSelection:`. Key actions 36 (Find Regular Expression: a case-sensitive regex search that does not change the bar's saved query), 57 (down), 58 (up) are Done.

- [ ] Pure tests (`find_ui.rs`): `short_queries_wait_a_second`, `escape_closes_and_clears`, `return_goes_up_and_shift_return_down` (on `FindState` with a fake clock). Scene test: `match_highlights_use_the_match_color`. Commit `Add the find bar`.

### Task 3: Semantic History resolution

**Files:** Create `src/semantic.rs`.

**Interfaces:** `pub enum Target { Link(String), File { path: PathBuf, line: Option<u32>, column: Option<u32> }, Action { rule: usize, action: usize, text: String }, Url(String), RawCommand(String) }`; `pub fn resolve(ctx: &ClickContext) -> Option<Target>` where `ClickContext { line_text: String, before: String, after: String (up to 2000 chars each, iTerm2 maxSemanticHistoryPrefixOrSuffix), link: Option<String>, cwd: Option<PathBuf>, rules: Vec<Rule>, raw_command: bool }`; `pub fn find_path(before, after, cwd) -> Option<(PathBuf, Option<u32>, Option<u32>)>` (chunks split on `[\t ():",]`, up to 10 chunks right, backslash unescapes, trailing `!?.,;:` and ellipsis removed, relative to `cwd`, git prefixes `a/ b/ i/ w/ c/ o/` removed, line and column from iTerm2's suffix patterns); `pub fn url_like(before, after) -> Option<String>` (URL characters, trailing punctuation removed, a colon means a URL, else a slash not in first place, `https://` prefixed, `http://` for a single word).

- [ ] Failing tests with temp files: `finds_a_path_with_line_and_column`, `finds_a_python_traceback_path` (`File "x.py", line 3, in f`), `strips_git_diff_prefixes`, `trailing_punctuation_is_not_part_of_the_path`, `url_like_text_becomes_a_url`, `resolution_follows_iterm2s_order`. Commit `Add Semantic History resolution`.

### Task 4: Smart Selection

**Files:** Create `src/smartsel.rs`; modify `src/input.rs` (four clicks), `src/app/mod.rs`, `schema/global.toml` (`DoubleClickPerformsSmartSelection` bool false, tab General, section Selection).

**Interfaces:** `pub struct Rule { pub regex: String, pub precision: f64, pub notes: String, pub actions: Vec<RuleAction> }`, `RuleAction { title, action: i64, parameter }`, `pub fn rules(profile) -> Vec<Rule>` (the profile's list, else the schema default), `pub fn select_at(lines: &[String], click: (usize, usize), rules: &[Rule]) -> Option<(usize /*rule*/, Range)>` (score = precision × length; the highest wins; text window is 2 lines each side), `pub fn expand(parameter, captures, cwd, host, user, escape: Escape) -> String` (`\0`..`\8`, `\d`, `\h`, `\u`, `\n`, `\\`; captures quoted for cmd.exe for command actions).

- [ ] Failing tests: `precision_times_length_picks_the_url_over_the_word`, `double_click_setting_swaps_word_and_smart`, `quad_click_is_smart_selection`, `action_parameters_substitute`. Commit `Add Smart Selection`.

### Task 5: Win-click, hover, links, and the context menu

**Files:** Modify `src/app/mod.rs`, `src/app/actions.rs`, `src/app/menu.rs`, `src/scene.rs`, `src/pointer.rs` (statuses), `src/winkeys.rs` (public mask), `src/commands.rs`.

**Interfaces:** A Win-click (when `CommandSelection` is on) resolves the target (Task 3) and opens it through the profile's Semantic History: best editor (ShellExecute open, a directory in Explorer), url (`\1`..`\5` substituted, percent-encoded), editor (the `editor` value as an exe path, with the file), command and raw command (`cmd.exe /c`, bash-style values quoted for cmd), send text, coprocess (milestone 6). With Win held, the target under the pointer is underlined in Link Color and the pointer is a hand. OSC 8 cells draw a dashed underline in Link Color. The right-click menu adds Copy Link Address (on an OSC 8 cell) or Copy URL, and the matched smart selection rule's actions at the top. Pointer actions Open URL/Semantic History, Open URL in background, Smart Selection, Smart Selection Ignoring Newlines, and Copy Link Address are Done.

- [ ] Pure tests: `semantic_history_command_quotes_for_cmd`, `url_action_percent_encodes`. Scene test: `links_draw_a_dashed_underline`. Commit `Open files and URLs with Win-click; draw links`.

### Task 6: Filter, Zoom In on Selection, Zoom Out

**Files:** Create `src/synthetic.rs`; modify `src/app/mod.rs`, `src/app/find_ui.rs`, `src/commands.rs`.

**Interfaces:** `Synthetic { screen: Screen (unlimited scrollback, no cursor), kind: Filter { query } | Zoom }`; `pub fn filter_lines(screen: &Screen, pattern) -> Vec<String>` (whole logical lines that match; an empty query matches all), `pub fn zoom_lines(screen, range) -> Vec<String>`. A pane with a synthetic view draws it instead of its own screen; new output refilters (debounced) while a filter is up; Esc or Zoom Out ends it and restores the scroll position.

- [ ] Failing tests: `filter_keeps_whole_matching_lines`, `filtering_a_large_buffer_is_quick`, `zoom_copies_the_selected_lines`. Commit `Add Filter and Zoom`.

### Task 7: Labeled results: Find URLs, Smart Selection matches, Pick Result to Open

**Files:** Modify `src/app/find_ui.rs`, `src/scene.rs`.

**Interfaces:** `pub fn labels(n) -> Vec<String>` (`1`..`9`, then `A`, `B`, ..., more letters when needed); visible results get labels drawn over them; a key picks one: Open (Semantic History, else the text as a URL) or Copy; Alt+key does the other (Copy toasts "Copied"). Find URLs uses iTerm2's `findUrlsRegex`; Find All Smart Selection Matches ORs every rule of precision Normal or higher.

- [ ] Failing test: `labels_run_digits_then_letters`. Commit `Add labeled results`.

### Task 8: Find Globally, Select Matches, Open Selection

**Files:** Create `src/global_find_ui.rs`; modify `src/app/*`.

**Interfaces:** Find Globally: an egui window with a query field, the mode menu, and results grouped by session (snippets with the match marked); a click reveals the pane and selects the match. Select Matches: selects from the first to the last match of the current results and copies every match, one per line (alacritty has one selection range; ruled Partial). Open Selection: an existing file in the selection (relative to the working directory) through Semantic History, else the text as a URL; beeps when nothing is selected.

- [ ] Pure test: `global_results_group_by_session`. Commit `Add Find Globally, Select Matches, and Open Selection`.

### Task 9: Settings editors

**Files:** Modify `src/settings_keys.rs` or a new `src/settings_advanced.rs`, `src/settings_ui.rs`.

**Interfaces:** Profile Advanced tab: Smart Selection rules table (notes, regex, precision menu; add, remove, edit; actions list with title, action menu, parameter), Semantic History (action menu with the seven iTerm2 choices and the editor or text field), "Use interpolated strings" checkbox. Pointer pane: Win-click opens filename/URL enabled. A General > Selection row for Double-click performs smart selection on the Pointer pane until the General pane exists (milestone 13).

- [ ] Pure test: `rule_rows_round_trip`. Commit `Add the Smart Selection and Semantic History editors`.

### Task 10: Parity and acceptance

- [ ] Update the rows 4b closes, recount `00-roadmap.md`, write `docs/acceptance/m4b-checklist.md`. Commit.
