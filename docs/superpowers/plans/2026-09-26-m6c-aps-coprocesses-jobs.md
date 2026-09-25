# Tandem Milestone 6c: The Foreground Job, Coprocesses, and Automatic Profile Switching

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's foreground-job features (the job variables, the Job title components, Job Started and Job Ended event triggers, the per-trigger Job field, Prompt Before Closing with Jobs to Ignore), coprocesses (Session > Run Coprocess... and Stop Coprocess, the Run Coprocess and Run Silent Coprocess triggers, key action 35, the Smart Selection, Semantic History, and context-menu coprocess actions, `fileDropCoprocess`, the indicator, the stderr banner), and Automatic Profile Switching (Bound Hosts rules, scoring, the profile stack, Prevent Automatic Profile Switching, the banner, and a log).

**Architecture:** Pure models decide everything that has a rule: `proctree.rs` (a snapshot of processes, then the display job and its ancestors with iTerm2's depth rules), `jobtitle.rs` (the session title from Title Components), `close_prompt.rs` (whether closing needs a confirmation and its text), `coprocess.rs` (the per-pane state, the MRU, the ignore-errors list, the entry-point policy, the stderr cap), and `aps.rs` (rule parsing, scoring, and the profile stack). The Windows pieces are thin: a per-pane Job Object with a completion port (and a Toolhelp poll as the fallback) feeds `proctree`; a `cmd.exe /d /s /c` launcher with pipes runs coprocesses; `Pane::pump` copies raw output to the coprocess before the screen sees it.

**Tech Stack:** as milestone 6a, plus `windows-sys` features for Job Objects, Toolhelp, and I/O completion ports.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `01-profile-settings.md` Title Components, Prevent Automatic Profile Switching, Prompt Before Closing 2, Jobs to Ignore, Bound Hosts, the coprocess trigger, Smart Selection, and Semantic History rows; `02-global-settings.md` KEY_ACTION_RUN_COPROCESS, fileDropCoprocess, disableTopRightIndicators, showAutomaticProfileSwitchingBanner, preserveFontSizeOnAutomaticProfileSwitch, fastForegroundJobUpdates; `03-features-and-menus.md` Session > Run Coprocess... and Stop Coprocess, APS, the Run Coprocess trigger. iTerm2 facts: `.superpowers/research/m6c-coprocess-job.md` (all sections) and `.superpowers/research/m6-iterm2-facts.md` sections 2.6 and 6.

## Global Constraints

- Everything from milestones 0 to 6b still holds.
- Windows has no process groups: the foreground job is the deepest console-subsystem descendant of the pane's shell, ties to the newest; depth is capped at 50; a cycle or a stale parent (a child created before its parent) gives no link.
- `jobName` is the image name without a trailing ".exe"; `processTitle` is argv0 as typed; `commandLine` is the full line. The root shell is not in `foregroundJobAncestors`.
- Job names match case-insensitively, with or without ".exe" (the ignore list, the trigger Job field, Job Started and Job Ended). APS job globs stay case-sensitive as in iTerm2 and are tested against the name and the command line.
- Jobs to Ignore seeds `ssh`, `telnet`, `plink` into every profile (iTerm2 seeds `rlogin, ssh, slogin, telnet`); `conhost`, `OpenConsole`, and the root shell are always ignored. Prompt Before Closing defaults to Never.
- The prompt texts, verbatim: `A session with profile "X" is running a, b, and c.` (more than 3: `a, b, c, and N other job(s).`), footer "You can change this setting in Settings > Profiles > Session", and `The profile "X" always requires confirmation.`
- A coprocess is `cmd.exe /d /s /c <command>`, hidden, with pipes, in its own Job Object that kills its tree on close. It reads the raw pty output before parsing and triggers; its output goes to the pty through `Pane::reply` as if typed. Its folder is the pane's known folder, else the user's home.
- One coprocess per pane on every path: the menu item is disabled, triggers show "<title> trigger: Can't run two coprocesses at once." with Silence Warning (once per session), and the other paths refuse (iTerm2 replaces without stopping; Tandem refuses and records it as a ruling).
- Silent (mute) drops the output from the screen while the coprocess runs; the coprocess still gets every byte.
- The coprocess stderr keeps 100000 characters, then "-- output truncated --". The MRU keeps 20 commands (`Coprocess MRU`); the ignore list is `NoSyncCoprocessCommandsToIgnoreErrorOutput`.
- APS follows `RULE` and `APS` exactly (weights, squash, sticky `!`, `&job`, path with a trailing `/`, the stack, return to the original profile when nothing matches). Expression rules `{...}` score by the interpolated value, with a call refused as in 6a.
- New sessions start with the local host name (`GetComputerNameExW` DNS full) and user (`GetUserNameW`), so host rules work without shell integration.

## Review Focus

1. A shell that starts a GUI program (`start notepad`) and a deep chain of shells. Expect: the GUI program is not the job; the deepest console program is; no cycle hangs. Test: Task 1 `gui_children_are_not_the_job` and `cycles_and_stale_parents`.
2. A coprocess that prints nothing and never exits, then the pane closes. Expect: the coprocess tree ends (Job Object), no thread stays blocked, no zombie cmd.exe. Test: Task 5 `pane_close_ends_the_coprocess` (model: the Stop path runs on close).
3. A coprocess that floods its stdout. Expect: input to the pty is bounded (10 KB pending), the window stays responsive. Test: Task 5 `coprocess_output_is_bounded`.
4. APS with two profiles scoring the same, and a path rule with a trailing `*`. Expect: iTerm2's tie rule (first wins, strictly greater needed), `/foo/bar/*` matches `/foo/bar`. Test: Task 7 `scores_follow_iterm2`.
5. Job Started for a job whose name repeats in the ancestry (cmd inside cmd). Expect: Started fires once for the name while it is present, Ended when the last one leaves. Test: Task 3 `job_events_diff_names`.

---

### Task 1: The process tree and the foreground job

**Files:** Create `src/proctree.rs`; modify `src/pty.rs` (a per-pane Job Object through `PROC_THREAD_ATTRIBUTE_JOB_LIST`, `Pty::pid()`), create `src/jobwatch.rs` (the completion-port thread, the Toolhelp snapshot, command lines through `NtQueryInformationProcess`), `Cargo.toml` features.

**Interfaces:** `Proc { pid, ppid, name, created, gui }`; `display_job(procs, root) -> Option<Job { name, title, cmdline, pid }>` (deepest console descendant, newest on ties, depth 50, stale links dropped); `ancestors(procs, root) -> Vec<String>` (lowercased, deepest first, root excluded); `strip_exe(name)`; `JobWatch::new(notify)` posts `UserEvent::JobsChanged` (debounced 16 ms) and polls at 0.5 s focused and 5 s unfocused when events are not available.

- [ ] Failing tests: `deepest_console_descendant_wins`, `gui_children_are_not_the_job`, `cycles_and_stale_parents`, `ancestors_exclude_the_root`, `exe_is_stripped`. Commit `Find the foreground job`.

### Task 2: Job variables and the title

**Files:** Create `src/jobtitle.rs`; modify `src/app/prompted_ui.rs` (the `jobName`, `processTitle`, `commandLine`, `jobPid`, `deepestJob`, `foregroundJobAncestors` variables), `src/app/requests_ui.rs` (tab labels), `src/app/mod.rs` (JobsChanged).

**Interfaces:** `title(components: u32, parts: TitleParts { session_name, profile_name, job_title, job_cmdline, user, host, path, tty }) -> String`, a port of `iTermSessionTitleBuiltInFunction.m:340-400` (the job as " (job)" after a name, or alone; Job and Job with Arguments exclusive).

- [ ] Failing tests: `title_components_follow_iterm2`, `job_alone_or_in_parentheses`, `job_variables_interpolate`. Commit `Show the job in titles`.

### Task 3: Job triggers

**Files:** modify `src/trigger_events.rs` (Job Started 113 and Job Ended 114 fire), `src/trigger_eval.rs` (the Job field gate), `src/settings_triggers.rs` (remove the 6c notes).

**Interfaces:** `job_events(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>)` (a set diff, case-insensitive, original case kept for captures); a trigger with `eventParams.jobName` fires only on an equal name (empty never fires); a trigger's `job` gate passes only when the lowercased name (with or without ".exe") is in the ancestors.

- [ ] Failing tests: `job_events_diff_names`, `job_filter_is_exact`, `job_field_gates_triggers`. Commit `Fire job triggers`.

### Task 4: Prompt before closing

**Files:** Create `src/close_prompt.rs`; modify `schema/profile.toml` (Jobs to Ignore seed), `src/app/mod.rs` (pane, tab, and window close ask first).

**Interfaces:** `close_reason(mode, ignore: &[String], profile: &str, jobs: &[String]) -> Option<String>` with the texts in Global Constraints; `jobs` is every descendant of the shell (not only the foreground one), built-ins removed; the window and tab close collect the reasons of their panes into one question.

- [ ] Failing tests: `close_reason_texts`, `ignore_list_matches_names`, `never_and_always`. Commit `Ask before closing running jobs`.

### Task 5: The coprocess core

**Files:** Create `src/coprocess.rs` (model) and `src/coproc_win.rs` (launcher, reader threads); modify `src/pane.rs` (the tee and the mute in `pump`, output through `reply`, stop on close).

**Interfaces:** `Coprocess::start(command, mute, cwd, env) -> Result<Coprocess, String>`; `write(&[u8])` (bounded, drops nothing: pauses reading the pty chunk copy beyond 1 MB pending with a note); `poll() -> CoEvent { Output(Vec<u8>), Ended { stderr } }`; `stop()`; `Stderr` capped at 100000 characters then "-- output truncated --"; `Mru::add` (20, newest first, deduplicated); `Policy::start(existing: bool, entry) -> Allowed | Refused(Option<String>)`.

- [ ] Failing tests: `mru_keeps_twenty`, `stderr_is_capped`, `one_coprocess_per_pane`, `mute_hides_output_from_the_screen`, `coprocess_output_is_bounded`, `pane_close_ends_the_coprocess`, Windows `coprocess_round_trip` (run `cmd /c more` style echo through pipes). Commit `Run coprocesses`.

### Task 6: Coprocess UI

**Files:** Create `src/app/coprocess_ui.rs`; modify `schema/menus.toml` and `src/commands.rs` (Run Coprocess... and Stop Coprocess), `src/app/triggers_ui.rs`, `src/app/semantic_ui.rs`, `src/keyaction.rs`, the context menu, `docs/parity/03-features-and-menus.md` (the Stop shortcut is Opt+Cmd+Shift+R).

**Interfaces:** the Run Coprocess dialog (label "Coprocess command to run:", an editable list with the MRU, "Silently ignore errors from this command", OK and Cancel, a blank command beeps); Stop in the Session and context menus while one runs; the top-right indicator with the tooltip "A coprocess is running." (hidden by `disableTopRightIndicators`); the banner `Coprocess "<cmd>" terminated with output on stderr.` with View Errors (a temp file in Notepad) and Ignore Errors from This Command; the trigger banner with Silence Warning (`NoSyncSuppressCoprocessTriggerWarning`); key action 35, Smart Selection and Semantic History coprocess actions, and `fileDropCoprocess` (mute, `\(filenames)`).

- [ ] Pure tests `stderr_banner_text`, `trigger_refusal_text`, `file_drop_command`. Commit `Coprocess menus and indicator`.

### Task 7: Automatic Profile Switching

**Files:** Create `src/aps.rs` and `src/app/aps_ui.rs`; modify `src/settings_*` (the Bound Hosts list editor with its checks and help), `src/app/requests_ui.rs` (host, user, path, and job changes call APS), `src/session_profile.rs` (stack entries).

**Interfaces:** `Rule::parse(&str) -> Rule`; `Rule::score(&Facts { host, user, path, job, cmdline }) -> f64`; `best(profiles) -> Option<(guid, sticky)>`; `Stack::decide(current, facts) -> Option<Switch>` with the push, pop, sticky reset, and trim-to-original rules; Prevent Automatic Profile Switching stops it; the banner `Switched to profile "X".` (with `showAutomaticProfileSwitchingBanner`); font size kept with `preserveFontSizeOnAutomaticProfileSwitch`; the debug log checkbox writes to a Tandem log window (the Script Console is milestone 11).

- [ ] Failing tests: `rules_parse_like_iterm2`, `scores_follow_iterm2`, `stack_push_pop_and_trim`, `sticky_resets_the_stack`, `prevent_stops_switching`, `editor_rejects_duplicates`. Commit `Switch profiles automatically`.

### Task 8: Parity and acceptance

- [ ] Update the rows 6c closes, recount, write `docs/acceptance/m6c-checklist.md`. Commit.
