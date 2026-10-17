# Copy Mode

Copy Mode lets you select and copy text from the screen and its scrollback with the keyboard only. It works like a small vi: you move a cursor around the buffer, start a selection, move to the end, and copy.

## Start and stop

- Choose Edit > Copy Mode, or press Ctrl+Shift+Alt+C (see the note on shortcut keys below).
- With text selected, press Shift with an arrow key, Home, End, Page Up, or Page Down. Copy Mode starts at that end of the selection. Turn this off with Settings > Pointer > Selection > "Automatically enter copy mode on Shift+Arrow key with selection".
- To leave, press `q` or Esc (leaves and clears the selection), Ctrl+C or Ctrl+G (leaves and keeps the selection), or `y` (copies and leaves).

The Help > Copy Mode Shortcuts menu item opens this page.

Termurion shows the Copy Mode cursor with a flag marker. The view scrolls to keep the cursor visible.

## Move

Most keys can take a count first. Type a number, then the key. For example `5j` moves down five lines, and `3w` moves forward three words. The count can be up to 100000. A `0` after other digits continues the count; a `0` alone means the start of the line.

| Key | Moves |
|---|---|
| `h` or Left | Left one character |
| `l` or Right | Right one character |
| `k` or Up | Up one line |
| `j` or Down | Down one line |
| `w` | Forward to the start of the next word |
| `W` | Forward to the next run of non-blank characters |
| `b` | Back to the start of the previous word |
| `B` | Back to the previous run of non-blank characters |
| `0` | Start of the line |
| `^` | First non-blank character of the line |
| `$` | End of the text on the line |
| `g` or Home | Start of the buffer |
| `G` or End | End of the buffer |
| `H` | Top of the visible area |
| `M` | Middle of the visible area |
| `L` | Bottom of the visible area |
| Ctrl+B or Page Up | Up one screen |
| Ctrl+F or Page Down | Down one screen |
| Ctrl+U | Up half a screen |
| Ctrl+D | Down half a screen |
| `[` | Previous mark |
| `]` | Next mark |
| Ctrl+Y | Scroll the view up without moving the cursor |
| Ctrl+E | Scroll the view down without moving the cursor |

Marks include the prompt marks that [shell integration](shell-integration.md) creates. With them, `[` and `]` jump from command to command.

## Select

| Key | Does |
|---|---|
| `v`, Space, or Ctrl+Space | Start or stop a character selection |
| `V` | Start or stop a line selection |
| Ctrl+V | Start or stop a box (rectangular) selection |
| `o` | Swap the cursor with the other end of the selection |

After you start a selection, every move extends it.

## Copy

| Key | Does |
|---|---|
| `y` or Ctrl+K | Copy the selection and leave Copy Mode |

## Find

Press `/` to open the find bar. See [Search Syntax](search-syntax.md) for what you can type there.

## Drive Copy Mode from a key binding

In Settings > Keys, a key binding can use the "Copy Mode Commands" action. Its parameter is a string of vim-style keys that Termurion runs as if you had typed them, for example `3j<C-v>$y`. Special keys are written in angle brackets: `<C-v>` is Ctrl+V, `<Space>` a space, `<CR>` Enter, and `<lt>` the `<` character.

## Shortcut keys on Windows

In these docs, "Shortcut Key" is the modifier pair that stands in for iTerm2's Cmd. It is Ctrl+Shift by default. You can change it to Ctrl+Alt in Settings > Keys. Alt stands in for iTerm2's Option. A menu always shows the keys that apply to your choice.

No Termurion shortcut uses the Win key. Windows keeps it. A setting saved as Win becomes Ctrl+Shift.

With Ctrl+Shift as the Shortcut Key, Cmd is Ctrl+Shift, Cmd+Shift is Ctrl+Shift+Alt, Cmd+Option is Shift+Alt, and Cmd+Ctrl is Ctrl+Alt (for example View > Toggle Full Screen is Ctrl+Alt+F). With Ctrl+Alt as the Shortcut Key, Cmd is Ctrl+Alt, Cmd+Shift is Ctrl+Shift+Alt, Cmd+Option is Ctrl+Shift, and Cmd+Ctrl is Shift+Alt.

A chord that asks for three of iTerm2's modifiers at once (Cmd+Option+Shift, Cmd+Ctrl+Shift) has no pair left, so these commands use a key of their own, the same in both modes. Where Windows Terminal has a shortcut, Termurion uses it (Alt+Shift+- and Alt+Shift+= split a pane). The keys never use a letter or digit with Ctrl+Alt or Shift+Alt, because a profile's own shortcut (Cmd+Ctrl+key) uses those.

| Command | iTerm2 | Keys |
|---|---|---|
| Shell > New Window with Current Profile | Cmd+Option+Shift+N | Ctrl+Shift+Alt+N |
| Shell > New Tab with Current Profile | Cmd+Option+Shift+T | Ctrl+Shift+F11 |
| Shell > Split Horizontally... | Cmd+Option+Shift+H | Alt+Shift+- |
| Shell > Split Vertically... | Cmd+Option+Shift+V | Alt+Shift+= |
| Shell > Print > Buffer | Cmd+Option+Shift+P | Ctrl+Shift+F10 |
| Shell > Broadcast Input > Send Input to Current Session Only | Cmd+Option+Shift+I | Alt+Shift+F1 |
| Shell > Broadcast Input > Toggle Broadcast Input to Current Session | Cmd+Ctrl+Option+Shift+I | Alt+Shift+F2 |
| Termurion > Make Termurion Default Term | Cmd+Ctrl+Shift+\ | Ctrl+Shift+Alt+F10 |
| Termurion > Make Terminal Default Term | Cmd+Ctrl+Option+Shift+\ | Ctrl+Shift+Alt+F11 |
| Shell > tmux > Detach | Cmd+Ctrl+Shift+D | Ctrl+Alt+F1 |
| Shell > tmux > New Tmux Window | Cmd+Ctrl+Shift+N | Ctrl+Alt+F2 |
| Shell > tmux > New Tmux Tab | Cmd+Ctrl+Shift+T | Ctrl+Alt+F3 |
| Shell > tmux > Pause Pane | Cmd+Ctrl+Shift+P | Ctrl+Alt+F4 |
| Shell > tmux > Dashboard | Cmd+Ctrl+Shift+Enter | Ctrl+Alt+F5 |
| Edit > Selection Respects Soft Boundaries | Cmd+Option+Shift+A | Ctrl+Shift+F5 |
| Edit > Find > Clear Find | Cmd+Option+Shift+E | Ctrl+Shift+F2 |
| Edit > Find > Find All Smart Selection Matches | Cmd+Option+Shift+U | Ctrl+Shift+F3 |
| Edit > Find > Pick Result to Open | Cmd+Option+Shift+O | Ctrl+Shift+F4 |
| Edit > Find > Filter | Cmd+Option+Shift+F | Ctrl+Shift+F1 |
| Edit > Marks and Annotations > Next Annotation | Cmd+Option+Shift+Down | Ctrl+Shift+F7 |
| Edit > Marks and Annotations > Previous Annotation | Cmd+Option+Shift+Up | Ctrl+Shift+F8 |
| Edit > Set Named Mark | Cmd+Option+Shift+M | Ctrl+Shift+F6 |
| Edit > Fold All | Cmd+Option+Shift+L | Ctrl+Shift+F9 |
| View > Auto Command Completion | Cmd+Option+Shift+Y | Alt+Shift+F3 |
| View > Auto Composer | Cmd+Option+Shift+. | Alt+Shift+F4 |
| Session > Stop Coprocess | Cmd+Option+Shift+R | Alt+Shift+F5 |
| Session > Bury Session | Cmd+Option+Shift+B | Alt+Shift+F6 |
| Session > Open AI Chat... | Cmd+Ctrl+Shift+Y | Alt+Shift+F7 |
| Window > Cockpit | Cmd+Option+Shift+C | Alt+Shift+F8 |
| Window > Arrangements > Save Current Window as Arrangement | Cmd+Option+Shift+S | Alt+Shift+F9 |
| Window > Select Tab > Select Next Tab Group | Cmd+Ctrl+Shift+] | Ctrl+Shift+Alt+PgDn |
| Window > Select Tab > Select Previous Tab Group | Cmd+Ctrl+Shift+[ | Ctrl+Shift+Alt+PgUp |
| Window > Move Tab > Move Tab Left | Cmd+Option+Shift+[ | Alt+Shift+PgUp |
| Window > Move Tab > Move Tab Right | Cmd+Option+Shift+] | Alt+Shift+PgDn |
| Debug build only: Capture GPU Frame | Cmd+Ctrl+Option+Shift+G | Ctrl+Shift+Alt+F12 |
| Workgroup peer 1 to 9 | Cmd+Option+Shift+1 to 9 | Ctrl+Shift+Alt+F1 to F9 |
| A profile's new-window shortcut | Cmd+Ctrl+Option+key | Ctrl+Shift+Alt+key |

A profile's new-window shortcut is Ctrl+Shift+Alt+key. That press is also Cmd+Shift+key, so a key that Cmd+Shift already uses for a command keeps the command, and that profile opens in a tab with its Ctrl+Alt+key shortcut (Alt+Shift+key with Ctrl+Alt as the Shortcut Key). The keys Q, R, X and 1 to 9 are free for a window.

Settings > Keys does not record a Win key press. It shows a note instead.
