# FX Profiles

The cursor effects (the render plugins) need to know how an app lays out its screen. Where is the cursor? Where is the input box? Which character marks the prompt? An FX profile holds those answers for one app. It does not choose which effects run. It only tells them how to read the app.

Termurion ships six profiles and picks one for each pane on its own. You can edit them, add your own, and pick one by hand for a pane.

## The six shipped profiles

| Profile | Reads | Recognized by |
|---|---|---|
| Shell | A plain shell prompt. The fallback for everything. | Nothing. It is used when no other profile matches. |
| Claude Code | Claude Code, with its input between two lines. | The program `claude`, the text "Claude Code", the prompt `❯`. |
| Codex | Codex, with its input on a shaded band. | The program `codex`, the text "OpenAI Codex", the prompt `›`. |
| opencode | opencode, with its input on a shaded band. | The program `opencode`, the text "OpenCode". |
| Hermes | Hermes, with its input between two lines. | The texts "Hermes Agent" and "Hermes", the prompt `❯`. |
| agy | Antigravity, with its input between two lines. | The text "Antigravity", the prompt `>`. |

## The folder

The profiles are plain TOML files, one per profile, in a folder named `FXProfiles` beside the settings file:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\Termurion\FXProfiles` |
| macOS | `~/Library/Application Support/Termurion/FXProfiles` |
| Linux | `~/.config/termurion/FXProfiles` (or `$XDG_CONFIG_HOME/termurion/FXProfiles`) |

Termurion writes the six shipped files there the first time it starts. Open Settings with Ctrl+Shift+, and go to FX Profiles, then press Show Folder to open it.

You can edit a file in any text editor. Termurion picks up a change when the pane has activity (output, a key, or the window gets focus), and while the FX Profiles page is open. It does not watch the folder on a timer.

Limits: at most 64 profiles load. A file must have a name that no other profile has. An empty file, a file with a syntax error, or a value out of range is not loaded. The FX Profiles page lists it with its file name, the line, and the reason. Termurion never trims a value to fit.

A key you leave out takes the Shell value. A key Termurion does not know is kept and not used. It shows as a warning, and a save from the page writes it back.

## Every key

Each key is shown with its Shell default and its limit. A glyph list is one string, so `"❯>›"` is three glyphs.

### Top level

| Key | Default | Meaning |
|---|---|---|
| `name` | `"Shell"` | The name the lists and menus show. 1 to 100 characters in the file (the page allows 64). |

### `[match]`

How Auto detection recognizes the app. A hit on any of these counts.

| Key | Default | Meaning |
|---|---|---|
| `program` | none | Program names, as a list. Matches the name of the program running in the pane. Up to 16 entries of 1 to 200 characters. |
| `text` | none | Texts, as a list. Matches text anywhere in the pane's visible screen. Up to 16 entries of 1 to 200 characters. |
| `prompt` | none | Prompt glyphs. Matches when one of them is the nearest character left of the cursor. Up to 32 glyphs. |
| `shape` | not used | A box shape that counts as a match: `"lines"`, `"band"`, `"either"`, or `"none"`. |

### `[cursor]`

| Key | Default | Meaning |
|---|---|---|
| `kind` | `"either"` | Which cursor the effects follow: `"real"` (the terminal's own), `"painted"` (one the app draws), or `"either"`. |
| `mark` | `"reverse"` | How an app paints its cursor: `"reverse"` (inverse video) or a background color like `"#2c2c2c"`. Write one, or a list of them (1 to 32). |
| `echo_anchor` | `true` | Keep the effect on the typed text while it echoes back. |

### `[box]`

The box around the input.

| Key | Default | Meaning |
|---|---|---|
| `kind` | `"either"` | `"lines"` (a line above and below), `"band"` (a shaded block), `"either"`, or `"none"`. |
| `line_glyphs` | `"─━"` (U+2500, U+2501) | Characters a box line is drawn with. Up to 32 glyphs. |
| `line_fill` | `0.8` | How much of a row a line must cover. 0.1 to 1. |
| `bottom_within` | `2` | How many rows below the cursor the bottom line may be. 0 to 50. |
| `band_rows` | `[2, 12]` | The fewest and most rows a band has. Each 1 to 50, the first not over the second. |
| `band_max_screen` | `0.5` | The most of the screen's height a band may fill. 0.1 to 1. |
| `band_min_width` | `0.5` | The least of the width a band must cover. 0.1 to 1. |
| `band_tolerance` | `2` | How many rows may differ in color inside a band. 0 to 50. |
| `edge_glyphs` | `"▀"` (U+2580) | Half block characters that end a band as an edge row. Up to 32 glyphs. |

### `[prompt_mark]`

| Key | Default | Meaning |
|---|---|---|
| `glyphs` | `"❯>›"` | Characters that count as the prompt mark at the start of the input. Up to 32 glyphs. An empty string means the app has none. |

### `[done]`

When an answer counts as finished.

| Key | Default | Meaning |
|---|---|---|
| `enter` | `"\r"` | What the Enter key sends. 1 to 16 bytes. |
| `quiet_ms` | `1200` | How long the output must stay still before the answer counts as done. 100 to 60000. |
| `busy_ms` | `1500` | How long output after Enter must last before the program counts as busy. 0 to 60000. |
| `osc133` | `true` | Believe the shell's own prompt and command marks. |

## Auto detection

Each pane starts on Auto. Termurion scores every profile against what the pane shows and picks the best one. A match is stronger or weaker by kind. From strongest to weakest:

1. `program`: the program running in the pane.
2. `text`: a text on the screen.
3. `prompt`: a prompt glyph at the cursor.
4. `shape`: a box shape around the cursor.

If two profiles match the same way, the one with more hits wins. If they still tie, a shipped profile wins over yours, and yours go in name order.

A program or text match is strong. Termurion remembers it for that part of the screen, so a banner that scrolls away does not lose the profile. The memory is dropped when the screen clears (leaving the alternate screen, clear, or a terminal reset). A prompt or shape match is weak: it is used when there is no strong match and nothing remembered. With no match at all, the pane uses the Shell profile.

A tmux window split into several panes is read region by region, so each region can have its own profile under Auto.

### Pick one for a pane

Right-click the pane and open FX Profile. The menu lists Auto, then every loaded profile, and checks the current pick. A pick applies to the whole Termurion pane, including every tmux pane inside it. Pick Auto to give control back to detection.

The pick is saved with the pane, by profile name. If you rename a profile on the FX Profiles page, the panes that picked it follow the new name. If the picked profile is deleted, the pane runs Auto, and the menu shows the old name as missing.

## The Shell profile

Shell is the fallback. A key missing from any profile takes the Shell value, and a pane that matches nothing uses it. Because of that, its name is fixed (the Name field is read-only) and the Delete button is off for it. You can still edit its other values or Reset it.

## Make your own

Say you have a tool that draws its input between two `═` lines and shows a `λ` as the prompt.

1. Open Settings, go to FX Profiles, and press New. A new profile starts with the Shell values.
2. Give it a name, such as `Lambda`.
3. Under Match, set Prompt glyphs to `λ`.
4. Under Input Box, set Kind to lines and Line glyphs to `U+2550`.
5. Under Prompt Mark, set Glyphs to `λ`.
6. Press Save.

The page cannot show box drawing and block characters in its font, so those glyph fields show and take codes like `U+2500`. Write several as codes separated by spaces: `U+2550 U+2500`. Other characters are written as themselves.

The same profile as a file, `lambda.toml`:

```toml
name = "Lambda"

[match]
prompt = "λ"

[box]
kind = "lines"
line_glyphs = "═"

[prompt_mark]
glyphs = "λ"
```

Every other key takes the Shell value. If you also know the program's name, add `program = ["mytool"]` under `[match]`. A program match is the strongest.

## The preview

Under the form, Preview shows what the rules you have typed (saved or not) find on the focused pane:

- Match: whether the profile matches the pane, and by which kind of hit (a text, a prompt, or a box shape).
- Cursor: the row and column of the cursor, and whether it is real or painted.
- Input box: the kind (lines or band) and its rows.
- Prompt mark: the glyph and where it is.

Use it to check a profile against an app that is running in a pane. With no pane open it says "No pane to read." An error in the form (a name already taken, a value out of range) shows in red in its place, and Save stays off until it is fixed.

## Buttons on the page

| Button | What it does |
|---|---|
| New | A new profile with the Shell values. |
| Duplicate | A copy of the selected profile. |
| Delete | Removes the selected profile's file after you confirm. Off for Shell. |
| Import | Copies a profile file into the folder. A file over 64 KiB is refused. If the name is taken, it asks before it replaces that profile. |
| Export | Saves the selected profile to a file. |
| Reset | Puts a shipped profile back to the text Termurion ships. Your edits to it are lost. Only for the shipped profiles. |
| Restore Shipped Profiles | Writes back any shipped file that is gone. |
| Show Folder | Opens the folder. |
| Save, Revert | Write the form to the file, or drop the edits you have not saved. |

## Updates to the shipped files

A new version of Termurion may change a shipped profile. Termurion replaces a shipped file with the new text only if you have not changed it. If you did change it, your file is kept, and the list marks it "newer version available". Press Reset to take the new text. The list also marks a shipped profile as "shipped" (unchanged) or "changed" (yours).

A shipped profile you delete stays deleted when Termurion updates. Restore Shipped Profiles brings back every one that is gone.
