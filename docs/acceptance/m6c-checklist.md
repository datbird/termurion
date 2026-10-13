# Milestone 6c acceptance checklist

Close any open Termurion window, then run `C:\Users\datbi\src\termurion\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The shell is the profile's default (PowerShell or cmd) unless a
line says WSL. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | With the stock Title Components (Job), a new tab reads `pwsh` (or `cmd`). Run `ping -n 20 127.0.0.1`: within a second the tab reads `ping`, and back to the shell when it ends. | |
| 2 | Settings > Profiles > General > Title components: turn on Session Name and Job. The tab reads `<profile name> (pwsh)`. With Command Line instead of Job, `ping -n 20 127.0.0.1` shows the full command in the parentheses. | |
| 3 | Start `notepad` from the shell: the tab still names the shell (a GUI program is not the job). | |
| 4 | Triggers editor, Match Against "Job Started", Job `ping`, action Show Alert `started \0`: `ping -n 3 127.0.0.1` shows "started PING" (or "started ping"). A "Job Ended" trigger for `ping` fires when it ends. | |
| 5 | A regular expression trigger with Job `ping` (Show Alert on `Reply`): it fires while ping runs, not when you type `echo Reply` at the prompt. | |
| 6 | Settings > Profiles > Session > Prompt before closing: "If there are jobs besides the ignore list". Run `ping -n 60 127.0.0.1`, press Cmd+W: a question "Close session ..." says the session is running ping; Cancel keeps it. At an idle prompt, Cmd+W closes without asking. | |
| 7 | Jobs to Ignore lists ssh, telnet, and plink. `ssh` into any host, then Cmd+W: no question. Add `ping` to the list: item 6 no longer asks. | |
| 8 | Prompt before closing "Always": closing any tab asks, with `The profile "<name>" always requires confirmation.` Closing the window asks once for all of them. | |
| 9 | Session > Run Coprocess... (Option+Cmd+R): a window "Coprocess command to run:" with a history list and "Silently ignore errors from this command". OK with an empty command beeps and stays open. | |
| 10 | Run the coprocess `echo echo hello from coprocess`: the shell runs `echo hello from coprocess` as if typed. The coprocess icon shows at the pane's top right while it runs and goes away when it ends. | |
| 11 | Run the coprocess `findstr /c:"ERROR"` then type `echo ERROR here` and `echo fine`: the coprocess types back the ERROR line only. Session > Stop Coprocess (Option+Cmd+Shift+R) ends it; the icon goes away. | |
| 12 | Right-click while a coprocess runs: the menu starts with "Stop Coprocess". | |
| 13 | Run the coprocess `echo oops 1>&2`: a bar says `Coprocess "echo oops 1>&2" terminated with output on stderr.` View Errors opens the text in Notepad. Run it again and choose Ignore Errors from This Command: the next run shows no bar. | |
| 14 | A Run Coprocess trigger while a coprocess runs: a bar "Run Coprocess trigger: Can't run two coprocesses at once." with Silence Warning, once per session. | |
| 15 | A Run Silent Coprocess trigger with `ping -n 10 127.0.0.1 >NUL`: while it runs, typing `dir` shows nothing new; output shows again after it ends. | |
| 16 | Settings > Advanced > "Coprocess for dropped files": `cmd /c echo \(filenames) > %TEMP%\dropped.txt`. Drop two files on the pane: the file holds both paths. With the setting empty, a drop pastes the paths (quoted when they hold a space). | |
| 17 | Keys: bind a key to "Run Coprocess" with text `echo echo from a key`: the key runs it. Bind "Copy Interpolated String" with `\(session.jobName) in \(session.path)`: the clipboard holds the job and folder. | |
| 18 | Duplicate the Default profile as "Build", give it another background color, and in Profiles > Advanced > Automatic Profile Switching add `C:\src\*` (use a folder you have). With shell integration, `cd C:\src\x`: the session switches to Build and "Switched to profile "Build"." shows. `cd C:\`: back to Default. | |
| 19 | Add the rule `&ping` to Build: `ping -n 5 127.0.0.1` switches to Build while ping runs, then back. | |
| 20 | Add the same rule to a second profile: the editor warns `The profile "Build" is already bound to hostname ...` and takes it out of this profile. An empty or repeated rule beeps and goes. | |
| 21 | Edit Session, turn on Prevent Automatic Profile Switching: item 18's `cd` no longer switches this session. | |
| 22 | Settings > Advanced > "Write Automatic Profile Switching debug logs": after item 18, the Script Console's "Automatic Profile Switching" entry (Scripts > Manage > Console, milestone 11d) lists each decision. | |
