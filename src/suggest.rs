use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::paths;

/// Maximum number of candidates examined per scope
const CANDIDATE_LIMIT: usize = 10;

/// Lists the parent directories in order of closeness (root itself is not included)
pub fn ancestors(cwd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = std::path::Path::new(cwd);
    while let Some(parent) = cur.parent() {
        let s = parent.to_string_lossy();
        if s.is_empty() || s == "/" {
            break;
        }
        out.push(s.into_owned());
        cur = parent;
    }
    out
}

/// Result of selecting among candidates
enum Outcome {
    /// Command to suggest
    Suggest(String),
    /// A candidate identical to the input line was found (no suggestion)
    Identical,
    /// No valid candidate was found
    NoMatch,
}

/// Picks the first valid candidate. Candidates referencing deleted
/// files are skipped; Identical is returned when one equals the line.
fn pick(cwd: &str, line: &str, candidates: Vec<(String, String)>) -> Outcome {
    for (cmd, paths) in candidates {
        if !paths::valid(cwd, &paths) {
            continue;
        }
        return if cmd == line {
            Outcome::Identical
        } else {
            Outcome::Suggest(cmd)
        };
    }
    Outcome::NoMatch
}

/// Searches one scope. Exact-case matches are preferred, falling back
/// to the latest case-insensitive match (like fish).
fn search_scope(conn: &Connection, cwd: &str, line: &str, dir: Option<&str>) -> Result<Outcome> {
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, true)?;
    match pick(cwd, line, candidates) {
        Outcome::NoMatch => {}
        other => return Ok(other),
    }
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, false)?;
    Ok(pick(cwd, line, candidates))
}

pub fn suggest(conn: &Connection, cwd: &str, line: &str) -> Result<Option<String>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    // Scope 1: exact cwd match
    match search_scope(conn, cwd, line, Some(cwd))? {
        Outcome::Suggest(cmd) => return Ok(Some(cmd)),
        Outcome::Identical => return Ok(None),
        Outcome::NoMatch => {}
    }
    // Scope 2: parent directories (nearest first)
    for anc in ancestors(cwd) {
        match search_scope(conn, cwd, line, Some(&anc))? {
            Outcome::Suggest(cmd) => return Ok(Some(cmd)),
            Outcome::Identical => return Ok(None),
            Outcome::NoMatch => {}
        }
    }
    // Scope 3: global
    match search_scope(conn, cwd, line, None)? {
        Outcome::Suggest(cmd) => Ok(Some(cmd)),
        Outcome::Identical | Outcome::NoMatch => Ok(None),
    }
}
