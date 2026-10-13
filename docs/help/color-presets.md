# Color Presets

A color preset is a saved set of colors for a profile: foreground, background, the 16 ANSI colors, cursor, selection, and more. Termurion ships with built-in presets, reads iTerm2 `.itermcolors` files, and lets you save your own. There is no online gallery.

## Where to find them

Open Settings (Ctrl+Shift+,), choose Profiles, pick a profile, then open the Colors tab. Press Color Presets... to see the list.

## Built-in presets

These presets come with Termurion:

- Regular
- High Contrast
- Solarized
- Light Background
- Dark Background
- Solarized Light
- Solarized Dark
- Pastel (Dark Background)
- Tango Light
- Tango Dark
- Smoooooth

Regular, High Contrast, and Solarized carry separate light and dark colors. In the menu they come first, each marked with a yin-yang symbol, and a separator follows. The rest are plain presets with one set of colors. Custom presets that you add appear in their own group below, sorted the same way.

A check mark in the menu shows the preset that matches the profile's current colors. A minus sign means that, with separate colors on, one of the two modes matches the preset.

## Load a preset

Click a preset in the menu. Hovering over one for a moment previews it, and moving away puts your colors back.

What happens next depends on the profile's "Use Separate Colors for Light and Dark Mode" option and on the preset:

- Preset has light and dark colors, profile has separate colors off: Termurion loads both sets and turns separate colors on.
- Preset has light and dark colors, profile has separate colors on: Termurion asks "Update Which Modes?" with the choices Update Both Modes and Update Light (or Dark) Mode Only.
- Preset has one set of colors, profile has separate colors on: the same question appears. The single set goes to the mode you choose.
- Preset has one set of colors, profile has separate colors off: the colors replace the profile's colors for the current Windows light or dark appearance.

The Editing selector at the top of the tab (Light Mode or Dark Mode) says which set you change. It is enabled only when separate colors are on. It starts from the current Windows appearance.

## Import a color file

Choose Import... in the menu. Pick one or more `.itermcolors` files (up to 32 at once). Any color scheme made for iTerm2 works, including files from the community collections you find online.

- Each file becomes a custom preset named after the file.
- If a file cannot be read or is not a valid color scheme, Termurion shows "Import Failed."
- If a preset with the same colors already exists, Termurion asks "Add duplicate color preset?" and adds it only if you choose Add it anyway.

## Export and save your own

- Export... writes the current colors to a `.itermcolors` file (the suggested name is `Colors.itermcolors`) and shows the file in Explorer. You can share it with iTerm2 users too.
- Save As... stores the current colors as a custom preset in Termurion. The name box starts with the name of the custom preset most like your current colors, if one is close. Saving under an existing name replaces that preset.
- Delete Preset... removes a custom preset you pick. You cannot delete the built-in presets. With no custom presets, Termurion says "No deletable color presets."

You can keep up to 256 custom presets. Names are limited to 64 characters.

## Presets from a program

A program can switch presets with the `SetColors=preset=<name>` escape sequence (OSC 1337). The `it2setcolor` utility does this. The name can be a built-in or a custom preset.
