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
fn last_component(path: &str) -> &str {
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

/// A local file, or a folder with everything under it (symbolic links inside a folder are
/// skipped, Decision).
fn put_disk<W: std::io::Write>(tar: &mut tar::Builder<W>, name: &str, path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_dir() {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return put(tar, name, Some(&bytes));
    }
    put(tar, name, None)?;
    let mut children: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
        if kind.is_symlink() {
            continue;
        }
        children.push((entry.file_name().to_string_lossy().into_owned(), entry.path()));
    }
    children.sort();
    for (child, p) in children {
        put_disk(tar, &format!("{name}/{child}"), &p)?;
    }
    Ok(())
}

/// The gzipped tar of a job's sources (`TJ:53-56`), named by `job.entries()`. A network
/// path is never read (an error, so the job is skipped).
pub fn tarball(sources: &[Source], job: &TarJob) -> Result<Vec<u8>, String> {
    let entries = job.entries();
    if entries.len() != sources.len() {
        return Err("sources do not match the job".into());
    }
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    for (source, name) in sources.iter().zip(&entries) {
        match source {
            Source::Embedded(i) => put(&mut tar, name, Some(FILES.get(*i).ok_or("no such bundled file")?.bytes))?,
            Source::Disk(p) => {
                if crate::prompted::network_path(&p.to_string_lossy()) {
                    return Err("a network path is never read".into());
                }
                put_disk(&mut tar, name, p)?;
            }
        }
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// Every job's tarball and its remote folder (enumeratePayloads,
/// `ConductorPayloadBuilder.swift:20-26`): a job that fails to pack is skipped.
pub fn tarballs(payloads: &[Payload]) -> Vec<(Vec<u8>, String)> {
    group(payloads).into_iter().filter_map(|(job, sources)| tarball(&sources, &job).ok().map(|t| (t, job.destination))).collect()
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

        // iTerm2's "/" counts 2 (TJ:79): files at the roots of two drives share a job whose
        // local base is "/", so they unpack under a folder named for the drive (bug B16, kept).
        let jobs = group(&[disk(r"C:\x", "/$HOME"), disk(r"D:\y", "/$HOME")]);
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].0.local_base.clone(), jobs[0].0.destination.as_str()), (s(&["/"]), "/$HOME"));
        assert_eq!(jobs[0].0.entries(), s(&["C:/x", "D:/y"]));
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
        let out = tarballs(&payloads);
        assert_eq!(out.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), [PAYLOAD_BASE, "/$HOME/.iterm2/shell-integration/fish/vendor_conf.d"]);
        let out = tarballs(&[vimrc]);
        assert_eq!(untar(&out[0].0), vec![(".vimrc".to_string(), 0o644, b"set nu\n".to_vec())]);
        assert_eq!(out[0].1, "/$HOME");

        assert!(tarball(&[Source::Disk(PathBuf::from(r"\\host\share\x"))], &TarJob::new(components(r"\\host\share\x"), "/$HOME")).is_err(), "a network path is never read");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
