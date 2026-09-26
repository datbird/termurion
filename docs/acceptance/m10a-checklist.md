# Milestone 10a acceptance checklist

Close any open Tandem window. Copy `tandem.exe`, `tandem-host.exe`, `tandem-git.exe`,
`conpty.dll`, and `OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` together,
then run `tandem.exe`.

Setup: on the profile you use, bind a key to key action 15 ("Start Instant Replay", Settings >
Keys > Key Bindings), in addition to the default Win+Alt+B. Open a second window. Have a tmux
session attached in one pane of the first window. This pass is the first look at a real screen
(every task deferred launching the GUI on EXTREME, since the rules forbid it while the machine
may be in use). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In a pane, run `dir /s C:\Windows` to generate a burst of output, let it finish, then press Win+Alt+B (View > Start Instant Replay) or your bound key: a floating panel titled "Instant Replay Control" appears below the pane, and the screen freezes on the second-newest recorded frame (kept iTerm2 bug, not the newest). | |
| 2 | Press Left: the screen steps back one frame. Press Shift+Left: it steps back 15 frames (beeping if fewer than 15 remain and stopping at the oldest). Press Right and Shift+Right: the same, forward. Press any plain letter key: it beeps and nothing changes. Press Esc: the panel closes and the pane returns to live, showing the shell's current output, not a stale frame. | |
| 3 | While stepping through frames, watch the panel's slider: its thumb tracks the current position, and short tick marks along the slider mark every recorded frame's timestamp (the ticks are computed once, when the panel opens, and do not move or add new ticks while it stays open, even as new frames are recorded; kept iTerm2 bug). | |
| 4 | While replaying, the panel's center label reads a time (and a date, if not today); at the far left it reads the earliest recorded time; at the far right it reads "Now". Press Esc to return to live, then reopen the panel with nothing yet stepped back: the center label reads "Live View" and the slider sits at its right end. | |
| 5 | Click "Export...": the buttons change to "Set Start" and a "Cancel" button appears; the slider jumps to the earliest frame. Click "Set Start": the button becomes "Set End"; the slider jumps to "Now". Drag the slider left of the start point and click "Set End" again: the panel beeps and stays in the Set End step (no dialog). Drag back past the start point and click "Set End": a Save dialog appears, defaulting to "Recording.itr" in your home folder. | |
| 6 | Reopen the panel, click "Export...", then "Set Start", then press Esc: the panel closes and the pane returns fully to live (Esc always leaves replay, even mid-export; it never just backs out one export step). | |
| 7 | With something recorded and not replaying, choose Session > Log > Export Recording: the same Save dialog appears and writes a `.itr` file. | |
| 8 | Start a replay (Win+Alt+B), then choose Session > Log > Export Recording from the menu while still replaying: the Save dialog appears, then an info bar or dialog reads "Error encoding recording." (kept iTerm2 bug: the menu command targets the replay's own synthetic session, which has nothing to export). While still replaying, click the panel's own "Export..." button instead: it exports the live pane's recording normally, with no error. | |
| 9 | Choose Session > Log > Import Recording and pick the `.itr` file saved above: a new tab opens at index 0 of the current window, already replaying that recording. Press Esc: the tab closes (a loaded recording has no shell process to keep open). | |
| 10 | On a pane with nothing recorded (a freshly started session, before any output), choose Session > Log > Export Recording: the Save dialog still appears and writes a valid (empty) file. Import that file: a new tab opens at index 0 showing a blank screen with no replay panel (nothing to step through), and it closes normally. | |
| 11 | Press Win+Alt+K (Edit > Clear Instant Replay) on a live pane with something recorded, then immediately Win+Alt+B: the panel opens but there is nothing to step back through (the buffer is empty). | |
| 12 | Start a replay, then press Win+Alt+K while still replaying: nothing is cleared (kept iTerm2 bug: Clear Instant Replay during a replay targets the replay's own session, which has no recorder); stepping and exporting from that pane still work once you return to live. | |
| 13 | Set "Memory usage per session (in MB)" (General > Instant Replay) to 0 and start a brand-new session (new tab or window): produce output, then Win+Alt+B: nothing was recorded, so the panel opens with nothing to show. An existing session already running when you changed the setting keeps its old buffer size until it is restarted. | |
| 14 | While replaying (panel open), try to split the pane (Win+D or Win+Shift+D): the split is refused, nothing happens. | |
| 15 | While replaying, switch to a different pane or a different tab: that pane's replay panel closes, that pane returns to live, and switching back to the original pane's tab reopens its panel still mid-replay. | |
| 16 | In the tmux-attached pane, run a command, then Win+Alt+B: the tmux pane records and replays the same way an ordinary pane does. | |
| 17 | Open a background tab (not the frontmost one) running a busy command (for example a loop printing text), leave it in the background for several seconds, bring it forward, and Win+Alt+B: it recorded while backgrounded (at roughly once a second, sparser than the frontmost tab's much faster rate), and stepping back shows its earlier output. | |
| 18 | Resize the Instant Replay panel wider by dragging its right edge: only the slider and the tick strip above it stretch; the "Time", "Now", and current-time labels and the Export/Cancel buttons stay put at their original positions (xib autoresizing) instead of moving with the edge. | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 19 | First Start Instant Replay shows the second-newest recorded frame, not the newest (see check 1). | |
| 20 | A replay whose current frame has since been evicted from the buffer keeps showing that stale frame; pressing Right from there jumps all the way to the oldest frame still held, and Left beeps instead of stepping further back. | |
| 21 | Clear Instant Replay during a replay strands the replay and clears nothing; the buffer is only cleared the next time it is run from a live pane (see check 12). | |
| 22 | An exported range restamps every frame in it as a key frame, timestamped at the moment of export, not its original recording time. | |
| 23 | The panel's tick marks are computed once when it opens and never move or gain new ticks while it stays open, even as more frames are recorded (see check 3). | |
| 24 | After Clear Instant Replay during a replay, the panel's label switches to "Live View" but the stale frame stays on screen underneath it (iTerm2 shows a 1970 date in the same situation instead). | |
| 25 | Menu Export Recording run during a replay shows the Save dialog and then "Error encoding recording."; the panel's own Export button still works during a replay (see check 8). | |
| 26 | Esc always leaves replay outright, even in the middle of picking an export range (Set Start or Set End); it never just cancels one export step back to Normal (see check 6). | |
| 27 | Widening the panel stretches only the slider and its tick strip; the labels and buttons keep their original positions (see check 18). | |
| 28 | Exporting an empty recorder writes a valid, empty `.itr` file, and importing it opens a blank tab with no replay to step through (see check 10). | |
| 29 | The panel is fully opaque; iTerm2's 90% transparency (brightening to 100% on hover) is not built. | |
| 30 | Split selection (Move Session to Split Pane) is not implemented in Tandem, so the closer that would end a replay when a split-selection pick starts cannot be exercised; only its pure rule is tested. | |
| 31 | Closing Tandem, restarting it, or saving/restoring a window arrangement never brings back a tab opened by Import Recording; that tab is left out of every saved arrangement the same way iTerm2 leaves out its own synthetic replay sessions. | |
| 32 | A `.itr` file Tandem exports cannot be opened by real iTerm2, and an iTerm2 recording cannot be imported into Tandem; the two use unrelated file formats. | |
| 33 | Recording speed follows iTerm2's own cadence, not a flat rate: idle or backgrounded panes record about once a second, the frontmost pane under heavy output records up to 60 times a second, and a pane on the alternate screen (an editor or pager) always records every 16 ms regardless of throughput. | |
| 34 | A frame recorded at a different pane size than the one replaying it is cropped or padded to fit; unlike iTerm2, the window is never resized to match the recorded frame. | |
