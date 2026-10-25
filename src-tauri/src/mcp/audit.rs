//! A record of everything a client asked of Corelens and was given.
//!
//! One row per tool call: when, which project, which tool and arguments,
//! the device and address it touched, the command sent to a device, how it
//! ended, and the result **as returned** — after redaction, so this table
//! holds nothing the client was not given. It is the record of what left
//! Corelens, which is why it keeps the text a debug log never would. In the
//! database, deleted with its project, never in an export or the shared
//! folder; read, cleared and exported as a change record from Settings.

use rusqlite::{params, Connection};
use serde::Serialize;

/// Schema 9.
pub fn tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            at_ms INTEGER NOT NULL,
            project_id TEXT,
            tool TEXT NOT NULL,
            arguments TEXT NOT NULL DEFAULT '{}',
            device TEXT NOT NULL DEFAULT '',
            address TEXT NOT NULL DEFAULT '',
            command TEXT NOT NULL DEFAULT '',
            outcome TEXT NOT NULL,
            result TEXT NOT NULL DEFAULT '',
            bytes INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS mcp_audit_time ON mcp_audit (at_ms DESC);",
    )
}

/// One call, as it is written down.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub at_ms: i64,
    pub project_id: Option<String>,
    pub tool: String,
    pub arguments: String,
    pub device: String,
    pub address: String,
    pub command: String,
    /// `ok`, `error` or `withheld`.
    pub outcome: String,
    pub result: String,
    pub bytes: i64,
    pub duration_ms: i64,
}

pub fn record(conn: &Connection, e: &Entry) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO mcp_audit (at_ms, project_id, tool, arguments, device, address, command, outcome, result, bytes, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![e.at_ms, e.project_id, e.tool, e.arguments, e.device, e.address, e.command, e.outcome, e.result, e.bytes, e.duration_ms],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Newest first; one project's when named.
pub fn list(conn: &Connection, project_id: Option<&str>, limit: i64) -> rusqlite::Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, at_ms, project_id, tool, arguments, device, address, command, outcome, result, bytes, duration_ms
         FROM mcp_audit WHERE (?1 IS NULL OR project_id = ?1) ORDER BY at_ms DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit], |r| {
        Ok(Entry {
            id: r.get(0)?,
            at_ms: r.get(1)?,
            project_id: r.get(2)?,
            tool: r.get(3)?,
            arguments: r.get(4)?,
            device: r.get(5)?,
            address: r.get(6)?,
            command: r.get(7)?,
            outcome: r.get(8)?,
            result: r.get(9)?,
            bytes: r.get(10)?,
            duration_ms: r.get(11)?,
        })
    })?;
    rows.collect()
}

/// The app setting for how many days the record is kept: 30, 90 or 365.
pub const RETENTION_SETTING: &str = "aiAuditRetentionDays";
pub const DEFAULT_RETENTION_DAYS: i64 = 90;
/// The most the record may hold, as five files of 10 MB would: past it,
/// the oldest calls go first.
pub const SIZE_CAP: i64 = 5 * 10 * 1024 * 1024;

pub fn retention_days(conn: &Connection) -> i64 {
    crate::db::all_settings(conn)
        .ok()
        .and_then(|s| s.get(RETENTION_SETTING).and_then(|v| v.parse::<i64>().ok()))
        .filter(|d| [30, 90, 365].contains(d))
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Keeps the record inside its retention and its size: calls older than
/// the retention are removed, then the oldest until what is left fits
/// [`SIZE_CAP`]. The change record follows the same retention.
pub fn prune(conn: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let days = retention_days(conn);
    let before = now_ms - days * 86_400_000;
    let mut removed = conn.execute("DELETE FROM mcp_audit WHERE at_ms < ?1", params![before])?;
    let total: i64 = conn.query_row("SELECT COALESCE(SUM(LENGTH(result) + LENGTH(arguments) + 256), 0) FROM mcp_audit", [], |r| r.get(0))?;
    if total > SIZE_CAP {
        // The newest rows that fit, by a running total from the newest.
        removed += conn.execute(
            "DELETE FROM mcp_audit WHERE id IN (
                SELECT id FROM (
                    SELECT id, SUM(LENGTH(result) + LENGTH(arguments) + 256) OVER (ORDER BY at_ms DESC, id DESC) AS kept
                    FROM mcp_audit
                ) WHERE kept > ?1
            )",
            params![SIZE_CAP],
        )?;
    }
    removed += crate::mcp::changes::store::prune(conn, now_ms, days)?;
    Ok(removed)
}

pub fn clear(conn: &Connection, project_id: Option<&str>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM mcp_audit WHERE (?1 IS NULL OR project_id = ?1)", params![project_id])
}

/// Rows of projects that no longer exist.
pub fn purge_orphans(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM mcp_audit WHERE project_id IS NOT NULL AND project_id NOT IN (SELECT id FROM projects)", [])
}

/// A time as the record shows it, in UTC.
pub fn when(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The rows, oldest first, as a change record a person can read and file.
pub fn change_record(project: &str, entries: &[Entry]) -> String {
    let mut out = format!(
        "# Change record — {project}\n\nWhat an AI assistant asked Corelens Desktop AI to do, and what Corelens returned to it, \
         with passwords, keys and community strings removed. Oldest first. {} call(s).\n\n",
        entries.len()
    );
    for e in entries.iter().rev() {
        out.push_str(&format!("## {} — {}{}\n\n", when(e.at_ms), e.tool, if e.device.is_empty() { String::new() } else { format!(" on {}", e.device) }));
        if !e.address.is_empty() {
            out.push_str(&format!("- Address: {}\n", e.address));
        }
        if !e.command.is_empty() {
            out.push_str(&format!("- Command: `{}`\n", e.command.replace('`', "'")));
        }
        out.push_str(&format!("- Arguments: `{}`\n- Outcome: {} ({} bytes, {} ms)\n\n", e.arguments.replace('`', "'"), e.outcome, e.bytes, e.duration_ms));
        if !e.result.is_empty() {
            out.push_str("```text\n");
            out.push_str(&e.result.replace("```", "'''"));
            if !e.result.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
    }
    out
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_tests(&c);
        c
    }

    fn row(at_ms: i64, result: &str) -> Entry {
        Entry { at_ms, tool: "get_device".into(), outcome: "ok".into(), result: result.into(), ..Default::default() }
    }

    #[test]
    fn calls_past_the_retention_go_and_the_setting_chooses_it() {
        let c = conn();
        let now = 400 * 86_400_000;
        record(&c, &row(now - 100 * 86_400_000, "old")).unwrap();
        record(&c, &row(now - 40 * 86_400_000, "middle")).unwrap();
        record(&c, &row(now - 86_400_000, "new")).unwrap();
        prune(&c, now).unwrap();
        let left: Vec<String> = list(&c, None, 10).unwrap().into_iter().map(|e| e.result).collect();
        assert_eq!(left, ["new", "middle"], "90 days by default");
        crate::db::set_setting(&c, RETENTION_SETTING, Some("30")).unwrap();
        prune(&c, now).unwrap();
        assert_eq!(list(&c, None, 10).unwrap().len(), 1);
        crate::db::set_setting(&c, RETENTION_SETTING, Some("7")).unwrap();
        assert_eq!(retention_days(&c), DEFAULT_RETENTION_DAYS, "only 30, 90 or 365");
    }

    #[test]
    fn past_the_size_cap_the_oldest_go_first() {
        let c = conn();
        let big = "x".repeat(4 * 1024 * 1024);
        for i in 0..20 {
            record(&c, &row(1_000 + i, &big)).unwrap();
        }
        prune(&c, 2_000).unwrap();
        let left = list(&c, None, 100).unwrap();
        let total: i64 = left.iter().map(|e| e.result.len() as i64 + 256 + 2).sum();
        assert!(total <= SIZE_CAP, "{total}");
        assert_eq!(left.len(), 12);
        assert_eq!(left.last().unwrap().at_ms, 1_008, "the newest are what is kept");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY);").unwrap();
        tables(&c).unwrap();
        c
    }

    #[test]
    fn calls_are_kept_newest_first_per_project_and_cleared() {
        let c = conn();
        c.execute("INSERT INTO projects (id) VALUES ('p1')", []).unwrap();
        for (i, p) in [(1, "p1"), (2, "p1"), (3, "gone")] {
            record(&c, &Entry { at_ms: i * 1000, project_id: Some(p.into()), tool: format!("t{i}"), outcome: "ok".into(), ..Default::default() }).unwrap();
        }
        assert_eq!(list(&c, Some("p1"), 10).unwrap().iter().map(|e| e.tool.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);
        assert_eq!(purge_orphans(&c).unwrap(), 1);
        assert_eq!(clear(&c, Some("p1")).unwrap(), 2);
        assert!(list(&c, None, 10).unwrap().is_empty());
    }

    #[test]
    fn the_change_record_reads_oldest_first_and_cannot_be_broken_by_a_result() {
        let entries = vec![
            Entry { at_ms: 1_761_264_000_000, tool: "run_show_command".into(), device: "SW1".into(), address: "192.0.2.10".into(), command: "show ip arp".into(), outcome: "ok".into(), result: "a ``` fence\n".into(), arguments: "{}".into(), ..Default::default() },
            Entry { at_ms: 1_761_260_400_000, tool: "list_devices".into(), outcome: "ok".into(), arguments: "{}".into(), ..Default::default() },
        ];
        let md = change_record("Example project", &entries);
        assert!(md.find("list_devices").unwrap() < md.find("run_show_command").unwrap());
        assert!(md.contains("## 2025-10-24 00:00:00 UTC — run_show_command on SW1"), "{md}");
        assert!(md.contains("- Command: `show ip arp`"));
        assert_eq!(md.matches("```").count(), 2, "a fence in a result does not end the block: {md}");
    }
}
