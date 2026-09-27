# Milestone 10b acceptance checklist

Build a release binary (`scripts/xt build --release`); the debug build is a console-subsystem app
and never detects a password prompt. Close any open Tandem window, copy `tandem.exe`,
`tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: create a second profile with "Open Password Manager Automatically" turned on (Session
tab). Add a trigger with the regex `Password:` and action Open Password Manager; make one copy of
it with the parameter set to a specific account name and, separately, try it with the parameter
left as Unlock (or empty). Open a window with two panes and turn on Broadcast Input for both, so
one can be put at a real password prompt (`ssh` to a host, or `runas`) and the other left at an
ordinary shell prompt. Have the 1Password desktop app and `op` (2.23 or newer) installed if
available, and `lpass` if available; mark a row "not applicable" instead of fail when the tool
itself is not installed on this machine. Reach a Windows account signed in with Windows Hello
(PIN), one with password-only sign-in, and, if one is available, a Microsoft account, to exercise
the authentication gate's paths. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Press Win+Alt+F (Window > Password Manager): the authentication gate runs first (Windows Hello or the Windows credential prompt captioned "Tandem" / "Tandem wants to open the password manager."), then the Password Manager window opens, one per terminal window. | |
| 2 | On a PIN-enabled account, the gate offers Windows Hello first; entering the PIN opens the manager. | |
| 3 | On a password-only account (no Windows Hello configured), the gate falls back to the Windows credential prompt (`CredUIPromptForWindowsCredentialsW`); entering the password opens the manager. | |
| 4 | On a Microsoft account, Windows Hello (or its credential-prompt fallback) authenticates the same way and the manager opens. | |
| 5 | Cancel Windows Hello (Esc or its Cancel button): the gate fails with "Authentication failed because password entry was cancelled." and the manager does not open. | |
| 6 | In Settings > Advanced > Password Manager, turn off "Require Authentication to Open Password Manager": Win+Alt+F now opens the manager with no prompt at all. | |
| 7 | Turn "Require Authentication to Open" back on and turn on "Require Authentication after Screen Locks"; lock the screen (Win+L), sign back in, and reopen the manager: it re-authenticates. While "Require Authentication on Every Open" is also on, the Screen Locks item shows checked and disabled no matter what it is actually set to. | |
| 8 | Add an account (name, user name, and a typed or generated password): it appears in the list, and in Control Panel > Credential Manager > Windows Credentials as `Tandem/<account> - <user>`. | |
| 9 | Edit the account three ways and confirm each commits correctly: rename it; change only the password; save with the password field left unchanged (no password change happens). | |
| 10 | Reveal the account's password ("Password for \"<account>\"" with OK and Copy) and separately use Copy Password from the row's right-click menu; both put the password on the clipboard. | |
| 11 | Remove the account: it disappears from the list and from Control Panel > Credential Manager. | |
| 12 | At a `runas /user:x cmd` password prompt, select the account and Enter Password: the echo probe confirms the prompt does not echo, and the password is typed and submitted. | |
| 13 | At a real `ssh` password prompt, Enter Password succeeds the same way; with "Automatically Send Return" on, Enter follows the password automatically. | |
| 14 | At an ordinary shell prompt that echoes (not a real password prompt), Enter Password shows the "Are you really at a password prompt?"-style check: Return alone answers Cancel (nothing is sent); clicking "Enter Password" sends anyway. | |
| 15 | Watch for the "Password sent" toast during check 12 or 13: it appears for the probe's own space keystroke, before the real password is sent (kept bug), and does not appear again for the probe's backspace. | |
| 16 | Type a value into the 2FA field before Enter Password: the typed value (trimmed) is appended to the password instead of the account's own one-time code. | |
| 17 | Leave the 2FA field empty on an account with "Append OTP" on (or a 1Password item carrying an OTP): the account's own one-time code is appended instead. | |
| 18 | Use Enter User Name (button or Win+U): the account's user name, followed by Return, is typed into the focused session. | |
| 19 | With Broadcast on and both panes in the group, Enter User Name types the name once into each broadcast session (kept bug: it is not deduplicated or combined). | |
| 20 | With Broadcast on, one pane at a password prompt and the other at an ordinary shell, Enter Password: the "Not all sessions at password prompt" warning appears; Cancel sends to neither pane, and "Enter Password in Sessions at Prompt" sends only to the pane(s) actually at a prompt. | |
| 21 | With every broadcast pane at a password prompt, Enter Password sends to all of them at once with no warning shown. | |
| 22 | Fire the "Password:" trigger whose parameter names a specific account on a matching output line: the manager opens already showing that account selected. | |
| 23 | Fire the trigger whose parameter is Unlock (or empty) on a matching line: the manager opens with no account preselected. | |
| 24 | With "Open Password Manager Automatically" on for the profile, reach a real password prompt in that session (for example `ssh`) with no trigger and no click: the manager opens by itself on the rising edge of the detected prompt. | |
| 25 | At a password prompt, click directly on the key-cursor glyph in the cursor cell: the manager opens for that session, and the click still behaves as an ordinary click otherwise (selection start etc.). | |
| 26 | Compare the key-cursor glyph's color in a dark-background profile and a light-background profile: white glyph on the dark background, black glyph on the light one. | |
| 27 | Confirm password-prompt detection (checks 22-26) only works from the release build; a debug build shows no key cursor and never opens automatically, since it runs as a console-subsystem process. | |
| 28 | With the Password Manager window focused, press Win+B (Broadcast), Win+U (Enter User Name), and Win+C (Copy Password): each acts on the manager window and is not swallowed by tray focus, Accessibility, or Copilot's own use of those chords. | |
| 29 | Type into a password field inside the manager (the search/typed password field, the 2FA field, or an adapter's secret Settings field): confirm no IME candidate window ever appears, even with an IME active. | |
| 30 | Start Instant Replay on a pane (Win+Alt+B), then try Enter Password on it directly and as part of a broadcast: the replaying pane receives nothing either way; a pane that has since closed mid-broadcast is silently skipped, not sent to. | |
| 31 | With the 1Password desktop app and `op` 2.23+ installed, choose 1Password as the source: sign-in succeeds (desktop-app biometrics or the master-password prompt), the account list loads, and an OTP item's Enter Password appends its live code. | |
| 32 | With `op` not on PATH or WinGet's install folder, the "Can't Find 1Password CLI" alert appears with a Locate button; pick `op.exe` by hand, and confirm the manager works and the pick is remembered for later use. | |
| 33 | With more than one 1Password account signed in, use Switch Account: the account picker appears and forces a fresh sign-in for the chosen account. | |
| 34 | Let (or simulate) a signed-in `op` session expire, then use the manager: the first call fails, Tandem silently re-authenticates once, and the retried call succeeds with nothing shown to the user. | |
| 35 | With `lpass` installed but not logged in, choose LastPass as the source: the needs-login "Authentication Failed" alert appears with Open Terminal Window and Copy Command; Open Terminal Window runs `lpass login <email>` in a visible window and reports "Login Successful" once it succeeds. | |
| 36 | Confirm no native pinentry GUI ever appears during any LastPass operation (`LPASS_DISABLE_PINENTRY=1` is set on every `lpass` call Tandem makes). | |
| 37 | Choose KeePassXC, Bitwarden, or Keeper as the source: each shows "Adapter not found." under "Password Manager Error," then silently falls back to Windows Credential Manager. | |
| 38 | Start a slow operation (for example Reveal Password against a CLI-backed source) and click Cancel on the busy scrim before it replies: the pending reply is dropped, and the window returns to normal with nothing applied. | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 39 | The probe's own space keystroke shows the "Password sent" toast before any password is actually sent; the probe's backspace shows no toast (see check 15). | |
| 40 | Enter User Name with Broadcast on types the account name once per broadcast session into every broadcast session, with no deduplication (see check 19). | |
| 41 | "Require Authentication after Screen Locks" shows checked and disabled while "Require Authentication on Every Open" is on, whatever it is actually set to (see check 7). | |
| 42 | Choosing a source whose check fails (a missing adapter, or an unusable CLI) shows its own alert, then silently falls back to Windows Credential Manager (see checks 32 and 37). | |
| 43 | A Windows Hello cancel fails the authentication gate outright; it does not fall back to the credential prompt (only a missing or unconfigured Hello device does) (see check 5). | |
| 44 | The Password Manager window has no animations to turn off; `disablePasswordManagerAnimations` has no visible effect. | |
| 45 | Password fields inside the manager have no caret or text selection: typing only appends, Backspace only removes the last character, and there is no way to edit the middle of a typed password. | |
| 46 | LastPass's mid-command session expiry is not silently retried the way iTerm2 does; a call made with an expired session fails once, and only reopening the manager (or the needs-login alert's Open Terminal Window) recovers it. | |
| 47 | KeePassXC, Bitwarden, and Keeper appear as choosable sources in the gear menu (the adapter protocol is built), but none of their adapter executables ship in this milestone, so all three always fall back (see check 37). | |
