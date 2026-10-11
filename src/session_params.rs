//! The "Session Parameter" prompt's pure half (`iTermSessionFactory.m:215-297`, `:484-523`):
//! which `$$NAME$$` variables a new session's command, name and folder hold, how the prompt
//! names them, and how the answers go into the texts.
//!
//! Rulings (m13y2): an answer is inserted exactly as the person typed it, with no shell quoting
//! (iTerm2 does none); an answer is never scanned again, so it cannot name another variable;
//! URL values are never substituted (14e-D43); the variable names are untrusted text (a profile
//! can arrive by remote prefs or Import All), so a launch with more than `MAX_NAMES` distinct
//! names or a name over `MAX_NAME_LEN` characters is refused, and a name is shown through
//! `inert_files::shown_name`. Answers are not logged and `Queue`'s `Debug` omits them.

use std::collections::BTreeSet;

/// The prompt's title (the xib's window title).
pub const TITLE: &str = "Session Parameter";

/// Most distinct names one launch may ask for.
pub const MAX_NAMES: usize = 8;

/// Longest variable name (with its `$$` marks) a launch may ask for.
pub const MAX_NAME_LEN: usize = 64;

/// Longest answer kept, in characters.
pub const MAX_ANSWER: usize = 4096;

/// Every `$$NAME$$` of `text` (`NSStringITerm.m:1729-1752`): pairs of `$$` delimit a candidate;
/// `$$$$` is the escape for `$$`, not a variable; an unmatched trailing `$$` is ignored.
pub fn double_dollar_variables(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut at = 0;
    while let Some(start) = text[at..].find("$$").map(|i| at + i) {
        let Some(end) = text[start + 2..].find("$$").map(|i| start + 2 + i + 2) else { break };
        // A candidate of four characters is `$$$$`, the escape.
        if end - start > 4 {
            set.insert(text[start..end].to_string());
        }
        at = end;
    }
    set
}

/// The name as the prompt shows it (`iTermSessionFactory.m:492-499`).
pub fn pretty_name(var: &str) -> String {
    let name = var.replace("$$", "").replace('_', " ").to_lowercase();
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The prompt's line for `var`.
pub fn label(var: &str) -> String {
    let shown = crate::inert_files::shown_name(&pretty_name(var));
    format!("Enter the value for \u{201c}{shown}\u{201d}:")
}

/// Every variable of the three texts that `known` does not answer, in a stable (sorted) order.
pub fn missing(command: &str, name: &str, dir: &str, known: &[(&str, String)]) -> Vec<String> {
    let mut all = double_dollar_variables(command);
    all.extend(double_dollar_variables(name));
    all.extend(double_dollar_variables(dir));
    all.into_iter().filter(|v| !known.iter().any(|(k, _)| k == v)).collect()
}

/// The answer of a session that cannot prompt (`iTermSessionFactory.m:488-490`).
pub fn answer_when_disabled() -> String {
    String::new()
}

/// Why a launch asking for `vars` is refused, when it is.
pub fn refusal(vars: &[String]) -> Option<String> {
    if vars.len() > MAX_NAMES {
        return Some(format!("This session asks for {} parameters; at most {MAX_NAMES} are allowed, so it did not start.", vars.len()));
    }
    if vars.iter().any(|v| v.chars().count() > MAX_NAME_LEN) {
        return Some(format!("A parameter name is longer than {MAX_NAME_LEN} characters, so the session did not start."));
    }
    None
}

/// `text` with `$$$$` as `$$` and each variable of `answers` replaced, in one pass: an answer is
/// never scanned again. A variable with no answer stays as written.
pub fn apply(text: &str, answers: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(start) = text[at..].find("$$").map(|i| at + i) {
        out.push_str(&text[at..start]);
        let Some(end) = text[start + 2..].find("$$").map(|i| start + 2 + i + 2) else {
            at = start;
            break;
        };
        let candidate = &text[start..end];
        if candidate == "$$$$" {
            out.push_str("$$");
        } else if let Some((_, value)) = answers.iter().find(|(k, _)| k == candidate) {
            out.push_str(value);
        } else {
            out.push_str(candidate);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// The answer as kept: at most `MAX_ANSWER` characters.
pub fn clean_answer(typed: &str) -> String {
    typed.chars().take(MAX_ANSWER).collect()
}

/// The names still to ask and the answers so far, one session's worth.
#[derive(Clone, PartialEq, Eq)]
pub struct Queue {
    asking: Vec<String>,
    answers: Vec<(String, String)>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Queue {{ asking: {}, answered: {} }}", self.asking.len(), self.answers.len())
    }
}

impl Queue {
    pub fn new(vars: Vec<String>) -> Queue {
        Queue { asking: vars, answers: Vec::new() }
    }

    /// The name to ask now.
    pub fn current(&self) -> Option<&str> {
        self.asking.get(self.answers.len()).map(String::as_str)
    }

    /// Records the answer to the current name; true when it was the last.
    pub fn answer(&mut self, typed: &str) -> bool {
        if let Some(var) = self.current().map(str::to_string) {
            self.answers.push((var, clean_answer(typed)));
        }
        self.current().is_none()
    }

    /// The answers once every name is answered.
    pub fn answers(&self) -> &[(String, String)] {
        &self.answers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn double_dollar_variables_follow_iterm2() {
        assert_eq!(double_dollar_variables("ssh $$HOST$$ -p $$PORT$$"), set(&["$$HOST$$", "$$PORT$$"]));
        assert_eq!(double_dollar_variables("a $$$$ b"), set(&[]), "the escape");
        assert_eq!(double_dollar_variables("$$A$$$$B$$"), set(&["$$A$$", "$$B$$"]));
        assert_eq!(double_dollar_variables("x $$A"), set(&[]), "unmatched");
        assert_eq!(double_dollar_variables(""), set(&[]));
        assert_eq!(pretty_name("$$MY_HOST$$"), "My host");
        assert_eq!(pretty_name("$$x$$"), "X");
        assert_eq!(label("$$MY_HOST$$"), "Enter the value for \u{201c}My host\u{201d}:");
        assert_eq!(TITLE, "Session Parameter");
        let known = [("$$USER$$", "u".to_string())];
        assert_eq!(missing("go $$USER$$@$$TARGET$$", "$$TARGET$$", "", &known), vec!["$$TARGET$$".to_string()]);
        assert_eq!(missing("$$B$$ $$A$$", "$$C$$", "$$A$$", &[]), vec!["$$A$$".to_string(), "$$B$$".to_string(), "$$C$$".to_string()], "one name once, in a stable order");
        assert_eq!(answer_when_disabled(), "");
    }

    #[test]
    fn apply_substitutes_once_and_never_rescans() {
        let a = vec![("$$HOST$$".to_string(), "box".to_string())];
        assert_eq!(apply("ssh $$HOST$$", &a), "ssh box");
        assert_eq!(apply("cost $$$$5", &[]), "cost $$5", "the escape alone");
        assert_eq!(apply("$$A$$$$B$$", &[("$$A$$".into(), "1".into()), ("$$B$$".into(), "2".into())]), "12");
        assert_eq!(apply("x $$HOST", &a), "x $$HOST", "unmatched stays");
        assert_eq!(apply("$$UNKNOWN$$", &a), "$$UNKNOWN$$");
        let chained = vec![("$$A$$".to_string(), "$$B$$".to_string()), ("$$B$$".to_string(), "no".to_string())];
        assert_eq!(apply("$$A$$", &chained), "$$B$$", "an answer is never scanned again");
        let quote = vec![("$$A$$".to_string(), "a; b \"c\"".to_string())];
        assert_eq!(apply("run $$A$$", &quote), "run a; b \"c\"", "no shell quoting, as iTerm2");
    }

    #[test]
    fn untrusted_names_are_capped_and_cleaned() {
        let many: Vec<String> = (0..9).map(|i| format!("$$V{i}$$")).collect();
        assert!(refusal(&many).is_some());
        assert!(refusal(&many[..8]).is_none());
        assert!(refusal(&[format!("$${}$$", "A".repeat(80))]).is_some());
        let shown = label("$$EV\u{202e}IL\u{200b}$$");
        assert!(!shown.contains('\u{202e}') && !shown.contains('\u{200b}'));
        let long = label(&format!("$${}$$", "a".repeat(500)));
        assert!(long.chars().count() < 260);
        assert_eq!(clean_answer(&"z".repeat(MAX_ANSWER + 5)).chars().count(), MAX_ANSWER);
    }

    #[test]
    fn queue_asks_one_name_at_a_time() {
        let mut q = Queue::new(vec!["$$A$$".into(), "$$B$$".into()]);
        assert_eq!(q.current(), Some("$$A$$"));
        assert!(!q.answer("one"));
        assert_eq!(q.current(), Some("$$B$$"));
        assert!(q.answer("two"));
        assert_eq!(q.current(), None);
        assert_eq!(q.answers(), &[("$$A$$".to_string(), "one".to_string()), ("$$B$$".to_string(), "two".to_string())]);
        assert!(!format!("{q:?}").contains("one"), "values never reach a log");
    }
}
