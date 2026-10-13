//! THE extractor: every archive Termurion unpacks from outside (Import All's gzip tar, a script zip, a
//! remote folder's zip) is read once into memory, checked whole by `zip_check`'s walk, and then
//! written by Termurion from the same bytes through the same walk (final findings r2, the class fix:
//! no tar.exe, so no second parser, no reopen by path, no filter guessed). Only plain files and
//! folders are written, each a new item under `to` (`create_new`: nothing that is there already is
//! replaced or followed), and a file gets exactly the bytes its entry claims.

use crate::localfs::{self, LocalPath};
use crate::zip_check::{self, Limits};
use std::path::Path;

/// The most an archive file may weigh to be read whole.
pub const MAX_ARCHIVE: u64 = 1 << 30;

/// The file at `path`, judged and read through ONE handle, at most `max` bytes (refused above).
pub fn read_whole(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let shown = |e: std::io::Error| format!("{}: {e}", path.display());
    let file = localfs::judge_io(path).and_then(|p| localfs::open(&p)).map_err(shown)?;
    let len = localfs::file_stat(&file).map_err(shown)?.len;
    if len > max {
        return Err(zip_check::refused("the archive is too large"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(max + 1).read_to_end(&mut bytes).map_err(shown)?;
    if bytes.len() as u64 > max {
        return Err(zip_check::refused("the archive is too large"));
    }
    Ok(bytes)
}

/// Extracts the gzip tar `bytes` into the folder `to` (made when missing): checked whole first
/// (`zip_check::check_tgz` with `more`), then written by the same walk. The total size written.
pub fn tgz(bytes: &[u8], to: &Path, limits: Limits, more: &dyn Fn(&str) -> bool) -> Result<u64, String> {
    zip_check::check_tgz(bytes, limits, more)?;
    let mut write = writer(to)?;
    zip_check::walk_tgz(bytes, limits, more, &mut write)
}

/// Extracts the zip `bytes` into the folder `to` (made when missing): checked whole first
/// (`zip_check::check_total`), then written by the same walk. The total size written.
pub fn zip(bytes: &[u8], to: &Path, limits: Limits) -> Result<u64, String> {
    zip_check::check_total(bytes, limits)?;
    let mut write = writer(to)?;
    zip_check::walk_zip(bytes, limits, Some(&mut write))
}

/// The visitor that writes each checked entry under `to`: a folder (and any folder above an entry)
/// made, a file made new and given exactly its claimed bytes.
fn writer(to: &Path) -> Result<impl FnMut(&str, Option<&mut dyn std::io::Read>, u64) -> Result<(), String>, String> {
    let base = localfs::judge_io(to).map_err(|e| format!("{}: {e}", to.display()))?;
    localfs::create_dir_all(&base).map_err(|e| format!("{}: {e}", to.display()))?;
    Ok(move |name: &str, data: Option<&mut dyn std::io::Read>, size: u64| {
        let failed = |e: std::io::Error| zip_check::refused(&format!("\u{201c}{}\u{201d} could not be written ({e})", name.escape_debug()));
        let parts: Vec<&str> = name.trim_end_matches('/').split('/').collect();
        let (last, above) = parts.split_last().ok_or_else(|| zip_check::refused("an entry has no name"))?;
        let mut folder: LocalPath = base.clone();
        for part in above {
            folder = folder.child(part).ok_or_else(localfs::refused).map_err(failed)?;
            make_dir(&folder).map_err(failed)?;
        }
        let item = folder.child(last).ok_or_else(localfs::refused).map_err(failed)?;
        let Some(data) = data else { return make_dir(&item).map_err(failed) };
        let mut file = localfs::open_custom(&item, |o| {
            o.write(true).create_new(true);
        })
        .map_err(failed)?;
        let wrote = std::io::copy(data, &mut file).map_err(failed)?;
        if wrote != size {
            return Err(zip_check::refused(&format!("\u{201c}{}\u{201d} holds less data than it claims", name.escape_debug())));
        }
        Ok(())
    })
}

/// A folder made, or one already there kept (never a file or a link of that name).
fn make_dir(p: &LocalPath) -> std::io::Result<()> {
    match localfs::create_dir(p) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && localfs::lstat(p).is_ok_and(|s| s.is_dir && !s.is_reparse) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_check::tests::{dir, file, tar_entry, tgz as gz, zip as make_zip};

    fn fresh(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("termurion-extract-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![(String::new(), dir.to_path_buf())];
        while let Some((rel, d)) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let name = format!("{rel}{}", e.file_name().to_string_lossy());
                if e.file_type().unwrap().is_dir() {
                    out.push(format!("{name}/"));
                    stack.push((format!("{name}/"), e.path()));
                } else {
                    out.push(format!("{name}={}", std::fs::read_to_string(e.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// Final findings r2, the class fix: the walk that checks is the walk that writes, plain files
    /// and folders only, each new; a refused archive writes nothing.
    #[test]
    fn one_walk_checks_and_writes() {
        let none = |_: &str| false;
        let t = fresh("tgz");
        let blocks = [tar_entry("d/", b'5', b"", ""), tar_entry("d/a.txt", b'0', b"abc", ""), tar_entry("e/f/g.txt", b'0', b"g", "")].concat();
        assert_eq!(tgz(&gz(&blocks), &t.join("out"), Limits::DEFAULT, &none), Ok(4));
        assert_eq!(listing(&t.join("out")), ["d/", "d/a.txt=abc", "e/", "e/f/", "e/f/g.txt=g"]);
        // A refused archive (a link after a good file) writes nothing at all.
        let bad = [tar_entry("d/a.txt", b'0', b"abc", ""), tar_entry("d/l", b'1', b"", "../../x")].concat();
        assert!(tgz(&gz(&bad), &t.join("bad"), Limits::DEFAULT, &none).is_err());
        assert!(!t.join("bad").exists());
        // Two entries of one name (any case): the second is never written over the first.
        let twice = [tar_entry("d/a.txt", b'0', b"one", ""), tar_entry("d/A.TXT", b'0', b"two", "")].concat();
        assert!(tgz(&gz(&twice), &t.join("twice"), Limits::DEFAULT, &none).is_err());
        assert_eq!(std::fs::read_to_string(t.join("twice").join("d").join("a.txt")).unwrap(), "one");

        // A zip: stored and deflated data, folders, each exactly as claimed.
        let z = make_zip(&[dir("d/"), file("d/a.txt", "abc"), file("d/s/b.txt", "b")]);
        assert_eq!(zip(&z, &t.join("z"), Limits::DEFAULT), Ok(4));
        assert_eq!(listing(&t.join("z")), ["d/", "d/a.txt=abc", "d/s/", "d/s/b.txt=b"]);
        // Data that is not what the directory says (a changed byte: the CRC) fails the extraction.
        let mut torn = make_zip(&[file("d/a.txt", "abc")]);
        let at = torn.windows(3).position(|w| w == b"abc").unwrap();
        torn[at] = b'x';
        assert!(zip(&torn, &t.join("torn"), Limits::DEFAULT).is_err());
        let _ = std::fs::remove_dir_all(&t);
    }

    /// A deflated zip as tar.exe writes it (bsdtar's own zip writer) extracts the same bytes.
    #[test]
    fn a_deflated_zip_extracts() {
        let t = fresh("deflate");
        std::fs::create_dir_all(t.join("src").join("d")).unwrap();
        std::fs::write(t.join("src").join("d").join("a.txt"), "a".repeat(5000)).unwrap();
        let tar = crate::script_provision::RealRunner::system_tool("tar.exe").unwrap();
        let made = std::process::Command::new(tar).current_dir(t.join("src")).args(["-a", "-cf", "x.zip", "d"]).status().unwrap();
        assert!(made.success());
        let bytes = read_whole(&t.join("src").join("x.zip"), MAX_ARCHIVE).unwrap();
        assert_eq!(zip(&bytes, &t.join("out"), Limits::DEFAULT), Ok(5000));
        assert_eq!(std::fs::read_to_string(t.join("out").join("d").join("a.txt")).unwrap(), "a".repeat(5000));
        assert!(read_whole(&t.join("src").join("x.zip"), 10).is_err(), "over the cap");
        let _ = std::fs::remove_dir_all(&t);
    }

    /// One entry's zip with its data as given (`method` 0 stored, 8 deflated), the sizes and CRC
    /// the directory claims.
    fn one_entry(name: &str, method: u16, data: &[u8], claimed: u32, crc: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20u16, 0, method, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(claimed.to_le_bytes());
        out.extend([name.len() as u16, 0].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(name.as_bytes());
        out.extend(data);
        let cd = out.len() as u32;
        out.extend(0x0201_4b50u32.to_le_bytes());
        out.extend([0x031eu16, 20, 0, method, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(claimed.to_le_bytes());
        out.extend([name.len() as u16, 0, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(((0o100_644u32) << 16).to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(name.as_bytes());
        let cd_len = out.len() as u32 - cd;
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u16, 0, 1, 1].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(cd_len.to_le_bytes());
        out.extend(cd.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    /// R14de-1 (zip bombs), now without a watch: a deflated entry is read only to the size its
    /// directory claims, so data that expands further fails the extraction with at most the claim
    /// written; an encrypted entry or another method is refused before anything is written.
    #[test]
    fn a_zip_bomb_writes_no_more_than_its_claim() {
        use std::io::Write;
        let t = fresh("bomb");
        let plain = vec![b'z'; 1 << 20];
        let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        deflate.write_all(&plain).unwrap();
        let packed = deflate.finish().unwrap();
        let mut crc = flate2::Crc::new();
        crc.update(&plain);
        let honest = one_entry("d/z.txt", 8, &packed, plain.len() as u32, crc.sum());
        assert_eq!(zip(&honest, &t.join("ok"), Limits::DEFAULT), Ok(1 << 20));
        let bomb = one_entry("d/z.txt", 8, &packed, 10, crc.sum());
        assert!(zip(&bomb, &t.join("bomb"), Limits::DEFAULT).is_err());
        let written = std::fs::metadata(t.join("bomb").join("d").join("z.txt")).map(|m| m.len()).unwrap_or(0);
        assert!(written <= 10 + 8192, "wrote {written}");
        for (method, flags) in [(12u16, 0u16), (8, 1)] {
            let mut odd = one_entry("d/z.txt", method, &packed, plain.len() as u32, crc.sum());
            let cd = odd.windows(4).position(|w| w == 0x0201_4b50u32.to_le_bytes()).unwrap();
            odd[cd + 8..cd + 10].copy_from_slice(&flags.to_le_bytes());
            assert!(zip(&odd, &t.join(format!("odd{method}")), Limits::DEFAULT).is_err(), "{method} {flags}");
            assert!(!t.join(format!("odd{method}")).exists());
        }
        let _ = std::fs::remove_dir_all(&t);
    }
}
