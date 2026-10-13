//! iTerm2's interpolated strings (`EP:343-407`) on the expression language in `expr`, for
//! the callers that refuse function calls: triggers, badges, APS rules, coprocess
//! templates, Copy Interpolated String, and status bar strings without calls (those with
//! calls run through `script_eval`). Termurion keeps one extension of its own: `a ?? b` inside
//! `\( )`, the first alternative whose text is not empty.

use crate::expr::{self, Ctx, NoCalls, Outcome, Part, Value};

/// A scope over a string lookup and a trigger's captures: a dotless name also tries
/// `session.<name>`; `matches` is the array of captures (a group that did not match is "");
/// values are typed as the API types them (`api_vars::typed`), user variables stay text.
pub struct StrScope<'a> {
    pub lookup: &'a dyn Fn(&str) -> Option<String>,
    pub matches: &'a [Option<&'a str>],
}

impl expr::Scope for StrScope<'_> {
    fn value(&self, path: &str) -> Option<Value> {
        if path == "matches" {
            return (!self.matches.is_empty()).then(|| Value::Array(self.matches.iter().map(|m| Value::from(m.unwrap_or(""))).collect()));
        }
        let text = (self.lookup)(path).or_else(|| (!path.contains('.')).then(|| (self.lookup)(&format!("session.{path}"))).flatten())?;
        if path.starts_with("user.") || path.contains(".user.") {
            return Some(Value::String(text));
        }
        Some(crate::api_vars::typed(path.rsplit('.').next().unwrap_or(path), &text))
    }
}

/// A name followed by `(` outside a string: the call test for a body that does not parse.
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

/// True when a body's alternative calls a function.
fn alt_calls(alt: &str) -> bool {
    expr::parse_expression(alt).map_or_else(|_| calls_in(alt), |e| expr::contains_call(&e))
}

/// True when an expression inside the template calls a function (SetBadgeFormat refuses).
pub fn has_calls(template: &str) -> bool {
    expr::template_pieces(template).iter().any(|(is_expr, body)| *is_expr && split_fallback(body).iter().any(|a| alt_calls(a)))
}

enum Piece {
    Text(String),
    /// A body's alternatives (one unless it uses `??`).
    Alts(Vec<expr::Expr>),
}

/// The template parsed: a function call anywhere first, then the first syntax error. The
/// rest of an unclosed `\(` is text, as in iTerm2.
fn parse(template: &str) -> Result<Vec<Piece>, String> {
    let raw = expr::template_pieces(template);
    if raw.iter().any(|(is_expr, body)| *is_expr && split_fallback(body).iter().any(|a| alt_calls(a))) {
        return Err(expr::CALLS_NOT_ALLOWED.into());
    }
    let mut out = Vec::new();
    for (is_expr, text) in raw {
        if !is_expr {
            out.push(Piece::Text(text));
            continue;
        }
        let alts = split_fallback(&text).iter().map(|a| expr::parse_expression(a)).collect::<Result<Vec<_>, _>>()?;
        out.push(Piece::Alts(alts));
    }
    Ok(out)
}

/// Whether the template would evaluate: its syntax and no function call; an unclosed `\(`
/// is reported too, though evaluation reads its rest as text (the trigger editor warns).
pub fn check(template: &str) -> Result<(), String> {
    if let Some(column) = expr::unclosed_at(template) {
        return Err(format!("Unclosed \\( at column {column}."));
    }
    parse(template).map(|_| ())
}

fn text_of(parts: &[Part], scope: &StrScope, cx: &Ctx) -> Result<String, String> {
    match expr::eval_template(parts, scope, &mut NoCalls, cx) {
        Outcome::Done(v) => Ok(expr::to_text(&v)),
        Outcome::Failed(f) => Err(f.reason),
        Outcome::Pending => Err(expr::CALLS_NOT_ALLOWED.into()),
    }
}

/// An interpolated string that becomes a cmd.exe command line: the template's text stays as
/// written and each `\( )` is evaluated on its own into a value for the one builder
/// (`semantic::build_cmd_line`: refused by the context-free list, bare inside the template's
/// own quotes, quoted outside them). Values are what `lookup` and `matches` give, unquoted.
/// Err is the evaluation error, or `CMD_REFUSED`.
pub fn evaluate_cmd(template: &str, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>], lax: bool) -> Result<String, String> {
    use crate::semantic::CmdPiece;
    parse(template)?;
    let mut pieces = Vec::new();
    for (is_expr, text) in expr::template_pieces(template) {
        pieces.push(if is_expr { CmdPiece::Value(evaluate_lax(&format!("\\({text})"), lookup, matches, lax)?) } else { CmdPiece::Text(text) });
    }
    crate::semantic::build_cmd_line(&pieces).ok_or_else(|| crate::semantic::CMD_REFUSED.to_string())
}

/// Evaluates an interpolated string with the lax policy on (an undefined bare reference is
/// empty) and function calls refused.
pub fn evaluate(template: &str, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>]) -> Result<String, String> {
    evaluate_lax(template, lookup, matches, true)
}

/// Evaluates an interpolated string with function calls refused; `lax` is
/// `laxNilPolicyInInterpolatedStrings`. In `a ?? b` an alternative that is an undefined
/// reference counts as empty either way.
pub fn evaluate_lax(template: &str, lookup: &dyn Fn(&str) -> Option<String>, matches: &[Option<&str>], lax: bool) -> Result<String, String> {
    let pieces = parse(template)?;
    let scope = StrScope { lookup, matches };
    let cx = Ctx { invocation: template, lax };
    if pieces.iter().all(|p| !matches!(p, Piece::Alts(a) if a.len() > 1)) {
        let parts: Vec<Part> = pieces
            .into_iter()
            .map(|p| match p {
                Piece::Text(t) => Part::Text(t),
                Piece::Alts(mut a) => Part::Expr(a.remove(0)),
            })
            .collect();
        return text_of(&parts, &scope, &cx);
    }
    let fallback = Ctx { invocation: template, lax: true };
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => out.push_str(&t),
            Piece::Alts(alts) if alts.len() == 1 => out.push_str(&text_of(&[Part::Expr(alts[0].clone())], &scope, &cx)?),
            Piece::Alts(alts) => {
                for a in alts {
                    let t = text_of(&[Part::Expr(a)], &scope, &fallback)?;
                    if !t.is_empty() {
                        out.push_str(&t);
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
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

    /// A cmd.exe template: each `\( )` is a value for the one builder (bare inside the
    /// template's quotes, quoted outside, refused by the context-free list), the text stays.
    #[test]
    fn cmd_templates_use_the_one_builder() {
        let m = [Some("all"), Some("a --flag"), Some("x&calc")];
        let lookup = |n: &str| match n {
            "session.path" => Some("C:\\a b".to_string()),
            "user.x" => Some("a|b".to_string()),
            _ => None,
        };
        assert_eq!(evaluate_cmd(r#"git log "\(matches[1])" \(session.path)"#, &lookup, &m, true).as_deref(), Ok(r#"git log "a --flag" "C:\a b""#));
        assert_eq!(evaluate_cmd(r"run \(path ?? user.x)", &lookup, &m, true).as_deref(), Ok(r#"run "C:\a b""#), "a fallback is one value");
        for bad in [r#"run "\(matches[2])""#, r"run \(user.x)", r#"run \("a\"b")"#] {
            assert_eq!(evaluate_cmd(bad, &lookup, &m, true), Err(crate::semantic::CMD_REFUSED.to_string()), "{bad}");
        }
        assert_eq!(evaluate_cmd(r"run \(matches[1])", &lookup, &[Some(""), Some("ok"), Some("x&calc")], true).as_deref(), Ok(r#"run "ok""#), "an unused capture does not refuse");
        assert!(evaluate_cmd(r"run \(f())", &lookup, &m, true).is_err(), "calls are still refused");
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
        assert_eq!(evaluate(r"a \(session.hostname", &vars, &[]).unwrap(), r"a \(session.hostname", "no expression: the whole text (EP:400-405)");
        assert_eq!(evaluate(r"\(user.x) \(session.hostname", &vars, &[]).unwrap(), "42 session.hostname", "the rest of an unclosed \\( is text");
        assert!(check(r"a \(session.hostname").unwrap_err().contains("column 3"), "the trigger editor still warns");
        assert_eq!(evaluate(r"\(1 + 2)", &vars, &[]).unwrap(), "3", "the full expression language");
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

    #[test]
    fn lax_nil_follows_iterm2() {
        let undefined = crate::expr::undefined("nope");
        assert_eq!(evaluate_lax(r"a\(nope)b", &vars, &[], true).unwrap(), "ab");
        assert_eq!(evaluate_lax(r"a\(nope)b", &vars, &[], false).unwrap_err(), undefined);
        for lax in [true, false] {
            let e = evaluate_lax(r"\(nope + 1)", &vars, &[], lax).unwrap_err();
            assert_eq!(e, "Subexpression error: Nil operand in binary operation (+)", "not a bare reference");
        }
        assert_eq!(evaluate_lax(r"\(f(x: nope))", &vars, &[], true).unwrap_err(), crate::expr::CALLS_NOT_ALLOWED);
        assert_eq!(evaluate_lax(r#"\(user.missing ?? "none")"#, &vars, &[], false).unwrap(), "none", "?? still works");
    }

    #[test]
    fn interpolation_keeps_termurion_forms() {
        let m = [Some("all"), Some("one")];
        let e = evaluate_lax(r"\(matches[7])", &vars, &m, false).unwrap_err();
        assert_eq!(e, "Index 7 out of range of \u{201c}matches\u{201d}, which has 2 values", "the Correction, with lax off");
        assert_eq!(evaluate(r"\(matches[7])", &vars, &m).unwrap(), "", "lax: a bare indexed reference that fails is empty (EP:380-398)");
        let cols = |n: &str| (n == "session.columns").then(|| "80".to_string());
        assert_eq!(evaluate(r"\(columns + 1)", &cols, &[]).unwrap(), "81", "a numeric variable is a number");
        assert_eq!(evaluate(r#"\(user.x)\"q\" \(1 / 2)"#, &vars, &[]).unwrap(), "42\"q\" 0.5");
        assert!(check(r"\(f())").is_err() && check(r"\(1 +)").is_err() && check(r"\(user.x + 1)").is_ok());
    }
}
