//! The two read-only DevTools calls milestone 13x4 makes (Task 5): `Page.getResourceTree` and
//! `Page.getResourceContent`. Pure: the calls are built and the replies parsed here, and
//! `App::devtools_read` (`src/app/devtools_read_app.rs`) sends them through the app's one DevTools
//! sender.
//!
//! Security (preflight B1, accepted): both methods are in `browser_world::ALLOWED` (the one list the
//! app and the host read), but they are host-initiated reads only. Termurion's Rust code calls them; no
//! page or isolated-world script can trigger them, name their arguments or receive a raw result: a
//! page message never reaches a DevTools sender, the `frameId` and `url` of a content read come from
//! Termurion's own resource tree read, and every reply is page-derived data, size-capped here.

use serde_json::{Value, json};

/// The only methods `App::devtools_read` sends (the second, narrower gate after
/// `browser_world::ALLOWED`).
pub const ALLOWED: [&str; 2] = ["Page.getResourceTree", "Page.getResourceContent"];

/// What a refused call answers.
pub const REFUSED: &str = "devtools method not allowed";

/// What an oversize reply answers.
pub const TOO_LARGE: &str = "the resource is too large";

/// What a read past the total budget answers.
pub const OVER_BUDGET: &str = "the page's resources are too large";

/// The largest reply for one call, in the bytes of its JSON text (what `ViewGate::admit` weighs
/// for a `ScriptDone`, and the runner's `MAX_RESULT`).
pub const MAX_RESOURCE: usize = 4 * 1024 * 1024;

/// The most one read job takes in all, measured the same way.
pub const MAX_TOTAL: usize = 64 * 1024 * 1024;

/// The most frames of one tree that are read, and how deep (a page's tree is page-derived).
pub const MAX_FRAMES: usize = 256;
const MAX_DEPTH: usize = 32;

/// The longest `frameId` or `url` a content read carries.
const MAX_ARG: usize = 8192;

pub fn allowed(method: &str) -> bool {
    ALLOWED.contains(&method)
}

/// Why `devtools_read` would not send this call, or None. The method is on the list, and the
/// arguments have exactly the shape Termurion builds: `{}` for the tree, `{frameId, url}` (two strings)
/// for a content read.
pub fn refusal(method: &str, params: &Value) -> Option<&'static str> {
    if !allowed(method) {
        return Some(REFUSED);
    }
    let Some(o) = params.as_object() else { return Some(REFUSED) };
    let ok = match method {
        "Page.getResourceTree" => o.is_empty(),
        _ => {
            o.len() == 2
                && ["frameId", "url"].iter().all(|k| o.get(*k).and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.len() <= MAX_ARG))
        }
    };
    (!ok).then_some(REFUSED)
}

/// The weight of a reply: the bytes of its JSON text.
pub fn reply_weight(reply: &Value) -> usize {
    reply.to_string().len()
}

/// The arguments of `Page.getResourceContent`.
pub fn resource_content_params(frame_id: &str, url: &str) -> Value {
    json!({"frameId": frame_id, "url": url})
}

/// The frame objects of a tree, main frame first, then child frames depth first, with each frame's
/// resource list. Bounded in count and depth.
fn frames(tree: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
        if depth > MAX_DEPTH || out.len() >= MAX_FRAMES || !node.get("frame").is_some_and(Value::is_object) {
            return;
        }
        out.push(node);
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = tree.get("frameTree") {
        walk(root, 0, &mut out);
    }
    out
}

fn frame_id(node: &Value) -> Option<&str> {
    node.pointer("/frame/id").and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The frame ids of a `Page.getResourceTree` reply: the main frame, then child frames depth first.
pub fn frame_ids(tree: &Value) -> Vec<String> {
    frames(tree).into_iter().filter_map(frame_id).map(str::to_string).collect()
}

/// The main frame's id and its own URL (which carries no fragment).
pub fn main_frame(tree: &Value) -> Option<(String, String)> {
    let node = *frames(tree).first()?;
    let url = node.pointer("/frame/url").and_then(Value::as_str)?;
    Some((frame_id(node)?.to_string(), url.to_string()))
}

fn resources(node: &Value) -> impl Iterator<Item = &Value> {
    node.get("resources").and_then(Value::as_array).into_iter().flatten()
}

/// The id of the first frame, depth first, that lists a resource with this `url` or is the document
/// of this `url`.
pub fn frame_for(tree: &Value, url: &str) -> Option<String> {
    frames(tree)
        .into_iter()
        .find(|n| n.pointer("/frame/url").and_then(Value::as_str) == Some(url) || resources(n).any(|r| r.get("url").and_then(Value::as_str) == Some(url)))
        .and_then(frame_id)
        .map(str::to_string)
}

/// The `mimeType` of the first frame resource, or frame document, whose `url` is this one.
pub fn resource_mime(tree: &Value, url: &str) -> Option<String> {
    for n in frames(tree) {
        if n.pointer("/frame/url").and_then(Value::as_str) == Some(url) {
            if let Some(m) = n.pointer("/frame/mimeType").and_then(Value::as_str) {
                return Some(m.to_string());
            }
        }
        if let Some(r) = resources(n).find(|r| r.get("url").and_then(Value::as_str) == Some(url)) {
            if let Some(m) = r.get("mimeType").and_then(Value::as_str) {
                return Some(m.to_string());
            }
        }
    }
    None
}

/// The body of a `Page.getResourceContent` reply: `content` decoded from base64 when
/// `base64Encoded` is true, else its UTF-8 bytes. None without `content`, for bad base64, and for a
/// reply over `MAX_RESOURCE`.
pub fn content_bytes(reply: &Value) -> Option<Vec<u8>> {
    if reply_weight(reply) > MAX_RESOURCE {
        return None;
    }
    decode_content(reply)
}

/// The decoding alone; the caller has weighed the reply against its own cap (`Budget::take`).
fn decode_content(reply: &Value) -> Option<Vec<u8>> {
    use base64::Engine;
    let content = reply.get("content")?.as_str()?;
    if reply.get("base64Encoded").and_then(Value::as_bool).unwrap_or(false) {
        base64::engine::general_purpose::STANDARD.decode(content).ok()
    } else {
        Some(content.as_bytes().to_vec())
    }
}

/// The running total of one read job (View Source reads one body, Save Page As reads many).
#[derive(Debug)]
pub struct Budget {
    used: usize,
    per: usize,
    total: usize,
}

/// `MAX_RESOURCE` per reply and `MAX_TOTAL` in all (View Source).
impl Default for Budget {
    fn default() -> Budget {
        Budget::with_limits(MAX_RESOURCE, MAX_TOTAL)
    }
}

impl Budget {
    /// A job with its own caps (Save Page As: `page_saver::MAX_REPLY_BYTES`, `MAX_READ_TOTAL`).
    pub fn with_limits(per: usize, total: usize) -> Budget {
        Budget { used: 0, per, total }
    }

    /// Takes one content reply: its body, or why it was refused (over the per-resource cap, over the
    /// total, or no content). A refused reply costs nothing.
    pub fn take(&mut self, reply: &Value) -> Result<Vec<u8>, &'static str> {
        let w = reply_weight(reply);
        if w > self.per {
            return Err(TOO_LARGE);
        }
        if self.used.saturating_add(w) > self.total {
            return Err(OVER_BUDGET);
        }
        let bytes = decode_content(reply).ok_or("no content")?;
        self.used += w;
        Ok(bytes)
    }

    pub fn used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        json!({"frameTree": {
            "frame": {"id": "F1", "url": "https://a/", "mimeType": "text/html"},
            "childFrames": [{"frame": {"id": "F2", "url": "https://b/"}, "resources": [{"url": "https://b/i.png", "type": "Image", "mimeType": "image/png"}]}],
            "resources": [{"url": "https://a/s.css", "type": "Stylesheet", "mimeType": "text/css"}]
        }})
    }

    #[test]
    fn devtools_parse_follows_chrome() {
        let t = tree();
        assert_eq!(frame_ids(&t), ["F1", "F2"]);
        assert_eq!(main_frame(&t), Some(("F1".into(), "https://a/".into())));
        assert_eq!(frame_for(&t, "https://b/i.png"), Some("F2".into()));
        assert_eq!(frame_for(&t, "https://a/s.css"), Some("F1".into()));
        assert_eq!(frame_for(&t, "https://a/"), Some("F1".into()));
        assert_eq!(frame_for(&t, "https://c/x"), None);
        assert_eq!(resource_mime(&t, "https://b/i.png"), Some("image/png".into()));
        assert_eq!(resource_mime(&t, "https://a/"), Some("text/html".into()));
        let mut bare = tree();
        bare["frameTree"]["frame"].as_object_mut().unwrap().remove("mimeType");
        assert_eq!(resource_mime(&bare, "https://a/"), None);
        assert_eq!(content_bytes(&json!({"content": "aGk=", "base64Encoded": true})), Some(b"hi".to_vec()));
        assert_eq!(content_bytes(&json!({"content": "hi", "base64Encoded": false})), Some(b"hi".to_vec()));
        assert_eq!(content_bytes(&json!({"base64Encoded": false})), None);
        assert_eq!(content_bytes(&json!({"content": "!!", "base64Encoded": true})), None, "bad base64");
        let p = resource_content_params("F1", "https://a/");
        assert_eq!((p["frameId"].as_str(), p["url"].as_str()), (Some("F1"), Some("https://a/")));
        assert_eq!(ALLOWED, ["Page.getResourceTree", "Page.getResourceContent"]);
        for m in ["Network.enable", "Runtime.evaluate", "Page.navigate", "Page.getResourceTree ", "page.getresourcetree", ""] {
            assert!(!allowed(m), "{m}");
        }
        assert!(allowed("Page.getResourceTree") && allowed("Page.getResourceContent"));
    }

    /// A page-derived tree cannot make the parser deep or wide.
    #[test]
    fn a_hostile_tree_is_bounded() {
        let mut node = json!({"frame": {"id": "leaf", "url": "https://x/"}});
        for i in 0..200 {
            node = json!({"frame": {"id": format!("F{i}"), "url": "https://x/"}, "childFrames": [node]});
        }
        let ids = frame_ids(&json!({"frameTree": node}));
        assert_eq!(ids.len(), MAX_DEPTH + 1);
        let wide: Vec<Value> = (0..1000).map(|i| json!({"frame": {"id": format!("W{i}"), "url": "https://x/"}})).collect();
        assert_eq!(frame_ids(&json!({"frameTree": {"frame": {"id": "M", "url": "https://x/"}, "childFrames": wide}})).len(), MAX_FRAMES);
        assert_eq!(frame_ids(&json!({"frameTree": {"frame": {"id": 5}}})), Vec::<String>::new(), "a non-string id is no frame id");
        assert_eq!(main_frame(&json!({})), None);
    }

    /// B1: only the two reads, with the arguments Termurion builds.
    #[test]
    fn only_host_built_reads_are_sent() {
        assert_eq!(refusal("Page.getResourceTree", &json!({})), None);
        assert_eq!(refusal("Page.getResourceContent", &resource_content_params("F1", "https://a/")), None);
        assert_eq!(refusal("Runtime.evaluate", &json!({"expression": "1"})), Some(REFUSED));
        assert_eq!(refusal("Network.enable", &json!({})), Some(REFUSED));
        assert_eq!(refusal("Page.getResourceTree", &json!({"x": 1})), Some(REFUSED), "no extra arguments");
        assert_eq!(refusal("Page.getResourceContent", &json!({"frameId": "F1"})), Some(REFUSED));
        assert_eq!(refusal("Page.getResourceContent", &json!({"frameId": "F1", "url": "u", "x": 1})), Some(REFUSED));
        assert_eq!(refusal("Page.getResourceContent", &json!({"frameId": 1, "url": "u"})), Some(REFUSED));
        assert_eq!(refusal("Page.getResourceContent", &json!({"frameId": "F1", "url": "x".repeat(MAX_ARG + 1)})), Some(REFUSED));
        assert_eq!(refusal("Page.getResourceTree", &json!("{}")), Some(REFUSED));
        // The host list and this one agree: the narrower gate is a subset.
        for m in ALLOWED {
            assert!(crate::browser_world::method_allowed(m), "{m}");
        }
    }

    /// B1: the size caps hold per resource and in total, measured as the JSON text.
    #[test]
    fn the_caps_hold() {
        let fit = json!({"content": "a".repeat(MAX_RESOURCE - 64), "base64Encoded": false});
        assert!(reply_weight(&fit) <= MAX_RESOURCE);
        assert!(content_bytes(&fit).is_some());
        let big = json!({"content": "a".repeat(MAX_RESOURCE), "base64Encoded": false});
        assert_eq!(content_bytes(&big), None, "over the per-resource cap");
        // A job with its own caps (Save Page As) takes what the default refuses, up to its own.
        assert_eq!(Budget::default().take(&big), Err(TOO_LARGE));
        let mut wide = Budget::with_limits(2 * MAX_RESOURCE, 3 * MAX_RESOURCE);
        assert!(wide.take(&big).is_ok(), "decoded under the job's cap");
        assert_eq!(Budget::with_limits(MAX_RESOURCE / 2, 3 * MAX_RESOURCE).take(&fit), Err(TOO_LARGE));
        let mut b = Budget::default();
        assert_eq!(b.take(&big), Err(TOO_LARGE));
        assert_eq!(b.used(), 0, "a refused reply costs nothing");
        let mut taken = 0;
        while b.take(&fit).is_ok() {
            taken += 1;
        }
        assert!(taken >= 1 && taken * reply_weight(&fit) <= MAX_TOTAL, "{taken}");
        assert_eq!(b.take(&fit), Err(OVER_BUDGET));
        assert!(b.used() <= MAX_TOTAL);
        assert_eq!(b.take(&json!({"base64Encoded": true})), Err("no content"));
    }
}
