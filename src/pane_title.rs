//! The pane title bar model (`SessionTitleView.m`, `SessionView.m`, `PTYTab.m`, `iTermTheme.m`):
//! when a tab shows title bars, the ordinals, the title text, the geometry of every part and the
//! colors. Pure; the drawing and clicks live in `app::pane_title_ui`.

use crate::hexcolor;
use crate::layout::Rect;
use crate::palette::Rgb;
use crate::tab_theme::{self, TabStyle};

/// Points added to the status bar height (`iTermGetSessionViewTitleHeight`, `SV:53-55`).
pub const TITLE_PT: f32 = 1.0;

const MARGIN: f32 = 5.0;
const CLOSE: f32 = 17.0;
const MENU: f32 = 16.0;
const MENU_RIGHT: f32 = 6.0;
const LOCK: f32 = 14.0;
const LINE: f32 = 1.0;

/// The title bar's height in pixels.
pub fn height(status_bar_pt: f32, scale: f32) -> f32 {
    ((status_bar_pt + TITLE_PT) * scale).round()
}

/// `PTYTab updatePaneTitles` (`PTYTab.m:610-622`): the same answer for every pane of a tab.
/// `any_top_bar` is a per-pane status bar at the top.
pub fn shows_titles(show: bool, even_one: bool, panes: usize, any_top_bar: bool) -> bool {
    (show && (even_one || panes > 1)) || any_top_bar
}

/// The shortcut ordinal of each pane in reading order (`updateSessionOrdinals`,
/// `PTYTab.m:678-691`): 1 to 8, later panes 0, and the last pane 9 when the counter reached 9
/// (so with exactly eight panes the eighth shows 9, a kept quirk).
pub fn ordinals(n: usize) -> Vec<u32> {
    let mut i = 1u32;
    let mut out: Vec<u32> = (0..n)
        .map(|_| {
            if i < 9 {
                i += 1;
                i - 1
            } else {
                0
            }
        })
        .collect();
    if i == 9 {
        if let Some(last) = out.last_mut() {
            *last = 9;
        }
    }
    out
}

/// The label (`SessionTitleView.m:250-277`): the shortcut prefix, the ordinal and three spaces
/// before the title when the pane switch modifier is not "none". The title is program text
/// (OSC, tmux, Title Function) so it goes through `inert_files::shown_name` first (P1).
pub fn title_text(title: &str, ordinal: u32, pane_tag: i64) -> String {
    let shown = crate::inert_files::shown_name(title);
    match crate::status_tool::modifier_prefix(pane_tag) {
        Some(prefix) if ordinal != 0 => format!("{prefix}+{ordinal}   {shown}"),
        _ => shown,
    }
}

/// The label cut to `cells` terminal cells by display width, tail truncated with an ellipsis
/// (`STV` truncates the tail). The text is program text: control, bidirectional and
/// zero-width characters are dropped (so no mark is left unpaired), a cluster (a base with
/// its combining marks, a pair of regional indicators) is never split, and only as much of
/// the text as can matter is read.
pub fn fit_label(text: &str, cells: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let is_indicator = |c: char| ('\u{1f1e6}'..='\u{1f1ff}').contains(&c);
    let mut clusters: Vec<(String, usize)> = Vec::new();
    let mut total = 0usize;
    for c in text.chars().filter(|&c| !crate::inert_files::is_hidden(c)) {
        let w = c.width().unwrap_or(0);
        let joins = match clusters.last() {
            Some((last, _)) => w == 0 || (is_indicator(c) && last.chars().count() == 1 && last.chars().all(is_indicator)),
            None => false,
        };
        if joins {
            let last = clusters.last_mut().expect("joins implies a cluster");
            last.0.push(c);
            last.1 += w;
        } else {
            clusters.push((c.to_string(), w));
        }
        total += w;
        if total > cells {
            break;
        }
    }
    if total <= cells {
        return clusters.into_iter().map(|(s, _)| s).collect();
    }
    if cells == 0 {
        return String::new();
    }
    while total + 1 > cells {
        let Some((_, w)) = clusters.pop() else { break };
        total -= w;
    }
    let mut out: String = clusters.into_iter().map(|(s, _)| s).collect();
    out.push('\u{2026}');
    out
}

/// Where every part of a title bar sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub bar: Rect,
    pub close: Rect,
    pub label: Rect,
    pub lock: Option<Rect>,
    pub menu: Rect,
    /// The slot of a per-pane status bar in the title (`layoutStatusBar`).
    pub status: Rect,
    /// The 1 point line along the bottom edge.
    pub line: Rect,
}

/// `layoutSubviews` and `layoutStatusBar` (`STV:132-157`, `:182-195`) in pixels, top down.
pub fn geometry(bar: Rect, scale: f32, locked: bool) -> Geometry {
    let s = scale;
    let sq = |x: f32, size: f32| Rect { x, y: bar.y + (bar.h - size) / 2.0, w: size, h: size };
    let close = sq(bar.x + MARGIN * s, CLOSE * s);
    let menu = sq(bar.x + bar.w - (MENU + MENU_RIGHT) * s, MENU * s);
    let lock = locked.then(|| sq(menu.x - (MARGIN + LOCK) * s, LOCK * s));
    let left = close.x + close.w + MARGIN * s;
    let mut right = menu.x - MARGIN * s;
    let status = Rect { x: left, y: bar.y, w: (right - left).max(0.0), h: (bar.h - LINE * s).max(0.0) };
    if let Some(l) = lock {
        right -= l.w + MARGIN * s;
    }
    let label = Rect { x: left, y: bar.y, w: (right - left).max(0.0), h: bar.h };
    let line = Rect { x: bar.x, y: bar.y + bar.h - LINE * s, w: bar.w, h: LINE * s };
    Geometry { bar, close, label, lock, menu, status, line }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Close,
    Menu,
    Lock,
    Bar,
}

/// The part under a point.
pub fn hit(g: &Geometry, x: f32, y: f32) -> Option<Hit> {
    if g.close.contains(x, y) {
        Some(Hit::Close)
    } else if g.menu.contains(x, y) {
        Some(Hit::Menu)
    } else if g.lock.is_some_and(|l| l.contains(x, y)) {
        Some(Hit::Lock)
    } else if g.bar.contains(x, y) {
        Some(Hit::Bar)
    } else {
        None
    }
}

/// The colors of a title bar. `background` is opaque: the Minimal theme's transparency alpha
/// (`transparencyAlpha`, `iTermTheme.m:134-150`) is multiplied in by the caller (13g's seam,
/// `any_pane_is_transparent`), Minimal branch only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub background: Rgb,
    /// The background's alpha: the pane's `transparencyAlpha` in the Minimal theme's session
    /// colored branch (`iTermTheme.m:322-353`, `PSMShouldExtendTransparencyIntoMinimalTabBar`),
    /// 1 everywhere else.
    pub background_alpha: f32,
    pub text: (Rgb, f32),
    pub menu: (Rgb, f32),
    /// The bottom line and its alpha; `None` in the Minimal theme.
    pub line: Option<(Rgb, f32)>,
}

pub struct Parts<'a> {
    /// The chosen theme (`tab_style`).
    pub style: TabStyle,
    /// One of the four fixed styles (`resolved_style`).
    pub resolved: TabStyle,
    /// The pane is not the tab's active pane.
    pub dimmed: bool,
    /// `adjustedDimmingAmount`.
    pub dim_amount: f32,
    pub dim_only_text: bool,
    pub dim_inactive: bool,
    pub window_focused: bool,
    pub session_bg: Rgb,
    pub tab_color: Option<Rgb>,
    pub distinct_tab_colors: bool,
    pub custom_bg: &'a str,
    pub custom_text: &'a str,
    /// A per-pane status bar's layout background; wins over everything.
    pub bar_bg: Option<Rgb>,
    /// `MinimalTextLegibilityAdjustment`.
    pub legibility: f32,
    pub locked: bool,
    /// The pane's `transparencyAlpha` (13g's `pane_alpha` on a see-through window, else 1).
    pub transparency_alpha: f32,
}

/// A fixed style for a switch iTerm2 asserts on (Automatic, Compact, Minimal): Dark.
fn fixed(resolved: TabStyle) -> TabStyle {
    match resolved {
        TabStyle::Light | TabStyle::Dark | TabStyle::LightHighContrast | TabStyle::DarkHighContrast => resolved,
        _ => TabStyle::Dark,
    }
}

fn gray_background(resolved: TabStyle, inactive: bool) -> Rgb {
    let level = match (fixed(resolved), inactive) {
        (TabStyle::Light, true) => 0.58,
        (TabStyle::Light, false) => 0.70,
        (TabStyle::LightHighContrast, true) => 0.68,
        (TabStyle::LightHighContrast, false) => 0.80,
        (TabStyle::DarkHighContrast, true) => 0.08,
        (TabStyle::DarkHighContrast, false) => 0.17,
        (_, true) => 0.18,
        (_, false) => 0.27,
    };
    hexcolor::white(level)
}

/// `backgroundColorForDecorativeSubviewsForTabColor:` (`TH:281-320`) for a non-Minimal theme.
fn tinted(tab: Rgb, resolved: TabStyle) -> Rgb {
    let (h, s, b) = hexcolor::to_hsb(tab);
    match fixed(resolved) {
        TabStyle::Light => hexcolor::from_hsb(h, s * 0.5, b.max(0.7)),
        TabStyle::LightHighContrast => hexcolor::from_hsb(h, s * 0.25, b.max(0.85)),
        TabStyle::DarkHighContrast => hexcolor::from_hsb(h, s * 0.95, b.min(0.15)),
        _ => hexcolor::from_hsb(h, s * 0.75, b.min(0.3)),
    }
}

fn background(p: &Parts) -> Rgb {
    if let Some(bg) = p.bar_bg {
        return bg;
    }
    if let Some(custom) = hex_word(p.custom_bg) {
        return if p.dimmed && p.dim_inactive { hexcolor::dim_by(custom, 0.3) } else { custom };
    }
    let minimal = p.style == TabStyle::Minimal;
    if minimal {
        if let Some(tab) = p.tab_color.filter(|_| p.distinct_tab_colors) {
            return if p.dimmed { hexcolor::dim_by(tab, 0.3) } else { tab };
        }
    }
    match p.tab_color {
        Some(tab) if !minimal => {
            let c = tinted(tab, p.resolved);
            if p.dimmed { hexcolor::dim_by(c, 0.3) } else { c }
        }
        _ if minimal => {
            if p.dimmed && !p.dim_only_text { tab_theme::dimmed(p.session_bg, p.dim_amount) } else { p.session_bg }
        }
        _ => gray_background(p.resolved, p.dimmed),
    }
}

/// A custom color word counts only with a leading "#" (`hasPrefix:@"#"`) and a valid value.
fn hex_word(word: &str) -> Option<Rgb> {
    if word.starts_with('#') { hexcolor::parse(word) } else { None }
}

/// The colors of a title bar (`SessionTitleView updateTextColor`, `drawRect:`; `SV:2689-2697`).
pub fn look(p: &Parts) -> Look {
    let background = background(p);
    let minimal = p.style == TabStyle::Minimal;
    let text = if let Some(c) = hex_word(p.custom_text) {
        (c, 1.0)
    } else if minimal {
        let f = tab_theme::minimal_text(background, true, p.window_focused, p.legibility);
        (f.color, f.alpha)
    } else {
        let dim = p.dim_amount > 0.0;
        let level = match (fixed(p.resolved), dim) {
            (TabStyle::Light, true) => 0.3,
            (TabStyle::Light, false) => 0.2,
            (TabStyle::LightHighContrast, _) => 0.0,
            (TabStyle::DarkHighContrast, _) => 1.0,
            (_, true) => 0.6,
            (_, false) => 0.8,
        };
        (hexcolor::white(level), 1.0)
    };
    let menu = if hex_word(p.custom_text).is_some() { (text.0, 1.0) } else { (text.0, 0.55) };
    let line = (!minimal).then(|| {
        if tab_theme::perceived(background) > 0.5 { (hexcolor::white(0.0), 0.25) } else { (hexcolor::white(1.0), 0.25) }
    });
    Look { background, background_alpha: background_alpha(p), text, menu, line }
}

/// The background's alpha (P11): iTerm2 extends transparency into the title bar only in the
/// Minimal theme (`PSMShouldExtendTransparencyIntoMinimalTabBar`) and only where it draws the
/// session color (`dimmedBackgroundColorWithAppearance:`, `iTermTheme.m:322-353`); a custom
/// color, a distinct tab color (`iTermTheme.m:134-142`) and a status bar's color are opaque.
fn background_alpha(p: &Parts) -> f32 {
    let session_colored = p.style == TabStyle::Minimal && p.bar_bg.is_none() && hex_word(p.custom_bg).is_none() && !(p.tab_color.is_some() && p.distinct_tab_colors);
    if session_colored { p.transparency_alpha.clamp(0.0, 1.0) } else { 1.0 }
}

/// The pane's rectangle cut into its title bar (on top) and the rest.
pub fn split(rect: Rect, height: f32) -> (Rect, Rect) {
    let h = height.min(rect.h).max(0.0);
    (Rect { x: rect.x, y: rect.y, w: rect.w, h }, Rect { x: rect.x, y: rect.y + h, w: rect.w, h: rect.h - h })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn title_bars_show_as_iterm2() {
        assert!(!shows_titles(true, false, 1, false));
        assert!(shows_titles(true, false, 2, false));
        assert!(shows_titles(true, true, 1, false));
        assert!(!shows_titles(false, true, 3, false));
        assert!(shows_titles(false, false, 1, true));
        assert_eq!(height(21.0, 1.0), 22.0);
        assert_eq!(height(21.0, 1.5), 33.0);
    }

    #[test]
    fn ordinals_follow_iterm2() {
        assert_eq!(ordinals(3), vec![1, 2, 3]);
        assert_eq!(ordinals(8), vec![1, 2, 3, 4, 5, 6, 7, 9]);
        assert_eq!(ordinals(9), vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(ordinals(10), vec![1, 2, 3, 4, 5, 6, 7, 8, 0, 9]);
        let twelve = ordinals(12);
        assert_eq!(&twelve[7..], &[8, 0, 0, 0, 9]);
        assert_eq!(ordinals(1), vec![1]);
    }

    #[test]
    fn title_text_follows_iterm2() {
        assert_eq!(title_text("bash", 0, 5), "bash");
        assert_eq!(title_text("bash", 2, 9), "bash");
        assert_eq!(title_text("bash", 2, 5), "Alt+2   bash");
        assert_eq!(title_text("vim", 9, 6), "Win+Alt+9   vim");
        assert_eq!(title_text("x", 1, 1), "Ctrl+1   x");
        assert_eq!(title_text("x", 3, 4), "Win+3   x");
        // P1: a title is untrusted program text.
        let hostile = title_text("a\u{202e}b\u{200b}\r\nc", 2, 5);
        assert_eq!(hostile, "Alt+2   abc");
        let long = title_text(&"w".repeat(10_000), 2, 5);
        assert!(long.chars().count() <= 200 + 1 + "Alt+2   ".len());
        assert!(!long.contains('\n'));
    }

    #[test]
    fn geometry_follows_iterm2() {
        let g = geometry(r(0.0, 0.0, 400.0, 22.0), 1.0, false);
        assert_eq!(g.close, r(5.0, 2.5, 17.0, 17.0));
        assert_eq!(g.menu, r(378.0, 3.0, 16.0, 16.0));
        assert_eq!(g.label, r(27.0, 0.0, 346.0, 22.0));
        assert_eq!(g.lock, None);
        assert_eq!(g.status, r(27.0, 0.0, 346.0, 21.0));
        assert_eq!(g.line, r(0.0, 21.0, 400.0, 1.0));
        let l = geometry(r(0.0, 0.0, 400.0, 22.0), 1.0, true);
        assert_eq!(l.lock, Some(r(359.0, 4.0, 14.0, 14.0)));
        assert_eq!(l.label.w, 327.0);
        assert_eq!(l.status.w, 346.0);
        let s = geometry(r(100.0, 50.0, 400.0, 33.0), 1.5, false);
        assert_eq!(s.close, r(107.5, 53.75, 25.5, 25.5));
        assert_eq!(s.menu, r(467.0, 54.5, 24.0, 24.0));
        assert_eq!(s.line.h, 1.5);
        assert_eq!(geometry(r(0.0, 0.0, 30.0, 22.0), 1.0, true).label.w, 0.0);
        let c = |q: Rect| (q.x + q.w / 2.0, q.y + q.h / 2.0);
        let (x, y) = c(l.close);
        assert_eq!(hit(&l, x, y), Some(Hit::Close));
        let (x, y) = c(l.menu);
        assert_eq!(hit(&l, x, y), Some(Hit::Menu));
        let (x, y) = c(l.lock.unwrap());
        assert_eq!(hit(&l, x, y), Some(Hit::Lock));
        assert_eq!(hit(&l, 100.0, 10.0), Some(Hit::Bar));
        assert_eq!(hit(&l, 100.0, 30.0), None);
        assert_eq!(hit(&l, -1.0, 10.0), None);
    }

    #[test]
    fn split_cuts_the_title_off_the_top() {
        let (bar, rest) = split(r(10.0, 20.0, 300.0, 100.0), 22.0);
        assert_eq!(bar, r(10.0, 20.0, 300.0, 22.0));
        assert_eq!(rest, r(10.0, 42.0, 300.0, 78.0));
        let (bar, rest) = split(r(0.0, 0.0, 50.0, 10.0), 22.0);
        assert_eq!((bar.h, rest.h), (10.0, 0.0));
    }

    fn parts(resolved: TabStyle, dimmed: bool) -> Parts<'static> {
        Parts {
            style: TabStyle::Automatic,
            resolved,
            dimmed,
            dim_amount: if dimmed { 0.4 } else { 0.0 },
            dim_only_text: false,
            dim_inactive: false,
            window_focused: true,
            session_bg: rgb(0, 0, 0),
            tab_color: None,
            distinct_tab_colors: false,
            custom_bg: "",
            custom_text: "",
            bar_bg: None,
            legibility: 0.0,
            locked: false,
            transparency_alpha: 1.0,
        }
    }

    /// P11: the Minimal title bar takes the pane's transparency only where iTerm2's Minimal
    /// branch draws the session color (`iTermTheme.m:322-353`); a custom color, a distinct tab
    /// color, a status bar's color and every other theme stay opaque.
    #[test]
    fn minimal_background_alpha_follows_transparency() {
        let minimal = |dimmed: bool| {
            let mut p = parts(TabStyle::Dark, dimmed);
            p.style = TabStyle::Minimal;
            p.transparency_alpha = 0.6;
            p
        };
        assert_eq!(look(&minimal(false)).background_alpha, 0.6);
        assert_eq!(look(&minimal(true)).background_alpha, 0.6);
        let mut p = minimal(true);
        p.dim_only_text = true;
        assert_eq!(look(&p).background_alpha, 0.6);
        // A tab color alone still draws the session color in Minimal.
        let mut p = minimal(false);
        p.tab_color = Some(rgb(255, 0, 0));
        assert_eq!(look(&p).background_alpha, 0.6);
        p.distinct_tab_colors = true;
        assert_eq!(look(&p).background_alpha, 1.0);
        let mut p = minimal(false);
        p.custom_bg = "#112233";
        assert_eq!(look(&p).background_alpha, 1.0);
        let mut p = minimal(false);
        p.bar_bg = Some(rgb(1, 2, 3));
        assert_eq!(look(&p).background_alpha, 1.0);
        // Other themes never extend transparency into the title bar.
        let mut p = parts(TabStyle::Dark, false);
        p.transparency_alpha = 0.6;
        assert_eq!(look(&p).background_alpha, 1.0);
        p.tab_color = Some(rgb(255, 0, 0));
        assert_eq!(look(&p).background_alpha, 1.0);
        // An opaque pane keeps an opaque bar.
        let mut p = minimal(false);
        p.transparency_alpha = 1.0;
        assert_eq!(look(&p).background_alpha, 1.0);
    }

    #[test]
    fn colors_follow_iterm2() {
        let w = hexcolor::white;
        let l = look(&parts(TabStyle::Dark, false));
        assert_eq!((l.background, l.text, l.line), (w(0.27), (w(0.8), 1.0), Some((w(1.0), 0.25))));
        let l = look(&parts(TabStyle::Dark, true));
        assert_eq!((l.background, l.text.0), (w(0.18), w(0.6)));
        let l = look(&parts(TabStyle::Light, false));
        assert_eq!((l.background, l.text, l.line), (w(0.70), (w(0.2), 1.0), Some((w(0.0), 0.25))));
        let l = look(&parts(TabStyle::Light, true));
        assert_eq!((l.background, l.text.0), (w(0.58), w(0.3)));
        for (st, act, dim, text) in [
            (TabStyle::LightHighContrast, 0.80, 0.68, 0.0),
            (TabStyle::DarkHighContrast, 0.17, 0.08, 1.0),
        ] {
            assert_eq!(look(&parts(st, false)).background, w(act));
            let d = look(&parts(st, true));
            assert_eq!((d.background, d.text.0), (w(dim), w(text)));
        }
        // Tab colors.
        let red = rgb(255, 0, 0);
        let with = |st: TabStyle, dimmed: bool| {
            let mut p = parts(st, dimmed);
            p.tab_color = Some(red);
            look(&p).background
        };
        assert_eq!(with(TabStyle::Dark, false), rgb(77, 19, 19));
        assert_eq!(with(TabStyle::Dark, true), hexcolor::dim_by(rgb(77, 19, 19), 0.3));
        assert_eq!(with(TabStyle::Light, false), rgb(255, 128, 128));
        assert_eq!(with(TabStyle::LightHighContrast, false), hexcolor::from_hsb(0.0, 0.25, 1.0));
        assert_eq!(with(TabStyle::DarkHighContrast, false), hexcolor::from_hsb(0.0, 0.95, 0.15));
        // Minimal.
        let minimal = |dimmed: bool| {
            let mut p = parts(TabStyle::Dark, dimmed);
            p.style = TabStyle::Minimal;
            p
        };
        let l = look(&minimal(false));
        assert_eq!(l.background, rgb(0, 0, 0));
        assert_eq!(l.line, None);
        let f = tab_theme::minimal_text(rgb(0, 0, 0), true, true, 0.0);
        assert_eq!(l.text, (f.color, f.alpha));
        assert_eq!(look(&minimal(true)).background, rgb(51, 51, 51));
        let mut p = minimal(true);
        p.dim_only_text = true;
        assert_eq!(look(&p).background, rgb(0, 0, 0));
        let mut p = minimal(true);
        p.tab_color = Some(red);
        assert_eq!(look(&p).background, rgb(51, 51, 51));
        p.distinct_tab_colors = true;
        assert_eq!(look(&p).background, hexcolor::dim_by(red, 0.3));
        p.dimmed = false;
        p.dim_amount = 0.0;
        assert_eq!(look(&p).background, red);
        // Custom colors.
        let custom = |bg: &'static str, dimmed: bool, dim_inactive: bool| {
            let mut p = parts(TabStyle::Light, dimmed);
            p.tab_color = Some(red);
            p.custom_bg = bg;
            p.dim_inactive = dim_inactive;
            look(&p).background
        };
        assert_eq!(custom("#336699", false, true), rgb(0x33, 0x66, 0x99));
        assert_eq!(custom("#336699", true, true), rgb(47, 83, 119));
        assert_eq!(custom("#336699", true, false), rgb(0x33, 0x66, 0x99));
        assert_eq!(custom("336699", false, false), rgb(255, 128, 128));
        assert_eq!(custom("", false, false), rgb(255, 128, 128));
        let mut p = parts(TabStyle::Dark, false);
        p.custom_text = "#ff0000";
        let l = look(&p);
        assert_eq!((l.text, l.menu), ((red, 1.0), (red, 1.0)));
        p.custom_text = "";
        assert_eq!(look(&p).menu, (w(0.8), 0.55));
        p.bar_bg = Some(rgb(1, 2, 3));
        p.custom_bg = "#336699";
        assert_eq!(look(&p).background, rgb(1, 2, 3));
    }
}
