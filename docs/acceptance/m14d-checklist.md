# Milestone 14d acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe` (SSH profiles run its
`utilities\it2ssh.exe`), `conpty.dll`, and `OpenConsole.exe` from
`%USERPROFILE%\src\termurion\target\release\` (the worktree build is in
`%USERPROFILE%\src\termurion-14d\target\release\`) together, then run `tandem.exe` once. Quit Termurion,
end `tandem-host.exe` (Task Manager), and start Termurion again.

Nothing of 14d was seen on screen while it was built: no window was drawn, no App-level flow ran,
and no reveal, drop, alert or menu was exercised (the pure layers carry the unit tests). Every
row below is therefore a first look. Do 14c's checklist first (framing must work).

Setup:

- Host A: a Linux host with Python 3.7 or later, reachable with OpenSSH for Windows and key
  authentication, with a home folder holding a few files, a hidden file (`.hidden`), a folder
  `proj` with files inside, a file `a.txt`, and a file with an accented name (`cafe` with an e-acute, `.txt`).
- Host B: a second Linux host reachable the same way, for the Connected Hosts rows.
- The `SSH test` profile (General > Command type SSH, Command `user@hostA`, Configure...: SSH
  Integration on), and a second profile for host B. A plain PowerShell profile for the local checks.
- A small local folder `C:\dl-test` with `one.txt`, `two.txt`, and a folder `dir1` with a file.

Run every check in a new session unless it says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | With only a PowerShell tab open, Shell > ssh > Download Files... is disabled. Open `SSH test` and wait for framing: the item turns on. Switch to the PowerShell tab: it is off again (per window and per focus). Open a second Termurion window: the item is right there too. | |
| 2 | Choose Download Files... in the ssh pane: the panel opens (title "Open") on the remote home, host A is selected under Connected Hosts, and the listing shows `a.txt`, `proj` and the others with size, kind and date ("M/d/yy h:mm a") columns. `.hidden` is not listed. | |
| 3 | In the panel: open `proj` (double click), Back, Forward, Go Up (Ctrl+Up), Go to Home (Ctrl+Shift+H), Refresh (Ctrl+R). The path field and the listing follow each. Go to Folder (Ctrl+Shift+G): typing `~/pr` offers `proj` (completions are lower-cased, a kept quirk). | |
| 4 | Show Hidden Files: `.hidden` appears and the listing is fetched again (a kept quirk). Click each column header: it sorts, and a second click on the same header reverses the sort. The Date Created column shows the modification date. | |
| 5 | Type a file name in the search field and press Enter: matching files under the folder stream in; pressing Enter on an empty field returns to the listing; changing folder stops the search. | |
| 6 | Open host B's `SSH` tab too: Connected Hosts lists both hosts; selecting host B lists its home even if the stat of its saved path failed (kept quirk). Close host B's tab: its name goes from the list. | |
| 7 | Select `a.txt` and one more file, press Open: both download into a new `%TEMP%\Tandem-<id>` folder and Explorer opens with both selected. The batch folder name is Termurion's (a deviation from iTerm2's bare `%TEMP%`). | |
| 8 | In a second run select the folder `proj` only and press Open: its files are inside `%TEMP%\Tandem-<id>\proj\proj` (iTerm2's nesting) and Explorer shows it selected. | |
| 9 | Select one file and one folder together: Open stays disabled (iTerm2's rule). Select two files: Open is on. | |
| 10 | Right-click a downloaded file > Properties: the General tab shows "This file came from another computer" (the Mark of the Web, with an `ssh://` HostUrl in its `Zone.Identifier` stream: `Get-Content -Stream Zone.Identifier <file>`). | |
| 11 | Start a Download Files of a large file, then close the SSH tab (or `Disconnect`) before it ends: a notification "Download failed for ..." with the reason "The connection to <host> was lost." arrives, the transfer slot frees (a following download works) and no `.Termurion.*` file remains in the batch folder. | |
| 12 | Close the panel with the window button and reopen it: it opens on the last folder (the saved path, stored on Open, close and resize). | |
| 13 | `ls` in the ssh pane, then Win-click `a.txt`: it downloads into `%USERPROFILE%\Downloads` and a "Download finished for ..." notification appears; the file carries the mark (as row 10). | |
| 14 | Win-click `a.txt` again: an alert asks "Overwrite existing file?" with Keep Both and Replace. Keep Both saves `a (2).txt`; again with Replace overwrites. Closing that alert with its window button keeps both (never replaces). Tick the remember box if offered: Replace is never remembered. | |
| 15 | Win-click the folder name `proj`: `proj.zip` lands in Downloads (a kept quirk: a click gives a zip, Download Files unzips). | |
| 16 | Win-click the accented file: "File not found" (the framer's, kept: only request paths are latin1). | |
| 17 | In a PowerShell tab, `ls C:\dl-test` and Win-click a local file name: it opens or reveals locally as before (a local pane is untouched). | |
| 18 | Alt-drop `one.txt` from Explorer onto the ssh session: an alert "OK to copy ... to user@host:/path?" shows host and path; OK uploads it and `ls` shows it. Drop it again: the remote file becomes `one (2).txt`, the first stays. | |
| 19 | Alt-drop the folder `dir1`: after OK the remote has `dir1.tgz`. Drop the same files WITHOUT Alt: their paths paste as before (quoted). | |
| 20 | Alt-drop a file over 64 MiB: a message refuses it and nothing is sent. Alt-drop a `\\server\share\file` network path: refused with a message. | |
| 21 | Alt-drop a file, and close the confirm alert with its window button (or Esc): it is Cancel, nothing is uploaded and no remote temp file is left (security). | |
| 22 | In the Composer on host A, type `cat a` then Tab: remote file names complete; type `gi` Tab at the start of a line: remote commands (from the remote PATH) complete. A name with a newline or bidi control never appears. | |
| 23 | Win-click, SECURITY: on host A run `printf 'C:\\Windows\\System32\\calc.exe\n'` and Win-click that text; then run `printf '\033]1337;RemoteHost=user@localhost\a'` and click it again (a host the remote reports never makes the pane local). Nothing opens or runs on this PC; at most a download of a remote path is tried. | |
| 24 | Right-click the same remote text: Smart Selection actions (Open File, Run Command, Run Coprocess, Open URL) are greyed; Copy and Send text work. Show in Explorer is greyed. A local pane's menu is unchanged. | |
| 25 | In the ssh pane run `printf 'file:///C:/Windows/win.ini\n'` and Win-click it, and Open Selection on it: nothing opens locally (a download of `/C:/Windows/win.ini` from the remote is tried and fails). `printf 'https://example.com\n'` and click: the browser opens (a web link is allowed). | |
| 26 | Porthole: in an ssh pane, a document link to a local-looking path does not open or reveal locally (it downloads or does nothing). Status bar: click the Current Directory component in an ssh pane: its menu has Show in Explorer greyed and nothing opens locally. New Session Here from an ssh pane does not start in the remote path. | |
| 27 | Arrangements, SECURITY: in a non-SSH profile (PowerShell) run `ssh user@hostA` by hand with integration so the pane frames, `cd /tmp/x`, then Window > Save Window Arrangement. Restore it: the PowerShell pane starts in its profile's start folder, not `/tmp/x`. Do the same with the `SSH test` profile: it reconnects as in 14c. | |
| 28 | Zip bomb watchdog, SECURITY: on host A run `mkdir big; head -c 3G /dev/zero > big/z`, then Download Files `big` (an honest 3 GiB folder; a forged archive that understates its size cannot be made from the shell, its watch is covered by the unit test only). With little free space the extraction stops, the partial tree is removed from `%TEMP%\Tandem-<id>` and a notification says there is no room. With less than 1 GiB free on the drive a large folder is refused ("Not enough space on the drive"). Termurion does not freeze while it runs. | |
| 29 | A folder holding a symlink (`ln -s /etc proj/link`) or a file named `x.lnk`, `x.url` or `desktop.ini`: Download Files refuses the item naming the entry; no `.lnk` or `.url` file is ever created locally from a remote name. | |
| 30 | Connection lost mid-transfer: pull the network cable of host A (or kill sshd) during a Win-click download of a big file: the item fails, a notification "Download failed" arrives, the slot frees. Any `.Termurion.*` hidden file older than a day in Downloads or `%TEMP%` is swept at the next launch. | |
| 31 | A FAT (or exFAT) drive as Downloads or Temp (set `TEMP` for a test, or Download Files into it): the download fails "Failed to quarantine" and the temp is removed (an iTerm2 difference). | |
| 32 | With a big folder in the panel (over 100000 entries, if one exists): the listing is cut with a notice. Search results stop at 10000. | |
| 33 | Notifications: a download whose name holds a bidi or zero-width character shows a cleaned name. A missing Downloads folder notifies instead of failing silently. | |
| 34 | Two Win-click downloads of the same name at once: both end, the last one wins, neither interleaves, no temp stays. | |
| 35 | The Download Files menu item in a second window follows that window's pane (row 1) and the panel's Ctrl shortcuts do not trigger Windows Win-key shortcuts. | |

Rulings that decide what you see (full list with costs in
`docs/superpowers/rulings/m14d-rulings.txt`): every remote name passes the inert-file rule before the
first fetch and every downloaded file is marked; a remote that sends more than it claimed fails the
download; a size-less stream stops 1 GiB short of a full disk; a folder zip is checked by Termurion's
own parser (the 11d Scripts import shares it) and extracted by that parser, each file exactly its
claimed size (13y1 round 3: no tar.exe); in a pane with a framing conductor pane text never opens, runs or reveals on this PC
and its remote folder is never saved into an arrangement; an upload alert closed is Cancel and
Replace is never remembered; Download Files writes into a fresh `Termurion-<id>` folder; no transfers
list or Stop (deferred, ruling 14d-2), no Favorites, no "Download using SSH Integration" item (ruling
14d-5); Alt-drop and Win-click downloads work over the conductor only (no scp).
