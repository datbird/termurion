# Milestone 13n acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line is a first look. The pure
parts (weight and bold choice, line height, ligature levels, anti-alias and thin-stroke rules, the box and
Powerline shapes, the Special Font Config table and its editor's field checks, the text size rules) are unit
tested, and two checks were measured headless on EXTREME (Cascadia Code glyph ids with and without ligatures; an
offscreen "W" with 51 alpha levels before the anti-alias fix and 2 after). The app wiring (per-pane faces,
pointer cells, tmux sizing, broadcast, Automatic Profile Switching) is covered by pure-function and source-order
tests only, so read those rows twice. Fonts needed: Consolas, Cascadia Mono, Cascadia Code, a Nerd Font (for
example MesloLGS NF), a CJK font and a font with no bold face. Text settings are on the profile Text tab; the
Advanced keys are under Settings > Advanced > Drawing, Windows and Session. Run the drawing rows at 100%, 125%
and 200% display scale. Mark each line pass or fail.

## Per-pane fonts and line height

| # | Check | Result |
|---|---|---|
| 1 | Two panes in one tab with different profiles (different Normal Font, size and Vertical Spacing): each draws in its own font, fills its pane at its own number of columns and rows, and the cursor and selection fill its own cell. | |
| 2 | Click, double-click and drag-select in each of those two panes: the cell under the pointer is the right one in both. | |
| 3 | Change one profile's Normal Font size in Settings while both panes are open: only that pane's text, grid and cursor change; the neighbor does not move. | |
| 4 | Window chrome (tab bar, status bar, toolbelt, banners) stays in the default profile's font while a pane uses another. A badge in a pane with another font keeps its size. | |
| 5 | A composer, an info bar and a rendered porthole in a pane with a large font line up with that pane's rows. Make Screenshot on that pane uses its font and cell. | |
| 6 | A tmux window: its size report follows the focused pane's font; columns and rows stay tmux's. | |
| 7 | Move the window between monitors of different scale: both panes keep their own sizes and no text is mis-sized. | |
| 8 | Consolas line height matches Windows Terminal's at the same size. Vertical Spacing 1.25 and 1.5: rows get taller, text stays centered, underline and strikethrough sit at the right height. | |
| 9 | Advanced "List of fonts to use alternate rounding algorithm...": emptying the list (every font generous) or adding "cascadia" changes row height live in all panes with no restart. | |
| 10 | A profile whose font is not installed draws with the fallback font (the "Font not installed" banner names only the default profile's fonts). | |

## Bold and italic

| # | Check | Result |
|---|---|---|
| 11 | Cascadia Mono: `printf '\e[1mbold\e[0m'` draws a real bold face. A family with no heavy bold: bold text is heavier by a one pixel double strike with no ghosting of neighbors in the row. | |
| 12 | "Draw bold text in bold font" off: bold draws regular weight (Brighten Bold Text is a separate color setting); tab titles and group chips stay bold; a composer or info bar in a pane keeps its bold. | |
| 13 | "Allow italic text" off draws italics upright; on slants (fake slant when the family has no italic). | |
| 14 | Advanced "Minimum weight difference between regular and bold font." at 9 makes Cascadia Mono bold double-struck live in all panes; 4 restores real bold. Underlined and struck double-struck bold is not gapped a pixel wider. | |
| 15 | Two panes, one profile with Use Bold Font off: each follows its own switch. | |

## Ligatures

| # | Check | Result |
|---|---|---|
| 16 | Stock profile with Cascadia Code: `echo '-> != === www'` shows plain separate characters (no joined arrows). | |
| 17 | ASCII Ligatures on, Cascadia Code: the same line joins `->`, `!=` and `===`; letters beside an arrow (`a->b`) never join it; "www" never joins while Advanced "Improves drawing performance..." is on. With that key off, letter ligatures may form in fonts that have them. | |
| 18 | ASCII Ligatures on, put the cursor on a ligature (block, then bar and underline): a block cursor on a ligature splits it per cell; a ligature is also broken by a color change (`printf '-\e[31m>\e[0m'`) and by selecting half of it. | |
| 19 | Advanced "Enable contextual alternates" off with ASCII Ligatures on in Cascadia Code: arrows stay unjoined; on: joined. | |
| 20 | Courier New or another blacklisted family: never a ligature even with the switch on. | |
| 21 | Non-ASCII Ligatures with a Nerd Font: a run of private-use symbols draws as one piece; Arabic still joins as before with the switch off. | |
| 22 | Two panes, one profile with ASCII Ligatures on: each follows its own switch. Tab titles and other chrome text show no ligatures. | |

## Anti-aliasing and thin strokes

| # | Check | Result |
|---|---|---|
| 23 | ASCII Anti Aliased off: ASCII is hard-edged (no gray pixels) while non-ASCII text stays smooth; the Non-ASCII switch off hard-edges non-ASCII text. Check on light and on dark cells. | |
| 24 | Stock profile at 100%: text smooth and unchanged. | |
| 25 | Thin Strokes "Always" lightens text; "Never" does not; "On Retina Displays" lightens only at 200%; "With Dark Backgrounds" thins light-on-dark cells and not dark-on-light ones (check a light cell in a dark pane and a dark cell in a light pane). | |
| 26 | Advanced "Force text to be anti-aliased on Retina displays." on at 200%: an aliased profile draws smooth. | |
| 27 | Bold in a family with no bold face: aliased text widens by one pixel; anti-aliased at 100% looks heavier without widening; at 200% shifted one physical pixel. | |
| 28 | Fill a window with many distinct glyphs (CJK pages) after a hard-edged line: the aliased text stays hard-edged afterward (atlas growth). | |

## Box drawing and Powerline

| # | Check | Result |
|---|---|---|
| 29 | `printf '\u250c\u2500\u2510\n\u2514\u2500\u2518\n'` draws a closed box with no gaps at 100%, 125%, 150% and 200%, and again after dragging the window to a monitor of another scale (DPI change). | |
| 30 | A box-drawing TUI (htop, `ls --tree`, tmux borders): lines meet at corners with no gaps or doubled pixels at 100%, 125% and 200%; heavy and double lines are symmetric; thickness doubles at 200%. | |
| 31 | Block elements (eighth-block progress bar, halves, quadrants, sextants) fill their fractions edge to edge with no seam between rows. The shades U+2591 to U+2593 are the font's own glyphs (not flat fills). Rounded corners, diagonals, triangles and wedges look right. | |
| 32 | Box glyphs never draw through a popup: find bar, toast, completion list, flashing tab bar, timestamp column or the info bar; a box line under any of them is gone, and the popup's own box stays on top. | |
| 33 | Reduce Flicker on, a TUI that hides the cursor while redrawing: borders do not blink away during the hold. An Auto Composer over a screen with box lines: the lines under it are gone. | |
| 34 | Powerline prompt (oh-my-posh or starship with a Nerd Font) with "Use built-in Powerline glyphs" on: seamless separators at 100%, 125%, 150% and 200% and after a DPI change; off: the font draws them. | |
| 35 | Branch, LN and padlock (U+E0A0 to U+E0A2) centered in the cell; slanted separators, half circles, flames, waves, pixel squares, lego, rings and the K shape (U+E0B8 to U+E0D7) recognizable, mirrored pairs mirror, no clipping. | |
| 36 | Advanced "Draw certain Powerline symbols double-width?" off: wide symbols draw one cell wide; on: two cells over the next cell. "Include extended symbols..." off: extended code points use the font. "Use your typeface's box-drawing characters..." on (applies at once): box and Powerline fall back to the font. | |
| 37 | Powerline glyphs never draw over a popup, toast, timestamp or the info bar; minimum contrast does not recolor separators; a right-to-left line with a wide symbol keeps the symbol at its cell. | |

## Make Text Bigger and Smaller

| # | Check | Result |
|---|---|---|
| 38 | Two panes: Win+= (Make Text Bigger) on one grows only it; the other keeps size and grid; the window grows to keep the tab's grid. Make Text Normal Size on the zoomed pane restores its profile size; on an unzoomed pane nothing happens. | |
| 39 | Ctrl+wheel zooms only the pane under focus; steps stop at 4 and 128 pt with no dead press. The text cursor of the zoomed pane snaps, the neighbor's does not move. | |
| 40 | Broadcast to all panes of the tab with Advanced "Should growing or shrinking the font ..." on: Bigger grows every receiving pane; off: only the focused one; a pane in a tab not broadcasting never changes. | |
| 41 | View > Size Changes Update Profile: toggling shows the check; with it on, Bigger also grows the saved profile (and the sized Special Font Config entries); Win+= then a new tab opens bigger. | |
| 42 | Automatic Profile Switching: zoom a pane +2, run a command that switches its profile by rule; the new profile keeps the +2; with the Advanced key off it opens at the profile's own size. | |
| 43 | A tmux window: Bigger changes every pane of that tmux window, the grids stay tmux's, the report follows the focused pane. Restore Text and Session Size acts as Make Text Normal Size. | |

## Special Font Config editor

| # | Check | Result |
|---|---|---|
| 44 | Settings > Profiles > Text > Manage Special Exceptions opens the window with its explanation; the table starts empty; Install Nerd Font Bundle is disabled with "Downloading fonts is not available in Tandem." | |
| 45 | "+" opens "Edit Special Exception": U+41 to U+50 shows the ASCII refusal; U+100 to U+1ff with Consolas previews its characters (the preview uses the UI font); OK is disabled until valid and a font is picked. | |
| 46 | Add Preset... lists presets alphabetically; picking one fills the range; pick a font, OK: the row appears sorted. An overlapping range shows "Range includes an already-assigned code point."; double-click a row edits it; "-" deletes. | |
| 47 | Export then Import a .json file round-trips; a file naming a missing font or malformed text shows iTerm2's alert text. | |
| 48 | A range U+E000 to U+F8FF mapped to a Nerd Font draws the icons in that font while the text stays in the main font. With U+E0B0 to U+E0FF mapped and Draw Powerline Glyphs off, the arrows draw in that font; with it on they stay Tandem's. | |
| 49 | A destination entry (start U+41, destination U+10000): `printf '\xf0\x90\x80\x80'` draws "A" from the chosen font; copying the selection copies the original character. Make Text Bigger on a pane with a sized entry grows that entry too. | |
