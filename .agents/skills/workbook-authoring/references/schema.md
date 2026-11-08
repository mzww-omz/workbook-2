# Workbook JSON schema (schemaVersion 2)

完全な仕様はリポジトリの `docs/QUESTION-FORMATS.md` を優先する。ここは生成時に
必要な最小限の形をまとめたもの。

## トップレベル

```json
{
  "format": "workbook",
  "schemaVersion": 2,
  "id": "workbook-id",
  "revision": 1,
  "title": "教材タイトル",
  "description": "",
  "language": "ja",
  "tags": ["中2", "理科"],
  "assets": [],
  "notes": []
}
```

- `format` は `workbook`。
- `schemaVersion: 1` は旧6形式だけ。新形式・`assets`・Content の `asset` 参照は v2 のみ。
- 未知のフィールドは拒否される。推測でフィールドを追加しない。
- ID は空でなく160文字以内。Workbook 内で Note ID、Question ID は一意。
- `notes` は1つ以上。各 Note に `knowledgeCard` と1つ以上の `questions`。

## Note と Question

```json
{
  "id": "note-1",
  "title": "オームの法則",
  "knowledgeCard": "## V = RI\n...",
  "questions": []
}
```

```json
{
  "id": "q-1",
  "revision": 1,
  "type": "numeric",
  "prompt": "電圧6V、抵抗3Ωのとき電流は？",
  "explanation": "I = V / R = 6 / 3 = 2A です。",
  "answer": { "value": 2, "unit": "A" }
}
```

`type` は `single_choice` / `true_false` / `multiple_choice` / `short_text` /
`cloze` / `flashcard` / `numeric` / `ordering` / `matching` / `hotspot` / `labeling`。

## Content

文字列は Markdown として扱う。ブロックが必要なときは配列にする。

```json
"prompt": [
  { "type": "markdown", "text": "次の図を見て答えなさい。" },
  { "type": "image", "asset": "cell-diagram", "alt": "動物細胞の模式図", "caption": "図1", "fit": "contain" },
  { "type": "math", "text": "V = RI" },
  { "type": "code", "text": "let x = 1;", "language": "rust" }
]
```

- `image` は `url` と `asset` の**どちらか一方だけ**。
  - `url`: 既存 v1 互換の HTTPS URL。Workbook 2 は外部取得をしないため、表示は
    オフラインのプレースホルダになる。教材画像には使わない。
  - `asset`: 同梱画像の ID。`assets` に存在しなければならない。
- `alt` は必須。空文字は拒否。何が描かれているか分かる説明にする。
- `caption` は任意。`fit` は `contain`（既定）または `cover`。
- Markdown / Math / Code / Image は同じ Content 内で混在できる。

## 同梱画像 (assets)

```json
"assets": [
  {
    "id": "circuit",
    "alt": "電池と抵抗の直列回路",
    "mediaType": "image/svg+xml",
    "file": "assets/circuit.svg"
  },
  {
    "id": "cell",
    "alt": "植物細胞の模式図",
    "mediaType": "image/png",
    "data": "iVBORw0KGgo..."
  }
]
```

- `file` は workbook JSON からの相対パス。`..`、絶対パス、ドライブ文字は拒否。
- `data` は base64。`file` と `data` はどちらか一方。
- `mediaType` は `image/png` / `image/jpeg` / `image/svg+xml`。
- 1ファイル 5MB、合計 50MB、最大50個。PNG/JPEG は実サイズ、SVG は静的要素を検証。
- SVG は `script`、外部参照、`foreignObject`、`use`、`image`、`style`、イベント属性を拒否。
- 学習時はファイルパスを読まず、インポート時に SQLite へ保存したバイト列を使う。
  バックアップ／復元にも含まれる。

## 回答仕様の要点

| type | answer |
|---|---|
| single_choice | `{"optionId":"a"}` |
| multiple_choice | `{"optionIds":["a","b"]}` |
| true_false | `true` / `false` |
| short_text | `{"accepted":["答え","別解"]}` |
| cloze | `{"blanks":{"blank-id":["答え"]}}` |
| numeric | `{"value":3,"unit":"A","tolerance":0.001,"significantFigures":2,"acceptedUnits":{"mA":0.001},"requireUnit":true}` |
| ordering | `{"orders":[["id1","id2"]]}` |
| matching | `{"pairs":[{"left":"l","right":"r"}],"alternatives":[]}` |
| hotspot | `{"regionIds":["r1"]}` |
| labeling | `{"pairs":{"target":"label"},"alternatives":[]}` |

Numeric の `acceptedUnits` は「その単位の1単位が正解単位で何になるか」の倍率。
例: 正解 `cm` に対する `m: 100` は妥当。`kg: 100` は次元が違うため Error。

## Linter

```bash
cargo run -p workbook-lint -- path/to/workbook.json
cargo run -p workbook-lint -- path/to/dir --format json
cargo run -p workbook-lint -- path/to/dir --strict
```

- ディレクトリは `workbook.json`、または直下の `.json` が1つだけならそれを使う。
- Error が1つでもあれば終了コード1。`--strict` は Warning も1にする。
- JSON 出力は `code` / `severity` / `message` / `path` / `suggestion` を持つ。
