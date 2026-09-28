//! it2git (milestone 11e): iTerm2's `OtherResources/Utilities/it2git`, which describes the
//! git repository in the current folder as eleven user variables. `git_values` is pure (git
//! through a closure); `it2git` runs the git `git_state::find_git` finds through `Io.run`,
//! with the status bar poller's checks and guards.

use crate::util_cli::{Cmd, Io};
use crate::util_osc::b64;

/// The variables it2git sets, in the order it sets them.
pub const VARS: [&str; 11] = [
    "gitBranch",
    "gitDirty",
    "gitPushCount",
    "gitPullCount",
    "gitAdds",
    "gitDeletes",
    "gitLinesInserted",
    "gitLinesDeleted",
    "gitFilesAdded",
    "gitFilesModified",
    "gitFilesDeleted",
];

/// `$(...)`: trailing newlines dropped.
fn captured(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

/// `$(cut -f<field> <<< "$text")`: per line, the field (tab separated), a line without a
/// tab printed whole.
fn cut(text: &str, field: usize) -> String {
    let fields: Vec<&str> = text.split('\n').map(|line| if line.contains('\t') { line.split('\t').nth(field - 1).unwrap_or("") } else { line }).collect();
    captured(&fields.join("\n"))
}

/// `wc -l`: the newlines in the output (unpadded, as GNU writes it).
fn newlines(out: &Option<(i32, String)>) -> String {
    out.as_ref().map_or(0, |(_, s)| s.matches('\n').count()).to_string()
}

/// The standard output of a git that ran and succeeded.
fn ok(out: Option<(i32, String)>) -> Option<String> {
    out.filter(|(code, _)| *code == 0).map(|(_, s)| s)
}

/// `line_counts`: the insertions and deletions `diff --numstat` lists, "-" (binary) as 0.
fn line_counts(numstat: &str) -> (i64, i64) {
    let (mut ins, mut del) = (0i64, 0i64);
    // `read` skips a last line that has no newline.
    let complete = numstat.rsplit_once('\n').map_or("", |(done, _)| done);
    for line in complete.split('\n').filter(|_| !numstat.is_empty()) {
        // `IFS=$'\t' read -r line` trims tabs at both ends.
        let line = line.trim_matches('\t');
        let (i, rest) = line.split_once('\t').unwrap_or((line, line));
        let d = rest.split('\t').next().unwrap_or("");
        // Bash arithmetic: "-" is set to 0, anything else not a number reads as 0.
        ins += i.parse::<i64>().unwrap_or(0);
        del += d.parse::<i64>().unwrap_or(0);
    }
    (ins, del)
}

/// `file_counts`: added, modified and deleted files from `status --porcelain`.
fn file_counts(porcelain: &str) -> (usize, usize, usize) {
    let (mut added, mut modified, mut deleted) = (0, 0, 0);
    for line in captured(porcelain).split('\n').filter(|l| !l.is_empty()) {
        let mut c = line.chars();
        let (x, y) = (c.next(), c.next());
        if (x, y) == (Some('?'), Some('?')) || x == Some('A') || y == Some('A') {
            added += 1;
        } else if x == Some('D') || y == Some('D') {
            deleted += 1;
        } else {
            modified += 1;
        }
    }
    (added, modified, deleted)
}

/// The eleven values, in `VARS` order, from `git` (arguments to exit code and standard
/// output; None: git could not run, which reads as bash's failed command).
pub fn git_values(git: &mut dyn FnMut(&[&str]) -> Option<(i32, String)>) -> Vec<(&'static str, String)> {
    if ok(git(&["rev-parse", "--git-dir"])).is_none() {
        return VARS.iter().map(|v| (*v, String::new())).collect();
    }
    let status = ["status", "--porcelain", "--ignore-submodules", "-unormal"];
    let dirty = match ok(git(&status)) {
        Some(out) if !captured(&out).is_empty() => "dirty",
        _ => "clean",
    };
    let counts = ok(git(&["rev-list", "--left-right", "--count", "HEAD...@{u}"])).map_or("error".to_string(), |o| captured(&o));
    let branch = match git(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        Some((0, out)) => captured(&out),
        first => {
            // `a || b` inside `$(...)`: both outputs captured, the second's code decides.
            let before = first.map(|(_, s)| s).unwrap_or_default();
            ok(git(&["rev-parse", "--short", "HEAD"])).map_or(String::new(), |out| captured(&(before + &out)))
        }
    };
    let adds = newlines(&git(&["ls-files", "--others", "--exclude-standard"]));
    let deletes = newlines(&git(&["ls-files", "--deleted", "--exclude-standard"]));
    let numstat = git(&["diff", "--numstat", "--diff-filter=MRT", "HEAD"]).map(|(_, s)| s).unwrap_or_default();
    let (ins, del) = line_counts(&numstat);
    let (fa, fm, fd) = ok(git(&status)).map_or((0, 0, 0), |p| file_counts(&p));
    let values = [
        branch,
        dirty.to_string(),
        cut(&counts, 1),
        cut(&counts, 2),
        adds,
        deletes,
        ins.to_string(),
        del.to_string(),
        fa.to_string(),
        fm.to_string(),
        fd.to_string(),
    ];
    VARS.iter().copied().zip(values).collect()
}

/// `iterm2_set_user_var`: `ESC ] 1337;SetUserVar=<name>=<base64> BEL`, never wrapped.
pub fn set_user_var(name: &str, value: &str) -> String {
    format!("\x1b]1337;SetUserVar={name}={}\x07", b64(value.as_bytes()))
}

/// The environment of every git it2git runs: the poller's (`GIT_ENV` set, `GIT_ENV_REMOVED`
/// gone).
fn git_env() -> Vec<(String, Option<String>)> {
    use crate::git_state::{GIT_ENV, GIT_ENV_REMOVED};
    GIT_ENV.iter().map(|(k, v)| (k.to_string(), Some(v.to_string()))).chain(GIT_ENV_REMOVED.iter().map(|k| (k.to_string(), None))).collect()
}

/// `it2git`: the variables for the repository in the working folder (arguments ignored).
/// It runs from the prompt in any folder, so git runs with the status bar poller's guards
/// (`git_state`): a folder `vet_repo` refuses, or a partial clone on a git that cannot stop a
/// lazy fetch, is no repository (11 empty values, nothing run); every line has the
/// poller's hardening and filter overrides and its environment.
pub fn it2git(_args: &[String], io: &mut Io) -> i32 {
    use crate::git_state::{NO_LAZY_FETCH_VERSION, find_git, guarded_args, parse_git_version, vet_repo};
    let path = (io.env)("PATH").unwrap_or_default();
    let program = find_git(&path, &|p| p.is_file());
    let cwd = io.cwd.clone();
    let vet = program.as_ref().and_then(|_| vet_repo(&cwd));
    let run = &mut *io.run;
    let env = git_env();
    let mut exec = |args: Vec<String>, quiet: bool| {
        let program = program.as_ref()?;
        let cmd = Cmd { program: program.to_string_lossy().into_owned(), args, cwd: Some(cwd.clone()), stdin: None, inherit: false, quiet, env: env.clone() };
        run(&cmd).ok().map(|o| (o.code, String::from_utf8_lossy(&o.stdout).into_owned()))
    };
    let allowed = match &vet {
        None => false,
        Some(v) if v.partial_clone => exec(vec!["version".into()], true)
            .filter(|(code, _)| *code == 0)
            .and_then(|(_, out)| parse_git_version(out.as_bytes()))
            .is_some_and(|v| v >= NO_LAZY_FETCH_VERSION),
        Some(_) => true,
    };
    let mut git = |args: &[&str]| {
        let vet = vet.as_ref().filter(|_| allowed)?;
        // The script sends every git's stderr to /dev/null except `ls-files`'.
        exec(guarded_args(vet, args), args.first() != Some(&"ls-files"))
    };
    let values = git_values(&mut git);
    let out: String = values.iter().map(|(n, v)| set_user_var(n, v)).collect();
    let _ = io.out.write_all(out.as_bytes());
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_state::HARDENED;
    use crate::screen::Screen;
    use crate::util_cli::{Harness, Output, TempDir};

    /// A fake git answering by its arguments; `rev_list` and `git_dir` fail when false.
    fn fake(git_dir: bool, rev_list: bool) -> impl FnMut(&[&str]) -> Option<(i32, String)> {
        move |args: &[&str]| {
            let s = |code: i32, out: &str| Some((code, out.to_string()));
            match args.join(" ").as_str() {
                "rev-parse --git-dir" => s(if git_dir { 0 } else { 128 }, if git_dir { ".git\n" } else { "" }),
                "status --porcelain --ignore-submodules -unormal" => s(0, " M a\n?? b\nD  c\n"),
                "rev-list --left-right --count HEAD...@{u}" => if rev_list { s(0, "1\t2\n") } else { s(128, "") },
                "symbolic-ref -q --short HEAD" => s(0, "main\n"),
                "ls-files --others --exclude-standard" => s(0, "b\n"),
                "ls-files --deleted --exclude-standard" => s(0, ""),
                "diff --numstat --diff-filter=MRT HEAD" => s(0, "3\t4\ta\n-\t-\tbin\n"),
                other => panic!("unexpected git {other}"),
            }
        }
    }

    fn values(v: &[(&'static str, String)]) -> Vec<String> {
        assert_eq!(v.iter().map(|(n, _)| *n).collect::<Vec<_>>(), VARS);
        v.iter().map(|(_, s)| s.clone()).collect()
    }

    #[test]
    fn it2git_follows_the_script() {
        let want = ["main", "dirty", "1", "2", "1", "0", "3", "4", "1", "1", "1"];
        assert_eq!(values(&git_values(&mut fake(true, true))), want);
        let v = values(&git_values(&mut fake(true, false)));
        assert_eq!((v[2].as_str(), v[3].as_str()), ("error", "error"));
        assert_eq!(values(&git_values(&mut fake(false, true))), vec![String::new(); 11]);
        assert_eq!(values(&git_values(&mut |_: &[&str]| None)), vec![String::new(); 11], "no git");

        // The branch fallback, and a clean tree.
        let mut detached = |args: &[&str]| match args[0] {
            "symbolic-ref" => Some((1, String::new())),
            "rev-parse" if args[1] == "--short" => Some((0, "abc1234\n".to_string())),
            "status" | "ls-files" | "diff" => Some((0, String::new())),
            "rev-list" => Some((0, "0\t0\n".to_string())),
            _ => Some((0, ".git\n".to_string())),
        };
        let v = values(&git_values(&mut detached));
        assert_eq!(v, ["abc1234", "clean", "0", "0", "0", "0", "0", "0", "0", "0", "0"].map(String::from));

        assert_eq!(set_user_var("gitBranch", "main"), "\x1b]1337;SetUserVar=gitBranch=bWFpbg==\x07");
        assert_eq!(set_user_var("gitBranch", ""), "\x1b]1337;SetUserVar=gitBranch=\x07");
        assert_eq!(cut("a", 2), "a");
        assert_eq!(file_counts("AM x\nMD y\nR  z\n"), (1, 1, 1));
        assert_eq!(line_counts("5\t1\tf\n2\t2\tg"), (5, 1), "a last line without a newline is skipped");

        // Through `it2git`: the git found on PATH runs in the working folder, never wrapped,
        // each line guarded as the poller's (a repository whose config names programs).
        let tmp = TempDir::new("tandem-util-git");
        let dir = tmp.to_path_buf();
        std::fs::write(dir.join("git.exe"), b"").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("config"), "[core]\n\tfsmonitor = calc.exe\n[filter \"evil\"]\n\tclean = calc.exe\n").unwrap();
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        h.env.insert("TERM".into(), "tmux-256color".into());
        h.env.insert("PATH".into(), dir.display().to_string());
        let answer = |code: i32, out: &str| Ok(Output { code, stdout: out.as_bytes().to_vec() });
        h.answers = [
            answer(0, ".git\n"),
            answer(0, " M a\n?? b\nD  c\n"),
            answer(0, "1\t2\n"),
            answer(0, "main\n"),
            answer(0, "b\n"),
            answer(0, ""),
            answer(0, "3\t4\ta\n-\t-\tbin\n"),
            answer(0, " M a\n?? b\nD  c\n"),
        ]
        .into();
        assert_eq!(h.run("it2git", &[]), 0);
        let expected: String = VARS.iter().zip(want).map(|(n, v)| set_user_var(n, v)).collect();
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), expected);
        assert!(!expected.contains('\n') && !expected.contains("tmux"));
        assert_eq!(h.runs.len(), 8);
        assert!(h.runs.iter().all(|c| c.program == dir.join("git.exe").display().to_string() && c.cwd.as_deref() == Some(dir.as_path())));
        assert_eq!(h.runs.iter().filter(|c| !c.quiet).count(), 2, "only ls-files keeps its stderr");
        let script: [&[&str]; 8] = [
            &["rev-parse", "--git-dir"],
            &["status", "--porcelain", "--ignore-submodules", "-unormal"],
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
            &["symbolic-ref", "-q", "--short", "HEAD"],
            &["ls-files", "--others", "--exclude-standard"],
            &["ls-files", "--deleted", "--exclude-standard"],
            &["diff", "--numstat", "--diff-filter=MRT", "HEAD"],
            &["status", "--porcelain", "--ignore-submodules", "-unormal"],
        ];
        for (c, args) in h.runs.iter().zip(script) {
            assert_eq!(&c.args[..HARDENED.len()], HARDENED, "{:?}", c.args);
            assert!(c.args.iter().any(|a| a == "filter.evil.clean="), "{:?}", c.args);
            assert_eq!(&c.args[c.args.len() - args.len()..], args);
            assert_eq!(c.env, git_env());
            assert!(c.env.contains(&("GIT_NO_LAZY_FETCH".to_string(), Some("1".to_string()))) && c.env.contains(&("GIT_DIR".to_string(), None)));
        }

        // A partial clone on a git before 2.44 (no GIT_NO_LAZY_FETCH) is no repository; on a
        // newer git it is read.
        std::fs::write(dir.join(".git").join("config"), "[extensions]\n\tpartialClone = origin\n").unwrap();
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        h.env.insert("PATH".into(), dir.display().to_string());
        h.answers = [answer(0, "git version 2.43.0.windows.1\n")].into();
        assert_eq!(h.run("it2git", &[]), 0);
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), VARS.iter().map(|n| set_user_var(n, "")).collect::<String>());
        assert_eq!(h.runs.len(), 1);
        assert_eq!(h.runs[0].args, ["version"]);
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        h.env.insert("PATH".into(), dir.display().to_string());
        h.answers = [answer(0, "git version 2.45.1\n"), answer(128, "")].into();
        assert_eq!(h.run("it2git", &[]), 0);
        assert_eq!(h.runs.len(), 2, "version, then rev-parse (not a repository to this fake)");
        // A network working folder: nothing runs.
        let mut h = Harness { cwd: std::path::PathBuf::from("\\\\host\\share"), ..Default::default() };
        h.env.insert("PATH".into(), dir.display().to_string());
        assert_eq!(h.run("it2git", &[]), 0);
        assert!(h.runs.is_empty());
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), VARS.iter().map(|n| set_user_var(n, "")).collect::<String>());

        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        h.env.insert("PATH".into(), dir.join("nothing").display().to_string());
        assert_eq!(h.run("it2git", &[]), 0);
        let empty: String = VARS.iter().map(|n| set_user_var(n, "")).collect();
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), empty);
        assert!(empty.starts_with("\x1b]1337;SetUserVar=gitBranch=\x07\x1b]1337;SetUserVar=gitDirty=\x07"));
        assert!(h.runs.is_empty(), "no git found: nothing runs");

        let mut s = Screen::new(80, 24);
        s.feed(expected.as_bytes());
        assert_eq!(s.shell.vars.get("gitBranch").map(String::as_str), Some("main"));
        assert_eq!(s.shell.vars.get("gitDirty").map(String::as_str), Some("dirty"));
        assert_eq!(s.shell.vars.get("gitLinesDeleted").map(String::as_str), Some("4"));
    }

    /// A repository whose config names an fsmonitor hook and a clean filter: it2git, run
    /// with the real git, runs neither (plain `git status` afterwards measures whether the
    /// setup would have).
    #[cfg(windows)]
    #[test]
    fn repository_programs_never_run() {
        use std::process::{Command, Stdio};
        let path = std::env::var("PATH").unwrap_or_default();
        let Some(git) = crate::git_state::find_git(&path, &|p| p.is_file()) else {
            eprintln!("MEASURED: no git on PATH; it2git_follows_the_script pins the guarded lines");
            return;
        };
        let tmp = TempDir::new("tandem-util-git-hooks");
        let dir = tmp.to_path_buf();
        let plain = |args: &[&str]| {
            let ok = Command::new(&git).args(args).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
            assert!(ok, "git {args:?}");
        };
        plain(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), b"alpha\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), b"*.txt filter=evil\n").unwrap();
        plain(&["add", "."]);
        plain(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]);
        plain(&["config", "core.fsmonitor", "echo ran >> hook-ran.txt"]);
        plain(&["config", "filter.evil.clean", "echo ran >> filter-ran.txt; cat"]);
        // The same bytes with a new time: status must hash the file, through its clean filter.
        let f = std::fs::File::options().write(true).open(dir.join("a.txt")).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10)).unwrap();
        drop(f);

        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut input: &[u8] = b"";
        let mut tty = crate::util_cli::FakeTty::default();
        let env = |k: &str| std::env::var(k).ok();
        let mut random = || 0u32;
        let mut run = crate::util_console::run_command;
        let have = |_: &str| true;
        let mut io = Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
            stdin_tty: false,
            stdout_tty: false,
            tty: &mut tty,
            env: &env,
            cwd: dir.clone(),
            random: &mut random,
            run: &mut run,
            have: &have,
        };
        assert_eq!(it2git(&[], &mut io), 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b]1337;SetUserVar=gitBranch=") && !text.starts_with("\x1b]1337;SetUserVar=gitBranch=\x07"), "a branch: {text:?}");
        assert!(!dir.join("hook-ran.txt").exists(), "the fsmonitor hook ran");
        assert!(!dir.join("filter-ran.txt").exists(), "the clean filter ran");

        let _ = Command::new(&git).args(["status", "--porcelain"]).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        println!("MEASURED plain git status ran the hook: {}, the filter: {}", dir.join("hook-ran.txt").exists(), dir.join("filter-ran.txt").exists());
    }
}
