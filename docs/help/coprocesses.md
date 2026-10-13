# Coprocesses

A coprocess is a program that runs beside a terminal session and is wired to it. Everything the session prints goes to the coprocess's input. Everything the coprocess prints goes to the session as if you had typed it. This lets a small program watch a session and answer it, for example to log in, answer prompts, or react to output.

## Start and stop

- Start: Session > Run Coprocess...
- Stop: Session > Stop Coprocess

The Run Coprocess window asks "Coprocess command to run:". It keeps your last 20 commands in a list. The check box "Silently ignore errors from this command" suppresses the stderr banner (see below) for that command.

A pane can run one coprocess at a time. If one is already running, the menu item is disabled, and other ways of starting one show a banner that says to stop it first.

You can also bind a key to the action Run Coprocess in Settings > Keys.

## How the command runs

Termurion runs the command with `cmd.exe /d /s /c <command>`, hidden, in the pane's current folder (your home folder if the pane's folder is not a local one). Each coprocess runs inside a Windows Job Object. Stopping it ends every program it started.

A coprocess has three pipes: standard input (the session's output), standard output (sent to the session as keyboard input), and standard error (kept for you to read).

## Example

Suppose a long installer keeps asking "Continue? (y/n)". A coprocess can answer for you. Write a small script, `answer.ps1`, that reads the session's output line by line from standard input. When a line contains "Continue?", it prints `y` followed by a newline to standard output. Then choose Session > Run Coprocess... and enter:

```
powershell -NoProfile -File C:\tools\answer.ps1
```

The script sees what the installer prints. What the script prints reaches the installer as typed text. Choose Session > Stop Coprocess when you are done. Keep the script's output small and flush it, so the session gets the answer at once.

## Indicator

While a coprocess runs, an indicator shows in the top right of the pane. The Advanced setting "Disable indicator icons that appear in the top right of a session?" hides it.

## Silent coprocesses

A silent coprocess (Run Silent Coprocess, available in triggers) works like a normal one. While it runs, the session's output is not drawn on screen. Use it when the coprocess consumes the output and you do not want to see it.

## Errors

If the coprocess ends having written to standard error, a bar appears: "terminated with output on stderr". It has two buttons:

- View Errors: saves the error text to a file and opens it. Termurion keeps up to 100,000 characters of stderr.
- Ignore Errors from This Command: stops the bar for this command from now on.

If a coprocess does not read its input fast enough, Termurion queues up to 4 MB of terminal output for it. Past that, output is dropped and a banner says so. Coprocess output flows to the session only as fast as the session reads it.

## Other ways to start one

- Triggers: the actions "Run Coprocess..." and "Run Silent Coprocess..." start a coprocess when text matches. If one is already running, the trigger shows "Can't run two coprocesses at once" with a Silence Warning button. See [Triggers](triggers.md).
- Smart Selection: an action of type Run Coprocess starts one for the selected text. See [Smart Selection](smart-selection.md).
- Semantic history: the "coprocess" action runs a command for a clicked file.
- Dropping files: the Advanced setting "When files are dropped into a terminal window, execute a silent coprocess." takes a command. Use `\(filenames)` for the quoted, space-separated paths. If the setting is empty, the file names are typed at the prompt instead. A dropped name that holds characters cmd.exe would act on (such as `&`, `%`, or parentheses) is refused and nothing runs.
- Captured Output: see [Captured Output](captured-output.md).
- Python API: a script can run a coprocess. See [Python API](python-api.md).

## Safety

A coprocess command runs as you, with your permissions. Do not put text you did not write into a coprocess command. Termurion checks the commands it builds from dropped files and from Smart Selection captures, but a command you type is yours.

## Related pages

- [Triggers](triggers.md)
- [Smart Selection](smart-selection.md)
- [Captured Output](captured-output.md)
