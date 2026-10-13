# Milestone 5b acceptance checklist

Close any open Termurion window, then run `C:\Users\datbi\src\termurion\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`) for its tools. Mark each line
pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In WSL, `printf '\e[>4;2m'; cat -v`, then press Ctrl+1 and Shift+Enter: `cat` shows `^[[27;5;49~` and `^[[27;2;13~`. Session > Terminal State shows modifyOtherKeys Mode 2 checked. Ctrl+C, then `printf '\e[>4;0m'`. | |
| 2 | Session > Terminal State > CSI u Mode, then `cat -v` and press Ctrl+I and Alt+A: `^[[105;5u` and `^[[97;3u`. Ctrl+A still gives `^A`. Pick Standard Key Reporting Mode again. | |
| 3 | `printf '\e[>1u'; cat -v`, then press Esc and Ctrl+A: `^[[27u` and `^[[97;5u`; plain letters type as usual. Ctrl+C, then `printf '\e[<u'`: keys are back to normal. | |
| 4 | `printf '\e[>11u'; cat -v`, hold Ctrl and press A: a press, then `^[[97;5:3u` on release. Hold A with Ctrl for a second: repeats show `:2`. Release Shift alone: `^[[57441;1:3u`. Then `reset`: keys are back to normal. | |
| 5 | Session > Terminal State > Report All Event Types (with no program flags): the item is checked, and `cat -v` shows releases for Esc. Pick it again: unchecked. | |
| 6 | Session > Terminal State > Raw Key Reporting Mode, `cat -v`, press A: `^[]1337;d;1;61;65;61^G` then `^[]1337;u;...^G` on release. Press Shift alone: `^[]1337;f;2^G`. Terminal State > Reset: back to standard keys. | |
| 7 | On a German keyboard layout (add it in Windows if needed), in CSI u Mode and with `printf '\e[>1u'`, AltGr+Q types `@`, not an escape code. | |
| 7b | On the German layout, `printf '\e[>8u'; cat -v`, then AltGr+Q: note what arrives (`^[[64u` or `^[[113u`). On a US layout in CSI u Mode, Ctrl+Alt+A gives `^[[97;7u`, not a plain `a`. | |
| 7c | Type `reset` after picking Raw Key Reporting Mode in the menu: the menu choice stays (only Terminal State > Reset clears it). | |
| 8 | Settings > Profiles > Keys: "Apps can change how keys are reported" off: step 3's `printf '\e[>1u'` no longer changes keys. "Report keys using CSI u" on: a new tab starts in CSI u Mode. Set both back. | |
| 9 | Session > Terminal State > Literal Controls, then `printf 'a\tb\n'`: the tab shows as a reverse `^I` and the new line as `^J`. Turn it off: output is normal. | |
| 10 | `printf '\e[c'; read -rs -t1 -d c x; echo "$x" | cat -v` shows `^[[?64;1;2;4;6;17;18;21;22;52`. Session > Terminal State > Emulation Level > VT100, then again: `^[[?1;2`. Set VT500 back. | |
| 11 | `printf '\e[>q'; read -rs -t1 x; echo "$x" | cat -v` shows `Termurion` and the version. `printf '\e[1337n'` answers `ITERM2`. | |
| 12 | `printf '\e[?996n'; read -rs -t1 -d n x; echo "$x" | cat -v` shows `997;1` on a dark profile. `printf '\e[?2031h\e]11;#ffffff\a'` answers `997;2` at once. | |
| 13 | `printf '\e[?2026hHELLO'` and wait: HELLO shows within a fraction of a second although the update never ended. | |
| 14 | Run `btop` or `nvim` (they use synchronized output): no tearing while they redraw. | |
