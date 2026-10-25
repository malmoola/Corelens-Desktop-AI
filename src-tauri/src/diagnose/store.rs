//! Diagnoses as the project keeps them: one row per run, its whole report
//! as JSON, every text in it already through the redaction gate. The
//! previous run of the same scope is what the next one is compared with.
//! Deleted with the project; pruned with the AI audit record's retention.

use corelens_diagnose::model::Report;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Schema 11.
pub fn tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS diagnostic_runs (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            scope_key TEXT NOT NULL,
            started_ms INTEGER NOT NULL,
            source TEXT NOT NULL,
            body TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS diagnostic_runs_scope ON diagnostic_runs (project_id, scope_key, started_ms DESC);",
    )
}

pub fn put(conn: &Connection, project_id: &str, source: &str, r: &Report) -> rusqlite::Result<()> {
    let body = serde_json::to_string(r).unwrap_or_default();
    conn.execute(
        "INSERT INTO diagnostic_runs (id, project_id, scope_key, started_ms, source, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET body = excluded.body",
        params![r.id, project_id, r.scope.key(), r.started_ms, source, body],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, Report)>> {
    let row: Option<(String, String)> = conn.query_row("SELECT project_id, body FROM diagnostic_runs WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(row.and_then(|(p, b)| serde_json::from_str(&b).ok().map(|r| (p, r))))
}

/// The newest finished run of this scope before `before_ms`.
pub fn previous(conn: &Connection, project_id: &str, scope_key: &str, before_ms: i64) -> rusqlite::Result<Option<Report>> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body FROM diagnostic_runs WHERE project_id = ?1 AND scope_key = ?2 AND started_ms < ?3 ORDER BY started_ms DESC LIMIT 1",
            params![project_id, scope_key, before_ms],
            |r| r.get(0),
        )
        .optional()?;
    Ok(body.and_then(|b| serde_json::from_str(&b).ok()))
}

/// The newest run of the whole project, for knowing where an address is.
pub fn latest_project(conn: &Connection, project_id: &str) -> rusqlite::Result<Option<Report>> {
    previous(conn, project_id, "project", i64::MAX)
}

/// One line of the list the page shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: String,
    pub scope: String,
    pub started_ms: i64,
    pub source: String,
    pub critical: usize,
    pub warning: usize,
    pub top_cause: Option<String>,
    pub cancelled: bool,
}

pub fn list(conn: &Connection, project_id: &str, limit: i64) -> rusqlite::Result<Vec<Summary>> {
    let mut stmt = conn.prepare("SELECT source, body FROM diagnostic_runs WHERE project_id = ?1 ORDER BY started_ms DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![project_id, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|(source, b)| serde_json::from_str::<Report>(&b).ok().map(|r| (source, r)))
        .map(|(source, r)| {
            use corelens_diagnose::playbook::Severity;
            Summary {
                id: r.id.clone(),
                scope: r.scope.describe(),
                started_ms: r.started_ms,
                source,
                critical: r.findings.iter().filter(|f| f.severity == Severity::Critical).count(),
                warning: r.findings.iter().filter(|f| f.severity == Severity::Warning).count(),
                top_cause: r.causes.first().map(|c| c.title.clone()),
                cancelled: r.cancelled,
            }
        })
        .collect())
}

pub fn clear(conn: &Connection, project_id: Option<&str>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM diagnostic_runs WHERE (?1 IS NULL OR project_id = ?1)", params![project_id])
}

/// Runs older than the retention.
pub fn prune(conn: &Connection, now_ms: i64, days: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM diagnostic_runs WHERE started_ms < ?1", params![now_ms - days * 86_400_000])
}

pub fn purge_orphans(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM diagnostic_runs WHERE project_id NOT IN (SELECT id FROM projects)", [])
}
