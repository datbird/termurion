# Milestone 13k acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line is a first look. The pure
parts (geometry, blink timing, fade curves, smart color math, mute, shadow, outline, slide and smear rules,
IME layout and caret) are unit tested. The wiring into the app (the clocks, the frame requests, the IME
events and candidate window position) and all drawing were not, so read those rows twice. Settings: the
cursor rows are on the profile Text and Colors tabs; the twelve other keys are Settings > Advanced (Terminal,
Drawing). Run the Shape rows once at 100% display scale and once at 150%. The IME rows need the Microsoft
Japanese IME and a Chinese Pinyin IME added in Windows language settings. Mark each line pass or fail.

## Shape and size (run at 100% and again at 150% display scale)

| # | Check | Result |
|---|---|---|
| 1 | Default profile, Cursor Type box, Vertical Spacing 150%: the box is shorter than the line and centered in it, the letter inside it uses Cursor Text Color. At spacing 100% it fills the cell. | |
| 2 | Settings > Advanced > Terminal FullHeightCursor on: the box fills the whole line height at spacing 150%. At spacing below 100% it grows from the line's top (kept iTerm2 geometry), overflowing the line bottom; a tall cursor can be painted over by the next row (known minor, see rulings). | |
| 3 | Cursor Type vertical bar: one point wide at both scales (about 1 px at 100%, 1 to 2 px at 150%). VerticalBarCursorWidth 3 makes it three points. | |
| 4 | Cursor Type underline: two points high on the bottom of the cursor box at both scales. UnderlineCursorHeight 1 makes it thinner; UnderlineCursorOffset 3 lifts it by three points. | |
| 5 | Put the cursor on a Japanese wide character (e.g. type it, press Left): the box and the underline are two cells wide, the bar stays thin. | |
| 6 | Unfocused pane: a hollow box follows the same geometry (with Hide cursor when unfocused off). | |
| 7 | Pane transparency below 100%: the box cursor stays opaque and the cell background around it keeps the pane's see-through alpha, no opaque patch. | |

## Blink

| # | Check | Result |
|---|---|---|
| 8 | Blinking cursor on: the cursor flips about every half second (not 530 ms). Typing or moving it keeps it solid, then it blinks again after half a second. | |
| 9 | TimeBetweenBlinks 1 (Advanced > Terminal): the blink is half as fast. | |
| 10 | Run a build log that scrolls with the cursor on the last row: the cursor still blinks. | |
| 11 | Only the focused pane of the key window blinks; other panes and an inactive window show it solid or hollow. | |
| 12 | Blink off in every profile: idle CPU stays flat (no wake from the cursor clock; check Task Manager for a minute). | |
| 13 | Cursor Hidden When Unfocused (Hide cursor when unfocused) on: click another pane, then another window: no cursor, not even a hollow box, in the unfocused ones. | |
| 14 | Password prompt (key glyph, 10b): with Hide cursor when unfocused on, check whether the key glyph also hides in an unfocused pane (open question from the Task 3 review; note what you see). | |

## Programs changing shape and blink

| # | Check | Result |
|---|---|---|
| 15 | Allow Change Cursor Blink off: `printf '\e[1 q'` changes the shape (block) but starts no blink; `printf '\e[?12h'` does nothing. | |
| 16 | Allow Change Cursor Blink on: `printf '\e[1 q'` starts a blink, `printf '\e[?12l'` stops it, `printf '\e[0 q'` and `reset` (RIS) restore the profile's shape and blink. The profile in Settings is never edited. | |
| 17 | `printf '\e[5 q'` (blinking bar) and `[6 q` (steady bar): shape changes always. | |

## Smooth blink and presets

| # | Check | Result |
|---|---|---|
| 18 | Text tab: Blinking cursor on, Smooth blink on: the cursor fades in and out (breathing) instead of snapping. A block cursor's letter blends toward Cursor Text Color mid-fade. | |
| 19 | The Presets row sits above Fade In Duration and lists Breathing, Linear, Fast, Slow, Subtle. Each one changes the six rows below and shows checked; editing any timing or curve clears the check. | |
| 20 | The Presets row and the six rows are greyed unless both Blinking cursor and Smooth blink are on. | |
| 21 | Fade curves: set the in and out curves to each of linear, ease in, ease out, ease in-out and watch the fade change shape; set all four durations to 0: the cursor stays solid and no frames are drawn. | |
| 22 | Typing or moving: the cursor is solid for half a second, then the fade restarts. An unfocused hollow box stays solid, no fade. | |
| 23 | During a dwell the process is idle (CPU flat); frames come at about 60 per second only while a fade runs. | |
| 24 | Known gap carried to 13m: with Smooth blink on, blinking text (SGR 5) does not blink until 13m adds its own clock. | |

## Smart Cursor Color

| # | Check | Result |
|---|---|---|
| 25 | Colors tab: Smart Cursor Color on with a box cursor greys out Cursor Color and Cursor Text Color. With Use Separate Colors only the active mode's setting counts. | |
| 26 | `ls --color` (or a colored prompt): the box takes the color of the letter under it and the letter takes the cell background, so the cursor stays readable over every color. | |
| 27 | Cursor over a letter whose neighbors have a background close to the letter's color: the box turns a contrasting gray. Letter and background close in brightness: the letter turns black or white. | |
| 28 | SmartCursorColorBgThreshold and SmartCursorColorFgThreshold at 0 and at 1 change that behavior at the extremes. | |
| 29 | Unfocused pane: hollow box in the profile cursor color. Bar and underline cursors keep the profile color. | |
| 30 | Known gap: the smart colors ignore Dim Only Text (13i), so the cursor is not dimmed with the text. | |

## Cursor Boost, shadow, outline

| # | Check | Result |
|---|---|---|
| 31 | Colors tab, Cursor Boost 0.8: text, cell backgrounds, the selection and find matches dim toward the background; the cursor box and its letter, links and labels stay bright. With separate light and dark colors the boost follows the active mode. | |
| 32 | Selected text dims once, not twice, when no Selected Text Color is set. | |
| 33 | Cursor shadow on with an underline and then a bar: a one point line above the underline and right of the bar, white for a dark cursor color and black for a light one, half transparent. No shadow for a box. | |
| 34 | Advanced > Drawing DrawOutlineAroundCursor on: a thin background-colored frame around an underline or bar cursor, none for a box. At 150% the shadow and outline widths scale. | |
| 35 | With Smooth blink on, the shadow and the outline fade with the cursor. With Auto Composer or a covering overlay open, they are hidden with the cursor. | |
| 36 | Password prompt key glyph: no shadow, outline, slide or smear is drawn under or around it. | |

## Smooth slide and animated large movements

| # | Check | Result |
|---|---|---|
| 37 | Cursor Type bar or underline, Smooth slide on: typing glides the cursor between cells over 50 ms; a box cursor never glides; a step left glides too. | |
| 38 | A jump of more than one cell or to another line snaps. CursorSlideAnimationMaxCells 3 lets 3 cell jumps glide; CursorSlideAnimationDuration 0.5 makes the glide visibly slow. Shadow and outline glide with the cursor. | |
| 39 | Snap checks: Ctrl+wheel zoom, a font size change, resizing the window, dragging a divider, scrolling back, switching tabs, switching to and from `vim` (alternate screen), changing the display scale: the cursor appears at its place at once, never gliding or smearing from the old place. | |
| 40 | Animate large cursor movements on: in `vim`, `gg` then `G` (a 150 point or longer jump) draws a short cursor-colored smear (0.1 s, slow-fast-slow) between old and new place; short moves draw nothing. | |
| 41 | Animate movement only in interactive apps on: smears happen inside `vim`, `less`, `htop`, not at a shell prompt (try Ctrl+L, `clear`). Off: a prompt jump smears too. | |
| 42 | CursorAnimationMinDistance 20 smears short moves; CursorSmearAnimationDuration 1.0 makes it slow. With Smooth slide on a one cell step slides and does not smear; a far jump smears. The smear follows Cursor Color, never the Smart colors. | |
| 43 | A box cursor smears only in the focused pane; a hollow box never does. CPU is idle after a glide or smear ends (no steady frames). | |

## IME composition (Windows IME)

| # | Check | Result |
|---|---|---|
| 44 | Microsoft Japanese IME, romaji: type `nihongo`: the hiragana composition appears at the cursor with an underline on the default background, the terminal cursor is gone and a 2 point caret in IME Cursor Color shows. Nothing reaches the shell before commit. | |
| 45 | Space converts, the candidate window opens right under the caret; Enter commits the kanji to the shell. | |
| 46 | Chinese Pinyin IME: type `zhongwen`: the same, the candidate window sits under the caret, a number key picks. | |
| 47 | The candidate window follows the caret after the cursor moves (output, slide and smear on), inside a split pane, in a pane with a 13h title bar, in a second window, and after resizing. | |
| 48 | Compose a long string near the right edge: a wide character moves whole to the next row and the caret follows. | |
| 49 | Change IME Cursor Color in Settings: the caret color changes; with Cursor Boost above 0 it is muted by the boost. | |
| 50 | Cancel: compose, then click another pane, switch tab, press Alt+Tab away, or press Esc: the composition disappears uncommitted, the IME's own window closes, and nothing is typed. (The cancel toggles IME permission off and on; if the OS window lingers until the next key, note it.) | |
| 51 | Esc with no composition still reaches the program (try Esc in `vim`). | |
| 52 | Broadcast Input on: a committed string reaches every broadcast pane as typed text. | |
| 53 | Password prompt (key glyph): composition draws no extra cursor over the key. | |
| 54 | Settings, tool and other egui windows: the IME still types into their text fields. | |
| 55 | Open a second window and recreate one for transparency (13g Transparency change): the IME still works in it. | |
| 56 | With Find (Edit > Find) open and its field focused, compose and commit Japanese text: it goes into the Find field, not the shell. | |
| 57 | Compose, then press Esc, lose focus (Alt+Tab) or switch pane: the composition is dropped and the IME's stray commit for it (if it sends one) types nothing anywhere. Another window repainting during a composition does not cancel it. | |
| 58 | Run `printf '\e[?25l'` (hide the cursor) and compose: the composition still draws at the cursor cell with its caret and the candidate window sits there. Scroll back and compose: it draws at the cursor cell, or at the bottom left if the cursor is off view, and keys are not swallowed once the composition is gone. Committed text scrolls the view to the bottom like typing. | |

## Kept quirks and rulings a person can see

| # | Check | Result |
|---|---|---|
| 59 | A cursor that blinks starts hidden-or-visible by its own clock after a restart, so the first flip after the clock restarts can be half a period late (kept, as iTerm2). | |
| 60 | Negative Vertical Spacing with FullHeightCursor: the cursor overflows into the next row and may be painted over (listed in the rulings as a deferred minor). | |
| 61 | The Alt+click arrow write bypasses the IME hold, so a click during a composition can write one stray arrow (kept, rare). | |
| 62 | A cursor never glides after a tab switch even if its pane's cursor moved while hidden; a pane's first sighting counts as a move and is solid for half a second. | |

## Final review additions

| # | Check | Result |
|---|---|---|
| 63 | Two terminal windows, one tailing a log: in the other, a bar or underline cursor with Smooth slide on slides, and a block with Animate large movements smears, without being cut short; a smooth blink fade in one window does not repaint the other (watch idle CPU in Task Manager). | |
| 64 | Auto Composer: compose and commit Japanese text into its dropdown and into a composer status bar field: it lands there, not in the shell. If the field has let go, the text goes to the shell as a typed key would. | |
| 65 | Toolbelt: with the keyboard in a tool's search field (Profiles, Snippets, Jobs and the rest), in Notes and in Codecierge's goal and reply, compose and commit: the text lands in that field. With a tool's list focused it is dropped, as a typed letter does nothing. | |
| 66 | Inline chat panel with its input focused: composing and committing types into the input. | |
| 67 | Chinese Pinyin: compose, press Esc (nothing is typed), then type a comma or period in Chinese mode: the full-width punctuation types. Press Win+. (emoji panel) after an Esc and pick an emoji: it types. Japanese: Esc during a composition, then nothing is typed. | |
| 68 | Korean IME: type two Hangul words, then Space, digits and arrows between syllables: no key is lost. (If Space is lost, hold only the `Process` presses instead of every press.) | |
| 69 | A program that hides the cursor (a pager, htop) with Blinking Cursor and Smooth blink on: idle CPU stays at rest (no frames at 60 Hz for a cursor nobody sees). | |
| 70 | Switch Smooth blink off while a blinking cursor is mid fade: the cursor shows at once and then blinks, never starting hidden. | |
| 71 | Recreate a window for alpha (13g) while a composition is idle: the next composition's candidate window sits at the caret at once. | |
