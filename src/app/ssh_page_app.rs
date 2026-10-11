//! The ssh page on the app side (milestone 13x8 Task 9): `iterm2-ssh://<host><path>` shows a remote file
//! over a session the person already has. The URL only SELECTS among the registry's sessions; it never
//! opens a connection, asks for a credential, or becomes part of a command: the path is one typed
//! argument of the transfer engine (14d). Remote content is served like a downloaded file (no script,
//! sandbox, nosniff) through the one serving path of file pages.

use super::*;
use crate::ssh_files::ScpPath;
use crate::ssh_page::{self, PageJob, Step};
use crate::ssh_transfer::Action;

impl App {
    /// Whether a browser pane may be opened for a View: browser profiles on and the runtime found
    /// (`iTermBrowserGateway.browserAllowed`, `GW:98-111`).
    pub(super) fn browser_allowed(&self) -> bool {
        self.browser_gate().0
    }

    /// The View question's answer: Download (0) downloads, View (1) opens the tab (once more only if the
    /// browser is still allowed), Cancel (2) does nothing.
    pub(super) fn ssh_view_answered(&mut self, pane: PaneId, to: &ScpPath, index: usize) {
        match index {
            0 => self.ssh_download(pane, &to.path),
            1 if self.browser_allowed() => {
                let url = ssh_page::view_in_browser_url(&to.hostname, &to.path);
                self.open_ssh_view(&url, Some(pane));
            }
            _ => {}
        }
    }

    /// Answers the deferred request of `view_pane` if that view still waits (a view made again ignores
    /// a token it never issued; a dropped view has nothing to answer).
    fn ssh_page_answer(&self, view_pane: PaneId, token: u64, answer: crate::webhost::Answer) {
        if let Some(v) = self.browser_views.get(&view_pane).and_then(|b| b.view.as_ref()) {
            v.answer_request(token, answer);
        }
    }

    /// A request for an `iterm2-ssh:` URL the one request judge allowed (the pane's own navigation, or
    /// a subresource of its page's host and tree). The first session of the registry that matches the
    /// URL and has a pane (`conductors(for:).first`, `SSHM:49`) fetches the file as one stream; no
    /// session shows the page that says so. The answer comes at the transfer's end (`ssh_page_job`).
    pub(super) fn ssh_page_request(&mut self, pane: PaneId, token: u64, url: &str, document: bool) {
        let Some((host, user, port, path)) = ssh_page::parse(url) else {
            return self.ssh_page_answer(pane, token, ssh_page::error_answer("Invalid file path.", url));
        };
        let found = self.ssh_registry.identities().into_iter().filter(|id| ssh_page::identity_matches(id, &host, user.as_deref(), port)).find_map(|id| self.pane_for_identity(&id));
        let Some(conductor) = found else {
            return self.ssh_page_answer(pane, token, ssh_page::page_answer(ssh_page::no_conductor_page(Some(&host))));
        };
        if let Err(why) = ssh_page::admit(&self.ssh_page_jobs, pane) {
            return self.ssh_page_answer(pane, token, ssh_page::error_answer(why, url));
        }
        let ext = std::path::Path::new(&path).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let mime = crate::mime_ext::mime_for_extension(&ext).unwrap_or("application/octet-stream");
        let (job, actions) = self.ssh_transfers.download_file_only(conductor, &path);
        self.ssh_page_jobs.insert(job, PageJob { pane: conductor, view_pane: pane, token, url: url.to_string(), path, document, mime, body: Vec::new(), answered: false });
        self.ssh_transfer_apply(actions);
    }

    /// Offered every action of the transfer engine first (`ssh_transfer_apply`): the size, bytes, end
    /// and unzip of a page job are taken here, so a page's bytes never reach Downloads. True when the
    /// action belonged to a page job.
    pub(super) fn ssh_page_job(&mut self, action: &Action) -> bool {
        let (job, step) = match action {
            Action::Size { job, size } => (*job, ssh_page::on_size(&mut self.ssh_page_jobs, *job, *size)),
            Action::Write { job, bytes } => (*job, ssh_page::on_write(&mut self.ssh_page_jobs, *job, bytes)),
            Action::Unzip { job } => (*job, ssh_page::on_unzip(&mut self.ssh_page_jobs, *job)),
            Action::Done { job, result } => (*job, ssh_page::on_done(&mut self.ssh_page_jobs, *job, result)),
            _ => return false,
        };
        match step {
            Step::NotMine => return false,
            Step::Taken => {}
            Step::Body(done) => {
                let (view, token) = (done.view_pane, done.token);
                self.ssh_page_answer(view, token, ssh_page::body_answer(done));
            }
            Step::Error { view_pane, token, url, text, cancel } => {
                self.ssh_page_answer(view_pane, token, ssh_page::error_answer(&text, &url));
                if cancel {
                    // The engine ends the job; its Done comes back here and is taken (already answered).
                    let actions = self.ssh_transfers.fail(job, &text);
                    self.ssh_transfer_apply(actions);
                }
            }
        }
        true
    }

    /// The view of `pane` was dropped or made again (or the pane went): its page jobs stop, answer
    /// nothing, and free what they hold. A deferral its view still held was completed by the view's
    /// own shutdown.
    pub(super) fn ssh_page_view_gone(&mut self, pane: PaneId) {
        for job in ssh_page::abandon_view(&mut self.ssh_page_jobs, pane) {
            if self.ssh_transfers.is_live(job) {
                let actions = self.ssh_transfers.fail(job, "The page was closed.");
                self.ssh_transfer_apply(actions);
            } else {
                self.ssh_page_jobs.remove(&job);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn between<'a>(s: &'a str, from: &str, to: &str) -> &'a str {
        let a = s.find(from).unwrap_or_else(|| panic!("missing {from}"));
        &s[a..a + s[a..].find(to).unwrap_or_else(|| panic!("missing {to}"))]
    }

    /// View in 14d's Download or View: the type decides, the browser must be allowed, the prompt keeps
    /// 14d's identifier, and only View opens a tab at the view URL through the one open path.
    #[test]
    fn view_in_browser_follows_iterm2() {
        use crate::ssh_files::{download_or_view, ViewChoice};
        let png = crate::mime_ext::mime_for_extension("png");
        assert_eq!(download_or_view(png, true), ViewChoice::Ask { mime: "image/png".into() });
        assert_eq!(download_or_view(crate::mime_ext::mime_for_extension("tar"), true), ViewChoice::Download);
        assert_eq!(download_or_view(png, false), ViewChoice::Download);
        let files = include_str!("ssh_files_app.rs");
        let choose = between(files, "pub(super) fn ssh_download_or_view", "/// The Win-click's last phase");
        assert!(choose.contains("crate::mime_ext::mime_for_extension(") && choose.contains("self.browser_allowed()"));
        assert!(choose.contains("ssh_files::VIEW_HEADING") && choose.contains("ssh_files::view_id(") && choose.contains("ssh_files::view_title("));
        assert!(choose.contains("Then::SshViewOrDownload"), "the prompt, not a silent choice");
        assert!(choose.contains("ssh_page::view_in_browser_url(") && choose.contains("ssh_page::view_url_ok("), "a path the URL cannot carry downloads");
        assert!(!choose.contains("BROWSER_ALLOWED"));
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::SshViewOrDownload { pane, to, .. } => self.ssh_view_answered(pane, &to, index)"));
        assert!(alerts.contains("Then::SshViewOrDownload { .. } => vec![0, 2]"), "only View is remembered");
        let me = include_str!("ssh_page_app.rs");
        let answered = between(me, "pub(super) fn ssh_view_answered", "/// Answers the deferred request");
        assert!(answered.contains("0 => self.ssh_download(") && answered.contains("self.open_ssh_view(") && answered.contains("self.browser_allowed()"));
        // The tab opens through the browser's own open path, whose first load is Tandem's own.
        let browser = include_str!("browser_app.rs");
        let open = between(browser, "pub(super) fn open_ssh_view(", "pub(super) fn open_link_split");
        assert!(open.contains("ssh_page::view_url_ok(") && open.contains("self.ssh_view_url = Some(") && open.contains("self.open_in_browser_pane("));
        let start = between(browser, "pub(super) fn start_browser_pane", "let dev_null");
        assert!(start.contains("self.ssh_view_url.take()") && start.contains("ssh_page::view_url_ok("));
        let m = include_str!("mod.rs");
        assert!(m.contains("ssh_view_url: Option<String>,") && m.contains("ssh_page_jobs: HashMap<u64, crate::ssh_page::PageJob>,"));
    }

    /// A page job's actions go to the page first; nothing of it reaches 14d's Downloads bookkeeping.
    #[test]
    fn page_jobs_never_reach_downloads() {
        let files = include_str!("ssh_files_app.rs");
        let apply = between(files, "pub(super) fn ssh_transfer_apply", "/// A download's own temp file");
        let page = apply.find("self.ssh_page_job(&action)").expect("offered to the page first");
        assert!(page < apply.find("match action").unwrap(), "before any arm");
        for local in ["Action::Size", "Action::Write", "Action::Done", "Action::Unzip"] {
            assert!(page < apply.find(local).unwrap(), "{local}");
        }
        let me = include_str!("ssh_page_app.rs");
        let job = between(me, "pub(super) fn ssh_page_job", "/// The view");
        assert!(job.contains("Action::Size") && job.contains("Action::Write") && job.contains("Action::Unzip") && job.contains("Action::Done"));
        assert!(!job.contains("ssh_locals") && !job.contains("downloads_folder") && !job.contains("banners"));
    }

    /// A request names a session the person has; the path is one typed argument; no URL is logged.
    #[test]
    fn ssh_requests_use_open_sessions_only() {
        let me = include_str!("ssh_page_app.rs");
        let live = me.split("#[cfg(test)]").next().unwrap();
        let req = between(live, "pub(super) fn ssh_page_request", "pub(super) fn ssh_page_job");
        let parse = req.find("ssh_page::parse(").unwrap();
        let ident = req.find("ssh_page::identity_matches(").unwrap();
        let pane = req.find("self.pane_for_identity(").unwrap();
        let start = req.find("self.ssh_transfers.download_file_only(").unwrap();
        assert!(parse < ident && ident < pane && pane < start);
        assert!(req.contains("self.ssh_registry.identities()"), "only a session the registry holds");
        assert!(req.contains("ssh_page::no_conductor_page("), "none: the page that says so");
        assert!(req.contains("ssh_page::admit("), "a few at a time");
        for banned in ["rlog!", "dlog", "Command::new", "ssh_connect", "ssh_start", "prompt", "password"] {
            assert!(!live.contains(banned), "{banned}");
        }
        // The deferral is answered only into the view that asked.
        assert!(live.contains("self.browser_views.get(&job.view_pane)") || live.contains("self.browser_views.get(&view_pane)"));
        // The page request path hands an allowed ssh request here, after the one request judge.
        let pages = include_str!("browser_pages_app.rs");
        let r = between(pages, "pub(super) fn browser_page_request", "/// The answers the workers finished");
        assert!(r.find("page_request_allowed(").unwrap() < r.find("ssh_page_request(").unwrap());
        assert!(r.contains("if allowed && ssh {"));
    }

    /// A view that is dropped or made again abandons its page jobs and a pane that goes does too.
    #[test]
    fn dropped_views_abandon_page_jobs() {
        let scripts = include_str!("browser_scripts_app.rs");
        let reset = between(scripts, "pub(super) fn browser_reset_world", "// --- Setup ---");
        assert!(reset.contains("self.ssh_page_view_gone(pane);"));
        let m = include_str!("mod.rs");
        assert!(between(m, "pub(super) fn pane_gone", "self.close_browser_view(id);").contains("self.ssh_page_view_gone(id);"), "a browser pane that goes");
        assert!(include_str!("ssh_files_app.rs").contains("fn ssh_files_pane_gone"), "a session that goes fails its waiters, whose Done answers the page");
    }
}
