use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::paths;

/// 各スコープで調査する候補の最大件数
const CANDIDATE_LIMIT: usize = 10;

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

/// 候補の選択結果
enum Outcome {
    /// 提案するコマンド
    Suggest(String),
    /// 入力行と同一のコマンドが見つかった (提案しない)
    Identical,
    /// 有効な候補が無かった
    NoMatch,
}

/// 候補リストから最初の有効な候補を選ぶ。
/// 削除済みファイルを参照する候補はスキップし、入力行と同一なら Identical。
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

/// 一つのスコープを検索する。exact-case 一致を優先し、
/// 無ければ case-insensitive の最新候補にフォールバックする (fish と同じ)。
fn search_scope(conn: &Connection, cwd: &str, line: &str, dir: Option<&str>) -> Result<Outcome> {
    let sensitive = match dir {
        Some(dir) => db::suggest_in_dir(conn, dir, line, CANDIDATE_LIMIT, true)?,
        None => db::suggest_global(conn, line, CANDIDATE_LIMIT, true)?,
    };
    match pick(cwd, line, sensitive) {
        Outcome::NoMatch => {}
        other => return Ok(other),
    }
    let icase = match dir {
        Some(dir) => db::suggest_in_dir(conn, dir, line, CANDIDATE_LIMIT, false)?,
        None => db::suggest_global(conn, line, CANDIDATE_LIMIT, false)?,
    };
    Ok(pick(cwd, line, icase))
}

pub fn suggest(conn: &Connection, cwd: &str, line: &str) -> Result<Option<String>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    // スコープ 1: cwd 完全一致
    match search_scope(conn, cwd, line, Some(cwd))? {
        Outcome::Suggest(cmd) => return Ok(Some(cmd)),
        Outcome::Identical => return Ok(None),
        Outcome::NoMatch => {}
    }
    // スコープ 2: 親ディレクトリ(近い順)
    for anc in ancestors(cwd) {
        match search_scope(conn, cwd, line, Some(&anc))? {
            Outcome::Suggest(cmd) => return Ok(Some(cmd)),
            Outcome::Identical => return Ok(None),
            Outcome::NoMatch => {}
        }
    }
    // スコープ 3: グローバル
    match search_scope(conn, cwd, line, None)? {
        Outcome::Suggest(cmd) => Ok(Some(cmd)),
        Outcome::Identical | Outcome::NoMatch => Ok(None),
    }
}
