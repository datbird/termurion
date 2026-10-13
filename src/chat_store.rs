//! The AI chat database: iTerm2's `chatdb.sqlite` schema (`AITerm/ChatDatabase.swift`,
//! `Chat+Database.swift`, `Message+Database.swift`, `ChatBlob.swift`), its migrations, the rows,
//! the search and the one-instance lock. Pure over rusqlite. Every value is a bound parameter;
//! no SQL text is built from data, and nothing here logs a message's contents.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension, Row};

use crate::chat_message::{decode_content, encode_content, new_id, Content, Message, Participant};

pub const FILE_NAME: &str = "chatdb.sqlite";
/// The lock file beside the database (iTerm2's named lock, `DB:96-110`).
pub const LOCK_NAME: &str = "chatdb-lock";
pub const OPEN_FAILED_HEADING: &str = "Error";
pub const OPEN_FAILED: &str = "AI Chat could not open because of a problem loading the database. Verify there is only one instance of Tandem running.";

/// `%APPDATA%\Termurion\chatdb.sqlite`, beside the settings file.
pub fn default_path() -> PathBuf {
    crate::globals::default_path().with_file_name(FILE_NAME)
}

// The tables, verbatim (`Chat+Database.swift:28-45`, `Message+Database.swift:30-43`,
// `CompanionAlert+Database.swift:45-55`, `ChatBlob.swift:120-133`).
const CHAT_SCHEMA: &str = "create table if not exists Chat
    (uuid text,
     title text not null,
     creationDate integer not null,
     lastModifiedDate integer not null,
     orchestrationEnabled integer DEFAULT 0,
     sessionGuid text,
     browserSessionGuid text,
     permissions text,
     vectorStore text,
     modelName text,
     claimedScopes text,
     watchers text,
     icon blob,
     blobProtocol text)";
const MESSAGE_SCHEMA: &str = "create table if not exists Message
    (seq integer primary key autoincrement,
     uniqueID text,
     author text not null,
     chatID text not null,
     content text not null,
     sentDate integer not null,
     responseID text,
     agentReasoning text,
     firstBlobRef text)";
const ALERT_SCHEMA: &str = "create table if not exists CompanionAlert
    (seq integer primary key autoincrement,
     uniqueID text,
     threadKey text not null,
     title text not null,
     body text not null,
     createdDate integer not null)";
const BLOB_SCHEMA: &str = "create table if not exists ChatBlob
    (seq integer primary key autoincrement,
     blobID text,
     chatID text not null,
     blobProtocol integer not null,
     role text not null,
     payload blob not null,
     responseID text,
     tokenCount text,
     wireFormatVersion integer not null default 0)";

// Columns added when missing, in iTerm2's order (`Chat+Database.swift:47-80`,
// `Message+Database.swift:45-56`, `ChatBlob.swift:139-152`).
const CHAT_MIGRATIONS: [(&str, &str); 9] = [
    ("vectorStore", "ALTER TABLE Chat ADD COLUMN vectorStore text"),
    ("modelName", "ALTER TABLE Chat ADD COLUMN modelName text"),
    ("sessionGuid", "ALTER TABLE Chat ADD COLUMN sessionGuid text"),
    ("browserSessionGuid", "ALTER TABLE Chat ADD COLUMN browserSessionGuid text"),
    ("claimedScopes", "ALTER TABLE Chat ADD COLUMN claimedScopes text"),
    ("watchers", "ALTER TABLE Chat ADD COLUMN watchers text"),
    ("icon", "ALTER TABLE Chat ADD COLUMN icon blob"),
    ("orchestrationEnabled", "ALTER TABLE Chat ADD COLUMN orchestrationEnabled integer DEFAULT 0"),
    ("blobProtocol", "ALTER TABLE Chat ADD COLUMN blobProtocol text"),
];
const MESSAGE_MIGRATIONS: [(&str, &str); 3] = [
    ("responseID", "ALTER TABLE Message ADD COLUMN responseID text"),
    ("agentReasoning", "ALTER TABLE Message ADD COLUMN agentReasoning text"),
    ("firstBlobRef", "ALTER TABLE Message ADD COLUMN firstBlobRef text"),
];
const BLOB_MIGRATIONS: [(&str, &str); 2] = [
    ("tokenCount", "ALTER TABLE ChatBlob ADD COLUMN tokenCount text"),
    ("wireFormatVersion", "ALTER TABLE ChatBlob ADD COLUMN wireFormatVersion integer not null default 0"),
];

/// The Message table rebuilt with `seq` (SQLite cannot add an AUTOINCREMENT key with ALTER
/// TABLE), rows copied in rowid order, the NOT NULL columns coalesced (`DB:201-259`).
const MESSAGE_ADD_SEQ: &str = "create table Message_new
    (seq integer primary key autoincrement,
     uniqueID text,
     author text not null,
     chatID text not null,
     content text not null,
     sentDate integer not null,
     responseID text,
     agentReasoning text,
     firstBlobRef text);
insert into Message_new (uniqueID, author, chatID, content, sentDate, responseID, agentReasoning, firstBlobRef)
    select uniqueID, coalesce(author, 'user'), coalesce(chatID, ''), coalesce(content, ''), coalesce(sentDate, 0), responseID, agentReasoning, firstBlobRef
    from Message order by rowid;
drop table Message;
alter table Message_new rename to Message;";

const MESSAGE_INDEX: &str = "create index if not exists Message_chatID_seq on Message (chatID, seq)";
const BLOB_INDEX: &str = "create index if not exists ChatBlob_chatID_seq on ChatBlob (chatID, seq)";

fn column_names(conn: &Connection, pragma: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(pragma)?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?.collect();
    names
}

/// Creates the table and adds its missing columns, in one transaction; returns the columns
/// the table had before the migrations.
fn table_step(conn: &mut Connection, schema: &str, pragma: &str, migrations: &[(&str, &str)]) -> rusqlite::Result<Vec<String>> {
    let tx = conn.transaction()?;
    tx.execute_batch(schema)?;
    let existing = column_names(&tx, pragma)?;
    for (column, sql) in migrations {
        if !existing.iter().any(|c| c == column) {
            tx.execute_batch(sql)?;
        }
    }
    tx.commit()?;
    Ok(existing)
}

fn one_step(conn: &mut Connection, sql: &str) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(sql)?;
    tx.commit()
}

/// iTerm2's `createTables` (`DB:126-199`), each step in its own transaction.
fn prepare(conn: &mut Connection) -> rusqlite::Result<()> {
    table_step(conn, CHAT_SCHEMA, "PRAGMA table_info(Chat)", &CHAT_MIGRATIONS)?;
    let message = table_step(conn, MESSAGE_SCHEMA, "PRAGMA table_info(Message)", &MESSAGE_MIGRATIONS)?;
    // After the added columns, so the copy carries them (`DB:154-160`).
    if !message.is_empty() && !message.iter().any(|c| c == "seq") {
        one_step(conn, MESSAGE_ADD_SEQ)?;
    }
    one_step(conn, MESSAGE_INDEX)?;
    one_step(conn, ALERT_SCHEMA)?;
    table_step(conn, BLOB_SCHEMA, "PRAGMA table_info(ChatBlob)", &BLOB_MIGRATIONS)?;
    one_step(conn, BLOB_INDEX)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    /// Seconds since 1970.
    pub creation_date: f64,
    pub last_modified_date: f64,
    pub orchestration_enabled: bool,
    /// The linked terminal session (the column is `sessionGuid`).
    pub terminal_session_guid: Option<String>,
    pub browser_session_guid: Option<String>,
    /// The per-chat permission overrides, Swift JSON (Task 3).
    pub permissions: String,
    pub vector_store: Option<String>,
    pub model_name: Option<String>,
    /// Stored newline-separated.
    pub claimed_scopes: Vec<String>,
    /// JSON text kept opaque (12f); an empty column reads as None.
    pub watchers: Option<String>,
    pub icon: Option<Vec<u8>>,
    /// iTerm2's `iTermAIAPI` raw value as decimal text; None until the chat's blobs exist.
    pub blob_protocol: Option<String>,
}

impl Chat {
    pub fn new(title: &str, now: f64) -> Chat {
        Chat {
            id: new_id(),
            title: title.to_string(),
            creation_date: now,
            last_modified_date: now,
            orchestration_enabled: false,
            terminal_session_guid: None,
            browser_session_guid: None,
            permissions: String::new(),
            vector_store: None,
            model_name: None,
            claimed_scopes: Vec::new(),
            watchers: None,
            icon: None,
            blob_protocol: None,
        }
    }
}

/// One wire fragment of a chat (`ChatBlob.swift`), kept for import and fork (ruling 12c-2).
#[derive(Clone, Debug, PartialEq)]
pub struct BlobRow {
    pub seq: i64,
    pub blob_id: String,
    pub chat_id: String,
    pub protocol: i64,
    /// "user", "assistant" or "tool".
    pub role: String,
    pub payload: Vec<u8>,
    pub response_id: Option<String>,
    /// Stored as text, so an unknown count stays distinct from 0.
    pub token_count: Option<i64>,
    pub wire_format_version: i64,
}

fn text_of(v: ValueRef) -> Option<String> {
    match v {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn int_of(v: ValueRef) -> Option<i64> {
    match v {
        ValueRef::Integer(i) => Some(i),
        ValueRef::Real(f) => Some(f as i64),
        ValueRef::Text(t) => std::str::from_utf8(t).ok()?.trim().parse().ok(),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

/// A date column: Tandem writes REAL seconds, iTerm2 may have left an INTEGER.
fn real_of(v: ValueRef) -> Option<f64> {
    match v {
        ValueRef::Integer(i) => Some(i as f64),
        ValueRef::Real(f) => Some(f),
        ValueRef::Text(t) => std::str::from_utf8(t).ok()?.trim().parse().ok(),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn bytes_of(v: ValueRef) -> Option<Vec<u8>> {
    match v {
        ValueRef::Blob(b) | ValueRef::Text(b) => Some(b.to_vec()),
        _ => None,
    }
}

/// A Chat row; None for a row without an id, a title or its dates (iTerm2 skips those).
fn chat_of(row: &Row) -> rusqlite::Result<Option<Chat>> {
    let (Some(id), Some(title), Some(creation_date), Some(last_modified_date)) = (text_of(row.get_ref(0)?), text_of(row.get_ref(1)?), real_of(row.get_ref(2)?), real_of(row.get_ref(3)?)) else {
        return Ok(None);
    };
    Ok(Some(Chat {
        id,
        title,
        creation_date,
        last_modified_date,
        orchestration_enabled: int_of(row.get_ref(4)?).unwrap_or(0) != 0,
        terminal_session_guid: text_of(row.get_ref(5)?),
        browser_session_guid: text_of(row.get_ref(6)?),
        permissions: text_of(row.get_ref(7)?).unwrap_or_default(),
        vector_store: text_of(row.get_ref(8)?),
        model_name: text_of(row.get_ref(9)?),
        claimed_scopes: text_of(row.get_ref(10)?).unwrap_or_default().split('\n').filter(|s| !s.is_empty()).map(str::to_string).collect(),
        watchers: text_of(row.get_ref(11)?).filter(|s| !s.is_empty()),
        icon: bytes_of(row.get_ref(12)?),
        blob_protocol: text_of(row.get_ref(13)?),
    }))
}

/// A Message row; None for a row without an id, a known author, a chat, content or a date.
fn message_of(row: &Row) -> rusqlite::Result<Option<Message>> {
    let seq = int_of(row.get_ref(0)?).unwrap_or(0);
    let author = text_of(row.get_ref(2)?).and_then(|a| Participant::parse(&a));
    let (Some(unique_id), Some(author), Some(chat_id), Some(content), Some(sent_date)) = (text_of(row.get_ref(1)?), author, text_of(row.get_ref(3)?), text_of(row.get_ref(4)?), real_of(row.get_ref(5)?)) else {
        return Ok(None);
    };
    Ok(Some(Message {
        seq,
        unique_id,
        author,
        chat_id,
        content: decode_content(&content),
        sent_date,
        response_id: text_of(row.get_ref(6)?),
        agent_reasoning: text_of(row.get_ref(7)?),
        first_blob_ref: text_of(row.get_ref(8)?),
    }))
}

fn blob_of(row: &Row) -> rusqlite::Result<Option<BlobRow>> {
    let (Some(blob_id), Some(chat_id), Some(role), Some(payload)) = (text_of(row.get_ref(1)?), text_of(row.get_ref(2)?), text_of(row.get_ref(4)?), bytes_of(row.get_ref(5)?)) else {
        return Ok(None);
    };
    Ok(Some(BlobRow {
        seq: int_of(row.get_ref(0)?).unwrap_or(0),
        blob_id,
        chat_id,
        protocol: int_of(row.get_ref(3)?).unwrap_or(0),
        role,
        payload,
        response_id: text_of(row.get_ref(6)?),
        token_count: int_of(row.get_ref(7)?),
        wire_format_version: int_of(row.get_ref(8)?).unwrap_or(0),
    }))
}

/// Runs a query and keeps the rows that read.
fn rows<T, P: rusqlite::Params>(conn: &Connection, sql: &str, params: P, read: fn(&Row) -> rusqlite::Result<Option<T>>) -> rusqlite::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut out = Vec::new();
    let mut found = stmt.query(params)?;
    while let Some(row) = found.next()? {
        if let Some(item) = read(row)? {
            out.push(item);
        }
    }
    Ok(out)
}

fn blobs_in(conn: &Connection, chat: &str) -> rusqlite::Result<Vec<BlobRow>> {
    rows(
        conn,
        "select seq, blobID, chatID, blobProtocol, role, payload, responseID, tokenCount, wireFormatVersion from ChatBlob where chatID = ? order by seq asc",
        [chat],
        blob_of,
    )
}

fn insert_blob(conn: &Connection, b: &BlobRow) -> rusqlite::Result<i64> {
    conn.execute(
        "insert into ChatBlob (blobID, chatID, blobProtocol, role, payload, responseID, tokenCount, wireFormatVersion) values (?, ?, ?, ?, ?, ?, ?, ?)",
        params![b.blob_id, b.chat_id, b.protocol, b.role, b.payload, b.response_id, b.token_count.map(|t| t.to_string()), b.wire_format_version],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The search's SQL for `tokens` terms: one bound LIKE per term, all required, newest first.
/// Built from fixed text and the count only.
pub(crate) fn search_sql(tokens: usize) -> String {
    let mut sql = String::from("select chatID, uniqueID from Message where ");
    for i in 0..tokens {
        if i > 0 {
            sql.push_str(" and ");
        }
        sql.push_str("content like ? escape '\\'");
    }
    sql.push_str(" order by seq desc");
    sql
}

/// `%token%` with LIKE's `%`, `_` and the escape character itself escaped (ruling 10).
fn like_pattern(token: &str) -> String {
    let mut out = String::from("%");
    for c in token.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The open chat database. The lock file is held for the store's life.
pub struct Store {
    conn: Connection,
    /// Held, never read: dropping the store releases the lock.
    _lock: Option<File>,
}

impl Store {
    /// Takes the lock, opens the database at `path` (creating its folder), checks it and brings
    /// the schema up to date. Any failure leaves no store; the caller shows `OPEN_FAILED`, never
    /// the error text (it can name the path).
    pub fn open(path: &Path, lock_path: &Path) -> Result<Store, String> {
        for dir in [path.parent(), lock_path.parent()].into_iter().flatten() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(err)?;
            }
        }
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(lock_path).map_err(err)?;
        lock.try_lock().map_err(err)?;
        // No URI parsing: the path is only ever `default_path()`.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(path, flags).map_err(err)?;
        // `open` (`iTermDatabase.m:255-329`): a database that fails the check is closed before
        // anything is written to it. iTerm2's one-in-a-hundred full `integrity_check` and its
        // slow-check dialog are left out (Decision).
        let check = conn.query_row("pragma quick_check", [], |row| row.get_ref(0).map(text_of)).map_err(err)?;
        if check.as_deref() != Some("ok") {
            return Err("the chat database failed its integrity check".to_string());
        }
        // An imported file's triggers and views cannot call SQL functions with side effects.
        conn.pragma_update(None, "trusted_schema", false).map_err(err)?;
        prepare(&mut conn).map_err(err)?;
        Ok(Store { conn, _lock: Some(lock) })
    }

    /// A private database for tests; no lock.
    pub fn open_in_memory() -> Result<Store, String> {
        let mut conn = Connection::open_in_memory().map_err(err)?;
        conn.pragma_update(None, "trusted_schema", false).map_err(err)?;
        prepare(&mut conn).map_err(err)?;
        Ok(Store { conn, _lock: None })
    }

    /// Makes every later Message insert or update fail, as a full disk would (tests only).
    #[cfg(test)]
    pub(crate) fn refuse_message_writes(&self) {
        self.conn
            .execute_batch(
                "create temp trigger refuse_insert before insert on Message begin select raise(abort, 'refused'); end;
                 create temp trigger refuse_update before update on Message begin select raise(abort, 'refused'); end;",
            )
            .unwrap();
    }

    /// Runs `f` as one unit: every write it makes lands, or none does (a savepoint, so the
    /// store's own multi-row writes nest inside it).
    pub fn atomically<T>(&mut self, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
        self.unit_begin()?;
        let result = f(self).and_then(|v| self.unit_settle().map(|()| v));
        if result.is_err() {
            self.unit_abort();
        }
        result
    }

    /// Starts a unit of writes by hand (`atomically`'s savepoint), for a caller that settles
    /// it before it acts on the writes (a publish settles before it delivers).
    pub fn unit_begin(&mut self) -> Result<(), String> {
        self.conn.execute_batch("savepoint atomically").map_err(err)
    }

    /// Makes the unit's writes final.
    pub fn unit_settle(&mut self) -> Result<(), String> {
        self.conn.execute_batch("release atomically").map_err(err)
    }

    /// Takes back every write of an unsettled unit.
    pub fn unit_abort(&mut self) {
        let _ = self.conn.execute_batch("rollback to atomically; release atomically");
    }

    /// Makes every later Chat update fail (tests only).
    #[cfg(test)]
    pub(crate) fn refuse_chat_updates(&self) {
        self.conn.execute_batch("create temp trigger refuse_chat_update before update on Chat begin select raise(abort, 'refused'); end;").unwrap();
    }

    /// Every chat, most recently modified first.
    pub fn chats(&self) -> Result<Vec<Chat>, String> {
        rows(
            &self.conn,
            "select uuid, title, creationDate, lastModifiedDate, orchestrationEnabled, sessionGuid, browserSessionGuid, permissions, vectorStore, modelName, claimedScopes, watchers, icon, blobProtocol from Chat order by lastModifiedDate desc, rowid desc",
            [],
            chat_of,
        )
        .map_err(err)
    }

    /// (chat id, guid) of every chat with a linked terminal session, for the indicators and for a
    /// closing pane; a blank guid is no link.
    pub fn linked_sessions(&self) -> Result<Vec<(String, String)>, String> {
        rows(&self.conn, "select uuid, sessionGuid from Chat where sessionGuid is not null and sessionGuid != ''", [], |row| Ok(Some((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))).map_err(err)
    }

    pub fn chat(&self, id: &str) -> Result<Option<Chat>, String> {
        let found = rows(
            &self.conn,
            "select uuid, title, creationDate, lastModifiedDate, orchestrationEnabled, sessionGuid, browserSessionGuid, permissions, vectorStore, modelName, claimedScopes, watchers, icon, blobProtocol from Chat where uuid = ? limit 1",
            [id],
            chat_of,
        )
        .map_err(err)?;
        Ok(found.into_iter().next())
    }

    pub fn add_chat(&mut self, c: &Chat) -> Result<(), String> {
        self.conn
            .execute(
                "insert into Chat (uuid, title, creationDate, lastModifiedDate, orchestrationEnabled, sessionGuid, browserSessionGuid, permissions, vectorStore, modelName, claimedScopes, watchers, icon, blobProtocol)
                 values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    c.id,
                    c.title,
                    c.creation_date,
                    c.last_modified_date,
                    c.orchestration_enabled as i64,
                    c.terminal_session_guid,
                    c.browser_session_guid,
                    c.permissions,
                    c.vector_store,
                    c.model_name,
                    c.claimed_scopes.join("\n"),
                    c.watchers.as_deref().unwrap_or(""),
                    c.icon,
                    c.blob_protocol,
                ],
            )
            .map(|_| ())
            .map_err(err)
    }

    /// Rewrites every column of the chat's row (`Chat+Database.swift:132-167`).
    pub fn update_chat(&mut self, c: &Chat) -> Result<(), String> {
        self.conn
            .execute(
                "update Chat set title = ?, creationDate = ?, lastModifiedDate = ?, orchestrationEnabled = ?, sessionGuid = ?, browserSessionGuid = ?, permissions = ?,
                 vectorStore = ?, modelName = ?, claimedScopes = ?, watchers = ?, icon = ?, blobProtocol = ? where uuid = ?",
                params![
                    c.title,
                    c.creation_date,
                    c.last_modified_date,
                    c.orchestration_enabled as i64,
                    c.terminal_session_guid,
                    c.browser_session_guid,
                    c.permissions,
                    c.vector_store,
                    c.model_name,
                    c.claimed_scopes.join("\n"),
                    c.watchers.as_deref().unwrap_or(""),
                    c.icon,
                    c.blob_protocol,
                    c.id,
                ],
            )
            .map(|_| ())
            .map_err(err)
    }

    /// Moves the chat to the front of the list (`LM:121-128`).
    pub fn bump(&mut self, id: &str, now: f64) -> Result<(), String> {
        self.conn.execute("update Chat set lastModifiedDate = ? where uuid = ?", params![now, id]).map(|_| ()).map_err(err)
    }

    /// Trims the title; a blank or unchanged title (or an unknown chat) writes nothing and
    /// answers false. A rename clears the icon, which is drawn from the title (`LM:130-157`).
    pub fn rename(&mut self, id: &str, title: &str) -> Result<bool, String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        match self.chat(id)? {
            Some(chat) if chat.title != trimmed => {}
            _ => return Ok(false),
        }
        self.conn.execute("update Chat set title = ?, icon = NULL where uuid = ?", params![trimmed, id]).map_err(err)?;
        Ok(true)
    }

    /// Deletes the chat's messages, then its blobs, then the chat, in one transaction.
    pub fn delete_chat(&mut self, id: &str) -> Result<(), String> {
        let tx = self.conn.savepoint().map_err(err)?;
        tx.execute("delete from Message where chatID = ?", [id]).map_err(err)?;
        tx.execute("delete from ChatBlob where chatID = ?", [id]).map_err(err)?;
        tx.execute("delete from Chat where uuid = ?", [id]).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// The chat's messages in seq order.
    pub fn messages(&self, chat: &str) -> Result<Vec<Message>, String> {
        rows(
            &self.conn,
            "select seq, uniqueID, author, chatID, content, sentDate, responseID, agentReasoning, firstBlobRef from Message where chatID = ? order by seq",
            [chat],
            message_of,
        )
        .map_err(err)
    }

    pub fn message(&self, unique_id: &str) -> Result<Option<Message>, String> {
        let found = rows(
            &self.conn,
            "select seq, uniqueID, author, chatID, content, sentDate, responseID, agentReasoning, firstBlobRef from Message where uniqueID = ? order by seq limit 1",
            [unique_id],
            message_of,
        )
        .map_err(err)?;
        Ok(found.into_iter().next())
    }

    /// The unique id of the chat's highest-seq message.
    pub fn newest_id(&self, chat: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("select uniqueID from Message where chatID = ? order by seq desc limit 1", [chat], |row| row.get_ref(0).map(text_of))
            .optional()
            .map(Option::flatten)
            .map_err(err)
    }

    /// Stores the message's eight columns (`Message+Database.swift:116-143`) and answers its seq.
    pub fn add_message(&mut self, m: &Message) -> Result<i64, String> {
        self.conn
            .execute(
                "insert into Message (uniqueID, author, chatID, content, sentDate, responseID, agentReasoning, firstBlobRef) values (?, ?, ?, ?, ?, ?, ?, ?)",
                params![m.unique_id, m.author.as_str(), m.chat_id, encode_content(&m.content), m.sent_date, m.response_id, m.agent_reasoning, m.first_blob_ref],
            )
            .map_err(err)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Rewrites one message's content; false when no row has that id.
    pub fn replace_content(&mut self, unique_id: &str, c: &Content) -> Result<bool, String> {
        let changed = self.conn.execute("update Message set content = ? where uniqueID = ?", params![encode_content(c), unique_id]).map_err(err)?;
        Ok(changed > 0)
    }

    /// Appends to a message's `agentReasoning` (ruling I-3); false when no row has that id.
    pub fn append_reasoning(&mut self, unique_id: &str, text: &str) -> Result<bool, String> {
        let changed = self.conn.execute("update Message set agentReasoning = coalesce(agentReasoning, '') || ? where uniqueID = ?", params![text, unique_id]).map_err(err)?;
        Ok(changed > 0)
    }

    pub fn remove_messages(&mut self, chat: &str, ids: &[String]) -> Result<(), String> {
        let tx = self.conn.savepoint().map_err(err)?;
        for id in ids {
            tx.execute("delete from Message where chatID = ? and uniqueID = ?", params![chat, id]).map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    /// Drops the chat's blobs: a deletion breaks the append-only rounds they describe, so the
    /// next capture starts again (`ChatListModel.swift:245-264`).
    pub fn clear_blobs(&mut self, chat: &str) -> Result<(), String> {
        self.conn.execute("delete from ChatBlob where chatID = ?", [chat]).map(|_| ()).map_err(err)
    }

    /// Stores a blob and answers its seq.
    pub fn add_blob(&mut self, b: &BlobRow) -> Result<i64, String> {
        insert_blob(&self.conn, b).map_err(err)
    }

    /// The chat's blobs oldest first.
    pub fn blobs(&self, chat: &str) -> Result<Vec<BlobRow>, String> {
        blobs_in(&self.conn, chat).map_err(err)
    }

    /// A fork's blobs (`ChatWindowController.swift:944-979`): when the retained messages'
    /// `firstBlobRef`s, in order, are exactly the first blob ids of `from`
    /// (`ChatBlobAssembler.forkBlobPrefix`), those blobs are copied into `to`, each with a
    /// fresh blob id, and `from`'s `blobProtocol` goes with them; any gap copies nothing.
    /// Answers the old-to-new blob id map (empty when nothing was copied), with which the
    /// caller points the forked messages at their copies (`set_first_blob_ref`). The fork's
    /// chat row must exist first, as iTerm2 creates the chat before copying; an error otherwise.
    pub fn copy_blobs(&mut self, from: &str, to: &str, retained_refs: &[String]) -> Result<HashMap<String, String>, String> {
        let tx = self.conn.savepoint().map_err(err)?;
        let fork_exists: bool = tx.query_row("select exists (select 1 from Chat where uuid = ?)", params![to], |r| r.get(0)).map_err(err)?;
        if !fork_exists {
            return Err("the fork's chat does not exist".to_string());
        }
        let blobs = blobs_in(&tx, from).map_err(err)?;
        let mut map = HashMap::new();
        if retained_refs.is_empty() || retained_refs.len() > blobs.len() || blobs.iter().zip(retained_refs).any(|(b, r)| b.blob_id != *r) {
            return Ok(map);
        }
        for blob in &blobs[..retained_refs.len()] {
            let copy = BlobRow { blob_id: new_id(), chat_id: to.to_string(), ..blob.clone() };
            insert_blob(&tx, &copy).map_err(err)?;
            map.insert(blob.blob_id.clone(), copy.blob_id);
        }
        tx.execute("update Chat set blobProtocol = (select blobProtocol from Chat where uuid = ?) where uuid = ? and exists (select 1 from Chat where uuid = ? and blobProtocol is not null)", params![from, to, from])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(map)
    }

    /// Links a message to the blob of the round it begins (`ChatListModel.setFirstBlobRef`);
    /// false when the chat has no such message.
    pub fn set_first_blob_ref(&mut self, chat: &str, unique_id: &str, blob_ref: &str) -> Result<bool, String> {
        let changed = self.conn.execute("update Message set firstBlobRef = ? where chatID = ? and uniqueID = ?", params![blob_ref, chat, unique_id]).map_err(err)?;
        Ok(changed > 0)
    }

    /// (chat id, message unique id) of every message whose stored JSON contains each
    /// whitespace-separated term, case-insensitively for ASCII, newest first (`DB:680-696`,
    /// with bound parameters; ruling 10). An empty query finds nothing.
    pub fn search(&self, query: &str) -> Result<Vec<(String, String)>, String> {
        let patterns: Vec<String> = query.split_whitespace().map(like_pattern).collect();
        if patterns.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(&search_sql(patterns.len())).map_err(err)?;
        let found = stmt
            .query_map(params_from_iter(patterns.iter()), |row| Ok((text_of(row.get_ref(0)?), text_of(row.get_ref(1)?))))
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        Ok(found.into_iter().filter_map(|(chat, id)| Some((chat?, id?))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_message::Participant;
    use rusqlite::Connection;

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let pragma = match table {
            "Chat" => "PRAGMA table_info(Chat)",
            "Message" => "PRAGMA table_info(Message)",
            "ChatBlob" => "PRAGMA table_info(ChatBlob)",
            "CompanionAlert" => "PRAGMA table_info(CompanionAlert)",
            _ => panic!("no such table"),
        };
        column_names(conn, pragma).unwrap()
    }

    fn has(conn: &Connection, kind: &str, name: &str) -> bool {
        conn.query_row("select count(*) from sqlite_master where type = ? and name = ?", rusqlite::params![kind, name], |row| row.get::<_, i64>(0)).unwrap() == 1
    }

    const CHAT_COLUMNS: [&str; 14] = [
        "uuid", "title", "creationDate", "lastModifiedDate", "orchestrationEnabled", "sessionGuid", "browserSessionGuid", "permissions", "vectorStore", "modelName", "claimedScopes",
        "watchers", "icon", "blobProtocol",
    ];
    const MESSAGE_COLUMNS: [&str; 9] = ["seq", "uniqueID", "author", "chatID", "content", "sentDate", "responseID", "agentReasoning", "firstBlobRef"];
    const BLOB_COLUMNS: [&str; 9] = ["seq", "blobID", "chatID", "blobProtocol", "role", "payload", "responseID", "tokenCount", "wireFormatVersion"];

    #[test]
    fn schema_follows_iterm2() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(columns(&store.conn, "Chat"), CHAT_COLUMNS);
        assert_eq!(columns(&store.conn, "Message"), MESSAGE_COLUMNS);
        assert_eq!(columns(&store.conn, "ChatBlob"), BLOB_COLUMNS);
        assert_eq!(columns(&store.conn, "CompanionAlert"), ["seq", "uniqueID", "threadKey", "title", "body", "createdDate"]);
        assert!(has(&store.conn, "index", "Message_chatID_seq"));
        assert!(has(&store.conn, "index", "ChatBlob_chatID_seq"));
        assert_eq!(FILE_NAME, "chatdb.sqlite");
        assert_eq!(LOCK_NAME, "chatdb-lock");
    }

    fn dump(conn: &Connection) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut stmt = conn.prepare("select type || ' ' || name || ' ' || coalesce(sql, '') from sqlite_master order by name").unwrap();
        out.extend(stmt.query_map([], |row| row.get::<_, String>(0)).unwrap().map(|r| r.unwrap()));
        let mut stmt = conn.prepare("select seq, uniqueID, author, chatID, content, sentDate from Message order by seq").unwrap();
        out.extend(
            stmt.query_map([], |row| Ok(format!("{:?} {:?} {:?} {:?} {:?} {:?}", row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, i64>(5)?)))
                .unwrap()
                .map(|r| r.unwrap()),
        );
        out
    }

    #[test]
    fn migrations_follow_iterm2() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"create table Chat (uuid text, title text not null, creationDate integer not null, lastModifiedDate integer not null, permissions text);
               insert into Chat values ('C1', 'Old', 1700000000, 1700000001, NULL);
               create table Message (uniqueID text, author text not null, chatID text not null, content text not null, sentDate integer not null);
               insert into Message values ('M2', 'user', 'C1', '{"markdown":{"_0":"b"}}', 1700000002);
               insert into Message values ('M1', 'agent', 'C1', '{"futureThing":{}}', 1700000003);
               create table ChatBlob (seq integer primary key autoincrement, blobID text, chatID text not null, blobProtocol integer not null, role text not null, payload blob not null, responseID text);
               insert into ChatBlob (blobID, chatID, blobProtocol, role, payload) values ('B1', 'C1', 2, 'user', x'00');"#,
        )
        .unwrap();
        prepare(&mut conn).unwrap();
        // iTerm2 adds nine columns in its migration order; it has no migration for `permissions`.
        assert_eq!(
            columns(&conn, "Chat"),
            [
                "uuid", "title", "creationDate", "lastModifiedDate", "permissions", "vectorStore", "modelName", "sessionGuid", "browserSessionGuid", "claimedScopes", "watchers", "icon",
                "orchestrationEnabled", "blobProtocol",
            ]
        );
        assert_eq!(columns(&conn, "Message"), MESSAGE_COLUMNS);
        assert_eq!(columns(&conn, "ChatBlob"), BLOB_COLUMNS);
        assert!(has(&conn, "table", "CompanionAlert"));
        assert!(!has(&conn, "table", "Message_new"));
        let orchestration: i64 = conn.query_row("select orchestrationEnabled from Chat where uuid = 'C1'", [], |row| row.get(0)).unwrap();
        assert_eq!(orchestration, 0);
        let before = dump(&conn);
        assert!(before.contains(&r#"1 "M2" "user" "C1" "{\"markdown\":{\"_0\":\"b\"}}" 1700000002"#.to_string()), "{before:?}");
        assert!(before.contains(&r#"2 "M1" "agent" "C1" "{\"futureThing\":{}}" 1700000003"#.to_string()), "{before:?}");
        prepare(&mut conn).unwrap();
        assert_eq!(dump(&conn), before);

        let mut store = Store { conn, _lock: None };
        let chats = store.chats().unwrap();
        assert_eq!((chats[0].id.as_str(), chats[0].title.as_str(), chats[0].creation_date, chats[0].last_modified_date), ("C1", "Old", 1700000000.0, 1700000001.0));
        assert!(!chats[0].orchestration_enabled);
        assert_eq!((chats[0].permissions.as_str(), &chats[0].claimed_scopes, &chats[0].watchers, &chats[0].blob_protocol), ("", &vec![], &None, &None));
        let messages = store.messages("C1").unwrap();
        assert_eq!(messages.iter().map(|m| (m.seq, m.unique_id.as_str())).collect::<Vec<_>>(), [(1, "M2"), (2, "M1")]);
        assert_eq!(messages[0].content, Content::Markdown("b".into()));
        assert_eq!(messages[1].content, Content::Unsupported(r#"{"futureThing":{}}"#.into()));
        let blobs = store.blobs("C1").unwrap();
        assert_eq!((blobs[0].token_count, blobs[0].wire_format_version), (None, 0));
        // A new message continues the sequence.
        let seq = store.add_message(&Message::new("C1", Participant::User, Content::Markdown("c".into()), 1.0)).unwrap();
        assert_eq!(seq, 3);
    }

    fn chat(id: &str, title: &str, t: f64) -> Chat {
        Chat { id: id.into(), ..Chat::new(title, t) }
    }

    #[test]
    fn linked_sessions_lists_only_linked_chats() {
        let mut store = Store::open_in_memory().unwrap();
        let mut a = chat("A", "First", 1.0);
        a.terminal_session_guid = Some("g1".into());
        let mut b = chat("B", "Second", 2.0);
        b.terminal_session_guid = Some(String::new());
        let mut c = chat("C", "Third", 3.0);
        c.terminal_session_guid = Some("g2".into());
        c.browser_session_guid = Some("browser".into());
        for x in [&a, &b, &c, &chat("D", "Fourth", 4.0)] {
            store.add_chat(x).unwrap();
        }
        let mut got = store.linked_sessions().unwrap();
        got.sort();
        assert_eq!(got, vec![("A".to_string(), "g1".to_string()), ("C".to_string(), "g2".to_string())], "a blank guid is not a link; the browser guid is not a terminal link");
    }

    #[test]
    fn rows_follow_iterm2() {
        let mut store = Store::open_in_memory().unwrap();
        let fresh = Chat::new("T", 5.5);
        assert_eq!((fresh.creation_date, fresh.last_modified_date, fresh.permissions.as_str(), fresh.orchestration_enabled), (5.5, 5.5, "", false));
        assert_eq!(fresh.id.len(), 36);
        let mut a = chat("A", "First", 100.5);
        a.icon = Some(vec![1, 2, 3]);
        a.claimed_scopes = vec!["w1".into(), "session:7".into()];
        a.terminal_session_guid = Some("7".into());
        a.permissions = "[]".into();
        a.watchers = Some("[{}]".into());
        a.blob_protocol = Some("2".into());
        a.model_name = Some("gpt".into());
        store.add_chat(&a).unwrap();
        assert_eq!(store.chats().unwrap(), [a.clone()]);
        assert_eq!(store.chat("A").unwrap(), Some(a.clone()));
        assert_eq!(store.chat("nope").unwrap(), None);
        let scopes: String = store.conn.query_row("select claimedScopes from Chat where uuid = 'A'", [], |row| row.get(0)).unwrap();
        assert_eq!(scopes, "w1\nsession:7");
        let kind: String = store.conn.query_row("select typeof(creationDate) from Chat where uuid = 'A'", [], |row| row.get(0)).unwrap();
        assert_eq!(kind, "real");
        store.add_chat(&chat("B", "Second", 200.25)).unwrap();
        assert_eq!(store.chats().unwrap().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        store.bump("A", 300.75).unwrap();
        let chats = store.chats().unwrap();
        assert_eq!(chats.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(chats[0].last_modified_date, 300.75);

        assert!(!store.rename("A", "  ").unwrap());
        assert_eq!(store.chat("A").unwrap().unwrap().title, "First");
        assert!(store.rename("A", " New \n").unwrap());
        let renamed = store.chat("A").unwrap().unwrap();
        assert_eq!((renamed.title.as_str(), &renamed.icon), ("New", &None));
        assert!(!store.rename("A", "New").unwrap());
        assert!(!store.rename("nope", "X").unwrap());

        let mut updated = renamed.clone();
        updated.vector_store = Some("vs".into());
        updated.orchestration_enabled = true;
        updated.browser_session_guid = Some("9".into());
        store.update_chat(&updated).unwrap();
        assert_eq!(store.chat("A").unwrap(), Some(updated));

        let m1 = Message::new("A", Participant::User, Content::PlainText { text: "hello".into(), context: None }, 400.5);
        let mut m2 = Message::new("A", Participant::Agent, Content::Markdown("hi".into()), 401.0);
        m2.response_id = Some("resp".into());
        m2.agent_reasoning = Some("thought".into());
        m2.first_blob_ref = Some("B1".into());
        let m3 = Message::new("A", Participant::Agent, Content::Commit(m2.unique_id.clone()), 402.0);
        let other = Message::new("B", Participant::User, Content::Markdown("b".into()), 403.0);
        let s1 = store.add_message(&m1).unwrap();
        let s2 = store.add_message(&m2).unwrap();
        let s3 = store.add_message(&m3).unwrap();
        store.add_message(&other).unwrap();
        assert!(s1 < s2 && s2 < s3);
        let read = store.messages("A").unwrap();
        assert_eq!(read.iter().map(|m| m.seq).collect::<Vec<_>>(), [s1, s2, s3]);
        assert_eq!(read[1], Message { seq: s2, ..m2.clone() });
        assert_eq!(read[0].author, Participant::User);
        let author: String = store.conn.query_row("select author from Message where seq = ?", [s2], |row| row.get(0)).unwrap();
        assert_eq!(author, "agent");
        assert_eq!(store.message(&m1.unique_id).unwrap(), Some(Message { seq: s1, ..m1.clone() }));
        assert_eq!(store.message("nope").unwrap(), None);
        assert_eq!(store.newest_id("A").unwrap(), Some(m3.unique_id.clone()));
        assert_eq!(store.newest_id("Z").unwrap(), None);

        assert!(store.replace_content(&m2.unique_id, &Content::Markdown("hi there".into())).unwrap());
        assert!(!store.replace_content("nope", &Content::Markdown("x".into())).unwrap());
        let read = store.messages("A").unwrap();
        assert_eq!(read[1].content, Content::Markdown("hi there".into()));
        assert_eq!(read[0].content, m1.content);

        store.remove_messages("A", &[m1.unique_id.clone(), other.unique_id.clone()]).unwrap();
        assert_eq!(store.messages("A").unwrap().len(), 2);
        assert_eq!(store.messages("B").unwrap().len(), 1);

        store.add_blob(&blob("A", "B1")).unwrap();
        store.add_blob(&blob("B", "B2")).unwrap();
        store.delete_chat("A").unwrap();
        assert_eq!(store.chat("A").unwrap(), None);
        assert!(store.messages("A").unwrap().is_empty());
        assert!(store.blobs("A").unwrap().is_empty());
        assert_eq!(store.messages("B").unwrap().len(), 1);
        assert_eq!(store.blobs("B").unwrap().len(), 1);
        assert!(store.chat("B").unwrap().is_some());

        // iTerm2 wrote whole seconds as INTEGER.
        store.conn.execute("insert into Message (uniqueID, author, chatID, content, sentDate) values ('I1', 'user', 'B', '{\"markdown\":{\"_0\":\"i\"}}', 1700000000)", []).unwrap();
        assert_eq!(store.message("I1").unwrap().unwrap().sent_date, 1700000000.0);
    }

    fn blob(chat: &str, id: &str) -> BlobRow {
        BlobRow { seq: 0, blob_id: id.into(), chat_id: chat.into(), protocol: 2, role: "user".into(), payload: vec![0, 1, 2], response_id: None, token_count: None, wire_format_version: 0 }
    }

    #[test]
    fn blobs_follow_iterm2() {
        let mut store = Store::open_in_memory().unwrap();
        let mut full = blob("a", "X1");
        full.response_id = Some("r".into());
        full.token_count = Some(42);
        full.wire_format_version = 1;
        full.role = "assistant".into();
        full.protocol = 5;
        let s1 = store.add_blob(&full).unwrap();
        let s2 = store.add_blob(&blob("a", "X2")).unwrap();
        let s3 = store.add_blob(&blob("a", "X3")).unwrap();
        store.add_blob(&blob("z", "Z1")).unwrap();
        assert!(s1 < s2 && s2 < s3);
        let read = store.blobs("a").unwrap();
        assert_eq!(read[0], BlobRow { seq: s1, ..full.clone() });
        assert_eq!(read.iter().map(|b| b.blob_id.as_str()).collect::<Vec<_>>(), ["X1", "X2", "X3"]);
        let token: String = store.conn.query_row("select tokenCount from ChatBlob where seq = ?", [s1], |row| row.get(0)).unwrap();
        assert_eq!(token, "42");

        assert_eq!(store.blobs("a").unwrap().len(), 3);
        store.clear_blobs("a").unwrap();
        assert!(store.blobs("a").unwrap().is_empty());
        assert_eq!(store.blobs("z").unwrap().len(), 1, "another chat's blobs stay");
    }

    #[test]
    fn fork_blobs_follow_iterm2() {
        let mut store = Store::open_in_memory().unwrap();
        let mut full = blob("a", "X1");
        full.response_id = Some("r".into());
        full.token_count = Some(42);
        full.protocol = 5;
        store.add_blob(&full).unwrap();
        for id in ["X2", "X3"] {
            store.add_blob(&blob("a", id)).unwrap();
        }
        let refs = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Nothing copies for no refs, more refs than blobs, a mismatch or a gap.
        for (to, retained) in [("n1", refs(&[])), ("n2", refs(&["X1", "X2", "X3", "X4"])), ("n3", refs(&["X2"])), ("n4", refs(&["X1", "X3"]))] {
            store.add_chat(&Chat { id: to.into(), ..Chat::new(to, 1.0) }).unwrap();
            assert!(store.copy_blobs("a", to, &retained).unwrap().is_empty(), "{retained:?}");
            assert!(store.blobs(to).unwrap().is_empty(), "{retained:?}");
        }

        // The fork's chat comes first.
        assert!(store.copy_blobs("a", "none", &refs(&["X1"])).is_err());
        assert!(store.blobs("none").unwrap().is_empty());

        // A fork of the first two rounds: fresh ids, everything else copied.
        let mut source = Chat::new("S", 1.0);
        source.id = "a".into();
        source.blob_protocol = Some("5".into());
        store.add_chat(&source).unwrap();
        let fork = Chat::new("F", 2.0);
        store.add_chat(&fork).unwrap();
        let mut m1 = Message::new("a", Participant::User, Content::PlainText { text: "one".into(), context: None }, 1.0);
        m1.first_blob_ref = Some("X1".into());
        let mut m2 = Message::new("a", Participant::User, Content::PlainText { text: "two".into(), context: None }, 2.0);
        m2.first_blob_ref = Some("X2".into());
        let reply = Message::new("a", Participant::Agent, Content::Markdown("ok".into()), 3.0);
        let kept = [m1, reply, m2];
        let retained: Vec<String> = kept.iter().filter_map(|m| m.first_blob_ref.clone()).collect();
        let map = store.copy_blobs("a", &fork.id, &retained).unwrap();
        let copied = store.blobs(&fork.id).unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(map.len(), 2);
        let originals = store.blobs("a").unwrap();
        for (copy, original) in copied.iter().zip(&originals) {
            assert!(originals.iter().all(|o| o.blob_id != copy.blob_id), "a fork shares no blob id with its source");
            assert_eq!(map[&original.blob_id], copy.blob_id);
            assert_eq!(*copy, BlobRow { seq: copy.seq, blob_id: copy.blob_id.clone(), chat_id: fork.id.clone(), ..original.clone() });
        }
        assert_eq!(store.chat(&fork.id).unwrap().unwrap().blob_protocol.as_deref(), Some("5"), "the source's protocol goes with the blobs");

        // The fork's clones point at the copies.
        for m in &kept {
            let clone = Message { unique_id: new_id(), chat_id: fork.id.clone(), first_blob_ref: None, ..m.clone() };
            store.add_message(&clone).unwrap();
            if let Some(new_ref) = m.first_blob_ref.as_ref().and_then(|r| map.get(r)) {
                assert!(store.set_first_blob_ref(&fork.id, &clone.unique_id, new_ref).unwrap());
            }
        }
        assert!(!store.set_first_blob_ref(&fork.id, "missing", "X1").unwrap());
        store.delete_chat("a").unwrap();
        let blob_ids: Vec<String> = store.blobs(&fork.id).unwrap().into_iter().map(|b| b.blob_id).collect();
        assert_eq!(blob_ids.len(), 2, "deleting the source keeps the fork's blobs");
        let linked: Vec<String> = store.messages(&fork.id).unwrap().into_iter().filter_map(|m| m.first_blob_ref).collect();
        assert_eq!(linked.len(), 2);
        assert!(linked.iter().all(|r| blob_ids.contains(r)), "every forked message's firstBlobRef resolves in the fork");
    }

    #[test]
    fn corrupt_database_does_not_open() {
        // Removed when dropped, a failed assertion included.
        let dir = crate::util_cli::TempDir::new("termurion-chatdb-corrupt");
        let path = dir.join(FILE_NAME);
        let garbage: Vec<u8> = (0..8192u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&path, &garbage).unwrap();
        assert!(Store::open(&path, &dir.join(LOCK_NAME)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), garbage, "nothing is written to a file that fails the check");

        // A real database cut short.
        let good = dir.join("good.sqlite");
        {
            let mut store = Store::open(&good, &dir.join("good-lock")).unwrap();
            for i in 0..200 {
                store.add_message(&Message::new("C", Participant::User, Content::Markdown(format!("message {i} {}", "x".repeat(200))), 1.0)).unwrap();
            }
        }
        let bytes = std::fs::read(&good).unwrap();
        let cut = dir.join("cut.sqlite");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        assert!(Store::open(&cut, &dir.join("cut-lock")).is_err());
        // A readable database with one broken page: the schema reads, the check does not pass.
        let page = u16::from_be_bytes([bytes[16], bytes[17]]) as usize;
        let mut broken = bytes.clone();
        broken[bytes.len() - page] = 0;
        let bad = dir.join("bad.sqlite");
        std::fs::write(&bad, &broken).unwrap();
        assert!(Store::open(&bad, &dir.join("bad-lock")).is_err());
        assert_eq!(std::fs::read(&bad).unwrap(), broken, "nothing is written to a database that fails the check");
        assert!(Store::open(&good, &dir.join("good-lock")).is_ok(), "an intact database opens");
    }

    #[test]
    fn search_binds_parameters() {
        let mut store = Store::open_in_memory().unwrap();
        let first = Message::new("C1", Participant::User, Content::PlainText { text: "it's here".into(), context: None }, 1.0);
        let second = Message::new("C2", Participant::Agent, Content::Markdown("other".into()), 2.0);
        let third = Message::new("C3", Participant::User, Content::Markdown("price 500 here".into()), 3.0);
        let fourth = Message::new("C3", Participant::User, Content::Markdown("50% off_sale here".into()), 4.0);
        for m in [&first, &second, &third, &fourth] {
            store.add_message(m).unwrap();
        }
        let hit = |c: &str, m: &Message| (c.to_string(), m.unique_id.clone());
        assert_eq!(store.search("it's").unwrap(), [hit("C1", &first)]);
        assert_eq!(store.search("IT'S here").unwrap(), [hit("C1", &first)]);
        assert!(store.search("").unwrap().is_empty());
        assert!(store.search(" \t\n").unwrap().is_empty());
        assert_eq!(store.search("here").unwrap(), [hit("C3", &fourth), hit("C3", &third), hit("C1", &first)]);
        assert_eq!(store.search("here  price").unwrap(), [hit("C3", &third)]);
        assert!(store.search("it's other").unwrap().is_empty());
        assert_eq!(store.search("50%").unwrap(), [hit("C3", &fourth)]);
        assert_eq!(store.search("off_sale").unwrap(), [hit("C3", &fourth)]);
        assert!(store.search("price_500").unwrap().is_empty());
        assert!(store.search("\\").unwrap().is_empty());
        assert!(store.search("'; drop table Message; --").unwrap().is_empty());
        assert_eq!(store.messages("C1").unwrap().len(), 1);
        assert_eq!(
            search_sql(3),
            "select chatID, uniqueID from Message where content like ? escape '\\' and content like ? escape '\\' and content like ? escape '\\' order by seq desc"
        );
        assert_eq!(search_sql(1), "select chatID, uniqueID from Message where content like ? escape '\\' order by seq desc");
        assert_eq!(like_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
    }

    #[cfg(windows)]
    #[test]
    fn lock_follows_iterm2() {
        let dir = crate::util_cli::TempDir::new("termurion-chatdb-lock");
        let path = dir.join("sub").join(FILE_NAME);
        let lock = dir.join("sub").join(LOCK_NAME);
        let first = Store::open(&path, &lock).unwrap();
        assert!(Store::open(&path, &lock).is_err());
        drop(first);
        let mut again = Store::open(&path, &lock).unwrap();
        again.add_chat(&Chat::new("T", 1.0)).unwrap();
        drop(again);
        assert_eq!(Store::open(&path, &lock).unwrap().chats().unwrap().len(), 1);
        assert_eq!(OPEN_FAILED_HEADING, "Error");
        assert_eq!(OPEN_FAILED, "AI Chat could not open because of a problem loading the database. Verify there is only one instance of Tandem running.");
        assert!(default_path().ends_with(std::path::Path::new("Termurion").join("chatdb.sqlite")));
    }
}
