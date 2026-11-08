---
name: workbook-authoring
description: Create or revise Workbook 2 study material in the repository's workbook JSON format. Use when asked to generate a workbook, question set, quiz, drill, or study notes for Japanese middle-school subjects (国語・数学・理科・社会・英語), when converting source material into Workbook questions, or when a workbook must pass `workbook-lint` before import. Produces schemaVersion 2 JSON with bundled assets, answers, explanations, and FSRS-friendly wording.
---

# Workbook Authoring

教材を「それらしい文章」ではなく、Workbook 2 にインポートでき、Linter を通り、
学習者が実際に解ける問題集として作るための手順。

## 最初に読むもの

- `references/schema.md` — 現在の JSON スキーマ、Content、Asset、検証規則
- `references/question-types.md` — 使える問題形式と選び方
- `references/quality-rules.md` — 正解・解説・図表・FSRS の品質基準

リポジトリ直下の `docs/QUESTION-FORMATS.md` と `docs/WORKBOOK-LINTER.md` が正。
古いサンプルや記憶でスキーマを補わない。

## ワークフロー

1. **目的** — 定期テスト対策、単元導入、苦手分野の反復など、何のための教材かを1文で決める。
2. **範囲** — 学年・教科・単元・前提知識を整理する。教科書範囲外を混ぜない。
3. **学習目標** — 「説明できる」「計算できる」「資料から読み取れる」など、観察可能な形で2〜4個。
4. **問題形式** — 目標に合う形式を選ぶ。形式を増やすこと自体を目的にしない。
5. **問題と正解** — 正解を先に確定し、問題文は正解が一意になるように書く。
6. **解説** — 正解の理由、間違えやすい点、次に使う考え方を書く。答えの言い換えだけにしない。
7. **図表** — 正確性が重要な図は SVG など再現可能な形式で同梱する。外部 URL を増やさない。
8. **JSON 生成** — `schemaVersion: 2`。同梱画像は `assets` に置き、Content から `asset` で参照する。
9. **Linter** — `cargo run -p workbook-lint -- <path> --strict` を実行する。
   Error には `WB-ASSET-002`（画像の破損・MIME不一致・SVG安全性）も含まれる。
10. **Error 修正** — Error が残った教材は完成品として渡さない。画像を差し替えたら
   必ず Linter を再実行する。
11. **Warning 評価** — `--strict` でも確認し、意図的に残す Warning は理由を添える。
12. **受け渡し** — workbook JSON と `assets/` を同じディレクトリに置き、インポート可能な状態で渡す。

## 科目別の形式選び

| 教科 | 例 | 向く形式 |
|---|---|---|
| 国語 | 漢字の読み、語句、文法 | short_text, cloze, matching |
| 数学 | 計算、方程式、関数 | numeric, single_choice（途中式は Content） |
| 理科 | 計算、回路、細胞、実験 | numeric, labeling, hotspot, ordering |
| 社会 | 年表、地図、資料 | ordering, hotspot（正確な地図がある場合）, matching |
| 英語 | 語順、単語、読解 | ordering, matching, short_text |

存在しない形式を作らない。現在の形式で表現できないものは、より近い形式に置き換えるか、
制限として明記する。

## 正解の確認

- 計算は自分で計算し直す。単位換算と有効数字を確認する。
- 別解があり得るなら `accepted` / `orders` / `alternatives` に入れる。
- 正解が一意でない問題、根拠を確認できない問題は完成品から外す。
- 解説と正解が食い違っていないか読み返す。

## 図表

- 理科・社会・数学では、必要なら図を使う。装飾のための画像は入れない。
- 地図・回路図・グラフは、見た目と正解データ（座標・ラベル・値）を照合する。
- SVG は静的な図形だけにする。許可される主な要素は `svg`、`g`、`path`、`rect`、
  `circle`、`ellipse`、`line`、`polygon`、`polyline`、`defs`、`linearGradient`、
  `radialGradient`、`clipPath`、`stop`、`title`、`desc`。それ以外は Linter が Error にする。
- `script`、`foreignObject`、`iframe`、`object`、`embed`、`image`、`use`、`style`、
  イベントハンドラ属性、`href` / `xlink:href`、外部 URL、DTD、ENTITY、未知の名前空間は
  禁止。`href = "..."` のような空白挿入も XML 属性として検出される。
  `url(...)` は `url(#gradient)` のような同一 SVG 内の参照だけにする。
- PNG/JPEG はヘッダーだけのファイルや途中で切れたファイルを渡さない。Linter は全体を
  デコードし、MIME とバイト列の不一致、CRC 不正、IEND 欠落を `WB-ASSET-002` にする。
- 画像サイズは 1ファイル 5MB、合計 50MB、1画像 2,400万画素以下。SVG は 2MB 以下。
- AI 画像生成だけで正確な地図や回路を済ませない。再現可能な SVG を優先する。
- 出典・ライセンスを記録できる場合は Knowledge Card か別ファイルに残す。

## 完了条件

- `workbook-lint` の Error が 0。
- サンプルは少数の高品質な問題でよい。大量生産より、正解と解説の検証を優先する。
- 出力先の例: `.agents/skills/workbook-authoring/assets/examples/<subject>/workbook.json`

## 参照サンプル

`assets/examples/` に basic / mathematics / science / social-studies / japanese / english がある。
コピーして内容を差し替えるときも、Linter を必ず再実行する。
