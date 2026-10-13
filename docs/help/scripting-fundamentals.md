# Scripting Fundamentals

Termurion has a small expression language that appears in several places: badge text, window and tab titles, status bar components, key bindings, and triggers. You write text with expressions inside it, and Termurion fills in the values. Expressions can read [variables](variables.md) and, in some places, call functions that a [Python API](python-api.md) script has registered.

## Interpolated strings

An interpolated string is ordinary text with expressions wrapped in `\( )`. Termurion replaces each `\( )` with the value of the expression inside.

```
Host: \(hostname)  Folder: \(path)
```

If `hostname` is `box` and `path` is `C:\Users\me`, this shows `Host: box  Folder: C:\Users\me`.

Some rules:

- Text outside `\( )` is kept as is, with the usual escapes: `\n` (newline), `\t` (tab), `\r`, `\e` (escape), `\\` (a backslash), and `\"`.
- A `\(` with no closing `)` is treated as plain text.
- An expression can be as long as you like and can contain more expressions in strings: `"v=\(1 + 1)"` is `v=2`.
- A name with no dot also tries the `session.` scope, so `\(hostname)` and `\(session.hostname)` give the same result.

### Where interpolated strings are used

- **Badge text.** Settings > Profiles > General > Badge. See [Badges](badges.md).
- **Window and tab titles.** Settings > Profiles > Window > Title, when you use a custom title.
- **Status bar.** The Interpolated String component shows the result, and the Call Script Function component runs a function when you click it. See [Status Bar](status-bar.md).
- **Triggers and Smart Selection actions.** Off by default. Turn on "Triggers use interpolated strings" or "Smart Selection actions use interpolated strings" in Settings > Profiles > Advanced. See [Triggers](triggers.md) and [Smart Selection](smart-selection.md).
- **Key bindings.** The "Invoke Script Function..." action calls a function, and "Copy Interpolated String" puts the result on the clipboard.
- **Starting folder.** A profile's custom starting folder is evaluated as an interpolated string. A result that is not a local folder is refused and the session starts in your home folder.
- **Dropped files.** The silent coprocess run when you drop files into a window is an interpolated string where `\(filenames)` is the shell-quoted list of paths.

Not every place allows function calls. Badges, triggers, Smart Selection actions, and Copy Interpolated String accept variables and operators but refuse calls. Status bar strings and the Invoke Script Function action accept calls.

### Undefined variables

By default, a reference to a variable that does not exist becomes an empty string, so `\(bogus)` is empty. Turn this off in Settings > Advanced > Scripting ("Should references to undefined variables in interpolated strings be converted to empty string?") to get an error instead. Even with it on, an undefined variable used as a function argument is still an error. Add `?` after a name to say a missing value is fine: `\(user.thing?)`.

## Expressions

The expression language works like a small calculator.

- **Numbers, strings, `true`, `false`, and `null`.** Strings use double quotes.
- **Variables.** Names with dots, such as `session.path` or `tab.title`. An array element is `user.list[1]`.
- **Arithmetic.** `+ - * /` and parentheses. A string that reads as a number counts as a number, so `"80" + 1` is `81`.
- **Comparison.** `== != < > <= >=`.
- **Logic.** `&& || !`. These give numbers (1 or 0), not words.
- **Conditions.** `a ? b : c`. The test must be a number or `null`.
- **Arrays.** `[1, "x"]`, but only as a whole expression, for example as a function argument.
- **Fallback.** Termurion adds one extension: `a ?? b ?? c` inside `\( )` gives the first alternative that is not empty.

An error shows a message that says what is wrong, for example "Arithmetic operations require numeric operands".

## Calling functions

A function call has a name and named arguments:

```
name(arg1: value, arg2: value)
```

A function from a script has a namespace, for example `mytool.greet(name: "Ada")`. Termurion's own functions use the namespace `iterm2`. Pass the name of a user variable with `&` to give the function a reference instead of its value: `f(b: &user.x)`.

### Built-in functions

These are available without any script:

| Function | What it does |
|---|---|
| `iterm2.alert(title, subtitle, buttons)` | Shows an alert with buttons |
| `iterm2.get_string(title, subtitle, placeholder, defaultValue)` | Asks for a line of text |
| `iterm2.count(array)` | Number of items in an array |
| `iterm2.urlEncode(string)` | Percent-encodes text |
| `iterm2.with_alpha(color, alpha)` | A color with a given alpha |
| `iterm2.get_profile_property(key)` | Reads a profile setting of the session |
| `iterm2.focus()` | Focuses the session |
| `iterm2.paste()` | Pastes into the session |
| `iterm2.set_status(...)` | Sets the session's status |
| `iterm2.get_background_task_count()` | Number of background tasks |
| `iterm2.add_clipping(type, title, detail)` | Adds a clipping |
| `iterm2.archive_clippings()` | Archives clippings |
| `iterm2.move_tab_to_window()` | Moves the tab to its own window |
| `iterm2.set_status_bar_component_unread_count(identifier, count)` | Sets a status bar count |

Some functions are methods on a session, tab, or window: `set_name`, `run_tmux_command`, `run_coprocess`, `stop_coprocess`, `get_coprocess`, and `get_time_offset` on a session; `set_title` and `select_pane_in_direction` on a tab; and `set_title` on a window.

### Functions from scripts

A [Python API](python-api.md) script can register functions with the `iterm2` Python package. Once registered, you can call them from the places above. The Scripting Inspector (open it from Scripts > Manage > Console > Inspector) lists the registered functions and the script that owns each one. It also lets you browse [variables](variables.md).

Scripts can also register title providers, status bar components, and context menu items.

## Related

- [Variables](variables.md)
- [Python API](python-api.md)
- [Triggers](triggers.md)
