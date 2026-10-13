# Milestone 13x6 acceptance checklist (browser permissions and notifications)

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run; a camera or microphone if available, otherwise mark the
media rows not applicable). Mark each line pass or fail.

Before anything else: `m13x6-rulings.txt` ends with FLAGS FOR ROBERT, the behaviors that differ
from iTerm2. Section A lists the probe tests (none has run: Probe: PENDING). Sections B to J are
what no test could execute (a real prompt, a real camera, the real balloon).

Test pages (serve a folder with `python -m http.server 8000`; the origin is
`http://127.0.0.1:8000`): `note.html`

```
<title>Termurion 13x6 note</title>
<button onclick="Notification.requestPermission().then(p=>document.title=p)">ask</button>
<button onclick="new Notification('T',{body:'B'})">show</button>
<button onclick="new Notification('T')">show no body</button>
```

`geo.html` (`navigator.geolocation.getCurrentPosition(p=>document.title='ok '+p.coords.latitude, e=>document.title='err '+e.code)`
from a button), `cam.html` (`navigator.mediaDevices.getUserMedia({video:true,audio:true})` from a
button, showing the stream in a `<video autoplay>`), `audio.html` (a looping `<audio src="tone.wav" autoplay loop>`
or a button calling `play()`), and `paste.html` (a button calling `navigator.clipboard.readText()`).

## A. The desktop probe (the controller runs it with Robert's yes)

The 15 new tests are all in `webhost_win::probe::` (none has run; the default run is 48 probe
tests, 33 inherited and 15 new, plus `webhost_win::tests::menu_gets_our_items`):

- [ ] `permissions_page_buttons_need_a_trusted_click`
- [ ] `permission_request_reaches_the_app`
- [ ] `saved_grant_is_live_before_the_promise_resolves` (the premise of ruling A: a mirrored Allow is live before the promise resolves; the one most likely to fail)
- [ ] `profile_writes_answer_once_with_their_sequence`
- [ ] `a_profile_read_says_it_is_complete`
- [ ] `a_profile_grant_is_honored_with_no_request`
- [ ] `a_reload_after_the_confirmed_reset_sees_the_default`
- [ ] `revocation_reload_sweep_reads_the_frames_origins`
- [ ] `unasked_kinds_are_denied_by_the_host`
- [ ] `permission_and_notification_floods_are_bounded`
- [ ] `a_frame_request_names_the_frame`
- [ ] `audio_state_reaches_the_app`
- [ ] `same_document_navigation_starts_nothing`
- [ ] `a_page_cannot_unmute_itself`
- [ ] `service_worker_notifications_are_refused` (ruling R13x6-SW: a granted origin's service worker never installs, so no persistent notification reaches WebView2's own toast)

Full run (from an ssh session on EXTREME, with Robert's yes and no implementer building in the
checkout; a PowerShell console window shows for the whole run; `probe: process session N` must
show N not 0, session 0 prints SKIPPED, which is not a pass):

```
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout>
```

Only the 13x6 tests: `-Filters` is a `string[]`, and `-File` passes a comma string as ONE
filter, so use a wrapper `C:\Users\datbi\src\run-13x6-probe.ps1` and run it the same way:

```
$f = @(
 'webhost_win::probe::permissions_page_buttons_need_a_trusted_click',
 'webhost_win::probe::permission_request_reaches_the_app',
 'webhost_win::probe::saved_grant_is_live_before_the_promise_resolves',
 'webhost_win::probe::profile_writes_answer_once_with_their_sequence',
 'webhost_win::probe::a_profile_read_says_it_is_complete',
 'webhost_win::probe::a_profile_grant_is_honored_with_no_request',
 'webhost_win::probe::a_reload_after_the_confirmed_reset_sees_the_default',
 'webhost_win::probe::revocation_reload_sweep_reads_the_frames_origins',
 'webhost_win::probe::unasked_kinds_are_denied_by_the_host',
 'webhost_win::probe::permission_and_notification_floods_are_bounded',
 'webhost_win::probe::a_frame_request_names_the_frame',
 'webhost_win::probe::audio_state_reaches_the_app',
 'webhost_win::probe::same_document_navigation_starts_nothing',
 'webhost_win::probe::a_page_cannot_unmute_itself',
 'webhost_win::probe::service_worker_notifications_are_refused'
)
& C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout> -Filters $f
```

## B. The permission prompt

- [ ] Open a Browser profile tab on `note.html` and press "ask": an alert "Allow Notifications" appears with "The website http://127.0.0.1:8000 wants to send you notifications." and buttons Allow and Block.
- [ ] Enter, Space and Escape do nothing; no button is the default or focused; for the first second after the alert opens (and after you click another window and back) a click does nothing; after that only a mouse click or an Alt key letter answers.
- [ ] Close the alert with its window close button: the request is blocked, nothing is saved, and pressing "ask" again within 60 seconds is denied with no alert; after 60 seconds it asks again.
- [ ] With the question open, switch to another tab: the request stays pending; switch back and it is asked (a hidden pane never pops an alert).
- [ ] With the question open, navigate the pane to another page: the alert closes by itself and a late click saves nothing.
- [ ] Close the pane with a question open: the alert closes and nothing is saved.
- [ ] A page asking for several things at once (camera and microphone together) shows one alert "Allow Camera and Microphone".

## C. Allow, deny, revoke, reset take effect at once

- [ ] Notifications: Allow, reload the page: no second question; "show" produces a Termurion notification with sound; clicking it brings the pane forward and does nothing else.
- [ ] Notifications: Block, reload: "ask" gives "denied" with no alert; "show" shows nothing.
- [ ] Location: with Windows Settings > Privacy > Location off, a request is refused with no alert; on, an alert "Allow Location" appears; Allow gives a position; the next request on a reload gives it with no question.
- [ ] Camera and microphone: Allow, the video shows; the camera light is on; the toolbar menu Reset (camera and microphone) or `iterm2-about:permissions` Revoke reloads the page and the camera light goes OFF at once.
- [ ] Deny a camera request: the page gets a denial; the row shows Blocked; a Denied row reset reloads nothing.
- [ ] Reset a Notifications row from the toolbar menu: the next "show" asks again (no reload happens for notifications).
- [ ] Reset a granted Location row: the page reloads and the next request asks again.

## D. Start-up sync

- [ ] Grant notifications, quit Termurion, start it: the pane's first page is blank until the permission store has synced (a moment), then loads; a granted site needs no question.
- [ ] While unsynced (right after start), press Alt+Left, F5 and the mouse back button: none of them loads a page that has a revoked grant; the page waits blank and loads after the sync.
- [ ] Revoke a site, then Alt+Left to it from another page: it does not get its old grant.

## E. Notifications from pages

- [ ] The alert and the notification name the real end of a long host: a host like `very-long-name.example.com.evil.test` shows its END and port behind an ellipsis, never only its start.
- [ ] "show no body" shows the origin as the text; a long title or body is cut with an ellipsis.
- [ ] A page cannot close or replace a Termurion alert (a finished command's notification) with its own close; a Termurion alert shown after the page's notification stays up when the page closes its own.
- [ ] A page navigating away drops its notification only when it is still the one on screen.

## F. Audio

- [ ] Load `audio.html`: it starts muted and the bar "Audio was muted. Allow playback by ...?" appears.
- [ ] Allow Once: sound plays until the next page load; the bar returns on the next load.
- [ ] Allow Always: sound plays and every later load of the site plays with no bar; the toolbar menu lists "Reset Audio Playback Permission (Allowed)".
- [ ] Deny Always: stays silent with no bar; the toolbar menu offers "Unmute Current Page", which unmutes only this page.
- [ ] A page script cannot unmute itself.

## G. Toolbar site permissions

- [ ] The toolbar menu shows the page's origin line (punycode for a non-ASCII host, never Unicode), then "Reset <Type> Permission (<Decision>)" items sorted by name, then "Unmute Current Page" when muted, each block followed by a separator, before Settings; items have no icons.
- [ ] Reset removes the row (see section C for reloads).

## H. iterm2-about:permissions

- [ ] Open `iterm2-about:permissions`: the saved rows list, filter by type, status and search; paging works.
- [ ] Revoke asks the page's confirm and reloads the site's open pages; it says revoked only when the row was removed.
- [ ] Revoke All and Clear All remove rows after their confirm.
- [ ] A web page that links to it (the link opens a new tab) can show it; clicking works only with a real click, never with a script's `click()`.

## I. Indicators

- [ ] Zoom a browser pane (Maximize): the Maximized indicator shows in the toolbar; a click shows its help text.
- [ ] Termurion draws no camera or microphone capture indicator (iTerm2 has none); the camera light shows capture.
- [ ] Turn on debug logging: its indicator shows in every browser pane's toolbar.
- [ ] Too many indicators are clipped at the toolbar's end, not scrolled.

## J. By design (these must NOT work)

- [ ] `paste.html`: the in-page Paste button (clipboard read) fails; Ctrl+V into the page still pastes.
- [ ] A page cannot save several downloads without a click each (repeat automatic downloads blocked).
- [ ] A `file:` page and a `data:` page get no notification or location question; they are denied.
- [ ] A Dev Null pane keeps no permission after closing, and never writes the Browser profile or table.
