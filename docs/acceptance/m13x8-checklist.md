# Milestone 13x8 acceptance checklist (browser pages and extensions)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run). Mark each line pass or fail.

Before anything else: `m13x8-rulings.txt` ends with PROBE and FLAGS FOR ROBERT, the behaviors that
differ from iTerm2. Section A lists the probe tests (none has run yet: Probe PENDING). Sections B to N
are what no test could execute (a real click, a real native window, a real download).

Test material: a folder served with `python -m http.server 8000` (`http://127.0.0.1:8000`); a folder
`C:\t13x8\` holding `a.txt`, `b.html` (with a `<script>` that writes "ran" into the page and tries
`fetch('c.txt')`, and a link to `https://example.com/`), `c.txt`, a `.png`, a `.js` and a subfolder.

## A. Probe tests (webhost_win::probe, all `#[ignore]`, console session only)

- [ ] `webhost_win::probe::failed_load_posts_its_url`
- [ ] `webhost_win::probe::server_answers_and_network_failures_differ` (404 with a body, empty 404, 503, a dead port, a 302 to the dead port, 204)
- [ ] `webhost_win::probe::file_pages_cannot_read_or_send` (the served file's script never runs: title stays, no connection, nothing refused because nothing is tried; R13x8-NS)
- [ ] `webhost_win::probe::tandem_loads_leave_a_file_page_for_the_web` (from a file page, Tandem's own load and a 302 hop land on the web, no 403)
- [ ] `webhost_win::probe::ssh_page_serves_text_and_a_closed_view_completes_the_deferral`
- [ ] `webhost_win::probe::download_lands_where_told`
- [ ] `webhost_win::probe::an_unpacked_extension_loads`
- [ ] `webhost_win::probe::redirect_onto_a_power_page_is_refused` (a server 302 to `iterm2-about:settings` from a Tandem page and from a web page)

Full run (from an ssh session on EXTREME, with Robert's yes and no implementer building in the
checkout; a PowerShell console window shows for the whole run; `probe: process session N` must show
N not 0, session 0 prints SKIPPED, which is not a pass):

```
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout>
```

Only the 13x8 tests: `-Filters` is a `string[]`, and `-File` passes a comma string as ONE filter,
so use a wrapper `C:\Users\datbi\src\run-13x8-probe.ps1` and run it the same way:

```
$f = @(
 'webhost_win::probe::failed_load_posts_its_url',
 'webhost_win::probe::server_answers_and_network_failures_differ',
 'webhost_win::probe::file_pages_cannot_read_or_send',
 'webhost_win::probe::ssh_page_serves_text_and_a_closed_view_completes_the_deferral',
 'webhost_win::probe::download_lands_where_told',
 'webhost_win::probe::an_unpacked_extension_loads',
 'webhost_win::probe::redirect_onto_a_power_page_is_refused'
)
& C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout> -Filters $f
```

Questions only the probe answers (write each answer in the rulings): the allowed-origin pattern for
the custom schemes, the Source of a sandboxed page, view Source's top_url for a file page, a trusted
Try Again click, a hidden ResultFilePath being honored, Chromium's own Mark of the Web, repeat
automatic downloads with Handled true, and extension add and enable.

## B. Onboarding

- [ ] A new browser tab (first ever, `NoSyncBrowserOnboardingCompleted` unset) shows onboarding-intro; Next shows features.
- [ ] The profile step: Create Browser Profile makes "Web Browser" (Settings > Profiles shows it); on a second visit the step says "Profile Exists" and creates nothing.
- [ ] The setup step: Ad Blocker and Instant Replay turn on (Settings and the profile show it).
- [ ] Finish lands on the welcome page and does not loop back to onboarding; a later tab skips onboarding.
- [ ] Nothing happens on any onboarding button until a real click (a script-dispatched click does nothing).

## C. Error page

- [ ] Open `https://nonexistent.invalid/`: "Server Not Found" with the URL and Try Again; Try Again retries that URL.
- [ ] Open a page with a bad certificate (a local test server with a self-signed certificate): "Secure Connection Failed", no "continue anyway", no Edge page.
- [ ] A very long URL or one with `<b>` and quotes in it shows as text, cut with the end of its host kept; nothing runs.
- [ ] A server that answers 404 (or 503) with its own body shows the SERVER's page, not Tandem's error page.
- [ ] Stop a load part way: no error page for a cancelled load.
- [ ] A site blocked by the ad blocker shows Tandem's blocked page, not a Chromium 403.
- [ ] On a machine with an old WebView2 runtime (if you have one): the banner "This WebView2 runtime is too old" shows and no browser opens.

## D. File pages

- [ ] Type `file:///C:/Windows/`: the styled listing, folders first (with folder glyphs), then names ignoring case; hidden entries are not listed.
- [ ] Click `..` up to `C:\`: at the drive root there is no `..` row.
- [ ] Open `a.txt` from the listing: shown as text in a box; a `.png` shows as an image; a `.js` shows as text.
- [ ] Open `b.html` you made yourself: it renders WITHOUT its script (no "ran"; R13x8-NS, stricter than iTerm2: local HTML apps do not run). An `.svg` and an `.xml` file run no script either.
- [ ] Its link to `https://example.com/` does nothing (R13x8-P2); typing the address in the address bar from that tab goes there.
- [ ] `file://server/share/x`, `file:///C:/a/../b`, `file:///C:/x.txt:stream` and a device name (`file:///C:/con`) are refused ("Invalid file path"), with no network probe.
- [ ] A file over 64 MB is not shown.
- [ ] A junction inside the folder that points outside it does not lead out.
- [ ] A web page (http://127.0.0.1:8000) with a link to `file:///C:/t13x8/a.txt`: clicking it does nothing (refused even on click).

## E. Settings page

- [ ] Menu > Settings opens `iterm2-about:settings`.
- [ ] Clear Cookies (the page's own confirm): sign in to a site, clear, reload: signed out.
- [ ] Clear All Data: a NATIVE Tandem window asks first; Cancel clears nothing; OK empties the history page and the bookmarks page, and the page says so.
- [ ] Search engine: each built-in choice changes what the URL bar searches. Custom with a URL lacking `%@` shows the error; an `http://` template is refused (https only); a template with two `%@` is refused.
- [ ] Ad blocking: the toggle works; statistics show a rule count; Update Now reports the result once (invalid list address, download fail, or success); a list address that is not https is refused.
- [ ] Proxy: host and port save; port 0 or 70000 shows the error; a host with a space, `--flag` or a user name shows "Invalid proxy host".
- [ ] Link opening: Always Ask brings back the "Open in Tandem?" question; the other choices change what a clicked link does.
- [ ] Manage Passwords opens the password manager; Manage Permissions opens `iterm2-about:permissions`.
- [ ] In a Dev Null pane the settings page shows its Dev Null note.
- [ ] Rapid clicking a privileged button (Clear, Update Now, proxy) acts at most about once a second.

## F. Back and Forward onto power pages

- [ ] Open Settings, go to a web page, press Back: the settings page comes back (loaded fresh by Tandem), no wrong banner. Same for Forward and the long-press history list.
- [ ] A web page that calls `history.back()` or `history.go(-1)` while the settings page is the previous entry does not land on it.
- [ ] A web page cannot navigate to `iterm2-about:settings`, `iterm2-about:downloads` or `iterm2-file:` by click, script or redirect.
- [ ] A local HTML file with a link to `https://example.com/`: clicking it does nothing (a file or ssh page leaves for no other page, R13x8-P2); the address bar still goes there.

## G. Downloads

- [ ] Download a file from a page: it lands in Downloads under the URL's name, a notification names the final name, progress shows as a banner.
- [ ] Download it again: the second is numbered (`name (1).ext`), the first is untouched.
- [ ] A dangerous type (`.exe`, `.bat`, `.js`) is saved renamed with `.download` and is never run or opened.
- [ ] Right-click the saved file > Properties: it carries the "unblock" mark (Mark of the Web, internet zone).
- [ ] A name Windows forbids (`a:b`) is saved cleaned (`a_b`).
- [ ] From a local test server, a download at `/..%5C..%5Cx.exe` lands in Downloads as `x.exe` (`.download` renamed) and nowhere else.
- [ ] Start 4 downloads in one pane: the 4th is cancelled with a banner.
- [ ] In a Dev Null pane a download is refused with a banner and nothing is written.
- [ ] `iterm2-about:downloads` (type it in the URL bar) lists downloads with Cancel, Remove and Show in folder; there is no Downloads menu item.
- [ ] After a download ends, later downloads still work (no stuck count).

## H. Bookmark tags and Edit Bookmark

- [ ] Add Bookmark on a page opens the Edit Bookmark window (always on top, centered on the browser window) with Title, URL and Tags; type tags separated by commas and see completions; Save stores them.
- [ ] Add Bookmark on an already bookmarked page removes it and toasts.
- [ ] Delete in the editor asks first, then removes the bookmark; the question shows above the editor.
- [ ] Navigating the pane or closing it closes the editor.
- [ ] No editor opens in a Dev Null pane.
- [ ] The bookmarks page lists the tags; filtering by one tag, then by two, shows only bookmarks that carry every chosen tag; text search plus tag works.
- [ ] A tag like `<b>x</b>` is shown as text. Enter with a completion list open saves (a known parity detail).

## I. The ssh page and View

- [ ] In an ssh-integrated session Win-click a remote `.png`: "Download or View File?"; View opens it in a browser tab; Download still downloads.
- [ ] Win-click a remote `.html`: View shows it with no script running (remote HTML files display without scripts, R13x8-NS).
- [ ] A remote folder name asks the same question and View shows "Only a file can be shown."
- [ ] Close the connection and reload the tab: the "Could not display file" page; its Try Again (a link, the page runs no script) reloads the page.
- [ ] The URL of another host, user or port than an open session shows the no-connection page; typing an `iterm2-ssh:` URL for your open session works, and a link inside a served page never leads to another ssh or file page.
- [ ] A web page cannot open `iterm2-ssh:` by script or click.

## J. Extensions

The settings page's Extensions section is HIDDEN (FLAGS FOR ROBERT 8): there is no control to choose an
extensions folder, so the lines below are BLOCKED until the section is shown (one flag,
`show_extensions`) or Robert rules otherwise. Mark them blocked, not failed. Once shown:

- [ ] Choose Folder opens a native folder picker; choosing an unpacked extension folder (or its parent) lists the extension with its name, version and description ("No description available" when none).
- [ ] Enabling asks in a native Tandem window that lists every permission and every site and must be scrolled to its end; Cancel loads nothing; Enable loads it into the next browser tab and it acts.
- [ ] A manifest with `optional_permissions` or `optional_host_permissions` lists them in the window under "It may ask later for:".
- [ ] A manifest asking for `debugger` or `nativeMessaging` shows a WARNING line first in the window. In a Dev Null pane the settings page shows an empty extensions list and Reveal says it is not allowed.
- [ ] A manifest name with `<b>` or a script tag shows as text on the page and in the window.
- [ ] The approval window says the extension is loaded into "all browser profiles that share this browser data"; after approving in one browser profile, a tab of another browser profile has it too, its switch on the settings page is the same switch, and a Dev Null pane has neither (ROBERT FLAG 8b).
- [ ] Open a browser tab of a profile with no extensions folder chosen: the settings Extensions list still shows the approved ones, and deleting an approved extension's folder then opening any tab removes it.
- [ ] Edit the extension's `manifest.json`: it asks again; edit only a script file: it does not (FLAG: only the manifest is hashed).
- [ ] Delete the folder, open a new browser tab: the extension is gone and removed from the profile.
- [ ] A Browser Extensions Root or Active IDs value that arrived by editing the profile file, import or sync loads nothing without the approval.
- [ ] A Dev Null pane never loads extensions and refuses the picker.
- [ ] An extension's own page (`chrome-extension:`) cannot open settings, file, ssh or downloads pages.

## K. Web pages can never reach local pages

- [ ] From a page on `http://127.0.0.1:8000`, a link, a script `location=`, a meta refresh and a redirect to each of `iterm2-about:settings`, `iterm2-about:downloads`, `iterm2-about:onboarding-setup`, `iterm2-file:///C:/`, `iterm2-ssh://...` and `file:///C:/` do nothing.
- [ ] A web page's script cannot post a message that clears data, changes settings or starts a download action (DevTools: `window.chrome.webview.postMessage` with any shape does nothing).
- [ ] An `iframe` of a Tandem page inside a web page is refused (frame-ancestors none).

## L. Security of local pages

- [ ] A saved HTML page opened through `file:` runs none of its scripts (DevTools console shows the CSP refusal), cannot reach the network, submit a form or open a window.
- [ ] An outbound link in a local or remote `.html` does nothing (FLAG 1); the address bar, a bookmark or the menu still leave the page for the web.
- [ ] No Tandem script (autofill button, password key button, find, marks) runs inside a file or ssh page.

## M. Nothing leaks

- [ ] Look through `%LOCALAPPDATA%\Tandem` logs after these steps: no file path, remote URL, download name, tag, proxy host or password appears (words and counts only).

## N. Existing browser behavior

- [ ] The pages of 13x to 13x7 (welcome, bookmarks, history, permissions, source, autofill, password manager, HTTP sign-in) still work.
- [ ] Tab and window close with a download or an editor open leaves nothing running (Task Manager shows no stray `Tandem` process).
