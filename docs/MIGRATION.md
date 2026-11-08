# 旧 WorkBook v1 からの移行

## 保証範囲

- 旧 Electron Workbook **0.1.0 / schema v1**（`application_id=1463962443`, `user_version=1`）
- 旧DBは読み取り専用でステージングコピーし、integrity / foreign key / 必須テーブル / 教材JSON / FSRSカード / settings / sessions を検証
- 変換先の新アプリDBへ追加した後、件数・参照整合性を確認
- 失敗時は新アプリDBを変更しない（`apply_legacy_plan` 全体が1トランザクション、事前に安全バックアップ）
- 旧DB、旧アプリのデータディレクトリ、バックアップは書き換えない

**v2 は実在しないため未対応**です。`user_version != 1` は明示的に拒否します。

## 手順

1. 新アプリの「設定 → 旧Workbookから移行」
2. 旧 `workbook.sqlite`（または `workbook-data` フォルダ）を選択
3. プレビューで教材数 / Note / 問題 / FSRS状態 / 回答 / アクティブセッションを確認
4. 「移行する」
5. 新アプリDBに追加され、教材一覧に表示

旧アプリを起動したままでも読み取り専用コピーを使うため、旧DBを変更しません。ただし一貫したコピーのため、旧アプリを終了してからの移行を推奨します。

## 変換内容

| 旧 | 新 |
|---|---|
| `workbooks.id` | 新しいローカルUUIDを発行。`source_id` に旧IDを保持 |
| `workbooks.content` | JSON v1を再検証して保存 |
| `notes.introduced` | `notes.introduced` に保持 |
| `questions.buried_until` | `questions.buried_until` に保持 |
| `review_states.card` | `review_states` の各列に分解。`stabilityFast` を保持 |
| `scheduledDays` / `learningStep` / `reps` / `lapses` | 保持 |
| カードのalgorithm | `FSRS-7` / `legacy-ts-fsrs-6.0.0` を記録 |
| `sessions.data` | `sessions` + `session_items` + `session_workbooks` に正規化 |
| `review_logs` / `practice_attempts` | `attempts` に1行ずつ移行 |
| `settings` | theme/newLimit/automaticRating/retention/externalImages/autoBackup を保持。timezoneはシステム |
| アクティブセッション | 復元可能。ただし新アプリに既存アクティブセッションがある場合は拒否 |

## 履歴の制約

旧 v1 は `(session_id, workbook_id, question_id)` が主キーで、1セッション・1問題につき1件しか保持しません。Workbook 2 は捏造した再出題履歴を作らず、保存済みの1件を1 Attempt として移行します。移行プレビューにも警告を表示します。

旧セッションの `outcomes` は Attempt の正本として使わず、`review_logs` / `practice_attempts` を優先します。セッションの進行状態、draft、hint、revealed、question_elapsed_ms は保持します。

## バックアップと復元

- 教材Import前 / 旧データ移行前: `backups/pre-import-<timestamp>-<unique>.sqlite`
- 削除前: `backups/pre-delete-<timestamp>-<unique>.sqlite`
- 復元前: `backups/pre-restore-<timestamp>-<unique>.sqlite`
- リセット前: `backups/pre-reset-<timestamp>-<unique>.sqlite`
- 日次: `backups/daily-YYYY-MM-DD.sqlite` を7世代

操作前バックアップは一意名で作成し、既存ファイルを上書きしません。日次以外は自動削除せず、
ユーザーが不要になった時点で削除します。

復元は新アプリの完全バックアップのみ受け付けます。旧WorkbookのDBや教材JSONは復元対象ではありません。

## 復旧

移行・復元で失敗した場合、現在の新アプリDBは置き換え前の状態に戻ります。エラー内容を確認し、必要なら設定の「バックアップから復元」で安全バックアップを選択してください。

旧アプリの物理破損時は、旧アプリを終了して `workbook-data` 全体を別名へ移動し、旧アプリの正常なバックアップから復旧してください。Workbook 2 は旧DBを自動修復しません。
