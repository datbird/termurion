# Tandem Milestone 4a: Marks and Shell Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's shell integration and marks on Windows: OSC 133, OSC 7, and the OSC 1337 shell-integration keys are understood; prompt, manual, and named marks live on their rows through scrolling and resizing; the mark commands work (Set Mark, Set Named Mark, Jump to Mark, Next and Previous Mark, Clear to Last Mark, Select Output of Last Command, Select Current Command, the mark alerts); marks show in the left margin; Install Shell Integration sets up pwsh (Tandem's own script) and bash or zsh (iTerm2's installer); and "Reuse previous session's directory" works.

**Architecture:** alacritty_terminal is vendored in `crates/alacritty_terminal` with one patch: each `Row` carries a `mark: u32` that moves with the row through scrolling and reflow (TANDEM-PATCHES.md). `osc.rs` is a streaming scanner that finds the OSCs alacritty ignores and says where each one ends in the byte stream, so `Screen::feed` can stop there, act at the exact cursor position, and continue. `marks.rs` holds the mark records and the shell-integration state (directory, host, user, user variables, command history). The app runs the commands and draws the indicators. `egui_host.rs` factors the egui window plumbing out of the settings window, so a small prompt window can ask for a mark name. `notify.rs` shows a Windows notification (a tray balloon) for "Post Notification".

**Tech Stack:** as milestone 3, plus the vendored alacritty_terminal and windows-sys `Win32_UI_Shell`.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory: `docs/parity/03-features-and-menus.md` (shell integration; captured output, annotations, marks and navigation; section 2 rows for OSC 7, 133, 1337 keys), `docs/parity/01-profile-settings.md` (Show Mark Indicators, Place Prompt at First Column, Custom Directory Recycle). Milestones 4b (find, Smart Selection, Semantic History, links) and 4c (copy mode, folding, annotations, notes, timestamps, cursor guide, the other Edit tools) get their own plans.

## Global Constraints

- Everything from milestones 0 to 3 still holds.
- iTerm2 facts come from the research pass recorded in the ledger (VT100Terminal.m 4839-5006, VT100ScreenMutableState+TerminalDelegate.m 2543-2814, VT100ScreenMutableState.m 3482-4856, PTYSession.m 8592, 10565-10669, 14138-14166, 16636-16767, iTermTextDrawingHelper.m 1231-1370, iterm2_shell_integration.bash/zsh).
- Every OSC is also fed to alacritty unchanged; Tandem only watches. OSC 8 (hyperlinks) stays alacritty's.
- Marks are stored on rows (the patch), never as line numbers.
- A command that needs a later milestone stays disabled; "Post Notification" works through `notify.rs`.

## Review Focus

1. An OSC split across two reads (`ESC ] 1 3` | `3 ; A BEL`). Expect: one prompt mark. Test: Task 2 `an_osc_split_across_reads_is_one_event`.
2. A long `ls` after a mark, so the mark scrolls into history and then out of it. Expect: navigation finds it while in history and drops it cleanly after. Test: Task 1 `a_mark_moves_into_history_with_its_row` and Task 3 `a_mark_that_leaves_history_is_forgotten`.
3. Narrowing and widening the window with a wrapped command line under a prompt mark. Expect: the mark stays on the prompt's row. Test: Task 1 `a_mark_survives_reflow`.
4. A PSReadLine prompt redraw on every key (the prompt re-sends 133;A at the same place). Expect: one mark, not one per key. Test: Task 3 `a_repeated_prompt_start_is_one_mark`.
5. Ctrl+C at a prompt (D with no C). Expect: the half-made command mark is removed. Test: Task 3 `an_aborted_command_removes_its_mark`.

---

### Task 1: Row marks in alacritty

**Files:** `crates/alacritty_terminal/` (vendored 0.26.0, tests removed), `crates/alacritty_terminal/TANDEM-PATCHES.md`, `Cargo.toml` (`[patch.crates-io]`), `Cargo.lock`; tests in `src/screen.rs`.

**Interfaces:** Produces `Row::mark() -> u32`, `Row::set_mark(u32)` (on `alacritty_terminal::grid::Row`); `Screen::set_row_mark(line: i32, id: u32)`, `Screen::row_marks() -> Vec<(i32, u32)>` (line in grid coordinates: negative is history, top first).

- [ ] **Step 1: Failing tests** (`src/screen.rs`):

```rust
#[test]
fn a_mark_moves_into_history_with_its_row() {
    let mut s = Screen::new(20, 3);
    s.feed(b"prompt> ");
    s.set_row_mark(0, 7);
    for i in 0..5 { s.feed(format!("\r\nout{i}").as_bytes()); }
    let marks = s.row_marks();
    assert_eq!(marks.len(), 1);
    let (line, id) = marks[0];
    assert_eq!(id, 7);
    assert!(line < 0, "{line}");
    assert!(s.line_text(line).starts_with("prompt>"));
}

#[test]
fn a_mark_survives_reflow() {
    let mut s = Screen::new(10, 5);
    s.feed(b"$ ");
    s.set_row_mark(0, 3);
    s.feed(b"abcdefghijklmnop\r\nnext");
    s.resize(6, 5);
    s.resize(30, 5);
    let (line, id) = s.row_marks()[0];
    assert_eq!(id, 3);
    assert!(s.line_text(line).starts_with("$ abcdefghijklmnop"));
}

#[test]
fn a_cleared_row_loses_its_mark() {
    let mut s = Screen::new(20, 3);
    s.set_row_mark(1, 5);
    s.feed(b"\x1b[H\x1b[J");
    assert!(s.row_marks().is_empty());
}
```

Run `scripts/xt test --lib screen::`. Expected: FAIL (no `set_row_mark`).

- [ ] **Step 2: Implement** `set_row_mark`, `row_marks` (scan every row from the top of history), `line_text(line) -> String`. Step 3: PASS. Commit `Vendor alacritty_terminal with row marks`.

---

### Task 2: OSC scanner

**Files:** Create `src/osc.rs`.

**Interfaces:** Produces:
- `pub enum Osc { Prompt { kind: PromptKind, aid: Option<String>, fresh_line: bool }, CommandStart, OutputStart, CommandEnd { code: Option<i32>, aid: Option<String> }, Location(String) /* OSC 7 URL */, Kv(String, String) /* OSC 1337 key, value */ }`, `pub enum PromptKind { Initial, Secondary, Continuation, Right, Unknown }`
- `#[derive(Default)] pub struct Scanner` with `scan(&mut self, bytes: &[u8]) -> Vec<(usize, Osc)>`: each event with the offset just past its terminator (BEL or ESC \\) in `bytes`. State carries across calls. Only payloads that start with `133;`, `7;`, or `1337;` are kept (up to 64 KiB; longer ones are dropped); everything else is skipped without buffering.
- `pub fn parse(payload: &str) -> Option<Osc>` per VT100Terminal executeFinalTermToken (A and N fresh line, P not; `k=` kinds; B; C; D with the first integer positional argument as the code, bare D ignored) and executeXtermSetKvp (split at the first `=`).

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn finds_133_7_and_1337() {
    let mut s = Scanner::default();
    let ev = s.scan(b"a\x1b]133;A;aid=1\x07b\x1b]7;file://h/x\x1b\\c\x1b]1337;CurrentDir=/tmp\x07");
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[0].0, 15);
    assert!(matches!(&ev[0].1, Osc::Prompt { kind: PromptKind::Initial, aid: Some(a), fresh_line: true } if a == "1"));
    assert!(matches!(&ev[1].1, Osc::Location(u) if u == "file://h/x"));
    assert!(matches!(&ev[2].1, Osc::Kv(k, v) if k == "CurrentDir" && v == "/tmp"));
}

#[test]
fn an_osc_split_across_reads_is_one_event() {
    let mut s = Scanner::default();
    assert!(s.scan(b"\x1b]13").is_empty());
    let ev = s.scan(b"3;A\x07rest");
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, 4);
}

#[test]
fn other_oscs_are_skipped() {
    let mut s = Scanner::default();
    assert!(s.scan(b"\x1b]0;title\x07\x1b]8;;http://x\x1b\\").is_empty());
}

#[test]
fn command_end_takes_the_first_integer() {
    assert!(matches!(parse("133;D;2;aid=4"), Some(Osc::CommandEnd { code: Some(2), aid: Some(a) }) if a == "4"));
    assert!(matches!(parse("133;D;aid=4"), Some(Osc::CommandEnd { code: Some(0), .. })));
    assert!(parse("133;D").is_none());
    assert!(matches!(parse("133;A;k=s"), Some(Osc::Prompt { kind: PromptKind::Secondary, .. })));
    assert!(matches!(parse("133;P"), Some(Osc::Prompt { fresh_line: false, .. })));
    assert!(matches!(parse("1337;SetMark"), Some(Osc::Kv(k, v)) if k == "SetMark" && v.is_empty()));
}
```

Run `scripts/xt test --lib osc::`. Expected: FAIL. Step 2 implement, Step 3 PASS. Commit `Add an OSC scanner for shell integration`.

---

### Task 3: Marks and shell integration state

**Files:** Create `src/marks.rs`; modify `src/screen.rs` (`feed` splits at scanner events).

**Interfaces:** Produces in `tandem::marks`:
- `pub struct Mark { pub id: u32, pub kind: Kind, pub command: String, pub code: Option<i32>, pub start: Option<SystemTime>, pub end: Option<SystemTime>, pub aid: Option<String>, pub name: Option<String> }`, `pub enum Kind { Prompt, Manual }` (a named mark is a Manual mark with a name)
- `pub struct Shell { pub cwd: Option<String>, pub host: Option<String>, pub user: Option<String>, pub shell: Option<String>, pub version: Option<u32>, pub vars: BTreeMap<String, String>, pub history: Vec<CommandRecord> }`, `pub struct CommandRecord { pub command: String, pub cwd: Option<String>, pub host: Option<String>, pub start: SystemTime, pub end: Option<SystemTime>, pub code: Option<i32> }`
- `pub fn color(m: &Mark) -> MarkColor` (`Success` blue for code 0 or none, `Other` yellow for 128-160, `Error` red otherwise)
- On `Screen`: `pub marks: Marks` (records by id, `last_command_mark`, `output_range`, `alert_next`), `pub shell: Shell`, `fn apply(&mut self, osc)`, `pub fn marks_in_order(&self) -> Vec<(i32, &Mark)>`, `pub fn set_manual_mark(&mut self, line: i32, name: Option<String>) -> u32`, `pub fn last_output_range(&self) -> Option<(Point, Point)>`, `pub fn current_command_range(&self) -> Option<(Point, Point)>`, `pub fn take_mark_alert(&mut self) -> bool` (a mark was made while `alert_next` was set)
- Behavior (research notes): A (initial or unknown kind) makes a prompt mark on the cursor row, after CR LF when `fresh_line`, the cursor is past column 0, and the profile's Place Prompt at First Column is on (`Screen::place_prompt_first_column`); a repeated A on the same row with no command yet reuses the mark; the previous command mark gets its end time. B records where the command starts. C takes the command text from there to the cursor, sets the start time, adds a history record, and starts the output range. D sets the code and end time on the aid's mark or the last command mark; D while still in the command (no C) removes that mark. OSC 7 (`file://user@host/path?machineID=...`, percent-decoded) sets host, user, and cwd; without `machineID` it also makes a prompt mark. 1337 `CurrentDir`, `RemoteHost` (split on the last `@`), `SetUserVar` (base64 value; a name with `.` is ignored), `ShellIntegrationVersion` (`n;shell=x`), and `SetMark` (a manual mark on the cursor row). Records whose row mark is gone are pruned on each `marks_in_order`.

- [ ] **Step 1: Failing tests** (`src/marks.rs`, through `Screen`):

```rust
fn prompt(s: &mut Screen, cmd: &str, code: i32) {
    s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    s.feed(cmd.as_bytes());
    s.feed(b"\r\n\x1b]133;C\x07output\r\n");
    s.feed(format!("\x1b]133;D;{code}\x07").as_bytes());
}

#[test]
fn a_command_becomes_a_mark_with_its_text_and_code() {
    let mut s = Screen::new(40, 10);
    prompt(&mut s, "ls -l", 2);
    let marks = s.marks_in_order();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].1.command, "ls -l");
    assert_eq!(marks[0].1.code, Some(2));
    assert_eq!(color(marks[0].1), MarkColor::Error);
    assert_eq!(s.shell.history[0].command, "ls -l");
}

#[test]
fn a_repeated_prompt_start_is_one_mark() {
    let mut s = Screen::new(40, 10);
    s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    s.feed(b"\r\x1b[K\x1b]133;A\x07$ \x1b]133;B\x07l");
    assert_eq!(s.marks_in_order().len(), 1);
}

#[test]
fn an_aborted_command_removes_its_mark() {
    let mut s = Screen::new(40, 10);
    s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07abc^C\r\n\x1b]133;D;130\x07");
    assert!(s.marks_in_order().is_empty());
}

#[test]
fn a_mark_that_leaves_history_is_forgotten() {
    let mut s = Screen::with_scrollback(20, 3, 5);
    prompt(&mut s, "x", 0);
    for i in 0..20 { s.feed(format!("line{i}\r\n").as_bytes()); }
    assert!(s.marks_in_order().is_empty());
}

#[test]
fn output_of_the_last_command_is_its_range() {
    let mut s = Screen::new(40, 10);
    prompt(&mut s, "echo hi", 0);
    s.feed(b"\x1b]133;A\x07$ ");
    let (a, b) = s.last_output_range().unwrap();
    assert_eq!(s.text_between(a, b).trim_end(), "output");
}

#[test]
fn location_reports_set_the_directory_host_and_user() {
    let mut s = Screen::new(40, 10);
    s.feed(b"\x1b]7;file://rob@box/home/rob/my%20dir?machineID=1:\x07");
    assert_eq!((s.shell.user.as_deref(), s.shell.host.as_deref(), s.shell.cwd.as_deref()), (Some("rob"), Some("box"), Some("/home/rob/my dir")));
    assert!(s.marks_in_order().is_empty());
    s.feed(b"\x1b]1337;RemoteHost=a@b@c\x07\x1b]1337;SetUserVar=gitBranch=bWFpbg==\x07");
    assert_eq!((s.shell.user.as_deref(), s.shell.host.as_deref()), (Some("a@b"), Some("c")));
    assert_eq!(s.shell.vars["gitBranch"], "main");
}

#[test]
fn osc7_without_a_machine_id_is_also_a_prompt() {
    let mut s = Screen::new(40, 10);
    s.feed(b"\x1b]7;file://box/tmp\x07$ ");
    assert_eq!(s.marks_in_order().len(), 1);
}

#[test]
fn prompts_start_in_the_first_column() {
    let mut s = Screen::new(40, 10);
    s.feed(b"no newline\x1b]133;A\x07$ ");
    let (line, _) = s.marks_in_order()[0];
    assert!(s.line_text(line).starts_with("$ "));
}
```

Run `scripts/xt test --lib marks::`. Expected: FAIL. Step 2 implement (`Screen::with_scrollback` sets the history limit; `text_between` reads grid text). Step 3 PASS. Commit `Track shell-integration marks, command text, and location`.

---

### Task 4: Egui host and the prompt window

**Files:** Create `src/egui_host.rs`, `src/prompt_ui.rs`; modify `src/settings_ui.rs` (uses the host), `src/lib.rs`.

**Interfaces:** Produces `EguiHost` (window, surface, egui state, renderer) with `open(event_loop, gpu handles, title, size) -> Result<EguiHost, String>`, `event(&WindowEvent) -> bool`, `paint(&mut self, f: impl FnOnce(&mut egui::Ui)) -> egui output handled`, `id()`, `focus()`, `hwnd()`. `PromptWindow` with `open(.., title, label, initial)` and `redraw() -> PromptResult { Open, Cancel, Ok(String) }`: one text field, OK and Cancel, Enter is OK and Escape is Cancel.

- [ ] Pure test (`prompt_ui.rs`): `a_named_mark_defaults_to_the_time` (`default_mark_name(SystemTime)` formats `yyyy-MM-dd HH:mm:ss` in local time, as iTerm2's sheet does). Step 2: refactor `SettingsWindow` onto `EguiHost` with no behavior change (the existing settings tests stay green). Commit `Factor the egui window host out of Settings; add a prompt window`.

---

### Task 5: Mark commands, alerts, and notifications

**Files:** Create `src/notify.rs`; modify `src/commands.rs` (DONE list, owner of Post Notification), `src/keyaction.rs` (action 71 Done), `schema/global.toml` (`Mark Alert Action` enum "Post Notification" / "Modal Alert" / "Ring Bell", default "Post Notification"; `Alert On Marks in Offscreen Sessions` bool false), `src/app/menu.rs`, `src/app/mod.rs`, `Cargo.toml` (windows-sys `Win32_UI_Shell`).

**Interfaces:** `notify::show(hwnd, title, body) -> Result<(), String>` (Shell_NotifyIconW balloon from a tray icon Tandem adds and later removes). App commands:
- `saveScrollPosition:` Set Mark: a manual mark on the top visible row.
- `addNamedMark:` Set Named Mark: the prompt window, default the local time; empty cancels.
- `jumpToSavedScrollPosition:` Jump to Mark: the last mark of any kind to the top of the view.
- `nextMark:` / `previousMark:`: from the current mark position (reset when a mark is added), wrapping. A command mark goes to the top of the view and its command is selected; another mark is centered.
- `clearToLastMark:`: clears from the line after the last mark above the cursor that is not the current prompt's (from the mark's own line when it is directly above the cursor).
- `selectOutputOfLastCommand:`, `selectCurrentCommand:`: the ranges from Task 3.
- `toggleAlertOnNextMark:` (check mark), `toggleAlertOnMarksInOffscreenSessions:` (check mark, global), the three alert choices (radio check marks on `Mark Alert Action`). When a mark is made with the alert set (or in a pane that is not in the active tab, with the offscreen option on): Modal Alert shows `Mark set in session "<title>".`; Ring Bell plays the error sound if the previous command failed, else the default sound (MessageBeep); Post Notification shows "Mark Set" / "Session <title> #<n> had a mark set." Key action 71 toggles the same flag.
- Pure test (`app/menu.rs` or a helper module): `next_mark_wraps_both_ways` over a list of lines (`next_line(lines: &[i32], from: Option<i32>, forward) -> Option<i32>`), `clear_to_last_mark_picks_the_line_after` (`clear_start(marks: &[i32], prompt_line: Option<i32>, cursor: i32) -> Option<i32>`).

- [ ] Steps: failing helper tests, helpers, wiring, full suite, commit `Run the mark commands and mark alerts`.

---

### Task 6: Mark indicators

**Files:** Modify `src/scene.rs`, `src/app/mod.rs`.

**Interfaces:** `scene::mark_bars(frame, marks: &[(row_on_screen, MarkColor)], inner: Rect, cell, pad)`: a bar in the left padding of the row, blue (0.538, 0.757, 1.0), yellow (0.857, 0.847, 0.426), or red (0.987, 0.448, 0.426), when the pane's profile has Show Mark Indicators on or the mark has a name.

- [ ] Pure test (`scene.rs`): `mark_bars_sit_in_the_left_padding` (the quad's x is inside the pad, its y is the row's). Commit `Draw mark indicators in the left margin`.

---

### Task 7: Install Shell Integration and the pwsh script

**Files:** Create `resources/shell_integration/tandem_shell_integration.ps1`; modify `src/app/menu.rs`, `src/commands.rs`.

**Interfaces:**
- The script: a `prompt` wrapper that emits, per prompt, `ESC]133;D;<exit>;aid=<old>BEL` (after the first prompt), `ESC]7;file://<user>@<host><pct-path>?machineID=1:BEL`, `ESC]133;A;aid=<new>BEL`, the original prompt text, and `ESC]133;B;aid=<new>BEL`; a PSReadLine Enter handler that emits `ESC]133;C;aid=<new>BEL` before accepting the line; `ESC]1337;ShellIntegrationVersion=1;shell=pwsh BEL` once. The exit code is 0 when `$?` is true, else `$LASTEXITCODE` or 1. Works in Windows PowerShell 5.1 and pwsh 7 (`[char]27`, `[char]7`).
- `installShellIntegration:`: asks (MessageBox, Yes/No) and then, by the focused pane's command line: pwsh or powershell: writes the script to `%APPDATA%\Tandem\shell_integration\`, and types a command that adds `. "<path>"` to `$PROFILE` if missing and loads it now; bash, zsh, fish, or WSL: types iTerm2's `curl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash`; cmd: says cmd has no prompt hooks and suggests pwsh.
- Pure test: `install_command_depends_on_the_shell` (`install_plan(cmdline) -> Plan { Pwsh, Curl, Unsupported }`). Integration test (`pane.rs`, the Windows test machine only): `the_pwsh_script_emits_prompt_marks` runs `pwsh -NoProfile -NoLogo -Command ". '<script>'; prompt"` in a pane and checks that the screen got a prompt mark and a directory.

- [ ] Commit `Add Install Shell Integration and the PowerShell script`.

---

### Task 8: Reuse previous session's directory

**Files:** Modify `src/profile.rs`, `src/app/mod.rs`.

**Interfaces:** `profile::start_dir_with(p, previous: Option<&Path>) -> (PathBuf, Option<String>)`: "Recycle" uses `previous` when it is a directory, else home. The app passes the focused pane's shell-integration directory when its host is this machine (or not reported), for a new tab, split, or window.

- [ ] Pure tests: `recycle_uses_the_previous_directory`, `recycle_falls_back_home`. Commit `Reuse the previous session's directory`.

---

### Task 9: Parity and acceptance

- [ ] Update the rows this milestone closes (03 shell integration, marks and navigation, section 2 OSC rows; 01 Show Mark Indicators, Place Prompt at First Column, Custom Directory; 02 Mark Alert Action if present), recount `00-roadmap.md`, and write `docs/acceptance/m4a-checklist.md` (install in pwsh and WSL, marks in the margin with colors, Win+Shift+Up/Down, Set Mark, Set Named Mark, Jump to Mark, Clear to Last Mark, Select Output of Last Command, each alert kind, a new tab in the same directory). Commit.
