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

/// `MaxSemanticHistoryPrefixOrSuffix`'s default: iTerm2 looks this far each way from the click.
pub const DEFAULT_MAX_CONTEXT: usize = 2000;

/// What the hover banner shows for a target (`TVA:733-748`).
pub fn preview_text(target: &Target, osc8: bool, smart_open_url: Option<&str>, show_preview: bool) -> Option<String> {
    match target {
        Target::Link(url) => Some(url.clone()),
        Target::Url(url) if show_preview || osc8 => Some(url.clone()),
        Target::Action { .. } if show_preview => smart_open_url.map(str::to_string),
        _ => None,
    }
}

/// Like `resolve`, with the target's byte range in `before` + `after`. The range is None
/// for a link or a Smart Selection action, whose extent the caller knows.
pub fn resolve_range(ctx: &ClickContext) -> Option<(Target, Option<std::ops::Range<usize>>)> {
    if let Some(link) = &ctx.link {
        return Some((Target::Link(link.clone()), None));
    }
    if let Some((path, line, column, range)) = path_in(&ctx.before, &ctx.after, ctx.cwd.as_deref()) {
        return Some((Target::File { path, line, column }, Some(range)));
    }
    if let Some((rule, action, text)) = &ctx.smart {
        return Some((Target::Action { rule: *rule, action: *action, text: text.clone() }, None));
    }
    if ctx.raw_command {
        let range = word_range(&ctx.before, &ctx.after);
        if !range.is_empty() {
            let word = format!("{}{}", ctx.before, ctx.after)[range.clone()].to_string();
            return Some((Target::RawCommand(word), Some(range)));
        }
    }
    url_in(&ctx.before, &ctx.after).map(|(u, r)| (Target::Url(u), Some(r)))
}

pub fn resolve(ctx: &ClickContext) -> Option<Target> {
    resolve_range(ctx).map(|(t, _)| t)
}

/// iTerm2 splits the text around a click into chunks at these characters, and tries
/// paths made of up to this many chunks each way.
const SEPARATORS: &[char] = &['\t', ' ', '(', ')', ':', '"', ','];
const MAX_CHUNKS: usize = 10;

/// The chunk under the click, as a range of `before` + `after`.
fn word_range(before: &str, after: &str) -> std::ops::Range<usize> {
    let start = before.rfind(SEPARATORS).map_or(0, |i| i + 1);
    let end = after.find(SEPARATORS).unwrap_or(after.len());
    start..before.len() + end
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
    // A share name ("\\\\host\\x", "//host/x") or URL text would make Windows try the
    // network, which can take 20 s and hands the host the user's credentials.
    tries.retain(|t| !(t.starts_with("\\\\") || t.starts_with("//") || t.contains("://")));
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
    path_in(before, after, cwd).map(|(p, l, c, _)| (p, l, c))
}

type PathHit = (PathBuf, Option<u32>, Option<u32>, std::ops::Range<usize>);

fn path_in(before: &str, after: &str, cwd: Option<&Path>) -> Option<PathHit> {
    let (starts, ends) = chunk_bounds(before, after);
    // The longest candidate that exists wins, so a path with spaces beats its last word.
    let mut best: Option<(std::ops::Range<usize>, PathBuf, String)> = None;
    for &s in &starts {
        for &e in &ends {
            let joined = format!("{}{}", &before[s..], &after[..e]);
            let (l, r) = clean_range(&joined);
            if l >= r || best.as_ref().is_some_and(|(range, _, _)| range.len() >= r - l) {
                continue;
            }
            if let Some(p) = existing(&joined[l..r], cwd) {
                // The text after the kept path, for its line and column.
                let rest = format!("{}{}", &joined[r..], &after[e..]);
                best = Some((s + l..s + r, p, rest));
            }
        }
    }
    best.map(|(range, p, rest)| {
        let (line, column) = line_and_column(&rest);
        (p, line, column, range)
    })
}

/// URL-like text around the click, made into a URL.
pub fn url_like(before: &str, after: &str) -> Option<String> {
    url_in(before, after).map(|(u, _)| u)
}

fn url_in(before: &str, after: &str) -> Option<(String, std::ops::Range<usize>)> {
    let url_char = |c: char| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c);
    let mut start = before.char_indices().rev().find(|(_, c)| !url_char(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let end = after.char_indices().find(|(_, c)| !url_char(*c)).map_or(after.len(), |(i, _)| i);
    let mut s = format!("{}{}", &before[start..], &after[..end]);
    loop {
        let unbalanced = |open: char, close: char, s: &str| s.matches(close).count() > s.matches(open).count();
        let wrapped = [('(', ')'), ('[', ']'), ('\'', '\'')].iter().any(|(o, c)| s.len() > 1 && s.starts_with(*o) && s.ends_with(*c));
        if wrapped {
            s.remove(0);
            s.pop();
            start += 1;
        } else if s.starts_with(['(', '[', '\'']) && !s.ends_with([')', ']', '\'']) {
            s.remove(0);
            start += 1;
        } else if s.ends_with(['.', ',', ';', ':', '!', '?', '\''])
            || s.ends_with(')') && unbalanced('(', ')', &s)
            || s.ends_with(']') && unbalanced('[', ']', &s)
        {
            s.pop();
        } else {
            break;
        }
    }
    let range = |from: usize| start + from..start + s.len();
    if let Some(i) = s.find("://") {
        let scheme_start = s[..i].rfind(|c: char| !(c.is_ascii_alphanumeric() || "+.-".contains(c))).map_or(0, |j| j + 1);
        let scheme = &s[scheme_start..i];
        return (!scheme.is_empty() && s.len() > i + 3).then(|| (s[scheme_start..].to_string(), range(scheme_start)));
    }
    if s.starts_with("mailto:") && s.len() > 7 {
        return Some((s.clone(), range(0)));
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    let labels: Vec<&str> = host.split('.').collect();
    let domain = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()));
    domain.then(|| (format!("https://{s}"), range(0)))
}

/// A profile's Semantic History setting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    /// "best editor", "url", "editor", "command", "raw command", "coprocess", or "send text".
    pub action: String,
    /// The URL, command, or text for those actions.
    pub text: String,
    /// The editor for "editor".
    pub editor: String,
}

impl Prefs {
    pub fn from_value(v: Option<&toml::Value>) -> Prefs {
        let t = v.and_then(|v| v.as_table());
        let s = |k: &str| t.and_then(|t| t.get(k)).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let action = s("action");
        Prefs { action: if action.is_empty() { "best editor".into() } else { action }, text: s("text"), editor: s("editor") }
    }
}

/// What opening a target runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    /// Open a file, folder, or URL with its default app.
    Open(String),
    /// A command line for cmd.exe, run on its own.
    Command(String),
    /// Text typed into the session.
    Send(String),
    /// An action a later milestone brings: its name and the milestone.
    Later(&'static str, u8),
    /// A coprocess command for the session.
    Coprocess(String),
    /// Show a file in Explorer instead of running it.
    Reveal(String),
    /// Not opened, and why.
    Blocked(String),
}

/// The link schemes that open in the browser or mail app.
const WEB: &[&str] = &["http", "https", "mailto", "ftp"];

/// The lowercased scheme of a URL; None for a path.
fn scheme(target: &str) -> Option<String> {
    let s = target.split_once(':').map(|(s, _)| s).filter(|s| {
        // A drive letter ("C:") is a path, not a scheme.
        s.len() > 1 && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    })?;
    Some(s.to_ascii_lowercase())
}

/// The links remote text may open (P10): ftp is not one, since Explorer or a registered
/// client may take it, not only the browser.
const REMOTE_WEB: &[&str] = &["http", "https", "mailto"];

/// Whether `target` is a web or mail link remote text may open: guard_open hands it to the
/// browser or mail app, never opens a path.
pub fn web_link(target: &str) -> bool {
    scheme(target).is_some_and(|s| REMOTE_WEB.contains(&s.as_str()))
}

/// What opening `target` (a URL or a path) runs. Only http, https, mailto, and ftp links
/// open; a file: link is a path; a program or script is shown, never run.
pub fn guard_open(target: &str, has_code: bool) -> Run {
    match scheme(target).as_deref() {
        None => file_open(target, None, None, has_code),
        Some(s) if WEB.contains(&s) => Run::Open(target.to_string()),
        Some("file") => match file_url_path(target) {
            Some(p) => file_open(&p, None, None, has_code),
            None => Run::Blocked(format!("Tandem does not open network file links: {target}")),
        },
        Some(s) => Run::Blocked(format!("Tandem opens only http, https, mailto, and ftp links, not {s}: links.")),
    }
}

/// Programs and scripts: "open" would run them, so Tandem shows them instead.
const RUNNABLE: &[&str] = &[
    "exe", "com", "bat", "cmd", "btm", "js", "jse", "vbs", "vbe", "vb", "wsf", "wsh", "ws", "hta", "msi", "msp", "msc",
    "lnk", "scr", "pif", "cpl", "reg", "url", "ps1", "psm1", "py", "pyw", "pyc", "pyz", "jar", "appref-ms",
    "application", "gadget", "inf", "scf", "sct", "chm", "library-ms", "settingcontent-ms", "appx", "msix",
    "appinstaller", "diagcab",
];

pub(crate) fn runnable(path: &str) -> bool {
    let Some(ext) = Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()) else { return false };
    RUNNABLE.contains(&ext.as_str())
        || std::env::var("PATHEXT").is_ok_and(|p| p.split(';').any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))
}

/// A local file: link as a Windows path. None for a link to another machine.
fn file_url_path(url: &str) -> Option<String> {
    let rest = &url[url.find(':')? + 1..];
    let rest = rest.strip_prefix("//").map_or(Some(rest), |r| {
        let (host, path) = r.split_at(r.find('/').unwrap_or(r.len()));
        (host.is_empty() || host.eq_ignore_ascii_case("localhost")).then_some(path)
    })?;
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&out).to_string();
    // "/C:/x" is "C:\x".
    let path = match path.strip_prefix('/') {
        Some(p) if p.as_bytes().get(1) == Some(&b':') => p.to_string(),
        _ => path,
    };
    (!path.starts_with("//")).then(|| path.replace('/', "\\"))
}

/// Opening a file: its default app, except a program or script, which VS Code opens when
/// it is on the PATH and Explorer shows otherwise.
fn file_open(p: &str, line: Option<u32>, column: Option<u32>, has_code: bool) -> Run {
    let goto = || {
        let mut at = p.to_string();
        if let Some(n) = line {
            at.push_str(&format!(":{n}"));
            if let Some(c) = column {
                at.push_str(&format!(":{c}"));
            }
        }
        cmd_quote(&at)
    };
    if has_code && (line.is_some() || runnable(p)) {
        Run::Command(format!("code -g {}", goto()))
    } else if runnable(p) {
        Run::Reveal(p.to_string())
    } else {
        Run::Open(p.to_string())
    }
}

/// The text around the click and the session's folder, for \\3, \\4, and \\5.
#[derive(Clone, Debug, Default)]
pub struct Around<'a> {
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub cwd: &'a str,
}

/// explorer.exe in the Windows folder, the parent of the System32 folder `system`
/// (`GetSystemDirectoryW`): run by its full path, never found by the search order.
pub fn explorer_in(system: &Path) -> Option<PathBuf> {
    system.parent().map(|windows| windows.join("explorer.exe"))
}

/// Quotes a value for cmd.exe.
pub fn cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Percent-encodes everything but letters, digits, and "-._~".
pub fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Fills iTerm2's \\1 (file), \\2 (line), \\3 (text before the click), \\4 (text after it),
/// and \\5 (working directory) into `template`, each passed through `each`.
pub fn fill(template: &str, values: [&str; 5], each: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some(d @ '1'..='5')) => {
                out.push_str(&each(values[*d as usize - '1' as usize]));
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// What a Win-click on `target` runs under `prefs`. `has_code` says VS Code is on the
/// PATH, which the best editor uses to go to a line. Smart Selection actions are the
/// caller's (they carry their own parameters).
pub fn plan(target: &Target, prefs: &Prefs, around: &Around, has_code: bool) -> Option<Run> {
    match target {
        Target::Link(u) | Target::Url(u) => Some(guard_open(u, has_code)),
        Target::Action { .. } => None,
        Target::RawCommand(word) => (prefs.action == "raw command")
            .then(|| Run::Command(fill(&prefs.text, [word, "", around.prefix, around.suffix, around.cwd], cmd_quote))),
        Target::File { path, line, column } => {
            let p = path.to_string_lossy().to_string();
            let l = line.map(|n| n.to_string()).unwrap_or_default();
            let values = [p.as_str(), l.as_str(), around.prefix, around.suffix, around.cwd];
            // VS Code's -g takes file:line:column.
            let goto = || {
                let mut at = p.clone();
                if let Some(n) = line {
                    at.push_str(&format!(":{n}"));
                    if let Some(c) = column {
                        at.push_str(&format!(":{c}"));
                    }
                }
                cmd_quote(&at)
            };
            Some(match prefs.action.as_str() {
                "url" => guard_open(&fill(&prefs.text, values, percent_encode), has_code),
                "editor" if prefs.editor.to_lowercase().contains("code") => Run::Command(format!("{} -g {}", cmd_quote(&prefs.editor), goto())),
                "editor" => Run::Command(format!("{} {}", cmd_quote(&prefs.editor), cmd_quote(&p))),
                "command" | "raw command" => Run::Command(fill(&prefs.text, values, cmd_quote)),
                "coprocess" => Run::Coprocess(fill(&prefs.text, values, cmd_quote)),
                "send text" => Run::Send(fill(&prefs.text, values, str::to_string)),
                _ => file_open(&p, *line, *column, has_code),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_runs_from_the_windows_folder() {
        // Final review Minor 4: by its full path, never Tandem's folder or PATH first.
        assert_eq!(explorer_in(Path::new(r"C:\Windows\System32")), Some(PathBuf::from(r"C:\Windows\explorer.exe")));
    }

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

    #[test]
    fn semantic_history_command_quotes_for_cmd() {
        let file = Target::File { path: PathBuf::from("C:\\a b\\x.rs"), line: Some(3), column: None };
        let around = Around { prefix: "at ", suffix: " now", cwd: "C:\\w" };
        let prefs = Prefs { action: "command".into(), text: "vim \\1 +\\2".into(), editor: String::new() };
        assert_eq!(plan(&file, &prefs, &around, false), Some(Run::Command("vim \"C:\\a b\\x.rs\" +\"3\"".into())));
        let raw = Prefs { action: "raw command".into(), text: "echo \\1 in \\5".into(), editor: String::new() };
        assert_eq!(plan(&Target::RawCommand("w\"x".into()), &raw, &around, false), Some(Run::Command("echo \"w\"\"x\" in \"C:\\w\"".into())));
        let best = Prefs::from_value(None);
        assert_eq!(best.action, "best editor");
        assert_eq!(plan(&file, &best, &around, false), Some(Run::Open("C:\\a b\\x.rs".into())));
        assert_eq!(plan(&file, &best, &around, true), Some(Run::Command("code -g \"C:\\a b\\x.rs:3\"".into())));
        let editor = Prefs { action: "editor".into(), text: String::new(), editor: "C:\\ed\\ed.exe".into() };
        assert_eq!(plan(&file, &editor, &around, false), Some(Run::Command("\"C:\\ed\\ed.exe\" \"C:\\a b\\x.rs\"".into())));
        let send = Prefs { action: "send text".into(), text: "open \\1\\n".into(), editor: String::new() };
        assert_eq!(plan(&file, &send, &around, false), Some(Run::Send("open C:\\a b\\x.rs\\n".into())));
        let co = Prefs { action: "coprocess".into(), text: "x".into(), editor: String::new() };
        assert_eq!(plan(&file, &co, &around, false), Some(Run::Coprocess("x".into())));
        assert_eq!(plan(&Target::Url("https://a/".into()), &co, &around, false), Some(Run::Open("https://a/".into())));
        assert_eq!(plan(&Target::Link("file:///c:/x".into()), &best, &around, false), Some(Run::Open("c:\\x".into())));
    }

    #[test]
    fn url_action_percent_encodes() {
        let file = Target::File { path: PathBuf::from("C:\\a b\\x.rs"), line: Some(3), column: Some(1) };
        let prefs = Prefs { action: "url".into(), text: "https://x/open?f=\\1&l=\\2".into(), editor: String::new() };
        assert_eq!(
            plan(&file, &prefs, &Around::default(), false),
            Some(Run::Open("https://x/open?f=C%3A%5Ca%20b%5Cx.rs&l=3".into()))
        );
        assert_eq!(percent_encode("a-._~/é"), "a-._~%2F%C3%A9");
    }

    #[test]
    fn hover_ranges_cover_the_target() {
        let d = dir("range", &["src/main.rs"]);
        let (before, after) = click("at src/ma|in.rs:3 and https://a.b/c. x");
        let ctx = ClickContext { before: before.clone(), after: after.clone(), cwd: Some(d), ..Default::default() };
        let text = format!("{before}{after}");
        let (t, r) = resolve_range(&ctx).unwrap();
        assert!(matches!(t, Target::File { line: Some(3), .. }));
        assert_eq!(&text[r.unwrap()], "src/main.rs");
        let (before, after) = click("at src/main.rs:3 and https://a.|b/c. x");
        let ctx = ClickContext { before: before.clone(), after: after.clone(), ..Default::default() };
        let text = format!("{before}{after}");
        let (t, r) = resolve_range(&ctx).unwrap();
        assert_eq!(t, Target::Url("https://a.b/c".into()));
        assert_eq!(&text[r.unwrap()], "https://a.b/c");
        let (before, after) = click("(www.ex|ample.com)");
        let (_, r) = resolve_range(&ClickContext { before: before.clone(), after: after.clone(), ..Default::default() }).unwrap();
        assert_eq!(&format!("{before}{after}")[r.unwrap()], "www.example.com");
    }

    #[test]
    fn unc_paths_are_never_probed() {
        // A dead address: probing it as a share takes about 21 s on Windows.
        let t = std::time::Instant::now();
        assert_eq!(find_path("see ", "\\\\10.255.255.1\\x\\y", None), None);
        assert_eq!(find_path("go ", "//10.255.255.1/x", None), None);
        let d = dir("unc", &[]);
        let (b, a) = click("at https://10.255.255.1/x|/y now");
        assert_eq!(find_path(&b, &a, Some(&d)), None);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn executables_are_shown_not_run() {
        let best = Prefs::from_value(None);
        for name in ["setup.py", "build.js", "x.EXE", "run.bat", "a.lnk", "b.ps1"] {
            let file = Target::File { path: PathBuf::from(format!("C:\\w\\{name}")), line: None, column: None };
            assert_eq!(plan(&file, &best, &Around::default(), false), Some(Run::Reveal(format!("C:\\w\\{name}"))), "{name}");
        }
        let script = Target::File { path: PathBuf::from("C:\\w\\setup.py"), line: Some(2), column: None };
        assert_eq!(plan(&script, &best, &Around::default(), true), Some(Run::Command("code -g \"C:\\w\\setup.py:2\"".into())));
        let text = Target::File { path: PathBuf::from("C:\\w\\notes.txt"), line: None, column: None };
        assert_eq!(plan(&text, &best, &Around::default(), false), Some(Run::Open("C:\\w\\notes.txt".into())));
    }

    #[test]
    fn links_open_only_safe_schemes() {
        assert_eq!(guard_open("https://docs.rs/x", false), Run::Open("https://docs.rs/x".into()));
        assert_eq!(guard_open("mailto:a@b.c", false), Run::Open("mailto:a@b.c".into()));
        assert!(matches!(guard_open("ms-msdt:/id x", false), Run::Blocked(_)));
        assert!(matches!(guard_open("search-ms:query=x", false), Run::Blocked(_)));
        assert_eq!(guard_open("file:///C:/Users/u/Downloads/x.exe", false), Run::Reveal("C:\\Users\\u\\Downloads\\x.exe".into()));
        assert_eq!(guard_open("file:///C:/docs/a%20b.txt", false), Run::Open("C:\\docs\\a b.txt".into()));
        assert_eq!(guard_open("C:\\tools\\x.bat", false), Run::Reveal("C:\\tools\\x.bat".into()));
        let best = Prefs::from_value(None);
        let link = Target::Link("file:///C:/x/evil.cmd".into());
        assert_eq!(plan(&link, &best, &Around::default(), false), Some(Run::Reveal("C:\\x\\evil.cmd".into())));
        assert!(matches!(plan(&Target::Url("javascript://x".into()), &best, &Around::default(), false), Some(Run::Blocked(_))));
    }
    #[test]
    fn url_preview_follows_iterm2() {
        // An OSC 8 link always shows its URL.
        assert_eq!(preview_text(&Target::Link("https://a/".into()), true, None, false), Some("https://a/".into()));
        assert_eq!(preview_text(&Target::Url("https://b".into()), false, None, true), Some("https://b".into()));
        assert_eq!(preview_text(&Target::Url("https://b".into()), false, None, false), None);
        let action = Target::Action { rule: 0, action: 0, text: "x".into() };
        assert_eq!(preview_text(&action, false, Some("https://c"), true), Some("https://c".into()));
        assert_eq!(preview_text(&action, false, Some("https://c"), false), None);
        assert_eq!(preview_text(&action, false, None, true), None, "a non Open URL action shows nothing");
        let file = Target::File { path: PathBuf::from("a.txt"), line: None, column: None };
        assert_eq!(preview_text(&file, false, None, true), None);
        assert_eq!(preview_text(&Target::RawCommand("x".into()), false, None, true), None);
    }
}
