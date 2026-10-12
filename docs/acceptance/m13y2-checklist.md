# Milestone 13y2 acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, `OpenConsole.exe`,
`iterm2-bitwarden-adapter.exe`, `iterm2-keepassxc-adapter.exe` and `iterm2-keeper-adapter.exe`
from `C:\Users\datbi\src\tandem\target\release\` together (the adapters must sit beside
`tandem.exe`), then run `tandem.exe`.

Setup: a window with a vertical split; a second PC or a shared folder for the data sync rows; a
signed `.its` from iTerm2's script gallery and a copy of it with one byte changed in the payload;
the Bitwarden, KeePassXC and Keeper CLIs or service for rows 14 to 16 (`lpass` for rows 12 and 13);
an old `settings.toml` with `disableTopRightIndicators = true` for row 9. Rows 3, 7, 8, 11 to 16,
18 and 22 cover what tests could not execute (a real tmux, CryptoAPI revocation, the password
manager CLIs, 200% DPI, Windows' recent-items setting); do them on a real desktop. Mark each line
pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Session > Make Screenshot of an inactive split pane (click the other pane first): the picture is as bright as the pane would be focused, not dimmed. Both the minimap and the annotated image. | |
| 2 | Open Quickly (Shift+Win+O) with a session of an inactive split pane selected: the preview panel shows it undimmed. | |
| 3 | A tmux pane with the default Title Components shows "↣ <window name>" in its pane title bar and its tab; with the Title Function of a custom provider the prefix is still there once (no double prefix). | |
| 4 | Settings > Profiles > General > Title Function reads "Job" for the default components (2); turn Custom on with no function: it reads "Built-in components"; pick a registered function: its name shows (cleaned, at most 128 characters). | |
| 5 | Import a signed `.its` from iTerm2's script gallery (Scripts > Manage > Import): "Confirm Installation" names the author (and the issuer), Reveal Contents shows the files in Explorer and installs nothing, OK installs. | |
| 6 | The copy with a changed byte is refused with "Signature does not match payload." and nothing is installed or left in a temp folder. | |
| 7 | An `.its` whose script asks for AutoLaunch asks "Allow Auto-Launch?" with Launch Automatically and "Lauch Manually" (iTerm2's spelling); Launch Manually installs outside AutoLaunch. A machine with no network: an archive whose certificate chain is in the Windows store verifies from the cache; a chain needing a download is refused with the chain error (record which you saw). | |
| 8 | Import-time safety: an `.its` over 256 MB or one with a corrupt header is refused with iTerm2's text; nothing runs from the archive before OK. | |
| 9 | An old `settings.toml` with `disableTopRightIndicators = true` still hides the indicators after the rename sweep; after the next save the file holds `DisableTopRightIndicators`, and a lower-case duplicate is gone. | |
| 10 | Edit Session (Session > Edit Session) changes the font of one pane only; the profile and its other panes keep the font. Copy Current Settings to Selected Profile asks "Replace profile ... with the current settings?" before replacing; its default destination is the session's profile. Hovering a preset or a key binding there leaves the saved profile untouched. | |
| 11 | With the settings folder on a second PC's shared folder (Settings > General > Settings), "Sync These Too" asks for consent once, copies snippets, the global note and the icon tables to `<folder>\Data`, and a second machine picks them up. Make both copies differ: the conflict window asks which to keep; closing it with the X keeps the local copy and changes nothing. A failed backup (read-only folder) never pushes stale files over the folder. | |
| 12 | LastPass with an expired session (`lpass logout`): a command asks "Enter your LastPass master password:" once, mid-command; the right password completes it; a wrong password ends with "Incorrect master password." and the next request asks afresh; Cancel ends with "Canceled." | |
| 13 | LastPass: the master password never shows in the debug log (`%TEMP%\debuglog.txt`), a process listing or the Locate result. | |
| 14 | Bitwarden source (Password Manager > source Bitwarden CLI) with `bw` installed: log in with the master password, the accounts list, one copies its password; the master password is not on any command line. | |
| 15 | KeePassXC source with `keepassxc-cli` installed and a database path: the accounts list; no key file prompt exists (ruled). | |
| 16 | Keeper source with Keeper Commander's service on `http://127.0.0.1` or an https URL: the accounts list; an `http://` LAN URL is refused with a fixed text. | |
| 17 | Edit > Set Named Mark adds a "Named Mark <name>" note over the mark's line; Rename Named Mark changes the note's text. | |
| 18 | `UseOldStyleDropDownViews` on (Settings > Advanced): a new window's find bar uses the old layout at 200% DPI scaling without clipping, its previous, next and close buttons work, and the paste progress shows PasteView's wording; an existing window keeps its own style until reopened. A browser pane's find bar takes the old layout too. | |
| 19 | A program that asked for focus reports (`printf '\e[?1004h'`, then `cat -v`) gets `^[[O` and `^[[I` when you move between two split panes, once per change; closing the active pane reports focus-in to the survivor. | |
| 20 | A profile whose command, name or folder holds `$$LOCATION$$` asks "Enter the value for "Location":" in the Session Parameter window; the answer reaches the session; Restart Session reruns with the stored answers (it does not ask again, ruled); `$$$$` becomes `$$`; Cancel opens nothing. | |
| 21 | A URL value typed as an answer is not substituted into a command, name or folder as a URL (ruled); nothing typed shows in the debug log. | |
| 22 | Windows Settings > Personalization > Start > "Show recently opened items in Start, Jump Lists and File Explorer" off: right-click the Tandem taskbar button: the tasks and profiles still show, and no red "Jump List: Access is denied" banner appears. Turn it on and restart or rebuild the list: the Recents category returns. | |

## Probe tests

13y2 adds no runtime probe test (nothing here needs a WebView2 controller that the desktop run
does not already cover). To regression-run the existing `webhost_win::probe::` tests on the
console desktop, use a `.ps1` wrapper (`-Filters` is a `string[]` and `-File` passes a comma
string as one filter):

```powershell
$f = @('webhost_win::probe::')
& C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout> -Filters $f
```

The script runs only on Robert's yes. A line that ends `SKIPPED` (session 0) is not a pass.
