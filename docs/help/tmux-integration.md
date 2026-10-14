# tmux Integration

tmux integration lets Termurion show a tmux session as native tabs, windows, and panes. You use tmux for its persistent sessions, and Termurion for its scrollback, mouse, copy, and search. Each tmux window becomes a Termurion tab. Each tmux pane becomes a Termurion pane. When you close the Termurion window, tmux keeps the session alive. When you attach again, the layout comes back.

This works through tmux control mode. You do not need to learn tmux keys to use it.

## Choosing a tmux

Termurion works with tmux control mode on Windows three ways.

| Way | Use it when | Install |
|---|---|---|
| psmux | You want native Windows tmux, with no WSL. Recommended for this. | `winget install psmux` (https://github.com/psmux/psmux) |
| tmux in WSL | You already work in a WSL distribution. | `sudo apt install tmux` in the distribution |
| tmux over ssh | The tmux server runs on another host. | tmux on that host |

Native Windows support is tested against psmux 3.3.8. WSL support is tested against tmux 3.4. See [Tested with](#tested-with).

### Start from the menu

- Shell > tmux > New psmux Session opens a new tab that runs `psmux.exe -CC new-session -A -s termurion`.
- Shell > tmux > New tmux Session in WSL opens a new tab that runs `wsl.exe -e tmux -CC new-session -A -s termurion`.

Both attach to the session named `termurion`, or make it if it does not exist. If the program is missing, an info bar tells you the command that installs it (`winget install psmux`, or `sudo apt install tmux` in WSL) and no tab opens. If WSL itself is missing, the bar says `wsl --install`.

### Start by hand

In any Termurion tab, run the command for your way:

```
psmux -CC new-session -A -s main
wsl.exe -e tmux -CC new-session -A -s main
ssh -t host tmux -CC new -A -s main
```

You can also run `tmux -CC` or `tmux -CC attach` in a shell that already has tmux, as described next.

## Start

In a shell that runs tmux (usually WSL, or an ssh session to a host that has tmux), run:

```
tmux -CC
```

This starts a new session. To attach to one that already exists:

```
tmux -CC attach
```

For a named session, use `tmux -CC attach -t name`. Termurion sees the control mode start, and the shell tab is replaced by the tmux windows.

The first tab, the one where you typed the command, stays as the gateway. It is the session that carries the tmux protocol. If you set "Automatically bury the tmux client session after connecting" (Settings > Advanced), it is hidden and appears under Session > Buried Sessions.

## Requirements

- tmux on the host. Termurion detects the version when it connects. Features that need a newer tmux say so below.
- The ConPTY that ships with Termurion (the `resources\conpty` folder, `conpty.dll` and `OpenConsole.exe`). The ConPTY that comes with Windows drops the start sequence of control mode (`DCS 1000p`), so tmux integration cannot start through it. Termurion loads the shipped ConPTY itself. If you replace or remove that folder, tmux integration stops working.

## psmux differences

psmux is not tmux, and its control mode does less. Termurion detects psmux when it connects and turns off what psmux cannot do. What you see:

- Move Session to Split Pane shows a banner and does nothing. psmux has no `move-pane` or `join-pane` in control mode.
- You cannot reorder tabs by dragging. psmux has no `swap-window` in control mode, so tabs keep their order.
- In the Dashboard, the + button (new session) and Attach are off. psmux has no `new-session` or `attach-session` in control mode. The Dashboard lists the attached session only.
- User variables are not saved to tmux. psmux refuses pane options that start with `@`.
- Detach drops the client and leaves the session running in the psmux server, because psmux's client does not leave until it reads more input.
- If the attached session is killed, Termurion ends the connection itself, because psmux sends no exit notice.
- Keys sent to a pane may log a "command timed out" error from psmux. The keys still arrive, and Termurion ignores the error.
- psmux starts a session at 120x30. Termurion resizes it to the tab size when it connects.
- psmux reports version 3.3.8, which Termurion reads as tmux 3.3. Pause mode and per-window sizes are on.

Everything else the live suite checks (attach, sizing, new window, split, resize, layouts, rename, send keys, pause, kill pane, detach) passes on psmux. The full list is in `docs/probe/2026-10-14-psmux-wsl-tmux-results.md`.

## Windows, tabs, and panes

- A tmux window opens as a tab. New tmux windows made outside Termurion open where the setting "Where should new tmux windows not created by Termurion open?" says: a new window, the focused window (the default), or the topmost window that already shows a tab from that session.
- Splits and layouts match tmux.
- Titles start with an arrow marker to show a tmux tab. Change it with the Advanced setting "Insert this string at the start of tab and window titles to indicate tmux integration". The tmux client name can also show in brackets in the window title (another setting).
- Panes use a profile named "tmux", made from your default profile the first time. Turn this off with "Use \"tmux\" profile rather than profile of the connecting session".
- With variable window sizes (tmux 2.9 or later) each window can have its own size. The Advanced settings "Allow variable window sizes in tmux integration" and "Remember window sizes in tmux integration?" control this.

## Menu: Shell > tmux

| Item | What it does |
|---|---|
| Detach | Leaves the session running in tmux and disconnects Termurion from it. |
| Force Detach | Drops the connection without asking tmux. Use it when tmux does not answer. |
| New Tmux Window | Makes a new tmux window as a Termurion window. |
| New Tmux Tab | Makes a new tmux window as a tab. |
| Pause Pane | Pauses output for the current pane (tmux 3.2 or later). |
| Dashboard | Opens the tmux Dashboard (Shift+Ctrl+Win+Return). |

Detach, New Tmux Window, New Tmux Tab, and Pause Pane have shortcuts with the Shortcut Key plus Ctrl+Shift. The menu shows the exact keys for your setup.

New Tab and New Window ask what you want while a tmux pane is current: a new tmux window, a window or tab with your default profile, or Cancel. Two Advanced settings let you answer once and not be asked again.

Closing a tmux tab or window asks whether to kill it or hide it. The Advanced settings "Suppress kill/hide dialog when closing a tmux tab" and "...window" silence the question.

## Dashboard

Shell > tmux > Dashboard lists the sessions and windows of a tmux connection. A check mark marks a window that is open in Termurion. Use it to:

- Attach to or detach from a session. The + and - buttons add and remove sessions.
- Open a window in tabs, or hide it.
- Search window names with the "Search Windows" field.

If you are connected to more than one tmux server, pick the connection at the top.

## Pause mode

With tmux 3.2 or later, tmux pauses a pane whose output a client is too slow to read. Termurion warns you before this happens. "Suppress announcement that tmux will pause a session" silences that warning. A paused pane shows a bar. Use Shell > tmux > Pause Pane to pause a pane yourself.

## Clipboard

"Mirror tmux paste buffer to local clipboard" (Advanced) copies tmux's paste buffer to the Windows clipboard whenever it changes. Termurion asks the first time.

## Slow tmux response

If tmux does not answer its first command within five seconds, Termurion shows "Slow tmux Response". On an old host, tmux may expect a newline rather than a carriage return to end commands. Change this in Settings > Profiles > Terminal with "tmux uses newline (vs CRLF)", using the Reveal Setting button on the bar. "Suppress warning about a slow tmux response" silences it.

If tmux stops answering after you connect, Termurion asks "Force Detach? Tmux is not responding" with buttons Detach and Cancel.

## tmux 2.2

tmux 2.2 has a bug that affects Termurion. It cannot take a character outside the Basic Multilingual Plane, for example most emoji, when the character is sent as a key. If you type or paste such a character into a pane of a tmux 2.2 server, Termurion cannot send it. Instead you see a bar that says "Because of a bug in tmux 2.2, the character (the character itself) cannot be sent." with a Why? button that opens this section.

Only tmux 2.2 has the problem. Termurion works out the server's version when it connects. Older and newer versions are not affected. To fix it, upgrade tmux on the host (to 2.3 or later) or avoid those characters. Other characters, including accented letters and CJK text, work in 2.2.

## Settings in this area

All of these are in Settings (Ctrl+Shift+,), mostly under Advanced and General > tmux:

- "Open tmux windows and tabs in the background?"
- "Close tmux windows after detaching?" (works when the profile's "After a session ends" is "No Action")
- "Disable window position restoration in tmux integration."
- "Convert italics to reverse video in tmux integration?"
- "Status bar shows tmux status bar content, not native components" (see [Status Bar](status-bar.md))
- "Poll for foreground job name in tmux integration with tmux < 3.2?"

## Tested with

- Native Windows: psmux 3.3.8 (from winget).
- WSL: tmux 3.4.

Termurion's live suite starts a real server of each kind, attaches through the same code the app uses, and checks the features above. It uses its own server names and never touches your default servers. The results are in `docs/probe/2026-10-14-psmux-wsl-tmux-results.md`.

## Related pages

- [Shell Integration](shell-integration.md)
- [SSH Integration](ssh-integration.md)
- [Status Bar](status-bar.md)
