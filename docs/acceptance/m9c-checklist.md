# Milestone 9c acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Use a shell with Termurion's shell integration (PowerShell or WSL bash). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Before any shell integration was used (fresh settings): Session > Open Command History... shows the "About Shell Integration" bar with Learn More..., OK, Install Now. | |
| 2 | Run `git status`, `git log -1`, `ls` a few times. Type `gi` and choose Session > Open Command History... (Shift+Win+;): the git commands show with "Last Used"; Enter on `git status` types only `t status`. | |
| 3 | With nothing typed, the list is newest first. | |
| 4 | `cd` to three folders. Session > Open Recent Directories... (Alt+Win+/): they show; Enter at the prompt types the chosen path, with spaces escaped. | |
| 5 | Settings > General: "Save copy/paste and command history to disk" on. Quit and start Termurion: the command history and directories are still there. | |
