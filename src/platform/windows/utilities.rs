//! Windows utilities folder beside termurion.exe: one hard link (or copy) of `termurion-utils.exe`
//! per utility name, brought up to date at every start.

use std::path::Path;

use crate::utilities::{stale, Ensured, Stamp, API_SCRIPT, EXE, FOLDER};

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    Some(Stamp { len: m.len(), modified })
}

/// Makes `exe_dir\utilities` and brings one `<name>.exe` per `util_cli::NAMES` entry up to
/// date with `exe_dir\termurion-utils.exe`: a stale one is removed (or, when it is running,
/// renamed aside to `<name>.exe.old`, which the next start removes) and hard linked again, or
/// copied when a link cannot be made (another volume, a file system without links).
/// `it2api.py` is written when its bytes are not `util_api::script()`'s. Every file that
/// cannot be brought up to date is reported in `failed`. Files Termurion does not make are never
/// touched. Err, with nothing probed, when Termurion's folder is a network path (in any
/// spelling) or not a local drive path; Err when a link on the way leads to a network path,
/// when `termurion-utils.exe` is missing (no folder is made) or the folder cannot be made.
pub fn ensure(exe_dir: &Path) -> Result<Ensured, String> {
    let text = exe_dir.to_string_lossy();
    let local = crate::util_cli::local_form(&text);
    if crate::prompted::network_path(local) {
        return Err(format!("Termurion runs from a network folder ({text}), so the utilities are not put on PATH"));
    }
    if !crate::prompted::local_path_ok(local) {
        return Err(format!("Termurion's folder ({text}) is not a local folder, so the utilities are not put on PATH"));
    }
    let exe_dir = Path::new(local);
    let folder = exe_dir.join(FOLDER);
    if !crate::util_cli::may_open(&folder) {
        return Err(format!("{} leads to a network folder, so the utilities are not put on PATH", folder.display()));
    }
    let src = exe_dir.join(EXE);
    let Some(src_stamp) = stamp(&src) else { return Err(format!("{} is missing", src.display())) };
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut failed = Vec::new();
    for name in crate::util_cli::NAMES {
        let file = format!("{name}.exe");
        let dst = folder.join(&file);
        let old = folder.join(format!("{file}.old"));
        // Set aside by an earlier start; still running when it cannot be removed.
        if old.exists() {
            let _ = std::fs::remove_file(&old);
        }
        if !stale(src_stamp, stamp(&dst)) {
            continue;
        }
        if let Err(e) = refresh(&src, &dst, &old) {
            failed.push(format!("{file}: {e}"));
        }
    }
    let api = folder.join(API_SCRIPT);
    let script = crate::util_api::script();
    if std::fs::read(&api).ok().as_deref() != Some(script.as_bytes())
        && let Err(e) = std::fs::write(&api, script) {
            failed.push(format!("{API_SCRIPT}: {e}"));
        }
    Ok(Ensured { folder, failed })
}

/// Replaces `dst` with a link to (or a copy of) `src`. A `dst` that cannot be removed (a
/// running utility on older systems) can still be renamed, so it is moved to `old` first. A
/// folder of that name is never touched.
fn refresh(src: &Path, dst: &Path, old: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dst) {
        // Not a file Termurion made: left alone.
        Ok(m) if m.is_dir() => return Err(std::io::Error::other("a folder has this name")),
        Ok(_) if std::fs::remove_file(dst).is_err() => std::fs::rename(dst, old)?,
        _ => {}
    }
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}
