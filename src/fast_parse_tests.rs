//! The printable ASCII fast paths (Task 8d) against the char by char parse: the same screen,
//! cursor, modes, counters and damage after every chunk of a random stream.

use crate::screen::{Look, Screen};
use crate::trigger_eval::TriggerPrefs;
use crate::triggers::Trigger;
use alacritty_terminal::term::TermDamage;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: &[&str] = &["the", "quick", "terminal", "parses", "every", "byte", "of", "output", "flood", "line", "and", "draws", "it"];

fn prose(rng: &mut Rng, want: usize) -> String {
    let mut s = String::new();
    while s.len() < want {
        s.push_str(WORDS[rng.below(WORDS.len())]);
        s.push(' ');
    }
    s.truncate(want);
    s
}

const PIECES: &[&[u8]] = &[
    b"\t", b"\r", b"\x08", b"\x7f", b"\r\n", "\u{4e00}".as_bytes(), "\u{1f600}".as_bytes(), "e\u{301}".as_bytes(),
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}".as_bytes(), "\u{1f1fa}\u{1f1f8}".as_bytes(), "\u{2764}\u{fe0f}".as_bytes(),
    b"\xff", b"\xc3", b"\xe4\xb8", b"\xa9", b"\x1b[4h", b"\x1b[4l", b"\x1b(0lqqk\x1b(B", b"\x1b)0\x0eqq\x0f", b"\x1b[?6h", b"\x1b[?6l",
    b"\x1b[5;10r", b"\x1b[r", b"\x1b[?7l", b"\x1b[?7h", b"\x1b[?69h\x1b[10;40s", b"\x1b[?69l", b"\x1b[3;70H", b"\x1b[5b",
    b"\x1b[?1049h", b"\x1b[?1049l", "\u{4e00}\u{4e00}\x1b[1;2Habc".as_bytes(), b"\x1b]0;t\x07", b"\x1b]133;A\x07",
    b"\x1b[?25l", b"\x1b[?25h", b"\x1b[1;31m", b"\x1b[m", b"\x1b[3;1H", b"\x1b[24;19H",
];

/// One piece of output: the bench's flood line, a plain line longer than the width, or a
/// fixed piece.
fn piece(rng: &mut Rng, i: usize) -> Vec<u8> {
    match rng.below(10) {
        0..=2 => {
            let want = 40 + rng.below(80);
            format!("{i:07} \x1b[3{}mflood\x1b[0m {}\r\n", 1 + i % 6, prose(rng, want)).into_bytes()
        }
        3 => {
            let want = 1 + rng.below(200);
            prose(rng, want).into_bytes()
        }
        _ => PIECES[rng.below(PIECES.len())].to_vec(),
    }
}

fn trig(regex: &str) -> Trigger {
    Trigger { regex: regex.into(), action: "HighlightTrigger".into(), parameter: "{#ff0000,#000000}".into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }
}

fn screen(cols: usize, rows: usize, ambiguous_wide: bool, triggers: bool, fast: bool) -> Screen {
    // A short history keeps the grid compare quick; rows still scroll into it and out.
    let look = Look { ambiguous_wide, scrollback: 10, ..Look::default() };
    let mut s = Screen::with_look(cols, rows, Default::default(), look);
    s.set_clock(Some(1000));
    s.set_fast_paths(fast);
    if triggers {
        s.set_triggers(vec![trig("flood|quick")], TriggerPrefs::default());
        s.triggers.unbudgeted_for_test();
    }
    s
}

fn damage(s: &mut Screen) -> String {
    let term = s.term_mut();
    let d = match term.damage() {
        TermDamage::Full => "full".to_string(),
        TermDamage::Partial(lines) => format!("{:?}", lines.collect::<Vec<_>>()),
    };
    term.reset_damage();
    d
}

/// Every row's cells and marks, history first: what the grid's Debug shows apart from the
/// rows' occupancy, quick enough to check after every chunk.
fn rows_equal(a: &Screen, b: &Screen) -> bool {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let (ga, gb) = (a.term().grid(), b.term().grid());
    if (ga.history_size(), ga.screen_lines(), ga.columns(), ga.display_offset()) != (gb.history_size(), gb.screen_lines(), gb.columns(), gb.display_offset()) {
        return false;
    }
    (-(ga.history_size() as i32)..ga.screen_lines() as i32).all(|i| {
        let (ra, rb) = (&ga[Line(i)], &gb[Line(i)]);
        let meta = |r: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>| (r.mark(), r.manual(), r.time(), r.block(), r.button(), r.porthole(), r.alt(), r.fold(), r.note());
        ra[Column(0)..] == rb[Column(0)..] && meta(ra) == meta(rb)
    })
}

/// Everything the two parses must agree on after a chunk; the whole grid's Debug only when
/// `full` (the rows are compared by `rows_equal` every time).
fn state(s: &mut Screen, reply: &[u8], full: bool) -> String {
    let d = damage(s);
    let requests = s.take_requests();
    let t = s.term();
    let c = &t.grid().cursor;
    format!(
        "reply {reply:?}\ncursor {:?} wrap {}\nmode {:?}\nprinted {} echo {:?} {:?} armed {} watch {}\ndamage {d}\ntitle {:?}\nmarks {:?}\nrequests {requests:?}\ngrid {:?}",
        c.point,
        c.input_needs_wrap,
        t.mode(),
        t.printed,
        t.echo_point,
        s.echo_at,
        t.echo_armed,
        t.watch_hit,
        s.title(),
        s.marks.records,
        if full { format!("{:?}", t.grid()) } else { String::new() },
    )
}

#[test]
fn fast_print_matches_char_by_char() {
    let configs = [(80, 24, false, false), (20, 5, false, false), (80, 24, true, false), (20, 5, true, false), (80, 24, false, true), (20, 5, true, true)];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for round in 0..3000 {
        let (cols, rows, wide, triggers) = configs[round % configs.len()];
        let (mut fast, mut slow) = (screen(cols, rows, wide, triggers, true), screen(cols, rows, wide, triggers, false));
        let mut stream = Vec::new();
        for i in 0..1 + rng.below(24) {
            stream.extend(piece(&mut rng, i));
        }
        // Small cuts split characters and sequences; large ones keep whole lines together.
        let most = [8, 64, 4096][rng.below(3)];
        let mut at = 0;
        let mut chunks = 0;
        while at < stream.len() {
            let take = (1 + rng.below(most)).min(stream.len() - at);
            let chunk = &stream[at..at + take];
            match rng.below(8) {
                0 => {
                    fast.note_typed();
                    slow.note_typed();
                }
                1 => {
                    let (top, left) = (rng.below(rows) as i32, rng.below(cols));
                    let watch = Some((top, top + rng.below(3) as i32, left, left + rng.below(cols)));
                    fast.term_mut().print_watch = watch;
                    slow.term_mut().print_watch = watch;
                }
                2 => {
                    fast.term_mut().print_watch = None;
                    slow.term_mut().print_watch = None;
                    fast.term_mut().watch_hit = false;
                    slow.term_mut().watch_hit = false;
                }
                _ => {}
            }
            let (a, b) = (fast.feed(chunk), slow.feed(chunk));
            assert!(rows_equal(&fast, &slow), "round {round} ({cols}x{rows} wide {wide} triggers {triggers}) at {at}+{take}: the rows differ in {:?}", String::from_utf8_lossy(&stream));
            chunks += 1;
            let full = chunks % 16 == 0 || at + take == stream.len();
            let (a, b) = (state(&mut fast, &a, full), state(&mut slow, &b, full));
            if a != b {
                let line = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
                panic!(
                    "round {round} ({cols}x{rows} wide {wide} triggers {triggers}) at {at}+{take} of {:?}\nfirst difference on line {line}:\nfast {}\nslow {}",
                    String::from_utf8_lossy(&stream),
                    a.lines().nth(line).unwrap_or_default(),
                    b.lines().nth(line).unwrap_or_default(),
                );
            }
            at += take;
        }
    }
}

/// Parse rate on the bench's flood block, fast paths on and off. A measurement, not a check:
/// `cargo test --release --lib -- --ignored flood_parse_rate --nocapture`.
#[test]
#[ignore]
fn flood_parse_rate() {
    let mut rng = Rng(7);
    let mut block = Vec::new();
    let mut i = 0;
    while block.len() < 1_000_000 {
        i += 1;
        let want = 40 + rng.below(80);
        block.extend(format!("{i:07} \x1b[3{}mflood\x1b[0m {}\r\n", 1 + i % 6, prose(&mut rng, want)).into_bytes());
    }
    for fast in [false, true, false, true] {
        let mut s = screen(120, 40, false, false, fast);
        let started = std::time::Instant::now();
        for _ in 0..20 {
            for chunk in block.chunks(64 << 10) {
                s.feed(chunk);
            }
        }
        let secs = started.elapsed().as_secs_f64();
        println!("fast {fast}: {:.1} MB/s", 20.0 * block.len() as f64 / secs / 1e6);
    }
}
