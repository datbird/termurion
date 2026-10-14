# Milestone 4a acceptance checklist

Close any open Termurion window, then run `%USERPROFILE%\src\termurion\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In a PowerShell tab: iTerm2 > Install Shell Integration > Yes (the box says it sets the execution policy to RemoteSigned for your user only if scripts are blocked). The command runs, and each new prompt gets a blue bar in the left margin. Open a new PowerShell tab: its prompts have bars too (the profile loads the script). Your own Enter and prompt (oh-my-posh, starship) still work, and a failed command still shows as failed in them. | |
| 2 | Run `cmd /c exit 3`: the next prompt's bar for that command turns red. Run `dir`: blue. | |
| 3 | In a WSL tab: Install Shell Integration > Yes types iTerm2's curl installer. After it finishes, open a new WSL tab: prompts have bars. | |
| 4 | Run a few commands, then Win+Shift+Up and Win+Shift+Down: the view jumps between commands, with the command selected; at the ends it wraps. | |
| 5 | Win+Shift+M (Set Mark) at some scroll position, scroll away, Win+Shift+J (Jump to Mark): back to that position. | |
| 6 | Edit > Set Named Mark (Win+Alt+Shift+M): a window asks for a name, prefilled with the date and time. OK adds a mark; Cancel or an empty name adds none. | |
| 7 | Edit > Select Output of Last Command (Win+Shift+A): the last command's output is selected and copied. Edit > Select Current Command selects the command text. | |
| 8 | Run `dir` twice, then Edit > Clear to Last Mark (Win+Alt+L): the output since the last command mark is gone and the prompt moves up under it. | |
| 9 | Resize the window narrower and wider with long wrapped output: the bars stay beside their prompts. | |
| 10 | Edit > Marks and Annotations > Alerts: the three choices have one check mark (Post Notification by default). Win+Alt+A (Alert on Next Mark) is checked; run `Start-Sleep 5` and switch to another app: a Windows notification "Mark Set" appears when the prompt returns. The check mark clears. | |
| 11 | Pick Show Modal Alert Box, then repeat row 10: a message box says the mark was set. Pick Play a Sound: a sound plays. | |
| 12 | Tick Alert on Marks in Offscreen Sessions; start `Start-Sleep 5` in one tab and switch to another tab: the alert fires when that prompt returns. | |
| 13 | Settings > Profiles > General: set Initial working directory to "Reuse previous session's directory". `cd C:\Windows`, then Win+T: the new tab starts in `C:\Windows`. | |
| 14 | Settings > Profiles > Terminal: untick Show Mark Indicators: the bars disappear (named marks still show). | |
| 15 | Type `echo one two three`, move the cursor to the middle, press Enter, then Win+Shift+Up: the whole command is selected, not only the part left of the cursor. | |
| 16 | Open vim or htop in WSL, quit it: the bars and Win+Shift+Up still work. | |
