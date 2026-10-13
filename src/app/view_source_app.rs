//! The View Source page in a browser pane (milestone 13x4, Task 6): the answer to a request for
//! `iterm2-about:source`. The page is Termurion's HTML under `browser_pages`' per-response nonce
//! policy; the source in it is a web page's text, escaped by `view_source`. The page posts nothing
//! (`browser_pages::route` knows no `source` page), so it has no use of the bridge.

use super::*;
use crate::view_source::{self, Next, Stage, Verdict};
use serde_json::json;
use crate::webhost::Answer;

thread_local! {
    static LAST_INVALID_LOG: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// At most one "Invalid source URL" line a second.
fn invalid_log_due() -> bool {
    LAST_INVALID_LOG.with(|c| {
        let now = std::time::Instant::now();
        let due = c.get().is_none_or(|t| now.duration_since(t) >= std::time::Duration::from_secs(1));
        if due {
            c.set(Some(now));
        }
        due
    })
}

impl App {
    /// The answer for a request for a source URL, or None when `url` is not one (the other local
    /// pages' rule answers). Only a document request takes the pane's pending source.
    pub(super) fn browser_source_answer(&mut self, pane: PaneId, url: &str, context: i32) -> Option<Answer> {
        // Only the main frame can request the source: every subframe navigation to an `iterm2-about:`
        // URL is cancelled in WebView2 (`webhost::frame_nav_cancelled`), so no frame's request reaches
        // here. The main frame's `Starting` arms the pane, but arming does not depend on the event
        // order: a source document request with a pending source is the main frame's either way.
        let armed = self.browser_views.get_mut(&pane).is_some_and(|b| std::mem::take(&mut b.source_nav));
        let have = self.browser_views.get(&pane).is_some_and(|b| b.pending_source.is_some());
        let top_level = view_source::is_source_url(url) && context == crate::browser_pages::CONTEXT_DOCUMENT && (armed || have);
        let nonce = crate::browser_pages::new_nonce();
        let css = crate::browser_js::resource("iterm-browser-common.css").unwrap_or_default();
        let mut none = None;
        let pending = self.browser_views.get_mut(&pane).map_or(&mut none, |b| &mut b.pending_source);
        let (status, mime, body) = match view_source::request(pending, url, context, top_level, css, &nonce) {
            Verdict::NotSource => return None,
            Verdict::Page(html) => (200, "text/html; charset=utf-8", html),
            Verdict::Invalid => {
                // Rate-limited: a page can ask for this URL as often as it likes.
                if invalid_log_due() {
                    let what = view_source::INVALID_URL;
                    crate::rlog!("{}", what);
                }
                (404, "text/plain; charset=utf-8", "Not found".to_string())
            }
            // Logged by `render`; the pending source went with the failed page.
            Verdict::Unavailable | Verdict::Refused => (404, "text/plain; charset=utf-8", "Not found".to_string()),
        };
        Some(Answer::Respond { status, mime: mime.to_string(), body: body.into_bytes(), headers: crate::browser_pages::response_headers(&nonce) })
    }
    /// View Source from the page context menu (`LPM:201-219`, `BM:1669-1704`): read the page's source,
    /// store it as the pane's pending source and load `iterm2-about:source` in the same view. Called
    /// only from a person's menu pick. The read is for the page of this navigation epoch: a result
    /// that arrives after a navigation is dropped (Task 5 review), as is one for a closed pane.
    pub(super) fn view_page_source(&mut self, pane: PaneId) {
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let epoch = bv.reader.epoch;
        let choice = view_source::source_choice(&url, bv.status);
        self.view_source_step(pane, url, epoch, view_source::start(choice));
    }

    /// One step of the flow; each asynchronous answer re-enters here with the next step.
    fn view_source_step(&mut self, pane: PaneId, url: String, epoch: u64, next: Next) {
        let current = |app: &App| app.browser_views.get(&pane).is_some_and(|b| b.reader.current(epoch));
        if !current(self) {
            return;
        }
        match next {
            Next::Nothing => {}
            Next::Log(line) => crate::rlog!("{}", crate::dlog::name(&line)),
            Next::Show(p) => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.pending_source = Some(p);
                }
                self.browser_navigate(pane, view_source::SOURCE_URL);
            }
            Next::Read { method, params, stage } => {
                self.devtools_read(pane, method, params, crate::devtools::MAX_RESOURCE, Box::new(move |app, pane, r| {
                    if !app.browser_views.get(&pane).is_some_and(|b| b.reader.current(epoch)) {
                        return;
                    }
                    let next = match stage {
                        Stage::Tree => view_source::after_tree(r),
                        Stage::Content => view_source::after_content(r, &url),
                    };
                    app.view_source_step(pane, url, epoch, next)
                }));
            }
            // The page world, no bridge and no secret (R13x45-2).
            Next::Dom => {
                self.browser_page_call(pane, view_source::DOM_BODY, json!({}), Box::new(move |app, pane, r| {
                    if !app.browser_views.get(&pane).is_some_and(|b| b.reader.current(epoch)) {
                        return;
                    }
                    let next = view_source::after_dom(r, &url);
                    app.view_source_step(pane, url, epoch, next)
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::browser_js::{NavGate, nav_gate_tab};
    use crate::view_source::*;
    use serde_json::Value;

    /// Security (review M5): the source page links every attribute value but lower-case `javascript:`
    /// and `data:`, so a page's `JAVASCRIPT:` link reaches the host as a `NewWindow`. The one gate
    /// (`nav_gate_tab`, which `Event::NewWindow` calls) lets only web URLs open a tab.
    #[test]
    fn view_source_links_open_only_web_urls() {
        for bad in ["JAVASCRIPT:alert(1)", "javascript:alert(1)", " javascript:alert(1)", "data:text/html,<b>x</b>", "vbscript:x", "file:///C:/Windows/win.ini", "\tjava\nscript:alert(1)"] {
            assert!(!matches!(nav_gate_tab(bad), NavGate::Navigate(_)), "{bad:?} opened a tab");
        }
        assert!(matches!(nav_gate_tab("https://termurion.test/a"), NavGate::Navigate(_)));
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let nw = &app[app.find("Event::NewWindow { url, .. } =>").unwrap()..app.find("Event::ExternalUri").unwrap()];
        assert!(nw.contains("nav_gate_tab(url)") && nw.contains("NavGate::Navigate(u)"), "only a gate-approved URL opens a tab");
    }

    /// The source page is answered by one function that checks the pending source's rule, with the
    /// response headers of the other local pages (nonce policy), and the pending source is per pane.
    #[test]
    fn source_answers_carry_the_policy() {
        let src = include_str!("view_source_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("browser_pages::response_headers(&nonce)") && prod.contains("view_source::request("));
        let db = include_str!("browser_db_app.rs");
        assert!(db.contains("self.browser_source_answer(pane, url, context)"));
        // The page posts nothing: the router knows no `source` page, so the bridge gets it nothing.
        let db = crate::browser_db::Db::open_in_memory();
        let msg = serde_json::json!({"handler": "iterm2-about:source", "body": {"action": "loadEntries"}}).to_string();
        assert!(crate::browser_pages::route(&db, &msg, "iterm2-about:source").is_empty());
    }

    /// Runs the flow against the script seam's queue, as the app's callbacks do, one step at a time.
    fn drive(choice: Choice, page_url: &str, answers: Vec<Result<Value, String>>) -> (Vec<String>, Next) {
        use super::super::browser_test_seam::{Runner, ScriptSeam};
        let mut seam = ScriptSeam::default();
        seam.answers.extend(answers);
        let mut next = start(choice);
        loop {
            next = match next {
                Next::Read { method, params, stage } => {
                    assert!(crate::devtools::refusal(method, &params).is_none(), "devtools_read would send {method} {params}");
                    let r = seam.run(1, Runner::DevTools, &format!("{method} {params}"));
                    if stage == Stage::Tree { after_tree(r) } else { after_content(r, page_url) }
                }
                Next::Dom => {
                    let r = seam.run(1, Runner::Page, DOM_BODY);
                    after_dom(r, page_url)
                }
                done => return (seam.calls.iter().map(|(_, r, t)| format!("{r:?} {t}")).collect(), done),
            };
        }
    }

    fn tree(url: &str) -> Value {
        serde_json::json!({"frameTree": {"frame": {"id": "F1", "url": url, "mimeType": "text/html"}, "resources": []}})
    }

    #[test]
    fn view_source_flow_follows_iterm2() {
        let p = "https://a/#frag";
        // Cached: the tree, then the content of the main frame's own URL (never the fragment's).
        let (calls, end) = drive(Choice::Cached, p, vec![Ok(tree("https://a/")), Ok(serde_json::json!({"content": "<p>orig</p>", "base64Encoded": false}))]);
        assert_eq!(calls, vec!["DevTools Page.getResourceTree {}".to_string(), "DevTools Page.getResourceContent {\"frameId\":\"F1\",\"url\":\"https://a/\"}".to_string()]);
        assert_eq!(end, Next::Show(Pending { raw: "<p>orig</p>".into(), url: p.into() }));
        // Base64 and invalid UTF-8 decode with replacement.
        let b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode([b'a', 0xFF, b'b'])
        };
        let (_, end) = drive(Choice::Cached, p, vec![Ok(tree("https://a/")), Ok(serde_json::json!({"content": b64, "base64Encoded": true}))]);
        assert_eq!(end, Next::Show(Pending { raw: "a\u{FFFD}b".into(), url: p.into() }));
        // A failed tree, a failed or empty content, falls back to one page-world DOM read.
        for answers in [
            vec![Err("devtools".into()), Ok(Value::String("<dom>".into()))],
            vec![Ok(serde_json::json!({})), Ok(Value::String("<dom>".into()))],
            vec![Ok(tree("https://a/")), Err("nope".into()), Ok(Value::String("<dom>".into()))],
            vec![Ok(tree("https://a/")), Ok(serde_json::json!({"content": "", "base64Encoded": false})), Ok(Value::String("<dom>".into()))],
        ] {
            let (calls, end) = drive(Choice::Cached, p, answers);
            assert_eq!(calls.iter().filter(|c| c.starts_with("Page ")).count(), 1, "{calls:?}");
            assert_eq!(calls.last().unwrap(), &format!("Page {DOM_BODY}"));
            assert_eq!(end, Next::Show(Pending { raw: "<dom>".into(), url: p.into() }));
        }
        // Dom: one page-world call, no DevTools.
        let (calls, end) = drive(Choice::Dom, p, vec![Ok(Value::String("<dom>".into()))]);
        assert_eq!(calls, vec![format!("Page {DOM_BODY}")]);
        assert!(matches!(end, Next::Show(_)));
        // A DOM answer that is not a string, and an error, log and do nothing else.
        let (_, end) = drive(Choice::Dom, p, vec![Ok(Value::Null)]);
        assert_eq!(end, Next::Log("Failed to get HTML source".into()));
        let (_, end) = drive(Choice::Dom, p, vec![Err("boom".into())]);
        assert_eq!(end, Next::Log("Error getting page source: boom".into()));
        // The error text is a page's: control and hidden characters do not reach the log.
        let (_, end) = drive(Choice::Dom, p, vec![Err("a\u{1b}[31mb\nc".into())]);
        assert_eq!(end, Next::Log("Error getting page source: a[31mbc".into()));
        // A source over the cap is nothing shown.
        let (_, end) = drive(Choice::Dom, p, vec![Ok(Value::String("x".repeat(MAX_SOURCE + 1)))]);
        assert_eq!(end, Next::Log("Failed to get HTML source".into()));
        // Nothing: no call at all.
        let (calls, end) = drive(Choice::Nothing, "", vec![]);
        assert!(calls.is_empty() && end == Next::Nothing);
    }

    /// The flow's wiring: only a menu pick starts it, every asynchronous answer is checked against the
    /// navigation epoch, the DOM read is in the page world and the DevTools reads use the one door.
    #[test]
    fn view_source_is_wired_safely() {
        let src = include_str!("view_source_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert_eq!(prod.matches("reader.current(epoch)").count(), 3);
        assert!(prod.contains("self.devtools_read(pane, method, params,") && !prod.contains("browser_devtools(") && !prod.contains("browser_js_call("));
        assert!(prod.contains("self.browser_page_call(pane, view_source::DOM_BODY, json!({}),"));
        assert!(!prod.contains("format!"), "page data is never built into text with format!");
        let ctx = include_str!("browser_ctx_app.rs");
        assert!(ctx.contains("Item::ViewSource => a.view_page_source(pane),") && ctx.contains("view_source: true"));
        for (name, text) in [("browser_app.rs", include_str!("browser_app.rs")), ("browser_scripts_app.rs", include_str!("browser_scripts_app.rs")), ("browser_reader_app.rs", include_str!("browser_reader_app.rs")), ("browser_ctx_app.rs", ctx)] {
            let code = &text[..text.find("#[cfg(test)]").unwrap()];
            let n = code.matches("view_page_source(").count();
            assert_eq!(n, if name == "browser_ctx_app.rs" { 1 } else { 0 }, "{name}: only the menu pick starts View Source");
        }
        let app = include_str!("browser_app.rs");
        assert!(app.contains("bv.status = if *ok { *status } else { 0 };") && app.contains("Event::Navigated { status, .. }"));
    }

    /// The log of an invalid source URL is rate-limited, and the source is taken only after the main
    /// frame's own navigation to it armed the pane.
    #[test]
    fn source_takes_are_armed_by_the_main_frame_and_the_log_is_limited() {
        let src = include_str!("view_source_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("if invalid_log_due() {") && prod.contains("from_secs(1)"));
        assert!(prod.contains("std::mem::take(&mut b.source_nav)") && prod.contains("(armed || have)") && prod.contains("request(pending, url, context, top_level,"));
        let r = include_str!("browser_restore_app.rs");
        let r = &r[..r.find("#[cfg(test)]").unwrap()];
        let starting = &r[r.find("pub(super) fn browser_nav_starting").unwrap()..];
        assert!(starting.contains("bv.source_nav = crate::view_source::is_source_url(url);") && starting.contains("if !redirect {"));
        assert!(!starting.contains("source_nav") || starting.find("source_nav").unwrap() < starting.find("nav_state.starting").unwrap());
    }
}
