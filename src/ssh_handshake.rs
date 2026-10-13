//! iTerm2's SSH integration handshake, as the terminal sees it: OSC 1337 `it2ssh=` (the
//! flags), `SendConductor=` (the version check and its banners) and the conductor bootstrap
//! typed into the session (`TD:3955-4004`, `PS:20283-20301`). `conductor.sh` ships unmodified
//! (ruling 2), pinned by its SHA-256.

use std::collections::BTreeMap;

use base64::Engine;

/// iTerm2's `conductor.sh` (`SI:ssh-helpers/conductor.sh` at `bbf5a33`), byte for byte.
pub const CONDUCTOR_SH: &str = include_str!("../resources/ssh/conductor.sh");
/// The SHA-256 of `CONDUCTOR_SH`, lowercase hex.
pub const CONDUCTOR_SHA256: &str = "bf04e9a90460d9e2de13043d60cc7bea4d797a42a7e6c358c25e3c78671cd5d5";

/// `SendConductor` with a version below 3 or none (iTerm2's text).
pub const OUT_OF_DATE: &str = "Out-of-date version of it2ssh detected. Please upgrade it2ssh.";
/// `SendConductor` with a version above 3.
pub const FUTURE: &str = "Future version of it2ssh detected. Please upgrade Termurion.";
/// The token sent when it2ssh gave an empty one: base64 of "none".
pub const NO_TOKEN: &str = "bm9uZQ==";

/// The four `it2ssh=` flags a version 3 `SendConductor` hands to the bootstrap. The token is
/// a secret: never log or show it (`Debug` gives only its length).
#[derive(Clone, PartialEq, Eq)]
pub struct Handshake {
    pub token: String,
    pub unique_id: String,
    pub encoded_ba: String,
    pub ssh_args: String,
}

impl std::fmt::Debug for Handshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handshake")
            .field("token_len", &self.token.len())
            .field("unique_id", &self.unique_id)
            .field("encoded_ba", &self.encoded_ba)
            .field("ssh_args", &self.ssh_args)
            .finish()
    }
}

/// What a `SendConductor=` sequence does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendConductor {
    /// No `it2ssh=` came first.
    Ignore,
    /// A version other than 3: this banner, and the flags are gone.
    Banner(&'static str),
    /// Version 3: type the conductor.
    Send(Handshake),
}

/// `it2ssh=`'s value split on each single space; fewer than four parts is ignored
/// (`TD:3955-3970`).
pub fn parse_it2ssh(value: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = value.split(' ').map(str::to_string).collect();
    (parts.len() >= 4).then_some(parts)
}

/// `SendConductor=`'s outcome with the flags `it2ssh=` left (`TD:3972-4004`). The caller has
/// already taken the flags: every outcome but `Ignore` clears them in iTerm2 too, and with no
/// flags there is nothing to clear.
pub fn send_conductor(value: &str, flags: Option<Vec<String>>) -> SendConductor {
    let Some(flags) = flags else { return SendConductor::Ignore };
    let v = key_values(value, ';').get("v").map(|v| integer_value(v));
    match v {
        None => SendConductor::Banner(OUT_OF_DATE),
        Some(v) if v < 3 => SendConductor::Banner(OUT_OF_DATE),
        Some(v) if v > 3 => SendConductor::Banner(FUTURE),
        _ => match flags.as_slice() {
            [token, unique_id, encoded_ba, ssh_args, ..] => SendConductor::Send(Handshake {
                token: token.clone(),
                unique_id: unique_id.clone(),
                encoded_ba: encoded_ba.clone(),
                ssh_args: ssh_args.clone(),
            }),
            _ => SendConductor::Ignore,
        },
    }
}

/// iTerm2's `it_keyValuePairsSeparatedBy:` (`NS:2668-2679`): each item split at its first
/// `=`; items without one are dropped; a later key wins.
pub fn key_values(text: &str, sep: char) -> BTreeMap<String, String> {
    text.split(sep).filter_map(|item| item.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// `NSString.integerValue`: leading whitespace skipped, an optional sign, the leading digits;
/// no digits is 0. Out of range saturates.
pub fn integer_value(s: &str) -> i64 {
    let s = s.trim_start();
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i64 = 0;
    for d in digits.bytes().take_while(u8::is_ascii_digit) {
        let d = (d - b'0') as i64;
        n = if negative { n.saturating_mul(10).saturating_sub(d) } else { n.saturating_mul(10).saturating_add(d) };
    }
    n
}

/// The bytes typed into the session to start the conductor (`PS:20283-20301`): `^C`, the
/// marker line, the base64 of the conductor and its `main` call in 80-character lines, and a
/// line holding only ESC.
pub fn bootstrap(conductor: &str, h: &Handshake) -> Vec<u8> {
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    // No empty lines: the remote read loop ends at one. A single pass, as iTerm2's.
    let script = conductor.replace("\n\n", "\n \n");
    let token = if h.token.is_empty() { NO_TOKEN.to_string() } else { b64(&h.token) };
    let message = format!("{script}main {token} {} {} {}", b64(&h.unique_id), b64(&h.encoded_ba), b64(&h.ssh_args));
    let encoded = b64(&message);
    // Base64 is ASCII, so byte chunks are character chunks.
    let lines: Vec<&str> = encoded.as_bytes().chunks(80).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    let mut out = b"\x03\n-- BEGIN CONDUCTOR --\n".to_vec();
    out.extend(lines.join("\n").into_bytes());
    out.extend(b"\n\x1b\n");
    out
}

/// The banner after EndSSH ended `count` hops (`SM:7473-7484`), naming where the session now
/// is. More than one hop with no location ends in two periods, as iTerm2's. The location comes
/// from the remote's ssh args, so it is cut to `conductor::MAX_SHOWN` (`conductor::shown`).
pub fn end_banner(count: usize, location: Option<&str>) -> String {
    let preamble = if count == 1 { "ssh exited".to_string() } else { format!("{count} ssh sessions ended.") };
    match location {
        Some(at) => format!("{preamble} \u{2014} now at {}.", crate::conductor::shown(at)),
        None => format!("{preamble}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(bytes: &[u8]) -> String {
        let text = std::str::from_utf8(bytes).unwrap();
        let body = text.strip_prefix("\x03\n-- BEGIN CONDUCTOR --\n").unwrap().strip_suffix("\n\x1b\n").unwrap();
        let joined: String = body.split('\n').collect();
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(joined).unwrap()).unwrap()
    }

    fn h(token: &str, unique_id: &str, encoded_ba: &str, ssh_args: &str) -> Handshake {
        Handshake { token: token.into(), unique_id: unique_id.into(), encoded_ba: encoded_ba.into(), ssh_args: ssh_args.into() }
    }

    #[test]
    fn conductor_sh_is_iterm2s() {
        use sha2::{Digest, Sha256};
        let sum: String = Sha256::digest(CONDUCTOR_SH.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sum, CONDUCTOR_SHA256);
        assert_eq!(CONDUCTOR_SH.len(), 12784);
        assert_eq!(CONDUCTOR_SH.matches('\n').count(), 505);
        assert!(!CONDUCTOR_SH.contains('\r'));
    }

    #[test]
    fn it2ssh_flags_follow_iterm2() {
        let v = |s: &[&str]| Some(s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(parse_it2ssh("none 1234512345 NDZB LXAg"), v(&["none", "1234512345", "NDZB", "LXAg"]));
        assert_eq!(parse_it2ssh("a b c"), None);
        assert_eq!(parse_it2ssh("a  b c d"), v(&["a", "", "b", "c", "d"]));
        assert_eq!(parse_it2ssh(""), None);
    }

    #[test]
    fn send_conductor_follows_iterm2() {
        let p = || Some(vec!["t".to_string(), "u".into(), "b".into(), "s".into()]);
        let send = SendConductor::Send(h("t", "u", "b", "s"));
        assert_eq!(send_conductor("v=3", None), SendConductor::Ignore);
        assert_eq!(send_conductor("v=3", p()), send);
        assert_eq!(send_conductor("v=2", p()), SendConductor::Banner(OUT_OF_DATE));
        assert_eq!(send_conductor("", p()), SendConductor::Banner(OUT_OF_DATE));
        assert_eq!(send_conductor("v", p()), SendConductor::Banner(OUT_OF_DATE));
        assert_eq!(send_conductor("v=4", p()), SendConductor::Banner(FUTURE));
        for value in ["x=1;v=3", "v=3abc", "v= 3", "v=03"] {
            assert_eq!(send_conductor(value, p()), send, "{value}");
        }
        assert_eq!(integer_value("-2x"), -2);
        assert_eq!(integer_value("x"), 0);
        let kv = key_values("a=1;b;a=2;c=x=y", ';');
        assert_eq!(kv.get("a").map(String::as_str), Some("2"), "a later key wins");
        assert_eq!(kv.get("c").map(String::as_str), Some("x=y"), "split at the first =");
        assert!(!kv.contains_key("b"), "an item without = is dropped");
    }

    #[test]
    fn bootstrap_follows_iterm2() {
        assert_eq!(bootstrap("echo hi\n", &h("none", "1234512345", "NDZBYQ==", "aG9zdCA=")), crate::ssh_probe::BOOTSTRAP);
        let bytes = bootstrap("echo hi\n", &h("", "1", "QQ==", "aA=="));
        assert_eq!(decoded(&bytes), format!("echo hi\nmain {NO_TOKEN} MQ== UVE9PQ== YUE9PQ=="));
        assert!(decoded(&bootstrap("x\n\n\ny\n", &h("", "1", "QQ==", "aA=="))).starts_with("x\n \n\ny\nmain "));
        let bytes = bootstrap(CONDUCTOR_SH, &h("none", "1234512345", "NDZBYQ==", "aG9zdCA="));
        assert!(bytes.starts_with(b"\x03\n-- BEGIN CONDUCTOR --\n"));
        assert!(bytes.ends_with(b"\n\x1b\n"));
        let text = std::str::from_utf8(&bytes).unwrap();
        let body = text.strip_prefix("\x03\n-- BEGIN CONDUCTOR --\n").unwrap().strip_suffix("\n\x1b\n").unwrap();
        let lines: Vec<&str> = body.split('\n').collect();
        assert!(lines.len() > 2);
        for line in &lines[..lines.len() - 1] {
            assert_eq!(line.len(), 80);
        }
        assert!(!lines[lines.len() - 1].is_empty() && lines[lines.len() - 1].len() <= 80);
    }

    #[test]
    fn handshake_debug_hides_the_token() {
        let text = format!("{:?}", h("s3cr3t-token", "1", "QQ==", "aA=="));
        assert!(!text.contains("s3cr3t"));
        assert!(text.contains("token_len: 12"));
    }

    #[test]
    fn end_banner_follows_iterm2() {
        assert_eq!(end_banner(1, None), "ssh exited.");
        assert_eq!(end_banner(1, Some("rob@web")), "ssh exited \u{2014} now at rob@web.");
        assert_eq!(end_banner(2, None), "2 ssh sessions ended..");
        assert_eq!(end_banner(3, Some("h")), "3 ssh sessions ended. \u{2014} now at h.");
    }

    /// The location comes from the remote's ssh args (up to the conductor parser's 1 MiB):
    /// it is cut to `conductor::MAX_SHOWN` characters like the other remote-sized texts.
    #[test]
    fn end_banner_cuts_a_remote_sized_location() {
        let long = "h".repeat(crate::conductor::MAX_SHOWN + 500);
        let cut = format!("{}\u{2026}", "h".repeat(crate::conductor::MAX_SHOWN));
        assert_eq!(end_banner(1, Some(&long)), format!("ssh exited \u{2014} now at {cut}."));
        assert_eq!(end_banner(2, Some(&long)), format!("2 ssh sessions ended. \u{2014} now at {cut}."));
        let fits = "h".repeat(crate::conductor::MAX_SHOWN);
        assert_eq!(end_banner(1, Some(&fits)), format!("ssh exited \u{2014} now at {fits}."));
    }
}
