//! Chat icons drawn from a model's SVG (milestone 13x, Task 4), the pure half. iTerm2's
//! `ChatIconGenerator` extracts every balanced `<svg>` document from the model's reply and
//! rasterizes them in order until one renders (`CIG:160-316`). The reply is untrusted text, so
//! this file also holds what iTerm2 gets from its sandboxed decoder (`CIG:340-362`): a strict
//! pre-check of each candidate (`check`) before anything is handed to a web view, the page the
//! view is served (`wrapper_doc`) and the judgement of what that page posts back.

use std::collections::HashMap;
use std::time::Duration;

/// Pixel side of the stored icon (`CIG:23`).
pub const SIDE: u32 = 128;
/// Bounds on extraction work (`CIG:176-179`).
pub const MAX_TOKENS: usize = 512;
pub const MAX_CANDIDATES: usize = 8;

/// The largest SVG candidate rendered (preflight P23).
pub const MAX_SVG_BYTES: usize = 256 * 1024;
/// The largest PNG stored (preflight P23); a larger one clears the icon.
pub const MAX_PNG_BYTES: usize = 256 * 1024;
/// Element, nesting and `<use>` bounds of a candidate.
pub const MAX_ELEMENTS: usize = 2000;
pub const MAX_DEPTH: usize = 32;
pub const MAX_USES: usize = 32;
/// How long one candidate may take to render before it counts as failed (brief, `Rasterizer`).
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(5);
/// A capture claiming more than this many pixels a side is not decoded.
const MAX_CAPTURE_SIDE: u32 = 4096;

// --- Extraction (`CIG:160-316`) ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `<svg ...>`
    Open,
    /// `<svg .../>`
    SelfClosed,
    /// `</svg ...>`
    Close,
}

struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

/// The first match of ASCII `needle` in `hay` at or after `from`, ignoring ASCII case.
fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle)).map(|p| p + from)
}

/// One forward scan. The next match of each token type is cached and searched again only from a
/// later position, so the cost stays linear (`svgTokens`).
fn svg_tokens(s: &str) -> Vec<Token> {
    let b = s.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    let mut next_open = find_ci(b, b"<svg", cursor);
    let mut next_close = find_ci(b, b"</svg", cursor);
    while cursor < b.len() && tokens.len() < MAX_TOKENS {
        if next_open.is_some_and(|o| o < cursor) {
            next_open = find_ci(b, b"<svg", cursor);
        }
        if next_close.is_some_and(|c| c < cursor) {
            next_close = find_ci(b, b"</svg", cursor);
        }
        let (is_open, at, len) = match (next_open, next_close) {
            (Some(o), c) if c.is_none_or(|c| o < c) => (true, o, 4),
            (_, Some(c)) => (false, c, 5),
            _ => break,
        };
        let upper = at + len;
        if upper >= b.len() {
            break;
        }
        let following = s[upper..].chars().next().unwrap_or('\0');
        if is_open {
            if !(following == '>' || following == '/' || following.is_whitespace()) {
                // A word like <svgfoo>, not an opening tag.
                cursor = upper;
                continue;
            }
        } else if !(following == '>' || following.is_whitespace()) {
            // </svgText> or </svg:svg>, not our close.
            cursor = upper;
            continue;
        }
        let Some(tag_end) = find_ci(b, b">", upper) else { break };
        let kind = if !is_open {
            Kind::Close
        } else if tag_end > upper && b[tag_end - 1] == b'/' {
            Kind::SelfClosed
        } else {
            Kind::Open
        };
        tokens.push(Token { kind, start: at, end: tag_end + 1 });
        cursor = tag_end + 1;
    }
    tokens
}

/// All balanced candidate documents of `reply`, in order (`extractSVGCandidates`). A bare
/// self-closing `<svg/>` is appended only when nothing else balanced.
pub fn candidates(reply: &str) -> Vec<String> {
    let tokens = svg_tokens(reply);
    let mut results: Vec<String> = Vec::new();
    let mut self_closing: Option<(usize, usize)> = None;
    for (i, cand) in tokens.iter().enumerate() {
        if results.len() >= MAX_CANDIDATES {
            break;
        }
        match cand.kind {
            Kind::Close => {}
            Kind::SelfClosed => {
                if self_closing.is_none() {
                    self_closing = Some((cand.start, cand.end));
                }
            }
            Kind::Open => {
                let mut depth = 1usize;
                for t in &tokens[i + 1..] {
                    match t.kind {
                        Kind::Open => depth += 1,
                        Kind::SelfClosed => {}
                        Kind::Close => {
                            depth -= 1;
                            if depth == 0 {
                                results.push(reply[cand.start..t.end].to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    if results.is_empty() {
        if let Some((a, b)) = self_closing {
            results.push(reply[a..b].to_string());
        }
    }
    results
}

// --- Coalescing (`CIG:41-73`, `131-143`) ---

/// Per-chat request state, for coalescing only: an entry exists exactly while a generation is
/// under way, and a newer title parks in `pending`.
#[derive(Default)]
pub struct Request {
    pub pending: Option<String>,
}

/// What a request does.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Run a generation for this (trimmed) title.
    Start(String),
    /// A generation is running: the newest title waits behind it.
    Park,
    /// Nothing starts: the AI gate is closed or the title is blank.
    Drop,
}

/// A running request parks the newest title first, even for a blank title or a closed gate (the
/// guards drop it when it re-enters); else the gate and the blank title drop; else start.
pub fn request(requests: &mut HashMap<String, Request>, chat: &str, title: &str, allowed: bool) -> Step {
    let trimmed = title.trim();
    if let Some(r) = requests.get_mut(chat) {
        r.pending = Some(trimmed.to_string());
        return Step::Park;
    }
    if !allowed || trimmed.is_empty() {
        return Step::Drop;
    }
    requests.insert(chat.to_string(), Request::default());
    Step::Start(trimmed.to_string())
}

/// A generation ended: the parked title to run instead, when a newer one arrived (the result is
/// then dropped, never stored for a title the chat no longer has).
pub fn finish(requests: &mut HashMap<String, Request>, chat: &str) -> Option<String> {
    requests.remove(chat).and_then(|r| r.pending)
}

// --- The pre-check of untrusted SVG ---

/// Why a candidate is not rendered. Never carries the document's text.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    TooLarge,
    Malformed,
    /// A DOCTYPE, an entity declaration or another processing instruction than `xml`.
    Declaration,
    /// A character reference outside the five predefined ones and numeric ones.
    Entity,
    Element(String),
    EventAttribute(String),
    /// A reference to anything but a same-document id or an inline raster image.
    External(String),
    /// CSS that could fetch or run something.
    Style,
    Depth,
    Count,
}

/// The elements an icon may hold (SVG 1.1 and 2 graphics, painting and filter elements): anything
/// else, including every element of another namespace (XHTML `a`, `img`, `form`), is refused. The
/// check fails closed: a name nobody listed is not allowed. `script`, `foreignObject`, `iframe`,
/// `animate`'s href and the like are not here or are judged below.
const ALLOWED_ELEMENTS: &[&str] = &[
    "svg", "g", "defs", "symbol", "use", "path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "text", "tspan", "textpath", "lineargradient", "radialgradient", "stop", "clippath", "mask", "pattern", "filter", "style", "title", "desc", "metadata", "marker", "image", "a", "switch", "animate", "animatemotion", "animatetransform", "set", "mpath", "view", "feblend", "fecolormatrix", "fecomponenttransfer", "fecomposite", "feconvolvematrix", "fediffuselighting", "fedisplacementmap", "fedistantlight", "fedropshadow", "feflood", "fefunca", "fefuncb", "fefuncg", "fefuncr", "fegaussianblur", "feimage", "femerge", "femergenode", "femorphology", "feoffset", "fepointlight", "fespecularlighting", "fespotlight", "fetile", "feturbulence",
];

/// The attributes an icon may carry (geometry, painting, text, gradients, filters, animation timing,
/// ids and classes). The local name is compared in lower case after the namespace prefix; `xmlns`
/// declarations are allowed by name. Anything else (`srcset`, `ping`, `background`, `poster`, `data`,
/// `target`, `download`, `autofocus`, `contenteditable` ...) is refused: the check fails closed.
const ALLOWED_ATTRIBUTES: &[&str] = &[
    "id", "class", "style", "lang", "space", "version", "baseprofile", "role", "tabindex", "x", "y", "x1", "x2", "y1", "y2", "z", "cx", "cy", "r", "rx", "ry", "fx", "fy", "fr", "width", "height", "viewbox", "preserveaspectratio", "d", "points", "pathlength", "transform", "transform-origin", "transform-box", "patterntransform", "gradienttransform", "gradientunits", "spreadmethod", "patternunits", "patterncontentunits", "clippathunits", "maskunits", "maskcontentunits", "markerunits", "markerwidth", "markerheight", "refx", "refy", "orient", "offset", "href", "dx", "dy", "rotate", "textlength", "lengthadjust", "startoffset", "method", "spacing", "side", "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-opacity", "stroke-linecap", "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray", "stroke-dashoffset", "opacity", "color", "color-interpolation", "color-interpolation-filters", "color-rendering", "stop-color", "stop-opacity", "flood-color", "flood-opacity", "lighting-color", "clip", "clip-path", "clip-rule", "mask", "mask-type", "filter", "marker-start", "marker-mid", "marker-end", "paint-order", "vector-effect", "shape-rendering", "image-rendering", "text-rendering", "display", "visibility", "overflow", "isolation", "mix-blend-mode", "font-family", "font-size", "font-size-adjust", "font-stretch", "font-style", "font-variant", "font-weight", "letter-spacing", "word-spacing", "text-anchor", "text-decoration", "dominant-baseline", "alignment-baseline", "baseline-shift", "direction", "unicode-bidi", "writing-mode", "glyph-orientation-vertical", "kerning", "xmlns", "in", "in2", "result", "type", "values", "mode", "operator", "k1", "k2", "k3", "k4", "stddeviation", "edgemode", "kernelmatrix", "kernelunitlength", "divisor", "bias", "targetx", "targety", "order", "preservealpha", "radius", "scale", "xchannelselector", "ychannelselector", "basefrequency", "numoctaves", "seed", "stitchtiles", "surfacescale", "diffuseconstant", "specularconstant", "specularexponent", "limitingconeangle", "pointsatx", "pointsaty", "pointsatz", "azimuth", "elevation", "slope", "intercept", "amplitude", "exponent", "tablevalues", "filterunits", "primitiveunits", "attributename", "attributetype", "from", "to", "by", "begin", "end", "dur", "min", "max", "restart", "repeatcount", "repeatdur", "calcmode", "keytimes", "keysplines", "keypoints", "accumulate", "additive", "path", "origin", "systemlanguage", "requiredfeatures", "requiredextensions", "zoomandpan", "viewtarget", "media", "title", "contentstyletype", "externalresourcesrequired",
];

fn attribute_allowed(local: &str, full: &str) -> bool {
    full.to_ascii_lowercase().starts_with("xmlns") || ALLOWED_ATTRIBUTES.contains(&local)
}

fn name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':')
}

fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

/// Character references in `s`: the five predefined and numeric ones only.
fn decode_entities(s: &str) -> Result<String, Refusal> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(semi) = after.find(';').filter(|n| *n <= 10) else { return Err(Refusal::Entity) };
        let name = &after[..semi];
        let c = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(h) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(h, 16).ok().filter(|_| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
                } else {
                    name.strip_prefix('#').filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).and_then(|d| d.parse::<u32>().ok())
                };
                match code.and_then(char::from_u32) {
                    Some(c) if c != '\0' => c,
                    _ => return Err(Refusal::Entity),
                }
            }
        };
        out.push(c);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A reference that stays inside the document, or an inline raster image.
fn href_ok(v: &str) -> bool {
    let t = v.trim_matches(|c: char| c.is_whitespace() || c.is_control());
    let l = t.to_ascii_lowercase();
    if l.starts_with('#') {
        return true;
    }
    ["data:image/png", "data:image/jpeg", "data:image/jpg", "data:image/gif", "data:image/webp"].iter().any(|p| l.starts_with(p) && matches!(l.as_bytes().get(p.len()), Some(b';' | b',')))
}

/// CSS text (a style element, a style or presentation attribute): no escapes, no imports, and
/// every `url(` names a same-document id or an inline raster image.
fn css_ok(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    if l.contains('\\') || ["@import", "expression(", "image-set(", "src(", "-moz-binding", "behavior"].iter().any(|bad| l.contains(bad)) {
        return false;
    }
    let mut rest = l.as_str();
    while let Some(p) = rest.find("url(") {
        let arg = rest[p + 4..].trim_start().trim_start_matches(['\'', '"']).trim_start();
        if !href_ok(arg) {
            return false;
        }
        rest = &rest[p + 4..];
    }
    true
}

/// Judges one SVG candidate before a web view sees it: size, element and depth caps, no script,
/// no foreign content, no event attribute, no reference outside the document, no CSS fetch, no
/// declaration (an entity table is the billion laughs), well-formed nesting. The answer is Ok or
/// the first reason; the document's text is never echoed.
pub fn check(svg: &str) -> Result<(), Refusal> {
    if svg.len() > MAX_SVG_BYTES {
        return Err(Refusal::TooLarge);
    }
    if svg.chars().any(|c| c < ' ' && !matches!(c, '\t' | '\n' | '\r')) {
        return Err(Refusal::Malformed);
    }
    let b = svg.as_bytes();
    let mut i = 0usize;
    let mut stack: Vec<String> = Vec::new();
    let (mut elements, mut uses, mut roots) = (0usize, 0usize, 0usize);
    let mut style_text = String::new();
    while i < b.len() {
        if b[i] != b'<' {
            let end = svg[i..].find('<').map_or(b.len(), |n| i + n);
            let text = decode_entities(&svg[i..end])?;
            if stack.is_empty() {
                if !text.chars().all(char::is_whitespace) {
                    return Err(Refusal::Malformed);
                }
            } else if stack.last().is_some_and(|n| local_name(n) == "style") {
                style_text.push_str(&text);
            }
            i = end;
            continue;
        }
        let rest = &svg[i..];
        if rest.starts_with("<!--") {
            let Some(n) = rest[4..].find("-->") else { return Err(Refusal::Malformed) };
            i += 4 + n + 3;
        } else if rest.starts_with("<![CDATA[") {
            let Some(n) = rest[9..].find("]]>") else { return Err(Refusal::Malformed) };
            if stack.is_empty() {
                return Err(Refusal::Malformed);
            }
            if stack.last().is_some_and(|n| local_name(n) == "style") {
                style_text.push_str(&rest[9..9 + n]);
            }
            i += 9 + n + 3;
        } else if rest.starts_with("<!") {
            return Err(Refusal::Declaration);
        } else if rest.starts_with("<?") {
            let Some(n) = rest.find("?>") else { return Err(Refusal::Malformed) };
            let target: String = rest[2..].chars().take_while(|c| !c.is_whitespace() && *c != '?').collect();
            if target != "xml" {
                return Err(Refusal::Declaration);
            }
            if !stack.is_empty() || roots > 0 {
                return Err(Refusal::Malformed);
            }
            i += n + 2;
        } else if rest.starts_with("</") {
            let name_end = rest[2..].find(|c: char| c.is_whitespace() || c == '>').map_or(rest.len(), |n| n + 2);
            let name = &rest[2..name_end];
            let after = rest[name_end..].trim_start();
            if name.is_empty() || !after.starts_with('>') {
                return Err(Refusal::Malformed);
            }
            if stack.pop().as_deref() != Some(name) {
                return Err(Refusal::Malformed);
            }
            if local_name(name) == "style" {
                if !css_ok(&style_text) {
                    return Err(Refusal::Style);
                }
                style_text.clear();
            }
            i += rest.len() - after.len() + 1;
        } else {
            i += start_tag(svg, i, &mut stack, &mut elements, &mut uses, &mut roots)?;
        }
    }
    if !stack.is_empty() || roots == 0 {
        return Err(Refusal::Malformed);
    }
    Ok(())
}

/// One start tag at `at`; the bytes it takes. Pushes the element unless it closes itself.
fn start_tag(svg: &str, at: usize, stack: &mut Vec<String>, elements: &mut usize, uses: &mut usize, roots: &mut usize) -> Result<usize, Refusal> {
    let b = svg.as_bytes();
    let mut j = at + 1;
    let first = j;
    while j < b.len() && name_char(b[j]) {
        j += 1;
    }
    if j == first || b[first].is_ascii_digit() || matches!(b[first], b'-' | b'.') {
        return Err(Refusal::Malformed);
    }
    let name = &svg[first..j];
    let local = local_name(name);
    let mut attrs: Vec<(String, String, String)> = Vec::new();
    let self_closed;
    loop {
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        match b.get(j) {
            None => return Err(Refusal::Malformed),
            Some(b'>') => {
                self_closed = false;
                j += 1;
                break;
            }
            Some(b'/') => {
                if b.get(j + 1) != Some(&b'>') {
                    return Err(Refusal::Malformed);
                }
                self_closed = true;
                j += 2;
                break;
            }
            Some(_) => {}
        }
        let an = j;
        while j < b.len() && name_char(b[j]) {
            j += 1;
        }
        if j == an || b[an].is_ascii_digit() {
            return Err(Refusal::Malformed);
        }
        let aname = &svg[an..j];
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            return Err(Refusal::Malformed);
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let q = match b.get(j) {
            Some(q @ (b'"' | b'\'')) => *q,
            _ => return Err(Refusal::Malformed),
        };
        j += 1;
        let Some(len) = b[j..].iter().position(|c| *c == q) else { return Err(Refusal::Malformed) };
        let raw = &svg[j..j + len];
        if raw.contains('<') {
            return Err(Refusal::Malformed);
        }
        attrs.push((local_name(aname), aname.to_string(), decode_entities(raw)?));
        j += len + 1;
    }
    if stack.is_empty() {
        *roots += 1;
        if *roots > 1 || local != "svg" {
            return Err(Refusal::Malformed);
        }
    }
    if !ALLOWED_ELEMENTS.contains(&local.as_str()) {
        return Err(Refusal::Element(local));
    }
    *elements += 1;
    if *elements > MAX_ELEMENTS {
        return Err(Refusal::Count);
    }
    if local == "use" {
        *uses += 1;
        if *uses > MAX_USES {
            return Err(Refusal::Count);
        }
    }
    if stack.len() + 1 > MAX_DEPTH {
        return Err(Refusal::Depth);
    }
    let animates = matches!(local.as_str(), "animate" | "set" | "animatemotion" | "animatetransform");
    for (an, full, av) in &attrs {
        if an.starts_with("on") {
            return Err(Refusal::EventAttribute(an.clone()));
        }
        if !attribute_allowed(an, full) {
            return Err(Refusal::External(an.clone()));
        }
        match an.as_str() {
            "href" if !href_ok(av) => return Err(Refusal::External(an.clone())),
            "src" | "base" | "action" | "formaction" => return Err(Refusal::External(an.clone())),
            "attributename" if animates && local_name(av.trim()) == "href" => return Err(Refusal::External(an.clone())),
            _ => {}
        }
        if !css_ok(av) {
            return Err(Refusal::Style);
        }
    }
    if !self_closed {
        stack.push(name.to_string());
    }
    Ok(j - at)
}

// --- The page the view loads ---

/// The page the rasterizer's view is served (`webhost::LocalDoc`, never NavigateToString): a 128 by
/// 128 transparent page with the SVG as a base64 `data:` image (`object-fit: cover` is the aspect
/// fill of `CIG:365-388`). The policy is a response header: nothing but data images and the page's
/// own nonce'd script, which posts "ok" once the image decoded and painted with a size, else
/// "fail". The SVG is only ever inside the `<img>`: no script in it runs, and its text is never part
/// of the page.
pub fn wrapper_doc(svg: &str) -> crate::webhost::LocalDoc {
    use base64::Engine as _;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<style>html,body{{margin:0;padding:0;width:{SIDE}px;height:{SIDE}px;overflow:hidden;background:transparent}}\
img{{display:block;width:{SIDE}px;height:{SIDE}px;object-fit: cover}}</style></head>\
<body><img id=\"i\" alt=\"\" src=\"data:image/svg+xml;base64,{b64}\">\
<script nonce=\"{nonce}\">(function(){{var i=document.getElementById('i');var sent=false;\
function done(ok){{if(sent)return;sent=true;window.chrome.webview.postMessage(ok?'ok':'fail');}}\
function go(){{var raf=window.requestAnimationFrame||function(f){{setTimeout(f,16)}};raf(function(){{raf(function(){{done(i.naturalWidth>0)}})}})}}\
i.onerror=function(){{done(false)}};\
if(i.decode){{i.decode().then(go,function(){{done(false)}})}}else if(i.complete){{go()}}else{{i.onload=go}}\
}})();</script></body></html>"
    );
    let csp = format!("default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'nonce-{nonce}'; frame-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'");
    crate::webhost::LocalDoc::new(html, &csp)
}

/// What the wrapper page posted (preflight P17): only the exact JSON strings `"ok"` and `"fail"`
/// count, and only when `LocalDoc::trusts` the message's source (the page's exact URL).
pub fn verdict(json: &str) -> Option<bool> {
    match json {
        "\"ok\"" => Some(true),
        "\"fail\"" => Some(false),
        _ => None,
    }
}

/// A captured PNG as the stored icon: decoded within bounds, brought to exactly `SIDE` by `SIDE`
/// when the capture was at another scale, encoded again (which drops every ancillary chunk), at
/// most `MAX_PNG_BYTES`.
pub fn normalize_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use image::{ImageFormat, ImageReader, Limits};
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_CAPTURE_SIDE);
    limits.max_image_height = Some(MAX_CAPTURE_SIDE);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("the capture is not a usable PNG: {e}"))?;
    let img = if img.width() == SIDE && img.height() == SIDE { img } else { img.resize_exact(SIDE, SIDE, image::imageops::FilterType::Triangle) };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).map_err(|e| format!("the icon could not be encoded: {e}"))?;
    let out = out.into_inner();
    if out.len() > MAX_PNG_BYTES {
        return Err("the icon is larger than 256 KiB".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(reply: &str) -> Vec<String> {
        candidates(reply)
    }

    #[test]
    fn svg_candidates_follow_iterm2() {
        let one = "<svg viewBox='0 0 1 1'><rect/></svg>";
        assert_eq!(c(one), vec![one.to_string()]);
        assert_eq!(c("wrapped in <svg>...</svg> tags:\n<svg a><g/></svg>"), vec!["<svg>...</svg>".to_string(), "<svg a><g/></svg>".to_string()]);
        // A word that starts like the tag is not the tag.
        assert_eq!(c("<svgfoo>x</svgfoo> <svg>a</svg>"), vec!["<svg>a</svg>".to_string()]);
        assert_eq!(c("<svg>a</svgText> b</svg>"), vec!["<svg>a</svgText> b</svg>".to_string()]);
        assert_eq!(c("<svg/>"), vec!["<svg/>".to_string()]);
        // The self-closing document is a last resort.
        assert_eq!(c("<svg/> then <svg>x</svg>"), vec!["<svg>x</svg>".to_string()]);
        // Nesting: the outer span comes first (iTerm2 also lists the inner balanced span after it).
        let nested = c("<svg><svg></svg></svg>");
        assert_eq!(nested[0], "<svg><svg></svg></svg>");
        assert_eq!(nested.get(1).map(String::as_str), Some("<svg></svg>"));
        // An unbalanced open is skipped and the second one wins.
        assert_eq!(c("<svg> <svg>y</svg>"), vec!["<svg>y</svg>".to_string()]);
        // 600 openers: the scan stops at 512 tokens and nothing balances.
        assert!(c(&"<svg ".repeat(600) .replace("<svg ", "<svg a>")).is_empty());
        // 20 balanced documents: 8.
        assert_eq!(c(&"<svg>x</svg> ".repeat(20)).len(), MAX_CANDIDATES);
        assert_eq!(c("<SVG>..</SVG>"), vec!["<SVG>..</SVG>".to_string()]);
        assert!(c("no markup at all").is_empty());
        assert!(c("").is_empty());
        // Multi-byte text around the tags never splits a character.
        assert_eq!(c("\u{e9}\u{4e2d}<svg>\u{1F600}</svg>\u{e9}"), vec!["<svg>\u{1F600}</svg>".to_string()]);
    }

    #[test]
    fn token_scan_is_bounded() {
        // 512 tokens are read; a document that would close only after them is not found.
        let mut reply = String::from("<svg>");
        reply.push_str(&"<svg a/>".repeat(520));
        reply.push_str("</svg>");
        // (Only the self-closing last resort remains, never the whole document.)
        assert_eq!(c(&reply), vec!["<svg a/>".to_string()]);
        // Just inside the bound it is.
        let mut ok = String::from("<svg>");
        ok.push_str(&"<svg a/>".repeat(100));
        ok.push_str("</svg>");
        assert_eq!(c(&ok).len(), 1);
    }

    #[test]
    fn icon_requests_coalesce_as_iterm2() {
        let mut r: HashMap<String, Request> = HashMap::new();
        assert_eq!(request(&mut r, "c", "Rust", true), Step::Start("Rust".into()));
        assert_eq!(request(&mut r, "c", "Newer", true), Step::Park);
        assert_eq!(finish(&mut r, "c"), Some("Newer".to_string()));
        assert!(!r.contains_key("c"));
        assert_eq!(request(&mut r, "d", "  ", true), Step::Drop);
        assert_eq!(request(&mut r, "e", "x", false), Step::Drop);
        assert!(r.is_empty());
        // A request while one runs parks even when blank or disallowed.
        assert_eq!(request(&mut r, "c", "A", true), Step::Start("A".into()));
        assert_eq!(request(&mut r, "c", "   ", false), Step::Park);
        assert_eq!(finish(&mut r, "c"), Some(String::new()));
        // Nothing parked: nothing to restart.
        assert_eq!(request(&mut r, "c", " B ", true), Step::Start("B".into()));
        assert_eq!(finish(&mut r, "c"), None);
        assert_eq!(finish(&mut r, "c"), None);
    }

    #[test]
    fn wrapper_doc_holds_the_svg_only_as_data() {
        use crate::webhost::Answer;
        let svg = "<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
        let doc = wrapper_doc(svg);
        let Answer::Respond { body, headers, .. } = doc.answer(doc.url()) else { panic!("served") };
        let html = String::from_utf8(body).unwrap();
        let csp = headers.iter().find(|(k, _)| k == "Content-Security-Policy").map(|(_, v)| v.clone()).expect("the policy is a header");
        assert!(html.contains("data:image/svg+xml;base64,"));
        assert!(html.contains("object-fit: cover"));
        // The SVG's own text is nowhere in the page, only its base64; no policy meta in the page.
        assert!(!html.contains("alert(1)") && !html.contains("<script>alert"));
        assert!(!html.contains("Content-Security-Policy"), "the policy is a header, not markup");
        // One script, the wrapper's own, under a nonce; no external reference, default-src none.
        assert_eq!(html.matches("<script").count(), 1);
        assert!(csp.contains("default-src 'none'") && csp.contains("img-src data:") && csp.contains("frame-src 'none'"));
        assert!(!csp.contains("script-src 'unsafe-inline'"), "scripts run only under the nonce");
        let nonce = csp.split("script-src 'nonce-").nth(1).and_then(|s| s.split('\'').next()).expect("a nonce in the policy");
        assert!(nonce.len() >= 16);
        assert!(html.contains(&format!("<script nonce=\"{nonce}\">")));
        assert!(!html.contains("http://") && !html.contains("https://"));
        let other = wrapper_doc(svg);
        assert_ne!(other.url(), doc.url(), "a fresh URL per page");
        use base64::Engine as _;
        let b64 = html.split("base64,").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(b64).unwrap(), svg.as_bytes());
    }

    const GOOD: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 64 64\"><defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"#08f\"/><stop offset=\"1\" stop-color=\"#f80\"/></linearGradient><circle id=\"dot\" r=\"4\"/><style>.a{fill:url(#g);stroke:none}</style></defs><rect width=\"64\" height=\"64\" fill=\"url(#g)\"/><use xlink:href=\"#dot\" x=\"10\" y=\"10\" class=\"a\"/><use href=\"#dot\" x=\"20\" y=\"20\" style=\"fill:url('#g')\"/><!-- a comment with <script> in it --><path d=\"M4 4L60 60\" stroke=\"white\"/><text>a &amp; b &#169; &#x41;</text></svg>";

    /// Review fix (Task 4, Minor 1): the check is an allowlist of SVG elements and attributes, so a
    /// name nobody listed fails closed.
    #[test]
    fn the_check_is_an_allowlist() {
        let x = |inner: &str| format!("<svg xmlns='http://www.w3.org/2000/svg'>{inner}</svg>");
        for bad in [
            "<h:img xmlns:h='http://www.w3.org/1999/xhtml' srcset='http://x 1x'/>",
            "<h:a xmlns:h='http://www.w3.org/1999/xhtml' ping='http://x'>t</h:a>",
            "<h:form xmlns:h='http://www.w3.org/1999/xhtml' action='http://x'/>",
            "<video poster='x'/>",
            "<rect width='1' height='1' background='http://x'/>",
            "<rect width='1' height='1' poster='http://x'/>",
            "<rect width='1' height='1' data='http://x'/>",
            "<a href='#a' target='_blank' ping='http://x'><rect/></a>",
            "<rect width='1' height='1' autofocus=''/>",
            "<custom-element/>",
        ] {
            assert!(check(&x(bad)).is_err(), "{bad}");
        }
        for ok in ["<rect width='1' height='1' fill='red' stroke-width='2'/>", "<g opacity='.5'><circle cx='1' cy='1' r='1'/></g>", "<defs><linearGradient id='g'><stop offset='0' stop-color='#fff'/></linearGradient></defs>", "<filter id='f'><feGaussianBlur stdDeviation='2'/></filter>", "<text x='1' y='1' font-family='a' text-anchor='middle'>t</text>"] {
            assert_eq!(check(&x(ok)), Ok(()), "{ok}");
        }
    }

    #[test]
    fn a_plain_icon_is_accepted() {
        assert_eq!(check(GOOD), Ok(()));
        assert_eq!(check("<svg/>"), Ok(()));
        assert_eq!(check("<?xml version=\"1.0\"?><svg xmlns='http://www.w3.org/2000/svg'><image href=\"data:image/png;base64,AAAA\"/></svg>"), Ok(()));
    }

    /// The hostile document the brief names: script, an event attribute, an external href and a
    /// billion-laughs entity table, each on its own and all together.
    #[test]
    fn hostile_svg_is_refused() {
        let laughs = "<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\"><!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">]><svg xmlns=\"http://www.w3.org/2000/svg\"><text>&lol3;</text></svg>";
        let hostile = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"><script>alert(2)</script><image xlink:href=\"https://evil.test/x.png\"/></svg>");
        assert!(check(&hostile).is_err());
        assert_eq!(check(laughs), Err(Refusal::Declaration));
        let refused = [
            "<svg><script>alert(1)</script></svg>",
            "<svg><SCRIPT type='x'/></svg>",
            "<svg><svg:script>alert(1)</svg:script></svg>",
            "<svg onload=\"alert(1)\"><rect/></svg>",
            "<svg><rect onclick='x()' width='1'/></svg>",
            "<svg><rect ONMOUSEOVER=\"x()\"/></svg>",
            "<svg><image href=\"http://evil.test/a.png\"/></svg>",
            "<svg><image xlink:href=\"https://evil.test/a.png\"/></svg>",
            "<svg><image href=\"//evil.test/a.png\"/></svg>",
            "<svg><image href=\"file:///C:/Windows/win.ini\"/></svg>",
            "<svg><image href=\"\\\\\\\\host\\\\share\\\\a.png\"/></svg>",
            "<svg><use href=\"other.svg#a\"/></svg>",
            "<svg><a href=\"javascript:alert(1)\"><rect/></a></svg>",
            "<svg><image href=\"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=\"/></svg>",
            "<svg><image href=\"data:text/html,<script>1</script>\"/></svg>",
            "<svg><image href=\"&#104;ttp://evil.test/a.png\"/></svg>",
            "<svg><foreignObject><div>hi</div></foreignObject></svg>",
            "<svg><iframe src='x'/></svg>",
            "<svg><object data='x'/></svg>",
            "<svg><embed src='x'/></svg>",
            "<svg><link rel='stylesheet' href='http://evil.test/a.css'/></svg>",
            "<svg><style>@import url(http://evil.test/a.css);</style></svg>",
            "<svg><style>@import 'http://evil.test/a.css';</style></svg>",
            "<svg><style>rect{fill:url(http://evil.test/a.svg#a)}</style></svg>",
            "<svg><style><![CDATA[rect{fill:url(http://evil.test/a.svg#a)}]]></style></svg>",
            "<svg><rect style=\"fill:url(https://evil.test/a.svg#a)\"/></svg>",
            "<svg><rect fill=\"url(  'https://evil.test/a.svg#a' )\"/></svg>",
            "<svg><rect fill=\"U&#82;L(https://evil.test/a#a)\"/></svg>",
            "<svg><rect style=\"fill:\\75 rl(http://evil.test/a)\"/></svg>",
            "<svg><style>rect{background:image-set('http://evil.test/a' 1x)}</style></svg>",
            "<svg><animate attributeName=\"href\" to=\"http://evil.test/a\"/></svg>",
            "<svg><set attributeName=\"xlink:href\" to=\"http://evil.test/a\"/></svg>",
            "<?xml-stylesheet href=\"http://evil.test/a.css\"?><svg/>",
            "<!DOCTYPE svg [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><svg>&x;</svg>",
            "<!DOCTYPE svg><svg/>",
            "<svg>&custom;</svg>",
            "<svg><rect id=\"&custom;\"/></svg>",
        ];
        for s in refused {
            assert!(check(s).is_err(), "must refuse: {s}");
        }
        // The more specific reasons.
        assert!(matches!(check("<svg><script/></svg>"), Err(Refusal::Element(_))));
        assert!(matches!(check("<svg onload='x'/>"), Err(Refusal::EventAttribute(_))));
        assert!(matches!(check("<svg><image href='http://a/b'/></svg>"), Err(Refusal::External(_))));
        assert_eq!(check("<svg>&custom;</svg>"), Err(Refusal::Entity));
    }

    #[test]
    fn caps_bound_size_depth_and_count() {
        let big = format!("<svg>{}</svg>", "a".repeat(MAX_SVG_BYTES));
        assert_eq!(check(&big), Err(Refusal::TooLarge));
        let deep = format!("<svg>{}{}</svg>", "<g>".repeat(MAX_DEPTH + 1), "</g>".repeat(MAX_DEPTH + 1));
        assert_eq!(check(&deep), Err(Refusal::Depth));
        let ok_deep = format!("<svg>{}{}</svg>", "<g>".repeat(MAX_DEPTH - 2), "</g>".repeat(MAX_DEPTH - 2));
        assert_eq!(check(&ok_deep), Ok(()));
        let many = format!("<svg>{}</svg>", "<rect/>".repeat(MAX_ELEMENTS + 1));
        assert_eq!(check(&many), Err(Refusal::Count));
        let uses = format!("<svg><g id='a'/>{}</svg>", "<use href='#a'/>".repeat(MAX_USES + 1));
        assert_eq!(check(&uses), Err(Refusal::Count));
    }

    #[test]
    fn malformed_documents_are_refused_never_a_panic() {
        for s in ["", "   ", "not xml", "<svg", "<svg><rect", "<svg><rect a=\"1></svg>", "<svg a=1></svg>", "<svg a></svg>", "<svg><g></svg>", "</svg>", "<svg></g></svg>", "<svg/><svg/>", "<g/>", "<svg><!-- unclosed </svg>", "<svg><![CDATA[ x </svg>", "<svg><?pi </svg>", "<svg>\u{0}</svg>", "\u{feff}<svg/>x"] {
            let r = std::panic::catch_unwind(|| check(s));
            assert!(r.is_ok(), "no panic for {s:?}");
        }
        for s in ["", "   ", "not xml", "<svg", "<svg><rect", "<svg><rect a=\"1></svg>", "<svg a=1></svg>", "<svg a></svg>", "<svg><g></svg>", "</svg>", "<svg></g></svg>", "<svg/><svg/>", "<g/>", "<svg><!-- unclosed </svg>", "<svg><![CDATA[ x </svg>", "<svg><?pi </svg>"] {
            assert!(check(s).is_err(), "must refuse {s:?}");
        }
        // Every prefix and every single-byte damage of a valid document is judged without panicking.
        for end in 0..GOOD.len() {
            if GOOD.is_char_boundary(end) {
                let _ = check(&GOOD[..end]);
            }
        }
        let junk: String = (0..4000u32).map(|i| char::from_u32(0x20 + (i * 37) % 0x2000).unwrap_or('x')).collect();
        let _ = check(&junk);
        let _ = check(&format!("<svg>{junk}</svg>"));
    }

    #[test]
    fn only_the_page_and_data_images_are_served() {
        use crate::webhost::Answer;
        let doc = wrapper_doc("<svg/>");
        assert!(matches!(doc.answer(doc.url()), Answer::Respond { status: 200, .. }));
        assert_eq!(doc.answer("data:image/svg+xml;base64,AAAA"), Answer::Continue);
        for u in ["http://evil.test/a.png", "https://evil.test/a", "HTTPS://evil.test", "ftp://x/a", "ws://x", "wss://x", "file:///C:/x", "//evil.test/a", "\\\\host\\share\\a", "", "about:blank"] {
            assert_eq!(doc.answer(u), Answer::Block, "{u}");
        }
    }

    #[test]
    fn the_wrapper_is_judged_by_its_own_source() {
        let mut d = wrapper_doc("<svg/>");
        let url = d.url().to_string();
        // Nothing is trusted before the view reported its document.
        assert!(!d.trusts(&url));
        d.saw_source(&url);
        assert!(d.trusts(&url));
        assert_eq!(verdict("\"ok\""), Some(true));
        assert_eq!(verdict("\"fail\""), Some(false));
        // Another source answers nothing; text outside the closed set answers nothing.
        assert!(!d.trusts("https://evil.test/") && !d.trusts("data:text/html,a"));
        for m in ["ok", "\"OK\"", "\"ok \"", "{\"ok\":1}", "true", "\"fail\" ", "", "\"o\\u006b\""] {
            assert_eq!(verdict(m), None, "{m}");
        }
        // Once the view navigated anywhere else, nothing counts any more.
        d.saw_source("https://evil.test/");
        assert!(!d.trusts(&url));
        // A first source that is not the document is never trusted.
        let mut k = wrapper_doc("<svg/>");
        k.saw_source("https://evil.test/");
        assert!(!k.trusts("https://evil.test/") && !k.trusts(k.url()));
    }

    fn png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(px));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_capture_becomes_a_128_pixel_square_png() {
        let out = normalize_png(&png(128, 128, [255, 0, 0, 255])).unwrap();
        let img = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (SIDE, SIDE));
        assert_eq!(img.get_pixel(64, 64).0, [255, 0, 0, 255]);
        // A capture at another scale is brought to the side.
        let big = normalize_png(&png(192, 192, [0, 255, 0, 255])).unwrap();
        let img = image::load_from_memory(&big).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (SIDE, SIDE));
        assert_eq!(img.get_pixel(64, 64).0, [0, 255, 0, 255]);
        assert!(normalize_png(b"not a png").is_err());
        assert!(normalize_png(&[]).is_err());
        // A claim of a huge image is refused before it is decoded.
        let mut huge = png(2, 2, [0, 0, 0, 255]);
        huge[16..20].copy_from_slice(&60000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&60000u32.to_be_bytes());
        assert!(normalize_png(&huge).is_err());
    }

    #[test]
    fn a_stored_png_is_at_most_256_kib() {
        // Noise does not compress: a 128 pixel square of it stays far under, so the bound is
        // exercised through the constant and the function's own check.
        assert_eq!(MAX_PNG_BYTES, 256 * 1024);
        let mut img = image::RgbaImage::new(SIDE, SIDE);
        let mut x = 12345u32;
        for p in img.pixels_mut() {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            *p = image::Rgba(x.to_le_bytes());
        }
        let mut raw = std::io::Cursor::new(Vec::new());
        img.write_to(&mut raw, image::ImageFormat::Png).unwrap();
        let out = normalize_png(&raw.into_inner()).unwrap();
        assert!(out.len() <= MAX_PNG_BYTES);
    }
}
