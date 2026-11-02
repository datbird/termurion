# Unix test gaps after P0

P0 makes the crate and all programs compile on Linux and macOS. It does not make the tests pass there.
Nothing below is fixed in P0; P1 and later work through it by cause.

Linux (sirtoolio, debug build, the last code commit of P0): `cargo test --lib` gave 6270 passed, 220 failed, 11 ignored
(683 s; `workgroup::tests::the_model_never_holds_what_the_write_point_would_drop` alone needs minutes on a debug build).
The failing set is the same 220 tests as before the final review fixes. The log is a local file, not in the repo.
Windows: `scripts/xt test --lib` gave 6802 passed, 0 failed, 143 ignored before the final review fixes; after them the
only failures were two tests the fixes tripped (corrected and green on rerun) and the known timing flake
`find::tests::find_over_a_large_buffer_is_quick`.

## Failures by cause (every failure is in exactly one cause; the counts sum to 220)

| Cause | Count |
|---|---|
| Local shells are Unsupported | 29 |
| Session host is Unsupported | 19 |
| Dynamic Profiles are Windows only | 8 |
| Windows-only tools missing | 20 |
| Fonts and text metrics | 5 |
| Windows path and platform rules in assertions | 121 |
| Timing | 6 |
| Text encoding and normalization | 11 |
| Licenses | 1 |
| Total | 220 |

There is no "other" bucket: a failure not matched by a message rule was put under cause 6 by its module, and the lists below name every test.

### Local shells are Unsupported (29)

Tests that start a pane, shell or pty fail at the spawn with "Local shells is not available on Linux yet". Fixed by the P1 pty backend.

- `host::tests::the_prompt_watch_skips_paused_and_unattached_sessions`
- `pane::tests::a_live_shell_ends_before_its_restart`
- `pane::tests::a_paused_pane_stops_reading`
- `pane::tests::a_refused_write_keeps_the_composer_probe`
- `pane::tests::a_view_keeps_the_panes_own_modes_and_style`
- `pane::tests::an_ended_pane_resizes_its_screen_not_its_console`
- `pane::tests::clear_buffer_jiggles_when_asked`
- `pane::tests::clear_buffer_keeps_conpty_in_step`
- `pane::tests::clear_buffer_skips_an_ended_pane`
- `pane::tests::clear_to_last_mark_keeps_conpty_in_step`
- `pane::tests::close_asks_for_survivors_of_an_ended_pane`
- `pane::tests::copies_read_the_screen_the_user_selects_in`
- `pane::tests::coprocess_types_into_the_shell`
- `pane::tests::drop_does_not_block`
- `pane::tests::ended_pane_has_no_shell_pid`
- `pane::tests::ended_pane_keeps_its_screen`
- `pane::tests::ended_pane_takes_no_input`
- `pane::tests::exit_code_is_reported`
- `pane::tests::extras_ride_with_the_screen`
- `pane::tests::focus_reports_are_not_held`
- `pane::tests::mute_hides_output_from_the_screen`
- `pane::tests::pump_shows_output`
- `pane::tests::relaunch_keeps_the_screen`
- `pane::tests::resize_to_zero_keeps_a_minimum`
- `pane::tests::selection_writes_keep_state`
- `pane::tests::survivors_are_taken_at_the_exit`
- `pane::tests::the_pwsh_script_emits_prompt_marks`
- `pane::tests::the_pwsh_script_keeps_the_prompt_state`
- `pane::tests::the_shell_generation_moves_with_exit_and_restart`

### Session host is Unsupported (19)

"Session host is not available on Linux yet". Each host test also waits 5 s in `Conn::open` before it fails, which adds minutes to a run.

- `host::tests::detach_resumes_a_paused_session`
- `host::tests::exited_sessions_wait_for_forget`
- `host::tests::host_launches_and_echoes`
- `host::tests::launch_env_reaches_the_session`
- `host::tests::launch_with_runs_the_transport`
- `host::tests::output_waits_while_detached`
- `host::tests::second_client_is_refused`
- `host::tests::versions_one_to_three_are_spoken`
- `host_client::tests::a_busy_host_says_so`
- `host_client::tests::a_changed_watch_clears_hosted_prompts`
- `host_client::tests::a_transport_launch_runs_in_the_host`
- `host_client::tests::a_version_three_host_is_too_old_for_a_transport`
- `host_client::tests::a_version_two_host_keeps_version_two`
- `host_client::tests::an_attached_pane_sets_its_size`
- `host_client::tests::an_older_host_is_spoken_to_in_version_one`
- `host_client::tests::drop_kills_the_session`
- `host_client::tests::hosted_exit_code_arrives`
- `host_client::tests::hosted_pane_runs_a_shell`
- `host_client::tests::reattach_continues_the_output`

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

### Windows-only tools missing (20)

tar.exe, powershell.exe, cmd.exe (mklink), zip creation, and the Windows log-folder rule.

- `app::menu::install_tests::the_pwsh_install_command_adds_one_line_to_the_profile`
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

### Fonts and text metrics (5)

Text widths and face lookup measured against Windows fonts (Cascadia Mono is not installed).

- `render::tests::face_lookup_does_not_scan_fonts_per_call`
- `render::tests::faces_follow_profiles`
- `render::tests::postscript_names_resolve_to_installed_families`
- `render::tests::text_advance_matches_the_cell_width`
- `render::tests::underlines_break_around_glyph_ink`

### Windows path and platform rules in assertions (121)

Expected values are `C:\` paths, `\` separators, drive letters, UNC and subst rules, 8.3 names, PATHEXT, explorer.exe, USERPROFILE, the registry, or a path judge that refuses `/tmp` paths. Named by module below.

- `ai_actions::tests::file_target_follows_windows`
- `ai_actions::tests::get_help_runs_system_powershell`
- `app::dynamic_profiles_app::tests::write_back_stays_in_the_folder`
- `app::git_ui::tests::single_use_windows_follow_iterm2`
- `app::page_saver_app::tests::a_huge_url_list_is_capped`
- `app::page_saver_app::tests::a_large_resource_is_saved_up_to_the_stated_cap`
- `app::page_saver_app::tests::save_flow_follows_iterm2`
- `app::page_saver_app::tests::the_alert_shows_sanitized_text_and_the_door_refuses_types`
- `app::page_saver_app::tests::write_failures_follow_iterm2`
- `app::prefs_ui::tests::a_folder_linked_to_a_network_path_is_never_read_or_written`
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
- `inert_files::tests::write_new_checked_never_overwrites_and_marks`
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
- `profile::tests::a_missing_start_dir_falls_back_to_home_with_a_warning`
- `profile::tests::start_dir_is_home_by_default_and_expands_tilde`
- `remote_prefs::tests::a_folder_linked_to_a_network_path_is_never_probed`
- `remote_prefs::tests::locations_follow_iterm2`
- `script_archive::tests::archive_entries_are_checked`
- `script_archive::tests::archive_layout_follows_iterm2`
- `script_archive::tests::export_follows_iterm2`
- `script_deps::tests::confirmation_names_the_configured_index`
- `script_deps::tests::dependency_editor_follows_iterm2`
- `script_deps::tests::editor_state_follows_iterm2`
- `script_deps::tests::pip_commands_follow_iterm2`
- `script_export_ui::tests::export_rows_follow_the_chooser`
- `script_launch::tests::classify_follows_iterm2`
- `script_launch::tests::launch_env_follows_iterm2`
- `script_new::tests::new_script_paths_follow_iterm2`
- `script_provision::tests::failed_rebuild_keeps_the_old_venv`
- `script_provision::tests::install_uv_follows_iterm2`
- `script_provision::tests::interrupted_swap_is_recovered`
- `script_provision::tests::provision_follows_iterm2`
- `script_provision::tests::update_check_follows_iterm2`
- `script_runtime::tests::commands_follow_iterm2`
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

Time-bounded tests on a loaded debug build, and thread-count tests. `find_over_a_large_buffer_is_quick` is the known flake.

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

### Licenses (1)

`licenses::tests::every_dependency_is_listed`: webview2-com has no license entry (Windows-only crate, absent from the Unix tree).

- `licenses::tests::every_dependency_is_listed`

## Tests hidden from Unix on purpose

A `#[cfg(windows)]` on a test or part of one is listed here and in `scripts/cfg-audit.allow` with its reason. In
`src/app/`: the DACL check in `adblock_app` (SDDL), the exclusive share mode in `archive_ui`, the links-and-devices
test in `emu_ui` (junctions, device names, file symlinks; the symlink test is gated off Unix), the real-HWND tests in
`menu_hold_tests`, the Mark of the Web test in `page_saver_app` (NTFS stream), the ICU pattern test in
`statusbar_content`. Outside `src/app/`: in `url_launch.rs` the lone-surrogate test and the Windows pipe tests, in `host.rs`
the two live ConPTY tests, and the Windows-only tests that sit beside the Windows code they test in `dlog`, `localfs`,
`script_provision`, `url_schemes` and others (counted by the whole-file entries).

## The cfg(unix) unit tests from Tasks 1 to 7

The tests under `platform::unix::*` and the stand-ins ran on Linux and all passed (none failed in the list above). None
runs on macOS yet; the Mac check is compile only.
