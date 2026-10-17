//! Instant Replay's recorder (iTerm2's DVR): a rolling, memory-capped video of a
//! session's screen. The byte ring emulates `DVRBuffer` exactly (positions and lengths
//! only, so frames are evicted at the same moments iTerm2 evicts them), while the frames
//! themselves hold alacritty rows: key frames every row, diff frames only the dirty ones.
//! Byte accounting uses Termurion's cell: a line is `size_of::<Cell>() * (cols + 1)` bytes.

use std::collections::VecDeque;

use alacritty_terminal::grid::Row;
use alacritty_terminal::term::cell::Cell;

/// Every 100th diff-eligible frame is a key frame (`kKeyFrameFrequency`).
pub const KEY_FRAME_EVERY: u32 = 100;
/// A frame may be a diff only when more than this share of its lines is clean.
pub const CLEAN_SHARE: f64 = 0.8;

/// Bytes one line takes: the cells plus iTerm2's continuation cell.
pub fn line_bytes(cols: usize) -> u64 {
    std::mem::size_of::<Cell>() as u64 * (cols as u64 + 1)
}

/// Bytes a key frame takes: every line, each with its 4-byte metadata length (the
/// metadata payload counts 0; Termurion keeps it inside the row).
pub fn key_bytes(cols: usize, rows: usize) -> u64 {
    rows as u64 * (line_bytes(cols) + 4)
}

/// Bytes a diff frame takes: a 5-byte entry (type, length) per line, the dirty lines'
/// bytes, and a 5-byte metadata entry per line (`E:268-362`).
pub fn diff_bytes(cols: usize, dirty: usize, rows: usize) -> u64 {
    rows as u64 * 5 + dirty as u64 * line_bytes(cols) + rows as u64 * 5
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Key,
    Diff,
}

/// `DVRFrameInfo`: size, cursor (column, line), commit time in microseconds, kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub cols: usize,
    pub rows: usize,
    pub cursor: (usize, usize),
    pub timestamp: i64,
    pub kind: Kind,
}

/// `DVRBuffer`'s circular store without the bytes. Keys `first..next` are live; block
/// `k` is `blocks[k - first]` as (position, length). The used region is `begin..end`,
/// wrapped when `begin > end`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ring {
    pub capacity: u64,
    pub begin: u64,
    pub end: u64,
    pub first: u64,
    pub next: u64,
    blocks: VecDeque<(u64, u64)>,
}

impl Ring {
    pub fn new(capacity: u64) -> Ring {
        Ring { capacity, ..Ring::default() }
    }

    /// `hasSpaceAvailable:` (`B:196-215`), strict on every side.
    pub fn has_space(&self, len: u64) -> bool {
        if self.begin <= self.end {
            self.capacity.saturating_sub(self.end) > len || self.begin > len
        } else {
            self.begin - self.end > len
        }
    }

    /// `reserve:` (`B:145-163`): frees from the front until `len` fits, then picks the
    /// scratch offset: `end`, or 0 when the tail is too short. Returns (scratch, blocks
    /// freed). iTerm2 asserts when it would free past the last block; this stops there.
    pub fn reserve(&mut self, len: u64) -> (u64, u64) {
        let mut freed = 0;
        while !self.has_space(len) && !self.blocks.is_empty() {
            self.deallocate();
            freed += 1;
        }
        let wrap = self.begin <= self.end && self.capacity.saturating_sub(self.end) < len;
        (if wrap { 0 } else { self.end }, freed)
    }

    /// `allocateBlock:` (`B:165-179`): the block at `scratch`; returns its key.
    pub fn allocate(&mut self, scratch: u64, len: u64) -> u64 {
        self.blocks.push_back((scratch, len));
        self.end = scratch + len;
        self.next += 1;
        self.next - 1
    }

    /// `deallocateBlock` (`B:181-187`): frees the first block; `begin` moves to its end.
    pub fn deallocate(&mut self) {
        if let Some((pos, len)) = self.blocks.pop_front() {
            self.begin = pos + len;
            self.first += 1;
        }
    }

    /// The newest key, None when nothing is stored.
    pub fn last(&self) -> Option<u64> {
        (!self.blocks.is_empty()).then(|| self.next - 1)
    }
}

#[derive(Clone, Debug)]
pub enum Lines {
    /// Every row.
    Key(Vec<Row<Cell>>),
    /// The dirty rows; None for a clean one.
    Diff(Vec<Option<Row<Cell>>>),
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub info: Info,
    pub lines: Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appended {
    ReadOnly,
    TooBig,
    Key,
    Diff,
}

/// `DVR` with its encoder. `frames` runs parallel to the ring's blocks (keys
/// `ring.first..ring.next`).
#[derive(Clone, Debug)]
pub struct Dvr {
    pub ring: Ring,
    /// A loaded recording: appends are ignored and it cannot be cleared.
    pub read_only: bool,
    frames: VecDeque<Frame>,
    /// The rows of the last key frame (`lastFrame_`).
    last: Option<Vec<Row<Cell>>>,
    /// The last committed frame's info (`lastInfo_`).
    last_info: Option<Info>,
    /// Diff-eligible frames seen (`count_`).
    count: u32,
    /// Bytes of diffs since the last key frame (`bytesSinceLastKeyFrame_`).
    since_key: u64,
}

impl Dvr {
    pub fn new(capacity: u64) -> Dvr {
        Dvr { ring: Ring::new(capacity), read_only: false, frames: VecDeque::new(), last: None, last_info: None, count: 0, since_key: 0 }
    }

    /// Drops the frames whose blocks the ring freed.
    fn sync(&mut self) {
        let live = (self.ring.next - self.ring.first) as usize;
        while self.frames.len() > live {
            self.frames.pop_front();
        }
    }

    /// One screen image (`D:76-105`, `E:104-151`): `rows` are the visible lines, `clean`
    /// marks the lines unchanged since the last frame, `now_us` stamps the commit.
    pub fn append(&mut self, rows: &[Row<Cell>], clean: &[bool], cursor: (usize, usize), now_us: i64) -> Appended {
        if self.read_only {
            return Appended::ReadOnly;
        }
        let height = rows.len();
        let width = rows.first().map_or(0, |r| r.len());
        let length = key_bytes(width, height);
        if length > self.ring.capacity / 2 {
            // "Protect the buffer from overflowing if you have a really big window."
            return Appended::TooBig;
        }
        // The full key-frame size is reserved even when a diff follows.
        let (scratch, freed) = self.ring.reserve(length);
        if freed > 0 {
            self.sync();
            // "If the first block is a diff frame it's useless."
            while self.frames.front().is_some_and(|f| f.info.kind == Kind::Diff) {
                self.ring.deallocate();
                self.frames.pop_front();
            }
        }
        let is_clean = |y: usize| clean.get(y).copied().unwrap_or(false);
        let clean_count = (0..height).filter(|y| is_clean(*y)).count();
        // Termurion adds "something is stored": iTerm2 would write a diff with no key frame
        // under it (and assert when decoding it) after evicting every frame.
        let eligible = clean_count as f64 > height as f64 * CLEAN_SHARE
            && self.last.is_some()
            && self.last_info.is_some_and(|i| i.cols == width && i.rows == height)
            && self.since_key < self.ring.capacity / 2
            && !self.frames.is_empty();
        let mut kind = Kind::Diff;
        if !eligible || {
            let c = self.count;
            self.count = self.count.wrapping_add(1);
            c.is_multiple_of(KEY_FRAME_EVERY)
        } {
            kind = Kind::Key;
        }
        let diff_len = diff_bytes(width, height - clean_count, height);
        if kind == Kind::Diff && diff_len > length {
            // "Diff ended up being larger than a key frame would be."
            kind = Kind::Key;
        }
        let (lines, len) = match kind {
            Kind::Key => {
                self.last = Some(rows.to_vec());
                self.since_key = 0;
                (Lines::Key(rows.to_vec()), length)
            }
            Kind::Diff => {
                self.since_key += diff_len;
                let dirty = rows.iter().enumerate().map(|(y, r)| (!is_clean(y)).then(|| r.clone())).collect();
                (Lines::Diff(dirty), diff_len)
            }
        };
        let info = Info { cols: width, rows: height, cursor, timestamp: now_us, kind };
        self.last_info = Some(info);
        self.ring.allocate(scratch, len);
        self.frames.push_back(Frame { info, lines });
        match kind {
            Kind::Key => Appended::Key,
            Kind::Diff => Appended::Diff,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// `firstTimeStamp`, 0 when empty.
    pub fn first_timestamp(&self) -> i64 {
        self.frames.front().map_or(0, |f| f.info.timestamp)
    }

    /// `lastTimeStamp`, 0 when empty.
    pub fn last_timestamp(&self) -> i64 {
        self.frames.back().map_or(0, |f| f.info.timestamp)
    }

    /// `firstTimestampAfter:` (`B:233-250`): the insertion point of `t + 1`; 0 when none.
    pub fn first_timestamp_after(&self, t: i64) -> i64 {
        let target = t.saturating_add(1);
        let i = self.frames.partition_point(|f| f.info.timestamp < target);
        self.frames.get(i).map_or(0, |f| f.info.timestamp)
    }

    pub fn timestamps(&self) -> Vec<i64> {
        self.frames.iter().map(|f| f.info.timestamp).collect()
    }

    /// `canClear` (`D:107-109`): not a loaded recording and no replay decoding it.
    pub fn can_clear(&self, replaying: bool) -> bool {
        !self.read_only && !replaying
    }

    /// `clear` (`D:111-119`): a fresh buffer and encoder of the same capacity. The
    /// caller checks `can_clear` for a running replay; a read-only recorder is kept.
    pub fn clear(&mut self) {
        if !self.can_clear(false) {
            return;
        }
        *self = Dvr::new(self.ring.capacity);
    }

    /// `copyWithFramesFrom:to:` behind `dictionaryValueFrom:to:` (`D:164-174,215-245`):
    /// the whole range is this recorder as is; any other range is a new recorder of the
    /// same capacity holding the frames from `seek(from)` while `timestamp <= to` (`to`
    /// -1 runs to the end), each appended with no clean lines, so every one is a key
    /// frame, stamped by `now_us`.
    pub fn copy_range(&self, from: i64, to: i64, now_us: &mut dyn FnMut() -> i64) -> Dvr {
        if from == self.first_timestamp() && to == self.last_timestamp() {
            return self.clone();
        }
        let mut copy = Dvr::new(self.ring.capacity);
        let mut dec = Decoder::default();
        if dec.seek(self, from) {
            while dec.timestamp() <= to || to == -1 {
                let cursor = dec.info.map_or((0, 0), |i| i.cursor);
                copy.append(&dec.rows, &[], cursor, now_us());
                if !dec.next(self) {
                    break;
                }
            }
        }
        copy
    }

    /// `dictionaryValueFrom:to:` (`D:164-174`, with `DVRBuffer`'s and `DVRIndexEntry`'s
    /// dictionaries): `{ version: 4, capacity, buffer: { firstKey, nextKey, begin, end,
    /// index } }`, the range copied as `copy_range` does. Each index entry keeps iTerm2's
    /// `info`, `position` and `frameLength` and carries its frame's rows in `lines` (key
    /// frames every row, diffs null for a clean one) where iTerm2 keeps a byte store.
    /// Null when a row cannot be written (iTerm2's nil dictionary).
    pub fn to_json(&self, from: i64, to: i64, now_us: &mut dyn FnMut() -> i64) -> serde_json::Value {
        use serde_json::json;
        let copy;
        let d = if from == self.first_timestamp() && to == self.last_timestamp() {
            self
        } else {
            copy = self.copy_range(from, to, now_us);
            &copy
        };
        let mut index = serde_json::Map::new();
        for (i, (f, (pos, len))) in d.frames.iter().zip(d.ring.blocks.iter()).enumerate() {
            let lines = match &f.lines {
                Lines::Key(rows) => serde_json::to_value(rows),
                Lines::Diff(rows) => serde_json::to_value(rows),
            };
            let Ok(lines) = lines else { return serde_json::Value::Null };
            let info = f.info;
            let entry = json!({
                "info": {
                    "width": info.cols,
                    "height": info.rows,
                    "cursorX": info.cursor.0,
                    "cursorY": info.cursor.1,
                    "timestamp": info.timestamp,
                    "frameType": match info.kind { Kind::Key => 0, Kind::Diff => 1 },
                },
                "position": pos,
                "frameLength": len,
                "lines": lines,
            });
            index.insert((d.ring.first + i as u64).to_string(), entry);
        }
        json!({
            "version": 4,
            "capacity": d.ring.capacity,
            "buffer": {
                "firstKey": d.ring.first,
                "nextKey": d.ring.next,
                "begin": d.ring.begin,
                "end": d.ring.end,
                "index": index,
            },
        })
    }

    /// `loadDictionary:` (`D:176-213`, `B:110-143`): a read-only recorder, or None when the
    /// inner version is not 1 to 4, the capacity is 0, the buffer is missing (a missing
    /// index is an empty recording), or (Termurion,
    /// which keeps rows instead of a byte store) an entry is missing or malformed: every
    /// key from `firstKey` up to `nextKey` needs an entry whose rows match its size, and
    /// the first must be a key frame. `begin` and `end` outside the store become 0, as
    /// iTerm2 has it.
    pub fn from_json(v: &serde_json::Value) -> Option<Dvr> {
        let version = v.get("version")?.as_i64()?;
        if !(1..=4).contains(&version) {
            return None;
        }
        let capacity = v.get("capacity")?.as_u64().filter(|c| *c > 0)?;
        let buffer = v.get("buffer")?.as_object()?;
        let key_of = |k: &str| buffer.get(k).and_then(|x| x.as_u64());
        let (first, next) = (key_of("firstKey").unwrap_or(0), key_of("nextKey").unwrap_or(0));
        if next < first {
            return None;
        }
        let inside = |x: Option<u64>| x.filter(|x| *x < capacity).unwrap_or(0);
        // No index loads as an empty buffer (`B:122-130` walks a nil dictionary).
        let empty = serde_json::Map::new();
        let (index, next) = match buffer.get("index") {
            None | Some(serde_json::Value::Null) => (&empty, first),
            Some(v) => (v.as_object()?, next),
        };
        let mut d = Dvr::new(capacity);
        d.ring.first = first;
        d.ring.next = next;
        d.ring.begin = inside(key_of("begin"));
        d.ring.end = inside(key_of("end"));
        for key in first..next {
            let e = index.get(&key.to_string())?;
            let info = e.get("info")?;
            let n = |k: &str| info.get(k).and_then(|x| x.as_u64()).map(|x| x as usize);
            let (cols, rows) = (n("width")?, n("height")?);
            let kind = match info.get("frameType")?.as_u64()? {
                0 => Kind::Key,
                1 => Kind::Diff,
                _ => return None,
            };
            if key == first && kind != Kind::Key {
                return None;
            }
            let info = Info { cols, rows, cursor: (n("cursorX")?, n("cursorY")?), timestamp: info.get("timestamp")?.as_i64()?, kind };
            let lines = e.get("lines")?.clone();
            let lines = match kind {
                Kind::Key => {
                    let l: Vec<Row<Cell>> = serde_json::from_value(lines).ok()?;
                    (l.len() == rows && l.iter().all(|r| r.len() == cols)).then_some(Lines::Key(l))?
                }
                Kind::Diff => {
                    let l: Vec<Option<Row<Cell>>> = serde_json::from_value(lines).ok()?;
                    (l.len() == rows && l.iter().flatten().all(|r| r.len() == cols)).then_some(Lines::Diff(l))?
                }
            };
            let pos = e.get("position").and_then(|x| x.as_u64()).unwrap_or(0);
            let len = e.get("frameLength").and_then(|x| x.as_u64()).unwrap_or(0);
            d.ring.blocks.push_back((pos, len));
            d.frames.push_back(Frame { info, lines });
        }
        d.read_only = true;
        Some(d)
    }

    pub fn frame(&self, key: u64) -> Option<&Frame> {
        if key < self.ring.first {
            return None;
        }
        self.frames.get((key - self.ring.first) as usize)
    }
}

/// `DVRDecoder`: the rebuilt screen of one frame. It holds rows, not references into
/// the recorder, so an evicted frame stays on screen (`invalidateIndex:`).
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    key: Option<u64>,
    pub rows: Vec<Row<Cell>>,
    pub info: Option<Info>,
}

impl Decoder {
    /// `seek:` (`DC:84-96`): the first frame at or after `t`; false when none.
    pub fn seek(&mut self, d: &Dvr, t: i64) -> bool {
        match d.frames.iter().position(|f| f.info.timestamp >= t) {
            Some(i) => {
                self.load(d, d.ring.first + i as u64);
                true
            }
            None => false,
        }
    }

    /// `next` (`DC:114-129`): from an evicted frame (or none yet) to the first frame;
    /// false past the last.
    pub fn next(&mut self, d: &Dvr) -> bool {
        let Some(last) = d.ring.last() else { return false };
        let key = match self.key(d) {
            None => d.ring.first,
            Some(k) if k + 1 > last => return false,
            Some(k) => k + 1,
        };
        self.load(d, key);
        true
    }

    /// `prev` (`DC:131-138`): false at the first frame and from an evicted one.
    pub fn prev(&mut self, d: &Dvr) -> bool {
        match self.key(d) {
            Some(k) if k > d.ring.first => {
                self.load(d, k - 1);
                true
            }
            _ => false,
        }
    }

    pub fn timestamp(&self) -> i64 {
        self.info.map_or(0, |i| i.timestamp)
    }

    /// The frame shown, None once the recorder evicted it.
    pub fn key(&self, d: &Dvr) -> Option<u64> {
        self.key.filter(|k| *k >= d.ring.first && *k < d.ring.next)
    }

    /// `_seekToEntryWithKey:` (`DC:193-233`): the nearest key frame at or before `key`,
    /// then every diff up to it. An out-of-range key becomes the first.
    fn load(&mut self, d: &Dvr, key: u64) {
        let key = if key < d.ring.first || key >= d.ring.next { d.ring.first } else { key };
        let mut j = key;
        while j > d.ring.first && d.frame(j).is_some_and(|f| f.info.kind != Kind::Key) {
            j -= 1;
        }
        for k in j..=key {
            let Some(f) = d.frame(k) else { return };
            match &f.lines {
                Lines::Key(rows) => self.rows = rows.clone(),
                Lines::Diff(rows) => {
                    for (y, r) in rows.iter().enumerate() {
                        if let (Some(r), Some(slot)) = (r, self.rows.get_mut(y)) {
                            *slot = r.clone();
                        }
                    }
                }
            }
            self.info = Some(f.info);
        }
        self.key = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Column;

    fn row(cols: usize, ch: char) -> Row<Cell> {
        let mut r: Row<Cell> = Row::new(cols);
        for x in 0..cols {
            r[Column(x)].c = ch;
        }
        r
    }

    /// A screen with a letter per row.
    fn screen(cols: usize, rows: usize) -> Vec<Row<Cell>> {
        (0..rows).map(|y| row(cols, (b'a' + (y % 26) as u8) as char)).collect()
    }

    /// The first `n` of `rows` lines clean.
    fn clean(rows: usize, n: usize) -> Vec<bool> {
        (0..rows).map(|y| y < n).collect()
    }

    fn kinds(d: &Dvr) -> Vec<Kind> {
        d.frames.iter().map(|f| f.info.kind).collect()
    }

    #[test]
    fn ring_follows_iterm2() {
        let mut r = Ring::new(100);
        assert_eq!(r.last(), None);
        assert!(r.has_space(99));
        assert!(!r.has_space(100), "the space test is strict");
        assert_eq!(r.reserve(60), (0, 0));
        assert_eq!(r.allocate(0, 60), 0);
        assert_eq!(r.reserve(30), (60, 0));
        assert_eq!(r.allocate(60, 30), 1);
        assert_eq!((r.begin, r.end, r.first, r.next), (0, 90, 0, 2));
        assert_eq!(r.last(), Some(1));
        // 10 left at the end and begin 0: key 0 goes, and 10 < 20 wraps to 0.
        assert_eq!(r.reserve(20), (0, 1));
        assert_eq!((r.begin, r.first), (60, 1));
        assert_eq!(r.allocate(0, 20), 2);
        assert_eq!((r.begin, r.end), (60, 20));
        // begin > end: the space is begin - end, strictly.
        assert!(r.has_space(39));
        assert!(!r.has_space(40));
        assert_eq!(r.reserve(39), (20, 0));
        assert_eq!(r.allocate(20, 39), 3);
        assert_eq!(r.end, 59);
        // 1 free: key 1 (60..90) goes and begin moves to its end.
        assert_eq!(r.reserve(10), (59, 1));
        assert_eq!((r.begin, r.first), (90, 2));
        r.deallocate();
        assert_eq!((r.begin, r.first), (20, 3), "deallocate sets begin to the freed block's end");
        assert_eq!(r.last(), Some(3));

        // Exactly enough room at the end does not wrap.
        let mut r = Ring::new(100);
        let (s, _) = r.reserve(60);
        r.allocate(s, 60);
        assert_eq!(r.reserve(40), (60, 1));
        r.allocate(60, 40);
        assert_eq!((r.begin, r.end), (60, 100));
    }

    #[test]
    fn encoder_follows_iterm2() {
        let (cols, rows) = (10, 50);
        let kb = key_bytes(cols, rows);
        let a = screen(cols, rows);
        let wide = screen(cols + 1, rows);

        // Clean share, size and the first frames.
        let mut d = Dvr::new(kb * 1000);
        assert_eq!(d.append(&a, &[], (0, 0), 1), Appended::Key, "no previous frame");
        assert_eq!(d.append(&a, &clean(rows, 41), (0, 0), 2), Appended::Key, "the first eligible frame");
        assert_eq!(d.append(&a, &clean(rows, 41), (0, 0), 3), Appended::Diff, "41 of 50 clean");
        assert_eq!(d.append(&a, &clean(rows, 40), (0, 0), 4), Appended::Key, "40 of 50 is not more than 80 %");
        assert_eq!(d.append(&a, &clean(rows, 41), (0, 0), 5), Appended::Diff);
        assert_eq!(d.append(&wide, &clean(rows, 50), (0, 0), 6), Appended::Key, "a size change");
        assert_eq!(d.append(&a, &clean(rows, 50), (0, 0), 7), Appended::Key, "and back");
        assert_eq!(d.append(&a, &clean(rows, 50), (0, 0), 8), Appended::Diff);
        assert_eq!(d.timestamps(), vec![1, 2, 3, 4, 5, 6, 7, 8], "stamped at commit");
        assert_eq!(d.frame(2).unwrap().info.kind, Kind::Diff);

        // The counter: 1st eligible a key, 99 diffs, the 101st a key; an ineligible
        // frame between them does not advance it.
        let mut d = Dvr::new(kb * 1000);
        let mut t = 0;
        let mut add = |d: &mut Dvr, c: &[bool]| {
            t += 1;
            d.append(&a, c, (0, 0), t)
        };
        assert_eq!(add(&mut d, &[]), Appended::Key);
        assert_eq!(add(&mut d, &clean(rows, 45)), Appended::Key);
        for i in 0..99 {
            if i == 50 {
                assert_eq!(add(&mut d, &clean(rows, 10)), Appended::Key, "ineligible");
            }
            assert_eq!(add(&mut d, &clean(rows, 45)), Appended::Diff, "eligible frame {}", i + 2);
        }
        assert_eq!(add(&mut d, &clean(rows, 45)), Appended::Key, "the 101st eligible frame");
        assert_eq!(add(&mut d, &clean(rows, 45)), Appended::Diff);

        // Too big for half the capacity: dropped, the buffer untouched.
        let mut d = Dvr::new(2 * kb - 1);
        assert_eq!(d.append(&a, &[], (0, 0), 1), Appended::TooBig);
        assert_eq!(d.ring, Ring::new(2 * kb - 1));
        assert!(d.is_empty());

        // Bytes since the key frame reaching half the capacity force a key frame.
        let cap = 20 * kb;
        let mut d = Dvr::new(cap);
        d.append(&a, &[], (0, 0), 1);
        d.append(&a, &clean(rows, 41), (0, 0), 2);
        let mut t = 2;
        while d.since_key < cap / 2 {
            t += 1;
            assert_eq!(d.append(&a, &clean(rows, 41), (0, 0), t), Appended::Diff);
            assert!(t < 90, "the byte trigger comes before the counter");
        }
        assert_eq!(d.ring.first, 0, "nothing evicted");
        assert_eq!(d.append(&a, &clean(rows, 41), (0, 0), t + 1), Appended::Key);
        assert_eq!(d.since_key, 0);

        // After freeing, leading diffs are evicted until a key frame leads.
        let mut d = Dvr::new(4 * kb);
        let mut rule_evicted = 0;
        for t in 0..600 {
            let before: Vec<Kind> = kinds(&d);
            let mut shadow = d.ring.clone();
            let (_, ring_freed) = shadow.reserve(kb);
            let first = d.ring.first;
            let c = if t % 37 == 0 { clean(rows, 0) } else { clean(rows, 45) };
            d.append(&a, &c, (0, 0), t);
            let evicted = (d.ring.first - first) as usize;
            assert!(evicted as u64 >= ring_freed);
            for k in &before[ring_freed as usize..evicted] {
                assert_eq!(*k, Kind::Diff, "only diffs go beyond what the ring freed");
            }
            rule_evicted += evicted - ring_freed as usize;
            assert_eq!(d.frames.front().unwrap().info.kind, Kind::Key);
            assert_eq!(d.frames.len() as u64, d.ring.next - d.ring.first);
        }
        assert!(rule_evicted > 0, "leading diffs were evicted");

        // Read-only records nothing.
        let mut d = Dvr::new(kb * 10);
        d.read_only = true;
        assert_eq!(d.append(&a, &[], (0, 0), 1), Appended::ReadOnly);
        assert!(d.is_empty());
    }

    /// Frames A (key), A (key, the first eligible), B = A with row 3 changed (diff),
    /// C = B with row 7 changed (diff), at 100, 200, 300, 400.
    fn chain() -> (Dvr, Vec<Vec<Row<Cell>>>) {
        let (cols, rows) = (10, 10);
        let a = screen(cols, rows);
        let mut b = a.clone();
        b[3] = row(cols, 'X');
        let mut c = b.clone();
        c[7] = row(cols, 'Y');
        let mut clean3 = vec![true; rows];
        clean3[3] = false;
        let mut clean7 = vec![true; rows];
        clean7[7] = false;
        let mut d = Dvr::new(key_bytes(cols, rows) * 100);
        d.append(&a, &[], (1, 1), 100);
        d.append(&a, &vec![true; rows], (2, 2), 200);
        d.append(&b, &clean3, (3, 3), 300);
        d.append(&c, &clean7, (4, 4), 400);
        assert_eq!(kinds(&d), vec![Kind::Key, Kind::Key, Kind::Diff, Kind::Diff]);
        (d, vec![a.clone(), a, b, c])
    }

    #[test]
    fn decoder_follows_iterm2() {
        let (d, shots) = chain();
        match &d.frame(3).unwrap().lines {
            Lines::Diff(l) => {
                assert!(l[7].is_some());
                assert_eq!(l.iter().filter(|r| r.is_some()).count(), 1, "only the dirty row is kept");
            }
            Lines::Key(_) => panic!("frame 3 is a diff"),
        }
        let mut dec = Decoder::default();
        assert!(dec.seek(&d, 400));
        assert_eq!(dec.rows, shots[3], "two diffs rebuilt on the key frame");
        assert_eq!(dec.info.unwrap().cursor, (4, 4));
        assert_eq!(dec.key(&d), Some(3));
        assert!(dec.seek(&d, 250), "the first frame at or after");
        assert_eq!(dec.timestamp(), 300);
        assert_eq!(dec.rows, shots[2]);
        assert!(!dec.seek(&d, 401));
        assert!(dec.seek(&d, 0));
        assert_eq!(dec.key(&d), Some(0));
        assert!(!dec.prev(&d), "prev at the first key");
        assert!(dec.next(&d));
        assert!(dec.next(&d));
        assert!(dec.next(&d));
        assert_eq!(dec.rows, shots[3]);
        assert!(!dec.next(&d), "next past the last");
        assert!(dec.prev(&d));
        assert_eq!((dec.timestamp(), &dec.rows), (300, &shots[2]));

        assert_eq!(d.first_timestamp_after(150), 200);
        assert_eq!(d.first_timestamp_after(200), 300);
        assert_eq!(d.first_timestamp_after(0), 100);
        assert_eq!(d.first_timestamp_after(400), 0);
        assert_eq!((d.first_timestamp(), d.last_timestamp()), (100, 400));
        let empty = Dvr::new(1000);
        assert_eq!((empty.first_timestamp(), empty.last_timestamp()), (0, 0));
        assert!(!Decoder::default().next(&empty));

        // Evicting the decoder's frame strands it: the rows stay, prev is false and
        // next lands on the first frame.
        let (cols, rows) = (10, 10);
        let kb = key_bytes(cols, rows);
        let mut d = Dvr::new(4 * kb);
        let mut t = 0;
        for _ in 0..2 {
            t += 1;
            d.append(&screen(cols, rows), &[], (0, 0), t);
        }
        let mut dec = Decoder::default();
        assert!(dec.seek(&d, 0));
        assert_eq!(dec.key(&d), Some(0));
        let kept = dec.rows.clone();
        let other: Vec<Row<Cell>> = (0..rows).map(|_| row(cols, 'Z')).collect();
        while d.ring.first == 0 {
            t += 1;
            d.append(&other, &[], (0, 0), t);
        }
        assert_eq!(dec.key(&d), None);
        assert_eq!(dec.rows, kept, "the evicted frame stays on screen");
        assert!(!dec.prev(&d));
        assert!(dec.next(&d));
        assert_eq!(dec.key(&d), Some(d.ring.first));
        assert_eq!(dec.timestamp(), d.first_timestamp());
    }

    #[test]
    fn bytes_follow_iterm2() {
        let cell = std::mem::size_of::<Cell>() as u64;
        assert_eq!(line_bytes(80), cell * 81);
        assert_eq!(key_bytes(80, 24), 24 * (line_bytes(80) + 4));
        assert_eq!(diff_bytes(80, 1, 24), 24 * 5 + line_bytes(80) + 24 * 5);
        assert!(diff_bytes(80, 24, 24) > key_bytes(80, 24), "an all-dirty diff falls back to a key frame");

        let a = screen(80, 24);
        let kb = key_bytes(80, 24);
        let mut d = Dvr::new(2 * (kb - 1));
        assert_eq!(d.append(&a, &[], (0, 0), 1), Appended::TooBig, "capacity / 2 + 1 bytes");
        let mut d = Dvr::new(2 * kb);
        assert_eq!(d.append(&a, &[], (0, 0), 1), Appended::Key, "exactly capacity / 2 fits");

        // A 4 MiB recorder drops any frame over 2 MiB.
        let mib4 = 4 * 1024 * 1024;
        let fit = (mib4 / 2 / (line_bytes(80) + 4)) as usize;
        let mut d = Dvr::new(mib4);
        assert_eq!(d.append(&screen(80, fit), &[], (0, 0), 1), Appended::Key);
        assert_eq!(d.append(&screen(80, fit + 1), &[], (0, 0), 2), Appended::TooBig);
    }

    #[test]
    fn copy_range_follows_iterm2() {
        let (d, shots) = chain();
        let mut clock = 1000;
        let mut now = || {
            clock += 1;
            clock
        };
        let whole = d.copy_range(100, 400, &mut now);
        assert_eq!(whole.timestamps(), vec![100, 200, 300, 400]);
        assert_eq!(kinds(&whole), kinds(&d));

        let part = d.copy_range(200, 300, &mut now);
        assert_eq!(part.timestamps(), vec![1001, 1002], "restamped");
        assert_eq!(kinds(&part), vec![Kind::Key, Kind::Key]);
        let mut dec = Decoder::default();
        assert!(dec.seek(&part, 1002));
        assert_eq!(dec.rows, shots[2]);
        assert_eq!(dec.info.unwrap().cursor, (3, 3), "the cursor is kept");

        let tail = d.copy_range(250, -1, &mut now);
        assert_eq!(tail.timestamps(), vec![1003, 1004], "to -1 runs to the end");

        assert!(d.can_clear(false));
        assert!(!d.can_clear(true), "not while replaying");
        let mut c = d.clone();
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.ring, Ring::new(d.ring.capacity));
        assert_eq!(c.last_timestamp(), 0);
        let mut ro = d.clone();
        ro.read_only = true;
        assert!(!ro.can_clear(false));
        ro.clear();
        assert_eq!(ro.timestamps(), d.timestamps(), "a read-only recorder is not cleared");
    }
}
