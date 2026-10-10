# Milestone 13x5 acceptance checklist (browser named marks and browser triggers)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run). Mark each line pass or fail.

Before anything else: `m13x5-rulings.txt` first lines record the 13x5 desktop probe (Probe: PASS,
41 of 41 on EXTREME, 2026-10-10). 13x2's probe passed (PASS, 12 of 12 on EXTREME, 2026-10-07), so the
isolated world exists; a (world) line that finds a feature inert is a fail, not "not shipped".
Section A lists the probe tests; sections B to F are what no test could execute (a real page, the
real windows, the real menus).

Test pages (serve a folder with `python -m http.server 8000`): `article.html`, a long article with a
headline and 20 paragraphs (one of them has "Windows" and "RFC 2616" in its text); `form.html`, a
form that POSTs to `/post` and shows the result; `hide.html`, a page whose script, once an
annotation exists, sets `opacity:0` on every `body > *`, covers the right edge with a fixed
transparent `div` of `z-index:2147483647`, and listens for `click` and `keydown` on `document`.
Use a normal Browser profile, and a second one with Dev Null Mode on.

## A. The desktop probe (the controller runs it with Robert's yes)

Command (from a .ps1 or -Command: `-File` cannot pass an array, and a comma string is one filter): `& scripts/probe-desktop.ps1 -Filters @('webhost_win::probe::', 'webhost_win::tests::menu_gets_our_items', 'webhost_win::tests::document_method_is_reported')`
(the default filters run the first two; the third is 13x5's and is not in the default list). 13x5's
tests are:

- [ ] `probe::marks_scripts_run_in_a_page` (the show, update-position, navigate and reveal scripts run in the isolated world)
- [ ] `probe::annotations_post_renames` (layout monitor posts; a page's synthetic click on the annotation buttons posts nothing)
- [ ] `probe::triggers_fire_in_a_page`
- [ ] `probe::triggers_skip_local_pages` (nothing fires on `iterm2-about:`)
- [ ] `probe::many_matches_are_capped`
- [ ] `probe::page_never_sees_the_marks_or_trigger_secrets`
- [ ] `tests::document_method_is_reported` (a form POST reports POST, a link click GET; and a POST result holding an iframe that loads the page's own URL still reports POST, R13x5-7)

R13x5-6's claim is checked by `marks_scripts_run_in_a_page`: innerText, `window.find`, a select-all and a
body range do not read an annotation's name (R13x5-43).

## B. Adding marks (world)

- [ ] Open `article.html`. Right-click a paragraph, "Add Named Mark...", accept the time-stamped name: the paragraph flashes blue and an annotation appears at the right of the page.
- [ ] Edit > Set Named Mark marks the page's centre (the same prompt, then the same annotation).
- [ ] Named Marks tool > Add (browser session focused) adds a mark on the page.
- [ ] Submit `form.html`. On the result page Edit > Set Named Mark does nothing (no prompt). The context menu's "Add Named Mark..." opens the prompt but adds nothing after OK (iTerm2's per-path gate, R13x5-5).
- [ ] A Tandem page (`iterm2-about:history`) takes no mark by any path (menu, context menu, tool).
- [ ] Open a terminal and a browser Set Named Mark prompt: only one is open at a time (the second request focuses the first).
- [ ] A mark with a name over 256 characters is cut; a mark on a page with `&` in the marked text reveals the right place and writes no pair of its own.
- [ ] A Dev Null profile: marks added there are gone after closing the profile; marks in the normal profile are unaffected and are not listed in the Dev Null session.

## C. Reveal, rename, remove

- [ ] Scroll away. Pick the mark from Edit > Named Marks: the page scrolls back smoothly and outlines the paragraph.
- [ ] Open another page, pick the mark from the Named Marks tool, then again from Open Quickly ("mark name"): the article loads and scrolls to it. Clicking a link while it loads cancels the reveal instead of scrolling the wrong page.
- [ ] Browser marks are listed newest first in Edit > Named Marks and in the tool, per browser session in Open Quickly.
- [ ] Rename from the Named Marks tool (Edit) or Edit > Named Marks: a Tandem name window opens (not the page); Edit > Named Marks, the tool, Open Quickly and the annotation follow.
- [ ] Click Rename on the annotation: a Tandem name window opens outside the page showing the stored name; the page never shows an editable box. Keys and clicks in the first half second of any such window are ignored.
- [ ] Click Delete on the annotation: a Tandem confirm window opens, Cancel is the default (Enter cancels). Delete removes the mark from all three lists and the annotation leaves the page.
- [ ] Remove from the Named Marks tool deletes with no confirm (as iTerm2's).
- [ ] Navigating away while a rename or delete window opened from the page's annotation is open closes it with no change (a window the person opened from the Named Marks tool or the menu stays open, R13x5-9).
- [ ] Removing the last mark of a page clears the annotation box.

## D. Hostile pages (world)

- [ ] On `hide.html` with an annotation: the page hides or covers the box. It cannot rename or delete it; a click under the page's overlay cannot reach Delete. This is a residual (R13x5-6): the person may see no annotation, and the worst result of luring a press is Tandem's own window opening.
- [ ] A page that moves or deletes the box: the mark still exists in Edit > Named Marks and can be renamed and removed from Tandem's windows.
- [ ] A page script calling `.click()` on a visible annotation button, or typing into it, changes nothing.
- [ ] A rename or delete request for a mark of another page, or from before a navigation, opens no window.

## E. Browser triggers (world)

In the Browser profile's Triggers editor (a terminal profile's editor offers terminal actions only).

- [ ] Highlight Text (Page Content, URL regex ".", content regex "Windows") with a yellow background: the word is highlighted on every page; a P3 color is drawn (converted to CSS); an equal text and background color hides the match (accepted, as the terminal trigger).
- [ ] Make Hyperlink on `RFC ([0-9]+)` to `https://www.rfc-editor.org/rfc/rfc\1`: the text becomes a link that opens that URL. A template that yields `javascript:...`, `data:...` or `file:...` makes no link.
- [ ] Inject Javascript (URL Regex) `document.title='x'` changes the tab title. The confirm shows the whole script (scroll to the end before Turn On works) and names that it runs in the page.
- [ ] Inject Javascript (Content Regex) with `captures` in the script (`document.title = captures[0]`) sees the match; a page whose text holds quotes and `</script>` runs the same script unchanged.
- [ ] Reload After Delay "5" reloads after five seconds, at most once per 10 seconds; a delay of `abc` or `1e3` reloads nothing.
- [ ] Enter Reader Mode shows the page in Reader Mode.
- [ ] Enter Workgroup on a page match: a question names the workgroup and asks (always); Cancel is not asked again for the same page and workgroup; a second match while the question is up opens no second question. Exit Workgroup ends it without a question.
- [ ] A terminal profile with a browser action stored does nothing in a terminal.
- [ ] Triggers do nothing on `iterm2-about:` pages, including after a profile edit (the list is not resent there).
- [ ] A page with many matches (a page with thousands of repeats of the matched word) still fires: at most 64 matches per check and 256 effects per pane per 10 seconds.
- [ ] A slow regex on a huge page (a 1 MiB `<pre>` with a quadratic pattern such as `foo.*bar` against a line of "foo") stalls only that page's renderer; Tandem's window and other panes stay responsive. A pattern that nests an unbounded quantifier in a repeated group is refused with one message and not given to the page.
- [ ] A hash-only navigation drops later matches until the next page load (fails closed, noted in the rulings).

## F. Import, export and the editor

- [ ] Export a Browser profile with all eight browser triggers and import it again: the eight arrive; Highlight Text and Make Hyperlink arrive on; Inject Javascript (both), Reload After Delay, Enter Reader Mode and the workgroup actions arrive off, and the import summary names them by title.
- [ ] Turning one of those on goes through the editor's confirm; Enable All Triggers leaves them as stored and says so.
- [ ] An imported trigger with an over-long parameter or a match type outside 0 to 2 makes the file malformed; nothing is imported.
- [ ] Picking "+" in a Browser profile adds a default browser trigger that fires nothing until its action is picked again (iTerm2's matchType 0).
- [ ] A bad regex shows no error in the editor (checked in the page; the page log has the line).
- [ ] Changing a trigger in the editor reaches open pages (the list is resent when a browser action's table changes) and does not reload unrelated pages.
