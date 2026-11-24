# iTerm2 Upstream Parity (64791fc to a81a54b7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every applicable iTerm2 change made upstream between the fork point 64791fc (2026-09-23) and a81a54b7 (2026-11-23) is in Termurion, with iTerm2's wording and defaults, on Windows, Linux and macOS.

**Architecture:** Phase 1 ports the class A behavior fixes, settings, menus, key actions and API changes, grouped by subsystem, each task test-first and small enough to ship in the next patch release. Phase 2 ports the twelve class B features, one task group each, smallest first and Floating Panes, the SSH Integration overhaul and tmux Workgroups last. Each phase ends on a tree the controller can release. A feature that cannot work on an OS reports "<feature> is not available on <OS> yet" through the existing `platform::Unsupported` type (`src/platform/mod.rs:212`), with the reason this plan gives.

**Tech Stack:** Rust 2024; egui 0.36; winit 0.30 (vendored); vendored `alacritty_terminal` and `vte`; WebView2 (Windows only browser); prost 0.14 for the API; the iTerm2 resources copied from upstream (`framer.py`, shell integration scripts, `ai-models.json`, `it2ssh`). New crates only where a task names them (Task 28: `aes-gcm`, `pbkdf2`, `hmac`, and `unicode-normalization` if not already in `Cargo.lock`).

**Spec:** `docs/port/iterm2-upstream-2026-11-23.md` (the triage: classes A, B, C, file:line in Termurion). Upstream source: `target/iterm2-upstream` (a clone of gnachman/iTerm2; `git log 64791fc..a81a54b7`). Upstream wording sources, in this order: the code string catalog `xcstrings/Localizable.xcstrings` (English value of the named key), the xib catalogs under `sources/**/mul.lproj/*.xcstrings` and `.xib` files, `sources/Settings/iTermAdvancedSettingsModel.m`, and the release notes `docs/notes-3.7.*.txt`. A task that quotes a string quotes it from those; a task that names a key means "copy that key's English value verbatim".

## Global Constraints

- Scope: the three security fixes (Instant Replay allowlist 5964ff23, SetWindowFrame clamp 15d9b471/00ff77e7, kitty `t=f` denylist 807bf9b1) are OUT of this plan; they ship separately. Do not touch `src/app/replay_ui.rs:444-458`, the SetWindowFrame path (`src/marks.rs:1644`, `src/app/requests_ui.rs:295-305`) or the kitty file path check (`src/kitty.rs:80`) except where a task below says so for a different reason.
- iTerm2's exact wording and defaults decide, on all three OSes. "iTerm2" in user-visible text becomes "Termurion" (the project's existing rule, as in `api_proto::MALFORMED`); "Mac" becomes "computer" only where the text is about the user's machine and the existing code already does so (follow the nearest existing string). Menu shortcuts use the project's Shortcut key mapping (`ShortcutKey`, `schema/global.toml`): iTerm2's Cmd is the Shortcut key, Option is Alt.
- Every feature works on Windows, Linux and macOS, or shows and logs "<feature> is not available on <OS> yet" (the `platform::Unsupported` text) with the reason recorded in the task and in the setting's help text. The browser engine is WebView2, so every browser-engine item below reports `Browser is not available on <OS> yet` on Linux and macOS (no embedded browser engine on Unix, `src/platform/unix/browser.rs:1`).
- Each task names the upstream commit(s) it ports. A commit message names them too, as `(iTerm2 cecaae04)` at the end of the subject line or in the body.
- If a task's first test passes on main before any change, that behavior was already ported: keep the test, say so in the task report, and go on to the task's next behavior. Never weaken a test to make it pass.
- No em dashes and no emoji anywhere: code, comments, docs, commit messages. Short human comments, only where the why is not obvious.
- No host or user names in commits. No Claude attribution, no `Co-Authored-By` lines, in commits, tags or PRs.
- Patch version bumps only. Implementers do not push, do not bump the version, and do not edit `docs/FIXES.md`; the controller does all three at each Release checkpoint.
- Tests: Linux `scripts/check-unix --linux-only --test [FILTER]`; macOS `scripts/check-unix --mac-only --test [FILTER]`; Windows `scripts/xt test --lib -- [FILTER]` (full Windows suite with `--test-threads=1`; never two `scripts/xt` at once). Every task's last test step runs all three with the task's filters.
- Known failures that are not regressions: Linux full suite, exactly `.superpowers/sdd/2026-11-18-draw-speed/linux-baseline-failures.txt`; macOS adds `pane::tests::the_pwsh_script_*`; Windows has 3 (`handoff_win::tests::handles_are_typed_before_use`, `platform::windows::pty::tests::a_close_never_happens_while_attached`, `util_query::tests::it2ul_follows_the_script`).
- Settings: a new setting is a row in `schema/global.toml` or `schema/profile.toml` with iTerm2's key, label, default and help text (advanced settings: the `DEFINE_*` description in `iTermAdvancedSettingsModel.m` at a81a54b7, first line as `label`, whole text as `help`). A renamed key goes in `settings_keys::RENAMED` (`src/settings_keys.rs:17`) so old settings files still load. A setting lands in the same task as the behavior it controls, never before.
- API protocol version (`api_proto::PROTOCOL_VERSION`, `src/api_proto.rs:12`): advertise the highest version whose every feature is implemented. Phase 1 ends at "1.20" (Session Note API); Task 50 (Floating Panes API) raises it to "1.23", Task 51 to "1.24" (nested tab group fields land in Task 45 but 1.24 also needs 1.21). The test `api_proto::tests::advertised_version_has_every_feature` (Task 25) holds a table of (version, feature check) and fails when the constant runs ahead.
- Work on main. One commit per Commit step.

## Review Focus

1. **A program's report arrives inside a tmux Integration pane** (DECRQCRA, OSC 7501 query, ReportCellSize, CSI ? 997 n): the reply goes to the pane's program through tmux, not to the gateway's shell. Test: Task 19 `iterm2_only_reports_answer_in_a_tmux_pane`.
2. **Settings files written by an older Termurion** (keys `NoSyncDisableDECRQCRA`, "Show status in tab subtitle" stored state, Session Status priority lists without "done" and "error"): they load with the user's choice kept and the new entries added in iTerm2's order. Tests: Task 1 `the_old_decrqcra_key_still_loads`, Task 12 `saved_priorities_gain_done_and_error_below_waiting`.
3. **A flood**: a program cycling OSC 7501 records, OSC 9;4 reports or titles as fast as it can write. Memory stays bounded and the screen keeps up. Tests: Task 14 `a_status_flood_keeps_one_report_per_record`, Task 11 `a_progress_flood_moves_one_timer`.
4. **An OS without the platform piece** (Linux under Wayland for Lock Location; Linux without `paplay`, `pw-play` or `aplay` for Bell sound; Linux and macOS for the browser engine): the row is shown, says "<feature> is not available on <OS> yet" with the reason, and nothing crashes or silently does nothing. Tests: Task 21 `lock_location_reports_wayland`, Task 10 `bell_sound_reports_no_player`, Task 30 `show_toolbar_reports_no_browser`.
5. **A tab, pane or session that goes away while a dialog about it is open** (Rename Tab, Close Other Tabs confirmation, it2 against a closed session, Control-Tab switcher over a closing tab): the action skips what is gone and never acts on whichever item is current instead. Tests: Task 20 `rename_tab_skips_a_closed_tab`, `close_others_skips_tabs_that_closed`, Task 43 `switcher_drops_a_closed_tab`.

---

## File Structure (main touch points)

- Emulation: `src/emu.rs` (DECRQCRA), `src/screen.rs` (rectangle ops, BDSM/SCP, offers), `crates/vte/src/ansi.rs` and `crates/alacritty_terminal/src/term/mod.rs` (parsing, emoji cells; patches recorded in each crate's `TERMURION-PATCHES.md`), `src/bidi.rs`, `src/images.rs`, `src/kitty.rs`, `src/reports.rs`, `src/progress.rs`, `src/keyproto.rs`, `src/cursor_anim.rs`.
- Create `src/program_status.rs` (OSC 7501 records), `src/lock_location.rs` (pure anchor logic), `src/bell_sound.rs` (sound list and player choice).
- Status: `src/status_tool.rs`, `src/tabstatus.rs`, `src/cc_status.rs`.
- tmux: `src/tmux_gateway.rs`, `src/tmux_controller.rs`, `src/tmux_windows.rs`, `src/tmux_keys.rs`, `src/tmux_sizes.rs`, `src/tmux_persist.rs`, `src/tmux_dashboard.rs`.
- Tabs and windows: `src/tab_menu.rs`, `src/close_rules.rs`, `src/app/close_ui.rs`, `src/tab_groups.rs`, `src/app/windows_ui.rs`, `src/hotkey.rs`.
- Settings and menus: `schema/*.toml`, `src/settings_keys.rs`, `src/keyaction.rs`, `src/app/menu.rs`.
- API: `src/api_proto.rs`, `src/api_pb.rs` (regenerated with `tools/protogen` from upstream `proto/api.proto` at a81a54b7), `src/builtin_fns.rs`, `src/script_fn.rs`, `src/it2_session.rs`.
- AI: `src/ai_catalog.rs`, `resources/ai/ai-models.json`, `src/ai_keys.rs`, `src/ai_model.rs`, `src/ai_models.rs`, `src/settings_ai.rs`.
- Phase 2 creates: `src/floating.rs`, `src/tab_switcher.rs`, `src/menu_bar_item.rs`, `src/exec_permits.rs`, `src/quick_look_hover.rs`, `src/md_render.rs`, `src/flow_monitor.rs`, `resources/ssh/shell_integration/iterm2_shell_integration.nu`, `resources/ssh/it2.py`, `resources/ssh/it2_git_state.py`.

---

# Phase 1: class A behavior, settings, menus and API

## Emulation

### Task 1: DECRQCRA reply width, skipped cells, `DisableDECRQCRA` key

Ports 94932033d, 677f647d0 (checksum part), 11462c19a (the key rename).

**Files:** Modify `src/emu.rs:124-134` (`checksum`, `checksum_reply`), `src/screen.rs:1829` (the rectangle walk), `src/emu.rs:73,101`, `src/globals.rs:146`, `src/settings_keys.rs:17` (`RENAMED`), `schema/global.toml:4530`. Test: `src/emu.rs` tests, `src/screen.rs` tests, `src/globals.rs` tests.

**Interfaces:** Produces `emu::checksum_reply(id: i64, sum: u32) -> String` (always four lowercase hex digits: `sum & 0xffff`), `emu::checksum(cells: impl Iterator<Item = char>) -> u32` (wrapping unsigned sum), and the key `"DisableDECRQCRA"` (constant `emu::DECRQCRA_ID`).

- [ ] **Step 1: Write the failing tests.** In `emu.rs` `decrqcra_follows_iterm2`: replace the old assert with `checksum_reply(1, 40000) == "\x1bP1!~9c40\x1b\\"` and add `checksum_reply(1, 0x1_0000 + 5) == "\x1bP1!~0005\x1b\\"`. In `screen.rs` new `decrqcra_skips_wide_halves_and_tab_fillers`: feed "\u{4e00}" (wide) then ask `CSI 1;1;1;1;1;2 * y`; the sum counts U+4E00 once, not its spacer; feed "\tA" and the checksum of columns 1..9 counts only "A". New `the_old_decrqcra_key_still_loads` in `globals.rs`: a file with `NoSyncDisableDECRQCRA = false` loads as `DisableDECRQCRA == Some(false)`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test decrqcra the_old_decrqcra`. Expected: FAIL.
- [ ] **Step 3: Implement.** Rename the key everywhere (`emu.rs`, `globals.rs` `KEPT_WHEN_DEFAULT`, schema row key), add `("NoSyncDisableDECRQCRA", "DisableDECRQCRA")` to `RENAMED`. The schema label and help stay "Disable DECRQCRA?" / "Disable DECRQCRA?\nThis control sequence allows an app running in the terminal to read its contents.". The rectangle walk skips a cell that is the right half of a wide character or a tab filler.
- [ ] **Step 4: Run** the same filters on Linux, macOS and `scripts/xt test --lib -- decrqcra the_old_decrqcra`. Expected: PASS.
- [ ] **Step 5: Commit** `DECRQCRA: four hex digits, skip wide halves and tab fillers, key DisableDECRQCRA (iTerm2 94932033d 677f647d0 11462c19a)`.

### Task 2: DECCRA, DECFRA, DECERA, and DCH at the right margin

Ports 677f647d0, 94932033d (DECCRA in origin mode), eb6043e19 (clamps). Termurion has no rectangle-area operations today (the triage lists them as missing).

**Files:** Modify `crates/vte/src/ansi.rs` (dispatch `CSI Pts;Pls;Pbs;Prs;Pps;Ptd;Pld;Ppd $ v`, `CSI Pch;Pt;Pl;Pb;Pr $ x`, `CSI Pt;Pl;Pb;Pr $ z` to new `Handler` methods), `crates/alacritty_terminal/src/term/mod.rs` (implement them and DCH), both `TERMURION-PATCHES.md`. Test: `crates/alacritty_terminal` tests via `src/screen.rs` feed tests.

**Interfaces:** Produces `Handler::copy_rect(&mut self, src: Rect1, dst_top: i64, dst_left: i64)`, `Handler::fill_rect(&mut self, ch: u32, r: Rect1)`, `Handler::erase_rect(&mut self, r: Rect1)` where `Rect1 { top, left, bottom, right }` are the 1-based parameters before clipping.

- [ ] **Step 1: Write the failing tests** in `src/screen.rs`: `deccra_reads_destination_row_then_column` (copy rows 1..2 col 1..2 to `Ptd=5;Pld=10`: cells land at row 5 col 10); `deccra_destination_counts_from_margins_in_origin_mode` (DECOM on, DECSTBM 3..10, DECSLRM 5..20: `Ptd=1;Pld=1` means row 3 col 5); `decfra_rejects_out_of_range_characters` (`CSI 65601;1;1;1;1 $ x` fills nothing; `CSI 65;...` fills "A"; only 32..126 and 160..255 fill, as xterm); `decera_erases_to_blank_keeping_nothing` ; `dch_at_the_right_margin_deletes_that_cell` (cursor moved to the last column with CUP, `CSI P` removes the cell; right after printing in the last column, pending wrap, `CSI P` deletes nothing); `huge_rect_params_clip_to_the_screen` (`2147483639` in every slot completes and changes only on-screen cells).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test deccra decfra decera dch_at huge_rect`. Expected: FAIL.
- [ ] **Step 3: Implement** per xterm and the upstream commits: page parameters are ignored; rectangles clip to the screen (and to the margins in origin mode); copy goes through a temporary so overlaps are right.
- [ ] **Step 4: Run** the filters on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Emulation: DECCRA, DECFRA and DECERA, and DCH at the right margin (iTerm2 677f647d0 94932033d eb6043e19)`.

### Task 3: BDSM and SCP control right-to-left reordering

Ports 91411f536, 9d5e5b4ea, 3aa834f1a.

**Files:** Modify `crates/vte/src/ansi.rs:746,1668` (SCP is parsed; route `CSI Ps SP k` and ANSI mode 8 to Termurion), `src/screen.rs`, `src/bidi.rs`, `schema/global.toml` (new row), `src/restore_state.rs` (save both values). Test: `src/bidi.rs`, `src/screen.rs`.

**Interfaces:** Produces `bidi::LineDir { Default, Ltr, Rtl }` stored per line, `Screen::bidi_explicit: bool`, and the advanced setting `HonorBidiSupportModeEscapeSequence` (default `true`, label "Let apps control right-to-left reordering with the BDSM and SCP escape sequences?", help the full `honorBidiSupportModeEscapeSequence` text, section "Experimental").

- [ ] **Step 1: Write the failing tests:** `csi_8_l_writes_text_unreordered` (RTL support on; `CSI 8 l` then Hebrew text: the line's RTL flag and per-cell RTL status are clear; `CSI 8 h` then the same text is reordered); `scp_sets_the_base_direction` (`CSI 2 SP k` then mixed text: the line records `Rtl`; `CSI 0 SP k` restores `Default`); `decrqm_8_reports_the_mode` (`CSI 8 $ p` answers `CSI 8;1 $ y` in implicit mode, `;2` in explicit; with RTL support off it answers `;4`); `the_setting_off_ignores_both`; `reset_restores_implicit_and_default`; `turning_reordering_off_drops_alternate_screen_tables` (3aa834f1a).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test csi_8_l scp_sets decrqm_8 bidi`. Expected: FAIL.
- [ ] **Step 3: Implement.** The direction rides in line metadata so saved sessions keep it; an explicit-mode write clears the line's sticky RTL flag.
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Bidi: BDSM and SCP escape sequences (iTerm2 91411f536 9d5e5b4ea 3aa834f1a)`.

### Task 4: Inline and kitty image sizing, limits and cache order

Ports 94932033d (sizing), 0caccf136 and 500c4799b (bounds), 3c84279a9 (LRU reads count), fdd88955b (release images drawn over), 923838c76 and 5f1786330 (cropped placeholders).

**Files:** Modify `src/images.rs:143-154`, `src/kitty.rs:25` (eviction), `src/kitty.rs` placement and decode paths, `src/scene.rs` (placeholder crop). Test: `src/images.rs`, `src/kitty.rs`.

**Interfaces:** Produces `kitty::Kitty::touch(&mut self, id: u32)` called whenever an image is drawn; `images::fit(...)` keeps its signature.

- [ ] **Step 1: Write the failing tests:** `a_too_wide_image_shrinks_to_the_columns_left` (grid 80, cursor col 60, image 40 cells wide keep-aspect: width 20, height scaled by 20/40); `a_tall_image_capped_at_255_rows_narrows_in_proportion` (1000 rows wide 100: rows 255, width `ceil(100*255/1000)`); `kitty_evicts_the_least_recently_displayed` (three images over budget; drawing the oldest makes the middle one go); `kitty_rows_cap_at_65535` (`r=4294967295` moves the cursor at most 65535 lines and returns); `kitty_raw_size_overflow_is_rejected` (`s=4294967295,v=4294967295,f=32` gives the error reply, no panic); `kitty_truncated_zlib_is_an_error`; `an_image_drawn_over_is_released` (an image whose every cell is overwritten by a newer image is dropped from the store); `a_cropped_placeholder_draws_its_part` (placeholder cells of a `y=10,h=20` crop sample rows 10..30 only).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test a_too_wide a_tall_image kitty_ an_image_drawn a_cropped`. Expected: FAIL.
- [ ] **Step 3: Implement.** Keep `kitty::BUDGET` (320 MiB); change only the order. Update the doc comment at `kitty.rs:24` to "the least recently displayed goes first".
- [ ] **Step 4: Run** on all three OSes plus `render_pixel`. Expected: PASS.
- [ ] **Step 5: Commit** `Images: iTerm2's sizing, bounds and least-recently-displayed kitty cache (iTerm2 94932033d 0caccf136 500c4799b 3c84279a9 fdd88955b 923838c76)`.

### Task 5: Skin-toned emoji in one cell, and combining-mark fixes

Ports ec087cf9b, f63e43701, 4ef38be0a, 1f95cec9c.

**Files:** Modify `crates/alacritty_terminal/src/term/mod.rs:288-289,1631-1645` (the `is_modifier` rule), its `TERMURION-PATCHES.md`, `src/bidi_mirror.rs`, `src/widths.rs`. Test: `src/screen.rs` feed tests.

- [ ] **Step 1: Write the failing tests:** `a_skin_tone_after_a_modifier_base_joins_one_wide_cell` ("\u{1F44D}\u{1F3FB}" occupies columns 0..2 as one cell holding both code points); `a_skin_tone_after_anything_else_keeps_its_own_cell` ("a\u{1F3FB}"); `a_skin_tone_in_a_later_write_merges` (feed "\u{1F44D}" then "\u{1F3FB}" separately: one cell); `spacing_combining_runs_do_not_panic` (1000 U+0903 after "a", and at column 0); `non_bmp_mirror_pairs_are_skipped`; `writing_on_a_wide_right_half_lands_in_place` (1f95cec9c: cursor on the spacer, print "x": "x" at that column, not two to the right).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test skin_tone spacing_combining non_bmp writing_on_a_wide`. Expected: FAIL.
- [ ] **Step 3: Implement.** Emoji_Modifier_Base comes from the Unicode emoji data table already used for pictographs.
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Cells: a skin-toned emoji is one two-column cell (iTerm2 ec087cf9b f63e43701 4ef38be0a 1f95cec9c)`.

### Task 6: Offers for modes a program left on: light/dark reports, key reporting

Ports dcaf334ad (mode 2031), 1acab0d1d (stuck key reporting offer).

**Files:** Modify `src/stuck_mouse.rs` (the leftover-report detector), `src/app/host_offers_ui.rs:52` (the offer list), `src/screen.rs` (whole-value kitty flag writes), `schema/global.toml` (new row), `src/meta_frustration.rs` if that is where the key-reporting offer lives (`grep -n "key reporting" src`). Test: `src/stuck_mouse.rs`, `src/app/host_offers_ui.rs`.

**Interfaces:** Produces `Offer::DarkModeReporting` with text `PTYSession.DarkModeReportingLeftOn` ("Looks like light/dark mode change reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?"), the advanced setting `NoSyncNeverAskAboutDEC2031Frustration` (default `false`, label and help "Suppress the notification asking if you want to disable light/dark mode change reporting", section "Warnings"), and `stuck_keys::ShellOwnsMode` (the verdict recomputed at each command start).

- [ ] **Step 1: Write the failing tests:** `an_echoed_997_report_offers_to_turn_2031_off` (mode 2031 on, the shell echoes "997;1n" at the prompt: the offer appears once; Yes turns 2031 off); `a_host_change_turns_2031_off` (shell integration reports a new host); `the_suppress_setting_skips_the_offer`; `a_shell_that_writes_the_whole_key_mode_gets_no_offer` (between FTCS D and the next FTCS C the shell sends `CSI = 0 ; 1 u`; an app then leaves flags on: no offer at the next prompt); `push_and_pop_do_not_count_as_the_shell_owning_it`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test 997_report 2031 the_suppress whole_key_mode push_and_pop`. Expected: FAIL.
- [ ] **Step 3: Implement** using the existing mouse and DEC 2048 leftover detection as the pattern.
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Offers: turn off leftover light/dark reports; no key reporting offer when the shell owns the mode (iTerm2 dcaf334ad 1acab0d1d)`.

### Task 7: Keys, input methods and pointer

Ports e71838655, 83a0a8dfd, beb7cfbff, df780a158, 7f4ff7397, 52fdf260e, f5415b024, f49137e8a, 06e2a2c32, 3eca5d044.

**Files:** Modify `src/keyproto.rs`, `src/keys.rs`, `src/ime.rs`, `src/bindings.rs` (physical-key matching), `src/pointer_shape.rs`, `src/mouse.rs`, `src/wheel.rs`. Test: those files' tests.

- [ ] **Step 1: Write the failing tests:** `csi_u_with_option_as_meta_sends_the_meta_byte` (CSI u reporting on, Alt set to Meta, Alt+a sends `ESC a`); `a_dead_key_that_does_not_combine_reports_both` (kitty report-all: dead `´` then `x` reports the dead key then `x`); `caps_lock_input_switch_sends_no_malformed_report` (macOS only test, `#[cfg(target_os = "macos")]`: a Caps Lock press that only switches the input source reports a well-formed Caps Lock key event, never a bare `CSI ; u`); `old_bindings_do_not_hijack_a_in_physical_mode` (a saved binding with an empty character and keycode 0 never matches the A key; no offer to turn physical mode on for ^A); `arrow_or_tab_while_composing_commits_and_acts` (IME preedit "ㅎ", Left: commit then send `CSI D`, one press); `return_and_backspace_while_composing_act_once`; `alt_press_shows_the_ibeam_while_mouse_reporting` (pointer shape switches to I-beam on Alt down, back on Alt up, without moving); `mode_1003_reports_motion_at_once` (after `CSI ? 1003 h`, the next motion event is reported with no other event first); `a_double_click_in_an_unfocused_reporting_pane_reports_both`; `sideways_scroll_never_scrolls_history_with_vertical_only_alt_scroll` ("Restrict alternate mouse scroll to vertical scrolling" on, alternate screen: a horizontal delta never moves the scrollback).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test csi_u_with a_dead_key old_bindings composing alt_press mode_1003 a_double_click sideways_scroll` and `--mac-only --test caps_lock_input`. Expected: FAIL.
- [ ] **Step 3: Implement** each to pass.
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Input: CSI u meta byte, dead keys, IME commits, pointer and scroll fixes (iTerm2 e71838655 83a0a8dfd beb7cfbff df780a158 7f4ff7397 f5415b024 f49137e8a 06e2a2c32 3eca5d044)`.

### Task 8: Cursor animation, smooth typing and ligatures

Ports c54b27dad, ea23bff03, 31f52b277, 124020064.

**Files:** Modify `src/cursor_anim.rs`, `src/app/cursor_ui.rs:229`, `src/scene.rs` (cursor over ligatures), `src/fonts.rs` (disabled ligatures), `schema/profile.toml` (help of "Cursor Smooth Slide"), `schema/global.toml` (new row). Test: `src/cursor_anim.rs`, `src/scene.rs`.

**Interfaces:** Produces the advanced setting `CursorAnimationInputWindow` (float, default `1.0`, label "When animated cursor movement is enabled, only animate a movement that happens within this many seconds of your last keystroke or mouse action.", help the full upstream text, section "Drawing") and `Pane::last_user_input: Option<Instant>` (set by keystrokes, mouse reports, password entry and broadcast fan-out).

- [ ] **Step 1: Write the failing tests:** `a_large_jump_long_after_input_does_not_smear`; `a_large_jump_right_after_input_smears`; `the_window_setting_moves_the_cutoff`; `smooth_slide_includes_the_block_cursor` (profile help now "Whether to smoothly slide underscore, vertical bar, and block cursors"); `a_block_cursor_keeps_a_ligature_whole` ("--" in a ligature font, cursor on the second "-": both glyphs drawn, cursor cell inverted only); `disabled_ligatures_stay_off_when_not_preferring_speed` ("fi" with ligatures off draws two glyphs).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test a_large_jump the_window_setting smooth_slide a_block_cursor disabled_ligatures`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes and `render_pixel`. Expected: PASS.
- [ ] **Step 5: Commit** `Cursor: animate only after input, smooth block cursor, whole ligatures (iTerm2 c54b27dad ea23bff03 31f52b277 124020064)`.

### Task 9: HDR cursor brightness setting

Ports ed001e0bd, d80345a8e, a345740c8.

Termurion draws to an SDR surface on every OS (the wgpu surface format is sRGB 8-bit; there is no extended-range output), so the HDR cursor itself is not available anywhere yet. The keys are added so settings round-trip with iTerm2, and the row says why it does nothing.

**Files:** Modify `schema/profile.toml` (new row after "HDR Cursor", line 545), `src/settings_colors.rs:136`. Test: `src/schema.rs` tests, `src/settings_colors.rs`.

**Interfaces:** Produces the profile key `HDR Cursor Brightness` (float, `light_dark = true`, default 16, range 2 to 16 in whole steps, label "HDR cursor brightness", help the slider tooltip "Peak brightness of the HDR cursor, as a multiple of normal white. The display shows at most what its current HDR headroom allows, so higher values may look the same." followed by a blank line and `HDR cursor is not available on <OS> yet: Termurion draws to a standard dynamic range surface.`).

- [ ] **Step 1: Write the failing tests:** `hdr_brightness_follows_iterm2` (key, defaults 16 for plain, light and dark, range 2..16, step 1); `the_hdr_rows_say_why_they_do_nothing` (the help of "HDR Cursor" and "HDR Cursor Brightness" ends with the Unsupported text for the running OS).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test hdr_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Profiles: HDR Cursor Brightness setting (iTerm2 ed001e0bd d80345a8e)`.

### Task 10: Bell sound per profile

Ports e798b2297, dd0e08275, 30a3a507a.

**Files:** Create `src/bell_sound.rs`; modify `schema/profile.toml` (row in Terminal tab near "Silence bell", line 2324), `src/bell.rs`, `src/platform/clock.rs` and `src/platform/{windows,unix}/clock.rs` (`play_file`), `src/platform/macos/` (an NSSound call; add the `NSSound` feature to `objc2-app-kit` in `Cargo.toml`), the profile Terminal settings page. Test: `src/bell_sound.rs`.

**Interfaces:** Produces the profile key `Bell Sound` (string, default `""` meaning the system alert sound; label "Bell sound:"; help "The sound the terminal bell plays, or empty for the system alert sound"); `bell_sound::installed() -> Vec<(String, PathBuf)>` (Windows `%WINDIR%\Media\*.wav`; macOS `/System/Library/Sounds`, `/Library/Sounds`, `~/Library/Sounds`; Linux `/usr/share/sounds/freedesktop/stereo` and `~/.local/share/sounds`); `bell_sound::resolve(value: &toml::Value) -> Choice` (`Choice::System`, `Choice::File(PathBuf)`; a non-string value is `System`, 30a3a507a); `platform::play_file(path: &Path) -> Result<(), String>` (Windows `PlaySoundW` with `SND_FILENAME`; macOS `NSSound`; Linux the first of `pw-play`, `paplay`, `aplay` on PATH, spawned detached). Menu entries: "System Alert Sound", the installed names, "Other…" (file dialog titled "Choose the sound the bell should play."). An unplayable file shows "This sound can’t be played, so the bell plays the system alert sound." under the menu.

- [ ] **Step 1: Write the failing tests:** `bell_sound_follows_iterm2` (key, default, label); `a_non_string_value_is_the_system_sound`; `installed_lists_each_os_folder` (temp folder fixtures per OS); `bell_sound_reports_no_player` (Linux, no player on a fake PATH: the row's note is "Bell sound is not available on Linux yet: no pw-play, paplay or aplay was found." and the bell falls back to today's behavior); `an_unplayable_file_falls_back`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test bell_sound a_non_string installed_lists an_unplayable`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS. Controller listens once per OS.
- [ ] **Step 5: Commit** `Profiles: Bell sound (iTerm2 e798b2297 dd0e08275 30a3a507a)`.

### Task 11: OSC 9;4 timeout, paused spinner, spinner follows Show activity indicator

Ports 70a4ca8bc, 49936d3a5 (paused-indeterminate state), 7e862eb49.

**Files:** Modify `src/progress.rs:9-110`, `src/osc.rs:77`, the progress timer in `src/app/progress_ui.rs`, `src/app/activity_ui.rs:122`, `src/tab_layout.rs:835`, `schema/global.toml` (new row). Test: `src/progress.rs`.

**Interfaces:** Produces `Progress::PausedIndeterminate` (drawn as a still yellow band in the pane and a pause glyph in the tab), `progress::Timeout { deadline: Option<Instant> }` with `fn report(&mut self, now: Instant, secs: f64)` (moves one deadline) and `fn expired(&self, now: Instant) -> bool`, and the advanced setting `ProgressReportTimeout` (float, default 15, label "Hide a progress bar reported with OSC 9;4 after this many seconds without an update.", help the full upstream text, section "Terminal").

- [ ] **Step 1: Write the failing tests:** `a_bar_hides_after_15_seconds_without_a_report`; `zero_keeps_the_bar`; `a_newer_report_moves_the_deadline`; `a_restored_bar_starts_the_timer`; `9_4_4_over_a_spinner_is_a_paused_spinner`; `a_progress_flood_moves_one_timer` (10 000 reports leave one deadline); `hide_activity_indicator_hides_the_tab_spinner`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test progress a_bar_hides zero_keeps 9_4_4 hide_activity`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Progress: 15-second timeout, paused spinner, spinner follows Show activity indicator (iTerm2 70a4ca8bc 49936d3a5 7e862eb49)`.

## Session Status

### Task 12: Session Status: "Show status on tabs", done and error, expiring statuses, the status variable

Ports d81b6f6ea, 49936d3a5 (done and error, order), b3e73418d, d4756ce40, 7e85a540f, 84dc60625, 87d83b8f2, and the `status` session variable (3.7.5 notes).

**Files:** Modify `src/status_tool.rs:319-324` (label, help text), `src/tabstatus.rs`, `src/api_vars.rs` (session variable `status`), `src/builtin_fns.rs` (`set_status` gains `expires_on`, `progress`), `src/cc_status.rs` (turn statuses scoped to the turn), `src/restore_state.rs`. Test: `src/status_tool.rs`, `src/tabstatus.rs`.

**Interfaces:** Produces `status_tool::SUBTITLE_LABEL == "Show status on tabs"` (renamed constant `SHOW_ON_TABS_LABEL`), default priority list `["waiting", "done", "error", "working", "idle"]`, `TabStatus::expires_on: Option<ExpiresOn>` with `ExpiresOn::ProgressEnd` (the only value; others answer `SetStatus.UnknownExpiresOn` "Unknown expires_on “%@”. The only supported value is progress-end."), `progress` values `running` and `stopped` (others `SetStatus.UnknownProgress`).

- [ ] **Step 1: Write the failing tests:** `the_tabs_checkbox_follows_iterm2` (label "Show status on tabs"; off hides the status text and the colored dot on the tab); `the_default_order_is_waiting_done_error_working_idle`; `saved_priorities_gain_done_and_error_below_waiting` (a saved list without them gets both inserted after "waiting", with idle's notify setting); `typing_in_a_done_session_turns_it_idle_without_notifying`; `a_status_expires_when_its_progress_ends`; `an_unknown_expires_on_is_refused_with_iterm2s_text`; `the_status_variable_holds_the_status`; `acknowledging_is_not_a_change_for_notify`; `a_restored_status_shows_on_the_tab`; `the_help_text_follows_iterm2` (HELP_TEXT equals the upstream `ToolStatus.helpMarkdown` at a81a54b7 with "iTerm2" as Termurion).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test status_tool tabstatus the_status_variable`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Session Status: Show status on tabs, done and error, expiring statuses (iTerm2 d81b6f6ea 49936d3a5 b3e73418d d4756ce40 84dc60625 87d83b8f2)`.

### Task 13: Session Status: user colors, background status, agent reports, Zellij

Ports 9c191a02f, d6b8b0d88, 3e937a620, 569dde456, and the 3.7.6 note "Session Status now follows a session rename".

**Files:** Modify `src/status_tool.rs`, `src/status_popover_ui.rs` (two color wells per pattern), `src/tabstatus.rs` (draw-time override), `src/cc_status.rs`, `src/claude_code.rs`. Test: `src/status_tool.rs`, `src/cc_status.rs`.

**Interfaces:** Produces `Priority { pattern, notify, dot: Option<Rgb>, text: Option<Rgb> }` (saved in the existing priority list; empty means the program's color), column titles "Dot" and "Text" (`StatusPriority.DotColorColumn`, the Text column's key next to it), tooltips from `StatusPriority.DotColorTooltip` and its Text twin; `cc_status::BACKGROUND` status "background" with color purple as upstream `CS` (copy the hex from `cc-status/Sources` at a81a54b7).

- [ ] **Step 1: Write the failing tests:** `a_dot_override_recolors_only_existing_dots`; `a_text_override_applies_without_a_program_color`; `the_first_matching_pattern_decides`; `stored_and_api_colors_stay_the_programs`; `a_turn_over_with_work_running_is_background`; `agent_status_reports_merge_with_the_hook` (an OSC 7501 report from the agent and the hook's last message both shown); `inside_zellij_reports_no_status` (`ZELLIJ` set in the hook's environment: no status sent); `status_follows_a_rename`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test status_tool cc_status`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Session Status: user colors, background status, agent reports (iTerm2 9c191a02f d6b8b0d88 3e937a620 569dde456)`.

### Task 14: Program Status Protocol (OSC 7501)

Ports 49936d3a5, a63ed419c, 257d58637, f084c3584, a89169cb2.

**Files:** Create `src/program_status.rs` (grammar, validation, records); modify `src/osc.rs` (route OSC 7501), `src/screen.rs` (feature query reply), `src/status_tool.rs` (child rows), `src/progress.rs` (records drive the bar), `src/tmux_gateway.rs` (answer the query in tmux panes). Test: `src/program_status.rs`.

**Interfaces:** Produces `program_status::Record { id: String, state: State, message: Option<String>, progress: Option<u8>, title: Option<String> }`, `State { Idle, Working, Blocked(Reason), Done, Error }` with Reason texts `ProgramStatus.NeedsAnswer` "Waiting for an answer", `NeedsApproval` "Waiting for approval", `NeedsLogin` "Waiting for login"; `Records::apply(&mut self, batch: &[Report]) -> Changes` (latest per record, clears as barriers), cap 256 records per session; child rows at most 4 plus "+N".

- [ ] **Step 1: Write the failing tests:** grammar and limits from the spec copied by upstream (`sources/ProgramStatus*` at a81a54b7: every rejected form is ignored); `the_feature_query_is_answered` (also in a tmux pane); `done_and_error_survive_the_prompt_and_exit`; `the_most_urgent_record_is_the_tab_status`; `records_drive_the_bar_without_starting_9_4`; `a_status_flood_keeps_one_report_per_record` (one million alternating reports in one batch leave one per record and one tab update); `four_child_rows_and_a_plus_n_row`; `notifications_are_rate_limited_per_session`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test program_status`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Session Status: the Program Status Protocol, OSC 7501 (iTerm2 49936d3a5 a63ed419c 257d58637 a89169cb2)`.

## Shell integration, find and commands

### Task 15: Right-prompt marks, tcsh script, scripts at a81a54b7

Ports 71309eb6b, eaee1c8f4, 731e0e23c, c18299754, 8febee7d4, 3a6a0cb14, cd75c6170, 8a7240ece, and the fish 4.0.6 fix (3.7.8 notes). Termurion ships no tcsh script today (`src/shell_wizard.rs:83`); the triage asks for it.

**Files:** Replace `resources/ssh/shell_integration/iterm2_shell_integration.{bash,fish,zsh,xonsh}` with upstream `Resources/shell_integration/` at a81a54b7 (keep Termurion's 4-line xonsh change if still needed; diff first); add `iterm2_shell_integration.tcsh`; modify `src/ssh_inject.rs:22` (embed list), `src/shell_wizard.rs:83,821` (tcsh is a known shell), `src/marks.rs` and `src/prompt_state.rs` (OSC 133;P;k=r), the command extractor (`src/cmdsel.rs`), `src/autocomplete.rs` ("awss"). Test: those files.

**Interfaces:** Produces `prompt_state::RightPrompt` ranges excluded from the recorded command, and the extractor null policy "a null after content on a line ends that line".

- [ ] **Step 1: Write the failing tests:** `a_marked_right_prompt_is_not_in_the_command`; `an_unmarked_rprompt_after_nulls_is_not_in_the_command`; `a_long_command_reaching_the_right_prompt_keeps_every_character`; `select_current_command_keeps_text_typed_over_a_hidden_right_prompt`; `auto_completion_inserts_only_what_is_missing` (typed "aws" with an autosuggestion after the cursor: choosing "aws s3" inserts " s3"); `tcsh_is_a_known_shell`; `the_embedded_scripts_match_upstream` (sha256 of each embedded script equals the file in `resources/`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test right_prompt rprompt select_current auto_completion tcsh the_embedded_scripts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller runs the shell-integration live check (zsh, fish, bash, tcsh) on Linux and macOS.
- [ ] **Step 5: Commit** `Shell integration: right-prompt marks, tcsh, scripts from iTerm2 a81a54b7 (iTerm2 71309eb6b 731e0e23c c18299754 8febee7d4)`.

### Task 16: Find, selection and folds

Ports fe54bd10c, 710d0f737, 754761537, 1429bf528, fcbc399f4, b4e21eb62, and "Selecting a different command while Find is open now searches the newly selected command" (3.7.8beta1).

**Files:** Modify `src/find.rs`, `src/screen.rs` (cells the cursor skipped), `src/app/selection_ui.rs:308-329`, `src/wordsel.rs`, `src/fold.rs:5`. Test: those files.

**Interfaces:** Produces `fold::summary(first: &Row, last: &str, n: usize, cols: usize) -> Row` (keeps the first line's styles; a double-width or double-height first line draws single).

- [ ] **Step 1: Write the failing tests:** `find_treats_skipped_cells_as_spaces` (feed `a CSI C b`: a search for "a b" matches, the skipped cell counting as a space as in fe54bd10c); `a_wrapped_backward_search_stops_before_its_first_match`; `a_selection_of_only_nulls_is_dropped_on_resize`; `clicking_a_command_with_find_open_does_not_scroll`; `a_new_selected_command_is_searched`; `word_regex_matches_across_a_soft_wrap`; `a_fold_keeps_the_first_lines_colors`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test find_ selection wordsel fold`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Find and folds: skipped cells, wrapped backward search, styled fold summary (iTerm2 fe54bd10c 710d0f737 754761537 1429bf528 b4e21eb62)`.

### Task 17: Commands, Command Info and captured output

Ports 72c4fa851, a39abb8b6, f53ab9c4f, 2d98231fb, d7527f799, ccf07f53a.

**Files:** Modify `src/cmdinfo.rs`, `src/marks.rs`, `src/captured.rs`, `src/ai_terminal.rs` (what AI and Codecierge are told). Test: those files.

- [ ] **Step 1: Write the failing tests:** `output_size_stops_before_the_next_prompt`; `output_size_ignores_zshs_percent_marker`; `a_quick_command_never_shows_negative_time`; `a_prompt_taller_than_four_lines_reads_the_right_line`; `saved_output_keeps_the_last_2000_lines_and_says_how_many_were_left_out` (text from upstream `d7527f799`); `an_aborted_command_is_reported_as_aborted` (no exit status 0).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test cmdinfo output_size a_prompt_taller saved_output an_aborted`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Commands: Command Info size and time, 2,000-line output, aborted commands (iTerm2 72c4fa851 a39abb8b6 d7527f799 ccf07f53a)`.

## tmux Integration

### Task 18: tmux protocol: %exit, zoom, sizes, keys

Ports cecaae04d, ba2f3ca8b, 273ac6d36, d135a673f, a8d85041e, fcde8a6a9, ad80bfc58, e9dfc4fa4, 97616c127, a90c74604, 795e2aaba, 8242815bc.

**Files:** Modify `src/tmux_gateway.rs:290-310`, `src/tmux_controller.rs:1480-1485`, `src/tmux_sizes.rs`, `src/tmux_layout.rs`, `src/tmux_keys.rs`. Test: those files.

**Interfaces:** Produces `Gateway::set_end_guard_optional(&mut self, optional: bool)` (true until the version is known and for tmux older than 1.9); window opens wait until the version is known.

- [ ] **Step 1: Write the failing tests:** `exit_inside_a_reply_is_data_on_1_9_and_later` (the `%begin`/`%end` block carrying "%exit" keeps the connection); `exit_ends_an_unclosed_reply_on_1_8`; `a_zoomed_window_stays_zoomed_on_3_3` (no `resize-pane -Z` on attach; a `list-windows` layout request instead; the named zoomed pane becomes active without telling tmux); `older_tmux_still_unzooms`; `a_zoomed_grid_survives_a_layout_update`; `resizing_with_a_zoomed_pane_does_not_oscillate`; `a_refused_size_request_is_retried`; `tmux_3_8_opens_at_the_profile_size`; `attaching_in_tabs_mode_does_not_grow_split_windows`; `each_pane_uses_its_own_right_gutter`; `a_divider_drag_ends_where_it_showed`; `home_and_end_keep_their_modifiers` (Ctrl+Home sends `CSI 1;5 H` through `send-keys`); `key_encoding_is_stable_across_a_detach`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tmux_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller attaches to tmux 3.4 in WSL and a Linux tmux 3.5 with a zoomed window.
- [ ] **Step 5: Commit** `tmux: %exit inside replies, zoom kept on attach, size and key fixes (iTerm2 cecaae04d ba2f3ca8b 273ac6d36 d135a673f ad80bfc58 795e2aaba 8242815bc)`.

### Task 19: tmux sessions, windows and replies

Ports d5b299707, 6aba6a57c, fb30c1ede, 5cbc7d7b1, 6f5030ecb, 3fe71fd29, 54d0da776, 8647c2be2, cb2749b60, 3ec44777a, aa10c177a, 6c7024a6a, d2de7d6a4, 8bf82e429, 7f9f24a1c, 3932d4608, db8daf71b, 94a40e856.

**Files:** Modify `src/tmux_controller.rs`, `src/tmux_windows.rs`, `src/tmux_persist.rs`, `src/tmux_dashboard.rs`, `src/tmux_hook.rs` (nested `-CC`), `src/archive.rs`, `src/app/tmux_ui.rs`, `src/app/tmux_dashboard_app.rs`. Test: those files.

**Interfaces:** Produces the warning text `PTYSession.TmuxIntegrationCannotNest` ("tmux integration can’t start in a session that is already part of tmux integration. The new tmux client is waiting in control mode; press Return in this session to end it.", "iTerm2" kept as is since it is about tmux integration); tab groups saved per tmux session in the session's `@iterm2_*` option as upstream names it.

- [ ] **Step 1: Write the failing tests:** `new_window_d_opens_a_background_tab`; `tmux_cc_inside_a_tmux_pane_explains`; `iterm2_only_reports_answer_in_a_tmux_pane` (ReportCellSize, ReportWindowFrame, ReportScreenFrames, the OSC 7501 query); `closing_a_tmux_pane_saves_no_archive`; `always_closes_only_windows_the_detach_left`; `one_sessions_window_change_does_not_switch_another_sessions_tab`; `detach_closes_every_tmux_session_in_the_window`; `custom_titles_and_groups_stay_per_session`; `tab_groups_come_back_after_reattach`; `the_dashboard_selects_the_attached_session`; `user_vars_reported_while_opening_are_kept`; `quit_waits_for_a_buried_gateway`; `a_hotkey_window_does_not_move_to_a_tmux_position`; `a_native_session_never_joins_a_tmux_tab`; `selecting_a_closed_window_keeps_the_connection`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tmux_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `tmux: background windows, nesting warning, reports in panes, per-session titles and groups (iTerm2 d5b299707 6aba6a57c fb30c1ede aa10c177a 54d0da776)`.

## Tabs, windows and menus

### Task 20: Tabs: Rename Tab, batch close confirmation, titles

Ports f74abc16e, 7ea65c742, 6b64d5802, f18c79a8a, b754d09e6, 15e5a48b8, 0e2105cdc, dcd01a915, db940e958, 5080209f8, d89b08f69, 62df824da, 71fb6def7, 074d280c8, f4b87c0d0.

**Files:** Modify `src/tab_menu.rs:66-160,211`, `src/close_rules.rs`, `src/app/close_ui.rs`, `src/tab_layout.rs:547` (HTML titles in pane title bars), `src/pane_title.rs`, `src/tab_groups.rs`, `src/tab_drag.rs`, `src/arrangement.rs`. Test: those files.

**Interfaces:** Produces the tab menu item "Rename Tab…" (`PseudoTerminal.RenameTab`) opening the Edit Tab Title dialog for the clicked tab; `close_rules::batch_prompt(tabs: &[TabJobs]) -> Option<Prompt>` with titles "Close all other tabs?", "Close all tabs to the right of this one?", "Close all tabs below this one?" and bodies `PseudoTerminal.CloseBody.Tabs.OneTab` ("Tab #%1$d is running %2$@."), `.Jobs`, `.List` plus `.Bullet` lines ("Tab #%1$d: %2$@"), `.Locked`.

- [ ] **Step 1: Write the failing tests:** `the_tab_menu_has_rename_tab` (after "Edit Session…" per upstream order in `PseudoTerminal.m` at a81a54b7); `rename_tab_skips_a_closed_tab`; `close_others_asks_when_jobs_run` (Prompt Before Closing rules); `close_others_skips_tabs_that_closed`; `the_window_close_prompt_lists_every_panes_jobs`; `a_manual_tab_title_survives_a_label_refresh`; `move_tab_left_keeps_a_pinned_tab_out_of_groups`; `unpinning_a_middle_pinned_tab_keeps_the_bar_whole`; `duplicate_tab_keeps_variable_prompt_answers`; `background_animated_titles_keep_moving`; `unmaximize_after_window_resize_fills_the_window`; `a_tab_drag_stays_on_the_bar_until_well_away`; `pane_title_bars_render_html_titles` and `window_title_strips_tags`, `entities_decode_without_tags`; `the_window_keeps_focus_after_a_style_change`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tab_menu close_ rename_tab a_manual_tab move_tab unpinning duplicate_tab animated unmaximize tab_drag html_title entities window_keeps`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Tabs: Rename Tab, confirm batch closes, keep manual titles (iTerm2 f74abc16e 7ea65c742 6b64d5802 f18c79a8a 15e5a48b8)`.

### Task 21: Window > Lock Location

Ports 96b53e37c, cfa49e72c.

Works on Windows, macOS and Linux X11. Under Wayland a client cannot read or set its window position, so the menu item is disabled and its tooltip says "Lock Location is not available on Linux under Wayland yet: Wayland does not let a program place its windows."

**Files:** Create `src/lock_location.rs` (pure: anchor, dormancy, learn rules); modify `schema/menus.toml` (after "Lock Layout", line 2225: `path = ["Window", "Lock Location"]`, `selector = "toggleLocationLocked:"`, `identifier = "Lock Location"`), `src/app/windows_ui.rs`, `src/arrangement.rs` (saved with the window, not copied to a duplicate), `src/menu_tips.rs` (`MenuTip.LockLocation` text). Test: `src/lock_location.rs`.

**Interfaces:** Produces `LockLocation { display: DisplayId, display_size: (u32, u32), rel: Option<Rect> }` (`rel` None for maximized, centered and edge-attached styles: only the display is pinned), `fn on_displays_changed(&mut self)` (goes dormant at once), `fn on_frame(&mut self, frame: Rect, displays: &[Display]) -> Action` (`Action::Restore(Rect)`, `Learn`, `Ignore`), `fn on_user_drag(&mut self) -> Released`.

- [ ] **Step 1: Write the failing tests:** `a_moved_window_goes_back`; `a_resize_while_awake_is_learned`; `nothing_is_learned_while_dormant`; `the_window_returns_when_its_display_comes_back`; `a_maximized_window_pins_only_its_display`; `a_drag_releases_the_lock`; `a_duplicated_window_is_not_locked`; `lock_location_reports_wayland`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test lock_location`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller unplugs and replugs a monitor on EXTREME and the Mac.
- [ ] **Step 5: Commit** `Windows: Lock Location (iTerm2 96b53e37c cfa49e72c)`.

### Task 22: Show Clippings shortcut and the Set Character Encoding action

Ports 660c95167, 06569a03c, 460be17ce.

**Files:** Modify `schema/menus.toml:1209-1213` (`key = "G"`, `mods = ["shift", "option", "cmd"]`), `src/keyaction.rs:321-444` (action 78), the key binding editor, `src/encoding.rs`. Test: `src/keyaction.rs`, `src/encoding.rs`.

**Interfaces:** Produces action code 78 `Set Character Encoding…` (menu name `EditKeyAction.ActionSetCharacterEncoding`), parameter the encoding number as text; display `KeyBindingAction.SetCharacterEncoding` "Set Character Encoding to “%@”" or `.SetUnavailableCharacterEncoding`; on use, an in-session unobtrusive message `KeyBindingAction.SetCharacterEncodingToast` "Character encoding set to %@"; an unavailable encoding is ignored, kept on save, and shown as "Unavailable Encoding". Code 77 is reserved for Task 49 (`actions().len()` becomes 80 there).

- [ ] **Step 1: Write the failing tests:** `show_clippings_is_shift_alt_shortcut_g`; `make_text_bigger_owns_shift_equals`; `action_78_sets_the_encoding`; `an_unavailable_encoding_is_kept_and_ignored`; `actions_list_runs_to_78` (`actions().len() == 79`, 77 absent until Task 49).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test show_clippings make_text_bigger action_78 an_unavailable_encoding actions_list`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Menus: Show Clippings moves off Make Text Bigger; Set Character Encoding action (iTerm2 660c95167 06569a03c 460be17ce)`.

### Task 23: Focus Follows Mouse rework

Ports bc36feb8f, 1c9bb498f, c9b0cccd8, 8e98b39bb, c0b68a52c.

**Files:** Modify `src/app/mouse_ui.rs` (focus decisions), `schema/global.toml` (new row after "FocusFollowsMouse"), the Pointer settings page. Test: create the pure decider in `src/ffm.rs`.

**Interfaces:** Produces the global key `FFM Including Subviews` (bool, default false, label "…even to views like Search or Composer", dimmed when FocusFollowsMouse is off from the first show), `ffm::decide(prev: Target, now: Target, include_views: bool, dragging: bool, still: bool) -> Option<Target>`.

- [ ] **Step 1: Write the failing tests:** `moving_within_a_pane_changes_nothing`; `entering_a_pane_restores_its_last_focus`; `with_views_on_focus_follows_onto_the_find_field_and_back`; `no_switch_while_dragging`; `a_pane_pointed_at_during_a_sheet_gets_focus_when_it_closes`; `the_subviews_row_is_dimmed_when_ffm_is_off`; `the_find_caret_is_restored_not_selected`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test ffm`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Pointer: Focus Follows Mouse leaves fields in a pane alone (iTerm2 bc36feb8f 1c9bb498f c9b0cccd8)`.

### Task 24: Session Note behavior and API

Ports e96ad7215, 6ae082a6c, 09894ed6e, d226ee413, 5ed491d0c, 296452661, efdb4c58b.

**Files:** Modify `src/notes.rs`, `src/note_ui.rs`, `src/open_quickly.rs`, `src/builtin_fns.rs` (Session Note functions as upstream `iTermBuiltInFunctions` names them at a81a54b7), `src/api_proto.rs:12` ("1.20"). Test: `src/notes.rs`, `src/builtin_fns.rs`.

- [ ] **Step 1: Write the failing tests:** `escape_collapses_and_returns_to_the_terminal`; `shift_return_inserts_a_newline`; `the_note_stays_above_clippings`; `open_quickly_shows_the_note_and_keeps_it`; `hiding_a_focused_note_returns_focus`; `a_floating_note_reappears_after_restore`; `note_api_reads_and_writes_the_note` (each function upstream added, with its argument names and errors).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test notes note_api`. Expected: FAIL.
- [ ] **Step 3: Implement.** Set `PROTOCOL_VERSION = "1.20"`.
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Session Note: Escape collapses, API support, protocol 1.20 (iTerm2 e96ad7215 5ed491d0c efdb4c58b)`.

## API and it2

### Task 25: API RPC changes

Ports 2c11be0df, 57695ac60, fe8cbff71, bfc4d0ec7, f1cd67a6b, 2f8c65b46, dc65dae7f, 2765edb79, 76bd5c9f8, ebda02119, 49c77281c (server-visible bugs), a65edebd7 (send_text tool).

**Files:** Regenerate `src/api_pb.rs` from upstream `proto/api.proto` at a81a54b7 with `tools/protogen`; modify `src/api_proto.rs`, `src/script_fn.rs:844` (context menu providers), `src/ctx_menu.rs`, `src/builtin_fns.rs` (`browser_eval_js(js:)`), `src/app/browser_scripts_app.rs`, `src/api_layout.rs`, `src/api_server.rs` (accept loop after EMFILE), `src/orch_tools.rs` (send_text). Test: those files.

**Interfaces:** Produces `iterm2.browser_eval_js(js:)` (page world, may await, JSON result, null for nothing; first use per origin asks, answer stored as `NoSyncBrowserJavaScriptAllowed_<origin>`; only http and https), on Linux and macOS it returns `Browser is not available on <OS> yet`; context menu registration fields `submenu` and `visible_when` (parsed without calls, evaluated synchronously at menu open; a non-parsing or calling expression is refused at registration); `api_proto::FEATURES: &[(&str, fn() -> bool)]`.

- [ ] **Step 1: Write the failing tests:** `advertised_version_has_every_feature` (table rows 1.20 Session Note, 1.21 floating panes, 1.22 eval js, 1.23 context submenus, 1.24 nested tab groups; the constant may not exceed the last contiguous true row); `eval_js_asks_once_per_origin`; `eval_js_refuses_after_a_timeout_answer`; `eval_js_reports_no_browser_on_unix`; `submenu_items_merge_by_title`; `visible_when_hides_unless_true`; `visible_when_refuses_calls`; `nsnull_results_are_json_null`; `an_unchanged_layout_leaves_the_tab_alone`; `update_layout_gives_each_pane_its_size`; `preferred_size_follows_layout_changes`; `a_layout_with_a_browser_pane_keeps_sizes`; `the_server_accepts_after_fd_exhaustion`; `selection_string_joins_windowed_lines`; `send_text_peels_one_trailing_newline_as_return`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test api_ script_fn ctx_menu eval_js send_text`. Expected: FAIL.
- [ ] **Step 3: Implement.** Keep `PROTOCOL_VERSION` at "1.20".
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `API: eval_javascript, context submenus, layout and server fixes (iTerm2 2c11be0df fe8cbff71 bfc4d0ec7 f1cd67a6b 76bd5c9f8)`.

### Task 26: it2: session list with --format, clippings verbs

Ports 9e65c39eb, c6b1e4dcf, c78a9bf79, c40dc5515, and `--tmux`/`--tmux-pane` on clippings (3.7.7beta2 notes). Termurion's `it2` has only `add-clipping` and `archive-clippings` (`src/it2_session.rs`); `session list` is added because the triage names `--format`.

**Files:** Modify `src/it2_session.rs`, `src/builtin_fns.rs` (`iterm2.interpolate(string:)` returns its argument), `src/clippings.rs` (unique IDs). Test: `src/it2_session.rs`.

**Interfaces:** Produces `Command::List { json: bool, format: Option<String> }` printing upstream's columns (from `it2cli/Sources/it2core/Commands/SessionCommand.swift` at a81a54b7) with plain, unescaped values; `--format` prints one line per session, or adds `"formatted"` with `--json`; a session whose evaluation fails gets an empty string and a note on stderr; a format failing everywhere is an error. `add-clipping` prints the new clipping's ID; `Command::Delete { session, id }` with errors `DeleteClipping.MissingArgument` "Missing required argument" and `DeleteClipping.NotFound` "No clipping with ID %@".

- [ ] **Step 1: Write the failing tests:** `list_prints_plain_values` (a tty `/dev/ttys001`, a name `a/b`); `format_prints_one_line_per_session`; `format_with_json_adds_formatted`; `a_failing_session_gets_an_empty_line`; `an_older_app_without_interpolate_fails_cleanly`; `add_clipping_prints_its_id`; `delete_clipping_removes_by_id`; `delete_clipping_errors_follow_iterm2`; `clippings_take_tmux_and_tmux_pane`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test it2_session`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `it2: session list with --format, delete-clipping (iTerm2 9e65c39eb c6b1e4dcf c40dc5515)`.

## AI, password manager, integrations

### Task 27: AI catalog, Grok, API key choice, base URLs

Ports 6b1f131bf, f365dd26a, 8b6cc382d, 601c0a260, 8e90f1c2c, 2793f4eb4, fd89b9e2d, ae970a680, 353fd51ce, 696a31946, 576e73c10, 79ad72a0b, ce9e53572, 524320058, c2bcc5d11, 6f7254113, cd6120e1c, 11ee1f1f6, bb83d5e15, afd3607ff, 3cb1bf023.

**Files:** Replace `resources/ai/ai-models.json` with upstream `OtherResources/ai-models.json` at a81a54b7 (catalog version 6); modify `src/ai_catalog.rs`, `src/ai_keys.rs:47`, `src/ai_model.rs`, `src/ai_models.rs`, `src/ai_discovery.rs` (Fetch Models), `src/settings_ai.rs`, `src/chat_toolbar.rs:82` (vendor titles), `src/ai_test_conn.rs`, `src/ai_wire_log.rs`. Test: those files.

**Interfaces:** Produces `Vendor::XAI` (title "Grok", key account "xAI API Key for iTerm2", own hosts `api.x.ai` and its subdomains, a model name containing "grok" classifies as xAI, an `xai-` key is never OpenAI or DeepSeek; `reasoning_effort` sent for xAI when an effort is chosen); `ManualKey { Automatic, Vendor(Vendor), Own, None }` with labels `AIModelEditor.AutomaticVendorKey` "Automatic (%@ API key)", `.AutomaticNoKey`, `.AutomaticNoKeyLocal`, `.AutomaticNoKeyUnrecognizedHost`, `.VendorAPIKey`, `.ModelOwnAPIKey`, `.NoAPIKey` and hints `AIKeyHint.*`; `ai_keys::own_host(vendor, url) -> bool` (Automatic sends a key only to its vendor's host; Azure hosts include `.azure.us`, `.azure.cn`, serverless); `endpoint_for(base: &Url, api: Api) -> Url` (adds `/chat/completions` or `/messages` to a base URL).

- [ ] **Step 1: Write the failing tests:** `the_catalog_is_version_6_and_lists_grok`; `grok_is_its_own_vendor`; `automatic_sends_a_key_only_to_its_own_host` (OpenRouter gets no OpenAI key; `x.openai.azure.us` gets it the Azure way); `an_explicit_vendor_key_goes_even_to_a_local_endpoint`; `a_preset_resets_headers_and_key_choice`; `the_economy_model_keeps_the_manual_models_auth`; `a_base_url_gets_the_api_endpoint`; `fetch_models_lists_openai_and_anthropic_compatible_servers` (errors `AIModelList.*`); `a_local_chat_never_falls_back_to_the_cloud`; `a_removed_manual_model_shows_an_error`; `custom_header_values_are_redacted_in_logs`; `test_connection_rejects_bad_headers_and_can_stop`; `network_errors_read_http_request_failed`; `opening_settings_reads_no_secret` (key presence comes from a cached flag).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test ai_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `AI: catalog 6 with Grok, per-model API key choice, base URLs (iTerm2 6b1f131bf f365dd26a 8b6cc382d 601c0a260 ae970a680 576e73c10 79ad72a0b)`.

### Task 28: Password Manager export and import, authentication choice

Ports f66776ac9, e81d5c2c4, 5e6d09a93, 9ab9b0e37, d3ba85077, 83107911e, 9179e15db, 253c19a32, ba0042135, 73e1d7d53, 5c7a35dd4, 82344fba2.

**Files:** Modify `Cargo.toml` (`aes-gcm = "0.10"`, `pbkdf2 = { version = "0.12", default-features = false, features = ["hmac"] }`, `hmac = "0.12"`, `unicode-normalization = "0.1"` unless already present), `src/sigarchive.rs` (encryption), `src/pwm.rs`, `src/pwm_ui.rs`, `src/pwm_run.rs` (adapters run with the user's shell PATH), `src/pwm_bitwarden.rs`, `src/settings_export.rs` (optional part of Export All Settings and Data). Test: `src/sigarchive.rs`, `src/pwm.rs`.

**Interfaces:** Produces `sigarchive::encrypt(payload, passphrase, iterations: u32) -> Vec<u8>` and `decrypt(..)` with metadata `encryption = "aes-256-gcm"`, `kdf = "pbkdf2-sha256"`, key 32 bytes, salt 16, overhead 12 + 16, default iterations 600000, minimum 100000, maximum 10000000, the passphrase NFC-normalized, header and metadata as AAD (byte-compatible with upstream `SIGArchiveEncryption.m` so files move between iTerm2 and Termurion); file extension `.iterm2passwords`; import asks per conflict with Always Replace / Never Replace, skips identical, reports partial progress. Authentication choice: one menu with never, once per launch, once per launch and after the screen locks, every time it opens (labels from `sources/PasswordManager/mul.lproj/iTermPasswordManager.xcstrings`).

- [ ] **Step 1: Write the failing tests:** `an_encrypted_archive_round_trips`; `decrypts_the_upstream_fixture` (fixture made with upstream `ModernTests/SIGArchiveEncryptionTests.swift` vectors, committed under `tests/fixtures/pwm/`); `too_few_or_too_many_iterations_are_refused`; `nfc_and_nfd_passphrases_match`; `import_asks_per_conflict_and_skips_identical`; `export_and_import_authenticate_first`; `every_open_mode_asks_each_open_and_hides_accounts`; `a_screen_lock_revokes_and_forgets_names`; `adapters_find_tools_on_the_shell_path`; `bitwarden_reports_the_cli_error_and_the_missing_folder`; `closing_the_manager_cancels_an_scp_password_request`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test sigarchive pwm`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Password Manager: export and import, one authentication choice (iTerm2 f66776ac9 e81d5c2c4 5c7a35dd4)`.

### Task 29: Claude Code integration and resuming agent conversations

Ports 7ade1ebf3, f8363613b, 7c0477c87, cf33c462d, 4ccf2bc25, bc248c3ab, bf655e047, afce7c434, 3b08436fa, 911c38938 (local half: `it2 handle-agent-hook` accepted as the hook command), the 3.7 note "The status hook now does nothing when it can’t reach iTerm2".

**Files:** Modify `src/claude_code.rs`, `src/app/claude_code_app.rs`, `src/cc_status.rs`, `src/bin/cc-status.rs`, `src/it2_session.rs` (verb `handle-agent-hook [--agent codex]` runs the same code as cc-status), `src/restore_state.rs`, `src/announce.rs` (resume banner), `schema/global.toml` (new row). Test: those files.

**Interfaces:** Produces the advanced setting `OfferToResumeAgentConversations` (default true, label "Offer to resume Claude Code and Codex conversations that a restart ended.", help the full upstream text, section "Session"); the banner and its "Resume All" button text from `3b08436fa`.

- [ ] **Step 1: Write the failing tests:** `a_symlinked_settings_json_is_edited_through_the_link`; `an_unparseable_settings_json_is_left_alone`; `home_tilde_and_wrapper_hooks_are_recognized`; `a_hook_that_runs_other_programs_is_kept`; `an_empty_settings_json_uninstalls_quietly`; `a_dangling_symlink_gets_mode_0644`; `the_offer_skips_claude_p_and_plugin_list`; `handle_agent_hook_matches_cc_status`; `the_hook_does_nothing_without_the_app`; `a_restart_offers_to_resume_the_conversation`; `the_resume_setting_off_offers_nothing`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test claude_code cc_status handle_agent_hook resume`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Claude Code: settings.json safety, handle-agent-hook, offer to resume conversations (iTerm2 7ade1ebf3 f8363613b afce7c434 3b08436fa)`.

### Task 30: Browser fixes and Web > Show Toolbar (Windows)

Ports 3fdf2ae81, e75f3617e, 90240d2c6, 908e98a3b, 14b02e228, 98040bc3f, cead3c225, a92f32393, f57dd7e2b, 4d5cac550, 5794785e4, 225c6e2d7, 3a184589a.

Windows only; on Linux and macOS each entry point reports `Browser is not available on <OS> yet` (no embedded browser engine on Unix).

**Files:** Modify `schema/menus.toml` (`path = ["Web", "Show Toolbar"]`, `selector = "browserToggleToolbar:"`, after a separator as upstream), `src/browser.rs`, `src/webhost_win.rs`, `src/view_source.rs`, `resources/browser/error-page.html` and its template code, `src/browser_reader.rs`, `src/builtin_fns.rs` (hide-toolbar API function as `3fdf2ae81` names it). Test: those files.

- [ ] **Step 1: Write the failing tests:** `show_toolbar_toggles_and_is_saved`; `show_toolbar_reports_no_browser` (Unix); `view_source_shows_the_page_source_with_attribute_values_highlighted`; `the_error_page_escapes_the_url` (a URL with `"><script>` shows as text); `placeholders_substitute_in_one_pass`; `a_popup_tabs_context_menu_acts_on_that_tab`; `reader_mode_and_remove_distractions_run`; `keepassxc_remembers_basic_auth`; `the_ai_name_title_is_hidden_for_browser_profiles`; `find_on_page_with_porthole_results`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- browser view_source error_page` and `scripts/check-unix --linux-only --test show_toolbar_reports`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Browser: Show Toolbar, View Source, escaped error page (iTerm2 3fdf2ae81 e75f3617e 908e98a3b)`.

### Task 31: Triggers and Dynamic Profiles

Ports c788da7f3, e170f0674, b7821dea8, bf8b833e5, 0c7dbe4b3, 3a97d1ca0, 1e66fea95, d6e2da153, 75e03729b, 464374ce7, e1f5e2f3d, df72ea406, the 3.7.8 note "A Variable Changed trigger on a tab variable now also runs for a new split pane".

**Files:** Modify `src/triggers.rs`, `src/trigger_eval.rs`, `src/trigger_events.rs`, `src/trigger_file.rs`, `src/smartsel.rs`, `src/dynamic_profiles.rs`, `src/session_env.rs`. Test: those files.

- [ ] **Step 1: Write the failing tests:** `a_trigger_without_a_regex_loads`; `a_smart_selection_rule_without_a_regex_is_skipped`; `malformed_untrusted_parameters_are_ignored` (a list where text is expected; a number where text is expected is used as text); `hand_written_triggers_keep_state_across_reload`; `make_hyperlink_default_is_a_working_url`; `a_match_interpolation_does_not_stall_output`; `event_triggers_wait_for_setup_and_stop_at_termination`; `set_profile_setting_trigger_fires`; `variable_changed_on_a_tab_runs_for_a_new_split`; `dynamic_profile_values_of_the_wrong_type_are_dropped`; `the_error_alert_counts_each_bad_file_once`; `environment_entries_split_on_the_first_equals`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test trigger smartsel dynamic_profile environment_entries`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Triggers and Dynamic Profiles: untrusted values, missing regexes, event timing (iTerm2 c788da7f3 e170f0674 bf8b833e5 1e66fea95 75e03729b)`.

### Task 32: Session lifecycle and restoration

Ports 0f4c69c52, 065289754, 9db0ed4b7, e33fa985a, 2e23316c3, 289681483, fa73c179a, 9c83fba6f, 673520779, 72c85bfd1, 800dcf986, 4acdd013c, 0ab4c83f7, d2d73e836, 8f3ecc772, 7593ab037.

**Files:** Modify `src/sleep_guard.rs`, `src/app/sleep_ui.rs`, `src/restore_state.rs`, `src/app/restore_ui.rs`, `src/session_end.rs`, `src/start_dir.rs`, `src/osc.rs` (OSC 7 locality), `src/archive.rs:24-99`, `src/undo_close.rs`, `src/session_profile.rs`, `src/screen.rs` (color-only reload), `src/util_files.rs` (it2dl via `env bash`). Test: those files.

**Interfaces:** Produces the default archive folder: Windows `%APPDATA%\Termurion\Archives`, macOS `~/Library/Application Support/Termurion/Archives`, Linux `$XDG_DATA_HOME/termurion/Archives`, filled in when "Archive Sessions on Closure" is turned on with an empty folder and used by profiles already on with none.

- [ ] **Step 1: Write the failing tests:** `prevent_sleep_takes_effect_on_click_and_after_restart_session`; `a_cleared_screen_is_not_brought_back_on_restore`; `a_running_programs_scroll_region_is_restored`; `output_starts_right_after_the_restored_banner`; `short_output_is_not_hidden_by_the_restart_offer`; `new_sessions_use_the_directory_after_a_typed_ahead_cd`; `a_nested_shells_directory_wins`; `a_verified_osc7_is_not_overridden_by_a_stale_report`; `an_empty_archive_folder_gets_the_default`; `archive_names_replace_slashes`; `undo_close_restores_from_archives_after_the_timeout`; `changing_profile_type_keeps_names`; `an_escape_color_change_reloads_only_colors_and_keeps_copy_mode`; `first_output_is_never_lost`; `it2dl_finds_bash_through_env`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test sleep restore archive undo_close start_dir osc7 profile_type color_change it2dl`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Sessions: Prevent Sleep, restoration, directories after cd, archive defaults (iTerm2 065289754 9db0ed4b7 9c83fba6f 72c85bfd1 4acdd013c d2d73e836)`.

### Task 33: AI chat, Cockpit and Codecierge fixes

Ports 01cfccd29, 75dbc2d9c, c79ba5cf2, 850d060c0, 465e43122, 2096b0066, 459169fac, 66f8ce4ca, 699d4c2d2, 7fc0a31fb, 1e4d2ca5c, 172b87ee6, 25c6932e7, eaa8dc072, 996418e66, 390aed4ed, 6b77dd670, 94a74f336, f96c57449, 2821a9cb0, 2df9602e0, 3c44141a5, e62653351, bf9ebba38, 07f4b9dce, 01094a56e, eab34f50b, f3021bb83, 759b8500b, 08b465561, 25dea523e, the 3.7.8beta1 note "If a terminal session ends while AI chat is waiting on a command in it, the chat now continues".

**Files:** Modify `src/ai_chat.rs`, `src/chat_engine.rs` (`src/app/chat_engine.rs`), `src/chat_store.rs`, `src/orch_watch.rs`, `src/chat_message.rs` (code fences), `src/composer_suggest.rs`, `src/cockpit.rs`, `src/cockpit_ui.rs`, `src/codecierge.rs`, `src/ai_gate.rs` (lockout). Test: those files.

- [ ] **Step 1: Write the failing tests:** `stopping_one_reply_leaves_the_other_chat`; `a_deleted_message_is_not_sent_and_cancels_its_turn`; `links_watches_and_timers_survive_a_restoring_quit`; `unlinking_keeps_timers`; `stop_says_what_it_cancelled`; `each_tool_request_runs_once`; `fences_with_any_language_name_are_code`; `a_streamed_final_code_block_is_code`; `file_placeholders_expand_to_the_whole_name_once`; `ghost_text_for_files_is_escaped`; `an_unfinished_tool_call_is_described_accurately`; `a_dead_session_does_not_hang_the_chat`; `cockpit_opt_return_sends`; `cockpit_sends_only_the_selection`; `cockpit_skips_closed_targets_and_names_them` (`Cockpit.TargetsGoneStatus` "Nothing was sent. These targets no longer exist: %@"); `cockpit_picker_closes_after_at_space`; `engage_ai_is_disabled_in_the_cockpit`; `codecierge_stops_the_right_pane`; `the_ai_lockout_covers_timers_watches_and_engage`; `clicking_a_session_picks_the_mention`; `browser_mentions_show_a_globe`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test ai_chat chat_ cockpit codecierge composer_suggest`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `AI chat, Cockpit, Codecierge: cancellation, restoration, sending fixes (iTerm2 01cfccd29 850d060c0 94a74f336 eab34f50b)`.

### Task 34: Smaller interface fixes

Ports 11462c19a, 1314b06b3, the 3.7.7beta2 note on narrow announcements, f29420ae4, 4d907f27d, 52f95bb02, e29a787c3, 2f8072822, 709472649, 762644229, 8cdafc0e9, d661a610e, a705ad7ac, e4a99fc85, 81c766ce0, 8d5538e58, 3d339d8f7, c3560d2f8, 498eb0bf9, 49ad0c97e, 52a3e470a, 88c2b73eb, 2121dddb9, 1a10eb1e3, 023d97c2a, 6d9acd06d, the 3.7 note "A Select Menu Item key binding whose menu item was disabled ... now does nothing".

**Files:** Modify `src/suppressed_alerts_ui.rs`, `src/announce.rs`, `src/infobar.rs`, `src/url_schemes.rs` (x-man-page), `src/url_guess.rs`, `src/advanced_paste.rs`, `src/paste.rs`, `src/semantic.rs`, `src/kitty_dnd.rs`, `src/inline_file.rs`, `src/statusbar_sparkline.rs`, `src/settings_colors.rs`, `src/settings_ui.rs`, `src/hotkey.rs` and its indicator, `src/app/actions.rs:68`, `src/alerts.rs`. Test: those files.

**Interfaces:** Produces the pinned hotkey indicator popover `Indicator.PinnedHotkeyWindow` "This Hotkey Window is pinned. [Unpin](%@)" with a tokenized `iterm2:unpin-hotkey-window` link handled like the other indicator actions (`profileHotKeyForWindowController(nil)` matches nothing).

- [ ] **Step 1: Write the failing tests:** `suppressed_alerts_lists_announcements_and_unsuppresses`; `keys_typed_while_an_announcement_slides_away_are_kept`; `narrow_announcements_put_buttons_below`; `x_man_page_runs_no_command_and_renders_no_file` (`x-man-page://-P%20touch%20x` and `x-man-page:///etc/passwd` are refused); `search_the_web_encodes_plus`; `open_selection_as_url_takes_scheme_less_links_and_hides_otherwise`; `advanced_paste_counts_cr_lines_and_no_extra_trailing_line`; `the_tabs_prompt_focuses_the_spaces_field`; `editor_urls_encode_values_once`; `macvim_gets_an_escaped_file_url_and_textmate_the_raw_path`; `kitty_drops_into_yazi_and_survive_restore`; `the_json_promotion_is_used_only_when_shown`; `a_declined_download_is_not_drawn`; `sparklines_tick_once_a_second_in_low_power`; `the_theme_warning_names_unused_colors`; `a_popup_with_an_unknown_saved_value_shows_the_default`; `pickers_show_select_setting_until_chosen`; `the_pin_indicator_unpins`; `a_resized_centered_hotkey_window_keeps_its_size`; `toasts_and_popups_follow_the_theme`; `save_now_clears_the_unsaved_indicator`; `find_in_settings_focuses_its_field`; `alert_shortcuts_keep_editing_keys_and_skip_h_q_w_m`; `a_disabled_select_menu_item_fires_nothing`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test suppressed announce x_man_page search_the_web open_selection advanced_paste semantic kitty_dnd sparkline hotkey alert select_menu`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Interface: Suppressed Alerts lists announcements, x-man-page hardening, hotkey Unpin link and smaller fixes (iTerm2 11462c19a f29420ae4 c3560d2f8 e29a787c3)`.

### Release checkpoint 1 (controller)

- [ ] Full suites on Linux, macOS and Windows (`--test-threads=1`); only the known failures.
- [ ] Hand check per OS: Rename Tab, Close Other Tabs prompt, Lock Location, Bell sound, Show Clippings shortcut, OSC 7501 demo (`printf` cases from upstream `tests/`), tmux zoomed attach.
- [ ] Controller bumps the patch version, writes `docs/FIXES.md`, releases on all three OSes per the project's release rule.

---

# Phase 2: class B features (one group each, largest last)

### Task 35: Code review model picker

Ports fba72a29f, 1107096a2.

**Files:** Modify `src/code_review.rs`, `src/code_review_ui.rs`, `src/workgroup.rs` (record keeps model and git base), `resources/code-review-settings.txt` (upstream copy at a81a54b7). Test: `src/code_review.rs`.

**Interfaces:** Produces a toolbar combo with Fable, Opus, Sonnet, Haiku and free text; a star sets the default for new workgroups (global key as upstream names it); the next review uses the chosen model.

- [ ] **Step 1: Write the failing tests:** `the_picker_lists_upstreams_models`; `a_typed_model_is_used_next_review`; `the_star_sets_the_default`; `the_record_keeps_model_and_base`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test code_review`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Code Review: a model picker in the toolbar (iTerm2 fba72a29f 1107096a2)`.

### Task 36: imgcat Markdown rendering

Ports 415378116, bdf60aaf3, 4624671a6, af1e48041, 8a7240ece.

**Files:** Create `src/md_render.rs` (block layout with `pulldown-cmark`, already a dependency: reflowed paragraphs, bold headings, hanging lists to any depth, code background, blockquote bar, tables sized to fit, task-list checkboxes); modify `src/inline_file.rs`, `src/util_files.rs` (imgcat sends the absolute path as the name), `src/chat_message.rs` (AI chat uses the same block parser). Test: `src/md_render.rs`.

- [ ] **Step 1: Write the failing tests:** `paragraphs_reflow_to_the_width`; `lists_hang_and_nest`; `numbered_lists_resume_after_code`; `tables_fit_columns`; `relative_images_resolve_against_the_file`; `imgcat_sends_the_absolute_path`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test md_render imgcat`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes and a pixel case in `render_pixel_tests`. Expected: PASS.
- [ ] **Step 5: Commit** `imgcat: Markdown shown as a rendered document (iTerm2 415378116 bdf60aaf3 4624671a6)`.

### Task 37: Workgroup browser that follows a variable (Windows)

Ports 5a9dde9fc. Windows only; on Linux and macOS a workgroup browser member is created as today's stand-in and reports `Browser is not available on <OS> yet`.

**Files:** Modify `src/workgroup.rs`, `src/workgroup_edit.rs` (URL field accepts `\(user.browserURL)`), `src/workgroup_peers.rs`, `src/it2_session.rs` (`set-var --peer-name`). Test: `src/workgroup.rs`.

- [ ] **Step 1: Write the failing tests:** `a_browser_url_with_a_variable_follows_it`; `set_var_peer_name_reaches_the_peer`; `an_unset_variable_loads_nothing`; `unix_reports_no_browser`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test workgroup` and `scripts/xt test --lib -- workgroup`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Workgroups: a browser member follows a variable (iTerm2 5a9dde9fc)`.

### Task 38: nushell shell integration

Ports b134abde9, 9af57e38c, 32b4a67c7, 49f1daef4, 0f2ac83f3, 2f4270d3d.

**Files:** Add `resources/ssh/shell_integration/iterm2_shell_integration.nu` (upstream at a81a54b7); modify `src/ssh_inject.rs:22`, `src/shell_wizard.rs` (nu is a known shell; online install falls back to the Internet-Free installer when the website rejects nushell), `src/session_state.rs:89`, `src/screen.rs` (hold the screen during a prompt repaint), `schema/global.toml` (new row). Test: those files.

**Interfaces:** Produces the advanced setting `HoldScreenDuringPromptRepaint` (default true, label "Keep showing the screen as it was while the shell redraws its prompt?", help the full text with "iTerm2" as Termurion, section "Drawing").

- [ ] **Step 1: Write the failing tests:** `nu_is_a_known_shell_and_gets_its_script`; `the_embedded_nu_script_matches`; `a_prompt_repaint_does_not_blink` (frames between OSC 133;A and the end of the repaint show the old screen); `the_hold_setting_off_draws_every_frame`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test nu_ prompt_repaint hold_setting`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller runs nu on Linux and Windows.
- [ ] **Step 5: Commit** `Shell integration: nushell (iTerm2 b134abde9 49f1daef4)`.

### Task 39: Session-count variables

Ports cd249d48f (variables).

**Files:** Modify `src/api_vars.rs` (app scope), `src/interp.rs`. Test: `src/api_vars.rs`.

**Interfaces:** Produces `iterm2.workingSessionCount` and `iterm2.waitingSessionCount` (sessions whose status text reads "working" or "waiting", case-insensitive as the priority matcher), updated on every status change.

- [ ] **Step 1: Write the failing test** `session_counts_follow_statuses`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test session_counts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Variables: iterm2.workingSessionCount and waitingSessionCount (iTerm2 cd249d48f)`.

### Task 40: Menu bar item

Ports cd249d48f, 9bbe6a975, 372f593e2. macOS: the status item (`src/platform/macos/status_item.rs`) shows the icon and the text. Windows: the notification area icon shows the text as its tooltip, and with `MenuBarItemDrawsBadge` the badge inside the icon (a notification area icon cannot show text beside it). Linux: the StatusNotifierItem (`src/platform/linux/tray.rs`) sets its title and `XAyatanaLabel`; panels that ignore the label show the tooltip.

**Files:** Create `src/menu_bar_item.rs` (evaluate, badge text); modify `src/resident.rs` (`icon_wanted` also when `ShowMenuBarItem`), the three tray backends, `schema/global.toml` (rows in Appearance > General), `src/statusbar.rs` if the error item lives in the menu. Test: `src/menu_bar_item.rs`.

**Interfaces:** Produces global keys `ShowMenuBarItem` (bool, default false, label "Show menu bar icon", tooltip "Shows an iTerm2 icon in the menu bar, followed by the text below. It is always shown while iTerm2 is excluded from the Dock so you can get back to Settings." with Termurion and "taskbar" on Windows only where the existing resident text does so) and `MenuBarItemString` (string, default "", placeholder "Interpolated string to show in menu bar (global scope)", row label "Menu bar icon:"); the advanced setting `MenuBarItemDrawsBadge` (default false, upstream label and help); `menu_bar_item::badge(text: &str) -> Option<String>` (one character, or a number with "+" for 10 or more; empty or "0" is the plain icon); errors show the bug icon and the menu item "Show Menu Bar Item Error…" opening heading "Menu Bar Item Error" and message "The menu bar item’s interpolated string “%1$@” could not be evaluated:" plus the error.

- [ ] **Step 1: Write the failing tests:** `the_rows_follow_iterm2`; `badge_rules_follow_iterm2` (`"3"`, `"12"` to `"9+"` per upstream code, `""`, `"0"`); `an_evaluation_error_shows_the_bug_and_the_menu_item`; `the_icon_shows_while_excluded_from_the_taskbar_or_dock` (372f593e2: the checkbox is not stuck when the exclusion needs hotkey windows only); `each_os_backend_gets_text_or_tooltip`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test menu_bar_item`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller checks the icon on each.
- [ ] **Step 5: Commit** `Menu bar item with an interpolated string (iTerm2 cd249d48f 9bbe6a975 372f593e2)`.

### Task 41: Quick Look on hover (all OSes)

Ports d3ef1b0aa (hover half), 1984bf58d (strings).

**Files:** Create `src/quick_look_hover.rs` (allowlist normalization, private-address rule, hover and auto-close timers); modify `src/web_popover.rs`, `src/pointer.rs`, `schema/global.toml`, the Pointer settings page ("Preview links on hover:" with Configure…). Test: `src/quick_look_hover.rs`.

**Interfaces:** Produces global keys `QuickLookHoverMode` (int: 0 off, 1 hover, 2 Shortcut key+hover; default 0), `QuickLookHoverDelay` (0.75), `QuickLookHoverAutoCloseDelay` (1.5), `QuickLookHoverDomainAllowlist` (array, default empty; a saved string is split on commas and whitespace), `QuickLookHoverIncludesPlainTextURLs` (false); the secure allow-all switch (needs elevation as upstream; on Windows and Linux the existing admin prompt path, `src/platform/user_auth.rs`); Configure sheet strings `QuickLookSettings.*`; offer `WebQuickLook.OfferToAllowDomain` with "Allow %@" and "Don’t Ask Again". The basic preview is the existing popover on every OS.

- [ ] **Step 1: Write the failing tests:** `entries_normalize_and_match_subdomains`; `private_addresses_need_their_own_entry`; `hover_opens_after_the_delay_and_closes_after_leaving`; `plain_urls_need_the_setting`; `an_unlisted_domain_offers_to_allow`; `hover_previews_never_send_cookies` (the popover runs with a fresh profile).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test quick_look`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Quick Look: previews on hover for allowed domains (iTerm2 d3ef1b0aa)`.

### Task 42: Quick Look on the browser engine (Windows)

Ports d3ef1b0aa (engine half). Linux and macOS: the checkbox is disabled with `Browser is not available on <OS> yet`, and previews use the basic popover.

**Files:** Modify `src/web_popover.rs`, `src/platform/windows/web_popover.rs`, `schema/global.toml`. Test: `src/web_popover.rs`.

**Interfaces:** Produces `QuickLookUsesBrowserEngine` (bool, default false, label "Use the iTerm2 browser engine for web previews" with Termurion, tooltip `QuickLookSettings.UseBrowserEngineToolTip`); buttons `BrowserQuickLook.OpenInTab`, `.OpenInWindow`, `.OpenInITermBrowser`, `.OpenInDefaultBrowser`, `.CloseAccessibility`; fallbacks `PointerPreferences.QuickLookBrowserProfilesOff` and `.QuickLookPluginNotInstalled` adapted to WebView2 missing.

- [ ] **Step 1: Write the failing tests:** `engine_previews_share_sign_ins_except_on_hover`; `the_engine_row_reports_no_browser_on_unix`; `open_buttons_follow_iterm2`.
- [ ] **Step 2: Run** `scripts/xt test --lib -- web_popover` and `scripts/check-unix --linux-only --test the_engine_row`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Quick Look: previews on the browser engine (iTerm2 d3ef1b0aa)`.

### Task 43: Control-Tab switcher

Ports 51aecbf67, 1d9f3c159.

**Files:** Create `src/tab_switcher.rs` (pure model: MRU order, last-seen text) and `src/app/tab_switcher_ui.rs` (egui overlay with a preview from the scene cache); modify `src/mru.rs`, `src/app/mod.rs` (modifier release ends the cycle), `schema/global.toml` (Appearance > Tabs). Test: `src/tab_switcher.rs`.

**Interfaces:** Produces `ShowTabSwitcherWhenCyclingTabs` (bool, default false, row label "Control-Tab Switcher:", checkbox "Show a list of tabs with a preview while cycling through tabs", tooltip from `peF-rz-Vml`); rows show icon, status, `TabSwitcher.LastSeenAgo` "Last seen %@ ago" / `.LastSeenJustNow` "Last seen just now", `.TmuxClient` "tmux window", `.TmuxGateway` "tmux gateway".

- [ ] **Step 1: Write the failing tests:** `the_switcher_orders_most_recent_first`; `release_switches_to_the_highlight`; `escape_cancels`; `switcher_drops_a_closed_tab`; `only_bindings_with_a_modifier_show_it`; `last_seen_text_follows_iterm2`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tab_switcher`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Tabs: the Control-Tab switcher (iTerm2 51aecbf67 1d9f3c159)`.

### Task 44: Output-fairness limits

Ports 574e1ab87, 1fb625788, 0c99c5735, 2e46a53be, 49fe45f9d, 261df3b32, a73bf4f09, 10fe6d221, 99815ce15, 1d372130e, 9a37dd4b9.

Termurion already reads each pty on its own thread with a byte budget (`src/pty_budget.rs`); this task adds the limit on how many sessions parse at once, with visible sessions weighted.

**Files:** Create `src/exec_permits.rs` (permit line, time slices, weights, lost-wakeup recovery); modify `src/host.rs` / the parse loop where chunks are taken, `schema/global.toml` (General > Magic row and three Experimental advanced rows). Test: `src/exec_permits.rs`.

**Interfaces:** Produces `MaximumConcurrentTokenExecutions` (int, default 0 meaning Automatic; label "Sessions processing output at once:", an "Automatic" checkbox stored as the synthetic `MaximumConcurrentTokenExecutionsAutomatic`, Automatic being the number of performance cores as upstream computes); advanced `LimitSessionsProcessingOutput` (true), `VisibleSessionExecutionWeight` (4), `TokenExecutionTimeSlice` (0.005), labels and help from upstream with the sentence "This applies only with the new implementation of session input and output." removed (Termurion has only one implementation); `Permits::acquire(&self, session: Id, visible: bool) -> Permit`, slice checked every 8 tokens, a holder runs without the lock when nobody waits.

- [ ] **Step 1: Write the failing tests:** `at_most_n_sessions_hold_permits`; `visible_sessions_get_four_times_the_slices`; `a_holder_with_nobody_waiting_keeps_running_unbilled`; `a_lost_wakeup_recovers`; `a_session_freed_while_waiting_gives_up_its_place`; `the_limit_off_grants_everyone`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test exec_permits`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Controller runs `scripts/draw-bench` flood rows and a keystroke echo test with 8 flooding sessions; expected typing latency no worse than 1.1.11.
- [ ] **Step 5: Commit** `Output: limit how many sessions process output at once (iTerm2 574e1ab87 49fe45f9d 0c99c5735)`.

### Task 45: Nested tab groups, model and drawing

Ports 744e524d4, b3010f0d4, 5460ca25c, 89ce90f28, d0fb4101b, aa0aa329d.

**Files:** Modify `src/tab_groups.rs`, `src/tab_layout.rs`, `src/tab_theme.rs` (Minimal theme capsule, strip and count badge), `src/arrangement.rs` (repair deeper nesting on load), `src/api_layout.rs` (fields `tab_group_parent_id`, `_name`, `_color`, `_collapsed`). Test: `src/tab_groups.rs`.

**Interfaces:** Produces `Group { parent: Option<GroupId>, .. }` (one level deep: a group whose parent has a parent is repaired by moving it up, keeping the parent alive); each level collapses on its own.

- [ ] **Step 1: Write the failing tests:** `a_group_nests_one_level`; `deeper_nesting_is_repaired_on_load`; `the_parent_survives_repair`; `each_level_collapses_alone`; `api_reports_the_parent_fields`; `minimal_chips_show_a_capsule_strip_and_count`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tab_groups`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Tab groups: nested one level deep (iTerm2 744e524d4 b3010f0d4 5460ca25c)`.

### Task 46: Nested tab groups, menus and drags

Ports 744e524d4, b3010f0d4 (UI).

**Files:** Modify `schema/menus.toml` (`["Window", "Tab", "Move Tab to New Sub-group…"]`, `identifier = "Move Tab to New Sub-group"`, `selector = "addTabToNewSubgroupAction:"`, after "Add Tab to New Group…"), `src/tab_menu.rs` (group menu: "Move Group Into", "Move Group Out of Parent Group", "New Tab Sub-group"), `src/tab_drag.rs` (drop a group into another). Test: `src/tab_menu.rs`, `src/tab_drag.rs`.

- [ ] **Step 1: Write the failing tests:** `the_menu_has_move_tab_to_new_subgroup`; `group_menu_moves_in_and_out`; `dragging_a_group_into_a_group_nests_it`; `a_nested_group_cannot_take_a_group`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test tab_menu tab_drag`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Tab groups: sub-group menus and drags (iTerm2 744e524d4 b3010f0d4)`.

### Task 47: Floating panes, model and layout

Ports c77dbd257 (core), e083cab0a, 27a7ea1fb, 6f2d987b3, f354788c5. Design reference: upstream `docs/floating-panes-design.md`.

**Files:** Create `src/floating.rs` (per-tab float list, z-order, frames in cells, maximize, hidden flag); modify `src/layout.rs`, `src/pane.rs`, `src/scene.rs` (draw over splits, title bar, edges), `src/undo_close.rs`, `src/arrangement.rs`. Test: `src/floating.rs`.

**Interfaces:** Produces `Floats { list: Vec<Float>, hidden: bool }`, `Float { pane: PaneId, frame: CellRect, maximized: Option<CellRect>, title_bar: bool }`, `Floats::bring_to_front`, `send_to_back`, `select_next`, `dock(&mut self, pane, target: PaneId, side: Side)`, `move_by(dx, dy)`; errors `FloatingPane.*` texts.

- [ ] **Step 1: Write the failing tests:** `a_float_draws_over_splits`; `z_order_moves`; `maximize_and_restore_keep_the_frame`; `undo_close_restores_place_and_maximize`; `hidden_floats_do_not_count_toward_the_gpu_pane_limit` (if Termurion has such a limit; else the test asserts they are skipped in hit testing); `arrangements_save_and_restore_floats`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test floating`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Floating panes: model, drawing, undo and arrangements (iTerm2 c77dbd257 27a7ea1fb 6f2d987b3)`.

### Task 48: Floating panes, mouse and docking

Ports c77dbd257 (interaction), 565d5f52b, f74767055, ace14a181.

**Files:** Modify `src/app/pane_move_ui.rs`, `src/app/mouse_ui.rs`, `src/pane_move.rs`. Test: `src/pane_move.rs`.

- [ ] **Step 1: Write the failing tests:** `dragging_the_title_bar_moves`; `dragging_an_edge_resizes`; `control_drag_docks_and_keeps_focus`; `control_drag_to_another_tab_moves_it`; `swap_highlights_only_the_float`; `a_workgroup_member_never_becomes_a_float`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test pane_move floating`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Floating panes: move, resize and dock with the mouse (iTerm2 c77dbd257 565d5f52b f74767055)`.

### Task 49: Floating panes, menus, key action 77, pointer action

Ports c77dbd257, ce2920d7f, e8b2e91b6.

**Files:** Modify `schema/menus.toml` ("New Floating Pane with Current Profile" in Shell with `key = "n"`, `mods = ["option", "cmd"]`; Session > Move Session > "Move Session to Floating Pane"; Window > Split Pane > "Floating Panes" submenu: Bring to Front, Send to Back, Select Next Floating Pane, Dock Floating Pane, Hide Floating Panes, separator, Move Floating Pane Up/Down/Left/Right, identifiers as upstream `Floating Panes.<title>`), `src/keyaction.rs` (77 "New Floating Pane with Profile", display `KeyBindingAction.NewFloatingPaneWithProfile`), `src/pointer.rs` (`PointerPrefs.ActionNewFloatingPaneWithProfile`), `src/ctx_menu.rs` (Bring to Front, Send to Back, Dock Floating Pane, Move Session to Floating Pane, Open URL in Floating Pane), the profiles window button "New Floating Pane", `schema/global.toml` (`NewInstanceOpenStyle` help gains "4: Floating Pane"), `src/open_file.rs:53`. Test: `src/keyaction.rs`, `src/app/menu.rs` tests.

- [ ] **Step 1: Write the failing tests:** `floating_menus_follow_iterm2` (titles, order, shortcut); `action_77_opens_a_float`; `actions_list_runs_to_78_with_77` (`actions().len() == 80`); `new_instance_style_4_opens_a_float`; `move_session_to_floating_pane_refuses_a_float_target` (`MoveSession.DestinationIsFloating`).
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test floating_menus action_77 actions_list new_instance move_session`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Floating panes: menus, key binding and pointer actions (iTerm2 c77dbd257 ce2920d7f)`.

### Task 50: Floating panes, tmux and API

Ports eb7f13278, 4da67fc7b, ca1603f8a, 9dc7d14af, ecb8e27fd, 5496b9a85, the protocol 1.21 floating pane RPCs.

**Files:** Modify `src/tmux_controller.rs`, `src/tmux_layout.rs` (floats kept in tmux as upstream stores them), `src/api_layout.rs` (`FloatingPane`, `floating_panes`, `floating_panes_hidden`; `ApplyLayout.FloatingSession` error), `src/api_proto.rs` (`"1.23"`). Test: those files.

- [ ] **Step 1: Write the failing tests:** `a_native_session_never_floats_in_a_tmux_tab`; `floats_keep_their_place_on_attach_past_column_80`; `resizing_a_tmux_float_asks_tmux`; `a_float_over_pane_0_stays_put_with_a_filter`; `api_lists_floats_and_hidden_state`; `advertised_version_has_every_feature` now passes at "1.23".
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test floating tmux_ api_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `Floating panes: tmux and Python API, protocol 1.23 (iTerm2 eb7f13278 4da67fc7b ca1603f8a)`.

### Task 51: Protocol 1.24

- [ ] **Step 1:** `advertised_version_has_every_feature` table row 1.24 is true (Task 45 fields present). Set `PROTOCOL_VERSION = "1.24"`.
- [ ] **Step 2: Run** `scripts/check-unix --test advertised_version` on Linux and macOS, `scripts/xt test --lib -- advertised_version`. Expected: PASS.
- [ ] **Step 3: Commit** `API: protocol 1.24`.

### Task 52: SSH Integration, framer and flow control

Ports ea7819a7f, 231fd4d9d, ad80b196b, 959856a7d, 9b82aed36, a1adaf6e3, 4a1356ee2, 61f9d8b20, 475f1e2b8, 2959777d4, 156992306, 89775dd2e, 22fadab31, 00f4bebec, 8d7b16948.

**Files:** Replace `resources/ssh/framer.py` and `resources/ssh/conductor.sh` pieces with upstream `OtherResources/framer.py` at a81a54b7 (upstream grew 1359 lines); modify `src/conductor.rs`, `src/conductor_framer.rs`, `src/conductor_parser.rs`, `src/ssh_recovery.rs`, `schema/global.toml` (SSH section). Test: those files, plus `tests/fixtures/ssh` cassettes recorded with `scripts/record-conductor.py`.

**Interfaces:** Produces the credit protocol as upstream framer (the terminal grants credit; nested connections paced too), advanced `SshFlowControl` (default true, label "Pace output from remote hosts.", help upstream with iTerm2 as Termurion, "Takes effect for new connections."), long commands never split.

- [ ] **Step 1: Write the failing tests:** `the_framer_waits_for_credit`; `control_c_takes_effect_at_once_under_load`; `nested_connections_are_paced`; `long_commands_are_sent_whole`; `an_abrupt_death_returns_to_the_local_shell`; `an_abandoned_connections_output_is_drawn_not_acted_on`; `disconnecting_a_nested_connection_keeps_the_outer`; `flow_control_off_sends_unpaced`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test conductor ssh_recovery`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller connects to a Linux and a macOS host and floods `yes`.
- [ ] **Step 5: Commit** `SSH Integration: the new framer with flow control and pacing (iTerm2 ea7819a7f 231fd4d9d ad80b196b 9b82aed36)`.

### Task 53: SSH Integration, flow control monitor

Ports ad80b196b (monitor), 69d0b490a, 3452fc03b, 33187b479, 7a1367f54, 5c7113da4.

**Files:** Create `src/flow_monitor.rs` (model) and `src/app/flow_monitor_ui.rs` (egui window: credit outstanding, delays, rates, one-minute charts; redraws on change or once a second; closing turns the setting off; Shortcut key+W closes it); `schema/global.toml` (`ShowSSHFlowControlMonitor`, default false, upstream label and help). Test: `src/flow_monitor.rs`.

- [ ] **Step 1: Write the failing tests:** `the_monitor_lists_nested_connections`; `it_survives_a_restart`; `a_stopped_connection_is_not_never_tried`; `it_notices_events_after_the_list_is_full`; `closing_turns_the_setting_off`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test flow_monitor`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `SSH Integration: the flow control monitor (iTerm2 ad80b196b 69d0b490a 7a1367f54)`.

### Task 54: SSH Integration, it2 on remote hosts and host permissions

Ports 814de935d, 49b8bd0ef, e7bc90fa2, 555ef2efb, ea561184e, a2089a0b9, 5da164a2b, 8cc3a2378, 373df1dfe, c9e1ae77e, 936a28bb2, e9786b8bd, d09f489e9, 46acdb573, a1f6ce9d9, 7374bd37a, e4f7df4d9.

**Files:** Add `resources/ssh/it2.py` (upstream `OtherResources/it2.py`), `.zshenv` handling (upstream `OtherResources/.zshenv`); modify `src/ssh_inject.rs`, `src/host_state.rs`, `src/ssh_registry.rs`, `src/it2_session.rs` (route by tmux session id), `src/settings_ssh.rs` and the Magic "Manage Permissions…" sheet (a tab beside browser scripting permissions), `src/app/host_offers_ui.rs` ("Always Allow This Host"). Test: those files.

**Interfaces:** Produces `~/.iterm2/bin/it2` on the remote as a real command, first on PATH after a login zsh's startup files; a command naming no session acts on the session it runs in; host grants keyed by `(HostName, port)` so hosts sharing a HostName stay apart; turning off host control revokes the grant everywhere.

- [ ] **Step 1: Write the failing tests:** `remote_it2_is_on_path_first`; `remote_it2_without_a_session_acts_on_its_own`; `remote_it2_reads_stdin`; `tcsh_and_no_shell_integration_still_get_it2`; `always_allow_skips_later_prompts`; `hosts_sharing_a_hostname_differ_by_port`; `grants_from_before_include_and_match_are_kept`; `a_tmux_pane_this_connection_does_not_show_is_refused`; `the_socket_dir_is_removed_at_the_end`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test ssh_inject host_state ssh_registry it2_session`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller runs it2 from `sh -c` on a remote host.
- [ ] **Step 5: Commit** `SSH Integration: it2 on remote hosts, per-host permissions (iTerm2 814de935d 49b8bd0ef e7bc90fa2 46acdb573 a1f6ce9d9)`.

### Task 55: SSH Integration, remote git, jobs, Claude Code and workgroup peers

Ports b70785ffe, 32e6234f4, 39c9f5989, b7f28af96, b3052589d, ce9199f6b, a275a69b6, ec4558b23, d76040188, eb3f4cf90, 6d9acd06d (remote half), 4178cc858, de6d8f5ce.

**Files:** Add `resources/ssh/it2_git_state.py`; replace `resources/utilities/it2git` with upstream; modify `src/git_state.rs`, `src/ssh_procs.rs`, `src/claude_code.rs` (install on a remote host; uninstall skips this machine), `src/workgroup_run.rs` (peers start where the leader is). Test: those files.

- [ ] **Step 1: Write the failing tests:** `remote_git_state_runs_git_there`; `it2git_counts_like_local` (untracked counted, conflicts not, detached HEAD full id); `a_remote_job_is_the_terminals_foreground_job`; `claude_code_installs_on_a_remote_host`; `uninstall_never_edits_this_machine_through_a_loopback`; `an_unresponsive_host_fails_fast_on_uninstall`; `remote_leader_starts_peers_on_that_host`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test git_state ssh_procs claude_code workgroup_run`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `SSH Integration: remote git, jobs, Claude Code and workgroup peers (iTerm2 b70785ffe 32e6234f4 39c9f5989)`.

### Task 56: SSH Integration, transfers and ssh config

Ports 120de33b4, 690032eb8, 5c3a2150d, b1df11bf0, be774fda6, 610a5b4ad, 3610cd1f7, f637cb8e9, 54a65ad1c, e076d23f5, dd4d18f3b, eb5ef8e4b, 7d735855c, ed628f051, c3477e8ff, bb60d7218, d6d7e8709.

**Files:** Modify `src/ssh_transfer.rs`, `src/transfers.rs`, `src/ssh_files.rs`, `src/ssh_config.rs` (Include and Match; `Match exec` never applies). Test: those files.

**Interfaces:** Produces pipelined 4 KiB download requests; failure notifications naming the file and the reason (upstream texts from `be774fda6`, `3610cd1f7`, `f637cb8e9`); a canceled transfer reported as canceled.

- [ ] **Step 1: Write the failing tests:** `downloads_pipeline_4_kib_requests`; `same_named_uploads_get_distinct_names`; `an_upload_never_overwrites_when_the_check_fails`; `a_failed_download_leaves_no_hidden_file`; `a_folder_download_reports_left_out_files`; `failure_notifications_name_file_and_reason`; `cancel_is_not_failure`; `download_files_accepts_any_file`; `include_and_match_resolve_like_ssh`; `match_exec_never_applies`; `user_at_host_is_not_the_host_name`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test ssh_transfer transfers ssh_files ssh_config`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `SSH Integration: faster transfers with clear failures, ssh config Include and Match (iTerm2 b1df11bf0 690032eb8 d6d7e8709)`.

### Task 57: SSH Integration, it2ssh and restoration

Ports 7e8db7cd7, d1a58fed0, fa179d0ba, a18224fc9, 2f4270d3d, e6e13ee0b, 6fc99cb40, 21bc38e28, 44f5b04e0, 33a498421, 6a0e134fc (bundled it2ssh), 270ca41c4, e0f077e36, 4a4bd021a, 23090b7cd, caabf13d0, e3639138b, 2190c6531, f3031cc39, eb2f4eec4.

**Files:** Replace `resources/ssh/it2ssh` with upstream `OtherResources/it2ssh`; modify `src/util_it2ssh.rs`, `src/ssh_args.rs`, `src/ssh_recovery.rs`, `src/restore_state.rs`. Test: those files.

- [ ] **Step 1: Write the failing tests:** `it2ssh_exits_with_ssh_status`; `it2ssh_refuses_misplaced_no_shell_integration`; `a_shell_named_as_the_command_is_execed`; `non_ascii_arguments_pass_exactly`; `the_unreadable_arguments_banner_follows_iterm2`; `a_bare_control_c_answers_an_endssh_with_no_connection`; `remote_bash_is_a_login_shell`; `disconnect_works_before_the_framer_starts`; `restored_shells_get_no_framer_commands`; `a_restored_connection_whose_ssh_ended_is_not_recovered`; `the_bundled_it2ssh_wins_over_a_stale_copy`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test it2ssh ssh_args ssh_recovery restore`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `SSH Integration: it2ssh arguments, exit status and restoration (iTerm2 e6e13ee0b 21bc38e28 270ca41c4 23090b7cd)`.

### Task 58: tmux Workgroups, model

Ports 635cda4ed, 06ceda8db, 39c9f5989 (tmux half), 9b3abc01d.

**Files:** Modify `src/workgroup.rs`, `src/tmux_persist.rs` (workgroup kept on the tmux server as upstream options), `src/tmux_controller.rs`. Test: `src/workgroup.rs`, `src/tmux_persist.rs`.

**Interfaces:** Produces `TmuxWorkgroup` stored in tmux options so another computer attaching picks it up; refusal below tmux 3.2 with upstream's text from `06ceda8db`.

- [ ] **Step 1: Write the failing tests:** `a_workgroup_is_saved_on_the_tmux_server`; `attach_picks_it_up`; `older_than_3_2_is_refused_with_iterm2s_text`; `shell_integration_loads_inside_tmux`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test workgroup tmux_persist`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `tmux: workgroups kept on the tmux server (iTerm2 635cda4ed 06ceda8db)`.

### Task 59: tmux Workgroups, peers, detach and close

Ports 522847437, c450cf1c5, d0c4c39b6, 87e3ae336, e90afae1a, 479dbc2c4, dd2ef1a38, 8144be2b4, 08bc2bec9, d3b985c6b, d2ac604be, 0ab0b9be1, 11fdf081a, bc26e447e, 243d5f281, 963670d99, 240381b91, 5b59eb32a, b58fab8cf, 33fb90776, 937ce398c, c151bb32d, d22f0c651, 908eb6b51, 9da747c45.

**Files:** Modify `src/workgroup_peers.rs`, `src/app/workgroup_app.rs`, `src/app/workgroup_toolbar_ui.rs`, `src/tmux_windows.rs`, `src/tmux_dashboard.rs`, `src/close_prompt.rs`. Test: those files.

- [ ] **Step 1: Write the failing tests:** `switching_peers_keeps_focus_in_the_slot`; `detaching_keeps_the_peer_and_leader_in_view`; `vars_set_before_the_peer_exists_apply`; `a_parked_filtered_member_is_buried`; `parked_windows_stay_out_of_the_dashboard`; `a_late_leader_window_adopts_the_workgroup`; `closing_a_member_asks_and_names_it`; `ending_leaves_peers_to_tmux`; `filter_stand_ins_fit_their_view`; `closing_a_filtered_pane_kills_it`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test workgroup tmux_`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes; controller runs a Claude Code workgroup inside tmux on Linux, detaches and reattaches from Windows.
- [ ] **Step 5: Commit** `tmux: workgroup peers across switches, detach and close (iTerm2 522847437 c450cf1c5 8144be2b4 479dbc2c4)`.

### Task 60: tmux Workgroups, Jobs and directories

Ports 5e01d4fae, 951bb2f5c, e9786b8bd (tmux half), de6d8f5ce, 4178cc858, the 3.7.8beta2 note "panes a Claude Code workgroup opened outside of tmux ... now start in your home directory".

**Files:** Modify `src/jobwatch.rs`, `src/app/jobs_ui.rs`, `src/workgroup_run.rs`, `src/start_dir.rs`. Test: those files.

- [ ] **Step 1: Write the failing tests:** `jobs_shows_a_local_tmux_servers_pane_processes`; `jobs_follows_the_root_pid_and_provider`; `a_remote_tmux_leaders_outside_peers_start_at_home`; `peers_start_where_the_leader_kind_says`.
- [ ] **Step 2: Run** `scripts/check-unix --linux-only --test jobwatch jobs_ workgroup_run start_dir`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** on all three OSes. Expected: PASS.
- [ ] **Step 5: Commit** `tmux: workgroup jobs and start directories (iTerm2 5e01d4fae 951bb2f5c 4178cc858)`.

### Release checkpoint 2 (controller)

- [ ] Full suites on all three OSes; only the known failures.
- [ ] Hand checks per OS: floating pane create, drag, dock, undo; SSH flood with Control-C; it2 on a remote host; a tmux workgroup survives detach; Control-Tab switcher; menu bar item.
- [ ] Controller bumps the patch version, writes `docs/FIXES.md`, releases on all three OSes.

---

## Not ported (judged not applicable)

Recorded here so the next upstream sweep does not re-triage them.

- iTerm2 Buddy and the Companion relay (about 40 commits): Termurion has no phone app or relay.
- Sparkle 2, the backup update notice, resumable update downloads, "Install on Quit" and `keepJobsRunningWhenQuitInstallsUpdate`: Termurion's updater never installs on quit and has no Sparkle.
- `engageLayoutEngineInTerminalWindows`, Auto Layout fixes, AppKit tab title indexing, `NSAutomatic*` text substitutions and the double-space period: AppKit only; Termurion's windows are winit and egui.
- `useTaskIOMonitors` and the per-session mutation queues: Termurion already parses each session on its own thread; there is no second implementation to choose.
- Voice Control corrections and keyboard event taps (4b9c910c1, adf4675e1): Termurion has no NSTextInputClient replacement path and installs no event tap.
- The legacy (non-GPU) renderer fixes (e91f5de60, 935713b62, 013a840c8, 29a335e89): Termurion has one renderer.
- Sixel 8192 x 8192 cap (a9acfc907): Termurion already caps at 50 000 000 pixels and `MAX_SIDE` (`src/sixel.rs:5-7`), tighter than upstream.
- The removed Mojave shadow advanced settings (013795e69): never ported, nothing to remove.
- Translations and localizations: Termurion is English only.

## Self-review notes

- Coverage: every triage line in "Behavior Termurion still has the old way" maps to Tasks 18 (%exit, zoom), 1 (DECRQCRA), 4 (images, kitty LRU), 5 (skin tone), 22 (Show Clippings), 1 (rename), 12 (Show status on tabs), 27 (catalog), 24/25/51 (protocol). "Missing settings, menus and small behavior": Bell Sound 10, HDR 9, Quick Look keys 41/42, ShowMenuBarItem and MenuBarItemString 40, ShowTabSwitcherWhenCyclingTabs 43, FFM Including Subviews 23, MaximumConcurrentTokenExecutions 44; the 15 advanced settings: Tasks 1 (rename), 3, 6, 8, 11, 29, 38, 40, 44 (three), 52, 53, and three judged not applicable; menus 21, 49, 46, 30; key actions 49 (77), 22 (78); OSC 9;4 11; mode 2031 6; right prompt and tcsh 15; nested `tmux -CC` 19; Grok and key choice 27; password export 28; API items 24, 25, 26; OSC 7501 14; BDSM/SCP 3; DECCRA/DECFRA/DECERA 2. Class B: Tasks 35 to 60.
- Types: `api_proto::FEATURES` (Task 25) is read by Tasks 50 and 51; key action numbering 78 (Task 22) and 77 (Task 49) agree on `actions().len()` 79 then 80.

## Controller rulings (2026-11-23)

- tcsh: Task 15 ships the tcsh script. iTerm2 ships one, and its behavior decides.
- `it2 session list`: Task 26 adds the verb, so `--format` has a home, as in iTerm2.
- Rectangle sequences: Task 2 ports every rectangle-area sequence iTerm2 supports at a81a54b7, not only the three the triage named. Check DECCARA, DECRARA, DECSACE and DECSERA in upstream and add the ones it handles.
- New crates for password export (Task 28): allowed. The export format must match iTerm2, and it needs them.
