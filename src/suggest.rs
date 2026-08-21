use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::paths;

/// Maximum number of candidates examined per scope
const CANDIDATE_LIMIT: usize = 10;

/// Approximate VM instructions between progress-handler invocations
/// (checking the deadline is cheap, so this stays coarse)
const PROGRESS_OPS: i32 = 100_000;

pub fn suggest(conn: &Connection, cwd: &str, line: &str) -> Result<Option<String>> {
    suggest_budgeted(conn, cwd, line, None)
}

/// Like `suggest`, but stops once `budget` elapses (returning None).
/// `budget = None` installs no timer and behaves exactly like `suggest`.
/// The shell passes a ~200ms budget so a slow database never freezes the
/// UI; on timeout the suggestion is simply skipped.
pub fn suggest_budgeted(
    conn: &Connection,
    cwd: &str,
    line: &str,
    budget: Option<Duration>,
) -> Result<Option<String>> {
    let deadline = budget.map(|b| Instant::now() + b);
    if let Some(deadline) = deadline {
        let dl = deadline;
        conn.progress_handler(PROGRESS_OPS, Some(move || Instant::now() >= dl));
    }
    let out = search(conn, cwd, line, deadline);
    if budget.is_some() {
        // Leave the connection reusable (it is a fresh one per call anyway)
        conn.progress_handler(PROGRESS_OPS, None::<fn() -> bool>);
    }
    match out {
        Ok(v) => Ok(v),
        Err(e) => {
            if let Some(rusqlite::Error::SqliteFailure(ref err, _)) =
                e.downcast_ref::<rusqlite::Error>()
            {
                if err.code == rusqlite::ErrorCode::OperationInterrupted {
                    return Ok(None);
                }
            }
            Err(e)
        }
    }
}

fn expired(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// The scoped search (the former `suggest` body), with deadline checks
/// between the per-scope queries.
fn search(
    conn: &Connection,
    cwd: &str,
    line: &str,
    deadline: Option<Instant>,
) -> Result<Option<String>> {
    if line.trim().is_empty() || expired(deadline) {
        return Ok(None);
    }
    // Normalize trailing slashes so "/proj/sub/" behaves like "/proj/sub"
    let cwd_norm = {
        let t = cwd.trim_end_matches('/');
        if t.is_empty() {
            "/"
        } else {
            t
        }
    };
    // Scope 1: exact cwd match
    match search_scope(conn, cwd_norm, line, Some(cwd_norm), deadline)? {
        Outcome::Suggest(cmd) => return Ok(Some(cmd)),
        Outcome::Identical => return Ok(None),
        Outcome::NoMatch => {}
    }
    // Scope 2: parent directories (nearest first)
    for anc in ancestors(cwd_norm) {
        if expired(deadline) {
            return Ok(None);
        }
        match search_scope(conn, cwd_norm, line, Some(&anc), deadline)? {
            Outcome::Suggest(cmd) => return Ok(Some(cmd)),
            Outcome::Identical => return Ok(None),
            Outcome::NoMatch => {}
        }
    }
    // Scope 3: global
    if expired(deadline) {
        return Ok(None);
    }
    match search_scope(conn, cwd_norm, line, None, deadline)? {
        Outcome::Suggest(cmd) => Ok(Some(cmd)),
        Outcome::Identical | Outcome::NoMatch => Ok(None),
    }
}

/// Lists the parent directories in order of closeness (root itself is not included).
/// Trailing slashes are normalized so "/a/b/" and "/a/b//" behave like "/a/b".
pub fn ancestors(cwd: &str) -> Vec<String> {
    let normalized = cwd.trim_end_matches('/');
    let normalized = if normalized.is_empty() {
        "/"
    } else {
        normalized
    };
    let mut out = Vec::new();
    let mut cur = std::path::Path::new(normalized);
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
#[derive(Debug, PartialEq)]
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
fn pick(
    cwd: &str,
    line: &str,
    candidates: Vec<(String, String)>,
    deadline: Option<Instant>,
) -> Outcome {
    for (cmd, paths) in candidates {
        // Stop scanning once the budget has elapsed: the filesystem checks
        // below are not covered by the SQLite progress_handler.
        if expired(deadline) {
            return Outcome::NoMatch;
        }
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
fn search_scope(
    conn: &Connection,
    cwd: &str,
    line: &str,
    dir: Option<&str>,
    deadline: Option<Instant>,
) -> Result<Outcome> {
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, true)?;
    match pick(cwd, line, candidates, deadline) {
        Outcome::NoMatch => {}
        other => return Ok(other),
    }
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, false)?;
    Ok(pick(cwd, line, candidates, deadline))
}

#[cfg(test)]
mod tests {
    use super::pick;
    use super::Outcome;
    use std::time::{Duration, Instant};

    #[test]
    fn pick_returns_nomatch_when_deadline_expired() {
        // Even a valid, matching candidate is not selected once the deadline
        // has passed: pick must stop instead of scanning further candidates.
        let expired = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(
            pick(
                "/x",
                "cargo",
                vec![("cargo build".to_string(), String::new())],
                expired,
            ),
            Outcome::NoMatch
        );
    }

    #[test]
    fn pick_selects_first_valid_candidate() {
        // Regression guard: no deadline -> the first valid candidate wins.
        assert_eq!(
            pick(
                "/x",
                "cargo",
                vec![("cargo build".to_string(), String::new())],
                None,
            ),
            Outcome::Suggest("cargo build".to_string())
        );
    }

    #[test]
    fn pick_returns_identical_for_matching_line() {
        assert_eq!(
            pick(
                "/x",
                "cargo build",
                vec![("cargo build".to_string(), String::new())],
                None,
            ),
            Outcome::Identical
        );
    }
}
