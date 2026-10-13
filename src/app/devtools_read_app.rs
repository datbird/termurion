//! The allowlisted DevTools reads (milestone 13x4 Task 5). `devtools_read` is the one door View
//! Source and Save Page As use for `Page.getResourceTree` and `Page.getResourceContent`; it is called
//! by Termurion's own Rust code only, with arguments from Termurion's own resource tree read, and what comes
//! back is page-derived data (size-capped; see `crate::devtools`).

use super::browser_scripts_app::JsDone;
use super::*;
use serde_json::Value;

impl App {
    /// A host-initiated read. Refused (`done(Err("devtools method not allowed"))`, nothing sent) for
    /// any other method or any arguments Termurion would not build; a reply over `cap` (bytes of its JSON
    /// text: `devtools::MAX_RESOURCE` for View Source and the tree, `page_saver::MAX_REPLY_BYTES` for
    /// a saved resource) is an error. `done` always runs, also when the pane or page is gone (the
    /// runner answers it).
    pub(super) fn devtools_read(&mut self, pane: PaneId, method: &str, params: Value, cap: usize, done: JsDone) {
        if let Some(why) = crate::devtools::refusal(method, &params) {
            done(self, pane, Err(why.into()));
            return;
        }
        // The reply is page-derived: over the per-resource cap it is an error, never passed on.
        self.browser_devtools_capped(
            pane,
            method,
            params,
            cap,
            Box::new(move |app, pane, r| {
                let r = match r {
                    Ok(v) if crate::devtools::reply_weight(&v) > cap => Err(crate::devtools::TOO_LARGE.into()),
                    r => r,
                };
                done(app, pane, r)
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        src.split("\n#[cfg(test)]").next().unwrap_or(src)
    }

    fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("missing {from}"));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("missing {to}"));
        &src[a..b]
    }

    /// Every source file of the app folder, test modules cut off.
    fn app_sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
            .map(|e| (e.file_name().to_string_lossy().to_string(), prod(&std::fs::read_to_string(e.path()).unwrap()).to_string()))
            .collect();
        files.sort();
        files
    }

    /// Security: `devtools_read` asks `devtools::refusal` before it sends anything, answers the refusal
    /// to `done`, and caps the reply.
    #[test]
    fn devtools_read_refuses_other_methods() {
        let me = prod(include_str!("devtools_read_app.rs"));
        let f = between(me, "pub(super) fn devtools_read", "\n    }\n");
        assert!(f.contains("crate::devtools::refusal(method, &params)"), "the narrower gate is asked");
        assert!(f.find("refusal(").unwrap() < f.find("self.browser_devtools_capped(").unwrap(), "before the send");
        assert!(f.contains("done(self, pane, Err(why.into()))") && f.contains("return;"), "the refusal answers done and sends nothing");
        assert_eq!(me.matches("browser_devtools_capped(").count(), 1, "one send");
        assert!(f.contains("reply_weight(&v) > cap") && f.contains("TOO_LARGE"), "the reply is capped per resource, at the caller's cap");
        // The callers' caps: View Source at the DevTools cap, Save Page As at its stated one.
        assert!(include_str!("view_source_app.rs").contains("self.devtools_read(pane, method, params, crate::devtools::MAX_RESOURCE,"));
    }

    /// Security (B1): a page or isolated-world message cannot reach a DevTools read. The message
    /// router and every handler take a `Routed`; none names a DevTools sender, and `devtools_read` is
    /// called only from the files of the reads (Tasks 7 and 9).
    #[test]
    fn a_page_message_cannot_ask_for_a_devtools_read() {
        let files = app_sources();
        let router = files.iter().find(|(n, _)| n == "browser_scripts_app.rs").unwrap().1.as_str();
        let msg = between(router, "pub(super) fn browser_message", "pub(super) fn browser_page_open");
        for bad in ["devtools_read", "devtools_send", "browser_devtools", "getResource"] {
            // (`browser_devtools` also matches `browser_devtools_capped`, `devtools_send` its capped form.)
            assert!(!msg.contains(bad), "the router names {bad}");
        }
        for (name, src) in &files {
            if src.contains("devtools_read(") && !matches!(name.as_str(), "devtools_read_app.rs" | "view_source_app.rs" | "page_saver_app.rs") {
                panic!("{name} calls devtools_read");
            }
            // A handler that takes a routed (page or world) message never reaches a sender.
            for chunk in src.split("fn ").skip(1) {
                let (head, body) = chunk.split_once("\n    }\n").unwrap_or((chunk, ""));
                let _ = body;
                if head.lines().next().is_some_and(|l| l.contains("Routed")) {
                    for bad in ["devtools_read", "devtools_send", "browser_devtools"] {
                        assert!(!head.contains(bad), "{name}: a Routed handler names {bad}");
                    }
                }
            }
        }
        // No script text (a body Termurion evaluates) names a read either.
        for (name, src) in &files {
            if name != "devtools_read_app.rs" {
                assert!(!src.contains("\"Page.getResource") || matches!(name.as_str(), "view_source_app.rs" | "page_saver_app.rs"), "{name} names a read method");
            }
        }
    }
}
