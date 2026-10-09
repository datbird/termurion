# Milestone 13x3 acceptance checklist (browser find and copy mode)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run). Mark each line pass or fail.

Before anything else: `m13x3-rulings.txt` line "Probe:" records the 13x3 desktop probe
(`PENDING`, or the last run's `PASS`/`FAIL`). Section A lists the probe tests; the rows in B to G are
what no test could execute (a real page, the real clipboard, a real key, a window of the desktop).
Rows marked (world) are inert only if the view's isolated world is unavailable, which is a fail
when section A passed.

Test pages (serve a folder with `python -m http.server 8000`): `long.html`, a long article with the
word "the" many times, words "The" and "there", and links; `frames.html`:

```
<title>Tandem 13x3 frames</title>
<p>outer the text</p>
<iframe srcdoc="<p>inner the text</p>" width="300" height="80"></iframe>
<iframe src="https://example.org/" width="300" height="80"></iframe>
<p><a href="https://example.com/a">example link</a> <a href="https://example.com/b">second link</a></p>
```

## A. The desktop probe (the controller runs it with Robert's yes)

Command: `scripts/probe-desktop.ps1` (default filters `webhost_win::probe::` and
`webhost_win::tests::menu_gets_our_items`: 22 probe tests and the menu test, 23). The eleven new
tests are:

- [ ] `graph_reaches_a_srcdoc_iframe`
- [ ] `find_result_is_capped_in_the_bridge`
- [ ] `find_result_is_capped_for_wide_text`
- [ ] `find_highlights_across_frames`
- [ ] `find_urls_show_bubbles`
- [ ] `find_messages_forged_by_a_page`
- [ ] `find_forged_click_cannot_reach_the_client`
- [ ] `browser_copy_mode_follows_iterm2`
- [ ] `page_cannot_hijack_copy_mode`
- [ ] `copy_mode_keys_in_iframes_and_untrusted`
- [ ] `copy_mode_one_key_is_one_line_and_escape_leaves`

Notes: the Client bridge cap's JavaScript arithmetic has no in-tree runtime test (browser_js::tests
pins its text and that a payload at CAP_BUDGET passes the gate); only `find_result_is_capped_in_the_bridge`
and `find_result_is_capped_for_wide_text` run the script itself. The second block of
`page_cannot_hijack_copy_mode` (no copy scripts) now asserts the page received the key, no cursor
exists and nothing is selected, so it can fail; the probe does not read the clipboard. Record the output beside `probe-run-2026-10-07c.txt`.

## B. Find bar on a real page

- [ ] Open a browser pane on `long.html`, press Win+F, type "the": yellow highlights, one orange, the count beside the bar. (world)
- [ ] The page moves down by one bar height while the bar is open and back when it closes; with an announcement bar showing, the two stack once, no gap and no overlap.
- [ ] Return moves forward and Shift+Return also forward. Edit > Find > Find Next and Find Previous move as `SwapFindNextPrevious` says.
- [ ] Type "The": fewer matches (smart case). Pick Case-Sensitive Regex in the mode menu and type "th[ae]": matches follow.
- [ ] Esc clears every highlight and the page gets the keyboard; clicking the page while the bar is open ends typing in the bar and the page types.
- [ ] Clicking the address field while the bar is open: the keys go only to the address field (never to both).
- [ ] Open `frames.html`, find "the": matches appear in the outer text and in the srcdoc iframe; a match inside the cross-site frame is not required (recorded in the rulings).
- [ ] Edit > Find > Clear Find clears the page; Jump to Selection scrolls the page's selection to the middle; Filter and Find Cursor do nothing on a browser pane (known, not greyed).
- [ ] The status-bar Search Tool field gives the keyboard back and forth the same way.
- [ ] Select text on the page, Edit > Find > Use Selection for Find: the find bar (open it) shows the selection; with a regex mode picked, the dots and stars in it are escaped (`BVC:778-791`).
- [ ] With a search active, Edit > Find > Pick Result to Open shows letter bubbles over the visible matches and a typed letter opens the match's link; with no search it does nothing (`BVC:1429-1435`, `1322-1323`).

## C. Find URLs

- [ ] On `frames.html`, Edit > Find > Find URLs shows letter bubbles on the links. Typing one of them with a real key opens a new browser tab; Esc removes the bubbles.
- [ ] The bubbles do not appear for a page that calls `dispatchEvent(new KeyboardEvent(...))`: a synthetic key on the page picks nothing (R13x3-2).
- [ ] A custom Find URLs pattern in Advanced settings is used; one with `(?i)` shows no bubbles (known).

## D. Find Globally over a browser pane

- [ ] Edit > Find > Find Globally "the" with a terminal pane and the browser pane open: the page's matches are listed with context under the browser session.
- [ ] Clicking one scrolls the page to that match and focuses its pane.
- [ ] Close the browser pane or navigate it, then look at the list: its rows are gone (or the window re-searches); no click focuses a dead pane. Close the Find Globally window and reopen it: no old rows.

## E. Copy mode on a real page

- [ ] Edit > Copy Mode on a browser pane shows the cursor. (world)
- [ ] `j`, `w`, `v`, `w`, `y`: a phrase is selected and copied; paste it in a terminal pane and in another app (the clipboard really holds it); the cursor is hidden after `y`. `q` leaves and keeps the selection.
- [ ] One press of `j` moves exactly one line (no double delivery); Esc leaves copy mode.
- [ ] `3j`, `V`, `$`, `y` copy three lines; `0` and the digits act as iTerm2's count; Ctrl+Space toggles character selection.
- [ ] A key binding with key action 73 "3jV$y" copies lines from the page; one with a parse error shows Tandem's banner.
- [ ] `/` from copy mode opens the find bar with the last query.
- [ ] Navigating or a failed navigation while copy mode is on turns it off.
- [ ] Focus inside the srcdoc iframe with copy mode on: `j` moves the cursor, the iframe's own key handlers see nothing.
- [ ] Task 1 minor 4 (carried): the cap arithmetic is verified only by section A; if `find_result_is_capped_*` fails, a very common word on a long page (700 or more matches) stops updating its count.

## F. Keys Tandem must not swallow in copy mode

- [ ] With copy mode on, Alt+F4 closes the window and F10 reaches the host as it does for a terminal pane (Task 8 minor 1). `GateState::owner` excludes them (`system_key`, final wave); if either is still swallowed, record it.
- [ ] A person's key binding on Ctrl+B/F/U/D wins over that copy-mode key on a page (the ruled order); a binding on a plain letter does not (known, WebView2 raises no accelerator event).
- [ ] Keys with no copy-mode meaning (Tab, Enter, F5) do nothing while copy mode is on (known).
- [ ] Copy mode left on after a crash of the page process or a world restart: the page gets its keyboard back. The gate holds copy mode only while the world is open and both world transitions clear it (final wave, Task 8 minor 2); if the keyboard stays dead, that is a regression.

## G. A page cannot start or steer copy mode

- [ ] A page that runs `window.iTerm2CopyMode = {...}` or sends synthetic `keydown` events (use the console of a test page) cannot turn copy mode on or move the cursor; `typeof window.iTerm2CopyMode` in the page is the page's own value.
- [ ] A page cannot read the copied text or the secret through the `y` key; the clipboard is written only after a real `y`.
- [ ] A hostile page posting `iTermGlobalClick` or `iTermIframeClick` messages from a frame cannot run code in the isolated world (no `pwned` evaluated, page title unchanged).
