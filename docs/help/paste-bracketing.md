# Paste Bracketing

Paste bracketing (also called bracketed paste) is a way for a program to tell the difference between text you typed and text you pasted. When a program turns it on, Termurion wraps each paste in two markers. The program can then treat the paste as plain data. A shell, for example, can show a pasted multi-line command for you to review instead of running every line as soon as it sees a newline.

## How it works

A program turns bracketing on by sending the escape sequence `ESC [ ? 2004 h`, and off with `ESC [ ? 2004 l`. While it is on, Termurion sends:

```
ESC [ 200 ~   the pasted text   ESC [ 201 ~
```

Termurion removes any Esc character from the pasted text first, so pasted text cannot contain the closing marker and break out of the bracket.

Termurion brackets a paste only when both of these are true:

- the program in the session has turned bracketing on, and
- the profile allows it (Settings > Profiles > Terminal > "Applications may use paste bracketing", on by default).

If either is false, the text goes in as plain input.

## Turn it on or off for a session

Session > Terminal State > Paste Bracketing shows whether bracketing is on for the current session (it has a check mark when on). Choosing it flips the setting. You rarely need this, but it helps when a program that crashed left bracketing on.

When [shell integration](shell-integration.md) reports that the host changed and paste bracketing was left on by the previous one, Termurion asks "Looks like paste bracketing was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?". You can have it always turn off on a host change with "Always turn off paste bracketing when host changes?" in Settings > Advanced > Warnings.

## Advanced Paste

Edit > Paste Special > Advanced Paste... (Alt+Shift+V) has a "Bracketed paste mode" checkbox. It is available only when the program in the session has turned bracketing on. Termurion remembers your choice for the next Advanced Paste.

## Why it matters

Without bracketing, a program reads a pasted newline exactly as if you had pressed Enter. A multi-line paste then runs each line at once, before you can read it. With bracketing, the program knows the whole block is a paste and can wait for you.

## Pasting tabs

When bracketing is off, a plain paste that contains tabs offers to convert the tabs to spaces. You can turn this question off with Edit > Paste Special > Prompt to Convert Tabs to Spaces when Pasting. When bracketing is on, tabs go through unchanged.

## Multi-line warnings

Bracketing is separate from the paste warnings. Termurion can still ask before you paste several lines. Edit > Paste Special has these toggles:

- Warn Before Multi-Line Paste
- Limit Multi-Line Paste Warning to Shell Prompt
- Warn Before Pasting One Line Ending in a Newline at Shell Prompt

The "shell prompt" options use [shell integration](shell-integration.md) to know when you are at a prompt.

## If pasted text runs as commands

If you paste into a program and every line runs at once, the program has not turned bracketing on, or Termurion is not using it. Check:

1. Settings > Profiles > Terminal > "Applications may use paste bracketing" is on.
2. Session > Terminal State > Paste Bracketing is checked while the program is in front. If it is not, the program has not asked for bracketing.
