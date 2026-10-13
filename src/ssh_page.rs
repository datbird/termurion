//! The ssh page (milestone 13x8 Task 9, pure): `iterm2-ssh://<host><path>` names a file on a host
//! the person already has an ssh-integrated session with (`SSHM:42-125`, `CR:51-67`, `CD:3118-3124`).
//! The URL is untrusted data: it only ever selects among sessions the person opened and carries a
//! path that the transfer engine sends as a typed argument, never as part of a shell string.

use crate::layout::PaneId;
use crate::ssh_args::Identity;

/// The most a page holds of one remote file (Task 3's limit, review I5): WebView2 answers a
/// request with one body, so the file is buffered whole.
pub const PAGE_CAP: u64 = crate::file_serve::MAX_FILE;

/// Termurion's text for a file over the cap.
pub const TOO_LARGE: &str = "The file is larger than 64 MB and cannot be shown.";

/// Termurion's text for a folder (the single-stream download would zip it).
pub const NOT_A_FILE: &str = "Only a file can be shown.";

/// Termurion's text for a page asking for more files than may be fetched at once.
pub const TOO_MANY: &str = "Too many files are being fetched for this page.";

/// Remote files one browser view may have in flight, and all views together (a page of one hundred
/// images does not start one hundred transfers).
pub const MAX_PER_VIEW: usize = 8;
pub const MAX_TOTAL: usize = 16;

/// A remote file being fetched for a browser view.
#[derive(Debug, PartialEq)]
pub struct PageJob {
    /// The pane whose conductor fetches.
    pub pane: PaneId,
    /// The browser pane that asked and the deferred request's token in its current view.
    pub view_pane: PaneId,
    pub token: u64,
    pub url: String,
    /// The remote path, decoded (it names the type; it is never sent anywhere but as a typed path).
    pub path: String,
    /// Whether this is the page itself (not a subresource of it).
    pub document: bool,
    pub mime: &'static str,
    pub body: Vec<u8>,
    /// The request was answered or abandoned: the rest of the job's actions are dropped, and its
    /// `Done` removes it.
    pub answered: bool,
}

/// Whether `next` more bytes after `buffered` pass the cap.
pub fn over_cap(buffered: u64, next: u64) -> bool {
    buffered.saturating_add(next) > PAGE_CAP
}

/// Characters of a path that `URLComponents` leaves as they are (`urlPathAllowed`): everything else,
/// including `?`, `#` and `%`, is percent-escaped.
fn path_keeps(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b)
}

/// `iterm2-ssh://<host><path>` (`CD:3118-3124`): no user, no port.
pub fn view_in_browser_url(host: &str, path: &str) -> String {
    let mut out = format!("iterm2-ssh://{host}");
    for &b in path.as_bytes() {
        if path_keeps(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// (host, user, port, decoded path) of an `iterm2-ssh:` URL, or None for another scheme, an empty or
/// odd host, a bad port, or a path the one decoder refuses (a `..` segment, a backslash, a control
/// character, bytes that are not UTF-8).
pub fn parse(url: &str) -> Option<(String, Option<String>, Option<u16>, String)> {
    let (scheme, authority, path) = crate::browser_pages::local_parts(url)?;
    if scheme != crate::browser_pages::SSH_SCHEME {
        return None;
    }
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()).filter(|u| !u.is_empty()), h),
        None => (None, authority.as_str()),
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        (h, after.strip_prefix(':').map(str::to_string))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p.to_string())),
            None => (hostport, None),
        }
    };
    let host_ok = !host.is_empty() && host.len() <= 255 && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._:".contains(&b));
    if !host_ok || user.as_deref().is_some_and(|u| u.len() > 255 || u.chars().any(|c| c.is_control() || c.is_whitespace())) {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().ok()?),
    };
    Some((host.to_string(), user, port, path))
}

/// Whether `id` is the session an URL names (`CR:51-67`): the URL's host is the session's hostname or
/// host; a URL user must equal the session's user when it has one; a URL port other than 0 must equal
/// the session's, and a session port 0 never matches port 22.
pub fn identity_matches(id: &Identity, host: &str, user: Option<&str>, port: Option<u16>) -> bool {
    if !id.hostname.eq_ignore_ascii_case(host) && !id.host.eq_ignore_ascii_case(host) {
        return false;
    }
    if let (Some(url_user), Some(session_user)) = (user, id.username.as_deref()) {
        if url_user != session_user {
            return false;
        }
    }
    if let Some(p) = port.filter(|p| *p != 0) {
        if id.port != p || (id.port == 0 && p == 22) {
            return false;
        }
    }
    true
}

/// The page for a host nobody is connected to (`SSHM:49-62`, `SSHP`): the host as text, cleaned and
/// escaped, "(nil)" when the URL had none.
pub fn no_conductor_page(host: Option<&str>) -> String {
    let shown = crate::inert_files::display_text(host.unwrap_or("(nil)"), 255, false);
    let escaped = crate::view_source::escape_html(&shown).replace('{', "&#123;");
    crate::browser_pages::page(crate::browser_js::resource("ssh-page-no-conductor.html").unwrap_or_default(), &[("HOST", &escaped)])
}

/// What the transfer engine's next action means for the page jobs.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Not a page job: the caller carries on with the action as it would.
    NotMine,
    /// A page job's action, taken; nothing more to do.
    Taken,
    /// The request must be answered with this text as an error page (and, when `cancel`, the transfer
    /// ended). The job stays in the table, answered, until the engine's `Done` removes it.
    Error { view_pane: PaneId, token: u64, url: String, text: String, cancel: bool },
    /// The file arrived whole: answer with this job (already out of the table).
    Body(PageJob),
}

fn refuse(job: &mut PageJob, text: &str, cancel: bool) -> Step {
    job.answered = true;
    job.body = Vec::new();
    Step::Error { view_pane: job.view_pane, token: job.token, url: job.url.clone(), text: text.to_string(), cancel }
}

/// Whether one more job may start for `view_pane` (a page of many files starts a few at a time).
pub fn admit(jobs: &std::collections::HashMap<u64, PageJob>, view_pane: PaneId) -> Result<(), &'static str> {
    let live = |j: &&PageJob| !j.answered;
    if jobs.values().filter(live).count() >= MAX_TOTAL || jobs.values().filter(live).filter(|j| j.view_pane == view_pane).count() >= MAX_PER_VIEW {
        return Err(TOO_MANY);
    }
    Ok(())
}

/// The first stat's size: a file known to pass the cap ends the job at once.
pub fn on_size(jobs: &mut std::collections::HashMap<u64, PageJob>, job: u64, size: Option<u64>) -> Step {
    let Some(j) = jobs.get_mut(&job) else { return Step::NotMine };
    if j.answered {
        return Step::Taken;
    }
    match size {
        Some(s) if s > PAGE_CAP => refuse(j, TOO_LARGE, true),
        _ => Step::Taken,
    }
}

/// Bytes of the file: kept while they fit under the cap, else the job ends and holds nothing.
pub fn on_write(jobs: &mut std::collections::HashMap<u64, PageJob>, job: u64, bytes: &[u8]) -> Step {
    let Some(j) = jobs.get_mut(&job) else { return Step::NotMine };
    if j.answered {
        return Step::Taken;
    }
    if over_cap(j.body.len() as u64, bytes.len() as u64) {
        return refuse(j, TOO_LARGE, true);
    }
    j.body.extend_from_slice(bytes);
    Step::Taken
}

/// A folder's zip is about to be unpacked: not a file.
pub fn on_unzip(jobs: &mut std::collections::HashMap<u64, PageJob>, job: u64) -> Step {
    let Some(j) = jobs.get_mut(&job) else { return Step::NotMine };
    if j.answered {
        return Step::Taken;
    }
    refuse(j, NOT_A_FILE, true)
}

/// The engine ended the job: its file, or the reason. The job leaves the table.
pub fn on_done(jobs: &mut std::collections::HashMap<u64, PageJob>, job: u64, result: &Result<crate::ssh_transfer::Outcome, String>) -> Step {
    let Some(mut j) = jobs.remove(&job) else { return Step::NotMine };
    if j.answered {
        return Step::Taken;
    }
    match result {
        Ok(crate::ssh_transfer::Outcome::File) => Step::Body(j),
        Ok(_) => refuse(&mut j, NOT_A_FILE, false),
        Err(e) => refuse(&mut j, e, false),
    }
}

/// The view or the session is gone: every job of the view stops answering. Returns the ids to cancel.
pub fn abandon_view(jobs: &mut std::collections::HashMap<u64, PageJob>, view_pane: PaneId) -> Vec<u64> {
    let mut out = Vec::new();
    for (id, j) in jobs.iter_mut().filter(|(_, j)| j.view_pane == view_pane && !j.answered) {
        j.answered = true;
        j.body = Vec::new();
        out.push(*id);
    }
    out.sort_unstable();
    out
}

fn respond(status: u16, mime: &str, body: Vec<u8>, headers: Vec<(String, String)>) -> crate::webhost::Answer {
    crate::webhost::Answer::Respond { status, mime: mime.to_string(), body, headers }
}

/// The page that waits where a remote file would be: Termurion's own text, 200 (as iTerm2's failed
/// scheme task shows a page, `BM:1382`), with no script (`browser_pages::scriptless_page`) under the
/// one no-script policy of the scheme (R13x8-NS).
pub fn page_answer(html: String) -> crate::webhost::Answer {
    respond(200, "text/html; charset=utf-8", crate::browser_pages::scriptless_page(&html).into_bytes(), crate::browser_pages::file_headers())
}

/// An error as a page (the generic error page with the error's text).
pub fn error_answer(text: &str, url: &str) -> crate::webhost::Answer {
    page_answer(crate::browser_pages::described_error_page(text, Some(url)))
}

/// The remote file as the person asked for it, through the one serving path of file pages
/// (`file_serve`): the no-script policy on everything.
pub fn body_answer(job: PageJob) -> crate::webhost::Answer {
    crate::file_serve::file_answer(std::path::Path::new(&job.path), job.document, job.mime, job.body)
}

/// Whether `url` is a well-formed view URL Termurion itself made (the one gate the first load of a View
/// tab passes): `iterm2-ssh://<host><path>` with no user and no port.
pub fn view_url_ok(url: &str) -> bool {
    matches!(parse(url), Some((_, None, None, _))) && url.len() <= 8192
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Identity {
        Identity { host: "box".into(), hostname: "box.lan".into(), username: Some("u".into()), port: 22 }
    }

    #[test]
    fn ssh_url_matching_follows_iterm2() {
        assert_eq!(view_in_browser_url("box", "/home/u/a b.txt"), "iterm2-ssh://box/home/u/a%20b.txt");
        assert_eq!(parse("iterm2-ssh://box/home/u/a%20b.txt"), Some(("box".into(), None, None, "/home/u/a b.txt".into())));
        let i = id();
        assert!(identity_matches(&i, "box", None, None));
        assert!(identity_matches(&i, "box.lan", None, None), "hostname or host");
        assert!(!identity_matches(&i, "other", None, None));
        assert!(identity_matches(&i, "box", Some("u"), None));
        assert!(!identity_matches(&i, "box", Some("v"), None));
        let nouser = Identity { username: None, ..id() };
        assert!(identity_matches(&nouser, "box", Some("anyone"), None), "an identity with no user accepts any");
        assert!(identity_matches(&i, "box", None, Some(22)));
        assert!(!identity_matches(&i, "box", None, Some(2222)));
        assert!(identity_matches(&i, "box", None, Some(0)), "port 0 is absent");
        let zero = Identity { port: 0, ..id() };
        assert!(!identity_matches(&zero, "box", None, Some(22)), "iTerm2's odd rule is kept");
        assert!(identity_matches(&zero, "box", None, Some(0)));
        let page = no_conductor_page(Some("box"));
        assert!(page.contains("No connection to <strong>box</strong> using ssh integration could be found."), "{page}");
        assert!(page.contains("Problem viewing file"));
        assert!(no_conductor_page(None).contains("<strong>(nil)</strong>"));
        // A hostile host is text: no markup, no template key.
        let evil = no_conductor_page(Some("<img src=x onerror=1>{{HOST}}\u{202e}"));
        assert!(!evil.contains("<img") && !evil.contains('\u{202e}'), "{evil}");
        // The three characters a path cannot carry raw round-trip.
        let url = view_in_browser_url("box", "/h/a#b?c%d.txt");
        assert_eq!(url, "iterm2-ssh://box/h/a%23b%3Fc%25d.txt");
        assert_eq!(parse(&url), Some(("box".into(), None, None, "/h/a#b?c%d.txt".into())));
        // The parts of an authority.
        assert_eq!(parse("iterm2-ssh://me@box:2222/x"), Some(("box".into(), Some("me".into()), Some(2222), "/x".into())));
        assert_eq!(parse("iterm2-ssh://[::1]:22/x"), Some(("::1".into(), None, Some(22), "/x".into())));
        for bad in ["iterm2-ssh:///x", "iterm2-ssh://box:99999/x", "iterm2-ssh://bo x/x", "iterm2-ssh://box/a/../b", "iterm2-ssh://box/a%00b", "iterm2-file://box/x", "iterm2-ssh://box%2Fx/y", "https://box/x"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert!(view_url_ok("iterm2-ssh://box/a.txt") && !view_url_ok("iterm2-ssh://u@box/a.txt") && !view_url_ok("iterm2-ssh://box:22/a.txt"));
    }

    #[test]
    fn a_path_is_never_a_shell_string() {
        // The path reaches the engine as one typed argument; nothing here builds a command.
        let src = include_str!("ssh_page.rs").split("#[cfg(test)]").next().unwrap();
        for banned in ["Command::new", "format!(\"ssh", "sh -c", "scp "] {
            assert!(!src.contains(banned), "{banned}");
        }
        let (.., path) = parse(&view_in_browser_url("box", "/tmp/$(rm -rf ~)`id`;&|.txt")).unwrap();
        assert_eq!(path, "/tmp/$(rm -rf ~)`id`;&|.txt");
    }

    fn job(token: u64) -> PageJob {
        PageJob { pane: 1, view_pane: 2, token, url: "iterm2-ssh://box/a.txt".into(), path: "/a.txt".into(), document: true, mime: "text/plain", body: Vec::new(), answered: false }
    }

    fn headers_of(a: &crate::webhost::Answer) -> Vec<(String, String)> {
        match a {
            crate::webhost::Answer::Respond { headers, .. } => headers.clone(),
            _ => panic!("not a response"),
        }
    }

    #[test]
    fn page_jobs_follow_the_cap() {
        use std::collections::HashMap;
        let mut jobs: HashMap<u64, PageJob> = HashMap::new();
        // A stat that already passes the cap ends the job at once; it holds nothing and its Done removes it.
        jobs.insert(7, job(70));
        let s = on_size(&mut jobs, 7, Some(100 * 1024 * 1024));
        assert!(matches!(&s, Step::Error { token: 70, text, cancel: true, .. } if text == TOO_LARGE), "{s:?}");
        assert_eq!(on_write(&mut jobs, 7, b"late"), Step::Taken, "bytes after the end are dropped");
        assert!(jobs[&7].body.is_empty() && jobs[&7].answered);
        assert_eq!(on_done(&mut jobs, 7, &Err("cancelled".into())), Step::Taken, "no second answer");
        assert!(jobs.is_empty());
        // Unknown size, 1 MB writes: the 65th passes the cap; at most PAGE_CAP is ever held.
        jobs.insert(8, job(80));
        let chunk = vec![1u8; 1024 * 1024];
        for n in 1..=64 {
            assert_eq!(on_write(&mut jobs, 8, &chunk), Step::Taken, "write {n}");
            assert!(jobs[&8].body.len() as u64 <= PAGE_CAP);
        }
        assert_eq!(jobs[&8].body.len() as u64, PAGE_CAP);
        assert!(matches!(on_write(&mut jobs, 8, &chunk), Step::Error { token: 80, cancel: true, .. }), "the 65th");
        assert!(jobs[&8].body.is_empty());
        assert_eq!(on_done(&mut jobs, 8, &Err("cancelled".into())), Step::Taken);
        assert!(jobs.is_empty(), "the job is dropped");
        // A size at the cap is fine; an unknown size is fine.
        jobs.insert(9, job(90));
        assert_eq!(on_size(&mut jobs, 9, Some(PAGE_CAP)), Step::Taken);
        assert_eq!(on_size(&mut jobs, 9, None), Step::Taken);
        // Not a page job: the caller carries on.
        assert_eq!(on_size(&mut jobs, 1234, Some(1)), Step::NotMine);
        assert_eq!(on_write(&mut jobs, 1234, b"x"), Step::NotMine);
        assert_eq!(on_done(&mut jobs, 1234, &Ok(crate::ssh_transfer::Outcome::File)), Step::NotMine);
    }

    #[test]
    fn page_jobs_end_as_iterm2_does() {
        use crate::ssh_transfer::Outcome;
        use std::collections::HashMap;
        let mut jobs: HashMap<u64, PageJob> = HashMap::new();
        jobs.insert(1, job(10));
        on_write(&mut jobs, 1, b"hello ");
        on_write(&mut jobs, 1, b"world");
        let Step::Body(j) = on_done(&mut jobs, 1, &Ok(Outcome::File)) else { panic!("a file") };
        assert!(jobs.is_empty());
        assert_eq!((j.token, j.body.as_slice()), (10, b"hello world".as_slice()));
        // The file is served as a downloaded file: sandbox, no script, nosniff, its table type.
        let a = body_answer(PageJob { mime: "image/png", document: false, ..j });
        let crate::webhost::Answer::Respond { status, mime, body, headers } = a else { panic!() };
        assert_eq!((status, mime.as_str(), body.as_slice()), (200, "image/png", b"hello world".as_slice()));
        let csp = &headers.iter().find(|(k, _)| k == "Content-Security-Policy").unwrap().1;
        assert!(csp.starts_with("sandbox;") && csp.contains("script-src 'none'"), "{csp}");
        assert!(headers.iter().any(|(k, v)| k == "X-Content-Type-Options" && v == "nosniff"));
        // The page itself as text is escaped, not run (a remote .txt, an unknown type).
        let doc = PageJob { document: true, path: "/x.bin".into(), mime: "application/octet-stream", body: b"<script>alert(1)</script>".to_vec(), ..job(1) };
        let crate::webhost::Answer::Respond { mime, body, .. } = body_answer(doc) else { panic!() };
        let text = String::from_utf8(body).unwrap();
        assert_eq!(mime, "text/html; charset=utf-8");
        assert!(text.contains("&lt;script&gt;") && !text.contains("<script>"), "{text}");
        // A folder (zipped by the engine) and an error are pages, not files.
        jobs.insert(2, job(20));
        assert!(matches!(on_done(&mut jobs, 2, &Ok(Outcome::ZipOfFolder)), Step::Error { text, cancel: false, .. } if text == NOT_A_FILE));
        jobs.insert(3, job(30));
        assert!(matches!(on_done(&mut jobs, 3, &Err("No such file".into())), Step::Error { token: 30, text, .. } if text == "No such file"));
        jobs.insert(4, job(40));
        assert!(matches!(on_unzip(&mut jobs, 4), Step::Error { cancel: true, .. }));
        // Every Termurion-made answer is 200, sandboxed, nosniff; the error text is escaped.
        for a in [error_answer("a <b>failed</b>", "iterm2-ssh://box/a"), page_answer(no_conductor_page(Some("box")))] {
            let h = headers_of(&a);
            let crate::webhost::Answer::Respond { status, body, .. } = &a else { panic!() };
            assert_eq!(*status, 200);
            assert!(h.iter().any(|(k, v)| k == "Content-Security-Policy" && v.starts_with("sandbox")), "{h:?}");
            assert!(h.iter().any(|(k, v)| k == "X-Content-Type-Options" && v == "nosniff"));
            assert!(!String::from_utf8_lossy(body).contains("<b>failed"));
        }
        // R13x8-NS: Termurion's own pages at an `iterm2-ssh:` URL get the same no-script policy, so they hold
        // no script; Try Again is a link to the page itself (a reload the verdict allows), which needs none.
        for a in [error_answer("x", "iterm2-ssh://box/a"), page_answer(no_conductor_page(Some("box")))] {
            assert_eq!(headers_of(&a), crate::browser_pages::file_headers());
            let crate::webhost::Answer::Respond { body, .. } = &a else { panic!() };
            let text = String::from_utf8_lossy(body);
            assert!(!text.contains("<script") && !text.contains("onclick"), "{text}");
            assert_eq!(text.matches(crate::browser_pages::SCRIPTLESS_RETRY).count(), 1, "{text}");
        }
        // A view that went away stops its jobs once.
        jobs.clear();
        jobs.insert(5, job(50));
        jobs.insert(6, PageJob { view_pane: 3, ..job(60) });
        assert_eq!(abandon_view(&mut jobs, 2), vec![5]);
        assert!(abandon_view(&mut jobs, 2).is_empty());
        assert_eq!(on_write(&mut jobs, 5, b"x"), Step::Taken);
        assert!(!jobs[&6].answered);
    }

    #[test]
    fn page_jobs_are_admitted_a_few_at_a_time() {
        use std::collections::HashMap;
        let mut jobs: HashMap<u64, PageJob> = HashMap::new();
        for i in 0..MAX_PER_VIEW as u64 {
            assert_eq!(admit(&jobs, 2), Ok(()));
            jobs.insert(i, job(i));
        }
        assert_eq!(admit(&jobs, 2), Err(TOO_MANY), "one view's share");
        assert_eq!(admit(&jobs, 3), Ok(()), "another view still starts");
        for i in 100..100 + (MAX_TOTAL - MAX_PER_VIEW) as u64 {
            jobs.insert(i, PageJob { view_pane: 3 + i, ..job(i) });
        }
        assert_eq!(admit(&jobs, 99), Err(TOO_MANY), "the app-wide total");
    }

    #[test]
    fn ssh_page_buffer_is_capped() {
        assert!(!over_cap(PAGE_CAP - 1, 1));
        assert!(over_cap(PAGE_CAP, 1));
        assert!(over_cap(u64::MAX, 1));
        assert_eq!(PAGE_CAP, 64 * 1024 * 1024);
        assert_eq!(TOO_LARGE, "The file is larger than 64 MB and cannot be shown.");
    }
}
