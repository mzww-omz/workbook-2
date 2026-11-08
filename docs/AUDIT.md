# M0 — 旧 Workbook 監査結果

対象: `https://github.com/mzww-omz/workbook` の clone（commit `9419e33`、`package.json` version `0.1.0`）
監査日: 2026-11-07
方法: ソース、`README.md`、`DESIGN.md`、`PRODUCT.md`、`docs/FORMAT.md`、テスト、`node:sqlite` スキーマ、`ts-fsrs 6.0.0` の実挙動を実行して確認。

## 1. 現行仕様（実コードが基準）

| 項目 | 事実 |
|---|---|
| アプリ | Electron 45.2.1 / Svelte 5 / TypeScript / `node:sqlite` |
| DB | `application_id = 1463962443`, `user_version = 1` のみ。v2 は存在しない |
| 保存先 | `app.getPath('userData')/workbook-data/workbook.sqlite` |
| FSRS | `ts-fsrs 6.0.0` の `DefaultScheduler`, `version: 'FSRS-7'` |
| FSRS設定 | retention 0.9 既定、`enableShortTerm: true`、learning `1m/10m`、relearning `10m`、fuzz OFF、`allowModelOverride: false` |
| 新規上限 | settings `newLimit`、既定 10。**1セッションあたり** |
| 学習日 | `Date#setHours(hours-4)` によるローカル4時境界。次回境界は次のローカル4時 |
| 回答評価 | 不正解/わからない→Again、ヒントあり正解→Hard、ヒントなし正解→Good。Easy自動なし |
| 手動評価 | `automaticRating=false` では評価必須 |
| セッション | 同時1つ。`sessions.data` に queue/index/phase/draft/outcomes/skipped をJSON保存 |
| 復習キュー | セッション開始時に固定。Review は Note ごとに1問、最も古い期限順。New は問題一覧順で上限まで |
| 短期再出題 | FSRS/`ts-fsrs` は Again の期限を1分後に返すが、**固定キューに再挿入しない**ため、同一セッション中に再出題されない |
| Learn Ahead | 実装なし。20分以内の前倒し再出題なし |
| 履歴 | `review_logs` / `practice_attempts` の PK は `(session_id, workbook_id, question_id)`。1セッション1問題1件で上書き |
| 訂正 | 現在の Feedback のみ。保存した評価前カードと元回答時刻から再計算し、同じログを更新。Attemptは増えない |
| Note | `notes.introduced` は非Practiceの回答確定時に1。スキップ/練習では更新しない |
| 関連問題 | Review回答後、同Noteの他Questionを次の学習日4時まで `buried_until`。FSRS期限は変更しない |
| JSON | `.workbook.json` v1。6形式。10MiB。未知フィールド拒否。ID一意性・正解参照・空欄対応を検証 |
| バックアップ | WAL/FULL。起動時と1分ごとの日付確認で日次7世代。pre-migration/pre-restore/pre-reset |
| 復元 | アプリID/user_version/スキーマ/整合性/FK/教材JSON/settings/sessionを検証し、ステージング後に置換。失敗時ロールバック |
| 初回学習カード | New の Note 初回回答前に intro 画面を表示 |
| 教材コピー | インポートごとに新しいローカルUUID。同一JSONでも独立 |

## 2. README・過去計画と実コードの差異

| 記述 | 実際 |
|---|---|
| READMEは「セッションの保存と再開」「回答・下書き・結果画面の保存」と記載 | 実装されている。ただしキューは開始時固定 |
| READMEは「Againを直後へ再挿入しません」と明記 | そのとおり。新要件の Learn Ahead / 短期再出題とは異なる |
| v0.2.0計画の「動的キュー」「Learn Ahead」「Attempt履歴」 | 現行コードには存在しない |
| 「20件のテスト」 | 実行可能。ただし短期再出題・Learn Ahead・複数Attemptを検証するテストはない |
| v1/v2移行 | 実在するのは v1 のみ。v0/v2からの移行コードはない |
| `outcomes` の上書き問題 | キーが `workbookId/questionId`。固定キューが同一問題を再挿入しないため表面上は問題化しにくいが、再出題を導入すると履歴が壊れる構造 |

## 3. 引き継ぐ仕様

- JSON教材 v1 の全形式・検証・採点正規化（NFKC/trim/lowercase/空白統一）
- FSRS-7 の数値状態（stability, stabilityFast, difficulty, state, dueAt, lastReviewAt, reps, lapses, scheduledDays, learningStep）
- Learning `1m/10m`、Relearning `10m`、retention 0.9、fuzz OFF、`allowModelOverride=false`
- 4時学習日、ReviewのNote単位抑制、同一Questionの短期再出題許可
- Note初回カード、スキップ/練習の非更新、Easy自動なし
- 完全バックアップ、日次7世代、復元検証、ローカルUUID

## 4. 改善・再設計する部分

| 旧 | Workbook 2 |
|---|---|
| 固定キュー | 回答ごとに DB から動的再選択 |
| 短期再出題なし | due Learning/Relearning を最優先。20分以内 Learn Ahead |
| `review_logs` PK = session/workbook/question | `attempts` 独立UUID。同一問題の複数回答を全件保持 |
| 訂正で outcome JSON を上書き | Attempt行を更新し、評価前状態＋元時刻から再計算。件数不変 |
| Session JSON に全状態 | `sessions` / `session_items` / `session_workbooks` / `attempts` に正規化 |
| New上限が「選んだ問題数」 | ユニーク新規Questionで数える（セッション開始時に確定） |
| Review開始時のみ期限判定 | 起動・再開・回答後・アクティブ化・タイマー・4時境界で再評価 |
| icon/attempt履歴なし | 独立attempt、state revision、session revision、冪等commit |
| 旧DB v1のみ | v1移行を実装。v2は実在しないため未対応を明記 |

## 5. 未確認・制約

- 旧リポジトリに Windows / Linux 実機検証はない。Workbook 2 も macOS arm64 のみ検証
- VoiceOver 全画面監査、日本語IMEの手動確認は旧アプリでも未完了。Workbook 2 も未実施
- 旧アプリのIntel Mac・大規模教材性能は未確認
- v2スキーマは存在しないため、移行対象は v1 のみ
- 実ユーザーデータは使用していない。Golden Fixture は合成データのみ

## 6. 互換性フィクスチャ

`fixtures/legacy-fsrs7-golden.json` を `ts-fsrs 6.0.0` の `DefaultScheduler` で生成した。内容は合成データで、実ユーザーデータを含まない。再生成は `scripts/generate-legacy-golden.ts`。

検証結果は [COMPATIBILITY.md](COMPATIBILITY.md)。
