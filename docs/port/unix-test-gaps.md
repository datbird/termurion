# Unix test gaps after P0

P0 makes the crate and all programs compile on Linux and macOS. It does not make the tests pass there.
`cargo test --lib` on Linux (sirtoolio, debug build), 2026-11-01: 6263 passed, 225 failed, 11 ignored
(645 s; the whole run is slow because `workgroup::tests::the_model_never_holds_what_the_write_point_would_drop`
needs minutes on a debug build). On Windows the same tree gives 6801 passed, 0 failed, 143 ignored.
Nothing below is fixed in P0. P1 and later work through it by cause.

## By cause

1. Local shells are Unsupported (about 30 failures). Tests that start a real pane, shell or pty fail at the
   spawn with "Local shells is not available on Linux yet" (`pane::tests`, `start_dir::tests`,
   `workgroup_run::tests`, `app::git_ui`, `host_client::tests`). Fixed by the P1 pty backend.
2. Session host is Unsupported (about 25). `host::tests`, `host_client::tests`, `session_log::tests` fail with
   "Session host is not available on Linux yet". Each host test also waits 5 s in `Conn::open` before it fails,
   so these add minutes to a run.
3. Windows paths and rules in assertions (about 85, the `left == right` failures). Tests that assume `C:\`
   paths, `\` separators, cmd.exe, `.exe` names, drive letters or 8.3 names: `semantic::tests`, `netpath::tests`,
   `scripts::tests`, `app::settings_export_ui`, `app::scripts_app`, `script_provision::tests`, `script_deps::tests`,
   `script_archive::tests`, `chat_input::tests`, `app::page_saver_app`, `app::url_handler_ui`,
   `url_schemes::registration_tests`, `extract::tests`, `ssh_inject::tests`, `settings_ssh::tests`.
4. Dynamic Profiles are Windows only (about 10, `app::dynamic_profiles_app`): "Dynamic Profiles are read on
   Windows only". The app also shows that alert once at every Linux start.
5. Windows fonts and text metrics (about 10): `render::tests`, `scene::tests`, `find::tests` (text widths from
   Windows fonts, normalization with Windows font fallback).
6. Windows-only tools missing (about 10): `tar.exe`, `powershell.exe`, `cmd.exe mklink`, zip creation, the log
   folder rule ("the log folder is not a full path": `dlog`).
7. Licenses: `licenses::tests::every_dependency_is_listed` lacks an entry for webview2-com, which is
   Windows-only and absent from the Unix dependency tree.
8. Timing: `find::tests::find_over_a_large_buffer_is_quick` is a flake on a loaded machine (re-run alone).
9. One stale source scan: `app::semantic_ui::tests::every_entry_point_is_gated` failed in that run on a
   NOT_PANE_TEXT entry that sat after its test module; fixed before P0 closed.

Failing test names: run `cargo test --lib 2>&1 | grep FAILED`.

## Tests hidden from Unix on purpose

A `#[cfg(windows)]` on a test or part of one needs a line here (plan item 5). They are listed in
`scripts/cfg-audit.allow` with their reason. In `src/app/`: the DACL check in `adblock_app` (SDDL), the exclusive
share mode in `archive_ui`, the links-and-devices test in `emu_ui` (junctions, device names, file symlinks;
symlink test gated off Unix), the real-HWND tests in `menu_hold_tests`, the Mark of the Web test in
`page_saver_app` (NTFS stream), the ICU pattern test in `statusbar_content`. Outside `src/app/` many Windows-only
tests sit beside the Windows code they test (`dlog`, `localfs`, `script_provision`, `url_schemes` and others).
The `scripts/cfg-audit.allow` whole-file entries count them.

## The cfg(unix) unit tests from Tasks 1 to 7

The tests under `platform::unix::*` and the stand-ins (browser, ipc, a11y, handoff, launch pipe, clock,
shell_extras, clipboard, and others): 25 of them ran on Linux and all passed. None runs on macOS yet (the
Mac check is compile only).
