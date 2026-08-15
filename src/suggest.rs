use anyhow::Result;
use rusqlite::Connection;

use crate::db;

/// 親ディレクトリを近い順に列挙(ルート自体は含まない)
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

pub fn suggest(conn: &Connection, cwd: &str, line: &str) -> Result<Option<String>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    // スコープ 1: cwd 完全一致
    if let Some(cmd) = db::suggest_in_dir(conn, cwd, line)? {
        if cmd != line {
            return Ok(Some(cmd));
        }
        return Ok(None);
    }
    // スコープ 2: 親ディレクトリ(近い順)
    for anc in ancestors(cwd) {
        if let Some(cmd) = db::suggest_in_dir(conn, &anc, line)? {
            if cmd != line {
                return Ok(Some(cmd));
            }
            return Ok(None);
        }
    }
    // スコープ 3: グローバル
    if let Some(cmd) = db::suggest_global(conn, line)? {
        if cmd != line {
            return Ok(Some(cmd));
        }
    }
    Ok(None)
}
