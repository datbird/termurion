//! The Smart Selection editor's "Open Regular Expression Visualizer": a regular expression read into
//! its parts (iTerm2 draws a railroad diagram from its `ICURegexToRailroadConverter`) and drawn as
//! nested boxes: a sequence runs left to right, alternatives are listed under "one of", and a
//! group or a repeat wraps what it holds. The reader is forgiving: it never fails, and what it
//! does not know is shown as text.

/// A part of a regular expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    /// Characters that match themselves.
    Text(String),
    /// A bracketed set, as written.
    Class(String),
    /// An escape or a dot, in words.
    Word(String),
    /// A position, not a character (`^`, `$`, `\b`).
    Anchor(String),
    /// A group: its title (capturing number, name, look-around) and what it holds.
    Group(String, Box<Node>),
    /// How often a part repeats, in words.
    Repeat(Box<Node>, String),
}

struct Reader {
    chars: Vec<char>,
    at: usize,
    groups: usize,
}

/// Reads `pattern`.
pub fn parse(pattern: &str) -> Node {
    let mut r = Reader { chars: pattern.chars().collect(), at: 0, groups: 0 };
    let mut tree = r.alternation();
    // A stray ")" ends the first read; the rest is read on, as part of a sequence.
    while r.at < r.chars.len() {
        r.at += 1;
        let more = r.alternation();
        tree = Node::Seq(vec![tree, Node::Text(")".into()), more]);
    }
    tree
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn alternation(&mut self) -> Node {
        let mut options = vec![self.sequence()];
        while self.peek() == Some('|') {
            self.at += 1;
            options.push(self.sequence());
        }
        if options.len() == 1 { options.remove(0) } else { Node::Alt(options) }
    }

    fn sequence(&mut self) -> Node {
        let mut parts: Vec<Node> = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom();
            let atom = self.quantified(atom);
            let merged = match (parts.last_mut(), &atom) {
                (Some(Node::Text(t)), Node::Text(more)) => {
                    t.push_str(more);
                    true
                }
                _ => false,
            };
            if !merged {
                parts.push(atom);
            }
        }
        if parts.len() == 1 { parts.remove(0) } else { Node::Seq(parts) }
    }

    fn atom(&mut self) -> Node {
        let c = self.chars[self.at];
        self.at += 1;
        match c {
            '.' => Node::Word("any character".into()),
            '^' => Node::Anchor("start of line".into()),
            '$' => Node::Anchor("end of line".into()),
            '[' => Node::Class(self.class()),
            '(' => self.group(),
            '\\' => self.escape(),
            other => Node::Text(other.to_string()),
        }
    }

    fn class(&mut self) -> String {
        let mut text = String::from("[");
        let mut first = true;
        while let Some(c) = self.peek() {
            self.at += 1;
            text.push(c);
            match c {
                '\\' => {
                    if let Some(n) = self.peek() {
                        text.push(n);
                        self.at += 1;
                    }
                }
                '[' if self.peek() == Some(':') => {
                    while let Some(n) = self.peek() {
                        self.at += 1;
                        text.push(n);
                        if n == ']' {
                            break;
                        }
                    }
                }
                ']' if !first => break,
                _ => {}
            }
            first = first && c == '^';
        }
        text
    }

    fn group(&mut self) -> Node {
        let title = if self.peek() == Some('?') {
            let rest: String = self.chars[self.at..].iter().take(4).collect();
            let (skip, name) = if rest.starts_with("?:") {
                (2, "Group (not captured)".to_string())
            } else if rest.starts_with("?=") {
                (2, "Followed by".to_string())
            } else if rest.starts_with("?!") {
                (2, "Not followed by".to_string())
            } else if rest.starts_with("?<=") {
                (3, "Preceded by".to_string())
            } else if rest.starts_with("?<!") {
                (3, "Not preceded by".to_string())
            } else if rest.starts_with("?<") || rest.starts_with("?P<") {
                let from = if rest.starts_with("?P<") { 3 } else { 2 };
                let end = self.chars[self.at..].iter().position(|c| *c == '>').unwrap_or(from).max(from);
                let name: String = self.chars[self.at + from..self.at + end].iter().collect();
                self.groups += 1;
                (end + 1, format!("Group {} \u{201c}{name}\u{201d}", self.groups))
            } else {
                // Flags such as (?i): shown as they are.
                let end = self.chars[self.at..].iter().position(|c| *c == ')').unwrap_or(self.chars.len() - self.at);
                let flags: String = self.chars[self.at..self.at + end].iter().collect();
                self.at += end + usize::from(self.at + end < self.chars.len());
                return Node::Word(format!("flags ({flags})"));
            };
            self.at += skip;
            name
        } else {
            self.groups += 1;
            format!("Group {}", self.groups)
        };
        let inner = self.alternation();
        if self.peek() == Some(')') {
            self.at += 1;
        }
        Node::Group(title, Box::new(inner))
    }

    fn escape(&mut self) -> Node {
        let Some(c) = self.peek() else { return Node::Text("\\".into()) };
        self.at += 1;
        let word = |s: &str| Node::Word(s.to_string());
        match c {
            'd' => word("digit"),
            'D' => word("not a digit"),
            'w' => word("word character"),
            'W' => word("not a word character"),
            's' => word("whitespace"),
            'S' => word("not whitespace"),
            'n' => word("newline"),
            't' => word("tab"),
            'r' => word("carriage return"),
            'b' => Node::Anchor("word boundary".into()),
            'B' => Node::Anchor("not a word boundary".into()),
            'A' => Node::Anchor("start of text".into()),
            'z' | 'Z' => Node::Anchor("end of text".into()),
            'p' | 'P' | 'x' | 'u' => {
                let mut text = format!("\\{c}");
                if self.peek() == Some('{') {
                    while let Some(n) = self.peek() {
                        self.at += 1;
                        text.push(n);
                        if n == '}' {
                            break;
                        }
                    }
                }
                Node::Word(text)
            }
            d if d.is_ascii_digit() && d != '0' => Node::Word(format!("what group {d} matched")),
            other => Node::Text(other.to_string()),
        }
    }

    /// The repeat after an atom, if there is one.
    fn quantified(&mut self, atom: Node) -> Node {
        let said = match self.peek() {
            Some('*') => Some("zero or more times".to_string()),
            Some('+') => Some("one or more times".to_string()),
            Some('?') => Some("optional".to_string()),
            Some('{') => {
                let close = self.chars[self.at..].iter().position(|c| *c == '}');
                let body = close.map(|e| self.chars[self.at + 1..self.at + e].iter().collect::<String>());
                match body.as_deref().map(|b| b.split_once(',').map_or((b, None), |(lo, hi)| (lo, Some(hi)))) {
                    Some((lo, hi)) if !lo.is_empty() && lo.chars().all(|c| c.is_ascii_digit()) && hi.is_none_or(|h| h.chars().all(|c| c.is_ascii_digit())) => {
                        let words = match hi {
                            None => format!("{lo} times"),
                            Some("") => format!("{lo} or more times"),
                            Some(h) => format!("{lo} to {h} times"),
                        };
                        self.at += close.unwrap_or(0);
                        Some(words)
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(mut said) = said else { return atom };
        self.at += 1;
        match self.peek() {
            Some('?') => {
                said.push_str(", as few as possible");
                self.at += 1;
            }
            Some('+') => {
                said.push_str(", never giving back");
                self.at += 1;
            }
            _ => {}
        }
        Node::Repeat(Box::new(atom), said)
    }
}

/// Draws `node`.
pub fn show(ui: &mut egui::Ui, node: &Node) {
    let boxed = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).small().weak());
                body(ui);
            });
        });
    };
    match node {
        Node::Seq(parts) => {
            ui.horizontal_wrapped(|ui| {
                for (i, p) in parts.iter().enumerate() {
                    if i > 0 {
                        ui.label("\u{2192}");
                    }
                    show(ui, p);
                }
            });
        }
        Node::Alt(options) => boxed(ui, "one of", &mut |ui| {
            for o in options {
                show(ui, o);
            }
        }),
        Node::Group(title, inner) => boxed(ui, title, &mut |ui| show(ui, inner)),
        Node::Repeat(inner, said) => boxed(ui, said, &mut |ui| show(ui, inner)),
        Node::Text(t) => {
            ui.label(egui::RichText::new(format!("\u{201c}{t}\u{201d}")).monospace());
        }
        Node::Class(t) => {
            ui.label(egui::RichText::new(t).monospace().strong());
        }
        Node::Word(t) | Node::Anchor(t) => {
            ui.label(egui::RichText::new(t).italics());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Node {
        Node::Text(s.into())
    }

    #[test]
    fn literals_join_and_a_repeat_binds_to_the_last_atom() {
        assert_eq!(parse("ab+"), Node::Seq(vec![text("a"), Node::Repeat(Box::new(text("b")), "one or more times".into())]));
        assert_eq!(parse("abc"), text("abc"));
    }

    #[test]
    fn alternation_groups_and_classes() {
        assert_eq!(parse("a|b"), Node::Alt(vec![text("a"), text("b")]));
        let p = parse(r"(\d+)-(?:x|y)");
        assert_eq!(
            p,
            Node::Seq(vec![
                Node::Group("Group 1".into(), Box::new(Node::Repeat(Box::new(Node::Word("digit".into())), "one or more times".into()))),
                text("-"),
                Node::Group("Group (not captured)".into(), Box::new(Node::Alt(vec![text("x"), text("y")]))),
            ])
        );
        assert_eq!(parse("[a-z]{2,4}"), Node::Repeat(Box::new(Node::Class("[a-z]".into())), "2 to 4 times".into()));
        assert_eq!(parse("[[:alpha:]_]"), Node::Class("[[:alpha:]_]".into()));
        assert_eq!(parse("a*?"), Node::Repeat(Box::new(text("a")), "zero or more times, as few as possible".into()));
    }

    #[test]
    fn a_broken_pattern_still_reads() {
        // An unclosed group and bracket, a stray ")" and a "{" that is no count.
        for p in ["(abc", "[abc", "a)b", "a{x", "\\", "(?<name>x)", "(?i)a", "(?"] {
            let _ = parse(p);
        }
        assert_eq!(parse("a{x"), text("a{x"));
    }
}
