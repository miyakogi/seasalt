# 履歴肥大化対策 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 履歴 DB に件数上限（デフォルト 100,000 件、`SEASALT_HISTORY_MAX` で設定変更）の自動トリムと `seasalt clear` サブコマンドを追加する。

**Architecture:** 既存の db/main 構造を維持した局所追加。トリムは record 経路に毎回載せ、`started_at` 降順で新しい方を上限分残す。スキーマに `idx_history_started_at` を追加（`CREATE INDEX IF NOT EXISTS` で既存 DB も自動適用）。clear は全件削除 + VACUUM。

**Tech Stack:** Rust (edition 2021), rusqlite, clap, anyhow; bash smoke test; Criterion bench

**Spec:** docs/internal/specs/2026-08-16-history-limit-design.md（本プランの要求の権威。実装者は spec とこのプランを両方読む）

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy && cargo test` をこの順で実行し、全て green（clippy は --all-targets で benches を含める）
- Task 2 は追加で `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK` であること
- Task 3 は追加で `cargo bench --bench suggest "trim_history"` が完走すること（criterion のフィルタで trim グループのみ実行）
- コメントは英語、テスト fixture も英語。設計 spec は日本語（既存文書）
- トリムは record ごとに実行（PID 間引きはしない — spec §2 参照）
- `SEASALT_HISTORY_MAX`: 未設定/パース失敗 → 100,000 / `0` → 無制限 / `N` → N
- スコープ外: TTL トリム、VACUUM 定期実行、設定ファイル化、delete/search/suggest の挙動変更、bash スニペットの変更
- コミットはユーザー事前承認済み（2026-08-16 確認済み）

---

### Task 1: DB 層（スキーマ + trim_history + clear）

**Files:**
- Modify: `src/db.rs`
- Test: `tests/db_test.rs`

**Interfaces:**
- Produces:
  - `pub fn trim_history(conn: &Connection, max: usize) -> Result<()>`
  - `pub fn clear(conn: &Connection) -> Result<()>`
  - SCHEMA に `idx_history_started_at` 追加

- [ ] **Step 1: 失敗するテストを追加** — `tests/db_test.rs` 末尾に追加:

```rust
#[test]
fn trim_history_keeps_newest_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..12 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "")
            .unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
    let cmds: Vec<String> = conn
        .prepare("SELECT cmd FROM history ORDER BY started_at")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(cmds, (2..12).map(|i| format!("cmd {i}")).collect::<Vec<_>>());
}

#[test]
fn trim_history_at_limit_deletes_nothing() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..10 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "")
            .unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
}

#[test]
fn trim_history_under_limit_deletes_nothing() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..5 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "")
            .unwrap();
    }
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 5);
}

#[test]
fn trim_history_protects_refreshed_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    // "make" is the oldest row, then refreshed by a re-run (dedup)
    db::record_history(&conn, "/x", "make", 1000, "s", "").unwrap();
    for i in 1..12 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "")
            .unwrap();
    }
    db::record_history(&conn, "/x", "make", 9999, "s", "").unwrap();
    db::trim_history(&conn, 10).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 10);
    let has_make: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM history WHERE cmd = 'make')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_make);
}

#[test]
fn clear_removes_all_rows() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    for i in 0..5 {
        db::record_history(&conn, "/x", &format!("cmd {i}"), 1000 + i as i64, "s", "")
            .unwrap();
    }
    db::clear(&conn).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn init_creates_started_at_index() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_history_started_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "idx_history_started_at");
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test db_test trim_history_`
  Expected: FAIL（`trim_history` 関数が存在しないためコンパイルエラー）

- [ ] **Step 3: 実装** — `src/db.rs` に実装:

SCHEMA 定数の最後（既存インデックスの後）に追加:

```sql
CREATE INDEX IF NOT EXISTS idx_history_started_at ON history(started_at);
```

`delete_by_ids` の後に追加:

```rust
/// Deletes history rows older than the newest `max` rows (by
/// started_at, newest first). When the table has fewer than `max`
/// rows, the subquery yields NULL and nothing is deleted. Callers
/// must not pass `max = 0` (unlimited is handled before calling).
pub fn trim_history(conn: &Connection, max: usize) -> Result<()> {
    conn.execute(
        "DELETE FROM history WHERE started_at < (
           SELECT started_at FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1
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
```

- [ ] **Step 4: テスト確認** — Run: `cargo test --test db_test`
  Expected: 全 PASS

- [ ] **Step 5: ゲート確認** — Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test`
  Expected: 全て green

- [ ] **Step 6: コミット** — `git add src/db.rs tests/db_test.rs && git commit -m "feat: add history trimming and clear to the db layer"`

---

### Task 2: CLI 層（record トリム + clear サブコマンド）

**Files:**
- Modify: `src/main.rs`
- Test: `tests/cli_test.rs`（新テスト 3 件追加）
- Modify: `tests/bash/smoke.sh`（clear の節を追加）

**Consumes:** Task 1 の `db::trim_history(conn, max)` と `db::clear(conn)`、`idx_history_started_at`

- [ ] **Step 1: 失敗するテストを追加** — `tests/cli_test.rs` 末尾に追加:

```rust
#[test]
fn record_trims_to_history_max() {
    let dir = temp_data_dir();
    for i in 0..12 {
        let cmd = format!("cmd {i}");
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .env("SEASALT_HISTORY_MAX", "10")
            .args(["record", "--cwd", "/x", "--session", "s1", "--", &cmd])
            .status()
            .unwrap();
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "cmd"])
        .output()
        .unwrap();
    let lines: Vec<&str> = String::from_utf8(out.stdout).unwrap().lines().collect();
    assert_eq!(lines.len(), 10);
    assert!(lines.iter().any(|l| l.ends_with("cmd 11")));
    assert!(!lines.iter().any(|l| l.ends_with("cmd 0")));
}

#[test]
fn history_max_zero_disables_trim() {
    let dir = temp_data_dir();
    for i in 0..12 {
        let cmd = format!("cmd {i}");
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .env("SEASALT_HISTORY_MAX", "0")
            .args(["record", "--cwd", "/x", "--session", "s1", "--", &cmd])
            .status()
            .unwrap();
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "cmd"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 12);
}

#[test]
fn clear_removes_all_history() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "echo hello"])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["clear"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "hello"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test cli_test record_trims_to_history_max`
  Expected: FAIL（トリム未実装のため 12 行が残り `assert_eq!(lines.len(), 10)` が失敗）

- [ ] **Step 3: 実装** — `src/main.rs` に実装:

`Command` enum に追加（`Delete` の後、`Init` の前）:

```rust
    /// Delete all history entries and reclaim the file space (VACUUM)
    Clear,
```

`main()` の interactive 判定を更新:

```rust
    let interactive = matches!(
        cli.command,
        Command::Init { .. }
            | Command::Search { .. }
            | Command::Delete { .. }
            | Command::Clear
    );
```

`run()` の `Command::Record` 分岐の `record_history` 呼び出し後に追加:

```rust
            if let Some(max) = history_max() {
                seasalt::db::trim_history(&conn, max)?;
            }
```

`run()` に `Command::Clear` 分岐を追加:

```rust
        Command::Clear => {
            let conn = open_db()?;
            seasalt::db::clear(&conn)?;
        }
```

`now_ms` の近くにヘルパーを追加:

```rust
/// Default history row limit when SEASALT_HISTORY_MAX is unset
const DEFAULT_HISTORY_MAX: usize = 100_000;

/// Resolves the history row limit from SEASALT_HISTORY_MAX. Unset or
/// unparsable values fall back to the default; "0" means unlimited
/// (None). record is hook-facing and silent, so failures are not
/// reported.
fn history_max() -> Option<usize> {
    match std::env::var("SEASALT_HISTORY_MAX") {
        Ok(v) if v == "0" => None,
        Ok(v) => Some(v.parse::<usize>().unwrap_or(DEFAULT_HISTORY_MAX)),
        Err(_) => Some(DEFAULT_HISTORY_MAX),
    }
}
```

- [ ] **Step 4: smoke.sh に clear の節を追加** — `tests/bash/smoke.sh` の `run_suite` 内、**末尾**（`unset _ble_edit_str _ble_edit_ind` の後）に追加。途中に挿入すると auto-complete 節 (suggest が `normal-after-private` を期待) が壊れるため必ず末尾:

```bash
  # clear removes everything
  _seasalt_preexec "echo to-be-cleared"
  _seasalt_precmd
  "$BIN" clear
  out=$("$BIN" search --all to-be-cleared)
  [[ -z $out ]] || fail "clear did not remove history: $out"
```

- [ ] **Step 5: テスト確認** — Run: `cargo test --test cli_test`（新規 3 件 + 既存が全て green。既存テストは SEASALT_HISTORY_MAX 未設定で行数が少ないためトリムの影響なし）

- [ ] **Step 6: ゲート + smoke 確認** — Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test && cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
  Expected: 全て green / `OK`

- [ ] **Step 7: コミット** — `git add src/main.rs tests/cli_test.rs tests/bash/smoke.sh && git commit -m "feat: enforce history max on record and add clear command"`

---

### Task 3: ベンチマーク追加

**Files:**
- Modify: `benches/suggest.rs`

**Consumes:** Task 1 の `db::trim_history(conn, max)`、既存のシード 100k 行 DB（`dir.db_path(100_000)`）

- [ ] **Step 1: 実装** — `bench_in_process` の後に追加:

```rust
/// Cost of the automatic trim on record: under the limit (the steady
/// state, 0 rows deleted) and over the limit (deleting 10k rows once
/// the limit is crossed).
fn bench_trim(c: &mut Criterion, dir: &BenchDir) {
    let path = dir.db_path(100_000);
    let mut group = c.benchmark_group("trim_history");
    group.sample_size(50);
    group.bench_function("under_limit_100k", |b| {
        b.iter(|| {
            let conn = db::open(&path).unwrap();
            black_box(db::trim_history(&conn, 100_000).unwrap());
        })
    });
    group.sample_size(10);
    group.bench_function("over_limit_delete_10k", |b| {
        b.iter_batched(
            || {
                let conn = db::open(&path).unwrap();
                // Re-grow the table to ~110k rows with fresh commands
                // so each iteration deletes the same 10k again.
                let base = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                conn.execute_batch("BEGIN").unwrap();
                {
                    let mut stmt = conn
                        .prepare(
                            "INSERT INTO history (cwd, cmd, started_at, session, paths)
                             VALUES ('/x', ?1, ?2, 'bench', '')",
                        )
                        .unwrap();
                    for i in 0..10_000 {
                        stmt.execute(rusqlite::params![format!("fresh {i}"), base + i as i64])
                            .unwrap();
                    }
                }
                conn.execute_batch("COMMIT").unwrap();
                conn
            },
            |conn| {
                black_box(db::trim_history(&conn, 100_000).unwrap());
            },
            criterion::BatchSize::SmallInput,
        )
    });
    group.finish();
}
```

`run_all` の `bench_in_process(c, &dir);` の直後に追加:

```rust
    bench_trim(c, &dir);
```

- [ ] **Step 2: 検証** — Run: `cargo fmt && cargo clippy --all-targets && cargo test && cargo bench --bench suggest "trim_history"`
  Expected: 全て green。`cargo bench` のフィルタで `trim_history` グループのみ実行され、両シナリオが完走すること（実行時間 ~1 分以内）

- [ ] **Step 3: コミット** — `git add benches/suggest.rs && git commit -m "bench: add trim_history scenarios"`

---

### Task 4: ドキュメント

**Files:**
- Modify: `README.md`（英語）
- Modify: `docs/internal/specs/2026-08-15-seasalt-design.md`（日本語）

- [ ] **Step 1: README 更新** — 3 箇所:

Features の「Exit-code tracking」の後に bullet 追加:

```markdown
- **History size limit** — history is automatically trimmed to the
  newest 100,000 entries (configurable via `SEASALT_HISTORY_MAX`,
  `0` disables trimming). `seasalt clear` deletes everything and
  reclaims the file space.
```

Usage の `seasalt delete ID...` の後に追加:

```markdown
seasalt clear
    Delete all history entries and reclaim the file space (VACUUM).
    Prints nothing on success.
```

Configuration の `SEASALT_PRIVATE_MODE` の段落の後に追加:

```markdown
Set `SEASALT_HISTORY_MAX` to change the automatic history size limit
(default 100,000 entries; the oldest entries are dropped on record).
`0` disables trimming.
```

- [ ] **Step 2: 設計仕様更新** — `docs/internal/specs/2026-08-15-seasalt-design.md` の 2 箇所:

§3 ストレージの SQL ブロック（`CREATE INDEX idx_history_cwd_cmd ...` の後）に追加:

```sql
CREATE INDEX idx_history_started_at ON history(started_at);
```

§3 ストレージのパラグラフ末尾に追加:

```
履歴は件数上限 (デフォルト 100,000 件、`SEASALT_HISTORY_MAX` で変更、`0` で無効化) を持ち、record のたびに `started_at` が古い行から上限を超える分を自動削除する。dedup で更新された行は最新扱いになるため保護される。`idx_history_started_at` はこのトリムに使う。削除コストは実測済み (100k 行で warm ~0.2ms / cold ~2.1ms、設計ドキュメント 2026-08-16-history-limit-design.md §2 参照)。
```

§4 の `seasalt delete ID...` の後に追加:

```
- `seasalt clear`
  - 履歴を全件削除し、`VACUUM` でファイル領域を回収する (fish の `history clear` 相当)
  - 成功時は何も出力しない。interactive コマンド (エラーは stderr)
```

- [ ] **Step 3: 検証** — Run: `git diff --stat` で README と設計仕様のみ変更されていることを確認。テストは不要（ドキュメントのみ）

- [ ] **Step 4: コミット** — `git add README.md docs/internal/specs/2026-08-15-seasalt-design.md && git commit -m "docs: document history limit and clear command"`

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check && cargo clippy --all-targets && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo bench --bench suggest "trim_history"` が完走
- [ ] `git status` が clean、`git log --oneline -6` で 4 コミットを確認
