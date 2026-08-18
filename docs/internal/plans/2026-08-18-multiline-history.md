# 複数行コマンド対応 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 複数行コマンドの履歴動作(記録・dedup・suggest)をテストで固定し、先頭空白ガードをスニペットとバイナリで統一、`search` 出力の cmd フィールドをエスケープして 1 行 = 1 エントリを維持する。

**Architecture:** 既存の main.rs への局所修正のみ。記録・suggest 経路は変更しない(動作は既に正しい)。ガード判定とエスケープヘルパーは main.rs 内で完結させる。テストは cli_test と smoke.sh に追加。

**Tech Stack:** Rust (edition 2021), rusqlite, clap, anyhow; bash smoke test

**Spec:** docs/internal/specs/2026-08-18-multiline-history-design.md(本プランの要求の権威。実装者は spec とこのプランを両方読む)

## Global Constraints

- 各タスク完了時: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test` をこの順で実行し、全て green
- Task 3 は追加で `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK` であること
- コメントは英語、テスト fixture も英語。設計 spec・本プランは日本語(既存文書)
- `suggest` 出力はエスケープしない。エスケープは `search` / `search --tsv` の cmd フィールドのみ
- スコープ外: DB スキーマ・suggest ロジック・bash スニペットの変更、`\r` / cwd フィールドのエスケープ、ble.sh ゴースト描画の修正(実機確認のみ)
- コミットは各タスクでユーザー事前承認を得ること

---

### Task 1: 設計・計画文書

**Files:**
- Add: `docs/internal/specs/2026-08-18-multiline-history-design.md`(済)
- Add: `docs/internal/plans/2026-08-18-multiline-history.md`(本ファイル)

- [ ] **Step 1: 文書の見直し** — プレースホルダ(placeholder)・矛盾・曖昧さがないか spec と本プランを確認
- [ ] **Step 2: コミット** — `git add docs/internal/specs/2026-08-18-multiline-history-design.md docs/internal/plans/2026-08-18-multiline-history.md && git commit -m "docs: add multi-line history design and plan"`(要承認)

---

### Task 2: Rust 実装(ガード統一 + search 出力エスケープ)

**Files:**
- Modify: `src/main.rs`
- Test: `tests/cli_test.rs`(新規テスト 5 件追加)

**Consumes:** なし(既存構造のみ)

- [ ] **Step 1: 失敗するテストを追加** — `tests/cli_test.rs` 末尾に追加:

```rust
const MULTI_LINE: &str = "echo multi\n  echo line2";

#[test]
fn record_and_search_multiline_command() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", MULTI_LINE])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "echo multi"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // The embedded newline is escaped, so the entry stays one physical line
    assert_eq!(text.lines().count(), 1);
    let fields: Vec<&str> = text.trim().split('\t').collect();
    assert_eq!(fields[2], "echo multi\\n  echo line2");
    assert_eq!(fields[3], ""); // exit_code is NULL (no exit call)
}

#[test]
fn suggest_matches_multiline_prefix() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", MULTI_LINE])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/x", "--", "echo multi"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), MULTI_LINE);
}

#[test]
fn record_multiline_dedups_to_single_row() {
    let dir = temp_data_dir();
    for _ in 0..2 {
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", "/x", "--session", "s1", "--", MULTI_LINE])
            .status()
            .unwrap();
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "echo multi"])
        .output()
        .unwrap();
    assert_eq!(out.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
}

#[test]
fn record_ignores_leading_newline_command() {
    // A command starting with a newline is skipped, matching the
    // snippet's [[:space:]] guard (spec §4.1).
    let dir = temp_data_dir();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "\nsecret-thing"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "secret-thing"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
}

#[test]
fn search_escapes_control_characters_in_cmd() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "echo a\\tb\\c"])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "echo a"])
        .output()
        .unwrap();
    // Backslash and tab appear escaped; the entry is one line
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("echo a\\\\tb\\\\c"));
}
```

- [ ] **Step 2: 失敗確認** — Run: `cargo test --test cli_test multiline_` と `cargo test --test cli_test search_escapes_control_characters_in_cmd`
  Expected: FAIL(エスケープ未実装のため出力が複数行になり assert が失敗。`record_ignores_leading_newline_command` は既存挙動でもパスする)

- [ ] **Step 3: 実装** — `src/main.rs` に 2 点:

ガード(main.rs:106):

```rust
            if cmd.chars().next().is_some_and(char::is_whitespace) {
                return Ok(());
            }
```

`now_ms` の近くにヘルパー追加:

```rust
/// Escapes backslash, newline and tab in a command so search output
/// stays one line per entry (multi-line commands are stored raw).
/// suggest output is not escaped: it is the raw command for acceptance.
fn escape_cmd(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}
```

`Command::Search` 分岐(main.rs:151-157)で `e.cmd` をエスケープしてから出力(エスケープ計算はループ内。`entries` はたかだか limit 件で、`items` に一時保持する必要なし):

```rust
            for e in entries {
                let cmd = escape_cmd(&e.cmd);
                if tsv {
                    let code = e.exit_code.map(|c| c.to_string()).unwrap_or_default();
                    println!("{}\t{}\t{}\t{}\t{}", e.id, e.cwd, cmd, code, e.started_at);
                } else {
                    println!("{}\t{}", e.id, cmd);
                }
            }
```

- [ ] **Step 4: テスト確認** — Run: `cargo test --test cli_test`(新規 5 件 + 既存全て green。既存テストの cmd は `\`・改行・タブを含まないためエスケープの影響なし)
- [ ] **Step 5: ゲート確認** — Run: `cargo fmt && cargo check && cargo clippy --all-targets && cargo test`
  Expected: 全て green
- [ ] **Step 6: コミット** — `git add src/main.rs tests/cli_test.rs && git commit -m "feat: escape control chars in search output and unify leading-whitespace guard"`(要承認)

---

### Task 3: smoke.sh に複数行の節を追加

**Files:**
- Modify: `tests/bash/smoke.sh`

**Consumes:** Task 2 のバイナリ(エスケープ済み search 出力)

- [ ] **Step 1: 節を追加** — `run_suite` 内、**末尾**(clear の節の後、`}` の前)に追加:

```bash
  # Multi-line commands are recorded, deduped, searched and suggested
  _seasalt_preexec $'echo multi\n  echo line2'
  [[ $_seasalt_last_id =~ ^[0-9]+$ ]] || fail "multi-line record failed: $_seasalt_last_id"
  _seasalt_precmd
  local mlout msugg
  mlout=$("$BIN" search --tsv "echo multi")
  [[ $(printf '%s\n' "$mlout" | wc -l) -eq 1 ]] || fail "multi-line entry spans rows: $mlout"
  [[ $mlout == *'echo multi\n  echo line2'* ]] || fail "multi-line cmd not escaped in search: $mlout"
  _seasalt_preexec $'echo multi\n  echo line2'
  _seasalt_precmd
  rows=$("$BIN" search --tsv "echo multi" | wc -l)
  [[ $rows -eq 1 ]] || fail "multi-line dedup failed: $rows"
  msugg=$("$BIN" suggest --cwd "$PWD" -- 'echo multi')
  [[ $msugg == $'echo multi\n  echo line2' ]] || fail "multi-line suggest mismatch: $msugg"
  _seasalt_preexec $'\nsecret-multi'
  [[ -z $_seasalt_last_id ]] || fail "leading-newline command recorded: $_seasalt_last_id"
```

- [ ] **Step 2: 検証** — Run: `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
  Expected: `OK`
- [ ] **Step 3: コミット** — `git add tests/bash/smoke.sh && git commit -m "test: cover multi-line history in bash smoke"`(要承認)

---

### Task 4: ドキュメント反映

**Files:**
- Modify: `docs/internal/specs/2026-08-15-seasalt-design.md`(日本語)
- Modify: `README.md`(英語)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: 設計仕様更新** — `docs/internal/specs/2026-08-15-seasalt-design.md`:
  - §4 `seasalt record` の guard の記述を「先頭が空白 (スペースまたはタブ)」→「先頭が空白文字 (スペース・タブ・改行など)」に修正(スニペット側は `[[:space:]]*`、バイナリ側は `char::is_whitespace()`)
  - §4 `seasalt search` の出力仕様に「cmd 内の `\`・改行・タブは `\\`・`\n`・`\t` にエスケープされ、1 行 = 1 エントリが維持される」を追記
- [ ] **Step 2: README 更新** — `seasalt search ...` の Usage 記述(147-151 行付近)にエスケープ仕様を追記。Features に複数行コマンドのサポートを 1 行追加
- [ ] **Step 3: CHANGELOG 更新** — 先頭に `## [Unreleased]` 節を追加:

```markdown
## [Unreleased]

- Multi-line commands are recorded verbatim and suggested by their first
  line; `seasalt search` escapes embedded newlines/tabs/backslashes in
  its output so every entry stays on a single line
- Commands starting with any whitespace (including a leading newline)
  are never recorded (unified with the shell-side guard)
```

- [ ] **Step 4: 検証** — Run: `git diff --stat` で対象ファイルのみ変更されていることを確認。コード変更なし
- [ ] **Step 5: コミット** — `git add docs/internal/specs/2026-08-15-seasalt-design.md README.md CHANGELOG.md && git commit -m "docs: document multi-line history support"`(要承認)

---

### 最終検証(全タスク完了後)

- [ ] `cargo fmt --check && cargo check && cargo clippy --all-targets && cargo test` が全て green
- [ ] `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt` が `OK`
- [ ] `git status` が clean、`git log --oneline -5` で 4 コミットを確認
- [ ] 手動確認: 実機 ble.sh で複数行サジェストのゴースト描画を確認する(コード変更なし。表示崩れがあれば別タスク)