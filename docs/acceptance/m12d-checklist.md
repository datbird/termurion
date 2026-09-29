# Milestone 12d acceptance checklist (rows so far)

Task 11 completes this file in the m10b form; the rows below were written by the tasks that
cannot test them without a window. Build a release binary (`scripts/xt build --release`), close any
open Tandem window, copy `tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and
`OpenConsole.exe` together, then run `tandem.exe`. Setup: an API key for one provider from 12a,
"Enable generative AI features" on.

| # | Check | Result |
|---|---|---|
| 1 | With the chat window open, start Ollama discovery (or wait for it) so it finds a model that was not listed: the model popup lists it without reopening the window. | |
| 2 | Type a word in the search field: after a short pause results appear as small bubbles (yours at the right in the accent color, the agent's at the left in gray) with the matches strong and underlined. A snippet that held markdown (inline code, a link) shows as raw text there, a known gap. | |
| 3 | Search a chat that cannot be read (for example while the database is locked): the banner reads "The search could not be completed." with no store text. | |
| 4 | Click a search result: its chat shows, the search field keeps its text and results, and the matching message is centered in the conversation. A result whose message is hidden (a command's answer) shows the chat and does not scroll. | |
| 5 | A conversation shows a centered date row before each day ("Today", "Yesterday", a weekday, "Sep 3", "Sep 3, 2025"), your messages at the right in the accent color with white text, the agent's at the left in a faint fill, each bubble at most 70 percent wide, the short time beside it, and system rows (notices, offers, watcher events) full width and muted. | |
| 6 | Reply markdown renders: headings, lists, bold, italic, inline code, a fenced code block with a copy glyph at its first line that copies the block and shows "Copied" for about a second; two fenced blocks with only blank lines between them copy as one. | |
| 7 | A reply containing `[site](https://example.com)`, `[file](file:///C:/Windows/notepad.exe)`, `[share](\\host\share)` and `[x](iterm2:unlink-session-chat)`: only the web link is underlined and opens the browser; the others are plain text and do nothing when clicked. | |
| 8 | A command request flagged by the safety check shows the warning glyph before its text. | |
| 9 | Right-click your own message: Edit, Copy, Fork, Delete. Right-click the agent's: Copy only. Edit removes that message and everything after it (the text goes to the input once Task 7 lands); Delete removes the same; Copy puts the rendered text on the clipboard. | |
| 10 | Fork a message: a new chat opens holding the messages before it, titled "<title> (Forked at <date> <time>)"; forking that chat again replaces the suffix instead of adding a second one. The fork keeps the source's icon and linked session. | |
| 11 | A multipart reply with a file attachment shows the file's glyph and name; right-click offers "Reveal in Explorer" (and "Open" only for txt, md, json, csv, log, pdf, png, jpg, gif or webp). Reveal shows the file under `%TEMP%\TandemChatAttachments\<chat>\<attachment>\`. A `.docm`, `.rdp` or `.html` attachment offers Reveal only. | |
| 12 | A code-interpreter attachment is a box titled "Code Interpreter" with its own copy glyph. | |
| 13 | While the agent is typing a spinner shows under the last message; a new message scrolls the view to the bottom only when it was at the bottom. | |
