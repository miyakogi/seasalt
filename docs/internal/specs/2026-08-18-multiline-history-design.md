# seasalt 複数行コマンド対応 設計

- 日付: 2026-08-18
- ステータス: 承認済み(設計ブレスト完了)

## 1. 概要

bash + ble.sh では複数行コマンド(`for` ループやヘレドキュメント、`$'...'` 区切りの
入力)を1つのコマンドとして記録・サジェストできる。本設計はこの挙動を**明示的な仕様と
して固定し、テストで保護する**こと、および現状で放置されている **2 つの穴**を塞ぐことを
目的とする。

- **先頭空白ガードの不統一** — スニペットは `[[ $cmd == [[:space:]]* ]]`(改行も含む)で
  ガードするが、バイナリ側 `record` は先頭 space/tab のみスキップする。仕様 §4 の
  「スニペットと record 本体の両方でガードする」原則から外れている
- **`search` 出力の行構造破壊** — cmd に埋め込まれた改行がそのまま出力され、
  `1 行 = 1 エントリ`の契約が崩れ、`delete` の id 抽出やスクリプト解析を壊す

## 2. 現状の確認(設計時点の挙動)

複数行コマンドはデータ経路の大部分で**すでに正しく動作している**:

1. **記録**: スニペットは `"$_seasalt_bin" record --cwd "$PWD" --session ... -- "$cmd"`
   と**単一の argv 要素**でコマンド全文を渡す(seasalt.bash:16)。`main.rs` の
   `cmd.join(" ")` は単一要素に対して何もしないため、改行はそのまま DB へ保存される
2. **dedup**: キーは `(cwd, cmd)` なので、改行を含む全文をキーに重複除去が働く
3. **suggest**: needle が改行を含んでいても、GLOB の `*` と LIKE の `%` は改行を跨いで
   マッチする。前置一致のため「1行目まで打った時点」で複数行コマンド全体が提案される
4. **paths**: `tokenize` は `char::is_whitespace()` で分割する(paths.rs:97)ため、
   改行を含むコマンドも正しくトークナイズできる

本設計はこの動作を変更しない。**記録・suggest の語彙は生のコマンド文字列**であり、
表示用の変形は行わない。

## 3. 要件

1. 複数行コマンドは改行を保ったまま履歴に記録され、前置一致でサジェストされる(動作は
   §2 のとおり。テストで保護する)
2. 先頭が空白文字(スペース・タブ・改行を含む)のコマンドは、スニペットとバイナリの
   両方で記録されない(ガードを統一する)
3. `seasalt search` / `search --tsv` の出力では、cmd 内の `\`・改行・タブを
   `\\`・`\n`・`\t` のリテラル文字列にエスケープし、**1 行 = 1 エントリ**を維持する
4. `seasalt suggest` の出力は**エスケープしない**(受け入れ用の生コマンドのため)

## 4. 実装

### 4.1 先頭空白ガードの統一 (src/main.rs)

現状(main.rs:106):

```rust
if cmd.starts_with(' ') || cmd.starts_with('\t') {
    return Ok(());
}
```

スニペット(seasalt.bash:13)の `[[ $cmd == [[:space:]]* ]]` に合わせて、先頭が任意の
空白文字ならスキップする:

```rust
if cmd.chars().next().is_some_and(char::is_whitespace) {
    return Ok(());
}
```

`char::is_whitespace()` はスペース・タブ・改行・CR などを含むため、POSIX の
`[[:space:]]` と同じ範囲をカバーする。

### 4.2 search 出力のエスケープ (src/main.rs)

`Command::Search` 分岐で、cmd のみエスケープしてから出力する。ヘルパー:

```rust
/// Escapes backslash, newline and tab in a command so search output
/// stays one line per entry (multi-line commands are stored raw).
fn escape_cmd(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}
```

- 適用順は `.replace` の連結順に等しい(バックスラッシュを先に置換してから `\n`/`\t` を
  合成するため、二重エスケープは発生しない)
- 既定出力(`id\tcmd`)と `--tsv`(5 列)の両方に適用する。適用対象は cmd フィールドのみ
  (cwd 等の他フィールドは対象外。パスに制御文字が入るケースは現実的でないため YAGNI)
- `suggest` 出力には適用しない(§3.4)

## 5. テスト

- **cli_test**:
  - 複数行 cmd を record → `search --all --tsv` が 1 物理行になり、cmd に `\n` リテラル
    が現れる
  - `suggest` が「1行目までの prefix」から複数行コマンド全体を返す
  - 同一複数行 cmd の再実行で 1 行のまま(dedup)
  - 先頭が `\n` の cmd は記録されない(ガード統一)
  - タブ・バックスラッシュ入り cmd の search 出力がエスケープされる
- **smoke.sh**: 複数行 record → search 1 行・suggest 一致・dedup・先頭 `\n` 不記録の節を
  追加(snippet 変更なし)
- **db_test**: 複数行 cmd の dedup は cli_test で網羅できるため追加しない

## 6. スコープ外

- DB スキーマ変更、suggest ロジック変更、スニペット変更(いずれも不要)
- `\r`・cwd フィールドのエスケープ、fish との挙動差の完全一致化
- ble.sh の複数行ゴースト**描画**の修正(コード変更なし。実機で挙動を確認し、崩れていれば
  別タスクで相談)