# Triggers

A trigger watches a session's output for text that matches a regular expression, or for an event, and then does something: highlight the text, ring the bell, show an alert, send text, run a command, set a mark, and more. Triggers belong to a profile.

## Add a trigger

1. Open Settings (Ctrl+Shift+,), choose Profiles, pick a profile, and open the Advanced tab.
2. In the Triggers section, press +.
3. Fill in the fields below the list.

You can also select some text in a session and choose Session > Triggers > Add Trigger.... Termurion adds a trigger whose regular expression is that text, with special characters escaped, and opens the editor on it.

The Triggers section has these fields:

- **Name.** A label for the list.
- **Match Against.** "Regular Expression", or one of the events listed below.
- **Regular expression.** The pattern to look for. See [Search Syntax](search-syntax.md) for regular expression notes.
- **Job.** Fire only while this program is in front, for example `emacs`. Leave it empty for any program.
- **Action** and **Parameters.** What to do. The parameter's meaning depends on the action.
- **Instant.** Check the line while it is still being written, without waiting for the end of the line.
- **Enabled.** Turn the trigger off without deleting it.

Press - to delete the selected trigger. Right-click a trigger to export it. Import reads `.it2triggers` files, which are the same files iTerm2 uses.

## When a trigger runs

For a regular expression trigger, Termurion checks each logical line when it ends. An Instant trigger also checks the line that is still being written. Triggers run in list order. The "Stop Processing Triggers" action ends the pass so triggers below it do not run for that text.

Termurion limits how long one pattern may take on a line, so a runaway pattern cannot freeze the window.

By default, triggers keep firing inside full-screen programs. To change this for the current session, use Session > Triggers > Enable Triggers in Interactive Apps. The profile setting is Settings > Profiles > Advanced > Triggers > Enable triggers in interactive apps.

Session > Triggers also has Enable All and Disable All, and Edit Triggers, which opens the editor.

## Actions

Termurion offers these actions:

| Action | What it does |
|---|---|
| Annotate... | Adds an annotation to the matched text |
| Bounce Dock Icon | Flashes the taskbar button (once, or until you switch to the window) |
| Buffer Input... | Starts or stops holding typed input |
| Capture Output | Saves the matching line with the last command so it shows in the Captured Output tool; the parameter can name a coprocess to run when you activate an entry. Needs [shell integration](shell-integration.md). See [Captured Output](captured-output.md) |
| Change Style... | Applies SGR style codes to the text |
| Enter Workgroup... / Exit Workgroup | Changes workgroup |
| Fold to Named Mark | Folds the lines between the named mark and the match |
| Highlight Line... | Colors the whole line |
| Highlight Text... | Colors the matched text |
| Inject Data... | Pretends the program printed text (use `\e` for Esc, `\a` for Ctrl+G) |
| Invoke Script Function | Calls a function from a [Python API](python-api.md) script |
| Make Hyperlink... | Turns the match into a link, for example `https://\0` |
| Open Password Manager... | Opens the password manager for an account |
| Post Notification... | Shows a notification |
| Prompt Detected | Treats the matched line as a shell prompt and marks it |
| Report Directory | Tells Termurion the current folder |
| Report User & Host | Tells Termurion the user and host |
| Ring Bell | Rings the bell |
| Run Command... | Runs a command (through cmd.exe) |
| Run Coprocess... | Starts a coprocess that reads the session's output |
| Run Silent Coprocess... | Same, but hides its output |
| Send Text... | Types text into the session |
| Set Mark | Sets a mark, and keeps or stops scrolling |
| Set Named Mark | Sets a mark with a name |
| Set Profile Setting... | Turns a profile checkbox on or off |
| Set Tab Status... | Sets the tab's status dot, text color, and status text |
| Set Title... | Sets the session title |
| Set User Variable... | Sets a `user.` [variable](variables.md) |
| Show Alert... | Shows an alert box |
| Stop Processing Triggers | Skips the triggers below this one |

Some actions that run code or change workgroups stay off when you press Enable All. Turn each one on yourself in the editor after you read what it does.

## Using the match in the parameter

In a parameter, `\0` is the whole match and `\1` to `\8` are the capture groups. A group that did not match is empty. You can also write `\n`, `\t`, `\r`, `\e`, `\a`, `\\`, and `\xHH`.

For Run Command, Run Coprocess, and Run Silent Coprocess, the match goes onto a cmd.exe command line, so Termurion treats it as text from a program and handles it with care.

For more power, turn on "Triggers use interpolated strings" in the same Advanced tab section. Parameters are then [interpolated strings](scripting-fundamentals.md#interpolated-strings), and the captures are in the array `matches`, for example `\(matches[1])`. Function calls are not allowed in those strings. To call a function, use the Invoke Script Function action.

## Events

Set "Match Against" to an event instead of a regular expression to react to something other than text:

- Prompt Detected
- Command Finished
- Directory Changed
- Host Changed
- User Changed
- Idle (Silence)
- Activity After Idle
- Session Ended
- Bell Received
- Long-Running Command
- Custom Escape Sequence
- Notification Posted
- Progress Bar Changed
- Job Started
- Job Ended
- Variable Changed

Several of these need [shell integration](shell-integration.md) to know when a prompt or command starts and ends. Enter Workgroup and Exit Workgroup are not offered for Session Ended, because there is no live session to act on.

## Web browser profiles

A profile that is a web browser has its own trigger actions, matched against the page URL or page content: Enter Reader Mode, Highlight Text, Inject Javascript, Make Hyperlink, Reload After Delay, and the workgroup actions. See [Web Browser](web-browser.md).

## Limits

A profile can hold up to 256 triggers when you import a file. A regular expression can be 4096 characters, and a parameter 64 KB.

## Related

- [Smart Selection](smart-selection.md)
- [Scripting Fundamentals](scripting-fundamentals.md)
- [Coprocesses](coprocesses.md)
