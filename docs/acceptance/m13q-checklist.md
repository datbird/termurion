# Milestone 13q acceptance checklist

Nothing of 13q was run on screen during the build. Build a release binary (`scripts/xt build --release`;
the debug build is a console-subsystem app). Close any open Tandem window, copy `tandem.exe`,
`tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: make three profiles from Settings > Profiles > Session: one with "After a session ends" set to
No Action, one to Close, one to Restart. Give the No Action profile a Log Directory of a scratch folder
and a second profile Automatically Log. Have WSL with `asciinema` (for the .cast check) and `tmux`
available; mark a row "not applicable" instead of fail when the tool is not installed. Rows marked
Safety are the ones the reviews found: run them all. A program-set title can be made with
`$Host.UI.RawUI.WindowTitle = '..\..\x'` in PowerShell. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Profile set to After a session ends: No Action. Type `exit`: the pane stays with its text and a blue centered "Session Ended" divider below, the cursor under it, and the bar "Session ended (command exited). Restart it?" with Restart and Don't Ask Again. A Windows notification for the ended session also shows. | |
| 2 | In that ended pane, type, paste and press Enter: nothing happens, nothing is sent. Scrollback, Find and copy still work on the kept text. | |
| 3 | Click Restart on the bar: the same pane gets a new prompt below the divider (not over the top lines), the old text and scrollback are kept, the bar is gone, the pane's lock and pinned width are kept. | |
| 4 | End the session again and click Don't Ask Again: the bar goes, later exits show no bar, and Settings > Advanced > Session "Suppress the Restart Session offer" (SuppressRestartAnnouncement) shows Yes. | |
| 5 | Close Pane on an ended pane (nothing left running): it closes at once with no question. | |
| 6 | New profile (stock, Close): `exit` closes the pane at once. A profile imported with no "After a session ends" value keeps the ended pane instead (iTerm2's missing-key default). An existing Tandem profile saved before 13q now keeps the pane (release-note item, ruling P12). | |
| 7 | Close profile whose command is `cmd /c exit`: opening it shows "A session ended very soon after starting. Check that the command in profile ..." with a "Suppress this message permanently" box and OK. Check the box: the next short session closes with no warning and Settings > Session "Warn about short-lived sessions" shows unchecked; check it again and the warning returns. | |
| 8 | Close a pane after more than 3 seconds, a git window, a tmux pane, a recording or a restored archive: no short-lived warning. Advanced > Warnings "Warn about short-lived sessions" time changes the 3 seconds. | |
| 9 | Profile set to Restart: `exit` restarts the shell about one second later in the same pane, history kept. A command that exits at once (`cmd /c exit`) stops restarting after five tries in ten seconds and shows "Session keeps ending. Restart it?". | |
| 10 | Set sessionEndMessageText to "Bye" and SessionEndMessageDividerCharacter to "=" in Advanced > Session: the end divider reads "==== Bye ====" style (text centered in "="). An empty divider character draws a thin line (U+2500) in iTerm2's blue. The custom text also replaces "Finished", "tmux detached" and "Session Restarted". | |
| 11 | A full-screen program (for example `less` or `vim`) killed from Task Manager: the divider lands in the normal screen, plain (no line art, no link, no bold), and the dead program's screen is gone. | |
| 12 | Session > Restart Session over a running `ping -t localhost` (nothing asked unless it left programs): the old ping stops, a "Session Restarted" divider appears under the text, a new prompt follows, scrollback is kept, mouse reporting and the alternate screen are reset, lock and pinned width kept. | |
| 13 | Restart Session in a pane with Instant Replay history: the history survives the restart. | |
| 14 | A profile whose command does not exist, restarted: the bar "Session failed to start." shows (no buttons) and the ended pane shows the error text. A restart that fails over a live shell leaves an ended pane (the old shell is gone, as in iTerm2). | |
| 15 | Close an ended pane while a Restart timer waits: nothing restarts afterward. | |
| 16 | Restarted pane with a profile that has Initial Text: the text is typed once into the new shell (not twice), and the pane starts in the folder it first started in, not another pane's folder. | |
| 17 | Run a git command from the git menu (for example Log): its window stays when the command ends with a "Finished" divider, no Session Ended alert, no Restart offer, and the owner pane's git status clears at that moment. | |
| 18 | A script REPL window and Manage Dependencies run: when they end they stay with the divider and show no restart offer. | |
| 19 | tmux: run `tmux -CC`, then detach from the gateway: each tmux window shows "tmux detached" and, with No Action and TmuxWindowsShouldCloseAfterDetach unset, asks "Close tmux windows after detaching?" with Always and Never on each window. Always closes all of them and the next detach closes without asking; Never keeps them and the next detach keeps without asking. | |
| 20 | Settings > Advanced > Tmux Integration shows the key as an optional Unspecified/No/Yes popup; set to Yes the detach closes the windows with no question under No Action. | |
| 21 | Safety: a shell that ended while `start /b ping -t localhost` still runs shows the divider and a bar naming ping. Close Pane asks first (naming ping); answering No leaves the pane; answering yes ends ping. | |
| 22 | Safety: the same pane (ended, ping running): Session > Restart Session, the offer bar's Restart, and a Restart-action timer. The menu and the bar ask "Restart session?" naming ping; the timer shows the restart offer instead of restarting. With a Close profile the pane stays (divider and bar) until closed by hand. | |
| 23 | Safety: close or restart a pane in the instant (a fraction of a second) after the shell exits while it left `start /b` running: the question still names the program (the survivors are known at the exit). | |
| 24 | Log to File in each style, then open the file. Raw: the bytes with escapes kept. Plain text: lines with "[yyyy-MM-dd hh.mm.ss.SSS] " in front, no stamp on blank lines, no escape codes. HTML: opens in a browser with colors in spans and <br/> line ends; text such as <b> in the output shows as text. asciinema: a .cast with a header line and "o" and "r" records (after resizing the pane), plays with asciinema on WSL. | |
| 25 | Session > Log > Log to File: a save dialog titled "Log to File", then a "Logging Style" window naming "Last used: ...". Picking an existing file asks "... already exists. Do you want to replace it or append to it?" with Replace and Append; both work. The item is checked while logging, clicking it stops the log, and closing the pane stops the file growing. | |
| 26 | Automatically Log with Log Directory D:\logs (missing): "Logging Folder Not Found" asks to create it; OK makes it and the file appears named <yyyyMMdd_HHmmss>.<profile>.w0t0p0.<id>.<pid>.<n>.log; Cancel logs nothing. | |
| 27 | Automatically Log with an empty Log Directory writes to the home folder under the default name. Two sessions in the same window write two different files. | |
| 28 | After `exit` (No Action) the log ends with the output and the Log to File item is unchecked; Restart Session makes the same file grow (appended), not a new file. With AutologAppends off, a restart replaces it. | |
| 29 | A log folder on a removed USB drive (or an unreachable \\host\share typed plainly in Log Directory): no probing delay, one "Couldn't write to session log" notification, and the log goes off. A \\host\share folder made by an interpolated value is refused with "Cannot start logging". | |
| 30 | Settings > Advanced > Experimental Features smartLoggingWithAutoComposer on, with Auto Composer: the plain text log holds the prompt once and the command as sent, without prompt redraw noise. | |
| 31 | Safety: a program that sets its window title to `..\..\x` or `C:\Windows` or `a/b` in a profile whose Log Filename Format is `\(session.name).log`: the log file lands inside the log folder, with the title as one name piece ("_" in place of separators); nothing is written outside the folder. | |
| 32 | Safety: a title that makes a 200+ character name ending in `.bat` (or a title of exactly `NUL`, `CON` or `COM1`): the log file name ends in `.log` and never names a device; no .bat appears in the log folder. | |
| 33 | Safety: a Log Directory typed as plain `C:\temp\new` writes to that folder (the backslashes are kept); a template using `\(profileName)` evaluates. | |
| 34 | A very slow log disk or share while output streams fast (for example `type` of a big file): output never stalls; if the write queue fills, the log goes off with one notification. | |
| 35 | Shell > Archives > Save Archive of Current Session...: a save dialog defaulting to "<session name>.itermarchive" ("Tandem Session.itermarchive" when unnamed); the file appears and the menu lists it after the separator by its name. | |
| 36 | Shell > Archives > Restore Archive... (or the recent item): a new window opens with the pane's text and an "Archive Restored" banner; typing does nothing; Session > Restart Session starts a live shell below the kept text. | |
| 37 | Closing a restored archive pane within 3 seconds shows no short-lived warning. | |
| 38 | Archive On Closure with an Archive Directory: closing a pane writes "9-27-26_3.04.05_PM - <name>.itermarchive" there (never ending in .log); closing two panes of the same name in the same second writes "... (2)" as well; an empty directory writes nothing; an unwritable or relative folder shows an "Archiving failed" notification. | |
| 39 | Archive On Closure and Quit: quit Tandem with the shells ending with it and Archive On Closure on: an archive of each pane is in the folder when it exits. Quit that keeps the shells in the host writes none. | |
| 40 | Safety: a crafted archive opens no more than the limit: a file over 512 MB is refused with a message; more than 64 windows or 64 tabs in a window is refused; a pane with more than 10,000,000 lines opens with the last 10,000,000; an unmarked file with several windows asks "Restore Archive?" first. | |
| 41 | Safety: a downloaded (Mark of the Web) archive asks "Restore Archive?" listing what its profiles run; No opens nothing. After Yes, Restart on its pane runs its profile; with No nothing starts. | |
| 42 | Safety: a program-set title cannot move the archive either: the archive name for a session titled `..\x` stays inside the Archive Directory. | |
| 43 | Safety: closing the shell that logs, quitting Tandem and restarting a pane each leave the log file complete up to the last output (no lost tail, no second writer overwriting). | |
| 44 | With Advanced > Experimental Features KillSessionsOnLogout and "Run jobs in servers" on: with a long job running, sign out of Windows and sign back in: no session comes back, no tandem-host or shell remains, and the session logs are complete. With the key off the sessions survive the sign-out (when the host does). The sign-out waits 4.5 s at most; if a modal dialog stops the app thread it waits up to 8 s and the sessions end without the log flush. | |
| 45 | Safety: logoff ends hosted sessions and writes logs: with Automatically Log on in a hosted pane with two Tandem windows open, sign out and read the log afterward: the output up to the sign-out is in it, and Archive On Closure wrote its archive. Turn the key off with the second window current: the sessions survive the sign-out. | |
| 46 | Restore a window (session restoration) whose pane started in D:\work: `cd` elsewhere, end the shell and Restart: the new shell starts in D:\work. | |
| 47 | Advanced settings that are stored and inert (ruling R5): Multiserver and UseNewContentFormat show and save but turning them off changes nothing. | |
| 48 | Settings > Session shows After a session ends, Warn about short-lived sessions, Automatically Log, Log Directory, Log Filename Format, Logging style, Archive On Closure and Archive Directory with iTerm2's enable rules (log rows enabled only with Automatically Log; the short-lived row only for Close). | |
| 49 | Kept iTerm2 quirks to recognise (not defects): a Restart end action has no limit in iTerm2 (Tandem stops after five in ten seconds); the end action Restart never writes "Session Restarted" (only Restart Session over a live shell does); a custom end text replaces every message; with No Action the short-lived warning appears only when the pane is closed within the limit; AutoLogFormat only supplies a profile's missing Log Filename Format. | |
| 50 | Safety: Session > Restart Session over a running shell asks "Restart session?"; leave it open, exit the shell (Restart end action) and let the timer restart it, type something in the new shell, then click OK on the old question: the new shell is NOT ended; the question asks again about the new shell. | |
| 51 | Safety: save a restored archive pane through session restoration (quit with it open, relaunch): it comes back as an archived pane (text, no shell, no profile started); Restart starts the shell. | |
| 52 | Safety: an imported profile or archive with Log Filename Format `x.bat` and Log Directory the Startup folder: its open question names the log and folder, and the file made is `x.bat.log`. Same for `\(profileName).cmd`. | |
| 53 | Restart over a live shell whose new command fails to start: the pane keeps its text, the log file is complete, the progress bar and drop targets are gone. | |
| 54 | Close a pane in a background window with Archive On Closure on: the archive holds that window's pane, frame and title, not the current window's. | |
| 55 | Two logging panes idle: Tandem shows no periodic wakeups (idle CPU stays at zero); a log on a removed drive still gives its one notification. | |
| 56 | Restore Archive of a file over 512 MB is refused at once (no pause). Load Arrangement of an unmarked file with several windows opens with no question. | |
