# Milestone 13b acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: start from settings with no remembered answers (Tandem > Suppressed Alerts… empty). Have a
PowerShell pane with Tandem's shell integration (Tandem > Install Shell Integration), a WSL bash
pane for the `printf` lines, and an ssh host whose shell has iTerm2's shell integration installed
(it must report its host name). "Tandem > …" is the application menu (iTerm2's app menu). A
button's Alt shortcut is Alt plus the letter shown underlined with the Advanced setting "Buttons in
modal alerts indicate keyboard shortcuts." (General). Mark each line pass or fail.

## Remembered answers and Suppressed Alerts

| # | Check | Result |
|---|---|---|
| 1 | Paste two lines into a shell: "Confirm Multi-Line Paste" asks with Paste, Cancel and Advanced..., and its checkbox reads "Remember my choice". Check it and choose Paste. | |
| 2 | Paste two lines again: nothing asks, the paste goes in. Tandem > Suppressed Alerts now reads "1 Alert Suppressed (< 1 min ago)…"; after a few minutes the time grows ("5 min ago") within a minute of the real age. | |
| 3 | Open Tandem > Suppressed Alerts…: the "Suppressed Alerts" window lists the paste warning with the columns Alert (heading, then the text), Automatic Response ("Paste") and Last Suppressed ("moments ago" or "N minutes ago"). Hovering the row shows the warning's text. | |
| 4 | Select the row and press Un-suppress (or double-click it): the row goes, the menu reads "Suppressed Alerts…", and the next two-line paste asks again. Un-suppress All clears every row. With nothing suppressed the window says "No alerts are currently being suppressed." | |
| 5 | Remember the paste answer again, then turn on "Always show alerts with remembered selections" in the window: the next two-line paste asks, its text ends with "This alert had a saved selection of “Paste”. It is being shown because “Always show alerts with remembered selections” is turned on in Tandem > Suppressed Alerts.", and a fourth button "Permanently Forget Saved Selection" forgets the answer and asks again without it. Quit and restart Tandem: the checkbox is off again. | |
| 6 | Paste a single line ending in a newline at a prompt: its warning has two buttons that are not Cancel (Paste, Paste Without Newline), so the box reads "Remember my choice"; check it and choose Cancel: nothing is remembered and the next such paste asks again. | |
| 7 | Turn on "Buttons in modal alerts indicate keyboard shortcuts." (Settings > Advanced, General). Open two panes, turn on Broadcast Input for both, and choose Edit > Clear Buffer: "Clear in All Broadcasted-to Sessions?" asks with Clear All, Clear Current Session Only and Cancel; "Clear Current Session Only" shows an underlined C, and Alt+C presses it (only the current pane clears). Clear All (the first button) and Cancel have no letter. Turn the setting off: no letter is underlined and Alt+C still presses it. | |
| 8 | In Settings > Advanced (Warnings), "Send Clear Session to all broadcasted-to sessions?" set to No: Clear Buffer with broadcast still asks (No means not silenced). Remembering Clear Current Session Only makes it answer silently, and it appears in Suppressed Alerts. | |

## Confirmations

| # | Check | Result |
|---|---|---|
| 9 | Session > Restart Session in a pane whose shell is running: "Restart session?" ("Running jobs will be killed.", OK and Cancel, the box "Remember my choice"). OK restarts the shell. In a pane whose shell has exited, Restart Session restarts at once without asking. | |
| 10 | In the WSL pane, `printf '\033]1337;File=name=YS50eHQ=;size=3:YWJj\a'`: "Allow Terminal-Initiated Download?" ("The terminal has initiated transfer of a file named “a.txt” of size 3 bytes. Download it?", Yes and No). Yes saves `a.txt` (or a renamed copy) to Downloads; No saves nothing, and so does closing the question with the title bar X or Alt+F4. | |
| 11 | `printf '\033]1337;File=name=YS50eHQ=;size=3;inline=1:YWJj\a'`: "Allow Terminal-Initiated Display?" asks before the file shows, and the pane's later output waits until it is answered. Closing it with the title bar X or Alt+F4 does not show the file (No) and the output goes on. | |
| 12 | Open an annotation (Annotate Selection) and press its Delete: "Really remove annotation?" (heading "Confirm", OK and Cancel, "Suppress this message permanently"). OK removes it; with the box checked, the next Delete removes at once. | |
| 13 | Settings > Profiles > Keys, set Option Key Sends (Left) to "Set high bit (not recommended) / Meta": the warning "You have chosen to have an option key act as Meta..." shows over the settings window with OK only and "Suppress this message permanently". | |
| 14 | Add a profile key binding for a keystroke the global key bindings already have: "The keyboard shortcut you have set for this profile will take precedence over an existing shortcut for the same key combination in a global shortcut." (OK and Cancel); Cancel does not save the binding, OK saves it. Add a global binding for a key some profile binds: "The global keyboard shortcut you have set is overridden by at least one profile..." (OK and Cancel). | |

## Offers after a host change

| # | Check | Result |
|---|---|---|
| 15 | In the PowerShell pane, `ssh` to the host, run `printf '\033[?1004h\033[?2004h'`, then `exit`: back at the local prompt, bars offer "Looks like focus reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?" (Yes, Always, Never) and the same for paste bracketing (with Help, which opens https://iterm2.com/paste_bracketing). Each appears once. Yes turns the mode off (Session > Terminal State shows it off). | |
| 16 | Answer Always to both, then repeat check 15: nothing is offered and both modes are off after `exit`. Settings > Advanced (Warnings) shows "Always turn off focus reporting when host changes?" and the paste row at Yes. Set them to Unspecified: the next round offers again. Never stores No and leaves the modes on silently. | |
| 17 | Repeat with `printf '\033[?1000h'` on the remote host: the mouse reporting offer appears the same way. A remote host whose shell is fish never gets the paste bracketing offer. | |

## Mouse reporting frustration

| # | Check | Result |
|---|---|---|
| 18 | In the WSL pane, `printf '\033[?1000h'`, drag across text (no selection appears), then Edit > Copy (or Ctrl+Shift+C): the bar "Looks like you’re trying to copy to the pasteboard, but mouse reporting has prevented making a selection. Disable mouse reporting?" offers Temporarily, Permanently and Stop Asking. It is offered once per drag; a click, another key or a drag released within half a second before Copy does not offer. | |
| 19 | Temporarily turns mouse reporting off until the program turns it on again; Permanently also turns "Mouse Reporting" off in the pane's profile (its session copy when it has one); Stop Asking sets "Suppress the notification asking if you want to disable mouse reporting that is shown after a drag followed by Win+C..." in Settings > Advanced (Warnings) and it is never offered again. | |

## Arrangements

| # | Check | Result |
|---|---|---|
| 20 | Make a profile whose Command type is SSH, open a window with it in a folder you can delete (a local Windows folder), save it as a named arrangement, delete the folder, and restore the arrangement: the bar "The saved arrangement “<name>” has a bad initial directory of “<folder>” for this session." offers Don’t Warn Again and Repair. Repair asks for a folder; restore again: the session starts in the picked folder and nothing is offered. Don’t Warn Again turns on the Advanced setting and the bar never shows. An SSH-profile session saved with a Unix folder (`/home/...`) shows the same bar on restore. | |
| 21 | The same with an ordinary (Login Shell) profile: the session starts in the home folder and the pane shows the line "The directory “<folder>” is unavailable. Started in home directory instead."; no bar. A folder saved as a Unix path (a WSL pane's `/home/...`) shows neither. | |
| 22 | Restore an arrangement saved on another machine (or edit one so its session's Guid names no local profile): the session restores with the arrangement's own profile settings (its colors and command), as a session copy, with no banner, even when a local profile has the same name. | |
| 23 | Edit an arrangement so a session's embedded profile has no Name or no Guid and restore it by name: the bar "This arrangement’s profile is missing. This could be due to a bug in iTerm2 version 3.5.7, which caused profiles to be corrupted in saved arrangements." offers Assign Profile; it opens a profile list ("Select a profile to use for this session. Your selection will be saved back to the arrangement."), Enter picks, and the next restore uses the picked profile without asking. | |

## Shell integration

| # | Check | Result |
|---|---|---|
| 24 | In a WSL bash, send the version report of iTerm2's version 4 bash script: `printf '\033]1337;ShellIntegrationVersion=4;shell=bash\a'`: the bar "This account’s Shell Integration scripts are out of date." offers Upgrade and Silence Warning. Silence Warning stops it for that user and host. The PowerShell pane with Tandem's script (version 1, pwsh) never shows it. | |
| 25 | Settings > General > Software Update, "Ignore non-critical shell integration updates" off: a report of version 5 for bash is now out of date (latest 22); on (the default), it is not. Upgrade away from a prompt (while a program runs) asks "It looks like you're not at a command prompt." with Run Installer Anyway and Cancel. | |

## Kept iTerm2 quirks and rulings a person can see

| # | Check | Result |
|---|---|---|
| 26 | Two buttons may share one shortcut letter (iTerm2 never marks a letter as taken); Alt plus that letter presses the first. | |
| 27 | Remembering Cancel on "Restart session?" makes Restart Session do nothing from the menu until it is un-suppressed (iTerm2's restart question names no cancel button). | |
| 28 | "Always show alerts with remembered selections" resets when Tandem restarts; the menu title's age can lag by up to a minute. | |
| 29 | The settings warnings (Meta, key overrides, another hotkey profile) never underline their letters; Alt plus the letter still works. | |
| 30 | In the remembered mode the annotation question has no explanation or Forget button; un-suppress it from Suppressed Alerts. | |
| 31 | The question before adding another hotkey profile is always asked and has no checkbox (iTerm2's is persistent); a box ticked in an older Tandem no longer skips it. | |
| 32 | With Save changes set to Manually for a settings folder, the quit question asks at every quit after a change, even with Lose Changes remembered. | |
| 33 | The paste and tmux close questions are windows, not pane bars; a second paste while the paste question is open is dropped (paste again). | |
| 34 | The broadcast input warning is headed "Warning" with OK and Cancel; "Install Script Archive?" is silenced for ten minutes only. | |
| 35 | A remembered "Use Default Profile" answers the tmux New Tab/Window question (the old rule read either answer as a tmux tab). | |
| 36 | Inline images ask "Allow Terminal-Initiated Display?" each time until Yes is remembered; a file with no size reads "0 bytes"; an untrusted terminal is asked, not refused. A remembered Yes on a file over 1 GiB asks "Allow Large File Download?" (Allow, Deny). | |
| 37 | A host whose reported name changes only in case offers to turn modes off; a remote host whose name equals this machine's never does. | |
| 38 | An arrangement from another machine no longer picks a same-named local profile; after window restoration (no arrangement name) the broken profile bar has no Assign Profile button. | |
| 39 | Advanced settings that do nothing yet: "Suppress the notification that a restored session’s profile no longer exists?", "Suppress the notification asking if you want to disable resize reporting" and "Suppress warning about copying a session's settings over a Profile". | |
| 40 | Silence Warning on the shell integration notice is stored in `settings.toml` only (`SuppressShellIntegrationUpgradeAnnouncementForHost_<user>@<host>`); no setting shows it. | |
| 41 | A temporary silence imported from iTerm2 reads as long expired and asks again. | |
| 42 | Ctrl+C from the Jobs tool is refused while its pane waits on an inline file question; a Copy or Send ^C key that brings the frustration offer also sends ^C to the program. | |
| 43 | The bad folder bar closes on a key press (iTerm2's announcement is not important); Repair appears only for a profile whose Command type is SSH, not for a Command profile that runs ssh.exe. | |

## Final fix wave

| # | Check | Result |
|---|---|---|
| 44 | While the download question of check 10 is open, send the same `printf` twice more in that pane: no second question queues (the later files are refused, as No), and a two-line paste in another pane still asks its question after it. | |
| 45 | Settings folder: pick a custom folder with its checkbox on so "Copy local settings to custom folder now?" asks; closing it with the title bar X copies nothing (Don't Copy). | |
| 46 | Session > Restart Session picked twice before answering asks once. | |
| 47 | A file name the terminal sends with control or right-to-left characters, or over 200 characters, is shown without them (cut with an ellipsis) in the download and display questions. | |
| 48 | An ordinary profile's arrangement folder `~/gone` (edited by hand) starts in home with the unavailable banner; an existing `~/x` is used as that folder under home. | |

## Landing fixes

| # | Check | Result |
|---|---|---|
| 49 | Settings folder: with a custom folder in use, edit its settings file in Notepad after Tandem starts, then Save Now: "Settings at <folder> changed since Tandem started. Overwrite it?" asks; closing it with the title bar X leaves the file as Notepad saved it (Discard Local Changes). | |
| 50 | With Save changes set to Manually and a change made, quit: closing "Settings have changed. Copy them to <folder>?" with the title bar X copies them (the one question whose close does not refuse); Escape does nothing. | |
| 51 | An SSH-profile session saved with contents and the folder `~/proj` in a named arrangement restores with the bad folder bar (Repair) and without the window banner "The folder ... from the arrangement does not exist"; it shows the same bar on every restore. | |
| 52 | A file name the terminal sends with a line or paragraph separator (U+2028, U+2029) or a zero-width character (U+200B to U+200D, U+2060, U+FEFF) is shown on one line without them in the download and display questions. | |
| 53 | In a tmux window with two tabs, click one tab's close button twice before answering: the "Kill tmux window, terminating its jobs, or hide it?" question asks once. | |
