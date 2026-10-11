//! Browser downloads on the App side (13x8 Task 7; `DL:66-189`). The web view only writes where it
//! is told (`webhost_win`); everything else is decided here, once: Dev Null refuses, the caps and
//! the caps deny a flood, the file is written as a dot-named `.download` file in the
//! Downloads folder and finished by `ssh_local::finish_with` (the Mark of the Web through
//! `inert_files::mark_strict`, a refused type removed, then the final name from
//! `browser_download::destination`). Nothing is opened or run: the page's "Show in folder" only
//! selects the file in Explorer. Names and hosts are shown through `inert_files::display_text`.

use super::*;
use crate::browser_download as bd;
use crate::webhost::Event;

fn exists(p: &std::path::Path) -> bool {
    crate::localfs::local_exists(p)
}

impl App {
    /// Where a download event goes (the pane is the view's own).
    pub(super) fn browser_download_event(&mut self, pane: PaneId, e: &Event) {
        match e {
            Event::DownloadStarting { token, url, total, .. } => self.download_starting(pane, *token, url, *total),
            Event::DownloadProgress { id, received, total, .. } => self.download_progress(*id, *received, *total),
            Event::DownloadDone { id, result, .. } => self.download_done(*id, result.clone()),
            // The view's own caps cancelled one: said, never silent (Task 7 review I1).
            Event::DownloadRefused { .. } => self.banners.set("action", bd::TOO_MANY_TEXT),
            _ => {}
        }
    }

    fn download_answer(&self, pane: PaneId, token: u64, id: u64, path: Option<&std::path::Path>) {
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.answer_download(token, id, path);
        }
    }

    fn download_starting(&mut self, pane: PaneId, token: u64, url: &str, total: Option<u64>) {
        self.download_seq += 1;
        let id = self.download_seq;
        let shown = crate::inert_files::display_text(&bd::kept_name(url), 200, false);
        let running = |d: &&bd::Download| d.phase == bd::Phase::Running;
        let in_pane = self.browser_downloads.values().filter(running).filter(|d| d.pane == pane).count();
        let all = self.browser_downloads.values().filter(running).count();
        if let bd::Verdict::Deny(text) = bd::verdict(self.pane_dev_null(pane), in_pane, all) {
            self.banners.set("action", text);
            return self.download_answer(pane, token, id, None);
        }
        let fail = |why: &str| Self::ssh_notify(&crate::ssh_local::download_failed(&shown), why);
        let Some(dir) = self.downloads_folder() else {
            fail(crate::ssh_local::NO_DOWNLOADS);
            return self.download_answer(pane, token, id, None);
        };
        let taken = |p: &std::path::Path| exists(p) || self.browser_downloads.values().any(|d| d.phase == bd::Phase::Running && d.path == p);
        let dest = match bd::destination(&dir, url, &taken) {
            Ok(d) => d,
            Err(why) => {
                fail(why);
                return self.download_answer(pane, token, id, None);
            }
        };
        let temp = bd::temp_path(&dir, &uuid::Uuid::new_v4().simple().to_string());
        if !bd::download_path_ok(&dir, &temp) {
            fail(crate::ssh_local::NOT_ALLOWED);
            return self.download_answer(pane, token, id, None);
        }
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.browser_downloads.insert(id, bd::Download { pane, url: url.to_string(), name, path: dest, received: 0, total, temp: temp.clone(), step: 0, phase: bd::Phase::Running });
        self.download_trim();
        self.download_banner(id);
        self.download_answer(pane, token, id, Some(&temp));
        self.downloads_page_refresh();
    }

    fn download_banner(&mut self, id: u64) {
        let Some(d) = self.browser_downloads.get(&id) else { return };
        let text = bd::progress_text(&crate::inert_files::display_text(&d.name, 200, false), &crate::inert_files::display_text(&bd::host_of(&d.url), 100, false), d.received, d.total);
        self.banners.set(bd::BANNER_KEY, text);
        self.redraw();
    }

    fn download_progress(&mut self, id: u64, received: u64, total: Option<u64>) {
        let Some(d) = self.browser_downloads.get_mut(&id).filter(|d| d.phase == bd::Phase::Running) else { return };
        d.received = received;
        d.total = total.or(d.total);
        let step = bd::progress_step(d.received, d.total);
        if step == d.step {
            return;
        }
        d.step = step;
        self.download_banner(id);
        self.downloads_page_refresh();
    }

    fn download_done(&mut self, id: u64, result: Result<(), String>) {
        let Some(d) = self.browser_downloads.get(&id).cloned() else { return };
        let shown = crate::inert_files::display_text(&d.name, 200, false);
        let remove_temp = |d: &bd::Download| {
            if let Ok(l) = crate::localfs::judge_io(&d.temp) {
                let _ = crate::localfs::remove_file(&l);
            }
        };
        let outcome: Result<std::path::PathBuf, String> = match (&d.phase, result) {
            (bd::Phase::Running, Ok(())) => self.download_finish(&d),
            (bd::Phase::Running, Err(why)) => Err(why),
            // Cancelled by the person (or the view): nothing to report.
            _ => {
                remove_temp(&d);
                if let Some(e) = self.browser_downloads.get_mut(&id) {
                    e.phase = bd::Phase::Cancelled;
                }
                self.download_banner_clear();
                self.downloads_page_refresh();
                return;
            }
        };
        match outcome {
            Ok(path) => {
                // The name the file really got (the final pick may differ from the first).
                let final_name = path.file_name().map(|n| crate::inert_files::display_text(&n.to_string_lossy(), 200, false)).unwrap_or(shown);
                if let Some(e) = self.browser_downloads.get_mut(&id) {
                    e.name = final_name.clone();
                    e.path = path;
                    e.phase = bd::Phase::Finished;
                }
                Self::ssh_notify(&crate::ssh_local::download_finished(&final_name), "");
            }
            Err(why) => {
                remove_temp(&d);
                if let Some(e) = self.browser_downloads.get_mut(&id) {
                    e.phase = bd::Phase::Failed(why.clone());
                }
                Self::ssh_notify(&crate::ssh_local::download_failed(&shown), &why);
            }
        }
        self.download_banner_clear();
        self.downloads_page_refresh();
    }

    /// Marks the finished hidden file and moves it to its final name, picked again now (another download
    /// may have taken the first choice) and checked to sit directly in the Downloads folder.
    fn download_finish(&mut self, d: &bd::Download) -> Result<std::path::PathBuf, String> {
        let dir = d.temp.parent().ok_or_else(|| crate::ssh_local::NOT_ALLOWED.to_string())?.to_path_buf();
        // The move never replaces: a file that appeared under the picked name since the pick makes the
        // move fail with AlreadyExists, and the name is picked again (a few times at most).
        let mark = |p: &std::path::Path, u: &str| crate::inert_files::mark_strict(p, bd::mark_url(u).as_deref());
        for _ in 0..4 {
            let dest = bd::destination(&dir, &d.url, &exists).map_err(str::to_string)?;
            match crate::ssh_local::finish_no_replace(&d.temp, &dest, &d.url, &mark) {
                Err(crate::ssh_local::NoReplace::Taken) => continue,
                Err(crate::ssh_local::NoReplace::Failed(why)) => return Err(why),
                Ok(p) => return Ok(p),
            }
        }
        Err(crate::ssh_local::NOT_ALLOWED.to_string())
    }

    fn download_banner_clear(&mut self) {
        let next = self.browser_downloads.iter().filter(|(_, d)| d.phase == bd::Phase::Running).map(|(id, _)| *id).max();
        match next {
            Some(id) => self.download_banner(id),
            None => self.banners.clear(bd::BANNER_KEY),
        }
    }

    /// Old rows fall off the list (never a running one).
    fn download_trim(&mut self) {
        while self.browser_downloads.len() > bd::MAX_ROWS {
            let oldest = self.browser_downloads.iter().filter(|(_, d)| d.phase != bd::Phase::Running).map(|(id, _)| *id).min();
            match oldest {
                Some(id) => {
                    self.browser_downloads.remove(&id);
                }
                None => break,
            }
        }
    }

    /// A view went away (the pane closed, or the view was rebuilt): its running downloads die with it
    /// (the host cancels the operations), their hidden files are removed.
    pub(super) fn browser_downloads_view_gone(&mut self, pane: PaneId) {
        let mut any = false;
        for d in self.browser_downloads.values_mut().filter(|d| d.pane == pane && d.phase == bd::Phase::Running) {
            d.phase = bd::Phase::Cancelled;
            if let Ok(l) = crate::localfs::judge_io(&d.temp) {
                let _ = crate::localfs::remove_file(&l);
            }
            any = true;
        }
        if any {
            self.download_banner_clear();
            self.downloads_page_refresh();
        }
    }

    /// The rows the page shows, newest first.
    fn downloads_rows(&self) -> serde_json::Value {
        let mut ids: Vec<u64> = self.browser_downloads.keys().copied().collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        serde_json::Value::Array(ids.into_iter().filter_map(|id| self.browser_downloads.get(&id).map(|d| bd::row_json(id, d))).collect())
    }

    /// Redraws the list in every pane that shows the downloads page (guarded to that page).
    pub(super) fn downloads_page_refresh(&mut self) {
        let rows = self.downloads_rows();
        let call = format!("window.onDownloads && window.onDownloads({rows})");
        let panes: Vec<PaneId> = self.browser_views.keys().copied().filter(|p| self.panes.get(p).and_then(|x| x.browser_io()).is_some_and(|io| io.url == bd::PAGE_URL)).collect();
        for pane in panes {
            for reply in crate::browser_pages::script(bd::PAGE, &call) {
                if let (crate::browser_pages::Reply::Script(js), Some(v)) = (reply, self.browser_views.get(&pane).and_then(|b| b.view.as_ref())) {
                    v.execute_script(&js, 0);
                }
            }
        }
    }

    /// A request from the downloads page. It acts only when the pane's own main-frame URL AND the
    /// message's source are the page, so a web page or a page that navigated away changes nothing. The
    /// page names a download by id only; no path or name comes from it.
    pub(super) fn browser_downloads_message(&mut self, pane: PaneId, source: &str, msg: bd::Msg) {
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        if url != bd::PAGE_URL || source != bd::PAGE_URL {
            return;
        }
        match msg {
            bd::Msg::List => self.downloads_page_refresh(),
            bd::Msg::Cancel(id) => {
                let Some(d) = self.browser_downloads.get_mut(&id).filter(|d| d.phase == bd::Phase::Running) else { return };
                d.phase = bd::Phase::Cancelled;
                let owner = d.pane;
                if let Some(v) = self.browser_views.get(&owner).and_then(|b| b.view.as_ref()) {
                    v.cancel_download(id);
                }
                self.download_banner_clear();
                self.downloads_page_refresh();
            }
            bd::Msg::Remove(id) => {
                if self.browser_downloads.get(&id).is_some_and(|d| d.phase != bd::Phase::Running) {
                    self.browser_downloads.remove(&id);
                    self.downloads_page_refresh();
                }
            }
            // Selects the file in Explorer; nothing runs it, whatever its type.
            bd::Msg::ShowInFolder(id) => {
                let Some(path) = self.browser_downloads.get(&id).filter(|d| d.phase == bd::Phase::Finished).map(|d| d.path.to_string_lossy().replace('/', "\\")) else { return };
                self.reveal_in_explorer(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn src() -> &'static str {
        include_str!("browser_download_app.rs").split("#[cfg(test)]").next().unwrap()
    }

    /// The decision order: Dev Null, caps and repeat before any name or folder; the name is checked
    /// (destination) before the answer; the file is written hidden and finished by the one finisher.
    #[test]
    fn a_download_is_judged_then_written_hidden_then_finished_once() {
        let s = src();
        let v = s.find("bd::verdict(").unwrap();
        let dir = s.find("self.downloads_folder()").unwrap();
        let dest = s.find("bd::destination(&dir, url").unwrap();
        let answer = s.find("self.download_answer(pane, token, id, Some(&temp))").unwrap();
        assert!(v < dir && dir < dest && dest < answer);
        assert!(s.contains("bd::temp_path(&dir"));
        assert!(s.contains("ssh_local::finish_no_replace(&d.temp, &dest, &d.url") && !s.contains("finish_with("), "the move never replaces");
        assert!(s.contains("inert_files::mark_strict(p, bd::mark_url(u).as_deref())"));
        assert!(!s.contains("download_quiet") && !s.contains("cooldown"), "no second flood owner");
        // Nothing here opens or runs a file: Explorer only selects.
        for banned in ["open_with_shell", "shell_verb", "Command::new", "ShellExecute", "run_external"] {
            assert!(!s.contains(banned), "{banned}");
        }
        // The page acts only from its own pane and source, and names a download by id.
        let m = &s[s.find("pub(super) fn browser_downloads_message").unwrap()..];
        assert!(m.contains("if url != bd::PAGE_URL || source != bd::PAGE_URL {\n            return;"));
    }

    #[test]
    fn the_wiring_is_complete() {
        let app = include_str!("browser_app.rs");
        assert!(app.contains("Event::DownloadStarting { .. } | Event::DownloadProgress { .. } | Event::DownloadDone { .. } | Event::DownloadRefused { .. } => self.browser_download_event(pane, e),"));
        assert!(app.contains("downloads: true,"));
        let scripts = include_str!("browser_scripts_app.rs");
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..scripts.find("// --- Setup ---").unwrap()];
        assert!(reset.contains("self.browser_downloads_view_gone(pane);"));
        let db = include_str!("browser_db_app.rs");
        assert!(db.contains("Reply::Downloads(msg) => self.browser_downloads_message(pane, source, msg),"));
    }
}
