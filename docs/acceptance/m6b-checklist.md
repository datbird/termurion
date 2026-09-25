# Milestone 6b acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `printf '\a'`: Windows plays its Default Beep. Settings > Profiles > Terminal > Silence bell: no sound. | |
| 2 | Turn on "Show bell icon in tabs", open a second tab, run `sleep 2; printf '\a'` in the first and switch to the second: the first tab shows a bell icon until you select it. | |
| 3 | Turn on "Flash visual bell": `printf '\a'` flashes the pane. | |
| 4 | `while true; do printf '\a'; sleep 0.005; done`: at most about 10 beeps a second, and within a few seconds a bar asks "The bell is ringing a lot. Silence it?" with four buttons. Silence Bell Temporarily: quiet for a minute. Ctrl+C the loop. | |
| 5 | Turn on "Notification Center alerts" and the Bell filter; with "Show bell icon in tabs" on, switch to another app, then `sleep 3; printf '\a'`: a Windows notification "Bell" / "Session <name> #1 just rang a bell!" appears. Click it: Tandem comes forward on that session. | |
| 6 | Turn on the Idle filter: in a background tab run `for i in 1 2 3; do echo $i; sleep 1; done`: about 3 s after the output stops, "Idle" / "Session <name> in tab #2 became idle." | |
| 7 | Turn on the New Output filter: output in a background tab posts "New Output" once per burst. The tab shows a dot while the output is new, and the idle icon after it stops. | |
| 8 | `printf '\e]9;Build finished\a'` (Send escape sequence-generated alerts on): "Alert" / "Session <name> #1: Build finished", even while you look at it. | |
| 9 | `printf '\e]1337;Notification=title=%s;message=%s\a' "$(printf Deploy | base64)" "$(printf done | base64)"`: a notification titled "Deploy" reading "done". | |
| 10 | Turn on Session Ended; `exit` in a background tab: "Session Ended" / `Session "<name>" in tab #N just terminated.` | |
| 11 | Window > Notify on Status Change, then in another pane `printf '\e]21337;status=waiting\a'`: a bar "Session status changed. ..." with OK and Reveal. | |
| 12 | Settings > Profiles > Session > Periodically send an ASCII code when idle, code 32, period 5: `cat -v` shows a space every 5 s of quiet. | |
| 13 | Settings > Profiles > General > Badge: `\(session.hostname)` (or plain text `BUILD`): the text shows large, red and half transparent, at the pane's top right, behind the text, and shrinks when the pane gets smaller. | |
| 14 | `printf '\e]1337;SetBadgeFormat=%s\a' "$(printf 'hello' | base64)"` changes this session's badge to "hello". With `\(f())` a bar says the badge was not updated (function calls are not allowed). | |
| 15 | Triggers editor, Match Against "Command Finished", Exit code `!0`, action Show Alert `failed with \0`: with shell integration, `false` shows "failed with 1"; `true` shows nothing. | |
| 16 | Match Against "Idle (Silence)", quiet 5 s, Show Alert `quiet \0`: 5 s after output stops the bar reads "quiet 5", once per quiet spell. | |
| 17 | Match Against "Custom Escape Sequence", sequence id `me`, Show Alert `\1`: `printf '\e]1337;Custom=id=me:hello\a'` shows "hello". | |
| 18 | A Ring Bell trigger follows the bell settings above (silenced when Silence bell is on). | |
