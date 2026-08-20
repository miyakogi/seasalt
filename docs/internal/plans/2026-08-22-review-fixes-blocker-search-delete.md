# レビュー軽微修正（blocker 100ms / delete 900件分割 / search 正規化） Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 前回レビューで指摘された軽微 3 点（busy_timeout テストの境界値、delete の SQLite 変数上限、search の cwd 正規化漏れ）を局所修正し、CHANGELOG に記録する。

**Architecture:** `tests/db_test.rs` の blocker sleep を 150→100ms に、`src/db.rs` の `delete_by_ids` を 900 件ずつチャンクに、`src/search.rs` と `src/main.rs` の search 経路の cwd を `trim_end_matches('/')` で正規化する。DB スキーマ変更なし。各タスクは独立してテスト・コミット可能。

**Tech Stack:** Rust 2021 / rusqlite 0.37 bundled / clap 4

**Spec:** レビューコメント 2026-08-22 と本 plan が権威。既存設計は `docs/internal/specs/2026-08-15-seasalt-design.md` §4（record/search/suggest の cwd 正規化方針）を参照。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` をこの順で実行し、全て green（AGENTS.md の pre-commit gate）
- コメントは英語、テストも英語。plan は日本語
- Silence contract 維持: `record`/`exit`/`suggest` は stderr に書かない
- `init` は DB/data dir に触れない
- スコープ外: `src/db.rs` の TOCTOU 権限窓（対応しない）、リリースタグ打ち・バージョン bump（別途待機）
- コミットは各タスクで事前承認

---

### Task 1: blocker sleep 150ms → 100ms で flaky 余裕を確保

**Files:**
- Modify: `tests/db_test.rs:326` — `Duration::from_millis(150)` → `100`
- Modify: `CHANGELOG.md:5` — Unreleased に 1 行追記（任意だが一括で）

**Interfaces:**
- Consumes: `std::sync::mpsc::channel` / `rusqlite::Connection::busy_timeout(150ms)`（`src/db.rs:68`）
- Produces: `writers_wait_for_busy_database` が 150ms timeout に対して 100ms ロックで 50ms 余裕を持つ

- [ ] **Step 1: 現状の境界値を確認**

  現行 `tests/db_test.rs:326`:

  ```rust
  std::thread::sleep(std::time::Duration::from_millis(150));
  ```

  `src/db.rs:68` の `busy_timeout` も 150ms で同値。負荷時に `record_history` が `SQLITE_BUSY` で失敗し得る。

- [ ] **Step 2: 実装（`tests/db_test.rs:326`）**

  ```rust
  std::thread::sleep(std::time::Duration::from_millis(100));
  ```

  変更はこの 1 行のみ。`busy_timeout` 150ms に対して 50ms の余裕が生まれ、CI の揺れでも安定する。

- [ ] **Step 3: テストを 3 回連続で PASS を確認**

  Run: `for i in 1 2 3; do cargo test --test db_test writers_wait_for_busy_database -- --nocapture 2>&1 | tail -n 5; done`
  Expected: 3 回とも `ok`

- [ ] **Step 4: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

- [ ] **Step 5: コミット（要承認）**

  ```bash
  git add tests/db_test.rs
  git commit -m "test(db): lower blocker sleep to 100ms for busy_timeout margin"
  ```

  CHANGELOG は軽微なテスト調整のため追記しない（Task 2,3 のコミットで Unreleased にまとめる）。

---

### Task 2: delete_by_ids を 900 件ずつに分割（SQLite 変数上限対策）

**Files:**
- Modify: `src/db.rs:195-206` — チャンク化
- Modify: `tests/db_test.rs:679` — 1200 件での上限超えテストを追加
- Modify: `CHANGELOG.md:5` — Unreleased に 1 行追記

**Interfaces:**
- Consumes: `rusqlite::params_from_iter`
- Produces: `delete_by_ids(conn, ids) -> Result<()>` が 1000 件超でも `SQLITE_ERROR` にならない

- [ ] **Step 1: 失敗テストを書く**

  `tests/db_test.rs:679` の `delete_by_ids_large_batch` の後に追加:

  ```rust
  #[test]
  fn delete_by_ids_over_sqlite_limit() {
      let conn = Connection::open_in_memory().unwrap();
      db::init(&conn).unwrap();
      let mut ids = Vec::new();
      for i in 0..1200 {
          let id = db::record_history(&conn, "/a", &format!("cmd {i}"), i as i64, "s", "", "bash").unwrap();
          ids.push(id);
      }
      // 1200 > 999 (default SQLite max variables) — must not fail
      db::delete_by_ids(&conn, &ids).unwrap();
      let count: i64 = conn.query_row("SELECT count(*) FROM history", [], |r| r.get(0)).unwrap();
      assert_eq!(count, 0);
  }
  ```

  現行の `IN (?, ?, ... 1200個)` は `SQLITE_ERROR: too many SQL variables` で FAIL する。

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test db_test delete_by_ids_over_sqlite_limit -- --nocapture`
  Expected: FAIL with `too many SQL variables`

- [ ] **Step 3: 実装（`src/db.rs:195-206`）**

  現行:

  ```rust
  pub fn delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()> {
      if ids.is_empty() {
          return Ok(());
      }
      let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
      let sql = format!("DELETE FROM history WHERE id IN ({placeholders})");
      let mut stmt = conn.prepare(&sql)?;
      stmt.execute(rusqlite::params_from_iter(ids))?;
      Ok(())
  }
  ```

  置換後（900 件ずつ）：

  ```rust
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
  ```

  900 は SQLite のデフォルト 999 に対して余裕を持った値。`chunk` ごとに `prepare` するが、`delete` は頻度が低く、単一 `DELETE` よりも `for chunk` のオーバーヘッドは無視できる。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test db_test delete_by_ids -- --nocapture`
  Expected: 4 件 PASS（既存 3 + 新規 1）

- [ ] **Step 5: CHANGELOG 追記**

  `## [Unreleased]` に:

  ```md
  - Harden `delete` for 1000+ ids by chunking SQLite variables
  ```

  ただし Task 1 はテストのみで CHANGELOG 不要のため、この Task で 1 行にまとめる。

- [ ] **Step 6: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

- [ ] **Step 7: コミット（要承認）**

  ```bash
  git add src/db.rs tests/db_test.rs CHANGELOG.md
  git commit -m "fix(db): chunk delete_by_ids to avoid SQLite variable limit"
  ```

---

### Task 3: search の cwd 正規化（record/suggest と統一）

**Files:**
- Modify: `src/search.rs:10-19` — `default_cwd()` の返り値を正規化
- Modify: `src/main.rs:142-158` — `Search` の `cwd` と `cwd_filter` を正規化
- Modify: `tests/cli_test.rs` — 新規テスト `search_with_trailing_slash_cwd` を追加
- Modify: `CHANGELOG.md` — Unreleased に 1 行追記

**Interfaces:**
- Consumes: `std::env::var("PWD")` / `std::env::current_dir()` / `clap` の `Search.cw`
- Produces: `search` が `/proj/sub/` でも `/proj/sub` と同一視してヒットする

- [ ] **Step 1: 失敗テストを書く**

  `tests/cli_test.rs` 末尾に追加:

  ```rust
  #[test]
  fn search_with_trailing_slash_cwd() {
      let dir = temp_data_dir();
      bin()
          .env("SEASALT_DATA_DIR", &dir)
          .args(["record", "--cwd", "/proj/sub", "--session", "s1", "--", "echo hello"])
          .status()
          .unwrap();
      // Search with trailing slash should still find it
      let out = bin()
          .env("SEASALT_DATA_DIR", &dir)
          .args(["search", "--cwd", "/proj/sub/", "hello"])
          .output()
          .unwrap();
      assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\techo hello");
      // --cwd via PWD with trailing slash (default_cwd path)
      let out = bin()
          .env("SEASALT_DATA_DIR", &dir)
          .env("PWD", "/proj/sub/")
          .args(["search", "hello"])
          .output()
          .unwrap();
      assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\techo hello");
  }
  ```

  現行は `record` 側が `cwd_norm` に正規化されるため DB には `/proj/sub` で保存されるが、`search` 側が `/proj/sub/` のまま `WHERE cwd = '/proj/sub/'` で検索し、0 件で FAIL する。

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test cli_test search_with_trailing_slash_cwd -- --nocapture`
  Expected: FAIL with `assertion failed: left: "" right: "1\techo hello"`

- [ ] **Step 3: 実装（`src/search.rs:10-19`）**

  現行 `default_cwd()`:

  ```rust
  pub fn default_cwd() -> Option<String> {
      std::env::var("PWD")
          .ok()
          .filter(|p| !p.is_empty() && p.starts_with('/'))
          .or_else(|| {
              std::env::current_dir()
                  .ok()
                  .map(|p| p.to_string_lossy().into_owned())
          })
  }
  ```

  置換後（末尾 `/` を正規化、ただし `/` は維持）：

  ```rust
  pub fn default_cwd() -> Option<String> {
      let normalize = |s: String| {
          let t = s.trim_end_matches('/').to_string();
          if t.is_empty() { "/".to_string() } else { t }
      };
      std::env::var("PWD")
          .ok()
          .filter(|p| !p.is_empty() && p.starts_with('/'))
          .map(normalize)
          .or_else(|| {
              std::env::current_dir()
                  .ok()
                  .map(|p| p.to_string_lossy().into_owned())
                  .map(normalize)
          })
  }
  ```

  `src/main.rs:142-158` の `Search` 分岐でも `cwd` を正規化:

  ```rust
  let cwd_filter = if all {
      None
  } else {
      match cwd {
          Some(dir) => {
              let t = dir.trim_end_matches('/');
              Some(if t.is_empty() { "/".to_string() } else { t.to_string() })
          }
          None => seasalt::search::default_cwd(),
      }
  };
  // 既存の match cwd { Some(dir) => Some(dir), None => default_cwd() } を上記に置換
  ```

  `dir` は `String` なので `trim_end_matches` 後に `to_string`。`"/"` のケースは `trim` で空になるため `"/"` に戻す。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test cli_test search_with_trailing_slash_cwd -- --nocapture`
  Expected: PASS

  Run: `cargo test --test cli_test 2>&1 | grep "test result"`
  Expected: 既存 25 + 新規 1 で 26 件 PASS

- [ ] **Step 5: CHANGELOG 追記**

  `## [Unreleased]` に:

  ```md
  - Normalize trailing slashes in `search` so `--cwd /proj/sub/` matches `/proj/sub`
  ```

  Task 2 で既に 1 行追加しているため、追記は 2 行目として積む。

- [ ] **Step 6: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

  Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt && bash tests/zsh/smoke.sh target/release/seasalt`
  Expected: 両方 `OK`（`search` 変更は `cli_test` と smoke の `search --tsv` 節で検証）

- [ ] **Step 7: コミット（要承認）**

  ```bash
  git add src/search.rs src/main.rs tests/cli_test.rs CHANGELOG.md
  git commit -m "fix(search): normalize trailing slashes in cwd"
  ```

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` が green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt` が `OK`（または `SKIP`）
- [ ] `git log --oneline -4` で 3 コミット（Task 1〜3）が積まれていること
- [ ] `git diff  b2b3f30..HEAD --stat` が `tests/db_test.rs`, `src/db.rs`, `src/search.rs`, `src/main.rs`, `tests/cli_test.rs`, `CHANGELOG.md` のみに収まっていること
- [ ] `CHANGELOG.md` の `## [Unreleased]` に Task 2,3 の 2 行が追記されていること（Task 1 はテストのみで追記なし）

## Self-Review

- [x] Spec coverage: blocker 余裕（Task 1）、delete 上限（Task 2）、search 正規化（Task 3）全てに task がある。TOCTOU とリリースはスコープ外として明記
- [x] Placeholder scan: `TBD`/`TODO` なし。全 step に具体的なコードとコマンドを記載
- [x] Type consistency: `delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()>` / `default_cwd() -> Option<String>` / `Search.cw: Option<String>` のシグネチャは不変。チャンク化は `ids.chunks(900)` で型一致
