//! Turns a porthole's text into styled lines: markdown (pulldown-cmark), JSON (pretty
//! printed, key order kept), or code (syntect, colored from the profile's ANSI colors as
//! iTerm2's highlight templates do); then lays them out at a width.

use std::sync::OnceLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::easy::ScopeRangeIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack};
use syntect::util::LinesWithEndings;

use super::detect::{self, Language};
use crate::palette::{rgb, Palette, Rgb};

/// Which font a run uses: the prose font (Segoe UI Variable) or the profile's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Prose,
    Code,
}

/// Text in one style.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub face: Face,
    pub link: Option<String>,
}

impl Run {
    fn new(text: impl Into<String>, color: Rgb, face: Face) -> Run {
        Run { text: text.into(), color, bold: false, italic: false, underline: false, face, link: None }
    }

    fn same_style(&self, o: &Run) -> bool {
        (self.color, self.bold, self.italic, self.underline, self.face, &self.link) == (o.color, o.bold, o.italic, o.underline, o.face, &o.link)
    }
}

/// A paragraph, heading, list item, or source line: wrapped as one unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub runs: Vec<Run>,
    /// Text size against the terminal font (headings).
    pub scale: f32,
    /// Left indent in cells (lists, quotes).
    pub indent: f32,
    /// A line of a code block: drawn on a shaded band.
    pub code_bg: bool,
    /// A horizontal rule across the width.
    pub rule: bool,
}

impl Block {
    fn new(indent: f32) -> Block {
        Block { runs: Vec::new(), scale: 1.0, indent, code_bg: false, rule: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

impl Doc {
    /// The document as text: each block's runs joined, the blocks joined with a newline; a
    /// rule is an empty line.
    pub fn plain(&self) -> String {
        let lines: Vec<String> = self.blocks.iter().map(|b| if b.rule { String::new() } else { b.runs.iter().map(|r| r.text.as_str()).collect() }).collect();
        lines.join("\n")
    }
}

pub fn render(text: &str, language: &Language, palette: &Palette, dark: bool) -> Doc {
    render_themed(text, language, palette, dark, &Theme::builtin(palette, dark))
}

/// `render` with a highlight theme of its own.
pub fn render_themed(text: &str, language: &Language, palette: &Palette, dark: bool, theme: &Theme) -> Doc {
    match language {
        Language::Markdown => markdown(text, palette, theme),
        Language::Json => json(text, palette, dark),
        Language::Plain => Doc { blocks: plain_lines(text, palette.fg, 0.0, false) },
        Language::Code(name) => Doc { blocks: code_lines(text, name, palette, theme, 0.0, false) },
    }
}

fn plain_lines(text: &str, fg: Rgb, indent: f32, code_bg: bool) -> Vec<Block> {
    text.lines()
        .map(|l| {
            let mut b = Block::new(indent);
            b.code_bg = code_bg;
            if !l.is_empty() {
                b.runs.push(Run::new(l, fg, Face::Code));
            }
            b
        })
        .collect()
}

/// Syntect scope prefixes to highlight.js class names (iTerm2's themes are written for
/// them), the more specific first. An empty name is the text color.
fn scope_classes() -> &'static [(Scope, &'static str)] {
    static TABLE: OnceLock<Vec<(Scope, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let rules: &[(&str, &str)] = &[
            ("comment", "comment"),
            ("string.regexp", "regexp"),
            ("string", "string"),
            ("constant.character", "string"),
            ("markup.inserted", "addition"),
            ("constant.numeric", "number"),
            ("constant.language", "literal"),
            ("support.type", "type"),
            ("support.class", "type"),
            ("support.function", "built_in"),
            ("variable.parameter", "params"),
            ("markup.deleted", "deletion"),
            ("storage.type.function", "keyword"),
            ("storage.type.struct", "keyword"),
            ("storage.type.enum", "keyword"),
            ("storage.type.class", "keyword"),
            ("storage.type.trait", "keyword"),
            ("storage.type.impl", "keyword"),
            ("storage.type.module", "keyword"),
            ("storage.type.type", "keyword"),
            ("storage.type.union", "keyword"),
            ("storage.type.interface", "keyword"),
            ("storage.type.namespace", "keyword"),
            ("storage.type", "type"),
            ("storage.modifier", "keyword"),
            ("keyword.operator", ""),
            ("keyword", "keyword"),
            ("entity.other.attribute-name.id", "selector-id"),
            ("entity.other.attribute-name.class", "selector-class"),
            ("entity.other.attribute-name", "attribute"),
            ("entity.name.tag", "tag"),
            ("entity.name.section", "section"),
            ("entity.name", "title"),
            ("markup.heading", "section"),
            ("markup.underline.link", "link"),
            ("meta.preprocessor", "meta"),
            ("variable.language", "variable"),
            ("markup.bold", "strong"),
            ("markup.italic", "emphasis"),
        ];
        rules.iter().map(|(s, c)| (Scope::new(s).expect("a valid scope"), *c)).collect()
    })
}

fn classify(stack: &ScopeStack) -> &'static str {
    for scope in stack.as_slice().iter().rev() {
        if let Some((_, c)) = scope_classes().iter().find(|(p, _)| p.is_prefix_of(*scope)) {
            return c;
        }
    }
    ""
}

/// Highlight colors by highlight.js class: from a CSS file, or the built-in templates.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    fg: Rgb,
    classes: std::collections::HashMap<String, (Option<Rgb>, bool, bool)>,
}

/// Termurion's two templates, iTerm2's class groups: `${name}` is a profile color.
const DARK_CSS: &str = "
.hljs { color: ${fg}; }
.hljs-comment, .hljs-quote { color: ${cyan}; }
.hljs-variable, .hljs-template-variable, .hljs-tag, .hljs-name, .hljs-selector-id, .hljs-selector-class, .hljs-regexp, .hljs-link, .hljs-meta { color: ${brightRed}; }
.hljs-number, .hljs-built_in, .hljs-literal, .hljs-type, .hljs-params, .hljs-deletion { color: ${brightBlue}; }
.hljs-title, .hljs-section, .hljs-attribute { color: ${brightYellow}; }
.hljs-string, .hljs-symbol, .hljs-bullet, .hljs-addition { color: ${brightGreen}; }
.hljs-keyword, .hljs-selector-tag { color: ${brightMagenta}; }
.hljs-emphasis { font-style: italic; }
.hljs-strong { font-weight: bold; }
";

const LIGHT_CSS: &str = "
.hljs { color: ${fg}; }
.hljs-comment, .hljs-quote { color: ${brightBlack}; }
.hljs-variable, .hljs-template-variable, .hljs-tag, .hljs-name, .hljs-selector-id, .hljs-selector-class, .hljs-regexp, .hljs-link, .hljs-meta { color: ${red}; }
.hljs-number, .hljs-built_in, .hljs-literal, .hljs-type, .hljs-params, .hljs-deletion { color: ${blue}; }
.hljs-title, .hljs-section, .hljs-attribute { color: ${yellow}; }
.hljs-string, .hljs-symbol, .hljs-bullet, .hljs-addition { color: ${green}; }
.hljs-keyword, .hljs-selector-tag { color: ${magenta}; }
.hljs-emphasis { font-style: italic; }
.hljs-strong { font-weight: bold; }
";

const COLOR_NAMES: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
];

fn parse_hex(v: &str) -> Option<Rgb> {
    let h = v.trim().strip_prefix('#').filter(|h| h.is_ascii())?;
    let n = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        3 => {
            let d = |i: usize| n(&h[i..i + 1]).map(|x| x * 17);
            Some(rgb(d(0)?, d(1)?, d(2)?))
        }
        6 => Some(rgb(n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?)),
        _ => None,
    }
}

impl Theme {
    /// iTerm2's template for a dark or light background, in the profile's colors.
    pub fn builtin(palette: &Palette, dark: bool) -> Theme {
        Theme::from_css(if dark { DARK_CSS } else { LIGHT_CSS }, palette)
    }

    /// A theme from CSS: `${fg}`, `${bg}`, and the 16 color names fill in from the profile;
    /// rules `.hljs-<class>[, ...] { color; font-weight: bold; font-style: italic }` count,
    /// anything else is ignored.
    pub fn from_css(css: &str, palette: &Palette) -> Theme {
        let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
        let mut css = css.replace("${fg}", &hex(palette.fg)).replace("${bg}", &hex(palette.bg)).replace("${fontFamily}", "monospace");
        for (i, name) in COLOR_NAMES.iter().enumerate() {
            css = css.replace(&format!("${{{name}}}"), &hex(palette.ansi[i]));
        }
        // Comments out.
        let mut clean = String::new();
        let mut rest = css.as_str();
        while let Some(i) = rest.find("/*") {
            clean.push_str(&rest[..i]);
            rest = rest[i + 2..].split_once("*/").map_or("", |(_, r)| r);
        }
        clean.push_str(rest);
        let mut t = Theme { fg: palette.fg, classes: std::collections::HashMap::new() };
        for rule in clean.split('}') {
            let Some((selectors, body)) = rule.split_once('{') else { continue };
            let (mut color, mut bold, mut italic) = (None, false, false);
            for decl in body.split(';') {
                let Some((k, v)) = decl.split_once(':') else { continue };
                match (k.trim(), v.trim()) {
                    ("color", v) => color = parse_hex(v),
                    ("font-weight", v) => bold = v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600),
                    ("font-style", v) => italic = v == "italic",
                    _ => {}
                }
            }
            for sel in selectors.split(',') {
                match sel.trim() {
                    ".hljs" => {
                        if let Some(c) = color {
                            t.fg = c;
                        }
                    }
                    s => {
                        if let Some(class) = s.strip_prefix(".hljs-") {
                            t.classes.insert(class.to_string(), (color, bold, italic));
                        }
                    }
                }
            }
        }
        t
    }

    /// A class's color, bold, and italic.
    pub fn style(&self, class: &str) -> (Rgb, bool, bool) {
        match self.classes.get(class) {
            Some((c, b, i)) => (c.unwrap_or(self.fg), *b, *i),
            None => (self.fg, false, false),
        }
    }
}

/// The theme for a CSS file setting (`nativeRenderingCSSDark` or `...Light`): the built-in
/// template when the setting is empty, and also, with the error, when the file cannot be read.
pub fn load_theme(path: &str, palette: &Palette, dark: bool) -> (Theme, Option<String>) {
    if path.trim().is_empty() {
        return (Theme::builtin(palette, dark), None);
    }
    // A setting (saved, importable): judged before it is read.
    match crate::localfs::judge_io(path).and_then(|p| crate::localfs::read_to_string(&p)) {
        Ok(css) => (Theme::from_css(&css, palette), None),
        Err(e) => (Theme::builtin(palette, dark), Some(format!("Could not read the highlight theme {path}: {e}"))),
    }
}

/// Source lines highlighted with a syntect grammar; plain lines when it has none.
fn code_lines(text: &str, syntax: &str, palette: &Palette, theme: &Theme, indent: f32, code_bg: bool) -> Vec<Block> {
    let set = detect::syntaxes();
    let Some(syntax) = set.find_syntax_by_name(syntax) else { return plain_lines(text, palette.fg, indent, code_bg) };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let mut b = Block::new(indent);
        b.code_bg = code_bg;
        let ops = match state.parse_line(line, set) {
            Ok(ops) => ops,
            // A grammar that fails mid-document: the rest shows plain.
            Err(_) => {
                out.extend(plain_lines(line, palette.fg, indent, code_bg));
                continue;
            }
        };
        for (range, op) in ScopeRangeIterator::new(&ops, line) {
            if stack.apply(op).is_err() {
                stack = ScopeStack::new();
            }
            let piece = line[range].trim_end_matches(['\n', '\r']);
            if piece.is_empty() {
                continue;
            }
            let (color, bold, italic) = theme.style(classify(&stack));
            let run = Run { bold, italic, ..Run::new(piece, color, Face::Code) };
            match b.runs.last_mut() {
                Some(last) if last.same_style(&run) => last.text.push_str(&run.text),
                _ => b.runs.push(run),
            }
        }
        out.push(b);
    }
    out
}

fn mix(bg: Rgb, fg: Rgb, t: f32) -> Rgb {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    rgb(m(bg.r, fg.r), m(bg.g, fg.g), m(bg.b, fg.b))
}

/// JSON pretty printed as iTerm2 does (4 spaces, one member per line), with key order kept
/// and booleans as true and false.
fn json(text: &str, palette: &Palette, dark: bool) -> Doc {
    let (green, red) = if dark { (rgb(102, 255, 102), rgb(255, 102, 102)) } else { (rgb(51, 178, 51), rgb(178, 51, 51)) };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            let mut b = Block::new(0.0);
            b.runs.push(Run::new(e.to_string(), red, Face::Code));
            return Doc { blocks: vec![b] };
        }
    };
    let colors = JsonColors { green, red, grey: rgb(128, 128, 128), punct: mix(palette.bg, palette.fg, 0.75), fg: palette.fg };
    let mut blocks = Vec::new();
    json_value(&value, 0, None, false, &colors, &mut blocks);
    Doc { blocks }
}

struct JsonColors {
    green: Rgb,
    red: Rgb,
    grey: Rgb,
    punct: Rgb,
    fg: Rgb,
}

fn json_value(v: &serde_json::Value, indent: usize, key: Option<&str>, comma: bool, c: &JsonColors, out: &mut Vec<Block>) {
    use serde_json::Value;
    let start = |out: &mut Vec<Block>| {
        let mut b = Block::new(0.0);
        if indent > 0 {
            b.runs.push(Run::new(" ".repeat(indent), c.fg, Face::Code));
        }
        if let Some(k) = key {
            b.runs.push(Run::new(serde_json::to_string(k).unwrap_or_default(), c.green, Face::Code));
            b.runs.push(Run::new(": ", c.punct, Face::Code));
        }
        out.push(b);
    };
    let end = |out: &mut Vec<Block>, text: &str| {
        let mut b = Block::new(0.0);
        if indent > 0 {
            b.runs.push(Run::new(" ".repeat(indent), c.fg, Face::Code));
        }
        b.runs.push(Run::new(text, c.punct, Face::Code));
        if comma {
            b.runs.push(Run::new(",", c.punct, Face::Code));
        }
        out.push(b);
    };
    let push = |out: &mut Vec<Block>, run: Run| {
        if let Some(b) = out.last_mut() {
            b.runs.push(run);
        }
    };
    match v {
        Value::Object(m) if !m.is_empty() => {
            start(out);
            push(out, Run::new("{", c.punct, Face::Code));
            for (i, (k, val)) in m.iter().enumerate() {
                json_value(val, indent + 4, Some(k), i + 1 < m.len(), c, out);
            }
            end(out, "}");
            return;
        }
        Value::Array(a) if !a.is_empty() => {
            start(out);
            push(out, Run::new("[", c.punct, Face::Code));
            for (i, val) in a.iter().enumerate() {
                json_value(val, indent + 4, None, i + 1 < a.len(), c, out);
            }
            end(out, "]");
            return;
        }
        _ => {}
    }
    start(out);
    let run = match v {
        Value::Object(_) => Run::new("{}", c.punct, Face::Code),
        Value::Array(_) => Run::new("[]", c.punct, Face::Code),
        Value::String(s) => Run::new(serde_json::to_string(s).unwrap_or_default(), c.green, Face::Code),
        Value::Number(n) => Run::new(n.to_string(), c.red, Face::Code),
        Value::Bool(b) => Run::new(b.to_string(), c.red, Face::Code),
        Value::Null => Run::new("null", c.grey, Face::Code),
    };
    push(out, run);
    if comma {
        push(out, Run::new(",", c.punct, Face::Code));
    }
}

const HEADING_SCALES: [f32; 6] = [2.0, 1.5, 1.3, 1.0, 0.8, 0.7];

/// Markdown in the prose font, every style in the default foreground (iTerm2), code in the
/// profile's font, fenced code highlighted by its info string.
fn markdown(text: &str, palette: &Palette, theme: &Theme) -> Doc {
    let fg = palette.fg;
    let mut out: Vec<Block> = Vec::new();
    let mut cur: Option<Block> = None;
    let (mut italic, mut bold, mut quote) = (0u32, 0u32, 0u32);
    let mut link: Option<String> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_open = false;
    let mut code: Option<(String, String)> = None;
    let mut table: Option<(Vec<Vec<String>>, usize)> = None;
    let mut heading = false;
    let indent = |quote: u32, lists: &Vec<Option<u64>>| (quote * 2) as f32 + (lists.len() as f32 * 2.0);
    fn finish(out: &mut Vec<Block>, cur: &mut Option<Block>) {
        if let Some(b) = cur.take() {
            out.push(b);
        }
    }
    fn spacer(out: &mut Vec<Block>) {
        if out.last().is_some_and(|b| !b.runs.is_empty() || b.rule) {
            out.push(Block::new(0.0));
        }
    }
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text.trim(), opts) {
        let style = |t: &str, face: Face, italic: u32, bold: u32, link: &Option<String>| Run {
            italic: italic > 0,
            bold: bold > 0,
            underline: link.is_some(),
            link: link.clone(),
            ..Run::new(t, fg, face)
        };
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                finish(&mut out, &mut cur);
                let mut b = Block::new(indent(quote, &lists));
                b.scale = HEADING_SCALES[heading_index(level)];
                cur = Some(b);
                heading = true;
            }
            Event::End(TagEnd::Heading(_)) => {
                heading = false;
                finish(&mut out, &mut cur);
                spacer(&mut out);
            }
            Event::Start(Tag::Paragraph) => {
                if item_open {
                    item_open = false;
                } else {
                    finish(&mut out, &mut cur);
                    cur = Some(Block::new(indent(quote, &lists)));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                finish(&mut out, &mut cur);
                if lists.is_empty() {
                    spacer(&mut out);
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                finish(&mut out, &mut cur);
                quote += 1;
                italic += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                finish(&mut out, &mut cur);
                quote = quote.saturating_sub(1);
                italic = italic.saturating_sub(1);
                spacer(&mut out);
            }
            Event::Start(Tag::List(first)) => {
                finish(&mut out, &mut cur);
                lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                finish(&mut out, &mut cur);
                lists.pop();
                if lists.is_empty() {
                    spacer(&mut out);
                }
            }
            Event::Start(Tag::Item) => {
                finish(&mut out, &mut cur);
                let depth = lists.len().saturating_sub(1);
                let mut b = Block::new((quote * 2) as f32 + depth as f32 * 2.0 + 1.0);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "\u{2022} ".to_string(),
                };
                b.runs.push(Run::new(marker, fg, Face::Prose));
                cur = Some(b);
                item_open = true;
            }
            Event::End(TagEnd::Item) => {
                item_open = false;
                finish(&mut out, &mut cur);
            }
            Event::TaskListMarker(done) => {
                if let Some(b) = cur.as_mut() {
                    b.runs.push(Run::new(if done { "[x] " } else { "[ ] " }, fg, Face::Code));
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                finish(&mut out, &mut cur);
                let info = match kind {
                    CodeBlockKind::Fenced(i) => i.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((info, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((info, body)) = code.take() {
                    let body = body.strip_suffix('\n').unwrap_or(&body);
                    let lang = detect::for_type_hint(&info).and_then(|c| c.into_iter().next()).unwrap_or_else(|| detect::guess(body, &[]));
                    let ind = indent(quote, &lists);
                    let lines = match &lang {
                        Language::Code(name) => code_lines(body, name, palette, theme, ind, true),
                        _ => plain_lines(body, fg, ind, true),
                    };
                    out.extend(lines);
                    spacer(&mut out);
                }
            }
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => link = None,
            Event::Start(Tag::Table(_)) => {
                finish(&mut out, &mut cur);
                table = Some((Vec::new(), 0));
            }
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                if let Some((rows, _)) = table.as_mut() {
                    rows.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some((rows, head)) = table.as_mut() {
                    *head = rows.len();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(row) = table.as_mut().and_then(|(rows, _)| rows.last_mut()) {
                    row.push(String::new());
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((rows, head)) = table.take() {
                    out.extend(table_lines(&rows, head, fg, indent(quote, &lists)));
                    spacer(&mut out);
                }
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                if let Some((_, body)) = code.as_mut() {
                    body.push_str(&t);
                } else if let Some(cell) = table.as_mut().and_then(|(rows, _)| rows.last_mut()).and_then(|r| r.last_mut()) {
                    cell.push_str(&t);
                } else {
                    let b = cur.get_or_insert_with(|| Block::new(indent(quote, &lists)));
                    b.runs.push(style(&t, Face::Prose, italic, bold + heading as u32, &link));
                }
            }
            Event::Code(t) => {
                if let Some(cell) = table.as_mut().and_then(|(rows, _)| rows.last_mut()).and_then(|r| r.last_mut()) {
                    cell.push_str(&t);
                } else {
                    let b = cur.get_or_insert_with(|| Block::new(indent(quote, &lists)));
                    b.runs.push(style(&t, Face::Code, italic, bold, &link));
                }
            }
            Event::SoftBreak => {
                if let Some(b) = cur.as_mut() {
                    b.runs.push(style(" ", Face::Prose, italic, bold, &link));
                }
            }
            Event::HardBreak => {
                let ind = cur.as_ref().map_or(0.0, |b| b.indent);
                finish(&mut out, &mut cur);
                cur = Some(Block::new(ind));
            }
            Event::Rule => {
                finish(&mut out, &mut cur);
                out.push(Block { rule: true, ..Block::new(indent(quote, &lists)) });
                spacer(&mut out);
            }
            _ => {}
        }
    }
    finish(&mut out, &mut cur);
    while out.last().is_some_and(|b| b.runs.is_empty() && !b.rule) {
        out.pop();
    }
    Doc { blocks: out }
}

fn heading_index(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 0,
        HeadingLevel::H2 => 1,
        HeadingLevel::H3 => 2,
        HeadingLevel::H4 => 3,
        HeadingLevel::H5 => 4,
        HeadingLevel::H6 => 5,
    }
}

/// A table as aligned rows in the profile's font: the header bold, a rule under it.
fn table_lines(rows: &[Vec<String>], head: usize, fg: Rgb, indent: f32) -> Vec<Block> {
    use unicode_width::UnicodeWidthStr;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols).map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.width()).max().unwrap_or(0)).collect();
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..cols).map(|c| {
            let s = row.get(c).map_or("", String::as_str);
            format!("{s}{}", " ".repeat(widths[c].saturating_sub(s.width())))
        }).collect();
        let mut b = Block::new(indent);
        b.runs.push(Run { bold: i < head, ..Run::new(cells.join(" \u{2502} "), fg, Face::Code) });
        out.push(b);
        if i + 1 == head {
            let mut r = Block::new(indent);
            let line: Vec<String> = widths.iter().map(|w| "\u{2500}".repeat(*w)).collect();
            r.runs.push(Run::new(line.join("\u{2500}\u{253c}\u{2500}"), fg, Face::Code));
            out.push(r);
        }
    }
    out
}

/// A laid-out line: its runs, where it starts, and its height.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualLine {
    pub runs: Vec<Run>,
    /// Each run's left edge (from the line's `x`) and width.
    pub spans: Vec<(f32, f32)>,
    pub x: f32,
    pub y: f32,
    pub h: f32,
    pub scale: f32,
    pub code_bg: bool,
    pub rule: bool,
    /// The document block it belongs to (a wrapped paragraph's lines share one).
    pub block: usize,
}

/// A place in a layout: a visual line and a character in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub lines: Vec<VisualLine>,
    pub height: f32,
    /// The widest line (Wide mode scrolls across it).
    pub width: f32,
}

impl Layout {
    /// The lines that show between `y0` and `y1`.
    pub fn visible(&self, y0: f32, y1: f32) -> &[VisualLine] {
        &self.lines[self.visible_range(y0, y1)]
    }

    /// The indices of those lines.
    pub fn visible_range(&self, y0: f32, y1: f32) -> std::ops::Range<usize> {
        let start = self.lines.partition_point(|l| l.y + l.h <= y0);
        let end = self.lines.partition_point(|l| l.y < y1);
        start..end.max(start)
    }

    /// A visual line's text.
    pub fn line_text(&self, i: usize) -> String {
        self.lines.get(i).map_or(String::new(), |l| l.runs.iter().map(|r| r.text.as_str()).collect())
    }

    /// Each character of a line with the run it is in.
    fn chars(&self, i: usize) -> Vec<(char, &Run)> {
        self.lines.get(i).map_or(Vec::new(), |l| l.runs.iter().flat_map(|r| r.text.chars().map(move |c| (c, r))).collect())
    }

    /// The character at `x`, `y` (from the text's top left); a line's end past its last
    /// character. None below the last line or above the first.
    pub fn position(&self, x: f32, y: f32, measure: &dyn Fn(&str, Face, f32) -> f32) -> Option<Pos> {
        if y < 0.0 {
            return None;
        }
        let line = self.lines.partition_point(|l| l.y + l.h <= y);
        let l = self.lines.get(line)?;
        let mut at = l.x;
        let chars = self.chars(line);
        for (col, (c, run)) in chars.iter().enumerate() {
            let w = measure(&c.to_string(), run.face, l.scale);
            if x < at + w {
                return Some(Pos { line, col });
            }
            at += w;
        }
        Some(Pos { line, col: chars.len() })
    }

    /// Where a position's character starts, from the text's left.
    pub fn x_of(&self, pos: Pos, measure: &dyn Fn(&str, Face, f32) -> f32) -> f32 {
        let Some(l) = self.lines.get(pos.line) else { return 0.0 };
        l.x + self.chars(pos.line).iter().take(pos.col).map(|(c, r)| measure(&c.to_string(), r.face, l.scale)).sum::<f32>()
    }

    fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
        if a <= b { (a, b) } else { (b, a) }
    }

    /// The runs of each visual line between two positions, cut to them, and whether each
    /// line ends its paragraph (not a wrap).
    pub fn rows_between(&self, a: Pos, b: Pos) -> Vec<(Vec<Run>, bool)> {
        if self.lines.is_empty() {
            return Vec::new();
        }
        let (a, b) = Self::ordered(a, b);
        let mut out = Vec::new();
        for i in a.line..=b.line.min(self.lines.len().saturating_sub(1)) {
            let (from, to) = (if i == a.line { a.col } else { 0 }, if i == b.line { b.col } else { usize::MAX });
            let mut runs: Vec<Run> = Vec::new();
            let mut col = 0;
            for run in &self.lines[i].runs {
                let n = run.text.chars().count();
                let (s, e) = (from.max(col), to.min(col + n));
                if s < e {
                    let text: String = run.text.chars().skip(s - col).take(e - s).collect();
                    runs.push(Run { text, ..run.clone() });
                }
                col += n;
            }
            let hard = self.lines.get(i + 1).is_none_or(|n| n.block != self.lines[i].block);
            out.push((runs, hard));
        }
        out
    }

    /// The plain text between two positions: wrapped lines join, paragraphs end in a newline.
    pub fn text(&self, a: Pos, b: Pos) -> String {
        let rows = self.rows_between(a, b);
        let n = rows.len();
        let mut out = String::new();
        for (i, (runs, hard)) in rows.into_iter().enumerate() {
            out.extend(runs.iter().map(|r| r.text.as_str()));
            if hard && i + 1 < n {
                out.push('\n');
            }
        }
        out
    }

    /// Matches in the rendered text, line by line.
    pub fn find(&self, re: &regex::Regex) -> Vec<(Pos, Pos)> {
        self.find_in(re, 0..self.lines.len())
    }

    /// Matches in some of the lines (those in view).
    pub fn find_in(&self, re: &regex::Regex, lines: std::ops::Range<usize>) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        for i in lines.start.min(self.lines.len())..lines.end.min(self.lines.len()) {
            let t = self.line_text(i);
            for m in re.find_iter(&t) {
                if m.start() == m.end() {
                    continue;
                }
                let col = |byte: usize| t[..byte].chars().count();
                out.push((Pos { line: i, col: col(m.start()) }, Pos { line: i, col: col(m.end()) }));
            }
        }
        out
    }

    /// The link a character belongs to.
    pub fn link_at(&self, pos: Pos) -> Option<&str> {
        let l = self.lines.get(pos.line)?;
        let mut col = 0;
        for run in &l.runs {
            let n = run.text.chars().count();
            if pos.col < col + n {
                return run.link.as_deref();
            }
            col += n;
        }
        None
    }
}

/// Pieces of a run's text that wrap as units: words with the spaces after them.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, ch) in text.char_indices() {
        if ch == ' ' {
            in_space = true;
        } else if in_space {
            out.push(&text[start..i]);
            start = i;
            in_space = false;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Lays a document out `width` pixels wide (no wrapping when `wide`). `measure` gives a
/// text's width in a face at a scale.
pub fn layout(doc: &Doc, width: f32, cell_h: f32, measure: &dyn Fn(&str, Face, f32) -> f32, wide: bool) -> Layout {
    let cell_w = measure(" ", Face::Code, 1.0).max(1.0);
    let mut out = Layout::default();
    let mut y = 0.0;
    for (block, b) in doc.blocks.iter().enumerate() {
        let x = b.indent * cell_w;
        let avail = (width - x).max(cell_w);
        let h = cell_h * b.scale;
        let mut line: Vec<Run> = Vec::new();
        let mut w = 0.0f32;
        let mut lines: Vec<(Vec<Run>, f32)> = Vec::new();
        let add = |line: &mut Vec<Run>, run: &Run, text: &str| match line.last_mut() {
            Some(last) if last.same_style(run) => last.text.push_str(text),
            _ => line.push(Run { text: text.to_string(), ..run.clone() }),
        };
        for run in &b.runs {
            for tok in tokens(&run.text) {
                let tw = measure(tok, run.face, b.scale);
                let fit = measure(tok.trim_end(), run.face, b.scale);
                if !wide && w > 0.0 && w + fit > avail {
                    lines.push((std::mem::take(&mut line), w));
                    w = 0.0;
                }
                if !wide && fit > avail {
                    // A word wider than the line breaks where it must.
                    let mut piece = String::new();
                    for ch in tok.chars() {
                        let mut next = piece.clone();
                        next.push(ch);
                        if !piece.is_empty() && w + measure(&next, run.face, b.scale) > avail {
                            add(&mut line, run, &piece);
                            lines.push((std::mem::take(&mut line), w + measure(&piece, run.face, b.scale)));
                            w = 0.0;
                            piece.clear();
                        }
                        piece.push(ch);
                    }
                    let pw = measure(&piece, run.face, b.scale);
                    add(&mut line, run, &piece);
                    w += pw;
                    continue;
                }
                add(&mut line, run, tok);
                w += tw;
            }
        }
        if !line.is_empty() || lines.is_empty() {
            lines.push((line, w));
        }
        for (runs, lw) in lines {
            out.width = out.width.max(x + lw);
            let mut at = 0.0;
            let spans = runs
                .iter()
                .map(|r| {
                    let w = measure(&r.text, r.face, b.scale);
                    at += w;
                    (at - w, w)
                })
                .collect();
            out.lines.push(VisualLine { runs, spans, x, y, h, scale: b.scale, code_bg: b.code_bg, rule: b.rule, block });
            y += h;
        }
    }
    out.height = y;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Palette;

    fn text(b: &Block) -> String {
        b.runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn run<'a>(doc: &'a Doc, s: &str) -> &'a Run {
        doc.blocks.iter().flat_map(|b| &b.runs).find(|r| r.text == s).unwrap_or_else(|| panic!("no run {s:?} in {doc:#?}"))
    }

    #[test]
    fn plain_text_follows_the_doc() {
        let p = Palette::default();
        let doc = render("# T\n\nsome **bold** text\n\n- a\n- b", &Language::Markdown, &p, true);
        let joined = doc.blocks.iter().map(text).collect::<Vec<_>>().join("\n");
        assert_eq!(doc.plain(), joined);
        assert_eq!(doc.plain(), "T\n\nsome bold text\n\n\u{2022} a\n\u{2022} b");
        let ruled = Doc { blocks: vec![Block::new(0.0), Block { rule: true, ..Block::new(0.0) }] };
        assert_eq!(ruled.plain(), "\n");
        assert_eq!(Doc::default().plain(), "");
    }

    #[test]
    fn markdown_blocks_and_styles() {
        let p = Palette::default();
        let md = "# Title\n\nSome *em* and **strong** and `code`, [link](https://x.org).\n\n- a\n- b\n\n1. one\n\n> quoted\n\n---\n\n```rust\nfn x() {}\n```\n";
        let doc = render(md, &Language::Markdown, &p, true);
        let blocks: Vec<&Block> = doc.blocks.iter().filter(|b| !b.runs.is_empty() || b.rule).collect();
        assert_eq!((text(blocks[0]).as_str(), blocks[0].scale), ("Title", 2.0));
        assert!(run(&doc, "em").italic);
        assert!(run(&doc, "strong").bold);
        assert_eq!(run(&doc, "code").face, Face::Code);
        assert_eq!(run(&doc, "link").link.as_deref(), Some("https://x.org"));
        assert!(run(&doc, "link").underline);
        assert_eq!(run(&doc, "em").color, p.fg, "prose is the default foreground");
        assert!(blocks.iter().any(|b| text(b) == "\u{2022} a" && b.indent > 0.0), "bullets");
        assert!(blocks.iter().any(|b| text(b) == "1. one"), "numbers");
        assert!(blocks.iter().any(|b| text(b) == "quoted" && b.indent > 0.0), "quotes are indented");
        assert!(blocks.iter().any(|b| b.rule), "a rule");
        let code = blocks.iter().find(|b| b.code_bg).expect("a code block");
        assert_eq!(text(code), "fn x() {}");
        assert_eq!(code.runs[0].color, p.ansi[13], "fenced code is highlighted by its info string");
    }

    #[test]
    fn json_keeps_order_and_bools() {
        let p = Palette::default();
        let doc = render("{\"b\":true,\"a\":[1,null]}", &Language::Json, &p, true);
        let lines: Vec<String> = doc.blocks.iter().map(text).collect();
        assert_eq!(lines, vec!["{", "    \"b\": true,", "    \"a\": [", "        1,", "        null", "    ]", "}"]);
        assert_eq!(run(&doc, "\"b\"").color, crate::palette::rgb(102, 255, 102), "keys green");
        assert_eq!(run(&doc, "true").color, crate::palette::rgb(255, 102, 102), "true, not 1");
        assert_eq!(run(&doc, "null").color, crate::palette::rgb(128, 128, 128));
        let light = render("[1]", &Language::Json, &p, false);
        assert_eq!(run(&light, "1").color, crate::palette::rgb(178, 51, 51));
    }

    #[test]
    fn json_error_shows_red() {
        let p = Palette::default();
        let doc = render("{bad", &Language::Json, &p, true);
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].runs[0].color, crate::palette::rgb(255, 102, 102));
        assert!(!text(&doc.blocks[0]).is_empty());
    }

    #[test]
    fn code_colors_from_palette() {
        let p = Palette::default();
        let rust = Language::Code("Rust".into());
        let dark = render("fn main() {} // hi", &rust, &p, true);
        assert_eq!(run(&dark, "fn").color, p.ansi[13], "keywords bright magenta on dark");
        assert!(dark.blocks[0].runs.iter().any(|r| r.text.contains("hi") && r.color == p.ansi[6]), "comments cyan on dark");
        let light = render("fn main() {} // hi", &rust, &p, false);
        assert_eq!(run(&light, "fn").color, p.ansi[5]);
        assert!(light.blocks[0].runs.iter().any(|r| r.text.contains("hi") && r.color == p.ansi[8]));
        let plain = render("a\nb", &Language::Plain, &p, true);
        assert_eq!(plain.blocks.iter().map(text).collect::<Vec<_>>(), vec!["a", "b"]);
    }

    fn measure(s: &str, _: Face, scale: f32) -> f32 {
        s.chars().count() as f32 * 10.0 * scale
    }

    #[test]
    fn wraps_to_width_unless_wide() {
        let p = Palette::default();
        let doc = render("aaa bbb ccc", &Language::Markdown, &p, true);
        let l = layout(&doc, 75.0, 20.0, &measure, false);
        let lines: Vec<String> = l.lines.iter().map(|v| v.runs.iter().map(|r| r.text.as_str()).collect()).collect();
        assert_eq!(lines, vec!["aaa bbb ", "ccc"]);
        assert_eq!(l.height, 40.0);
        let w = layout(&doc, 75.0, 20.0, &measure, true);
        assert_eq!(w.lines.len(), 1);
        assert_eq!(w.width, 110.0, "wide: the longest line's width");
        let long = render("abcdefghij", &Language::Plain, &p, true);
        assert_eq!(layout(&long, 45.0, 20.0, &measure, false).lines.len(), 3, "a word longer than the width breaks");
    }

    #[test]
    fn layout_is_bounded_by_the_view() {
        let p = Palette::default();
        let md: String = (0..5000).map(|i| format!("Line {i} with *some* words in it.\n\n")).collect();
        let t = std::time::Instant::now();
        let doc = render(&md, &Language::Markdown, &p, true);
        let l = layout(&doc, 800.0, 20.0, &measure, false);
        assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(l.lines.len() >= 5000);
        let seen = l.visible(1000.0, 1100.0);
        assert!(!seen.is_empty() && seen.len() <= 6, "only the lines in view: {}", seen.len());
        assert!(seen.iter().all(|v| v.y + v.h > 1000.0 && v.y < 1100.0));
    }

    #[test]
    fn porthole_copy_uses_rendered_text() {
        let p = Palette::default();
        let doc = render("# Title\n\nsome **bold** words here", &Language::Markdown, &p, true);
        let l = layout(&doc, 100.0, 20.0, &measure, false);
        let texts: Vec<String> = (0..l.lines.len()).map(|i| l.line_text(i)).collect();
        assert_eq!(texts, vec!["Title", "", "some bold ", "words here"]);
        let end = Pos { line: 3, col: 10 };
        assert_eq!(l.text(Pos { line: 0, col: 0 }, end), "Title\n\nsome bold words here", "wrapped lines of one paragraph join");
        assert_eq!(l.text(end, Pos { line: 2, col: 5 }), "bold words here", "either order");
        let rows = l.rows_between(Pos { line: 2, col: 5 }, Pos { line: 2, col: 9 });
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.iter().any(|r| r.text == "bold" && r.bold), "styles kept for Copy with Styles");
        assert_eq!(l.position(15.0, 65.0, &measure), Some(Pos { line: 2, col: 1 }), "x 15 is in the second character");
        assert_eq!(l.position(999.0, 65.0, &measure), Some(Pos { line: 2, col: 10 }), "past the end: the line's end");
        assert_eq!(l.position(0.0, 999.0, &measure), None);
        assert_eq!(l.x_of(Pos { line: 2, col: 5 }, &measure), 50.0);
    }

    #[test]
    fn find_sees_rendered_text() {
        let p = Palette::default();
        let doc = render("# Heading\n\nfind **me** and me", &Language::Markdown, &p, true);
        let l = layout(&doc, 1000.0, 20.0, &measure, false);
        let re = regex::Regex::new("me").unwrap();
        let hits = l.find(&re);
        assert_eq!(hits.len(), 2, "markdown markers are not searched: {hits:?}");
        assert_eq!(hits[0], (Pos { line: 2, col: 5 }, Pos { line: 2, col: 7 }));
        let link = render("[here](https://x.org)", &Language::Markdown, &p, true);
        let ll = layout(&link, 1000.0, 20.0, &measure, false);
        assert_eq!(ll.link_at(Pos { line: 0, col: 1 }), Some("https://x.org"));
        assert_eq!(ll.link_at(Pos { line: 0, col: 9 }), None);
    }

    #[test]
    fn css_placeholders_and_rules() {
        let p = Palette::default();
        let css = ".hljs { color: ${fg}; background: ${bg}; }\n/* a comment */\n.hljs-keyword,\n.hljs-selector-tag { color: ${red}; font-weight: bold }\n.hljs-string{color:#0f0;font-style:italic}\n.hljs-number { color: #123456 }";
        let t = Theme::from_css(css, &p);
        assert_eq!(t.style("keyword"), (p.ansi[1], true, false));
        assert_eq!(t.style("selector-tag"), (p.ansi[1], true, false));
        assert_eq!(t.style("string"), (crate::palette::rgb(0, 255, 0), false, true), "#rgb and italic");
        assert_eq!(t.style("number"), (crate::palette::rgb(0x12, 0x34, 0x56), false, false));
        assert_eq!(t.style("title"), (p.fg, false, false), "unnamed classes use the text color");
        let doc = render_themed("fn main() {}", &Language::Code("Rust".into()), &p, true, &t);
        assert_eq!(run(&doc, "fn").color, p.ansi[1]);
        assert!(run(&doc, "fn").bold);
    }

    #[test]
    fn bad_css_falls_to_template() {
        let p = Palette::default();
        let (t, err) = load_theme("C:\\no\\such\\file.css", &p, true);
        assert!(err.is_some_and(|e| e.contains("file.css")));
        assert_eq!(t, Theme::builtin(&p, true));
        let (t, err) = load_theme("", &p, false);
        assert!(err.is_none());
        assert_eq!(t, Theme::builtin(&p, false));
        assert_eq!(Theme::builtin(&p, true).style("keyword"), (p.ansi[13], false, false), "the dark template");
        assert_eq!(Theme::builtin(&p, false).style("comment"), (p.ansi[8], false, false), "the light template");
    }

    #[test]
    fn empty_layouts_and_bad_css_do_not_crash() {
        let p = Palette::default();
        let l = layout(&Doc::default(), 100.0, 20.0, &measure, false);
        let a = Pos { line: 0, col: 0 };
        assert!(l.rows_between(a, a).is_empty());
        assert_eq!(l.text(a, a), "");
        let t = Theme::from_css(".hljs-keyword { color: #\u{20ac}1; }", &p);
        assert_eq!(t.style("keyword").0, p.fg);
    }

    #[test]
    fn long_words_break_in_linear_time() {
        let p = Palette::default();
        let word = "x".repeat(200_000);
        let doc = render(&word, &Language::Plain, &p, true);
        let t = std::time::Instant::now();
        let l = layout(&doc, 800.0, 20.0, &measure, false);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(l.lines.len(), 2500);
    }

    #[test]
    fn find_in_a_range_of_lines() {
        let p = Palette::default();
        let doc = render("me\nme\nme\nme", &Language::Plain, &p, true);
        let l = layout(&doc, 100.0, 20.0, &measure, false);
        let re = regex::Regex::new("me").unwrap();
        assert_eq!(l.find_in(&re, 1..3).len(), 2);
        assert_eq!(l.find_in(&re, 3..99).len(), 1);
    }
}
