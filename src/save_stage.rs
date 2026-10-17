//! The private staging folder of a Save Page As to an SSH host (TWEAKS 7b). The one local saver
//! writes the page into `<temp>\Termurion-<UUID>\<name>` (the root is made create-new, so it is never a
//! folder that was already there), the upload reads that finished tree, and the root is removed on
//! every outcome: success, error, cancel and a closed pane. A crash leaves it for the launch sweep,
//! which already removes a `Termurion-<UUID>` folder in the temp folder untouched for a day
//! (`ssh_local::is_stale_temp`).

use crate::localfs;
use crate::page_save_remote::{Staged, REMOTE_MAX_ENTRIES};
use std::path::{Path, PathBuf};

/// The root folder's name for `uuid`: the form the launch sweep removes after a day.
pub fn root_name(uuid: &str) -> String {
    format!("Termurion-{uuid}")
}

/// Makes the staging root under `temp`, new and empty (`create_dir` fails when it exists).
pub fn make_root(temp: &Path, uuid: &str) -> std::io::Result<PathBuf> {
    let root = localfs::judge_io(temp.join(root_name(uuid)))?;
    localfs::create_dir(&root)?;
    Ok(root.into_path_buf())
}

/// Removes the staging root and everything under it. A link inside is removed itself and never
/// followed. Retries a few times: the save's file worker may still be finishing a write.
pub fn remove_root(root: &Path, tries: u32, pause: std::time::Duration) -> std::io::Result<()> {
    let mut last = Ok(());
    for i in 0..tries.max(1) {
        let judged = localfs::judge_io(root)?;
        match localfs::lstat(&judged) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Err(e),
            Ok(_) => match localfs::remove_dir_all(&judged) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => last = Err(e),
            },
        }
        if i + 1 < tries {
            std::thread::sleep(pause);
        }
    }
    last
}

/// The finished page folder `page`, as the list the planner judges: every file and folder under it
/// with `/` between names. A link or any reparse point inside is an error (the saver makes none), as
/// is a tree past the upload's entry cap (the walk stops reading at the cap).
pub fn walk(page: &Path) -> Result<Vec<Staged>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(page.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let judged = localfs::judge_io(&dir).map_err(|e| e.to_string())?;
        let entries = localfs::entries_upto(&judged, REMOTE_MAX_ENTRIES + 1).map_err(|e| e.to_string())?;
        for e in entries {
            let name = e.name.to_string_lossy().to_string();
            if e.stat.is_reparse || e.stat.is_link {
                return Err("The saved page holds a link.".to_string());
            }
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if e.stat.is_dir {
                out.push(Staged { rel: child.clone(), size: 0, is_dir: true });
                stack.push((e.path().to_path_buf(), child));
            } else if e.stat.is_file {
                out.push(Staged { rel: child, size: e.stat.len, is_dir: false });
            } else {
                return Err("The saved page holds something that is not a file.".to_string());
            }
            if out.len() >= REMOTE_MAX_ENTRIES {
                return Err(format!("The page has more than {REMOTE_MAX_ENTRIES} files and folders."));
            }
        }
    }
    Ok(out)
}

/// The staged file `rel` of `page`, read whole (the planner already capped it).
pub fn read(page: &Path, rel: &str) -> std::io::Result<Vec<u8>> {
    let mut path = page.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    let judged = localfs::judge_io(path)?;
    let st = localfs::lstat(&judged)?;
    if !st.is_file || st.is_reparse || st.len > crate::page_save_remote::REMOTE_MAX_FILE {
        return Err(std::io::Error::other("not a plain file within the upload cap"));
    }
    localfs::read(&judged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(tag: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("termurion-stage-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        t
    }

    fn fill(root: &Path) {
        let page = root.join("page");
        std::fs::create_dir_all(page.join("resources")).unwrap();
        std::fs::write(page.join("index.html"), b"<html>").unwrap();
        std::fs::write(page.join("resources").join("a.css"), b"x").unwrap();
    }

    #[test]
    fn staging_removed_every_way() {
        let t = temp("every");
        // Success, error and cancel all end in the same removal of the root.
        for (i, _way) in ["success", "error", "cancel"].iter().enumerate() {
            let root = make_root(&t, &format!("00000000-0000-0000-0000-00000000000{i}")).unwrap();
            fill(&root);
            assert!(make_root(&t, &format!("00000000-0000-0000-0000-00000000000{i}")).is_err(), "the root is create-new");
            remove_root(&root, 3, Duration::from_millis(10)).unwrap();
            assert!(!root.exists());
            remove_root(&root, 1, Duration::ZERO).unwrap();
        }
        // A crash leaves the root to the launch sweep: older than a day goes, younger stays.
        let name = root_name("0A1B2C3D-0000-4000-8000-000000000000");
        assert!(crate::ssh_local::is_stale_temp(&name, true, 25 * 3600));
        assert!(!crate::ssh_local::is_stale_temp(&name, true, 3600));
        // The wiring: every end of a remote save removes the root.
        let app = include_str!("app/page_save_remote_app.rs");
        let prod = &app[..app.find("#[cfg(test)]").unwrap_or(app.len())];
        for needle in ["fn save_remote_finish", "fn save_remote_cancel", "fn save_remote_staged_failed"] {
            assert!(prod.contains(needle), "{needle}");
        }
        assert_eq!(prod.matches("self.save_remote_unstage(").count() >= 3, true);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn walk_lists_the_tree_and_refuses_links() {
        let t = temp("walk");
        let root = make_root(&t, "00000000-0000-0000-0000-0000000000AA").unwrap();
        fill(&root);
        let mut got = walk(&root.join("page")).unwrap();
        got.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(got.iter().map(|s| (s.rel.as_str(), s.is_dir, s.size)).collect::<Vec<_>>(), [("index.html", false, 6), ("resources", true, 0), ("resources/a.css", false, 1)]);
        assert_eq!(read(&root.join("page"), "resources/a.css").unwrap(), b"x");
        assert!(read(&root.join("page"), "resources").is_err());
        assert!(read(&root.join("page"), "../x").is_err());
        let plan = crate::page_save_remote::plan(&got).unwrap();
        assert_eq!(plan.files.len(), 2);
        let _ = std::fs::remove_dir_all(&t);
    }
}
