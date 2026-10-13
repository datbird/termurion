# Status Bar

The status bar is a strip of live information on a session: the current directory, the git branch, CPU use, a clock, and more. Each piece is a component. You choose the components and their order for each profile.

## Turn it on

1. Open Settings with Ctrl+Shift+, and go to Profiles > Session.
2. Under Status Bar, turn on "Status bar enabled".
3. Press "Configure Status Bar". The button is enabled once the status bar is on.

In the setup window:

- The component menu shows every component. Drag one into Active Components to add it.
- Drag within Active Components to reorder. Drag a component out, or select it and press Delete, to remove it.
- Select an active component and press "Configure Component" to open a sheet with a control for each of its options. The button is disabled for a component with no options.
- Auto-Rainbow colors the components automatically: Disabled, Light Colors, Dark Colors, or Automatic.
- Advanced holds the bar's colors, its font, and three check boxes (see below).
- Changes show behind the window as you make them. Cancel puts back the layout the window opened with. OK keeps it.

## Components

| Component | What it shows |
|---|---|
| Battery Level | Battery level and its recent history. |
| CPU Utilization | Current CPU use, with a small graph. |
| Memory Utilization | Current memory use, with a small graph. |
| Network Throughput | Download and upload rates. |
| git state | A summary of the git state of the current directory. |
| Clock | The date and time, in a format you set. |
| Custom Action | A button that performs an action, like a key binding. |
| Actions Menu | A menu of actions you click to run. |
| Snippets Menu | A menu of saved snippets to paste. |
| Triggers Menu | A menu to turn triggers on and off. |
| Host Name | The current host name. Needs shell integration. |
| User Name | The current user name. Needs shell integration. |
| Job Name | The running job, with parent process names if there is room. |
| Current Directory | The current directory. Best with shell integration. |
| Search Tool | A field to search the terminal. |
| Filter Tool | A field to hide lines that do not match. |
| Composer | A text field for composing a command line. |
| Interpolated String | The value of a string with inline expressions, such as session variables or script functions. |
| Call Script Function | A button that calls a script function you name. |

Also available are spacers: Spring (grows to fill space) and Fixed-size Spacer. A progress indicator and the tmux left and right components are used by the system.

For host name, user name, and directory components to be accurate, install shell integration. See [Shell Integration](shell-integration.md).

A script can add its own components. See [Python API](python-api.md).

The Interpolated String component uses the same syntax as badges and titles. See [Variables](variables.md) and [Scripting Fundamentals](scripting-fundamentals.md#interpolated-strings).

## Options shared by components

Most components let you set a text color, a background color, and a font. Priority and compression resistance decide which component gives up space first when the bar is too narrow. A component can also have a minimum and maximum width.

## Advanced options

In the Advanced part of the setup window:

- "Prefer tight packing to stable positioning": uses the tightly packed layout instead of the stable one.
- "Remove components with empty values": drops a component that has nothing to show.
- "Draw separator between status bar and terminal": draws a thin line between the bar and the text.

## Where the bar goes

In Settings > Appearance, "Status bar location" puts the bar at the top or the bottom. In the Panes section, "Separate status bars per pane" gives each split pane the status bar of its own profile. With it off, the window shows the status bar of the current session. "Height of the status bar in points" (Advanced) defaults to 21.

The git component runs `git` with the PATH and timeout set in Settings > Advanced ("PATH used when running git for the status bar component" and the timeout setting next to it). "Animate graph-based status bar components?" (Advanced > Drawing) controls the graph animation.

## In tmux tabs

In a tmux tab with the status bar on, the bar shows tmux's own status-left and status-right by default. To show the profile's components instead, turn off the setting (Settings > General > tmux) "Status bar shows tmux status bar content, not native components". See [tmux Integration](tmux-integration.md).

## Related pages

- [Badges](badges.md)
- [Variables](variables.md)
- [Python API](python-api.md)
