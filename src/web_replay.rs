//! Browser Instant Replay, the pure part (milestone 13x, Task 10): the frame ring bounded by the
//! Instant Replay memory setting, the sensitive-page mark, the capture interval and its back-off, the playback clock, the time text and the
//! Motion JPEG AVI writer. iTerm2: `BVC:258-340`, `IRMB:22-60`, `VPW`. No web view, no window, no disk.
//! Frames are page pictures, so they are kept in memory only: nothing here touches a file.

use std::time::Duration;

pub const WINDOW_TITLE: &str = "Instant Replay";
pub const PLAY: &str = "Play";
pub const PAUSE: &str = "Pause";
pub const PLAY_BACKWARD: &str = "Play Backward";
/// `VideoPlayback.RevealInFinder` with Finder named Explorer.
pub const REVEAL: &str = "Reveal in Explorer";
/// `BrowserViewController.ProblemSavingMovieHeading` (`BVC:277-285`).
pub const MOVIE_HEADING: &str = "Problem saving instant replay movie";
/// The `WebInstantReplayFrameRate` default (`AS:845`).
pub const DEFAULT_RATE: f64 = 60.0;
/// P16: one frame over this is dropped.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
/// P16: the sum over every pane's ring.
pub const TOTAL_CAP: usize = 256 * 1024 * 1024;
/// P16: the writer refuses a movie over this.
pub const MAX_AVI: usize = 1024 * 1024 * 1024;
/// The movie's timeline (frames per second); the capture rate is at most this in practice.
pub const FPS: u32 = 60;
/// A pause between two frames longer than this plays as this long (a hidden pane makes none).
pub const MAX_GAP: Duration = Duration::from_secs(2);
/// No capture tick is more often than this (a capture takes longer anyway).
pub const MIN_TICK: Duration = Duration::from_millis(10);

/// The script tokens of the capture round trip (the app's other script users pick theirs elsewhere).
pub const PROBE_TOKEN: u64 = 0x7E57_0001;
pub const CAPTURE_TOKEN: u64 = 0x7E57_0002;
/// A capture that has not answered by then is forgotten (the view was hidden or went away).
pub const PENDING_LIMIT: Duration = Duration::from_secs(5);

/// The `autocomplete` tokens that make a field sensitive (a password, a one-time code, a card).
pub const SENSITIVE_AUTOCOMPLETE: &[&str] = &["current-password", "new-password", "one-time-code", "cc-number", "cc-csc"];

/// The top document's flag the sensitive mark script keeps (a getter a page cannot redefine).
pub const SENSITIVE_FLAG: &str = "__tandemReplaySensitive";

/// The one CSS selector of a sensitive field: a password input, or an input whose `autocomplete`
/// holds one of `SENSITIVE_AUTOCOMPLETE` as a whole token, in any case.
pub fn sensitive_selector() -> String {
    let mut parts = vec!["input[type=password i]".to_string()];
    parts.extend(SENSITIVE_AUTOCOMPLETE.iter().map(|t| format!("input[autocomplete~=\"{t}\" i]")));
    parts.join(",")
}

/// The rule `sensitive_selector` states, for an input of `type` with `autocomplete`.
pub fn sensitive_input(kind: &str, autocomplete: &str) -> bool {
    kind.eq_ignore_ascii_case("password") || autocomplete.split_ascii_whitespace().any(|t| SENSITIVE_AUTOCOMPLETE.iter().any(|s| s.eq_ignore_ascii_case(t)))
}

/// The script run at the start of every document in every frame (final review I1): it marks the
/// page sensitive once a field `sensitive_selector` matches exists anywhere in it, found by a scan
/// at the start and at DOMContentLoaded, a MutationObserver (attributes too, so a field turned into
/// a password field counts), focus and input in the capture phase, inside open shadow roots and in
/// every shadow root made later (open or closed, through `attachShadow`). A child frame tells the
/// top with `window.top.postMessage` (it stays in the page: no new host channel); the top keeps
/// the mark in a getter defined before any page script, so a page can only set it, never clear
/// it. The mark ends only with the top document (a top-level navigation): a "show password"
/// toggle that turns the field to text does not resume capture.
pub fn sensitive_script() -> String {
    let sel = sensitive_selector().replace('\\', "\\\\").replace('\'', "\\'");
    let flag = SENSITIVE_FLAG;
    format!(
        "(function(){{try{{var SEL='{sel}';var top=window===window.top;var hit=false;var watchers=[];\
function mark(){{if(hit)return;hit=true;for(var i=0;i<watchers.length;i++){{try{{watchers[i].disconnect()}}catch(e){{}}}}watchers=[];\
if(!top){{try{{window.top.postMessage({{{flag}:1}},'*')}}catch(e){{}}}}}}\
if(top){{Object.defineProperty(window,'{flag}',{{get:function(){{return hit}},configurable:false}});\
window.addEventListener('message',function(e){{try{{if(e.data&&e.data.{flag}===1)mark()}}catch(x){{}}}},true)}}\
function scan(root){{if(hit||!root)return;try{{if(root.shadowRoot)scan(root.shadowRoot);if(root.querySelector&&root.querySelector(SEL))return mark();\
var all=root.querySelectorAll?root.querySelectorAll('*'):[];for(var i=0;i<all.length&&!hit;i++){{if(all[i].shadowRoot)scan(all[i].shadowRoot)}}}}catch(e){{}}}}\
function check(n){{try{{if(!n||n.nodeType!==1)return;if(n.matches(SEL))return mark();scan(n)}}catch(e){{}}}}\
function watch(root){{if(hit)return;try{{var o=new MutationObserver(function(rs){{for(var i=0;i<rs.length&&!hit;i++){{var r=rs[i];\
if(r.type==='attributes')check(r.target);else for(var j=0;j<r.addedNodes.length;j++)check(r.addedNodes[j])}}}});\
o.observe(root,{{childList:true,subtree:true,attributes:true,attributeFilter:['type','autocomplete']}});watchers.push(o)}}catch(e){{}}scan(root)}}\
try{{var attach=Element.prototype.attachShadow;Element.prototype.attachShadow=function(){{var r=attach.apply(this,arguments);try{{watch(r)}}catch(e){{}}return r}}}}catch(e){{}}\
function ev(e){{try{{var t=e.composedPath?e.composedPath()[0]:e.target;if(t&&t.nodeType===1&&t.matches(SEL))mark()}}catch(x){{}}}}\
document.addEventListener('focusin',ev,true);document.addEventListener('input',ev,true);\
watch(document);document.addEventListener('DOMContentLoaded',function(){{scan(document)}})}}catch(e){{}}}})()"
    )
}

/// The script run before each capture: true while the page is marked sensitive (the mark script
/// above, in any frame), or while a visible password field is in the top document; also true when
/// the mark script is not there or anything fails (fail closed). Only a plain `false` lets a
/// capture through.
pub const PROBE_JS: &str = "(function(){try{if(window.__tandemReplaySensitive!==false)return true;var a=document.querySelectorAll('input[type=password]');for(var i=0;i<a.length;i++){var r=a[i].getBoundingClientRect();if(r.width>0&&r.height>0)return true}return false}catch(e){return true}})()";

/// Whether the probe's answer lets the capture go on: only a plain `false`.
pub fn probe_allows(answer: &Result<String, String>) -> bool {
    matches!(answer, Ok(s) if s.trim() == "false")
}

/// The folder a saved movie goes to (P16): Termurion's own, swept at the next launch and at exit.
pub fn replay_folder(local_appdata: &std::path::Path) -> std::path::PathBuf {
    local_appdata.join("Tandem").join("Replay")
}

/// `Could not create movie: %@` (`BVC:277-285`).
pub fn movie_failed(e: &str) -> String {
    format!("Could not create movie: {e}")
}

/// The Instant Replay memory setting (`IRMemory`, megabytes per session) as a byte cap; 0 is off.
pub fn cap_bytes(mb: i64) -> usize {
    mb.clamp(0, 1000) as usize * 1024 * 1024
}

/// Whether a pane captures now: its profile has Instant Replay on, it is not Dev Null (nothing of a
/// private pane is kept), the memory setting is not 0, the pane is shown and its window is on
/// screen (not minimized or hidden: no idle wake, no blank frames pushing out the history).
pub fn recording(profile_on: bool, dev_null: bool, mb: i64, shown: bool, on_screen: bool) -> bool {
    profile_on && !dev_null && mb > 0 && shown && on_screen
}

/// The slowest the capture backs off to while the page does not change.
pub const MAX_BACKOFF: Duration = Duration::from_secs(1);

/// The wait before the next capture: the rate's `base` after a changed frame; after a repeat (or a
/// refused probe) double the last wait, at least twice the rate and at most `MAX_BACKOFF`.
pub fn backoff(last: Duration, base: Duration, repeated: bool) -> Duration {
    if !repeated {
        return base;
    }
    last.max(base).saturating_mul(2).min(MAX_BACKOFF.max(base))
}

/// The time between captures at `rate` frames per second; None for a rate that is not a positive
/// finite number.
pub fn interval(rate: f64) -> Option<Duration> {
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(1.0 / rate).ok()
}

/// `interval`, never faster than `MIN_TICK`.
pub fn tick_interval(rate: f64) -> Option<Duration> {
    interval(rate).map(|d| d.max(MIN_TICK))
}

/// `formatTime` (`VPW:319-325`): `%02d:%02d` of the whole minutes and seconds.
pub fn format_time(secs: f64) -> String {
    let total = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// One pane's frames, oldest first, each with its capture time since the recording began.
pub struct Ring {
    max: usize,
    frames: Vec<(Duration, Vec<u8>)>,
    bytes: usize,
}

impl Ring {
    /// A ring that keeps at most `max_bytes` (0 keeps nothing).
    pub fn new(max_bytes: usize) -> Ring {
        Ring { max: max_bytes, frames: Vec::new(), bytes: 0 }
    }

    /// Adds a frame, then drops the oldest until the total fits. The newest frame always stays (a
    /// frame bigger than the cap is kept alone), unless it is over `MAX_FRAME` (dropped) or empty.
    /// A frame byte-equal to the newest is not kept (the newest's time stands, and the timeline
    /// plays the gap); false then, true for any other frame.
    pub fn push(&mut self, at: Duration, jpeg: Vec<u8>) -> bool {
        if self.frames.last().is_some_and(|(_, last)| last.len() == jpeg.len() && *last == jpeg) {
            return false;
        }
        if self.max == 0 || jpeg.is_empty() || jpeg.len() > MAX_FRAME {
            return true;
        }
        self.bytes += jpeg.len();
        self.frames.push((at, jpeg));
        self.fit();
        true
    }

    fn fit(&mut self) {
        while self.bytes > self.max && self.frames.len() > 1 {
            self.drop_oldest();
        }
    }

    pub fn frames(&self) -> &[(Duration, Vec<u8>)] {
        &self.frames
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    /// The bytes of every frame held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// A new cap (the memory setting changed); frames over it go, oldest first.
    pub fn set_max(&mut self, max_bytes: usize) {
        self.max = max_bytes;
        if max_bytes == 0 {
            return self.clear();
        }
        self.fit();
    }

    /// Drops the oldest frame; false when there was none.
    pub fn drop_oldest(&mut self) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        let (_, f) = self.frames.remove(0);
        self.bytes -= f.len();
        true
    }
}

/// Keeps the sum of several rings at most `total` bytes: the ring whose oldest frame is oldest
/// shrinks first (P16). Each ring comes with the time its recording began, since its frames are
/// timed from there.
pub fn trim_total(rings: &mut [(&mut Ring, Duration)], total: usize) {
    loop {
        let sum: usize = rings.iter().map(|(r, _)| r.bytes()).sum();
        if sum <= total {
            return;
        }
        let oldest = rings.iter_mut().filter_map(|(r, began)| r.frames().first().map(|f| *began + f.0).map(|t| (t, r))).min_by_key(|(t, _)| *t);
        match oldest {
            Some((_, r)) => {
                r.drop_oldest();
            }
            None => return,
        }
    }
}

/// The pixel size a JPEG's frame header (SOF) says; None for anything else.
pub fn jpeg_size(jpeg: &[u8]) -> Option<(u32, u32)> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xFF {
            return None;
        }
        let marker = jpeg[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // Markers with no length: TEM, RSTn, SOI. EOI and a scan end the header part.
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            at += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let b = jpeg.get(at + 4..at + 9)?;
            let (h, w) = (u16::from_be_bytes([b[1], b[2]]) as u32, u16::from_be_bytes([jpeg[at + 7], jpeg[at + 8]]) as u32);
            return (w > 0 && h > 0).then_some((w, h));
        }
        at += 2 + len.max(2);
    }
    None
}

/// Where each frame plays, in seconds from the first: the real spacing, a pause longer than
/// `MAX_GAP` shortened to it. The movie and the playback window share it.
pub fn timeline(frames: &[(Duration, Vec<u8>)]) -> Vec<f64> {
    let mut out = Vec::with_capacity(frames.len());
    let mut at = Duration::ZERO;
    for (i, f) in frames.iter().enumerate() {
        if i > 0 {
            at += f.0.saturating_sub(frames[i - 1].0).min(MAX_GAP);
        }
        out.push(at.as_secs_f64());
    }
    out
}

fn le32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + data.len() + 1);
    v.extend_from_slice(tag);
    le32(&mut v, data.len() as u32);
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + body.len());
    data.extend_from_slice(kind);
    data.extend_from_slice(body);
    chunk(b"LIST", &data)
}

/// A Motion JPEG AVI of `frames` (JPEG bytes) at `w` by `h`: one video stream on a 60 per second
/// timeline, each frame at its capture time (a zero-length chunk repeats the frame before it, so the
/// timing survives a constant frame rate), with an index. Empty when there is nothing to save, the
/// size is 0 or the movie would pass `MAX_AVI` (P16).
pub fn mjpeg_avi(frames: &[(Duration, Vec<u8>)], w: u32, h: u32) -> Vec<u8> {
    if frames.is_empty() || w == 0 || h == 0 {
        return Vec::new();
    }
    let marks = timeline(frames);
    // One slot per tick of the timeline; a frame landing on a tick that is taken is left out.
    let mut slots: Vec<Option<&[u8]>> = Vec::new();
    for (i, (_, jpeg)) in frames.iter().enumerate() {
        let tick = (marks[i] * FPS as f64).round() as usize;
        if i > 0 && tick < slots.len() {
            continue;
        }
        while slots.len() < tick {
            slots.push(None);
        }
        slots.push(Some(jpeg));
    }
    let payload: usize = slots.iter().map(|s| 8 + s.map_or(0, |j| j.len() + (j.len() & 1))).sum();
    if payload.saturating_add(slots.len() * 16) > MAX_AVI {
        return Vec::new();
    }
    let biggest = slots.iter().map(|s| s.map_or(0, |j| j.len())).max().unwrap_or(0) as u32;
    let total = slots.len() as u32;

    let mut avih = Vec::with_capacity(56);
    for n in [1_000_000 / FPS, 0, 0, 0x10, total, 0, 1, biggest, w, h, 0, 0, 0, 0] {
        le32(&mut avih, n);
    }
    let mut strh = Vec::with_capacity(56);
    strh.extend_from_slice(b"vidsMJPG");
    for n in [0, 0, 0, 1, FPS, 0, total, biggest, 0xFFFF_FFFF, 0] {
        le32(&mut strh, n);
    }
    for n in [0u16, 0, w.min(0xFFFF) as u16, h.min(0xFFFF) as u16] {
        strh.extend_from_slice(&n.to_le_bytes());
    }
    let mut strf = Vec::with_capacity(40);
    for n in [40, w, h] {
        le32(&mut strf, n);
    }
    strf.extend_from_slice(&1u16.to_le_bytes());
    strf.extend_from_slice(&24u16.to_le_bytes());
    strf.extend_from_slice(b"MJPG");
    for n in [w.saturating_mul(h).saturating_mul(3), 0, 0, 0, 0] {
        le32(&mut strf, n);
    }
    let strl = list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)].concat());
    let hdrl = list(b"hdrl", &[chunk(b"avih", &avih), strl].concat());

    // The movie list; the index offsets count from the "movi" word.
    let mut movi = Vec::with_capacity(payload + 4);
    movi.extend_from_slice(b"movi");
    let mut idx = Vec::with_capacity(slots.len() * 16);
    for s in &slots {
        let data = s.unwrap_or(&[]);
        idx.extend_from_slice(b"00dc");
        le32(&mut idx, if data.is_empty() { 0 } else { 0x10 });
        le32(&mut idx, movi.len() as u32);
        le32(&mut idx, data.len() as u32);
        movi.extend_from_slice(&chunk(b"00dc", data));
    }
    let movi = chunk(b"LIST", &movi);
    let idx1 = chunk(b"idx1", &idx);

    let mut body = Vec::with_capacity(4 + hdrl.len() + movi.len() + idx1.len());
    body.extend_from_slice(b"AVI ");
    body.extend_from_slice(&hdrl);
    body.extend_from_slice(&movi);
    body.extend_from_slice(&idx1);
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    le32(&mut out, body.len() as u32);
    out.extend_from_slice(&body);
    out
}

/// The playback position, in seconds on the `timeline`: `dir` is 1 playing, -1 playing backward, 0
/// paused.
#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub pos: f64,
    pub dir: i32,
    pub end: f64,
}

impl Clock {
    /// Paused on the last frame (`seekToLastFrame`, `VPW:420-430`).
    pub fn at_end(end: f64) -> Clock {
        let end = if end.is_finite() { end.max(0.0) } else { 0.0 };
        Clock { pos: end, dir: 0, end }
    }

    /// `dt` seconds pass: the position moves, and playing stops at either end.
    pub fn tick(&mut self, dt: f64) {
        if self.dir == 0 || !dt.is_finite() {
            return;
        }
        self.pos += self.dir as f64 * dt.max(0.0);
        if self.pos >= self.end && self.dir > 0 {
            self.pos = self.end;
            self.dir = 0;
        } else if self.pos <= 0.0 && self.dir < 0 {
            self.pos = 0.0;
            self.dir = 0;
        }
    }

    pub fn seek(&mut self, to: f64) {
        if to.is_finite() {
            self.pos = to.clamp(0.0, self.end);
        }
    }

    /// Play / Pause (`togglePlayPause`): playing pauses; else it plays from the position, from the
    /// start when the position is the end.
    pub fn forward(&mut self) {
        if self.dir == 1 {
            return self.pause();
        }
        if self.pos >= self.end {
            self.pos = 0.0;
        }
        self.dir = 1;
    }

    /// Play Backward (`togglePlayBackward`): playing backward pauses.
    pub fn backward(&mut self) {
        self.dir = if self.dir == -1 { 0 } else { -1 };
    }

    pub fn pause(&mut self) {
        self.dir = 0;
    }

    /// An arrow key: paused, then `by` frames from the shown one (clamped to the first and last).
    pub fn step(&mut self, marks: &[f64], by: i32) {
        self.pause();
        if marks.is_empty() {
            return;
        }
        let to = (self.index(marks) as i64 + by as i64).clamp(0, marks.len() as i64 - 1) as usize;
        self.pos = marks[to];
    }

    /// The frame shown: the last one at or before the position.
    pub fn index(&self, marks: &[f64]) -> usize {
        marks.iter().rposition(|m| *m <= self.pos + 1e-9).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn frame_ring_follows_iterm2() {
        let mut r = Ring::new(10);
        r.push(ms(0), vec![1; 4]);
        r.push(ms(10), vec![2; 4]);
        r.push(ms(20), vec![3; 4]);
        // 12 bytes over the cap of 10: the oldest goes first.
        assert_eq!(r.frames().iter().map(|f| f.0).collect::<Vec<_>>(), vec![ms(10), ms(20)]);
        assert_eq!(r.bytes(), 8);
        r.push(ms(30), vec![4; 2]);
        assert_eq!(r.bytes(), 10);
        assert_eq!(r.frames().len(), 3);
        // One frame larger than the cap stays alone (the newest always stays).
        r.push(ms(40), vec![5; 11]);
        assert_eq!(r.frames().len(), 1);
        assert_eq!(r.frames()[0].0, ms(40));
        assert_eq!(r.bytes(), 11);
        r.clear();
        assert!(r.frames().is_empty());
        assert_eq!(r.bytes(), 0);
        // The rate is an interval, and anything that is not a positive number is none.
        let i = interval(60.0).unwrap();
        assert!((i.as_secs_f64() - 0.016_666).abs() < 0.000_01, "{i:?}");
        assert_eq!(interval(0.0), None);
        assert_eq!(interval(-1.0), None);
        assert_eq!(interval(f64::NAN), None);
        assert_eq!(interval(f64::INFINITY), None);
        assert_eq!(interval(1e-300), None);
        assert_eq!(tick_interval(1e6), Some(MIN_TICK), "a capture is never asked for faster than 100 a second");
        assert_eq!(tick_interval(0.0), None);
        let d = crate::schema::find_global("WebInstantReplayFrameRate").and_then(|s| s.default.clone()).unwrap();
        assert_eq!(d, toml::Value::Float(60.0));
        let s = crate::schema::find_global("WebInstantReplayFrameRate").unwrap();
        assert_eq!((s.tab.as_str(), s.section.as_str()), ("Advanced", "Web Browser"));
    }

    #[test]
    fn replay_is_bounded_three_ways() {
        // P16: one frame over 8 MiB is dropped (the newest-stays rule is under that bound).
        let mut r = Ring::new(100 * 1024 * 1024);
        r.push(ms(0), vec![0; MAX_FRAME + 1]);
        assert!(r.frames().is_empty());
        r.push(ms(1), Vec::new());
        assert!(r.frames().is_empty(), "an empty frame is nothing");
        // IRMemory: MB to bytes, 0 is off, and a ring of 0 keeps nothing.
        assert_eq!(cap_bytes(4), 4 * 1024 * 1024);
        assert_eq!(cap_bytes(0), 0);
        assert_eq!(cap_bytes(-3), 0);
        assert_eq!(cap_bytes(5000), 1000 * 1024 * 1024);
        let mut off = Ring::new(0);
        off.push(ms(0), vec![1]);
        assert!(off.frames().is_empty());
        // Lowering the cap trims, oldest first, keeping the newest.
        let mut r = Ring::new(100);
        for i in 0..5 {
            r.push(ms(i), vec![i as u8; 10]);
        }
        r.set_max(25);
        assert_eq!(r.frames().len(), 2);
        assert_eq!(r.frames()[1].0, ms(4));
        // The sum over all panes: the ring whose oldest frame is oldest shrinks first.
        let (mut a, mut b) = (Ring::new(1000), Ring::new(1000));
        a.push(ms(0), vec![0; 10]);
        a.push(ms(500), vec![1; 10]);
        b.push(ms(0), vec![0; 10]);
        b.push(ms(100), vec![1; 10]);
        // b started 200 ms after a: its oldest frame is at 200, a's at 0.
        trim_total(&mut [(&mut a, ms(0)), (&mut b, ms(200))], 30);
        assert_eq!((a.frames().len(), b.frames().len()), (1, 2));
        assert_eq!(a.frames()[0].0, ms(500));
        trim_total(&mut [(&mut a, ms(0)), (&mut b, ms(200))], 0);
        assert_eq!(a.bytes() + b.bytes(), 0);
        // Capture runs only while the profile has it on, the pane shows, the setting is not 0 and the
        // pane is not Dev Null.
        assert!(recording(true, false, 4, true, true));
        assert!(!recording(false, false, 4, true, true));
        assert!(!recording(true, true, 4, true, true), "a Dev Null pane is never recorded");
        assert!(!recording(true, false, 0, true, true));
        assert!(!recording(true, false, 4, false, true), "a hidden pane has no idle wake");
    }

    #[test]
    fn the_capture_skips_a_page_with_a_password_field() {
        assert!(probe_allows(&Ok("false".into())));
        assert!(!probe_allows(&Ok("true".into())));
        assert!(!probe_allows(&Ok("null".into())));
        assert!(!probe_allows(&Err("x".into())));
        assert!(PROBE_JS.contains("type=password"));
        assert_ne!(PROBE_TOKEN, CAPTURE_TOKEN);
        let f = replay_folder(std::path::Path::new(r"C:\Users\a\AppData\Local"));
        assert!(f.ends_with(r"Termurion\Replay"), "{f:?}");
    }

    /// Final review I1: one script in every frame marks the page sensitive while a password,
    /// one-time-code or card field exists anywhere in it (shadow roots and frames included), and the
    /// mark sticks until the top page navigates.
    #[test]
    fn the_sensitive_mark_covers_frames_shadow_roots_and_field_kinds() {
        let sel = sensitive_selector();
        assert!(sel.contains("input[type=password i]"), "{sel}");
        for t in ["current-password", "new-password", "one-time-code", "cc-number", "cc-csc"] {
            assert!(sel.contains(&format!("input[autocomplete~=\"{t}\" i]")), "{t}: {sel}");
            assert!(SENSITIVE_AUTOCOMPLETE.contains(&t));
        }
        // The rule the selector states, in Rust (tokens, any case).
        assert!(sensitive_input("password", ""));
        assert!(sensitive_input("PassWord", ""));
        assert!(sensitive_input("text", "section-pay billing cc-number"));
        assert!(sensitive_input("text", "ONE-TIME-CODE"));
        assert!(sensitive_input("tel", "cc-csc"));
        assert!(!sensitive_input("text", "username"));
        assert!(!sensitive_input("text", "cc-numbers"), "a whole token, not a prefix");
        assert!(!sensitive_input("email", "email"));
        let js = sensitive_script();
        assert!(js.contains(&sel), "the script uses the one selector");
        // Every frame reports to the top through the page itself (no new host channel); the top
        // keeps a flag a page cannot clear (a getter defined before any page script).
        assert!(js.contains("window.top.postMessage"));
        assert!(js.contains(&format!("Object.defineProperty(window,'{SENSITIVE_FLAG}'")));
        assert!(js.contains("configurable:false"));
        // Found by a scan at DOMContentLoaded, a MutationObserver, focus and input in the capture
        // phase, inside open shadow roots, and in every shadow root made later (open or closed).
        for part in ["DOMContentLoaded", "MutationObserver", "'focusin'", "'input'", "shadowRoot", "attachShadow", "composedPath"] {
            assert!(js.contains(part), "{part}");
        }
        assert!(!js.contains("\u{2014}") && js.is_ascii());
        // The probe reads the flag first and fails closed when the script is not there.
        assert!(PROBE_JS.contains(&format!("window.{SENSITIVE_FLAG}!==false")), "{PROBE_JS}");
    }

    /// Final review I2: a window that is not on screen (minimized, hidden) never records.
    #[test]
    fn a_window_off_screen_never_records() {
        assert!(recording(true, false, 4, true, true));
        assert!(!recording(true, false, 4, true, false), "a minimized window has no idle wake");
    }

    /// Final review I3: a frame byte-equal to the newest is not kept, and the capture backs off
    /// while nothing changes.
    #[test]
    fn identical_frames_are_kept_once_and_the_capture_backs_off() {
        let mut r = Ring::new(1000);
        assert!(r.push(ms(0), vec![1, 2, 3]));
        assert!(!r.push(ms(10), vec![1, 2, 3]), "a repeat is not new");
        assert_eq!(r.frames().len(), 1);
        assert_eq!(r.frames()[0].0, ms(0), "the first frame's time stands");
        assert!(r.push(ms(20), vec![1, 2, 4]));
        assert!(r.push(ms(30), vec![1, 2, 3]), "only the newest is compared");
        assert_eq!(r.frames().len(), 3);
        let base = ms(16);
        let mut wait = base;
        let mut seen = Vec::new();
        for _ in 0..10 {
            wait = backoff(wait, base, true);
            seen.push(wait);
        }
        assert_eq!(seen[0], ms(32));
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), MAX_BACKOFF, "never slower than once a second");
        assert_eq!(backoff(wait, base, false), base, "a change resets the rate");
        assert_eq!(backoff(ms(1), base, true), ms(32), "never faster than the rate");
    }

    #[test]
    fn playback_follows_iterm2() {
        // VPW:319-325: minutes and seconds, two digits each.
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(65.4), "01:05");
        assert_eq!(format_time(3600.0), "60:00");
        assert_eq!(format_time(-5.0), "00:00");
        assert_eq!(format_time(f64::NAN), "00:00");
        assert_eq!(WINDOW_TITLE, "Instant Replay");
        assert_eq!(PLAY, "Play");
        assert_eq!(PAUSE, "Pause");
        assert_eq!(PLAY_BACKWARD, "Play Backward");
        assert_eq!(REVEAL, "Reveal in Explorer");
        assert_eq!(MOVIE_HEADING, "Problem saving instant replay movie");
        assert_eq!(movie_failed("disk full"), "Could not create movie: disk full");
    }

    #[test]
    fn the_clock_plays_pauses_and_steps() {
        let marks = [0.0, 1.0, 2.0, 3.0];
        let mut c = Clock::at_end(3.0);
        assert_eq!((c.pos, c.dir), (3.0, 0), "iTerm2 seeks to the last frame first");
        assert_eq!(c.index(&marks), 3);
        c.forward();
        assert_eq!((c.pos, c.dir), (0.0, 1), "play at the end starts again");
        c.tick(1.5);
        assert_eq!(c.pos, 1.5);
        assert_eq!(c.index(&marks), 1);
        c.tick(5.0);
        assert_eq!((c.pos, c.dir), (3.0, 0), "forward stops at the end");
        c.backward();
        assert_eq!(c.dir, -1);
        c.tick(1.0);
        assert_eq!(c.pos, 2.0);
        c.backward();
        assert_eq!(c.dir, 0, "backward again pauses");
        c.backward();
        c.tick(10.0);
        assert_eq!((c.pos, c.dir), (0.0, 0), "backward stops at the start");
        c.step(&marks, 1);
        assert_eq!(c.pos, 1.0);
        c.step(&marks, 1);
        assert_eq!(c.pos, 2.0);
        c.step(&marks, -1);
        assert_eq!(c.pos, 1.0);
        c.step(&marks, -5);
        assert_eq!(c.pos, 0.0);
        c.seek(99.0);
        assert_eq!(c.pos, 3.0);
        c.seek(-1.0);
        assert_eq!(c.pos, 0.0);
        c.seek(f64::NAN);
        assert_eq!(c.pos, 0.0);
        // A step pauses.
        c.forward();
        c.step(&marks, 1);
        assert_eq!(c.dir, 0);
    }

    /// A minimal JPEG header: SOI, an APP0, SOF0 with the size, EOI.
    fn jpeg(w: u16, h: u16, pad: usize) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        v.extend([0xFF, 0xC0, 0x00, 0x0B, 8]);
        v.extend(h.to_be_bytes());
        v.extend(w.to_be_bytes());
        v.extend([1, 1, 0x11, 0]);
        v.extend(std::iter::repeat_n(0xAA, pad));
        v.extend([0xFF, 0xD9]);
        v
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn jpeg_size_reads_the_frame_header() {
        assert_eq!(jpeg_size(&jpeg(640, 480, 3)), Some((640, 480)));
        assert_eq!(jpeg_size(&[0xFF, 0xD8]), None);
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        assert_eq!(jpeg_size(&jpeg(0, 5, 0)), None, "a zero size is no size");
    }

    #[test]
    fn the_timeline_compresses_idle_gaps() {
        let f = |t| (ms(t), vec![1u8]);
        let t = timeline(&[f(0), f(100), f(10_000), f(10_050)]);
        let want = [0.0, 0.1, 2.1, 2.15];
        assert!(t.len() == 4 && t.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9), "{t:?}");
        assert_eq!(timeline(&[]), Vec::<f64>::new());
    }

    #[test]
    fn the_avi_has_right_sizes_and_frame_count() {
        let frames = vec![(ms(100), jpeg(64, 48, 1)), (ms(200), jpeg(64, 48, 2)), (ms(250), jpeg(64, 48, 0))];
        let avi = mjpeg_avi(&frames, 64, 48);
        assert_eq!(&avi[0..4], b"RIFF");
        assert_eq!(&avi[8..12], b"AVI ");
        assert_eq!(u32_at(&avi, 4) as usize, avi.len() - 8, "the RIFF size covers the rest");
        assert_eq!(avi.len() % 2, 0);
        // The timeline is 60 per second from the first frame: 0, 6, 9 -> 10 chunks.
        let find = |tag: &[u8]| avi.windows(4).position(|w| w == tag).unwrap();
        let avih = find(b"avih");
        assert_eq!(u32_at(&avi, avih + 4), 56);
        assert_eq!(u32_at(&avi, avih + 8 + 16) as usize, 10, "dwTotalFrames");
        assert_eq!(u32_at(&avi, avih + 8 + 32), 64);
        assert_eq!(u32_at(&avi, avih + 8 + 36), 48);
        let strh = find(b"strh");
        assert_eq!(&avi[strh + 8..strh + 12], b"vids");
        assert_eq!(&avi[strh + 12..strh + 16], b"MJPG");
        assert_eq!(u32_at(&avi, strh + 8 + 20), 1, "dwScale");
        assert_eq!(u32_at(&avi, strh + 8 + 24), 60, "dwRate");
        assert_eq!(u32_at(&avi, strh + 8 + 32), 10, "dwLength");
        // The movi list: every chunk is `00dc`; three hold a frame, the rest repeat the last.
        let movi = find(b"movi");
        let list = movi - 8;
        assert_eq!(&avi[list..list + 4], b"LIST");
        let end = list + 8 + u32_at(&avi, list + 4) as usize;
        let (mut at, mut chunks, mut full) = (movi + 4, 0, 0);
        let mut idx_entries = Vec::new();
        while at < end {
            assert_eq!(&avi[at..at + 4], b"00dc");
            let n = u32_at(&avi, at + 4) as usize;
            idx_entries.push((at - movi, n));
            chunks += 1;
            if n > 0 {
                full += 1;
                assert_eq!(&avi[at + 8..at + 10], &[0xFF, 0xD8], "a frame's bytes");
            }
            at += 8 + n + (n & 1);
        }
        assert_eq!((chunks, full), (10, 3));
        assert_eq!(at, end);
        // idx1 follows with one entry per chunk.
        assert_eq!(&avi[end..end + 4], b"idx1");
        assert_eq!(u32_at(&avi, end + 4) as usize, 16 * 10);
        assert_eq!(end + 8 + 16 * 10, avi.len());
        for (i, (off, n)) in idx_entries.iter().enumerate() {
            let e = end + 8 + 16 * i;
            assert_eq!(&avi[e..e + 4], b"00dc");
            assert_eq!(u32_at(&avi, e + 8) as usize, *off);
            assert_eq!(u32_at(&avi, e + 12) as usize, *n);
        }
        // Nothing to save is nothing, and an over-large movie is refused.
        assert!(mjpeg_avi(&[], 64, 48).is_empty());
        assert!(mjpeg_avi(&frames, 0, 48).is_empty());
    }
}
