# Web Browser

Termurion has a built-in web browser. A browser pane is a pane, tab, or window that shows a web page instead of a shell. You can split it, move it, search it, and use copy mode in it, like any terminal pane. Pages are drawn by the Microsoft WebView2 Runtime, so that runtime must be installed.

## Requirements

The WebView2 Runtime ships with current Windows 10 and 11 and with Microsoft Edge. If it is missing, Termurion asks before it opens a page. The question is "WebView2 Runtime Required", with buttons to download the runtime or use your system browser. If the runtime is in a folder that cannot be detected (a fixed-version runtime), set "Location of the WebView2 Runtime folder" in Settings > Advanced.

## Open a browser

There are two ways to get a browser pane.

1. Make a browser profile. Open Settings with Ctrl+Shift+, go to Profiles > General, and set "Profile type" to Browser. Any new tab or window opened with that profile is a browser. The default start page is a welcome page. The setting "Enable browser-style profiles?" (Advanced, on by default) controls these profiles. A change needs a restart.
2. Click a link in a terminal. Termurion asks "Open in Termurion?" with the buttons "Open in Termurion" and "Use Default Browser". Tick "Do not ask again" to remember the answer. The same answer is the setting "Open links using the in-app browser?". For a local file the choice is "Use Default App". The browser can show web formats, PDFs, and images.

## The toolbar

A browser pane has a toolbar with Back, Forward, Reload, and an address bar.

- Press Enter in the address bar. If the text looks like a URL, Termurion loads it. Otherwise it searches. The search engine comes from the Advanced setting "Template for URL of search engine" (Google by default, with `%@` standing for your text). Typing also shows suggestions, from "Template of URL for typeahead search suggestions".
- Press and hold Back or Forward for the history list.
- The toolbar menu holds Reader Mode, Remove Distractions, Ask AI, Add Bookmark, Manage Bookmarks, History, and Settings.

## Reader mode and distraction removal

Reader Mode shows the article text in a clean layout. Remove Distractions hides page clutter and keeps the page. Each has an "Exit" item in the same menu. Both are off for pages without a URL.

## Page context menu

Right-click a page for:

- Split Pane Vertically, Split Pane Horizontally
- Move Browser to Split Pane, Move Browser to Tab, Move Browser to Window
- Swap With Session
- Search with (your search engine) for selected text
- Add Named Mark
- Save Page As, Print
- Copy Page Title
- View Source
- Remove Element

Smart Selection actions and helper items for the selected text also appear. See [Smart Selection](smart-selection.md).

## Keys and copy mode

Most terminal key bindings work in a browser pane. Text size commands (Make Text Bigger, Smaller, Normal Size), Copy, and Paste reach the page. Copy mode works on a page text with the same keys as the terminal. See [Copy Mode](copy-mode.md).

## Bookmarks, history, and downloads

Bookmarks and history have their own pages, opened from the toolbar menu. A file the page downloads is saved only after Termurion checks its name, and Termurion never opens or runs it for you.

## Private browsing

A profile in Dev Null mode uses its own private store (an InPrivate-style session, a new folder each run). It has no bookmark or history items, and the AI cannot read its pages.

## Settings

Settings > Advanced, section Web Browser:

| Setting | Meaning |
|---|---|
| Enable ad blocking in the web browser? | Download and apply a filter list. Off by default. |
| URL for ad blocking filter list | A JSON list in WebKit content blocker format (EasyList by default). Refreshed every 24 hours. |
| Enable HTTP proxy for web browser? | Send all browser traffic through the proxy below. |
| HTTP proxy hostname or IP address, HTTP proxy port | The proxy (127.0.0.1 and 8118 by default). |
| Instant replay frame rate for web browser sessions | Frame rate of the page's instant replay. |

The toolbar menu's Settings item opens the browser's own settings page.

## Extensions

Termurion loads an extension only from a folder you chose, and only after you approve its manifest in a Termurion window. The default extensions folder is `%APPDATA%\Termurion\BrowserExtensions`.

## Passwords and autofill

When a page has a login field, a button lets you pick a stored credential from your password manager. Termurion fills it only after you pick it, and only into the page whose origin matches the stored account. See [Password Manager](password-manager.md).

## Triggers

Triggers can run in a browser pane too, for example to highlight text, make a link, or run script on matches. See [Triggers](triggers.md).

## Related pages

- [Smart Selection](smart-selection.md)
- [Copy Mode](copy-mode.md)
- [Triggers](triggers.md)
