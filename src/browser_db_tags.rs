//! Bookmark tags and the key-value table (`BBT`, `BKV`, iTerm2's `Database/BrowserBookmarkTags.swift`
//! and `BrowserKeyValueStore.swift`), as `impl Db` blocks over 13x2's `conn()`.
//!
//! One store owner: the tags live in the browser's own database file, written through the one
//! connection, each change a bound parameter. A tag is text a person typed or an imported file held,
//! so it is shown through `display_text`, cut to `MAX_TAG` characters and at most `MAX_TAGS_PER`
//! stay on one bookmark. Foreign keys stay OFF (iTerm2 never turns them on): the declared reference
//! is to the implicit `rowid` of a table keyed by `url`, which SQLite refuses when enforced; the
//! bookmark's tag rows are removed explicitly with the bookmark (`Db::remove_bookmark`).
//!
//! The key-value table is created and never read or written here: it held iTerm2's own extension
//! storage, and WebView2 extensions keep Chromium's storage. Nothing secret goes in it.

use crate::browser_db::{like_contains, lim, off, terms_of, Db};
use rusqlite::params;
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use rusqlite::OptionalExtension;

/// `BBT:26-37`, verbatim.
pub const TAGS_SCHEMA: &str = "create table if not exists BrowserBookmarkTags
    (bookmarkRowId integer not null,
     tag text not null,
     PRIMARY KEY (bookmarkRowId, tag),
     FOREIGN KEY (bookmarkRowId) REFERENCES BrowserBookmarks(rowid) ON DELETE CASCADE);

CREATE INDEX IF NOT EXISTS idx_browser_bookmark_tags_rowid ON BrowserBookmarkTags(bookmarkRowId);
CREATE INDEX IF NOT EXISTS idx_browser_bookmark_tags_tag ON BrowserBookmarkTags(tag);";

/// `BKV:30-47`, verbatim.
pub const KV_SCHEMA: &str = "create table if not exists BrowserKeyValueStore
    (area text not null,
     extensionId text,
     key text not null,
     value text not null,
     size Int,
     PRIMARY KEY(area, extensionId, key));
CREATE INDEX IF NOT EXISTS idx_browser_key_value_store_ext ON BrowserKeyValueStore
    (area,
     extensionId,
     key);
CREATE UNIQUE INDEX IF NOT EXISTS idx_bkv_area_ext_key ON BrowserKeyValueStore
    (area,
     extensionId,
     key);";

/// A tag longer than this (characters) is cut.
pub const MAX_TAG: usize = 64;
/// At most this many tags stay on one bookmark.
pub const MAX_TAGS_PER: usize = 32;
/// At most this many tags are searched for at once.
pub const MAX_SEARCH_TAGS: usize = 16;
/// A listing of every tag holds at most this many.
const MAX_ALL_TAGS: i64 = 1000;

/// A tag as stored and shown: the one display sanitizer, trimmed, at most `MAX_TAG` characters;
/// None when nothing is left.
pub fn clean_tag(tag: &str) -> Option<String> {
    let t = crate::inert_files::display_text(tag, MAX_TAG * 4, false);
    let t: String = t.trim().chars().take(MAX_TAG).collect();
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

impl Db {
    fn bookmark_row(&self, url: &str) -> Option<i64> {
        let url = crate::browser_db::clean_url(url)?;
        self.conn().query_row("select rowid from BrowserBookmarks where url = ?1", params![url], |r| r.get(0)).optional().ok().flatten()
    }

    /// False when the bookmark is not there, the tag is blank, it is already on the bookmark, or the
    /// bookmark holds `MAX_TAGS_PER` (`BD:633-640`).
    pub fn add_tag(&self, url: &str, tag: &str) -> bool {
        let (Some(row), Some(tag)) = (self.bookmark_row(url), clean_tag(tag)) else { return false };
        // One statement: the count and the insert cannot interleave.
        self.conn()
            .execute(
                "insert or ignore into BrowserBookmarkTags (bookmarkRowId, tag) select ?1, ?2 where (select count(*) from BrowserBookmarkTags where bookmarkRowId = ?1) < ?3",
                params![row, tag, MAX_TAGS_PER as i64],
            )
            == Ok(1)
    }

    pub fn remove_tag(&self, url: &str, tag: &str) -> bool {
        let (Some(row), Some(tag)) = (self.bookmark_row(url), clean_tag(tag)) else { return false };
        self.conn().execute("delete from BrowserBookmarkTags where bookmarkRowId = ?1 and tag = ?2", params![row, tag]).is_ok_and(|n| n > 0)
    }

    /// The bookmark's tags, sorted (`BBT:80-87`). A row an imported file holds is read clean.
    pub fn tags_for(&self, url: &str) -> Vec<String> {
        let Some(row) = self.bookmark_row(url) else { return Vec::new() };
        let raw: Vec<String> = self.rows("select tag from BrowserBookmarkTags where bookmarkRowId = ?1 order by tag asc limit ?2", params![row, (MAX_TAGS_PER * 4) as i64], |r| r.get(0));
        clean_all(raw, MAX_TAGS_PER)
    }

    /// Every tag in use, distinct and sorted (`BBT:89-95`).
    pub fn all_tags(&self) -> Vec<String> {
        let raw: Vec<String> = self.rows("select distinct tag from BrowserBookmarkTags order by tag asc limit ?1", params![MAX_ALL_TAGS], |r| r.get(0));
        clean_all(raw, MAX_ALL_TAGS as usize)
    }

    /// Bookmarks matching any term in the URL or title (all when `terms` is blank) and carrying every
    /// one of `tags`, newest first, as (URL, title, date added) (`BBT:111-157`). Every text is a bound
    /// parameter, the pattern characters of a term literal.
    pub fn search_bookmarks_with_tags(&self, terms: &str, tags: &[String], offset: usize, limit: usize) -> Vec<(String, Option<String>, i64)> {
        let mut tags: Vec<String> = tags.iter().filter_map(|t| clean_tag(t)).collect();
        tags.sort();
        tags.dedup();
        tags.truncate(MAX_SEARCH_TAGS);
        let terms = terms_of(terms);
        let mut sql = String::from("select b.url, b.title, b.dateAdded from BrowserBookmarks b");
        let mut args: Vec<Value> = terms.iter().map(|t| Value::Text(like_contains(t))).collect();
        if !terms.is_empty() {
            let conds: Vec<String> = (1..=terms.len()).map(|i| format!("b.url like ?{i} escape '\\' or b.title like ?{i} escape '\\'")).collect();
            sql.push_str(&format!(" where ({})", conds.join(" or ")));
        }
        if !tags.is_empty() {
            let first = args.len() + 1;
            let marks: Vec<String> = (0..tags.len()).map(|i| format!("?{}", first + i)).collect();
            sql.push_str(if args.is_empty() { " where" } else { " and" });
            sql.push_str(&format!(" (select count(distinct t.tag) from BrowserBookmarkTags t where t.bookmarkRowId = b.rowid and t.tag in ({})) = ?{}", marks.join(", "), first + tags.len()));
            let n = tags.len() as i64;
            args.extend(tags.into_iter().map(Value::Text));
            args.push(Value::Integer(n));
        }
        let sql = format!("{sql} order by b.dateAdded desc, b.rowid desc limit ?{} offset ?{}", args.len() + 1, args.len() + 2);
        args.extend([Value::Integer(lim(limit)), Value::Integer(off(offset))]);
        self.rows(&sql, params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    }
}

/// Stored tags as they are shown: clean, none blank, distinct, at most `max`.
fn clean_all(raw: Vec<String>, max: usize) -> Vec<String> {
    let mut out: Vec<String> = raw.iter().filter_map(|t| clean_tag(t)).collect();
    out.sort();
    out.dedup();
    out.truncate(max);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(db: &Db, sql: &str) -> Vec<String> {
        let mut st = db.conn().prepare(sql).unwrap();
        st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn().query_row(&format!("select count(*) from {table}"), [], |r| r.get(0)).unwrap()
    }

    /// Review Focus 6 and R13x8-3, over rusqlite's real SQLite.
    #[test]
    fn tag_schema_follows_iterm2() {
        let db = Db::open_in_memory();
        assert_eq!(names(&db, "select name from pragma_table_info('BrowserBookmarkTags')"), ["bookmarkRowId", "tag"]);
        assert_eq!(names(&db, "select name from pragma_table_info('BrowserKeyValueStore')"), ["area", "extensionId", "key", "value", "size"]);
        let idx = names(&db, "select name from sqlite_master where type = 'index' and name like 'idx_%' order by name");
        for want in ["idx_browser_bookmark_tags_rowid", "idx_browser_bookmark_tags_tag", "idx_browser_key_value_store_ext", "idx_bkv_area_ext_key"] {
            assert!(idx.iter().any(|i| i == want), "{want} in {idx:?}");
        }
        assert!(!db.add_tag("u", "t"), "no bookmark, no tag");
        assert!(db.add_bookmark("u", None, 1));
        assert!(db.add_tag("u", "t"), "foreign keys are off: no foreign key mismatch");
        assert!(!db.add_tag("u", "t"), "primary key");
        let fk: i64 = db.conn().query_row("pragma foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 0, "iTerm2 never runs the pragma");
        // Removing the bookmark removes its tags; a bookmark that takes the rowid inherits none.
        assert!(db.remove_bookmark("u"));
        assert_eq!(count(&db, "BrowserBookmarkTags"), 0);
        assert!(db.add_bookmark("a", None, 2));
        assert!(db.add_tag("a", "x"));
        assert!(db.remove_bookmark("a"));
        assert!(db.add_bookmark("v", None, 3));
        assert!(db.tags_for("v").is_empty(), "no inherited tag");
        // Clear all leaves both tables empty.
        assert!(db.add_tag("v", "y"));
        db.clear_bookmarks();
        assert_eq!(count(&db, "BrowserBookmarks"), 0);
        assert_eq!(count(&db, "BrowserBookmarkTags"), 0);
    }

    #[test]
    fn tag_search_follows_iterm2() {
        let db = Db::open_in_memory();
        db.add_bookmark("https://a.test/", Some("Alpha"), 3);
        db.add_bookmark("https://b.test/", Some("Beta"), 2);
        db.add_bookmark("https://c.test/", Some("Gamma"), 1);
        for (u, t) in [("https://a.test/", "x"), ("https://a.test/", "y"), ("https://b.test/", "x")] {
            assert!(db.add_tag(u, t));
        }
        assert_eq!(db.all_tags(), ["x", "y"]);
        assert_eq!(db.tags_for("https://a.test/"), ["x", "y"]);
        let urls = |terms: &str, tags: &[&str], off: usize, lim: usize| -> Vec<String> {
            let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
            db.search_bookmarks_with_tags(terms, &tags, off, lim).into_iter().map(|r| r.0).collect()
        };
        assert_eq!(urls("", &["x"], 0, 50), ["https://a.test/", "https://b.test/"]);
        assert_eq!(urls("", &["x", "y"], 0, 50), ["https://a.test/"], "every tag must match");
        assert_eq!(urls("", &["x", "x"], 0, 50), ["https://a.test/", "https://b.test/"], "a repeated tag is one tag");
        assert_eq!(urls("b", &["x"], 0, 50), ["https://b.test/"]);
        assert_eq!(urls("gamma beta", &[], 0, 50), ["https://b.test/", "https://c.test/"], "terms split on whitespace, any matches");
        assert_eq!(urls("", &["x"], 1, 50), ["https://b.test/"]);
        assert_eq!(urls("", &["x"], 0, 1), ["https://a.test/"]);
        // Pattern characters in a term are literal and bound, never SQL.
        assert!(urls("%", &["x"], 0, 50).is_empty());
        assert!(urls("'; drop table BrowserBookmarks; --", &[], 0, 50).is_empty());
        assert_eq!(count(&db, "BrowserBookmarks"), 3);
    }

    #[test]
    fn tags_are_bounded_and_clean() {
        let db = Db::open_in_memory();
        db.add_bookmark("u", None, 1);
        assert!(db.add_tag("u", &"z".repeat(500)));
        assert_eq!(db.tags_for("u")[0].chars().count(), MAX_TAG);
        assert!(!db.add_tag("u", "  "), "blank");
        assert!(!db.add_tag("u", "\u{202e}\u{200b}"), "nothing left after the sanitizer");
        for i in 0..MAX_TAGS_PER {
            let _ = db.add_tag("u", &format!("t{i}"));
        }
        assert_eq!(db.tags_for("u").len(), MAX_TAGS_PER);
        assert!(!db.add_tag("u", "one-more"));
        // A tag row an imported file holds is shown clean.
        db.conn().execute("insert into BrowserBookmarkTags (bookmarkRowId, tag) values ((select rowid from BrowserBookmarks where url = 'u'), ?1)", ["ev\u{202e}il"]).unwrap();
        assert!(db.tags_for("u").iter().all(|t| !t.contains('\u{202e}')));
    }
}
