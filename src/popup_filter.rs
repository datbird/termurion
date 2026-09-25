//! iTerm2's popup filter (PopupWindowController): what is typed matches when its characters
//! appear in order, ignoring case, within an entry's first 200 characters. After 4 s
//! without typing, the next key starts a new filter; Backspace clears it.

/// The matched character positions, or None.
pub fn matches(filter: &str, value: &str) -> Option<Vec<usize>> {
    let mut want = filter.chars().flat_map(char::to_lowercase).peekable();
    let mut hits = Vec::new();
    for (i, c) in value.chars().take(200).enumerate() {
        let Some(w) = want.peek() else { break };
        if c.to_lowercase().next() == Some(*w) {
            hits.push(i);
            want.next();
        }
    }
    want.peek().is_none().then_some(hits)
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    text: String,
    last: Option<f64>,
}

impl Filter {
    pub fn type_char(&mut self, c: char, now: f64) {
        if self.last.is_some_and(|t| now - t > 4.0) {
            self.text.clear();
        }
        self.text.push(c);
        self.last = Some(now);
    }

    pub fn backspace(&mut self) {
        self.text.clear();
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popups_filter_like_iterm2() {
        assert_eq!(matches("gco", "git checkout main"), Some(vec![0, 4, 9]));
        assert_eq!(matches("GCO", "git checkout"), Some(vec![0, 4, 9]), "ignores case");
        assert_eq!(matches("", "x"), Some(vec![]));
        assert_eq!(matches("zz", "git"), None);
        let far = format!("{}z", "a".repeat(200));
        assert_eq!(matches("z", &far), None, "only the first 200 characters");
        let mut f = Filter::default();
        f.type_char('g', 0.0);
        f.type_char('c', 1.0);
        assert_eq!(f.text(), "gc");
        f.type_char('x', 6.0);
        assert_eq!(f.text(), "x", "4 s idle: a new filter");
        f.type_char('y', 6.5);
        f.backspace();
        assert_eq!(f.text(), "", "Backspace clears it all");
    }
}
