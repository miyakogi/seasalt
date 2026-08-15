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
  session TEXT
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
    Ok(())
}

pub fn insert_history(
    conn: &Connection,
    cwd: &str,
    cmd: &str,
    started_at: i64,
    session: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at, session) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![cwd, cmd, started_at, session],
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

pub fn suggest_in_dir(conn: &Connection, cwd: &str, needle: &str) -> Result<Option<String>> {
    if needle.is_empty() {
        return Ok(None);
    }
    let pattern = format!("{}%", escape_like(needle));
    let mut stmt = conn.prepare(
        "SELECT cmd FROM history WHERE cwd = ?1 AND cmd LIKE ?2 ESCAPE '\\'
         ORDER BY started_at DESC, id DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![cwd, pattern])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row.get(0)?));
    }
    Ok(None)
}

pub fn suggest_global(conn: &Connection, needle: &str) -> Result<Option<String>> {
    if needle.is_empty() {
        return Ok(None);
    }
    let pattern = format!("{}%", escape_like(needle));
    let mut stmt = conn.prepare(
        "SELECT cmd FROM history WHERE cmd LIKE ?1 ESCAPE '\\'
         ORDER BY started_at DESC, id DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![pattern])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row.get(0)?));
    }
    Ok(None)
}

fn escape_like(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect()
}
