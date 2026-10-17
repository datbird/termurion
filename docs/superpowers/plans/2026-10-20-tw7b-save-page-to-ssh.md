# Tandem Tweak 7b: Browser Save Page As to an SSH destination (TWEAKS item 7b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope (read first):** `docs/TWEAKS.md` item 7b; `docs/superpowers/rulings/m13x4-rulings.txt` (R13x4 local folders only, R13x4-r2-2 and -3 for `write_new_checked`, R13x4-final-2 caps, R13x4-final-3 Abandon); `src/page_saver.rs` (the saver job, `MAX_RESOURCES` 2000, `MAX_TOTAL_BYTES` 256 MiB, `MAX_RESOURCE_BYTES` 8 MiB, `destination`, `Step`); `src/inert_files.rs` (`write_new_checked`, `mark_strict`, `opened_name`); `src/ssh_files.rs` and `src/app/ssh_files_app.rs` (the SSH file owner: `ScpPath`, `can_transfer`, `upload_refused`, `UPLOAD_MAX`, `ssh_upload`, `Then::SshUpload`, `SshKeepOrReplace`).

**Build order:** after the Termurion rename (m14r). Do not start before it lands; names in the code move, the design here does not.

**Goal:** from a browser pane that belongs to a session on an SSH host (an `Io::Ssh` pane whose profile has SSH file transfer allowed), Save Page As offers "Remote folder..." and the saved page (index.html plus its resource folder, up to 2000 files and 256 MiB) lands in a folder on that host. Local saving is unchanged.

**Non-goals:** saving from a page to a host Tandem has no SSH session to; resuming a half-saved upload; syncing; any new SSH library or crate (the existing owner is the only transport); saving to a UNC path.

**Architecture (decisions Robert needs not re-make):**
1. **Stage locally, then upload.** The saver already builds the whole tree on disk through `write_new_checked`. The remote save runs the same job into a private staging folder under `%LOCALAPPDATA%\Tandem\SaveStage\<uuid>\` (created with create-new, deleted deepest first at the end, on every outcome, and swept at startup for folders older than a day). Nothing the saver guarantees is re-implemented for the remote side; the upload is a second, separate phase that reads the finished staging tree. Cost: disk use up to 256 MiB and a short delay before the first byte leaves.
2. **Mark of the Web is replaced by a refused-extension rule plus inert names, not a stream.** A remote filesystem has no `Zone.Identifier`. What stands in: every name the saver would neutralize locally (`inert_files::high_risk`, `NEUTRAL_SUFFIX` `.download`) is neutralized on the remote side too, `page_saver::refused_extension` names are never uploaded, the uploaded files get mode 0644 and folders 0755 (never executable), and the confirm text says plainly that the host will not mark the files as downloaded from the web. The local staging copy is still written marked and strict, so a drive that keeps no streams still fails the save as today (the staging folder is local).
3. **Create-new on the remote.** The destination folder is created with `mkdir` (fails if it exists; the saver's "unique name" rule picks `name`, `name (2)`, ... by asking the host for the listing first). No file is overwritten: each file is uploaded to `<name>.part` and renamed with `mv -n` semantics (a rename that fails when the target exists). A refusal leaves the remote untouched except for folders this save made.
4. **Cleanup mirrors R13x4-final-3.** A cancelled, failed or pane-closed upload removes, deepest first, only the files and folders THIS save created (a ledger of created remote paths, never a path that existed), each folder only if empty. A finished upload clears the ledger.
5. **One owner.** All remote work goes through `ssh_files` and the existing channel of the session (the same framing and `can_transfer` gate as Upload with SCP). No second connection, no `scp.exe`, no shell string: file names travel as the owner already sends them (base64 chunks, `Chunk`), never spliced into a command.

## Global Constraints

- Everything from milestones 0 to 13y2 and the Termurion rename still holds.
- A remote path is judged by `ssh_files::scp_path` only; the remote folder text is shown through `inert_files::shown_name` and never executed.
- Caps: the saver's (2000 resources, 256 MiB, 8 MiB each) bind the staging; the upload adds one cap, `REMOTE_MAX_FILES` = 2000 including the folders it makes, and refuses a tree over it before the first `mkdir`.
- The upload runs off the UI thread; its progress is one banner (`save-remote`), updated at most 4 per second; Cancel stops after the file in flight and runs the cleanup.
- Names: `inert_files::opened_name` applies to the remote name as well (a name the saver would cut or neutralize locally is cut or neutralized the same way), and a name with a path separator, `..`, a control character or an empty name is refused.
- No emoji, no em dashes. Nothing secret is printed. Do not launch the GUI except in the last task.

## Review Focus

1. The remote tree equals the staged tree, file for file, and every name equals the local saver's for the same page. Test: `remote_names_equal_local_names`.
2. Nothing is overwritten and nothing is left: a name that already exists on the host stops that file (the save reports it), a cancel or a dropped connection removes only what this save made. Tests: `upload_never_replaces`, `cleanup_removes_only_ledger`.
3. No shell string anywhere: a file named `a; rm -rf ~` or `$(x)` uploads as that literal name or is refused by the name rule. Test: `hostile_names_stay_data`.
4. The staging folder is always removed, on success, failure, cancel and a crash (startup sweep). Test: `staging_removed_every_way`.
5. The 2000-file cap and the byte cap hold on the remote side exactly as on the local one. Test: `remote_caps_match_the_saver`.
6. Mark of the Web stand-in: executable names neutralized, mode 0644/0755, the confirm text names the difference. Test: `remote_files_are_inert`.
7. The picker never lists more than a bounded number of entries and never follows a link out of what it shows. Test: `picker_is_bounded`.

---

### Task 1: Pure planner for the remote save

**Files:** Create `src/page_save_remote.rs` (pure, `pub mod`), tests inline; modify `src/lib.rs`.

- [ ] **Step 1: failing tests** `remote_names_equal_local_names`, `remote_caps_match_the_saver`, `hostile_names_stay_data`, `remote_files_are_inert`: given a staged tree listing (`Vec<(rel_path, size, is_dir)>`) the planner returns an ordered `Vec<Op>` (`Mkdir(path)`, `Put{rel, tmp, size, mode}`, `Rename{from, to}`), refuses names by `opened_name`/`high_risk` rules, refuses over `REMOTE_MAX_FILES`, and orders directories before their files.
- [ ] **Step 2:** run, expect failure (module absent).
- [ ] **Step 3:** implement the planner and the ledger type (`Created` with `record`, `undo_order` = deepest first).
- [ ] **Step 4:** green; commit `Save to SSH: pure planner and ledger (TWEAKS 7b)`.
- Acceptance: the four tests fail before and pass after; no Win32 or network code in the module.

### Task 2: Remote folder picker over the SSH owner

**Files:** modify `src/ssh_files.rs` (a `list_dir` request and reply parser if the owner has none; reuse its framing), create `src/app/save_remote_ui.rs`.

- [ ] **Step 1: failing tests** `picker_is_bounded` (entries capped at 500, names through `shown_name`, a link entry is shown as a link and is not entered), `picker_path_is_judged` (the chosen path goes through `scp_path`).
- [ ] **Step 2:** run, expect failure.
- [ ] **Step 3:** implement a list-folder request on the existing owner (reply size and line caps as `FILE_REPLY_MAX` and `FILE_REPLY_LINES`), and a small picker window or panel: current path, folders only, up one level, Make folder, Choose. It starts at the session's current directory when known (`scp_path` with the pane's cwd), else the home folder.
- [ ] **Step 4:** green; commit `Save to SSH: remote folder picker (TWEAKS 7b)`.
- Acceptance: tests pass; the picker is reachable only when `can_transfer` is true for the pane (framing on, identity known).

### Task 3: Save Page As entry and the staging phase

**Files:** modify the Save Page As menu route in `src/app/browser_app.rs` (the one entry that builds the local `Job`), `src/page_saver.rs` (a `Destination::Staging` variant that reuses `destination` checks), create `src/save_stage.rs` (pure naming, sweep rule) and wire it in `src/app/save_remote_ui.rs`.

- [ ] **Step 1: failing tests** `staging_removed_every_way` (success, error, cancel: the folder is gone; a folder older than a day is swept at start, a younger one is kept), `save_offers_remote_only_when_allowed`.
- [ ] **Step 2:** run, expect failure.
- [ ] **Step 3:** add "Remote folder..." to Save Page As for an SSH browser pane; on choose, run the existing saver job into the staging folder (same caps, same `write_new_checked`), then hand the finished tree to Task 4. No change to the local route.
- [ ] **Step 4:** green; commit `Save to SSH: stage the saved page locally (TWEAKS 7b)`.
- Acceptance: the local save path is byte for byte unchanged (existing saver tests pass untouched).

### Task 4: Upload with create-new, ledger and cleanup

**Files:** modify `src/app/ssh_files_app.rs` (an upload job that takes the planner's ops), `src/app/save_remote_ui.rs`, `src/app/scripts_alerts.rs` (the confirm question `Then::SaveRemote`, input-guarded, closed window = Cancel; the Mark of the Web difference is in its text).

- [ ] **Step 1: failing tests** `upload_never_replaces` (a fake owner that reports "exists" stops that file and the save says which), `cleanup_removes_only_ledger` (a fake owner holding pre-existing folders: after a cancel only ledger paths are removed, deepest first, folders only if empty), `confirm_names_the_difference` (the question text mentions that the host keeps no web mark and shows the host and folder through `shown_name`).
- [ ] **Step 2:** run, expect failure. The fake owner must refuse what the real one refuses (an existing target, a non-empty folder), as the earlier fakes do.
- [ ] **Step 3:** implement: confirm, then `mkdir` the destination (unique name), per file `put` to `<name>.part` then no-overwrite rename, progress banner, Cancel, ledger, cleanup. One owner only; no shell string.
- [ ] **Step 4:** green; commit `Save to SSH: upload with create-new and cleanup (TWEAKS 7b)`.
- Acceptance: tests pass; a 2000-file tree uploads through the fake owner within the cap; over the cap is refused before any `mkdir`.

### Task 5: Docs and parity

**Files:** `docs/TWEAKS.md` (item 7b to Done), `docs/parity/` (the Save Page As row), `docs/superpowers/rulings/` (a tw7b ruling file with the five decisions above and their cost if wrong).

- [ ] **Step 1:** write the rulings and the parity row; recount the parity totals with the same check the earlier sweeps used.
- [ ] **Step 2:** commit `Docs: Save Page As to SSH (TWEAKS 7b)`.

### Task 6: Full verification and a desktop look

- [ ] **Step 1:** `scripts/xt test --tests --no-fail-fast` (known flake `find_over_a_large_buffer_is_quick` passes alone) and `scripts/xt build --release`, foreground.
- [ ] **Step 2 (Robert's machine, with warning):** one real save of a small page and of a large one to a real host (a second Linux machine or a file server): the folder appears with the unique name, nothing `.part` is left, the page opens from the host, a cancel mid-way leaves no folder; the staging folder is gone afterwards.

## Acceptance (whole plan)

- Save Page As from an SSH browser pane offers a remote folder; the saved page and its resources land in a new folder on the host, up to 2000 files and 256 MiB, with the saver's names.
- No remote file is replaced; a cancel, error or closed pane leaves only what was already there.
- Executable-looking names are neutralized on the host, modes are 0644/0755, and the confirm text states that the host keeps no web mark.
- The local save path and every earlier saver test are unchanged; the staging folder never outlives the save.

## Decisions (made 2026-10-17)

The user approved building this and left the design points to the builder. Each decision follows iTerm2 where it has behavior for SSH-integration transfers, else the safest equivalent of the local saver. Where the built code differs from the tasks above, this section wins.

1. **Which pages offer it.** Save Page As offers "Remote Folder..." only for a browser pane showing an `iterm2-ssh://<host>/<path>` page whose host has an open session with a framing conductor that `can_transfer` to it (the same registry lookup the ssh page itself uses: `ssh_page::parse`, `identity_matches`, `pane_for_identity`). A browser pane keeps no record of the pane it was opened from, so that URL is the only honest link between a page and a session; any other page saves locally as before. The question "Where do you want to save this page?" has three buttons: Remote Folder..., This Computer..., Cancel. A closed window is Cancel.
2. **Remote folder picker.** iTerm2 chooses remote folders with its own SSH file panel (`iTermOpenPanel` with `canChooseDirectories`, `preferredSSHIdentity`), so Termurion reuses its remote file panel (Download Files) in a folder mode instead of a new window: title "Choose a Folder", button "Choose", only folders and links listed, at most 500 rows (a notice says so), a link is shown as a link and a double click does not enter it, the host list is the one host of the page, no "Use System Panel...". Choose picks the one selected folder, else the folder shown; it is disabled while loading or when a link or several rows are selected. It starts at the session's current directory when known, else the saved path, the home folder, then "/". The chosen path goes through `ssh_files::folder_target` (absolute, at most 4096 characters, no control character, no `..`; judged by `scp_path`) and `can_transfer` again before the question. "Make folder" is not built: the save itself makes a new folder in the chosen one, and iTerm2's panel has none either.
3. **Staging.** As planned, with one change of place: the staging folder is `%TEMP%\Termurion-<UUID>` (made create-new), not `%LOCALAPPDATA%\Termurion\SaveStage`, because the launch sweep (`ssh_local::sweep_stale`, P24) already removes that exact name when older than a day, so no second sweep or scheduled job is added. It is removed on success, error, cancel, a closed pane and a staging failure (a thread retries for about ten seconds while the file worker finishes); a crash leaves it to the launch sweep. The staging copy is written marked and strict like any local save, so a drive that keeps no streams still fails the save (the temp folder is local).
4. **Whole-folder upload over the SSH file machinery.** Nothing new on the wire: the page folder is made with the owner's `mkdir`, each file goes through the transfer engine's existing upload (`ssh_transfers.upload`: hidden `.uploading-<UUID>` temp, 1024-byte `append` chunks, a `stat` probe, then `mv`), the way Alt-drop upload does in iTerm2. Files go one at a time, in order (folders first), off the event loop (replies drive it), so a slow host never blocks the UI. The temp name is iTerm2's, not the `.part` of the plan above. The progress banner `save-remote` moves at most four times a second. The caps: `REMOTE_MAX_ENTRIES` = 2003 (the saver's 2000 resources plus `index.html`, the page folder and `resources`: exactly what the saver can produce, so no page it saves is refused for size), 256 MiB in all, and 64 MiB a file (the file owner's cap); a tree over any cap is refused before the first request.
5. **Create-new on the remote.** The framer's `mkdir` fails when the path exists and `create` truncates, so: the new folder's name is found with `stat` (`name`, `name (2)` ... `name (100)`, the same probe as iTerm2's upload names) and made with `mkdir`; every file is created under a fresh UUID temp name inside that new folder, so nothing that was on the host is ever opened for writing. The framer's `mv` is `shutil.move`, which replaces an existing target, and it has no no-clobber form; so the final move is protected by the folder being new, by the owner's `stat` probe, and by the save stopping (and removing what it made) if the file owner reports it landed under a different name than planned. The only unguarded case is another program creating the same name inside the folder in the milliseconds between the probe and the move; that is the same window iTerm2's own upload has, and the save reports and removes its copy in the cases the owner can see.
6. **Cleanup mirrors R13x4-final-3.** A ledger records each remote path only after the host said it made it (the one in flight at a cancel is added too, as the local `MakeDir` whose answer had not arrived). A cancel, error, lost connection or closed pane sends `rm` (never recursive) for exactly those paths, deepest first; the framer's non-recursive `rm` of a folder is `rmdir`, which the host refuses unless the folder is empty, so a folder somebody else put a file in stays. The engine's own failure path removes the hidden temp of the file in flight. A finished save clears the ledger. A cancel while a file is in flight stops after that request, not after the file.
7. **What stands in for Mark of the Web.** A host has no `Zone.Identifier` and iTerm2 marks nothing remotely either. What stands in: every name is the saver's name (already neutralized with `.download`); a name still high risk after that (a stream form) is refused; names with a separator, `..`, a control character or no text are refused; no mode is set by the save (the framer can only change owner bits; files and folders get the host's umask default, which never carries an execute bit on a created file, so the plan's explicit 0644/0755 is not applied). The confirm question says plainly: "The host will not mark the saved files as downloaded from the web." It is shown with the host and folder cleaned by `shown_name`, with OK and Cancel; a closed window is Cancel.
8. **One owner, no shell string.** Every request is a typed `FileSub` (`stat`, `mkdir`, `rm`) or the engine's upload; names travel base64-encoded as the owner already sends them. A name such as `a; rm -rf ~` or `$(x)` is data and uploads under that literal name (tested).
9. **Not built (with the reason).** Resuming a half-saved upload and syncing (non-goals). A live test against a real host: the repo has no live SSH file-transfer test to follow (the only live SSH probe is `ssh_probe_win`, which needs a WSL sshd and key and tests the ssh client, not the framer); the framer's behavior that the decisions above rely on (`mkdir` fails on an existing path, `create` truncates, `mv` replaces, `rm` of a folder is `rmdir`) was read from the bundled `resources/ssh/framer.py`, and the unit tests use a fake host that refuses what that script refuses.
