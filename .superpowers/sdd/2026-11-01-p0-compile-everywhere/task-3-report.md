# Task 3 report: IPC behind platform::ipc

## Built
- `winpipe.rs` moved (git mv) to `src/platform/windows/ipc.rs`, byte for byte; `src/winpipe.rs` deleted after every caller moved.
- `src/platform/ipc.rs` facade re-exports `Pipe, Listener, ConnectError, connect, connect_owned, host_pipe_name, secret_pipe_name, url_pipe_name, own_peer, thread_peer`.
- `src/platform/unix/ipc.rs`: every function returns `Unsupported { feature: "Session host" }` as a String. `Pipe`/`Listener` hold an `Infallible`, so no value can exist. `own_peer`, `thread_peer` and `connect_owned` deny. Tests: `unix_ipc_connect_is_unsupported`, `unix_ipc_checks_deny`.
- Win32-only helpers (`user_sid`, `elevated`, `file_owner_sid`, `set_file_owner_to_user`, `process_token_facts`, `own_token_facts`, `process_image`, `session_of`, `sid_text`) are reached only through `crate::platform::windows::ipc`, from code that was already cfg(windows) or `*_win`. No ACL, SDDL, owner or token check was touched.
- Callers moved: host, host_client, ssh_secret_win, url_launch, api_peer, api_auth, localfs, browser_db, dlog, handoff_win, util_imgls, webhost_win, dynamic_profiles_trust, app/ssh_app, app/api_ui, bin/termurion-host.
- Source-scan pins followed the move: localfs `("platform/windows/ipc.rs","connect_owned")`, `("host_base.rs","log")`; launch_ui `include_str!("../platform/windows/ipc.rs")`; default_term test now looks for `platform::ipc::thread_peer()` (same proof).

## Moved to Task 4
- Moved to Task 4: host (needs pty, conpty)
- Moved to Task 4: host_client (needs pty)
Both are `#[cfg(windows)]` in lib.rs with a comment. Tiny piece moved early: new ungated `src/host_base.rs` holds `log`, `log_path`, `HOST_EXE` (used by crashdump, gpu_pick, render, url_launch, dll). `host` and `host_client` re-export them so old paths still work.

## check-unix
- Before: linux 84, macos 84. After: linux 46, macos 46. No error left in ipc, host_base, host, host_client, url_launch, api_peer, or their callers.

## Unix-only lines checked by hand (crate does not type-check cfg(unix) yet)
platform/unix/ipc.rs, all of it: `no<T>()` (`Unsupported -> String` via `From`), `match self.never {}` on `&self` (empty match on an `Infallible` place, coerces to each return type, does not move), `Duration::ZERO`, `Result::err().unwrap()` (no Debug bound needed), `matches!` with a guard on `ConnectError::Other(e)`, `Peer` is the ungated `api_peer::Peer`. platform/ipc.rs `use super::unix::ipc as imp` plus the pub use list matches the Unix names one for one.
Also checked: both cargo checks report zero diagnostics in these files (they are type-checked once their module resolves, which they do).

## Windows tests
- Focused `scripts/xt test --lib url_launch`: 17 passed.
- Full `scripts/xt test --lib`: see bottom line.

## Decisions
- Decision: host and host_client stay Windows-only now - cost if wrong: Task 4 un-gates two lines.
- Decision: added one `#[cfg(windows)]` block inside the url_launch test `a_url_argument...` for the lone-surrogate OsString (it did not compile on Unix) - cost if wrong: remove the cfg when a Unix variant exists.
- Decision: `HOST_EXE` stays "termurion-host.exe" in host_base - cost if wrong: Task 4/P1 sets a Unix name.

## Concerns
- First full xt run hung (known deadlock); killed the test process by PID on EXTREME and reran.
- Full `scripts/xt test --lib`: 6796 passed, 0 failed, 143 ignored.
