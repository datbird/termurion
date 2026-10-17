# Password Manager

The password manager stores account names, user names, and passwords, and types them into a terminal session for you. Passwords stay out of your shell history, because Termurion sends them straight to the program that asked, with echo off. The window can keep passwords in Windows Credential Manager, or it can read them from LastPass, 1Password, KeePassXC, Bitwarden, or Keeper.

## Open it

Choose Window > Password Manager. With the default Shortcut Key (Ctrl+Shift), the shortcut is Shift+Alt+F.

You can have it open at the right moment too:

- A profile can open it by itself when the session reaches a password prompt. Turn on "Open Password Manager Automatically" in Settings > Profiles > Session.
- A trigger with the action "Open Password Manager..." opens it when a pattern appears, for example `[Pp]assword:`. See [Triggers](triggers.md).
- When a program turns echo off to ask for a password, Termurion shows a key icon at the cursor. Click it to open the password manager. The Advanced setting "Show key at cursor at password prompt?" controls this.

## Use it

1. Pick a row in the list. The list has the columns Account Name, User Name, and Password. Type in the search field to filter by account name.
2. Press one of the Enter buttons: Enter Password, Enter User Name, or Enter Username & Password. The text goes to the session that had focus when you opened the window. The window shows hints for the shortcuts, for example Ctrl+Shift+U for the user name with the default Shortcut Key.
3. Close the window.

A row also offers Copy Password, Reveal Password, and, for sources with one-time codes, Append OTP. A "2-Factor Code" field adds a code to the end of the password. The Broadcast check box (Ctrl+Shift+B) sends the password to several sessions at once. The echo probe decides which of them are at a password prompt.

The + and - buttons add and remove accounts. Edit changes the account name, user name, or password. The window warns "Are you sure you want to delete this password?" before it deletes.

## The gear menu

The gear menu at the bottom of the window holds:

- The password source (see below).
- Require Authentication to Open Password Manager, Require Authentication after Screen Locks, and Require Authentication on Every Open. These ask for Windows Hello or your Windows password before the window opens. The first has a matching setting in Settings > Advanced, so a failed prompt can never lock you out.
- Probe Before Sending Password: first checks that the session is really at a password prompt (it sends a space and a backspace and watches whether the terminal echoes them), and asks you if it is not sure.
- Automatically Send Return Key after Password.
- Switch Account (for sources that have more than one account).
- Settings... (for sources that need configuration).
- Reload Items.
- Reset Integration Configuration. This erases only Termurion's settings for the source. Your passwords are not touched.

## Password sources

| Source | What you need | Notes |
|---|---|---|
| Windows Credential Manager | Nothing. | The default. Termurion stores each account as a generic credential whose name starts with `Termurion/`. Only your Windows account can read them. |
| LastPass Integration | The LastPass command line client, `lpass.exe`. | See "LastPass" below. |
| 1Password Integration | The 1Password command line client, `op`, version 2.23 or later. | Termurion looks on PATH and in WinGet's link folder. If it is elsewhere, press Locate. The Advanced setting "1Password account name" chooses the account. |
| KeePassXC Integration | `keepassxc-cli` and a database file. | Termurion asks for the path to the database and for your master password. |
| Bitwarden Integration | The Bitwarden CLI, `bw`. | Termurion unlocks the vault with your master password and keeps the session key in memory only. |
| Keeper Security | Keeper Commander in Service Mode. | Enter the API URL, for example `http://127.0.0.1:8900/api/v2`. The "master password" is the service's API key. A Sync Down command is available in the gear menu. |

KeePassXC, Bitwarden, and Keeper work through small helper programs installed beside `termurion.exe`. Master passwords and API keys are held in memory and passed to the tools without going on a command line.

## LastPass

Termurion talks to LastPass through its command line client, `lpass`. Termurion cannot guess where `lpass.exe` is on Windows, because it has no standard install location. The first time you choose LastPass, Termurion shows "Can't Find LastPass CLI":

> In order to use the LastPass integration, Termurion needs to know where to find the CLI app named "lpass". Select Locate to provide its location.

The buttons are:

- Locate: pick `lpass.exe`. Termurion accepts only a local file named `lpass`. It remembers the path.
- Cancel.
- Help: opens this page.

If you do not have the client, install a Windows build of it first, then press Locate. Check that `lpass login you@example.com` works in a terminal.

If the sign in fails, Termurion shows "Authentication Failed" with the hint to run `lpass login` and your email address, and three buttons: Open Terminal Window, Copy Command, and Cancel. Open Terminal Window starts a session where you can sign in. After you sign in once, reload the items.

Termurion lists accounts from your LastPass groups. The default group is named `iTerm2`. To use other groups, set "Comma-separated list of LastPass groups for the password manager to look in for passwords" in Settings > Advanced. Use names made only of letters, digits, and spaces. A name with any other character is ignored.

LastPass can time out and ask for the master password again. Termurion asks you once and retries.

## In the web browser

The browser has its own password manager choice and its own list of accounts. They are kept apart from the terminal's. See [Web Browser](web-browser.md).

## Safety notes

- Passwords are held in memory only while needed, then cleared.
- Do not put a password in a trigger, profile, or script. Use this window instead.
- Show key at cursor only appears for a real password prompt (echo off, line input on) after a short delay. The Advanced setting "How long (seconds) terminal echo must remain disabled before Termurion treats it as a password prompt" sets the delay.

## Related pages

- [Triggers](triggers.md)
- [Web Browser](web-browser.md)
- [SSH Integration](ssh-integration.md)
