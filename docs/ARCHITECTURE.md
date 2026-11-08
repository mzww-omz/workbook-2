# Architecture

```text
workbook-app     macOS起動、データディレクトリ、メニュー
  └─ workbook-ui        GPUI Kit views / input / dialogs / theme
       └─ workbook-service    Application Service（コマンド、トランザクション境界）
            ├─ workbook-scheduler  FSRS-7 adapter / StepPolicy / QueueSelector / Clock
            ├─ workbook-import     JSON v1 / 旧DB v1 の検証・移行計画
            ├─ workbook-storage    SQLite schema / attempts / backup / restore
            └─ workbook-domain     Content / Question / grading / Settings / 4時カレンダー
```

- `workbook-domain` は GPUI / SQLite / FSRS に依存しない
- `workbook-scheduler` は UI / DB に依存せず、`Clock` を注入できる
- `workbook-ui` は SQLite や FSRS ライブラリを直接触らず、`workbook-service` のコマンドだけを呼ぶ
- SQLite が唯一の永続状態。GPUI Entity は画面の一時状態だけを持つ

`workbook-service` は推奨構成に追加した層で、UIとStorageの間にコマンド境界を置くために分離した。

## FSRS 回答確定フロー

```text
回答入力
  │
  ├─ EvaluateAnswer（サービス内）
  │    correct/hint/unknown を計算、draft/elapsed/pending_at を session に保存
  │    自動評価ONなら automatic_rating（domainの純粋関数）で rating を決定
  │    rating が決まる場合は 1 トランザクションで CommitRating 相当まで実行
  │    （Attempt・FSRS状態・Sessionを同時に保存）
  │    自動採点不可なら Feedback の未確定状態で止め、手動評価へフォールバック
  │
  ├─ 自動評価OFF、または手動フォールバック時
  │    ユーザーが Again/Hard/Good/Easy を選択
  │
  └─ CommitRating(attempt_id, rating, answered_at)
       BEGIN IMMEDIATE
         before_state ← review_states
         elapsed = answered_at - before.last_review_at
         memory ← FSRS7.next_states(before, elapsed)
         due/state/step ← StepPolicy(decision)
         review_states ← after_state
         attempts INSERT（独立Attempt ID, rating_source=automatic|manual）
         sessions ← pending解除, revision+1
         session_items ← answered
         notes ← introduced=1（Practice以外）
         questions ← sibling bury（Reviewのみ）
       COMMIT
```

自動評価ポリシーは `workbook-domain` の `automatic_rating` にあり、時刻・UI・DBに依存しない。

| 状況 | rating |
|---|---|
| 不正解 / 「わからない」 | Again |
| ヒント使用後の正解 | Hard |
| ヒントなしの正解 | Good |
| 明確に容易（自動では判定しない） | Easy（手動のみ） |
| 自動採点不可（フラッシュカード） | 手動フォールバック |

- 同一Attempt IDの再送は既存Attemptを返し、FSRSを二度更新しない
- 自動評価は評価と確定を同じトランザクションで行うため、クラッシュ時に未確定の回答だけが残らない
- 相違なるAttempt IDでも、Feedback中の二重確定は `session.pending_correct` と revision で拒否
- 訂正は `before_state_json` と `answered_at` から再計算し、同じAttempt行を更新する。Attempt件数は増えない。`rating_source` は manual になる
- Practice は review_states を書かない。Skip も書かない

## 動的キュー

`QueueSelector` は次問のたびに以下を再評価する。

1. `due_at <= now` の Learning / Relearning
2. 現在のWorkbookの未回答 initial item（Reviewは開始時対象、Newは選択済み新規）
3. 1と2が空のときだけ、`0 < due - now <= 20分` かつ同じ学習日の Learning / Relearning
4. なければ WorkbookResult

- 各区分は `due_at`、ordinal、Question ID で安定ソート
- Learn Ahead は前倒し表示のみ。保存済み `due_at` は書き換えない
- 1問だけの教材でも、他に候補がなければ1分後のカードを即再出題できる
- 同じ Note の別QuestionはReviewで1問に抑制。同じ Question の短期再出題は許可

## 学習日

- `due_at` は UTC Instant で保存
- 学習日は IANA タイムゾーンに変換し、ローカル時刻から4時間引いた日付
- 次境界は次のローカル4時を UTC に変換
- Learn Ahead は same learning day のときだけ
- 3:55に4:05の期限が来る短期カードは前倒ししない

## SQLite

`application_id = 1463962418`（`"WBK2"`）、`user_version = 2`。

| Table | 役割 |
|---|---|
| `workbooks` | ローカルUUID、教材JSON、import日時、last_used |
| `notes` | Note、初回学習済み |
| `questions` | Question、関連問題の延期 |
| `review_states` | FSRS状態、due、revision |
| `sessions` | mode/status/cursor/phase/draft/pending/revision |
| `session_items` | セッションの初期対象と状態 |
| `session_workbooks` | 複数WorkbookのResult順序 |
| `attempts` | 回答ごとの独立UUID、正誤、評価、評価元（automatic/manual）、前後状態、時刻 |
| `settings` | JSON設定 |
| `meta` | アプリ識別情報 |

設定:
`WAL`, `synchronous=FULL`, `foreign_keys=ON`, `trusted_schema=OFF`, `busy_timeout=5000`。

スキーマ移行:
`user_version=1` のDBは起動時に `attempts.rating_source` を追加し、`user_version=2` へ更新する。変更前に `backups/pre-migration-v1-*.sqlite` をSQLite backup APIで作成する。v1のバックアップも復元時に同じ移行を行う。

## 再評価タイミング

| タイミング | 実装 |
|---|---|
| 起動 | `WorkbookService::open` → snapshot / active view |
| セッション開始 | `start_session` が現在時刻で候補を再計算 |
| 回答確定後 | commit 後に UI が `refresh()`（snapshot 再取得） |
| セッション再開 | `resume` が `active_view()` をDBから再取得 |
| アプリ再アクティブ | `observe_window_activation` |
| 起動中の期限到来 | `next_wake_at` に基づくタイマー（最大10分、期限が近ければ期限時刻） |
| 4時境界 | `next_wake_at` が次の4時も返す |

画面描画中にDB全体を走査しない。Today/Library/History は snapshot / キャッシュを使い、更新はタイマーまたはコマンド後の `refresh` で行う。History は画面遷移時に読み込む。

## Today

`review_candidates` と `upcoming_short_term` から実データを算出する。

- 復習: 期限到来済み Learning/Relearning + 期限到来 Review（Note単位抑制後）
- 新規: `review_states` がないユニークQuestion
- 短期再学習: 20分以内の Learning/Relearning
- 次の期限: 未来の最小 `due_at`
- Workbook別の復習/短期/新規

短期再学習が残っている場合は「完了」と表示せず、バナーと「今すぐ再開」を出す。通常の期限候補がない場合、Reviewセッション開始時に Learn Ahead 対象を候補へ含める。

## バックアップ

- SQLite backup API で一貫したコピー
- 日次 `daily-YYYY-MM-DD.sqlite` を7世代
- pre-import / pre-delete / pre-restore / pre-reset の安全バックアップ
- 復元は別DBで application_id / user_version / スキーマ / trigger / integrity / FK / 教材JSON / settings / review_states を検証
- 復元失敗時は旧DBへ戻す
