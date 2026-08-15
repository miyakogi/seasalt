# 履歴重複除去 (History Dedup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 同一 (cwd, cmd) のコマンド再実行時に新規行を作らず既存行を最新化する完全 dedup を実装する (fish の "Any duplicate history items are automatically removed" パリティ)

**Architecture:** `db::insert_history` を upsert 化 (`record_history` に改名)。同一 (cwd, cmd) の行があれば `started_at`/`paths`/`session` を更新し `exit_code` を NULL にリセットして同 id を返す。`(cwd, cmd)` 複合インデックスを追加。exit 更新は `session` 照合をやめ id のみに単純化 (dedup で行の session が書き換わるため)

**Tech Stack:** Rust (edition 2021) / rusqlite 0.37 (bundled) / clap 4

**Spec:** `docs/superpowers/specs/2026-08-15-seasalt-design.md` (Task 3 で本実装に合わせて更新)

## Global Constraints

- Rust edition 2021、依存クレート追加なし
- `record` / `exit` / `suggest` はフックから呼ばれるため、エラー時も stderr に出さず非 0 終了 (現状維持)
- コードコメントは既存スタイルに合わせ日本語
- コミットメッセージは英語、各コミット前にユーザー承認を得る (AGENTS.md)
- 各コミット前に `cargo fmt` → `cargo check` → `cargo clippy` → `cargo test` を実行
- 既存 DB に溜まっている重複行は放置する (新規 record からのみ dedup)

---

### Task 1: DB 層 — dedup 記録と id 照合 exit

**Files:**
- Modify: `src/db.rs` (SCHEMA, insert_history→record_history, update_exit_code)
- Modify: `src/main.rs:76-89` (record/exit アームの呼び出し)
- Modify: `tests/db_test.rs` (呼び出し更新 + 新テスト)
- Modify: `tests/suggest_test.rs` (改名追随)

**Interfaces:**
- Consumes: 既存 `paths::required_paths` (main.rs の呼び出しは不変)
- Produces: `db::record_history(conn, cwd, cmd, started_at, session, paths) -> Result<i64>` / `db::update_exit_code(conn, id, code) -> Result<()>` (Task 2 が CLI 経由で消費)

- [ ] **Step 1: テストを先に書く (red)**

`tests/db_test.rs` に 3 テストを追加し、既存テストを新シグネチャへ更新:

```rust
#[test]
fn record_dedups_same_command_and_bumps_to_latest() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();

    let id1 = db::record_history(&conn, "/tmp/a", "nvim a.txt", 1000, "s1", "a.txt").unwrap();
    let id2 = db::record_history(&conn, "/tmp/a", "nvim b.txt", 2000, "s1", "b.txt").unwrap();
    // 非連続でも同一 (cwd, cmd) は新規行を作らず、既存行が最新化される
    let id3 = db::record_history(&conn, "/tmp/a", "nvim a.txt", 3000, "s2", "").unwrap();

    assert_eq!(id1, id3);
    assert_ne!(id1, id2);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history WHERE cwd = '/tmp/a'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    let (started_at, paths, exit_code, session): (i64, String, Option<i64>, String) = conn
        .query_row(
            "SELECT started_at, paths, exit_code, session FROM history WHERE id = ?1",
            [id1],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(started_at, 3000);
    assert_eq!(paths, "");
    assert_eq!(exit_code, None);
    assert_eq!(session, "s2");
}

#[test]
fn record_does_not_dedup_across_directories() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let a = db::record_history(&conn, "/dir/a", "ls", 1000, "s", "").unwrap();
    let b = db::record_history(&conn, "/dir/b", "ls", 2000, "s", "").unwrap();
    assert_ne!(a, b);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn deduped_record_resets_exit_code_until_exit() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let id = db::record_history(&conn, "/tmp/a", "make", 1000, "s1", "").unwrap();
    db::update_exit_code(&conn, id, 0).unwrap();
    // 再実行で dedup されると exit_code は未確定に戻る
    db::record_history(&conn, "/tmp/a", "make", 2000, "s2", "").unwrap();
    let code: Option<i64> = conn
        .query_row("SELECT exit_code FROM history WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert_eq!(code, None);
    // session が書き換わっていても id だけで照合できる
    db::update_exit_code(&conn, id, 7).unwrap();
    let code: Option<i64> = conn
        .query_row("SELECT exit_code FROM history WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert_eq!(code, Some(7));
}
```

既存テストの更新 (すべて機械的):

- `insert_and_update_exit_code_roundtrip`: `db::insert_history` → `db::record_history`、`db::update_exit_code(&conn, "s1", id, 42)` → `db::update_exit_code(&conn, id, 42)`
- `insert_returns_increasing_ids` / `insert_and_read_paths_roundtrip` / `init_adds_paths_column_to_old_schema`: `insert_history` → `record_history`
- `update_exit_code_on_missing_row_is_ok`: `db::update_exit_code(&conn, "nope", 999, 0)` → `db::update_exit_code(&conn, 999, 0)`

`tests/suggest_test.rs` の `db::insert_history` を全て `db::record_history` へ (7 箇所: 16, 21-25, 93, 105, 161 行)。既存テストの seed は全て (cwd, cmd) が異なるため挙動は変わらない。

- [ ] **Step 2: 実行して失敗を確認**

Run: `cargo test --test db_test`
Expected: FAIL — `record_history` が存在しない (コンパイルエラー)

- [ ] **Step 3: 実装**

`src/db.rs`:

1. `use rusqlite::{Connection, OptionalExtension};` に変更
2. SCHEMA にインデックス追加:

```sql
CREATE INDEX IF NOT EXISTS idx_history_cwd_cmd ON history(cwd, cmd);
```

3. `insert_history` を `record_history` に改名し、本体を upsert 化:

```rust
/// 履歴を記録する。同一 (cwd, cmd) の既存行があれば新規行を作らず、
/// その行を最新 (started_at 更新・paths 置換・exit_code リセット) に
/// 書き換える (fish と同様、重複コマンドは履歴に 1 行しか残らない)。
/// 行 id を返す。
pub fn record_history(
    conn: &Connection,
    cwd: &str,
    cmd: &str,
    started_at: i64,
    session: &str,
    paths: &str,
) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM history WHERE cwd = ?1 AND cmd = ?2
             ORDER BY started_at DESC, id DESC LIMIT 1",
            rusqlite::params![cwd, cmd],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE history SET started_at = ?1, session = ?2, paths = ?3, exit_code = NULL WHERE id = ?4",
                rusqlite::params![started_at, session, paths, id],
            )?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO history (cwd, cmd, started_at, session, paths) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![cwd, cmd, started_at, session, paths],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }
}
```

4. `update_exit_code` を id 照合に変更:

```rust
/// 行 id で exit_code を更新する (dedup で行が他セッションの実行に
/// 書き換わっても正しく追従できるよう、session は照合に使わない)
pub fn update_exit_code(conn: &Connection, id: i64, code: i64) -> Result<()> {
    conn.execute(
        "UPDATE history SET exit_code = ?1 WHERE id = ?2",
        rusqlite::params![code, id],
    )?;
    Ok(())
}
```

`src/main.rs`:
- record アーム (80 行目): `seasalt::db::insert_history` → `seasalt::db::record_history`
- exit アーム (88 行目): `seasalt::db::update_exit_code(&conn, &session, last_id, code)` → `seasalt::db::update_exit_code(&conn, last_id, code)` (CLI の `--session` 引数はこの時点では残したままにする — Task 2 で除去)

- [ ] **Step 4: テスト実行で緑を確認**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 5: lint / fmt / 検証**

Run: `cargo fmt && cargo check && cargo clippy && cargo test`
Expected: clippy 警告 0、全テスト PASS

- [ ] **Step 6: コミット (ユーザー承認後に)**

```bash
git add src/db.rs src/main.rs tests/db_test.rs tests/suggest_test.rs
git commit -m "feat: deduplicate identical history entries per directory"
```

---

### Task 2: CLI exit から --session を除去 + 統合スニペット更新 + smoke 検証

**Files:**
- Modify: `src/main.rs:29-36` (Exit コマンド定義), `src/main.rs:83-89` (exit アーム)
- Modify: `src/bash/seasalt.bash:16-23` (`_seasalt_precmd`)
- Modify: `tests/cli_test.rs` (exit 呼び出し更新 + 新テスト)
- Modify: `tests/bash/smoke.sh` (dedup 検証追加)

**Interfaces:**
- Consumes: Task 1 の `db::record_history` / `db::update_exit_code(conn, id, code)` (CLI 経由)
- Produces: CLI サブコマンド `seasalt exit --last-id ID --code CODE` (session 引数なし)

- [ ] **Step 1: テストを先に書く (red)**

`tests/cli_test.rs`:
- `record_then_exit_then_search` の exit 呼び出しを更新:
```rust
        .args(["exit", "--last-id", "1", "--code", "0"])
```
(現状は `--session s1` あり → この時点では新 CLI が未実装なので失敗する)

- 新テスト追加:
```rust
#[test]
fn record_dedups_identical_command() {
    let dir = temp_data_dir();
    let first = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "echo hello"])
        .output()
        .unwrap();
    let second = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s2", "--", "echo hello"])
        .output()
        .unwrap();
    let id1 = String::from_utf8(first.stdout).unwrap();
    let id2 = String::from_utf8(second.stdout).unwrap();
    assert_eq!(id1.trim(), id2.trim());

    // 履歴には 1 行だけ残る
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--tsv", "hello"])
        .output()
        .unwrap();
    assert_eq!(out.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
}
```

`tests/bash/smoke.sh` — 検索アサーション (68-69 行) の直後に追加:

```bash
  # 同一コマンドの再実行は新規行を作らず既存行を最新化する (dedup)
  _seasalt_preexec "echo hello world"
  _seasalt_precmd
  rows=$("$BIN" search --tsv echo | wc -l)
  [[ $rows -eq 1 ]] || fail "duplicate history rows after re-run: $rows"
```

- [ ] **Step 2: 実行して失敗を確認**

Run: `cargo test --test cli_test`
Expected: FAIL — `exit` に `--session` 必須なのに省略したため (dedup 新テストは Task 1 実装により既に通る)

- [ ] **Step 3: 実装**

`src/main.rs` Exit 定義:
```rust
    /// Update the exit code of a recorded command (precmd hook)
    Exit {
        #[arg(long)]
        last_id: i64,
        #[arg(long)]
        code: i64,
    },
```
exit アーム:
```rust
        Command::Exit { last_id, code } => {
            seasalt::db::update_exit_code(&conn, last_id, code)?;
        }
```

`src/bash/seasalt.bash` の `_seasalt_precmd`:
```bash
  function _seasalt_precmd {
    local code;
    code=$?;
    if [[ "$_seasalt_last_id" ]]; then
      "$_seasalt_bin" exit --last-id "$_seasalt_last_id" --code "$code" >/dev/null 2>&1;
      _seasalt_last_id=;
    fi;
  };
```
(`_seasalt_session` 変数と record への `--session` は残す — セッション列は将来用に維持)

- [ ] **Step 4: テスト実行で緑を確認**

Run: `cargo test`
Expected: 全テスト PASS

- [ ] **Step 5: smoke 検証**

Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
Expected: `OK`

- [ ] **Step 6: コミット (ユーザー承認後に)**

```bash
git add src/main.rs src/bash/seasalt.bash tests/cli_test.rs tests/bash/smoke.sh
git commit -m "feat: drop session matching from exit flow"
```

---

### Task 3: ドキュメント更新 (spec + README)

**Files:**
- Modify: `docs/superpowers/specs/2026-08-15-seasalt-design.md`
- Modify: `README.md`

- [ ] **Step 1: spec を実装に合わせて更新**

1. §3 スキーマブロックに追加:
```sql
CREATE INDEX idx_history_cwd_cmd ON history(cwd, cmd);
```
追記: 「`(cwd, cmd)` の複合インデックスは record 時の重複判定に使う」

2. §4 `record` の説明を差し替え:
```
- `seasalt record --cwd DIR -- CMD`
  - preexec フックから呼ばれ履歴を記録する (実行前時点のエントリ作成)
  - 同一 (cwd, cmd) の既存行があれば新規行を作らず、その行を最新化する (started_at 更新・paths 置換・exit_code リセット)。fish と同様、重複コマンドは履歴に 1 行しか残らない
  - 引数のうち記録時点で実在したファイルパスのみを `paths` に保存する (存在しなかった引数は制約にならない)
  - 行 id を stdout に出力し、bash 側の変数に保持
```

3. §4 `exit` の説明を差し替え:
```
- `seasalt exit --last-id N --code CODE`
  - precmd フックから呼ばれ、行 id で特定したエントリに exit_code を update する (session は照合に使わない: dedup で行が他セッションの実行に書き換わり得るため)
```

4. §6 に小節を追加:
```
### 履歴の重複除去 (fish パリティ)

- record 時に同一 (cwd, cmd) の既存行があれば、新規 insert せずに行を最新化する (started_at 更新・paths 置換・exit_code リセット)
- 同一コマンドは連続・非連続を問わず 1 行しか残らない (fish の "Any duplicate history items are automatically removed" に相当。fish はコマンド文字列のみで判定するが、seasalt はディレクトリ別スコープが本体のため (cwd, cmd) をキーにする)
- 既に溜まっている旧データの重複行は放置する (新規 record からのみ dedup が効く)
- トレードオフ: 中間実行の時刻・exit code は残らない (最後の実行分のみ)
```

- [ ] **Step 2: README を更新**

1. Features の Stale-file filtering の後に追加:
```markdown
- **Duplicate suppression** — re-running the same command in the
  same directory refreshes its existing entry (moving it to the top)
  instead of adding another copy, like fish. Duplicate rows created
  by older versions are left as-is.
```
2. Usage の CLI 記述を更新:
```diff
- seasalt exit --session SESS --last-id ID --code CODE
+ seasalt exit --last-id ID --code CODE
     Store the exit code of a recorded command.
```
3. `seasalt record` の説明に 1 文追加:
```
     Record a command into history. Prints the row id. Re-running the
     same command in the same directory updates the existing entry.
```

- [ ] **Step 3: コミット (ユーザー承認後に)**

```bash
git add docs/superpowers/specs/2026-08-15-seasalt-design.md README.md
git commit -m "docs: document history deduplication"
```

---

## 最終検証 (全タスク後)

```bash
cargo fmt && cargo check && cargo clippy && cargo test
cargo build --release && bash tests/bash/smoke.sh target/release/seasalt
cargo install --path .
```

期待: clippy 警告 0、全テスト PASS、smoke `OK`。必要なら pty で「同一ディレクトリで同じコマンドを 2 回実行 → `seasalt search` で 1 行」を手動確認。
