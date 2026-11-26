//! Stored pictures the Metal tests compare against. wgpu is not in the macOS build, so a test's
//! reference is what native Metal drew when it was recorded, at a time it matched wgpu's Metal
//! backend byte for byte (`TERMURION_RECORD_METAL=1`).
//!
//! One tape per test in `tests/fixtures/metal`: `<test>.tape` holds every picture and text the test
//! checks, in order, and `<test>.txt` is its index (name, size and FNV-1a hash of each entry).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;

/// Recording replaces the stored tapes with what Metal draws now.
pub const RECORD_ENV: &str = "TERMURION_RECORD_METAL";

const MAGIC: &[u8] = b"termurion metal tape 1\n";

// How an entry is stored: pixels whole, pixels as the change from the last picture of the same
// size moved up or down by whole rows (a scroll), a copy of an earlier picture, or a text.
// "The same size" is one of the last `RECENT` pictures of that size: an effect timeline that
// switches between two screens finds the other screen two back.
const RAW: u8 = 0;
const XOR: u8 = 1;
const SAME: u8 = 2;
const TEXT: u8 = 3;
const RECENT: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Pixels { name: String, w: u32, h: u32, rgba: Vec<u8> },
    Text { name: String, text: String },
}

impl Entry {
    #[cfg(test)]
    fn as_pixels(&self) -> Vec<u8> {
        match self {
            Entry::Pixels { rgba, .. } => rgba.clone(),
            Entry::Text { .. } => Vec::new(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Entry::Pixels { name, .. } | Entry::Text { name, .. } => name,
        }
    }

    fn index_line(&self, i: usize) -> String {
        match self {
            Entry::Pixels { name, w, h, rgba } => format!("{i}\t{name}\t{w}x{h}\t{:016x}", fnv(rgba)),
            Entry::Text { name, text } => format!("{i}\t{name}\ttext\t{:016x}", fnv(text.as_bytes())),
        }
    }
}

/// FNV-1a, 64 bits.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/metal")
}

pub fn recording() -> bool {
    std::env::var(RECORD_ENV).is_ok_and(|v| v == "1")
}

/// One test's tape: recorded, or played back entry by entry against what the test draws.
pub struct Tape {
    test: String,
    mode: Mode,
    finished: bool,
}

enum Mode {
    Record(Writer),
    Play { player: Player, index: Vec<String> },
}

impl Tape {
    pub fn open(test: &str) -> Tape {
        let mode = if recording() {
            Mode::Record(Writer::default())
        } else {
            let path = dir().join(format!("{test}.tape"));
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}; record it with {RECORD_ENV}=1", path.display()));
            let index = std::fs::read_to_string(dir().join(format!("{test}.txt"))).unwrap_or_default().lines().map(String::from).collect();
            Mode::Play { player: Player::new(bytes), index }
        };
        Tape { test: test.to_string(), mode, finished: false }
    }

    /// The next entry: kept when recording, else checked against the stored one. Returns the
    /// stored pixels for the caller's own comparison (None when recording or for a text).
    pub fn take(&mut self, got: Entry) -> Option<Vec<u8>> {
        let test = &self.test;
        let (player, index) = match &mut self.mode {
            Mode::Record(w) => {
                w.push(got);
                return None;
            }
            Mode::Play { player, index } => (player, index),
        };
        let n = player.n;
        let Some(want) = player.next() else {
            panic!("{test}: entry {n} ({}) is not on the tape; record again with {RECORD_ENV}=1", got.name());
        };
        // The index is the tape's own check: a damaged tape is not a drawing difference.
        assert_eq!(index.get(n).map(String::as_str), Some(want.index_line(n).as_str()), "{test}: the tape and its index disagree at entry {n}");
        assert_eq!(want.name(), got.name(), "{test}: entry {n} is another case; record again with {RECORD_ENV}=1");
        match (want, got) {
            (Entry::Pixels { w, h, rgba, .. }, Entry::Pixels { w: gw, h: gh, name, .. }) => {
                assert_eq!((w, h), (gw, gh), "{test}: {name}: the picture size differs from the stored one");
                Some(rgba)
            }
            (Entry::Text { text, .. }, Entry::Text { text: got, name }) => {
                assert_eq!(text, got, "{test}: {name}: differs from the stored text");
                None
            }
            _ => panic!("{test}: entry {n}: a picture where a text is stored, or the other way"),
        }
    }

    /// Writes the recorded tape, or checks every stored entry was played.
    pub fn finish(mut self) {
        self.finished = true;
        match &mut self.mode {
            Mode::Record(w) => {
                std::fs::create_dir_all(dir()).unwrap();
                std::fs::write(dir().join(format!("{}.tape", self.test)), &w.out).unwrap();
                std::fs::write(dir().join(format!("{}.txt", self.test)), &w.index).unwrap();
            }
            Mode::Play { player, index } => {
                assert!(player.next().is_none(), "{}: the test checked {} of {} stored entries", self.test, player.n - 1, index.len());
            }
        }
    }
}

impl Drop for Tape {
    fn drop(&mut self) {
        // A test that panics leaves its tape alone; one that never calls `finish` is a bug.
        if !self.finished && !std::thread::panicking() {
            panic!("{}: the tape was never finished", self.test);
        }
    }
}

fn deflate(bytes: &[u8], level: u32) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn inflate(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out).unwrap();
    out
}

/// `a` with each of its rows XORed with row `y + shift` of `b` (rows `b` lacks stay as they are).
fn xor_shifted(a: &[u8], b: &[u8], w: u32, shift: i32) -> Vec<u8> {
    let row = w as usize * 4;
    let rows = (a.len() / row) as i64;
    let mut out = a.to_vec();
    for y in 0..rows {
        let from = y + shift as i64;
        if (0..rows).contains(&from) {
            let (o, f) = (y as usize * row, from as usize * row);
            for (x, z) in out[o..o + row].iter_mut().zip(&b[f..f + row]) {
                *x ^= z;
            }
        }
    }
    out
}

/// The row shift that lines up the most rows of `new` with `old`: a scrolled screen moves by a
/// line's height. Zero when nothing lines up better.
fn best_shift(new: &[u8], old: &[u8], w: u32) -> i32 {
    let row = w as usize * 4;
    let mut at: HashMap<&[u8], Vec<i64>> = HashMap::new();
    for (y, r) in old.chunks(row).enumerate() {
        at.entry(r).or_default().push(y as i64);
    }
    let mut votes: HashMap<i64, usize> = HashMap::new();
    for (y, r) in new.chunks(row).enumerate() {
        // A row seen everywhere (a blank one) says nothing about the shift.
        if let Some(list) = at.get(r).filter(|l| l.len() < 8) {
            for from in list {
                *votes.entry(from - y as i64).or_default() += 1;
            }
        }
    }
    votes.into_iter().max_by_key(|&(k, n)| (n, std::cmp::Reverse(k.abs()))).map_or(0, |(k, _)| k as i32)
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// Writes a tape one entry at a time. A picture is stored as the smallest of itself and its
/// changes from the recent pictures of its size (an effect timeline changes only its region), or
/// as a copy of an equal earlier one; only compressed copies are kept, so a long test stays small in memory.
pub struct Writer {
    pub out: Vec<u8>,
    pub index: String,
    n: usize,
    last: HashMap<(u32, u32), VecDeque<Vec<u8>>>,
    // Earlier pictures by hash and size, compressed: equal compressed bytes are equal pictures.
    seen: HashMap<(u64, u32, u32), Vec<(usize, Vec<u8>)>>,
}

impl Default for Writer {
    fn default() -> Writer {
        Writer { out: MAGIC.to_vec(), index: String::new(), n: 0, last: HashMap::new(), seen: HashMap::new() }
    }
}

impl Writer {
    pub fn push(&mut self, e: Entry) {
        self.index += &(e.index_line(self.n) + "\n");
        match e {
            Entry::Text { name, text } => {
                self.out.push(TEXT);
                put_str(&mut self.out, &name);
                put_str(&mut self.out, &text);
            }
            Entry::Pixels { name, w, h, rgba } => {
                let raw = deflate(&rgba, 6);
                let key = (fnv(&rgba), w, h);
                if let Some((k, _)) = self.seen.get(&key).and_then(|list| list.iter().find(|(_, z)| *z == raw)) {
                    self.out.push(SAME);
                    put_str(&mut self.out, &name);
                    put_u32(&mut self.out, *k as u32);
                } else {
                    let mut best = (RAW, 0, 0, raw.clone());
                    for (back, l) in self.last.get(&(w, h)).into_iter().flatten().enumerate() {
                        let moved = best_shift(&rgba, l, w);
                        for shift in if moved == 0 { vec![0] } else { vec![0, moved] } {
                            let x = deflate(&xor_shifted(&rgba, l, w, shift), 6);
                            if x.len() < best.3.len() {
                                best = (XOR, back, shift, x);
                            }
                        }
                    }
                    let (kind, back, shift, data) = best;
                    self.out.push(kind);
                    put_str(&mut self.out, &name);
                    put_u32(&mut self.out, w);
                    put_u32(&mut self.out, h);
                    if kind == XOR {
                        put_u32(&mut self.out, back as u32);
                        put_u32(&mut self.out, shift as u32);
                    }
                    put_u32(&mut self.out, data.len() as u32);
                    self.out.extend_from_slice(&data);
                    self.seen.entry(key).or_default().push((self.n, raw));
                }
                remember(&mut self.last, w, h, rgba);
            }
        }
        self.n += 1;
    }
}

/// Keeps `rgba` as the newest of the last `RECENT` pictures of its size.
fn remember(last: &mut HashMap<(u32, u32), VecDeque<Vec<u8>>>, w: u32, h: u32, rgba: Vec<u8>) {
    let list = last.entry((w, h)).or_default();
    list.push_front(rgba);
    list.truncate(RECENT);
}

/// Reads a tape back in order, keeping only what later entries refer to.
pub struct Player {
    bytes: Vec<u8>,
    at: usize,
    pub n: usize,
    last: HashMap<(u32, u32), VecDeque<Vec<u8>>>,
    /// The pictures a later copy names, compressed.
    targets: HashSet<usize>,
    kept: HashMap<usize, (u32, u32, Vec<u8>)>,
}

impl Player {
    pub fn new(bytes: Vec<u8>) -> Player {
        assert!(bytes.starts_with(MAGIC), "not a Metal tape");
        let mut p = Player { bytes, at: MAGIC.len(), n: 0, last: HashMap::new(), targets: HashSet::new(), kept: HashMap::new() };
        // One pass over the headers: which pictures are copied later.
        while p.at < p.bytes.len() {
            let kind = p.take(1)[0];
            let _ = p.string();
            match kind {
                TEXT => {
                    let _ = p.string();
                }
                SAME => {
                    let k = p.u32() as usize;
                    p.targets.insert(k);
                }
                RAW | XOR => {
                    let _ = (p.u32(), p.u32());
                    if kind == XOR {
                        let _ = (p.u32(), p.u32());
                    }
                    let n = p.u32() as usize;
                    p.at += n;
                }
                k => panic!("unknown tape entry kind {k}"),
            }
        }
        p.at = MAGIC.len();
        p
    }

    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.bytes[self.at..self.at + n];
        self.at += n;
        s
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }

    fn string(&mut self) -> String {
        let n = self.u32() as usize;
        String::from_utf8(self.take(n).to_vec()).unwrap()
    }

    pub fn next(&mut self) -> Option<Entry> {
        if self.at >= self.bytes.len() {
            self.n += 1;
            return None;
        }
        let kind = self.take(1)[0];
        let name = self.string();
        let e = match kind {
            TEXT => Entry::Text { name, text: self.string() },
            _ => {
                let (w, h, rgba) = if kind == SAME {
                    let k = self.u32() as usize;
                    let (w, h, z) = self.kept.get(&k).expect("a copy of a picture not on the tape");
                    (*w, *h, inflate(z, (w * h * 4) as usize))
                } else {
                    let (w, h) = (self.u32(), self.u32());
                    let (back, shift) = if kind == XOR { (self.u32() as usize, self.u32() as i32) } else { (0, 0) };
                    let n = self.u32() as usize;
                    let at = self.at;
                    self.at += n;
                    let mut rgba = inflate(&self.bytes[at..at + n], (w * h * 4) as usize);
                    if kind == XOR {
                        let base = self.last.get(&(w, h)).and_then(|l| l.get(back)).expect("a change with no picture before it");
                        rgba = xor_shifted(&rgba, base, w, shift);
                    }
                    assert_eq!(rgba.len(), (w * h * 4) as usize, "{name}: a short picture");
                    if self.targets.contains(&self.n) {
                        self.kept.insert(self.n, (w, h, deflate(&rgba, 1)));
                    }
                    (w, h, rgba)
                };
                remember(&mut self.last, w, h, rgba.clone());
                Entry::Pixels { name, w, h, rgba }
            }
        };
        self.n += 1;
        Some(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tape_reads_back_what_was_written() {
        let pic = |name: &str, seed: u8| Entry::Pixels { name: name.into(), w: 3, h: 2, rgba: (0..24).map(|i| (i as u8).wrapping_mul(seed)).collect() };
        let entries = vec![
            pic("a", 1),
            Entry::Text { name: "state".into(), text: "(true, 3)".into() },
            pic("b", 7),
            pic("a again", 1),
            Entry::Pixels { name: "other size".into(), w: 1, h: 1, rgba: vec![1, 2, 3, 4] },
            pic("c", 9),
            pic("c again", 9),
            Entry::Pixels { name: "tall".into(), w: 1, h: 12, rgba: (0..48).map(|i| (i * 7) as u8).collect() },
            // The tall one scrolled up two rows, a new bottom.
            Entry::Pixels { name: "scrolled".into(), w: 1, h: 12, rgba: (8..56).map(|i| if i < 48 { (i * 7) as u8 } else { 1 }).collect() },
        ];
        let mut w = Writer::default();
        for e in &entries {
            w.push(e.clone());
        }
        let mut p = Player::new(w.out.clone());
        assert_eq!(p.targets.len(), 2, "the repeats are stored as copies");
        let a: Vec<u8> = (0..48).map(|i| (i * 7) as u8).collect();
        assert_eq!(best_shift(&entries[8].as_pixels(), &a, 1), 2, "the scroll is found");
        let back: Vec<Entry> = std::iter::from_fn(|| p.next()).collect();
        assert_eq!(back, entries);
        let lines: Vec<String> = entries.iter().enumerate().map(|(i, e)| e.index_line(i)).collect();
        assert_eq!(w.index.lines().collect::<Vec<_>>(), lines);
    }
}
