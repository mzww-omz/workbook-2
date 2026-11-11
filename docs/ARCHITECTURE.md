# Architecture

```text
workbook-app     macOS起動、データディレクトリ、メニュー
  └─ workbook-ui        GPUI Kit views / input / dialogs / theme
       └─ workbook-service    Application Service（コマンド、トランザクション境界）
            ├─ workbook-scheduler  FSRS-7 adapter / StepPolicy / QueueSelector / Clock
            ├─ workbook-import     JSON v1/v2・画像アセット / 旧DB v1 の検証・移行計画
            ├─ workbook-storage    SQLite schema / attempts / assets / backup / restore
            └─ workbook-domain     Content / Question / grading / Settings / 4時カレンダー

workbook-lint    単体CLI。workbook-domain の検証と workbook-import のアセット読込を再利用
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
  │    grade_answer_detailed で correct / near_miss / incorrect / manual を判定
  │    Near Miss なら assist_json に履歴を追記し、Attempt も FSRS も書かず Feedback
  │    correct/incorrect は automatic_rating（domainの純粋関数）で rating を決定
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
| ヒント使用後の正解 / Near Miss 後に正解 | Hard |
| ヒントなしの正解 | Good |
| 明確に容易（自動では判定しない） | Easy（手動のみ） |
| 自動採点不可（フラッシュカード） | 手動フォールバック |

Near Miss は `sessions.assist_json` に履歴を残し、`retry_answer` で Question に戻る。
最終 Attempt に `attempts.assist_json` として履歴を保存する。Near Miss の段階では
`attempts` も `review_states` も変更しない。

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

## UI の入力座標とコンテンツ描画

- Hotspot は画像要素の実測 bounds と GPUI の `ClickEvent` 位置（論理ピクセル）から、
  `contain` 表示のレターボックスを考慮して 0〜1 に正規化する。20×20 などの固定
  グリッドは使わない。保存形式は従来どおり `Answer::Hotspot { x, y }`。
- 座標変換、多角形の代表点、並べ替え、割当ては `workbook-ui/src/ui_logic.rs` の
  純粋関数に置き、ディスプレイなしで単体テストする。
- Ordering / Matching / Labeling / 選択肢は `workbook-ui/src/content.rs` の共通
  レンダラーを使い、Text / Markdown / Image / Math / Code を ID ベースの回答形式を
  変えずに表示する。問題形式ごとの専用レンダラーは作らない。
- Content の画像は `ImageSource::Url`（HTTPS・外部取得なし）と
  `ImageSource::Asset`（教材同梱）を型で区別する。アセットは `StudyView.assets` として
  Service が SQLite から読み、UI はアセット ID で参照する。UI は SQLite を直接触らない。
- 画像は `workbook-ui/src/content.rs` でワークブック ID + アセット ID + 内容
  フィンガープリントをキーに `Arc<Image>` を LRU キャッシュし（推定デコードメモリ
  64MiB・最大 24 件）、毎フレームのデコードと問題切替ごとの再デコードを避ける。
  追い出した画像は `App::remove_asset` で GPUI のアセットキャッシュからも解放する。
  再インポート・アセット更新・Workbook 削除・バックアップ復元で無効化する。
- 画像の表示領域は `ui_logic::image_box` で求める。`contain` は画像全体を
  アスペクト比維持で shrink-wrap し、`cover` は 4:3 のビューポートを画像で埋めて
  中央基準で切り抜く。Hotspot / Labeling は座標精度のため常に `contain`。
  Hotspot のクリック座標と Labeling の配置先は、同じ実測 bounds と
  `ui_logic::contained_image_rect` / `image_point_to_container` を使う。これにより
  レターボックス、ウィンドウ拡縮、スクロール、Retina/HiDPI で正規化座標の意味が
  変わらない。Labeling のターゲットを外側要素の `%` で置かない。
- 数式は `workbook-ui/src/math.rs` で LaTeX サブセットを解析し、分数・根号・
  上下付き・大型演算子・括弧・本文テキストのレイアウトツリーへ変換して GPUI の
  flex / text 要素で描画する。WebView やブラウザエンジンは使わない。解析不能な
  入力は生の LaTeX と理由を表示し、元の LaTeX をアクセシビリティラベルにする。
- UI は採点しない。採点と Near Miss の判定は `workbook-domain::grading` が持つ。

## 時間予算

`workbook-scheduler/src/budget.rs` は純粋関数で、候補を
「期限到来Short-term → 期限到来Review → 新規 → Learn Ahead」の順に並べ、
回答履歴の平均時間（形式別の保守的初期値にフォールバック）で予算内に収まる項目を選ぶ。

- 予算は出題量の目標であり、強制終了タイマーではない
- 予算0は拒否。最初の1問が推定超過でも、学習不能にしないため選択する
- 選ばれなかった問題の `due_at` は変更しない
- `sessions.time_budget_seconds` に保存し、StudyViewは使用時間と残り目安を表示する
- 予算超過後も回答は通常どおり確定できる

## Topic と Scope

```text
Topic (stable id, optional parent)
  └─ question_topics (workbook_id, question_id, topic_id)

Scope (ExamScope/StudyScope JSON)
  = (group1 OR group2 ... OR includeQuestions)
    AND NOT (excludeQuestions OR excludeWorkbooks OR excludeTags OR excludeTopics)
  group = (workbook_ids empty or contains) AND (tags empty or intersects)
          AND (topics empty or intersects)
```

- Topicの同一性はID。同じ親の下で正規化名が同じ場合だけ衝突として拒否する
- 親Topicを選ぶと、解決時に子孫IDへ展開する。保存時に子孫を凍結しない
- JSON教材の `topics` は `数学/一次関数` のようなパス。Import時にTopicを解決・共有する
- Exportは `topicCatalog`（ID・表示名・親ID）を併記する。再Import時はIDを優先し、
  改名後でも同じTopicへ割り当てる。catalogのない旧JSONはパスだけで解決し、推測で統合しない
- Goal/ExamのScopeから参照中のTopicは削除を拒否する
- Scopeは毎回現在の教材一覧から解決し、Session開始時に `session_items` へ固定する
- 学習中のSessionは変更しない。新教材・新Topicは次の解決から反映する

## Goal と Exam

- `goals` は汎用目標。名前、Scope、任意の期限・目標スコア、1日の学習時間、アーカイブを持つ
- 既存Examは `Goal::from_exam` で一覧・Planner・統計へ適合表示する。Exam行・模試・Sessionは変更しない
- Goal Planは既存 `plan_exam` を再利用し、期限・1日の学習時間・今日の学習済み時間・新規上限・
  想起確率・苦手・未学習から今日の出題を選ぶ
- Sessionの `goal_id` で今日の学習時間をGoal別に差し引く
- Topic別統計は保存済み状態とAttemptの実測のみ。習得率・合否予測は作らない
- 初回正答率は各Questionの最初の記録済みAttemptだけを分母にする

## Authoring

- `WorkbookService::save_workbook` は既存教材の `local_id` を維持し、
  `update_workbook_content` がNote/QuestionをIDで突合する
- 変更のないQuestionは `questions` 行と `review_states` を保持する
- 削除されたQuestionはFKカスケードでFSRS状態を消すが、`attempts` は独立して残す
- 保存前にDomain Validationと `workbook-lint` を実行し、パス付きエラーを返す
- 既存教材の保存前に `pre-edit-*.sqlite` を作る。学習中Workbookの編集は拒否する
- JSON ExportはTopicパスを復元し、画像をbase64で埋め込む
- AI Provider・APIキー・クラウド呼び出しは実装しない

## SQLite

`application_id = 1463962418`（`"WBK2"`）、`user_version = 8`。

| Table | 役割 |
|---|---|
| `workbooks` | ローカルUUID、教材JSON、import日時、last_used |
| `notes` | Note、初回学習済み |
| `questions` | Question、関連問題の延期 |
| `topics` | 安定ID、名前、正規化名、任意の親、説明 |
| `question_topics` | QuestionとTopicの多対多 |
| `assets` | 教材同梱画像（media type、alt、実サイズ、バイト列） |
| `review_states` | FSRS状態、due、revision |
| `sessions` | mode/status/cursor/phase/draft/pending/assist/revision、試験ID、Goal ID、時間予算、模試制限時間 |
| `session_items` | セッションの初期対象と状態 |
| `session_workbooks` | 複数WorkbookのResult順序 |
| `attempts` | 回答ごとの独立UUID、正誤、評価、評価元（automatic/manual）、前後状態、Near Miss履歴、時刻 |
| `exams` | 試験名、目標日、1日の学習時間、スコープJSON、アーカイブ |
| `goals` | 汎用目標、スコープJSON、期限、目標スコア、1日の学習時間、アーカイブ |
| `mock_answers` | 模試の回答（FSRSとは分離） |
| `mock_results` | 模試の提出集計 |
| `settings` | JSON設定 |
| `meta` | アプリ識別情報 |

設定:
`WAL`, `synchronous=FULL`, `foreign_keys=ON`, `trusted_schema=OFF`, `busy_timeout=5000`。

スキーマ移行:
`user_version=1` のDBは起動時に `attempts.rating_source`、`assist_json` 列、`sessions.assist_json`、
`assets` テーブルを追加する。v4 で `exams`、`mock_answers`、`mock_results` と
sessions の exam/time-limit 列、v5 で `exams.daily_minutes`、v6 で `sessions.time_budget_seconds`、
v7 で `topics` / `question_topics`、v8 で `goals` / `sessions.goal_id`、
v9 で `questions.revision` と `attempts.question_revision` を追加する。
変更前に `backups/pre-migration-v{from}-*.sqlite` をSQLite backup APIで作成する。
古いバックアップも復元時に同じ前方移行を行う。

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
- Goal / Examの進捗（期限、対象、学習済み、復習、カバレッジ）
- 主操作「今日の学習を始める」。時間予算（3/5/10/20分/制限なし）を選んで開始する

短期再学習が残っている場合は「完了」と表示せず、バナーと「今すぐ再開」を出す。通常の期限候補がない場合、Reviewセッション開始時に Learn Ahead 対象を候補へ含める。

## 画像アセット検証

- 画像アセットは `workbook-domain::inspect_image` が Import と Linter の共通入口。
- PNG/JPEG は宣言された `mediaType` とバイト列のマジックを照合したうえで、
  ヘッダー読取 → 寸法・画素数の上限確認 → 完全デコードの順に検証する。
  24バイトの PNG ヘッダーだけ、IEND 欠落、CRC 不正、途中で切れた JPEG、
  不正セグメント、MIME 不一致はすべて拒否する。
- デコードは `image` crate の PNG/JPEG のみを有効化し、幅・高さ 20,000、
  画素 2,400万、割当量の上限をデコーダーにも渡す。破損データで panic しない。
- SVG は `roxmltree` で XML として解析し、要素・属性の許可リストで検証する。
  文字列検索は使わない。DTD・外部エンティティ・未知の名前空間・`script` /
  `foreignObject` / `iframe` / `object` / `embed` / `image` / `use` / `style`、
  イベントハンドラ、外部 URL を拒否する。`url(...)` は `#fragment` のみ許可。
- 不正な画像は SQLite へ書き込む前に拒否する。Linter は `WB-ASSET-002` として
  Error を返す。

## バックアップ

- SQLite backup API で一貫したコピー
- 日次 `daily-YYYY-MM-DD.sqlite` を7世代
- pre-import / pre-delete / pre-edit / pre-delete-topic / pre-delete-goal / pre-delete-exam / pre-restore / pre-reset の安全バックアップ
- 通常の `import_workbook_json` / `import_workbook_file` も、教材と全アセットの
  検証成功後に `pre-import-*.sqlite` を作成する。検証失敗時はバックアップを作らない。
  バックアップ作成に失敗した場合は Import を開始しない。
- 安全バックアップ名は時刻 + ランダムUUID。`create_new` で名前を予約するため、
  同時刻の連続Importでも既存バックアップを上書きしない。
- バックアップは `<data>/backups/` に 0600 で保存する。日次だけ7世代で自動削除し、
  操作前バックアップはユーザーが明示的に削除するまで保持する。
- 復元は別DBで application_id / user_version / スキーマ / trigger / integrity / FK / 教材JSON / settings / review_states を検証
- 復元失敗時は旧DBへ戻す

## Exam

```text
Exam scope (JSON)
  │  resolve_scope（純粋関数、現在のWorkbook一覧とタグ）
  ▼
ScopeQuestion [(workbook_id, question_id)]
  │
  ├─ Plan     → 期限・学習時間・FSRS・苦手から今日の候補を選び、新規は単元単位、既習は混合
  ├─ Review   → 既存 review_candidates を範囲で絞り、既存 Session / Attempt / FSRS を共有
  ├─ Practice → 戦略で絞り、既存 Session / attempts に記録。review_states は書かない
  └─ Mock     → session_items を固定し、mock_answers / mock_results に記録
```

`Plan` の選定・時間配分・Interleaving は `workbook-scheduler/src/planner.rs` の純粋関数。
想起確率は `fsrs-rs` の `current_retrievability` を使い、忘却曲線を再実装しない。
`Plan` も通常の Attempt とFSRS状態を書き、模試だけが分離される。

スコープ解決は `workbook-domain/src/exam.rs` の純粋関数で、DB にも UI にも依存しない。
`workbook-service` は教材一覧からカタログを構築して解決し、セッション開始時に
`session_items` へ固定する。したがって、学習中に教材をインポートしてもその
セッションの出題は変わらない。統計・プレビューは毎回現在のカタログから再計算する。
