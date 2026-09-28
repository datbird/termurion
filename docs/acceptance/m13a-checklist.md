# Milestone 13a acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: start from fresh settings (rename `%APPDATA%\Tandem` aside first, and put it back after the
erase checks). Have an iTerm2 settings plist at hand whose globals hold `Control` = 1 and a legacy
single hotkey window (`Hotkey` and `HotKeyTogglesWindow` true, `HotKeyBookmark` naming one of its
profiles' Guid). For the URL checks, Save Now to a folder first, commit that
`tandem-settings.toml` to a GitHub repository and use its raw URL
(`https://raw.githubusercontent.com/...`): a `python -m http.server` on sirtoolio is not reachable
from EXTREME. Install DebugView (Sysinternals) for the dry run. "Explorer" below is the file window
Tandem opens with the item selected. Mark each line pass or fail.

## The Advanced pane

| # | Check | Result |
|---|---|---|
| 1 | Settings > Advanced shows a search field (hint "Search"), "Show only non-default values" (its tooltip "When checked, removes items from the list whose value equals the default value.") and the columns "Setting" and "Value". The categories run alphabetically (Drawing, Experimental Features, General, Hotkey, Pasteboard, Scripting, Session, Tabs, Terminal, Tmux Integration, Warnings, Windows), each a bold header across the whole row; scrolling keeps the current header pinned at the top until the next one pushes it up. Each row shows the description's first line, and the rest in smaller gray text under it. | |
| 2 | Type "tmux titles": only the Tmux Integration header and "Insert this string at the start of tab and window titles to indicate tmux integration." remain. Type "tmux prefix": nothing is listed (iTerm2 searches the description and category, never the key name). | |
| 3 | Type "pasteboard": the whole Pasteboard category is listed (the word matches the category). Clear the field: every row is back. | |
| 4 | Turn on "Show only non-default values" on fresh settings: the list is empty. Change "Timeout in seconds when running git for the status bar component." to 5: the row stays while you type; with the filter still on, it is listed under General once the field loses focus. | |
| 5 | With the filter on, type 4 back into that field: the row stays while the field has focus and leaves the list when the field loses focus (4 is the default, and the key leaves `settings.toml`). | |
| 6 | A No/Yes row (for example "Disable control sequence to open URLs?") offers No and Yes; Yes writes `NoSyncDisableOpenURL = true` to `%APPDATA%\Tandem\settings.toml`, No removes it. | |
| 7 | "Prevent control sequences from changing the current profile?" offers Unspecified, No and Yes. Yes stores true, No stores false, Unspecified removes the key. With it on Unspecified, `printf '\e]1337;SetProfile=Default\a'` in a WSL or Git Bash pane asks again every time; answering the question with a remembered choice shows No or Yes on the row. | |
| 8 | The enum row "Where should new tmux windows not created by Tandem open?" is a popup of New window, Focused window, Topmost session window; picking New window stores 0, picking Focused window (the default) removes the key. | |
| 9 | An integer row ("Number of autocomplete options to present."): typing "12abc" stores 12 and the field shows 12 when it loses focus; "abc" stores 0; " -3" stores -3. | |
| 10 | A float row ("Timeout in seconds when running git for the status bar component."): "1e1" becomes 10 and ".5" becomes 0.5 when the field loses focus; "x" becomes 0. | |
| 11 | A string row ("Insert this string at the start of tab and window titles...") stores the text as typed, spaces included; typing the default back removes the key. | |
| 12 | Hover a value: the tooltip is the whole description. | |

## Indicators and Experimental

| # | Check | Result |
|---|---|---|
| 13 | The search glyph at the right of the Settings page bar opens "Search Options" with "Show indicators for non-default values" (off at first). Turn it on: the item is checked, and `NoSyncHideDefaultValuedSettings = true` is in `settings.toml`. | |
| 14 | With indicators on, change a checkbox on General: a thin bar in the accent color appears at its left; set it back: the bar goes. A disabled control never shows a bar. Keys > Leader and a profile's Keys > "Delete sends ^H" show a bar once changed. | |
| 15 | With indicators on, a profile renamed from "Default" shows a bar on its Name; a new profile shows none on its fonts, colors or working directory. | |
| 16 | General > Experimental shows "Enable support for right-to-left scripts" (checked on fresh settings) with the note "This setting also enables ligature support in the GPU renderer." under it, and "Use SSH integration for ssh: URLs" (unchecked). Neither changes anything yet (13m and milestone 14). | |
| 17 | Profiles > Import from iTerm2... with the setup plist: Keys > Remap Modifiers shows Right Control for both "Remap left Control key to" and "Remap right Control key to" (from `Control` = 1), the profile `HotKeyBookmark` names now has its hotkey (the plist's key and modifiers, auto-hide from `HotkeyAutoHides`), and Keys > Hotkey ("Show/hide all windows with a system-wide hotkey") is cleared. A plist whose `HotkeyMigratedFromSingleToMulti` is true moves no hotkey (only `Control` applies); after any plist import Tandem's `settings.toml` holds the flag. | |

## Load settings from a custom folder or URL

| # | Check | Result |
|---|---|---|
| 18 | General > Settings, "External settings:": "Load settings from a custom folder or URL" is unchecked; the field (placeholder "Enter path to folder with settings") and Browse are disabled; "Save changes:", its popup and Save Now are disabled. | |
| 19 | Check the box with the field empty: the folder chooser opens. Pick a new empty folder: "Copy local settings to custom folder now?" (Copy, Don't Copy) asks; Copy writes `<folder>\tandem-settings.toml` holding `default`, `[[profile]]` and `[settings]`. The field shows the folder and a check mark; "Save changes:" and Save Now are enabled. | |
| 20 | Type `\\server\share` (or any UNC path): the glyph is the warning, "Save changes:" and Save Now are disabled, and nothing is read or written there. A folder that does not exist also shows the warning. Uncheck the box: the glyph fades to half. | |
| 21 | Set "Save changes:" to Automatically and change any setting: `tandem-settings.toml` is rewritten within a second. | |
| 22 | Set it to Manually, change a setting and Save Now: the file is rewritten. Edit the file by hand in Notepad, then Save Now: "Settings at ... changed since Tandem started. Overwrite it?" (Overwrite, Discard Local Changes). | |
| 23 | Still on Manually, change a setting and quit: "Settings have changed. Copy them to <folder>?" (Copy, Lose Changes, and a "Do not ask again" box). Copy writes the file and Tandem exits. | |
| 24 | Set the popup to When Quitting, change a setting and quit: the file is written without asking (the popup is the question's remembered answer). Set it to Manually: the next quit after a change loses the changes without asking. | |
| 25 | Quit, delete `tandem-settings.toml`, start Tandem: after the first window opens, "Failed to load settings from custom directory. Falling back to local copy." / "Missing or malformed file at "<folder>"" (OK, Reveal in Settings); Tandem runs on its local settings, and Reveal in Settings opens General. | |
| 26 | Put the file back, change `default` or a setting in it by hand and start Tandem: the file's values are in effect. | |
| 27 | Type the GitHub raw URL: the check mark shows; "Save changes:" and Save Now are disabled (a URL is never written). Quit and start: the URL's settings are in effect (fetched with `curl.exe`). | |
| 28 | Set "Timeout for downloading settings" (Advanced, General) to 0.001 and start Tandem: "Failed to load settings from URL. Falling back to local copy." with "HTTP request failed: ..." and the buttons OK, Reveal in Settings and "Try Again Without Timeout"; Try Again loads the URL. Set the timeout back. | |
| 29 | With the URL loaded, change a setting and quit (no question). Start again: "Load settings from URL? Some changes were made to the local copy that will be lost." (Keep Local Changes, Disable Loading from URL, Discard Local Changes); Keep keeps the change, Discard applies the URL's settings, Disable turns the checkbox off. | |
| 30 | Clear the field while the box is checked and restart: "Error Loading Settings" / "You have enabled 'Load settings from a custom folder or URL' in settings but the location is not set." (Don't Load Remote Settings, Cancel). | |
| 31 | Advanced > Warnings: turning on "Suppress changed-setting warning when settings are loaded from a custom folder." sets the Save changes popup's remembered answer (the same key); the "...from a URL." row is stored and does nothing. | |

## Export, import and erase

| # | Check | Result |
|---|---|---|
| 32 | "Backup & restore:" > Export All Settings and Data... opens a save dialog "Export Tandem Settings and Data" named `Tandem State.tandemexport`; saving writes it and Explorer opens with it selected. Turn off "Reveal file created by 'Export All Settings and Data' in Explorer upon completion" (Advanced, General) and export again: no Explorer window. | |
| 33 | `tar -tzf "Tandem State.tandemexport"` lists `python-runtimes\PythonRuntimes.json`, `user-defaults\UserDefaults.toml`, `app-support\...` (no `settings.toml`, `profiles.toml`, `python-api-consent`, `host.log`) and `scripts\script <UUID>.zip` or `autolaunch <UUID>.zip` for each script. | |
| 34 | Change a setting, a profile, a snippet and delete a script, then Import All Settings and Data... with that export: "Importing Settings and Data" (OK, Cancel); OK quits Tandem and it starts again by itself. The exported settings, profiles, snippets and scripts are back (AutoLaunch scripts in AutoLaunch), and `%TEMP%` holds no `.tandem-import-*` folder afterwards. The Python API consent is not carried over: the API stays off until "Enable Python API?" is answered OK again. | |
| 35 | Turn on "Dry-run 'Erase All Settings and Data'." (Advanced, General). "Reset:" > Erase All Settings and Data... asks "Would you like to export your settings and data first?..." (Export First, Skip Export, Cancel). Skip Export, then "Dry-Run Erase?" (Cancel, Run Dry Run): Run Dry Run shows "Dry Run Complete"; DebugView shows one "[Erase dry-run] would remove ..." line per entry of `%APPDATA%\Tandem` and of the Scripts folder, and nothing is deleted. | |
| 36 | Export First in the erase question opens the export dialog; canceling it stops the erase. | |
| 37 | Turn the dry run off. Erase All, Skip Export, then "Erase Everything?" lists what goes and says Windows Credential Manager entries are not erased (Cancel, Erase Everything and Quit; Enter cancels). Erase Everything and Quit: Tandem exits at once, `%APPDATA%\Tandem` is empty, and Control Panel > Credential Manager still holds the `Tandem/...` entries. Put the saved folder back. | |
| 38 | With the dry run on, turn on General > Python API > "Custom Python API Scripts Folder" and type a folder with a space in its name (for example `C:\Temp\My Scripts`, holding a file): its glyph shows the warning, and the dry run lists `%APPDATA%\Tandem\Scripts`'s entries, never that folder's (Tandem erases the Scripts folder it uses, as iTerm2's `scriptsPath`). | |
| 39 | Import All with an export rebuilt to hold a hard-linked pair (in a copy of the unpacked export, `fsutil hardlink create app-support\b.txt app-support\a.txt`, then `tar -czf x.tandemexport -C <copy> app-support`): "Problem Importing" names the entry ("is a link or has another name"), Tandem does not restart, and `%TEMP%` holds no `.tandem-import-*` folder. | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 40 | The Advanced texts are iTerm2's, typos included ("becuase", "Requres") and some macOS references, except rows Tandem does differently, which say what Tandem does. | |
| 41 | "Number of entries to save in Paste History." draws an empty small line under it (iTerm2's description ends with a newline). | |
| 42 | A typed float shows Rust's form ("0.1"), where iTerm2 could show 0.100000001. | |
| 43 | "Show indicators for non-default values" is in a menu at the right of the page bar, not in a settings search field (Tandem has none). | |
| 44 | The right-to-left checkbox's tooltip talks about buried sessions and tmux window sizes (iTerm2's copy-paste slip). | |
| 45 | Twenty keys that are not iTerm2 Advanced settings (Advanced Paste memories, password manager defaults, warning silences, the APS log toggle, the Broadcast Input pattern toggle) are on no pane; those without a control elsewhere change only by editing `settings.toml`. | |
| 46 | "Folder for config files." (PreferredBaseDir) is stored and moves nothing. | |
| 47 | A legacy dynamic hotkey profile is not migrated by the plist import; set its hotkey by hand. | |
| 48 | `tandem-settings.toml` is not interchangeable with iTerm2's plist; a named file must end in `.toml`; `https://` with no host shows the warning glyph. | |
| 49 | Snippets, global notes and icon customizations stay on each machine (not synced through the folder). | |
| 50 | A settings folder on a network share must be mapped to a drive letter. | |
| 51 | The URL question and load failures appear after the first window opens; that window starts with the local settings. | |
| 52 | In Automatically mode, typing a new folder path writes the file into whatever partial path exists half a second after the first keystroke, as in iTerm2. | |
| 53 | While the quit question is open, terminal windows ignore input and Quit. | |
| 54 | A large export freezes the windows until tar finishes (they may show Not Responding), as iTerm2's modal export does. | |
| 55 | Links and junctions inside `%APPDATA%\Tandem` are not exported. | |
| 56 | Import restarts Tandem once (iTerm2 twice); a Tandem started by hand during the restart runs a second copy. Imported AutoLaunch scripts first run at the next start. The settings folder's quit question may ask before the restart. | |
| 57 | The erase dry run's lines need DebugView; a running Python script's files may survive the erase (removal errors are ignored, as iTerm2's). | |
