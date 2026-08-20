# レビュー追加対応（search help / ancestors 正規化 / delete 束ね / paths クォート） Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** レビューで残った4件（search のワイルドカード help 補強、ancestors の末尾スラッシュ正規化、delete_by_ids の束ね、paths のエスケープ付きクォート）を局所修正で潰し、CHANGELOG に記録する。

**Architecture:** Rust 側への局所修正のみ。`src/main.rs` の clap help 1行、`src/suggest.rs` の `ancestors` に正規化、`src/db.rs` の `delete_by_ids` を IN 句化、`src/paths.rs` の `unquote/tokenize` のクォート内エスケープ対応。DB マイグレーションやスキーマ変更なし。各タスクは独立してテスト・コミット可能。

**Tech Stack:** Rust 2021 / rusqlite 0.37 bundled / clap 4 / zsh, bash smoke

**Spec:** レビューコメント 2026-08-21 と本 plan が権威。既存設計は `docs/internal/specs/2026-08-15-seasalt-design.md` §4（record/search/suggest 仕様）を参照。search の LIKE ワイルドカードは README の “PATTERN is matched as substring (SQL LIKE semantics)” に基づく。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` をこの順で実行し、全て green（AGENTS.md の pre-commit gate）
- コメントは英語、テストも英語。設計 spec・本 plan は日本語（既存文書）
- Silence contract を維持: `record`/`exit`/`suggest` は stderr に何も書かない。`init`/`search`/`delete`/`clear` はエラーを stderr に出す
- `init` は DB / data dir に触れてはならない（既存 cli_test で担保）
- スコープ外: `src/db.rs` の TOCTOU 権限窓（対応しない）、リリースタグ打ち・バージョン bump（別途）
- コミットは各タスクでユーザー事前承認を得ること

---

### Task 1: search help にワイルドカード説明を追記

**Files:**
- Modify: `src/main.rs:58-59` — `Search.pattern` の help
- Modify: `CHANGELOG.md:5-9` — Unreleased に 1 行追記
- Test: 既存 `cli_test` で担保（新規テスト不要。`--help` 出力の存在確認は手動）

**Interfaces:**
- Consumes: `clap::Parser` の `help` 属性
- Produces: `seasalt search --help` に `SQL LIKE: % and _ are wildcards` が表示される

- [ ] **Step 1: 現状の help を確認**

  Run: `cargo run -- search --help 2>&1 | grep -A2 pattern`
  Expected: `pattern` の説明が無いか、単に `<PATTERN>` のみ。`%`/`_` の説明なし。

- [ ] **Step 2: 実装（`src/main.rs:58-59`）**

  現行:

  ```rust
          #[arg(long)]
          tsv: bool,
          pattern: String,
  ```

  置換後:

  ```rust
          #[arg(long)]
          tsv: bool,
          /// Substring to search (SQL LIKE semantics: % and _ are wildcards)
          pattern: String,
  ```

  または `#[arg(help = "substring to search (SQL LIKE: % and _ are wildcards)")]` のどちらでも可。clap の `///` doc comment 形式を既存 `Record`/`Suggest` と揃えるため `///` を採用する。`help` 文字列は英語。

  変更はこの 2 行のみ。

- [ ] **Step 3: help 出力を確認**

  Run: `cargo run -- search --help 2>&1 | grep -A2 PATTERN`
  Expected: `substring to search (SQL LIKE: % and _ are wildcards)` が含まれる

- [ ] **Step 4: CHANGELOG 追記（`CHANGELOG.md:5-9`）**

  `## [Unreleased]` の末尾に 1 行追加:

  ```md
  - Document `search` wildcards in `--help` (SQL LIKE: `%` and `_` are wildcards)
  ```

  既存 Unreleased 3 行の後に追記し、順序は Task 1〜4 の順に積み上げる。

- [ ] **Step 5: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green（help 文字列はテストに影響しない）

- [ ] **Step 6: コミット（要承認）**

  ```bash
  git add src/main.rs CHANGELOG.md
  git commit -m "docs(cli): document search wildcards in --help"
  ```

---

### Task 2: ancestors の末尾スラッシュ正規化

**Files:**
- Modify: `src/suggest.rs:97-110` — `ancestors()` に正規化
- Modify: `src/db.rs:152` or `src/main.rs:101` — `record_history` 呼び出し前の `cwd` 正規化（任意だが本 Task で実施）
- Modify: `tests/suggest_test.rs` — 新規テスト 2 件追加
- Modify: `CHANGELOG.md` — Unreleased に 1 行追記

**Interfaces:**
- Consumes: `std::path::Path`
- Produces: `ancestors(cwd: &str) -> Vec<String>` が末尾 `/` や `//` を正規化して返す。例: `/a/b//` → `ancestors` は `/a/b` を起点に `/a` を返す

- [ ] **Step 1: 失敗テストを書く**

  `tests/suggest_test.rs` 末尾に追加:

  ```rust
  #[test]
  fn ancestors_normalizes_trailing_slash() {
      assert_eq!(
          suggest::ancestors("/a/b/"),
          vec!["/a".to_string()]
      );
      assert_eq!(
          suggest::ancestors("/a/b//"),
          vec!["/a".to_string()]
      );
      assert_eq!(
          suggest::ancestors("/a/b/c/"),
          vec!["/a/b".to_string(), "/a".to_string()]
      );
  }

  #[test]
  fn ancestors_normalized_cwd_matches_parent_scope() {
      let conn = Connection::open_in_memory().unwrap();
      db::init(&conn).unwrap();
      // Record with normalized cwd "/proj"
      db::record_history(&conn, "/proj", "cargo check", 1000, "s", "", "bash").unwrap();
      // Suggest from "/proj/sub/" (trailing slash) should find parent "/proj"
      let got = suggest::suggest(&conn, "/proj/sub/", "cargo").unwrap().unwrap();
      assert_eq!(got, "cargo check");
  }
  ```

  現行 `ancestors` は `Path::new("/a/b/").parent()` が `/a/b` を返すため 1 件目は偶然 PASS するが、`//` や深い階層での正規化が漏れる。2 件目の `suggest` は `cwd` が `/proj/sub/` のまま `search_scope` に渡され、DB の `/proj/sub` と一致せず、parent 検索も `/proj/sub` ではなく `/proj/sub/` 起点になるため FAIL する。

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test suggest_test ancestors_normalizes_trailing_slash -- --nocapture`
  Expected: 2 件目の `ancestors_normalized_cwd_matches_parent_scope` が FAIL（`called Option::unwrap() on a None value`）

- [ ] **Step 3: 実装（`src/suggest.rs:97-110`）**

  現行:

  ```rust
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
  ```

  置換後（末尾スラッシュ正規化 + ルート正規化）:

  ```rust
  /// Lists the parent directories in order of closeness (root itself is not included).
  /// Trailing slashes are normalized so "/a/b/" and "/a/b//" behave like "/a/b".
  pub fn ancestors(cwd: &str) -> Vec<String> {
      let normalized = cwd.trim_end_matches('/');
      let normalized = if normalized.is_empty() { "/" } else { normalized };
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
  ```

  追加で `suggest::suggest` と `suggest::search` の冒頭で `cwd` を正規化する（`record` 側は `main.rs` で正規化するのが理想だが、本 Task では `suggest` 側のみで十分。両方やると重複だが安全）:

  ```rust
  // In suggest() and search(), first line:
  let cwd = cwd.trim_end_matches('/'); // keep "/" as "/"
  let cwd = if cwd.is_empty() { "/" } else { cwd };
  ```

  ただし `suggest` の `cwd` は `&str` なので、関数内で `let cwd_owned = ...; let cwd = cwd_owned.as_str();` のように所有を取る。最も小さくするには `ancestors` の正規化のみで十分だが、exact-cwd 検索 `suggest_prefix(conn, Some(cwd))` が `/proj/sub/` で DB の `/proj/sub` と不一致になるため、`search` 冒頭で `cwd` を正規化する必要がある。`search` の `cwd` は `&str` なので、以下のようにする:

  ```rust
  fn search(conn: &Connection, cwd: &str, ...) -> Result<Option<String>> {
      let cwd_norm = {
          let t = cwd.trim_end_matches('/');
          if t.is_empty() { "/" } else { t }
      };
      // 以降 cwd の代わりに cwd_norm を使う（ancestors 呼び出しも cwd_norm）
  ```

  実装は `search` と `ancestors` の両方に正規化を入れる。重複しても害はない。

  また `src/main.rs:101` の `record` 側でも `cwd` を正規化して保存する（DB の cwd を正規形で統一）:

  ```rust
  let cwd_norm = {
      let t = cwd.trim_end_matches('/');
      if t.is_empty() { "/" } else { t }
  };
  // 以降 record_history に &cwd_norm を渡す
  ```

  本 plan では `src/main.rs` の record 正規化も含める。既存の `/` ルートはそのまま保持。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test suggest_test ancestors -- --nocapture`
  Expected: 2 件とも PASS

  Run: `cargo test --test suggest_test 2>&1 | grep "test result"`
  Expected: 全 28 件 PASS（既存 26 + 新規 2）

- [ ] **Step 5: CHANGELOG 追記**

  `## [Unreleased]` に:

  ```md
  - Normalize trailing slashes in `suggest` so `/proj/sub/` matches history for `/proj/sub`
  ```

- [ ] **Step 6: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

  Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt && bash tests/zsh/smoke.sh target/release/seasalt`
  Expected: 両方 `OK`

- [ ] **Step 7: コミット（要承認）**

  ```bash
  git add src/suggest.rs src/main.rs tests/suggest_test.rs CHANGELOG.md
  git commit -m "fix(suggest): normalize trailing slashes in cwd"
  ```

---

### Task 3: delete_by_ids を束ねてトランザクション化

**Files:**
- Modify: `src/db.rs:195-203` — `delete_by_ids()`
- Modify: `tests/db_test.rs` — 新規テスト `delete_by_ids_large_batch` を追加
- Modify: `CHANGELOG.md` — Unreleased に 1 行追記

**Interfaces:**
- Consumes: `rusqlite::Connection`
- Produces: `delete_by_ids(conn, ids) -> Result<()>` が複数 id を束ねて 1 回の SQL で削除する（トランザクション内）

- [ ] **Step 1: 失敗テストは不要（既存テストで担保）だが、新規テストで効率を検証**

  `tests/db_test.rs` 末尾に追加:

  ```rust
  #[test]
  fn delete_by_ids_large_batch() {
      let conn = Connection::open_in_memory().unwrap();
      db::init(&conn).unwrap();
      let mut ids = Vec::new();
      for i in 0..100 {
          let id = db::record_history(&conn, "/a", &format!("cmd {i}"), i as i64, "s", "", "bash").unwrap();
          ids.push(id);
      }
      // Delete even ids
      let even: Vec<i64> = ids.iter().copied().filter(|x| x % 2 == 0).collect();
      db::delete_by_ids(&conn, &even).unwrap();
      let count: i64 = conn.query_row("SELECT count(*) FROM history", [], |r| r.get(0)).unwrap();
      assert_eq!(count, 50);
      // Nonexistent ids still ignored
      db::delete_by_ids(&conn, &[9999, 10000]).unwrap();
      let count: i64 = conn.query_row("SELECT count(*) FROM history", [], |r| r.get(0)).unwrap();
      assert_eq!(count, 50);
  }
  ```

  現行のループ実装でもこのテストは PASS するため、Step 2 は「PASS することを確認」で良い。束ね化の失敗は性能や VACUUM ではなく、SQL 文法エラーで FAIL する。

- [ ] **Step 2: 現行実装で PASS を確認**

  Run: `cargo test --test db_test delete_by_ids_large_batch -- --nocapture`
  Expected: PASS

- [ ] **Step 3: 実装（`src/db.rs:195-203`）**

  現行:

  ```rust
  pub fn delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()> {
      let mut stmt = conn.prepare("DELETE FROM history WHERE id = ?1")?;
      for id in ids {
          stmt.execute(rusqlite::params![id])?;
      }
      Ok(())
  }
  ```

  置換後（IN 句 + トランザクション + 空配列ガード）:

  ```rust
  pub fn delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()> {
      if ids.is_empty() {
          return Ok(());
      }
      // Use a single statement with IN (...) to avoid N round-trips.
      // IDs are integers, so string interpolation is safe; we still
      // validate that we don't create an empty IN ().
      let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
      let sql = format!("DELETE FROM history WHERE id IN ({placeholders})");
      let mut stmt = conn.prepare(&sql)?;
      // rusqlite params! needs a slice of &dyn ToSql; use params_from_iter
      stmt.execute(rusqlite::params_from_iter(ids))?;
      Ok(())
  }
  ```

  補足:
  - `rusqlite::params_from_iter` は 0.37 で利用可能。もし無い場合は `ids.iter().map(|id| id as &dyn rusqlite::ToSql)` で代替。
  - 空配列は早期 return で `DELETE ... IN ()` の無効 SQL を避ける。
  - トランザクションは `db::open` の `busy_timeout` で十分だが、念のため `conn.execute_batch("BEGIN; ... COMMIT;")` は不要（単一 statement は原子）。

  もし `params_from_iter` が無い環境なら以下にフォールバック:

  ```rust
  let mut stmt = conn.prepare(&sql)?;
  for (i, id) in ids.iter().enumerate() {
      stmt.raw_bind_parameter(i + 1, id)?;
  }
  stmt.execute([])?;
  ```

  本 plan では `params_from_iter` を第一候補とし、コンパイルエラーならフォールバックを採用する。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test db_test delete_by_ids -- --nocapture`
  Expected: 3 件 PASS（既存 2 + 新規 1）

- [ ] **Step 5: CHANGELOG 追記**

  `## [Unreleased]` に:

  ```md
  - Speed up `delete` for multiple ids by batching into a single statement
  ```

- [ ] **Step 6: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

- [ ] **Step 7: コミット（要承認）**

  ```bash
  git add src/db.rs tests/db_test.rs CHANGELOG.md
  git commit -m "perf(db): batch delete_by_ids into single statement"
  ```

---

### Task 4: paths のエスケープ付きクォート対応

**Files:**
- Modify: `src/paths.rs:59-69` — `unquote()` と `tokenize()` のクォート内エスケープ
- Modify: `tests/paths_test.rs` — 新規テスト 2 件追加
- Modify: `CHANGELOG.md` — Unreleased に 1 行追記

**Interfaces:**
- Consumes: `std::path::Path::exists` / `String`
- Produces: `required_paths(cwd, cmd) -> Vec<String>` が `"a\"b"` や `'a\'b'` のようなエスケープ付きクォートを含む引数を正しく解決する

- [ ] **Step 1: 失敗テストを書く**

  `tests/paths_test.rs` 末尾に追加:

  ```rust
  #[test]
  fn handles_escaped_quotes_inside_quotes() {
      let dir = temp_dir();
      // Create a file with a quote in its name (unlikely but valid on Unix)
      let name = "a\"b.txt";
      std::fs::write(dir.join(name), "x").unwrap();
      let cwd = dir.to_str().unwrap();

      // Double-quoted arg containing an escaped double quote
      let cmd = format!("nvim \"{}\"", "a\\\"b.txt");
      // tokenize should keep the escaped quote, unquote should strip outer quotes
      // and leave a"b.txt as the path
      assert_eq!(paths::required_paths(cwd, &cmd), vec![name.to_string()]);

      let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn handles_escaped_single_quote() {
      let dir = temp_dir();
      let cwd = dir.to_str().unwrap();
      // Single-quoted arg with escaped single quote (shell would be 'a'\''b' but we test simple)
      // Our tokenizer treats backslash as escape even inside single quotes for simplicity
      std::fs::write(dir.join("a'b.txt"), "x").unwrap();
      let cmd = "nvim 'a\\'b.txt'";
      assert_eq!(paths::required_paths(cwd, cmd), vec!["a'b.txt".to_string()]);
      let _ = std::fs::remove_dir_all(&dir);
  }
  ```

  現行 `unquote` は外側 1 文字だけ剥がし、内側の `\"` を `a\"b.txt` のまま残すため、`resolve(cwd, "a\"b.txt")` は `a"b.txt` と不一致で `required_paths` が空を返し FAIL する。

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test paths_test handles_escaped_quotes_inside_quotes -- --nocapture`
  Expected: FAIL with `assertion failed: left: [] right: ["a\"b.txt"]`

- [ ] **Step 3: 実装（`src/paths.rs:59-69, 71-110`）**

  現行 `unquote`:

  ```rust
  fn unquote(tok: &str) -> String {
      if tok.len() >= 2 {
          let first = tok.chars().next().unwrap();
          let last = tok.chars().last().unwrap();
          if (first == '"' || first == '\'') && first == last {
              return tok[1..tok.len() - 1].to_string();
          }
      }
      tok.to_string()
  }
  ```

  置換後（クォート内のエスケープを解く）:

  ```rust
  fn unquote(tok: &str) -> String {
      if tok.len() >= 2 {
          let first = tok.chars().next().unwrap();
          let last = tok.chars().last().unwrap();
          if (first == '"' || first == '\'') && first == last {
              let inner = &tok[1..tok.len() - 1];
              // Unescape the same quote char and backslash inside
              let mut out = String::with_capacity(inner.len());
              let mut chars = inner.chars().peekable();
              while let Some(c) = chars.next() {
                  if c == '\\' {
                      if let Some(&next) = chars.peek() {
                          if next == first || next == '\\' {
                              out.push(next);
                              chars.next();
                              continue;
                          }
                      }
                  }
                  out.push(c);
              }
              return out;
          }
      }
      tok.to_string()
  }
  ```

  `tokenize` は現行のまま `escaped` フラグをクォート内外で正しく扱っているため変更不要。`escaped` が `true` のときに `cur.push(c)` して `escaped = false` にするロジックは既にクォート内外で有効（Step 1 の前に `if escaped` をチェックしている）。必要なら `tokenize` の `Some(q)` 分岐で `if c == q && !escaped` のようにするが、現行は `escaped` チェックが先なので正しい。念のためコメントを 1 行追加:

  ```rust
  // Backslash escapes the next character even inside quotes
  ```

  `required_paths` の `contains("$(")` 等の判定は `unquote` 前の `tok` で行うため、エスケープされたクォートが `valid` に影響しない。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test paths_test handles_escaped -- --nocapture`
  Expected: 2 件とも PASS

  Run: `cargo test --test paths_test 2>&1 | grep "test result"`
  Expected: 全 10 件 PASS（既存 8 + 新規 2）

- [ ] **Step 5: CHANGELOG 追記**

  `## [Unreleased]` に:

  ```md
  - Fix `required_paths` to handle escaped quotes inside quoted arguments
  ```

- [ ] **Step 6: ゲート確認**

  Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
  Expected: 全て green

  Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt && bash tests/zsh/smoke.sh target/release/seasalt`
  Expected: 両方 `OK`（paths 変更は suggest 経路に影響するため smoke の stale-file フィルタ節が通ることを確認）

- [ ] **Step 7: コミット（要承認）**

  ```bash
  git add src/paths.rs tests/paths_test.rs CHANGELOG.md
  git commit -m "fix(paths): handle escaped quotes inside quoted args"
  ```

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt` が `OK`（または `SKIP`）
- [ ] `git log --oneline -6` で 4 コミット（Task 1〜4）が積まれていること
- [ ] `git diff 05e8037..HEAD --stat` が `src/main.rs`, `src/suggest.rs`, `src/db.rs`, `src/paths.rs`, `tests/*`, `CHANGELOG.md` のみに収まっていること
- [ ] `CHANGELOG.md` の `## [Unreleased]` に 4 行が追記されていること

## Self-Review

- [x] Spec coverage: Task 1（search help）、Task 2（ancestors 正規化）、Task 3（delete 束ね）、Task 4（paths クォート）全てに task がある。Task 5,6 はスコープ外として明記
- [x] Placeholder scan: `TBD`/`TODO`/`placeholder` なし。全 step に具体的なコードとコマンドを記載。`Similar to Task N` なし
- [x] Type consistency: `ancestors(cwd: &str) -> Vec<String>` / `delete_by_ids(conn: &Connection, ids: &[i64]) -> Result<()>` / `required_paths(cwd: &str, cmd: &str) -> Vec<String>` / `unquote(tok: &str) -> String` のシグネチャは既存と一致。各タスクの Produces/Consumes も一致
