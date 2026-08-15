use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  cwd TEXT NOT NULL,
  cmd TEXT NOT NULL,
  exit_code INTEGER,
  started_at INTEGER NOT NULL,
  session TEXT,
  paths TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_history_cwd ON history(cwd);
CREATE INDEX IF NOT EXISTS idx_history_cmd ON history(cmd);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub cwd: String,
    pub cmd: String,
    pub exit_code: Option<i64>,
    pub started_at: i64,
}

pub fn default_db_path() -> Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("SEASALT_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Some(data) = dirs::data_dir() {
        data.join("seasalt")
    } else {
        anyhow::bail!("cannot determine data directory; set SEASALT_DATA_DIR");
    };
    std::fs::create_dir_all(&base)
        .with_context(|| format!("cannot create data directory {}", base.display()))?;
    Ok(base.join("history.sqlite3"))
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    if path != Path::new(":memory:") {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // 旧スキーマ (paths 列なし) からのマイグレーション
    if !has_column(conn, "paths")? {
        conn.execute_batch("ALTER TABLE history ADD COLUMN paths TEXT NOT NULL DEFAULT ''")?;
    }
    Ok(())
}

fn has_column(conn: &Connection, name: &str) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(history)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let col: String = row.get(1)?;
        if col == name {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn insert_history(
    conn: &Connection,
    cwd: &str,
    cmd: &str,
    started_at: i64,
    session: &str,
    paths: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at, session, paths) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![cwd, cmd, started_at, session, paths],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_exit_code(conn: &Connection, session: &str, id: i64, code: i64) -> Result<()> {
    conn.execute(
        "UPDATE history SET exit_code = ?1 WHERE session = ?2 AND id = ?3",
        rusqlite::params![code, session, id],
    )?;
    Ok(())
}

/// prefix 一致の候補を新しい順に最大 limit 件返す (cmd, paths)
pub fn suggest_in_dir(
    conn: &Connection,
    cwd: &str,
    needle: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("{}%", escape_like(needle));
    let mut stmt = conn.prepare(
        "SELECT cmd, paths FROM history WHERE cwd = ?1 AND cmd LIKE ?2 ESCAPE '\\'
         ORDER BY started_at DESC, id DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![cwd, pattern, limit as i64], map_candidate)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// prefix 一致の候補を新しい順に最大 limit 件返す (cmd, paths)
pub fn suggest_global(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("{}%", escape_like(needle));
    let mut stmt = conn.prepare(
        "SELECT cmd, paths FROM history WHERE cmd LIKE ?1 ESCAPE '\\'
         ORDER BY started_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], map_candidate)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn map_candidate(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
    Ok((r.get(0)?, r.get(1)?))
}

fn escape_like(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect()
}
