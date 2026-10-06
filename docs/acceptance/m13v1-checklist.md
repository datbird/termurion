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
