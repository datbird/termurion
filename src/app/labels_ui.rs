//! iTerm2's content shortcuts: Find URLs, Find All Smart Selection Matches, and Pick
//! Result to Open put a label on each result on screen; typing a label opens it, and
//! Alt with the label copies it.

use super::*;
use crate::find::{self, Match, Pos};

/// Labels for `n` results: 1 to 9, then A to Z; past 35 results, two letters after the
/// digits, so no label is the start of another.
pub(super) fn labels(n: usize) -> Vec<String> {
    let letters = || ('A'..='Z').map(String::from);
    let rest: Box<dyn Iterator<Item = String>> = if n <= 9 + 26 {
        Box::new(letters())
    } else {
        Box::new(letters().flat_map(move |a| letters().map(move |b| format!("{a}{b}"))))
    };
    (1..=9).map(|d| d.to_string()).chain(rest).take(n).collect()
}

/// The result a typed label picks.
pub(super) fn pick(labels: &[String], typed: &str) -> Option<usize> {
    labels.iter().position(|l| l.eq_ignore_ascii_case(typed))
}

/// Some label starts with what was typed, so another key may finish it.
pub(super) fn partial(labels: &[String], typed: &str) -> bool {
    labels.iter().any(|l| l.len() > typed.len() && l.to_ascii_uppercase().starts_with(&typed.to_ascii_uppercase()))
}

/// A modifier key on its own, which must not put the labels away (Alt picks Copy).
pub(super) fn is_modifier_key(key: &winit::keyboard::Key) -> bool {
    use winit::keyboard::{Key, NamedKey};
    matches!(
        key,
        Key::Named(NamedKey::Alt | NamedKey::AltGraph | NamedKey::Shift | NamedKey::Control | NamedKey::Super | NamedKey::Meta | NamedKey::Hyper | NamedKey::CapsLock)
    )
}

/// The labeled results on screen in one pane.
pub(super) struct Shortcuts {
    pub pane: PaneId,
    pub items: Vec<(String, Match, String)>,
    /// Letters typed so far, for two-letter labels.
    pub typed: String,
}

/// Find URLs reads at most this many bytes of a logical line past the visible rows each way (a
/// visible line can be thousands of rows long, P10).
const FIND_LINE_CAP: usize = 100_000;

/// The most rows below a match that a soft boundary may extend it over.
const EXTEND_ROWS: usize = 200;

/// `FindUrlsRegex` compiled the way iTerm2 searches with it: case-sensitive, with bounded
/// backtracking (B6). None for an invalid pattern.
pub(super) fn find_urls_regex(g: &Globals) -> Option<fancy_regex::Regex> {
    fancy_regex::RegexBuilder::new(&crate::smartsel::to_rust_regex(&g.string("FindUrlsRegex")))
        .backtrack_limit(20_000)
        .delegate_size_limit(1 << 20)
        .delegate_dfa_size_limit(1 << 20)
        .build()
        .ok()
}

/// The matches of `re` in `text[window]` as byte ranges of `text`; an error ends the scan.
fn url_hits(re: &fancy_regex::Regex, text: &str, window: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for m in re.find_iter(&text[window.clone()]) {
        let Ok(m) = m else { break };
        if m.end() > m.start() {
            out.push(window.start + m.start()..window.start + m.end());
        }
    }
    out
}

/// The bytes of `line` worth scanning: the visible rows' cells widened by FIND_LINE_CAP each way.
fn scan_window(line: &find::Logical, rows: &std::ops::RangeInclusive<usize>) -> std::ops::Range<usize> {
    let first = line.cells.partition_point(|(_, p)| p.row < *rows.start());
    let last = line.cells.partition_point(|(_, p)| p.row <= *rows.end());
    let lo = line.cells.get(first).map_or(line.text.len(), |c| c.0);
    let hi = line.cells.get(last).map_or(line.text.len(), |c| c.0);
    let (mut lo, mut hi) = (lo.saturating_sub(FIND_LINE_CAP), (hi + FIND_LINE_CAP).min(line.text.len()));
    while !line.text.is_char_boundary(lo) {
        lo -= 1;
    }
    while !line.text.is_char_boundary(hi) {
        hi += 1;
    }
    lo..hi
}

/// The logical lines of the visible rows, widened (as `FIND_LINE_CAP` allows) over the lines that
/// run past them.
fn visible_url_lines(screen: &crate::screen::Screen) -> Vec<find::Logical> {
    find::visible_lines(screen, FIND_LINE_CAP)
}

/// The cell characters of buffer rows `from..from + count` (fewer at the buffer's end).
fn buffer_rows(screen: &crate::screen::Screen, from: usize, count: usize) -> Vec<Vec<char>> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = screen.term().grid();
    let top = -(grid.history_size() as i32);
    let bottom = grid.screen_lines() as i32 - 1;
    (0..count)
        .map(|i| from as i32 + top + i as i32)
        .take_while(|l| *l <= bottom)
        .map(|l| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect())
        .collect()
}

/// Matches in `lines` that start on screen, each with its text.
fn on_screen(lines: &[find::Logical], hits: impl Iterator<Item = (usize, std::ops::Range<usize>)>, rows: &std::ops::RangeInclusive<usize>) -> Vec<(Match, String)> {
    hits.filter(|(_, r)| !r.is_empty())
        .map(|(i, r)| {
            let l = &lines[i];
            (Match { start: l.pos_before(r.start), end: l.pos_before(r.end - 1) }, l.text[r].to_string())
        })
        .filter(|(m, _)| rows.contains(&m.start.row))
        .collect()
}

impl App {
    /// Find URLs (`urls`), Find All Smart Selection Matches, or Pick Result to Open: labels
    /// on the focused pane's results on screen.
    pub(super) fn reveal_shortcuts(&mut self, kind: &str) {
        let Some(id) = self.focused_id() else { return };
        // A browser pane has no cells: Find URLs asks the page, and the text-view pickers do nothing.
        if self.is_browser_pane(id) {
            if kind == "urls" {
                self.browser_find_urls(id);
            }
            return;
        }
        let Some(p) = self.panes.get(&id) else { return };
        let lines = visible_url_lines(&p.screen);
        let rows = find::visible_rows(&p.screen);
        let find_hits: Vec<(Match, String)> = if kind == "pick" {
            let matches = self.finds.get(&id).map(|f| f.results.matches.clone()).unwrap_or_default();
            matches
                .into_iter()
                .filter(|m| rows.contains(&m.start.row))
                .map(|m| {
                    let (a, b) = (super::find_ui::to_point(&p.screen, m.start), super::find_ui::to_point(&p.screen, m.end));
                    (m, p.screen.term().bounds_to_string(a, b))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut items = if kind == "urls" {
            let Some(re) = find_urls_regex(&self.globals) else {
                self.banners.set("action", "Find URLs: invalid regular expression");
                return self.redraw();
            };
            let hits = lines.iter().enumerate().flat_map(|(i, l)| url_hits(&re, &l.text, scan_window(l, &rows)).into_iter().map(move |r| (i, r)).collect::<Vec<_>>());
            let mut items = on_screen(&lines, hits, &rows);
            if self.globals.bool("FindURLsRespectsSoftBoundaries") {
                let Some(p) = self.panes.get(&id) else { return };
                for (m, text) in items.iter_mut() {
                    let Some((lo, hi)) = super::selection_ui::click_window(&p.screen, m.start.row, m.start.col) else { continue };
                    if m.end.row != m.start.row || m.end.col < hi {
                        continue;
                    }
                    let grid_rows = buffer_rows(&p.screen, m.start.row, EXTEND_ROWS);
                    if let Some(((row, col), t)) = crate::seltools::extend_across_divider(&grid_rows, (0, m.start.col), (lo, hi), &re) {
                        m.end = Pos { row: m.start.row + row, col };
                        *text = t;
                    }
                }
            }
            items
        } else if !find_hits.is_empty() {
            find_hits
        } else {
            let rules = self.store.get(&p.profile).map(crate::smartsel::rules).unwrap_or_default();
            // Every rule of Normal precision or more, the longest match first where they overlap.
            let mut hits: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
            for rule in rules.iter().filter(|r| r.precision >= 1.0) {
                let Ok(re) = fancy_regex::Regex::new(&crate::smartsel::to_rust_regex(&rule.regex)) else { continue };
                for (i, l) in lines.iter().enumerate() {
                    hits.extend(re.find_iter(&l.text).flatten().map(|m| (i, m.range())));
                }
            }
            hits.sort_by_key(|(i, r)| (*i, r.start, std::cmp::Reverse(r.end)));
            let mut kept: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
            for (i, r) in hits {
                if kept.last().is_none_or(|(li, lr)| *li != i || r.start >= lr.end) {
                    kept.push((i, r));
                }
            }
            on_screen(&lines, kept.into_iter(), &rows)
        };
        items.sort_by_key(|(m, _)| m.start);
        if items.is_empty() {
            self.banners.set("action", "Nothing to pick on screen.");
            return self.redraw();
        }
        let names = labels(items.len());
        let items = names.into_iter().zip(items).map(|(l, (m, t))| (l, m, t)).collect();
        self.shortcuts = Some(Shortcuts { pane: id, items, typed: String::new() });
        self.belt_yield_keys();
        self.redraw();
    }

    /// A key while labels show. Returns true when the labels took it.
    pub(super) fn shortcut_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let Some(sc) = self.shortcuts.as_mut() else { return false };
        if is_modifier_key(&ev.logical_key) {
            return true;
        }
        let text = ev.text.as_deref().unwrap_or_default();
        let is_label_key = text.len() == 1 && text.chars().all(|c| c.is_ascii_alphanumeric());
        if !is_label_key {
            // Esc, or any other key, puts the labels away.
            self.shortcuts = None;
            self.redraw();
            return true;
        }
        sc.typed.push_str(text);
        let names: Vec<String> = sc.items.iter().map(|(l, _, _)| l.clone()).collect();
        if let Some(i) = pick(&names, &sc.typed) {
            let (pane, text) = (sc.pane, sc.items[i].2.clone());
            self.shortcuts = None;
            if self.mods.alt {
                if let Some(cb) = &mut self.clipboard {
                    if let Err(e) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                }
                self.banners.set("action", "Copied.");
            } else {
                self.open_text(pane, &text);
            }
        } else if !partial(&names, &sc.typed) {
            self.shortcuts = None;
        }
        self.redraw();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Final review fix 4 (P10): Find URLs builds the visible rows' lines widened by at most
    /// FIND_LINE_CAP bytes each way, not the whole wrapped line a program printed.
    #[test]
    fn find_urls_text_is_bounded_on_a_huge_line() {
        let mut s = crate::screen::Screen::with_scrollback(80, 24, 20_000);
        s.feed("x".repeat(1_000_000).as_bytes());
        s.feed(b"\r\ntail\r\n");
        find::work::take();
        let lines = visible_url_lines(&s);
        let read = find::work::take();
        assert!(read <= 2 * FIND_LINE_CAP + 40 * 80, "{read} cells read");
        let bytes: usize = lines.iter().map(|l| l.text.len()).sum();
        assert!(bytes <= 2 * FIND_LINE_CAP + 40 * 80, "{bytes}");
        assert!(lines.iter().any(|l| l.text == "tail"), "the short line is whole");
        let rows = find::visible_rows(&s);
        assert!(lines.iter().flat_map(|l| l.cells.iter()).any(|(_, p)| rows.contains(&p.row)));
    }

    fn urls(g: &Globals, text: &str) -> Vec<String> {
        let re = find_urls_regex(g).expect("pattern");
        url_hits(&re, text, 0..text.len()).into_iter().map(|r| text[r].to_string()).collect()
    }

    #[test]
    fn find_urls_pattern_follows_iterm2() {
        let mut g = Globals::empty(std::path::Path::new("unused.toml"));
        assert_eq!(urls(&g, "see https://ex-a.com:8080/a?b=1&c=(2) now"), ["https://ex-a.com:8080/a?b=1&c=(2)"]);
        for none in ["ftp://x.org", "www.x.org", "mailto:a@b.c", "HTTPS://X.ORG"] {
            assert!(urls(&g, none).is_empty(), "{none}");
        }
        assert_eq!(urls(&g, "http://a.b\u{2502}c"), ["http://a.b"]);
        g.set("FindUrlsRegex", toml::Value::String("ftp://[a-z.]+".into()));
        assert_eq!(urls(&g, "ftp://x.org"), ["ftp://x.org"]);
        g.set("FindUrlsRegex", toml::Value::String("(unclosed".into()));
        assert!(find_urls_regex(&g).is_none());
    }

    /// The inputs sit between the configured limits and the libraries' defaults (measured on
    /// EXTREME: 16 'a's is above 20 000 backtracking steps and below the default 1 000 000;
    /// 50 000 repeats compile under the default 10 MiB size limit and over 1 MiB), so removing
    /// either bound from `find_urls_regex` fails this test.
    #[test]
    fn find_urls_pattern_is_bounded() {
        let mut g = Globals::empty(std::path::Path::new("unused.toml"));
        // Backtracking: the lookahead needs the backtracking VM; the limit is an error, which ends
        // the scan with no hits, where the default limit would finish with no match.
        g.set("FindUrlsRegex", toml::Value::String("(a+)+(?=c)".into()));
        let text = "a".repeat(16);
        let re = find_urls_regex(&g).expect("pattern");
        assert!(re.find(&text).is_err(), "the 20 000 step limit is hit");
        assert!(url_hits(&re, &text, 0..text.len()).is_empty());
        let default = fancy_regex::Regex::new("(a+)+(?=c)").unwrap();
        assert!(matches!(default.find(&text), Ok(None)), "the same input is fine under the default");
        // Size: a pattern over the 1 MiB compile limit is refused, one the default would accept.
        g.set("FindUrlsRegex", toml::Value::String("(?:ab){50000}".into()));
        assert!(find_urls_regex(&g).is_none());
        assert!(fancy_regex::Regex::new("(?:ab){50000}").is_ok(), "the default limit accepts it");
        // A small pattern still compiles.
        g.set("FindUrlsRegex", toml::Value::String("https?://\\S+".into()));
        assert!(find_urls_regex(&g).is_some());
    }

    #[test]
    fn labels_run_digits_then_letters() {
        let l = labels(35);
        assert_eq!(&l[..3], ["1", "2", "3"]);
        assert_eq!(l[8], "9");
        assert_eq!(l[9], "A");
        assert_eq!(l[34], "Z");
        let l = labels(40);
        assert_eq!(l[9], "AA");
        assert_eq!(l[10], "AB");
        assert_eq!(labels(0).len(), 0);
        assert_eq!(labels(9 + 26 * 26).last().map(String::as_str), Some("ZZ"));
    }

    #[test]
    fn a_typed_label_picks_its_result() {
        let l = labels(20);
        assert_eq!(pick(&l, "3"), Some(2));
        assert_eq!(pick(&l, "a"), Some(9));
        assert_eq!(pick(&l, "?"), None);
        let l = labels(40);
        assert_eq!(pick(&l, "A"), None);
        assert!(partial(&l, "a"));
        assert_eq!(pick(&l, "ab"), Some(10));
        assert!(!partial(&l, "Q"));
    }

    #[test]
    fn modifier_keys_keep_the_labels() {
        use winit::keyboard::{Key, NamedKey};
        assert!(is_modifier_key(&Key::Named(NamedKey::Alt)));
        assert!(is_modifier_key(&Key::Named(NamedKey::Shift)));
        assert!(is_modifier_key(&Key::Named(NamedKey::Control)));
        assert!(!is_modifier_key(&Key::Named(NamedKey::Escape)));
        assert!(!is_modifier_key(&Key::Character("a".into())));
    }
}
