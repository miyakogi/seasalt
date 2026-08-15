use anyhow::Result;
use rusqlite::Connection;

use crate::db::HistoryEntry;

pub fn search(
    conn: &Connection,
    cwd: Option<&str>,
    pattern: &str,
    limit: usize,
) -> Result<Vec<HistoryEntry>> {
    let like = format!("%{}%", pattern);
    let mut stmt = if cwd.is_some() {
        conn.prepare(
            "SELECT id, cwd, cmd, exit_code, started_at FROM history
             WHERE cwd = ?1 AND cmd LIKE ?2
             ORDER BY started_at DESC, id DESC LIMIT ?3",
        )?
    } else {
        conn.prepare(
            "SELECT id, cwd, cmd, exit_code, started_at FROM history
             WHERE cmd LIKE ?1
             ORDER BY started_at DESC, id DESC LIMIT ?2",
        )?
    };
    let rows = if let Some(dir) = cwd {
        stmt.query_map(rusqlite::params![dir, like, limit as i64], map_row)?
    } else {
        stmt.query_map(rusqlite::params![like, limit as i64], map_row)?
    };
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: r.get(0)?,
        cwd: r.get(1)?,
        cmd: r.get(2)?,
        exit_code: r.get(3)?,
        started_at: r.get(4)?,
    })
}
