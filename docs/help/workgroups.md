# Workgroups

A workgroup is a saved set of related sessions that you start with one command. It has a main session and other members that belong to it: peers that swap into the same pane, splits next to it, or tabs. Each member has a profile, a command, and a toolbar. Use a workgroup for a task that always needs the same layout, such as an editor with a diff and a test runner, or a coding agent with a diff viewer and a code reviewer.

## Enter and exit

- Enter: choose Shell > Workgroups and pick a workgroup from the list. The menu works from a session that is not already in a workgroup.
- Exit: choose Shell > Workgroups > Exit Workgroup, from the main session. A workgroup's members close when it exits.
- A trigger can enter and exit automatically. The trigger actions are "Enter Workgroup..." and "Exit Workgroup". See [Triggers](triggers.md).

Only a session that is not in a workgroup can enter one. To switch to another workgroup, exit first. A tmux session cannot be the main session of a workgroup. A workgroup cannot start in a replay or an archive, or in a window whose layout is locked.

Closing a session that is the main session of a workgroup warns you: "This will also exit the workgroup and close N other sessions."

Each member's environment has the variable `ITERM_WORKGROUP_ID` set to an id that begins with `wg-`.

## Member types

| Type | What it is |
|---|---|
| Root | The main session. Every workgroup has exactly one. |
| Peer | Takes over the root's pane. Only one peer is shown at a time. Switch with the Mode Switcher in the toolbar, with a shortcut you set, or with Window > Split Pane > Select Split Pane > Previous Peer and Next Peer. |
| Split | A new pane beside its parent. You set the direction (Vertical or Horizontal), the side (Left, Right, Top, Bottom), and the share of the parent it takes. |
| Tab | A new tab. |

A workgroup can have up to 32 members. Each member has a profile, a command, a display name, and a mode. A member can also show a web page by URL.

## Modes

A member's mode is Regular, Code Review, or Diff.

- Diff shows changed files. It uses git commands with `\(gitBase)` for the base to compare against and `\(file)` for one chosen file, for example `git diff \(gitBase) -- \(file)`.
- Code Review starts a review session with a saved prompt. See [Claude Code](claude-code.md).

## Toolbar items

Each member can have a toolbar strip with these items:

- Git Status
- Changed File Selector
- Mode Switcher (the peer switcher)
- Navigation (Back and Forward through changed files)
- Reload
- Spacer
- Git Base Selector
- Auto-send clippings when idle
- Auto-request review when idle
- Usage (shows Claude usage; with no command of its own it runs `claude -p /usage`)

## Edit workgroups

Open Settings with Ctrl+Shift+, and choose Arrangements > Workgroups. The editor lists your workgroups. You can:

- Add a new workgroup, or add a preset: "Coding Agent + Diff" or "Coding Agent + Diff + Code Review".
- Add sessions and set their profile, command, side, and toolbar items.
- Set a shortcut for each peer, and shortcuts for Back, Forward, and Reload.
- Undo and Redo your edits.

Commands you type in the editor are only stored. They do not run until you enter the workgroup.

## Trust

A workgroup's commands run programs, so Termurion is careful.

- A workgroup you made yourself, entered from the menu, with 8 or fewer members, starts without a question.
- A workgroup entered by a trigger, or one with more than 8 members, asks first. The question lists what will run, and the buttons are Cancel and Enter. Cancel is the default.
- A workgroup that came from an import, a remote settings folder, or a Dynamic Profile is marked "imported". Termurion asks before it runs its commands. To start it without asking, read its commands in Settings > Arrangements > Workgroups and press "Trust this workgroup". See [Dynamic Profiles](dynamic-profiles.md).

If the commands are too long to show in full, Termurion does not start the workgroup. Read them in the editor and trust it there.

## Restoring

When you restore a window arrangement, Termurion can bring back a workgroup. It restores the layout and does not run the member programs again.

## Related pages

- [Claude Code](claude-code.md)
- [Triggers](triggers.md)
- [Dynamic Profiles](dynamic-profiles.md)
