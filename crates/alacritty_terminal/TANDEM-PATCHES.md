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

5. `grid/row.rs`: `Row` also has `time`, `fold`, and `note` slots with getters and setters,
   cleared by `reset`. Reflow merges a continuation row's ids and later time into the row
   above (`absorb_tags`), and a split-off part keeps its line's time. `term/mod.rs`: `Term`
   has a `stamp` (Unix seconds) that `write_at_cursor` puts on the row it writes (timestamps).

6. `grid/resize.rs`: `Grid::splice(first, last, with)` replaces a run of rows and returns the
   removed ones (folds and Replace Selection). Rows saved at another width grow or wrap to
   the current width.

7. `term/cell.rs`: `CellExtra` holds an inline image `Tile` (image id, column, and row in the
   image) with `Cell::image()` and `Cell::set_image()`; a cell with a tile is not empty.

8. `term/mod.rs`: device attributes follow iTerm2 for `Term.emulation_level` (100 to 500,
   default 500): DA1 `CSI ? 1 ; 2 c` (VT100), `CSI ? 62 ; 1 ; 2 ; 4 ; 6 ; 22 ; 28 c` (VT200 and
   VT300), `CSI ? 64 ; 1 ; 2 ; 4 ; 6 ; 17 ; 18 ; 21 ; 22 ; 52 c` (VT400 and VT500; 4 is sixel);
   DA2 `CSI > vt ; 2500 ; 0 c` (vt 0, 1, 32, 41, 64); DA3 (VT400 and up) `DCS ! | 6954726D ST`. `Term.da1_clipboard` (default true) drops `52` when
   the clipboard policy says programs may not use it.

9. `term/mod.rs`: `Term.modify_other_keys` (0, 1, 2) is set by `CSI > 4 ; n m` and reported
   for `CSI ? 4 m` as `CSI > 4 ; n m`; RIS clears it. `Config.kitty_keyboard` now means
   "programs may change how keys are reported" (Tandem's profile setting Allow
   modifyOtherKeys) and gates both kitty flags and modifyOtherKeys. Upstream's
   `push_keyboard_mode` removed the oldest entry from the title stack when the keyboard stack
   was full, which panics on an empty title stack; it now removes from the keyboard stack.
   `Term::clear_keyboard_modes` drops every program request (Terminal State > Reset), and
   `Term::scroll_region` reads the scrolling region (DECRQSS).

10. `grid/row.rs`: `Row` has a `block` slot (`block()`, `set_block()`), cleared by `reset`, merged by
    `absorb_tags` and kept by split rows like `time`. `term/mod.rs`: `Term.block` (0 for none) is
    stamped on the row `write_at_cursor` writes and on the row a line feed leaves (OSC 1337 Block).

To update alacritty_terminal, copy the new version here and apply these changes again.
