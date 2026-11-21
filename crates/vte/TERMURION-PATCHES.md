# Termurion patches to vte 0.15.0

This is vte 0.15.0 from crates.io (Apache-2.0 OR MIT, see the LICENSE files), used through
`[patch.crates-io]` in Termurion's Cargo.toml. The examples, tests and docs are left out. Changes:

1. `lib.rs`: `State` is public and `Parser::state()` returns it. Termurion's output scanner
   (`src/osc.rs`) runs a second parser over the same bytes and reads where it is, so the
   scanner can never disagree with the parser that runs the screen about where a control
   sequence or a string starts and ends (milestone 13o, Task 2 fix 1).

2. `ansi.rs`: `CSI Pl ; Pr s` calls the new `Handler::set_left_right_margins` (DECSLRM). Its
   default saves the cursor for a bare `CSI s`, as before; alacritty_terminal decides by DECLRMM.

3. `ansi.rs`: `Rgb`, `Color` and `NamedColor` derive `Hash`, so `alacritty_terminal`'s `Cell` can
   be hashed. Termurion's row cache (`src/scene_rows.rs`) keys a built row by its cells.

4. `lib.rs`, `ansi.rs`: printable ASCII in bulk. `ground_dispatch` passes each maximal run of
   0x20 to 0x7E to the new `Perform::print_ascii` (default: `print` for each character) and
   everything else (controls, DEL, non-ASCII) one at a time as before, in order. `Performer`
   hands the run to the new `Handler::input_ascii` (default: `input` for each character) and
   keeps the run's last character for REP. alacritty_terminal writes a run's cells in one pass
   (its patch 42), the flood parse's largest cost.

To update vte, copy the new version here and apply these changes again.
