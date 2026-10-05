# Milestone 13r acceptance checklist

Build a release binary (`scripts/xt build --release`), close any open Tandem window, copy `tandem.exe`,
`tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Task 10 fills in the
rest of this list; the rows below are recorded as the fix rounds land. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| R1 | Ruling (iTerm2 parity): a trigger with action Invoke Script Function whose script calls `iterm2.run_coprocess(commandLine: ...)` with a capture runs that command line as the script wrote it; Tandem does not refuse it (the person's own installed script). Every Tandem-built command line (a Run Command trigger `run \1` on output `x&calc`) is refused with the banner and calc never starts. | |
| R2 | `UrlHandlerCommand` = `explorer.exe \(url)`: Win-click an https link opens it in the browser and no "URL Handler Command Failed" banner shows (explorer.exe exits 1); its Script Console entry "URL Handler" notes the non-zero status. `UrlHandlerCommand` = `nothere.exe \(url)`: the banner shows. | |
| R3 | `UrlHandlerCommand` = `cmd.exe /c start "" \(url)` (or `conhost`, `wt`, `rundll32 ...`): any link click shows "its program hands its arguments to a shell or another program" and nothing starts. | |
| R4 | `UrlHandlerCommand` = `\(session.path)\open.exe \(url)`: refused, "a session variable cannot name the program". With a tab title holding spaces, `C:\tools\open.exe --title=\(session.name) \(url)` passes the title as one argument; a title holding `&` is refused. | |
| R5 | `fileDropCoprocess` = `type \(filenames)`: drop a file named `a&calc&.txt` on a pane: the refusal banner shows and calc never starts; a file named `notes 1.txt` runs `type "...notes 1.txt"`. | |
| R6 | A Capture Output trigger, regex `error: (.*)`, parameter `notepad \1`, with shell integration: `echo "error: x&calc"` is not captured with a runnable command (the refusal banner shows); `echo "error: x"` captures, and a double-click in Captured Output opens Notepad on `x`. | |
