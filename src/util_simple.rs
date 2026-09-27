//! it2copy, it2setcolor, it2attention, it2universion and it2setkeylabel (milestone 11e):
//! iTerm2's one-sequence utilities (`OtherResources/Utilities/`), byte for byte. Pure: files
//! through `Io.cwd`, `$RANDOM` through `Io.random`.

use std::io::Write;

use crate::util_cli::{self, Io};
use crate::util_files::{Found, find};
use crate::util_osc::{b64_line, fold, osc, seq, st};

fn put(w: &mut dyn Write, s: &str) {
    let _ = w.write_all(s.as_bytes());
}

fn lines(v: &[&str]) -> String {
    v.iter().map(|l| format!("{l}\n")).collect()
}

/// `$RANDOM$RANDOM`: two values of 0 to 32767 written one after the other.
pub fn copy_uid(a: u32, b: u32) -> String {
    format!("{}{}", a % 32768, b % 32768)
}

/// it2copy's `show_help` (to stderr).
fn copy_help(name: &str) -> String {
    lines(&[
        &format!("Usage: {name}"),
        "          Copies to clipboard from standard input",
        &format!("       {name} filename"),
        "          Copies to clipboard from file",
    ])
}

/// Arms (or clears) the ST the Ctrl+C handler writes while an OSC is open: the script's
/// `inosc` and `trap clean_up INT`.
fn arm_st(io: &mut Io, pending: Option<&'static str>) {
    // The handler writes to the console handle directly: what is buffered goes first.
    let _ = io.out.flush();
    #[cfg(windows)]
    crate::util_console::set_pending_st(pending);
    #[cfg(not(windows))]
    let _ = pending;
}

/// `print_osc; inosc=1; printf <body>; print_st; inosc=0`, one sequence at a time.
fn send_one(io: &mut Io, term: Option<&str>, body: &str) {
    put(io.out, osc(term));
    arm_st(io, Some(st(term)));
    put(io.out, body);
    put(io.out, st(term));
    arm_st(io, None);
}

/// `base64 [< file] | send`: the OSC head is printed before the data is read, as the
/// script's pipeline does; `read` yields the bytes (None: the read failed, which sends
/// nothing, as base64 then writes nothing).
fn send(io: &mut Io, term: Option<&str>, read: &mut dyn FnMut(&mut Io) -> Option<Vec<u8>>) {
    // macOS `base64` writes one line and its newline; empty input writes nothing.
    let encode = |data: Option<Vec<u8>>| data.filter(|d| !d.is_empty()).map_or(String::new(), |d| b64_line(&d));
    if term.is_some_and(|t| t.starts_with("tmux")) {
        // `send_tmux`: the uid, then one `fold` line (80 columns) per piece.
        let uid = copy_uid((io.random)(), (io.random)());
        send_one(io, term, &format!("1337;Copy=2;{uid}"));
        let data = encode(read(io));
        for line in fold(&data, 80) {
            send_one(io, term, &format!("1337;Copy=3;{uid}:{line}"));
        }
        send_one(io, term, &format!("1337;Copy=4;{uid}"));
        return;
    }
    // `send_regular`: the piped base64 through `cat`, its newline kept before the ST.
    put(io.out, osc(term));
    put(io.out, "1337;Copy=:");
    arm_st(io, Some(st(term)));
    let data = encode(read(io));
    put(io.out, &data);
    put(io.out, st(term));
    arm_st(io, None);
}

/// `it2copy`: standard input, or the first argument's file, to the clipboard.
pub fn it2copy(args: &[String], io: &mut Io) -> i32 {
    let term = util_cli::term(io);
    let term = term.as_deref();
    if let Some(a) = args.first() {
        match a.as_str() {
            "-h" | "--h" | "--help" => {
                put(io.err, &copy_help("it2copy"));
                return 0;
            }
            _ if a.starts_with('-') => {
                put(io.err, &format!("ERROR: Unknown option flag: {a}\n"));
                put(io.err, &copy_help("it2copy"));
                return 1;
            }
            _ => {}
        }
        return match find(&io.cwd, a) {
            Found::File(bytes) => {
                let mut bytes = Some(bytes);
                send(io, term, &mut |_| bytes.take());
                0
            }
            // `[ -r dir ]` holds; `base64 < dir` then fails to read and writes nothing.
            Found::Dir(_) => {
                send(io, term, &mut |io| {
                    put(io.err, "base64: read error: Is a directory\n");
                    None
                });
                0
            }
            Found::Missing => {
                put(io.err, &format!("ERROR: it2copy: {a}: No such file or directory\n"));
                2
            }
        };
    }
    send(io, term, &mut |io| {
        let mut data = Vec::new();
        io.input.read_to_end(&mut data).ok().map(|_| data)
    });
    0
}

/// it2setcolor's help up to the line that goes to stderr.
const SETCOLOR_HEAD: &[&str] = &["Usage", "", "1) To set a specific color to an RGB value:"];

/// it2setcolor's help after that line.
const SETCOLOR_TAIL: &[&str] = &[
    "For example:",
    "  it2setcolor fg fff",
    "",
    "name is one of:",
    "  fg bg bold link selbg selfg curbg curfg underline tab",
    "  black red green yellow blue magenta cyan white",
    "  br_black br_red br_green br_yellow br_blue br_magenta br_cyan br_white",
    "",
    "color is of the format:",
    "  RGB        (three hex digits, like fff)",
    "  RRGGBB     (six hex digits, like f0f0f0)",
    "  cs:RGB     (cs is a color space name)",
    "  cs:RRGGBB  (cs is a color space name)",
    "",
    "  The color space names accepted in the color are:",
    "    srgb       (sRGB, the default if none is specified)",
    "    rgb        (Apple's old device-independent colorspace)",
    "    p3         (Apple's fancy large-gamut colorspace)",
    "",
    "2) To switch to a named color preset:",
    "  it2setcolor preset name",
    "For example:",
    "  it2setcolor preset 'Light Background'",
    "",
    "3) To reset the current tab's color to the system default:",
    "  it2setcolor tab default",
];

/// The one line of it2setcolor's help that goes to stderr.
pub const SETCOLOR_STDERR_LINE: &str = "  it2setcolor name color [name color...]\n";

/// it2setcolor's help, the part on stdout (everything but `SETCOLOR_STDERR_LINE`).
pub fn setcolor_help() -> String {
    lines(SETCOLOR_HEAD) + &lines(SETCOLOR_TAIL)
}

/// it2setcolor's `show_help`: the ST first when the sequence is open, then the help, one
/// line of it on stderr.
fn setcolor_show_help(io: &mut Io, term: Option<&str>, open: bool) {
    if open {
        put(io.out, st(term));
    }
    put(io.out, &lines(SETCOLOR_HEAD));
    let _ = io.out.flush();
    put(io.err, SETCOLOR_STDERR_LINE);
    put(io.out, &lines(SETCOLOR_TAIL));
}

/// `it2setcolor`: every `name color` pair in one `SetColors=` sequence.
pub fn it2setcolor(args: &[String], io: &mut Io) -> i32 {
    let term = util_cli::term(io);
    let term = term.as_deref();
    if args.is_empty() {
        setcolor_show_help(io, term, false);
        return 0;
    }
    let mut open = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--h" | "--help" => {
                setcolor_show_help(io, term, open);
                return 0;
            }
            _ if a.starts_with('-') => {
                put(io.err, &format!("ERROR: Unknown option flag: {a}\n"));
                setcolor_show_help(io, term, open);
                return 1;
            }
            _ => {
                let Some(value) = args.get(i + 1) else {
                    setcolor_show_help(io, term, open);
                    return 0;
                };
                if open {
                    put(io.out, ",");
                } else {
                    open = true;
                    put(io.out, osc(term));
                    put(io.out, "1337;SetColors=");
                }
                // Names are not checked, so future colors work too.
                put(io.out, &format!("{a}={value}"));
                i += 1;
            }
        }
        i += 1;
    }
    if open {
        put(io.out, st(term));
    } else {
        setcolor_show_help(io, term, false);
    }
    0
}

/// it2attention's `show_help` (to stderr).
pub fn attention_help(name: &str) -> String {
    lines(&[
        "Usage:",
        &format!("  {name} start"),
        "    Begin bouncing the dock icon if another app is active",
        &format!("  {name} stop"),
        "    Stop bouncing the dock icon if another app is active",
        &format!("  {name} once"),
        "    Bounce the dock icon once if another app is active",
        &format!("  {name} fireworks"),
        "    Show an explosion animation at the cursor",
    ])
}

/// `it2attention start|stop|once|fireworks`.
pub fn it2attention(args: &[String], io: &mut Io) -> i32 {
    let term = util_cli::term(io);
    let value = match args.first().map(String::as_str) {
        Some("start") => "1",
        Some("stop") => "0",
        Some("once") => "once",
        Some("fireworks") => "fireworks",
        _ => {
            put(io.err, &attention_help("it2attention"));
            return 1;
        }
    };
    put(io.out, &seq(term.as_deref(), &format!("1337;RequestAttention={value}")));
    0
}

/// it2universion's `show_help` (to stderr).
pub fn universion_help(name: &str) -> String {
    lines(&[
        "Usage:",
        &format!("  {name} set 8"),
        &format!("  {name} set 9"),
        &format!("  {name} push [name]"),
        "     Saves the current version with an optional name.",
        &format!("  {name} pop [name]"),
        "     If name is given, all versions up to and including the one with the matching name are popped.",
    ])
}

/// The `push`/`pop` argument check both it2universion and it2setkeylabel make: no name
/// (`Some("")`), a name, or None after printing why (exit 1).
fn stack_name<'a>(args: &'a [String], io: &mut Io, help: &str) -> Option<&'a str> {
    match args.len() {
        1 => Some(""),
        2 if args[1].is_empty() => {
            put(io.err, "Name must not be empty\n");
            None
        }
        2 => Some(&args[1]),
        _ => {
            put(io.err, help);
            None
        }
    }
}

/// `it2universion set N | push [name] | pop [name]`.
pub fn it2universion(args: &[String], io: &mut Io) -> i32 {
    let term = util_cli::term(io);
    let help = universion_help("it2universion");
    let body = match args.first().map(String::as_str) {
        Some("set") if args.len() == 2 => {
            // `set_version $2`, unquoted: the first word of the argument.
            let word = args[1].split([' ', '\t', '\n']).find(|w| !w.is_empty()).unwrap_or("");
            format!("1337;UnicodeVersion={word}")
        }
        Some(word @ ("push" | "pop")) => match stack_name(args, io, &help) {
            Some(name) => format!("1337;UnicodeVersion={word} {name}"),
            None => return 1,
        },
        _ => {
            put(io.err, &help);
            return 1;
        }
    };
    put(io.out, &seq(term.as_deref(), &body));
    0
}

/// it2setkeylabel's `show_help` (to stderr), with its literal `$RANDOM` example.
pub fn keylabel_help(name: &str) -> String {
    let mut v: Vec<String> = [
        "Usage:".to_string(),
        format!("   {name} set Fn Label"),
        "     Where n is a value from 1 to 20".to_string(),
        format!("   {name} set status Label"),
        "     Sets the touch bar status button's label".to_string(),
        format!("   {name} push [name]"),
        // The script's inner quotes around "." end and restart its string: they vanish.
        "     Saves the current labels with an optional name. Resets labels to their default value, unless name begins with a . character.".to_string(),
        format!("   {name} pop [name]"),
        "     If name is given, all key labels up to and including the one with the matching name are popped.".to_string(),
        String::new(),
        "Example:".to_string(),
        "#!/usr/bin/env bash".to_string(),
        "# Wrapper script for mc that sets function key labels".to_string(),
        "NAME=mc_$RANDOM".to_string(),
        "# Save existing labels".to_string(),
        format!("{name} push $NAME"),
    ]
    .into();
    for (key, label) in [
        ("F1", "Help"),
        ("F2", "Menu"),
        ("F3", "View"),
        ("F4", "Edit"),
        ("F5", "Copy"),
        ("F6", "Move"),
        ("F7", "Mkdir"),
        ("F8", "Del"),
        ("F9", "Menu"),
        ("F10", "Quit"),
        ("F11", "Menu"),
        ("F13", "View"),
        ("F14", "Edit"),
        ("F15", "Copy"),
        ("F16", "Move"),
        ("F17", "Find"),
    ] {
        v.push(format!("{name} set {key} {label}"));
    }
    v.push("mc".to_string());
    v.push("# Restore labels to their previous state".to_string());
    v.push(format!("{name} pop $NAME"));
    v.iter().map(|l| format!("{l}\n")).collect()
}

/// `it2setkeylabel set K L | push [name] | pop [name]`.
pub fn it2setkeylabel(args: &[String], io: &mut Io) -> i32 {
    let term = util_cli::term(io);
    let help = keylabel_help("it2setkeylabel");
    let body = match args.first().map(String::as_str) {
        Some("set") if args.len() == 3 => format!("1337;SetKeyLabel={}={}", args[1], args[2]),
        Some(word @ ("push" | "pop")) => {
            let key = if word == "push" { "PushKeyLabels" } else { "PopKeyLabels" };
            match stack_name(args, io, &help) {
                Some("") => format!("1337;{key}"),
                Some(name) => format!("1337;{key}={name}"),
                None => return 1,
            }
        }
        _ => {
            put(io.err, &help);
            return 1;
        }
    };
    put(io.out, &seq(term.as_deref(), &body));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc1337::Attention;
    use crate::screen::{Request, Screen};
    use crate::util_cli::Harness;
    use crate::util_osc::{TMUX_OSC, TMUX_ST};
    use std::path::PathBuf;

    /// A fresh folder for one test: `notes.txt` holds "hello\n", `big.bin` 100 bytes.
    fn fixture(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-util-simple-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello\n").unwrap();
        std::fs::write(dir.join("big.bin"), [b'x'; 100]).unwrap();
        dir
    }

    fn harness(dir: &std::path::Path, term: Option<&str>) -> Harness {
        let mut h = Harness { cwd: dir.to_path_buf(), stdin_tty: true, ..Default::default() };
        if let Some(t) = term {
            h.env.insert("TERM".into(), t.into());
        }
        h
    }

    fn text(v: &[u8]) -> String {
        String::from_utf8(v.to_vec()).unwrap()
    }

    fn wrap(body: &str) -> String {
        format!("{TMUX_OSC}{body}{TMUX_ST}")
    }

    #[test]
    fn it2copy_follows_the_script() {
        let dir = fixture("copy");
        let mut h = harness(&dir, Some("xterm-256color"));
        assert_eq!(h.run("it2copy", &["notes.txt"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;Copy=:aGVsbG8K\n\x07");
        assert!(h.err.is_empty());

        let mut h = harness(&dir, None);
        h.input = b"hi".to_vec();
        h.stdin_tty = false;
        assert_eq!(h.run("it2copy", &[]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;Copy=:aGk=\n\x07");

        let mut h = harness(&dir, Some("tmux-256color"));
        h.randoms = [12, 40000].into();
        assert_eq!(h.run("it2copy", &["notes.txt"]), 0);
        let want = wrap("1337;Copy=2;127232") + &wrap("1337;Copy=3;127232:aGVsbG8K") + &wrap("1337;Copy=4;127232");
        assert_eq!(text(&h.out), want);

        let mut h = harness(&dir, Some("tmux"));
        h.randoms = [1, 2].into();
        assert_eq!(h.run("it2copy", &["big.bin"]), 0);
        let out = text(&h.out);
        let pieces: Vec<usize> = out
            .split(TMUX_OSC)
            .filter_map(|s| s.strip_prefix("1337;Copy=3;12:"))
            .map(|s| s.strip_suffix(TMUX_ST).unwrap().len())
            .collect();
        assert_eq!(pieces, [80, 56]);

        let mut h = harness(&dir, Some("screen"));
        assert_eq!(h.run("it2copy", &["notes.txt"]), 0);
        assert_eq!(text(&h.out), wrap("1337;Copy=:aGVsbG8K\n"));

        let mut h = harness(&dir, None);
        assert_eq!(h.run("it2copy", &["missing"]), 2);
        assert_eq!(text(&h.err), "ERROR: it2copy: missing: No such file or directory\n");
        assert!(h.out.is_empty());

        let help = "Usage: it2copy\n          Copies to clipboard from standard input\n       it2copy filename\n          Copies to clipboard from file\n";
        let mut h = harness(&dir, None);
        assert_eq!(h.run("it2copy", &["-x"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: Unknown option flag: -x\n{help}"));
        let mut h = harness(&dir, None);
        assert_eq!(h.run("it2copy", &["-h"]), 0);
        assert_eq!(text(&h.err), help);
        assert!(h.out.is_empty());

        // Empty input: base64 writes nothing.
        let mut h = harness(&dir, None);
        h.stdin_tty = false;
        assert_eq!(h.run("it2copy", &[]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;Copy=:\x07");
        // A folder: `[ -r ]` passes, base64 fails to read it.
        let mut h = harness(&dir, None);
        assert_eq!(h.run("it2copy", &["sub"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;Copy=:\x07");
        assert_eq!(text(&h.err), "base64: read error: Is a directory\n");
    }

    fn one(term: Option<&str>, name: &str, args: &[&str]) -> (i32, String, String) {
        let mut h = harness(&std::env::temp_dir(), term);
        let code = h.run(name, args);
        (code, text(&h.out), text(&h.err))
    }

    #[test]
    fn setters_follow_the_scripts() {
        let cases: &[(&str, &[&str], &str)] = &[
            ("it2setcolor", &["fg", "fff", "bg", "000"], "1337;SetColors=fg=fff,bg=000"),
            ("it2setcolor", &["preset", "Light Background"], "1337;SetColors=preset=Light Background"),
            ("it2setcolor", &["tab", "default"], "1337;SetColors=tab=default"),
            ("it2attention", &["start"], "1337;RequestAttention=1"),
            ("it2attention", &["stop"], "1337;RequestAttention=0"),
            ("it2attention", &["once"], "1337;RequestAttention=once"),
            ("it2attention", &["fireworks"], "1337;RequestAttention=fireworks"),
            ("it2universion", &["set", "9"], "1337;UnicodeVersion=9"),
            ("it2universion", &["push"], "1337;UnicodeVersion=push "),
            ("it2universion", &["push", "work"], "1337;UnicodeVersion=push work"),
            ("it2universion", &["pop"], "1337;UnicodeVersion=pop "),
            ("it2setkeylabel", &["set", "F1", "Help"], "1337;SetKeyLabel=F1=Help"),
            ("it2setkeylabel", &["push"], "1337;PushKeyLabels"),
            ("it2setkeylabel", &["push", "n"], "1337;PushKeyLabels=n"),
            ("it2setkeylabel", &["pop", "n"], "1337;PopKeyLabels=n"),
        ];
        for (name, args, body) in cases {
            assert_eq!(one(None, name, args), (0, format!("\x1b]{body}\x07"), String::new()), "{name} {args:?}");
            assert_eq!(one(Some("screen"), name, args), (0, wrap(body), String::new()), "{name} {args:?} wrapped");
        }
        let (code, out, err) = one(None, "it2setcolor", &["fg", "fff", "bg"]);
        assert_eq!(code, 0);
        assert_eq!(out, format!("\x1b]1337;SetColors=fg=fff\x07{}", setcolor_help()));
        assert_eq!(err, SETCOLOR_STDERR_LINE);
        // `set_version $2`, unquoted.
        assert_eq!(one(None, "it2universion", &["set", " 8 x"]).1, "\x1b]1337;UnicodeVersion=8\x07");
    }

    #[test]
    fn setter_texts_follow_the_scripts() {
        let help = setcolor_help();
        assert!(help.starts_with("Usage\n\n1) To set a specific color to an RGB value:\nFor example:\n  it2setcolor fg fff\n"));
        assert!(help.ends_with("3) To reset the current tab's color to the system default:\n  it2setcolor tab default\n"));
        assert_eq!(help.lines().count(), 29);
        assert_eq!(one(None, "it2setcolor", &[]), (0, help.clone(), SETCOLOR_STDERR_LINE.to_string()));
        assert_eq!(one(None, "it2setcolor", &["-h"]), (0, help.clone(), SETCOLOR_STDERR_LINE.to_string()));
        assert_eq!(one(None, "it2setcolor", &["-x"]), (1, help.clone(), format!("ERROR: Unknown option flag: -x\n{SETCOLOR_STDERR_LINE}")));
        // An open sequence is closed before the help.
        assert_eq!(one(None, "it2setcolor", &["fg", "fff", "-x"]).1, format!("\x1b]1337;SetColors=fg=fff\x07{help}"));

        let attention = attention_help("it2attention");
        assert!(attention.starts_with("Usage:\n  it2attention start\n    Begin bouncing the dock icon if another app is active\n"));
        assert!(attention.ends_with("  it2attention fireworks\n    Show an explosion animation at the cursor\n"));
        assert_eq!(attention.lines().count(), 9);
        for args in [&[][..], &["bogus"]] {
            assert_eq!(one(None, "it2attention", args), (1, String::new(), attention.clone()), "{args:?}");
        }

        let uni = universion_help("it2universion");
        assert_eq!(
            uni,
            "Usage:\n  it2universion set 8\n  it2universion set 9\n  it2universion push [name]\n     Saves the current version with an optional name.\n  it2universion pop [name]\n     If name is given, all versions up to and including the one with the matching name are popped.\n"
        );
        for args in [&[][..], &["set"], &["set", "8", "9"], &["push", "a", "b"], &["bogus"]] {
            assert_eq!(one(None, "it2universion", args), (1, String::new(), uni.clone()), "{args:?}");
        }
        for args in [&["push", ""][..], &["pop", ""]] {
            assert_eq!(one(None, "it2universion", args), (1, String::new(), "Name must not be empty\n".to_string()), "{args:?}");
            assert_eq!(one(None, "it2setkeylabel", args), (1, String::new(), "Name must not be empty\n".to_string()), "{args:?}");
        }

        let keys = keylabel_help("it2setkeylabel");
        assert!(keys.starts_with("Usage:\n   it2setkeylabel set Fn Label\n     Where n is a value from 1 to 20\n"));
        assert!(keys.contains("\n     Saves the current labels with an optional name. Resets labels to their default value, unless name begins with a . character.\n"));
        assert!(keys.contains("\nNAME=mc_$RANDOM\n# Save existing labels\nit2setkeylabel push $NAME\nit2setkeylabel set F1 Help\n"));
        assert!(keys.contains("\nit2setkeylabel set F11 Menu\nit2setkeylabel set F13 View\n"));
        assert!(keys.ends_with("it2setkeylabel set F17 Find\nmc\n# Restore labels to their previous state\nit2setkeylabel pop $NAME\n"));
        assert_eq!(keys.lines().count(), 35);
        for args in [&[][..], &["set", "F1"], &["set", "F1", "a", "b"], &["pop", "a", "b"], &["bogus"]] {
            assert_eq!(one(None, "it2setkeylabel", args), (1, String::new(), keys.clone()), "{args:?}");
        }
    }

    fn feed(s: &mut Screen, bytes: &str) {
        s.feed(bytes.as_bytes());
    }

    #[test]
    fn setters_reach_tandem() {
        let dir = fixture("reach");
        for term in [None, Some("tmux-256color"), Some("screen")] {
            let mut h = harness(&dir, term);
            h.randoms = [3, 4].into();
            assert_eq!(h.run("it2copy", &["notes.txt"]), 0);
            let mut s = Screen::new(80, 24);
            feed(&mut s, &text(&h.out));
            assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("hello\n".into())], "{term:?}");
        }

        let mut s = Screen::new(80, 24);
        feed(&mut s, &one(None, "it2setcolor", &["tab", "default"]).1);
        assert_eq!(s.take_requests(), vec![Request::TabColor(None)]);
        feed(&mut s, &one(Some("tmux"), "it2setcolor", &["fg", "f00"]).1);
        assert!(s.color_sets.iter().any(|(n, _)| n == "fg"), "{:?}", s.color_sets);

        feed(&mut s, &one(None, "it2attention", &["once"]).1);
        assert_eq!(s.take_requests(), vec![Request::Attention(Attention::Once)]);

        feed(&mut s, &one(None, "it2universion", &["set", "8"]).1);
        assert_eq!(s.unicode.version, 8);
        for args in [&["push"][..], &["set", "9"]] {
            feed(&mut s, &one(None, "it2universion", args).1);
        }
        assert_eq!(s.unicode.version, 9);
        feed(&mut s, &one(None, "it2universion", &["pop"]).1);
        assert_eq!(s.unicode.version, 8);

        feed(&mut s, &one(Some("screen"), "it2setkeylabel", &["set", "F1", "Help"]).1);
        assert_eq!(s.key_labels.current.get("F1").map(String::as_str), Some("Help"));

        // it2git's output, from a repository on "main" with a change.
        let mut git = |args: &[&str]| match args[0] {
            "status" => Some((0, " M a\n".to_string())),
            "symbolic-ref" => Some((0, "main\n".to_string())),
            "rev-list" => Some((0, "0\t0\n".to_string())),
            _ => Some((0, String::new())),
        };
        let vars: String = crate::util_git::git_values(&mut git).iter().map(|(n, v)| crate::util_git::set_user_var(n, v)).collect();
        feed(&mut s, &vars);
        assert_eq!(s.shell.vars.get("gitBranch").map(String::as_str), Some("main"));
        assert_eq!(s.shell.vars.get("gitDirty").map(String::as_str), Some("dirty"));
        assert_eq!(s.line_text(0).trim(), "", "nothing visible");
    }
}
