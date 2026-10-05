# Milestone 13j acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line here is a first
look. The pure parts (DECSET 1007, the alternate-screen decision, the inferrer, the accumulators,
the report steps and gates, the overlay list, the first-mouse rules, the stuck-report pattern, the
focus follows mouse and pinch rules, the pointer shape choice and the drawn I-beam images) are unit
tested. The mouse paths, Win32 activation, the cursor itself and the touchpad were not, so read
those rows twice. Setup: the profile switches are Settings > Profiles > Terminal (reporting and
alternate scroll) and Settings > Profiles > Keys; the rest are Settings > Advanced ("Mouse"),
Settings > Pointer (focus follows mouse) and Settings > Appearance. Have two Tandem windows open for
the rows that cross windows, `vim` and `less` installed in WSL or MSYS, and a laptop with a precision
touchpad for the touchpad rows. Mark each line pass or fail.

## The alternate screen wheel

| # | Check | Result |
|---|---|---|
| 1 | Run `less` on a long file. The wheel does NOT move it (the default changed: the alternate screen no longer gets arrows unless asked). Leave `less`, scroll back: the history scrolls. | |
| 2 | `printf '\e[?1007h'` then `less`: one arrow per notch (three notches move three lines). `printf '\e[?1007l'` turns it off again. | |
| 3 | A new Tandem tab starts with 1007 off: `printf '\e[?1007$p'` answers `\e[?1007;2$y` (reset). | |
| 4 | Profile "Automatically enable alternate mouse scroll" on: `less` gets arrows from the wheel without 1007. | |
| 5 | With that on, "Restrict alternate mouse scroll to vertical" on: a horizontal swipe in `less` does nothing; off: a horizontal swipe sends Left and Right arrows. | |
| 6 | "Applications may use alternate mouse scroll" off: `printf '\e[?1007h'` is refused (the DECRQM answer stays reset) and the wheel in `less` does nothing. A tmux pane follows the same switch. | |
| 7 | Advanced AlternateMouseScroll on: arrows from the wheel everywhere in the alternate screen, even with the profile off, and the vertical restriction does not apply. | |
| 8 | Alt+wheel in `less` never sends arrows (with any of the switches). | |
| 9 | Kept quirk: set Advanced AlternateMouseScrollStringForUp to `x` with the feature off; in `less` a horizontal wheel is swallowed and a vertical one does nothing. The strings themselves are never typed. | |
| 10 | Kept quirk: the alternate mouse scroll question (an info bar) almost never appears, because iTerm2 only feeds it horizontal wheel events. Swipe sideways in `less` a few times: if it appears, Yes turns on Advanced AlternateMouseScroll, Don't Ask Again is remembered (Advanced > Stored), and it asks at most once per pane. | |
| 11 | Broadcast Input on with two panes both in `less` with 1007 on: one wheel notch over one pane moves both (arrows broadcast, ruled). Over a mouse-reporting `vim` in a broadcast set, only the pane under the pointer gets the wheel report. | |
| 12 | Arrows carry modifiers: Shift or Ctrl held during the wheel in `less` with 1007 on sends the modified arrow (no change in `less`, but `cat -v` after `printf '\e[?1049h\e[?1007h'` shows the sequences). | |
| 13 | A tmux pane in the alternate screen follows its own mode: the wheel still scrolls tmux's copy mode as before. | |

## Mouse reporting

| # | Check | Result |
|---|---|---|
| 14 | A new profile has Mouse Reporting on (Report Mouse Clicks and Drags). `vim` with `:set mouse=a` gets clicks and moves its cursor. An old profile that stored off stays off. | |
| 15 | `vim` with `set mouse=a`: one wheel notch scrolls by vim's own step once, not three times. | |
| 16 | Advanced ProportionalScrollWheelReporting off and DoubleReportScrollWheel on: each notch gives two reports (vim moves twice as far). Proportional on, fast spin of the wheel: several steps per event, never more than 32. | |
| 17 | Report Mouse Wheel Events off: `vim` gets no wheel; the wheel scrolls history or sends arrows as it would without mouse mode. | |
| 18 | Report Mouse Clicks and Drags off: clicks select text even in `vim`; with mode 1003 on, hover motion is still reported (kept, as iTerm2). | |
| 19 | Restrict mouse reporting to alternate screen on: at a shell prompt `printf '\e[?1000h'` then click: nothing is reported (a click selects). In `vim` it reports. | |
| 20 | Advanced ReportHorizontalScrollEvents off: a horizontal swipe in `vim` reports nothing. On: Left and Right wheel reports. | |
| 21 | Horizontal scrolling sensitivity (Advanced, read when a pane is created): the default 0.1 means a horizontal wheel step needs ten times the movement; open a NEW pane after changing it to 1.0 to feel the difference (an existing pane keeps its value). | |
| 22 | Advanced ScrollWheelAcceleration 2: a wheel notch moves farther; 1 is plain; 0 does nothing odd (kept: iTerm2's pow(0, 0) * -1). | |
| 23 | Advanced UseModernScrollWheelAccumulator off and SensitiveScrollWheel on: the smallest wheel movement scrolls a line. With the modern accumulator on, SensitiveScrollWheel changes nothing (kept, as iTerm2). | |

## First mouse

| # | Check | Result |
|---|---|---|
| 24 | Two Tandem windows, `vim` with mouse on in the back one. Click inside the back window: it comes forward, the click selects (text under it highlights) but `vim` sees nothing (the cursor does not move). A drag from that click and its release are also not reported; the NEXT click is reported. | |
| 25 | Advanced ReportOnFirstMouse on: the same click is reported to `vim` too. | |
| 26 | Advanced AlwaysAcceptFirstMouse off: the click only activates the window (no selection, no report). A Win-click on a link still acts. | |
| 27 | The activating click never selects, copies or moves a cursor: a click-drag that activates the window selects nothing and copies nothing (check the clipboard); an Alt-click that activates it does not move the prompt cursor; a double click that activates does not copy a smart selection. | |
| 28 | Clicking the title bar, a divider, the status bar or the tab bar of a background window is NOT first mouse: a divider drag still drags, and the next click in the terminal is reported normally. | |
| 29 | Hover (mode 1003) in the activating window is not reported until the next press (kept). | |
| 30 | Kept, ruled: a right click in a background window or on an unfocused pane only focuses it (the context menu waits for the next right click). A pen or touch activation may hold one report afterward (flag is not expired; deferred). | |

## Stuck reporting

| # | Check | Result |
|---|---|---|
| 31 | `printf '\e[?1000h'` at a shell prompt, then click: the shell echoes `^[[M...` and the bar says "Looks like mouse reporting was left on..." with "_Yes", "Always" and "Never". "_Yes" stops the echo (reporting is off for that pane only; another pane in mouse mode keeps reporting). | |
| 32 | "Always" turns it off at once the next time; "Never" never offers again. Advanced AutodetectMouseReportingStuck off: no offer. Advanced NoSyncNeverAskAboutMouseReportingFrustration on: no offer. | |
| 33 | A program that only prints the text `0;12;5M` never triggers the offer (it needs a report Tandem actually sent). Short reports (one-digit SGR, X10) do not arm it (kept, as iTerm2). | |

## Focus follows mouse and pinch

| # | Check | Result |
|---|---|---|
| 34 | Focus follows mouse on: moving between two Tandem windows makes the one under the pointer key while Tandem is active (new in 13j). Moving over a tab bar, title bar, status bar or any bar or popup does not move focus. | |
| 35 | Hovering a pane while a Tandem dialog, a password prompt or an approval is waiting does not take focus. With focus follows mouse (and steal, aggressive) on, start a Microsoft Pinyin composition in a pane, then move the pointer over another pane and into another Tandem window: neither the pane nor the window takes focus and the composition stays until Enter or Esc (13k merge). | |
| 36 | StealKeyFocus on (with focus follows mouse): from another app, moving the pointer into a Tandem window brings Tandem forward, or Windows flashes the taskbar button instead (partial, expected). A hidden hotkey window never pops up under a hover. | |
| 37 | AggressiveFocusFollowsMouse on: Alt+Tab back to Tandem activates the window under the pointer; if the pointer has not moved since Tandem lost activation, nothing changes. Switching between two Tandem windows does not trigger it. | |
| 38 | Ctrl+wheel (Ctrl only) over a pane changes the font size, one notch one step; Ctrl+wheel over a title bar or status bar does not. A touchpad pinch does the same. With a mouse-reporting `vim`, Ctrl+wheel changes the font and is not reported. The font size cannot run away: keep spinning, it stops at the drawn limits. All windows change together (app-wide, ruled). | |
| 39 | PinchToChangeFontSizeDisabled on: Ctrl+wheel is reported or scrolls like any wheel. | |

## The pointer

| # | Check | Result |
|---|---|---|
| 40 | The pointer is an I-beam over text, the circled I-beam under `vim` with mouse on, an arrow over an inline image, a pointing hand over a Win-hovered link, a crosshair with Ctrl+Alt held, resize arrows over a divider and the side bar handle. It does not flicker. | |
| 41 | UseSystemCursorWhenPossible off shows Tandem's drawn I-beam (compare size and position with the system one; look at 100 and 150 percent scaling and on a second monitor at another scale, where the images keep the first window's scale). The circled I-beam is always Tandem's drawing (a guess at iTerm2's sizes: 8x16 and 16x32). | |
| 42 | Over a bar, popup or other overlay the pointer is the arrow or hand of the overlay, never the reporting circle. A program that asks for a pointer name (OSC 22) gets the normal pointer (not built; 13y2). | |

## Pointer actions

| # | Check | Result |
|---|---|---|
| 43 | Settings > Pointer: bind a click to Next Window, another to Previous Window, another to New Window With Profile (pick a profile): each runs. Move Pane still works. With clicks reporting to a program (`vim` mouse on), a bound action still runs when the click is refused by a gate (clicks off), and does not run when the program owns the click. | |

## Security: Tandem overlays never reach the program

Run `vim` (or `htop`) with the mouse on in the pane. Every row: nothing the program shows changes
(no cursor move, no wheel scroll, no `:mouse` event) and nothing is typed.

| # | Check | Result |
|---|---|---|
| 44 | Left, right and middle click on each overlay inside the pane: an info bar (the stuck offer, the alternate scroll question, a host-change offer), a porthole, a block pill, a picker, the chat indicator, the find bar, the completion list, the composer and its popup, a toast, a banner. The program sees none of it. | |
| 45 | Hover over each of those overlays with mouse mode 1003 on (`printf '\e[?1003h\e[?1006h'` then `cat`): no motion reports appear. | |
| 46 | Wheel over each overlay: no wheel report and no arrows reach the program, and the history under it does not scroll (ruled). | |
| 47 | Move Pane pick (start Move Pane, then click, hover and wheel over the dimmed pick view): nothing reaches a program. | |
| 48 | Right and middle press on a divider, the title bar, the status bar, the tab bar, the toolbelt and the scroll bar: nothing reaches the program. | |
| 49 | A press the program already saw may release over an overlay (the release is still sent so the program is not left holding a button): drag from the program onto an overlay and release. | |
| 50 | The activating click is not reported (rows 24 to 27). | |
| 51 | Alt-click with a program in mouse mode does nothing (the prompt cursor does not move); at a plain prompt with no mouse mode Alt-click moves the cursor. | |
| 52 | A stuck-reporting offer turns off only that pane's reporting (row 31). | |
| 53 | An agent's injected scroll (API or an orchestration "scroll 3") still sends N wheel reports for N notches to a reporting program, never arrows, never offers the question. | |

## Touchpad (laptop)

| # | Check | Result |
|---|---|---|
| 54 | A laptop precision touchpad does not over-scroll: a gentle two-finger swipe over a long file scrolls about as far as the fingers move (not ten times); slow drags accumulate to whole lines. | |
| 55 | A mouse wheel notch still scrolls three lines; a notch under `vim` mouse mode reports once. | |
| 56 | Natural scrolling direction: with the touchpad's setting to "Down to scroll up" (natural) and off, the vertical history scroll follows Windows. Horizontal reports to `vim` (a horizontal swipe with ReportHorizontalScrollEvents on) go the direction Windows delivers when Advanced NaturalScrollingAffectsHorizontalMouseReporting is on; with it off (default) the direction is swapped when natural scrolling is on. Mapping of ScrollDirection 0 to natural is a guess: if left and right feel reversed, tell us. | |
| 57 | FastTrackpad off slows a touchpad scroll (legacy speed); on is the default. | |
| 58 | A touchpad pinch changes the font size (row 38). | |

## Kept iTerm2 quirks and rulings you can see

| # | Check | Result |
|---|---|---|
| 59 | The wheel in `less` does nothing by default (R3, changed from Tandem's old arrows). | |
| 60 | One report and one arrow per notch (R3, was three). | |
| 61 | Horizontal sensitivity 0.1 (R3). | |
| 62 | A new profile reports the mouse (R3 addition). | |
| 63 | Font changes from Ctrl+wheel are app-wide, not per session (ruled). | |
| 64 | A mouse-reporting program does not get Ctrl+wheel by default (ruled). | |
| 65 | Plain focus follows mouse now makes another Tandem window key (ruled; can take focus from a window you are typing in, as iTerm2). | |
| 66 | A tilt-wheel mouse's horizontal reports are not reversed by the touchpad setting (deferred; iTerm2 would). | |
| 67 | Stuck-report matching needs an unbroken echo (ruled); output with cursor moves between the characters is missed. | |
| 68 | Open URL in background and QuickLook are partial (final, R7). | |
