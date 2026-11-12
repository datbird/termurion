//! FX profiles: the rules the cursor effects use to read an app's screen, as plain data.
//! A profile is one TOML file; a missing key takes the Shell value. This module only holds
//! and parses the data. The scene code reads it.

use crate::palette::{rgb, Rgb};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct FxRules {
    pub name: String,
    pub matching: Match,
    pub cursor: CursorRule,
    pub boxes: BoxRule,
    pub prompt_marks: Vec<char>,
    pub done: DoneRule,
}

/// How Auto detection recognizes the app. Any hit counts.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Match {
    pub program: Vec<String>,
    pub text: Vec<String>,
    pub prompt: Vec<char>,
    pub shape: Option<BoxKindRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Real,
    Painted,
    Either,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintedMark {
    Reverse,
    Background(Rgb),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CursorRule {
    pub kind: CursorKind,
    pub marks: Vec<PaintedMark>,
    pub echo_anchor: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxKindRule {
    Lines,
    Band,
    Either,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoxRule {
    pub kind: BoxKindRule,
    pub line_glyphs: Vec<char>,
    pub line_fill: f32,
    pub bottom_within: usize,
    pub band_rows: (usize, usize),
    pub band_max_screen: f32,
    pub band_min_width: f32,
    pub band_tolerance: usize,
    pub edge_glyphs: Vec<char>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DoneRule {
    pub enter: Vec<u8>,
    pub quiet: Duration,
    pub busy: Duration,
    pub osc133: bool,
}

/// The Shell rules: today's hard-coded values.
impl Default for FxRules {
    fn default() -> Self {
        FxRules {
            name: "Shell".to_string(),
            matching: Match::default(),
            cursor: CursorRule { kind: CursorKind::Either, marks: vec![PaintedMark::Reverse], echo_anchor: true },
            boxes: BoxRule {
                kind: BoxKindRule::Either,
                line_glyphs: vec!['\u{2500}', '\u{2501}'],
                line_fill: 0.8,
                bottom_within: 2,
                band_rows: (2, 12),
                band_max_screen: 0.5,
                band_min_width: 0.5,
                band_tolerance: 2,
                edge_glyphs: vec!['\u{2580}', '\u{2584}'],
            },
            prompt_marks: "\u{276f}>$%#\u{203a}".chars().collect(),
            done: DoneRule { enter: b"\r".to_vec(), quiet: Duration::from_millis(1200), busy: Duration::from_millis(1500), osc133: true },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: Option<usize>,
    pub message: String,
}

/// The six files Termurion ships: (file name, text).
pub const SHIPPED: &[(&str, &str)] = &[
    ("shell.toml", include_str!("../resources/fx_profiles/shell.toml")),
    ("claude-code.toml", include_str!("../resources/fx_profiles/claude-code.toml")),
    ("codex.toml", include_str!("../resources/fx_profiles/codex.toml")),
    ("opencode.toml", include_str!("../resources/fx_profiles/opencode.toml")),
    ("hermes.toml", include_str!("../resources/fx_profiles/hermes.toml")),
    ("agy.toml", include_str!("../resources/fx_profiles/agy.toml")),
];

const MAX_GLYPHS: usize = 32;
const MAX_TEXTS: usize = 16;
const MAX_TEXT_LEN: usize = 200;
const MAX_PROGRAMS: usize = 16;
const MAX_NAME: usize = 100;

const TOP_KEYS: &[&str] = &["name", "match", "cursor", "box", "prompt_mark", "done"];
const MATCH_KEYS: &[&str] = &["program", "text", "prompt", "shape"];
const CURSOR_KEYS: &[&str] = &["kind", "mark", "echo_anchor"];
const BOX_KEYS: &[&str] = &["kind", "line_glyphs", "line_fill", "bottom_within", "band_rows", "band_max_screen", "band_min_width", "band_tolerance", "edge_glyphs"];
const MARK_KEYS: &[&str] = &["glyphs"];
const DONE_KEYS: &[&str] = &["enter", "quiet_ms", "busy_ms", "osc133"];

/// Where a key sits in the source, for an error's line number: the first line in `section`
/// that starts with `key` and then `=` or a space.
fn line_of(text: &str, section: &str, key: &str) -> Option<usize> {
    let mut at = String::new();
    for (i, l) in text.lines().enumerate() {
        let t = l.trim();
        if t.starts_with('[') && !t.starts_with("[[") {
            at = t.trim_start_matches('[').split(']').next().unwrap_or("").trim().to_string();
            if key.is_empty() && at == section {
                return Some(i + 1);
            }
        } else if at == section && !key.is_empty() {
            if let Some(rest) = t.strip_prefix(key) {
                if rest.starts_with(['=', ' ', '\t']) {
                    return Some(i + 1);
                }
            }
        }
    }
    None
}

struct Reader<'a> {
    text: &'a str,
    warnings: Vec<String>,
}

impl Reader<'_> {
    fn err(&self, section: &str, key: &str, msg: &str) -> ParseError {
        let shown = if section.is_empty() { key.to_string() } else { format!("{section}.{key}") };
        ParseError { line: line_of(self.text, section, key), message: format!("`{shown}`: {msg}") }
    }

    /// A section's table (a missing one is empty), its unknown keys warned.
    fn section<'t>(&mut self, top: &'t toml::Table, name: &str, known: &[&str]) -> Result<Option<&'t toml::Table>, ParseError> {
        let Some(v) = top.get(name) else { return Ok(None) };
        let Some(t) = v.as_table() else {
            return Err(ParseError { line: line_of(self.text, "", name), message: format!("`{name}` must be a table") });
        };
        for k in t.keys().filter(|k| !known.contains(&k.as_str())) {
            self.warnings.push(format!("unknown key `{name}.{k}`"));
        }
        Ok(Some(t))
    }

    fn string(&self, t: Option<&toml::Table>, section: &str, key: &str) -> Result<Option<String>, ParseError> {
        match t.and_then(|t| t.get(key)) {
            None => Ok(None),
            Some(toml::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(self.err(section, key, "must be a string")),
        }
    }

    fn glyphs(&self, t: Option<&toml::Table>, section: &str, key: &str) -> Result<Option<Vec<char>>, ParseError> {
        let Some(s) = self.string(t, section, key)? else { return Ok(None) };
        let g: Vec<char> = s.chars().collect();
        if g.len() > MAX_GLYPHS {
            return Err(self.err(section, key, &format!("at most {MAX_GLYPHS} glyphs")));
        }
        Ok(Some(g))
    }

    fn strings(&self, t: Option<&toml::Table>, section: &str, key: &str, max: usize, max_len: usize) -> Result<Option<Vec<String>>, ParseError> {
        let Some(v) = t.and_then(|t| t.get(key)) else { return Ok(None) };
        let Some(a) = v.as_array() else { return Err(self.err(section, key, "must be a list of strings")) };
        if a.len() > max {
            return Err(self.err(section, key, &format!("at most {max} entries")));
        }
        let mut out = Vec::new();
        for item in a {
            let Some(s) = item.as_str() else { return Err(self.err(section, key, "must be a list of strings")) };
            let n = s.chars().count();
            if n == 0 || n > max_len {
                return Err(self.err(section, key, &format!("each entry is 1 to {max_len} characters")));
            }
            out.push(s.to_string());
        }
        Ok(Some(out))
    }

    fn boolean(&self, t: Option<&toml::Table>, section: &str, key: &str) -> Result<Option<bool>, ParseError> {
        match t.and_then(|t| t.get(key)) {
            None => Ok(None),
            Some(toml::Value::Boolean(b)) => Ok(Some(*b)),
            Some(_) => Err(self.err(section, key, "must be true or false")),
        }
    }

    fn int(&self, t: Option<&toml::Table>, section: &str, key: &str, lo: i64, hi: i64) -> Result<Option<i64>, ParseError> {
        match t.and_then(|t| t.get(key)) {
            None => Ok(None),
            Some(toml::Value::Integer(n)) if (lo..=hi).contains(n) => Ok(Some(*n)),
            Some(toml::Value::Integer(_)) => Err(self.err(section, key, &format!("must be {lo} to {hi}"))),
            Some(_) => Err(self.err(section, key, "must be a whole number")),
        }
    }

    fn float(&self, t: Option<&toml::Table>, section: &str, key: &str, lo: f64, hi: f64) -> Result<Option<f32>, ParseError> {
        let n = match t.and_then(|t| t.get(key)) {
            None => return Ok(None),
            Some(toml::Value::Float(f)) => *f,
            Some(toml::Value::Integer(i)) => *i as f64,
            Some(_) => return Err(self.err(section, key, "must be a number")),
        };
        if !(lo..=hi).contains(&n) {
            return Err(self.err(section, key, &format!("must be {lo} to {hi}")));
        }
        Ok(Some(n as f32))
    }

    fn kind(&self, t: Option<&toml::Table>, section: &str, key: &str, names: &[&str]) -> Result<Option<usize>, ParseError> {
        let Some(s) = self.string(t, section, key)? else { return Ok(None) };
        match names.iter().position(|n| *n == s) {
            Some(i) => Ok(Some(i)),
            None => Err(self.err(section, key, &format!("must be one of {}", names.join(", ")))),
        }
    }
}

const BOX_NAMES: [&str; 4] = ["lines", "band", "either", "none"];
const BOX_KINDS: [BoxKindRule; 4] = [BoxKindRule::Lines, BoxKindRule::Band, BoxKindRule::Either, BoxKindRule::None];

/// Reads a profile. Missing keys take the Shell values; unknown keys come back as warnings.
/// A value out of range is an error, never a clamp.
pub fn parse(text: &str) -> Result<(FxRules, Vec<String>), ParseError> {
    let top: toml::Table = toml::from_str(text).map_err(|e| ParseError {
        line: e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1),
        message: e.message().to_string(),
    })?;
    let mut rd = Reader { text, warnings: Vec::new() };
    let mut r = FxRules::default();
    for k in top.keys().filter(|k| !TOP_KEYS.contains(&k.as_str())) {
        rd.warnings.push(format!("unknown key `{k}`"));
    }
    if let Some(n) = rd.string(Some(&top), "", "name")? {
        if n.trim().is_empty() || n.chars().count() > MAX_NAME {
            return Err(rd.err("", "name", &format!("1 to {MAX_NAME} characters")));
        }
        r.name = n;
    }

    let m = rd.section(&top, "match", MATCH_KEYS)?;
    if let Some(v) = rd.strings(m, "match", "program", MAX_PROGRAMS, MAX_TEXT_LEN)? {
        r.matching.program = v;
    }
    if let Some(v) = rd.strings(m, "match", "text", MAX_TEXTS, MAX_TEXT_LEN)? {
        r.matching.text = v;
    }
    if let Some(v) = rd.glyphs(m, "match", "prompt")? {
        r.matching.prompt = v;
    }
    if let Some(i) = rd.kind(m, "match", "shape", &BOX_NAMES)? {
        r.matching.shape = Some(BOX_KINDS[i]);
    }

    let c = rd.section(&top, "cursor", CURSOR_KEYS)?;
    if let Some(i) = rd.kind(c, "cursor", "kind", &["real", "painted", "either"])? {
        r.cursor.kind = [CursorKind::Real, CursorKind::Painted, CursorKind::Either][i];
    }
    if let Some(v) = c.and_then(|c| c.get("mark")) {
        let bad = || rd.err("cursor", "mark", "must be \"reverse\" or a color like \"#2c2c2c\", or a list of them (1 to 32)");
        let items: Vec<&toml::Value> = match v {
            toml::Value::Array(a) => a.iter().collect(),
            one => vec![one],
        };
        if items.is_empty() || items.len() > MAX_GLYPHS {
            return Err(bad());
        }
        let mut marks = Vec::new();
        for i in items {
            marks.push(i.as_str().and_then(parse_mark).ok_or_else(bad)?);
        }
        r.cursor.marks = marks;
    }
    if let Some(b) = rd.boolean(c, "cursor", "echo_anchor")? {
        r.cursor.echo_anchor = b;
    }

    let b = rd.section(&top, "box", BOX_KEYS)?;
    if let Some(i) = rd.kind(b, "box", "kind", &BOX_NAMES)? {
        r.boxes.kind = BOX_KINDS[i];
    }
    if let Some(v) = rd.glyphs(b, "box", "line_glyphs")? {
        r.boxes.line_glyphs = v;
    }
    if let Some(v) = rd.float(b, "box", "line_fill", 0.1, 1.0)? {
        r.boxes.line_fill = v;
    }
    if let Some(v) = rd.int(b, "box", "bottom_within", 0, 50)? {
        r.boxes.bottom_within = v as usize;
    }
    if let Some(v) = b.and_then(|b| b.get("band_rows")) {
        let ok = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some((a[0].as_integer()?, a[1].as_integer()?)));
        match ok {
            Some((lo, hi)) if (1..=50).contains(&lo) && (1..=50).contains(&hi) && lo <= hi => r.boxes.band_rows = (lo as usize, hi as usize),
            _ => return Err(rd.err("box", "band_rows", "must be [min, max], each 1 to 50, min not over max")),
        }
    }
    if let Some(v) = rd.float(b, "box", "band_max_screen", 0.1, 1.0)? {
        r.boxes.band_max_screen = v;
    }
    if let Some(v) = rd.float(b, "box", "band_min_width", 0.1, 1.0)? {
        r.boxes.band_min_width = v;
    }
    if let Some(v) = rd.int(b, "box", "band_tolerance", 0, 50)? {
        r.boxes.band_tolerance = v as usize;
    }
    if let Some(v) = rd.glyphs(b, "box", "edge_glyphs")? {
        r.boxes.edge_glyphs = v;
    }

    let p = rd.section(&top, "prompt_mark", MARK_KEYS)?;
    if let Some(v) = rd.glyphs(p, "prompt_mark", "glyphs")? {
        r.prompt_marks = v;
    }

    let d = rd.section(&top, "done", DONE_KEYS)?;
    if let Some(s) = rd.string(d, "done", "enter")? {
        if s.is_empty() || s.len() > 16 {
            return Err(rd.err("done", "enter", "1 to 16 bytes"));
        }
        r.done.enter = s.into_bytes();
    }
    if let Some(v) = rd.int(d, "done", "quiet_ms", 100, 60_000)? {
        r.done.quiet = Duration::from_millis(v as u64);
    }
    if let Some(v) = rd.int(d, "done", "busy_ms", 0, 60_000)? {
        r.done.busy = Duration::from_millis(v as u64);
    }
    if let Some(b) = rd.boolean(d, "done", "osc133")? {
        r.done.osc133 = b;
    }
    Ok((r, rd.warnings))
}

fn parse_mark(s: &str) -> Option<PaintedMark> {
    if s == "reverse" {
        return Some(PaintedMark::Reverse);
    }
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(PaintedMark::Background(rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)))
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn chars(g: &[char]) -> String {
    q(&g.iter().collect::<String>())
}

fn list(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "))
}

fn kind_name(k: BoxKindRule) -> &'static str {
    BOX_NAMES[BOX_KINDS.iter().position(|x| *x == k).unwrap_or(2)]
}

/// The unknown entries of one section of `keep`, as TOML lines.
fn extras(keep: &toml::Table, section: &str, known: &[&str]) -> String {
    let t: toml::Table = keep.get(section).and_then(|v| v.as_table()).map(|t| t.iter().filter(|(k, _)| !known.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    toml::to_string(&t).unwrap_or_default()
}

/// Writes every key. Unknown keys in `keep` (a table read from the old file) are written back.
pub fn to_toml(rules: &FxRules, keep: &toml::Table) -> String {
    let unknown: toml::Table = keep.iter().filter(|(k, _)| !TOP_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    let (tables, scalars): (toml::Table, toml::Table) = {
        let (mut t, mut s) = (toml::Table::new(), toml::Table::new());
        for (k, v) in unknown {
            if v.is_table() { t.insert(k, v) } else { s.insert(k, v) };
        }
        (t, s)
    };
    let mut o = format!("name = {}\n", q(&rules.name));
    o.push_str(&toml::to_string(&scalars).unwrap_or_default());

    o.push_str("\n[match]\n");
    o.push_str(&format!("program = {}\ntext = {}\nprompt = {}\n", list(&rules.matching.program), list(&rules.matching.text), chars(&rules.matching.prompt)));
    if let Some(s) = rules.matching.shape {
        o.push_str(&format!("shape = {}\n", q(kind_name(s))));
    }
    o.push_str(&extras(keep, "match", MATCH_KEYS));

    let kind = match rules.cursor.kind {
        CursorKind::Real => "real",
        CursorKind::Painted => "painted",
        CursorKind::Either => "either",
    };
    let one = |m: &PaintedMark| match m {
        PaintedMark::Reverse => "reverse".to_string(),
        PaintedMark::Background(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
    };
    let mark = match rules.cursor.marks.as_slice() {
        [m] => q(&one(m)),
        many => list(&many.iter().map(one).collect::<Vec<_>>()),
    };
    o.push_str(&format!("\n[cursor]\nkind = {}\nmark = {}\necho_anchor = {}\n", q(kind), mark, rules.cursor.echo_anchor));
    o.push_str(&extras(keep, "cursor", CURSOR_KEYS));

    let b = &rules.boxes;
    o.push_str(&format!(
        "\n[box]\nkind = {}\nline_glyphs = {}\nline_fill = {:?}\nbottom_within = {}\nband_rows = [{}, {}]\nband_max_screen = {:?}\nband_min_width = {:?}\nband_tolerance = {}\nedge_glyphs = {}\n",
        q(kind_name(b.kind)),
        chars(&b.line_glyphs),
        b.line_fill,
        b.bottom_within,
        b.band_rows.0,
        b.band_rows.1,
        b.band_max_screen,
        b.band_min_width,
        b.band_tolerance,
        chars(&b.edge_glyphs),
    ));
    o.push_str(&extras(keep, "box", BOX_KEYS));

    o.push_str(&format!("\n[prompt_mark]\nglyphs = {}\n", chars(&rules.prompt_marks)));
    o.push_str(&extras(keep, "prompt_mark", MARK_KEYS));

    let d = &rules.done;
    o.push_str(&format!(
        "\n[done]\nenter = {}\nquiet_ms = {}\nbusy_ms = {}\nosc133 = {}\n",
        q(&String::from_utf8_lossy(&d.enter)),
        d.quiet.as_millis(),
        d.busy.as_millis(),
        d.osc133
    ));
    o.push_str(&extras(keep, "done", DONE_KEYS));

    if !tables.is_empty() {
        o.push('\n');
        o.push_str(&toml::to_string(&tables).unwrap_or_default());
    }
    o
}

/// The Shell numbers with every rule's line glyphs, edge glyphs, prompt marks and painted marks added. Used to
/// find the cursor before a profile is chosen. Painted marks are added too.
pub fn union(rules: &[&FxRules]) -> FxRules {
    let mut u = FxRules::default();
    for r in rules {
        for (to, from) in [
            (&mut u.boxes.line_glyphs, &r.boxes.line_glyphs),
            (&mut u.boxes.edge_glyphs, &r.boxes.edge_glyphs),
            (&mut u.prompt_marks, &r.prompt_marks),
        ] {
            for g in from {
                if !to.contains(g) {
                    to.push(*g);
                }
            }
        }
        for m in &r.cursor.marks {
            if !u.cursor.marks.contains(m) {
                u.cursor.marks.push(*m);
            }
        }
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    #[test]
    fn an_empty_file_is_the_shell_rules() {
        let (r, w) = parse("").unwrap();
        assert_eq!(r.done.quiet, Duration::from_millis(1200));
        assert_eq!(r.boxes.line_glyphs, vec!['\u{2500}', '\u{2501}']);
        assert!(w.is_empty());
        assert_eq!(r, FxRules::default());
    }

    #[test]
    fn every_key_reads() {
        let t = r##"
name = "Full"
[match]
program = ["nvim", "vi"]
text = ["Hello", "World"]
prompt = "λ"
shape = "band"
[cursor]
kind = "painted"
mark = "#102030"
echo_anchor = false
[box]
kind = "either"
line_glyphs = "═"
line_fill = 0.5
bottom_within = 4
band_rows = [3, 9]
band_max_screen = 0.4
band_min_width = 0.6
band_tolerance = 3
edge_glyphs = "▔"
[prompt_mark]
glyphs = "λ%"
[done]
enter = "\n"
quiet_ms = 2500
busy_ms = 300
osc133 = false
"##;
        let (r, w) = parse(t).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(r.name, "Full");
        assert_eq!(r.matching.program, vec!["nvim".to_string(), "vi".to_string()]);
        assert_eq!(r.matching.text, vec!["Hello".to_string(), "World".to_string()]);
        assert_eq!(r.matching.prompt, vec!['λ']);
        assert_eq!(r.matching.shape, Some(BoxKindRule::Band));
        assert_eq!(r.cursor.kind, CursorKind::Painted);
        assert_eq!(r.cursor.marks, vec![PaintedMark::Background(rgb(0x10, 0x20, 0x30))]);
        assert!(!r.cursor.echo_anchor);
        assert_eq!(r.boxes.kind, BoxKindRule::Either);
        assert_eq!(r.boxes.line_glyphs, vec!['═']);
        assert_eq!(r.boxes.line_fill, 0.5);
        assert_eq!(r.boxes.bottom_within, 4);
        assert_eq!(r.boxes.band_rows, (3, 9));
        assert_eq!(r.boxes.band_max_screen, 0.4);
        assert_eq!(r.boxes.band_min_width, 0.6);
        assert_eq!(r.boxes.band_tolerance, 3);
        assert_eq!(r.boxes.edge_glyphs, vec!['▔']);
        assert_eq!(r.prompt_marks, vec!['λ', '%']);
        assert_eq!(r.done.enter, b"\n".to_vec());
        assert_eq!(r.done.quiet, Duration::from_millis(2500));
        assert_eq!(r.done.busy, Duration::from_millis(300));
        assert!(!r.done.osc133);
        assert_eq!(parse(&to_toml(&r, &toml::Table::new())).unwrap().0, r);
    }

    #[test]
    fn out_of_range_is_an_error_with_its_line() {
        let e = parse("[done]\nquiet_ms = 50\n").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("quiet_ms"));
        assert!(parse("[box]\nline_fill = 1.5\n").is_err());
        assert!(parse("[box]\nband_rows = [5, 2]\n").is_err());
        assert!(parse("[box]\nline_glyphs = \"0123456789012345678901234567890123\"\n").is_err());
        assert!(parse("[match]\ntext = [\"\"]\n").is_err());
        let many = format!("[match]\ntext = [{}]\n", vec!["\"a\""; 17].join(","));
        assert!(parse(&many).is_err());
    }

    #[test]
    fn a_syntax_error_has_a_line() {
        let e = parse("name = \"x\"\n[done\n").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(parse("name = 3\n").is_err());
    }

    #[test]
    fn unknown_keys_are_warnings_and_kept_on_save() {
        let t = "name = \"X\"\nfuture = 3\n";
        let (r, w) = parse(t).unwrap();
        assert_eq!(w, vec!["unknown key `future`".to_string()]);
        let keep: toml::Table = t.parse().unwrap();
        assert!(to_toml(&r, &keep).contains("future = 3"));
        let t2 = "[done]\nlater = \"x\"\n[extra]\na = 1\n";
        let (r2, w2) = parse(t2).unwrap();
        assert_eq!(w2.len(), 2);
        let out = to_toml(&r2, &t2.parse().unwrap());
        assert!(out.contains("later = \"x\"") && out.contains("[extra]"));
        parse(&out).unwrap();
    }

    #[test]
    fn a_painted_mark_is_reverse_or_a_color() {
        let m = |s: &str| parse(&format!("[cursor]\nmark = \"{s}\"\n")).map(|(r, _)| r.cursor.marks);
        assert_eq!(m("reverse").unwrap(), vec![PaintedMark::Reverse]);
        assert_eq!(m("#2c2c2c").unwrap(), vec![PaintedMark::Background(rgb(44, 44, 44))]);
        assert!(m("blue").is_err());
        assert!(m("#12345").is_err());
    }

    #[test]
    fn every_shipped_profile_parses_without_warnings() {
        assert_eq!(SHIPPED.len(), 6);
        for (f, t) in SHIPPED {
            let (_, w) = parse(t).unwrap_or_else(|e| panic!("{f}: {}", e.message));
            assert!(w.is_empty(), "{f}");
        }
    }

    #[test]
    fn the_shell_file_is_the_default_rules() {
        let (_, t) = SHIPPED.iter().find(|(f, _)| *f == "shell.toml").unwrap();
        assert_eq!(parse(t).unwrap().0, FxRules::default());
    }

    #[test]
    fn round_trip_keeps_every_value() {
        for (_, t) in SHIPPED {
            let (r, _) = parse(t).unwrap();
            assert_eq!(parse(&to_toml(&r, &toml::Table::new())).unwrap().0, r);
        }
    }

    #[test]
    fn every_shipped_text_is_on_its_recorded_screen() {
        let screen = |f: &str| {
            let raw = std::fs::read_to_string(format!("{}/tests/fixtures/agents/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            raw.replace(|c: char| c == '\x1b', "\u{1}")
        };
        let strip = |s: String| regex::Regex::new("\u{1}[\\[()][0-9;?]*[A-Za-z0-9]").unwrap().replace_all(&s, "").into_owned();
        for (file, shot) in [("codex.ansi", "codex.toml"), ("opencode.ansi", "opencode.toml"), ("hermes.ansi", "hermes.toml"), ("agy.ansi", "agy.toml")] {
            let (_, t) = SHIPPED.iter().find(|(f, _)| *f == shot).unwrap();
            let (r, _) = parse(t).unwrap();
            let s = strip(screen(file));
            assert!(r.matching.text.iter().any(|x| s.contains(x.as_str())), "{shot} on {file}");
        }
    }

    #[test]
    fn union_takes_every_line_glyph() {
        let mut a = FxRules::default();
        a.boxes.line_glyphs = vec!['═'];
        a.prompt_marks = vec!['λ'];
        let shell = FxRules::default();
        let u = union(&[&a, &shell]);
        for g in ['─', '━', '═'] {
            assert!(u.boxes.line_glyphs.contains(&g), "{g}");
        }
        assert_eq!(u.boxes.line_glyphs.len(), 3);
        assert!(u.prompt_marks.contains(&'λ') && u.prompt_marks.contains(&'❯'));
        assert_eq!(u.done, shell.done);
    }

    #[test]
    fn marks_may_be_a_list() {
        let t = "[cursor]\nmark = [\"reverse\", \"#2c2c2c\"]\n";
        let (r, _) = parse(t).unwrap();
        assert_eq!(r.cursor.marks, vec![PaintedMark::Reverse, PaintedMark::Background(rgb(44, 44, 44))]);
        let out = to_toml(&r, &toml::Table::new());
        assert!(out.contains("mark = [\"reverse\", \"#2c2c2c\"]"), "{out}");
        assert_eq!(parse(&out).unwrap().0, r);
        assert!(parse("[cursor]\nmark = []\n").is_err());
        assert!(parse("[cursor]\nmark = [\"reverse\", \"blue\"]\n").is_err());
        let many = format!("[cursor]\nmark = [{}]\n", vec!["\"reverse\""; 33].join(","));
        assert!(parse(&many).is_err());
    }

    #[test]
    fn union_takes_every_painted_mark() {
        let mut a = FxRules::default();
        a.cursor.marks = vec![PaintedMark::Background(rgb(44, 44, 44))];
        let shell = FxRules::default();
        let u = union(&[&shell, &a, &a]);
        assert_eq!(u.cursor.marks, vec![PaintedMark::Reverse, PaintedMark::Background(rgb(44, 44, 44))]);
    }
}
