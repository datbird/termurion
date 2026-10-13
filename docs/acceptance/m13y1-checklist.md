# Milestone 13y1 acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.

Setup: a git repository with at least two commits and an uncommitted change. Claude Code installed
for the Claude steps; otherwise use a stand-in command (a batch file named `claude.cmd` on PATH that
prints and waits). Rows 7, 9, 13, 14, 15 and 16 cover what tests could not execute (the moved-member
peer swap, the restore question flow, the DPAPI key, Import All and auto-send); do them on a real
desktop. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Arrangements > Workgroups: Add the "Coding Agent + Diff + Code Review" preset; it shows a main session with Diff and Code Review peers. Rename it; the tree and Shell > Workgroups show the new name. | |
| 2 | In the editor add a split and a tab to the main session, then Undo and Redo with the buttons and with Ctrl+Z and Ctrl+Y. (Edit > Undo does not reach Settings; ruled, not a failure.) | |
| 3 | Shell > Workgroups lists the workgroup. Entering it on a session (a workgroup you made yourself) shows the toolbar with Chat, Diff and Code Review; a workgroup that came from an import or a trigger first asks, listing every command it would run (default button Cancel). | |
| 4 | In a git repository with a clean tree, press Alt+Shift+Win+2: the Diff peer shows the waiting panel. Change a file; the panel goes away and the diff starts on its own. | |
| 5 | Pick a file in the changed-file pulldown: the Diff peer shows that file. Change the base to HEAD~1: the diff and the file list follow the new base. | |
| 6 | Alt+Win+[ and Alt+Win+] cycle the peers in place (no resize of the shown pane); a peer never started is skipped. | |
| 7 | Moved-member peer swap: with a workgroup entered, drag its pane (or use Move Session to another tab or window, Swap with another pane, Break Out). The member moves like any pane; then press Alt+Win+] and [: the peers still swap in the pane's new place and the toolbar follows. Close it: the group closes with it. A hidden peer cannot be dragged. | |
| 8 | The Code Review peer shows a prompt overlay (pick a saved prompt or type one). Start runs the review in the peer with the prompt as one argument; the overlay goes away. | |
| 9 | Turn on Auto-Send Clippings and Auto-Request Review in the toolbar of a leader that runs a coding agent (Claude Code, else a stand-in named claude): a review is pasted back into the agent's session and submitted after the delay. Auto-send to a coding agent: also confirm that with a plain shell prompt in the main session (agent exited) nothing is pasted, and that a transport such as wsl or ssh in the main session gets nothing. | |
| 10 | Close the leader while the workgroup is entered: the warning reads "This will also exit the workgroup ..."; Cancel keeps everything. | |
| 11 | Exit Workgroup (Shell > Workgroups): the tab's title and group are restored and the other members close. | |
| 12 | Save an arrangement (Window > Save Window Arrangement) with the workgroup entered, quit, open the arrangement: because it is a named arrangement it asks before restoring an imported workgroup (your own is restored); the shown and buried members come back, split and tab children are plain panes (ruled). | |
| 13 | adopt_one restore question flow: import a workgroup (Import All of a Termurion export that holds one, or a remote-prefs folder; an iTerm2 export and a Dynamic Profile carry none) and enter it, say Yes to the question, quit Termurion with Restore Windows on, start it again: the window comes back and a question lists the commands once more (same commands, sealed approval, so it may restore unasked: record which). Edit one of its commands in Settings, restart: you are asked again. Answer No: the buried members' shells end and the anchor is a plain session. Answer Yes while another workgroup is entered in that window: nothing is left hidden. | |
| 14 | Review and Diff peers after a restart: they come back as idle sessions (no program) with their overlay; Start or the Diff selection runs them. Nothing runs at the restore by itself. | |
| 15 | DPAPI key: with Termurion closed delete workgroup-approval-key beside the settings file, start Termurion: a fresh key is made and every imported workgroup's restore asks again. Replace the file with 32 plain bytes (or copy it from another user or machine): it is refused and replaced the same way, and everything asks. | |
| 16 | Import All refusing an aliased name: make an export archive, add a file named `notes.` (trailing dot), another `a.txt:stream`, and one named WORKGR~1; Import All refuses the whole archive with the name shown, and imports nothing; remove the names and it imports. Check again after a restart that nothing half-imported. | |
| 17 | iTerm2 > Install Claude Code Integration shows Install Workgroup and Auto-Enter Workgroup (already installed ones checked); install both. | |
| 18 | Running `claude` in a session (Auto-Enter on) asks to enter the workgroup (the Enter trigger always asks); Enter it; quitting claude exits it. | |
| 19 | The Cockpit (Alt+Win+Shift+C) with Workgroup grouping shows the workgroup with members titled "Diff: ..." and "Code Review: ..."; double-clicking the workgroup row reveals the Chat session. | |
| 20 | An orchestration chat's `@wg-` mention shows the workgroup's name; add_workgroup_clipping on the workgroup id lands in the Chat session's clippings. Orchestration cannot enter a workgroup, and start_code_review on a busy review refuses. | |
| 21 | A workgroup whose Tab member uses a Browser profile with a URL opens that URL. | |
| 22 | A Diff file named `50%.txt` is picked and diffed in any leader (cmd.exe or PowerShell): the name goes to git as one argument, no shell quotes it. A workgroup whose per-file command runs a `.cmd` or `.bat` refuses that name with a banner. | |
| 23 | A repository whose .git/config names a program (diff.external, core.pager, core.fsmonitor, a gpg.program): the Diff peer refuses to start git with a banner and runs no program from the config. | |
| 24 | The Imported mark: Settings > Arrangements > Workgroups shows an imported workgroup as imported; editing it keeps the mark; only Trust this workgroup clears it. | |
| 25 | Uninstall Claude Code Integration removes the workgroup and the two triggers (asking Keep or Remove if the workgroup was edited). | |
