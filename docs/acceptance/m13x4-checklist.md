# Milestone 13x4 acceptance checklist (browser reader, source, saver)

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe` on a machine with the
WebView2 Runtime installed (one run). Mark each line pass or fail.

Before anything else: `m13x4-rulings.txt` line "Probe:" records the 13x4 desktop probe (PASS, 34
of 34 on EXTREME on 2026-10-10, run c). Section A lists the probe tests; sections B to F are what no test
could execute (a real page, a real drive, the real Explorer). Reader Mode, Remove Distractions and
Remove Element cannot be compared with a running iTerm2 (iTerm2's never run at `64791fc`, see the
rulings); the expected behavior is written on each line. Rows marked (world) are inert only if the
view's isolated world is unavailable, which is a fail when section A passed.

Test pages (serve a folder with `python -m http.server 8000`): `article.html`, a news-style page with
a headline, 8 paragraphs, an image, a cookie banner (`position:fixed`) and an ad `<div>`;
`short.html`, `<title>short</title><p>hi</p>`; `links.html`:

```
<title>Tandem 13x4 links</title>
<p><a href="JAVASCRIPT:alert(1)">js link</a></p>
<div style="background:url(javascript:alert(1))">css js</div>
<p><a href="https://example.com/">example</a> <a href="a.exe::$DATA">stream link</a></p>
<img src="pic.png"><link rel="stylesheet" href="s.css">
```

(put a small `pic.png` and a `s.css` next to it).

## A. The desktop probe (the controller runs it with Robert's yes)

Command: `scripts/probe-desktop.ps1` (default filters `webhost_win::probe::` and
`webhost_win::tests::menu_gets_our_items`: 33 probe tests and the menu test, 34). The eleven new
tests are:

- [x] `reader_mode_runs_in_a_page`
- [x] `reader_article_handlers_stay_in_the_page_world`
- [x] `page_never_sees_the_view_secret`
- [x] `markdown_is_turndowns`
- [x] `remove_element_runs_in_a_page`
- [x] `devtools_reads_a_page`
- [x] `view_source_reads_the_original`
- [x] `view_source_arming_facts`
- [x] `view_source_shows_the_page`
- [x] `save_page_writes_a_folder`
- [x] `extend_word_forward_stops_before_the_space`

All 34 passed on 2026-10-10 (run c, after one product fix and the probe fixes; m13x4-rulings.txt).

## B. Reader Mode, Remove Distractions, Remove Element (world)

- [ ] Open `article.html` in a browser pane. The toolbar menu shows Reader Mode, Remove Distractions and "Ask AI..." (disabled). Right after the first load the reader entries may take a moment (the page text is read after the first Menu press).
- [ ] Reader Mode: the article alone, styled, with its headline; the banner and ad are gone. The menu now says "Exit Reader Mode". Escape returns the page; the menu says Reader Mode again.
- [ ] Reader Mode on a real news site (any large article): it enters in under a second or two and the window stays responsive.
- [ ] Remove Distractions: the pointer becomes a crosshair; clicking the cookie banner removes it; clicking the article text does not remove the whole page; Escape ends it and the menu says "Remove Distractions" again.
- [ ] On `short.html` (no article) the reader entries are absent; Remove Distractions is still offered.
- [ ] Navigate to another page while in Reader Mode: the new page is not marked as in reader mode and the menu says Reader Mode.
- [ ] A page that sends a synthetic Escape key event does not leave Reader Mode; the Escape key you press does.
- [ ] Right-click the ad and choose Remove Element: the ad is removed; right-click the headline and choose Remove Element: only that element goes.

## C. View Source (works in a real page and in a frame)

- [ ] Right-click a real page, View Source: a tab opens `iterm2-about:source` showing "Page Source" with highlighted source and clickable links. Click a link: a new browser tab opens it. Back returns to the page. Reload of the source tab shows "No source available".
- [ ] The source shown is the page's original text (view-source of `article.html` shows the HTML you wrote, not the scripted DOM).
- [ ] Open `links.html`, View Source: clicking the linkified `JAVASCRIPT:alert(1)` and the `url(javascript:alert(1))` opens nothing.
- [ ] Right-click inside an iframe (use a page with an iframe) and choose View Source: it does NOT open a source page for the frame (refused; the main page's source or nothing, never an error page). Typing `iterm2-about:source` in the address field with no pending source shows "No source available" or a 404, not a previous page's source.
- [ ] A very large page (several MB) opens its source, or shows "No source available" at the 4 MiB cap, without hanging.

## D. Save Page As to an NTFS folder

- [ ] Right-click `links.html` (images and CSS), Save Page As..., pick `C:\Users\datbi\Desktop` and name `saved1`: a folder `saved1` holding `index.html` and a `resources` folder is created and Explorer shows `index.html`.
- [ ] Disconnect the network (or stop the web server) and open `saved1\index.html` offline in a browser: the page, its image and its CSS show.
- [ ] Mark of the Web on every file: in PowerShell run `Get-ChildItem saved1 -Recurse -File | ForEach-Object { "$($_.FullName): " + (Get-Content -LiteralPath $_.FullName -Stream Zone.Identifier -ErrorAction SilentlyContinue | Out-String) }`. Every file prints `ZoneId=3` and a `HostUrl=` of the page.
- [ ] Save to an existing folder name (save again as `saved1`): "Save Failed" with the cause; nothing in `saved1` is changed or deleted.
- [ ] Save to a network path (a mapped drive's UNC path): "Save Failed" with "Network locations are not supported for Save Page As."
- [ ] Save to a FAT or exFAT USB stick: a clear Save Failed (no Mark of the Web on that drive) and nothing left behind on the stick (no folder, no files).
- [ ] Cancel mid-save: start a save of a large page, then re-run Save Page As or close the pane while it writes: the partial folder is removed (and an existing folder of the same name is never touched).
- [ ] Cancel at once: choose the folder, then re-run Save Page As (or close the pane) within a second: no empty folder is left.
- [ ] A page with a 5 MB photo saves the photo into `resources`; a resource over 8 MiB keeps its web address in `index.html`.
- [ ] The Save Failed text names the error, never text taken from the page.

## E. Risky names and links (changes on main)

- [ ] Download an `.exe` through a page link (an inline download): the file is saved as `x.exe.download`, and opening it from Tandem (a path link, semantic history) reveals it in Explorer and never launches it.
- [ ] In `links.html`, click `a.exe::$DATA`: it is revealed in Explorer or refused, never opened.
- [ ] A `.cmd`, `.lnk` and `.js` saved the same way end in `.download`.
- [ ] A file link to a normal `.txt` still opens.

## F. Parity and page text

- [ ] The page's plain text drives the toolbar menu's reader offer only after the first Menu press (a short delay the first time, as iTerm2); a page that navigates between the press and the answer never gets the old page's offer.
- [ ] Right-click menu on a page shows Save Page As..., Print..., View Source and Remove Element (Print is 13x2's).
- [ ] A browser pane in a hidden tab does not get a popup menu from a late reader answer.
