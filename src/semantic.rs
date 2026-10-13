//! iTerm2's Semantic History: what a Cmd-click (Win-click here) opens. The order is
//! iTerm2's: an OSC 8 link, an existing file (with a line and column when the text says
//! one), a Smart Selection rule's action, the raw command, then URL-like text.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Link(String),
    File { path: PathBuf, line: Option<u32>, column: Option<u32> },
    /// A Smart Selection rule's first action: rule index, action index, matched text.
    Action { rule: usize, action: usize, text: String },
    Url(String),
    RawCommand(String),
    /// The command line of this prompt mark: Win-click on it shows Command Info
    /// (`UAF:275-286`, `UAF:380-399`).
    CommandInfo(u32),
}

/// What is around a click. `before` ends at the clicked character; `after` starts at it.
#[derive(Clone, Debug, Default)]
pub struct ClickContext {
    pub before: String,
    pub after: String,
    /// The OSC 8 link on the clicked cell.
    pub link: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The Smart Selection rule with actions that matched at the click, if any.
    pub smart: Option<(usize, usize, String)>,
    /// Semantic History is set to "raw command".
    pub raw_command: bool,
    /// The URL guessing settings and the pane's locality (`App::url_prefs`).
    pub urls: crate::url_guess::UrlPrefs,
    /// Probe no path on disk (a pane whose text a remote host wrote, hover).
    pub no_probe: bool,
    /// The detection context used a column window (`UAH:86`): hard newlines were not respected.
    pub windowed: bool,
    /// The same click with hard newlines respected, for the file phase's second pass
    /// (`UAF:291-297`); None when the first context already respected them.
    pub respecting: Option<(String, String)>,
    /// The filename characters (`FilenameCharacterSet`); None is iTerm2's default.
    pub filename_set: Option<crate::url_guess::CharSet>,
    /// Ignored folders and mapped drives (`App::path_probe`, P19).
    pub probe: crate::netpath::Probe,
    /// The phase order (`PrioritizeSmartSelectionActions`, `DisableSmartSelectionActionsOnClick`).
    pub order: Order,
    /// The prompt mark whose command line holds the click and that command's byte range in
    /// `before` + `after`; None when `EnableCmdClickPromptForShowCommandInfo` is off or the
    /// click is not on a command (`UAF:275-286`).
    pub prompt: Option<(u32, std::ops::Range<usize>)>,
}

/// Which context a resolved range belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// `before` and `after`.
    Detect,
    /// `respecting`.
    Respecting,
}

/// `MaxSemanticHistoryPrefixOrSuffix`'s default: iTerm2 looks this far each way from the click.
pub const DEFAULT_MAX_CONTEXT: usize = 2000;

/// What the hover banner shows for a target (`TVA:733-748`).
pub fn preview_text(target: &Target, smart_open_url: Option<&str>, show_preview: bool) -> Option<String> {
    let text = match target {
        Target::Link(url) => url.as_str(),
        Target::Url(url) if show_preview => url.as_str(),
        Target::Action { .. } if show_preview => smart_open_url?,
        _ => return None,
    };
    Some(shown_preview(text))
}

/// The most characters a hover banner shows.
pub const PREVIEW_MAX: usize = 512;

/// Program-controlled text (an OSC 8 URI) as a banner shows it: control, bidirectional and
/// zero-width characters dropped, cut at `PREVIEW_MAX` with an ellipsis.
fn shown_preview(text: &str) -> String {
    let clean = crate::inert_files::clean_name(text);
    match clean.char_indices().nth(PREVIEW_MAX) {
        Some((i, _)) => format!("{}\u{2026}", &clean[..i]),
        None => clean,
    }
}

/// Like `resolve`, with the target's byte range in `before` + `after`. The range is None
/// for a link or a Smart Selection action, whose extent the caller knows.
pub fn resolve_range(ctx: &ClickContext) -> Option<(Target, Option<std::ops::Range<usize>>)> {
    resolve_in(ctx).map(|(t, r, _)| (t, r))
}

/// The order the click phases run in (`UAF:186-232`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Default,
    /// `PrioritizeSmartSelectionActions`: Smart Selection right after the link.
    SmartFirst,
    /// `DisableSmartSelectionActionsOnClick`: Smart Selection never runs on a click.
    NoSmart,
}

/// One step of resolving a click (`UAF:31-40`; the respecting pass belongs to `File`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Prompt,
    Link,
    File,
    SmartAction,
    RawCommand,
    UrlLike,
}

/// The order for the two keys; disabling wins (`UAF:186-231`).
pub fn order_from(prioritize: bool, disable: bool) -> Order {
    if disable {
        Order::NoSmart
    } else if prioritize {
        Order::SmartFirst
    } else {
        Order::Default
    }
}

/// The phases a click runs, in order.
pub fn phases(order: Order) -> &'static [Phase] {
    use Phase::*;
    match order {
        Order::Default => &[Prompt, Link, File, SmartAction, RawCommand, UrlLike],
        Order::SmartFirst => &[Prompt, Link, SmartAction, File, RawCommand, UrlLike],
        Order::NoSmart => &[Prompt, Link, File, RawCommand, UrlLike],
    }
}

/// Whether a Win-click acts as a click on its target (`MH:278-295`): a click that was not the
/// window's first acts (when the pane was not focused, the plain press focuses it first and
/// the click still acts); a first click that brought the window forward only activates it
/// unless the pane was already focused and `CmdClickWhenInactiveInvokesSemanticHistory` is on.
pub fn win_click_acts(first_mouse: bool, pane_was_focused: bool, when_inactive: bool) -> bool {
    !first_mouse || (pane_was_focused && when_inactive)
}

/// Whether the Win-hover underline may show (`TVA:466-472`): the window is key or the key is on.
pub fn hover_allowed(window_focused: bool, when_inactive: bool) -> bool {
    window_focused || when_inactive
}

/// Like `resolve_range`, with the context the range belongs to.
pub fn resolve_in(ctx: &ClickContext) -> Option<(Target, Option<std::ops::Range<usize>>, Pass)> {
    let default_set;
    let set = match &ctx.filename_set {
        Some(s) => s,
        None => {
            default_set = default_filename_set();
            &default_set
        }
    };
    // P1: at most this many probes per click, across both passes.
    let budget = crate::netpath::Budget::new(MAX_PROBES);
    for phase in phases(ctx.order) {
        match phase {
            // The click is on a prompt's command line (`UAF:275-286`).
            Phase::Prompt => {
                if let Some((mark, range)) = ctx.prompt.as_ref().filter(|(_, r)| r.contains(&ctx.before.len())) {
                    return Some((Target::CommandInfo(*mark), Some(range.clone()), Pass::Detect));
                }
            }
            Phase::Link => {
                if let Some(link) = &ctx.link {
                    return Some((Target::Link(link.clone()), None, Pass::Detect));
                }
            }
            Phase::File if !ctx.no_probe => {
                if let Some((path, line, column, range)) = path_in(&ctx.before, &ctx.after, ctx.cwd.as_deref(), set, &ctx.probe, &budget) {
                    return Some((Target::File { path, line, column }, Some(range), Pass::Detect));
                }
                // ExistingFileRespectingHardNewlines (`UAF:291-297`).
                if let Some((b, a)) = &ctx.respecting {
                    if let Some((path, line, column, range)) = path_in(b, a, ctx.cwd.as_deref(), set, &ctx.probe, &budget) {
                        return Some((Target::File { path, line, column }, Some(range), Pass::Respecting));
                    }
                }
            }
            Phase::File => {}
            Phase::SmartAction => {
                if let Some((rule, action, text)) = &ctx.smart {
                    return Some((Target::Action { rule: *rule, action: *action, text: text.clone() }, None, Pass::Detect));
                }
            }
            Phase::RawCommand => {
                if ctx.raw_command {
                    let range = word_range(&ctx.before, &ctx.after);
                    if !range.is_empty() {
                        let word = format!("{}{}", ctx.before, ctx.after)[range.clone()].to_string();
                        return Some((Target::RawCommand(word), Some(range), Pass::Detect));
                    }
                }
            }
            Phase::UrlLike => {
                if let Some((u, r)) = crate::url_guess::guess(&ctx.before, &ctx.after, &ctx.urls) {
                    return Some((Target::Url(u), Some(r), Pass::Detect));
                }
            }
        }
    }
    None
}

pub fn resolve(ctx: &ClickContext) -> Option<Target> {
    resolve_range(ctx).map(|(t, _)| t)
}

/// iTerm2 splits the text around a click into chunks at these characters, and tries
/// paths made of up to this many chunks each way.
const SEPARATORS: &[char] = &['\t', ' ', '(', ')', ':', '"', ','];
const MAX_CHUNKS: usize = 10;

/// The chunk under the click, as a range of `before` + `after`.
fn word_range(before: &str, after: &str) -> std::ops::Range<usize> {
    let start = before.rfind(SEPARATORS).map_or(0, |i| i + 1);
    let end = after.find(SEPARATORS).unwrap_or(after.len());
    start..before.len() + end
}

/// Where candidate paths may start in `before` (nearest first) and end in `after`.
fn chunk_bounds(before: &str, after: &str) -> (Vec<usize>, Vec<usize>) {
    let mut starts: Vec<usize> = before.char_indices().rev().filter(|(_, c)| SEPARATORS.contains(c)).map(|(i, c)| i + c.len_utf8()).collect();
    starts.push(0);
    starts.dedup();
    starts.truncate(MAX_CHUNKS + 1);
    let mut ends: Vec<usize> = after.char_indices().filter(|(_, c)| SEPARATORS.contains(c)).map(|(i, _)| i).collect();
    ends.push(after.len());
    ends.dedup();
    ends.truncate(MAX_CHUNKS);
    (starts, ends)
}

/// Trims what cannot end or start a path: quotes, brackets, and trailing punctuation.
/// Returns the kept byte range of `s`.
fn clean_range(s: &str) -> (usize, usize) {
    let mut l = 0;
    let mut r = s.len();
    loop {
        let t = &s[l..r];
        if let Some(c) = t.chars().next().filter(|c| c.is_whitespace() || "[<'`".contains(*c)) {
            l += c.len_utf8();
        } else if t.ends_with("...") {
            r -= 3;
        } else if let Some(c) = t.chars().last().filter(|c| c.is_whitespace() || "!?.,;:]>'`\u{2026}".contains(*c)) {
            r -= c.len_utf8();
        } else {
            return (l, r);
        }
    }
}

fn line_patterns() -> &'static [regex::Regex] {
    static P: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        [r"^:(\d+)(?::(\d+))?", r"^\((\d+)(?:,\s*(\d+))?\)", r"^\[(\d+)(?:,\s*(\d+))?\]", r#"^"?, line (\d+)"#]
            .iter()
            .map(|p| regex::Regex::new(p).expect("line pattern"))
            .collect()
    })
}

/// The line and column that `rest` (the text right after a path) starts with.
fn line_and_column(rest: &str) -> (Option<u32>, Option<u32>) {
    for re in line_patterns() {
        if let Some(c) = re.captures(rest) {
            let n = |i: usize| c.get(i).and_then(|m| m.as_str().parse().ok());
            return (n(1), n(2));
        }
    }
    (None, None)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// The most files one click probes (P1), across both passes.
const MAX_PROBES: usize = 120;

#[cfg(test)]
thread_local! {
    /// How many candidates `existing` probed on this thread (tests).
    static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one probe (an `exists` call or a link read) for the budget test.
#[cfg(test)]
pub(crate) fn count_probe() {
    PROBES.with(|n| n.set(n.get() + 1));
}

/// The existing file or folder `text` names, relative to `cwd`.
fn existing(text: &str, cwd: Option<&Path>, probe: &crate::netpath::Probe, budget: &crate::netpath::Budget) -> Option<PathBuf> {
    let text = text.replace("\\ ", " ");
    let mut tries = vec![text.clone()];
    for prefix in ["a/", "b/", "i/", "w/", "c/", "o/"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            tries.push(rest.to_string());
        }
    }
    // A share name ("\\\\host\\x", "//host/x") or URL text would make Windows try the
    // network, which can take 20 s and hands the host the user's credentials.
    // The repo's one rule (scripts::is_network_path) covers every mixed-separator spelling.
    tries.retain(|t| !(crate::scripts::is_network_path(t) || t.contains("://")));
    tries.into_iter().find_map(|t| {
        let p = match t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
            Some(rest) => home()?.join(rest),
            None if Path::new(&t).is_absolute() => PathBuf::from(&t),
            None => cwd?.join(&t),
        };
        // After the join, so a network home or working folder cannot slip in (a working
        // folder that may not be probed hides every relative candidate under it), and only
        // through folders and links that stay local, outside ignored folders and mapped
        // drives (`netpath::may_probe`, `IPF:fileExistsAtPathLocally`). Never a probe first.
        if budget.left() == 0 {
            return None;
        }
        let local = crate::localfs::LocalPath::semantic_with(&p.to_string_lossy(), probe, budget)?;
        if !budget.spend() {
            return None;
        }
        crate::localfs::exists(&local).then_some(p)
    })
}

/// An existing file or folder around the click, with a line and column when the text
/// after it gives them.
pub fn find_path(before: &str, after: &str, cwd: Option<&Path>, probe: &crate::netpath::Probe) -> Option<(PathBuf, Option<u32>, Option<u32>)> {
    path_in(before, after, cwd, &default_filename_set(), probe, &crate::netpath::Budget::new(MAX_PROBES)).map(|(p, l, c, _)| (p, l, c))
}

type PathHit = (PathBuf, Option<u32>, Option<u32>, std::ops::Range<usize>);

/// iTerm2's default filename characters.
fn default_filename_set() -> crate::url_guess::CharSet {
    crate::url_guess::filename(&crate::url_guess::CharSet::default(), crate::url_guess::DEFAULT_FILENAME_CHARS)
}

/// The path hit's range is in the coordinates of `before` + `after` as given.
fn path_in(before: &str, after: &str, cwd: Option<&Path>, set: &crate::url_guess::CharSet, probe: &crate::netpath::Probe, budget: &crate::netpath::Budget) -> Option<PathHit> {
    // Only the filename characters around the click are candidates (`UAF:437-452`).
    let (b, a) = crate::sh_context::filename_parts(before, after, set);
    let shift = before.len() - b.len();
    let whole = format!("{before}{after}");
    let (before, after) = (b.as_str(), a.as_str());
    let (starts, ends) = chunk_bounds(before, after);
    // The longest candidate that exists wins, so a path with spaces beats its last word.
    let mut best: Option<(std::ops::Range<usize>, PathBuf)> = None;
    for &s in &starts {
        for &e in &ends {
            let joined = format!("{}{}", &before[s..], &after[..e]);
            let (l, r) = clean_range(&joined);
            if l >= r || best.as_ref().is_some_and(|(range, _)| range.len() >= r - l) {
                continue;
            }
            if let Some(p) = existing(&joined[l..r], cwd, probe, budget) {
                best = Some((s + l..s + r, p));
            }
        }
    }
    best.map(|(range, p)| {
        let range = range.start + shift..range.end + shift;
        // The line and column come from the whole suffix after the path, as iTerm2 reads them
        // (`UAF:524-528` passes the un-narrowed located suffix to `cleanedUpPathFromPath:suffix:`).
        let (line, column) = line_and_column(&whole[range.end..]);
        (p, line, column, range)
    })
}

/// URL-like text around the click, made into a URL (13s's signature; the settings are the
/// defaults but for the slash rule).
pub fn url_like(before: &str, after: &str, require_slash: bool) -> Option<String> {
    url_like_in(before, after, &crate::url_guess::UrlPrefs { require_slash, ..Default::default() })
}

/// `url_like` under every URL setting (B6): the click, the menu and Copy URL all call this.
pub fn url_like_in(before: &str, after: &str, prefs: &crate::url_guess::UrlPrefs) -> Option<String> {
    crate::url_guess::guess(before, after, prefs).map(|(u, _)| u)
}

/// 13s's `looks_like_url` (`UAF:896-941`) under the default URL characters.
pub fn looks_like_url(s: &str, require_slash: bool) -> bool {
    crate::url_guess::looks_like_url(s, require_slash, &crate::url_guess::CharSet::default())
}

/// A profile's Semantic History setting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    /// "best editor", "url", "editor", "command", "raw command", "coprocess", or "send text".
    pub action: String,
    /// The URL, command, or text for those actions.
    pub text: String,
    /// The editor for "editor".
    pub editor: String,
}

impl Prefs {
    pub fn from_value(v: Option<&toml::Value>) -> Prefs {
        let t = v.and_then(|v| v.as_table());
        let s = |k: &str| t.and_then(|t| t.get(k)).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let action = s("action");
        Prefs { action: if action.is_empty() { "best editor".into() } else { action }, text: s("text"), editor: s("editor") }
    }
}

/// What opening a target runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    /// Open a file, folder, or URL with its default app.
    Open(String),
    /// A command line for cmd.exe, run on its own.
    Command(String),
    /// Text typed into the session.
    Send(String),
    /// An action a later milestone brings: its name and the milestone.
    Later(&'static str, u8),
    /// A coprocess command for the session.
    Coprocess(String),
    /// Show a file in Explorer instead of running it.
    Reveal(String),
    /// A local file the caller checks off the UI thread (`open_local_result`): the network-drive
    /// and ignored-folder rule, then its existence, then `file_open`. Nothing here touches the disk.
    OpenLocal { path: String, has_code: bool },
    /// A program started directly with arguments, never through a shell.
    Spawn { program: PathBuf, args: Vec<String> },
    /// A command line run in a new single-use window of the default profile.
    Window(String),
    /// A link of another app's scheme (`vscode:`): the caller asks once per scheme, then hands it
    /// to the app Windows registered (`link_scheme`). Never a shell string.
    OpenScheme(String),
    /// Not opened, and why.
    Blocked(String),
}

/// A bare drive root (`C:\`, `c:/`) as `C:\.`: the same folder, with no trailing separator, so it is
/// safe in cmd.exe, PowerShell and POSIX quoting alike (final wave ruling). None for anything else.
pub fn drive_root_form(v: &str) -> Option<String> {
    let b = v.as_bytes();
    (b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')).then(|| format!("{}:\\.", b[0] as char))
}

/// Whether a substituted value is refused on a cmd.exe command line. The rule holds whether
/// or not the template quotes the placeholder, so it never depends on quote state: cmd.exe
/// metacharacters (`& | < > ^ " % ! ( ) ` + "`" + `), every control character (CR, LF, NUL included),
/// the quote lookalikes a program may fold to `"` (U+FF02, U+201C, U+201D, U+2018, U+2019), and
/// a trailing backslash (it would escape the closing quote for the child's argument parser).
/// Quoting stays as defence in depth.
pub fn cmd_unsafe(v: &str) -> bool {
    v.ends_with('\\')
        || v.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '"' | '%' | '!' | '(' | ')' | '`' | '\u{ff02}' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}') || c.is_control())
}

/// `fill` for a cmd.exe command line, through the one builder: None when a value is refused.
fn fill_cmd(template: &str, values: [&str; 5]) -> Option<String> {
    build_cmd_line(&fill_pieces(template, values))
}

/// A piece of a cmd.exe command line template: the person's own text, or a value substituted into it.
#[derive(Clone, Debug, PartialEq)]
pub enum CmdPiece {
    Text(String),
    Value(String),
}

/// THE builder of a cmd.exe command line from a template and the values substituted into it
/// (Semantic History, Smart Selection, triggers, Capture Output, file drops, interpolated
/// templates). The template's text is the person's and stays as written. Each value is refused
/// when `cmd_unsafe` (the context-free list: None, nothing runs); otherwise it goes in bare
/// where the template's own quotes enclose it, and in quotes of its own elsewhere, so a value
/// with spaces is always one argument for the program cmd.exe starts (a second pair inside the
/// template's quotes would close them). The whole line then goes to cmd.exe as `cmd_args`.
pub fn build_cmd_line(pieces: &[CmdPiece]) -> Option<String> {
    let mut out = String::new();
    let mut quoted = false;
    for p in pieces {
        // A drive root is safe only in its `X:\.` form (no trailing backslash).
        let root = if let CmdPiece::Value(v) = p { drive_root_form(v) } else { None };
        let p = &root.map_or_else(|| p.clone(), CmdPiece::Value);
        match p {
            CmdPiece::Text(t) => {
                quoted ^= t.chars().filter(|&c| c == '"').count() % 2 == 1;
                out.push_str(t);
            }
            CmdPiece::Value(v) if cmd_unsafe(v) => return None,
            CmdPiece::Value(v) if quoted => out.push_str(v),
            CmdPiece::Value(v) => out.push_str(&cmd_quote(v)),
        }
    }
    Some(out)
}

/// The arguments cmd.exe gets for a command line that holds substituted program text: `/s`
/// makes it strip exactly the quotes added here, so the quotes around a value (which may be
/// the first thing on the line) stay and `& | < > ^ ( )` inside them are inert; `/v:off` stops
/// `!VAR!` rewriting a value when the registry turns delayed expansion on; `/d` skips
/// AutoRun. The one place a Termurion command line for cmd.exe is built (Semantic History,
/// Smart Selection, triggers and coprocesses).
pub fn cmd_args(line: &str) -> String {
    format!("/d /v:off /s /c \"{line}\"")
}

/// `cmd_args` as the whole command line handed to CreateProcess.
pub fn cmd_command_line(line: &str) -> String {
    format!("cmd.exe {}", cmd_args(line))
}

/// What a refused value in a Send Text says.
pub const SEND_REFUSED: &str = "Termurion did not type this text: a value in it holds a control character (a line break would run a second command).";

/// Whether any substituted value of a Send Text holds a control character; the template's own
/// text (its `\n`) is not a value and stays.
pub fn send_unsafe(values: &[&str]) -> bool {
    values.iter().any(|v| v.chars().any(char::is_control))
}

pub const CMD_REFUSED: &str = "Termurion did not run this command: a value in it holds a character cmd.exe would read as more than text (& | < > ^ \" % ! ( ) `, a control character, a quote mark, or a trailing backslash).";

/// Arguments a batch file (or cmd.exe) would re-read as syntax: refused for a `.cmd` or `.bat`
/// program (the BatchUnsafe set).
pub fn batch_unsafe(arg: &str) -> bool {
    arg.contains(['%', '^', '&', '|', '<', '>', '"']) || arg.chars().any(char::is_control)
}

/// The program a handler command names, looked up through `exists`: a name with a folder as
/// written (with each PATHEXT extension when it has none), a bare name in each PATH folder in
/// order, each with the name as written (only when it has an extension: an extensionless file
/// is no program, VS Code's `code` is a shell script beside `code.cmd`) and then each PATHEXT
/// extension. A network path is never asked (UNC invariant).
pub fn resolve_program(name: &str, path_var: &str, pathext: &str, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let exts: Vec<&str> = pathext.split(';').map(str::trim).filter(|e| !e.is_empty()).collect();
    let has_ext = Path::new(name).extension().is_some();
    let candidates = |folder: Option<&str>| -> Vec<String> {
        let base = match folder {
            Some(f) => format!("{}\\{}", f.trim_end_matches(['\\', '/']), name),
            None => name.to_string(),
        };
        let bare = has_ext.then(|| base.clone());
        bare.into_iter().chain(exts.iter().map(|e| format!("{base}{}", e.to_ascii_lowercase()))).collect()
    };
    let try_all = |list: Vec<String>| list.into_iter().filter(|c| !crate::scripts::is_network_path(c)).map(PathBuf::from).find(|p| exists(p));
    let folder_given = name.contains(['\\', '/']) || Path::new(name).is_absolute();
    if folder_given {
        // A relative name with a folder would resolve against Termurion's own current folder.
        if !Path::new(name).is_absolute() {
            return None;
        }
        return try_all(candidates(None));
    }
    path_var.split(';').map(str::trim).filter(|d| !d.is_empty() && !crate::scripts::is_network_path(d)).find_map(|d| try_all(candidates(Some(d))))
}

/// `resolve_program` over the real file system and the PATH the caller gives (the editor's
/// spawn): the same walk, PATHEXT in the order PATH searches use.
pub fn resolve_on_path(name: &Path, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let name = name.to_string_lossy();
    let path_var = path.map(|p| std::env::split_paths(p).map(|d| d.to_string_lossy().to_string()).collect::<Vec<_>>().join(";")).unwrap_or_default();
    resolve_program(&name, &path_var, ".COM;.EXE;.BAT;.CMD", &|p| crate::localfs::is_local_file(p))
}

/// Whether a program's arguments are parsed by cmd.exe: a batch file, or cmd.exe itself.
pub fn is_batch(program: &Path) -> bool {
    program.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        || program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("cmd"))
}

/// The window title of a URL handler's Script Console entry (`UAH:340-375`).
pub const HANDLER_TITLE: &str = "URL Handler";
/// The banner when a URL handler command fails (`UAH`).
pub const HANDLER_FAILED: &str = "URL Handler Command Failed";

/// A private-use marker bound to `url` while the handler template is evaluated, so the URL
/// never passes through the evaluator or the splitter.
pub const URL_TOKEN: &str = "\u{E000}termurion-url\u{E000}";

/// The marker bound to the template's `n`th session variable while it is evaluated.
pub fn var_token(n: usize) -> String {
    format!("\u{E000}termurion-var-{n}\u{E000}")
}

/// Why a URL handler command is not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    UrlInProgram,
    ControlCharacter,
    BatchUnsafe,
    /// The program is not on the PATH (P14): never started "as written".
    Unresolved,
    /// The program hands its arguments to a shell, a console host or another program
    /// (`FORWARDERS`): never a URL handler.
    Forwarder,
    /// A session variable is or starts the program token.
    VariableInProgram,
    /// A session variable holds what the context-free list refuses (`cmd_unsafe`), what the
    /// program would read as syntax, or the marker character.
    VariableUnsafe,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::Empty => "",
            Refusal::UrlInProgram => "Termurion did not run the URL handler command: the URL cannot name the program.",
            Refusal::ControlCharacter => "Termurion did not pass this URL to the URL handler command: it contains a control character.",
            Refusal::BatchUnsafe => "Termurion did not pass this URL to the URL handler command: a batch file or cmd.exe would read its special characters.",
            Refusal::Unresolved => HANDLER_FAILED,
            Refusal::Forwarder => "Termurion did not run the URL handler command: its program hands its arguments to a shell or another program, so it cannot be a URL handler.",
            Refusal::VariableInProgram => "Termurion did not run the URL handler command: a session variable cannot name the program.",
            Refusal::VariableUnsafe => "Termurion did not run the URL handler command: a session variable in it holds a quote, a control character or another character a command reader would act on.",
        }
    }
}

/// `CommandLineToArgvW`'s split of a command line: the first token by the program-name rule
/// (quotes toggle, backslashes are literal), the rest by the argument rules (2n backslashes
/// before a quote give n and a quote toggle, 2n+1 give n and a literal quote, `""` inside
/// quotes a literal quote).
pub fn split_command_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let skip = |i: &mut usize| {
        while *i < chars.len() && matches!(chars[*i], ' ' | '\t') {
            *i += 1;
        }
    };
    skip(&mut i);
    if i >= chars.len() {
        return out;
    }
    let mut program = String::new();
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && matches!(c, ' ' | '\t') {
            break;
        } else {
            program.push(c);
        }
        i += 1;
    }
    out.push(program);
    loop {
        skip(&mut i);
        if i >= chars.len() {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                let start = i;
                while i < chars.len() && chars[i] == '\\' {
                    i += 1;
                }
                let n = i - start;
                if chars.get(i) == Some(&'"') {
                    arg.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        arg.push('"');
                        i += 1;
                    }
                    // An even run leaves the quote for the toggle below.
                } else {
                    arg.extend(std::iter::repeat_n('\\', n));
                }
            } else if c == '"' {
                if quoted && chars.get(i + 1) == Some(&'"') {
                    // shell32 (unlike the C runtime) ends the quoted run here.
                    arg.push('"');
                    quoted = false;
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else if !quoted && matches!(c, ' ' | '\t') {
                break;
            } else {
                arg.push(c);
                i += 1;
            }
        }
        out.push(arg);
    }
    out
}

/// Run Command in Window (`IC:2147-2154`): a command ending in `&` with more before it runs
/// the rest hidden; any other runs in a single-use window, through the one cmd.exe builder
/// (a bare `/c` would let the first quote of a value change how cmd.exe reads the line).
pub fn run_in_window(command: &str) -> Run {
    match command.strip_suffix('&') {
        Some(rest) if !rest.is_empty() => Run::Command(rest.to_string()),
        _ => Run::Window(cmd_command_line(command)),
    }
}

/// Whether a URL handler is asked for this run: an http, https, mailto or ftp URL only, and
/// never for a file (a `Target::File`, a `file:` link or a path).
pub fn handler_applies(run: &Run, from_file: bool) -> bool {
    match run {
        Run::Open(u) if !from_file => !crate::scripts::is_network_path(u) && scheme(u).is_some_and(|s| WEB.contains(&s.as_str())),
        _ => false,
    }
}

/// Programs that re-read their arguments as script: `cmd.exe` and batch files (`is_batch`), and
/// the interpreters and shells Pass A ruling 2 names (P13).
fn interpreter(program: &Path) -> bool {
    let stem = program.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(stem.as_str(), "powershell" | "pwsh" | "wscript" | "cscript" | "mshta" | "wsl" | "bash")
}

/// Programs that hand their arguments on to a shell, a console host or another program
/// (forwarders, other shells, Windows' signed launchers, cmd.exe itself): never a URL handler,
/// whatever the URL, matched by the resolved file name's stem (any folder, any extension).
const FORWARDERS: &[&str] = &["conhost", "forfiles", "wt", "sh", "zsh", "rundll32", "regsvr32", "msiexec", "certutil", "cmd"];

fn forwarder(program: &Path) -> bool {
    let stem = program.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    FORWARDERS.contains(&stem.as_str())
}

/// Whether `url` holds a character the program would read as syntax: `% ^ & | < > "` and any
/// control character for a batch file or cmd.exe, and for the interpreters also semicolon, dollar, quotes, backtick, parentheses, braces and `!`.
fn reparsed(program: &Path, url: &str) -> bool {
    if is_batch(program) {
        return batch_unsafe(url);
    }
    interpreter(program) && (batch_unsafe(url) || url.contains([';', '$', '\'', '`', '(', ')', '{', '}', '!']))
}

/// The program and arguments a URL handler command starts, never through a shell (13p-r).
/// `evaluated` holds `URL_TOKEN` for the URL and `var_token(n)` for `vars[n]`, the template's
/// session variables (program text: OSC 7 folders, titles, hosts): each is put back after the
/// split, so it is exactly one argument or part of one, never the program, and each passes
/// the context-free list (`cmd_unsafe`) and the program's own syntax rule (`reparsed`).
pub fn handler_plan(evaluated: &str, url: &str, vars: &[String], resolve: &dyn Fn(&str) -> Option<PathBuf>) -> Result<Run, Refusal> {
    if evaluated.trim().is_empty() {
        return Err(Refusal::Empty);
    }
    if url.chars().any(char::is_control) {
        return Err(Refusal::ControlCharacter);
    }
    let tokens = split_command_line(evaluated);
    let Some((first, rest)) = tokens.split_first() else { return Err(Refusal::Empty) };
    if first.contains(URL_TOKEN) {
        return Err(Refusal::UrlInProgram);
    }
    if first.contains('\u{E000}') {
        return Err(Refusal::VariableInProgram);
    }
    if vars.iter().any(|v| cmd_unsafe(v) || v.contains('\u{E000}')) {
        return Err(Refusal::VariableUnsafe);
    }
    let program = resolve(first).ok_or(Refusal::Unresolved)?;
    if forwarder(&program) {
        return Err(Refusal::Forwarder);
    }
    if vars.iter().any(|v| reparsed(&program, v)) {
        return Err(Refusal::VariableUnsafe);
    }
    let mut held = false;
    let mut args: Vec<String> = rest
        .iter()
        .map(|t| {
            held |= t.contains(URL_TOKEN);
            let mut t = t.clone();
            for (n, v) in vars.iter().enumerate() {
                t = t.replace(&var_token(n), v);
            }
            t.replace(URL_TOKEN, url)
        })
        .collect();
    if !held {
        args.push(url.to_string());
    }
    if reparsed(&program, url) {
        return Err(Refusal::BatchUnsafe);
    }
    Ok(Run::Spawn { program, args })
}

/// The link schemes that open in the browser or mail app.
const WEB: &[&str] = &["http", "https", "mailto", "ftp"];

/// The lowercased scheme of a URL; None for a path.
fn scheme(target: &str) -> Option<String> {
    let s = target.split_once(':').map(|(s, _)| s).filter(|s| {
        // A drive letter ("C:") is a path, not a scheme.
        s.len() > 1 && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    })?;
    Some(s.to_ascii_lowercase())
}

/// The links remote text may open (P10): ftp is not one, since Explorer or a registered
/// client may take it, not only the browser.
const REMOTE_WEB: &[&str] = &["http", "https", "mailto"];

/// Whether `target` is a web or mail link remote text may open: guard_open hands it to the
/// browser or mail app, never opens a path.
pub fn web_link(target: &str) -> bool {
    scheme(target).is_some_and(|s| REMOTE_WEB.contains(&s.as_str()))
}

/// What opening `target` runs. `target` is text a program may have written (an OSC 8 link,
/// a URL found in output, a Smart Selection `openUrl:`), so only http, https, mailto, and ftp
/// links open: a target with no scheme (a path, a drive path, a network path) is refused (B1),
/// a network path first of all, and a file: link is a local path through `file_open`.
pub fn guard_open(target: &str, has_code: bool) -> Run {
    // The one network rule, shared with the browser's gate (R13x23 fix pass item 3): percent-encoded
    // and host forms of a network `file:` link are refused here too, before anything else.
    if let Some(why) = crate::browser_js::gate_refusal(target) {
        return Run::Blocked(why);
    }
    if crate::scripts::is_network_path(target) {
        return Run::Blocked(format!("Termurion does not open network paths: {target}"));
    }
    match scheme(target).as_deref() {
        None => Run::Blocked("Termurion opens only http, https, mailto, and ftp links, not paths or other text.".into()),
        Some(s) if WEB.contains(&s) => Run::Open(encode_url_chars(target)),
        Some("file") => match file_url_path(target) {
            Some(p) if !crate::scripts::is_network_path(&p) => Run::OpenLocal { path: p, has_code },
            _ => Run::Blocked(format!("Termurion does not open network file links: {target}")),
        },
        // An ssh: link a bound handler would have opened never gets here; an unbound one is not
        // handed to whatever the system registered (14e's Decision).
        Some("ssh") => Run::Blocked("Termurion opens ssh: links when ssh is set to open in Termurion (Settings > Profiles > General > URL schemes).".into()),
        Some(s) => match crate::link_scheme::screen(s, target) {
            Ok(()) => Run::OpenScheme(target.to_string()),
            Err(why) => Run::Blocked(why),
        },
    }
}

/// A Smart Selection `openFile:`: the parameter is capture text, so it is only ever a path
/// (never a protocol), resolved against the pane's folder, local, existing, and opened by its
/// default app; a program or script is shown, not run, and no editor command line is built.
pub fn open_file_run(path: &str, cwd: Option<&Path>, _has_code: bool) -> Run {
    let blocked = |why: &str| Run::Blocked(format!("Termurion did not open this file: {why}"));
    if path.chars().any(char::is_control) {
        return blocked("its name holds a control character.");
    }
    if crate::scripts::is_network_path(path) {
        return Run::Blocked(format!("Termurion does not open network paths: {path}"));
    }
    if scheme(path).is_some() {
        return blocked("it is a link, not a path.");
    }
    let given = Path::new(path);
    let full = if given.is_absolute() {
        given.to_path_buf()
    } else {
        match cwd {
            Some(c) => c.join(given),
            None => return blocked("it has no folder to start from."),
        }
    };
    let text = full.to_string_lossy().to_string();
    if crate::scripts::is_network_path(&text) {
        return Run::Blocked(format!("Termurion does not open network paths: {path}"));
    }
    // The drive, the ignored folders, the links and the file's existence are asked off the UI thread.
    Run::OpenLocal { path: text, has_code: false }
}

/// What `Run::OpenLocal` becomes: the one probe rule first (`netpath::may_probe`: a network path,
/// a mapped drive or an ignored folder is never touched), then `exists`, then `file_open`. Runs
/// on a worker, so a slow drive never freezes the window.
pub fn open_local_result(path: &str, has_code: bool, probe: &crate::netpath::Probe, exists: &dyn Fn(&str) -> bool) -> Run {
    if !crate::netpath::may_probe(path, probe) {
        return Run::Blocked(format!("Termurion does not open network paths: {path}"));
    }
    if !exists(path) {
        return Run::Blocked("Termurion did not open this file: it does not exist.".into());
    }
    file_open(path, None, None, has_code)
}

/// A Smart Selection `openUrl:`: a link, through the same guard as every other.
pub fn open_url_run(text: &str, has_code: bool) -> Run {
    guard_open(text, has_code)
}

/// Programs and scripts: "open" would run them, so Termurion shows them instead. The one rule is the
/// write door's, `inert_files::high_risk`: the name as Windows opens it (`inert_files::opened_name`:
/// a stream suffix, trailing dots and spaces dropped, and any stream fails closed), the one list,
/// and the machine's own PATHEXT.
pub(crate) fn runnable(path: &str) -> bool {
    crate::inert_files::high_risk(path)
}

/// A local file: link as a Windows path. None for a link to another machine.
pub(crate) fn file_url_path(url: &str) -> Option<String> {
    let rest = &url[url.find(':')? + 1..];
    let rest = rest.strip_prefix("//").map_or(Some(rest), |r| {
        let (host, path) = r.split_at(r.find('/').unwrap_or(r.len()));
        (host.is_empty() || host.eq_ignore_ascii_case("localhost")).then_some(path)
    })?;
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&out).to_string();
    // "/C:/x" is "C:\x".
    let path = match path.strip_prefix('/') {
        Some(p) if p.as_bytes().get(1) == Some(&b':') => p.to_string(),
        _ => path,
    };
    let path = path.replace('/', "\\");
    (!crate::scripts::is_network_path(&path)).then_some(path)
}

/// VS Code's `-g` argument: file:line:column.
fn goto_text(p: &str, line: Option<u32>, column: Option<u32>) -> String {
    let mut at = p.to_string();
    if let Some(n) = line {
        at.push_str(&format!(":{n}"));
        if let Some(c) = column {
            at.push_str(&format!(":{c}"));
        }
    }
    at
}

/// Opening a file: its default app, except a program or script, which VS Code opens when
/// it is on the PATH and Explorer shows otherwise.
fn file_open(p: &str, line: Option<u32>, column: Option<u32>, has_code: bool) -> Run {
    if has_code && (line.is_some() || runnable(p)) {
        Run::Spawn { program: PathBuf::from("code"), args: vec!["-g".into(), goto_text(p, line, column)] }
    } else if runnable(p) {
        Run::Reveal(p.to_string())
    } else {
        Run::Open(p.to_string())
    }
}

/// The text around the click and the session's folder, for \\3, \\4, and \\5.
#[derive(Clone, Debug, Default)]
pub struct Around<'a> {
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub cwd: &'a str,
}

/// explorer.exe in the Windows folder, the parent of the System32 folder `system`
/// (`GetSystemDirectoryW`): run by its full path, never found by the search order.
pub fn explorer_in(system: &Path) -> Option<PathBuf> {
    system.parent().map(|windows| windows.join("explorer.exe"))
}

/// The one builder of the argument explorer.exe gets to select `path` (every Reveal: Win-click,
/// document links, scripts, downloads, the allow-all file): `/select,"<path>"`, passed raw. None
/// when the path holds a quote (it would end the argument and let the rest add Explorer
/// switches, `/root,\\host\share`), a control character, or is a network path.
pub fn explorer_select_arg(path: &str) -> Option<String> {
    let refused = path.contains('"') || path.chars().any(char::is_control) || crate::scripts::is_network_path(path);
    (!refused).then(|| format!("/select,{}", cmd_quote(path)))
}

/// Quotes a value for cmd.exe.
pub fn cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// A URL with every character a ShellExecute handler could read as more than the URL percent-
/// encoded, as iTerm2's NSURL does: `"`, `<`, `>`, a backtick, `^`, `{`, `}`, `|`, `\`, space,
/// every control character and everything outside ASCII. The rest (including an existing `%`) stays.
pub fn encode_url_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() && !c.is_ascii_control() && !matches!(c, '"' | '<' | '>' | '`' | '^' | '{' | '}' | '|' | '\\' | ' ') {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// What `shell_open` hands ShellExecute: a web or mail URL is percent-encoded (`encode_url_chars`),
/// anything else (a settings page, another scheme) is passed as written.
pub fn shell_target(target: &str) -> String {
    if web_link(target) { encode_url_chars(target) } else { target.to_string() }
}

/// The schemes the URL sink hands the shell besides Termurion's registered ones (`url_schemes::SCHEMES`):
/// web and mail links, and the Settings app's pages (Default apps).
const SHELL_SCHEMES: &[&str] = &["http", "https", "mailto", "ms-settings"];

/// The URL sink's input (judge round 5): `target` as `shell_target` gives it, only when it is a
/// URL of a scheme Termurion means to open (`SHELL_SCHEMES` and Termurion's own ftp, ssh and telnet) and no
/// network path. A path, a drive path, a file link, a shell namespace (`search-ms:`, `shell:`) or any
/// other protocol handler is None (round 5 item 3; a file opens only as a judged `LocalPath`,
/// `localfs::open_with_shell`).
pub fn shell_url(target: &str) -> Option<String> {
    let url = scheme(target).is_some_and(|s| SHELL_SCHEMES.contains(&s.as_str()) || crate::url_schemes::SCHEMES.contains(&s.as_str())) && !crate::scripts::is_network_path(target);
    url.then(|| shell_target(target))
}

/// Percent-encodes everything but letters, digits, and "-._~".
pub fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Fills iTerm2's \\1 (file), \\2 (line), \\3 (text before the click), \\4 (text after it),
/// and \\5 (working directory) into `template`, each passed through `each`.
pub fn fill(template: &str, values: [&str; 5], each: impl Fn(&str) -> String) -> String {
    fill_pieces(template, values)
        .into_iter()
        .map(|p| match p {
            CmdPiece::Text(t) => t,
            CmdPiece::Value(v) => each(&v),
        })
        .collect()
}

/// `fill`'s template split into its text and the values for \\1 to \\5.
fn fill_pieces(template: &str, values: [&str; 5]) -> Vec<CmdPiece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some(d @ '1'..='5')) => {
                out.push(CmdPiece::Text(std::mem::take(&mut text)));
                out.push(CmdPiece::Value(values[*d as usize - '1' as usize].to_string()));
                chars.next();
            }
            _ => text.push(c),
        }
    }
    out.push(CmdPiece::Text(text));
    out
}

/// What a Win-click on `target` runs under `prefs`. `has_code` says VS Code is on the
/// PATH, which the best editor uses to go to a line. Smart Selection actions are the
/// caller's (they carry their own parameters).
pub fn plan(target: &Target, prefs: &Prefs, around: &Around, has_code: bool) -> Option<Run> {
    match target {
        Target::Link(u) | Target::Url(u) => Some(guard_open(u, has_code)),
        // The app shows its window; nothing runs.
        Target::Action { .. } | Target::CommandInfo(_) => None,
        Target::RawCommand(word) => (prefs.action == "raw command").then(|| {
            fill_cmd(&prefs.text, [word, "", around.prefix, around.suffix, around.cwd]).map_or_else(|| Run::Blocked(CMD_REFUSED.into()), Run::Command)
        }),
        Target::File { path, line, column } => {
            let p = path.to_string_lossy().to_string();
            let l = line.map(|n| n.to_string()).unwrap_or_default();
            let values = [p.as_str(), l.as_str(), around.prefix, around.suffix, around.cwd];
            let cmd = |make: fn(String) -> Run| fill_cmd(&prefs.text, values).map_or_else(|| Run::Blocked(CMD_REFUSED.into()), make);
            Some(match prefs.action.as_str() {
                "url" => guard_open(&fill(&prefs.text, values, percent_encode), has_code),
                // The editor starts directly with the path as one argument, never through cmd.exe.
                "editor" if prefs.editor.to_lowercase().contains("code") => {
                    Run::Spawn { program: PathBuf::from(&prefs.editor), args: vec!["-g".into(), goto_text(&p, *line, *column)] }
                }
                "editor" => Run::Spawn { program: PathBuf::from(&prefs.editor), args: vec![p.clone()] },
                "command" | "raw command" => cmd(Run::Command),
                "coprocess" => cmd(Run::Coprocess),
                "send text" if send_unsafe(&values) => Run::Blocked(SEND_REFUSED.into()),
                "send text" => Run::Send(fill(&prefs.text, values, str::to_string)),
                _ => file_open(&p, *line, *column, has_code),
            })
        }
    }
}

/// Whether a plan acts on the pane's own shell (Send Text, Run Coprocess): an ended session
/// has none, so the action is refused with a message (13q S6, 13r P11).
pub fn needs_live_session(run: &Run) -> bool {
    matches!(run, Run::Send(_) | Run::Coprocess(_))
}

#[cfg(test)]
mod tests {
    /// Final wave item 4: a web URL reaches ShellExecute with no quote, space or control character.
    #[test]
    fn web_links_reach_shellexecute_encoded() {
        let open = |u: &str| match guard_open(u, false) {
            Run::Open(t) => t,
            other => panic!("{other:?}"),
        };
        assert_eq!(open("mailto:a@b?x=\" /a C:\\secret.txt"), "mailto:a@b?x=%22%20/a%20C:%5Csecret.txt");
        assert_eq!(open("https://a.b/x\r\ny"), "https://a.b/x%0D%0Ay");
        assert_eq!(open("https://a.b/<>`^{}|\u{e9}"), "https://a.b/%3C%3E%60%5E%7B%7D%7C%C3%A9");
        assert_eq!(open("https://a.b/p%20q?x=1&y=2#f"), "https://a.b/p%20q?x=1&y=2#f", "a valid URL is unchanged");
        let encoded = open("mailto:a@b?x=\" /a C:\\secret.txt");
        assert!(!encoded.contains(['"', ' ', '\\']));
    }

    /// Residual fix (Porthole document links, and every other `shell_open` caller): a link holding a
    /// quote, a space and a control character is encoded at the sink; a path is not touched.
    #[test]
    fn shell_open_encodes_every_web_target() {
        let link = "mailto:a@b?x=\" /a C:\\secret.txt\u{7}";
        assert_eq!(shell_target(link), "mailto:a@b?x=%22%20/a%20C:%5Csecret.txt%07");
        assert_eq!(shell_target("https://a.b/p%20q"), "https://a.b/p%20q");
        assert_eq!(shell_target("C:\\My Docs\\a b.txt"), "C:\\My Docs\\a b.txt", "a path is not a URL");
        let sink = include_str!("app/semantic_ui.rs");
        let body = &sink[sink.find("pub(super) fn shell_open(target: &str, background: bool)").unwrap()..];
        assert!(body[..body.find("ShellExecuteW(std").unwrap()].contains("crate::semantic::shell_url(target)"), "shell_open encodes through shell_url (shell_target)");
        let porthole = include_str!("app/porthole_ui.rs");
        let f = &porthole[porthole.find("fn open_link_target").unwrap()..];
        assert!(f[..f.find("pub(super) fn porthole_link_bar").unwrap()].contains("shell_open("), "Porthole web links open through shell_open");
    }

    /// Judge round 5, item 3: the shell sink takes URLs only. A path, a drive path, a network path
    /// or a file: link is refused there (a file goes through `localfs::open_with_shell`, which
    /// takes a judged `LocalPath`), so no caller can hand ShellExecute an unjudged file.
    #[test]
    fn the_shell_sink_opens_only_urls() {
        assert_eq!(shell_url("https://a.b/x y").as_deref(), Some("https://a.b/x%20y"));
        assert_eq!(shell_url("mailto:a@b.c").as_deref(), Some("mailto:a@b.c"));
        assert_eq!(shell_url("ms-settings:defaultapps").as_deref(), Some("ms-settings:defaultapps"));
        for bad in [r"C:\x.txt", "c:/x.txt", r"Z:\share\x.pdf", r"\\host\s\x", "//host/s/x", r"\\?\C:\x", "file:///C:/x.txt", "FILE://host/s/x", "x.txt", r"..\x", ""] {
            assert_eq!(shell_url(bad), None, "{bad}");
        }
        // Round 5 item 3: only the schemes Termurion means to hand the shell. A shell namespace or any
        // other protocol handler (search-ms: makes Explorer connect to a share) is refused.
        for good in ["http://h/x", "HTTPS://h", "mailto:a@b.c", "ftp://h/f", "ssh://u@h", "telnet://h", crate::url_schemes::DEFAULT_APPS_URI] {
            assert!(shell_url(good).is_some(), "{good}");
        }
        for bad in ["search-ms:query=x&crumb=location:\\\\evil\\s", "shell:startup", "ms-msdt:/id x", "javascript:alert(1)", "vscode://file/c:/x", "ms-word:ofe|u|https://h/x", "itms://x", "res://x.dll/1", "mk:@MSITStore:x"] {
            assert_eq!(shell_url(bad), None, "{bad}");
        }
        let sink = include_str!("app/semantic_ui.rs");
        let body = &sink[sink.find("pub(super) fn shell_open(target: &str, background: bool)").unwrap()..];
        let body = &body[..body.find("ShellExecuteW(std").unwrap()];
        assert!(body.contains("crate::semantic::shell_url(target)"), "the sink refuses anything but a URL first");
        let porthole = include_str!("app/porthole_ui.rs");
        let f = &porthole[porthole.find("fn open_link_target").unwrap()..];
        let f = &f[..f.find("pub(super) fn porthole_link_bar").unwrap()];
        let judged = f.find("LocalPath::judge(").expect("a document's file link is judged");
        assert!(judged < f.find("reveal_in_explorer(").unwrap() && judged < f.find("open_with_shell(").unwrap(), "judged before Explorer or the shell sees it");
        assert!(!include_str!("porthole/mod.rs").contains("local_path_ok"), "the text check is no gate for document links");
        let ui = include_str!("app/semantic_ui.rs");
        let open = &ui[ui.find("Run::Open(target) => {").unwrap()..];
        let open = &open[..open.find("Run::Command(line)").unwrap()];
        assert!(open.contains("LocalPath::semantic(") && open.contains("open_with_shell("), "a Semantic History file opens as a judged LocalPath");
    }

    /// TWEAKS 6 review I4: the schemes Termurion owns (or does not offer) never reach the Windows handler.
    #[test]
    fn termurions_own_schemes_never_reach_the_windows_handler() {
        for u in ["telnet://h", "TELNET://h:23", "ssh://h", "x-man-page://1/ls", "iterm2:annotation?ann=A", "termurion://x"] {
            assert!(matches!(guard_open(u, false), Run::Blocked(_)), "{u}");
        }
        let Run::Blocked(why) = guard_open("telnet://h", false) else { panic!("refused") };
        assert!(why.contains("telnet"), "{why}");
        for s in crate::url_schemes::SCHEMES {
            assert!(crate::link_scheme::OWN.contains(&s), "{s}");
        }
    }

    #[test]
    fn an_unbound_ssh_link_says_how_to_open_it() {
        let Run::Blocked(why) = guard_open("ssh://host", false) else { panic!("refused") };
        assert!(why.contains("Termurion opens ssh: links when ssh is set to open in Termurion"), "{why}");
        let Run::Blocked(other) = guard_open("ms-msdt:x", false) else { panic!("refused") };
        assert!(other.contains("never opens ms-msdt: links"), "{other}");
    }

    use super::*;
    use crate::netpath::{Drives, Probe};
    use std::sync::Arc;

    #[test]
    fn explorer_runs_from_the_windows_folder() {
        // Final review Minor 4: by its full path, never Termurion's folder or PATH first.
        assert_eq!(explorer_in(Path::new(r"C:\Windows\System32")), Some(PathBuf::from(r"C:\Windows\explorer.exe")));
    }

    /// A fresh folder with `files` in it (a trailing slash makes a folder).
    fn dir(name: &str, files: &[&str]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("termurion-sem-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in files {
            let p = d.join(f);
            if f.ends_with('/') {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "x").unwrap();
            }
        }
        d
    }

    /// Splits `text` at the `|` that marks the click.
    fn click(text: &str) -> (String, String) {
        let (a, b) = text.split_once('|').unwrap();
        (a.to_string(), b.to_string())
    }

    #[test]
    fn finds_a_path_with_line_and_column() {
        let d = dir("linecol", &["src/main.rs"]);
        let (b, a) = click("error at src/ma|in.rs:12:5: oops");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("src/main.rs"), Some(12), Some(5))));
        let (b, a) = click("src\\ma|in.rs(7,3): error C2065");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("src\\main.rs"), Some(7), Some(3))));
        let (b, a) = click("src/main.r|s");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("src/main.rs"), None, None)));
    }

    #[test]
    fn finds_an_absolute_path_and_one_with_spaces() {
        let d = dir("abs", &["my file.txt"]);
        let full = d.join("my file.txt").to_string_lossy().to_string();
        let at = full.len() - 3;
        let (b, a) = (format!("see {}", &full[..at]), format!("{} now", &full[at..]));
        assert_eq!(find_path(&b, &a, None, &Probe::default()), Some((d.join("my file.txt"), None, None)));
    }

    #[test]
    fn finds_a_python_traceback_path() {
        let d = dir("py", &["x.py"]);
        let (b, a) = click("  File \"x|.py\", line 3, in f");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("x.py"), Some(3), None)));
    }

    #[test]
    fn strips_git_diff_prefixes() {
        let d = dir("git", &["lib/x.rs"]);
        let (b, a) = click("+++ b/lib/|x.rs");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("lib/x.rs"), None, None)));
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_path() {
        let d = dir("punct", &["notes.md", "docs/"]);
        let (b, a) = click("read no|tes.md.");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("notes.md"), None, None)));
        let (b, a) = click("see (not|es.md)!");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("notes.md"), None, None)));
        let (b, a) = click("in do|cs, see");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), Some((d.join("docs"), None, None)));
        assert_eq!(find_path("nothing he", "re", Some(&d), &Probe::default()), None);
    }

    #[test]
    fn url_like_text_becomes_a_url() {
        let (b, a) = click("go to https://exa|mple.com/a?b=1.");
        assert_eq!(url_like(&b, &a, true).as_deref(), Some("https://example.com/a?b=1"));
        let (b, a) = click("(see www.exam|ple.com/x)");
        assert_eq!(url_like(&b, &a, true).as_deref(), Some("https://www.example.com/x"));
        let (b, a) = click("mail mailto:a@|b.com");
        assert_eq!(url_like(&b, &a, true).as_deref(), Some("mailto:a@b.com"));
        let (b, a) = click("just a wo|rd");
        assert_eq!(url_like(&b, &a, true), None);
        let (b, a) = click("version 1.2|.3");
        assert_eq!(url_like(&b, &a, true), None);
    }

    /// R3, 02:839, merged onto 13r's guard (S1): a colon string is a URL only with one of the
    /// schemes 13r allows (B1), whatever the slash rule says; a bare host with a port or a line
    /// number is never guessed (13s's own port branch is gone with `url_in`).
    #[test]
    fn colon_strings_need_the_slash_too() {
        for slash in [true, false] {
            for text in ["see exam|ple.com:8080 now", "at main|.py:42", "x ex|ample.org:443", "see exam|ple.com:8080/x now"] {
                let (b, a) = click(text);
                assert_eq!(url_like(&b, &a, slash), None, "{text} {slash}");
            }
            let (b, a) = click("see https://exam|ple.com:8080/x now");
            assert_eq!(url_like(&b, &a, slash).as_deref(), Some("https://example.com:8080/x"));
        }
    }

    /// Merge seam (S1): 13r's `url_guess` is the one URL guess; semantic.rs keeps only thin
    /// wrappers (no second scanner, no second character set).
    #[test]
    fn the_url_guess_is_one_copy() {
        let code = include_str!("semantic.rs");
        let code = &code[..code.find("#[cfg(test)]\nmod tests").unwrap()];
        for gone in ["fn url_in(", "fn url_char(", "require_slash: bool,\n"] {
            assert!(!code.contains(gone), "{gone}");
        }
        assert!(code.contains("crate::url_guess::looks_like_url(s, require_slash, &crate::url_guess::CharSet::default())"));
        assert!(code.contains("crate::url_guess::guess(before, after, prefs)"));
    }

    #[test]
    fn looks_like_url_follows_iterm2() {
        for (s, want) in [("example.com", false), ("example.com/", true), ("foo/bar", true), ("/usr/bin", false), ("", false), ("a b", false)] {
            assert_eq!(looks_like_url(s, true), want, "require_slash true: {s:?}");
        }
        for (s, want) in [("example.com", true), ("10.0.0.1", true), ("256.1.1.1", true), ("localhost", false), ("-a.com", false), ("a b", false), ("/usr/bin", false), ("foo/bar", true)] {
            assert_eq!(looks_like_url(s, false), want, "require_slash false: {s:?}");
        }
    }

    #[test]
    fn url_guess_needs_a_slash() {
        let guess = |text: &str, slash: bool| {
            let (b, a) = click(text);
            url_like(&b, &a, slash)
        };
        assert_eq!(guess("exam|ple.com", true), None);
        assert_eq!(guess("exam|ple.com/x", true).as_deref(), Some("https://example.com/x"));
        assert_eq!(guess("exam|ple.com", false).as_deref(), Some("https://example.com"));
        assert_eq!(guess("go https://exa|mple.com", true).as_deref(), Some("https://example.com"));
    }

    #[test]
    fn path_uses_filename_parts() {
        let d = dir("fnparts", &["x.txt"]);
        assert_eq!(find_path("see <", "x.txt>", Some(&d), &Probe::default()).map(|(p, _, _)| p), Some(d.join("x.txt")));
        assert_eq!(find_path("a|b ", "x.txt", Some(&d), &Probe::default()).map(|(p, _, _)| p), Some(d.join("x.txt")));
        // The range stays in the coordinates of the text given.
        let (before, after) = ("see <x".to_string(), ".txt>".to_string());
        let ctx = ClickContext { before: before.clone(), after: after.clone(), cwd: Some(d.clone()), ..Default::default() };
        let (_, range) = resolve_range(&ctx).unwrap();
        assert_eq!(&format!("{before}{after}")[range.unwrap()], "x.txt");
    }

    #[test]
    fn second_pass_respects_hard_newlines() {
        let d = dir("respect", &["b.txt"]);
        // Two rows joined (a hard newline between "see a" and "b.txt"): no file. The logical
        // line alone holds b.txt.
        let ctx = ClickContext { before: "see a".into(), after: "b.txt".into(), cwd: Some(d.clone()), ..Default::default() };
        assert_eq!(resolve_in(&ctx), None);
        let ctx = ClickContext { respecting: Some((String::new(), "b.txt".into())), ..ctx };
        let (t, range, pass) = resolve_in(&ctx).unwrap();
        assert!(matches!(t, Target::File { .. }));
        assert_eq!((range, pass), (Some(0..5), Pass::Respecting));
        // The first pass wins when it finds something.
        let ctx = ClickContext { before: String::new(), after: "b.txt".into(), cwd: Some(d), respecting: Some((String::new(), "b.txt".into())), ..Default::default() };
        assert_eq!(resolve_in(&ctx).unwrap().2, Pass::Detect);
    }

    const UNSAFE_CHARS: &[char] = &['&', '|', '<', '>', '^', '"', '%', '!', '(', ')', '`', '\r', '\n', '\0', '\u{1b}', '\u{85}', '\u{ff02}', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}'];

    #[test]
    fn cmd_unsafe_is_the_context_free_refusal_list() {
        for c in UNSAFE_CHARS {
            assert!(cmd_unsafe(&format!("a{c}b")), "{c:?}");
        }
        assert!(cmd_unsafe("C:\\dir\\") && cmd_unsafe("x\\"), "a trailing backslash");
        assert!(!cmd_unsafe("C:\\dir\\a b-c_d.txt") && !cmd_unsafe("a\\b") && !cmd_unsafe(""));
    }

    #[test]
    fn a_pre_quoted_placeholder_cannot_carry_a_command() {
        let at = |prefix: &str| plan(&Target::RawCommand("w".into()), &Prefs { action: "raw command".into(), text: "echo \"\\3\"".into(), editor: String::new() }, &Around { prefix, suffix: "", cwd: "" }, false);
        assert_eq!(at("x&calc"), Some(Run::Blocked(CMD_REFUSED.into())));
        assert_eq!(at("x\u{ff02}&calc"), Some(Run::Blocked(CMD_REFUSED.into())));
        assert_eq!(at("x\\"), Some(Run::Blocked(CMD_REFUSED.into())));
        // A safe value still runs: the exact line cmd.exe is created with.
        let Some(Run::Command(line)) = plan(&Target::RawCommand("w".into()), &Prefs { action: "raw command".into(), text: "echo \\3".into(), editor: String::new() }, &Around { prefix: "abc", suffix: "", cwd: "" }, false) else { panic!() };
        assert_eq!(cmd_command_line(&line), "cmd.exe /d /v:off /s /c \"echo \"abc\"\"");
    }

    /// A placeholder the template already quotes gets the value bare (no second pair of quotes),
    /// so a value with spaces stays one argument for the program cmd.exe starts; an unquoted
    /// placeholder gets its own quotes. The exact CreateProcess line and its argv.
    /// Final wave ruling: a working folder at a drive root (`\5`) reaches the builder as `X:\.`.
    #[test]
    fn a_drive_root_reaches_the_cmd_builder_in_its_safe_form() {
        assert_eq!(fill_cmd("dir \\5", ["", "", "", "", "C:\\"]).as_deref(), Some("dir \"C:\\.\""));
        assert_eq!(fill_cmd("dir \"\\5\"", ["", "", "", "", "d:/"]).as_deref(), Some("dir \"d:\\.\""));
        assert_eq!(fill_cmd("dir \\5", ["", "", "", "", "C:\\a\\"]), None, "a folder with a trailing backslash stays refused");
    }

    #[test]
    fn a_pre_quoted_placeholder_stays_one_argument() {
        let v = "a --output=C:\\p";
        let line = fill_cmd("git log \"\\1\"", [v, "", "", "", ""]).unwrap();
        assert_eq!(cmd_command_line(&line), "cmd.exe /d /v:off /s /c \"git log \"a --output=C:\\p\"\"");
        assert_eq!(split_command_line(&line), ["git", "log", v]);
        assert_eq!(fill_cmd("git log \\1", [v, "", "", "", ""]).as_deref(), Some(line.as_str()), "unquoted: quoted by the builder");
        let mixed = fill_cmd("p \"--x=\\1\" \\1 \"\\2\"", [v, "", "", "", ""]).unwrap();
        assert_eq!(mixed, "p \"--x=a --output=C:\\p\" \"a --output=C:\\p\" \"\"");
        assert_eq!(split_command_line(&mixed), ["p", "--x=a --output=C:\\p", v, ""]);
        assert_eq!(fill_cmd("git log \"\\1\"", ["x&calc", "", "", "", ""]), None, "the refusal holds inside the template's quotes");
    }

    /// The one builder: the person's text as written; each value refused by the context-free
    /// list, else bare inside the template's own quotes and quoted outside them.
    #[test]
    fn the_one_cmd_builder_follows_the_template_quotes() {
        use CmdPiece::{Text, Value};
        let t = |s: &str| Text(s.into());
        let v = |s: &str| Value(s.into());
        assert_eq!(build_cmd_line(&[t("run "), v("a b")]).as_deref(), Some("run \"a b\""));
        assert_eq!(build_cmd_line(&[t("run \""), v("a b"), t("\" "), v("c")]).as_deref(), Some("run \"a b\" \"c\""));
        assert_eq!(build_cmd_line(&[t("run \"x\" "), v("")]).as_deref(), Some("run \"x\" \"\""), "an even count of quotes is closed");
        assert_eq!(build_cmd_line(&[t("echo %PATH% & "), v("ok")]).as_deref(), Some("echo %PATH% & \"ok\""), "the template's own text stays");
        for bad in ["a&b", "a\"b", "a\\", "a\nb", "a\u{ff02}"] {
            assert_eq!(build_cmd_line(&[t("run \""), v(bad), t("\"")]), None, "{bad:?}");
            assert_eq!(build_cmd_line(&[t("run "), v(bad)]), None, "{bad:?}");
        }
    }

    /// Every Explorer reveal's argument comes from one builder: the path in quotes, and refused
    /// when it holds a quote (it would end the argument and add Explorer switches such as
    /// `/root,\\host\share`), a control character, or is a network path.
    #[test]
    fn explorer_select_is_one_argument() {
        assert_eq!(explorer_select_arg(r"C:\a b\x.exe").as_deref(), Some(r#"/select,"C:\a b\x.exe""#));
        for bad in [r#"C:\x" /root,\\evil\s\x.exe"#, "C:\\a\nb.exe", r"\\evil\s\x.exe", r"//evil/s/x.exe"] {
            assert_eq!(explorer_select_arg(bad), None, "{bad:?}");
        }
    }

    /// Programs that pass their arguments on to a shell, a console host or another program are
    /// never URL handlers, whatever the URL, by resolved file name (any folder, any extension).
    /// Every session variable in a handler template is exactly one argument (or part of one),
    /// never the program, and passes the context-free list and the program's syntax rule.
    #[test]
    fn handler_variables_are_one_argument_each() {
        let v = |vals: &[&str]| vals.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (t0, t1) = (var_token(0), var_token(1));
        let url = "https://a/b";
        assert_eq!(handler_plan(&format!("open.exe {t0} {URL_TOKEN}"), url, &v(&["My Title  with spaces"]), &fake_resolve), spawn_of(r"C:\t\open.exe", &["My Title  with spaces", url]));
        assert_eq!(handler_plan(&format!("open.exe --t={t0} --p={t1}"), url, &v(&["a b", "C:\\x y"]), &fake_resolve), spawn_of(r"C:\t\open.exe", &["--t=a b", "--p=C:\\x y", url]));
        for program in [format!("{t0}"), format!("{t0}\\open.exe"), format!("\"{t0}\"")] {
            assert_eq!(handler_plan(&format!("{program} {URL_TOKEN}"), url, &v(&["C:\\t"]), &fake_resolve), Err(Refusal::VariableInProgram), "{program}");
        }
        for bad in ["a&calc", "a\"b", "x\n", "a\\", "a\u{E000}b"] {
            assert_eq!(handler_plan(&format!("open.exe {t0}"), url, &v(&[bad]), &fake_resolve), Err(Refusal::VariableUnsafe), "{bad:?}");
        }
        assert_eq!(handler_plan(&format!("pwsh -File h.ps1 {t0}"), url, &v(&["a;b"]), &fake_resolve), Err(Refusal::VariableUnsafe), "an interpreter's syntax");
    }

    #[test]
    fn handler_forwarders_are_refused() {
        for program in [
            r"C:\Windows\System32\conhost.exe",
            r"C:\Windows\System32\forfiles.exe",
            r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\WT.EXE",
            r"C:\Program Files\Git\usr\bin\sh.exe",
            r"C:\msys64\usr\bin\zsh.exe",
            r"C:\Windows\System32\rundll32.exe",
            r"C:\Windows\SysWOW64\regsvr32.exe",
            r"C:\Windows\System32\msiexec.exe",
            r"C:\Windows\System32\certutil.exe",
            r"C:\Windows\System32\cmd.exe",
            r"C:\tools\Cmd.com",
        ] {
            assert_eq!(handler_plan(&format!(r#""{program}" {URL_TOKEN}"#), "https://a/b", &[], &fake_resolve), Err(Refusal::Forwarder), "{program}");
        }
        assert_eq!(handler_plan(&format!("cmd.exe /c start \"\" {URL_TOKEN}"), "https://a/b", &[], &fake_resolve), Err(Refusal::Forwarder));
        assert_eq!(handler_plan(&format!(r#""C:\t\shell.exe" {URL_TOKEN}"#), "https://a/b", &[], &fake_resolve), spawn_of(r"C:\t\shell.exe", &["https://a/b"]), "a stem is matched whole");
    }

    #[test]
    fn prompt_phase_follows_iterm2() {
        let (before, after) = click("ab|cdefgh");
        let on = ClickContext { before, after, link: Some("https://a.b/".into()), prompt: Some((7, 2..8)), ..Default::default() };
        // Before any link.
        assert_eq!(resolve_range(&on), Some((Target::CommandInfo(7), Some(2..8))));
        // The first and last bytes of the range are in it, the byte after is not.
        for (at, hit) in [(2, true), (7, true), (1, false), (8, false), (9, false)] {
            let (before, after) = "abcdefghij".split_at(at);
            let ctx = ClickContext { before: before.into(), after: after.into(), link: Some("https://a.b/".into()), prompt: Some((7, 2..8)), ..Default::default() };
            let got = resolve(&ctx);
            assert_eq!(got == Some(Target::CommandInfo(7)), hit, "offset {at}");
            // Outside the command the next phases answer: here the link.
            assert!(got.is_some());
        }
        // The key off: nothing changes.
        assert_eq!(resolve(&ClickContext { prompt: None, ..on.clone() }), Some(Target::Link("https://a.b/".into())));
        // With Smart Selection first the prompt still goes first (UAF:186-232: Prompt leads).
        assert_eq!(resolve(&ClickContext { order: Order::SmartFirst, ..on.clone() }), Some(Target::CommandInfo(7)));
        // A prompt click plans nothing to run: the app shows the window.
        let prefs = Prefs::default();
        assert_eq!(plan(&Target::CommandInfo(7), &prefs, &Around::default(), false), None);
    }

    #[test]
    fn phase_order_follows_iterm2() {
        use Phase::*;
        assert_eq!(phases(Order::Default), &[Prompt, Link, File, SmartAction, RawCommand, UrlLike]);
        assert_eq!(phases(Order::SmartFirst), &[Prompt, Link, SmartAction, File, RawCommand, UrlLike]);
        assert_eq!(phases(Order::NoSmart), &[Prompt, Link, File, RawCommand, UrlLike]);
        assert_eq!(order_from(true, true), Order::NoSmart);
        assert_eq!(order_from(false, true), Order::NoSmart);
        assert_eq!(order_from(true, false), Order::SmartFirst);
        assert_eq!(order_from(false, false), Order::Default);
        // A file and an action: the action first only with SmartFirst.
        let d = dir("order5", &["example.com"]);
        let (before, after) = click("exam|ple.com");
        let mut ctx = ClickContext { before, after, cwd: Some(d), ..Default::default() };
        ctx.smart = Some((1, 0, "example.com".into()));
        assert!(matches!(resolve(&ctx), Some(Target::File { .. })));
        ctx.order = Order::SmartFirst;
        assert_eq!(resolve(&ctx), Some(Target::Action { rule: 1, action: 0, text: "example.com".into() }));
        // An action and URL-like text: NoSmart gives the URL.
        let (before, after) = click("see http://exa|mple.com/x");
        let mut ctx = ClickContext { before, after, ..Default::default() };
        ctx.smart = Some((1, 0, "http://example.com/x".into()));
        assert!(matches!(resolve(&ctx), Some(Target::Action { .. })));
        ctx.order = Order::NoSmart;
        assert_eq!(resolve(&ctx), Some(Target::Url("http://example.com/x".into())));
    }

    #[test]
    fn inactive_win_click_follows_iterm2() {
        assert!(win_click_acts(false, true, false));
        assert!(!win_click_acts(true, false, false));
        assert!(!win_click_acts(true, false, true));
        assert!(!win_click_acts(true, true, false));
        assert!(win_click_acts(true, true, true));
        assert!(hover_allowed(true, false));
        assert!(!hover_allowed(false, false));
        assert!(hover_allowed(false, true));
    }

    #[test]
    fn resolution_follows_iterm2s_order() {
        let d = dir("order", &["example.com"]);
        let (before, after) = click("exam|ple.com");
        let mut ctx = ClickContext { before, after, cwd: Some(d.clone()), ..Default::default() };
        ctx.link = Some("https://linked/".into());
        ctx.smart = Some((1, 0, "example.com".into()));
        ctx.raw_command = true;
        assert_eq!(resolve(&ctx), Some(Target::Link("https://linked/".into())));
        ctx.link = None;
        assert!(matches!(resolve(&ctx), Some(Target::File { .. })));
        ctx.cwd = None;
        assert_eq!(resolve(&ctx), Some(Target::Action { rule: 1, action: 0, text: "example.com".into() }));
        ctx.smart = None;
        assert_eq!(resolve(&ctx), Some(Target::RawCommand("example.com".into())));
        ctx.raw_command = false;
        // A bare two-label name is a URL only with a slash, unless the slash rule is off.
        assert_eq!(resolve(&ctx), None);
        ctx.urls.require_slash = false;
        assert_eq!(resolve(&ctx), Some(Target::Url("https://example.com".into())));
        ctx.before = "plain ".into();
        ctx.after = "".into();
        assert_eq!(resolve(&ctx), None);
    }

    #[test]
    fn semantic_history_command_quotes_for_cmd() {
        let file = Target::File { path: PathBuf::from("C:\\a b\\x.rs"), line: Some(3), column: None };
        let around = Around { prefix: "at ", suffix: " now", cwd: "C:\\w" };
        let prefs = Prefs { action: "command".into(), text: "vim \\1 +\\2".into(), editor: String::new() };
        assert_eq!(plan(&file, &prefs, &around, false), Some(Run::Command("vim \"C:\\a b\\x.rs\" +\"3\"".into())));
        let raw = Prefs { action: "raw command".into(), text: "echo \\1 in \\5".into(), editor: String::new() };
        assert_eq!(plan(&Target::RawCommand("w x".into()), &raw, &around, false), Some(Run::Command("echo \"w x\" in \"C:\\w\"".into())));
        assert!(matches!(plan(&Target::RawCommand("w\"x".into()), &raw, &around, false), Some(Run::Blocked(_))), "a quote is refused (B3)");
        let best = Prefs::from_value(None);
        assert_eq!(best.action, "best editor");
        assert_eq!(plan(&file, &best, &around, false), Some(Run::Open("C:\\a b\\x.rs".into())));
        assert_eq!(plan(&file, &best, &around, true), Some(Run::Spawn { program: PathBuf::from("code"), args: vec!["-g".into(), "C:\\a b\\x.rs:3".into()] }));
        let editor = Prefs { action: "editor".into(), text: String::new(), editor: "C:\\ed\\ed.exe".into() };
        assert_eq!(plan(&file, &editor, &around, false), Some(Run::Spawn { program: PathBuf::from("C:\\ed\\ed.exe"), args: vec!["C:\\a b\\x.rs".into()] }));
        let send = Prefs { action: "send text".into(), text: "open \\1\\n".into(), editor: String::new() };
        assert_eq!(plan(&file, &send, &around, false), Some(Run::Send("open C:\\a b\\x.rs\\n".into())));
        let co = Prefs { action: "coprocess".into(), text: "x".into(), editor: String::new() };
        assert_eq!(plan(&file, &co, &around, false), Some(Run::Coprocess("x".into())));
        assert_eq!(plan(&Target::Url("https://a/".into()), &co, &around, false), Some(Run::Open("https://a/".into())));
        assert_eq!(plan(&Target::Link("file:///c:/x".into()), &best, &around, false), Some(Run::OpenLocal { path: "c:\\x".into(), has_code: false }));
    }

    #[test]
    fn url_action_percent_encodes() {
        let file = Target::File { path: PathBuf::from("C:\\a b\\x.rs"), line: Some(3), column: Some(1) };
        let prefs = Prefs { action: "url".into(), text: "https://x/open?f=\\1&l=\\2".into(), editor: String::new() };
        assert_eq!(
            plan(&file, &prefs, &Around::default(), false),
            Some(Run::Open("https://x/open?f=C%3A%5Ca%20b%5Cx.rs&l=3".into()))
        );
        assert_eq!(percent_encode("a-._~/é"), "a-._~%2F%C3%A9");
    }

    #[test]
    fn hover_ranges_cover_the_target() {
        let d = dir("range", &["src/main.rs"]);
        let (before, after) = click("at src/ma|in.rs:3 and https://a.b/c. x");
        let ctx = ClickContext { before: before.clone(), after: after.clone(), cwd: Some(d), ..Default::default() };
        let text = format!("{before}{after}");
        let (t, r) = resolve_range(&ctx).unwrap();
        assert!(matches!(t, Target::File { line: Some(3), .. }));
        assert_eq!(&text[r.unwrap()], "src/main.rs");
        let (before, after) = click("at src/main.rs:3 and https://a.|b/c. x");
        let ctx = ClickContext { before: before.clone(), after: after.clone(), ..Default::default() };
        let text = format!("{before}{after}");
        let (t, r) = resolve_range(&ctx).unwrap();
        assert_eq!(t, Target::Url("https://a.b/c".into()));
        assert_eq!(&text[r.unwrap()], "https://a.b/c");
        let (before, after) = click("(www.ex|ample.com/x)");
        let (_, r) = resolve_range(&ClickContext { before: before.clone(), after: after.clone(), ..Default::default() }).unwrap();
        assert_eq!(&format!("{before}{after}")[r.unwrap()], "www.example.com/x");
    }

    #[test]
    fn unc_paths_are_never_probed() {
        // A dead address: probing it as a share takes about 21 s on Windows.
        let t = std::time::Instant::now();
        assert_eq!(find_path("see ", "\\\\10.255.255.1\\x\\y", None, &Probe::default()), None);
        assert_eq!(find_path("go ", "//10.255.255.1/x", None, &Probe::default()), None);
        let d = dir("unc", &[]);
        let (b, a) = click("at https://10.255.255.1/x|/y now");
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), None);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    /// Every drive is a mapped network drive.
    struct AllRemote;
    impl Drives for AllRemote {
        fn dos_device(&self, letter: char) -> Option<String> {
            Some(format!(r"\Device\LanmanRedirector\;{letter}:0000000000012345\srv\share"))
        }
        fn drive_type(&self, _letter: char) -> u32 {
            4
        }
    }

    #[test]
    fn ignored_paths_are_not_found() {
        let d = dir("ignored", &["x.txt"]);
        let here = d.to_string_lossy().to_string();
        assert_eq!(find_path("", "x.txt", Some(&d), &Probe::default()).map(|(p, _, _)| p), Some(d.join("x.txt")));
        // Under an ignored folder: the working folder and an absolute path alike.
        let ignoring = Probe::from_settings(&here, false, Arc::new(crate::netpath::LocalDrives));
        assert_eq!(find_path("", "x.txt", Some(&d), &ignoring), None);
        assert_eq!(find_path("", &d.join("x.txt").to_string_lossy(), None, &ignoring), None);
        // A mapped drive is not probed unless network mounts are allowed.
        let remote = Probe { drives: Arc::new(AllRemote), ..Probe::default() };
        assert_eq!(find_path("", "x.txt", Some(&d), &remote), None);
        let allowed = Probe { allow_network: true, ..remote };
        assert_eq!(find_path("", "x.txt", Some(&d), &allowed).map(|(p, _, _)| p), Some(d.join("x.txt")));
        // The click resolves through the context's probe too.
        let ctx = ClickContext { before: String::new(), after: "x.txt".into(), cwd: Some(d.clone()), probe: ignoring, ..Default::default() };
        assert!(!matches!(resolve(&ctx), Some(Target::File { .. })));
    }

    #[test]
    fn a_click_probes_at_most_120_files() {
        let d = dir("budget", &[]);
        let words: Vec<String> = (0..60).map(|i| format!("a/w{i}")).collect();
        let (b, a) = (words[..30].join(" ") + " ", words[30..].join(" "));
        PROBES.with(|n| n.set(0));
        assert_eq!(find_path(&b, &a, Some(&d), &Probe::default()), None);
        let probed = PROBES.with(|n| n.get());
        assert!(probed > 100 && probed <= 120, "{probed}: exists calls and link reads together");
    }

    #[test]
    fn executables_are_shown_not_run() {
        let best = Prefs::from_value(None);
        for name in ["setup.py", "build.js", "x.EXE", "run.bat", "a.lnk", "b.ps1"] {
            let file = Target::File { path: PathBuf::from(format!("C:\\w\\{name}")), line: None, column: None };
            assert_eq!(plan(&file, &best, &Around::default(), false), Some(Run::Reveal(format!("C:\\w\\{name}"))), "{name}");
        }
        let script = Target::File { path: PathBuf::from("C:\\w\\setup.py"), line: Some(2), column: None };
        assert_eq!(plan(&script, &best, &Around::default(), true), Some(Run::Spawn { program: PathBuf::from("code"), args: vec!["-g".into(), "C:\\w\\setup.py:2".into()] }));
        let text = Target::File { path: PathBuf::from("C:\\w\\notes.txt"), line: None, column: None };
        assert_eq!(plan(&text, &best, &Around::default(), false), Some(Run::Open("C:\\w\\notes.txt".into())));
    }

    #[test]
    fn links_open_only_safe_schemes() {
        assert_eq!(guard_open("https://docs.rs/x", false), Run::Open("https://docs.rs/x".into()));
        assert_eq!(guard_open("mailto:a@b.c", false), Run::Open("mailto:a@b.c".into()));
        assert!(matches!(guard_open("ms-msdt:/id x", false), Run::Blocked(_)));
        assert!(matches!(guard_open("search-ms:query=x", false), Run::Blocked(_)));
        let settle = |r: Run| match r {
            Run::OpenLocal { path, has_code } => open_local_result(&path, has_code, &crate::netpath::Probe::default(), &|_| true),
            other => other,
        };
        assert_eq!(settle(guard_open("file:///C:/Users/u/Downloads/x.exe", false)), Run::Reveal("C:\\Users\\u\\Downloads\\x.exe".into()));
        assert_eq!(settle(guard_open("file:///C:/docs/a%20b.txt", false)), Run::Open("C:\\docs\\a b.txt".into()));
        assert!(matches!(guard_open("C:\\tools\\x.bat", false), Run::Blocked(_)));
        let best = Prefs::from_value(None);
        let link = Target::Link("file:///C:/x/evil.cmd".into());
        assert_eq!(plan(&link, &best, &Around::default(), false).map(settle), Some(Run::Reveal("C:\\x\\evil.cmd".into())));
        assert!(matches!(plan(&Target::Url("javascript://x".into()), &best, &Around::default(), false), Some(Run::Blocked(_))));
    }
    #[test]
    fn drive_paths_never_open_as_urls() {
        // B1: a colon string is no URL unless its scheme is one guard_open passes on, and
        // guard_open refuses what is not a link.
        let best = Prefs::from_value(None);
        let plan_of = |t: &str| plan(&Target::Url(t.into()), &best, &Around::default(), false);
        assert!(matches!(plan_of(r"C:\missing\x.txt"), Some(Run::Blocked(_))));
        assert!(matches!(plan_of("notes.txt"), Some(Run::Blocked(_))));
        let (b, a) = click(r"see C:\missing\|x.txt now");
        assert_eq!(url_like(&b, &a, true), None);
    }

    /// TWEAKS 6: another app's scheme is a request to open (the App asks once per scheme); the
    /// always-blocked ones and text that is no URL stay refused.
    #[test]
    fn other_schemes_become_open_scheme_unless_blocked() {
        assert_eq!(guard_open("vscode://file/C:/x.rs:3", false), Run::OpenScheme("vscode://file/C:/x.rs:3".into()));
        assert_eq!(guard_open("Slack://open", false), Run::OpenScheme("Slack://open".into()));
        for t in ["ms-msdt:/id x", "search-ms:query=a", "javascript:alert(1)", "shell:startup", "ms-word:ofe|u|https://h/x.docx", "vscode:a\nb"] {
            assert!(matches!(guard_open(t, false), Run::Blocked(_)), "{t}");
        }
        assert!(matches!(guard_open("ssh://h", false), Run::Blocked(_)));
        assert_eq!(guard_open("https://a.b/x", false), Run::Open("https://a.b/x".into()));
    }

    #[test]
    fn link_without_scheme_is_blocked() {
        let best = Prefs::from_value(None);
        for t in [r"\\evil\s\x", "//evil/s/x", r"/\evil/s", r"\??\UNC\evil\s", r"\\?\UNC\evil\s\x", r"C:\x\y.txt", "relative/x.txt"] {
            assert!(matches!(guard_open(t, false), Run::Blocked(_)), "{t}");
            assert!(matches!(plan(&Target::Link(t.into()), &best, &Around::default(), false), Some(Run::Blocked(_))), "{t}");
        }
        assert!(matches!(guard_open("file://evil/s/x", false), Run::Blocked(_)));
    }

    /// R13x23 fix pass item 3 (the terminal half): every network target is refused by the one gate
    /// first, whatever its spelling, and the local long-path form is not one.
    #[test]
    fn guard_open_passes_the_gate() {
        for t in ["file:////host/share/x", "file://host/share/x", "file:///%5C%5Chost%5Cshare%5Cx", r"\\host\share\x", "//host/share/x", r"\\?\UNC\host\share\x", "  //host/share/x"] {
            assert!(matches!(guard_open(t, false), Run::Blocked(_)), "{t}");
            assert_eq!(guard_open(t, false), Run::Blocked(crate::browser_js::gate_refusal(t).unwrap()), "{t}: the gate's own text");
        }
        assert!(crate::browser_js::gate_refusal(r"\\?\D:\s").is_none());
        assert!(matches!(guard_open("file:///C:/a/b.txt", false), Run::OpenLocal { .. }));
    }

    #[test]
    fn open_file_action_never_probes_a_network_path() {
        assert!(matches!(open_file_run(r"\\evil\s\a.txt", None, false), Run::Blocked(_)));
        assert!(matches!(open_file_run("//evil/s/a.txt", None, false), Run::Blocked(_)));
        assert!(matches!(open_url_run("ms-msdt:/id x", false), Run::Blocked(_)));
        assert!(matches!(open_url_run(r"\\evil\s\a.exe", false), Run::Blocked(_)));
        assert_eq!(open_url_run("https://a.b/x", false), Run::Open("https://a.b/x".into()));
    }

    /// Final wave item 5: openFile: and file: links obey the mapped-drive and ignored-folder rule,
    /// and the existence test is not made where the link is planned (the UI thread).
    #[test]
    fn local_opens_obey_the_probe_rule_and_never_probe_when_planned() {
        let probe = crate::netpath::Probe::from_settings("D:\\slow", false, std::sync::Arc::new(crate::netpath::MappedLetters(&['Z'])));
        let asked = std::cell::Cell::new(0);
        let exists = |_: &str| {
            asked.set(asked.get() + 1);
            true
        };
        for p in ["Z:\\x\\a.txt", "z:/x/a.txt", "\\\\?\\Z:\\x\\a.txt", "D:\\slow\\a.txt"] {
            assert!(matches!(open_local_result(p, false, &probe, &exists), Run::Blocked(_)), "{p}");
        }
        assert_eq!(asked.get(), 0, "nothing was probed on a mapped drive or an ignored folder");
        let allowed = crate::netpath::Probe::from_settings("D:\\slow", true, std::sync::Arc::new(crate::netpath::MappedLetters(&['Z'])));
        assert_eq!(open_local_result("Z:\\x\\a.txt", false, &allowed, &exists), Run::Open("Z:\\x\\a.txt".into()), "EnableSemanticHistoryOnNetworkMounts allows it");
        assert_eq!(asked.get(), 1);
        // Planning touches no disk: a link and an openFile: give the marker and ask nothing.
        assert!(matches!(guard_open("file:///Z:/x/a.txt", false), Run::OpenLocal { .. }));
        assert!(matches!(open_file_run("Z:\\x\\a.txt", None, false), Run::OpenLocal { .. }));
        assert!(matches!(open_file_run("C:\\definitely\\missing\\a.txt", None, false), Run::OpenLocal { .. }), "existence is the worker's question");
        assert!(matches!(guard_open("file://evil/s/x", false), Run::Blocked(_)));
    }

    #[test]
    fn open_file_is_never_a_protocol_open() {
        let d = dir("openfile", &["a b.txt", "run.exe"]);
        for text in ["ms-msdt:/id PCWDiagnostic", "search-ms:query=x", "https://a/x.exe", "ms-settings:network", "file:///C:/x", "javascript:alert(1)"] {
            assert!(matches!(open_file_run(text, Some(&d), true), Run::Blocked(_)), "{text}");
        }
        let full = d.join("a b.txt");
        let local = |t: String| Run::OpenLocal { path: t, has_code: false };
        assert_eq!(open_file_run(&full.to_string_lossy(), None, false), local(full.to_string_lossy().to_string()));
        assert_eq!(open_file_run("a b.txt", Some(&d), true), local(full.to_string_lossy().to_string()), "relative to the pane folder; never code -g");
        assert!(matches!(open_file_run("a b.txt", None, false), Run::Blocked(_)), "no folder to resolve against");
        let exe = d.join("run.exe");
        let probe = crate::netpath::Probe::default();
        let on_disk = |t: &str| Path::new(t).exists();
        assert_eq!(open_local_result(&exe.to_string_lossy(), false, &probe, &on_disk), Run::Reveal(exe.to_string_lossy().to_string()));
        assert_eq!(open_local_result(&full.to_string_lossy(), false, &probe, &on_disk), Run::Open(full.to_string_lossy().to_string()));
        assert!(matches!(open_local_result(&d.join("missing.txt").to_string_lossy(), false, &probe, &on_disk), Run::Blocked(_)));
        assert!(matches!(open_file_run("a\u{1b}b.txt", Some(&d), false), Run::Blocked(_)));
        let pct = r"C:\t\a%CMDCMDLINE%&calc.exe";
        assert!(!matches!(open_file_run(pct, None, true), Run::Command(_) | Run::Open(_)));
    }

    #[test]
    fn percent_never_reaches_cmd() {
        assert!(cmd_unsafe("a%CMDCMDLINE%b") && cmd_unsafe("a\"b") && cmd_unsafe("a\nb") && cmd_unsafe("a\u{1b}"));
        assert!(cmd_unsafe(r"C:\a&b\x.txt"), "refused whether or not the template quotes it");
        let evil = Target::File { path: PathBuf::from(r"C:\t\a%CMDCMDLINE%&calc.txt"), line: None, column: None };
        let cmd = Prefs { action: "command".into(), text: "vim \\1".into(), editor: String::new() };
        assert!(matches!(plan(&evil, &cmd, &Around::default(), false), Some(Run::Blocked(_))), "\\1");
        let ok = Target::File { path: PathBuf::from(r"C:\t\x.txt"), line: None, column: None };
        let around = Around { prefix: "x%CMDCMDLINE%&calc", suffix: "", cwd: "" };
        let three = Prefs { action: "command".into(), text: "echo \\3".into(), editor: String::new() };
        assert!(matches!(plan(&ok, &three, &around, false), Some(Run::Blocked(_))), "\\3");
        let co = Prefs { action: "coprocess".into(), text: "echo \\4".into(), editor: String::new() };
        let around = Around { prefix: "", suffix: "\"&calc", cwd: "" };
        assert!(matches!(plan(&ok, &co, &around, false), Some(Run::Blocked(_))), "\\4 with a quote");
        let raw = Prefs { action: "raw command".into(), text: "echo \\1".into(), editor: String::new() };
        assert!(matches!(plan(&Target::RawCommand("%COMSPEC%".into()), &raw, &Around::default(), false), Some(Run::Blocked(_))));
        // The person's own template text is theirs: a % in it is not a substituted value.
        let own = Prefs { action: "command".into(), text: "echo %USERNAME% \\1".into(), editor: String::new() };
        assert!(matches!(plan(&ok, &own, &Around::default(), false), Some(Run::Command(_))));
    }

    #[test]
    fn command_line_never_lets_a_value_add_a_command() {
        // The exact arguments: cmd strips only the outer quotes added here.
        assert_eq!(cmd_args(r#""x.txt" y"#), r#"/d /v:off /s /c ""x.txt" y""#);
        assert_eq!(cmd_command_line("dir"), r#"cmd.exe /d /v:off /s /c "dir""#);
        // A template that starts with a substituted value: its first quote must survive.
        assert_eq!(fill_cmd("\\1", ["foo&cd.>pwned.txt", "", "", "", ""]), None, "refused before it can reach cmd.exe");
        // Defence in depth: even if a value got through, its quotes survive /s.
        let line = cmd_quote("foo&cd.>pwned.txt");
        assert_eq!(cmd_args(&line), r##"/d /v:off /s /c ""foo&cd.>pwned.txt"""##);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let d = dir("inject", &[]);
            std::fs::create_dir_all(&d).unwrap();
            let out = std::process::Command::new("cmd.exe").raw_arg(cmd_args(&line)).current_dir(&d).output().unwrap();
            assert!(!d.join("pwned.txt").exists(), "a second command ran: {out:?}");
        }
    }

    /// The type is the text after the last dot of the final name, a leading dot included, trailing
    /// dots and spaces ignored, as Windows reads it (review I2); the list covers the Windows
    /// high-risk types.
    #[test]
    fn runnable_reads_the_type_as_windows_does() {
        for bad in [".exe", r"C:\x\.hta", "x.exe.", "x.cmd ", ".LNK", "x.psd1", "x.ps1xml", "x.xll", "x.hlp", "x.mst", "x.search-ms", "x.wsc", "x.ins", "x.isp", "x.jnlp"] {
            assert!(runnable(bad), "{bad:?}");
        }
        for fine in [".bashrc", "x.exe_", "x.exe.download", r"C:\x.exe\notes", "notes.md", "a.txt"] {
            assert!(!runnable(fine), "{fine:?}");
        }
    }

    /// The one normalizer (findings r2 N1): a stream suffix, trailing dots and spaces, a `\\?\`
    /// prefix and a drive-relative path are read as Windows opens them, and a stream `:` past the
    /// drive is dangerous whatever the base type (fail closed: revealed, never opened).
    #[test]
    fn a_stream_or_alias_form_never_hides_a_program() {
        for bad in [r"C:\x\a.exe::$DATA", "a.exe:stream", "a.EXE . .", r"\\?\C:\a.exe", r"\\.\C:\a.exe", "x.LNK:$DATA", "C:a.exe", r"C:\x\a.ps1::$DATA", "a.exe:", r"C:\x\a.txt:s", r"C:\d:s\x.txt", "a.p\u{17f}1"] {
            assert!(runnable(bad), "{bad:?}");
            assert!(crate::inert_files::high_risk(bad), "{bad:?}");
        }
        for fine in [r"C:\x\a.txt", r"\\?\C:\a.txt", "C:a.txt", r"C:\x\notes.md"] {
            assert!(!runnable(fine), "{fine:?}");
        }
        // Every open-or-reveal decision of a file link follows it.
        let settle = |r: Run| match r {
            Run::OpenLocal { path, has_code } => open_local_result(&path, has_code, &crate::netpath::Probe::default(), &|_| true),
            other => other,
        };
        assert_eq!(settle(guard_open("file:///C:/x/a.exe::$DATA", false)), Run::Reveal("C:\\x\\a.exe::$DATA".into()));
        assert_eq!(settle(open_file_run("C:\\x\\a.exe:s", None, false)), Run::Reveal("C:\\x\\a.exe:s".into()));
    }

    #[test]
    fn resolve_on_path_prefers_a_runnable_extension() {
        // VS Code's bin folder holds `code` (a shell script) beside `code.cmd`.
        let d = dir("resolve", &["code", "code.cmd", "tool.exe", "plain", "x.bat"]);
        let path = std::env::join_paths([&d]).unwrap();
        assert_eq!(resolve_on_path(Path::new("code"), Some(&path)), Some(d.join("code.cmd")));
        assert_eq!(resolve_on_path(Path::new("tool"), Some(&path)), Some(d.join("tool.exe")));
        assert_eq!(resolve_on_path(Path::new("x"), Some(&path)), Some(d.join("x.bat")));
        // An extensionless file alone is no program; a name with its own extension is taken as is.
        assert_eq!(resolve_on_path(Path::new("plain"), Some(&path)), None);
        assert_eq!(resolve_on_path(Path::new("code.cmd"), Some(&path)), Some(d.join("code.cmd")));
        // A PATH entry that is a share is never looked in.
        let shared = std::env::join_paths([PathBuf::from(r"\\10.255.255.1\bin"), d.clone()]).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(resolve_on_path(Path::new("tool"), Some(&shared)), Some(d.join("tool.exe")));
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn editor_spawns_without_a_shell() {
        let file = Target::File { path: PathBuf::from(r"C:\a b\x.rs"), line: Some(3), column: Some(2) };
        let ed = Prefs { action: "editor".into(), text: String::new(), editor: r"C:\Program Files\Ed\ed.exe".into() };
        assert_eq!(
            plan(&file, &ed, &Around::default(), false),
            Some(Run::Spawn { program: PathBuf::from(r"C:\Program Files\Ed\ed.exe"), args: vec![r"C:\a b\x.rs".into()] })
        );
        let code = Prefs { action: "editor".into(), text: String::new(), editor: "code".into() };
        assert_eq!(
            plan(&file, &code, &Around::default(), false),
            Some(Run::Spawn { program: PathBuf::from("code"), args: vec!["-g".into(), r"C:\a b\x.rs:3:2".into()] })
        );
        let best = Prefs { action: "best editor".into(), text: String::new(), editor: String::new() };
        assert_eq!(
            plan(&file, &best, &Around::default(), true),
            Some(Run::Spawn { program: PathBuf::from("code"), args: vec!["-g".into(), r"C:\a b\x.rs:3:2".into()] })
        );
    }

    #[test]
    fn menu_and_click_agree_on_urls() {
        let docs = crate::url_guess::UrlPrefs::default();
        let loose = crate::url_guess::UrlPrefs { require_slash: false, ..Default::default() };
        let remote = crate::url_guess::UrlPrefs { cwd_is_local: false, ..Default::default() };
        let strict = crate::url_guess::UrlPrefs { conservative: true, ..Default::default() };
        for text in ["exam|ple.com", "foo/b|ar", "https://exa|mple.com", "plain wo|rd"] {
            for prefs in [&docs, &loose, &remote, &strict] {
                let (before, after) = click(text);
                let ctx = ClickContext { before: before.clone(), after: after.clone(), urls: prefs.clone(), ..Default::default() };
                let clicked = matches!(resolve(&ctx), Some(Target::Url(_)));
                assert_eq!(url_like_in(&before, &after, prefs).is_some(), clicked, "{text}");
            }
        }
    }

    #[test]
    fn url_preview_follows_iterm2() {
        // An OSC 8 link always shows its URL.
        assert_eq!(preview_text(&Target::Link("https://a/".into()), None, false), Some("https://a/".into()));
        assert_eq!(preview_text(&Target::Url("https://b".into()), None, true), Some("https://b".into()));
        assert_eq!(preview_text(&Target::Url("https://b".into()), None, false), None);
        let action = Target::Action { rule: 0, action: 0, text: "x".into() };
        assert_eq!(preview_text(&action, Some("https://c"), true), Some("https://c".into()));
        assert_eq!(preview_text(&action, Some("https://c"), false), None);
        assert_eq!(preview_text(&action, None, true), None, "a non Open URL action shows nothing");
        let file = Target::File { path: PathBuf::from("a.txt"), line: None, column: None };
        assert_eq!(preview_text(&file, None, true), None);
        assert_eq!(preview_text(&Target::RawCommand("x".into()), None, true), None);
    }

    #[test]
    fn preview_is_sanitized_and_bounded() {
        let raw = format!("https://a/\u{1b}[31m\u{202e}x\n\u{85}\u{200b}{}", "a".repeat(5000));
        let shown = preview_text(&Target::Link(raw), None, false).unwrap();
        assert!(!shown.chars().any(|c| c.is_control() || matches!(c, '\u{202e}' | '\u{200b}')), "{shown:?}");
        assert!(shown.starts_with("https://a/[31mx"), "{shown:?}");
        assert_eq!(shown.chars().count(), PREVIEW_MAX + 1);
        assert!(shown.ends_with('\u{2026}'));
        let short = preview_text(&Target::Link("https://a/b".into()), None, false).unwrap();
        assert_eq!(short, "https://a/b");
        let action = Target::Action { rule: 0, action: 0, text: "x".into() };
        assert_eq!(preview_text(&action, Some("https://c/\u{7}d"), true).as_deref(), Some("https://c/d"));
    }

    #[test]
    fn hover_probes_no_network_path() {
        // Every mixed-separator spelling of a share is skipped before any probe; a dead
        // address takes about 21 s to answer on Windows.
        let t = std::time::Instant::now();
        for p in [r"\\10.255.255.1\x\y", "//10.255.255.1/x", r"\/10.255.255.1/x", r"/\10.255.255.1\x", r"\\?\UNC\10.255.255.1\x"] {
            assert_eq!(find_path("see ", p, None, &Probe::default()), None, "{p}");
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        // A remote pane probes nothing: an existing file is no target.
        let d = dir("noprobe", &["a.txt"]);
        let (before, after) = click("open a.|txt");
        let ctx = ClickContext { before, after, cwd: Some(d), ..Default::default() };
        assert!(matches!(resolve(&ctx), Some(Target::File { .. })));
        assert_eq!(resolve(&ClickContext { no_probe: true, ..ctx }), None);
    }

    #[test]
    fn file_links_decode_to_no_share() {
        for u in ["file:///%5C%5Cevil/x", "file:////evil/x", "file://%5Cevil/x"] {
            assert!(matches!(guard_open(u, false), Run::Blocked(_)), "{u}");
        }
    }

    #[test]
    fn remote_pane_probes_nothing() {
        let d = dir("remote", &["a.txt"]);
        let (before, after) = click("open a.|txt");
        let ctx = ClickContext { before, after, cwd: Some(d), no_probe: true, respecting: Some((String::new(), "a.txt".into())), ..Default::default() };
        let before_count = PROBES.with(|n| n.get());
        assert!(resolve_in(&ctx).is_none_or(|(t, _, _)| !matches!(t, Target::File { .. })));
        assert_eq!(PROBES.with(|n| n.get()), before_count, "no candidate was probed, in either pass");
        let live = ClickContext { no_probe: false, ..ctx };
        assert!(matches!(resolve(&live), Some(Target::File { .. })));
        assert!(PROBES.with(|n| n.get()) > before_count, "the counter sees real probes");
    }

    #[test]
    fn run_in_window_follows_iterm2() {
        // The one cmd.exe builder (standing rule): `/d /v:off /s /c "..."`, not a bare `/c`.
        assert_eq!(run_in_window("dir"), Run::Window(cmd_command_line("dir")));
        assert_eq!(run_in_window("dir"), Run::Window("cmd.exe /d /v:off /s /c \"dir\"".into()));
        // A trailing `&` with more before it runs the rest hidden.
        assert_eq!(run_in_window("start x &"), Run::Command("start x ".into()));
        // One character is not the detached form.
        assert_eq!(run_in_window("&"), Run::Window(cmd_command_line("&")));
    }

    fn fake_resolve(n: &str) -> Option<PathBuf> {
        match n {
            "h.cmd" => Some(PathBuf::from(r"C:\t\h.cmd")),
            "open.exe" => Some(PathBuf::from(r"C:\t\open.exe")),
            "cmd.exe" => Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
            "pwsh" => Some(PathBuf::from(r"C:\t\pwsh.exe")),
            n if n.contains('\\') => Some(PathBuf::from(n)),
            _ => None,
        }
    }

    fn spawn_of(program: &str, args: &[&str]) -> Result<Run, Refusal> {
        Ok(Run::Spawn { program: PathBuf::from(program), args: args.iter().map(|a| a.to_string()).collect() })
    }

    #[test]
    fn url_handler_follows_iterm2() {
        let plan = |t: &str, u: &str| handler_plan(t, u, &[], &fake_resolve);
        assert_eq!(plan(&format!("open.exe --new {URL_TOKEN}"), "https://a/?q=1%20"), spawn_of(r"C:\t\open.exe", &["--new", "https://a/?q=1%20"]));
        // No marker: the URL is appended as the last argument.
        assert_eq!(plan("open.exe", "https://a/"), spawn_of(r"C:\t\open.exe", &["https://a/"]));
        assert_eq!(plan(&format!(r#""C:\Program Files\B\b.exe" "--u={URL_TOKEN}""#), "https://a/"), spawn_of(r"C:\Program Files\B\b.exe", &["--u=https://a/"]));
        // SECURITY: one argument, byte for byte; no shell.
        let evil = "https://x/%CMDCMDLINE%&calc";
        assert_eq!(plan(&format!("open.exe {URL_TOKEN}"), evil), spawn_of(r"C:\t\open.exe", &[evil]));
        assert_eq!(plan(&format!("h.cmd {URL_TOKEN}"), evil), Err(Refusal::BatchUnsafe));
        for u in ["https://x/a^b", "https://x/a|b", "https://x/a<b", "https://x/a>b", "https://x/a\"b"] {
            assert_eq!(plan(&format!("h.cmd {URL_TOKEN}"), u), Err(Refusal::BatchUnsafe), "{u}");
        }
        assert_eq!(plan(&format!("h.cmd {URL_TOKEN}"), "https://a/b"), spawn_of(r"C:\t\h.cmd", &["https://a/b"]));
        assert_eq!(plan(&format!("cmd.exe /c start \"\" {URL_TOKEN}"), evil), Err(Refusal::Forwarder));
        // P13: an interpreter that re-parses its arguments gets the same refusal.
        assert_eq!(plan(&format!("pwsh -Command Start-Process {URL_TOKEN}"), "https://x/?a=1;calc"), Err(Refusal::BatchUnsafe));
        assert_eq!(plan(&format!("{URL_TOKEN} x"), "https://a/"), Err(Refusal::UrlInProgram));
        assert_eq!(plan("open.exe", "https://a/\nx"), Err(Refusal::ControlCharacter));
        assert_eq!(plan("open.exe", "https://a/\rx"), Err(Refusal::ControlCharacter));
        assert_eq!(plan("   ", "https://a/"), Err(Refusal::Empty));
        // P14: a program found nowhere is refused, never started "as written".
        assert_eq!(plan("nothere.exe", "https://a/"), Err(Refusal::Unresolved));
        // The URL can never add arguments or reach the program.
        assert_eq!(plan(&format!("open.exe {URL_TOKEN}"), "https://a/ --evil \"x y\""), spawn_of(r"C:\t\open.exe", &["https://a/ --evil \"x y\""]));
        assert_eq!(Refusal::Empty.text(), "");
        assert_eq!(Refusal::UrlInProgram.text(), "Termurion did not run the URL handler command: the URL cannot name the program.");
        assert_eq!(Refusal::ControlCharacter.text(), "Termurion did not pass this URL to the URL handler command: it contains a control character.");
        assert_eq!(Refusal::BatchUnsafe.text(), "Termurion did not pass this URL to the URL handler command: a batch file or cmd.exe would read its special characters.");
        assert_eq!(Refusal::Unresolved.text(), HANDLER_FAILED);
        assert_eq!(HANDLER_TITLE, "URL Handler");
    }

    #[test]
    fn split_follows_commandlinetoargvw() {
        let v = |l: &str| split_command_line(l);
        assert_eq!(v(r#"p "b c" d\"e x\\"y z""#), ["p", "b c", "d\"e", "x\\y z"]);
        assert_eq!(v(r#""C:\x y\p.exe" z"#), ["C:\\x y\\p.exe", "z"]);
        assert_eq!(v("a  b\tc"), ["a", "b", "c"]);
        assert_eq!(v(r#"p "" q"#), ["p", "", "q"]);
        // shell32: `""` inside quotes is a literal quote that also ends the quoted run.
        assert_eq!(v(r#"p "a""b c" d"#), ["p", "a\"b", "c d"]);
    }

    /// Final wave minor 11: a relative name with a folder is never resolved against Termurion's own folder.
    #[test]
    fn a_relative_program_with_a_folder_is_not_resolved() {
        for name in [r"sub\p.exe", r"..\p.exe", "sub/p.exe", r".\p.exe"] {
            assert_eq!(resolve_program(name, r"C:\t", ".EXE", &|_| true), None, "{name}");
        }
        assert_eq!(resolve_program("p", r"C:\t", ".EXE", &|_| true), Some(PathBuf::from(r"C:\t\p.exe")), "a bare name still walks PATH");
        assert_eq!(resolve_program(r"C:\x\p.exe", "", ".EXE", &|_| true), Some(PathBuf::from(r"C:\x\p.exe")), "an absolute path still resolves");
    }

    /// Final wave minor 9: a Send Text never types a control character a value brought.
    #[test]
    fn send_text_refuses_control_characters_in_values_only() {
        let send = Prefs { action: "send text".into(), text: "open \\1 \\3\\n".into(), editor: String::new() };
        let file = Target::File { path: PathBuf::from(r"C:\a b\x.rs"), line: None, column: None };
        let around = Around { prefix: "", suffix: "", cwd: "" };
        assert_eq!(plan(&file, &send, &around, false), Some(Run::Send("open C:\\a b\\x.rs \\n".into())), "the control: the template's own text goes");
        let tainted = Around { prefix: "a\nrm -rf x", suffix: "", cwd: "" };
        assert_eq!(plan(&file, &send, &tainted, false), Some(Run::Blocked(SEND_REFUSED.into())));
        let named = Target::File { path: PathBuf::from("C:\\a\r\nb.rs"), line: None, column: None };
        assert_eq!(plan(&named, &send, &around, false), Some(Run::Blocked(SEND_REFUSED.into())));
    }

    #[test]
    fn resolve_program_walks_path_and_pathext() {
        let seen = std::cell::RefCell::new(Vec::<PathBuf>::new());
        let exists = |p: &Path| {
            seen.borrow_mut().push(p.to_path_buf());
            p == Path::new(r"C:\t\h.cmd")
        };
        assert_eq!(resolve_program("h", r"C:\t;\\srv\s", ".EXE;.CMD", &exists), Some(PathBuf::from(r"C:\t\h.cmd")));
        assert!(seen.borrow().iter().all(|p| !p.to_string_lossy().starts_with(r"\\")), "never asked about the share: {:?}", seen.borrow());
        // The share first: skipped, the next folder still found.
        assert_eq!(resolve_program("h", r"\\srv\s;C:\t", ".EXE;.CMD", &exists), Some(PathBuf::from(r"C:\t\h.cmd")));
        assert_eq!(resolve_program("nope", r"C:\t", ".EXE", &exists), None);
        // A name with a folder: as written, PATHEXT tried when it has no extension, nothing asked of a share.
        let only = |p: &Path| p == Path::new(r"C:\x\p.exe");
        assert_eq!(resolve_program(r"C:\x\p", "", ".EXE", &only), Some(PathBuf::from(r"C:\x\p.exe")));
        assert_eq!(resolve_program(r"C:\x\p.exe", "", ".EXE", &only), Some(PathBuf::from(r"C:\x\p.exe")));
        assert_eq!(resolve_program(r"\\srv\s\p.exe", "", ".EXE", &|_| true), None);
        // An extensionless file is no program (VS Code's `code`).
        let code = |p: &Path| p == Path::new(r"C:\t\code");
        assert_eq!(resolve_program("code", r"C:\t", ".EXE;.CMD", &code), None);
    }

    #[test]
    fn batch_programs_are_known() {
        assert!(is_batch(Path::new(r"C:\t\H.BAT")));
        assert!(is_batch(Path::new(r"C:\t\h.cmd")));
        assert!(is_batch(Path::new(r"C:\Windows\System32\cmd.exe")));
        assert!(is_batch(Path::new(r"C:\Windows\System32\CMD.EXE")));
        assert!(!is_batch(Path::new(r"C:\t\open.exe")));
    }

    #[test]
    fn the_handler_is_asked_only_for_web_urls() {
        assert!(handler_applies(&Run::Open("https://a/".into()), false));
        assert!(handler_applies(&Run::Open("HTTP://a/".into()), false));
        assert!(handler_applies(&Run::Open("mailto:a@b".into()), false));
        assert!(handler_applies(&Run::Open("ftp://a/".into()), false));
        assert!(!handler_applies(&Run::Open("https://a/".into()), true), "a Target::File");
        assert!(!handler_applies(&Run::Open("file:///C:/a".into()), false));
        assert!(!handler_applies(&Run::Open(r"C:\a\b.txt".into()), false));
        assert!(!handler_applies(&Run::Open(r"\\evil\s\x".into()), false));
        assert!(!handler_applies(&Run::Command("https://a/".into()), false));
    }

    #[cfg(windows)]
    #[test]
    fn split_matches_the_system() {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let table = [
            r#"p a b"#,
            r#"p "b c" d"#,
            r#""C:\Program Files\x\p.exe" a"#,
            r#"p a\b c\\d"#,
            r#"p a\"b"#,
            r#"p a\\"b c" d"#,
            r#"p "" x"#,
            r#"p "a""b c" d"#,
            r#"p \\\"x y"#,
            r#"p "x y"z w"#,
            r#"p a"b c"d e"#,
            "p a\tb  c",
            r#"p """a b""#,
            r#"p "a b""""#,
            r#"p "a"" b"#,
            r#"p a"" b"#,
        ];
        for line in table {
            let wide: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
            let mut n = 0;
            let ours = split_command_line(line);
            let theirs: Vec<String> = unsafe {
                let argv = CommandLineToArgvW(wide.as_ptr(), &mut n);
                assert!(!argv.is_null());
                let out = (0..n as usize)
                    .map(|i| {
                        let mut len = 0;
                        while *(*argv.add(i)).add(len) != 0 {
                            len += 1;
                        }
                        String::from_utf16_lossy(std::slice::from_raw_parts(*argv.add(i), len))
                    })
                    .collect();
                LocalFree(argv as _);
                out
            };
            assert_eq!(ours, theirs, "{line}");
        }
    }

    /// 13q S6/P11: Send Text and Run Coprocess act on the pane's own shell; an ended session has
    /// none, so they say so instead of dropping the text. Every other plan acts outside the pane.
    #[test]
    fn only_send_and_coprocess_need_a_live_session() {
        assert!(needs_live_session(&Run::Send("x".into())));
        assert!(needs_live_session(&Run::Coprocess("x".into())));
        for r in [Run::Open("https://a.b".into()), Run::Command("x".into()), Run::Blocked("x".into()), Run::Reveal("C:\\x".into()), Run::Window("x".into()), Run::OpenLocal { path: "C:\\x".into(), has_code: false }] {
            assert!(!needs_live_session(&r), "{r:?}");
        }
    }
}
