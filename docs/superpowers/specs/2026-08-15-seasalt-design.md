# seasalt 設計ドキュメント

- 日付: 2026-08-15
- ステータス: 承認済み(設計ブレスト完了)

## 1. 概要

bash 用のプラグイン。Rust 製のシングルバイナリ `seasalt` と ble.sh 統合層から構成され、以下を提供する:

- **fish 風のインライン自動補完**(ゴーストテキスト + 右矢印で確定)
- **フォルダ別の履歴管理**(現在ディレクトリとその親を優先するサジェスト)

bash 単体(readline)にはインライン補完の API が存在しないため、行編集層には ble.sh を利用する。ユーザー環境には ble.sh 0.4.0-devel3 が導入済み。

atuin(既に導入済み)とは独立したストアを持ち、完全共存する。atuin = 履歴検索・同期、seasalt = インライン補完 + フォルダ別履歴。

## 2. 制約と根拠

- bash 5.3 の readline にインライン補完 API は無い。外部プロセスから bash の行編集へ介入できない。
- ble.sh は行編集を置き換えており、以下が確認済みの正式な拡張ポイント:
  - `_ble_complete_auto_source=(history syntax)` 配列に自作ソースを追加できる
  - `ble/complete/auto-complete/source:<name>` 関数を定義し、`ble/complete/auto-complete/enter "$type" "$COMP1" "$suggest" ...` を呼ぶことで灰色サジェスト描画と → キー確定を実現できる
  - 返り値 `148` で「タイムアウト・後で再試行」のプロトコルがある
  - `blehook PREEXEC/PRECMD` でコマンド実行の前後フックが可能(既存の bash-preexec/atuin と干渉しない)
- fish 4.x にはフォルダ別コマンド履歴機能は存在しない(fish の directory history はディレクトリ移動履歴)。本設計は「fish らしい自然な履歴サジェスト」+「フォルダ重み付け」の解釈に基づく。

## 3. アーキテクチャ

3層構成:

```
┌─────────────────────────────────────────────┐
│ bash + ble.sh(行編集・描画・キー確定)        │
│   seasalt init bash が生成する統合スニペット │
├─────────────────────────────────────────────┤
│ seasalt バイナリ(サブコマンド方式)           │
│   record / suggest / search / init          │
├─────────────────────────────────────────────┤
│ SQLite(WAL) ~/.local/share/seasalt/         │
└─────────────────────────────────────────────┘
```

### プロセスモデル

SQLite 直接アクセス方式(デーモンなし)。`suggest` は呼ばれるたびに DB を開いて prefix 検索する。WAL モード + インデックスにより 1〜5ms の応答を想定。性能実測で問題が出た場合のみデーモン化を検討する。

### ストレージ

パス: `$XDG_DATA_HOME/seasalt/history.sqlite3`(デフォルト `~/.local/share/seasalt/`)

```sql
CREATE TABLE history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  cwd TEXT NOT NULL,
  cmd TEXT NOT NULL,
  exit_code INTEGER,
  started_at INTEGER NOT NULL,
  session TEXT
);
CREATE INDEX idx_history_cwd ON history(cwd);
CREATE INDEX idx_history_cmd ON history(cmd);
```

## 4. コンポーネント

### Rust コア(シングルバイナリ `seasalt`)

- `seasalt record --cwd DIR -- CMD`
  - preexec フックから呼ばれ履歴を insert(実行前時点のエントリ作成)
  - 新しい行の rowid を stdout に出力し、bash 側の変数に保持
- `seasalt exit --session SESSION --last-id N --code CODE`
  - precmd フックから呼ばれ、`(session, id)` で特定したエントリに exit_code を update
- `seasalt suggest --cwd DIR -- LINE`
  - フォルダスコープで prefix 一致の最良候補を stdout に出力
  - スコープ: cwd 完全一致 → 親ディレクトリ順(深さ優先で近い方から)→ グローバル
  - 各スコープ内は最新優先
  - 候補が無ければ何も出力しない
- `seasalt search [--all] PATTERN`
  - フォルダ絞り検索 CLI(フェーズ 1)
- `seasalt init bash`
  - 統合スニペット全文(関数定義 + フック登録)を stdout に出力し、`.bashrc` で eval する
  - スニペットはバイナリに埋め込む(`include_str!`)ので、nix ストアパスの変化に強い

### ble.sh 統合層

`seasalt init bash` の出力内容:

1. `ble/complete/auto-complete/source:seasalt` 関数の定義
   - `_ble_edit_str` と `$PWD` を `seasalt suggest` に渡す
   - 結果があれば `ble/complete/auto-complete/enter h 0 "$suggest" '' "$cmd"` を呼ぶ
2. `_ble_complete_auto_source` 配列への挿入(`seasalt history syntax` の順)
3. `blehook PREEXEC+=seasalt_record` による履歴記録
4. `blehook PRECMD+=seasalt_exit` による exit_code 更新

→ キーによる確定は ble.sh の auto_complete キーマップが標準搭載しており追加実装不要。

呼び出しは同期で開始する。応答遅延が体感に出た場合は ble.sh の bgproc(非同期)へ移行する。

## 5. データフロー

1. `.bashrc` に `eval "$(seasalt init bash)"` を追記
2. タイピング → ble.sh idle 処理 → `source:seasalt` → `seasalt suggest` → 灰色テキスト表示 → → キーで確定
3. コマンド実行 → `blehook PREEXEC` → `seasalt record`(WAL insert)→ 返った `(session, id)` を変数保持
4. プロンプト再表示 → `blehook PRECMD` → `seasalt exit` で exit_code update

## 6. サジェストのスコープ仕様(fish 風)

優先順位:

1. cwd 完全一致の履歴から prefix 一致の最新
2. 親ディレクトリを近い順に検索
3. グローバル全体から prefix 一致の最新

候補が無い場合は補完なし。ble.sh の既存 `history` / `syntax` ソースがそのままフォールバックとして機能する。

## 7. エラー処理

- seasalt バイナリ不在・DB が開けない → 補完を静かに無効化(ble.sh の補完を壊さない)
- DB 破損 → 退避(リネーム)して新規作成
- suggest タイムアウト → 補完なしで継続

## 8. テスト

- Rust ユニットテスト: suggest のスコープ優先度を in-memory SQLite で網羅
  - cwd 一致が親より優先されること
  - 親がグローバルより優先されること
  - 各スコープ内で最新が優先されること
  - 候補なしで空応答になること
- シェル側: ble.sh 非依存でロード可能な設計にし、関数単体の smoke test(bash のみで実行)
- 検証コマンド: `cargo fmt` → `cargo check` → `cargo clippy` → `cargo test`

## 9. マイルストーン

- **M1**: コア CLI(record / suggest / search)+ SQLite + ユニットテスト + flake.nix
- **M2**: ble.sh 統合(init bash / source:seasalt / blehook)+ 実機確認 + 性能計測
- **M3**: 検索 UI(Ctrl-R、fzf 連携か ratatui かはその時点で判断)
- **M4**: Tab 補完の fish 風化

## 10. 依存クレート

- `rusqlite`(bundled SQLite)
- `clap`(CLI パース)
- `dirs`(データディレクトリ解決)
- `anyhow`(エラー処理)
- TUI 関連は M3 で追加判断
