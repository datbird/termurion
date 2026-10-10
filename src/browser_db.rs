//! The browser's history and bookmarks (milestone 13x, Task 7): `browserdb-<user>.sqlite` in
//! iTerm2's tables (`BrowserHistory`, `BrowserBookmarks`, `BrowserVisits`; `BrowserHistory.swift`,
//! `BrowserBookmarks.swift`, `BrowserVisits.swift`) over 12c's `rusqlite`. Everything a page
//! chose (a URL, a title) is untrusted: it is bounded and cleaned at the one write point
//! (`record_visit`, `update_title`, `add_bookmark`) and read back only through bound parameters.

use crate::localfs::{self, LocalPath};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// The three tables and their indexes, verbatim (`BrowserHistory.swift:41-56`,
/// `BrowserBookmarks.swift:34-43`, `BrowserVisits.swift:55-71`). The tag, named-mark, permission and
/// key-value tables are not created (Decision).
pub const SCHEMA: &str = "
create table if not exists BrowserHistory
    (id text primary key,
     url text not null,
     title text,
     visitDate integer not null,
     sessionGuid text,
     referrerUrl text,
     transitionType integer not null);
CREATE INDEX IF NOT EXISTS idx_browser_history_url ON BrowserHistory(url);
CREATE INDEX IF NOT EXISTS idx_browser_history_date ON BrowserHistory(visitDate DESC);
CREATE INDEX IF NOT EXISTS idx_browser_history_session ON BrowserHistory(sessionGuid);
CREATE INDEX IF NOT EXISTS idx_browser_history_title ON BrowserHistory(title);
create table if not exists BrowserBookmarks
    (url text primary key,
     title text,
     dateAdded integer not null);
CREATE INDEX IF NOT EXISTS idx_browser_bookmarks_title ON BrowserBookmarks(title);
CREATE INDEX IF NOT EXISTS idx_browser_bookmarks_date ON BrowserBookmarks(dateAdded DESC);
create table if not exists BrowserVisits
    (hostname text not null,
     path text not null,
     visitCount integer not null,
     lastVisitDate integer not null,
     firstVisitDate integer not null,
     title text,
     url text,
     PRIMARY KEY (hostname, path));
CREATE INDEX IF NOT EXISTS idx_browser_visits_hostname ON BrowserVisits(hostname);
CREATE INDEX IF NOT EXISTS idx_browser_visits_path ON BrowserVisits(path);
CREATE INDEX IF NOT EXISTS idx_browser_visits_count ON BrowserVisits(visitCount DESC);
CREATE INDEX IF NOT EXISTS idx_browser_visits_url ON BrowserVisits(url);
";

/// A URL longer than this (bytes) is not stored; a title is cut to `MAX_TITLE` characters (P10).
pub const MAX_URL: usize = 2048;
pub const MAX_TITLE: usize = 512;
/// The most rows any query returns, and the most search terms of eight characters or fewer it reads.
pub const MAX_LIMIT: usize = 500;
const MAX_OFFSET: usize = 1_000_000;
const MAX_TERMS: usize = 8;
const MAX_TERM: usize = 128;

/// The most rows each table keeps (P10). A full history drops its oldest rows; a full bookmark
/// table refuses the new one (nothing the person kept is deleted).
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub history: i64,
    pub visits: i64,
    pub bookmarks: i64,
    /// Named marks (13x5, B8): a full table refuses the new one.
    pub named_marks: i64,
}

pub const CAPS: Caps = Caps { history: 100_000, visits: 100_000, bookmarks: 20_000, named_marks: 20_000 };

/// One row of the history page.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub visit_date: i64,
    pub transition: i64,
}

/// One bookmark row of the bookmarks page.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRow {
    pub url: String,
    pub title: Option<String>,
    pub date_added: i64,
}

/// One most-visited site of the welcome page (`topVisitedUrls`).
#[derive(Clone, Debug, PartialEq)]
pub struct TopSite {
    pub url: String,
    pub title: Option<String>,
    pub hostname: String,
    pub visits: i64,
}

/// `<LOCALAPPDATA>\Tandem\browserdb-<user>.sqlite` (P10: machine-local, WAL on a roaming profile
/// corrupts and would sync history across machines; iTerm2's name, `BD:15-26`).
pub fn path(local_appdata: &Path, user: &str) -> PathBuf {
    local_appdata.join("Tandem").join(format!("browserdb-{user}.sqlite"))
}

/// Query parameters iTerm2 removes before it counts a visit (`BrowserVisits.parseUrl`).
const TRACKING: [&str; 8] = ["utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content", "fbclid", "gclid", "ref"];

/// A URL's host (lowercase, with a period in front for domain matching) and path with its query,
/// as iTerm2 counts visits (`BrowserVisits.parseUrl`): no fragment, no user information or port,
/// no tracking parameter. Text with no authority gives the period alone and the rest as the path.
pub fn host_and_path(url: &str) -> (String, String) {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return (url.to_string(), String::new());
    }
    let no_fragment = url.split('#').next().unwrap_or(url);
    let (before_query, query) = match no_fragment.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (no_fragment, None),
    };
    // The part after the scheme: `//authority/path` or an opaque rest.
    let after_scheme = match before_query.split_once(':') {
        Some((scheme, rest)) if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) => rest,
        _ => before_query,
    };
    let (host, path) = match after_scheme.strip_prefix("//") {
        Some(rest) => {
            let end = rest.find('/').unwrap_or(rest.len());
            let authority = &rest[..end];
            let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let host = if let Some(inner) = hostport.strip_prefix('[') {
                inner.split_once(']').map_or(inner, |(h, _)| h)
            } else {
                match hostport.rsplit_once(':') {
                    Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => h,
                    _ => hostport,
                }
            };
            (host.to_ascii_lowercase(), rest[end..].to_string())
        }
        None => (String::new(), after_scheme.to_string()),
    };
    let mut path = path;
    if let Some(q) = query {
        let kept: Vec<&str> = q.split('&').filter(|item| !item.is_empty() && !TRACKING.contains(&item.split('=').next().unwrap_or("").to_ascii_lowercase().as_str())).collect();
        if !kept.is_empty() {
            path.push('?');
            path.push_str(&kept.join("&"));
        }
    }
    (format!(".{host}"), path)
}

/// The URL as stored: not over `MAX_URL` bytes, no control character, no `user:password@`.
/// None when it cannot be stored.
fn clean_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL || url.chars().any(char::is_control) {
        return None;
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        if !scheme.is_empty() && !scheme.contains('/') {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            if let Some((_, hostport)) = rest[..end].rsplit_once('@') {
                return Some(format!("{scheme}://{hostport}{}", &rest[end..]));
            }
        }
    }
    Some(url.to_string())
}

/// A title as stored: control, bidirectional and zero width characters gone (the one display
/// sanitizer), at most `MAX_TITLE` characters, None when nothing is left.
fn clean_title(title: &str) -> Option<String> {
    let t = crate::inert_files::display_text(title, MAX_TITLE, false);
    (!t.is_empty()).then_some(t)
}

/// `text` as a `LIKE` pattern for "contains", the pattern characters taken literally.
fn like_contains(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// A search text's terms: whitespace separated, at most `MAX_TERMS`, each cut to `MAX_TERM`.
fn terms_of(text: &str) -> Vec<String> {
    text.split_whitespace().take(MAX_TERMS).map(|t| t.chars().take(MAX_TERM).collect()).collect()
}

fn lim(n: usize) -> i64 {
    n.min(MAX_LIMIT) as i64
}

fn off(n: usize) -> i64 {
    n.min(MAX_OFFSET) as i64
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Schemas later plans add (each plan's tables, verbatim from iTerm2's `Database/*.swift`), run in
/// order after `SCHEMA` by `open` and `open_in_memory`.
pub const EXTRA_SCHEMAS: &[&str] = &[crate::browser_marks_db::SCHEMA];

fn bring_up(conn: &Connection, extra: &[&str]) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(err)?;
    for schema in extra {
        conn.execute_batch(schema).map_err(err)?;
    }
    Ok(())
}

pub struct Db {
    conn: Connection,
    caps: Caps,
}

impl Db {
    /// Opens (making it, owner-only, when absent) the database at `path`: the path is judged local
    /// first, a file that is not a database is set aside as `.bad` and replaced, and the schema is
    /// brought up (P10). The error text can name a path: callers show only that it failed.
    pub fn open(path: &Path) -> Result<Db, String> {
        let judged = LocalPath::judge(path).ok_or_else(|| "the browser database is not on a local disk".to_string())?;
        let dir = judged.parent().ok_or_else(|| "no folder".to_string())?;
        localfs::create_dir_all(&dir).map_err(err)?;
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| "no file name".to_string())?.to_string();
        match Db::open_file(&dir, &name) {
            Ok(db) => Ok(db),
            Err(first) => {
                // Not a database (or damaged): keep it for a person to look at, start again.
                let (Some(file), Some(bad)) = (dir.unfollowed(&name), dir.unfollowed(format!("{name}.bad"))) else { return Err(first) };
                let _ = localfs::remove_unfollowed(&bad);
                localfs::rename_unfollowed(&file, &bad).map_err(err)?;
                if let Some(journal) = dir.unfollowed(format!("{name}-journal")) {
                    let _ = localfs::remove_unfollowed(&journal);
                }
                Db::open_file(&dir, &name)
            }
        }
    }

    fn open_file(dir: &LocalPath, name: &str) -> Result<Db, String> {
        let file = dir.unfollowed(name).ok_or_else(|| "bad file name".to_string())?;
        let judged = dir.child(name).ok_or_else(|| "the database file is a link to somewhere not local".to_string())?;
        if !localfs::exists(&judged) {
            #[cfg(windows)]
            drop(localfs::create_owner_only(&file).map_err(err)?);
        }
        let _ = &file;
        // A file with another header is not ours to open: SQLite would take a short one for a
        // fresh database and write over it.
        if localfs::exists(&judged) {
            use std::io::Read;
            let mut head = [0u8; 16];
            let mut f = localfs::open(&judged).map_err(err)?;
            let n = f.read(&mut head).map_err(err)?;
            if n > 0 && &head[..n] != &b"SQLite format 3\0"[..n] {
                return Err("the browser database file is not a database".to_string());
            }
        }
        let conn = localfs::open_database(&judged).map_err(err)?;
        let check: String = conn.query_row("pragma quick_check", [], |r| r.get(0)).map_err(err)?;
        if check != "ok" {
            return Err("the browser database failed its integrity check".to_string());
        }
        // An imported file's triggers and views cannot call SQL functions with side effects.
        conn.pragma_update(None, "trusted_schema", false).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_millis(1000)).map_err(err)?;
        bring_up(&conn, EXTRA_SCHEMAS)?;
        Ok(Db { conn, caps: CAPS })
    }

    /// Dev Null Mode's database: in memory, gone with the process (iTerm2's `file::memory:`).
    pub fn open_in_memory() -> Db {
        Db::open_in_memory_with(EXTRA_SCHEMAS)
    }

    fn open_in_memory_with(extra: &[&str]) -> Db {
        let conn = Connection::open_in_memory().expect("an in-memory database");
        bring_up(&conn, extra).expect("the schema");
        Db { conn, caps: CAPS }
    }

    /// The connection, for the queries a later plan writes as `impl Db` blocks in its own file.
    #[allow(dead_code)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn named_marks_cap(&self) -> i64 {
        self.caps.named_marks
    }

    pub fn with_caps(mut self, caps: Caps) -> Db {
        self.caps = caps;
        self
    }

    // --- Writes (the one place a page's text is bounded) ---

    /// One navigation: a history row, and the visit row of its host and path counted up (`BD:346-400`).
    /// A new title replaces the old only when it is not empty. Text a page chose is cleaned and
    /// bounded here; a URL that cannot be stored is not recorded.
    pub fn record_visit(&self, url: &str, title: Option<&str>, session: Option<&str>, referrer: Option<&str>, transition: i64, now: i64) {
        let Some(url) = clean_url(url) else { return };
        let title = title.and_then(clean_title);
        let session = session.filter(|s| s.len() <= 128);
        let referrer = referrer.and_then(clean_url);
        let _ = self.conn.execute(
            "insert into BrowserHistory (id, url, title, visitDate, sessionGuid, referrerUrl, transitionType) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![uuid::Uuid::new_v4().to_string(), url, title, now, session, referrer, transition],
        );
        let (host, path) = host_and_path(&url);
        let updated = self.conn.execute(
            "update BrowserVisits set visitCount = visitCount + 1, lastVisitDate = ?1, title = coalesce(?2, title), url = ?3 where hostname = ?4 and path = ?5",
            params![now, title, url, host, path],
        );
        if updated == Ok(0) {
            let _ = self.conn.execute(
                "insert into BrowserVisits (hostname, path, visitCount, lastVisitDate, firstVisitDate, title, url) values (?1, ?2, 1, ?3, ?3, ?4, ?5)",
                params![host, path, now, title, url],
            );
        }
        // Over the cap: the oldest rows go, one statement each.
        let _ = self.conn.execute(
            "delete from BrowserHistory where id in (select id from BrowserHistory order by visitDate asc, rowid asc limit max(0, (select count(*) from BrowserHistory) - ?1))",
            params![self.caps.history],
        );
        let _ = self.conn.execute(
            "delete from BrowserVisits where rowid in (select rowid from BrowserVisits order by lastVisitDate asc, rowid asc limit max(0, (select count(*) from BrowserVisits) - ?1))",
            params![self.caps.visits],
        );
    }

    /// A page's new title, in both tables (`updateTitle(_:forUrl:)`); an empty one changes nothing.
    pub fn update_title(&self, title: &str, url: &str) {
        let (Some(title), Some(url)) = (clean_title(title), clean_url(url)) else { return };
        let _ = self.conn.execute("update BrowserHistory set title = ?1 where url = ?2", params![title, url]);
        let (host, path) = host_and_path(&url);
        let _ = self.conn.execute("update BrowserVisits set title = ?1 where hostname = ?2 and path = ?3", params![title, host, path]);
    }

    /// False when the URL cannot be stored, is already a bookmark, or the table is full.
    pub fn add_bookmark(&self, url: &str, title: Option<&str>, now: i64) -> bool {
        let Some(url) = clean_url(url) else { return false };
        let full: bool = self.conn.query_row("select count(*) >= ?1 from BrowserBookmarks", params![self.caps.bookmarks], |r| r.get(0)).unwrap_or(true);
        if full {
            return false;
        }
        let title = title.and_then(clean_title);
        self.conn.execute("insert into BrowserBookmarks (url, title, dateAdded) values (?1, ?2, ?3)", params![url, title, now]) == Ok(1)
    }

    pub fn remove_bookmark(&self, url: &str) -> bool {
        let Some(url) = clean_url(url) else { return false };
        self.conn.execute("delete from BrowserBookmarks where url = ?1", params![url]).is_ok_and(|n| n > 0)
    }

    pub fn delete_history(&self, id: &str) {
        let _ = self.conn.execute("delete from BrowserHistory where id = ?1", params![id]);
    }

    pub fn clear_history(&self) {
        let _ = self.conn.execute("delete from BrowserHistory", []);
        let _ = self.conn.execute("delete from BrowserVisits", []);
    }

    pub fn clear_bookmarks(&self) {
        let _ = self.conn.execute("delete from BrowserBookmarks", []);
    }

    // --- Reads (every text is a bound parameter; every limit at most `MAX_LIMIT`) ---

    pub fn visit_count(&self, url: &str) -> i64 {
        let Some(url) = clean_url(url) else { return 0 };
        let (host, path) = host_and_path(&url);
        self.conn.query_row("select visitCount from BrowserVisits where hostname = ?1 and path = ?2", params![host, path], |r| r.get(0)).optional().ok().flatten().unwrap_or(0)
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        let Some(url) = clean_url(url) else { return false };
        self.conn.query_row("select 1 from BrowserBookmarks where url = ?1", params![url], |_| Ok(())).optional().ok().flatten().is_some()
    }

    /// Whether a URL, exactly as given, is in the history, the visits or the bookmarks: the page
    /// messages that navigate honor only such a URL.
    pub fn knows_url(&self, url: &str) -> bool {
        self.conn
            .query_row(
                "select exists(select 1 from BrowserHistory where url = ?1) or exists(select 1 from BrowserVisits where url = ?1) or exists(select 1 from BrowserBookmarks where url = ?1)",
                params![url],
                |r| r.get(0),
            )
            .unwrap_or(false)
    }

    /// Bookmarks whose URL or title holds `prefix`, newest first (`bookmarkSuggestionsQuery`).
    pub fn bookmark_suggestions(&self, prefix: &str, limit: usize) -> Vec<(String, Option<String>)> {
        let p = like_contains(prefix);
        self.rows("select url, title from BrowserBookmarks where url like ?1 escape '\\' or title like ?1 escape '\\' order by dateAdded desc limit ?2", params![p, lim(limit)], |r| Ok((r.get(0)?, r.get(1)?)))
    }

    /// Bookmarks matching any term of `terms` (all when empty), newest first, as (URL, title, date added).
    pub fn search_bookmarks(&self, terms: &str, offset: usize, limit: usize) -> Vec<(String, Option<String>, i64)> {
        let (sql, mut args) = Db::any_term("select url, title, dateAdded from BrowserBookmarks", &terms_of(terms), "url", "title");
        let sql = format!("{sql} order by dateAdded desc limit ?{} offset ?{}", args.len() + 1, args.len() + 2);
        args.extend([Value::Integer(lim(limit)), Value::Integer(off(offset))]);
        self.rows(&sql, params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    }

    /// Every bookmark in `sort` order (`dateAdded`, `title` or `url`; anything else is the date).
    pub fn all_bookmarks(&self, sort: &str, offset: usize, limit: usize) -> Vec<BookmarkRow> {
        let order = match sort {
            "title" => "title asc",
            "url" => "url asc",
            _ => "dateAdded desc",
        };
        self.rows(&format!("select url, title, dateAdded from BrowserBookmarks order by {order} limit ?1 offset ?2"), params![lim(limit), off(offset)], |r| Ok(BookmarkRow { url: r.get(0)?, title: r.get(1)?, date_added: r.get(2)? }))
    }

    /// The URL bar's history candidates (`suggestionsQuery`): visits whose host holds the typed
    /// host (and whose path starts with the typed path), most visited first, as (URL, title, visits).
    pub fn history_suggestions(&self, query: &str, limit: usize) -> Vec<(String, Option<String>, i64)> {
        let q = query.trim();
        let (host, path) = host_and_path(&if q.contains("://") { q.to_string() } else { format!("//{q}") });
        if path.is_empty() {
            self.rows(
                "select url, title, visitCount from BrowserVisits where hostname like ?1 escape '\\' and url is not null order by visitCount desc, lastVisitDate desc limit ?2",
                params![like_contains(&host), lim(limit)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        } else {
            let suffix = like_contains(&host)[..like_contains(&host).len() - 1].to_string();
            let prefix = format!("{}%", &like_contains(&path)[1..like_contains(&path).len() - 1]);
            self.rows(
                "select url, title, visitCount from BrowserVisits where hostname like ?1 escape '\\' and path like ?2 escape '\\' and url is not null order by visitCount desc, lastVisitDate desc limit ?3",
                params![suffix, prefix, lim(limit)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        }
    }

    /// History rows matching any term (all, newest first, when `terms` is empty).
    pub fn search_history(&self, terms: &str, offset: usize, limit: usize) -> Vec<HistoryRow> {
        let t = terms_of(terms);
        if t.is_empty() {
            return self.recent_history(offset, limit);
        }
        let (sql, mut args) = Db::any_term("select id, url, title, visitDate, transitionType from BrowserHistory", &t, "url", "title");
        let sql = format!("{sql} order by visitDate desc limit ?{} offset ?{}", args.len() + 1, args.len() + 2);
        args.extend([Value::Integer(lim(limit)), Value::Integer(off(offset))]);
        self.rows(&sql, params_from_iter(args), history_row)
    }

    pub fn recent_history(&self, offset: usize, limit: usize) -> Vec<HistoryRow> {
        self.rows("select id, url, title, visitDate, transitionType from BrowserHistory order by visitDate desc limit ?1 offset ?2", params![lim(limit), off(offset)], history_row)
    }

    /// Visited sites for Open Quickly: every term in the URL, or every term in the title, as (URL,
    /// title), newest first.
    pub fn visits_matching(&self, terms: &str) -> Vec<(String, Option<String>)> {
        self.visits_matching_since(terms, i64::MIN, 0, 50)
    }

    /// `visits_matching` for iTerm2's Open Quickly (`searchVisits`: seen within 90 days, at least
    /// twice, 50 rows).
    pub fn visits_matching_since(&self, terms: &str, since: i64, min_count: i64, limit: usize) -> Vec<(String, Option<String>)> {
        let t = terms_of(terms);
        if t.is_empty() {
            return Vec::new();
        }
        let all = |col: &str, base: usize| (0..t.len()).map(|i| format!("{col} like ?{} escape '\\'", base + i + 1)).collect::<Vec<_>>().join(" and ");
        let n = t.len();
        let sql = format!(
            "select url, title from BrowserVisits where url is not null and (({}) or ({})) and lastVisitDate > ?{} and visitCount >= ?{} order by lastVisitDate desc limit ?{}",
            all("url", 0),
            all("title", n),
            2 * n + 1,
            2 * n + 2,
            2 * n + 3
        );
        let mut args: Vec<Value> = t.iter().chain(t.iter()).map(|x| Value::Text(like_contains(x))).collect();
        args.extend([Value::Integer(since), Value::Integer(min_count), Value::Integer(lim(limit))]);
        self.rows(&sql, params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))
    }

    /// The most visited sites (`topVisitedUrls`).
    pub fn top_sites(&self, limit: usize) -> Vec<TopSite> {
        self.rows("select url, title, hostname, visitCount from BrowserVisits where url is not null order by visitCount desc, lastVisitDate desc limit ?1", params![lim(limit)], |r| {
            Ok(TopSite { url: r.get(0)?, title: r.get(1)?, hostname: r.get::<_, String>(2)?.trim_start_matches('.').to_string(), visits: r.get(3)? })
        })
    }

    // --- Helpers ---

    /// `base` with `where (url like ?1 or title like ?1) or (...)`: any term in either column.
    fn any_term(base: &str, terms: &[String], a: &str, b: &str) -> (String, Vec<Value>) {
        if terms.is_empty() {
            return (base.to_string(), Vec::new());
        }
        let conds: Vec<String> = (1..=terms.len()).map(|i| format!("{a} like ?{i} escape '\\' or {b} like ?{i} escape '\\'")).collect();
        (format!("{base} where {}", conds.join(" or ")), terms.iter().map(|t| Value::Text(like_contains(t))).collect())
    }

    fn rows<T>(&self, sql: &str, args: impl rusqlite::Params, map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> Vec<T> {
        let Ok(mut st) = self.conn.prepare_cached(sql) else { return Vec::new() };
        let Ok(it) = st.query_map(args, map) else { return Vec::new() };
        it.flatten().collect()
    }
}

fn history_row(r: &rusqlite::Row) -> rusqlite::Result<HistoryRow> {
    Ok(HistoryRow { id: r.get(0)?, url: r.get(1)?, title: r.get(2)?, visit_date: r.get(3)?, transition: r.get(4)? })
}

#[cfg(test)]
mod tests {
    /// 13x2 Task 1: a later plan's schema runs after the base one, and `conn` reaches it.
    #[test]
    fn extra_schemas_run() {
        let db = Db::open_in_memory_with(&["create table if not exists T (a text);", "insert into T values ('x');"]);
        let mut st = db.conn().prepare("select * from T").expect("the extra table exists");
        let n: usize = st.query_map([], |r| r.get::<_, String>(0)).unwrap().count();
        assert_eq!(n, 1);
        assert!(EXTRA_SCHEMAS.is_empty() || Db::open_in_memory().conn().prepare("select 1").is_ok());
        assert!(Db::open_in_memory().conn().prepare("select * from BrowserHistory").is_ok());
    }

    use super::*;

    fn columns(db: &Db, table: &str) -> Vec<String> {
        let mut st = db.conn.prepare(&format!("pragma table_info({table})")).unwrap();
        st.query_map([], |r| r.get::<_, String>(1)).unwrap().map(Result::unwrap).collect()
    }

    fn indexes(db: &Db) -> Vec<String> {
        let mut st = db.conn.prepare("select name from sqlite_master where type = 'index' and name like 'idx_%' order by name").unwrap();
        st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn.query_row(&format!("select count(*) from {table}"), [], |r| r.get(0)).unwrap()
    }

    /// The injection strings a hostile page chooses as its title and its URL.
    const EVIL_TITLE: &str = "\"><img src=x onerror=alert(1)>";
    const EVIL_URL: &str = "https://evil.test/');alert(1);//\"q";

    #[test]
    fn schema_follows_iterm2() {
        let db = Db::open_in_memory();
        assert_eq!(columns(&db, "BrowserHistory"), ["id", "url", "title", "visitDate", "sessionGuid", "referrerUrl", "transitionType"]);
        assert_eq!(columns(&db, "BrowserBookmarks"), ["url", "title", "dateAdded"]);
        assert_eq!(columns(&db, "BrowserVisits"), ["hostname", "path", "visitCount", "lastVisitDate", "firstVisitDate", "title", "url"]);
        assert_eq!(
            indexes(&db),
            [
                "idx_browser_bookmarks_date",
                "idx_browser_bookmarks_title",
                "idx_browser_history_date",
                "idx_browser_history_session",
                "idx_browser_history_title",
                "idx_browser_history_url",
                "idx_browser_named_marks_date",
                "idx_browser_named_marks_guid",
                "idx_browser_named_marks_url",
                "idx_browser_visits_count",
                "idx_browser_visits_hostname",
                "idx_browser_visits_path",
                "idx_browser_visits_url",
            ]
        );
        let p = path(Path::new("C:\\Users\\a\\AppData\\Local"), crate::browser::USER_ID);
        assert!(p.to_string_lossy().ends_with("browserdb-AC0E9812-7F88-478B-B361-5526082EDDB3.sqlite"), "{p:?}");
        assert!(p.to_string_lossy().contains("Tandem"));
    }

    #[test]
    fn visits_follow_iterm2() {
        let db = Db::open_in_memory();
        db.record_visit("https://a.b/x?q", Some("First"), Some("s1"), None, 0, 100);
        db.record_visit("https://a.b/x?q", None, Some("s1"), None, 1, 200);
        assert_eq!(count(&db, "BrowserHistory"), 2, "every navigation is a history row");
        assert_eq!(count(&db, "BrowserVisits"), 1, "one visit row per host and path");
        assert_eq!(db.visit_count("https://a.b/x?q"), 2);
        let title: Option<String> = db.conn.query_row("select title from BrowserVisits", [], |r| r.get(0)).unwrap();
        assert_eq!(title.as_deref(), Some("First"), "an empty title keeps the old one");
        // iTerm2's parse (BrowserVisits.parseUrl): a period before the host, the query in the path,
        // no fragment, no tracking parameters.
        assert_eq!(host_and_path("https://A.b/x?q"), (".a.b".to_string(), "/x?q".to_string()));
        assert_eq!(host_and_path("https://a.b/x?utm_source=1&q=2&fbclid=3#top"), (".a.b".to_string(), "/x?q=2".to_string()));
        assert_eq!(host_and_path("https://u:p@a.b:8443/"), (".a.b".to_string(), "/".to_string()));
        assert_eq!(host_and_path("https://a.b"), (".a.b".to_string(), String::new()));
        // A title change reaches both tables.
        db.update_title("Second", "https://a.b/x?q");
        let hist: Vec<Option<String>> = db.conn.prepare("select title from BrowserHistory").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert!(hist.iter().all(|t| t.as_deref() == Some("Second")), "{hist:?}");
        let visit: Option<String> = db.conn.query_row("select title from BrowserVisits", [], |r| r.get(0)).unwrap();
        assert_eq!(visit.as_deref(), Some("Second"));
        // Bookmarks: a primary key on the URL.
        assert!(db.add_bookmark("https://a.b/x?q", Some("Mine"), 300));
        assert!(!db.add_bookmark("https://a.b/x?q", Some("Mine"), 301), "the second add is refused");
        assert!(db.is_bookmarked("https://a.b/x?q") && !db.is_bookmarked("https://a.b/other"));
        assert_eq!(db.bookmark_suggestions("a.b", 10), vec![("https://a.b/x?q".to_string(), Some("Mine".to_string()))], "by URL");
        assert_eq!(db.bookmark_suggestions("Min", 10).len(), 1, "by title");
        assert!(db.remove_bookmark("https://a.b/x?q") && !db.remove_bookmark("https://a.b/x?q"));
        // History suggestions: by visits, then recency.
        let sug = db.history_suggestions("a.b", 10);
        assert_eq!(sug, vec![("https://a.b/x?q".to_string(), Some("Second".to_string()), 2)]);
        db.delete_history(&db.recent_history(0, 10)[0].id);
        assert_eq!(count(&db, "BrowserHistory"), 1);
        db.clear_history();
        assert_eq!((count(&db, "BrowserHistory"), count(&db, "BrowserVisits")), (0, 0));
    }

    #[test]
    fn hostile_text_is_stored_inert_and_bounded() {
        let db = Db::open_in_memory();
        // A quote is legal in a URL and is kept; the title is text, kept as text (the page escapes it).
        db.record_visit(EVIL_URL, Some(EVIL_TITLE), None, None, 1, 10);
        db.add_bookmark(EVIL_URL, Some(EVIL_TITLE), 11);
        let rows = db.recent_history(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].url.as_str(), rows[0].title.as_deref()), (EVIL_URL, Some(EVIL_TITLE)));
        assert!(db.is_bookmarked(EVIL_URL) && db.knows_url(EVIL_URL));
        // Terms are parameters: SQL, wildcards and quotes are only text.
        for term in ["'; drop table BrowserHistory;--", "%", "_", "\"><img", "\\", "x' or '1'='1"] {
            let _ = db.search_history(term, 0, 10);
            let _ = db.search_bookmarks(term, 0, 10);
            let _ = db.history_suggestions(term, 10);
            let _ = db.visits_matching(term);
        }
        assert_eq!(count(&db, "BrowserHistory"), 1, "the table survived");
        assert!(db.search_history("%", 0, 10).is_empty(), "a percent sign is not a wildcard");
        assert!(db.search_history("_vil", 0, 10).is_empty(), "an underscore is not a wildcard");
        assert_eq!(db.search_history("onerror", 0, 10).len(), 1, "a real substring matches");
        // Control and bidi characters never reach the table; the title is cut at 512.
        db.record_visit("https://c.test/", Some(&format!("a\u{202e}b\u{0}c\nd{}", "x".repeat(600))), None, None, 1, 20);
        let t = db.search_history("c.test", 0, 10)[0].title.clone().unwrap();
        assert!(t.starts_with("abcd") && t.chars().count() == 512, "{}", t.chars().count());
        // A URL over 2048 bytes is not stored; user:password@ never is.
        db.record_visit(&format!("https://long.test/{}", "p".repeat(2100)), Some("L"), None, None, 1, 30);
        assert!(db.search_history("long.test", 0, 10).is_empty());
        db.record_visit("https://user:secret@h.test/p", Some("H"), None, None, 1, 40);
        let h = db.search_history("h.test", 0, 10);
        assert_eq!(h[0].url, "https://h.test/p");
        assert!(!db.knows_url("https://user:secret@h.test/p"));
        db.add_bookmark("https://u2:pw2@k.test/", Some("K"), 50);
        assert!(db.is_bookmarked("https://k.test/"));
        // Every limit is at most 500.
        for i in 0..520 {
            db.record_visit(&format!("https://many.test/{i}"), None, None, None, 1, 100 + i);
        }
        assert_eq!(db.recent_history(0, 100_000).len(), 500);
        assert_eq!(db.search_history("many.test", 0, 100_000).len(), 500);
    }

    #[test]
    fn the_tables_are_capped() {
        let db = Db::open_in_memory().with_caps(Caps { history: 5, visits: 4, bookmarks: 3, named_marks: 3 });
        for i in 0..10 {
            db.record_visit(&format!("https://a.test/{i}"), None, None, None, 1, 100 + i);
        }
        assert_eq!(count(&db, "BrowserHistory"), 5);
        assert_eq!(count(&db, "BrowserVisits"), 4);
        let newest = db.recent_history(0, 10);
        assert_eq!(newest[0].url, "https://a.test/9", "the oldest rows go first");
        for i in 0..5 {
            db.add_bookmark(&format!("https://b.test/{i}"), None, 200 + i);
        }
        assert_eq!(count(&db, "BrowserBookmarks"), 3, "a full table refuses; nothing the person kept is deleted");
        assert!(db.is_bookmarked("https://b.test/0") && !db.is_bookmarked("https://b.test/4"));
    }

    #[test]
    fn the_file_is_judged_owner_only_and_replaced_when_corrupt() {
        let dir = std::env::temp_dir().join(format!("tandem-bdb-{}", uuid::Uuid::new_v4().simple()));
        let file = path(&dir, "u");
        let db = Db::open(&file).unwrap();
        db.record_visit("https://a.test/", Some("A"), None, None, 0, 1);
        drop(db);
        assert!(file.exists());
        #[cfg(windows)]
        {
            let judged = crate::localfs::LocalPath::judge(&file).unwrap();
            let sddl = crate::localfs::dacl_sddl(&judged.parent().unwrap().unfollowed("browserdb-u.sqlite").unwrap()).unwrap();
            assert_eq!(sddl.matches("(A;").count(), 1, "one allow entry: {sddl}");
            assert!(sddl.contains(&crate::winpipe::user_sid().unwrap()) && sddl.starts_with("D:P"), "{sddl}");
        }
        let again = Db::open(&file).unwrap();
        assert_eq!(again.recent_history(0, 10).len(), 1, "history survives a reopen");
        drop(again);
        // A file that is not a database is set aside, never a panic.
        std::fs::write(&file, b"this is not sqlite at all, it is long enough to be a header ........").unwrap();
        let fresh = Db::open(&file).unwrap();
        assert!(fresh.recent_history(0, 10).is_empty());
        drop(fresh);
        assert!(file.with_file_name("browserdb-u.sqlite.bad").exists(), "the corrupt file was renamed");
        // A path on a share is refused.
        assert!(Db::open(Path::new("\\\\srv\\share\\browserdb-u.sqlite")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
