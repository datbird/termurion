//! TerminalHardRules plus the Windows set (Task 9). The floor under the safety check: iTerm2's
//! POSIX rules ported from `AITerm/TerminalHardRules.swift`, and a Windows set (ruling 9). The
//! rules never allow and never block: a reason means "needs manual approval", None defers to the
//! classifier.

use crate::ai_shell::ShellKind;

/// The reason a line needs a person's approval, or None to defer to the classifier. The ESC and
/// CR rules hold for every shell; the POSIX set runs for Posix and Unknown, the Windows set for
/// Cmd, PowerShell and Unknown. The first reason wins.
pub fn check(line: &str, kind: ShellKind) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(r) = floor(line) {
        return Some(r);
    }
    if matches!(kind, ShellKind::Posix | ShellKind::Unknown) {
        if let Some(r) = posix(line) {
            return Some(r);
        }
    }
    if matches!(kind, ShellKind::Cmd | ShellKind::PowerShell | ShellKind::Unknown) {
        return windows(line, kind);
    }
    None
}

/// The character-presence floor (`TerminalHardRules.swift` evaluate): ESC could rewrite the screen
/// or drive OSC 52, CR tokenizes differently for the shell than for the safety check.
fn floor(line: &str) -> Option<String> {
    if line.contains('\u{1b}') {
        return Some(ESCAPE.to_string());
    }
    if line.contains('\r') {
        return Some(CARRIAGE_RETURN.to_string());
    }
    None
}

/// iTerm2's rules, unchanged (`evaluate`, `TerminalHardRules.swift:153-313`).
pub fn posix(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(r) = floor(line) {
        return Some(r);
    }
    if line.starts_with('^') {
        return Some(HISTORY.to_string());
    }
    if let Some(rest) = line.strip_prefix('!') {
        if let Some(second) = rest.chars().next() {
            if !matches!(second, ' ' | '\t' | '=' | '(' | '\n') {
                return Some(HISTORY.to_string());
            }
        }
    }
    // Any LF defers: the classifier reads a heredoc or a loop in full.
    if line.contains('\n') {
        return None;
    }
    let segments = split_posix(line).ok();
    let Some(segments) = segments else {
        return Some(UNBALANCED.to_string());
    };
    for (text, after_pipe) in &segments {
        if has_history_expansion(text) {
            return Some(HISTORY.to_string());
        }
        let words = tokenize_posix(text);
        if redirects_to_block_device(text) {
            return Some(DEVICE_REDIRECT.to_string());
        }
        let Some((cmd, args)) = effective_command(&words) else { continue };
        if let Some(r) = catastrophic(&cmd, &args, *after_pipe) {
            return Some(r);
        }
    }
    None
}

const ZSH_DANGEROUS: &[&str] = &[
    "zmodload", "sysopen", "sysread", "syswrite", "sysseek", "zpty", "ztcp", "zsocket", "zf_rm", "zf_mv",
    "zf_ln", "zf_chmod", "zf_chown", "zf_mkdir", "zf_rmdir", "zf_chgrp",
];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "mksh", "pdksh", "ash", "fish", "tcsh", "csh", "busybox"];
const MODIFIERS: &[&str] = &[
    "sudo", "doas", "command", "builtin", "exec", "eval", "!", "env", "nohup", "nice", "time", "then", "do", "noglob",
    "nocorrect",
];
const DESTRUCTIVE_EXEC: &[&str] =
    &["rm", "unlink", "rmdir", "mv", "dd", "shred", "chmod", "chown", "chgrp", "truncate", "mkfs"];
const SAFE_DEV: &[&str] = &["null", "zero", "stdout", "stderr", "tty", "random", "urandom", "full"];

fn catastrophic(cmd: &str, args: &[String], after_pipe: bool) -> Option<String> {
    if after_pipe && SHELLS.contains(&cmd) {
        return Some(pipe_reason(cmd));
    }
    if ZSH_DANGEROUS.contains(&cmd) {
        return Some(zsh_reason(cmd));
    }
    if cmd == "rm" && has_flag(args, &['r', 'R'], "recursive") && args.iter().any(|a| is_catastrophic_target(a)) {
        return Some(RM_ROOT.to_string());
    }
    if matches!(cmd, "chmod" | "chown" | "chgrp")
        && has_flag(args, &['R'], "recursive")
        && args.iter().any(|a| is_catastrophic_target(a))
    {
        return Some(CHMOD_ROOT.to_string());
    }
    if cmd == "dd" && args.iter().any(|a| is_device_write_arg(a)) {
        return Some(DD_DEVICE.to_string());
    }
    if cmd == "mkfs" || cmd.starts_with("mkfs.") {
        return Some(MKFS.to_string());
    }
    if cmd == "find" && args.iter().any(|a| is_catastrophic_target(a)) && find_is_destructive(args) {
        return Some(FIND_ROOT.to_string());
    }
    None
}

/// True for an unquoted, unescaped `!` that triggers history expansion (see the source's long
/// comment): not followed by whitespace, `=`, `(` or a newline, not after `$`, not in `${!`.
fn has_history_expansion(segment: &str) -> bool {
    let chars: Vec<char> = segment.chars().collect();
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_double = !in_double;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = true;
            continue;
        }
        if c != '!' {
            continue;
        }
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        if prev == Some('$') {
            continue;
        }
        if prev == Some('{') && i >= 2 && chars[i - 2] == '$' {
            continue;
        }
        match chars.get(i + 1) {
            None | Some(' ') | Some('\t') | Some('=') | Some('(') | Some('\n') => continue,
            _ => return true,
        }
    }
    false
}

/// The segments of a line split on unquoted `|`, `&`, `;`, each with whether a pipe started it.
/// Err when the line ends inside a quote or on a dangling backslash.
pub fn split_posix(line: &str) -> Result<Vec<(String, bool)>, ()> {
    let chars: Vec<char> = line.chars().collect();
    let mut segs: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_pipe = false;
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if escaped {
            cur.push(c);
            escaped = false;
            i += 1;
            continue;
        }
        if c == '\\' && !in_single {
            cur.push(c);
            escaped = true;
            i += 1;
            continue;
        }
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            cur.push(c);
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        let mut split = |after_pipe: bool, cur: &mut String, cur_pipe: &mut bool| {
            segs.push((std::mem::take(cur), *cur_pipe));
            *cur_pipe = after_pipe;
        };
        match c {
            '\'' => {
                in_single = true;
                cur.push(c);
            }
            '"' => {
                in_double = true;
                cur.push(c);
            }
            '|' => {
                if next == Some('|') {
                    split(false, &mut cur, &mut cur_pipe);
                    i += 1;
                } else if next == Some('&') {
                    split(true, &mut cur, &mut cur_pipe);
                    i += 1;
                } else {
                    split(true, &mut cur, &mut cur_pipe);
                }
            }
            '&' => {
                if next == Some('>') {
                    cur.push(c);
                } else {
                    split(false, &mut cur, &mut cur_pipe);
                    if next == Some('&') {
                        i += 1;
                    }
                }
            }
            ';' => split(false, &mut cur, &mut cur_pipe),
            _ => cur.push(c),
        }
        i += 1;
    }
    segs.push((cur, cur_pipe));
    if in_single || in_double || escaped {
        return Err(());
    }
    Ok(segs)
}

/// Quote-aware word split: quotes and backslashes are stripped, whitespace splits.
fn tokenize_posix(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    for ch in s.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            started = true;
            continue;
        }
        if in_single {
            started = true;
            if ch == '\'' {
                in_single = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if in_double {
            started = true;
            if ch == '"' {
                in_double = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if ch == '\'' {
            in_single = true;
            started = true;
            continue;
        }
        if ch == '"' {
            in_double = true;
            started = true;
            continue;
        }
        if ch == ' ' || ch == '\t' {
            if started {
                words.push(std::mem::take(&mut cur));
                started = false;
            }
            continue;
        }
        cur.push(ch);
        started = true;
    }
    if started {
        words.push(cur);
    }
    words
}

fn is_env_assignment(w: &str) -> bool {
    let Some(eq) = w.find('=') else { return false };
    if eq == 0 {
        return false;
    }
    let name = &w[..eq];
    let first = name.chars().next().unwrap();
    (first.is_alphabetic() || first == '_') && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn base_command_name(token: &str) -> String {
    let lower = token.to_lowercase();
    match lower.rfind('/') {
        Some(i) => lower[i + 1..].to_string(),
        None => lower,
    }
}

/// A segment's command and arguments with leading `VAR=val` words and modifiers (and a
/// modifier's own option flags) skipped.
fn effective_command(words: &[String]) -> Option<(String, Vec<String>)> {
    let mut i = 0;
    let mut saw_modifier = false;
    while i < words.len() {
        let w = &words[i];
        if is_env_assignment(w) {
            i += 1;
            continue;
        }
        let base = base_command_name(w.strip_prefix('\\').unwrap_or(w));
        if MODIFIERS.contains(&base.as_str()) {
            saw_modifier = true;
            i += 1;
            continue;
        }
        if saw_modifier && w.starts_with('-') && w.chars().count() > 1 {
            i += 1;
            continue;
        }
        return Some((base, words[i + 1..].to_vec()));
    }
    None
}

fn is_block_device_path(p: &str) -> bool {
    let Some(dev) = p.strip_prefix("/dev/") else { return false };
    !dev.is_empty() && !SAFE_DEV.contains(&dev) && !dev.starts_with("fd/")
}

fn is_device_write_arg(arg: &str) -> bool {
    arg.strip_prefix("of=").map(is_block_device_path).unwrap_or(false)
}

/// An unquoted `>` or `>>` whose target is a raw device, on any command. Scans the raw text with
/// quote state so a `>` inside a string is literal.
fn redirects_to_block_device(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == '\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == '"' {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_double {
            i += 1;
            continue;
        }
        if c == '\'' {
            in_single = true;
            i += 1;
            continue;
        }
        if c != '>' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if chars.get(j) == Some(&'>') {
            j += 1;
        }
        while matches!(chars.get(j), Some(' ') | Some('\t')) {
            j += 1;
        }
        let mut target = String::new();
        while j < chars.len() {
            match chars[j] {
                ' ' | '\t' | '>' | '<' | '|' | ';' | '&' | '\n' => break,
                '"' | '\'' => j += 1,
                ch => {
                    target.push(ch);
                    j += 1;
                }
            }
        }
        if is_block_device_path(&target) {
            return true;
        }
        i = j;
    }
    false
}

fn has_flag(args: &[String], short: &[char], long: &str) -> bool {
    for a in args {
        if let Some(l) = a.strip_prefix("--") {
            if l == long {
                return true;
            }
        } else if a.starts_with('-') && a.chars().count() > 1 && a.chars().skip(1).any(|c| short.contains(&c)) {
            return true;
        }
    }
    false
}

fn find_is_destructive(args: &[String]) -> bool {
    if args.iter().any(|a| a == "-delete") {
        return true;
    }
    for (i, a) in args.iter().enumerate() {
        if ["-exec", "-execdir", "-ok", "-okdir"].contains(&a.as_str()) && i + 1 < args.len() {
            let target = base_command_name(&args[i + 1]);
            if DESTRUCTIVE_EXEC.contains(&target.as_str()) || SHELLS.contains(&target.as_str()) {
                return true;
            }
        }
    }
    false
}

/// The whole filesystem root or home in any spelling (`/`, `//`, `/*`, `/.`, `/..`, `~`, `~/`,
/// `~/*`, `$HOME`, `${HOME}`, `$HOME/*`), never a subpath.
fn is_catastrophic_target(token: &str) -> bool {
    let mut s = token.to_string();
    if s.ends_with('*') {
        s.pop();
    }
    if s.chars().count() > 1 && s.ends_with('/') {
        s.pop();
    }
    if s.is_empty() {
        return false;
    }
    if s == "/." || s == "/.." {
        s = "/".to_string();
    }
    if s.chars().all(|c| c == '/') {
        s = "/".to_string();
    }
    matches!(s.as_str(), "/" | "~" | "$HOME" | "${HOME}")
}

// ---- The Windows set (ruling 9) ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Cmd,
    Ps,
}

/// Commands a pipe must not feed: iex, another shell, and (ruling, beyond ruling 9) the POSIX
/// shells and wsl, since `curl.exe x | bash` reaches WSL's bash.
const WIN_PIPE_SHELLS: &[&str] = &["iex", "invoke-expression", "powershell", "pwsh", "cmd", "wsl"];
const PS_REMOVE: &[&str] = &["remove-item", "ri", "rm", "rmdir", "rd", "del", "erase"];
const WIN_TARGETS: &[&str] = &[
    "~",
    "$home",
    "${home}",
    "${env:userprofile}",
    "${env:homedrive}",
    "${env:systemdrive}",
    "${env:homedrive}${env:homepath}",
    "$env:userprofile",
    "$env:homedrive",
    "$env:systemdrive",
    "$env:homedrive$env:homepath",
    "%userprofile%",
    "%homedrive%",
    "%systemdrive%",
    "%homedrive%%homepath%",
];
/// The line itself plus three unwrapped `cmd /c` or `powershell -Command` levels.
const WIN_DEPTH: usize = 4;

/// The Windows set: recursive deletes of a drive root or home, `format`, `diskpart`, and a pipe
/// into `iex`, `powershell`, `pwsh` or `cmd`. Cmd uses cmd's rules, PowerShell its own, Unknown
/// both (first reason wins). Posix has no Windows set.
pub fn windows(line: &str, kind: ShellKind) -> Option<String> {
    match kind {
        ShellKind::Cmd => windows_flavor(line, Flavor::Cmd, WIN_DEPTH),
        ShellKind::PowerShell => windows_flavor(line, Flavor::Ps, WIN_DEPTH),
        ShellKind::Unknown => windows_flavor(line, Flavor::Cmd, WIN_DEPTH).or_else(|| windows_flavor(line, Flavor::Ps, WIN_DEPTH)),
        ShellKind::Posix => None,
    }
}

/// The line split on the shell's separators with whether a pipe started each segment. Unknown
/// splits as PowerShell. Err on an unbalanced quote or a dangling escape.
pub fn split_windows(line: &str, kind: ShellKind) -> Result<Vec<(String, bool)>, ()> {
    scan_win(line, if kind == ShellKind::Cmd { Flavor::Cmd } else { Flavor::Ps })
}

fn escape_char(f: Flavor) -> char {
    if f == Flavor::Cmd {
        '^'
    } else {
        '`'
    }
}

fn scan_win(line: &str, f: Flavor) -> Result<Vec<(String, bool)>, ()> {
    scan_win_marked(line, f).map(|segs| segs.into_iter().map(|(text, pipe, _)| (text, pipe)).collect())
}

/// `scan_win`, each segment also marked when a bracket started it (a script block or grouping
/// inside a pipeline stage, which does not end the pipeline).
fn scan_win_marked(line: &str, f: Flavor) -> Result<Vec<(String, bool, bool)>, ()> {
    let chars: Vec<char> = line.chars().collect();
    let mut segs: Vec<(String, bool, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_pipe = false;
    let mut cur_bracket = false;
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if escaped {
            cur.push(c);
            escaped = false;
            i += 1;
            continue;
        }
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if f == Flavor::Ps && c == '`' {
                escaped = true;
            } else if c == '"' {
                in_double = false;
            }
            cur.push(c);
            i += 1;
            continue;
        }
        // PowerShell's `${name}` is a variable, not a script block.
        if f == Flavor::Ps && c == '$' && next == Some('{') {
            if let Some(end) = braced_var_end(&chars, i) {
                cur.extend(&chars[i..end]);
                i = end;
                continue;
            }
        }
        let mut split = |after_pipe: bool, bracket: bool, cur: &mut String, cur_pipe: &mut bool, cur_bracket: &mut bool| {
            segs.push((std::mem::take(cur), *cur_pipe, *cur_bracket));
            *cur_pipe = after_pipe;
            *cur_bracket = bracket;
        };
        if c == escape_char(f) {
            cur.push(c);
            escaped = true;
        } else {
            match c {
                '"' => {
                    in_double = true;
                    cur.push(c);
                }
                '\'' if f == Flavor::Ps => {
                    in_single = true;
                    cur.push(c);
                }
                '|' => {
                    if next == Some('|') {
                        split(false, false, &mut cur, &mut cur_pipe, &mut cur_bracket);
                        i += 1;
                    } else {
                        split(true, false, &mut cur, &mut cur_pipe, &mut cur_bracket);
                    }
                }
                '&' if f == Flavor::Cmd => {
                    split(false, false, &mut cur, &mut cur_pipe, &mut cur_bracket);
                    if next == Some('&') {
                        i += 1;
                    }
                }
                '&' if next == Some('&') => {
                    split(false, false, &mut cur, &mut cur_pipe, &mut cur_bracket);
                    i += 1;
                }
                ';' if f == Flavor::Ps => split(false, false, &mut cur, &mut cur_pipe, &mut cur_bracket),
                // Grouping and script blocks start and end commands: each inner command is
                // checked as a segment of its own.
                // A bracket right after a pipe keeps the pipe for the command it opens.
                '(' | ')' | '{' | '}' => {
                    if !cur.trim().is_empty() {
                        split(false, true, &mut cur, &mut cur_pipe, &mut cur_bracket);
                    }
                }
                '\n' => split(false, false, &mut cur, &mut cur_pipe, &mut cur_bracket),
                _ => cur.push(c),
            }
        }
        i += 1;
    }
    segs.push((cur, cur_pipe, cur_bracket));
    if in_single || in_double || escaped {
        return Err(());
    }
    Ok(segs)
}

fn tokenize_win(s: &str, f: Flavor) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    for ch in s.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if in_single {
            started = true;
            if ch == '\'' {
                in_single = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if in_double {
            started = true;
            if f == Flavor::Ps && ch == '`' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        if ch == escape_char(f) {
            escaped = true;
            started = true;
            continue;
        }
        if ch == '"' {
            in_double = true;
            started = true;
            continue;
        }
        if ch == '\'' && f == Flavor::Ps {
            in_single = true;
            started = true;
            continue;
        }
        if ch == ' ' || ch == '\t' || ch == '\r' {
            if started {
                words.push(std::mem::take(&mut cur));
                started = false;
            }
            continue;
        }
        cur.push(ch);
        started = true;
    }
    if started {
        words.push(cur);
    }
    words
}

/// Lowercase command name without a directory and without `.exe` or `.com`.
fn win_command_name(token: &str) -> String {
    let lower = token.to_lowercase();
    let base = match lower.rfind(['\\', '/']) {
        Some(i) => &lower[i + 1..],
        None => &lower,
    };
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".com")).unwrap_or(base);
    base.to_string()
}

/// One normalizing pass before any matching (Task 9 review, the floor is not a parser): what
/// PowerShell's lexer accepts as a dash, a quote or whitespace becomes ASCII, and a line
/// continued with the shell's escape before a line break is joined.
fn normalize_win(line: &str, f: Flavor) -> String {
    let esc = escape_char(f);
    let mut s = String::with_capacity(line.len());
    // How many escapes end `s`: an odd run ends with one that is not itself escaped.
    let mut run = 0;
    for c in line.chars() {
        let c = if f == Flavor::Ps { ps_ascii(c) } else { c };
        if c == '\n' && run % 2 == 1 {
            s.pop();
            run = 0;
            continue;
        }
        run = if c == esc { run + 1 } else { 0 };
        s.push(c);
    }
    s
}

/// Where a PowerShell `${name}` variable starting at `chars[i]` ends (past its `}`), if it does.
fn braced_var_end(chars: &[char], i: usize) -> Option<usize> {
    let close = chars[i + 2..].iter().position(|&c| c == '}')?;
    Some(i + 2 + close + 1)
}

/// The line with grouping brackets read as spaces (a PowerShell `${name}` is kept).
fn flatten_brackets(line: &str, f: Flavor) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if f == Flavor::Ps && chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
            if let Some(end) = braced_var_end(&chars, i) {
                out.extend(&chars[i..end]);
                i = end;
                continue;
            }
        }
        out.push(if matches!(chars[i], '(' | ')' | '{' | '}') { ' ' } else { chars[i] });
        i += 1;
    }
    out
}

/// PowerShell reads U+2013 to U+2015 as a parameter dash, the curly quotes as quotes, and any
/// Unicode space as whitespace.
fn ps_ascii(c: char) -> char {
    match c {
        '\u{2013}' | '\u{2014}' | '\u{2015}' => '-',
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
        '\u{85}' => '\n',
        ' ' | '\t' | '\n' => c,
        c if c.is_whitespace() => ' ',
        c => c,
    }
}

/// The words of a cmd line, each cut at cmd's own argument delimiters `,` `;` `=` (quotes are
/// already gone).
fn cmd_pieces(word: &str) -> Vec<String> {
    word.split([',', ';', '=']).filter(|p| !p.is_empty()).map(str::to_string).collect()
}

/// A head word without the `@` that silences echo and, in cmd, the `,` `;` `=` it skips before
/// a command (`,format C:` is `format C:`).
fn trim_head(word: &str, f: Flavor) -> &str {
    if f == Flavor::Cmd {
        word.trim_start_matches(['@', ',', ';', '='])
    } else {
        word.trim_start_matches('@')
    }
}

/// The command named by `words[i]`, its arguments, and its arguments as written. cmd ends a
/// command's name at `/`, `,`, `;` or `=` (`rd/s/q C:\` is `rd /s /q C:\`), and splits its
/// arguments at the same delimiters; the words as written are what a wrapper hands on (a
/// quoted PowerShell script keeps its `;`).
fn split_head(words: &[String], i: usize, f: Flavor) -> (String, Vec<String>, Vec<String>) {
    let w = trim_head(&words[i], f);
    let mut args: Vec<String> = Vec::new();
    let mut raw: Vec<String> = Vec::new();
    let mut name = w;
    if f == Flavor::Cmd {
        // A path with a directory keeps its own slashes; only a bare word is cut.
        if let Some(at) = w.find(['/', ',', ';', '=']).filter(|&at| at > 0 && !w[..at].contains(['\\', ':'])) {
            name = &w[..at];
            args.extend(cmd_pieces(&w[at..]));
            raw.push(w[at..].to_string());
        }
    }
    for a in &words[i + 1..] {
        if f == Flavor::Cmd {
            args.extend(cmd_pieces(a));
        } else {
            args.push(a.clone());
        }
        raw.push(a.clone());
    }
    (win_command_name(name), args, raw)
}

/// The indexes where a command starts: the first word after `@`, `&`, `call` and `.` (dot
/// source), and in cmd the word after an `if` clause, `do` or `else`.
fn command_heads(words: &[String], f: Flavor) -> Vec<usize> {
    let lower = |i: usize| words.get(i).map(|w| trim_head(w, f).to_lowercase());
    let mut heads = Vec::new();
    let mut at_head = true;
    let mut i = 0;
    while i < words.len() {
        let w = lower(i).unwrap_or_default();
        if at_head {
            match w.as_str() {
                "" | "&" | "call" | "." => {
                    i += 1;
                    continue;
                }
                "do" | "else" if f == Flavor::Cmd => {
                    i += 1;
                    continue;
                }
                "if" if f == Flavor::Cmd => {
                    i += 1;
                    while matches!(lower(i).as_deref(), Some("/i" | "not")) {
                        i += 1;
                    }
                    i += match lower(i).as_deref() {
                        Some("exist" | "defined" | "errorlevel" | "cmdextversion") => 2,
                        _ if matches!(lower(i + 1).as_deref(), Some("equ" | "neq" | "lss" | "leq" | "gtr" | "geq")) => 3,
                        _ => 1,
                    };
                    continue;
                }
                _ => {}
            }
            heads.push(i);
            at_head = false;
        }
        if f == Flavor::Cmd && matches!(w.as_str(), "do" | "else") {
            at_head = true;
        }
        i += 1;
    }
    heads
}

/// A drive root or the home directory in any of its spellings: with a trailing separator, `*`,
/// `*.*`, `\.` or `\..`, a `\\?\` prefix, quotes already stripped.
fn is_win_target(token: &str) -> bool {
    let mut s = token.to_lowercase();
    for prefix in ["\\\\?\\", "\\\\.\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    // A separator, wildcard or dot component was taken off: what is left names the parent.
    let mut separator = false;
    loop {
        let before = s.len();
        if let Some(r) = s.strip_suffix("*.*").or_else(|| s.strip_suffix('*')) {
            s = r.to_string();
        }
        let trimmed = s.trim_end_matches(['\\', '/']);
        if trimmed.len() != s.len() {
            separator = true;
            s = trimmed.to_string();
        }
        for dots in ["\\..", "/..", "\\.", "/."] {
            if let Some(r) = s.strip_suffix(dots) {
                s = r.to_string();
                separator = true;
                break;
            }
        }
        if s.len() == before {
            break;
        }
    }
    if s.is_empty() {
        return separator;
    }
    let b: Vec<char> = s.chars().collect();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == ':' {
        return true;
    }
    WIN_TARGETS.contains(&s.as_str())
}

/// A target argument, also as the value of a colon-bound PowerShell parameter (`-Path:C:\`).
fn arg_is_target(arg: &str) -> bool {
    if is_win_target(arg) {
        return true;
    }
    match arg.strip_prefix('-').and_then(|a| a.split_once(':')) {
        Some((_, value)) => is_win_target(value),
        None => false,
    }
}

/// cmd's `/s` switch, alone or combined (`/s`, `/S`, `/s/q`, `/q/s`).
fn has_slash_s(args: &[String]) -> bool {
    args.iter().any(|a| a.starts_with('/') && a.to_lowercase().split('/').any(|p| p == "s"))
}

/// A PowerShell parameter that is a prefix of `-Recurse` (`-r`, `-re`, ..., `-Recurse:$true`).
fn has_ps_recurse(args: &[String]) -> bool {
    args.iter().any(|a| {
        let Some(name) = a.strip_prefix('-') else { return false };
        let name = name.split(':').next().unwrap_or("").to_lowercase();
        !name.is_empty() && "recurse".starts_with(&name)
    })
}

/// Whether `cmd` with `args` is a recursive delete (its target not looked at).
fn is_recursive_delete(cmd: &str, args: &[String], f: Flavor) -> bool {
    match f {
        Flavor::Cmd => matches!(cmd, "del" | "erase" | "rd" | "rmdir") && has_slash_s(args),
        Flavor::Ps => PS_REMOVE.contains(&cmd) && has_ps_recurse(args),
    }
}

fn windows_flavor(line: &str, f: Flavor, depth: usize) -> Option<String> {
    let norm = normalize_win(line, f);
    let segs = match scan_win_marked(&norm, f) {
        Ok(s) => s,
        Err(()) => return Some(UNBALANCED.to_string()),
    };
    // A pipeline's earlier segments named a target (`Get-ChildItem C:\ | Remove-Item -Recurse`),
    // across the script block of a pipe stage (`| Where-Object { .. } |`), and only there: a
    // condition's or grouping's own target does not carry into a later pipeline.
    let mut chain_target = false;
    // Inside a pipe stage (or the block it opened), and whether that stage is ForEach-Object.
    let (mut in_stage, mut foreach_stage) = (false, false);
    for (text, after_pipe, bracket) in &segs {
        let words = tokenize_win(text, f);
        let filter_block = !after_pipe && *bracket && in_stage;
        if !after_pipe && !filter_block && !words.is_empty() {
            chain_target = false;
        }
        let mut first = true;
        for h in command_heads(&words, f) {
            let (cmd, args, raw) = split_head(&words, h, f);
            if *after_pipe && first {
                foreach_stage = f == Flavor::Ps && matches!(cmd.as_str(), "foreach-object" | "foreach" | "%");
            }
            // A ForEach-Object block's command works on what the pipeline carried.
            let piped = (*after_pipe || (filter_block && foreach_stage)) && first;
            first = false;
            if let Some(r) = windows_verdict(&cmd, &args, &raw, piped, chain_target, f, depth) {
                return Some(r);
            }
        }
        chain_target |= words.iter().any(|w| arg_is_target(w));
        in_stage = *after_pipe || filter_block;
        if !in_stage {
            foreach_stage = false;
        }
    }
    // A delete anywhere in a segment, brackets read as spaces so the arguments flow across them
    // (`Remove-Item -Recurse (Get-Item C:\)`, `if exist x rd /s /q C:\`).
    let flat = flatten_brackets(&norm, f);
    for (text, _) in scan_win(&flat, f).unwrap_or_default() {
        let words = tokenize_win(&text, f);
        for i in 0..words.len() {
            let (cmd, args, _) = split_head(&words, i, f);
            if is_recursive_delete(&cmd, &args, f) && args.iter().any(|a| arg_is_target(a)) {
                return Some(WIN_DELETE.to_string());
            }
        }
    }
    None
}

/// `args` for the switch and target rules, `raw` (the words as written) for a wrapper's script.
fn windows_verdict(cmd: &str, args: &[String], raw: &[String], after_pipe: bool, chain_target: bool, f: Flavor, depth: usize) -> Option<String> {
    if after_pipe && (WIN_PIPE_SHELLS.contains(&cmd) || SHELLS.contains(&cmd)) {
        return Some(pipe_reason(cmd));
    }
    if is_recursive_delete(cmd, args, f) && (args.iter().any(|a| arg_is_target(a)) || (after_pipe && chain_target && f == Flavor::Ps)) {
        return Some(WIN_DELETE.to_string());
    }
    if cmd == "format" || cmd == "format-volume" {
        return Some(WIN_FORMAT.to_string());
    }
    if cmd == "diskpart" {
        return Some(WIN_DISKPART.to_string());
    }
    for w in wrapped_lines(cmd, raw, f) {
        match w {
            // An encoded script cannot be read, whatever the depth.
            Wrapped::Opaque => return Some(WIN_ENCODED.to_string()),
            Wrapped::Line(inner, flavor) if depth > 1 => {
                if let Some(r) = windows_flavor(&inner, flavor, depth - 1) {
                    return Some(r);
                }
            }
            // A wrapper still running a line past the last level read: flagged, never passed.
            Wrapped::Line(inner, _) if !inner.trim().is_empty() => return Some(WIN_NESTED.to_string()),
            Wrapped::Line(..) => {}
        }
    }
    None
}

/// What a wrapper runs.
enum Wrapped {
    /// The line it hands to a shell.
    Line(String, Flavor),
    /// A script that cannot be read (`-EncodedCommand`).
    Opaque,
}

/// PowerShell switches that take the next word as their value (a switch may be abbreviated to
/// any prefix that is not another switch's): Windows PowerShell 5.1's and pwsh's.
const PS_VALUE_SWITCHES: &[&str] = &[
    "executionpolicy",
    "ep",
    "windowstyle",
    "inputformat",
    "outputformat",
    "configurationname",
    "configurationfile",
    "custompipename",
    "settingsfile",
    "psconsolefile",
    "encodedarguments",
    "workingdirectory",
    "wd",
    "version",
];

/// Start-Process parameters that take a value (with their aliases and the common parameters),
/// other than -FilePath and -ArgumentList. Any parameter that is not one of `START_SWITCHES`
/// is read as taking one.
const START_SWITCHES: &[&str] = &["wait", "nonewwindow", "nnw", "passthru", "loaduserprofile", "lup", "usenewenvironment", "verbose", "vb", "debug", "db", "whatif", "wi", "confirm", "cf"];
const START_VALUES: &[&str] = &[
    "credential",
    "workingdirectory",
    "redirectstandarderror",
    "redirectstandardinput",
    "redirectstandardoutput",
    "rse",
    "rsi",
    "rso",
    "verb",
    "windowstyle",
    "environment",
    "erroraction",
    "warningaction",
    "informationaction",
    "errorvariable",
    "warningvariable",
    "informationvariable",
    "outvariable",
    "outbuffer",
    "pipelinevariable",
    "progressaction",
];

/// A PowerShell CLI switch's name, lowercased, without its prefix: the CLI takes `-`, `/` or a
/// Unicode dash (U+2013 to U+2015), and a second dash (`--enc`). None for a word that is no
/// switch.
fn ps_cli_switch(word: &str) -> Option<String> {
    let mut chars = word.chars();
    let lead = chars.next()?;
    let dash = |c: char| matches!(c, '-' | '\u{2013}' | '\u{2014}' | '\u{2015}');
    if !dash(lead) && lead != '/' {
        return None;
    }
    let rest = chars.as_str();
    let rest = match rest.chars().next() {
        Some(c) if dash(lead) && dash(c) => &rest[c.len_utf8()..],
        _ => rest,
    };
    Some(rest.to_lowercase())
}

/// The lines a `cmd /c`, `cmd /k`, `powershell -Command` (or a positional command) or `pwsh -c`
/// wrapper runs, or a `start` runs, and the shell that runs each. cmd also takes the line glued
/// to the switch (`/c"..."`), a switch combined with others (`/q/c`, `/e:on/c`), and its
/// delimiters `,` `;` `=` glue a switch to the word before it (`cmd,/c,format C:`); the script
/// itself is handed on as written. `start` gives two lines under cmd, since its first word can
/// be a title whose quotes are already gone.
fn wrapped_lines(cmd: &str, args: &[String], f: Flavor) -> Vec<Wrapped> {
    match cmd {
        "cmd" => {
            let text = args.join(" ");
            let mut start = 0;
            for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
                if !matches!(c, ' ' | '\t' | ',' | ';' | '=') {
                    continue;
                }
                let word = text[start..i].to_ascii_lowercase();
                if word.starts_with('/') {
                    // Each `/` in a switch word starts a switch of its own (`/q/c`).
                    for (at, _) in word.match_indices('/') {
                        if matches!(word[at + 1..].chars().next(), Some('c' | 'k' | 'r')) {
                            let script = text[start + at + 2..].trim_start_matches([' ', '\t', ',', ';', '=']);
                            return vec![Wrapped::Line(script.to_string(), Flavor::Cmd)];
                        }
                    }
                }
                start = i + c.len_utf8();
            }
            Vec::new()
        }
        "powershell" | "pwsh" => {
            let mut lines = Vec::new();
            let mut i = 0;
            while i < args.len() {
                let Some(switch) = ps_cli_switch(&args[i]) else {
                    // The first word that is no switch is the command (Windows PowerShell's
                    // default parameter is -Command).
                    lines.push(Wrapped::Line(args[i..].join(" "), Flavor::Ps));
                    return lines;
                };
                let name = switch.split(':').next().unwrap_or("").to_string();
                let value = args[i].split_once(':').map(|(_, v)| v.to_string());
                if name.is_empty() {
                    i += 1;
                    continue;
                }
                if "command".starts_with(name.as_str()) {
                    // `-Command:x` keeps its value as the line's first word.
                    let glued = value.filter(|v| !v.is_empty());
                    lines.push(Wrapped::Line(glued.into_iter().chain(args[i + 1..].iter().cloned()).collect::<Vec<_>>().join(" "), Flavor::Ps));
                    return lines;
                }
                if "encodedcommand".starts_with(name.as_str()) || name == "ec" {
                    return vec![Wrapped::Opaque];
                }
                // A script file is not a command line.
                if "file".starts_with(name.as_str()) {
                    return lines;
                }
                // A value switch takes the next word unless its value is bound with `:`. The
                // word is also read as the command, in case PowerShell reads this abbreviation as
                // a switch with no value.
                if value.is_none() && PS_VALUE_SWITCHES.iter().any(|v| v.starts_with(name.as_str())) {
                    if i + 1 < args.len() {
                        lines.push(Wrapped::Line(args[i + 1..].join(" "), Flavor::Ps));
                    }
                    i += 1;
                }
                i += 1;
            }
            lines
        }
        "start" if f == Flavor::Cmd => {
            let mut lines = Vec::new();
            let mut i = 0;
            while i < args.len() {
                let lower = args[i].to_lowercase();
                if lower.starts_with('/') {
                    if matches!(lower.trim_start_matches('/'), "d" | "node" | "affinity") {
                        i += 1;
                    }
                } else if !lower.is_empty() {
                    lines.push(Wrapped::Line(args[i..].join(" "), f));
                    // The first word may be the window title.
                    if lines.len() == 2 {
                        break;
                    }
                }
                i += 1;
            }
            lines
        }
        "start" | "start-process" | "saps" => start_process_line(args).map_or_else(Vec::new, |line| vec![Wrapped::Line(line, f)]),
        _ => Vec::new(),
    }
}

/// The line `Start-Process` runs: -FilePath (or the first positional word) then -ArgumentList
/// (or the words after it), each named parameter wherever it stands, by any unique prefix,
/// with its value after `:` or in the next word.
fn start_process_line(args: &[String]) -> Option<String> {
    let (mut program, mut arguments): (Option<String>, Option<String>) = (None, None);
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let word = &args[i];
        // A quoted word with a space in its name (`'-enc AAAA'`) is a value, not a parameter.
        let named = |s: &&str| !s.split(':').next().unwrap_or("").is_empty() && !s.split(':').next().unwrap_or("").contains([' ', '\t']);
        let Some(switch) = word.strip_prefix('-').filter(named) else {
            if !word.is_empty() {
                positional.push(word.clone());
            }
            i += 1;
            continue;
        };
        let lower = switch.to_lowercase();
        let name = lower.split(':').next().unwrap_or("").to_string();
        let glued = switch.split_once(':').map(|(_, v)| v.to_string());
        let is = |full: &str| full.starts_with(name.as_str());
        // An exact name wins (`-Verb`, not `-Verbose`); a name that is only a switch's prefix
        // takes no value; any other (a value parameter, an ambiguous or unknown one) takes one.
        let only_switch = START_SWITCHES.contains(&name.as_str()) || (START_SWITCHES.iter().any(|s| is(s)) && !START_VALUES.iter().any(|v| is(v)));
        let takes_value = START_VALUES.contains(&name.as_str()) || !only_switch;
        let target = if is("filepath") || name == "path" || name == "pspath" {
            Some(&mut program)
        } else if is("argumentlist") || name == "args" {
            Some(&mut arguments)
        } else {
            None
        };
        match (target, glued) {
            (Some(slot), Some(v)) => *slot = Some(v),
            (Some(slot), None) => {
                *slot = args.get(i + 1).cloned();
                i += 1;
            }
            (None, None) if takes_value => i += 1,
            _ => {}
        }
        i += 1;
    }
    let mut positional = positional.into_iter();
    let program = program.or_else(|| positional.next())?;
    let rest: Vec<String> = arguments.into_iter().chain(positional).collect();
    Some(std::iter::once(program).chain(rest).collect::<Vec<_>>().join(" "))
}

pub const ESCAPE: &str = "the command contains a terminal escape sequence";
pub const CARRIAGE_RETURN: &str = "the command contains a carriage return, which the shell and the safety check tokenize differently";
pub const HISTORY: &str = "the command uses shell history expansion (`!`/`^`), which resurrects a previous command that can't be safety-checked";
pub const UNBALANCED: &str = "the command has unbalanced quotes and cannot be parsed for safety";
pub const DEVICE_REDIRECT: &str = "the command writes directly to a device";
pub const RM_ROOT: &str = "the command recursively deletes the filesystem root or home directory";
pub const CHMOD_ROOT: &str = "the command recursively changes ownership/permissions on the filesystem root or home directory";
pub const DD_DEVICE: &str = "the command writes directly to a device with dd";
pub const MKFS: &str = "the command formats a filesystem (mkfs)";
pub const FIND_ROOT: &str = "the command deletes or runs a destructive command across the filesystem root or home directory with find";
pub const WIN_DELETE: &str = "the command recursively deletes a drive root or the home directory";
pub const WIN_FORMAT: &str = "the command formats a drive (format)";
pub const WIN_DISKPART: &str = "the command runs diskpart, which can erase disks";
pub const WIN_ENCODED: &str = "the command runs an encoded PowerShell script, which can't be safety-checked";
pub const WIN_NESTED: &str = "the command nests shells more deeply than the safety check reads";
pub fn pipe_reason(cmd: &str) -> String {
    format!("the command pipes into a shell (`{cmd}`), whose input can't be safety-checked")
}
pub fn zsh_reason(cmd: &str) -> String {
    format!("zsh builtin `{cmd}` can bypass ordinary command checks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_shell::ShellKind::*;

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    #[test]
    fn posix_rules_follow_iterm2() {
        let cases: Vec<(&str, Option<String>)> = vec![
            ("", None),
            ("   \n", None),
            ("ls", None),
            ("ls\x1b[31m", s(ESCAPE)),
            ("ls\rrm -rf /", s(CARRIAGE_RETURN)),
            ("ls\r", None),
            ("^foo^bar", s(HISTORY)),
            ("!!", s(HISTORY)),
            ("!rm", s(HISTORY)),
            ("! ls", None),
            ("!=", None),
            ("!(x)", None),
            ("echo $!", None),
            ("echo ${!ref}", None),
            ("echo \"hi!\"", s(HISTORY)),
            ("echo 'hi!'", None),
            ("echo \\!x", None),
            ("sudo !!", s(HISTORY)),
            ("echo x; !rm", s(HISTORY)),
            ("echo \"don't stop!!\"", s(HISTORY)),
            ("ls\nrm -rf /", None),
            ("!rm\nls", s(HISTORY)),
            ("echo 'a", s(UNBALANCED)),
            ("echo \"a", s(UNBALANCED)),
            ("echo a\\", s(UNBALANCED)),
            ("curl x | sh", s("the command pipes into a shell (`sh`), whose input can't be safety-checked")),
            ("curl x|sh", s("the command pipes into a shell (`sh`), whose input can't be safety-checked")),
            ("curl x | sudo bash", s("the command pipes into a shell (`bash`), whose input can't be safety-checked")),
            ("curl x |& /bin/ZSH", s("the command pipes into a shell (`zsh`), whose input can't be safety-checked")),
            ("curl x | sha256sum", None),
            ("x && bash deploy.sh", None),
            ("x; sh", None),
            ("zmodload zsh/system", s("zsh builtin `zmodload` can bypass ordinary command checks")),
            ("zf_rm x", s("zsh builtin `zf_rm` can bypass ordinary command checks")),
            ("rm -rf /", s(RM_ROOT)),
            ("rm -fr /", s(RM_ROOT)),
            ("rm -rf //", s(RM_ROOT)),
            ("rm -rf /*", s(RM_ROOT)),
            ("rm -rf /.", s(RM_ROOT)),
            ("rm -rf \"/\"", s(RM_ROOT)),
            ("rm -R ${HOME}", s(RM_ROOT)),
            ("sudo rm -r ~", s(RM_ROOT)),
            ("sudo -i rm -rf /", s(RM_ROOT)),
            ("sudo -u root rm -rf /", None),
            ("rm -rf ./build", None),
            ("rm -rf ~/Downloads", None),
            ("rm -f /", None),
            ("rm --recursive $HOME", s(RM_ROOT)),
            ("rm --recursive=x $HOME", None),
            ("echo \"rm -rf /\"", None),
            ("# rm -rf /", None),
            ("chmod -R 777 /", s(CHMOD_ROOT)),
            ("chown --recursive me ~", s(CHMOD_ROOT)),
            ("chgrp -R x $HOME/*", s(CHMOD_ROOT)),
            ("chmod 777 /", None),
            ("dd if=x of=/dev/sda", s(DD_DEVICE)),
            ("dd if=x of=/dev/null", None),
            ("dd if=x of=/dev/fd/3", None),
            ("mkfs.ext4 /dev/sdb1", s(MKFS)),
            ("mkfs /dev/sdb1", s(MKFS)),
            ("find / -delete", s(FIND_ROOT)),
            ("find ~ -exec rm {} ;", s(FIND_ROOT)),
            ("find / -exec sh -c x ;", s(FIND_ROOT)),
            ("find ~ -ok /bin/shred {} ;", s(FIND_ROOT)),
            ("find ~ -exec grep x {} ;", None),
            ("find . -delete", None),
            ("find / -name x", None),
            ("echo x > /dev/sda", s(DEVICE_REDIRECT)),
            ("echo x>/dev/sda", s(DEVICE_REDIRECT)),
            ("echo x 2>>/dev/sda", s(DEVICE_REDIRECT)),
            ("echo x > \"/dev/sda\"", s(DEVICE_REDIRECT)),
            ("echo x > /dev/null", None),
            ("echo x > /dev/fd/2", None),
            ("echo \"cat foo >/dev/sda\"", None),
            ("FOO=1 env nice rm -rf /", s(RM_ROOT)),
            ("FOO=bar", None),
            ("! rm -rf /", s(RM_ROOT)),
            ("\\rm -rf /", s(RM_ROOT)),
            ("ls && rm -rf /", s(RM_ROOT)),
            ("ls || rm -rf /", s(RM_ROOT)),
            ("ls & rm -rf /", s(RM_ROOT)),
            ("ls; rm -rf /", s(RM_ROOT)),
            ("ls &> out", None),
        ];
        for (line, want) in cases {
            assert_eq!(posix(line), want, "posix({line:?})");
        }
    }

    #[test]
    fn posix_split_records_pipes() {
        let got = split_posix("a | b && c ; d |& e || f & g").unwrap();
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.trim()).collect();
        assert_eq!(texts, ["a", "b", "c", "d", "e", "f", "g"]);
        let pipes: Vec<bool> = got.iter().map(|(_, p)| *p).collect();
        assert_eq!(pipes, [false, true, false, false, true, false, false]);
        assert_eq!(split_posix("echo 'a | b'").unwrap().len(), 1);
        assert_eq!(split_posix("echo 'a"), Err(()));
    }

    #[test]
    fn windows_rules_follow_the_ruling() {
        let del = s(WIN_DELETE);
        let cmd: Vec<(&str, Option<String>)> = vec![
            ("del /s /q C:\\", del.clone()),
            ("del /S C:\\*", del.clone()),
            ("del /s/q C:\\", del.clone()),
            ("erase /s %USERPROFILE%", del.clone()),
            ("rd /s /q C:\\", del.clone()),
            ("rmdir /S D:", del.clone()),
            ("rd /s /q \"C:\\\"", del.clone()),
            ("rd /s /q %HOMEDRIVE%%HOMEPATH%", del.clone()),
            ("rd /s /q \\", del.clone()),
            ("del /q C:\\temp\\x.txt", None),
            ("del /q C:\\", None),
            ("rd /s build", None),
            ("rd /s /q C:\\build", None),
            ("format D: /q", s(WIN_FORMAT)),
            ("format.com D:", s(WIN_FORMAT)),
            ("diskpart", s(WIN_DISKPART)),
            ("DISKPART.EXE /s x.txt", s(WIN_DISKPART)),
            ("type x | cmd", s("the command pipes into a shell (`cmd`), whose input can't be safety-checked")),
            ("type x | cmd.exe", s("the command pipes into a shell (`cmd`), whose input can't be safety-checked")),
            ("type x | powershell", s("the command pipes into a shell (`powershell`), whose input can't be safety-checked")),
            ("type x & cmd", None),
            ("dir & rd /s /q C:\\", del.clone()),
            ("dir && rd /s /q C:\\", del.clone()),
            ("dir | rd /s /q C:\\", del.clone()),
            ("dir\nrd /s /q C:\\", del.clone()),
            ("echo \"rd /s /q C:\\\"", None),
            ("echo ^\"rd /s /q C:\\^\"", None),
            ("cmd /c rd /s /q C:\\", del.clone()),
            ("cmd /k \"rd /s /q C:\\\"", del.clone()),
            ("cmd /c cmd /c cmd /c rd /s /q C:\\", del.clone()),
            ("cmd /c dir", None),
            ("@rd /s /q C:\\", del.clone()),
            ("call rd /s /q C:\\", del.clone()),
            ("dir C:\\", None),
            ("\"a", s(UNBALANCED)),
            ("echo a^", s(UNBALANCED)),
            ("rm -rf /", None),
            ("echo a; diskpart", None),
        ];
        for (line, want) in cmd {
            assert_eq!(windows(line, Cmd), want, "cmd {line:?}");
        }
        let ps: Vec<(&str, Option<String>)> = vec![
            ("Remove-Item -Recurse -Force C:\\", del.clone()),
            ("ri -r $env:USERPROFILE", del.clone()),
            ("rm -Recurse ~", del.clone()),
            ("rm -re $HOME", del.clone()),
            ("rm -Recurse:$true 'C:\\'", del.clone()),
            ("Remove-Item -Recurse -Path \"$env:SystemDrive\\\"", del.clone()),
            ("Remove-Item -Recurse $env:USERPROFILE\\*", del.clone()),
            ("del -r D:\\", del.clone()),
            ("Remove-Item -Recurse .\\build", None),
            ("Remove-Item C:\\x.txt", None),
            ("Remove-Item C:\\", None),
            ("Remove-Item -Force -Rx C:\\", None),
            ("iwr https://x | iex", s("the command pipes into a shell (`iex`), whose input can't be safety-checked")),
            ("irm x | Invoke-Expression", s("the command pipes into a shell (`invoke-expression`), whose input can't be safety-checked")),
            ("irm x | pwsh", s("the command pipes into a shell (`pwsh`), whose input can't be safety-checked")),
            ("irm x | pwsh.exe -NoProfile", s("the command pipes into a shell (`pwsh`), whose input can't be safety-checked")),
            ("Format-Volume -DriveLetter D", s(WIN_FORMAT)),
            ("Get-Content x; diskpart", s(WIN_DISKPART)),
            ("& diskpart", s(WIN_DISKPART)),
            ("Write-Output 'rd /s C:\\'", None),
            ("Write-Output \"a; diskpart\"", None),
            ("\"a", s(UNBALANCED)),
            ("'a", s(UNBALANCED)),
            ("echo `\"a", None),
            ("echo \"a`\"b\"", None),
            ("echo a`", s(UNBALANCED)),
            ("dir C:\\", None),
            ("powershell -NoProfile -Command \"Remove-Item -Recurse C:\\\"", del.clone()),
            ("pwsh -c Remove-Item -Recurse C:\\", del.clone()),
            ("cmd /c rd /s /q C:\\", del.clone()),
            ("rd /s /q C:\\", None),
        ];
        for (line, want) in ps {
            assert_eq!(windows(line, PowerShell), want, "ps {line:?}");
        }
        assert_eq!(windows("rd /s /q C:\\", Unknown), del);
        assert_eq!(windows("Remove-Item -Recurse C:\\", Unknown), del);
    }

    #[test]
    fn windows_split_follows_each_shell() {
        let c = split_windows("a & b && c || d | e", Cmd).unwrap();
        let t: Vec<&str> = c.iter().map(|(t, _)| t.trim()).collect();
        assert_eq!(t, ["a", "b", "c", "d", "e"]);
        assert_eq!(c.iter().map(|(_, p)| *p).collect::<Vec<_>>(), [false, false, false, false, true]);
        assert_eq!(split_windows("a; b", Cmd).unwrap().len(), 1);
        let p = split_windows("a; b && c | d", PowerShell).unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(split_windows("echo 'a; b'", PowerShell).unwrap().len(), 1);
        assert_eq!(split_windows("echo 'a; b'", Cmd).unwrap().len(), 1);
        assert_eq!(split_windows("\"a", Cmd), Err(()));
    }

    /// Task 9 review, Important 1 to 3: the Windows set normalizes before matching; every form
    /// the review listed, one line each. `del` is the drive-or-home reason.
    #[test]
    fn windows_floor_survives_rewrites() {
        let del = s(WIN_DELETE);
        let mut miss: Vec<String> = Vec::new();
        let mut expect = |label: &str, line: &str, got: Option<String>, want: &Option<String>| {
            if &got != want {
                miss.push(format!("{label} {line:?}: got {got:?}"));
            }
        };
        let cmd_del = [
            "del /s /q C:\\*.*",
            "del /s /q %USERPROFILE%\\*.*",
            "rd /s /q C:\\.",
            "rd /s /q C:\\..",
            "rd/s/q C:\\",
            "del/s/q C:\\*",
            "(rd /s /q C:\\)",
            "if exist C:\\ rd /s /q C:\\",
            "if not exist C:\\x rd /s /q C:\\",
            "for %i in (1) do rd /s /q C:\\",
            "rd /s /q ^\nC:\\",
            "rd,/s,/q,C:\\",
            "rd /s /q=C:\\",
            "rd /s /q \\\\?\\C:\\",
            "cmd /c\"rd /s /q C:\\\"",
            "cmd /c (rd /s /q C:\\)",
        ];
        for line in cmd_del {
            expect("cmd", line, windows(line, Cmd), &del);
            expect("unknown", line, windows(line, Unknown), &del);
        }
        let cmd_none = ["rd /s /q C:\\build\\.", "echo (rd /s /q C:\\build)", "if exist C:\\x echo hi", "dotnet format", "for %i in (1) do echo %i", "rd /s /q C:\\a,b"];
        for line in cmd_none {
            expect("cmd", line, windows(line, Cmd), &None);
        }
        expect("form", "if exist x format C:", windows("if exist x format C:", Cmd), &s(WIN_FORMAT));
        expect("form", "for %i in (1) do format C:", windows("for %i in (1) do format C:", Cmd), &s(WIN_FORMAT));
        expect("form", "(diskpart)", windows("(diskpart)", Cmd), &s(WIN_DISKPART));

        let ps_del = [
            "Remove-Item -Recurse ${env:USERPROFILE}",
            "Remove-Item -Recurse ${env:HOMEDRIVE}",
            "Remove-Item -Recurse ${HOME}",
            "Remove-Item -Recurse -Path:C:\\",
            "Remove-Item -Recurse -LiteralPath:C:\\",
            "Remove-Item -Recurse -Path:'C:\\'",
            "Remove-Item -Recurse \\\\?\\C:\\",
            "Remove-Item -Recurse C:\\*.*",
            "Remove-Item -Recurse C:\\.",
            "Remove-Item -Recurse C:\\..",
            "& {Remove-Item -Recurse C:\\}",
            "(Remove-Item -Recurse C:\\)",
            "Remove-Item -Recurse (Get-Item C:\\)",
            "powershell -c {Remove-Item -Recurse C:\\}",
            "if ($true) { Remove-Item -Recurse C:\\ }",
            "Remove-Item -Recurse `\nC:\\",
            "Remove-Item \u{2013}Recurse C:\\",
            "Remove-Item \u{2014}r C:\\",
            "Remove-Item \u{2015}Recurse C:\\",
            "Remove-Item -Recurse \u{201c}C:\\\u{201d}",
            "Remove-Item -Recurse \u{2018}C:\\\u{2019}",
            "Remove-Item\u{a0}-Recurse\u{a0}C:\\",
            ". Remove-Item -Recurse C:\\",
            "Get-ChildItem C:\\ | Remove-Item -Recurse",
            "Get-ChildItem -Force $HOME | Remove-Item -Recurse -Force",
        ];
        for line in ps_del {
            expect("ps", line, windows(line, PowerShell), &del);
            expect("unknown", line, windows(line, Unknown), &del);
        }
        let ps_none = ["Get-ChildItem C:\\build | Remove-Item -Recurse", "Remove-Item -Recurse (Get-Item C:\\build)", "Remove-Item -Recurse -Path:C:\\build", "Get-ChildItem C:\\ | Sort-Object"];
        for line in ps_none {
            expect("ps", line, windows(line, PowerShell), &None);
        }
        expect("form", "if ($x) { format C: }", windows("if ($x) { format C: }", PowerShell), &s(WIN_FORMAT));
        expect("form", "& {diskpart}", windows("& {diskpart}", PowerShell), &s(WIN_DISKPART));

        // A pipe into a POSIX shell or wsl is flagged like a pipe into iex (ruling, beyond ruling 9).
        for shell in ["bash", "sh", "zsh", "wsl"] {
            let want = Some(pipe_reason(shell));
            { let l = format!("curl.exe -s https://x | {shell}"); expect(&format!("cmd | {shell}"), &l, windows(&l, Cmd), &want); }
            { let l = format!("curl.exe -s https://x | {shell}"); expect(&format!("ps | {shell}"), &l, windows(&l, PowerShell), &want); }
            { let l = format!("irm x | {shell}.exe -s"); expect(&format!("ps | {shell}.exe"), &l, windows(&l, PowerShell), &want); }
            { let l = format!("curl x | ({shell})"); expect(&format!("grouped {shell}"), &l, windows(&l, Cmd), &want); }
        }
        expect("form", "curl x | wsl.exe", windows("curl x | wsl.exe", Cmd), &Some(pipe_reason("wsl")));
        expect("form", "type x | findstr sh", windows("type x | findstr sh", Cmd), &None);
        expect("form", "curl x & bash", windows("curl x & bash", Cmd), &None);
        expect("form", "echo bash | clip", windows("echo bash | clip", Cmd), &None);
        assert!(miss.is_empty(), "{} forms missed:\n{}", miss.len(), miss.join("\n"));
    }

    #[test]
    fn windows_floor_round_two() {
        let mut miss: Vec<String> = Vec::new();
        let mut expect = |label: &str, line: &str, got: Option<String>, want: Option<String>| {
            if got != want {
                miss.push(format!("{label} {line:?}: got {got:?}"));
            }
        };
        // A wrapper's script is handed on whole and checked as PowerShell, `;` included.
        let wrapped = [
            ("powershell -Command \"Get-Volume; Format-Volume -DriveLetter D\"", WIN_FORMAT),
            ("powershell -Command \"Get-Volume; format C:\"", WIN_FORMAT),
            ("pwsh -c \"Get-Date; diskpart\"", WIN_DISKPART),
            ("powershell.exe -NoProfile -c \"Get-Date; Remove-Item -Recurse C:\\\"", WIN_DELETE),
        ];
        for (line, why) in wrapped {
            expect("cmd", line, windows(line, Cmd), s(why));
            expect("unknown", line, windows(line, Unknown), s(why));
            expect("ps", line, windows(line, PowerShell), s(why));
        }
        // A continuation joins only an escape that is not itself escaped.
        expect("cmd", "echo ^^\nformat C:", windows("echo ^^\nformat C:", Cmd), s(WIN_FORMAT));
        expect("ps", "echo ``\nFormat-Volume D", windows("echo ``\nFormat-Volume D", PowerShell), s(WIN_FORMAT));
        expect("cmd", "rd /s /q ^\nC:\\", windows("rd /s /q ^\nC:\\", Cmd), s(WIN_DELETE));
        expect("ps", "Remove-Item -Recurse `\nC:\\", windows("Remove-Item -Recurse `\nC:\\", PowerShell), s(WIN_DELETE));
        // PowerShell reads U+2028 and U+2029 as whitespace, not as line breaks.
        for sep in ['\u{2028}', '\u{2029}'] {
            let line = format!("Remove-Item -Recurse{sep}C:\\");
            expect("ps", &line, windows(&line, PowerShell), s(WIN_DELETE));
        }
        // A filter stage keeps the pipeline's target.
        let filtered = "Get-ChildItem C:\\ | Where-Object { $_.Name } | Remove-Item -Recurse";
        expect("ps", filtered, windows(filtered, PowerShell), s(WIN_DELETE));
        let build = "Get-ChildItem C:\\build | Where-Object { $_.Name } | Remove-Item -Recurse";
        expect("ps", build, windows(build, PowerShell), None);
        assert!(miss.is_empty(), "{} forms missed:\n{}", miss.len(), miss.join("\n"));
    }

    #[test]
    fn windows_floor_round_three() {
        let mut miss: Vec<String> = Vec::new();
        let mut expect = |label: &str, line: &str, got: Option<String>, want: Option<&str>| {
            if got.as_deref() != want {
                miss.push(format!("{label} {line:?}: got {got:?}, want {want:?}"));
            }
        };
        // Item 7: cmd's own delimiters glue the switch and the script (both Cmd and Unknown).
        let glued = [
            ("cmd,/c,format C:", WIN_FORMAT),
            ("cmd /c,format C:", WIN_FORMAT),
            ("cmd;/c;diskpart", WIN_DISKPART),
            ("cmd=/c format C:", WIN_FORMAT),
            ("cmd /c;;format C:", WIN_FORMAT),
            ("cmd.exe,/k,diskpart", WIN_DISKPART),
        ];
        for (line, why) in glued {
            expect("cmd", line, windows(line, Cmd), Some(why));
            expect("unknown", line, windows(line, Unknown), Some(why));
        }
        expect("cmd", "cmd /c echo hi", windows("cmd /c echo hi", Cmd), None);
        expect("cmd", "cmd,/c,dir", windows("cmd,/c,dir", Cmd), None);
        // Item 10: a leading delimiter before a cmd command.
        for (line, why) in [(",format C:", WIN_FORMAT), (";diskpart", WIN_DISKPART), ("=format C:", WIN_FORMAT), (" , format C:", WIN_FORMAT), (",;rd /s /q C:\\", WIN_DELETE)] {
            expect("cmd", line, windows(line, Cmd), Some(why));
        }
        // Item 10: a positional PowerShell command, and an encoded one always.
        for (line, why) in [
            ("powershell format C:", WIN_FORMAT),
            ("powershell \"Get-Volume; format C:\"", WIN_FORMAT),
            ("pwsh -NoProfile -ExecutionPolicy Bypass diskpart", WIN_DISKPART),
            ("powershell.exe -NoLogo Remove-Item -Recurse C:\\", WIN_DELETE),
            ("powershell -EncodedCommand ZgBvAHIAbQBhAHQA", WIN_ENCODED),
            ("powershell -enc AAAA", WIN_ENCODED),
            ("pwsh -e AAAA", WIN_ENCODED),
            ("powershell -ec AAAA", WIN_ENCODED),
            ("powershell -NoProfile -EncodedCommand AAAA", WIN_ENCODED),
        ] {
            for k in [Cmd, PowerShell, Unknown] {
                expect(&format!("{k:?}"), line, windows(line, k), Some(why));
            }
        }
        for line in ["powershell -File x.ps1", "powershell -NoProfile Get-Date", "pwsh -ExecutionPolicy Bypass -File a.ps1 format"] {
            expect("cmd", line, windows(line, Cmd), None);
        }
        // Item 10: `start` runs its command.
        for (line, why) in [
            ("start diskpart", WIN_DISKPART),
            ("start /b format C:", WIN_FORMAT),
            ("start \"\" diskpart", WIN_DISKPART),
            ("start \"title\" /min cmd /c format C:", WIN_FORMAT),
            ("start /d C:\\x diskpart", WIN_DISKPART),
            ("start /wait rd /s /q C:\\", WIN_DELETE),
        ] {
            expect("cmd", line, windows(line, Cmd), Some(why));
        }
        expect("cmd", "start notepad", windows("start notepad", Cmd), None);
        expect("cmd", "start \"\" notepad C:\\x", windows("start \"\" notepad C:\\x", Cmd), None);
        expect("ps", "start diskpart", windows("start diskpart", PowerShell), Some(WIN_DISKPART));
        expect("ps", "Start-Process diskpart", windows("Start-Process diskpart", PowerShell), Some(WIN_DISKPART));
        expect("ps", "start notepad", windows("start notepad", PowerShell), None);
        // Item 10: ForEach-Object over a pipeline that named a target.
        for (line, why) in [
            ("Get-ChildItem C:\\ | ForEach-Object { Remove-Item $_ -Recurse }", Some(WIN_DELETE)),
            ("Get-ChildItem C:\\ | % { Remove-Item $_ -Recurse }", Some(WIN_DELETE)),
            ("Get-ChildItem C:\\ | foreach { rm $_ -r }", Some(WIN_DELETE)),
            ("Get-ChildItem C:\\build | ForEach-Object { Remove-Item $_ -Recurse }", None),
            ("Get-ChildItem C:\\ | ForEach-Object { Write-Host $_ }", None),
        ] {
            expect("ps", line, windows(line, PowerShell), why);
        }
        // Item 8: an `if` condition's target does not carry into a later pipeline.
        let cond = "if (Test-Path C:\\) { Get-ChildItem .\\tmp | Remove-Item -Recurse }";
        expect("ps", cond, windows(cond, PowerShell), None);
        let group = "(Get-ChildItem C:\\) | Remove-Item -Recurse";
        expect("ps", group, windows(group, PowerShell), Some(WIN_DELETE));
        let filtered = "Get-ChildItem C:\\ | Where-Object { $_.Name } | Remove-Item -Recurse";
        expect("ps", filtered, windows(filtered, PowerShell), Some(WIN_DELETE));
        assert!(miss.is_empty(), "{} forms missed:\n{}", miss.len(), miss.join("\n"));
    }

    #[test]
    fn windows_floor_final_review() {
        let mut miss: Vec<String> = Vec::new();
        let mut expect = |label: &str, line: &str, got: Option<String>, want: Option<&str>| {
            if got.as_deref() != want {
                miss.push(format!("{label} {line:?}: got {got:?}, want {want:?}"));
            }
        };
        // (a) The PowerShell CLI takes `/`, a Unicode dash and a doubled dash as a switch prefix.
        // (b) A colon-bound value takes no next word, and `-Command:` keeps its value.
        // (c) The value switches PowerShell 5.1 and pwsh name.
        for (line, why) in [
            ("powershell /c diskpart", WIN_DISKPART),
            ("powershell /Command \"format C:\"", WIN_FORMAT),
            ("powershell /enc AAAA", WIN_ENCODED),
            ("powershell /NoProfile /EncodedCommand AAAA", WIN_ENCODED),
            ("powershell \u{2013}enc AAAA", WIN_ENCODED),
            ("powershell \u{2014}c diskpart", WIN_DISKPART),
            ("powershell \u{2015}NoProfile \u{2013}c diskpart", WIN_DISKPART),
            ("pwsh --enc AAAA", WIN_ENCODED),
            ("pwsh --command diskpart", WIN_DISKPART),
            ("powershell -ExecutionPolicy:Bypass format C:", WIN_FORMAT),
            ("powershell -WindowStyle:Hidden diskpart", WIN_DISKPART),
            ("powershell -Command:diskpart", WIN_DISKPART),
            ("powershell -c:\"format C:\"", WIN_FORMAT),
            ("powershell -PSConsoleFile x diskpart", WIN_DISKPART),
            ("pwsh -SettingsFile x diskpart", WIN_DISKPART),
            ("pwsh -ConfigurationFile x diskpart", WIN_DISKPART),
            ("pwsh -CustomPipeName x diskpart", WIN_DISKPART),
            ("pwsh -EncodedArguments x diskpart", WIN_DISKPART),
        ] {
            for k in [Cmd, PowerShell, Unknown] {
                expect(&format!("{k:?}"), line, windows(line, k), Some(why));
            }
        }
        for line in ["powershell /NoProfile /File x.ps1", "powershell -ExecutionPolicy:Bypass -File a.ps1", "pwsh -SettingsFile x.json -NoProfile Get-Date"] {
            for k in [Cmd, PowerShell] {
                expect(&format!("{k:?}"), line, windows(line, k), None);
            }
        }
        // (c) Start-Process named parameters anywhere, by any unique prefix.
        for (line, why) in [
            ("Start-Process -ArgumentList '/c format C:' cmd", WIN_FORMAT),
            ("Start-Process -ArgumentList '/c format C:' -FilePath cmd", WIN_FORMAT),
            ("Start-Process -Args '/c diskpart' -Path cmd.exe", WIN_DISKPART),
            ("Start-Process -wo C:\\ diskpart", WIN_DISKPART),
            ("Start-Process -Credential $c diskpart", WIN_DISKPART),
            ("Start-Process -RedirectStandardOutput x.txt diskpart", WIN_DISKPART),
            ("Start-Process -rso x.txt -Wait -NoNewWindow diskpart", WIN_DISKPART),
            ("saps -Verb RunAs -FilePath:powershell -ArgumentList:'-enc AAAA'", WIN_ENCODED),
            ("start -WindowStyle Hidden cmd '/c format C:'", WIN_FORMAT),
        ] {
            expect("ps", line, windows(line, PowerShell), Some(why));
        }
        for line in ["Start-Process -Wait notepad", "Start-Process -ArgumentList C:\\x notepad", "Start-Process -wo C:\\ notepad"] {
            expect("ps", line, windows(line, PowerShell), None);
        }
        // (d) cmd's combined switches.
        for (line, why) in [("cmd /q/c format C:", WIN_FORMAT), ("cmd /d/q/c diskpart", WIN_DISKPART), ("cmd /e:on/c format C:", WIN_FORMAT), ("cmd /Q/K diskpart", WIN_DISKPART), ("cmd /v:on/r format C:", WIN_FORMAT)] {
            expect("cmd", line, windows(line, Cmd), Some(why));
            expect("unknown", line, windows(line, Unknown), Some(why));
        }
        expect("cmd", "cmd /q/c echo hi", windows("cmd /q/c echo hi", Cmd), None);
        // (e) Past the nesting limit the line is flagged, never passed.
        let deep = "cmd /c cmd /c cmd /c cmd /c format C:";
        expect("cmd", deep, windows(deep, Cmd), Some(WIN_NESTED));
        let deep_echo = "cmd /c cmd /c cmd /c cmd /c echo hi";
        expect("cmd", deep_echo, windows(deep_echo, Cmd), Some(WIN_NESTED));
        let deep_ps = "powershell -c powershell -c powershell -c powershell -c Get-Date";
        expect("ps", deep_ps, windows(deep_ps, PowerShell), Some(WIN_NESTED));
        let within = "cmd /c cmd /c cmd /c echo hi";
        expect("cmd", within, windows(within, Cmd), None);
        let within_bad = "cmd /c cmd /c cmd /c format C:";
        expect("cmd", within_bad, windows(within_bad, Cmd), Some(WIN_FORMAT));
        assert!(miss.is_empty(), "{} forms missed:\n{}", miss.len(), miss.join("\n"));
    }

    #[test]
    fn check_picks_the_sets() {
        assert_eq!(check("rm -rf /", Posix), s(RM_ROOT));
        assert_eq!(check("rd /s /q C:", Posix), None);
        assert_eq!(check("rd /s /q C:\\", Posix), s(UNBALANCED));
        assert_eq!(check("rd /s /q C:\\", Cmd), s(WIN_DELETE));
        assert_eq!(check("rm -rf /", Cmd), None);
        assert_eq!(check("rm -rf /", Unknown), s(RM_ROOT));
        assert_eq!(check("rd /s /q C:", Unknown), s(WIN_DELETE));
        assert_eq!(check("Remove-Item -Recurse C:", Unknown), s(WIN_DELETE));
        for k in [Cmd, PowerShell, Posix, Unknown] {
            assert_eq!(check("ls\x1b[31m", k), s(ESCAPE));
            assert_eq!(check("ls\rrm", k), s(CARRIAGE_RETURN));
            assert_eq!(check("  ", k), None);
        }
        assert_eq!(check("rm -rf /", PowerShell), None);
        assert_eq!(check("ls", Unknown), None);
    }
}
