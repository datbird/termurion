# Milestone 6a acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Profiles > Advanced > Triggers: press +, set Regular Expression `error`, Action "Highlight Text...", pick a red background. Run `echo an error here`: "error" shows on red, the rest does not. | |
| 2 | Add a trigger `(\w+)@(\w+)` with "Highlight Line...": `echo bob@box` colors the whole line. | |
| 3 | Add `^PASS` with "Change Style..." and `1;32`: `echo PASS` shows bold green. | |
| 4 | Add `ticket-(\d+)` with "Make Hyperlink..." and `https://example.com/\1`: `echo ticket-42`, Ctrl+click it: the browser opens example.com/42. | |
| 5 | Add `done` with "Show Alert..." and `Finished: \0`: `echo done` shows a bar "Finished: done" with OK and Disable This Alert. Run it twice within a second: one bar. Disable This Alert: later matches show nothing in this session. | |
| 6 | Add `ping` with "Send Text..." and `pong\r` (bash `read x; echo got $x`): typing `echo ping; read x; echo got $x` prints "got pong". | |
| 7 | Add `^\$ $` with "Prompt Detected", Instant on: in a shell without shell integration, the prompt rows get marks (Win+Shift+Up jumps to them). | |
| 8 | Add `note (.*)` with "Annotate..." and `\1`: `echo note hello` puts an annotation reading "hello" over the text. | |
| 9 | Add `^start$` "Set Named Mark" `top` and `^end$` "Fold to Named Mark" `top`; `printf 'start\na\nb\nend\n'` folds start to end into one line. | |
| 10 | Add `cd (\S+)` "Report Directory" `\1`, then `echo cd /tmp` and `printf '\e]1337;ReportVariable=%s\a' "$(printf path | base64)"; read -rs -t10 -d $'\a' x; echo "$x"` (allow the variable once): the reply decodes to `/tmp` (`echo ... | base64 -d`). | |
| 11 | Add `beep` "Bounce Dock Icon" (Bounce Once), switch to another app, then `sleep 2; echo beep` in Tandem: its taskbar button flashes once. | |
| 12 | Add `run it` "Run Command..." `echo hi > %TEMP%\tandem-trigger.txt`: `echo run it` creates that file. `exit 3` as the command shows "Run Command Trigger Failed". | |
| 13 | Add `x` with "Capture Output": `echo x` shows once "The trigger action “Capture Output” comes in milestone 9." | |
| 14 | Turn on "Use interpolated strings for parameters"; a Show Alert trigger `id=(\w+)` with `\(matches[1]) on \(session.name)` shows "abc on <the tab title>" for `echo id=abc`. `\(f(1))` shows no alert (function calls are refused). | |
| 15 | Run `vim`, type `:echo "error"`: no highlight while "Enable triggers in interactive apps" is off; Session > Triggers > Enable Triggers in Interactive Apps (check mark) turns it on for this session. | |
| 16 | Session > Triggers > Disable All: nothing fires; Enable All: they fire again (this session only; Settings still shows the saved states). | |
| 17 | Select the text `a.b*c` and choose Session > Triggers > Add Trigger...: Settings opens on a new trigger with Regular Expression `a\.b\*c` and name `a.b*c`. | |
| 18 | Instant: a trigger `Password:` with Instant on and "Show Alert..." fires on a `read -p "Password:"` prompt without Enter. | |
