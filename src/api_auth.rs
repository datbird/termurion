//! Authorization for the Python API. Task 2 holds only the connection key generator; the cookie
//! jar, the allow-all file and the alert texts join it later.

/// A fresh connection key for a client that sent no `x-iterm2-key`: an uppercase UUID, as
/// `[[NSUUID UUID] UUIDString]` gives (`W:171`).
pub fn fresh_key() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_key_is_an_uppercase_uuid() {
        let k = fresh_key();
        assert_eq!(k.len(), 36);
        for (i, c) in k.chars().enumerate() {
            if [8, 13, 18, 23].contains(&i) {
                assert_eq!(c, '-', "{k}");
            } else {
                assert!(c.is_ascii_digit() || ('A'..='F').contains(&c), "{k}");
            }
        }
        assert_eq!(&k[14..15], "4");
        assert_ne!(fresh_key(), k);
    }
}
