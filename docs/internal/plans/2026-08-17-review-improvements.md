# seasalt レビュー改善 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** コードレビューで発見した 6 項目(LIKE ESCAPE バグ / プロセス内タイムアウト / スキーマバージョン枠組み / 原子的 dedup / silence contract 集約 / テスト強化)を修正・改善する。

**Architecture:** 既存のモジュール構成(db / suggest / search / paths / integration)を維持した局所修正。`record` の dedup を `UNIQUE(cwd, cmd)` + upsert で原子的にし、`PRAGMA user_version` による段階的マイグレーションで既存データを一回だけ整理する。`suggest` の 200ms タイムアウトを外部 `timeout` コマンドから rusqlite `progress_handler` によるプロセス内実装へ移す。

**Tech Stack:** Rust (edition 2021), rusqlite 0.37 (bundled), clap, anyhow, dirs; bash スニペット; Nix flake

**Spec:** `docs/internal/specs/2026-08-15-seasalt-design.md`(§4 タイムアウト・§6 重複除去・§3 ストレージの文言更新を含む)

**検証済みの前提(実行時にコードで確認する):**
- SQLite の LIKE は `ESCAPE` 句なしでは `\` を通常文字扱い。`escape_like` が生成する `\_` パターンは現状壊れている(python3 in-memory SQLite で再現確認済み)。
- rusqlite 0.37 に `Connection::progress_handler(&self, num_ops: c_int, handler: Option<F>)`(F: `FnMut() -> bool + Send + 'static`)が存在。`true` で `SQLITE_INTERRUPT`。`num_ops < 1` で無効。
- 優先度: 新規 DB 作成時は user_version=0 → paths の必然で v1 → dedupe + UNIQUE index で v2。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy && cargo test` をこの順で実行し、全て green。最後のタスク後は `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK` を返すこと。
- コメントは英語。test fixture 文字列も英語。ユーザー向け出力は英語。
- コミットメッセージは英語。**各コミットは事前にユーザーへ依頼し、明示的な承認を得てから実行する**(AGENTS.md)。
- 本タスクと無関係のコードは変更しない。

---

### Task A: LIKE ESCAPE バグ修正(icase fallback の `_`/`%` 誤判定)

**Files:**
- Modify: `src/db.rs:200-228`(`suggest_prefix` の LIKE/GLOB 分岐)
- Test: `tests/suggest_test.rs`(新テスト 2 件追加)

**Interfaces:**
- Consumes: 既存 `db::suggest_prefix(conn, cwd: Option<&str>, needle, limit, sensitive) -> Result<Vec<(String,String)>>`
- Produces: 同一シグネチャ。`sensitive=false` のクエリに `cmd LIKE ? ESCAPE '\'` が付くようになる。

**背景:** `escape_like` は `%`/`_`/`\` をバックスラッシュでエスケープしたパターンを生成するが、SQLite は `ESCAPE` 句なしでは `\` を通常文字として扱うため、`_`/`%` を含む入力の icase フォールバックが誤動作する(実測で `ls \_%` は ESCAPE なしだと一致しない)。sensitive パス(GLOB)は正しいため、stale な exact-case 候補がある場合のみ顕在化する。

- [ ] **Step 1: 失敗するテストを追加** — `tests/suggest_test.rs` 末尾に:

```rust
#[test]
fn like_escape_handles_underscore_in_icase_fallback() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // Only a case-mismatched candidate exists: the sensitive GLOB pass
    // misses and the icase LIKE fallback must not treat `_` as a wildcard.
    db::record_history(&conn, "/proj/sub", "LS _x.txt", 2000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "ls _").unwrap().unwrap();
    assert_eq!(got, "LS _x.txt");
}

#[test]
fn like_escape_handles_percent_in_icase_fallback() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "PRINTF %s x", 2000, "s", "").unwrap();
    let got = suggest::suggest(&conn, "/proj/sub", "printf %s").unwrap().unwrap();
    assert_eq!(got, "PRINTF %s x");
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test suggest_test like_escape_handles_underscore_in_icase_fallback like_escape_handles_percent_in_icase_fallback`
  Expected: 両方 FAIL(`Option::unwrap` で panic)

- [ ] **Step 3: 実装** — `src/db.rs` の `suggest_prefix` 内、パターン・演算子構築部を置換:

```rust
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
```

(`pattern` 変数は既存の query_map 分岐で使用中のものと同じ 1 変数に統合。)

- [ ] **Step 4: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS(既存の `exact_case_preferred_when_line_contains_glob_chars` 等も green)

- [ ] **Step 5: コミット(要承認)** — `git add src/db.rs tests/suggest_test.rs && git commit -m "fix: escape LIKE metacharacters with an ESCAPE clause"`

---

### Task B: スキーママイグレーション枠組み(`PRAGMA user_version`)

**Files:**
- Modify: `src/db.rs:7-21,84-91`(`SCHEMA`・`init`)
- Test: `tests/db_test.rs`(新テスト追加)

**Interfaces:**
- Consumes: 既存 `init(conn) -> Result<()>`
- Produces: `init` が `PRAGMA user_version` を読み、未達なら段階的に適用・記録する。このタスクでは DB を v1 まで引き上げる(paths 列の後付けを 1 回の判定に)。SQL 実行後のパターンは変更しない。

- [ ] **Step 1: 失敗するテストを追加** — `tests/db_test.rs` 末尾に(この時点では失敗する):

```rust
#[test]
fn init_records_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 1);
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test db_test init_records_schema_version`
  Expected: FAIL(現在は user_version=0)

- [ ] **Step 3: 実装** — `src/db.rs` の `init` を置換し、ヘルパーを追加:

```rust
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
            conn.execute_batch(
                "ALTER TABLE history ADD COLUMN paths TEXT NOT NULL DEFAULT ''",
            )?;
        }
        conn.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}
```

- [ ] **Step 4: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS(`init_records_schema_version` PASS、既存 `init_adds_paths_column_to_old_schema` 等も green。fresh DB・legacy DB どちらも v1 になる)

- [ ] **Step 5: コミット(要承認)** — `git add src/db.rs tests/db_test.rs && git commit -m "refactor: version schema migrations via PRAGMA user_version"`

---

### Task C: 原子的 dedup(UNIQUE index + upsert)+ 旧データ整理

**Files:**
- Modify: `src/db.rs:7-21,105-142,164-176`(`SCHEMA`・`record_history`・`migrate`)
- Modify: `docs/internal/specs/2026-08-15-seasalt-design.md:64-81,165-168`(インデックス構成と重複除去方針の更新)
- Test: `tests/db_test.rs`(v2 への更新・重複整理テスト追加)

**Interfaces:**
- Consumes: Task B の `migrate`(v1 記録済み)
- Produces: `record_history` が単一ステートメントの upsert になり、`(cwd, cmd)` に対する UNIQUE index が存在する状態。`SCHEMA` から旧 `idx_history_cwd_cmd` が消え、`idx_history_cwd_cmd_unique` に置き換わる。DB は v2 まで引き上げる。

**背景:** 現行の SELECT→INSERT/UPDATE は check-then-write で、複数シェルの同時 record に競合レースがある。`UNIQUE(cwd, cmd)` + `INSERT ... ON CONFLICT(cwd, cmd) DO UPDATE` で原子的になる(最近値のみ保持・exit_code リセット、既存 `last_insert_rowid()` は conflicted-update でも更新行の id を返す — bundled SQLite ≥3.35 で保証)。既存 DB に古い重複行があると UNIQUE index 生成が失敗するため、v2 マイグレーションで `(cwd, cmd)` ごとに最新 1 行を残して一度だけ整理する(ユーザー承認済み)。

- [ ] **Step 1: 失敗するテストを追加** — `tests/db_test.rs`:

```rust
#[test]
fn init_dedupes_legacy_duplicates_and_adds_unique_index() {
    let conn = Connection::open_in_memory().unwrap();
    // Simulate a legacy DB: no paths column, duplicate (cwd, cmd) rows
    conn.execute_batch(
        "CREATE TABLE history (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          cwd TEXT NOT NULL,
          cmd TEXT NOT NULL,
          exit_code INTEGER,
          started_at INTEGER NOT NULL,
          session TEXT
        );",
    )
    .unwrap();
    conn.execute("INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 1000)", [])
        .unwrap();
    conn.execute("INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 2000)", [])
        .unwrap();
    conn.execute("INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'make', 3000)", [])
        .unwrap();
    conn.execute("INSERT INTO history (cwd, cmd, started_at) VALUES ('/x', 'test', 2500)", [])
        .unwrap();

    db::init(&conn).unwrap();

    let (count, latest): (i64, i64) = conn
        .query_row(
            "SELECT count(*), max(started_at) FROM history",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // 4 rows collapse to the newest 2 (one per (cwd, cmd))
    assert_eq!(count, 2);
    assert_eq!(latest, 3000);
    let name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index'
             AND name = 'idx_history_cwd_cmd_unique'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_history_cwd_cmd_unique");
}
```

既存テスト `init_records_schema_version` の期待値を 2 に更新(Step 3 と同じコミットで): `assert_eq!(v, 2);`

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test db_test init_dedupes_legacy_duplicates_and_adds_unique_index`
  Expected: FAIL(unique index が存在しない)

- [ ] **Step 3: 実装**

`src/db.rs` SCHEMA(7-21 行)から `idx_history_cwd_cmd` 行を削除。

`src/db.rs` `migrate` に v2 ステップを追加:

```rust
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
```

`src/db.rs` `record_history` を置換(SELECT 事前照合を削除):

```rust
pub fn record_history(
    conn: &Connection,
    cwd: &str,
    cmd: &str,
    started_at: i64,
    session: &str,
    paths: &str,
) -> Result<i64> {
    // Atomic upsert on (cwd, cmd): a re-run refreshes the existing row
    // (started_at/session/paths updated, exit_code reset) instead of
    // inserting a duplicate. The UNIQUE(cwd, cmd) index also removes the
    // check-then-write race between concurrent shells.
    conn.execute(
        "INSERT INTO history (cwd, cmd, started_at, session, paths)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(cwd, cmd) DO UPDATE SET
           started_at = excluded.started_at,
           session = excluded.session,
           paths = excluded.paths,
           exit_code = NULL",
        rusqlite::params![cwd, cmd, started_at, session, paths],
    )?;
    Ok(conn.last_insert_rowid())
}
```

設計仕様の更新:
- `docs/internal/specs/2026-08-15-seasalt-design.md` §3 ストレージ(75-76 行)付近: `idx_history_cwd_cmd` の記述を `idx_history_cwd_cmd_unique`(UNIQUE)へ、`paths 列の後付けは初回接続時に ALTER で自動マイグレーション`の文言に `PRAGMA user_version` による段階適用を追記。
- 同仕様 §6 重複除去(165-167 行付近): 「既に溜まっている旧データの重複行は放置する」を「初回 upgrade 時に `(cwd, cmd)` ごとに最新 1 行へ整理する」に更新。record の dedup が原子的 upsert になった旨も追記。

- [ ] **Step 4: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS。既存 `record_dedups_same_command_and_bumps_to_latest`(id1==id3 を upsert 後の `last_insert_rowid` で満たす)と `deduped_record_resets_exit_code_until_exit` が正しく通ること。

- [ ] **Step 5: コミット(要承認)** — `git add src/db.rs tests/db_test.rs docs/internal/specs/2026-08-15-seasalt-design.md && git commit -m "refactor: make history dedup atomic with a unique (cwd, cmd) index"`

---

### Task D: suggest のプロセス内タイムアウト(外部 `timeout` 除去)

**Files:**
- Modify: `src/suggest.rs`(`suggest` へ予算パラメータ追加)
- Modify: `src/main.rs:112-118`(`Suggest` 分岐に予算設定)
- Modify: `src/bash/seasalt.bash:56-65`(`timeout` 除去)
- Modify: `README.md`、`docs/internal/specs/2026-08-15-seasalt-design.md:126-128`、`AGENTS.md`(timeout 前提の記述更新)
- Test: `tests/suggest_test.rs`(新テスト追加)

**Interfaces:**
- Consumes: 既存 `suggest::suggest(conn, cwd, line) -> Result<Option<String>>`
- Produces: `pub fn suggest_budgeted(conn: &Connection, cwd: &str, line: &str, budget: Option<Duration>) -> Result<Option<String>>`。`suggest` は `suggest_budgeted(..., None)` への薄いラッパー(既存呼び出し・bench は不変)。`main.rs` が `Some(Duration::from_millis(200))` で呼ぶ。スニペットから `timeout 0.2` が消える。

**背景:** 現行の `timeout 0.2 "$bin" suggest ...` は「GNU coreutils の `timeout` + seasalt」で毎打鍵 2 回プロセスを起動しており(macOS では coreutils 必須)、ベンチ(設計書性能表)は `timeout` を介さないため実環境コストを過小計測している。プロセス内タイムアウトにすれば依存と 1 回の spawn が両方消える。`progress_handler` が期限超過で `SQLITE_INTERRUPT` を発生させ、スコープ境界の明示チェックでゼロ予算でも確定的に即座に打ち切れる。

- [ ] **Step 1: 失敗するテストを追加** — `tests/suggest_test.rs`:

```rust
#[test]
fn zero_budget_aborts_before_any_query() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/proj/sub", "cargo build", 5000, "s", "").unwrap();

    // An expired budget yields no suggestion (mid-query interrupts are
    // swallowed; the pre-query deadline check makes this deterministic).
    let got = suggest::suggest_budgeted(
        &conn,
        "/proj/sub",
        "cargo",
        Some(std::time::Duration::ZERO),
    )
    .unwrap();
    assert!(got.is_none());

    // A generous budget, by contrast, still suggests the match.
    let got = suggest::suggest_budgeted(
        &conn,
        "/proj/sub",
        "cargo",
        Some(std::time::Duration::from_secs(10)),
    )
    .unwrap();
    assert_eq!(got, Some("cargo build".to_string()));
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test suggest_test zero_budget_aborts_before_any_query`
  Expected: FAIL(`suggest_budgeted` 未定義)

- [ ] **Step 3: 実装** — `src/suggest.rs` を置換(non-scope ロジックは既存のまま移設):

```rust
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
        Err(e) if matches!(e.downcast_ref(), Some(rusqlite::Error::Interrupt)) => Ok(None),
        Err(e) => Err(e),
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
    // Scope 1: exact cwd match
    match search_scope(conn, cwd, line, Some(cwd))? {
        Outcome::Suggest(cmd) => return Ok(Some(cmd)),
        Outcome::Identical => return Ok(None),
        Outcome::NoMatch => {}
    }
    // Scope 2: parent directories (nearest first)
    for anc in ancestors(cwd) {
        if expired(deadline) {
            return Ok(None);
        }
        match search_scope(conn, cwd, line, Some(&anc))? {
            Outcome::Suggest(cmd) => return Ok(Some(cmd)),
            Outcome::Identical => return Ok(None),
            Outcome::NoMatch => {}
        }
    }
    // Scope 3: global
    if expired(deadline) {
        return Ok(None);
    }
    match search_scope(conn, cwd, line, None)? {
        Outcome::Suggest(cmd) => Ok(Some(cmd)),
        Outcome::Identical | Outcome::NoMatch => Ok(None),
    }
}
```

`Outcome`、`pick`、`search_scope`、`ancestors` は既存のまま(`search_scope` の `db::suggest_prefix` 呼び出しで検索中に中断された場合は `rusqlite::Error::Interrupt` として伝播し、上記 `downcast_ref` で吸収される)。

`src/main.rs` — `Command::Suggest` 分岐を置換し、冒頭 import に追加:

```rust
            let conn = open_db()?;
            let line = line.join(" ");
            if let Some(cmd) = seasalt::suggest::suggest_budgeted(
                &conn,
                &cwd,
                &line,
                Some(Duration::from_millis(200)),
            )? {
                println!("{cmd}");
            }
```

```rust
use std::time::Duration;
```

`src/bash/seasalt.bash` — `source:seasalt` 関数の該当行を置換:

```bash
  cmd=$("$_seasalt_bin" suggest --cwd "$PWD" -- "$_ble_edit_str" 2>/dev/null) || return 1;
```

スニペット側のコメント(現状「`seasalt suggest` is called with a timeout so a slow database never freezes the UI; on timeout the suggestion is simply skipped.」)を「the binary enforces an in-process 200ms timeout (`suggest_budgeted`), so a slow database never freezes the UI. Do not add an external `timeout` wrapper or background the call — the synchronous call is required (spec §4).」に更新。

ドキュメント更新:
- `README.md` の設定/要件関連で `timeout`/coreutils への言及を確認し、あれば削除。
- `docs/internal/specs/2026-08-15-seasalt-design.md` §4(126-128 行付近): 「`timeout 0.2` で 200ms を超えたら補完なしで継続する (`timeout` は GNU coreutils 由来。macOS では coreutils の導入が必要)」を「`suggest` 自体がプロセス内で 200ms のタイムアウトを課し、超過時は補完なしで継続する(外部 `timeout` 不要)」に更新。
- `AGENTS.md` のゴッチャ行「The snippet uses GNU coreutils `timeout` (macOS needs coreutils).」を「The snippet calls `seasalt suggest` directly; the binary enforces the 200ms timeout in-process. Do not reintroduce an external `timeout` wrapper or backgrounding.」に置換。

- [ ] **Step 4: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS。その後 `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`(スニペット変更の smoke 確認)

- [ ] **Step 5: コミット(要承認)** — `git add src/suggest.rs src/main.rs src/bash/seasalt.bash README.md docs/internal/specs/2026-08-15-seasalt-design.md AGENTS.md tests/suggest_test.rs && git commit -m "perf: timeout suggest in-process instead of via coreutils timeout"`

---

### Task E: silence contract の分岐一元化(`Command::interactive()`)

**Files:**
- Modify: `src/main.rs:69-86`

**Interfaces:**
- Consumes: 既存 `Command` enum
- Produces: `impl Command { fn interactive(&self) -> bool }`。`main` が `cli.command.interactive()` を使う。外部インターフェース不一致なし。既存テスト(`record_failure_is_silent` / `init_bash_rejects_unknown_shell`)が保証の回帰検証となる。

- [ ] **Step 1: 実装** — `src/main.rs` の `Command` enum 直後に追加:

```rust
impl Command {
    /// Interactive subcommands (init/search/delete/clear) report errors
    /// on stderr; hook-facing subcommands (record/exit/suggest) fail
    /// silently. Colocated here so new subcommands cannot silently
    /// forget to classify themselves.
    fn interactive(&self) -> bool {
        matches!(
            self,
            Command::Init { .. }
                | Command::Search { .. }
                | Command::Delete { .. }
                | Command::Clear
        )
    }
}
```

`main` 内の `interactive` 導出を置換:

```rust
    let interactive = cli.command.interactive();
```

(69-86 行の `matches!` を削除)

- [ ] **Step 2: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS(`record_failure_is_silent` で stderr 空、`init_bash_rejects_unknown_shell` で stderr 有り)

- [ ] **Step 3: コミット(要承認)** — `git add src/main.rs && git commit -m "refactor: classify subcommand error reporting via Command::interactive"`

---

### Task F: テスト強化(search ワイルドカード・bash_test 後始末)

**Files:**
- Test: `tests/cli_test.rs`(新テスト 1 件)
- Test: `tests/bash_test.rs`(一時ディレクトリ後始末追加)

- [ ] **Step 1: テスト追加** — `tests/cli_test.rs`:

```rust
#[test]
fn search_pattern_wildcards_follow_sql_like() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/a",
            "--session",
            "s1",
            "--",
            "unique-abc-xyz",
        ])
        .status()
        .unwrap();
    // '%' matches any sequence (trailing wildcard here)
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "unique-%"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\tunique-abc-xyz");
    // '_' matches exactly one character (the '-')
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "unique_abc"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\tunique-abc-xyz");
    // A literal-without-wildcard pattern only matches when it fits
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "unique-abc-xzz"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
}
```

`tests/bash_test.rs` の `init_bash_survives_eval_command_substitution`(38-47 行)と `init_bash_reorders_auto_source_after_idle`(81-89 行)のスクリプト内、`SEASALT_DATA_DIR=$(mktemp -d)` の直後に追加:

```bash
trap 'rm -rf "$SEASALT_DATA_DIR"' EXIT
```

- [ ] **Step 2: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS(`search_pattern_wildcards_follow_sql_like` PASS、bash_test 3 件 PASS)

- [ ] **Step 3: コミット(要承認)** — `git add tests/cli_test.rs tests/bash_test.rs && git commit -m "test: cover search wildcards and clean up bash test temp dirs"`

---

### 最終検証(全タスク完了後)

- [ ] `cargo fmt && cargo check && cargo clippy && cargo test` — 全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` — `OK`
- [ ] `cargo bench`(オプション:suggest 経路の性能回帰確認)※CI 対象外のため実行確認のみ推奨
- [ ] `git status` clean、`git log --oneline -10` でコミット内容確認