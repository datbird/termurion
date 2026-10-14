//! The session host's protocol: length-prefixed frames between Termurion and termurion-host, and
//! the per-session output ring the host keeps so a UI can reattach where it left off.

use std::collections::VecDeque;

/// The protocol this build speaks, and the oldest it still speaks. Version 2 adds password
/// prompt detection (`WatchPrompts`, `PasswordInput`) and console signals (`Signal`); both ends speak the lower of their two
/// versions, so a version 1 peer never sees those messages (an unknown message breaks the
/// stream). Version 3 hosts honor Launch's `env` (the session environment); the field was
/// always on the wire, and older hosts ignore it. Version 4 adds `LaunchWith` (a session on an
/// SSH transport, `ssh_transport::HOST_VERSION`). Version 5 adds `Goodbye`: a client that leaves
/// on purpose says so, so the host can tell it from a client that vanished.
pub const VERSION: u32 = 5;
pub const MIN_VERSION: u32 = 1;
/// A frame longer than this is refused (a broken or hostile peer).
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// A session the host runs, as the handshake reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub id: u64,
    pub pid: u32,
    pub cmdline: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<u32>,
    /// The output offset after the last byte the host holds.
    pub end: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Hello { version: u32 },
    Welcome { version: u32, host_pid: u32, instance: String, sessions: Vec<Live> },
    /// `busy`: another Termurion is attached (this one is a second copy).
    Refused { busy: bool, reason: String },
    Launch { tag: u64, cmdline: String, cwd: String, env: Vec<String>, cols: u16, rows: u16 },
    Launched { tag: u64, id: u64, pid: u32 },
    LaunchFailed { tag: u64, error: String },
    /// Start forwarding a session's output from `since`.
    Attach { id: u64, since: u64 },
    /// Output follows from `from`; `dropped` when bytes before it are gone.
    Attached { id: u64, from: u64, dropped: bool },
    Write { id: u64, bytes: Vec<u8> },
    Resize { id: u64, cols: u16, rows: u16 },
    Clear { id: u64 },
    Pause { id: u64, paused: bool },
    Output { id: u64, offset: u64, bytes: Vec<u8> },
    /// Bytes written to the session and not yet taken by it.
    Pending { id: u64, bytes: u64 },
    Exited { id: u64, code: u32 },
    /// The UI knows the session ended: the host forgets it.
    Forget { id: u64 },
    Kill { id: u64 },
    KillAll,
    /// Version 2, Termurion to the host: read each session's console input mode for password
    /// prompts, debounced by `debounce_ms`, or stop (None: nothing is attached to).
    WatchPrompts { debounce_ms: Option<u32> },
    /// Version 2, the host to Termurion: a session's console reached (or left) a password prompt,
    /// debounced. A host that never sends it leaves every session "not at a prompt".
    PasswordInput { id: u64, on: bool },
    /// Version 2, Termurion to the host: Send Signal's Ctrl+C (or Ctrl+Break) to a session's
    /// whole console. The host attaches (its consoles are its own to attach to).
    Signal { id: u64, ctrl_break: bool },
    /// Version 4, Termurion to the host: `Launch` on a transport (`ssh_transport::wire`). Version 4
    /// hosts understand it; a lower version never sees it (Termurion refuses the launch instead).
    LaunchWith { tag: u64, cmdline: String, cwd: String, env: Vec<String>, cols: u16, rows: u16, transport: u8 },
    /// Version 5, Termurion to the host: this client is leaving on purpose (quit or detach), so
    /// the pipe closing next is not a crash.
    Goodbye,
}

/// Field writers: integers little-endian, byte strings with a u32 length.
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend(v);
        self
    }
    fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.0.len() < n {
            return Err("a message is shorter than its fields".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Result<bool, String> {
        Ok(self.u8()? != 0)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|_| "a message holds text that is not UTF-8".to_string())
    }
    fn count(&mut self) -> Result<usize, String> {
        let n = self.u32()? as usize;
        // Every item takes at least one byte: a count past the rest is a lie.
        if n > self.0.len() {
            return Err("a message counts more items than it holds".into());
        }
        Ok(n)
    }
}

/// One message as a frame: its length (u32), then its tag and fields.
pub fn encode(m: &Msg) -> Vec<u8> {
    let mut w = W(Vec::new());
    match m {
        Msg::Hello { version } => w.u8(0).u32(*version),
        Msg::Welcome { version, host_pid, instance, sessions } => {
            w.u8(1).u32(*version).u32(*host_pid).str(instance).u32(sessions.len() as u32);
            for l in sessions {
                w.u64(l.id).u32(l.pid).str(&l.cmdline).str(&l.cwd).u16(l.cols).u16(l.rows);
                match l.exited {
                    Some(c) => w.u8(1).u32(c),
                    None => w.u8(0),
                };
                w.u64(l.end);
            }
            &mut w
        }
        Msg::Refused { busy, reason } => w.u8(2).u8(*busy as u8).str(reason),
        Msg::Launch { tag, cmdline, cwd, env, cols, rows } => {
            w.u8(3).u64(*tag).str(cmdline).str(cwd).u32(env.len() as u32);
            for e in env {
                w.str(e);
            }
            w.u16(*cols).u16(*rows)
        }
        Msg::Launched { tag, id, pid } => w.u8(4).u64(*tag).u64(*id).u32(*pid),
        Msg::LaunchFailed { tag, error } => w.u8(5).u64(*tag).str(error),
        Msg::Attach { id, since } => w.u8(6).u64(*id).u64(*since),
        Msg::Attached { id, from, dropped } => w.u8(7).u64(*id).u64(*from).u8(*dropped as u8),
        Msg::Write { id, bytes } => w.u8(8).u64(*id).bytes(bytes),
        Msg::Resize { id, cols, rows } => w.u8(9).u64(*id).u16(*cols).u16(*rows),
        Msg::Clear { id } => w.u8(10).u64(*id),
        Msg::Pause { id, paused } => w.u8(11).u64(*id).u8(*paused as u8),
        Msg::Output { id, offset, bytes } => w.u8(12).u64(*id).u64(*offset).bytes(bytes),
        Msg::Pending { id, bytes } => w.u8(13).u64(*id).u64(*bytes),
        Msg::Exited { id, code } => w.u8(14).u64(*id).u32(*code),
        Msg::Forget { id } => w.u8(15).u64(*id),
        Msg::Kill { id } => w.u8(16).u64(*id),
        Msg::KillAll => w.u8(17),
        Msg::WatchPrompts { debounce_ms } => match debounce_ms {
            Some(ms) => w.u8(18).u8(1).u32(*ms),
            None => w.u8(18).u8(0),
        },
        Msg::PasswordInput { id, on } => w.u8(19).u64(*id).u8(*on as u8),
        Msg::Signal { id, ctrl_break } => w.u8(20).u64(*id).u8(*ctrl_break as u8),
        Msg::LaunchWith { tag, cmdline, cwd, env, cols, rows, transport } => {
            w.u8(21).u64(*tag).str(cmdline).str(cwd).u32(env.len() as u32);
            for e in env {
                w.str(e);
            }
            w.u16(*cols).u16(*rows).u8(*transport)
        }
        Msg::Goodbye => w.u8(22),
    };
    let mut out = (w.0.len() as u32).to_le_bytes().to_vec();
    out.extend(w.0);
    out
}

fn decode(payload: &[u8]) -> Result<Msg, String> {
    let mut r = R(payload);
    let m = match r.u8()? {
        0 => Msg::Hello { version: r.u32()? },
        1 => {
            let (version, host_pid, instance) = (r.u32()?, r.u32()?, r.str()?);
            let n = r.count()?;
            let mut sessions = Vec::with_capacity(n);
            for _ in 0..n {
                let (id, pid, cmdline, cwd, cols, rows) = (r.u64()?, r.u32()?, r.str()?, r.str()?, r.u16()?, r.u16()?);
                let exited = if r.bool()? { Some(r.u32()?) } else { None };
                sessions.push(Live { id, pid, cmdline, cwd, cols, rows, exited, end: r.u64()? });
            }
            Msg::Welcome { version, host_pid, instance, sessions }
        }
        2 => Msg::Refused { busy: r.bool()?, reason: r.str()? },
        3 => {
            let (tag, cmdline, cwd) = (r.u64()?, r.str()?, r.str()?);
            let n = r.count()?;
            let env = (0..n).map(|_| r.str()).collect::<Result<Vec<_>, _>>()?;
            Msg::Launch { tag, cmdline, cwd, env, cols: r.u16()?, rows: r.u16()? }
        }
        4 => Msg::Launched { tag: r.u64()?, id: r.u64()?, pid: r.u32()? },
        5 => Msg::LaunchFailed { tag: r.u64()?, error: r.str()? },
        6 => Msg::Attach { id: r.u64()?, since: r.u64()? },
        7 => Msg::Attached { id: r.u64()?, from: r.u64()?, dropped: r.bool()? },
        8 => Msg::Write { id: r.u64()?, bytes: r.bytes()? },
        9 => Msg::Resize { id: r.u64()?, cols: r.u16()?, rows: r.u16()? },
        10 => Msg::Clear { id: r.u64()? },
        11 => Msg::Pause { id: r.u64()?, paused: r.bool()? },
        12 => Msg::Output { id: r.u64()?, offset: r.u64()?, bytes: r.bytes()? },
        13 => Msg::Pending { id: r.u64()?, bytes: r.u64()? },
        14 => Msg::Exited { id: r.u64()?, code: r.u32()? },
        15 => Msg::Forget { id: r.u64()? },
        16 => Msg::Kill { id: r.u64()? },
        17 => Msg::KillAll,
        18 => Msg::WatchPrompts { debounce_ms: if r.bool()? { Some(r.u32()?) } else { None } },
        19 => Msg::PasswordInput { id: r.u64()?, on: r.bool()? },
        20 => Msg::Signal { id: r.u64()?, ctrl_break: r.bool()? },
        21 => {
            let (tag, cmdline, cwd) = (r.u64()?, r.str()?, r.str()?);
            let n = r.count()?;
            let env = (0..n).map(|_| r.str()).collect::<Result<Vec<_>, _>>()?;
            Msg::LaunchWith { tag, cmdline, cwd, env, cols: r.u16()?, rows: r.u16()?, transport: r.u8()? }
        }
        22 => Msg::Goodbye,
        t => return Err(format!("unknown message {t}")),
    };
    if !r.0.is_empty() {
        return Err("a message has bytes after its fields".into());
    }
    Ok(m)
}

/// Splits a byte stream into messages.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    broken: bool,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        if !self.broken {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// The next whole message, None while one is incomplete. After an error the stream is
    /// broken and every later call gives the error again.
    pub fn next(&mut self) -> Option<Result<Msg, String>> {
        if self.broken {
            return Some(Err("the host connection is broken".into()));
        }
        let len = u32::from_le_bytes(self.buf.get(..4)?.try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            self.broken = true;
            self.buf = Vec::new();
            return Some(Err(format!("a message of {len} bytes is over the limit")));
        }
        if self.buf.len() < 4 + len {
            return None;
        }
        let m = decode(&self.buf[4..4 + len]);
        self.buf.drain(..4 + len);
        if m.is_err() {
            self.broken = true;
            self.buf = Vec::new();
        }
        Some(m)
    }
}

/// The output a session produced, the last `cap` bytes of it, with absolute offsets.
pub struct Ring {
    cap: usize,
    start: u64,
    buf: VecDeque<u8>,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring { cap, start: 0, buf: VecDeque::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes);
        let over = self.buf.len().saturating_sub(self.cap);
        if over > 0 {
            self.buf.drain(..over);
            self.start += over as u64;
        }
    }

    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// The bytes from `offset` on: (the offset they start at, whether bytes before them were
    /// dropped, the bytes).
    pub fn since(&self, offset: u64) -> (u64, bool, Vec<u8>) {
        if offset < self.start {
            return (self.start, true, self.buf.iter().copied().collect());
        }
        let skip = (offset - self.start).min(self.buf.len() as u64) as usize;
        (self.start + skip as u64, false, self.buf.iter().skip(skip).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Msg> {
        let live = Live { id: 7, pid: 1234, cmdline: "pwsh -NoLogo".into(), cwd: r"C:\src".into(), cols: 80, rows: 24, exited: Some(3), end: 99 };
        vec![
            Msg::Hello { version: 1 },
            Msg::Welcome { version: 1, host_pid: 42, instance: "abc".into(), sessions: vec![live.clone(), Live { exited: None, ..live }] },
            Msg::Refused { busy: true, reason: "another Termurion is attached".into() },
            Msg::Launch { tag: 5, cmdline: "cmd.exe".into(), cwd: String::new(), env: vec!["A=1".into(), "B=".into()], cols: 100, rows: 30 },
            Msg::Launch { tag: 6, cmdline: "pwsh.exe".into(), cwd: r"C:\src".into(), env: vec!["A=1".into(), "B=2".into()], cols: 80, rows: 24 },
            Msg::Launched { tag: 5, id: 9, pid: 77 },
            Msg::LaunchFailed { tag: 5, error: "no such file".into() },
            Msg::Attach { id: 9, since: 1 << 40 },
            Msg::Attached { id: 9, from: 10, dropped: true },
            Msg::Write { id: 9, bytes: b"ls\r\x00\xff".to_vec() },
            Msg::Resize { id: 9, cols: 120, rows: 40 },
            Msg::Clear { id: 9 },
            Msg::Pause { id: 9, paused: true },
            Msg::Output { id: 9, offset: 12, bytes: vec![] },
            Msg::Pending { id: 9, bytes: 4096 },
            Msg::Exited { id: 9, code: 0xC000013A },
            Msg::Forget { id: 9 },
            Msg::Kill { id: 9 },
            Msg::KillAll,
            Msg::WatchPrompts { debounce_ms: Some(200) },
            Msg::WatchPrompts { debounce_ms: None },
            Msg::PasswordInput { id: 9, on: true },
            Msg::PasswordInput { id: 9, on: false },
            Msg::Signal { id: 9, ctrl_break: false },
            Msg::Signal { id: 9, ctrl_break: true },
            Msg::LaunchWith { tag: 8, cmdline: "ssh.exe host".into(), cwd: String::new(), env: vec![], cols: 100, rows: 30, transport: 1 },
            Msg::Goodbye,
        ]
    }

    #[test]
    fn messages_round_trip() {
        // Version 4 hosts take LaunchWith, version 5 Goodbye; version 1 is still spoken.
        assert_eq!((VERSION, MIN_VERSION), (5, 1));
        let mut d = Decoder::default();
        for m in all() {
            d.push(&encode(&m));
        }
        for m in all() {
            assert_eq!(d.next(), Some(Ok(m)));
        }
        assert_eq!(d.next(), None);
    }

    #[test]
    fn launch_with_round_trips() {
        let m = Msg::LaunchWith { tag: 7, cmdline: "ssh.exe x".into(), cwd: "C:\\".into(), env: vec!["A=1".into()], cols: 80, rows: 24, transport: 2 };
        let mut d = Decoder::default();
        d.push(&encode(&m));
        assert_eq!(d.next(), Some(Ok(m)));
        assert_eq!(d.next(), None);
        assert_eq!((VERSION, MIN_VERSION), (5, 1));
    }

    #[test]
    fn partial_frames_wait() {
        let bytes: Vec<u8> = all().iter().flat_map(encode).collect();
        let mut d = Decoder::default();
        let mut got = Vec::new();
        for b in bytes {
            d.push(&[b]);
            while let Some(m) = d.next() {
                got.push(m.unwrap());
            }
        }
        assert_eq!(got, all());
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut d = Decoder::default();
        d.push(&((MAX_FRAME as u32) + 1).to_le_bytes());
        assert!(matches!(d.next(), Some(Err(_))));
        d.push(&encode(&Msg::KillAll));
        assert!(matches!(d.next(), Some(Err(_))), "the stream stays broken");
        let mut bad = Decoder::default();
        bad.push(&[1, 0, 0, 0, 250]);
        assert!(matches!(bad.next(), Some(Err(_))), "an unknown message");
        let mut short = Decoder::default();
        short.push(&[2, 0, 0, 0, 0, 1]);
        assert!(matches!(short.next(), Some(Err(_))), "a message cut short inside its frame");
    }

    #[test]
    fn ring_since_offsets() {
        let mut r = Ring::new(100);
        r.push(b"hello ");
        r.push(b"world");
        assert_eq!(r.end(), 11);
        assert_eq!(r.since(6), (6, false, b"world".to_vec()));
        assert_eq!(r.since(0), (0, false, b"hello world".to_vec()));
        assert_eq!(r.since(11), (11, false, vec![]));
        assert_eq!(r.since(50), (11, false, vec![]), "an offset past the end gives nothing");
    }

    #[test]
    fn ring_drops_the_oldest_and_says_so() {
        let mut r = Ring::new(8);
        r.push(b"0123456789");
        r.push(b"ab");
        assert_eq!(r.end(), 12);
        assert_eq!(r.since(0), (4, true, b"456789ab".to_vec()));
        assert_eq!(r.since(4), (4, false, b"456789ab".to_vec()), "nothing asked for was lost");
        assert_eq!(r.since(10), (10, false, b"ab".to_vec()));
    }
}
