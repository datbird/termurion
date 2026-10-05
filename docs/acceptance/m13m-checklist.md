# Milestone 13m acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line is a first look. The pure
parts (the contrast math, harmonized colors, mark colors, the bidi order and mirror table, the selection runs and
their drop rules, scrollbar geometry, ticks, stamp room) are unit tested; the app wiring (drawing, pointer routing,
relayout, tmux sizing) is covered by source-order tests only, so read those rows twice. Settings: contrast, blink
and the background image are on the profile Colors, Text and Window tabs; Bidi and its keys are Settings > General >
Experimental and Advanced; HideScrollbar is Appearance > Windows. Run the scrollbar rows at 100%, 125% and 200%
display scale. Mark each line pass or fail.


## Minimum Contrast, Harmonize, marks

| # | Check | Result |
|---|---|---|
| 1 | Settings > Profiles > Colors: Minimum Contrast at 0.5, then run `printf '\e[90;100mgray on gray\e[0m\n'`: the text is readable (shifted toward black or white). At 0 it is hard to read. | |
| 2 | Minimum Contrast at 1.0 (and at 0.97): the row shows "Black & White Text Only" in Settings; below 0.97 it does not. Text is plain black or white. | |
| 3 | Minimum Contrast at 0.5 and 1.0 with a selection over low-contrast text: the selected text stays readable against the selection color. With an explicit Selected Text Color set, that color is used (adjusted against the selection). | |
| 4 | Minimum Contrast at 0.5 with Find open on low-contrast text: matched text is readable against the find color. A cell both matched and selected: its text is adjusted against the selection color, not the match color. | |
| 5 | Minimum Contrast: the block cursor's text, a box-drawing or block character (`printf '\u2588\u2593'`), hidden text (SGR 8) and Find labels are not adjusted. Faint text (SGR 2) is adjusted after its fade. | |
| 6 | Harmonize 256 Colors with a light theme: `for i in $(seq 16 255); do printf "\e[48;5;${i}m  "; done; printf '\e[0m\n'` changes (cube and grays follow the theme). Off restores the standard cube. | |
| 7 | Use Theme Mark Colors on: prompt marks in the margin and on the scrollbar use ANSI blue (success), yellow, red (error). Off: the fixed colors. | |
| 8 | Auto Composer on (View > Auto Composer) draws a 1 px line mark between commands, ending 15 cells short of the right edge; alwaysUseLineStyleMarks (Advanced > Drawing) does the same without Auto Composer. Off: margin bars. Known: the line also draws through a selected command (skip owned by a later milestone). | |
| 9 | Line-style marks stop before the scrollbar strip and the Adjacent column; the FPS meter, badges, copy cursor and IME sit inside the text rectangle. | |

## Blink

| # | Check | Result |
|---|---|---|
| 10 | `printf '\e[5mblink\e[0m\n'` blinks only with Blink Allowed on (profile Text); off: steady. Turn it on live: it starts at once. | |
| 11 | SGR 5 with Cursor Smooth Blink on and a steady cursor: the text still blinks at TimeBetweenBlinks; the smooth cursor keeps fading. | |
| 12 | Blinking text with underline (`\e[4;5m`) or strikethrough (`\e[9;5m`): in the off phase the glyph and its line are both gone; neighbors keep theirs. | |
| 13 | An unfocused window's blinking text rests visible. A second blinking pane, another window: only the key window's clock runs; idle CPU is zero with no blinking text. | |
| 14 | Copy with Styles of blinking text, a trigger's Change Style with blink, and a DECRQSS SGR query (`printf '\eP$qm\e\\'` while SGR 5 is set) all carry the blink attribute. | |

## Background image

| # | Check | Result |
|---|---|---|
| 15 | Profile Window tab: the "Use background image" box opens the file picker (image filter); choosing a file sets the image; the Folder... button opens the folder picker for rotation; unchecking clears Location and Folder. The box reflects an image set by other means. | |

## Right-to-left rows

| # | Check | Result |
|---|---|---|
| 16 | `echo 'abc \u05e9\u05dc\u05d5\u05dd def'` reads right to left in the middle. With Bidi off (Settings > General > Experimental) it is logical order. | |
| 17 | An Arabic word (for example `echo 'مرحبا'`) shows joined letter forms, not isolated letters; a mixed Arabic and English line orders correctly. | |
| 18 | Brackets around Hebrew text are mirrored; brackets around English after English are not. Digits in a Hebrew sentence keep left-to-right order. | |
| 19 | DetectParagraphDirection plus RightJustifyRTLLines: a Hebrew-first line sits at the right edge; with RightJustifyRTLLines off it stays left; with detection off, no shift. | |
| 20 | The block cursor sits on the right letter in an RTL row; the cursor moves by logical order as you type and edit; the copy-mode cursor moves over the letters where they are drawn. | |
| 21 | Selecting with the mouse across a direction change (drag from the English text into the Hebrew, then back past it): the highlight is the cells swept on screen and the copied text (paste elsewhere) is the selected letters in logical order. A double click selects the word under the pointer. | |
| 22 | Copy and Copy with Styles of that visual selection paste the same cells, logical order; Print and the screenshot annotation show the same cells. | |
| 23 | Mouse reporting (`less` with mouse, vim): clicking an RTL row reports the VISUAL column (iTerm2, PTYMouseHandler.m:1404). | |
| 24 | An OSC 8 link on a Hebrew word in a mixed row (`printf '\e]8;;https://example.com\e\\\u05e9\u05dc\u05d5\u05dd\e]8;;\e\\\n'`): hover underlines the shown cells and a click opens that link, not a neighbor. Underline, strikethrough and link underlines sit under the moved cells. | |
| 25 | IME composition at the end of an RTL row: the composition and caret sit at the leftmost visual piece of the cells they cover, left of right-justified text; Hebrew being composed shows in typing order until committed (ruled). | |
| 26 | Find labels (Win-click URL labels) and note underlines sit under the shown cells. | |
| 27 | vim or less (alternate screen) with DisableBidiInAlternateScreen on: no reordering; off: reordered. | |
| 28 | Security: `printf '\xe2\x80\xaeabc\n'` (RLO), RLE, LRO, isolates and a mixed RLO and Hebrew line: English text is not reversed and nothing outside the line moves. A title set by OSC 0 containing RLO leaves the tab title, status bar and title bars in logical order. | |
| 29 | An inline image on a row with RTL text: the row is not reordered (ruled). | |

## Selection through scrolling and resize

| # | Check | Result |
|---|---|---|
| 30 | Select with a visual drag on an RTL row, then run output that scrolls the screen (`seq 1 200`): the selection moves with its line, or is gone when its line leaves the scrollback; a copy never pastes lines you did not select. | |
| 31 | In vim and less (scroll regions): scroll the pane with a selection inside the region; the selection moves only with the region's lines, nothing outside it highlights. | |
| 32 | Resize the window narrower and shorter with a selection active: the selection keeps what maps exactly, or clears; copy never pastes stale text. Clear To Mark also clears it. | |

## Scrollbar

| # | Check | Result |
|---|---|---|
| 33 | Windows Settings > Accessibility > Visual effects > Always show scrollbars ON: every pane has a 15 px track at its right edge and the grid is narrower by it; dragging the thumb scrolls; clicking the track pages; the wheel over the bar scrolls even in a mouse-reporting app; the arrow pointer shows over the bar. OFF: a thin thumb appears for about 1 s after a scroll, widens on hover, and the grid does not change. Toggle the setting while Tandem runs: panes resize without a restart (a tmux window reports the new columns). | |
| 34 | A pane with no history: a press on the bar does nothing (no drag, no page). | |
| 35 | Two split panes: each has its own bar and ticks. A completion list, find bar or toast over the right edge sits above the bar and its ticks. | |
| 36 | Prompt marks (green, red, gray), manual marks and annotations (yellow) show as ticks along the bar (Advanced > Experimental Features, ShowMarksInScrollbar). Find open: Find matches show as ticks that vanish when the bar closes. ShowLocationsInScrollbar off: no ticks; ShowMarksInScrollbar off: only Find ticks. | |
| 37 | HideScrollbar (Settings > Appearance > Windows) on: the strip and bar vanish, columns come back at once, wheel and keys still scroll. At 125% and 200% display scale the ticks, knob and padding scale. | |
| 38 | A new window with the legacy bar opens with its Columns setting (the bar's 15 points are added once). | |

## Adjacent timestamps

| # | Check | Result |
|---|---|---|
| 39 | View > Show Timestamps with the profile style Next to contents (Adjacent): a 100 pt column appears right of the text, the text reflows narrower, and run `stty size` (or look at a tmux window): the shell sees the new columns. Toggling off widens it back. | |
| 40 | The same inside tmux (tmux integration): the tmux window reports the narrower columns; with two panes only the Adjacent one narrows; the neighbor is unaffected. Over contents and Hover still draw over the text. | |
| 41 | A wide label (long date format) extends left over the text, never cut; the label colors follow the pane's colors on a light theme (readable). | |
| 42 | Windows "Always show scrollbars" on: open a window by Columns 100, and apply a Window Style from the menu; the window holds 100 columns beside the bar. | |
| 43 | With a program blinking text (SGR 5) in two windows, the key window's cursor blinking does not hide the other window's blinking text: it rests visible. | |
| 44 | Click an inactive window's scrollbar: the window activates and the bar does not page or start a drag. Any-motion mouse tracking (1003) in vim or tmux: moving over the bar sends no motion reports. | |
| 45 | In a full-screen app (alternate screen) the legacy bar draws no mark or note ticks; Find ticks still draw. After the app exits the ticks return in the right places. | |
