//! Instant Replay's replay session (pure): entering a recording, stepping and seeking
//! its decoder, the keys it takes while replaying, and the screen built from one of its
//! frames. `PT:10473-10484,10589-10608`, `PS:1470-1542`, `PT:14732-14738`.

use alacritty_terminal::grid::Row;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Cell;

use crate::dvr;
use crate::keys::Key;
use crate::palette::Palette;
use crate::screen::{Look, Screen};

/// Left and Right step 15 frames with Shift (`PS:12855-12891`).
pub const SHIFT_STEPS: u32 = 15;

/// One pane's replay: the decoder showing a frame, and whether it plays a loaded
/// recording (`_dvr.readOnly`) rather than a live session's own DVR. A loaded
/// recording's Esc closes the whole session; a live one's returns to the live view
/// (the app decides which, from this flag).
#[derive(Clone, Debug)]
pub struct Replay {
    pub decoder: Option<dvr::Decoder>,
    pub read_only: bool,
}

/// A menu command that acts on the terminal itself (Clear Buffer, Clear Scrollback, Reset,
/// the Terminal State items), which does nothing on a replaying pane: iTerm2 sends it to
/// the replay's shell-less synthetic session. Clear Instant Replay has its own path.
pub fn blocks_command(selector: &str) -> bool {
    selector != "clearInstantReplay:" && ["terminalState", "reset", "clear"].iter().any(|p| selector.starts_with(p))
}

/// `setDvr:liveSession:` (`PS:1470-1480`): None when nothing was recorded; else the
/// decoder seeked to the newest frame. Entering alone shows the newest frame; the kept
/// bug (the first Start Instant Replay shows the second-newest) comes from the caller's
/// sequence of `enter` then `step(.., -1)` (`PT:10473-10484` calls both in turn), not
/// from `enter` itself.
pub fn enter(d: &dvr::Dvr) -> Option<Replay> {
    let t = d.last_timestamp();
    if t == 0 {
        return None;
    }
    let mut decoder = dvr::Decoder::default();
    decoder.seek(d, t);
    Some(Replay { decoder: Some(decoder), read_only: d.read_only })
}

/// `irAdvance:dir` (`PS:1497-1512`): steps the decoder one frame; false (beep) at
/// either end, and when there is no decoder to step.
pub fn step(r: &mut Replay, d: &dvr::Dvr, dir: i32) -> bool {
    match r.decoder.as_mut() {
        Some(dec) if dir > 0 => dec.next(d),
        Some(dec) => dec.prev(d),
        None => false,
    }
}

/// `timestampForFraction:` (`PT:14732-14738`): the timestamp range multiplied by `f` in
/// float precision, as iTerm2's `long long range` times a `float f` does, then
/// truncated toward zero into the result.
pub fn timestamp_for_fraction(first: i64, last: i64, f: f32) -> i64 {
    let range = (last - first) as f32;
    first + (range * f) as i64
}

/// `instantReplaySeekTo:` (`PT:10461-10466`) plus `irSeekToAtLeast:` (`PS:1534-1542`):
/// seeks to the first frame at or after the fraction's timestamp; returns the decoder's
/// timestamp whether or not the seek moved it.
pub fn seek_fraction(r: &mut Replay, d: &dvr::Dvr, f: f32) -> i64 {
    let t = timestamp_for_fraction(d.first_timestamp(), d.last_timestamp(), f);
    match r.decoder.as_mut() {
        Some(dec) => {
            dec.seek(d, t);
            dec.timestamp()
        }
        None => -1,
    }
}

/// The decoder's timestamp, or -1 while live.
pub fn current(r: Option<&Replay>) -> i64 {
    r.and_then(|r| r.decoder.as_ref()).map_or(-1, |dec| dec.timestamp())
}

/// `canInstantReplayPrev` (`PS:7909-7918`): true while live, regardless of whether
/// anything was recorded (iTerm2's own answer); else whether the decoder is off the
/// first frame.
pub fn can_prev(r: Option<&Replay>, d: &dvr::Dvr) -> bool {
    match r.and_then(|r| r.decoder.as_ref()) {
        Some(dec) => dec.timestamp() != d.first_timestamp(),
        None => true,
    }
}

/// A key pressed while replaying (`PS:12795-12891`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrKey {
    /// Esc: closes the session for a loaded recording, else returns to the live view.
    Exit,
    Step { dir: i32, count: u32 },
    Beep,
    /// A Win chord: iTerm2's key equivalents run before the session ever sees the key,
    /// so these still reach the menus.
    Pass,
}

/// `handleKeypressInInstantReplay:` (`PS:12795-12891`). The brief names the key
/// parameter's type `keystroke::Key`; `keystroke.rs` only imports it privately from
/// `crate::keys`, so this takes the real path (Decision, see report).
pub fn ir_key(key: &Key, shift: bool, win: bool) -> IrKey {
    if win {
        return IrKey::Pass;
    }
    let count = if shift { SHIFT_STEPS } else { 1 };
    match key {
        Key::Escape => IrKey::Exit,
        Key::Left => IrKey::Step { dir: -1, count },
        Key::Right => IrKey::Step { dir: 1, count },
        _ => IrKey::Beep,
    }
}

/// What happened in a window, for what it does to Instant Replay there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayEvent {
    /// Another pane of the tab became active (`TAB:907-909`).
    OtherPaneActive,
    /// A tab came to the front; `replaying` when its active pane shows a frame
    /// (`showOrHideInstantReplayBar`, `PT:7520-7528`).
    TabShown { replaying: bool },
    /// Split selection mode began (`PT:12223-12226`).
    SplitSelection,
    /// A pane of the window closed (`closeSessionWithoutConfirmation:`, `PT:2766-2770`).
    PaneClosed,
    /// The window closed (`PT:4910-4914`).
    WindowClosed,
    /// A split was asked for (`canSplitPaneVertically:`, `PT:11124-11129`).
    Split,
}

/// What an event does to the window's Instant Replay panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Nothing,
    ClosePanel,
    OpenPanel,
    /// The asked-for action does not happen.
    Refuse,
}

/// iTerm2's closers: with the panel open, every event closes it except a split (refused)
/// and a tab whose active pane replays (shown); with it closed, only that tab opens it.
pub fn on_event(panel_open: bool, e: ReplayEvent) -> Close {
    match (panel_open, e) {
        (_, ReplayEvent::TabShown { replaying: true }) => Close::OpenPanel,
        (false, _) => Close::Nothing,
        (true, ReplayEvent::Split) => Close::Refuse,
        (true, _) => Close::ClosePanel,
    }
}

/// A frame's row fit to `cols`: the source's left columns, kept or padded blank on the
/// right (`VT100Grid.m:2198-2245`, `charsToCopyPerLine`).
fn fit_row(src: &Row<Cell>, cols: usize) -> Row<Cell> {
    let mut r: Row<Cell> = Row::new(cols);
    for x in 0..cols.min(src.len()) {
        r[Column(x)] = src[Column(x)].clone();
    }
    r
}

/// `setDvrFrame` (`PS:15314-15375`) without the window resize: a Decision (see the
/// brief's Resume notes) crops or pads the frame to the pane's size instead. Rows past
/// the frame's height stay blank (top-aligned); a taller frame keeps its bottom `rows`
/// lines (`VT100Grid.m:2201-2202`'s `sourceLineOffset`/`yOffset`). The cursor is clamped
/// exactly as `VT100Grid.m:2243-2245`.
pub fn frame_screen(frame: &dvr::Decoder, cols: usize, rows: usize, palette: Palette, look: Look) -> Screen {
    let mut screen = Screen::with_look(cols, rows, palette, look);
    let y_offset = frame.rows.len().saturating_sub(rows);
    let new_rows: Vec<Row<Cell>> = (0..rows)
        .map(|y| match frame.rows.get(y + y_offset) {
            Some(src) => fit_row(src, cols),
            None => Row::new(cols),
        })
        .collect();
    screen.splice_rows(0, rows as i32 - 1, new_rows);
    if let Some(info) = frame.info {
        let x = info.cursor.0.min(cols.saturating_sub(1));
        let y = info.cursor.1.saturating_sub(y_offset).min(rows.saturating_sub(1));
        screen.term_mut().grid_mut().cursor.point = Point::new(Line(y as i32), Column(x));
    }
    screen
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::grid::Dimensions;

    #[test]
    fn terminal_commands_do_nothing_on_a_replay() {
        // iTerm2 sends them to the replay's synthetic session, which has no shell.
        for sel in ["clearBuffer:", "clearScrollbackBuffer:", "clearToLastMark:", "clearToStartOfSelection:", "reset:", "resetCharset:", "terminalStateReset:", "terminalStateToggleMouseReporting:"] {
            assert!(blocks_command(sel), "{sel}");
        }
        assert!(!blocks_command("clearInstantReplay:"), "it has its own path");
        assert!(!blocks_command("copy:"));
        assert!(!blocks_command("paste:"));
    }

    fn row(cols: usize, ch: char) -> Row<Cell> {
        let mut r: Row<Cell> = Row::new(cols);
        for x in 0..cols {
            r[Column(x)].c = ch;
        }
        r
    }

    fn screen_rows(cols: usize, rows: usize) -> Vec<Row<Cell>> {
        (0..rows).map(|y| row(cols, (b'a' + (y % 26) as u8) as char)).collect()
    }

    /// A recorder holding `n` key frames (each ineligible for a diff: `clean` is empty),
    /// timestamped 1, 2, 3, ... and cursor `(0, y)` on the y-th frame appended.
    fn recorder_with(n: usize, cols: usize, rows: usize) -> dvr::Dvr {
        let mut d = dvr::Dvr::new(dvr::key_bytes(cols, rows) * 1000);
        for i in 0..n {
            d.append(&screen_rows(cols, rows), &[], (0, i), (i + 1) as i64);
        }
        d
    }

    #[test]
    fn replay_steps_follow_iterm2() {
        let empty = dvr::Dvr::new(dvr::key_bytes(4, 4) * 1000);
        assert!(enter(&empty).is_none(), "nothing recorded");

        let d = recorder_with(3, 4, 4);
        let mut r = enter(&d).unwrap();
        assert_eq!(r.decoder.as_ref().unwrap().timestamp(), 3, "enter alone seeks the third (newest)");
        assert!(step(&mut r, &d, -1), "the app's first step back");
        assert_eq!(r.decoder.as_ref().unwrap().timestamp(), 2, "shows the second (kept, PS:1470-1532)");
        assert!(step(&mut r, &d, -1), "one more back reaches the first frame");
        assert_eq!(r.decoder.as_ref().unwrap().timestamp(), 1);
        assert!(!step(&mut r, &d, -1), "already at the first: false");
        assert_eq!(r.decoder.as_ref().unwrap().timestamp(), 1, "prev did not move it");

        let mut r2 = enter(&d).unwrap();
        assert!(!step(&mut r2, &d, 1), "already at the last (newest): false");

        assert_eq!(timestamp_for_fraction(1000, 2000, 0.25), 1250);
        assert_eq!(timestamp_for_fraction(1000, 2000, 0.1), 1100, "truncated, not rounded");

        let mut r3 = enter(&d).unwrap();
        step(&mut r3, &d, -1);
        step(&mut r3, &d, -1);
        assert_eq!(r3.decoder.as_ref().unwrap().timestamp(), 1, "at the first frame");
        assert_eq!(seek_fraction(&mut r3, &d, 1.0), 3, "lands on the last frame");

        assert!(can_prev(None, &d), "live: always true");
        let mut r4 = enter(&d).unwrap();
        step(&mut r4, &d, -1);
        step(&mut r4, &d, -1);
        assert!(!can_prev(Some(&r4), &d), "at the first frame: false");
        let r5 = enter(&d).unwrap();
        assert!(can_prev(Some(&r5), &d), "at the newest: not the first");

        assert_eq!(current(None), -1);
    }

    #[test]
    fn replay_closers_follow_iterm2() {
        use ReplayEvent::*;
        for e in [OtherPaneActive, SplitSelection, PaneClosed, WindowClosed] {
            assert_eq!(on_event(true, e), Close::ClosePanel, "{e:?}");
        }
        assert_eq!(on_event(true, Split), Close::Refuse);
        assert_eq!(on_event(true, TabShown { replaying: true }), Close::OpenPanel);
        assert_eq!(on_event(true, TabShown { replaying: false }), Close::ClosePanel);
        for e in [OtherPaneActive, SplitSelection, PaneClosed, WindowClosed, Split, TabShown { replaying: false }] {
            assert_eq!(on_event(false, e), Close::Nothing, "{e:?}");
        }
        assert_eq!(on_event(false, TabShown { replaying: true }), Close::OpenPanel);
    }

    #[test]
    fn replay_keys_follow_iterm2() {
        assert_eq!(ir_key(&Key::Escape, false, false), IrKey::Exit);
        assert_eq!(ir_key(&Key::Left, false, false), IrKey::Step { dir: -1, count: 1 });
        assert_eq!(ir_key(&Key::Left, true, false), IrKey::Step { dir: -1, count: 15 });
        assert_eq!(ir_key(&Key::Right, false, false), IrKey::Step { dir: 1, count: 1 });
        assert_eq!(ir_key(&Key::Text("a".into()), false, false), IrKey::Beep);
        assert_eq!(ir_key(&Key::Enter, false, false), IrKey::Beep);
        assert_eq!(ir_key(&Key::Up, false, false), IrKey::Beep);
        assert_eq!(ir_key(&Key::Text("b".into()), false, true), IrKey::Pass, "Win+Alt+B");
        assert_eq!(ir_key(&Key::Text("w".into()), false, true), IrKey::Pass, "Win+W");
    }

    #[test]
    fn frame_screen_follows_the_pane() {
        let mut d = dvr::Decoder::default();
        let mut dvr = dvr::Dvr::new(dvr::key_bytes(10, 3) * 1000);
        dvr.append(&screen_rows(10, 3), &[], (8, 1), 1);
        d.seek(&dvr, 1);

        let s = frame_screen(&d, 10, 5, Palette::default(), Look::default());
        assert_eq!(s.term().columns(), 10);
        assert_eq!(s.term().screen_lines(), 5);
        for y in 0..3 {
            assert_eq!(s.term().grid()[Line(y)][Column(0)].c, (b'a' + y as u8) as char, "the frame's rows keep their text");
        }
        for y in 3..5 {
            for x in 0..10 {
                assert_eq!(s.term().grid()[Line(y)][Column(x)].c, ' ', "rows past the frame are blank");
            }
        }

        let s2 = frame_screen(&d, 6, 3, Palette::default(), Look::default());
        assert_eq!(s2.term().columns(), 6);
        for y in 0..3 {
            for x in 0..6 {
                assert_eq!(s2.term().grid()[Line(y)][Column(x)].c, (b'a' + y as u8) as char, "columns 0 to 5 are kept");
            }
        }
        let cursor = s2.term().grid().cursor.point;
        assert_eq!(cursor.column, Column(5), "cursor at column 8 clamps to the last column (5)");
        assert_eq!(cursor.line, Line(1));
    }
}
