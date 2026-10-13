# SSH Integration

SSH integration makes an ssh session more than a plain terminal. When you connect through a Termurion SSH profile, a small helper runs on the remote host and talks to Termurion over the same connection. This lets Termurion:

- Install shell integration on the remote host for you, with no manual setup there.
- Know the remote host, user, and current directory.
- Download files with a file browser (Shell > ssh > Download Files...).
- Upload files by dropping them on the session.
- Reconnect through the same chain of hops when you duplicate a session or split a pane.

Termurion runs your normal Windows OpenSSH client (`ssh.exe`) for the connection. It connects the client through its shipped ConPTY so the protocol passes through unchanged.

## Make an SSH profile

1. Open Settings with Ctrl+Shift+, and go to Profiles > General.
2. Set "Command type" to SSH.
3. In the command field, type what you would type after `ssh`, for example `user@host` or `-p 2222 user@host`.
4. Press Configure... to open the Configure SSH sheet.

The Configure SSH sheet has these settings:

| Setting | Meaning |
|---|---|
| SSH Integration | On by default. With it off, the profile runs plain `ssh`. |
| Environment Variables to Copy to Remote Host | Names of local variables to set on the remote side. |
| Files to Copy to Remote Host | Files or folders to copy, each with a destination folder on the remote host. Add them with Files... or Folder.... |
| Path to SSH | The ssh program to run. The placeholder is `C:\Windows\System32\OpenSSH\ssh.exe`. Browse picks a file and checks that it is executable. |

If the command is empty, or SSH Integration is off, the profile runs plain `ssh`. A profile's Working Directory is used on the remote host only if it is a POSIX path (starting with `/` or `~`).

## it2ssh

`it2ssh` is a command in Termurion's utilities folder, which is on the PATH of every new session. It does the same thing as an SSH profile, but from a prompt:

```
it2ssh user@host
```

It needs a session started from a Termurion SSH profile. In any other session, `it2ssh` explains this and runs plain `ssh`.

## Remote requirements

The helper is a Python 3 script that Termurion sends to the host when you connect. The host needs a POSIX shell and `python3`. Termurion puts its shell integration files under `~/.iterm2/shell-integration` on the remote host. See [Shell Integration](shell-integration.md).

## Shell > ssh

| Item | What it does |
|---|---|
| Disconnect | Ends the SSH integration connection of the focused session. |
| Download Files... | Opens a file browser for connected hosts (see below). Enabled while any host is connected. |
| Remote host can control Termurion | Not available. Termurion does not include the `it2` command tree it would give to a remote host. |

## Download files

Shell > ssh > Download Files... opens a window with a list of connected hosts, Back and Forward buttons, a path field, a search field, and a file table. Choose files or folders and press Open.

Each chosen item downloads into a new folder under `%TEMP%` named `Termurion-` followed by a unique id. When the downloads finish, Termurion shows the files together. A folder arrives as a zip file that Termurion unpacks. A downloaded file carries the Windows Mark of the Web, and Termurion never runs it.

The "Use System Panel" button in the window uses the standard Windows file dialog instead.

## Upload files

Hold Alt while you drop files onto an SSH session. Termurion asks "OK to copy ... to ...?" and uploads them to the remote directory the shell is in. Without Alt, the file paths are pasted as text instead. Files on a network path are refused.

## Reconnecting

If the connection drops, or you use Duplicate Session or split a pane, Termurion starts the new session through the same hops. It only accepts user names, host names, and ports it can verify as safe. A hop that fails the check is not used.

## ssh: links

A link that starts with `ssh:` opens an SSH session. Termurion checks each part of the link (user, host, port) before it builds the command line, because a link comes from outside Termurion.

## Requirements and limits

- SSH integration uses the shipped ConPTY in passthrough mode. If you remove or replace the `resources\conpty` folder, integration cannot work.
- Termurion does not use scp for dropped files. If the helper cannot take the transfer, the paths are pasted.

## Related pages

- [Shell Integration](shell-integration.md)
- [tmux Integration](tmux-integration.md)
- [Password Manager](password-manager.md)
