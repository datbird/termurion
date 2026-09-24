# Tandem patches to alacritty_terminal 0.26.0

This is alacritty_terminal 0.26.0 from crates.io (Apache-2.0, see LICENSE-APACHE), used
through `[patch.crates-io]` in Tandem's Cargo.toml. The `tests/` directory (46 MB of ref
test data) is left out. Changes:

1. `grid/row.rs`: `Row` has `mark: u32` and `manual: u32` fields with `mark()`, `set_mark()`,
   `manual()`, and `set_manual()` (a row can hold a prompt mark and a manual mark). Tandem stores
   shell-integration and manual marks there, so a mark moves with its row through scrolling,
   scrollback, and reflow. `Row::reset` clears it, so a recycled row starts unmarked.
2. `grid/resize.rs`: when widening merges a wrapped continuation row into the row above,
   that row takes the continuation's mark if it has none. Narrowing keeps the mark on the
   first part of a split line.

3. `grid/resize.rs`: `Grid::cut_from(line)` removes every row from a line to the bottom,
   scrollback included (iTerm2's Clear to Last Mark).

4. `term/mod.rs`: `Term::primary_grid()` and `primary_grid_mut()` give the main screen's grid
   while the alternate screen is up, so marks survive full-screen apps.

To update alacritty_terminal, copy the new version here and apply these changes again.
