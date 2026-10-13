# Badges

A badge is a large, semi-transparent label drawn in the top right corner of a pane. Use it to show which host, user, or environment a session belongs to, such as "prod" or "build box". The badge sits behind the text and stays put while the text scrolls.

## Set a badge

Open Settings with Ctrl+Shift+, and go to Profiles > General. In the Badge section, type the text into "Badge text". The text is an interpolated string, so it can refer to session variables:

```
\(session.hostname)
\(session.username) on \(session.hostname)
```

A newline in the text starts a new line in the badge. See [Variables](variables.md) for the names you can use and [Scripting Fundamentals](scripting-fundamentals.md#interpolated-strings) for the syntax.

If the text refers to a variable that does not exist, or has an error, the badge shows "Error:" and the reason, so you can see what went wrong.

A badge shows at most 300 characters. The text is cleaned of control characters before it is drawn.

## Set a badge from a program

A program can set the badge of the session it runs in with an escape sequence:

```
ESC ] 1337 ; SetBadgeFormat = <base64 of the text> BEL
```

For example, in PowerShell:

```powershell
$b = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('\(session.hostname)'))
Write-Host -NoNewline "$([char]27)]1337;SetBadgeFormat=$b$([char]7)"
```

The change applies to that session.

For safety, a badge that calls a function (anything like `\(f())`) is refused. Termurion shows "The application attempted to set the badge to a value that would invoke a function call. For security reasons, this is not allowed and the badge was not updated." Plain variables are allowed.

A program can also change the badge color with `SetColors=badge=<hex color>` (OSC 1337 SetColors).

## Appearance

These are in Settings > Profiles:

| Setting | Where | Meaning |
|---|---|---|
| Badge font | General > Badge | The font. The default is Helvetica Bold. |
| Badge top margin | General > Badge | Space above the badge, in points. |
| Badge right margin | General > Badge | Space to the right of the badge, in points. |
| Badge max width | General > Badge | The most of the pane's width the badge may use (0 to 1). |
| Badge max height | General > Badge | The most of the pane's height the badge may use (0 to 1). |
| Badge Color | Colors > Other Colors | The color, with alpha. The default is red at 50 percent. |

The defaults for margins, size, and font come from Settings > Advanced, section Badge: "Font to use for the badge", "Should the badge render in bold type?", "Maximum width of the badge" (0.5), "Maximum height of the badge" (0.2), and the default top and right margins (10 points each).

Termurion picks the largest font size from 4 to 100 points at which the wrapped text fits inside the maximum width and height. A longer badge text therefore gets a smaller font.

## Related pages

- [Variables](variables.md)
- [Scripting Fundamentals](scripting-fundamentals.md)
- [Status Bar](status-bar.md)
