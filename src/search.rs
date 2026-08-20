use anyhow::Result;
use rusqlite::Connection;

use crate::db::HistoryEntry;

/// The directory to scope a cwd-less search to: the logical $PWD when
/// the shell exported it (matching what `record` stores), else the
/// physical current directory. None when neither is available (global
/// search).
pub fn default_cwd() -> Option<String> {
    let normalize = |s: String| {
        let t = s.trim_end_matches('/').to_string();
        if t.is_empty() {
            "/".to_string()
        } else {
            t
        }
    };
    std::env::var("PWD")
        .ok()
        .filter(|p| !p.is_empty() && p.starts_with('/'))
        .map(normalize)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .map(normalize)
        })
}

pub fn search(
    conn: &Connection,
    cwd: Option<&str>,
    pattern: &str,
    limit: usize,
) -> Result<Vec<HistoryEntry>> {
    let like = format!("%{}%", pattern);
    let mut stmt = if cwd.is_some() {
        conn.prepare(
            "SELECT id, cwd, cmd, exit_code, started_at, shell FROM history
             WHERE cwd = ?1 AND cmd LIKE ?2
             ORDER BY started_at DESC, id DESC LIMIT ?3",
        )?
    } else {
        conn.prepare(
            "SELECT id, cwd, cmd, exit_code, started_at, shell FROM history
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
        shell: r.get(5)?,
    })
}
