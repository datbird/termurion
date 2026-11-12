//! Auto detection for FX profiles: which profile reads each multiplexer region. A profile
//! scores against the region's visible text, the foreground job and the box around the cursor.

use alacritty_terminal::grid::Dimensions;
use crate::fx_profile::FxRules;
use crate::fx_store::Store;
use crate::scene::{effects_cursor, has_box, output_watch};
use crate::screen::Screen;
use std::sync::Arc;

/// Top row, bottom row, first column, last column.
pub type Region = (usize, usize, usize, usize);

const STRONG: u8 = 3;
const KEEP: usize = 16;

/// The region's visible text for searching: rows joined by newlines (blank rows left out), and
/// a set of the byte pairs that occur in it, so most texts that are not there fail without a scan.
struct Haystack {
    text: String,
    pairs: Vec<u64>,
    seen: std::collections::HashMap<String, bool>,
}

impl Haystack {
    fn new(screen: &Screen, (top, bottom, left, right): Region) -> Haystack {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags;
        let term = screen.term();
        let offset = term.grid().display_offset() as i32;
        let mut text = String::new();
        for r in top..=bottom.min(term.screen_lines().saturating_sub(1)) {
            let line = &term.grid()[Line(r as i32 - offset)];
            let from = text.len();
            for c in left..=right.min(term.columns().saturating_sub(1)) {
                let cell = &line[Column(c)];
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                }
            }
            if text[from..].trim().is_empty() {
                text.truncate(from);
            } else {
                text.push('\n');
            }
        }
        let mut pairs = vec![0u64; 1024];
        for w in text.as_bytes().windows(2) {
            let i = (w[0] as usize) << 8 | w[1] as usize;
            pairs[i >> 6] |= 1 << (i & 63);
        }
        Haystack { text, pairs, seen: Default::default() }
    }

    fn has(&mut self, needle: &str) -> bool {
        if let Some(&known) = self.seen.get(needle) {
            return known;
        }
        let possible = needle.as_bytes().windows(2).all(|w| {
            let i = (w[0] as usize) << 8 | w[1] as usize;
            self.pairs[i >> 6] & (1 << (i & 63)) != 0
        });
        let found = possible && self.text.contains(needle);
        self.seen.insert(needle.to_string(), found);
        found
    }
}

/// The nearest non-space cell left of the cursor on its row, within the region.
fn prompt_char(screen: &Screen, cursor: (usize, usize), left: usize) -> Option<char> {
    use alacritty_terminal::index::{Column, Line};
    let term = screen.term();
    let offset = term.grid().display_offset() as i32;
    let (row, col) = cursor;
    if row >= term.screen_lines() {
        return None;
    }
    let line = &term.grid()[Line(row as i32 - offset)];
    (left..col.min(term.columns())).rev().map(|c| line[Column(c)].c).find(|c| !c.is_whitespace())
}

/// The score of one profile given the pieces `choose` computes once per region.
fn score_with(rules: &FxRules, screen: &Screen, text: &mut Haystack, cursor: Option<(usize, usize)>, left: usize, job: Option<&str>) -> (u8, u32) {
    let m = &rules.matching;
    if let Some(j) = job {
        let hits = m.program.iter().filter(|p| crate::proctree::same_job(p, j)).count() as u32;
        if hits > 0 {
            return (4, hits);
        }
    }
    let hits = m.text.iter().filter(|t| text.has(t)).count() as u32;
    if hits > 0 {
        return (3, hits);
    }
    if !m.prompt.is_empty() && cursor.and_then(|c| prompt_char(screen, c, left)).is_some_and(|ch| m.prompt.contains(&ch)) {
        return (2, 1);
    }
    if let (Some(kind), Some((row, col))) = (m.shape, cursor) {
        let bx = crate::fx_profile::BoxRule { kind, ..rules.boxes.clone() };
        if has_box(screen, &bx, row, col) {
            return (1, 1);
        }
    }
    (0, 0)
}

/// How well `rules` fits the region: (strength, hits). Strength is 0 none, 1 shape, 2 prompt,
/// 3 text, 4 program; hits count the matches of that kind. `job` None skips `program`.
pub fn score(rules: &FxRules, screen: &Screen, region: Region, job: Option<&str>) -> (u8, u32) {
    let cursor = effects_cursor(screen, rules).map(|(r, c, _)| (r, c)).filter(|&(r, c)| r >= region.0 && r <= region.1 && c >= region.2 && c <= region.3);
    score_with(rules, screen, &mut Haystack::new(screen, region), cursor, region.2, job)
}

/// What each region showed that was worth remembering: a banner scrolls away, its profile stays.
#[derive(Default)]
pub struct Memory {
    regions: Vec<(Region, String)>,
    clears_seen: u64,
}

fn overlaps(a: Region, b: Region) -> bool {
    a.0 <= b.1 && b.0 <= a.1 && a.2 <= b.3 && b.2 <= a.3
}

impl Memory {
    /// The region around the effects' cursor and the profile that reads it.
    pub fn choose(&mut self, store: &Store, screen: &Screen, job: Option<&str>) -> (Region, Arc<FxRules>) {
        let find = store.find_rules();
        let cursor = effects_cursor(screen, &find).map(|(r, c, _)| (r, c));
        let (lines, cols) = (screen.term().screen_lines(), screen.term().columns());
        let region = output_watch(screen, &find).unwrap_or((0, lines.saturating_sub(1), 0, cols.saturating_sub(1)));
        let mut text = Haystack::new(screen, region);
        // Shipped profiles first, then the user's in name order; the first of equals wins.
        let mut order: Vec<&crate::fx_store::Loaded> = store.profiles.iter().collect();
        order.sort_by_key(|l| (l.shipped.is_none(), if l.shipped.is_none() { l.rules.name.clone() } else { String::new() }));
        let mut best: Option<(u8, u32, &Arc<FxRules>)> = None;
        for l in order {
            let (s, h) = score_with(&l.rules, screen, &mut text, cursor, region.2, job);
            if s > 0 && best.is_none_or(|(bs, bh, _)| (s, h) > (bs, bh)) {
                best = Some((s, h, &l.rules));
            }
        }
        if let Some((_, _, rules)) = best.filter(|b| b.0 >= STRONG) {
            // A strong match replaces what this region remembered; a moved layout drops the old bounds.
            self.regions.retain(|(r, _)| !overlaps(*r, region));
            self.regions.push((region, rules.name.clone()));
            if self.regions.len() > KEEP {
                self.regions.remove(0);
            }
            return (region, rules.clone());
        }
        // Nothing strong now: the region's remembered profile, else the weak match, else Shell.
        self.regions.retain(|(r, _)| !overlaps(*r, region) || *r == region);
        if let Some(rules) = self.regions.iter().find(|(r, _)| *r == region).and_then(|(_, n)| store.by_name(n)) {
            return (region, rules);
        }
        (region, best.map_or_else(|| store.shell(), |b| b.2.clone()))
    }

    /// Drops every memory once the screen cleared (alternate screen exit, ED 2, ED 3, RIS).
    pub fn forget_cleared(&mut self, screen: &Screen) {
        if screen.clears != self.clears_seen {
            self.clears_seen = screen.clears;
            self.regions.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fx_detect_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn store(tag: &str) -> Store {
        Store::open(&tmp(tag))
    }

    fn agent(name: &str) -> Screen {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agents/");
        let text = std::fs::read_to_string(format!("{dir}{name}.ansi")).unwrap();
        let cursors = std::fs::read_to_string(format!("{dir}cursors.txt")).unwrap();
        let line = cursors.lines().find(|l| l.split(' ').next() == Some(name)).unwrap();
        let num = |key: &str| -> usize { line.split(' ').find_map(|w| w.strip_prefix(key)).unwrap().parse().unwrap() };
        let rows: Vec<String> = text.trim_end_matches('\n').split('\n').map(|r| format!("\x1b[0m{r}")).collect();
        let mut s = Screen::new(100, 30);
        s.feed(format!("\x1b[2J\x1b[H{}\x1b[{};{}H", rows.join("\r\n"), num("y=") + 1, num("x=") + 1).as_bytes());
        s
    }

    fn pick(st: &Store, s: &Screen) -> String {
        Memory::default().choose(st, s, None).1.name.clone()
    }

    #[test]
    fn each_recorded_agent_screen_picks_its_own_profile() {
        let st = store("agents");
        for (file, want) in [("codex", "Codex"), ("opencode", "opencode"), ("hermes", "Hermes"), ("agy", "agy")] {
            assert_eq!(pick(&st, &agent(file)), want, "{file}");
        }
    }

    #[test]
    fn a_claude_code_box_picks_claude_code() {
        let st = store("claude");
        let line = "\u{2500}".repeat(80);
        let mut s = Screen::new(80, 30);
        s.feed(format!("\x1b[?25l\x1b[2J\x1b[9;1H{line}\x1b[10;1H\u{276f} \x1b[7m \x1b[0m\x1b[11;1H{line}").as_bytes());
        assert_eq!(pick(&st, &s), "Claude Code");
    }

    #[test]
    fn a_plain_shell_picks_shell() {
        let st = store("shell");
        let mut s = Screen::new(80, 24);
        s.feed(b"user@host:~$ ls\r\nfile\r\nuser@host:~$ ");
        assert_eq!(pick(&st, &s), "Shell");
    }

    /// A tmux window split left and right: a Claude Code box on the left (its cursor painted)
    /// and a Codex band on the right.
    fn two_panes(real_cursor_on_right: bool) -> Screen {
        let mut o = String::from("\x1b[?25l\x1b[2J");
        for r in 1..=30 {
            o += &format!("\x1b[{r};51H\u{2502}");
        }
        let line = "\u{2500}".repeat(50);
        o += &format!("\x1b[2;1HClaude Code\x1b[19;1H{line}\x1b[20;1H\u{276f} \x1b[7m \x1b[0m\x1b[21;1H{line}");
        o += "\x1b[2;53HOpenAI Codex\x1b[20;53H\x1b[48;2;44;44;44m                                              \x1b[0m";
        if real_cursor_on_right {
            o += "\x1b[?25h\x1b[20;56H";
        }
        let mut s = Screen::new(100, 30);
        s.feed(o.as_bytes());
        s
    }

    #[test]
    fn two_tmux_panes_pick_per_region() {
        let st = store("two");
        let mut m = Memory::default();
        let (left, rules) = m.choose(&st, &two_panes(false), None);
        assert_eq!(rules.name, "Claude Code");
        assert!(left.3 < 50, "the left pane only: {left:?}");
        let (right, rules) = m.choose(&st, &two_panes(true), None);
        assert_eq!(rules.name, "Codex");
        assert!(right.2 > 50, "the right pane only: {right:?}");
        // The recorded layout: the Codex pane's own region picks Codex.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tmux-agents/");
        let mut s = Screen::new(201, 61);
        s.feed(&std::fs::read(format!("{dir}mx-codex.raw")).unwrap());
        assert_eq!(pick(&st, &s), "Codex");
    }

    #[test]
    fn a_banner_that_scrolled_away_keeps_its_profile() {
        let st = store("scroll");
        let mut s = agent("codex");
        let mut m = Memory::default();
        assert_eq!(m.choose(&st, &s, None).1.name, "Codex");
        s.feed("\r\nline".repeat(40).as_bytes());
        assert!(!Haystack::new(&s, (0, 29, 0, 99)).has("OpenAI Codex"), "the banner is gone");
        assert_eq!(m.choose(&st, &s, None).1.name, "Codex");
    }

    #[test]
    fn a_clear_forgets() {
        let st = store("clear");
        let mut s = agent("codex");
        let mut m = Memory::default();
        m.forget_cleared(&s);
        assert_eq!(m.choose(&st, &s, None).1.name, "Codex");
        s.feed(b"\x1b[2J\x1b[H$ ");
        // Memory holds until the clear is noticed, then the plain screen is Shell.
        m.forget_cleared(&s);
        assert_eq!(m.choose(&st, &s, None).1.name, "Shell");
    }

    #[test]
    fn clears_count_each_way() {
        // A program's ED 3 is gated by the history rules; when it passes it counts too.
        for seq in [&b"\x1b[2J"[..], b"\x1bc", b"\x1b[?1049h\x1b[?1049l"] {
            let mut s = Screen::new(20, 5);
            s.feed(b"hello");
            let before = s.clears;
            s.feed(seq);
            assert!(s.clears > before, "{seq:?}");
        }
        let mut s = Screen::new(20, 5);
        s.feed(b"hello\r\nworld\x1b[K");
        assert_eq!(s.clears, 0);
    }

    #[test]
    fn a_common_text_match_loses_to_a_program_match_and_ties_go_shipped() {
        let dir = tmp("tie");
        drop(Store::open(&dir));
        std::fs::write(dir.join("zzz.toml"), "name = \"Zzz\"\n[match]\ntext = [\"$\"]\n").unwrap();
        let st = Store::open(&dir);
        let mut s = Screen::new(80, 10);
        s.feed(b"user@host:~$ ");
        assert_eq!(Memory::default().choose(&st, &s, Some("codex")).1.name, "Codex");
        assert_eq!(Memory::default().choose(&st, &s, Some("bash")).1.name, "Zzz");
        // A tie at text strength: the shipped profile first.
        s.feed(b"\r\nOpenAI Codex $ ");
        assert_eq!(Memory::default().choose(&st, &s, None).1.name, "Codex");
    }

    #[test]
    fn detection_with_64_profiles_fits_the_budget() {
        let dir = tmp("budget");
        drop(Store::open(&dir));
        for i in 0..64 {
            let texts: Vec<String> = (0..16).map(|j| format!("\"needle number {i} {j} that is not on screen\"")).collect();
            std::fs::write(dir.join(format!("p{i:02}.toml")), format!("name = \"P{i:02}\"\n[match]\ntext = [{}]\n", texts.join(", "))).unwrap();
        }
        let st = Store::open(&dir);
        assert!(st.profiles.len() >= 64);
        let mut s = Screen::new(500, 200);
        let row = "the quick brown fox jumps over the lazy dog ".repeat(11);
        s.feed(format!("{}\x1b[1;1H", format!("{row}\r\n").repeat(199)).as_bytes());
        let mut m = Memory::default();
        let t = Instant::now();
        m.choose(&st, &s, None);
        let limit = if cfg!(debug_assertions) { 20 } else { 2 };
        assert!(t.elapsed().as_millis() < limit, "{:?}", t.elapsed());
    }
}
