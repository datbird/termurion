# Termurion patches to alacritty_terminal 0.26.0

This is alacritty_terminal 0.26.0 from crates.io (Apache-2.0, see LICENSE-APACHE), used
through `[patch.crates-io]` in Termurion's Cargo.toml. The `tests/` directory (46 MB of ref
test data) is left out. Changes:

1. `grid/row.rs`: `Row` has `mark: u32` and `manual: u32` fields with `mark()`, `set_mark()`,
   `manual()`, and `set_manual()` (a row can hold a prompt mark and a manual mark). Termurion stores
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
   "programs may change how keys are reported" (Termurion's profile setting Allow
   modifyOtherKeys) and gates both kitty flags and modifyOtherKeys. Upstream's
   `push_keyboard_mode` removed the oldest entry from the title stack when the keyboard stack
   was full, which panics on an empty title stack; it now removes from the keyboard stack.
   `Term::clear_keyboard_modes` drops every program request (Terminal State > Reset), and
   `Term::scroll_region` reads the scrolling region (DECRQSS).

10. `grid/row.rs`: `Row` has a `block` slot (`block()`, `set_block()`), cleared by `reset`, merged by
    `absorb_tags` and kept by split rows like `time`. `term/mod.rs`: `Term.block` (0 for none) is
    stamped on the row `write_at_cursor` writes and on the row a line feed leaves (OSC 1337 Block).

11. `grid/row.rs`: `Row` has a `button` slot (`button()`, `set_button()`), the key of the OSC 1337
    buttons on the row, cleared by `reset` and merged by `absorb_tags`. Split rows do not keep it:
    a button belongs to the part it was written in.

12. `grid/row.rs`: `Row` has a `porthole` slot (`porthole()`, `set_porthole()`), the porthole a row
    stands for, cleared by `reset`, merged by `absorb_tags`, and kept by split rows like `block`.
    `grid/mod.rs`: `Grid::max_scroll_limit()` reads the history limit (a porthole never takes
    more rows than the buffer holds).

13. `grid/mod.rs`: each `Grid` counts rows that scroll up off the top of its screen
    (`Grid::scroll_up` with the region starting at the top, which clearing the screen also uses);
    `Grid::scrolled_off()` reads it, so triggers can number rows.

14. `term/mod.rs`: `Term::set_scrollback_with_region(bool)` (default true). When it is off, a scroll
    region that starts at the top but stops above the bottom row (an app's status line) scrolls in
    place through `Grid::scroll_up_in_place` (`grid/mod.rs`), like a region below the top, so no
    line reaches history (iTerm2's Scrollback With Status Bar). `grid/storage.rs`: `Storage::swap`
    swaps whole rows with `slice::swap`; upstream copied four qwords, the size of an unpatched
    `Row`, so the Termurion row slots stayed behind and debug builds panicked on region scrolls.

15. `grid/mod.rs`: each `Grid` also counts rows lost off the top of its history (`dropped`): a full
    history scrolling, a smaller history limit, and the history limit cut in `resize.rs`'s
    `shrink_columns` and `splice`. Clearing the history does not count. `Grid::dropped()` plus the
    history size numbers lines as iTerm2's API does, which a reflow cannot move below zero.

16. `term/mod.rs`: DECSET 1007 starts off, as iTerm2 (`VT100Terminal.m:399`), and upstream's default
    had it on; `Term.alternate_scroll_allowed` (default true) refuses a set, never a reset (the
    profile's Allow Alternate Mouse Scroll). `Term::clear_alternate_scroll` turns it off for an
    initial OSC 133 prompt mark.

17. `term/mod.rs`: `Term::set_cursor_blink_change(bool)` (default true). When it is off, programs
    change the cursor's shape but not its blink: DECSCUSR keeps the current blink and
    DECSET/DECRST 12 are ignored; DECSCUSR 0 and RIS still restore the configured style (iTerm2's
    Allow Change Cursor Blink).

18. `term/cell.rs`, `term/mod.rs`: `Flags::BLINK` (the last free bit of the `u16`), set by SGR 5
    (`Attr::BlinkSlow`) and SGR 6 (`Attr::BlinkFast`) and cleared by SGR 25 (`Attr::CancelBlink`)
    and `Attr::Reset`. Upstream ignored all three. Termurion draws BLINK text in the off phase of the
    blink clock when the profile allows blinking text.

19. `selection.rs`, `term/mod.rs`: `Selection.runs` (inclusive per-line `(first, last)` cell pairs),
    `Selection::set_visual` (sets the runs and makes the region their hull) and
    `Selection::covers` (inside the runs, or true with none). `Term::selection_to_string` copies
    the runs, each line in logical order, joined by newlines (not across a wrapped line).
    `Selection::update` drops the runs; `rotate` moves them (and the visual drag's anchor,
    `set_visual_anchor`/`visual_anchor`, a grid line plus visual column) with the content and
    drops the runs that scroll out of the region. Termurion's visual character selection on
    bidirectional rows uses it (iTerm2's live visual range, `iTermSelection.m:409`).
    `Selection::drop_above` (called after `scroll_up_relative` scrolls the grid) drops runs
    above the oldest buffer line (alternate screen, full scrollback) and clamps the anchor;
    `selection_to_string` skips such runs; `rotate` returns None when every run was dropped.
    `Selection::rotate_in_place` is `rotate` for a scroll that moves only the region's rows even
    from the top (upstream's `range_top == 0` case, which moves history with it, becomes the
    `into_history` flag of `rotate_region`): `scroll_up_relative` uses it when the region
    scrolls in place (patch 14's `keep`) and `scroll_down_relative` always (a scroll down never
    moves history). History stays put; runs, the anchor and the hull leaving the region's top
    are dropped or clamped to it as below a fixed top.
    `Selection::shift_lines` replaces upstream's `rotate` in a height-only `Term::resize`: every
    line moved by `delta`, and runs and the visual anchor on a line now outside the buffer (rows
    cut below a high cursor, the oldest full scrollback, the alternate screen's top rows) are
    dropped; a visual selection's hull becomes its remaining runs', and a logical selection
    whose end left the buffer is cleared (None, as when nothing is left).

20. `term/mod.rs`: the plumb knobs of milestone 13o. `Term.tab_width` (default 8) and
    `Term::set_tab_width`: `TabStops::new(columns, width)` and `TabStops::resize(columns,
    width)` take the spacing (at least 1) and a reset or a wider grid uses `tab_width`;
    `set_tab_width` resets every stop, so it runs only on a session's first byte (iTerm2's
    Default tab stop width). `Term.xterm_version` (default 2500) is the version `identify_terminal`
    puts in DA2. `Term.color_report_16` (default true): `dynamic_color_sequence` formats `{:02x}`
    per channel when it is false (oscColorReport16Bits). `Term.focus_reporting_allowed` (default
    true): a DECSET 1004 is refused while false and DECRQM answers reset (iTerm2's
    `reportFocus = mode && terminalFocusReportingAllowed`, `VT100Terminal.m:857`, `:5691`); a reset
    is never refused (the shape of patch 16's `alternate_scroll_allowed`).

21. `term/mod.rs`: `Term.max_url_len` (default 2097152, iTerm2's maxURLLength). `set_hyperlink`
    returns before touching the template when the URI is longer, so a refused link neither starts
    nor ends one (`VT100Terminal.m:4189`); a later OSC 8 with no URI still ends an open link.
22. `term/mod.rs`: the title stack holds at most 10 titles (xterm's depth; it was 4096) and each
    entry is cut at 4096 bytes on a character boundary, so `CSI 22 t` repeated cannot hold
    gigabytes of a program's 1 MiB title. `KEYBOARD_MODE_STACK_MAX_DEPTH` keeps 4096 (it shared
    the constant). A title longer than 4096 bytes comes back cut from `CSI 23 t`. `Term::title_stack()`
    lets Termurion's tests read the stack.
23. `term/cell.rs`: `Cell::push_zerowidth` keeps at most `MAX_ZEROWIDTH` (16) marks per cell; later
    ones are dropped. Without it one `CSI 65535 b` after a combining mark added 65535 marks to a
    cell (REP now repeats the whole cell, in Termurion's `pass`, but any stream of marks was unbounded).
24. `term/mod.rs`, `grid/mod.rs`: scrollback in the alternate screen (iTerm2's Scrollback in
    Alternate Screen, on by default; upstream kept none). `Term.alt_scrollback` and
    `Term::set_alt_scrollback`: the alternate grid's history limit is the main screen's, or 0
    (`update_history_limits`, also run by `set_options`). Entering the alternate screen clears its
    history; leaving moves it into the main grid with `Grid::take_history` and
    `Grid::append_history` (rows go above line 0, oldest first, past the limit they are dropped
    from the top and counted in `dropped`; `scrolled_off` does not change, so rows on the screen
    keep their numbers).
    `Term::scroll_screen_into_history` (iTerm2's `scrollScreenIntoHistory`: the rows through the
    last non-empty one scroll up into the shown grid's history, whatever the scroll region).
    `Term.save_screen_on_clear`: ED 0 from the home cell of the alternate screen calls it first
    (iTerm2's saveScrollBufferWhenClearing; the main screen never does, zsh issue 8822).
    `Term.printed` counts characters written, for "has the program drawn anything since".
25. `term/mod.rs`: `WidthRule` and `Term.width_rule: Option<WidthRule>` (None keeps upstream's
    `c.width()`). `input` takes the cells of a character from the rule's `width` function
    (Termurion's: iTerm2's tables, U+200B by `zwsp_advances`). A U+FE0F after a narrow cell whose
    base `accepts_vs16`, with `vs16` or (`vs16_primary` and not in the alternate screen), widens
    that cell (`widen_previous`): the spacer is written and the cursor moves one more column; a
    cell in the last column moves to the next row first when line wrap is on (a leading spacer
    stays), and stays narrow when it is off. A regional indicator right after a cell holding a
    lone regional indicator joins it as a zero-width mark (one cluster) and the pair is always
    two cells: it widens a one-cell first indicator the same way (iTerm2 `StringToScreenChars`).
    A skin tone modifier keeps its own cell, as in iTerm2 with its default
    AggressiveBaseCharacterDetection (CharsetTests.swift:453-515); a pictograph (`is_pictograph`)
    right after a ZWJ in a pictograph's (or a skin tone's) cell joins it, so a ZWJ sequence is as
    wide as its first emoji.
26. `term/mod.rs`: ED 2 in the alternate screen first scrolls the screen into its history when
    `alt_scrollback` is on (iTerm2 `MS:1712-1720`; patch 24 did it for ED 0 at home only).
    `Term.history_splices` and `take_history_splices`: leaving the alternate screen records
    `(main scrolled_off, rows spliced)`, so a holder of absolute row numbers (those below
    `scrolled_off` named history rows) can move them down by the rows inserted after them.
27. `term/mod.rs`: DEC mode 2048 (in-band resize reports), `Term.resize_reports`. DECSET and
    DECRST set and clear it (also inside a parameter list), DECRQM answers set or reset instead of
    not recognized, RIS clears it. The report itself is written by Termurion after a pty resize.
28. `term/mod.rs`: `Term.tabs_wrap` (iTerm2's tabsWrapAround, default false) and `put_tab`. A
    tab in the pending-wrap state does nothing (upstream wrapped the line); with `tabs_wrap` a
    tab at the last column (or pending wrap) wraps the line softly and moves to the first tab
    stop after column 0 (`MS:2795-2818`), whether DECAWM is on or off (`wrapline_always`).
29. `term/mod.rs`: `Term.resets` counts RIS (`reset_state`), so Termurion can stop an OSC 9;4 progress bar on
    a reset (iTerm2 `terminalDidReset`).
30. `term/mod.rs`: `Term.history_clears` counts scrollback clears (`ClearMode::Saved` with history), so
    Termurion's extra selections, numbered by buffer row, can drop when a clear renumbers the rows.
31. `term/mod.rs`: `Term::is_tab_stop(column)`, so the copy text model can tell a tab's blanks from
    typed spaces (13s Task 3).
32. `tty/windows/conpty.rs`: `LoadLibraryExW("conpty.dll", NULL, LOAD_LIBRARY_SEARCH_SYSTEM32)`
    instead of a bare load, so no conpty.dll beside the exe or in the current folder loads.
33. `term/mod.rs`: `TermMode::META_SENDS_ESCAPE` (bit 23), DEC mode 1036 (xterm metaSendsEscape). DECSET and
    DECRST set and clear it (also inside a parameter list), DECRQM answers set or reset, RIS clears it
    (`mode &= VI`). DECSTR clears it too (Termurion's byte list in `Screen::soft_reset` sends `?1036l`, as iTerm2's softReset does). Termurion reads it in `KeyOptions::effective`.

To update alacritty_terminal, copy the new version here and apply these changes again.
