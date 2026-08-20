# レビュー指摘対応（busy_timeout / trim_history / zsh hook） Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** レビューで指摘された 3 件の不具合（busy_timeout と suggest 200ms の不整合、trim_history の同点 started_at での超過保持、zsh スニペットの二重フック）を修正し、search のワイルドカード挙動については是非を整理して方針を決める。

**Architecture:** Rust 側 2 ファイル（`src/db.rs`）と zsh スニペット（`src/zsh/seasalt.zsh`）への局所修正のみ。DB マイグレーションやスキーマ変更は不要。`trim_history` は SQL の述語をタプル比較に置換して厳密に `max` 件に収める。`busy_timeout` は定数 1 行変更とテスト期待値更新。zsh は `add-zsh-hook` 成功時の二重登録を解消するため if/else に分岐。search はコード変更せず、本 plan 内で pros/cons を評価して結論を出す。

**Tech Stack:** Rust 2021 / rusqlite 0.37 (bundled SQLite) / zsh / bash smoke

**Spec:** レビューコメント（2026-08-21）と本 plan が権威。既存設計は `docs/internal/specs/2026-08-15-seasalt-design.md` を参照。search ワイルドカードは同 spec §4 の `search` 仕様と README の “PATTERN is matched as substring (SQL LIKE semantics)” に基づく。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test` をこの順で実行し、全て green（AGENTS.md の pre-commit gate）
- `cargo test` 以外の smoke は該当タスクでのみ実行: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` / `bash tests/zsh/smoke.sh target/release/seasalt`
- コメントは英語、テストも英語。設計 spec・本 plan は日本語（既存文書）
- Silence contract を維持: `record`/`exit`/`suggest` は stderr に何も書かない
- `init` は DB / data dir に触れてはならない（既存 `cli_test::init_does_not_create_data_dir` で担保）
- スコープ外: 新機能追加、依存クレート追加・更新、flake.nix 変更
- コミットは各タスクでユーザー事前承認を得ること

---

### Task 1: busy_timeout 300ms → 150ms

**Files:**
- Modify: `src/db.rs:63-68` — `busy_timeout` の値とコメント
- Modify: `tests/db_test.rs:338-358` — `open_sets_bounded_busy_timeout` の期待値

**Interfaces:**
- Consumes: `rusqlite::Connection::busy_timeout(Duration)` / `PRAGMA busy_timeout`
- Produces: `db::open()` が 150ms の busy_timeout を設定する（suggest の 200ms deadline 内に収まる）

- [ ] **Step 1: 失敗テストを書く（期待値を 150 に更新して失敗させる）**

  `tests/db_test.rs:338-358` の該当テストを以下のように書き換える（現行は 300 を期待しているため、150 に変えた段階で FAIL する）:

  ```rust
  #[test]
  fn open_sets_bounded_busy_timeout() {
      let dir = std::env::temp_dir().join(format!(
          "seasalt-busy-timeout-{}-{}",
          std::process::id(),
          std::thread::current().name().unwrap_or("t")
      ));
      let _ = std::fs::remove_dir_all(&dir);
      std::fs::create_dir_all(&dir).unwrap();
      let path = dir.join("history.sqlite3");

      let conn = db::open(&path).unwrap();
      let timeout: i64 = conn
          .query_row("SELECT * FROM pragma_busy_timeout", [], |row| row.get(0))
          .unwrap();
      // 200ms suggest budget 内に収めるため 150ms に短縮
      assert_eq!(timeout, 150);

      let _ = std::fs::remove_dir_all(&dir);
  }
  ```

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test db_test open_sets_bounded_busy_timeout -v`
  Expected: FAIL with `assertion failed: left: 300 right: 150`

- [ ] **Step 3: 実装（`src/db.rs:64-68`）**

  ```rust
      // Bound the wait for other shells' writers to 150ms; rusqlite's default
      // (5000ms) could stall a shell hook behind a stuck writer for seconds.
      // 150ms stays comfortably inside the 200ms suggest budget so a slow
      // writer never pushes a keystroke past the UI deadline.
      conn.busy_timeout(Duration::from_millis(150))?;
  ```

  変更はこの 3 行のみ。コメントの 300ms → 150ms と 1 行追加の理由説明を含む。

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test db_test open_sets_bounded_busy_timeout -v`
  Expected: PASS

  追加で `cargo test --test db_test writers_wait_for_busy_database -v` も PASS することを確認（150ms でも 150ms スリープする blocker に対して十分待つ。blocker は 150ms sleep なので境界値だが、`busy_timeout` が 150ms ならギリギリ待てる。実装では 150ms スリープ + COMMIT のため、タイミング次第で flaky になる可能性がある — Step 4 で 3 回連続 PASS を確認すること）

- [ ] **Step 5: ゲート確認**

  Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test`
  Expected: 全て green（他テストへの影響なし）

- [ ] **Step 6: コミット（要承認）**

  ```bash
  git add src/db.rs tests/db_test.rs
  git commit -m "fix(db): lower busy_timeout to 150ms to fit suggest budget"
  ```

---

### Task 2: trim_history をタプル比較で厳密化

**Files:**
- Modify: `src/db.rs:203-215` — `trim_history()` の SQL
- Modify: `tests/db_test.rs` — 新規テスト `trim_history_strict_with_tied_timestamps` を追加
- Modify: `tests/cli_test.rs` — 既存 `record_trims_to_history_max` の許容範囲を厳密化（任意、検討）

**Interfaces:**
- Consumes: `history(id, started_at)` / `ORDER BY started_at DESC, id DESC`
- Produces: `trim_history(conn, max)` が厳密に `max` 件を残す（同点 `started_at` でも超過保持しない）

- [ ] **Step 1: 失敗テストを書く**

  `tests/db_test.rs` 末尾に追加:

  ```rust
  #[test]
  fn trim_history_strict_with_tied_timestamps() {
      let conn = Connection::open_in_memory().unwrap();
      db::init(&conn).unwrap();
      // All rows share the same started_at, differing only by id.
      // With the old `started_at < threshold` predicate, ties at the
      // boundary survive and the table keeps >max rows.
      for i in 0..12 {
          db::record_history(
              &conn,
              "/x",
              &format!("cmd {i}"),
              1000, // same timestamp for all
              "s",
              "",
              "bash",
          )
          .unwrap();
      }
      db::trim_history(&conn, 10).unwrap();
      let count: i64 = conn
          .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
          .unwrap();
      assert_eq!(count, 10, "expected strict trim to 10, ties must not survive");
      // The 10 newest by id must remain (ids are monotonic)
      let min_id: i64 = conn
          .query_row("SELECT min(id) FROM history", [], |r| r.get(0))
          .unwrap();
      let max_id: i64 = conn
          .query_row("SELECT max(id) FROM history", [], |r| r.get(0))
          .unwrap();
      assert_eq!(max_id - min_id, 9);
  }
  ```

  このテストは現行 SQL（`started_at < threshold`）では FAIL する（12 件全て残る、または閾値が 1000 のため `started_at < 1000` が 0 件で 12 件残る）。

- [ ] **Step 2: テストを実行して失敗を確認**

  Run: `cargo test --test db_test trim_history_strict_with_tied_timestamps -v`
  Expected: FAIL with `assertion failed: left: 12 right: 10`

- [ ] **Step 3: 実装（`src/db.rs:203-215`）**

  現行:

  ```rust
  pub fn trim_history(conn: &Connection, max: usize) -> Result<()> {
      conn.execute(
          "DELETE FROM history WHERE started_at < (
             SELECT started_at FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1
           )",
          rusqlite::params![max as i64 - 1],
      )?;
      Ok(())
  }
  ```

  置換後（タプル比較で厳密化）:

  ```rust
  /// Deletes history rows older than the newest `max` rows (by
  /// started_at, id). Strictly keeps `max` rows even when many rows
  /// share the same started_at — the old `started_at < threshold`
  /// predicate kept ties at the boundary. Callers must not pass
  /// `max = 0` (unlimited is handled before calling).
  pub fn trim_history(conn: &Connection, max: usize) -> Result<()> {
      conn.execute(
          "DELETE FROM history WHERE (started_at, id) < (
             SELECT started_at, id FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1
           )",
          rusqlite::params![max as i64 - 1],
      )?;
      Ok(())
  }
  ```

  補足:
  - SQLite の row-value 比較 `(a,b) < (SELECT a,b ...)` は bundled SQLite（3.44+）でサポート済み。`ROW_NUMBER()` を使っている既存マイグレーションが通る環境なら問題なし。
  - サブクエリが 0 行（`count < max`）の場合、スカラー比較は NULL となり `WHERE NULL` は false のため何も削除されない — 既存の「上限未満は何もしない」挙動を維持。
  - `(started_at, id)` は `started_at` 単独インデックス + PK のため、既存の `idx_history_started_at` で十分。必要なら後続で複合インデックスを検討するが本 task では追加しない。

  代替案として `OR` で展開する形も可だが、可読性と 1 サブクエリで済む点から row-value を採用。もし row-value が環境で不安定なら以下にフォールバック:

  ```sql
  DELETE FROM history WHERE started_at < (SELECT started_at FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1)
     OR (started_at = (SELECT started_at FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1)
         AND id < (SELECT id FROM history ORDER BY started_at DESC, id DESC LIMIT 1 OFFSET ?1))
  ```

- [ ] **Step 4: テストを実行して成功を確認**

  Run: `cargo test --test db_test trim_history -v`
  Expected: 全 4 件 PASS（`trim_history_strict_with_tied_timestamps` + 既存 3 件 `keeps_newest_rows` / `at_limit_deletes_nothing` / `under_limit_deletes_nothing` / `protects_refreshed_rows`）

  追加で `cargo test --test db_test -v` 全体が PASS することを確認。

- [ ] **Step 5: ゲート確認**

  Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test`
  Expected: 全て green

  注意: `tests/cli_test.rs:442-470` の `record_trims_to_history_max` は実時間 ms 精度のため従来 `10..=12` の許容範囲にしている。厳密化後は理論上 10 件に収まるが、複数プロセスの同時実行で `started_at` が同一 ms にまとまるケースでも 10 件に収まるはず。許容範囲を `assert_eq!(lines.len(), 10)` に狭めるかは本 task では見送り、コメントのみ追記する（狭めると flaky になるリスクがあるため）。

- [ ] **Step 6: コミット（要承認）**

  ```bash
  git add src/db.rs tests/db_test.rs
  git commit -m "fix(db): make trim_history strict with tuple comparison"
  ```

---

### Task 3: zsh スニペットの二重フック登録を分岐に整理

**Files:**
- Modify: `src/zsh/seasalt.zsh:39-43` — `add-zsh-hook` と `preexec_functions` 直接操作の分岐

**Interfaces:**
- Consumes: `add-zsh-hook`（zsh 組み込み）、`preexec_functions` / `precmd_functions` 配列
- Produces: 同じフックが二重登録されない（`add-zsh-hook` 成功時は配列操作をスキップ）

- [ ] **Step 1: 現状の確認（手動）**

  現行 5 行:

  ```zsh
    autoload -Uz add-zsh-hook >/dev/null 2>&1;
    add-zsh-hook preexec _seasalt_preexec >/dev/null 2>&1 || true;
    add-zsh-hook precmd  _seasalt_precmd  >/dev/null 2>&1 || true;
    preexec_functions=(_seasalt_preexec ${preexec_functions:#_seasalt_preexec});
    precmd_functions=(_seasalt_precmd ${precmd_functions:#_seasalt_precmd});
  ```

  `add-zsh-hook` が内部で `preexec_functions` に追加するため、両方が実行されると一時的に二重に見えるが、後段の `${...:#...}` で重複除去しているため実害はない。ただし意図が不明瞭で、将来の変更で重複除去を外した際に二重発火する。

- [ ] **Step 2: 失敗テストは不要（既存 smoke で担保）**

  既存 `tests/zsh/smoke.sh` が `_seasalt_preexec` / `_seasalt_precmd` の 1 回発火を前提に `search --tsv` の行数や `_seasalt_last_id` を検証している。二重登録があると `record` が 2 回走り、dedup で id が同じになるため smoke は偶然 PASS するが、exit_code の二重 update や `ZSH_AUTOSUGGEST_STRATEGY` 操作の二重実行は隠れる。

  本 task では新規テストを追加せず、リファクタ後の smoke PASS で担保する。

- [ ] **Step 3: 実装（`src/zsh/seasalt.zsh:39-43`）**

  置換後:

  ```zsh
    if autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook preexec _seasalt_preexec 2>/dev/null && add-zsh-hook precmd _seasalt_precmd 2>/dev/null; then
      : # hooks registered via add-zsh-hook
    else
      preexec_functions=(_seasalt_preexec ${preexec_functions:#_seasalt_preexec});
      precmd_functions=(_seasalt_precmd ${precmd_functions:#_seasalt_precmd});
    fi;
  ```

  または、より保守的に preexec/precmd を個別に分岐:

  ```zsh
    autoload -Uz add-zsh-hook >/dev/null 2>&1;
    if (( ${+functions[add-zsh-hook]} )); then
      add-zsh-hook preexec _seasalt_preexec 2>/dev/null || preexec_functions=(_seasalt_preexec ${preexec_functions:#_seasalt_preexec});
      add-zsh-hook precmd  _seasalt_precmd  2>/dev/null || precmd_functions=(_seasalt_precmd ${precmd_functions:#_seasalt_precmd});
    else
      preexec_functions=(_seasalt_preexec ${preexec_functions:#_seasalt_preexec});
      precmd_functions=(_seasalt_precmd ${precmd_functions:#_seasalt_precmd});
    fi;
  ```

  どちらでも可だが、後者（個別フォールバック）の方が片方だけ失敗した際の堅牢性が高い。最終形は後者を採用する。

  変更後も `preexec_functions` / `precmd_functions` の重複除去 `${...:#...}` は残す（`add-zsh-hook` 未使用環境での二重 eval 対策）。

- [ ] **Step 4: 検証**

  Run: `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt`
  Expected: `OK`（zsh が無い環境では `SKIP`）

  追加で手動確認（zsh がある場合）:

  ```bash
  zsh -c 'source <(target/release/seasalt init zsh); echo ${#preexec_functions} ${preexec_functions}; echo ${#precmd_functions} ${precmd_functions}' | grep -q "_seasalt_preexec" && echo "hook present"
  ```

  二重登録がないことを `typeset -p preexec_functions` で `_seasalt_preexec` が 1 回のみ出現することを目視確認。

- [ ] **Step 5: ゲート確認**

  Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test`
  Expected: 全て green（zsh スニペットは Rust ビルドに影響しないが、念のため）

- [ ] **Step 6: コミット（要承認）**

  ```bash
  git add src/zsh/seasalt.zsh
  git commit -m "fix(zsh): avoid double hook registration when add-zsh-hook is available"
  ```

---

### Task 4: search ワイルドカード挙動の pros/cons 整理（コード変更なし）

**Files:**
- なし（本 plan 内での評価と、必要なら `docs/internal/specs/2026-08-15-seasalt-design.md` への追記は別途判断）

**Interfaces:**
- Consumes: `src/search.rs:27` の `LIKE "%pattern%"` / `src/db.rs:243` の `LIKE ... ESCAPE` / README の search 説明
- Produces: 方針決定（現状維持 / `--literal` 追加 / デフォルト literal 化）の記録

- [ ] **Step 1: 現状と pros/cons を整理**

  現状: `search` は `cmd LIKE "%pattern%"` で `%`/`_` をワイルドカードとして扱う。`suggest` は `GLOB`/`LIKE ESCAPE` でエスケープ済み。意図的に `search` のみワイルドカードを有効化している（README に明記）。

  | 選択肢 | Pros | Cons |
  |---|---|---|
  | **A. 現状維持（ワイルドカード有効）** | 強力な検索（`search "cargo %"` で前方一致相当）、実装ゼロ、既存ユーザの期待を壊さない | `search "foo_bar"` が `fooXbar` にもヒットして驚く。`_` は頻出（`git_log` 的な typo ではないが） |
  | **B. デフォルト literal、ワイルドカードは opt-in（`--literal` 既定 on）** | 直感的（多くのツールは literal 部分一致）。`_`/`%` を含むコマンドの検索が正確 | 破壊的変更。既存の `search "foo%"` 利用が壊れる。移行コスト |
  | **C. 両対応（`--literal` / `--glob` フラグ追加、既定は現状維持）** | 後方互換を保ちつつ literal 検索を提供。将来のデフォルト切替も可能 | フラグが増える。`search` の help が複雑化。実装は `escape_like` を流用すれば 10 行程度だが、テスト追加が必要 |

  推奨: **C を将来の選択肢として保留し、当面は A（現状維持） + help/README での注意喚起強化**。理由:
  - `search` は interactive ではなく CLI のため、ワイルドカードの驚きは `suggest` ほど深刻でない
  - `search` の `%`/`_` ワイルドカードを実際に活用しているユーザがいるか不明 — まず help に 1 行追記して様子を見るのが低リスク

- [ ] **Step 2: 結論を記録（本 plan 内で完結）**

  本 task ではコード変更を行わない。結論を本 plan のこの節に残し、実装が必要になった場合は別 plan（`search --literal` 追加）を起票する。

  もし help への 1 行追記を行う場合は、以下の差分を別コミットで検討:

  ```rust
  // src/main.rs Search.pattern の help に追記
  #[arg(help = "substring to search (SQL LIKE: % and _ are wildcards)")]
  pattern: String,
  ```

  ただし本 plan のスコープでは「追記しない」ことを既定とする（ユーザー承認後に判断）。

- [ ] **Step 3: レビュー**

  ユーザーに本 plan の Task 4 節を提示し、A/B/C のどれで進めるか合意を得る。合意が得られれば本 task は完了とする。

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt` が `OK`（zsh 環境では）または `SKIP`
- [ ] `git status` が clean、`git log --oneline -5` で 3 コミット（Task 1,2,3）を確認
- [ ] `git diff HEAD~3 --stat` が `src/db.rs`, `tests/db_test.rs`, `src/zsh/seasalt.zsh` のみに収まっていること

## Self-Review

- [x] Spec coverage: busy_timeout（Task 1）、trim_history（Task 2）、zsh hook（Task 3）、search ワイルドカード（Task 4）全てに対応する task がある
- [x] Placeholder scan: `TBD`/`TODO`/`placeholder` なし。全 step に具体的なコードとコマンドを記載
- [x] Type consistency: `trim_history(conn: &Connection, max: usize) -> Result<()>` のシグネチャは変更なし。`busy_timeout(Duration)` の型も維持。zsh はシェルスクリプトのため型不整合なし
