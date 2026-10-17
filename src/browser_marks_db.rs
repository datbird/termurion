//! The named marks table (milestone 13x5, Task 2): iTerm2's `BrowserNamedMarks`
//! (`Browser/Database/BrowserNamedMarks.swift`, `BNM`) as `impl Db` blocks over 13x2's `Db::conn()`.
//! A mark is the person's data, so the table is bounded (count, guid, URL, name and text lengths)
//! at the one write point and every read drops a row that breaks a bound (a corrupt or oversized
//! file loads empty rather than feeding a menu or a script). Names are cleaned with
//! `inert_files::display_text` before they are stored. Every value is a bound parameter; the URL
//! prefix and the search text are escaped for LIKE (iTerm2 pastes them raw, so `%` and `_` in a
//! URL were wildcards: a recorded fix).

use crate::browser_db::Db;
use crate::browser_marks::{without_fragment, MAX_GUID, MAX_NAME, MAX_TEXT, MAX_URL};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

/// The table and its three indexes, verbatim (`BNM:47-59`, iTerm2's `Columns` names written out).
/// Four statements: 13x2 runs it with `execute_batch`.
pub const SCHEMA: &str = "
create table if not exists BrowserNamedMarks
    (sort integer primary key autoincrement,
     guid text unique not null,
     url text not null,
     name text not null,
     text text not null default '',
     dateAdded integer not null);

CREATE INDEX IF NOT EXISTS idx_browser_named_marks_guid ON BrowserNamedMarks(guid);
CREATE INDEX IF NOT EXISTS idx_browser_named_marks_url ON BrowserNamedMarks(url);
CREATE INDEX IF NOT EXISTS idx_browser_named_marks_date ON BrowserNamedMarks(dateAdded DESC);
";

/// The manager's cache reads every mark with this limit (`NMM:556-558`). The table never holds
/// more than `Caps::named_marks`, so it is the bound in practice.
pub const ALL_LIMIT: usize = 1_000_000;
/// The search limit iTerm2 defaults to (`BNM:168`).
pub const SEARCH_LIMIT: usize = 50;
const MAX_OFFSET: usize = 1_000_000;
const MAX_TERMS: usize = 128;

/// One mark of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub guid: String,
    pub url: String,
    pub name: String,
    pub sort: i64,
    pub text: String,
    pub date_added: f64,
}

fn escaped(text: &str, tail: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push_str(tail);
    out
}

/// A row read back, or None when a bound is broken (the row is skipped).
fn row_of(r: &rusqlite::Row) -> rusqlite::Result<Option<Row>> {
    let guid: String = r.get(1)?;
    let url: String = r.get(2)?;
    let name: String = r.get(3)?;
    let text: String = r.get::<_, Option<String>>(4)?.unwrap_or_default();
    // `dateAdded` is a real iTerm2 wrote, or an integer another writer did.
    let date_added: f64 = r.get(5)?;
    let sort: i64 = r.get(0)?;
    let ok = !guid.is_empty()
        && guid.len() <= MAX_GUID
        && !url.is_empty()
        && url.len() <= MAX_URL
        && name.len() <= MAX_NAME * 4
        && name.chars().count() <= MAX_NAME
        && text.len() <= MAX_TEXT * 4
        && text.chars().count() <= MAX_TEXT
        && date_added.is_finite();
    Ok(ok.then_some(Row { guid, url, name, sort, text, date_added }))
}

fn clean_name(name: &str) -> String {
    crate::inert_files::display_text(name, MAX_NAME, false)
}

impl Db {
    fn mark_rows(&self, sql: &str, args: impl rusqlite::Params) -> Vec<Row> {
        let Ok(mut st) = self.conn().prepare_cached(sql) else { return Vec::new() };
        let Ok(it) = st.query_map(args, row_of) else { return Vec::new() };
        it.flatten().flatten().collect()
    }

    /// `add` (`BNM:75-91`). False for a duplicate guid, a full table, or a value out of bounds
    /// (an empty or control-character URL, a URL over `MAX_URL`, text over `MAX_TEXT`, a guid over
    /// `MAX_GUID`, a time that is not finite). The name is cleaned and cut to `MAX_NAME`.
    pub fn add_named_mark(&self, guid: &str, url: &str, name: &str, text: &str, now: f64) -> bool {
        if guid.is_empty()
            || guid.len() > MAX_GUID
            || url.is_empty()
            || url.len() > MAX_URL
            || url.chars().any(char::is_control)
            || text.chars().count() > MAX_TEXT
            || !now.is_finite()
        {
            return false;
        }
        let cap = self.named_marks_cap();
        let full: bool = self.conn().query_row("select count(*) >= ?1 from BrowserNamedMarks", params![cap], |r| r.get(0)).unwrap_or(true);
        if full {
            return false;
        }
        self.conn()
            .execute(
                "insert into BrowserNamedMarks (guid, url, name, text, dateAdded) values (?, ?, ?, ?, ?)",
                params![guid, url, clean_name(name), text, now],
            )
            == Ok(1)
    }

    /// `removeNamedMark` (`BNM:178-180`): true when a row went.
    pub fn remove_named_mark(&self, guid: &str) -> bool {
        self.conn().execute("DELETE FROM BrowserNamedMarks WHERE guid = ?", params![guid]).is_ok_and(|n| n > 0)
    }

    /// `renameNamedMark` (`BNM:186-192`): true when a row changed. The name is cleaned and cut.
    pub fn rename_named_mark(&self, guid: &str, name: &str) -> bool {
        self.conn().execute("UPDATE BrowserNamedMarks SET name = ? WHERE guid = ?", params![clean_name(name), guid]).is_ok_and(|n| n > 0)
    }

    /// `getNamedMark` (`BNM:182-184`).
    pub fn named_mark(&self, guid: &str) -> Option<Row> {
        let mut st = self.conn().prepare_cached("SELECT * FROM BrowserNamedMarks WHERE guid = ?").ok()?;
        st.query_row(params![guid], row_of).optional().ok().flatten().flatten()
    }

    /// `getPaginatedNamedMarks` (`BNM:132-156`): newest first; with a page URL, that page's marks
    /// (URL prefix, fragment ignored) come first.
    pub fn named_marks_page(&self, sort_first: Option<&str>, offset: usize, limit: usize) -> Vec<Row> {
        let (limit, offset) = (limit.min(ALL_LIMIT) as i64, offset.min(MAX_OFFSET) as i64);
        match sort_first {
            Some(url) => {
                let url = without_fragment(url);
                self.mark_rows(
                    "SELECT * FROM BrowserNamedMarks ORDER BY CASE WHEN SUBSTR(url, 1, LENGTH(?)) = ? THEN 0 ELSE 1 END, sort DESC LIMIT ? OFFSET ?",
                    params![url, url, limit, offset],
                )
            }
            None => self.mark_rows("SELECT * FROM BrowserNamedMarks ORDER BY sort DESC LIMIT ? OFFSET ?", params![limit, offset]),
        }
    }

    /// `getNamedMarks(forUrl:)` (`BNM:158-166`): marks whose URL starts with this one without its
    /// fragment, newest first. A prefix, so `https://a.b/` finds the marks of `https://a.b/p`
    /// (iTerm2's `LIKE ? || '%'`, recorded).
    pub fn named_marks_for_url(&self, url: &str) -> Vec<Row> {
        let prefix = escaped(&without_fragment(url), "%");
        self.mark_rows("SELECT * FROM BrowserNamedMarks WHERE url LIKE ? ESCAPE '\\' ORDER BY sort DESC LIMIT ?", params![prefix, ALL_LIMIT as i64])
    }

    /// `searchNamedMarks` (`BNM:168-176`): the text anywhere in name, text or URL, newest first.
    pub fn search_named_marks(&self, terms: &str, offset: usize, limit: usize) -> Vec<Row> {
        let terms: String = terms.chars().take(MAX_TERMS).collect();
        let pattern = escaped(&terms, "");
        let pattern = format!("%{pattern}%");
        self.mark_rows(
            "SELECT * FROM BrowserNamedMarks WHERE name LIKE ?1 ESCAPE '\\' OR text LIKE ?1 ESCAPE '\\' OR url LIKE ?1 ESCAPE '\\' ORDER BY sort DESC LIMIT ?2 OFFSET ?3",
            params![pattern, limit.min(ALL_LIMIT) as i64, offset.min(MAX_OFFSET) as i64],
        )
    }
}

/// One validated answer of `update-mark-positions.js`.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionUpdate {
    pub guid: String,
    pub scroll_y: f64,
    pub offset_y: f64,
    pub element_top: f64,
}

/// The most updates one answer may hold, and the largest magnitude a position may have.
pub const MAX_UPDATES: usize = crate::browser_marks::MAX_MARKS;
const MAX_POSITION: f64 = 1.0e9;

/// The answer of `update-mark-positions.js` comes from data attributes the page can edit: keep an
/// entry only when its guid is one of this pane's marks (`known`) and every position is a finite
/// number within bounds; at most `MAX_UPDATES` entries, one per guid. Anything else is dropped.
pub fn valid_position_updates(answer: &Value, known: &dyn Fn(&str) -> bool) -> Vec<PositionUpdate> {
    let Some(list) = answer.as_array() else { return Vec::new() };
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite() && n.abs() <= MAX_POSITION);
    let mut out: Vec<PositionUpdate> = Vec::new();
    for item in list.iter().take(MAX_UPDATES) {
        let Some(guid) = item.get("guid").and_then(Value::as_str) else { continue };
        if guid.is_empty() || guid.len() > MAX_GUID || !known(guid) || out.iter().any(|u| u.guid == guid) {
            continue;
        }
        let (Some(scroll_y), Some(offset_y), Some(element_top)) = (num(item, "scrollY"), num(item, "offsetY"), num(item, "elementTop")) else { continue };
        out.push(PositionUpdate { guid: guid.to_string(), scroll_y, offset_y, element_top });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_marks_schema_follows_iterm2() {
        let db = Db::open_in_memory();
        let mut st = db.conn().prepare("PRAGMA table_info(BrowserNamedMarks)").unwrap();
        // (name, type, notnull, default, pk)
        let cols: Vec<(String, String, i64, Option<String>, i64)> =
            st.query_map([], |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap().map(Result::unwrap).collect();
        let names: Vec<&str> = cols.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["sort", "guid", "url", "name", "text", "dateAdded"]);
        assert_eq!((cols[0].1.to_lowercase(), cols[0].4), ("integer".to_string(), 1));
        for (i, ty) in [(1, "text"), (2, "text"), (3, "text"), (4, "text"), (5, "integer")] {
            assert_eq!(cols[i].1.to_lowercase(), ty);
            assert_eq!(cols[i].2, 1, "{} is not null", cols[i].0);
        }
        assert_eq!(cols[4].3.as_deref(), Some("''"));
        let mut st = db.conn().prepare("select name from sqlite_master where type = 'index'").unwrap();
        let idx: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for want in ["idx_browser_named_marks_guid", "idx_browser_named_marks_url", "idx_browser_named_marks_date"] {
            assert!(idx.iter().any(|i| i == want), "{want}");
        }
        // The if-not-exists forms: running the schema again succeeds.
        db.conn().execute_batch(SCHEMA).unwrap();
        db.conn().execute_batch(SCHEMA).unwrap();
        assert!(crate::browser_db::EXTRA_SCHEMAS.contains(&SCHEMA));
    }

    fn guids(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.guid.as_str()).collect()
    }

    #[test]
    fn named_mark_queries_follow_iterm2() {
        let db = Db::open_in_memory();
        assert!(db.add_named_mark("g1", "https://a.b/p#iterm-mark:x", "one", "", 1727000000.5));
        assert!(db.add_named_mark("g2", "https://c.d/", "two", "", 1727000001.0));
        assert!(!db.add_named_mark("g1", "https://e.f/", "dup", "", 1.0), "guid is unique");
        assert_eq!(db.named_mark("g1").unwrap().sort, 1);
        assert_eq!(db.named_mark("g2").unwrap().sort, 2);
        assert_eq!(guids(&db.named_marks_page(None, 0, ALL_LIMIT)), ["g2", "g1"]);
        assert_eq!(guids(&db.named_marks_page(Some("https://a.b/p#frag"), 0, 10)), ["g1", "g2"]);
        assert_eq!(guids(&db.named_marks_for_url("https://a.b/p#other")), ["g1"]);
        assert_eq!(guids(&db.named_marks_for_url("https://a.b/")), ["g1"], "a prefix");
        assert_eq!(guids(&db.search_named_marks("tw", 0, 50)), ["g2"]);
        assert_eq!(guids(&db.search_named_marks("a.b", 0, 50)), ["g1"], "the URL is searched");
        assert!(db.rename_named_mark("g1", "uno"));
        assert_eq!(db.named_mark("g1").unwrap().name, "uno");
        assert!(db.remove_named_mark("g2"));
        assert!(!db.remove_named_mark("g2"));
        assert!(!db.rename_named_mark("g2", "x"));
        assert_eq!(db.named_mark("g1").unwrap().date_added, 1727000000.5);
    }

    #[test]
    fn wildcards_in_urls_and_searches_are_literal() {
        let db = Db::open_in_memory();
        assert!(db.add_named_mark("g1", "https://a.b/100%_done", "n", "", 1.0));
        assert!(db.add_named_mark("g2", "https://a.b/100xxdone", "m", "", 2.0));
        assert_eq!(guids(&db.named_marks_for_url("https://a.b/100%_")), ["g1"]);
        assert_eq!(guids(&db.search_named_marks("%", 0, 50)), ["g1"]);
        assert!(db.search_named_marks("zzz", 0, 50).is_empty());
        // A page URL with an underscore or percent cannot see other URLs' marks.
        assert!(db.add_named_mark("g3", "https://a.b/abc", "o", "", 3.0));
        assert!(db.named_marks_for_url("https://a.b/_bc").is_empty());
        assert!(db.named_marks_for_url("https://a.b/%").is_empty());
        assert_eq!(guids(&db.named_marks_page(Some("https://a.b/_bc"), 0, 10)).first(), Some(&"g3"), "the page ordering is an exact prefix compare: no marks first, newest leads");
    }

    #[test]
    fn named_marks_are_bounded() {
        let db = Db::open_in_memory().with_caps(crate::browser_db::Caps { named_marks: 2, ..crate::browser_db::CAPS });
        assert!(!db.add_named_mark("", "https://a.b/", "n", "", 1.0), "empty guid");
        assert!(!db.add_named_mark(&"g".repeat(MAX_GUID + 1), "https://a.b/", "n", "", 1.0));
        assert!(!db.add_named_mark("g", "", "n", "", 1.0), "empty url");
        assert!(!db.add_named_mark("g", &format!("https://a.b/{}", "x".repeat(MAX_URL)), "n", "", 1.0));
        assert!(!db.add_named_mark("g", "https://a.b/\n", "n", "", 1.0), "control character");
        assert!(!db.add_named_mark("g", "https://a.b/", "n", &"t".repeat(MAX_TEXT + 1), 1.0));
        assert!(!db.add_named_mark("g", "https://a.b/", "n", "", f64::NAN));
        assert!(!db.add_named_mark("g", "https://a.b/", "n", "", f64::INFINITY));
        // A hostile name is cleaned and cut.
        assert!(db.add_named_mark("a", "https://a.b/", &format!("x\u{202E}y\u{0}z{}", "n".repeat(1000)), "", 1.0));
        let name = db.named_mark("a").unwrap().name;
        assert!(name.starts_with("xyzn") && name.chars().count() == MAX_NAME);
        assert!(db.add_named_mark("b", "https://a.b/", "n", "", 2.0));
        assert!(!db.add_named_mark("c", "https://a.b/", "n", "", 3.0), "a full table refuses; nothing is deleted");
        assert_eq!(db.named_marks_page(None, 0, ALL_LIMIT).len(), 2);
        assert!(db.rename_named_mark("a", &"q".repeat(1000)));
        assert_eq!(db.named_mark("a").unwrap().name.chars().count(), MAX_NAME);
    }

    #[test]
    fn corrupt_rows_load_empty() {
        let db = Db::open_in_memory();
        let c = db.conn();
        let big = "x".repeat(MAX_URL + 1);
        c.execute("insert into BrowserNamedMarks (guid, url, name, text, dateAdded) values ('big', ?, 'n', '', 1)", params![big]).unwrap();
        c.execute("insert into BrowserNamedMarks (guid, url, name, text, dateAdded) values ('nm', 'https://a.b/', ?, '', 1)", params!["n".repeat(MAX_NAME + 1)]).unwrap();
        c.execute("insert into BrowserNamedMarks (guid, url, name, text, dateAdded) values ('tx', 'https://a.b/', 'n', ?, 1)", params!["t".repeat(MAX_TEXT + 1)]).unwrap();
        c.execute("insert into BrowserNamedMarks (guid, url, name, text, dateAdded) values ('ok', 'https://a.b/', 'n', '', 7)", []).unwrap();
        assert_eq!(guids(&db.named_marks_page(None, 0, ALL_LIMIT)), ["ok"]);
        assert_eq!(db.named_mark("ok").unwrap().date_added, 7.0, "an integer dateAdded reads back");
        assert!(db.named_mark("big").is_none() && db.named_mark("nm").is_none() && db.named_mark("tx").is_none());
        assert_eq!(guids(&db.search_named_marks("", 0, 50)), ["ok"]);
    }

    #[test]
    fn position_updates_are_validated() {
        let known = |g: &str| g == "a" || g == "b";
        let answer = serde_json::json!([
            {"guid":"a","scrollY":10,"offsetY":0,"elementTop":42.5,"y":3},
            {"guid":"a","scrollY":11,"offsetY":0,"elementTop":1},
            {"guid":"zz","scrollY":1,"offsetY":0,"elementTop":1},
            {"guid":"b","scrollY":"1","offsetY":0,"elementTop":1},
            {"guid":"b","scrollY":1e30,"offsetY":0,"elementTop":1},
            {"guid":"b","scrollY":1,"offsetY":0},
            {"guid":7,"scrollY":1,"offsetY":0,"elementTop":1},
            {"guid":"b","scrollY":-5,"offsetY":2,"elementTop":9}
        ]);
        let got = valid_position_updates(&answer, &known);
        assert_eq!(got.iter().map(|u| (u.guid.as_str(), u.element_top)).collect::<Vec<_>>(), [("a", 42.5), ("b", 9.0)]);
        assert!(valid_position_updates(&serde_json::json!({"guid":"a"}), &known).is_empty());
        let many: Vec<Value> = (0..5000).map(|i| serde_json::json!({"guid":format!("g{i}"),"scrollY":1,"offsetY":0,"elementTop":1})).collect();
        assert!(valid_position_updates(&Value::Array(many), &|_| true).len() <= MAX_UPDATES);
    }
}
