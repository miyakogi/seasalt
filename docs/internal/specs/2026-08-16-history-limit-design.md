# seasalt 履歴肥大化対策 設計

- 日付: 2026-08-16
- ステータス: 承認済み(設計ブレスト完了)

## 1. 概要

履歴 DB (`history` テーブル) は件数上限を持たず無制限に肥大する。対策として以下を追加する:

1. **件数上限の自動トリム** — record 時に、上限を超えた分を古い順に自動削除 (fish の `history max` 相当)
2. **`seasalt clear` サブコマンド** — 全件削除 + VACUUM (fish の `history clear` 相当)

## 2. 要件

- 上限のデフォルトは **100,000 件** (性能実測に基づく: 10万行なら suggest の最悪ケースでも ~7ms で快適圏、100万行で劣化が始まるためその手前で止める)
- 環境変数 `SEASALT_HISTORY_MAX` で上書きできる: 未設定 → 100,000 / `N` → N 件 / `0` → 無制限 / パース失敗 → 100,000 (record は hook-facing で silent のため、エラーは出さずデフォルトにフォールバック)
- トリムは record のたびに実行する。コストは実測済み: 100k 行の DB で `OFFSET` サブクエリは warm ~0.2ms / cold ~2.1ms (カバリングインデックス `idx_history_started_at` を使用)。record (e2e 3.8ms) に比べ無視できるため間引きはしない (単純さ優先。当初検討した PID ベースの間引きは非決定的でテストが複雑になるため不採用)
- 残すのは `started_at` 降順 (新しい順) の上限件数。dedup で更新された行は started_at が新しくなるため保護される
- `seasalt clear`: 全件削除 + `VACUUM` (ファイル縮小)。interactive コマンド (エラーは stderr、成功時は無出力)

## 3. 実装

### 3.1 スキーマ変更 (src/db.rs SCHEMA)

`idx_history_started_at` を追加する。`CREATE INDEX IF NOT EXISTS` なので既存 DB は次回 open 時に自動適用され、マイグレーション (ALTER) は不要。

```sql
CREATE INDEX IF NOT EXISTS idx_history_started_at ON history(started_at);
```

### 3.2 トリム関数 (src/db.rs)

```rust
/// Deletes history rows older than the newest `max` rows (by
/// started_at, newest first). When the table has fewer than `max`
/// rows, the subquery yields NULL and nothing is deleted.
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

- `max = 0` (無制限) は呼び出し側で弾く
- 上限未満のとき `OFFSET ?1` が行を返さず NULL → `started_at < NULL` は常に false → 0 件削除
- `started_at` 同値の行は境界で残り得る (厳密に N 件より少し多くなる可能性があるが無害)

### 3.3 record 経路 (src/main.rs)

- `history_max() -> Option<usize>` ヘルパーを追加: env から上限を解決 (`None` = 無制限、`Some(n)` = n 件)
- `Command::Record` 分岐: `db::record_history` の後、`history_max()` が `Some(n)` なら `db::trim_history(&conn, n)?` を常に実行
- エラーは既存どおり silent

### 3.4 clear サブコマンド (src/main.rs / src/db.rs)

- `db.rs` に `pub fn clear(conn: &Connection) -> Result<()>` を追加: `DELETE FROM history;` + `VACUUM;`
- `main.rs`: `Command::Clear` を追加し、interactive 判定 (`matches!`) に追加。成功時は無出力

## 4. テスト

- **db_test**: `trim_history`
  - 上限超過時、古い行から削除され最新の N 件が残る
  - ちょうど N 件 → 0 件削除
  - N 件未満 → 0 件削除
  - dedup で更新された行が保護される (started_at 更新後は残る)
- **cli_test**:
  - `SEASALT_HISTORY_MAX=10` で 12 件 record → ちょうど 10 件が残り、最新の 10 件が含まれる (トリムは毎 record 実行なので決定的)
  - `SEASALT_HISTORY_MAX=0` → 無制限 (全件残る)
  - `clear` → `search --all` が空になり、成功時は無出力
  - 既存テストは SEASALT_HISTORY_MAX 未設定のためデフォルト 100,000 が効くが、行数が少ないので影響なし
- **smoke.sh**: record → `seasalt clear` → `search --all` が空、の節を追加 (スニペットの変更なし)

## 5. ベンチマーク

`benches/suggest.rs` に trim のシナリオを追加し、hot path への影響を実測する:

- 100k 行 DB で `trim_history(conn, 100_000)` (上限未満 → 0 件削除のコスト)
- 100k 行 DB で `trim_history(conn, 90_000)` (上限超過 → 10k 件削除のコスト)

## 6. ドキュメント

- **README**: Features に自動トリム、Usage に `seasalt clear`、Configuration に `SEASALT_HISTORY_MAX` を追記 (英語)
- **設計仕様 (2026-08-15-seasalt-design.md)**: §3 ストレージに上限の記述、§4 に `seasalt clear` とトリムの仕様を追記 (日本語)

## 7. スコープ外

- 古い重複行の整理、TTL (期間ベース) トリム、VACUUM の定期実行、上限の設定ファイル化 — すべて対象外
- `seasalt delete` と `search`/`suggest` の挙動は変更しない
