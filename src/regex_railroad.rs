//! The Smart Selection editor's "Open Regular Expression Visualizer": the regular expression drawn as
//! a railroad diagram (SVG) in a page that opens in the system browser. A port of iTerm2's
//! `ICURegexToRailroadConverter.swift` (which writes a railroad DSL for its `railroad_dsl` library);
//! this builds the same diagram's nodes for the `railroad` crate directly. iTerm2 shows the page in a
//! web view popover; Termurion has no popover, so the page is a file in the temp folder.

use railroad::{Choice, Comment, Diagram, Empty, End, LabeledBox, Node, NonTerminal, Optional, Repeat, Sequence, Start, Stylesheet, Terminal};

/// A part of the diagram, in the DSL's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum D {
    /// `!`
    Empty,
    /// `"text"`
    Term(String),
    /// `'text'`
    NonTerm(String),
    /// `` `text` ``
    Comment(String),
    /// `{a, b}`
    Seq(Vec<D>),
    /// `<a, b>`
    Choice(Vec<D>),
    /// `a*b`
    Repeat(Box<D>, Box<D>),
    /// `a?`
    Optional(Box<D>),
    /// `a#`label``
    Boxed(Box<D>, String),
}

/// What `parseElement` returns: a plain character that a sequence joins into a literal, or a part.
enum El {
    Char(char),
    Part(D),
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

/// Reads `pattern` into the diagram's parts.
pub fn convert(pattern: &str) -> D {
    let mut p = Parser { chars: pattern.chars().collect(), at: 0 };
    p.alternation()
}

fn term(s: &str) -> D {
    D::Term(s.to_string())
}

fn comment(s: &str) -> D {
    D::Comment(s.to_string())
}

impl Parser {
    fn more(&self) -> bool {
        self.at < self.chars.len()
    }

    fn peek(&self) -> char {
        self.chars.get(self.at).copied().unwrap_or('\0')
    }

    fn ahead(&self) -> char {
        self.chars.get(self.at + 1).copied().unwrap_or('\0')
    }

    fn advance(&mut self) {
        if self.more() {
            self.at += 1;
        }
    }

    fn alternation(&mut self) -> D {
        let mut options = Vec::new();
        let mut current = self.sequence();
        while self.more() && self.peek() == '|' {
            options.push(current);
            self.advance();
            current = self.sequence();
        }
        if options.is_empty() {
            current
        } else {
            options.push(current);
            D::Choice(options)
        }
    }

    fn sequence(&mut self) -> D {
        let mut parts: Vec<D> = Vec::new();
        let mut literal = String::new();
        while self.more() && self.peek() != '|' && self.peek() != ')' {
            let at = self.at;
            match self.element() {
                Some(El::Char(c)) => literal.push(c),
                Some(El::Part(d)) => {
                    if !literal.is_empty() {
                        parts.push(D::Term(std::mem::take(&mut literal)));
                    }
                    parts.push(d);
                }
                None => {
                    // A quantifier with nothing before it is dropped; a stray ")" ends the read.
                    if self.at == at {
                        break;
                    }
                }
            }
        }
        if !literal.is_empty() {
            parts.push(D::Term(literal));
        }
        match parts.len() {
            0 => D::Empty,
            1 => parts.remove(0),
            _ => D::Seq(parts),
        }
    }

    fn element(&mut self) -> Option<El> {
        if !self.more() {
            return None;
        }
        let c = self.peek();
        match c {
            '(' => Some(El::Part(self.group())),
            '[' => Some(El::Part(self.class())),
            '\\' => self.escape().map(El::Part),
            '.' => {
                self.advance();
                Some(El::Part(self.quantified(term("[any character]"))))
            }
            '^' => {
                self.advance();
                Some(El::Part(comment("start of line")))
            }
            '$' => {
                self.advance();
                Some(El::Part(comment("end of line")))
            }
            '*' | '+' | '?' | '{' => {
                self.advance();
                None
            }
            ')' => None,
            _ => {
                self.advance();
                if self.more() && matches!(self.peek(), '*' | '+' | '?' | '{') {
                    return Some(El::Part(self.quantified(D::Term(c.to_string()))));
                }
                Some(El::Char(c))
            }
        }
    }

    fn group(&mut self) -> D {
        self.advance(); // (
        let mut kind = "capture";
        let mut name = String::new();
        if self.more() && self.peek() == '?' {
            self.advance();
            if self.more() {
                let next = self.peek();
                match next {
                    ':' => {
                        self.advance();
                        kind = "non-capture";
                    }
                    '>' => {
                        self.advance();
                        kind = "atomic";
                    }
                    '=' => {
                        self.advance();
                        kind = "lookahead";
                    }
                    '!' => {
                        self.advance();
                        kind = "negative-lookahead";
                    }
                    '#' => {
                        self.advance();
                        while self.more() && self.peek() != ')' {
                            self.advance();
                        }
                        self.advance();
                        return comment("comment");
                    }
                    '<' => {
                        self.advance();
                        if self.more() {
                            match self.peek() {
                                '=' => {
                                    self.advance();
                                    kind = "lookbehind";
                                }
                                '!' => {
                                    self.advance();
                                    kind = "negative-lookbehind";
                                }
                                _ => {
                                    while self.more() && self.peek() != '>' {
                                        name.push(self.peek());
                                        self.advance();
                                    }
                                    self.advance();
                                    kind = "named";
                                }
                            }
                        }
                    }
                    f if "ismwx".contains(f) || f == '-' => {
                        let mut flags = String::new();
                        while self.more() && self.peek() != ':' && self.peek() != ')' {
                            flags.push(self.peek());
                            self.advance();
                        }
                        if self.more() && self.peek() == ':' {
                            self.advance();
                            kind = "flags";
                            name = flags;
                        } else {
                            self.advance();
                            return comment(&format!("flags: {flags}"));
                        }
                    }
                    _ => {}
                }
            }
        }
        let content = self.alternation();
        if self.more() && self.peek() == ')' {
            self.advance();
        }
        let label = match kind {
            "named" => Some(name),
            "atomic" => Some("atomic".to_string()),
            "lookahead" => Some("lookahead".to_string()),
            "negative-lookahead" => Some("negative lookahead".to_string()),
            "lookbehind" => Some("lookbehind".to_string()),
            "negative-lookbehind" => Some("negative lookbehind".to_string()),
            "flags" => Some(format!("flags: {name}")),
            _ => None,
        };
        let result = match label {
            Some(l) => D::Boxed(Box::new(content), l),
            None => content,
        };
        self.quantified(result)
    }

    fn class(&mut self) -> D {
        self.advance(); // [
        let mut negated = false;
        let mut elements: Vec<D> = Vec::new();
        if self.more() && self.peek() == '^' {
            negated = true;
            self.advance();
        }
        while self.more() && self.peek() != ']' {
            if self.peek() == '\\' {
                self.advance();
                if self.more() {
                    let e = self.peek();
                    self.advance();
                    elements.push(class_escape(e));
                }
            } else if self.peek() == '[' && self.ahead() == ':' {
                let mut posix = String::new();
                self.advance();
                self.advance();
                while self.more() && !(self.peek() == ':' && self.ahead() == ']') {
                    posix.push(self.peek());
                    self.advance();
                }
                self.advance();
                self.advance();
                elements.push(D::Term(format!("[{posix}]")));
            } else {
                let c = self.peek();
                self.advance();
                if self.more() && self.peek() == '-' && self.ahead() != ']' {
                    self.advance();
                    if self.more() {
                        let end = self.peek();
                        self.advance();
                        elements.push(D::Term(format!("[{c}-{end}]")));
                    }
                } else {
                    elements.push(D::Term(c.to_string()));
                }
            }
        }
        if self.more() && self.peek() == ']' {
            self.advance();
        }
        let content = if elements.is_empty() { D::Empty } else { D::Choice(elements) };
        let result = if negated { D::Seq(vec![comment("not"), content]) } else { content };
        self.quantified(result)
    }

    fn hex(&mut self, n: usize) -> String {
        let mut hex = String::new();
        for _ in 0..n {
            if self.more() && self.peek().is_ascii_hexdigit() {
                hex.push(self.peek());
                self.advance();
            }
        }
        hex
    }

    /// The text up to `close`, which is skipped.
    fn until(&mut self, close: char) -> String {
        let mut s = String::new();
        while self.more() && self.peek() != close {
            s.push(self.peek());
            self.advance();
        }
        self.advance();
        s
    }

    fn escape(&mut self) -> Option<D> {
        self.advance(); // backslash
        if !self.more() {
            return Some(term("\\"));
        }
        let e = self.peek();
        self.advance();
        let word = |me: &mut Parser, t: &str| Some(me.quantified(D::Term(format!("[{t}]"))));
        match e {
            'a' => word(self, "bell"),
            'A' => Some(comment("start of input")),
            'b' => Some(comment("word boundary")),
            'B' => Some(comment("non-word boundary")),
            'd' => word(self, "digit"),
            'D' => word(self, "non-digit"),
            'e' => word(self, "escape"),
            'f' => word(self, "form feed"),
            'G' => Some(comment("end of previous match")),
            'h' => word(self, "horizontal whitespace"),
            'H' => word(self, "non-horizontal whitespace"),
            'n' => word(self, "line feed"),
            'r' => word(self, "carriage return"),
            'R' => word(self, "newline"),
            's' => word(self, "whitespace"),
            'S' => word(self, "non-whitespace"),
            't' => word(self, "tab"),
            'v' => word(self, "vertical whitespace"),
            'V' => word(self, "non-vertical whitespace"),
            'w' => word(self, "word character"),
            'W' => word(self, "non-word character"),
            'X' => word(self, "grapheme cluster"),
            'Z' => Some(comment("end of input (before final newline)")),
            'z' => Some(comment("end of input")),
            'u' => {
                let h = self.hex(4);
                word(self, &format!("U+{h}"))
            }
            'U' => {
                let h = self.hex(8);
                word(self, &format!("U+{h}"))
            }
            'x' => {
                let h = if self.more() && self.peek() == '{' {
                    self.advance();
                    self.until('}')
                } else {
                    self.hex(2)
                };
                word(self, &format!("U+{h}"))
            }
            'N' => {
                if self.more() && self.peek() == '{' {
                    self.advance();
                    let n = self.until('}');
                    return word(self, &n);
                }
                Some(self.quantified(term("N")))
            }
            'p' | 'P' => {
                if self.more() && self.peek() == '{' {
                    self.advance();
                    let prop = self.until('}');
                    let desc = if e == 'P' { format!("not {prop}") } else { prop };
                    return word(self, &desc);
                }
                Some(self.quantified(D::Term(e.to_string())))
            }
            'k' => {
                if self.more() && self.peek() == '<' {
                    self.advance();
                    let n = self.until('>');
                    return Some(self.quantified(D::NonTerm(n)));
                }
                Some(self.quantified(term("k")))
            }
            '1'..='9' => Some(self.quantified(D::NonTerm(format!("group {e}")))),
            '0' => {
                let mut octal = String::from("0");
                for _ in 0..3 {
                    if self.more() && ('0'..='7').contains(&self.peek()) {
                        octal.push(self.peek());
                        self.advance();
                    }
                }
                word(self, &format!("\\o{octal}"))
            }
            'c' => {
                if self.more() {
                    let c = self.peek();
                    self.advance();
                    return word(self, &format!("control-{c}"));
                }
                Some(self.quantified(term("c")))
            }
            'Q' => {
                let mut quoted = String::new();
                while self.more() {
                    if self.peek() == '\\' && self.ahead() == 'E' {
                        self.advance();
                        self.advance();
                        break;
                    }
                    quoted.push(self.peek());
                    self.advance();
                }
                Some(self.quantified(D::Term(quoted)))
            }
            'E' => None,
            other => Some(self.quantified(D::Term(other.to_string()))),
        }
    }

    /// The repeat after a part (`applyQuantifier`).
    fn quantified(&mut self, base: D) -> D {
        if !self.more() {
            return base;
        }
        let rep = |b: &D| D::Repeat(Box::new(b.clone()), Box::new(D::Empty));
        let opt = |b: D| D::Choice(vec![D::Empty, b]);
        match self.peek() {
            '*' => {
                self.advance();
                let note = self.lazy_or_possessive("*");
                match note {
                    Some(n) => opt(D::Seq(vec![rep(&base), comment(&n)])).fix_choice(&base, &rep(&base), &n),
                    None => opt(rep(&base)),
                }
            }
            '+' => {
                self.advance();
                match self.lazy_or_possessive("+") {
                    Some(n) => D::Seq(vec![rep(&base), comment(&n)]),
                    None => rep(&base),
                }
            }
            '?' => {
                self.advance();
                match self.lazy_or_possessive("?") {
                    Some(n) => D::Choice(vec![D::Empty, base, comment(&n)]),
                    None => opt(base),
                }
            }
            '{' => {
                self.advance();
                let q = self.until('}');
                let suffix = if self.more() && self.peek() == '?' {
                    self.advance();
                    " (lazy)"
                } else if self.more() && self.peek() == '+' {
                    self.advance();
                    " (possessive)"
                } else {
                    ""
                };
                let parts: Vec<String> = q.split(',').map(|p| p.trim().to_string()).collect();
                let with_note = |d: D, n: String| if suffix.is_empty() { d } else { D::Seq(vec![d, D::Comment(n)]) };
                let copies = |n: usize| (0..n).map(|_| base.clone()).collect::<Vec<D>>();
                let number = |s: &str| s.parse::<usize>().ok();
                match parts.as_slice() {
                    [one] => {
                        if let Some(n) = number(one) {
                            return match n {
                                0 => D::Empty,
                                1 => with_note(base.clone(), format!("{{{n}}}{suffix}")),
                                _ => with_note(D::Seq(copies(n)), format!("{{{n}}}{suffix}")),
                            };
                        }
                    }
                    [lo, hi] if hi.is_empty() => {
                        if let Some(n) = number(lo) {
                            return match n {
                                0 => {
                                    let d = opt(rep(&base));
                                    if suffix.is_empty() { d } else { D::Choice(vec![D::Empty, D::Seq(vec![rep(&base), D::Comment(format!("{{0,}}{suffix}"))])]) }
                                }
                                1 => with_note(rep(&base), format!("{{1,}}{suffix}")),
                                _ => {
                                    let mut v = copies(n);
                                    v.push(D::Optional(Box::new(rep(&base))));
                                    with_note(D::Seq(v), format!("{{{n},}}{suffix}"))
                                }
                            };
                        }
                    }
                    [lo, hi] => {
                        if let (Some(n), Some(m)) = (number(lo), number(hi)) {
                            return if n == 0 && m == 1 {
                                if suffix.is_empty() { opt(base.clone()) } else { D::Choice(vec![D::Empty, base.clone(), D::Comment(format!("{{0,1}}{suffix}"))]) }
                            } else if n == 0 {
                                let mut v = vec![base.clone()];
                                v.extend((1..m).map(|_| opt(base.clone())));
                                with_note(opt(D::Seq(v)), format!("{{{n},{m}}}{suffix}"))
                            } else {
                                let mut v = copies(n);
                                v.extend((n..m).map(|_| opt(base.clone())));
                                with_note(D::Seq(v), format!("{{{n},{m}}}{suffix}"))
                            };
                        }
                    }
                    _ => {}
                }
                D::Seq(vec![base, D::Comment(format!("{{{q}}}{suffix}"))])
            }
            _ => base,
        }
    }

    /// "?" or "+" after a repeat sign: the note for a lazy or possessive repeat.
    fn lazy_or_possessive(&mut self, sign: &str) -> Option<String> {
        if self.more() && self.peek() == '?' {
            self.advance();
            Some(format!("{sign}? (lazy)"))
        } else if self.more() && self.peek() == '+' {
            self.advance();
            Some(format!("{sign}+ (possessive)"))
        } else {
            None
        }
    }
}

impl D {
    /// `<!, {base*!, `note`}>`: the lazy and possessive zero-or-more shape (the repeat and its note in one sequence).
    fn fix_choice(self, _base: &D, rep: &D, note: &str) -> D {
        D::Choice(vec![D::Empty, D::Seq(vec![rep.clone(), D::Comment(note.to_string())])])
    }
}

fn class_escape(c: char) -> D {
    let t = match c {
        'd' => "[digit]",
        'D' => "[non-digit]",
        's' => "[whitespace]",
        'S' => "[non-whitespace]",
        'w' => "[word character]",
        'W' => "[non-word character]",
        'h' => "[horizontal whitespace]",
        'H' => "[non-horizontal whitespace]",
        'v' => "[vertical whitespace]",
        'V' => "[non-vertical whitespace]",
        'n' => "[line feed]",
        'r' => "[carriage return]",
        't' => "[tab]",
        'f' => "[form feed]",
        'a' => "[bell]",
        'e' => "[escape]",
        other => return D::Term(other.to_string()),
    };
    term(t)
}

fn build(d: &D) -> Box<dyn Node> {
    match d {
        D::Empty => Box::new(Empty),
        D::Term(t) => Box::new(Terminal::new(t.clone())),
        D::NonTerm(t) => Box::new(NonTerminal::new(t.clone())),
        D::Comment(t) => Box::new(Comment::new(t.clone())),
        D::Seq(v) => Box::new(Sequence::new(v.iter().map(build).collect())),
        D::Choice(v) => Box::new(Choice::new(v.iter().map(build).collect())),
        D::Repeat(inner, sep) => Box::new(Repeat::new(build(inner), build(sep))),
        D::Optional(inner) => Box::new(Optional::new(build(inner))),
        D::Boxed(inner, label) => Box::new(LabeledBox::new(build(inner), Comment::new(label.clone()))),
    }
}

/// The page iTerm2 shows in its popover: the diagram's SVG alone in a body with no margin, or the
/// sentence it shows for an expression it cannot read.
pub fn page(regex: &str, dark: bool) -> String {
    let head = "<html>\n<head>\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"/>\n  <meta charset=\"utf-8\"/>\n</head>\n";
    let background = if dark { "background:#1e1e1e;color:#e0e0e0;" } else { "" };
    if fancy_regex::Regex::new(&crate::smartsel::to_rust_regex(regex)).is_err() {
        return format!("{head}<body style=\"margin:0;padding:0;{background}\">\nThe regular expression could not be parsed.\n</body>\n</html>\n");
    }
    let root: Sequence<Box<dyn Node>> = Sequence::new(vec![Box::new(Start), build(&convert(regex)), Box::new(End)]);
    let sheet = if dark { Stylesheet::Light.to_dark() } else { Stylesheet::Light };
    let diagram = Diagram::new_with_stylesheet(root, &sheet);
    format!("{head}<body style=\"margin:0;padding:0;{background}\">\n{diagram}\n</body>\n</html>\n")
}

/// The size the popover asks for, points: the SVG's own width and height with a small margin
/// (iTerm2 estimates it in a hidden web view; the SVG states it), or iTerm2's 400 by 100 for the
/// sentence about an expression it cannot read (`SVGViewController.swift`).
pub fn page_size(html: &str) -> (f64, f64) {
    let attr = |name: &str| {
        let start = html.find("<svg")?;
        let tag = &html[start..html[start..].find('>').map_or(html.len(), |e| start + e)];
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let end = tag[at..].find('"')?;
        tag[at..at + end].trim_end_matches("px").parse::<f64>().ok()
    };
    match (attr("width"), attr("height")) {
        (Some(w), Some(h)) => (w.ceil() + 16.0, h.ceil() + 16.0),
        _ => (400.0, 100.0),
    }
}

/// Writes the page for `regex` to the temp folder and opens it in the system browser (a judged
/// local file, opened through `localfs::open_with_shell`). The file is the same one each time, so
/// none piles up. The error is the reason it did not open.
pub fn open(regex: &str, dark: bool) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("termurion-regex-{}.html", std::process::id()));
    let local = crate::localfs::judge(&path).ok_or("The temp folder is not a local folder.")?;
    crate::localfs::write(&local, page(regex, dark)).map_err(|e| e.to_string())?;
    crate::localfs::open_with_shell(&local, None, false).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> D {
        D::Term(s.into())
    }

    #[test]
    fn literals_join_and_a_quantifier_takes_the_last_one() {
        assert_eq!(convert("abc"), t("abc"));
        assert_eq!(convert("ab+"), D::Seq(vec![t("a"), D::Repeat(Box::new(t("b")), Box::new(D::Empty))]));
        assert_eq!(convert(""), D::Empty);
    }

    #[test]
    fn quantifiers_follow_the_converter() {
        let rep = D::Repeat(Box::new(t("a")), Box::new(D::Empty));
        assert_eq!(convert("a*"), D::Choice(vec![D::Empty, rep.clone()]));
        assert_eq!(convert("a?"), D::Choice(vec![D::Empty, t("a")]));
        assert_eq!(convert("a+?"), D::Seq(vec![rep.clone(), D::Comment("+? (lazy)".into())]));
        assert_eq!(convert("a*?"), D::Choice(vec![D::Empty, D::Seq(vec![rep.clone(), D::Comment("*? (lazy)".into())])]));
        assert_eq!(convert("a{2}"), D::Seq(vec![t("a"), t("a")]));
        assert_eq!(convert("a{0}"), D::Empty);
        assert_eq!(convert("a{1,2}"), D::Seq(vec![t("a"), D::Choice(vec![D::Empty, t("a")])]));
        assert_eq!(convert("a{2,}"), D::Seq(vec![t("a"), t("a"), D::Optional(Box::new(rep))]));
        assert_eq!(convert("a{x}"), D::Seq(vec![t("a"), D::Comment("{x}".into())]));
    }

    #[test]
    fn groups_classes_and_escapes() {
        assert_eq!(convert("a|b"), D::Choice(vec![t("a"), t("b")]));
        assert_eq!(convert("(?<n>a)"), D::Boxed(Box::new(t("a")), "n".into()));
        assert_eq!(convert("(?=a)"), D::Boxed(Box::new(t("a")), "lookahead".into()));
        assert_eq!(convert("[^a-z]"), D::Seq(vec![D::Comment("not".into()), D::Choice(vec![t("[a-z]")])]));
        assert_eq!(convert("[[:alpha:]\\d]"), D::Choice(vec![t("[alpha]"), t("[digit]")]));
        assert_eq!(convert("\\d"), t("[digit]"));
        assert_eq!(convert("\\1"), D::NonTerm("group 1".into()));
        assert_eq!(convert("^$"), D::Seq(vec![D::Comment("start of line".into()), D::Comment("end of line".into())]));
        assert_eq!(convert("\\x{1F600}"), t("[U+1F600]"));
        assert_eq!(convert("\\Qa.b\\E"), t("a.b"));
    }

    #[test]
    fn a_broken_pattern_still_converts() {
        for p in ["(abc", "[abc", "a)b", "*a", "a{", "\\", "(?", "(?<", "(?i"] {
            let _ = convert(p);
        }
    }

    #[test]
    fn the_popover_is_as_large_as_the_diagram() {
        assert_eq!(page_size("<body><svg width=\"120\" height=\"40.5\" x=\"1\"></svg>"), (136.0, 57.0));
        assert_eq!(page_size(&page("(", false)), (400.0, 100.0));
        let (w, h) = page_size(&page(r"\d+", false));
        assert!(w > 16.0 && h > 16.0, "{w} {h}");
    }

    #[test]
    fn the_page_holds_the_diagram_or_the_sentence() {
        let ok = page(r"\d+-(a|b)", false);
        assert!(ok.contains("<svg") && ok.contains("[digit]"), "{ok}");
        assert!(page("(", false).contains("The regular expression could not be parsed."));
        assert!(page("a", true).contains("background:#1e1e1e"));
    }
}
