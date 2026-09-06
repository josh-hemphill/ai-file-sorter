//! SQLite WAL store owned by the engine. The UI never opens this database.

use aifs_domain::{
    ApplyJournal, JournalId, OperationPlan, PlanId, ProposalRevision, RevisionId, SessionId,
    WorkspaceSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use thiserror::Error;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS snapshots (
    session TEXT PRIMARY KEY,
    captured_at INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS revisions (
    id TEXT PRIMARY KEY,
    session TEXT NOT NULL,
    parent TEXT,
    created_at INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS plans (
    id TEXT PRIMARY KEY,
    session TEXT NOT NULL,
    revision TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS journals (
    id TEXT PRIMARY KEY,
    session TEXT NOT NULL,
    plan TEXT NOT NULL,
    json TEXT NOT NULL
);
";

/// Persistence failures.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON (de)serialisation error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Filesystem error while creating the database directory.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Engine-owned workspace database.
pub struct WorkspaceStore {
    conn: Connection,
}

impl WorkspaceStore {
    /// Opens an in-memory database (one engine process / test).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    /// Opens (or creates) a file-backed database.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '1')",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Stores the latest snapshot for a session (replacing any previous one).
    pub fn put_snapshot(&self, snapshot: &WorkspaceSnapshot) -> Result<(), StoreError> {
        let json = serde_json::to_string(snapshot)?;
        self.conn.execute(
            "INSERT INTO snapshots(session, captured_at, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(session) DO UPDATE SET captured_at = excluded.captured_at, json = excluded.json",
            params![
                snapshot.session.to_string(),
                snapshot.captured_at.as_millis(),
                json
            ],
        )?;
        Ok(())
    }

    /// Loads the latest snapshot for a session.
    pub fn get_snapshot(
        &self,
        session: SessionId,
    ) -> Result<Option<WorkspaceSnapshot>, StoreError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM snapshots WHERE session = ?1",
                params![session.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(StoreError::from)
    }

    /// Stores a revision.
    pub fn put_revision(&self, revision: &ProposalRevision) -> Result<(), StoreError> {
        let json = serde_json::to_string(revision)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO revisions(id, session, parent, created_at, json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                revision.id.to_string(),
                revision.session.to_string(),
                revision.parent.map(|id| id.to_string()),
                revision.created_at.as_millis(),
                json
            ],
        )?;
        Ok(())
    }

    /// Loads a revision by id.
    pub fn get_revision(&self, id: RevisionId) -> Result<Option<ProposalRevision>, StoreError> {
        load_json(
            &self.conn,
            "SELECT json FROM revisions WHERE id = ?1",
            &id.to_string(),
        )
    }

    /// Latest revision for a session (by created_at, then rowid).
    pub fn latest_revision(
        &self,
        session: SessionId,
    ) -> Result<Option<ProposalRevision>, StoreError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM revisions WHERE session = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![session.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(StoreError::from)
    }

    /// Stores a plan.
    pub fn put_plan(&self, session: SessionId, plan: &OperationPlan) -> Result<(), StoreError> {
        let json = serde_json::to_string(plan)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO plans(id, session, revision, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                plan.id.to_string(),
                session.to_string(),
                plan.revision.to_string(),
                json
            ],
        )?;
        Ok(())
    }

    /// Loads a plan.
    pub fn get_plan(&self, id: PlanId) -> Result<Option<OperationPlan>, StoreError> {
        load_json(
            &self.conn,
            "SELECT json FROM plans WHERE id = ?1",
            &id.to_string(),
        )
    }

    /// Stores a journal.
    pub fn put_journal(
        &self,
        session: SessionId,
        journal: &ApplyJournal,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string(journal)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO journals(id, session, plan, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                journal.id.to_string(),
                session.to_string(),
                journal.plan.to_string(),
                json
            ],
        )?;
        Ok(())
    }

    /// Loads a journal.
    pub fn get_journal(&self, id: JournalId) -> Result<Option<ApplyJournal>, StoreError> {
        load_json(
            &self.conn,
            "SELECT json FROM journals WHERE id = ?1",
            &id.to_string(),
        )
    }

    /// Stores an opaque engine-owned JSON blob.
    pub fn put_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Loads an opaque engine-owned JSON blob.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }
}

fn load_json<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    id: &str,
) -> Result<Option<T>, StoreError> {
    let json: Option<String> = conn
        .query_row(sql, params![id], |row| row.get(0))
        .optional()?;
    json.map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(StoreError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{RevisionAuthor, SessionId};
    use std::path::PathBuf;

    #[test]
    fn snapshot_round_trip() {
        let store = WorkspaceStore::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
        let snapshot = WorkspaceSnapshot::new(SessionId::new(), PathBuf::from("/tmp/inbox"));
        store
            .put_snapshot(&snapshot)
            .unwrap_or_else(|e| panic!("{e}"));
        let loaded = store
            .get_snapshot(snapshot.session)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("missing"));
        assert_eq!(loaded.session, snapshot.session);
        assert_eq!(loaded.root, snapshot.root);
    }

    #[test]
    fn revision_latest_wins() {
        let store = WorkspaceStore::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
        let session = SessionId::new();
        let first = ProposalRevision::new(session, RevisionAuthor::Engine, "one");
        let mut second = ProposalRevision::new(session, RevisionAuthor::User, "two");
        second.parent = Some(first.id);
        store.put_revision(&first).unwrap_or_else(|e| panic!("{e}"));
        store
            .put_revision(&second)
            .unwrap_or_else(|e| panic!("{e}"));
        let latest = store
            .latest_revision(session)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("missing"));
        assert_eq!(latest.id, second.id);
    }

    #[test]
    fn meta_round_trip() {
        let store = WorkspaceStore::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
        store
            .put_meta("app_settings", r#"{"analyze_images":true}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        let loaded = store
            .get_meta("app_settings")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("missing"));
        assert!(loaded.contains("analyze_images"));
    }
}
