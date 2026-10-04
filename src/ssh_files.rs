//! The file subcommands of iTerm2's conductor (`CD:743-910`, milestone 14d): what each
//! sends to framer.py's `file` command, how each is described, and what a status means
//! (`EP:11-26`, `SEX:14-27`). Pure: nothing here talks to a host.

use base64::Engine;

/// `nonEmptyBase64EncodedString` (`Data+iTerm.swift:15-20`): `=` for no bytes, since an
/// empty line would end the framer's command.
pub fn b64(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "=".into();
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The text of a reply nobody could decode (Tandem's; iTerm2's description is not a sentence).
pub const GARBAGE: &str = "Server returned garbage";

/// The most reply bytes a fetch holds (P8): a chunk is 4096 bytes at most, about 5.5 KiB of base64.
pub const FETCH_REPLY_MAX: usize = 16 * 1024;
/// The most reply bytes and lines any other file reply holds (P8): a listing or an answer
/// of suggestions. Past it the request fails with `GARBAGE`; the connection never does.
pub const FILE_REPLY_MAX: usize = 16 * 1024 * 1024;
pub const FILE_REPLY_LINES: usize = 100_000;
/// Whether one more reply line of `line_len` bytes fits after `lines` lines holding `kept` bytes.
pub fn reply_fits(kept: usize, lines: usize, line_len: usize, max_bytes: usize, max_lines: usize) -> bool {
    lines < max_lines && kept + line_len <= max_bytes
}

/// The status a reply that overran its cap ends with (never a framer status, which are 0 to
/// 255): `check` says `GARBAGE` for it.
pub const OVERRUN_STATUS: i32 = -2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sorting {
    ByName,
    ByDate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chunk {
    pub offset: u64,
    pub size: u64,
}

/// `SuggestionRequest.Inputs` (`SR:17`): the key of the suggestion cache and the request.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SuggestInputs {
    pub prefix: String,
    pub full_prefix: String,
    pub full_suffix: String,
    pub directories: Vec<String>,
    pub working_directory: Option<String>,
    pub executable: bool,
    pub limit: usize,
}

impl SuggestInputs {
    /// `debugDescription` (`SR:17-19`).
    pub fn debug_text(&self) -> String {
        format!(
            "prefix={} fullPrefix={} fullSuffix={} dirs={} pwd={} executable={}",
            self.prefix,
            self.full_prefix,
            self.full_suffix,
            self.directories.join(", "),
            self.working_directory.as_deref().unwrap_or("none"),
            self.executable
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchSub {
    Start { query: String, base: String },
    Ack { id: String, count: u32 },
    Stop { id: String },
}

/// `FileSubcommand` (`CD:743-910`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSub {
    Ls { path: String, sort: Sorting },
    /// The unique id is local only: it is never on the wire.
    Fetch { path: String, chunk: Option<Chunk>, unique_id: Option<String> },
    Stat { path: String },
    Suggest(SuggestInputs),
    Rm { path: String, recursive: bool },
    Ln { target: String, link: String },
    Mv { source: String, dest: String },
    Mkdir { path: String },
    Create { path: String, content: Vec<u8> },
    Append { path: String, content: Vec<u8> },
    /// Whole seconds: the framer truncates to them (`F:850-853`), which keeps this `Eq`.
    Utime { path: String, seconds: i64 },
    Chmod { path: String, r: bool, w: bool, x: bool },
    Zip { path: String },
    Search(SearchSub),
}

fn path_line(path: &str) -> String {
    b64(path.as_bytes())
}

impl FileSub {
    /// stringValue: the lines joined by `\n`.
    pub fn string_value(&self) -> String {
        let lines: Vec<String> = match self {
            FileSub::Ls { path, sort } => vec!["ls".into(), path_line(path), if *sort == Sorting::ByName { "n" } else { "d" }.into()],
            FileSub::Fetch { path, chunk, .. } => {
                let mut v = vec!["fetch".to_string(), path_line(path)];
                if let Some(c) = chunk {
                    v.push(c.offset.to_string());
                    v.push(c.size.to_string());
                }
                v
            }
            FileSub::Stat { path } => vec!["stat".into(), path_line(path)],
            FileSub::Suggest(i) => {
                let dirs = if i.directories.is_empty() { " ".to_string() } else { i.directories.iter().map(|d| path_line(d)).collect::<Vec<_>>().join(" ") };
                vec!["suggest".into(), path_line(&i.prefix), dirs, path_line(i.working_directory.as_deref().unwrap_or("//")), if i.executable { "rx" } else { "r" }.into(), i.limit.to_string()]
            }
            FileSub::Rm { path, recursive } => {
                let mut v = vec!["rm".to_string()];
                if *recursive {
                    v.push("-r".into());
                }
                v.push(path_line(path));
                v
            }
            FileSub::Ln { target, link } => vec!["ln".into(), path_line(target), path_line(link)],
            FileSub::Mv { source, dest } => vec!["mv".into(), path_line(source), path_line(dest)],
            FileSub::Mkdir { path } => vec!["mkdir".into(), path_line(path)],
            FileSub::Create { path, content } => vec!["create".into(), path_line(path), b64(content)],
            FileSub::Append { path, content } => vec!["append".into(), path_line(path), b64(content)],
            // Swift's Double description: whole seconds end ".0".
            FileSub::Utime { path, seconds } => vec!["utime".into(), path_line(path), format!("{:?}", *seconds as f64)],
            FileSub::Chmod { path, r, w, x } => {
                let bits: String = [(*r, 'r'), (*w, 'w'), (*x, 'x')].iter().map(|(on, c)| if *on { *c } else { '-' }).collect();
                vec!["chmod-u".into(), path_line(path), bits]
            }
            FileSub::Zip { path } => vec!["zip".into(), path_line(path)],
            FileSub::Search(SearchSub::Start { query, base }) => vec!["search".into(), "start".into(), path_line(query), path_line(base)],
            FileSub::Search(SearchSub::Ack { id, count }) => vec!["search".into(), "ack".into(), id.clone(), count.to_string()],
            FileSub::Search(SearchSub::Stop { id }) => vec!["search".into(), "stop".into(), id.clone()],
        };
        lines.join("\n")
    }

    /// operationDescription (`CD:870-909`).
    pub fn description(&self) -> String {
        match self {
            FileSub::Ls { path, sort } => format!("ls {path} {}", if *sort == Sorting::ByName { "byName" } else { "byDate" }),
            FileSub::Fetch { path, chunk: Some(c), .. } => format!("fetch {path} offset={} size={}", c.offset, c.size),
            FileSub::Fetch { path, chunk: None, .. } => format!("fetch {path}"),
            FileSub::Stat { path } => format!("stat {path}"),
            FileSub::Suggest(i) => format!("fetchSuggestions {}", i.debug_text()),
            FileSub::Rm { path, recursive } => format!("rm {}{path}", if *recursive { "-r " } else { "" }),
            FileSub::Ln { target, link } => format!("ln -s {target} {link}"),
            FileSub::Mv { source, dest } => format!("mv {source} {dest}"),
            FileSub::Mkdir { path } => format!("mkdir {path}"),
            FileSub::Create { path, content } => format!("create {path} length={} bytes", content.len()),
            FileSub::Append { path, content } => format!("append {path} length={} bytes", content.len()),
            FileSub::Utime { path, seconds } => format!("utime {path} {}", utc_text(*seconds)),
            FileSub::Chmod { path, r, w, x } => format!("chmod {path} r={r} w={w} x={x}"),
            FileSub::Zip { path } => format!("zip {path}"),
            FileSub::Search(SearchSub::Start { query, base }) => format!("search start {query} {base}"),
            FileSub::Search(SearchSub::Ack { id, count }) => format!("search ack {id} {count}"),
            FileSub::Search(SearchSub::Stop { id }) => format!("search stop {id}"),
        }
    }

    /// Only a fetch pipelines (`CD:1375-1388`).
    pub fn pipelines(&self) -> bool {
        matches!(self, FileSub::Fetch { .. })
    }

    pub fn is_suggest(&self) -> bool {
        matches!(self, FileSub::Suggest(_))
    }

    /// The local unique id of a fetch, for canceling a transfer's queued chunks.
    pub fn fetch_id(&self) -> Option<&str> {
        match self {
            FileSub::Fetch { unique_id: Some(id), .. } => Some(id),
            _ => None,
        }
    }
}

/// `yyyy-MM-dd HH:mm:ss +0000` of Unix seconds (iTerm2 prints a `Date` in UTC).
fn utc_text(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} +0000", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// What a failed request is (`SEX:14-27`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileError {
    ConnectionClosed,
    NotFound,
    Internal,
    Canceled,
    /// A reply that could not be decoded or overran its cap (`GARBAGE`).
    Garbage,
}

impl FileError {
    pub fn text(self) -> &'static str {
        match self {
            FileError::ConnectionClosed => "Connection closed",
            FileError::NotFound => "File not found",
            FileError::Internal => "Internal error",
            FileError::Canceled => "File transfer canceled",
            FileError::Garbage => GARBAGE,
        }
    }
}

/// `EP:11-26`: below 0 the connection closed (an abort, a cancel or the command timeout),
/// above 0 the file was not found whatever errno the remote sent. `OVERRUN_STATUS` is Tandem's
/// own (a reply past its cap).
pub fn check(status: i32) -> Result<(), FileError> {
    match status {
        0 => Ok(()),
        OVERRUN_STATUS => Err(FileError::Garbage),
        s if s < 0 => Err(FileError::ConnectionClosed),
        _ => Err(FileError::NotFound),
    }
}

/// An upload's destination (`SCPPath`): the remote path, host and user the pane reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScpPath {
    pub path: String,
    pub hostname: String,
    pub username: String,
}

impl ScpPath {
    pub fn user_at_host(&self) -> String {
        format!("{}@{}", self.username, self.hostname)
    }
}

/// `scpPathForFile` (`VS:741-772`): needs the line's host, user and working directory and a
/// host that is not this machine. A name starting `/` or `~` is used as is, else it is appended
/// to the working directory; an empty name is the working directory itself (no trailing `/`).
pub fn scp_path(file: &str, cwd: Option<&str>, host: Option<&str>, user: Option<&str>, host_is_local: bool) -> Option<ScpPath> {
    let (cwd, host, user) = (cwd?, host.filter(|h| !h.is_empty())?, user.filter(|u| !u.is_empty())?);
    if host_is_local {
        return None;
    }
    let path = if file.starts_with('/') || file.starts_with('~') {
        file.to_string()
    } else if file.is_empty() {
        let t = cwd.trim_end_matches('/');
        if t.is_empty() && cwd.starts_with('/') { "/".to_string() } else { t.to_string() }
    } else {
        crate::remote_file::posix_join(cwd, file)
    };
    Some(ScpPath { path, hostname: host.to_string(), username: user.to_string() })
}

/// `canTransferFilesTo` (`PS:15250-15289`): the conductor frames, the host is the identity's
/// host, hostname or discovered hostname, and the user is the identity's (any when it has none).
pub fn can_transfer(framing: bool, identity: &crate::ssh_args::Identity, discovered: Option<&str>, to: &ScpPath) -> bool {
    if !framing {
        return false;
    }
    let host_ok = to.hostname == identity.host || to.hostname == identity.hostname || discovered.is_some_and(|d| d == to.hostname);
    let user_ok = identity.username.as_deref().is_none_or(|u| u == to.username);
    host_ok && user_ok
}

/// `TV:5210-5271`: the question before an Alt-drop uploads.
pub fn confirm_upload(files: &[String], to: &ScpPath) -> String {
    let dest = format!("{}:{}", to.user_at_host(), to.path);
    if files.len() == 1 {
        format!("OK to copy\n{}\nto\n{dest}?", files[0])
    } else {
        format!("OK to copy the following files:\n{}\n\nto\n{dest}?", files.join(", "))
    }
}

/// The most an Alt-drop sends (P19): each KiB is one conductor command with its own 30 s timer.
pub const UPLOAD_MAX: u64 = 64 * 1024 * 1024;

/// Why a dropped file may not be uploaded (P19): a UNC or device path (reading it would contact
/// a share), or more than `UPLOAD_MAX` bytes. `size` is None when it is not known yet.
pub fn upload_refused(local: &str, size: Option<u64>) -> Option<String> {
    if crate::prompted::network_path(local) {
        return Some("A network path cannot be uploaded.".to_string());
    }
    size.filter(|s| *s > UPLOAD_MAX).map(|_| "Too large to upload (the limit is 64 MiB).".to_string())
}

/// What a download offers (`CD:1630-1675`): a viewable type may be shown in a browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewChoice {
    Download,
    Ask { mime: String },
}

/// Tandem has no built-in browser, so nothing is offered for viewing (iTerm2's own path when
/// its browser is off): the mime type is never consulted.
pub const BROWSER_ALLOWED: bool = false;

pub const VIEW_HEADING: &str = "Download or View File?";
pub const DOWNLOAD: &str = "Download";
pub const VIEW: &str = "View";

/// `downloadOrView`: archives are never viewed, no browser or no mime type downloads.
pub fn download_or_view(mime: Option<&str>, browser_allowed: bool) -> ViewChoice {
    match mime {
        Some(m) if browser_allowed && !matches!(m, "application/zip" | "application/x-gtar" | "application/x-tar") => ViewChoice::Ask { mime: m.to_string() },
        _ => ViewChoice::Download,
    }
}

pub fn view_title(name: &str) -> String {
    format!("Download {name} or view in browser?")
}

/// The remembered-answer id of the view prompt.
pub fn view_id(mime: &str, to: &ScpPath) -> String {
    format!("DownloadOrViewInBrowser_{mime} {}", to.user_at_host())
}

/// The Win-click's last phase (`UF:879-899`): the Smart Selection match at the click, through
/// `scpPathForFile`.
pub fn secure_copy_target(smart_text: Option<&str>, scp: impl Fn(&str) -> Option<ScpPath>) -> Option<ScpPath> {
    scp(smart_text?)
}

/// How a Win-click treats a pane (P10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickRoute {
    /// As for any pane: the local plan (open, run, reveal), then the secure-copy phase.
    Local,
    /// Remote text: only the secure copy; no local run is ever built from it.
    SecureCopyOnly,
}

/// How a click (or Open Selection, or a label pick) treats text in a pane. `remote` is the one
/// guard, `App::remote_text`: the pane has a framing conductor, whatever host the remote reports.
/// `link` is the URL when the target is a link or URL. Remote text may open only a web or mail
/// link; anything else, a `file:` link included, may only download.
pub fn click_route(remote: bool, link: Option<&str>) -> ClickRoute {
    if remote && !link.is_some_and(crate::semantic::web_link) {
        ClickRoute::SecureCopyOnly
    } else {
        ClickRoute::Local
    }
}

/// Whether a Smart Selection action may run for a match in a pane: in a remote pane only the
/// actions that stay in Tandem (send the text back, copy it); open, run and coprocess never do.
pub fn remote_action_allowed(remote: bool, action: &str) -> bool {
    !remote || matches!(action, "sendText:" | "copy:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sug() -> SuggestInputs {
        SuggestInputs { prefix: "src/fo".into(), full_prefix: "ls src/fo".into(), full_suffix: String::new(), directories: vec!["/usr/bin".into()], working_directory: None, executable: true, limit: 8 }
    }

    #[test]
    fn file_wire_follows_iterm2() {
        let p = || "/tmp".to_string();
        assert_eq!(FileSub::Ls { path: p(), sort: Sorting::ByName }.string_value(), "ls\nL3RtcA==\nn");
        assert_eq!(FileSub::Ls { path: p(), sort: Sorting::ByDate }.string_value(), "ls\nL3RtcA==\nd");
        let f = FileSub::Fetch { path: p(), chunk: Some(Chunk { offset: 0, size: 4096 }), unique_id: Some("x".into()) };
        assert_eq!(f.string_value(), "fetch\nL3RtcA==\n0\n4096");
        assert_eq!(FileSub::Fetch { path: p(), chunk: None, unique_id: None }.string_value(), "fetch\nL3RtcA==");
        assert_eq!(FileSub::Stat { path: p() }.string_value(), "stat\nL3RtcA==");
        assert_eq!(FileSub::Zip { path: p() }.string_value(), "zip\nL3RtcA==");
        assert_eq!(FileSub::Mkdir { path: p() }.string_value(), "mkdir\nL3RtcA==");
        assert_eq!(FileSub::Rm { path: p(), recursive: true }.string_value(), "rm\n-r\nL3RtcA==");
        assert_eq!(FileSub::Rm { path: p(), recursive: false }.string_value(), "rm\nL3RtcA==");
        assert_eq!(FileSub::Ln { target: p(), link: "/tmp/f".into() }.string_value(), "ln\nL3RtcA==\nL3RtcC9m");
        assert_eq!(FileSub::Mv { source: "/tmp/f.uploading-U1".into(), dest: "/tmp/f".into() }.string_value(), "mv\nL3RtcC9mLnVwbG9hZGluZy1VMQ==\nL3RtcC9m");
        assert_eq!(FileSub::Create { path: "/tmp/f".into(), content: vec![] }.string_value(), "create\nL3RtcC9m\n=");
        assert_eq!(FileSub::Create { path: "/tmp/f".into(), content: vec![0, 1, 2] }.string_value(), "create\nL3RtcC9m\nAAEC");
        assert_eq!(FileSub::Append { path: "/tmp/f".into(), content: vec![] }.string_value(), "append\nL3RtcC9m\n=");
        assert_eq!(FileSub::Append { path: "/tmp/f".into(), content: vec![0, 1, 2] }.string_value(), "append\nL3RtcC9m\nAAEC");
        assert_eq!(FileSub::Utime { path: p(), seconds: 1_700_000_000 }.string_value(), "utime\nL3RtcA==\n1700000000.0");
        assert_eq!(FileSub::Chmod { path: p(), r: true, w: false, x: true }.string_value(), "chmod-u\nL3RtcA==\nr-x");

        assert_eq!(FileSub::Suggest(sug()).string_value(), "suggest\nc3JjL2Zv\nL3Vzci9iaW4=\nLy8=\nrx\n8");
        assert_eq!(FileSub::Suggest(SuggestInputs { directories: vec![], ..sug() }).string_value().split('\n').nth(2), Some(" "));
        assert_eq!(FileSub::Suggest(SuggestInputs { directories: vec!["/usr/bin".into(), "//".into()], ..sug() }).string_value().split('\n').nth(2), Some("L3Vzci9iaW4= Ly8="));
        assert_eq!(FileSub::Suggest(SuggestInputs { working_directory: Some("/tmp".into()), ..sug() }).string_value().split('\n').nth(3), Some("L3RtcA=="));
        assert_eq!(FileSub::Suggest(SuggestInputs { executable: false, ..sug() }).string_value().split('\n').nth(4), Some("r"));
        assert_eq!(FileSub::Suggest(SuggestInputs { prefix: String::new(), ..sug() }).string_value().split('\n').nth(1), Some("="));

        assert_eq!(FileSub::Search(SearchSub::Start { query: "abc".into(), base: p() }).string_value(), "search\nstart\nYWJj\nL3RtcA==");
        assert_eq!(FileSub::Search(SearchSub::Ack { id: "ID".into(), count: 1 }).string_value(), "search\nack\nID\n1");
        assert_eq!(FileSub::Search(SearchSub::Stop { id: "ID".into() }).string_value(), "search\nstop\nID");
        assert_eq!(b64(&[]), "=");
        assert_eq!(b64("h\u{e9}llo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn file_descriptions_follow_iterm2() {
        let p = || "/tmp".to_string();
        assert_eq!(FileSub::Ls { path: p(), sort: Sorting::ByName }.description(), "ls /tmp byName");
        assert_eq!(FileSub::Ls { path: p(), sort: Sorting::ByDate }.description(), "ls /tmp byDate");
        let f = FileSub::Fetch { path: p(), chunk: Some(Chunk { offset: 0, size: 4096 }), unique_id: Some("x".into()) };
        assert_eq!(f.description(), "fetch /tmp offset=0 size=4096");
        assert_eq!(FileSub::Fetch { path: p(), chunk: None, unique_id: None }.description(), "fetch /tmp");
        assert_eq!(FileSub::Zip { path: p() }.description(), "zip /tmp");
        assert_eq!(FileSub::Stat { path: p() }.description(), "stat /tmp");
        assert_eq!(FileSub::Rm { path: p(), recursive: true }.description(), "rm -r /tmp");
        assert_eq!(FileSub::Rm { path: p(), recursive: false }.description(), "rm /tmp");
        assert_eq!(FileSub::Ln { target: p(), link: "/tmp/f".into() }.description(), "ln -s /tmp /tmp/f");
        assert_eq!(FileSub::Mv { source: "/a".into(), dest: "/b".into() }.description(), "mv /a /b");
        assert_eq!(FileSub::Mkdir { path: p() }.description(), "mkdir /tmp");
        assert_eq!(FileSub::Create { path: "/tmp/f".into(), content: vec![0, 1, 2] }.description(), "create /tmp/f length=3 bytes");
        assert_eq!(FileSub::Append { path: "/tmp/f".into(), content: vec![0, 1, 2] }.description(), "append /tmp/f length=3 bytes");
        assert_eq!(FileSub::Utime { path: p(), seconds: 1_700_000_000 }.description(), "utime /tmp 2023-11-14 22:13:20 +0000");
        assert_eq!(FileSub::Utime { path: p(), seconds: -1 }.description(), "utime /tmp 1969-12-31 23:59:59 +0000");
        assert_eq!(FileSub::Chmod { path: p(), r: true, w: false, x: true }.description(), "chmod /tmp r=true w=false x=true");
        assert_eq!(FileSub::Suggest(sug()).description(), "fetchSuggestions prefix=src/fo fullPrefix=ls src/fo fullSuffix= dirs=/usr/bin pwd=none executable=true");
        assert_eq!(FileSub::Search(SearchSub::Start { query: "abc".into(), base: p() }).description(), "search start abc /tmp");
        assert_eq!(FileSub::Search(SearchSub::Ack { id: "ID".into(), count: 1 }).description(), "search ack ID 1");
        assert_eq!(FileSub::Search(SearchSub::Stop { id: "ID".into() }).description(), "search stop ID");

        assert!(f.pipelines());
        assert!(!FileSub::Stat { path: p() }.pipelines());
        assert!(!FileSub::Suggest(sug()).pipelines());
        assert!(FileSub::Suggest(sug()).is_suggest());
        assert!(!f.is_suggest());
        assert_eq!(f.fetch_id(), Some("x"));
        assert_eq!(FileSub::Fetch { path: p(), chunk: None, unique_id: None }.fetch_id(), None);
        assert_eq!(FileSub::Stat { path: p() }.fetch_id(), None);
    }

    #[test]
    fn reply_caps_hold() {
        assert!(reply_fits(0, 0, 10, 10, 5));
        assert!(!reply_fits(0, 0, 11, 10, 5), "one line over the bytes");
        assert!(!reply_fits(5, 1, 6, 10, 5), "the sum over the bytes");
        assert!(reply_fits(0, FILE_REPLY_LINES - 1, 1, FILE_REPLY_MAX, FILE_REPLY_LINES));
        assert!(!reply_fits(0, FILE_REPLY_LINES, 1, FILE_REPLY_MAX, FILE_REPLY_LINES), "the line count");
    }

    #[test]
    fn file_status_follows_iterm2() {
        assert_eq!(check(0), Ok(()));
        assert_eq!(check(-1), Err(FileError::ConnectionClosed));
        for s in [1, 2, 100, 255] {
            assert_eq!(check(s), Err(FileError::NotFound), "{s}");
        }
        assert_eq!(FileError::ConnectionClosed.text(), "Connection closed");
        assert_eq!(FileError::NotFound.text(), "File not found");
        assert_eq!(FileError::Internal.text(), "Internal error");
        assert_eq!(FileError::Canceled.text(), "File transfer canceled");
        // A reply past its cap is garbage, not a closed connection (P8).
        assert_eq!(check(OVERRUN_STATUS), Err(FileError::Garbage));
        assert_eq!(FileError::Garbage.text(), GARBAGE);
    }

    fn to(host: &str, user: &str) -> ScpPath {
        ScpPath { path: "/home/u".into(), hostname: host.into(), username: user.into() }
    }

    #[test]
    fn scp_paths_follow_iterm2() {
        let p = |f: &str, cwd: Option<&str>| scp_path(f, cwd, Some("h"), Some("u"), false);
        assert_eq!(p("a.txt", Some("/home/u")), Some(ScpPath { path: "/home/u/a.txt".into(), hostname: "h".into(), username: "u".into() }));
        assert_eq!(p("/etc/x", Some("/home/u")).unwrap().path, "/etc/x");
        assert_eq!(p("~/x", Some("/home/u")).unwrap().path, "~/x");
        assert_eq!(p("", Some("/home/u")).unwrap().path, "/home/u");
        assert_eq!(p("", Some("/home/u/")).unwrap().path, "/home/u");
        assert_eq!(p("", Some("/")).unwrap().path, "/");
        assert_eq!(p("a", None), None);
        assert_eq!(scp_path("a", Some("/x"), None, Some("u"), false), None);
        assert_eq!(scp_path("a", Some("/x"), Some("h"), None, false), None);
        assert_eq!(scp_path("a", Some("/x"), Some("h"), Some("u"), true), None);
        assert_eq!(to("h", "u").user_at_host(), "u@h");
    }

    #[test]
    fn transfer_gate_follows_iterm2() {
        use crate::ssh_args::Identity;
        let id = Identity { host: "box".into(), hostname: "box.lan".into(), username: None, port: 22 };
        assert!(can_transfer(true, &id, None, &to("box", "u")));
        assert!(can_transfer(true, &id, None, &to("box.lan", "u")));
        assert!(!can_transfer(true, &id, None, &to("other", "u")));
        assert!(can_transfer(true, &id, Some("other"), &to("other", "u")));
        assert!(!can_transfer(false, &id, None, &to("box", "u")));
        let id = Identity { username: Some("u".into()), ..id };
        assert!(!can_transfer(true, &id, None, &to("box", "v")));
        assert!(can_transfer(true, &id, None, &to("box", "u")));
    }

    #[test]
    fn upload_confirmation_follows_iterm2() {
        assert_eq!(confirm_upload(&["C:\\a.txt".into()], &to("h", "u")), "OK to copy\nC:\\a.txt\nto\nu@h:/home/u?");
        assert_eq!(confirm_upload(&["C:\\a.txt".into(), "C:\\b.txt".into()], &to("h", "u")), "OK to copy the following files:\nC:\\a.txt, C:\\b.txt\n\nto\nu@h:/home/u?");
    }

    #[test]
    fn unc_drops_are_refused() {
        for p in ["\\\\attacker\\s\\x", "//attacker/s/x", "\\\\?\\UNC\\h\\s", "\\\\.\\pipe\\x"] {
            assert!(upload_refused(p, Some(1)).is_some(), "{p}");
            assert!(upload_refused(p, None).is_some(), "{p}");
        }
        assert_eq!(upload_refused("C:\\a.txt", Some(1)), None);
        assert_eq!(upload_refused("C:\\a.txt", None), None);
    }

    #[test]
    fn uploads_have_a_size_cap() {
        assert_eq!(upload_refused("C:\\a", Some(UPLOAD_MAX)), None);
        assert!(upload_refused("C:\\a", Some(UPLOAD_MAX + 1)).is_some());
        assert!(upload_refused("C:\\a", Some(u64::MAX)).is_some());
    }

    #[test]
    fn download_or_view_follows_iterm2() {
        assert_eq!(download_or_view(Some("image/png"), false), ViewChoice::Download);
        assert_eq!(download_or_view(Some("image/png"), true), ViewChoice::Ask { mime: "image/png".into() });
        for m in ["application/zip", "application/x-gtar", "application/x-tar"] {
            assert_eq!(download_or_view(Some(m), true), ViewChoice::Download);
        }
        assert_eq!(download_or_view(None, true), ViewChoice::Download);
        assert_eq!(download_or_view(Some("text/html"), true), ViewChoice::Ask { mime: "text/html".into() });
        assert_eq!(view_title("a.png"), "Download a.png or view in browser?");
        assert_eq!((VIEW_HEADING, DOWNLOAD, VIEW), ("Download or View File?", "Download", "View"));
        let to = ScpPath { path: "/a".into(), hostname: "h".into(), username: "u".into() };
        assert_eq!(view_id("image/png", &to), "DownloadOrViewInBrowser_image/png u@h");
        assert!(!BROWSER_ALLOWED);
    }

    #[test]
    fn secure_copy_follows_iterm2() {
        let to = || Some(ScpPath { path: "/w/a.txt".into(), hostname: "h".into(), username: "u".into() });
        assert_eq!(secure_copy_target(Some("a.txt"), |_| to()), to());
        assert_eq!(secure_copy_target(None, |_| to()), None);
        assert_eq!(secure_copy_target(Some("a.txt"), |_| None), None);
    }

    #[test]
    fn remote_click_builds_no_local_run() {
        // A framing conductor: a path, a command word or a Smart Selection action in the text
        // may only download (P10). The remote writes the host report (OSC 1337 RemoteHost,
        // OSC 7), so `localhost` or this PC's name from it changes nothing: the route never
        // reads it.
        assert_eq!(click_route(true, None), ClickRoute::SecureCopyOnly);
        // A web or mail link opens as before.
        for u in ["http://example.com/a", "HTTPS://example.com", "mailto:a@example.com", "ftp://example.com/f"] {
            assert_eq!(click_route(true, Some(u)), ClickRoute::Local, "{u}");
        }
        // A link guard_open would open as a local path, or refuse, may only download.
        for u in ["file:///C:/Users/x/a.txt", "FILE://localhost/C:/a.doc", "file://server/share/a", "C:/a.txt", "www.example.com", "ms-settings:display"] {
            assert_eq!(click_route(true, Some(u)), ClickRoute::SecureCopyOnly, "{u}");
        }
        // No framing conductor is today's click.
        assert_eq!(click_route(false, None), ClickRoute::Local);
        assert_eq!(click_route(false, Some("file:///C:/a.txt")), ClickRoute::Local);
    }

    #[test]
    fn remote_smart_actions_never_run_here() {
        for a in ["openFile:", "openUrl:", "runCommand:", "runCommandInWindow:", "runCoprocess:", "someFutureAction:"] {
            assert!(!remote_action_allowed(true, a), "{a}");
            assert!(remote_action_allowed(false, a), "{a}");
        }
        for a in ["sendText:", "copy:"] {
            assert!(remote_action_allowed(true, a), "{a}");
        }
    }
}
