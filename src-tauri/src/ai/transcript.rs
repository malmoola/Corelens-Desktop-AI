//! The chat's transcript: one row per message, per project, in the
//! project database.
//!
//! Kept so a conversation is still there when the project is opened again
//! and so the model can be sent what was said; separate from the audit
//! record (which is what tools returned), with its own retention and a
//! *Clear all*. Every text in it is what was sent or shown — already
//! through the redaction gate. Deleted with its project.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::provider::{Message, Usage};

pub const RETENTION_SETTING: &str = "aiChatRetentionDays";
pub const DEFAULT_RETENTION_DAYS: i64 = 90;

/// Schema 10, with the change record.
pub fn tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_chat (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id TEXT NOT NULL,
            at_ms INTEGER NOT NULL,
            provider TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            body TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ai_chat_project ON ai_chat (project_id, id);",
    )
}

/// One message as the panel shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: i64,
    pub at_ms: i64,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub message: Message,
}

pub fn add(conn: &Connection, project_id: &str, at_ms: i64, provider: &str, model: &str, usage: Usage, message: &Message) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO ai_chat (project_id, at_ms, provider, model, input_tokens, output_tokens, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, at_ms, provider, model, usage.input_tokens as i64, usage.output_tokens as i64, serde_json::to_string(message).unwrap_or_default()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The project's messages, oldest first; the last `limit`.
pub fn list(conn: &Connection, project_id: &str, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, provider, model, input_tokens, output_tokens, body FROM (
            SELECT * FROM ai_chat WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2
         ) ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![project_id, limit], |r| {
        let body: String = r.get(6)?;
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, body))
    })?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|(id, at_ms, provider, model, i, o, body)| {
            Some(Row { id, at_ms, provider, model, usage: Usage { input_tokens: i as u64, output_tokens: o as u64 }, message: serde_json::from_str(&body).ok()? })
        })
        .collect())
}

/// The tokens a project's conversation has used, as the providers counted.
pub fn totals(conn: &Connection, project_id: &str) -> rusqlite::Result<Usage> {
    conn.query_row("SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0) FROM ai_chat WHERE project_id = ?1", params![project_id], |r| {
        Ok(Usage { input_tokens: r.get::<_, i64>(0)? as u64, output_tokens: r.get::<_, i64>(1)? as u64 })
    })
}

/// One project's transcript, or every project's.
pub fn clear(conn: &Connection, project_id: Option<&str>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM ai_chat WHERE (?1 IS NULL OR project_id = ?1)", params![project_id])
}

pub fn retention_days(conn: &Connection) -> i64 {
    crate::db::all_settings(conn)
        .ok()
        .and_then(|s| s.get(RETENTION_SETTING).and_then(|v| v.parse::<i64>().ok()))
        .filter(|d| [30, 90, 365].contains(d))
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

pub fn prune(conn: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let before = now_ms - retention_days(conn) * 86_400_000;
    conn.execute("DELETE FROM ai_chat WHERE at_ms < ?1", params![before])
}

pub fn purge_orphans(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM ai_chat WHERE project_id NOT IN (SELECT id FROM projects)", [])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_reads_back_in_order_counts_its_tokens_and_is_pruned_and_cleared() {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c);
        let day = 86_400_000;
        add(&c, "p1", 100 * day, "anthropic", "claude-opus-5-5", Usage::default(), &Message::User { text: "old".into() }).unwrap();
        add(&c, "p1", 200 * day, "anthropic", "claude-opus-5-5", Usage { input_tokens: 10, output_tokens: 3 }, &Message::User { text: "new".into() }).unwrap();
        add(&c, "p2", 200 * day, "openai", "gpt-6-astra", Usage { input_tokens: 1, output_tokens: 1 }, &Message::User { text: "other".into() }).unwrap();
        assert_eq!(list(&c, "p1", 10).unwrap().iter().map(|r| r.message.clone()).collect::<Vec<_>>(), [Message::User { text: "old".into() }, Message::User { text: "new".into() }]);
        assert_eq!(totals(&c, "p1").unwrap(), Usage { input_tokens: 10, output_tokens: 3 });
        assert_eq!(prune(&c, 201 * day).unwrap(), 1, "90 days by default");
        crate::db::set_setting(&c, RETENTION_SETTING, Some("30")).unwrap();
        assert_eq!(retention_days(&c), 30);
        assert_eq!(clear(&c, Some("p1")).unwrap(), 1);
        assert_eq!(clear(&c, None).unwrap(), 1, "Clear all takes every project's");
    }
}
