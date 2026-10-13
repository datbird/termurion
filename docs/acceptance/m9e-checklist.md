# Milestone 9e acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Shortcuts > Snippets: + opens "Edit Snippet" (with text on the clipboard, the value is that text and the title its first 40 characters). Make `ls -la` titled "list", no tags; OK adds the row. | |
| 2 | Add "deploy" with tags `work/git, home` and "status" with tag `work`. Edit > Snippets shows the disabled "Press Option to Edit Before Sending", a line, "list", a line, then folders "home" and "work"; "work" holds folder "git" (with "deploy"), a line, then "status". | |
| 3 | Choose Edit > Snippets > list: `ls -la` is typed into the shell (not run: the snippet has no newline). Make a snippet whose value is `echo hi\n` (C-Style Backslash Escaping): choosing it runs `echo hi`. | |
| 4 | Hold Alt while choosing a snippet: Advanced Paste opens with the snippet's text (it is also on the clipboard). | |
| 5 | In the profile's Advanced settings set Snippets Filter to `work`: Edit > Snippets shows only "deploy" and "status" for that session; clear it and all return. | |
| 6 | Snippets tab: the search `tag:work -title:status` shows only "deploy"; `^ls` shows "list". Right-click a row: Duplicate, Delete, Add Above, Add Below, Edit all work. Drag a row (empty search) to reorder; Edit > Snippets follows. Delete or Backspace removes the selected rows. | |
| 7 | Select two snippets and Export... to a `.it2snippets` file; delete them; Import... the file: they return. Import a file holding `{}`: "Import Failed" with "Malformed file at ...". | |
| 8 | Close and reopen Termurion: the snippets are still there (`%APPDATA%\Tandem\snippets.json`). | |
| 9 | Settings > Keys: bind a key to "Send Snippet"; the parameter is a snippet picker. The table reads "Send Snippet “list”". Press the key: `ls -la` is typed. Delete that snippet: the table reads "Send Deleted Snippet (no action)" and the key does nothing. | |
| 10 | Settings > Shortcuts > Actions: + opens "Edit Action". Make "hello" = Send Text `echo hello\n`, "Apply to all sessions in tab". Edit > Actions > hello in a tab split in two: both panes run `echo hello`, once each. The table's Action column reads "In all sessions in the tab, Send Text: ...". | |
| 11 | Actions Export... and Import... round-trip through a `.it2actions` file. | |
| 12 | A key binding for Send Text with "Apply to all sessions" sends to every session in every window. | |
