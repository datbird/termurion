//! The View Source page in a browser pane (milestone 13x4, Task 6): the answer to a request for
//! `iterm2-about:source`. The page is Tandem's HTML under `browser_pages`' per-response nonce
//! policy; the source in it is a web page's text, escaped by `view_source`. The page posts nothing
//! (`browser_pages::route` knows no `source` page), so it has no use of the bridge.

use super::*;
use crate::view_source::{self, Verdict};
use crate::webhost::Answer;

impl App {
    /// The answer for a request for a source URL, or None when `url` is not one (the other local
    /// pages' rule answers). Only a document request takes the pane's pending source.
    pub(super) fn browser_source_answer(&mut self, pane: PaneId, url: &str, context: i32) -> Option<Answer> {
        let nonce = crate::browser_pages::new_nonce();
        let css = crate::browser_js::resource("iterm-browser-common.css").unwrap_or_default();
        let mut none = None;
        let pending = self.browser_views.get_mut(&pane).map_or(&mut none, |b| &mut b.pending_source);
        let (status, mime, body) = match view_source::request(pending, url, context, css, &nonce) {
            Verdict::NotSource => return None,
            Verdict::Page(html) => (200, "text/html; charset=utf-8", html),
            Verdict::Invalid => {
                let what = view_source::INVALID_URL;
                crate::rlog!("{}", what);
                (404, "text/plain; charset=utf-8", "Not found".to_string())
            }
            Verdict::Refused => (404, "text/plain; charset=utf-8", "Not found".to_string()),
        };
        Some(Answer::Respond { status, mime: mime.to_string(), body: body.into_bytes(), headers: crate::browser_pages::response_headers(&nonce) })
    }
}

#[cfg(test)]
mod tests {
    use crate::browser_js::{NavGate, nav_gate_tab};

    /// Security (review M5): the source page links every attribute value but lower-case `javascript:`
    /// and `data:`, so a page's `JAVASCRIPT:` link reaches the host as a `NewWindow`. The one gate
    /// (`nav_gate_tab`, which `Event::NewWindow` calls) lets only web URLs open a tab.
    #[test]
    fn view_source_links_open_only_web_urls() {
        for bad in ["JAVASCRIPT:alert(1)", "javascript:alert(1)", " javascript:alert(1)", "data:text/html,<b>x</b>", "vbscript:x", "file:///C:/Windows/win.ini", "\tjava\nscript:alert(1)"] {
            assert!(!matches!(nav_gate_tab(bad), NavGate::Navigate(_)), "{bad:?} opened a tab");
        }
        assert!(matches!(nav_gate_tab("https://tandem.test/a"), NavGate::Navigate(_)));
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
}
