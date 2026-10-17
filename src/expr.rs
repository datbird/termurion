//! iTerm2's expression language (`EP`, `SUB`, `EV`): a tokenizer and precedence parser
//! into an `Expr` tree (function calls numbered by `site` in pre-order), interpolated
//! strings (`\( )` inside text), JSON values, and an evaluator that asks a `Calls` hook for
//! every call and may answer `Pending` (the caller replays the tree once the call is done).
//!
//! iTerm2 looks variables up while it parses (`EP:409-460`), so an undefined variable in a
//! whole expression (the top, a call argument, an array element) is a parse error that
//! stops everything, while one used as an operand has no value and fails only when its
//! operation runs ("Nil operand", `iTermParsedExpression.m:415-436`). The evaluator keeps
//! that split: `stat` is what the parse would have found, `run` what evaluation finds.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

pub use serde_json::Value;

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Bool(bool),
    Null,
    /// A string literal, interpolated when it holds `\( )`.
    Str(Vec<Part>),
    /// A variable; `optional` for `name?` (undefined is null).
    Path { path: String, optional: bool },
    /// `path[index]`, the only indexing (`EP:785-798`).
    Index { path: String, index: Box<Expr> },
    /// `[a, b]`, only as a whole expression (`EP:717-738`).
    Array(Vec<Expr>),
    Call(Call),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Bin(Box<Expr>, BinOp, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `expr ?`: an undefined value is null.
    Opt(Box<Expr>),
}

/// A piece of an interpolated string.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    fn text(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }
}

/// A function call: `namespace.name(arg: value, ...)`; `site` numbers the calls of one
/// parse in pre-order (a call before its arguments).
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub namespace: Option<String>,
    pub name: String,
    pub args: Vec<(String, Arg)>,
    pub site: usize,
}

impl Call {
    /// `namespace.name`, or `name` alone.
    pub fn fq_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}.{}", self.name),
            None => self.name.clone(),
        }
    }

    /// The call's signature: its fully qualified name and sorted argument names.
    pub fn signature(&self) -> String {
        let names: Vec<&str> = self.args.iter().map(|(n, _)| n.as_str()).collect();
        signature(self.namespace.as_deref(), &self.name, &names)
    }
}

/// An argument: a value, or `&path` (a reference to a user variable).
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Value(Expr),
    Ref(String),
}

/// A function's signature (`BIF:35-44`): `namespace.name(a,b)` with the argument names sorted.
pub fn signature(namespace: Option<&str>, name: &str, args: &[&str]) -> String {
    let mut args = args.to_vec();
    args.sort_unstable();
    let fq = match namespace {
        Some(ns) => format!("{ns}.{name}"),
        None => name.to_string(),
    };
    format!("{fq}({})", args.join(","))
}

/// A fully qualified name split at its last dot into namespace and name.
pub fn split_name(fq: &str) -> (Option<&str>, &str) {
    match fq.rfind('.') {
        Some(i) => (Some(&fq[..i]), &fq[i + 1..]),
        None => (None, fq),
    }
}

// ---------------------------------------------------------------------------------------
// Tokens (`EP:155-229`)

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// `name?` with no space (the path without the `?`).
    OptPath(String),
    Op(&'static str),
    Num(f64),
    True,
    False,
    Ident(String),
    /// A string's raw content, an interpolated string of its own.
    Str(String),
    /// A character no token starts with.
    Bad,
    End,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    /// Character index in the text.
    at: usize,
}

const TWO: [&str; 6] = ["==", "!=", "<=", ">=", "&&", "||"];
const ONE: [&str; 17] = ["<", ">", "(", ")", ":", ",", ".", "?", "[", "]", ";", "&", "+", "-", "*", "/", "!"];

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn ident_body(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The text's tokens; `used` counts the tokens of the whole parse (nested strings too),
/// which may not pass `MAX_TOKENS`.
fn tokenize(chars: &[char], used: &mut usize) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < chars.len() {
        if out.len() + *used >= MAX_TOKENS {
            return Err(too_long());
        }
        let c = chars[i];
        // OptionalPath first: an identifier path immediately followed by `?`.
        if ident_start(c) {
            let mut j = i + 1;
            while j < chars.len() && (ident_body(chars[j]) || chars[j] == '.') {
                j += 1;
            }
            if chars.get(j) == Some(&'?') {
                let path: String = chars[i..j].iter().collect();
                if !["true", "false", "null"].contains(&path.as_str()) && !path.ends_with('.') {
                    out.push(Token { tok: Tok::OptPath(path), at: i });
                    i = j + 1;
                    continue;
                }
            }
        }
        for op in TWO {
            let o = op.as_bytes();
            if c == o[0] as char && chars.get(i + 1) == Some(&(o[1] as char)) {
                out.push(Token { tok: Tok::Op(op), at: i });
                i += 2;
                continue 'outer;
            }
        }
        for op in ONE {
            if op.starts_with(c) {
                out.push(Token { tok: Tok::Op(op), at: i });
                i += 1;
                continue 'outer;
            }
        }
        if ident_start(c) {
            let mut j = i + 1;
            while j < chars.len() && ident_body(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let tok = match word.as_str() {
                "true" => Tok::True,
                "false" => Tok::False,
                _ => Tok::Ident(word),
            };
            out.push(Token { tok, at: i });
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
                j += 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if matches!(chars.get(j), Some('e' | 'E')) {
                let mut k = j + 1;
                if matches!(chars.get(k), Some('+' | '-')) {
                    k += 1;
                }
                if chars.get(k).is_some_and(char::is_ascii_digit) {
                    j = k;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                }
            }
            let text: String = chars[i..j].iter().collect();
            out.push(Token { tok: Tok::Num(text.parse().unwrap_or(0.0)), at: i });
            i = j;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"'
            && let Some(end) = swifty(&chars[i + 1..], true).end {
                out.push(Token { tok: Tok::Str(chars[i + 1..i + 1 + end].iter().collect()), at: i });
                i += end + 2;
                continue;
            }
        out.push(Token { tok: Tok::Bad, at: i });
        break;
    }
    *used += out.len();
    out.push(Token { tok: Tok::End, at: chars.len() });
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Interpolated strings (`iTermSwiftyStringParser.m`)

struct Swifty {
    /// Literal (false) and expression (true) pieces, literals unexpanded.
    pieces: Vec<(bool, String)>,
    /// With `stop_at_quote`: the index of the closing quote.
    end: Option<usize>,
    /// The index of the `\` of an expression left open at the end.
    open: Option<usize>,
}

/// iTerm2's swifty string scanner: `\(` opens an expression, parentheses nest, a string
/// inside an expression may hold `\( )` of its own.
fn swifty(chars: &[char], stop_at_quote: bool) -> Swifty {
    #[derive(PartialEq)]
    enum St {
        Lit,
        LitEsc,
        Expr,
        ExprStr,
        ExprStrEsc,
    }
    let mut st = St::Lit;
    let mut pieces = Vec::new();
    let (mut start, mut parens, mut open) = (0, 0, 0);
    let mut stack: Vec<i32> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        match st {
            St::Lit => {
                if c == '\\' {
                    st = St::LitEsc;
                } else if c == '"' && stop_at_quote {
                    return Swifty { pieces, end: Some(i), open: None };
                }
            }
            St::LitEsc => {
                if c == '(' {
                    if i - 1 > start {
                        pieces.push((false, chars[start..i - 1].iter().collect()));
                    }
                    open = i - 1;
                    start = i + 1;
                    parens = 1;
                    st = St::Expr;
                } else {
                    st = St::Lit;
                }
            }
            St::Expr => {
                if c == '(' {
                    parens += 1;
                } else if c == ')' {
                    parens -= 1;
                    if parens == 0 {
                        if let Some(p) = stack.pop() {
                            parens = p;
                            st = St::ExprStr;
                        } else {
                            pieces.push((true, chars[start..i].iter().collect()));
                            start = i + 1;
                            st = St::Lit;
                        }
                    }
                } else if c == '"' {
                    st = St::ExprStr;
                }
            }
            St::ExprStr => {
                if c == '\\' {
                    st = St::ExprStrEsc;
                } else if c == '"' {
                    st = St::Expr;
                }
            }
            St::ExprStrEsc => {
                if c == '(' {
                    stack.push(parens);
                    parens = 1;
                    st = St::Expr;
                } else {
                    st = St::ExprStr;
                }
            }
        }
    }
    // An expression left open: the rest is literal text (`iTermSwiftyStringParser.m:125-131`).
    let open = matches!(st, St::Expr | St::ExprStr | St::ExprStrEsc).then_some(open);
    if start < chars.len() {
        pieces.push((false, chars[start..].iter().collect()));
    }
    Swifty { pieces, end: None, open }
}

/// A literal's backslash escapes expanded (`NSStringITerm.m:456-504`): `\n \r \t \a \b \f
/// \e \\ \" \/` and `\uXXXX`; any other backslash stays.
fn expand_escapes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let e = chars[i + 1];
        let simple = match e {
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'f' => Some('\x0c'),
            'e' => Some('\x1b'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            '/' => Some('/'),
            _ => None,
        };
        if let Some(r) = simple {
            out.push(r);
            i += 2;
            continue;
        }
        if e == 'u' && i + 5 < chars.len() {
            let hex: String = chars[i + 2..i + 6].iter().collect();
            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().filter(|_| hex.chars().all(|h| h.is_ascii_hexdigit())).and_then(char::from_u32) {
                out.push(ch);
                i += 6;
                continue;
            }
        }
        out.push('\\');
        out.push(e);
        i += 2;
    }
    out
}

/// An interpolated string split into literal text (escapes expanded) and the raw text of
/// each `\( )` (`true`), as iTerm2 splits it (`EP:343-407`): the rest of an unclosed `\(`
/// is literal text, and a string with no expression is its whole text expanded.
pub fn template_pieces(template: &str) -> Vec<(bool, String)> {
    let chars: Vec<char> = template.chars().collect();
    let s = swifty(&chars, false);
    if !s.pieces.iter().any(|(expr, _)| *expr) {
        return vec![(false, expand_escapes(template))];
    }
    s.pieces.into_iter().map(|(expr, t)| if expr { (true, t) } else { (false, expand_escapes(&t)) }).collect()
}

/// The column (1-based) of an unclosed `\(`, whose rest iTerm2 reads as text.
pub fn unclosed_at(template: &str) -> Option<usize> {
    let chars: Vec<char> = template.chars().collect();
    swifty(&chars, false).open.map(|at| at + 1)
}

fn template_at(template: &str, b: &mut Budget) -> Result<Vec<Part>, String> {
    let mut parts: Vec<Part> = Vec::new();
    for (expr, text) in template_pieces(template) {
        if expr {
            parts.push(Part::Expr(parse_at(&text, b)?.0));
        } else if let Some(Part::Text(last)) = parts.last_mut() {
            last.push_str(&text);
        } else {
            parts.push(Part::Text(text));
        }
    }
    Ok(parts)
}

/// Parses an interpolated string; the calls of all its parts are numbered together.
pub fn parse_template(template: &str) -> Result<Vec<Part>, String> {
    template_at(template, &mut Budget::default())
}

// ---------------------------------------------------------------------------------------
// The parser (`EP:465-807`)
//
// Terminal output reaches the parser (OSC 1337 SetBadgeFormat, trigger parameters), so a
// parse is bounded: at most `MAX_TOKENS` tokens (nested strings included) and a tree and
// a parser nesting of at most `MAX_DEPTH` levels. Evaluating and dropping the largest
// tree accepted then recurses at most `MAX_DEPTH` deep.

/// The deepest tree (and parser nesting) a parse accepts.
pub const MAX_DEPTH: usize = 64;
/// The most tokens a parse accepts, nested strings included.
pub const MAX_TOKENS: usize = 4096;

fn too_deep() -> String {
    format!("Expression is nested too deeply (the limit is {MAX_DEPTH} levels).")
}

fn too_long() -> String {
    format!("Expression is too long (the limit is {MAX_TOKENS} tokens).")
}

/// What a parse and the strings nested in it share: the next call site, the tokens used
/// and the parser's nesting.
#[derive(Default)]
struct Budget {
    site: usize,
    tokens: usize,
    nest: usize,
}

/// What may start a primary expression, in Termurion's order.
const PRIMARY: [&str; 9] = ["(", "!", "-", "Number", "true", "false", "Identifier", "OptionalPath", "SwiftyString"];
/// What may follow a complete operand.
const FOLLOW: [&str; 13] = ["*", "/", "+", "-", "<", ">", "<=", ">=", "==", "!=", "&&", "||", "?"];

/// The binary operators by precedence, loosest first.
fn binop(t: &Tok) -> Option<(usize, BinOp)> {
    let Tok::Op(o) = t else { return None };
    Some(match *o {
        "||" => (0, BinOp::Or),
        "&&" => (1, BinOp::And),
        "==" => (2, BinOp::Eq),
        "!=" => (2, BinOp::Ne),
        "<" => (3, BinOp::Lt),
        ">" => (3, BinOp::Gt),
        "<=" => (3, BinOp::Le),
        ">=" => (3, BinOp::Ge),
        "+" => (4, BinOp::Add),
        "-" => (4, BinOp::Sub),
        "*" => (5, BinOp::Mul),
        "/" => (5, BinOp::Div),
        _ => return None,
    })
}

/// An expression and the depth of its tree.
type Parsed = (Expr, usize);

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Token>,
    pos: usize,
    b: &'a mut Budget,
    /// The operand just parsed was a bare path (it could have gone on with `.`, `(` or `[`).
    after_path: bool,
}

fn quote(t: &str) -> String {
    format!("\u{201c}{t}\u{201d}")
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    /// Consumes a token; the end stays put.
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn eat(&mut self, op: &str) -> bool {
        let yes = self.is(op);
        if yes {
            self.next();
        }
        yes
    }

    /// An identifier, consumed only when there is one.
    fn ident(&mut self) -> Option<String> {
        let Tok::Ident(n) = self.peek() else { return None };
        let n = n.clone();
        self.next();
        Some(n)
    }

    /// A node of depth `d`, refused past `MAX_DEPTH`.
    fn node(&self, d: usize) -> Result<usize, String> {
        if d > MAX_DEPTH {
            Err(too_deep())
        } else {
            Ok(d)
        }
    }

    /// "Syntax error at index N of \u{201c}text\u{201d}. Expected ..." (`EP:850-865`).
    fn syntax(&self, expected: &[&str]) -> String {
        let at = self.toks[self.pos].at;
        let list: Vec<String> = expected.iter().map(|t| quote(t)).collect();
        if list.len() == 1 {
            format!("Syntax error at index {at} of {}. Expected: {}", quote(self.text), list[0])
        } else {
            format!("Syntax error at index {at} of {}. Expected one of: {}", quote(self.text), list.join(", "))
        }
    }

    /// The tokens that could follow the operand just parsed, then `closers`.
    fn after_operand(&self, closers: &[&'static str]) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Vec::new();
        if self.after_path {
            v.extend(["(", ".", "["]);
        }
        v.extend(FOLLOW);
        v.extend(closers);
        v
    }

    fn expect_after(&mut self, op: &'static str, closers: &[&'static str]) -> Result<(), String> {
        if self.eat(op) {
            Ok(())
        } else {
            Err(self.syntax(&self.after_operand(closers)))
        }
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Tok::Num(_) | Tok::True | Tok::False | Tok::Ident(_) | Tok::OptPath(_) | Tok::Str(_) => true,
            Tok::Op(o) => ["(", "!", "-"].contains(o),
            _ => false,
        }
    }

    fn path_rest(&mut self, first: String) -> Result<String, String> {
        let mut path = first;
        while self.eat(".") {
            let Some(p) = self.ident() else { return Err(self.syntax(&["Identifier"])) };
            path.push('.');
            path.push_str(&p);
        }
        Ok(path)
    }

    /// `expression ::= Subexpression | '[' ']' | '[' comma_delimited_expressions ']'`. An
    /// array literal is a nesting of the parser too, counted as `conditional` counts one.
    fn expression(&mut self, closers: &[&'static str]) -> Result<Parsed, String> {
        if !self.eat("[") {
            return self.conditional(closers);
        }
        self.b.nest += 1;
        if self.b.nest > MAX_DEPTH {
            return Err(too_deep());
        }
        let r = self.array();
        self.b.nest -= 1;
        r
    }

    /// The items of an array literal after its `[`.
    fn array(&mut self) -> Result<Parsed, String> {
        let mut items = Vec::new();
        let mut d = 0;
        if !self.eat("]") {
            loop {
                let (e, ed) = self.expression(&[",", "]"])?;
                items.push(e);
                d = d.max(ed);
                if self.eat(",") {
                    continue;
                }
                self.expect_after("]", &[",", "]"])?;
                break;
            }
        }
        self.after_path = false;
        Ok((Expr::Array(items), self.node(d + 1)?))
    }

    /// `ConditionalExpression`: the ternary (right-nested) and `expr ?`. Every nesting of
    /// the parser passes here, so this is where it is counted.
    fn conditional(&mut self, closers: &[&'static str]) -> Result<Parsed, String> {
        self.b.nest += 1;
        if self.b.nest > MAX_DEPTH {
            return Err(too_deep());
        }
        let r = self.conditional_body(closers);
        self.b.nest -= 1;
        r
    }

    fn conditional_body(&mut self, closers: &[&'static str]) -> Result<Parsed, String> {
        let (c, cd) = self.binary(0)?;
        if !self.eat("?") {
            return Ok((c, cd));
        }
        if !self.starts_primary() {
            self.after_path = false;
            return Ok((Expr::Opt(Box::new(c)), self.node(cd + 1)?));
        }
        let (t, td) = self.conditional(&[":"])?;
        self.expect_after(":", &[":"])?;
        let (f, fd) = self.conditional(closers)?;
        Ok((Expr::Cond(Box::new(c), Box::new(t), Box::new(f)), self.node(1 + cd.max(td).max(fd))?))
    }

    /// The binary operators by precedence climbing: left-associative, at most six calls
    /// deep for one nesting.
    fn binary(&mut self, min: usize) -> Result<Parsed, String> {
        let (mut l, mut d) = self.unary()?;
        while let Some((prec, op)) = binop(self.peek()) {
            if prec < min {
                break;
            }
            self.next();
            let (r, rd) = self.binary(prec + 1)?;
            d = self.node(1 + d.max(rd))?;
            l = Expr::Bin(Box::new(l), op, Box::new(r));
        }
        Ok((l, d))
    }

    /// Unary `!` and `-`, gathered without recursion.
    fn unary(&mut self) -> Result<Parsed, String> {
        let mut ops = Vec::new();
        while self.is("!") || self.is("-") {
            ops.push(self.is("!"));
            self.next();
        }
        let (mut e, mut d) = self.primary()?;
        for not in ops.into_iter().rev() {
            d = self.node(d + 1)?;
            e = if not { Expr::Not(Box::new(e)) } else { Expr::Neg(Box::new(e)) };
        }
        Ok((e, d))
    }

    fn primary(&mut self) -> Result<Parsed, String> {
        self.after_path = false;
        let at = self.pos;
        let parsed = match self.next() {
            Tok::Num(n) => (Expr::Num(n), 1),
            Tok::True => (Expr::Bool(true), 1),
            Tok::False => (Expr::Bool(false), 1),
            Tok::Str(raw) => self.string(&raw)?,
            Tok::OptPath(path) => (Expr::Path { path, optional: true }, 1),
            Tok::Op("(") => {
                let e = self.conditional(&[")"])?;
                self.expect_after(")", &[")"])?;
                self.after_path = false;
                e
            }
            Tok::Ident(first) => self.reference(first)?,
            _ => {
                self.pos = at;
                return Err(self.syntax(&PRIMARY));
            }
        };
        Ok(parsed)
    }

    /// A nested string, one more level of the parser. The heavier arms of `primary` are
    /// functions of their own to keep its stack frame small.
    #[inline(never)]
    fn string(&mut self, raw: &str) -> Result<Parsed, String> {
        self.b.nest += 1;
        if self.b.nest > MAX_DEPTH {
            return Err(too_deep());
        }
        let parts = template_at(raw, self.b)?;
        self.b.nest -= 1;
        let d = parts.iter().map(|p| if let Part::Expr(e) = p { depth(e) } else { 0 }).max().unwrap_or(0);
        Ok((Expr::Str(parts), self.node(d + 1)?))
    }

    /// A path, a call, an indexed path or `null`.
    #[inline(never)]
    fn reference(&mut self, first: String) -> Result<Parsed, String> {
        let path = self.path_rest(first)?;
        if self.is("(") {
            let (c, d) = self.call(path)?;
            Ok((Expr::Call(c), d))
        } else if self.eat("[") {
            let (index, d) = self.conditional(&["]"])?;
            self.expect_after("]", &["]"])?;
            Ok((Expr::Index { path, index: Box::new(index) }, self.node(d + 1)?))
        } else if path == "null" {
            Ok((Expr::Null, 1))
        } else {
            self.after_path = true;
            Ok((Expr::Path { path, optional: false }, 1))
        }
    }

    /// `call ::= path arglist`; the next token is `(`.
    #[inline(never)]
    fn call(&mut self, fq: String) -> Result<(Call, usize), String> {
        let site = self.b.site;
        self.b.site += 1;
        self.next();
        let mut args = Vec::new();
        let mut d = 0;
        if !self.eat(")") {
            loop {
                let Some(name) = self.ident() else { return Err(self.syntax(&["Identifier"])) };
                if !self.eat(":") {
                    return Err(self.syntax(&[":"]));
                }
                let arg = if self.eat("&") {
                    let Some(first) = self.ident() else { return Err(self.syntax(&["Identifier"])) };
                    let path = self.path_rest(first)?;
                    if path == "null" {
                        return Err("&null is never allowed".into());
                    }
                    if !path.starts_with("user.") {
                        return Err(cannot_reference(&path));
                    }
                    args.push((name, Arg::Ref(path)));
                    if self.eat(",") {
                        continue;
                    }
                    // Only more of the path, or the next argument, can follow a reference.
                    if !self.eat(")") {
                        return Err(self.syntax(&[".", ",", ")"]));
                    }
                    break;
                } else {
                    let (e, ed) = self.expression(&[",", ")"])?;
                    d = d.max(ed);
                    Arg::Value(e)
                };
                args.push((name, arg));
                if self.eat(",") {
                    continue;
                }
                self.expect_after(")", &[",", ")"])?;
                break;
            }
        }
        self.after_path = false;
        let (namespace, name) = split_name(&fq);
        Ok((Call { namespace: namespace.map(str::to_string), name: name.to_string(), args, site }, self.node(d + 1)?))
    }

    /// `callsequence ::= callsequence ';' call | call`.
    fn calls(&mut self) -> Result<Vec<Call>, String> {
        let mut out = Vec::new();
        loop {
            let Some(first) = self.ident() else { return Err(self.syntax(&["Identifier"])) };
            let path = self.path_rest(first)?;
            if !self.is("(") {
                return Err(self.syntax(&["(", "."]));
            }
            out.push(self.call(path)?.0);
            if self.eat(";") {
                continue;
            }
            if *self.peek() != Tok::End {
                return Err(self.syntax(&[";", "EOF"]));
            }
            return Ok(out);
        }
    }
}

/// A tree's depth; the parser has bounded it by `MAX_DEPTH`.
fn depth(e: &Expr) -> usize {
    1 + match e {
        Expr::Num(_) | Expr::Bool(_) | Expr::Null | Expr::Path { .. } => 0,
        Expr::Str(parts) => parts.iter().map(|p| if let Part::Expr(e) = p { depth(e) } else { 0 }).max().unwrap_or(0),
        Expr::Index { index, .. } => depth(index),
        Expr::Array(items) => items.iter().map(depth).max().unwrap_or(0),
        Expr::Call(c) => c.args.iter().map(|(_, a)| if let Arg::Value(e) = a { depth(e) } else { 0 }).max().unwrap_or(0),
        Expr::Not(x) | Expr::Neg(x) | Expr::Opt(x) => depth(x),
        Expr::Bin(l, _, r) => depth(l).max(depth(r)),
        Expr::Cond(c, t, f) => depth(c).max(depth(t)).max(depth(f)),
    }
}

fn cannot_reference(path: &str) -> String {
    format!("Can\u{2019}t form reference to non-existent or read-only container for variable {path}")
}

fn parser<'a>(text: &'a str, b: &'a mut Budget) -> Result<Parser<'a>, String> {
    let chars: Vec<char> = text.chars().collect();
    let toks = tokenize(&chars, &mut b.tokens)?;
    Ok(Parser { text, toks, pos: 0, b, after_path: false })
}

fn parse_at(text: &str, b: &mut Budget) -> Result<Parsed, String> {
    let mut p = parser(text, b)?;
    let e = p.expression(&["EOF"])?;
    if *p.peek() != Tok::End {
        return Err(p.syntax(&p.after_operand(&["EOF"])));
    }
    Ok(e)
}

/// Parses an expression (iTerm2's `expression` start).
pub fn parse_expression(text: &str) -> Result<Expr, String> {
    parse_at(text, &mut Budget::default()).map(|(e, _)| e)
}

/// Parses a call or a call sequence (iTerm2's `callsequence` start); sites are numbered
/// across the whole sequence. A value is a syntax error.
pub fn parse_calls(text: &str) -> Result<Vec<Call>, String> {
    let mut b = Budget::default();
    parser(text, &mut b)?.calls()
}

/// The signature of an invocation holding one call (`EP:38-90`).
pub fn call_signature(invocation: &str) -> Result<String, String> {
    let calls = parse_calls(invocation)?;
    match calls.as_slice() {
        [one] => Ok(one.signature()),
        _ => Err("Expected single function call".into()),
    }
}

/// True when the expression calls a function anywhere, strings included.
pub fn contains_call(e: &Expr) -> bool {
    match e {
        Expr::Num(_) | Expr::Bool(_) | Expr::Null | Expr::Path { .. } => false,
        Expr::Call(_) => true,
        Expr::Str(parts) => parts.iter().any(|p| matches!(p, Part::Expr(e) if contains_call(e))),
        Expr::Index { index, .. } => contains_call(index),
        Expr::Array(items) => items.iter().any(contains_call),
        Expr::Not(x) | Expr::Neg(x) | Expr::Opt(x) => contains_call(x),
        Expr::Bin(l, _, r) => contains_call(l) || contains_call(r),
        Expr::Cond(c, t, f) => contains_call(c) || contains_call(t) || contains_call(f),
    }
}

// ---------------------------------------------------------------------------------------
// Scopes, calls and outcomes

/// Where variables come from.
pub trait Scope {
    fn value(&self, path: &str) -> Option<Value>;
    /// Whether `&path` may be formed (a user variable).
    fn writable(&self, path: &str) -> bool {
        path.starts_with("user.")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    Plain,
    /// No function is registered for the signature.
    Unregistered,
    Timeout,
    /// The Python API is off.
    Disabled,
}

/// Why an evaluation failed: the reason, a script's traceback, and the connection that
/// answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub reason: String,
    pub detail: Option<String>,
    pub kind: FailKind,
    pub conn: Option<u64>,
}

impl Failure {
    pub fn plain(reason: impl Into<String>) -> Failure {
        Failure { reason: reason.into(), detail: None, kind: FailKind::Plain, conn: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done(Value),
    Failed(Failure),
    /// A call has not answered yet.
    Pending,
}

/// Runs function calls; asked once per call whose arguments are all known.
pub trait Calls {
    fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>) -> Outcome;
    /// Whether `call`'s answer came only after waiting (it was `Pending` when first asked).
    /// An operator error keeps its "In <invocation>: " prefix only when neither operand
    /// waited (`SUB:806-846`); a replay that remembers finished calls says so here.
    fn waited(&self, _call: &Call) -> bool {
        false
    }
}


/// Refuses every call.
pub struct NoCalls;

pub const CALLS_NOT_ALLOWED: &str = "Function calls are not allowed here.";

impl Calls for NoCalls {
    fn call(&mut self, _call: &Call, _args: &BTreeMap<String, Value>) -> Outcome {
        Outcome::Failed(Failure::plain(CALLS_NOT_ALLOWED))
    }
}

/// The evaluation's context: the text being evaluated (named in operator errors) and
/// whether a bare undefined reference in an interpolated string is empty
/// (`laxNilPolicyInInterpolatedStrings`).
pub struct Ctx<'a> {
    pub invocation: &'a str,
    pub lax: bool,
}

/// "Reference to undefined variable ..." (`EP:290-304`).
pub fn undefined(path: &str) -> String {
    format!("Reference to undefined variable \u{201c}{path}\u{201d}. Change it to \u{201c}{path}?\u{201d} to treat the undefined value as null.")
}

/// A value's type as messages name it.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(_) | Value::Bool(_) => "number",
        Value::Array(_) => "array",
        Value::Object(_) => "dictionary",
        Value::Null => "null",
    }
}

// ---------------------------------------------------------------------------------------
// Values during evaluation: numbers stay doubles (infinity and NaN included, as NSNumber).

#[derive(Clone, Debug, PartialEq)]
enum V {
    Null,
    Num(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<V>),
    Obj(serde_json::Map<String, Value>),
}

impl V {
    fn from_json(v: Value) -> V {
        match v {
            Value::Null => V::Null,
            Value::Bool(b) => V::Bool(b),
            Value::Number(n) => V::Num(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => V::Str(s),
            Value::Array(a) => V::Arr(a.into_iter().map(V::from_json).collect()),
            Value::Object(o) => V::Obj(o),
        }
    }

    /// The JSON value; a whole number is an integer, infinity and NaN are null.
    fn to_json(&self) -> Value {
        match self {
            V::Null => Value::Null,
            V::Bool(b) => Value::Bool(*b),
            V::Num(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => Value::from(*n as i64),
            V::Num(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
            V::Str(s) => Value::String(s.clone()),
            V::Arr(a) => Value::Array(a.iter().map(V::to_json).collect()),
            V::Obj(o) => Value::Object(o.clone()),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            V::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            V::Null => "null",
            V::Num(_) | V::Bool(_) => "number",
            V::Str(_) => "string",
            V::Arr(_) => "array",
            V::Obj(_) => "dictionary",
        }
    }

    /// Interpolation's text (`EV:446-465`).
    fn text(&self) -> String {
        match self {
            V::Null => String::new(),
            V::Num(n) => num_text(*n),
            V::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            V::Str(s) => s.clone(),
            V::Arr(a) => format!("[{}]", a.iter().map(V::text).collect::<Vec<_>>().join(", ")),
            V::Obj(_) => json_text(&self.to_json()),
        }
    }
}

/// A number as NSNumber's `stringValue` writes it: a whole number without a fraction.
fn num_text(n: f64) -> String {
    if n.is_nan() {
        "nan".into()
    } else if n.is_infinite() {
        if n > 0.0 { "inf" } else { "-inf" }.into()
    } else if n.fract() == 0.0 && n.abs() < 1.0e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn json_number_text(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        num_text(n.as_f64().unwrap_or(0.0))
    }
}

/// A value's text in an interpolated string (`EV:446-465`): a string itself, a number as
/// NSNumber writes it (`3`, `0.5`, a boolean `1`), an array `[a, b]`, null nothing, a
/// dictionary its JSON.
pub fn to_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Number(n) => json_number_text(n),
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(to_text).collect::<Vec<_>>().join(", ")),
        Value::Object(_) => json_text(v),
    }
}

/// A value's JSON (`NSJSONSerialization+iTerm.m`): a top-level number (a boolean too) as
/// NSNumber's `stringValue`, collections as JSON with whole numbers written without a
/// fraction.
pub fn json_text(v: &Value) -> String {
    fn write(v: &Value, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&json_number_text(n)),
            Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, out);
                }
                out.push(']');
            }
            Value::Object(o) => {
                out.push('{');
                for (i, (k, x)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k).unwrap_or_default());
                    out.push(':');
                    write(x, out);
                }
                out.push('}');
            }
        }
    }
    match v {
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        _ => {
            let mut out = String::new();
            write(v, &mut out);
            out
        }
    }
}

// ---------------------------------------------------------------------------------------
// Operators (`SUB:86-377`)

/// Both operands as numbers when both convert (a string that reads as an integer or a
/// double), else neither.
fn numberize(a: &V, b: &V) -> (V, V) {
    fn num(v: &V) -> Option<f64> {
        match v {
            V::Str(s) => s.parse::<i64>().ok().map(|i| i as f64).or_else(|| s.parse::<f64>().ok()),
            other => other.number(),
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => (V::Num(x), V::Num(y)),
        _ => (a.clone(), b.clone()),
    }
}

fn lexicographic(a: &[V], b: &[V]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for (x, y) in a.iter().zip(b) {
        let o = match (x, y) {
            (V::Arr(x), V::Arr(y)) => lexicographic(x, y),
            (V::Str(x), V::Str(y)) => x.cmp(y),
            _ => match (x.number(), y.number()) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                _ => Ordering::Equal,
            },
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// NSArray's `isEqual:`: numbers by value, strings, nested arrays, nulls.
fn same(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Arr(x), V::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (V::Str(x), V::Str(y)) => x == y,
        (V::Null, V::Null) => true,
        (V::Obj(x), V::Obj(y)) => x == y,
        _ => match (a.number(), b.number()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn binary(op: BinOp, a: &V, b: &V) -> Result<V, String> {
    use std::cmp::Ordering;
    let (a, b) = numberize(a, b);
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            let (Some(x), Some(y)) = (a.number(), b.number()) else {
                return Err("Arithmetic operations require numeric operands".into());
            };
            Ok(V::Num(match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                _ => x / y,
            }))
        }
        BinOp::And | BinOp::Or => {
            let (Some(x), Some(y)) = (a.number(), b.number()) else {
                return Err("Logical operations require numeric operands".into());
            };
            let yes = if op == BinOp::And { x != 0.0 && y != 0.0 } else { x != 0.0 || y != 0.0 };
            Ok(V::Num(if yes { 1.0 } else { 0.0 }))
        }
        BinOp::Eq | BinOp::Ne => {
            let equal = match (&a, &b) {
                (V::Num(x), V::Num(y)) => x == y,
                (V::Str(x), V::Str(y)) => x == y,
                (V::Arr(x), V::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
                (V::Null, V::Null) => true,
                _ => false,
            };
            Ok(V::Bool(if op == BinOp::Eq { equal } else { !equal }))
        }
        _ => {
            let order = match (&a, &b) {
                (V::Num(x), V::Num(y)) => match x.partial_cmp(y) {
                    Some(o) => o,
                    None => return Ok(V::Bool(false)),
                },
                (V::Str(x), V::Str(y)) => x.cmp(y),
                (V::Arr(x), V::Arr(y)) => lexicographic(x, y),
                (V::Null, _) | (_, V::Null) => {
                    let both = a == V::Null && b == V::Null;
                    return Ok(V::Bool(matches!(op, BinOp::Le | BinOp::Ge) && both));
                }
                _ => {
                    return Err(format!(
                        "Cannot compare incompatible types with relational operators. Left-hand side is {}, right-hand side is {}",
                        a.type_name(),
                        b.type_name()
                    ))
                }
            };
            Ok(V::Bool(match op {
                BinOp::Lt => order == Ordering::Less,
                BinOp::Gt => order == Ordering::Greater,
                BinOp::Le => order != Ordering::Greater,
                _ => order != Ordering::Less,
            }))
        }
    }
}

fn truthy(v: &V) -> Result<bool, String> {
    match v {
        V::Null => Ok(false),
        other => other.number().map(|n| n != 0.0).ok_or_else(|| format!("Type mismatch: expected number but got {}", other.type_name())),
    }
}

fn nil_operand(what: &str) -> Failure {
    Failure::plain(format!("Subexpression error: Nil operand in {what}"))
}

// ---------------------------------------------------------------------------------------
// Evaluation

/// What the parse would have made of an expression before `ConditionalExpression` takes it
/// as a whole: fine, an undefined reference, an optional undefined one (null), or an error.
#[derive(Clone)]
enum Stat {
    Ok,
    Undef(String),
    OptNil,
    Err(Failure),
}

fn lookup(scope: &dyn Scope, path: &str) -> Option<Value> {
    scope.value(path).filter(|v| !v.is_null())
}

fn not_array(path: &str, v: &Value) -> String {
    format!("Variable \u{201c}{path}\u{201d} is of type {}, not array", type_name(v))
}

/// A bare reference (`EP:380-398`: a path or an indexed path, perhaps optionalized), which
/// the lax policy turns into "" when it fails.
fn bare(e: &Expr) -> bool {
    match e {
        Expr::Path { .. } | Expr::Index { .. } => true,
        Expr::Opt(x) => bare(x),
        _ => false,
    }
}

#[derive(Debug)]
enum R {
    Val(V),
    Fail(Failure),
    Pending,
}

/// Several results together: the last failure, else pending, else the values.
fn gather(results: Vec<R>) -> Result<Vec<V>, R> {
    let mut vals = Vec::new();
    let (mut fail, mut pending) = (None, false);
    for r in results {
        match r {
            R::Val(v) => vals.push(v),
            R::Fail(f) => fail = Some(f),
            R::Pending => pending = true,
        }
    }
    match (fail, pending) {
        (Some(f), _) => Err(R::Fail(f)),
        (None, true) => Err(R::Pending),
        (None, false) => Ok(vals),
    }
}

/// One evaluation. Each node's static classification and each index's element are worked
/// out once and kept by node address, so an evaluation is linear in the tree's size.
struct Ev<'a> {
    scope: &'a dyn Scope,
    cx: &'a Ctx<'a>,
    stats: RefCell<HashMap<*const Expr, Stat>>,
    elements: RefCell<HashMap<*const Expr, Result<V, Failure>>>,
    /// Calls so far that answered only after waiting (`Calls::waited`).
    waited: Cell<usize>,
}

impl<'a> Ev<'a> {
    fn new(scope: &'a dyn Scope, cx: &'a Ctx<'a>) -> Ev<'a> {
        Ev { scope, cx, stats: RefCell::default(), elements: RefCell::default(), waited: Cell::new(0) }
    }

    fn stat(&self, e: &Expr) -> Stat {
        let key = e as *const Expr;
        if let Some(s) = self.stats.borrow().get(&key) {
            return s.clone();
        }
        let s = self.stat_of(e);
        self.stats.borrow_mut().insert(key, s.clone());
        s
    }

    fn stat_of(&self, e: &Expr) -> Stat {
        match e {
            Expr::Path { path, optional } => match lookup(self.scope, path) {
                None if *optional => Stat::OptNil,
                None => Stat::Undef(path.clone()),
                Some(Value::Object(_)) => Stat::Err(Failure::plain(undefined(path))),
                Some(_) => Stat::Ok,
            },
            Expr::Index { path, index } => match lookup(self.scope, path) {
                None => Stat::Undef(path.clone()),
                // The index waits on a call: the rest is checked when it has a value.
                Some(Value::Array(_)) if contains_call(index) => Stat::Ok,
                Some(_) => match self.element(path, index) {
                    Ok(_) => Stat::Ok,
                    Err(f) => Stat::Err(f),
                },
            },
            Expr::Str(parts) => {
                for p in parts {
                    if let Part::Expr(x) = p
                        && let Some(f) = self.whole(x)
                            && !(self.cx.lax && bare(x)) {
                                return Stat::Err(f);
                            }
                }
                Stat::Ok
            }
            Expr::Call(c) => {
                for (_, a) in &c.args {
                    match a {
                        Arg::Value(x) => {
                            if let Some(f) = self.whole(x) {
                                return Stat::Err(f);
                            }
                        }
                        Arg::Ref(p) if p == "null" => return Stat::Err(Failure::plain("&null is never allowed")),
                        Arg::Ref(p) if !self.scope.writable(p) => return Stat::Err(Failure::plain(cannot_reference(p))),
                        Arg::Ref(_) => {}
                    }
                }
                Stat::Ok
            }
            Expr::Opt(x) => match self.stat(x) {
                Stat::Undef(_) => Stat::OptNil,
                other => other,
            },
            _ => Stat::Ok,
        }
    }

    /// The error a whole expression has before it runs (an undefined reference is one).
    fn whole(&self, e: &Expr) -> Option<Failure> {
        match self.stat(e) {
            Stat::Undef(p) => Some(Failure::plain(undefined(&p))),
            Stat::Err(f) => Some(f),
            Stat::Ok | Stat::OptNil => None,
        }
    }

    /// True when an operand has no value for its operation (`iTermParsedExpression.m:415-436`):
    /// an undefined reference, an error, an array held by a variable, or an interpolated
    /// string.
    fn operand_nil(&self, e: &Expr) -> bool {
        if matches!(self.stat(e), Stat::Undef(_) | Stat::Err(_)) {
            return true;
        }
        let mut base = e;
        while let Expr::Opt(x) = base {
            base = x;
        }
        match base {
            Expr::Path { path, .. } => matches!(lookup(self.scope, path), Some(Value::Array(_))),
            Expr::Index { path, index } if !contains_call(index) => matches!(self.element(path, index), Ok(V::Arr(_))),
            Expr::Str(parts) => parts.iter().any(|p| matches!(p, Part::Expr(_))),
            Expr::Array(_) => true,
            _ => false,
        }
    }

    /// `path[index]` for an index with no call (`EP:409-460`), worked out once.
    fn element(&self, path: &str, index: &Expr) -> Result<V, Failure> {
        let key = index as *const Expr;
        if let Some(r) = self.elements.borrow().get(&key) {
            return r.clone();
        }
        let r = self.element_of(path, index, None);
        self.elements.borrow_mut().insert(key, r.clone());
        r
    }

    /// `path[index]`; `known` is the index's value when a call made it.
    fn element_of(&self, path: &str, index: &Expr, known: Option<V>) -> Result<V, Failure> {
        let array = match lookup(self.scope, path) {
            None => return Err(Failure::plain(undefined(path))),
            Some(Value::Array(a)) => a,
            Some(v) => return Err(Failure::plain(not_array(path, &v))),
        };
        let value = match known {
            Some(v) => v,
            None => {
                if self.operand_nil(index) {
                    return Err(Failure::plain(format!("Array index for \u{201c}{path}\u{201d} must be a number, not {}", self.index_type(index))));
                }
                match self.run(index, &mut NoCalls) {
                    R::Val(v) => v,
                    R::Fail(f) => return Err(Failure::plain(format!("Error evaluating index expression: {}", f.reason))),
                    R::Pending => return Err(Failure::plain("Error evaluating index expression")),
                }
            }
        };
        let i = match &value {
            V::Str(_) => return Err(Failure::plain("Error evaluating index expression: Cannot convert string literal to number")),
            V::Null => return Err(Failure::plain("Error evaluating index expression: Cannot convert null to number")),
            v => match v.number() {
                Some(n) => n as i64,
                None => return Err(Failure::plain(format!("Array index for \u{201c}{path}\u{201d} must be a number, not {}", v.type_name()))),
            },
        };
        if i < 0 || i as usize >= array.len() {
            let n = array.len();
            return Err(Failure::plain(if n == 1 {
                format!("Index {i} out of range of \u{201c}{path}\u{201d}, which has 1 value")
            } else {
                format!("Index {i} out of range of \u{201c}{path}\u{201d}, which has {n} values")
            }));
        }
        match &array[i as usize] {
            Value::Null | Value::Object(_) => Err(Failure::plain(undefined(path))),
            v => Ok(V::from_json(v.clone())),
        }
    }

    /// The type an index with no value reports.
    fn index_type(&self, index: &Expr) -> &'static str {
        match index {
            Expr::Path { path, .. } => lookup(self.scope, path).map_or("null", |v| type_name(&v)),
            Expr::Str(_) => "string",
            _ => "null",
        }
    }

    fn run(&self, e: &Expr, calls: &mut dyn Calls) -> R {
        if let Expr::Opt(x) = e {
            return match self.stat(x) {
                Stat::Undef(_) => R::Val(V::Null),
                _ => self.run(x, calls),
            };
        }
        if let Some(f) = self.whole(e) {
            return R::Fail(f);
        }
        match e {
            Expr::Num(n) => R::Val(V::Num(*n)),
            Expr::Bool(b) => R::Val(V::Bool(*b)),
            Expr::Null => R::Val(V::Null),
            Expr::Str(parts) => self.run_template(parts, calls),
            Expr::Path { path, .. } => R::Val(lookup(self.scope, path).map_or(V::Null, V::from_json)),
            Expr::Index { path, index } => self.run_index(path, index, calls),
            Expr::Array(items) => match gather(items.iter().map(|x| self.run(x, calls)).collect()) {
                Ok(vals) => R::Val(V::Arr(vals)),
                Err(r) => r,
            },
            Expr::Call(c) => self.run_call(c, calls),
            Expr::Not(x) => self.run_unary(x, true, calls),
            Expr::Neg(x) => self.run_unary(x, false, calls),
            Expr::Bin(l, op, r) => self.run_bin(l, *op, r, calls),
            Expr::Cond(c, t, f) => self.run_cond(c, t, f, calls),
            Expr::Opt(_) => unreachable!("handled above"),
        }
    }

    // The arms of `run` are functions of their own to keep its stack frame small.

    #[inline(never)]
    fn run_index(&self, path: &str, index: &Expr, calls: &mut dyn Calls) -> R {
        let r = if contains_call(index) {
            match self.run(index, calls) {
                R::Val(v) => self.element_of(path, index, Some(v)),
                other => return other,
            }
        } else {
            self.element(path, index)
        };
        match r {
            Ok(v) => R::Val(v),
            Err(f) => R::Fail(f),
        }
    }

    #[inline(never)]
    fn run_unary(&self, x: &Expr, not: bool, calls: &mut dyn Calls) -> R {
        if self.operand_nil(x) {
            return R::Fail(nil_operand(if not { "logical not operation" } else { "unary operation" }));
        }
        match self.run(x, calls) {
            R::Val(v) => match (v.number(), not) {
                (Some(n), true) => R::Val(V::Num(if n == 0.0 { 1.0 } else { 0.0 })),
                (Some(n), false) => R::Val(V::Num(-n)),
                (None, true) => R::Fail(Failure::plain(format!("Logical not requires numeric operand, got {}", v.type_name()))),
                (None, false) => R::Fail(Failure::plain("Cannot negate non-numeric value")),
            },
            other => other,
        }
    }

    #[inline(never)]
    fn run_bin(&self, l: &Expr, op: BinOp, r: &Expr, calls: &mut dyn Calls) -> R {
        if self.operand_nil(l) || self.operand_nil(r) {
            return R::Fail(nil_operand(&format!("binary operation ({})", op.text())));
        }
        // Both sides run (no short circuit); the right side's failure wins (SUB:806-825).
        let waited = self.waited.get();
        let (a, b) = (self.run(l, calls), self.run(r, calls));
        let (a, b) = match (a, b) {
            (_, R::Fail(f)) | (R::Fail(f), _) => return R::Fail(f),
            (R::Val(a), R::Val(b)) => (a, b),
            _ => return R::Pending,
        };
        match binary(op, &a, &b) {
            Ok(v) => R::Val(v),
            // An operand that waited on a call loses the prefix (SUB:826-846).
            Err(m) if self.waited.get() > waited => R::Fail(Failure::plain(m)),
            Err(m) => R::Fail(Failure::plain(format!("In {}: {m}", self.cx.invocation))),
        }
    }

    #[inline(never)]
    fn run_cond(&self, c: &Expr, t: &Expr, f: &Expr, calls: &mut dyn Calls) -> R {
        if [c, t, f].iter().any(|x| self.operand_nil(x)) {
            return R::Fail(nil_operand("ternary operation"));
        }
        match self.run(c, calls) {
            R::Val(v) => match truthy(&v) {
                Ok(true) => self.run(t, calls),
                Ok(false) => self.run(f, calls),
                Err(m) => R::Fail(Failure::plain(m)),
            },
            other => other,
        }
    }

    /// A call: its arguments together (`SFC:586-620`), then the hook once all are known. A
    /// failing argument that is a call becomes "In call to <name>: ..." unless no function
    /// was registered for it (`SFC:655-679`).
    fn run_call(&self, c: &Call, calls: &mut dyn Calls) -> R {
        let mut results = Vec::new();
        for (_, a) in &c.args {
            results.push(match a {
                Arg::Ref(p) => R::Val(V::Str(p.clone())),
                Arg::Value(x) => match self.run(x, calls) {
                    R::Fail(f) => match x {
                        Expr::Call(inner) if f.kind != FailKind::Unregistered => {
                            R::Fail(Failure { reason: format!("In call to {}: {}", inner.name, f.reason), kind: FailKind::Plain, ..f })
                        }
                        _ => R::Fail(f),
                    },
                    other => other,
                },
            });
        }
        // The first failing argument, else wait for all.
        if let Some(i) = results.iter().position(|r| matches!(r, R::Fail(_))) {
            return results.swap_remove(i);
        }
        let vals = match gather(results) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let args: BTreeMap<String, Value> = c.args.iter().zip(vals).map(|((n, _), v)| (n.clone(), v.to_json())).collect();
        match calls.call(c, &args) {
            Outcome::Done(v) => {
                if calls.waited(c) {
                    self.waited.set(self.waited.get() + 1);
                }
                R::Val(V::from_json(v))
            }
            Outcome::Failed(f) => R::Fail(f),
            Outcome::Pending => R::Pending,
        }
    }

    /// An interpolated string (`EP:343-407`, `EV:338-414`): an error a part has before it
    /// runs stops the string (with `lax`, a bare reference's is ""); then every part runs.
    fn run_template(&self, parts: &[Part], calls: &mut dyn Calls) -> R {
        let mut skip = vec![false; parts.len()];
        for (i, p) in parts.iter().enumerate() {
            if let Part::Expr(x) = p
                && let Some(f) = self.whole(x) {
                    if self.cx.lax && bare(x) {
                        skip[i] = true;
                    } else {
                        return R::Fail(f);
                    }
                }
        }
        let results = parts
            .iter()
            .zip(skip)
            .map(|(p, skip)| match p {
                Part::Text(t) => R::Val(V::Str(t.clone())),
                Part::Expr(_) if skip => R::Val(V::Null),
                Part::Expr(x) => self.run(x, calls),
            })
            .collect();
        match gather(results) {
            Ok(vals) => R::Val(V::Str(vals.iter().map(V::text).collect())),
            Err(r) => r,
        }
    }
}

fn outcome(r: R) -> Outcome {
    match r {
        R::Val(v) => Outcome::Done(v.to_json()),
        R::Fail(f) => Outcome::Failed(f),
        R::Pending => Outcome::Pending,
    }
}

/// Evaluates an expression; every call goes to `calls`.
pub fn eval(e: &Expr, scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> Outcome {
    outcome(Ev::new(scope, cx).run(e, calls))
}

/// Evaluates an interpolated string to a `Value::String`.
pub fn eval_template(parts: &[Part], scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> Outcome {
    outcome(Ev::new(scope, cx).run_template(parts, calls))
}

#[cfg(test)]
fn run(e: &Expr, scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> R {
    Ev::new(scope, cx).run(e, calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Fix(BTreeMap<String, Value>);

    impl Scope for Fix {
        fn value(&self, path: &str) -> Option<Value> {
            self.0.get(path).cloned()
        }
    }

    fn fix() -> Fix {
        let mut m = BTreeMap::new();
        m.insert("user.x".to_string(), Value::from("42"));
        m.insert("user.list".to_string(), serde_json::json!(["a", "b"]));
        m.insert("user.one".to_string(), serde_json::json!(["a"]));
        m.insert("columns".to_string(), Value::from(80));
        Fix(m)
    }

    fn run_text(text: &str, calls: &mut dyn Calls) -> Outcome {
        let e = parse_expression(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        eval(&e, &fix(), calls, &Ctx { invocation: text, lax: true })
    }

    fn ok(text: &str) -> Value {
        match run_text(text, &mut NoCalls) {
            Outcome::Done(v) => v,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn err(text: &str) -> String {
        match run_text(text, &mut NoCalls) {
            Outcome::Failed(f) => f.reason,
            other => panic!("{text}: {other:?}"),
        }
    }

    /// Answers every call by name from a table; the sites in `pending` wait, the names in
    /// `fail` fail with "boom".
    #[derive(Default)]
    struct Fake {
        values: BTreeMap<String, Value>,
        pending: BTreeSet<usize>,
        fail: BTreeSet<String>,
        asked: Vec<(String, usize)>,
        /// Every call answered only after waiting.
        waited: bool,
    }

    impl Calls for Fake {
        fn call(&mut self, call: &Call, _args: &BTreeMap<String, Value>) -> Outcome {
            self.asked.push((call.name.clone(), call.site));
            if self.fail.contains(&call.name) {
                return Outcome::Failed(Failure { detail: Some("tb".into()), ..Failure::plain("boom") });
            }
            if self.pending.contains(&call.site) {
                return Outcome::Pending;
            }
            Outcome::Done(self.values.get(&call.name).cloned().unwrap_or(Value::Null))
        }

        fn waited(&self, _call: &Call) -> bool {
            self.waited
        }
    }

    /// Runs `f` on a thread with the stack of a Windows main thread (1 MiB), in a debug
    /// build whose frames are several times a release build's. A stack overflow aborts the
    /// whole test binary, so these tests were run one at a time for RED.
    fn small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new().stack_size(1 << 20).spawn(f).expect("thread").join().expect("no panic")
    }

    /// Runs `f` on its own thread and fails the test when it takes more than `secs`.
    fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("thread");
        rx.recv_timeout(std::time::Duration::from_secs(secs)).expect("finished in time")
    }

    #[test]
    fn grammar_follows_iterm2() {
        assert_eq!(ok("1 + 2 * 3"), Value::from(7));
        assert_eq!(ok("(1 + 2) * 3"), Value::from(9));
        assert_eq!(ok("-3 + 1"), Value::from(-2));
        assert_eq!(ok("!0"), Value::from(1), "logical not gives a number (SUB:91-96)");
        assert_eq!(ok("!!2"), Value::from(1));
        assert_eq!(ok("1 < 2"), Value::Bool(true), "comparisons give booleans");
        assert_eq!(ok("1 < 2 && 2 < 3"), Value::from(1), "logic gives numbers (SUB:178-187)");
        assert_eq!(ok("0 || 0"), Value::from(0));
        assert_eq!(ok(r#"1 == 1 ? "a" : "b""#), Value::from("a"));
        assert_eq!(ok(r#"1 == 2 ? "a" : 3 > 2 ? "b" : "c""#), Value::from("b"), "the ternary nests to the right");
        assert_eq!(ok(r#"[1, "x", [2]]"#), serde_json::json!([1, "x", [2]]));
        assert_eq!(ok("true"), Value::Bool(true));
        assert_eq!(ok("null"), Value::Null);
        assert_eq!(ok(r#""a\tb\b\f\a\n\r""#), Value::from("a\tb\u{8}\u{c}\u{7}\n\r"));
        assert_eq!(ok(r#""v=\(1 + 1)""#), Value::from("v=2"));
        assert_eq!(ok("user.list[1]"), Value::from("b"));
        assert_eq!(ok("user.x?"), Value::from("42"));
        assert_eq!(ok("nope?"), Value::Null);
        assert_eq!(ok("nope ?"), Value::Null);

        let calls = parse_calls("f(a: 1, b: &user.x)").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].namespace, None);
        assert_eq!(calls[0].args, vec![("a".to_string(), Arg::Value(Expr::Num(1.0))), ("b".to_string(), Arg::Ref("user.x".into()))]);
        assert_eq!(calls[0].site, 0);
        let calls = parse_calls(r#"iterm2.alert(title: "t", subtitle: "", buttons: ["OK"]); g()"#).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].namespace.as_deref(), calls[0].name.as_str(), calls[0].site), (Some("iterm2"), "alert", 0));
        assert_eq!((calls[1].name.as_str(), calls[1].site), ("g", 1), "sites count across the sequence");
        let calls = parse_calls("f(x: g(y: 1))").unwrap();
        assert_eq!(calls[0].site, 0);
        match &calls[0].args[0].1 {
            Arg::Value(Expr::Call(g)) => assert_eq!((g.name.as_str(), g.site), ("g", 1)),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse_calls("f(x: &null)").unwrap_err(), "&null is never allowed");
        assert_eq!(parse_calls("f(x: &columns)").unwrap_err(), "Can\u{2019}t form reference to non-existent or read-only container for variable columns");
        for text in ["x", "1 + 2"] {
            let e = parse_calls(text).unwrap_err();
            assert!(e.starts_with("Syntax error at index"), "the call parser takes only calls: {e}");
        }
        let e = parse_expression("1 +").unwrap_err();
        assert!(e.starts_with("Syntax error at index 3 of \u{201c}1 +\u{201d}. Expected one of: \u{201c}"), "{e}");
        assert_eq!(parse_calls("x").unwrap_err(), "Syntax error at index 1 of \u{201c}x\u{201d}. Expected one of: \u{201c}(\u{201d}, \u{201c}.\u{201d}");
        assert_eq!(parse_calls("1 + 2").unwrap_err(), "Syntax error at index 0 of \u{201c}1 + 2\u{201d}. Expected: \u{201c}Identifier\u{201d}");
        assert!(parse_expression("[1] == [1]").is_err(), "an array is a whole expression only (EP:717-738)");
        assert!(parse_expression("x?1:2").is_err(), "x? is one token, as in iTerm2");
    }

    #[test]
    fn values_follow_iterm2() {
        assert_eq!(ok(r#""80" + 1"#), Value::from(81), "strings that read as numbers are numbers");
        assert_eq!(err(r#""a" + 1"#), "In \"a\" + 1: Arithmetic operations require numeric operands");
        assert_eq!(ok(r#""a" < "b""#), Value::Bool(true));
        assert_eq!(err(r#""a" < 1"#), "In \"a\" < 1: Cannot compare incompatible types with relational operators. Left-hand side is string, right-hand side is number");
        assert_eq!(ok(r#"1 == "1""#), Value::Bool(true));
        assert_eq!(ok(r#""a" == 1"#), Value::Bool(false));
        assert_eq!(ok("null == null"), Value::Bool(true));
        assert_eq!(ok("null < 1"), Value::Bool(false));
        assert_eq!(ok("null <= null"), Value::Bool(true));
        assert_eq!(err(r#"!"x""#), "Logical not requires numeric operand, got string");
        assert_eq!(err(r#"-"x""#), "Cannot negate non-numeric value");
        assert_eq!(err(r#""x" ? 1 : 2"#), "Type mismatch: expected number but got string");
        assert_eq!(ok("null ? 1 : 2"), Value::from(2));
        assert_eq!(ok("true + 1"), Value::from(2), "a boolean is a number, as NSNumber");
        assert_eq!(ok("0 / 0 == 0 / 0"), Value::Bool(false), "NaN is unequal to itself");
        assert_eq!(ok("1 / 0 > 1000000"), Value::Bool(true));
        let inf = run(&parse_expression("1 / 0").unwrap(), &fix(), &mut NoCalls, &Ctx { invocation: "1 / 0", lax: true });
        assert!(matches!(inf, R::Val(V::Num(n)) if n.is_infinite()), "{inf:?}");

        // Arrays compare as values; only a call can make one an operand (EP:717-738).
        let mut fake = Fake::default();
        fake.values.insert("f".into(), serde_json::json!([1, 2]));
        fake.values.insert("g".into(), serde_json::json!([1, 2]));
        fake.values.insert("h".into(), serde_json::json!([1, 3]));
        assert_eq!(run_text("f() == g()", &mut fake), Outcome::Done(Value::Bool(true)));
        assert_eq!(run_text("f() < h()", &mut fake), Outcome::Done(Value::Bool(true)));
        match run_text("f() + 1", &mut fake) {
            Outcome::Failed(f) => assert_eq!(f.reason, "In f() + 1: Arithmetic operations require numeric operands", "a call that answered at once keeps the prefix"),
            other => panic!("{other:?}"),
        }
        fake.waited = true;
        match run_text("f() + 1", &mut fake) {
            Outcome::Failed(f) => assert_eq!(f.reason, "Arithmetic operations require numeric operands", "no prefix when an operand waited on a call (SUB:826-846)"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn undefined_follows_iterm2() {
        assert_eq!(err("nope"), "Reference to undefined variable \u{201c}nope\u{201d}. Change it to \u{201c}nope?\u{201d} to treat the undefined value as null.");
        assert_eq!(err("nope + 1"), "Subexpression error: Nil operand in binary operation (+)", "an undefined operand has no value (iTermParsedExpression.m:415-436)");
        assert_eq!(err("!nope"), "Subexpression error: Nil operand in logical not operation");
        assert_eq!(err("1 ? 2 : nope"), "Subexpression error: Nil operand in ternary operation");
        assert_eq!(ok("nope? == null"), Value::Bool(true));
        assert_eq!(err("user.list[2]"), "Index 2 out of range of \u{201c}user.list\u{201d}, which has 2 values");
        assert_eq!(err("user.one[3]"), "Index 3 out of range of \u{201c}user.one\u{201d}, which has 1 value");
        assert_eq!(err("user.x[0]"), "Variable \u{201c}user.x\u{201d} is of type string, not array");
        assert_eq!(err("user.list[-1]"), "Index -1 out of range of \u{201c}user.list\u{201d}, which has 2 values");
        assert_eq!(err("nope[0]"), "Reference to undefined variable \u{201c}nope\u{201d}. Change it to \u{201c}nope?\u{201d} to treat the undefined value as null.");
        assert_eq!(err(r#"user.list["a"]"#), "Error evaluating index expression: Cannot convert string literal to number");
        assert_eq!(err("user.list == user.list"), "Subexpression error: Nil operand in binary operation (==)", "a variable holding an array is no operand");
    }

    #[test]
    fn text_follows_iterm2() {
        assert_eq!(to_text(&Value::from("s")), "s");
        assert_eq!(to_text(&serde_json::json!(3.0)), "3");
        assert_eq!(to_text(&serde_json::json!(0.5)), "0.5");
        assert_eq!(to_text(&Value::Bool(true)), "1");
        assert_eq!(to_text(&serde_json::json!([1, "a", [2]])), "[1, a, [2]]");
        assert_eq!(to_text(&Value::Null), "");
        assert_eq!(to_text(&serde_json::json!({"a": 1})), "{\"a\":1}");
        assert_eq!(json_text(&Value::from("s")), "\"s\"");
        assert_eq!(json_text(&serde_json::json!(3.0)), "3");
        assert_eq!(json_text(&Value::Bool(true)), "1", "NSNumber stringValue, kept");
        assert_eq!(json_text(&serde_json::json!([true, 1.5, 2.0])), "[true,1.5,2]");
        assert_eq!(json_text(&Value::Null), "null");
        assert_eq!(signature(Some("iterm2"), "alert", &["title", "buttons", "subtitle"]), "iterm2.alert(buttons,subtitle,title)");
        assert_eq!(signature(None, "f", &[]), "f()");
        assert_eq!(split_name("title.com.x_y.f"), (Some("title.com.x_y"), "f"));
        assert_eq!(split_name("f"), (None, "f"));
        assert_eq!(call_signature("f(b: 1, a: 2)").unwrap(), "f(a,b)");
        assert_eq!(call_signature("f(); g()").unwrap_err(), "Expected single function call");
        let c = &parse_calls("title.x.f(b: 1)").unwrap()[0];
        assert_eq!((c.fq_name(), c.signature()), ("title.x.f".to_string(), "title.x.f(b)".to_string()));
        assert_eq!(type_name(&serde_json::json!({})), "dictionary");
        assert_eq!(undefined("a.b"), "Reference to undefined variable \u{201c}a.b\u{201d}. Change it to \u{201c}a.b?\u{201d} to treat the undefined value as null.");
        assert!(contains_call(&parse_expression(r#""\(f())""#).unwrap()));
        assert!(!contains_call(&parse_expression("user.list[1] + 2").unwrap()));
        let parts = parse_template(r#"a\n\(1)\"bA"#).unwrap();
        assert_eq!(parts, vec![Part::Text("a\n".into()), Part::Expr(Expr::Num(1.0)), Part::Text("\"bA".into())]);
    }

    #[test]
    fn calls_are_asked_once_per_site() {
        let text = "f(x: g(), y: h())";
        let mut fake = Fake::default();
        fake.pending.insert(1);
        assert_eq!(run_text(text, &mut fake), Outcome::Pending);
        assert_eq!(fake.asked, vec![("g".to_string(), 1), ("h".to_string(), 2)], "both arguments in one pass, never f");

        let mut fake = Fake::default();
        fake.values.insert("f".into(), Value::from("done"));
        assert_eq!(run_text(text, &mut fake), Outcome::Done(Value::from("done")));
        assert_eq!(fake.asked.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["g", "h", "f"]);

        let mut fake = Fake::default();
        fake.fail.insert("g".into());
        match run_text(text, &mut fake) {
            Outcome::Failed(f) => assert_eq!((f.reason.as_str(), f.detail.as_deref()), ("In call to g: boom", Some("tb"))),
            other => panic!("{other:?}"),
        }
        assert!(!fake.asked.iter().any(|a| a.0 == "f"), "a failed argument stops before f");

        let mut fake = Fake::default();
        assert!(matches!(run_text("f(x: nope)", &mut fake), Outcome::Failed(_)));
        assert!(fake.asked.is_empty(), "an undefined argument is a parse error: nothing is called");
        assert_eq!(run_text("f()", &mut NoCalls), Outcome::Failed(Failure::plain(CALLS_NOT_ALLOWED)));
    }

    #[test]
    fn hostile_input_is_refused() {
        small_stack(|| {
            let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
            assert_eq!(parse_expression(&deep).unwrap_err(), too_long());
            assert_eq!(parse_expression(&format!("{}1{}", "(".repeat(1_000), ")".repeat(1_000))).unwrap_err(), too_deep());
            assert_eq!(parse_expression(&format!("{}1", "!".repeat(100_000))).unwrap_err(), too_long());
            assert_eq!(parse_expression(&format!("{}1", "!".repeat(1_000))).unwrap_err(), too_deep());
            assert_eq!(parse_expression(&format!("1{}", "+1".repeat(100_000))).unwrap_err(), too_long());
            assert_eq!(parse_expression(&format!("1{}", "+1".repeat(1_000))).unwrap_err(), too_deep(), "a flat chain deeper than the limit");
            let mut nested = "1".to_string();
            for _ in 0..5000 {
                nested = format!("\"\\({nested})\"");
            }
            assert_eq!(parse_expression(&nested).unwrap_err(), too_deep());
            let wrapped = format!("\\({deep})");
            assert!(!crate::interp::has_calls(&wrapped));
            assert!(crate::interp::evaluate(&wrapped, &|_| None, &[]).is_err());
            assert!(!crate::interp::has_calls(&format!("\\({nested})")));
            assert!(parse_calls(&format!("f(x: {deep})")).is_err());

            // A huge string is one token.
            let big = format!("\"{}\"", "a".repeat(1 << 20));
            assert_eq!(ok(&big), Value::from("a".repeat(1 << 20)));
            let template = format!("{}\\(1)", "b".repeat(1 << 20));
            assert_eq!(crate::interp::evaluate(&template, &|_| None, &[]).unwrap().len(), (1 << 20) + 1);

            // The largest trees accepted parse, evaluate and drop on this stack.
            let n = MAX_DEPTH - 1;
            assert_eq!(ok(&format!("{}1{}", "(".repeat(n), ")".repeat(n))), Value::from(1));
            assert_eq!(ok(&format!("1{}", "+1".repeat(n))), Value::from(MAX_DEPTH as i64));
            assert_eq!(ok(&format!("{}1", "!".repeat(n - 1))), Value::from(1));
            let mut strings = "user.x".to_string();
            for _ in 0..MAX_DEPTH / 2 - 1 {
                strings = format!("\"\\({strings})\"");
            }
            assert_eq!(ok(&strings), Value::from("42"));
            let wide = vec!["1"; MAX_TOKENS / 2 - 1].join("+");
            assert!(parse_expression(&wide).is_err(), "a wide tree is deep on its left");
            let wide = format!("[{}]", vec!["1"; MAX_TOKENS / 2 - 1].join(","));
            assert_eq!(ok(&wide).as_array().map(Vec::len), Some(MAX_TOKENS / 2 - 1));

            // Nested array literals count toward the depth limit, closed or not.
            assert_eq!(parse_expression(&"[".repeat(4000)).unwrap_err(), too_deep());
            assert_eq!(parse_expression(&format!("{}1{}", "[".repeat(2000), "]".repeat(2000))).unwrap_err(), too_deep());
            assert!(!crate::interp::has_calls(&format!("\\({})", "[".repeat(4000))));
        });
    }

    #[test]
    fn empty_calls_are_refused() {
        assert_eq!(parse_calls("").unwrap_err(), "Syntax error at index 0 of \u{201c}\u{201d}. Expected: \u{201c}Identifier\u{201d}");
        assert!(parse_calls("  ").unwrap_err().starts_with("Syntax error at index 2 of"), "the end of input");
        assert!(call_signature("").is_err());
        assert!(parse_calls("f(").unwrap_err().starts_with("Syntax error at index 2 of"), "the end of input is where the error is");
        assert_eq!(
            parse_calls("f(x: &user.a b)").unwrap_err(),
            "Syntax error at index 13 of \u{201c}f(x: &user.a b)\u{201d}. Expected one of: \u{201c}.\u{201d}, \u{201c},\u{201d}, \u{201c})\u{201d}"
        );
    }

    #[test]
    fn evaluation_is_linear() {
        let out = within(5, || {
            let mut text = "0".to_string();
            // Two levels of tree each: just under the limit.
            for _ in 0..(MAX_DEPTH - 2) / 2 {
                text = format!("user.zeros[{text} + 0]");
            }
            let mut m = BTreeMap::new();
            m.insert("user.zeros".to_string(), serde_json::json!([0]));
            let e = parse_expression(&text).unwrap();
            eval(&e, &Fix(m), &mut NoCalls, &Ctx { invocation: &text, lax: true })
        });
        assert_eq!(out, Outcome::Done(Value::from(0)));
        let out = within(5, || {
            let mut text = "user.x".to_string();
            // A nested string is two levels of the parser.
            for _ in 0..MAX_DEPTH / 2 - 1 {
                text = format!("\"\\({text})\"");
            }
            let mut opt = "nope".to_string();
            for _ in 0..MAX_DEPTH - 2 {
                opt = format!("({opt} ?)");
            }
            (run_text(&text, &mut NoCalls), run_text(&opt, &mut NoCalls))
        });
        assert_eq!(out, (Outcome::Done(Value::from("42")), Outcome::Done(Value::Null)));
    }
}
