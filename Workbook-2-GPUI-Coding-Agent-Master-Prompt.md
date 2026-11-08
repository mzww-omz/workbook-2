# Workbook 2 — Rust / GPUI 全面リライト実装マスタープロンプト

> この文書全体をCodex / Pi AgentなどのCoding Agentに渡してください。既存アプリの現状確認から始め、段階的に実際のコード・移行・検証まで進めるための指示です。

あなたはRust、GPUI、SQLite、FSRS、デスクトップアプリのUXに精通したシニアエンジニア兼プロダクトデザイナーです。

既存のElectron製アプリ **Workbook** を、**Rust + GPUIで動く新しい完全ネイティブアプリ「Workbook 2」（仮称）**として再構築してください。既存機能と学習データの互換性を重視しますが、UI/UX、コード構造、責務分離は全面的に設計し直します。

既存プロジェクト: https://github.com/mzww-omz/workbook  
主対象: **macOS / Apple Silicon**  
将来: Windows、Linuxへの移植を想定したOS非依存設計  
新アプリ: **Rust + GPUI + SQLite、オフライン完結、WebView/Electronなし**

**重要: 設計案だけで終了しないでください。** 監査→互換性検証→Rustプロジェクト構築→学習エンジン→UI→移行→テスト→ドキュメントの順に、実際に動くアプリを実装してください。長い作業になった場合も、マイルストーン単位で完了させ、未完了部分は正確に報告してください。テスト結果やスクリーンショットを捏造してはいけません。

## 0. 最優先原則

1. **既存データの安全性が最優先**。既存のElectronアプリ、ユーザーDB、バックアップ、教材、履歴、FSRS状態を上書き・削除・破壊しない。新アプリは別のデータディレクトリと別のアプリIDを使用する。
2. **現行ソースが仕様の基準**。README、DESIGN、PRODUCT、DB定義、既存テスト、UI、現在のFSRSライブラリ/バージョンを最初に監査し、ここで記述する過去のv0.2.0計画との差を洗い出す。過去計画が実装済みだと決めつけない。
3. **学習ロジックは正確に保つ**。FSRS-7、短期学習、Learn Ahead、日付境界、セッション、回答履歴、復元を新UIから分離する。移植時にFSRSの数式を思いつきで再実装しない。
4. **既存のリポジトリを壊さず新アプリを作る**。実際のワークスペース構造を確認し、独立したRust workspaceとして構築する。既存Electronコードをいきなり置換しない。無関係の変更やユーザー作業を維持する。
5. **実機検証を重視**。macOSが利用できる場合はネイティブアプリを起動し、画面・日本語IME・キーボード・復旧を検証する。macOS環境がなければヘッドレステストとビルド可能な範囲まで進め、未検証項目を明記する。
6. **勝手な外部変更禁止**。push、PR作成、公開リリース、アプリ配布、既存リポジトリの削除、データ送信は明示的な依頼があるまで実行しない。ローカルの安全な編集とテストは進めてよい。
7. **確定仕様と提案を区別**。不明点は既存コード・テストで確認し、互換性に重大な影響があるときのみ質問する。通常の実装細部は合理的に判断し、設計文書に記録する。

## 1. 最初に実施する監査（M0）

対象リポジトリで少なくとも次を調べてください（パスが異なる場合は検索する）。

- `README.md`、`DESIGN.md`、`PRODUCT.md`、`AGENTS.md`（存在するもの）
- `package.json`、ロックファイル、Electronのmain/preload、IPCの定義
- `src/main/store.ts`、`src/main/index.ts`、`src/shared/domain.ts`、`src/shared/api.ts`または現行の対応箇所
- FSRSの実装・設定・学習ステップ・回答判定・キュー選択
- `review_states`、`review_logs`、`sessions`、`outcomes`、SQLiteの`user_version`/`application_id`
- JSON教材形式、Note/Question関係、知識カードの初回表示、バックアップ/復元
- 既存テスト、実装済みv0.2.0機能と未実装事項

最初の成果物として、**現行仕様 / 引き継ぐ仕様 / 改善・再設計する部分 / 未確認事項**を簡潔な表で示してください。実コードと過去の設計が衝突する場合は、データ損失を起こさない方針で判断し、重要な差異を説明してください。

既存アプリのFSRS出力を再現するため、**匿名化した合成データのGolden Fixture**（New/Learning/Review/Relearning × Again/Hard/Good/Easy × 時刻・設定）を先に用意してください。実ユーザーデータをリポジトリにコミットしてはいけません。

## 2. アーキテクチャ

新しいCargo Workspaceの推奨構成:

```text
workbook-2/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── workbook-app/       # 起動・OSウィンドウ・GPUI初期化
│   ├── workbook-ui/        # GPUI Views、ナビゲーション、テーマ、キーバインド
│   ├── workbook-domain/    # Workbook/Note/Question/Session/Attemptなどの型
│   ├── workbook-scheduler/ # FSRSアダプタ、短期ステップ、出題キュー、Clock
│   ├── workbook-storage/   # SQLite、トランザクション、移行、バックアップ
│   └── workbook-import/    # JSON・旧DBインポート、検証、プレビュー
├── assets/
├── fixtures/
└── docs/
```

- `workbook-domain`はGPUIやSQLiteに依存しない純粋なドメイン。
- `workbook-scheduler`はUIを知らず、時刻を注入できる。
- `workbook-ui`はDB/FSRSライブラリに直接アクセスせず、Application Service経由のコマンドを使う。
- GPUI `Entity`は画面上の一時状態、SQLiteは永続状態。責務を混ぜない。
- 可能な限りOS固有機能は`workbook-app`やPlatform Adapterに隔離し、後からWindows/Linuxを追加できるようにする。
- GPUIはpre-1.0でAPI変更が起こるため依存バージョンを**固定**する。GPUIコンポーネントライブラリを使う場合、異なるGPUI crateが重複する依存衝突を必ず確認する。
- Rustはstableを優先し、現行GPUIが要求する最新ツールチェーン・ネイティブ依存関係を公式資料で確認する。

## 3. プロダクトとUI/UX

設計コンセプトは **「Metro / Swiss Styleの明快さ + macOSネイティブの自然さ」**。

### デザイン上の禁止事項

- AI生成サイト風の過剰な角丸、グラデーション、装飾カード、意味のないグロー、派手なアニメーション
- 色の使いすぎ、統一されていないアイコン、不自然な自作アイコン、余白だけ大きくて情報の少ない画面
- スケジューラの内部数値を学習画面に大量表示すること
- ローディングや遷移だけが作り込まれ、学習操作が未実装のダミーUI

### 推奨デザイン

- 大きく左揃えのタイポグラフィ、明瞭な情報階層、少ない色数、アクセントCyanを限定使用
- Light / Dark / System、コントラスト配慮、減らしたモーション、状態の明確なフィードバック
- macOS標準のウィンドウ動作、自然なメニュー、正しいキーボードフォーカス
- GPUI標準要素または互換コンポーネントを活用。必要性のないアイコン自作は避ける
- 日本語・英語テキスト、日本語IMEの変換・確定・選択・コピー/ペーストを実機で確認

### 必須画面

1. **Today** — 今日の復習/新規件数、教材別内訳、途中セッションの再開、学習開始
2. **Library** — Workbook一覧、検索、詳細、JSONインポート/プレビュー、独立コピーの管理、安全な削除
3. **Study** — New/Review/Practice、問題、入力、ヒント、「わからない」、解答確認、解説、Again/Hard/Good/Easy、評価訂正、終了・再開
4. **History** — セッション履歴、異なる問題数、全回答回数、初回正答率、短期再学習回数
5. **Settings** — FSRS設定、定着率、出題上限、テーマ、データ管理、バックアップ/復元

Studyではサイドバーを隠し、本文の横幅を抑え、問題 → 回答 → フィードバック → 次へ、の操作に集中させる。複数Workbookを横断するToday学習でも、Workbookごとの結果を経て明示的に次へ進む。

### キーバインド

- `Cmd+1/2/3`: Today / Library / History
- `Cmd+,`: Settings
- `Cmd+O`: Import
- `Cmd+F`: Search
- Study中`Cmd+Return`: 回答チェック。通常のReturnはテキスト入力・IMEと競合させない
- Feedback中、入力フォーカスがないとき`1/2/3/4`: Again/Hard/Good/Easy
- `Esc`: 学習の中断/終了ダイアログ（勝手に進捗破棄しない）

すべてのキーボード操作はマウスやアクセシビリティAPIからも実行できる。テキスト編集やIME変換中にキーを誤発火させない。VoiceOverとフォーカス表示をテストする。

v1は**JSON教材のインポート・検証・プレビューを必須**にし、リッチな内蔵問題エディタはv1.1以降に回してよい。ただし現行Workbookの教材表示・解答形式は失わない。

## 4. スケジューラ：厳密な仕様

**ここはUIよりも優先度が高い。全規則をテスト可能な独立モジュールとして実装してください。**

### 確定設定

- FSRS: **FSRS-7**
- Desired Retention: 既存設定を保持、既定**90%**
- Learning Steps: **1分 → 10分**
- Relearning Steps: **10分**
- Learn Ahead Limit: **20分**（初期v1は固定、設定UI不要）
- 1日の新規問題数: 既存値を保持、既定**10問**
- 学習日の切り替え: **ローカル時刻の午前4時**

### 層の分離

1. `FsrsModel`: 安定度、難易度、忘却確率、長期復習間隔を計算
2. `StepPolicy`: New → Learning → Review、Review → Relearning → Reviewなど、短期ステップと評価に応じた状態遷移
3. `QueueSelector`: いま出す問題を決定（期限・モード・Learn Ahead）
4. `SessionPolicy`: 新規上限、教材境界、関連Note問題、スキップ、再開
5. `StudyCalendar`: UTC保存時刻とIANAタイムゾーンから午前4時の学習日を算出
6. `SchedulerService`: `Clock`を注入し、FSRS更新とDB保存を原子的に実行

Rustの`open-spaced-repetition/fsrs-rs`をFSRS計算ライブラリの第一候補にする。ただし、**これだけで既存`ts-fsrs`の`DefaultScheduler`全体と同等だと仮定しない**。学習ステップ、状態遷移、短期再学習、fuzz、最大間隔、タイムスタンプの扱いを旧実装のGolden Fixtureと比較する。FSRS数式を独自実装することは避け、必要な周辺スケジューリングポリシーのみ実装する。合致しない場合は互換性ギャップと移行方針を明文化し、実ユーザーのFSRS状態移行を止める。

### 次の問題の優先順位

次問決定のたびに、現在時刻と保存された状態から候補を**動的に**再計算する。セッション開始時の固定キューだけで完結させない。

1. `due_at <= now` のLearning/Relearning
2. モードに応じた通常問題（Reviewならセッション開始時に確定した期限到来Review、Newなら最初に選択されたNew）
3. 上記が空の場合のみ、`0 < due_at - now <= 20分` のLearning/Relearningを自動前倒し
4. なければそのWorkbookのResult画面

各区分では`due_at`が古い順、同時刻なら初期選択順・Question IDを使って安定化する。Learn Aheadは通常Reviewや未学習Newに適用しない。画面タイマーではなくCoreが時刻判定を担当する。

**Anki互換の重要な意味**: 1問しかない場合、その1問が1分後・10分後に期限到来する予定でも、ほかに問題がなく20分以内であれば即再出題してよい。最低1分待機、確認ダイアログ、前倒しの回数制限を勝手に追加しない。Ratingのない自動回答はしない。終了はいつでも可能にする。

前倒しだけで永続`due_at`を書き換えない。実際に回答を確定した時刻でFSRSを更新する。学習日がまたがる未来の短期ステップを、前日の学習へ前倒ししてはいけない（例: 午前3:55の時点で午前4:05に期限が来るステップ）。`due_at`はUTC Instantで保持し、日付境界判定のみ設定タイムゾーンで行う。

### モードと関連問題

- **New**: セッション開始時に選択した異なる新規Question（上限内）と、そのQuestionのLearning短期再出題を扱う。同一Questionの再出題を新規枚数に加算しない。Noteの知識カードは当該Noteの初回実回答前に一度だけ表示する。
- **Review**: セッション開始時の期限到来Reviewと、そこから生じた短期Learning/Relearningを扱う。セッション中に新たに期限到来した通常Reviewを勝手に追加しなくてよい。
- **Practice**: FSRS状態を一切変更せず、短期ステップ/前倒しも行わない。
- 同じNoteに属する別Questionは通常復習では1問まで扱い、残りは翌学習日まで抑制する。ただし**同一Questionの短期再出題は許可**。関連Questionの元のFSRS期限を変更しない。
- スキップはFSRSもNoteの初回学習済み状態も更新せず、そのセッションでは自動再選択しない。
- 「今日はここまで」は常に利用可能。短期カードが残っていても期限をそのまま保存して終了できる。
- 複数WorkbookのToday復習でも他Workbookのカードで割り込まない。Workbookごとの結果から手動で次へ進む。

### 評価と回答

回答の正誤とFSRS評価を区別する。デフォルトの候補:

- 不正解または「わからない」→ Again
- 正解かつヒント利用 → Hardの初期候補（ヒントが答えを直接明かす場合はより慎重に扱う）
- ヒントなし正解 → Goodの初期候補
- 易しかったと利用者が選んだ場合 → Easy

実際の既存Workbookの回答判定、ヒント、評価訂正の仕様を監査してから互換性を確定する。**忘れた問題をHard扱いしない**。FSRSにとってHardは「思い出したが難しかった」であり、不正解の代用ではない。自動的にEasyへ格上げしない。

回答処理を2段階にする:

1. `EvaluateAnswer`: 正誤と解説を準備。FSRS/DBにレビュー履歴を書かない。
2. `CommitRating(attempt_id, rating, answered_at)`: FSRS状態、Attempt、Sessionを同じSQLiteトランザクションで確定。

Feedback画面では**現在のAttemptだけ**評価・正誤の訂正を許可する。保存した「評価前FSRS状態」と元の回答時刻から再計算し、既存Attemptを修正する。新しいAttemptを追加しない。次の問題に進んだ後の過去Attempt訂正は対象外。

### 再起動・時刻・設定変更

- Clockは注入可能なinterface/traitとし、`FixedClock`で1分や20分を待たずにテストする。
- 起動し直したときは、保存したセッションカーソル（問題表示中 / 評価前 / 評価確定済みフィードバック / 次へ進んだ後）を復元する。
- 評価確定後・画面遷移前にクラッシュしても二重Attemptを作成しない。
- 新規Question上限は異なる新規問題の導入数で扱い、スキップや未確定で学習済みカウントを誤増加させない。
- Desired RetentionやFSRSパラメータの変更で、既存カードの`due_at`を無断で全件更新しない。将来のReschedule機能はプレビュー・バックアップ・明示確認付きにする。
- FSRSパラメータ・アルゴリズムバージョン・ステップポリシーのバージョンを、再現性がある形式で保存する。

## 5. DBと回答履歴

最低限の概念テーブル:

| Table | Purpose |
|---|---|
| `workbooks` | ローカルUUID単位の教材（同一JSONの別コピーは独立） |
| `notes` | 教材内の知識・問題の単位 |
| `questions` | Noteに属する個別Question |
| `review_states` | 各Questionの現在のFSRS状態、期限、状態バージョン |
| `sessions` | モード、日時、状態、現在のWorkbook・カーソル |
| `session_items` | 初期対象、スキップ、抑制、短期再出題状態 |
| `attempts` | 1回の確定回答ごとの記録 |
| `settings` | Retention、新規上限、テーマなど |

以前のWorkbookの`review_logs`はQuestion単位キー、`sessions.outcomes`も辞書だったため、同じセッションで同じQuestionを繰り返すと記録が上書きされる問題があった（現コードの改善状況は監査で確認する）。**キューに同じIDを追加するだけの対処は禁止**。

`attempts`には独立UUID、Session/Workbook/Note/Question、実回答時刻、正誤、Again/Hard/Good/Easy、ヒント/わからない/所要時間、評価前後のFSRS状態または完全再計算に必要な情報を保存する。

同一Attempt IDの二重要求は冪等。異なるAttempt IDなら同じQuestionの再回答でも新しい履歴。`session_revision`/`review_state_revision`などの期待値検証で古い同時要求を防ぐ。DBの単一トランザクションで状態・Attempt・セッションを原子的に更新する。

結果画面・統計は、**異なる問題数 / 総回答回数 / 短期再学習回数 / 初回正答率 / スキップ数**を混同しない。後の正答で初回不正解を消さない。

## 6. 旧データ移行・バックアップ

既存Electronアプリからの移行を必須機能として実装する。

1. 初回の「旧Workbookからインポート」でDBまたはバックアップを選択。
2. 元データをread-onlyで検出し、バージョン・Workbook数・履歴件数などのプレビューを表示。
3. ステージングディレクトリへ安全コピーして、SQLite integrity/foreign-key checkと形式検証。
4. v1/v2など**実際に存在する旧スキーマ**から新形式に変換。独立コピー、初回学習済み、兄弟Questionの延期、FSRS完全状態、設定、履歴、アクティブセッションを保存。
5. 旧履歴がQuestionあたり1Attemptしか保持していない場合、捏造した再出題履歴を作らず、保存済み事実から1Attemptへ移行。
6. 変換先の教材数、Question数、状態、期限、履歴、参照整合性を検証した後にのみ新アプリDBを採用。
7. 失敗時は元データと既存の新アプリDBを一切破壊せず、明確なエラーと復旧手順を表示。

旧アプリDBをその場で上書きしない。完全バックアップ、7世代の日次バックアップ、削除前/復元前の安全バックアップ、ステージング復元、失敗時のロールバックを整える。

Workbookの削除は**インポートごとのローカルUUID**で実行し、アクティブセッション中は対象削除禁止。削除前にバックアップし、教材と関連する記録を参照整合性を保って処理する。同じ教材を別にインポートしたコピーまで削除してはいけない。

## 7. 実装フェーズとDoneの条件

### M0 — ソース監査と互換性フィクスチャ

- 旧アプリの正しい機能・DB・回答仕様を確認し、監査報告と互換性テストケースを作成。
- 旧TSの評価→状態・期限の出力をGolden Fixture化。
- GPUI依存と対応OS、コンポーネント版の整合性を確認。

**Done:** 仕様差分と移行対象が整理され、移植後に何を検証するか明確。

### M1 — Core、Storage、Scheduler

- Cargo Workspaceを構築、domain、SQLite、Clock、StepPolicy、FSRS Adapter、QueueSelector、Session Commandsを実装。
- Legacy DBのテスト用フィクスチャとマイグレーション基盤を作成。
- 旧TSとのFSRS Golden Tests、20分境界、再回答・訂正・冪等性テスト。

**Done:** GUIなしでも一連の学習とセッション復元が正しく動き、FSRS互換性が検証できる。

### M2 — GPUIで学習縦断フロー

- ネイティブWindow、Today、Study、Feedback、Result、Historyの最低限の操作をつなぐ。
- New/Review/Practice、Hint、Unknown、Skip、Again/Hard/Good/Easy、訂正、Finish、Resumeを実装。
- Keyboard、IME、Focus、例外・空状態を確認。

**Done:** 実際の教材で1セッションを終了・再開でき、学習内容が保存される。ダミーボタンは残さない。

### M3 — Library、Settings、Migration

- LibraryとWorkbook詳細、検索、JSON import/validation/preview、削除。
- 旧アプリDB移行、バックアップ/復元、Retentionなどの設定、履歴集計を完成。
- 破損データ、同一教材の複数コピー、移行失敗をテスト。

**Done:** 既存ユーザーが安全に乗り換えられる。

### M4 — macOS v1完成

- Apple Silicon `.app` のビルド、macOSらしい基本挙動、Light/Dark、Japanese IME、VoiceOver、キーボード、DPI/ウィンドウサイズの確認。
- UIスクリーンショット（Today / Library / Study / Feedback / History / Settings）を実際に取得。環境制限で取得できない場合は明記。
- README、PRODUCT、DESIGN、AGENTS（存在する場合）、移行/復元手順、開発コマンドを更新。

**Done:** macOSで実用可能なv1候補を動かせる。Windows/Linuxは後続フェーズとし、未検証でも対応済みと表現しない。

## 8. 必須テスト（省略禁止）

1. FSRS-7: New/Learning/Review/Relearning × 4 Ratingsの比較フィクスチャ。
2. StepPolicy: 1m/10mの学習、10mの再学習、卒業、Againでの巻き戻り。
3. Learn Ahead: `19m59s`と`20m`は対象、`20m01s`は対象外。通常出題があると前倒ししない。
4. 1問のみの教材で即再出題可能、かつ無限ループ中でも手動終了可能。
5. 03:55→04:05の前倒し禁止、04:00境界、深夜0時通過、タイムゾーンの影響。
6. 同一Questionの複数Attemptが全件残る。重複IDの再送やRating訂正ではAttempt件数が増えない。
7. 関連Noteの別Questionは抑制、同じQuestionの短期再出題のみ許可。
8. 新規上限はユニークQuestionで数える。Practice/SkipはFSRSを更新しない。
9. 複数Workbookセッションの教材別Resultと明示的なNext。
10. 回答直後・評価後・画面遷移前など各段階のクラッシュ/再起動復元。
11. v1/v2旧DBの移行、バックアップ/復元、壊れたDB/JSONの拒否、Rollback。
12. 同一JSONの独立した複数コピーと、安全な単一Workbook削除。
13. 初回正答率、異なる問題数、Attempt数、短期再学習回数の正しい集計。
14. 連打/ショートカット競合/古いSession Revisionによる二重保存の防止。
15. macOS実機での日本語IME・日本語文字表示・アクセシビリティ・ウィンドウ挙動。

可能なら `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`cargo check --workspace`、macOSでのrelease buildを実行する。テストが失敗したら修正し、実行できないものは理由を正確に報告する。

## 9. 仕上げと最終報告

完成時には日本語で次を報告してください。

1. 実装したフェーズ・機能、未実装事項。
2. 作成・更新した主なファイルとアーキテクチャ上の重要な決定。
3. FSRS-7と旧TypeScript版の互換性検証の結果（完全一致・差分・未確認を区別）。
4. 旧DB/JSONの移行保証範囲、バックアップ/復元の動作状況。
5. `cargo fmt/clippy/test/check/build`の実際の実行結果。
6. 実画面のスクリーンショットまたは取得できない理由。
7. macOSでの起動手順、残る問題と次のマイルストーン。

**「実装しました」と言うためには、少なくとも動作するコードと実行した検証結果が必要です。** UIモックだけ、未実装のスタブだけ、説明資料だけで完了としないでください。

---

### 参考資料（アクセス可能なら参照）

- 既存Workbook: https://github.com/mzww-omz/workbook
- GPUI: https://github.com/zed-industries/zed/tree/main/crates/gpui
- GPUI Examples: https://github.com/zed-industries/zed/tree/main/crates/gpui/examples
- Rust FSRS: https://github.com/open-spaced-repetition/fsrs-rs
- TS FSRS: https://github.com/open-spaced-repetition/ts-fsrs
- Anki Scheduler: https://docs.ankiweb.net/deck-options.html
- 補助設計書: `Workbook-2-GPUI-Design-v0.1.md`（別途渡されている場合のみ）

開始してください。まずM0の監査結果を簡潔に示し、安全な新アプリの作業場所を確定してから、M1以降のコード実装へ進んでください。
