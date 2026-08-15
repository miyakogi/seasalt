# レビュー修正 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** コードレビューで承認された 6 項目（GLOB バグ修正 / busy_timeout / テスト env 競合 / DB API 平坦化 / search デフォルト cwd / 文書・ビルド修正）を実装する。

**Architecture:** 既存構造（db / suggest / search / paths モジュール）を維持したままの局所修正。新規依存なし。DB 層は `suggest_prefix` 単一エントリポイントに集約される。

**Tech Stack:** Rust (edition 2021), rusqlite (bundled), clap, anyhow, dirs; bash スニペット; Nix flake

**Spec:** docs/superpowers/specs/2026-08-15-seasalt-design.md（§5 の文言修正を含む）

**スコープ外（明示的に除外）:** DB 破損リカバリ（spec §7、要判断のため）、search と suggest_prefix の SQL 共通化（過剰抽象化）、record ロジックの lib 化。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy && cargo test` をこの順で実行し、全て green であること
- 最終タスク後: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
- コメントは英語。テストの fixture 文字列も英語
- コミットは事前承認済み（ユーザー確認済み 2026-08-16）
- 本タスクと無関係なコードの変更・リファクタはしない

---

### Task 1: GLOB エスケープ修正（case 優先の回帰修正）

**Files:**
- Modify: `src/db.rs:225-233`（`escape_glob`）
- Test: `tests/suggest_test.rs`（新テスト追加）

**背景:** SQLite GLOB にバックスラッシュエスケープは存在しない（in-memory SQLite で検証済み: `'ls *.txt' GLOB 'ls \*.txt'` → 0）。現実装は `* ? [ ] \` を `\` でエスケープしており、グロブ文字を含む行で exact-case 優先が壊れる（icase fallback がマスクするため、観測可能な症状は「exact-case が古くても優先されるべき場面で newer の icase が選ばれる」）。

- [ ] **Step 1: 失敗するテストを追加** — `tests/suggest_test.rs` 末尾に:

```rust
#[test]
fn exact_case_preferred_when_line_contains_glob_chars() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/x", "LS *.txt", 2000, "s", "").unwrap();
    db::record_history(&conn, "/x", "ls *.txt", 1000, "s", "").unwrap();
    // Like fish, the exact-case match wins even though it is older;
    // the broken GLOB escape made the sensitive pass miss and the
    // newer icase candidate "LS *.txt" win instead.
    let got = suggest::suggest(&conn, "/x", "ls *").unwrap().unwrap();
    assert_eq!(got, "ls *.txt");
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test suggest_test exact_case_preferred_when_line_contains_glob_chars`
  Expected: FAIL（got = `"LS *.txt"`）

- [ ] **Step 3: 実装修正** — `src/db.rs` の `escape_glob` を置換:

```rust
/// Escapes the GLOB special characters (* ? [) using character
/// classes: SQLite's GLOB has no escape character, so [*], [?] and
/// [[] match those characters literally. \ and ] are literal outside
/// a class and need no escaping.
fn escape_glob(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '*' => vec!['[', '*', ']'],
            '?' => vec!['[', '?', ']'],
            '[' => vec!['[', '[', ']'],
            other => vec![other],
        })
        .collect()
}
```

- [ ] **Step 4: テスト確認** — Run: `cargo test --test suggest_test`
  Expected: 全 PASS（既存の `case_sensitive_match_is_preferred` 等も green）

- [ ] **Step 5: コミット** — `git add src/db.rs tests/suggest_test.rs && git commit -m "fix: escape GLOB metacharacters with char classes"`（事前承認済み）

---

### Task 2: busy_timeout 追加

**Files:**
- Modify: `src/db.rs:50-63`（`open`）
- Test: `tests/db_test.rs`（新テスト追加）

**背景:** WAL でも writer-writer はデフォルト busy_timeout=0 で即 `SQLITE_BUSY`。複数ターミナルの同時実行で `record` が静かに失敗し履歴から欠落する。suggest は WAL リーダーなのでブロックされず、UI の 0.2s timeout とは無関係。

- [ ] **Step 1: 失敗するテストを追加** — `tests/db_test.rs` 末尾に:

```rust
#[test]
fn writers_wait_for_busy_database() {
    let dir = std::env::temp_dir().join(format!(
        "seasalt-busy-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.sqlite3");

    let conn = db::open(&path).unwrap();
    let blocker = db::open(&path).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        blocker.execute_batch("COMMIT").unwrap();
    });
    rx.recv().unwrap();
    // Starts while the other connection holds the write lock; without
    // busy_timeout this fails immediately with SQLITE_BUSY.
    let id = db::record_history(&conn, "/x", "echo hi", 1000, "s", "").unwrap();
    handle.join().unwrap();
    assert!(id > 0);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test db_test writers_wait_for_busy_database`
  Expected: FAIL（`database is locked` 系エラー）

- [ ] **Step 3: 実装** — `src/db.rs` の `open()` 内、`init(&conn)?` の直前に 1 行追加（`use std::time::Duration;` を import 追加）:

```rust
    // Wait briefly for other shells' writers instead of failing
    // immediately with SQLITE_BUSY (WAL readers are unaffected).
    conn.busy_timeout(Duration::from_millis(300))?;
```

- [ ] **Step 4: テスト確認** — Run: `cargo test --test db_test`
  Expected: 全 PASS

- [ ] **Step 5: コミット** — `git add src/db.rs tests/db_test.rs && git commit -m "fix: wait for concurrent writers instead of failing with SQLITE_BUSY"`（事前承認済み）

---

### Task 3: env 変数を触るテストの直列化

**Files:**
- Modify: `tests/db_test.rs`（`default_db_path_respects_env_override`、`new_data_dir_and_db_get_restricted_permissions`、`existing_data_dir_and_db_permissions_are_left_unchanged` の 3 テスト）

**背景:** `std::env::set_var` は process-global。3 テストが同一プロセス内の並列スレッドで実行されると競合し flaky になり得る。

- [ ] **Step 1: ロック追加** — `tests/db_test.rs` 冒頭（`use` 群の後）:

```rust
/// Serializes tests that mutate process-global env vars
/// (std::env::set_var is process-wide; parallel tests would race).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

- [ ] **Step 2: 3 テストにガード追加** — 各テスト関数の先頭行に `let _guard = ENV_LOCK.lock().unwrap();` を追加（対象行: 54 付近、202 付近、231 付近）

- [ ] **Step 3: 検証** — Run: `cargo fmt && cargo test --test db_test`（数回繰り返して安定を確認）
  Expected: 全 PASS

- [ ] **Step 4: コミット** — `git add tests/db_test.rs && git commit -m "test: serialize env-var mutating tests"`（事前承認済み）

---

### Task 4: suggest DB API の平坦化

**Files:**
- Modify: `src/db.rs:159-182`（`suggest_in_dir`/`suggest_global` 削除、`suggest_prefix` を pub(crate) 化）
- Modify: `src/suggest.rs:53-67`（`search_scope` 簡素化）
- Test: `tests/db_test.rs`（case テスト 2 件削除）、`tests/suggest_test.rs`（グローバル case テスト 1 件追加）

**Interfaces:**
- Produces: `pub(crate) fn suggest_prefix(conn: &Connection, cwd: Option<&str>, needle: &str, limit: usize, sensitive: bool) -> Result<Vec<(String, String)>>`
- Deletes: `suggest_in_dir`, `suggest_global`

- [ ] **Step 1: db.rs の書き換え** — 159-182 行の 2 関数 + それぞれの doc comment を削除し、`suggest_prefix` の宣言を `fn suggest_prefix(` → `pub(crate) fn suggest_prefix(` に変更。削除した doc の 1 つ目（prefix 一致・recency 降順・case の説明）を `suggest_prefix` の doc として残す。

- [ ] **Step 2: suggest.rs の書き換え** — `search_scope` 本体を置換:

```rust
fn search_scope(conn: &Connection, cwd: &str, line: &str, dir: Option<&str>) -> Result<Outcome> {
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, true)?;
    match pick(cwd, line, candidates) {
        Outcome::NoMatch => {}
        other => return Ok(other),
    }
    let candidates = db::suggest_prefix(conn, dir, line, CANDIDATE_LIMIT, false)?;
    Ok(pick(cwd, line, candidates))
}
```

- [ ] **Step 3: テスト整理** — `tests/db_test.rs` から `suggest_in_dir_is_case_sensitive_when_requested` と `suggest_global_is_case_sensitive_when_requested` を削除（dir スコープの case 挙動は `tests/suggest_test.rs` の `case_sensitive_match_is_preferred` 等が既にカバー）。グローバルスコープの case カバレッジを保つため `tests/suggest_test.rs` に追加:

```rust
#[test]
fn global_scope_prefers_exact_case() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/a", "Cargo build", 2000, "s", "").unwrap();
    db::record_history(&conn, "/a", "cargo check", 1000, "s", "").unwrap();
    // /nowhere has no cwd/parent match; the global scope prefers
    // the exact-case candidate even though it is older.
    let got = suggest::suggest(&conn, "/nowhere", "cargo").unwrap().unwrap();
    assert_eq!(got, "cargo check");
}
```

- [ ] **Step 4: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS・warning なし（`cargo check` で `suggest_prefix` の未使用警告が出ないことを確認）

- [ ] **Step 5: コミット** — `git add src/db.rs src/suggest.rs tests/db_test.rs tests/suggest_test.rs && git commit -m "refactor: collapse suggest_in_dir/suggest_global into suggest_prefix"`（事前承認済み）

---

### Task 5: search のデフォルトスコープをカレントディレクトリに

**Files:**
- Modify: `src/main.rs:42-53, 110-128`
- Test: `tests/cli_test.rs`（既存 2 テスト修正 + 新テスト 1 件）

**背景:** README/spec は「デフォルトはカレントディレクトリ」だが実装はグローバル検索。`--cwd` 省略時に `std::env::current_dir()` を既定値にする。あわせて `if all { None } else { cwd }` の死んだ分岐（clap の conflicts_with で --all と --cwd は共存不能）も解消される。

- [ ] **Step 1: 失敗するテストを追加** — `tests/cli_test.rs` 末尾に:

```rust
#[test]
fn search_defaults_to_current_directory() {
    let dir = temp_data_dir();
    let here = std::env::current_dir().unwrap();
    let here = here.to_str().unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", here, "--session", "s1", "--", "unique-cmd-xyz"])
        .status()
        .unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/elsewhere", "--session", "s1", "--", "unique-cmd-other"])
        .status()
        .unwrap();
    // Without --cwd the search is scoped to the process cwd
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "unique-cmd"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\tunique-cmd-xyz");
    // --all finds both
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "unique-cmd"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 2);
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test cli_test search_defaults_to_current_directory`
  Expected: FAIL（デフォルトがグローバルのため 2 件ヒットし 1 件目 assert が失敗）

- [ ] **Step 3: 実装** — `src/main.rs` の `Command::Search` 定義の doc と `--cwd` の help を更新:

```rust
    /// Search history (scoped to the current directory by default)
    Search {
        #[arg(long, conflicts_with = "all", help = "directory to search (default: the current directory)")]
        cwd: Option<String>,
```

`run` 内の `Command::Search` 分岐の cwd_filter 部分を置換:

```rust
            let cwd_filter = if all {
                None
            } else {
                match cwd {
                    Some(dir) => Some(dir),
                    None => std::env::current_dir()
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned()),
                }
            };
```

（current_dir が失敗する異常系ではグローバル検索にフォールバック）

- [ ] **Step 4: 既存テスト修正** — `tests/cli_test.rs` の `record_then_exit_then_search`（39 行）と `record_dedups_identical_command`（87 行）の `search --tsv ...` に `--all` を追加（cwd `/tmp/x`・`/x` への記録がテストプロセスの cwd と異なるため）。例: `.args(["search", "--all", "--tsv", "hello"])`

- [ ] **Step 5: 検証** — Run: `cargo fmt && cargo check && cargo clippy && cargo test`
  Expected: 全 PASS（smoke.sh の `search --tsv hello` は同一シェル内なので影響なし）

- [ ] **Step 6: コミット** — `git add src/main.rs tests/cli_test.rs && git commit -m "feat: scope search to the current directory by default"`（事前承認済み）

---

### Task 6: 文書・ビルド設定の軽微修正

**Files:**
- Modify: `docs/superpowers/specs/2026-08-15-seasalt-design.md:125`
- Modify: `flake.nix`

- [ ] **Step 1: spec §5 修正** — 125 行目 `返った (session, id) を変数保持` → `返った行 id を変数保持`（§4 および実装と一致させる。session は照合に使われない）

- [ ] **Step 2: flake.nix の version 一本化** — `outputs` の let に追加し、version/pname を Cargo.toml から読む:

```nix
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          version = cargoToml.package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "fish-style inline autosuggestion and per-directory history for bash";
            mainProgram = "seasalt";
          };
        };
```

- [ ] **Step 3: 検証** — Run: `nix eval .#packages.x86_64-linux.default.version`
  Expected: `"0.1.0"` を出力

- [ ] **Step 4: コミット** — `git add docs/superpowers/specs/2026-08-15-seasalt-design.md flake.nix && git commit -m "chore: read package version from Cargo.toml; align spec data flow"`（事前承認済み）

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt && cargo check && cargo clippy && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK` を出力
- [ ] `git status` が clean、`git log --oneline -6` でコミット内容を確認
