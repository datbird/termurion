//! The git wrapper's texts (milestone 9i, Task 4): `tandem-git.exe` (Task 6) is the Windows
//! port of iTerm2's `sources/iterm2_git_wrapper.sh`, which a git menu command's single-use
//! window runs. It prints the banner, runs git, and on failure asks to be disinterred and
//! waits. Lines end in CRLF (ConPTY). `command_line` builds the window's command.

use std::path::Path;

/// The wrapper's echo lines before git runs: the folder, the command, an empty line, an SGR
/// reset, and the window title. Each echo joins its words with one space, as bash's does.
pub fn banner(pwd: &str, args: &[String]) -> String {
    let words = std::iter::once("$ git").chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    format!("$ cd {pwd}\r\n{words}\r\n\r\n\x1b[m\x1b]0;git {}\x1b\\\r\n", args.join(" "))
}

/// What the wrapper prints when git fails: disinter the buried session, then the error in
/// reverse video. The doubled space after `git` and the one before the quote are echo's
/// word joining (kept).
pub fn failure(args: &[String]) -> String {
    let words = std::iter::once("'git ").chain(args.iter().map(String::as_str)).chain(std::iter::once("'.")).collect::<Vec<_>>().join(" ");
    format!("\x1b]1337;Disinter\x1b\\\r\n\x1b[7mAn error occurred while running {words}\x1b[m\r\nYou may safely close this window.\r\n")
}

/// The single-use window's command line: the wrapper, the git it runs, then git's
/// arguments, each quoted for the MSVC runtime's `CommandLineToArgvW` rules. The program
/// name is only wrapped in quotes (argv[0] takes no escapes).
pub fn command_line(wrapper: &Path, git: &Path, args: &[String]) -> String {
    let program = wrapper.to_string_lossy();
    let mut out = if program.contains([' ', '\t']) { format!("\"{program}\"") } else { program.into_owned() };
    for a in std::iter::once(git.to_string_lossy().into_owned()).chain(args.iter().cloned()) {
        out.push(' ');
        quote_arg(&a, &mut out);
    }
    out
}

/// One argument: bare unless empty or holding whitespace or a quote; inside quotes, the
/// backslashes before a quote are doubled and the quote escaped, and trailing backslashes
/// doubled before the closing quote.
fn quote_arg(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        let n = if c == '"' { backslashes * 2 + 1 } else { backslashes };
        out.extend(std::iter::repeat_n('\\', n));
        backslashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wrapper_prints_like_iterm2() {
        let args = s(&["commit", "-a"]);
        assert_eq!(banner("C:\\r", &args), "$ cd C:\\r\r\n$ git commit -a\r\n\r\n\x1b[m\x1b]0;git commit -a\x1b\\\r\n");
        assert_eq!(banner("C:\\r", &[]), "$ cd C:\\r\r\n$ git\r\n\r\n\x1b[m\x1b]0;git \x1b\\\r\n", "echo's word joining with no arguments");
        assert_eq!(
            failure(&args),
            "\x1b]1337;Disinter\x1b\\\r\n\x1b[7mAn error occurred while running 'git  commit -a '.\x1b[m\r\nYou may safely close this window.\r\n"
        );
        assert_eq!(failure(&[]), "\x1b]1337;Disinter\x1b\\\r\n\x1b[7mAn error occurred while running 'git  '.\x1b[m\r\nYou may safely close this window.\r\n");

        let git = Path::new("C:\\Program Files\\Git\\cmd\\git.exe");
        let wrapper = Path::new("C:\\Tandem\\tandem-git.exe");
        assert_eq!(command_line(wrapper, Path::new("git"), &s(&["push", "origin", "main"])), "C:\\Tandem\\tandem-git.exe git push origin main");
        assert_eq!(
            command_line(wrapper, git, &s(&["checkout", "my branch", "say \"hi\"", "a\\b", "", "tail\\ x\\"])),
            "C:\\Tandem\\tandem-git.exe \"C:\\Program Files\\Git\\cmd\\git.exe\" checkout \"my branch\" \"say \\\"hi\\\"\" a\\b \"\" \"tail\\ x\\\\\""
        );
        assert_eq!(command_line(Path::new("C:\\My Apps\\tandem-git.exe"), Path::new("git"), &s(&["a\\\"b"])), "\"C:\\My Apps\\tandem-git.exe\" git \"a\\\\\\\"b\"");
    }
}
