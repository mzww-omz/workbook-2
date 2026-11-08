# Workbook Linter

`workbook-lint` は、Workbook JSON をアプリへインポートする前に静的検証する
GUI 不要の CLI です。単なる JSON Schema 検証ではなく、教材としての妥当性と
著作品質を診断します。

## 構成

```text
workbook-domain   構造・意味制約の正本（Importer と同じ検証）
workbook-import   同梱画像のパス・MIME・実サイズ検証
workbook-lint     追加の教材品質チェック、診断コード、CLI、JSON出力
```

Domain の `Workbook::validate` と `Content::validate_with_assets` を再利用します。
Importer と Linter に別々の構造検証を実装しません。Linter は Domain で検出できる
Error をそのまま報告し、その上で Numeric / Hotspot / Content / 教材品質の
Warning を追加します。

## 使い方

```bash
# 単一ファイル
cargo run -p workbook-lint -- ./my-workbook/workbook.json

# アセットを含むディレクトリ（workbook.json または .json が1つだけ）
cargo run -p workbook-lint -- ./my-workbook/

# 自動処理向け JSON
cargo run -p workbook-lint -- ./my-workbook/ --format json

# CI: Warning も失敗にする
cargo run -p workbook-lint -- ./my-workbook/ --strict
```

終了コード:

| code | 意味 |
|---|---|
| 0 | Error なし（`--strict` では Warning もなし） |
| 1 | Workbook の構造・内容・アセット検証エラー、JSON 構文エラー、`--strict` 時の Warning |
| 2 | CLI 引数エラー、入力ファイル／ディレクトリの IO エラー、JSON 出力自体の失敗 |

判定は「Workbook を読めたか」で行います。

- `WB-JSON-001`（JSON 構文不正）と `WB-JSON-002`（サイズ超過）は、ファイルを
  読めたうえでの教材検証エラーなので **1**。
- `WB-IO-001`（存在しないパス、読み取り不能、JSON が無いディレクトリ、複数候補）は
  **2**。診断 JSON 自体は同じ形式で出力します（`version: 1`、`ok: false`）。
- `--format json` のシリアライズに失敗した場合も **2**。

## 診断形式

人間向け出力:

```text
./my-workbook/workbook.json
WARNING WB-HOT-002
  notes[1].questions[0].hotspot.regions[2].shape
  領域が非常に小さく（面積 0.0012）、画面上で選びにくい可能性があります
  提案: 領域を広げるか、別の画像で示してください

結果: 0 error, 1 warning, 0 info
OK: インポート可能です
```

JSON 出力は次の安定した形です。`version` は診断形式のバージョンで、Workbook の
`schemaVersion` とは別です。

```json
{
  "version": 1,
  "path": "./my-workbook/workbook.json",
  "ok": false,
  "summary": { "errors": 1, "warnings": 2, "infos": 0 },
  "diagnostics": [
    {
      "code": "WB-ASSET-001",
      "severity": "error",
      "message": "assets に存在しない画像IDです: cell-diagram",
      "path": "notes[0].questions[0].prompt[1].asset",
      "suggestion": "assets に画像を追加するか、参照IDを修正してください"
    }
  ]
}
```

- `code`: 将来変更しにくい安定した診断コード。
- `severity`: `error` / `warning` / `info`。
- `path`: Workbook 内の位置。JSON Pointer ではなく、既存 Validator と同じ
  `notes[0].questions[1].answer...` 形式。
- `suggestion`: 安全に言える場合だけ。意味の推測が必要な場合は省略。

## 診断コード

### 構造・スキーマ

| code | severity | 内容 |
|---|---|---|
| `WB-JSON-001` | error | JSON 構文エラー |
| `WB-JSON-002` | error | JSON が 10MB を超える |
| `WB-SCHEMA-001` | error | 未対応の `schemaVersion` |
| `WB-SCHEMA-002` | error | v1 で v2 専用の形式・アセットを使用 |
| `WB-VALIDATION-001` | error | Domain のその他の構造・意味検証エラー |
| `WB-IO-001` | error | ファイル／ディレクトリを読めない |
| `WB-ANSWER-001` | error | 回答仕様の不整合 |
| `WB-FORMAT-001` | error | Matching / Labeling の対応関係の不整合 |

### アセット

| code | severity | 内容 |
|---|---|---|
| `WB-ASSET-001` | error | 存在しない Asset 参照、重複 ID、読込不能 |
| `WB-ASSET-002` | error | ファイル欠落、パス逸脱、MIME 不一致、破損、SVG 安全性 |
| `WB-ASSET-003` | warning | 未使用アセット |
| `WB-ASSET-004` | warning | アセットの alt が短すぎる |

### Numeric

| code | severity | 内容 |
|---|---|---|
| `WB-NUM-006` | error | `acceptedUnits` の単位次元が正解と矛盾 |
| `WB-NUM-007` | warning | `acceptedUnits` の倍率が既知の単位表と不一致 |
| `WB-NUM-008` | warning | tolerance が極端に大きい |

NaN / Infinity、0除算、不正な分数、tolerance、significantFigures の基本検証は
Domain が行い、`WB-ANSWER-001` または `WB-VALIDATION-001` として報告されます。
教材独自の単位は、既知の単位表にない限り拒否しません。

### Hotspot

| code | severity | 内容 |
|---|---|---|
| `WB-HOT-001` | error | 座標範囲外、面積0、図形不正、正解領域の欠落など |
| `WB-HOT-002` | warning | 領域が非常に小さい |
| `WB-HOT-003` | warning | 領域が極端に細い |
| `WB-HOT-004` | error | 多角形の自己交差 |
| `WB-HOT-005` | warning | キーボード用の代表点を求められない |
| `WB-HOT-006` | warning | 領域図形が完全に重複 |

旧 20×20 グリッドの制限は使いません。小さくても有効な領域は Error にせず、
操作性の低さを Warning にします。

### Content / Math

| code | severity | 内容 |
|---|---|---|
| `WB-CONTENT-001` | error | 空の Markdown ブロック |
| `WB-CONTENT-004` | warning | Content 画像の alt が短すぎる |
| `WB-CONTENT-006` | warning | 1つの Content が極端に長い |
| `WB-CONTENT-008` | info | Code の言語名が空 |
| `WB-MATH-001` | warning | `$` の対応が怪しい |
| `WB-MATH-002` | warning | 波かっこの対応が怪しい |
| `WB-MATH-003` | warning | `\left` と `\right` の数が違う |

数式の意味が正しいかどうかは、根拠なく断定しません。

### 教材品質

| code | severity | 内容 |
|---|---|---|
| `WB-QUALITY-001` | warning | 選択肢の内容が重複 |
| `WB-QUALITY-002` | warning | 正解だけ極端に長く推測しやすい |
| `WB-QUALITY-003` | warning | 解説が極端に短い |
| `WB-STYLE-001` | warning | ID・タイトル・タグの前後に不要な空白 |

ヒューリスティックな品質警告はデフォルトで Error にしません。誤検知があり得る
ため、`--strict` で扱いを決めます。

## 自動修正

今回のリリースではファイルを書き換える `--fix` は提供しません。将来の安全な
自動修正のために `fix_candidates` API を用意し、現在は `WB-STYLE-001`（前後の
空白）だけを対象候補にしています。

次の値は自動修正しません。

- 正解、accepted、orders、pairs
- 解説、問題文の意味
- 単位、有効数字、tolerance
- 図形座標、Hotspot の領域
- アセットの内容

自動修正を追加する場合は、デフォルトで元ファイルを変更せず、dry-run と差分確認を
必須にします。

## CI への組み込み

```bash
cargo run -q -p workbook-lint -- ./workbooks --format json --strict > lint.json
```

`ok: false`、終了コード 1（教材の問題）または 2（入力の IO 問題）で失敗させます。
JSON は `version` と `diagnostics[].code` を契約として扱えます。終了コード 2 でも
`WB-IO-001` を含む同じ形の JSON を出力するため、ログから原因を区別できます。

## 制限

- 教材内容の意味的な正しさは自動証明しません。計算・図の整合性は作者が確認します。
- 画像の内容（何が描かれているか）は検証しません。形式・安全性・サイズのみです。
- Hotspot の重なりは採点上許容されます。Linter は完全一致の重複だけを Warning にします。
- 自然言語の自由記述は採点しません。
- 自動修正は未実装です（API の設計のみ）。

## 関連

- Content / Asset の仕様: [`QUESTION-FORMATS.md`](QUESTION-FORMATS.md)
- AI 向け作成手順: `.agents/skills/workbook-authoring/SKILL.md`
- サンプル: `.agents/skills/workbook-authoring/assets/examples/`
