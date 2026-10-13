# Inline Images

Termurion can show pictures inside the terminal grid. A program sends an image, and the image takes up a block of cells that scrolls with the text. Termurion understands the iTerm2 inline images protocol (OSC 1337 File), sixel graphics, and the kitty graphics protocol.

## Show an image with imgcat

Termurion ships `imgcat` in its utilities folder. The folder is added to the end of PATH in every new session, so you can run it from any shell:

```
imgcat photo.png
imgcat -W 40 -H 20 photo.png
imgcat -W 50% photo.png
```

`imgcat` also reads an image from standard input:

```
type photo.png | imgcat
```

Useful options:

| Option | What it does |
|---|---|
| `-W N`, `-H N` | Set the width or height. A plain number is character cells. `Npx` is pixels. `N%` is a percent of the session. `auto` picks a size. |
| `-r` | Keep the image's shape when it is scaled (the default). |
| `-s` | Stretch the image to the width and height you gave. |
| `-p`, `-n` | Print or do not print the file name after each image. |
| `-u` | Treat the following arguments as URLs. |
| `-t type` | Give a type hint (a MIME type or an extension such as `.png`). |
| `-l` | Use the legacy single-sequence protocol. |

Run `imgcat --help` for the full list. A related tool, `imgls`, lists a folder with a thumbnail next to each image. See [Shell Integration](shell-integration.md) for how the utilities folder is set up.

## Supported formats

Termurion decodes PNG, JPEG, GIF, BMP, WebP, TIFF, and ICO files.

- A GIF plays its frames. A GIF keeps at most 1,000 frames.
- An image can be at most 8192 pixels on a side.
- An image is at most 255 rows tall.
- A file that does not decode shows a placeholder.

## Allow or deny

When a program sends an inline image, Termurion pauses that pane's output and asks whether to allow it. The question shows the file name and size. If you allow it, the image shows and output continues. If you deny it, the image is dropped. The same kind of question appears for file downloads (see `it2dl`). A downloaded file is only saved after you agree.

## Sending images from your own program

The protocol is a single escape sequence:

```
ESC ] 1337 ; File = [arguments] : base64-data BEL
```

The arguments are separated by semicolons. Termurion reads these:

| Argument | Meaning |
|---|---|
| `inline=1` | Show the file in the terminal. Without it, the file is a download. |
| `name=` | The file name, in base64. |
| `size=` | The size in bytes. Used in the allow question. |
| `width=`, `height=` | A number of cells, `Npx`, `N%`, or `auto`. |
| `preserveAspectRatio=0` | Stretch instead of fitting inside the box. |
| `type=` | A MIME type, extension, or language name. |

Termurion also accepts the multipart form (MultipartFile, FilePart, FileEnd) that `imgcat` uses for large files.

## Sixel and kitty graphics

Programs that draw with sixel (`ESC P ... q`) or with the kitty graphics protocol work without any setting. Sixel pixels are drawn opaque.

## Turn it off

The Advanced setting "Disable potentially insecure escape sequences" turns off the File sequence, along with several other sequences. With it on, inline images and file downloads stop working. Leave it off unless you are viewing content you do not trust. Open Settings with Ctrl+Shift+, and search for the setting by name.

## Related pages

- [Shell Integration](shell-integration.md)
