# Tandem Milestone 14r: Rename the project to Termurion (TWEAKS items 3 and 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope (read first):** `docs/TWEAKS.md` items 2 and 3 (verbatim requirements); the pre-flight `.superpowers/sdd/tweaks/termurion-preflight.md` (8,752 case-insensitive "tandem" hits in 789 tracked files, groups A to F, risks 1 to 12, the 10-task split this plan refines). Item 1 (dark menu bar) is a separate plan and is not part of this milestone.

**Pre-start gate (Robert's, not the executor's):** a USPTO search for TERMURION in classes 9 and 42, and registration of the domains he wants (termurion.com first; .dev, .app, .io, .org optional), are Robert's to do. **No task of this plan starts until Robert confirms both in writing.** Task 0 only records that confirmation. If he rejects the name, this plan is void and nothing in the repo has changed.

**Goal:** every place the person or Windows sees the product says Termurion (window, menu bar and its application menu, About, dialogs, notifications, tray, taskbar, Jump List, shortcuts, uninstall entry, exe names, registry, pipes, data folders, repo), an existing Tandem profile keeps working untouched (settings, history, scripts, passwords, sealed approvals and autofill, default-terminal handoff, URL schemes), and the docs, parity files, scripts and remotes agree.

**Non-goals (iTerm2 compatibility stays, risk 2 of the pre-flight "Keep" list):** plist keys and defaults domain names (`com.googlecode.iterm2`), the `.itermtab` extension, OSC 1337 and every other escape sequence, TERM_PROGRAM `iTerm.app`, the Python API module and names (`iterm2`, `it2*`), menu command selectors, `LICENSE-iTerm2` / `COPYING-iTerm2` and third-party notices, parity-doc references to "iTerm2 source", and the recorded tmux fixtures `tests/fixtures/tmux/*.bin` (their bytes contain `/mnt/c/Users/user2/src/tandem`). Historical plans and rulings text under `docs/superpowers/plans` and `docs/superpowers/rulings` stay as written records. The English phrase "in tandem" stays. No new features, no behaviour change beyond names and migration.

**Architecture:** one exact-string table (Task 1) drives every rename; a dry-run tool lists each hit and its table row before anything is written, and a diff against a backup tree is the check after (never a blanket sed). Names a user or another program can still hold (old env vars, old pipe and host names for one release, old Credential Manager targets, old `.tandemexport`) get read-side aliases; new writes use the new name. Migration is copy-once and non-destructive. The menu root keeps an internal id so saved key bindings do not break.

**Tech Stack:** as milestones 0 to 13y2. No new crates. Tests run on the Windows test machine with `scripts/xt test --lib <filter>`; the build is `scripts/xt build --release`. Win32 code stays `#[cfg(windows)]`; characters outside ASCII are `\u{...}` escapes.

## Global Constraints

- Everything from milestones 0 to 13y2 still holds. This milestone changes names, not behaviour.
- **DPAPI entropy stays byte-identical:** `KEY_ENTROPY = b"tandem workgroup approval key\0v1"` (`src/workgroup_restore.rs:557`) and `CARD_ENTROPY = b"tandem autofill contact card\0v1"` (`src/browser_autofill.rs:323`) are part of the seal. They are never table rows; each gets a comment "legacy name, part of the seal, do not rename", and a test pins the bytes (Task 5).
- **Data folders:** `%APPDATA%\Tandem` and `%LOCALAPPDATA%\Tandem` are migrated once by COPY (never move) to `Termurion`, before any settings, terminfo, history, Recents, Scripts, uv or AIChatWire read; a marker file records the migration; the old folder is kept until Robert confirms (a later release or an explicit action removes it). The install folder `%LOCALAPPDATA%\Programs\Tandem` is replaced by `...\Programs\Termurion` by the installer.
- **Registry:** `Software\Tandem\*` is migrated to `Software\Termurion\*`, including the default-terminal pair `Software\Tandem\PreviousDefaultTerminal`, `Capabilities`, `URLAssociations`, `RegisteredApplications`, `Software\Classes\Applications\tandem.exe` and the `Tandem.<scheme>` ProgIds. **The CLSID GUID `{6A1E8D3C-...}` is unchanged** (`CLSID_TANDEM_TERMINAL` is renamed in code only); `LocalServer32` is rewritten to the new exe through default_term.rs's existing "moved exe" fixer.
- **Credential Manager:** targets `Tandem/...`, `Tandem` and `Tandem-Browser` (`pwm_source.rs:15,22`, `pwm_credman.rs`) are read under both prefixes; new writes use `Termurion/...`; an old entry found is copied to the new target, and the old one is deleted only after the copy reads back equal.
- **Host and launch pipes:** `tandem-host.exe` becomes `termurion-host.exe`, and the host pipe and launch pipes (`tandem-url-*`, `tandem-ssh-probe-*`, `tandem-ssh-secrets-*`, `tandem-bonjour-*`) change together in one task. For one release the new app also looks for an old-named host or launch pipe, and the new host answers on both names; the installer refuses to run while any Tandem process is running (already true), so no rename happens mid-session.
- **Aliases for one release:** `.tandemexport` is imported as well as the new `.termurionexport` (export writes the new); the `TANDEM_*` environment variables (`TANDEM_UI`, `TANDEM_SCRIPT`, `TANDEM_SSH_TRANSPORT`, `TANDEM_PAGE`, `TANDEM_T`, `TANDEM_*_CHILD`, `TANDEM_RESIZE_*`, `TANDEM_WAIT_PID`) are read as aliases when the `TERMURION_*` one is unset; `.tandem-tmp` and `settings.json.tandem-backup` sweeps cover both names; the old shell-integration file name keeps working until the installer rewrites it.
- **Menu root:** `schema/menus.toml`'s top menu keeps an internal id `iTerm2` (the path root used as a key in `src/bindings.rs`, `app/menu.rs`, `arrange_ui.rs`, `dlog_ui.rs`, `suppressed_app.rs` and the other 14 files with `iTerm2 >`); only the DISPLAYED title and the displayed items (About, Hide, Quit, Make Default Term) change, through a `title` field or a display map. Saved bindings stored as `iTerm2 > ...` load unchanged; a `Termurion > ...` path is accepted as an alias.
- **Version:** patch bump only (the second number is Robert's call). The Windows firewall, Defender, SmartScreen, Start Menu pins and Jump List pins see a new identity; the release note says so.
- Do not blanket-replace in code: exact strings from the Task 1 table, dry run, diff against the backup tree. Source-scan tests (`include_str!`) and the `localfs.rs:911` manifest are edited in the same commit as the module they scan.
- Nothing secret is printed. Do not launch the GUI except in Task 11. Tests that touch the registry, Credential Manager or DPAPI use a test-only key or target prefix and clean up in a `Drop` guard.

## Review Focus

1. The table is the only source of renames: every changed line in the diff maps to a table row, and no "iTerm2 compatibility" key from the Non-goals list changed. Test: Task 1 `rename_table_is_exact`, Task 9 `compat_keys_unchanged`.
2. Seals survive: a blob sealed by the old build unseals in the new one, and the two entropy constants equal the legacy bytes. Test: Task 5 `dpapi_entropy_is_legacy`, `old_sealed_blobs_unseal`.
3. Data migration is copy-once, marked, ordered before every reader, and leaves the old folder. Test: Task 5 `profile_migration_copies_once`, `migration_runs_before_readers`, `migration_on_a_real_profile_copy`.
4. Registry and defaults: default-terminal pair and url schemes survive; CLSID unchanged. Test: Task 4 `registry_migrates_default_terminal`, `clsid_guid_unchanged`.
5. Credentials: found under either prefix. Test: Task 5 `credman_reads_both_prefixes`.
6. Pipes and host change together, with the one-release fallback. Test: Task 3 `host_and_launch_pipes_change_together`, `old_host_pipe_is_found`.
7. Aliases: `.tandemexport`, `TANDEM_*`. Test: Task 3 `old_export_extension_imports`, `old_env_vars_are_aliases`.
8. Item 2: no displayed "iTerm2" or "Tandem" anywhere on the menu bar or its menus; saved `iTerm2 > ...` bindings still resolve. Test: Task 7 `menu_displays_product_name`, `menu_root_id_stays`.
9. User-visible text: no "Tandem" in any string the person can read. Test: Task 6 `no_tandem_in_user_strings`.
10. Living docs and tooling agree with the code. Test: Task 10 `living_docs_say_termurion`.

---

### Task 0: Pre-start gate

**Files:** none changed in the repo; the confirmation is recorded in `.superpowers/sdd/tweaks/termurion-gate.txt` by the controller.

- [ ] **Step 1:** ask Robert whether he has (a) run the USPTO TESS search for TERMURION in classes 9 (software) and 42 (SaaS/software design) and found no conflicting mark, and (b) registered the domains he wants. These are his to do; the executor does neither.
- [ ] **Step 2:** only on his written yes, create the gate file with the date and his words, then start Task 1. Without it, stop; no later task may run.
- Acceptance: gate file exists and quotes Robert's confirmation of both items.

### Task 1: String table, dry-run tool, backup

**Files:** Create `tools/rename_table.toml` (rows: `group`, `old`, `new`, `mode` = `exact` or `word`, `scope` globs, `reason`); create `tools/rename_dryrun.py` (lists every match of `tandem` case-insensitive with its table row or "UNMAPPED", writes no file); create `tools/rename_apply.py` (applies only mapped rows to the files a task names, refuses UNMAPPED hits in them, writes a diff against `.rename-backup/`); modify `.gitignore` for `.rename-backup/`.

Table groups follow the pre-flight: A user strings, B identifiers (exe, registry, pipes, env, JS globals, extensions, Credential Manager), C data paths, D code identifiers (crate, fns, types, consts), E docs and scripts, F assets. Rows that must be present as KEEP (mode `keep`, never applied): the two entropy byte strings, the CLSID GUID, the tmux fixture path, "in tandem", `LICENSE-*`/third-party text, and every Non-goals key. Historical docs globs are marked `keep-history`.

- [ ] **Step 1: failing tests** in `tools/test_rename_table.py` (run with python3, no GUI): `rename_table_is_exact` (every row's `old` is a literal, no regex, no empty string, no `old` that is a prefix of a KEEP row without the KEEP row winning; KEEP rows win ties); `dryrun_lists_all_hits` (the dry run's total over a fixture tree equals `git grep -ci tandem` for it); `apply_refuses_unmapped` (a fixture file with an unmapped hit makes `rename_apply.py` exit non-zero and change nothing).
- [ ] **Step 2:** run them; expect failure (tool absent).
- [ ] **Step 3:** write the table from the pre-flight counts (A to F), the dry-run and apply tools, and run the dry run over the real tree: the report must show 0 UNMAPPED outside `keep-history` paths. Add rows until that holds. Commit the dry-run report under `.superpowers/sdd/tweaks/termurion-dryrun.txt`.
- [ ] **Step 4:** copy the tree to `.rename-backup/` (untracked) before Task 2.
- [ ] **Step 5:** commit `rename: exact-string table and dry-run tool (m14r)`.
- Acceptance: tests pass; dry run reports 0 UNMAPPED; KEEP rows cover the entropy strings, the CLSID, the fixtures and the compat keys.

### Task 2: Crate, binaries and build

**Files:** Modify `Cargo.toml` (name `termurion`, bin names `termurion`, `termurion-host`, `termurion-git`, `termurion-utils`), `Cargo.lock`, `build.rs` (panic text), `src/bin/tandem.rs`, `tandem-host.rs`, `tandem-git.rs`, `tandem-utils.rs` (rename the files), every `use tandem::...` and `tandem::` path in `src`, `tests`, the adapter binaries; crates `alacritty_terminal`, `glyphon`, `vte` patch docs (`TANDEM-PATCHES.md` becomes `TERMURION-PATCHES.md`, patch marker comments updated via the table; no behaviour change in the patched crates).

- [ ] **Step 1: failing test** `crate_and_bins_are_renamed` (a `#[test]` reading `Cargo.toml` with `include_str!`): package name is `termurion`; the four bins exist with the new names; no `[[bin]]` still says `tandem`. Also `adapters_link_only_pure_modules` keeps the existing rule (adapters `use termurion::` only `pwm_*`, `ai_transport`, `ai_winhttp`).
- [ ] **Step 2:** `scripts/xt test --lib crate_and_bins` expect failure.
- [ ] **Step 3:** apply table groups B (exe names) and D (crate, paths) to these files with `rename_apply.py`; `git mv` the bin files; fix `include_str!` paths in source-scan tests in the same commit per module (mouse_ui.rs:744, resident_ui.rs:400, others from `git grep include_str`).
- [ ] **Step 4:** `scripts/xt build --release` and the full `scripts/xt test --lib`; fix only rename fallout.
- [ ] **Step 5:** commit `rename: crate and binaries are termurion (m14r)`.
- Acceptance: release build succeeds producing `termurion.exe`, `termurion-host.exe`, `termurion-git.exe`, `termurion-utils.exe`; full `--lib` green; the tmux fixtures byte-identical (`git diff --stat tests/fixtures` empty).

### Task 3: Runtime identifiers and aliases (pipes, host, env, extensions, classes, ids, JS)

**Files:** Modify `src/ssh_probe_win.rs`, `src/url_launch.rs`, the host launch code and `src/bin/termurion-host.rs` (pipe and host names), `src/app/hotkey_ui.rs` (window class), `src/semantic_ui.rs`, `src/find_ui.rs`, `src/marks_ui.rs` (`tandem.ctx.*`, `tandem.findmode.`, `tandem.finderase`, `tandem.browsermark.`, `tandem.namedmark.` become `termurion.*`), `src/settings_export_ui.rs` (`.termurionexport`), `src/dynamic_profiles_app.rs` (`.termurion-tmp`), `src/app/files_ui.rs` (probe name), the env-var reads (new helper `src/envname.rs`: `fn var(name_suffix) -> Option<String>` reads `TERMURION_<x>` then `TANDEM_<x>`), `resources/browser/*.js` and `browser_js.rs`/`browser_pages.rs` (`__tandemClient`, `__tandemPost`, `__TandemOsc`, `data-tandem-*`, `tandem-glyph`, `//tandem.test/` origin, `tandem-cloak.js`, `{{TANDEM_UI}}` placeholder), `resources/shell_integration/tandem_shell_integration.ps1` (new file name plus the six installer references), the Python API wire names only after Step 1 proves whether `tandem.join`, `tandem_command`, `by_tandem` are protocol-visible (if a wire field, keep and alias; if internal, rename).

- [ ] **Step 1: failing tests:** `host_and_launch_pipes_change_together` (one const table: host pipe and every launch pipe prefix start with `termurion-`; a test building both from the same function); `old_host_pipe_is_found` (a fake listener on the old pipe name is connected to when the new one is absent, and the new host also serves the old name); `old_env_vars_are_aliases` (`TANDEM_UI` set, `TERMURION_UI` unset: `envname::var("UI")` returns it; both set: new wins); `old_export_extension_imports` (a `.tandemexport` file imports; export writes `.termurionexport`); `wire_names_audit` (lists `tandem.join`, `tandem_command`, `by_tandem` hits and asserts each is classed internal or wire in a table in the test); `menu_ids_alias_both_ways` (a stored `tandem.ctx.copy` id resolves to the new one); `shell_integration_old_name_still_works` (the old file name is listed as a legacy name the installer rewrites).
- [ ] **Step 2:** `scripts/xt test --lib envname`, `pipes`, `export` etc.; expect failures.
- [ ] **Step 3:** implement with the table; keep legacy constants named `LEGACY_*` with a comment; resolve the pre-flight open question about AppUserModelID and mutex names by `git grep -n -i "AppUserModelID\|CreateMutex\|SetCurrentProcessExplicitAppUserModelID"`: if the app sets none, Task 8 adds an explicit `Termurion.Terminal` AppUserModelID so the taskbar and Jump List group stably, and this task records the finding.
- [ ] **Step 4:** run filters, full `--lib`, release build.
- [ ] **Step 5:** commit `rename: identifiers, pipes and aliases (m14r)`.
- Acceptance: tests above green; `git grep -n -i '"tandem[-_.]' src` returns only `LEGACY_*` lines and KEEP rows.

### Task 4: Registry, default terminal and URL schemes

**Files:** Modify `src/url_schemes.rs` (`Software\Termurion\Capabilities`, `...\URLAssociations`, ProgIds `Termurion.<scheme>`, `Software\Classes\Applications\termurion.exe`, the RegisteredApplications entry), `src/default_term.rs` (`CLSID_TANDEM_TERMINAL` becomes `CLSID_TERMURION_TERMINAL`, GUID value unchanged; `Software\Termurion\PreviousDefaultTerminal`; display name "Termurion Terminal Handoff"), `src/handoff_win.rs` (lines near 30 and 334), new `src/registry_migrate.rs` (pure plan builder plus `#[cfg(windows)]` applier).

- [ ] **Step 1: failing tests** (pure, on the plan builder with an in-memory fake registry; a Windows test uses a unique `Software\TermurionTest-<pid>` root and a `Drop` guard):
  - `registry_migrates_default_terminal`: with `Software\Tandem\PreviousDefaultTerminal` holding a pair, the plan copies it to `Software\Termurion\PreviousDefaultTerminal` byte for byte, rewrites `LocalServer32` to the new exe path, deletes the old `Capabilities`, `RegisteredApplications` entry and `Tandem.<scheme>` ProgIds, and rebinds `ssh` and other schemes that pointed at Tandem to `Termurion.<scheme>`; schemes Tandem did not own are untouched.
  - `clsid_guid_unchanged`: the const equals `{6A1E8D3C-...}` as in `main` before the rename (the literal is copied from `git show HEAD~:src/default_term.rs`, pinned in the test).
  - `migration_is_idempotent`: running the plan twice changes nothing the second time.
  - `old_exe_gone_is_fixed`: a default-terminal registration whose `LocalServer32` names a missing `tandem.exe` is repaired to the new exe.
- [ ] **Step 2:** run `scripts/xt test --lib registry_migrate` and `default_term`; expect failures.
- [ ] **Step 3:** implement; the applier runs once at startup (after Task 5's profile migration, before URL scheme use) and writes a marker value `Software\Termurion\Migrated=1`; the old `Software\Tandem` tree is left except the keys named above that would shadow the new ones.
- [ ] **Step 4:** run filters, full `--lib`, release build.
- [ ] **Step 5:** commit `rename: registry, default terminal and url schemes (m14r)`.
- Acceptance: tests green; on a clean fake registry nothing is written when `Software\Tandem` is absent.

### Task 5: Data-folder, Credential Manager and seal migration

**Files:** Create `src/profile_migrate.rs` (pure plan: `plan(old_root, new_root, state) -> Vec<Step>`; applier copies files, writes `migrated-from-tandem.json` marker with date, version, file count and byte count); modify `src/app/mod.rs:4120` (call before settings load), `src/localfs.rs:911` (manifest of judged paths gets the new folder and keeps the old as legacy, and its source-scan test), `src/history_ui.rs:11`, `src/jumplist_ui.rs:26`, `src/settings_export_ui.rs:191` (`appdata_tandem` becomes `appdata_termurion`), `src/script_runtime_ui.rs:75`, `src/ai_wire_log.rs:246`, `src/ai_discovery.rs:683`, `src/adblock_app.rs:79`, `src/browser_extensions_app.rs:93`, `src/code_review_app.rs:760`, `src/app/menu.rs:777`, `src/pwm_source.rs`, `src/pwm_credman.rs`, `src/workgroup_restore.rs:41,557` and `src/browser_autofill.rs:323` (comments only, bytes untouched).

- [ ] **Step 1: failing tests:**
  - `dpapi_entropy_is_legacy`: `KEY_ENTROPY == b"tandem workgroup approval key\0v1"` and `CARD_ENTROPY == b"tandem autofill contact card\0v1"` (literals typed in the test).
  - `old_sealed_blobs_unseal` (Windows): a blob sealed with the legacy bytes (made in the test with `CryptProtectData` and the literal, also a checked-in fixture sealed on the Windows test machine by the pre-rename build, `tests/fixtures/dpapi/legacy-approval.bin`, created in Task 5 Step 3 from `main` before Task 2 if available, else generated in-test) unseals through the new code path, for both the approval key and the autofill card.
  - `profile_migration_copies_once`: old `Tandem\` present, `Termurion\` absent: after migration both exist, contents equal, marker present, old untouched; second run does nothing; if `Termurion\` already exists with files, nothing is copied and the old folder is left (no merge, no overwrite).
  - `migration_runs_before_readers`: a source scan asserts the call precedes the first settings read in startup order.
  - `migration_on_a_real_profile_copy`: the test takes `TERMURION_TEST_PROFILE` (a copy of Robert's real `%APPDATA%\Tandem` made by the executor on the Windows test machine with `robocopy`, never the live folder) into a temp root, migrates, and checks: file counts and a SHA-256 over every file equal, settings load through the real parser with zero warnings, `Recents\*.itermtab` listed, the Scripts folder and `uv` dir present, `pbhistory.json` parses. The test is `#[ignore]` unless the variable is set; the executor runs it with the variable set and records the output.
  - `credman_reads_both_prefixes` (Windows, test prefix): an entry written under the old `Tandem/` target is returned by the new reader; writing stores `Termurion/`; the old entry is deleted only after the copy reads back equal; a failed copy leaves the old entry.
  - `judged_paths_manifest_lists_both`: `localfs.rs` manifest names `Termurion` and legacy `Tandem`.
- [ ] **Step 2:** run `scripts/xt test --lib profile_migrate`, `workgroup_restore`, `browser_autofill`, `pwm_credman`; expect failures.
- [ ] **Step 3:** implement; roaming-redirected or UNC `%APPDATA%` is a judged path (use the existing localfs judgement; if the target is unreachable skip migration and keep reading the old folder). Report the migration in the log, never the file contents.
- [ ] **Step 4:** run filters, full `--lib`, release build; run the real-profile-copy test and keep its output in `.superpowers/sdd/tweaks/termurion-migration-run.txt`.
- [ ] **Step 5:** commit `rename: data, credential and seal migration (m14r)`.
- Acceptance: all tests green including the real-profile-copy run; no write to the live `%APPDATA%\Tandem` during any test.

### Task 6: User-visible strings (item 3 text)

**Files:** Modify (table group A) `src/app/mod.rs:6993` (window title), `src/emu_ui.rs:163`, `src/seltools_ui.rs:61`, MessageBox captions (`app/files_ui.rs:113,120`, `app/menu.rs:1671`, `app/mod.rs:1810`, `pwm_app.rs:2732` plus its test at `2956`, `ai_engage_ui.rs:127`), About text (`app/menu.rs:763`: "Termurion {ver}\nAn iTerm2-style terminal for Windows." keeps "iTerm2-style" as an honest description, ruling recorded), notifications (`app/launch_ui.rs:97`), HTTP User-Agent (`ai_winhttp.rs:430`, `ai_wire_log.rs:400`, `app/browser_app.rs:215`), `scripts/gen-tips.py:25` (map `("iTerm2","Termurion")`) then regenerate `resources/tips.toml`, `schema/global.toml` (79 lines), `src/settings_keys.rs`, `advanced_pane.rs`, `api_auth.rs`, `ai_prompts`/`ai_tools` prompts, `resources/ai/prompts`, `welcome-page.html`, `code-review-settings.txt`, `ai_turn.rs:22`, `browser_pages.rs`, `browser_settings_page.rs`, `browser_js.rs`, `update.rs` (display text; release URLs move to `datbird/termurion/releases` in Task 9), `jumplist_ui.rs`/`jumplist_win.rs` strings, tray and tooltips. Also the "User text is iTerm2's verbatim with iTerm2 replaced" rule: new text rule is "iTerm2" replaced by "Termurion".

- [ ] **Step 1: failing test** `no_tandem_in_user_strings`: a source-scan over `schema/*.toml`, `resources/**` (except KEEP rows and third-party notices) and string literals passed to the UI/MessageBox/notify/title/tooltip functions (an allowlist file `tools/user_string_scan.txt` for KEEP hits such as the pinned entropy bytes) finds no `Tandem`/`tandem` (case-insensitive) outside the allowlist; plus targeted tests: `window_title_is_product_name`, `about_text_names_product` (starts "Termurion "), `user_agent_is_termurion` (`Termurion/<ver>`; the browser suffix likewise), `tips_regenerated` (running `scripts/gen-tips.py` reproduces `resources/tips.toml` byte for byte).
- [ ] **Step 2:** run the filters; expect failures.
- [ ] **Step 3:** apply group A rows with `rename_apply.py` file by file; regenerate tips with `gen-tips.py`, never hand edit; update `pwm_app.rs` CAPTION test with the constant.
- [ ] **Step 4:** run filters, full `--lib`, release build.
- [ ] **Step 5:** commit `rename: every user-visible string says Termurion (m14r)`.
- Acceptance: `no_tandem_in_user_strings` green; `git diff` shows only table-mapped lines.

### Task 7: Item 2, the menu bar shows the product name

**Files:** Modify `schema/menus.toml` (top menu keeps internal id `["iTerm2"]`, gains `title = "Termurion"`; item display texts: "About Termurion", "Hide Termurion", "Quit Termurion", "Make Termurion Default Term"; selectors unchanged), `src/app/menu.rs` (build the HMENU captions from the display title; the "Also install iTerm2 Utilities?" box becomes "Also install Termurion Utilities?"; the comment at `:122`), `scripts/gen-menus.py` (emit and read the display title), `src/bindings.rs`, `arrange_ui.rs`, `dlog_ui.rs`, `suppressed_app.rs` and the other `iTerm2 >` files (resolve both `iTerm2 > X` and `Termurion > X`, store the internal form), `schema/global.toml` and the settings panes that show "iTerm2" as a label, `welcome-page.html`, the Help menu text, browser-settings leftovers found by the audit.

- [ ] **Step 1: failing tests:** `menu_displays_product_name` (the built top-level caption is "Termurion"; the captions of About, Hide, Quit, Make Default Term contain "Termurion" and not "iTerm2"; no other menu caption contains "iTerm2" or "Tandem"); `menu_root_id_stays` (the internal id still reads `iTerm2`; a stored binding `iTerm2 > Preferences...` and `Termurion > Preferences...` both resolve to the same command; saved settings written back use the internal form); `compat_keys_list` (a test listing the iTerm2 strings that must remain, from the Non-goals: plist domain, `.itermtab`, OSC 1337 strings, `iTerm.app`, selectors; asserts each is still present in its file); `no_displayed_iterm2` (scan of `schema/*.toml` display fields and dialog literals: every remaining "iTerm2" hit is in the compat allowlist).
- [ ] **Step 2:** run the filters; expect failures.
- [ ] **Step 3:** implement the display-title layer; regenerate with `gen-menus.py`; walk the pre-flight Section 2 "Show" list and the audit items (menus.toml, dialogs, settings labels, welcome page, Help menu) and record each decision in `docs/superpowers/rulings/m14r-rulings.txt` (created here).
- [ ] **Step 4:** run filters, full `--lib`, release build.
- [ ] **Step 5:** commit `menus: the application menu shows Termurion (m14r, TWEAKS 2)`.
- Acceptance: tests green; keep-list recorded; TWEAKS item 2 moves to Done in Task 10.

### Task 8: Installer, shortcuts, taskbar identity and Jump List

**Files:** Modify `scripts/install-windows.ps1` (install dir `%LOCALAPPDATA%\Programs\Termurion`, exe names, `Termurion.lnk` on Desktop and Start Menu, description, messages, uninstall entry name and DisplayName under the Uninstall key, `Get-Process termurion`; it also removes the old `Tandem.lnk`, the old Start Menu entry and old Uninstall entry, and the old install dir only after the new exe launches `--version`), `src/jumplist_win.rs` and `src/jumplist_ui.rs` (AppUserModelID `Termurion.Terminal`, set explicitly at process start before any window, Recents folder name, task names, the "Tandem Recents" folder via the migration), `src/app/mod.rs` (call `SetCurrentProcessExplicitAppUserModelID`), the shortcut writer (PKEY_AppUserModel_ID on the `.lnk` equals the process AppUserModelID), the exe icon resources and `build.rs` version info (ProductName, FileDescription, InternalName, OriginalFilename say Termurion).

- [ ] **Step 1: failing tests:** `appusermodelid_is_termurion` (const equals `Termurion.Terminal`, one source for process and `.lnk`); `shortcut_uses_same_aumid`; `versioninfo_strings_say_termurion` (source scan of build.rs/resource strings); `installer_script_names` (a Pester-free check in Python reading `install-windows.ps1`: contains no unmapped `Tandem`, mentions `Termurion.lnk`, removes the legacy `Tandem.lnk`, uninstall DisplayName "Termurion"; file stays ASCII-only); `jumplist_tasks_say_termurion`.
- [ ] **Step 2:** run the filters and the python check; expect failures.
- [ ] **Step 3:** implement; the installer keeps refusing to run while any `tandem*` or `termurion*` process is running.
- [ ] **Step 4:** run filters, full `--lib`, release build; `powershell -NoProfile -File scripts/install-windows.ps1 -WhatIf`-style dry run if the script supports it, otherwise only the Python check (the real install is Task 11).
- [ ] **Step 5:** commit `rename: installer, shortcuts and taskbar identity (m14r)`.
- Acceptance: tests green; script parses (`powershell -NoProfile -Command "[scriptblock]::Create((Get-Content scripts/install-windows.ps1 -Raw))"` on the Windows test machine).

### Task 9: Repo, GitHub, scripts, the Windows test machine folders and update URLs

**Files:** Modify `scripts/xt` (lines 7, 21, 22: remote folder `termurion`, worktrees `termurion-*`, vendor copy from `%USERPROFILE%\src\termurion`), `scripts/probe-desktop.ps1` (Repo path, `termurion-probe-*` task and dir), `scripts/record-conductor.py` (USER, prefix), `scripts/gen-tips.py`, `scripts/gen-menus.py`, `tools/protogen` (temp names), `src/update.rs` (releases URLs `datbird/termurion/releases`, 11 hits), `.superpowers/*` helper scripts that hardcode the path, the git remote on each host.

Ordered steps (this task touches live machines, so each step is verified before the next and the controller runs the GitHub rename; the executor does not ask Robert to rename anything by hand):

- [ ] **Step 1: failing tests:** `xt_paths_say_termurion` (Python scan of `scripts/xt`: no `tandem` outside KEEP/legacy comments); `update_urls_say_termurion` (every GitHub URL in `update.rs` has `datbird/termurion`; a test that the old `datbird/tandem` URL is still listed as a fallback for the one-release redirect); `compat_keys_unchanged` (the Task 7 keep-list still passes after all tasks).
- [ ] **Step 2:** run them; expect failures.
- [ ] **Step 3:** edit scripts and `update.rs` via the table. Then, in this order: (a) on the Windows test machine `Rename-Item %USERPROFILE%\src\tandem termurion` is NOT done in place: create the new checkout `%USERPROFILE%\src\termurion` by cloning after the GitHub rename, keep the old folder until Task 11 passes (it holds `target\release` and `vendor`; copy `vendor` first to save a rebuild), (b) rename the GitHub repo `datbird/tandem` to `datbird/termurion` with `gh repo rename` (GitHub redirects the old URL), (c) update `origin` to `git@github-datbird:datbird/termurion.git` on the Linux dev machine (`~/gitrepos/tandem` becomes `~/gitrepos/termurion` by `mv` after the working tree is clean and pushed), a second Linux machine, and the Windows test machine, (d) `scripts/xt` smoke: `scripts/xt build --release` from the new folder.
- [ ] **Step 4:** run filters, full `--lib`, release build from the new the Windows test machine folder.
- [ ] **Step 5:** commit `rename: repo, scripts and update urls (m14r)`; push to the renamed origin.
- Acceptance: `git remote -v` on all three hosts shows `termurion.git`; `scripts/xt build --release` works from the new path; the old folder is not deleted yet (Task 11 deletes it after the probe passes).

### Task 10: Docs, parity, rulings, README, logo, memory

**Files:** Modify `README.md` (29 hits), `docs/TWEAKS.md` (items 2 and 3 move to Done with date and the commit hashes; item 1 stays Open), `docs/parity/00-roadmap.md`, `01`, `02`, `03` (living text only, "Tandem" to "Termurion" via the table; "iTerm2" source references stay), `docs/acceptance/*` checklist headings, the milestone 14 roadmap sentence, `docs/assets/tandem-logo.svg` to `docs/assets/termurion-logo.svg` (text in the logo updated; the README image link too; if the logo spells the word, redraw it by editing the SVG text only), `docs/superpowers/specs/2026-09-23-tandem-design.md` renamed to `2026-09-23-termurion-design.md` with every link to it updated (a historical plan's link to the old filename gets a one-line redirect note rather than an edit), `progress.md`, `docs/probe`, `.superpowers/sdd-templates/*`. Historical plans and rulings (2,914 and 1,283 hits) are NOT edited, except one header note added to `docs/superpowers/plans/README` (or `docs/superpowers/README.md`) saying "Plans and rulings before 2026-10-20 use the project's former name, Tandem." Memory notes live outside the repo (`project-tandem.md`, `tandem-session-handoff.md`, MEMORY.md index): the controller updates them as the last step of the milestone, not the executor.

- [ ] **Step 1: failing test** `living_docs_say_termurion` (an `include_str!` scan of README, `docs/TWEAKS.md` Done entry, `docs/parity/*`, `docs/acceptance/*` headings: no `Tandem` except inside a quoted former-name note, the milestone 14r line, or the allowlist); `spec_link_resolves` (every markdown link to `specs/2026-09-23-*` points at an existing file); `logo_file_exists`; `rulings_file_exists` (`m14r-rulings.txt` has one entry per decision below).
- [ ] **Step 2:** run `scripts/xt test --lib docs_audit` (or the existing doc-audit filter from 13y2); expect failures.
- [ ] **Step 3:** apply with `rename_apply.py` to the named living files only; `git mv` the logo and spec; record decisions in `m14r-rulings.txt`: history left as written; "iTerm2-style terminal" kept in About; menu root internal id; DPAPI entropy legacy; one-release aliases and when they end (the milestone after next, Robert's call); old folders kept until confirmed.
- [ ] **Step 4:** run filters and the full `--lib` (the 13y2 audit test reads these docs).
- [ ] **Step 5:** commit `docs: Termurion rename, living docs and parity (m14r)`.
- Acceptance: tests green; the 13y2 parity audit still passes.

### Task 11: Desktop probe on the Windows test machine and manual checklist

**Files:** Create `docs/acceptance/m14r-termurion-checklist.md` (the manual checklist below); modify `scripts/probe-desktop.ps1` only if Task 9 left it incomplete; evidence goes under `docs/probe/m14r/`.

Warn Robert before driving his desktop (he is at the machine); the probe runs under the `termurion-probe-*` task and a scratch profile folder, never his live profile, except the one real-profile migration step he approves.

- [ ] **Step 1:** on the Windows test machine install with the new `install-windows.ps1` from the new folder. Before it, take `robocopy` copies of `%APPDATA%\Tandem` and `%LOCALAPPDATA%\Tandem` to `%USERPROFILE%\termurion-probe-backup\` (kept). Record `Get-ItemProperty` of the Uninstall, `Software\Tandem` and CLSID keys before and after.
- [ ] **Step 2: probe run** (`scripts/probe-desktop.ps1`, screenshot per step with the repo's capture tool): first run migrates (marker present, old folder intact); the window title, menu bar and application menu read Termurion; About reads "Termurion <ver>"; a Credential Manager password saved by the old build is found; a workgroup approval and an autofill card sealed by the old build read; a pane from the old host is NOT expected (the installer refused while running) but a fresh `termurion-host.exe` session starts and survives restart; `ssh://` link opens in Termurion; default-terminal handoff works if Termurion is set as default; `Get-Process` shows only `termurion*`.
- [ ] **Step 3: string scan:** run a case-insensitive scan of `termurion.exe`, `termurion-host.exe`, `termurion-git.exe`, `termurion-utils.exe` (PowerShell with a byte search for UTF-8 and UTF-16LE "tandem") and the installed resources; the only hits allowed are the two DPAPI entropy strings, `LEGACY_*` names, the fixture-free compat strings in the allowlist and the license notices. Record the hit list in `docs/probe/m14r/strings.txt`.
- [ ] **Step 4: manual checklist** (Robert ticks each; screenshots attached where marked):
  - [ ] Taskbar: icon and hover title say Termurion; pin, unpin, re-pin works; no second duplicate group; the old pinned Tandem shortcut is gone or flagged broken (release note).
  - [ ] Jump List: right-click shows Termurion tasks and Recents (migrated `.itermtab` entries present).
  - [ ] Shortcuts: Desktop and Start Menu `Termurion.lnk` launch it; `Tandem.lnk` removed; its AppUserModelID matches the taskbar group.
  - [ ] Settings > Apps > Installed apps (and `appwiz.cpl`): one entry "Termurion", publisher and icon right, Uninstall removes the install dir and shortcuts but leaves the data folder; no "Tandem" entry remains.
  - [ ] Notifications: a toast (for example a long command finishing, or a bell) is titled Termurion, and its Settings > Notifications entry says Termurion.
  - [ ] Window title, tray, About box, the menu bar (Termurion, Shell, Edit, View, Session, Scripts, Profiles, Window, Help), Quit/Hide wording.
  - [ ] Existing key bindings, profiles, history, scripts, snippets and the settings export/import (`.tandemexport` imports; export writes `.termurionexport`).
  - [ ] Passwords, workgroup approvals, autofill card, browser pages and extensions still work.
  - [ ] Windows Firewall / Defender / SmartScreen prompts appear once for the new exe path and are accepted (noted in the release note).
  - [ ] `ssh`, `http` handler, default terminal still bound to Termurion.
- [ ] **Step 5: clean up:** only after Robert ticks the checklist, the controller removes `%USERPROFILE%\src\tandem` (after confirming nothing runs from it), the old `tandem-probe-*` scheduled tasks and folders, and the stray `tandem-11e`-style worktrees; the old `%APPDATA%\Tandem` and `%LOCALAPPDATA%\Tandem` folders stay until Robert says to remove them. Update the memory notes and handoff (controller). Commit `docs: m14r desktop probe and checklist`; push.
- Acceptance: probe evidence and string-scan list committed; checklist fully ticked by Robert or each unticked item filed in `docs/TWEAKS.md` as a new Open item.

---

## Decisions (each goes in `m14r-rulings.txt`)

- Historical plans and rulings keep "Tandem". Cost: grep for the old name still finds them; the plans README says why.
- About keeps "An iTerm2-style terminal for Windows." Cost: iTerm2 is named in the product description.
- The DPAPI entropy strings stay "tandem ...". Cost: the old name survives in two byte strings no person sees.
- Old env vars, `.tandemexport`, old host/launch pipe names and old Credential Manager targets are read for one release. Cost: dead aliases to remove later.
- The old data folders are never deleted by the app. Cost: duplicate data on disk until Robert removes it.
- The menu root keeps the internal id `iTerm2`. Cost: a saved bindings file still says `iTerm2 > ...`.
- The Windows security identity is new (path, AppUserModelID); Defender, firewall and pins are re-established once. Cost: one-time prompts.

## Pitfalls for the implementer

- "tandem" appears in English ("in tandem") and in binary fixtures; the dry run flags them and the table keeps them.
- Never rename `tests/fixtures/tmux/*.bin` content or the two entropy byte strings; a table row hitting them is a table bug.
- Source-scan tests fail on rename; edit them in the same commit as their module.
- The migration must run before the first read of any Termurion folder; a late reader that creates an empty `Termurion\` first makes the copy skip (guard: treat a folder with only an empty skeleton as absent, test `empty_new_folder_is_not_a_migration_block`).
- Do not rename the host while sessions run; do not run the installer while any `tandem*` or `termurion*` process exists.
- `.ps1` files are ASCII-only; JS placeholders like `{{TANDEM_UI}}` are matched by the injection code, so rename both sides in one commit.
- `pkill -f` and `Stop-Process` by pattern can match the shell that runs them; kill by PID.
