# Milestone 9b acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Copy three different selections, paste one. Session > Open Paste History... (Shift+Ctrl+Win+H): a "Paste History" window lists them newest first with "N lines, size, date". | |
| 2 | Type a few letters of one entry (in order, not next to each other): the list narrows and the letters show bold. Backspace clears the filter. Enter pastes the chosen entry. Esc closes. | |
| 3 | Copy text with tabs and newlines. Edit > Paste Special > Advanced Paste... (Alt+Win+V): the source list shows earlier entries and the clipboard; the preview shows the text. | |
| 4 | Turn on "Replace" with `foo` -> `bar`: the preview changes. Turn on "Base-64 encode": the preview becomes base64. "Minimum duration" follows the Chunk Size slider. Paste sends the preview. | |
| 5 | Open Advanced Paste again: the choices you left on come back (they are remembered). | |
| 6 | Paste three lines at a prompt; in the question choose "Advanced...": Advanced Paste opens with those lines. | |
| 7 | Settings > General: turn on "Save copy/paste and command history to disk", copy something, quit, start Termurion: Open Paste History still lists it. | |
