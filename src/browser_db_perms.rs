//! The `BrowserPermissions` table (`BP`, iTerm2's `Database/BrowserPermissions.swift`) and its
//! queries, as an `impl Db` block over 13x2's `conn()`.
//!
//! The table is the person's own data. Nothing imports, exports, syncs or fills it (settings
//! Import All, remote prefs, it2 / API and dynamic profiles never name it); only the permission
//! dialog and the person's own Reset / Revoke write here. A stored grant is per origin, bounded
//! (row count and string lengths), and revocable. A row the readers cannot vouch for (a decision
//! that is not exactly "granted" or "denied", an unknown type, an origin that is not the canonical
//! `normalize_origin` form or is too long, a date that is not a finite number) is skipped, so
//! damaged or hand-edited data reads as no decision, never as consent (deny).

use crate::browser_db::Db;
use crate::browser_perms::{normalize_origin, Decision, PermissionType};
use rusqlite::types::Value;
use rusqlite::{params, Row};

/// iTerm2's text, byte for byte (the Swift literal with its indentation stripped).
pub const SCHEMA: &str = "create table if not exists BrowserPermissions
    (origin text not null,
     permissionType text not null,
     decision text not null,
     createdAt integer not null,
     updatedAt integer not null,
     PRIMARY KEY (origin, permissionType));

CREATE INDEX IF NOT EXISTS idx_browser_permissions_origin ON BrowserPermissions(origin);
CREATE INDEX IF NOT EXISTS idx_browser_permissions_type ON BrowserPermissions(permissionType);
CREATE INDEX IF NOT EXISTS idx_browser_permissions_created ON BrowserPermissions(createdAt DESC);";

/// An origin longer than this (bytes) is neither stored nor read.
pub const MAX_ORIGIN: usize = 300;
/// At most this many saved permissions; a new origin and type is refused when the table is full
/// (an update of an existing row is still allowed). Six types, so about 1600 origins.
pub const MAX_PERMISSIONS: i64 = 10_000;
/// A listing never returns more than this many rows.
const MAX_LISTED: i64 = MAX_PERMISSIONS;

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRow {
    pub origin: String,
    pub kind: PermissionType,
    pub decision: Decision,
    pub created_at: f64,
    pub updated_at: f64,
}

const COLUMNS: &str = "origin, permissionType, decision, createdAt, updatedAt";

fn number(v: Value) -> Option<f64> {
    let n = match v {
        Value::Integer(i) => i as f64,
        Value::Real(r) => r,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// The row as the readers accept it, None for anything they cannot vouch for.
fn read_row(r: &Row) -> Option<PermissionRow> {
    let origin: String = r.get(0).ok()?;
    let kind: String = r.get(1).ok()?;
    let decision: String = r.get(2).ok()?;
    if origin.len() > MAX_ORIGIN || normalize_origin(&origin).as_deref() != Some(origin.as_str()) {
        return None;
    }
    Some(PermissionRow {
        origin,
        kind: PermissionType::parse(&kind)?,
        decision: Decision::parse(&decision)?,
        created_at: number(r.get(3).ok()?)?,
        updated_at: number(r.get(4).ok()?)?,
    })
}

/// An origin the table may be asked about: canonical and bounded.
fn usable(origin: &str) -> bool {
    origin.len() <= MAX_ORIGIN && normalize_origin(origin).as_deref() == Some(origin)
}

impl Db {
    pub fn get_permission(&self, origin: &str, t: PermissionType) -> Option<PermissionRow> {
        if !usable(origin) {
            return None;
        }
        let sql = format!("select {COLUMNS} from BrowserPermissions where origin = ?1 and permissionType = ?2");
        self.conn().query_row(&sql, params![origin, t.raw()], |r| Ok(read_row(r))).ok().flatten()
    }

    /// Select, then update the decision and `updatedAt` (keeping `createdAt`) or insert, in one
    /// transaction (`BD:773-790`). False when the origin is not usable, the table is full for a
    /// new row, the date is not finite, or the write failed.
    pub fn save_permission(&self, origin: &str, t: PermissionType, d: Decision, now: f64) -> bool {
        if !usable(origin) || !now.is_finite() {
            return false;
        }
        let Ok(tx) = self.conn().unchecked_transaction() else { return false };
        let exists: bool = tx
            .query_row("select count(*) > 0 from BrowserPermissions where origin = ?1 and permissionType = ?2", params![origin, t.raw()], |r| r.get(0))
            .unwrap_or(false);
        let wrote = if exists {
            tx.execute("update BrowserPermissions set decision = ?1, updatedAt = ?2 where origin = ?3 and permissionType = ?4", params![d.raw(), now, origin, t.raw()])
        } else {
            let full: bool = tx.query_row("select count(*) >= ?1 from BrowserPermissions", params![MAX_PERMISSIONS], |r| r.get(0)).unwrap_or(true);
            if full {
                return false;
            }
            tx.execute(
                "insert or replace into BrowserPermissions (origin, permissionType, decision, createdAt, updatedAt) values (?1, ?2, ?3, ?4, ?5)",
                params![origin, t.raw(), d.raw(), now, now],
            )
        };
        wrote == Ok(1) && tx.commit().is_ok()
    }

    /// Reset deletes the row (`BD:765-771`).
    pub fn reset_permission(&self, origin: &str, t: PermissionType) {
        let _ = self.conn().execute("delete from BrowserPermissions where origin = ?1 and permissionType = ?2", params![origin, t.raw()]);
    }

    /// Revoke deletes the row (`BD:837-849`); true when a row went.
    pub fn revoke_permission(&self, origin: &str, t: PermissionType) -> bool {
        self.conn().execute("delete from BrowserPermissions where origin = ?1 and permissionType = ?2", params![origin, t.raw()]).is_ok_and(|n| n > 0)
    }

    /// Deletes every row of the origin (`BP:230-232`); true when any went.
    pub fn revoke_all_permissions(&self, origin: &str) -> bool {
        self.conn().execute("delete from BrowserPermissions where origin = ?1", params![origin]).is_ok_and(|n| n > 0)
    }

    /// Every saved permission, newest `createdAt` first (`BP:218-220`).
    pub fn all_permissions(&self) -> Vec<PermissionRow> {
        self.list(None)
    }

    /// One origin's saved permissions, newest `createdAt` first (`BP:210-212`).
    pub fn permissions_for_origin(&self, origin: &str) -> Vec<PermissionRow> {
        if !usable(origin) {
            return Vec::new();
        }
        self.list(Some(origin))
    }

    fn list(&self, origin: Option<&str>) -> Vec<PermissionRow> {
        let filter = if origin.is_some() { "where origin = ?1" } else { "where ?1 is null" };
        let sql = format!("select {COLUMNS} from BrowserPermissions {filter} order by createdAt desc, rowid desc limit {MAX_LISTED}");
        let Ok(mut st) = self.conn().prepare(&sql) else { return Vec::new() };
        let Ok(rows) = st.query_map(params![origin], |r| Ok(read_row(r))) else { return Vec::new() };
        rows.flatten().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_perms::PermissionType::*;
    use Decision::*;

    const A: &str = "https://a.b";
    const B: &str = "https://c.d:8443";

    #[test]
    fn permission_schema_follows_iterm2() {
        let db = Db::open_in_memory();
        let c = db.conn();
        let mut s = c.prepare("pragma table_info(BrowserPermissions)").unwrap();
        let cols: Vec<(String, String, i64, i64)> =
            s.query_map([], |r| Ok((r.get(1)?, r.get::<_, String>(2)?.to_lowercase(), r.get(3)?, r.get(5)?))).unwrap().map(Result::unwrap).collect();
        let want = [("origin", "text", 1), ("permissionType", "text", 2), ("decision", "text", 0), ("createdAt", "integer", 0), ("updatedAt", "integer", 0)];
        assert_eq!(cols.len(), 5);
        for (got, w) in cols.iter().zip(want) {
            assert_eq!((got.0.as_str(), got.1.as_str(), got.2, got.3), (w.0, w.1, 1, w.2));
        }
        let mut s = c.prepare("pragma index_list(BrowserPermissions)").unwrap();
        let names: Vec<String> = s.query_map([], |r| r.get(1)).unwrap().map(Result::unwrap).collect();
        for n in ["idx_browser_permissions_origin", "idx_browser_permissions_type", "idx_browser_permissions_created"] {
            assert!(names.iter().any(|x| x == n), "{n}");
        }
        let resume = "create table if not exists BrowserPermissions\n    (origin text not null,\n     permissionType text not null,\n     decision text not null,\n     createdAt integer not null,\n     updatedAt integer not null,\n     PRIMARY KEY (origin, permissionType));\n\nCREATE INDEX IF NOT EXISTS idx_browser_permissions_origin ON BrowserPermissions(origin);\nCREATE INDEX IF NOT EXISTS idx_browser_permissions_type ON BrowserPermissions(permissionType);\nCREATE INDEX IF NOT EXISTS idx_browser_permissions_created ON BrowserPermissions(createdAt DESC);";
        assert_eq!(SCHEMA, resume);
        assert!(crate::browser_db::EXTRA_SCHEMAS.contains(&SCHEMA));
    }

    #[test]
    fn rows_survive_a_second_open_of_one_file() {
        let dir = std::env::temp_dir().join(format!("termurion-perms-{}", uuid::Uuid::new_v4()));
        let file = dir.join("browserdb-u.sqlite");
        {
            let db = Db::open(&file).unwrap();
            assert!(db.save_permission(A, Camera, Granted, 5.0));
        }
        let db = Db::open(&file).unwrap();
        assert_eq!(db.get_permission(A, Camera).map(|r| r.decision), Some(Granted));
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn permission_queries_follow_iterm2() {
        let db = Db::open_in_memory();
        assert!(db.save_permission(A, Camera, Granted, 100.5));
        let r = db.get_permission(A, Camera).unwrap();
        assert_eq!((r.created_at, r.updated_at, r.decision), (100.5, 100.5, Granted));
        assert!(db.save_permission(A, Camera, Denied, 200.0));
        let r = db.get_permission(A, Camera).unwrap();
        assert_eq!((r.created_at, r.updated_at, r.decision), (100.5, 200.0, Denied));
        assert_eq!(db.get_permission(A, Microphone), None);
        db.reset_permission(A, Camera);
        assert_eq!(db.get_permission(A, Camera), None);

        assert!(db.save_permission(A, Camera, Granted, 10.0));
        assert!(db.save_permission(A, Microphone, Granted, 30.0));
        assert!(db.save_permission(B, Camera, Denied, 20.0));
        let all: Vec<f64> = db.all_permissions().iter().map(|r| r.created_at).collect();
        assert_eq!(all, vec![30.0, 20.0, 10.0]);
        let mine: Vec<(PermissionType, f64)> = db.permissions_for_origin(A).iter().map(|r| (r.kind, r.created_at)).collect();
        assert_eq!(mine, vec![(Microphone, 30.0), (Camera, 10.0)]);
        assert!(db.revoke_all_permissions(A));
        assert!(!db.revoke_all_permissions(A));
        assert_eq!(db.all_permissions().len(), 1);
        assert!(db.revoke_permission(B, Camera));
        assert!(!db.revoke_permission(B, Camera));
        assert!(db.all_permissions().is_empty());
    }

    #[test]
    fn a_row_the_readers_cannot_vouch_for_reads_as_no_decision() {
        let db = Db::open_in_memory();
        let put = |o: &str, t: &str, d: &str, c: Value| {
            db.conn().execute("insert into BrowserPermissions values (?1, ?2, ?3, ?4, ?4)", params![o, t, d, c]).unwrap();
        };
        let camera = Camera.raw();
        put(A, camera, "maybe", Value::Integer(1));
        put("https://e.f", camera, "granted", Value::Integer(1)); // good
        put("https://g.h", "teleport", "granted", Value::Integer(1));
        put("HTTPS://I.J", camera, "granted", Value::Integer(1)); // not canonical
        put("about://", camera, "granted", Value::Integer(1));
        put("data://", camera, "granted", Value::Integer(1));
        put(&format!("https://{}.com", "a".repeat(400)), camera, "granted", Value::Integer(1));
        put("https://k.l", camera, "Granted", Value::Integer(1)); // exact only
        put("https://m.n", camera, "granted", Value::Real(f64::INFINITY));
        put("https://o.p", camera, "granted", Value::Text("soon".into()));
        assert_eq!(db.get_permission(A, Camera), None);
        assert_eq!(db.get_permission("https://k.l", Camera), None);
        assert_eq!(db.get_permission("https://m.n", Camera), None);
        assert_eq!(db.get_permission("https://o.p", Camera), None);
        let all = db.all_permissions();
        assert_eq!(all.len(), 1, "{all:?}");
        assert_eq!(all[0].origin, "https://e.f");
        assert!(db.permissions_for_origin("https://I.J").is_empty());
        // INTEGER dates read too.
        assert_eq!(all[0].created_at, 1.0);
    }

    #[test]
    fn saves_are_bounded_and_canonical() {
        let db = Db::open_in_memory();
        assert!(!db.save_permission("https://A.B", Camera, Granted, 1.0), "not canonical");
        assert!(!db.save_permission("about://", Camera, Granted, 1.0));
        assert!(!db.save_permission("data://", Camera, Granted, 1.0));
        assert!(!db.save_permission("", Camera, Granted, 1.0));
        assert!(!db.save_permission(&format!("https://{}.com", "a".repeat(400)), Camera, Granted, 1.0));
        assert!(!db.save_permission(A, Camera, Granted, f64::NAN));
        assert!(!db.save_permission(A, Camera, Granted, f64::INFINITY));
        assert!(db.all_permissions().is_empty());
        // Full: a new row is refused, an existing one still updates.
        db.conn()
            .execute(
                "insert into BrowserPermissions select 'https://h' || value || '.com', 'camera', 'denied', 1, 1 from (with recursive n(value) as (select 1 union all select value + 1 from n where value < ?1) select value from n)",
                params![MAX_PERMISSIONS],
            )
            .unwrap();
        assert!(!db.save_permission(A, Camera, Granted, 1.0));
        assert!(db.get_permission(A, Camera).is_none());
        assert!(db.save_permission("https://h1.com", Camera, Granted, 2.0));
        assert_eq!(db.get_permission("https://h1.com", Camera).unwrap().decision, Granted);
    }

    #[test]
    fn text_a_page_chose_is_a_bound_parameter() {
        let db = Db::open_in_memory();
        assert!(db.save_permission(A, Camera, Granted, 1.0));
        assert!(db.permissions_for_origin("https://a.b' or '1'='1").is_empty());
        assert!(!db.revoke_all_permissions("x' or '1'='1"));
        assert_eq!(db.all_permissions().len(), 1);
    }

    #[test]
    fn no_import_export_or_sync_path_names_the_table() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if !name.ends_with(".rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let body = text.split("#[cfg(test)]").next().unwrap_or("");
                if body.contains("BrowserPermissions") || body.contains("browser_db_perms") || body.contains("save_permission") {
                    hits.push(name);
                }
            }
        }
        hits.sort();
        // Only the table itself and the place that registers it, the browser app, and the
        // permissions page (Task 9) that LISTS the person's own rows for display (browser_pages.rs
        // routes the page's typed messages, browser_perms.rs filters and pages the rows): nothing
        // there writes a row, reads a file or sends anything.
        let allowed = ["browser_db.rs", "browser_db_perms.rs", "browser_pages.rs", "browser_perms.rs", "browser_perms_app.rs", "lib.rs"];
        for h in &hits {
            assert!(allowed.contains(&h.as_str()), "{h} names the permissions table: grants are never imported, exported or synced");
        }
    }
}
