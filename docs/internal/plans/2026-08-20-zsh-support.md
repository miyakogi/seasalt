# zsh サポート Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** zsh の履歴記録（preexec/precmd フック）とインライン提案（zsh-autosuggestions ストラテジ）を追加し、履歴は統一したまま `shell` 列でタグ付けする。

**Architecture:** 新規 zsh スニペット `src/zsh/seasalt.zsh` をバイナリが埋め込み、`seasalt init zsh` で出力する。preexec/precmd フックが記録・終了コード保存を行い、`_zsh_autosuggest_strategy_seasalt` が zsh-autosuggestions の非同期フォーク内で `seasalt suggest` を呼ぶ（非ブロッキング）。`history` に `shell TEXT NOT NULL DEFAULT 'bash'` を追加（migration v3）。dedup `(cwd, cmd)` は維持。

**Tech Stack:** Rust (rusqlite, clap), zsh (add-zsh-hook, zsh-autosuggestions), SQLite WAL。

**Spec:** `docs/internal/specs/2026-08-20-zsh-support-design.md`（本プランの要求の権威。実装者は spec とこのプランを両方読む）

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy && cargo test` をこの順で実行し、全て green
- Task 5 は追加で `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt` が `OK` であること
- コメントは英語、テストも英語。設計 spec と本プランは日本語（既存文書）
- Silence contract: `record`/`exit`/`suggest` は stderr に何も書かない。`init`/`search`/`delete`/`clear` はエラーを stderr に出す
- `init` は DB / data dir に触れてはならない（cli_test で検証）
- bash スニペットの挙動は変えない（`--shell` 無指定 = 既定 `bash` で記録されること）
- スコープ外: fish/nushell/powershell、`suggest` の shell フィルタ、シェル別 DB 分離
- コミットは各タスクの指示どおり行う

---

### Task 2: DB — `shell` 列（schema, migration v3, record_history, HistoryEntry）

**Files:**
- Modify: `src/db.rs` — `SCHEMA` / `migrate()` / `record_history()` / `HistoryEntry`
- Modify: `tests/db_test.rs` — 新規テスト＋既存の全 `record_history(` 呼び出しへ `, "bash")` を追加

**Interfaces:**
- Consumes: 既存 `has_column(conn, name)` / `Connection`
- Produces: `record_history(conn, cwd, cmd, started_at, session, paths, shell) -> Result<i64>`; `HistoryEntry.shell: String`

- [ ] **Step 1: 失敗テストを書く**（`tests/db_test.rs` に追加）

```rust
#[test]
fn record_history_stores_shell() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    db::record_history(&conn, "/tmp/a", "echo hi", 1000, "s1", "", "zsh").unwrap();
    let shell: String = conn
        .query_row("SELECT shell FROM history WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(shell, "zsh");
}

#[test]
fn record_history_on_conflict_updates_shell_keeps_same_id() {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    let id1 = db::record_history(&conn, "/x", "echo hi", 1, "s1", "", "bash").unwrap();
    let id2 = db::record_history(&conn, "/x", "echo hi", 2, "s2", "", "zsh").unwrap();
    assert_eq!(id1, id2);
    let (count, shell): (i64, String) = conn
        .query_row("SELECT COUNT(*), MAX(shell) FROM history", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(shell, "zsh");
}

#[test]
fn migration_v3_adds_shell_column_defaulting_to_bash() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("seasalt-mig{}", std::process::id()));
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
               paths TEXT NOT NULL DEFAULT ''
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
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let conn = db::open(&path).unwrap();
    let shell: String = conn
        .query_row("SELECT shell FROM history WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(shell, "bash");
    std::fs::remove_dir_all(&dir).unwrap();
}
```

既存の `db::record_history(&conn, ...)`（6引数）の呼び出し全てに末尾 `, "bash")` を追加する。

- [ ] **Step 2: db_test を実行して失敗を確認** — `cargo test --test db_test`（`record_history` の7引数形が無い/`shell` 列が無いため FAIL 期待）
- [ ] **Step 3: `src/db.rs` を実装**
  - `SCHEMA` の `paths` の後に `\n  shell TEXT NOT NULL DEFAULT 'bash'`
  - `HistoryEntry` に `pub shell: String,`
  - `migrate()` の `if version < 2` ブロック後:
    ```rust
    if version < 3 {
        if !has_column(conn, "shell")? {
            conn.execute_batch("ALTER TABLE history ADD COLUMN shell TEXT NOT NULL DEFAULT 'bash'")?;
        }
        conn.pragma_update(None, "user_version", 3)?;
    }
    ```
  - `record_history` に引数と upsert 追加:
    ```rust
    "INSERT INTO history (cwd, cmd, started_at, session, paths, shell)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT(cwd, cmd) DO UPDATE SET
       started_at = excluded.started_at,
       session = excluded.session,
       paths = excluded.paths,
       shell = excluded.shell,
       exit_code = NULL
     RETURNING id"
    ```
- [ ] **Step 4: db_test を実行して成功を確認** — `cargo test --test db_test`（新規3件＋更新済み呼び出しが全て PASS）
- [ ] **Step 5: コミット** — `git add src/db.rs tests/db_test.rs && git commit -m "feat(db): add shell column tagging unified history"`

---

### Task 3: CLI — `record --shell`（既定 bash）, `search --tsv` に shell 列, `about`

**Files:**
- Modify: `src/search.rs` — `shell` を SELECT
- Modify: `src/main.rs` — `Record.shell`（既定 `bash`）、`record_history` へ、tsv 6列目、`about`
- Modify: `tests/cli_test.rs` — shell タグのテスト追加

**Interfaces:**
- Consumes: `record_history(..., shell)` / `HistoryEntry.shell`
- Produces: `record --shell <name>` 既定 `bash`; `search --tsv` が 6 列目に `shell`; `about` "…for bash and zsh"

- [ ] **Step 1: 失敗テスト**（`tests/cli_test.rs` に追加）

```rust
#[test]
fn record_shell_default_bash_and_tsv_sixth_column() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "echo a"])
        .status()
        .unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record", "--cwd", "/y", "--session", "s2",
            "--shell", "zsh", "--", "echo b",
        ])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "echo"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.trim().lines().collect();
    assert!(lines.iter().any(|l| l.ends_with("bash")), "got: {text}");
    assert!(lines.iter().any(|l| l.ends_with("zsh")), "got: {text}");
}
```

- [ ] **Step 2: cli_test を実行して失敗確認** — `cargo test --test cli_test record_shell_default_bash_and_tsv_sixth_column`
- [ ] **Step 3: 実装**
  - `src/search.rs`: 両 SELECT に `, shell` 追加、`map_row` に `shell: r.get(5)?,`
  - `src/main.rs`: `about` を `"fish-style autosuggestion and per-directory history for bash and zsh"`、`Record` に `#[arg(long, default_value = "bash")] shell: String,`、`record_history(..., &shell)`、tsv 分岐を `println!("{}\t{}\t{}\t{}\t{}\t{}", e.id, e.cwd, cmd, code, e.started_at, e.shell);`
- [ ] **Step 4: cli_test 成功確認** — `cargo test --test cli_test`（新規＋既存 `record_then_exit_then_search` の `fields[0..4]` は末尾追加なので不変）
- [ ] **Step 5: コミット** — `git add src/search.rs src/main.rs tests/cli_test.rs && git commit -m "feat(cli): record --shell tag, expose shell in search --tsv"`

---

### Task 4: zsh スニペット + `init zsh` emitter

**Files:**
- Create: `src/zsh/seasalt.zsh`
- Modify: `src/integration.rs` — `zsh_init_script()`
- Modify: `src/main.rs` — `init` の `"zsh"` 分岐
- Modify: `tests/cli_test.rs` — `init zsh` の内容・未対応 shell

**Interfaces:**
- Consumes: 起動時に何も使わない（init は DB/data dir に触れない）
- Produces: `zsh_init_script() -> &'static str`; `seasalt init zsh` がスニペットを出力; `seasalt init fish` が stderr にエラー

- [ ] **Step 1: 失敗テスト**（`tests/cli_test.rs` に追加）

```rust
#[test]
fn init_zsh_emits_snippet() {
    let out = bin().args(["init", "zsh"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("_zsh_autosuggest_strategy_seasalt"));
    assert!(text.contains("--shell zsh"));
    assert!(text.contains("precmd_functions"));
}

#[test]
fn init_unsupported_shell_fails() {
    let out = bin().args(["init", "fish"]).output().unwrap();
    assert!(!out.status.success());
}
```

- [ ] **Step 2: cli_test で失敗確認** — `cargo test --test cli_test init_zsh`（`unsupported shell: zsh` で FAIL 期待）
- [ ] **Step 3: `src/zsh/seasalt.zsh` を作成**（内容は spec に従う: `_seasalt_bin` トップレベル設定、`preexec_functions`/`precmd_functions` 先頭挿入、`_seasalt_preexec` は `$2`、`--shell zsh`、`_seasalt_precmd` 先頭で `$?` 捕捉、`_zsh_autosuggest_strategy_seasalt`、`ZSH_AUTOSUGGEST_STRATEGY=(seasalt ${ZSH_AUTOSUGGEST_STRATEGY[@]:-history})`、未ロード時 stderr 警告）
- `src/integration.rs` に `include_str!("zsh/seasalt.zsh")` の `zsh_init_script()`
- `src/main.rs` の `init` arm に `"zsh"` を追加
- [ ] **Step 4: cli_test 成功確認** — `cargo test --test cli_test init_zsh init_unsupported_shell_fails`
- [ ] **Step 5: コミット** — `git add src/zsh/seasalt.zsh src/integration.rs src/main.rs tests/cli_test.rs && git commit -m "feat(zsh): emit zsh integration snippet via init zsh"`

---

### Task 5: zsh ランタイム smoke テスト

**Files:**
- Create: `tests/zsh/smoke.sh`

**Interfaces:**
- Consumes: `seasalt` release バイナリ（引数 `$1`）、`zsh`（PATH）、Task 4 のスニペット
- Produces: 成功時 exit 0。FAIL/SKIP メッセージ。`cargo test` では実行されない

- [ ] **Step 1: `tests/zsh/smoke.sh` を作成**（release build 後に実行）

```bash
#!/usr/bin/env bash
# zsh integration smoke test. NOT run by `cargo test`; run explicitly:
#   cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt
set -euo pipefail
BIN=${1:?usage: smoke.sh /path/to/seasalt}
command -v zsh >/dev/null || { echo "SKIP: zsh not installed"; exit 0; }

DATA_DIR=$(mktemp -d)
trap 'rm -rf "$DATA_DIR"' EXIT
export SEASALT_DATA_DIR="$DATA_DIR"
export SEASALT_BIN="$BIN"

INIT=$("$BIN" init zsh)
grep -q -- '--shell zsh' <<<"$INIT" || { echo "FAIL: init zsh missing --shell zsh"; exit 1; }
grep -q '_zsh_autosuggest_strategy_seasalt' <<<"$INIT" || { echo "FAIL: init zsh missing strategy"; exit 1; }

ZDIR=$(mktemp -d)
printf 'source <(%q init zsh)\n' "$BIN" > "$ZDIR/.zshrc"

ZDOTDIR="$ZDIR" zsh -ic 'echo seasalt-smoke-cmd' >/dev/null
OUT=$("$BIN" search --all --tsv 'seasalt-smoke-cmd')
grep -q $'\tzsh' <<<"$OUT" || { echo "FAIL: recorded row lacks shell=zsh:"; echo "$OUT"; exit 1; }

echo "OK"
```

`chmod +x tests/zsh/smoke.sh`

- [ ] **Step 2: run** — `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt`（zsh 導入時 `OK` 期待）
- [ ] **Step 3: コミット** — `git add tests/zsh/smoke.sh && git commit -m "test(zsh): add runtime smoke test"`

---

### Task 6: ユーザー向けドキュメント（CHANGELOG / README）

**Files:**
- Modify: `CHANGELOG.md`, `README.md`

- [ ] **Step 1: `CHANGELOG.md`** の先頭に追加

```
## [Unreleased]

- zsh support: `seasalt init zsh` installs preexec/precmd history hooks and a
  zsh-autosuggestions strategy for inline suggestions (requires zsh-autosuggestions;
  zsh >= 5.0.8). History stays unified; records are tagged with their shell
  (`shell` column, exposed as the last column of `seasalt search --tsv`).
```

- [ ] **Step 2: `README.md`** — 対応シェルを bash + zsh に更新。zsh セクション: `eval "$(seasalt init zsh)"`、zsh-autosuggestions を先に source（`init zsh` をなるべく最後に置く。ストラテジ配列の上書き防止）、zsh ≥5.0.8、履歴はシェル間で統一。
- [ ] **Step 3: コミット** — `git add CHANGELOG.md README.md && git commit -m "docs: zsh changelog and readme"`

---

### Task 7: 全体検証

**Files:** 変更なし。

- [ ] **Step 1: pre-commit gate** — `cargo fmt && cargo check && cargo clippy && cargo test`（全て pass）
- [ ] **Step 2: 両 smoke** — `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt && bash tests/zsh/smoke.sh target/release/seasalt`（両方 `OK`）
- [ ] **Step 3: `init` が DB/data dir に触れないことを確認**（既存 cli_test で担保）