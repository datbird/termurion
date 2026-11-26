//! What the Metal tests compare against. wgpu is not in the macOS build, so a test's reference is
//! what native Metal drew when it was recorded, at a time it matched wgpu's Metal backend byte for
//! byte (`TERMURION_RECORD_METAL=1`).
//!
//! One index per test in `tests/fixtures/metal/<test>.txt`: every picture and text the test checks,
//! in order, as its name, its size (or `text`) and the FNV-1a hash of its bytes. Only the hashes
//! are kept, so the repository holds no pictures; a frame that differs is written as a PNG under
//! `target/metal-mismatch/` for a person to look at.

use std::path::PathBuf;

/// Recording replaces the stored indexes with what Metal draws now.
pub const RECORD_ENV: &str = "TERMURION_RECORD_METAL";

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Pixels { name: String, w: u32, h: u32, rgba: Vec<u8> },
    Text { name: String, text: String },
}

impl Entry {
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

/// Where a differing frame goes.
pub fn mismatch_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/metal-mismatch")
}

pub fn recording() -> bool {
    std::env::var(RECORD_ENV).is_ok_and(|v| v == "1")
}

/// One test's index: recorded, or played back entry by entry against what the test draws.
pub struct Index {
    test: String,
    record: bool,
    lines: Vec<String>,
    next: usize,
    finished: bool,
}

impl Index {
    pub fn open(test: &str) -> Index {
        let record = recording();
        let lines = if record {
            Vec::new()
        } else {
            let path = dir().join(format!("{test}.txt"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}; record it with {RECORD_ENV}=1", path.display()));
            text.lines().map(String::from).collect()
        };
        Index::with(test, record, lines)
    }

    fn with(test: &str, record: bool, lines: Vec<String>) -> Index {
        Index { test: test.to_string(), record, lines, next: 0, finished: false }
    }

    /// The next entry: kept when recording, else its hash checked against the stored one. A
    /// picture that differs is written as a PNG, and the failure names it.
    pub fn take(&mut self, got: Entry) {
        let n = self.next;
        self.next += 1;
        let line = got.index_line(n);
        if self.record {
            self.lines.push(line);
            return;
        }
        let test = &self.test;
        let Some(want) = self.lines.get(n) else {
            panic!("{test}: entry {n} ({}) is not in the index; record again with {RECORD_ENV}=1", got.name());
        };
        if *want == line {
            return;
        }
        let shown = match &got {
            Entry::Pixels { w, h, rgba, .. } => match save_png(test, n, *w, *h, rgba) {
                Ok(p) => format!("the frame drawn now is {}", p.display()),
                Err(e) => format!("the frame could not be written: {e}"),
            },
            Entry::Text { text, .. } => format!("the text now: {text}"),
        };
        panic!("{test}: entry {n} differs from the stored one\n  stored: {want}\n  now:    {line}\n  {shown}");
    }

    /// Writes the recorded index, or checks every stored entry was played.
    pub fn finish(mut self) {
        self.finished = true;
        if self.record {
            std::fs::create_dir_all(dir()).unwrap();
            let text: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(dir().join(format!("{}.txt", self.test)), text).unwrap();
        } else {
            assert_eq!(self.next, self.lines.len(), "{}: the test checked {} of {} stored entries", self.test, self.next, self.lines.len());
        }
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        // A test that panics leaves its index alone; one that never calls `finish` is a bug.
        if !self.finished && !std::thread::panicking() {
            panic!("{}: the index was never finished", self.test);
        }
    }
}

/// `rgba` as `target/metal-mismatch/<test>-<entry>.png`.
fn save_png(test: &str, n: usize, w: u32, h: u32, rgba: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(mismatch_dir()).map_err(|e| e.to_string())?;
    let path = mismatch_dir().join(format!("{test}-{n:05}.png"));
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).ok_or("the picture is shorter than its size")?;
    img.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pic(name: &str, seed: u8) -> Entry {
        Entry::Pixels { name: name.into(), w: 3, h: 2, rgba: (0..24).map(|i| (i as u8).wrapping_mul(seed)).collect() }
    }

    /// What a recording writes plays back, and a frame that differs fails with its PNG named.
    #[test]
    fn a_differing_frame_is_written_as_a_png() {
        let test = "reference_self_test";
        let mut rec = Index::with(test, true, Vec::new());
        rec.take(pic("a", 1));
        rec.take(Entry::Text { name: "state".into(), text: "(true, 3)".into() });
        rec.take(pic("b", 7));
        let lines = std::mem::take(&mut rec.lines);
        rec.finished = true;
        let mut play = Index::with(test, false, lines.clone());
        play.take(pic("a", 1));
        play.take(Entry::Text { name: "state".into(), text: "(true, 3)".into() });
        play.take(pic("b", 7));
        play.finish();
        // The second picture one byte off.
        let png = mismatch_dir().join(format!("{test}-00002.png"));
        let _ = std::fs::remove_file(&png);
        let failed = std::panic::catch_unwind(|| {
            let mut play = Index::with(test, false, lines.clone());
            play.take(pic("a", 1));
            play.take(Entry::Text { name: "state".into(), text: "(true, 3)".into() });
            let Entry::Pixels { mut rgba, .. } = pic("b", 7) else { unreachable!() };
            rgba[5] ^= 1;
            play.take(Entry::Pixels { name: "b".into(), w: 3, h: 2, rgba });
        });
        let why = *failed.expect_err("a differing frame passed").downcast::<String>().unwrap();
        assert!(why.contains("entry 2 differs") && why.contains(&png.display().to_string()), "{why}");
        let back = image::open(&png).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (3, 2));
        let _ = std::fs::remove_file(&png);
    }
}
