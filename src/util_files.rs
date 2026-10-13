//! imgcat, it2cat and it2dl (milestone 11e): iTerm2's file utilities
//! (`OtherResources/Utilities/`), byte for byte. Pure: files through `Io.cwd`, curl through
//! `Io.run`.

use std::path::{Path, PathBuf};

use crate::util_cli::{self, Cmd, Io};
use crate::util_osc::{b64, b64_line, fold, seq, wrapped};

/// imgcat's `validate_size_unit` regex, its `(:?` typo kept (an optional colon, not a
/// group): `^(:?[0-9]+(:?px|%)?|auto)$`.
pub fn size_unit_ok(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^(:?[0-9]+(:?px|%)?|auto)$").expect("imgcat's size regex")).is_match(s)
}

/// One `print_image` call: the name sent (None or empty: none), the file's bytes, and the
/// optional parameters as the script passes them (empty means not sent).
#[derive(Clone, Debug, Default)]
pub struct Image<'a> {
    pub name: Option<&'a str>,
    pub data: &'a [u8],
    pub width: Option<&'a str>,
    pub height: Option<&'a str>,
    pub preserve: Option<&'a str>,
    pub kind: Option<&'a str>,
    /// `-l`: one `File=` sequence instead of a Multipart transfer.
    pub legacy: bool,
}

/// `[ -n "$x" ]`.
fn given(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.is_empty())
}

/// Everything imgcat's `print_image` prints before its `printf '\n'`.
pub fn image_bytes(term: Option<&str>, img: &Image) -> String {
    let data = b64(img.data);
    let mut head = format!("1337;{}=inline=1;size={}", if img.legacy { "File" } else { "MultipartFile" }, img.data.len());
    if let Some(n) = given(img.name) {
        head.push_str(&format!(";name={}", b64(n.as_bytes())));
    }
    for (key, v) in [("width", img.width), ("height", img.height), ("preserveAspectRatio", img.preserve), ("type", img.kind)] {
        if let Some(v) = given(v) {
            head.push_str(&format!(";{key}={v}"));
        }
    }
    if img.legacy {
        return seq(term, &format!("{head}:{data}"));
    }
    // 200-character `fold` chunks help it through tmux.
    let mut s = seq(term, &head);
    for part in fold(&data, 200) {
        s.push_str(&seq(term, &format!("1337;FilePart={part}")));
    }
    s.push_str(&seq(term, "1337;FileEnd"));
    s
}

/// it2cat's `print_file` before its `printf '\n'`; `mode` is "regular" or "wide".
pub fn text_file(term: Option<&str>, name: Option<&str>, data: &[u8], kind: Option<&str>, mode: &str) -> String {
    let mut body = format!("1337;File=inline=1;size={}", data.len());
    if let Some(n) = given(name) {
        body.push_str(&format!(";name={}", b64(n.as_bytes())));
    }
    if let Some(k) = given(kind) {
        body.push_str(&format!(";type={k}"));
    }
    if !mode.is_empty() {
        body.push_str(&format!(";mode={mode}"));
    }
    body.push_str(&format!(":{}", b64(data)));
    seq(term, &body)
}

/// it2dl's `send_file_data` with `encoded` already base64: under tmux or screen a Multipart
/// transfer in 200-character parts; else one literal `ESC ] ... BEL` with `encoded` as is.
fn send_file_data(term: Option<&str>, name: &str, encoded: &str, size: usize) -> String {
    if !wrapped(term) {
        return format!("{}{encoded}\x07", plain_head(name, size as u64));
    }
    let name = b64(name.as_bytes());
    let mut s = seq(term, &format!("1337;MultipartFile=name={name};size={size}"));
    for part in fold(encoded, 200) {
        s.push_str(&seq(term, &format!("1337;FilePart={part}")));
    }
    s.push_str(&seq(term, "1337;FileEnd"));
    s
}

/// The plain path's literal head, up to the data: `ESC ]1337;File=name=<base64>;size=<n>:`.
fn plain_head(name: &str, size: u64) -> String {
    format!("\x1b]1337;File=name={};size={size}:", b64(name.as_bytes()))
}

/// Bytes it2dl reads per piece on the plain path (a multiple of 3).
const PIECE: usize = 57 * 1024;

/// it2dl's `send_file`: `send_file_data` for a file, whose base64 comes from
/// `b64_encode < file` straight to stdout, so macOS base64's newline is kept (before BEL on
/// the plain path; dropped by word splitting on the Multipart path).
pub fn download(term: Option<&str>, name: &str, data: &[u8]) -> String {
    send_file_data(term, name, &b64_line(data), data.len())
}

/// A folder as it2dl sends it (`tar -czf - -C dirname basename`): a gzipped tar whose
/// entries start at the folder's own name, which is `prompted::upload_archive` of the one
/// folder.
pub fn folder_archive(dir: &Path) -> Result<Vec<u8>, String> {
    crate::prompted::upload_archive(&[dir.to_path_buf()])
}

const IMGCAT_HELP: &[&str] = &[
    "",
    "Usage: imgcat [-p] [-n] [-W width] [-H height] [-r] [-s] [-u] [-t file-type] [-f] filename ...",
    "       cat filename | imgcat [-W width] [-H height] [-r] [-s]",
    "",
    "Display images inline in the Termurion using Inline Images Protocol",
    "",
    "Options:",
    "",
    "    -h, --help                      Display help message",
    "    -p, --print                     Enable printing of filename or URL after each image",
    "    -n, --no-print                  Disable printing of filename or URL after each image",
    "    -u, --url                       Interpret following filename arguments as remote URLs",
    "    -f, --file                      Interpret following filename arguments as regular Files",
    "    -t, --type file-type            Provides a type hint",
    "    -r, --preserve-aspect-ratio     When scaling image preserve its original aspect ratio",
    "    -s, --stretch                   Stretch image to specified width and height (this option is opposite to -r)",
    "    -W, --width N                   Set image width to N character cells, pixels or percent (see below)",
    "    -H, --height N                  Set image height to N character cells, pixels or percent (see below)",
    "    -l, --legacy                    Use legacy protocol that sends the whole image in a single control sequence",
    "",
    "    If you don't specify width or height an appropriate value will be chosen automatically.",
    "    The width and height are given as word 'auto' or number N followed by a unit:",
    "        N      character cells",
    "        Npx    pixels",
    "        N%     percent of the session's width or height",
    "        auto   the image's inherent size will be used to determine an appropriate dimension",
    "",
    "    If a type is provided, it is used as a hint to disambiguate.",
    "    The file type can be a mime type like text/markdown, a language name like Java, or a file extension like .c",
    "    The file type can usually be inferred from the extension or its contents. -t is most useful when",
    "    a filename is not available, such as whe input comes from a pipe.",
    "",
    "Examples:",
    "",
    "    $ imgcat -W 250px -H 250px -s avatar.png",
    "    $ cat graph.png | imgcat -W 100%",
    "    $ imgcat -p -W 500px -u http://host.tld/path/to/image.jpg -W 80 -f image.png",
    "    $ cat url_list.txt | xargs imgcat -p -W 40 -u",
    "    $ imgcat -t application/json config.json",
    "",
];

const IT2CAT_HELP: &[&str] = &[
    "",
    "Usage: it2cat [-w] [-t file-type] [-u] [-f] filename ...",
    "       cat filename | it2cat [-w] [-t file-type]",
    "",
    "Display a text file with native rendering.",
    "",
    "Options:",
    "",
    "    -h, --help                      Display help message",
    "    -u, --url                       Interpret following filename arguments as remote URLs",
    "    -f, --file                      Interpret following filename arguments as regular Files",
    "    -t, --type file-type            Provides a type hint",
    "    -w, --wide                      Render in wide mode with a horizontal scrollbar",
    "",
    "    If a type is provided, it is used as a hint to disambiguate.",
    "    The file type can be a mime type like text/markdown, a language name like Java, or a file extension like .c",
    "    The file type can usually be inferred from the extension or its contents. -t is most useful when",
    "    a filename is not available, such as whe input comes from a pipe.",
    "",
    "Examples:",
    "",
    "    $ it2cat file.txt",
    "    $ cat graph.png | it2cat",
    "    $ it2cat -u http://example.com/path/to/file.txt -f otherfile.txt",
    "    $ cat url_list.txt | xargs it2cat -u",
    "    $ it2cat -w -t application/json config.json",
    "",
];

fn lines(v: &[&str]) -> String {
    v.iter().map(|l| format!("{l}\n")).collect()
}

/// imgcat's `show_help` (to stderr).
pub fn imgcat_help() -> String {
    lines(IMGCAT_HELP)
}

/// it2cat's `show_help` (to stderr).
pub fn it2cat_help() -> String {
    lines(IT2CAT_HELP)
}

fn put(w: &mut dyn std::io::Write, s: &str) {
    let _ = w.write_all(s.as_bytes());
}

/// The scripts' `error`: "ERROR: " and the message, on stderr.
fn error(io: &mut Io, msg: &str) {
    put(io.err, &format!("ERROR: {msg}\n"));
}

/// What `[ -r "$1" ]` and `cat "$1"` find for an argument, resolved against `cwd`.
pub(crate) enum Found {
    File(Vec<u8>),
    Dir,
    Missing,
}

/// A network path, or one reached through a link to one, is never probed (it counts as
/// missing).
pub(crate) fn find(cwd: &Path, arg: &str) -> Found {
    match locate(cwd, arg) {
        Some((_, true)) => Found::Dir,
        Some((p, false)) => crate::localfs::read(&p).map_or(Found::Missing, Found::File),
        None => Found::Missing,
    }
}

/// The path an argument names and whether it is a folder; None when it is missing or may
/// not be probed (a network path, or one reached through a link to one; a verbatim local
/// path is read in its plain form).
pub(crate) fn locate(cwd: &Path, arg: &str) -> Option<(crate::localfs::LocalPath, bool)> {
    if arg.is_empty() || crate::prompted::network_path(util_cli::local_form(arg)) {
        return None;
    }
    let p = crate::localfs::LocalPath::judge(util_cli::join_local(cwd, arg))?;
    crate::localfs::stat(&p).ok().map(|m| (p, m.is_dir))
}

/// `basename -- "$1"`: the last component, trailing separators ignored.
fn basename(arg: &str) -> &str {
    let t = arg.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        return &arg[..arg.len().min(1)];
    }
    t.rsplit(['/', '\\']).next().unwrap_or(t)
}

#[derive(Clone, Copy, PartialEq)]
enum Viewer {
    Imgcat,
    It2cat,
}

/// imgcat and it2cat's main: the same loop, with imgcat's sizing flags or it2cat's `-w`.
fn view(tool: Viewer, args: &[String], io: &mut Io) -> i32 {
    let (prog, what, help) = match tool {
        Viewer::Imgcat => ("imgcat", "image", imgcat_help()),
        Viewer::It2cat => ("it2cat", "file", it2cat_help()),
    };
    let term = util_cli::term(io);
    let term = term.as_deref();
    let mut has_stdin = !io.stdin_tty;
    if !has_stdin && args.is_empty() {
        put(io.err, &help);
        return 0;
    }
    let (mut print_filename, mut legacy, mut is_url, mut shown) = (false, false, false, false);
    let (mut width, mut height, mut preserve, mut file_type) = (String::new(), String::new(), "", String::new());
    let mut mode = "regular";
    let show = |io: &mut Io, name: &str, data: &[u8], print_filename: bool, width: &str, height: &str, preserve: &str, file_type: &str, legacy: bool, mode: &str| {
        let bytes = match tool {
            Viewer::Imgcat => {
                let img = Image { name: Some(name), data, width: Some(width), height: Some(height), preserve: Some(preserve), kind: Some(file_type), legacy };
                image_bytes(term, &img)
            }
            Viewer::It2cat => text_file(term, Some(name), data, Some(file_type), mode),
        };
        put(io.out, &bytes);
        put(io.out, "\n");
        if print_filename {
            put(io.out, &format!("{name}\n"));
        }
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let next = args.get(i + 1).map_or("", String::as_str);
        let imgcat = tool == Viewer::Imgcat;
        match a {
            "-h" | "--h" | "--help" => {
                put(io.err, &help);
                return 0;
            }
            "-p" | "--p" | "--print" if imgcat => print_filename = true,
            "-n" | "--n" | "--no-print" if imgcat => print_filename = false,
            "-W" | "--W" | "--width" | "-H" | "--H" | "--height" if imgcat => {
                if !size_unit_ok(next) {
                    error(io, &format!("Invalid image sizing unit - '{next}'"));
                    put(io.err, &help);
                    return 1;
                }
                if a.ends_with('W') || a == "--width" {
                    width = next.to_string();
                } else {
                    height = next.to_string();
                }
                i += 1;
            }
            "-r" | "--r" | "--preserve-aspect-ratio" if imgcat => preserve = "1",
            "-s" | "--s" | "--stretch" if imgcat => preserve = "0",
            "-l" | "--l" | "--legacy" if imgcat => legacy = true,
            "-w" | "--w" | "--wide" if !imgcat => mode = "wide",
            "-f" | "--f" | "--file" => {
                has_stdin = false;
                is_url = false;
            }
            "-u" | "--u" | "--url" => {
                if !(io.have)("curl") {
                    error(io, "missing dependency: can't find curl");
                    return 1;
                }
                has_stdin = false;
                is_url = true;
            }
            "-t" | "--t" | "--type" => {
                file_type = next.to_string();
                i += 1;
            }
            _ if a.starts_with('-') => {
                error(io, &format!("Unknown option flag: {a}"));
                put(io.err, &help);
                return 1;
            }
            _ => {
                let data = if is_url {
                    // `curl -fs "$1" | b64_encode` under pipefail: curl's code decides.
                    let cmd = Cmd { program: "curl".into(), args: vec!["-fs".into(), a.to_string()], cwd: Some(io.cwd.clone()), stdin: None, inherit: false, quiet: false, env: Vec::new(), merge_stderr: false };
                    match (io.run)(&cmd) {
                        Ok(o) if o.code == 0 => o.stdout,
                        other => {
                            // A curl that could not start reads as bash's "command not found".
                            let code = other.map_or(127, |o| o.code);
                            error(io, &format!("Could not retrieve {what} from URL {a}, error_code: {code}"));
                            return 2;
                        }
                    }
                } else {
                    match find(&io.cwd, a) {
                        Found::File(b) => b,
                        Found::Dir => {
                            put(io.err, &format!("cat: {a}: Is a directory\n"));
                            Vec::new()
                        }
                        Found::Missing => {
                            error(io, &format!("{prog}: {a}: No such file or directory"));
                            return 2;
                        }
                    }
                };
                has_stdin = false;
                show(io, a, &data, print_filename, &width, &height, preserve, &file_type, legacy, mode);
                shown = true;
            }
        }
        i += 1;
    }
    if has_stdin {
        let mut data = Vec::new();
        let _ = io.input.read_to_end(&mut data);
        show(io, "", &data, false, &width, &height, preserve, &file_type, legacy, mode);
        shown = true;
    }
    if !shown {
        let none = match tool {
            Viewer::Imgcat => "No image provided. Check command line options.",
            Viewer::It2cat => "No file provided. Check command line options.",
        };
        error(io, none);
        put(io.err, &help);
        return 1;
    }
    0
}

/// `imgcat`.
pub fn imgcat(args: &[String], io: &mut Io) -> i32 {
    view(Viewer::Imgcat, args, io)
}

/// `it2cat`.
pub fn it2cat(args: &[String], io: &mut Io) -> i32 {
    view(Viewer::It2cat, args, io)
}

/// `it2dl`: each file or folder offered for download in turn; the exit code is the last
/// command's, so 0 once any argument was handled.
pub fn it2dl(args: &[String], io: &mut Io) -> i32 {
    if args.is_empty() {
        put(io.out, "Usage: it2dl file|directory ...\n");
        return 1;
    }
    let term = util_cli::term(io);
    let term = term.as_deref();
    for arg in args {
        let missing = |io: &mut Io| put(io.err, &format!("File or directory {arg} does not exist or is not readable.\n"));
        match locate(&io.cwd, arg) {
            None => missing(io),
            // The archive's base64 comes from `$(...)` through `printf '%s'`: no newline.
            Some((p, true)) => match real_folder(&p).map(|dir| folder_archive(&dir)) {
                None => missing(io),
                Some(Ok(archive)) => put(io.out, &send_file_data(term, &format!("{}.tar.gz", basename(arg)), &b64(&archive), archive.len())),
                Some(Err(_)) => put(io.err, &format!("Error: Failed to create tar.gz archive for directory: {arg}\n")),
            },
            Some((p, false)) if wrapped(term) => match crate::localfs::read(&p) {
                Ok(bytes) => put(io.out, &download(term, basename(arg), &bytes)),
                Err(_) => missing(io),
            },
            // `b64_encode < file` streams: the head, the base64 piece by piece, then macOS
            // base64's newline and BEL.
            Some((p, false)) => match crate::localfs::open(&p).and_then(|f| Ok((crate::localfs::file_stat(&f)?.len, f))) {
                Ok((size, mut f)) => {
                    put(io.out, &plain_head(basename(arg), size));
                    crate::util_osc::stream_b64(&mut f, PIECE, &mut |text| put(io.out, text));
                    put(io.out, "\n\x07");
                }
                Err(_) => missing(io),
            },
        }
    }
    0
}

/// A folder by its real name (`..` and `.` resolved), in `local_form`; None when that name
/// may not be opened (`util_cli::may_open`).
fn real_folder(dir: &crate::localfs::LocalPath) -> Option<PathBuf> {
    let real = crate::localfs::canonicalize(dir)?;
    let real = PathBuf::from(util_cli::local_form(&real.path().to_string_lossy()));
    crate::localfs::judged(&real).then_some(real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline_file::Download;
    use crate::screen::Screen;
    use crate::util_cli::{Cmd, Harness, Output};
    use crate::util_osc::{TMUX_OSC, TMUX_ST, b64};
    use std::path::{Path, PathBuf};

    /// A fresh folder for one test: `a.png` holds "PNGDATA", `notes.txt` "hello\n".
    fn fixture(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-util-files-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"PNGDATA").unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello\n").unwrap();
        dir
    }

    /// A run in `dir` with a console stdin.
    fn harness(dir: &Path) -> Harness {
        Harness { cwd: dir.to_path_buf(), stdin_tty: true, ..Default::default() }
    }

    fn text(v: &[u8]) -> String {
        String::from_utf8(v.to_vec()).unwrap()
    }

    fn with_term(dir: &Path, term: &str) -> Harness {
        let mut h = harness(dir);
        h.env.insert("TERM".into(), term.into());
        h
    }

    const A_PNG: &str = "\x1b]1337;MultipartFile=inline=1;size=7;name=YS5wbmc=\x07\x1b]1337;FilePart=UE5HREFUQQ==\x07\x1b]1337;FileEnd\x07\n";

    #[test]
    fn size_units_follow_the_script() {
        for ok in ["10", "10px", "50%", "auto", ":10", "10:px", ":10:px"] {
            assert!(size_unit_ok(ok), "{ok:?}");
        }
        // ":10:%" fails: the kept regex allows the colon only before "px" (pre-flight ruling).
        for bad in ["10x", "px", "", "AUTO", "10 px", ":10:%", "10:%"] {
            assert!(!size_unit_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn imgcat_follows_the_script() {
        let dir = fixture("imgcat");
        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["a.png"]), 0);
        assert_eq!(text(&h.out), A_PNG);
        assert!(h.err.is_empty());

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-p", "-W", "10", "-r", "-t", ".png", "a.png"]), 0);
        assert_eq!(
            text(&h.out),
            "\x1b]1337;MultipartFile=inline=1;size=7;name=YS5wbmc=;width=10;preserveAspectRatio=1;type=.png\x07\x1b]1337;FilePart=UE5HREFUQQ==\x07\x1b]1337;FileEnd\x07\na.png\n"
        );

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-s", "a.png"]), 0);
        assert!(text(&h.out).contains(";name=YS5wbmc=;preserveAspectRatio=0\x07"));

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-l", "a.png"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;File=inline=1;size=7;name=YS5wbmc=:UE5HREFUQQ==\x07\n");

        std::fs::write(dir.join("big.bin"), [b'x'; 250]).unwrap();
        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["big.bin"]), 0);
        let out = text(&h.out);
        let parts: Vec<usize> = out.split("1337;FilePart=").skip(1).map(|p| p.find('\x07').unwrap()).collect();
        assert_eq!(parts, [200, 136]);
        assert!(out.contains(";size=250;"));

        let mut h = with_term(&dir, "tmux-256color");
        assert_eq!(h.run("imgcat", &["a.png"]), 0);
        let wrapped = format!(
            "{TMUX_OSC}1337;MultipartFile=inline=1;size=7;name=YS5wbmc={TMUX_ST}{TMUX_OSC}1337;FilePart=UE5HREFUQQ=={TMUX_ST}{TMUX_OSC}1337;FileEnd{TMUX_ST}\n"
        );
        assert_eq!(text(&h.out), wrapped);

        let mut h = Harness { cwd: dir.clone(), stdin_tty: false, input: b"PNGDATA".to_vec(), ..Default::default() };
        assert_eq!(h.run("imgcat", &[]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;MultipartFile=inline=1;size=7\x07\x1b]1337;FilePart=UE5HREFUQQ==\x07\x1b]1337;FileEnd\x07\n");

        // Stdin is not read once a file was shown.
        let mut h = Harness { cwd: dir.clone(), stdin_tty: false, input: b"OTHER".to_vec(), ..Default::default() };
        assert_eq!(h.run("imgcat", &["a.png"]), 0);
        assert_eq!(text(&h.out), A_PNG);

        let mut h = harness(&dir);
        h.answers.push_back(Ok(Output { code: 0, stdout: b"PNGDATA".to_vec() }));
        assert_eq!(h.run("imgcat", &["-u", "http://h/x.png"]), 0);
        assert_eq!(
            h.runs,
            vec![Cmd { program: "curl".into(), args: vec!["-fs".into(), "http://h/x.png".into()], cwd: Some(dir.clone()), stdin: None, inherit: false, quiet: false, env: Vec::new(), merge_stderr: false }]
        );
        assert_eq!(
            text(&h.out),
            "\x1b]1337;MultipartFile=inline=1;size=7;name=aHR0cDovL2gveC5wbmc=\x07\x1b]1337;FilePart=UE5HREFUQQ==\x07\x1b]1337;FileEnd\x07\n"
        );

        // -H sets the height only.
        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-H", "5", "a.png"]), 0);
        assert_eq!(
            text(&h.out),
            "\x1b]1337;MultipartFile=inline=1;size=7;name=YS5wbmc=;height=5\x07\x1b]1337;FilePart=UE5HREFUQQ==\x07\x1b]1337;FileEnd\x07\n"
        );

        // -f turns URL mode back off: the second name is a file, curl runs once.
        let mut h = harness(&dir);
        h.answers.push_back(Ok(Output { code: 0, stdout: b"PNGDATA".to_vec() }));
        assert_eq!(h.run("imgcat", &["-u", "http://h/x.png", "-f", "a.png"]), 0);
        assert_eq!(h.runs.len(), 1);
        assert!(text(&h.out).ends_with(A_PNG), "{:?}", text(&h.out));

        // -l under tmux: the one File= sequence, wrapped.
        let mut h = with_term(&dir, "tmux-256color");
        assert_eq!(h.run("imgcat", &["-l", "a.png"]), 0);
        assert_eq!(text(&h.out), format!("{TMUX_OSC}1337;File=inline=1;size=7;name=YS5wbmc=:UE5HREFUQQ=={TMUX_ST}\n"));

        // A folder reads as empty data with cat's complaint (the script's `$(cat dir | ...)`).
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["sub"]), 0);
        assert_eq!(text(&h.err), "cat: sub: Is a directory\n");
        assert_eq!(text(&h.out), "\x1b]1337;MultipartFile=inline=1;size=0;name=c3Vi\x07\x1b]1337;FileEnd\x07\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imgcat_errors_follow_the_script() {
        let dir = fixture("imgcat-errors");
        let help = imgcat_help();

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &[]), 0);
        assert_eq!(text(&h.err), help);
        assert!(h.out.is_empty());

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["missing.png"]), 2);
        assert_eq!(text(&h.err), "ERROR: imgcat: missing.png: No such file or directory\n");
        // A network path is never probed, in any spelling.
        for p in ["\\\\host\\s\\a.png", "//host/s/a.png", "/\\host\\a.png", "\\/host/a.png", "\\\\?\\UNC\\host\\s", "\\\\.\\pipe\\x", "\\??\\UNC\\h\\s"] {
            let mut h = harness(&dir);
            assert_eq!(h.run("imgcat", &[p]), 2, "{p}");
            assert_eq!(text(&h.err), format!("ERROR: imgcat: {p}: No such file or directory\n"));
            let mut h = harness(&dir);
            assert_eq!(h.run("it2dl", &[p]), 0, "{p}");
            assert_eq!(text(&h.err), format!("File or directory {p} does not exist or is not readable.\n"));
        }

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-W", "10x", "a.png"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: Invalid image sizing unit - '10x'\n{help}"));
        assert!(h.out.is_empty());

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-q"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: Unknown option flag: -q\n{help}"));

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["-p"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: No image provided. Check command line options.\n{help}"));

        let mut h = harness(&dir);
        h.missing.push("curl".into());
        assert_eq!(h.run("imgcat", &["-u", "http://h/x.png"]), 1);
        assert_eq!(text(&h.err), "ERROR: missing dependency: can't find curl\n");
        assert!(h.runs.is_empty());

        let mut h = harness(&dir);
        h.answers.push_back(Ok(Output { code: 6, stdout: Vec::new() }));
        assert_eq!(h.run("imgcat", &["-u", "http://h/x.png"]), 2);
        assert_eq!(text(&h.err), "ERROR: Could not retrieve image from URL http://h/x.png, error_code: 6\n");

        let mut h = harness(&dir);
        assert_eq!(h.run("imgcat", &["--help"]), 0);
        assert_eq!(text(&h.err), help);

        assert!(help.starts_with("\nUsage: imgcat [-p] [-n] [-W width] [-H height] [-r] [-s] [-u] [-t file-type] [-f] filename ...\n"));
        assert!(help.contains("\nDisplay images inline in the Termurion using Inline Images Protocol\n"));
        assert!(help.contains("\n    $ imgcat -t application/json config.json\n\n"));
        assert!(help.ends_with("\n\n"));
        assert!(!help.contains("iTerm2"));
        assert_eq!(help.lines().count(), 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn it2cat_follows_the_script() {
        let dir = fixture("it2cat");
        let help = it2cat_help();

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &["notes.txt"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;File=inline=1;size=6;name=bm90ZXMudHh0;mode=regular:aGVsbG8K\x07\n");

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &["-w", "-t", "text/plain", "notes.txt"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;File=inline=1;size=6;name=bm90ZXMudHh0;type=text/plain;mode=wide:aGVsbG8K\x07\n");

        let mut h = with_term(&dir, "screen");
        assert_eq!(h.run("it2cat", &["notes.txt"]), 0);
        assert_eq!(text(&h.out), format!("{TMUX_OSC}1337;File=inline=1;size=6;name=bm90ZXMudHh0;mode=regular:aGVsbG8K{TMUX_ST}\n"));

        let mut h = Harness { cwd: dir.clone(), input: b"hello\n".to_vec(), ..Default::default() };
        assert_eq!(h.run("it2cat", &[]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;File=inline=1;size=6;mode=regular:aGVsbG8K\x07\n");

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &[]), 0);
        assert_eq!(text(&h.err), help);

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &["missing.txt"]), 2);
        assert_eq!(text(&h.err), "ERROR: it2cat: missing.txt: No such file or directory\n");

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &["-p"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: Unknown option flag: -p\n{help}"));

        let mut h = harness(&dir);
        assert_eq!(h.run("it2cat", &["-w"]), 1);
        assert_eq!(text(&h.err), format!("ERROR: No file provided. Check command line options.\n{help}"));

        let mut h = harness(&dir);
        h.missing.push("curl".into());
        assert_eq!(h.run("it2cat", &["-u", "http://h/x.txt"]), 1);
        assert_eq!(text(&h.err), "ERROR: missing dependency: can't find curl\n");

        let mut h = harness(&dir);
        h.answers.push_back(Ok(Output { code: 6, stdout: Vec::new() }));
        assert_eq!(h.run("it2cat", &["-u", "http://h/x.txt"]), 2);
        assert_eq!(text(&h.err), "ERROR: Could not retrieve file from URL http://h/x.txt, error_code: 6\n");

        assert!(help.starts_with("\nUsage: it2cat [-w] [-t file-type] [-u] [-f] filename ...\n"));
        assert!(help.contains("\nDisplay a text file with native rendering.\n"));
        assert!(help.ends_with("\n    $ it2cat -w -t application/json config.json\n\n"));
        assert_eq!(help.lines().count(), 27);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    fn entries(archive: &[u8]) -> Vec<String> {
        let mut a = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        a.entries().unwrap().map(|e| String::from_utf8_lossy(&e.unwrap().path_bytes()).into_owned()).collect()
    }

    /// The name and the decoded data of one plain `File=name=...;size=...:<data>BEL`.
    fn sent(out: &[u8]) -> (String, Vec<u8>) {
        let out = text(out);
        let rest = out.strip_prefix("\x1b]1337;File=name=").unwrap_or_else(|| panic!("{out:?}"));
        let (name, rest) = rest.split_once(';').unwrap();
        let (_, data) = rest.split_once(':').unwrap();
        let name = String::from_utf8(crate::util_osc::unb64(name)).unwrap();
        (name, crate::util_osc::unb64(data.trim_end_matches('\x07')))
    }

    #[test]
    fn it2dl_follows_the_script() {
        let dir = fixture("it2dl");

        // The plain path sends `b64_encode < file` straight to stdout, so macOS base64's
        // newline lands before BEL (the script wins over the brief's text).
        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &["notes.txt"]), 0);
        assert_eq!(text(&h.out), "\x1b]1337;File=name=bm90ZXMudHh0;size=6:aGVsbG8K\n\x07");

        let mut h = with_term(&dir, "screen");
        assert_eq!(h.run("it2dl", &["notes.txt"]), 0);
        assert_eq!(
            text(&h.out),
            "\x1bPtmux;\x1b\x1b]1337;MultipartFile=name=bm90ZXMudHh0;size=6\x07\x1b\\\x1bPtmux;\x1b\x1b]1337;FilePart=aGVsbG8K\x07\x1b\\\x1bPtmux;\x1b\x1b]1337;FileEnd\x07\x1b\\"
        );

        std::fs::create_dir_all(dir.join("dir")).unwrap();
        std::fs::write(dir.join("dir").join("f.txt"), b"inside\n").unwrap();
        let archive = folder_archive(&dir.join("dir")).unwrap();
        assert_eq!(entries(&archive), ["dir/", "dir/f.txt"]);
        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &["dir"]), 0);
        // The archive's base64 comes from `$(...)` and `printf '%s'`: no newline.
        assert_eq!(text(&h.out), format!("\x1b]1337;File=name=ZGlyLnRhci5neg==;size={}:{}\x07", archive.len(), b64(&archive)));

        // Under screen the folder goes as a Multipart transfer in 200-character parts.
        let mut h = with_term(&dir, "screen");
        assert_eq!(h.run("it2dl", &["dir"]), 0);
        let mut want = format!("{TMUX_OSC}1337;MultipartFile=name=ZGlyLnRhci5neg==;size={}{TMUX_ST}", archive.len());
        for part in crate::util_osc::fold(&b64(&archive), 200) {
            want.push_str(&format!("{TMUX_OSC}1337;FilePart={part}{TMUX_ST}"));
        }
        want.push_str(&format!("{TMUX_OSC}1337;FileEnd{TMUX_ST}"));
        assert_eq!(text(&h.out), want);

        // A file larger than one streamed piece, and an empty one, as the whole-file form.
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(dir.join("big.bin"), &big).unwrap();
        std::fs::write(dir.join("empty.bin"), b"").unwrap();
        for (name, bytes) in [("big.bin", &big[..]), ("empty.bin", &[][..])] {
            let mut h = harness(&dir);
            assert_eq!(h.run("it2dl", &[name]), 0);
            assert!(text(&h.out) == download(None, name, bytes), "{name}");
        }

        // `..` and `.` send the script's names ("...tar.gz", "..tar.gz") with the folder's
        // entries under its real name (tar would write "./" or strip "../").
        std::fs::create_dir_all(dir.join("dir").join("deeper")).unwrap();
        let mut h = harness(&dir.join("dir").join("deeper"));
        assert_eq!(h.run("it2dl", &[".."]), 0);
        assert!(h.err.is_empty(), "{:?}", text(&h.err));
        let (name, archive) = sent(&h.out);
        assert_eq!(name, "...tar.gz");
        assert_eq!(sorted(entries(&archive)), ["dir/", "dir/deeper", "dir/f.txt"]);
        let mut h = harness(&dir.join("dir"));
        assert_eq!(h.run("it2dl", &["."]), 0);
        let (name, archive) = sent(&h.out);
        assert_eq!(name, "..tar.gz");
        assert_eq!(sorted(entries(&archive)), ["dir/", "dir/deeper", "dir/f.txt"]);

        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &[]), 1);
        assert_eq!(text(&h.out), "Usage: it2dl file|directory ...\n");
        assert!(h.err.is_empty());

        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &["missing", "notes.txt"]), 0);
        assert_eq!(text(&h.err), "File or directory missing does not exist or is not readable.\n");
        assert_eq!(text(&h.out), "\x1b]1337;File=name=bm90ZXMudHh0;size=6:aGVsbG8K\n\x07");

        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &["notes.txt", "missing"]), 0, "the last command is the message");

        // The name sent is the last component.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("x.txt"), b"hello\n").unwrap();
        let mut h = harness(&dir);
        assert_eq!(h.run("it2dl", &["sub\\x.txt"]), 0);
        assert!(text(&h.out).starts_with(&format!("\x1b]1337;File=name={};size=6:", b64(b"x.txt"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn png_2x2() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn files_reach_tandem() {
        let dir = fixture("reach");
        std::fs::write(dir.join("pic.png"), png_2x2()).unwrap();
        for (term, args) in [(None, &["pic.png"][..]), (Some("tmux-256color"), &["pic.png"][..]), (None, &["-l", "pic.png"][..])] {
            let mut h = harness(&dir);
            if let Some(t) = term {
                h.env.insert("TERM".into(), t.into());
            }
            assert_eq!(h.run("imgcat", args), 0);
            let mut s = Screen::new(80, 24);
            s.feed(&h.out);
            s.allow_inline();
            assert_eq!(s.images.by_id.len(), 1, "{term:?} {args:?}");
            let img = s.images.by_id.values().next().unwrap();
            assert_eq!((img.width_px, img.height_px), (2, 2), "{term:?} {args:?}");
        }
        for term in [None, Some("screen")] {
            let mut h = harness(&dir);
            if let Some(t) = term {
                h.env.insert("TERM".into(), t.into());
            }
            assert_eq!(h.run("it2dl", &["notes.txt"]), 0);
            let mut s = Screen::new(80, 24);
            s.feed(&h.out);
            assert_eq!(s.downloads, vec![Download { name: "notes.txt".into(), bytes: b"hello\n".to_vec() }], "{term:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
