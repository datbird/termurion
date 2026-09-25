//! iTerm2's interpolated strings, the part triggers and badges use: `\(path)` with dotted
//! variable names, `matches[n]`, quoted strings, and `a ?? b`. Function calls are refused.

/// The template split into text and the expressions inside `\( )`; an error names the
/// column (1-based) of an unclosed `\(`.
fn pieces(template: &str) -> Result<Vec<(bool, String)>, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                '(' => {
                    let start = i;
                    let mut depth = 1;
                    let mut j = i + 2;
                    let mut quoted = false;
                    while j < chars.len() && depth > 0 {
                        match chars[j] {
                            '\\' if quoted => j += 1,
                            '"' => quoted = !quoted,
                            '(' if !quoted => depth += 1,
                            ')' if !quoted => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    if depth > 0 {
                        return Err(format!("Unclosed \\( at column {}.", start + 1));
                    }
                    out.push((false, std::mem::take(&mut text)));
                    out.push((true, chars[i + 2..j - 1].iter().collect()));
                    i = j;
                    continue;
                }
                'n' => text.push('\n'),
                't' => text.push('\t'),
                'r' => text.push('\r'),
                'e' => text.push('\x1b'),
                'a' => text.push('\x07'),
                '\\' => text.push('\\'),
                other => {
                    text.push('\\');
                    text.push(other);
                }
            }
            i += 2;
            continue;
        }
        text.push(c);
        i += 1;
    }
    out.push((false, text));
    Ok(out)
}

/// A name followed by `(` outside a string.
fn calls_in(expr: &str) -> bool {
    let mut quoted = false;
    let mut prev_ident = false;
    let mut escape = false;
    for c in expr.chars() {
        if quoted {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                prev_ident = false;
            }
            '(' if prev_ident => return true,
            c if c.is_alphanumeric() || c == '_' || c == '.' => prev_ident = true,
            c if c.is_whitespace() => {}
            _ => prev_ident = false,
        }
    }
    false
}

/// True when an expression inside the template calls a function (SetBadgeFormat refuses).
pub fn has_calls(template: &str) -> bool {
    pieces(template).is_ok_and(|p| p.iter().any(|(expr, s)| *expr && calls_in(s)))
}

fn term(t: &str, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>]) -> Result<Option<String>, String> {
    let t = t.trim();
    if let Some(body) = t.strip_prefix('"').and_then(|b| b.strip_suffix('"')) {
        return Ok(Some(body.replace("\\\"", "\"")));
    }
    if let Some(n) = t.strip_prefix("matches[").and_then(|r| r.strip_suffix(']')) {
        let n: usize = n.trim().parse().map_err(|_| format!("Bad index in {t}."))?;
        return Ok(matches.get(n).copied().flatten().map(str::to_string));
    }
    let name_ok = !t.is_empty() && t.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_'));
    if !name_ok {
        return Err(format!("Tandem cannot evaluate \"{t}\": use a variable name, matches[n], a quoted string, or ??."));
    }
    Ok(lookup(t).or_else(|| (!t.contains('.')).then(|| lookup(&format!("session.{t}"))).flatten()))
}

/// Evaluates an interpolated string. Unknown variables are empty.
pub fn evaluate(template: &str, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>]) -> Result<String, String> {
    let mut out = String::new();
    for (is_expr, s) in pieces(template)? {
        if !is_expr {
            out.push_str(&s);
            continue;
        }
        if calls_in(&s) {
            return Err("Function calls are not allowed here.".into());
        }
        let mut value = None;
        for alt in split_fallback(&s) {
            match term(&alt, lookup, matches)? {
                Some(v) if !v.is_empty() => {
                    value = Some(v);
                    break;
                }
                _ => {}
            }
        }
        out.push_str(&value.unwrap_or_default());
    }
    Ok(out)
}

/// `a ?? b ?? c` into its parts (not inside strings).
fn split_fallback(expr: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            quoted = !quoted;
        }
        if !quoted && c == '?' && chars.get(i + 1) == Some(&'?') {
            parts.push(std::mem::take(&mut cur));
            i += 2;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    parts.push(cur);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str) -> Option<String> {
        match name {
            "session.hostname" => Some("box".into()),
            "user.x" => Some("42".into()),
            _ => None,
        }
    }

    #[test]
    fn paths_matches_and_literals() {
        let m = [Some("all"), Some("one")];
        assert_eq!(evaluate(r#"host \(session.hostname) x=\(user.x) m=\(matches[1]) \("lit")\n"#, &vars, &m).unwrap(), "host box x=42 m=one lit\n");
        assert_eq!(evaluate(r"\(hostname)", &vars, &m).unwrap(), "box", "a bare name is a session variable");
        assert_eq!(evaluate(r"[\(nope.var)][\(matches[7])]", &vars, &m).unwrap(), "[][]", "unknown is empty");
        assert_eq!(evaluate(r"a\\(b)", &vars, &m).unwrap(), r"a\(b)", "an escaped backslash");
        assert_eq!(evaluate(r#"\("a)b")"#, &vars, &m).unwrap(), "a)b", "a parenthesis inside quotes");
    }

    #[test]
    fn interpolation_refuses_calls_and_bad_syntax() {
        let e = evaluate(r"\(f(x))", &vars, &[]).unwrap_err();
        assert!(e.contains("Function calls"), "{e}");
        let e = evaluate(r"a \(session.hostname", &vars, &[]).unwrap_err();
        assert!(e.contains("column 3"), "{e}");
        assert!(evaluate(r"\(1 + 2)", &vars, &[]).is_err(), "only names, matches, strings, and ??");
        assert!(has_calls(r"x \(f(1))"));
        assert!(has_calls(r#"\(user.x ?? g())"#));
        assert!(!has_calls(r"\(user.x)"));
        assert!(!has_calls("f(x)"), "outside \\( ) is text");
        assert!(!has_calls(r#"\("f(x)")"#), "inside a string is text");
    }

    #[test]
    fn fallback_operator() {
        assert_eq!(evaluate(r#"\(user.missing ?? "none")"#, &vars, &[]).unwrap(), "none");
        assert_eq!(evaluate(r#"\(user.x ?? "none")"#, &vars, &[]).unwrap(), "42");
        assert_eq!(evaluate(r"\(user.missing ?? hostname)", &vars, &[]).unwrap(), "box");
    }
}
