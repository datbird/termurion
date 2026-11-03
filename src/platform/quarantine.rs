//! The "from the internet" mark on a downloaded file. Windows writes the Zone.Identifier stream, macOS
//! the quarantine attribute; Linux has no mark and reports Unsupported.

use std::path::Path;

#[cfg(unix)]
use super::unix::quarantine as imp;
#[cfg(windows)]
use super::windows::quarantine as imp;

/// Marks the existing file `path` with `text` (the zone file's contents). Err when it cannot.
pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    imp::mark(path, text)
}

/// The page a file came from: the value of the `HostUrl=` line of zone text (CRLF or LF).
pub fn host_url(zone_text: &str) -> Option<&str> {
    zone_text.lines().find_map(|l| l.trim_end_matches('\r').strip_prefix("HostUrl=")).filter(|u| !u.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_url_is_read() {
        assert_eq!(host_url("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=ssh://u@h/x\r\n"), Some("ssh://u@h/x"));
        assert_eq!(host_url("[ZoneTransfer]\nZoneId=3\nHostUrl=ssh://u@h/x\n"), Some("ssh://u@h/x"));
        assert_eq!(host_url("[ZoneTransfer]\r\nZoneId=3\r\n"), None);
    }
}
