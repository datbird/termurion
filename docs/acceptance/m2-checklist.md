# Milestone 2 acceptance checklist

Run `%USERPROFILE%\src\termurion\target\release\tandem.exe`. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Ctrl+Shift+, opens Settings: a profile list on the left, iTerm2's eight tabs across the top. | |
| 2 | Colors tab: change the background color. Every open pane using that profile changes at once. | |
| 3 | Text tab: change the cursor to Vertical Bar and turn on Blinking Cursor. The cursor changes and blinks. | |
| 4 | Type a new font size in Normal Font (for example `Cascadia Mono 14`) and press Enter. The text resizes once, not on every keystroke. | |
| 5 | Switch Windows between light and dark mode. The terminal colors follow, as iTerm2 follows macOS. | |
| 6 | "Import from iTerm2..." with your Mac's `com.googlecode.iterm2.plist` (or a Dynamic Profiles JSON). Your profiles appear with their own colors and font names. | |
| 7 | Ctrl+Shift+O opens Profiles. Double-click a profile: a new tab opens with it. Split Right and Split Down work too. | |
| 8 | Session tab: set Command to `wsl.exe` (Custom Command = Command). New tabs with that profile open WSL. | |
| 9 | Close Termurion and start it again. Your edits are still there (`%APPDATA%\Tandem\profiles.toml`). | |

## Deferred review items

1. Importing a profile with the same Guid as an existing one replaces it without asking.
2. Color edits always write alpha 1.0, so colors with transparency (Cursor Guide) lose it.
3. `schema::find` is a linear scan; index it when a hot path needs it.
4. Font size: iTerm2 points are drawn at 4/3 px each (the Windows convention; the milestone 1 look was approved). A Mac `13` draws a little larger than on the Mac.
5. Custom Command `SSH` and `Browser`, and Custom Directory `Recycle` and `Advanced`, start the default shell or home without a note.
6. The import file dialog blocks drawing while it is open.
7. egui's requested repaint delay is not honored, so a tooltip may wait for the next mouse move.
