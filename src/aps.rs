//! Automatic Profile Switching, a port of iTerm2's `iTermRule` and
//! `iTermAutomaticProfileSwitcher`: rules in a profile's Bound Hosts, their scores, and the
//! stack of profiles a session goes back through.


/// The facts a session's rules are scored against (None: not known).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts<'a> {
    pub host: Option<&'a str>,
    pub user: Option<&'a str>,
    pub path: Option<&'a str>,
    pub job: Option<&'a str>,
    pub cmdline: Option<&'a str>,
}

/// One rule from a profile's Bound Hosts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub sticky: bool,
    pub expression: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    pub path: Option<String>,
    pub job: Option<String>,
}

/// iTerm2's glob: `*` matches anything (also nothing); anchored unless the pattern starts
/// or ends with `*`.
pub fn glob(s: &str, pattern: &str, case_sensitive: bool) -> bool {
    let (s, pattern) = if case_sensitive { (s.to_string(), pattern.to_string()) } else { (s.to_lowercase(), pattern.to_lowercase()) };
    let anchor_start = !pattern.starts_with('*');
    let mut start = 0;
    for part in pattern.split('*') {
        if part.is_empty() {
            continue;
        }
        let Some(at) = s[start..].find(part) else { return false };
        if anchor_start && start == 0 && at != 0 {
            return false;
        }
        start += at + part.len();
    }
    if pattern.ends_with('*') { true } else { start == s.len() }
}

fn squash(x: f64) -> f64 {
    x / (x + 1.0)
}

impl Rule {
    pub fn parse(text: &str) -> Rule {
        let mut rule = Rule::default();
        let mut s = text;
        if let Some(rest) = s.strip_prefix('!') {
            rule.sticky = true;
            s = rest;
        }
        if s.starts_with('{') && s.ends_with('}') && s.len() >= 2 {
            rule.expression = Some(s[1..s.len() - 1].to_string());
            return rule;
        }
        if let Some((before, job)) = s.split_once('&') {
            rule.job = Some(job.to_string());
            s = before;
        }
        // A path rule: a leading "/", or on Windows a drive ("C:\" or "C:/").
        let b = s.as_bytes();
        let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
        if s.starts_with('/') || drive {
            rule.path = Some(s.to_string());
            return rule;
        }
        let at = s.find('@');
        let mut colon = s.find(':');
        if let Some(at) = at {
            rule.user = Some(s[..at].to_string());
            match colon {
                // Malformed, like foo:bar@baz: iTerm2 keeps only the user.
                Some(c) if c < at => colon = None,
                Some(c) => rule.host = Some(s[at + 1..c].to_string()),
                None => rule.host = Some(s[at + 1..].to_string()).filter(|h| !h.is_empty()),
            }
            if colon.is_none() {
                return rule;
            }
        }
        match colon {
            Some(c) => {
                if rule.host.is_none() {
                    rule.host = Some(s[..c].to_string()).filter(|h| !h.is_empty());
                }
                rule.path = Some(s[c + 1..].to_string());
            }
            None if !s.is_empty() => rule.host = Some(s.to_string()),
            None => {}
        }
        rule
    }

    /// iTerm2's score: 0 when any part fails. `expr` scores an expression rule.
    pub fn score(&self, f: &Facts, expr: &dyn Fn(&str) -> f64) -> f64 {
        if let Some(e) = &self.expression {
            return expr(e);
        }
        const PATH_PARTIAL: f64 = 0.0;
        const CATCH_ALL: f64 = 1.0;
        const PATH_EXACT: f64 = 2.0;
        const USER_EXACT: f64 = 4.0;
        const JOB: f64 = 8.0;
        const HOST_PARTIAL: f64 = 16.0;
        const HOST_EXACT: f64 = 32.0;
        let mut score = 0.0;
        if let Some(j) = &self.job {
            let hit = |v: Option<&str>| v.is_some_and(|v| glob(v, j, true));
            if !hit(f.job) && !hit(f.cmdline) {
                return 0.0;
            }
            score += JOB;
        }
        if let Some(h) = &self.host {
            if !h.contains('*') && f.host == Some(h.as_str()) {
                score += HOST_EXACT;
            } else if h == "*" {
                if self.user.is_none() && self.path.is_none() && self.job.is_none() {
                    score += CATCH_ALL;
                }
            } else if f.host.is_some_and(|v| glob(v, h, false)) {
                score += HOST_PARTIAL * (1.0 + squash(h.chars().count() as f64));
            } else if !h.is_empty() {
                return 0.0;
            }
        }
        match &self.user {
            Some(u) if f.user == Some(u.as_str()) => score += USER_EXACT,
            Some(u) if !u.is_empty() => return 0.0,
            _ => {}
        }
        if let Some(rule_path) = &self.path {
            // Windows folders ignore case and take either slash; a Unix path keeps iTerm2's
            // exact comparison.
            let windows = |s: &str| s.contains('\\') || s.as_bytes().get(1) == Some(&b':');
            let fold = |s: &str| s.replace('/', "\\").to_lowercase();
            let win = windows(rule_path) || f.path.is_some_and(windows);
            let p = if win { fold(rule_path) } else { rule_path.clone() };
            let actual = f.path.map(|a| if win { fold(a) } else { a.to_string() });
            // A trailing separator lets "/foo/bar/*" match "/foo/bar" (on Windows, "\").
            let sep = if win { '\\' } else { '/' };
            let exact = actual.as_deref().is_some_and(|a| !p.contains('*') && a == p);
            let partial = actual.as_deref().is_some_and(|a| {
                let augmented = if a.ends_with(sep) { a.to_string() } else { format!("{a}{sep}") };
                glob(&augmented, &p, true) || glob(a, &p, true)
            });
            if exact {
                score += PATH_EXACT;
            } else if partial {
                score += PATH_PARTIAL + squash(p.chars().count() as f64);
            } else if !p.is_empty() {
                return 0.0;
            }
        }
        score
    }
}

/// A profile and its rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub guid: String,
    pub name: String,
    pub rules: Vec<String>,
}

/// A profile's best rule score.
pub fn profile_score(rules: &[String], f: &Facts, expr: &dyn Fn(&str) -> f64) -> f64 {
    rules.iter().map(|r| Rule::parse(r).score(f, expr)).fold(0.0, f64::max)
}

/// The best profile for the facts: (guid, score, sticky). A rule in two profiles belongs to
/// the last; the first of equal scores wins; a score must be above 0.
pub fn best(profiles: &[Candidate], f: &Facts, expr: &dyn Fn(&str) -> f64) -> Option<(String, f64, bool)> {
    let mut owners: Vec<(&str, &str)> = Vec::new();
    for p in profiles {
        for r in &p.rules {
            match owners.iter_mut().find(|(rule, _)| *rule == r.as_str()) {
                Some(o) => o.1 = &p.guid,
                None => owners.push((r, &p.guid)),
            }
        }
    }
    let mut out: Option<(String, f64, bool)> = None;
    let mut top = 0.0;
    for (text, guid) in owners {
        let rule = Rule::parse(text);
        let score = rule.score(f, expr);
        if score > top {
            top = score;
            out = Some((guid.to_string(), score, rule.sticky));
        }
    }
    out
}

/// One session's switcher: the stack of profiles it came from, and the last facts.
#[derive(Clone, Debug, Default)]
pub struct Switcher {
    stack: Vec<String>,
    last: Option<(Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, String)>,
}

impl Switcher {
    pub fn stack(&self) -> Vec<&str> {
        self.stack.iter().map(String::as_str).collect()
    }

    /// The profile loads again (Settings changed): the next facts are looked at even when
    /// they did not change.
    pub fn mark_dirty(&mut self) {
        self.last = None;
    }

    fn rules_of<'a>(profiles: &'a [Candidate], guid: &str) -> &'a [String] {
        profiles.iter().find(|p| p.guid == guid).map_or(&[], |p| p.rules.as_slice())
    }

    /// The profile to switch to, if any, for a session now on `current`.
    pub fn decide(&mut self, current: &str, f: &Facts, profiles: &[Candidate], expr: &dyn Fn(&str) -> f64) -> Option<String> {
        let own = |v: Option<&str>| v.map(str::to_string);
        let key = (own(f.host), own(f.user), own(f.path), own(f.job), own(f.cmdline), current.to_string());
        if self.last.as_ref() == Some(&key) {
            return None;
        }
        self.last = Some(key);
        let current_score = profile_score(Self::rules_of(profiles, current), f, expr);
        let topmost = self.stack.iter().rev().find_map(|g| {
            let s = profile_score(Self::rules_of(profiles, g), f, expr);
            (s > 0.0).then(|| (g.clone(), s))
        });
        let switched = if let Some((g, _)) = topmost.filter(|(g, s)| g != current && *s > current_score) {
            while self.stack.last() == Some(&g) {
                self.stack.pop();
            }
            Some(g)
        } else {
            match best(profiles, f, expr) {
                Some((g, _, sticky)) if g != current => {
                    if self.stack.last().map(String::as_str) != Some(current) {
                        self.stack.push(current.to_string());
                    }
                    if sticky {
                        self.stack.clear();
                        self.stack.push(g.clone());
                    }
                    Some(g)
                }
                Some(_) => None,
                None if !self.stack.is_empty() => {
                    self.stack.truncate(1);
                    (self.stack[0] != current).then(|| self.stack[0].clone())
                }
                None => None,
            }
        };
        if let (Some(g), Some(k)) = (&switched, self.last.as_mut()) {
            k.5 = g.clone();
        }
        switched
    }
}

/// Why the rule editor refuses a rule.
#[derive(Clone, Debug, PartialEq)]
pub enum RuleProblem {
    /// Empty, or already in this profile: a beep, and the row goes away.
    Beep,
    /// Another profile has it: the warning, with that profile's name.
    Owned { profile: String, message: String },
}

pub fn check_rule(rule: &str, own: &[String], others: &[Candidate]) -> Result<(), RuleProblem> {
    let r = rule.trim();
    if r.is_empty() || own.iter().any(|o| o.trim() == r) {
        return Err(RuleProblem::Beep);
    }
    match others.iter().find(|p| p.rules.iter().any(|x| x.trim() == r)) {
        Some(p) => Err(RuleProblem::Owned { profile: p.name.clone(), message: format!("The profile \u{201c}{}\u{201d} is already bound to hostname \u{201c}{r}\u{201d}.", p.name) }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(host: Option<&'a str>, user: Option<&'a str>, path: Option<&'a str>, job: Option<&'a str>) -> Facts<'a> {
        Facts { host, user, path, job, cmdline: job }
    }

    fn no_expr(_: &str) -> f64 {
        f64::NEG_INFINITY
    }

    fn cand(guid: &str, rules: &[&str]) -> Candidate {
        Candidate { guid: guid.into(), name: guid.into(), rules: rules.iter().map(|r| r.to_string()).collect() }
    }

    #[test]
    fn rules_parse_like_iterm2() {
        let r = Rule::parse("!me@box:/src&vim");
        assert_eq!((r.sticky, r.user.as_deref(), r.host.as_deref(), r.path.as_deref(), r.job.as_deref()), (true, Some("me"), Some("box"), Some("/src"), Some("vim")));
        let r = Rule::parse("box");
        assert_eq!((r.host.as_deref(), r.user, r.path), (Some("box"), None, None));
        let r = Rule::parse("me@");
        assert_eq!((r.user.as_deref(), r.host), (Some("me"), None));
        assert_eq!(Rule::parse("/a:b@c").path.as_deref(), Some("/a:b@c"), "a leading / is a path, whatever follows");
        let r = Rule::parse("box:/x");
        assert_eq!((r.host.as_deref(), r.path.as_deref()), (Some("box"), Some("/x")));
        let r = Rule::parse("foo:bar@baz");
        assert_eq!((r.user.as_deref(), r.host, r.path), (Some("foo:bar"), None, None), "a colon before the @ is malformed: iTerm2 keeps only the user");
        assert_eq!(Rule::parse("!{a + 1}").expression.as_deref(), Some("a + 1"));
        let r = Rule::parse("&vim");
        assert_eq!((r.job.as_deref(), r.host), (Some("vim"), None));
    }

    #[test]
    fn scores_follow_iterm2() {
        let s = |rule: &str, f: Facts| Rule::parse(rule).score(&f, &no_expr);
        let here = facts(Some("a.example.com"), Some("me"), Some("/foo/bar"), Some("vim"));
        assert_eq!(s("a.example.com", here), 32.0);
        assert_eq!(s("*.EXAMPLE.com", here), 16.0 * (1.0 + 13.0 / 14.0), "a host glob ignores case");
        assert_eq!(s("A.example.com", here), 16.0 * (1.0 + 13.0 / 14.0), "the exact test does not: a partial match");
        assert_eq!(s("*", here), 1.0, "a lone * is the catch-all");
        assert_eq!(s("me@*", here), 4.0, "* with a user adds nothing");
        assert_eq!(s("you@", here), 0.0);
        assert_eq!(s("/foo/bar/*", here), 10.0 / 11.0, "/foo/bar/* matches /foo/bar");
        assert_eq!(s("/foo/bar", here), 2.0);
        assert_eq!(s("/foo", here), 0.0);
        assert_eq!(s("&vi*", here), 8.0);
        assert_eq!(s("&emacs", here), 0.0, "a job that does not match fails the rule");
        assert_eq!(s("a.example.com&emacs", here), 0.0);
        assert!(glob("abc", "a*c", true) && !glob("abcabd", "a*c", true) && glob("xab", "*ab", true) && !glob("ABC", "abc", true));
        assert!(!glob("abcabc", "a*c", true), "iTerm2 matches each piece at its first place, so this fails there too");
        let profiles = [cand("A", &["a.example.com"]), cand("B", &["a.example.com"]), cand("C", &["*"])];
        assert_eq!(best(&profiles, &here, &no_expr).map(|b| b.0), Some("B".to_string()), "the same rule in two profiles: the last one owns it");
        let tie = [cand("A", &["*.example.com"]), cand("B", &["a.example.co*"])];
        assert_eq!(best(&tie, &here, &no_expr).map(|b| b.0), Some("A".to_string()), "a tie goes to the first");
        let win = facts(None, None, Some(r"C:\src"), None);
        assert_eq!(s(r"C:\src\*", win), 8.0 / 9.0, "a Windows folder rule with a trailing \\* matches the folder");
        assert_eq!(best(&[cand("A", &["zzz"])], &here, &no_expr), None, "nothing above 0");
        let expr = |e: &str| if e == "two" { 2.5 } else { f64::NEG_INFINITY };
        assert_eq!(Rule::parse("{two}").score(&here, &expr), 2.5);
    }

    #[test]
    fn stack_push_pop_and_trim() {
        let profiles = [cand("D", &[]), cand("A", &["a"]), cand("B", &["b"])];
        let mut aps = Switcher::default();
        let mut cur = "D".to_string();
        let go = |aps: &mut Switcher, cur: &mut String, host: Option<&str>| {
            let f = facts(host, None, None, None);
            if let Some(g) = aps.decide(cur, &f, &profiles, &no_expr) {
                *cur = g;
            }
        };
        go(&mut aps, &mut cur, Some("a"));
        assert_eq!((cur.as_str(), aps.stack()), ("A", vec!["D"]));
        go(&mut aps, &mut cur, Some("b"));
        assert_eq!((cur.as_str(), aps.stack()), ("B", vec!["D", "A"]));
        go(&mut aps, &mut cur, Some("a"));
        assert_eq!((cur.as_str(), aps.stack()), ("A", vec!["D"]), "A is in the stack and outranks B");
        go(&mut aps, &mut cur, Some("zzz"));
        assert_eq!((cur.as_str(), aps.stack()), ("D", vec!["D"]), "nothing matches: back to the original");
        assert_eq!(aps.decide("D", &facts(Some("zzz"), None, None, None), &profiles, &no_expr), None, "nothing changed");
    }

    #[test]
    fn sticky_resets_the_stack() {
        let profiles = [cand("D", &[]), cand("A", &["a"]), cand("B", &["!b"])];
        let mut aps = Switcher::default();
        let a = aps.decide("D", &facts(Some("a"), None, None, None), &profiles, &no_expr).unwrap();
        let b = aps.decide(&a, &facts(Some("b"), None, None, None), &profiles, &no_expr).unwrap();
        assert_eq!((b.as_str(), aps.stack()), ("B", vec!["B"]));
        assert_eq!(aps.decide(&b, &facts(Some("zzz"), None, None, None), &profiles, &no_expr), None, "B is the new base");
    }

    #[test]
    fn editor_rejects_duplicates() {
        let others = [cand("X", &["box"])];
        assert_eq!(check_rule("  ", &[], &others), Err(RuleProblem::Beep));
        assert_eq!(check_rule("a", &["a".to_string()], &others), Err(RuleProblem::Beep));
        assert_eq!(check_rule("box", &[], &others), Err(RuleProblem::Owned { profile: "X".into(), message: "The profile \u{201c}X\u{201d} is already bound to hostname \u{201c}box\u{201d}.".into() }));
        assert_eq!(check_rule("new", &[], &others), Ok(()));
    }

    #[test]
    fn windows_folders_ignore_case() {
        let s = |rule: &str, path: &str| Rule::parse(rule).score(&Facts { path: Some(path), ..Default::default() }, &no_expr);
        assert_eq!(s(r"C:\src\*", r"c:\SRC\x"), 8.0 / 9.0, "cd c:\\src\\x still matches");
        assert_eq!(s(r"C:\src", r"c:\Src"), 2.0);
        assert_eq!(s(r"C:/src/*", r"C:\src\x"), 8.0 / 9.0, "either slash");
        assert_eq!(s("/foo", "/FOO"), 0.0, "a Unix path keeps iTerm2's case");
    }
}
