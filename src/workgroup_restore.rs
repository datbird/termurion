//! Restoring workgroups (milestone 13y1, Task 9): the pure side. The descriptor a shown member
//! saves in its session value, read back bounded and without panics, whether a restored session
//! must not relaunch its program, the instance id a restored instance takes and what an adoption
//! rebuilds from the model. The app side is `app/workgroup_restore_app.rs`.
//!
//! Restoration is data, not execution: nothing here starts a program, and the descriptor is the
//! file's word, so every id, count and size in it is judged before it is used (P9).
//!
//! Citations: `WR` `iTermWorkgroupRestoration.swift`, `WRC` `WorkgroupRestorationCoordinator.swift`,
//! `WC` `iTermWorkgroupController.swift` under `~/gitrepos/iterm2-ref/sources/Workgroups`; `PS`
//! `PTYSession.m`.

use crate::arrangement::{Table, Value};
use crate::workgroup::{Mode, Source, Workgroup};

pub const WORKGROUP_ID: &str = "workgroupID";
pub const INSTANCE_ID: &str = "instanceID";
pub const GIT_BASE: &str = "gitBase";
pub const LEADER_ID: &str = "leaderID";
pub const ANCHOR_ID: &str = "anchorID";
pub const MEMBERS: &str = "members";
pub const MEMBER_GUIDS: &str = "memberGUIDs";
pub const NON_PEER_CHILDREN: &str = "nonPeerChildren";
pub const ANCHOR_PENDING: &str = "anchorPending";
pub const CONFIG_ID: &str = "configID";
pub const DISPLAY_NAME: &str = "displayName";
pub const FRAME: &str = "frame";
pub const ARRANGEMENT: &str = "arrangement";
pub const GUID: &str = "guid";
pub const PENDING: &str = "pending";
/// Not iTerm2's: the person's answer for an import's commands, sealed to this install (`seal`): the
/// HMAC, under the install's own key, of the workgroup id and the digest of the commands answered for
/// (`run::Approved`). A restored import runs its commands only while they are still those, and only
/// when the seal comes from Tandem's own launch state (the Task 9 review: a plain digest is computable
/// by whoever holds the workgroup).
pub const APPROVAL: &str = "approvalDigest";

/// The install's approval key, beside the settings: made owner-only at first use, never exported,
/// imported or synced (`settings_export::APP_SUPPORT_EXCLUDE`), never in a settings, profiles or
/// arrangement file.
pub const KEY_FILE: &str = "workgroup-approval-key";
/// The key's length in bytes.
pub const KEY_LEN: usize = 32;

/// Where a restore read its descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Launch-time window restoration: the state Tandem itself wrote at quit.
    Launch,
    /// A saved arrangement by name. Arrangements travel through remote prefs and imports, so an
    /// approval found there is never the person's answer: every import asks.
    Named,
}

/// One buried member's saved state: its own session value and whether it still sat on its pre-launch
/// overlay (never started: restored fresh, not adopted, `WR:44-50`).
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub config_id: String,
    pub display_name: String,
    pub frame: String,
    pub arrangement: Value,
    pub pending: bool,
}

/// A split or tab child, found again by its session's GUID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub config_id: String,
    pub guid: String,
}

/// What the visible member of a peer group saves (`WR:encodeState`).
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor {
    pub workgroup_id: String,
    /// Empty when the file held none or one Tandem would not have minted.
    pub instance_id: String,
    pub git_base: String,
    pub leader_id: String,
    pub anchor_id: String,
    pub members: Vec<Member>,
    pub member_guids: Vec<String>,
    pub non_peer_children: Vec<Child>,
    pub anchor_pending: bool,
    pub approval: Option<String>,
}

/// Strings of an arrangement at most (one whole saved session with its history, the restoration
/// cap's order of size), and nodes and depth: a file cannot make a restore walk without end.
const MAX_ARRANGEMENT_BYTES: usize = 128 << 20;
const MAX_ARRANGEMENT_NODES: usize = 2_000_000;
const MAX_DEPTH: usize = 32;
/// A GUID or frame text at most.
const MAX_TEXT: usize = 128;
const MAX_NAME: usize = 128;

/// The text bytes of `v` when it is an arrangement Tandem could have saved: a table, within the depth,
/// node and byte limits (an iterative walk, so a hostile depth cannot overflow the stack).
fn arrangement_size(v: &Value) -> Option<usize> {
    if !v.is_table() {
        return None;
    }
    let (mut nodes, mut bytes) = (0usize, 0usize);
    let mut stack: Vec<(&Value, usize)> = vec![(v, 1)];
    while let Some((v, depth)) = stack.pop() {
        nodes += 1;
        if depth > MAX_DEPTH || nodes > MAX_ARRANGEMENT_NODES {
            return None;
        }
        match v {
            Value::String(s) => bytes = bytes.saturating_add(s.len()),
            Value::Table(t) => {
                for (k, x) in t {
                    bytes = bytes.saturating_add(k.len());
                    stack.push((x, depth + 1));
                }
            }
            Value::Array(a) => stack.extend(a.iter().map(|x| (x, depth + 1))),
            _ => {}
        }
        if bytes > MAX_ARRANGEMENT_BYTES {
            return None;
        }
    }
    Some(bytes)
}

/// A GUID Tandem could have minted or iTerm2 wrote: letters, digits, dashes and underscores.
fn guid_ok(s: &str) -> bool {
    (1..=MAX_TEXT).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn text_of<'a>(t: &'a Table, key: &str) -> Option<&'a str> {
    t.get(key).and_then(Value::as_str)
}

fn id_of(t: &Table, key: &str) -> Option<String> {
    text_of(t, key).filter(|s| crate::workgroup::id_ok(s)).map(str::to_string)
}

/// A SHA-256 digest in hex, the form `run::Approved::digest` writes.
fn digest_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Descriptor {
    /// The value under the session's "Workgroup" key (`WR:encodeState`). `nonPeerChildren` is written
    /// only when there are some.
    pub fn to_value(&self) -> Value {
        let mut t = Table::new();
        t.insert(WORKGROUP_ID.into(), Value::String(self.workgroup_id.clone()));
        t.insert(INSTANCE_ID.into(), Value::String(self.instance_id.clone()));
        t.insert(GIT_BASE.into(), Value::String(self.git_base.clone()));
        t.insert(LEADER_ID.into(), Value::String(self.leader_id.clone()));
        t.insert(ANCHOR_ID.into(), Value::String(self.anchor_id.clone()));
        let members = self
            .members
            .iter()
            .map(|m| {
                let mut e = Table::new();
                e.insert(CONFIG_ID.into(), Value::String(m.config_id.clone()));
                e.insert(DISPLAY_NAME.into(), Value::String(m.display_name.clone()));
                e.insert(FRAME.into(), Value::String(m.frame.clone()));
                e.insert(ARRANGEMENT.into(), m.arrangement.clone());
                e.insert(PENDING.into(), Value::Boolean(m.pending));
                Value::Table(e)
            })
            .collect();
        t.insert(MEMBERS.into(), Value::Array(members));
        t.insert(MEMBER_GUIDS.into(), Value::Array(self.member_guids.iter().cloned().map(Value::String).collect()));
        t.insert(ANCHOR_PENDING.into(), Value::Boolean(self.anchor_pending));
        if !self.non_peer_children.is_empty() {
            let kids = self
                .non_peer_children
                .iter()
                .map(|c| {
                    let mut e = Table::new();
                    e.insert(CONFIG_ID.into(), Value::String(c.config_id.clone()));
                    e.insert(GUID.into(), Value::String(c.guid.clone()));
                    Value::Table(e)
                })
                .collect();
            t.insert(NON_PEER_CHILDREN.into(), Value::Array(kids));
        }
        if let Some(a) = &self.approval {
            t.insert(APPROVAL.into(), Value::String(a.clone()));
        }
        Value::Table(t)
    }

    /// Reads a saved descriptor. None without a workgroup, leader and anchor id (`WRC:reconstruct`'s
    /// first guard). Every other field is judged and a bad one is dropped alone: an instance id Tandem
    /// would not mint reads as none, a base that is not usable as HEAD, a member or child that does
    /// not fit is skipped, counts are capped at `MAX_SESSIONS`.
    pub fn from_value(v: &Value) -> Option<Descriptor> {
        let t = v.as_table()?;
        let max = crate::workgroup::MAX_SESSIONS;
        let workgroup_id = id_of(t, WORKGROUP_ID)?;
        let leader_id = id_of(t, LEADER_ID)?;
        let anchor_id = id_of(t, ANCHOR_ID)?;
        let instance_id = text_of(t, INSTANCE_ID).filter(|s| crate::workgroup_run::valid_instance_id(s)).unwrap_or_default().to_string();
        let git_base = text_of(t, GIT_BASE).filter(|b| crate::workgroup_git::base_ok(b)).unwrap_or(crate::workgroup_git::DEFAULT_BASE).to_string();
        let all = t.get(MEMBERS).and_then(Value::as_array);
        let mut members = Vec::new();
        for i in kept_members(t) {
            let Some(m) = all.and_then(|a| a.get(i)).and_then(Value::as_table) else { continue };
            let (Some(config_id), Some(arrangement)) = (id_of(m, CONFIG_ID), m.get(ARRANGEMENT)) else { continue };
            let display_name = text_of(m, DISPLAY_NAME).map(|n| crate::inert_files::display_text(n, MAX_NAME, false)).unwrap_or_default();
            let frame = text_of(m, FRAME).filter(|f| f.len() <= MAX_TEXT).unwrap_or_default().to_string();
            let pending = m.get(PENDING).and_then(Value::as_bool).unwrap_or(false);
            members.push(Member { config_id, display_name, frame, arrangement: arrangement.clone(), pending });
        }
        let member_guids = t.get(MEMBER_GUIDS).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|g| guid_ok(g)).take(2 * max).map(str::to_string).collect();
        let non_peer_children = t
            .get(NON_PEER_CHILDREN)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let c = c.as_table()?;
                Some(Child { config_id: id_of(c, CONFIG_ID)?, guid: text_of(c, GUID).filter(|g| guid_ok(g))?.to_string() })
            })
            .take(max)
            .collect();
        Some(Descriptor {
            workgroup_id,
            instance_id,
            git_base,
            leader_id,
            anchor_id,
            members,
            member_guids,
            non_peer_children,
            anchor_pending: t.get(ANCHOR_PENDING).and_then(Value::as_bool).unwrap_or(false),
            approval: text_of(t, APPROVAL).filter(|a| digest_ok(a)).map(str::to_string),
        })
    }
}

/// The indices in a descriptor table's `members` that `Descriptor::from_value` reads: a table with a
/// usable config id and an arrangement within the limits, at most `MAX_SESSIONS`, until the total byte
/// cap. One predicate for the read and the host plan (Task 9 review minor 2).
fn kept_members(t: &Table) -> Vec<usize> {
    let mut total = 0usize;
    let mut kept = Vec::new();
    for (i, m) in t.get(MEMBERS).and_then(Value::as_array).into_iter().flatten().enumerate() {
        if kept.len() >= crate::workgroup::MAX_SESSIONS {
            break;
        }
        let Some(m) = m.as_table() else { continue };
        let (Some(_), Some(arrangement)) = (id_of(m, CONFIG_ID), m.get(ARRANGEMENT)) else { continue };
        let Some(size) = arrangement_size(arrangement) else { continue };
        total = total.saturating_add(size);
        if total > MAX_ARRANGEMENT_BYTES {
            break;
        }
        kept.push(i);
    }
    kept
}

/// The saved sessions embedded in a session table's descriptor, in `members` order: exactly the members
/// `Descriptor::from_value` reads (`kept_members`). The restore's host plan claims the shells they
/// kept, so none comes back as an orphan tab, and none is claimed that the restore never reads.
pub fn embedded(session: &Table) -> Vec<&Table> {
    let Some(d) = session.get(crate::arrangement::WORKGROUP_KEY).and_then(Value::as_table) else { return Vec::new() };
    let Some(members) = d.get(MEMBERS).and_then(Value::as_array) else { return Vec::new() };
    kept_members(d).into_iter().filter_map(|i| members.get(i)?.get(ARRANGEMENT)?.as_table()).collect()
}

/// `embedded`, writable: each member's session table, disjoint.
pub fn embedded_mut(session: &mut Table) -> Vec<&mut Table> {
    let kept = match session.get(crate::arrangement::WORKGROUP_KEY).and_then(Value::as_table) {
        Some(d) => kept_members(d),
        None => return Vec::new(),
    };
    let members = session.get_mut(crate::arrangement::WORKGROUP_KEY).and_then(|d| d.get_mut(MEMBERS)).and_then(Value::as_array_mut);
    members.into_iter().flatten().enumerate().filter(|(i, _)| kept.contains(i)).filter_map(|(_, m)| m.get_mut(ARRANGEMENT)).filter_map(Value::as_table_mut).collect()
}

/// The modes whose restored session must not start a replacement program (`WR:modeInhibitsRelaunch`,
/// one source of truth for the anchor and the buried peers).
pub fn mode_inhibits(mode: Mode) -> bool {
    matches!(mode, Mode::CodeReview | Mode::Diff)
}

/// Whether restoring the anchor starts no program of its own (`WR:shouldInhibitRelaunch`, `PS:2361-2390`).
/// A Code Review or Diff session with no program to take back is restored as an idle session: its last
/// output stays and nothing runs until the person starts it. A shell would be a stray one. When the
/// overlay was showing (`anchorPending`) there was no program at all, and the overlay comes back instead
/// (iTerm2 relaunches its shell under the overlay; Tandem has no shell to put under it). `had_program`
/// is a program that came back from the session host: nothing to inhibit then.
pub fn inhibit_relaunch(d: &Descriptor, anchor_mode: Mode, had_program: bool) -> bool {
    let program = had_program && !d.anchor_pending;
    !program && mode_inhibits(anchor_mode)
}

/// Whether a restored session starts no program, worked out before its attach is tried and asked
/// again after it (Task 9 review minor 3): an attach that fails leaves the session with no program,
/// so a Code Review or Diff session then comes back idle too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inhibit {
    /// A buried Code Review or Diff member is being built (`inhibit_next`).
    member: bool,
    /// The anchor's mode, when its workgroup still exists, and whether it was on its overlay.
    anchor: Option<(Mode, bool)>,
}

impl Inhibit {
    pub fn of(d: Option<&Descriptor>, anchor_mode: Option<Mode>, member: bool) -> Inhibit {
        Inhibit { member, anchor: d.zip(anchor_mode).map(|(d, m)| (m, d.anchor_pending)) }
    }

    /// The answer for a session that has a program from the session host (`had_program`) or not.
    pub fn with(self, had_program: bool) -> bool {
        let anchor = self.anchor.is_some_and(|(mode, pending)| !(had_program && !pending) && mode_inhibits(mode));
        (self.member && !had_program) || anchor
    }
}

/// The instance id a restored instance takes (`WC:adopt`): the saved one, unless it is taken or not
/// one Tandem mints (the file's word), then a fresh one.
pub fn resolve_instance_id(saved: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if crate::workgroup_run::valid_instance_id(saved) && !taken(saved) {
        return saved.to_string();
    }
    loop {
        let id = crate::workgroup_run::mint_instance_id();
        if !taken(&id) {
            return id;
        }
    }
}

/// Why a descriptor adopts nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The workgroup was deleted since the save: the sessions stay plain sessions.
    Gone,
    /// The descriptor does not fit the workgroup it names (another leader, an unknown or repeated member).
    Mismatch,
    /// The leader is neither the shown member nor restored: it cannot be rebuilt (`WRC`: corrupt descriptor).
    NoLeader,
}

/// What an adoption rebuilds from a descriptor and the model.
#[derive(Clone, Debug, PartialEq)]
pub struct Adoption {
    /// The workgroup as the model has it now, not as it was saved.
    pub workgroup: Workgroup,
    /// The member shown in the leader's place (the anchor).
    pub active: String,
    pub leader: String,
    pub git_base: String,
    /// The root, then its peers, in the order entering spawns them.
    pub peers: Vec<String>,
    /// Every peer but the shown one: kept alive in no layout, the leader among them when it is not shown.
    pub buried: Vec<String>,
    /// Buried members rebuilt from their saved sessions.
    pub restored: Vec<String>,
    /// Buried members that come back fresh: the pending ones, which wait or ask again.
    pub fresh: Vec<String>,
    /// The splits and tabs found again by GUID.
    pub children: Vec<Child>,
}

/// The peers of `w` in entering order: the root, then its peers.
fn peer_order(w: &Workgroup) -> Vec<String> {
    let Some(root) = w.root() else { return Vec::new() };
    let mut out = vec![root.unique_identifier.clone()];
    out.extend(w.children(&root.unique_identifier).into_iter().filter(|c| matches!(c.kind, crate::workgroup::Kind::Peer)).map(|c| c.unique_identifier.clone()));
    out
}

/// What adopting `d` rebuilds, judged against the workgroup the model has now (`WRC:reconstruct`).
pub fn adoption(d: &Descriptor, lookup: &dyn Fn(&str) -> Option<Workgroup>) -> Result<Adoption, Refusal> {
    let workgroup = lookup(&d.workgroup_id).ok_or(Refusal::Gone)?;
    let peers = peer_order(&workgroup);
    let root = peers.first().ok_or(Refusal::Mismatch)?;
    // The leader is the root; the anchor and every embedded member a peer of it, each once.
    if &d.leader_id != root || !peers.contains(&d.anchor_id) {
        return Err(Refusal::Mismatch);
    }
    let mut seen = vec![d.anchor_id.as_str()];
    for m in &d.members {
        if !peers.contains(&m.config_id) || seen.contains(&m.config_id.as_str()) {
            return Err(Refusal::Mismatch);
        }
        seen.push(&m.config_id);
    }
    if d.anchor_id != d.leader_id && !d.members.iter().any(|m| m.config_id == d.leader_id && !m.pending) {
        return Err(Refusal::NoLeader);
    }
    let buried: Vec<String> = peers.iter().filter(|p| **p != d.anchor_id).cloned().collect();
    let restored = d.members.iter().filter(|m| !m.pending).map(|m| m.config_id.clone()).collect();
    // A peer with no saved session that is a Diff or Code Review member comes back waiting; a regular
    // one that was never started stays unstarted (nothing runs at a restore).
    let fresh = buried
        .iter()
        .filter(|p| !d.members.iter().any(|m| &m.config_id == *p && !m.pending))
        .filter(|p| workgroup.session(p).is_some_and(|s| mode_inhibits(s.mode)))
        .cloned()
        .collect();
    let children = d
        .non_peer_children
        .iter()
        .filter(|c| workgroup.session(&c.config_id).is_some_and(|s| matches!(s.kind, crate::workgroup::Kind::Split(_) | crate::workgroup::Kind::Tab)))
        .cloned()
        .collect();
    Ok(Adoption { active: d.anchor_id.clone(), leader: d.leader_id.clone(), git_base: d.git_base.clone(), peers, buried, restored, fresh, children, workgroup })
}

/// An anchor not in a window this long stops blocking Enter Workgroup (`pendingReconstructionTimeout`,
/// a soft limit: the entry is kept), and is given up on after `GIVE_UP`.
pub const RELEASE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
pub const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(120);

/// Whether rebuilding asks the person first, and whether the question can show every command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    None,
    Showable,
    TooLong,
}

/// Why a restore is not adopted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The anchor closed before the rebuild.
    AnchorGone,
    /// The anchor never reached a window.
    GaveUp,
    /// The anchor entered a workgroup meanwhile (another pass, a manual entry, a trigger).
    Taken,
    /// The workgroup is gone or the descriptor does not fit it (`adoption`).
    Unfit,
    /// The commands are too long to show in full, so they cannot be asked about.
    TooLong,
    /// The person said no, or the commands changed since the question.
    Declined,
}

/// What a waiting restore does next (`WRC.reconstruct`, with the restore question in between).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The anchor's window is not up: look again. `release` lets Enter Workgroup in again (soft limit).
    Wait { release: bool },
    /// The question is up: wait for the answer.
    Hold,
    /// Ask the person now.
    Ask,
    /// Rebuild the instance, then release the claims.
    Adopt,
    /// Not adopted: release the claims and end the buried members' shells.
    Discard(Why),
}

impl Step {
    /// Whether the restore still claims its anchor and sessions after this step: the Enter Workgroup
    /// trigger and a manual entry stay out of them.
    pub fn keeps_claims(self) -> bool {
        matches!(self, Step::Ask | Step::Hold | Step::Wait { release: false })
    }
}

/// What the app knows of one waiting restore when it looks at it.
#[derive(Clone, Copy, Debug)]
pub struct Facts {
    pub anchor_alive: bool,
    pub in_window: bool,
    /// The anchor belongs to an instance already.
    pub in_workgroup: bool,
    /// The restore question is up.
    pub asked: bool,
    pub age: std::time::Duration,
    /// The person's answer, when this look comes from it (false also when the commands changed since).
    pub answer: Option<bool>,
    /// `adoption` found a plan.
    pub fits: bool,
    pub question: Question,
}

/// The next step for one restore. The order matters: a closed anchor ends everything; an unanswered
/// question holds; an anchor that entered a workgroup meanwhile is never adopted twice and its buried
/// members' shells end, answered or not.
pub fn next_step(f: &Facts) -> Step {
    if !f.anchor_alive {
        return Step::Discard(Why::AnchorGone);
    }
    if f.answer.is_none() {
        if f.asked {
            return Step::Hold;
        }
        if !f.in_window {
            if f.age >= GIVE_UP {
                return Step::Discard(Why::GaveUp);
            }
            return Step::Wait { release: f.age >= RELEASE_AFTER };
        }
    }
    if f.in_workgroup {
        return Step::Discard(Why::Taken);
    }
    if !f.fits {
        return Step::Discard(Why::Unfit);
    }
    match (f.answer, f.question) {
        (Some(true), _) | (None, Question::None) => Step::Adopt,
        (Some(false), _) => Step::Discard(Why::Declined),
        (None, Question::Showable) => Step::Ask,
        (None, Question::TooLong) => Step::Discard(Why::TooLong),
    }
}

/// The approval a launch state carries for `workgroup_id` answered with `digest` (`run::Approved`):
/// HMAC-SHA256 under the install's key over both, each length-prefixed, as lowercase hex. Only this
/// install can make it, so no file from elsewhere can carry an answer the person never gave here.
pub fn seal(key: &[u8], workgroup_id: &str, digest: &str) -> String {
    let mut msg = b"tandem workgroup approval\0".to_vec();
    for part in [workgroup_id, digest] {
        msg.extend((part.len() as u64).to_le_bytes());
        msg.extend(part.as_bytes());
    }
    crate::host_identity::hmac_sha256_hex(key, &msg)
}

/// Equal texts, compared in time that does not depend on where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether adopting asks the person again: the person's own workgroup never; an import runs its
/// commands unasked only from Tandem's own launch state (`Origin::Launch`) whose approval is this
/// install's seal (`key`) of the commands it would run now. A named arrangement, a missing key, a
/// plain digest or a seal from another install all ask (the trust ruling and the Task 9 review).
pub fn needs_question(source: Source, d: &Descriptor, w: &Workgroup, origin: Origin, key: Option<&[u8]>) -> bool {
    if source.is_person() {
        return false;
    }
    let (Origin::Launch, Some(key), Some(approval)) = (origin, key, d.approval.as_deref()) else { return true };
    !same(approval, &seal(key, &w.unique_identifier, crate::workgroup_run::Approved::of(w).digest()))
}

/// The entropy the key is protected with: DPAPI unprotects it only with these bytes too, so a blob
/// another program protected for this user is no key either.
#[cfg(windows)]
const KEY_ENTROPY: &[u8] = b"tandem workgroup approval key\0v1";

/// DPAPI (`CryptProtectData`, this user's scope, no UI) over `data` with `KEY_ENTROPY`, or the
/// reverse. None on any failure.
#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len()).ok()?, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: KEY_ENTROPY.len() as u32, pbData: KEY_ENTROPY.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: the blobs point at live buffers of the lengths given (DPAPI only reads them); the
    // output is copied out and freed once.
    unsafe {
        let ok = if protect {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        if !protect {
            std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        }
        LocalFree(out.pbData as HLOCAL);
        Some(bytes)
    }
}

/// The key as the key file holds it: protected for this user (`dpapi`).
#[cfg(windows)]
pub fn protect_key(key: &[u8; KEY_LEN]) -> Option<Vec<u8>> {
    dpapi(key, true)
}

/// The key a key file's bytes hold, when this user's DPAPI unprotects them with Tandem's entropy
/// into exactly a key. Plain bytes, another user's or another install's blob: None.
#[cfg(windows)]
pub fn unprotect_key(blob: &[u8]) -> Option<[u8; KEY_LEN]> {
    let plain = dpapi(blob, false)?;
    plain.as_slice().try_into().ok()
}

/// The install's approval key in `dir` (Tandem's settings folder), made when absent. The file holds
/// the key protected with DPAPI for this user (`protect_key`) and is read only when it is this
/// user's own owner-only file (`localfs::is_owner_only`), never through a link. So no file copied
/// in (an archive's, through any name) can be a key: one that is not owner-only or does not
/// unprotect is removed (through the handle that judged it, never a file another Tandem put there
/// since) and a fresh key made, so every earlier approval asks again (Task 9 findings r2). A new
/// key is written whole to an owner-only temporary file and then given the name only if no other
/// Tandem gave it one first (then that key is read), so no reader ever sees half a key. None when
/// the folder or file cannot be used: then nothing restores unasked.
#[cfg(windows)]
pub fn key_in(dir: &std::path::Path) -> Option<[u8; KEY_LEN]> {
    use crate::localfs;
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    let dir = localfs::LocalPath::judge(dir)?;
    localfs::create_dir_all(&dir).ok()?;
    let file = dir.unfollowed(KEY_FILE)?;
    // A file at the name: its key, or None after removing it.
    let read = |file: &localfs::Unfollowed| -> std::io::Result<Option<[u8; KEY_LEN]>> {
        const DELETE: u32 = 0x0001_0000;
        const READ_CONTROL: u32 = 0x0002_0000;
        let mut f = localfs::open_unfollowed(file, |o| {
            o.access_mode(windows_sys::Win32::Foundation::GENERIC_READ | READ_CONTROL | DELETE).share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
        })?;
        let mut blob = Vec::new();
        let key = if localfs::is_owner_only(&f) && (&mut f).take(4096).read_to_end(&mut blob).is_ok() { unprotect_key(&blob) } else { None };
        if key.is_none() {
            delete_open(&f)?;
        }
        Ok(key)
    };
    for _ in 0..8 {
        match read(&file) {
            Ok(Some(k)) => return Some(k),
            Ok(None) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // Another Tandem holds it open for writing or removing: read again.
            Err(e) if e.raw_os_error() == Some(32) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            Err(_) => return None,
        }
        let mut k = [0u8; KEY_LEN];
        getrandom::fill(&mut k).ok()?;
        let blob = protect_key(&k)?;
        let temp = dir.unfollowed(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4()))?;
        let written = localfs::create_owner_only(&temp).and_then(|mut f| f.write_all(&blob).and_then(|()| f.sync_all()));
        if written.is_err() {
            let _ = localfs::remove_unfollowed(&temp);
            return None;
        }
        let named = rename_new(&temp, &file);
        let _ = localfs::remove_unfollowed(&temp);
        match named {
            Ok(()) => return Some(k),
            // Another copy of Tandem named its key first: read that one.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Marks the open file (opened with DELETE access) for removal when its handles close: the very
/// file that was judged, whatever has the name by then.
#[cfg(windows)]
fn delete_open(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle};
    let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: an open handle and a structure of the size given.
    let ok = unsafe { SetFileInformationByHandle(f.as_raw_handle() as _, FileDispositionInfo, &info as *const _ as *const _, std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// `from` renamed to `to` only when nothing has that name (`MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING`): AlreadyExists otherwise. The file keeps its own DACL.
#[cfg(windows)]
fn rename_new(from: &crate::localfs::Unfollowed, to: &crate::localfs::Unfollowed) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (a, b) = (wide(from.path()), wide(to.path()));
    // SAFETY: both NUL-terminated and live across the call.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(a.as_ptr(), b.as_ptr(), 0) };
    if ok != 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    // ERROR_ALREADY_EXISTS (183) / ERROR_FILE_EXISTS (80).
    if matches!(e.raw_os_error(), Some(183) | Some(80)) { Err(std::io::ErrorKind::AlreadyExists.into()) } else { Err(e) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> Value {
        let mut t = Table::new();
        t.insert("Name".into(), Value::String(name.into()));
        Value::Table(t)
    }

    fn descriptor() -> Descriptor {
        Descriptor {
            workgroup_id: "builtin.claudeCode".into(),
            instance_id: "wg-0F1C2A3B-0000-4000-8000-000000000001".into(),
            git_base: "main".into(),
            leader_id: "MAIN".into(),
            anchor_id: "DIFF".into(),
            members: vec![
                Member { config_id: "MAIN".into(), display_name: "Chat".into(), frame: "{{0, 0}, {800, 600}}".into(), arrangement: session("chat"), pending: false },
                Member { config_id: "REVIEW".into(), display_name: "Code Review".into(), frame: "{{0, 0}, {800, 600}}".into(), arrangement: session("review"), pending: true },
            ],
            member_guids: vec!["6F1C2A3B-0000-4000-8000-0000000000A1".into(), "6F1C2A3B-0000-4000-8000-0000000000A2".into()],
            non_peer_children: vec![Child { config_id: "SPLIT".into(), guid: "6F1C2A3B-0000-4000-8000-0000000000B1".into() }],
            anchor_pending: true,
            approval: Some("ab".repeat(32)),
        }
    }

    /// Review Focus 8: the descriptor has iTerm2's keys, round-trips, and inhibits what iTerm2 inhibits.
    #[test]
    fn descriptor_follows_iterm2() {
        let d = descriptor();
        let v = d.to_value();
        let t = v.as_table().expect("a table");
        for key in [WORKGROUP_ID, INSTANCE_ID, GIT_BASE, LEADER_ID, ANCHOR_ID, MEMBERS, MEMBER_GUIDS, NON_PEER_CHILDREN, ANCHOR_PENDING] {
            assert!(t.contains_key(key), "{key}");
        }
        assert_eq!(t[WORKGROUP_ID].as_str(), Some("builtin.claudeCode"));
        assert_eq!(t[GIT_BASE].as_str(), Some("main"));
        assert_eq!(t[ANCHOR_PENDING].as_bool(), Some(true));
        let members = t[MEMBERS].as_array().unwrap();
        assert_eq!(members.len(), 2);
        for k in [CONFIG_ID, DISPLAY_NAME, FRAME, ARRANGEMENT, PENDING] {
            assert!(members[0].as_table().unwrap().contains_key(k), "{k}");
        }
        let child = &t[NON_PEER_CHILDREN].as_array().unwrap()[0];
        assert_eq!(child.get(CONFIG_ID).and_then(Value::as_str), Some("SPLIT"));
        assert!(child.get(GUID).is_some());
        assert_eq!(Descriptor::from_value(&v), Some(d.clone()));
        // No children: the key is absent (iTerm2 writes it only when there are some).
        let mut none = d.clone();
        none.non_peer_children.clear();
        assert!(!none.to_value().as_table().unwrap().contains_key(NON_PEER_CHILDREN));
        // Without the workgroup id there is nothing to restore.
        let mut broken = v.clone();
        broken.as_table_mut().unwrap().remove(WORKGROUP_ID);
        assert_eq!(Descriptor::from_value(&broken), None);
        // A saved Code Review anchor on its overlay, and a Diff anchor that was waiting, do not relaunch;
        // a Regular one does; one whose program came back from the host has nothing to inhibit.
        assert!(inhibit_relaunch(&d, Mode::CodeReview, false));
        assert!(inhibit_relaunch(&d, Mode::Diff, false));
        assert!(!inhibit_relaunch(&d, Mode::Regular, false));
        let mut live = d.clone();
        live.anchor_pending = false;
        assert!(inhibit_relaunch(&live, Mode::Diff, false), "a live program that is gone leaves its last output, never a stray shell");
        assert!(!inhibit_relaunch(&live, Mode::Diff, true), "a program that came back is not relaunched either way");
    }

    /// The file is another party's word: ids, counts and sizes are judged, and nothing panics.
    #[test]
    fn descriptor_is_read_bounded() {
        let good = descriptor().to_value();
        let with = |f: &dyn Fn(&mut Table)| {
            let mut v = good.clone();
            f(v.as_table_mut().unwrap());
            Descriptor::from_value(&v)
        };
        // Not a table, wrong types, empty and hostile ids.
        for v in [Value::Boolean(true), Value::String("x".into()), Value::Array(Vec::new()), Value::Table(Table::new())] {
            assert_eq!(Descriptor::from_value(&v), None);
        }
        assert_eq!(with(&|t| { t.insert(WORKGROUP_ID.into(), Value::Integer(3)); }), None);
        assert_eq!(with(&|t| { t.insert(LEADER_ID.into(), Value::String("a b".into())); }), None);
        assert_eq!(with(&|t| { t.insert(ANCHOR_ID.into(), Value::String("x".repeat(129))); }), None);
        // An instance id Tandem would not have minted is dropped (a fresh one is minted at adoption).
        let d = with(&|t| { t.insert(INSTANCE_ID.into(), Value::String("wg-../..".into())); }).unwrap();
        assert_eq!(d.instance_id, "");
        // A base that is an option or holds a control reads as HEAD.
        assert_eq!(with(&|t| { t.insert(GIT_BASE.into(), Value::String("--output=x".into())); }).unwrap().git_base, "HEAD");
        assert_eq!(with(&|t| { t.remove(GIT_BASE); }).unwrap().git_base, "HEAD");
        // A malformed approval digest is none.
        assert_eq!(with(&|t| { t.insert(APPROVAL.into(), Value::String("not hex".into())); }).unwrap().approval, None);
        // Too many members: the excess is dropped, not the descriptor; a member with a bad id or no arrangement is skipped.
        let many = with(&|t| {
            let m: Vec<Value> = (0..200)
                .map(|i| {
                    let mut m = Table::new();
                    m.insert(CONFIG_ID.into(), Value::String(format!("M{i}")));
                    m.insert(ARRANGEMENT.into(), session("x"));
                    Value::Table(m)
                })
                .collect();
            t.insert(MEMBERS.into(), Value::Array(m));
        })
        .unwrap();
        assert_eq!(many.members.len(), crate::workgroup::MAX_SESSIONS);
        let skipped = with(&|t| {
            let mut bad_id = Table::new();
            bad_id.insert(CONFIG_ID.into(), Value::String("no good".into()));
            bad_id.insert(ARRANGEMENT.into(), session("x"));
            let mut no_arrangement = Table::new();
            no_arrangement.insert(CONFIG_ID.into(), Value::String("OK".into()));
            t.insert(MEMBERS.into(), Value::Array(vec![Value::Table(bad_id), Value::Table(no_arrangement), Value::Integer(1)]));
        })
        .unwrap();
        assert!(skipped.members.is_empty());
        // An arrangement nested past the depth limit is skipped whole.
        let mut deep = session("deep");
        for _ in 0..200 {
            let mut t = Table::new();
            t.insert("x".into(), deep);
            deep = Value::Table(t);
        }
        let deep_member = with(&|t| {
            let mut m = Table::new();
            m.insert(CONFIG_ID.into(), Value::String("DEEP".into()));
            m.insert(ARRANGEMENT.into(), deep.clone());
            t.insert(MEMBERS.into(), Value::Array(vec![Value::Table(m)]));
        })
        .unwrap();
        assert!(deep_member.members.is_empty());
        // Children and guids are bounded and judged too.
        let kids = with(&|t| {
            let kids: Vec<Value> = (0..100)
                .map(|i| {
                    let mut k = Table::new();
                    k.insert(CONFIG_ID.into(), Value::String(format!("K{i}")));
                    k.insert(GUID.into(), Value::String(if i == 0 { "bad guid\n".into() } else { format!("G{i}") }));
                    Value::Table(k)
                })
                .collect();
            t.insert(NON_PEER_CHILDREN.into(), Value::Array(kids));
            t.insert(MEMBER_GUIDS.into(), Value::Array((0..500).map(|i| Value::String(format!("G{i}"))).collect()));
        })
        .unwrap();
        assert_eq!(kids.non_peer_children.len(), crate::workgroup::MAX_SESSIONS);
        assert!(kids.non_peer_children.iter().all(|c| c.guid != "bad guid\n"));
        assert!(kids.member_guids.len() <= 2 * crate::workgroup::MAX_SESSIONS);
    }

    /// `WC:adopt`: the saved id, a fresh one when it is taken or not one Tandem makes.
    #[test]
    fn instance_ids_follow_iterm2() {
        let saved = "wg-0F1C2A3B-0000-4000-8000-000000000001";
        assert_eq!(resolve_instance_id(saved, &|_| false), saved);
        let fresh = resolve_instance_id(saved, &|id| id == saved);
        assert!(fresh.starts_with(crate::workgroup_run::INSTANCE_PREFIX) && fresh != saved && crate::workgroup_run::valid_instance_id(&fresh), "{fresh}");
        let other = resolve_instance_id("not-an-id", &|_| false);
        assert!(crate::workgroup_run::valid_instance_id(&other) && other != "not-an-id");
        assert!(crate::workgroup_run::valid_instance_id(&resolve_instance_id("", &|_| false)));
    }

    fn model() -> Workgroup {
        let mut w = crate::workgroup::presets::coding_agent_plus_diff_plus_code_review(Some(("builtin.claudeCode", "MAIN", "DIFF", "REVIEW")), Some("Claude Code"));
        w.unique_identifier = "builtin.claudeCode".into();
        w
    }

    /// `WRC:reconstruct`: the config comes from the model; the anchor stays shown and the leader goes buried.
    #[test]
    fn adopt_follows_iterm2() {
        let d = descriptor();
        let a = adoption(&d, &|id| (id == "builtin.claudeCode").then(model)).expect("adopts");
        assert_eq!((a.active.as_str(), a.leader.as_str(), a.git_base.as_str()), ("DIFF", "MAIN", "main"));
        assert_eq!(a.peers, ["MAIN", "DIFF", "REVIEW"]);
        assert_eq!(a.buried, ["MAIN", "REVIEW"], "every peer but the shown one is buried, the leader among them");
        assert_eq!(a.workgroup, model());
        // Embedded members: the leader from its saved arrangement; the pending review comes back fresh.
        assert_eq!(a.restored, ["MAIN"]);
        assert_eq!(a.fresh, ["REVIEW"]);
        // A workgroup deleted since the save adopts nothing.
        assert_eq!(adoption(&d, &|_| None).err(), Some(Refusal::Gone));
        // A descriptor that does not fit the workgroup it names adopts nothing.
        let mut other_leader = d.clone();
        other_leader.leader_id = "DIFF".into();
        assert_eq!(adoption(&other_leader, &|_| Some(model())).err(), Some(Refusal::Mismatch));
        let mut unknown_anchor = d.clone();
        unknown_anchor.anchor_id = "NOPE".into();
        assert_eq!(adoption(&unknown_anchor, &|_| Some(model())).err(), Some(Refusal::Mismatch));
        let mut twice = d.clone();
        twice.members.push(d.members[0].clone());
        assert_eq!(adoption(&twice, &|_| Some(model())).err(), Some(Refusal::Mismatch));
        let mut anchor_embedded = d.clone();
        anchor_embedded.members[1].config_id = "DIFF".into();
        assert_eq!(adoption(&anchor_embedded, &|_| Some(model())).err(), Some(Refusal::Mismatch));
        // A leader that is neither the anchor nor restored cannot be rebuilt (WRC: corrupt descriptor).
        let mut no_leader = d.clone();
        no_leader.members.remove(0);
        assert_eq!(adoption(&no_leader, &|_| Some(model())).err(), Some(Refusal::NoLeader));
        // Children are kept only for a split or tab of the model.
        let mut kids = d.clone();
        kids.non_peer_children = vec![Child { config_id: "DIFF".into(), guid: "G".into() }, Child { config_id: "GONE".into(), guid: "H".into() }];
        assert!(adoption(&kids, &|_| Some(model())).unwrap().children.is_empty());
    }

    /// The trust ruling: only the person's own workgroup restores without the stored approval, and an
    /// import's approval stands only as this install's seal in its own launch state.
    #[test]
    fn restored_imports_need_their_stored_approval() {
        let w = model();
        let key = [7u8; KEY_LEN];
        let sealed = seal(&key, &w.unique_identifier, crate::workgroup_run::Approved::of(&w).digest());
        let mut d = descriptor();
        d.approval = Some(sealed.clone());
        assert!(!needs_question(Source::Person, &d, &w, Origin::Named, None));
        assert!(!needs_question(Source::Import, &d, &w, Origin::Launch, Some(&key)), "the answer the person gave is still the one on file");
        d.approval = None;
        assert!(needs_question(Source::Import, &d, &w, Origin::Launch, Some(&key)));
        assert!(needs_question(Source::DynamicProfile, &d, &w, Origin::Launch, Some(&key)));
        d.approval = Some("0".repeat(64));
        assert!(needs_question(Source::RemotePrefs, &d, &w, Origin::Launch, Some(&key)), "a command changed since");
        let mut changed = w.clone();
        changed.sessions[1].command.push_str(" --x");
        d.approval = Some(sealed.clone());
        assert!(needs_question(Source::Import, &d, &changed, Origin::Launch, Some(&key)));
        let mut renamed = w.clone();
        renamed.unique_identifier = "other".into();
        assert!(needs_question(Source::Import, &d, &renamed, Origin::Launch, Some(&key)), "a seal is for one workgroup");
        d.approval = None;
        assert!(needs_question(Source::Stored, &d, &w, Origin::Launch, Some(&key)), "an unknown mark fails closed");
    }

    /// Task 9 review C1: the plain digest of the commands is computable by anyone who holds the
    /// workgroup, so a descriptor carrying it is no answer of the person's; nor is a seal under another
    /// install's key, a launch with no key, or any approval in a named arrangement (remote prefs, Import
    /// All and the API can all deliver one, and OpenArrangementAtStartup opens it by name).
    #[test]
    fn a_computable_digest_is_no_approval() {
        let w = model();
        let key = [7u8; KEY_LEN];
        let digest = crate::workgroup_run::Approved::of(&w).digest().to_string();
        let sealed = seal(&key, &w.unique_identifier, &digest);
        let mut d = descriptor();
        for source in [Source::Import, Source::RemotePrefs, Source::DynamicProfile, Source::Stored] {
            d.approval = Some(digest.clone());
            assert!(needs_question(source, &d, &w, Origin::Launch, Some(&key)), "{source:?}: a forged digest still asks");
            assert!(needs_question(source, &d, &w, Origin::Named, Some(&key)), "{source:?}");
            d.approval = Some(seal(&[8u8; KEY_LEN], &w.unique_identifier, &digest));
            assert!(needs_question(source, &d, &w, Origin::Launch, Some(&key)), "{source:?}: another install's seal");
            d.approval = Some(sealed.clone());
            assert!(needs_question(source, &d, &w, Origin::Named, Some(&key)), "{source:?}: a named arrangement always asks");
            assert!(needs_question(source, &d, &w, Origin::Launch, None), "{source:?}: no key, no approval");
        }
        assert_ne!(sealed, digest);
        assert!(digest_ok(&sealed), "a seal reads back as an approval");
    }

    /// The key is made owner-only once and read back the same; a file that does not unprotect is replaced.
    #[cfg(windows)]
    #[test]
    fn the_approval_key_is_made_once_and_owner_only() {
        let dir = std::env::temp_dir().join(format!("tandem-wg-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = key_in(&dir).expect("made");
        assert_eq!(key_in(&dir), Some(first), "read back, not made again");
        let file = crate::localfs::LocalPath::judge(&dir).unwrap().unfollowed(KEY_FILE).unwrap();
        let sddl = crate::localfs::dacl_sddl(&file).unwrap();
        assert!(sddl.contains("D:P(A;;FA;;;S-") && sddl.matches("(A;").count() == 1, "{sddl}");
        std::fs::write(dir.join(KEY_FILE), b"short").unwrap();
        let again = key_in(&dir).expect("replaced");
        assert_ne!(again, first);
        assert_eq!(unprotect_key(&std::fs::read(dir.join(KEY_FILE)).unwrap()), Some(again), "the file holds the key protected");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Task 9 findings r2 (Critical): a key file that came from anywhere but this install's own
    /// protect call is no key. Plain bytes (an archive's, an older Tandem's) do not unprotect, and
    /// the install's own protected bytes in a file without the owner-only DACL (copied in) are not
    /// read: each time a fresh owner-only key replaces the file, so every earlier approval asks.
    #[cfg(windows)]
    #[test]
    fn a_key_file_copied_in_is_no_key() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("tandem-wg-key-copied-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = crate::localfs::LocalPath::judge(&dir).unwrap().unfollowed(KEY_FILE).unwrap();
        // Planted plain bytes, even in an owner-only file.
        let planted = [9u8; KEY_LEN];
        crate::localfs::create_owner_only(&file).unwrap().write_all(&planted).unwrap();
        let made = key_in(&dir).expect("a fresh key");
        assert_ne!(made, planted, "plain bytes in the key file are never the key");
        assert_eq!(key_in(&dir), Some(made), "the fresh key reads back");
        // The install's own protected bytes, but in a file with an inherited DACL.
        let bytes = std::fs::read(dir.join(KEY_FILE)).unwrap();
        assert_ne!(bytes.len(), KEY_LEN, "the file holds the protected key, not the key");
        std::fs::remove_file(dir.join(KEY_FILE)).unwrap();
        std::fs::write(dir.join(KEY_FILE), &bytes).unwrap();
        let again = key_in(&dir).expect("a fresh key");
        assert_ne!(again, made, "a key file without the owner-only DACL is not read");
        let sddl = crate::localfs::dacl_sddl(&file).unwrap();
        assert!(sddl.contains("D:P(A;;FA;;;S-") && sddl.matches("(A;").count() == 1, "{sddl}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Task 9 findings r2 (minor): copies of Tandem starting together all end with the one key on
    /// disk: a key is named only whole and only when no other has the name.
    #[cfg(windows)]
    #[test]
    fn tandems_starting_together_share_one_key() {
        for round in 0..10 {
            let dir = std::env::temp_dir().join(format!("tandem-wg-key-race-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let got: Vec<Option<[u8; KEY_LEN]>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..6).map(|_| s.spawn(|| key_in(&dir))).collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let on_disk = key_in(&dir).expect("a key");
            assert!(got.iter().all(|k| *k == Some(on_disk)), "round {round}: {got:?}");
            let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            assert_eq!(names, [KEY_FILE], "no temporary file is left");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Task 9 findings r2 (Critical): the key is DPAPI-protected for this user with Tandem's own
    /// entropy, so the same bytes protected without it (or by anyone else) do not unprotect.
    #[cfg(windows)]
    #[test]
    fn the_key_unprotects_only_with_its_entropy() {
        let key = [5u8; KEY_LEN];
        let blob = protect_key(&key).expect("protected");
        assert_eq!(unprotect_key(&blob), Some(key));
        assert_eq!(unprotect_key(&key), None, "plain bytes");
        let mut bent = blob.clone();
        let last = bent.len() - 1;
        bent[last] ^= 1;
        assert_eq!(unprotect_key(&bent), None, "a changed blob");
    }

    /// Task 9 review I2: the decision steps for one restore, in order, and where the claims are kept.
    #[test]
    fn restore_steps_keep_the_claims_through_the_question() {
        use std::time::Duration;
        let base = Facts { anchor_alive: true, in_window: true, in_workgroup: false, asked: false, age: Duration::ZERO, answer: None, fits: true, question: Question::None };
        let step = |f: &dyn Fn(&mut Facts)| {
            let mut x = base;
            f(&mut x);
            next_step(&x)
        };
        assert_eq!(step(&|_| {}), Step::Adopt, "the person's own, or an approval that stands");
        // The question: asked once, and the claims stay until the answer.
        assert_eq!(step(&|f| f.question = Question::Showable), Step::Ask);
        assert!(Step::Ask.keeps_claims() && Step::Hold.keeps_claims(), "nothing enters the anchor while the person reads");
        assert_eq!(step(&|f| (f.question, f.asked) = (Question::Showable, true)), Step::Hold);
        assert_eq!(step(&|f| (f.question, f.asked, f.in_workgroup) = (Question::Showable, true, true)), Step::Hold, "nothing is decided before the answer");
        assert_eq!(step(&|f| f.question = Question::TooLong), Step::Discard(Why::TooLong));
        // The answer.
        assert_eq!(step(&|f| (f.question, f.asked, f.answer) = (Question::Showable, true, Some(true))), Step::Adopt);
        assert_eq!(step(&|f| (f.question, f.asked, f.answer) = (Question::Showable, true, Some(false))), Step::Discard(Why::Declined));
        // The session entered a workgroup while the question was up, then Restore: the hidden members'
        // shells end (Discard), nothing is adopted twice and nothing is orphaned.
        assert_eq!(step(&|f| (f.question, f.asked, f.answer, f.in_workgroup) = (Question::Showable, true, Some(true), true)), Step::Discard(Why::Taken));
        assert_eq!(step(&|f| f.in_workgroup = true), Step::Discard(Why::Taken), "another pass rebuilt it: the members' shells still end");
        // The anchor closed, answered or not.
        assert_eq!(step(&|f| (f.anchor_alive, f.asked) = (false, true)), Step::Discard(Why::AnchorGone));
        assert_eq!(step(&|f| (f.anchor_alive, f.answer) = (false, Some(true))), Step::Discard(Why::AnchorGone));
        // A config gone or not fitting.
        assert_eq!(step(&|f| f.fits = false), Step::Discard(Why::Unfit));
        assert_eq!(step(&|f| (f.fits, f.answer) = (false, Some(true))), Step::Discard(Why::Unfit));
        // Waiting for the window: a soft release, then giving up.
        assert_eq!(step(&|f| f.in_window = false), Step::Wait { release: false });
        assert!(Step::Wait { release: false }.keeps_claims() && !Step::Wait { release: true }.keeps_claims());
        assert_eq!(step(&|f| (f.in_window, f.age) = (false, RELEASE_AFTER)), Step::Wait { release: true });
        assert_eq!(step(&|f| (f.in_window, f.age) = (false, GIVE_UP)), Step::Discard(Why::GaveUp));
        // An answer is taken where the window stands now.
        assert_eq!(step(&|f| (f.in_window, f.asked, f.answer) = (false, true, Some(true))), Step::Adopt);
        for done in [Step::Adopt, Step::Discard(Why::Taken), Step::Discard(Why::Declined)] {
            assert!(!done.keeps_claims(), "{done:?}");
        }
    }

    /// The embedded members' shells are claimed by the restore's host plan, each by its own table.
    #[test]
    fn embedded_members_are_reachable_for_the_host_plan() {
        let mut anchor = Table::new();
        anchor.insert("Name".into(), Value::String("anchor".into()));
        anchor.insert(crate::arrangement::WORKGROUP_KEY.into(), descriptor().to_value());
        assert_eq!(embedded(&anchor).len(), 2);
        assert_eq!(embedded(&anchor)[0].get("Name").and_then(Value::as_str), Some("chat"));
        for (i, m) in embedded_mut(&mut anchor).into_iter().enumerate() {
            m.insert("Tandem Attach".into(), Value::Integer(i as i64));
        }
        let d = Descriptor::from_value(anchor.get(crate::arrangement::WORKGROUP_KEY).unwrap()).unwrap();
        assert_eq!(d.members[1].arrangement.get("Tandem Attach").and_then(Value::as_integer), Some(1));
        // A session with no descriptor, or a broken one, has none.
        assert!(embedded(&Table::new()).is_empty());
        let mut broken = Table::new();
        broken.insert(crate::arrangement::WORKGROUP_KEY.into(), Value::Boolean(true));
        assert!(embedded(&broken).is_empty() && embedded_mut(&mut broken).is_empty());
    }

    /// Task 9 review minor 2: the host plan claims exactly the members a restore reads, so a member
    /// `from_value` skips never has its shell claimed and left running.
    #[test]
    fn embedded_members_are_the_ones_read() {
        let mut anchor = Table::new();
        let mut v = descriptor().to_value();
        let members = v.get_mut(MEMBERS).and_then(Value::as_array_mut).unwrap();
        let mut bad = members[0].as_table().unwrap().clone();
        bad.insert(CONFIG_ID.into(), Value::String("bad id with spaces".into()));
        members.insert(0, Value::Table(bad));
        anchor.insert(crate::arrangement::WORKGROUP_KEY.into(), v);
        let read = Descriptor::from_value(anchor.get(crate::arrangement::WORKGROUP_KEY).unwrap()).unwrap();
        assert_eq!(read.members.len(), 2);
        assert_eq!(embedded(&anchor).len(), read.members.len(), "a skipped member is not claimed");
        assert_eq!(embedded_mut(&mut anchor).len(), read.members.len());
        assert_eq!(embedded(&anchor)[0].get("Name").and_then(Value::as_str), Some("chat"));
    }

    /// The anchor's session value carries the descriptor and the app wires it, saving and restoring.
    #[test]
    fn the_app_wires_restoration_as_data() {
        let arrange = include_str!("app/arrange_ui.rs");
        let production = arrange.split("#[cfg(test)]").next().unwrap();
        assert!(production.contains("arrangement::WORKGROUP_KEY"), "the descriptor goes under iTerm2's key");
        assert!(production.contains("snapshot_window_copy"), "a duplicate carries none (P9)");
        let app = include_str!("app/workgroup_restore_app.rs");
        let prod = app.split("#[cfg(test)]").next().unwrap();
        for banned in ["bury_pane", "self.buried", "Burial", "disinter", "enter_workgroup_now", "start_command_override"] {
            assert!(!prod.contains(banned), "{banned}");
        }
        assert!(prod.contains("needs_question("), "an import asks again unless its approval stands");
        assert!(prod.contains("if p.m.launch { wr::Origin::Launch } else { wr::Origin::Named }"), "only the launch state's approval is read");
        assert!(prod.contains("crate::workgroup::restored_toggles("), "toggles come back only on adopted members");
        assert!(include_str!("app/restore_ui.rs").contains("workgroup_restore::embedded"), "embedded members take the host-key plan");
        assert!(include_str!("app/workgroup_app.rs").contains("self.is_restoring_session(pane)"), "a trigger checks the restoring flag when it fires");
    }
}
