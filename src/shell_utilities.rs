//! The utilities the Internet-Free install unpacks (`utilities.tgz` in iTerm2's bundle): the
//! scripts of `dist/utilities`, built into a tar.gz and sent as base64 lines.

macro_rules! utility {
    ($($name:literal),* $(,)?) => {
        /// (file name, bytes) of every utility, in `dist/utilities` listing order.
        pub static FILES: [(&str, &[u8]); utility!(@count $($name),*)] = [$(($name, include_bytes!(concat!("../dist/utilities/", $name)))),*];
    };
    (@count) => { 0 };
    (@count $head:literal $(, $rest:literal)*) => { 1 + utility!(@count $($rest),*) };
}

utility!("imgcat", "imgls", "it2api", "it2attention", "it2cat", "it2check", "it2copy", "it2dl", "it2getvar", "it2git", "it2profile", "it2setcolor", "it2setkeylabel", "it2tip", "it2ul", "it2universion");

/// The utilities as a gzip-compressed tar, each file executable.
pub fn tgz() -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, bytes) in FILES.iter() {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_cksum();
        let _ = tar.append_data(&mut header, name, *bytes);
    }
    let raw = tar.into_inner().unwrap_or_default();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = std::io::Write::write_all(&mut gz, &raw);
    gz.finish().unwrap_or_default()
}

/// `base64EncodedStringWithOptions:` with 76-character lines ended by a line feed.
pub fn base64_archive() -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(tgz());
    encoded.as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_unpacks_to_the_listed_files() {
        let raw = tgz();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut tar_bytes = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut tar_bytes).unwrap();
        let mut ar = tar::Archive::new(&tar_bytes[..]);
        let names: Vec<String> = ar.entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, FILES.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>());
        assert!(base64_archive().lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn the_list_is_the_distribution_folder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist").join("utilities");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        on_disk.sort();
        let mut listed: Vec<String> = FILES.iter().map(|(n, _)| n.to_string()).collect();
        listed.sort();
        assert_eq!(listed, on_disk);
    }
}
