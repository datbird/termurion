# Python API Security

The Python API gives a program full control of Termurion. A script that connects can read everything on screen, read and change profiles and settings, open and close sessions, and type into any shell as you. Treat a script that uses the API the same way you treat any program you run as yourself. Run only scripts you trust.

This page explains what Termurion does to keep other programs out. [Python API Authentication](python-api-auth.md) covers the cookie and the allow-all option in detail.

## The API is off by default

The server does not run until you turn on Settings > General > Python API > Enable Python API. When you turn it on, Termurion shows "Enable Python API?" with the warning that the API allows scripts you run to control Termurion and access all its data. Turning it off disconnects every script.

## Local connections only

The server listens on `127.0.0.1` and `[::1]`, port 1912, and on no other address. A web page cannot reach it either: Termurion refuses a connection whose `Origin` header does not name `localhost`. It also requires the `api.iterm2.com` WebSocket subprotocol and a library version header, which the `iterm2` Python package sends.

## Same user, same level

For every connection, Termurion finds which process owns the other end of the socket. It refuses the connection unless that process:

- runs as the same Windows user,
- has the same elevation (an administrator process cannot connect to a normal Termurion, or the reverse),
- has the same integrity level,
- is not in an app container, and
- does not use a restricted token.

This keeps a less trusted process, such as a sandboxed program, from typing into your shells.

## You approve each program

A program that connects must either present a valid cookie or be approved by you. When Termurion cannot tell, it shows "Allow Python API Usage?" and names the program. OK lets that connection through once. Cancel refuses it. More Info refuses it and opens this page. Always appears only when the session cookie option is off (see [Python API Authentication](python-api-auth.md)).

Scripts you start from the Scripts menu are launched by you, so Termurion treats them as approved. The Script Console logs the reason for every decision, for example "Script launched by user action" or "Denied by user".

## Limits

To make flooding harder, Termurion allows at most 32 open API connections and at most 8 that have not been approved yet. A single message or frame may be at most 16 MiB.

## If a prompt appears that you did not expect

An "Allow Python API Usage?" prompt means some program on this computer, running as you, tried to connect. If you were not running a script, choose Cancel and find out which program it was. The prompt names the program when it gave a name. The Script Console (Scripts > Manage > Console) shows the connection and the reason it was allowed or denied.

## Practical advice

- Leave the API off unless you use it.
- Keep the authentication method at "Require a cookie" unless you have a reason to change it.
- Do not turn on "Set ITERM2_COOKIE environment variable" if other programs run in your shells that you do not trust, because they would inherit the cookie.
- Review a script before you run it, and watch the Script Console while it runs.
