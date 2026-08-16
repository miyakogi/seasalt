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

SQLite 直接アクセス方式(デーモンなし)。`suggest` は呼ばれるたびに DB を開いて prefix 検索する。WAL モード + インデックスにより 1〜5ms の応答を想定。

**性能実測 (2026-08-16、benches/suggest.rs による Criterion 計測、中央値):**

| シナリオ | 1千行 | 1万行 | 10万行 | 100万行 |
|---|---|---|---|---|
| in-process hit | 0.15ms | 0.33ms | 2.74ms | 22.9ms |
| in-process miss(全スコープ空振り) | 0.25ms | 0.71ms | 7.11ms | 68.0ms |
| in-process miss_deep(親10段・24クエリ) | 0.38ms | 0.67ms | 4.59ms | 37.0ms |
| end-to-end hit(プロセス起動込み) | — | 1.22ms | 3.91ms | 25.2ms |
| end-to-end record(preexec フック) | — | — | 3.80ms | — |

**結論: デーモン化は不要と判断する。** 実用的な履歴規模(10万行 ≒ 数年分)では end-to-end で 4ms 未満、100万行の最悪ケースでも 68ms で ble.sh 側の 200ms timeout に大きな余裕がある。100万行を超える運用になった場合はグローバル空振り時のソート込みスキャンが支配的になるため、その時点でインデックス/クエリ最適化を検討する(現時点では YAGNI)。

### ストレージ

パス: `$XDG_DATA_HOME/seasalt/history.sqlite3`(デフォルト `~/.local/share/seasalt/`)

```sql
CREATE TABLE history (
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
CREATE INDEX idx_history_cwd_cmd ON history(cwd, cmd);
CREATE INDEX idx_history_started_at ON history(started_at);
```

`paths` には record 時点で実在したファイル引数のパスを NUL 区切りで保存する(詳細は §6)。`(cwd, cmd)` の複合インデックスは record 時の重複判定に使う。旧スキーマ(`paths` 列なし)の DB は初回接続時に `ALTER TABLE ... ADD COLUMN` で自動マイグレーションされる。

履歴は件数上限 (デフォルト 100,000 件、`SEASALT_HISTORY_MAX` で変更、`0` で無効化) を持ち、record のたびに `started_at` が古い行から上限を超える分を自動削除する。dedup で更新された行は最新扱いになるため保護される。`idx_history_started_at` はこのトリムに使う。削除コストは実測済み (100k 行で warm ~0.2ms / cold ~2.1ms、設計ドキュメント 2026-08-16-history-limit-design.md §2 参照)。

新規作成時のみ、データディレクトリは 0700、履歴ファイルは 0600 に設定する(既存のディレクトリ・ファイルのパーミッションは変更しない)。履歴に機微な情報が入り得るため。WAL モードの `-wal`/`-shm` ファイルはディレクトリの 0700 により保護される。

## 4. コンポーネント

### Rust コア(シングルバイナリ `seasalt`)

- `seasalt record --cwd DIR -- CMD`
  - preexec フックから呼ばれ履歴を記録する (実行前時点のエントリ作成)
  - 先頭が空白 (スペースまたはタブ) のコマンドは記録しない (bash の `HISTCONTROL=ignorespace` と同じ。パスワードなどのセンシティブな入力を誤って保存しないため)。スニペットの `_seasalt_preexec` と record 本体の両方でガードする
  - 環境変数 `SEASALT_PRIVATE_MODE` が非空の間は記録しない (fish の `$fish_private_mode` 相当。既存履歴とサジェストには影響しない。スニペットの `_seasalt_preexec` のみでガードする)
  - 同一 (cwd, cmd) の既存行があれば新規行を作らず、その行を最新化する (started_at 更新・paths 置換・exit_code リセット)。fish と同様、重複コマンドは履歴に 1 行しか残らない
  - 引数のうち記録時点で実在したファイルパスのみを `paths` に保存する (存在しなかった引数は制約にならない)
  - 行 id を stdout に出力し、bash 側の変数に保持
- `seasalt exit --last-id N --code CODE`
  - precmd フックから呼ばれ、行 id で特定したエントリに exit_code を update する (session は照合に使わない: dedup で行が他セッションの実行に書き換わり得るため)
- `seasalt suggest --cwd DIR -- LINE`
  - フォルダスコープで prefix 一致の最良候補を stdout に出力
  - スコープ: cwd 完全一致 → 親ディレクトリ順(深さ優先で近い方から)→ グローバル
  - 各スコープ内は最新優先
  - 各スコープ内では、大文字小文字が完全一致する prefix 候補を優先し、無ければ case-insensitive の最新候補にフォールバックする (fish の autosuggestion と同じ。sensitive 検索は SQLite の GLOB を使う)
  - 表示のケースについて: タイプ中の表示は入力テキストのケースのまま (例: `cd pic` + ゴースト `tures`) で、確定時のみ `cd Pictures` になる。fish は「最後のトークンに大文字が無ければ表示だけ候補のケースに合わせる」(combine_command_and_autosuggestion) が、ble.sh の auto-complete は実バッファ + ゴーストの合成描画のため表示だけの調整ができない。バッファ自体を候補のケースで書き換える案 (ble-edit/content/replace) は undo・カーソル操作への副作用があるため採用しない (表示の差異のみで確定結果は同じ)
  - 保存済み `paths` が suggest 時点の cwd 基準で存在しなくなった候補はスキップし、次の候補へフォールバックする(全候補が消滅していれば何も出力しない)
  - 候補が無ければ何も出力しない
- `seasalt search [--all] PATTERN`
  - フォルダ絞り検索 CLI(フェーズ 1)
  - デフォルト出力は `id<TAB>cmd`、`--tsv` は id, cwd, cmd, exit_code, started_at(行削除のために id を常に表示する)
  - パターンは SQL LIKE の部分一致で、`%` と `_` はワイルドカード
- `seasalt delete ID...`
  - 指定した行 id の履歴を削除する。存在しない id は静かに無視し、成功時は何も出力しない
  - 誤ってパスワードなどを記録してしまった行の削除に使う
- `seasalt clear`
  - 履歴を全件削除し、`VACUUM` でファイル領域を回収する (fish の `history clear` 相当)
  - 成功時は何も出力しない。interactive コマンド (エラーは stderr)
- `seasalt init bash`
  - 統合スニペット全文(関数定義 + フック登録)を stdout に出力し、`.bashrc` で eval する
  - スニペットはバイナリに埋め込む(`include_str!`)ので、nix ストアパスの変化に強い

### ble.sh 統合層

`seasalt init bash` の出力内容:

1. `ble/complete/auto-complete/source:seasalt` 関数の定義
   - `_ble_edit_str` と `$PWD` を `seasalt suggest` に渡す
   - 呼び出しは同期で、`timeout 0.2` で 200ms を超えたら補完なしで継続する (`timeout` は GNU coreutils 由来。macOS では coreutils の導入が必要)
   - 結果があれば `ble/complete/auto-complete/enter h 0 "$suggest" '' "$cmd"` を呼ぶ
   - 非同期化 (ble.sh の bgproc / バックグラウンドサブシェル) は調査済み: いずれも bash 5.3 のジョブ表との相互作用で `[1] <pid>` のジョブ通知が表示される。ble.sh 側の修正待ちのため同期版を維持する
2. `_ble_complete_auto_source` 配列の再整列(`seasalt syntax` の順)
   - ble.sh は core-complete を遅延ロードし、その際に配列を `(history syntax)` へ無条件リセットする。統合スニペットは idle タスクで「seasalt を先頭に置き、`atuin-history` と `history` を除去する」再整列を実行する
   - インライン提案は seasalt のみが担当し、削除済みファイルを参照するコマンドが他ソースから提案されるのを防ぐ(Ctrl-R の履歴検索には影響しない)
3. `blehook PREEXEC+=seasalt_record` による履歴記録
4. `blehook PRECMD+=seasalt_exit` による exit_code 更新

→ キーによる確定は ble.sh の auto_complete キーマップが標準搭載しており追加実装不要。

スニペットは quoted eval (`eval "$(seasalt init bash)"`) を前提とする。unquoted の `eval $(...)` は word-split により壊れて構文エラーになる(意図的)。

## 5. データフロー

1. `.bashrc` に `eval "$(seasalt init bash)"` を追記
2. タイピング → ble.sh idle 処理 → `source:seasalt` → `seasalt suggest` → 灰色テキスト表示 → → キーで確定
3. コマンド実行 → `blehook PREEXEC` → `seasalt record`(WAL insert)→ 返った行 id を変数保持
4. プロンプト再表示 → `blehook PRECMD` → `seasalt exit` で exit_code update

## 6. サジェストのスコープ仕様(fish 風)

優先順位:

1. cwd 完全一致の履歴から prefix 一致の最新
2. 親ディレクトリを近い順に検索
3. グローバル全体から prefix 一致の最新

候補が無い場合は補完なし。インライン提案は seasalt のみが担当する(`history` / `atuin-history` ソースは統合スニペットが除外するため、フォールバックは存在しない)。

### 削除済みファイルのフィルタリング(fish と同様のセマンティクス)

- record 時に、引数のうち実在したファイルパスのみを `paths` に保存する
- suggest 時に、保存されたパスが「suggest 時点の cwd」基準で存在しなければ、その候補をスキップして次の候補へフォールバックする
- 記録時点で存在しなかった引数(`echo hello` の `hello`、`git push` の `push` など)は制約にならない
- 相対パスは record 時の cwd ではなく suggest 時点の cwd 基準で判定し、絶対パスはそのまま判定する

### 履歴の重複除去 (fish パリティ)

- record 時に同一 (cwd, cmd) の既存行があれば、新規 insert せずに行を最新化する (started_at 更新・paths 置換・exit_code リセット)
- 同一コマンドは連続・非連続を問わず 1 行しか残らない (fish の "Any duplicate history items are automatically removed" に相当。fish はコマンド文字列のみで判定するが、seasalt はディレクトリ別スコープが本体のため (cwd, cmd) をキーにする)
- 既に溜まっている旧データの重複行は放置する (新規 record からのみ dedup が効く)
- トレードオフ: 中間実行の時刻・exit code は残らない (最後の実行分のみ)

## 7. エラー処理

- seasalt バイナリ不在・DB が開けない → 補完を静かに無効化(ble.sh の補完を壊さない)
- DB 破損 → 自動リカバリは行わない(退避リネームや新規作成の実装予定なし)。DB が開けない場合はエラー終了し、フック側(record/exit/suggest)では静かに無効化される
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
- **M3**: 検索 UI(Ctrl-R) — **見送り**(2026-08-16 決定)。Ctrl-R 履歴検索は atuin がカバーしており共存できるため(§1 の住み分け方針)。seasalt はインライン補完と CLI 検索(`seasalt search`)に集中する
- **M4**: Tab 補完の fish 風化

## 10. 依存クレート

- `rusqlite`(bundled SQLite)
- `clap`(CLI パース)
- `dirs`(データディレクトリ解決)
- `anyhow`(エラー処理)
