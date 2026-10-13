//! iTerm2's framer.py, bundled unmodified, and the text that launches it (`CD:2044-2063`).

/// framer.py as iTerm2 ships it (`OtherResources/framer.py` at `64791fc`, byte for byte).
pub const FRAMER: &str = include_str!("../resources/ssh/framer.py");

/// The one line of framer.py that iTerm2 replaces at launch (`F:48`).
pub const SUB: &str = "#{SUB}";

/// The text `execFramer` sends (`CD:2044-2063`): `#{SUB}` becomes `DEPTH=<depth>`. iTerm2
/// adds `VERBOSE=1` only for debug logging; Termurion never does.
pub fn framer_code(depth: i32) -> String {
    FRAMER.replace(SUB, &format!("DEPTH={depth}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn framer_script_is_iterm2s() {
        let hex: String = Sha256::digest(FRAMER.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert!(hex.starts_with("1261e224b5100e5d"), "{hex}");
        assert_eq!(FRAMER.matches(SUB).count(), 1);
        assert_eq!(FRAMER.lines().nth(47), Some("#{SUB}"));
    }

    #[test]
    fn framer_code_follows_iterm2() {
        assert_eq!(framer_code(0), FRAMER.replace("#{SUB}", "DEPTH=0"));
        let code = framer_code(3);
        assert!(code.contains("\nDEPTH=3\n"));
        assert!(!code.contains("#{SUB}"));
        assert!(!code.contains("VERBOSE=1"));
        assert_eq!(code.len(), FRAMER.len() - 6 + "DEPTH=3".len());
    }
}
