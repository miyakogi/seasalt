# zsh サポート 設計

- 日付: 2026-08-20
- ステータス: 承認済み（brainstorming での design 承認済み、実装プランは `docs/internal/plans/2026-08-20-zsh-support.md`）

## 目的

bash に加えて zsh を第一級サポートする。履歴はシェル間で **統一したまま**、各レコードにどのシェル由来かを `shell` 列でタグ付けする。インライン提案は **zsh-autosuggestions**（必須依存）のカスタムストラテジとして実装し、同ライブラリのネイティブ非同期を活用して UI をブロックしない。

## 背景と参考（類似プロジェクト）

- **atuin / mcfly** はどちらも**単一 DB を全シェルで統一共有**する。シェル分離はコミュニティの二番手要望（atuin issue #2540、フォーラムの "shell" filter mode 要望）で、タグ付け→後からフィルタという方向が望まれる。
- seasalt の既存 dedup `(cwd, cmd)` はシェル非依存。**統一履歴を維持**し、`shell` 列は情報用タグに留める（シェル毎のフィルタは将来機能）。

## 非同期の設計判断

- zsh-autosuggestions は `zle -F` + プロセス置換 `<(...)` による**ネイティブ非同期がある**。`ZSH_AUTOSUGGEST_USE_ASYNC` で制御し、**zsh ≥5.0.8 ではデフォルト ON**。
- ストラテジ関数 `_zsh_autosuggest_strategy_seasalt` は**フォークした子プロセス内で実行**される。その中で `seasalt suggest` を（同期のまま）呼んでも**メインシェルはブロックされない**→ UI 非ブロッキングを実現。
- bash/ble.sh の「裸の `&` で `[1] pid` が漏れる」gotcha は bash 固有。zsh のプロセス置換は**ジョブ扱いにならず**、ジョブ終了通知を出さない（zsh ドキュメントの Jobs & Signals より）。**zsh では非同期が安全に使える**。
- バイナリの `suggest_budgeted`（200ms in-process timeout）の役割は、非同期下では「UI 凍結防止」ではなく**フォーク内サブプロセスの最長稼働を抑える堅牢性の保険**。変更不要で残す。
- 前提: **zsh ≥ 5.0.8**（未満では非同期が無効になり、Ctrl+C の既知バグ #364 の余地があるため）。明示的な強制はせず既定に任せる。

## DB 設計

`history` に `shell TEXT NOT NULL DEFAULT 'bash'` を追加（**migration v3**）。既存レコードは `bash` になる（bash のみで記録されてきたため正しい）。

- `record_history(conn, cwd, cmd, started_at, session, paths, shell)` に引数追加。
- upsert `ON CONFLICT(cwd, cmd) DO UPDATE SET shell = excluded.shell`（再実行時は最新のシェルをタグ）。
- dedup `(cwd, cmd)` のまま＝シェル非依存の統一履歴。シェル毎のユニーク化はしない。

## CLI

- `record` に `--shell`（既定 `bash`）。bash スニペットは無変更（既定値依存）、zsh スニペットは `--shell zsh` を渡す。
- `init` に `zsh` 分岐 → `integration::zsh_init_script()`（`include_str!("zsh/seasalt.zsh")`）。
- `search --tsv` の出力に `shell` を**末尾の 6 列目**として追記（タグの観測・テストが可能に）。既存の前列（id/cwd/cmd/exit_code/started_at）は不変。
- `about` を "for bash and zsh" に。
- 制約: **init は DB / data dir に触れてはいけない**。スニペットは定義のみで起動時に DB を開かない。

## zsh スニペット（src/zsh/seasalt.zsh）

- `_seasalt_bin` は**トップレベル（非 local）**で設定（非同期のフォーク内サブシェルから参照するため）。
- フックは `preexec_functions` / `precmd_functions` の**先頭へ直接挿入**（`add-zsh-hook` は末尾追加のため使わない前提で prepend。`autoload` で `add-zsh-hook` を呼び機構を作ってから配列先頭へ挿入）。理由: **zsh は precmd フックを先頭から順に呼び、各フックは直前フックの戻り値を `$?` で見る**。`_seasalt_precmd` が最初でないと終了コードを正しく捕捉できない（実バグ）。
- `_seasalt_preexec`: `$2`= 完全コマンドライン（改行込み対応）。空白始まり・`SEASALT_PRIVATE_MODE` 時はスキップし、`record --cwd "$PWD" --session ... --shell zsh -- "$2"`。
- `_seasalt_precmd`: 先頭で `local code=$?` を捕捉し、`exit --last-id ... --code ...`。
- 提案ストラテジ:
  ```
  _zsh_autosuggest_strategy_seasalt() {
    emulate -L zsh; typeset -g suggestion
    suggestion=$("$_seasalt_bin" suggest --cwd "$PWD" -- "$1" 2>/dev/null) || suggestion=
  }
  ```
- `ZSH_AUTOSUGGEST_STRATEGY` は**置換せず先頭へ挿入**（既存ストラテジを保持、空 DB 時は既定 `history` にフォールバック）:
  `ZSH_AUTOSUGGEST_STRATEGY=(seasalt ${ZSH_AUTOSUGGEST_STRATEGY[@]:-history})`
- zsh-autosuggestions 未ロード時は stderr に警告（load order は README へ）。

## 検証

- 単体/CLI: db_test（shell 引数・conflict 更新・v3 migration）、cli_test（`init zsh` 内容、`--shell`）。
- ランタイム smoke: `tests/zsh/smoke.sh`（release build）。zle シミュレーションは flake 回避で対象外とし、(a) フック登録、(b) record/exit が shell=zsh で記録される、(c) ストラテジ関数を直接呼び `$suggestion` を検証、に限定。

## スコープ外

- fish / nushell / powershell 対応
- `suggest` の shell フィルタリング（タグ活用は将来機能）
- シェル別 DB 分離