# Search Syntax

Find looks for text in a session's screen and scrollback. You can search for plain text or use a regular expression. This page explains the search modes, the regular expression syntax, and the related commands: Find, Filter, Find Globally, and Find URLs.

## Open Find

Choose Edit > Find > Find... (Ctrl+Shift+F). Type in the find bar. Termurion searches as you type and highlights the matches.

| Command | Menu | Keys |
|---|---|---|
| Find... | Edit > Find > Find... | Ctrl+Shift+F |
| Find Next | Edit > Find > Find Next | Ctrl+Shift+G |
| Find Previous | Edit > Find > Find Previous | Ctrl+Shift+Alt+G |
| Use Selection for Find | Edit > Find > Use Selection for Find | Ctrl+Shift+E |
| Clear Find | Edit > Find > Clear Find | Win+Alt+Shift+E |
| Find Globally... | Edit > Find > Find Globally... | Ctrl+Shift+Alt+F |
| Select Matches | Edit > Find > Select Matches | Alt+Shift+Enter |
| Find URLs | Edit > Find > Find URLs | Alt+Shift+U |
| Filter | Edit > Find > Filter | Win+Alt+Shift+F |

By default, Find Next searches toward older text (up) and Find Previous searches down. This is iTerm2's behavior. To get the usual Windows direction, turn off "Swap Find Next and Find Previous" in Settings > Advanced > General.

For a one- or two-character query, Termurion waits a moment before it searches. The wait is "Time to wait before performing Find action on 1- or 2- character queries" in Settings > Advanced > General.

## Search modes

The find bar has a mode menu. The mode decides how the query is read.

| Mode | The query is | Case |
|---|---|---|
| Smart Case | Plain text | Ignores case unless the query has a capital letter |
| Case Sensitive | Plain text | Must match exactly |
| Case Insensitive | Plain text | Ignores case |
| Case-Sensitive Regex | Regular expression | Must match exactly |
| Case-Insensitive Regex | Regular expression | Ignores case |

Smart Case is the default. In the plain text modes, every character means itself. A `.` is a period and `*` is an asterisk.

## Regular expressions

In the two regex modes, the query is a regular expression in the Rust `regex` syntax. This is close to the syntax in most languages. A few things to know:

- `.` matches any character, `\d` a digit, `\w` a word character, `\s` white space.
- `^` and `$` match the start and end of a line of text.
- `[abc]`, `[a-z]`, and `[^abc]` are character classes.
- `*`, `+`, `?`, and `{2,5}` repeat, and `|` means "or".
- `( )` groups, and `(?: )` groups without capturing.
- `\b` matches the edge of a word.
- Lookahead, lookbehind, and backreferences are not supported in Find. A pattern that uses them is an error.

An expression that does not compile shows the query in the find bar's error color, and nothing is highlighted.

## What a match can cover

Termurion searches each logical line. A line that wrapped onto several screen rows is one logical line, so a match can cross the wrap. A match cannot cross a real line break. An empty match is ignored.

## Filter

Edit > Find > Filter hides every line that does not match the query. The query can be plain text or a regular expression, using the same modes. The view updates live as new text arrives.

## Search one command's output

With [shell integration](shell-integration.md) installed, you can click a command to select it, and Find then searches only that command's output. The find bar says "Searching only selected command" when it does. See Edit > Select Output of Last Command and Edit > Select Current Command.

## Find Globally

Edit > Find > Find Globally... searches every session at once and groups the results by session. From three characters on it searches as you type. A shorter query waits for Return.

## Find URLs and Smart Selection matches

Edit > Find > Find URLs highlights the web addresses on screen. The expression it uses is in Settings > Advanced ("Regular expression for Find URLs command"). Edit > Find > Find All Smart Selection Matches highlights everything that a [Smart Selection](smart-selection.md) rule recognizes.

## Search history

The status bar Search tool keeps a history of searches. Its menu has Erase Search History.

## Related

- [Triggers](triggers.md) use regular expressions on new output.
- [Smart Selection](smart-selection.md)
- [Copy Mode](copy-mode.md) opens the find bar with `/`.
