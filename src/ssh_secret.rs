//! The origin secrets it2ssh fetches to prove a session started on this machine
//! (`SS:54-93`): 16 random bytes as lowercase hex, each accepted once.

use std::collections::VecDeque;

/// Random bytes in one secret (`SS:77`).
pub const SECRET_BYTES: usize = 16;
/// The most secrets remembered at once (iTerm2 keeps every one); past it the oldest goes.
pub const MAX_SECRETS: usize = 256;

/// The secrets handed out and not yet used.
#[derive(Default)]
pub struct Secrets {
    live: VecDeque<String>,
}

/// Only how many are live: a secret is never printed.
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets").field("live_len", &self.live.len()).finish()
    }
}

impl Secrets {
    /// A new secret from `bytes`, remembered until checked (`SS:62-73`).
    pub fn mint(&mut self, bytes: [u8; SECRET_BYTES]) -> String {
        let s = hex(&bytes);
        self.live.push_back(s.clone());
        while self.live.len() > MAX_SECRETS {
            self.live.pop_front();
        }
        s
    }

    /// Whether `s` is a remembered secret; a true answer forgets it (`SS:54-60`).
    pub fn check(&mut self, s: &str) -> bool {
        match self.live.iter().position(|x| x == s) {
            Some(i) => {
                self.live.remove(i);
                true
            }
            None => false,
        }
    }
}

/// Lowercase `%02x` of each byte (`ND:424-432`).
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `s` has the shape of a secret: exactly 32 characters of `0-9a-f`.
pub fn valid(s: &str) -> bool {
    s.len() == 2 * SECRET_BYTES && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_follow_iterm2() {
        let mut s = Secrets::default();
        assert!(s.mint([0; SECRET_BYTES]) == "0".repeat(32));
        let ab = s.mint([0xab; SECRET_BYTES]);
        assert!(ab == "ab".repeat(16));
        assert!(s.check(&ab), "a minted secret is accepted");
        assert!(!s.check(&ab), "and only once");
        assert!(!s.check("x"));
        let mut s = Secrets::default();
        let minted: Vec<String> = (0..=MAX_SECRETS as u16).map(|i| {
            let mut b = [0u8; SECRET_BYTES];
            b[..2].copy_from_slice(&i.to_be_bytes());
            s.mint(b)
        }).collect();
        assert!(!s.check(&minted[0]), "past MAX_SECRETS the oldest is forgotten");
        assert!(s.check(&minted[MAX_SECRETS]), "the newest is kept");
        assert!(!valid(&"AB".repeat(16)), "uppercase is not iTerm2's hex");
        assert!(!valid(&"a".repeat(31)));
        assert!(valid(&minted[1]));
        assert!(hex(&[0, 15, 255]) == "000fff");
    }

    #[test]
    fn secrets_debug_hides_the_secrets() {
        let mut s = Secrets::default();
        let ab = s.mint([0xab; SECRET_BYTES]);
        let text = format!("{s:?}");
        assert!(!text.contains(&ab) && !text.contains("abab"));
        assert!(text.contains("live_len: 1"));
    }
}
