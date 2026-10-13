# Milestone 10b acceptance checklist

Build a release binary (`scripts/xt build --release`); the debug build is a console-subsystem app
and never detects a password prompt in a local pane. Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.

Setup: create a second profile with "Open Password Manager Automatically" turned on (Session
tab). Add a trigger with the regex `[Pp]assword:` and action Open Password Manager (the capital
letter matters: ssh prints "user@host's password:"); make one copy of it with the parameter set
to a specific account name and, separately, try it with the parameter left as Unlock (or empty).
Open a window with two panes and turn on Broadcast Input for both, so one can be put at a real
password prompt (`ssh` to a host, or `runas`) and the other left at an ordinary shell prompt. Have the 1Password desktop app and `op` (2.23 or newer) installed if
available, and `lpass` if available; mark a row "not applicable" instead of fail when the tool
itself is not installed on this machine. Reach a Windows account signed in with Windows Hello
(PIN), one with password-only sign-in, and, if one is available, a Microsoft account, to exercise
the authentication gate's paths. Mark each line pass or fail.

Detection (automatic opening, the key cursor and its click) fires only for a reader that turns
line input on and echo off. ssh.exe's password prompt (measured console mode 0x0000) and runas's
(0x01F1) read in raw mode and are not detected (m10b rulings); use them for Enter Password and the
trigger, not for detection. For detection, run this PowerShell reader (measured 0x0003) in a
PowerShell pane; it prints `Password: ` and waits for one line, and Enter or Ctrl+C ends it:

```
$k = Add-Type -Name K -Namespace T -PassThru -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetStdHandle(int n); [DllImport("kernel32.dll")] public static extern bool SetConsoleMode(IntPtr h, uint m);'; [void]$k::SetConsoleMode($k::GetStdHandle(-10), 3); Write-Host -NoNewline 'Password: '; $null = [Console]::In.ReadLine()
```

| # | Check | Result |
|---|---|---|
| 1 | Press Win+Alt+F (Window > Password Manager): the authentication gate runs first (Windows Hello or the Windows credential prompt captioned "Termurion" / "Termurion wants to open the password manager."), then the Password Manager window opens, one per terminal window. | |
| 2 | On a PIN-enabled account, the gate offers Windows Hello first; entering the PIN opens the manager. | |
| 3 | On a password-only account (no Windows Hello configured), the gate falls back to the Windows credential prompt (`CredUIPromptForWindowsCredentialsW`); entering the password opens the manager. | |
| 4 | On a Microsoft account, Windows Hello (or its credential-prompt fallback) authenticates the same way and the manager opens. | |
| 5 | Cancel Windows Hello (Esc or its Cancel button): the gate fails with "Authentication failed because password entry was cancelled." and the manager does not open. | |
| 6 | In Settings > Advanced > Password Manager, turn off "Require Authentication to Open Password Manager": Win+Alt+F now opens the manager with no prompt at all. | |
| 7 | Turn "Require Authentication to Open" back on and turn on "Require Authentication after Screen Locks"; lock the screen (Win+L), sign back in, and reopen the manager: it re-authenticates. While "Require Authentication on Every Open" is also on, the Screen Locks item shows checked and disabled no matter what it is actually set to. | |
| 8 | Add an account (name, user name, and a typed or generated password): it appears in the list, and in Control Panel > Credential Manager > Windows Credentials as `Termurion/<account> - <user>`. | |
| 9 | Edit the account three ways and confirm each commits correctly: rename it; change only the password; save with the password field left unchanged (no password change happens). | |
| 10 | Reveal the account's password ("Password for \"<account>\"" with OK and Copy) and separately use Copy Password from the row's right-click menu; both put the password on the clipboard. | |
| 11 | Remove the account: it disappears from the list and from Control Panel > Credential Manager. | |
| 12 | At a `runas /user:x cmd` password prompt, select the account and Enter Password: the echo probe confirms the prompt does not echo, and the password is typed and submitted. | |
| 13 | At a real `ssh` password prompt, the `[Pp]assword:` trigger opens the manager (ssh is not detected, so this is its way in); Enter Password then succeeds the same way, and with "Automatically Send Return" on, Enter follows the password automatically. | |
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
| 24 | Release build. With "Open Password Manager Automatically" on for the profile and the trigger off, run the PowerShell reader above in that session: the manager opens by itself on the rising edge of the detected prompt. At an `ssh` or `runas` prompt it does not (raw mode, by ruling). | |
| 25 | Release build. At the PowerShell reader's prompt, the cursor shows the key glyph (drawn over the cell); click directly on it: the manager opens for that session on the mouse up, and the click otherwise behaves as an ordinary click (selection start etc.). | |
| 26 | Release build. Compare the key-cursor glyph's color at the PowerShell reader's prompt in a dark-background profile and a light-background profile: white glyph on the dark background, black glyph on the light one. | |
| 27 | Confirm detection (checks 24-26) in a local pane works only from the release build; a debug build shows no key cursor there and never opens automatically, since it runs as a console-subsystem process. (Hosted panes are read by tandem-host in either build.) | |
| 28 | With the Password Manager window focused, press Win+B (Broadcast), Win+U (Enter User Name), and Win+C (Copy Password): each acts on the manager window and is not swallowed by tray focus, Accessibility, or Copilot's own use of those chords. | |
| 29 | With an IME active, type into each password field inside the manager (the password field of Add Account and of the Edit panel, a master password or login prompt's secret field, an adapter's secret Settings field): no IME candidate window ever appears. The search field and the 2FA field are plain fields, as in iTerm2, and do take the IME. | |
| 30 | Start Instant Replay on a pane (Win+Alt+B), then try Enter Password on it directly and as part of a broadcast: the replaying pane receives nothing either way; a pane that has since closed mid-broadcast is silently skipped, not sent to. | |
| 31 | With the 1Password desktop app and `op` 2.23+ installed, choose 1Password as the source: sign-in succeeds (desktop-app biometrics or the master-password prompt), the account list loads, and an OTP item's Enter Password appends its live code. | |
| 32 | With `op` not on PATH or WinGet's install folder, the "Can't Find 1Password CLI" alert appears with a Locate button; pick `op.exe` by hand, and confirm the manager works and the pick is remembered for later use. | |
| 33 | With more than one 1Password account signed in, use Switch Account: the account picker appears and forces a fresh sign-in for the chosen account. | |
| 34 | Let (or simulate) a signed-in `op` session expire, then use the manager: the first call fails, Termurion silently re-authenticates once, and the retried call succeeds with nothing shown to the user. | |
| 35 | With `lpass` installed but not logged in, choose LastPass as the source: the needs-login "Authentication Failed" alert appears with Open Terminal Window and Copy Command; Open Terminal Window runs `lpass login <email>` in a visible window and reports "Login Successful" once it succeeds. | |
| 36 | Confirm no native pinentry GUI ever appears during any LastPass operation (`LPASS_DISABLE_PINENTRY=1` is set on every `lpass` call Termurion makes). | |
| 37 | Choose KeePassXC, Bitwarden, or Keeper as the source: each shows "Adapter not found." under "Password Manager Error," then silently falls back to Windows Credential Manager. | |
| 38 | Start a slow operation (for example Reveal Password against a CLI-backed source) and click Cancel on the busy scrim before it replies: the pending reply is dropped, and the window returns to normal with nothing applied. | |
| 39 | Turn on "Require Authentication on Every Open" (with "after Screen Locks" off): close and reopen the manager several times without locking the screen; each open asks to authenticate again. Turn it off: a reopen in the same run opens with no prompt. | |
| 40 | With the `[Pp]assword:` trigger and "Open Password Manager Automatically" both on, reach the PowerShell reader's prompt (both fire together) and cancel Windows Hello: the gate fails once, and no second authentication prompt pops up at once. | |
| 41 | In a hosted pane (the session host running), run something that prints for a long time (`dir /s C:\`), close the pane, and let Undo Close expire without undoing. Then close another hosted pane: it closes; the closed panes' shells are gone from Task Manager; detection (check 24) still works in a new hosted pane. | |
| 42 | With Windows clipboard history (and, if available, cloud clipboard) on, use Copy Password, Reveal's Copy, Ctrl+C and Win+C in the manager: each pastes normally, but Win+V's history shows no password entry, and the password does not reach another signed-in device. | |
| 43 | With the Jobs tool, send INT (Ctrl+C) and QUIT (Ctrl+Break) to a program in a local pane and in a hosted pane (for example `ping -t localhost`): the program gets each one; Termurion and tandem-host keep running, and every other pane (local and hosted) survives. | |
| 44 | Send a signal to a pane at the moment another pane closes: the window never freezes; a failure (if any) shows in the Jobs banner. | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 45 | The probe's own space keystroke shows the "Password sent" toast before any password is actually sent; the probe's backspace shows no toast (see check 15). | |
| 46 | Enter User Name with Broadcast on types the account name once per broadcast session into every broadcast session, with no deduplication (see check 19). | |
| 47 | "Require Authentication after Screen Locks" shows checked and disabled while "Require Authentication on Every Open" is on, whatever it is actually set to (see check 7). | |
| 48 | Choosing a source whose check fails (a missing adapter, or an unusable CLI) shows its own alert, then silently falls back to Windows Credential Manager (see checks 32 and 37). | |
| 49 | A Windows Hello cancel fails the authentication gate outright; it does not fall back to the credential prompt (only a missing or unconfigured Hello device does) (see check 5). | |
| 50 | The Password Manager window has no animations to turn off; `disablePasswordManagerAnimations` has no visible effect. | |
| 51 | Password fields inside the manager have no caret or text selection: typing only appends, Backspace only removes the last character, and there is no way to edit the middle of a typed password. | |
| 52 | LastPass's mid-command session expiry is not silently retried the way iTerm2 does; a call made with an expired session fails once, and only reopening the manager (or the needs-login alert's Open Terminal Window) recovers it. | |
| 53 | KeePassXC, Bitwarden, and Keeper appear as choosable sources in the gear menu (the adapter protocol is built), but none of their adapter executables ship in this milestone, so all three always fall back (see check 37). | |
