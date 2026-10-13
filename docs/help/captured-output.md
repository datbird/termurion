# Captured Output

Captured Output collects lines from a command's output and lists them in a tool on the Toolbelt. You set up a Capture Output trigger with a regular expression. Each line that matches becomes an entry. Click an entry to jump to that line in the session. Double-click an entry to run a command on it.

A typical use is a compiler. A trigger matching `^(.+):(\d+): error` collects every error line from a build. You can then step through them without scrolling.

## Example

You run a build that prints lines like `src\main.rs:42: error: missing semicolon`. You want to jump between the errors.

1. Add a Capture Output trigger with the regular expression `^(.+):(\d+): error`.
2. Leave the Parameter empty, or enter a coprocess command such as `code -g \1:\2` to open the file at that line when you double-click the entry.
3. Run the build with shell integration active. Every matching line appears in the Captured Output tool.
4. Click an entry to see the line in the session.

## What you need

- A Capture Output trigger on the session's profile. See [Triggers](triggers.md).
- Shell integration installed in the shell. Captures belong to the command that printed them, and Termurion finds the command through shell integration marks. See [Shell Integration](shell-integration.md). Without it, Termurion shows "A Capture Output trigger fired, but Shell Integration is not installed."
- The Captured Output tool must be visible. If it is not, Termurion shows "A Capture Output trigger fired, but the Captured Output tool is not visible."

Each of these warnings can be silenced.

## Show the tool

Choose View > Toolbelt > Show Toolbelt. With the default Shortcut Key (Ctrl+Shift), the shortcut is Ctrl+Shift+Alt+B. Then turn on Captured Output in the Toolbelt's tool list. You can change the shortcut key in Settings > Keys.

## Make a trigger

1. Open Settings (Ctrl+Shift+,) and go to Profiles > Advanced > Triggers, then Edit.
2. Add a trigger. Set the Regular Expression to match the lines you want.
3. Set Action to "Capture Output".
4. The Parameter is optional. It is a coprocess command to run when you double-click an entry. Use `\1`, `\2`, and so on for the parts of the match that the regular expression captured.

## Using the tool

- Each entry begins with a check mark or a dash (a dash means not yet checked).
- Click an entry to scroll the session to its line. The session takes the keyboard.
- Double-click an entry to run its coprocess command. It runs as a normal coprocess, not silent. If a coprocess is already running in the pane, it does not start, and you see "Captured Output trigger: Can't run two coprocesses at once." See [Coprocesses](coprocesses.md).
- Right-click an entry and choose Toggle Checkmark to track which entries you have handled.
- Type in the search field to filter the list. The match ignores case.
- The Clear button (trash can) hides the current entries. It is disabled when the list is empty. New captures after the clear still appear.
- The Help button opens this page.

A line that grows as it prints (a progress line) merges into one entry instead of creating many.

## Clear from a program

A program can clear the list by sending the escape sequence `OSC 1337 ; ClearCapturedOutput`. The tool then shows nothing until new captures arrive.

## Related pages

- [Triggers](triggers.md)
- [Coprocesses](coprocesses.md)
- [Shell Integration](shell-integration.md)
