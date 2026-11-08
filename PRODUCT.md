# Product

## Platform

macOS / Apple Silicon first. Rust + GPUI + SQLite. Offline only, no WebView, no Electron, no cloud.

## Purpose

「最初は教えてくれる。次からは思い出させる。」

教材を読み、Noteの知識カードで導入し、Questionで想起する。FSRS-7が次の復習を決め、ユーザーは評価だけを選ぶ。

## Users

個人学習者、友人同士でJSON教材を共有する人。アカウント、同期、教材エディタ、AI、通知、ゲーミフィケーションはv1の範囲外。

## Capabilities

- Today: 期限到来Review、Learning短期、新規、Workbook別内訳、次の期限
- Library: 検索、詳細、JSON import/validation/preview、独立コピー、安全な削除
- Study: New/Review/Practice、6形式、ヒント、わからない、スキップ、評価、訂正、終了、再開
- History: 異なる問題数 / 総回答 / 短期再学習 / 初回正答率 / スキップ / 学習時間
- Settings: retention、新規上限、テーマ、タイムゾーン、バックアップ、復元、旧DB移行、整合性、履歴リセット
- FSRS-7 automatic scheduling、Learn Ahead 20分、午前4日境界
- 新規は既定10問/日。Skip・Practice・未確定評価はカウントしない

## Constraints

- SQLite が唯一の永続状態
- 既存 Electron アプリとデータディレクトリを共有しない
- 回答評価のユーザー操作を自動省略しない
- Easyを自動判定しない
- 外部画像は取得しない
- v1はJSON教材の読み込みに限定し、内蔵エディタは後続
- Windows/Linuxは将来対応。v1で対応済みと表現しない

## Brand

Metro / Swiss Styleの明快さ + macOSネイティブ。左揃えの大きな文字、少ない色、Cyanの限定使用、角丸・グラデーション・装飾を抑える。

## Accessibility

キーボード操作、明確なフォーカス、色だけに依存しない正誤表示、画像alt、44px以上の主要操作。VoiceOver全画面監査は未完了。
