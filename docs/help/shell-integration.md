# Shell Integration

Shell integration is a small script that your shell runs. It tells Termurion where each prompt starts, where each command starts and ends, which exit status it had, and which folder, user, and host you are in. With that, Termurion can treat a command and its output as one unit instead of a pile of text.

## What it gives you

Once integration is active in a session, these work:

- Marks in the left margin at each prompt (Settings > Profiles > Terminal > Shell Integration > Show Mark Indicators).
- Edit > Select Output of Last Command (Ctrl+Shift+Alt+A) and Edit > Select Current Command.
- Edit > Marks and Annotations > Next Mark and Previous Mark (Ctrl+Shift+Alt+Down and Ctrl+Shift+Alt+Up).
- Edit > Clear to Last Mark (Alt+Shift+L).
- Session > Open Command History... (Ctrl+Shift+Alt+;) and Session > Open Recent Directories... (Alt+Shift+/).
- A floating bar that shows the current prompt when it has scrolled off the top (Show Offscreen Command Line, same settings section).
- The current folder, user, and host, which profile rules and other features use.
- Warnings that only apply at a prompt, such as the multi-line paste warning. See [Paste Bracketing](paste-bracketing.md) for how paste is sent.
- Capture Output, which warns that it needs shell integration. See [Captured Output](captured-output.md).

A session without integration still works. These features are simply unavailable or less precise.

Termurion uses the shortcut key Ctrl+Shift where iTerm2 uses Cmd, and Alt where iTerm2 uses Option. You can change the shortcut key in Settings > Keys. The key names above assume the default.

## PowerShell

PowerShell (Windows PowerShell 5.1 and PowerShell 7) is the shell Termurion supports directly. The script is `termurion_shell_integration.ps1`. It wraps your `prompt` function and sends these sequences:

- OSC 133 `A`, `B`, `C`, and `D` for the prompt start, prompt end, command start, and command end with the exit status.
- OSC 7 with your user name, computer name, and current folder.
- OSC 1337 `ShellIntegrationVersion`, which tells Termurion the shell is `powershell` or `pwsh`.

It keeps your own prompt. Tools such as starship and oh-my-posh still draw as before. It also starts the prompt on a fresh line, as the Place Prompt at First Column setting does in iTerm2.

### Install from the menu

1. Open a tab that runs PowerShell.
2. Choose Termurion > Install Shell Integration.
3. Read the question and choose Yes.

Termurion saves the script to `%APPDATA%\Termurion\shell_integration\termurion_shell_integration.ps1`. It then types one command into your session. That command:

- Sets the execution policy to RemoteSigned for your user only, and only if it is Restricted or AllSigned.
- Creates your `$PROFILE` file if it does not exist.
- Adds one line to `$PROFILE` that loads the script, unless the line is already there.
- Loads the script in the current session.

New PowerShell sessions then load it on their own.

### Load it automatically instead

Turn on Settings > Profiles > General > Load shell integration automatically. Termurion then starts PowerShell with `-NoExit -Command` and a one-line command that reads the script, so the script runs after your `$PROFILE`. The script is read as text, so the execution policy is not involved and is not changed.

Termurion leaves a command line alone if it already runs something or cannot take a second command. That includes `-Command`, `-File`, `-EncodedCommand`, `-NoExit`, or a script name on the command line.

## bash, zsh, and fish

Inside WSL, Git Bash, or an ssh session, choose Termurion > Install Shell Integration. Termurion asks whether to also install the utilities (scripts to download files, show images inline, and more). Then it types one of these into your session:

```
curl -L https://raw.githubusercontent.com/datbird/termurion/main/dist/install_shell_integration.sh | bash
curl -L https://raw.githubusercontent.com/datbird/termurion/main/dist/install_shell_integration_and_utilities.sh | bash
```

The first installs only the integration. The second installs the integration and the utilities. The installer adds a line to your shell's startup file. Review the script before you pipe it into `bash` if you want to know exactly what it changes.

The scripts for bash, zsh, fish, and xonsh are in the repository under `resources/ssh/shell_integration`. For hosts you reach over ssh, see [SSH Integration](ssh-integration.md), which can load them for you.

## Other shells

cmd.exe has no prompt hooks, so it cannot have shell integration. Termurion shows a message saying so if you try to install it. Use PowerShell, or run bash, zsh, or fish in WSL.

## Troubleshooting

- Nothing changes after install: open a new tab, or run `. $PROFILE` in PowerShell.
- Marks do not appear: check Show Mark Indicators in Settings > Profiles > Terminal.
- Folder tracking is off: check that "Accept OSC 7 to set username, hostname, and path" is on in Settings > Advanced > Terminal. Turning it off also turns off folder, host, and user tracking.
- If you turn on the setting that disables potentially insecure escape sequences (Settings > Advanced), some shell integration features stop working.
