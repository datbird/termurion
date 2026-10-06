# Milestone 13v2 acceptance checklist (updates, tips, Dynamic Profiles, menu tips, screen readers)

Build a release binary (`scripts/xt build --release`); close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Run the whole list in
a desktop session on EXTREME (not over ssh: a session without a desktop cannot show a window, a
tooltip or a screen reader). Mark each line pass or fail. Every row below was never exercised by
the automated suite, which has no composed desktop, no real screen reader and no real GitHub
feed; the tests cover the models, the gates and a loopback `curl.exe` only.

Setup:
- A public releases feed with one release newer than the build. The feed is
  `https://api.github.com/repos/datbird/tandem/releases?per_page=30`; while that repository is
  private it answers 404 (rows 8 and 9). For the other rows make the repository public (Robert's
  call) or point the pinned URL at a public repository you control for the test build; put a
  release tagged higher than the build's version (`v9.9.9`) with Markdown notes on it.
- A fresh settings folder for the launch-count rows (rename `%APPDATA%\Tandem` away, restore it
  after). The launch count is the number of times `tandem.exe` has started: a quit and a start.
- Narrator (Win+Ctrl+Enter) and NVDA (installed, not portable is fine) for the screen reader rows.
  Accessibility Insights for Windows if available.
- A second local user `tandemtest` (no admin) and an elevated PowerShell for the security rows.

## Updates

| # | Check | Result |
|---|---|---|
| 1 | Settings > General > Software Update (just above Experimental) shows "Check for updates automatically" (checked on a fresh install) and "Update to Beta test releases" (unchecked). | |
| 2 | Tandem > Check For Updates... (the tray item too) opens "Software Update" with "Checking for updates..." and Cancel, then "A new version of Tandem is available!" with "Tandem 9.9.9 is now available (you have <current>). Would you like to learn more about this update on the web?", "Release Notes:" in a scroll area (plain text, no formatting), and Skip This Version, Remind Me Later, Learn More... | |
| 3 | Learn More... opens the release page in the browser and nothing is downloaded (check the Downloads folder and Task Manager for a new process other than the browser). | |
| 4 | Skip This Version, then Check For Updates again: the window still offers it (a manual check ignores the skip). Quit and start Tandem (a background check runs): nothing shows. | |
| 5 | Remind Me Later (and closing the window with its close box) leaves the next background check free to offer it again. | |
| 6 | While a check is showing "Checking for updates...", Cancel: the window closes, no error, no second request starts at once (watch Task Manager for `curl.exe`). Quitting Tandem during a check ends `curl.exe` too. | |
| 7 | With a build newer than every release: a manual check shows "You're up-to-date!" and "Tandem <newest> is currently the newest version available." plus, when running a newer one, "(You are currently running version <current>.)". | |
| 8 | With the network off (disable the adapter): a manual check shows "Update Error!" with Sparkle's text and OK; a background check (restart Tandem with the second-launch answer Check Automatically) shows nothing. | |
| 9 | While the releases URL answers 404 (the repository private): a manual check closes the Checking window with no error; a background check shows nothing. | |
| 10 | Fresh settings folder: the first launch ever neither asks nor checks (watch for `curl.exe`). The second launch asks "Check for updates automatically?" with Check Automatically and Don't Check, after the first window shows. Closing that question with its close box stores nothing: it asks again on the third launch. | |
| 11 | Check Automatically then restart: a background check runs (a newer release shows the window; none shows nothing); Settings shows the box checked. Don't Check: the box is cleared and no check runs. | |
| 12 | Update to Beta test releases: with a pre-release newer than the newest final one, it is offered only with the box checked. | |
| 13 | Security: a feed served over http or with a bad certificate is refused. Run a local TLS-inspecting proxy whose certificate is not in the Windows store (mitmproxy without installing its CA) and start Tandem with `HTTPS_PROXY` pointing at it: a manual check shows "Update Error!" and no release is offered. Then make the proxy answer the feed request with a 302 to `http://127.0.0.1:<port>/`: the check ends in "Update Error!" and nothing is fetched over http (the proxy log shows no http request). | |
| 14 | Security: a release whose `html_url` is on another host (edit the test feed to `https://evil.example/x`, then to a `file:` path): Learn More opens the pinned releases page, never the other address. | |
| 15 | Security: a planted `%USERPROFILE%\.curlrc` containing `insecure` and a `CURL_CA_BUNDLE` environment variable pointing at an empty file change nothing: row 13's bad certificate is still refused, and row 2 still works. | |

## Tip of the Day

| # | Check | Result |
|---|---|---|
| 16 | Tandem > Show Tip of the Day shows a card at the top left of the work area (8 px in), 400 px wide, always on top, with no taskbar button. It does not take typing focus while a command runs in the pane. | |
| 17 | The card shows the title large, the body, and "Tandem tip of the day" small and italic at the right; buttons Learn More (only when the tip has a link), Dismiss Tip, More Options. More Options adds Show This Later, Share, Show Tips Weekly (or Daily), Disable Tips (or Enable Tips), Show Next Tip, Show Previous Tip. | |
| 18 | Next and Previous change the tip; Dismiss Tip closes the card and the tip does not come back as the next scheduled tip. Share, then paste in Notepad: "Title", a blank line, the body, a blank line, the address. | |
| 19 | Show Tips Weekly changes the label to Show Tips Daily and back. Disable Tips closes the card, stops scheduled tips and the menu item still shows one on demand. Settings > Advanced > Tip of the Day shows both rows and they follow. | |
| 20 | Drag the card to another place, close it, then Show Tip of the Day again: it opens at the dragged spot. Drag it off the screen edge (second monitor unplugged): it opens on the work area. The menu item is grayed while a card shows. | |
| 21 | A card asked for from the menu does not take focus: Esc does nothing until the card is clicked, then Esc dismisses it (Esc in another app never does). | |
| 22 | Learn More on a tip with a link opens the browser on iterm2.com and dismisses the card. | |
| 23 | Fresh settings folder, the third launch (the update question answered on the second): the question "See Tips of the Day?" with Yes and No shows after the first window; Yes shows a card one second later; No stops it and shows no card. Closing the question with its close box asks again next launch. | |
| 24 | While only the tray icon is up (every window closed with the resident option on), no tip card appears. | |
| 25 | A tip names real things: pick five tips and look up each quoted menu path, setting name and key chord in Tandem. | |

## Dynamic Profiles

| # | Check | Result |
|---|---|---|
| 26 | Create `%APPDATA%\Tandem\DynamicProfiles\dev.json` with `{"Profiles":[{"Name":"Dev","Guid":"dyn-dev-1","Badge Text":"DEV"}]}`: Dev appears in the Profiles menu and Open Quickly within a second. Running sessions are unaffected. | |
| 27 | Change the Name in the file: the menu changes within a second. Delete the file: the profile goes; a session already open from it keeps its look. | |
| 28 | A bad file (`{"Profiles":[{"Name":"x"}]}`, no Guid, or broken JSON) shows "Dynamic Profiles Error" with OK, View Log and Reveal in Explorer; View Log opens the Script Console on "Dynamic Profiles"; ten broken files give one alert saying "N additional errors may be seen in the log." | |
| 29 | A profile with `"Dynamic Profile Parent Name": "Default"` inherits the parent's settings and shows only its own differences. | |
| 30 | Edit Dev in Settings > Profiles: "Changes Will Be Lost" with Mark as Rewritable, Reveal in Explorer and Cancel. Cancel changes nothing; Reveal selects the file; the question comes once per profile per Settings window, and its answer is silenced for ten minutes as the other warnings are. | |
| 31 | Mark as Rewritable, then change the badge: within a second the file holds the new value (only the changed keys, the file's type kept) and the profile list shows no flicker. A file edited by hand between the load and the write is left alone (the console says "changed on disk, not rewritten"). | |
| 32 | Profiles > Open All and a tag open do not open Dev; opening it by itself does. Dev is not in `%APPDATA%\Tandem\profiles.toml` and can not be made the default (Set as Default is refused or the saved default stays). | |
| 33 | Security: a dynamic profile file owned by another user is refused with a banner. In an elevated PowerShell: `icacls "%APPDATA%\Tandem\DynamicProfiles\dev.json" /setowner tandemtest`. The reload shows "Dynamic Profiles Error" naming the owner; Dev is gone. Restore the owner and Dev comes back. The same for `icacls <file> /grant Users:(W)` (writable by others). | |
| 34 | Security: a file carrying the download mark is refused with a banner. `Set-Content -Path dev.json:Zone.Identifier -Value "[ZoneTransfer]`nZoneId=3"`: the alert says to unblock the file first; `Unblock-File` brings it back. | |
| 35 | Security: a junction inside the Dynamic Profiles folder is refused. `mklink /J "%APPDATA%\Tandem\DynamicProfiles\jct" C:\Temp\dp` with a valid profile file in `C:\Temp\dp`: the profile is not loaded and the alert names the link. A symbolic link to a file, and a hard link, are refused too. | |
| 36 | Security: a Dynamic Profiles folder set in Advanced to a network path (`\\host\share\dp`) or a mapped drive is refused with a banner and no read of it. A profile with a Command in a folder the person owns runs it when opened by hand; the same file in a folder another user can write is refused. | |
| 37 | Advanced > General shows "Path to folder with dynamic profiles", the delay row and "Add a 'Dynamic' tag to dynamic profiles" with iTerm2's help text; the tag row adds the tag to Dev. | |
| 38 | A program cannot make a profile dynamic: in a pane run `printf '\e]1337;SetProfile=Is Dynamic Profile=true\a'`; nothing changes and Tandem shows nothing. | |

## Menu hints and tips, Help items, screenshot cap

| # | Check | Result |
|---|---|---|
| 39 | Settings > Advanced > General shows "Show hints in split pane menu items", "Time to wait before showing tips in the main menu" and "Maximum pixel height for a single screenshot file" with iTerm2's text. | |
| 40 | With "Show hints in split pane menu items" on and a restart, the four Shell split items start with the line-box and bar-box glyphs, horizontal items with one and vertical items with the other. Off: no glyphs. | |
| 41 | Hover Session > Bury Session for a second: a text tip appears right of the item, inside the screen. Move to Shell > Split Vertically with Current Profile: its tip replaces the first at once. Close the menu: the tip goes. | |
| 42 | View > Toolbelt > Notes (a submenu item) shows its tip beside the item, not at the menu bar's corner. Tips are text only. | |
| 43 | Menu Tip Delay set to 3 delays the first tip to about three seconds; moving to another tipped item while a tip shows swaps it at once. | |
| 44 | Help > Open Source Licenses opens "Tandem Licenses.txt" in Notepad: it lists crates (alacritty_terminal, vte, glyphon among them), the ConPTY notice and iTerm2's GPL notice for the bundled scripts and data. Open it twice: no second file is left in `%TEMP%`. | |
| 45 | Help > Copy Mode Shortcuts opens iTerm2's page in the browser. | |
| 46 | Settings > Advanced screenshot cap 10000, then Make Screenshot of a tall scrollback: more than one file, none taller than 10000 px. A cap of 5 behaves as 1000. | |

## Screen readers (Narrator, then NVDA)

| # | Check | Result |
|---|---|---|
| 47 | Narrator (Win+Ctrl+Enter), focus Tandem: it reads the focused pane as "shell" (a Text element in the window). Tandem uses no CPU at idle with Narrator running and with it off (Task Manager). | |
| 48 | Narrator reads the prompt; Caps+Up arrow reviews the lines (one terminal row each); Caps+Left and Right review by character and word; the pane's own mouse selection is spoken as selected text. | |
| 49 | Run `dir`: the output is announced once after it ends, without the typed command. `cat` of a long file announces only the last 256 lines. | |
| 50 | Backspace at the prompt announces the deleted character. | |
| 51 | Narrator's selection of a word selects it in the pane (the same cells highlight); with NVDA, review-cursor select does the same. | |
| 52 | Scroll back through history with the review cursor: the pane scrolls to follow. | |
| 53 | Settings > Advanced > Terminal "Maximum number of lines of history to expose to Accessibility" set to 10 limits review to the last 10 lines (plus the screen); 0 and -5 behave as 1000. | |
| 54 | Two split panes: each is its own element; Narrator's next and previous item moves between them; moving focus between panes announces the newly focused one. | |
| 55 | A second window: only the current window's text updates; switching windows republishes. | |
| 56 | Toggle window transparency (the window is recreated) with Narrator running: it keeps reading. Close a window with Narrator on: no crash and no stale text. Quit Tandem: Narrator recovers. | |
| 57 | Repeat rows 47 to 52 with NVDA (numpad review keys; NVDA's UI Automation support for terminals may need "Use UI Automation to access the Windows Terminal" style setting on). Accessibility Insights: the tree is Pane > Text "shell" with the Text pattern, read-only. | |
| 58 | Security: a screen reader reads nothing typed at a password prompt. In a PowerShell pane run the reader from the m10b checklist (it prints `Password: ` with echo off), with Narrator on: only "Password:" is readable by review, and nothing typed is spoken or readable. Repeat with Settings > Advanced "Show key at cursor at password prompt" both on and off: the result is the same. Selecting from the screen reader at that prompt does nothing. A prompt that wraps onto a second row (a narrow pane) still shows only the prompt. `ssh.exe` and `runas` read in raw mode and are not detected (m10b ruling): at their prompts the text is not hidden. Mark that row not applicable, not fail. | |
| 59 | Security: a screen reader holding a closed window's text gets an error. In PowerShell: `Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes`; get Tandem's pane element, `$tp = $el.GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)`, `$r = $tp.DocumentRange`, `$r.GetText(-1)` works; close the Tandem window; `$r.GetText(-1)` now throws ElementNotAvailable (no text, no hang). Accessibility Insights holding the tree shows the same. | |
| 60 | A pane that is first seen at a password prompt (open a profile whose command is the reader) exposes nothing until the prompt clears; then its text appears. | |
| 61 | Fast output does not storm a reader: with Narrator on, run a loop that rewrites one line a thousand times a second (`1..5000 \| % { Write-Host -NoNewline "`r$_" }`): Narrator does not stutter or lag, and Tandem's CPU stays at its normal level for that output. | |
| 62 | With Narrator on, step through Instant Replay (or open a Filter or Zoom view): Narrator announces nothing from the frames and review reads the live pane's text. Narrator's select or scroll from the screen reader does nothing while the view is up and works again after it closes. | |
