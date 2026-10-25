//! The change record: every proposal, whoever made it, and everything
//! that happened to it, in the project database.
//!
//! One row per change, its whole record as JSON (the shape the page
//! reads), so a record written by this version reads back whole in the
//! next. Every text in it — snapshots, push output, verification — passed
//! the redaction gate before it was stored. Deleted with its project;
//! pruned by the retention set in Settings → AI.

use rusqlite::{params, Connection, OptionalExtension};

use super::Change;

/// Schema 10.
pub fn tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS config_changes (
            id TEXT PRIMARY KEY,
            project_id TEXT,
            created_ms INTEGER NOT NULL,
            status TEXT NOT NULL,
            body TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS config_changes_time ON config_changes (project_id, created_ms DESC);",
    )
}

pub fn put(conn: &Connection, c: &Change) -> rusqlite::Result<()> {
    let body = serde_json::to_string(c).unwrap_or_default();
    conn.execute(
        "INSERT INTO config_changes (id, project_id, created_ms, status, body) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status, body = excluded.body",
        params![c.id, c.project_id, c.created_ms, c.status.as_str(), body],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Change>> {
    let body: Option<String> = conn.query_row("SELECT body FROM config_changes WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
    Ok(body.and_then(|b| serde_json::from_str(&b).ok()))
}

/// Newest first; one project's when named.
pub fn list(conn: &Connection, project_id: Option<&str>, limit: i64) -> rusqlite::Result<Vec<Change>> {
    let mut stmt = conn.prepare("SELECT body FROM config_changes WHERE (?1 IS NULL OR project_id = ?1) ORDER BY created_ms DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![project_id, limit], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(Result::ok).filter_map(|b| serde_json::from_str(&b).ok()).collect())
}

/// Changes left waiting or running when the app last closed: no one is
/// waiting for them any more, so they are closed and say why.
pub fn close_interrupted(conn: &Connection) -> rusqlite::Result<usize> {
    let open: Vec<String> = {
        let mut stmt = conn.prepare("SELECT body FROM config_changes WHERE status IN ('pending', 'running')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(Result::ok).collect()
    };
    let mut n = 0;
    for body in open {
        if let Ok(mut c) = serde_json::from_str::<Change>(&body) {
            c.interrupted();
            put(conn, &c)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Closed changes older than `days`.
pub fn prune(conn: &Connection, now_ms: i64, days: i64) -> rusqlite::Result<usize> {
    let before = now_ms - days.max(1) * 86_400_000;
    conn.execute("DELETE FROM config_changes WHERE created_ms < ?1 AND status NOT IN ('pending', 'running')", params![before])
}

pub fn purge_orphans(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM config_changes WHERE project_id IS NOT NULL AND project_id NOT IN (SELECT id FROM projects)", [])
}
