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
| 14 | A request bubble (Run Commands at Ask) shows four full-width 30-point rows under its text: "Allow Once", "Always Allow", then "Deny this Time" and "Always Deny" in the error color, each with a line above it. Press one: all four grey at once. Closing the chat window, or a new message arriving, never presses one. | |
| 15 | While a request is the last message its buttons are live; once any newer message follows (or the request was answered from elsewhere) they are greyed and pressing does nothing. Answer Always Allow: the next call of that category runs without asking; Always Deny: it is refused without asking. | |
| 16 | A "Select a Session" bubble shows "Select a Session" and "Cancel". Press Select a Session: the chat shows "Waiting for a session to be selected..." with a Cancel button, and every terminal pane of every window (including a window that was behind) shows a centered "Link this session to AI chat?" button over its text. | |
| 17 | While the picker is open, a click elsewhere in a terminal keeps it waiting; Escape in a terminal window, or Cancel in the chat, removes every picker button and declines the request. Press the button on a pane: the chat links to that pane (notice "This chat has been linked to terminal session ..."), the permissions bubble appears, and the parked request runs against that pane. | |
| 18 | Stop a turn (or let it end) while a select-session request waits and the picker is open: the picker buttons go away by themselves within a moment. Close the chat window while the picker is open for a request: the picker goes away and the request is declined. | |
| 19 | A new chat made from a session shows "Link" and "Enable Orchestration". "Link" links that session; afterwards both grey. Close the session first: both are grey. "Enable Orchestration" is grey with the tooltip "Orchestration comes in milestone 12f." (also "Not Now", "Approve", "Deny" and "Revoke" on their bubbles). | |
| 20 | The permissions bubble lists seven rows ("AI can Check Terminal State: Provided automatically" when Always, and so on for Run Commands, View Contents, Write to the Clipboard, Control Terminal, View Manpages, Write to the File System). It stays tappable after a press (Task 6 makes a press cycle the permission and ask Confirm Change); closing the linked session greys it. | |
| 21 | A terminal titled with a backtick or a line break (`printf '\e]0;a`b\nc\a'`) links: the notice shows the title on one line with the backtick backslash-escaped. | |
| 22 | While a command runs for the chat the bubble shows "Cancel" (live only while it runs); press it: the command is canceled and the turn goes on. | |
