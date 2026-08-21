# Review Followups: pick deadline / composite index / empty env / tsv cwd / icase docs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** レビューで承認された5件（pick の deadline 監視、`(cwd, started_at, id)` 複合インデックス[計測→実装]、空 `SEASALT_DATA_DIR` の耐性、`search --tsv` の cwd エスケープ、非ASCII icase の README 注記）を局所修正で潰す。

**Architecture:** Rust 側の局所修正のみ。`src/suggest.rs` の `pick` に deadline を貫通、`src/db.rs` の SCHEMA を複合インデックス化し migration v4 を追加、`src/db.rs` の `default_db_path` で空envを未設定扱いに、`src/main.rs` の TSV 出力で `cwd` も `escape_cmd` で1行化、`README.md` に英文1行注記。DB スキーマはインデックス名変更のみで列追加なし。

**Tech Stack:** Rust 2021 / rusqlite 0.37 bundled / clap 4 / criterion 0.5（計測） / bash, zsh smoke

**Spec:** レビューコメント 2026-08-21〜22 と本 plan が権威。既存設計は `docs/internal/specs/2026-08-15-seasalt-design.md` §4（suggest は同期・200ms 予算）を参照。シェルクォートは `src/bash/seasalt.bash`・`src/zsh/seasalt.zsh` を参照。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` をこの順で実行し、全て green（AGENTS.md の pre-commit gate）
- suggest 経路を触るタスクは加えて: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`（Task 1・最終検証）
- コメントは英語、テストも英語。設計 spec・本 plan は日本語（既存文書）
- Silence contract を維持: `record`/`exit`/`suggest` は stderr に何も書かない。`init`/`search`/`delete`/`clear` はエラーを stderr に出す
- `init` は DB / data dir に触れてはならない（既存 cli_test で担保）
- CHANGELOG は各タスクで `## [Unreleased]` に1行追記（バージョン bump・release は本 plan のスコープ外）
- コミットは各タスクでユーザー事前承認を得ること（`git add` までは実行可、`git commit` は承認後に1回）
- スコープ外: サイレンス契約の clap `try_parse` 化、初回マイグレーションの `BEGIN IMMEDIATE`、`[profile.release]`、`rust-version`（いずれも見送り判断済み）

---

### Task 1: suggest の pick に deadline 監視を追加

**Files:**
- Modify: `src/suggest.rs` — `pick()` / `search_scope()` のシグネチャ、同ファイル末尾に `#[cfg(test)] mod tests` を新設
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `suggest_budgeted` → `search(conn, cwd, line, deadline)` が `search_scope(...)` と `pick(...)` を呼ぶ経路（既存）
- Produces: `pick(cwd: &str, line: &str, candidates: Vec<(String, String)>, deadline: Option<Instant>) -> Outcome` — deadline 超過時に即 `NoMatch` を返す
- Produces: `search_scope(conn: &Connection, cwd: &str, line: &str, dir: Option<&str>, deadline: Option<Instant>) -> Result<Outcome>` — `pick` に deadline を渡す
- Produces: テストから直接参照する `Outcome`（`Suggest`/`Identical`/`NoMatch`）と `pick`

背景: `progress_handler`（suggest.rs:33）は SQLite 実行のみ中断する。候補選択の `pick → paths::valid → resolve().exists()`（suggest.rs:140-152, paths.rs:38-45）は Rust 側で止まらず、最大各 scope 10 候補の stat が予算超過後も走り得る。`pick` で deadline を見ることで、1フォールバックでの超過を防ぐ。

- [ ] **Step 1: 失敗テストを書く**

`src/suggest.rs` 末尾に追記:

```rust
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
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `cargo test --test suggest_test pick -- --nocapture` に加え、lib の unit test へ
Expected: `cargo test --lib` で `cargo build` エラー（`pick` が未定義の deadline 引数を受けないためコンパイル失敗）。これは想定通りの失敗。

※ `pick` は integration test から見えないため、本 Task の pass/fail は `cargo test --lib` で確認する。

- [ ] **Step 3: 実装**

`suggest.rs` の `search_scope`（suggest.rs:156-163）と `pick`（suggest.rs:140-152）を更新:

```rust
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
```

`search_scope`:

```rust
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
```

`suggest.rs` の `search`（suggest.rs:80, 90, 100）の3箇所の `search_scope(...)` 呼び出しに `deadline` を第5引数として渡す:

- suggest.rs:80 → `search_scope(conn, cwd_norm, line, Some(cwd_norm), deadline)`
- suggest.rs:90 → `search_scope(conn, cwd_norm, line, Some(&anc), deadline)`
- suggest.rs:100 → `search_scope(conn, cwd_norm, line, None, deadline)`

- [ ] **Step 4: テストを実行して成功を確認**

Run: `cargo test --lib suggest`
Expected: 新規 `pick_*` 3件が PASS。
Run: `cargo test --test suggest_test`
Expected: 既存 suggest テスト全件 PASS（シグネチャは internal のみ変更のため外部 API 不変）。

- [ ] **Step 5: CHANGELOG 追記**

`CHANGELOG.md` 冒頭（`All notable changes...` の直後）に `## [Unreleased]` を新設し、1件目を追記:

```md
## [Unreleased]

- Harden `suggest` to stop scanning candidates once the 200ms budget has elapsed
```

（既存の `## [0.2.2]` より上に置く。）

- [ ] **Step 6: ゲート確認 + smoke**

Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: 全て green
Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
Expected: `OK`（suggest 経路の回帰なし）

- [ ] **Step 7: コミット（要承認）**

```bash
git add src/suggest.rs CHANGELOG.md
git commit -m "perf(suggest): stop picking candidates after the budget elapses"
```

---

### Task 2: (cwd, started_at, id) 複合インデックス（計測 → 実装）

**Files:**
- Modify: `src/db.rs` — SCHEMA の `idx_history_cwd` を複合化、`migrate` に v4 追加
- Modify: `tests/db_test.rs` — `init_records_schema_version` の期待値 3→4、`migration_v4_*` 新規テスト、`idx_history_cwd` 存在テストがあれば更新（無い）
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `db::open` → `init` → `migrate`（既存）
- Produces: スキーマ v4。`idx_history_cwd` を `idx_history_cwd_started ON history(cwd, started_at DESC, id DESC)` に置換
- Produces: `PRAGMA user_version = 4`

背景: suggest の scope クエリ（db.rs:258-267）は `WHERE cwd = ? AND cmd GLOB ? ORDER BY started_at DESC, id DESC LIMIT 10`。現行 `idx_history_cwd` だと「cwd 全行を取って in-memory ソート」。複合インデックスなら `(cwd, started_at, id)` の順で index walk しながら GLOB フィルタし、10件で打ち切れる。`search` の scope クエリ対象（search.rs:39-42）にも同効。**計測手法は `cargo bench`（`suggest_in_process` の miss@1M が最悪ケースのソート）で before/after を取得する。**

- [ ] **Step 1: baseline を計測**

Run: `cargo bench --bench suggest suggest_in_process`
Expected: `miss/1000000`, `miss_deep/1000000` の ms を記録（これが「before」）。criterion のイテレーション統計に note するか、後述 Step 5 の比較用に控える。出力例: `suggest_in_process/miss_deep/1000000 time: [X ms Y ms Z ms]`。

- [ ] **Step 2: 失敗テストを書く**

`tests/db_test.rs` の `init_records_schema_version` 期待値を `3` → `4` に変更:

```rust
    assert_eq!(v, 4);
```

同テストに残る `assert_eq!(v, 4)` 部分。また migration テストを新設:

```rust
#[test]
fn migration_v4_replaces_cwd_index_with_composite() {
    let dir = std::env::temp_dir().join(format!("seasalt-mig4-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               cwd TEXT NOT NULL,
               cmd TEXT NOT NULL,
               exit_code INTEGER,
               started_at INTEGER NOT NULL,
               session TEXT,
               paths TEXT NOT NULL DEFAULT '',
               shell TEXT NOT NULL DEFAULT 'bash'
             );
             CREATE INDEX idx_history_cwd ON history(cwd);
             CREATE INDEX idx_history_cmd ON history(cmd);
             CREATE INDEX idx_history_started_at ON history(started_at);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history (cwd, cmd, started_at) VALUES ('/a', 'echo hi', 1)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
    }
    let conn = db::open(&path).unwrap();

    // Old single-column index is gone, composite index is in place
    let old: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'idx_history_cwd'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
    let new: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'idx_history_cwd_started'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new, 1);
    // Data survives the migration
    let (cmd, v): (String, i64) = conn
        .query_row(
            "SELECT cmd, (SELECT * FROM pragma_user_version) FROM history WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(cmd, "echo hi");
    assert_eq!(v, 4);
    std::fs::remove_dir_all(&dir).unwrap();
}
```

- [ ] **Step 3: テストを実行して失敗を確認**

Run: `cargo test --test db_test migration_v4 -- --nocapture`
Expected: `migration_v4_replaces_cwd_index_with_composite` が FAIL（インデックス名不一致。`idx_history_cwd_started` が count 0）。
Run: `cargo test --test db_test init_records_schema_version`
Expected: FAIL（`assertion failed: v == 4`）。

- [ ] **Step 4: 実装**

`src/db.rs` の SCHEMA（db.rs:18）を変更:

```rust
CREATE INDEX IF NOT EXISTS idx_history_cwd_started ON history(cwd, started_at DESC, id DESC);
```

`migrate`（db.rs:95-133）の v3 ブロック後に v4 ブロックを追加:

```rust
    if version < 4 {
        // v3 -> v4: the single-column cwd index is replaced by a composite
        // (cwd, started_at, id) index so the scoped suggest/search queries
        // can walk rows in recency order without an in-memory sort.
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_history_cwd;
             CREATE INDEX IF NOT EXISTS idx_history_cwd_started ON history(cwd, started_at DESC, id DESC);",
        )?;
        conn.pragma_update(None, "user_version", 4)?;
    }
```

- [ ] **Step 5: テストを実行して成功を確認 + 複合インデックスで再計測**

Run: `cargo test --test db_test migration_v4 -- --nocapture`
Expected: PASS（count 0 / count 1 / data 保持 / v=4）
Run: `cargo test --test db_test`
Expected: 既存含め全 PASS（`init_records_schema_version` の v=4 を含む）。
Run: `cargo bench --bench suggest suggest_in_process`
Expected: `miss/1000000`, `miss_deep/1000000` の ms を記録（after）。**after が before より悪化していないことを確認。** 改善していればその旨、悪化していなければ「悪化なし」をコミットメッセージ/レビューに記載。悪化した場合はこのタスクでインデックス変更を revert するコミットを追加し報告する（複合化しない判断）。

- [ ] **Step 6: CHANGELOG 追記**

`## [Unreleased]` に追記:

```md
- Speed up `suggest`/`search` in high-volume directories with a `(cwd, started_at, id)` composite index
```

- [ ] **Step 7: ゲート確認 + smoke**

Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: 全て green
Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
Expected: `OK`

- [ ] **Step 8: コミット（要承認）**

```bash
git add src/db.rs tests/db_test.rs CHANGELOG.md
git commit -m "perf(db): index cwd+started_at so scoped suggest/search avoid a sort"
```

---

### Task 3: 空 SEASALT_DATA_DIR を未設定扱いに

**Files:**
- Modify: `src/db.rs` — `default_db_path()`
- Modify: `tests/db_test.rs` — 新規テスト
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: 環境変数 `SEASALT_DATA_DIR`
- Produces: `default_db_path() -> Result<PathBuf>` — env が空文字のときだけ XDG/`dirs::data_dir()` にフォールバックする

背景: `src/db.rs:33-51` は `SEASALT_DATA_DIR=""` を参照し `PathBuf::from("")` → `create_dir_all("")` が実エラーし全サブコマンドが壊れる。空文字は未設定と同等に扱う。

- [ ] **Step 1: 失敗テストを書く**

`tests/db_test.rs` 末尾に追記（`ENV_LOCK` を使う既存パターンに合わせる）:

```rust
#[test]
fn default_db_path_treats_empty_env_as_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    let data_home = std::env::temp_dir().join(format!("seasalt-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_home);
    // Empty SEASALT_DATA_DIR must be ignored, falling back to XDG_DATA_HOME
    std::env::set_var("SEASALT_DATA_DIR", "");
    std::env::set_var("XDG_DATA_HOME", &data_home);
    let path = db::default_db_path().unwrap();
    assert_eq!(path, data_home.join("seasalt").join("history.sqlite3"));
    std::env::remove_var("SEASALT_DATA_DIR");
    std::env::remove_var("XDG_DATA_HOME");
    let _ = std::fs::remove_dir_all(&data_home);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `cargo test --test db_test default_db_path_treats_empty_env_as_unset -- --nocapture`
Expected: FAIL（`SEASALT_DATA_DIR=""` が `PathBuf::from("")` となり assert 不一致 or `create_dir_all("")` エラー）。

- [ ] **Step 3: 実装**

`src/db.rs` の `default_db_path`（db.rs:33-51）第1分岐を変更:

```rust
pub fn default_db_path() -> Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("SEASALT_DATA_DIR") {
        // An empty value is treated as unset so a stray exported variable
        // does not break every subcommand with a nonexistent base path.
        if dir.is_empty() {
            if let Some(data) = dirs::data_dir() {
                data.join("seasalt")
            } else {
                anyhow::bail!("cannot determine data directory; set SEASALT_DATA_DIR");
            }
        } else {
            PathBuf::from(dir)
        }
    } else if let Some(data) = dirs::data_dir() {
        data.join("seasalt")
    } else {
        anyhow::bail!("cannot determine data directory; set SEASALT_DATA_DIR");
    };
    // ...（既存の existed / create_dir_all / restrict_dir の流れはそのまま）
```

（後続の `existed`〜`restrict_dir` 部分は変更しない。）

- [ ] **Step 4: テストを実行して成功を確認**

Run: `cargo test --test db_test default_db_path -- --nocapture`
Expected: 既存 `default_db_path_respects_env_override` と新規 `default_db_path_treats_empty_env_as_unset` の2件 PASS。

- [ ] **Step 5: CHANGELOG 追記**

`## [Unreleased]` に追記:

```md
- Treat an empty `SEASALT_DATA_DIR` as unset instead of failing on a nonexistent path
```

- [ ] **Step 6: ゲート確認**

Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: 全て green

- [ ] **Step 7: コミット（要承認）**

```bash
git add src/db.rs tests/db_test.rs CHANGELOG.md
git commit -m "fix(db): ignore an empty SEASALT_DATA_DIR"
```

---

### Task 4: search --tsv の cwd もエスケープ

**Files:**
- Modify: `src/main.rs` — TSV 分岐の `e.cwd` を `escape_cmd` に通す
- Modify: `tests/cli_test.rs` — 新規テスト
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `escape_cmd(&str) -> String`（main.rs:213-217、既存）
- Produces: `seasalt search --tsv` の cwd 列がバックスラッシュ/改行/タブを含む場合も1行でエスケープされる

背景: `src/main.rs:177-185` で `cmd` は `escape_cmd` されるが `e.cwd` は生出力。Unix ではディレクトリ名にタブ/改行が許容され、TSV の列が崩れる。TSV の機械可読性を維持するため cwd も 1行化する。

- [ ] **Step 1: 失敗テストを書く**

`tests/cli_test.rs` に追記（`temp_data_dir` の並行安全なユニーク名パターンを使う）:

```rust
#[test]
fn search_tsv_escapes_cwd() {
    let dir = temp_data_dir();
    let name = std::thread::current().name().unwrap_or("t").to_string();
    // A directory whose name contains a tab (valid on Unix) breaks TSV columns
    let cwd_dir =
        std::env::temp_dir().join(format!("seasalt-tsv-cwd-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&cwd_dir);
    let tab_cwd = cwd_dir.join("a\tb");
    std::fs::create_dir_all(&tab_cwd).unwrap();
    let cwd = tab_cwd.to_str().unwrap();

    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", cwd, "--session", "s1", "--", "echo hi"])
        .status()
        .unwrap();

    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "echo hi"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // The cwd stays on a single line: the embedded tab is escaped as a literal backslash-t
    assert_eq!(text.lines().count(), 1, "got: {text}");
    assert!(text.contains("a\\tb"), "cwd not escaped: {text}");

    let _ = std::fs::remove_dir_all(&cwd_dir);
}
```

- [ ] **Step 2: テストを実行して失敗を確認**

Run: `cargo test --test cli_test search_tsv_escapes_cwd -- --nocapture`
Expected: FAIL（`lines().count()` が 2 になる or `a\\tb` を含まない。生タブで TSV 列が崩れる）。

- [ ] **Step 3: 実装**

`src/main.rs` の TSV 分岐（main.rs:178-182）で `e.cwd` を `escape_cmd` に通す:

```rust
                if tsv {
                    let code = e.exit_code.map(|c| c.to_string()).unwrap_or_default();
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        e.id, escape_cmd(&e.cwd), cmd, code, e.started_at, e.shell
                    );
```

（非-TSV 分岐は cwd を出さないため変更不要。`escape_cmd` の doc コメント「search output stays one line per entry」の趣旨に合致。）

- [ ] **Step 4: テストを実行して成功を確認**

Run: `cargo test --test cli_test search_tsv_escapes_cwd -- --nocapture`
Expected: PASS（1行、`a\\tb` を含む）。
Run: `cargo test --test cli_test record_then_exit_then_search`
Expected: 既存 TSV テスト（`fields[0..4] == ["1","/tmp/x",...]`）で cwd `/tmp/x` はエスケープ前後で不変のため PASS。

- [ ] **Step 5: CHANGELOG 追記**

`## [Unreleased]` に追記:

```md
- Escape the `cwd` column in `search --tsv` output so directory names containing tabs or newlines stay on one line
```

- [ ] **Step 6: ゲート確認**

Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: 全て green

- [ ] **Step 7: コミット（要承認）**

```bash
git add src/main.rs tests/cli_test.rs CHANGELOG.md
git commit -m "fix(search): escape cwd in --tsv output"
```

---

### Task 5: 非ASCII icase の文法を README に注記

**Files:**
- Modify: `README.md` — Known limitations に1件
- Modify: `CHANGELOG.md`

**Interfaces:**
- なし（ドキュメントのみ）

背景: suggest の case-insensitive フォールバックは SQLite LIKE/GLOB で ASCII のみ大小差を無視（`src/db.rs:248-257`）。README（README.md:20-24）の「exact case preferred … like fish」は ASCII 前提である旨を措置する。コード変更は任意とし本 plan では注記のみ（修正するなら `lower()` 列が必要な大工事のため見送り）。

- [ ] **Step 1: README に Known limitations を追記**

`README.md` の `## Known limitations` ブロック（README.md:284-294）冒頭に追記:

```md
- Case-insensitive suggestion matching only folds ASCII case (SQLite's
  `LIKE`/`GLOB`); non-ASCII case pairs such as `café`/`CAFÉ` are treated as
  distinct.
```

- [ ] **Step 2: CHANGELOG 追記**

`## [Unreleased]` に追記:

```md
- Document that case-insensitive suggestions only fold ASCII case
```

- [ ] **Step 3: ゲート確認（README 変更のみ）

Run: 変更が md のみであることを確認（`git status --short`）
Expected: `README.md` と `CHANGELOG.md` のみ変更。

- [ ] **Step 4: コミット（要承認）**

```bash
git add README.md CHANGELOG.md
git commit -m "docs: note ASCII-only case folding in suggestions"
```

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo bench --bench suggest suggest_in_process` の `miss_deep/1000000` が baseline コミット時点より悪化していない（Task 2 の記録と比較）
- [ ] `git log --oneline -6` に Task 1〜5 の5コミットが積まれていること
- [ ] `CHANGELOG.md` の `## [Unreleased]` に5行が追記されていること
- [ ] `PRAGMA user_version` が 4（`cargo test --test db_test init_records_schema_version` で担保）

## Self-Review

- [x] Spec coverage: 承認された5件に Task 1〜5 が対応（pick deadline / 複合インデックス / 空env / tsv cwd / icase 注記）。見送り判断済みの clap・migration BEGIN・profile.release・rust-version は Global Constraints のスコープ外に明記
- [x] Placeholder scan: `TBD`/`TODO`/`placeholder` なし。全 step に具体的なコードとコマンドを記載。"Similar to Task N" を排除し、各テストをコピーして完全記述
- [x] Type consistency: `pick(cwd, line, candidates, deadline) -> Outcome` / `search_scope(conn, cwd, line, dir, deadline) -> Result<Outcome>` のシグネチャは Task 1 内で defined→used が一致。`Outcome`/`pick` は private のまま `#[cfg(test)] mod tests`（同一モジュール内）から参照。`escape_cmd`/`default_db_path`/`db::open` は既存のまま再利用。`init_records_schema_version` の期待値変更は db.rs v4 と整合
- [x] 計測ゲート: Task 2 は Step 1（before）→ Step 5（after）で `cargo bench` を実行し、悪化時は revert する分岐を明記

---

## Outcome (2026-08-22)

5件中4件を採用、1件を不採用として本ブランチを `main` にマージした（merge `4c7f9d9`）。

| Task | 結果 | コミット |
|------|------|----------|
| 1 pick deadline | 採用 | `8c5a58c` |
| 2 composite `(cwd, started_at, id)` | **不採用** | —（計測で悪化のため revert） |
| 3 空 `SEASALT_DATA_DIR` | 採用 | `44b5813` |
| 4 `search --tsv` cwd エスケープ | 採用 | `7663526` |
| 5 非ASCII icase README 注記 | 採用 | `3f49174` |

### Task 2 不採用の経緯

- `cargo bench --bench suggest suggest_in_process` で `miss/1M` と `miss_deep/1M` を前後計測した。
  - `miss/1M`: `185.32ms` → `188.01ms`（**+1.45% 悪化**, `p<0.05`、再走でも `~186.7ms` と baseline 超えが再現）
  - `miss_deep/1M`: `154.47ms` → `154.08ms`（横ばい）
- いずれの指標でも改善せず、主要指標が悪化したため plan のゲート「悪化した場合は revert するコミットを追加し報告する（複合化しない判断）」に従い、SCHEMA/migration v4/テストの変更をコミット前に revert した。
- 考察: `miss` は `cwd` 一致行（1M中 ~125k）を全走査して早終了しないため、複合インデックスの「10件で打ち切り」の利得が出ない。複合 index の幅増加分のオーバーヘッドのみが残った。再検討するなら `hit`（早終了する）シナリオで利得を測るベンチが必要。

この Outcome 自体が不採用の永続記録であり、SDD の ledger（`.superpowers/sdd/.../progress.md`、gitignore 対象）は worktree 削除で失われているが内容はここに引き継いだ。