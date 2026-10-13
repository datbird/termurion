//! cc-status (milestone 12f): the hook Claude Code runs on each event, ported from iTerm2's
//! `cc-status/Sources/cc-status/main.swift` (`CS`). It maps a hook event to a Session Status
//! (status, colors, detail, background task count) and reports it to Termurion's Python API as
//! `iterm2.set_status(...)`. Its input is untrusted: any local program can run it and feed it
//! any bytes, so everything shown is capped and every failure is a quiet exit 0. Pure here;
//! `real_main` (Windows) wires stdin, the environment and the socket.

use serde_json::Value;

/// The status colors (`CS:44-187`).
pub const WORKING: &str = "#ff9500";
pub const WAITING: &str = "#5f87ff";
pub const IDLE_DOT: &str = "#00d75f";
pub const IDLE_TEXT: &str = "#888888";

/// The longest detail (`CS` `condense`'s default limit).
pub const DETAIL_LIMIT: usize = 180;

/// The longest path shown before it is shortened (`CS` `shortPath`).
pub const PATH_LIMIT: usize = 60;

/// The invocation that reads the stored background task count.
pub const GET_COUNT: &str = "iterm2.get_background_task_count()";

/// The reason the app gives for a status whose cookie is not its session's (ruling P14); the
/// hook drops it without a word.
pub const DROPPED: &str = "The cookie does not belong to that session.";

/// What one hook event changes. A `None` field is left alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub status: Option<String>,
    pub dot_color: Option<String>,
    pub text_color: Option<String>,
    pub detail: Option<String>,
    pub background_tasks: Option<i64>,
}

impl Update {
    /// `iterm2.set_status(...)` with the fields that are set, in it2's order (`SessionCommand.swift:985-1012`).
    /// Strings are JSON-quoted, so a backslash is doubled and no text can open an interpolation.
    pub fn invocation(&self) -> String {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
        let mut args = Vec::new();
        if let Some(v) = &self.status {
            args.push(format!("status: {}", q(v)));
        }
        if let Some(v) = &self.text_color {
            args.push(format!("text_color: {}", q(v)));
        }
        if let Some(v) = &self.dot_color {
            args.push(format!("dot_color: {}", q(v)));
        }
        if let Some(v) = &self.detail {
            args.push(format!("detail: {}", q(v)));
        }
        if let Some(n) = self.background_tasks {
            args.push(format!("background_tasks: {n}"));
        }
        format!("iterm2.set_status({})", args.join(", "))
    }

    fn paint(&mut self, status: &str, dot: &str, text: &str, detail: Option<String>) {
        self.status = Some(status.to_string());
        self.dot_color = Some(dot.to_string());
        self.text_color = Some(text.to_string());
        self.detail = detail;
    }

    fn working(&mut self, detail: String) {
        self.paint("working", WORKING, WORKING, Some(detail));
    }

    fn idle(&mut self, detail: String) {
        self.paint("idle", IDLE_DOT, IDLE_TEXT, Some(detail));
    }
}

/// The session id after the first colon of `TERM_SESSION_ID` (`w0t0p0:<id>`, `CS:5-17`).
pub fn session_id(term_session_id: Option<&str>) -> Option<String> {
    let (_, id) = term_session_id?.split_once(':')?;
    (!id.is_empty()).then(|| id.to_string())
}

fn text<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event.get(key).and_then(Value::as_str)
}

/// The status for a hook event; `None` is a quiet exit (`CS:44-187`). `stored` reads the stored
/// count and is called only where iTerm2 reads it. Whatever the payload holds, the detail
/// that comes out is at most `DETAIL_LIMIT` characters.
pub fn map(event: &Value, stored: &mut dyn FnMut() -> i64) -> Option<Update> {
    let name = text(event, "hook_event_name")?;
    let mut u = Update::default();
    let last_message = |e: &Value| text(e, "last_assistant_message").map_or_else(String::new, |m| condense(m, DETAIL_LIMIT));
    match name {
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => u.working(String::new()),
        "PermissionRequest" => u.paint("waiting", WAITING, WAITING, permission_detail(event)),
        "Notification" => {
            let kind = text(event, "notification_type");
            if kind == Some("idle_prompt") {
                let running = background_count(event, None).unwrap_or_else(|| stored().max(0));
                if running > 0 {
                    u.working(background_detail(running));
                } else {
                    u.idle(last_message(event));
                }
            } else {
                let detail = if kind == Some("permission_prompt") { None } else { text(event, "message").map(|m| condense(m, DETAIL_LIMIT)) };
                u.paint("waiting", WAITING, WAITING, detail);
            }
        }
        "Stop" => {
            let counted = background_count(event, None);
            u.background_tasks = counted;
            match counted.unwrap_or(0) {
                n if n > 0 => u.working(background_detail(n)),
                _ => u.idle(last_message(event)),
            }
        }
        "SubagentStop" => {
            let remaining = background_count(event, text(event, "agent_id"))?;
            u.background_tasks = Some(remaining);
            if remaining > 0 {
                u.working(background_detail(remaining));
            }
        }
        "StopFailure" => {
            let counted = background_count(event, None);
            u.background_tasks = counted;
            let running = counted.unwrap_or_else(|| stored().max(0));
            if running > 0 {
                u.working(background_detail(running));
            } else {
                u.idle(String::new());
            }
        }
        "SessionStart" | "SessionEnd" => {
            u.idle(String::new());
            u.background_tasks = Some(0);
        }
        _ => return None,
    }
    u.detail = u.detail.map(|d| condense(&d, DETAIL_LIMIT));
    Some(u)
}

/// Whitespace runs collapsed to one space, cut to `limit` characters with an ellipsis (`CS:430-440`).
pub fn condense(s: &str, limit: usize) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(limit.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// How many background tasks the payload lists as running, `None` when it has no such field
/// (`CS:274-330`): unknown statuses count as active, `excluding` drops the agent that just stopped.
pub fn background_count(event: &Value, excluding: Option<&str>) -> Option<i64> {
    const FINISHED: [&str; 7] = ["completed", "failed", "cancelled", "canceled", "killed", "stopped", "done"];
    match event.get("background_tasks")? {
        Value::Array(items) => {
            let active = items.iter().filter(|e| {
                let Some(o) = e.as_object() else { return true };
                if let (Some(skip), Some(id)) = (excluding, o.get("id").and_then(Value::as_str)) {
                    if skip == id {
                        return false;
                    }
                }
                !o.get("status").and_then(Value::as_str).is_some_and(|st| FINISHED.contains(&st.to_lowercase().as_str()))
            });
            Some(active.count() as i64)
        }
        Value::Object(o) => Some(o.len() as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

pub fn background_detail(n: i64) -> String {
    if n == 1 { "1 background task running".to_string() } else { format!("{n} background tasks running") }
}

/// "Allow <tool>: <key field>?" for a PermissionRequest payload (`CS:333-360`).
pub fn permission_detail(event: &Value) -> Option<String> {
    let tool = text(event, "tool_name")?;
    let empty = Value::Null;
    let input = event.get("tool_input").filter(|v| v.is_object()).unwrap_or(&empty);
    if tool == "AskUserQuestion" {
        let asked: Vec<&str> = input.get("questions").and_then(Value::as_array).into_iter().flatten().filter_map(|q| q.get("question").and_then(Value::as_str)).collect();
        if !asked.is_empty() {
            return Some(condense(&asked.join(" "), DETAIL_LIMIT));
        }
    }
    if tool == "ExitPlanMode" {
        return Some("Review proposed plan?".to_string());
    }
    Some(format!("Allow {}?", tool_summary(tool, input)))
}

/// One line naming a tool call by its most identifying field (`CS:363-413`).
pub fn tool_summary(name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    match name {
        "Bash" => condense(&format!("Bash: {}", field("command")), DETAIL_LIMIT),
        "Read" | "Edit" | "Write" | "MultiEdit" => format!("{name}: {}", short_path(field("file_path"), PATH_LIMIT)),
        "NotebookEdit" => format!("NotebookEdit: {}", short_path(field("notebook_path"), PATH_LIMIT)),
        "Grep" => {
            let mut s = format!("Grep \u{201C}{}\u{201D}", field("pattern"));
            if !field("glob").is_empty() {
                s.push_str(&format!(" in {}", field("glob")));
            } else if !field("path").is_empty() {
                s.push_str(&format!(" in {}", short_path(field("path"), PATH_LIMIT)));
            }
            s
        }
        "Glob" => format!("Glob: {}", field("pattern")),
        "Agent" | "Task" => format!("{name}: {}", field("description")),
        "WebFetch" => format!("WebFetch: {}", short_url(field("url"))),
        "WebSearch" => format!("WebSearch: {}", field("query")),
        _ => {
            if let Some(rest) = name.strip_prefix("mcp__") {
                let parts: Vec<&str> = rest.split("__").filter(|p| !p.is_empty()).collect();
                if parts.len() >= 2 {
                    return format!("{}/{}", parts[0], parts[parts.len() - 1]);
                }
            }
            humanize(name)
        }
    }
}

/// A PascalCase identifier split into words, acronym runs kept together (`CS:416-441`).
pub fn humanize(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && !current.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower) {
                words.push(std::mem::take(&mut current));
            }
        }
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() { name.to_string() } else { words.join(" ") }
}

/// A long path shortened to `...<sep>parent<sep>file`, split on both separators and joined with the
/// path's own (a backslash when it has one).
pub fn short_path(p: &str, limit: usize) -> String {
    if p.chars().count() <= limit {
        return p.to_string();
    }
    let sep = if p.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [] => p.to_string(),
        [file] => format!("\u{2026}{sep}{file}"),
        [.., parent, file] => format!("\u{2026}{sep}{parent}{sep}{file}"),
    }
}

/// A URL shortened to `host/<first path part>`; text that is no URL with a host stays as it is (`CS:466-480`).
pub fn short_url(u: &str) -> String {
    let Some((_, rest)) = u.split_once("://") else { return u.to_string() };
    let (authority, path) = rest.split_once(['/', '?', '#']).map_or((rest, ""), |(a, _)| (a, &rest[a.len()..]));
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') { host.split_once(']').map_or(host, |(h, _)| &host[..=h.len()]) } else { host.split(':').next().unwrap_or("") };
    if host.is_empty() {
        return u.to_string();
    }
    let first = path.strip_prefix('/').and_then(|p| p.split(['/', '?', '#']).next()).unwrap_or("");
    if first.is_empty() { host.to_string() } else { format!("{host}/{first}") }
}

/// Whether a request whose cookie is `request_cookie` may report for the pane whose own cookie
/// is `pane_cookie` (ruling P14). A hosted shell taken back after a relaunch keeps its old
/// `TERM_SESSION_ID` while pane ids restart, so the id alone could name an unrelated pane; the
/// cookie minted for that pane is the proof.
pub fn may_report(pane_cookie: Option<&str>, request_cookie: Option<&str>) -> bool {
    match (pane_cookie, request_cookie) {
        (Some(a), Some(b)) if !a.is_empty() => crate::api_auth::same_secret(a.as_bytes(), b.as_bytes()),
        _ => false,
    }
}

/// The API as the hook uses it.
pub trait Api {
    fn invoke(&mut self, session: &str, invocation: &str) -> Result<Value, String>;
}

/// One hook run: always 0. A failure prints one line (never a cookie) unless the app dropped the status.
pub fn run(term_session_id: Option<&str>, stdin: &str, api: &mut dyn Api, err: &mut dyn std::io::Write) -> i32 {
    let Some(session) = session_id(term_session_id) else { return 0 };
    let Ok(event) = serde_json::from_str::<Value>(stdin) else { return 0 };
    let Some(name) = text(&event, "hook_event_name").map(str::to_string) else { return 0 };
    let mut stored = || match api.invoke(&session, GET_COUNT) {
        Ok(Value::Number(n)) => n.as_i64().unwrap_or(0).max(0),
        Ok(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0).max(0),
        _ => 0,
    };
    let Some(update) = map(&event, &mut stored) else { return 0 };
    if let Err(reason) = api.invoke(&session, &update.invocation()) {
        if reason != DROPPED {
            let _ = writeln!(err, "cc-status: could not update Termurion for {}: {reason}", crate::inert_files::shown_name(&name));
        }
    }
    0
}

/// Reports a status over a connection made only when something is sent.
#[cfg(windows)]
struct Lazy {
    cookie: Option<String>,
    key: Option<String>,
    client: Option<Result<crate::api_client::Client, String>>,
}

#[cfg(windows)]
impl Api for Lazy {
    fn invoke(&mut self, session: &str, invocation: &str) -> Result<Value, String> {
        let (cookie, key) = (self.cookie.as_deref(), self.key.as_deref());
        let c = self.client.get_or_insert_with(|| crate::api_client::Client::connect(cookie, key, "cc-status", std::time::Duration::from_secs(5)));
        match c {
            Ok(c) => c.invoke(session, invocation),
            Err(e) => Err(e.clone()),
        }
    }
}

/// The hook as a program: stdin (read to its end, at most 16 MiB), `TERM_SESSION_ID`,
/// `ITERM2_COOKIE` and `ITERM2_KEY`, one connection (5 s) made only when `map` gave an update.
#[cfg(windows)]
pub fn real_main() -> i32 {
    use std::io::Read;
    let mut raw = Vec::new();
    let _ = std::io::stdin().lock().take(16 * 1024 * 1024).read_to_end(&mut raw);
    let stdin = String::from_utf8_lossy(&raw).into_owned();
    let term = std::env::var("TERM_SESSION_ID").ok();
    let nonempty = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let mut api = Lazy { cookie: nonempty("ITERM2_COOKIE"), key: nonempty("ITERM2_KEY"), client: None };
    run(term.as_deref(), &stdin, &mut api, &mut std::io::stderr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `map` with a `stored` closure that counts its calls and answers 2.
    fn mapped(event: Value) -> (Option<Update>, usize) {
        let mut calls = 0;
        let mut stored = || {
            calls += 1;
            2
        };
        let u = map(&event, &mut stored);
        (u, calls)
    }

    fn upd(status: &str, dot: &str, text: &str, detail: Option<&str>, tasks: Option<i64>) -> Update {
        Update { status: Some(status.into()), dot_color: Some(dot.into()), text_color: Some(text.into()), detail: detail.map(str::to_string), background_tasks: tasks }
    }

    #[test]
    fn events_follow_iterm2() {
        for name in ["UserPromptSubmit", "PreToolUse", "PostToolUse"] {
            assert_eq!(mapped(json!({"hook_event_name": name})), (Some(upd("working", WORKING, WORKING, Some(""), None)), 0), "{name}");
        }
        let ask = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "ls"}});
        assert_eq!(mapped(ask).0, Some(upd("waiting", WAITING, WAITING, Some("Allow Bash: ls?"), None)));
        assert_eq!(mapped(json!({"hook_event_name": "PermissionRequest", "tool_name": "ExitPlanMode"})).0.unwrap().detail.as_deref(), Some("Review proposed plan?"));
        let q = json!({"hook_event_name": "PermissionRequest", "tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "One  ?"}, {"question": "Two\n?"}]}});
        assert_eq!(mapped(q).0.unwrap().detail.as_deref(), Some("One ? Two ?"));
        // idle_prompt reads the stored count only when the payload has no task list.
        let idle = json!({"hook_event_name": "Notification", "notification_type": "idle_prompt"});
        assert_eq!(mapped(idle), (Some(upd("working", WORKING, WORKING, Some("2 background tasks running"), None)), 1));
        let done = json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "background_tasks": [], "last_assistant_message": "Done.\n\nAll good"});
        assert_eq!(mapped(done), (Some(upd("idle", IDLE_DOT, IDLE_TEXT, Some("Done. All good"), None)), 0));
        let other = json!({"hook_event_name": "Notification", "notification_type": "other", "message": "Need input"});
        assert_eq!(mapped(other).0, Some(upd("waiting", WAITING, WAITING, Some("Need input"), None)));
        let perm = json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "m"});
        assert_eq!(mapped(perm).0, Some(upd("waiting", WAITING, WAITING, None, None)));
        // Stop.
        let stop = json!({"hook_event_name": "Stop", "background_tasks": [{"status": "running"}, {"status": "completed"}]});
        assert_eq!(mapped(stop), (Some(upd("working", WORKING, WORKING, Some("1 background task running"), Some(1))), 0));
        let stop = json!({"hook_event_name": "Stop", "last_assistant_message": "ok"});
        assert_eq!(mapped(stop), (Some(upd("idle", IDLE_DOT, IDLE_TEXT, Some("ok"), None)), 0), "no field: no count, the stored count is not read");
        // SubagentStop.
        assert_eq!(mapped(json!({"hook_event_name": "SubagentStop"})), (None, 0));
        let sub = json!({"hook_event_name": "SubagentStop", "agent_id": "a1", "background_tasks": [{"id": "a1"}, {"id": "b"}]});
        assert_eq!(mapped(sub).0, Some(upd("working", WORKING, WORKING, Some("1 background task running"), Some(1))));
        let sub = json!({"hook_event_name": "SubagentStop", "agent_id": "a1", "background_tasks": [{"id": "a1"}]});
        assert_eq!(mapped(sub).0, Some(Update { background_tasks: Some(0), ..Default::default() }));
        // StopFailure reads the stored count without a list.
        assert_eq!(mapped(json!({"hook_event_name": "StopFailure"})), (Some(upd("working", WORKING, WORKING, Some("2 background tasks running"), None)), 1));
        let fail = json!({"hook_event_name": "StopFailure", "background_tasks": []});
        assert_eq!(mapped(fail), (Some(upd("idle", IDLE_DOT, IDLE_TEXT, Some(""), Some(0))), 0));
        for name in ["SessionStart", "SessionEnd"] {
            assert_eq!(mapped(json!({"hook_event_name": name})).0, Some(upd("idle", IDLE_DOT, IDLE_TEXT, Some(""), Some(0))), "{name}");
        }
        for bad in [json!({}), json!({"hook_event_name": 4}), json!({"hook_event_name": "Bogus"}), json!([]), json!("")] {
            assert_eq!(mapped(bad), (None, 0));
        }
        assert_eq!(session_id(Some("w0t0p0:42")).as_deref(), Some("42"));
        assert_eq!(session_id(Some("42")), None);
        assert_eq!(session_id(None), None);
    }

    #[test]
    fn summaries_follow_iterm2() {
        assert_eq!(tool_summary("Bash", &json!({"command": "ls   -la\n x"})), "Bash: ls -la x");
        assert_eq!(tool_summary("Read", &json!({"file_path": "C:\\a\\b.txt"})), "Read: C:\\a\\b.txt");
        let posix = format!("/{}/dir/file.rs", "a".repeat(60));
        assert_eq!(short_path(&posix, PATH_LIMIT), "\u{2026}/dir/file.rs");
        let win = format!("C:\\{}\\dir\\file.rs", "a".repeat(60));
        assert_eq!(short_path(&win, PATH_LIMIT), "\u{2026}\\dir\\file.rs");
        assert_eq!(tool_summary("Grep", &json!({"pattern": "fn x", "glob": "*.rs"})), "Grep \u{201C}fn x\u{201D} in *.rs");
        assert_eq!(tool_summary("Grep", &json!({"pattern": "fn x", "path": "src"})), "Grep \u{201C}fn x\u{201D} in src");
        assert_eq!(tool_summary("Glob", &json!({"pattern": "**/*.rs"})), "Glob: **/*.rs");
        assert_eq!(tool_summary("Task", &json!({"description": "scan"})), "Task: scan");
        assert_eq!(tool_summary("WebFetch", &json!({"url": "https://docs.rs/serde/latest"})), "WebFetch: docs.rs/serde");
        assert_eq!(tool_summary("WebSearch", &json!({"query": "q"})), "WebSearch: q");
        assert_eq!(tool_summary("mcp__github__create_issue", &json!({})), "github/create_issue");
        assert_eq!(tool_summary("AskUserQuestion", &json!({})), "Ask User Question");
        assert_eq!(humanize("URLFetch"), "URL Fetch");
        let long = "a ".repeat(200);
        assert_eq!(condense(&long, DETAIL_LIMIT), format!("{}\u{2026}", &long[..179]));
        assert_eq!(condense(&long, DETAIL_LIMIT).chars().count(), 180);
        let exact = "a".repeat(180);
        assert_eq!(condense(&exact, DETAIL_LIMIT), exact);
        assert_eq!(background_detail(1), "1 background task running");
        assert_eq!(background_detail(3), "3 background tasks running");
        let count = |v: Value| background_count(&json!({"background_tasks": v}), None);
        assert_eq!(count(json!(["x", {"status": "RUNNING"}, {"status": "Completed"}, {"status": "failed"}, {"status": "cancelled"}, {"status": "canceled"}, {"status": "killed"}, {"status": "stopped"}, {"status": "done"}, {}])), Some(3));
        assert_eq!(count(json!({"a": 1, "b": 2})), Some(2));
        assert_eq!(count(json!(5)), Some(5));
        assert_eq!(count(json!("x")), None);
        assert_eq!(background_count(&json!({}), None), None);
    }

    /// Anything the hook displays is bounded, whatever the payload holds.
    #[test]
    fn what_the_hook_sends_is_capped() {
        let huge = "x".repeat(100_000);
        let ask = json!({"hook_event_name": "PermissionRequest", "tool_name": "Glob", "tool_input": {"pattern": huge}});
        let detail = mapped(ask).0.unwrap().detail.unwrap();
        assert!(detail.chars().count() <= DETAIL_LIMIT, "{}", detail.chars().count());
        let msg = json!({"hook_event_name": "Notification", "notification_type": "x", "message": huge});
        assert!(mapped(msg).0.unwrap().detail.unwrap().chars().count() <= DETAIL_LIMIT);
        let name = json!({"hook_event_name": "PermissionRequest", "tool_name": huge});
        assert!(mapped(name).0.unwrap().detail.unwrap().chars().count() <= DETAIL_LIMIT);
    }

    /// A status whose cookie is another pane's changes nothing (ruling P14).
    #[test]
    fn a_wrong_cookie_changes_nothing() {
        assert!(may_report(Some("aaaa"), Some("aaaa")));
        assert!(!may_report(Some("aaaa"), Some("bbbb")), "another pane's cookie");
        assert!(!may_report(Some("aaaa"), None), "no cookie proves nothing");
        assert!(!may_report(None, Some("aaaa")), "a pane with no cookie");
        assert!(!may_report(None, None));
        assert!(!may_report(Some(""), Some("")), "an empty cookie is no cookie");
    }

    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, String)>,
        fail: Option<String>,
    }

    impl Api for Fake {
        fn invoke(&mut self, session: &str, invocation: &str) -> Result<Value, String> {
            self.calls.push((session.to_string(), invocation.to_string()));
            match &self.fail {
                Some(e) => Err(e.clone()),
                None if invocation == GET_COUNT => Ok(json!(2)),
                None => Ok(Value::Null),
            }
        }
    }

    fn ran(term: Option<&str>, stdin: &str, api: &mut Fake) -> (i32, String) {
        let mut err = Vec::new();
        let code = run(term, stdin, api, &mut err);
        (code, String::from_utf8(err).unwrap())
    }

    #[test]
    fn invocation_follows_it2() {
        let u = upd("working", "#ff9500", "#ff9500", Some(""), None);
        assert_eq!(u.invocation(), r##"iterm2.set_status(status: "working", text_color: "#ff9500", dot_color: "#ff9500", detail: "")"##);
        assert_eq!(Update { background_tasks: Some(0), ..Default::default() }.invocation(), "iterm2.set_status(background_tasks: 0)");
        let q = Update { detail: Some("say \"hi\"\nthere \\(1+1)".into()), background_tasks: Some(3), ..Default::default() };
        assert_eq!(q.invocation(), r#"iterm2.set_status(detail: "say \"hi\"\nthere \\(1+1)", background_tasks: 3)"#, "quoted as JSON: no escape can open an interpolation");
        // A notification idle_prompt without tasks: GET_COUNT, then the status.
        let mut api = Fake::default();
        let (code, err) = ran(Some("w0t0p0:42"), r#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#, &mut api);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(api.calls.len(), 2);
        assert_eq!(api.calls[0], ("42".to_string(), GET_COUNT.to_string()));
        assert_eq!(api.calls[1].0, "42");
        assert!(api.calls[1].1.starts_with("iterm2.set_status(status: \"working\""));
        // A failing API: one line on stderr, exit 0.
        let mut bad = Fake { fail: Some("no route".into()), ..Default::default() };
        let (code, err) = ran(Some("w0t0p0:42"), r#"{"hook_event_name":"Stop"}"#, &mut bad);
        assert_eq!((code, err.as_str()), (0, "cc-status: could not update Termurion for Stop: no route\n"));
        // A dropped status (another session's cookie) is silent.
        let mut dropped = Fake { fail: Some(DROPPED.into()), ..Default::default() };
        assert_eq!(ran(Some("w0t0p0:42"), r#"{"hook_event_name":"Stop"}"#, &mut dropped), (0, String::new()));
        // No TERM_SESSION_ID, no call.
        let mut none = Fake::default();
        assert_eq!(ran(None, r#"{"hook_event_name":"Stop"}"#, &mut none), (0, String::new()));
        assert_eq!(ran(Some("42"), r#"{"hook_event_name":"Stop"}"#, &mut none), (0, String::new()));
        for junk in ["", "[]", "{", "\u{0}\u{1}", r#"{"hook_event_name":3}"#] {
            assert_eq!(ran(Some("w0t0p0:42"), junk, &mut none), (0, String::new()), "{junk:?}");
        }
        assert!(none.calls.is_empty());
        let headers = crate::api_client::request_headers("localhost", Some("c"), None, "cc-status");
        let has = |k: &str, v: &str| headers.iter().any(|(hk, hv)| hk.eq_ignore_ascii_case(k) && hv == v);
        assert!(has("Sec-WebSocket-Protocol", "api.iterm2.com"));
        assert!(has("Origin", "http://localhost"));
        assert!(has("x-iterm2-library-version", "cc-status 1.0"));
        assert!(has("x-iterm2-advisory-name", "cc-status"));
        assert!(has("x-iterm2-disable-auth-ui", "true"));
        assert!(has("x-iterm2-cookie", "c"));
        assert!(!headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-iterm2-key")));
    }
}
