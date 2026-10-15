# Smart Selection

Smart Selection picks the piece of text under the pointer that looks like something: a URL, a file path, an email address, a quoted string. It uses a list of regular expressions called rules. Each rule has a precision, and you can attach actions to a rule, such as opening a URL or copying a value.

## Use it

Quadruple-click text in a terminal session. Termurion tests every rule against the text near the click and selects the best match. If no rule matches, it selects a word.

To swap the clicks, turn on Settings > Pointer > Selection > Double-click performs smart selection. A double click then does Smart Selection and a quadruple click selects a word. A triple click always selects a line.

To select every match on screen at once, choose Edit > Find > Find All Smart Selection Matches (Win+Alt+Shift+U).

Pointer actions named "Smart Selection" and "Smart Selection Ignoring Newlines" are also available for pointer bindings.

## How the best match is chosen

For every rule, Termurion finds matches that cover the clicked character. Each match gets a score: the rule's precision multiplied by the match length in characters. The highest score wins. If two scores tie, the earlier rule in the list wins. A rule whose regular expression does not compile is skipped.

The precision levels and their multipliers are:

| Precision | Multiplier |
|---|---|
| Very low | 0.00001 |
| Low | 0.001 |
| Normal | 1 |
| High | 1000 |
| Very high | 1,000,000 |

A long match with Low precision can still lose to a short match with High precision. That is why the built-in email and URL rules are High and Very high: they should beat the general "path" and "word" rules that match the same text.

## The built-in rules

A profile that has no rules of its own uses this list:

- Word bounded by whitespace (low)
- C++ namespace::identifier
- Paths
- Quoted string
- Java/Python include paths
- mailto URL
- Obj-C selector (high)
- email address (high)
- URLs (http and https)
- SSH URL (very high)
- Telnet URL (very high)

The built-in rules have no actions.

## Edit the rules

Rules belong to a profile. Open Settings (Ctrl+Shift+,), choose Profiles, pick a profile, then open the Advanced tab and find the Smart Selection section.

- Press + to add a rule, or select a rule and press - to remove it. Load Defaults puts the built-in rules back.
- Select a rule, then set its Name, Regular expression, and Precision (Very Low to Very High).
- Syntax Help opens this page. Open Regular Expression Visualizer draws the expression as a railroad diagram, as iTerm2 does, in a popover; the same button (now Close Regular Expression Visualization) closes it. Without the WebView2 runtime the page opens in your browser.
- Click in the sample text at the bottom to see which rule wins at that spot and what it matches.
- If the expression does not compile, the editor shows the error in red and the rule is skipped until you fix it.

Expressions are ICU style. Termurion reads `[:letter:]` and `[:number:]` classes as Unicode letters and numbers.

## Actions

Select a rule and press Actions…, then +. Each action has a title, a type, and a parameter. The types are:

- Open File
- Open URL
- Run Command
- Run Coprocess
- Send Text
- Run Command in Window
- Copy

In a parameter, `\0` is the whole match and `\1` to `\8` are the capture groups. `\d` is the current folder, `\h` the host, `\u` the user, `\n` a newline, and `\\` a backslash. Turn on Smart Selection actions use interpolated strings in the same section to evaluate the parameter as an [interpolated string](scripting-fundamentals.md#interpolated-strings) instead.

Actions appear in the context menu when you right-click a selection shorter than 400 characters. A Win-click (a click with the shortcut key held, Ctrl+Shift by default) on matching text runs the rule's first action. To stop that, turn on "Disable performing the first matching Smart Selection's Action on Win-click" in Settings > Advanced > Semantic History. To check Smart Selection actions before existing files on a Win-click, turn on "Check smart selection actions before existing files on Win-click?" in the same section.

## Related

- [Triggers](triggers.md) also use regular expressions, but they watch new output instead of a click.
- [Search Syntax](search-syntax.md)
