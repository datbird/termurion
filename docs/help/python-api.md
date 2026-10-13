# Python API

Termurion speaks iTerm2's Python API. The API is a WebSocket protocol that lets a program control Termurion: it can list windows, tabs, and sessions, send text, read the screen, create tabs and panes, watch for events, and register new functions and status bar components. Because the protocol is the same, the `iterm2` Python package from PyPI works with Termurion, and so do most existing iTerm2 scripts.

## Turn it on

1. Open Settings (Ctrl+Shift+,) and choose General.
2. In the Python API section, turn on Enable Python API.

Termurion then listens on `127.0.0.1` and `[::1]`, port 1912. It accepts connections from this computer only. If you launch a script while the API is off, Termurion asks "Enable Python API?" first.

Read [Python API Security](python-api-security.md) before you turn it on. A script that connects can read and change anything in Termurion. [Python API Authentication](python-api-auth.md) explains how Termurion decides which programs may connect.

## Run a script

Scripts live in `%APPDATA%\Termurion\Scripts`. To use another folder, turn on Custom Python API Scripts Folder in the same settings section and choose a folder that exists and has no spaces in its path.

Termurion watches the folder. Scripts you add appear in the Scripts menu. Choose one to run it. Everything below is in the Scripts > Manage submenu:

- **New Python Script** makes a script from a template. You choose an environment (Basic, or Full Environment with its own packages) and a template (Simple, which runs and ends, or Long-Running Daemon, which stays running and reacts to events). Script names cannot contain spaces.
- **Open Python REPL** opens a window with an interactive Python prompt that is already allowed to talk to Termurion. It runs the standard `asyncio` REPL, so you can use `await` at the prompt.
- **Manage Dependencies...** lists scripts and the pip packages they need.
- **Install Python Runtime** and **Install Already-Downloaded Python Runtime** set up the Python that scripts use.
- **Reveal Scripts in Explorer** opens the Scripts folder.
- **Import...** and **Export...** move scripts as `.its` archives.
- **Console** (Alt+Shift+J) opens the Script Console.

Scripts you put in the `AutoLaunch` folder inside the Scripts folder start once when Termurion starts.

## The Python runtime

Termurion does not ship Python. When you first run a script, it asks before it downloads anything:

- `uv` from github.com/astral-sh/uv
- a Python interpreter from github.com/astral-sh/python-build-standalone
- the `iterm2` and `certifi` packages, plus any packages your script needs, from pypi.org

Nothing is downloaded without your OK. The files go under `%APPDATA%\Termurion\uv`. Basic scripts and the REPL share one environment per Python version. A Full Environment script gets its own `.venv` folder next to a `setup.cfg` that lists its dependencies. The default Python version is 3.12.

You can also run a script yourself with any Python that has the `iterm2` package. See [Python API Authentication](python-api-auth.md) for how it connects.

## Script Console

Scripts > Manage > Console shows each script, each connection, and an app-wide log. Select a row to see its log or the API calls it made. The buttons Inspector, Terminate, Restart, and Clear Terminated act on the selected script.

## What scripts can register

Scripts can register functions that Termurion calls, title providers, status bar components, and context menu items. See [Scripting Fundamentals](scripting-fundamentals.md) for how functions and [interpolated strings](scripting-fundamentals.md#interpolated-strings) work, [Variables](variables.md) for the data a script can read, and [Triggers](triggers.md) for calling a function from a trigger.

## Notes for iTerm2 users

- The WebSocket subprotocol is still `api.iterm2.com` because the Python package sends that name.
- The package looks for its allow-all authentication file under `~/Library/Application Support/iTerm2` even on Windows. Termurion uses that same path (see [Python API Authentication](python-api-auth.md)).
