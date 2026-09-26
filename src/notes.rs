//! The Notes tool's store, pure rules: its two modes and when a change to the global note is
//! written (iTerm2's `Toolbelt/ToolNotes.m`, `TN`), and the global note's file with iTerm2's
//! rules for never losing it (`GlobalNote`, plain `std::fs`). The editor is `text_edit`; the app
//! owns the path under `%APPDATA%\Tandem`, the session note on the Pane and the drawing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Which note the tool edits (`TN:91-99`): the one global note, or the current session's note
/// (the one Edit Session Note shows). Global first; the choice is not saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Global,
    Session,
}

/// The mode control's segments, in order (`TN:94`).
pub const MODES: [(Mode, &str); 2] = [(Mode::Global, "Global"), (Mode::Session, "Session")];

/// The global note's file, plain UTF-8 text beside Tandem's other files (iTerm2 keeps
/// `notes.rtfd`; ruling: Tandem's editor is plain text only).
pub const FILE_NAME: &str = "notes.txt";

/// A global note this long or longer is written on a trailing debounce instead of on every
/// change (`TN:554`, `100 * 1024` characters).
pub const LARGE: usize = 100 * 1024;

/// The trailing debounce for a large global note (`TN:598`, 2.0 s after the last change).
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// When the global note is written: at once for a note under `LARGE`, else `DEBOUNCE` after
/// the last change (`TN:554-598`). `dirty` marks an armed write not yet made, which closing the
/// tool or switching its mode flushes.
#[derive(Clone, Debug, Default)]
pub struct Saver {
    due: Option<Instant>,
    dirty: bool,
}

impl Saver {
    /// The global note changed and is now `chars` long (`length`). True: write it now (this
    /// also disarms a pending write, the note shrank). False: a write is armed for
    /// `DEBOUNCE` from `now`, replacing any earlier one.
    pub fn changed(&mut self, chars: usize, now: Instant) -> bool {
        if chars < LARGE {
            self.due = None;
            self.dirty = false;
            true
        } else {
            self.due = Some(now + DEBOUNCE);
            self.dirty = true;
            false
        }
    }

    /// When the armed write is due, for the app's wake-up timer.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// True once, when the armed write is due at `now`: write the note.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(d) if now >= d => {
                self.due = None;
                self.dirty = false;
                true
            }
            _ => false,
        }
    }

    /// True when an armed write was still pending (the tool is closing or leaving Global
    /// mode): write the note now. Disarms it.
    pub fn flush(&mut self) -> bool {
        let dirty = self.dirty;
        self.due = None;
        self.dirty = false;
        dirty
    }
}

/// A note's length as `LARGE` counts it: `NSString` length, UTF-16 units (`TN:554` reads
/// `textStorage.length`).
pub fn length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The global note's text from its file's bytes: UTF-8, invalid bytes replaced, a leading
/// byte order mark dropped.
pub fn read_note(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// The debounced large-note write retries while the file stays unreadable, at most this many
/// times (`TN` `kMaxLargeNoteWriteRetries`).
pub const NOTE_RETRIES: u32 = 60;

/// What a write of the global note did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrote {
    Written,
    /// Nothing new to write, or an empty note with no file (a deleted note stays deleted).
    Nothing,
    /// The file is there but could not be read: its content is unknown, so it is left alone
    /// and the write retried later.
    Deferred,
    /// The file could be read again and holds the note now (the typing over the unread,
    /// emptied view was backed up and dropped).
    Adopted,
}

/// The file's size and modification time: a cheap fingerprint of the notes file, to tell
/// whether someone else wrote it since this note read or wrote it (`notesFileStatToken`).
fn stat_token(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The global note (one for every window) and its file, kept with iTerm2's rules for never
/// losing a note (`TN:161-280,500-548`): a file that exists but cannot be read is never
/// overwritten by the empty view that failed read left, unless the file is gone by the time
/// the user typed; typing over such a view is dropped (backed up) if the file becomes readable;
/// a file changed by someone else is backed up before it is overwritten; an empty note never
/// recreates a missing file; every write goes through a temporary file and a rename.
#[derive(Default)]
pub struct GlobalNote {
    path: PathBuf,
    loaded: bool,
    pub text: String,
    /// A read failed: the file may hold a note the view does not (`needsReloadBeforeSave_`).
    needs_reload: bool,
    /// The view is empty because that read failed (`viewEmptiedByFailedLoad_`).
    emptied: bool,
    /// The user typed since the failed read (`editedSinceFailedReload_`).
    edited_since_failed: bool,
    /// The note holds a change not yet on disk (`globalNoteDirty_`).
    dirty: bool,
    token: Option<(u64, Option<SystemTime>)>,
    saver: Saver,
    retries: u32,
    retry_at: Option<Instant>,
}

impl GlobalNote {
    pub fn new(path: PathBuf) -> GlobalNote {
        GlobalNote { path, ..GlobalNote::default() }
    }

    /// Reads the file the first time the note is wanted. Some error when it exists but
    /// could not be read (the note then shows empty and the file is protected).
    pub fn ensure_loaded(&mut self) -> Option<String> {
        if self.loaded {
            return None;
        }
        self.loaded = true;
        self.edited_since_failed = false;
        self.dirty = false;
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                self.text = read_note(&bytes);
                self.needs_reload = false;
                self.emptied = false;
                self.token = stat_token(&self.path);
                None
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.text.clear();
                self.needs_reload = false;
                self.emptied = false;
                self.token = None;
                None
            }
            Err(e) => {
                self.text.clear();
                self.needs_reload = true;
                self.emptied = true;
                self.token = None;
                Some(format!("Could not read the notes in {}: {e}", self.path.display()))
            }
        }
    }

    /// The file exists (an error other than not found counts as there: never overwrite what
    /// cannot be looked at).
    fn file_exists(&self) -> bool {
        match std::fs::metadata(&self.path) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }

    fn clear_stale(&mut self) {
        self.needs_reload = false;
        self.emptied = false;
        self.edited_since_failed = false;
    }

    /// The user changed the note to `text` at `now`: written at once while short, else on the
    /// 2 s trailing debounce (`Saver`).
    pub fn edited(&mut self, text: String, now: Instant) -> Result<Wrote, String> {
        self.text = text;
        self.dirty = true;
        if self.needs_reload {
            self.edited_since_failed = true;
        }
        self.retries = 0;
        self.retry_at = None;
        if self.saver.changed(length(&self.text), now) { self.write_retrying(now) } else { Ok(Wrote::Nothing) }
    }

    /// When the debounced write or a retry is due.
    pub fn due(&self) -> Option<Instant> {
        [self.saver.due(), self.retry_at].into_iter().flatten().min()
    }

    /// Makes a write that is due at `now`.
    pub fn tick(&mut self, now: Instant) -> Result<Wrote, String> {
        let retry = self.retry_at.is_some_and(|t| now >= t);
        if !self.saver.take_due(now) && !retry {
            return Ok(Wrote::Nothing);
        }
        self.retry_at = None;
        self.write_retrying(now)
    }

    /// Writes a pending change now (Tandem is quitting).
    pub fn flush(&mut self) -> Result<Wrote, String> {
        self.saver.flush();
        self.retry_at = None;
        if self.dirty { self.write() } else { Ok(Wrote::Nothing) }
    }

    /// `write`, retried after `DEBOUNCE` (at most `NOTE_RETRIES` times) while the file
    /// cannot be read.
    fn write_retrying(&mut self, now: Instant) -> Result<Wrote, String> {
        let r = self.write();
        if r == Ok(Wrote::Deferred) && self.retries < NOTE_RETRIES {
            self.retries += 1;
            self.retry_at = Some(now + DEBOUNCE);
        }
        r
    }

    /// A sibling of the notes file for a recovery copy.
    fn beside(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }

    fn write(&mut self) -> Result<Wrote, String> {
        let exists = self.file_exists();
        if self.needs_reload {
            if !exists {
                if self.edited_since_failed {
                    // The note went away and the user typed a new one: nothing to clobber.
                    self.clear_stale();
                } else {
                    // Unedited and gone: the stale view must not recreate it.
                    self.text.clear();
                    self.clear_stale();
                }
            } else if self.edited_since_failed && !self.emptied {
                self.clear_stale();
            } else {
                match std::fs::read(&self.path) {
                    Ok(bytes) => {
                        if self.edited_since_failed && !self.text.is_empty() {
                            let _ = std::fs::write(self.beside("notes-discarded.txt"), self.text.as_bytes());
                        }
                        self.text = read_note(&bytes);
                        self.clear_stale();
                        self.dirty = false;
                        self.token = stat_token(&self.path);
                        return Ok(Wrote::Adopted);
                    }
                    Err(_) => return Ok(Wrote::Deferred),
                }
            }
        }
        if self.text.is_empty() && !exists {
            return Ok(Wrote::Nothing);
        }
        if !self.dirty {
            return Ok(Wrote::Nothing);
        }
        if exists && (self.token.is_none() || self.token != stat_token(&self.path)) {
            // Someone else wrote it since this note read it: keep their copy.
            let _ = std::fs::copy(&self.path, self.beside("notes-backup.txt"));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not save the notes: {e}"))?;
        }
        let tmp = self.beside("notes.txt.tmp");
        if let Err(e) = std::fs::write(&tmp, self.text.as_bytes()).and_then(|()| std::fs::rename(&tmp, &self.path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Could not save the notes: {e}"));
        }
        self.token = stat_token(&self.path);
        self.dirty = false;
        Ok(Wrote::Written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_save_like_iterm2() {
        assert_eq!(MODES.map(|(_, name)| name), ["Global", "Session"]);
        assert_eq!(MODES[0].0, Mode::default());
        assert_eq!((FILE_NAME, LARGE, DEBOUNCE), ("notes.txt", 102_400, Duration::from_secs(2)));

        // Under LARGE characters every change writes at once and arms nothing.
        let t0 = Instant::now();
        let mut s = Saver::default();
        assert!(s.changed(LARGE - 1, t0));
        assert_eq!(s.due(), None);
        assert!(!s.take_due(t0 + Duration::from_secs(10)));
        assert!(!s.flush());

        // At LARGE or more: a trailing debounce, 2 s after the last change.
        assert!(!s.changed(LARGE, t0));
        assert_eq!(s.due(), Some(t0 + DEBOUNCE));
        let t1 = t0 + Duration::from_millis(1500);
        assert!(!s.changed(LARGE + 5, t1));
        assert_eq!(s.due(), Some(t1 + DEBOUNCE));
        assert!(!s.take_due(t0 + DEBOUNCE));
        assert!(s.take_due(t1 + DEBOUNCE));
        assert_eq!(s.due(), None);
        assert!(!s.take_due(t1 + DEBOUNCE * 2));

        // Shrinking under LARGE while a write is armed writes now and disarms it.
        assert!(!s.changed(LARGE, t0));
        assert!(s.changed(10, t1));
        assert_eq!(s.due(), None);

        // Closing or switching mode flushes an armed write once.
        assert!(!s.changed(LARGE, t0));
        assert!(s.flush());
        assert_eq!(s.due(), None);
        assert!(!s.flush());

        // Length counts like NSString: UTF-16 units, not bytes.
        assert_eq!(length("a\u{e9}"), 2);
        assert_eq!(length("\u{1F600}"), 2);
    }

    /// Losing a note is the worst failure: a file that could not be read is never written
    /// over by the empty view that read left, the typing over it is backed up when the file
    /// reads again, an empty note never creates a file, a file changed elsewhere is backed up,
    /// and writes leave no temporary file.
    #[test]
    fn global_note_never_loses_the_file() {
        let dir = std::env::temp_dir().join(format!("tandem-notes-{}-{}", std::process::id(), SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
        let path = dir.join("Tandem").join(FILE_NAME);
        let now = Instant::now();

        // No file: empty, and an empty note writes nothing.
        let mut g = GlobalNote::new(path.clone());
        assert_eq!(g.ensure_loaded(), None);
        assert_eq!(g.edited(String::new(), now), Ok(Wrote::Nothing));
        assert!(!path.exists());
        // A first note is written, through a temporary file that does not stay.
        assert_eq!(g.edited("hello".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert!(!path.with_file_name("notes.txt.tmp").exists());

        // Someone else rewrote it: their copy is kept aside before it is overwritten.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "theirs, longer").unwrap();
        assert_eq!(g.edited("mine".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(path.with_file_name("notes-backup.txt")).unwrap(), "theirs, longer");

        // A file that exists but cannot be read (a directory stands in for a locked file).
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut g = GlobalNote::new(path.clone());
        assert!(g.ensure_loaded().is_some(), "the failed read is reported");
        assert_eq!(g.text, "");
        // Typing over the emptied view never writes over the unread file; it retries.
        assert_eq!(g.edited("typed".into(), now), Ok(Wrote::Deferred));
        assert!(path.is_dir());
        assert!(g.due().is_some(), "a retry is armed");
        // The file reads again: it wins, and the typing is kept aside.
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "the real note").unwrap();
        assert_eq!(g.tick(now + DEBOUNCE), Ok(Wrote::Adopted));
        assert_eq!(g.text, "the real note");
        assert_eq!(std::fs::read_to_string(path.with_file_name("notes-discarded.txt")).unwrap(), "typed");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the real note");

        // Unreadable, then gone before the user typed: the typing is a new note.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut g = GlobalNote::new(path.clone());
        assert!(g.ensure_loaded().is_some());
        std::fs::remove_dir(&path).unwrap();
        assert_eq!(g.edited("new".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");

        // A large note waits for the debounce, and a flush writes it.
        let big = "x".repeat(LARGE);
        assert_eq!(g.edited(big.clone(), now), Ok(Wrote::Nothing));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(g.tick(now), Ok(Wrote::Nothing));
        assert_eq!(g.flush(), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), big);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
