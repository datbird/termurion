# iTerm2 features and menus

Sources used: `/home/tbird/gitrepos/iterm2-ref/sources/MainMenu/Base.lproj/MainMenu.xib` for every menu command, and `sources/VT100/VT100Terminal.m` plus related parser files (`VT100SixelParser.m`, `InlineImages/KittyImageCommand.swift`, `Keyboard/iTermModernKeyMapper.swift`, `DragAndDrop/KittyDnD*.swift`) for escape sequences. Subsystem behavior comes from reading the matching source files directly: `TmuxController.m`, `TmuxGateway.m`, the shell integration scripts and utilities, `iTermStatusBar*Component*`, `Tool*.m`/`Tool*.swift`, the trigger action classes, `PasswordManager*`, `Composer*`, `FindViewController`/`iTermFindDriver`, `PasteSpecial*`/`PasteboardHistory*`, `OpenQuickly*`, `Arrangement*`, hotkey window classes, `iTermAI`/`AILiveHarness`, and `proto/api.proto`.

## 1. Menu commands

### iTerm2 menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| iTerm2 > About iTerm2 | no shortcut | Shows the About panel with version and build info. | have |
| iTerm2 > Show Tip of the Day | no shortcut | Shows a tip of the day dialog on startup or on demand. (milestone 13) | missing |
| iTerm2 > Companion Device Settings | no shortcut | Opens settings for pairing a companion mobile device with iTerm2. (milestone 13) | missing |
| iTerm2 > Suppressed Alerts... | no shortcut | Lists alert dialogs the user chose to suppress and lets them re-enable. (milestone 13) | missing |
| iTerm2 > Check for Updates... | no shortcut | Manually checks the Sparkle updater for a new iTerm2 build. (milestone 13) | missing |
| iTerm2 > Toggle Debug Logging | no shortcut | Turns internal debug log capture on or off. (milestone 6) | missing |
| iTerm2 > Save Retrospective Debug Logs | no shortcut | Dumps recent internal debug logs to a file for bug reports. (milestone 13) | missing |
| iTerm2 > Copy Performance Stats | no shortcut | Copies GPU and CPU performance counters to the clipboard. (milestone 13) | missing |
| iTerm2 > Capture GPU Frame | Ctrl+Opt+Cmd+G | Captures a single Metal GPU frame for renderer debugging. (milestone 13) | missing |
| iTerm2 > Check Settings Control Truncation | no shortcut | Debug tool that scans the Preferences UI for truncated control text. (milestone 13) | missing |
| iTerm2 > Preferences... | Cmd+, | Opens the Preferences/Settings window. | have |
| iTerm2 > Services | no shortcut | macOS system Services submenu, populated dynamically by the OS. | missing |
| iTerm2 > Hide iTerm2 | Cmd+H | Hides the application. | have |
| iTerm2 > Hide Others | Opt+Cmd+H | Hides all other applications. (milestone 13) | missing |
| iTerm2 > Show All | no shortcut | Unhides all hidden applications. (milestone 13) | missing |
| iTerm2 > Secure Keyboard Entry | Opt+Cmd+S | Enables Secure Keyboard Entry to block other apps from reading keystrokes. (milestone 13) | missing |
| iTerm2 > Make iTerm2 Default Term | Ctrl+Shift+Cmd+\ | Registers iTerm2 as the default terminal app for the OS. (milestone 13) | missing |
| iTerm2 > Make Terminal Default Term | Ctrl+Opt+Shift+Cmd+\ | Restores Terminal.app as the default terminal app. (milestone 13) | missing |
| iTerm2 > Install Shell Integration | no shortcut | Installs the shell integration script into the user's shell rc file. (milestone 4) | have |
| iTerm2 > Install Claude Code Integration | no shortcut | Installs hooks for Claude Code terminal integration. (milestone 13) | missing |
| iTerm2 > Reinstall Claude Code Integration | no shortcut | Reinstalls the Claude Code integration hooks. (milestone 13) | missing |
| iTerm2 > Uninstall Claude Code Integration | no shortcut | Removes the Claude Code integration hooks. (milestone 13) | missing |
| iTerm2 > Remove Recent Profiles from Dock Menu | no shortcut | Clears the recently used profile list shown in the Dock menu. (milestone 13) | missing |
| iTerm2 > Quit iTerm2 | Cmd+Q | Quits the application. | have |

### Shell menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Shell > New Window | Cmd+N | Opens a new terminal window with the default profile. | have, milestone 7a |
| Shell > New Window with Current Profile | Opt+Cmd+Shift+N | Opens a new window reusing the current session's profile. | have, milestone 7a |
| Shell > New Tab | Cmd+T | Opens a new tab with the default profile. | have |
| Shell > New Tab Next to Current Tab | Opt+Cmd+T | Opens a new tab immediately to the right of the current one. | have |
| Shell > New Tab with Current Profile | Opt+Cmd+T | Opens a new tab reusing the current session's profile. | have |
| Shell > Duplicate Window | no shortcut | Opens a new window cloning the current window's layout. (milestone 7) | have, milestone 7c: the window snapshotted and restored, splits and folders included |
| Shell > Duplicate Tab | no shortcut | Opens a new tab cloning the current tab's split layout. | have |
| Shell > Split Horizontally with Current Profile | Cmd+D | Splits the current pane into top and bottom using the current profile. | have |
| Shell > Split Vertically with Current Profile | Cmd+D | Splits the current pane into left and right using the current profile. | have |
| Shell > Split Horizontally... | Opt+Cmd+H | Splits the pane top and bottom, prompting for which profile to use. | have |
| Shell > Split Vertically... | Opt+Cmd+V | Splits the pane left and right, prompting for which profile to use. | have |
| Shell > Save Contents... | no shortcut | Saves the full scrollback contents of the session to a file. | have |
| Shell > Save Selected Text... | Cmd+S | Saves the current text selection to a file. | have |
| Shell > Close | Cmd+W | Closes the current session or pane. | have |
| Shell > Close Terminal Window | Shift+Cmd+W | Closes the entire terminal window. | have, milestone 7a |
| Shell > Close All Panes in Tab | Opt+Cmd+W | Closes every pane in the current tab. | have |
| Shell > Undo Close | Shift+Cmd+T | Reopens the most recently closed session. (milestone 7) | have, milestone 7b |
| Shell > Archives > Save Archive of Current Session... | no shortcut | Saves session scrollback and state as a reloadable archive file. (milestone 7) | missing |
| Shell > Archives > Restore Archive... | no shortcut | Reloads a previously saved session archive. (milestone 7) | missing |
| Shell > Workgroups > Exit Workgroup | no shortcut | Leaves a linked group of peer sessions. (milestone 7) | missing |
| Shell > Broadcast Input > Send Input to Current Session Only | Opt+Cmd+Shift+I | Turns off input broadcasting, typing goes only to the active pane. | have, milestone 7f |
| Shell > Broadcast Input > Broadcast Input to All Panes in All Tabs | Cmd+Shift+I | Sends typed input to every pane in every tab of the window. | have, milestone 7f |
| Shell > Broadcast Input > Broadcast Input to All Panes in Current Tab | Opt+Cmd+I | Sends typed input to every pane in the current tab. | have, milestone 7f |
| Shell > Broadcast Input > Toggle Broadcast Input to Current Session | Ctrl+Opt+Cmd+Shift+I | Adds or removes just the current session from the broadcast set. | have, milestone 7f |
| Shell > Broadcast Input > Show Background Pattern Indicator | no shortcut | Draws red diagonal stripes on panes receiving broadcast input (off by default). | have, milestone 7f |
| Shell > Broadcast Input > Current Session is Broadcast Source | no shortcut | Marks the current session as the source pane for broadcasting. | have, milestone 7f |
| Shell > Toggle Buffer Input | no shortcut | Buffers keystrokes typed while a command is running instead of discarding them. | have |
| Shell > tmux > Detach | Ctrl+Cmd+D | Detaches from the attached tmux session, leaving it running server side. (milestone 8) | have |
| Shell > tmux > Force Detach | no shortcut | Forcibly detaches from tmux even if the gateway is unresponsive. (milestone 8) | have |
| Shell > tmux > New Tmux Window | Ctrl+Cmd+N | Creates a new window inside the attached tmux session. (milestone 8) | have |
| Shell > tmux > New Tmux Tab | Ctrl+Cmd+T | Creates a new tab or pane inside the attached tmux session. (milestone 8) | have |
| Shell > tmux > Pause Pane | Ctrl+Cmd+P | Pauses output updates for a tmux pane to save bandwidth. (milestone 8) | have |
| Shell > tmux > Dashboard | no shortcut | Opens a dashboard listing all attached tmux sessions and windows. (milestone 8) | have |
| Shell > ssh > Disconnect | no shortcut | Disconnects the built in SSH integration session. | missing, the SSH integration milestone |
| Shell > ssh > Download Files... | no shortcut | Downloads files from the remote host over the SSH integration channel. | missing, the SSH integration milestone |
| Shell > ssh > Remote host can control iTerm2 | no shortcut | Lets escape sequences from the remote SSH host control iTerm2 features. | missing, the SSH integration milestone |
| Shell > Page Setup... | Cmd+P | Opens the macOS page setup dialog for printing. | have |
| Shell > Print > Screen | Cmd+P | Prints the visible screen contents. | partial, prints the rows in view; black and white is asked before the print dialog; inline images do not print |
| Shell > Print > Selection | Opt+Cmd+P | Prints the current text selection. | partial, as Screen |
| Shell > Print > Buffer | Opt+Cmd+P | Prints the full scrollback buffer. | partial, as Screen |

### Edit menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Edit > Undo | Cmd+Z | Undoes the last editable action, for example in the Composer or a text field. | have, disabled while a terminal has focus, as iTerm2 |
| Edit > Redo | Cmd+Z | Redoes the last undone editable action. | have, disabled while a terminal has focus, as iTerm2 |
| Edit > Cut | Cmd+X | Cuts selected text in editable fields, not the terminal grid. | have, never enabled in a session, as iTerm2 |
| Edit > Copy | Cmd+C | Copies the current selection to the clipboard. | have |
| Edit > Copy with Styles | Opt+Cmd+C | Copies the selection to the clipboard preserving colors and font styling. | have, HTML and RTF with the plain text |
| Edit > Copy with Control Sequences | no shortcut | Copies the selection including the raw escape sequences that produced it. | have |
| Edit > Copy Mode | Cmd+C | Enters keyboard driven vi-like selection mode for copying text. (milestone 5) | have |
| Edit > Paste | Cmd+V | Pastes clipboard contents into the session. | have |
| Edit > Paste Special > Advanced Paste... | Opt+Cmd+V | Opens a dialog with paste transform options such as escaping, encoding, and tab conversion. (milestone 9) | have, milestone 9b |
| Edit > Paste Special > Paste Selection | Cmd+V | Pastes the current selection, similar to middle-click paste on X11. | have |
| Edit > Paste Special > Paste File Base64-Encoded | no shortcut | Pastes a file's contents as a base64-encoded string. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Paste Slowly | no shortcut | Pastes character by character with a delay, for flaky serial links. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Paste Faster | no shortcut | Increases the speed used by slow paste. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Paste Slowly Faster | no shortcut | Increases the per-character delay speed of slow paste. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Paste Slower | no shortcut | Decreases the speed used by slow paste. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Paste Slowly Slower | no shortcut | Decreases the per-character delay speed of slow paste. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Warn Before Multi-Line Paste | no shortcut | Toggles a confirmation prompt before pasting multiple lines. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Limit Multi-Line Paste Warning to Shell Prompt | no shortcut | Restricts the multi-line paste warning to only fire at a shell prompt. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Prompt to Convert Tabs to Spaces when Pasting | no shortcut | Toggles a prompt offering to convert tabs to spaces on paste. (milestone 9) | have, milestone 9a |
| Edit > Paste Special > Warn Before Pasting One Line Ending in a Newline at Shell Prompt | no shortcut | Toggles a warning for a single line paste that ends with a newline at the prompt. (milestone 9) | have, milestone 9a |
| Edit > Replace Selection > Render Selection Natively | Opt+Cmd+E | Renders the selected text as an image or markup instead of raw text. (milestone 4) | have, milestone 5f: the selection's whole rows become a porthole (markdown, JSON, or highlighted code), Win+Alt+E; beeps over a porthole or with no selection |
| Edit > Replace Selection > Replace with Pretty-Printed JSON | no shortcut | Reformats the selected JSON text with indentation. (milestone 4) | have |
| Edit > Replace Selection > Replace with Base 64-Encoded Value | no shortcut | Replaces the selection with its base64-encoded form. (milestone 4) | have |
| Edit > Replace Selection > Replace with Base 64-Decoded Value | no shortcut | Replaces the selection with its base64-decoded form. (milestone 4) | have |
| Edit > Engage Artificial Intelligence | Cmd+Y | Opens an AI prompt to turn a natural-language request into a shell command. (milestone 12) | missing |
| Edit > Explain Output with AI | Opt+Cmd+Y | Sends recent terminal output to AI and shows an explanation. (milestone 12) | missing |
| Edit > Snippets | no shortcut | Dynamic submenu listing saved text snippets to insert into the session. | have |
| Edit > Actions | no shortcut | Dynamic submenu listing saved actions, macro-like keystroke, text, or script triggers. | have |
| Edit > Open Selection | Opt+Cmd+O | Opens the current selection as a URL or file path. (milestone 4) | have |
| Edit > Jump to Selection | Cmd+J | Scrolls the viewport so the current selection is visible. (milestone 4) | have |
| Edit > Select All | Cmd+A | Selects all text in the current session's scrollback. | have |
| Edit > Selection Respects Soft Boundaries | Opt+Cmd+A | Toggles whether selection stops at shell integration soft line boundaries. (milestone 4) | partial |
| Edit > Select Output of Last Command | Cmd+A | Selects the output of the most recently run shell command, needs shell integration marks. (milestone 4) | have |
| Edit > Select Current Command | no shortcut | Selects the command currently being typed at the prompt. (milestone 4) | have |
| Edit > Find > Find... | Cmd+F | Opens the in-session find bar. (milestone 4) | have |
| Edit > Find > Find Next | Cmd+G | Jumps to the next find match. (milestone 4) | have |
| Edit > Find > Find Previous | Cmd+G | Jumps to the previous find match. (milestone 4) | have |
| Edit > Find > Use Selection for Find | Cmd+E | Loads the current selection as the find query. (milestone 4) | have |
| Edit > Find > Clear Find | Opt+Cmd+E | Clears the find query and highlights. (milestone 4) | have |
| Edit > Find > Find Globally... | Cmd+F | Searches across all open sessions at once. (milestone 4) | have |
| Edit > Find > Select Matches | no shortcut | Selects all text matching the current find query. (milestone 4) | partial |
| Edit > Find > Find URLs | Opt+Cmd+U | Highlights all URLs detected in the buffer for quick opening. (milestone 4) | have |
| Edit > Find > Find All Smart Selection Matches | Opt+Cmd+U | Highlights all matches of the active Smart Selection rule. (milestone 4) | have |
| Edit > Find > Pick Result to Open | Opt+Cmd+O | Shows shortcut labels over found links and paths so one can be opened by key. (milestone 4) | have |
| Edit > Find > Filter | Opt+Cmd+F | Filters the visible buffer to only lines matching a query. (milestone 4) | partial |
| Edit > Marks and Annotations > Set Mark | Cmd+M | Drops a scroll position mark at the current line. (milestone 4) | have |
| Edit > Marks and Annotations > Jump to Mark | Cmd+J | Scrolls to the last saved mark. (milestone 4) | have |
| Edit > Marks and Annotations > Next Mark | Shift+Cmd+Down | Jumps to the next mark below in the scrollback. (milestone 4) | have |
| Edit > Marks and Annotations > Previous Mark | Shift+Cmd+Up | Jumps to the previous mark above in the scrollback. (milestone 4) | have |
| Edit > Marks and Annotations > Add Annotation at Cursor | Opt+Cmd+M | Adds a free-text sticky annotation attached to the cursor line. (milestone 4) | have |
| Edit > Marks and Annotations > Annotate Selection | no shortcut | Adds a free-text annotation attached to the current selection. (milestone 4) | have |
| Edit > Marks and Annotations > Next Annotation | Opt+Shift+Cmd+Down | Jumps to the next annotation below. (milestone 4) | have |
| Edit > Marks and Annotations > Previous Annotation | Opt+Shift+Cmd+Up | Jumps to the previous annotation above. (milestone 4) | have |
| Edit > Marks and Annotations > Alerts > Alert on Next Mark | Opt+Cmd+A | Arms a one-shot alert that fires the next time a mark is created. (milestone 4) | have |
| Edit > Marks and Annotations > Alerts > Alert on Marks in Offscreen Sessions | no shortcut | Extends mark alerts to sessions not currently visible. (milestone 4) | have |
| Edit > Marks and Annotations > Alerts > Show Modal Alert Box | no shortcut | Sets mark alerts to show a modal dialog. (milestone 4) | have |
| Edit > Marks and Annotations > Alerts > Post Notification | no shortcut | Sets mark alerts to post a macOS notification. (milestone 4) | have |
| Edit > Marks and Annotations > Alerts > Play a Sound | no shortcut | Sets mark alerts to play a sound. (milestone 4) | have |
| Edit > Set Named Mark | Opt+Cmd+M | Creates a mark with a user-supplied name for later lookup. (milestone 4) | have |
| Edit > Named Marks | no shortcut | Dynamic submenu listing named marks in the current session. | have |
| Edit > Fold Selected Lines | Cmd+L | Collapses the selected lines into a single foldable summary line. (milestone 4) | have |
| Edit > Fold All | Opt+Shift+Cmd+L | Collapses all foldable regions detected in the buffer. (milestone 4) | have |
| Edit > Clear Buffer | Cmd+K | Clears the visible screen and scrollback. | have |
| Edit > Clear Instant Replay | Opt+Cmd+K | Clears the Instant Replay history buffer. (milestone 10) | missing |
| Edit > Clear Scrollback Buffer | Cmd+K | Clears the scrollback history, keeping the visible screen. | have |
| Edit > Clear to Start of Selection | Cmd+L | Clears the buffer from the top down to the start of the current selection. (milestone 4) | have |
| Edit > Clear to Last Mark | Opt+Cmd+L | Clears the buffer up to the most recent mark. (milestone 4) | have |

### View menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| View > Show Tabs in Fullscreen | Cmd+U | Toggles whether the tab bar is shown while in full screen mode. (milestone 7) | missing |
| View > Toggle Full Screen | Ctrl+Cmd+F | Toggles native full screen mode for the window. | have |
| View > Toolbelt > Show Toolbelt | Cmd+B | Shows or hides the Toolbelt sidebar of utility panels. (milestone 9) | missing |
| View > Toolbelt > Set Default Width | no shortcut | Sets the current Toolbelt width as the default for new windows. (milestone 9) | missing |
| View > Use Transparency | Cmd+U | Toggles background transparency for the window. (milestone 7) | missing |
| View > Disable Transparency for Active Window | no shortcut | Forces the current window to be opaque regardless of the profile setting. (milestone 7) | missing |
| View > Zoom In on Selection | Opt+Cmd+Z | Temporarily enlarges the pane to focus on the current selection. (milestone 4) | partial |
| View > Zoom Out | no shortcut | Reverses Zoom In on Selection. (milestone 4) | have |
| View > Find Cursor | Cmd+/ | Flashes a highlight around the cursor to help locate it. (milestone 4) | have |
| View > Show Annotations | Cmd+\ | Shows or hides annotation markers in the buffer. (milestone 4) | have |
| View > Show Clippings | Shift+Cmd+= | Shows the Clippings/Snippets quick-insert panel. (milestone 9) | missing |
| View > Show Inline Chat | Shift+Cmd+, | Shows an inline AI chat panel attached to the session. (milestone 12) | missing |
| View > Show Composer | Shift+Cmd+. | Shows the Composer, a multi-line command editor above the prompt. (milestone 9) | missing |
| View > Show Cursor Guide | Opt+Cmd+; | Shows a horizontal highlight line tracking the cursor's row. (milestone 4) | have |
| View > Show Session Note | Opt+Cmd+G | Shows a free-text note panel attached to the session. (milestone 4) | have |
| View > Show Timestamps | Cmd+E | Shows a timestamp gutter next to each line of output. (milestone 4) | partial |
| View > Auto Command Completion | Shift+Opt+Cmd+Y | Toggles automatic command-history-based autocomplete suggestions. (milestone 9) | have, milestone 9d |
| View > Auto Composer | Opt+Shift+Cmd+. | Toggles automatically opening the Composer for multi-line input. (milestone 9) | missing |
| View > Open Quickly | Cmd+O | Opens a fuzzy search palette to jump to any window, tab, session, profile, or command. (milestone 9) | missing |
| View > Maximize Active Pane | no shortcut | Temporarily expands the focused pane to fill the tab, hiding siblings. | have |
| View > Make Text Bigger | Cmd++ | Increases the session font size. Tandem: Changes every pane, since all panes share one font until per-pane fonts arrive. | partial |
| View > Make Text Normal Size | Cmd+0 | Resets the font size to the profile default. | have |
| View > Restore Text and Session Size | Opt+Cmd+0 | Resets both font size and window dimensions to profile defaults. | have |
| View > Make Text Smaller | Cmd+- | Decreases the session font size. Tandem: Changes every pane, since all panes share one font until per-pane fonts arrive. | partial |
| View > Size Changes Update Profile | no shortcut | Toggles whether font size changes are saved back to the profile. (milestone 7) | missing |
| View > Start Instant Replay | Opt+Cmd+B | Opens Instant Replay, a scrubber for rewinding the screen's visual history. (milestone 10) | missing |

### Session menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Session > Edit Session... | Cmd+I | Opens the profile/session editor for the current session. | have |
| Session > Run Coprocess... | Opt+Cmd+R | Runs a local command piped to and from the session's I/O. (milestone 6) | have, milestone 6c |
| Session > Stop Coprocess | Opt+Cmd+Shift+R | Stops the running coprocess. (milestone 6) | have, milestone 6c |
| Session > Restart Session | no shortcut | Restarts the session's shell or command without closing the pane. | have |
| Session > Duplicate Session | no shortcut | Opens a new session cloning the current one's profile and directory. | have |
| Session > Open Autocomplete... | Cmd+; | Opens the shell integration based command autocomplete popup. (milestone 9) | have, milestone 9d |
| Session > Open Command History... | Shift+Cmd+; | Opens the Toolbelt command history panel/popup. (milestone 9) | have, milestone 9c |
| Session > Open Recent Directories... | Opt+Cmd+/ | Opens the Toolbelt recent directories panel/popup. (milestone 9) | have, milestone 9c |
| Session > Open Paste History... | Shift+Cmd+H | Opens the paste history popup for re-pasting earlier clipboard entries. (milestone 9) | have, milestone 9b |
| Session > Open AI Chat... | Ctrl+Cmd+Y | Opens an AI chat window scoped to the current session. (milestone 12) | missing |
| Session > Triggers > Add Trigger... | no shortcut | Adds a new trigger rule, a regex matched action on output, for the profile. (milestone 6) | have, milestone 6a |
| Session > Triggers > Edit Triggers | no shortcut | Opens the trigger list editor for the current profile. (milestone 6) | have, milestone 6a |
| Session > Triggers > Enable Triggers in Interactive Apps | no shortcut | Allows triggers to fire even while a full-screen interactive app such as vim is running. (milestone 6) | have, milestone 6a |
| Session > Triggers > Enable All | no shortcut | Enables every trigger for the profile. (milestone 6) | have, milestone 6a |
| Session > Triggers > Disable All | no shortcut | Disables every trigger for the profile. (milestone 6) | have, milestone 6a |
| Session > Reset | Cmd+R | Sends a terminal reset, clearing modes and screen state. | have |
| Session > Reset Character Set | no shortcut | Resets the terminal's character set designation to default. | have |
| Session > Move Session > Move Session to Window | no shortcut | Moves the current session/pane out into a new window. (milestone 7) | have, milestone 7a |
| Session > Move Session > Move Session to Tab | no shortcut | Moves the current session/pane out into a new tab. (milestone 7) | have, milestone 8h |
| Session > Move Session > Move Session to Split Pane | no shortcut | Moves the current session into a split position picked interactively. (milestone 7) | missing |
| Session > Log > Log to File | no shortcut | Starts or stops logging session output to a file. (milestone 6) | missing |
| Session > Log > Import Recording | no shortcut | Loads a previously exported session recording for playback. (milestone 10) | missing |
| Session > Log > Export Recording | no shortcut | Exports the session's recorded history to a file. (milestone 10) | missing |
| Session > Make Screenshot... | no shortcut | Captures a screenshot of the session content. | have, milestone 5e: a frozen copy with a camera sign, the panel with minimap and line range, Redact and Highlight, blur or solid, Copy, Save, Save As, 30000 px parts |
| Session > Terminal State > Alternate Screen | no shortcut | Manually toggles the alternate screen buffer mode. | have |
| Session > Terminal State > Focus Reporting | no shortcut | Manually toggles DECSET focus in/out event reporting. | have |
| Session > Terminal State > Mouse Reporting | no shortcut | Manually toggles mouse tracking/reporting mode. | have |
| Session > Terminal State > Paste Bracketing | no shortcut | Manually toggles bracketed paste mode. | have |
| Session > Terminal State > Application Cursor | no shortcut | Manually toggles DECCKM application cursor key mode. | have |
| Session > Terminal State > Application Keypad | no shortcut | Manually toggles application keypad mode. | have |
| Session > Terminal State > Standard Key Reporting Mode | no shortcut | Sets keyboard protocol to legacy/standard key reporting. | have |
| Session > Terminal State > modifyOtherKeys Mode 1 | no shortcut | Sets the xterm modifyOtherKeys keyboard protocol to mode 1. | have |
| Session > Terminal State > modifyOtherKeys Mode 2 | no shortcut | Sets the xterm modifyOtherKeys keyboard protocol to mode 2. | have |
| Session > Terminal State > CSI u Mode | no shortcut | Enables the CSI u (fixterms) keyboard protocol. | have |
| Session > Terminal State > Raw Key Reporting Mode | no shortcut | Enables raw keyboard event reporting. | have |
| Session > Terminal State > Disambiguate Escape | no shortcut | Enables the kitty keyboard protocol's escape key disambiguation flag. | have |
| Session > Terminal State > Report All Event Types | no shortcut | Enables the kitty keyboard protocol's key-up/repeat event reporting. | have |
| Session > Terminal State > Report Alternate Keys | no shortcut | Enables kitty keyboard protocol reporting of alternate keys (shifted symbols). | have |
| Session > Terminal State > Report All Keys as Escape Codes | no shortcut | Enables kitty keyboard protocol full escape-code key reporting. | have |
| Session > Terminal State > Report Associated Text | no shortcut | Enables kitty keyboard protocol reporting of the text a key event produces. | have |
| Session > Terminal State > Literal Controls | no shortcut | Toggles literal, non-interpreted, handling of control characters. | have |
| Session > Terminal State > Emulation Level > VT100 | no shortcut | Restricts terminal emulation to VT100-level feature set. | have, the level sets what DA1, DA2, DA3, and DECRQSS DECSCL report |
| Session > Terminal State > Emulation Level > VT200 | no shortcut | Restricts terminal emulation to VT200-level feature set. | have, the level sets what DA1, DA2, DA3, and DECRQSS DECSCL report |
| Session > Terminal State > Emulation Level > VT300 | no shortcut | Restricts terminal emulation to VT300-level feature set. | have, the level sets what DA1, DA2, DA3, and DECRQSS DECSCL report |
| Session > Terminal State > Emulation Level > VT400 | no shortcut | Restricts terminal emulation to VT400-level feature set. | have, the level sets what DA1, DA2, DA3, and DECRQSS DECSCL report |
| Session > Terminal State > Emulation Level > VT500 | no shortcut | Restricts terminal emulation to VT500-level feature set. | have, the level sets what DA1, DA2, DA3, and DECRQSS DECSCL report |
| Session > Terminal State > Reset | no shortcut | Resets all terminal state toggles to their defaults. | have |
| Session > Bury Session | Opt+Cmd+B | Hides the session from the tab bar while it keeps running, listed under Buried Sessions. (milestone 7) | have |
| Session > Buried Sessions | no shortcut | Dynamic submenu listing buried sessions to bring back. | have |

### Scripts menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Scripts > Manage > New Python Script | no shortcut | Creates a new Python API automation script from a template. (milestone 11) | missing |
| Scripts > Manage > Open Python REPL | no shortcut | Opens an interactive Python REPL connected to the running iTerm2 via the API. (milestone 11) | missing |
| Scripts > Manage > Manage Dependencies... | no shortcut | Opens the editor for Python package dependencies used by scripts. (milestone 11) | missing |
| Scripts > Manage > Install Python Runtime | no shortcut | Downloads and installs the bundled Python runtime used to run scripts. (milestone 11) | missing |
| Scripts > Manage > Install Already-Downloaded Python Runtime | no shortcut | Installs a Python runtime package already present on disk. (milestone 11) | missing |
| Scripts > Manage > Reveal Scripts in Finder | no shortcut | Opens the Scripts folder in Finder. (milestone 11) | missing |
| Scripts > Manage > Import... | no shortcut | Imports a script bundle into the Scripts folder. (milestone 11) | missing |
| Scripts > Manage > Export... | no shortcut | Exports a script as a shareable bundle. (milestone 11) | missing |
| Scripts > Manage > Console | Opt+Cmd+J | Opens the Script Console showing output and errors from running scripts. (milestone 11) | missing |
| Scripts (dynamic list) | no shortcut | The Scripts menu also lists installed scripts directly as runnable items, populated at runtime. | missing |

### Profiles menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Profiles > Open Profiles... | Cmd+O | Opens the Profiles window listing all saved profiles. | have |
| Profiles > Press Option for New Window | no shortcut | Informational hint item, not an action, showing that holding Option changes profile launch behavior. | have, milestone 7a |
| Profiles > Open In New Window | no shortcut | Header above the dynamically inserted list of profiles, each launchable in a new window or tab. | have, milestone 7a: the profiles are listed; Alt picks a window |

### Toolbelt menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Toolbelt (menu location) | no shortcut | In this iTerm2 build the Toolbelt commands live under View > Toolbelt rather than a standalone top-level menu; see Show Toolbelt and Set Default Width in the View menu above. | missing |

### Window menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Window > Minimize | Cmd+M | Minimizes the window to the Dock. | have |
| Window > Zoom | Shift+Cmd+0 | Zooms/maximizes the window. | have |
| Window > Arrangements > Save Window Arrangement | Shift+Cmd+S | Saves the current set of windows, tabs, and splits as a named arrangement. (milestone 7) | have, milestone 7c |
| Window > Arrangements > Save Current Window as Arrangement | Opt+Shift+Cmd+S | Saves just the current window as a named arrangement. (milestone 7) | have, milestone 7c |
| Window > Arrangements > Load Arrangement from File... | no shortcut | Loads a window arrangement from a file on disk. (milestone 7) | have, milestone 7c |
| Window > Arrangements > Restore Window Arrangement | no shortcut | Dynamic submenu listing saved arrangements to restore as windows. | have, milestone 7c: the default is marked; it has no Shift+Cmd+R (Win chords reach only fixed menu items) |
| Window > Arrangements > Restore Window Arrangement as Tabs | no shortcut | Dynamic submenu listing saved arrangements to restore merged into tabs of the current window. | have, milestone 7c |
| Window > Arrangements > Change Profile in Arrangement... | no shortcut | Bulk changes which profile a saved arrangement's sessions use. (milestone 7) | have, milestone 7c: in Settings > Arrangements |
| Window > Edit Window Title | no shortcut | Opens an editor to set a custom window title override. (milestone 7) | have, milestone 7a |
| Window > Tab > Edit Tab Title | no shortcut | Opens an editor to set a custom tab title override. (milestone 7) | have, milestone 7a |
| Window > Tab > Select Tab > Select Next Tab | Shift+Cmd+] | Switches focus to the next tab. | have |
| Window > Tab > Select Tab > Select Previous Tab | Shift+Cmd+[ | Switches focus to the previous tab. | have |
| Window > Tab > Select Tab > Select Next Tab Group | Ctrl+Shift+Cmd+] | Switches focus to the next tab group. (milestone 7) | missing |
| Window > Tab > Select Tab > Select Previous Tab Group | Ctrl+Shift+Cmd+[ | Switches focus to the previous tab group. (milestone 7) | missing |
| Window > Tab > Move Tab > Move Tab to New Window | no shortcut | Pulls the current tab out into its own new window. (milestone 7) | have, milestone 7a |
| Window > Tab > Move Tab > Move Tab Left | Opt+Shift+Cmd+[ | Reorders the current tab one position to the left. | have |
| Window > Tab > Move Tab > Move Tab Right | Opt+Shift+Cmd+] | Reorders the current tab one position to the right. | have |
| Window > Tab > Add Tab to New Group... | no shortcut | Creates a new tab group containing the current tab. (milestone 7) | missing |
| Window > Tab > Remove Tab from Group | no shortcut | Removes the current tab from its tab group. (milestone 7) | missing |
| Window > Split Pane > Select Split Pane > Select Pane Above | Opt+Cmd+Up | Moves focus to the pane directly above. | have |
| Window > Split Pane > Select Split Pane > Select Pane Below | Opt+Cmd+Down | Moves focus to the pane directly below. | have |
| Window > Split Pane > Select Split Pane > Select Pane Left | Opt+Cmd+Left | Moves focus to the pane to the left. | have |
| Window > Split Pane > Select Split Pane > Select Pane Right | Opt+Cmd+Right | Moves focus to the pane to the right. | have |
| Window > Split Pane > Select Split Pane > Next Pane | Cmd+] | Moves focus to the next pane in tab order. | have |
| Window > Split Pane > Select Split Pane > Previous Pane | Cmd+[ | Moves focus to the previous pane in tab order. | have |
| Window > Split Pane > Select Split Pane > Previous Peer | Opt+Cmd+[ | Moves focus to the previous peer session in a workgroup. (milestone 7) | missing |
| Window > Split Pane > Select Split Pane > Next Peer | Opt+Cmd+] | Moves focus to the next peer session in a workgroup. (milestone 7) | missing |
| Window > Split Pane > Resize Split Pane > Move Divider Up | Ctrl+Cmd+Up | Resizes the pane by nudging its divider up via keyboard. | have |
| Window > Split Pane > Resize Split Pane > Move Divider Down | Ctrl+Cmd+Down | Resizes the pane by nudging its divider down via keyboard. | have |
| Window > Split Pane > Resize Split Pane > Move Divider Left | Ctrl+Cmd+Left | Resizes the pane by nudging its divider left via keyboard. | have |
| Window > Split Pane > Resize Split Pane > Move Divider Right | Ctrl+Cmd+Right | Resizes the pane by nudging its divider right via keyboard. | have |
| Window > Split Pane > Lock Split Pane Width | no shortcut | Locks the current pane's width so window resizes do not change it. (milestone 7) | missing |
| Window > Window Style > Normal | no shortcut | Sets the window style to a normal title-barred window. | have |
| Window > Window Style > Full Screen | no shortcut | Sets the window style to native full screen. | have |
| Window > Window Style > Maximized | no shortcut | Sets the window style to maximized, fills the screen and keeps chrome. | have |
| Window > Window Style > No Title Bar | no shortcut | Sets the window style to borderless with no title bar. (milestone 7) | have, milestone 7a |
| Window > Window Style > Centered | no shortcut | Sets the window style to a centered floating panel. (milestone 7) | have, milestone 7a |
| Window > Window Style > Centered, No Title Bar | no shortcut | Sets the window style to a centered borderless panel. (milestone 7) | have, milestone 7a |
| Window > Window Style > Full-Width Bottom of Screen | no shortcut | Docks the window along the full bottom edge of the screen. (milestone 7) | have, milestone 7a |
| Window > Window Style > Full-Width Top of Screen | no shortcut | Docks the window along the full top edge of the screen. (milestone 7) | have, milestone 7a |
| Window > Window Style > Full-Height Left of Screen | no shortcut | Docks the window along the full left edge of the screen. (milestone 7) | have, milestone 7a |
| Window > Window Style > Full-Height Right of Screen | no shortcut | Docks the window along the full right edge of the screen. (milestone 7) | have, milestone 7a |
| Window > Window Style > Bottom of Screen | no shortcut | Docks the window at the bottom of the screen at its natural size. (milestone 7) | have, milestone 7a |
| Window > Window Style > Top of Screen | no shortcut | Docks the window at the top of the screen at its natural size. (milestone 7) | have, milestone 7a |
| Window > Window Style > Left of Screen | no shortcut | Docks the window at the left of the screen at its natural size. (milestone 7) | have, milestone 7a |
| Window > Window Style > Right of Screen | no shortcut | Docks the window at the right of the screen at its natural size. (milestone 7) | have, milestone 7a |
| Window > Lock Size | no shortcut | Prevents the window from being resized. (milestone 7) | have, milestone 7a |
| Window > Lock Layout | no shortcut | Prevents the split pane layout from being changed. (milestone 7) | have, milestone 7a |
| Window > Notify on Status Change | no shortcut | Posts a notification when a background session's status changes, such as bell, idle, or done. (milestone 6) | have, milestone 6b: a bar with Reveal on the next status change |
| Window > Arrange Windows > Merge All Windows | no shortcut | Merges all open windows' tabs into a single window. (milestone 7) | have, milestone 7a |
| Window > Arrange Windows > Arrange Windows Horizontally | no shortcut | Tiles all open windows horizontally across the screen. (milestone 7) | have, milestone 7a |
| Window > Arrange Windows > Arrange Split Panes Evenly | no shortcut | Resets all split dividers in the tab to equal sizes. | have |
| Window > Resize Window > Decrease Height | Opt+Cmd+Up | Shrinks the window height by one row via keyboard. (milestone 7) | have, milestone 7a |
| Window > Resize Window > Increase Height | Opt+Cmd+Down | Grows the window height by one row via keyboard. (milestone 7) | have, milestone 7a |
| Window > Resize Window > Decrease Width | no shortcut | Shrinks the window width by one column via keyboard. (milestone 7) | have, milestone 7a |
| Window > Resize Window > Increase Width | no shortcut | Grows the window width by one column via keyboard. (milestone 7) | have, milestone 7a |
| Window > Password Manager | Opt+Cmd+F | Opens the Password Manager window for storing and injecting credentials. (milestone 10) | missing |
| Window > AI Chats | Cmd+Y | Opens the window listing all AI chat conversations. (milestone 12) | missing |
| Window > Cockpit | Opt+Cmd+C | Opens the Cockpit window, a command and session overview dashboard. (milestone 12) | missing |
| Window > Pin Hotkey Window | Shift+Cmd+- | Pins the hotkey (Quake-style dropdown) window so it stays open. (milestone 7) | have |
| Window > Bring All To Front | no shortcut | Brings all of the app's windows to the front. | have |

### Help menu
| Menu > Item | Default shortcut | What it does | Tandem status |
|---|---|---|---|
| Help > iTerm2 Help | Cmd+? | Opens the iTerm2 help/documentation. | have |
| Help > Copy Mode Shortcuts | no shortcut | Shows a cheat sheet of Copy Mode keyboard shortcuts. (milestone 13) | missing |
| Help > Open Source Licenses | no shortcut | Shows license text for bundled open source components. (milestone 13) | missing |
| Help > GPU Renderer Availability | no shortcut | Shows diagnostic info on which GPU renderer is active and why. (milestone 13) | missing |

Note: MainMenu.xib also defines a Web menu (Open Location, Back, Forward, Reload Page, History) for iTerm2's built in browser feature. That menu falls outside the ten requested menus and is not listed here.

## 2. Escape sequences and protocols

Everything iTerm2 supports beyond baseline xterm compatibility. Tandem's VT engine is alacritty_terminal, which already parses OSC 8 hyperlinks and OSC 52 clipboard payloads at the protocol level, so those two rows are marked partial rather than missing; everything else below is missing.

| Sequence | What it does | Tandem status |
|---|---|---|
| OSC 1337 ; File=... (inline images) | Displays or downloads an inline image or arbitrary file. Args include name, size, width, height, preserveAspectRatio, inline, type, mode, inset. Core of iTerm2 inline image support (imgcat). | have, inline images with iTerm2 sizing; downloads ask, then save to the Downloads folder (no "Choose..." yet); milestone 5f: text documents (`inline=1` with a text type or name, or UTF-8 that is not a picture) show rendered in place, `mode=wide` |
| OSC 1337 ; MultipartFile=... | Same as File= but data arrives in chunks via FilePart/FileEnd. | have |
| OSC 1337 ; FilePart=... | Appends a base64 chunk to an in-progress multipart file transfer. | have |
| OSC 1337 ; FileEnd | Ends a multipart file transfer. | have |
| OSC 1337 ; SetMark | Places a manual mark at the current line for mark navigation. | have |
| OSC 1337 ; CursorShape=N | Sets the cursor shape: 0 block, 1 bar, 2 underline. | have |
| OSC 1337 ; AddNote / AddHiddenNote (deprecated) | Old names for AddAnnotation and AddHiddenAnnotation. | have |
| OSC 1337 ; BeginFile / EndFile (deprecated) | Old file-transfer keys that iTerm2 only logs. | have, ignored as iTerm2 does |
| OSC 1337 ; StealFocus | Brings the iTerm2 window/tab to the foreground, trusted only. | partial, asks each time with Allow / Deny; iTerm2 can remember the answer |
| OSC 1337 ; ClearScrollback | Clears the scrollback buffer. | have |
| OSC 1337 ; CurrentDir=&lt;path&gt; | Reports current working directory for shell integration, trusted only. | have |
| OSC 1337 ; RemoteHost=&lt;user@host&gt; | Reports remote user/host for shell integration, for example after ssh, trusted only. | have |
| OSC 1337 ; ShellIntegrationVersion=&lt;n&gt; | Announces shell integration script version and shell type. | have |
| OSC 1337 ; SetProfile=&lt;name&gt; | Switches the session's profile on the fly, trusted only. | have |
| OSC 1337 ; AddNote / AddAnnotation=&lt;text&gt; | Adds a visible annotation bound to the current text. | have |
| OSC 1337 ; AddHiddenNote / AddHiddenAnnotation=&lt;text&gt; | Adds a hidden annotation. | have |
| OSC 1337 ; HighlightCursorLine[=bool] | Toggles cursor line highlighting from the shell. | have |
| OSC 1337 ; ClearCapturedOutput | Clears the captured output list, a trigger feature. | missing, milestone 6 (captured output) |
| OSC 1337 ; CopyToClipboard=&lt;dest&gt; ... OSC 1337 ; EndCopy | Legacy iTerm2 copy-to-pasteboard sequence, streams text until EndCopy, trusted only. | have, named pasteboards all go to the clipboard |
| OSC 1337 ; Copy=&lt;mode&gt;[;uid] ... | Copies arbitrary pasteboard data, monolithic or segmented, into the system clipboard, trusted only. | have |
| OSC 1337 ; RequestAttention=fireworks\|once\|flash\|1\|0 | Bounces the dock icon, flashes the screen, or fires a fireworks animation to get user attention. | have, the taskbar button flashes where the dock icon bounces |
| OSC 1337 ; SetBackgroundImageFile=&lt;path&gt; | Sets the session's background image, trusted only. | have |
| OSC 1337 ; SetProfileProperty=&lt;key&gt;=&lt;base64 JSON&gt; | Changes a single profile property at runtime, trusted only. | have, the session gets its own profile copy; several keys are listed in the bar text |
| OSC 1337 ; SetBadgeFormat=&lt;base64&gt; | Sets the session badge text/template shown in the corner of the pane. | have, milestone 6b (function calls refused) |
| OSC 1337 ; SetUserVar=&lt;name&gt;=&lt;base64 value&gt; | Sets a user-defined variable exposed to status bar, badges, and scripting. | have |
| OSC 1337 ; ReportCellSize (query) | Terminal reports character cell size in points and scale factor. | have |
| OSC 1337 ; SetWindowFrame=x;y;w;h | Sets the window's frame in points. | have |
| OSC 1337 ; ReportWindowFrame (query) | Terminal reports current window frame in points. | have |
| OSC 1337 ; ReportScreenFrames (query, trusted) | Terminal reports frames of all connected screens. | have, monitor work areas |
| OSC 1337 ; UnicodeVersion=push\|pop\|&lt;n&gt; | Pushes/pops or sets the Unicode width calculation version for wide character handling. | partial, the version is kept and reported; character widths do not change with it |
| OSC 1337 ; SetColors=&lt;name&gt;=&lt;color&gt;,... | Sets one or more named ANSI/theme colors at once. | partial, every color but badge (milestone 6) and preset (no color presets yet); tab draws a stripe |
| OSC 1337 ; SetKeyLabel=&lt;key&gt;=&lt;label&gt; | Sets the on screen label for a Touch Bar key. | partial, Windows has no Touch Bar: labels are kept, not shown |
| OSC 1337 ; PushKeyLabels / PopKeyLabels | Saves/restores a set of Touch Bar key labels. | partial, Windows has no Touch Bar: labels are kept, not shown |
| OSC 1337 ; Disinter | Detaches (disinters) a session from its buried state. | have, milestone 8e: a buried session that sends it is restored |
| OSC 1337 ; ReportVariable=&lt;base64 name&gt; (trusted) | Terminal reports the value of a named session variable. | partial, the session, user, tab, window, and app names Tandem has |
| OSC 1337 ; Custom=&lt;params&gt;:&lt;payload&gt; (trusted) | Fires a user-defined custom escape-sequence trigger with key/value params and a payload. | have, milestone 6b: the Custom Escape Sequence event trigger |
| OSC 1337 ; Notification=&lt;text&gt; | Posts a rich user notification (toast). | have, milestone 6b: title, subtitle, and message (no image) |
| OSC 1337 ; Capabilities (query, trusted) | Terminal reports its capability set. | have |
| OSC 1337 ; Env=&lt;name&gt;=&lt;base64 value&gt; | Updates a tracked environment variable for the session, trusted only. | partial, kept for the SSH integration milestone |
| OSC 1337 ; it2ssh=&lt;params&gt; | Begins iTerm2's own SSH integration (Conductor) session. | missing, the SSH integration milestone |
| OSC 1337 ; SendConductor=&lt;data&gt; | Sends data to the Conductor SSH integration channel. | missing, the SSH integration milestone |
| OSC 1337 ; EndSSH=&lt;id&gt; | Ends an SSH integration session, trusted only. | missing, the SSH integration milestone |
| OSC 1337 ; OpenURL=&lt;id&gt;:&lt;base64 url&gt; (trusted) | Asks iTerm2 to open a URL in the default browser. | partial, web and mail links open; other schemes are refused |
| OSC 1337 ; wrap=a=start;cmd=...;uid=... (trusted) | Wraps a command for iTerm2's command wrapping/AI or Conductor pipeline. | missing, the SSH integration milestone |
| OSC 1337 ; ExecFailed | Signals that exec of a wrapped command failed. | partial, the flag is kept; the session-ended message comes with milestone 6 |
| OSC 1337 ; Block / UpdateBlock | Marks a folded/collapsible output block, start/end, fold/unfold, for command output blocks. | have, milestone 5e: the gutter bar, the hover pill (copy, fold), a margin click folds; milestone 5f: `render=1` shows the block rendered in place |
| OSC 1337 ; Button=type=copy\|custom;... | Inserts an inline clickable button, for example a copy-output button, in a command-output block. | have, milestone 5e: copy and custom buttons, SF Symbol names drawn with Segoe Fluent Icons, the click report only when trusted |
| OSC 1337 ; RequestUpload=&lt;params&gt; (trusted) | Asks the user to pick a local file to upload/send to the shell. | partial, files only (no folders); progress as a message, not a transfers menu |
| OSC 1337 (bare, terminal-to-host report) | Terminal reports its iTerm2 version string as a DSR-style query. | have, `CSI 1337 n` answers `CSI ITERM2 <version> n` |
| OSC 21337 (tab status) | Sets a structured tab status key/value payload shown in the tab. | partial, the dot, status text, and priority; the notification and taskbar badge come with milestone 6 |
| OSC 133 ; A / N / P (FTCS_PROMPT_START) | Marks the start of a shell prompt, optionally forcing a fresh line, carries k=&lt;kind&gt; and aid=&lt;id&gt;. Basis of iTerm2's command marks and prompt navigation. | have |
| OSC 133 ; B (FTCS_COMMAND_START) | Marks the end of the prompt and start of the typed command. | have |
| OSC 133 ; C (FTCS_COMMAND_EXECUTED) | Marks the end of the typed command and start of its output. | have |
| OSC 133 ; D[;exit][;aid=] (FTCS_COMMAND_FINISHED) | Marks command completion and reports its exit code, used for command marks, red/green gutter marks, and jump to prior command. | have |
| OSC 133 ; E (semantic text start) | Marks the start of semantic text of a given type, for example filename, directory, pid. | have, parsed and ignored, as iTerm2 does |
| OSC 133 ; F (semantic text end) | Marks the end of semantic text. | have, parsed and ignored, as iTerm2 does |
| OSC 133 ; G (progress) | Updates or clears a progress indicator/percentage shown for the running command. | have, parsed and ignored, as iTerm2 does |
| OSC 9 ; &lt;text&gt; | Posts a plain notification with the text (iTerm2's Growl-style OSC 9). | have, milestone 6b |
| OSC 9 ; 4 ; &lt;state&gt; ; &lt;progress&gt; | Shows or clears a progress bar for the session. | partial, milestone 6b: parsed, and the Progress Bar Changed event trigger fires; no bar is drawn yet |
| OSC 133 ; H (terminal command) | Sends an arbitrary terminal command token to the delegate. | have, parsed and ignored, as iTerm2 does |
| OSC 7 ; file://user@host/path | Reports the current directory, user, and host as a URL; iTerm2's current shell scripts use it instead of 1337;CurrentDir and RemoteHost. Without a machineID it also marks a prompt. | have |
| OSC 8 ; params ; url ST ... text ... OSC 8 ; ; ST | Standard terminal hyperlinks; iTerm2 tracks link start/end and renders clickable text. | have, links draw a dashed underline; Win-click opens them; the right-click menu and a pointer action copy the address |
| OSC 52 (PASTE64) set/query clipboard | Reads or writes the system clipboard, and other X11-style selections, via a base64 payload. | have, writes under Applications in terminal may access clipboard; reads ask, as iTerm2 |
| DECRQSS / DECRPSS (SGR, DECSCL, DECSCUSR, DECSCA, DECSTBM, DECSLRM, DECSLPP) | Request/report current terminal settings, graphic rendition, conformance level, cursor style, protection attribute, margins, page size, as an ANSI-standard report-settings sequence. | have, SGR, DECSCL, DECSCUSR, DECSCA, DECSTBM, DECSLRM, DECSLPP; XTVERSION and XTGETTCAP (TN, name, Co, colors, RGB) too |
| Sixel graphics (DECSIXEL, via VT100SixelParser/VT100DCSParser) | Parses and renders sixel bitmap graphics sent by an application, for example img2sixel. | partial, decoded by Tandem (palette, HLS, repeats, raster size); pixels opaque as in iTerm2; DECSDM is not handled yet |
| Kitty graphics protocol (transmit, put, delete, animate, compose, query) | Parses and renders the kitty terminal graphics protocol for inline images and animations, rendered via a dedicated Metal renderer. | partial, transmit (direct, file, temp file, zlib, chunks), display, delete, query; z-index, pixel offsets, unicode placeholders, and animation are not drawn (iTerm2 does not animate either) |
| Kitty keyboard protocol (progressive enhancement, CSI u key reporting) | Disambiguated keyboard reporting mode used by some modern CLI apps. | have, all five flags, the stack per screen, and Session > Terminal State toggles; modifyOtherKeys 1 and 2, CSI u, and raw key reporting (Windows virtual key codes) too |
| Synchronized output, DEC private mode 2026 (BSU/ESU) | Suppresses screen redraws between begin and end markers so a full-screen app can update atomically without tearing. | have, mode 2026 and the DCS form; an update that never ends is drawn after 150 ms |
| DEC private mode 996/997 (dark-mode query) | Application asks the terminal whether the OS is in dark or light mode; terminal replies with mode 997. | have, judged from the pane's background; mode 2031 reports changes |
| CSI ? Ps c (DA1/DA2/DA3) with iTerm2 extensions | Extended device-attributes reporting including iTerm2's emulation level, up to VT400 class, and macro-space/checksum reports. | have, per emulation level as iTerm2 answers |
| Kitty drag-and-drop file protocol | iTerm2-specific extension layered on drag-and-drop for transferring files over SSH sessions using a kitty-protocol-derived encoding. | missing |

## 3. Subsystems

### tmux integration
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| tmux integration | tmux -CC control mode | Attaches to a tmux session over a control-mode connection so tmux windows appear as native windows/tabs instead of a text UI. | partial | Milestone 8a to 8c: the gateway, the protocol, tmux windows as tabs and windows, sizes, edits from Tandem, closing questions, @hidden and @affinities, and detach. Pause mode (8d), the Dashboard and burying (8e), and the monitors, user variables, and TERM rules (8f) are built, and clipboard sync, client tracking, and color reports (8g); pane moves, hotkeys and tab colors, and window persistence (8h) remain. It needs the shipped ConPTY (conpty.dll and OpenConsole.exe beside tandem.exe). |
| tmux integration | Session dashboard | Lists all tmux sessions on the server with a picker to attach, rename, or create a new one. | have | Milestone 8e: the tmux Dashboard (connections popup, sessions with rename, add, remove, attach, detach; windows with search, rename, add, remove, open in tabs or windows, hide; the attach limit and hidden-window checkbox). Dragging windows between sessions is not in it. |
| tmux integration | Window to tab/window mapping | Maps each tmux window to a native window or tab, tracks window IDs, opens new tmux windows automatically. | partial | Milestone 8b: each tmux window opens as a tab or window by OpenTmuxWindowsIn and anonymousTmuxWindowsOpenInCurrentWindow, affinities read, %window-add/close/renamed and %session-changed followed. Saving affinities and tab moves are 8c. |
| tmux integration | Pane layout sync | Parses tmux's layout string and reproduces the split-pane geometry natively, keeps it in sync as tmux panes are added, closed, or resized. | have | Milestone 8b/8c: tmux layouts become cell-exact splits and %layout-change is followed; divider drags send resize-pane on release (tmux lines wait during the drag), splits send split-window, zoom and swap relist. |
| tmux integration | Detach and reattach | Cleanly detaches the control client from tmux without killing the tmux session, and reattaches later restoring layout. | partial | Milestone 8c: Detach (a second request offers Force Detach), Force Detach, and reattach with the same windows grouped by @affinities. Window origins and per-window settings wait for 8e. |
| tmux integration | Window/session/pane rename sync | Propagates tmux renames of windows, sessions, and panes to the native UI, and lets the user rename back into tmux. | have | Milestone 8b/8c: %window-renamed renames the tab, and Edit Tab Title on a tmux tab sends rename-window (pane titles need 2.6, via the same escape). |
| tmux integration | Per-window and per-tab settings persistence | Saves and restores window affinities, origins, hidden windows, and per-window/per-tab overrides such as font and key mappings, across attach cycles. | have | Milestone 8c/8h: @affinities, @hidden, @per_window_settings (size or fs; Tandem has no toolbelt yet), @per_tab_settings (Edit Tab Title), @origins (Tandem's screen pixels, kept on a monitor), and @buried_indexes (a hidden window returns to its tab), each saved only when it changed and restored on attach. |
| tmux integration | Hidden windows | Lets a tmux window exist on the server but stay hidden from the native window list; can be revealed later. | have | Milestone 8b/8e: @hidden windows stay closed on attach, Hide keeps a window in tmux only, and the Dashboard and Session > Buried Sessions open them again. |
| tmux integration | tmux status bar passthrough | Mirrors tmux's own status line content into iTerm2 when tmux driven status text changes. | missing | |
| tmux integration | tmux option monitor | Watches tmux global/session/window options, for example pane-border-status, and reacts live. | have | Milestone 8f: iTerm2's option monitors: tmux 3.2 subscriptions (refresh-client -B, ids it2_N) with a first display-message poll, and event polls before 3.2 (OSC 0/1/2, key presses, line feeds for the directory, the job timer with pollForTmuxForegroundJob). Pane title, foreground job (when the profile shows an icon), working directory, and the set-titles string of each window. The status-bar monitor waits for the status bar (milestone 9). |
| tmux integration | Copy mode bridging | Lets iTerm2 exit/enter tmux copy mode and reconcile it with native scrollback/selection. | have | Milestone 8a: copy-mode -q on attach, which is all iTerm2 does (it ignores %pane-mode-changed). |
| tmux integration | Pane pause/buffer management | Pauses/unpauses individual tmux panes and monitors buffer size to avoid unbounded growth while paused. | have | Milestone 8d: pause mode on tmux 3.2+: refresh-client -fpause-after, the %extended-output latency monitor with iTerm2's fit, red zone, and hysteresis, the warning and paused bars, automatic unpause, and unpause by recapture. |
| tmux integration | Break pane out to window/tab | Moves a tmux pane into its own new window or tab, or docks it next to a sibling, both directions. | have | Milestone 8h: Move Session to Window and Move Session to Tab on a tmux pane send break-pane -P -F "#{window_id}" -s "%P"; the new window opens in a new Tandem window (a little right of and below this one) or beside its old tab. Dragging panes and Move Session to Split Pane wait for pane drag and drop. |
| tmux integration | Window/pane hotkeys | Lets a tmux pane carry its own quick-access hotkey binding independent of the main app hotkey window. | missing | Milestone 8h reads nothing from @hotkeys: Tandem has no session hotkeys yet (01 "Session hotkey"). |
| tmux integration | Tab color from tmux | Reads/writes a tmux pane's associated tab color string so tab coloring matches tmux state. | have | Milestone 8h: @tab_colors keeps each tmux pane's tab color (#rrggbb or none; iTerm2's p3# read as sRGB), written when a profile or a program sets it and restored on attach; iTerm2's "Fix corrupted tab colors?" question for pure red. |
| tmux integration | Client tracking / multi-client awareness | Tracks other attached tmux clients and reacts to their session/window changes, for example someone else attaching on another machine. | have | Milestone 8g: on tmux 3.6 and later the client learns its #{client_name}, lists the session's clients (list-clients with client_control_mode), relists on %client-session-changed, and drops a client on %client-detached. Made once per connection with the first session, as iTerm2. |
| tmux integration | OSC 4/10/11/52 responder election | Decides whether iTerm2 or tmux itself is the designated responder for color and clipboard OSC queries when multiple clients are attached. | have | Milestone 8g: iTerm2's rule: no plain client attached and this is the control client with the smallest name. The elected client answers OSC 4 in tmux panes (3.6+) and OSC 52 reads by the get-clipboard/set-clipboard levels (3.6+, 3.7+); OSC 10/11 are left to tmux, which gets each pane's colors with refresh-client -r (3.4+). |
| tmux integration | Local vs remote tmux detection | Distinguishes a tmux server running on the local machine from one reached over SSH, to decide default behaviors such as default TERM and local file access. | have | Milestone 8f: the server pid is read (display-message -p "#{pid}") as iTerm2 does; on Windows no local process can be the tmux server (a WSL pid is never a Windows process), so every server counts as remote and jobs come from the monitor. |
| tmux integration | User variables over tmux | Carries iTerm2 shell integration user variables through the tmux control protocol, since tmux would otherwise swallow OSC 1337. | have | Milestone 8f: SetUserVar in a tmux pane is saved as the pane option @uservars (u_ hex of NUL-joined key=value, tmux 3.1 and later) and read back on attach. |
| tmux integration | Gateway ping/keepalive and version guessing | Sends periodic pings and probes tmux server version/capabilities, for example UTF-8 support, to adapt protocol handling. | have | Milestone 8a: the kick-off (phony-command with the 5 s "Slow tmux Response" warning, refresh-client wait-exit), the aggressive-resize and UTF-8 checks with their alerts, and the version from #{version} or the 2.3/2.2/2.1/1.9 probes. tmux has no periodic ping; iTerm2 sends none either. |

### shell integration
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Shell integration | Install scripts for bash/zsh/fish/tcsh/xonsh | Shell-specific hook scripts that install prompt/precmd/preexec hooks. | have | PowerShell: Tandem ships its own script (Install Shell Integration). bash, zsh, fish (WSL, Git Bash, ssh): iTerm2's own installer. cmd.exe has no prompt hooks. |
| Shell integration | FinalTerm-style prompt marks (OSC 133) | Marks prompt-start, command-start, command-output-start, and command-end so the terminal knows prompt/command/output boundaries. | have |  |
| Shell integration | Current directory reporting | Shell reports cwd on every prompt so the terminal always knows the working directory, independent of parsing prompt text. | have | OSC 7 and 1337;CurrentDir. |
| Shell integration | Remote host reporting | Shell reports user@host so the terminal can track SSH hops and label tabs/panes accordingly, and offer host-based profile switching. | have | Host and user are tracked; host-based profile switching comes in milestone 6. |
| Shell integration | User-defined variables | Lets shell scripts publish arbitrary key/value variables consumable by status bar components, badges, and triggers. | have | Stored per session; badges and the status bar that show them come in milestones 6 and 9. |
| Shell integration | Shell integration version handshake | Reports the installed shell integration script's version and shell name so the app can warn about outdated installs. | have | Version and shell are stored; there is no upgrade prompt. |
| Shell integration | Command history capture | Captures each command typed, via prompt marks, and its associated host/directory for the Command History toolbelt tool and search. | have | Milestone 9c: every command with its directory, time, and exit code per user@host, ranked by iTerm2 frecency, kept 90 days, saved with "Save copy/paste and command history to disk". |
| Shell integration | Command status marks (success/failure coloring) | Colors the mark for a finished command based on its exit code, surfaced in the scrollbar and Command History tool. | have | Blue, yellow, or red in the left margin. |
| Shell integration | Automatic profile switching by hostname | Switches to a different saved profile automatically when remote host reporting detects an SSH session to a known host. | have, milestone 6c |  |
| Shell integration | it2git status variables | A shell hook script that publishes git branch/dirty state as user variables for status bar/prompt use. | missing | |
| Shell integration | imgcat | Displays an image file inline in the terminal using the proprietary inline-image protocol. | missing | |
| Shell integration | it2cat | Generic inline-file display for any file, same proprietary protocol as imgcat. | missing | |
| Shell integration | it2dl | Downloads a file or directory from a remote/local session straight into the user's Downloads folder via the terminal transport. | missing | |
| Shell integration | it2ul | Uploads a file from the local machine into the session, opposite direction of it2dl, using a proprietary upload sequence. | missing | |
| Shell integration | it2copy | Copies stdin to the system clipboard via OSC 52, working through SSH/tmux hops. | partial | Tandem's alacritty_terminal core already parses OSC 52 for basic clipboard writes, but there is no it2copy equivalent utility or SSH/tmux-safe wrapping since Tandem has no tmux layer. |
| Shell integration | it2setcolor | Sets terminal foreground/background/ANSI palette colors from the shell via escape sequences. | missing | |
| Shell integration | it2check | Checks whether the current terminal understands iTerm2 proprietary escape codes before a script uses them. | missing | |
| Shell integration | it2attention | Requests user attention, dock bounce or fireworks animation, from a shell script. | missing | |
| Shell integration | it2universion | Reports/queries the terminal's declared Unicode version for width calculations. | missing | |
| Shell integration | it2getvar | Reads back a user-defined or built-in session variable from the shell. | missing | |
| Shell integration | it2profile | Switches the current session's profile from a shell command. | missing | |
| Shell integration | it2setkeylabel | Sets Touch Bar function-key labels from the shell. | missing | macOS Touch Bar only; not applicable on Windows. |
| Shell integration | it2ssh | A wrapper around ssh that propagates shell integration state across the SSH hop automatically. | missing | |
| Shell integration | it2tip | Shows an interactive contextual tip/hint in the terminal. | missing | |
| Shell integration | it2api | Python API CLI wrapper for scripting the app from the command line. | missing | |
| Shell integration | it2cli | A newer Swift-based CLI that embeds the utilities as one binary. | missing | |

### status bar
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Status bar | Status bar framework | An optional bar at top or bottom of a session showing a configurable, ordered row of components with per-component settings and priorities. | missing | |
| Status bar | Battery Level component | Shows current battery charge percentage/state. | missing | |
| Status bar | CPU Utilization component | Shows live CPU usage as text or a graphic/sparkline. | missing | |
| Status bar | Memory Utilization component | Shows live memory usage. | missing | |
| Status bar | Network Throughput component | Shows live network send/receive rates. | missing | |
| Status bar | Clock component | Shows the current time/date, configurable format. | missing | |
| Status bar | Custom Action component | A clickable button that runs a user-defined action, shell command, menu item, or script function, when clicked. | missing | |
| Status bar | Action Menu component | A dropdown menu of user-defined quick actions. | missing | |
| Status bar | Snippets Menu component | A dropdown menu to pick from saved text snippets and send them to the session. | missing | |
| Status bar | Triggers Menu component | A dropdown to quickly enable/disable/manage triggers for the current profile from the status bar. | missing | |
| Status bar | git state component | Shows current branch and dirty/clean state for the working directory, via shell integration git reporting. | missing | |
| Status bar | Hostname component | Shows the remote or local hostname, sourced from shell integration remote host reporting. | missing | |
| Status bar | Username component | Shows the current username, sourced from shell integration remote host reporting. | missing | |
| Status bar | Job Name component | Shows the name of the foreground process running in the session. | missing | |
| Status bar | Working Directory component | Shows the current directory, sourced from shell integration current directory reporting. | missing | |
| Status bar | Search Tool component | Embeds a find/search field directly in the status bar instead of an overlay. | missing | |
| Status bar | Filter component | Filters/hides scrollback lines that do not match a live query. | missing | |
| Status bar | Composer component | Embeds the multi-line Composer text-entry box in the status bar. | missing | |
| Status bar | Fixed-size Spacer component | A layout spacer of fixed width, for arranging other components. | missing | |
| Status bar | Spring component | A flexible/expanding layout spacer, for arranging other components. | missing | |
| Status bar | Interpolated String component | Shows arbitrary text built from an interpolated string expression referencing session variables. | missing | |
| Status bar | Call Script Function component | Shows text/value returned by a registered Python API script function, refreshed live. | missing | |
| Status bar | Setup/configure UI | A drag-and-drop editor for choosing, ordering, and configuring which components appear per profile. | missing | |

### toolbelt
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Toolbelt | Toolbelt panel | A dockable sidebar panel hosting a stack of tool widgets, shown/hidden per window. | missing | |
| Toolbelt | Actions tool | Lists user-defined quick actions, same actions as the status bar Action Menu, for one-click invocation. | missing | |
| Toolbelt | Captured Output tool | Lists text captured by Capture Output triggers, with click-to-jump to the source line. | missing | |
| Toolbelt | Command History tool | Lists commands run in the session, from shell integration prompt marks, searchable, grouped by host. | missing | |
| Toolbelt | Named Marks tool | Lists user-named marks (bookmarked lines) for quick navigation. | missing | |
| Toolbelt | Session Status tool | Shows live status/state info about the current session. | missing | |
| Toolbelt | Recent Directories tool | Lists recently visited, and optionally starred/pinned, working directories, click to cd or open. | missing | |
| Toolbelt | Jobs tool | Lists running child processes/jobs in the current session, with the ability to signal/kill them. | missing | |
| Toolbelt | Notes tool | A free-form notes pane, either global or scoped per session. | missing | |
| Toolbelt | Paste History tool | Lists recently pasted text snippets for reuse, disabled automatically when secure keyboard entry is on. | missing | |
| Toolbelt | Profiles tool | Lists all saved profiles with buttons to open one in a new tab, window, or split. | missing | |
| Toolbelt | Snippets tool | Lists saved reusable text snippets to insert into the session. | missing | |
| Toolbelt | Codecierge tool | An AI assistant panel that suggests or walks through commands, part of iTerm2's AI features. | missing | |
| Toolbelt | Dynamic web tools | User-added tools that embed an arbitrary web page inside the toolbelt as a persistent panel. | missing | |

### triggers
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Triggers | Trigger engine | Per-profile list of regex-matched rules evaluated against incoming terminal text, each firing an action. | have, milestone 6a |  |
| Triggers | Show Alert trigger | Pops a modal alert dialog with text derived from the match. | have, milestone 6a |  |
| Triggers | Capture Output trigger | Captures the matching line into the Captured Output toolbelt tool. | missing, milestone 9 (listed in the editor) |  |
| Triggers | Run Coprocess trigger | Launches an external command as a coprocess piped to/from the session on match. | have, milestone 6c |  |
| Triggers | Annotate trigger | Attaches an annotation to the matching text automatically. | have, milestone 6a |  |
| Triggers | Set Mark trigger | Drops a mark at the matching line for later navigation. | have, milestone 6a |  |
| Triggers | Make Hyperlink trigger | Turns matching text into a clickable hyperlink automatically. | have, milestone 6a |  |
| Triggers | Bounce Dock Icon trigger | Bounces the app icon in the Dock on match, to get attention. | have, milestone 6a: the taskbar button flashes |  |
| Triggers | Report Directory trigger | Feeds a matched directory string into iTerm2's own cwd tracking, an alternative to shell integration reporting. | have, milestone 6a |  |
| Triggers | Ring Bell trigger | Plays the terminal bell sound on match. | have, milestone 6b |  |
| Triggers | Invoke Script Function trigger | Calls a registered Python API script function with the match as an argument. | missing, milestone 11 (listed in the editor) |  |
| Triggers | Highlight Line trigger | Colors the whole matching line's background/foreground. | have, milestone 6a |  |
| Triggers | Highlight Text trigger | Colors just the matched substring. | have, milestone 6a |  |
| Triggers | Run Command trigger | Runs a shell command, not attached as a coprocess, when the pattern matches. | have, milestone 6a |  |
| Triggers | Report User & Host trigger | Feeds a matched user@host string into iTerm2's own host tracking, an alternative to shell integration remote host reporting. | have, milestone 6a |  |
| Triggers | Prompt Detected trigger | Manually marks a matching line as a shell prompt for terminals without shell integration installed. | have, milestone 6a |  |
| Triggers | Stop Processing Triggers trigger | Halts evaluation of any further triggers on the current line. | have, milestone 6a |  |
| Triggers | Set Title trigger | Sets the session's title from the match. | have, milestone 6a |  |
| Triggers | Open Password Manager trigger | Opens the password manager automatically on match, for example detecting a login prompt. | missing, milestone 10 (listed in the editor) |  |
| Triggers | Post Notification trigger | Posts a system notification on match. | have, milestone 6b |  |
| Triggers | Send Text trigger | Sends literal text back into the session as if typed, on match, for example auto-answering a prompt. | have, milestone 6a |  |
| Triggers | Fold trigger | Collapses matched lines into a folded/hidden block in scrollback. | have, milestone 6a |  |
| Triggers | Inject Data trigger | Injects data into the terminal's input stream as if it arrived from the pty, on match. | have, milestone 6a |  |
| Triggers | Set Named Mark trigger | Creates a named mark from the match, for the Named Marks toolbelt tool. | have, milestone 6a |  |
| Triggers | Set Profile Boolean trigger | Flips a boolean profile setting on match. | have, milestone 6a |  |
| Triggers | Set Tab Status trigger | Sets tab status/badge indicators from the match. | have, milestone 6a |  |
| Triggers | Set User Variable trigger | Sets an arbitrary user variable from the match, same namespace as shell integration SetUserVar. | have, milestone 6a |  |
| Triggers | Change Style (SGR) trigger | Applies an SGR text style/color change based on the match. | have, milestone 6a |  |
| Triggers | Enter/Exit Workgroup trigger | Groups sessions into a workgroup context based on matched text, used with AI/browser features. | missing | |
| Triggers | Buffer Input trigger | Buffers subsequent input until a condition is met, based on the match. | have, milestone 6a |  |

### captured output, annotations, marks and navigation
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Captured output | Capture Output pipeline | Text matched by a Capture Output trigger is stored as a record tied to the originating command mark. | missing | |
| Captured output | Captured Output toolbelt list | Displays all captured snippets in one list, click-to-jump to the source line in the buffer. | missing | |
| Annotations | Inline annotation | A user- or trigger-attached note bound to a range of terminal text, shown inline and editable, independent of triggers. | have | A row holds one annotation. |
| Annotations | Annotation from Named Mark | Auto-generates annotation text describing a named mark. | missing | |
| Marks and navigation | Command mark (prompt-based) | A mark automatically placed at each shell prompt via shell integration, carrying the command text, start/end time, and exit code. | have |  |
| Marks and navigation | Exit-code coloring | Colors a command mark green/red based on success/failure of the exit code, visible in the scrollbar gutter. | have | In the left margin (Tandem has no scrollbar). |
| Marks and navigation | Manual mark | A mark the user places explicitly, not tied to a command, for bookmarking a scrollback position. | have |  |
| Marks and navigation | Named mark | A manual or triggered mark with a user-supplied label, surfaced in the Named Marks toolbelt tool. | have | The Named Marks tool comes in milestone 9. |
| Marks and navigation | Image mark | A mark type used to anchor an inline image's position in the buffer. | have | Images live in their cells, so they move with their lines. |
| Marks and navigation | Fold mark | Marks a block of lines as folded/collapsed, paired with the Fold trigger. | have, milestone 6a adds the Fold to Named Mark trigger |  |
| Marks and navigation | Block mark / Button mark | Marks used by block-based UI, for example clickable inline buttons in output such as AI suggestions. | missing | |
| Marks and navigation | Next/previous mark navigation | Keyboard commands to jump the viewport to the next or previous mark in scrollback. | have | Tandem has scrollback but no mark concept to navigate between; plain scroll-by-page exists instead. |
| Marks and navigation | Mark rendering in scrollbar | Draws small colored tick marks in the scrollbar gutter representing command marks, for an at-a-glance command timeline. | partial | Marks show in the left margin; Tandem has no scrollbar. |
| Marks and navigation | Select output of last/given command | Uses command mark ranges to select exactly the output of one command for copy. | have |  |
| Marks and navigation | Clear buffer up to mark | Clears scrollback history up to, but not including, a specific mark. | have |  |

### find
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Find | Find bar | Opens the in-pane find bar. | have |  |
| Find | Find Next / Find Previous | Moves to next/previous match. | have | Find Next goes up, iTerm2's default. |
| Find | Smart case sensitivity | Default mode, case-insensitive unless the query has an uppercase letter. | have |  |
| Find | Case-sensitive substring mode | Forces exact-case plain substring matching. | have |  |
| Find | Case-insensitive substring mode | Forces case-insensitive plain substring matching. | have |  |
| Find | Case-sensitive regex mode | Query is treated as a regular expression, case sensitive. | have | Rust regex syntax, which covers the common ICU syntax. |
| Find | Case-insensitive regex mode | Query is treated as a regular expression, case insensitive. | have | Rust regex syntax, which covers the common ICU syntax. |
| Find | Use Selection for Find | Populates the find query from the current text selection. | have |  |
| Find | Clear Find | Clears the find query and highlighted matches. | have |  |
| Find | Find Globally | Searches across all sessions/tabs/windows at once, not just the active pane. | have |  |
| Find | Select Matches | Converts all current find matches into multiple text selections. | partial | A selection is one range in Tandem, so it runs from the first match to the last, and every match is copied, one per line. |
| Find | Find URLs | Special search mode that highlights only URL-shaped text. | have |  |
| Find | Find All Smart Selection Matches | Highlights every match of the active Smart Selection rule. | have |  |
| Find | Pick Result to Open | Lets you choose one match and open it, for example as a URL or file. | have |  |
| Find | Filter mode | Hides all scrollback lines that do not match the query, showing only matching lines. | partial | Shows the lines as plain text, without their colors. |
| Find | Filter progress indicator | Shows progress while a filter search runs over large scrollback. | missing | |
| Find | Jump to Selection | Scrolls the viewport to the current selection/match. | have |  |

### paste special / paste history
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Paste special | Tab transform | Converts tabs to spaces, configurable count, or leaves them, before paste. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Convert newlines | Rewrites line endings before paste. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Remove newlines | Strips all newlines, joining pasted text into one line. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Escape shell characters with backslash | Backslash-escapes shell metacharacters so pasted text is treated literally by the shell. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Remove control codes | Strips control characters from the pasted text. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Bracketed paste mode toggle | Chooses whether to wrap the paste in bracketed-paste escape sequences. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Base64 encode | Encodes the pasted payload as base64, for use with shell functions that decode it. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Regex substitution | Applies a user-defined regex find/replace to the pasted text before sending. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Wait for prompt | Delays each chunk of a multi-line paste until a shell prompt is detected, requires shell integration. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Convert Unicode punctuation | Normalizes smart quotes/dashes to plain ASCII equivalents. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste special | Chunked/slow paste with delay | Splits large pastes into chunks with a configurable delay between them, to avoid overwhelming the shell. | have | Milestone 9a/9b: the paste pipeline and the Advanced Paste window (iTerm2's controls, enabling rules, and remembered defaults). |
| Paste history | Paste History popup | Shows a searchable list of everything previously pasted. | missing | |
| Paste history | Paste History toolbelt tool | Toolbelt panel listing past pastes, selectable to re-paste. | missing | Tandem has no toolbelt. |
| Paste history | Erase paste history | Clears all remembered paste entries. | missing | |
| Paste | Plain paste (Ctrl+V / middle-click) | Directly injects clipboard text into the pty with no transform. | have | Milestone 9a: newlines to CR, unsafe control codes removed, bracketing when asked; 768 units every 0.01 s, a "Pasting..." bar with Cancel after 3 s, the multi-line, one-line, size, and tabs questions. |

### open quickly
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Open Quickly | Command palette launcher | Opens a fuzzy-search overlay across many item kinds at once. | missing | |
| Open Quickly | Search sessions | Jumps to any currently open session by title/content. | missing | |
| Open Quickly | Search windows | Jumps to any open window. | missing | |
| Open Quickly | Search profiles | Opens a new session/tab from a matching profile. | missing | |
| Open Quickly | Change profile in place | Re-applies a different profile's settings to the current session. | missing | |
| Open Quickly | Search saved arrangements | Restores a saved window arrangement by name. | missing | |
| Open Quickly | Search help topics | Jumps to a help/documentation entry. | missing | |
| Open Quickly | Search scripts | Runs an installed Python/AppleScript automation script. | missing | |
| Open Quickly | Search color presets | Applies a color preset to the current session. | missing | |
| Open Quickly | Search actions | Runs a user-defined Action, a saved key-action. | missing | |
| Open Quickly | Search snippets | Inserts a saved text snippet. | missing | |
| Open Quickly | Search script invocations | Runs a saved script function invocation. | missing | |
| Open Quickly | Search named marks | Jumps to a scrollback location the user bookmarked with a name. | missing | |
| Open Quickly | Search menu items | Finds and triggers any application menu command by name. | missing | |
| Open Quickly | Search bookmarks/URLs | Opens a saved URL bookmark. | missing | |

### profiles window (launcher)
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Profiles window | Open Profiles window | Opens a picker listing all saved profiles. | missing | Tandem has no profiles concept. |
| Profiles window | Open in new tab / window / pane | From the picker, launches the selected profile into a new tab, new window, vertical split, or horizontal split. | missing | |
| Profiles window | Tag filter sidebar | Filters the profile list by user-defined tags. | missing | |
| Profiles window | Search field | Live-filters the profile list by typed text. | missing | |
| Profiles window | Edit Profiles shortcut | Jumps straight from the picker into the profile's settings pane. | missing | |
| Profiles window | Close after opening | Option to auto-dismiss the picker once a profile has been opened. | missing | |
| Profiles window | New tabs open in new window | Toggle changes whether picking a profile opens a new tab in the current window or always a new window. | missing | |

### session restoration
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Session restoration | Restore windows/tabs/panes on relaunch | Recreates the full window/tab/split layout automatically the next time the app starts. | have |  |
| Session restoration | Restore scrollback contents | Optionally restores the actual on-screen and scrollback text, not just an empty pane layout. | have |  |
| Session restoration | Restore working directory per pane | Each restored pane returns to, or attempts to cd back into, its prior working directory, via shell integration. | have |  |
| Session restoration | Keep windows within current displays | When restoring, clamps window frames so they stay on currently connected monitors. | have |  |
| Session restoration | Reattach to still-running shell after crash | A job-server process keeps the shell alive independent of the app, so a crash or relaunch can reattach to the same live pty instead of losing the session. | have |  |
| Session restoration | Restore notice on quit | Quit confirmation tells the user their layout will come back next launch. | have |  |

### arrangements
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Arrangements | Save Window Arrangement | Saves the layout of all windows as the default arrangement. | have, milestone 7c |  |
| Arrangements | Save Current Window as Arrangement | Saves just the frontmost window as a named arrangement. | have, milestone 7c |  |
| Arrangements | Load Arrangement from File | Imports an arrangement saved to disk. | have, milestone 7c |  |
| Arrangements | Restore Window Arrangement (by name) | Lists every saved arrangement by name for one-click restore. | have, milestone 7c |  |
| Arrangements | Restore Window Arrangement as Tabs | Restores a saved multi-window arrangement collapsed into tabs of a single window. | have, milestone 7c |  |
| Arrangements | Change Profile in Arrangement | Bulk-reassigns all sessions in a saved arrangement to a different profile before/at restore. | have, milestone 7c |  |
| Arrangements | Arrangement trust gate | Prompts before running any startup commands embedded in an untrusted imported arrangement. | have, milestone 7c: a file with a Mark of the Web asks first, naming commands, initial text, and triggers |  |

### hotkey windows
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Hotkey windows | Global hotkey summon/dismiss | A system-wide key combo shows or hides a dedicated terminal window from any app. | have |  |
| Hotkey windows | Per-profile hotkey binding | Any profile can be assigned its own dedicated hotkey window. | have |  |
| Hotkey windows | Modifier-only activation | Can trigger on holding a modifier key alone, for example double-tap Option, instead of a full chord. | have |  |
| Hotkey windows | Auto-hide on focus loss | Window disappears automatically when it loses focus. | have |  |
| Hotkey windows | Show on app activation | Auto-hidden hotkey window reappears when the app is activated again. | have |  |
| Hotkey windows | Animated show/hide | Slide/fade animation when the hotkey window appears or disappears. | partial |  |
| Hotkey windows | Floating (always-on-top) | Keeps the hotkey window above all other windows. | have |  |
| Hotkey windows | Alternate shortcuts | A hotkey window can respond to more than one key combination. | have |  |
| Hotkey windows | Dock icon preference | Controls whether the app shows in the Dock while only a hotkey window is open. | missing | Maps to taskbar visibility on Windows. |
| Hotkey windows | Multiple independent hotkey windows | Several different profiles can each have their own hotkey window active simultaneously. | have |  |

### AI / chat features
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| AI features | AI Chats window | A dedicated chat sidebar/window for conversing with an LLM about the terminal session. | missing | |
| AI features | Multi-provider support | Supports OpenAI, Gemini, DeepSeek, Ollama (local), Apple Intelligence, and other providers, selectable per chat. | missing | |
| AI features | Explain this output | Sends recent terminal output to the AI and asks it to explain what happened or why a command failed. | missing | |
| AI features | AI-generated tab titles | Uses the AI to propose a short descriptive title for a tab based on its activity. | missing | |
| AI features | Command safety checker | Screens AI-suggested shell commands for danger, for example destructive rm, before allowing them to run. | missing | |
| AI features | Terminal hard rules / guardrails | Hard-coded limits on what an AI agent is allowed to do to the terminal, regardless of provider. | missing | |
| AI features | Agentic tool calling into the terminal | Exposes tools letting the AI check prompt state, run commands, read command history/output/screen contents, get terminal size, detect SSH, get cwd, set clipboard, insert text at cursor, read a man page, create a file, search/load a web page, and restart the session. | missing | |
| AI features | Streaming responses | Chat responses stream token by token instead of waiting for the full reply. | missing | |
| AI features | Custom API headers / keys | Lets the user supply custom auth headers and API keys per provider. | missing | |
| AI features | Withheld-key advice | Warns the user when no API key is configured and explains how to add one. | missing | |
| AI features | Chat history persistence | Chats and messages are stored in a local database and browsable later. | missing | |
| AI features | In-chat find | Search within AI chat transcripts. | missing | |

### Python API
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Python API | Session/window/tab control | Create tabs, split panes, send text, list sessions, activate/focus a session or window, close, reorder tabs, set tab layout. | missing | |
| Python API | Screen/buffer reading | Read the on-screen grid contents, capture a screenshot of a session, get the current shell prompt, list recent prompts. | missing | |
| Python API | Variables | Get, set, and monitor iTerm2/session/user-defined variables in real time. | missing | |
| Python API | Profiles | List profiles, read/write individual profile properties, react to profile-change events. | missing | |
| Python API | Notifications and monitors | Subscribe to keystroke events, and filter/suppress them, prompt events, and other async notifications. | missing | |
| Python API | Server-originated RPC / registered functions | Register Python functions that iTerm2 itself can call, for example from triggers or status bar, and invoke arbitrary registered functions. | missing | |
| Python API | Status bar components | Register a custom status bar component driven entirely from a Python script. | missing | |
| Python API | Color presets | Apply or enumerate color presets programmatically. | missing | |
| Python API | Preferences | Read and write app-level preferences via the API. | missing | |
| Python API | Broadcast domains | Get and set which sessions are grouped into a broadcast-input domain. | missing | |
| Python API | tmux integration control | Drive tmux integration mode, attach, list, and so on, through the API. | missing | |
| Python API | Menu items | Trigger any application menu command by identifier. | missing | |
| Python API | Session restart | Restart a session's underlying shell process. | missing | |
| Python API | Saved arrangements | Save/restore window arrangements via script. | missing | |
| Python API | Selection | Read or set the current text selection in a session. | missing | |
| Python API | Inject | Feed synthetic data into a session as if it were program output. | missing | |
| Python API | Generic property get/set | Generic get/set for object properties not covered by a specific message type. | missing | |
| Python API | Transactions | Batch multiple API calls into a single atomic transaction. | missing | |

### Instant Replay
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Instant Replay | Time-travel scrollback (DVR) | Records a rolling video-like buffer of every pane's screen state so you can scrub backward in time, independent of normal scrollback. | missing | |
| Instant Replay | Step backward/forward | Steps one frame back or forward through recorded frames. | missing | |
| Instant Replay | Scrub bar / seek | Drag a slider to jump to any point in the recorded timeline. | missing | |
| Instant Replay | Export replay range | Exports a saved range of frames from the DVR buffer. | missing | |
| Instant Replay | Clear Instant Replay buffer | Wipes the DVR recording. | missing | |
| Instant Replay | Per-pane independent buffers | Each session has its own DVR instance tied to its grid, not a global recording. | missing | |

### password manager
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Password manager | Local encrypted store (Keychain) | Stores account/password pairs in the macOS Keychain by default. | missing | Would need Windows Credential Manager or DPAPI-backed store instead of Keychain. |
| Password manager | 1Password integration | Reads accounts/passwords/OTP from a running 1Password app via its CLI/API. | missing | |
| Password manager | LastPass integration | Reads accounts and can send OTP codes from LastPass. | missing | |
| Password manager | Bitwarden integration | Reads accounts from Bitwarden CLI. | missing | |
| Password manager | KeePassXC integration | Reads accounts from a local KeePassXC database. | missing | |
| Password manager | Keeper integration | Reads accounts from Keeper password manager. | missing | |
| Password manager | Generic command-line adapter | Lets a user plug in any external password tool via a shell adapter. | missing | |
| Password manager | Send password into session | Selecting an entry types/pastes the password into the focused pane, with a masked echo probe so it does not print to screen. | missing | |
| Password manager | OTP / TOTP autofill | Reads a one-time-password field and can type it after the password. | missing | |
| Password manager | Trigger-driven auto-open | A trigger can be configured to open the password manager automatically when a password prompt is detected. | missing | |
| Password manager | Re-authentication gate | Requires the user to re-authenticate before reusing access to stored credentials after a timeout. | missing | |
| Password manager | Broadcast-safe password entry | The password manager's broadcast choice: echo-probes every broadcast session and sends the password only to those at a password prompt; ordinary Enter Password is never broadcast. | missing | Milestone 10 (password manager). |

### Composer
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Composer | Multi-line command input | A floating or docked text box for composing multi-line commands before sending them to the shell. | missing | |
| Composer | Syntax highlighting | Highlights shell syntax in the composer text as you type. | missing | |
| Composer | Command history search | Opens history filtered by a typed prefix, browsable from the composer. | missing | |
| Composer | Autocomplete / suggestions | Shows a ghost-text suggestion and a completions popup window while typing. | missing | |
| Composer | Send vs enqueue | Can send the composed text immediately or enqueue it to run after the current command finishes. | missing | |
| Composer | Send to Advanced Paste | Routes composed text through Paste Special instead of a plain send. | missing | |
| Composer | Copy with styles / control sequences | Special copy commands preserve formatting or literal control sequences from the composer buffer. | missing | |
| Composer | Status bar composer component | A compact composer can live embedded in the status bar as well as its own window. | missing | |

### broadcast input modes
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Broadcast input | Send Input to Current Session Only | Turns broadcasting off, restores normal single-pane typing. | have |  |
| Broadcast input | Broadcast to All Panes in All Tabs | Every keystroke is sent to every pane in every tab of the window. | have |  |
| Broadcast input | Broadcast to All Panes in Current Tab | Every keystroke is sent to every pane in the current tab. | have |  |
| Broadcast input | Toggle Broadcast Input to Current Session | Adds/removes just the current session from the active broadcast domain. | have |  |
| Broadcast input | Background pattern indicator | Optional diagonal stripe overlay rendered on panes currently receiving broadcast input, so it is visually obvious. | have |  |
| Broadcast input | Broadcast source designation | Marks a specific session as the source pane whose input gets mirrored elsewhere. | have |  |

### utilities scripts
| Subsystem | Feature | What it does | Tandem status | Windows note |
|---|---|---|---|---|
| Utilities | imgcat | Displays an image inline in the terminal using the iTerm2 inline-image protocol. | missing | |
| Utilities | imgls | Directory listing supplemented with inline image thumbnails and dimensions. | missing | |
| Utilities | it2api | Python-based CLI front end for driving the Python API, for example listing sessions, from the command line. | missing | |
| Utilities | it2attention | Requests user attention: bounces the dock icon, fires a fireworks visual effect, or a one-off bounce. | missing | |
| Utilities | it2cat | Prints a file to the terminal using the inline-image/file-transfer OSC protocol. | missing | |
| Utilities | it2check | Probes whether the terminal understands iTerm2's proprietary escape sequences before using them, via a device-status report. | missing | |
| Utilities | it2copy | Copies stdin to the system clipboard using the OSC 52/1337 copy sequence. | missing | |
| Utilities | it2dl | Downloads a file or directory from the remote/session to the local machine via the file-transfer protocol. | missing | |
| Utilities | it2getvar | Reads an iTerm2 session variable's value from the shell. | missing | |
| Utilities | it2git | Reports current git repo status as user-defined variables for status bar display. | missing | |
| Utilities | it2profile | Changes the current session's profile from the shell. | missing | |
| Utilities | it2setcolor | Sets terminal color scheme/individual colors from the shell. | missing | |
| Utilities | it2setkeylabel | Sets Touch Bar function-key labels from the shell. | missing | macOS Touch Bar only; not applicable on Windows. |
| Utilities | it2ssh | Wrapper around ssh that carries iTerm2 shell-integration/session state through to the remote host. | missing | |
| Utilities | it2tip | Shows a rotating did-you-know tip about iTerm2 features. | missing | |
| Utilities | it2ul | Uploads a file or directory from the local machine into the session via the file-transfer protocol. | missing | |
| Utilities | it2universion | Sets/pushes/pops the terminal's reported unicode version, for compatibility with apps sensitive to grapheme width rules. | missing | |

## Counts

Updated after milestone 3a (2026-09-24).

- Rows: 632
- have: 62
- partial: 6
- missing: 564
