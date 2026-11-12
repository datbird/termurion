# FX profiles: per-app rules for the cursor effects

Date: 2026-11-12. Status: design, waiting for the owner's approval. Ships as Termurion 1.1.0
(the owner asked for the minor bump by name), together with the two held 1.0.6 fixes (the macOS
pointer size and the per-pane done signal).

## Intent

The render plugin effects find an app's cursor, input box, prompt mark, Enter and done by reading
the screen. The rules for that reading are hard-coded in Rust today (`src/scene.rs`,
`src/render_fx.rs`): line glyphs and a fill share for a line box, size limits for a shaded band,
a reverse-video cell for a painted cursor, quiet and busy times for done. Those rules were tuned
for Claude Code, Codex, opencode, Hermes and agy.

The owner wants any user to teach the effects a new app (their own TUI, an agent Termurion does not
know) without a code change. An FX profile is a plain file that holds one app's reading rules.
Termurion ships profiles for Claude Code, Codex, opencode, Hermes, agy and the shell. Users make
more in the GUI or by dropping a file in a folder.

A profile says HOW the effects read an app. It never says WHICH effects run: the effect list stays
in Profiles > Window > Effects, as now.

This reverses two points of `2026-11-09-other-ai-agents-design.md`: "Termurion never checks a
program's name" and "Non-goal: per-tool settings or a tool picker". The owner asked for both now.

## Goals

1. Every hard-coded reading rule moves into profile data. The Rust code keeps the algorithms
   (how to find a line box, a band, a painted cursor, a pane's borders) and takes their numbers
   and glyphs from the active profile.
2. Six shipped profiles: Claude Code, Codex, opencode, Hermes, agy, Shell. With them, every
   recorded agent screen and every existing scene and render test gives the same result as 1.0.5
   plus the held fixes. The shipped profiles are the proof that the data covers today's rules.
3. Termurion picks a profile per app on screen by detection, and the user can override the pick
   per pane.
4. Users list, create, edit, duplicate, delete, import, export and reset profiles in the GUI.
5. Profiles live as plain files in a dedicated folder that a user can open, edit by hand and copy
   files into.

## Non-goals

- Choosing effects, effect order or effect settings per app.
- Rules written as code or scripts. A profile is data only.
- Changing how tmux and zellij pane borders are found. Borders belong to the multiplexer, not the
  app, so the border and status bar rules (`pane_cols`, `output_watch`'s status row) stay global.

## Anti-goals (a failure even if profiles work)

- Any app that gets effects in 1.0.5 looks or behaves differently with the shipped profiles.
- A shell, vim, less, htop, mc or a tmux status bar gets box effects (the anti-goal list of the
  2026-11-09 design still holds).
- A broken or hostile profile file crashes Termurion, hangs a frame, or makes a frame visibly
  slower.
- A frame gets visibly slower with many profiles installed.

## The profile file

TOML, one profile per file, named `<something>.toml`. TOML because `settings.toml` and the
render plugin manifests already use it. Every key is optional; a missing key takes the value of
the Shell profile's built-in default, so a small file stays small. Unknown keys are kept on save
and reported as a warning on the profile page.

The sections below list what a profile holds. Key names are the builder's call; the meaning is not.

- `name`: shown in the GUI and the pane menu. Unique; the file name does not have to match.
- `[match]`: how Auto detection recognizes the app. All lists; any hit counts.
  - `program`: foreground job names (the name Termurion already shows in tab titles). Only
    usable when Termurion sees the job: a local pane, no tmux or ssh in between.
  - `text`: text that appears on the app's screen, such as `OpenAI Codex` or `Hermes Agent`.
  - `prompt`: prompt mark glyphs left of the cursor on its row, such as `❯`, `>`, `›`.
  - `shape`: the box kind next to the cursor (`lines`, `band`) as a weak clue.
- `[cursor]`: `real`, `painted` or `either`. For painted: the cell attribute that marks it
  (reverse video today) and whether the echo after a keystroke may anchor it (today's `echo_at`).
- `[box]`: `lines`, `band`, `either` or `none`.
  - Lines: the line glyphs (`─`, `━` today), the share of the pane's columns they must fill
    (80% today), how far below the cursor the bottom line may sit (2 rows today), and whether a
    line row with a frame corner counts (no today).
  - Band: minimum and maximum rows (2 and 12 today), maximum share of the screen (half), minimum
    share of the columns (half), the column tolerance when growing (2 cells), and the half-block
    edge glyphs (`▀`, `▄`).
- `[prompt_mark]`: the glyphs the effects treat as the prompt mark. See "Shaders" below.
- `[done]`: the Enter key bytes that start a wait (`\r` today), the quiet time (1200 ms), the
  busy time (1500 ms), and whether OSC 133;D ends the wait.

Values are checked on load with hard limits (for example quiet time 100 ms to 60 s, at most a few
dozen glyphs per list, text matches of at most 200 characters, at most 64 profiles loaded). A value
out of range is an error for that profile, never a clamp the user cannot see.

## The folder

`FXProfiles`, beside the settings file, the same pattern as `RenderPlugins`
(`render_fx::user_dir`). On Windows that is `%APPDATA%\Termurion\FXProfiles`; macOS and Linux use
their own settings folders.

- Termurion writes the six shipped profiles into the folder on first start, so a user can open
  and edit them like any other file.
- Each shipped file carries a marker of the version it came from (a comment line or a key; builder's
  call). On an update, Termurion replaces a shipped file only when the user has not changed it
  since it was written. A changed one is kept, and the profile page shows that a newer shipped
  version exists, with Reset to take it.
- Deleting a shipped file deletes that profile; Termurion does not write it back on the next start.
  The profile page offers "Restore shipped profiles" to bring missing ones back.
- Termurion watches the folder. A new, changed or removed file takes effect without a restart.
- A file that does not parse or fails its limits is skipped. Termurion keeps running, and the
  profile page lists the file with its error (file, line, message). Other profiles are unaffected.
- The Shell profile is always present: if its file is missing or broken, the built-in copy is used,
  because it is the fallback for everything else.
- Two files with the same `name`: the first in file-name order loads, the second is skipped with an
  error on the profile page.

## The Shell profile (owner's decision)

The Shell profile holds today's general rules: cursor `either`, box `either`, and the 1.0.5 numbers
and glyphs. So an app with no profile of its own (an unknown TUI, or a known app whose banner
scrolled away before Termurion saw it) gets exactly the 1.0.5 behavior. A plain shell prompt still
gets no box, because the shape rules already reject it. Deleting the Claude Code profile leaves
Claude Code on these general rules.

## Order of work per screen change (my call)

Finding the cursor needs rules, and choosing a profile needs the cursor's region. So:

1. Find the effects' cursor with the general rules widened by every loaded profile's cursor and
   line rules (the union of their line glyphs and painted-cursor marks). This keeps today's
   `painted_cursor` algorithm (every box with one marked cell is a candidate; the echo picks the
   pane typed in) and lets a custom profile with `═` lines and a painted cursor be found.
2. Find the region around that cursor (`output_watch`'s columns and rows).
3. Choose the profile for the region (Auto or the pane's pick).
4. Read the box, the prompt mark and the cursor again with that profile's rules only.

`painted_cursor`, `output_watch` and the `input_box` closure take the rules as an argument instead
of reading constants; their signatures may change.

## What stays fixed algorithm (my call)

The profile feeds glyph lists and numbers. The logic that keeps false boxes out stays in code and
is not configurable: a line row with a junction, a frame corner at the pane's edge or a tmux pane
title is never a box line; the vim status strip rule and the default-background rule for bands;
the half-block edge logic. Pane borders and the status bar row stay global (not the profile's line
glyphs).

The painted cursor mark is one of a fixed set: reverse video (today), or a background color given
as `#rrggbb` and compared after the palette resolves it. `either` means: the real cursor when it
is shown, else the painted one. For the box, `either` keeps today's precedence: a line box wins
over a band.

## Choosing a profile

### Auto (default)

Detection runs per region, not per Termurion pane: the region is the multiplexer pane around the
effects' cursor (the same columns and rows `output_watch` finds), or the whole screen without a
multiplexer. So two tmux panes in one Termurion pane can run Claude Code and Codex and each gets
its own profile.

- Each profile scores against the region. A `program` hit (when the job is visible) and a `text`
  hit are strong; a `prompt` hit is medium; a `shape` hit is weak. The highest score wins; ties go
  to the profile listed first (shipped profiles first, then the user's in name order). No hit at
  all gives the Shell profile.
- Banners scroll away, so a strong match is remembered for that region until the region clears
  (alternate screen exit, a full clear, or a different strong match). The region is keyed by its
  bounds; a changed layout drops the memory for that region.
- Detection is cheap: it runs when the screen changed and at most a few times a second, never per
  frame, and it scans only the region's visible rows.
- "The region clears" means: the alternate screen is left, ED 2, ED 3 or RIS clears it, or another
  profile makes a strong match there. A `program` match (when the job is visible) always wins over
  the memory. My call: after an agent that does not use the alternate screen (Hermes, agy) exits,
  the memory may keep its profile on the shell prompt; this is harmless, because its box rules find
  no box on a prompt.
- `program` matches the foreground job's executable name as Termurion's job tracking reports it
  (`src/proctree.rs`), not its full command line. Several agents run as `node`, so the shipped
  profiles rely on `text` and `prompt` first; the builder records the real job names on each OS and
  adds them where they are distinct.

### Per-pane pick

The pane's right-click menu gets "FX Profile" with "Auto" and every loaded profile. A pick applies
to the whole Termurion pane, including every tmux pane inside it. "Auto" hands control back to
detection. The pick is saved with the pane by profile name, as an optional field that older saved
sessions simply lack (no format version change). Renaming a profile in the GUI moves the picks
that name it. A picked profile that was deleted falls back to Auto, and the menu shows it.

### Done timing (my call)

The `[done]` rules come from the profile of the region where Enter was pressed, fixed for that
wait. A focus or region change mid-wait does not change them. The wait watches that region's
output (the per-pane done fix held for this release).

## Shaders

The 13 shaders find the prompt mark by pixel shape (`clChevron`, `clMarkLeft`), not by data, so
the glyphs in a profile cannot reach them as they are. The design passes the mark from Rust:

- Rust finds the profile's prompt mark glyph left of the cursor on the cursor's row (the nearest
  match within the region's columns, skipping spaces, wide glyphs counted as one cell) and passes its
  cell rectangle in a new uniform (`iPromptMark`, zero when none), appended to `Globals` with the
  std140 rules `src/render_fx_prefix.glsl` describes.
- `render_fx::patch_input_box`, which already rewrites the box helpers in every shader, also
  rewrites the mark helpers (`clMarkLeft`, `clChevron` and whatever they call to find the mark) to
  use `iPromptMark` when it is set, and keeps the pixel check when it is not (user shaders written
  before 1.1.0 keep working). The existing GPU render tests in `src/render.rs` prove the mark still
  changes its look for every effect.

## The GUI

A new page in global Settings (not in a Profile, since FX profiles are global): "FX Profiles". The
exact place in the Settings tree is the builder's call, next to where the render plugin folder is
already mentioned if there is such a place.

- A list of profiles: name, shipped or user, changed-from-shipped mark, error mark.
- Buttons: New, Duplicate, Delete, Import, Export, Reset (shipped profiles only), Restore shipped
  profiles, Show Folder.
- Delete asks first. Import of a name that already exists asks before it replaces it.
- Export writes the selected profile to a file the user picks. Import copies a file into the
  folder after it passes the same checks as a load.
- An editor form with one section per rule group (match, cursor, box, prompt mark, done). Each field
  has a hover text saying what it does and its default.
- A live preview under the form runs the form's unsaved values in memory (a separate path from the
  loaded profiles) and marks what they find in the focused pane right now
  (the region, the cursor, the box edges, the prompt mark, and whether `match` hits), so the user
  sees at once when a rule is wrong. Builder's call how it draws (an overlay on the pane or a
  text summary), as long as the user can see which rule found what.
- Saving writes the file in the folder; the folder watcher then reloads it like a hand edit.

## Tests

- Profile parsing: every key, defaults from Shell, every limit, unknown keys kept, a broken file
  reported with its line, 64-profile cap.
- Parity: the six shipped profiles against every screen in `tests/fixtures/agents/` and the 35 tmux
  layouts recorded on 2026-11-12 (now in the session scratchpad; the first task copies them to
  `tests/fixtures/tmux-agents/` with their `.panes` files), and every existing `scene.rs` and `render.rs` effect test,
  give the same cursor, box, mark and signals as before. These tests must fail if a shipped value
  changes.
- Detection: each recorded agent screen picks its own profile; a plain shell screen picks Shell;
  a tmux screen with Claude Code beside Codex picks each per region; a banner that scrolled away
  keeps its profile; a per-pane pick wins over detection.
- Custom profile: a test app screen that no shipped profile reads (for example a box drawn with
  `═` lines and a `λ` prompt) gets no box with the shipped profiles and gets one with a small
  custom profile.
- Folder: first start writes the shipped files; an unchanged shipped file updates; a changed one
  does not; a deleted one stays deleted; a broken file is skipped with an error.
- Performance: detection cost with 64 profiles stays under a set budget per screen change.

## Docs

A help page for FX profiles: what a profile is, the folder per OS, every key with its default, a
worked example of a custom profile, and how to read the live preview. The README's render plugin
section names the feature in one line.

## Order of the work

The two held fixes (macOS pointer size, per-pane done signal) are committed on main first. They are
the parity baseline: "the same result as 1.0.5" means 1.0.5 plus those two fixes.

## Left to the builder

- Key names and the TOML layout, if every rule above is there.
- The scoring weights, if the detection tests pass.
- The marker for "shipped and unchanged" (hash, version key, or a comment).
- Where the page sits in Settings and how the preview draws.
- How the per-pane pick is stored with the pane.
