use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::providers::{Part, Role};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: Role,
    pub parts: Vec<Part>,
    pub model: Option<String>,
    pub created_at: i64,
}

pub const UNTITLED: &str = "New session";

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS sessions (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 role TEXT NOT NULL,
                 parts TEXT NOT NULL,
                 model TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_by_session ON messages(session_id, id);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn create_session(&self) -> anyhow::Result<Session> {
        let now = crate::now_millis();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: UNTITLED.into(),
            created_at: now,
            updated_at: now,
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![session.id, session.title, now, now],
        )?;
        Ok(session)
    }

    pub fn sessions(&self) -> anyhow::Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Session {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn session(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT id, title, created_at, updated_at FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(Session {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn rename_session(&self, id: &str, title: &str) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn messages(&self, session_id: &str) -> anyhow::Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, parts, model, created_at
             FROM messages WHERE session_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (id, session_id, role, parts, model, created_at) = row?;
            Ok(StoredMessage {
                id,
                session_id,
                role: serde_json::from_value(serde_json::Value::String(role))?,
                parts: serde_json::from_str(&parts)?,
                model,
                created_at,
            })
        })
        .collect()
    }

    pub fn append(
        &self,
        session_id: &str,
        role: Role,
        parts: Vec<Part>,
        model: Option<&str>,
    ) -> anyhow::Result<StoredMessage> {
        let now = crate::now_millis();
        let role_text = serde_json::to_value(role)?
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, parts, model, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role_text, serde_json::to_string(&parts)?, model, now],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
            params![session_id, now],
        )?;
        Ok(StoredMessage {
            id,
            session_id: session_id.to_owned(),
            role,
            parts,
            model: model.map(str::to_owned),
            created_at: now,
        })
    }
}
