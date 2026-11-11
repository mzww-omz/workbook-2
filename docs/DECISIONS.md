# 設計判断

## 1. GPUI の固定と vendored patch

- `gpui-kit = "=0.9.0"`、内部で `gpui-pre 0.3.11`（Zed commit snapshot）に固定
- GPUIはpre-1.0のため、バージョンを固定
- GPUI Kit の macOS backend が stable Rust 1.96 では未安定の `Result::map_or_default` を3箇所使用していた
- `vendor/gpui-pre-macos` に該当crateをコピーし、`.map_or_else(Default::default, ...)` に機械置換
- `[patch.crates-io]` でそのcrateだけ差し替え。GPUI本体のフォークや独自描画はしていない
- 異なるGPUI crateを混在させていない（UIは `gpui-kit` の再エクスポートのみ使用）

## 2. FSRS は `fsrs-rs` main の FSRS-7

- crates.io `fsrs 6.6.2` は FSRS-6（21パラメータ、`stability_fast` なし）
- FSRS-7 は main branch に存在するため commit `65224f1d...` 固定
- FSRS数式はライブラリに任せ、短期ステップ・状態・dueは `StepPolicy` に分離
- 旧 `ts-fsrs DefaultScheduler` の `allowModelOverride=false` を再現
- f32/f64差は許容範囲としてテストし、完全一致とは主張しない

## 3. `workbook-service` を追加

推奨構成の `workbook-app` に置くと UI と循環するため、Application Service を独立crateにした。

- UI → service → storage/scheduler/import
- service がトランザクション境界と revision / idempotency を所有
- storage は FSRS を知らない
- scheduler は DB / UI を知らない

## 4. Attempt を独立テーブルに

旧 `review_logs` は `(session, workbook, question)` がPKで、同じ問題の再回答を保持できなかった。

- `attempts.id` を独立UUID
- `session_id`, `question_id`, `answered_at`, rating, correct, hint, unknown, answer, before/after state, state_revision, short_term を保存
- 評価訂正は行更新。新しいAttemptを追加しない
- 二重送信はAttempt IDで冪等
- 短期再出題は `short_term=1` で区別

## 5. 動的キュー

固定キューは「期限が来た短期カードを同じセッションで出せない」「Learn Aheadできない」ため廃止。

- 次問のたびに `session_items` + `review_states` を参照
- 候補は DB の現在値から再計算
- セッション開始時の通常対象は `session_items` に固定し、Review中に新しいReview期限が来ても勝手に追加しない（旧仕様を維持）
- 短期カードは同じセッション内で再出題

## 6. Learn Ahead

- 20分以内、同じ学習日、通常候補が空のときだけ
- `due_at` は変更しない
- 確認ダイアログ・回数制限・最低待機は追加しない
- 3:55→4:05 のような学習日跨ぎは対象外

## 7. 4時学習日とタイムゾーン

- 保存はUTC Instant
- 設定 `timezone` はIANA名。既定はシステム
- `StudyCalendar` が日付境界とLearn Ahead可否を所有
- 画面の日付・履歴表示も同じタイムゾーン

## 8. Today の「完了」判定

- 期限到来Review/Learningだけで完了にしない
- 20分以内の短期再学習があれば「短期再学習が待っています」を表示
- 通常候補がないReviewセッション開始時は Learn Ahead 対象を初期候補に含める
- これにより「短期カードが残っているのに完了」を防ぐ

## 9. 起動中の更新

- 描画ごとにDBを読まない
- `observe_window_activation` で再アクティブ時に再評価
- `next_wake_at`（次のdue / 次の4時）に合わせたタイマー。最大10分で再確認
- テキスト入力中はタイマー更新をスキップし、IME入力を乱さない

## 10. 画像・数式

- 外部画像は取得しない。既存の image ブロックは alt / caption / URL を表示
- Hotspot / Labeling 用の画像は教材に同梱し、SQLite の `assets` テーブルへ取り込む
- mathブロックは `workbook-ui/src/math.rs` のLaTeXサブセット解析 + GPUIレイアウトで組版する。
  依存crateを増やさず、WebViewやブラウザエンジンも使わない。分数・根号・上下付き・
  大型演算子・括弧・本文テキストに対応し、解析不能時は生LaTeXを理由付きで表示する
- 画像の `contain` は全体表示、`cover` は4:3ビューポートへの中央クロップ。Hotspot /
  Labelingは座標精度のため常に `contain`
- デコード済み画像は workbook + asset + 内容フィンガープリントのLRU（64MiB / 24件）。
  問題切替では消さず、再インポート・更新・削除・復元で無効化する
- MarkdownはGPUIのrich text。HTMLは実行しない

## 11. バックアップ

- 使用中DBのコピーではなく SQLite backup API
- 0700ディレクトリ、0600ファイル
- 7世代日次 + 安全バックアップ
- 復元は検証後にファイル置換、失敗時ロールバック

## 12. 新規上限

設定 `new_limit` は「1日の新規Question数」として扱う。

- 当日の新規セッションで commit された distinct Question を数える
- Skip、Practice、未確定のEvaluateは数えない
- 4時境界でリセット
- Newセッション開始時に残り枠を `new_limit - introduced_today` で計算
- 残り0ならセッションを開始しない

## 13. 自動評価

- `workbook-domain::automatic_rating` を純粋関数として置き、時刻・UI・DBに依存させない
- 不正解 / わからない → Again、ヒント使用後の正解 → Hard、ヒントなし正解 → Good
- Easy は自動判定しない。回答時間や初回正解を根拠にしない
- フラッシュカードは主観評価のため手動フォールバック。ただし「わからない」は Again
- 自動評価ONでは `EvaluateAnswer` 内で評価と確定を同一トランザクションにする
- 自動評価OFFでは従来の `CommitRating` を残す
- 訂正は既存の `CorrectAttempt` を使い、Attemptを増やさず `rating_source=manual` にする
- `attempts.rating_source` で automatic / manual を識別する

## 14. schemaVersion 2 と v1 互換

- v1 の6形式・JSON構造・採点結果は変更しない
- 新しい5形式と `assets` は `schemaVersion: 2` でのみ許可
- v1 で v2 形式を使うとインポート時にパス付きで拒否
- v2 でも v1 の6形式を使える
- Question の新規フィールドは `Option` と `skip_serializing_if` で、v1 を読み書きしても
  JSON に空フィールドを増やさない
- 回答は `Answer` の新しい variant として保存し、既存4 variant のシリアライズは不変

## 15. 画像アセットはSQLiteへ取り込む

- 教材JSONは `file`（相対パス）または `data`（base64）を宣言できる
- インポート時に PNG/JPEG のマジックと実サイズ、SVGの安全な静的サブセットを検証し、バイト列をDBへ保存する
- 学習中は教材ファイルのパスを読まない。パストラバーサルとsymlink escapeはインポート時に拒否
- GIF/WebPは拒否。SVGはスクリプト・外部参照・イベントハンドラ・style・foreignObject・use/imageを拒否した静的SVGのみ
- 画像がDBに入るため、SQLite backup APIのバックアップ／復元にそのまま含まれる
- `file` 参照はJSON単体インポートでは解決できないため、ファイルからインポートする

## 16. Near Miss は未確定状態

- `correct/incorrect` の二値ではなく `correct/near_miss/incorrect/manual` を domain で返す
- Near Miss は Attempt も FSRS 状態も書かない。`sessions.assist_json` に履歴だけ残す
- `retry_answer` で Question に戻り、draft を保持する
- 最終 Attempt に履歴を移し、`hint` または履歴がある正解は Hard
- 補助メッセージは一般化し、正解値・単位・解説を出さない
- 確実に判定できる理由だけを使う。自然言語の曖昧な回答は自動採点しない

## 17. 画像の構造検証と完全デコード

- SVG の安全性判定は文字列検索ではなく、`roxmltree` で解析した XML ツリー上の
  要素・属性許可リストで行う。`href = "..."` の空白、`xlink:href`、実体参照、
  名前空間、大文字小文字の差で検査を回避できない
- `roxmltree 0.21` は MIT / Apache-2.0、依存は `memchr` のみ。DTD と外部実体解決は
  既定で無効にし、ノード数上限を設定する
- PNG/JPEG は `image 0.25`（`default-features = false`, `png` + `jpeg` のみ）で
  完全デコードする。ライセンスは MIT / Apache-2.0。ヘッダー寸法だけで成功にしない
- デコード前に形式マジック・寸法・画素数を検証し、デコーダーにも幅・高さ・割当量の
  上限を渡す。24バイト PNG、IEND 欠落、CRC 不正、途中で切れた JPEG は Import と
  Linter の両方で拒否する
- Labeling の配置先は Hotspot と同じ `contain` 矩形変換を使う。UI 側で二重実装せず、
  `workbook-ui/src/ui_logic.rs` の純粋関数を共有する
- `seed_demo` はマーカーファイルで所有を確認し、管理下の SQLite とバックアップだけを
  削除する。任意ディレクトリの `remove_dir_all` は行わない
- 通常 Import も検証成功後に一意名の `pre-import-*.sqlite` を作る。バックアップ失敗時は
  Import しない。日次バックアップのみ7世代で、操作前バックアップは自動削除しない

## 18. Exam はコピーせず条件を保持する

- Exam は Question を複製せず、Workbook / Workbookタグ / 個別問題の条件を JSON で保存する
- 条件は `OR` グループ内の `AND`、同一条件内の複数値は `OR`。除外は包含の後に適用する
- 解決は毎回現在の教材一覧から行い、セッション開始時に `session_items` として固定する
- Review は既存の FSRS 状態・スケジューラ・兄弟抑制をそのまま共有する
- Practice と Mock は `attempts` / `mock_answers` に記録し、`review_states` を変更しない
- Mock は正解・不正解・未回答・採点なしを分離し、得点の分母は正解 + 不正解とする
- schema v4 で `exams`、`mock_answers`、`mock_results` と session の exam/time-limit 列を追加する

## 19. Exam Plan は FSRS の上に置く

- 記憶モデルを増やさず、`workbook-scheduler/src/planner.rs` を純粋関数として追加する
- 入力はスコープ内問題、FSRS要約、Attempt統計、所要時間推定、期限、1日の学習時間、
  新規上限の残り。出力は今日の候補・理由・Workbook別配分・実現可能性・警告
- 想起確率は `fsrs-rs` の `current_retrievability` を `Scheduler::retrievability_at` から
  呼ぶ。忘却曲線を自前実装しない
- 未学習は単元ブロック、既習は単元/形式が隣接しないよう混合する。単元がなければ
  優先順にフォールバックする
- 前倒し復習は `exam_recall` が低い/苦手な既習問題だけを候補にし、通常の
  `Scheduler::review` で記録する。FSRSの保存状態を直接書き換えない
- `Mode::Plan` は New/Review と同じFSRS更新経路を使い、`new_limit` の消費も
  `before_state_json IS NULL` の初回Attemptで数える。Practice/Mock は従来どおり
  FSRSを更新しない
- 計画はキャッシュしない。`ExamSummary` とセッション開始のたびに現在のSQLiteから
  再計算し、日付・範囲・時間・履歴の変更が即座に反映される

## 20. 時間予算は選択目標であり、タイマーではない

- `workbook-scheduler/src/budget.rs` の純粋関数で候補を選ぶ
- 優先順位は短期再学習 → 復習 → 新規 → Learn Ahead。既存QueueSelectorの意味を変えない
- 回答履歴の平均時間、履歴がなければ形式別初期値（5〜300秒にクランプ）
- 予算を超える最初の1問は選択する。3分でも学習を開始できるようにするため
- 回答途中の強制終了、未消化復習の完了扱い、`due_at` の書き換えはしない
- `sessions.time_budget_seconds` に保存し、schema v6で追加する

## 21. Topicは安定IDで共有し、名前で同一視しない

- `topics` はID、名前、正規化名、任意の親を持つ
- 同じ親の下でNFKC・小文字化・空白圧縮した名前が一致する場合だけ衝突として拒否する
- 別の親の同名Topicは別ID。階層なしも許可する
- Question JSONの `topics` は `/` 区切りパス。Import時に解決し、Workbook横断でIDを共有する
- Exportは任意の `topicCatalog` に安定ID・表示名・親IDを保存する。再ImportはIDを優先し、
  改名後も同じTopicへ割り当てる。catalogのない旧JSONはパス解決のみで、推測統合はしない
- 親Topicの選択は解決時に子孫へ展開する。保存時に子孫を凍結しない
- Goal/Exam Scopeが参照するTopic（include/exclude）は削除を拒否し、参照元を提示する

## 22. ScopeはExamの条件モデルを一般化して再利用する

- `ExamScope` を `StudyScope` の実体として維持し、既存JSONとExamを壊さない
- グループ内はWorkbook/Tag/TopicのAND、グループ間と個別QuestionはOR
- 除外は包含の後。空Scopeは0問
- 学習中のSessionは開始時 `session_items` を維持し、新教材は次回解決から反映する
- Scope統計は実測のみで、習得率を捏造しない

## 23. GoalはExamを移行せず適合表示する

- `goals` テーブルを追加し、`Goal::from_exam` で既存Examを一覧・Planner・統計へ適合する
- Exam行、模試回答・結果、Sessionは変更しない。模試はExam詳細に残す
- Goal Planは既存 `plan_exam` とFSRSを使う。第2の記憶モデルを作らない
- `sessions.goal_id` でGoal別の今日の学習時間を差し引く（schema v8）
- Topic別統計は対象・学習済み・未学習・期限・苦手・回答数・初回正答率だけ
- 期限不足は `Insufficient` として表示し、達成可能と偽らない

## 24. AuthoringはIDを維持して前方更新する

- 既存教材の保存は `local_id` を維持し、Note/QuestionをIDで突合する
- Note移動は先に移動先NoteとQuestionをupsertし、削除は最後に行う。移動だけで
  `review_states` を失わない
- 解説・ヒント・Topicだけの変更はFSRS状態とRevisionを保持する
- 問題文・正解・選択肢・形式・Flashcardの表裏など学習対象が変わる変更はRevisionを進め、
  `review_states` を同一Transactionで削除する。保存前にUIが対象問題を提示する
- 削除したQuestionだけがFKカスケードで状態を失う。AttemptはQuestionへのFKを持たず、
  回答時の `question_revision` を保ったまま履歴として残る
- 学習中Workbookの編集は拒否する
- 保存前にDomain ValidationとLinter、既存教材は `pre-edit` バックアップ
- ContentはMarkdown文字列とContent JSON（数式・画像・コード）の両方を編集できる
- 11形式のうち構造化編集が複雑なMatching/Hotspot/Labelingは問題JSONで安全に編集する
- JSON ExportはTopicパスとbase64画像を埋め込み、単体で再Importできる
- AI Provider・APIキー・クラウド呼び出しはスコープ外
