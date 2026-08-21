use std::path::{Path, PathBuf};
use std::time::Duration;

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
  paths TEXT NOT NULL DEFAULT '',
  shell TEXT NOT NULL DEFAULT 'bash'
);
CREATE INDEX IF NOT EXISTS idx_history_cwd ON history(cwd);
CREATE INDEX IF NOT EXISTS idx_history_cmd ON history(cmd);
CREATE INDEX IF NOT EXISTS idx_history_started_at ON history(started_at);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub cwd: String,
    pub cmd: String,
    pub exit_code: Option<i64>,
    pub started_at: i64,
    pub shell: String,
}

pub fn default_db_path() -> Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("SEASALT_DATA_DIR") {
        // An empty value is treated as unset so a stray exported variable
        // does not break every subcommand with a nonexistent base path.
        if dir.is_empty() {
            if let Some(data) = dirs::data_dir() {
                data.join("seasalt")
            } else {
                anyhow::bail!("cannot determine data directory; set SEASALT_DATA_DIR");
            }
        } else {
            PathBuf::from(dir)
        }
    } else if let Some(data) = dirs::data_dir() {
        data.join("seasalt")
    } else {
        anyhow::bail!("cannot determine data directory; set SEASALT_DATA_DIR");
    };
    let existed = base.exists();
    std::fs::create_dir_all(&base)
        .with_context(|| format!("cannot create data directory {}", base.display()))?;
    // Set 0700 only when the directory is newly created (existing
    // directories are left untouched). A 0700 directory also protects
    // the WAL files (-wal/-shm).
    if !existed {
        restrict_dir(&base)?;
    }
    Ok(base.join("history.sqlite3"))
}

pub fn open(path: &Path) -> Result<Connection> {
    let is_memory = path == Path::new(":memory:");
    let existed = is_memory || path.exists();
    let conn = Connection::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    if !is_memory {
        // Set 0600 only when the file is newly created (existing files are left untouched)
        if !existed {
            restrict_file(path)?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    // Bound the wait for other shells' writers to 150ms; rusqlite's default
    // (5000ms) could stall a shell hook behind a stuck writer for seconds.
    // 150ms stays comfortably inside the 200ms suggest budget so a slow
    // writer never pushes a keystroke past the UI deadline.
    conn.busy_timeout(Duration::from_millis(150))?;
    init(&conn)?;
    Ok(conn)
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    migrate(conn)
}

/// Applies schema migrations in order, tracking the applied version in
/// PRAGMA user_version so the per-open checks (PRAGMA table_info etc.)
/// run once instead of on every connection.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        // v0 -> v1: ensure the paths column (legacy DBs predate it)
        if !has_column(conn, "paths")? {
            conn.execute_batch("ALTER TABLE history ADD COLUMN paths TEXT NOT NULL DEFAULT ''")?;
        }
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        // v1 -> v2: collapse legacy duplicate (cwd, cmd) rows (keeping
        // the newest) and enforce uniqueness so record can upsert
        // atomically. The plain (cwd, cmd) index is superseded by the
        // unique one.
        conn.execute_batch(
            "DELETE FROM history WHERE id NOT IN (
               SELECT id FROM (
                 SELECT id, ROW_NUMBER() OVER (
                   PARTITION BY cwd, cmd ORDER BY started_at DESC, id DESC
                 ) AS rn FROM history
               ) WHERE rn = 1
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_history_cwd_cmd_unique ON history(cwd, cmd);
             DROP INDEX IF EXISTS idx_history_cwd_cmd;",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    if version < 3 {
        // v2 -> v3: tag each record with the shell it came from. Existing
        // rows (all recorded through bash so far) default to 'bash'.
        if !has_column(conn, "shell")? {
            conn.execute_batch(
                "ALTER TABLE history ADD COLUMN shell TEXT NOT NULL DEFAULT 'bash'",
            )?;
        }
        conn.pragma_update(None, "user_version", 3)?;
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

/// Records a command in history. If a row with the same (cwd, cmd)
/// already exists, it is refreshed instead of inserting a new one
/// (started_at/session/paths are updated, exit_code is reset). Like
/// fish, a duplicate command leaves only one row in history. Returns
/// the row id.
pub fn record_history(
    conn: &Connection,
    cwd: &str,
    cmd: &str,
    started_at: i64,
    session: &str,
    paths: &str,
    shell: &str,
) -> Result<i64> {
    // Atomic upsert on (cwd, cmd): a re-run refreshes the existing row
    // (started_at/session/paths/shell updated, exit_code reset) instead of
    // inserting a duplicate. The UNIQUE(cwd, cmd) index also removes the
    // check-then-write race between concurrent shells. Dedup stays
    // shell-independent; the shell column is an informational tag.
    //
    // Note: RETURNING id is unambiguous across SQLite versions and avoids
    // relying on last_insert_rowid() semantics on the ON CONFLICT / DO UPDATE path.
    let id: i64 = conn.query_row(
        "INSERT INTO history (cwd, cmd, started_at, session, paths, shell)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(cwd, cmd) DO UPDATE SET
           started_at = excluded.started_at,
           session = excluded.session,
           paths = excluded.paths,
           shell = excluded.shell,
           exit_code = NULL
         RETURNING id",
        rusqlite::params![cwd, cmd, started_at, session, paths, shell],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Updates the exit_code of a row by id. The session is not used for
/// matching: dedup may rewrite the row for another session's execution.
pub fn update_exit_code(conn: &Connection, id: i64, code: i64) -> Result<()> {
    conn.execute(
        "UPDATE history SET exit_code = ?1 WHERE id = ?2",
        rusqlite::params![code, id],
    )?;
    Ok(())
}

/// Deletes history rows by id. Nonexistent ids are silently ignored
/// (used to remove rows that accidentally recorded secrets).
pub fn delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(900) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM history WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        stmt.execute(rusqlite::params_from_iter(chunk))?;
    }
    Ok(())
}

/// Deletes history rows older than the newest `max` rows (by
/// started_at, id). Strictly keeps `max` rows even when many rows
/// share the same started_at — the old `started_at < threshold`
/// predicate kept ties at the boundary. When the table has fewer
/// than `max` rows, the subquery yields no row and nothing is
/// deleted. Callers must not pass `max = 0` (unlimited is handled
/// before calling).
pub fn trim_history(conn: &Connection, max: usize) -> Result<()> {
    conn.execute(
        "DELETE FROM history WHERE (started_at, id) < (
           SELECT started_at, id FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1
         )",
        rusqlite::params![max as i64 - 1],
    )?;
    Ok(())
}

/// Deletes all history rows and reclaims the file space (VACUUM).
/// Interactive command: success is silent, errors go to stderr.
pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute_batch("DELETE FROM history; VACUUM;")?;
    Ok(())
}

/// Returns the prefix-matching candidates in descending order of
/// recency, up to limit (cmd, paths). When cwd is Some, candidates are
/// restricted to that directory. When sensitive, matching is
/// case-sensitive (fish's autosuggestion prefers exact-case matches).
pub(crate) fn suggest_prefix(
    conn: &Connection,
    cwd: Option<&str>,
    needle: &str,
    limit: usize,
    sensitive: bool,
) -> Result<Vec<(String, String)>> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let (pattern, op_free, op_scoped) = if sensitive {
        let pat = format!("{}*", escape_glob(needle));
        (pat, "cmd GLOB ?1", "cmd GLOB ?2")
    } else {
        // LIKE needs an explicit ESCAPE clause: without it the
        // backslashes from escape_like are treated as ordinary
        // characters and `_`/`%` still act as wildcards.
        let pat = format!("{}%", escape_like(needle));
        (pat, "cmd LIKE ?1 ESCAPE '\\'", "cmd LIKE ?2 ESCAPE '\\'")
    };
    let sql = match cwd {
        Some(_) => format!(
            "SELECT cmd, paths FROM history WHERE cwd = ?1 AND {op_scoped}
             ORDER BY started_at DESC, id DESC LIMIT ?3"
        ),
        None => format!(
            "SELECT cmd, paths FROM history WHERE {op_free}
             ORDER BY started_at DESC, id DESC LIMIT ?2"
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = match cwd {
        Some(dir) => {
            stmt.query_map(rusqlite::params![dir, pattern, limit as i64], map_candidate)?
        }
        None => stmt.query_map(rusqlite::params![pattern, limit as i64], map_candidate)?,
    };
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Escapes the GLOB special characters (* ? [) using character
/// classes: SQLite's GLOB has no escape character, so [*], [?] and
/// [[] match those characters literally. \ and ] are literal outside
/// a class and need no escaping.
fn escape_glob(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '*' => vec!['[', '*', ']'],
            '?' => vec!['[', '?', ']'],
            '[' => vec!['[', '[', ']'],
            other => vec![other],
        })
        .collect()
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
