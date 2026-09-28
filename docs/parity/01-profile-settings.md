# iTerm2 profile settings

This inventory was built from iTerm2 source in a read-only reference clone (gnachman/iTerm2), primarily sources/Settings/iTermProfilePreferences.m, sources/Settings/Profiles/ITAddressBookMgr.h, and the per-tab ProfilesXPreferencesViewController.m files, plus the trigger, smart selection, and semantic history classes. It lists every user-visible per-profile setting, grouped by the tab it appears under in iTerm2's profile editor.

Tandem is a Windows terminal in Rust aiming for one-to-one parity with iTerm2. As of this writing Tandem has a ConPTY pty, an alacritty_terminal core, a wgpu and glyphon renderer, one hard-coded profile (iTerm2 default colors, Cascadia Mono 12pt, shell pwsh or powershell), tabs, splits with draggable dividers and inactive-pane dimming, xterm key encoding, mouse reporting modes 1000/1002/1003/1006, selection and clipboard behavior, bracketed paste, wheel routing, and 10000 lines of fixed scrollback. Tandem has no settings UI and no profile system yet, so almost every row below is missing as a configurable setting; a few rows are marked partial where Tandem's hard-coded behavior already matches iTerm2's default for that setting.

## General

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Profile name | Name | Names the profile, shown in the profile list and available for the window and tab title. | have |  |
| Profile icon | Icon | Chooses none, an automatic app icon, or a custom image to represent the profile. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Custom icon path | Custom Icon Path | File path to the custom icon image when icon is set to custom. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Profile shortcut | Shortcut | A Cmd+Ctrl+letter or digit keystroke that opens a new window with this profile. | partial | macOS Cmd+Ctrl chord; map to a Windows-equivalent modifier combo. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Tags | Tags | Free-text tags used to organize and search profiles. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Profile type | Profile Type (Phony) | Segmented control choosing Terminal or Browser mode for the profile; synthetic, backed by the command type value. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Command type | Custom Command | Chooses what runs at session start: login shell, custom shell, custom command, SSH, or browser. | partial | Login Shell, Command, and Custom Shell work. SSH works (milestone 14a): the Command runs as `ssh <command>`, or with SSH Integration on as `it2ssh <command>` from the utilities folder (plain ssh with a banner until milestone 14b's conductor hook); a blank Command runs the shell. Browser comes later. |
| Command / command line | Command | The literal command, shell path, or SSH arguments to run, depending on command type. | have |  |
| Initial URL | Initial URL | The URL a Browser-type profile opens on start. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Send text at start | Initial Text | Text automatically typed into the session as soon as it starts. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Initial working directory | Custom Directory | Chooses Home, reuse previous session's directory, a custom path, or per-window/tab/pane advanced rules. | partial | Home, a fixed directory, and Reuse previous session's directory (through shell integration) work; Advanced comes in milestone 7. |
| Custom directory path | Working Directory | The path used when initial working directory is set to custom. | have |  |
| Advanced working directory (window) | AWDS Window Option / AWDS Window Directory | Sets a working-directory rule specific to new windows from this profile. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Advanced working directory (tab) | AWDS Tab Option / AWDS Tab Directory | Sets a working-directory rule specific to new tabs from this profile. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Advanced working directory (pane) | AWDS Pane Option / AWDS Pane Directory | Sets a working-directory rule specific to new split panes from this profile. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Badge text | Badge Text | Overlay text drawn in the corner of the terminal, can reference session variables. | have | Milestone 6b: interpolated text, the largest size that fits, behind the text; Helvetica becomes Arial. |
| Badge appearance (font, margins, max size) | Badge Font, Badge Top Margin, Badge Right Margin, Badge Max Width, Badge Max Height | Configures the badge's font and its position and size limits via a separate sheet. | have | Milestone 6b: interpolated text, the largest size that fits, behind the text; Helvetica becomes Arial. |
| Subtitle | Subtitle | A second line of small text shown under the tab title when the tall tab bar is enabled. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Title components | Title Components | Bitmask popup choosing what appears in the window/tab title: session name, job, working directory, tty, custom, profile name, profile+session name, user, host, command line, size, temporary session name, or an AI-generated name. | have | Milestone 6c: every component but TTY (Windows sessions have none); parts are joined with " - ". Milestone 11c: Custom uses the profile's Title Function, a registered script title provider ("\u{2026}" when none is registered or the provider fails to resolve, the ladybug on any other error); the settings popup lists "Built-in components" plus every registered provider, and choosing "Built-in components" writes Title Components 2 (Job) rather than previewing the built-in title. |
| Custom title function | Title Function | Points the title at a registered scripting-API function instead of the built-in components. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Custom window title | Custom Window Title | Literal override text for the window title when "use custom window title" is on. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use custom window title | Use Custom Window Title | Enables the custom window title override instead of computed title components. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Custom tab title | Custom Tab Title | Literal override text for the tab title when "use custom tab title" is on. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use custom tab title | Use Custom Tab Title | Enables the custom tab title override instead of computed title components. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| URL schemes handled | n/a (app-level scheme registration keyed to profile GUID) | Assigns URL schemes (ssh, http, https, etc) to open with this profile. | missing |  |
| Configure SSH | SSH | Opens a sheet with SSH Integration, the environment variables and files to copy to the remote host, and the path to ssh, for the SSH command type (host, user, port and auth come from the Command, not this sheet). | have | Milestone 14a: "Configure…" shows only for the SSH command type and opens iTerm2's sheet with its four settings (`sshIntegration`, `environmentVariablesToCopy`, `filesToCopy`, `pathToSSH`); Path to SSH becomes the session's `SSH` variable. Variables and files are copied once milestone 14b's conductor runs. Windows note: the variables are one text field split on spaces and commas; "+" offers Files… or Folder… (Windows cannot choose both in one dialog); Browse accepts `.exe` files. |
| Load shell integration automatically | Load Shell Integration Automatically | Automatically injects the shell integration script for supported shells (bash, fish, xonsh, zsh) at session start. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Run command in login shell | Run Command In Login Shell | Wraps a custom command so it still runs inside a login shell for dotfile sourcing. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Allow title setting | Allow Title Setting | Permits escape sequences from the running program to change the session/window title; also drives a lock icon when editing the current session. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Prevent Automatic Profile Switching | Prevent Automatic Profile Switching | Once a session's profile is changed by hand, blocks automatic profile switching rules from overriding it again. | have | Milestone 6c. |
| Session hotkey | Session Hotkey | A one-off keystroke bound to the current session instance (set from Edit Current Session, not stored on the base profile). | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |

## Colors

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Use Separate Colors for Light and Dark Mode | Use Separate Colors for Light and Dark Mode | Splits most other colors and sliders on this tab into independent Light and Dark variants, applied automatically with the system appearance. | have |  |
| Light/Dark mode editing selector | n/a | Popup that picks which appearance variant you are editing right now; tracks system appearance, not itself a saved value. | missing |  |
| Theme overrides mode warning | n/a | Warning shown when the active tab theme forces a fixed appearance so automatic light/dark switching will not occur. | missing |  |
| Foreground Color | Foreground Color | Default text color (has separate Light/Dark variant when enabled). | have |  |
| Background Color | Background Color | Default terminal background color (has separate Light/Dark variant when enabled). | have |  |
| Use custom color for bold text | Use Bright Bold | Enables the Bold Color swatch; also makes bold text in a normal ANSI color use that color's bright variant. | have |  |
| Bold Color | Bold Color | Color used for bold text when the custom bold color option is on. | have |  |
| Use bright version of ANSI colors for bold text | Brighten Bold Text | Brightens text drawn in a normal ANSI color when it is also bold. | have |  |
| Link Color | Link Color | Color for clickable hyperlink and URL text. | have |  |
| Match Color | Match Background Color | Background used to highlight Find, Smart Selection, and Semantic History matches. | have |  |
| Selection Color | Selection Color | Background color of selected text. | have |  |
| Use custom color for selected text | Use Selected Text Color | Enables a custom color for text drawn over a selection instead of keeping its normal foreground color. | have |  |
| Selected Text Color | Selected Text Color | Text color drawn over a selection, when enabled. | have |  |
| Cursor Color | Cursor Color | Color of the text cursor; disabled when Smart Cursor Color is on with a box cursor. | have |  |
| Cursor Text Color | Cursor Text Color | Color of text drawn under or over the cursor. | have |  |
| Smart Cursor Color | Smart Cursor Color | Computes a contrasting cursor color automatically for a box cursor instead of using fixed swatches. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| IME Cursor Color | IME Cursor Color | Cursor color shown while composing text through an Input Method Editor. | partial | Windows also has IME; concept still applies. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Badge Color | Badge Color | Color of the badge text overlay drawn in the corner of the terminal. | have | Milestone 6b: interpolated text, the largest size that fits, behind the text; Helvetica becomes Arial. |
| Use custom tab color | Use Tab Color | Enables a custom color for the session's tab. | have | Milestone 8h: the tab takes the profile's tab color (light and dark variants); a program's tab color wins. |
| Tab Color | Tab Color | Color used to color the session's tab, when enabled. | have | Milestone 8h. |
| Use custom underline color | Use Underline Color | Enables a custom color for underlines instead of matching the underlined text's own color. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Underline Color | Underline Color | Color of underlines under underlined text, when enabled. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Border around active pane | Use Active Pane Border | Draws a custom-colored border around the currently focused split pane. | partial | Tandem dims inactive panes instead of bordering the active one; different mechanism. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Active Pane Border Color | Active Pane Border Color | Color of the focused split-pane border, when enabled. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use cursor guide | Use Cursor Guide | Draws a highlighted horizontal guide across the row containing the text cursor. | have | View > Show Cursor Guide turns it on or off per session. |
| Cursor Guide Color | Cursor Guide Color | Color and opacity of the cursor guide row highlight. | have |  |
| Use ANSI colors for mark indicators | Use Theme Mark Colors | Makes command-status marks in the left margin use the ANSI palette instead of fixed theme colors. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Harmonize 256 Colors | Harmonize 256 Colors | Chooses the extended 256-color palette to visually match the 8 normal ANSI colors instead of the standard RGB cube. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use HDR cursor when available | HDR Cursor | On HDR-capable displays, draws the cursor at maximum HDR brightness white. | partial | Windows also supports HDR displays; not macOS-specific. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Ansi 0 Color (Black) | Ansi 0 Color | ANSI palette slot 0. | have |  |
| Ansi 1 Color (Red) | Ansi 1 Color | ANSI palette slot 1. | have |  |
| Ansi 2 Color (Green) | Ansi 2 Color | ANSI palette slot 2. | have |  |
| Ansi 3 Color (Yellow) | Ansi 3 Color | ANSI palette slot 3. | have |  |
| Ansi 4 Color (Blue) | Ansi 4 Color | ANSI palette slot 4. | have |  |
| Ansi 5 Color (Magenta) | Ansi 5 Color | ANSI palette slot 5. | have |  |
| Ansi 6 Color (Cyan) | Ansi 6 Color | ANSI palette slot 6. | have |  |
| Ansi 7 Color (White) | Ansi 7 Color | ANSI palette slot 7. | have |  |
| Ansi 8 Color (Bright Black) | Ansi 8 Color | Bright variant of Black. | have |  |
| Ansi 9 Color (Bright Red) | Ansi 9 Color | Bright variant of Red. | have |  |
| Ansi 10 Color (Bright Green) | Ansi 10 Color | Bright variant of Green. | have |  |
| Ansi 11 Color (Bright Yellow) | Ansi 11 Color | Bright variant of Yellow. | have |  |
| Ansi 12 Color (Bright Blue) | Ansi 12 Color | Bright variant of Blue. | have |  |
| Ansi 13 Color (Bright Magenta) | Ansi 13 Color | Bright variant of Magenta. | have |  |
| Ansi 14 Color (Bright Cyan) | Ansi 14 Color | Bright variant of Cyan. | have |  |
| Ansi 15 Color (Bright White) | Ansi 15 Color | Bright variant of White. | have |  |
| Minimum Contrast | Minimum Contrast | Forces a minimum contrast ratio between text and background, shifting colors toward black or white as needed. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Excessive contrast warning | n/a | Warning shown when Minimum Contrast is set so high most colors render as plain black and white. | missing |  |
| Faint Text Alpha | Faint Text Alpha | Opacity used to render faint or dim styled text. | have |  |
| Cursor Boost | Cursor Boost | Increases the cursor color's saturation relative to surrounding text so it stands out. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Color Presets menu | n/a | Lists built-in and custom named color schemes; clicking one applies all its color keys to the profile. | missing |  |
| Import preset | n/a | Imports one or more .itermcolors files into the custom presets list. | missing |  |
| Export preset | n/a | Saves the profile's current colors to a .itermcolors file. | missing |  |
| Save preset as | n/a | Saves the current colors as a new named custom preset. | missing |  |
| Delete preset | n/a | Deletes a chosen custom preset; built-in presets cannot be deleted. | missing |  |
| Visit Online Gallery | n/a | Opens the iterm2.com color gallery web page. | missing |  |
| Edit/remove color binding | n/a | Binds a color swatch to a dynamic expression (session variable or function call) instead of a fixed color. | missing | The expression language and script function calls exist (milestone 11c); this row is the color-picker UI to bind a swatch to one, which is not built. |
| Hue/Chroma visualization wheel | n/a | Graphical wheel plotting the six chromatic ANSI color pairs by hue and chroma for drag-to-edit color picking. | missing |  |

## Text

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Normal (ASCII) font | Normal Font | Font family, weight, and size for ASCII text, and for all text when no non-ASCII font is set. | have |  |
| Non-ASCII font | Non Ascii Font | Font family and size for characters above code point 127, when enabled. | have |  |
| Use a different font for non-ASCII text | Use Non-ASCII Font | Turns on the separate non-ASCII font, antialiasing, ligature, and spacing controls. | have |  |
| Manage Special Exceptions | Special Font Config | Assigns additional per-Unicode-range font overrides beyond the normal and non-ASCII font pair. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Horizontal Spacing | Horizontal Spacing | Stretches or compresses the character cell width, 50 to 200 percent. | have |  |
| Vertical Spacing | Vertical Spacing | Stretches or compresses the line height, 50 to 200 percent. | have |  |
| Cursor style | Cursor Type | Chooses underline, vertical bar, or filled box cursor shape. | have | Renderer draws a fixed cursor shape, not selectable. |
| Blinking cursor | Blinking Cursor | Makes the text cursor blink. | have |  |
| Cursor shadow | Cursor Shadow | Draws a soft drop shadow under the cursor; only for underline and bar styles. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Hide cursor when unfocused | Cursor Hidden When Unfocused | Hides the cursor when the window or pane is not focused. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Cursor smooth slide | Cursor Smooth Slide | Animates small cursor movements with an eased slide instead of an instant jump. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Cursor smooth blink | Cursor Smooth Blink | Replaces abrupt on/off blinking with an eased fade cycle. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink fade in duration | Cursor Blink Fade In Duration | Seconds the smooth-blink fade from hidden to visible takes. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink fade out duration | Cursor Blink Fade Out Duration | Seconds the smooth-blink fade from visible to hidden takes. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink fade in curve | Cursor Blink Fade In Curve | Easing applied to the fade in: linear, ease in, ease out, or ease in-out. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink fade out curve | Cursor Blink Fade Out Curve | Easing applied to the fade out, same four options. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink visible dwell | Cursor Blink Visible Dwell | Seconds the cursor holds fully visible between fades. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink hidden dwell | Cursor Blink Hidden Dwell | Seconds the cursor holds fully hidden between fades. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Smooth blink presets | n/a | One-click presets (Breathing, Linear, Fast, Slow, Subtle) for the six smooth-blink values. | missing |  |
| Allow blinking text | Blink Allowed | Permits text carrying the terminal blink attribute to actually blink. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Animate large cursor movements | Animate Movement | Plays an animation when the cursor makes a large jump. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Animate movement only in interactive apps | Animate Movement Only in Interactive Apps | Restricts large-movement animation to alternate-screen apps. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Draw bold text in bold font | Use Bold Font | Renders bold-attribute text with the font's real bold variant instead of faux weight. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Allow italic text | Use Italic Font | Permits rendering italic-attribute text in an italic font variant. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Thin Strokes | Thin Strokes | Chooses when to render glyphs with thinner strokes: never, always, on Retina, with dark backgrounds, or both. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Anti-aliased (ASCII) | ASCII Anti Aliased | Controls whether the normal font is anti-aliased. | partial | glyphon renders anti-aliased glyph textures by default, not a toggle. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Anti-aliased (non-ASCII) | Non-ASCII Anti Aliased | Controls whether the non-ASCII font is anti-aliased. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use ligatures (ASCII) | ASCII Ligatures | Enables optional typographic ligatures for the normal font. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use ligatures (non-ASCII) | Non-ASCII Ligatures | Enables optional typographic ligatures for the non-ASCII font. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use built-in Powerline glyphs | Draw Powerline Glyphs | Draws Powerline symbol glyphs with iTerm2's own renderer instead of relying on the font. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Ambiguous characters are double-width | Ambiguous Double Width | Treats Unicode ambiguous-width characters as occupying two cells instead of one. | partial | alacritty_terminal applies a fixed unicode-width table; not adjustable. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Unicode Normalization Form | Unicode Normalization | Chooses how incoming Unicode text is normalized before rendering: none, NFC, NFD, or HFS+. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use Unicode version 9+ widths | Unicode Version | Toggles the character-width table between Unicode 8 and Unicode 9+, which narrowed most emoji from 2 cells to 1. | partial | alacritty_terminal uses one fixed width table version, not selectable. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |

## Window

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Columns (initial width) | Columns | Initial window width in character columns. | have |  |
| Rows (initial height) | Rows | Initial window height in character rows. | have |  |
| Width, % of screen | Width Percentage | Initial window width as a percentage of screen width instead of a fixed column count. | have | Milestone 7a: the edge styles use it; a negative value means the columns. |
| Height, % of screen | Height Percentage | Initial window height as a percentage of screen height instead of a fixed row count. | have | Milestone 7a. |
| Columns/Rows units popup | n/a | Chooses whether size is given in cells or percent of screen; hidden for styles with no variable size. | partial | Milestone 7a: the Window style list offers each edge style in its percent and its cells form (iTerm2 stores the cells id when neither popup says % of screen); there is no separate units popup. |
| Width, points (Browser profile) | Width in Points | Initial window width in points for the Browser profile type. | partial | Not applicable; Tandem has no browser profile type. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Height, points (Browser profile) | Height in Points | Initial window height in points for the Browser profile type. | partial | Not applicable; Tandem has no browser profile type. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Window style: Normal | Window Type = 0 | Ordinary resizable titled window. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Full Screen | Window Type = 1 | Opens directly into fullscreen, hiding the menu bar. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Lion Full Screen | Window Type = 4 | Runtime variant using native macOS Spaces-based fullscreen; not directly selectable. | missing | macOS-only concept (Spaces); no Windows analogue. |
| Window style: Top of Screen | Window Type = 2 or 9 | Window docked to the top edge of the screen. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Bottom of Screen | Window Type = 5 or 8 | Window docked to the bottom edge of the screen. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Left of Screen | Window Type = 6 or 10 | Window docked to the left edge of the screen. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Right of Screen | Window Type = 7 or 11 | Window docked to the right edge of the screen. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: No Title Bar | Window Type = 12 | Resizable window with no title bar or chrome. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Compact | Window Type = 13 | Theme-derived compact-chrome variant of Normal, substituted automatically under the Compact tab style. | have | Milestone 7a: opens as its Normal, Maximized, or Centered form (the compact chrome is a macOS theme). |
| Window style: Accessory | Window Type = 14 | Floating panel-like window, reachable only via scripting API or command line, not the popup. | missing | macOS-panel concept; closest Windows equivalent is a topmost tool window. |
| Window style: Maximized | Window Type = 15 | Window opens filling the available screen area, keeping title bar visible. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Compact, Maximized | Window Type = 16 | Theme-derived compact-chrome equivalent of Maximized. | have | Milestone 7a: opens as its Normal, Maximized, or Centered form (the compact chrome is a macOS theme). |
| Window style: Centered | Window Type = 17 | Window opens centered on screen at the given size. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Window style: Compact, Centered | Window Type = 18 | Theme-derived compact-chrome equivalent of Centered. | have | Milestone 7a: opens as its Normal, Maximized, or Centered form (the compact chrome is a macOS theme). |
| Window style: Centered, No Title Bar | Window Type = 19 | Combines centered placement with no title bar in every theme. | have | Milestone 7a: iTerm2's frame rules on the monitor's work area; Full Screen is borderless full screen. |
| Screen | Screen | Picks which physical display a new window opens on. | have | Milestone 7a: No Preference, Screen with Cursor, or a monitor by number (the primary first). |
| Space | Space | Picks which macOS Space (virtual desktop) a new window opens on: current, all, or a specific numbered space. | partial | macOS-only concept (Spaces); Windows virtual desktops have no per-app auto-assign setting. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Hide after opening | Hide After Opening | New windows are immediately hidden after opening. | have | Milestone 7a: the window opens minimized. |
| Lock window size automatically | Lock Window Size Automatically | Prevents the window from being resized by escape sequences or the user once opened. | have | Milestone 7a. |
| Always open in a new window, never a tab | Prevent Opening in a Tab | Forces this profile to always create a new window instead of a new tab. | have | Milestone 7a. |
| Open toolbelt | Open Toolbelt | New windows open with the Toolbelt sidebar panel visible. | have | Milestone 9j: fires on a new, non-arranged window's first tab (tab count 0 to 1), including a hotkey window (which never grows for it); a restored arrangement uses its own "Has Toolbelt" key instead. |
| Use custom window title | Use Custom Window Title | Enables overriding the window title bar text with a custom string. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Custom window title | Custom Window Title | The interpolated-string template used as the window title, when enabled. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use custom tab title | Use Custom Tab Title | Enables overriding the tab title text with a custom string. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Custom tab title | Custom Tab Title | The interpolated-string template used as the tab title, when enabled. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use transparency | Initial Use Transparency | Master on/off switch for whether the window starts with background transparency. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Transparency | Transparency | Slider setting how transparent the background is. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Keep background colors opaque | Only The Default BG Color Uses Transparency | Restricts transparency to the default background color only, leaving non-default backgrounds opaque. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Blur | Blur | Enables a Gaussian-style blur of content behind the window, visible through transparent regions. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Blur Radius | Blur Radius | Sets the strength of the background blur effect. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Use background image | n/a | Toggles whether a background image is shown at all. | missing |  |
| Background image source mode | Background Image Source Mode | Chooses a single fixed image versus rotating automatically through a folder. | have | Milestone 5d. |
| Background image (path) | Background Image Location | Path to the single background image file. | have | Milestone 5d. |
| Background image folder (path) | Background Image Folder Location | Path to a folder of images to rotate through. | have | Milestone 5d. |
| Background image rotation interval | Background Image Folder Interval | Seconds between automatic image changes in folder rotation mode. | have | Milestone 5d. |
| Background image scaling mode | Background Image Mode | Chooses stretch, tile, scale to fill, or scale to fit for the background image. | have | Milestone 5d. |
| Blending | Blend | Sets how much of the background color shows through over the background image. | have | Milestone 5d. |

## Terminal

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Scrollback Lines | Scrollback Lines | Number of scrollback lines to keep, 0 up to 10,000,000. | have |  |
| Unlimited Scrollback | Unlimited Scrollback | Removes the scrollback line cap entirely. | have |  |
| Save lines to scrollback with status bar present | Scrollback With Status Bar | Controls whether lines are added to scrollback while a status bar (e.g. tmux) is shown. | have | Milestone 9h: with it off, a scroll region on the main screen that starts at the top row and stops above the bottom row (a status line kept below it) scrolls without saving lines; a full-screen scroll still saves, and the alternate screen (tmux, vim, less) keeps no history either way (alacritty_terminal patch 14). |
| Save lines to scrollback in alternate screen | Scrollback in Alternate Screen | Controls whether output is added to scrollback while the alternate screen buffer is active. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Drag to scroll disabled in alternate screen | Drag to Scroll in Alternate Screen Mode Disabled | Disables click-and-drag scrolling while the alternate screen is active. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Automatically enable alternate mouse scroll | Automatically Enable Alternate Mouse Scroll | Automatically turns on translating scroll wheel input into arrow keys when an app enables the alternate screen. | partial | Tandem already routes wheel to arrow keys in the alt screen. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Restrict alternate mouse scroll to vertical | Restrict Alternate Mouse Scroll to Vertical | Limits the alt-screen scroll-to-arrow translation to vertical scrolling only. | partial | Mouse wheel input Tandem handles is vertical only. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Applications may use alternate mouse scroll | Allow Alternate Mouse Scroll | Permits the alt-screen scroll-to-arrow-key feature to be used at all. | partial | Always on in Tandem; no toggle to disable it. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Character Encoding | Character Encoding | Text encoding used to interpret the terminal's byte stream. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Terminal Type (report as) | Terminal Type | The TERM value the terminal reports to programs, e.g. xterm-256color. | partial | alacritty_terminal reports a fixed TERM value, not editable per profile. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Answerback String | Answerback String | String sent in response to an ENQ terminal-identification query. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Report Mouse Clicks and Drags (Mouse Reporting) | Mouse Reporting | Enables xterm-style mouse reporting to the running program. | partial | Tandem implements mouse reporting modes 1000/1002/1003/1006, always on. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Report Mouse Wheel Events | Mouse Reporting allow mouse wheel | Lets scroll wheel events be reported to the terminal program. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Report Mouse Clicks and Drags | Mouse Reporting allow clicks and drags | Lets click and drag events be reported to the terminal program. | partial | Tandem supports drag/double/triple/block selection alongside reporting modes 1002/1003. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Restrict mouse reporting to alternate screen | Restrict Mouse Reporting to Alternate Screen Mode | Limits xterm mouse reporting so it only applies while the alternate screen is active. | partial | Tandem's reporting is always active, not restricted by screen mode. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Terminal may report its title | Allow Title Reporting | Allows programs to query the current window or tab title via escape sequence. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Applications may use paste bracketing | Allow Paste Bracketing | Allows the terminal to wrap pasted text in bracketed-paste escape sequences for apps that request it. | have | Milestone 9a: off, pastes are never bracketed. |
| Disable Printing | Disable Printing | Blocks the terminal from honoring escape-sequence print requests. | partial | Tandem has no print support either way. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Disable Alternate Screen (smcup/rmcup) | Disable Smcup Rmcup | Ignores smcup/rmcup so full-screen apps cannot switch to the alternate screen. | partial | alacritty_terminal supports the alt screen itself; there is no toggle to block it. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Terminal may resize window | Disable Window Resizing | Controls whether escape sequences may resize the terminal window. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Terminal may resize window when not focused | Disable Window Resizing by Unfocused Sessions | Controls whether an unfocused window can still be resized by escape sequences. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Programs can change cursor blink | Allow Change Cursor Blink | Allows escape sequences to toggle whether the cursor blinks. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Silence Bell | Silence Bell | Mutes the audible bell sound. | have | Milestone 6b: the Windows Default Beep, the tab bell icon, the pane flash. |
| Send Notification (master toggle) | BM Growl | Enables user notifications for terminal events. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Bell (alert filter) | Send Bell Alert | Whether a bell event triggers a notification. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Idle (alert filter) | Send Idle Alert | Whether the session going idle triggers a notification. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| New Output (alert filter) | Send New Output Alert | Whether new output after idle triggers a notification. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Session Ended (alert filter) | Send Session Ended Alert | Whether the session ending triggers a notification. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Terminal-Generated Alerts (alert filter) | Send Terminal Generated Alerts | Whether escape-sequence-generated alerts trigger a notification. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Suppress Alerts in Active Session | Suppress Alerts in Active Session | Suppresses notifications for the session currently focused. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |
| Also Send to Companion | Send Alerts To Companion | Also delivers alerts to a paired companion iPhone app. | partial | macOS/iOS-only concept. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Flashing Bell | Flashing Bell | Flashes the screen when the bell rings. | have | Milestone 6b: the Windows Default Beep, the tab bell icon, the pane flash. |
| Visual Bell | Visual Bell | Enables a visual indicator for the bell. | have | Milestone 6b: the Windows Default Beep, the tab bell icon, the pane flash. |
| Set Locale Variables | Set Local Environment Vars | Chooses how $LANG/$LC_CTYPE are set for new sessions: do not set, automatic, custom, or minimal. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Change Locale | Custom Locale | Opens a locale picker for the custom locale mode. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Place prompt at first column | Place Prompt at First Column | Forces the shell prompt to always be placed at column 1. | have |  |
| Show Mark Indicators | Show Mark Indicators | Shows indicator marks in the left margin for shell-integration command marks. | have | A bar in the left margin, in iTerm2's colors. |
| Cmd-click on path opens navigator | Prompt Path Click Opens Navigator | Makes a click on a shell-integration-reported path open it in the file navigator. | partial | Requires shell integration; also a macOS Cmd-click convention. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Show Offscreen Command Line | Show Offscreen Command line | Shows a floating bar with the current prompt when it has scrolled off the top. | partial | Requires shell integration. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Show offscreen command line for current command | Show Offscreen Command line for Current Command | Extends the floating bar to show the running command, not just the prompt. | partial | Requires shell integration. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| tmux uses newline (vs CRLF) | Tmux Newline | Controls whether tmux integration sends newline or CRLF line endings. | have | Milestone 8a: tmux commands end with "\n" when on, "\r" when off. |
| Filter Alerts | n/a | Opens the sheet containing the per-event alert filter checkboxes. | have | Milestone 6b: Windows notifications (a tray balloon shown as a toast); a click reveals the session. |

## Session

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Close or restart session on end | Close Sessions On End | What happens when the shell process exits: default, close, or restart. | partial | Tandem closes the tab/pane when the process exits, matching the default; close/restart are not selectable. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Warn about short-lived sessions | n/a | Warns when a session that ended very quickly is about to close automatically. | missing |  |
| Prompt before closing | Prompt Before Closing 2 | Never prompt, always prompt, or prompt unless only ignorable jobs are running. | have | Milestone 6c: Never, Always, or when jobs besides the ignore list run (every program under the shell counts); an OK and Cancel question with iTerm2's text. |
| Jobs to Ignore | Jobs to Ignore | Job or process names that do not count as "still running" for the prompt-before-closing check. | have | Milestone 6c: ssh, telnet, and plink by default (iTerm2: rlogin, ssh, slogin, telnet); names match without case and with or without .exe; conhost and OpenConsole never count. |
| Session Close Undo Timeout | Session Close Undo Timeout | Seconds available to undo (reopen) a closed session. | have | Milestone 7b: the closed shell keeps running, not drawn, until the timeout; 0 turns Undo Close off. |
| Automatically Log | Automatically Log | Enables automatic session logging to disk. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Log Directory | Log Directory | Directory session logs are written to. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Log Filename Format | Log Filename Format | Filename template for the log file, supporting variable substitution. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Logging style | Plain Text Logging | Chooses raw, plain text, HTML, or asciicast logging. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Archive On Closure | Archive On Closure | Enables automatic archiving of the session's content when it closes. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Archive Directory | Archive Directory | Directory closed-session archives are written to. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Send code when idle | Send Code When Idle | Anti-idle/keepalive checkbox that periodically sends an ASCII code while the session is idle. | have | Milestone 6b. |
| Idle code | Idle Code | ASCII code value, 0 to 255, sent periodically while idle. | have | Milestone 6b. |
| Idle period | Idle Period | Seconds between sending the idle code. | have | Milestone 6b. |
| Reduce Flicker | Reduce Flicker | Enables extra frame synchronization to reduce visual flicker during redraws. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Prevent Sleep | Prevent Sleep | Keeps the system from idle-sleeping while a session using this profile exists. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Default Pane Locked | Default Pane Locked | Whether new split panes from this profile start locked against closing or replacement. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Buffer Input by Default | Buffer Input by Default | Whether keyboard input is buffered by default in new sessions from this profile. | have | Milestone 5c. |
| Show Status Bar | Show Status Bar | Enables the per-session status bar. | have | Milestone 9g: "Status bar enabled" on Profiles > Session with iTerm2's tooltip; the bar shows at the top or bottom of the tab area for the current session, or per pane; the bar's menu has Disable Status Bar (a session's own profile copy when it has one). |
| Configure Status Bar | Status Bar Layout | Opens a sheet to arrange status bar components such as clock, CPU, network, memory, working directory, and git branch. | have | Milestone 9g: the button (on while the bar is enabled), the bar's menu, and the placeholder open iTerm2's setup window: component menu, Active Components with drag, drop, reorder and delete, Configure Component for every knob type, Auto-Rainbow, Advanced, live apply with Cancel restoring. The layout is iTerm2's dictionary; classes Tandem does not show yet stay in it. Milestone 9h adds the content components to the menu; milestone 9i adds Battery, CPU, Memory, Network and git state, with Windows samplers behind the four graphs and the git CLI behind the git component. |
| Open Password Manager Automatically | Open Password Manager Automatically | Opens the password manager when the session reaches a password prompt. | have | Milestone 10b: fires on the rising edge of a detected password prompt (echo off, line input on, debounced), not at session start (a correction to the initial research, section 0). Readers that use raw mode (ssh.exe, runas, WSL) are not detected; see the m10b rulings. |
| Timestamps Style | Timestamps Style | How per-line timestamps display when shown: overlap, hover, or adjacent. | partial | Over contents and Hover work; Next to contents draws like Over contents, since the grid keeps its width. |
| Timestamps Visible | Timestamps Visible | Whether per-line timestamps are shown at all. | have | View > Show Timestamps turns them on or off per session. |
| Enable Progress Bars | Enable Progress Bars | Enables OSC 9;4 style progress-bar indicators in the session. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Progress Bar Height | Progress Bar Height | Height of the progress bar in points. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Progress Bar Color Scheme | Progress Bar Color Scheme | Color scheme for the progress bar: default, rainbow, red, green, blue, yellow, purple, cyan, or orange. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |

## Keys

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Option Key Sends (Left) | Option Key Sends | Chooses what the left Option key sends: Normal, Meta, or Esc+. | have | Alt is Option; left and right Alt each follow their own setting. Normal acts as Esc+ (Windows has no Option character layer). |
| Option Key Sends (Right) | Right Option Key Sends | Same popup for the right-hand Option key, letting the two sides differ. | have | Missing right-side value follows the left, as in iTerm2. |
| Left Option Key Independently Changeable | Left Option Key Changeable | Unlocks the left Option key's own dropdown instead of mirroring the right. | partial | macOS-only concept. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Right Option Key Independently Changeable | Right Option Key Changeable | Unlocks the right Option key's own dropdown instead of mirroring the left. | partial | macOS-only concept. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Application Keypad Allowed | Application Keypad Allowed | Permits apps to switch the numeric keypad into application mode via escape sequences. | have |  |
| Apps can change how keys are reported | Allow modifyOtherKeys | Lets programs turn on modifyOtherKeys and the kitty keyboard protocol. | have | Milestone 5b. |
| Movement keys scroll outside interactive apps | Movement Keys Scroll Outside Interactive Apps | Lets arrow/movement keys scroll the buffer instead of being sent to the shell outside interactive apps. | have |  |
| Treat Option as Alt | Treat Option as Alt | Chooses whether Esc+ mode reports the key as Alt versus older Meta-style behavior. | have |  |
| Report keys using CSI u | Use libtickit protocol | Starts each session in CSI u key reporting (fixterms). | have | Milestone 5b. |
| Left Control Key | Left Control Key | Remaps the left Control key to a bucky bit: regular, Hyper, Meta, or Super. | partial | Saved and shown in Settings; the effect comes with the keyboard protocols in milestone 5. |
| Right Control Key | Right Control Key | Remaps the right Control key independently to a bucky bit. | partial | Saved and shown in Settings; the effect comes with the keyboard protocols in milestone 5. |
| Left Command Key | Left Command Key | Remaps the left Command key to a bucky bit. | partial | Saved and shown in Settings; the effect comes with the keyboard protocols in milestone 5. |
| Right Command Key | Right Command Key | Remaps the right Command key independently to a bucky bit. | partial | Saved and shown in Settings; the effect comes with the keyboard protocols in milestone 5. |
| Function Key | Function Key | Remaps the hardware Fn key to a bucky bit. | partial | Saved and shown in Settings; the effect comes with the keyboard protocols in milestone 5. |
| Delete sends Ctrl-H | n/a (a Keyboard Map entry) | Makes Delete/Backspace send Control-H instead of the default DEL byte. | missing |  |
| Key Mappings table | Keyboard Map | Per-profile table of custom keystroke-to-action bindings. | have | Every action code is stored; actions owned by later milestones say so when pressed. |
| Key Mapping Presets | n/a | Loads a built-in preset (xterm-style, Natural, Terminal.app compatibility, Numeric Keypad) or imports/exports bindings. | have | The three iTerm2 presets, replace or add. |
| Repair Key Mappings | n/a | Repairs a profile's key mappings if they are detected as corrupted. | have | Nothing to repair: Tandem never wrote the broken form iTerm2 repairs. |
| Has Hotkey (global hotkey window) | Has Hotkey | Turns this profile into a global Quake-style dropdown terminal window. | have | Milestone 7e. |
| Configure Hotkey Window | n/a | Opens the sheet for setting the hotkey shortcut and window behavior. | have | Milestone 7e: the HotKey Window Preferences sheet with every control; conflict notes read "Warning:". |
| Hotkey shortcut | HotKey Key Code, HotKey Characters, HotKey Characters Ignoring Modifiers, HotKey Modifier Flags | Records the global keystroke that toggles the hotkey window. | have | Milestone 7e: RegisterHotKey, or the keyboard hook for Win chords the shell owns; Cmd is Win, Option is Alt. |
| Hotkey window autohide | HotKey Window AutoHides | Hides the hotkey window automatically when it loses focus. | have | Milestone 7e: another app or a non-sibling terminal window hides it; Settings and dialogs do not. |
| Reopen hotkey window on activation | HotKey Window Reopens On Activation | Reshows a previously autohidden hotkey window when the app is activated. | have | Milestone 7e. |
| Animate hotkey window | HotKey Window Animates | Enables a slide/fade animation when the hotkey window shows or hides. | partial | Milestone 7e: top, bottom, and right edge styles slide (0.25 s, ease-in); styles iTerm2 fades show and hide at once. |
| Hotkey window floats | HotKey Window Floats | Keeps the hotkey window always on top of other windows. | have | Milestone 7e: always on top. |
| Hotkey window dock click action | HotKey Window Dock Click Action | Chooses what clicking the Dock icon does regarding the hotkey window. | partial | Milestone 7e: kept and shown disabled; Windows has no Dock and hotkey windows have no taskbar button. |
| Activate hotkey with modifier alone | HotKey Activated By Modifier, HotKey Modifier Activation | Lets a bare modifier tap, instead of a full combo, toggle the hotkey window. | have | Milestone 7e: double tap of Control, Shift, Alt, or Win (0.01 to 0.3 s); AltGr and key repeat never count. |
| Alternate hotkey shortcuts | HotKey Alternate Shortcuts | Additional keystrokes that also trigger the same hotkey window. | have | Milestone 7e: the Additional Hotkeys list. |
| Touch Bar item list | Touch Bar Map | Per-key mapping table for Touch Bar buttons. | partial | macOS-only, hardware-specific; Touch Bar no longer exists on current Macs. Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |

## Advanced

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Automatic Profile Switching rules | Bound Hosts | Hostname, username, or path rules that auto-switch to this profile when the remote host matches, via shell integration. | have | Milestone 6c: iTerm2's rules and scores, the profile stack, and sticky rules; a rule starting with a drive (C:\) is a folder rule; the job comes from the process tree. |
| Snippets Filter | Snippets Filter | Filters which of the app's global Snippets appear in this profile's Snippets menu. | have | Milestone 9e: Edit > Snippets lists only snippets holding every tag (exact names) of the focused session's profile; the Snippets toolbelt tool comes with the toolbelt. |
| Enable triggers in interactive apps | Enable Triggers in Interactive Apps | Lets triggers keep firing while a full-screen interactive program is running. | have | Milestone 6a. |
| Triggers use interpolated strings | Triggers Use Interpolated Strings | Makes trigger parameter fields evaluate as interpolated-string expressions instead of literal backreferences. | have | Milestone 6a: names, matches[n], quoted strings, and ??. Milestone 11c: the full iTerm2 expression language (arithmetic, comparisons, `x?`, indexing, `&path` references); trigger parameters always run lax and refuse function calls with "Function calls are not allowed here." (Decision: screen code has no scope to call from); badges, Automatic Profile Switching rules, coprocess command templates, and Copy Interpolated String refuse calls the same way. |
| Smart Selection actions use interpolated strings | Smart Selection Actions Use Interpolated Strings | Makes smart-selection action parameters evaluate as interpolated-string expressions. | partial | Saved and shown in Settings (milestone 2); the effect comes in a later milestone, see 00-roadmap.md. |
| Edit Triggers | n/a | Opens the Triggers editor for this profile's trigger list. | have | Milestone 6a. |
| Edit Smart Selection | n/a | Opens the Smart Selection editor for this profile's rule list. | have | Profile Advanced tab. |
| Edit Semantic History | Semantic History | Opens the Semantic History configuration for cmd/opt-click behavior on paths and URLs. | have | Profile Advanced tab. |
| Automatic Profile Switching debug logging | n/a | Logs why and when automatic profile switching rules match, for debugging. | have | Milestone 6c; since milestone 11d the lines go to the Script Console's "Automatic Profile Switching" entry (`automatic-profile-switching.log` is no longer written). |

## Triggers

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Trigger list | Triggers | Ordered list of regex-based rules that watch output and perform an action when matched. | have | Milestone 6a. |
| Regex / match pattern | n/a (per-row field) | The regular expression a trigger tests each new line or screen content against. | have | Milestone 6a. |
| Parameter | n/a (per-row field) | The action-specific argument for the chosen trigger action. | have | Milestone 6a. |
| Enabled checkbox | n/a (per-row field) | Turns an individual trigger on or off without deleting it. | have | Milestone 6a. |
| Instant (partial line) flag | n/a (per-row field) | Lets a trigger fire on a partial, not yet newline-terminated, line. | have | Milestone 6a. |
| Show Alert | n/a (action) | Shows a modal alert dialog containing the matched text. | have | Milestone 6a: an announcement bar with OK and Disable This Alert, once per second per trigger. |
| Annotate | n/a (action) | Attaches an inline annotation to the matching text. | have | Milestone 6a. |
| Ring Bell | n/a (action) | Rings the terminal bell when the pattern matches. | have | Milestone 6b: the bell settings apply. |
| Bounce Dock Icon | n/a (action) | Bounces the app's Dock icon to get attention. | have | Milestone 6a: the taskbar button flashes (until Tandem is in front, or once). |
| Capture Output | n/a (action) | Captures the matching line into the Captured Output tool for later re-run. | missing | Listed in the editor; the Captured Output tool comes in milestone 9. |
| Run Coprocess | n/a (action) | Launches an external command fed the session's I/O, shown in the session. | have | Milestone 6c: cmd.exe /d /s /c, hidden, in its own Job Object; one per session (a second shows iTerm2's warning). |
| Run Silent Coprocess | n/a (action) | Same as Run Coprocess but its output is not displayed. | have | Milestone 6c: the session's output is not drawn while it runs. |
| Highlight Text | n/a (action) | Recolors just the matched text. | have | Milestone 6a. |
| Highlight Line | n/a (action) | Recolors the entire line containing the match. | have | Milestone 6a. |
| Make Hyperlink | n/a (action) | Turns the matched text into a clickable hyperlink built from the match. | have | Milestone 6a. |
| Invoke Script Function | n/a (action) | Invokes a registered scripting API function, passing the match as arguments. | have | Milestone 11c: calls the registered function with the match's temporary variables (`trigger.captured_strings`, `trigger.captured_ranges`, `trigger.input`, `trigger.line_number`) and backreferences as `matches`; every regex match on the line invokes, origin "Trigger"; a failure shows the (silenceable, mergeable) Function Call Failed alert. |
| Set Title | n/a (action) | Sets the session title to a value derived from the match. | have | Milestone 6a. |
| Prompt Detected | n/a (action) | Marks the matching line as a shell prompt for prompt-based navigation. | have | Milestone 6a. |
| Post Notification | n/a (action) | Posts a system notification containing the matched text. | have | Milestone 6b: a Windows notification. |
| Set Mark | n/a (action) | Drops a mark at the matching line for later jump-to-mark navigation. | have | Milestone 6a. |
| Open Password Manager | n/a (action) | Opens the password manager, optionally filtered to an account name. | have | Milestone 10b: the Open Password Manager trigger action, opening to the matched account name (or unlocked for UNLOCK or an empty parameter). |
| Run Command | n/a (action) | Runs a shell command, not shown in the session, when the pattern matches. | have | Milestone 6a: cmd.exe /d /c, hidden, 16 at once. |
| Send Text | n/a (action) | Sends literal text to the session as if typed. | have | Milestone 6a. |
| Report Directory | n/a (action) | Tells the terminal the current working directory, emulating shell-integration reporting. | have | Milestone 6a. |
| Report User and Host | n/a (action) | Tells the terminal the current user and hostname, feeding Automatic Profile Switching. | have | Milestone 6a. |
| Stop Processing Triggers | n/a (action) | Halts evaluation of further triggers on the current line. | have | Milestone 6a. |

## Smart Selection

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Smart Selection rule list | Smart Selection Rules | Ordered regex rules that classify what a double or triple click selects, each with a precision level and actions. | have | Rules are ICU regular expressions; Tandem reads them with fancy-regex after mapping [:letter:] and [:number:]. A rule that still does not compile is skipped, and the editor says so. |
| Built-in rules | n/a | Ships default regexes for a whitespace-bounded word, C++ namespace::identifier, file paths, quoted strings, dotted Java/Python paths, mailto/http/https/ssh/telnet URLs, Obj-C selectors, and email addresses. | have |  |
| Open File | n/a (action) | Opens the selected text as a file path with its default or resolved application. | have |  |
| Open URL | n/a (action) | Opens the selected text as a URL in the browser. | have |  |
| Run Command | n/a (action) | Runs a shell command built from the selected text without opening a window. | have | Runs through cmd.exe, with the captures quoted for it. |
| Run Coprocess | n/a (action) | Runs a coprocess fed by or feeding the session, built from the selected text. | have | Milestone 6c. |
| Send Text | n/a (action) | Types the selected text, or a template built from it, back into the session. | have |  |
| Run Command in Window | n/a (action) | Runs a shell command built from the selected text and shows its output in a new window. | partial | Runs the command on its own, without a window, until separate windows arrive in milestone 7. |
| Copy | n/a (action) | Copies the selected or matched text to the clipboard. | have |  |

## Semantic History

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Semantic History | Semantic History | Recognizes file paths and URLs in output and, on cmd/opt-click, opens them via a configurable action instead of just selecting text. | have | Win-click, since Cmd is Win. |
| Open with default app | n/a (action: best editor) | Opens the clicked file with the app macOS considers its default handler. | have | Opens with the Windows default app; with VS Code on the PATH and a line number in the text, runs code -g file:line:column. |
| Open URL | n/a (action: url) | Opens a URL template, with filename and line-number substitutions, in the default browser. | have |  |
| Open with editor | n/a (action: editor) | Opens the clicked text file in a specific, user-chosen editor app. | have | The editor is a program path; VS Code gets -g file:line:column. |
| Run command | n/a (action: command) | Runs a custom command, with path/line/prefix/suffix/pwd substitutions, when an existing filename is clicked. | have | Runs through cmd.exe, with the values quoted for it. |
| Always run command | n/a (action: raw command) | Runs a custom command on any clicked text, even if it is not a valid existing filename. | have | Runs through cmd.exe. |
| Run coprocess | n/a (action: coprocess) | Runs a coprocess with the same substitutions whenever an existing filename is clicked. | have | Milestone 6c. |
| Send text | n/a (action: send text) | Sends literal or template text to the session whenever an existing filename is clicked. | have |  |

## Totals

Updated after milestone 4c (2026-09-24).

- Rows: 314
- have: 73
- partial: 175
- missing: 66
