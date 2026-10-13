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

In these docs, "Shortcut Key" is the modifier that stands in for iTerm2's Cmd. It is Ctrl+Shift by default. You can change it to Win or Ctrl+Alt in Settings > Keys. Alt stands in for iTerm2's Option. A menu always shows the keys that apply to your choice.
