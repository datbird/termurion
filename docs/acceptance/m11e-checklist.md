# Milestone 11e acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`
once. Check that a `utilities` folder now sits beside `tandem.exe` holding 16 programs (`imgcat.exe`,
`imgls.exe`, `it2api.exe`, `it2attention.exe`, `it2cat.exe`, `it2check.exe`, `it2copy.exe`,
`it2dl.exe`, `it2getvar.exe`, `it2git.exe`, `it2profile.exe`, `it2setcolor.exe`,
`it2setkeylabel.exe`, `it2tip.exe`, `it2ul.exe`, `it2universion.exe`) and `it2api.py`. Quit Termurion,
end `tandem-host.exe` (Task Manager), and start Termurion again, so new sessions get the new
environment.

Setup: have a small picture `pic.png` and a text `README.md` in a scratch folder such as `C:\tmp`,
a folder `C:\tmp\up` to upload into, and two sibling folders and one file elsewhere to upload.
Have a git repository with a local change. Have a second profile named `Other`. Have WSL with bash
(or Git Bash) for the bash-script checks. For the it2api checks, install Python 3 and run
`py -3 -m pip install --user iterm2 "websockets<14"` (Termurion never installs it; the `websockets<14`
pin avoids the `extra_headers` TypeError noted in the m11a checklist), and turn on Settings >
General > Python API > "Enable Python API" and `setCookie`. Run every check in a new PowerShell
session unless it says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `$env:PATH` ends with `...\utilities` (the folder beside `tandem.exe`); `$env:TERM_PROGRAM` is `iTerm.app`, `$env:TERM_PROGRAM_VERSION` `3.7.0`, `$env:COLORTERM` `truecolor`, `$env:LC_TERMINAL` `iTerm2`, `$env:LC_TERMINAL_VERSION` `3.7.0`, and `$env:TERM_FEATURES` is set. | |
| 2 | `it2check; $LASTEXITCODE` prints 0 (Termurion answers `ITERM2 3.7.0`). `it2check 9.0; $LASTEXITCODE` prints 1. | |
| 3 | `imgcat pic.png` shows the picture; `imgcat -W 50% pic.png` shows it at half the pane's width; `cmd /c "imgcat < pic.png"` shows it from stdin; `imgcat -u https://iterm2.com/img/logo2x.jpg` shows iTerm2's logo (curl.exe fetches it). | |
| 4 | `imgcat` with no arguments prints its help and returns 0; `imgcat nosuch.png; $LASTEXITCODE` prints "imgcat: nosuch.png: No such file or directory" and 2; the help says "Termurion", never "iTerm2". | |
| 5 | `imgls` in `C:\tmp` lists the folder with a thumbnail before each image; `imgls -l` adds a long listing with Windows owner and group names; `imgls --dimensions` adds each image's size in pixels. | |
| 6 | `it2cat README.md` renders the file with Termurion's text document view. | |
| 7 | `it2dl README.md` offers the download `README.md`, and `it2dl C:\tmp\up` offers `up.tar.gz`; accepting saves each to the Downloads folder, and the archive opens to the folder's files. | |
| 8 | `"hi" \| it2copy`, then paste in any program: the clipboard holds "hi" and a line break. `it2copy README.md` copies the file's text. | |
| 9 | Run `it2copy` with no argument (it waits on stdin), type a few characters and press Ctrl+C: it2copy ends and the pane shows no stray sequence or stuck state (the pending ST closed the OSC). | |
| 10 | `it2setcolor bg 002` turns the background dark blue; `it2setcolor tab f00` gives the tab a red color; `it2setcolor tab default` removes it. | |
| 11 | `it2attention start`, then switch to another window within a second or two: Termurion's taskbar button flashes until Termurion is focused again; `it2attention stop` stops it; `it2attention once` (switch away again) flashes once; `it2attention fireworks` shows fireworks at the cursor. | |
| 12 | `it2universion set 8`, `it2universion push`, `it2universion set 9`, `it2universion pop` and `it2universion push work`, `it2universion pop work` each return 0 and print nothing. | |
| 13 | In the git repository, run `it2git`, then `it2getvar user.gitBranch`: the first time, Termurion answers with an empty value and shows the bar reading A request to report variable user.gitBranch was denied. Allow it in the future? (the name in curly quotes); choose Always Allow and run it again: it prints the branch. Each name needs its own Always Allow: `it2getvar user.gitDirty` first prints an empty line and shows its own bar; after Always Allow it prints `dirty`. Outside a repository `it2git` clears them (`it2getvar user.gitBranch` prints an empty line). | |
| 14 | After Always Allow for each name (as in check 13), `it2getvar session.name` prints the session's name, and `it2getvar user.nosuch` prints an empty line and returns 0. `it2getvar -x` prints "ERROR: Unknown option: -x" and the help, and returns 1. | |
| 15 | `it2profile -g` prints the profile name (after Always Allow for `profileName`, as in check 13); `it2profile -s Other` shows Termurion's SetProfile bar and, once allowed, the session switches to `Other`; `it2profile -r` returns it to the profile it started with. | |
| 16 | `it2ul C:\tmp\up` shows the bar "Upload files or folders?" with Files and Folders; choose Files, pick one file: `it2ul` extracts it into `C:\tmp\up` and ends. | |
| 17 | `it2ul C:\tmp\up`, choose Folders, pick two sibling folders: both extract into `C:\tmp\up` with their own names and contents. | |
| 18 | `it2ul`, then press any key at the Files/Folders bar: the bar goes away, `it2ul` ends at once ("Upload aborted" is not required; the upload is cancelled with ^C) and nothing is typed into the shell. | |
| 19 | In a new session, run `it2getvar user.q1`, `it2getvar user.q2`, `it2getvar user.q3` and `it2getvar user.q4` (names never allowed, so each returns at once and leaves its own bar waiting; do not answer them). With those four bars waiting, `it2ul` ends at once instead of waiting on a bar nobody sees. | |
| 20 | Upload a 5 MB file with `accelerateUploads` on (default), then turn Settings > Advanced > "Make uploads with it2ul really fast." off and upload it again: the second is visibly slower; with `QuickPasteBytesPerCall` lowered as well, it slows further (the quick-paste pace). | |
| 21 | `it2tip` prints "Termurion tip - <title>:", the tip, and a "Learn more:" link that opens on Win-click; running it again prints the next tip. | |
| 22 | `it2setkeylabel set F1 Help; $LASTEXITCODE` prints 0 and nothing visible changes (Termurion keeps the label and shows it nowhere). | |
| 23 | `it2api list-sessions` in a new session (after `setCookie` is on) lists Termurion's windows, tabs and sessions with their ids; `it2api send-text <id from the list> hi` types "hi" into that session (SendText has been built since milestone 11b). | |
| 24 | With no Python on PATH (a session started with a PATH holding only `C:\Windows\System32`, `%LOCALAPPDATA%\Microsoft\WindowsApps` and the utilities folder), `it2api list-sessions` prints "it2api needs Python 3. Install it, then run:" and returns 1; the WindowsApps `python.exe` alias is never started (no Store window opens). | |
| 25 | Turn off Settings > Advanced > "Add path to Termurion utilities to $PATH for new sessions?": a new session's PATH lacks the utilities folder. Turn it back on. | |
| 26 | Turn off Settings > Advanced > "Set LC_TERMINAL=iTerm2.": a new session has no `LC_TERMINAL` or `LC_TERMINAL_VERSION`, but still has `TERM_PROGRAM`. Turn it back on. | |
| 27 | In a WSL bash tab, iTerm2 > Install Shell Integration shows a Yes/No/Cancel box offering iTerm2 Utilities; Yes types `curl -L https://iterm2.com/shell_integration/install_shell_integration_and_utilities.sh \| bash`; run it again after answering No: No is now the default button (the choice is remembered in NoSyncInstallUtilities). In a PowerShell tab the box offers no utilities choice. | |
| 28 | After the installer in WSL, iTerm2's bash scripts still work there: `~/.iterm2/imgcat pic.png` shows the picture. `~/.iterm2/it2check; echo $?` prints 1 (kept item 49; measured). | |
| 29 | In PowerShell, `$env:TERM = "tmux-256color"; imgcat pic.png` still shows the picture (the utility wraps for tmux and Termurion reads the wrapped form); `$env:TERM = "screen"; it2copy README.md` still fills the clipboard. | |
| 30 | Run `it2ul`, and while the Files/Folders bar waits, end `it2ul.exe` in Task Manager (a hard kill, so the console input mode is not put back): if the shell then echoes oddly or ignores keys, Session > Reset restores it. Dismiss the bar afterwards. | |
| 31 | Start `utilities\it2copy.exe` in a plain console outside Termurion (it waits on stdin) and leave it running. Close Termurion, **delete** `tandem-utils.exe` (Windows allows deleting a running program's name; copying over it fails because `it2copy.exe` is a hard link to the same running file), copy the newer `tandem-utils.exe` in, then start Termurion: every `utilities\*.exe` now has the new size, and no banner shows. If a name could not be refreshed, the banner "Some utilities could not be refreshed (...). Close any running utility and restart Termurion." names it. End `it2copy.exe` with Ctrl+C afterwards. | |
| 32 | Robert, from a non-elevated console on EXTREME (a normal, not administrator, PowerShell), in `C:\Users\datbi\src\termurion-11e` (or the main checkout once merged): `cargo test --lib refresh_failures_are_reported -- --nocapture` passes and does not print "SeRestorePrivilege is enabled", so the read-only-folder case is observed at least once. | |
| 33 | Robert, once: `py -3 -m venv C:\Users\datbi\src\termurion\target\api-venv`, then `C:\Users\datbi\src\termurion\target\api-venv\Scripts\python.exe -m pip install iterm2 "websockets<14"`, then from sirtoolio `scripts/xt test --lib it2api_lists -- --ignored --nocapture` in the 11e worktree: the test passes without the "skipped:" line, and its MEASURED lines show `id=1`, `id=2`, `id=3`, `id=4`, `id=7` and `id=8` from list-sessions and "Key window: pty-A" from show-focus. Record the measured output here. | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 34 | Termurion reports itself as iTerm2 3.7.0 to `CSI 1337 n` and in `TERM_PROGRAM`/`LC_TERMINAL`, so programs that detect iTerm2 turn on iTerm2 features, macOS-only ones included; XTVERSION still says "Termurion". | |
| 35 | `it2setcolor preset <name>` does nothing (color presets are milestone 13); the other colors apply. | |
| 36 | `it2universion` changes the version Termurion keeps and reports, but character widths do not change with it. | |
| 37 | `it2setkeylabel` labels are kept and shown nowhere (Windows has no Touch Bar). | |
| 38 | An upload asks Files or Folders first (Windows cannot pick both in one dialog), and any key at that bar cancels the upload (see checks 16 to 18). | |
| 39 | Some `it2tip` tips name Mac keys (the Cmd glyph) or features Termurion does not have; only "iTerm2" is changed to "Termurion". | |
| 40 | `imgls -l` shows Windows account names (no domain) as owner and group, "?" when a name cannot be read, and SIDs with `-n`; `imgls --method sips` finds no method and exits 1 (dimensions come from Termurion's own decoder, method "image"). | |
| 41 | `it2dl` archives any folder under its real on-disk name (a link's target, the name's on-disk case, the long name for an 8.3 short name), while the sent `<name>.tar.gz` keeps the name given, as iTerm2's script does; so `it2dl .` and `it2dl ..` offer `..tar.gz` and `...tar.gz`, but the entries start with the folder's real name, where the script's tar writes `./...` for `.` (and strips `../` for `..`), and a linked or differently spelled folder's entries start with the real name, where the script's tar keeps the given one. | |
| 42 | `it2copy "my notes.txt"` copies the file, where iTerm2's script sends an empty copy (its unquoted redirect). | |
| 43 | Another `imgcat` earlier on PATH wins over Termurion's, since the utilities folder is appended at the end, as iTerm2 does. | |
| 44 | Install Shell Integration shows a three-button box instead of iTerm2's "Also install iTerm2 Utilities" checkbox; the label keeps "iTerm2 Utilities" because the installer downloads iTerm2's scripts. | |
| 45 | `it2api` needs Python 3 with the `iterm2` package; Termurion does not install it, and `it2` (iTerm2's Swift CLI) and `it2ssh` (milestone 14) are not provided. | |
| 46 | A dirty working tree inside a git submodule does not count in `user.gitLinesInserted`/`user.gitLinesDeleted` after `it2git` (a moved submodule commit still does). | |
| 47 | While `it2check` waits for its answer, a real focus change produces nothing: ConPTY consumes `ESC [ I` and delivers no focus record. The Windows reader puts the swallowed `ESC [ I` back only when the reply itself starts `TERM2 `, so the one exposure is a terminal whose answer literally starts `TERM2 `. | |
| 48 | Termurion run from a network folder, or from a folder whose path holds ";", does not put the utilities on PATH and says why in a banner. | |
| 49 | iTerm2's own bash `it2check` fails in WSL (exit 1, measured) and Git Bash (no exit within 20 s, measured; whether Ctrl+C ends it is not measured), and over ssh.exe (inferred from the same ConPTY input, not measured): the pane's ConPTY takes the reply's leading `ESC [ I` for a focus report and drops it, so the script reads the name `RM2`. Termurion's `it2check.exe` restores the prefix and passes (check 2). A ConPTY limitation, kept. | |
