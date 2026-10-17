//! The Composer's command parser, syntax-highlight roles and colors, and the command and
//! file validity checkers (milestone 9l Task 4): a plain-Rust port of iTerm2's
//! `CommandParser`, `SyntaxHighlighter` and `FileChecker`.
//!
//! iTerm2 sources (`~/gitrepos/iterm2-ref/sources/Composer`): `CP` `CommandParser.swift`
//! (role parsing, one role per UTF-16 unit of the command line), `SH`
//! `SyntaxHighlighter.swift` (the 1024-unit limit, the colors and the L* contrast
//! adjustment), `FC` `FileChecker.swift` (the command and file validity caches).
//!
//! Decision (controller ruling): builtins, cmdlets and aliases of the session's shell are
//! NOT colored as invalid commands, unlike iTerm2's kept bug (`FC:166-236`, `SH:151-161`,
//! where only PATH executables are valid and everything else, including `cd` or `ll`, reads
//! red). On Windows most everyday commands are PowerShell cmdlets, so painting them red
//! would mark nearly everything invalid. `known_shell_command` below is the piece the app's
//! `command_ok` closure (built on top of `Checker`, in a later task) consults alongside a
//! real PATH/PATHEXT lookup: PATH/PATHEXT hits, cmd/PowerShell/POSIX builtins, the
//! PowerShell Verb-Noun cmdlet shape, and a short list of common PowerShell aliases are all
//! valid.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use crate::palette::Rgb;

// ---------------------------------------------------------------------------------------
// Command parser (`CP`)
// ---------------------------------------------------------------------------------------

/// A role per UTF-16 unit of the parsed command line (`CP:74-85`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    Command,
    Whitespace,
    Quoted,
    Other,
    Placeholder,
    QuotationMark,
    UnbalancedQuotationMark,
    UnbalancedParen,
    Paren,
    Subshell,
}

const BACKSLASH: u16 = b'\\' as u16;
const DOUBLE_QUOTE: u16 = b'"' as u16;
const SINGLE_QUOTE: u16 = b'\'' as u16;
const OPEN_PAREN: u16 = b'(' as u16;
const CLOSE_PAREN: u16 = b')' as u16;

fn is_ws(u: u16) -> bool {
    char::from_u32(u as u32).is_some_and(|c| c.is_whitespace())
}

/// `n`, `a`, `t`, `r` expand to their control characters for analysis (`SH:102-105`); any
/// other escaped letter is left as itself.
fn escape_for(c: u16) -> Option<u16> {
    match c {
        x if x == b'n' as u16 => Some(0x0A),
        x if x == b'a' as u16 => Some(0x07),
        x if x == b't' as u16 => Some(0x09),
        x if x == b'r' as u16 => Some(0x0D),
        _ => None,
    }
}

fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

/// iTerm2's `expandingTildeInPathPreservingSlash`: `~` alone or `~/rest` expands the home
/// directory, keeping the slash that followed it; anything else is unchanged.
fn expand_tilde(s: &str) -> String {
    if s == "~" {
        home_dir()
    } else if let Some(rest) = s.strip_prefix("~/") {
        format!("{}/{}", home_dir(), rest)
    } else {
        s.to_string()
    }
}

/// Mirrors `CommandParser`'s instance state. One role is assigned to each input index as it
/// is consumed (`append`, `CP:39-45`); a quote or paren's opening index is provisionally
/// `Unbalanced*` and is fixed up to the balanced role when its match is found
/// (`balanceQuote`/`balanceParen`, `CP:227-233`), never fixed up if it never closes.
struct Parser {
    roles: Vec<Role>,
    components: Vec<String>,
    current_value: Vec<u16>,
    single_quote: Option<usize>,
    double_quote: Option<usize>,
    paren_indexes: Vec<usize>,
    escape: bool,
    is_first_char_of_word: bool,
    first_char_quoted: bool,
}

impl Parser {
    fn new(len: usize) -> Parser {
        Parser {
            roles: vec![Role::Placeholder; len],
            components: Vec::new(),
            current_value: Vec::new(),
            single_quote: None,
            double_quote: None,
            paren_indexes: Vec::new(),
            escape: false,
            is_first_char_of_word: true,
            first_char_quoted: true,
        }
    }

    /// `role(_:)` (`CP:87-125`): while no word has been flushed yet, everything (including
    /// its own whitespace and quote and paren characters) reads as `Command` - only the
    /// first word gets the command role, but that role covers its own surrounding
    /// whitespace too (a kept iTerm2 trait, not a Termurion bug).
    fn role_of(&self, c: u16) -> Role {
        if self.components.is_empty() {
            return Role::Command;
        }
        if is_ws(c) {
            return Role::Whitespace;
        }
        if c == SINGLE_QUOTE {
            return if self.single_quote.is_none() { Role::UnbalancedQuotationMark } else { Role::QuotationMark };
        }
        if c == DOUBLE_QUOTE {
            return if self.double_quote.is_none() && self.single_quote.is_none() {
                Role::UnbalancedQuotationMark
            } else {
                Role::QuotationMark
            };
        }
        if self.single_quote.is_some() || self.double_quote.is_some() {
            return Role::Quoted;
        }
        if c == OPEN_PAREN {
            return Role::UnbalancedParen;
        }
        if c == CLOSE_PAREN {
            return if self.paren_indexes.is_empty() { Role::UnbalancedParen } else { Role::Paren };
        }
        if !self.paren_indexes.is_empty() {
            return Role::Subshell;
        }
        Role::Other
    }

    /// `append(_:_:hidden:)` (`CP:39-45`): assigns this index's role from the state at the
    /// moment of the call (before any quote/paren toggle that this same character causes),
    /// and adds the character to the word being built unless it is `hidden` (a quote mark or
    /// unquoted whitespace, which the shell would not see as part of the argument text).
    fn append(&mut self, idx: usize, c: u16, hidden: bool) {
        self.roles[idx] = self.role_of(c);
        if !hidden {
            self.current_value.push(c);
        }
    }

    fn balance(&mut self, idx: usize, from: Role, to: Role) {
        if self.roles[idx] == from {
            self.roles[idx] = to;
        }
    }

    fn toggle_double_quote(&mut self, idx: usize) {
        if let Some(open) = self.double_quote.take() {
            self.balance(open, Role::UnbalancedQuotationMark, Role::QuotationMark);
        } else {
            self.double_quote = Some(idx);
        }
        self.is_first_char_of_word = false;
    }

    fn toggle_single_quote(&mut self, idx: usize) {
        if let Some(open) = self.single_quote.take() {
            self.balance(open, Role::UnbalancedQuotationMark, Role::QuotationMark);
        } else {
            self.single_quote = Some(idx);
        }
        self.is_first_char_of_word = false;
    }

    /// `handleUnquotedWhitespace` (`CP:265-278`): flushes the word built so far into a
    /// component, tilde-expanding it unless its first character was quoted; repeated
    /// whitespace between words flushes nothing.
    fn flush_word(&mut self) {
        if self.is_first_char_of_word {
            return;
        }
        let value = String::from_utf16_lossy(&self.current_value);
        let component = if self.first_char_quoted { value } else { expand_tilde(&value) };
        self.components.push(component);
        self.current_value.clear();
        self.first_char_quoted = true;
        self.is_first_char_of_word = true;
    }

    /// `handleEscaped` (`CP:197-225`): the character after an unescaped backslash. The `n`,
    /// `a`, `t`, `r` expansions and the plain fallback each add one visible character
    /// carrying the role of that expanded (or literal) character; the in-quote fallbacks
    /// ("determined by testing with bash") add a leading backslash for text fidelity, though
    /// (a Task 4 simplification) only the last character written at this index keeps its
    /// role, since a Termurion role array holds one role per raw input index, not per output
    /// character.
    fn handle_escaped(&mut self, c: u16, j: usize) {
        self.is_first_char_of_word = false;
        self.escape = false;
        if let Some(expanded) = escape_for(c) {
            self.append(j, expanded, false);
        } else if self.double_quote.is_some() {
            if c == DOUBLE_QUOTE {
                self.append(j, DOUBLE_QUOTE, false);
            } else if c == BACKSLASH {
                self.append(j, BACKSLASH, false);
            } else {
                self.append(j, BACKSLASH, false);
                self.append(j, c, false);
            }
        } else if self.single_quote.is_some() {
            if c == SINGLE_QUOTE {
                self.append(j, BACKSLASH, false);
            } else {
                self.append(j, BACKSLASH, false);
                self.append(j, c, false);
            }
        } else {
            self.append(j, c, false);
        }
    }

    /// `handleUnescaped` (`CP:153-195`).
    fn handle_unescaped(&mut self, c: u16, j: usize) {
        if c == DOUBLE_QUOTE && self.single_quote.is_none() {
            self.append(j, c, true);
            self.toggle_double_quote(j);
            return;
        }
        if c == SINGLE_QUOTE && self.double_quote.is_none() {
            self.append(j, c, true);
            self.toggle_single_quote(j);
            return;
        }
        if c == 0 {
            self.single_quote = None;
            self.double_quote = None;
        }
        let is_whitespace = c == 0 || is_ws(c);
        if self.single_quote.is_none() && self.double_quote.is_none() && is_whitespace {
            if c != 0 {
                self.append(j, c, true);
            }
            self.flush_word();
            return;
        }
        if self.is_first_char_of_word {
            self.first_char_quoted = self.double_quote.is_some() || self.single_quote.is_some();
            self.is_first_char_of_word = false;
        }
        if c == OPEN_PAREN {
            self.paren_indexes.push(j);
            self.append(j, c, false);
        } else if c == CLOSE_PAREN {
            self.append(j, c, false);
            if let Some(open) = self.paren_indexes.pop() {
                self.balance(open, Role::UnbalancedParen, Role::Paren);
            }
        } else {
            self.append(j, c, false);
        }
    }

    /// `handle(character:at:)` (`CP:138-151`).
    fn handle(&mut self, c: u16, j: usize) {
        if c == BACKSLASH && !self.escape {
            self.append(j, c, true);
            self.escape = true;
            return;
        }
        if self.escape {
            self.handle_escaped(c, j);
            return;
        }
        self.handle_unescaped(c, j);
    }
}

/// Parses `command` (UTF-16 units, as the composer's text is stored) into a role per unit
/// and the shell-word components a send would see (`CP:54-72`). Only the first word gets the
/// `Command` role, including its own leading and trailing whitespace; every later word is
/// `Other`, `Quoted` or a paren/subshell role instead, regardless of position (`CP:87-90`).
#[allow(clippy::needless_range_loop)] // the index is used on more than one array (or is the loop's own end marker)
pub fn parse(command: &[u16]) -> (Vec<Role>, Vec<String>) {
    let len = command.len();
    let mut p = Parser::new(len);
    for j in 0..=len {
        let mut c = if j < len { command[j] } else { 0u16 };
        if j == len {
            p.escape = false;
        } else if c == 0 {
            // A raw NUL can't really happen, but iTerm2 treats it as a space rather than
            // risk it being read as end-of-string (`CP:61-64`).
            c = b' ' as u16;
        }
        p.handle(c, j);
    }
    (p.roles, p.components)
}

// ---------------------------------------------------------------------------------------
// Colors and the L* contrast adjustment (`SH:38-75`, `SH:291-331`)
// ---------------------------------------------------------------------------------------

/// One color per highlight role (`SH:38-49`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub regular: Rgb,
    pub invalid: Rgb,
    pub unknown: Rgb,
    pub command: Rgb,
    pub quoted: Rgb,
    pub argument: Rgb,
    pub quotation: Rgb,
    pub error: Rgb,
    pub paren: Rgb,
    pub file: Rgb,
}

/// Builds the highlighter's palette from the session's background, foreground and 16 ANSI
/// colors, each pushed at least 35 L* units from the background (`SH:51-70`). ANSI indices
/// follow the schema's numbering: 1 red, 2 green, 4 blue, 5 magenta, 6 cyan.
pub fn colors(bg: Rgb, fg: Rgb, ansi: &[Rgb; 16]) -> Colors {
    let c = |color: Rgb| contrasting(bg, color, 35.0);
    Colors {
        regular: c(fg),
        invalid: c(ansi[1]),
        unknown: c(ansi[6]),
        command: c(ansi[4]),
        quoted: c(ansi[2]),
        argument: c(ansi[6]),
        quotation: c(ansi[2]),
        error: c(ansi[1]),
        paren: c(ansi[4]),
        file: c(ansi[5]),
    }
}

const D65_XN: f64 = 95.0489;
const D65_YN: f64 = 100.0;
const D65_ZN: f64 = 108.8840;
const LAB_DELTA: f64 = 6.0 / 29.0;

struct Lab {
    l: f64,
    a: f64,
    b: f64,
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn lab_f(t: f64) -> f64 {
    if t > LAB_DELTA.powi(3) { t.cbrt() } else { t / (3.0 * LAB_DELTA * LAB_DELTA) + 4.0 / 29.0 }
}

fn lab_f_inv(t: f64) -> f64 {
    if t > LAB_DELTA { t.powi(3) } else { 3.0 * LAB_DELTA * LAB_DELTA * (t - 4.0 / 29.0) }
}

/// sRGB to CIE Lab (D65 white point), through linear-light XYZ.
fn rgb_to_lab(c: Rgb) -> Lab {
    let r = srgb_to_linear(c.r);
    let g = srgb_to_linear(c.g);
    let b = srgb_to_linear(c.b);
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) * 100.0;
    let y = (0.2126729 * r + 0.7151522 * g + 0.0721750 * b) * 100.0;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) * 100.0;
    let fx = lab_f(x / D65_XN);
    let fy = lab_f(y / D65_YN);
    let fz = lab_f(z / D65_ZN);
    Lab { l: 116.0 * fy - 16.0, a: 500.0 * (fx - fy), b: 200.0 * (fy - fz) }
}

fn lab_to_rgb(lab: Lab) -> Rgb {
    let fy = (lab.l + 16.0) / 116.0;
    let fx = fy + lab.a / 500.0;
    let fz = fy - lab.b / 200.0;
    let x = D65_XN * lab_f_inv(fx) / 100.0;
    let y = D65_YN * lab_f_inv(fy) / 100.0;
    let z = D65_ZN * lab_f_inv(fz) / 100.0;
    let r = 3.2404542 * x - 1.5371385 * y - 0.4985314 * z;
    let g = -0.9692660 * x + 1.8760108 * y + 0.0415560 * z;
    let b = 0.0556434 * x - 0.2040259 * y + 1.0572252 * z;
    crate::palette::rgb(linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b))
}

/// Pushes `fg`'s L* at least `min_l` units away from `bg`'s, raising it if that stays at or
/// under 100, otherwise lowering it (`colorWithContrast`, `SH:292-331`). The source's
/// containment check is `fg.l > darkMax && fg.l < brightMax`, both strict; Task 4 uses
/// `fg.l >= darkMax` (inclusive) so a foreground exactly as dark as a `darkMax` floored to 0
/// (e.g. black on black) still gets pushed, while a foreground already exactly `min_l` away
/// stays untouched, matching the brief's own worked examples (Decision: cost is a boundary
/// foreground exactly `min_l` above a very dark background could shift where the literal
/// Swift, which floors `darkMax` at 0, would not; no test case in this task hits it).
pub fn contrasting(bg: Rgb, fg: Rgb, min_l: f64) -> Rgb {
    let bg_lab = rgb_to_lab(bg);
    let mut fg_lab = rgb_to_lab(fg);
    let dark_max = (bg_lab.l - min_l).max(0.0);
    let bright_min = (bg_lab.l + min_l).min(100.0);
    if fg_lab.l >= dark_max && fg_lab.l < bright_min {
        if fg_lab.l + min_l <= 100.0 {
            fg_lab.l += min_l;
        } else {
            fg_lab.l -= min_l;
        }
    }
    lab_to_rgb(fg_lab)
}

// ---------------------------------------------------------------------------------------
// Highlighting (`SH:94-143`)
// ---------------------------------------------------------------------------------------

/// The command line is parsed and highlighted only up to this many UTF-16 units at a time
/// (`SH:95`).
pub const MAX_UNITS: usize = 1024;

/// Yes/no/still-checking, for both command validity and file existence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    Yes,
    No,
    Pending,
}

/// One colored, optionally bold, run of `text` (a UTF-16 unit range).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub color: Rgb,
    pub bold: bool,
}

/// Highlights `range` of `text` (UTF-16 units): parses it, colors each contiguous role run,
/// and looks up command validity and file existence through the injected closures (pure over
/// them so tests do not need a filesystem or a shell). Beyond `MAX_UNITS` the tail is reset
/// to `regular` and only the first `MAX_UNITS` units are actually parsed; the reset range's
/// low end is the literal constant `MAX_UNITS`, not `range.start + MAX_UNITS`, so with a
/// non-zero `range.start` (a prompt prefix) the reset undershoots by the prefix length - a
/// kept iTerm2 bug (`SH:94-100`, ".superpowers" resume notes' "1024-unit highlight reset
/// measured from 0").
pub fn highlight(
    text: &str,
    range: Range<usize>,
    colors: &Colors,
    command_ok: &dyn Fn(&str) -> Check,
    file_exists: &dyn Fn(&str) -> Check,
) -> Vec<Span> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut spans = Vec::new();
    highlight_range(&units, range, colors, command_ok, file_exists, &mut spans);
    spans
}

fn highlight_range(
    units: &[u16],
    range: Range<usize>,
    colors: &Colors,
    command_ok: &dyn Fn(&str) -> Check,
    file_exists: &dyn Fn(&str) -> Check,
    out: &mut Vec<Span>,
) {
    if range.end.saturating_sub(range.start) > MAX_UNITS {
        out.push(Span { range: MAX_UNITS..range.end, color: colors.regular, bold: false });
        highlight_range(units, range.start..(range.start + MAX_UNITS), colors, command_ok, file_exists, out);
        return;
    }
    let start = range.start.min(units.len());
    let end = range.end.min(units.len());
    if start >= end {
        return;
    }
    let sub = &units[start..end];
    let (roles, _components) = parse(sub);
    let mut i = 0;
    while i < roles.len() {
        let role = roles[i];
        let mut j = i + 1;
        while j < roles.len() && roles[j] == role {
            j += 1;
        }
        let seg = (start + i)..(start + j);
        match role {
            Role::Command => {
                let word = String::from_utf16_lossy(&sub[i..j]);
                let color = match command_ok(word.trim()) {
                    Check::No => colors.invalid,
                    Check::Yes => colors.command,
                    Check::Pending => colors.unknown,
                };
                out.push(Span { range: seg, color, bold: true });
            }
            Role::Whitespace | Role::Placeholder => {}
            Role::Quoted => out.push(Span { range: seg, color: colors.quoted, bold: false }),
            Role::Other => {
                let word = String::from_utf16_lossy(&sub[i..j]);
                let color = match file_exists(&word) {
                    Check::Yes => colors.file,
                    Check::No | Check::Pending => colors.argument,
                };
                out.push(Span { range: seg, color, bold: false });
            }
            Role::QuotationMark => out.push(Span { range: seg, color: colors.quotation, bold: false }),
            Role::UnbalancedQuotationMark | Role::UnbalancedParen => {
                out.push(Span { range: seg, color: colors.error, bold: false })
            }
            Role::Paren | Role::Subshell => out.push(Span { range: seg, color: colors.paren, bold: false }),
        }
        i = j;
    }
}

// ---------------------------------------------------------------------------------------
// Command and file checkers (`FC`)
// ---------------------------------------------------------------------------------------

/// At most this many command or file checks are outstanding (queued or handed out via
/// `requests` and not yet `answer`ed) at once (`FC:21`, `maxConcurrentChecks`).
pub const MAX_CHECKS: usize = 5;
/// A check with no answer for this long reads as `No` (`FC:158`, `pendingTimeout`).
pub const PENDING: Duration = Duration::from_secs(10);
/// A `Yes` or `No` answer is trusted for this long before it is rechecked (`FC:161`,
/// `steadyStateTimeout`).
pub const STEADY: Duration = Duration::from_secs(60);
/// While a file's existence is still pending, the highlighter rechecks it this often
/// (`SH:212`, the 0.1 s `asyncAfter`).
pub const RECHECK: Duration = Duration::from_millis(100);
/// Highlighting itself is rate-limited to once this often (`CTV:47-48`).
pub const RATE: Duration = Duration::from_millis(50);

/// A command or file validity check the checker cannot answer itself; the caller runs it
/// (PATH/PATHEXT lookup or a file stat, on a worker thread) and reports back with `answer`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Request {
    Command(String),
    File(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CommandEntry {
    Pending(Instant),
    Valid(Instant),
    Invalid(Instant),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FileEntry {
    Pending,
    Known(bool),
}

/// The command and file validity caches (`FileChecker` and `LocalFileChecker`), pure: it
/// never runs a check itself, only decides what is known, what needs asking, and how many
/// asks may be outstanding at once.
#[derive(Default)]
pub struct Checker {
    commands: HashMap<String, CommandEntry>,
    files: HashMap<String, FileEntry>,
    queue: Vec<Request>,
    in_flight: usize,
    /// Requests the checker itself gave up waiting on (a command's pending timeout, or a
    /// file swept by `clear_files`) that were already handed out via `requests` (so a
    /// worker might still answer them late). A count, not a set (fix round 2, finding 2's
    /// residual gap): `Request` carries no per-dispatch identity, so the same command
    /// string or file path can be abandoned more than once (timed out, redispatched, timed
    /// out again) before its stale workers answer; each abandonment must swallow exactly
    /// one later `answer` or `lost`, or a second one for the same key would decrement
    /// `in_flight` again for a slot that redispatching had already re-occupied.
    abandoned: HashMap<Request, u32>,
}

impl Checker {
    fn make_request(&mut self, r: Request) {
        self.in_flight += 1;
        self.queue.push(r);
    }

    /// Frees `r`'s outstanding slot because the checker, not a real answer, is done waiting
    /// on it: dropped outright if it was still sitting in the undrained queue (nothing was
    /// ever handed to a worker, so nothing will ever answer it), otherwise credited in
    /// `abandoned` so a later `answer` or `lost` for it is a no-op instead of double-freeing.
    fn abandon(&mut self, r: Request) {
        if let Some(pos) = self.queue.iter().position(|q| *q == r) {
            self.queue.remove(pos);
        } else {
            *self.abandoned.entry(r).or_insert(0) += 1;
        }
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Consumes one abandoned credit for `r`, if any, and reports whether it did: `true`
    /// means a prior `abandon` already freed this slot, so the caller (`answer` or `lost`)
    /// must not free it again.
    fn consume_abandoned(&mut self, r: &Request) -> bool {
        let Some(count) = self.abandoned.get_mut(r) else { return false };
        *count -= 1;
        if *count == 0 {
            self.abandoned.remove(r);
        }
        true
    }

    /// `LocalFileChecker.commandIsValid` (`FC:166-236`), simplified to one answer per
    /// command rather than iTerm2's per-PATH-directory pending count (Decision: a caller
    /// answers a `Request::Command` once, having tried every candidate from
    /// `command_candidates` itself; cost if wrong: a very slow single PATH entry could delay
    /// the answer for the whole command instead of only that entry).
    pub fn command(&mut self, cmd: &str, now: Instant) -> Check {
        if let Some(entry) = self.commands.get(cmd).copied() {
            match entry {
                CommandEntry::Valid(t) if now.duration_since(t) < STEADY => return Check::Yes,
                CommandEntry::Invalid(t) if now.duration_since(t) < STEADY => return Check::No,
                CommandEntry::Pending(t) if now.duration_since(t) < PENDING => return Check::Pending,
                CommandEntry::Pending(_) => {
                    // Unanswered past the pending timeout: give up waiting and read as
                    // invalid rather than iTerm2's silent retry, so the caller sees a
                    // definite answer (Decision, see the brief's own worked example). The
                    // slot this request held is freed too (fix round 1, finding 2), or a
                    // stalled command would permanently starve every command check once
                    // MAX_CHECKS had ever timed out.
                    self.commands.insert(cmd.to_string(), CommandEntry::Invalid(now));
                    self.abandon(Request::Command(cmd.to_string()));
                    return Check::No;
                }
                _ => {
                    self.commands.remove(cmd);
                }
            }
        }
        if self.in_flight >= MAX_CHECKS {
            return Check::Pending;
        }
        self.commands.insert(cmd.to_string(), CommandEntry::Pending(now));
        self.make_request(Request::Command(cmd.to_string()));
        Check::Pending
    }

    /// `FileChecker.cachedCheckIfFileExists`/`checkIfFileExists` (`FC:52-82`): no per-entry
    /// expiry, only the wholesale `clear_files` a composer dismissal does. Never queues a
    /// stat for a UNC-looking path (fix round 1, finding 1: `command_candidates` already
    /// refused these, but `file` - used for a command's plain-argument words too - did not;
    /// probing one would leak the user's Windows credentials to whatever `host` is, project
    /// invariant 5).
    pub fn file(&mut self, path: &str) -> Check {
        if crate::prompted::network_path(path) {
            return Check::No;
        }
        if let Some(entry) = self.files.get(path).copied() {
            return match entry {
                FileEntry::Known(true) => Check::Yes,
                FileEntry::Known(false) => Check::No,
                FileEntry::Pending => Check::Pending,
            };
        }
        if self.in_flight >= MAX_CHECKS {
            return Check::Pending;
        }
        self.files.insert(path.to_string(), FileEntry::Pending);
        self.make_request(Request::File(path.to_string()));
        Check::Pending
    }

    /// A handed-out request whose worker died or failed before answering (it never will, so
    /// every dispatched request ends in exactly one `answer` or one `lost`): its slot is freed
    /// once and its pending entry forgotten, so a later lookup asks again. Unlike the
    /// checker's own give-ups (`abandon`, where a late answer may still come), nothing is
    /// recorded to swallow a later answer; a request the checker had already abandoned (its
    /// slot already freed) frees nothing more.
    pub fn lost(&mut self, r: Request) {
        if self.consume_abandoned(&r) {
            return;
        }
        match &r {
            Request::Command(cmd) => {
                if matches!(self.commands.get(cmd), Some(CommandEntry::Pending(_))) {
                    self.commands.remove(cmd);
                }
            }
            Request::File(path) => {
                if self.files.get(path) == Some(&FileEntry::Pending) {
                    self.files.remove(path);
                }
            }
        }
        if let Some(pos) = self.queue.iter().position(|q| *q == r) {
            self.queue.remove(pos);
        }
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// The requests newly created since the last call, for the caller to actually run.
    pub fn requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.queue)
    }

    /// Records a request's result and frees its outstanding slot, unless the checker had
    /// already given up on and freed it itself (fix round 1, finding 2: a straggling answer
    /// for an abandoned request updates the cache but must not free a second slot).
    pub fn answer(&mut self, r: Request, ok: bool, now: Instant) {
        match r.clone() {
            Request::Command(cmd) => {
                self.commands.insert(cmd, if ok { CommandEntry::Valid(now) } else { CommandEntry::Invalid(now) });
            }
            Request::File(path) => {
                self.files.insert(path, FileEntry::Known(ok));
            }
        }
        if !self.consume_abandoned(&r) {
            self.in_flight = self.in_flight.saturating_sub(1);
        }
    }

    /// The composer's dismissal resets the file cache (`FC:46-50`, `reset`); command
    /// validity is left alone (PATH does not change just because the composer closed). Any
    /// file check still pending is abandoned too (fix round 1, finding 2's "same for file
    /// requests"): unlike a command, a file check has no timeout of its own to give up with
    /// (`file` takes no `now`), so dismissal is the one moment a stalled file check's slot
    /// can be freed short of the worker itself finally answering it.
    pub fn clear_files(&mut self) {
        let stale: Vec<String> =
            self.files.iter().filter(|(_, e)| **e == FileEntry::Pending).map(|(p, _)| p.clone()).collect();
        self.files.clear();
        for path in stale {
            self.abandon(Request::File(path));
        }
    }
}

fn is_drive_rooted(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// Every path a bare command name could resolve to: an absolute or `~` path is checked
/// directly with no PATHEXT variants (`FC:195-207`); otherwise each `path` directory is
/// tried bare and with each `pathext` suffix appended, directories first (`FC:208-225`). A
/// UNC-looking or device command (`\\host\...`, `//host/...`, `\\?\UNC\...`, ...) never
/// becomes a candidate at all (project invariant 5: never probe a UNC path as a file, which
/// would leak NTLM credentials to whatever `host` is); `Checker::file` refuses the same
/// shapes for plain-argument words that were never run through this function.
pub fn command_candidates(cmd: &str, path: &[String], pathext: &[String], home: &str) -> Vec<String> {
    if crate::prompted::network_path(cmd) {
        return Vec::new();
    }
    if cmd.starts_with('/') || cmd.starts_with('\\') || is_drive_rooted(cmd) {
        return vec![cmd.to_string()];
    }
    if cmd == "~" {
        return vec![home.to_string()];
    }
    if let Some(rest) = cmd.strip_prefix("~/") {
        return vec![format!("{home}/{rest}")];
    }
    let mut out = Vec::new();
    for dir in path {
        let sep = if dir.ends_with('\\') || dir.ends_with('/') { "" } else { "\\" };
        let base = format!("{dir}{sep}{cmd}");
        out.push(base.clone());
        for ext in pathext {
            out.push(format!("{base}{ext}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Controller ruling: builtins, cmdlets and aliases read as valid commands
// ---------------------------------------------------------------------------------------

/// cmd.exe's own commands.
pub const CMD_BUILTINS: &[&str] = &[
    "assoc", "break", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir", "echo", "endlocal",
    "erase", "exit", "for", "ftype", "goto", "if", "md", "mkdir", "move", "path", "pause", "popd", "prompt",
    "pushd", "rd", "rem", "ren", "rename", "rmdir", "set", "setlocal", "shift", "start", "time", "title", "type",
    "ver", "verify", "vol",
];

/// PowerShell's own language keywords and always-present commands (not cmdlets, which are
/// caught by `looks_like_cmdlet` instead).
pub const POWERSHELL_BUILTINS: &[&str] =
    &["cd", "cls", "clear", "dir", "echo", "exit", "history", "popd", "pushd", "pwd", "set", "type"];

/// POSIX (bash/zsh/sh) shell builtins.
pub const POSIX_BUILTINS: &[&str] = &[
    "alias", "bg", "bind", "break", "builtin", "case", "cd", "command", "continue", "declare", "do", "done",
    "echo", "elif", "else", "enable", "esac", "eval", "exec", "exit", "export", "fc", "fg", "fi", "for",
    "function", "getopts", "hash", "help", "history", "if", "jobs", "kill", "let", "local", "logout", "popd",
    "printf", "pushd", "pwd", "read", "readonly", "return", "set", "shift", "shopt", "source", "suspend", "test",
    "then", "time", "times", "trap", "type", "typeset", "ulimit", "umask", "unalias", "unset", "until", "wait",
    "while",
];

/// Common PowerShell aliases for everyday commands (the controller ruling's list, plus a few
/// equally common ones); not exhaustive (Decision: a rarer alias falls back to a real
/// PATH/PATHEXT lookup instead, cost is a false "unknown" until that resolves).
pub const POWERSHELL_ALIASES: &[&str] = &[
    "ls", "dir", "cd", "cat", "echo", "rm", "cp", "mv", "gci", "gc", "sl", "pwd", "cls", "gm", "gp", "gv", "ii",
    "ps", "rd", "ri", "rmdir", "rv", "sc", "select", "si", "sleep", "sort", "sp", "sv", "tee", "type", "where",
    "write", "kill", "clear", "man", "diff", "cpi", "mi", "ni", "rni", "gjb", "sajb", "gcm", "gv", "iex", "cnsn",
];

/// PowerShell's approved verbs (a representative common subset: `Get-Verb`'s full list runs
/// to about 100). Fix round 1, finding 4: typed lowercase, a Verb-Noun command with one of
/// these verbs is still recognizably a cmdlet (`get-childitem`), unlike a plain hyphenated
/// argument word with no approved verb (`dry-run`).
pub const POWERSHELL_APPROVED_VERBS: &[&str] = &[
    "get", "set", "new", "remove", "add", "clear", "copy", "move", "rename", "start", "stop", "restart", "test",
    "invoke", "import", "export", "select", "where", "foreach", "sort", "measure", "out", "write", "read",
    "format", "convert", "convertto", "convertfrom", "enable", "disable", "install", "uninstall", "update",
    "find", "save", "resolve", "split", "join", "push", "pop", "wait", "show", "hide", "enter", "exit",
];

fn looks_like_cmdlet(cmd: &str) -> bool {
    let Some(dash) = cmd.find('-') else { return false };
    let (verb, rest) = cmd.split_at(dash);
    let noun = &rest[1..];
    !verb.is_empty()
        && !noun.is_empty()
        && verb.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && verb.chars().all(|c| c.is_ascii_alphabetic())
        && noun.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && noun.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A Verb-Noun command in any case, so long as the verb is one of PowerShell's approved
/// verbs; a plain hyphenated word (`dry-run`) whose first part isn't an approved verb stays
/// a non-command (fix round 1, finding 4).
fn looks_like_approved_verb_noun(cmd: &str) -> bool {
    let Some(dash) = cmd.find('-') else { return false };
    let (verb, rest) = cmd.split_at(dash);
    let noun = &rest[1..];
    !noun.is_empty()
        && noun.chars().all(|c| c.is_ascii_alphanumeric())
        && POWERSHELL_APPROVED_VERBS.contains(&verb.to_ascii_lowercase().as_str())
}

/// The controller ruling's non-PATH validity: a builtin of cmd, PowerShell or a POSIX shell,
/// the PowerShell Verb-Noun cmdlet shape (capitalized, or any case with an approved verb),
/// or a common PowerShell alias. The app's `command_ok` closure (a later task) is this
/// or-ed with a real PATH/PATHEXT hit.
pub fn known_shell_command(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    CMD_BUILTINS.contains(&lower.as_str())
        || POWERSHELL_BUILTINS.contains(&lower.as_str())
        || POSIX_BUILTINS.contains(&lower.as_str())
        || POWERSHELL_ALIASES.contains(&lower.as_str())
        || looks_like_cmdlet(cmd)
        || looks_like_approved_verb_noun(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn roles_follow_iterm2() {
        // "ls -l": the first word's role covers its own trailing space too, then the
        // second word is plain `Other` (CP:87-90).
        let (roles, components) = parse(&u("ls -l"));
        assert_eq!(roles, vec![Role::Command, Role::Command, Role::Command, Role::Other, Role::Other]);
        assert_eq!(components, vec!["ls".to_string(), "-l".to_string()]);

        // `echo "a b"`: the opening quote is provisionally unbalanced until the close is
        // seen, then fixed up to QuotationMark; the space inside the quotes is still
        // Whitespace (role() checks whitespace before quoting, CP:91-93).
        let (roles, components) = parse(&u("echo \"a b\""));
        assert_eq!(
            roles,
            vec![
                Role::Command,
                Role::Command,
                Role::Command,
                Role::Command,
                Role::Command,
                Role::QuotationMark,
                Role::Quoted,
                Role::Whitespace,
                Role::Quoted,
                Role::QuotationMark,
            ]
        );
        assert_eq!(components, vec!["echo".to_string(), "a b".to_string()]);

        // An unclosed quote after a command stays unbalanced forever (no matching close to
        // fix it up).
        let (roles, _) = parse(&u("cmd 'x"));
        assert_eq!(roles[4], Role::UnbalancedQuotationMark);
        assert_eq!(roles[5], Role::Quoted);

        // `(a)` after a command: Paren, Subshell, Paren, once the close balances the open.
        let (roles, _) = parse(&u("cmd (a)"));
        assert_eq!(roles[4..7], [Role::Paren, Role::Subshell, Role::Paren]);

        // A stray close paren never balances.
        let (roles, _) = parse(&u("cmd )"));
        assert_eq!(roles[4], Role::UnbalancedParen);

        // Only the first word is a command; "grep" after a pipe is a plain argument.
        let (roles, components) = parse(&u("ls | grep x"));
        assert!(roles[5..9].iter().all(|r| *r == Role::Other));
        assert_eq!(components, vec!["ls".to_string(), "|".to_string(), "grep".to_string(), "x".to_string()]);

        // A backslash escape expands into an actual character for analysis.
        let (_, components) = parse(&u("a\\nb"));
        assert_eq!(components, vec!["a\nb".to_string()]);

        // Components, with tilde expansion for the unquoted word only.
        let home = home_dir();
        let (_, components) = parse(&u("cd ~/x \"y z\""));
        assert_eq!(components, vec!["cd".to_string(), format!("{home}/x"), "y z".to_string()]);
    }

    fn test_colors() -> Colors {
        let bg = crate::palette::rgb(0, 0, 0);
        let fg = crate::palette::rgb(255, 255, 255);
        let mut ansi = [crate::palette::rgb(0, 0, 0); 16];
        ansi[1] = crate::palette::rgb(255, 0, 0);
        ansi[2] = crate::palette::rgb(0, 255, 0);
        ansi[4] = crate::palette::rgb(0, 0, 255);
        ansi[5] = crate::palette::rgb(255, 0, 255);
        ansi[6] = crate::palette::rgb(0, 255, 255);
        colors(bg, fg, &ansi)
    }

    #[test]
    fn highlight_follows_iterm2() {
        let colors = test_colors();
        let yes = |_: &str| Check::Yes;
        let no = |_: &str| Check::No;
        let pending = |_: &str| Check::Pending;

        let spans = highlight("ls", 0..2, &colors, &yes, &no);
        assert_eq!(spans, vec![Span { range: 0..2, color: colors.command, bold: true }]);

        let spans = highlight("bogus", 0..5, &colors, &no, &no);
        assert_eq!(spans, vec![Span { range: 0..5, color: colors.invalid, bold: true }]);

        let spans = highlight("maybe", 0..5, &colors, &pending, &no);
        assert_eq!(spans, vec![Span { range: 0..5, color: colors.unknown, bold: true }]);

        // An existing-file argument is `file`, else `argument`.
        let file_exists = |p: &str| if p == "file.txt" { Check::Yes } else { Check::No };
        let spans = highlight("ls file.txt", 0..11, &colors, &yes, &file_exists);
        let arg = spans.iter().find(|s| s.range == (3..11)).expect("argument span");
        assert_eq!(arg.color, colors.file);
        assert!(!arg.bold);

        let spans = highlight("ls nope", 0..7, &colors, &yes, &file_exists);
        let arg = spans.iter().find(|s| s.range == (3..7)).expect("argument span");
        assert_eq!(arg.color, colors.argument);

        // A 1500-unit text: 0..1024 colored by role, 1024..1500 reset to `regular`.
        let text: String = std::iter::repeat_n('a', 1500).collect();
        let spans = highlight(&text, 0..1500, &colors, &yes, &no);
        assert!(spans.contains(&Span { range: 1024..1500, color: colors.regular, bold: false }));
        assert!(spans.contains(&Span { range: 0..1024, color: colors.command, bold: true }));

        // With a 10-unit offset, the reset still starts at the literal 1024, not 1034 (a
        // kept iTerm2 bug: SH:97 uses `maxLength..<upperBound`, not `lowerBound+maxLength`).
        let text: String = std::iter::repeat_n('a', 1520).collect();
        let spans = highlight(&text, 10..1510, &colors, &yes, &no);
        assert!(spans.contains(&Span { range: 1024..1510, color: colors.regular, bold: false }));
        assert!(spans.contains(&Span { range: 10..1034, color: colors.command, bold: true }));

        // contrasting(): black on black is raised by 35 L*; a color already 35 away is
        // unchanged; one near white (on a white background) is lowered.
        let black = crate::palette::rgb(0, 0, 0);
        let white = crate::palette::rgb(255, 255, 255);
        let boosted = contrasting(black, black, 35.0);
        assert!((rgb_to_lab(boosted).l - 35.0).abs() < 1.0);

        // Comfortably clear of the boundary (a value quantized through 8-bit RGB can land a
        // fraction of an L* unit either side of an exact "35 away", which would make a
        // boundary-hugging test flaky rather than testing anything real).
        let far_enough = lab_to_rgb(Lab { l: 50.0, a: 0.0, b: 0.0 });
        let unchanged = contrasting(black, far_enough, 35.0);
        assert!((rgb_to_lab(unchanged).l - 50.0).abs() < 1.0);

        let near_white = lab_to_rgb(Lab { l: 85.0, a: 0.0, b: 0.0 });
        let lowered = contrasting(white, near_white, 35.0);
        assert!((rgb_to_lab(lowered).l - 50.0).abs() < 2.0);
    }

    #[test]
    fn checker_follows_iterm2() {
        assert_eq!(MAX_CHECKS, 5);
        assert_eq!(PENDING, Duration::from_secs(10));
        assert_eq!(STEADY, Duration::from_secs(60));
        assert_eq!(RECHECK, Duration::from_millis(100));
        assert_eq!(RATE, Duration::from_millis(50));

        let mut c = Checker::default();
        let t0 = Instant::now();

        // At most 5 outstanding requests.
        for i in 0..6 {
            assert_eq!(c.command(&format!("cmd{i}"), t0), Check::Pending);
        }
        let reqs = c.requests();
        assert_eq!(reqs.len(), 5);
        assert!(c.requests().is_empty(), "requests() drains the queue");

        // A command answered true stays Yes for 60 s.
        c.answer(reqs[0].clone(), true, t0);
        assert_eq!(c.command("cmd0", t0 + Duration::from_secs(59)), Check::Yes);
        assert_eq!(c.command("cmd0", t0 + Duration::from_secs(61)), Check::Pending);

        // A request unanswered for 10 s counts as No.
        assert_eq!(reqs[1], Request::Command("cmd1".to_string()));
        assert_eq!(c.command("cmd1", t0 + Duration::from_secs(11)), Check::No);

        // command_candidates.
        assert_eq!(
            command_candidates(
                "git",
                &["C:\\bin".to_string()],
                &[".EXE".to_string(), ".CMD".to_string()],
                "C:\\Users\\rob",
            ),
            vec!["C:\\bin\\git".to_string(), "C:\\bin\\git.EXE".to_string(), "C:\\bin\\git.CMD".to_string()],
        );
        assert_eq!(
            command_candidates("/usr/bin/tool", &["C:\\bin".to_string()], &[".EXE".to_string()], "C:\\Users\\rob"),
            vec!["/usr/bin/tool".to_string()],
        );
        assert_eq!(
            command_candidates("~", &["C:\\bin".to_string()], &[".EXE".to_string()], "C:\\Users\\rob"),
            vec!["C:\\Users\\rob".to_string()],
        );
        assert!(command_candidates("\\\\host\\share\\evil.exe", &[], &[], "home").is_empty());
        assert!(command_candidates("//host/share/evil", &[], &[], "home").is_empty());

        // clear_files empties the file cache. A fresh checker, since the one above has its
        // 5 outstanding slots already spoken for by the command scenarios above.
        let mut c = Checker::default();
        assert_eq!(c.file("/tmp/a"), Check::Pending);
        let file_req = c.requests().pop().unwrap();
        c.answer(file_req, true, t0);
        assert_eq!(c.file("/tmp/a"), Check::Yes);
        c.clear_files();
        assert_eq!(c.file("/tmp/a"), Check::Pending);
    }

    #[test]
    fn known_shell_command_follows_controller_ruling() {
        assert!(known_shell_command("cd"), "cmd/PowerShell/POSIX builtin");
        assert!(known_shell_command("LS"), "alias, case-insensitive");
        assert!(known_shell_command("Get-ChildItem"), "Verb-Noun cmdlet shape");
        assert!(known_shell_command("Set-Location"), "Verb-Noun cmdlet shape");
        assert!(!known_shell_command("frobnicate"), "not a builtin, alias or cmdlet shape");
        // Fix round 1, finding 4: a lowercase-typed Verb-Noun with an approved verb is a
        // cmdlet too (`get-childitem`), but a plain hyphenated word with no approved verb
        // (`dry-run`) is not.
        assert!(known_shell_command("get-childitem"), "approved verb, lowercase");
        assert!(known_shell_command("Get-CHILDITEM"), "approved verb, mixed case noun");
        assert!(!known_shell_command("dry-run"), "not an approved verb");
    }

    #[test]
    fn file_refuses_unc_paths() {
        // Fix round 1, finding 1: Checker::file must never enqueue a stat for a UNC-looking
        // path (project invariant 5, an NTLM leak), matching command_candidates.
        let mut c = Checker::default();
        for p in ["\\\\host\\share\\x", "//host/share/x", "\\\\?\\UNC\\host\\share\\x"] {
            assert_eq!(c.file(p), Check::No, "{p}");
        }
        assert!(c.requests().is_empty(), "no stat was ever queued for a UNC path");
    }

    #[test]
    fn command_timeout_frees_its_slot_and_a_late_answer_does_not_double_free() {
        // Fix round 1, finding 2: a Pending command entry that times out must free its
        // in_flight slot (so a later distinct command can still be dispatched), and a
        // straggling answer for that same abandoned request must not free a second slot.
        let mut c = Checker::default();
        let t0 = Instant::now();
        for i in 0..5 {
            assert_eq!(c.command(&format!("s{i}"), t0), Check::Pending);
        }
        let reqs = c.requests();
        assert_eq!(reqs.len(), 5);

        let t1 = t0 + Duration::from_secs(11);
        // s0 times out from the checker's own point of view.
        assert_eq!(c.command("s0", t1), Check::No);
        // A slot is free: a 6th, previously-blocked command can now be dispatched.
        assert_eq!(c.command("fresh", t1), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("fresh".to_string())]);

        // Every slot is genuinely full again (s1..s4 still outstanding, plus fresh).
        assert_eq!(c.command("overflow", t1), Check::Pending, "no slot available");
        assert!(c.requests().is_empty(), "overflow was never actually queued");

        // The worker's answer for the abandoned s0 finally arrives late.
        c.answer(reqs[0].clone(), true, t1);
        assert!(c.requests().is_empty(), "answering the abandoned request queues nothing new");
        assert_eq!(c.command("overflow2", t1), Check::Pending, "the late answer did not double-free a slot");
    }

    #[test]
    fn lost_requests_free_their_slot_once() {
        // A worker that dies (or panics) never answers: the request's slot is freed once,
        // its pending entry forgotten so a later lookup asks again, and a later real answer
        // for a new ask of the same thing frees its own slot normally.
        let t0 = Instant::now();
        let mut c = Checker::default();
        for i in 0..MAX_CHECKS {
            assert_eq!(c.command(&format!("c{i}"), t0), Check::Pending);
        }
        let reqs = c.requests();
        assert_eq!(c.command("extra", t0), Check::Pending);
        assert!(c.requests().is_empty(), "full");
        c.lost(reqs[0].clone());
        // One slot free: a new command is asked.
        assert_eq!(c.command("extra", t0), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("extra".to_string())]);
        // The lost command is asked again (not stuck pending), once a slot frees.
        c.answer(Request::Command("extra".to_string()), true, t0);
        assert_eq!(c.command("c0", t0), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("c0".to_string())]);
        // Its answer frees its slot (nothing left abandoned to swallow it).
        c.answer(Request::Command("c0".to_string()), false, t0);
        assert_eq!(c.command("more", t0), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("more".to_string())]);
        // The pending timeout of a lost command never frees a second slot.
        let t1 = t0 + PENDING + Duration::from_secs(1);
        assert_eq!(c.command("c1", t1), Check::No);
        // A lost file check is asked again too.
        let mut f = Checker::default();
        assert_eq!(f.file("a.txt"), Check::Pending);
        let r = f.requests();
        f.lost(r[0].clone());
        assert_eq!(f.file("a.txt"), Check::Pending);
        assert_eq!(f.requests(), vec![Request::File("a.txt".to_string())]);
    }

    #[test]
    fn abandoning_the_same_key_twice_keeps_in_flight_exact() {
        // Fix round 2, finding 2's residual gap: `abandoned` had no per-request identity, so
        // the same command string abandoned twice (timed out, redispatched, timed out again)
        // before both stale workers answered would only swallow the first late answer; the
        // second would decrement in_flight for a slot a fresh dispatch had already
        // re-occupied, letting more than MAX_CHECKS ever be truly outstanding.
        let mut c = Checker::default();
        let t0 = Instant::now();
        for i in 0..5 {
            assert_eq!(c.command(&format!("k{i}"), t0), Check::Pending);
        }
        let reqs = c.requests();
        assert_eq!(reqs.len(), 5);

        // Two abandonments of the very same request (as if it timed out, was redispatched
        // once its Invalid cache entry itself went stale, and its second worker also went
        // silent), each promptly followed by a distinct command claiming the freed slot.
        c.abandon(Request::Command("k0".to_string()));
        assert_eq!(c.command("k5", t0), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("k5".to_string())]);
        c.abandon(Request::Command("k0".to_string()));
        assert_eq!(c.command("k6", t0), Check::Pending);
        assert_eq!(c.requests(), vec![Request::Command("k6".to_string())]);

        // Every slot (k1..k4, k5, k6) is genuinely occupied now: a 7th is refused.
        assert_eq!(c.command("k7", t0), Check::Pending, "capacity check only");
        assert!(c.requests().is_empty(), "k7 was never actually queued");

        // Both stale workers now answer late for the very same original request.
        c.answer(reqs[0].clone(), true, t0);
        c.answer(reqs[0].clone(), true, t0);

        // Neither late answer freed anything further: capacity is still exactly full.
        assert_eq!(c.command("k8", t0), Check::Pending, "still no slot: no double-free");
        assert!(c.requests().is_empty());
    }

    #[test]
    fn clear_files_swept_twice_for_the_same_path_keeps_in_flight_exact() {
        let mut c = Checker::default();
        let t0 = Instant::now();
        assert_eq!(c.file("/tmp/a"), Check::Pending);
        let first = c.requests();
        assert_eq!(first, vec![Request::File("/tmp/a".to_string())]);
        c.clear_files(); // still Pending: abandoned once, in_flight freed.

        assert_eq!(c.file("/tmp/a"), Check::Pending); // re-requested, a fresh Pending entry.
        let second = c.requests();
        assert_eq!(second, vec![Request::File("/tmp/a".to_string())]);
        c.clear_files(); // abandoned a second time for the same path.

        // Fill every slot with distinct commands to make the count observable.
        for i in 0..5 {
            assert_eq!(c.command(&format!("c{i}"), t0), Check::Pending);
        }
        assert_eq!(c.requests().len(), 5);

        // Both stale workers answer late for the same original path.
        c.answer(first[0].clone(), true, t0);
        c.answer(second[0].clone(), true, t0);

        // Still exactly full: the two file sweeps' late answers freed nothing further.
        assert_eq!(c.command("overflow", t0), Check::Pending, "no slot: no double-free from the file sweeps");
        assert!(c.requests().is_empty());
    }
}
