//! iTerm2's Autocomplete (Autocomplete.m, PopupModel.m): the word before the cursor
//! completed from the screen and scrollback (searched backwards), the paste history, and
//! the shell history, each candidate scored by its context and how far back it was found.

/// Context words gathered on each side (iTerm2's kMaxQueryContextWords, kMaxResultContextWords).
const MAX_CONTEXT: usize = 4;

/// What is being completed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub prefix: String,
    /// The words before the prefix, nearest first.
    pub context: Vec<String>,
    /// The cursor follows whitespace: the last whole word is the prefix, and candidates are
    /// the words that come after it.
    pub whitespace_before: bool,
}

/// A candidate: what is inserted, what the list shows before it, and its score.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub value: String,
    pub prefix: String,
    pub score: f64,
    /// Each further hit adds its score times this, which shrinks 0.8 each time.
    mult: f64,
}

/// The run of one kind of character at `col`: word characters, whitespace, or a single
/// other character (iTerm2's rangeForWordAt). Past the line's end: an empty range there.
fn word_range(line: &[char], col: usize, word: &dyn Fn(char) -> bool) -> (usize, usize) {
    let Some(&c) = line.get(col) else { return (col, col) };
    let class = |c: char| if word(c) { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let k = class(c);
    if k == 2 {
        return (col, col + 1);
    }
    let mut a = col;
    while a > 0 && class(line[a - 1]) == k {
        a -= 1;
    }
    let mut b = col + 1;
    while b < line.len() && class(line[b]) == k {
        b += 1;
    }
    (a, b)
}

fn text(line: &[char], a: usize, b: usize) -> String {
    line[a.min(line.len())..b.min(line.len())].iter().collect()
}

fn blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

/// Up to `max` non-blank words before (`row`, `col`), nearest first (iTerm2's
/// contextWordsStartingAt:maxWords:).
fn context_before(lines: &[Vec<char>], row: usize, col: usize, max: usize, word: &dyn Fn(char) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let (mut r, mut c) = (row, col);
    for _ in 0..max * 2 {
        if out.len() >= max {
            break;
        }
        if c == 0 {
            if r == 0 {
                break;
            }
            r -= 1;
            c = lines[r].len();
            if c == 0 {
                continue;
            }
        }
        c -= 1;
        let (a, b) = word_range(&lines[r], c, word);
        let s = text(&lines[r], a, b);
        if !blank(&s) {
            out.push(s);
        }
        c = a;
    }
    out
}

/// The query at the cursor (`row`, `col`: the cell the cursor is on).
pub fn query_at(lines: &[Vec<char>], row: usize, col: usize, word: &dyn Fn(char) -> bool) -> Query {
    let Some(line) = lines.get(row) else { return Query::default() };
    if col == 0 {
        return Query::default();
    }
    // The character left of the cursor.
    let at = col - 1;
    let whitespace_before = line.get(at).is_none_or(|c| c.is_whitespace());
    let (start, _) = word_range(line, at, word);
    let s = text(line, start, at + 1);
    if !blank(&s) {
        let context = context_before(lines, row, start, MAX_CONTEXT, word);
        return Query { prefix: s, context, whitespace_before };
    }
    // Only whitespace: the word before it is the prefix.
    let mut context = context_before(lines, row, start.min(line.len()), MAX_CONTEXT + 1, word);
    let prefix = if context.is_empty() { String::new() } else { context.remove(0) };
    Query { prefix, context, whitespace_before }
}

/// iTerm2's contextSimilarityBetweenQuery:andResult:.
pub fn similarity(query: &[String], result: &[String]) -> f64 {
    let mut scratch: Vec<String> = result.to_vec();
    let mut sim = 0.0;
    for (i, qs) in query.iter().enumerate() {
        let length_factor = if qs.chars().count() == 1 { 5.0 } else { 1.0 };
        for (j, rs) in scratch.iter_mut().enumerate() {
            let distance = ((i as f64 - j as f64).abs() + 1.0) * length_factor;
            if qs == rs {
                sim += 1.0 / distance;
                rs.clear();
                break;
            } else if !rs.is_empty() && qs.to_lowercase() == rs.to_lowercase() {
                sim += 0.9 / distance;
                rs.clear();
                break;
            }
        }
    }
    sim * 1.5
}

/// iTerm2's scoreResultNumber:queryContext:resultContext:joiningPrefixLength:word:.
pub fn score(n: usize, query: &[String], result: &[String], joining: usize, word: &str) -> f64 {
    let sim = similarity(query, result) * 2.0;
    let mut score = (1.0 + sim * sim) / (n as f64 + 3.0);
    // Very short candidates are hardly worth completing, unless the context is strong.
    let length = (word.chars().count() + joining) as f64;
    if length < 4.0 && sim < 2.0 {
        score *= length / 50.0;
    }
    // Suffixes beat whole next words.
    if joining == 0 {
        score /= 2.0;
    }
    score
}

/// The candidates, at most `max` (a new one past it pushes out the lowest).
#[derive(Clone, Debug)]
pub struct Model {
    pub max: usize,
    entries: Vec<Candidate>,
}

impl Model {
    pub fn new(max: usize) -> Model {
        Model { max, entries: Vec::new() }
    }

    /// iTerm2's addHit:.
    pub fn add(&mut self, value: &str, prefix: &str, score: f64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.value == value) {
            e.mult *= 0.8;
            e.score += score * e.mult;
            return;
        }
        if self.entries.len() >= self.max {
            if let Some(i) = self.entries.iter().enumerate().min_by(|a, b| a.1.score.total_cmp(&b.1.score)).map(|(i, _)| i) {
                self.entries.remove(i);
            }
        }
        self.entries.push(Candidate { value: value.to_string(), prefix: prefix.to_string(), score, mult: 1.0 });
    }

    /// Best first.
    pub fn sorted(&self) -> Vec<Candidate> {
        let mut v = self.entries.clone();
        v.sort_by(|a, b| b.score.total_cmp(&a.score));
        v
    }
}

/// Candidates from the screen and scrollback, searched backwards from the prefix.
pub fn from_screen(lines: &[Vec<char>], row: usize, col: usize, q: &Query, word: &dyn Fn(char) -> bool, m: &mut Model) {
    let prefix: Vec<char> = q.prefix.chars().collect();
    if prefix.is_empty() {
        return;
    }
    let plen = prefix.len();
    let shown = if q.whitespace_before { format!("{} ", q.prefix) } else { q.prefix.clone() };
    let same = |a: char, b: char| a.to_lowercase().eq(b.to_lowercase());
    // The search starts where the prefix starts, going back.
    let Some(line) = lines.get(row) else { return };
    let limit = if col == 0 { 0 } else { word_range(line, col - 1, word).0 };
    let mut n = 0;
    for r in (0..=row).rev() {
        let l = &lines[r];
        let end = if r == row { limit } else { l.len() };
        for p in (0..end.min(l.len())).rev() {
            if p + plen > l.len() || !(0..plen).all(|k| same(l[p + k], prefix[k])) {
                continue;
            }
            // The word the match starts; it must start with the prefix.
            let (a, mut b) = word_range(l, p, word);
            if a != p {
                continue;
            }
            while b - a < plen && b < l.len() {
                b = word_range(l, b, word).1;
            }
            let found: Vec<char> = l[a..b].to_vec();
            if found.len() < plen || !(0..plen).all(|k| same(found[k], prefix[k])) {
                continue;
            }
            let full = found.len() == plen;
            let result_ctx = context_before(lines, r, p, MAX_CONTEXT, word);
            let candidate = if full {
                // The word after the match (the one after any whitespace).
                let after = p + plen;
                if after >= l.len() {
                    String::new()
                } else {
                    let (x, y) = word_range(l, after, word);
                    let w1 = text(l, x, y);
                    if blank(&w1) {
                        if y < l.len() {
                            let (x2, y2) = word_range(l, y, word);
                            let w2 = text(l, x2, y2);
                            if q.whitespace_before { w2 } else { format!(" {w2}") }
                        } else {
                            String::new()
                        }
                    } else {
                        w1
                    }
                }
            } else if !q.whitespace_before {
                found[plen..].iter().collect()
            } else {
                String::new()
            };
            if blank(&candidate) {
                continue;
            }
            let joining = if full { 0 } else { plen };
            m.add(&candidate, &shown, score(n, &q.context, &result_ctx, joining, &candidate));
            n += 1;
        }
    }
}

/// Candidates from the paste history: (value, age in seconds).
pub fn from_pastes(entries: &[(String, f64)], q: &Query, word: &dyn Fn(char) -> bool, m: &mut Model) {
    let prefix: Vec<char> = q.prefix.to_lowercase().chars().collect();
    if prefix.is_empty() {
        return;
    }
    let shown = if q.whitespace_before { format!("{} ", q.prefix) } else { q.prefix.clone() };
    for (value, age) in entries {
        let chars: Vec<char> = value.chars().collect();
        let lower: Vec<char> = value.to_lowercase().chars().collect();
        if lower.len() != chars.len() {
            continue;
        }
        let Some(at) = (0..lower.len()).find(|&i| lower[i..].starts_with(&prefix)) else { continue };
        // Only where a word starts.
        if at > 0 && word(chars[at - 1]) {
            continue;
        }
        let rest: String = chars[at + prefix.len()..].iter().collect();
        let n = (age.max(0.0) / 60.0).sqrt().min(20.0) as usize;
        m.add(&rest, &shown, score(n, &q.context, &[], prefix.len(), &rest));
    }
}

/// Candidates from the shell history: (command, uses, seconds since its last use) whose
/// command starts with what is typed (`typed`, up to the cursor).
pub fn from_commands(entries: &[(String, usize, f64)], typed: &str, q: &Query, word: &dyn Fn(char) -> bool, m: &mut Model) {
    let shown = if q.whitespace_before { format!("{} ", q.prefix) } else { q.prefix.clone() };
    let plen = q.prefix.chars().count();
    let mut i = 0;
    for (command, uses, age) in entries {
        let Some(rest) = command.strip_prefix(typed) else { continue };
        let next: String = rest.chars().take_while(|c| word(*c)).collect();
        if next.is_empty() {
            continue;
        }
        // The whole prompt is in its favor: the query is its own result context.
        let mut s = score(i, &q.context, &q.context, plen, &next);
        i += 1;
        s *= (*uses as f64).sqrt();
        s /= (age / 86_400.0).sqrt().max(1.0);
        m.add(&next, &shown, s.min(10.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<Vec<char>> {
        text.iter().map(|l| l.chars().collect()).collect()
    }

    fn w(c: char) -> bool {
        c.is_alphanumeric() || "/-+\\~_.".contains(c)
    }

    fn values(m: &Model) -> Vec<String> {
        m.sorted().into_iter().map(|c| c.value).collect()
    }

    #[test]
    fn screen_words_complete_the_prefix() {
        let l = lines(&["git status", "git sta"]);
        let q = query_at(&l, 1, 7, &w);
        assert_eq!(q, Query { prefix: "sta".into(), context: vec!["git".into(), "status".into(), "git".into()], whitespace_before: false }, "context goes back across lines");
        let mut m = Model::new(20);
        from_screen(&l, 1, 7, &q, &w, &mut m);
        assert_eq!(values(&m), vec!["tus"], "a suffix of the word that starts with the prefix");
        let l = lines(&["git status", "git "]);
        let q = query_at(&l, 1, 4, &w);
        assert_eq!((q.prefix.as_str(), q.whitespace_before), ("git", true));
        let mut m = Model::new(20);
        from_screen(&l, 1, 4, &q, &w, &mut m);
        assert_eq!(values(&m), vec!["status"], "after a space: the word that followed");
        assert_eq!(m.sorted()[0].prefix, "git ");
        let l = lines(&["gitk", "echo gi"]);
        let q = query_at(&l, 1, 7, &w);
        let mut m = Model::new(20);
        from_screen(&l, 1, 7, &q, &w, &mut m);
        assert_eq!(values(&m), vec!["tk"]);
    }

    #[test]
    fn hits_add_with_decay() {
        let mut m = Model::new(20);
        for _ in 0..3 {
            m.add("a", "", 1.0);
        }
        assert!((m.sorted()[0].score - 2.44).abs() < 1e-9, "1 + 0.8 + 0.64");
        let mut m = Model::new(2);
        m.add("a", "", 1.0);
        m.add("b", "", 0.5);
        m.add("c", "", 3.0);
        assert_eq!(values(&m), vec!["c", "a"], "the lowest goes");
    }

    #[test]
    fn scores_follow_iterm2() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!((similarity(&s(&["git"]), &s(&["git"])) - 1.5).abs() < 1e-9);
        assert!((similarity(&s(&["Git"]), &s(&["git"])) - 1.35).abs() < 1e-9);
        assert!((similarity(&s(&["a"]), &s(&["a"])) - 0.3).abs() < 1e-9, "one-character words count less");
        assert!((similarity(&s(&["x", "git"]), &s(&["git"])) - 0.75).abs() < 1e-9, "distance 2");
        assert!((score(0, &s(&["git"]), &s(&["git"]), 3, "tus") - 10.0 / 3.0).abs() < 1e-9);
        assert!((score(0, &[], &[], 0, "ab") - (1.0 / 3.0) * (2.0 / 50.0) / 2.0).abs() < 1e-12, "short and a full word");
    }

    #[test]
    fn pastes_and_commands_add_candidates() {
        let q = Query { prefix: "git".into(), context: vec![], whitespace_before: false };
        let mut m = Model::new(20);
        from_pastes(&[("run git stash".into(), 0.0), ("legit".into(), 0.0)], &q, &w, &mut m);
        assert_eq!(values(&m), vec![" stash"], "only at a word start");
        let q = Query { prefix: "sta".into(), context: vec!["git".into()], whitespace_before: false };
        let mut m = Model::new(20);
        from_commands(&[("git status -s".into(), 4, 0.0), ("ls".into(), 9, 0.0)], "git sta", &q, &w, &mut m);
        assert_eq!(values(&m), vec!["tus"], "the next word of commands that start with what is typed");
        assert!(m.sorted()[0].score <= 10.0);
    }
}
