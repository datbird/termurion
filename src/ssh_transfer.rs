//! The transfer engine (milestone 14d, `FT:11-346`, `CFT:81-195`, `CD:447-450`): one app-wide
//! set of download jobs that turns each conductor reply into `Action`s and never touches the
//! disk or the wire itself. Pure. Everything a reply holds is hostile: sizes are never used to
//! allocate, every buffer is bounded by the chunk size times the window, and a reply for a
//! finished job is ignored.

use crate::layout::PaneId;
use crate::remote_file::{decode_fetch, RemoteFile};
use crate::ssh_files::{check, Chunk, FileError, FileSub, GARBAGE};
use std::collections::{HashMap, VecDeque};

/// Free space a download never uses: before each write of a stream with no claimed size the
/// drive must keep this much free after it.
pub const RESERVE: u64 = 1 << 30;

/// Whether `add` more bytes fit on a drive with `free` bytes free and still leave `RESERVE`.
pub fn has_room(free: u64, add: u64) -> bool {
    free.checked_sub(RESERVE).is_some_and(|f| f >= add)
}

/// A single-stream fetch asks this many bytes (`CFT:81-99`).
pub const STREAM_CHUNK: u64 = 1024;
/// A chunked fetch asks this many (`FT:130-306`).
pub const CHUNK: u64 = 4096;
/// Fetches a chunked download keeps in flight.
pub const MAX_IN_FLIGHT: usize = 8;
/// Chunked downloads running at once (`CD:447-450`).
pub const MAX_DOWNLOADS: usize = 2;
/// An upload appends this many bytes per command (`FT:348-418`).
pub const UPLOAD_CHUNK: usize = 1024;
/// Names an upload tries for a free one: `f`, `f (2)` ... `f (100)` (`FT:348-418`).
pub const NAME_TRIES: usize = 100;
/// `FT:348-418`.
pub const TOO_MANY: &str = "Too many iterations to find a valid file name on remote host for upload";

/// The hidden name a file is uploaded under before it moves into place.
pub fn upload_temp(dest: &str, uuid: &str) -> String {
    format!("{dest}.uploading-{uuid}")
}

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    File,
    Directory,
    ZipOfFolder,
    Uploaded { remote_name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send { job: JobId, tag: u64, pane: PaneId, sub: FileSub, high: bool },
    Fire { pane: PaneId, sub: FileSub },
    Size { job: JobId, size: Option<u64> },
    Write { job: JobId, bytes: Vec<u8> },
    Sent { job: JobId, bytes: u64 },
    Unzip { job: JobId },
    Done { job: JobId, result: Result<Outcome, String> },
}

/// `FT:200-206` (`Download ended prematurely`).
pub fn premature(received: u64, size: u64) -> String {
    format!("Download ended prematurely (received {received} of {size} bytes)")
}

/// `FT:140-142`.
pub fn not_regular(path: &str, host: &str) -> String {
    format!("{path} on {host} is not a regular file")
}

/// The longest remote zip path taken (the remote chose it; it only goes back to the remote).
const ZIP_PATH_MAX: usize = 4096;

/// What a job does once it holds its slot.
#[derive(Clone, Copy, Debug)]
enum Ready {
    Zip,
    File(u64),
}

/// A chunked file's window (`DownloadState`, `FT:180-206`). Everything here is bounded by
/// `MAX_IN_FLIGHT` chunks of `CHUNK` bytes; the claimed size only bounds how many fetches are issued.
struct Chunks {
    size: u64,
    next: u64,
    pending: VecDeque<u64>,
    arrived: HashMap<u64, Vec<u8>>,
    received: u64,
}

/// What a step leaves its job as.
enum Flow {
    Keep,
    End(Result<Outcome, String>),
}

fn fail(e: impl ToString) -> Flow {
    Flow::End(Err(e.to_string()))
}

enum State {
    Stat,
    Zip,
    ZipStat,
    Waiting(Ready),
    Stream { offset: u64, size: Option<u64> },
    Chunks(Chunks),
    Unzipping,
    /// An upload's `create` is out.
    UpCreate,
    /// An upload's `append` of the chunk at `offset` is out.
    UpAppend { offset: usize },
    /// An upload's name probe: the stat of try `n` (0 based) is out.
    UpProbe { n: usize },
    /// An upload's `mv` onto `name` is out.
    UpMove { name: String },
}

/// What an upload keeps (the data is read whole; the caller caps it, P19).
struct Upload {
    temp: String,
    data: Vec<u8>,
}

struct Job {
    pane: PaneId,
    path: String,
    host: String,
    chunked: bool,
    /// The remote zip of a folder, once it exists: removed when the job ends, success or not.
    zip: Option<String>,
    /// What is fetched now: the path, or the zip.
    fetch_path: String,
    holds_slot: bool,
    state: State,
    up: Option<Upload>,
}

pub struct Transfers {
    jobs: HashMap<JobId, Job>,
    /// Request tag to the job waiting for it.
    tags: HashMap<u64, JobId>,
    next_job: JobId,
    next_tag: u64,
    running: usize,
    /// Chunked jobs that have their file or folder but no slot yet, in arrival order.
    queue: VecDeque<JobId>,
}

impl Default for Transfers {
    fn default() -> Self {
        Transfers::new()
    }
}

impl Transfers {
    pub fn new() -> Transfers {
        Transfers { jobs: HashMap::new(), tags: HashMap::new(), next_job: 1, next_tag: 1, running: 0, queue: VecDeque::new() }
    }

    /// Win-click: one stream, no slot (`CFT:81-99`).
    pub fn download(&mut self, pane: PaneId, remote: &str) -> (JobId, Vec<Action>) {
        self.add(pane, remote, "", false)
    }

    /// Download Files: chunked, two at a time (`FT:130-306`).
    pub fn download_chunked(&mut self, pane: PaneId, remote: &str, host: &str) -> (JobId, Vec<Action>) {
        self.add(pane, remote, host, true)
    }

    fn add(&mut self, pane: PaneId, remote: &str, host: &str, chunked: bool) -> (JobId, Vec<Action>) {
        let id = self.next_job;
        self.next_job += 1;
        let job = Job { pane, path: remote.to_string(), host: host.to_string(), chunked, zip: None, fetch_path: remote.to_string(), holds_slot: false, state: State::Stat, up: None };
        let mut out = Vec::new();
        self.send(id, &job, FileSub::Stat { path: remote.to_string() }, &mut out);
        self.jobs.insert(id, job);
        (id, out)
    }

    /// Alt-drop: `data` goes to `dest` over the conductor (`FT:348-418`): a hidden temp is
    /// created, appended in 1024-byte chunks, a free name is probed with `stat` and the temp
    /// moves there. Any failure removes the temp. No slot: uploads are not queued.
    pub fn upload(&mut self, pane: PaneId, dest: &str, data: Vec<u8>, uuid: &str) -> (JobId, Vec<Action>) {
        let id = self.next_job;
        self.next_job += 1;
        let temp = upload_temp(dest, uuid);
        let job = Job { pane, path: dest.to_string(), host: String::new(), chunked: true, zip: None, fetch_path: dest.to_string(), holds_slot: false, state: State::UpCreate, up: Some(Upload { temp: temp.clone(), data }) };
        let mut out = Vec::new();
        self.send(id, &job, FileSub::Create { path: temp, content: Vec::new() }, &mut out);
        self.jobs.insert(id, job);
        (id, out)
    }

    pub fn running(&self) -> usize {
        self.running
    }

    /// The local side could not go on (no room, a write or the mark failed): the job ends with
    /// `reason`, its slot is freed and its remote zip removed, and the queue moves on. Replies
    /// for it that are still on the way are ignored. A job that is not live gives `[]`.
    pub fn fail(&mut self, job: JobId, reason: &str) -> Vec<Action> {
        let Some(j) = self.jobs.remove(&job) else { return Vec::new() };
        let mut out = Vec::new();
        self.end(job, j, Err(reason.to_string()), &mut out);
        self.pump(&mut out);
        out
    }

    pub fn is_live(&self, job: JobId) -> bool {
        self.jobs.contains_key(&job)
    }

    /// Keeps the job or ends it, as `flow` says.
    fn settle(&mut self, id: JobId, j: Job, flow: Flow, out: &mut Vec<Action>) {
        match flow {
            Flow::Keep => {
                self.jobs.insert(id, j);
            }
            Flow::End(result) => self.end(id, j, result, out),
        }
    }

    fn send(&mut self, id: JobId, job: &Job, sub: FileSub, out: &mut Vec<Action>) {
        let tag = self.next_tag;
        self.next_tag += 1;
        self.tags.insert(tag, id);
        out.push(Action::Send { job: id, tag, pane: job.pane, sub, high: false });
    }

    /// A reply to the request `tag` of `job`. A reply for a finished job or an unknown tag is `[]`.
    pub fn reply(&mut self, job: JobId, tag: u64, content: &str, status: i32) -> Vec<Action> {
        if self.tags.get(&tag) != Some(&job) {
            return Vec::new();
        }
        self.tags.remove(&tag);
        let Some(mut j) = self.jobs.remove(&job) else { return Vec::new() };
        let mut out = Vec::new();
        let flow = self.step(job, &mut j, tag, content, status, &mut out);
        self.settle(job, j, flow, &mut out);
        self.pump(&mut out);
        out
    }

    /// The local unzip finished (`FT:150-172`).
    pub fn unzipped(&mut self, job: JobId, result: Result<(), String>) -> Vec<Action> {
        if !matches!(self.jobs.get(&job).map(|j| &j.state), Some(State::Unzipping)) {
            return Vec::new();
        }
        let j = self.jobs.remove(&job).unwrap();
        let mut out = Vec::new();
        match result {
            Ok(()) => self.end(job, j, Ok(Outcome::Directory), &mut out),
            Err(e) => self.end(job, j, Err(e), &mut out),
        }
        self.pump(&mut out);
        out
    }

    /// Ends a job: its remote zip is removed, its tags forgotten, its slot freed. The caller pumps.
    fn end(&mut self, id: JobId, j: Job, result: Result<Outcome, String>, out: &mut Vec<Action>) {
        // A failed upload removes its hidden temp (a finished one has moved it).
        let leftover = j.up.as_ref().filter(|_| result.is_err()).map(|u| u.temp.clone());
        let fire = j.zip.or(leftover).map(|z| Action::Fire { pane: j.pane, sub: FileSub::Rm { path: z, recursive: false } });
        let done = Action::Done { job: id, result };
        // A chunked download removes the zip before it reports (`FT:150-172`), a stream after (`FT:338-342`).
        if j.chunked {
            out.extend(fire);
            out.push(done);
        } else {
            out.push(done);
            out.extend(fire);
        }
        self.tags.retain(|_, v| *v != id);
        if j.holds_slot {
            self.running -= 1;
        }
    }

    /// Starts waiting jobs while slots are free, in arrival order.
    fn pump(&mut self, out: &mut Vec<Action>) {
        while self.running < MAX_DOWNLOADS {
            let Some(id) = self.queue.pop_front() else { break };
            let Some(mut j) = self.jobs.remove(&id) else { continue };
            let State::Waiting(ready) = j.state else {
                self.jobs.insert(id, j);
                continue;
            };
            self.running += 1;
            j.holds_slot = true;
            let flow = self.begin(id, &mut j, ready, out);
            self.settle(id, j, flow, out);
        }
    }

    /// A chunked job that has its slot starts its zip or its chunks.
    fn begin(&mut self, id: JobId, j: &mut Job, ready: Ready, out: &mut Vec<Action>) -> Flow {
        match ready {
            Ready::Zip => {
                j.state = State::Zip;
                let path = j.path.clone();
                self.send(id, j, FileSub::Zip { path }, out);
                Flow::Keep
            }
            Ready::File(size) => self.start_chunks(id, j, size, out),
        }
    }

    fn start_chunks(&mut self, id: JobId, j: &mut Job, size: u64, out: &mut Vec<Action>) -> Flow {
        out.push(Action::Size { job: id, size: Some(size) });
        let mut c = Chunks { size, next: 0, pending: VecDeque::new(), arrived: HashMap::new(), received: 0 };
        self.issue(id, j, &mut c, out);
        self.after_chunks(id, j, c, out)
    }

    /// Keeps the window, or finishes when nothing is pending: a file is done, a zip is
    /// unzipped by the local side (`Unzip`, then `unzipped`).
    fn after_chunks(&mut self, id: JobId, j: &mut Job, c: Chunks, out: &mut Vec<Action>) -> Flow {
        if !c.pending.is_empty() {
            j.state = State::Chunks(c);
            return Flow::Keep;
        }
        if j.zip.is_some() {
            j.state = State::Unzipping;
            out.push(Action::Unzip { job: id });
            Flow::Keep
        } else {
            Flow::End(Ok(Outcome::File))
        }
    }

    /// Issues fetches while fewer than eight are pending and the next offset is below the
    /// claimed size. The claim only bounds the count of fetches; nothing is sized by it.
    fn issue(&mut self, id: JobId, j: &Job, c: &mut Chunks, out: &mut Vec<Action>) {
        while c.pending.len() < MAX_IN_FLIGHT && c.next < c.size {
            let tag = self.next_tag;
            let sub = FileSub::Fetch { path: j.fetch_path.clone(), chunk: Some(Chunk { offset: c.next, size: CHUNK }), unique_id: Some(format!("{id}-{tag}")) };
            self.send(id, j, sub, out);
            c.pending.push_back(tag);
            c.next = c.next.saturating_add(CHUNK);
        }
    }

    /// Runs the step for `tag`'s reply on the job's state.
    fn step(&mut self, id: JobId, j: &mut Job, tag: u64, content: &str, status: i32, out: &mut Vec<Action>) -> Flow {
        if j.up.is_some() {
            return self.step_upload(id, j, status, out);
        }
        if let Err(e) = check(status) {
            return fail(e.text());
        }
        match std::mem::replace(&mut j.state, State::Stat) {
            State::Stat => self.on_stat(id, j, content, out),
            State::Zip => {
                if content.is_empty() || content.len() > ZIP_PATH_MAX {
                    return fail(GARBAGE);
                }
                j.zip = Some(content.to_string());
                j.fetch_path = content.to_string();
                j.state = State::ZipStat;
                self.send(id, j, FileSub::Stat { path: content.to_string() }, out);
                Flow::Keep
            }
            State::ZipStat => self.on_zip_stat(id, j, content, out),
            State::Stream { offset, size } => self.on_stream(id, j, offset, size, content, out),
            State::Chunks(c) => self.on_chunk(id, j, c, tag, content, out),
            // Waiting and unzipping have no request out, so no reply can reach them.
            other => {
                j.state = other;
                Flow::Keep
            }
        }
    }

    /// An upload's reply (`FT:348-418`). A stat that errors counts as free (`try?`); the `mv`
    /// reply's content is ignored (iTerm2 parses it and uses nothing from it).
    fn step_upload(&mut self, id: JobId, j: &mut Job, status: i32, out: &mut Vec<Action>) -> Flow {
        let state = std::mem::replace(&mut j.state, State::Stat);
        if let State::UpProbe { n } = state {
            if status == 0 {
                // Taken: the next name, or give up after `NAME_TRIES` stats.
                if n + 1 >= NAME_TRIES {
                    return fail(TOO_MANY);
                }
                return self.probe(id, j, n + 1, out);
            }
            let (temp, name) = (j.up.as_ref().map(|u| u.temp.clone()).unwrap_or_default(), Self::upload_name(&j.path, n));
            j.state = State::UpMove { name: name.clone() };
            self.send(id, j, FileSub::Mv { source: temp, dest: name }, out);
            return Flow::Keep;
        }
        if let Err(e) = check(status) {
            return fail(e.text());
        }
        match state {
            State::UpCreate => self.upload_next(id, j, 0, out),
            State::UpAppend { offset } => {
                let len = j.up.as_ref().map_or(0, |u| UPLOAD_CHUNK.min(u.data.len() - offset));
                out.push(Action::Sent { job: id, bytes: len as u64 });
                self.upload_next(id, j, offset + len, out)
            }
            State::UpMove { name } => {
                if let Some(u) = j.up.as_mut() {
                    u.temp.clear();
                }
                Flow::End(Ok(Outcome::Uploaded { remote_name: name }))
            }
            other => {
                j.state = other;
                Flow::Keep
            }
        }
    }

    /// The next chunk at `offset`, or the name probe when the data is all sent.
    fn upload_next(&mut self, id: JobId, j: &mut Job, offset: usize, out: &mut Vec<Action>) -> Flow {
        let Some(u) = j.up.as_ref() else { return fail(FileError::Internal.text()) };
        if offset >= u.data.len() {
            return self.probe(id, j, 0, out);
        }
        let end = (offset + UPLOAD_CHUNK).min(u.data.len());
        let sub = FileSub::Append { path: u.temp.clone(), content: u.data[offset..end].to_vec() };
        j.state = State::UpAppend { offset };
        self.send(id, j, sub, out);
        Flow::Keep
    }

    /// `dest`, then `dest (2)` ... (`FT:348-418`).
    fn upload_name(dest: &str, n: usize) -> String {
        if n == 0 { dest.to_string() } else { format!("{dest} ({})", n + 1) }
    }

    fn probe(&mut self, id: JobId, j: &mut Job, n: usize, out: &mut Vec<Action>) -> Flow {
        j.state = State::UpProbe { n };
        let path = Self::upload_name(&j.path, n);
        self.send(id, j, FileSub::Stat { path }, out);
        Flow::Keep
    }

    /// The first stat (`FT:11-60`, `FT:130-172`).
    fn on_stat(&mut self, id: JobId, j: &mut Job, content: &str, out: &mut Vec<Action>) -> Flow {
        let file = match RemoteFile::parse(content) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
        if !j.chunked {
            if file.is_folder() {
                j.state = State::Zip;
                let path = j.path.clone();
                self.send(id, j, FileSub::Zip { path }, out);
                return Flow::Keep;
            }
            return self.start_stream(id, j, file.size(), out);
        }
        let ready = if file.is_folder() {
            Ready::Zip
        } else if !file.is_regular() {
            return fail(not_regular(&j.path, &j.host));
        } else {
            match file.size() {
                Some(n) => Ready::File(n),
                None => return fail(FileError::Internal.text()),
            }
        };
        // The slot is taken after the first stat succeeds, in arrival order (Decision).
        if self.running < MAX_DOWNLOADS && self.queue.is_empty() {
            self.running += 1;
            j.holds_slot = true;
            self.begin(id, j, ready, out)
        } else {
            j.state = State::Waiting(ready);
            self.queue.push_back(id);
            Flow::Keep
        }
    }

    /// The zip's stat: the zip is fetched like a file, and the job keeps its slot.
    fn on_zip_stat(&mut self, id: JobId, j: &mut Job, content: &str, out: &mut Vec<Action>) -> Flow {
        let file = match RemoteFile::parse(content) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
        if !j.chunked {
            return self.start_stream(id, j, file.size(), out);
        }
        if !file.is_regular() {
            let zip = j.zip.clone().unwrap_or_default();
            return fail(not_regular(&zip, &j.host));
        }
        match file.size() {
            Some(n) => self.start_chunks(id, j, n, out),
            None => fail(FileError::Internal.text()),
        }
    }

    fn start_stream(&mut self, id: JobId, j: &mut Job, size: Option<u64>, out: &mut Vec<Action>) -> Flow {
        out.push(Action::Size { job: id, size });
        j.state = State::Stream { offset: 0, size };
        self.stream_fetch(id, j, 0, out);
        Flow::Keep
    }

    fn stream_fetch(&mut self, id: JobId, j: &Job, offset: u64, out: &mut Vec<Action>) {
        let sub = FileSub::Fetch { path: j.fetch_path.clone(), chunk: Some(Chunk { offset, size: STREAM_CHUNK }), unique_id: None };
        self.send(id, j, sub, out);
    }

    /// Sequential 1024-byte fetches until an empty reply (`CFT:81-99`). A reply longer than
    /// asked, or data past the size the remote claimed, is garbage (Decision: iTerm2 takes both).
    fn on_stream(&mut self, id: JobId, j: &mut Job, offset: u64, size: Option<u64>, content: &str, out: &mut Vec<Action>) -> Flow {
        let bytes = match decode_fetch(content) {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        if bytes.len() as u64 > STREAM_CHUNK {
            return fail(GARBAGE);
        }
        if bytes.is_empty() {
            return Flow::End(Ok(if j.zip.is_some() { Outcome::ZipOfFolder } else { Outcome::File }));
        }
        let next = offset + bytes.len() as u64;
        if size.is_some_and(|s| next > s) {
            return fail(GARBAGE);
        }
        out.push(Action::Write { job: id, bytes });
        j.state = State::Stream { offset: next, size };
        self.stream_fetch(id, j, next, out);
        Flow::Keep
    }

    /// A chunk arrived: replies are taken strictly in order (`FT:180-206`).
    fn on_chunk(&mut self, id: JobId, j: &mut Job, mut c: Chunks, tag: u64, content: &str, out: &mut Vec<Action>) -> Flow {
        let bytes = match decode_fetch(content) {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        if bytes.len() as u64 > CHUNK {
            return fail(GARBAGE);
        }
        c.arrived.insert(tag, bytes);
        while let Some(&front) = c.pending.front() {
            let Some(bytes) = c.arrived.remove(&front) else { break };
            c.pending.pop_front();
            if bytes.is_empty() {
                // Empty with others pending: the file ended early. Empty for the last: it shrank.
                if !c.pending.is_empty() {
                    return fail(premature(c.received, c.size));
                }
                break;
            }
            c.received += bytes.len() as u64;
            out.push(Action::Write { job: id, bytes });
            self.issue(id, j, &mut c, out);
        }
        self.after_chunks(id, j, c, out)
    }
}

/// A folder unzip running on a worker thread: its job, the folder it lands in, and the channel
/// it answers on once.
pub struct UnzipWait {
    pub job: JobId,
    pub dest: std::path::PathBuf,
    pub rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// What a crashed worker's job is told.
pub const UNZIP_CRASHED: &str = "Internal error";

/// Takes the answers of the finished unzips out of `waits`. A worker that ended without
/// answering (it crashed) answers `UNZIP_CRASHED` for its job, so the download slot and the
/// folder it holds are released.
pub fn drain_unzips(waits: &mut Vec<UnzipWait>) -> Vec<(JobId, std::path::PathBuf, Result<(), String>)> {
    let mut out = Vec::new();
    waits.retain(|w| match w.rx.try_recv() {
        Ok(r) => {
            out.push((w.job, w.dest.clone(), r));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            out.push((w.job, w.dest.clone(), Err(UNZIP_CRASHED.to_string())));
            false
        }
    });
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_crashed_unzip_worker_fails_its_job() {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (tx2, rx2) = std::sync::mpsc::channel::<Result<(), String>>();
        let (_keep, rx3) = std::sync::mpsc::channel::<Result<(), String>>();
        let mut waits = vec![
            super::UnzipWait { job: 1, dest: "a".into(), rx },
            super::UnzipWait { job: 2, dest: "b".into(), rx: rx2 },
            super::UnzipWait { job: 3, dest: "c".into(), rx: rx3 },
        ];
        drop(tx); // the worker panicked: its sender dropped without an answer
        tx2.send(Ok(())).unwrap();
        let done = super::drain_unzips(&mut waits);
        assert_eq!(done, vec![(1, "a".into(), Err(super::UNZIP_CRASHED.to_string())), (2, "b".into(), Ok(()))]);
        assert_eq!(waits.len(), 1, "a worker still running keeps waiting");
    }

    use super::*;
    use crate::ssh_files::{Chunk, GARBAGE};
    use base64::Engine;

    const P: PaneId = 7;

    fn json_file(path: &str, size: u64) -> String {
        format!(r#"{{"absolutePath":"{path}","kind":{{"file":{{"_0":{{"size":{size}}}}}}}}}"#)
    }
    fn json_folder(path: &str) -> String {
        format!(r#"{{"absolutePath":"{path}","kind":{{"folder":{{}}}}}}"#)
    }
    fn b64n(n: usize, fill: u8) -> String {
        base64::engine::general_purpose::STANDARD.encode(vec![fill; n])
    }
    fn stat(path: &str) -> FileSub {
        FileSub::Stat { path: path.into() }
    }
    fn rm(path: &str) -> FileSub {
        FileSub::Rm { path: path.into(), recursive: false }
    }
    fn stream_fetch(path: &str, offset: u64) -> FileSub {
        FileSub::Fetch { path: path.into(), chunk: Some(Chunk { offset, size: 1024 }), unique_id: None }
    }
    /// The sends in `acts` as (tag, sub); a chunked fetch's unique id is blanked.
    fn sends(acts: &[Action]) -> Vec<(u64, FileSub)> {
        acts.iter()
            .filter_map(|a| match a {
                Action::Send { tag, sub: FileSub::Fetch { path, chunk, .. }, .. } => Some((*tag, FileSub::Fetch { path: path.clone(), chunk: *chunk, unique_id: None })),
                Action::Send { tag, sub, .. } => Some((*tag, sub.clone())),
                _ => None,
            })
            .collect()
    }
    fn only_send(acts: &[Action]) -> (u64, FileSub) {
        let s = sends(acts);
        assert_eq!(s.len(), 1, "{acts:?}");
        s.into_iter().next().unwrap()
    }
    fn tag_of(acts: &[Action]) -> u64 {
        only_send(acts).0
    }
    fn done(job: JobId, result: Result<Outcome, &str>) -> Action {
        Action::Done { job, result: result.map_err(|s| s.to_string()) }
    }
    fn write(job: JobId, bytes: &[u8]) -> Action {
        Action::Write { job, bytes: bytes.to_vec() }
    }

    #[test]
    fn stream_download_follows_iterm2() {
        let mut t = Transfers::new();
        let (job, acts) = t.download(P, "/h/a.txt");
        assert_eq!(acts, [Action::Send { job, tag: 1, pane: P, sub: stat("/h/a.txt"), high: false }]);
        assert!(t.is_live(job));
        let acts = t.reply(job, 1, &json_file("/h/a.txt", 2500), 0);
        assert_eq!(acts.len(), 2, "{acts:?}");
        assert_eq!(acts[0], Action::Size { job, size: Some(2500) });
        assert_eq!(acts[1], Action::Send { job, tag: 2, pane: P, sub: stream_fetch("/h/a.txt", 0), high: false });
        let acts = t.reply(job, 2, &b64n(1024, 1), 0);
        assert_eq!(acts[0], write(job, &[1; 1024]));
        assert_eq!(only_send(&acts[1..]), (3, stream_fetch("/h/a.txt", 1024)));
        let acts = t.reply(job, 3, &b64n(1024, 2), 0);
        assert_eq!(only_send(&acts[1..]), (4, stream_fetch("/h/a.txt", 2048)));
        let acts = t.reply(job, 4, &b64n(452, 3), 0);
        assert_eq!(acts[0], write(job, &[3; 452]));
        assert_eq!(only_send(&acts[1..]), (5, stream_fetch("/h/a.txt", 2500)));
        assert_eq!(t.reply(job, 5, "", 0), [done(job, Ok(Outcome::File))]);
        assert!(!t.is_live(job));
        assert_eq!(t.reply(job, 5, "", 0), [], "a reply for a finished job");
        assert_eq!(t.running(), 0, "a stream never holds a slot");

        // A folder: zip, stat the zip, stream it, save as a zip, remove the remote zip.
        let (job, acts) = t.download(P, "/h/d");
        let tag = tag_of(&acts);
        let acts = t.reply(job, tag, &json_folder("/h/d"), 0);
        assert_eq!(only_send(&acts), (tag + 1, FileSub::Zip { path: "/h/d".into() }));
        let acts = t.reply(job, tag + 1, "/h/.tmpab", 0);
        assert_eq!(only_send(&acts), (tag + 2, stat("/h/.tmpab")));
        let acts = t.reply(job, tag + 2, &json_file("/h/.tmpab", 3), 0);
        assert_eq!(acts[0], Action::Size { job, size: Some(3) });
        assert_eq!(only_send(&acts), (tag + 3, stream_fetch("/h/.tmpab", 0)));
        let acts = t.reply(job, tag + 3, &b64n(3, 9), 0);
        assert_eq!(acts[0], write(job, &[9; 3]));
        assert_eq!(only_send(&acts), (tag + 4, stream_fetch("/h/.tmpab", 3)));
        let acts = t.reply(job, tag + 4, "", 0);
        assert_eq!(acts, [done(job, Ok(Outcome::ZipOfFolder)), Action::Fire { pane: P, sub: rm("/h/.tmpab") }]);

        // The zip's fetch fails: the remote zip is removed on failure too.
        let (job, acts) = t.download(P, "/h/d");
        let tag = tag_of(&acts);
        let tag = tag_of(&t.reply(job, tag, &json_folder("/h/d"), 0));
        let tag = tag_of(&t.reply(job, tag, "/h/.tmpab", 0));
        let acts = t.reply(job, tag, &json_file("/h/.tmpab", 3), 0);
        let tag = tag_of(&acts);
        assert_eq!(t.reply(job, tag, "", 2), [done(job, Err("File not found")), Action::Fire { pane: P, sub: rm("/h/.tmpab") }]);

        // A reply longer than the fetch asked: garbage (Decision).
        let (job, acts) = t.download(P, "/h/a");
        let tag = tag_of(&acts);
        let tag = tag_of(&t.reply(job, tag, &json_file("/h/a", 5000), 0)[1..]);
        assert_eq!(t.reply(job, tag, &b64n(1025, 1), 0), [done(job, Err(GARBAGE))]);

        // Errors at every step.
        let (job, acts) = t.download(P, "/h/a");
        assert_eq!(t.reply(job, tag_of(&acts), "", 2), [done(job, Err("File not found"))]);
        let (job, acts) = t.download(P, "/h/a");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_file("/h/a", 5), 0)[1..]);
        assert_eq!(t.reply(job, tag, "", -1), [done(job, Err("Connection closed"))]);
        let (job, acts) = t.download(P, "/h/a");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_file("/h/a", 5), 0)[1..]);
        assert_eq!(t.reply(job, tag, "@@", 0), [done(job, Err(GARBAGE))]);
        let (job, acts) = t.download(P, "/h/a");
        assert_eq!(t.reply(job, tag_of(&acts), "not json", 0), [done(job, Err(GARBAGE))]);
        // A zip answer nobody can use.
        let (job, acts) = t.download(P, "/h/d");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0));
        assert_eq!(t.reply(job, tag, "", 0), [done(job, Err(GARBAGE))]);

        // Unknown tags and jobs.
        let (job, acts) = t.download(P, "/h/a");
        let tag = tag_of(&acts);
        assert_eq!(t.reply(job, tag + 100, "", 0), []);
        assert_eq!(t.reply(job + 100, tag, "", 0), []);
        assert!(t.is_live(job));
    }

    /// Starts a chunked download of `size` bytes at `path`; its actions after the stat reply.
    fn start_chunked(t: &mut Transfers, path: &str, size: u64) -> (JobId, Vec<Action>) {
        let (job, acts) = t.download_chunked(P, path, "host");
        let tag = tag_of(&acts);
        let acts = t.reply(job, tag, &json_file(path, size), 0);
        (job, acts)
    }
    fn fetch_at(path: &str, offset: u64) -> FileSub {
        FileSub::Fetch { path: path.into(), chunk: Some(Chunk { offset, size: 4096 }), unique_id: None }
    }

    #[test]
    fn chunked_download_follows_iterm2() {
        let mut t = Transfers::new();
        let (job, acts) = t.download_chunked(P, "/h/big", "host");
        assert_eq!(acts, [Action::Send { job, tag: 1, pane: P, sub: stat("/h/big"), high: false }]);
        let acts = t.reply(job, 1, &json_file("/h/big", 40960), 0);
        assert_eq!(acts[0], Action::Size { job, size: Some(40960) });
        let s = sends(&acts);
        assert_eq!(s.len(), 8);
        for (i, (tag, sub)) in s.iter().enumerate() {
            assert_eq!((*tag, sub), (i as u64 + 2, &fetch_at("/h/big", i as u64 * 4096)));
        }
        let mut ids: Vec<String> = acts.iter().filter_map(|a| match a {
            Action::Send { sub: FileSub::Fetch { unique_id: Some(id), .. }, .. } => Some(id.clone()),
            _ => None,
        }).collect();
        assert_eq!(ids.len(), 8);
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8, "distinct unique ids");
        assert_eq!(t.running(), 1);

        // Replies come out of order; the writes stay in order.
        assert_eq!(t.reply(job, 3, &b64n(4096, 2), 0), []);
        let acts = t.reply(job, 2, &b64n(4096, 1), 0);
        assert_eq!(acts.len(), 4, "{acts:?}");
        assert_eq!(acts[0], write(job, &[1; 4096]));
        assert_eq!(sends(&acts[1..2]), [(10, fetch_at("/h/big", 32768))]);
        assert_eq!(acts[2], write(job, &[2; 4096]));
        assert_eq!(sends(&acts[3..4]), [(11, fetch_at("/h/big", 36864))]);
        // Tags 4..=11 remain; answer them in reverse.
        let mut written = vec![];
        let mut all: Vec<Action> = vec![];
        for tag in (4..=11u64).rev() {
            all.extend(t.reply(job, tag, &b64n(4096, tag as u8), 0));
        }
        for a in &all {
            if let Action::Write { bytes, .. } = a {
                written.push(bytes[0]);
            }
        }
        assert_eq!(written, (4..=11u8).collect::<Vec<_>>(), "offset order");
        assert_eq!(all.last(), Some(&done(job, Ok(Outcome::File))));
        assert_eq!(all.iter().filter(|a| matches!(a, Action::Done { .. })).count(), 1);
        assert_eq!(t.running(), 0);
        assert!(!t.is_live(job));

        // Sizes that are not a whole window.
        let (_, acts) = start_chunked(&mut t, "/h/b", 8192);
        assert_eq!(sends(&acts).len(), 2);
        let (job, acts) = start_chunked(&mut t, "/h/c", 5000);
        assert_eq!(sends(&acts).iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(), [fetch_at("/h/c", 0), fetch_at("/h/c", 4096)]);
        let second = sends(&acts)[1].0;
        let acts = t.reply(job, sends(&acts)[0].0, &b64n(4096, 1), 0);
        assert_eq!(acts, [write(job, &[1; 4096])]);
        let acts = t.reply(job, second, &b64n(904, 2), 0);
        assert_eq!(acts, [write(job, &[2; 904]), done(job, Ok(Outcome::File))]);

        // Size 0: no fetch.
        let (job, acts) = start_chunked(&mut t, "/h/z", 0);
        assert_eq!(acts, [Action::Size { job, size: Some(0) }, done(job, Ok(Outcome::File))]);

        // An empty chunk before the end.
        let (job, acts) = start_chunked(&mut t, "/h/p", 40960);
        let first = sends(&acts)[0].0;
        assert_eq!(t.reply(job, first, &b64n(4096, 1), 0)[0], write(job, &[1; 4096]));
        let acts = t.reply(job, first + 1, "", 0);
        assert_eq!(acts, [done(job, Err("Download ended prematurely (received 4096 of 40960 bytes)"))]);
        assert_eq!(t.reply(job, first + 2, &b64n(4096, 1), 0), []);

        // An empty reply for the last pending fetch is accepted.
        let (job, acts) = start_chunked(&mut t, "/h/s", 5000);
        let first = sends(&acts)[0].0;
        assert_eq!(t.reply(job, first, &b64n(4096, 1), 0), [write(job, &[1; 4096])]);
        assert_eq!(t.reply(job, first + 1, "", 0), [done(job, Ok(Outcome::File))]);

        // A chunk longer than asked: garbage.
        let (job, acts) = start_chunked(&mut t, "/h/g", 40960);
        let first = sends(&acts)[0].0;
        assert_eq!(t.reply(job, first, &b64n(4097, 1), 0), [done(job, Err(GARBAGE))]);
        assert_eq!(t.reply(job, first + 1, &b64n(4096, 1), 0), []);

        // Not a regular file; no size.
        let (job, acts) = t.download_chunked(P, "/h/l", "host");
        let acts = t.reply(job, tag_of(&acts), r#"{"absolutePath":"/h/l","kind":{"symlink":{"_0":"/etc"}}}"#, 0);
        assert_eq!(acts, [done(job, Err("/h/l on host is not a regular file"))]);
        let (job, acts) = t.download_chunked(P, "/h/n", "host");
        let acts = t.reply(job, tag_of(&acts), r#"{"absolutePath":"/h/n","kind":{"file":{"_0":{}}}}"#, 0);
        assert_eq!(acts, [done(job, Err("Internal error"))]);
        assert_eq!(t.running(), 1, "only the 8192 byte download of /h/b still runs");
    }

    #[test]
    fn chunked_folder_follows_iterm2() {
        let mut t = Transfers::new();
        let (job, acts) = t.download_chunked(P, "/h/d", "host");
        let acts = t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0);
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, FileSub::Zip { path: "/h/d".into() });
        let (tag, sub) = only_send(&t.reply(job, tag, "/h/.z", 0));
        assert_eq!(sub, stat("/h/.z"));
        let acts = t.reply(job, tag, &json_file("/h/.z", 5), 0);
        assert_eq!(acts[0], Action::Size { job, size: Some(5) });
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, fetch_at("/h/.z", 0));
        assert_eq!(t.reply(job, tag, &b64n(5, 4), 0), [write(job, &[4; 5]), Action::Unzip { job }]);
        assert!(t.is_live(job));
        assert_eq!(t.running(), 1, "unzipping holds the slot");
        assert_eq!(t.unzipped(job, Ok(())), [Action::Fire { pane: P, sub: rm("/h/.z") }, done(job, Ok(Outcome::Directory))]);
        assert_eq!(t.running(), 0);
        assert_eq!(t.unzipped(job, Ok(())), [], "a second answer");

        // A bad zip.
        let (job, acts) = t.download_chunked(P, "/h/d", "host");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0));
        let tag = tag_of(&t.reply(job, tag, "/h/.z", 0));
        let tag = tag_of(&t.reply(job, tag, &json_file("/h/.z", 5), 0));
        t.reply(job, tag, &b64n(5, 4), 0);
        assert_eq!(t.unzipped(job, Err("bad zip".into())), [Action::Fire { pane: P, sub: rm("/h/.z") }, done(job, Err("bad zip"))]);

        // The zip's fetch fails.
        let (job, acts) = t.download_chunked(P, "/h/d", "host");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0));
        let tag = tag_of(&t.reply(job, tag, "/h/.z", 0));
        let tag = tag_of(&t.reply(job, tag, &json_file("/h/.z", 5), 0));
        assert_eq!(t.reply(job, tag, "", 2), [Action::Fire { pane: P, sub: rm("/h/.z") }, done(job, Err("File not found"))]);
        assert_eq!(t.running(), 0);

        // The zip itself fails: nothing to remove.
        let (job, acts) = t.download_chunked(P, "/h/d", "host");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0));
        assert_eq!(t.reply(job, tag, "", 1), [done(job, Err("File not found"))]);
        assert_eq!(t.running(), 0);
    }

    #[test]
    fn downloads_queue_follows_iterm2() {
        let mut t = Transfers::new();
        let mut jobs = vec![];
        let mut fetches = vec![];
        for i in 0..3 {
            let (job, acts) = t.download_chunked(P, &format!("/h/f{i}"), "host");
            let acts = t.reply(job, tag_of(&acts), &json_file(&format!("/h/f{i}"), 4096), 0);
            jobs.push(job);
            fetches.push(sends(&acts));
        }
        assert_eq!(fetches[0].len(), 1);
        assert_eq!(fetches[1].len(), 1);
        assert_eq!(fetches[2].len(), 0, "no slot");
        assert_eq!(t.running(), 2);
        let acts = t.reply(jobs[0], fetches[0][0].0, &b64n(4096, 1), 0);
        assert_eq!(acts[0], write(jobs[0], &[1; 4096]));
        assert_eq!(acts[1], done(jobs[0], Ok(Outcome::File)));
        assert!(acts.contains(&Action::Size { job: jobs[2], size: Some(4096) }));
        assert_eq!(sends(&acts).len(), 1);
        assert_eq!(sends(&acts)[0].1, fetch_at("/h/f2", 0));
        assert_eq!(t.running(), 2);

        // Two folders at once both reach their zip fetches.
        let mut t = Transfers::new();
        let mut zips = vec![];
        for i in 0..2 {
            let (job, acts) = t.download_chunked(P, &format!("/h/d{i}"), "host");
            let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder(&format!("/h/d{i}")), 0));
            let tag = tag_of(&t.reply(job, tag, &format!("/h/.z{i}"), 0));
            let acts = t.reply(job, tag, &json_file(&format!("/h/.z{i}"), 4096), 0);
            zips.push(sends(&acts));
        }
        assert_eq!(zips[0][0].1, fetch_at("/h/.z0", 0));
        assert_eq!(zips[1][0].1, fetch_at("/h/.z1", 0));
        assert_eq!(t.running(), 2);

        // A third folder waits for a slot before its zip.
        let (job3, acts) = t.download_chunked(P, "/h/d2", "host");
        assert_eq!(t.reply(job3, tag_of(&acts), &json_folder("/h/d2"), 0), []);
        assert_eq!(t.running(), 2);

        // Single streams never wait.
        let (_, acts) = t.download(P, "/h/s");
        assert_eq!(acts.len(), 1);
        assert_eq!(t.running(), 2);
    }

    #[test]
    fn hostile_sizes_never_allocate() {
        // A claimed size of u64::MAX only bounds the window: eight fetches, no buffer sized by it.
        let mut t = Transfers::new();
        let (job, acts) = start_chunked(&mut t, "/h/huge", u64::MAX);
        assert_eq!(sends(&acts).len(), 8);
        assert_eq!(acts[0], Action::Size { job, size: Some(u64::MAX) });
        // Offsets near the end of u64 do not overflow.
        let (job, acts) = start_chunked(&mut t, "/h/edge", u64::MAX - 1);
        assert_eq!(sends(&acts).len(), 8);
        assert!(t.is_live(job));
        // Single stream: only the claimed size is reported.
        let (job, acts) = t.download(P, "/h/a");
        let acts = t.reply(job, tag_of(&acts), &json_file("/h/a", u64::MAX), 0);
        assert_eq!(acts[0], Action::Size { job, size: Some(u64::MAX) });
    }

    #[test]
    fn a_local_failure_ends_the_job() {
        let mut t = Transfers::new();
        // A remote claiming u64::MAX: eight fetches out, then the local side fails the job.
        let (a, acts) = start_chunked(&mut t, "/h/a", u64::MAX);
        let a_tags: Vec<u64> = sends(&acts).iter().map(|(tag, _)| *tag).collect();
        let (b, _) = start_chunked(&mut t, "/h/b", u64::MAX);
        // A third waits for a slot.
        let (c, acts) = t.download_chunked(P, "/h/c", "host");
        assert_eq!(t.reply(c, tag_of(&acts), &json_file("/h/c", 10), 0), []);
        assert_eq!(t.running(), 2);
        let acts = t.fail(a, "Not enough space on the drive");
        assert_eq!(acts[0], done(a, Err("Not enough space on the drive")));
        assert_eq!(acts.len(), 3, "{acts:?}");
        assert_eq!(acts[1], Action::Size { job: c, size: Some(10) });
        assert_eq!(only_send(&acts).1, fetch_at("/h/c", 0), "the queued job takes the slot");
        assert!(!t.is_live(a) && t.is_live(b) && t.is_live(c));
        assert_eq!(t.running(), 2);
        // Replies still on the way are ignored; a second fail is nothing.
        for tag in a_tags {
            assert_eq!(t.reply(a, tag, &b64n(4096, 1), 0), []);
        }
        assert_eq!(t.fail(a, "again"), []);
        assert_eq!(t.fail(b, "x"), [done(b, Err("x"))]);
        // A folder job fails with its remote zip removed first.
        let (job, acts) = t.download_chunked(P, "/h/d", "host");
        let tag = tag_of(&t.reply(job, tag_of(&acts), &json_folder("/h/d"), 0));
        t.reply(job, tag, "/h/.tmpzz", 0);
        assert_eq!(t.fail(job, "y"), [Action::Fire { pane: P, sub: rm("/h/.tmpzz") }, done(job, Err("y"))]);
    }

    #[test]
    fn unsized_streams_are_bounded_by_free_space() {
        assert!(has_room(RESERVE + 1024, 1024));
        assert!(!has_room(RESERVE + 1023, 1024));
        assert!(!has_room(RESERVE, 1));
        assert!(!has_room(u64::MAX, u64::MAX), "no overflow");
    }

    fn append(temp: &str, n: usize) -> FileSub {
        FileSub::Append { path: temp.into(), content: vec![b'x'; n] }
    }
    fn mv(source: &str, dest: &str) -> FileSub {
        FileSub::Mv { source: source.into(), dest: dest.into() }
    }

    #[test]
    fn upload_follows_iterm2() {
        let mut t = Transfers::new();
        let temp = "/tmp/f.uploading-U1";
        let (job, acts) = t.upload(P, "/tmp/f", vec![b'x'; 2500], "U1");
        assert_eq!(upload_temp("/tmp/f", "U1"), temp);
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, FileSub::Create { path: temp.into(), content: vec![] });
        let (tag, sub) = only_send(&t.reply(job, tag, "", 0));
        assert_eq!(sub, append(temp, 1024));
        let acts = t.reply(job, tag, "", 0);
        assert_eq!(acts[0], Action::Sent { job, bytes: 1024 });
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, append(temp, 1024));
        let acts = t.reply(job, tag, "", 0);
        assert_eq!(acts[0], Action::Sent { job, bytes: 1024 });
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, append(temp, 452));
        let acts = t.reply(job, tag, "", 0);
        assert_eq!(acts[0], Action::Sent { job, bytes: 452 });
        let (tag, sub) = only_send(&acts);
        assert_eq!(sub, stat("/tmp/f"));
        // Taken: the next name.
        let (tag, sub) = only_send(&t.reply(job, tag, &json_file("/tmp/f", 1), 0));
        assert_eq!(sub, stat("/tmp/f (2)"));
        // Free (status 2): the move.
        let (tag, sub) = only_send(&t.reply(job, tag, "", 2));
        assert_eq!(sub, mv(temp, "/tmp/f (2)"));
        // The mv reply's content is ignored, junk or the framer's stat JSON.
        let mut again = Transfers::new();
        let (j2, a2) = again.upload(P, "/tmp/f", vec![], "U1");
        let (tg, _) = only_send(&a2);
        let (tg, sub) = only_send(&again.reply(j2, tg, "", 0));
        assert_eq!(sub, stat("/tmp/f"), "an empty upload goes from create to stat");
        let (tg, sub) = only_send(&again.reply(j2, tg, "", -1));
        assert_eq!(sub, mv(temp, "/tmp/f"), "a stat answered -1 counts as free");
        assert_eq!(again.reply(j2, tg, "junk", 0), [done(j2, Ok(Outcome::Uploaded { remote_name: "/tmp/f".into() }))]);
        let json = r#"{"absolutePath":"/tmp/f (2)","kind":{"file":{"_0":{"size":2500}}}}"#;
        assert_eq!(t.reply(job, tag, json, 0), [done(job, Ok(Outcome::Uploaded { remote_name: "/tmp/f (2)".into() }))]);
        assert!(!t.is_live(job));
    }

    #[test]
    fn upload_failures_follow_iterm2() {
        let temp = "/tmp/f.uploading-U1";
        let failed = |job, why: &str| [Action::Fire { pane: P, sub: rm(temp) }, done(job, Err(why))];
        // Create status 1, an append -1, a mv status 1.
        let mut t = Transfers::new();
        let (job, acts) = t.upload(P, "/tmp/f", vec![b'x'; 10], "U1");
        let (tag, _) = only_send(&acts);
        assert_eq!(t.reply(job, tag, "", 1), failed(job, "File not found"));
        let (job, acts) = t.upload(P, "/tmp/f", vec![b'x'; 10], "U1");
        let (tag, _) = only_send(&acts);
        let (tag, _) = only_send(&t.reply(job, tag, "", 0));
        assert_eq!(t.reply(job, tag, "", -1), failed(job, "Connection closed"));
        let (job, acts) = t.upload(P, "/tmp/f", vec![], "U1");
        let (tag, _) = only_send(&acts);
        let (tag, _) = only_send(&t.reply(job, tag, "", 0));
        let (tag, _) = only_send(&t.reply(job, tag, "", 2));
        assert_eq!(t.reply(job, tag, "", 1), failed(job, "File not found"));
        // Every name taken: exactly 100 stats.
        let (job, acts) = t.upload(P, "/tmp/f", vec![], "U1");
        let (tag, _) = only_send(&acts);
        let (mut tag, mut sub) = only_send(&t.reply(job, tag, "", 0));
        let mut asked = vec![];
        let end = loop {
            let FileSub::Stat { path } = &sub else { panic!("{sub:?}") };
            asked.push(path.clone());
            let acts = t.reply(job, tag, &json_file(path, 1), 0);
            if sends(&acts).is_empty() {
                break acts;
            }
            (tag, sub) = only_send(&acts);
        };
        assert_eq!(asked.len(), 100);
        assert_eq!((asked[0].as_str(), asked[1].as_str(), asked[99].as_str()), ("/tmp/f", "/tmp/f (2)", "/tmp/f (100)"));
        assert_eq!(end, failed(job, TOO_MANY));
        assert_eq!(TOO_MANY, "Too many iterations to find a valid file name on remote host for upload");
    }

    #[test]
    fn a_local_failure_removes_the_upload_temp() {
        let mut t = Transfers::new();
        let (job, _) = t.upload(P, "/tmp/f", vec![b'x'; 10], "U1");
        assert_eq!(t.fail(job, "x"), [Action::Fire { pane: P, sub: rm("/tmp/f.uploading-U1") }, done(job, Err("x"))]);
    }
}
