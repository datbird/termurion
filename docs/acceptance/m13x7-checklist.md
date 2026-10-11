# Milestone 13x7 acceptance checklist (browser autofill and passwords)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run). Mark each line pass or fail.

Before anything else: `m13x7-rulings.txt` ends with PROBE and FLAGS FOR ROBERT, the behaviors that
differ from iTerm2. Section A lists the probe tests (none has run: Probe: PENDING). Sections B to J
are what no test could execute (a real click, a real native window, a real sign-in).

Test pages (serve a folder with `python -m http.server 8000`; that origin is `http://127.0.0.1:8000`,
plain http, so it must NOT fill or save). For https use any public sign-up form and any https site
you have an account for. `form.html`:

```
<form>
<input name="first" autocomplete="given-name"> <input name="email" type="email">
<input name="phone" type="tel"> <input name="zip" autocomplete="postal-code">
<input name="user"> <input name="pw" type="password">
</form>
```

For HTTP authentication use any local tool that answers 401 with `WWW-Authenticate: Basic realm="x"`.

## A. Probe tests (webhost_win::probe, all `#[ignore]`, console session only)

- [ ] `webhost_win::probe::engine_autofill_off`
- [ ] `webhost_win::probe::autofill_press_requests_and_the_confirmed_fill_writes`
- [ ] `webhost_win::probe::password_button_appears_on_focus_and_scripted_click_opens_nothing`
- [ ] `webhost_win::probe::password_writer_writes_only_a_visible_password_field`
- [ ] `webhost_win::probe::basic_auth_round_trip`
- [ ] `webhost_win::probe::a_view_without_basic_auth_cancels`
- [ ] `webhost_win::probe::integrated_challenges_reach_the_app` (R13x710-1: an NTLM/Negotiate challenge reaches the app, nothing is sent)
- [ ] `webhost_win::probe::proxy_challenge_reports_the_proxy` (R13x710-2: whether a 407 names the proxy or the site)

Gap: no probe dispatches a TRUSTED click (`Input.dispatchMouseEvent`) on the autofill or password
button (Task 3 and Task 7 reviews); the probes click with untrusted events only and prove those do
nothing. The trusted press is lines in sections B and E below, by hand.

Full run (from an ssh session on EXTREME, with Robert's yes and no implementer building in the
checkout; a PowerShell console window shows for the whole run; `probe: process session N` must show
N not 0, session 0 prints SKIPPED, which is not a pass):

```
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout>
```

Only the 13x7 tests: `-Filters` is a `string[]`, and `-File` passes a comma string as ONE filter,
so use a wrapper `C:\Users\datbi\src\run-13x7-probe.ps1` and run it the same way:

```
$f = @(
 'webhost_win::probe::engine_autofill_off',
 'webhost_win::probe::autofill_press_requests_and_the_confirmed_fill_writes',
 'webhost_win::probe::password_button_appears_on_focus_and_scripted_click_opens_nothing',
 'webhost_win::probe::password_writer_writes_only_a_visible_password_field',
 'webhost_win::probe::basic_auth_round_trip',
 'webhost_win::probe::a_view_without_basic_auth_cancels',
 'webhost_win::probe::integrated_challenges_reach_the_app',
 'webhost_win::probe::proxy_challenge_reports_the_proxy'
)
& C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout> -Filters $f
```

## B. Autofill: the button only requests

- [ ] On an https sign-up form an autofill button appears beside the name, email, phone and address fields; none appears on a password field, on the plain http `form.html`, or inside an iframe.
- [ ] Pressing a button (a real click) fills NOTHING yet: a native Tandem question opens that names the site (real origin, punycode shown as is) and lists each field type it would fill, with Fill and Cancel.
- [ ] In DevTools (F12) `document.querySelector('[data-iterm-autofill]')` finds nothing (the button is in a closed root) and a scripted `.click()` on anything opens no question.
- [ ] Session > Open Autocomplete in the browser pane opens the same question for the whole form; Fill All only requests too.
- [ ] The question answers Fill only to a mouse click or a touch tap. Tab, Space and Enter do nothing and nothing is focused or default; Escape cancels (the only key). For the first second after it opens (and after switching to another window and back) a click does nothing; a finger held on Fill through the 1 s guard does not fill when it lifts.
- [ ] Fill: the page fields fill and flash, only the types listed, only into this origin.
- [ ] Navigate the pane (or reload) while the question is open: the question closes and a late click fills nothing. After a navigation a fill already asked for is dropped.
- [ ] A trusted press on the page's own look-alike button (a page that draws a fake "Autofill" button) does nothing.
- [ ] Small fields (a short ZIP or state field under 200 px wide and 25 px tall) get no button (iTerm2's rule, a known limit).
- [ ] No Edge address or password bubble appears anywhere in the browser pane (engine autofill is off).

## C. The contact card

- [ ] The first fill with an empty card opens the contact card editor; OK saves it and the waiting fill continues with its question; Cancel drops the fill, and a new page load asks again.
- [ ] The card file is `autofill-contact.dat` in `%LOCALAPPDATA%\Tandem` (not beside the globals, not readable as text); File > Export All Settings carries no card value.
- [ ] Delete the file: the editor opens again; a corrupt file counts as an empty card.

## D. Password button in pages

- [ ] On an https login page a key button appears in the password field; it does not appear on the http `form.html`, in an iframe, or on `iterm2-about:` pages.
- [ ] A real click on the key button opens the Tandem password manager titled for the browser (gear menu without Probe and Send Return). A scripted click does nothing.
- [ ] The manager opened from the page takes arrows, Home, End, Page keys and Escape only: Return, Space, Tab, Delete, Ctrl+C and typed keys do nothing, Win+B/U/C do nothing, and for the first second a click does nothing. An account is picked by a click or a tap only (a finger held through the guard does not pick).
- [ ] The manager lists ONLY accounts named exactly for this page's https origin (for example an account named `https://example.com - me`); an account for a subdomain, another site or http is hidden; on the http page the list is empty.
- [ ] Nothing is typed into the page until an account is picked; after the pick the password appears in the visible password field, the page then shows it filled.
- [ ] Navigate away between opening the manager and picking: nothing is typed.
- [ ] A text field (not a password field) refuses the write with a toast; no "fill it anyway" question exists.
- [ ] The manager's source choice is separate from a terminal window's; a Credential Manager account added here shows under `Tandem-Browser/` in Control Panel > Credential Manager and not in a terminal's manager; unlocking one store leaves the other locked.
- [ ] Enter Username & Password (opened by the person, not a page) fills the user name, moves to the password field and fills it; a terminal pane's manager Enter never writes into a browser pane.

## E. HTTP authentication (the sign-in prompt)

- [ ] A 401 Basic page (real https site or a proxy challenge) opens a native Tandem sign-in window that names the real origin (or "proxy"), shows the server's realm only after "The site describes this sign-in as (text from the site, not from Tandem):" as plain text (an over-long realm is cut), and shows the cleartext warning over http. WebView2's own dialog never appears.
- [ ] The prompt only appears for the shown pane: switch to another tab and the sign-in is cancelled (the page shows its 401); a 401 loading in a hidden tab asks nothing.
- [ ] Remember this password then OK signs in; after a Tandem restart the saved password only PREFILLS the prompt, and the prompt never submits on Enter while prefilled (OK by click, or type something first); there is no silent sign-in.
- [ ] A saved password the server rejects is not prefilled or resent again this session and Remember is off on the re-prompt; the page does not loop.
- [ ] Cancel: the page shows its 401 and that site stays quiet for 60 s (reloading asks nothing), then asks again.
- [ ] A page that reloads itself after every 401 cannot flood: at most one prompt open and four waiting, the rest cancelled; closing the tab with prompts open cancels all.
- [ ] Over plain http: the cleartext warning shows, nothing is looked up, nothing is offered to remember (no Remember box saves for http).
- [ ] Dev Null Mode: no Remember checkbox, no Password Manager button, no lookup.
- [ ] The Password Manager button on the prompt lists only accounts for the challenge's exact https origin and a pick fills the prompt.
- [ ] Windows integrated sign-in is never automatic: against an intranet site that offers NTLM or Negotiate (if you have one) Tandem prompts or fails, it never signs in with your Windows account by itself.
- [ ] With BrowserProxyEnabled on and a proxy that asks for a password: the prompt names the proxy, nothing is looked up or remembered for any site, and Remember does not offer to save.
- [ ] Terminal password manager unaffected: in a terminal pane Password Manager still works as in 10b and lists only terminal accounts.

## F. Nothing leaks

- [ ] Look through `%LOCALAPPDATA%\Tandem` logs after a fill and a sign-in: no card value, password or typed user name appears (hosts and counts only).
- [ ] A page script listening on `message`, `postMessage` or the console cannot see the autofill secret, the card or a password.
