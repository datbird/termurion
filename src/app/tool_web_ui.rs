//! Dynamic toolbelt tools as web views (milestone 13x, Task 2): a tool a script registered
//! (`RegisterTool`) shows its URL in a WebView2 child of the window, in the wrapper's content
//! rectangle (iTerm2's `ToolWebView`, `TWV:45-111`). The URL comes from the registered definition
//! (the person's script through the API, or an import), never from terminal output; the view is
//! guarded by the one navigation verdict (http and https only), gets no host objects, and what the
//! page posts is the logger's closed shape, logged by its byte count only (preflight B4, B5, B8,
//! P17). The view is a child of its own window and lives in that window's `WinSlot`.

use std::collections::HashMap;

use super::*;
use crate::layout::Rect;
use crate::toolbelt::{self, ToolId};
use crate::toolbelt_layout as tbl;
use crate::webhost::{self, Event, Runtime, Store, ViewId};
use crate::webhost_win::{ViewSetup, WebView};

/// A dynamic tool a window's belt shows now: its name, its registered URL and the wrapper's
/// content rectangle (None while the wrapper is collapsed).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Wanted {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) content: Option<Rect>,
}

/// One change to a window's tool views.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Op {
    Create { name: String, url: String, content: Rect },
    Place { name: String, content: Rect },
    Hide(String),
    Close(String),
}

/// What a tool view may load: an http or https page with a host. A tool's URL is a global an
/// import or the API writes, so nothing else (no `file:`, no custom scheme) becomes a view
/// (preflight B4c); the view's own guard enforces the same list.
pub(super) fn tool_url_ok(url: &str) -> bool {
    let scheme = crate::webhost::scheme_of(url);
    (scheme == "http" || scheme == "https") && toolbelt::url_host(url).is_some()
}

/// The changes that bring a window's views (`have`: tool name to the URL it was made for) to what
/// its belt shows. A view is made when its wrapper has room, and not at all without a runtime
/// (iTerm2 draws nothing when the view cannot be made, `TBV:600-611`); a new URL under a name is a
/// new view, as iTerm2's hide and show of the old registration is; a collapsed wrapper hides it.
pub(super) fn plan(wanted: &[Wanted], have: &HashMap<String, String>, runtime_ok: bool) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut stale: Vec<&String> = have.keys().filter(|n| !wanted.iter().any(|w| &w.name == *n)).collect();
    stale.sort();
    ops.extend(stale.into_iter().map(|n| Op::Close(n.clone())));
    let mut done: Vec<&str> = Vec::new();
    for w in wanted {
        // A name listed twice shows once.
        if done.contains(&w.name.as_str()) {
            continue;
        }
        done.push(&w.name);
        let make = |ops: &mut Vec<Op>| {
            if let (true, true, Some(content)) = (runtime_ok, tool_url_ok(&w.url), w.content) {
                ops.push(Op::Create { name: w.name.clone(), url: w.url.clone(), content });
            }
        };
        match have.get(&w.name) {
            Some(url) if *url == w.url => ops.push(match w.content {
                Some(content) => Op::Place { name: w.name.clone(), content },
                None => Op::Hide(w.name.clone()),
            }),
            Some(_) => {
                ops.push(Op::Close(w.name.clone()));
                make(&mut ops);
            }
            None => make(&mut ops),
        }
    }
    ops
}

/// ToolWebView's configuration (`TWV:49-60`): the application name in the agent (the
/// `WebUserAgent` setting is ignored for tools), no automatic windows, developer tools on, the
/// logger script in every frame. The page may reach http and https only.
pub(super) fn tool_setup() -> ViewSetup {
    ViewSetup {
        user_agent: Some(String::new()),
        scripts: vec![webhost::LOGGER_JS.to_string()],
        devtools: true,
        new_windows: false,
        guard_local_schemes: true,
        allowed_schemes: Some(vec!["http".to_string(), "https".to_string()]),
        web_message: true,
        ..Default::default()
    }
}

/// The byte length of the text a page logged, when the message is the logger's closed shape
/// (`{console: [string...]}` or `{error: string}`); None for anything else. The text itself is
/// never kept past this (preflight B8).
pub(super) fn logged_bytes(json: &str) -> Option<usize> {
    match webhost::logger_message(json)? {
        webhost::LoggerMessage::Console(items) => Some(items.iter().map(String::len).sum()),
        webhost::LoggerMessage::Error(e) => Some(e.len()),
    }
}

/// A view a window holds for one tool.
#[derive(Clone, Debug)]
pub(super) struct Loaded {
    pub(super) id: ViewId,
    pub(super) url: String,
    /// The window handle the view is a child of (a window made again gets its views moved).
    pub(super) parent: isize,
}

/// The tool views whose parent is not `new`, the handle of a window made again.
pub(super) fn reparents(have: &HashMap<String, Loaded>, new: isize) -> Vec<String> {
    have.iter().filter(|(_, l)| l.parent != new).map(|(n, _)| n.clone()).collect()
}

impl App {
    /// This window was made again: its tool views move to the new handle before the old window
    /// (their parent) is destroyed.
    pub(super) fn tool_views_remade(&mut self, new_hwnd: isize) {
        for name in reparents(&self.tool_loaded, new_hwnd) {
            if let Some(v) = self.tool_views.get(&name) {
                v.set_parent(new_hwnd);
            }
            if let Some(l) = self.tool_loaded.get_mut(&name) {
                l.parent = new_hwnd;
            }
        }
    }

    /// Brings this window's tool views to the belt's layout (`tools` are the split's tools, one per
    /// wrapper). Called from `draw_toolbelt`; a COM call happens only when something changed (the
    /// host caches bounds and visibility, P24).
    pub(super) fn place_tool_views(&mut self, layout: &tbl::BeltLayout, tools: &[ToolId]) {
        let registry = toolbelt::dynamic_tools(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref());
        let mut wanted = Vec::new();
        for (w, tool) in layout.wrappers.iter().zip(tools) {
            if let ToolId::Dynamic(name) = tool {
                if let Some(d) = registry.iter().find(|d| d.name == *name) {
                    wanted.push(Wanted { name: d.name.clone(), url: d.url.clone(), content: w.content });
                }
            }
        }
        if wanted.is_empty() && self.tool_views.is_empty() {
            return;
        }
        let have: HashMap<String, String> = self.tool_loaded.iter().map(|(n, l)| (n.clone(), l.url.clone())).collect();
        let runtime_ok = !wanted.is_empty() && self.web_runtime() != Runtime::Missing;
        let hwnd = self.hwnd();
        for op in plan(&wanted, &have, runtime_ok) {
            match op {
                Op::Close(name) => self.close_tool_view(&name),
                Op::Hide(name) => {
                    if let Some(v) = self.tool_views.get(&name) {
                        v.set_visible(false);
                    }
                }
                Op::Place { name, content } => {
                    let moved = hwnd.filter(|h| self.tool_loaded.get(&name).is_some_and(|l| l.parent != *h));
                    if let (Some(h), Some(v)) = (moved, self.tool_views.get(&name)) {
                        v.set_parent(h);
                        if let Some(l) = self.tool_loaded.get_mut(&name) {
                            l.parent = h;
                        }
                    }
                    if let Some(v) = self.tool_views.get(&name) {
                        v.set_bounds(webhost::physical_bounds(content));
                        v.set_visible(true);
                    }
                }
                Op::Create { name, url, content } => {
                    let Some(hwnd) = hwnd else { continue };
                    let id = self.next_view_id();
                    let sink = self.web_envs.sink();
                    let Some(env) = self.web_env(Store::Default) else { continue };
                    let view = WebView::create(env, hwnd, id, webhost::physical_bounds(content), tool_setup(), sink);
                    view.navigate(&url);
                    view.set_visible(true);
                    self.tool_views.insert(name.clone(), view);
                    self.tool_loaded.insert(name, Loaded { id, url, parent: hwnd });
                    self.tool_view_ids.insert(id, false);
                }
            }
        }
    }

    /// The belt is hidden: its views hide with it and keep their pages.
    pub(super) fn hide_tool_views(&mut self) {
        for v in self.tool_views.values() {
            v.set_visible(false);
        }
    }

    /// Closes this window's view of the tool `name`.
    fn close_tool_view(&mut self, name: &str) {
        if let Some(l) = self.tool_loaded.remove(name) {
            self.tool_view_ids.remove(&l.id);
        }
        if let Some(v) = self.tool_views.remove(name) {
            v.close();
        }
    }

    /// The window is closing: its views close first, while their parent exists.
    pub(super) fn close_tool_views(&mut self) {
        let names: Vec<String> = self.tool_views.keys().cloned().collect();
        for n in names {
            self.close_tool_view(&n);
        }
    }

    /// A script registered the tool `identifier` again with the same name and URL
    /// (`TWV:140-146`): every window's view of it loads its URL again. A view that failed is closed
    /// instead, and made anew by the next layout.
    pub(super) fn reload_tool_view(&mut self, identifier: &str) {
        let registry = toolbelt::dynamic_tools(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref());
        let Some(name) = registry.into_iter().find(|d| d.identifier == identifier).map(|d| d.name) else { return };
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if !self.enter(id) {
                continue;
            }
            let failed = self.tool_loaded.get(&name).is_some_and(|l| self.tool_view_ids.get(&l.id).copied().unwrap_or(false));
            if failed {
                self.close_tool_view(&name);
                self.redraw();
            } else if let Some(v) = self.tool_views.get(&name) {
                v.reload();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// A tool view's event. True when the view is a tool's (the event is consumed). A page's
    /// console and error text is counted and dropped, never logged (B8); every other message shape
    /// is dropped (P17).
    pub(super) fn tool_view_event(&mut self, event: &Event) -> bool {
        let view = event.view();
        if !self.tool_view_ids.contains_key(&view) {
            return false;
        }
        match event {
            Event::Message { json, .. } => {
                if let Some(n) = logged_bytes(json) {
                    crate::host::log(&format!("webhost: tool view {view} console: {n} bytes"));
                }
            }
            Event::Failed(..) => {
                self.tool_view_ids.insert(view, true);
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use std::collections::HashMap;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Option<Rect> {
        Some(Rect { x, y, w, h })
    }
    fn want(name: &str, url: &str, content: Option<Rect>) -> Wanted {
        Wanted { name: name.into(), url: url.into(), content }
    }
    fn have(v: &[(&str, &str)]) -> HashMap<String, String> {
        v.iter().map(|(n, u)| (n.to_string(), u.to_string())).collect()
    }

    #[test]
    fn tool_views_follow_the_layout() {
        let a = r(1.0, 2.0, 30.0, 40.0);
        // A shown tool with room gets a view; a collapsed one gets none.
        assert_eq!(plan(&[want("Web", "https://a", a)], &have(&[]), true), vec![Op::Create { name: "Web".into(), url: "https://a".into(), content: a.unwrap() }]);
        assert!(plan(&[want("Web", "https://a", None)], &have(&[]), true).is_empty());
        // A made view moves with the wrapper, hides when it collapses and closes when the tool goes.
        assert_eq!(plan(&[want("Web", "https://a", a)], &have(&[("Web", "https://a")]), true), vec![Op::Place { name: "Web".into(), content: a.unwrap() }]);
        assert_eq!(plan(&[want("Web", "https://a", None)], &have(&[("Web", "https://a")]), true), vec![Op::Hide("Web".into())]);
        assert_eq!(plan(&[], &have(&[("Web", "https://a")]), true), vec![Op::Close("Web".into())]);
        // A new URL under the same name is a new view (the old tool hid and showed again).
        assert_eq!(
            plan(&[want("Web", "https://b", a)], &have(&[("Web", "https://a")]), true),
            vec![Op::Close("Web".into()), Op::Create { name: "Web".into(), url: "https://b".into(), content: a.unwrap() }]
        );
        // No runtime: nothing is made (iTerm2 draws nothing when the view cannot be made); stale views still close.
        assert!(plan(&[want("Web", "https://a", a)], &have(&[]), false).is_empty());
        assert_eq!(plan(&[], &have(&[("Web", "https://a")]), false), vec![Op::Close("Web".into())]);
        // A URL the tool view may not load is not made a view of.
        assert!(plan(&[want("Web", "file:///C:/x.html", a)], &have(&[]), true).is_empty());
    }

    /// A tool's URL comes from a registered definition that an import or the API wrote: only web pages load.
    #[test]
    fn tool_urls_are_web_pages_only() {
        for ok in ["http://localhost:8080/x", "https://example.com/a?b=c", "HTTPS://example.com"] {
            assert!(tool_url_ok(ok), "{ok}");
        }
        for bad in ["file:///C:/Windows/", "file://host/share/x", "javascript:alert(1)", "data:text/html,x", "iterm2-about:welcome", "ms-msdt://x", "//example.com", "https:///nohost", ""] {
            assert!(!tool_url_ok(bad), "{bad}");
        }
    }

    /// ToolWebView's configuration (`TWV:49-60`) with the preflight's bounds.
    #[test]
    fn tool_view_setup_follows_iterm2() {
        let s = tool_setup();
        assert_eq!(s.user_agent.as_deref(), Some(""), "the engine's agent with the application name; the setting is ignored");
        assert_eq!(s.scripts, vec![crate::webhost::LOGGER_JS.to_string()]);
        assert!(s.devtools);
        assert!(!s.new_windows, "javaScriptCanOpenWindowsAutomatically = NO");
        assert!(s.web_message, "the logger's channel");
        assert!(s.guard_local_schemes, "B4: the one navigation verdict");
        assert_eq!(s.allowed_schemes, Some(vec!["http".to_string(), "https".to_string()]));
        assert!(s.resource_filters.is_empty());
        assert!(!s.in_private, "tools keep the persistent default store (TWV:57)");
    }

    /// P17, B5: a tool view accepts only the logger's closed shape.
    #[test]
    fn tool_messages_are_the_logger_shape() {
        assert_eq!(logged_bytes(r#"{"console":["ab","cde"]}"#), Some(5));
        assert_eq!(logged_bytes(r#"{"error":"boom"}"#), Some(4));
        assert_eq!(logged_bytes(r#"{"invocation":"f()","callback":"cb"}"#), None);
        assert_eq!(logged_bytes(r#"{"console":[1]}"#), None);
        assert_eq!(logged_bytes(&format!(r#"{{"error":"{}"}}"#, "x".repeat(3000))), None);
        assert_eq!(logged_bytes("not json"), None);
    }

    /// B8: what a page logs never reaches the debug log; only a count does.
    #[test]
    fn tool_console_text_never_reaches_the_log() {
        let src = include_str!("tool_web_ui.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let logs: Vec<&str> = prod.lines().filter(|l| l.contains("host::log(") || l.contains("dlog!") || l.contains("rlog!") || l.contains("eprintln!")).collect();
        assert!(!logs.is_empty());
        for l in logs {
            for banned in ["json", "text", "Console(", "Error(", "message", "url"] {
                assert!(!l.contains(banned), "a log line names page data ({banned}): {l}");
            }
        }
        assert!(!prod.contains("eprintln!"));
    }

    /// Every tool view is made by one function, with the one setup, on the default store.
    #[test]
    fn every_tool_view_is_made_with_the_tool_setup() {
        let src = include_str!("tool_web_ui.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert_eq!(prod.matches("WebView::create(").count(), 1);
        assert!(prod.contains("tool_setup()"));
        assert!(prod.contains("Store::Default"));
        assert!(!prod.contains("AddHostObject") && !prod.contains("navigate_to_string"));
    }

    /// Review fix 1: a remade window's new handle takes every view whose parent was the old one.
    #[test]
    fn tool_views_move_to_a_remade_window() {
        let l = |p| Loaded { id: 1, url: "https://a".into(), parent: p };
        let mut have: HashMap<String, Loaded> = HashMap::new();
        have.insert("A".into(), l(10));
        have.insert("B".into(), l(20));
        have.insert("C".into(), l(20));
        let mut moved = reparents(&have, 20);
        moved.sort();
        assert_eq!(moved, vec!["A".to_string()], "only a view not already on the new handle");
        assert!(reparents(&HashMap::new(), 5).is_empty());
    }

    /// Per-window views live in the window's slot; the id map is app-wide.
    #[test]
    fn tool_views_are_per_window() {
        let w = include_str!("windows_ui.rs");
        assert!(w.contains("swap(&mut self.tool_views, &mut s.tool_views)"));
        assert!(w.contains("tool_views: HashMap<String, crate::webhost_win::WebView>"));
        assert!(!w.contains("tool_view_ids"));
    }
}
