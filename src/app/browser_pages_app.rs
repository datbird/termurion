//! File and ssh pages on the app side (milestone 13x8, Task 1): which page the main frame is on and
//! the answer to a request for an `iterm2-file:` or `iterm2-ssh:` URL. Every answer carries the
//! sandbox policy (`browser_pages::sandbox_headers`); a request that is not the pane's own
//! navigation or a subresource of its page's tree is a 403 with no text. Tasks 3 and 9 add the
//! arms that serve a file, a listing and a remote file; until then every allowed request is a 404.

use super::*;
use crate::browser_pages::{self, NOT_FOUND_HTML};
use crate::webhost::Answer;

impl App {
    /// A main-frame navigation began and the one verdict let it go (`Event::Starting`): a file or
    /// ssh URL sets the pane's page root, any other URL clears it. Redirects into a local scheme
    /// never get this far (`webhost::navigation_allowed`).
    pub(super) fn browser_page_starting(&mut self, pane: PaneId, url: &str) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.page_root = browser_pages::page_root(url);
        bv.page_url = bv.page_root.is_some().then(|| url.to_string());
    }

    /// The answer to a request for an `iterm2-file:` or `iterm2-ssh:` URL.
    pub(super) fn browser_page_request(&mut self, pane: PaneId, token: u64, url: &str, context: i32) {
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let allowed = browser_pages::page_request_allowed(bv.page_root.as_ref(), bv.page_url.as_deref(), url, context);
        let status = if allowed { 404 } else { 403 };
        let answer = Answer::Respond { status, mime: "text/html; charset=utf-8".to_string(), body: NOT_FOUND_HTML.as_bytes().to_vec(), headers: browser_pages::sandbox_headers() };
        if let Some(v) = bv.view.as_ref() {
            v.answer_request(token, answer);
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_pages_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// Every answer of a file or ssh request is built in one place with the sandbox headers, and the
    /// verdict is the pure rule's.
    #[test]
    fn file_and_ssh_answers_are_sandboxed() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_page_request").unwrap()..];
        assert!(f.contains("browser_pages::page_request_allowed(bv.page_root.as_ref(), bv.page_url.as_deref(), url, context)"));
        assert_eq!(f.matches("Answer::Respond").count(), 1, "one place builds the answer");
        assert!(f.contains("headers: browser_pages::sandbox_headers()"));
        let db = include_str!("browser_db_app.rs");
        let db = &db[..db.find("#[cfg(test)]").unwrap()];
        let r = &db[db.find("fn browser_local_request").unwrap()..db.find("pub(super) fn local_page_message").unwrap()];
        assert!(r.contains("browser_pages::FILE_SCHEME | browser_pages::SSH_SCHEME") && r.contains("self.browser_page_request(pane, token, url, context);"));
        assert!(r.find("browser_page_request").unwrap() < r.find("adblock_answer").unwrap(), "the ad blocker never sees a file or ssh URL");
    }

    /// The page root is set from the one Starting arm, ahead of the other consumers, and a view's
    /// reset clears it; a file page's messages are dropped before any handler.
    #[test]
    fn the_page_root_follows_the_main_frame() {
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let start = &app[app.find("Event::Starting { url, kind, user, redirect, .. } => {").unwrap()..];
        assert!(start.find("self.browser_page_starting(pane, url)").unwrap() < start.find("self.marks_method_event(pane, e)").unwrap());
        let scripts = include_str!("browser_scripts_app.rs");
        let scripts = &scripts[..scripts.find("#[cfg(test)]").unwrap()];
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..scripts.find("// --- Setup ---").unwrap()];
        assert!(reset.contains("bv.page_root = None") && reset.contains("bv.page_url = None"));
        let router = &scripts[scripts.find("pub(super) fn browser_message").unwrap()..];
        let router = &router[..router.find("match r.handler.as_str()").unwrap()];
        assert!(router.contains("b.page_root.is_some()") && router.contains("return;"));
        let p = prod();
        assert!(!p.contains("std::fs") && !p.contains("localfs"), "this file reads no file; the serving arms live on a worker (Tasks 3, 9)");
    }
}
