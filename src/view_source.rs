//! The View Source page (milestone 13x4, Task 6): iTerm2's source page generator
//! (`ViewSource/view-source.html`, `SH` = its scheme handler) and the rule for when Tandem serves it.
//! Pure. The SOURCE is a web page's text and the URL is a web page's URL, so both ride the template
//! as inert data: the source is HTML-escaped (and so can never open a tag, a script or an attribute
//! in Tandem's `iterm2-about:` origin), the URL is escaped for the visible line and is a JSON string
//! (`<`, `>`, `&` as `\u` escapes) in the script, and the template is filled in ONE pass so a value
//! that holds `{{NONCE}}`, `{{URL}}` or `</script>` is never read as a slot. The response also
//! carries `browser_pages`' per-load nonce policy; the one script is the copied highlighter.

use crate::browser_js::resource;

pub const SOURCE_URL: &str = "iterm2-about:source";
pub const NO_SOURCE: &str = "No source available";
pub const INVALID_URL: &str = "Invalid source URL";
pub const GLYPH_SPAN: &str = "<span style=\"font-family:'Segoe Fluent Icons'\">&#xE943;</span>";
/// The most source text a page shows: Task 5's DevTools result cap (a larger page never gets here).
pub const MAX_SOURCE: usize = crate::devtools::MAX_RESOURCE;
const SCHEME: &str = "iterm2-about:";
/// iTerm2's emoji in the header (`SH` template line 1 of the body), shown as a glyph here.
const EMOJI: &str = "\u{1F4BB}";

/// A page's source waiting for the pane's next `iterm2-about:source` document request.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub raw: String,
    pub url: String,
}

impl Pending {
    /// None when the source is over the cap (nothing is shown, as a failed read).
    pub fn new(raw: String, url: String) -> Option<Pending> {
        (raw.len() <= MAX_SOURCE).then_some(Pending { raw, url })
    }
}

/// iTerm2's escaping of the source (`SH:17-25`): `&`, `<`, `>`, `"`, `'`, newline to `<br/>`, then
/// two spaces to `&nbsp;&nbsp;` (pairs from the left); tabs stay.
pub fn escape_source(raw: &str) -> String {
    escape_html(raw).replace('\n', "<br/>").replace("  ", "&nbsp;&nbsp;")
}

/// The first five of them (`SI:391-399`).
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// A JSON string literal that is also safe inside a `<script>` element: no `<`, `>` or `&`.
fn script_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()).replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026")
}

/// One pass over the template: each `{{KEY}}` of `subs` becomes its value, which is never scanned
/// again. (`browser_js::fill_template` would put its own CSS in first, and would read the source.)
fn fill(src: &str, subs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        match tail.find("}}") {
            Some(end) => match subs.iter().find(|(k, _)| *k == &tail[..end]) {
                Some((_, v)) => {
                    out.push_str(v);
                    rest = &tail[end + 2..];
                }
                None => {
                    out.push_str("{{");
                    rest = tail;
                }
            },
            None => {
                out.push_str("{{");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn render(source_html: &str, url: &str, common_css: &str, nonce: &str) -> String {
    let template = resource("view-source.html").unwrap_or_default().replace(EMOJI, GLYPH_SPAN);
    fill(&template, &[("COMMON_CSS", common_css), ("NONCE", nonce), ("URL", &escape_html(url)), ("URL_JSON", &script_string(url)), ("SOURCE", source_html)])
}

/// The page for a source and the URL it came from (`SH:15-32`), under this response's nonce.
pub fn page(raw: &str, url: &str, common_css: &str, nonce: &str) -> String {
    let mut end = raw.len().min(MAX_SOURCE);
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    render(&escape_source(&raw[..end]), url, common_css, nonce)
}

/// Nothing is waiting (`SH:53-57`); iTerm2 leaves `{{URL}}` showing, Tandem shows an empty line.
pub fn fallback_page(common_css: &str, nonce: &str) -> String {
    render(NO_SOURCE, "", common_css, nonce)
}

/// The waiting page, taken (it serves once), else the fallback (`SH:48-57`).
pub fn serve(pending: &mut Option<Pending>, common_css: &str, nonce: &str) -> String {
    match pending.take() {
        Some(p) => page(&p.raw, &p.url, common_css, nonce),
        None => fallback_page(common_css, nonce),
    }
}

/// What a request for an `iterm2-about:` URL is to View Source.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Not a source URL: the other local pages' rule decides.
    NotSource,
    /// The page, the pending source taken.
    Page(String),
    /// `iterm2-about:source` as anything but a document request (a web page's image, script or
    /// fetch): a 404 that leaves the pending source for the navigation it belongs to.
    Refused,
    /// `iterm2-about:source` plus a query, fragment or more (`SH:43-46`): a 404; the text is logged.
    Invalid,
}

/// Whether `url` names the source page, and what a request for it gets. Only a document request
/// takes the pending source (`browser_pages::CONTEXT_DOCUMENT`).
pub fn request(pending: &mut Option<Pending>, url: &str, context: i32, common_css: &str, nonce: &str) -> Verdict {
    let Some((scheme, rest)) = url.split_once(':') else { return Verdict::NotSource };
    if !scheme.eq_ignore_ascii_case(&SCHEME[..SCHEME.len() - 1]) || !rest.starts_with("source") {
        return Verdict::NotSource;
    }
    if rest != "source" {
        return Verdict::Invalid;
    }
    if context != crate::browser_pages::CONTEXT_DOCUMENT {
        return Verdict::Refused;
    }
    Verdict::Page(serve(pending, common_css, nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_pages::CONTEXT_DOCUMENT;

    #[test]
    fn source_page_follows_iterm2() {
        assert_eq!(escape_source("<a href=\"x\">'&'</a>\n  t\tz"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;<br/>&nbsp;&nbsp;t\tz");
        assert_eq!(escape_source("   "), "&nbsp;&nbsp; ");
        assert_eq!(escape_html("a'b\"<"), "a&#39;b&quot;&lt;");
        let p = page("<p>{{URL}}</p>", "https://a/?q='x'", "CSS", "N1");
        assert!(p.contains("<title>View Source</title>"));
        assert!(p.contains("<p class=\"url\">https://a/?q=&#39;x&#39;</p>"));
        assert!(p.contains("const currentURL = \"https://a/?q='x'\";"), "the script's URL is a JSON string (P12)");
        assert!(p.contains("&lt;p&gt;{{URL}}&lt;/p&gt;"), "the literal braces survive");
        assert!(p.contains("CSS") && !p.replace("{{URL}}", "").contains("{{"), "no slot is left, only the source's own braces");
        assert!(p.contains(&format!("{GLYPH_SPAN} Page Source")) && !p.contains('\u{1F4BB}'));
        assert!(p.contains("<script nonce=\"N1\">"), "the one script carries the response's nonce");
        assert_eq!(p.matches("<script").count(), 1);
    }

    #[test]
    fn the_script_url_is_typed_data() {
        for url in ["https://a/?q=\\", "https://a/?a=1&b=2", "https://a/#\"'", "https://a/#</script><img src=x onerror=alert(1)>"] {
            let p = page("x", url, "", "N");
            let line = p.lines().find(|l| l.contains("const currentURL = ")).unwrap().trim();
            let json = line.strip_prefix("const currentURL = ").unwrap().strip_suffix(';').unwrap();
            let back: String = serde_json::from_str(json).unwrap_or_else(|e| panic!("{url}: {json}: {e}"));
            assert_eq!(back, url, "the literal decodes to the URL");
            assert!(!json.contains(['<', '>', '&']), "{json}");
            assert_eq!(p.matches("</script>").count(), 1, "{url}: a URL cannot end the script");
            assert!(!p.contains("<img"), "{url}");
        }
    }

    /// Security: hostile source is text. It cannot open a tag, end the script, plant a nonce or fill a slot.
    #[test]
    fn hostile_source_stays_inert() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let raw = "</script><script nonce=\"FAKE\">alert(1)</script><img src=x onerror=alert(1)>{{NONCE}}{{URL}}{{SOURCE}}{{COMMON_CSS}}<!--";
        let p = page(raw, "https://a/", "", nonce);
        assert_eq!(p.matches("<script").count(), 1, "no script but the template's");
        assert_eq!(p.matches("</script>").count(), 1);
        assert!(!p.contains("<img"));
        assert_eq!(p.matches("<!--").count(), resource("view-source.html").unwrap().matches("<!--").count(), "no comment opener from the source");
        assert!(p.contains("{{NONCE}}{{URL}}") && p.contains("&lt;!--"));
        assert_eq!(p.matches("nonce=\"").count(), 1, "only Tandem's nonce attribute exists");
        assert!(p.contains(&format!("<script nonce=\"{nonce}\">")));
        assert!(!p.contains("FAKE\">") && p.contains("&lt;script nonce=&quot;FAKE&quot;&gt;"), "the fake nonce is escaped text");
        assert!(p.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(p.contains("{{NONCE}}{{URL}}{{SOURCE}}{{COMMON_CSS}}"), "slots in the source are not filled");
        // The source lies inside the one code element and holds no tag of its own.
        let code = &p[p.find("class=\"language-html source-code\">").unwrap()..];
        let code = &code[code.find('>').unwrap() + 1..code.find("</code>").unwrap()];
        assert!(!code.contains('<'), "{code}");
    }

    #[test]
    fn source_serving_follows_iterm2() {
        let mut pending = Pending::new("P".into(), "https://a/".into());
        let first = serve(&mut pending, "C", "N");
        assert!(first.contains("<code class=\"language-html source-code\">P</code>") && pending.is_none());
        let second = serve(&mut pending, "C", "N");
        assert!(second.contains(NO_SOURCE) && second.contains(&format!("{GLYPH_SPAN} Page Source")) && !second.contains('\u{1F4BB}') && !second.contains("{{"));
        assert_eq!(SOURCE_URL, "iterm2-about:source");
        assert!(Pending::new("x".repeat(MAX_SOURCE + 1), String::new()).is_none(), "over the cap, nothing is shown");
        assert!(Pending::new("x".repeat(MAX_SOURCE), String::new()).is_some());
    }

    /// Preflight B2: a web page's subresource request for the source URL must not take the person's source.
    #[test]
    fn source_not_consumed_by_a_subresource() {
        let mut pending = Pending::new("SECRET-SRC".into(), "https://a/".into());
        for context in [0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] {
            assert_eq!(request(&mut pending, SOURCE_URL, context, "", "N"), Verdict::Refused, "context {context}");
            assert!(pending.is_some(), "context {context} left the source");
        }
        assert_eq!(request(&mut pending, "iterm2-about:source?x", CONTEXT_DOCUMENT, "", "N"), Verdict::Invalid);
        assert_eq!(request(&mut pending, "iterm2-about:source#f", CONTEXT_DOCUMENT, "", "N"), Verdict::Invalid);
        assert!(pending.is_some(), "an invalid URL leaves it too");
        assert_eq!(request(&mut pending, "iterm2-about:history", CONTEXT_DOCUMENT, "", "N"), Verdict::NotSource);
        assert_eq!(request(&mut pending, "https://a/source", CONTEXT_DOCUMENT, "", "N"), Verdict::NotSource);
        match request(&mut pending, "ITERM2-ABOUT:source", CONTEXT_DOCUMENT, "", "N") {
            Verdict::Page(p) => assert!(p.contains("SECRET-SRC")),
            other => panic!("{other:?}"),
        }
        assert!(pending.is_none());
    }
}
