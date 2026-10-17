//! Save Page As to a folder on an SSH host (TWEAKS 7b), the pure part: what the finished local copy
//! becomes on the host, and the upload as a state machine that never talks to a host itself.
//!
//! The page is saved first into a private staging folder by the one local saver (`page_saver`), so
//! every guarantee of that saver (caps, names, create-new writes) already holds for the tree this
//! module reads. Here the tree is judged again for the host (`plan`), and the upload is a list of
//! requests (`Req`) answered one at a time (`Reply`): the app owns the transport (`ssh_files`, the
//! one file owner of the session) and this module owns the order, the create-new rule and the
//! cleanup ledger. No Win32 and no network code.
//!
//! Rules (plan 2026-10-20-tw7b-save-page-to-ssh.md, "Decisions"):
//! - the destination folder is new: its name is probed with `stat` and made with `mkdir`, which the
//!   host refuses when the name exists; nothing already on the host is ever written or removed;
//! - a host keeps no Mark of the Web, so what stands in is inert names (`inert_files::neutralized`;
//!   a name that is still high risk is refused) and no mode the save sets: the host creates files
//!   with its umask default, which never carries an execute bit;
//! - the cleanup removes only what THIS save made (`Ledger`), deepest first, and a folder only
//!   while it is empty (the host's `rmdir` refuses otherwise).

use crate::page_saver;

/// The most entries (files plus the folders this save makes) the upload takes: exactly what the
/// saver can produce, `MAX_RESOURCES` resources, `index.html`, the page folder and `resources`.
pub const REMOTE_MAX_ENTRIES: usize = page_saver::MAX_RESOURCES + 3;
/// The most bytes (the saver's total) and the most in one file (the file owner's upload cap).
pub const REMOTE_MAX_BYTES: u64 = page_saver::MAX_TOTAL_BYTES;
pub const REMOTE_MAX_FILE: u64 = crate::ssh_files::UPLOAD_MAX;
/// Folder names tried for a free one: `name`, `name (2)` ... `name (100)`.
pub const NAME_TRIES: usize = 100;
/// What the confirm question says the host does not do.
pub const NO_WEB_MARK: &str = "The host will not mark the saved files as downloaded from the web.";

/// One entry of the staged tree: the path under the page folder with `/` between names ("" is the
/// page folder itself and is not listed), its size, and whether it is a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub rel: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub rel: String,
    pub size: u64,
}

/// What goes to the host: folders (parents first) and files (shallow first), names as the host gets them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub dirs: Vec<String>,
    pub files: Vec<PlanFile>,
    pub bytes: u64,
}

/// The name one component has on the host, or why it is refused. A name with a separator, a `..`,
/// a control character or no text is refused; a name Windows would run (the one list) is kept
/// under the saver's neutral suffix, and refused if it still looks like one.
pub fn remote_component(name: &str) -> Result<String, String> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) || name.chars().any(char::is_control);
    if bad {
        return Err(format!("The name \"{}\" cannot be saved on the host.", crate::inert_files::shown_name(name)));
    }
    let kept = crate::inert_files::neutralized(name);
    if crate::inert_files::high_risk(&kept) {
        return Err(format!("The name \"{}\" cannot be saved on the host.", crate::inert_files::shown_name(name)));
    }
    Ok(kept)
}

fn depth(rel: &str) -> usize {
    rel.split('/').count()
}

/// Judges the staged tree for the host. Refused over the caps (before anything is made), for an
/// entry whose parent is not a listed folder, for a name `remote_component` refuses, and for two
/// entries that end with one name.
pub fn plan(staged: &[Staged]) -> Result<Plan, String> {
    if staged.len() + 1 > REMOTE_MAX_ENTRIES {
        return Err(format!("The page has more than {} files and folders.", REMOTE_MAX_ENTRIES));
    }
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<PlanFile> = Vec::new();
    let mut bytes = 0u64;
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ordered: Vec<&Staged> = staged.iter().collect();
    ordered.sort_by(|a, b| depth(&a.rel).cmp(&depth(&b.rel)).then_with(|| a.rel.cmp(&b.rel)));
    let mut folders: std::collections::HashSet<String> = std::collections::HashSet::new();
    folders.insert(String::new());
    for e in ordered {
        let comps: Vec<String> = e.rel.split('/').map(remote_component).collect::<Result<_, _>>()?;
        let (name, parent) = comps.split_last().map(|(n, p)| (n.clone(), p.join("/"))).unwrap_or_default();
        if !folders.contains(&parent) {
            return Err(format!("The saved page is missing the folder of \"{}\".", crate::inert_files::shown_name(&e.rel)));
        }
        let rel = if parent.is_empty() { name } else { format!("{parent}/{name}") };
        if !known.insert(rel.clone()) {
            return Err(format!("Two saved files would have the name \"{}\" on the host.", crate::inert_files::shown_name(&rel)));
        }
        if e.is_dir {
            folders.insert(rel.clone());
            dirs.push(rel);
        } else {
            if e.size > REMOTE_MAX_FILE {
                return Err(format!("\"{}\" is too large to upload (the limit is 64 MiB).", crate::inert_files::shown_name(&rel)));
            }
            bytes = bytes.saturating_add(e.size);
            files.push(PlanFile { rel, size: e.size });
        }
    }
    if bytes > REMOTE_MAX_BYTES {
        return Err("The page is larger than 256 MiB.".to_string());
    }
    Ok(Plan { dirs, files, bytes })
}

/// A path this save made on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: String,
    pub dir: bool,
}

/// Every path this save made, in the order made. The cleanup removes exactly these.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    made: Vec<Created>,
}

impl Ledger {
    pub fn record(&mut self, path: &str, dir: bool) {
        if !self.made.iter().any(|c| c.path == path) {
            self.made.push(Created { path: path.to_string(), dir });
        }
    }

    /// Deepest first, so a file goes before its folder and a folder before its parent.
    pub fn undo_order(&self) -> Vec<Created> {
        let mut v = self.made.clone();
        v.sort_by_key(|c| std::cmp::Reverse(c.path.matches('/').count()));
        v
    }

    pub fn clear(&mut self) {
        self.made.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.made.is_empty()
    }
}

/// What the upload asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Req {
    /// `stat` the path (the answer is `Reply::Stat`).
    Stat(String),
    /// `mkdir` the path (`Reply::Made`).
    Mkdir(String),
    /// Upload the staged file `rel` to the host path `dest` (`Reply::Put`).
    Put { rel: String, dest: String, size: u64 },
    /// Stop the upload in flight (it removes its own hidden temp), no answer.
    Abort,
    /// `rm` the path without recursion (a file, or a folder only while empty), no answer.
    Rm(String),
    /// `done` of `total` files are up.
    Progress { done: usize, total: usize },
    /// The save is over: the folder it made on the host, or why it ended.
    Finished(Result<String, String>),
}

/// The owner's answers. `Stat` and `Made` carry the framer's status (0 is yes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Stat(i32),
    Made(i32),
    /// The upload ended: the name it landed under, or why it failed.
    Put(Result<String, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum St {
    Idle,
    Probing(usize),
    MakingDir(usize),
    Putting(usize),
    Over,
}

/// The upload as a state machine: one request out at a time.
#[derive(Debug)]
pub struct Upload {
    plan: Plan,
    parent: String,
    base: String,
    root: String,
    ledger: Ledger,
    st: St,
}

pub const NO_NAME: &str = "No free folder name was found on the host.";
pub const CANCELED: &str = "The upload was canceled.";

fn join(dir: &str, name: &str) -> String {
    crate::remote_file::posix_join(dir, name)
}

impl Upload {
    /// `parent` is the host folder the person chose; `base` the new folder's wanted name.
    pub fn new(plan: Plan, parent: &str, base: &str) -> Upload {
        Upload { plan, parent: parent.to_string(), base: base.to_string(), root: String::new(), ledger: Ledger::default(), st: St::Idle }
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn is_over(&self) -> bool {
        self.st == St::Over
    }

    fn name_try(&self, n: usize) -> String {
        if n == 0 { self.base.clone() } else { format!("{} ({})", self.base, n + 1) }
    }

    pub fn start(&mut self) -> Vec<Req> {
        if self.st != St::Idle {
            return Vec::new();
        }
        self.st = St::Probing(0);
        vec![Req::Stat(join(&self.parent, &self.name_try(0)))]
    }

    fn fail(&mut self, why: String) -> Vec<Req> {
        let mut out = self.undo();
        self.st = St::Over;
        out.push(Req::Finished(Err(why)));
        out
    }

    fn undo(&mut self) -> Vec<Req> {
        let out = self.ledger.undo_order().into_iter().map(|c| Req::Rm(c.path)).collect();
        self.ledger.clear();
        out
    }

    fn next_dir(&mut self, i: usize) -> Vec<Req> {
        match self.plan.dirs.get(i) {
            Some(d) => {
                self.st = St::MakingDir(i);
                vec![Req::Mkdir(join(&self.root, d))]
            }
            None => self.next_put(0),
        }
    }

    fn next_put(&mut self, i: usize) -> Vec<Req> {
        match self.plan.files.get(i) {
            Some(f) => {
                self.st = St::Putting(i);
                vec![Req::Put { rel: f.rel.clone(), dest: join(&self.root, &f.rel), size: f.size }]
            }
            None => {
                self.st = St::Over;
                self.ledger.clear();
                vec![Req::Finished(Ok(self.root.clone()))]
            }
        }
    }

    pub fn reply(&mut self, r: Reply) -> Vec<Req> {
        match (self.st.clone(), r) {
            (St::Probing(n), Reply::Stat(status)) => {
                if status < 0 {
                    return self.fail(crate::ssh_files::FileError::ConnectionClosed.text().to_string());
                }
                if status > 0 {
                    // Free (a failed stat counts as free, as the file owner's own name probe does).
                    self.root = join(&self.parent, &self.name_try(n));
                    self.st = St::MakingDir(usize::MAX);
                    return vec![Req::Mkdir(self.root.clone())];
                }
                if n + 1 >= NAME_TRIES {
                    return self.fail(NO_NAME.to_string());
                }
                self.st = St::Probing(n + 1);
                vec![Req::Stat(join(&self.parent, &self.name_try(n + 1)))]
            }
            (St::MakingDir(i), Reply::Made(status)) => {
                if status != 0 {
                    let why = if status < 0 { crate::ssh_files::FileError::ConnectionClosed.text().to_string() } else { format!("The folder \"{}\" could not be made on the host.", crate::inert_files::shown_name(&self.root)) };
                    return self.fail(why);
                }
                if i == usize::MAX {
                    self.ledger.record(&self.root, true);
                    return self.next_dir(0);
                }
                let path = join(&self.root, &self.plan.dirs[i]);
                self.ledger.record(&path, true);
                self.next_dir(i + 1)
            }
            (St::Putting(i), Reply::Put(result)) => {
                let want = join(&self.root, &self.plan.files[i].rel);
                match result {
                    Err(why) => self.fail(why),
                    Ok(landed) if landed != want => {
                        // The host already had that name (another program wrote it meanwhile): the
                        // file owner kept both, so the extra copy is ours to remove, and the save stops.
                        self.ledger.record(&landed, false);
                        self.fail(format!("\"{}\" already exists on the host.", crate::inert_files::shown_name(&self.plan.files[i].rel)))
                    }
                    Ok(landed) => {
                        self.ledger.record(&landed, false);
                        let mut out = vec![Req::Progress { done: i + 1, total: self.plan.files.len() }];
                        out.extend(self.next_put(i + 1));
                        out
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    /// The person cancelled, the pane closed or the connection dropped: stop, remove what this
    /// save made (and what its in-flight step may have made), report once.
    pub fn cancel(&mut self, why: &str) -> Vec<Req> {
        let mut out = Vec::new();
        match self.st.clone() {
            St::Over => return Vec::new(),
            St::Idle | St::Probing(_) => {}
            St::MakingDir(i) => {
                // The mkdir may have happened with its answer still on the way.
                let path = if i == usize::MAX { self.root.clone() } else { join(&self.root, &self.plan.dirs[i]) };
                self.ledger.record(&path, true);
            }
            St::Putting(i) => {
                out.push(Req::Abort);
                self.ledger.record(&join(&self.root, &self.plan.files[i].rel), false);
            }
        }
        out.extend(self.undo());
        self.st = St::Over;
        out.push(Req::Finished(Err(why.to_string())));
        out
    }
}

/// The question before the upload: what, to where, and what the host does not do.
pub fn confirm_text(page: &str, to_user_at_host: &str, folder: &str) -> String {
    format!("OK to save the page\n{page}\nto\n{to_user_at_host}:{folder}?\n\n{NO_WEB_MARK}")
}

/// The wanted name of the new folder on the host: the saver's folder name for the page.
pub fn folder_base(url: &str) -> String {
    page_saver::default_name(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn d(rel: &str) -> Staged {
        Staged { rel: rel.into(), size: 0, is_dir: true }
    }
    fn f(rel: &str, size: u64) -> Staged {
        Staged { rel: rel.into(), size, is_dir: false }
    }

    /// A host that refuses what the real one refuses: `stat` of a missing path is 2; `mkdir` of an
    /// existing path, or one with no parent, fails (100); `rm` of a missing path or a folder that
    /// is not empty fails; an upload onto an existing name lands under `name (2)`, as the file
    /// owner's name probe does, and into a missing folder fails.
    #[derive(Default)]
    struct Host {
        nodes: BTreeMap<String, bool>,
        log: Vec<String>,
    }

    impl Host {
        fn with(paths: &[(&str, bool)]) -> Host {
            Host { nodes: paths.iter().map(|(p, d)| (p.to_string(), *d)).collect(), log: Vec::new() }
        }
        fn parent_ok(&self, p: &str) -> bool {
            let par = crate::remote_file::posix_parent(p);
            par == "/" || self.nodes.get(&par) == Some(&true)
        }
        fn apply(&mut self, r: &Req) -> Option<Reply> {
            match r {
                Req::Stat(p) => Some(Reply::Stat(if self.nodes.contains_key(p) { 0 } else { 2 })),
                Req::Mkdir(p) => {
                    self.log.push(format!("mkdir {p}"));
                    if self.nodes.contains_key(p) || !self.parent_ok(p) {
                        return Some(Reply::Made(100));
                    }
                    self.nodes.insert(p.clone(), true);
                    Some(Reply::Made(0))
                }
                Req::Put { dest, .. } => {
                    self.log.push(format!("put {dest}"));
                    if !self.parent_ok(dest) {
                        return Some(Reply::Put(Err("File not found".into())));
                    }
                    let mut name = dest.clone();
                    let mut n = 1;
                    while self.nodes.contains_key(&name) {
                        n += 1;
                        name = format!("{dest} ({n})");
                    }
                    self.nodes.insert(name.clone(), false);
                    Some(Reply::Put(Ok(name)))
                }
                Req::Rm(p) => {
                    self.log.push(format!("rm {p}"));
                    let has_kids = self.nodes.keys().any(|k| k.starts_with(&format!("{p}/")));
                    if self.nodes.contains_key(p) && !has_kids {
                        self.nodes.remove(p);
                    }
                    None
                }
                Req::Abort | Req::Progress { .. } | Req::Finished(_) => None,
            }
        }
    }

    /// Runs the upload against the host until it finishes or `stop_after` requests were answered.
    fn run(up: &mut Upload, host: &mut Host, stop_after: usize) -> (Vec<Req>, Option<Result<String, String>>) {
        let mut seen = Vec::new();
        let mut todo: std::collections::VecDeque<Req> = up.start().into();
        let mut answered = 0;
        let mut end = None;
        while let Some(r) = todo.pop_front() {
            seen.push(r.clone());
            if let Req::Finished(x) = &r {
                end = Some(x.clone());
            }
            if answered >= stop_after {
                continue;
            }
            if let Some(reply) = host.apply(&r) {
                answered += 1;
                todo.extend(up.reply(reply));
            }
        }
        (seen, end)
    }

    fn tree() -> Vec<Staged> {
        vec![f("index.html", 10), d("resources"), f("resources/a.css", 5), f("resources/b.png", 7)]
    }

    #[test]
    fn remote_names_equal_local_names() {
        let p = plan(&tree()).unwrap();
        assert_eq!(p.dirs, ["resources"]);
        assert_eq!(p.files.iter().map(|x| x.rel.as_str()).collect::<Vec<_>>(), ["index.html", "resources/a.css", "resources/b.png"]);
        assert_eq!(p.bytes, 22);
        // Names the saver made from hostile page names stay what they are.
        for url in ["https://x.test/a.js", "https://x.test/setup.exe", "https://x.test/CON", "https://x.test/a%3Ab"] {
            let local = page_saver::base_filename(url, None);
            let s = vec![d("resources"), f(&format!("resources/{local}"), 1)];
            assert_eq!(plan(&s).unwrap().files[0].rel, format!("resources/{local}"), "{url}");
        }
        // The saver's own folder name for a page is the base of the remote folder.
        assert_eq!(folder_base("https://example.com/x"), page_saver::default_name("https://example.com/x"));
        // A name the saver never leaves runnable is neutralized the same way if one slipped through.
        assert_eq!(plan(&[f("setup.exe", 1)]).unwrap().files[0].rel, "setup.exe.download");
    }

    #[test]
    fn remote_caps_match_the_saver() {
        assert_eq!(REMOTE_MAX_ENTRIES, 2003);
        assert_eq!(REMOTE_MAX_BYTES, 256 * 1024 * 1024);
        // The biggest page the saver makes: 2000 resources, index.html, `resources` (and the page folder).
        let mut max = vec![f("index.html", 1), d("resources")];
        max.extend((0..page_saver::MAX_RESOURCES).map(|i| f(&format!("resources/r{i}.txt"), 1)));
        assert_eq!(plan(&max).unwrap().files.len(), 2001);
        max.push(f("resources/extra.txt", 1));
        assert!(plan(&max).is_err(), "one entry past the cap is refused");
        // Bytes: exactly the total passes, one more does not (files of the 8 MiB resource cap).
        let per = page_saver::MAX_RESOURCE_BYTES;
        let n = (REMOTE_MAX_BYTES / per) as usize;
        let mut sized: Vec<Staged> = (0..n).map(|i| f(&format!("f{i}.bin"), per)).collect();
        assert!(plan(&sized).is_ok());
        sized.push(f("last.bin", 1));
        assert!(plan(&sized).is_err());
        assert!(plan(&[f("big.bin", REMOTE_MAX_FILE + 1)]).is_err());
        // The cap is judged before anything is made: the planner has no host.
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        assert_eq!(up.start(), [Req::Stat("/home/u/page".into())], "a stat, never a mkdir, comes first");
    }

    #[test]
    fn hostile_names_stay_data() {
        for bad in ["", ".", "..", "a/b", "a\\b", "x\ny", "x\u{7}y", "a/../b"] {
            assert!(plan(&[f(bad, 1)]).is_err(), "{bad:?}");
        }
        // Names with shell characters are data: they plan as that exact name, on the typed requests.
        for odd in ["a; rm -rf ~", "$(x)", "`id`", "a b", "'q'", "x|y&z"] {
            let p = plan(&[f(odd, 1)]).unwrap();
            assert_eq!(p.files[0].rel, odd);
            let mut up = Upload::new(p, "/home/u", "page");
            let mut host = Host::with(&[("/home/u", true)]);
            let (seen, end) = run(&mut up, &mut host, usize::MAX);
            assert!(end.unwrap().is_ok());
            assert!(seen.contains(&Req::Put { rel: odd.into(), dest: format!("/home/u/page/{odd}"), size: 1 }));
            assert!(host.nodes.contains_key(&format!("/home/u/page/{odd}")));
        }
        // A stream form or a runnable name that the suffix cannot save is refused.
        assert!(plan(&[f("a.exe:evil", 1)]).is_err());
        // Duplicates after neutralizing are refused.
        assert!(plan(&[f("a.exe", 1), f("a.exe.download", 1)]).is_err());
        // An entry whose folder is not listed is refused.
        assert!(plan(&[f("nope/a.txt", 1)]).is_err());
    }

    #[test]
    fn remote_files_are_inert() {
        let p = plan(&[f("run.bat", 1), f("x.ps1", 1), f("y.js", 1), f("ok.html", 1)]).unwrap();
        let names: Vec<&str> = p.files.iter().map(|x| x.rel.as_str()).collect();
        assert!(names.contains(&"run.bat.download") && names.contains(&"x.ps1.download") && names.contains(&"y.js.download") && names.contains(&"ok.html"), "{names:?}");
        assert!(p.files.iter().all(|x| !crate::inert_files::high_risk(&x.rel)));
        // The confirm text says what the host does not do, and shows the host and folder it is given.
        let t = confirm_text("example.com", "u@h", "/home/u");
        assert!(t.contains(NO_WEB_MARK) && t.contains("u@h:/home/u") && t.contains("example.com"));
        assert!(NO_WEB_MARK.contains("will not mark") && NO_WEB_MARK.contains("web"));
    }

    #[test]
    fn upload_never_replaces() {
        // The folder name is probed: `page` and `page (2)` exist, so `page (3)` is made.
        let mut host = Host::with(&[("/home/u", true), ("/home/u/page", true), ("/home/u/page (2)", true)]);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let (_, end) = run(&mut up, &mut host, usize::MAX);
        assert_eq!(end.unwrap().unwrap(), "/home/u/page (3)");
        assert!(host.nodes.contains_key("/home/u/page (3)/resources/b.png"));
        assert!(!host.nodes.keys().any(|k| k.starts_with("/home/u/page/")), "the existing folder is untouched");
        // A file that appears at its name meanwhile stops the save, which removes everything it made,
        // the extra copy included, and the other program's file stays.
        let mut host = Host::with(&[("/home/u", true)]);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let mut todo: std::collections::VecDeque<Req> = up.start().into();
        let mut end = None;
        while let Some(r) = todo.pop_front() {
            if matches!(&r, Req::Put { rel, .. } if rel == "resources/a.css") {
                host.nodes.insert("/home/u/page/resources/a.css".into(), false);
            }
            if let Req::Finished(x) = &r {
                end = Some(x.clone());
            }
            if let Some(reply) = host.apply(&r) {
                todo.extend(up.reply(reply));
            }
        }
        let why = end.unwrap().unwrap_err();
        assert!(why.contains("already exists"), "{why}");
        assert!(host.nodes.contains_key("/home/u/page/resources/a.css"), "the other program's file is not removed");
        assert!(!host.nodes.contains_key("/home/u/page/resources/a.css (2)") && !host.nodes.contains_key("/home/u/page/index.html"), "everything this save made is gone");
        // 100 taken names end the probe.
        let mut taken = vec![("/home/u".to_string(), true), ("/home/u/page".to_string(), true)];
        taken.extend((2..=100).map(|n| (format!("/home/u/page ({n})"), true)));
        let mut host = Host { nodes: taken.into_iter().collect(), log: Vec::new() };
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let (_, end) = run(&mut up, &mut host, usize::MAX);
        assert_eq!(end.unwrap().unwrap_err(), NO_NAME);
        assert!(host.log.is_empty(), "no mkdir was tried");
    }

    #[test]
    fn cleanup_removes_only_ledger() {
        // The host already has /home/u/keep (a folder with a file) and the chosen folder is /home/u.
        let before = [("/home/u", true), ("/home/u/keep", true), ("/home/u/keep/x.txt", false)];
        // Cancel after the folders and the first file are up.
        let mut host = Host::with(&before);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let (seen, end) = run(&mut up, &mut host, 4);
        assert!(end.is_none(), "stopped before the end: {seen:?}");
        let out = up.cancel("stop");
        for r in &out {
            host.apply(r);
        }
        assert!(matches!(out.last(), Some(Req::Finished(Err(m))) if m == "stop"));
        assert_eq!(host.nodes.keys().map(String::as_str).collect::<Vec<_>>(), ["/home/u", "/home/u/keep", "/home/u/keep/x.txt"], "back to what was there");
        // Deepest first: files before the folder that holds them, that folder before the page folder.
        let rms: Vec<&str> = out.iter().filter_map(|r| if let Req::Rm(p) = r { Some(p.as_str()) } else { None }).collect();
        let pos = |p: &str| rms.iter().position(|x| *x == p).unwrap_or_else(|| panic!("{p} not removed in {rms:?}"));
        assert!(pos("/home/u/page/resources/a.css") < pos("/home/u/page/resources"));
        assert!(pos("/home/u/page/resources") < pos("/home/u/page"));
        assert!(!rms.iter().any(|p| p.starts_with("/home/u/keep")), "{rms:?}");
        // A folder that is not empty (somebody put a file in) is refused by the host and stays.
        let mut host = Host::with(&before);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        run(&mut up, &mut host, 3);
        host.nodes.insert("/home/u/page/theirs.txt".into(), false);
        for r in up.cancel("stop") {
            host.apply(&r);
        }
        assert!(host.nodes.contains_key("/home/u/page") && host.nodes.contains_key("/home/u/page/theirs.txt"), "a folder with their file stays");
        assert!(!host.nodes.contains_key("/home/u/page/resources"), "what this save made in it is gone");
        // A cancel while the first mkdir's answer is on the way still removes the folder.
        let mut host = Host::with(&before);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let first = up.start();
        let stat = host.apply(&first[0]).unwrap();
        let mk = up.reply(stat);
        host.apply(&mk[0]);
        for r in up.cancel("stop") {
            host.apply(&r);
        }
        assert!(!host.nodes.contains_key("/home/u/page"));
        // A finished upload clears the ledger; a later cancel does nothing.
        let mut host = Host::with(&before);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        let (_, end) = run(&mut up, &mut host, usize::MAX);
        assert!(end.unwrap().is_ok() && up.ledger().is_empty() && up.cancel("late").is_empty());
        // A failed step removes what was made and reports once.
        let mut host = Host::with(&before);
        let mut up = Upload::new(plan(&tree()).unwrap(), "/home/u", "page");
        run(&mut up, &mut host, 3);
        let out = up.reply(Reply::Put(Err("Connection closed".into())));
        assert!(matches!(out.last(), Some(Req::Finished(Err(m))) if m == "Connection closed"));
        assert!(up.reply(Reply::Put(Ok("x".into()))).is_empty(), "late replies are ignored");
    }

    #[test]
    fn ledger_order_is_deepest_first() {
        let mut l = Ledger::default();
        l.record("/a", true);
        l.record("/a/b", true);
        l.record("/a/b/c.txt", false);
        l.record("/a/b/c.txt", false);
        assert_eq!(l.undo_order().iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), ["/a/b/c.txt", "/a/b", "/a"]);
    }

    /// What the decisions rely on is what the bundled framer does (read from its source, since the repo
    /// has no live file-transfer test and the framer only runs on the host): `mkdir` is `os.mkdir`
    /// (fails on an existing path), a non-recursive `rm` of a folder is `os.rmdir` (fails unless
    /// empty), `create` opens "wb" (so a temp name must be new), `mv` is `shutil.move` (replaces).
    #[test]
    fn the_framer_does_what_the_save_relies_on() {
        let f = crate::framer_script::FRAMER;
        let body = |name: &str| {
            let at = f.find(&format!("async def {name}(")).unwrap_or_else(|| panic!("{name}"));
            let rest = &f[at + 1..];
            &f[at..at + 1 + rest.find("\nasync def ").unwrap_or(rest.len())]
        };
        assert!(body("handle_file_mkdir").contains("os.mkdir(path)"));
        let rm = body("handle_file_rm");
        assert!(rm.contains("os.rmdir(path)") && rm.contains("shutil.rmtree(path)"), "rmtree only for rm -r, which the save never sends");
        assert!(body("handle_file_create").contains("open(path, \"wb\")"));
        assert!(body("handle_file_mv").contains("shutil.move(source, dest)"));
        assert!(body("handle_file_append").contains("open(path, \"ab\")"));
    }
}
