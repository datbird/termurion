# Milestone 9a acceptance checklist

Close any open Tandem window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.
Use a WSL bash tab with shell integration unless a line says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Copy three lines of text. Paste at the prompt: a bar asks "Confirm Multi-Line Paste: OK to paste 3 lines at shell prompt?" [Paste] [Cancel] [Advanced...]. Paste sends them. | |
| 2 | Copy `ls` plus a newline. Paste at the prompt: the bar offers "Paste Without Newline", which pastes `ls` only. | |
| 3 | Edit > Paste Special: uncheck "Warn Before Multi-Line Paste". Paste three lines: no question. Check it again. | |
| 4 | Run `cat -v`, then paste text with a tab while bracketing is off (`printf '\e[?2004l'` first): the tabs question shows; "Convert tabs to spaces" pastes 4 spaces. | |
| 5 | Copy a 2 MB text file's contents; paste into `cat > /dev/null`: a "Pasting... N%" bar with Cancel shows and counts up; Cancel stops it. | |
| 6 | Edit > Paste Special > Paste Slowly: the text arrives a few characters at a time. Paste Faster: a toast "Pasting at up to ... KB/sec"; Paste Slower likewise. Hold Alt in the menu for the Slowly forms. | |
| 7 | Copy a file in Explorer (Ctrl+C on the file). Edit > Paste Special > Paste File Base64-Encoded into `base64 -d > out; cmp out original` (end with Ctrl+D): no difference. | |
| 8 | Settings > Keys: bind a key to "Paste..." with Base64 on. Press it with text copied: base64 lines arrive. The key list shows "Base64, ..." as its summary. | |
| 9 | Copy text with an ESC and a Ctrl+G in it (for example from a file). Paste into `cat -v`: the controls are gone, tabs and newlines stay. | |
| 10 | In a tmux pane (8b), bind Shell > tmux > Send tmux Command (key action 65) to a key with `new-window`: it now works (fixes 8e). | |
