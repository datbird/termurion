# Variables

Termurion keeps named values about each session, tab, window, and the app. You can use them in [interpolated strings](scripting-fundamentals.md#interpolated-strings) (badges, titles, status bar, triggers), read them from [Python API](python-api.md) scripts, and set your own. This page lists the variables Termurion provides.

## Scopes

Variables belong to one of four scopes. In a session's interpolated string, you reach all four:

| You write | Meaning |
|---|---|
| `hostname` or `session.hostname` | A variable of this session |
| `user.name` | A user variable of this session |
| `tab.title` | A variable of this session's tab |
| `tab.window.title` | A variable of the window that holds the tab |
| `iterm2.pid` | A variable of the app |

A name without a dot is tried in the session scope. In a window's title, session variables are reached as `currentTab.currentSession.<name>`.

## Session variables

| Name | Value |
|---|---|
| `name`, `presentationName` | The session's title |
| `id` | The session's ID |
| `termid` | A name like `w0t1p0.<id>`: window, tab, and pane numbers, then the session ID |
| `profileName` | Name of the session's profile |
| `columns`, `rows` | Size of the screen |
| `showingAlternateScreen` | 1 when a full-screen program is using the alternate screen |
| `path` | Current folder, when shell integration (or tmux) reports it |
| `hostname`, `username` | Host and user reported by [shell integration](shell-integration.md) |
| `isLocalhost` | 1 when the session is on this computer |
| `jobName`, `deepestJob` | Name of the foreground program |
| `processTitle`, `commandLine`, `jobPid` | Details of the foreground program |
| `foregroundJobAncestors` | The foreground program's parents, one per line |
| `pid`, `effective_root_pid` | Process ID of the session's shell |
| `lastCommand` | The last command shell integration saw |
| `selection`, `selectionLength` | The selected text and its length |
| `creationTimeString` | When the session was created |
| `autoLogId`, `logFilename` | Used by session logging |
| `tmuxPaneTitle`, `tmuxStatusLeft`, `tmuxStatusRight` | Set for tmux panes |
| `sshIntegrationLevel` | 0 for a local shell, 2 when [SSH integration](ssh-integration.md) is active |
| `homeDirectory`, `shell`, `uname` | Home folder, shell, and system of the local computer or the SSH host |
| `gitBase` | Set on a workgroup leader |

Some of these exist only when the thing they describe exists. For example `path` is missing until shell integration reports a folder. The `selection` text is cut off at the limit in Settings > Advanced ("Maximum number of bytes of selection to provide to Python API"), and `selectionLength` is the full length.

Numbers such as `columns`, `rows`, `pid`, and `jobPid` are numbers, not text, so you can use them in arithmetic.

## Tab variables

Use these as `tab.<name>` from a session.

- `id`
- `title`: the tab's title as shown
- `titleOverride`: the text you set with Edit Tab Title, evaluated
- `titleOverrideFormat`: that text before evaluation
- `tmuxWindow`: the tmux window number, or -1
- `window`: the ID of the window that holds the tab

## Window variables

Use these as `tab.window.<name>` from a session.

- `id`
- `number`: the window number, counting from 1
- `title`
- `titleOverride` and `titleOverrideFormat`
- `isHotkeyWindow`: 1 for a hotkey window

## App variables

Use these as `iterm2.<name>`.

- `pid`: Termurion's process ID
- `localhostName`: this computer's name
- `effectiveTheme`: `light` or `dark`

## User variables

Names that start with `user.` are yours. They can be set in three ways:

- **A program in the session** sends the escape sequence `ESC ] 1337 ; SetUserVar=<name>=<base64 value> BEL`. The shell integration scripts for bash, zsh, fish, and xonsh define an `iterm2_set_user_var` function that sends it, for example `iterm2_set_user_var gitBranch main`.
- **A Python API script** sets them with `async_set_variable`. A value a script sets wins over one sent by escape sequence until the script sets it to null.
- **A trigger** can set one with the Set User Variable action. See [Triggers](triggers.md).

User variables exist on sessions, tabs, windows, and the app. Only names of the form `user.<name>` (one word after `user.`) can be set through the API.

## Seeing the values

Open Scripts > Manage > Console, then press Inspector. The Scripting Inspector shows your sessions, tabs, and windows as an outline and lists the variables of the one you select. The row menu has Copy ID, Copy Value, and Copy Path.

## Related

- [Scripting Fundamentals](scripting-fundamentals.md)
- [Python API](python-api.md)
