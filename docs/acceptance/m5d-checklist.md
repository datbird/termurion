# Milestone 5d acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Profiles > Window: set a background image (a photo). The pane shows it under the text, dimmed by the background color. Move Blending: toward Image it shows more. Colored cells (`printf '\e[41m red \e[0m'`) cover it. | |
| 2 | Try each scaling mode: Stretch fills, Tile repeats at full size, Scale to Fill crops, Scale to Fit shows the whole image with bars. | |
| 3 | Split the pane: each pane shows its own whole image (the default "Separate background images per pane"; its switch comes with the Appearance settings page in milestone 13). A split made from a pane a program changed (step 7) starts with the saved profile, not the change. |
| 4 | Source: Folder Rotation, a folder of 3 images, interval 5: the image changes every 5 seconds and each shows once before any repeats. | |
| 5 | Point the image at a file that is not an image: the pane shows its plain background, no error, no crash. | |
| 6 | `printf '\e]1337;SetProfile=Default\a'` (use a profile name you have): a bar asks "A control sequence attempted to change the current profile. Allow this in the future?". Press Alt+A (Always Allow): nothing changes this time. Send it again: the pane switches profile. | |
| 7 | `printf '\e]1337;SetProfileProperty=Blend=%s\a' "$(printf 0.9 | base64)"` with a background image: a bar says "An app tried to change the profile property Blending." Allow Once: only this pane's image gets stronger; other panes on the profile keep theirs. Edit another setting of that profile in Settings: this pane follows that edit but keeps its Blend. | |
| 8 | `printf '\e]1337;SetProfileProperty=Cursor Type=%s;Blend=%s\a' "$(printf 9 | base64)" "$(printf 0.1 | base64)"`: nothing happens (9 is not a cursor type, so the whole request is refused). | |
| 9 | `printf '\e]1337;SetBackgroundImageFile=%s\a' "$(printf 'C:\\Windows\\Web\\Wallpaper\\Windows\\img0.jpg' | base64)"` (a real path): a bar asks to set the image; Yes shows it in this pane only. With an empty value the bar asks "Remove background image?". | |
| 10 | Press any key while a bar that says "Press any key to dismiss this message." is showing: the bar goes, and the key does not reach the shell. | |
| 11 | `printf '\e]1337;OpenURL=:%s\a' "$(printf https://iterm2.com | base64)"`: a bar asks "Open this URL? https://iterm2.com". Always allow for this host: the browser opens; the next one for that host opens without asking. | |
| 12 | `printf '\e]1337;ReportVariable=%s\a' "$(printf columns | base64)"; read -rs -t10 -d $'\a' x; echo "$x" | cat -v`: the first time the reply is empty and a bar asks about "columns"; Always Allow; repeat: the reply carries the column count in base64. | |
| 13 | Install iTerm2's `it2ul` (from its utilities) in WSL, run `it2ul`: a file picker opens. Pick two small files: they arrive in the current folder, and a message shows the upload's progress. Run it again and Cancel: `it2ul` stops. | |
| 14 | During a large upload press Ctrl+C: a bar asks "A file is being uploaded. Abort the upload?"; OK stops it. | |
| 15 | Shell > Page Setup... (Win+Shift+P): the Windows page setup dialog opens; set landscape. | |
| 16 | Shell > Print > Buffer (Win+Alt+Shift+P): a box asks "Print in black and white?"; Yes, then print to Microsoft Print to PDF. The PDF has the whole buffer in the profile's font, landscape, no colors. Print again answering No: colors print behind the text. Print > Selection with nothing selected beeps. | |
