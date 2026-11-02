//! Splits a command line into words the way a POSIX shell does, with no expansion. The Unix pty
//! backend runs a profile's command through this, never through a shell, so nothing in the line
//! is ever evaluated. OS-free so the Windows tests run it too.

/// Words split on blanks. `'...'` is literal; `"..."` honors `\"`, `\\`, `\$` and `` \` ``; a
/// backslash outside quotes keeps the next character. No variable, tilde or glob expansion.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    // A quoted empty string ("") is still a word.
    let mut live = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if live {
                    words.push(std::mem::take(&mut cur));
                    live = false;
                }
            }
            '\'' => {
                live = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(q) => cur.push(q),
                        None => return Err(format!("unterminated quote in {line}")),
                    }
                }
            }
            '"' => {
                live = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            // A line continuation inside double quotes is removed.
                            Some('\n') => {}
                            Some(e @ ('"' | '\\' | '$' | '`')) => cur.push(e),
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                            None => return Err(format!("unterminated quote in {line}")),
                        },
                        Some(q) => cur.push(q),
                        None => return Err(format!("unterminated quote in {line}")),
                    }
                }
            }
            '\\' => match chars.next() {
                // A line continuation is removed and starts no word.
                Some('\n') => {}
                Some(n) => {
                    live = true;
                    cur.push(n);
                }
                // A backslash at the very end is a literal backslash.
                None => {
                    live = true;
                    cur.push('\\');
                }
            },
            other => {
                live = true;
                cur.push(other);
            }
        }
    }
    if live {
        words.push(cur);
    }
    if words.is_empty() {
        return Err("empty command line".to_string());
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_follows_posix_quoting() {
        assert_eq!(split(r#"a 'b c' "d \"e\" \$x" f\ g"#), Ok(vec!["a".to_string(), "b c".into(), "d \"e\" $x".into(), "f g".into()]));
        assert_eq!(split("  "), Err("empty command line".to_string()));
        assert!(split("a 'b").unwrap_err().contains("unterminated quote"));
        assert!(split("a \"b").unwrap_err().contains("unterminated quote"));
    }

    #[test]
    fn a_trailing_backslash_is_literal_and_a_continuation_vanishes() {
        assert_eq!(split("a \\"), Ok(vec!["a".to_string(), "\\".into()]));
        assert_eq!(split("a\\"), Ok(vec!["a\\".to_string()]));
        assert_eq!(split("a \\\nb"), Ok(vec!["a".to_string(), "b".into()]));
        assert_eq!(split("a\\\nb c"), Ok(vec!["ab".to_string(), "c".into()]));
        assert_eq!(split("a \\\n"), Ok(vec!["a".to_string()]), "no extra empty word");
        assert_eq!(split("\"x\\\ny\""), Ok(vec!["xy".to_string()]));
        assert_eq!(split("'a\\\nb'"), Ok(vec!["a\\\nb".to_string()]), "single quotes keep it");
    }

    #[test]
    fn split_does_not_expand_and_keeps_empty_words() {
        assert_eq!(split("echo $HOME ~ *"), Ok(vec!["echo".to_string(), "$HOME".into(), "~".into(), "*".into()]));
        assert_eq!(split("a '' b"), Ok(vec!["a".to_string(), String::new(), "b".into()]));
        assert_eq!(split(r#"sh -c 'echo X'"#), Ok(vec!["sh".to_string(), "-c".into(), "echo X".into()]));
        assert_eq!(split(r"C:\Program"), Ok(vec!["C:Program".to_string()]), "a Windows path is not a path here");
    }
}
