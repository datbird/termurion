//! Serving local files to `iterm2-file:` pages (milestone 13x8, Task 3): the filesystem half of the
//! file page. The person's own navigation (typed, Open File, a clicked terminal link) opens a file or a
//! folder; the page it shows may load files from its own folder tree. Nothing here is run, opened or
//! executed: a file is read as bytes and answered, a folder is listed.
//!
//! Every path comes from `browser_pages::file_path_of` (the one decoder: no share, no device name, no
//! stream, no `..`), is judged by `localfs::LocalPath::judge` (no link to a share), and is resolved to
//! its canonical form, which is judged again and must not be a UNC or device form. A subresource must
//! also resolve INSIDE its page's folder tree, so a link or junction in the tree cannot lead out of it.
//! Reads are bounded (`MAX_FILE`, `MAX_LISTED`) and run on worker threads (`Pool`), never on the UI
//! thread. Every answer carries the sandbox policy: a trusted HTML file runs script only in an opaque
//! origin that can reach nothing (`browser_pages::SANDBOX_CSP`); a file with a Mark of the Web, an SVG
//! or an XML file runs none (`UNTRUSTED_CSP`). The 404 for a missing, refused or oversized path carries
//! no text and names no path.

use crate::browser_pages::{self, PageRoot, CONTEXT_DOCUMENT, NOT_FOUND_HTML};
use crate::localfs::{self, LocalPath};
use crate::webhost::Answer;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The largest file answered whole (a larger one is a 404 with a log line that names no path).
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

fn answer(status: u16, mime: &str, body: Vec<u8>, headers: Vec<(String, String)>) -> Answer {
    Answer::Respond { status, mime: mime.to_string(), body, headers }
}

/// A missing, refused or oversized path: 404, no text, the sandbox policy.
pub fn not_found() -> Answer {
    answer(404, "text/html; charset=utf-8", NOT_FOUND_HTML.as_bytes().to_vec(), browser_pages::file_headers())
}

/// `p` lies strictly inside `root`, compared as Windows compares: separators alike, case ignored,
/// the `\\?\` prefix dropped, whole components only (`C:\ab` is not inside `C:\a`).
pub fn inside(root: &Path, p: &Path) -> bool {
    fn norm(p: &Path) -> String {
        let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches('\\').to_string();
        s
    }
    let (r, p) = (norm(root), norm(p));
    !r.is_empty() && p.len() > r.len() && p.starts_with(&r) && p.as_bytes()[r.len()] == b'\\'
}

/// The folder of a page root as a path (`C:/T/site/` is `C:\T\site`).
fn root_path(root: &PageRoot) -> PathBuf {
    PathBuf::from(root.folder.trim_end_matches('/').replace('/', "\\"))
}

/// A judged path resolved through every link, judged again and never a share or device form.
fn resolve(path: &Path) -> Option<LocalPath> {
    let local = LocalPath::judge(path)?;
    let canon = localfs::canonicalize(&local)?;
    browser_pages::link_target_ok(canon.path()).then_some(canon)
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A type whose document can run script or fetch: HTML and anything XML (SVG, XHTML, RSS).
fn markup(mime: &str) -> bool {
    mime == "text/html" || mime.contains("xml")
}

/// Whether the path an open handle names may be served: never a share or device form, and, for a
/// subresource, strictly inside its page's resolved tree (`base`). Pure, so the order is tested.
pub fn confined(base: Option<&Path>, final_path: &Path) -> bool {
    browser_pages::link_target_ok(final_path) && base.is_none_or(|b| inside(b, final_path))
}

/// The answer to a request for `url` under `root` (the page's tree; None for a request that has none,
/// which only a Document request may). Pure of any App state, so a worker thread runs it.
pub fn serve(url: &str, context: i32, root: Option<&PageRoot>) -> Answer {
    serve_with(url, context, root, MAX_FILE)
}

/// `serve` with the size cap given (a test lowers it).
pub fn serve_with(url: &str, context: i32, root: Option<&PageRoot>, max_file: u64) -> Answer {
    let document = context == CONTEXT_DOCUMENT;
    let Ok(path) = browser_pages::file_path_of(url) else { return not_found() };
    let Some(canon) = resolve(&path) else { return not_found() };
    // A subresource resolves inside its page's tree, links and junctions followed: one that leads out
    // is not served, and neither is one when the page has no root. The root is resolved once here and
    // checked again against the OPEN handle below.
    let base = if document {
        None
    } else {
        let Some(root) = root else { return not_found() };
        let Some(base) = resolve(&root_path(root)) else { return not_found() };
        if !inside(base.path(), canon.path()) {
            return not_found();
        }
        Some(base)
    };
    let Ok(stat) = localfs::stat(&canon) else { return not_found() };
    if stat.is_dir {
        return if document { listing(&path, &canon) } else { not_found() };
    }
    // Open ONCE. What the handle really names is checked (inside the root, no share or device form),
    // and the kind, the size, the bytes and the Mark of the Web all come from that handle or its final
    // path, never from a second look at a path that may have been swapped since (review I1).
    let Ok(mut file) = localfs::open(&canon) else { return not_found() };
    let Some(final_path) = localfs::final_path(&file) else { return not_found() };
    if !confined(base.as_ref().map(|b| b.path()), &final_path) {
        return not_found();
    }
    let Ok(stat) = localfs::file_stat(&file) else { return not_found() };
    if !stat.is_file || stat.len > max_file {
        if stat.len > max_file {
            crate::rlog!("browser: a local file over the size cap was not served");
        }
        return not_found();
    }
    let mut bytes = Vec::with_capacity(stat.len.min(1 << 20) as usize);
    // Read one byte past the cap: a file that grew after the check is refused, not cut.
    if (&mut file).take(max_file + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > max_file {
        return not_found();
    }
    // The Mark of the Web is read from the handle's verified final path: a copy of a downloaded file keeps it.
    let marked = crate::inert_files::has_mark(&final_path);
    let mime = crate::mime_ext::mime_for_extension(&extension(&final_path)).unwrap_or("application/octet-stream");
    file_answer(&path, document, mime, marked, bytes)
}

/// The answer that carries a file's bytes. A Document request: an HTML file as it is, a picture, video
/// or audio file as itself, an SVG or XML file as itself with no script, any other file as escaped
/// text in a `<pre>` (`FH:143-157`). A subresource: the bytes with the table's type. A file with a
/// Mark of the Web, an SVG or an XML file gets the no-script policy; so does every markup subresource
/// (only the page the person opened may run its own script).
fn file_answer(path: &Path, document: bool, mime: &str, marked: bool, bytes: Vec<u8>) -> Answer {
    let html = mime == "text/html";
    let untrusted = marked || mime.contains("xml") || (!document && html);
    let headers = if untrusted { browser_pages::untrusted_headers() } else { browser_pages::file_headers() };
    if document && (html || !(mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") || markup(mime))) {
        let page = browser_pages::file_page(path, &bytes);
        return answer(200, "text/html; charset=utf-8", page.into_bytes(), headers);
    }
    answer(200, mime, bytes, headers)
}

/// A folder as a page: its entries from the listing (kind read from the entry itself, never followed),
/// hidden ones marked, a link that leads off the machine left out. At most `MAX_LISTED + 1` are read.
fn listing(path: &Path, canon: &LocalPath) -> Answer {
    let Ok(read) = localfs::entries_upto(canon, browser_pages::MAX_LISTED + 1) else { return not_found() };
    let entries: Vec<browser_pages::Entry> = read
        .iter()
        .filter(|e| !e.stat.is_reparse || e.local().is_some())
        .map(|e| {
            let name = e.name.to_string_lossy().into_owned();
            let hidden = e.stat.hidden || name.starts_with('.');
            let dir = e.stat.is_dir || (e.stat.is_reparse && e.local().is_some_and(|l| localfs::is_dir(&l)));
            browser_pages::Entry { name, dir, hidden }
        })
        .collect();
    let page = browser_pages::directory_page(path, &entries);
    answer(200, "text/html; charset=utf-8", page.into_bytes(), browser_pages::file_headers())
}

// --- The worker pool: the UI thread never reads a file ---

/// One request waiting for a worker.
pub struct Job {
    pub pane: u64,
    pub token: u64,
    pub url: String,
    pub context: i32,
    pub root: Option<PageRoot>,
}

/// A finished request, for the UI thread to answer.
pub struct Done {
    pub pane: u64,
    pub token: u64,
    pub answer: Answer,
}

/// Requests that may wait at once; one more is refused with a 404 (a page asking for thousands of
/// files cannot queue them without end).
pub const MAX_QUEUED: usize = 128;
const WORKERS: usize = 2;

/// A few worker threads reading files, started with the pool and ended when it is dropped (the job
/// channel closes). `wake` tells the event loop a result is ready.
pub struct Pool {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    queued: Arc<AtomicUsize>,
}

impl Pool {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Pool {
        Pool::with_serve(wake, Arc::new(|j: &Job| serve(&j.url, j.context, j.root.as_ref())))
    }

    /// The pool with the work given (a test counts calls).
    pub fn with_serve(wake: Arc<dyn Fn() + Send + Sync>, work: Arc<dyn Fn(&Job) -> Answer + Send + Sync>) -> Pool {
        let (jobs, jobs_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        let jobs_rx = Arc::new(Mutex::new(jobs_rx));
        let queued = Arc::new(AtomicUsize::new(0));
        for _ in 0..WORKERS {
            let (rx, tx, wake, work, queued) = (jobs_rx.clone(), done_tx.clone(), wake.clone(), work.clone(), queued.clone());
            std::thread::spawn(move || loop {
                // The lock is held only while waiting for the next job.
                let job = match rx.lock() {
                    Ok(rx) => rx.recv(),
                    Err(_) => return,
                };
                let Ok(job) = job else { return };
                let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job))).unwrap_or_else(|_| not_found());
                queued.fetch_sub(1, Ordering::SeqCst);
                if tx.send(Done { pane: job.pane, token: job.token, answer }).is_err() {
                    return;
                }
                wake();
            });
        }
        Pool { jobs, done, queued }
    }

    /// Queues a request; false when the queue is full or the workers are gone (the caller answers 404).
    pub fn submit(&self, job: Job) -> bool {
        if self.queued.fetch_add(1, Ordering::SeqCst) >= MAX_QUEUED {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        if self.jobs.send(job).is_err() {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// The results ready now.
    pub fn finished(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(a: &Answer) -> (u16, String, String, Vec<(String, String)>) {
        let Answer::Respond { status, mime, body, headers } = a else { panic!("a response") };
        (*status, mime.clone(), String::from_utf8_lossy(body).into_owned(), headers.clone())
    }

    fn csp(headers: &[(String, String)]) -> &str {
        headers.iter().find(|(k, _)| k == "Content-Security-Policy").map(|(_, v)| v.as_str()).unwrap_or("")
    }

    /// A fresh folder under the temp folder (removed by the caller).
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tandem-fileserve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn url(p: &Path, dir: bool) -> String {
        browser_pages::file_url_of(p, dir)
    }

    fn root_of(p: &Path) -> PageRoot {
        browser_pages::page_root(&url(p, false)).unwrap()
    }

    #[test]
    fn inside_compares_whole_components() {
        assert!(inside(Path::new(r"C:\T\site"), Path::new(r"c:\t\SITE\a.png")));
        assert!(inside(Path::new("C:/T/site/"), Path::new(r"\\?\C:\T\site\sub\a.png")));
        assert!(inside(Path::new(r"C:\"), Path::new(r"C:\x")));
        assert!(!inside(Path::new(r"C:\T\site"), Path::new(r"C:\T\site")));
        assert!(!inside(Path::new(r"C:\T\site"), Path::new(r"C:\T\site2\a.png")));
        assert!(!inside(Path::new(r"C:\T\site"), Path::new(r"C:\T\other\a.png")));
        assert!(!inside(Path::new(r"C:\T\site"), Path::new(r"D:\T\site\a.png")));
        assert!(!inside(Path::new(""), Path::new(r"C:\x")));
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_lists_and_a_file_is_a_page() {
        let t = temp("list");
        std::fs::create_dir_all(t.join("sub")).unwrap();
        std::fs::write(t.join("b.txt"), "a<b").unwrap();
        std::fs::write(t.join("p.html"), "<b>x</b>").unwrap();
        std::fs::write(t.join(".hidden"), "h").unwrap();
        let (s, mime, html, h) = body(&serve(&url(&t, true), CONTEXT_DOCUMENT, None));
        assert_eq!((s, mime.as_str()), (200, "text/html; charset=utf-8"));
        assert!(html.contains(">sub</a>") && html.contains(">b.txt</a>") && html.contains(">p.html</a>") && !html.contains(".hidden"));
        assert_eq!(csp(&h), browser_pages::SANDBOX_CSP);
        let (s, _, text, h) = body(&serve(&url(&t.join("b.txt"), false), CONTEXT_DOCUMENT, None));
        assert_eq!((s, text.as_str()), (200, "<pre>a&lt;b</pre>"));
        assert_eq!(csp(&h), browser_pages::SANDBOX_CSP);
        // HTML from disk: as it is, with the sandbox (scripts only in an opaque origin).
        let (s, _, text, h) = body(&serve(&url(&t.join("p.html"), false), CONTEXT_DOCUMENT, None));
        assert_eq!((s, text.as_str()), (200, "<b>x</b>"));
        assert_eq!(csp(&h), browser_pages::SANDBOX_CSP);
        assert!(h.iter().any(|(k, v)| k == "X-Content-Type-Options" && v == "nosniff"));
        // A folder is no subresource; a missing path, a device name, a stream and a share are 404 with no text.
        let root = root_of(&t.join("p.html"));
        for (u, ctx) in [
            (url(&t, true), CONTEXT_FETCH_LIKE),
            (url(&t.join("nothing"), false), CONTEXT_DOCUMENT),
            ("iterm2-file:///C:/CON".to_string(), CONTEXT_DOCUMENT),
            (format!("{}:stream", url(&t.join("b.txt"), false)), CONTEXT_DOCUMENT),
            ("iterm2-file://srv/share/x".to_string(), CONTEXT_DOCUMENT),
            ("iterm2-file:///%5C%5Csrv%5Cshare".to_string(), CONTEXT_DOCUMENT),
            ("iterm2-file:".to_string(), CONTEXT_DOCUMENT),
        ] {
            let (s, _, text, h) = body(&serve(&u, ctx, Some(&root)));
            assert_eq!((s, text.as_str()), (404, ""), "{u}");
            assert_eq!(csp(&h), browser_pages::SANDBOX_CSP, "{u}");
        }
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// A subresource with no root, outside the root, or beyond a link out of it is a 404; one inside is served
    /// with its table type, and markup or a marked file runs no script.
    #[cfg(windows)]
    #[test]
    fn subresources_stay_inside_the_tree() {
        let t = temp("tree");
        let site = t.join("site");
        std::fs::create_dir_all(site.join("sub")).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        std::fs::write(site.join("a.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(site.join("sub").join("s.js"), "var a=1;").unwrap();
        std::fs::write(site.join("v.svg"), "<svg/>").unwrap();
        std::fs::write(site.join("f.html"), "<p>f</p>").unwrap();
        std::fs::write(t.join("other").join("secret.txt"), "s3cret").unwrap();
        let root = root_of(&site.join("a.html"));
        let (s, mime, text, h) = body(&serve(&url(&site.join("a.png"), false), 3, Some(&root)));
        assert_eq!((s, mime.as_str(), text.len()), (200, "image/png", 6));
        assert_eq!(csp(&h), browser_pages::SANDBOX_CSP);
        let (s, mime, _, _) = body(&serve(&url(&site.join("sub").join("s.js"), false), 6, Some(&root)));
        assert_eq!((s, mime.as_str()), (200, "text/javascript"));
        // Markup as a subresource never runs script.
        for f in ["v.svg", "f.html"] {
            let (s, _, _, h) = body(&serve(&url(&site.join(f), false), 4, Some(&root)));
            assert_eq!(s, 200, "{f}");
            assert_eq!(csp(&h), browser_pages::UNTRUSTED_CSP, "{f}");
        }
        // As the page the person opened, SVG shows with no script, HTML with the sandbox.
        let (_, mime, _, h) = body(&serve(&url(&site.join("v.svg"), false), CONTEXT_DOCUMENT, None));
        assert_eq!((mime.as_str(), csp(&h)), ("image/svg+xml", browser_pages::UNTRUSTED_CSP));
        // The sibling folder, the parent, no root, a folder: all 404.
        for (u, r) in [
            (url(&t.join("other").join("secret.txt"), false), Some(&root)),
            (url(&t.join("site2"), false), Some(&root)),
            (url(&site.join("a.png"), false), None),
            (url(&site.join("sub"), true), Some(&root)),
        ] {
            let (s, _, text, _) = body(&serve(&u, 3, r));
            assert_eq!((s, text.as_str()), (404, ""), "{u}");
        }
        // The size cap: one byte over is refused, the cap itself is served.
        let (s, ..) = body(&serve_with(&url(&site.join("a.png"), false), 3, Some(&root), 3));
        assert_eq!(s, 404);
        let (s, ..) = body(&serve_with(&url(&site.join("a.png"), false), 3, Some(&root), 4));
        assert_eq!(s, 200);
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// A junction inside the tree that leads out of it is not followed for a subresource.
    #[cfg(windows)]
    #[test]
    fn a_junction_cannot_lead_out() {
        let t = temp("junction");
        let site = t.join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        std::fs::write(t.join("other").join("secret.txt"), "s3cret").unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(site.join("j")).arg(t.join("other")).output().unwrap();
        assert!(made.status.success());
        let root = root_of(&site.join("a.html"));
        let (s, _, text, _) = body(&serve(&url(&site.join("j").join("secret.txt"), false), 3, Some(&root)));
        assert_eq!((s, text.as_str()), (404, ""), "a link out of the tree");
        std::fs::remove_dir(site.join("j")).unwrap();
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// A file with a Mark of the Web runs no script, even as the page the person opened.
    #[cfg(windows)]
    #[test]
    fn a_marked_file_is_untrusted() {
        let t = temp("motw");
        std::fs::write(t.join("m.html"), "<script>1</script>").unwrap();
        std::fs::write(t.join("n.html"), "<script>1</script>").unwrap();
        crate::inert_files::mark(&t.join("m.html"));
        // The test host keeps streams (NTFS): a missing mark is a regression, never a skip.
        assert!(crate::inert_files::has_mark(&t.join("m.html")), "the mark was not written or cannot be read");
        let (_, _, text, h) = body(&serve(&url(&t.join("m.html"), false), CONTEXT_DOCUMENT, None));
        assert_eq!((text.as_str(), csp(&h)), ("<script>1</script>", browser_pages::UNTRUSTED_CSP));
        let (_, _, _, h) = body(&serve(&url(&t.join("n.html"), false), CONTEXT_DOCUMENT, None));
        assert_eq!(csp(&h), browser_pages::SANDBOX_CSP);
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// The check is on what the open handle names, in the order: share or device form first, then
    /// the tree (review I1, pure).
    #[test]
    fn confinement_is_judged_on_the_final_path() {
        let base = Path::new(r"\\?\C:\T\site");
        assert!(confined(Some(base), Path::new(r"\\?\C:\T\site\a.png")));
        assert!(!confined(Some(base), Path::new(r"\\?\C:\T\other\secret.txt")), "a file that left the tree");
        assert!(!confined(Some(base), Path::new(r"\\?\C:\T\site2\a.png")), "whole components only");
        assert!(!confined(None, Path::new(r"\\?\UNC\host\share\a.txt")), "a share is never served");
        assert!(confined(None, Path::new(r"\\?\C:\T\site\a.png")), "a document has no tree to stay in");
    }

    /// A file opened through a junction names its real place, which is outside the tree: the check
    /// on the handle refuses what a check on the requested path would have passed.
    #[cfg(windows)]
    #[test]
    fn a_handle_through_a_junction_names_the_target() {
        let t = temp("handle");
        let site = t.join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        std::fs::write(t.join("other").join("secret.txt"), "s3cret").unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(site.join("j")).arg(t.join("other")).output().unwrap();
        assert!(made.status.success());
        let f = std::fs::File::open(site.join("j").join("secret.txt")).unwrap();
        let real = localfs::final_path(&f).expect("final path");
        let base = resolve(&site).unwrap();
        assert!(inside(base.path(), &site.join("j").join("secret.txt")), "the requested path looks inside");
        assert!(!confined(Some(base.path()), &real), "the handle's own path is outside");
        drop(f);
        std::fs::remove_dir(site.join("j")).unwrap();
        std::fs::remove_dir_all(&t).unwrap();
    }

    const CONTEXT_FETCH_LIKE: i32 = 3;

    /// The file answer decides the policy by type and mark alone (pure).
    #[test]
    fn the_policy_follows_the_type_and_the_mark() {
        let p = Path::new(r"C:\x\a");
        let policy = |document: bool, mime: &str, marked: bool| {
            let (_, m, _, h) = body(&file_answer(p, document, mime, marked, b"x".to_vec()));
            (m, csp(&h) == browser_pages::UNTRUSTED_CSP)
        };
        assert_eq!(policy(true, "text/html", false), ("text/html; charset=utf-8".into(), false));
        assert_eq!(policy(true, "text/html", true), ("text/html; charset=utf-8".into(), true));
        assert_eq!(policy(false, "text/html", false), ("text/html".into(), true));
        assert_eq!(policy(true, "image/svg+xml", false), ("image/svg+xml".into(), true));
        assert_eq!(policy(true, "application/xhtml+xml", false), ("application/xhtml+xml".into(), true));
        assert_eq!(policy(true, "application/xml", false), ("application/xml".into(), true));
        assert_eq!(policy(true, "image/png", false), ("image/png".into(), false));
        assert_eq!(policy(true, "image/png", true), ("image/png".into(), true));
        assert_eq!(policy(true, "text/plain", false), ("text/html; charset=utf-8".into(), false));
        assert_eq!(policy(true, "application/octet-stream", false), ("text/html; charset=utf-8".into(), false));
        assert_eq!(policy(false, "text/css", false), ("text/css".into(), false));
    }

    /// The pool answers off the calling thread, wakes the loop, bounds its queue and ends with it.
    #[test]
    fn the_pool_answers_on_a_worker() {
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let main = std::thread::current().id();
        let pool = Pool::with_serve(
            Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(move |j: &Job| {
                assert_ne!(std::thread::current().id(), main, "never the calling thread");
                answer(200, "text/plain", j.url.clone().into_bytes(), Vec::new())
            }),
        );
        for n in 0..5 {
            assert!(pool.submit(Job { pane: 1, token: n, url: format!("u{n}"), context: 1, root: None }));
        }
        let mut got = Vec::new();
        for _ in 0..500 {
            got.extend(pool.finished());
            if got.len() == 5 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        got.sort_by_key(|d| d.token);
        assert_eq!(got.iter().map(|d| d.token).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert!(woke.load(Ordering::SeqCst) >= 5);
        // A full queue refuses.
        let blocked = Pool::with_serve(Arc::new(|| {}), Arc::new(|_: &Job| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            not_found()
        }));
        let mut accepted = 0;
        for n in 0..(MAX_QUEUED + 10) as u64 {
            if blocked.submit(Job { pane: 1, token: n, url: String::new(), context: 1, root: None }) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, MAX_QUEUED);
        drop(blocked);
    }
}
