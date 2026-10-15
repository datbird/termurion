//! iTerm2's Smart Selection: regular-expression rules that decide what a click selects,
//! each with a precision and optional actions.

use crate::semantic::CmdPiece;

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

/// The actions the context menu offers for a selected text (`CMH:864-896`): for each rule that has
/// actions and whose regex matches somewhere in `text`, its first match's captures, in rule order.
/// A rule that does not compile is skipped. Returns (rule index, captures).
pub fn rules_matching_text(text: &str, rules: &[Rule]) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        if rule.actions.is_empty() {
            continue;
        }
        let Ok(re) = fancy_regex::Regex::new(&to_rust_regex(&rule.regex)) else { continue };
        if let Ok(Some(caps)) = re.captures(text) {
            out.push((i, (0..caps.len()).map(|g| caps.get(g).map_or(String::new(), |c| c.as_str().to_string())).collect()));
        }
    }
    out
}

/// Fills in an action's parameter: \0 to \8 are the captures, \d the working
/// directory, \h the host, \u the user, \n a newline, and \\ a backslash.
pub fn expand(parameter: &str, captures: &[String], cwd: &str, host: &str, user: &str) -> String {
    expand_pieces(parameter, captures, cwd, host, user)
        .into_iter()
        .map(|p| match p {
            CmdPiece::Text(t) | CmdPiece::Value(t) => t,
        })
        .collect()
}

/// `expand`'s parameter split into its text and the values it substitutes.
fn expand_pieces(parameter: &str, captures: &[String], cwd: &str, host: &str, user: &str) -> Vec<CmdPiece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let value = |out: &mut Vec<CmdPiece>, text: &mut String, v: &str| {
        out.push(CmdPiece::Text(std::mem::take(text)));
        out.push(CmdPiece::Value(v.to_string()));
    };
    let mut chars = parameter.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some(d @ '0'..='8') => value(&mut out, &mut text, captures.get(d as usize - '0' as usize).map_or("", String::as_str)),
            Some('d') => value(&mut out, &mut text, cwd),
            Some('h') => value(&mut out, &mut text, host),
            Some('u') => value(&mut out, &mut text, user),
            Some('n') => text.push('\n'),
            Some('\\') => text.push('\\'),
            _ => {
                text.push('\\');
                continue;
            }
        }
        chars.next();
    }
    out.push(CmdPiece::Text(text));
    out
}

/// The actions that put a capture on a cmd.exe command line.
pub fn runs_command(action: &str) -> bool {
    matches!(action, "runCommand:" | "runCommandInWindow:" | "runCoprocess:")
}

/// A capture written for `action` (`CAP:139-156`): a URL's spaces and non-ASCII
/// percent-encoded for Open URL, others as they are (the run actions' captures are values for
/// the one cmd.exe builder, which quotes them; iTerm2 backslash-escapes for sh).
pub fn encode_capture(action: &str, capture: &str) -> String {
    if action != "openUrl:" {
        return capture.to_string();
    }
    crate::semantic::encode_url_chars(capture)
}

/// An action's parameter (`CAP:161-255`). Interpolated: Copy with no parameter copies the whole
/// match, else the parameter is evaluated with the captures (`matches`) encoded per action, by
/// `eval(template, matches, cmd)`, where `cmd` says the result is a cmd.exe command line (the
/// evaluator then builds it with the one builder, `interp::evaluate_cmd`). Not interpolated:
/// `expand`'s `\0`..`\8` form, a run action's through `semantic::build_cmd_line`. Err is why
/// nothing runs: an evaluation error, or `CMD_REFUSED`.
#[allow(clippy::too_many_arguments)]
pub fn parameter(
    a: &RuleAction,
    captures: &[String],
    interpolated: bool,
    eval: &dyn Fn(&str, &[Option<&str>], bool) -> Result<String, String>,
    cwd: &str,
    host: &str,
    user: &str,
) -> Result<String, String> {
    if a.action == "copy:" && a.parameter.is_empty() {
        return Ok(captures.first().cloned().unwrap_or_default());
    }
    let cmd = runs_command(&a.action);
    if !interpolated {
        if cmd {
            return crate::semantic::build_cmd_line(&expand_pieces(&a.parameter, captures, cwd, host, user)).ok_or_else(|| crate::semantic::CMD_REFUSED.to_string());
        }
        if a.action == "sendText:" {
            let pieces = expand_pieces(&a.parameter, captures, cwd, host, user);
            let values: Vec<&str> = pieces.iter().filter_map(|p| if let CmdPiece::Value(v) = p { Some(v.as_str()) } else { None }).collect();
            if crate::semantic::send_unsafe(&values) {
                return Err(crate::semantic::SEND_REFUSED.to_string());
            }
        }
        return Ok(expand(&a.parameter, captures, cwd, host, user));
    }
    // Interpolated: the captures are the values the template can bring in.
    if a.action == "sendText:" && crate::semantic::send_unsafe(&captures.iter().map(String::as_str).collect::<Vec<_>>()) {
        return Err(crate::semantic::SEND_REFUSED.to_string());
    }
    let encoded: Vec<String> = captures.iter().map(|c| encode_capture(&a.action, c)).collect();
    let matches: Vec<Option<&str>> = encoded.iter().map(|c| Some(c.as_str())).collect();
    eval(&a.parameter, &matches, cmd)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_send_text_capture_with_a_line_break_is_refused() {
        let act = |parameter: &str| RuleAction { title: "t".into(), action: "sendText:".into(), parameter: parameter.into() };
        let eval = |t: &str, m: &[Option<&str>], _: bool| crate::interp::evaluate(t, &|_| None, m);
        let clean = vec!["x".to_string(), "y".to_string()];
        let tainted = vec!["x\ny".to_string(), "y".to_string()];
        assert_eq!(parameter(&act("go \\1\\n"), &clean, false, &eval, "", "", ""), Ok("go y\n".into()), "the control: the template's own \\n stays");
        assert_eq!(parameter(&act("go \\1"), &tainted, false, &eval, "", "", ""), Ok("go y".into()), "a capture that is not used is not a value");
        assert_eq!(parameter(&act("go \\0"), &tainted, false, &eval, "", "", ""), Err(crate::semantic::SEND_REFUSED.into()));
        assert_eq!(parameter(&act("go \\(matches[1])"), &tainted, true, &eval, "", "", ""), Err(crate::semantic::SEND_REFUSED.into()));
    }

    #[test]
    fn an_open_url_capture_is_encoded_for_quotes_and_controls() {
        assert_eq!(encode_capture("openUrl:", "mailto:a@b?x=\" /a"), "mailto:a@b?x=%22%20/a");
        assert_eq!(encode_capture("openUrl:", "https://a/x\ry"), "https://a/x%0Dy");
    }

    use super::*;

    #[test]
    fn interpolated_actions_follow_iterm2() {
        let act = |action: &str, parameter: &str| RuleAction { title: "t".into(), action: action.into(), parameter: parameter.into() };
        let caps = vec!["x.y".to_string(), "x".to_string()];
        // The interpolation itself; a command line through the one builder.
        let eval = |t: &str, m: &[Option<&str>], cmd: bool| if cmd { crate::interp::evaluate_cmd(t, &|_| None, m, true) } else { crate::interp::evaluate(t, &|_| None, m) };
        // Off: expand's answers.
        let old = act("runCommand:", "open \\1");
        assert_eq!(parameter(&old, &caps, false, &eval, "", "", ""), Ok("open \"x\"".into()));
        let old = act("sendText:", "open \\1");
        assert_eq!(parameter(&old, &caps, false, &eval, "", "", ""), Ok("open x".into()));
        // On: matches encoded per action.
        let run = act("runCommand:", "open \\(matches[1])");
        assert_eq!(parameter(&run, &caps, true, &eval, "", "", ""), Ok("open \"x\"".into()));
        let window = act("runCommandInWindow:", "open \\(matches[0])");
        assert_eq!(parameter(&window, &caps, true, &eval, "", "", ""), Ok("open \"x.y\"".into()));
        let send = act("sendText:", "open \\(matches[0])");
        assert_eq!(parameter(&send, &caps, true, &eval, "", "", ""), Ok("open x.y".into()));
        let url = act("openUrl:", "\\(matches[0])");
        assert_eq!(parameter(&url, &["a b\u{e9}".to_string()], true, &eval, "", "", ""), Ok("a%20b%C3%A9".into()));
        // Copy with no parameter copies the whole match, in both modes.
        let copy = act("copy:", "");
        assert_eq!(parameter(&copy, &caps, true, &eval, "", "", ""), Ok("x.y".into()));
        assert_eq!(parameter(&copy, &caps, false, &eval, "", "", ""), Ok("x.y".into()));
        // An evaluation error: nothing runs.
        assert!(parameter(&act("runCommand:", "\\(f())"), &caps, true, &eval, "", "", "").is_err());
        // A refused value: nothing runs, in both modes.
        let amp = vec!["x&calc".to_string(), "x&calc".to_string()];
        assert_eq!(parameter(&act("runCommand:", "open \\(matches[1])"), &amp, true, &eval, "", "", ""), Err(crate::semantic::CMD_REFUSED.into()));
        assert_eq!(parameter(&act("runCommand:", "open \\1"), &amp, false, &eval, "", "", ""), Err(crate::semantic::CMD_REFUSED.into()));
    }

    #[test]
    fn captures_are_encoded_per_action() {
        assert_eq!(encode_capture("openUrl:", "a b\u{e9}"), "a%20b%C3%A9");
        assert_eq!(encode_capture("openUrl:", "https://a/b?c=d"), "https://a/b?c=d");
        assert_eq!(encode_capture("runCommand:", "a b"), "a b", "the one cmd builder quotes it");
        assert_eq!(encode_capture("runCoprocess:", "x"), "x");
        assert_eq!(encode_capture("sendText:", "a b"), "a b");
        assert_eq!(encode_capture("openFile:", "a b"), "a b");
        assert!(runs_command("runCommand:") && runs_command("runCommandInWindow:") && runs_command("runCoprocess:"));
        assert!(!runs_command("openUrl:") && !runs_command("sendText:"));
    }

    #[test]
    fn a_pre_quoted_capture_is_refused_when_it_holds_a_command() {
        let run = |param: &str, caps: &[&str], cwd: &str, host: &str| {
            let a = RuleAction { title: "t".into(), action: "runCommand:".into(), parameter: param.into() };
            let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
            parameter(&a, &caps, false, &|_, _, _| Err("unused".into()), cwd, host, "")
        };
        let refused = Err(crate::semantic::CMD_REFUSED.to_string());
        assert_eq!(run("echo \"\\0\"", &["x&calc"], "", ""), refused);
        assert_eq!(run("run \\0", &["a\\"], "", ""), refused, "a trailing backslash");
        assert_eq!(run("run \\0", &["a\u{ff02}b"], "", ""), refused);
        assert_eq!(run("run \\d", &["x"], "C:\\Program Files (x86)", ""), refused);
        assert_eq!(run("run \\h", &["x"], "", "ho\nst"), refused);
        assert_eq!(run("run \\0 %PATH%", &["a", "%COMSPEC%"], "", ""), Ok("run \"a\" %PATH%".into()), "the template's own text stays; an unused capture does not matter");
        assert_eq!(run("run \\\\1 \\9", &["a", "%COMSPEC%"], "", ""), Ok("run \\1 \\9".into()), "an escaped backslash and an unset capture");
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
        assert_eq!(expand("open \\0 in \\d on \\h as \\u\\n\\\\", &caps, "C:\\w", "box", "me"), "open a b in C:\\w on box as me\n\\");
    }

    /// A run action's pre-quoted `\0` keeps a value with spaces one argument (no doubled quotes).
    #[test]
    fn a_pre_quoted_capture_stays_one_argument() {
        let a = RuleAction { title: "t".into(), action: "runCommand:".into(), parameter: "git log \"\\1\" \\d".into() };
        let caps = vec!["m".to_string(), "a --flag".to_string()];
        let eval = |_: &str, _: &[Option<&str>], _: bool| -> Result<String, String> { Err("unused".into()) };
        let line = parameter(&a, &caps, false, &eval, "C:\\w d", "", "").unwrap();
        assert_eq!(line, "git log \"a --flag\" \"C:\\w d\"");
        assert_eq!(crate::semantic::split_command_line(&line), ["git", "log", "a --flag", "C:\\w d"]);
    }

    #[test]
    fn folder_host_and_user_are_quoted_for_cmd() {
        let a = RuleAction { title: "t".into(), action: "runCoprocess:".into(), parameter: "x \\d \\h \\u".into() };
        let caps = vec!["m".to_string()];
        let eval = |_: &str, _: &[Option<&str>], _: bool| -> Result<String, String> { Err("unused".into()) };
        assert_eq!(parameter(&a, &caps, false, &eval, "C:\\a b", "h", "u"), Ok("x \"C:\\a b\" \"h\" \"u\"".into()));
        assert_eq!(parameter(&a, &caps, false, &eval, "C:\\a&b", "h", "u"), Err(crate::semantic::CMD_REFUSED.into()));
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;

    fn rule(regex: &str, actions: usize) -> Rule {
        Rule { regex: regex.into(), precision: 1.0, notes: String::new(), actions: (0..actions).map(|i| RuleAction { title: format!("a{i}"), action: "copy:".into(), parameter: "\\0".into() }).collect() }
    }

    /// `CMH:864-896`: every rule with actions whose regex matches somewhere in the selection gives its
    /// first match's captures; a rule with no actions, no match or a bad regex gives nothing.
    #[test]
    fn rules_match_the_selected_text() {
        let rules = [rule(r"(\w+)@(\w+)\.com", 1), rule(r"zzz", 2), rule(r"\d+", 0), rule(r"[", 1), rule(r"\d+", 1)];
        let found = rules_matching_text("write to me@host.com or 42 or 7", &rules);
        assert_eq!(found, vec![(0, vec!["me@host.com".to_string(), "me".to_string(), "host".to_string()]), (4, vec!["42".to_string()])]);
        assert!(rules_matching_text("", &rules).is_empty());
    }
}
