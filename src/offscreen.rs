//! iTerm2's offscreen command line: when a command's line has scrolled off the top and its
//! output is long, a band at the top of the pane shows the line (`PS:21346-21377`,
//! `VS:832-880`, `TDH:3497-3617`). The rules and the band's geometry and colors are pure.

use crate::layout::Rect;
use crate::palette::Rgb;

/// Points above and below the line inside the band (`iTermOffscreenCommandLineVerticalPadding`,
/// `TDH:70`).
pub const PADDING: f32 = 8.0;

/// One prompt mark and the facts the rules need, in Tandem's line numbering (negative into
/// history).
#[derive(Clone, Debug, PartialEq)]
pub struct PromptFacts {
    pub prompt_line: i32,
    pub command_line: Option<i32>,
    pub command_end_line: Option<i32>,
    pub has_code: bool,
    pub running: bool,
    pub ended: bool,
    /// The height of the command's output range (`VSS:1045-1080`, `VT100GridCoordRangeHeight`).
    pub output_rows: i32,
    pub command_empty: bool,
}

/// Everything `command_line` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub rows: usize,
    pub copy_mode: bool,
    pub user_scrolled: bool,
    /// The cursor's screen row, from 0.
    pub cursor_row: usize,
    pub enabled: bool,
    pub for_current: bool,
    pub alt_screen: bool,
    pub top_line: i32,
    pub lines_above: usize,
    pub history_top: i32,
    pub prompts: Vec<PromptFacts>,
}

/// The line to show in the band, or None.
pub fn command_line(f: &Facts) -> Option<i32> {
    if f.rows < 5 || f.copy_mode {
        return None;
    }
    // The band would sit over the cursor (`cursorY <= 3`, 1-based, `VSS:481-483`).
    if !f.user_scrolled && f.cursor_row + 1 <= 3 {
        return None;
    }
    if !f.enabled {
        return None;
    }
    // Drawn only when the view is scrolled away from the top (`TV:2153-2154`).
    if f.lines_above == 0 {
        return None;
    }
    // A first visible line below the scrollback in the alternate screen (`VS:835-837`).
    if f.alt_screen && f.top_line >= 0 {
        return None;
    }
    // The screen mark at or before the first visible line.
    let at = f.prompts.iter().rposition(|p| p.prompt_line <= f.top_line)?;
    let mark = &f.prompts[at];
    if !mark.has_code && !(f.for_current && !f.alt_screen) {
        return None;
    }
    let line = mark.command_line?;
    if line >= 0 || line < f.history_top || line == f.top_line {
        return None;
    }
    if mark.output_rows <= 3 || mark.command_empty {
        return None;
    }
    // Issue 11818: not over the output of `clear; command` (`PS:21360-21375`).
    let n = f.prompts.len();
    let last = &f.prompts[n - 1];
    let is_last = at == n - 1;
    let is_penultimate = n >= 2 && at == n - 2 && !last.running && !last.ended;
    if (is_last || is_penultimate) && mark.command_end_line.is_some_and(|e| e + 1 == f.top_line) {
        return None;
    }
    Some(line)
}

/// The band over the top of the text area `pane`.
pub fn band_rect(pane: Rect, cell: (f32, f32), scale: f32) -> Rect {
    Rect { h: cell.1 + 2.0 * PADDING * scale, ..pane }
}

/// The band's fill (with alpha) and outline for the pane's default background and text colors.
pub fn band_colors(bg: Rgb, fg: Rgb) -> ([f32; 4], [f32; 4]) {
    let n = |v: u8| f32::from(v) / 255.0;
    // The background dimmed toward gray 0.15 (`it_colorByDimmingByAmount:`, `CI:1032-1046`),
    // by 0.7 on a dark background and 0.1 on a light one, at alpha 0.5 (`TDH:3515-3529`).
    let amount = if crate::tab_theme::perceived(bg) < 0.5 { 0.7 } else { 0.1 };
    let dim = |v: u8| (1.0 - amount) * n(v) + amount * 0.15;
    let fill = [dim(bg.r), dim(bg.g), dim(bg.b), 0.5];
    // The outline blends the text and background halves (`TDH:3531-3537`; in sRGB, a Decision).
    let mid = |a: u8, b: u8| (n(a) + n(b)) / 2.0;
    (fill, [mid(fg.r, bg.r), mid(fg.g, bg.g), mid(fg.b, bg.b), 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    fn prompt() -> PromptFacts {
        PromptFacts { prompt_line: -60, command_line: Some(-60), command_end_line: Some(-60), has_code: true, running: false, ended: true, output_rows: 30, command_empty: false }
    }

    fn base() -> Facts {
        Facts { rows: 24, copy_mode: false, user_scrolled: true, cursor_row: 20, enabled: true, for_current: false, alt_screen: false, top_line: -40, lines_above: 100, history_top: -500, prompts: vec![prompt()] }
    }

    fn with(f: impl FnOnce(&mut Facts)) -> Option<i32> {
        let mut b = base();
        f(&mut b);
        command_line(&b)
    }

    fn with_prompt(f: impl FnOnce(&mut PromptFacts)) -> Option<i32> {
        with(|b| f(&mut b.prompts[0]))
    }

    #[test]
    fn offscreen_rules_follow_iterm2() {
        assert_eq!(command_line(&base()), Some(-60));
        // Refusals of PS:21346-21377.
        assert_eq!(with(|b| b.rows = 4), None);
        assert_eq!(with(|b| b.rows = 5), Some(-60));
        assert_eq!(with(|b| b.copy_mode = true), None);
        // iTerm2's 1-based cursorY <= 3 is rows 0 to 2 here.
        assert_eq!(with(|b| (b.user_scrolled, b.cursor_row) = (false, 2)), None);
        assert_eq!(with(|b| (b.user_scrolled, b.cursor_row) = (false, 3)), Some(-60));
        assert_eq!(with(|b| (b.user_scrolled, b.cursor_row) = (true, 0)), Some(-60));
        assert_eq!(with(|b| b.enabled = false), None);
        assert_eq!(with(|b| b.lines_above = 0), None);
        // The screen mark at or before the first visible line.
        assert_eq!(with(|b| b.prompts.clear()), None);
        assert_eq!(with(|b| b.prompts[0].prompt_line = -30), None);
        assert_eq!(with(|b| b.prompts[0].prompt_line = -40), Some(-60));
        let mut newer = prompt();
        newer.prompt_line = -50;
        newer.command_line = Some(-50);
        newer.command_end_line = Some(-50);
        assert_eq!(with(|b| b.prompts.push(newer.clone())), Some(-50));
        let mut below = prompt();
        below.prompt_line = -20;
        below.command_line = Some(-20);
        assert_eq!(with(|b| b.prompts.push(below.clone())), Some(-60));
        // Without a code (VS:837, PS:24304-24314).
        assert_eq!(with_prompt(|p| p.has_code = false), None);
        assert_eq!(with(|b| (b.prompts[0].has_code, b.for_current) = (false, true)), Some(-60));
        assert_eq!(with(|b| (b.prompts[0].has_code, b.for_current, b.alt_screen) = (false, true, true)), None);
        // The command's line.
        assert_eq!(with_prompt(|p| p.command_line = None), None);
        assert_eq!(with_prompt(|p| p.command_line = Some(-40)), None);
        assert_eq!(with_prompt(|p| p.command_line = Some(-501)), None);
        assert_eq!(with_prompt(|p| p.command_line = Some(-500)), Some(-500));
        assert_eq!(with_prompt(|p| p.command_line = Some(3)), None);
        // The output must be taller than three rows.
        assert_eq!(with_prompt(|p| p.output_rows = 3), None);
        assert_eq!(with_prompt(|p| p.output_rows = 4), Some(-60));
        assert_eq!(with_prompt(|p| p.command_empty = true), None);
        // Below the scrollback in the alternate screen.
        assert_eq!(with(|b| (b.alt_screen, b.top_line) = (true, 0)), None);
    }

    #[test]
    fn offscreen_issue_11818_clear_before_command() {
        // The newest prompt: refused when its end line plus one is the top line.
        let mut b = base();
        b.prompts[0].command_end_line = Some(-41);
        assert_eq!(command_line(&b), None);
        b.prompts[0].command_end_line = Some(-42);
        assert_eq!(command_line(&b), Some(-60));
        // The penultimate: refused while the last one is neither running nor ended.
        let two = |state: (bool, bool)| {
            let mut b = base();
            b.prompts[0].command_end_line = Some(-41);
            let mut last = prompt();
            (last.prompt_line, last.command_line, last.command_end_line) = (-20, Some(-20), Some(-20));
            (last.running, last.ended) = state;
            b.prompts.push(last);
            b
        };
        assert_eq!(command_line(&two((false, false))), None);
        assert_eq!(command_line(&two((true, false))), Some(-60));
        assert_eq!(command_line(&two((false, true))), Some(-60));
        // An older prompt is not covered by the rule.
        let mut three = two((false, false));
        three.prompts.insert(0, PromptFacts { prompt_line: -90, command_line: Some(-90), command_end_line: Some(-41), ..prompt() });
        three.prompts[1].prompt_line = -70;
        three.prompts[1].command_line = Some(-70);
        three.top_line = -80;
        three.prompts[2].command_end_line = Some(-41);
        assert_eq!(command_line(&three), Some(-90));
    }

    #[test]
    fn offscreen_band_follows_iterm2() {
        let pane = Rect { x: 10.0, y: 30.0, w: 400.0, h: 300.0 };
        assert_eq!(band_rect(pane, (8.0, 16.0), 1.0), Rect { x: 10.0, y: 30.0, w: 400.0, h: 32.0 });
        assert_eq!(band_rect(pane, (8.0, 16.0), 2.0), Rect { x: 10.0, y: 30.0, w: 400.0, h: 48.0 });
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.002);
        let (fill, outline) = band_colors(rgb(0, 0, 0), rgb(255, 255, 255));
        assert!(near(fill, [0.105, 0.105, 0.105, 0.5]), "{fill:?}");
        assert!(near(outline, [0.5, 0.5, 0.5, 1.0]), "{outline:?}");
        let (fill, outline) = band_colors(rgb(255, 255, 255), rgb(0, 0, 0));
        assert!(near(fill, [0.915, 0.915, 0.915, 0.5]), "{fill:?}");
        assert!(near(outline, [0.5, 0.5, 0.5, 1.0]), "{outline:?}");
    }
}
