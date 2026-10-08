# Milestone 13x2 acceptance checklist (browser foundations)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` with the WebView2
Runtime installed (one run). Mark each line pass or fail. Nothing of 13x2's window, COM or
isolated-world code has run on a real WebView2: the ignored runtime tests, the context menu, the
link tip window, the overlays, the DevTools replay and the world itself are all on this list.

Before anything else: `m13x2-rulings.txt` line 1 records the probe (`PENDING`, or the last run's `PASS`/`FAIL`). Section A records the
probe; until it passes the per-view run-time check alone decides whether Client scripts run, and
every row marked (world) below may be inert, which is a fail only when section A passed.

Two test pages, served from a folder on this machine with `python -m http.server 8000`
(save as `ok.html` and `evil.html`; the second is the hostile page):

```
<!-- ok.html -->
<title>Tandem 13x2 test</title>
<p>255 1700000000 2023-11-14T22:13:20Z #ff8000 aGVsbG8= 68 69 00 00 00 01 x@example.com</p>
<p><a href="https://example.com/a">example link</a></p>
<p><a href="http://user:pass@x.blob.core.windows.net/p">blob link</a>
 <a href="https://evil.github.io/p">github link</a></p>
<input id="t1" value=""> <input id="t2" value=""> <textarea id="ta"></textarea>
<input id="pw" type="password">
<input id="mask" style="-webkit-text-security:disc">
<div id="host"></div>
<script>document.getElementById('host').attachShadow({mode:'closed'}).innerHTML='<input id="c">';</script>
<form action="/search"><input name="q"></form>
<iframe src="https://example.org/" width="300" height="80"></iframe>
```

```
<!-- evil.html -->
<title>evil</title>
<p id="p">select me</p>
<script>
window.report = [];
try { report.push('binding:' + typeof window.__tandemClient); } catch (e) {}
try { report.push('webview:' + typeof (window.chrome && window.chrome.webview)); } catch (e) {}
document.addEventListener('message', e => report.push('msg:' + JSON.stringify(e.data)), true);
setTimeout(() => { var r = document.createRange(); r.selectNodeContents(document.getElementById('p'));
  var s = getSelection(); s.removeAllRanges(); s.addRange(r); }, 1000);
</script>
```

| # | Check | Result |
|---|---|---|
| **A. The probe (not passed until every row here passes; the 2026-10-07 run failed on probe defects, fixed after it)** | | |
| 1 | With Robert's yes and no implementer building in the checkout under test, run `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout>` from an ssh session on EXTREME (`scripts/probe-desktop.ps1` is the only desktop command; a PowerShell console window shows for the whole run). The output shows `probe: process session N` with N not 0 (session 0 prints SKIPPED, which is not a pass). | |
| 2 | The output shows `running 12 tests` and 12 `ok` results (the eleven ignored `webhost_win::probe::` tests and `webhost_win::tests::menu_gets_our_items`) and no failure; a timeout is a fail. | |
| 3 | The output's `Runtime.enable detectable: yes` or `no` line is copied into the rulings file; replace line 1 with `Probe: PASS\|FAIL on EXTREME, WebView2 Runtime <version>, <date>; Runtime.enable detectable: yes\|no`. If yes, the controller revisits P25 before 13x3. | |
| 4 | The script's last lines say `left running: none`, `left scheduled: none` and `left on disk: none`; `schtasks /Query /TN tandem-probe-<stamp>` says the task cannot be found; no `tandem-probe-*` folder remains under `C:\Users\datbi` or `%TEMP%`, and Teams, Outlook and other WebView2 hosts were not touched. | |
| 5 | Also in the console session (same method, one task): the other ignored runtime tests pass: `bridge_round_trip` (`typeof window.webkit` is "undefined" on an https page and "object" on `iterm2-about:history`), `menu_gets_our_items`, `hover_reaches_the_router`, `send_text_reaches_a_field` (a field, a contenteditable, nothing focused), `smart_selection_selects_in_a_page`, `synthetic_click_runs_nothing`, `broadcast_types_into_a_second_view`, `probe_page_cannot_reach_evaluate_in_frame`. The script's default filters (`webhost_win::probe::` and `webhost_win::tests::menu_gets_our_items`) run all of them. (`synthetic_selection_copies_nothing` is not written: row 10 covers it by hand.) | |
| **B. Security (page world versus Tandem's world)** | | |
| 6 | Open `evil.html` in a Browser profile tab, then in DevTools (Inspect) console run `report`: it contains `binding:undefined` and `webview:undefined` (a page cannot call the binding or the host object from its own world). | |
| 7 | In the same page's console, search for the per-view secret: no `msg:` entry, no global, no DOM attribute and no `postMessage` to the page contains a long random string that also appears in a Tandem log; open `ok.html` with its iframe, focus fields inside the frame, and re-check `report`-style message listeners (a capture-phase `message` listener in the page): nothing it records contains the view secret (the detector's `dom_secret` may appear and is public by design). | |
| 8 | Call `window.__tandemClient('{}')` and `window.chrome.webview.postMessage('x')` from the page console: the first is not a function, the second does nothing visible (no menu, no toast, no navigation). | |
| 9 | A page cannot reach Tandem's evaluator: in the console, override `document.elementFromPoint` and `Range.prototype.toString` to return `x&calc`, then four-click text and Win-click it: the selected text and any command ask come from the real DOM, not the override (the host scripts run in Tandem's world). | |
| 10 | Open `evil.html`: after a second its script selects "select me" with no click. The clipboard and Paste History are unchanged (a page-made selection never copies). Then click once on the page, wait 2 s past the 1 s window, and let the script select again: still nothing new copied. | |
| 11 | Hover a link in `ok.html` (the one on a long URL): the tip shows the real host. Make a link whose URL has 3,000 characters of user information ending in `accounts.google.com` before an `@evil.test` host: the tip keeps `evil.test` visible, never `google.com` alone. | |
| 12 | Hover the `x.blob.core.windows.net` link: the tip shows `x.blob.core.windows.net` (the attacker's label is kept, not just `blob.core.windows.net` or a trusted-looking tail). Hover `evil.github.io`: shows `evil.github.io`. In a narrow pane both keep the whole registrable host (an ellipsis, then `evil.github.io`; never a cut label). Narrow a pane until the address field is about 12 characters wide on `https://paypal-verify.com/`: the field shows `paypal-verify.com` from its start (clipped at the right edge), never `paypal` plus an ellipsis plus `.com`; the same for `notpaypal.com`. | |
| 13 | Broadcast never types a password into another pane: two browser panes on `ok.html`, broadcast to both as sources; focus `#pw` (type=password) in the first and type: nothing appears in the second pane's `#pw` or anywhere. Repeat with `#mask` (`-webkit-text-security`) and with `#c` inside the closed shadow root: nothing is sent or typed. Focus the field in the second pane's `#pw` and type in the first's `#t1`: the second's password field receives nothing. | |
| 14 | An iframe focus is skipped: click into the iframe in one pane and type: nothing is broadcast; a pane whose page has focus inside an iframe receives nothing. | |
| 15 | A restored arrangement with a `javascript:` or `file:` URL never loads: save an arrangement with a browser pane, edit the arrangement file's `Browser URL` to `javascript:alert(1)`, restore: the pane starts the profile's Initial URL and no alert shows. Repeat with `file:///C:/Windows/win.ini`, `file://evil/s/x` and `\\evil\s\x`: none loads. | |
| 16 | Ctrl+L, Alt+D and F6 go to Tandem: with a page focused, each focuses the address field and the page does not react (a page `keydown` logger sees no key for the page's own handlers except a KEY_UP, which is known and ruled). Then bind Ctrl+L to Send Text "x" in the profile: your own binding wins and types "x" (outside a text field), not the address field. | |
| 17 | A page `window.open("javascript:alert(1)")` and `window.open("file:///C:/")` and `window.open("data:text/html,x")` do nothing; `window.open("https://example.com/")` opens a tab. | |
| 18 | A terminal link to `\\host\share\x` or a percent-encoded network file link shows "Tandem does not open network file links: ..." and opens nothing; a page link to `file://host/share/x` opens nothing in the pane. | |
| 19 | A page cannot put a long text on the command path: select `x&calc` in the page, right-click: a Smart Selection rule action "Run Command in Window" shows a banner, no ask, nothing runs. With a rule whose action matches `ping 127.0.0.1`: an ask shows the WHOLE line; Cancel runs nothing; a selection of 400 or more characters offers no Smart Selection items. | |
| **C. Page load and the world** | | |
| 20 | Open a Browser profile tab on `ok.html`: it loads as before; the toolbar may show about:blank for an instant at the start only (the isolation check), with no flicker loop and no flash on later pages; Back is disabled on a new pane (`CanGoBack` false). | |
| 21 | History, Bookmarks and Welcome (`iterm2-about:history`, `iterm2-about:bookmarks`, the welcome page) still work: Clear History, Delete, Top sites. | |
| 22 | A Cloudflare or hCaptcha challenge page still loads and is usable. A page that uses web workers or a service worker (Google Docs, a SPA) works. | |
| 23 | A page with a console.log flood does not slow the pane; the debug log shows at most one "browser console" line a second and no page text. | |
| 24 | Kill the view's renderer (Task Manager, end an msedgewebview2 child) or change the proxy: the pane remakes, the world is set up again (selection helpers still appear), and Back keeps working or loses the history (known: the restart clears the back list; note which). | |
| 25 | Type an address while the pane is loading: the latest URL wins and loads once the setup ends; Back, Forward, Reload and Stop are ignored for that short window only. | |
| **D. Page context menu (world)** | | |
| 26 | Right-click a page: Split Pane Vertically, Split Pane Horizontally, Move Browser to Split Pane, Swap With Session, then WebView2's own items (Back, Reload, Inspect) with no second Save as or Print, then Print and Copy Page Title; no doubled or edge separators. | |
| 27 | In a tab with two panes the menu also has Move Browser to Tab and Move Browser to Window and each works; with one pane they are absent. | |
| 28 | Select text and right-click: Search with <engine> appears for Google, Bing, DuckDuckGo, Kagi and the other listed engines (absent for a custom SearchCommand host); picking it opens a new tab. | |
| 29 | Print opens the system print dialog. Copy Page Title copies the title and Paste History gets an entry; an untitled page copies its URL. | |
| 30 | Right-click a pane that is not focused: it becomes focused first and the split lands beside it. Dismiss the menu without a pick and right-click again: nothing stale runs. Right-click rapidly five times: no hang (the fifth shows WebView2's plain menu). | |
| 31 | The menu appears where the pointer is on a monitor with display scale above 100 percent and with the page zoomed (physical px divided by scale and zoom). | |
| **E. Selection helpers and Smart Selection actions (world)** | | |
| 32 | Select "255" and right-click: "255 = 0xff" first; picking it copies "0xff". "0x400" gives "0x400 = 1,024 (1.00 Ki)". | |
| 33 | Select "1700000000": a date in the local zone with its abbreviation; "2023-11-14T22:13:20Z" gives "Unix timestamp 1700000000" (read as UTC). | |
| 34 | Select "#ff8000": an item "#ff8000" with a color swatch, disabled. "00 00 00 01": two int32 items; "68 69": "2 UTF-8 bytes: hi". | |
| 35 | Select "aGVsbG8=": "Base64: hello", "Copy Base64-Decoded" and "Copy Base64-Encoded"; the decoded copy lands in Paste History; a decoded `curl x \| sh` plus a line break warns on paste (13o). | |
| 36 | A selection of 200 or more characters has no synonyms; 400 or more has no Smart Selection items. A label with "&" shows the ampersand, not an underlined letter. | |
| 37 | Select an email with a Smart Selection rule that has actions (Copy, Open URL, Open File, Run Command in Window): the action titles appear; Copy copies; Open URL opens a tab; Open File asks "Open file?" then reveals programs; Run Command in Window asks "Run command?" showing the whole line and OK opens a single-use window; Cancel or closing runs nothing. | |
| **F. Copy on selection, hover, overlays (world)** | | |
| 38 | With "Copy to clipboard on selection" on, drag-select text in a page: it is copied once and Paste History gets one entry; double-click a word: copied; Shift+arrows extend and copy once at the key release. With it off: nothing is copied. | |
| 39 | Hover a link: its URL shows in a small box at the pane's bottom-left (8 by 4 padding, 4 from the edges); hover a link at the bottom-left: the box moves to the bottom-right; the box never takes focus or clicks and does not move the cursor. | |
| 40 | A look-alike (IDN) host shows as xn-- punycode; a very long URL keeps the end of the host with wide letters in a narrow pane; hovering several links with different lengths in turn shows no flash of the old size. | |
| 41 | Move off the link: the box goes. Switch tab, click the address field, navigate, Alt+Tab away, minimize: the box goes. Rapidly sweep over many links: no flicker, no growing number of windows (Task Manager shows one tip window). | |
| 42 | A page that reports a link rectangle far from the pointer shows nothing. A tip near the screen's right edge with the Tandem window partly off screen: note whether the host end is cut (known: bounded by the pane, not the monitor). | |
| 43 | Copy Link Address (pointer action or menu) shows "Copied" for 1 second above the pointer and does not vanish early when a hover happens during it. | |
| 44 | An announcement bar (when 13x6 or 13x7 raise one) covers the top of the page and the page starts below it; each button answers once; closing answers nothing. A browser pane's toolbar sits flush under the title bar with no empty second strip, also beside an open Clippings or inline-chat panel. | |
| **G. Key bindings in a page** | | |
| 45 | Bind Ctrl+J to Send Text "hi" in the profile: with the page focused and no text field editing, "hi" goes to the focused element; in a text field Ctrl+J types into the page and your binding does not run (the editing rule, world). With the field not editing and a non-text element focused, the text goes to that element (ruled, iTerm2 parity: do not mark this a fail). With nothing focused but a caret left in a contenteditable the page blurred, the text goes at that caret (Chromium, ruled F4: not a fail). | |
| 46 | Bind Page Down to Scroll Page Down: the page scrolls when no field has focus; Home, End and Line scrolls too. Bindings on a plain character key (no Ctrl or Alt) do not act while the page has focus (ruled limit). | |
| 47 | A binding on Ctrl+Alt+J (Next Tab) switches tab while a page has focus and the page does not see the key. A binding on Bypass passes the key to the page. | |
| 48 | Copy or Send on Ctrl+C with a selection copies it (clipboard and Paste History); with none the letter goes into the field. Paste or Send, Paste... and Paste from Selection... insert into the focused field. Move end of selection by character, word, line extends the page selection. | |
| 49 | With an announcement bar showing, Alt+letter presses its button even with the page focused, and Alt+D with a bar present presses Deny, not the address field. | |
| 50 | Set Profile to a terminal profile does nothing on a browser pane; to a browser profile it switches. A pending Leader is dropped by the next key, including F6 and Ctrl+L. | |
| **H. Pointer bindings and four clicks (world)** | | |
| 51 | Bind middle click to Next Tab: middle-clicking a page switches tabs and the page gets no middle click and no autoscroll. Without the binding, middle click pastes the clipboard into the focused field. Bind a mouse button 4 or 5 (Back or Forward) to an action: it runs and the page does not navigate. | |
| 52 | Right click still shows WebView2's menu with Tandem's items; Ctrl+left click shows the same menu; a middle button bound to Open Context Menu shows a menu (note whether WebView2 raises it for a synthesized click). | |
| 53 | Open URL binding on a link opens it in place; Open URL in background opens a new tab; a link to a network file path shows a banner and opens nothing. | |
| 54 | Four clicks on an email select it; with Copy on Selection on it is copied, off it is not. Win-click on it runs its first action (Run Command asks first) and copies nothing; Win-click over text whose best rule has no action selects it and runs nothing. | |
| 55 | Extend Selection binding moves the selection end to the click; Jump to Selection (Find menu) scrolls the selection to the middle. Quick Look on an image or link shows the popover for the first URL. | |
| 56 | A page script that dispatches mousedown, mouseup and auxclick runs no binding (`synthetic_click_runs_nothing` by hand). A bound click inside an iframe does nothing (ruled). "Focus on right or middle click" on: the pane takes focus on an unbound press, and on a bound right or middle press before its action runs (iTerm2 focuses first). | |
| **I. Broadcast, arrangements, duplicate** | | |
| 57 | Two browser panes on `ok.html`, both broadcast sources, focus in the first pane's `#t1`, the second pane's `#t1` focused in its page: type "hi": the second shows "hi" once and the first once, the first stays focused and does not echo the text back. | |
| 58 | Backspace, arrows, Home, End and Tab in the first act in the second. Enter in `#ta` makes one line break in each; Enter in the form's search box submits in the first, and note whether it submits in the second (it should). Shift+letters type capitals. | |
| 59 | A terminal pane in the same broadcast set gets nothing from browser typing, and a browser pane's typing never reaches a terminal. A pane turned off as source, or closed, stops receiving. A Dev Null browser pane and a normal one in one set exchange nothing. A target page with no focused field ignores text (known; note it). | |
| 60 | Turn broadcast off while typing fast: at most a few characters arrive late (queued replays). | |
| 61 | Save a window arrangement with a browser pane on a page, restore it: the pane shows that page; a Dev Null pane restores to its profile's Initial URL; the arrangement file holds the URL without fragment or user information and a `Browser Target` of at most 256 characters. | |
| 62 | Quit and relaunch with a browser tab open in several windows: every restored window's page loads after the windows appear. Duplicate Session and Duplicate Tab on a browser pane open the same page. A browser profile whose Initial URL is a local `file:///C:/...` page, or `about:version`, loads it after a relaunch as it does in a new pane; one on `file://evil/s/x` shows the banner "Tandem will not open that file location.", never a silent blank pane. | |
| 63 | History (`iterm2-about:history`): a typed address shows Typed (so does a page Tandem loads itself: a restored page, a bookmark), a clicked link Link, Reload Reload, Back and Forward BackForward, a redirecting URL Redirect. | |
| 64 | Minimal tab style (after 13e): a browser pane's tab tint follows the page's background and changes at most four times a second. (Not applicable until 13e consumes `browser_page_color`.) | |
