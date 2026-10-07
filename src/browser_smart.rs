//! Smart Selection in a page (milestone 13x2, Tasks 4 and 7): iTerm2's `WebSmartMatch` and its
//! matcher (`iTermBrowserWebView+Actions.swift`). Counts are UTF-16 units, as the page's text is.
//! Pure. Web content is untrusted: the rules are bounded, the text is cut, a rule that does not
//! compile or that runs past `fancy_regex`'s backtrack limit gives no match.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::smartsel::{self, Rule};

/// The most rules matched (P20).
pub const MAX_RULES: usize = 256;
/// The most UTF-16 units of text matched (P20).
pub const MAX_UNITS: usize = 8192;
/// Compiled patterns kept (one per distinct rule text); the table is cleared when it is full.
const CACHE_MAX: usize = 512;

/// A rule's match in page text (`WVA:10-16`): how many units of it lie before and after the point
/// clicked (both 0 for a selection), the rule's index, the captures (`\0` first), the score.
#[derive(Clone, Debug, PartialEq)]
pub struct WebMatch {
    pub before: usize,
    pub after: usize,
    pub rule: usize,
    pub captures: Vec<String>,
    pub score: f64,
}

thread_local! {
    /// Patterns compile once per rule text (P20); a pattern that does not compile is remembered.
    static COMPILED: RefCell<HashMap<String, Option<Rc<fancy_regex::Regex>>>> = RefCell::new(HashMap::new());
}

/// The compiled pattern of an ICU rule, or None when it does not compile.
pub fn compiled(regex: &str) -> Option<Rc<fancy_regex::Regex>> {
    COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(found) = c.get(regex) {
            return found.clone();
        }
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        let re = fancy_regex::Regex::new(&smartsel::to_rust_regex(regex)).ok().map(Rc::new);
        c.insert(regex.to_string(), re.clone());
        re
    })
}

/// `text` cut to `MAX_UNITS` UTF-16 units, at a character boundary.
fn bounded(text: &str) -> &str {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > MAX_UNITS {
            return &text[..i];
        }
    }
    text
}

/// Every rule's first match in `text`, in rule order (`WVA:282-304`): its captures and the score,
/// the rule's precision times the match's length in UTF-16 units. An empty match counts for
/// nothing (Decision: iTerm2 would offer a rule's actions for an empty match).
pub fn all_matches(rules: &[Rule], text: &str) -> Vec<WebMatch> {
    let text = bounded(text);
    let mut out = Vec::new();
    for (i, rule) in rules.iter().enumerate().take(MAX_RULES) {
        let Some(re) = compiled(&rule.regex) else { continue };
        for caps in re.captures_iter(text).flatten() {
            let Some(m) = caps.get(0) else { continue };
            if m.as_str().is_empty() {
                continue;
            }
            let captures = (0..caps.len()).map(|g| caps.get(g).map_or(String::new(), |c| c.as_str().to_string())).collect();
            let score = rule.precision * m.as_str().encode_utf16().count() as f64;
            out.push(WebMatch { before: 0, after: 0, rule: i, captures, score });
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smartsel::Rule;

    fn rule(regex: &str, precision: f64) -> Rule {
        Rule { regex: regex.into(), precision, notes: String::new(), actions: Vec::new() }
    }

    /// `WVA:282-304`: one match per rule, the first in the text; UTF-16 counts; the score is the
    /// rule's precision times the match's length.
    #[test]
    fn all_matches_follow_iterm2() {
        let rules = vec![rule(r"\d+", 1.0), rule(r"[a-z]+@[a-z]+", 1000.0)];
        let m = all_matches(&rules, "x 42 a@b");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], WebMatch { before: 0, after: 0, rule: 0, captures: vec!["42".into()], score: 2.0 });
        assert_eq!(m[1], WebMatch { before: 0, after: 0, rule: 1, captures: vec!["a@b".into()], score: 3000.0 });
        // Groups are captures; one that did not take part is empty.
        let m = all_matches(&[rule(r"(a)|(b)", 1.0)], "b");
        assert_eq!(m[0].captures, vec!["b".to_string(), "".into(), "b".into()]);
        // The score counts UTF-16 units.
        let m = all_matches(&[rule(r"\p{So}+", 1.0)], "x \u{1f600}");
        assert_eq!(m[0].score, 2.0);
        // No match, an empty match and a rule that does not compile give nothing.
        assert!(all_matches(&[rule("zzz", 1.0), rule("(", 1.0), rule("", 1.0)], "abc").is_empty());
        // The rule index is kept past a skipped rule.
        let m = all_matches(&[rule("(", 1.0), rule("b", 1.0)], "abc");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rule, 1);
    }

    /// P20: the rules and the text are bounded.
    #[test]
    fn matching_is_bounded() {
        let many: Vec<Rule> = (0..300).map(|_| rule("a", 1.0)).collect();
        assert_eq!(all_matches(&many, "a").len(), MAX_RULES);
        let long = format!("{}needle", "x".repeat(MAX_UNITS));
        assert!(all_matches(&[rule("needle", 1.0)], &long).is_empty(), "past the text cap");
        // A hostile pattern gives up at the backtrack limit instead of hanging.
        let _ = all_matches(&[rule(r"(a+)+$", 1.0)], &format!("{}b", "a".repeat(5000)));
    }
}
