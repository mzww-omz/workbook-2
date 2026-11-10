# Workbook 2.5 実装進捗

作業ブランチ: `feat/workbook-2-5`

## Phase A — Product / UI Redesign

### 実装済み
- `PRODUCT.md` / `DESIGN.md` をWorkbook 2.5のコンセプトへ全面更新
- Todayに「今日の学習を始める」主操作を追加。復習 → 短期 → 新規の順で自動選択
- 中断Sessionの再開導線は従来どおり最上位
- Todayの学習時間選択（3/5/10/20分/制限なし）を追加（Phase Bで接続）

### 主要ファイル
- `PRODUCT.md`, `DESIGN.md`
- `crates/workbook-ui/src/views.rs`
- `crates/workbook-ui/src/lib.rs`

### テスト
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- コミット: `c1644bc`

### 未完了 / 次
- Goal / Exam進捗のToday表示はPhase Dで追加
- LibraryのAuthoring/Export導線はPhase Eで追加

## Phase B — Time-Budgeted Study

### 実装済み
- `workbook-scheduler::budget` を追加。短期再学習 → 復習 → 新規 → Learn Ahead の優先で予算内の項目を選択
- 回答履歴の平均時間を使い、履歴がない場合は問題形式別の保守的初期値
- 予算0は拒否。予算超過しそうな最初の1問は選択し、学習不能にしない
- `sessions.time_budget_seconds` を schema v6 で追加（前方マイグレーション + バックアップ）
- `WorkbookService::start_session_with_budget` を追加。Review予算はPlan経路で新規も混合
- Practiceの予算はFSRSを書き換えない
- StudyViewに予算目標・使用時間・残り目安・到達フラグを追加
- Study上バーに目標時間と残り目安を表示。到達しても強制終了しない
- Todayの時間選択を主操作へ接続

### 主要ファイル
- `crates/workbook-scheduler/src/budget.rs`（新規）
- `crates/workbook-scheduler/src/lib.rs`
- `crates/workbook-storage/src/schema.rs`
- `crates/workbook-storage/src/lib.rs`
- `crates/workbook-service/src/lib.rs`
- `crates/workbook-ui/src/views.rs`, `lib.rs`
- `crates/workbook-service/tests/budget.rs`（新規）
- `crates/workbook-storage/tests/storage.rs`

### テスト
- 予算境界（179/180/181/200秒）、優先順位、重複、非有限推定、20分
- 3分/20分Session、予算0拒否、回答途中で45分経過後も確定できること
- Review予算のPlan混合、PracticeのFSRS非更新、再起動後の予算保持
- v5→v6マイグレーションでSessionが保持されること

### 既知の制約
- 予算は選択目標であり、経過時間による自動終了は行わない（意図どおり）
- 実機UIでの3分/20分表示確認は未実施（Phase A〜E完了後にまとめて確認）

## Phase C — Topic / Scope System

### 実装済み
- `Topic` ドメイン型（安定ID、名前、任意の親、説明）と正規化・パス解析
- 同一親の下だけ同名を衝突扱い。別の親の同名Topicは別IDのまま
- Question JSONに任意の `topics` パス（例 `数学/一次関数`）を追加。schema v1/v2双方で省略可能
- Import時にTopicパスを解決・共有し、Workbook横断で同じTopic IDを再利用
- `topics` / `question_topics` を schema v7 で追加。v6から前方マイグレーション + バックアップ
- ScopeにTopic条件を追加（グループ内のWorkbook/Tag/TopicはAND、グループ間はOR）
- 親Topicを選ぶと子孫Topicを解決時に自動展開
- `exclude_topics` を追加。除外は包含の後に適用
- `ScopePreview`（対象・学習済み・未学習・復習期限・苦手・Workbook別・Topic別）を実測で算出
- `start_scope_plan` / `start_scope_review` / `start_scope_practice` を追加
- New教材は次回のScope解決から反映し、進行中Sessionの `session_items` は変更しない
- UI: 「範囲」画面。Topicツリー、Workbook、個別Questionの包含/除外、AND/OR切替、時間予算、開始操作
- UI: Topicの作成・改名・削除（子がある場合は拒否）、親指定
- Libraryから「範囲学習」へ遷移

### 主要ファイル
- `crates/workbook-domain/src/topic.rs`（新規）
- `crates/workbook-domain/src/exam.rs`, `workbook.rs`, `lib.rs`
- `crates/workbook-storage/src/schema.rs`, `lib.rs`
- `crates/workbook-service/src/lib.rs`
- `crates/workbook-ui/src/lib.rs`, `views.rs`, `modal.rs`
- `crates/workbook-service/tests/topics.rs`（新規）
- `crates/workbook-storage/tests/storage.rs`

### テスト
- Topicパス共有、親子解決、別親同名の非同一視、重複拒否、子あり削除拒否
- 個別QuestionのTopic割当変更
- Scopeの包含・除外・個別問題、未学習/復習/苦手の実測
- 横断ScopeのPlan開始、動的反映、進行中Sessionの固定
- 期限到来Short-termのScope復習
- schema v6→v7マイグレーション

### 既知の制約
- Topicの一括割当UIはPhase Eのエディタで扱う（現状はImportの `topics` と個別API）
- 実機UI確認は未実施

## Phase D — Goals / Advanced Planning

### 実装済み
- `Goal` ドメイン型（名前、Scope、任意の期限・目標スコア、1日の学習時間、アーカイブ、説明）
- 期限のないGoalを正式にサポート
- 既存Examを `Goal::from_exam` で読み取り専用に適合。Exam行・模試結果・Sessionは変更しない
- `goals` テーブルと `sessions.goal_id` を schema v8 で追加。v7から前方マイグレーション + バックアップ
- Goal CRUD、アーカイブ、削除（履歴・教材は保持）
- Goal削除時は安全バックアップ。試験由来Goalの削除は試験削除と同じ扱い
- 既存Plannerを再利用した `build_goal_plan`。期限・1日の学習時間・今日の学習済み時間・新規上限・想起確率・苦手・未学習を使う
- Goal Plan/Review/Practiceの開始。Sessionに `goal_id` を保存
- `GoalSummary`: 対象/学習済み/未学習/復習期限/苦手/カバレッジ/初回正答/今日の計画
- Topic別の実測統計（対象、学習済み、未学習、復習期限、苦手、回答数、初回正答率）
- 習得率・合否予測は表示しない。初回正答率は「最初の記録済み回答」のみ
- TodayにGoal/Examの進捗（期限、対象、学習済み、復習、カバレッジ）を表示
- 目標一覧をGoalとExamの統一リストへ変更。Examは「試験詳細・模試」から従来機能へ
- 範囲画面から「この範囲をGoalにする」フォーム
- Goal詳細で名前・期限・1日の学習時間の編集、計画サマリ、Topic別実測を表示

### 主要ファイル
- `crates/workbook-domain/src/goal.rs`（新規）
- `crates/workbook-storage/src/schema.rs`, `lib.rs`
- `crates/workbook-service/src/lib.rs`
- `crates/workbook-ui/src/lib.rs`, `views.rs`, `modal.rs`
- `crates/workbook-service/tests/goals.rs`（新規）
- `crates/workbook-storage/tests/storage.rs`

### テスト
- 期限なしGoal、実測カウント、Topic別集計
- 回答後の初回正答率、Goal Sessionの `goal_id`
- ExamがGoal一覧に重複せず出ること
- アーカイブ・削除後も教材が残ること
- 期限不足を `Insufficient` として表示し、達成可能と偽らないこと
- TodayのGoal brief
- 個別Question ScopeのGoal
- 1日の学習時間に応じたPlan件数
- schema v7→v8マイグレーション

### 既知の制約
- 実機UI確認は未実施
- GoalのScope編集は範囲画面から作り直す（詳細画面では名前・期限・時間のみ編集）


