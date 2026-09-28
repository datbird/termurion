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

const WIN_PIPE_SHELLS: &[&str] = &["iex", "invoke-expression", "powershell", "pwsh", "cmd"];
const PS_REMOVE: &[&str] = &["remove-item", "ri", "rm", "rmdir", "rd", "del", "erase"];
const WIN_TARGETS: &[&str] = &[
    "~",
    "$home",
    "${home}",
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
        let mut split = |after_pipe: bool, cur: &mut String, cur_pipe: &mut bool| {
            segs.push((std::mem::take(cur), *cur_pipe));
            *cur_pipe = after_pipe;
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
                        split(false, &mut cur, &mut cur_pipe);
                        i += 1;
                    } else {
                        split(true, &mut cur, &mut cur_pipe);
                    }
                }
                '&' if f == Flavor::Cmd => {
                    split(false, &mut cur, &mut cur_pipe);
                    if next == Some('&') {
                        i += 1;
                    }
                }
                '&' if next == Some('&') => {
                    split(false, &mut cur, &mut cur_pipe);
                    i += 1;
                }
                ';' if f == Flavor::Ps => split(false, &mut cur, &mut cur_pipe),
                '\n' => split(false, &mut cur, &mut cur_pipe),
                _ => cur.push(c),
            }
        }
        i += 1;
    }
    segs.push((cur, cur_pipe));
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

/// The effective command: a leading `@` (cmd), `&` (PowerShell's call operator) and `call` skipped.
fn win_effective_command(words: &[String]) -> Option<(String, Vec<String>)> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_start_matches('@');
        if w.is_empty() || w == "&" || w.eq_ignore_ascii_case("call") {
            i += 1;
            continue;
        }
        return Some((win_command_name(w), words[i + 1..].to_vec()));
    }
    None
}

/// A drive root or the home directory in any of its spellings, optionally with a trailing
/// separator, `*`, `\*` or `/*`.
fn is_win_target(token: &str) -> bool {
    let lower = token.to_lowercase();
    let mut s = lower.as_str();
    s = s.strip_suffix('*').unwrap_or(s);
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return s.len() > trimmed.len();
    }
    let b: Vec<char> = trimmed.chars().collect();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == ':' {
        return true;
    }
    WIN_TARGETS.contains(&trimmed)
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

fn windows_flavor(line: &str, f: Flavor, depth: usize) -> Option<String> {
    let segs = match scan_win(line, f) {
        Ok(s) => s,
        Err(()) => return Some(UNBALANCED.to_string()),
    };
    for (text, after_pipe) in &segs {
        let words = tokenize_win(text, f);
        let Some((cmd, args)) = win_effective_command(&words) else { continue };
        if let Some(r) = windows_verdict(&cmd, &args, *after_pipe, f, depth) {
            return Some(r);
        }
    }
    None
}

fn windows_verdict(cmd: &str, args: &[String], after_pipe: bool, f: Flavor, depth: usize) -> Option<String> {
    if after_pipe && WIN_PIPE_SHELLS.contains(&cmd) {
        return Some(pipe_reason(cmd));
    }
    let delete = match f {
        Flavor::Cmd => match cmd {
            "del" | "erase" | "rd" | "rmdir" => {
                has_slash_s(args)
            }
            _ => false,
        },
        Flavor::Ps => PS_REMOVE.contains(&cmd) && has_ps_recurse(args),
    };
    if delete && args.iter().any(|a| is_win_target(a)) {
        return Some(WIN_DELETE.to_string());
    }
    if cmd == "format" || cmd == "format-volume" {
        return Some(WIN_FORMAT.to_string());
    }
    if cmd == "diskpart" {
        return Some(WIN_DISKPART.to_string());
    }
    if depth > 1 {
        if let Some((inner, flavor)) = wrapped_line(cmd, args) {
            return windows_flavor(&inner, flavor, depth - 1);
        }
    }
    None
}

/// The line a `cmd /c`, `cmd /k`, `powershell -Command` or `pwsh -c` wrapper runs, and the shell
/// that runs it.
fn wrapped_line(cmd: &str, args: &[String]) -> Option<(String, Flavor)> {
    let (flag, flavor): (fn(&str) -> bool, Flavor) = match cmd {
        "cmd" => (|a| a == "/c" || a == "/k" || a == "/r", Flavor::Cmd),
        "powershell" | "pwsh" => (|a| a.len() >= 2 && a.starts_with('-') && "-command".starts_with(a), Flavor::Ps),
        _ => return None,
    };
    let at = args.iter().position(|a| flag(&a.to_lowercase()))?;
    let rest = args[at + 1..].join(" ");
    Some((rest, flavor))
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
