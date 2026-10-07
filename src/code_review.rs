//! Code Review mode (milestone 13y1, Task 7): the saved prompts, the review member's command as an
//! argument vector, the system prompt's temp file, and the rules of the two auto behaviors. Pure except the
//! temp file. Review text goes back into a coding agent's terminal, so escape injection and argument
//! injection are the threats this module is written against.
//!
//! Citations: `CRS` `CodeReviewPromptStore.swift`, `CRV` `CodeReviewPromptView.swift`, `CRM`
//! `CodeReviewPromptManagerWindowController.swift`, `PCR` `PTYSession+CodeReviewPrompt.swift`, `WPP`
//! `iTermWorkgroupPeerPort.swift` under `~/gitrepos/iterm2-ref/sources/Workgroups`.

use crate::workgroup::Mode;
use std::path::Path;
use std::time::Duration;

pub const TITLE: &str = "Code review prompt:";
pub const START: &str = "Start";
pub const MENU_HEADER: &str = "Prompts";
pub const SAVE_AS_NEW: &str = "Save Current as New\u{2026}";
pub const MANAGE: &str = "Manage Prompts\u{2026}";
pub const SAVE_TITLE: &str = "Name this prompt";
pub const SAVE_TEXT: &str = "Saved prompts can be re-loaded from the Prompts pulldown.";
pub const NAME_PLACEHOLDER: &str = "Prompt name";
pub const MANAGER_TITLE: &str = "Code Review Prompts";
pub const NAME_LABEL: &str = "Name:";
pub const UNTITLED: &str = "Untitled";
pub const PROMPT_LABEL: &str = "Prompt:";
pub const EMPTY_PLACEHOLDER: &str = "Select a prompt on the left to edit it, or click + to add a new prompt.";
pub const NEW_PROMPT: &str = "New Prompt";
pub const DEFAULT_NAME: &str = "Default";

/// The stored keys (`CRS:45-46`).
pub const KEY: &str = "CodeReviewSavedPrompts";
pub const LAST_KEY: &str = "NoSyncCodeReviewLastSelectedPromptUUID";

/// P10: the write point's bounds.
pub const MAX_PROMPTS: usize = 64;
pub const MAX_NAME: usize = 128;
pub const MAX_TEXT: usize = 64 * 1024;
/// B5: the prompt is one argument of at most this many characters (cut with a banner, never silently).
pub const MAX_PROMPT_CHARS: usize = 16 * 1024;
/// B5: the system prompt written to the temp file is at most this many bytes.
pub const SYSTEM_PROMPT_CAP: usize = 64 * 1024;
/// B6: an auto-send over this many bytes is not sent.
pub const MAX_SEND_BYTES: usize = 256 * 1024;
/// The most UTF-16 units of a whole command line (CreateProcess takes 32767).
const MAX_LINE_UNITS: usize = 32_000;
/// P17: the longest wait before the auto-send's Return.
const MAX_SUBMIT_DELAY: f64 = 60.0;

/// `CodeReviewSavedPrompt { uuid, name, text }` (`CRS:12-16`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedPrompt {
    pub uuid: String,
    pub name: String,
    pub text: String,
}

/// An upper-case uuid, as `UUID().uuidString` writes one.
pub fn fresh_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// Whether `s` has a uuid's shape: 36 characters of hex digits and dashes.
pub fn uuid_ok(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The prompt to preselect: the last selected one's text, else the first's, else "" (`CRS:131-137`).
pub fn default_prompt_text(prompts: &[SavedPrompt], last_selected: Option<&str>) -> String {
    last_selected.and_then(|u| prompts.iter().find(|p| p.uuid == u)).or_else(|| prompts.first()).map(|p| p.text.clone()).unwrap_or_default()
}

/// The names the orchestration prompt lists (`ChatAgent.swift:989-999`).
pub fn saved_prompt_names(prompts: &[SavedPrompt]) -> String {
    if prompts.is_empty() {
        return "(none saved)".to_string();
    }
    prompts.iter().map(|p| format!("  - {}", p.name)).collect::<Vec<_>>().join("\n")
}

/// The stored list and whether it was seeded. The whole `[SavedPrompt]` decodes or none does; anything
/// else seeds one prompt named "Default" with `legacy` (the "AI Prompt for Code Review" global). A decoded
/// list is `bounded` here, the one read every reader takes (P10): whatever wrote the value (remote prefs,
/// the Stored tab), the orchestration prompt, the pulldown and the manager see it bounded.
pub fn load_prompts(json: Option<&str>, legacy: &str) -> (Vec<SavedPrompt>, bool) {
    if let Some(list) = json.and_then(|j| serde_json::from_str::<Vec<SavedPrompt>>(j).ok()) {
        return (bounded(list), false);
    }
    (vec![SavedPrompt { uuid: fresh_uuid(), name: DEFAULT_NAME.to_string(), text: legacy.to_string() }], true)
}

/// Whether a seed is written back: only a missing or empty value. A malformed one stays on disk until the
/// first edit (P10): the seeded "Default" lives in memory.
pub fn should_persist_seed(json: Option<&str>) -> bool {
    json.is_none_or(|j| j.trim().is_empty())
}

/// The saved prompts the settings hold: the stored JSON, or the seed ("Default" from the "AI Prompt for
/// Code Review" global) in memory (`CRS:64-92`).
pub fn prompts_of(g: &crate::globals::Globals) -> Vec<SavedPrompt> {
    let raw = g.string(KEY);
    load_prompts(Some(raw.as_str()).filter(|r| !r.trim().is_empty()), &g.string(crate::ai_prompts::CODE_REVIEW_KEY)).0
}

/// The uuid of the prompt last selected, when one is stored.
pub fn last_selected(g: &crate::globals::Globals) -> Option<String> {
    Some(g.string(LAST_KEY)).filter(|u| !u.is_empty())
}

/// The JSON `CodeReviewSavedPrompts` stores.
pub fn encode_prompts(prompts: &[SavedPrompt]) -> String {
    serde_json::to_string(prompts).unwrap_or_else(|_| "[]".to_string())
}

/// P10: the list as the write point and every read keep it: at most 64 prompts, a name through
/// `display_text` (128 characters), a text of at most 64 KiB with no NUL, a uuid of the right shape and
/// unique. A uuid that is not is replaced by one made from its place in the list, so every read of the same
/// value gives the same uuids (a fresh one only if that one is taken).
pub fn bounded(prompts: Vec<SavedPrompt>) -> Vec<SavedPrompt> {
    let mut seen: Vec<String> = Vec::new();
    let kept: Vec<SavedPrompt> = prompts.into_iter().take(MAX_PROMPTS).collect();
    let valid: Vec<String> = kept.iter().filter(|p| uuid_ok(&p.uuid)).map(|p| p.uuid.clone()).collect();
    kept.into_iter()
        .enumerate()
        .map(|(i, p)| {
            let placed = format!("00000000-0000-4000-8000-{i:012X}");
            let uuid = if uuid_ok(&p.uuid) && !seen.contains(&p.uuid) {
                p.uuid
            } else if !seen.contains(&placed) && !valid.contains(&placed) {
                placed
            } else {
                fresh_uuid()
            };
            seen.push(uuid.clone());
            let name = crate::inert_files::display_text(&p.name, MAX_NAME, false);
            SavedPrompt { uuid, name, text: cut_bytes(&p.text.replace('\0', ""), MAX_TEXT) }
        })
        .collect()
}

/// `s` cut to at most `max` bytes at a character boundary.
fn cut_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// `base` made unique among `taken` ("New Prompt", "New Prompt 2", ...).
pub fn unique_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken.contains(n)).unwrap_or_else(|| base.to_string())
}

// --- The command (B5) ---

/// The review member's command as arguments. `cut` is true when the prompt was cut to the cap.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewArgv {
    pub argv: Vec<String>,
    pub cut: bool,
}

/// The prompt as one argument: line ends are LF (`PCR:152`), no NUL, at most `MAX_PROMPT_CHARS`.
pub fn prepare_prompt(text: &str) -> Result<(String, bool), String> {
    if text.contains('\0') {
        return Err("The prompt holds a NUL character, so the review did not start.".to_string());
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let cut = normalized.chars().count() > MAX_PROMPT_CHARS;
    Ok((if cut { normalized.chars().take(MAX_PROMPT_CHARS).collect() } else { normalized }, cut))
}

/// The person's template split into words, quotes as the template writes them: blanks separate, and a
/// `'...'` or `"..."` group is one piece of a word with its quote characters dropped. A quote belongs to
/// the TEMPLATE, never to a value that is put in later.
fn template_words(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    for c in template.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (None, ' ' | '\t') => {
                if has || !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                    has = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if has || !word.is_empty() {
        out.push(word);
    }
    out
}

/// One word with its expressions put in, left to right in one pass (a value is never scanned again).
/// Returns the text and whether the word began with the prompt.
fn expand_word(word: &str, prompt: &str, system_prompt_file: &Path, resources: &Path, base: &str) -> Result<(String, bool), String> {
    const BUNDLE: &str = "iterm2.appBundlePath)/Contents/Resources";
    let mut out = String::new();
    let mut rest = word;
    let mut starts_with_prompt = false;
    while let Some(at) = rest.find("\\(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        if let Some(r) = after.strip_prefix("codeReviewPrompt)") {
            starts_with_prompt |= out.is_empty();
            out.push_str(prompt);
            rest = r;
        } else if let Some(r) = after.strip_prefix("codeReviewSystemPromptFile)") {
            out.push_str(&system_prompt_file.to_string_lossy());
            rest = r;
        } else if let Some(r) = after.strip_prefix("gitBase)") {
            if !crate::workgroup_run::base_ok(base) {
                return Err("The base for this workgroup's command is not a usable name, so nothing started.".to_string());
            }
            out.push_str(base);
            rest = r;
        } else if let Some(r) = after.strip_prefix(BUNDLE) {
            out.push_str(&resources.to_string_lossy());
            match r.strip_prefix('/') {
                Some(r2) => {
                    out.push('\\');
                    rest = r2;
                }
                None => rest = r,
            }
        } else {
            return Err("A workgroup command uses an expression Tandem does not run here, so nothing started.".to_string());
        }
    }
    out.push_str(rest);
    Ok((out, starts_with_prompt))
}

/// The review command (`WorkgroupPresets` Code Review) as arguments: the person's template split into words
/// first, each value put inside its own word. The prompt is one argument (a prompt that starts with a dash
/// gets a leading space so it can never be read as an option), the paths are exactly the paths. The program
/// is the first word and is never an expression. An error is the banner text and nothing starts.
pub fn review_argv(template: &str, prompt: &str, system_prompt_file: &Path, resources: &Path, base: &str) -> Result<ReviewArgv, String> {
    let words = template_words(template);
    let Some(program) = words.first() else { return Err("This member has no command to run.".to_string()) };
    if program.contains("\\(") {
        return Err("A workgroup command uses an expression Tandem does not run here, so nothing started.".to_string());
    }
    let (prompt, cut) = prepare_prompt(prompt)?;
    let mut argv = vec![program.clone()];
    for w in &words[1..] {
        let (mut text, starts) = expand_word(w, &prompt, system_prompt_file, resources, base)?;
        if starts && text.starts_with('-') {
            text.insert(0, ' ');
        }
        argv.push(text);
    }
    Ok(ReviewArgv { argv, cut })
}

/// Whether a whole command line fits CreateProcess (refused above, never truncated).
pub fn line_fits(line: &str) -> bool {
    line.encode_utf16().count() <= MAX_LINE_UNITS
}

// --- The system prompt's temp file (B5) ---

const FILE_PREFIX: &str = "code-review-system-prompt-";

/// A name nobody can guess: 16 random hex digits (never the pane id).
pub fn system_prompt_file_name() -> String {
    format!("{FILE_PREFIX}{}.txt", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

/// Whether `name` is a file `write_system_prompt` makes.
pub fn is_review_file(name: &str) -> bool {
    name.strip_prefix(FILE_PREFIX).and_then(|r| r.strip_suffix(".txt")).is_some_and(|h| h.len() == 16 && h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
}

/// The system prompt as written: at most 64 KiB.
pub fn cap_system_prompt(text: &str) -> String {
    cut_bytes(text, SYSTEM_PROMPT_CAP)
}

/// Writes the system prompt to a NEW owner-only file in `dir` (a judged folder): an unguessable name,
/// `CREATE_NEW`, a DACL of the person alone. Returns the file's path.
#[cfg(windows)]
pub fn write_system_prompt(dir: &crate::localfs::LocalPath, text: &str) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    let name = system_prompt_file_name();
    let entry = dir.unfollowed(&name).ok_or_else(crate::localfs::refused)?;
    let mut f = crate::localfs::create_owner_only(&entry)?;
    f.write_all(cap_system_prompt(text).as_bytes())?;
    f.sync_all()?;
    Ok(entry.path().to_path_buf())
}

/// Removes every review file in `dir` (the next launch sweeps what an earlier run left).
pub fn sweep(dir: &crate::localfs::LocalPath) {
    let Ok(entries) = crate::localfs::entries(dir) else { return };
    for e in entries.iter().filter(|e| e.name.to_str().is_some_and(is_review_file)) {
        let _ = e.unlink();
    }
}

// --- The auto behaviors ---

/// The state word of a status: "idle", "working", "waiting", else "unknown" (`reportedState`).
pub fn state_word(status: Option<&str>) -> &'static str {
    match status.map(str::to_lowercase).as_deref() {
        Some("idle") => "idle",
        Some("working") => "working",
        Some("waiting") => "waiting",
        _ => "unknown",
    }
}

/// `WPP:216-230`: the main session went from working to idle, its toggle is on and exactly one review
/// session exists.
pub fn should_auto_request(prev: Option<&str>, now: &str, toggle: bool, review_sessions: usize) -> bool {
    prev == Some("working") && now == "idle" && toggle && review_sessions == 1
}

/// `WPP:261-300`: the text to send to the main session, or None. `clippings` are each clipping as it is
/// sent (`clippings::format`'s form); `separator` is the Vim-syntax clipping separator.
pub fn clippings_to_send(prev: Option<&str>, now: &str, mode: Mode, toggle: bool, main_busy: bool, clippings: &[String], separator: &str) -> Option<String> {
    if prev != Some("working") || now != "idle" || mode != Mode::CodeReview || !toggle || main_busy {
        return None;
    }
    let text = clippings.join(&crate::keyaction::expand(separator, crate::keyaction::Escaping::Vim));
    (!text.is_empty()).then_some(text)
}

/// The delay before the auto-send's Return (`WorkgroupAutoSendSubmitDelay`): negative or not finite is
/// zero, and at most 60 seconds (P17).
pub fn submit_delay(setting: f64) -> Duration {
    if !setting.is_finite() || setting <= 0.0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(setting.min(MAX_SUBMIT_DELAY))
}

/// The coding agent a review was started for: the main session's foreground job at that moment (the job
/// watcher's name for it, and its process id). A shell or an unknown job is no agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub pid: u32,
}

/// The agent a foreground job `(name, pid)` is, or None for a shell (bash, zsh, fish, pwsh, cmd, ...) or
/// no job.
pub fn agent_of(job: Option<(&str, u32)>) -> Option<Agent> {
    let (name, pid) = job?;
    let name = crate::session_state::normalize_job(name);
    (!name.is_empty() && !crate::session_state::is_shell(Some(&name))).then_some(Agent { name, pid })
}

/// Whether the main session's foreground job now is the agent the review was started for: the same
/// program in the same process. None started for (the main session was at a shell prompt then) is never.
pub fn agent_present(started_for: Option<&Agent>, now: Option<(&str, u32)>) -> bool {
    started_for.is_some_and(|a| agent_of(now).as_ref() == Some(a))
}

/// What an auto-send knows about its target (B6).
#[derive(Clone, Copy, Debug)]
pub struct SendTarget {
    pub alive: bool,
    pub password: bool,
    pub bracketed: bool,
    pub bytes: usize,
    /// The pane still has the workgroup's main-session identity.
    pub is_main: bool,
    /// The main session's foreground job is a shell (its prompt would run the text).
    pub shell: bool,
    /// The main session's foreground job is the coding agent the review was started for (`agent_present`).
    pub agent: bool,
}

/// Why an auto-send must not go: nothing is sent and a banner says this.
pub fn send_refusal(t: &SendTarget) -> Option<&'static str> {
    if !t.alive {
        return Some("The main session has ended, so the review was not sent.");
    }
    if !t.is_main {
        return Some("That session is no longer the workgroup's main session, so the review was not sent.");
    }
    if t.password {
        return Some("The main session is at a password prompt, so the review was not sent.");
    }
    if t.shell {
        return Some("The main session is at a shell prompt (its coding agent is not running), so the review was not sent.");
    }
    if !t.agent {
        return Some("The main session is no longer running the coding agent the review was started for, so the review was not sent.");
    }
    if !t.bracketed {
        return Some("The main session has not asked for bracketed paste (a program that reads pasted text is not running there), so the review was not sent.");
    }
    if t.bytes > MAX_SEND_BYTES {
        return Some("The review is over 256 KiB, so it was not sent. Send the clippings from the panel instead.");
    }
    None
}

/// The paste an auto-send makes: newlines as CR, controls removed, one bracket (B6).
pub fn auto_job(text: &str) -> crate::paste::Job {
    crate::paste::Job::new(text, crate::paste::NEWLINES | crate::paste::CONTROLS | crate::paste::BRACKET | crate::paste::PLAIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workgroup::Mode;
    use std::path::Path;

    const REVIEW: &str = "claude \\(codeReviewPrompt) --append-system-prompt-file '\\(codeReviewSystemPromptFile)' --settings '\\(iterm2.appBundlePath)/Contents/Resources/code-review-settings.txt'";

    fn argv(prompt: &str) -> Result<ReviewArgv, String> {
        review_argv(REVIEW, prompt, Path::new(r"C:\T\p.txt"), Path::new(r"C:\Tandem\resources"), "HEAD")
    }

    /// Review Focus 6 (B5): the command is an argument vector, and the texts are iTerm2's.
    #[test]
    fn review_overlay_follows_iterm2() {
        let claude = crate::workgroup::claude_code::config();
        let review = claude.sessions.iter().find(|s| s.mode == Mode::CodeReview).expect("the Claude Code workgroup has a review member");
        assert_eq!(review.command, REVIEW, "the command the template ships");
        let r = argv("check tests").unwrap();
        assert_eq!(r.argv, ["claude", "check tests", "--append-system-prompt-file", r"C:\T\p.txt", "--settings", r"C:\Tandem\resources\code-review-settings.txt"]);
        assert!(!r.cut);
        assert_eq!(
            (TITLE, START, MENU_HEADER, SAVE_AS_NEW, MANAGE, SAVE_TITLE, SAVE_TEXT, NAME_PLACEHOLDER),
            ("Code review prompt:", "Start", "Prompts", "Save Current as New\u{2026}", "Manage Prompts\u{2026}", "Name this prompt", "Saved prompts can be re-loaded from the Prompts pulldown.", "Prompt name")
        );
        assert_eq!(
            (MANAGER_TITLE, NAME_LABEL, UNTITLED, PROMPT_LABEL, NEW_PROMPT, EMPTY_PLACEHOLDER),
            ("Code Review Prompts", "Name:", "Untitled", "Prompt:", "New Prompt", "Select a prompt on the left to edit it, or click + to add a new prompt.")
        );
        assert_eq!(unique_name("New Prompt", &["New Prompt".to_string()]), "New Prompt 2");
        assert_eq!(unique_name("New Prompt", &["New Prompt".to_string(), "New Prompt 2".to_string()]), "New Prompt 3");
        assert_eq!(unique_name("New Prompt", &[]), "New Prompt");
        assert_eq!((KEY, LAST_KEY), ("CodeReviewSavedPrompts", "NoSyncCodeReviewLastSelectedPromptUUID"));
    }

    /// B5: whatever the prompt holds, it is exactly one argument of the line CreateProcess is given.
    #[test]
    fn review_prompt_is_one_argument() {
        let long = "a".repeat(20 * 1024);
        let prompts = ["say \"hi\"", "ends with a backslash\\", "a\\\" b", "--dangerously-skip-permissions", "line one\nline two", "crlf\r\ncr\rlf", "' ; calc & ' %PATH% ^ | >", &long];
        for p in prompts {
            let r = argv(p).unwrap();
            assert_eq!(r.argv.len(), 6, "{p:?}");
            let want = p.replace("\r\n", "\n").replace('\r', "\n");
            let want: String = want.chars().take(MAX_PROMPT_CHARS).collect();
            let want = if want.starts_with('-') { format!(" {want}") } else { want };
            assert_eq!(r.argv[1], want, "{p:?}");
            assert_eq!(r.cut, p.chars().count() > MAX_PROMPT_CHARS);
            // What claude.exe is handed after CreateProcess's split is that argument alone.
            let line = crate::workgroup_run::argv_line(Path::new(r"C:\bin\claude.exe"), &r.argv[1..]).unwrap();
            let back = crate::semantic::split_command_line(&line);
            assert_eq!(back[0], r"C:\bin\claude.exe");
            assert_eq!(&back[1..], &r.argv[1..], "{p:?}");
            assert!(line_fits(&line) || p.len() > 1024, "{p:?}");
        }
        assert!(argv("a\0b").is_err(), "a NUL never reaches a command line");
        // 16 KiB cut, never silent.
        assert_eq!(argv(&long).unwrap().argv[1].chars().count(), MAX_PROMPT_CHARS);
        assert!(argv(&long).unwrap().cut);
        // A template with an expression this task does not run, or an expression as the program, is refused.
        let bad = |t: &str| review_argv(t, "p", Path::new(r"C:\T\p.txt"), Path::new(r"C:\R"), "HEAD").is_err();
        assert!(bad("claude \\(other)"));
        assert!(bad("\\(codeReviewPrompt) x"));
        assert!(bad(""));
        // The prompt as a value inside a longer token stays inside it.
        let r = review_argv("claude --prompt=\\(codeReviewPrompt) --gitbase \\(gitBase)", "--x y", Path::new(r"C:\T\p.txt"), Path::new(r"C:\R"), "main").unwrap();
        assert_eq!(r.argv, ["claude", "--prompt=--x y", "--gitbase", "main"]);
        // A base that is an option is refused.
        assert!(review_argv("claude \\(gitBase)", "p", Path::new(r"C:\T\p.txt"), Path::new(r"C:\R"), "--evil").is_err());
        // The whole line must fit CreateProcess.
        assert!(!line_fits(&"x".repeat(40_000)));
        assert!(line_fits("claude x"));
    }

    /// B5: the system prompt's temp file has an unguessable name and nobody else's access.
    #[test]
    fn review_temp_file_is_owner_only_and_unguessable() {
        let (a, b) = (system_prompt_file_name(), system_prompt_file_name());
        assert_ne!(a, b);
        for n in [&a, &b] {
            let hex = n.strip_prefix("code-review-system-prompt-").and_then(|r| r.strip_suffix(".txt")).expect("the name's shape");
            assert_eq!(hex.len(), 16);
            assert!(hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{n}");
        }
        assert!(is_review_file(&a) && !is_review_file("notes.txt") && !is_review_file("code-review-system-prompt-.txt.exe"));
        assert_eq!(cap_system_prompt(&"x".repeat(70_000)).len(), SYSTEM_PROMPT_CAP);
        assert_eq!(cap_system_prompt("été").len(), "été".len());
    }

    #[cfg(windows)]
    #[test]
    fn review_temp_file_is_made_owner_only() {
        let base = std::env::temp_dir().join(format!("tandem-review-{}", std::process::id()));
        let dir = crate::localfs::LocalPath::judge(&base).or_else(|| {
            std::fs::create_dir_all(&base).ok()?;
            crate::localfs::LocalPath::judge(&base)
        });
        let dir = dir.expect("a local temp folder");
        crate::localfs::create_dir_all(&dir).unwrap();
        let path = write_system_prompt(&dir, "system text").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "system text");
        let entry = dir.unfollowed(path.file_name().unwrap()).unwrap();
        let sddl = crate::localfs::dacl_sddl(&entry).unwrap();
        assert!(sddl.contains("D:P(A;;FA;;;"), "a protected DACL of one allow entry: {sddl}");
        assert_eq!(sddl.matches("(A;").count(), 1, "{sddl}");
        // A second write is a new name; the sweep removes ours and keeps other files.
        let other = dir.unfollowed("keep.txt").unwrap();
        drop(crate::localfs::create_owner_only(&other).unwrap());
        let again = write_system_prompt(&dir, "x").unwrap();
        assert_ne!(path, again);
        sweep(&dir);
        assert!(!path.exists() && !again.exists());
        assert!(dir.path().join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn idle() -> &'static str {
        "idle"
    }

    /// `WPP:261-300`: the text is sent only on a working to idle edge of a review member with the toggle on.
    #[test]
    fn auto_send_follows_iterm2() {
        let clips = vec!["**t**\nd".to_string(), "e".to_string()];
        let sep = "\\n--\\n";
        let go = |prev: Option<&str>, now: &str, mode: Mode, toggle: bool, busy: bool, c: &[String]| clippings_to_send(prev, now, mode, toggle, busy, c, sep);
        assert_eq!(go(Some("working"), idle(), Mode::CodeReview, true, false, &clips).as_deref(), Some("**t**\nd\n--\ne"));
        assert_eq!(go(Some("working"), idle(), Mode::CodeReview, true, false, &["a".into(), "b".into()]).as_deref(), Some("a\n--\nb"));
        assert_eq!(go(Some("working"), idle(), Mode::CodeReview, true, true, &clips), None, "never inject into a busy main session");
        assert_eq!(go(Some("working"), idle(), Mode::CodeReview, false, false, &clips), None, "toggle off");
        assert_eq!(go(Some("working"), idle(), Mode::CodeReview, true, false, &[]), None, "nothing to send");
        assert_eq!(go(Some("working"), idle(), Mode::Regular, true, false, &clips), None, "not a review member");
        assert_eq!(go(Some("idle"), idle(), Mode::CodeReview, true, false, &clips), None, "idle to idle");
        assert_eq!(go(None, idle(), Mode::CodeReview, true, false, &clips), None, "a first status never fires");
        assert_eq!(go(Some("working"), "waiting", Mode::CodeReview, true, false, &clips), None);
        assert_eq!(submit_delay(0.501), std::time::Duration::from_millis(501));
        assert_eq!(submit_delay(-1.0), std::time::Duration::ZERO);
        assert_eq!(submit_delay(f64::NAN), std::time::Duration::ZERO);
        assert_eq!(submit_delay(5000.0), std::time::Duration::from_secs(60), "P17: a typo cannot hold the Return for the session");
        let row = crate::schema::find_global("WorkgroupAutoSendSubmitDelay").expect("the advanced row");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind, row.default.clone()), ("Advanced", "Pasteboard", crate::schema::Kind::Float, Some(toml::Value::Float(0.501))));
        assert_eq!(
            row.help,
            "Delay in seconds before the Return that submits an auto-sent code review.\nWhen the code-review workgroup toolbar auto-sends review results to the main session, it pastes the results and then sends a Return to submit them. This is how long it waits, after the pasted results finish, before sending that Return. A paste-debouncing TUI (such as Claude Code) processes a paste and a Return that arrive together as one batch, so the Return can be swallowed and the review never submits; the delay makes the Return land in a later read so it submits. The default matches Claude Code\u{2019}s 500 ms paste-settling window. Set to 0 to send it immediately."
        );
    }

    /// `WPP:216-230`.
    #[test]
    fn auto_request_follows_iterm2() {
        assert!(should_auto_request(Some("working"), "idle", true, 1));
        assert!(!should_auto_request(Some("working"), "idle", true, 2), "two review sessions are ambiguous");
        assert!(!should_auto_request(Some("working"), "idle", true, 0));
        assert!(!should_auto_request(Some("idle"), "idle", true, 1), "idle to idle");
        assert!(!should_auto_request(None, "idle", true, 1));
        assert!(!should_auto_request(Some("working"), "idle", false, 1), "toggle off");
        assert!(!should_auto_request(Some("working"), "working", true, 1));
        assert_eq!((state_word(Some("Idle")), state_word(Some("working")), state_word(Some("WAITING")), state_word(Some("busy")), state_word(None)), ("idle", "working", "waiting", "unknown", "unknown"));
    }

    /// B6: the refusals of an auto-send, in the order they are told.
    #[test]
    fn auto_send_needs_bracketed_paste() {
        let ok = SendTarget { alive: true, password: false, bracketed: true, bytes: 100, is_main: true, shell: false, agent: true };
        assert_eq!(send_refusal(&ok), None);
        assert!(send_refusal(&SendTarget { bracketed: false, ..ok }).is_some_and(|t| t.contains("bracketed")), "a plain shell would run each line");
        assert!(send_refusal(&SendTarget { alive: false, ..ok }).is_some());
        assert!(send_refusal(&SendTarget { is_main: false, ..ok }).is_some(), "the pane is no longer the main session");
        assert!(send_refusal(&SendTarget { bytes: MAX_SEND_BYTES + 1, ..ok }).is_some());
        assert_eq!(send_refusal(&SendTarget { bytes: MAX_SEND_BYTES, ..ok }), None);
    }

    #[test]
    fn auto_send_never_into_a_password_prompt() {
        let t = SendTarget { alive: true, password: true, bracketed: true, bytes: 1, is_main: true, shell: false, agent: true };
        assert!(send_refusal(&t).is_some_and(|r| r.contains("password")));
        let src = include_str!("app/code_review_app.rs");
        assert!(src.contains("password_input"), "the app reads the pane's password state");
    }

    /// 13y1 Task 7 item 4: an auto-send needs the coding agent the review was started for in front of the
    /// main session: the same program, the same process. A shell prompt (bash 5.1+, zsh and fish turn
    /// bracketed paste on there too) is refused with its own banner, as is any other program or a new run.
    #[test]
    fn auto_send_never_into_a_shell_prompt() {
        let ok = SendTarget { alive: true, password: false, bracketed: true, bytes: 100, is_main: true, shell: false, agent: true };
        let claude = agent_of(Some(("claude.exe", 40))).expect("an agent");
        assert_eq!(claude, Agent { name: "claude".into(), pid: 40 });
        for shell in ["bash", "-zsh", "fish", "pwsh.exe", "cmd.exe", "powershell", "/usr/bin/bash"] {
            assert_eq!(agent_of(Some((shell, 41))), None, "{shell}");
            assert!(!agent_present(Some(&claude), Some((shell, 41))), "{shell}: the agent has exited to its shell");
        }
        assert!(agent_present(Some(&claude), Some(("Claude", 40))));
        assert!(!agent_present(Some(&claude), Some(("claude", 41))), "a new run is not the agent the review was for");
        assert!(!agent_present(Some(&claude), Some(("vim", 40))));
        assert!(!agent_present(Some(&claude), None), "no job known");
        assert!(!agent_present(None, Some(("claude", 40))), "a review started while the main session was at a shell sends nothing");
        let at_shell = send_refusal(&SendTarget { shell: true, agent: false, ..ok });
        assert!(at_shell.is_some_and(|r| r.contains("shell prompt")), "{at_shell:?}");
        let gone = send_refusal(&SendTarget { agent: false, ..ok });
        assert!(gone.is_some_and(|r| r.contains("coding agent")), "{gone:?}");
        assert!(send_refusal(&SendTarget { shell: true, agent: true, ..ok }).is_some(), "a shell is never the agent");
        // The app checks both before the paste and again before the Return.
        let app = include_str!("app/code_review_app.rs");
        let live = app.split("\n#[cfg(test)]").next().unwrap();
        let target = &live[live.find("fn review_send_target(").unwrap()..];
        assert!(target.contains("cr::agent_present(") && target.contains("crate::session_state::is_shell("), "{target}");
        let paste = include_str!("app/paste_ui.rs");
        let tick = &paste[paste.find("fn paste_after_tick").unwrap()..paste.find("pub(super) fn paste_tick").unwrap()];
        assert!(tick.find("cr::send_refusal(&self.review_send_target(").expect("re-checked before the Return") < tick.find("self.user_write(").unwrap());
    }

    /// 13y1 Task 7 item 5 (P10): the bounds apply at every read, so a value written by any path (remote
    /// prefs, the Stored tab) reaches the orchestration prompt, the pulldown and the manager bounded; a
    /// read is stable (a replaced uuid is the same each time).
    #[test]
    fn saved_prompts_are_bounded_at_every_read() {
        let mut g = crate::globals::Globals::empty(&std::env::temp_dir().join("tandem-cr-bounds-never-saved.toml"));
        let many: Vec<SavedPrompt> = (0..200)
            .map(|i| SavedPrompt {
                uuid: if i == 1 { "bad".into() } else { format!("{i:08X}-0000-0000-0000-000000000000") },
                name: format!("n{i}\u{202e}\x1b[31m{}", "x".repeat(300)),
                text: "t".repeat(if i == 0 { 1_000_000 } else { 1 }),
            })
            .collect();
        g.set(KEY, toml::Value::String(encode_prompts(&many)));
        let read = prompts_of(&g);
        assert_eq!(read.len(), MAX_PROMPTS);
        for p in &read {
            assert!(p.text.len() <= MAX_TEXT && p.name.chars().count() <= MAX_NAME && uuid_ok(&p.uuid), "{:?}", p.name);
            assert!(!p.name.contains('\u{202e}') && !p.name.contains('\x1b'), "{:?}", p.name);
        }
        assert_eq!(prompts_of(&g), read, "a read is stable");
        let vars = crate::orch::prompt_vars(&read, None);
        assert!(vars[0].1.len() <= MAX_TEXT && !vars[1].1.contains('\u{202e}'));
        let (decoded, seeded) = load_prompts(Some(&encode_prompts(&many)), "");
        assert_eq!((decoded.len(), seeded), (MAX_PROMPTS, false), "the decode is the bound");
    }

    /// B6: what is pasted passes the paste path's own transforms: controls and ESC removed, newlines
    /// as CR, and the one bracket the pane asked for.
    #[test]
    fn auto_send_strips_controls_and_escape() {
        let job = auto_job("finding\x1b[200~ \x1b]0;x\x07 \x03 ok\nnext\ttab");
        assert_eq!(job.flags, crate::paste::NEWLINES | crate::paste::CONTROLS | crate::paste::BRACKET | crate::paste::PLAIN);
        let text = crate::paste::sanitize(&job, &crate::paste::Prefs::default());
        let bracketed = crate::paste::bracket(&text, false);
        let inner = bracketed.strip_prefix("\x1b[200~").and_then(|r| r.strip_suffix("\x1b[201~")).expect("one bracket");
        assert!(!inner.contains('\x1b') && !inner.contains('\x07') && !inner.contains('\x03'), "{inner:?}");
        assert!(inner.contains("next") && !inner.contains('\n'), "{inner:?}");
        let src = include_str!("app/code_review_app.rs");
        let live = src.split("\n#[cfg(test)]").next().unwrap();
        assert!(live.contains("paste_auto("), "the app pastes through the paste path");
        assert!(!live.contains("paste_ask_next") && !live.contains("paste_job("), "an auto path cannot answer a dialog");
        let paste = include_str!("app/paste_ui.rs");
        assert!(paste.contains("fn paste_auto") && paste.contains("self.paste_run(sender, job,"));
    }

    /// `CRS:64-92`, `CRS:131-137`, `ChatAgent.swift:989-999`.
    #[test]
    fn saved_prompts_follow_iterm2() {
        const U: &str = "123E4567-E89B-12D3-A456-426614174000";
        let (list, seeded) = load_prompts(Some(&format!(r#"[{{"uuid":"{U}","name":"N","text":"T"}}]"#)), "legacy");
        assert_eq!((list, seeded), (vec![SavedPrompt { uuid: U.into(), name: "N".into(), text: "T".into() }], false));
        // A value using `body` fails the whole decode, as iTerm2's JSONDecoder does.
        let (list, seeded) = load_prompts(Some(r#"[{"uuid":"U","name":"N","text":"T"},{"uuid":"V","name":"M","body":"B"}]"#), "legacy");
        assert!(seeded);
        assert_eq!((list.len(), list[0].name.as_str(), list[0].text.as_str()), (1, "Default", "legacy"));
        assert_eq!(list[0].uuid.len(), 36);
        assert_eq!(list[0].uuid, list[0].uuid.to_uppercase(), "an upper-case uuid like UUID().uuidString");
        for stored in [None, Some(""), Some("   "), Some("[{"), Some("{}"), Some("null")] {
            let (list, seeded) = load_prompts(stored, "L");
            assert!(seeded, "{stored:?}");
            assert_eq!((list.len(), list[0].name.as_str(), list[0].text.as_str()), (1, "Default", "L"), "{stored:?}");
        }
        // An empty list decodes: nothing is seeded.
        assert_eq!(load_prompts(Some("[]"), "L"), (vec![], false));
        // Only a missing or empty value is written back; a malformed one stays on disk until the first edit (P10).
        assert!(should_persist_seed(None) && should_persist_seed(Some("")) && !should_persist_seed(Some("[{")));
        let two = vec![SavedPrompt { uuid: "A".into(), name: "A".into(), text: "ta".into() }, SavedPrompt { uuid: "B".into(), name: "B".into(), text: "tb".into() }];
        assert_eq!(default_prompt_text(&two, Some("B")), "tb");
        assert_eq!(default_prompt_text(&two, Some("gone")), "ta", "the first when the last selected is gone");
        assert_eq!(default_prompt_text(&two, None), "ta");
        assert_eq!(default_prompt_text(&[], Some("B")), "");
        assert_eq!(saved_prompt_names(&[]), "(none saved)");
        assert_eq!(saved_prompt_names(&two), "  - A\n  - B");
        let json = encode_prompts(&two);
        let kept = vec![SavedPrompt { uuid: U.into(), name: "A".into(), text: "ta".into() }];
        assert_eq!(load_prompts(Some(&encode_prompts(&kept)), "").0, kept, "what is written is what is read");
        assert_eq!(load_prompts(Some(&json), "").0.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["ta", "tb"], "a uuid of the wrong shape is replaced at the read");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0], serde_json::json!({"uuid": "A", "name": "A", "text": "ta"}));
    }

    /// P10: the write point bounds the stored list.
    #[test]
    fn saved_prompts_are_bounded_at_the_write() {
        let many: Vec<SavedPrompt> = (0..70).map(|i| SavedPrompt { uuid: format!("{i:08X}-0000-0000-0000-000000000000"), name: format!("n{i}"), text: "t".into() }).collect();
        assert_eq!(bounded(many).len(), MAX_PROMPTS);
        let odd = bounded(vec![SavedPrompt { uuid: "not a uuid".into(), name: format!("a\u{202e}b{}", "x".repeat(200)), text: "y".repeat(70 * 1024) }]);
        assert_eq!(odd[0].name.chars().count(), MAX_NAME);
        assert!(!odd[0].name.contains('\u{202e}'), "no bidi override in a name the menu shows");
        assert_eq!(odd[0].text.len(), MAX_TEXT);
        assert!(uuid_ok(&odd[0].uuid) && odd[0].uuid != "not a uuid");
        assert!(uuid_ok("123E4567-E89B-12D3-A456-426614174000") && !uuid_ok("x") && !uuid_ok(&"A".repeat(37)));
    }

    /// 12f's orchestration prompt reads the saved prompts (R13xy-11).
    #[test]
    fn orchestration_variables_carry_the_saved_prompts() {
        let two = vec![SavedPrompt { uuid: "A".into(), name: "A".into(), text: "ta".into() }, SavedPrompt { uuid: "B".into(), name: "B".into(), text: "tb".into() }];
        assert_eq!(crate::orch::prompt_vars(&two, Some("B")), [("default_code_review_prompt", "tb".to_string()), ("saved_prompt_names", "  - A\n  - B".to_string())]);
        assert_eq!(crate::orch::prompt_vars(&[], None), [("default_code_review_prompt", String::new()), ("saved_prompt_names", "(none saved)".to_string())]);
    }
}
