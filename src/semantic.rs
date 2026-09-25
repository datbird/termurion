//! iTerm2's Semantic History: what a Cmd-click (Win-click here) opens. The order is
//! iTerm2's: an OSC 8 link, an existing file (with a line and column when the text says
//! one), a Smart Selection rule's action, the raw command, then URL-like text.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Link(String),
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
    /// A Smart Selection rule's first action: rule index, action index, matched text.
    Action { rule: usize, action: usize, text: String },
    Url(String),
    RawCommand(String),
}

/// What is around a click. `before` ends at the clicked character; `after` starts at it.
#[derive(Clone, Debug, Default)]
pub struct ClickContext {
    pub before: String,
    pub after: String,
    /// The OSC 8 link on the clicked cell.
    pub link: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The Smart Selection rule with actions that matched at the click, if any.
    pub smart: Option<(usize, usize, String)>,
    /// Semantic History is set to "raw command".
    pub raw_command: bool,
}

/// iTerm2 looks this far each way from the click.
pub const MAX_CONTEXT: usize = 2000;

pub fn resolve(ctx: &ClickContext) -> Option<Target> {
    if let Some(link) = &ctx.link {
        return Some(Target::Link(link.clone()));
    }
    if let Some((path, line, column)) = find_path(&ctx.before, &ctx.after, ctx.cwd.as_deref()) {
        return Some(Target::File { path, line, column });
    }
    if let Some((rule, action, text)) = &ctx.smart {
        return Some(Target::Action { rule: *rule, action: *action, text: text.clone() });
    }
    if ctx.raw_command {
        let word = word_at(&ctx.before, &ctx.after);
        if !word.is_empty() {
            return Some(Target::RawCommand(word));
        }
    }
    url_like(&ctx.before, &ctx.after).map(Target::Url)
}

/// iTerm2 splits the text around a click into chunks at these characters, and tries
/// paths made of up to this many chunks each way.
const SEPARATORS: &[char] = &['\t', ' ', '(', ')', ':', '"', ','];
const MAX_CHUNKS: usize = 10;

/// The chunk under the click.
fn word_at(before: &str, after: &str) -> String {
    let start = before.rfind(SEPARATORS).map_or(0, |i| i + 1);
    let end = after.find(SEPARATORS).unwrap_or(after.len());
    format!("{}{}", &before[start..], &after[..end])
}

/// Where candidate paths may start in `before` (nearest first) and end in `after`.
fn chunk_bounds(before: &str, after: &str) -> (Vec<usize>, Vec<usize>) {
    let mut starts: Vec<usize> = before.char_indices().rev().filter(|(_, c)| SEPARATORS.contains(c)).map(|(i, c)| i + c.len_utf8()).collect();
    starts.push(0);
    starts.dedup();
    starts.truncate(MAX_CHUNKS + 1);
    let mut ends: Vec<usize> = after.char_indices().filter(|(_, c)| SEPARATORS.contains(c)).map(|(i, _)| i).collect();
    ends.push(after.len());
    ends.dedup();
    ends.truncate(MAX_CHUNKS);
    (starts, ends)
}

/// Trims what cannot end or start a path: quotes, brackets, and trailing punctuation.
/// Returns the kept byte range of `s`.
fn clean_range(s: &str) -> (usize, usize) {
    let mut l = 0;
    let mut r = s.len();
    loop {
        let t = &s[l..r];
        if let Some(c) = t.chars().next().filter(|c| c.is_whitespace() || "[<'`".contains(*c)) {
            l += c.len_utf8();
        } else if t.ends_with("...") {
            r -= 3;
        } else if let Some(c) = t.chars().last().filter(|c| c.is_whitespace() || "!?.,;:]>'`\u{2026}".contains(*c)) {
            r -= c.len_utf8();
        } else {
            return (l, r);
        }
    }
}

fn line_patterns() -> &'static [regex::Regex] {
    static P: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        [r"^:(\d+)(?::(\d+))?", r"^\((\d+)(?:,\s*(\d+))?\)", r"^\[(\d+)(?:,\s*(\d+))?\]", r#"^"?, line (\d+)"#]
            .iter()
            .map(|p| regex::Regex::new(p).expect("line pattern"))
            .collect()
    })
}

/// The line and column that `rest` (the text right after a path) starts with.
fn line_and_column(rest: &str) -> (Option<u32>, Option<u32>) {
    for re in line_patterns() {
        if let Some(c) = re.captures(rest) {
            let n = |i: usize| c.get(i).and_then(|m| m.as_str().parse().ok());
            return (n(1), n(2));
        }
    }
    (None, None)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// The existing file or folder `text` names, relative to `cwd`.
fn existing(text: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let text = text.replace("\\ ", " ");
    let mut tries = vec![text.clone()];
    for prefix in ["a/", "b/", "i/", "w/", "c/", "o/"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            tries.push(rest.to_string());
        }
    }
    tries.into_iter().find_map(|t| {
        let p = match t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
            Some(rest) => home()?.join(rest),
            None if Path::new(&t).is_absolute() => PathBuf::from(&t),
            None => cwd?.join(&t),
        };
        p.exists().then_some(p)
    })
}

/// An existing file or folder around the click, with a line and column when the text
/// after it gives them.
pub fn find_path(before: &str, after: &str, cwd: Option<&Path>) -> Option<(PathBuf, Option<u32>, Option<u32>)> {
    let (starts, ends) = chunk_bounds(before, after);
    // The longest candidate that exists wins, so a path with spaces beats its last word.
    let mut best: Option<(usize, PathBuf, String)> = None;
    for &s in &starts {
        for &e in &ends {
            let joined = format!("{}{}", &before[s..], &after[..e]);
            let (l, r) = clean_range(&joined);
            if l >= r || best.as_ref().is_some_and(|(len, _, _)| *len >= r - l) {
                continue;
            }
            if let Some(p) = existing(&joined[l..r], cwd) {
                // The text after the kept path, for its line and column.
                let rest = format!("{}{}", &joined[r..], &after[e..]);
                best = Some((r - l, p, rest));
            }
        }
    }
    best.map(|(_, p, rest)| {
        let (line, column) = line_and_column(&rest);
        (p, line, column)
    })
}

/// URL-like text around the click, made into a URL.
pub fn url_like(before: &str, after: &str) -> Option<String> {
    let url_char = |c: char| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c);
    let start = before.char_indices().rev().find(|(_, c)| !url_char(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let end = after.char_indices().find(|(_, c)| !url_char(*c)).map_or(after.len(), |(i, _)| i);
    let mut s = format!("{}{}", &before[start..], &after[..end]);
    loop {
        let unbalanced = |open: char, close: char, s: &str| s.matches(close).count() > s.matches(open).count();
        if s.starts_with(['(', '[', '\'']) && !s.ends_with([')', ']', '\'']) {
            s.remove(0);
        } else if s.ends_with(['.', ',', ';', ':', '!', '?', '\''])
            || s.ends_with(')') && unbalanced('(', ')', &s)
            || s.ends_with(']') && unbalanced('[', ']', &s)
        {
            s.pop();
        } else {
            break;
        }
    }
    if let Some(i) = s.find("://") {
        let scheme_start = s[..i].rfind(|c: char| !(c.is_ascii_alphanumeric() || "+.-".contains(c))).map_or(0, |j| j + 1);
        let scheme = &s[scheme_start..i];
        return (!scheme.is_empty() && s.len() > i + 3).then(|| s[scheme_start..].to_string());
    }
    if s.starts_with("mailto:") && s.len() > 7 {
        return Some(s);
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    let labels: Vec<&str> = host.split('.').collect();
    let domain = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()));
    domain.then(|| format!("https://{s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder with `files` in it (a trailing slash makes a folder).
    fn dir(name: &str, files: &[&str]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tandem-sem-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in files {
            let p = d.join(f);
            if f.ends_with('/') {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "x").unwrap();
            }
        }
        d
    }

    /// Splits `text` at the `|` that marks the click.
    fn click(text: &str) -> (String, String) {
        let (a, b) = text.split_once('|').unwrap();
        (a.to_string(), b.to_string())
    }

    #[test]
    fn finds_a_path_with_line_and_column() {
        let d = dir("linecol", &["src/main.rs"]);
        let (b, a) = click("error at src/ma|in.rs:12:5: oops");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("src/main.rs"), Some(12), Some(5))));
        let (b, a) = click("src\\ma|in.rs(7,3): error C2065");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("src\\main.rs"), Some(7), Some(3))));
        let (b, a) = click("src/main.r|s");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("src/main.rs"), None, None)));
    }

    #[test]
    fn finds_an_absolute_path_and_one_with_spaces() {
        let d = dir("abs", &["my file.txt"]);
        let full = d.join("my file.txt").to_string_lossy().to_string();
        let at = full.len() - 3;
        let (b, a) = (format!("see {}", &full[..at]), format!("{} now", &full[at..]));
        assert_eq!(find_path(&b, &a, None), Some((d.join("my file.txt"), None, None)));
    }

    #[test]
    fn finds_a_python_traceback_path() {
        let d = dir("py", &["x.py"]);
        let (b, a) = click("  File \"x|.py\", line 3, in f");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("x.py"), Some(3), None)));
    }

    #[test]
    fn strips_git_diff_prefixes() {
        let d = dir("git", &["lib/x.rs"]);
        let (b, a) = click("+++ b/lib/|x.rs");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("lib/x.rs"), None, None)));
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_path() {
        let d = dir("punct", &["notes.md", "docs/"]);
        let (b, a) = click("read no|tes.md.");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("notes.md"), None, None)));
        let (b, a) = click("see (not|es.md)!");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("notes.md"), None, None)));
        let (b, a) = click("in do|cs, see");
        assert_eq!(find_path(&b, &a, Some(&d)), Some((d.join("docs"), None, None)));
        assert_eq!(find_path("nothing he", "re", Some(&d)), None);
    }

    #[test]
    fn url_like_text_becomes_a_url() {
        let (b, a) = click("go to https://exa|mple.com/a?b=1.");
        assert_eq!(url_like(&b, &a).as_deref(), Some("https://example.com/a?b=1"));
        let (b, a) = click("(see www.exam|ple.com/x)");
        assert_eq!(url_like(&b, &a).as_deref(), Some("https://www.example.com/x"));
        let (b, a) = click("mail mailto:a@|b.com");
        assert_eq!(url_like(&b, &a).as_deref(), Some("mailto:a@b.com"));
        let (b, a) = click("just a wo|rd");
        assert_eq!(url_like(&b, &a), None);
        let (b, a) = click("version 1.2|.3");
        assert_eq!(url_like(&b, &a), None);
    }

    #[test]
    fn resolution_follows_iterm2s_order() {
        let d = dir("order", &["example.com"]);
        let (before, after) = click("exam|ple.com");
        let mut ctx = ClickContext { before, after, cwd: Some(d.clone()), ..Default::default() };
        ctx.link = Some("https://linked/".into());
        ctx.smart = Some((1, 0, "example.com".into()));
        ctx.raw_command = true;
        assert_eq!(resolve(&ctx), Some(Target::Link("https://linked/".into())));
        ctx.link = None;
        assert!(matches!(resolve(&ctx), Some(Target::File { .. })));
        ctx.cwd = None;
        assert_eq!(resolve(&ctx), Some(Target::Action { rule: 1, action: 0, text: "example.com".into() }));
        ctx.smart = None;
        assert_eq!(resolve(&ctx), Some(Target::RawCommand("example.com".into())));
        ctx.raw_command = false;
        assert_eq!(resolve(&ctx), Some(Target::Url("https://example.com".into())));
        ctx.before = "plain ".into();
        ctx.after = "".into();
        assert_eq!(resolve(&ctx), None);
    }
}
