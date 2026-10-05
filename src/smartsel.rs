//! iTerm2's Smart Selection: regular-expression rules that decide what a click selects,
//! each with a precision and optional actions.

/// One action on a rule, as iTerm2 stores it: `action` is its selector ("openFile:",
/// "openUrl:", "runCommand:", "runCoprocess:", "sendText:", "runCommandInWindow:",
/// "copy:").
#[derive(Clone, Debug, PartialEq)]
pub struct RuleAction {
    pub title: String,
    pub action: String,
    pub parameter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub regex: String,
    pub precision: f64,
    pub notes: String,
    pub actions: Vec<RuleAction>,
}

/// iTerm2's precision names and their score multipliers.
pub const PRECISIONS: [(&str, f64); 5] =
    [("very_low", 0.00001), ("low", 0.001), ("normal", 1.0), ("high", 1000.0), ("very_high", 1_000_000.0)];

pub fn precision_of(name: &str) -> f64 {
    PRECISIONS.iter().find(|(n, _)| *n == name).map_or(1.0, |(_, v)| *v)
}

pub fn precision_name(v: f64) -> &'static str {
    PRECISIONS.iter().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs())).map_or("normal", |(n, _)| n)
}

/// Rules from a profile's "Smart Selection Rules" list.
pub fn rules_from(value: &toml::Value) -> Vec<Rule> {
    let s = |t: &toml::Table, k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
        .map(|t| Rule {
            regex: s(t, "regex"),
            precision: precision_of(&s(t, "precision")),
            notes: s(t, "notes"),
            actions: t
                .get("actions")
                .and_then(|a| a.as_array())
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_table())
                .map(|a| RuleAction { title: s(a, "title"), action: s(a, "action"), parameter: s(a, "parameter") })
                .collect(),
        })
        .collect()
}

/// A profile's rules: its own list, else iTerm2's built-in list.
pub fn rules(profile: &crate::profile::Profile) -> Vec<Rule> {
    profile.get("Smart Selection Rules").map(|v| rules_from(&v)).unwrap_or_default()
}

/// iTerm2's rules are ICU regular expressions. This turns the ICU-only parts they use
/// into the dialect fancy-regex reads.
pub fn to_rust_regex(icu: &str) -> String {
    icu.replace("[:letter:]", "\\p{L}").replace("[:number:]", "\\p{N}")
}

/// A rule's match at a click.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub rule: usize,
    /// Byte range in the text.
    pub range: std::ops::Range<usize>,
    /// \0 is the whole match, then the groups ("" for a group that did not take part).
    pub captures: Vec<String>,
}

/// The best rule match that covers byte `click` of `text`: the highest precision times
/// length wins, and an earlier rule wins a tie. A rule that does not compile is skipped.
pub fn select_at(text: &str, click: usize, rules: &[Rule]) -> Option<Found> {
    let mut best: Option<(f64, Found)> = None;
    for (i, rule) in rules.iter().enumerate() {
        let Ok(re) = fancy_regex::Regex::new(&to_rust_regex(&rule.regex)) else { continue };
        for caps in re.captures_iter(text).flatten() {
            let Some(m) = caps.get(0) else { continue };
            if m.start() > click {
                break;
            }
            if m.end() <= click || m.as_str().is_empty() {
                continue;
            }
            let score = rule.precision * m.as_str().chars().count() as f64;
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                let captures = (0..caps.len()).map(|g| caps.get(g).map_or(String::new(), |c| c.as_str().to_string())).collect();
                best = Some((score, Found { rule: i, range: m.range(), captures }));
            }
        }
    }
    best.map(|(_, f)| f)
}

/// How a capture is written into a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    None,
    /// Quoted for cmd.exe.
    Cmd,
}

/// Fills in an action's parameter: \0 to \8 are the captures, \d the working
/// directory, \h the host, \u the user, \n a newline, and \\ a backslash.
pub fn expand(parameter: &str, captures: &[String], cwd: &str, host: &str, user: &str, escape: Escape) -> String {
    let quote = |t: &str| match escape {
        Escape::None => t.to_string(),
        Escape::Cmd => format!("\"{}\"", t.replace('"', "\"\"")),
    };
    let mut out = String::new();
    let mut chars = parameter.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some(d @ '0'..='8') => out.push_str(&quote(captures.get(d as usize - '0' as usize).map_or("", String::as_str))),
            Some('d') => out.push_str(&quote(cwd)),
            Some('h') => out.push_str(&quote(host)),
            Some('u') => out.push_str(&quote(user)),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            _ => {
                out.push('\\');
                continue;
            }
        }
        chars.next();
    }
    out
}

/// The actions that put a capture on a cmd.exe command line.
pub fn runs_command(action: &str) -> bool {
    matches!(action, "runCommand:" | "runCommandInWindow:" | "runCoprocess:")
}

/// A capture written for `action` (`CAP:139-156`): quoted for cmd.exe for the run actions
/// (iTerm2 backslash-escapes for sh), a URL's spaces and non-ASCII percent-encoded for Open
/// URL, others as they are.
pub fn encode_capture(action: &str, capture: &str) -> String {
    if runs_command(action) {
        return crate::semantic::cmd_quote(capture);
    }
    if action != "openUrl:" {
        return capture.to_string();
    }
    let mut out = String::new();
    for c in capture.chars() {
        if c.is_ascii() && !c.is_ascii_control() && c != ' ' {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// An action's parameter (`CAP:161-255`). Interpolated: Copy with no parameter copies the whole
/// match, else the parameter is evaluated with the captures (`matches`) encoded per action;
/// None when evaluation fails (nothing runs). Not interpolated: `expand`'s `\0`..`\8` form.
#[allow(clippy::too_many_arguments)]
pub fn parameter(
    a: &RuleAction,
    captures: &[String],
    interpolated: bool,
    eval: &dyn Fn(&str, &[Option<&str>]) -> Result<String, String>,
    cwd: &str,
    host: &str,
    user: &str,
) -> Option<String> {
    if a.action == "copy:" && a.parameter.is_empty() {
        return Some(captures.first().cloned().unwrap_or_default());
    }
    if !interpolated {
        let escape = if runs_command(&a.action) { Escape::Cmd } else { Escape::None };
        return Some(expand(&a.parameter, captures, cwd, host, user, escape));
    }
    let encoded: Vec<String> = captures.iter().map(|c| encode_capture(&a.action, c)).collect();
    let matches: Vec<Option<&str>> = encoded.iter().map(|c| Some(c.as_str())).collect();
    eval(&a.parameter, &matches).ok()
}

/// Whether any value `parameter` substitutes is unsafe for cmd.exe (B3): `%`, a quote or a
/// control character, which `Escape::Cmd` quoting does not neutralize.
pub fn cmd_unsafe_values(parameter: &str, captures: &[String], cwd: &str, host: &str, user: &str) -> bool {
    let mut chars = parameter.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            continue;
        }
        let value = match chars.peek().copied() {
            Some(d @ '0'..='8') => captures.get(d as usize - '0' as usize).map_or("", String::as_str),
            Some('d') => cwd,
            Some('h') => host,
            Some('u') => user,
            Some('\\') | Some('n') => {
                chars.next();
                continue;
            }
            _ => continue,
        };
        if crate::semantic::cmd_unsafe(value) {
            return true;
        }
        chars.next();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolated_actions_follow_iterm2() {
        let act = |action: &str, parameter: &str| RuleAction { title: "t".into(), action: action.into(), parameter: parameter.into() };
        let caps = vec!["x.y".to_string(), "x".to_string()];
        // The evaluator reads `matches[n]` as the interpolation does: a template with `\(matches[1])`.
        let eval = |t: &str, m: &[Option<&str>]| -> Result<String, String> {
            if t == "boom" {
                return Err("bad".into());
            }
            Ok(t.replace("\\(matches[1])", m.get(1).copied().flatten().unwrap_or("")).replace("\\(matches[0])", m.first().copied().flatten().unwrap_or("")))
        };
        // Off: expand's answers.
        let old = act("runCommand:", "open \\1");
        assert_eq!(parameter(&old, &caps, false, &eval, "", "", ""), Some("open \"x\"".into()));
        let old = act("sendText:", "open \\1");
        assert_eq!(parameter(&old, &caps, false, &eval, "", "", ""), Some("open x".into()));
        // On: matches encoded per action.
        let run = act("runCommand:", "open \\(matches[1])");
        assert_eq!(parameter(&run, &caps, true, &eval, "", "", ""), Some("open \"x\"".into()));
        let window = act("runCommandInWindow:", "open \\(matches[0])");
        assert_eq!(parameter(&window, &caps, true, &eval, "", "", ""), Some("open \"x.y\"".into()));
        let send = act("sendText:", "open \\(matches[0])");
        assert_eq!(parameter(&send, &caps, true, &eval, "", "", ""), Some("open x.y".into()));
        let url = act("openUrl:", "\\(matches[0])");
        assert_eq!(parameter(&url, &["a b\u{e9}".to_string()], true, &eval, "", "", ""), Some("a%20b%C3%A9".into()));
        // Copy with no parameter copies the whole match, in both modes.
        let copy = act("copy:", "");
        assert_eq!(parameter(&copy, &caps, true, &eval, "", "", ""), Some("x.y".into()));
        assert_eq!(parameter(&copy, &caps, false, &eval, "", "", ""), Some("x.y".into()));
        // An evaluation error: nothing runs.
        assert_eq!(parameter(&act("runCommand:", "boom"), &caps, true, &eval, "", "", ""), None);
    }

    #[test]
    fn captures_are_encoded_per_action() {
        assert_eq!(encode_capture("openUrl:", "a b\u{e9}"), "a%20b%C3%A9");
        assert_eq!(encode_capture("openUrl:", "https://a/b?c=d"), "https://a/b?c=d");
        assert_eq!(encode_capture("runCommand:", "a b"), "\"a b\"");
        assert_eq!(encode_capture("runCommandInWindow:", "x"), "\"x\"");
        assert_eq!(encode_capture("runCoprocess:", "x"), "\"x\"");
        assert_eq!(encode_capture("sendText:", "a b"), "a b");
        assert_eq!(encode_capture("openFile:", "a b"), "a b");
        assert!(runs_command("runCommand:") && runs_command("runCommandInWindow:") && runs_command("runCoprocess:"));
        assert!(!runs_command("openUrl:") && !runs_command("sendText:"));
    }

    #[test]
    fn a_pre_quoted_capture_is_refused_when_it_holds_a_command() {
        let c = |t: &str| vec![t.to_string()];
        assert!(cmd_unsafe_values("echo \"\\0\"", &c("x&calc"), "", "", ""));
        assert!(cmd_unsafe_values("run \\0", &c("a\\"), "", "", ""), "a trailing backslash");
        assert!(cmd_unsafe_values("run \\0", &c("a\u{ff02}b"), "", "", ""));
        assert!(cmd_unsafe_values("run \\d", &c("x"), "C:\\Program Files (x86)", "", ""));
        assert!(!cmd_unsafe_values("run \\0", &c("a b.txt"), "C:\\w", "h", "u"));
    }

    #[test]
    fn cmd_unsafe_values_are_found() {
        let caps = vec!["a".to_string(), "%COMSPEC%".to_string()];
        assert!(!cmd_unsafe_values("run \\0 %PATH%", &caps, "", "", ""), "the template's own text stays");
        assert!(cmd_unsafe_values("run \\1", &caps, "", "", ""));
        assert!(cmd_unsafe_values("run \\d", &caps, "C:\\a\"b", "", ""));
        assert!(cmd_unsafe_values("run \\h", &caps, "", "ho\nst", ""));
        assert!(!cmd_unsafe_values("run \\\\1 \\9", &caps, "", "", ""), "an escaped backslash and an unset capture");
    }

    #[test]
    fn rules_parse_with_their_actions() {
        let v: toml::Value = toml::from_str::<toml::Table>(
            r#"r = [ { regex = "\\S+", precision = "very_high", notes = "n", actions = [ { title = "Open", action = "openUrl:", parameter = "https://x/\\0" } ] } ]"#,
        )
        .unwrap()["r"]
            .clone();
        let r = rules_from(&v);
        assert_eq!(r[0].precision, 1_000_000.0);
        assert_eq!(r[0].actions[0], RuleAction { title: "Open".into(), action: "openUrl:".into(), parameter: "https://x/\\0".into() });
        assert_eq!(precision_name(1000.0), "high");
    }

    fn stock_rules() -> Vec<Rule> {
        rules(&crate::profile::Profile::stock())
    }

    #[test]
    fn every_built_in_rule_compiles() {
        let r = stock_rules();
        assert!(r.len() >= 10);
        for rule in &r {
            fancy_regex::Regex::new(&to_rust_regex(&rule.regex)).unwrap_or_else(|e| panic!("{}: {e}", rule.notes));
        }
        assert_eq!(to_rust_regex("[[:letter:][:number:]._-]"), "[\\p{L}\\p{N}._-]");
    }

    #[test]
    fn precision_times_length_picks_the_url_over_the_word() {
        let r = stock_rules();
        let text = "see https://example.com/a_b now";
        let click = text.find("example").unwrap();
        let f = select_at(text, click, &r).unwrap();
        assert_eq!(&text[f.range.clone()], "https://example.com/a_b");
        assert_eq!(r[f.rule].notes, "HTTP URL");
        let text = "call foo::bar::baz() here";
        let f = select_at(text, text.find("bar").unwrap(), &r).unwrap();
        assert_eq!(&text[f.range], "foo::bar::baz");
        let text = "mail me at a.b@example.com today";
        let f = select_at(text, text.find("example").unwrap(), &r).unwrap();
        assert_eq!(&text[f.range], "a.b@example.com");
        // Only the plain word rule matches a lone word.
        let text = "one two";
        let f = select_at(text, 5, &r).unwrap();
        assert_eq!(&text[f.range], "two");
        assert_eq!(select_at("a  b", 1, &r), None);
    }

    #[test]
    fn a_rule_that_does_not_compile_is_skipped() {
        let bad = Rule { regex: "(".into(), precision: 1e6, notes: String::new(), actions: vec![] };
        let word = Rule { regex: "\\S+".into(), precision: 0.001, notes: String::new(), actions: vec![] };
        let f = select_at("abc", 1, &[bad, word]).unwrap();
        assert_eq!(f.rule, 1);
    }

    #[test]
    fn action_parameters_substitute() {
        let caps = vec!["a b".to_string(), "x\"y".to_string()];
        assert_eq!(expand("open \\0 in \\d on \\h as \\u\\n\\\\", &caps, "C:\\w", "box", "me", Escape::None), "open a b in C:\\w on box as me\n\\");
        assert_eq!(expand("code \\0 \\1 \\2", &caps, "", "", "", Escape::Cmd), "code \"a b\" \"x\"\"y\" \"\"");
    }

    #[test]
    fn folder_host_and_user_are_quoted_for_cmd() {
        let caps = vec!["m".to_string()];
        assert_eq!(expand("x \\d \\h \\u", &caps, "C:\\a&b", "h&calc", "u\"x", Escape::Cmd), "x \"C:\\a&b\" \"h&calc\" \"u\"\"x\"");
    }
}
