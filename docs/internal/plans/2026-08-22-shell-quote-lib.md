# シェルクォートのライブラリ化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `src/paths.rs:59-110` の手書き `tokenize`/`unquote` を `shlex` に置換し、17 行の場当たりを削除して POSIX 準拠の信頼性を得る。

**Architecture:** `Cargo.toml:9-14` に `shlex = "1.3"` を 1 行追加し、`src/paths.rs:13-34` の `required_paths` 内で `tokenize(cmd)` → `shlex::split(&cmd).unwrap_or_default()` に置換。`unquote` 関数は削除し、`is_operator` / `contains("$(")` / `resolve` フィルタはそのまま残す。DB スキーマ・マイグレーションなし。

**Tech Stack:** Rust 2021 / `shlex` 1.3（POSIX shell split）/ `rusqlite` 0.37 / `clap` 4

**Spec:** 本 plan と `AGENTS.md:31-32`（既存の再実装より保守されたライブラリを優先、ドキュメント確認後に追加）が権威。`docs/internal/specs/2026-08-15-seasalt-design.md` §6 の `paths` フィルタ仕様を維持。

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` をこの順で実行し、全て green
- コメントは英語、テストも英語。plan は日本語
- Silence contract 維持: `record`/`exit`/`suggest` は stderr に書かない
- `init` は DB/data dir に触れない
- スコープ外: `src/db.rs` の TOCTOU、`flake.nix`、`bench` の閾値変更、バージョン bump
- コミットは各タスクで事前承認

---

### Task 1: 検証 — `shlex` が既存 fixture を満たすか

**Files:**
- 一時検証のみ（コミットなし）。`tests/paths_test.rs:11-128` の 10 ケースと `tests/suggest_test.rs:27-450` の stale-file ケースを `shlex` で再現

**Interfaces:**
- Consumes: `shlex::split(&str) -> Option<Vec<String>>`
- Produces: 判定 — `shlex` が現行より正確か同等

- [ ] **Step 1: 一時 Rust スニペットで `shlex` の挙動を確認**

```rust
use shlex::split;
for cmd in [
    r#"nvim "my file.txt""#,
    r"nvim esc\.txt",
    r#"nvim "a\"b.txt""#,
    r"nvim 'a\'b.txt'",
    "git push origin main && echo hi > out.txt",
    "nvim $(date).txt",
] {
    println!("{cmd:?} -> {:?}", split(cmd));
}
```

期待:
- `"my file.txt"` → `["nvim", "my file.txt"]`（現行と同じ）
- `esc\.txt` → `["nvim", "esc.txt"]`（現行は `esc.txt`、一致）
- `"a\"b.txt"` → `["nvim", "a\"b.txt"]`（現行の 17 行 `unquote` と同じ `a"b.txt` に到達）
- `'a\'b.txt'` → `None` または `["nvim", "a'b.txt"]`（`shlex` は `'` 内の `\'` を POSIX では扱わないが、`cargo test` で要確認）
- `git push ... && ...` → `["git", "push", "origin", "main", "&&", "echo", "hi", ">", "out.txt"]`（`is_operator` で `&&`, `>` を除外できる）
- `$(date).txt` → `["nvim", "$(date).txt"]`（`contains("$(")` で除外できる）

- [ ] **Step 2: 不一致があれば `shell-words` でも同様に確認**

```rust
shell_words::split(cmd) // Result<Vec<String>, ParseError>
```

`shell-words` は `"` 内の `\` が `$`, `` ` ``, `"`, `\`, 改行 の前だけ特殊という POSIX 厳密な挙動。どちらが既存テストに近いかを比較し、近い方を採用。差がなければ `shlex`（DL 数・更新頻度で優位）を採用。

- [ ] **Step 3: 結論を記録**

本 plan 内に「採用: `shlex` 1.3」の 1 行を残す。検証で両方とも既存 fixture を満たさない場合は、本 plan 自体を中断し `unquote` の 17 行を維持する判断をユーザーに提示。

---

### Task 2: 置換 — `src/paths.rs` の `tokenize`/`unquote` を `shlex` に

**Files:**
- Modify: `Cargo.toml:9-14` — 依存追加
- Modify: `src/paths.rs:1-110` — `tokenize`/`unquote` を削除し `shlex::split` に
- Modify: `CHANGELOG.md:5` — Unreleased に 1 行
- Test: `tests/paths_test.rs:11-128`（既存 10 件がそのまま green になること）

**Interfaces:**
- Consumes: `shlex::split`
- Produces: `required_paths(cwd: &str, cmd: &str) -> Vec<String>` のシグネチャは不変、内部で `shlex::split` を使用

- [ ] **Step 1: `Cargo.toml` に依存追加**

```toml
[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
dirs = "6"
rusqlite = { version = "0.37", features = ["bundled", "hooks"] }
shlex = "1.3"
```

`Cargo.lock` は `cargo check` で自動更新。

- [ ] **Step 2: `src/paths.rs` を置換**

現行 `src/paths.rs:59-110` の `unquote`/`tokenize` 全体を削除し、`required_paths` を以下に:

```rust
use std::path::{Path, PathBuf};
use shlex;

const OPERATORS: &[&str] = &[
    "|", "||", "|&", "&&", "&", ";", ";;", "<", ">", ">>", "<<", "(", ")", "{", "}", "!", "$",
];

pub fn required_paths(cwd: &str, cmd: &str) -> Vec<String> {
    let tokens = shlex::split(cmd).unwrap_or_default();
    let mut out = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i == 0 {
            continue;
        }
        if tok.is_empty() || tok.starts_with('-') || is_operator(tok) {
            continue;
        }
        if tok.contains("$(") || tok.contains('`') {
            continue;
        }
        // shlex already did quote removal and unescaping, so tok is the path itself
        let path = tok.as_str();
        if path.is_empty() || matches!(path, "." | ".." | "./" | "../") {
            continue;
        }
        if resolve(cwd, path).exists() {
            out.push(path.to_string());
        }
    }
    out
}
```

`valid`/`resolve`/`is_operator` はそのまま。`unquote`/`tokenize` は削除。`shlex` が `None`（未閉じクォート・末尾 `\`）を返した場合は `unwrap_or_default()` で空ベクタ → `required_paths` は空を返し、候補は stale-file フィルタをスキップせずに表示される（現行の不正入力時の挙動と同等で安全）。

- [ ] **Step 3: `CHANGELOG.md` 追記**

`## [Unreleased]` に:

```md
- Replace hand-written shell quoting in `paths` with `shlex` for POSIX correctness
```

- [ ] **Step 4: ゲート確認**

Run: `cargo fmt && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: 全て green。特に `tests/paths_test.rs` 10 件、`tests/suggest_test.rs` 29 件、`tests/db_test.rs` 29 件が PASS。

Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt && bash tests/zsh/smoke.sh target/release/seasalt`
Expected: 両方 `OK`（`paths` 変更は `suggest` の stale-file 節で検証）

- [ ] **Step 5: コミット（要承認）**

```bash
git add Cargo.toml Cargo.lock src/paths.rs CHANGELOG.md
git commit -m "refactor(paths): use shlex for shell quoting"
```

---

### 最終検証（全タスク完了後）

- [ ] `cargo fmt --check && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo test` が green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt` が `OK`
- [ ] `git log --oneline -2` で Task 1 検証メモと Task 2 置換の 2 コミット（または Task 1 はコミットなしで Task 2 の 1 コミット）を確認
- [ ] `git diff  e59fdeb..HEAD --stat` が `Cargo.toml`, `Cargo.lock`, `src/paths.rs`, `CHANGELOG.md` のみに収まっていること
- [ ] `CHANGELOG.md` の `## [Unreleased]` に `shlex` 行が追記されていること

## Self-Review

- [x] Spec coverage: AGENTS.md の「ライブラリ優先・再実装しない」「既存依存をまず使う」を満たすため Task 1 で `shlex`/`shell-words` のドキュメント確認を明示。Task 2 で置換を完結
- [x] Placeholder scan: `TBD`/`TODO` なし。全 step に具体的なコードとコマンドを記載
- [x] Type consistency: `required_paths(cwd: &str, cmd: &str) -> Vec<String>` / `valid(cwd: &str, paths: &str) -> bool` のシグネチャは不変。`shlex::split` の `Option<Vec<String>>` を `unwrap_or_default` で吸収
