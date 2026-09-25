# Milestone 9d acceptance checklist

Close any open Tandem window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.
Use a shell with Tandem's shell integration. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `git status` so "status" is on screen. Type `git sta` and choose Session > Open Autocomplete... (Win+;): a list at the cursor shows `statu`-style suffixes with `tus` first; Enter types `tus`. | |
| 2 | Type `git ` (with the space) and open Autocomplete: the list offers `status` (the word that followed). | |
| 3 | With the list open, Tab adds the chosen text and searches again ("more"); Shift+Tab goes back ("less"); Esc closes; typing any other key closes it and the key reaches the shell. | |
| 4 | View > Auto Command Completion (Shift+Alt+Win+Y) on (check mark). Type `gi` at the prompt: after a moment a list of history commands starting with `gi` shows under the cursor with "Press Shift+Enter or Alt+Enter to send command.". | |
| 5 | Keep typing: the list follows. Up/Down choose. Shift+Enter types the rest; Alt+Enter types it and runs it; plain Enter runs what you typed and closes the list. | |
| 6 | Turn Auto Command Completion off: no list appears while typing. | |
