# Milestone 13v1 acceptance checklist (app integration)

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.

## The network-path judge (task 4 fix round 1, item 2)

Any probe of a path on a share sends the person's NTLM hash to the server, so Termurion must refuse
these paths without touching the network at all, the judge included. Setup: a share you control
(`\\server\share`, for example on a file server) holding `bg.png`, `notes.txt` and a git repository `repo`.
Run Wireshark on the Termurion machine with the display filter `smb2 || nbss || tcp.port == 445`
(or Process Monitor filtered to `tandem.exe` and `git.exe`, Path begins with the share and the
mapped letter). A row passes only when Termurion shows the refusal AND the capture shows no new SMB
session or request from `tandem.exe`, `tandem-host.exe` or `git.exe` while the row runs. Start each
row with `net use` listing the mapping and the capture cleared.

| # | Check | Result |
|---|---|---|
| 1 | A real mapped drive: `net use Z: \\server\share`. In Explorer, right-click `Z:\notes.txt` > Open with > Termurion. Termurion shows "Termurion does not open files on network locations." and opens nothing. Set a profile's Working Directory to `Z:\` and open a tab: it starts in the home folder with "The directory "Z:\" is on a network location. Started in home directory instead." No SMB traffic from Termurion in either step. | |
| 2 | A SUBST onto a share. First `subst Y: Z:\` (a SUBST onto the mapped drive), then a raw device-map letter onto the share itself from PowerShell: `Add-Type -Name D -Namespace W -MemberDefinition '[DllImport("kernel32.dll")] public static extern bool DefineDosDevice(int f, string d, string t);'; [W.D]::DefineDosDevice(1, 'X:', '\??\UNC\server\share')`. Repeat row 1's two steps with `Y:\notes.txt` and `X:\notes.txt`. Both are refused with the same texts, and the capture shows no SMB from Termurion (the judge reads the device map with QueryDosDeviceW and never asks GetDriveTypeW about Y: or X:). Remove them afterwards: `subst Y: /d` and `[W.D]::DefineDosDevice(2, 'X:', $null)`. | |
| 3 | A link to a share. Junctions cannot point at a share, so make a directory symbolic link (Developer Mode or an elevated prompt): `mkdir C:\t; mklink /D C:\t\share \\server\share`. Open `C:\t\share\notes.txt` with Termurion: refused, no SMB. Click `C:\t\share\notes.txt` printed in a pane (Semantic History): nothing opens and no SMB. Run `imgls C:\t` in a pane: `share` is listed as a link (it is read without being followed) and nothing is fetched from the share. | |
| 4 | A background image on a share. In a profile, set Background Image Location to `\\server\share\bg.png`, then to `Z:\bg.png`, then to `C:\t\share\bg.png` (row 3's link), each with the profile's window open. No image shows for any of them, and the capture shows no SMB from Termurion. A folder rotation pointed at `Z:\` shows nothing either. A local image still shows. | |
| 5 | A git repository on a share. Add the git component to the status bar, then in a PowerShell pane run `cd Z:\repo` (and, in another pane, `cd C:\t\share\repo`). The git component stays empty, and the capture shows no SMB from `tandem.exe` or `git.exe` (git is never started there). `cd` to a local repository: the component shows its branch. | |

## URL scheme repair (task 4 fix round 1, item 1)

| # | Check | Result |
|---|---|---|
| 6 | Bind ssh to a profile with OK. Copy the whole release folder to `D:\T2`, quit Termurion, rename the first folder, start `D:\T2\tandem.exe`: `HKCU\Software\Classes\ssh\shell\open\command` now names `D:\T2\tandem.exe` and `HKCU\Software\Tandem\Registered` value `ssh` names it too. | |
| 7 | With both copies present (undo the rename), start the copy that did not register: the ssh command still names the first copy (two live copies never take a scheme from each other). | |

## Default terminal (task 5)

Run these with a release build next to its own `conpty.dll` and `OpenConsole.exe`, Windows Terminal
installed. Note the pair first: `reg query "HKCU\Console\%%Startup"`.

| # | Check | Result |
|---|---|---|
| 8 | iTerm2 > Make iTerm2 Default Term: the banner says Termurion is the default; `reg query "HKCU\Console\%%Startup"` shows `DelegationConsole` `{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}` and `DelegationTerminal` `{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}`, `HKCU\Software\Tandem\PreviousDefaultTerminal` holds the old pair and the item is now greyed. Nothing under `HKLM` changed. | |
| 9 | With Termurion running and default, start `cmd.exe` from Win+R: a new Termurion tab (or a window when none) shows the cmd prompt, titled with the startup title; typing, resizing the window and the tab's close button work, and closing the tab ends cmd. | |
| 10 | Quit Termurion, run `cmd.exe` from Win+R: Windows starts `tandem.exe -Embedding`, the one Termurion window opens with that console and nothing else (no startup shell). Run `tandem.exe -Embedding` by hand with nothing else running and the default set: no window, and the process ends by itself about 15 s later. | |
| 11 | With Termurion running, run `tandem.exe -Embedding` by hand: it ends at once and shows nothing. Run `for ($i=0;$i -lt 30;$i++){ cmd /c start cmd }`: no more than a few tabs per second open, at most 16 stay, and the banner "N terminal hand-offs ignored" appears. | |
| 12 | Make Terminal Default Term (Alt on the iTerm2 menu): the old pair is back, `HKCU\Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}` and the saved key are gone, and the item is enabled again. With Windows Terminal removed (or renamed `wt.exe` alias), Make iTerm2 Default Term shows "Windows Terminal Required" and changes nothing. | |
| 13 | The server handle check: the first real handoff of cmd.exe must succeed (the `server` handle is checked as a process handle with `GetProcessId`). If row 9 fails with nothing opening and `host.log` names a refused handle, that check is wrong for this Windows build. | |
| 14 | Cold start (task 5 fix r1, item 1): `reg query "HKCU\Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}\LocalServer32"` shows only the quoted path to `tandem.exe` (no `-Embedding`; COM adds it). With Termurion closed, row 10 opens the console without any "Usage" message box. | |
| 15 | The caller's integrity and session (item 2): with Termurion running at medium integrity, start an elevated `cmd.exe` (Run as administrator): it does not become a Termurion tab (it opens in Windows' own console host), and `host.log` has no handoff from it. A normal `cmd.exe` still becomes a tab. | |
| 16 | The handoff class follows a moved exe (item 3): make Termurion the default, quit, move the release folder to `D:\T3` and start `D:\T3\tandem.exe`; `LocalServer32` now names `D:\T3\tandem.exe`. With the old folder back in place and a second copy started, the value stays on the copy that wrote it. | |
| 17 | An idle `-Embedding` start keeps the person's requests (item 4): with Termurion closed and the default set, run `tandem.exe -Embedding`, then within 15 s run `tandem.exe --new-window` and `tandem.exe --url ssh://localhost`: a Termurion window opens at once with both. Run `tandem.exe -Embedding` alone: it ends after about 15 s; a `tandem.exe` started right after that opens its own window normally. | |
| 18 | The reference handle (review, cannot verify): the first real handoff passes the signal and reference handle checks (`GetFileType` on the ConDrv reference handle is not FILE_TYPE_UNKNOWN with an error). If row 9 fails and `host.log` names a refused handle, that check is wrong. | |

## The network-path judge, round 5

Same setup and capture as rows 1-5.

| # | Check | Result |
|---|---|---|
| 19 | A recording or arrangement from Explorer on a share: right-click `Z:\x.itr` (and `C:\t\share\x.itermarrangement`, row 3's link) > Open with > Termurion: refused with the network banner, no SMB. Session > Log > Import Recording and Load Arrangement from File picking the same files in the dialog do open them (the person's own pick). | |
| 20 | A parent step after a link: `mkdir C:\r\x\y`, `mklink /D C:\r\d x\y`, `mklink /D C:\r\f \\server\share`. Open `C:\r\d\..\f\notes.txt` with Termurion (and click it printed in a pane): refused, no SMB. | |
| 21 | A Markdown document (Porthole) with links `[a](Z:\notes.txt)` and `[b](C:\t\share\notes.txt)`: clicking either shows the network banner and asks nothing, no SMB; with "Always" set earlier, still refused. A link to a local `.txt` asks and opens it. | |

## Jump List (task 6 fix r1)

| # | Check | Result |
|---|---|---|
| 22 | Right-click Termurion's taskbar button, Remove from this list on a profile item. Open two new tabs (two rebuilds) and edit a profile name: the item stays gone. Quit and start Termurion: still gone. `%APPDATA%\Tandem\Recents\removed.json` lists its arguments. | |

## Staying resident (task 7)

| # | Check | Result |
|---|---|---|
| 23 | With "Quit when all windows are closed" off, close the last window (Ctrl+D in its only shell, then the close button). Termurion stays: no taskbar button, no Alt+Tab entry, the notification area icon is there, and `tasklist` still shows tandem.exe. | |
| 24 | Left-click the icon: a window opens with one tab, on the screen it opened on before, with a working shell. Close it again: the icon stays. Quit from the icon's right-click menu: Termurion ends and the icon goes. | |
| 25 | Resident for an hour: no CPU use in Task Manager (0%), no wake-ups beyond Windows' own; the hidden window uses about its old memory. Restart Explorer (`taskkill /f /im explorer.exe`, then `explorer`): the icon comes back by itself. | |
| 26 | "Quit when all windows are closed" on, start Termurion and close its window within 10 seconds without typing or using Close: Termurion stays (log line "Not quitting Termurion because it ran very briefly"). Type a key, then close: Termurion ends. With MinRunningTime 0 it ends at once. | |
| 27 | OpenUntitledFile off, resident: a left click on the icon opens nothing (right-click > Main Menu > Shell > New Window still does). Start Termurion with the key off and "Open a new window at startup" on: the launch window still opens (only the launch plan decides). | |
| 28 | Resident, a Jump List item, `tandem.exe` again, or an ssh link: one window shows, with only the requested tab (a bare second start follows OpenUntitledFile). A global hotkey of a profile's hotkey window brings that window while resident. | |
| 29 | Resident, a Windows sign out or shutdown with Termurion running: it ends with the logoff, nothing hangs, the keyboard layout is not left forced after it. | |

## Shutdown and Downloads (task 8)

| # | Check | Result |
|---|---|---|
| 30 | NeverBlockSystemShutdown off, a shell running a program (for example `ping -t localhost`) and Prompt Before Closing asking: start a Windows shutdown. Windows' "apps are keeping Windows from shutting down" screen names Termurion with "Termurion is asking whether to quit.". Press Cancel: Termurion shows the quit question; Cancel keeps it running, and the screen's reason is gone. Start the shutdown again and choose "Shut down anyway": Termurion's windows and state are saved and Windows ends. | |
| 31 | Same with an idle shell and no Confirm Quit: the shutdown does not wait for Termurion and nothing is asked. With NeverBlockSystemShutdown on and a running program: no question, the state is saved, Windows ends. After signing in, Restore returns the windows. | |
| 32 | Settings > Advanced > Downloads folder: set a local folder, then `imgcat`-style download (OSC 1337 File) and an ssh download: the file lands there. Empty it: it lands in the Windows Downloads folder. Set a UNC path or a mapped share: the banner says the folder is not used and the file lands in the Windows Downloads folder. A downloaded file keeps its Mark of the Web. | |
| 33 | Settings > Advanced shows "Downloads folder." and "Overlay the suite name on the taskbar icon when launched with -suite.", the latter with no effect. | |

## Launch, open file, links, Jump List and the security rows (task 9)

Every row not marked otherwise was listed as untested in the task reports (the GUI was never launched). Rows 34 onward follow rows 1 to 33.

| # | Check | Result |
|---|---|---|
| | **Setup for rows 34 onward.** Profiles: one with an Initial Text (for example `echo hi`), one named `a "b" c` (quotes and spaces). Files: `C:\t\s.ps1` (a `Write-Host ok` script), `C:\t\a folder` (a folder with a space), a `.itr` recording, an `.itermcolors` file, an `.it2triggers` file. Windows 11 22H2 or later with Windows Terminal installed. | |
| | **A second tandem.exe (tasks 1 and 2)** | |
| 34 | With Termurion running, run `tandem.exe` again from a Run box: no second Termurion appears (Task Manager shows one `tandem.exe`), the running window comes forward, or a new window opens when none shows. | |
| 35 | `tandem.exe --new-window`, `--new-tab` and `--new-tab <profile guid>` open a window or a tab in the running Termurion and bring it forward; an unknown guid shows the banner `The profile "..." no longer exists.` and opens nothing. `tandem.exe --arrangement Name` restores a saved arrangement; an unknown name shows the arrangement banner. | |
| 36 | `tandem.exe --bogus` shows a dialog with the usage line and starts nothing. Six quick `tandem.exe --new-tab` runs open 4 tabs and show "2 launch requests ignored". | |
| 37 | Nothing running: `tandem.exe --new-tab` starts Termurion with its startup window plus one extra tab. Import All Settings and Data: Termurion restarts and comes back (the relaunch does not hand its start to the old process). A silent program holding a connection to the pipe does not delay the next `tandem.exe` request. | |
| 38 | With Termurion running, `tandem.exe C:\t\s.ps1` from a Run box asks "OK to run" once and runs the script in a new tab, which ends after it. Tick "do not ask again" (Silence) and run it again: no question. `tandem.exe "C:\t\a folder"` opens a tab in that folder with no question. | |
| 39 | Advanced > Windows "How should new instances be opened" set to 0, 1, 2 and 3 puts the next `tandem.exe C:\t\s.ps1` or folder in a window, a tab, a vertical split and a horizontal split; at -1 the three legacy keys decide. | |
| 40 | Advanced "Should opening a script disable Send Text at Start" on: only the script runs. Off: the profile's Initial Text runs first, then the script. | |
| 41 | Explorer's Open with > Termurion on a `.itr` file loads the recording. On an `.itermcolors` file it shows "Colors Scheme Imported" and the preset is in Colors > Color Presets\u{2026}; a second time asks "Add duplicate color preset?" (Cancel, Add it anyway); a broken file shows "Import Failed.". On a `.it2triggers` file it asks which profiles get the triggers. A `.iterm2arrangement` and a `.its` open through their importers (a downloaded arrangement still asks). | |
| 42 | A `.ps1` opened while Termurion runs with no window (resident) still asks "OK to run". Select 6 `.ps1` files in Explorer and Open with Termurion: one question per file, none dropped, no flood banner. Select 25: 16 ask and the banner says "9 files ignored" once. Closing a question window counts as Cancel. | |
| 43 | A script named with a quote look-alike, `$` or `%` shows "Termurion will not run ..." and runs nothing. Settings > Advanced and Explorer's Open with on a `.ps1` list Termurion (the registry is written once at startup, never a default association). | |
| | **Security: the launch pipe, files and links (review rows)** | |
| 44 | A low-integrity process holding the pipe name: close Termurion, run a low-integrity listener on `\\.\pipe\tandem-url-<SID>` (for example a small script launched through `icacls`-lowered or `psexec -l` PowerShell that creates the named pipe), then start `tandem.exe`. Termurion starts without a listener and shows the banner "Another program holds Termurion's launch pipe..."; the listener receives nothing (no request, no file path). With a normal Termurion running, `tandem.exe C:\t\s.ps1` is handed to it and the low-integrity process sees nothing. | |
| 45 | Files opened from Explorer, each refused with "Termurion does not open files on network locations." and no network traffic (packet capture, display filter `smb2 || nbss || tcp.port == 445`, as rows 1 to 5): a file on a mapped drive (`Z:\notes.txt`), a file on a SUBST drive whose target is the mapped drive or a share (`Y:\notes.txt`, row 2), a file opened through a directory symbolic link or junction chain that ends on a share (`C:\t\share\notes.txt`, row 3, and `C:\r\d\..\f\notes.txt`, row 20). The capture shows no SMB session from `tandem.exe` while each row runs. | |
| 46 | A downloaded script asks and is never silenced: save a `.ps1` from the internet (or add the stream `Zone.Identifier` with `ZoneId=3`), tick "do not ask again" on a local script first, then open the downloaded one with Termurion: the question shows with the line "This file came from outside this computer or the network." and asks every time. A local, not downloaded script then runs without the question. | |
| 47 | An ftp link's password is never saved: `start ftp://bob:secret@localhost/x` opens a tab running `ftp ftp://bob@localhost/x` (no password on the command line, in Task Manager's command line column or in the tab's title); quit Termurion with the tab open, restart, restore the window and save an arrangement: `secret` appears nowhere in `%APPDATA%\Tandem` (`findstr /s /i secret`). The ftp program asks for the password itself. | |
| 48 | A handoff from another session or an elevated console is refused: with Termurion at medium integrity and default terminal on, an elevated `cmd.exe` (Run as administrator) opens in Windows' own console host and is no Termurion tab (row 15); a `cmd.exe` started from a second Windows session of the same user (Fast User Switching or `psexec -s -i 2`) does not become a tab and `host.log` has no handoff from it. A request written to the launch pipe from another session is not accepted either. | |
| 49 | A Jump List entry for a profile named with quotes and spaces opens exactly that profile: right-click the taskbar button, New Tab... > `a "b" c` opens one tab of that profile and nothing else (no extra window, no second argument taken from the name); a profile named `--new-window` or ending in a backslash does the same; the item for a profile deleted since the list was built shows `The profile "..." no longer exists.` and opens nothing. | |
| | **URLs and schemes (tasks 3 and 4)** | |
| 50 | Profiles > General "Schemes handled:" > ssh asks "Termurion is not the default handler for ssh...", OK, then `start ssh://localhost` opens a tab running `ssh localhost`; `start "ssh://bad user@h"` shows "Illegal Username" and opens nothing. The menu lists ftp, ssh and telnet and checks only the bound, default scheme. Choosing ssh with another handler names it in the question; when Windows keeps the other app, Default apps opens on Termurion's page. Moving `tandem.exe` and starting it makes a bound scheme's command follow the new path (rows 6 and 7). | |
| 51 | `tandem.exe --url telnet://u@host:23` opens a tab running `telnet -l u host 23` with the banner "Opened a telnet link to u@host:23."; `--url ftp://host/pub/f.txt` opens a tab running `ftp ftp://host/pub/f.txt` (Windows' own ftp.exe takes a host, not a URL, so it prints "Unknown host" unless Path to ftp names a URL-aware program); `--url telnet://-h` opens nothing and `--url telnet://a;b@h` shows Illegal Username. Advanced Path to ftp and Path to telnet change the program. A profile with a custom command keeps it and its Send Text at Start is not typed. | |
| | **Default terminal, Jump List, residency and shutdown (the step 2 list)** | |
| 52 | Make iTerm2 Default Term, then Win+R `cmd` opens in a Termurion tab; Make Terminal Default Term restores the old default (rows 8 to 18 in detail). The Jump List (right-click the taskbar button) lists New Window (Default Profile), New Window..., New Tab..., Restore Arrangement (only when arrangements exist). Each click opens in the running Termurion. With "Add items to Recents" on, recents appear (newest first, one per profile, "New "name" tab") and Remove Recent Profiles from Dock Menu clears them; renaming a profile and clicking its old recent asks "Profile Renamed" with Open <old>, Open <new>, Cancel. The setting is in Settings > Advanced (Tabs), off by default. | |
| 53 | Close every window: Termurion keeps its notification area icon, a left click opens a window (rows 23 to 29). "Quit when all windows are closed" on: closing the last window within 10 s of launch without typing keeps Termurion, after typing it quits. Shut Windows down with a running `ping -t`: Windows lists Termurion as waiting with the reason and the quit question shows; with "Disable all confirmations on system shutdown" on, Windows shuts down without it (rows 30 and 31). A download goes to the Downloads folder after the folder was moved to D: (row 32). | |
| | **Final review fix wave** | |
| 54 | Two windows open, a `ping -t localhost` running and Prompt Before Closing asking: start a Windows shutdown. Windows names Termurion with "Termurion is asking whether to quit." and ONE quit question shows; Windows asking again (or a second window) never shows a second question and Termurion stays responsive meanwhile. Cancel in the question: Termurion keeps running and the next shutdown asks again with the reason. | |
| 55 | Same, but press Windows' "Shut down anyway" while the quit question is open: the question disappears, Termurion ends within 5 s, and after signing in Termurion restores its windows (state saved), the keyboard layout is the person's own and no stale tray icon remains. | |
| 56 | Same, but press Cancel on Windows' own shutdown screen while the question is open: the reason goes; the question stays until answered (OK quits, Cancel keeps Termurion). | |
| 57 | Resident (every window closed): open the Profiles window from the tray icon, select three profiles and Open in tabs: ONE window opens with three tabs. Two `start ssh://localhost` in quick succession while resident: one window with two tabs, each running ssh. | |
| 58 | Resident: `tandem.exe --new-tab <a deleted profile guid>` and a Jump List item of a deleted profile show a notification "The profile ... no longer exists." and open nothing; when a window next opens it shows no stale banner. | |
| 59 | "Quit when all windows are closed" off: Session > Bury Session on the only session of the only window hides the window and the tray icon stays; the tray's Buried Sessions restores it into a window, the program still running. With the key on, the banner "The last session cannot be buried: Termurion quits when its last window closes (Settings > General > Closing)." shows instead. | |
| 60 | A pane prints `search-ms:query=x`, `shell:startup` and `vscode://file/c:/x`: the link click (Semantic History) on each opens nothing; `https://example.com` and `mailto:a@b.c` still open. | |
| 61 | Downloads: set DownloadsDirectory to a share, download twice: the banner shows once, not per download; change the setting to a local folder: the next download goes there. Install Termurion under a folder named with `%` and bind ssh: the binding is refused (no registry write). | |
