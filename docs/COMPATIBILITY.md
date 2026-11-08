# FSRS-7 互換性検証

## 結論

- 旧 `ts-fsrs 6.0.0 DefaultScheduler`（FSRS-7）の出力を Golden Fixture 化し、Rust 実装と比較した
- **記憶状態（stability / stabilityFast / difficulty）、state、learningStep、reps、lapses、scheduledDays は全シナリオで一致**
- `due_at` は fsrs-rs が f32、ts-fsrs が f64 のため、長期間隔で最大 0.1% 程度の差が出る
- 短期ステップ・状態遷移・due は StepPolicy を別実装し、`allowModelOverride: false` の旧設定に合わせた
- **ビット単位の完全一致ではない**。互換性の範囲を以下に明記する

## 使用ライブラリ

| 役割 | 実装 |
|---|---|
| FSRS-7 数式 | `fsrs-rs` main branch、commit `65224f1de57c538385ecb5e4df0f2564ee6f0fb8` |
| 短期ステップ・状態 | `workbook-scheduler::StepPolicy`（本プロジェクト実装） |
| 旧互換対象 | `ts-fsrs 6.0.0` の `DefaultScheduler({ version:'FSRS-7', ... })` |

`fsrs = "6.6.2"`（crates.io）は FSRS-6 のみで、FSRS-7 の dual-trace 状態 `stabilityFast` を持たない。そのため FSRS-7 対応の main branch を commit 固定で使用した。数式の独自再実装はしていない。短期ステップとキュー選択は FSRS 数式ではないため、旧 `ts-fsrs` の middleware 挙動に合わせて独立実装した。

## 設定

```text
FSRS-7
desiredRetention: 0.9
enableShortTerm: true
learningSteps: 1m, 10m
relearningSteps: 10m
enableFuzz: false
allowModelOverride: false
graduatingInterval: 1日
maximumInterval: 36500日
```

## Golden Fixture

`fixtures/legacy-fsrs7-golden.json`（合成データのみ）

| シナリオ | 内容 |
|---|---|
| `new_good_graduation` | New→Good(10m)→Good→Good |
| `new_1`〜`new_4` | NewからAgain/Hard/Good/Easy、その後Good |
| `learning_again_then_good` | Good→Again→Good→Good |
| `learning_hard_then_good` | Good→Hard→Good |
| `learning_repeated_again` | Good→Again→Again→Good |
| `review_lapse` | 卒業後Review→Again→Relearning→Good |
| `review_lapse_twice` | RelearningでAgainを繰り返す |
| `review_hard` / `review_easy` | ReviewでHard/Easy |
| `long_sequence` | NewからGoodを8回、長期間隔 |

各ステップで次を比較:

- `stability`, `stability_fast`, `difficulty`
- `state`（New=0, Learning=1, Review=2, Relearning=3）
- `scheduled_days`, `learning_step`
- `due_at`, `last_review_at`
- `reps`, `lapses`

## 実測差

代表例（New→Easy）:

```text
ts-fsrs  due_at = 2026-02-23T20:52:05.937Z
fsrs-rs  due_at = 2026-02-23T20:51:55.503Z
差 = 10.434秒 / 53.87日 = 約 0.00022%
```

これは f32 の interval と f64 の interval の差であり、状態遷移の差ではない。Rust テストは次を許容する。

- 記憶状態・scheduledDays: 相対 `2e-4`（+ 絶対 `2e-4`）
- due_at: `5秒 + scheduled_days × 0.1%`

許容外の差が出ればテストは失敗する。長期間隔では分単位の差が出るため、「完全一致」ではなく「実用互換」と表現する。

## 短期ステップの互換ルール

旧 `DefaultScheduler` の `allowModelOverride: false` を再現した。

| 入力 | 評価 | 次 |
|---|---|---|
| New | Again | Learning, 1分, step 0 |
| New | Hard | Learning, 5.5分, step 0 |
| New | Good | Learning, 10分, step 1 |
| New | Easy | Review, FSRS間隔（最低1日） |
| Learning step 1 | Good | Review, FSRS間隔（最低1日） |
| Learning | Again | Learning, 1分, step 0 |
| Review | Again | Relearning, 10分, step 0 |
| Relearning | Hard | Relearning, 15分 |
| Relearning | Good | Review, FSRS間隔（最低1日） |
| Review | Hard/Good/Easy | Review, FSRS間隔 |

- モデル状態は評価ごとに常に FSRS-7 で更新する。ステップは due と state のみを上書きする
- `allowModelOverride=false` のため、ステップがある評価ではモデル間隔で上書きしない
- 最大間隔 36500日でクランプ

## 自動評価と互換性

自動評価は rating の決定方法だけを変える。同じ rating・回答時刻・評価前状態を与えた場合、FSRS-7 の記憶状態、state、learningStep、scheduledDays、due_at は手動評価と一致する。`workbook-service` のテストで `after_state_json` と `answered_at` を直接比較している。

自動評価が返すのは Again / Hard / Good のみで、Easy は返さない。フラッシュカードなど自動採点できない問題は手動評価へフォールバックする。

## 未確認

- 旧アプリの実ユーザー履歴によるパラメータ最適化（旧アプリも既定パラメータ）
- v2スキーマ（実在しない）
- Windows / Linux の FSRS 浮動小数点差
- 旧アプリのIntel Mac実機

## 再生成

```sh
cd scripts
npm install
npm run golden
```

生成先は `../fixtures/legacy-fsrs7-golden.json`。
