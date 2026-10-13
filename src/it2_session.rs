//! `it2 session add-clipping` and `it2 session archive-clippings` (milestone 13y1, Task 7, B7): the two
//! verbs of iTerm2's Swift `it2` that the code review agent uses to hand its findings back
//! (`SessionCommand.swift:807-906`). They call the app's `iterm2.add_clipping(...)` and
//! `iterm2.archive_clippings()` over the authenticated Python API with the calling session's cookie, so a
//! cookie can only touch its own session's clippings (the hook guard in `api_invoke_function`); no new
//! listener and no new authentication. Every field is bounded here and again by `Clippings::add`.

use crate::clippings::{MAX_DETAIL, MAX_TITLE, MAX_TYPE};

/// What `it2 session ...` asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Add { session: Option<String>, kind: String, title: String, detail: String },
    Archive { session: Option<String> },
}

const USAGE: &str = "Usage: it2 session add-clipping [--session=ID] TYPE TITLE DETAIL\n       it2 session archive-clippings [--session=ID]";

/// A session id as given: after the first colon of a `TERM_SESSION_ID` (`w0t0p0:<id>`), else as it is.
fn session_of(v: &str) -> String {
    crate::cc_status::session_id(Some(v)).unwrap_or_else(|| v.to_string())
}

/// Parses the arguments after `it2`. `--session=ID`, `--session ID` and `-s ID` name the session; `--`
/// ends the options.
pub fn parse(args: &[String], _term_session_id: Option<&str>) -> Result<Command, String> {
    let usage = || USAGE.to_string();
    if args.first().map(String::as_str) != Some("session") {
        return Err(usage());
    }
    let verb = args.get(1).map(String::as_str).ok_or_else(usage)?;
    let mut session = None;
    let mut positional: Vec<String> = Vec::new();
    let mut rest = args[2..].iter();
    let mut options = true;
    while let Some(a) = rest.next() {
        if options && a == "--" {
            options = false;
        } else if options && (a == "--session" || a == "-s") {
            session = Some(session_of(rest.next().ok_or_else(usage)?));
        } else if let Some(v) = options.then(|| a.strip_prefix("--session=")).flatten() {
            session = Some(session_of(v));
        } else if options && a.starts_with('-') && a.len() > 1 {
            return Err(format!("it2: unknown option {}\n{USAGE}", crate::inert_files::shown_name(a)));
        } else {
            positional.push(a.clone());
        }
    }
    match (verb, positional.as_slice()) {
        ("add-clipping", [kind, title, detail]) => {
            if kind.chars().count() > MAX_TYPE {
                return Err(format!("it2: the type is longer than {MAX_TYPE} characters"));
            }
            if title.chars().count() > MAX_TITLE {
                return Err(format!("it2: the title is longer than {MAX_TITLE} characters"));
            }
            if detail.len() > MAX_DETAIL {
                return Err(format!("it2: the detail is longer than {} KiB", MAX_DETAIL / 1024));
            }
            Ok(Command::Add { session, kind: kind.clone(), title: title.clone(), detail: detail.clone() })
        }
        ("archive-clippings", []) => Ok(Command::Archive { session }),
        _ => Err(usage()),
    }
}

/// The invocation the app evaluates. Strings are JSON-quoted (as `jsonString` does), so a backslash is
/// doubled and no text can open an interpolation.
pub fn invocation(c: &Command) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    match c {
        Command::Add { kind, title, detail, .. } => format!("iterm2.add_clipping(type: {}, title: {}, detail: {})", q(kind), q(title), q(detail)),
        Command::Archive { .. } => "iterm2.archive_clippings()".to_string(),
    }
}

/// Runs one command: 0, or 1 with one line on `err` (never a cookie).
pub fn run(c: &Command, term_session_id: Option<&str>, api: &mut dyn crate::cc_status::Api, err: &mut dyn std::io::Write) -> i32 {
    let session = match c {
        Command::Add { session, .. } | Command::Archive { session } => session.clone(),
    };
    let Some(session) = session.or_else(|| crate::cc_status::session_id(term_session_id)) else {
        let _ = writeln!(err, "it2: no session: pass --session or run it inside a Termurion session");
        return 1;
    };
    match api.invoke(&session, &invocation(c)) {
        Ok(_) => 0,
        Err(reason) => {
            let _ = writeln!(err, "it2: {}", crate::inert_files::shown_name(&reason));
            1
        }
    }
}

/// The utility: `ITERM2_COOKIE` and `ITERM2_KEY` from the environment authenticate it as the session that
/// started it; one connection of 5 seconds.
pub fn it2(args: &[String], io: &mut crate::util_cli::Io) -> i32 {
    let cmd = match parse(args, None) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(io.err, "{e}");
            return 2;
        }
    };
    #[cfg(windows)]
    {
        let nonempty = |n: &str| (io.env)(n).filter(|v| !v.is_empty());
        let (cookie, key, term) = (nonempty("ITERM2_COOKIE"), nonempty("ITERM2_KEY"), nonempty("TERM_SESSION_ID"));
        let mut api = Lazy { cookie, key, client: None };
        run(&cmd, term.as_deref(), &mut api, io.err)
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
        let _ = writeln!(io.err, "it2: Termurion's API is available on Windows only");
        1
    }
}

/// A connection made only when a request is sent.
#[cfg(windows)]
struct Lazy {
    cookie: Option<String>,
    key: Option<String>,
    client: Option<Result<crate::api_client::Client, String>>,
}

#[cfg(windows)]
impl crate::cc_status::Api for Lazy {
    fn invoke(&mut self, session: &str, invocation: &str) -> Result<serde_json::Value, String> {
        let (cookie, key) = (self.cookie.as_deref(), self.key.as_deref());
        let c = self.client.get_or_insert_with(|| crate::api_client::Client::connect(cookie, key, "it2", std::time::Duration::from_secs(5)));
        match c {
            Ok(c) => c.invoke(session, invocation),
            Err(e) => Err(e.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `it2 session add-clipping` and `archive-clippings` (`SessionCommand.swift:807-906`).
    #[test]
    fn it2_session_follows_iterm2() {
        let add = parse(&args(&["session", "add-clipping", "--session=w0t0p0:ABC-1", "Code Review Comment", "A title", "The detail"]), None).unwrap();
        assert_eq!(add, Command::Add { session: Some("ABC-1".into()), kind: "Code Review Comment".into(), title: "A title".into(), detail: "The detail".into() });
        // The short form, the separate form, and no session (the caller's own).
        let a = |v: &[&str]| parse(&args(v), None).unwrap();
        assert_eq!(a(&["session", "add-clipping", "-s", "X", "t", "u", "d"]), Command::Add { session: Some("X".into()), kind: "t".into(), title: "u".into(), detail: "d".into() });
        assert_eq!(a(&["session", "add-clipping", "--session", "w1t2p3:Y", "t", "u", "d"]), Command::Add { session: Some("Y".into()), kind: "t".into(), title: "u".into(), detail: "d".into() });
        assert_eq!(a(&["session", "add-clipping", "t", "u", "d"]), Command::Add { session: None, kind: "t".into(), title: "u".into(), detail: "d".into() });
        assert_eq!(a(&["session", "archive-clippings", "--session=w0t0p0:Z"]), Command::Archive { session: Some("Z".into()) });
        assert_eq!(a(&["session", "archive-clippings"]), Command::Archive { session: None });
        for bad in [&["session"][..], &["session", "add-clipping", "t", "u"], &["session", "add-clipping", "t", "u", "d", "e"], &["session", "nope"], &["list"], &["session", "archive-clippings", "extra"], &["session", "add-clipping", "--wat", "t", "u", "d"]] {
            assert!(parse(&args(bad), None).is_err(), "{bad:?}");
        }
        // The invocations: strings are JSON-quoted, so no text can open an interpolation.
        assert_eq!(invocation(&Command::Archive { session: None }), "iterm2.archive_clippings()");
        let c = Command::Add { session: None, kind: "T".into(), title: "a \"q\" \\(x)".into(), detail: "l1\nl2 \\".into() };
        assert_eq!(invocation(&c), "iterm2.add_clipping(type: \"T\", title: \"a \\\"q\\\" \\\\(x)\", detail: \"l1\\nl2 \\\\\")");
    }

    /// B7/P16: each field is bounded at the tool as at the app.
    #[test]
    fn it2_session_bounds_its_arguments() {
        let long = |n: usize| "x".repeat(n);
        let a = |t: &str, ti: &str, d: &str| parse(&args(&["session", "add-clipping", t, ti, d]), None);
        assert!(a(&long(64), &long(200), &long(64 * 1024)).is_ok());
        assert!(a(&long(65), "t", "d").is_err());
        assert!(a("t", &long(201), "d").is_err());
        assert!(a("t", "t", &long(64 * 1024 + 1)).is_err());
    }

    struct Fake(Vec<(String, String)>, Result<serde_json::Value, String>);

    impl crate::cc_status::Api for Fake {
        fn invoke(&mut self, session: &str, invocation: &str) -> Result<serde_json::Value, String> {
            self.0.push((session.to_string(), invocation.to_string()));
            self.1.clone()
        }
    }

    /// The session is `--session`'s id (the part after the first colon), else the caller's `TERM_SESSION_ID`;
    /// the answer is the exit code and one line on failure.
    #[test]
    fn it2_session_runs_over_the_api() {
        let mut api = Fake(Vec::new(), Ok(serde_json::Value::Null));
        let mut err = Vec::new();
        let cmd = parse(&args(&["session", "add-clipping", "--session=w0t0p0:ID9", "T", "t", "d"]), Some("w9t9p9:OTHER")).unwrap();
        assert_eq!(run(&cmd, Some("w9t9p9:OTHER"), &mut api, &mut err), 0);
        assert_eq!(api.0[0].0, "ID9", "--session wins");
        let cmd = parse(&args(&["session", "archive-clippings"]), None).unwrap();
        assert_eq!(run(&cmd, Some("w9t9p9:OTHER"), &mut api, &mut err), 0);
        assert_eq!(api.0[1], ("OTHER".to_string(), "iterm2.archive_clippings()".to_string()));
        assert_eq!(run(&cmd, None, &mut api, &mut err), 1, "no session at all");
        assert!(String::from_utf8(err.clone()).unwrap().contains("session"));
        let mut failing = Fake(Vec::new(), Err("The cookie does not belong to that session.".into()));
        err.clear();
        assert_eq!(run(&cmd, Some("w0t0p0:A"), &mut failing, &mut err), 1);
        assert!(String::from_utf8(err).unwrap().starts_with("it2: "));
    }

    /// The connection is a hook connection (the app's cookie guard applies): its library version is the
    /// one `api_ui` keys the guard on, and it asks for no authorization dialog.
    #[test]
    fn it2_connects_as_a_cookie_bound_hook() {
        let h = crate::api_client::request_headers("localhost", Some("c"), None, "it2");
        let get = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("x-iterm2-library-version"), Some(crate::api_client::LIBRARY_VERSION));
        assert_eq!(get("x-iterm2-disable-auth-ui"), Some("true"));
        assert_eq!(get("x-iterm2-advisory-name"), Some("it2"));
        assert!(include_str!("app/api_ui.rs").contains("p.upgrade.library_version == crate::api_client::LIBRARY_VERSION"));
    }

    #[test]
    fn it2_is_a_utility() {
        assert!(crate::util_cli::NAMES.contains(&"it2"));
        assert_eq!(crate::util_cli::name_of("C:\\t\\utilities\\IT2.EXE"), Some("it2"));
        assert!(include_str!("util_cli.rs").contains("\"it2\" => crate::it2_session::it2(args, io)"));
    }
}
