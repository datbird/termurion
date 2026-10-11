//! Browser downloads (13x8 Task 7; iTerm2 `BM:1573-1579`, `DL:66-189`): the name and place of a
//! file WebView2 saves for a page, the progress text, the closed set of actions the downloads page
//! may send and the page itself. Pure: the App decides and acts (`app::browser_download_app`), the
//! web view only writes where it is told (`webhost_win`). A name from a server or a page is
//! untrusted: it goes through `inline_file::clean_name` (one path component, no stream, no
//! reserved name, no bidi control) and `inert_files::neutralized` (the one dangerous-type list),
//! and nothing is ever opened or run from here.

use crate::layout::PaneId;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 14d's banner key for a transfer in progress.
pub const BANNER_KEY: &str = "transfer";
/// The downloads page: its name in `browser_pages::page_name`, its URL and its message handler.
pub const PAGE: &str = "downloads";
pub const PAGE_URL: &str = "iterm2-about:downloads";
pub const HANDLER: &str = "iterm2-about:downloads";
/// The longest file name kept (characters), before the `.download` suffix and a number.
pub const MAX_NAME: usize = 150;
/// Downloads one pane may have in flight, and the quiet time after one starts (a page cannot start
/// a flood; 13x6: repeat automatic downloads are denied).
pub const MAX_PER_PANE: usize = 3;
pub const MAX_IN_FLIGHT: usize = 8;
pub const REPEAT_MS: u64 = 3_000;
/// Rows kept in the list (finished ones fall off the old end).
pub const MAX_ROWS: usize = 200;
/// Longest source URL a Mark of the Web carries.
const MAX_MARK_URL: usize = 2048;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Running,
    Finished,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub pane: PaneId,
    pub url: String,
    pub name: String,
    /// Where the finished file is (the place it was told at the start until it is moved).
    pub path: PathBuf,
    pub received: u64,
    pub total: Option<u64>,
    /// The hidden `.download` file WebView2 writes while the download runs.
    pub temp: PathBuf,
    /// The last progress step shown (`progress_step`).
    pub step: u64,
    pub phase: Phase,
}

/// The URL's last path component, percent-decoded, as one safe file name; "download" when there is
/// none or `clean_name` only has its "Unnamed file" fallback. Cut to `MAX_NAME` characters keeping
/// the type (the text after the last dot, up to 16 characters).
pub fn suggested_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = match path.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, p)| p),
        None => path.rsplit_once(':').map_or(path, |(_, p)| p),
    };
    let last = path.rsplit('/').next().unwrap_or("");
    let name = crate::inline_file::clean_name(&percent_decode(last));
    if name == "Unnamed file" || name.is_empty() {
        return "download".to_string();
    }
    cut(&name)
}

fn cut(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= MAX_NAME {
        return name.to_string();
    }
    let ext_len = name.rfind('.').map(|i| name[i..].chars().count()).filter(|n| *n <= 16).unwrap_or(0);
    let head: String = chars[..MAX_NAME - ext_len].iter().collect();
    let tail: String = chars[chars.len() - ext_len..].iter().collect();
    let joined = crate::inline_file::clean_name(&format!("{head}{tail}"));
    if joined == "Unnamed file" { "download".to_string() } else { joined }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The name a download keeps: the suggested name, a dangerous type renamed with `.download`.
pub fn kept_name(url: &str) -> String {
    crate::inert_files::neutralized(&suggested_name(url))
}

/// The URL's host, else the whole URL (`DL:66-68`).
pub fn host_of(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else { return url.to_string() };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = auth.rsplit('@').next().unwrap_or(auth);
    let host = if let Some(v6) = host.strip_prefix('[') { v6.split(']').next().unwrap_or(v6) } else { host.split(':').next().unwrap_or(host) };
    if host.is_empty() { url.to_string() } else { host.to_string() }
}

/// 14d's progress form: "Downloading a.txt from h: 25%", or "... : 512 bytes" with no total.
pub fn progress_text(name: &str, host: &str, received: u64, total: Option<u64>) -> String {
    match total.filter(|t| *t > 0) {
        Some(t) => format!("Downloading {name} from {host}: {}%", (received.min(t) * 100) / t),
        None => format!("Downloading {name} from {host}: {received} bytes"),
    }
}

/// The banner is rewritten at most every 10 percent (every MiB with no total): the step a count is in.
pub fn progress_step(received: u64, total: Option<u64>) -> u64 {
    match total.filter(|t| *t > 0) {
        Some(t) => (received.min(t) * 10) / t,
        None => received >> 20,
    }
}

/// Whether `path` is a file directly in the Downloads folder: the canonical form of its parent is
/// the canonical Downloads folder, and its name is one component equal to `clean_name` of itself.
/// False when either cannot be canonicalized (a junction or link out of the folder, `..`, a
/// subfolder, a device name).
pub fn download_path_ok(downloads: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    if name != crate::inline_file::clean_name(name) {
        return false;
    }
    let (Some(parent), Some(dir)) = (path.parent(), Some(downloads)) else { return false };
    let canon = |p: &Path| crate::localfs::judge(p).and_then(|l| crate::localfs::canonicalize(&l));
    match (canon(parent), canon(dir)) {
        (Some(a), Some(b)) => a.path() == b.path(),
        _ => false,
    }
}

/// The hidden file WebView2 writes while a download runs: ends in `.download`, so it is inert at
/// every step, and `ssh_local::sweep_stale` finds it if Tandem dies.
pub fn temp_path(dir: &Path, uuid: &str) -> PathBuf {
    dir.join(format!("{}{}", crate::ssh_local::temp_name(uuid), crate::inert_files::NEUTRAL_SUFFIX))
}

/// The source address a Mark of the Web may carry: http or https, bounded, no control character.
pub fn mark_url(url: &str) -> Option<String> {
    let low = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    ((low.starts_with("http://") || low.starts_with("https://")) && url.len() <= MAX_MARK_URL && !url.chars().any(|c| c.is_control())).then(|| url.to_string())
}

/// The place a download will be finished at: a free name in `dir` (never asks: `prompt: false`),
/// refused when its type is one the shell reads on listing, and checked to sit directly in `dir`.
pub fn destination(dir: &Path, url: &str, exists: &dyn Fn(&Path) -> bool) -> Result<PathBuf, &'static str> {
    let dest = crate::ssh_local::final_destination(dir, &kept_name(url), false, exists).0;
    if crate::inert_files::refused_write(&dest.to_string_lossy()) {
        return Err(crate::ssh_local::NOT_ALLOWED);
    }
    if !download_path_ok(dir, &dest) {
        return Err(crate::ssh_local::NOT_ALLOWED);
    }
    Ok(dest)
}

/// What the app does with a download that is starting.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Allow,
    /// Denied, with the banner text.
    Deny(&'static str),
}

pub const DEV_NULL_TEXT: &str = "Downloads are off in a Dev Null tab: it writes nothing to disk. Use a normal browser tab.";
pub const TOO_MANY_TEXT: &str = "Too many downloads at once: this one was blocked.";
pub const REPEAT_TEXT: &str = "A page tried to start another download right away: it was blocked.";

/// Whether a download may start: not in a Dev Null pane, not past the in-flight caps, and not a
/// repeat from the same pane inside `REPEAT_MS`.
pub fn verdict(dev_null: bool, pane_in_flight: usize, all_in_flight: usize, repeat_quiet: bool) -> Verdict {
    if dev_null {
        Verdict::Deny(DEV_NULL_TEXT)
    } else if pane_in_flight >= MAX_PER_PANE || all_in_flight >= MAX_IN_FLIGHT {
        Verdict::Deny(TOO_MANY_TEXT)
    } else if repeat_quiet {
        Verdict::Deny(REPEAT_TEXT)
    } else {
        Verdict::Allow
    }
}

// --- The downloads page ---

/// What the downloads page may ask, by download id. Nothing else, and no path or name comes from it.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    List,
    ShowInFolder(u64),
    Cancel(u64),
    Remove(u64),
}

/// A message body typed: exactly `action` (and `id` for the three that name a download).
pub fn parse(body: &serde_json::Map<String, Value>) -> Option<Msg> {
    let action = body.get("action")?.as_str()?;
    let id = || body.get("id").and_then(Value::as_u64).filter(|n| *n < (1 << 53));
    let only = |keys: &[&str]| body.keys().all(|k| keys.contains(&k.as_str()));
    Some(match action {
        "list" if only(&["action"]) => Msg::List,
        "showInFolder" if only(&["action", "id"]) => Msg::ShowInFolder(id()?),
        "cancel" if only(&["action", "id"]) => Msg::Cancel(id()?),
        "remove" if only(&["action", "id"]) => Msg::Remove(id()?),
        _ => return None,
    })
}

/// The shown form of a row: names and hosts go through `display_text`, bounded.
pub fn row_json(id: u64, d: &Download) -> Value {
    let (state, detail) = match &d.phase {
        Phase::Running => ("running", progress_text(&crate::inert_files::display_text(&d.name, 200, false), &crate::inert_files::display_text(&host_of(&d.url), 100, false), d.received, d.total)),
        Phase::Finished => ("finished", String::new()),
        Phase::Failed(why) => ("failed", crate::inert_files::display_text(why, 200, false)),
        Phase::Cancelled => ("cancelled", String::new()),
    };
    serde_json::json!({
        "id": id,
        "name": crate::inert_files::display_text(&d.name, 200, false),
        "host": crate::inert_files::display_text(&host_of(&d.url), 100, false),
        "state": state,
        "detail": detail,
    })
}

/// The page, under the nonce policy (`browser_pages::response_headers`). It draws every value with
/// `textContent`, sends only the closed actions above, and ignores a click a script made up.
pub fn document(nonce: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"UTF-8\"><title>Downloads</title>\n\
<style>:root{{--bg:#fff;--fg:#1d1d1f;--dim:#86868b;--line:#d2d2d7}}@media (prefers-color-scheme: dark){{:root{{--bg:#1c1c1e;--fg:#fff;--dim:#8e8e93;--line:#38383a}}}}\
body{{font-family:'Segoe UI',system-ui,sans-serif;background:var(--bg);color:var(--fg);padding:32px 20px}}.c{{max-width:760px;margin:0 auto}}\
h1{{font-size:24px;margin-bottom:16px}}.row{{border-bottom:1px solid var(--line);padding:10px 0;display:flex;gap:12px;align-items:center}}\
.t{{flex:1;min-width:0}}.n{{font-weight:600;overflow-wrap:anywhere}}.d{{color:var(--dim);font-size:13px}}button{{margin-left:6px}}#none{{color:var(--dim)}}</style></head>\n\
<body><div class=\"c\"><h1>Downloads</h1><div id=\"rows\"></div><div id=\"none\">No downloads.</div></div>\n\
<script nonce=\"{nonce}\">\n\
function send(body){{ window.webkit.messageHandlers['iterm2-about:downloads'].postMessage(body); }}\n\
function button(label, action, id){{ var b=document.createElement('button'); b.textContent=label; b.setAttribute('data-action',action); b.setAttribute('data-id',String(id)); return b; }}\n\
window.onDownloads=function(list){{ var rows=document.getElementById('rows'); rows.textContent='';\n\
 document.getElementById('none').style.display=list.length?'none':'block';\n\
 list.forEach(function(r){{ var row=document.createElement('div'); row.className='row'; var t=document.createElement('div'); t.className='t';\n\
  var n=document.createElement('div'); n.className='n'; n.textContent=String(r.name); var d=document.createElement('div'); d.className='d';\n\
  d.textContent=r.state==='running'?String(r.detail):(r.state==='finished'?'Finished, from '+String(r.host):(r.state==='failed'?'Failed: '+String(r.detail):'Cancelled'));\n\
  t.appendChild(n); t.appendChild(d); row.appendChild(t);\n\
  if(r.state==='running'){{ row.appendChild(button('Cancel','cancel',r.id)); }} else {{ if(r.state==='finished'){{ row.appendChild(button('Show in folder','showInFolder',r.id)); }} row.appendChild(button('Remove','remove',r.id)); }}\n\
  rows.appendChild(row); }}); }};\n\
document.addEventListener('click',function(e){{ if(!e.isTrusted||!e.target||!e.target.closest){{return;}} var b=e.target.closest('[data-action]'); if(!b){{return;}}\n\
 send({{action:b.getAttribute('data-action'),id:Number(b.getAttribute('data-id'))}}); }});\n\
send({{action:'list'}});\n\
</script></body></html>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tandem-dl-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn download_names_follow_iterm2() {
        assert_eq!(suggested_name("https://h/x/file%20a.zip?x=1"), "file a.zip");
        assert_eq!(suggested_name("https://h/"), "download");
        assert_eq!(suggested_name("https://h"), "download");
        assert_eq!(host_of("https://h.example/x"), "h.example");
        assert_eq!(host_of("data:text/plain,x"), "data:text/plain,x");
        for (url, want) in [
            ("https://h/..%2F..%2Fx.exe", "x.exe"),
            ("https://h/..%5C..%5CStartup%5Cx.exe", "x.exe"),
            ("https://h/a.txt%3Aads", "a.txt_ads"),
            ("https://h/a.txt%3AZone.Identifier%3A%24DATA", "a.txt_Zone.Identifier_$DATA"),
            ("https://h/CON", "_CON"),
            ("https://h/nul.txt", "_nul.txt"),
            ("https://h/COM1.zip", "_COM1.zip"),
            ("https://h/CONIN%24", "_CONIN$"),
            ("https://h/%2E%2E", "download"),
            ("https://h/a%00b", "a_b"),
            ("https://h/x.%20.%20", "x"),
            ("https://h/a%E2%80%AEfdp.exe", "afdp.exe"),
        ] {
            let got = suggested_name(url);
            assert_eq!(got, want, "{url}");
            assert!(!got.contains(['/', '\\', ':']), "{got:?} is one component");
        }
        // A dangerous type is renamed by the one list, double extensions included.
        assert_eq!(kept_name("https://h/setup.exe"), "setup.exe.download");
        assert_eq!(kept_name("https://h/report.pdf.exe"), "report.pdf.exe.download");
        assert_eq!(kept_name("https://h/x.url"), "x.url.download");
        assert_eq!(kept_name("https://h/a.exe.txt"), "a.exe.txt");
        // A very long name is cut keeping its type.
        let long = format!("https://h/{}.zip", "a".repeat(400));
        let cut = suggested_name(&long);
        assert!(cut.chars().count() <= MAX_NAME && cut.ends_with(".zip"), "{cut}");
        let d = temp_dir("names");
        std::fs::write(d.join("a.zip"), b"x").unwrap();
        let exists = |p: &Path| p.exists();
        let (free, _) = crate::ssh_local::final_destination(&d, "a.zip", false, &exists);
        assert_eq!(free, d.join("a (1).zip"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn download_paths_stay_in_downloads() {
        let (d, other) = (temp_dir("in"), temp_dir("out"));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        assert!(download_path_ok(&d, &d.join("a.zip")));
        assert!(!download_path_ok(&d, &d.join("sub").join("a.zip")));
        assert!(!download_path_ok(&d, &d.join("..").join("a.zip")));
        assert!(!download_path_ok(&d, &other.join("a.zip")));
        assert!(!download_path_ok(&d, &d.join("CON")));
        assert!(!download_path_ok(&d, &d.join("a:b")));
        assert!(!download_path_ok(&d.join("missing"), &d.join("missing").join("a.zip")));
        // destination: free name, refused types, nothing outside.
        let exists = |p: &Path| p.exists();
        assert_eq!(destination(&d, "https://h/a.zip", &exists), Ok(d.join("a.zip")));
        assert_eq!(destination(&d, "https://h/setup.exe", &exists), Ok(d.join("setup.exe.download")));
        assert_eq!(destination(&d, "https://h/desktop.ini", &exists), Err(crate::ssh_local::NOT_ALLOWED));
        #[cfg(windows)]
        {
            let junction = d.join("j");
            let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&other).output().map(|o| o.status.success()).unwrap_or(false);
            if made {
                assert!(!download_path_ok(&d, &junction.join("a.zip")));
                let _ = std::fs::remove_dir(&junction);
            } else {
                eprintln!("note: junction could not be made, that case is skipped");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn download_progress_follows_iterm2() {
        assert_eq!(progress_text("a.zip", "h", 512, Some(2048)), "Downloading a.zip from h: 25%");
        assert_eq!(progress_text("a.zip", "h", 512, None), "Downloading a.zip from h: 512 bytes");
        assert_eq!(crate::ssh_local::download_finished("a.zip"), "Download finished for \u{201c}a.zip\u{201d}.");
        assert_eq!(crate::ssh_local::NO_DOWNLOADS, "Unable to find Downloads folder");
        assert_eq!(BANNER_KEY, "transfer");
        // At most every 10 percent.
        assert_eq!(progress_step(0, Some(1000)), 0);
        assert_eq!(progress_step(99, Some(1000)), 0);
        assert_eq!(progress_step(100, Some(1000)), 1);
        assert_eq!(progress_step(5000, Some(1000)), 10);
        assert_eq!(progress_step(3 << 20, None), 3);
    }

    #[test]
    fn a_page_cannot_start_a_flood() {
        assert_eq!(verdict(false, 0, 0, false), Verdict::Allow);
        assert_eq!(verdict(true, 0, 0, false), Verdict::Deny(DEV_NULL_TEXT));
        assert_eq!(verdict(false, MAX_PER_PANE, 0, false), Verdict::Deny(TOO_MANY_TEXT));
        assert_eq!(verdict(false, 0, MAX_IN_FLIGHT, false), Verdict::Deny(TOO_MANY_TEXT));
        assert_eq!(verdict(false, 1, 1, true), Verdict::Deny(REPEAT_TEXT));
        assert!(mark_url("https://h/x").is_some() && mark_url("blob:https://h/x").is_none() && mark_url("javascript:1").is_none());
        assert!(mark_url("https://h/x\r\nHostUrl=evil").is_none());
        assert!(temp_path(Path::new("D:\\dl"), "u").to_string_lossy().ends_with(".download"));
    }

    #[test]
    fn the_downloads_page_sends_a_closed_set() {
        let p = |s: &str| match serde_json::from_str::<Value>(s).unwrap() {
            Value::Object(m) => parse(&m),
            _ => None,
        };
        assert_eq!(p(r#"{"action":"list"}"#), Some(Msg::List));
        assert_eq!(p(r#"{"action":"cancel","id":3}"#), Some(Msg::Cancel(3)));
        assert_eq!(p(r#"{"action":"showInFolder","id":3}"#), Some(Msg::ShowInFolder(3)));
        assert_eq!(p(r#"{"action":"remove","id":3}"#), Some(Msg::Remove(3)));
        for bad in [r#"{"action":"open","id":3}"#, r#"{"action":"cancel"}"#, r#"{"action":"cancel","id":-1}"#, r#"{"action":"cancel","id":"3"}"#, r#"{"action":"list","path":"C:\\x.exe"}"#, r#"{"action":"remove","id":3,"path":"x"}"#] {
            assert_eq!(p(bad), None, "{bad}");
        }
        let d = Download { pane: 1, url: "https://h/x".into(), name: "a\u{202e}b.txt".into(), path: PathBuf::new(), received: 1, total: None, temp: PathBuf::new(), step: 0, phase: Phase::Running };
        assert_eq!(row_json(1, &d)["name"], "ab.txt");
        let html = document("NONCE1");
        assert!(html.contains("nonce=\"NONCE1\"") && !html.contains("innerHTML") && html.contains("isTrusted"));
    }
}
