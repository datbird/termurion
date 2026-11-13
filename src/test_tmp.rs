//! Temp folders for tests. Every test that needs scratch space uses `TempDir`, which deletes
//! itself on drop (also while a failed test unwinds), so a test run leaves nothing behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty folder under the temp folder for one test, removed when dropped.
pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
    /// `name` is a short tag; a leading "termurion-" is not repeated. Keep it short (socket paths).
    pub fn new(name: &str) -> TempDir {
        TempDir::new_in(&std::env::temp_dir(), name)
    }

    /// Like `new`, under another base folder (one the code under test accepts).
    pub fn new_in(base: &Path, name: &str) -> TempDir {
        let name = name.strip_prefix("termurion-").unwrap_or(name);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("termurion-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the folder (not created).
    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Files in these (file:reason) may keep a raw temp_dir() in their test code.
const ALLOW: &[&str] = &[
    "localfs.rs:checks the temp folder itself, creates nothing",
    "platform/paths.rs:the fallback base must equal the temp folder, creates nothing",
    "tmux_live.rs:ignored live tests share one empty psmux config file, reused every run",
    "platform/unix/ipc.rs:own Scratch wrapper removes its folders, links and sockets on drop",
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_raw_temp_dir_in_tests() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    let mut bad = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "test_tmp.rs" || ALLOW.iter().any(|a| a.split(':').next() == Some(rel.as_str())) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        // The test module is the first `#[cfg(test)]` that is followed by a `mod` line.
        let start = (0..lines.len()).find(|&i| {
            lines[i].trim() == "#[cfg(test)]"
                && lines.get(i + 1).is_some_and(|n| { let n = n.trim_start(); n.starts_with("mod ") || (n.starts_with("pub") && n.contains(" mod ")) })
        });
        let Some(start) = start else { continue };
        for (i, l) in lines.iter().enumerate().skip(start) {
            if l.contains("temp_dir()") || l.contains("env::temp_dir") {
                bad.push(format!("src/{rel}:{}", i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "use test_tmp::TempDir, not temp_dir(), in tests:\n{}", bad.join("\n"));
}
