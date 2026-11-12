//! The `FXProfiles` folder: shipped profile files, the user's edits, reload by polling.

use crate::fx_profile::{self, parse, FxRules, ParseError, SHIPPED};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const FOLDER: &str = "FXProfiles";
const BOOKKEEPING: &str = ".shipped.toml";
const MAX_PROFILES: usize = 64;
const SCAN_EVERY: Duration = Duration::from_secs(1);

/// `FXProfiles` beside the settings file.
pub fn user_dir() -> PathBuf {
    crate::globals::default_path().parent().map_or_else(|| PathBuf::from(FOLDER), |d| d.join(FOLDER))
}

pub struct Loaded {
    pub rules: Arc<FxRules>,
    pub file: PathBuf,
    /// The embedded text, when this file is one Termurion ships.
    pub shipped: Option<&'static str>,
    /// The user edited a shipped file.
    pub changed: bool,
    /// An edited shipped file whose embedded text has moved on.
    pub newer_shipped: bool,
    pub warnings: Vec<String>,
    pub raw: toml::Table,
}

#[derive(Debug)]
pub enum ImportError {
    Exists(String),
    Invalid(ParseError),
    Io(io::Error),
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        ImportError::Io(e)
    }
}

type Stamp = Vec<(String, u64, Option<SystemTime>)>;

pub struct Store {
    pub profiles: Vec<Loaded>,
    pub errors: Vec<(PathBuf, ParseError)>,
    dir: PathBuf,
    stamp: Stamp,
    last_scan: Option<Instant>,
}

/// FNV-1a 64 as 16 hex digits.
fn hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[derive(Default)]
struct Record {
    hash: String,
    deleted: bool,
}

fn read_records(dir: &Path) -> BTreeMap<String, Record> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(dir.join(BOOKKEEPING)) else { return out };
    let Ok(t) = text.parse::<toml::Table>() else { return out };
    for (k, v) in t {
        let Some(v) = v.as_table() else { continue };
        let hash = v.get("hash").and_then(|h| h.as_str()).unwrap_or_default().to_string();
        let deleted = v.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false);
        out.insert(k, Record { hash, deleted });
    }
    out
}

fn write_records(dir: &Path, recs: &BTreeMap<String, Record>) -> io::Result<()> {
    let mut t = toml::Table::new();
    for (k, r) in recs {
        let mut e = toml::Table::new();
        e.insert("hash".into(), r.hash.clone().into());
        e.insert("deleted".into(), r.deleted.into());
        t.insert(k.clone(), e.into());
    }
    write_atomic(&dir.join(BOOKKEEPING), &toml::to_string(&t).unwrap_or_default())
}

fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "profile".into() } else { s }
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let mut s = Store { profiles: Vec::new(), errors: Vec::new(), dir: dir.to_path_buf(), stamp: Vec::new(), last_scan: None };
        if let Err(e) = std::fs::create_dir_all(dir) {
            s.errors.push((dir.to_path_buf(), ParseError { line: None, message: format!("cannot create the folder: {e}") }));
            return s;
        }
        s.sync_shipped();
        s.reload();
        s
    }

    /// First start, updates and deletions of the shipped files.
    fn sync_shipped(&mut self) {
        let mut recs = read_records(&self.dir);
        for (name, text) in SHIPPED {
            let path = self.dir.join(name);
            let embedded = hash(text.as_bytes());
            match std::fs::read(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => match recs.get_mut(*name) {
                    Some(r) => r.deleted = true,
                    None => {
                        if write_atomic(&path, text).is_ok() {
                            recs.insert((*name).into(), Record { hash: embedded, deleted: false });
                        }
                    }
                },
                Err(_) => {}
                Ok(bytes) => {
                    let h = hash(&bytes);
                    let r = recs.entry((*name).into()).or_default();
                    r.deleted = false;
                    if h == embedded {
                        r.hash = h;
                    } else if h == r.hash && write_atomic(&path, text).is_ok() {
                        r.hash = embedded;
                    }
                }
            }
        }
        let _ = write_records(&self.dir, &recs);
    }

    fn scan(&self) -> Stamp {
        let mut v: Stamp = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.ends_with(".toml") || n == BOOKKEEPING {
                    return None;
                }
                let m = e.metadata().ok()?;
                m.is_file().then(|| (n, m.len(), m.modified().ok()))
            })
            .collect();
        v.sort();
        v
    }

    fn reload(&mut self) {
        let recs = read_records(&self.dir);
        self.stamp = self.scan();
        self.profiles.clear();
        self.errors.clear();
        for (n, _, _) in &self.stamp {
            let path = self.dir.join(n);
            let fail = |line, message: String| ParseError { line, message };
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    self.errors.push((path, fail(None, format!("cannot read: {e}"))));
                    continue;
                }
            };
            if text.trim().is_empty() {
                self.errors.push((path, fail(None, "the file is empty".into())));
                continue;
            }
            let (rules, warnings) = match parse(&text) {
                Ok(r) => r,
                Err(e) => {
                    self.errors.push((path, e));
                    continue;
                }
            };
            if self.profiles.len() >= MAX_PROFILES {
                self.errors.push((path, fail(None, format!("more than {MAX_PROFILES} profiles"))));
                continue;
            }
            if self.profiles.iter().any(|p| p.rules.name == rules.name) {
                self.errors.push((path, fail(None, format!("duplicate name \"{}\"", rules.name))));
                continue;
            }
            let shipped = SHIPPED.iter().find(|(f, _)| f == n).map(|(_, t)| *t);
            let (mut changed, mut newer_shipped) = (false, false);
            if let Some(t) = shipped {
                let h = hash(text.as_bytes());
                let rec = recs.get(n).map(|r| r.hash.as_str()).unwrap_or_default();
                changed = h != hash(t.as_bytes()) && h != rec;
                newer_shipped = changed && hash(t.as_bytes()) != rec;
            }
            let raw = text.parse::<toml::Table>().unwrap_or_default();
            self.profiles.push(Loaded { rules: Arc::new(rules), file: path, shipped, changed, newer_shipped, warnings, raw });
        }
    }

    /// Looks at the folder at most once a second. True when anything changed.
    pub fn refresh(&mut self, now: Instant) -> bool {
        if self.last_scan.is_some_and(|t| now.saturating_duration_since(t) < SCAN_EVERY) {
            return false;
        }
        self.last_scan = Some(now);
        if self.scan() == self.stamp {
            return false;
        }
        self.reload();
        true
    }

    pub fn shell(&self) -> Arc<FxRules> {
        self.by_name("Shell").unwrap_or_else(|| Arc::new(FxRules::default()))
    }

    pub fn by_name(&self, name: &str) -> Option<Arc<FxRules>> {
        self.profiles.iter().find(|p| p.rules.name == name).map(|p| p.rules.clone())
    }

    pub fn find_rules(&self) -> Arc<FxRules> {
        let all: Vec<&FxRules> = self.profiles.iter().map(|p| &*p.rules).collect();
        Arc::new(fx_profile::union(&all))
    }

    /// Writes back every shipped file that is gone.
    pub fn restore_shipped(&mut self) -> io::Result<()> {
        let mut recs = read_records(&self.dir);
        for (name, text) in SHIPPED {
            let path = self.dir.join(name);
            if !path.exists() {
                write_atomic(&path, text)?;
                recs.insert((*name).into(), Record { hash: hash(text.as_bytes()), deleted: false });
            }
        }
        write_records(&self.dir, &recs)?;
        self.reload();
        Ok(())
    }

    fn file_of(&self, name: &str) -> io::Result<&Loaded> {
        self.profiles.iter().find(|p| p.rules.name == name).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no profile named {name}")))
    }

    /// Puts a shipped profile back to its embedded text.
    pub fn reset(&mut self, name: &str) -> io::Result<()> {
        let p = self.file_of(name)?;
        let Some(text) = p.shipped else { return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a shipped profile")) };
        let (path, file) = (p.file.clone(), p.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
        write_atomic(&path, text)?;
        let mut recs = read_records(&self.dir);
        recs.insert(file, Record { hash: hash(text.as_bytes()), deleted: false });
        write_records(&self.dir, &recs)?;
        self.reload();
        Ok(())
    }

    fn fresh_path(&self, name: &str) -> PathBuf {
        let base = slug(name);
        let mut p = self.dir.join(format!("{base}.toml"));
        let mut n = 2;
        while p.exists() {
            p = self.dir.join(format!("{base}-{n}.toml"));
            n += 1;
        }
        p
    }

    /// Writes a profile. `file` None makes a new file named after the profile.
    pub fn save(&mut self, rules: &FxRules, keep: &toml::Table, file: Option<&Path>) -> io::Result<PathBuf> {
        if self.profiles.iter().any(|p| p.rules.name == rules.name && Some(p.file.as_path()) != file) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("a profile named {} exists", rules.name)));
        }
        let path = file.map_or_else(|| self.fresh_path(&rules.name), Path::to_path_buf);
        write_atomic(&path, &fx_profile::to_toml(rules, keep))?;
        self.reload();
        Ok(path)
    }

    pub fn delete(&mut self, name: &str) -> io::Result<()> {
        let path = self.file_of(name)?.file.clone();
        std::fs::remove_file(&path)?;
        if let Some(f) = path.file_name().map(|f| f.to_string_lossy().into_owned()) {
            if SHIPPED.iter().any(|(s, _)| *s == f) {
                let mut recs = read_records(&self.dir);
                recs.entry(f).or_default().deleted = true;
                write_records(&self.dir, &recs)?;
            }
        }
        self.reload();
        Ok(())
    }

    /// Copies a profile file in. Returns the profile's name.
    pub fn import(&mut self, from: &Path, replace: bool) -> Result<String, ImportError> {
        let text = std::fs::read_to_string(from)?;
        let (rules, _) = parse(&text).map_err(ImportError::Invalid)?;
        let existing = self.profiles.iter().find(|p| p.rules.name == rules.name).map(|p| p.file.clone());
        let path = match existing {
            Some(_) if !replace => return Err(ImportError::Exists(rules.name)),
            Some(f) => f,
            None => self.fresh_path(&rules.name),
        };
        write_atomic(&path, &text)?;
        self.reload();
        Ok(rules.name)
    }

    pub fn export(&self, name: &str, to: &Path) -> io::Result<()> {
        std::fs::copy(&self.file_of(name)?.file, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fx_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn named(n: &str) -> String {
        format!("name = \"{n}\"\n")
    }

    #[test]
    fn first_open_writes_the_six_shipped_files() {
        let d = tmp("first");
        let s = Store::open(&d.join("FXProfiles"));
        for (f, _) in SHIPPED {
            assert!(d.join("FXProfiles").join(f).exists());
        }
        assert!(d.join("FXProfiles").join(BOOKKEEPING).exists());
        assert_eq!(s.profiles.len(), 6);
        assert!(s.errors.is_empty());
        assert!(s.profiles.iter().all(|p| !p.changed));
    }

    #[test]
    fn an_unchanged_shipped_file_is_updated() {
        let d = tmp("update");
        Store::open(&d);
        let old = "name = \"Codex\"\n# old\n";
        std::fs::write(d.join("codex.toml"), old).unwrap();
        let mut recs = read_records(&d);
        recs.get_mut("codex.toml").unwrap().hash = hash(old.as_bytes());
        write_records(&d, &recs).unwrap();
        let s = Store::open(&d);
        let (_, text) = SHIPPED.iter().find(|(f, _)| *f == "codex.toml").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("codex.toml")).unwrap(), *text);
        assert!(s.profiles.iter().all(|p| !p.changed));
    }

    #[test]
    fn a_changed_shipped_file_is_kept_and_flagged() {
        let d = tmp("changed");
        Store::open(&d);
        let mine = "name = \"Codex\"\n# mine\n";
        std::fs::write(d.join("codex.toml"), mine).unwrap();
        let s = Store::open(&d);
        assert_eq!(std::fs::read_to_string(d.join("codex.toml")).unwrap(), mine);
        let p = s.profiles.iter().find(|p| p.rules.name == "Codex").unwrap();
        assert!(p.changed && !p.newer_shipped);
        // Pretend the shipped text moved on since the last write.
        let mut recs = read_records(&d);
        recs.get_mut("codex.toml").unwrap().hash = hash(b"older text");
        write_records(&d, &recs).unwrap();
        let s = Store::open(&d);
        let p = s.profiles.iter().find(|p| p.rules.name == "Codex").unwrap();
        assert!(p.changed && p.newer_shipped);
        assert_eq!(std::fs::read_to_string(d.join("codex.toml")).unwrap(), mine);
    }

    #[test]
    fn a_deleted_shipped_profile_stays_deleted_after_restart() {
        let d = tmp("deleted");
        Store::open(&d);
        std::fs::remove_file(d.join("hermes.toml")).unwrap();
        let s = Store::open(&d);
        assert!(!d.join("hermes.toml").exists());
        assert_eq!(s.profiles.len(), 5);
        let mut s = Store::open(&d);
        assert!(!d.join("hermes.toml").exists());
        s.restore_shipped().unwrap();
        assert!(d.join("hermes.toml").exists());
        assert_eq!(s.profiles.len(), 6);
        assert_eq!(Store::open(&d).profiles.len(), 6);
    }

    #[test]
    fn a_broken_file_is_an_error_and_others_load() {
        let d = tmp("broken");
        Store::open(&d);
        std::fs::write(d.join("zz.toml"), "name = [").unwrap();
        let s = Store::open(&d);
        assert_eq!(s.profiles.len(), 6);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].0.ends_with("zz.toml"));
        assert!(s.errors[0].1.line.is_some());
    }

    #[test]
    fn a_half_written_file_is_an_error_until_it_is_whole() {
        let d = tmp("half");
        let mut s = Store::open(&d);
        let t0 = Instant::now();
        std::fs::write(d.join("new.toml"), "").unwrap();
        assert!(s.refresh(t0 + Duration::from_secs(2)));
        assert_eq!(s.errors.len(), 1);
        std::fs::write(d.join("new.toml"), "[done]\nquiet").unwrap();
        assert!(s.refresh(t0 + Duration::from_secs(4)));
        assert_eq!(s.errors.len(), 1);
        std::fs::write(d.join("new.toml"), named("New")).unwrap();
        assert!(s.refresh(t0 + Duration::from_secs(6)));
        assert!(s.errors.is_empty());
        assert!(s.by_name("New").is_some());
    }

    #[test]
    fn duplicate_names_keep_the_first_file() {
        let d = tmp("dup");
        Store::open(&d);
        std::fs::write(d.join("a-x.toml"), named("X")).unwrap();
        std::fs::write(d.join("b-x.toml"), named("X")).unwrap();
        let s = Store::open(&d);
        assert!(s.profiles.iter().any(|p| p.file.ends_with("a-x.toml")));
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].0.ends_with("b-x.toml"));
        assert!(s.errors[0].1.message.contains("duplicate"));
    }

    #[test]
    fn the_65th_profile_is_an_error() {
        let d = tmp("many");
        Store::open(&d);
        for i in 0..60 {
            std::fs::write(d.join(format!("p{i:02}.toml")), named(&format!("P{i}"))).unwrap();
        }
        let s = Store::open(&d);
        assert_eq!(s.profiles.len(), 64);
        assert_eq!(s.errors.len(), 2);
        assert!(s.errors[0].1.message.contains("64"));
    }

    #[test]
    fn shell_is_there_even_without_its_file() {
        let d = tmp("shell");
        Store::open(&d);
        std::fs::remove_file(d.join("shell.toml")).unwrap();
        let s = Store::open(&d);
        assert_eq!(*s.shell(), FxRules::default());
    }

    #[test]
    fn refresh_sees_a_new_file_within_a_second() {
        let d = tmp("refresh");
        let mut s = Store::open(&d);
        let t0 = Instant::now();
        assert!(!s.refresh(t0));
        std::fs::write(d.join("extra.toml"), named("Extra")).unwrap();
        assert!(!s.refresh(t0 + Duration::from_millis(500)));
        assert!(s.by_name("Extra").is_none());
        assert!(s.refresh(t0 + Duration::from_millis(1100)));
        assert!(s.by_name("Extra").is_some());
    }

    #[test]
    fn import_refuses_an_existing_name_unless_replace() {
        let d = tmp("import");
        let mut s = Store::open(&d.join("store"));
        let src = d.join("in.toml");
        std::fs::write(&src, named("Codex")).unwrap();
        assert!(matches!(s.import(&src, false), Err(ImportError::Exists(n)) if n == "Codex"));
        assert_eq!(s.import(&src, true).unwrap(), "Codex");
        assert_eq!(s.profiles.len(), 6);
        std::fs::write(&src, named("Fresh")).unwrap();
        assert_eq!(s.import(&src, false).unwrap(), "Fresh");
        assert_eq!(s.profiles.len(), 7);
        std::fs::write(&src, "name = [").unwrap();
        assert!(matches!(s.import(&src, false), Err(ImportError::Invalid(_))));
    }

    #[test]
    fn export_writes_the_profile_file() {
        let d = tmp("export");
        let s = Store::open(&d.join("store"));
        let out = d.join("out.toml");
        s.export("Codex", &out).unwrap();
        let (_, text) = SHIPPED.iter().find(|(f, _)| *f == "codex.toml").unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), *text);
    }

    #[test]
    fn save_delete_and_reset_round_trip() {
        let d = tmp("save");
        let mut s = Store::open(&d);
        let mut r = FxRules::default();
        r.name = "Mine".into();
        let p = s.save(&r, &toml::Table::new(), None).unwrap();
        assert!(p.ends_with("mine.toml"));
        assert!(s.by_name("Mine").is_some());
        s.delete("Mine").unwrap();
        assert!(s.by_name("Mine").is_none());
        std::fs::write(d.join("codex.toml"), "name = \"Codex\"\n# mine\n").unwrap();
        let mut s = Store::open(&d);
        s.reset("Codex").unwrap();
        assert!(!s.profiles.iter().find(|p| p.rules.name == "Codex").unwrap().changed);
    }
}
