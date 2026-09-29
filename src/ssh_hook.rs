//! SSH integration's entry (iTerm2's DCS 2000p hook): finds `ESC P 2000 p` in a pane's output
//! (across reads) and hands everything after it to the conductor parser until it unhooks.
//! Bytes before the start and after the unhook go to the screen.

use crate::conductor_parser::{Parser, Token, AUTOPOLL_PID, NOTIF_PID};
use crate::side_channel::Reducer;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub const START: &[u8] = b"\x1bP2000p";

/// The hook's name in iTerm2's DCS parser.
pub const HOOK_DESCRIPTION: &str = "[SSH CONDUCTOR]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ordinary output for the pane's screen.
    Screen(Vec<u8>),
    /// A conductor token and its depth (0 before the framer).
    Ssh(Token, i32),
    /// One piece of a background process's output (`SSH_SIDE_CHANNEL`): never for the screen.
    /// `depth` is the depth of the hook that read the block.
    Side { text: String, pid: i32, channel: i32, depth: i32 },
    /// The raw bytes of a depth 0, channel -1 block, for the coprocess (`VP:405`).
    Raw(Vec<u8>),
}

/// How many remote processes the hooks at one depth follow at once (F13). A remote can name
/// any pid, at every level; past this the hook that asks lets go rather than keep a parser for
/// each.
pub const MAX_CHILDREN: usize = 256;

/// The deepest a hook nests (Tandem's guard, iTerm2 has none): a remote's cat'd nested DCS
/// 2000p and `%output` could otherwise recurse thousands of levels in one block and overflow
/// the stack. A hook deeper than this never hooks: what it is given is plain output.
pub const MAX_DEPTH: i32 = 8;

/// The depths that can hook, each with its own budget.
const LEVELS: usize = MAX_DEPTH as usize + 1;

/// The most bytes the parsers at one depth hold unfinished at once (an unfinished init line,
/// OSC or `%output` block), all hooks at that depth together.
pub const MAX_PENDING: usize = 16 * 1024 * 1024;

/// Why the hook let go past `MAX_PENDING` (Tandem's).
pub const TOO_MUCH_PENDING: &str = "SSH integration stopped: the remote host sent too much unfinished output";

/// Why the hook let go past `MAX_CHILDREN` (Tandem's).
pub const TOO_MANY_PROCESSES: &str = "SSH integration stopped: the remote host started too many processes";

/// Why the conductor ends when its parser lets go without saying so (Tandem's): a cap, an
/// unknown word, output that is not a reply. Without it the pane would go on framing keys
/// into a session whose output is no longer read.
pub const LOST_OUTPUT: &str = "SSH integration lost the remote's output";

/// A fresh parser id (iTerm2 gives each DCS 2000p hook a new UUID).
pub fn new_dcs_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// What the hooks at one depth hold (F13): the child hooks they made, the unfinished bytes
/// their parsers hold, and which of them owns a conductor (0: none).
#[derive(Default)]
struct Level {
    children: AtomicUsize,
    pending: AtomicUsize,
    owner: AtomicU64,
}

/// What a whole hook tree shares. Each depth has its own budget, so hooks at one depth never
/// refuse a shallower one: a cat'd file deep in a nested session cannot end the session
/// around it. The root is the only hook at depth 0, so its budget is its own.
#[derive(Default)]
struct Tree {
    levels: [Level; LEVELS],
    /// The last hook serial given out (the root is 1).
    serial: AtomicU64,
}

impl Tree {
    fn level(&self, depth: i32) -> Option<&Level> {
        usize::try_from(depth).ok().and_then(|d| self.levels.get(d))
    }

    fn next_serial(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed) + 2
    }
}

/// One child hook's place in its maker's depth budget, given back when the child is dropped,
/// however that happens.
struct Slot {
    tree: Arc<Tree>,
    depth: i32,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(level) = self.tree.level(self.depth) {
            level.children.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// The unfinished bytes one hook's parser holds, counted in its depth's budget and given back
/// when the hook is dropped.
#[derive(Default)]
struct Charge {
    tree: Arc<Tree>,
    depth: i32,
    bytes: usize,
}

impl Charge {
    /// Sets what this hook holds; false when it asked for more and its depth now holds more
    /// than `MAX_PENDING` (then it is the one refused, whoever else holds the rest).
    fn set(&mut self, bytes: usize) -> bool {
        let Some(level) = self.tree.level(self.depth) else { return bytes == 0 };
        let grew = bytes > self.bytes;
        if grew {
            level.pending.fetch_add(bytes - self.bytes, Ordering::Relaxed);
        } else {
            level.pending.fetch_sub(self.bytes - bytes, Ordering::Relaxed);
        }
        self.bytes = bytes;
        !grew || level.pending.load(Ordering::Relaxed) <= MAX_PENDING
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.set(0);
    }
}

/// A hook's claim to own the conductor of its depth, given back when its parser ends or the
/// hook is dropped.
struct Owner {
    tree: Arc<Tree>,
    depth: i32,
    serial: u64,
}

impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(level) = self.tree.level(self.depth) {
            let _ = level.owner.compare_exchange(self.serial, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
}

/// The parser of one remote process (`VP:181-275`), and its side channel's reducer.
struct Child {
    hook: Hook,
    reducer: Reducer,
    _slot: Slot,
}

/// The hook tree's rules (Tandem's guards around `VP:181-275`):
/// - A hook at depth d reads the output of one remote process of the hook at d - 1; hooks
///   past `MAX_DEPTH` never hook.
/// - At most one hook per depth owns a conductor: the one whose parser read an init line, on
///   a path of main (channel -1) outputs from the root. Another one's init line at that depth
///   hooks nothing and says nothing (the rest of that read is lost), and a side channel's
///   hook never owns (its conductor tokens are dropped, `T:1776-1805`). So the owners form one
///   chain from the root, and the app gives each its conductor at the same depth.
/// - A hook's children belong to its current parser: when the parser ends, however it ends,
///   they go, and with them every deeper hook under it.
/// - Each depth has its own budget (`MAX_CHILDREN` hooks made, `MAX_PENDING` bytes held); the
///   hook that asks past it is the one that lets go, with an error at its own depth only when
///   it owns a conductor. The root, alone at depth 0, answers only to its own caps.
pub struct Hook {
    /// 0 for a pane's own hook, one more for each nested hook (a remote ssh inside a framed
    /// session).
    depth: i32,
    /// Whether this hook may own its depth's conductor: the root, and a main output's child
    /// of one that may.
    may_own: bool,
    /// This hook's id in the tree (the root is 1).
    serial: u64,
    /// Held while this hook's parser owns its depth's conductor.
    owner: Option<Owner>,
    /// One parser per remote pid, at depth + 1.
    children: HashMap<i32, Box<Child>>,
    /// The first pid that wrote to the screen with no other process known (`VP:181-275`).
    main_pid: Option<i32>,
    /// The end of the last read that may begin `ESC P 2000 p`.
    held: Vec<u8>,
    parser: Option<Parser>,
    /// What the parser holds unfinished, in its depth's budget.
    charge: Charge,
    /// Where parser ids come from (None: `new_dcs_id`).
    ids: Option<fn() -> String>,
}

impl Default for Hook {
    fn default() -> Hook {
        Hook { depth: 0, may_own: true, serial: 1, owner: None, children: HashMap::new(), main_pid: None, held: Vec::new(), parser: None, charge: Charge::default(), ids: None }
    }
}

impl Hook {
    /// A hook whose parsers take their ids from `next` (tests).
    pub fn with_ids(next: fn() -> String) -> Hook {
        Hook { ids: Some(next), ..Default::default() }
    }

    /// A hook at `depth`: what a child of a hook at `depth - 1` is.
    pub fn with_depth(depth: i32) -> Hook {
        let mut hook = Hook::default();
        hook.depth = depth;
        hook.charge.depth = depth;
        hook
    }

    /// The pids that have a child parser now, sorted.
    pub fn child_pids(&self) -> Vec<i32> {
        let mut pids: Vec<i32> = self.children.keys().copied().collect();
        pids.sort_unstable();
        pids
    }

    pub fn active(&self) -> bool {
        self.parser.is_some()
    }

    /// Leaves the conductor without an `unhook` (iTerm2's forced unhook): what follows is
    /// output again. The parser's unparsed bytes (a partial init line or OSC) go with it.
    pub fn unhook(&mut self) {
        self.end_parser();
    }

    /// The parser ends: its unfinished bytes, its claim on the depth's conductor and its
    /// children go with it.
    fn end_parser(&mut self) {
        self.parser = None;
        self.charge.set(0);
        self.owner = None;
        self.drop_children();
    }

    /// Lets go of the hook that owns the conductor at `depth`, and so of every hook under it
    /// (a failed or refused conductor: iTerm2 never force-unhooks its parser). The hooks above
    /// it and the other processes of its parent stay. False when no hook owns that depth.
    pub fn let_go_owner(&mut self, depth: i32) -> bool {
        if depth == self.depth {
            if self.owner.is_none() {
                return false;
            }
            self.end_parser();
            return true;
        }
        depth > self.depth && self.children.values_mut().any(|child| child.hook.let_go_owner(depth))
    }

    /// Whether a hook owns the conductor at `depth`.
    pub fn owned(&self, depth: i32) -> bool {
        self.charge.tree.level(depth).is_some_and(|l| l.owner.load(Ordering::Relaxed) != 0)
    }

    /// How many child hooks the whole tree holds.
    pub fn tree_children(&self) -> usize {
        self.charge.tree.levels.iter().map(|l| l.children.load(Ordering::Relaxed)).sum()
    }

    /// The child hooks the hooks at `depth` made (that depth's budget in use).
    pub fn children_at(&self, depth: i32) -> usize {
        self.charge.tree.level(depth).map_or(0, |l| l.children.load(Ordering::Relaxed))
    }

    /// The unfinished parser bytes the whole tree holds.
    pub fn tree_pending(&self) -> usize {
        self.charge.tree.levels.iter().map(|l| l.pending.load(Ordering::Relaxed)).sum()
    }

    /// The unfinished parser bytes the hooks at `depth` hold.
    pub fn pending_at(&self, depth: i32) -> usize {
        self.charge.tree.level(depth).map_or(0, |l| l.pending.load(Ordering::Relaxed))
    }

    fn drop_children(&mut self) {
        self.children.clear();
        self.main_pid = None;
    }

    /// Claims this depth's conductor for the parser that just read its init line. False when
    /// another hook at this depth owns it. A hook that may not own claims nothing.
    fn claim(&mut self) -> bool {
        if !self.may_own {
            return true;
        }
        let Some(level) = self.charge.tree.level(self.depth) else { return false };
        if level.owner.compare_exchange(0, self.serial, Ordering::Relaxed, Ordering::Relaxed).is_err() {
            return false;
        }
        self.owner = Some(Owner { tree: self.charge.tree.clone(), depth: self.depth, serial: self.serial });
        true
    }

    /// The tail of the last read kept because it may begin `ESC P 2000 p`, handed back for
    /// output that no longer goes through the hook (tmux control mode began before it).
    pub fn take_held(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }

    /// A `%output` block (`VP:181-275`): its data goes to the child parser of its pid (made
    /// on first use, at depth + 1), and what the child finds goes up. False when this depth
    /// may make no more children.
    fn output(&mut self, pid: i32, channel: i32, data: Vec<u8>, out: &mut Vec<Out>) -> bool {
        if !self.children.contains_key(&pid) {
            let tree = self.charge.tree.clone();
            let Some(level) = tree.level(self.depth) else { return false };
            if level.children.load(Ordering::Relaxed) >= MAX_CHILDREN {
                return false;
            }
            if self.children.is_empty() && channel == -1 {
                self.main_pid = Some(pid);
            }
            level.children.fetch_add(1, Ordering::Relaxed);
            let slot = Slot { tree: tree.clone(), depth: self.depth };
            let depth = self.depth + 1;
            let hook = Hook {
                depth,
                may_own: self.may_own && channel == -1,
                serial: tree.next_serial(),
                ids: self.ids,
                charge: Charge { tree: tree.clone(), depth, bytes: 0 },
                ..Default::default()
            };
            self.children.insert(pid, Box::new(Child { hook, reducer: Reducer::default(), _slot: slot }));
        }
        let Some(child) = self.children.get_mut(&pid) else { return true };
        let mut pieces = Vec::new();
        for o in child.hook.feed(&data) {
            match o {
                Out::Screen(bytes) if channel == -1 => screen(out, &bytes),
                Out::Screen(bytes) => pieces.extend(child.reducer.feed(&bytes)),
                // A side channel keeps only text (`T:1776-1805`).
                Out::Ssh(..) if channel != -1 => {}
                other => {
                    flush_side(&mut pieces, pid, channel, self.depth, out);
                    out.push(other);
                }
            }
        }
        flush_side(&mut pieces, pid, channel, self.depth, out);
        // Autopoll and notification parsers keep nothing between blocks.
        if pid == AUTOPOLL_PID || pid == NOTIF_PID {
            self.children.remove(&pid);
        }
        // Only depth 0 keeps the block for the coprocess.
        if self.depth == 0 && channel == -1 {
            out.push(Out::Raw(data));
        }
        true
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Out> {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if let Some(parser) = &mut self.parser {
                let past_init = parser.past_init();
                let fed = parser.feed(&data);
                let pending = if fed.rest.is_some() { 0 } else { parser.pending_bytes() };
                let hooked_now = !past_init && fed.tokens.iter().any(|t| matches!(t, Token::Init(_)));
                if hooked_now && !self.claim() {
                    // Another hook at this depth owns its conductor: this one hooks nothing,
                    // and what its parser read is lost with it.
                    self.end_parser();
                    match fed.rest {
                        None => break,
                        Some(rest) => {
                            data = rest;
                            continue;
                        }
                    }
                }
                // A parser that unhooks with no word of its own gets one here (F28), unless it
                // never read its init line: it has no conductor, and the error would fail the
                // pane's existing one (a cat'd file can hold a broken DCS 2000p).
                let owned = past_init || hooked_now;
                let silent = owned && fed.rest.is_some() && !matches!(fed.tokens.last(), Some(Token::Unhook | Token::Error(_)));
                let mut capped = false;
                for token in fed.tokens {
                    match token {
                        Token::Output { pid, channel, data } => {
                            if !self.output(pid, channel, data, &mut out) {
                                capped = true;
                                break;
                            }
                        }
                        Token::Terminate { pid, rc } => {
                            self.children.remove(&pid);
                            if self.main_pid == Some(pid) {
                                self.main_pid = None;
                            }
                            out.push(Out::Ssh(Token::Terminate { pid, rc }, self.depth));
                        }
                        Token::Unhook => {
                            self.drop_children();
                            out.push(Out::Ssh(Token::Unhook, self.depth));
                        }
                        token => out.push(Out::Ssh(token, self.depth)),
                    }
                }
                if capped {
                    // The rest of the read is lost with the parser; the pane's screen gets
                    // nothing of it (it is a remote's flood).
                    out.push(Out::Ssh(Token::Error(TOO_MANY_PROCESSES.into()), self.depth));
                    self.end_parser();
                    break;
                }
                if silent {
                    out.push(Out::Ssh(Token::Error(LOST_OUTPUT.into()), self.depth));
                }
                if !self.charge.set(pending) {
                    // This depth holds too much unfinished output and this parser asked for
                    // more: it lets go, with what it held.
                    if owned {
                        out.push(Out::Ssh(Token::Error(TOO_MUCH_PENDING.into()), self.depth));
                    }
                    self.end_parser();
                    break;
                }
                match fed.rest {
                    None => break,
                    Some(rest) => {
                        self.end_parser();
                        data = rest;
                    }
                }
                continue;
            }
            // Past the depth cap nothing hooks: the bytes are plain output.
            let found = if self.depth > MAX_DEPTH { None } else { data.windows(START.len()).position(|w| w == START) };
            match found {
                Some(k) => {
                    screen(&mut out, &data[..k]);
                    let id = self.ids.map_or_else(new_dcs_id, |next| next());
                    self.parser = Some(Parser::new(&id));
                    data.drain(..k + START.len());
                }
                None => {
                    // A tail that may begin the start sequence waits for the next read.
                    let keep = if self.depth > MAX_DEPTH { 0 } else { (1..START.len()).rev().find(|n| data.ends_with(&START[..*n])).unwrap_or(0) };
                    screen(&mut out, &data[..data.len() - keep]);
                    self.held = data[data.len() - keep..].to_vec();
                    break;
                }
            }
        }
        out
    }
}

/// What the app keeps while it runs one batch of a pane's hook output: after it refused a
/// conductor at some depth (its depth did not follow the pane's conductors), everything left
/// in the batch from that depth or deeper came from the refused hook, so it is dropped.
#[derive(Default)]
pub struct Batch {
    refused: Option<i32>,
}

impl Batch {
    /// Whether `out` is dropped: a conductor token or side channel piece from a refused depth.
    pub fn drops(&self, out: &Out) -> bool {
        let depth = match out {
            Out::Ssh(_, depth) | Out::Side { depth, .. } => *depth,
            Out::Screen(_) | Out::Raw(_) => return false,
        };
        self.refused.is_some_and(|r| depth >= r)
    }

    /// The conductor an init line at `depth` asked for was made (`true`) or refused.
    pub fn hooked(&mut self, depth: i32, made: bool) {
        if made {
            // A shallower hook's new conductor: the refused hooks were under a parser that
            // ended before it, so nothing of theirs follows.
            if self.refused.is_some_and(|r| depth < r) {
                self.refused = None;
            }
        } else {
            self.refused = Some(self.refused.map_or(depth, |r| r.min(depth)));
        }
    }
}

/// Hands the reduced pieces on. A job's channel 1 is joined into one piece (a conductor
/// joins them with nothing, so the result is the same and a stream of newlines costs one
/// piece); autopoll and notif keep every piece, since autopoll ends on a piece that is exactly
/// "EOF" (`CD:2601`), and another channel's pieces each complete a job.
fn flush_side(pieces: &mut Vec<String>, pid: i32, channel: i32, depth: i32, out: &mut Vec<Out>) {
    if channel == 1 && pid != AUTOPOLL_PID && pid != NOTIF_PID {
        if !pieces.is_empty() {
            out.push(Out::Side { text: pieces.drain(..).collect(), pid, channel, depth });
        }
        return;
    }
    out.extend(pieces.drain(..).map(|text| Out::Side { text, pid, channel, depth }));
}

/// Adds screen bytes, joined to screen bytes just before them.
fn screen(out: &mut Vec<Out>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(Out::Screen(last)) = out.last_mut() {
        last.extend_from_slice(bytes);
    } else {
        out.push(Out::Screen(bytes.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor_parser::{parse_end, parse_init, Init};

    fn hook() -> Hook {
        Hook::with_ids(|| "D".into())
    }

    fn s(b: &[u8]) -> Out {
        Out::Screen(b.to_vec())
    }

    fn ssh(t: Token) -> Out {
        Out::Ssh(t, 0)
    }

    fn begin(id: &str) -> Out {
        ssh(Token::Begin(id.into()))
    }

    fn line(text: &str) -> Out {
        ssh(Token::Line(text.into()))
    }

    fn end(id: &str, status: u8, kind: &str) -> Out {
        ssh(Token::End { id: id.into(), status, kind: kind.into() })
    }

    fn init(token: &str, unique_id: &str, bool_args: &str, sshargs: &str) -> Out {
        ssh(Token::Init(Init {
            token: token.into(),
            unique_id: unique_id.into(),
            bool_args: bool_args.into(),
            sshargs: sshargs.into(),
            dcs_id: "D".into(),
        }))
    }

    const INIT: &[u8] = b"\x1bP2000pnone 1 YQ== - Yg==\n";

    /// A hook past its init line, in ground.
    fn hooked() -> Hook {
        let mut h = hook();
        assert_eq!(h.feed(INIT), vec![init("none", "1", "a", "b")]);
        h
    }

    /// A hook inside the reply to command 1.
    fn in_body() -> Hook {
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:begin 1\x1b\\"), vec![begin("1")]);
        h
    }

    #[test]
    fn hook_finds_the_start_follows_iterm2() {
        let mut h = hook();
        let out = h.feed(b"hi\x1bP2000pnone 12345 YmE= - c3NoIGFyZ3M=\n");
        assert_eq!(out, vec![s(b"hi"), init("none", "12345", "ba", "ssh args")]);
        assert!(h.active());
        assert_eq!(hook().feed(b"x\x1bP1000py"), vec![s(b"x\x1bP1000py")], "tmux's DCS is output here");
        assert_eq!(hook().feed(b"\x1bP2001p"), vec![s(b"\x1bP2001p")], "another DCS is output");
        assert_eq!(HOOK_DESCRIPTION, "[SSH CONDUCTOR]");
        let id = new_dcs_id();
        assert_eq!((id.len(), id.to_uppercase()), (36, id.clone()));
        assert_ne!(new_dcs_id(), new_dcs_id());
    }

    #[test]
    fn hook_survives_chunk_splits() {
        let mut h = hook();
        assert_eq!(h.feed(b"ab\x1bP20"), vec![s(b"ab")], "a possible start waits");
        assert_eq!(h.feed(b"00pnone 1 YQ== - Yg==\n"), vec![init("none", "1", "a", "b")]);
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2"), vec![]);
        assert_eq!(h.feed(b"x"), vec![s(b"\x1bP2x")], "a false start goes to the screen whole");
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000pnone 1 YQ=="), vec![]);
        assert_eq!(h.feed(b" - Yg=="), vec![], "the init line waits for its newline");
        assert_eq!(h.feed(b"\n"), vec![init("none", "1", "a", "b")]);
        assert_eq!(h.feed(b"\x1b]135;:begin 1\x1b"), vec![]);
        assert_eq!(h.feed(b"\\"), vec![begin("1")]);
        assert_eq!(h.feed(b"\x1b]135;:first\nsec"), vec![]);
        assert_eq!(h.feed(b"ond\x1b"), vec![]);
        assert_eq!(h.feed(b"\\"), vec![line("first\nsecond")]);
        assert_eq!(h.feed(b"\x1b]135;:end 1 0 r\x1b\\\x1b]135;:unh"), vec![end("1", 0, "r")]);
        assert_eq!(h.feed(b"ook\x1b\\\x1bP20"), vec![ssh(Token::Unhook)]);
        assert_eq!(h.feed(b"0"), vec![]);
        assert_eq!(h.feed(b"1"), vec![s(b"\x1bP2001")], "after the unhook the finder runs again");
    }

    #[test]
    fn init_rules_follow_iterm2() {
        let got = parse_init("t u YmE= x y - c3M=", "D").unwrap();
        assert_eq!(got, Init { token: "t".into(), unique_id: "u".into(), bool_args: "ba".into(), sshargs: "ss".into(), dcs_id: "D".into() });
        assert_eq!(parse_init("t u -", "D"), None, "fewer than 5 words with the id");
        assert_eq!(parse_init("t u YmE= c3M=", "D"), None, "no separator");
        assert_eq!(parse_init("t u Y*E= - c3M=", "D"), None, "bad boolean args");
        assert_eq!(parse_init("t u YmE= - c3M", "D"), None, "bad ssh args");
        assert_eq!(parse_init("t u YmE= -", "D").unwrap().sshargs, "", "the id counts as a word");
        assert_eq!(parse_init("none 1 YmE= - c3M=", "D").unwrap().token, "none");
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000p\xff\xfe - Yg==\nls"), vec![s(b"\nls")], "a non-UTF-8 line is dropped, silently");
        assert!(!h.active());
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000pbad\nls\r\n"), vec![s(b"\nls\r\n")], "a rejected line unhooks, silently");
        assert!(!h.active());
    }

    #[test]
    fn osc_reader_follows_iterm2() {
        assert_eq!(hooked().feed(b"\x1b]135;:begin 7\x1b\\"), vec![begin("7")]);
        assert_eq!(hooked().feed(b"\x1b]135;junk:begin 7\x1b\\"), vec![begin("7")], "text before the colon is ignored");
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;begin 7\x1b\\"), vec![lost()], "no colon: an empty payload unhooks");
        assert!(!h.active());
        assert_eq!(hooked().feed(b"\x1b]135;:begin 7\x07\x1b\\"), vec![begin("7\u{7}")], "BEL does not end an OSC");
        assert_eq!(hooked().feed(b"\x1b]135;:beg\x1b]135;:begin 9\x1b\\"), vec![begin("9")], "an interrupted OSC restarts");
        let mut h = hooked();
        assert_eq!(h.feed(b"\n\x1b[31mjunk\x1b]7;:x\x1b\\\x1b]135;:begin 1\x1b\\"), vec![begin("1")], "ground drops garbage");
        assert!(h.active());
    }

    #[test]
    fn body_follows_iterm2() {
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:a\nb\x1b\\"), vec![line("a\nb")]);
        assert_eq!(h.feed(b"\x1b]135;:end 2 0 r\x1b\\"), vec![line("end 2 0 r")], "another id is a line");
        assert_eq!(h.feed(b"\x1b]134;:end 1 0 r\x1b\\"), vec![end("1", 0, "r")]);
        assert_eq!(h.feed(b"\x1b]135;:begin 2\x1b\\"), vec![begin("2")], "ground again");
        let mut h = in_body();
        assert_eq!(h.feed(b"oops"), vec![lost(), s(b"oops")]);
        assert!(!h.active());
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]7;:x\x1b\\"), vec![lost(), s(b"\x1b]7;:x\x1b\\")], "another OSC is output");
        assert!(!h.active());
    }

    #[test]
    fn end_rules_follow_iterm2() {
        assert_eq!(parse_end("1 0 r"), Some(("1".into(), 0, "r".into())));
        assert_eq!(parse_end("1 r 0"), Some(("1".into(), 0, "0".into())), "the python bootstrap's swapped fields");
        assert_eq!(parse_end("1 07 r"), Some(("1".into(), 7, "r".into())));
        assert_eq!(parse_end("1 7x r"), Some(("1".into(), 7, "r".into())));
        assert_eq!(parse_end("1 256 r"), None);
        assert_eq!(parse_end("1 99999999999999999999999 r"), None);
        assert_eq!(parse_end("1 0"), None);
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:end 1 256 r\x1b\\"), vec![]);
        assert_eq!(h.feed(b"\x1b]135;:begin 2\x1b\\"), vec![begin("2")], "a dropped end still returns to ground");
    }

    #[test]
    fn unhook_follows_iterm2() {
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:unhook\x1b\\$ "), vec![ssh(Token::Unhook), s(b"$ ")]);
        assert!(!h.active());
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:what\x1b\\x"), vec![lost(), s(b"x")], "an unknown word unhooks");
        assert!(!h.active());
        let mut h = hooked();
        let out = h.feed(b"\x1b]135;:unhook\x1b\\x\x1bP2000pnone 1 YQ== - Yg==\n");
        assert_eq!(out, vec![ssh(Token::Unhook), s(b"x"), init("none", "1", "a", "b")]);
        assert!(h.active());
        let mut h = in_body();
        h.unhook();
        assert!(!h.active());
        assert_eq!(h.feed(b"\x1b]135;:end 1 0 r\x1b\\$ "), vec![s(b"\x1b]135;:end 1 0 r\x1b\\$ ")]);
    }

    #[test]
    fn framer_ground_follows_iterm2() {
        assert_eq!(hooked().feed(b"\x1b]134;:begin 5\x1b\\"), vec![begin("5")]);
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]134;:%ping\x1b\\"), vec![]);
        assert!(h.active(), "an unknown notification is ignored");
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]134;:nonsense\x1b\\z"), vec![lost(), s(b"z")]);
        assert!(!h.active());
    }

    #[test]
    fn payloads_decode_as_utf8() {
        assert_eq!(in_body().feed(b"\x1b]135;:caf\xc3\xa9\x1b\\"), vec![line("caf\u{e9}")]);
        assert_eq!(in_body().feed(b"\x1b]135;:caf\xff\x1b\\"), vec![line("caf\u{fffd}")]);
        // A character split across reads is whole.
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:caf\xc3"), vec![]);
        assert_eq!(h.feed(b"\xa9\x1b\\"), vec![line("caf\u{e9}")]);
    }

    /// Feeds `bytes` in chunks of `size` and returns what came out, screen output joined.
    fn trickle(h: &mut Hook, bytes: &[u8], size: usize) -> Vec<Out> {
        let mut out = Vec::new();
        for chunk in bytes.chunks(size) {
            for o in h.feed(chunk) {
                match o {
                    Out::Screen(b) => screen(&mut out, &b),
                    o => out.push(o),
                }
            }
        }
        out
    }

    #[test]
    fn held_input_is_capped() {
        use crate::conductor_parser::MAX_HELD;
        let flood = vec![b'a'; MAX_HELD];
        // An init line that never ends: everything after the start comes back as output.
        let mut h = hook();
        assert_eq!(h.feed(START), vec![]);
        assert_eq!(trickle(&mut h, &flood, 4096), vec![], "up to the cap it waits");
        assert!(h.active());
        assert_eq!(h.feed(b"a"), vec![s(&vec![b'a'; MAX_HELD + 1])], "an init line over the cap is not a protocol error");
        assert!(!h.active());
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000p"), vec![]);
        let out = trickle(&mut h, &[&flood[..], b"a"].concat(), 4095);
        assert_eq!(out, vec![s(&[&flood[..], b"a"].concat())]);
        assert!(!h.active());
        assert_eq!(h.feed(b"\nls"), vec![s(b"\nls")], "the newline after it is output too");
        // A line that ends inside the read that passes the cap is a line (here a rejected one).
        let mut h = hook();
        assert_eq!(h.feed(START), vec![]);
        assert_eq!(trickle(&mut h, &[&flood[..], b"a\nls"].concat(), MAX_HELD), vec![s(b"\nls")]);
        // An OSC that never ends, in ground and in a reply.
        for mut h in [hooked(), in_body()] {
            let osc = [&b"\x1b]135;:"[..], &flood[..]].concat();
            let out = trickle(&mut h, &osc, 4096);
            assert_eq!(out, vec![lost(), s(&osc)]);
            assert!(!h.active());
            assert_eq!(h.feed(b"$ "), vec![s(b"$ ")]);
        }
        // A whole stream bigger than the cap in one read is parsed, not cut.
        let big = [&b"\x1b]135;:"[..], &flood[..], b"b\x1b\\"].concat();
        let mut h = in_body();
        assert_eq!(h.feed(&big), vec![line(&format!("{}b", "a".repeat(MAX_HELD)))]);
        assert!(h.active());
    }

    #[test]
    fn scans_resume_at_every_split() {
        let stream: &[u8] = b"x\x1bP2000pnone 1 YQ== - Yg==\n\x1b[1mj\x1b]7;:q\x1b\\\x1b]135;:beg\x1b]135;:begin 1\x1b\\\x1b]135;:a\x1b\\\x1b]135;:caf\xc3\xa9\nz\x1b\\\x1b]134;:end 1 0 r\x1b\\zz\x1b[0m\x1b]135;:unhook\x1b\\$ ";
        let whole = trickle(&mut hook(), stream, stream.len());
        for k in 0..=stream.len() {
            let mut h = hook();
            let mut out = trickle(&mut h, &stream[..k], stream.len().max(1));
            for o in trickle(&mut h, &stream[k..], stream.len().max(1)) {
                match o {
                    Out::Screen(b) => screen(&mut out, &b),
                    o => out.push(o),
                }
            }
            assert_eq!(out, whole, "split at {k}");
        }
        for size in 1..8 {
            assert_eq!(trickle(&mut hook(), stream, size), whole, "chunks of {size}");
        }
        assert_eq!(tokens(&whole).last(), Some(&&Token::Unhook));
    }

    /// Feeds a recording in chunks, joining screen output across reads.
    fn run(bytes: &[u8], size: usize) -> Vec<Out> {
        let mut h = hook();
        let mut out = Vec::new();
        for chunk in bytes.chunks(size) {
            for o in h.feed(chunk) {
                match o {
                    Out::Screen(b) => screen(&mut out, &b),
                    o => out.push(o),
                }
            }
        }
        out
    }

    fn tokens(out: &[Out]) -> Vec<&Token> {
        out.iter().filter_map(|o| if let Out::Ssh(t, _) = o { Some(t) } else { None }).collect()
    }

    fn lines<'a>(out: &'a [Out]) -> Vec<&'a str> {
        tokens(out).into_iter().filter_map(|t| if let Token::Line(l) = t { Some(l.as_str()) } else { None }).collect()
    }

    #[test]
    fn recordings_parse() {
        let mut all = Vec::new();
        for name in ["level1-nopython", "python36", "python312", "quirks"] {
            let bytes = std::fs::read(format!("tests/fixtures/ssh/{name}.bin")).unwrap();
            let whole = run(&bytes, bytes.len());
            for size in [1, 2, 7, 64] {
                assert_eq!(run(&bytes, size), whole, "{name} in chunks of {size}");
            }
            all.push(whole);
        }
        // level1-nopython: getshell, setenv, write, cd, pythonversion, exec_login_shell.
        let out = &all[0];
        let t = tokens(out);
        assert_eq!(
            t[0],
            &Token::Init(Init {
                token: "none".into(),
                unique_id: "12345".into(),
                bool_args: "46AaCfGgKkMNnqsTtVvXxYy".into(),
                sshargs: "example.com ".into(),
                dcs_id: "D".into(),
            })
        );
        // Six commands, each a begin and an end with the same id.
        let mut open = None;
        let mut pairs = 0;
        for token in &t[1..] {
            match token {
                Token::Begin(id) => open = Some(id),
                Token::End { id, status: 0, kind } if Some(id) == open && kind == "r" => pairs += 1,
                _ => {}
            }
        }
        assert_eq!(pairs, 6);
        assert_eq!(t.iter().filter(|t| matches!(t, Token::Begin(_))).count(), 6);
        assert!(matches!(t[1], Token::Begin(_)));
        // With no getent, id, python or perl, conductor.sh calls its undefined using_passwd
        // and the shell's complaint lands inside getshell's reply, before the shell.
        let getshell: Vec<&str> = lines(out)[0].split('\n').collect();
        assert!(getshell[0].ends_with("using_passwd: not found"), "{getshell:?}");
        assert_eq!(getshell[1..], ["/bin/sh", "/home/user", ""]);
        assert!(matches!(t[2], Token::Line(_)) && matches!(t[3], Token::End { .. }));
        assert_eq!(t[t.len() - 1], &Token::Unhook);
        assert!(matches!(out.last(), Some(Out::Screen(b)) if String::from_utf8_lossy(b).contains("hi\r\n")));
        assert!(lines(&all[1]).iter().any(|l| l.starts_with("Python 3.6.9")));
        let t = tokens(&all[2]);
        assert!(lines(&all[2]).iter().any(|l| l.starts_with("Python 3.12.1")));
        assert!(matches!(t[t.len() - 2..], [Token::Begin(_), Token::End { .. }]), "ends with the quit pair");
        let t = tokens(&all[3]);
        assert!(lines(&all[3]).contains(&"bad command nosuch"));
        let ends: Vec<u8> = t.iter().filter_map(|t| if let Token::End { status, .. } = t { Some(*status) } else { None }).collect();
        assert_eq!(ends, [0, 0, 0, 0], "every status is 0 (iTerm2 bug B1)");
    }

    fn lost() -> Out {
        ssh(Token::Error(LOST_OUTPUT.into()))
    }

    #[test]
    fn a_silent_unhook_is_an_error() {
        use crate::ssh_fixtures::osc134;
        use crate::conductor_parser::MAX_HELD;
        // Garbage in ground, and an unknown OSC 134 word.
        let mut h = hooked();
        assert_eq!(h.feed(&[osc134("nonsense"), b"z".to_vec()].concat()), vec![lost(), s(b"z")]);
        assert!(!h.active());
        let mut h = hooked();
        assert_eq!(h.feed(&[osc134("%output 1 x -1 0"), b"z".to_vec()].concat()), vec![lost(), s(b"z")]);
        // The 1 MiB cap.
        let mut h = hooked();
        let osc = [&b"\x1b]134;:"[..], &vec![b'a'; MAX_HELD][..]].concat();
        let out = trickle(&mut h, &osc, 4096);
        assert_eq!(out, vec![lost(), s(&osc)]);
        assert!(!h.active());
        // A reply cut short by output that is not an OSC.
        let mut h = in_body();
        assert_eq!(h.feed(b"oops"), vec![lost(), s(b"oops")]);
        // A rejected init line has no conductor of its own to fail (see
        // `a_bad_init_leaves_the_conductor_alone`).
        assert_eq!(hook().feed(b"\x1bP2000pbad\nls"), vec![s(b"\nls")]);
        // A real unhook, and a parser error that already says why, add nothing.
        assert_eq!(hooked().feed(&[osc135("unhook"), b"$ ".to_vec()].concat()), vec![ssh(Token::Unhook), s(b"$ ")]);
        let long = "7".repeat(crate::conductor_parser::MAX_ID + 1);
        let out = hooked().feed(&osc134(&format!("begin {long}")));
        assert_eq!(out, vec![ssh(Token::Error(crate::conductor_parser::ID_TOO_LONG.into()))]);
        // Byte by byte, once.
        let mut h = hooked();
        let out = trickle(&mut h, &[osc134("nonsense"), b"z".to_vec()].concat(), 1);
        assert_eq!(out, vec![lost(), s(b"z")]);
    }

    /// Ordinary output (a cat'd file) holding a broken `DCS 2000p` never fails the conductor
    /// the pane already has: the parser that unhooked never produced an `Init`, so it owns
    /// nothing to fail (iTerm2 only logs a rejected init line).
    #[test]
    fn a_bad_init_leaves_the_conductor_alone() {
        let bad: [&[u8]; 3] = [b"\x1bP2000pbad\nls", b"\x1bP2000p\xff\xfe - Yg==\nls", b"\x1bP2000pnone 1 Y*== - Yg==\nls"];
        for bytes in bad {
            // After a level 1 conductor let go, and while a nested hook is being read.
            let mut h = hooked();
            assert_eq!(h.feed(&osc135("unhook")), vec![ssh(Token::Unhook)]);
            let out = h.feed(bytes);
            assert!(out.iter().all(|o| matches!(o, Out::Screen(_))), "{out:?}");
            assert!(!h.active());
            let mut h = hooked();
            let nested = crate::ssh_fixtures::output("1", 9, -1, 0, bytes);
            let out = h.feed(&nested);
            assert!(out.iter().all(|o| !matches!(o, Out::Ssh(Token::Error(_), _))), "{out:?}");
            assert!(h.active(), "the parent stays hooked");
            // Split at every byte, the same.
            let mut h = hook();
            let out = trickle(&mut h, bytes, 1);
            assert!(out.iter().all(|o| matches!(o, Out::Screen(_))), "{out:?}");
        }
        // A parser that got past its init line still says so.
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:what\x1b\\"), vec![lost()]);
    }

    fn osc135(p: &str) -> Vec<u8> {
        crate::ssh_fixtures::osc135(p)
    }

    fn side(text: &str, pid: i32, channel: i32, depth: i32) -> Out {
        Out::Side { text: text.into(), pid, channel, depth }
    }

    fn raw(b: &[u8]) -> Out {
        Out::Raw(b.to_vec())
    }

    #[test]
    fn nested_reparse_follows_iterm2() {
        use crate::ssh_fixtures::{autopoll, dcs_hook, notif, output, pre_reply, terminate};
        let mut h = hooked();
        // Depth 0 channel -1: the screen, and the same bytes for the coprocess.
        assert_eq!(h.feed(&output("1", 4242, -1, 0, b"hi\r\n")), vec![s(b"hi\r\n"), raw(b"hi\r\n")]);
        // A side channel never reaches the screen or the coprocess.
        assert_eq!(h.feed(&output("2", 4242, 1, 0, b"x\n")), vec![side("x\n", 4242, 1, 0)], "a job's pieces of one read are one");
        // A second ssh inside: the child's meta tokens carry the child's depth.
        let mut h = hooked();
        let inner = [dcs_hook("", "u", "YQ==", "Yg=="), pre_reply("1.1", &["zsh"])].concat();
        let got = h.feed(&output("3", 4242, -1, 0, &inner));
        let up = |t: Token| Out::Ssh(t, 1);
        assert_eq!(
            got,
            vec![
                up(Token::Init(Init { token: "none".into(), unique_id: "u".into(), bool_args: "a".into(), sshargs: "b".into(), dcs_id: "D".into() })),
                up(Token::Begin("1.1".into())),
                up(Token::Line("zsh".into())),
                up(Token::End { id: "1.1".into(), status: 0, kind: "r".into() }),
                raw(&inner),
            ]
        );
        assert_eq!(h.child_pids(), [4242]);
        // %terminate drops that pid's parser: what follows starts clean.
        assert_eq!(h.feed(&terminate(4242, 0)), vec![ssh(Token::Terminate { pid: 4242, rc: 0 })]);
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        assert_eq!(h.feed(&output("4", 4242, -1, 0, b"z")), vec![s(b"z"), raw(b"z")], "a fresh child, not the hooked one");
        // A side channel reduces per pid, with state across reads.
        let mut h = hooked();
        assert_eq!(h.feed(&output("5", 7, 1, 0, b"a\x1b[")), vec![side("a", 7, 1, 0)]);
        assert_eq!(h.feed(&output("6", 7, 1, 0, b"1mb")), vec![side("b", 7, 1, 0)]);
        // A job's channel 1 comes as one piece per read, not one per newline; autopoll and notif
        // keep every piece (autopoll ends only on a piece that is exactly "EOF").
        let mut h = hooked();
        assert_eq!(h.feed(&output("5a", 8, 1, 0, b"a\r\nb\nc\n\n")), vec![side("a\r\nb\nc\n\n", 8, 1, 0)]);
        assert_eq!(h.feed(&output("5b", 8, 2, 0, b"e\nf")), vec![side("e", 8, 2, 0), side("\n", 8, 2, 0), side("f", 8, 2, 0)], "stderr completes a job per piece");
        let mut h = hooked();
        // A child's screen bytes of a side channel are reduced, its tokens dropped.
        assert_eq!(h.feed(&output("7", 7, 1, 0, &pre_reply("9", &["q"]))), vec![]);
        // Autopoll and notif leave no child and keep no state.
        let mut h = hooked();
        let got = h.feed(&autopoll("8", "r\n"));
        let text: String = got.iter().map(|o| if let Out::Side { text, pid, channel, depth } = o { assert_eq!((*pid, *channel, *depth), (crate::conductor_parser::AUTOPOLL_PID, 1, 0)); text.as_str() } else { panic!("{o:?}") }).collect();
        assert_eq!(text, "r\n\nEOF\n");
        assert_eq!(got.iter().filter(|o| matches!(o, Out::Side { text, .. } if text == "EOF")).count(), 1, "EOF is a piece of its own");
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        let got = h.feed(&notif("%notif tty -echo"));
        assert!(got.iter().all(|o| matches!(o, Out::Side { pid, channel: 1, depth: 0, .. } if *pid == crate::conductor_parser::NOTIF_PID)), "{got:?}");
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        // The unhook at depth 0 drops every child.
        let mut h = hooked();
        h.feed(&output("9", 1, -1, 0, b"a"));
        h.feed(&output("10", 2, 1, 0, b"b"));
        assert_eq!(h.child_pids(), [1, 2]);
        assert_eq!(h.feed(&osc135("unhook")), vec![ssh(Token::Unhook)]);
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        let mut h = hooked();
        h.feed(&output("11", 3, -1, 0, b"a"));
        h.unhook();
        assert_eq!(h.child_pids(), Vec::<i32>::new(), "a forced unhook too");
    }

    #[test]
    fn a_child_can_nest_again() {
        use crate::ssh_fixtures::{dcs_hook, output, pre_reply};
        let mut h = hooked();
        let innermost = [dcs_hook("", "v", "YQ==", "Yg=="), pre_reply("2.1", &["sh"])].concat();
        let middle = [dcs_hook("", "u", "YQ==", "Yg=="), output("3", 9, -1, 1, &innermost)].concat();
        let got = h.feed(&output("4", 8, -1, 0, &middle));
        let depths: Vec<i32> = got.iter().filter_map(|o| if let Out::Ssh(_, d) = o { Some(*d) } else { None }).collect();
        assert_eq!(depths, [1, 2, 2, 2, 2], "each level's meta tokens carry its own depth");
    }

    #[test]
    fn children_are_capped() {
        use crate::ssh_fixtures::output;
        let mut h = hooked();
        let mut errors = 0;
        for pid in 1..=300 {
            for o in h.feed(&output(&pid.to_string(), pid, -1, 0, b"x")) {
                if o == ssh(Token::Error(TOO_MANY_PROCESSES.into())) {
                    errors += 1;
                    assert_eq!(pid, MAX_CHILDREN as i32 + 1, "the first pid past the cap");
                }
            }
            if !h.active() {
                break;
            }
        }
        assert_eq!(errors, 1);
        assert!(!h.active(), "past the cap the parser lets go");
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        // Pids that come and go never reach it.
        let mut h = hooked();
        for pid in 1..=1000 {
            h.feed(&output("1", pid, -1, 0, b"x"));
            h.feed(&crate::ssh_fixtures::terminate(pid, 0));
        }
        assert!(h.active());
        // Autopoll blocks make no children either.
        for _ in 0..1000 {
            h.feed(&crate::ssh_fixtures::autopoll("2", "r\n"));
        }
        assert_eq!(h.child_pids(), Vec::<i32>::new());
    }

    fn init_and_reply() -> Vec<u8> {
        use crate::ssh_fixtures::{dcs_hook, pre_reply};
        [dcs_hook("", "v", "YQ==", "Yg=="), pre_reply("1.1", &["sh"])].concat()
    }

    fn depths(outs: &[Out]) -> Vec<i32> {
        outs.iter().filter_map(|o| if let Out::Ssh(_, d) = o { Some(*d) } else { None }).collect()
    }

    #[test]
    fn nesting_depth_is_bounded() {
        use crate::ssh_fixtures::{dcs_hook, output};
        // One block that nests a DCS 2000p and an %output twenty levels deep (a cat'd file).
        let mut h = hooked();
        let mut inner = init_and_reply();
        for i in 0..19 {
            inner = [dcs_hook("", "v", "YQ==", "Yg=="), output(&i.to_string(), 5, -1, 0, &inner)].concat();
        }
        let got = h.feed(&output("o", 5, -1, 0, &inner));
        let d = depths(&got);
        assert!(!d.is_empty() && d.iter().all(|d| *d <= MAX_DEPTH), "{d:?}");
        assert_eq!(got.iter().filter(|o| matches!(o, Out::Ssh(Token::Init(_), _))).count(), MAX_DEPTH as usize, "one conductor per allowed level");
        // What is past the cap is plain output, and the depth 0 parser keeps working.
        assert!(got.iter().any(|o| matches!(o, Out::Screen(_))));
        assert!(h.active());
        assert_eq!(h.feed(&output("z", 4242, -1, 0, b"ok")), vec![s(b"ok"), raw(b"ok")]);
        // A hook past the cap does not even hold the start of a DCS 2000p back.
        let mut deep = Hook::with_depth(MAX_DEPTH + 1);
        assert_eq!(deep.feed(b"a\x1bP20"), vec![s(b"a\x1bP20")]);
        assert_eq!(deep.feed(b"00pnone u YQ== - Yg==\n"), vec![s(b"00pnone u YQ== - Yg==\n")]);
        assert!(!deep.active());
    }

    #[test]
    fn children_are_capped_per_depth() {
        use crate::ssh_fixtures::{dcs_hook, output};
        // Pid 5's parser (depth 1) is sent 300 processes of its own.
        let mut h = hooked();
        let mut errors = error_depths(&h.feed(&output("r", 5, -1, 0, &dcs_hook("", "v", "YQ==", "Yg=="))));
        for q in 1..=300 {
            errors.extend(error_depths(&h.feed(&output("r", 5, -1, 0, &output("q", q, -1, 1, b"x")))));
            assert!(h.children_at(1) <= MAX_CHILDREN, "{}", h.children_at(1));
        }
        assert_eq!(errors, [1], "the depth 1 parser, at its 257th process");
        assert!(h.active(), "the depth 0 parser is not the one that failed");
        assert_eq!((h.children_at(0), h.children_at(1)), (1, 0), "its children went with it");
        // Children given back leave room again.
        let mut h = hooked();
        for pid in 1..=200 {
            h.feed(&output("1", pid, -1, 0, b"x"));
        }
        assert_eq!(h.tree_children(), 200);
        h.unhook();
        assert_eq!(h.tree_children(), 0);
    }

    #[test]
    fn pending_bytes_are_capped_per_depth() {
        use crate::ssh_fixtures::{dcs_hook, osc134, output};
        // Twenty children of pid 5's parser each hold 900,000 bytes of an unfinished init line.
        let mut h = hooked();
        let mut errors = error_depths(&h.feed(&output("r", 5, -1, 0, &dcs_hook("", "v", "YQ==", "Yg=="))));
        let long = [&START[..], &vec![b'a'; 900_000][..]].concat();
        for q in 1..=20 {
            errors.extend(error_depths(&h.feed(&output("r", 5, -1, 0, &output("q", q, -1, 1, &long)))));
            assert!(h.pending_at(2) <= MAX_PENDING, "{}", h.pending_at(2));
        }
        assert_eq!(errors, Vec::<i32>::new(), "they own no conductor: refused without a word");
        assert_eq!(h.pending_at(2), 18 * 900_000, "the 19th and the 20th were refused");
        // The depth's owner asking past it is refused with an error at its depth.
        let mut inner = dcs_hook("", "w", "YQ==", "Yg==");
        inner.extend(osc134("%output x 9 1 2"));
        inner.extend(vec![b'b'; 900_000]);
        let got = h.feed(&output("r", 5, -1, 0, &output("q", 30, -1, 1, &inner)));
        assert_eq!(error_depths(&got), [2]);
        assert!(!h.owned(2) && h.owned(1) && h.active());
        h.unhook();
        assert_eq!(h.tree_pending(), 0, "given back");
    }

    #[test]
    fn let_go_owner_leaves_the_parents_hooked() {
        use crate::ssh_fixtures::{dcs_hook, output};
        let build = || {
            let mut h = hooked();
            let middle = [dcs_hook("", "u", "YQ==", "Yg=="), output("3", 9, -1, 1, &init_and_reply())].concat();
            assert_eq!(depths(&h.feed(&output("4", 8, -1, 0, &middle))), [1, 2, 2, 2, 2]);
            h
        };
        // Depth 2 goes: the depth 1 parser still reads its %output, into a fresh child that
        // may hook again.
        let mut h = build();
        assert_eq!(h.tree_children(), 2);
        assert!(!h.let_go_owner(3), "nothing owns depth 3");
        assert!(h.let_go_owner(2));
        assert_eq!((h.tree_children(), h.child_pids(), h.active(), h.owned(2)), (2, vec![8], true, false));
        let got = h.feed(&output("5", 8, -1, 0, &output("6", 9, -1, 1, b"zz")));
        assert_eq!(got, vec![s(b"zz"), raw(&output("6", 9, -1, 1, b"zz"))], "depth 1 still parses, its child shows plain output");
        assert_eq!(depths(&h.feed(&output("7", 8, -1, 0, &output("8", 9, -1, 1, &init_and_reply())))), [2, 2, 2, 2]);
        // Depth 1 goes: the depth 0 parser reads on, and the other processes stay.
        let mut h = build();
        h.feed(&output("9", 12, 1, 0, b"side"));
        assert!(h.let_go_owner(1));
        assert_eq!((h.tree_children(), h.child_pids(), h.active(), h.owned(1), h.owned(2)), (2, vec![8, 12], true, false, false));
        // Depth 0 is the whole hook.
        let mut h = build();
        assert!(h.let_go_owner(0));
        assert_eq!((h.tree_children(), h.active()), (0, false));
    }

    #[test]
    fn a_refused_depth_drops_the_rest_of_its_batch() {
        let mut b = Batch::default();
        let side = |depth| Out::Side { text: "x".into(), pid: 1, channel: 1, depth };
        assert!(!b.drops(&Out::Ssh(Token::Unhook, 3)));
        b.hooked(1, true);
        assert!(!b.drops(&Out::Ssh(Token::Unhook, 3)));
        b.hooked(2, false);
        assert!(b.drops(&Out::Ssh(Token::Line("l".into()), 2)) && b.drops(&Out::Ssh(Token::Unhook, 3)) && b.drops(&side(2)));
        assert!(!b.drops(&Out::Ssh(Token::Line("l".into()), 1)) && !b.drops(&side(1)), "shallower ones go on");
        assert!(!b.drops(&s(b"x")) && !b.drops(&raw(b"x")), "output is output");
        b.hooked(1, false);
        assert!(b.drops(&Out::Ssh(Token::Unhook, 1)), "the shallowest refusal counts");
        b.hooked(0, true);
        assert!(!b.drops(&Out::Ssh(Token::Unhook, 1)), "a new conductor above: nothing of the refused follows");
    }

    /// The depth of every error in `outs`, in order.
    fn error_depths(outs: &[Out]) -> Vec<i32> {
        outs.iter().filter_map(|o| if let Out::Ssh(Token::Error(_), d) = o { Some(*d) } else { None }).collect()
    }

    #[test]
    fn deep_children_never_starve_a_shallower_hook() {
        use crate::ssh_fixtures::{autopoll, dcs_hook, output};
        // Pid 5's parser (depth 1) follows 255 processes of its own: with the root's child for
        // pid 5, 256 hooks below the root.
        let mut h = hooked();
        let mut block = dcs_hook("", "v", "YQ==", "Yg==");
        for q in 1..=255 {
            block.extend(output("q", q, -1, 1, b"x"));
        }
        assert_eq!(error_depths(&h.feed(&output("r", 5, -1, 0, &block))), Vec::<i32>::new());
        // The root still makes its autopoll child each second, and a child for a new pid.
        for i in 0..3 {
            assert_eq!(error_depths(&h.feed(&autopoll("a", "r\n"))), Vec::<i32>::new(), "autopoll {i}");
            assert_eq!(error_depths(&h.feed(&output("n", 1000 + i, -1, 0, b"z"))), Vec::<i32>::new(), "pid {i}");
        }
        assert!(h.active(), "depth 0 keeps framing");
        // Pid 5's parser asks for more than its depth holds: it alone is refused, once.
        let mut errors = Vec::new();
        for q in 256..=300 {
            errors.extend(error_depths(&h.feed(&output("r", 5, -1, 0, &output("q", q, -1, 1, b"x")))));
        }
        assert_eq!(errors, [1], "the depth 1 hook that asked");
        assert!(h.active());
        assert_eq!(error_depths(&h.feed(&autopoll("a", "r\n"))), Vec::<i32>::new());
        assert_eq!(h.feed(&output("z", 4242, -1, 0, b"ok")), vec![s(b"ok"), raw(b"ok")], "the root still parses");
    }

    #[test]
    fn deep_pending_never_starves_a_shallower_hook() {
        use crate::ssh_fixtures::{dcs_hook, osc134, output};
        // Pid 5's parser (depth 1) has 16 children, each holding 1,000,000 bytes of an
        // unfinished init line (under the parser's own 1 MiB cap): 16,000,000 at depth 2.
        let mut h = hooked();
        let mut errors = error_depths(&h.feed(&output("r", 5, -1, 0, &dcs_hook("", "v", "YQ==", "Yg=="))));
        let long = [&START[..], &vec![b'a'; 1_000_000][..]].concat();
        for q in 1..=16 {
            errors.extend(error_depths(&h.feed(&output("r", 5, -1, 0, &output("q", q, -1, 1, &long)))));
        }
        assert_eq!(errors, Vec::<i32>::new());
        // The root holds a block of its own unfinished: it is not refused, and finishes it.
        let mut open = osc134("%output x 9 -1 0");
        open.extend(vec![b'b'; 900_000]);
        assert_eq!(error_depths(&h.feed(&open)), Vec::<i32>::new(), "depth 0 is not refused");
        assert!(h.active(), "depth 0 keeps framing");
        let got = h.feed(&osc134("%end x"));
        assert_eq!(got.first(), Some(&s(&vec![b'b'; 900_000])));
        // Pid 5's parser holds a block unfinished too: depth 2 being full does not refuse it.
        let mut inner = osc134("%output w 8 1 1");
        inner.extend(vec![b'c'; 500_000]);
        assert_eq!(error_depths(&h.feed(&output("y", 5, -1, 0, &inner))), Vec::<i32>::new(), "depth 1 is not refused");
        assert_eq!(error_depths(&h.feed(&output("y", 5, -1, 0, &osc134("%end w")))), Vec::<i32>::new());
        // A 17th unfinished init line at depth 2 is refused; it owns no conductor, so no error.
        let got = h.feed(&output("r", 5, -1, 0, &output("q", 17, -1, 1, &long)));
        assert_eq!(error_depths(&got), Vec::<i32>::new());
        assert!(h.tree_pending() <= 2 * MAX_PENDING, "{}", h.tree_pending());
        assert!(h.active());
    }

    #[test]
    fn one_hook_per_depth_owns_a_conductor() {
        use crate::ssh_fixtures::output;
        let mut h = hooked();
        assert_eq!(depths(&h.feed(&output("a", 5, -1, 0, &init_and_reply()))), [1, 1, 1, 1]);
        // Another process's DCS 2000p at the same depth hooks nothing and says nothing.
        let got = h.feed(&output("b", 6, -1, 0, &init_and_reply()));
        assert_eq!(got, vec![raw(&init_and_reply())], "only the coprocess copy");
        // Nor when it tries again.
        assert_eq!(depths(&h.feed(&output("c", 6, -1, 0, &init_and_reply()))), Vec::<i32>::new());
        // Once pid 5 unhooks, the depth is free.
        assert_eq!(h.feed(&output("d", 5, -1, 0, &osc135("unhook"))), vec![Out::Ssh(Token::Unhook, 1), raw(&osc135("unhook"))]);
        assert_eq!(depths(&h.feed(&output("e", 6, -1, 0, &init_and_reply()))), [1, 1, 1, 1]);
        // A side channel's hook never takes a depth from the one that frames.
        let mut h = hooked();
        assert_eq!(h.feed(&output("f", 7, 1, 0, &init_and_reply())), vec![]);
        assert_eq!(depths(&h.feed(&output("g", 5, -1, 0, &init_and_reply()))), [1, 1, 1, 1]);
    }

    #[test]
    fn a_parser_that_ends_takes_its_children() {
        use crate::ssh_fixtures::{osc134, output};
        let mut h = hooked();
        h.feed(&output("a", 5, -1, 0, &init_and_reply()));
        assert_eq!(h.child_pids(), [5]);
        // The root's parser ends with an error of its own: pid 5's hook goes with it.
        let long = "7".repeat(crate::conductor_parser::MAX_ID + 1);
        assert_eq!(error_depths(&h.feed(&osc134(&format!("begin {long}")))), [0]);
        assert!(!h.active());
        assert_eq!(h.child_pids(), Vec::<i32>::new());
        assert_eq!(h.tree_children(), 0);
    }
}
