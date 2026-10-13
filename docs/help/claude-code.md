# Claude Code Integration

Termurion can work with Claude Code, Anthropic's coding agent that runs in a terminal. After you set it up, each tab that runs `claude` shows its state (working, waiting for you, or idle), the Session Status tool lists every Claude session, and a Claude Code workgroup puts the agent, a diff viewer, and a code-review session in one pane that you can switch between with a click.

All of this is optional and undoable.

## Set it up

Choose Termurion > Install Claude Code Integration. A setup window called "Claude Code Integration Setup" first lists what it will change, under "Before You Start":

- Termurion's Python API is enabled.
- A cc-status hook is added to Claude Code's `settings.json`.
- The Toolbelt is shown.
- A Claude Code workgroup is added to Termurion's settings.
- Enter and Exit Workgroup triggers are added to the profiles you pick.

You then go through the steps with Back, Next, and Do It buttons:

| Step | What it does |
|---|---|
| Enable Python API | Turns on Termurion's Python API so Termurion can find the sessions that run Claude and track their status. Setup cannot continue without it. |
| Enable Session Cookies | Gives each new session a secret cookie. The hook runs in a new process and needs the cookie to prove it belongs to a Termurion session. This is a separate question. Setup cannot continue without it. |
| Install Hook | Adds a cc-status hook to your Claude Code settings. It runs while Claude works. |
| Show Toolbelt | Shows the Toolbelt and enables the Session Status tool. |
| Install Workgroup | Adds the Claude Code workgroup. See [Workgroups](workgroups.md). |
| Auto-Enter Workgroup | You choose the profiles where you run `claude`. Termurion adds triggers so the workgroup starts when `claude` starts and ends when it stops. |

Cookies let any program in a session use the Python API without a prompt. Read the question before you accept it. See [Python API Security](python-api-security.md).

To change or remove the setup later, use Termurion > Reinstall Claude Code Integration or Termurion > Uninstall Claude Code Integration. Uninstall removes the hook, the workgroup, and the triggers. It asks "Uninstall Claude Code Integration?" first, and it asks whether to leave the Python API on or turn it off.

If you have not set it up, Termurion may ask "Want to try Termurion's Claude Code integration?" once you have started Claude in three sessions. The buttons are Yes, Never, and Ask Later.

## The hook

The hook is `cc-status.exe`, installed beside `termurion.exe`. Claude Code runs it on each of these events: UserPromptSubmit, Stop, StopFailure, Notification, PermissionRequest, SessionEnd, PreToolUse, PostToolUse, SessionStart, and SubagentStop. Termurion keeps its own entries in `%USERPROFILE%\.claude\settings.json`. It leaves the rest of the file alone, keeps a backup, and writes the file by replacing it whole. If the file cannot be read or parsed, Termurion does not write to it.

The hook reads one event from standard input and reports a status to Termurion over the Python API. It maps events like this:

- Working: a prompt was submitted, or a tool is running. A task running in the background keeps the state Working.
- Waiting: Claude asks for permission, or sends a notification that needs you.
- Idle: Claude has stopped, with no background tasks left. The last message of Claude's reply is shown as the detail.

The hook never blocks Claude. Any failure makes it quit quietly.

## Session Status tool

The Session Status tool on the Toolbelt lists sessions with their state and detail. The same state shows on the tab. Open the Toolbelt with View > Toolbelt > Show Toolbelt (Ctrl+Shift+Alt+B by default).

## Health check

Once per launch Termurion checks that its hook is still in `settings.json`. Claude Code sometimes rewrites that file and drops it. If the hook is gone, Termurion asks "Claude Code Integration Looks Broken" and offers to Reinstall it, or Not Now. You can silence the prompt.

## Workgroup and code review

The Claude Code workgroup has the main Claude session and two peers: a diff viewer and a code-review session. In the code-review mode you write a prompt, press Start, and the review session starts with that prompt. Saved prompts are in the Prompts pulldown, with "Save Current as New..." and "Manage Prompts...". See [Workgroups](workgroups.md).

## Related pages

- [Workgroups](workgroups.md)
- [Python API](python-api.md)
- [Python API Security](python-api-security.md)
- [Triggers](triggers.md)
