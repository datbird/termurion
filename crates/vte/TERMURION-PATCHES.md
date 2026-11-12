# Termurion patches to vte 0.15.0

This is vte 0.15.0 from crates.io (Apache-2.0 OR MIT, see the LICENSE files), used through
`[patch.crates-io]` in Termurion's Cargo.toml. The examples, tests and docs are left out. Changes:

1. `lib.rs`: `State` is public and `Parser::state()` returns it. Termurion's output scanner
   (`src/osc.rs`) runs a second parser over the same bytes and reads where it is, so the
   scanner can never disagree with the parser that runs the screen about where a control
   sequence or a string starts and ends (milestone 13o, Task 2 fix 1).

2. `ansi.rs`: `CSI Pl ; Pr s` calls the new `Handler::set_left_right_margins` (DECSLRM). Its
   default saves the cursor for a bare `CSI s`, as before; alacritty_terminal decides by DECLRMM.

To update vte, copy the new version here and apply these changes again.
