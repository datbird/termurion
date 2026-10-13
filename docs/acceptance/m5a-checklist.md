# Milestone 5a acceptance checklist

Close any open Termurion window, then run `C:\Users\datbi\src\termurion\target\release\tandem.exe`.
Use a WSL tab for the image tools (`imgcat` from iTerm2's utilities, `img2sixel` from
libsixel-bin, `kitten icat` from kitty). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `imgcat some.png`: the picture shows at the cursor, at its own size, and the prompt comes back under it. A picture wider than the pane shrinks to the pane width. | |
| 2 | `imgcat -W 20 some.png` and `imgcat -H 50% some.png`: the picture takes 20 columns, then half the pane height, keeping its shape. | |
| 3 | Print enough lines to scroll the picture into the scrollback, then scroll back: it scrolls with the text. Clear Buffer (Win+K): it goes away. | |
| 4 | `imgcat some.gif` with an animated GIF: it plays. | |
| 5 | `printf '\e]1337;File=inline=1:bm90IGFuIGltYWdl\a'`: a grey box with a red cross (the broken image) shows. | |
| 6 | `it2dl somefile.txt` (or `printf '\e]1337;File=name=%s;size=5:aGVsbG8=\a' $(echo -n hi.txt \| base64)`): Termurion asks whether to download it; Yes saves it to your Downloads folder and says where; a second download of the same name becomes `hi (1).txt`. No saves nothing. | |
| 7 | `img2sixel some.png`: the sixel picture shows. `echo -e '\e[c'` answers with `62;4;22` (sixel is there). | |
| 8 | `kitten icat some.png`: the picture shows. `kitten icat --clear` removes it. | |
| 9 | Select across a line with a picture and copy: the picture cells give no text. Type over part of a picture: those cells become text. | |
| 10 | Resize the window narrower and wider with a picture on screen: no crash, the picture stays in its cells. | |
