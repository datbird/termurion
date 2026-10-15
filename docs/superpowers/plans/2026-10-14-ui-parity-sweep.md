# UI parity sweep: every menu, window, Settings control and label as iTerm2 has it

Robert (2026-10-14): the terminal right-click menu is missing about a dozen of iTerm2's items. It
was hand-copied from a short summary and never checked against iTerm2's source. Three audits then
found the same problem elsewhere. "Fix all of it", and "labels too, use iTerm2's exact wording".

## Rules for every package

- The source of truth is the iTerm2 reference clone (`iterm2-ref`, commit 64791fc): the xibs and
  the code that builds menus, alerts and views. Never a parity row, a plan or a summary. Read the
  iTerm2 code for each item: its title, its order, the condition that shows or enables it, and
  what it does.
- Exact wording. The only rewrites allowed are the Mac words Termurion already maps
  (`menus::shown_label`): Finder is Explorer, Cmd (⌘) is Win, Option (⌥) is Alt, Control is
  Ctrl, Return is Enter. Keep iTerm2's punctuation (… not ...). A label that names "iTerm2" as
  the app says "Termurion".
- macOS-only items (Services, AutoFill, Look Up, Share sheet, Touch Bar, trackpad gestures,
  proxy icon) are not built. Each one is listed in the package's rulings file as macOS only.
- An existing ruling that rules an item out stays ruled out. Name it in the rulings file.
- Every package adds a guard test that holds its surface to iTerm2's list (titles and order,
  typed into the test from the iTerm2 source with file:line), so a short copy cannot return.
- Update `docs/parity/*.md` rows that said "have" while the UI had no control, and add a row for
  each surface that had none (for example "Terminal context menu", per item).
- Rulings: `docs/superpowers/rulings/ui-sweep-<package>-rulings.txt`, with a FLAGS FOR ROBERT
  block at the end for every choice that differs from iTerm2 or that iTerm2 cannot answer on
  Windows.
- Repo is public: no host names, user names or home paths in code, docs or commits. No
  Claude attribution in commits. Commit author is the repo's configured author.

## Audits (the item lists)

- `docs/superpowers/audits/2026-10-14-ui-menus.md`: code-built menus (10 with gaps).
- `docs/superpowers/audits/2026-10-14-ui-settings.md`: Settings window (11 panes with gaps,
  plus label drift).
- `docs/superpowers/audits/2026-10-14-ui-windows.md`: windows, sheets, bars and alerts (21 with
  gaps). The audits were done by reading code; re-check each item against iTerm2 before
  building it, and record any audit item that turns out wrong.
- The terminal right-click menu (this plan, package A): iTerm2 `sources/ContextMenu/
  iTermTextViewContextMenuHelper.m` (`menuForEvent` and the menu built from line 380), against
  Termurion `src/commands.rs` `CONTEXT`, `src/app/menu.rs` `context_layout` and
  `src/app/semantic_ui.rs` `show_context_menu_at`.

## Packages (each in its own worktree and branch, merged to main in this order)

**A. Terminal right-click menu** (worktree `termurion-uia`, branch `ui-a`). Rebuild it from
`iTermTextViewContextMenuHelper.m`: every item, in iTerm2's order and sections, with iTerm2's
conditions, labels ("Split Pane Vertically", not the main menu's label) and no shortcut text
where iTerm2 shows none. Includes the New Window, New Tab and Select submenus, Copy Base64 and
the other selection items, Download with scp (Termurion's SSH file machinery; "Download using SSH
Integration" when that applies), Send Email to Selected Address (`mailto:`), Add Trigger…, Save,
Send Selection, Save Selection as Snippet, Annotate Selection and Reveal Annotation, Edit
Session…, Toggle Broadcasting Input, Lock Pane and the tab-wide lock items, Stop Coprocess, Close,
Restart, Bury, the Terminal State submenu, image and file items, the Replace with JSON/Base64
items. Also the timestamp margin menu ("Set Baseline for Relative Timestamps", "Disable Relative
Timestamps") and Copy on the workgroup branch item. Guard test: the context menu's item list
matches iTerm2's for a fixed set of states (no selection, selection, link, image, locked pane,
coprocess running, tmux pane).

**B. Main menu runtime items, transfers** (worktree `termurion-uib`, branch `ui-b`). The
Downloads and Uploads menus (Clear All; per transfer Stop, Show in Explorer, Remove from List,
Open, Get Info) over Termurion's existing OSC 1337 and SSH transfers, with iTerm2's transfer
alerts (abort download, Upload in Progress, where to save with Save to Downloads, tar failed,
exceeded declared size). Session > Triggers per-trigger check items. View > Current Tab Color.
Scripts menu Alt-alternate "Reveal %@". Companion status menu (only if Termurion has a companion
connection; else rule it). The unsaved-settings menu (rule it out if Termurion has no
load-from-URL settings mode; check first).

**S. Settings window** (worktree `termurion-uis`, branch `ui-s`). Every control in the settings
audit, and every label in iTerm2's exact wording, pane by pane against the xibs:
per-profile fonts with a real font picker (size, weight, the "recommended for this font" and
performance hints) and no lock on non-default profiles; Tags editor; Snippets Filter; Screen
popup; Title Components chooser; Enable Tall Tab Bar; the missing global controls (Memory usage
per session, tmux pause age limit, Allow sending of clipboard contents, New tabs open at end of
tab bar, GPU rendering if not ruled); the window restoration popup; controls moved to iTerm2's
pane; the Profiles list Other Actions… popup (Bulk Copy from Selected Profile… with its sheet,
Save Profile as JSON…, Save All Profiles as JSON…, Import JSON Profiles…); the Settings search
field and its results; the field "Reset to …" menu; the profile search field menu (Tags, Search
Syntax Help); Smart Selection editor (Load Defaults, Syntax Help, regex visualizer link, test
playground, Name column); Pointer Load Defaults; the Session "You may want a trigger to stop
buffering" note (iTerm2's warning sign drawn as Termurion's warning icon, as other warnings are). Every `List` or `Dict` row that still draws "N item(s)"
gets its editor or a ruling. Every "This editor arrives with its feature's milestone" tooltip is
gone at the end. Guard test: every schema row with a UI label matches iTerm2's xib label (a
generated table from the xibs, like `scripts/gen-menus.py`), and every row draws a control.

**W. Windows, bars and alerts** (worktree `termurion-uiw`, branch `ui-w`). Every gap in the
windows audit except the transfer alerts (package B): global search match-mode popup; command
info "Disable showing offscreen command"; print "Black and white" in the print flow; the shell
integration wizard (both paths, all pages); the crash reporter window at next launch for a new
dump (Send opens a new GitHub issue with the log, private paths removed; Discard deletes the
dump; Remind Me Later keeps it); the import status window; the Python runtime installer and
building-script windows; the What's New carousel (pages for features Termurion has; Companion
ruled R12); AI key window heading and charge notice; Split Panel (check the split-with-profile
flow first); the triggers editor items (Copy Trigger as URL to Clipboard, CPU Time histogram,
visualization toggle, per-event help, Exit Code popup, iTerm2's field names); Session Status
tool tooltips; AI chat orchestration notice; title bar lock tooltip and accessibility label;
status bar Show History and the RPC script launch prompt; annotation delete tooltip; Copy Mode
first-use notification; paste progress bar; title prompts "Set Tab Title" / "Set Window Title"
with their help text; the close alert disclosure; and every missing alert in the audit's last
group (resize, slow triggers, gnarly JSON, profile change scope, open N profiles, iterm2: URL,
paste choices, large paste, restore, horizontal scroll bar, empty image folder, API over SSH).
Guard test: a table of iTerm2 alert texts and Termurion's, one row per alert.

## Finish

Merge A, B, S, W to main in that order (resolve conflicts by keeping both sides' items), full
`scripts/xt test`, release build, install on the Windows test machine, push. Then FIXES item 5
moves to Done with the commits, the parity counts and `docs/ROADMAP.md` are updated, and the
worktrees and branches are removed.
