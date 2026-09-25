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

pub fn render(text: &str, language: &Language, palette: &Palette, dark: bool) -> Doc {
    match language {
        Language::Markdown => markdown(text, palette, dark),
        Language::Json => json(text, palette, dark),
        Language::Plain => Doc { blocks: plain_lines(text, palette.fg, 0.0, false) },
        Language::Code(name) => Doc { blocks: code_lines(text, name, palette, dark, 0.0, false) },
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

/// iTerm2's highlight classes, grouped by the color its templates give them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Comment,
    /// variable, tag, name, regexp, link, meta.
    Tag,
    /// number, built_in, literal, type, params, deletion.
    Number,
    /// title, section, attribute.
    Title,
    /// string, symbol, bullet, addition.
    String,
    /// keyword, selector-tag.
    Keyword,
    Emphasis,
    Strong,
    /// Operators and everything unnamed: the text color.
    Plain,
}

/// Scope prefixes to classes, the more specific first.
fn scope_classes() -> &'static [(Scope, Class)] {
    static TABLE: OnceLock<Vec<(Scope, Class)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let rules: &[(&str, Class)] = &[
            ("comment", Class::Comment),
            ("string.regexp", Class::Tag),
            ("string", Class::String),
            ("constant.character", Class::String),
            ("markup.inserted", Class::String),
            ("constant.numeric", Class::Number),
            ("constant.language", Class::Number),
            ("support.type", Class::Number),
            ("support.class", Class::Number),
            ("support.function", Class::Number),
            ("variable.parameter", Class::Number),
            ("markup.deleted", Class::Number),
            ("storage.type.function", Class::Keyword),
            ("storage.type.struct", Class::Keyword),
            ("storage.type.enum", Class::Keyword),
            ("storage.type.class", Class::Keyword),
            ("storage.type.trait", Class::Keyword),
            ("storage.type.impl", Class::Keyword),
            ("storage.type.module", Class::Keyword),
            ("storage.type.type", Class::Keyword),
            ("storage.type.union", Class::Keyword),
            ("storage.type.interface", Class::Keyword),
            ("storage.type.namespace", Class::Keyword),
            ("storage.type", Class::Number),
            ("storage.modifier", Class::Keyword),
            ("keyword.operator", Class::Plain),
            ("keyword", Class::Keyword),
            ("entity.other.attribute-name.id", Class::Tag),
            ("entity.other.attribute-name.class", Class::Tag),
            ("entity.other.attribute-name", Class::Title),
            ("entity.name.tag", Class::Tag),
            ("entity.name", Class::Title),
            ("markup.heading", Class::Title),
            ("markup.underline.link", Class::Tag),
            ("meta.preprocessor", Class::Tag),
            ("variable.language", Class::Tag),
            ("markup.bold", Class::Strong),
            ("markup.italic", Class::Emphasis),
        ];
        rules.iter().map(|(s, c)| (Scope::new(s).expect("a valid scope"), *c)).collect()
    })
}

fn classify(stack: &ScopeStack) -> Class {
    for scope in stack.as_slice().iter().rev() {
        if let Some((_, c)) = scope_classes().iter().find(|(p, _)| p.is_prefix_of(*scope)) {
            return *c;
        }
    }
    Class::Plain
}

/// A class's color and style, from iTerm2's dark or light template.
pub fn class_style(class: Class, palette: &Palette, dark: bool) -> (Rgb, bool, bool) {
    let a = |i: usize| palette.ansi[i];
    match (class, dark) {
        (Class::Comment, true) => (a(6), false, false),
        (Class::Comment, false) => (a(8), false, false),
        (Class::Tag, true) => (a(9), false, false),
        (Class::Tag, false) => (a(1), false, false),
        (Class::Number, true) => (a(12), false, false),
        (Class::Number, false) => (a(4), false, false),
        (Class::Title, true) => (a(11), false, false),
        (Class::Title, false) => (a(3), false, false),
        (Class::String, true) => (a(10), false, false),
        (Class::String, false) => (a(2), false, false),
        (Class::Keyword, true) => (a(13), false, false),
        (Class::Keyword, false) => (a(5), false, false),
        (Class::Emphasis, _) => (palette.fg, false, true),
        (Class::Strong, _) => (palette.fg, true, false),
        (Class::Plain, _) => (palette.fg, false, false),
    }
}

/// Source lines highlighted with a syntect grammar; plain lines when it has none.
fn code_lines(text: &str, syntax: &str, palette: &Palette, dark: bool, indent: f32, code_bg: bool) -> Vec<Block> {
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
            let (color, bold, italic) = class_style(classify(&stack), palette, dark);
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
fn markdown(text: &str, palette: &Palette, dark: bool) -> Doc {
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
                        Language::Code(name) => code_lines(body, name, palette, dark, ind, true),
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
    pub x: f32,
    pub y: f32,
    pub h: f32,
    pub scale: f32,
    pub code_bg: bool,
    pub rule: bool,
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
        let start = self.lines.partition_point(|l| l.y + l.h <= y0);
        let end = self.lines.partition_point(|l| l.y < y1);
        &self.lines[start..end.max(start)]
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
    for b in &doc.blocks {
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
            out.lines.push(VisualLine { runs, x, y, h, scale: b.scale, code_bg: b.code_bg, rule: b.rule });
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
}
