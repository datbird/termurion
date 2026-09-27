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

use std::collections::BTreeMap;

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

fn tokenize(chars: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < chars.len() {
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
            let o: Vec<char> = op.chars().collect();
            if chars[i..].starts_with(&o) {
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
        if c == '"' {
            if let Some(end) = swifty(&chars[i + 1..], true).end {
                out.push(Token { tok: Tok::Str(chars[i + 1..i + 1 + end].iter().collect()), at: i });
                i += end + 2;
                continue;
            }
        }
        out.push(Token { tok: Tok::Bad, at: i });
        break;
    }
    out.push(Token { tok: Tok::End, at: chars.len() });
    out
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
    let open = matches!(st, St::Expr | St::ExprStr | St::ExprStrEsc).then_some(open);
    if open.is_none() && start < chars.len() {
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
/// each `\( )` (`true`); an unclosed `\(` is an error naming its column (1-based). Tandem
/// keeps that error where iTerm2 reads the rest as text.
pub fn template_pieces(template: &str) -> Result<Vec<(bool, String)>, String> {
    let chars: Vec<char> = template.chars().collect();
    let s = swifty(&chars, false);
    if let Some(at) = s.open {
        return Err(format!("Unclosed \\( at column {}.", at + 1));
    }
    Ok(s.pieces.into_iter().map(|(expr, t)| if expr { (true, t) } else { (false, expand_escapes(&t)) }).collect())
}

fn template_at(template: &str, site: &mut usize) -> Result<Vec<Part>, String> {
    let mut parts: Vec<Part> = Vec::new();
    for (expr, text) in template_pieces(template)? {
        if expr {
            parts.push(Part::Expr(parse_at(&text, site)?));
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
    template_at(template, &mut 0)
}

// ---------------------------------------------------------------------------------------
// The parser (`EP:465-807`)

/// What may start a primary expression, in Tandem's order.
const PRIMARY: [&str; 9] = ["(", "!", "-", "Number", "true", "false", "Identifier", "OptionalPath", "SwiftyString"];
/// What may follow a complete operand.
const FOLLOW: [&str; 13] = ["*", "/", "+", "-", "<", ">", "<=", ">=", "==", "!=", "&&", "||", "?"];

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Token>,
    pos: usize,
    site: &'a mut usize,
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
            match self.next() {
                Tok::Ident(p) => {
                    path.push('.');
                    path.push_str(&p);
                }
                _ => {
                    self.pos -= 1;
                    return Err(self.syntax(&["Identifier"]));
                }
            }
        }
        Ok(path)
    }

    /// `expression ::= Subexpression | '[' ']' | '[' comma_delimited_expressions ']'`.
    fn expression(&mut self, closers: &[&'static str]) -> Result<Expr, String> {
        if !self.eat("[") {
            return self.conditional(closers);
        }
        let mut items = Vec::new();
        if self.eat("]") {
            self.after_path = false;
            return Ok(Expr::Array(items));
        }
        loop {
            items.push(self.expression(&[",", "]"])?);
            if self.eat(",") {
                continue;
            }
            self.expect_after("]", &[",", "]"])?;
            self.after_path = false;
            return Ok(Expr::Array(items));
        }
    }

    /// `ConditionalExpression`: the ternary (right-nested) and `expr ?`.
    fn conditional(&mut self, closers: &[&'static str]) -> Result<Expr, String> {
        let c = self.binary(0, closers)?;
        if !self.eat("?") {
            return Ok(c);
        }
        if !self.starts_primary() {
            self.after_path = false;
            return Ok(Expr::Opt(Box::new(c)));
        }
        let t = self.conditional(&[":"])?;
        self.expect_after(":", &[":"])?;
        let f = self.conditional(closers)?;
        Ok(Expr::Cond(Box::new(c), Box::new(t), Box::new(f)))
    }

    /// The binary levels, loosest first: `||`, `&&`, `==` `!=`, relational, `+` `-`, `*` `/`.
    fn binary(&mut self, level: usize, closers: &[&'static str]) -> Result<Expr, String> {
        const LEVELS: [&[(&str, BinOp)]; 6] = [
            &[("||", BinOp::Or)],
            &[("&&", BinOp::And)],
            &[("==", BinOp::Eq), ("!=", BinOp::Ne)],
            &[("<", BinOp::Lt), (">", BinOp::Gt), ("<=", BinOp::Le), (">=", BinOp::Ge)],
            &[("+", BinOp::Add), ("-", BinOp::Sub)],
            &[("*", BinOp::Mul), ("/", BinOp::Div)],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut l = self.binary(level + 1, closers)?;
        'more: loop {
            for (text, op) in LEVELS[level] {
                if self.eat(text) {
                    let r = self.binary(level + 1, closers)?;
                    l = Expr::Bin(Box::new(l), *op, Box::new(r));
                    continue 'more;
                }
            }
            return Ok(l);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        self.after_path = false;
        let at = self.pos;
        let e = match self.next() {
            Tok::Num(n) => Expr::Num(n),
            Tok::True => Expr::Bool(true),
            Tok::False => Expr::Bool(false),
            Tok::Str(raw) => Expr::Str(template_at(&raw, &mut *self.site)?),
            Tok::OptPath(path) => Expr::Path { path, optional: true },
            Tok::Op("(") => {
                let e = self.conditional(&[")"])?;
                self.expect_after(")", &[")"])?;
                self.after_path = false;
                e
            }
            Tok::Ident(first) => {
                let path = self.path_rest(first)?;
                if self.is("(") {
                    Expr::Call(self.call(path)?)
                } else if self.eat("[") {
                    let index = self.conditional(&["]"])?;
                    self.expect_after("]", &["]"])?;
                    Expr::Index { path, index: Box::new(index) }
                } else if path == "null" {
                    Expr::Null
                } else {
                    self.after_path = true;
                    Expr::Path { path, optional: false }
                }
            }
            _ => {
                self.pos = at;
                return Err(self.syntax(&PRIMARY));
            }
        };
        Ok(e)
    }

    /// `call ::= path arglist`; the next token is `(`.
    fn call(&mut self, fq: String) -> Result<Call, String> {
        let site = *self.site;
        *self.site += 1;
        self.next();
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                let name = match self.next() {
                    Tok::Ident(n) => n,
                    _ => {
                        self.pos -= 1;
                        return Err(self.syntax(&["Identifier"]));
                    }
                };
                if !self.eat(":") {
                    return Err(self.syntax(&[":"]));
                }
                let arg = if self.eat("&") {
                    let path = match self.next() {
                        Tok::Ident(p) => self.path_rest(p)?,
                        _ => {
                            self.pos -= 1;
                            return Err(self.syntax(&["Identifier"]));
                        }
                    };
                    if path == "null" {
                        return Err("&null is never allowed".into());
                    }
                    if !path.starts_with("user.") {
                        return Err(cannot_reference(&path));
                    }
                    self.after_path = true;
                    Arg::Ref(path)
                } else {
                    Arg::Value(self.expression(&[",", ")"])?)
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
        Ok(Call { namespace: namespace.map(str::to_string), name: name.to_string(), args, site })
    }

    /// `callsequence ::= callsequence ';' call | call`.
    fn calls(&mut self) -> Result<Vec<Call>, String> {
        let mut out = Vec::new();
        loop {
            let first = match self.next() {
                Tok::Ident(p) => p,
                _ => {
                    self.pos -= 1;
                    return Err(self.syntax(&["Identifier"]));
                }
            };
            let path = self.path_rest(first)?;
            if !self.is("(") {
                return Err(self.syntax(&["(", "."]));
            }
            out.push(self.call(path)?);
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

fn cannot_reference(path: &str) -> String {
    format!("Can\u{2019}t form reference to non-existent or read-only container for variable {path}")
}

fn parser<'a>(text: &'a str, site: &'a mut usize) -> Parser<'a> {
    let chars: Vec<char> = text.chars().collect();
    Parser { text, toks: tokenize(&chars), pos: 0, site, after_path: false }
}

fn parse_at(text: &str, site: &mut usize) -> Result<Expr, String> {
    let mut p = parser(text, site);
    let e = p.expression(&["EOF"])?;
    if *p.peek() != Tok::End {
        return Err(p.syntax(&p.after_operand(&["EOF"])));
    }
    Ok(e)
}

/// Parses an expression (iTerm2's `expression` start).
pub fn parse_expression(text: &str) -> Result<Expr, String> {
    parse_at(text, &mut 0)
}

/// Parses a call or a call sequence (iTerm2's `callsequence` start); sites are numbered
/// across the whole sequence. A value is a syntax error.
pub fn parse_calls(text: &str) -> Result<Vec<Call>, String> {
    let mut site = 0;
    parser(text, &mut site).calls()
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
enum Stat {
    Ok,
    Undef(String),
    OptNil,
    Err(Failure),
}

fn lookup(scope: &dyn Scope, path: &str) -> Option<Value> {
    scope.value(path).filter(|v| !v.is_null())
}

fn stat(e: &Expr, scope: &dyn Scope, cx: &Ctx) -> Stat {
    match e {
        Expr::Path { path, optional } => match lookup(scope, path) {
            None if *optional => Stat::OptNil,
            None => Stat::Undef(path.clone()),
            Some(Value::Object(_)) => Stat::Err(Failure::plain(undefined(path))),
            Some(_) => Stat::Ok,
        },
        Expr::Index { path, index } => match lookup(scope, path) {
            None => Stat::Undef(path.clone()),
            // The index waits on a call: the rest is checked when it has a value.
            Some(Value::Array(_)) if contains_call(index) => Stat::Ok,
            Some(_) => match element(path, index, None, scope, cx) {
                Ok(_) => Stat::Ok,
                Err(f) => Stat::Err(f),
            },
        },
        Expr::Str(parts) => {
            for p in parts {
                if let Part::Expr(x) = p {
                    if let Some(f) = whole(x, scope, cx) {
                        if !(cx.lax && bare(x)) {
                            return Stat::Err(f);
                        }
                    }
                }
            }
            Stat::Ok
        }
        Expr::Call(c) => {
            for (_, a) in &c.args {
                match a {
                    Arg::Value(x) => {
                        if let Some(f) = whole(x, scope, cx) {
                            return Stat::Err(f);
                        }
                    }
                    Arg::Ref(p) if p == "null" => return Stat::Err(Failure::plain("&null is never allowed")),
                    Arg::Ref(p) if !scope.writable(p) => return Stat::Err(Failure::plain(cannot_reference(p))),
                    Arg::Ref(_) => {}
                }
            }
            Stat::Ok
        }
        Expr::Opt(x) => match stat(x, scope, cx) {
            Stat::Undef(_) => Stat::OptNil,
            other => other,
        },
        _ => Stat::Ok,
    }
}

/// The error a whole expression has before it runs (an undefined reference is one).
fn whole(e: &Expr, scope: &dyn Scope, cx: &Ctx) -> Option<Failure> {
    match stat(e, scope, cx) {
        Stat::Undef(p) => Some(Failure::plain(undefined(&p))),
        Stat::Err(f) => Some(f),
        Stat::Ok | Stat::OptNil => None,
    }
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

/// True when an operand has no value for its operation (`iTermParsedExpression.m:415-436`):
/// an undefined reference, an error, an array held by a variable, or an interpolated string.
fn operand_nil(e: &Expr, scope: &dyn Scope, cx: &Ctx) -> bool {
    if matches!(stat(e, scope, cx), Stat::Undef(_) | Stat::Err(_)) {
        return true;
    }
    let mut base = e;
    while let Expr::Opt(x) = base {
        base = x;
    }
    match base {
        Expr::Path { path, .. } => matches!(lookup(scope, path), Some(Value::Array(_))),
        Expr::Index { path, index } if !contains_call(index) => matches!(element(path, index, None, scope, cx), Ok(V::Arr(_))),
        Expr::Str(parts) => parts.iter().any(|p| matches!(p, Part::Expr(_))),
        Expr::Array(_) => true,
        _ => false,
    }
}

fn not_array(path: &str, v: &Value) -> String {
    format!("Variable \u{201c}{path}\u{201d} is of type {}, not array", type_name(v))
}

/// `path[index]` (`EP:409-460`); `known` is the index's value when a call made it.
fn element(path: &str, index: &Expr, known: Option<V>, scope: &dyn Scope, cx: &Ctx) -> Result<V, Failure> {
    let array = match lookup(scope, path) {
        None => return Err(Failure::plain(undefined(path))),
        Some(Value::Array(a)) => a,
        Some(v) => return Err(Failure::plain(not_array(path, &v))),
    };
    let value = match known {
        Some(v) => v,
        None => {
            if operand_nil(index, scope, cx) {
                return Err(Failure::plain(format!("Array index for \u{201c}{path}\u{201d} must be a number, not {}", index_type(index, scope))));
            }
            match run(index, scope, &mut NoCalls, cx) {
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
fn index_type(index: &Expr, scope: &dyn Scope) -> &'static str {
    match index {
        Expr::Path { path, .. } => lookup(scope, path).map_or("null", |v| type_name(&v)),
        Expr::Str(_) => "string",
        _ => "null",
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

fn run(e: &Expr, scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> R {
    if let Expr::Opt(x) = e {
        return match stat(x, scope, cx) {
            Stat::Undef(_) => R::Val(V::Null),
            _ => run(x, scope, calls, cx),
        };
    }
    if let Some(f) = whole(e, scope, cx) {
        return R::Fail(f);
    }
    match e {
        Expr::Num(n) => R::Val(V::Num(*n)),
        Expr::Bool(b) => R::Val(V::Bool(*b)),
        Expr::Null => R::Val(V::Null),
        Expr::Str(parts) => run_template(parts, scope, calls, cx),
        Expr::Path { path, .. } => R::Val(lookup(scope, path).map_or(V::Null, V::from_json)),
        Expr::Index { path, index } => {
            let known = if contains_call(index) {
                match run(index, scope, calls, cx) {
                    R::Val(v) => Some(v),
                    other => return other,
                }
            } else {
                None
            };
            match element(path, index, known, scope, cx) {
                Ok(v) => R::Val(v),
                Err(f) => R::Fail(f),
            }
        }
        Expr::Array(items) => match gather(items.iter().map(|x| run(x, scope, calls, cx)).collect()) {
            Ok(vals) => R::Val(V::Arr(vals)),
            Err(r) => r,
        },
        Expr::Call(c) => run_call(c, scope, calls, cx),
        Expr::Not(x) | Expr::Neg(x) => {
            let not = matches!(e, Expr::Not(_));
            if operand_nil(x, scope, cx) {
                return R::Fail(nil_operand(if not { "logical not operation" } else { "unary operation" }));
            }
            match run(x, scope, calls, cx) {
                R::Val(v) => match (v.number(), not) {
                    (Some(n), true) => R::Val(V::Num(if n == 0.0 { 1.0 } else { 0.0 })),
                    (Some(n), false) => R::Val(V::Num(-n)),
                    (None, true) => R::Fail(Failure::plain(format!("Logical not requires numeric operand, got {}", v.type_name()))),
                    (None, false) => R::Fail(Failure::plain("Cannot negate non-numeric value")),
                },
                other => other,
            }
        }
        Expr::Bin(l, op, r) => {
            if operand_nil(l, scope, cx) || operand_nil(r, scope, cx) {
                return R::Fail(nil_operand(&format!("binary operation ({})", op.text())));
            }
            // Both sides run (no short circuit); the right side's failure wins (SUB:806-825).
            let (a, b) = (run(l, scope, calls, cx), run(r, scope, calls, cx));
            let (a, b) = match (a, b) {
                (_, R::Fail(f)) | (R::Fail(f), _) => return R::Fail(f),
                (R::Val(a), R::Val(b)) => (a, b),
                _ => return R::Pending,
            };
            match binary(*op, &a, &b) {
                Ok(v) => R::Val(v),
                // An operand that waited on a call loses the prefix (SUB:826-846).
                Err(m) if contains_call(l) || contains_call(r) => R::Fail(Failure::plain(m)),
                Err(m) => R::Fail(Failure::plain(format!("In {}: {m}", cx.invocation))),
            }
        }
        Expr::Cond(c, t, f) => {
            if [c, t, f].iter().any(|x| operand_nil(x, scope, cx)) {
                return R::Fail(nil_operand("ternary operation"));
            }
            match run(c, scope, calls, cx) {
                R::Val(v) => match truthy(&v) {
                    Ok(true) => run(t, scope, calls, cx),
                    Ok(false) => run(f, scope, calls, cx),
                    Err(m) => R::Fail(Failure::plain(m)),
                },
                other => other,
            }
        }
        Expr::Opt(_) => unreachable!("handled above"),
    }
}

/// A call: its arguments together (`SFC:586-620`), then the hook once all are known. A
/// failing argument that is a call becomes "In call to <name>: ..." unless no function was
/// registered for it (`SFC:655-679`).
fn run_call(c: &Call, scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> R {
    let mut results = Vec::new();
    for (_, a) in &c.args {
        results.push(match a {
            Arg::Ref(p) => R::Val(V::Str(p.clone())),
            Arg::Value(x) => match run(x, scope, calls, cx) {
                R::Fail(f) => match x {
                    Expr::Call(inner) if f.kind != FailKind::Unregistered => R::Fail(Failure {
                        reason: format!("In call to {}: {}", inner.name, f.reason),
                        kind: FailKind::Plain,
                        ..f
                    }),
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
        Outcome::Done(v) => R::Val(V::from_json(v)),
        Outcome::Failed(f) => R::Fail(f),
        Outcome::Pending => R::Pending,
    }
}

/// An interpolated string (`EP:343-407`, `EV:338-414`): an error a part has before it runs
/// stops the string (with `lax`, a bare reference's is ""); then every part runs.
fn run_template(parts: &[Part], scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> R {
    let mut skip = vec![false; parts.len()];
    for (i, p) in parts.iter().enumerate() {
        if let Part::Expr(x) = p {
            if let Some(f) = whole(x, scope, cx) {
                if cx.lax && bare(x) {
                    skip[i] = true;
                } else {
                    return R::Fail(f);
                }
            }
        }
    }
    let results = parts
        .iter()
        .zip(skip)
        .map(|(p, skip)| match p {
            Part::Text(t) => R::Val(V::Str(t.clone())),
            Part::Expr(_) if skip => R::Val(V::Null),
            Part::Expr(x) => run(x, scope, calls, cx),
        })
        .collect();
    match gather(results) {
        Ok(vals) => R::Val(V::Str(vals.iter().map(V::text).collect())),
        Err(r) => r,
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
    outcome(run(e, scope, calls, cx))
}

/// Evaluates an interpolated string to a `Value::String`.
pub fn eval_template(parts: &[Part], scope: &dyn Scope, calls: &mut dyn Calls, cx: &Ctx) -> Outcome {
    outcome(run_template(parts, scope, calls, cx))
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
            Outcome::Failed(f) => assert_eq!(f.reason, "Arithmetic operations require numeric operands", "no prefix when an operand is a call"),
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
}
