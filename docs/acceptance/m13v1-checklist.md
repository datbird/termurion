# Milestone 13v1 acceptance checklist (app integration)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

## The network-path judge (task 4 fix round 1, item 2)

Any probe of a path on a share sends the person's NTLM hash to the server, so Tandem must refuse
these paths without touching the network at all, the judge included. Setup: a share you control
(`\\server\share`, for example on bigboss) holding `bg.png`, `notes.txt` and a git repository `repo`.
Run Wireshark on the Tandem machine with the display filter `smb2 || nbss || tcp.port == 445`
(or Process Monitor filtered to `tandem.exe` and `git.exe`, Path begins with the share and the
mapped letter). A row passes only when Tandem shows the refusal AND the capture shows no new SMB
session or request from `tandem.exe`, `tandem-host.exe` or `git.exe` while the row runs. Start each
row with `net use` listing the mapping and the capture cleared.

| # | Check | Result |
|---|---|---|
| 1 | A real mapped drive: `net use Z: \\server\share`. In Explorer, right-click `Z:\notes.txt` > Open with > Tandem. Tandem shows "Tandem does not open files on network locations." and opens nothing. Set a profile's Working Directory to `Z:\` and open a tab: it starts in the home folder with "The directory "Z:\" is on a network location. Started in home directory instead." No SMB traffic from Tandem in either step. | |
| 2 | A SUBST onto a share. First `subst Y: Z:\` (a SUBST onto the mapped drive), then a raw device-map letter onto the share itself from PowerShell: `Add-Type -Name D -Namespace W -MemberDefinition '[DllImport("kernel32.dll")] public static extern bool DefineDosDevice(int f, string d, string t);'; [W.D]::DefineDosDevice(1, 'X:', '\??\UNC\server\share')`. Repeat row 1's two steps with `Y:\notes.txt` and `X:\notes.txt`. Both are refused with the same texts, and the capture shows no SMB from Tandem (the judge reads the device map with QueryDosDeviceW and never asks GetDriveTypeW about Y: or X:). Remove them afterwards: `subst Y: /d` and `[W.D]::DefineDosDevice(2, 'X:', $null)`. | |
| 3 | A link to a share. Junctions cannot point at a share, so make a directory symbolic link (Developer Mode or an elevated prompt): `mkdir C:\t; mklink /D C:\t\share \\server\share`. Open `C:\t\share\notes.txt` with Tandem: refused, no SMB. Click `C:\t\share\notes.txt` printed in a pane (Semantic History): nothing opens and no SMB. Run `imgls C:\t` in a pane: `share` is listed as a link (it is read without being followed) and nothing is fetched from the share. | |
| 4 | A background image on a share. In a profile, set Background Image Location to `\\server\share\bg.png`, then to `Z:\bg.png`, then to `C:\t\share\bg.png` (row 3's link), each with the profile's window open. No image shows for any of them, and the capture shows no SMB from Tandem. A folder rotation pointed at `Z:\` shows nothing either. A local image still shows. | |
| 5 | A git repository on a share. Add the git component to the status bar, then in a PowerShell pane run `cd Z:\repo` (and, in another pane, `cd C:\t\share\repo`). The git component stays empty, and the capture shows no SMB from `tandem.exe` or `git.exe` (git is never started there). `cd` to a local repository: the component shows its branch. | |

## URL scheme repair (task 4 fix round 1, item 1)

| # | Check | Result |
|---|---|---|
| 6 | Bind ssh to a profile with OK. Copy the whole release folder to `D:\T2`, quit Tandem, rename the first folder, start `D:\T2\tandem.exe`: `HKCU\Software\Classes\ssh\shell\open\command` now names `D:\T2\tandem.exe` and `HKCU\Software\Tandem\Registered` value `ssh` names it too. | |
| 7 | With both copies present (undo the rename), start the copy that did not register: the ssh command still names the first copy (two live copies never take a scheme from each other). | |

## Default terminal (task 5)

Run these with a release build next to its own `conpty.dll` and `OpenConsole.exe`, Windows Terminal
installed. Note the pair first: `reg query "HKCU\Console\%%Startup"`.

| # | Check | Result |
|---|---|---|
| 8 | iTerm2 > Make iTerm2 Default Term: the banner says Tandem is the default; `reg query "HKCU\Console\%%Startup"` shows `DelegationConsole` `{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}` and `DelegationTerminal` `{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}`, `HKCU\Software\Tandem\PreviousDefaultTerminal` holds the old pair and the item is now greyed. Nothing under `HKLM` changed. | |
| 9 | With Tandem running and default, start `cmd.exe` from Win+R: a new Tandem tab (or a window when none) shows the cmd prompt, titled with the startup title; typing, resizing the window and the tab's close button work, and closing the tab ends cmd. | |
| 10 | Quit Tandem, run `cmd.exe` from Win+R: Windows starts `tandem.exe -Embedding`, the one Tandem window opens with that console and nothing else (no startup shell). Run `tandem.exe -Embedding` by hand with nothing else running and the default set: no window, and the process ends by itself about 15 s later. | |
| 11 | With Tandem running, run `tandem.exe -Embedding` by hand: it ends at once and shows nothing. Run `for ($i=0;$i -lt 30;$i++){ cmd /c start cmd }`: no more than a few tabs per second open, at most 16 stay, and the banner "N terminal hand-offs ignored" appears. | |
| 12 | Make Terminal Default Term (Alt on the iTerm2 menu): the old pair is back, `HKCU\Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}` and the saved key are gone, and the item is enabled again. With Windows Terminal removed (or renamed `wt.exe` alias), Make iTerm2 Default Term shows "Windows Terminal Required" and changes nothing. | |
| 13 | The server handle check: the first real handoff of cmd.exe must succeed (the `server` handle is checked as a process handle with `GetProcessId`). If row 9 fails with nothing opening and `host.log` names a refused handle, that check is wrong for this Windows build. | |
| 14 | Cold start (task 5 fix r1, item 1): `reg query "HKCU\Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}\LocalServer32"` shows only the quoted path to `tandem.exe` (no `-Embedding`; COM adds it). With Tandem closed, row 10 opens the console without any "Usage" message box. | |
| 15 | The caller's integrity and session (item 2): with Tandem running at medium integrity, start an elevated `cmd.exe` (Run as administrator): it does not become a Tandem tab (it opens in Windows' own console host), and `host.log` has no handoff from it. A normal `cmd.exe` still becomes a tab. | |
| 16 | The handoff class follows a moved exe (item 3): make Tandem the default, quit, move the release folder to `D:\T3` and start `D:\T3\tandem.exe`; `LocalServer32` now names `D:\T3\tandem.exe`. With the old folder back in place and a second copy started, the value stays on the copy that wrote it. | |
| 17 | An idle `-Embedding` start keeps the person's requests (item 4): with Tandem closed and the default set, run `tandem.exe -Embedding`, then within 15 s run `tandem.exe --new-window` and `tandem.exe --url ssh://localhost`: a Tandem window opens at once with both. Run `tandem.exe -Embedding` alone: it ends after about 15 s; a `tandem.exe` started right after that opens its own window normally. | |
| 18 | The reference handle (review, cannot verify): the first real handoff passes the signal and reference handle checks (`GetFileType` on the ConDrv reference handle is not FILE_TYPE_UNKNOWN with an error). If row 9 fails and `host.log` names a refused handle, that check is wrong. | |

## The network-path judge, round 5

Same setup and capture as rows 1-5.

| # | Check | Result |
|---|---|---|
| 19 | A recording or arrangement from Explorer on a share: right-click `Z:\x.itr` (and `C:\t\share\x.itermarrangement`, row 3's link) > Open with > Tandem: refused with the network banner, no SMB. Session > Log > Import Recording and Load Arrangement from File picking the same files in the dialog do open them (the person's own pick). | |
| 20 | A parent step after a link: `mkdir C:\r\x\y`, `mklink /D C:\r\d x\y`, `mklink /D C:\r\f \\server\share`. Open `C:\r\d\..\f\notes.txt` with Tandem (and click it printed in a pane): refused, no SMB. | |
| 21 | A Markdown document (Porthole) with links `[a](Z:\notes.txt)` and `[b](C:\t\share\notes.txt)`: clicking either shows the network banner and asks nothing, no SMB; with "Always" set earlier, still refused. A link to a local `.txt` asks and opens it. | |

## Jump List (task 6 fix r1)

| # | Check | Result |
|---|---|---|
| 22 | Right-click Tandem's taskbar button, Remove from this list on a profile item. Open two new tabs (two rebuilds) and edit a profile name: the item stays gone. Quit and start Tandem: still gone. `%APPDATA%\Tandem\Recents\removed.json` lists its arguments. | |

## Staying resident (task 7)

| # | Check | Result |
|---|---|---|
| 23 | With "Quit when all windows are closed" off, close the last window (Ctrl+D in its only shell, then the close button). Tandem stays: no taskbar button, no Alt+Tab entry, the notification area icon is there, and `tasklist` still shows tandem.exe. | |
| 24 | Left-click the icon: a window opens with one tab, on the screen it opened on before, with a working shell. Close it again: the icon stays. Quit from the icon's right-click menu: Tandem ends and the icon goes. | |
| 25 | Resident for an hour: no CPU use in Task Manager (0%), no wake-ups beyond Windows' own; the hidden window uses about its old memory. Restart Explorer (`taskkill /f /im explorer.exe`, then `explorer`): the icon comes back by itself. | |
| 26 | "Quit when all windows are closed" on, start Tandem and close its window within 10 seconds without typing or using Close: Tandem stays (log line "Not quitting Tandem because it ran very briefly"). Type a key, then close: Tandem ends. With MinRunningTime 0 it ends at once. | |
| 27 | OpenUntitledFile off, resident: a left click on the icon opens nothing (right-click > Main Menu > Shell > New Window still does). Start Tandem with the key off and "Open a new window at startup" on: it starts resident, no window. | |
| 28 | Resident, a Jump List item, `tandem.exe` again, or an ssh link: one window shows, with only the requested tab (a bare second start follows OpenUntitledFile). A global hotkey of a profile's hotkey window brings that window while resident. | |
| 29 | Resident, a Windows sign out or shutdown with Tandem running: it ends with the logoff, nothing hangs, the keyboard layout is not left forced after it. | |

## Shutdown and Downloads (task 8)

| # | Check | Result |
|---|---|---|
| 30 | NeverBlockSystemShutdown off, a shell running a program (for example `ping -t localhost`) and Prompt Before Closing asking: start a Windows shutdown. Windows' "apps are keeping Windows from shutting down" screen names Tandem with "Tandem is asking whether to quit.". Press Cancel: Tandem shows the quit question; Cancel keeps it running, and the screen's reason is gone. Start the shutdown again and choose "Shut down anyway": Tandem's windows and state are saved and Windows ends. | |
| 31 | Same with an idle shell and no Confirm Quit: the shutdown does not wait for Tandem and nothing is asked. With NeverBlockSystemShutdown on and a running program: no question, the state is saved, Windows ends. After signing in, Restore returns the windows. | |
| 32 | Settings > Advanced > Downloads folder: set a local folder, then `imgcat`-style download (OSC 1337 File) and an ssh download: the file lands there. Empty it: it lands in the Windows Downloads folder. Set a UNC path or a mapped share: the banner says the folder is not used and the file lands in the Windows Downloads folder. A downloaded file keeps its Mark of the Web. | |
| 33 | Settings > Advanced shows "Downloads folder." and "Overlay the suite name on the taskbar icon when launched with -suite.", the latter with no effect. | |

