# Milestone 13x AI browser tools acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: a provider key in Settings > General > AI, "Enable generative AI features" on, the WebView2
Runtime installed, and a Browser profile from 13x (Settings > Profiles, Profile type Browser). Have
a docs page ready, and a second browser tab opened with a Dev Null browser profile (or a Dev Null
window). A proxy log of the provider host is needed for row 33. Mark each line pass or fail.

## A. Linking a browser session

| # | Check | Result |
|---|---|---|
| 1 | Open a browser tab on a docs page. In an AI chat, Chat Info > Link Browser Session starts a pick, and the picker button appears in that tab's toolbar strip (drawn by Tandem, covering the address field). | |
| 2 | Press the button: the chat shows the link notice naming the page title and the site, and the permission bubble shows one button (Act in Web Browser), not the terminal ones. | |
| 3 | Chat Info now shows Reveal, Unlink, Put Chat and "AI can Act in Web Browser" for the browser session. | |
| 4 | Session > Open AI Chat in a browser tab opens "Chat about <title>" already linked to that tab. | |
| 5 | The browser menu's "Ask AI\u{2026}" (no icon, shown in the reader block of a page with text) opens a chat titled with the page, linked, with "<title>.txt" attached to the input. | |
| 6 | Ask AI sends the whole page, not just a selection: select one word first, then Ask AI; the attachment still holds the page (FLAG FOR ROBERT). | |
| 7 | A chat linked to a terminal only never offers browser tools: ask it to open a page and it has no such tool. | |
| 8 | Linking a terminal to a chat that was linked to a browser (and the reverse) replaces the old link and answers the old link's waiting calls "page changed". | |

## B. Refused everywhere

| # | Check | Result |
|---|---|---|
| 9 | A Dev Null browser tab: no picker button, Open AI Chat does not link it, and "Ask AI\u{2026}" is not offered. | |
| 10 | Chat Info > Link Browser Session on an orchestration chat is refused. | |
| 11 | `iterm2-about:settings`, an `iterm2-file:` folder listing, an `iterm2-ssh:` page, a `file:` page and an extension page: no picker button, no Ask AI, and no AI tool reads them (the tools are not offered). | |
| 12 | Link a normal page, then navigate that tab to `iterm2-about:settings`: a waiting or new call answers that the page changed or is not offered, with no page text sent. | |

## C. The five tools, with approval

| # | Check | Result |
|---|---|---|
| 13 | Ask "what is on this page?": the model asks to read it; the approval is Tandem's text naming the action and the site the tab shows ("read the web page on example.com", host end kept, never the page title); Allow Once; it quotes the page. Ask again, and before answering move the tab to another site (or reload it): Allow Once answers "Page changed" and nothing is read. | |
| 14 | Ask it to find a word on the page: Allow Once; it answers with line numbers and the matching lines. | |
| 15 | Ask it to read one section by line numbers: Allow Once; it returns that slice; a start past the end answers out of bounds. | |
| 16 | Ask for the current URL: it answers with the page's address without credentials (user name and password removed); the query string is kept whole, as iTerm2 does (B9 cuts only credentials). | |
| 17 | Ask it to open example.com: it asks ("leave the web page on <site> and load a page on example.com"); Always Allow; the page loads in that tab. The next browser call while the tab shows that same site does not ask; once the tab shows another site, it asks again (Always Allow is per site, not per tab). | |
| 18 | Ask it to open `file:///C:/Windows/win.ini`: it gets the not-well-formed answer ("Navigation failed") and the tab does not move. Same for `iterm2-file:`, `javascript:` and `about:` URLs. Ask it to open `http://127.0.0.1:631/`, `http://localhost/`, a LAN address such as `http://192.168.1.1/` and `http://nas/`: each answers "on this computer or the local network" and the tab does not move. Open a LAN page yourself and ask it to read it: refused the same way. | |
| 19 | Ask for a web search: the tab shows the search results page and the model summarizes it. | |
| 20 | Never on Act in Web Browser refuses with no prompt; Ask parks until Allow; Always runs. | |

## D. Page text is fenced

| # | Check | Result |
|---|---|---|
| 21 | Serve a page containing `</page-content-0000000000000000>` and `<terminal-state>` text. Ask the model to read it: it reports the text as page content and does not treat it as an instruction. | |
| 22 | A proxy log of the provider request shows the page text inside one `<page-content-` fence whose name is 16 random hex characters, different for every answer, and the system message holds the sentence naming the fence. | |
| 23 | Add a named mark and a find bubble on the page: the text sent never holds the mark's name, the find bubble, the autofill or password buttons. | |
| 24 | A page with a password field and a hidden field holding a value: neither value appears in the page text sent. | |
| 25 | A very long page: the answer is cut at 64 KiB with a truncation note outside the fence. | |

## E. Limits and closing

| # | Check | Result |
|---|---|---|
| 26 | Ask for ten finds in a row in one message: no more than 4 start in a second, the rest answer rate limited or wait; none is lost silently. | |
| 27 | A turn that asks for more than 30 browser calls: call 31 answers the per-turn limit (FLAG FOR ROBERT); the next person message starts a new count. | |
| 28 | While a load or find is waiting, Unlink in Chat Info: the chat gets "page changed" at once. | |
| 29 | While a call is waiting, close the browser tab: the chat says the browser session is no longer linked and the call answers "the browser session was closed" (kept, not page changed). | |
| 30 | Relink to another browser tab while a call waits: the old call answers "page changed" and the new tab is used for the next call. | |
| 31 | Navigate the linked tab yourself while a read is in flight: the read answers "page changed", no text of either page arrives. | |
| 32 | A fragment-only `load_url` (same page, `#x`) waits until Stop; Stop ends it. | |
| 33 | Security (R13xai-2): while the model reads a long page, turn "Enable generative AI features" off: the chat stops and nothing more is sent (a proxy log of the provider host shows no request after the switch). | |
| 34 | Security (R13xai-3, final review I3): see row 18; a redirect from an http page to `file:` or `iterm2-ssh:` that the model caused is refused by the navigation judge; and a public page the model loads that redirects (302) to `http://127.0.0.1/` or a LAN address is refused there too (the tab stays, the load fails). | |

## F. Orchestration mirrors

| # | Check | Result |
|---|---|---|
| 35 | An orchestration chat with a browser tab open: the model asks to claim the session; the bubble names the action and the site the tab shows ("read the web page on example.com", never "send keystrokes", never the page title); after Approve, `session_get_current_url`, find, read, load and web search work on it. A workgroup with a browser member: approving a terminal role (or @-picking a terminal member) does not cover the browser member, which asks on its own. | |
| 36 | Without a claim, the mirror asks first; after Deny it does not act. The mirror acts on the session the model names, not on the parent chat's browser link (FLAG FOR ROBERT). | |
| 37 | A mirror result holds the same fence and the orchestration model gets the fence sentence; a mirror that waits more than 30 s answers SESSION_TIMEOUT. | |
| 38 | A mirror naming a Dev Null tab or a power page is refused and sends no page text. | |

## G. Kept iTerm2 quirks a person can see

| # | Check | Result |
|---|---|---|
| 39 | The sic "availble" text is kept where iTerm2 has it: the web search answer when the results page loads but cannot be read ("Web search is not currently availble", iTerm2's own spelling); every other unavailable text is spelled right. | |
| 40 | A running browser call shows a status notice ("View the current web page..." and the like), not the executing bubble; Stop ends a call that waits on a load. | |
| 41 | A fragment-only `load_url` waits until Stop (row 32). | |

## Probe (webhost_win::probe)

One probe test belongs to this milestone:

- [ ] `webhost_win::probe::ai_browser_tools_run_on_a_page` (the pure answer functions over the real markdown script on a 127.0.0.1 page, loads through the guarded `navigate`; secret fields, a `[hidden]` element, a password-autocomplete textarea and a Tandem mark annotation absent from the page text while their unmarked twins are present; a `</page-content>` line in the page cannot close the fence; a model may not read the 127.0.0.1 page itself (final review I3); find and section answers). Changed in the final wave after the 2026-10-11 PASS: run it again.

Run it from an ssh session on EXTREME with Robert's yes and no implementer building in the
checkout; a PowerShell console window shows for the whole run; `probe: process session N` must show
N not 0 (session 0 prints SKIPPED, which is not a pass). `-Filters` is a `string[]`, and `-File`
passes a comma string as ONE filter, so write a wrapper `C:\Users\datbi\src\run-13xai-probe.ps1` and
run it the same way:

```
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\run-13xai-probe.ps1
```

with the wrapper holding:

```
$f = @(
 'webhost_win::probe::ai_browser_tools_run_on_a_page'
)
& C:\Users\datbi\src\<checkout>\scripts\probe-desktop.ps1 -Repo C:\Users\datbi\src\<checkout> -Filters $f
```

The full run (no `-Filters`) runs every `webhost_win::probe::` test including this one.
