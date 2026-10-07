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
/// How long one page Smart Selection may search (P20).
const SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_millis(150);
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

/// The text of `text` from UTF-16 unit `from`, as a string (a lone surrogate is U+FFFD, one unit).
fn from_units(units: &[u16], from: usize) -> String {
    String::from_utf16_lossy(&units[from.min(units.len())..])
}

/// The UTF-16 unit count of the first `bytes` bytes of `text`.
fn units_before(text: &str, bytes: usize) -> usize {
    text[..bytes.min(text.len())].encode_utf16().count()
}

/// What a Win-click runs: the first action of the matched rule, when that rule has one
/// (`iTermBrowserWebView+Actions.swift:231`, `BWV:528-546`).
pub fn win_click_action(rules: &[Rule], m: &WebMatch) -> Option<smartsel::RuleAction> {
    rules.get(m.rule)?.actions.first().cloned()
}

/// The best Smart Selection match around a click in the page (`WVA:306-377`): `before` and `after`
/// are the page text on each side of the click. A rule's match must cover the click (units on both
/// sides); the best score wins per matched text, and the best of those overall. Counts are UTF-16
/// units (`WebMatch::before` and `after`). A rule without an action is skipped only when an action is
/// required. The text around the click is bounded to `MAX_UNITS` on each side and the rules to
/// `MAX_RULES` (P20); a rule that does not compile, or that runs past `fancy_regex`'s backtrack limit,
/// gives nothing.
pub fn first_match(before: &str, after: &str, rules: &[Rule], require_action: bool) -> Option<WebMatch> {
    let mut before_units: Vec<u16> = before.encode_utf16().collect();
    if before_units.len() > MAX_UNITS {
        // Keep the units next to the click; never start on the second half of a pair.
        let mut cut = before_units.len() - MAX_UNITS;
        if (0xDC00..0xE000).contains(&before_units[cut]) {
            cut += 1;
        }
        before_units.drain(..cut);
    }
    let after_text = bounded(after);
    let after_len = after_text.encode_utf16().count();
    let before_len = before_units.len();
    // iTerm2 restarts the search one unit after each match start; a hostile page can make that long, so
    // the whole search has a budget and gives the best match found when it runs out (P20).
    let deadline = std::time::Instant::now() + SEARCH_BUDGET;
    // The best match for each matched text, in the order first found (a tie keeps the earlier).
    let mut best: Vec<(String, WebMatch)> = Vec::new();
    for (index, rule) in rules.iter().enumerate().take(MAX_RULES) {
        if require_action && rule.actions.is_empty() {
            continue;
        }
        let Some(re) = compiled(&rule.regex) else { continue };
        let mut start = 0;
        // Every pass moves the start on; the bound stops a pathological text.
        let mut passes = 0;
        while start < before_len && passes < MAX_UNITS {
            passes += 1;
            if passes % 64 == 0 && std::time::Instant::now() > deadline {
                break;
            }
            if (0xDC00..0xE000).contains(&before_units[start]) {
                start += 1;
                continue;
            }
            let mut text = from_units(&before_units, start);
            text.push_str(after_text);
            let found = match re.captures(&text) {
                Ok(Some(c)) => c,
                _ => break,
            };
            let Some(whole) = found.get(0).filter(|m| !m.as_str().is_empty()) else { break };
            let lower = units_before(&text, whole.start());
            let upper = lower + whole.as_str().encode_utf16().count();
            let (abs_lower, abs_upper) = (start + lower, start + upper);
            // The part of the match before the click, and the part after it.
            let before_count = abs_upper.min(before_len).saturating_sub(abs_lower);
            let after_count = abs_upper.min(before_len + after_len).saturating_sub(abs_lower.max(before_len));
            if before_count > 0 && after_count > 0 {
                let score = rule.precision * (upper - lower) as f64;
                let matched = whole.as_str().to_string();
                let captures: Vec<String> = (0..found.len()).map(|g| found.get(g).map_or(String::new(), |c| c.as_str().to_string())).collect();
                let m = WebMatch { before: before_count, after: after_count, rule: index, captures, score };
                match best.iter_mut().find(|(t, _)| *t == matched) {
                    Some((_, old)) if old.score >= score => {}
                    Some((_, old)) => *old = m,
                    None => best.push((matched, m)),
                }
                start += upper - 1;
            } else {
                start += lower;
            }
            start += 1;
        }
    }
    let mut out: Option<WebMatch> = None;
    for (_, m) in best {
        if out.as_ref().is_none_or(|o| o.score < m.score) {
            out = Some(m);
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

    /// iTerm2's Cmd-click (`iTermBrowserWebView+Actions.swift:231`) matches with `requireAction: false`:
    /// the best overall rule is selected, and an action runs only if THAT rule has one.
    #[test]
    fn a_win_click_acts_only_on_the_best_rule() {
        use crate::smartsel::RuleAction;
        let action = RuleAction { action: "a".into(), title: "t".into(), parameter: "p".into() };
        let mut with = rule(r"[a-z]+", 1.0);
        with.actions.push(action.clone());
        let best_without = rule(r"[a-z0-9]+@[a-z]+", 1000.0);
        let rules = vec![with.clone(), best_without];
        // Best overall has no action: it is selected, nothing runs (the lower rule's action stays unrun).
        let m = first_match("bob@ex", "ample", &rules, false).unwrap();
        assert_eq!(m.rule, 1);
        assert_eq!(win_click_action(&rules, &m), None);
        // Best overall has one: its first action runs.
        let m = first_match("bob@ex", "ample", &[with.clone()], false).unwrap();
        assert_eq!(win_click_action(&[with], &m), Some(action));
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

    /// `WVA:306-377`: the page's matcher. A match must straddle the point; counts are UTF-16 units on
    /// each side; the best score per matched text wins; rules without actions are skipped only when an
    /// action is required.
    #[test]
    fn page_smart_selection_follows_iterm2() {
        use crate::smartsel::RuleAction;
        let with_action = |regex: &str, precision: f64| Rule {
            regex: regex.into(),
            precision,
            notes: String::new(),
            actions: vec![RuleAction { title: "Copy".into(), action: "copy:".into(), parameter: "\\0".into() }],
        };
        let email = with_action(r"\S+@\S+", 1000.0);
        let word = rule(r"\w+", 1.0);
        let rules = vec![email.clone(), word.clone()];
        let m = first_match("mail bob@ex", "ample.com now", &rules, false).unwrap();
        assert_eq!((m.rule, m.before, m.after), (0, 6, 9));
        assert_eq!(m.score, 1000.0 * 15.0);
        assert_eq!(m.captures[0], "bob@example.com");
        // With an action required the email rule still wins; only the actionless rule gives nothing.
        assert_eq!(first_match("mail bob@ex", "ample.com now", &rules, true).unwrap().rule, 0);
        assert_eq!(first_match("say hel", "lo there", &[word.clone()], false).unwrap().captures[0], "hello");
        assert!(first_match("say hel", "lo there", &[word.clone()], true).is_none());
        // A match wholly before or wholly after the click is ignored.
        assert!(first_match("bob@ex.com ", "tail", &[email.clone()], false).is_none());
        assert!(first_match("head ", "bob@ex.com", &[email.clone()], false).is_none());
        // A surrogate pair before the click counts two units, and a start inside it is skipped.
        let emoji = rule(r"\S+", 1.0);
        let m = first_match("a \u{1f600}b", "cd e", &[emoji.clone()], false).unwrap();
        assert_eq!((m.before, m.after), (3, 2), "the pair is two units: the word is b + the pair");
        let m = first_match("\u{1f600}", "x", &[rule(r".+", 1.0)], false).unwrap();
        assert_eq!((m.before, m.after), (2, 1));
        // The same matched text found twice keeps the higher score.
        let low = rule(r"\w+", 1.0);
        let high = rule(r"\w+", 5.0);
        let m = first_match("xy", "z", &[low, high], false).unwrap();
        assert_eq!((m.rule, m.score), (1, 15.0));
        // A rule that does not compile is skipped; the next one still matches.
        let m = first_match("ab", "cd", &[rule("(", 1.0), rule(r"\w+", 1.0)], false).unwrap();
        assert_eq!(m.rule, 1);
        // Empty text and an empty match give nothing.
        assert!(first_match("", "", &[word.clone()], false).is_none());
        assert!(first_match("a", "b", &[rule("x*", 1.0)], false).is_none());
        // The text around the point is bounded (P20): a match that needs more than the cap is not found.
        let far = format!("w{}", "x".repeat(MAX_UNITS + 10));
        assert!(first_match(&far, "y", &[rule("w[x]+y", 1.0)], false).is_none());
        assert!(first_match("wxx", "y", &[rule("w[x]+y", 1.0)], false).is_some());
    }
}
