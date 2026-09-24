# iTerm2 profile settings

This inventory was built from iTerm2 source in a read-only reference clone (gnachman/iTerm2), primarily sources/Settings/iTermProfilePreferences.m, sources/Settings/Profiles/ITAddressBookMgr.h, and the per-tab ProfilesXPreferencesViewController.m files, plus the trigger, smart selection, and semantic history classes. It lists every user-visible per-profile setting, grouped by the tab it appears under in iTerm2's profile editor.

Tandem is a Windows terminal in Rust aiming for one-to-one parity with iTerm2. As of this writing Tandem has a ConPTY pty, an alacritty_terminal core, a wgpu and glyphon renderer, one hard-coded profile (iTerm2 default colors, Cascadia Mono 12pt, shell pwsh or powershell), tabs, splits with draggable dividers and inactive-pane dimming, xterm key encoding, mouse reporting modes 1000/1002/1003/1006, selection and clipboard behavior, bracketed paste, wheel routing, and 10000 lines of fixed scrollback. Tandem has no settings UI and no profile system yet, so almost every row below is missing as a configurable setting; a few rows are marked partial where Tandem's hard-coded behavior already matches iTerm2's default for that setting.

## General

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Profile name | Name | Names the profile, shown in the profile list and available for the window and tab title. | missing | |
| Profile icon | Icon | Chooses none, an automatic app icon, or a custom image to represent the profile. | missing | |
| Custom icon path | Custom Icon Path | File path to the custom icon image when icon is set to custom. | missing | |
| Profile shortcut | Shortcut | A Cmd+Ctrl+letter or digit keystroke that opens a new window with this profile. | missing | macOS Cmd+Ctrl chord; map to a Windows-equivalent modifier combo. |
| Tags | Tags | Free-text tags used to organize and search profiles. | missing | |
| Profile type | Profile Type (Phony) | Segmented control choosing Terminal or Browser mode for the profile; synthetic, backed by the command type value. | missing | |
| Command type | Custom Command | Chooses what runs at session start: login shell, custom shell, custom command, SSH, or browser. | partial | Tandem always launches pwsh or powershell; not selectable. |
| Command / command line | Command | The literal command, shell path, or SSH arguments to run, depending on command type. | partial | Hard-coded to pwsh/powershell, not editable. |
| Initial URL | Initial URL | The URL a Browser-type profile opens on start. | missing | |
| Send text at start | Initial Text | Text automatically typed into the session as soon as it starts. | missing | |
| Initial working directory | Custom Directory | Chooses Home, reuse previous session's directory, a custom path, or per-window/tab/pane advanced rules. | missing | |
| Custom directory path | Working Directory | The path used when initial working directory is set to custom. | missing | |
| Advanced working directory (window) | AWDS Window Option / AWDS Window Directory | Sets a working-directory rule specific to new windows from this profile. | missing | |
| Advanced working directory (tab) | AWDS Tab Option / AWDS Tab Directory | Sets a working-directory rule specific to new tabs from this profile. | missing | |
| Advanced working directory (pane) | AWDS Pane Option / AWDS Pane Directory | Sets a working-directory rule specific to new split panes from this profile. | missing | |
| Badge text | Badge Text | Overlay text drawn in the corner of the terminal, can reference session variables. | missing | |
| Badge appearance (font, margins, max size) | Badge Font, Badge Top Margin, Badge Right Margin, Badge Max Width, Badge Max Height | Configures the badge's font and its position and size limits via a separate sheet. | missing | |
| Subtitle | Subtitle | A second line of small text shown under the tab title when the tall tab bar is enabled. | missing | |
| Title components | Title Components | Bitmask popup choosing what appears in the window/tab title: session name, job, working directory, tty, custom, profile name, profile+session name, user, host, command line, size, temporary session name, or an AI-generated name. | missing | |
| Custom title function | Title Function | Points the title at a registered scripting-API function instead of the built-in components. | missing | |
| Custom window title | Custom Window Title | Literal override text for the window title when "use custom window title" is on. | missing | |
| Use custom window title | Use Custom Window Title | Enables the custom window title override instead of computed title components. | missing | |
| Custom tab title | Custom Tab Title | Literal override text for the tab title when "use custom tab title" is on. | missing | |
| Use custom tab title | Use Custom Tab Title | Enables the custom tab title override instead of computed title components. | missing | |
| URL schemes handled | n/a (app-level scheme registration keyed to profile GUID) | Assigns URL schemes (ssh, http, https, etc) to open with this profile. | missing | |
| Configure SSH | SSH | Opens a sheet to fill in host, user, port, and auth for the SSH command type. | missing | |
| Load shell integration automatically | Load Shell Integration Automatically | Automatically injects the shell integration script for supported shells (bash, fish, xonsh, zsh) at session start. | missing | |
| Run command in login shell | Run Command In Login Shell | Wraps a custom command so it still runs inside a login shell for dotfile sourcing. | missing | |
| Allow title setting | Allow Title Setting | Permits escape sequences from the running program to change the session/window title; also drives a lock icon when editing the current session. | missing | |
| Prevent Automatic Profile Switching | Prevent Automatic Profile Switching | Once a session's profile is changed by hand, blocks automatic profile switching rules from overriding it again. | missing | |
| Session hotkey | Session Hotkey | A one-off keystroke bound to the current session instance (set from Edit Current Session, not stored on the base profile). | missing | |

## Colors

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Use Separate Colors for Light and Dark Mode | Use Separate Colors for Light and Dark Mode | Splits most other colors and sliders on this tab into independent Light and Dark variants, applied automatically with the system appearance. | missing | |
| Light/Dark mode editing selector | n/a | Popup that picks which appearance variant you are editing right now; tracks system appearance, not itself a saved value. | missing | |
| Theme overrides mode warning | n/a | Warning shown when the active tab theme forces a fixed appearance so automatic light/dark switching will not occur. | missing | |
| Foreground Color | Foreground Color | Default text color (has separate Light/Dark variant when enabled). | partial | Hard-coded to the iTerm2 default preset, not editable. |
| Background Color | Background Color | Default terminal background color (has separate Light/Dark variant when enabled). | partial | Hard-coded to the iTerm2 default preset, not editable. |
| Use custom color for bold text | Use Bright Bold | Enables the Bold Color swatch; also makes bold text in a normal ANSI color use that color's bright variant. | missing | |
| Bold Color | Bold Color | Color used for bold text when the custom bold color option is on. | missing | |
| Use bright version of ANSI colors for bold text | Brighten Bold Text | Brightens text drawn in a normal ANSI color when it is also bold. | missing | |
| Link Color | Link Color | Color for clickable hyperlink and URL text. | missing | |
| Match Color | Match Background Color | Background used to highlight Find, Smart Selection, and Semantic History matches. | missing | |
| Selection Color | Selection Color | Background color of selected text. | partial | Hard-coded to the default preset value, not editable. |
| Use custom color for selected text | Use Selected Text Color | Enables a custom color for text drawn over a selection instead of keeping its normal foreground color. | missing | |
| Selected Text Color | Selected Text Color | Text color drawn over a selection, when enabled. | missing | |
| Cursor Color | Cursor Color | Color of the text cursor; disabled when Smart Cursor Color is on with a box cursor. | partial | Hard-coded default cursor color, not editable. |
| Cursor Text Color | Cursor Text Color | Color of text drawn under or over the cursor. | missing | |
| Smart Cursor Color | Smart Cursor Color | Computes a contrasting cursor color automatically for a box cursor instead of using fixed swatches. | missing | |
| IME Cursor Color | IME Cursor Color | Cursor color shown while composing text through an Input Method Editor. | missing | Windows also has IME; concept still applies. |
| Badge Color | Badge Color | Color of the badge text overlay drawn in the corner of the terminal. | missing | |
| Use custom tab color | Use Tab Color | Enables a custom color for the session's tab. | missing | |
| Tab Color | Tab Color | Color used to color the session's tab, when enabled. | missing | |
| Use custom underline color | Use Underline Color | Enables a custom color for underlines instead of matching the underlined text's own color. | missing | |
| Underline Color | Underline Color | Color of underlines under underlined text, when enabled. | missing | |
| Border around active pane | Use Active Pane Border | Draws a custom-colored border around the currently focused split pane. | missing | Tandem dims inactive panes instead of bordering the active one; different mechanism. |
| Active Pane Border Color | Active Pane Border Color | Color of the focused split-pane border, when enabled. | missing | |
| Use cursor guide | Use Cursor Guide | Draws a highlighted horizontal guide across the row containing the text cursor. | missing | |
| Cursor Guide Color | Cursor Guide Color | Color and opacity of the cursor guide row highlight. | missing | |
| Use ANSI colors for mark indicators | Use Theme Mark Colors | Makes command-status marks in the left margin use the ANSI palette instead of fixed theme colors. | missing | |
| Harmonize 256 Colors | Harmonize 256 Colors | Chooses the extended 256-color palette to visually match the 8 normal ANSI colors instead of the standard RGB cube. | missing | |
| Use HDR cursor when available | HDR Cursor | On HDR-capable displays, draws the cursor at maximum HDR brightness white. | missing | Windows also supports HDR displays; not macOS-specific. |
| Ansi 0 Color (Black) | Ansi 0 Color | ANSI palette slot 0. | partial | Hard-coded default palette, not editable. |
| Ansi 1 Color (Red) | Ansi 1 Color | ANSI palette slot 1. | partial | Hard-coded default palette, not editable. |
| Ansi 2 Color (Green) | Ansi 2 Color | ANSI palette slot 2. | partial | Hard-coded default palette, not editable. |
| Ansi 3 Color (Yellow) | Ansi 3 Color | ANSI palette slot 3. | partial | Hard-coded default palette, not editable. |
| Ansi 4 Color (Blue) | Ansi 4 Color | ANSI palette slot 4. | partial | Hard-coded default palette, not editable. |
| Ansi 5 Color (Magenta) | Ansi 5 Color | ANSI palette slot 5. | partial | Hard-coded default palette, not editable. |
| Ansi 6 Color (Cyan) | Ansi 6 Color | ANSI palette slot 6. | partial | Hard-coded default palette, not editable. |
| Ansi 7 Color (White) | Ansi 7 Color | ANSI palette slot 7. | partial | Hard-coded default palette, not editable. |
| Ansi 8 Color (Bright Black) | Ansi 8 Color | Bright variant of Black. | partial | Hard-coded default palette, not editable. |
| Ansi 9 Color (Bright Red) | Ansi 9 Color | Bright variant of Red. | partial | Hard-coded default palette, not editable. |
| Ansi 10 Color (Bright Green) | Ansi 10 Color | Bright variant of Green. | partial | Hard-coded default palette, not editable. |
| Ansi 11 Color (Bright Yellow) | Ansi 11 Color | Bright variant of Yellow. | partial | Hard-coded default palette, not editable. |
| Ansi 12 Color (Bright Blue) | Ansi 12 Color | Bright variant of Blue. | partial | Hard-coded default palette, not editable. |
| Ansi 13 Color (Bright Magenta) | Ansi 13 Color | Bright variant of Magenta. | partial | Hard-coded default palette, not editable. |
| Ansi 14 Color (Bright Cyan) | Ansi 14 Color | Bright variant of Cyan. | partial | Hard-coded default palette, not editable. |
| Ansi 15 Color (Bright White) | Ansi 15 Color | Bright variant of White. | partial | Hard-coded default palette, not editable. |
| Minimum Contrast | Minimum Contrast | Forces a minimum contrast ratio between text and background, shifting colors toward black or white as needed. | missing | |
| Excessive contrast warning | n/a | Warning shown when Minimum Contrast is set so high most colors render as plain black and white. | missing | |
| Faint Text Alpha | Faint Text Alpha | Opacity used to render faint or dim styled text. | missing | |
| Cursor Boost | Cursor Boost | Increases the cursor color's saturation relative to surrounding text so it stands out. | missing | |
| Color Presets menu | n/a | Lists built-in and custom named color schemes; clicking one applies all its color keys to the profile. | missing | |
| Import preset | n/a | Imports one or more .itermcolors files into the custom presets list. | missing | |
| Export preset | n/a | Saves the profile's current colors to a .itermcolors file. | missing | |
| Save preset as | n/a | Saves the current colors as a new named custom preset. | missing | |
| Delete preset | n/a | Deletes a chosen custom preset; built-in presets cannot be deleted. | missing | |
| Visit Online Gallery | n/a | Opens the iterm2.com color gallery web page. | missing | |
| Edit/remove color binding | n/a | Binds a color swatch to a dynamic expression (session variable or function call) instead of a fixed color. | missing | |
| Hue/Chroma visualization wheel | n/a | Graphical wheel plotting the six chromatic ANSI color pairs by hue and chroma for drag-to-edit color picking. | missing | |

## Text

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Normal (ASCII) font | Normal Font | Font family, weight, and size for ASCII text, and for all text when no non-ASCII font is set. | partial | Hard-coded to Cascadia Mono 12pt, not editable. |
| Non-ASCII font | Non Ascii Font | Font family and size for characters above code point 127, when enabled. | missing | |
| Use a different font for non-ASCII text | Use Non-ASCII Font | Turns on the separate non-ASCII font, antialiasing, ligature, and spacing controls. | missing | |
| Manage Special Exceptions | Special Font Config | Assigns additional per-Unicode-range font overrides beyond the normal and non-ASCII font pair. | missing | |
| Horizontal Spacing | Horizontal Spacing | Stretches or compresses the character cell width, 50 to 200 percent. | missing | |
| Vertical Spacing | Vertical Spacing | Stretches or compresses the line height, 50 to 200 percent. | missing | |
| Cursor style | Cursor Type | Chooses underline, vertical bar, or filled box cursor shape. | partial | Renderer draws a fixed cursor shape, not selectable. |
| Blinking cursor | Blinking Cursor | Makes the text cursor blink. | missing | |
| Cursor shadow | Cursor Shadow | Draws a soft drop shadow under the cursor; only for underline and bar styles. | missing | |
| Hide cursor when unfocused | Cursor Hidden When Unfocused | Hides the cursor when the window or pane is not focused. | missing | |
| Cursor smooth slide | Cursor Smooth Slide | Animates small cursor movements with an eased slide instead of an instant jump. | missing | |
| Cursor smooth blink | Cursor Smooth Blink | Replaces abrupt on/off blinking with an eased fade cycle. | missing | |
| Smooth blink fade in duration | Cursor Blink Fade In Duration | Seconds the smooth-blink fade from hidden to visible takes. | missing | |
| Smooth blink fade out duration | Cursor Blink Fade Out Duration | Seconds the smooth-blink fade from visible to hidden takes. | missing | |
| Smooth blink fade in curve | Cursor Blink Fade In Curve | Easing applied to the fade in: linear, ease in, ease out, or ease in-out. | missing | |
| Smooth blink fade out curve | Cursor Blink Fade Out Curve | Easing applied to the fade out, same four options. | missing | |
| Smooth blink visible dwell | Cursor Blink Visible Dwell | Seconds the cursor holds fully visible between fades. | missing | |
| Smooth blink hidden dwell | Cursor Blink Hidden Dwell | Seconds the cursor holds fully hidden between fades. | missing | |
| Smooth blink presets | n/a | One-click presets (Breathing, Linear, Fast, Slow, Subtle) for the six smooth-blink values. | missing | |
| Allow blinking text | Blink Allowed | Permits text carrying the terminal blink attribute to actually blink. | missing | |
| Animate large cursor movements | Animate Movement | Plays an animation when the cursor makes a large jump. | missing | |
| Animate movement only in interactive apps | Animate Movement Only in Interactive Apps | Restricts large-movement animation to alternate-screen apps. | missing | |
| Draw bold text in bold font | Use Bold Font | Renders bold-attribute text with the font's real bold variant instead of faux weight. | missing | |
| Allow italic text | Use Italic Font | Permits rendering italic-attribute text in an italic font variant. | missing | |
| Thin Strokes | Thin Strokes | Chooses when to render glyphs with thinner strokes: never, always, on Retina, with dark backgrounds, or both. | missing | |
| Anti-aliased (ASCII) | ASCII Anti Aliased | Controls whether the normal font is anti-aliased. | partial | glyphon renders anti-aliased glyph textures by default, not a toggle. |
| Anti-aliased (non-ASCII) | Non-ASCII Anti Aliased | Controls whether the non-ASCII font is anti-aliased. | missing | |
| Use ligatures (ASCII) | ASCII Ligatures | Enables optional typographic ligatures for the normal font. | missing | |
| Use ligatures (non-ASCII) | Non-ASCII Ligatures | Enables optional typographic ligatures for the non-ASCII font. | missing | |
| Use built-in Powerline glyphs | Draw Powerline Glyphs | Draws Powerline symbol glyphs with iTerm2's own renderer instead of relying on the font. | missing | |
| Ambiguous characters are double-width | Ambiguous Double Width | Treats Unicode ambiguous-width characters as occupying two cells instead of one. | partial | alacritty_terminal applies a fixed unicode-width table; not adjustable. |
| Unicode Normalization Form | Unicode Normalization | Chooses how incoming Unicode text is normalized before rendering: none, NFC, NFD, or HFS+. | missing | |
| Use Unicode version 9+ widths | Unicode Version | Toggles the character-width table between Unicode 8 and Unicode 9+, which narrowed most emoji from 2 cells to 1. | partial | alacritty_terminal uses one fixed width table version, not selectable. |

## Window

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Columns (initial width) | Columns | Initial window width in character columns. | missing | |
| Rows (initial height) | Rows | Initial window height in character rows. | missing | |
| Width, % of screen | Width Percentage | Initial window width as a percentage of screen width instead of a fixed column count. | missing | |
| Height, % of screen | Height Percentage | Initial window height as a percentage of screen height instead of a fixed row count. | missing | |
| Columns/Rows units popup | n/a | Chooses whether size is given in cells or percent of screen; hidden for styles with no variable size. | missing | |
| Width, points (Browser profile) | Width in Points | Initial window width in points for the Browser profile type. | missing | Not applicable; Tandem has no browser profile type. |
| Height, points (Browser profile) | Height in Points | Initial window height in points for the Browser profile type. | missing | Not applicable; Tandem has no browser profile type. |
| Window style: Normal | Window Type = 0 | Ordinary resizable titled window. | missing | Maps directly to a normal Windows window. |
| Window style: Full Screen | Window Type = 1 | Opens directly into fullscreen, hiding the menu bar. | missing | Maps to Windows fullscreen/kiosk mode. |
| Window style: Lion Full Screen | Window Type = 4 | Runtime variant using native macOS Spaces-based fullscreen; not directly selectable. | missing | macOS-only concept (Spaces); no Windows analogue. |
| Window style: Top of Screen | Window Type = 2 or 9 | Window docked to the top edge of the screen. | missing | Maps to a Windows top-edge-snapped window. |
| Window style: Bottom of Screen | Window Type = 5 or 8 | Window docked to the bottom edge of the screen. | missing | Maps to a Windows bottom-edge-docked window, e.g. a Quake-style dropdown. |
| Window style: Left of Screen | Window Type = 6 or 10 | Window docked to the left edge of the screen. | missing | Maps to a Windows left-edge-snapped window. |
| Window style: Right of Screen | Window Type = 7 or 11 | Window docked to the right edge of the screen. | missing | Maps to a Windows right-edge-snapped window. |
| Window style: No Title Bar | Window Type = 12 | Resizable window with no title bar or chrome. | missing | Maps to a Windows borderless window. |
| Window style: Compact | Window Type = 13 | Theme-derived compact-chrome variant of Normal, substituted automatically under the Compact tab style. | missing | Cosmetic macOS chrome variant; closest Windows concept is a slim title bar. |
| Window style: Accessory | Window Type = 14 | Floating panel-like window, reachable only via scripting API or command line, not the popup. | missing | macOS-panel concept; closest Windows equivalent is a topmost tool window. |
| Window style: Maximized | Window Type = 15 | Window opens filling the available screen area, keeping title bar visible. | missing | Maps directly to Windows maximized window state. |
| Window style: Compact, Maximized | Window Type = 16 | Theme-derived compact-chrome equivalent of Maximized. | missing | Cosmetic macOS chrome variant. |
| Window style: Centered | Window Type = 17 | Window opens centered on screen at the given size. | missing | Maps directly to centering a Windows window on open. |
| Window style: Compact, Centered | Window Type = 18 | Theme-derived compact-chrome equivalent of Centered. | missing | Cosmetic macOS chrome variant. |
| Window style: Centered, No Title Bar | Window Type = 19 | Combines centered placement with no title bar in every theme. | missing | Maps to a centered borderless Windows window. |
| Screen | Screen | Picks which physical display a new window opens on. | missing | Maps directly to Windows multi-monitor placement. |
| Space | Space | Picks which macOS Space (virtual desktop) a new window opens on: current, all, or a specific numbered space. | missing | macOS-only concept (Spaces); Windows virtual desktops have no per-app auto-assign setting. |
| Hide after opening | Hide After Opening | New windows are immediately hidden after opening. | missing | |
| Lock window size automatically | Lock Window Size Automatically | Prevents the window from being resized by escape sequences or the user once opened. | missing | |
| Always open in a new window, never a tab | Prevent Opening in a Tab | Forces this profile to always create a new window instead of a new tab. | missing | |
| Open toolbelt | Open Toolbelt | New windows open with the Toolbelt sidebar panel visible. | missing | Tandem has no toolbelt feature at all yet. |
| Use custom window title | Use Custom Window Title | Enables overriding the window title bar text with a custom string. | missing | |
| Custom window title | Custom Window Title | The interpolated-string template used as the window title, when enabled. | missing | |
| Use custom tab title | Use Custom Tab Title | Enables overriding the tab title text with a custom string. | missing | |
| Custom tab title | Custom Tab Title | The interpolated-string template used as the tab title, when enabled. | missing | |
| Use transparency | Initial Use Transparency | Master on/off switch for whether the window starts with background transparency. | missing | |
| Transparency | Transparency | Slider setting how transparent the background is. | missing | |
| Keep background colors opaque | Only The Default BG Color Uses Transparency | Restricts transparency to the default background color only, leaving non-default backgrounds opaque. | missing | |
| Blur | Blur | Enables a Gaussian-style blur of content behind the window, visible through transparent regions. | missing | |
| Blur Radius | Blur Radius | Sets the strength of the background blur effect. | missing | |
| Use background image | n/a | Toggles whether a background image is shown at all. | missing | |
| Background image source mode | Background Image Source Mode | Chooses a single fixed image versus rotating automatically through a folder. | missing | |
| Background image (path) | Background Image Location | Path to the single background image file. | missing | |
| Background image folder (path) | Background Image Folder Location | Path to a folder of images to rotate through. | missing | |
| Background image rotation interval | Background Image Folder Interval | Seconds between automatic image changes in folder rotation mode. | missing | |
| Background image scaling mode | Background Image Mode | Chooses stretch, tile, scale to fill, or scale to fit for the background image. | missing | |
| Blending | Blend | Sets how much of the background color shows through over the background image. | missing | |

## Terminal

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Scrollback Lines | Scrollback Lines | Number of scrollback lines to keep, 0 up to 10,000,000. | partial | Hard-coded to 10000 lines, not configurable. |
| Unlimited Scrollback | Unlimited Scrollback | Removes the scrollback line cap entirely. | missing | |
| Save lines to scrollback with status bar present | Scrollback With Status Bar | Controls whether lines are added to scrollback while a status bar (e.g. tmux) is shown. | missing | |
| Save lines to scrollback in alternate screen | Scrollback in Alternate Screen | Controls whether output is added to scrollback while the alternate screen buffer is active. | missing | |
| Drag to scroll disabled in alternate screen | Drag to Scroll in Alternate Screen Mode Disabled | Disables click-and-drag scrolling while the alternate screen is active. | missing | |
| Automatically enable alternate mouse scroll | Automatically Enable Alternate Mouse Scroll | Automatically turns on translating scroll wheel input into arrow keys when an app enables the alternate screen. | have | Tandem already routes wheel to arrow keys in the alt screen. |
| Restrict alternate mouse scroll to vertical | Restrict Alternate Mouse Scroll to Vertical | Limits the alt-screen scroll-to-arrow translation to vertical scrolling only. | have | Mouse wheel input Tandem handles is vertical only. |
| Applications may use alternate mouse scroll | Allow Alternate Mouse Scroll | Permits the alt-screen scroll-to-arrow-key feature to be used at all. | have | Always on in Tandem; no toggle to disable it. |
| Character Encoding | Character Encoding | Text encoding used to interpret the terminal's byte stream. | partial | Hard-coded to UTF-8, not selectable. |
| Terminal Type (report as) | Terminal Type | The TERM value the terminal reports to programs, e.g. xterm-256color. | partial | alacritty_terminal reports a fixed TERM value, not editable per profile. |
| Answerback String | Answerback String | String sent in response to an ENQ terminal-identification query. | missing | |
| Report Mouse Clicks and Drags (Mouse Reporting) | Mouse Reporting | Enables xterm-style mouse reporting to the running program. | have | Tandem implements mouse reporting modes 1000/1002/1003/1006, always on. |
| Report Mouse Wheel Events | Mouse Reporting allow mouse wheel | Lets scroll wheel events be reported to the terminal program. | have | |
| Report Mouse Clicks and Drags | Mouse Reporting allow clicks and drags | Lets click and drag events be reported to the terminal program. | have | Tandem supports drag/double/triple/block selection alongside reporting modes 1002/1003. |
| Restrict mouse reporting to alternate screen | Restrict Mouse Reporting to Alternate Screen Mode | Limits xterm mouse reporting so it only applies while the alternate screen is active. | missing | Tandem's reporting is always active, not restricted by screen mode. |
| Terminal may report its title | Allow Title Reporting | Allows programs to query the current window or tab title via escape sequence. | missing | |
| Applications may use paste bracketing | Allow Paste Bracketing | Allows the terminal to wrap pasted text in bracketed-paste escape sequences for apps that request it. | have | Tandem already implements bracketed paste. |
| Disable Printing | Disable Printing | Blocks the terminal from honoring escape-sequence print requests. | missing | Tandem has no print support either way. |
| Disable Alternate Screen (smcup/rmcup) | Disable Smcup Rmcup | Ignores smcup/rmcup so full-screen apps cannot switch to the alternate screen. | missing | alacritty_terminal supports the alt screen itself; there is no toggle to block it. |
| Terminal may resize window | Disable Window Resizing | Controls whether escape sequences may resize the terminal window. | missing | |
| Terminal may resize window when not focused | Disable Window Resizing by Unfocused Sessions | Controls whether an unfocused window can still be resized by escape sequences. | missing | |
| Programs can change cursor blink | Allow Change Cursor Blink | Allows escape sequences to toggle whether the cursor blinks. | missing | |
| Silence Bell | Silence Bell | Mutes the audible bell sound. | missing | Tandem has no bell implementation yet. |
| Send Notification (master toggle) | BM Growl | Enables user notifications for terminal events. | missing | |
| Bell (alert filter) | Send Bell Alert | Whether a bell event triggers a notification. | missing | |
| Idle (alert filter) | Send Idle Alert | Whether the session going idle triggers a notification. | missing | |
| New Output (alert filter) | Send New Output Alert | Whether new output after idle triggers a notification. | missing | |
| Session Ended (alert filter) | Send Session Ended Alert | Whether the session ending triggers a notification. | missing | |
| Terminal-Generated Alerts (alert filter) | Send Terminal Generated Alerts | Whether escape-sequence-generated alerts trigger a notification. | missing | |
| Suppress Alerts in Active Session | Suppress Alerts in Active Session | Suppresses notifications for the session currently focused. | missing | |
| Also Send to Companion | Send Alerts To Companion | Also delivers alerts to a paired companion iPhone app. | missing | macOS/iOS-only concept. |
| Flashing Bell | Flashing Bell | Flashes the screen when the bell rings. | missing | |
| Visual Bell | Visual Bell | Enables a visual indicator for the bell. | missing | |
| Set Locale Variables | Set Local Environment Vars | Chooses how $LANG/$LC_CTYPE are set for new sessions: do not set, automatic, custom, or minimal. | missing | |
| Change Locale | Custom Locale | Opens a locale picker for the custom locale mode. | missing | |
| Place prompt at first column | Place Prompt at First Column | Forces the shell prompt to always be placed at column 1. | missing | Requires shell integration, which Tandem does not have. |
| Show Mark Indicators | Show Mark Indicators | Shows indicator marks in the left margin for shell-integration command marks. | missing | Requires shell integration. |
| Cmd-click on path opens navigator | Prompt Path Click Opens Navigator | Makes a click on a shell-integration-reported path open it in the file navigator. | missing | Requires shell integration; also a macOS Cmd-click convention. |
| Show Offscreen Command Line | Show Offscreen Command line | Shows a floating bar with the current prompt when it has scrolled off the top. | missing | Requires shell integration. |
| Show offscreen command line for current command | Show Offscreen Command line for Current Command | Extends the floating bar to show the running command, not just the prompt. | missing | Requires shell integration. |
| tmux uses newline (vs CRLF) | Tmux Newline | Controls whether tmux integration sends newline or CRLF line endings. | missing | Tandem has no tmux integration. |
| Filter Alerts | n/a | Opens the sheet containing the per-event alert filter checkboxes. | missing | |

## Session

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Close or restart session on end | Close Sessions On End | What happens when the shell process exits: default, close, or restart. | partial | Tandem closes the tab/pane when the process exits, matching the default; close/restart are not selectable. |
| Warn about short-lived sessions | n/a | Warns when a session that ended very quickly is about to close automatically. | missing | |
| Prompt before closing | Prompt Before Closing 2 | Never prompt, always prompt, or prompt unless only ignorable jobs are running. | missing | |
| Jobs to Ignore | Jobs to Ignore | Job or process names that do not count as "still running" for the prompt-before-closing check. | missing | |
| Session Close Undo Timeout | Session Close Undo Timeout | Seconds available to undo (reopen) a closed session. | missing | |
| Automatically Log | Automatically Log | Enables automatic session logging to disk. | missing | |
| Log Directory | Log Directory | Directory session logs are written to. | missing | |
| Log Filename Format | Log Filename Format | Filename template for the log file, supporting variable substitution. | missing | |
| Logging style | Plain Text Logging | Chooses raw, plain text, HTML, or asciicast logging. | missing | |
| Archive On Closure | Archive On Closure | Enables automatic archiving of the session's content when it closes. | missing | |
| Archive Directory | Archive Directory | Directory closed-session archives are written to. | missing | |
| Send code when idle | Send Code When Idle | Anti-idle/keepalive checkbox that periodically sends an ASCII code while the session is idle. | missing | |
| Idle code | Idle Code | ASCII code value, 0 to 255, sent periodically while idle. | missing | |
| Idle period | Idle Period | Seconds between sending the idle code. | missing | |
| Reduce Flicker | Reduce Flicker | Enables extra frame synchronization to reduce visual flicker during redraws. | missing | |
| Prevent Sleep | Prevent Sleep | Keeps the system from idle-sleeping while a session using this profile exists. | missing | |
| Default Pane Locked | Default Pane Locked | Whether new split panes from this profile start locked against closing or replacement. | missing | |
| Buffer Input by Default | Buffer Input by Default | Whether keyboard input is buffered by default in new sessions from this profile. | missing | |
| Show Status Bar | Show Status Bar | Enables the per-session status bar. | missing | |
| Configure Status Bar | Status Bar Layout | Opens a sheet to arrange status bar components such as clock, CPU, network, memory, working directory, and git branch. | missing | |
| Open Password Manager Automatically | Open Password Manager Automatically | Automatically opens the password manager when a new session starts. | missing | Tandem has no password manager. |
| Timestamps Style | Timestamps Style | How per-line timestamps display when shown: overlap, hover, or adjacent. | missing | |
| Timestamps Visible | Timestamps Visible | Whether per-line timestamps are shown at all. | missing | |
| Enable Progress Bars | Enable Progress Bars | Enables OSC 9;4 style progress-bar indicators in the session. | missing | |
| Progress Bar Height | Progress Bar Height | Height of the progress bar in points. | missing | |
| Progress Bar Color Scheme | Progress Bar Color Scheme | Color scheme for the progress bar: default, rainbow, red, green, blue, yellow, purple, cyan, or orange. | missing | |

## Keys

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Option Key Sends (Left) | Option Key Sends | Chooses what the left Option key sends: Normal, Meta, or Esc+. | partial | Tandem hard-codes Alt to Esc+ behavior; not configurable, no left/right distinction. |
| Option Key Sends (Right) | Right Option Key Sends | Same popup for the right-hand Option key, letting the two sides differ. | missing | macOS-only concept; Windows AltGr is a related but distinct idea. |
| Left Option Key Independently Changeable | Left Option Key Changeable | Unlocks the left Option key's own dropdown instead of mirroring the right. | missing | macOS-only concept. |
| Right Option Key Independently Changeable | Right Option Key Changeable | Unlocks the right Option key's own dropdown instead of mirroring the left. | missing | macOS-only concept. |
| Application Keypad Allowed | Application Keypad Allowed | Permits apps to switch the numeric keypad into application mode via escape sequences. | have | alacritty_terminal implements DECPAM/DECPNM as part of standard VT emulation, always on. |
| Allow modifyOtherKeys | Allow modifyOtherKeys | Enables the xterm modifyOtherKeys protocol for reporting more modifier key combinations. | missing | |
| Movement keys scroll outside interactive apps | Movement Keys Scroll Outside Interactive Apps | Lets arrow/movement keys scroll the buffer instead of being sent to the shell outside interactive apps. | missing | |
| Treat Option as Alt | Treat Option as Alt | Chooses whether Esc+ mode reports the key as Alt versus older Meta-style behavior. | missing | Folded into Tandem's fixed Alt-as-Esc+ behavior; no separate toggle. |
| Use libtickit protocol (CSI u) | Use libtickit protocol | Enables the CSI u keyboard protocol for unambiguous key reporting. | missing | |
| Left Control Key | Left Control Key | Remaps the left Control key to a bucky bit: regular, Hyper, Meta, or Super. | missing | |
| Right Control Key | Right Control Key | Remaps the right Control key independently to a bucky bit. | missing | |
| Left Command Key | Left Command Key | Remaps the left Command key to a bucky bit. | missing | macOS-only concept (Command key); nearest Windows key is the left Windows key. |
| Right Command Key | Right Command Key | Remaps the right Command key independently to a bucky bit. | missing | macOS-only concept (Command key). |
| Function Key | Function Key | Remaps the hardware Fn key to a bucky bit. | missing | macOS-only concept (Fn key); most Windows keyboards lack a reportable Fn key. |
| Delete sends Ctrl-H | n/a (a Keyboard Map entry) | Makes Delete/Backspace send Control-H instead of the default DEL byte. | missing | |
| Key Mappings table | Keyboard Map | Per-profile table of custom keystroke-to-action bindings. | missing | Tandem's xterm key encoding is a single fixed mapping, not user-editable. |
| Key Mapping Presets | n/a | Loads a built-in preset (xterm-style, Natural, Terminal.app compatibility, Numeric Keypad) or imports/exports bindings. | missing | |
| Repair Key Mappings | n/a | Repairs a profile's key mappings if they are detected as corrupted. | missing | |
| Has Hotkey (global hotkey window) | Has Hotkey | Turns this profile into a global Quake-style dropdown terminal window. | missing | |
| Configure Hotkey Window | n/a | Opens the sheet for setting the hotkey shortcut and window behavior. | missing | |
| Hotkey shortcut | HotKey Key Code, HotKey Characters, HotKey Characters Ignoring Modifiers, HotKey Modifier Flags | Records the global keystroke that toggles the hotkey window. | missing | |
| Hotkey window autohide | HotKey Window AutoHides | Hides the hotkey window automatically when it loses focus. | missing | |
| Reopen hotkey window on activation | HotKey Window Reopens On Activation | Reshows a previously autohidden hotkey window when the app is activated. | missing | |
| Animate hotkey window | HotKey Window Animates | Enables a slide/fade animation when the hotkey window shows or hides. | missing | |
| Hotkey window floats | HotKey Window Floats | Keeps the hotkey window always on top of other windows. | missing | Maps to a Windows topmost window flag; not macOS-exclusive. |
| Hotkey window dock click action | HotKey Window Dock Click Action | Chooses what clicking the Dock icon does regarding the hotkey window. | missing | macOS-only concept (Dock); nearest Windows equivalent is a taskbar icon click. |
| Activate hotkey with modifier alone | HotKey Activated By Modifier, HotKey Modifier Activation | Lets a bare modifier tap, instead of a full combo, toggle the hotkey window. | missing | Option/Command choices are macOS-only concepts. |
| Alternate hotkey shortcuts | HotKey Alternate Shortcuts | Additional keystrokes that also trigger the same hotkey window. | missing | |
| Touch Bar item list | Touch Bar Map | Per-key mapping table for Touch Bar buttons. | missing | macOS-only, hardware-specific; Touch Bar no longer exists on current Macs. |

## Advanced

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Automatic Profile Switching rules | Bound Hosts | Hostname, username, or path rules that auto-switch to this profile when the remote host matches, via shell integration. | missing | Requires shell integration. |
| Snippets Filter | Snippets Filter | Filters which of the app's global Snippets appear in this profile's Snippets menu. | missing | |
| Enable triggers in interactive apps | Enable Triggers in Interactive Apps | Lets triggers keep firing while a full-screen interactive program is running. | missing | |
| Triggers use interpolated strings | Triggers Use Interpolated Strings | Makes trigger parameter fields evaluate as interpolated-string expressions instead of literal backreferences. | missing | |
| Smart Selection actions use interpolated strings | Smart Selection Actions Use Interpolated Strings | Makes smart-selection action parameters evaluate as interpolated-string expressions. | missing | |
| Edit Triggers | n/a | Opens the Triggers editor for this profile's trigger list. | missing | |
| Edit Smart Selection | n/a | Opens the Smart Selection editor for this profile's rule list. | missing | |
| Edit Semantic History | Semantic History | Opens the Semantic History configuration for cmd/opt-click behavior on paths and URLs. | missing | |
| Automatic Profile Switching debug logging | n/a | Logs why and when automatic profile switching rules match, for debugging. | missing | |

## Triggers

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Trigger list | Triggers | Ordered list of regex-based rules that watch output and perform an action when matched. | missing | |
| Regex / match pattern | n/a (per-row field) | The regular expression a trigger tests each new line or screen content against. | missing | |
| Parameter | n/a (per-row field) | The action-specific argument for the chosen trigger action. | missing | |
| Enabled checkbox | n/a (per-row field) | Turns an individual trigger on or off without deleting it. | missing | |
| Instant (partial line) flag | n/a (per-row field) | Lets a trigger fire on a partial, not yet newline-terminated, line. | missing | |
| Show Alert | n/a (action) | Shows a modal alert dialog containing the matched text. | missing | |
| Annotate | n/a (action) | Attaches an inline annotation to the matching text. | missing | |
| Ring Bell | n/a (action) | Rings the terminal bell when the pattern matches. | missing | |
| Bounce Dock Icon | n/a (action) | Bounces the app's Dock icon to get attention. | missing | macOS-only concept (Dock); nearest Windows equivalent is taskbar flashing. |
| Capture Output | n/a (action) | Captures the matching line into the Captured Output tool for later re-run. | missing | |
| Run Coprocess | n/a (action) | Launches an external command fed the session's I/O, shown in the session. | missing | |
| Run Silent Coprocess | n/a (action) | Same as Run Coprocess but its output is not displayed. | missing | |
| Highlight Text | n/a (action) | Recolors just the matched text. | missing | |
| Highlight Line | n/a (action) | Recolors the entire line containing the match. | missing | |
| Make Hyperlink | n/a (action) | Turns the matched text into a clickable hyperlink built from the match. | missing | |
| Invoke Script Function | n/a (action) | Invokes a registered scripting API function, passing the match as arguments. | missing | |
| Set Title | n/a (action) | Sets the session title to a value derived from the match. | missing | |
| Prompt Detected | n/a (action) | Marks the matching line as a shell prompt for prompt-based navigation. | missing | Requires shell integration. |
| Post Notification | n/a (action) | Posts a system notification containing the matched text. | missing | |
| Set Mark | n/a (action) | Drops a mark at the matching line for later jump-to-mark navigation. | missing | |
| Open Password Manager | n/a (action) | Opens the password manager, optionally filtered to an account name. | missing | Tandem has no password manager. |
| Run Command | n/a (action) | Runs a shell command, not shown in the session, when the pattern matches. | missing | |
| Send Text | n/a (action) | Sends literal text to the session as if typed. | missing | |
| Report Directory | n/a (action) | Tells the terminal the current working directory, emulating shell-integration reporting. | missing | Requires shell integration. |
| Report User and Host | n/a (action) | Tells the terminal the current user and hostname, feeding Automatic Profile Switching. | missing | Requires shell integration. |
| Stop Processing Triggers | n/a (action) | Halts evaluation of further triggers on the current line. | missing | |

## Smart Selection

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Smart Selection rule list | Smart Selection Rules | Ordered regex rules that classify what a double or triple click selects, each with a precision level and actions. | missing | |
| Built-in rules | n/a | Ships default regexes for a whitespace-bounded word, C++ namespace::identifier, file paths, quoted strings, dotted Java/Python paths, mailto/http/https/ssh/telnet URLs, Obj-C selectors, and email addresses. | missing | |
| Open File | n/a (action) | Opens the selected text as a file path with its default or resolved application. | missing | |
| Open URL | n/a (action) | Opens the selected text as a URL in the browser. | missing | |
| Run Command | n/a (action) | Runs a shell command built from the selected text without opening a window. | missing | |
| Run Coprocess | n/a (action) | Runs a coprocess fed by or feeding the session, built from the selected text. | missing | |
| Send Text | n/a (action) | Types the selected text, or a template built from it, back into the session. | missing | |
| Run Command in Window | n/a (action) | Runs a shell command built from the selected text and shows its output in a new window. | missing | |
| Copy | n/a (action) | Copies the selected or matched text to the clipboard. | missing | Tandem already supports copy-on-select for plain selections, but not as a Smart Selection rule action. |

## Semantic History

| Setting (UI name) | Key (plist key) | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Semantic History | Semantic History | Recognizes file paths and URLs in output and, on cmd/opt-click, opens them via a configurable action instead of just selecting text. | missing | |
| Open with default app | n/a (action: best editor) | Opens the clicked file with the app macOS considers its default handler. | missing | |
| Open URL | n/a (action: url) | Opens a URL template, with filename and line-number substitutions, in the default browser. | missing | |
| Open with editor | n/a (action: editor) | Opens the clicked text file in a specific, user-chosen editor app. | missing | |
| Run command | n/a (action: command) | Runs a custom command, with path/line/prefix/suffix/pwd substitutions, when an existing filename is clicked. | missing | |
| Always run command | n/a (action: raw command) | Runs a custom command on any clicked text, even if it is not a valid existing filename. | missing | |
| Run coprocess | n/a (action: coprocess) | Runs a coprocess with the same substitutions whenever an existing filename is clicked. | missing | |
| Send text | n/a (action: send text) | Sends literal or template text to the session whenever an existing filename is clicked. | missing | |

## Totals

Rows: 314.

By status: have 8, partial 32, missing 274.
