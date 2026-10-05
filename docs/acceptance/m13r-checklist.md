# Milestone 13r acceptance checklist

Build a release binary (`scripts/xt build --release`), close any open Tandem window, copy `tandem.exe`,
`tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Task 10 fills in the
rest of this list; the rows below are recorded as the fix rounds land. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| R1 | Ruling (iTerm2 parity): a trigger with action Invoke Script Function whose script calls `iterm2.run_coprocess(commandLine: ...)` with a capture runs that command line as the script wrote it; Tandem does not refuse it (the person's own installed script). Every Tandem-built command line (a Run Command trigger `run \1` on output `x&calc`) is refused with the banner and calc never starts. | |
