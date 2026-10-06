//! it2tip (milestone 11e): iTerm2's tip of the day (`OtherResources/Utilities/it2tip`, a
//! Python script). The script ships byte for byte in `resources/utilities/it2tip`; its tip
//! list is read from there at run time. Pure apart from the index file.

use std::path::PathBuf;

use crate::util_cli::{self, Io};

/// One tip, as the script's list holds it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Tip {
    pub title: String,
    pub body: String,
    pub url: Option<String>,
}

/// iTerm2's script at `64791fc`, byte for byte.
const SCRIPT: &str = include_str!("../resources/utilities/it2tip");

/// The SHA-256 of `SCRIPT`.
pub const SCRIPT_SHA256: &str = "582555a80108ede97ebc088676370fc17033405a61b76853a06b47218d4c95ae";

/// The script's `tips` list, parsed as JSON once its trailing commas are dropped, with
/// "iTerm2" replaced by "Tandem" in titles and bodies.
pub fn tips() -> Vec<Tip> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let start = SCRIPT.find("tips = [").expect("it2tip holds its tips") + "tips = ".len();
    let end = SCRIPT[start..].find("\n  ]\n").expect("it2tip's tips end with \"  ]\"") + start;
    let list = format!("{}\n]", &SCRIPT[start..end]);
    let list = RE.get_or_init(|| regex::Regex::new(r",(\s*[\]}])").expect("trailing comma regex")).replace_all(&list, "$1");
    let mut tips: Vec<Tip> = serde_json::from_str(&list).expect("it2tip's tips parse as JSON");
    for t in &mut tips {
        t.title = t.title.replace("iTerm2", "Tandem");
        t.body = t.body.replace("iTerm2", "Tandem");
    }
    tips
}

/// The script's `osc`: under `screen*` a tmux passthrough ending BEL `ESC \`, else an OSC
/// ending `ESC \`. An unset `TERM` is not screen.
fn osc(term: Option<&str>, args: &[&str]) -> String {
    let body = args.join(";");
    if term.is_some_and(|t| t.starts_with("screen")) {
        format!("\x1bPtmux;\x1b\x1b]{body}\x07\x1b\\")
    } else {
        format!("\x1b]{body}\x1b\\")
    }
}

/// `print_tip`: the title line, the body, and the link when the tip has one.
pub fn tip_text(term: Option<&str>, t: &Tip) -> String {
    let mut s = format!("Tandem tip - {}:\n{}\n", t.title, t.body);
    if let Some(url) = &t.url {
        s.push_str(&format!("Learn more: {}{url}{}\n", osc(term, &["8", "", url]), osc(term, &["8", "", ""])));
    }
    s
}

/// Python's `int()` of a string: surrounding whitespace, a sign, ASCII digits with single
/// underscores between them. Saturates past `i64`.
fn python_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in digits.chars().filter(|c| *c != '_') {
        let d = c.to_digit(10)? as i64;
        n = n.saturating_mul(10).saturating_add(d);
    }
    Some(if neg { -n } else { n })
}

/// The next tip to show from the index file's text (None: no file): `last_index() + 1`,
/// back to 0 past the end with `-w`, None when there is nothing more to show.
pub fn next_index(saved: Option<&str>, wrap: bool, count: usize) -> Option<usize> {
    // `int(f.readlines()[0].rstrip())`, -1 on any failure (a missing first line included).
    let first = saved.and_then(|s| s.split(['\n', '\r']).next().filter(|_| !s.is_empty()));
    let last = first.and_then(python_int).filter(|n| *n >= -1).unwrap_or(-1);
    let mut i = last.saturating_add(1);
    if i >= count as i64 && wrap {
        i = 0;
    }
    (i < count as i64).then_some(i as usize)
}

/// Python's `expanduser` on Windows for a leading "~" alone: `USERPROFILE`, else
/// `HOMEDRIVE` + `HOMEPATH`, else unchanged.
fn expand_user(path: String, env: &dyn Fn(&str) -> Option<String>) -> String {
    let Some(rest) = path.strip_prefix('~') else { return path };
    if !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        return path;
    }
    let home = env("USERPROFILE").or_else(|| Some(env("HOMEDRIVE")? + &env("HOMEPATH")?));
    match home {
        Some(h) => h + rest,
        None => path,
    }
}

/// `RCFILE`: `.it2totd` in `XDG_DATA_HOME` (unset or empty: "~"), "~" expanded.
pub fn rc_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let home = env("XDG_DATA_HOME").filter(|h| !h.is_empty()).unwrap_or_else(|| "~".to_string());
    // `ntpath.join`: a separator only when the folder does not end with one (or a drive).
    let bare_drive = home.len() == 2 && home.ends_with(':');
    let sep = if home.ends_with(['\\', '/']) || bare_drive { "" } else { "\\" };
    PathBuf::from(expand_user(format!("{home}{sep}.it2totd"), env))
}

/// `it2tip [-w]`: the next tip, and its index saved.
pub fn it2tip(args: &[String], io: &mut Io) -> i32 {
    let path = util_cli::join_local(&io.cwd, &rc_path(io.env).to_string_lossy());
    let judged = crate::localfs::LocalPath::judge(&path);
    let saved = judged.as_ref().and_then(|p| crate::localfs::read(p).ok()).and_then(|b| String::from_utf8(b).ok());
    let tips = tips();
    let wrap = args.first().is_some_and(|a| a == "-w");
    let Some(i) = next_index(saved.as_deref(), wrap, tips.len()) else { return 0 };
    let term = util_cli::term(io);
    let _ = io.out.write_all(tip_text(term.as_deref(), &tips[i]).as_bytes());
    let _ = io.out.flush();
    // `save_index`: the number without a newline; a failure ends the script with a traceback.
    let written = match &judged {
        Some(p) => crate::localfs::write(p, i.to_string()).map_err(|e| e.to_string()),
        None => Err("network path".to_string()),
    };
    if let Err(e) = written {
        let _ = io.err.write_all(format!("it2tip: {}: {e}\n", path.display()).as_bytes());
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util_cli::Harness;
    use sha2::{Digest, Sha256};

    #[test]
    fn it2tip_follows_the_script() {
        let digest: String = Sha256::digest(SCRIPT.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(digest, SCRIPT_SHA256);

        let all = tips();
        assert_eq!(all.len(), 123);
        assert_eq!(all.iter().filter(|t| t.url.is_some()).count(), 30);
        assert!(all.iter().all(|t| !t.title.contains("iTerm2") && !t.body.contains("iTerm2")));
        let body = "The big new feature of Tandem version 3 is Shell Integration. Click \u{201c}Learn More\u{201d} for all the details.";
        let url = "https://iterm2.com/shell_integration.html";
        assert_eq!(all[0], Tip { title: "Shell Integration".into(), body: body.into(), url: Some(url.into()) });
        assert_eq!(all[1].url, None);
        assert_eq!(all[122].title, "SSH File Browser");

        let plain = format!("Tandem tip - Shell Integration:\n{body}\nLearn more: \x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\\n");
        assert_eq!(tip_text(None, &all[0]), plain);
        assert_eq!(tip_text(Some("tmux-256color"), &all[0]), plain);
        let wrapped = format!("Tandem tip - Shell Integration:\n{body}\nLearn more: \x1bPtmux;\x1b\x1b]8;;{url}\x07\x1b\\{url}\x1bPtmux;\x1b\x1b]8;;\x07\x1b\\\n");
        assert_eq!(tip_text(Some("screen-256color"), &all[0]), wrapped);
        assert_eq!(tip_text(Some("screen"), &all[1]), format!("Tandem tip - {}:\n{}\n", all[1].title, all[1].body));

        assert_eq!(next_index(None, false, 123), Some(0));
        assert_eq!(next_index(Some("5"), false, 123), Some(6));
        assert_eq!(next_index(Some("5\nx\n"), false, 123), Some(6));
        assert_eq!(next_index(Some(" 5 \r\n"), false, 123), Some(6));
        assert_eq!(next_index(Some("1_0"), false, 123), Some(11));
        assert_eq!(next_index(Some("122"), false, 123), None);
        assert_eq!(next_index(Some("122"), true, 123), Some(0));
        assert_eq!(next_index(Some("99999999999999999999999"), true, 123), Some(0));
        for bad in ["x", "", "\n5", "1__0", "_1", "-7"] {
            assert_eq!(next_index(Some(bad), false, 123), Some(0), "{bad:?}");
        }
        assert_eq!(next_index(Some("-1"), false, 123), Some(0));

        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(rc_path(&env(&[("XDG_DATA_HOME", "D:\\data"), ("USERPROFILE", "C:\\Users\\r")])), PathBuf::from("D:\\data\\.it2totd"));
        assert_eq!(rc_path(&env(&[("XDG_DATA_HOME", "D:\\data\\")])), PathBuf::from("D:\\data\\.it2totd"));
        assert_eq!(rc_path(&env(&[("XDG_DATA_HOME", ""), ("USERPROFILE", "C:\\Users\\r")])), PathBuf::from("C:\\Users\\r\\.it2totd"));
        assert_eq!(rc_path(&env(&[("USERPROFILE", "C:\\Users\\r")])), PathBuf::from("C:\\Users\\r\\.it2totd"));
        assert_eq!(rc_path(&env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", "\\Users\\h")])), PathBuf::from("C:\\Users\\h\\.it2totd"));
        assert_eq!(rc_path(&env(&[("XDG_DATA_HOME", "~\\x"), ("USERPROFILE", "C:\\U")])), PathBuf::from("C:\\U\\x\\.it2totd"));

        // The utility: tip 0, then tip 1, the index saved with no newline.
        let tmp = crate::util_cli::TempDir::new("tandem-util-tip");
        let dir = tmp.to_path_buf();
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        h.env.insert("XDG_DATA_HOME".into(), dir.display().to_string());
        assert_eq!(h.run("it2tip", &[]), 0);
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), plain);
        assert_eq!(std::fs::read(dir.join(".it2totd")).unwrap(), b"0");
        h.out.clear();
        assert_eq!(h.run("it2tip", &[]), 0);
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), tip_text(None, &all[1]));
        assert_eq!(std::fs::read(dir.join(".it2totd")).unwrap(), b"1");
        // Past the end: nothing, the file kept; `-w` starts again.
        std::fs::write(dir.join(".it2totd"), "122").unwrap();
        h.out.clear();
        assert_eq!(h.run("it2tip", &[]), 0);
        assert!(h.out.is_empty());
        assert_eq!(std::fs::read(dir.join(".it2totd")).unwrap(), b"122");
        assert_eq!(h.run("it2tip", &["-w"]), 0);
        assert_eq!(String::from_utf8(h.out.clone()).unwrap(), plain);
        assert_eq!(std::fs::read(dir.join(".it2totd")).unwrap(), b"0");
        assert!(h.err.is_empty());
    }
}
