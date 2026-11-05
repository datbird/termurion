# Unix test gaps after P3

P0 made the crate and all programs compile on Linux and macOS. P1 (the Unix core) added the pseudo terminal, the session host, job tracking, paths, the clipboard, opening URLs and files, tmux `-CC` and window restore, and moved the remaining Win32 code under `src/platform/`. P2 (macOS) added the Cmd shortcut key, the native menu bar, Quit and Dock handling, the ad hoc signed `Termurion.app`, system-wide hotkeys, notifications, the Keychain and Touch ID gate, quarantine and Finder aliases, and the iTerm2 settings import. P3 (Linux) added the in-window menu bar and pop-up menus, desktop notifications, the Secret Service and the account password gate, system-wide hotkeys on X11, downloads that keep their origin, and the AppImage. This file records what still fails and why.

Run of 2026-11-05 on the P3 Task 8 tree (debug build, `scripts/check-unix --test`):

- Linux: **6602 passed, 158 failed**, every one named by cause below.
- macOS (arm64 on funtimes, same tree): **6568 passed, 156 failed**. 153 are the same failures as Linux; 3 fail only on macOS (see "macOS-only differences"). 5 Linux failures pass on the Mac: `extract::tests::one_walk_checks_and_writes`, `find::tests::find_over_a_large_buffer_is_quick`, `ssh_procs::tests::a_huge_remote_list_closes_quickly_and_briefly`, `synthetic::tests::filtering_a_large_buffer_is_quick` and `trigger_eval::tests::each_line_is_checked_once`.
- Load flakes of the full Linux run, which pass alone: `find::tests::find_over_a_large_buffer_is_quick`, `ssh_procs::tests::a_huge_remote_list_closes_quickly_and_briefly`, the `trigger_eval` timing tests and `host::tests::host_launches_and_echoes`. They sit in their causes below (Timing) or pass in a given run.

The older run, kept for the history. Run of 2026-11-03 on the P2 Task 10 tree (debug build, `scripts/check-unix --test`):

- Linux: **6447 passed, 167 failed**. 165 are named by cause below. The other 2, `trigger_eval::tests::inject_does_not_restart_the_budget` and `trigger_eval::tests::lines_gone_before_the_check`, are load flakes of the full run (they pass alone).
- macOS (arm64 VM, same tree): **6481 passed, 160 failed** at 7e81d1d6; the licenses fix after it moves one to passed (6482 and 159). 155 are the same causes as Linux (the counts differ because some path tests happen to pass on a Mac: Windows path and platform rules 107 of 115, Timing 3 of 5), and 4 fail only on macOS (see "macOS-only differences").
- Nothing fails that is not named here. At the start of P1 Linux gave 6270 passed, 220 failed; at the end of P1 Linux 6403 passed, 173 failed and macOS 6405 passed, 171 failed.

Windows at the P3 Task 8 head: `scripts/win-check` 0 errors, 2 warnings (the same two `fetch_update` deprecations); **`scripts/xt test --lib` PASSES: 6935 passed, 0 failed, 149 ignored**.

Windows at the P2 head: the Windows lib, lib tests and binaries type-check from Linux (`scripts/win-check`: 0 errors, 2 warnings, both the `fetch_update` deprecations in `session_log.rs`). **The Windows test run (`scripts/xt test --lib`) PASSES: 6855 passed, 0 failed, 147 ignored** (2026-11-03, after the P2 head). The first run at 7e81d1d6 gave 3 failures, all tests P2 left behind, none a Windows behavior change: `licenses::tests::every_dependency_is_listed` (it asked for `global-hotkey`, a macOS-only crate), `platform::windows::chrome::menu_bar_replace_tests::install_menus_replaces_through_the_helper` (a source scan still naming `hwnd` after P2 named it `view`) and `remote_prefs::tests::the_live_folder_is_refused` (slash paths, which the judge refuses on Windows). The tests were fixed; the code was not changed.

A failure that passes alone is a flake (see Timing).

## Failures by cause on Linux (every failure is in exactly one cause; the counts sum to 158)

| Cause | Count |
|---|---|
| Coprocesses are Unsupported | 2 |
| Dynamic Profiles are Windows only | 8 |
| Windows-only tools missing | 19 |
| Fonts and text metrics | 3 |
| Windows path and platform rules in assertions | 109 |
| Timing | 6 |
| Text encoding and normalization | 11 |
| Total | 158 |

Gone since P0, because P1 fixed them: "Local shells are Unsupported" (29, the pty backend), "Session host is Unsupported" (19, the Unix socket host), the Licenses test on Linux, and two path-rule tests (`profile::tests::a_missing_start_dir_falls_back_to_home_with_a_warning` and `start_dir_is_home_by_default_and_expands_tilde`). Two of the old "Local shells" tests moved into the new Coprocesses cause.

There is no "other" bucket: every failure is named below.

### Coprocesses are Unsupported (2)

The coprocess backend returns Unsupported on Linux and macOS (P2 or later), and these two pane tests start `cmd.exe` as the coprocess.

- `pane::tests::coprocess_types_into_the_shell`
- `pane::tests::mute_hides_output_from_the_screen`

### Dynamic Profiles are Windows only (8)

"Dynamic Profiles are read on Windows only" (the app no longer raises that alert at a Linux start: it logs once and skips the folder).

- `app::dynamic_profiles_app::tests::a_missing_stamp_is_reported_not_dropped`
- `app::dynamic_profiles_app::tests::rename_names_the_judged_folder`
- `app::dynamic_profiles_app::tests::two_pending_edits_in_one_file_both_land`
- `app::dynamic_profiles_app::tests::write_back_is_atomic`
- `app::dynamic_profiles_app::tests::write_back_refuses_a_changed_file`
- `app::dynamic_profiles_app::tests::write_back_refuses_a_link`
- `app::dynamic_profiles_app::tests::write_back_refuses_a_swapped_identity`
- `app::dynamic_profiles_app::tests::write_back_writes_one_entry_and_keeps_the_rest`

### Windows-only tools missing (19)

tar.exe, powershell.exe, cmd.exe (mklink), zip creation, and the Windows log-folder rule.

- `app::scripts_app::tests::export_round_trips_through_tar`
- `app::scripts_app::tests::recovery_follows_iterm2`
- `app::scripts_app::tests::staging_walk_refuses_links`
- `app::settings_export_ui::tests::export_carries_scripts_and_folders`
- `app::settings_export_ui::tests::export_skips_scripts_that_are_not_local`
- `app::settings_export_ui::tests::export_writes_the_archive`
- `app::settings_export_ui::tests::import_never_writes_through_a_link_at_the_destination`
- `app::settings_export_ui::tests::import_refuses_links`
- `app::settings_export_ui::tests::import_round_trips_an_export`
- `extract::tests::a_deflated_zip_extracts`
- `inline_file::tests::saved_downloads_are_marked_as_from_the_internet`
- `script_provision::tests::system_tools_never_come_from_path`
- `semantic::tests::batch_programs_are_known`
- `semantic::tests::url_handler_follows_iterm2`
- `session_log::tests::an_automatic_log_name_always_ends_in_dot_log`
- `session_log::tests::auto_log_path_follows_iterm2`
- `session_log::tests::program_set_values_are_one_path_segment`
- `util_tip::tests::it2tip_follows_the_script`
- `workgroup_run::tests::cmd_parsed_lines_refuse_cmd_syntax`

### Fonts and text metrics (3)

Text widths and face lookup measured against Windows fonts (Cascadia Mono is not installed). Three tests of this group that failed after P2 (`faces_follow_profiles`, `text_advance_matches_the_cell_width`, `underlines_break_around_glyph_ink`) pass in the P3 run (not traced to one change). `every_effect_lights_both_box_lines_on_enter` asks for Consolas, which is not installed, and the embers effect leaves the top box line dark with the stand-in face ("Consolas 20 input 6 embers: top 0"); it fails the same way on macOS and every run, and was not diagnosed further.

- `render::tests::every_effect_lights_both_box_lines_on_enter`
- `render::tests::face_lookup_does_not_scan_fonts_per_call`
- `render::tests::postscript_names_resolve_to_installed_families`

### Windows path and platform rules in assertions (109)

Expected values are `C:\` paths, `\` separators, drive letters, UNC and subst rules, 8.3 names, PATHEXT, explorer.exe, USERPROFILE, the registry, or a path judge that refuses `/tmp` paths. Named by module below.

- `ai_actions::tests::file_target_follows_windows`
- `ai_actions::tests::get_help_runs_system_powershell`
- `app::dynamic_profiles_app::tests::write_back_stays_in_the_folder`
- `app::git_ui::tests::single_use_windows_follow_iterm2`
- `app::scripts_app::tests::autolaunch_follows_iterm2`
- `app::scripts_app::tests::autolaunch_skips_network_links`
- `app::scripts_app::tests::menu_entries_follow_iterm2`
- `app::scripts_app::tests::script_items_follow_iterm2`
- `app::settings_export_ui::tests::finish_import_applies_once`
- `app::settings_export_ui::tests::finish_import_keeps_the_ai_consent_switches`
- `app::settings_export_ui::tests::finish_import_marks_an_imported_state`
- `app::settings_export_ui::tests::import_never_reaches_a_protected_entry_through_an_alias`
- `app::url_handler_ui::tests::handler_spawns_without_a_shell`
- `app::url_handler_ui::tests::the_working_folder_is_a_local_drive_folder`
- `bell_badge::tests::sound_names_are_judged`
- `browser_extensions::tests::extension_folders_follow_iterm2`
- `browser_pages::file_page_tests::directory_listing_follows_iterm2`
- `chat_input::tests::a_dropped_item_that_links_to_a_network_path_is_never_touched`
- `chat_input::tests::a_folder_names_everything_it_leaves_out`
- `chat_input::tests::inserted_text_and_pending_lists_stay_tidy`
- `chat_input::tests::links_are_followed_only_inside_the_folder`
- `chat_input::tests::message_building_follows_iterm2`
- `chat_view::tests::attachment_paths_stay_under_the_chat`
- `claude_code::tests::config_dir_follows_windows`
- `code_review::tests::a_review_never_runs_git`
- `composer_worker::tests::checks_follow_the_rules`
- `conductor::tests::level_one_follows_iterm2`
- `conductor::tests::payload_order_follows_iterm2`
- `default_term::tests::a_moved_exe_gets_its_handoff_server_repaired`
- `dynamic_profiles::tests::dynamic_profiles_go_through_the_one_path_judge`
- `extract::tests::one_walk_checks_and_writes`
- `git_state::tests::a_git_program_runs_only_through_the_launch`
- `git_state::tests::fetch_state_with_asks_only_for_what_is_wanted`
- `git_state::tests::log_is_hardened_and_truncated_like_iterm2`
- `kitty_dnd::tests::kitty_dnd_is_bounded`
- `launch::tests::hostile_command_lines_are_refused`
- `launch::tests::requests_follow_the_command_line`
- `localfs::tests::a_person_picked_path_may_be_on_a_share`
- `localfs::tests::a_refused_path_is_never_judged`
- `netpath::tests::a_chain_of_relative_links_is_walked_where_windows_resolves_it`
- `netpath::tests::a_link_targets_own_folders_are_walked_root_first`
- `netpath::tests::a_parent_step_is_collapsed_before_any_link_is_read`
- `netpath::tests::ancestor_links_are_followed_root_first_and_checked`
- `netpath::tests::link_reads_count_toward_the_budget`
- `netpath::tests::reparse_data_is_read_and_unknown_tags_are_refused`
- `netpath::tests::subst_letters_are_resolved_and_judged`
- `page_saver::tests::save_destination_follows_windows`
- `remote_prefs::tests::locations_follow_iterm2`
- `script_archive::tests::archive_entries_are_checked`
- `script_archive::tests::archive_layout_follows_iterm2`
- `script_archive::tests::export_follows_iterm2`
- `script_deps::tests::confirmation_names_the_configured_index`
- `script_deps::tests::dependency_editor_follows_iterm2`
- `script_deps::tests::editor_state_follows_iterm2`
- `script_export_ui::tests::export_rows_follow_the_chooser`
- `script_launch::tests::classify_follows_iterm2`
- `script_launch::tests::launch_env_follows_iterm2`
- `script_new::tests::new_script_paths_follow_iterm2`
- `script_provision::tests::failed_rebuild_keeps_the_old_venv`
- `script_provision::tests::install_uv_follows_iterm2`
- `script_provision::tests::interrupted_swap_is_recovered`
- `script_provision::tests::provision_follows_iterm2`
- `script_provision::tests::update_check_follows_iterm2`
- `script_runtime::tests::layout_follows_windows`
- `scripts::tests::dos_device_names_are_never_local_files`
- `scripts::tests::links_are_followed_without_loops`
- `scripts::tests::roots_never_follow_network_links`
- `scripts::tests::script_tree_follows_iterm2`
- `scripts::tests::scripts_folder_follows_iterm2`
- `scripts::tests::the_one_rule_refuses_mapped_drives_and_ancestor_links`
- `semantic::tests::a_relative_program_with_a_folder_is_not_resolved`
- `semantic::tests::a_stream_or_alias_form_never_hides_a_program`
- `semantic::tests::explorer_runs_from_the_windows_folder`
- `semantic::tests::finds_a_path_with_line_and_column`
- `semantic::tests::handler_forwarders_are_refused`
- `semantic::tests::handler_variables_are_one_argument_each`
- `semantic::tests::local_opens_obey_the_probe_rule_and_never_probe_when_planned`
- `semantic::tests::resolve_on_path_prefers_a_runnable_extension`
- `semantic::tests::resolve_program_walks_path_and_pathext`
- `settings_export::tests::erase_plan_follows_iterm2`
- `settings_export::tests::pending_import_follows_iterm2`
- `settings_ssh::tests::browse_looks_only_at_an_absolute_local_folder`
- `settings_ssh::tests::browse_never_probes_a_network_path`
- `ssh_inject::tests::tarball_caps_hold`
- `ssh_inject::tests::tarball_follows_iterm2`
- `ssh_local::tests::final_names_follow_iterm2`
- `start_dir::tests::expanded_directory_refuses_unc`
- `start_dir::tests::resolve_follows_iterm2`
- `start_dir::tests::start_dirs_are_judged_before_is_dir`
- `start_dir::tests::windows_paths_interpolate_through_the_real_parser`
- `tab_icons::tests::icon_lookup_follows_iterm2`
- `transfers::tests::get_info_follows_iterm2`
- `trigger_eval::tests::each_line_is_checked_once`
- `url_schemes::registration_tests::a_moved_exe_is_repaired_only_for_what_the_ok_registered`
- `url_schemes::registration_tests::registration_follows_windows_entries`
- `url_schemes::registration_tests::unregistering_removes_only_what_termurion_wrote`
- `url_schemes::tests::an_exe_path_with_a_percent_is_refused`
- `util_api::tests::python_follows_windows`
- `util_api::tests::script_path_follows_the_program`
- `util_cli::tests::network_paths_are_never_opened`
- `util_cli::tests::path_folders_are_local`
- `util_files::tests::it2dl_follows_the_script`
- `util_git::tests::it2git_follows_the_script`
- `util_query::tests::it2ul_follows_the_script`
- `util_simple::tests::it2copy_follows_the_script`
- `web_replay::tests::the_capture_skips_a_page_with_a_password_field`
- `webhost::tests::dev_null_folders_are_per_run`
- `webhost::tests::folders_and_options_follow_windows`
- `workgroup_run::tests::a_member_never_runs_git_unchecked`

### Timing (6)

Time-bounded tests on a loaded debug build, and thread-count tests. `find_over_a_large_buffer_is_quick` and `a_huge_remote_list_closes_quickly_and_briefly` are the known flakes.

- `app::git_ui::tests::a_stuck_path_never_takes_a_second_thread`
- `app::git_ui::tests::worker_bounds_threads_and_abandons_stuck_ones`
- `find::tests::find_over_a_large_buffer_is_quick`
- `scripts::tests::slow_scan_follows_iterm2`
- `ssh_procs::tests::a_huge_remote_list_closes_quickly_and_briefly`
- `synthetic::tests::filtering_a_large_buffer_is_quick`

### Text encoding and normalization (11)

Session code page and Unicode normalization assertions. Not diagnosed further in P0 (the expected values may rely on Windows conversion calls).

- `api_buffer::tests::line_contents_follow_the_normalization_form`
- `claude_code::tests::the_same_folder_is_found_by_normalized_path`
- `find::tests::find_follows_the_normalization_form`
- `pane::tests::initial_text_leaves_in_the_session_encoding`
- `pane::tests::input_uses_the_session_encoding_but_tmux_is_utf8`
- `pane::tests::output_is_decoded_with_the_session_encoding`
- `pane::tests::replies_leave_raw_and_text_is_encoded_once`
- `scene::tests::normalization_form_reaches_the_drawn_text`
- `scene::tests::offscreen_band_normalizes`
- `screen::tests::text_leaves_the_grid_normalized`
- `styled_copy::tests::copies_follow_the_normalization_form`

## Known security gaps in the open judge (P2 Task 8)

Recorded after the Task 8 re-review; none is fixed, each is a residual window after the judge and before `open`.

- Only the file's own folder is tested by the owner rule, so a writable ancestor (a 0777 folder without the sticky bit holding your own folder, or a folder another non-root user owns) can still swap a whole folder.
- macOS ACLs are not read: an ACL that lets another user add files or change security on a folder or file is invisible to the mode test.
- Alias targets are screened with the default probe rule, so the person's PathsToIgnore list does not apply to them (it is stricter on network paths, not on ignored slow folders).

## macOS-only differences

macOS has the same failures as Linux except these 3, which fail only on macOS (`every_effect_lights_both_box_lines_on_enter` fails on Linux too now and is named under Fonts). The licenses test passes there now: it checks only the crates the running target builds (2026-11-03). The pwsh install profile test passes there now (it runs `pwsh` where that is the shell and returns when the program is missing).

- `pane::tests::the_pwsh_script_emits_prompt_marks`: no `pwsh` on the macOS VM
- `pane::tests::the_pwsh_script_keeps_the_prompt_state`: no `pwsh` on the macOS VM
- `render::tests::lightning_enter_surge_finds_the_claude_box_lines`: a render test that already failed on the macOS VM at the start of P1; not diagnosed

## Live tmux lines

The tmux `-CC` code is covered by four `unix-gui-check` scenarios on Linux with a live tmux: `tmux` (tabs in the attaching window), `tmux-nolang` (the app starts with no locale variable), `tmux-lcall` (the app starts with `LC_ALL=C`) and `tmux-windows` (the default, a new Termurion window). All four PASS (below). A fifth, `tmux-restore`, relaunches the app on the same config after a gateway was open, in three variants (see "P2 exit checks"). The macOS VM has tmux 3.8 (Homebrew, `/opt/homebrew/bin`, not on the PATH of a plain ssh command): the four live local tmux tests passed there in Task 7 with that tmux on the PATH. The GUI scenarios need Xvfb, so they run on Linux only.

## Tests hidden from Unix on purpose

A `#[cfg(windows)]` on a test or part of one is listed here and in `scripts/cfg-audit.allow` with its reason. In `src/app/`: the DACL check in `adblock_app` (SDDL), the exclusive share mode in `archive_ui`, the links-and-devices test in `emu_ui` (junctions, device names, file symlinks; the symlink test is gated off Unix), the real-HWND tests in `menu_hold_tests`, the Mark of the Web test in `page_saver_app` (NTFS stream), the ICU pattern test in `statusbar_content`. Outside `src/app/`: in `url_launch.rs` the lone-surrogate test and the Windows pipe tests, in `host.rs` the two live ConPTY tests, and the Windows-only tests that sit beside the Windows code they test in `dlog`, `localfs`, `script_provision`, `url_schemes`, `api_auth`, `api_peer`, `desktops`, `workgroup_restore` and others (counted by the whole-file entries; for the modules P1 moved, those entries now count test cfgs only).

## The cfg(unix) unit tests from Tasks 1 to 10

The tests under `platform::unix::*` and the stand-ins ran on both hosts. On Linux none of them is in the failing list. Under load `platform::unix::proc::tests::ctrl_c_and_break_are_int_and_quit` and `the_foreground_job_follows_the_pty` can fail in a full run; they pass alone. On macOS none of them fails either: `a_shell_inherits_no_other_pty` failed there until the final review fixes, because another thread's pipe could reach a new shell in the moment before std set its close-on-exec flag (macOS has no `pipe2`); a new shell now gets every descriptor from 3 up flagged close-on-exec.

## What is left in the cfg-audit allowlist

The whole-file entries whose reason reads "production cfg owed to the port" are Windows code that still sits behind a `#[cfg(windows)]` outside `src/platform/` (for example `url_schemes`, `encoding`, `netpath`, `input_locale`, `git_state`, `dll`, `browser_autofill`). P1 covered the modules moved in Tasks 9 and 10; the rest is owed to the port. `scripts/cfg-audit` exits 0 and stays the gate.

## P1 exit checks

`scripts/unix-gui-check` on Linux (Xvfb), run on the Task 10 tree, 2026-11-02. After the final review fixes (2026-11-03) `host`, `clipboard`, `restore-host` and `tmux-nolang` ran again and PASS; `clipboard` now keeps the pane's highlight when another program takes the primary selection (it fails with the old middle-click order and passes now):

| Scenario | Result |
|---|---|
| shell | PASS |
| layout | PASS |
| host | PASS |
| job | PASS |
| clipboard | PASS |
| tmux | PASS |
| tmux-nolang | PASS |
| tmux-lcall | PASS |
| tmux-windows | PASS |
| restore-windows | PASS |
| restore-host | PASS |

macOS hand checks on the macOS VM (Task 8 and its fix rounds), 2026-11-02:

- Host case (RunJobsInServers true): PASS. After `kill -9` of the app the shell and host stayed alive, the host started Termurion again, and the pane showed the same shell pid and the earlier marker line.
- Stock Quit then relaunch: restores. Quit through the stock Termurion > Quit item saved the windows (`App::exiting`), and the relaunch showed the earlier marker line with the "Session Contents Restored" banner. One pane only, because macOS has no native menu bar until P2.
- Tabs and splits on macOS: covered by the Linux `layout`, `restore-windows` and `restore-host` scenarios and the unit tests (`layout`, `arrangement`, `restore_state`, `shortcut_key` and `keyproto` ran green on macOS). They could not be built by hand on the VM, which has no native menu bar.
- Windows: `scripts/xt test --lib` is PENDING (host on hold; it runs at the end of P1). `scripts/win-check` is the compile proof only.

Parked for P2 (macOS packaging, the native menu bar and first run):

- Stock macOS Quit skips frame saving, notes and log flushing. The window state is saved on that path now; the rest waits for the native Quit going through `App::quit`.
- A windowless relaunch after the macOS Downloads prompt (the unsigned dev bundle asks for Downloads access on each launch; one relaunch left the process running with no window).
- The tmux control-mode dialog on stale saved state ("%begin with empty command queue" at launch after old tmux tests; Termurion's own error dialog, not a crash; cause not proven). Looked at in P2 Task 10: not reproduced (see "P2 exit checks").

## P2 exit checks

Run on 2026-11-03 on the P2 Task 10 tree. The VM checks ran on the macOS VM with the dev bundle (`scripts/mac-dev-app`) or the ad hoc signed bundle (`scripts/mac-bundle`).

### Linux `scripts/unix-gui-check`, every scenario (Xvfb)

| Scenario | Result |
|---|---|
| shell | PASS |
| layout | PASS |
| host | PASS |
| job | PASS |
| clipboard | PASS |
| tmux | PASS |
| tmux-nolang | PASS |
| tmux-lcall | PASS |
| tmux-windows | PASS |
| restore-windows | PASS |
| restore-host | PASS |
| tmux-restore | PASS |

`tmux-restore` runs three variants, each ending with a relaunch on the same config. PASS for a variant means no "tmux Reported a Problem" box after the relaunch (the app log and the stand-in dialog log are searched) and the pane answers a typed `echo $$`:

| Variant | What it does | Result |
|---|---|---|
| a | shells in the app, a tmux `-CC` gateway, quit with the Quit chord, relaunch | PASS (the pane is a plain shell with its restored text) |
| b | shells in the session host, `kill -9` of the app, the host reopens it | PASS (the tmux client is still in control mode) |
| c | b with the tmux server killed before the relaunch | PASS (a plain shell) |
| d | b with a tmux window made 50 ms before the kill, so a reply to the app's own command may be in flight (not in the default run; run three times) | PASS, 3 of 3 |

The macOS VM ran the same three by hand with the dev bundle and the system Quit (a: the Quit question answered OK; the second launch ran a plain shell): no box, the restored window showed the tmux tab and a plain tab, and in b the tmux client was still in control mode after the host reopened the app. Result: the "%begin with empty command queue" dialog is **not reproduced** on Linux or on macOS, so no code changed for it. The check stays as a regression scenario. If the dialog comes back, the layer to look at is `tmux_gateway.rs`: a `%begin` for a command the gateway never sent aborts there as iTerm2 does, and that rule is kept.

### Per-task VM checks (macOS VM)

| Task | VM check | Date |
|---|---|---|
| 1 Cmd key | Cmd-T and Cmd-D gave a tab and a split (3 distinct shells); the window title reads the Mac glyphs | 2026-11-03 |
| 2 Menu bar | menu items, context menu Copy, Settings > Keys recording of Cmd-T and a mapped Send Text, Cmd-V and Cmd-A in Settings, Quit and relaunch restoring tabs | 2026-11-03 |
| 3 Quit and Dock | `osascript` quit: the question, Cancel answered, OK quits; Cmd-H hides | 2026-11-03 |
| 4 Bundle | ten launch rounds: open, window listed, quit through `osascript`, process gone: 10 of 10 PASS; no Downloads prompt at launch | 2026-11-03 |
| 5 Hotkeys | the registration, cloaking and recorder checked; a real press could not be delivered on the VM (it never fires a Carbon hot key); moved to the owner's check | 2026-11-03 |
| 6 Notifications | the permission prompt, the banner, a click bringing Termurion to the front | 2026-11-03 |
| 7 Keychain | the live Keychain test passed; Password Manager opens in front, add, close, reopen, delete | 2026-11-03 |
| 8 Quarantine and aliases | a download carries the quarantine attribute with the Termurion agent name | 2026-11-03 |
| 9 iTerm2 import | the picker, an imported profile in the Profiles list (after the scroll fix) | 2026-11-03 |
| 10 tmux restore | variants a, b and c by hand (above) | 2026-11-03 |
| Final fix round | Settings > Keys recorder took Cmd-T as the chord and no tab opened; the Profiles menu draws a profile's Ctrl+Cmd key as a native key equivalent; with "Require Authentication after Screen Locks" on, a posted `com.apple.screenIsLocked` closed the open password manager and the next open asked for the password again (while still authenticated, a reopen did not ask). A real screen lock was not driven on the VM: the notification was posted from another process, the same delivery path. | 2026-11-03 |

The Windows words in macOS prose (Task 10, B) were checked in the unit tests and by the schema test that fails on any Explorer, taskbar, Win key, Credential Manager, Ctrl key or Alt key word left in a visible row on macOS.

### The owner's exit check

Run on 2026-11-04 on the owner's own Apple silicon Mac. The last build he ran is 283a4f69. Every step was also run on the macOS VM first (VM checklists 6 and 7), with screenshots read. The VM covers what the owner did not report on.

1. Install from the zip. Owner: PASS, installed over the first beta, with his old settings kept.
2. First open past Gatekeeper. Owner: PASS.
3. Tabs and splits from the menu and from Cmd-T and Cmd-D. VM: PASS.
4. `tmux -CC`, then Shell > tmux > Detach. VM: PASS (one prompt, the window offset from its parent, "Detached").
5. Import from iTerm2. VM: PASS (the picker opens at the iTerm2 settings file, and the profile is imported).
6. The system-wide hotkey. VM: FAIL on 10c6196a (a show after a hide needed two presses), fixed in 6c20c3e1, then PASS (four presses from Finder alternate show and hide). The owner did not report on it.
7. The notification banner. VM: PASS (Notification Center shows "hello").
8. The password manager. Owner: PASS with Touch ID. VM: PASS with the password (add, reopen, delete).
9. A restore after Cmd-Q and after Dock Quit. VM: PASS (the tabs, the splits and the text come back, with no tmux text and no lost blank row).
10. What failed, as the owner found it:
    - A red "Font not installed: Cascadia Mono" banner at launch. The first beta had saved the Windows default font in his profiles.toml. Fixed in d9e283f6 and f1789950: a saved Windows default font becomes Monaco at the same size, and the font Termurion ships as default never raises a banner. Owner: PASS.
    - No menu bar icon when all windows are closed, unlike the Windows tray icon. Added in 3ddee4ff. Owner: PASS.
    - The app stayed in the Dock with no windows open; he wanted it to work like 1Password. Fixed in 998ed6f5 and 283a4f69. Owner: PASS.

The owner's rule for open behavior questions: do what iTerm2 does by default, on every OS. Under that rule, the tmux restore and New Tab in a tmux window follow iTerm2 since 931df532 to effe4191. tmux tabs are not restored. A restored gateway is a real gateway. New Tab in a tmux window asks first. Quit detaches tmux. The VM ran a normal quit, a quit with "kill jobs on quit" off, and a `kill -9` crash.

### Windows test run

`scripts/xt test --lib` at effe4191: 6927 passed, 1 failed (`find_over_a_large_buffer_is_quick`, a timing test that passes when run alone). `scripts/win-check`: 0 errors.

### The macOS test sweep (Task 10)

Before the comparison above, 15 lib tests that failed on macOS and were not in this file were looked at one by one. Each is a real bug (fixed, test first) or a test that hard-coded Windows text or paths (it now asserts the right value per OS and keeps the Windows literal on Windows):

| Test | What it was | Broke at |
|---|---|---|
| `pointer::tests::keys_read_like_iterm2_rows` | real bug: a Cmd+Shift pointer row read "Win+Shift" on macOS; it now goes through the shortcut key's modifier spelling | `0e603aff` (the Cmd key) |
| `localfs::tests::every_path_probe_goes_through_the_judge` | real bug: the Import from iTerm2 picker tested the live settings file with a raw `exists()`; it is judged first now | `680ac1d7` (the iTerm2 import) |
| `ai_keys::tests::the_system_store_is_unsupported_off_windows` | macOS has the Keychain store now; the test skips macOS | `1d9f2ea0` (Keychain) |
| `pane_title::tests::title_text_follows_iterm2`, `scene::tests::tab_bar_draws_the_layout`, `status_tool::tests::shortcuts_follow_iterm2`, `tab_layout::tests::bar_metrics_follow_iterm2`, `tab_layout::tests::build_follows_iterm2`, `tab_layout::tests::numbers_follow_iterm2`, `wintitle::tests::compact_caption_follows_r11`, `wintitle::tests::number_label_follows_iterm2`, `wintitle::tests::window_menu_entry_follows_iterm2`, `workgroup_peers::tests::a_custom_shortcut_matches_its_key`, `workgroup_peers::tests::peer_shortcuts_follow_iterm2`, `profile_menu::tests::profile_shortcut_follows_iterm2` | tests that read Windows shortcut text (Ctrl+Shift+1); the shortcut mode starts as Cmd on macOS, so the labels are Mac glyphs. They use `platform::by_cmd` for the expected text | `0e603aff` and `56794cca` (the initial mode) |
| `script_deps::tests::pip_commands_follow_iterm2`, `script_runtime::tests::commands_follow_iterm2` | tests that built `C:\u\python` paths by hand; the expected paths are joined with the OS separator now | never passed on Unix (listed at the Task 3 review) |
| `app::menu::install_tests::the_pwsh_install_command_adds_one_line_to_the_profile` | the test ran `powershell.exe`; it runs `pwsh` off Windows and returns when pwsh is missing | never passed on macOS |
| `app::prefs_ui::tests::a_folder_linked_to_a_network_path_is_never_read_or_written`, `remote_prefs::tests::a_folder_linked_to_a_network_path_is_never_probed` | tests used a UNC link target and drive paths, which a Unix link reads as a plain name; they use `//h/s` and slash paths off Windows | never passed on Unix |

Left in the failing lists with their causes: the rest of `script_deps` (`confirmation_names_the_configured_index`, `dependency_editor_follows_iterm2`, `editor_state_follows_iterm2`), because `venv_python` and the script layout name a Windows virtual environment (`Scripts\python.exe`) and the Python API scripts are a later phase; and every other listed test, for the causes above.

### Windows words in macOS prose (Task 10)

One table (`platform::native_words`, applied on macOS only; Windows text is unchanged byte for byte, and Linux changes only the credential store's name, see P3 Task 4) names the Mac things: Explorer is Finder, Credential Manager is the Keychain, taskbar is Dock, Alt+Tab is the Cmd-Tab switcher, Left and Right Win are the Command keys, Alt and Ctrl keys are Option and Control. It runs on every Settings schema label, help and choice (`schema::profile`, `schema::global`), on the alert windows and message boxes, and on the tip card. The Keys page draws no empty Shortcut Key group and no Windows shortcut switch on macOS. Left as they are, each a different text needed per OS and not a word swap: the Dock and menu bar icon rows ("Add notification area icon when excluded from Dock", the notification area click choices; macOS has no menu bar icon yet), the desktop messages naming Task View (Win+Tab), the Windows 10 and 11 window border and theme help, the Windows sound names in the bell sound help, the "Windows cannot see the fn key" help, and the Windows-only menu tips (shown only by the Win32 menu tooltip).

### P3 Task 4: the Secret Service on Linux

The password manager and the AI keys use the Secret Service (`platform::linux::secrets`, the `secret-service` crate on the one `zbus` 5 of the tree). Linux failures: no test failed on "Password manager is not available" at the P2 count, so the 165 do not move; the stand-in's own test (`unix_credentials_unsupported`) and the AI keys test that asserted Unsupported (`ai_keys::tests::the_system_store_is_unsupported_off_windows`, which would now reach a real keyring) are gone, replaced by `platform::linux::secrets::tests`, each keyring test in a child process under its own `dbus-run-session`, `gnome-keyring-daemon` and scratch HOME. On Linux `native_words` maps "Credential Manager" to "the Secret Service". The unlock prompt is tested with a stand-in for the desktop prompter (`org.gnome.keyring.SystemPrompter`) on the private bus: a call made while the prompt is open answers "locked" at once, a dismissal lets the next call ask again, and a hung service trips a latch so later calls fail at once until a background probe gets an answer.

The password manager gate and the screen lock on Linux (P3 Task 5): the gate is the account password, asked in Termurion's own window and checked by PAM (`platform::linux::user_auth`, service `login`, authenticate only; libpam.so.0 is opened at run time, so there is no build dependency). Three wrong passwords fail it. The check runs on a worker thread and answers through `UserEvent::PwmGate`. A browser basic-auth save that needs the gate opens the same window at save time and saves after a grant (cancel or failure reports it unsaved); a basic-auth picker keeps its challenge alive while the gate waits. The screen lock is logind's `Session.Lock` for this login session plus `ActiveChanged(true)` of `org.freedesktop.ScreenSaver` and `org.gnome.ScreenSaver` (`platform::linux::screen_lock`); tests run a fake logind on a private bus. No Linux failure count moves: the full lib run showed three extra load flakes (render, two in trigger_eval) that pass alone.

## P3 exit checks

Run on 2026-11-05 on the P3 Task 8 tree. The AppImage is `target/linux/Termurion-0.0.1-beta10-x86_64.AppImage`, built by `scripts/linux-appimage` (the release build, an AppDir with `termurion`, `termurion-host`, `termurion-git` and `termurion-utils` side by side, the `.desktop` file and the icon, packed by appimagetool 1.9.1 with its SHA-256 checked first). The session host is found by the lookup the app already does (the file beside the running binary), so `AppRun` only starts `usr/bin/termurion`.

### The AppImage

| Check | Result |
|---|---|
| `unix-gui-check shell` with `TERMURION_BIN` set to the AppImage (`--appimage-extract-and-run`) on Xvfb | PASS |
| `xprop WM_CLASS` on its window | `"termurion", "dev.termurion.Termurion"` (the class is the app id and equals the `.desktop` `StartupWMClass`) |
| First launch with an empty config under Xvfb | a window with the menu bar and the login shell; no banner (screenshot `.superpowers/sdd/2026-11-03-p3-linux/task-8-shots/first-launch.png`). Cascadia Mono is not installed, and the font Termurion ships as default never raises a banner, so the pane draws with DejaVu Sans Mono and nothing is shown. |
| Wayland smoke: headless `weston --socket=wl-test`, no `DISPLAY` | the window opened on Wayland (`xdg_toplevel.set_app_id("dev.termurion.Termurion")`, title "Termurion") and the shell started (`pty: start 80x25` in host.log). The Settings hotkey row's Wayland reason is pinned by `platform::unix::keys::tests::hotkeys_on_wayland_name_wayland`; the row itself was not looked at under weston, since headless weston has no screenshot path here. |

### Linux `scripts/unix-gui-check`, every scenario (Xvfb, the debug build)

| Scenario | Result |
|---|---|
| shell, layout, menubar, popup, host, job, notify, hotkey, hotkey-refused | PASS |
| tmux, tmux-nolang, tmux-lcall, tmux-windows | PASS |
| restore-windows, restore-host | PASS |
| clipboard | PASS after a fix to the script: it dragged at screen point (2, 10), which was the window's corner before the window was centered and the menu bar took the top strip. It now drags inside the window under the bar. No app change. |
| tmux-restore a, b | PASS |
| tmux-restore c | FAIL. After the relaunch the pane shows the replayed tmux text and "Detached", draws no prompt, and a typed line never reaches the shell. It fails the same way at 762760e2 (the P2 head) and at the P3 base, so it is not from P3. The likely start is the P2 change that makes a restored gateway a real gateway (db168bd7 to effe4191), which came after the P2 table above (that table passed variant c). Not fixed here; it needs its own look at what a restored gateway does when its tmux server is gone. |

### Tests

- Linux `scripts/check-unix --linux-only --test`: 6602 passed, 158 failed, all named above.
- macOS on funtimes: 6568 passed, 156 failed (see the top of this file).
- `scripts/win-check`: 0 errors, 2 warnings. `scripts/xt test --lib`: 6935 passed, 0 failed.
- `scripts/cfg-audit`: exit 0.

### The owner's exit check

Pending. The steps are in the P3 plan, Task 8 Step 8; each answer is recorded here when he has run them.

## Deferred after P3

Each with its reason.

- The tray icon (StatusNotifierItem): a new D-Bus service with its own menu; not in the spec's P3 list.
- System-wide hotkeys on Wayland through the GlobalShortcuts portal: the spec says "Wayland portal later".
- Double-tap hotkeys on Linux: they need a global key monitor, which X11 gives only through XInput2 raw events and Wayland not at all.
- Packages beyond the AppImage (deb, rpm, Flatpak): the spec says "AppImage first".
- aarch64 Linux: x86_64 first.
- A systemd user unit for the session host (the spec's call): P1's detached host already does the job, as on macOS (P2 Decision 1).
- Printing through `lp`: the spec lists it under "Later: printing panels".
- The CI matrix (Decision 13).
- The Unix stand-ins P1 named for later phases (coprocesses, saving debug logs, Python API scripts, password manager command-line tools, Dynamic Profiles, the automation API peer check).
- `tmux-restore c` (above): a restored gateway whose tmux server is gone leaves a pane that takes no input.

## Deferred after P2

Each with its reason.

- Secure Keyboard Entry on macOS: a security mode with its own lifecycle (`EnableSecureEventInput`, focus, a stuck-on state); its own step after P2, not part of the spec's P2 list.
- Double-tap hotkeys on macOS: a global modifier monitor needs the Input Monitoring permission and its prompt.
- Notarization and a Developer ID signature: the spec puts them later. Until then each rebuild may ask once more for Keychain access and TCC grants.
- A launchd agent for the session host (Decision 1); `$TMPDIR` aging stays covered by P1's hourly refresh.
- Opening URLs and files handed over by Finder or other apps (`application:openURLs:`), and the Default Term handler for `ssh:` and `.command`: not in the spec's P2 list.
- A second copy started from the bare binary (no launch pipe on Unix, Decision 18).
- The Windows Transfers menu's Open without the open judge (parked from P1): Windows behavior does not change in the port; it belongs to a Windows fix.
- Quarantine on Linux: done in P3 Task 7. Downloads there carry the freedesktop origin attributes (`user.xdg.origin.url`, `user.xdg.referrer.url`) and no longer fail with "Failed to quarantine". Six tests that failed only on that mark now pass (`app::page_saver_app::tests::a_huge_url_list_is_capped`, `a_large_resource_is_saved_up_to_the_stated_cap`, `save_flow_follows_iterm2`, `the_alert_shows_sanitized_text_and_the_door_refuses_types`, `write_failures_follow_iterm2`, and `inert_files::tests::write_new_checked_never_overwrites_and_marks`).
- The Unix stand-ins P1 named for later phases (coprocesses, saving debug logs, Python API scripts, password manager command-line tools, Dynamic Profiles, the automation API peer check): not in the spec's P2 list.
- Reading iTerm2's own Keychain items (its passwords): another app's items ask for access one by one; the import copies settings, not secrets.
- The CI matrix (Decision 17).
- The open judge's three known gaps (above): a writable ancestor, ACLs, and the ignore list for alias targets.
- Known macOS limit: a profile with shortcut key Q never opens from its key. Its chord is Ctrl+Cmd+Q, which macOS takes system-wide for Lock Screen before any app sees it. iTerm2 has the same clash (its profile shortcuts are also Ctrl+Cmd+letter). The Profiles menu item still opens it.
- Chord collisions that predate P2 and match Windows (Decrease and Increase Height against Select Pane Above and Below; Cmd+Shift+= Show Clippings against Make Text Bigger): FIXES.md item 20.
- The Python API scripts' virtual environment layout is Windows-only (`Scripts\python.exe`); macOS and Linux use `bin/python` when that phase lands.
