# Python API Authentication

Before a program can use the [Python API](python-api.md), Termurion has to decide whether to let it in. This page covers the two ways to authenticate (a cookie, or the allow-all file) and what you see when neither applies. Read [Python API Security](python-api-security.md) first for the bigger picture.

## The decision order

For each connection Termurion checks, in this order:

1. The program presented a valid cookie. It is allowed.
2. The allow-all file exists and is valid. It is allowed.
3. The program asked that no prompt be shown (it sent `x-iterm2-disable-auth-ui`) and has no valid cookie. It is refused.
4. Otherwise Termurion asks you with "Allow Python API Usage?".

## Cookies

A cookie is a random 32-character string that Termurion creates and gives to a program it starts. The program passes it back in the `ITERM2_COOKIE` environment variable. The `iterm2` Python package reads that variable and the `ITERM2_KEY` variable on its own, so a script needs no extra code.

Termurion hands out cookies in these cases:

- **Scripts you launch from the Scripts menu.** Each gets a cookie that works for one connection.
- **Open Python REPL.** The REPL window gets its own cookie.
- **Sessions, if you opt in.** With Settings > Advanced > Scripting > "Set ITERM2_COOKIE environment variable, allowing Python scripts to be launched without confirmation?" on, new sessions get a cookie in their environment. Any script you run from a shell in such a session connects without a prompt. The setting only affects sessions created after you change it, and it only works while Enable Python API is on.

Turning on the session cookie option asks you to confirm: "New sessions will contain an environment variable that allows scripts to run without confirmation."

Any program you run in such a shell inherits the cookie, so it can use the API. Leave the option off if that worries you.

## The allow-all option

Settings > General > Python API > Authentication method for Python API has two choices:

- **Require a cookie** (the default).
- **Allow all apps to connect.** Any program running as you, at the same elevation, may connect without a cookie.

Choosing Allow all apps shows "Disable per-app authentication?" and asks you to confirm. Termurion then writes a small file named `disable-automation-auth`. The `iterm2` Python package looks for this file at the path below, even on Windows, so Termurion uses the same path:

```
%USERPROFILE%\Library\Application Support\iTerm2\disable-automation-auth
```

The file holds the hex of its own path, a space, and a fixed identifier. Termurion trusts the file only if all of these are true:

- it is a plain file (not a link, junction, or folder),
- you own it, and
- its contents are exactly what Termurion would write.

If the file exists but fails the check, for example because your profile folder moved, Termurion falls back to requiring a cookie and shows "Python API Permissions Reset". Choose Require a cookie to delete the file at any time.

## When a program is not authenticated

A program with no cookie and no allow-all file gets the "Allow Python API Usage?" prompt, with the program's name when it gave one. Choose:

- **OK** to allow that one connection.
- **Cancel** to deny it.
- **More Info** to deny it and open [Python API Security](python-api-security.md).
- **Always**, shown only while the session cookie option is off. It turns that option on after a second confirmation.

If the session cookie option is already on and a program connects without a cookie, the prompt adds a line saying the script has not presented a valid cookie.

## Running your own script outside Termurion

Start the script from a Termurion session with the session cookie option on, from the Scripts menu, or after choosing Allow all apps to connect. A script you start from an ordinary terminal with none of those gets the approval prompt every time.
