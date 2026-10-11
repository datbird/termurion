# Milestone 11d acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: turn on Settings > General > Python API > "Enable Python API" and accept the warning. Run
the checklist twice: once as a user with Python 3 installed (the py launcher or `python.exe` on
PATH), and once on a machine or user with no Python at all, where Tandem's own uv is the only
source. Start with no `%APPDATA%\Tandem\uv` folder and an empty `%APPDATA%\Tandem\Scripts`.
Provisioning installs the newest `iterm2` and `certifi` unpinned; if a script fails with a
TypeError about `extra_headers` (the m11a to m11c checklists pinned `websockets<14` by hand),
record it as a finding rather than pinning. The Script Console is Scripts > Manage > Console
(Win+Alt+J). "Explorer" below is the file window Tandem opens with the item selected.

## Runtime and downloads

| # | Check | Result |
|---|---|---|
| 1 | With no Python and no uv, Scripts > Manage > Install Python Runtime asks "Download Python Support?" whose text names uv 0.12.19 (about 17 MB) from github.com/astral-sh/uv, a Python interpreter from github.com/astral-sh/python-build-standalone, and "these Python packages from pypi.org: iterm2, certifi". | |
| 2 | Cancel (and closing the dialog) downloads nothing: no `%APPDATA%\Tandem\uv` folder appears and nothing is logged. | |
| 3 | OK shows the "Setting up the Python environment..." banner, logs uv's lines in the Script Console's "Tandem App" entry, and ends with `%APPDATA%\Tandem\uv\bin\uv.exe` and `uv\venvs\3.12` holding `.venv`-style `Scripts\python.exe` and `python-runtime.json`. The item now reads "Check for Updated Runtime". | |
| 4 | Check for Updated Runtime asks "Check for Updated Runtime?" (naming pypi.org and github.com/astral-sh/uv) before anything runs; Cancel does nothing; OK ends with a "Python Runtime" alert "uv 0.12.19 is up to date." (or "Upgraded uv ... to ..."), the same line in "Tandem App". | |
| 5 | Scripts > Manage > Install Already-Downloaded Python Runtime is disabled. | |
| 6 | On the machine with Python 3 installed and no uv, running a new basic script (item 10) asks "Download Python Packages?" naming pypi.org and `iterm2, certifi` (no uv question), builds the shared environment with that Python's `-m venv` and `-m pip`, and runs the script. | |
| 7 | Deleting `%APPDATA%\Tandem\uv\venvs\3.12\Scripts\python.exe` and then picking a basic script asks again before rebuilding; nothing downloads without that answer. | |

## New Python Script and running

| # | Check | Result |
|---|---|---|
| 8 | Scripts > Manage > New Python Script opens the picker: "Choose a Python environment:" with Basic and Full Environment (Segoe Fluent Icons glyphs, no emoji), then "Choose a script template:" with Simple and Long-Running Daemon; Cancel, Escape and closing the window make nothing. | |
| 9 | The save dialog opens in the Scripts folder. A place outside it gets "Invalid Folder" and the dialog again; a name with a space gets "Spaces Not Allowed" with "Use _ Instead of Space" and "Change Name". | |
| 10 | Basic + Simple saved as `hello.py`: "Open new script in <editor>?" names the app of the `.py` "edit" verb; OK opens it for editing (it does not run); "Show in Explorer" reveals it. The file is iTerm2's template with "Tandem" in place of "iTerm2" and a `#!/usr/bin/env python3` shebang. With no edit association the script is revealed instead. | |
| 11 | `hello.py` appears in the Scripts menu without a restart (within about half a second of the save). Picking it runs it: while it runs the item is checked, and its Script Console entry shows the command line (the python path, the script, no cookie), the output, and "Connection accepted: Script launched by user action". | |
| 12 | A Basic + Long-Running Daemon script keeps running (checked); picking its item again terminates it (the log gains "*Terminate button pressed*" and ends "** Script was killed by a signal **"), posts no notification, and the check clears. | |
| 13 | While the daemon runs, typing `printf "\033]1337;Custom=id=%s:%s\a" "shared-secret" "create-window"` in a Git Bash or WSL pane opens a new window. | |
| 14 | Alt held while picking a script's menu item reveals it in Explorer instead of running it. | |
| 15 | A script that raises (`raise Exception("x")` after connecting) shows a "Script Failed" notification: "<name>" ended unexpectedly. Its log ends "** Script exited with status 1 **". Clicking the notification opens the Script Console with that entry selected, Logs shown and scrolled to the end. A script that ends with status 0, or one you terminated, posts nothing. | |
| 16 | A daemon that starts a background helper (for example `subprocess.Popen(["ping", "-n", "300", "127.0.0.1"])`) and then exits normally leaves the helper running; quitting Tandem ends it (Task Manager shows no PING.EXE). | |
| 17 | Full Environment + Simple with "requests" in "PyPI Dependencies:" and the default Python version: "Download Python Packages?" names pypi.org and `requests, iterm2, certifi` (plus python-build-standalone when a Python must be fetched); OK builds `<Scripts>\name\.venv`, `python-runtime.json` and `setup.cfg` listing requests, then asks to open `name\name\name.py`. The script can `import requests` and runs from the menu. Its item is never checked while it runs, and picking it again starts a second copy (iTerm2's quirk). | |
| 18 | Deleting a full environment's `.venv` and picking it asks "Rebuild Python Environment?" (Rebuild, Cancel), then the download consent; Rebuild rebuilds and relaunches. An empty plain folder in the menu path (for example one made inside AutoLaunch) shows "Cannot Run Script" / "The script ... is malformed." with OK and Reveal. | |

## Script Console and its reveal points

| # | Check | Result |
|---|---|---|
| 19 | Win+Alt+J opens "Script Console": Name and Started columns (stopped entries in parentheses), All and Running, the Logs and Calls tabs, Filter, "Scroll to bottom on update", and the Inspector, Terminate, Restart and Clear Terminated buttons with their tooltips. | |
| 20 | Logs lines start with a "M/d HH:mm:ss.SSS" stamp; output without a final newline continues on the next chunk's line. Calls lists every request and reply of a connected script as "Script -> Tandem" and "Script <- Tandem" blocks (the real arrows), newest 100 kept. | |
| 21 | Filter "conn" matches "Connection accepted" (lowercase filter ignores case); "Conn" matches it but "CONN" does not (a filter with an uppercase letter is case-sensitive). | |
| 22 | Running hides stopped entries. Terminate is enabled only for a running script with one process and ends it; Restart relaunches the selected script; Clear Terminated removes every stopped entry. | |
| 23 | Inspector opens "Scripting Inspector": "Sessions, Tabs, and Windows" (root "Tandem", Buried Sessions when any), "Variables" (Path, Value) for the selected object with Copy Value and Copy Path, Reveal and Copy ID on the outline, and "Registered Functions" (Signature, Role, Script) listing a running script's `@iterm2.RPC` function. | |
| 24 | A script's RPC function that raises, called from a key binding (Invoke Script Function...), shows the Function Call Failed alert with a second button "Reveal in Script Console", which opens the console on that script's entry. | |
| 25 | A `StatusBarComponent` script added to a status bar, then stopped (Terminate): its function error click shows "Status Bar Script Error" whose text ends "It looks like the script is not running. Launching it might fix the problem." with Launch Script (starts it again) and Reveal in Explorer. | |
| 26 | Turning "Enable Python API" off stops every running script (logs end "** Script was killed by a signal **") and every entry, "Tandem App" and "Automatic Profile Switching" included, reads stopped; after Clear Terminated those two entries are gone until Tandem restarts (iTerm2's quirk). | |

## AutoLaunch, folder and menu requests

| # | Check | Result |
|---|---|---|
| 27 | Put a basic script in `<Scripts>\AutoLaunch`. With the API on, it starts at the next launch (an entry in the console) and nothing is downloaded (with its environment missing, "Tandem App" logs "The Python environment for ... is missing ..." instead). With the API off it does nothing at launch. A startup window still opens (the Windows rule; see the rulings). | |
| 28 | Several AutoLaunch entries start folders first, then by name ignoring case; `__pycache__` and hidden entries are skipped. | |
| 29 | Settings > General > Python API > "Custom Python API Scripts Folder": turning it on with an empty text opens the folder chooser; the text field and Browse follow the checkbox; the glyph is a check mark for an existing space-free folder, else a warning sign (dimmed when the box is off). | |
| 30 | A valid custom folder takes over the Scripts menu when editing ends (Tab out, Browse, or closing Settings), not on each keystroke. A folder with a space, a missing folder, and a network path (`\\server\share`) are all invalid (the default folder is used; the network path is never touched). A local junction (`mklink /J`) pointing at a folder is accepted and scanned. | |
| 31 | Scripts > Manage > Reveal Scripts in Explorer (the menu reads "Explorer", not "Finder") makes the Scripts folder when missing and selects it in Explorer. | |
| 32 | `await iterm2.MainMenu.async_select_menu_item(connection, "<full path of hello.py>")` from another script launches `hello.py`; with `"/Reveal/<full path>"` it reveals it. | |
| 33 | View > Open Quickly with "hello" lists it as a script ("Run Script") and again as "Menu item under Scripts > hello.py", plus its "Reveal hello.py" alternate (iTerm2 lists all three); picking the script row launches it. "/s" restricts to scripts. | |
| 34 | Settings > Advanced > "Write Automatic Profile Switching debug logs" on (`NoSyncEnableAutomaticProfileSwitchingLogging`): the console's "Automatic Profile Switching" entry is first at start and lists each decision as "[<session title>] <line>"; no `automatic-profile-switching.log` is written. | |

## Open Python REPL

| # | Check | Result |
|---|---|---|
| 35 | Scripts > Manage > Open Python REPL (API on) opens a window with the default profile running `python -m asyncio`, showing iTerm2's banner (bold heading, "Win-click here for Tandem Python API Docs" as a link) above the asyncio prompt, not pushed into scrollback by ConPTY's first output. The console gains a "REPL" entry. | |
| 36 | In it, `import iterm2`, `connection = await iterm2.Connection.async_create()` and `await iterm2.Window.async_create(connection)` open a new window. | |
| 37 | `exit()` leaves the REPL pane open with the session ended (it does not close by itself); closing it removes its cookie. With the API off, Open Python REPL asks "Enable Python API?" first. | |

## Import and Export

| # | Check | Result |
|---|---|---|
| 38 | Scripts > Manage > Export... opens "Export Script": folders as headings (AutoLaunch too), a checkbox per script, "Offer to launch automatically during installation" (off), OK disabled with nothing chosen, Cancel and Escape close. | |
| 39 | Exporting `hello.py` writes `hello.zip` on the Desktop and reveals it; exporting again writes `hello (2).zip`. The zip holds `hello/setup.cfg` and `hello/hello/hello.py` (the script byte for byte) and no `metadata.json`. A full environment exports `setup.cfg` and its inner folder only (no `.venv`). A failure shows "Export Failed". | |
| 40 | Scripts > Manage > Import... of `hello.zip` while `hello` exists asks "Script Already Exists" / A script named "hello" is already installed (Replace Script, Cancel). Cancel keeps the old script. Replace installs it (a full environment is built after the download consent), then "Script Imported Successfully" with OK and Launch; Launch runs it. | |
| 41 | Replace of a full-environment script that is running terminates it first; when a helper it left running holds its files, the import ends with the replace-failed text and the old script is left unchanged. Declining the download consent during a Replace restores the old script ("Replacing ... was canceled. The existing script was kept."). | |
| 42 | Import of a `.py` copies it into the Scripts folder (an existing name fails with Windows' "file exists" text). Importing two zips at once runs them one after the other, never "Another import is in progress". | |
| 43 | Import of a `.its` file that is not a signed archive shows "Could Not Install Script" with "This script archive is corrupt and cannot be installed." and Try Again (13y2 Task 4; a signed archive asks "Confirm Installation" instead). | |
| 44 | A zip holding `../x.py`, an absolute path, `CON`, or a symbolic link entry is refused ("The archive is not safe to extract: ...") and nothing is written beside the Scripts folder. A zip with no valid layout gives "Archive does not contain a valid Tandem script". | |
| 45 | An `.its` file dropped into the Scripts folder is offered at the next scan: "Install Script Archive?" (OK, Cancel, Move to Trash); OK leads to the `.its` check of item 43; Move to Trash puts the file in the Recycle Bin. Asked once per file per run. | |

## Manage Dependencies

| # | Check | Result |
|---|---|---|
| 46 | Scripts > Manage > Manage Dependencies... opens "Script Dependency Editor" with a "Script:" popup listing every script as "folder/name". A full environment shows "Python Version:", the Package Name / Installed Version table (versions from `pip show`, "[Error]" when unreadable), Add, Remove and Check for Update. | |
| 47 | Add asks "Add Dependency" / "What dependency would you like to add?"; `--index-url=x` or `pkg @ https://...` is refused and the question stays open. `rich` shows "Run this Command?" with the uv (or pip) command line and "Packages come from pypi.org."; OK runs it in a window that stays open with pip's output; afterwards `rich` is in the table and in `setup.cfg`. Only one pip command runs at a time; the editor's controls are off meanwhile. | |
| 48 | Check for Update on `rich` runs the upgrade command through "Run this Command?" and re-pins `setup.cfg` to `rich>=<version>`. Remove uninstalls it through the same question and drops it from `setup.cfg`; a failed remove shows "Removal Failed". | |
| 49 | Changing "Python Version:" to an older version asks "Confirm Python Downgrade", to a newer one "Confirm Python Upgrade"; OK asks the download consent, the banner reads "Rebuilding the Python environment...", and `setup.cfg` and `python-runtime.json` show the new version. With a helper of that script still running, the rebuild fails with "Could Not Change Python Version" and the old environment is kept. | |
| 50 | A basic script shows the upgrade text (with iTerm2's mismatched closing quote) and "Upgrade"; Upgrade makes `<name>\setup.cfg`, `<name>\.venv` and `<name>\<name>\<name>.py` (the script moved in) after the consent, and the editor selects the new folder. With a folder of that name already present it shows "Error" / "Can't upgrade because ... already exists". | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 51 | A full-environment script's menu item is never checked while it runs, and picking it launches another copy (iTerm2 compares the entry's main .py with the item's folder). | |
| 52 | "Offer to launch automatically during installation" has no effect on an unsigned export: its value goes into `metadata.json`, which only a signed archive carries. | |
| 53 | "Use _ Instead of Space" does not check the renamed path, so an existing file or folder of that name is overwritten or reused, as in iTerm2. | |
| 54 | After the API is turned off and Clear Terminated is pressed, "Tandem App" and "Automatic Profile Switching" lines go unseen until Tandem restarts. | |
| 55 | Open Quickly lists each script twice (script item and Scripts menu item) plus the "Reveal <name>" alternate, although Tandem's menu bar shows no alternates. | |
| 56 | Reveal Scripts in Explorer ignores a failure to make the folder and reveals anyway. | |
| 57 | The Scripts menu has no "Reveal <name>" alternate items (Windows menus have none); Alt held at the pick reveals. | |
| 58 | AutoLaunch does not suppress the startup window (Tandem opens one when nothing else would show), unlike iTerm2. | |
| 59 | Scripts end when Tandem quits, helpers included (each runs in a Job Object killed with Tandem); terminated scripts log "Script was killed by a signal". | |
| 60 | Only `.py` files (lowercase extension) and full-environment folders are scripts; `A.PY`, `.scpt` and `.app` are not listed, and nothing that is not a script is ever opened. | |
| 61 | Signed `.its` archives from iTerm2 users cannot be installed; ask for an unsigned `.zip` export. | |
| 62 | New scripts' templates always use `#!/usr/bin/env python3` (iTerm2 under uv substitutes "3" too). | |
| 63 | The picker asks a Full Environment's dependencies and Python version before the save dialog (Windows save dialogs take no accessory view). | |
| 64 | Console times read "M/d HH:mm:ss.SSS" (Started "M/d HH:mm:ss"), not the locale's format; Calls show Rust's pretty debug text of each message (cut at 16 KiB), not protobuf text format; a log line longer than 16 KiB is broken into stamped pieces. | |
| 65 | The Script Console has no find bar: its text is selectable, and the Filter narrows lines. The Inspector is not live: it refreshes when opened or when the console's Inspector button is pressed. | |
| 66 | The pip window of Manage Dependencies inherits Tandem's environment, so a person's own `PIP_*` or `UV_*` variables (for example `PIP_INDEX_URL`) apply to the command shown; provisioning runs strip them. A `pip.ini` is honored everywhere. | |
| 67 | A mapped drive letter that points at a share is not detected as a network path (only UNC paths and links reaching them are refused). | |
| 68 | Picker boxes, console buttons and Inspector rows use Segoe Fluent Icons glyphs instead of iTerm2's emoji and SF Symbols; without the icon font they show text. | |
