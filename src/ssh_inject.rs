//! Remote shell integration over the SSH conductor: iTerm2's ShellIntegrationInjector for
//! remote shells (`SII:131-149`, `SII:263-439`), the bundled remote files (unmodified,
//! ruling 2; `SII:151-178`) and the payload tarballs (`TJ`, `ConductorPayloadBuilder.swift`,
//! `CD:1590-1604`).

use std::path::{Path, PathBuf};

/// Where the shell integration files go, relative to the remote home.
pub const INTEGRATION_SUBDIR: &str = ".iterm2/shell-integration";
/// The destination the injection payloads are written to (`CD:2356-2360`).
pub const PAYLOAD_BASE: &str = "/$HOME/.iterm2/shell-integration";

/// A bundled remote file and the folder under `PAYLOAD_BASE` it goes to.
pub struct Embedded {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub subdir: &'static str,
}

macro_rules! embedded {
    ($name:literal, $subdir:literal) => {
        Embedded { name: $name, bytes: include_bytes!(concat!("../resources/ssh/shell_integration/", $name)), subdir: $subdir }
    };
}

/// iTerm2's files at `64791fc` (`SII:151-178`), in its listing order.
pub static FILES: [Embedded; 7] = [
    embedded!("iterm2_shell_integration.bash", ""),
    embedded!("iterm2_shell_integration.zsh", ""),
    embedded!("iterm2_shell_integration.fish", ""),
    embedded!("iterm2_shell_integration.xonsh", ""),
    embedded!("bash-si-loader", ""),
    embedded!(".zshenv", ""),
    embedded!("iterm2-shell-integration-loader.fish", "fish/vendor_conf.d"),
];

/// The local folder every embedded file is said to live in, as iTerm2's flat bundle
/// Resources hold all seven side by side.
const RESOURCES: &str = "tandem-resources";

/// shellSupportsInjection (`CD:2083-2098`), given the shell's last path component and the
/// version lines getshell reported.
pub fn shell_supports_injection(shell_last: &str, version: &str) -> bool {
    if ["fish", "xonsh", "zsh"].contains(&last_component(shell_last)) {
        return true;
    }
    if shell_last == "bash" {
        // macOS's bash has no usable --posix.
        return !(version.contains("GNU bash, version 3.2.57") && version.contains("apple-darwin"));
    }
    false
}

/// NSString's lastPathComponent for a `/` path: the last non-empty piece.
pub fn last_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return path;
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// `dir` plus one component, without doubling a `/`.
fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

fn get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Sets `key` in place when present, else appends it.
fn set(env: &mut Vec<(String, String)>, key: &str, value: String) {
    match env.iter_mut().find(|(k, _)| k == key) {
        Some(pair) => pair.1 = value,
        None => env.push((key.to_string(), value)),
    }
}

fn remove(env: &mut Vec<(String, String)>, key: &str) {
    env.retain(|(k, _)| k != key);
}

/// modifyRemoteShellEnvironment (`SII:131-149`): the environment and arguments that load
/// shell integration from `dir` for the remote `shell`, with `argv` given and returned
/// without the shell itself. An unknown shell gets both back unchanged. `home` is the remote
/// home getshell reported, used for bash's HISTFILE (ruling 10, iTerm2 bug B10 fixed).
pub fn modify_remote(dir: &str, home: &str, env: &[(String, String)], shell: &str, argv: &[String]) -> (Vec<(String, String)>, Vec<String>) {
    // SII:203-217: the last component, lowercased, one leading "-" removed. The injector is
    // made with allowSystemBash true and no `login` special case: the shell is a remote path.
    let lowered = last_component(shell).to_lowercase();
    let name = lowered.strip_prefix('-').unwrap_or(&lowered);
    let mut env = env.to_vec();
    match name {
        "fish" => {
            // SII:263-280
            set(&mut env, "XDG_DATA_DIRS", dir.to_string());
            set(&mut env, "IT2_FISH_XDG_DATA_DIRS", dir.to_string());
            (env, argv.to_vec())
        }
        "xonsh" => {
            // SII:282-304
            let script = format!("{dir}/iterm2_shell_integration.xonsh");
            let value = match get(&env, "XONSHRC") {
                Some(existing) if !existing.is_empty() => format!("{script}:{existing}"),
                _ => script,
            };
            set(&mut env, "XONSHRC", value);
            (env, argv.to_vec())
        }
        "zsh" => {
            // SII:306-328
            match get(&env, "ZDOTDIR").map(str::to_string) {
                Some(old) => set(&mut env, "IT2_ORIG_ZDOTDIR", old),
                None => remove(&mut env, "IT2_ORIG_ZDOTDIR"),
            }
            set(&mut env, "ZDOTDIR", dir.to_string());
            set(&mut env, "ITERM_INJECT_SHELL_INTEGRATION", "1".to_string());
            (env, argv.to_vec())
        }
        "bash" => {
            let mut full = Vec::with_capacity(argv.len() + 1);
            full.push(shell.to_string());
            full.extend_from_slice(argv);
            if bash(dir, home, &mut env, &mut full) {
                (env, full.split_off(1))
            } else {
                (env, argv.to_vec())
            }
        }
        _ => (env, argv.to_vec()),
    }
}

/// BashShellIntegrationInjection.computeModified (`SII:330-439`) over `argv` with the shell
/// at index 0. False: a non-interactive bash, left alone (`env` and `argv` untouched: every
/// early return comes before the first change).
fn bash(dir: &str, home: &str, env: &mut Vec<(String, String)>, argv: &mut Vec<String>) -> bool {
    // IT2_BASH_INJECT's words in a fixed order (Decision: iTerm2 joins a Set).
    let (mut posix, mut no_rc, mut no_profile) = (false, false, false);
    let mut posix_env = String::new();
    let mut rc_file = String::new();
    let mut remove_args: Vec<usize> = Vec::new();
    let mut expecting_multi_chars_opt = true;
    let mut expecting_option_arg = false;
    let mut interactive_opt = false;
    let mut expecting_file_arg = false;
    let mut file_arg_set = false;

    for (i, arg) in argv.iter().enumerate().skip(1) {
        if expecting_file_arg {
            file_arg_set = true;
            break;
        }
        if expecting_option_arg {
            expecting_option_arg = false;
            continue;
        }
        if arg == "-" || arg == "--" {
            expecting_file_arg = true;
            continue;
        }
        let after_first = arg.chars().skip(1).collect::<String>();
        if !arg.is_empty() && !after_first.starts_with('-') && (arg.starts_with('-') || arg.starts_with("+O")) {
            expecting_multi_chars_opt = false;
            let mut options = arg.trim_start_matches(['-', '+']).to_string();
            if let Some((lhs, rhs)) = options.split_once('O') {
                // shopt
                if rhs.is_empty() {
                    expecting_option_arg = true;
                }
                options = lhs.to_string();
            }
            if options.contains('c') {
                // Non-interactive, `bash -ic` included.
                return false;
            }
            if options.contains('s') {
                // Reads from stdin, followed by args.
                break;
            }
            if options.contains('i') {
                interactive_opt = true;
            }
        } else if arg.starts_with("--") && expecting_multi_chars_opt {
            match arg.as_str() {
                "--posix" => {
                    posix = true;
                    posix_env = get(env, "ENV").unwrap_or("").to_string();
                }
                "--norc" => {
                    no_rc = true;
                    remove_args.push(i);
                }
                "--noprofile" => {
                    no_profile = true;
                    remove_args.push(i);
                }
                "--rcfile" | "--init-file" if i + 1 < argv.len() => {
                    expecting_option_arg = true;
                    rc_file = argv[i + 1].clone();
                    remove_args.push(i);
                    remove_args.push(i + 1);
                }
                _ => {}
            }
        } else {
            file_arg_set = true;
            break;
        }
    }
    if file_arg_set && !interactive_opt {
        return false;
    }
    let mut inject = vec!["1"];
    if posix {
        inject.push("posix");
    }
    if no_rc {
        inject.push("no-rc");
    }
    if no_profile {
        inject.push("no-profile");
    }
    set(env, "ENV", join(dir, "bash-si-loader"));
    set(env, "IT2_BASH_INJECT", inject.join(" "));
    if !posix_env.is_empty() {
        set(env, "IT2_BASH_POSIX_ENV", posix_env);
    }
    if !rc_file.is_empty() {
        set(env, "IT2_BASH_RCFILE", rc_file);
    }
    let mut i = 0;
    argv.retain(|_| {
        let keep = !remove_args.contains(&i);
        i += 1;
        keep
    });
    if get(env, "HISTFILE").is_none() && !posix {
        // In POSIX mode the default history file is ~/.sh_history. The remote home, never a
        // local path (ruling 10).
        set(env, "HISTFILE", join(home.trim_end_matches('/'), ".bash_history"));
        set(env, "IT2_BASH_UNEXPORT_HISTFILE", "1".to_string());
    }
    argv.insert(1, "--posix".to_string());
    true
}

/// Where a payload's bytes come from: a local file or folder, or one of `FILES`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Disk(PathBuf),
    Embedded(usize),
}

/// A file or folder to copy to the remote folder `destination` (a remote path that may
/// start with `/$HOME`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub source: Source,
    pub destination: String,
}

/// add(path:destination:)'s destination rule (`CD:1590-1604`): "~" or "~/" is the home, a
/// relative destination loses its first two characters (iTerm2 bug B8, kept), and trailing
/// slashes go (not from "/").
pub fn tweak_destination(dest: &str) -> String {
    let mut tweaked = if dest == "~/" || dest == "~" {
        "/$HOME".to_string()
    } else if !dest.starts_with('/') {
        format!("/$HOME/{}", dest.chars().skip(2).collect::<String>())
    } else {
        dest.to_string()
    };
    while tweaked != "/" && tweaked.ends_with('/') {
        tweaked.pop();
    }
    tweaked
}

/// A Files to Copy local path with a leading `~` meaning `local_home` (iTerm2's
/// expandingTildeInPath; Tandem's home is the local profile folder).
pub fn expand_local_tilde(path: &str, local_home: &Path) -> PathBuf {
    if path == "~" {
        return local_home.to_path_buf();
    }
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => rest.split(['/', '\\']).filter(|c| !c.is_empty()).fold(local_home.to_path_buf(), |p, c| p.join(c)),
        None => PathBuf::from(path),
    }
}

/// The seven shell integration files, to `PAYLOAD_BASE` and its subfolders (`CD:2356-2360`:
/// not through add(path:destination:)).
pub fn injection_payloads() -> Vec<Payload> {
    FILES
        .iter()
        .enumerate()
        .map(|(i, f)| Payload {
            source: Source::Embedded(i),
            destination: if f.subdir.is_empty() { PAYLOAD_BASE.to_string() } else { join(PAYLOAD_BASE, f.subdir) },
        })
        .collect()
}

/// A path as URL.pathComponents: "/" first, then each non-empty piece. A Windows path
/// `C:\Users\u\x` is `["/", "C:", "Users", "u", "x"]`.
fn components(path: &str) -> Vec<String> {
    std::iter::once("/".to_string()).chain(path.split(['/', '\\']).filter(|c| !c.is_empty()).map(str::to_string)).collect()
}

fn source_components(source: &Source) -> Vec<String> {
    match source {
        Source::Disk(p) => components(&p.to_string_lossy()),
        Source::Embedded(i) => vec!["/".to_string(), RESOURCES.to_string(), FILES[*i].name.to_string()],
    }
}

/// lengthOfLongestCommonPrefix over component lists (`Array+iTerm.swift:107-128`): one list
/// is its own length.
fn common_len(lists: &[Vec<String>]) -> usize {
    match lists {
        [] => 0,
        [one] => one.len(),
        [first, rest @ ..] => {
            let mut i = 0;
            while i < first.len() && rest.iter().all(|l| l.len() > i && l[i] == first[i]) {
                i += 1;
            }
            i
        }
    }
}

/// deletingLastPathComponent on components; "/" stays "/".
fn parent(path: &[String]) -> Vec<String> {
    path[..path.len().saturating_sub(1).max(1)].to_vec()
}

/// A group of payloads that one tarball carries (iTerm2's TarJob): local paths as
/// components, the local folder the archive names are relative to, and the remote folder
/// the archive is unpacked in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TarJob {
    pub sources: Vec<Vec<String>>,
    pub local_base: Vec<String>,
    pub destination: String,
}

impl TarJob {
    /// A job for one payload (`TJ:19-23`).
    fn new(local: Vec<String>, destination: &str) -> TarJob {
        TarJob { local_base: parent(&local), sources: vec![local], destination: destination.to_string() }
    }

    /// The archive names: each source after the local base, joined with "/" (`TJ:36-39`).
    pub fn entries(&self) -> Vec<String> {
        self.sources.iter().map(|s| s[self.local_base.len().min(s.len())..].join("/")).collect()
    }

    /// This job with `local` added to go into `destination_parent`, if it fits
    /// (`TJ:58-116`, ported as written, its "/" arithmetic included).
    fn adding(&self, local: &[String], destination_parent: &str) -> Option<TarJob> {
        let mut destination = components(destination_parent);
        destination.push(local.last()?.clone());
        let parents: Vec<Vec<String>> = self.sources.iter().map(|s| parent(s)).collect();
        if common_len(&parents) <= 1 {
            return None;
        }
        let mut all_parents = parents;
        all_parents.push(parent(local));
        let source_prefix = all_parents[0][..common_len(&all_parents)].to_vec();
        // Sources that share only the root are on different drives: iTerm2 would group them
        // under "/" and unpack them in folders named for the drives (bug B16, fixed).
        if source_prefix.len() <= 1 {
            return None;
        }
        let base = components(&self.destination);
        let mut destinations = Vec::with_capacity(self.sources.len() + 1);
        for s in &self.sources {
            // pathByRemovingPrefix throws when the source is not under the local base.
            if !s.starts_with(&self.local_base) {
                return None;
            }
            let mut d = base.clone();
            d.extend_from_slice(&s[self.local_base.len()..]);
            destinations.push(d);
        }
        destinations.push(destination);
        let destination_prefix_count = common_len(&destinations);
        // The "/"-separated pieces of the prefix path: "/" counts 2, "/a" 2, "/a/b" 3.
        let source_prefix_count = source_prefix.len().max(2);
        let mut all_sources = self.sources.clone();
        all_sources.push(local.to_vec());
        for (source, dest) in all_sources.iter().zip(&destinations) {
            let suffix = &source[source_prefix_count.min(source.len())..];
            if !dest.ends_with(suffix) {
                return None;
            }
            if dest.len() - suffix.len() > destination_prefix_count {
                return None;
            }
        }
        // iTerm2 builds a relative path here and relies on its working directory being "/"
        // (`TJ:111`); Tandem makes it absolute.
        // Every destination has "/" and a name, so it holds at least two components.
        let first = &destinations[0];
        let drop = all_sources[0].len().saturating_sub(source_prefix_count);
        let kept = &first[1..first.len().saturating_sub(drop).max(1)];
        Some(TarJob { sources: all_sources, local_base: source_prefix, destination: format!("/{}", kept.join("/")) })
    }
}

/// ConductorPayloadBuilder.add (`ConductorPayloadBuilder.swift:11-19`): each payload joins
/// the first job that takes it, else starts its own. Each job comes with its sources in
/// the job's order.
pub fn group(payloads: &[Payload]) -> Vec<(TarJob, Vec<Source>)> {
    let mut jobs: Vec<(TarJob, Vec<Source>)> = Vec::new();
    'payloads: for p in payloads {
        let local = source_components(&p.source);
        for (job, sources) in jobs.iter_mut() {
            if let Some(next) = job.adding(&local, &p.destination) {
                *job = next;
                sources.push(p.source.clone());
                continue 'payloads;
            }
        }
        jobs.push((TarJob::new(local, &p.destination), vec![p.source.clone()]));
    }
    jobs
}

/// One archive entry with an explicit header: files 0o644, folders 0o755 (named with a
/// trailing "/"), mtime 0, owner 0.
fn put<W: std::io::Write>(tar: &mut tar::Builder<W>, name: &str, data: Option<&[u8]>) -> Result<(), String> {
    let folder = data.is_none();
    let mut header = tar::Header::new_gnu();
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    let data = match data {
        Some(bytes) => {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            bytes
        }
        None => {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            &[][..]
        }
    };
    let full = if folder { format!("{name}/") } else { name.to_string() };
    let slot = &mut header.as_old_mut().name;
    if full.len() < slot.len() {
        slot[..full.len()].copy_from_slice(full.as_bytes());
        header.set_cksum();
        tar.append(&header, data).map_err(|e| format!("{name}: {e}"))
    } else {
        // A long name goes through the GNU long-name record (a folder's "/" is dropped there).
        tar.append_data(&mut header, name, data).map_err(|e| format!("{name}: {e}"))
    }
}

/// The most file data one tarball carries (controller ruling: the payload crosses the
/// terminal in base64, so more is not practical).
pub const MAX_JOB_BYTES: u64 = 64 << 20;
/// The most entries (files and folders) one tarball carries.
pub const MAX_JOB_ENTRIES: usize = 10_000;
/// The most links followed while resolving one Files to Copy path.
const MAX_LINK_HOPS: usize = 40;

/// What a tarball may still take. Checked before anything is read, so a job over the cap
/// fails without its data in memory.
#[derive(Clone, Copy, Debug)]
struct Budget {
    bytes: u64,
    entries: usize,
}

impl Budget {
    fn take(&mut self, bytes: u64, what: &str) -> Result<(), String> {
        if self.entries == 0 {
            return Err(format!("{what}: more than {MAX_JOB_ENTRIES} entries"));
        }
        if bytes > self.bytes {
            return Err(format!("{what}: more than 64 MiB of files"));
        }
        self.entries -= 1;
        self.bytes -= bytes;
        Ok(())
    }
}

/// A junction's or link's target as a local path, or None for a network one. Windows
/// reports a junction's target in the verbatim form (`\\?\C:\x`), which is local.
fn local_link_target(target: &str) -> Option<&str> {
    for verbatim in [r"\\?\", r"\??\"] {
        if let Some(rest) = target.strip_prefix(verbatim) {
            return crate::prompted::local_path_ok(rest).then_some(rest);
        }
    }
    (!crate::prompted::network_path(target)).then_some(target)
}

/// `path` with every link in it replaced by its target, one component at a time from the
/// drive down, so a link (at the top or in a parent folder, directly or through a chain)
/// whose target is a network path is refused before anything on that path is touched.
/// `link_target` gives a path's target when the path itself is a link.
fn resolve_links(path: &Path, link_target: &mut dyn FnMut(&Path) -> Result<Option<PathBuf>, String>) -> Result<PathBuf, String> {
    use std::path::Component;
    let mut out = PathBuf::new();
    let mut rest: Vec<PathBuf> = path.components().rev().map(|c| PathBuf::from(c.as_os_str())).collect();
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        match part.components().next() {
            Some(Component::Normal(_)) => out.push(&part),
            Some(Component::ParentDir) => {
                out.pop();
                continue;
            }
            Some(Component::Prefix(_)) | Some(Component::RootDir) => {
                out.push(&part);
                continue;
            }
            Some(Component::CurDir) | None => continue,
        }
        let Some(target) = link_target(&out)? else { continue };
        hops += 1;
        if hops > MAX_LINK_HOPS {
            return Err(format!("{}: too many links", path.display()));
        }
        let target = target.to_string_lossy().into_owned();
        let local = local_link_target(&target).ok_or_else(|| format!("{}: a link to a network path is never followed", out.display()))?;
        out.pop();
        rest.extend(Path::new(local).components().rev().map(|c| PathBuf::from(c.as_os_str())));
    }
    Ok(out)
}

/// The target of `path` when it is a symbolic link or a junction (neither is followed).
fn disk_link_target(path: &Path) -> Result<Option<PathBuf>, String> {
    let local = crate::localfs::judge_io(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = crate::localfs::lstat(&local).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_link {
        return Ok(None);
    }
    crate::localfs::read_link(&local).map(Some).ok_or_else(|| format!("{}: the link cannot be read", path.display()))
}

/// A local file, or a folder with everything under it (symbolic links inside a folder are
/// skipped, Decision). `path` holds no links (`resolve_links`).
fn put_disk<W: std::io::Write>(tar: &mut tar::Builder<W>, name: &str, path: &Path, budget: &mut Budget) -> Result<(), String> {
    use std::io::Read;
    let local = crate::localfs::judge_io(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = crate::localfs::stat(&local).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_dir {
        budget.take(meta.len, &path.display().to_string())?;
        // A file that grew since its size was read stops at the budget's edge.
        let file = crate::localfs::open(&local).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut bytes = Vec::with_capacity(meta.len as usize);
        file.take(meta.len + budget.bytes + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let grown = (bytes.len() as u64).saturating_sub(meta.len);
        budget.bytes = budget.bytes.checked_sub(grown).ok_or_else(|| format!("{}: more than 64 MiB of files", path.display()))?;
        return put(tar, name, Some(&bytes));
    }
    budget.take(0, &path.display().to_string())?;
    put(tar, name, None)?;
    let mut children: Vec<(String, PathBuf)> = Vec::new();
    for entry in crate::localfs::entries(&local).map_err(|e| format!("{}: {e}", path.display()))? {
        if entry.stat.is_link {
            continue;
        }
        if children.len() >= budget.entries {
            return Err(format!("{}: more than {MAX_JOB_ENTRIES} entries", path.display()));
        }
        children.push((entry.name.to_string_lossy().into_owned(), entry.path().to_path_buf()));
    }
    children.sort();
    for (child, p) in children {
        put_disk(tar, &format!("{name}/{child}"), &p, budget)?;
    }
    Ok(())
}

/// The gzipped tar of a job's sources (`TJ:53-56`), named by `job.entries()`, within
/// `MAX_JOB_BYTES` and `MAX_JOB_ENTRIES`. Only an absolute local path is read, and never
/// through a link to a network path (an error, so the job is skipped).
pub fn tarball(sources: &[Source], job: &TarJob) -> Result<Vec<u8>, String> {
    tarball_within(sources, job, Budget { bytes: MAX_JOB_BYTES, entries: MAX_JOB_ENTRIES })
}

fn tarball_within(sources: &[Source], job: &TarJob, mut budget: Budget) -> Result<Vec<u8>, String> {
    let entries = job.entries();
    if entries.len() != sources.len() {
        return Err("sources do not match the job".into());
    }
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    for (source, name) in sources.iter().zip(&entries) {
        match source {
            Source::Embedded(i) => {
                let file = FILES.get(*i).ok_or("no such bundled file")?;
                budget.take(file.bytes.len() as u64, file.name)?;
                put(&mut tar, name, Some(file.bytes))?;
            }
            Source::Disk(p) => {
                // The one judge first (mapped and SUBST drives, links to shares on the way).
                if !crate::prompted::local_path_ok(&p.to_string_lossy()) || !crate::localfs::judged(p) {
                    return Err(format!("{}: only an absolute local path is read", p.display()));
                }
                let real = resolve_links(p, &mut disk_link_target)?;
                put_disk(&mut tar, name, &real, &mut budget)?;
            }
        }
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// Every job's tarball and its remote folder (enumeratePayloads,
/// `ConductorPayloadBuilder.swift:20-26`), and why each job that failed to pack was skipped
/// (for the pane, where the person sees it; iTerm2 only logs it).
pub fn tarballs(payloads: &[Payload]) -> (Vec<(Vec<u8>, String)>, Vec<String>) {
    let mut skipped = Vec::new();
    let packed = group(payloads)
        .into_iter()
        .filter_map(|(job, sources)| match tarball(&sources, &job) {
            Ok(t) => Some((t, job.destination)),
            Err(e) => {
                skipped.push(format!("A Files to Copy tarball for {} was skipped: {e}", job.destination));
                None
            }
        })
        .collect();
    (packed, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn embedded_files_are_iterm2s() {
        use sha2::{Digest, Sha256};
        let want = [
            ("iterm2_shell_integration.bash", 47053, "2ba74f411bc8e43e"),
            ("iterm2_shell_integration.zsh", 20739, "6826c8f0fedded72"),
            ("iterm2_shell_integration.fish", 22450, "c38b0938c5f64431"),
            ("iterm2_shell_integration.xonsh", 21541, "83fdfacb076028ca"),
            ("bash-si-loader", 2336, "0c1cd7ae5949b091"),
            (".zshenv", 1438, "9393d21a9772bdbe"),
            ("iterm2-shell-integration-loader.fish", 1528, "228a19f70f2928fd"),
        ];
        for (f, (name, len, sha)) in FILES.iter().zip(want) {
            assert_eq!(f.name, name);
            assert_eq!(f.bytes.len(), len, "{name}");
            let got: String = Sha256::digest(f.bytes).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(&got[..16], sha, "{name}");
            assert!(!f.bytes.windows(2).any(|w| w == b"\r\n"), "{name} has CR LF");
            assert_eq!(f.subdir, if name.ends_with("loader.fish") { "fish/vendor_conf.d" } else { "" });
        }
    }

    #[test]
    fn supports_injection_follows_iterm2() {
        for shell in ["fish", "xonsh", "zsh"] {
            assert!(shell_supports_injection(shell, ""), "{shell}");
        }
        assert!(shell_supports_injection("bash", "GNU bash, version 5.2.15(1)-release (x86_64-pc-linux-gnu)"));
        assert!(!shell_supports_injection("bash", "GNU bash, version 3.2.57(1)-release (arm64-apple-darwin23)"));
        for shell in ["tcsh", "sh", "-bash"] {
            assert!(!shell_supports_injection(shell, ""), "{shell}");
        }
    }

    const DIR: &str = "/home/u/.iterm2/shell-integration";

    #[test]
    fn zsh_fish_xonsh_follow_iterm2() {
        let argv = s(&["-l"]);
        let (env, args) = modify_remote(DIR, "/home/u", &pairs(&[("A", "b")]), "/bin/zsh", &argv);
        assert_eq!(env, pairs(&[("A", "b"), ("ZDOTDIR", DIR), ("ITERM_INJECT_SHELL_INTEGRATION", "1")]));
        assert_eq!(args, argv);
        let (env, _) = modify_remote(DIR, "/home/u", &pairs(&[("ZDOTDIR", "/z")]), "/bin/zsh", &argv);
        assert_eq!(env, pairs(&[("ZDOTDIR", DIR), ("IT2_ORIG_ZDOTDIR", "/z"), ("ITERM_INJECT_SHELL_INTEGRATION", "1")]));
        let (env, _) = modify_remote(DIR, "/home/u", &pairs(&[("IT2_ORIG_ZDOTDIR", "/old")]), "zsh", &argv);
        assert_eq!(env, pairs(&[("ZDOTDIR", DIR), ("ITERM_INJECT_SHELL_INTEGRATION", "1")]), "a stale IT2_ORIG_ZDOTDIR goes");
        for shell in ["/usr/local/bin/ZSH", "-zsh"] {
            let (env, _) = modify_remote(DIR, "/home/u", &[], shell, &[]);
            assert_eq!(get(&env, "ZDOTDIR"), Some(DIR), "{shell}");
        }

        let (env, args) = modify_remote(DIR, "/home/u", &[], "/usr/bin/fish", &argv);
        assert_eq!(env, pairs(&[("XDG_DATA_DIRS", DIR), ("IT2_FISH_XDG_DATA_DIRS", DIR)]));
        assert_eq!(args, argv);

        let script = format!("{DIR}/iterm2_shell_integration.xonsh");
        let (env, args) = modify_remote(DIR, "/home/u", &[], "/usr/bin/xonsh", &argv);
        assert_eq!(env, pairs(&[("XONSHRC", &script)]));
        assert_eq!(args, argv);
        let (env, _) = modify_remote(DIR, "/home/u", &pairs(&[("XONSHRC", "a")]), "xonsh", &[]);
        assert_eq!(env, pairs(&[("XONSHRC", &format!("{script}:a"))]));
        let (env, _) = modify_remote(DIR, "/home/u", &pairs(&[("XONSHRC", "")]), "xonsh", &[]);
        assert_eq!(env, pairs(&[("XONSHRC", &script)]), "an empty XONSHRC is replaced");

        let given = pairs(&[("A", "b")]);
        assert_eq!(modify_remote(DIR, "/home/u", &given, "/bin/tcsh", &argv), (given.clone(), argv.clone()));
    }

    fn bash(argv: &[&str], env: &[(&str, &str)]) -> (Vec<(String, String)>, Vec<String>) {
        modify_remote(DIR, "/home/u", &pairs(env), "/bin/bash", &s(argv))
    }

    #[test]
    fn bash_follows_iterm2() {
        let loader = format!("{DIR}/bash-si-loader");
        let (env, args) = bash(&[], &[]);
        assert_eq!(
            env,
            pairs(&[("ENV", &loader), ("IT2_BASH_INJECT", "1"), ("HISTFILE", "/home/u/.bash_history"), ("IT2_BASH_UNEXPORT_HISTFILE", "1")])
        );
        assert_eq!(args, s(&["--posix"]));

        let (env, args) = bash(&["--norc"], &[]);
        assert_eq!(get(&env, "IT2_BASH_INJECT"), Some("1 no-rc"));
        assert_eq!(args, s(&["--posix"]));
        let (env, args) = bash(&["--noprofile"], &[]);
        assert_eq!(get(&env, "IT2_BASH_INJECT"), Some("1 no-profile"));
        assert_eq!(args, s(&["--posix"]));

        let (env, args) = bash(&["--posix"], &[]);
        assert_eq!(env, pairs(&[("ENV", &loader), ("IT2_BASH_INJECT", "1 posix")]), "no HISTFILE, no IT2_BASH_POSIX_ENV");
        assert_eq!(args, s(&["--posix", "--posix"]), "iTerm2 keeps the person's --posix");
        let (env, _) = bash(&["--posix"], &[("ENV", "/e")]);
        assert_eq!(get(&env, "IT2_BASH_POSIX_ENV"), Some("/e"));
        assert_eq!(get(&env, "ENV"), Some(loader.as_str()));
        let (env, _) = bash(&["--posix"], &[("ENV", "")]);
        assert_eq!(get(&env, "IT2_BASH_POSIX_ENV"), None, "posixEnv.isEmpty");

        let (env, args) = bash(&["--rcfile", "/r"], &[]);
        assert_eq!(get(&env, "IT2_BASH_RCFILE"), Some("/r"));
        assert_eq!(args, s(&["--posix"]));
        let (env, args) = bash(&["--init-file", "/r", "-i"], &[]);
        assert_eq!(get(&env, "IT2_BASH_RCFILE"), Some("/r"));
        assert_eq!(args, s(&["--posix", "-i"]));

        for argv in [&["-c", "ls"][..], &["-ic", "ls"], &["script.sh"], &["--", "script.sh"]] {
            let given = pairs(&[("A", "b")]);
            assert_eq!(bash(argv, &[("A", "b")]), (given, s(argv)), "{argv:?}");
        }
        // -s stops the walk, so what follows is not a file; -O and +O take the next word.
        for argv in [&["-s", "x"][..], &["-O", "x"], &["+O", "x"]] {
            let (env, args) = bash(argv, &[]);
            assert_eq!(get(&env, "ENV"), Some(loader.as_str()), "{argv:?}");
            assert_eq!(args[1..], s(argv)[..], "{argv:?}");
        }
        let given = pairs(&[("A", "b")]);
        assert_eq!(bash(&["-O", "x", "script.sh"], &[("A", "b")]), (given, s(&["-O", "x", "script.sh"])));
        let (env, args) = bash(&["-i", "script.sh"], &[]);
        assert_eq!(get(&env, "ENV"), Some(loader.as_str()));
        assert_eq!(args, s(&["--posix", "-i", "script.sh"]));

        let (env, _) = bash(&[], &[("HISTFILE", "/h")]);
        assert_eq!(get(&env, "HISTFILE"), Some("/h"));
        assert_eq!(get(&env, "IT2_BASH_UNEXPORT_HISTFILE"), None);

        let (env, args) = bash(&["--noprofile", "--norc", "--posix"], &[]);
        assert_eq!(get(&env, "IT2_BASH_INJECT"), Some("1 posix no-rc no-profile"));
        assert_eq!(args, s(&["--posix", "--posix"]));
        let (env, _) = modify_remote(DIR, "/home/u", &[], "-bash", &[]);
        assert_eq!(get(&env, "ENV"), Some(loader.as_str()), "a login shell's name");
    }

    #[test]
    fn destinations_follow_iterm2() {
        assert_eq!(tweak_destination("~"), "/$HOME");
        assert_eq!(tweak_destination("~/"), "/$HOME");
        assert_eq!(tweak_destination("~/bin/"), "/$HOME/bin");
        assert_eq!(tweak_destination("bin"), "/$HOME/n", "iTerm2 bug B8, kept");
        assert_eq!(tweak_destination("/opt/x/"), "/opt/x");
        assert_eq!(tweak_destination("/"), "/");
        assert_eq!(expand_local_tilde("~/.vimrc", Path::new(r"C:\Users\u")), Path::new(r"C:\Users\u").join(".vimrc"));
        assert_eq!(expand_local_tilde("~", Path::new(r"C:\Users\u")), PathBuf::from(r"C:\Users\u"));
        assert_eq!(expand_local_tilde(r"D:\x", Path::new(r"C:\Users\u")), PathBuf::from(r"D:\x"));
        #[cfg(windows)]
        assert_eq!(expand_local_tilde("~/.vimrc", Path::new(r"C:\Users\u")), PathBuf::from(r"C:\Users\u\.vimrc"));
    }

    fn disk(path: &str, dest: &str) -> Payload {
        Payload { source: Source::Disk(PathBuf::from(path)), destination: dest.to_string() }
    }

    #[test]
    fn grouping_follows_iterm2() {
        let jobs = group(&injection_payloads());
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].0.destination, "/$HOME/.iterm2/shell-integration");
        assert_eq!(jobs[0].0.entries(), FILES[..6].iter().map(|f| f.name.to_string()).collect::<Vec<_>>());
        assert_eq!(jobs[0].1, (0..6).map(Source::Embedded).collect::<Vec<_>>());
        assert_eq!(jobs[1].0.destination, "/$HOME/.iterm2/shell-integration/fish/vendor_conf.d");
        assert_eq!(jobs[1].0.entries(), s(&["iterm2-shell-integration-loader.fish"]));
        assert_eq!(jobs[1].1, vec![Source::Embedded(6)]);

        let jobs = group(&[disk(r"C:\Users\u\.vimrc", "/$HOME"), disk(r"C:\Users\u\.bashrc", "/$HOME")]);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0.destination, "/$HOME");
        assert_eq!(jobs[0].0.entries(), s(&[".vimrc", ".bashrc"]));
        assert_eq!(jobs[0].0.sources[0], s(&["/", "C:", "Users", "u", ".vimrc"]));

        let jobs = group(&[disk(r"C:\Users\u\bin", "/$HOME"), disk(r"D:\x\y.txt", "/opt")]);
        assert_eq!(jobs.len(), 2);
        assert_eq!((jobs[0].0.destination.as_str(), jobs[1].0.destination.as_str()), ("/$HOME", "/opt"));
        assert_eq!((jobs[0].0.entries(), jobs[1].0.entries()), (s(&["bin"]), s(&["y.txt"])));

        // iTerm2's "/" counts 2 (TJ:79), so files at the roots of two drives would share a job
        // whose local base is "/" and unpack under a folder named for the drive (bug B16):
        // Tandem gives each drive its own job. Two files at one drive's root still share one.
        let jobs = group(&[disk(r"C:\x", "/$HOME"), disk(r"D:\y", "/$HOME")]);
        assert_eq!(jobs.len(), 2);
        assert_eq!((jobs[0].0.local_base.clone(), jobs[0].0.destination.as_str()), (s(&["/", "C:"]), "/$HOME"));
        assert_eq!((jobs[1].0.local_base.clone(), jobs[1].0.destination.as_str()), (s(&["/", "D:"]), "/$HOME"));
        assert_eq!((jobs[0].0.entries(), jobs[1].0.entries()), (s(&["x"]), s(&["y"])));
        let jobs = group(&[disk(r"C:\x", "/$HOME"), disk(r"C:\y", "/$HOME")]);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0.local_base, s(&["/", "C:"]));
        assert_eq!(jobs[0].0.entries(), s(&["x", "y"]));
    }

    /// (name, mode, bytes) for each entry, in archive order.
    fn untar(tgz: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
        use std::io::Read;
        tar::Archive::new(flate2::read::GzDecoder::new(tgz))
            .entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let name = String::from_utf8(e.path_bytes().into_owned()).unwrap();
                let mode = e.header().mode().unwrap();
                assert_eq!(e.header().mtime().unwrap(), 0);
                let mut bytes = Vec::new();
                e.read_to_end(&mut bytes).unwrap();
                (name, mode, bytes)
            })
            .collect()
    }

    #[test]
    fn tarball_follows_iterm2() {
        let jobs = group(&injection_payloads());
        let got = untar(&tarball(&jobs[0].1, &jobs[0].0).unwrap());
        let want: Vec<(String, u32, Vec<u8>)> = FILES[..6].iter().map(|f| (f.name.to_string(), 0o644, f.bytes.to_vec())).collect();
        assert_eq!(got, want);

        let dir = std::env::temp_dir().join(format!("tandem-ssh-inject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("a"), b"alpha").unwrap();
        std::fs::write(dir.join(".vimrc"), b"set nu\n").unwrap();
        let bin = Payload { source: Source::Disk(dir.join("bin")), destination: "/$HOME".into() };
        let jobs = group(std::slice::from_ref(&bin));
        let got = untar(&tarball(&jobs[0].1, &jobs[0].0).unwrap());
        assert_eq!(got, vec![("bin/".to_string(), 0o755, Vec::new()), ("bin/a".to_string(), 0o644, b"alpha".to_vec())]);

        // A missing file sinks its whole job; the others still go.
        let vimrc = Payload { source: Source::Disk(dir.join(".vimrc")), destination: "/$HOME".into() };
        let missing = Payload { source: Source::Disk(dir.join("nope")), destination: "/$HOME".into() };
        let mut payloads = injection_payloads();
        payloads.extend([vimrc.clone(), missing]);
        assert_eq!(group(&payloads).len(), 3, "the two disk files share a job");
        let (out, _) = tarballs(&payloads);
        assert_eq!(out.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), [PAYLOAD_BASE, "/$HOME/.iterm2/shell-integration/fish/vendor_conf.d"]);
        let (out, _) = tarballs(&[vimrc]);
        assert_eq!(untar(&out[0].0), vec![(".vimrc".to_string(), 0o644, b"set nu\n".to_vec())]);
        assert_eq!(out[0].1, "/$HOME");

        assert!(tarball(&[Source::Disk(PathBuf::from(r"\\host\share\x"))], &TarJob::new(components(r"\\host\share\x"), "/$HOME")).is_err(), "a network path is never read");
        for relative in ["x", r"C:x", r"\x"] {
            assert!(tarball(&[Source::Disk(PathBuf::from(relative))], &TarJob::new(components(relative), "/$HOME")).is_err(), "{relative}");
        }

        // A name of 100 bytes or more goes through the GNU long-name record.
        let long = "d".repeat(120);
        std::fs::create_dir_all(dir.join(&long)).unwrap();
        std::fs::write(dir.join(&long).join("f"), b"deep").unwrap();
        let (out, _) = tarballs(&[Payload { source: Source::Disk(dir.join(&long)), destination: "/$HOME".into() }]);
        assert_eq!(untar(&out[0].0), vec![(long.clone(), 0o755, Vec::new()), (format!("{long}/f"), 0o644, b"deep".to_vec())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tarball_caps_hold() {
        assert_eq!((MAX_JOB_BYTES, MAX_JOB_ENTRIES), (64 << 20, 10_000));
        let dir = std::env::temp_dir().join(format!("tandem-ssh-caps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(dir.join("bin").join(name), b"12345").unwrap();
        }
        let bin = [Source::Disk(dir.join("bin"))];
        let job = TarJob::new(components(&dir.join("bin").to_string_lossy()), "/$HOME");
        let packed = |bytes, entries| tarball_within(&bin, &job, Budget { bytes, entries }).map(|t| untar(&t).len());
        assert_eq!(packed(15, 4), Ok(4), "a folder and three files of 5 bytes fit exactly");
        assert!(packed(14, 4).unwrap_err().contains("64 MiB"), "one byte over");
        assert!(packed(15, 3).unwrap_err().contains("10000"), "one entry over");
        let embedded = injection_payloads();
        let jobs = group(&embedded);
        assert!(tarball_within(&jobs[0].1, &jobs[0].0, Budget { bytes: 1 << 20, entries: 5 }).is_err(), "bundled files count too");

        // Over the real cap the job is skipped like a missing file; its size is known before
        // any byte is read.
        let big = dir.join("big");
        std::fs::File::create(&big).unwrap().set_len(MAX_JOB_BYTES + 1).unwrap();
        let small = dir.join("small");
        std::fs::write(&small, b"s").unwrap();
        let to = |p: &Path, d: &str| Payload { source: Source::Disk(p.to_path_buf()), destination: d.into() };
        let (out, _) = tarballs(&[to(&big, "/$HOME"), to(&small, "/opt")]);
        assert_eq!(out.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), ["/opt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn links_to_network_paths_are_refused() {
        use std::collections::HashMap;
        let links: HashMap<&str, &str> = HashMap::from([
            (r"C:\a\share", r"\\host\share"),
            (r"C:\a\l1", "l2"),
            (r"C:\a\l2", r"//host/x"),
            (r"C:\a\j", r"\\?\C:\real"),
            (r"C:\a\u", r"\\?\UNC\host\x"),
            (r"C:\a\up", r"..\b"),
            (r"C:\loop", r"C:\loop"),
        ]);
        let resolve = |path: &str| {
            let mut seen: Vec<String> = Vec::new();
            let mut lookup = |p: &Path| {
                let p = p.to_string_lossy().into_owned();
                assert!(!crate::prompted::network_path(&p), "looked at {p}");
                seen.push(p.clone());
                Ok(links.get(p.as_str()).map(PathBuf::from))
            };
            resolve_links(Path::new(path), &mut lookup).map(|p| p.to_string_lossy().into_owned())
        };
        assert_eq!(resolve(r"C:\a\x.txt"), Ok(r"C:\a\x.txt".to_string()));
        assert!(resolve(r"C:\a\share\f").unwrap_err().contains("network"));
        assert!(resolve(r"C:\a\l1").unwrap_err().contains("network"), "a chain of links");
        assert!(resolve(r"C:\a\u").unwrap_err().contains("network"), "a verbatim UNC target");
        assert_eq!(resolve(r"C:\a\j\f"), Ok(r"C:\real\f".to_string()), "a junction's verbatim local target");
        assert_eq!(resolve(r"C:\a\up\f"), Ok(r"C:\b\f".to_string()), "a relative target");
        assert!(resolve(r"C:\loop").is_err(), "a loop ends");
    }
}
