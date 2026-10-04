//! Explain Output with AI's model (iTerm2's `AIExplanationRequest.swift`,
//! `AIExplanationResponse.swift`, `AIExplanationResponse+Mac.swift` and the bullets of
//! `ChatClient.swift:437-470`): the request and response with Swift's synthesized `Codable` JSON,
//! the prompt, the streaming parser and the chat markdown. Pure; the App side is
//! `app/ai_explain_app.rs`.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::ai_tools::truncated;
use crate::snippets::ellipsize;

/// Text from the terminal with the grid cell of each UTF-16 unit (`iTermCodableLocatedString`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    pub string: String,
    /// One (x, y) per UTF-16 unit of `string`, y a grid line at request time.
    pub coords: Vec<(i32, i32)>,
}

impl Located {
    pub fn to_json(&self) -> Value {
        let coords: Vec<Value> = self.coords.iter().map(|&(x, y)| json!({"x": x, "y": y})).collect();
        json!({"string": self.string, "gridCoords": {"coords": coords}})
    }

    pub fn from_json(v: &Value) -> Option<Located> {
        let string = v.get("string")?.as_str()?.to_string();
        let mut coords = Vec::new();
        for c in v.get("gridCoords")?.get("coords")?.as_array()? {
            coords.push((i32::try_from(c.get("x")?.as_i64()?).ok()?, i32::try_from(c.get("y")?.as_i64()?).ok()?));
        }
        Some(Located { string, coords })
    }

    pub fn lines(&self) -> Vec<&str> {
        self.string.split('\n').collect()
    }

    /// The UTF-16 offset where `line` starts (`offsetOfLineNumber:`); None past the last line.
    pub fn offset_of_line(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return Some(0);
        }
        let mut seen = 0;
        for (i, unit) in self.string.encode_utf16().enumerate() {
            if unit == u16::from(b'\n') {
                seen += 1;
                if seen == line {
                    return Some(i + 1);
                }
            }
        }
        None
    }
}

/// The request's session and the scrollback overflow when it was made (`Context`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub session_id: String,
    pub base_offset: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub command: Option<String>,
    pub truncated: bool,
    pub original: Located,
    pub question: String,
    pub subject: String,
    pub url: Option<String>,
    pub context: Context,
}

fn text_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

impl Request {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        if let Some(c) = &self.command {
            m.insert("command".into(), Value::String(c.clone()));
        }
        m.insert("truncated".into(), Value::Bool(self.truncated));
        m.insert("originalString".into(), self.original.to_json());
        m.insert("question".into(), Value::String(self.question.clone()));
        m.insert("subjectMatter".into(), Value::String(self.subject.clone()));
        if let Some(u) = &self.url {
            m.insert("url".into(), Value::String(u.clone()));
        }
        m.insert("context".into(), json!({"sessionID": self.context.session_id, "baseOffset": self.context.base_offset}));
        Value::Object(m)
    }

    /// Unknown keys (`_boxedUserInfo`) are ignored.
    pub fn from_json(v: &Value) -> Option<Request> {
        let context = v.get("context")?;
        Some(Request {
            command: text_of(v, "command"),
            truncated: v.get("truncated")?.as_bool()?,
            original: Located::from_json(v.get("originalString")?)?,
            question: text_of(v, "question")?,
            subject: text_of(v, "subjectMatter")?,
            url: text_of(v, "url"),
            context: Context { session_id: text_of(context, "sessionID")?, base_offset: context.get("baseOffset")?.as_i64()? },
        })
    }

    /// `snippetText`: the chat list's line.
    pub fn snippet(&self) -> String {
        if let Some(c) = &self.command {
            format!("Explain {c}")
        } else if !self.question.is_empty() {
            self.question.clone()
        } else {
            format!("Explain {}", self.subject)
        }
    }

    /// The text the model reads (`AIExplanationRequest.prompt`, `XQ:54-99`).
    pub fn prompt(&self) -> String {
        let mut instructions: Vec<String> = Vec::new();
        match &self.command {
            Some(c) => instructions.push(format!("After this paragraph I will attach the output of {c}, which the user needs help understanding.")),
            None => instructions.push("After this paragraph I will attach some text from a terminal emulator that the user needs help understanding.".into()),
        }
        if self.truncated {
            instructions.push("The beginning of the command output is not included because it was extremely long. What you see is just the latter part of it.".into());
        }
        let purpose = "Annotations should explain parts of the output that the user likely wants to know more about, such as error messages, abstruse terms of art, tersely formatted messages, or anything non-obvious.";
        if self.question.is_empty() {
            instructions.extend(["Respond with a sequence of annotations.".to_string(), purpose.to_string(), "Since the user asked for this to be explained, you should always add at least one annotation.".to_string()]);
        } else {
            instructions.extend([
                format!("The user has asked this question about it: \u{201c}{}\u{201d}", self.question),
                "You should respond to their question with a main response and, if needed, annotations of the text provided below.".to_string(),
                "The entirety of the main response should be wrapped in <response>\u{2026}</response> tags.".to_string(),
                "The main response should be markdown. < and & must be ampersand encoded.".to_string(),
                "You may also choose to include annotations that are relevant to the question.".to_string(),
                purpose.to_string(),
            ]);
        }
        instructions.extend(
            [
                "Annotations will be displayed to the user next to the parts of the terminal emulator's content.",
                "Don't annotate anything obvious, like self-explanatory comments in code.",
                "Never use a substring as its own annotation.",
                "An example of a good annotation: `Error: text file busy` might be annotated as `This usually happens when you try to modify or delete an executable that is currently running`.",
                "Each annotation you write should look like this: <annotation line=\"N\" text=\"A useful explanation of the annotated substring\" substring=\"text that occurs on line N that you wish to annotate\"/>.",
                "Use ampersand escaping like HTML if you need to use a quotation mark or an ampersand.",
                "You may provide more than one annotation per line of input if needed.",
                "Avoid overlapping annotations because they lead to a confusing user interface.",
                "I have added markup to help identify line numbers.",
                "Each line is wrapped in <line n=\"number\">\u{2026}</line>.",
                "The text to annotate follows:",
            ]
            .map(str::to_string),
        );
        format!("{}\n{}", instructions.join(" "), marked_up(&self.original.string))
    }
}

/// `&` then `<` escaped, each line wrapped with its number, joined by "\n".
fn marked_up(original: &str) -> String {
    original
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("<line n=\"{i}\">{line}</line>\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the next plain text of the conversation is (`MessageToPromptStateMachine.mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptMode {
    #[default]
    Regular,
    InitialExplanation,
}

/// The first plain text after an explanation (`conversationalPrompt`).
pub fn conversational(user: &str) -> String {
    format!("Stop using <annotation> and <response>. I am no longer parsing them. What follows will be a regular conversation. Here is the next prompt from the user: {user}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub line: usize,
    pub utf16_offset_in_line: usize,
    pub utf16_length: usize,
    pub note: String,
    pub annotated_text: String,
}

impl Annotation {
    fn to_json(&self) -> Value {
        json!({"line": self.line, "utf16OffsetInLine": self.utf16_offset_in_line, "utf16Length": self.utf16_length, "note": self.note, "annotatedText": self.annotated_text})
    }

    fn from_json(v: &Value) -> Option<Annotation> {
        let n = |key: &str| usize::try_from(v.get(key)?.as_u64()?).ok();
        Some(Annotation { line: n("line")?, utf16_offset_in_line: n("utf16OffsetInLine")?, utf16_length: n("utf16Length")?, note: text_of(v, "note")?, annotated_text: text_of(v, "annotatedText")? })
    }
}

fn annotations_json(a: &[Annotation]) -> Value {
    Value::Array(a.iter().map(Annotation::to_json).collect())
}

fn annotations_from(v: &Value) -> Option<Vec<Annotation>> {
    v.as_array()?.iter().map(Annotation::from_json).collect()
}

/// What one append found (`ExplanationResponse.Update`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub annotations: Vec<Annotation>,
    pub main_response: Option<String>,
    pub message_id: Option<String>,
    pub is_final: bool,
}

impl Update {
    pub fn new(is_final: bool, message_id: Option<String>) -> Update {
        Update { annotations: Vec::new(), main_response: None, message_id, is_final }
    }

    /// `init(_ full:)`: everything the response holds, final.
    pub fn full(r: &Response) -> Update {
        Update { annotations: r.annotations.clone(), main_response: r.main_response.clone(), message_id: None, is_final: true }
    }

    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("annotations".into(), annotations_json(&self.annotations));
        if let Some(t) = &self.main_response {
            m.insert("mainResponse".into(), Value::String(t.clone()));
        }
        if let Some(id) = &self.message_id {
            m.insert("messageID".into(), Value::String(id.clone()));
        }
        m.insert("final".into(), Value::Bool(self.is_final));
        Value::Object(m)
    }

    fn from_json(v: &Value) -> Option<Update> {
        Some(Update { annotations: annotations_from(v.get("annotations")?)?, main_response: text_of(v, "mainResponse"), message_id: text_of(v, "messageID"), is_final: v.get("final")?.as_bool()? })
    }
}

/// The model's answer to a request, grown as it streams (`ExplanationResponse`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub raw: String,
    pub unparsed: String,
    pub annotations: Vec<Annotation>,
    pub main_response: Option<String>,
    pub request: Request,
}

fn annotation_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| Regex::new(r#"<annotation line="(\d+)" text="(.*?)" substring="(.*?)" */>"#).expect("a fixed pattern"))
}

/// `CFXMLCreateStringByUnescapingEntities`: the five named entities and decimal and hex
/// character references; anything else stays as written.
pub fn unescape_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| decode_entity(&tail[1..end]).map(|c| (c, end + 1)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "quot" => Some('"'),
        "apos" => Some('\''),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

impl Response {
    pub fn new(request: Request) -> Response {
        Response { raw: String::new(), unparsed: String::new(), annotations: Vec::new(), main_response: None, request }
    }

    /// `init(text:request:final:)`: the first chunk parsed, its update discarded.
    pub fn with_text(request: Request, text: &str, is_final: bool) -> Response {
        let mut r = Response::new(request);
        r.append(text, is_final);
        r
    }

    /// More text from the model; what it completed.
    pub fn append(&mut self, s: &str, is_final: bool) -> Update {
        self.raw.push_str(s);
        self.unparsed.push_str(s);
        let mut update = Update::new(is_final, None);
        update.annotations = self.take_annotations();
        self.annotations.extend(update.annotations.iter().cloned());
        if let Some(main) = self.take_main_response() {
            self.main_response = Some(format!("{}{main}", self.main_response.as_deref().unwrap_or_default()));
            update.main_response = Some(main);
        }
        update
    }

    /// The `<annotation/>` elements in `unparsed`, cut out back to front; one is kept only when
    /// its line exists and its substring occurs in it (first occurrence, UTF-16 offsets).
    fn take_annotations(&mut self) -> Vec<Annotation> {
        let mut found = Vec::new();
        let mut cuts = Vec::new();
        for caps in annotation_pattern().captures_iter(&self.unparsed) {
            let (Some(whole), Some(line), Some(text), Some(substring)) = (caps.get(0), caps.get(1), caps.get(2), caps.get(3)) else { continue };
            // A line number too long for an integer leaves the element in the text.
            let Ok(line) = line.as_str().parse::<usize>() else { continue };
            found.push((line, unescape_entities(text.as_str()), unescape_entities(substring.as_str())));
            cuts.push(whole.range());
        }
        for range in cuts.into_iter().rev() {
            self.unparsed.replace_range(range, "");
        }
        let lines = self.request.original.lines();
        found
            .into_iter()
            .filter_map(|(line, note, substring)| {
                let text = lines.get(line)?;
                // Swift's `range(of: "")` is nil.
                let at = if substring.is_empty() { None } else { text.find(&substring) }?;
                Some(Annotation { line, utf16_offset_in_line: text[..at].encode_utf16().count(), utf16_length: substring.encode_utf16().count(), note, annotated_text: substring })
            })
            .collect()
    }

    /// The text between the first `<response>` and the first `</response>` after it once both
    /// have arrived, the pair cut out of `unparsed` (iTerm2 indexes the edited string with the
    /// old positions, `XRM:100-112`; Tandem takes the intended text, Decision).
    fn take_main_response(&mut self) -> Option<String> {
        const OPEN: &str = "<response>";
        const CLOSE: &str = "</response>";
        let start = self.unparsed.find(OPEN)?;
        let inner = start + OPEN.len();
        let end = inner + self.unparsed[inner..].find(CLOSE)?;
        let text = unescape_entities(&self.unparsed[inner..end]);
        self.unparsed.replace_range(start..end + CLOSE.len(), "");
        Some(text)
    }

    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("rawResponse".into(), Value::String(self.raw.clone()));
        m.insert("unparsed".into(), Value::String(self.unparsed.clone()));
        m.insert("annotations".into(), annotations_json(&self.annotations));
        if let Some(t) = &self.main_response {
            m.insert("mainResponse".into(), Value::String(t.clone()));
        }
        m.insert("request".into(), self.request.to_json());
        Value::Object(m)
    }

    fn from_json(v: &Value) -> Option<Response> {
        Some(Response {
            raw: text_of(v, "rawResponse")?,
            unparsed: text_of(v, "unparsed")?,
            annotations: annotations_from(v.get("annotations")?)?,
            main_response: text_of(v, "mainResponse"),
            request: Request::from_json(v.get("request")?)?,
        })
    }
}

/// The `explanationRequest` payload.
pub fn request_content(r: &Request) -> Value {
    json!({"request": r.to_json()})
}

pub fn parse_request_content(v: &Value) -> Option<Request> {
    Request::from_json(v.get("request")?)
}

/// The `explanationResponse` payload: `_1` is absent without an update.
pub fn response_content(r: &Response, u: Option<&Update>, markdown: &str) -> Value {
    let mut m = Map::new();
    m.insert("_0".into(), r.to_json());
    if let Some(u) = u {
        m.insert("_1".into(), u.to_json());
    }
    m.insert("markdown".into(), Value::String(markdown.to_string()));
    Value::Object(m)
}

pub fn parse_response_content(v: &Value) -> Option<(Response, Option<Update>, String)> {
    let update = match v.get("_1") {
        None | Some(Value::Null) => None,
        Some(u) => Some(Update::from_json(u)?),
    };
    Some((Response::from_json(v.get("_0")?)?, update, text_of(v, "markdown")?))
}

/// The hint after the bullets once the stream ended (`ChatClient.RevealAnnotationHint`).
pub const REVEAL_HINT: &str = "\nYou can click on a link in this message or on a yellow underline in the terminal to reveal an annotation.";

/// `inlineCode`: backticks escaped with a backslash, wrapped in backticks (`NS:3033-3037`).
pub fn inline_code(s: &str) -> String {
    format!("`{}`", s.replace('`', "\\`"))
}

/// One annotation's bullet text (`ChatClient.AnnotatedBullet`).
pub fn bullet(url: &str, annotated: &str, note: &str) -> String {
    format!("I [annotated]({url}) \u{201c}{annotated}\u{201d}: {note}")
}

/// The marginal markdown of an update (`ChatClient.swift:458-469`): the bullets, the hint on the
/// final update of a response that has annotations, then the main response.
pub fn markdown(bullets: &[String], is_final: bool, any_annotations: bool, main: Option<&str>) -> String {
    let mut out = bullets.iter().map(|b| format!("  * {b}")).collect::<Vec<_>>().join("\n");
    if !bullets.is_empty() {
        out.push('\n');
    }
    if is_final && any_annotations {
        out.push_str(REVEAL_HINT);
    }
    if let Some(main) = main {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(main);
    }
    out
}

/// `shortDescription` of an explanation request (`Message.swift:392-393`).
pub fn request_snippet(r: &Request, max: usize) -> String {
    format!("Explain {}", truncated(&r.original.string, max))
}

/// `shortDescription` of an explanation response (`Message.swift:394-399`).
pub fn response_snippet(r: &Response, u: Option<&Update>, max: usize) -> String {
    let (kind, annotations, main) = match u {
        Some(u) => ("Explanation (streaming)", u.annotations.len(), &u.main_response),
        None => ("Explanation", r.annotations.len(), &r.main_response),
    };
    format!("{kind}: {annotations} annotations: {}", main.as_deref().map_or_else(|| "No main response".to_string(), |m| truncated(m, max)))
}

pub const MAX_LINES: i64 = 1000;
pub const NO_CHAT_HEADING: &str = "Error";
pub const NO_CHAT_TEXT: &str = "AI Chat could not be opened. Verify you only have one instance of Tandem running.";
pub const COMMAND_OUTPUT: &str = "Command output";

/// `selectionForOutputToExplainWithAI` (`PS:21211-21238`): the first line to read and whether the
/// output was cut, more than `MAX_LINES` lines keeping the last `MAX_LINES`.
pub fn capped(start_line: i64, end_line: i64) -> (i64, bool) {
    if end_line - start_line > MAX_LINES {
        (end_line - MAX_LINES, true)
    } else {
        (start_line, false)
    }
}

/// What Explain reads (`PS:21211-21296`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The selection, and the command marks its first and last characters follow.
    Selection { text: String, marks: (Option<u32>, Option<u32>) },
    /// The last command's output.
    LastCommand { mark: u32, first_line: String, full: String },
}

/// `commandForOutputToExplainWithAI` (`PS:21240-21258`).
pub fn command_for(source: &Source, full_command_of: &dyn Fn(u32) -> Option<String>) -> Option<String> {
    match source {
        Source::Selection { marks: (Some(a), Some(b)), .. } if a == b => full_command_of(*a),
        Source::Selection { .. } => None,
        Source::LastCommand { full, .. } => Some(full.clone()).filter(|f| !f.is_empty()),
    }
}

/// `titleForExplainWithAI` (`PS:21260-21273`).
pub fn title(source: &Source) -> String {
    match source {
        Source::Selection { text, .. } => ellipsize(text, 16),
        Source::LastCommand { first_line, .. } if first_line.is_empty() => COMMAND_OUTPUT.to_string(),
        Source::LastCommand { first_line, .. } => first_line.clone(),
    }
}

/// `subjectMatterToExplainWithAI` (`PS:21275-21290`).
pub fn subject(source: &Source) -> String {
    match source {
        Source::Selection { text, .. } => {
            let short = ellipsize(text, 16);
            if short.is_empty() {
                "some selected text".to_string()
            } else {
                inline_code(&short)
            }
        }
        Source::LastCommand { first_line, .. } if first_line.is_empty() => "the last command".to_string(),
        Source::LastCommand { first_line, .. } => inline_code(first_line),
    }
}

/// The grid lines the located text reads: a line's cells as characters ('\0' for the spacer
/// after a wide character) and whether it wraps into the next line.
pub trait Rows {
    fn width(&self) -> usize;
    fn row(&self, line: i32) -> Option<(Vec<char>, bool)>;
}

/// The text from `start` to `end` (exclusive) as Copy extracts it with trimmed whitespace.
pub fn located(rows: &dyn Rows, start: (usize, i32), end: (usize, i32)) -> Located {
    let width = rows.width();
    let mut end = end;
    // An end at the start of a later line means the previous line's end.
    if end.1 > start.1 && end.0 == 0 {
        end = (width, end.1 - 1);
    }
    let (mut string, mut coords) = (String::new(), Vec::new());
    for y in start.1..=end.1 {
        let Some((cells, wraps)) = rows.row(y) else { break };
        let from = if y == start.1 { start.0 } else { 0 };
        let to = if y == end.1 { end.0.min(width) } else { width };
        let mut line: Vec<(usize, char)> = (from..to).map(|x| (x, cells.get(x).copied().unwrap_or(' '))).filter(|(_, c)| *c != '\0').collect();
        let soft = wraps && y != end.1;
        if !soft {
            while line.last().is_some_and(|(_, c)| c.is_whitespace()) {
                line.pop();
            }
        }
        for (x, c) in line {
            string.push(c);
            coords.extend(std::iter::repeat((x as i32, y)).take(c.len_utf16()));
        }
        if !soft && y != end.1 {
            string.push('\n');
            coords.push((width as i32, y));
        }
    }
    Located { string, coords }
}

/// An annotation's first cell and the cell after its last, each (column, grid line at request
/// time), wrapped by the width as `PSS:185-206` does. None when its line or units are missing.
pub fn cells(located: &Located, a: &Annotation, width: usize) -> Option<((usize, i32), (usize, i32))> {
    let first = located.offset_of_line(a.line)? + a.utf16_offset_in_line;
    if a.utf16_length == 0 || first + a.utf16_length > located.coords.len() || width == 0 {
        return None;
    }
    let (x, y) = located.coords[first];
    let (mut end_x, mut end_y) = (x as i64 + a.utf16_length as i64 - 1, y as i64);
    if end_x >= width as i64 {
        end_y += end_x / width as i64;
        end_x %= width as i64;
    }
    Some(((x as usize, y), ((end_x + 1) as usize, end_y as i32)))
}

/// `PSS:216-234`.
pub fn annotation_url(id: &str, session: &str) -> String {
    format!("iterm2:annotation?ann={id}&s={session}")
}

/// The annotation id and session of a reveal link; None for any other URL.
pub fn parse_annotation_url(url: &str) -> Option<(String, String)> {
    let query = url.strip_prefix("iterm2:annotation?")?;
    let (mut ann, mut session) = (None, None);
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("ann", v)) => ann = Some(v),
            Some(("s", v)) => session = Some(v),
            _ => {}
        }
    }
    Some((ann.filter(|v| !v.is_empty())?.to_string(), session.filter(|v| !v.is_empty())?.to_string()))
}

/// `PSS:262-283`: a link to the selection, `sub` being its start and end as (x, absolute line).
pub fn selection_url(session: &str, start: (usize, i64), end: (usize, i64)) -> String {
    format!("iterm2:/compound-location?session={session}&sub={},{},{},{}", start.0, start.1, end.0, end.1)
}

/// `PTA:172-174`: enabled with a selection or a command mark, whatever the gate says.
pub fn can_explain_with(has_selection: bool, has_command_mark: bool) -> bool {
    has_selection || has_command_mark
}

/// What Explain does next (ruling R12e-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplainStep {
    Stop,
    NoChat,
    AskKey,
    Send,
}

/// A closed gate is `Stop` whatever else holds.
pub fn explain_step(gate_open: bool, chat_open: bool, has_key: bool) -> ExplainStep {
    if !gate_open {
        ExplainStep::Stop
    } else if !chat_open {
        ExplainStep::NoChat
    } else if !has_key {
        ExplainStep::AskKey
    } else {
        ExplainStep::Send
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn located() -> Located {
        Located { string: "ab\nc".into(), coords: vec![(0, 5), (1, 5), (2, 5), (0, 6)] }
    }

    fn request() -> Request {
        Request {
            command: Some("make".into()),
            truncated: true,
            original: located(),
            question: String::new(),
            subject: "`make`".into(),
            url: Some("iterm2:/compound-location?session=G&sub=0,5,1,6".into()),
            context: Context { session_id: "G".into(), base_offset: 120 },
        }
    }

    fn plain(text: &str) -> Request {
        Request { original: Located { string: text.into(), coords: Vec::new() }, ..request() }
    }

    #[test]
    fn request_json_follows_swift() {
        let r = request();
        let v = r.to_json();
        assert_eq!(
            v,
            json!({
                "command": "make",
                "truncated": true,
                "originalString": {"string": "ab\nc", "gridCoords": {"coords": [{"x": 0, "y": 5}, {"x": 1, "y": 5}, {"x": 2, "y": 5}, {"x": 0, "y": 6}]}},
                "question": "",
                "subjectMatter": "`make`",
                "url": "iterm2:/compound-location?session=G&sub=0,5,1,6",
                "context": {"sessionID": "G", "baseOffset": 120}
            })
        );
        assert_eq!(Request::from_json(&v), Some(r.clone()));
        let bare = Request { command: None, url: None, ..r.clone() };
        let v = bare.to_json();
        assert!(v.get("command").is_none() && v.get("url").is_none());
        assert_eq!(Request::from_json(&v), Some(bare.clone()));
        let mut boxed = r.to_json();
        boxed["_boxedUserInfo"] = json!({"x": 1});
        assert_eq!(Request::from_json(&boxed), Some(r.clone()));
        assert_eq!(request_content(&r), json!({"request": r.to_json()}));
        assert_eq!(parse_request_content(&request_content(&r)), Some(r.clone()));
        assert_eq!(parse_request_content(&json!({})), None);

        assert_eq!(r.snippet(), "Explain make");
        assert_eq!(Request { command: None, question: "why?".into(), ..r.clone() }.snippet(), "why?");
        assert_eq!(bare.snippet(), "Explain `make`");
        assert_eq!(request_snippet(&r, 256), "Explain ab\nc");
        assert_eq!(request_snippet(&r, 3), "Explain ab\u{2026}");
        assert_eq!(located().lines(), vec!["ab", "c"]);
        assert_eq!((located().offset_of_line(0), located().offset_of_line(1), located().offset_of_line(2)), (Some(0), Some(3), None));
    }

    const PURPOSE: &str = "Annotations should explain parts of the output that the user likely wants to know more about, such as error messages, abstruse terms of art, tersely formatted messages, or anything non-obvious.";
    const TAIL: &str = "Annotations will be displayed to the user next to the parts of the terminal emulator's content. Don't annotate anything obvious, like self-explanatory comments in code. Never use a substring as its own annotation. An example of a good annotation: `Error: text file busy` might be annotated as `This usually happens when you try to modify or delete an executable that is currently running`. Each annotation you write should look like this: <annotation line=\"N\" text=\"A useful explanation of the annotated substring\" substring=\"text that occurs on line N that you wish to annotate\"/>. Use ampersand escaping like HTML if you need to use a quotation mark or an ampersand. You may provide more than one annotation per line of input if needed. Avoid overlapping annotations because they lead to a confusing user interface. I have added markup to help identify line numbers. Each line is wrapped in <line n=\"number\">\u{2026}</line>. The text to annotate follows:";

    #[test]
    fn prompt_follows_iterm2() {
        let r = Request { truncated: false, ..plain("a<b\n&c") };
        let body = "\n<line n=\"0\">a&lt;b</line>\n\n<line n=\"1\">&amp;c</line>\n";
        let first = "After this paragraph I will attach the output of make, which the user needs help understanding.";
        let none = "Respond with a sequence of annotations. ".to_string() + PURPOSE + " Since the user asked for this to be explained, you should always add at least one annotation.";
        assert_eq!(r.prompt(), format!("{first} {none} {TAIL}{body}"));
        let cut = "The beginning of the command output is not included because it was extremely long. What you see is just the latter part of it.";
        assert_eq!(Request { truncated: true, ..r.clone() }.prompt(), format!("{first} {cut} {none} {TAIL}{body}"));
        let no_command = Request { command: None, ..r.clone() }.prompt();
        assert!(no_command.starts_with("After this paragraph I will attach some text from a terminal emulator that the user needs help understanding. Respond with"));
        let asked = Request { question: "why?".into(), ..r.clone() }.prompt();
        let six = "The user has asked this question about it: \u{201c}why?\u{201d} You should respond to their question with a main response and, if needed, annotations of the text provided below. The entirety of the main response should be wrapped in <response>\u{2026}</response> tags. The main response should be markdown. < and & must be ampersand encoded. You may also choose to include annotations that are relevant to the question. ".to_string() + PURPOSE;
        assert_eq!(asked, format!("{first} {six} {TAIL}{body}"));
        assert!(!asked.contains("Respond with a sequence"));
        assert_eq!(
            conversational("why?"),
            "Stop using <annotation> and <response>. I am no longer parsing them. What follows will be a regular conversation. Here is the next prompt from the user: why?"
        );
    }

    fn busy() -> Request {
        plain("Error: text file busy\nok")
    }

    #[test]
    fn parser_follows_iterm2() {
        let mut r = Response::new(busy());
        let input = "Here <annotation line=\"0\" text=\"The binary is running &amp; locked\" substring=\"text file busy\"/> and";
        let u = r.append(input, false);
        let want = Annotation { line: 0, utf16_offset_in_line: 7, utf16_length: 14, note: "The binary is running & locked".into(), annotated_text: "text file busy".into() };
        assert_eq!(u.annotations, vec![want.clone()]);
        assert_eq!((r.unparsed.as_str(), r.raw.as_str()), ("Here  and", input));
        assert_eq!(r.annotations, vec![want]);

        // Split across two appends: found by the second only, once.
        let mut r = Response::new(busy());
        assert!(r.append("<annotation line=\"1\" text=\"n\" subs", false).annotations.is_empty());
        assert_eq!(r.append("tring=\"ok\"/>", false).annotations.len(), 1);
        assert_eq!(r.append("more", false).annotations.len(), 0);
        assert_eq!(r.annotations.len(), 1);

        // A missing line or a substring not on its line is dropped; spaces before "/>" are fine.
        let mut r = Response::new(busy());
        assert!(r.append("<annotation line=\"5\" text=\"n\" substring=\"ok\"/>", false).annotations.is_empty());
        assert!(r.append("<annotation line=\"1\" text=\"n\" substring=\"text\"/>", false).annotations.is_empty());
        assert_eq!(r.append("<annotation line=\"1\" text=\"n\" substring=\"k\"  />", false).annotations.len(), 1);

        // Two units for a character outside the BMP.
        let mut r = Response::new(plain("\u{1F600}ab"));
        let u = r.append("<annotation line=\"0\" text=\"n\" substring=\"ab\"/>", false);
        assert_eq!((u.annotations[0].utf16_offset_in_line, u.annotations[0].utf16_length), (2, 2));

        let mut r = Response::new(busy());
        let u = r.append("<response>A &lt; B</response> tail", true);
        assert_eq!((u.main_response.as_deref(), r.main_response.as_deref(), r.unparsed.as_str(), u.is_final), (Some("A < B"), Some("A < B"), " tail", true));
        r.append("<response> C</response>", true);
        assert_eq!(r.main_response.as_deref(), Some("A < B C"));
        // An unfinished block waits.
        let mut r = Response::new(busy());
        assert_eq!(r.append("<response>x", false).main_response, None);
        assert_eq!(r.append("y</response>", false).main_response.as_deref(), Some("xy"));

        assert_eq!(unescape_entities("&quot;x&apos;&#65;&#x42;&gt;&lt;&amp;"), "\"x'AB><&");
        assert_eq!(unescape_entities("a & b &nope; &#zz; &amp;lt;"), "a & b &nope; &#zz; &lt;");

        let r = Response::with_text(busy(), "t", true);
        assert_eq!(r.raw, "t");
        let mut grown = Response::new(busy());
        let u = grown.append("<annotation line=\"0\" text=\"n\" substring=\"Error\"/><response>m</response>", false);
        let with = Update { message_id: Some("ID".into()), ..u.clone() };
        for update in [None, Some(&with)] {
            let v = response_content(&grown, update, "md");
            assert_eq!(parse_response_content(&v), Some((grown.clone(), update.cloned(), "md".to_string())));
        }
        assert!(response_content(&grown, None, "").get("_1").is_none());
        assert_eq!(response_content(&grown, Some(&with), "")["_1"]["messageID"], "ID");
        assert_eq!(Update::full(&grown), Update { annotations: grown.annotations.clone(), main_response: Some("m".into()), message_id: None, is_final: true });
        assert_eq!(parse_response_content(&json!({"_0": {}})), None);

        assert_eq!(response_snippet(&grown, Some(&with), 256), "Explanation (streaming): 1 annotations: m");
        assert_eq!(response_snippet(&grown, None, 256), "Explanation: 1 annotations: m");
        assert_eq!(response_snippet(&Response::new(busy()), None, 256), "Explanation: 0 annotations: No main response");
    }

    #[test]
    fn bullets_follow_iterm2() {
        assert_eq!(bullet("iterm2:annotation?ann=A1&s=G", "text file busy", "The binary is running."), "I [annotated](iterm2:annotation?ann=A1&s=G) \u{201c}text file busy\u{201d}: The binary is running.");
        let (b1, b2) = ("b1".to_string(), "b2".to_string());
        assert_eq!(markdown(&[b1.clone(), b2], false, true, None), "  * b1\n  * b2\n");
        assert_eq!(markdown(&[b1], true, true, Some("Main")), format!("  * b1\n{REVEAL_HINT}\nMain"));
        assert_eq!(markdown(&[], true, false, Some("Main")), "Main");
        assert_eq!(markdown(&[], true, true, None), REVEAL_HINT);
        assert_eq!(REVEAL_HINT, "\nYou can click on a link in this message or on a yellow underline in the terminal to reveal an annotation.");
        assert_eq!(inline_code("x`y"), "`x\\`y`");
    }
    struct FakeRows(Vec<(String, bool)>);

    impl Rows for FakeRows {
        fn width(&self) -> usize {
            10
        }

        fn row(&self, line: i32) -> Option<(Vec<char>, bool)> {
            let (text, wraps) = self.0.get(usize::try_from(line).ok()?)?;
            Some((text.chars().collect(), *wraps))
        }
    }

    fn rows(lines: &[(&str, bool)]) -> FakeRows {
        FakeRows(lines.iter().map(|(t, w)| (t.to_string(), *w)).collect())
    }

    #[test]
    fn explain_source_follows_iterm2() {
        assert_eq!(capped(0, 999), (0, false));
        assert_eq!(capped(0, 1000), (0, false));
        assert_eq!(capped(5, 1006), (6, true));
        let full = |m: u32| Some(format!("cmd{m}"));
        let sel = |a, b| Source::Selection { text: "x".into(), marks: (a, b) };
        assert_eq!(command_for(&sel(Some(3), Some(3)), &full), Some("cmd3".into()));
        assert_eq!(command_for(&sel(Some(3), Some(4)), &full), None);
        assert_eq!(command_for(&sel(None, None), &full), None);
        let last = Source::LastCommand { mark: 1, first_line: "make".into(), full: "make\nmake install".into() };
        assert_eq!(command_for(&last, &|_| None), Some("make\nmake install".into()));
    }

    #[test]
    fn title_and_subject_follow_iterm2() {
        let sel = |t: &str| Source::Selection { text: t.into(), marks: (None, None) };
        let long = sel("error: file not found here");
        assert_eq!(title(&long), "error: file not\u{2026}");
        assert_eq!(subject(&long), "`error: file not\u{2026}`");
        assert_eq!(subject(&sel("a`b")), "`a\\`b`");
        let last = |first: &str| Source::LastCommand { mark: 1, first_line: first.into(), full: String::new() };
        assert_eq!(title(&last("cargo build")), "cargo build");
        assert_eq!(subject(&last("cargo build")), "`cargo build`");
        assert_eq!(title(&last("")), COMMAND_OUTPUT);
        assert_eq!(subject(&last("")), "the last command");
        assert_eq!(subject(&sel("  ")), "some selected text");
    }

    #[test]
    fn annotation_cells_follow_iterm2() {
        let one = super::located(&rows(&[("ls: nope  ", false)]), (0, 0), (10, 0));
        assert_eq!(one.string, "ls: nope");
        assert_eq!(one.coords, (0..8).map(|x| (x, 0)).collect::<Vec<_>>());

        let wrapped = super::located(&rows(&[("error: tex", true), ("t busy    ", false)]), (0, 0), (10, 1));
        assert_eq!(wrapped.string, "error: text busy");
        let a = Annotation { line: 0, utf16_offset_in_line: 7, utf16_length: 4, note: String::new(), annotated_text: String::new() };
        assert_eq!(cells(&wrapped, &a, 10), Some(((7, 0), (1, 1))));
        assert_eq!(cells(&wrapped, &Annotation { line: 5, ..a.clone() }, 10), None);
        assert_eq!(cells(&wrapped, &Annotation { utf16_offset_in_line: 99, ..a.clone() }, 10), None);

        let hard = super::located(&rows(&[("one", false), ("two", false)]), (0, 0), (3, 1));
        assert_eq!(hard.string, "one\ntwo");
        let second = Annotation { line: 1, utf16_offset_in_line: 1, utf16_length: 2, ..a };
        assert_eq!(cells(&hard, &second, 10), Some(((1, 1), (3, 1))));

        let emoji = super::located(&rows(&[("a\u{1f600}b", false)]), (0, 0), (3, 0));
        assert_eq!(emoji.string, "a\u{1f600}b");
        assert_eq!(emoji.coords, vec![(0, 0), (1, 0), (1, 0), (2, 0)]);

        // A range that starts mid-line and ends before the line's end reads only those cells.
        let part = super::located(&rows(&[("abcdef", false)]), (2, 0), (4, 0));
        assert_eq!((part.string.as_str(), part.coords.clone()), ("cd", vec![(2, 0), (3, 0)]));
    }

    #[test]
    fn annotation_url_round_trips() {
        assert_eq!(annotation_url("A1", "G1"), "iterm2:annotation?ann=A1&s=G1");
        assert_eq!(parse_annotation_url("iterm2:annotation?ann=A1&s=G1"), Some(("A1".into(), "G1".into())));
        assert_eq!(parse_annotation_url("iterm2:annotation?s=G1"), None);
        assert_eq!(parse_annotation_url("https://x"), None);
        assert_eq!(selection_url("G1", (0, 7), (3, 9)), "iterm2:/compound-location?session=G1&sub=0,7,3,9");
    }

    #[test]
    fn explain_enabled_follows_iterm2() {
        assert!(can_explain_with(true, false));
        assert!(can_explain_with(false, true));
        assert!(can_explain_with(true, true));
        assert!(!can_explain_with(false, false));
    }

    #[test]
    fn explain_is_gated() {
        for (chat, key) in [(true, true), (false, false), (true, false)] {
            assert_eq!(explain_step(false, chat, key), ExplainStep::Stop, "{chat} {key}");
        }
        assert_eq!(explain_step(true, false, true), ExplainStep::NoChat);
        assert_eq!(explain_step(true, true, false), ExplainStep::AskKey);
        assert_eq!(explain_step(true, true, true), ExplainStep::Send);

        // The gate input is the real gate for a host nobody consented to.
        let mut g = crate::globals::Globals::empty(std::path::Path::new("explain-gate-test.toml"));
        g.set(crate::ai_settings::ENABLE_AI, toml::Value::Boolean(true));
        g.set(crate::ai_settings::ADMIN, toml::Value::Boolean(true));
        let ep = crate::ai_models::Endpoint {
            vendor: crate::ai_settings::Vendor::OpenAI,
            model: "gpt-test".into(),
            url: "https://api.openai.com/v1/responses".into(),
            api: crate::ai_settings::Api::Responses,
            custom_headers: vec![],
        };
        let open = crate::ai_gate::check_endpoint(&g, Some(&ep)) == crate::ai_gate::Check::Pass;
        assert!(!open);
        assert_eq!(explain_step(open, true, true), ExplainStep::Stop);
    }
}
