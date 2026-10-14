# Milestone 13l acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line is a first look. The pure
parts (cadence, pacer, power state, stroke geometry and patterns, gap cutting, margin math and colors, the
flicker hold rule, the statistics text) are unit tested. The wiring into the app (the wake clocks, which window
is asked to draw, the power sampling, the adapter choice) and all drawing were not, so read those rows twice.
Settings: the drawing and GPU rows are Settings > Advanced (Drawing, General, Experimental Features) and
Settings > General > GPU Renderer; the margins are Settings > Appearance > Panes; Reduce Flicker and the
underline color are on the profile Session and Colors tabs. Open Settings > Advanced and turn Show FPS meter
on for the frame rows. Run the underline rows at 100%, 125% and 200% display scale. Mark each line pass or fail.

## Frame rate and the meter

| # | Check | Result |
|---|---|---|
| 1 | Show FPS meter on: magenta text on near black, " [Frame N: M fps] ", at the right end of the focused pane's top row. Off: nothing. | |
| 2 | Type at the prompt: about 60 fps. `type` (or `cat`) of a big file, over 10000 bytes a second: about 30 fps, scrolling smoothly. A key typed during it echoes at once. | |
| 3 | `vim` or `less` (alternate screen): 60 fps while scrolling. | |
| 4 | Heavy output (live check under load): `cat` a 50 MB file while the meter is on; it holds 30 (60 with Maximize throughput off, row 7), Task Manager shows the window is not starving the UI, and the keyboard stays responsive. Repeat with Advanced MaximumFrameRate 30 and MetalSlowFrameRate 60: the two swap. | |
| 5 | Battery saver on (unplugged laptop, Windows Settings > Battery; Windows 11's Energy saver counts): a large `cat` draws at 30 at most; turn it off and the rate returns within a second. Being on battery without the saver does not cap. | |
| 6 | Advanced LowPowerModeFrameRate 10 with the saver on: 10 fps. | |
| 7 | Settings > General > GPU Renderer, Maximize throughput off: 60 fps under heavy output (not adaptive). | |
| 8 | A still window with the meter on and MetalRedrawPeriod 0.5 (default): the frame number rises twice a second. | |
| 9 | Idle CPU zero with MetalRedrawPeriod 0 (live check): the frame number stops, Task Manager shows Termurion at 0% for a minute with a blink-off profile and no activity. At 0.5 the frame number rises twice a second and CPU is a small steady figure. | |
| 10 | Two panes: only the focused pane shows the meter. Two windows: each counts its own frames. A minimized, hidden or cloaked (hotkey) window draws no frames and wakes nothing. | |
| 11 | A cursor on row 0 at the right edge draws over the meter (kept, listed in the rulings). | |
| 12 | Output into a background tab or a minimized window: no frames; on restore the window shows current output. A background tab's title and new-output icon still update while output runs in it. | |
| 13 | A tmux or ssh pane in a background tab with output shows the new-output activity icon on its tab (intended, as iTerm2). | |
| 14 | Two windows, output in the unfocused one: it still updates; the focused one stays responsive. Cursor blink and slide or smear animation still run at their rates. | |

## Copy Performance Stats and GPU Renderer Availability

| # | Check | Result |
|---|---|---|
| 15 | After a few seconds of drawing, Copy Performance Stats (Edit or iTerm2 menu) then paste in a pane: "Precise timers Window 1: Dropped frames:" then the numbers on the next line, then blocks for Current Drawable Time and Start-to-Start Time. Before the first save the paste is empty. With no clipboard, a banner says the copy failed. | |
| 16 | Close a window and open another: its numbers start from zero. | |
| 17 | Help > GPU Renderer Availability: an OK alert, "GPU rendering is enabled for the current session.", a blank line, "Adapter: <name> (<backend>)". | |

## Power and the GPU

| # | Check | Result |
|---|---|---|
| 18 | Dual-GPU laptop: stderr lists every adapter ("adapter available:") and "renderer adapter:" shows the integrated one by default; Task Manager shows Termurion on the integrated GPU. | |
| 19 | Prefer integrated GPU off, restart Termurion (integrated GPU preference after restart): Termurion lands on the default (discrete) adapter. Toggling it shows the restart note under the row. | |
| 20 | Prefer integrated GPU is enabled on a two-GPU machine and disabled on a single-GPU machine; before the first window's GPU starts it reads disabled (kept). | |
| 21 | A see-through window (transparency) uses the same adapter as an opaque window (the log never shows two different names in "renderer adapter:"). | |
| 22 | Advanced Disable App Nap on, restart: Task Manager's Efficiency mode stays off for Termurion when minimized. The two Disable GPU rows, UseAdaptiveFrameRate, DisableAdaptiveFrameRateInInteractiveApps, UseLowPowerGPUWhenUnplugged and UseMultiPassUnderlineRenderer change nothing (stored, inert). | |

## Underline and strikethrough styles (run at 100%, 125% and 200%)

| # | Check | Result |
|---|---|---|
| 23 | `printf '\e[4mone\e[4:2m two\e[4:3m three\e[4:4m four\e[4:5m five\e[24m \e[9mstrike\e[29m \e[4;58:2::255:0:0mred\e[0m\n'`: single, double, curly, dotted, dashed, strikethrough, and a red underline. Double is two lines, curly is two alternating dashed lines. | |
| 24 | At 1x, 1.25x and 2x the five styles and the strikethrough keep their shapes with whole-pixel lines (no blur, no doubled thickness mismatch); the strike sits mid x-height; the double's two lines do not touch. Curly's two lines are 1 pixel apart at 1x and 2 pixels apart at 2x, each a whole pixel row (zoom a screenshot). | |
| 25 | `printf '\e[4;9mboth\e[0m\n'`: underline and strikethrough together. `\e[4;58;2;255;0;0mred\e[59m back\e[0m`: red, then text-colored. | |
| 26 | `gjpqy` underlined (`\e[4mgjpqy\e[0m`): the line breaks around the descenders one pixel either side. Advanced Use solid underlines on: one unbroken line. Strikethrough never breaks. A glyph with ink at one side leaves a wider gap than iTerm2's (kept). | |
| 27 | Same gaps with curly, dotted, dashed and an OSC 8 link; non-ASCII text on the line (Japanese) gaps too; a larger font and 200% scale: gaps hug the glyph, no flicker while scrolling output. | |
| 28 | Profile Use custom underline color on with green: plain underlines and strikethrough go green, SGR 58 still wins; the swatch greys out when the switch is off (light and dark variants). | |
| 29 | An OSC 8 link (`printf '\e]8;;https://example.com\e\\link\e]8;;\e\\\n'`) shows a 4-on, 3-off dash at the underline position; Advanced Underline OSC 8 hyperlinks off hides it; an SGR style on a link wins over the dash. | |
| 30 | Underlines in an unfocused or dimmed pane dim with it; faint (SGR 2) text dims its custom underline; a selection or Minimum Contrast does not recolor an SGR 58 line (kept, as iTerm2). | |
| 31 | `printf '\e[4:3;58:2::255:0:0mx\e[0m'`, then `reset`: nothing stays underlined or colored afterward. A program's SGR 24, 0 and 59 end it. | |
| 32 | Make Screenshot of underlined links with Underline OSC 8 hyperlinks off shows no dashes; struck text sits mid x-height; gaps appear in the screenshot. | |
| 33 | Hover, annotation and IME underlines do not gap at descenders and are not trimmed under a popup (kept minor). | |

## Underlines under every Termurion surface

Fill the pane with SGR 4 and 9 spaces first: `for i in $(seq 1 60); do printf '\e[4;9m%*s\e[0m\n' 160 ''; done`.

| # | Check | Result |
|---|---|---|
| 34 | A row of SGR 4/9 spaces under a toast (for example the copy toast): no stroke inside the toast, in the gap beside it, or on its blank rows. | |
| 35 | Under an error banner: no stroke inside it, and pane text no longer shows through it. | |
| 36 | Under the find bar, the announcement bar and per-line timestamps: no stroke inside them. | |
| 37 | Under prompts and popups: the completion list, the Auto Composer, the pane move pick, a tab or profile popup, the FPS meter: no stroke draws inside them. The Auto Composer covers the strokes of its rows. | |
| 38 | Under a flashing tab bar: no stroke through it. Badges are drawn over strokes without cutting them (kept, as iTerm2). | |

## Margins

| # | Check | Result |
|---|---|---|
| 39 | Default window: text sits 5 points from the sides and 2 from the top and bottom; the default 80x24 window grows accordingly; the side tab bar width is unchanged. | |
| 40 | Settings > Appearance > Panes, Side margins 20: every pane, every open window and the window size from Columns and Rows follow; a tmux window resizes to match (the rows too). | |
| 41 | `htop` (or vim) with a colored header: the margins take its color; a row with its own colored first or last cell colors that row's side margin; a default-colored row keeps the pane color. Advanced Extend background color into margins off: the pane color. | |
| 42 | Main screen in a profile with a different background than the first pane: the margins show that pane's own color. | |
| 43 | Margins with a background image: the image shows in the margins (no opaque strip in the pane color); a colored alt-screen margin still draws. | |
| 44 | Margins with transparency (pane 0.7, alt screen): the margins are as see-through as the text area, no darker band. | |
| 45 | Window edge-drag snap, screenshot width and mouse selection at the first column use the margins and line up. | |

## Reduce Flicker

| # | Check | Result |
|---|---|---|
| 46 | Profile Reduce Flicker on with `vim`: scrolling and redraws show no half-drawn frames (a program that hides the cursor while redrawing, such as a redraw loop `while :; do printf '\e[?25l\e[H...'; printf '\e[?25h'; done`). | |
| 47 | A program that hides the cursor for good: the screen still updates after 0.2 s, never frozen. Switch off: no change. Underlined text stays underlined during a hold. | |
| 48 | Window idle with the switch on, MetalRedrawPeriod 0, every window hidden: no CPU wakes at all (the power line has no timer either). Idle with the switch on: no CPU wakes from the hold. An IME composition during a hold may be missing for up to 0.2 s (kept). | |

## Known quirks (kept or deferred, each ruled on in m13l-rulings.txt)

| # | Check | Result |
|---|---|---|
| 49 | The default MetalRedrawPeriod 0.5 s means a visible idle window wakes twice a second (row 9 turns it off). | |
| 50 | The low-power cap applies to output only; a timer-driven redraw can exceed it. | |
| 51 | Settings > Appearance > Panes: TerminalMargin and TerminalVMargin draw one labeled drag value each; General > tmux, Instant Replay and AI pages show no new bare drag values. | |
| 52 | A resize, split or divider drag during a Reduce Flicker hold ends the hold at once (a live frame); a program that hides the cursor in a window on another tab or a parked window releases to a fresh frame within 0.2 s, even with MetalRedrawPeriod 0. | |
| 53 | A full screen of dotted underline (SGR 4:4 across a 200x60 window) scrolls without a visible stall (about 60k quads; time it). | |
| 54 | Copy Performance Stats numbers are Termurion's own and are not comparable with iTerm2's. | |
