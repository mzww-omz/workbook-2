# 問題形式リファレンス（schemaVersion 2）

既存の v1 6形式（single_choice / true_false / multiple_choice / short_text / cloze /
flashcard）は変更していません。v2 は v1 の全形式を含み、次の5形式と画像アセット、
Near Miss の補助情報を追加します。

```json
{
  "format": "workbook",
  "schemaVersion": 2,
  "id": "workbook-expanded-sample",
  "revision": 1,
  "title": "拡張問題形式サンプル",
  "description": "",
  "language": "ja",
  "tags": ["サンプル"],
  "assets": [],
  "notes": []
}
```

- `schemaVersion: 1` は v1 の6形式だけを受け付けます。v2 の形式や `assets`、Content の
  `asset` 参照を含むとインポートを拒否します。
- `schemaVersion: 2` は v1 の6形式もそのまま受け付けます。
- 未知のフィールド、未知の `type`、重複 ID、回答仕様の不整合はインポート時に拒否します。

## 回答の保存形式

Attempt には次の JSON が保存されます。表示テキストではなく ID と正規化座標だけを
保存するため、後からラベルや並びを変えても履歴を再現できます。

| 形式 | `Answer` | 保存例 |
|---|---|---|
| numeric | `Text` | `"3.0A"` |
| ordering | `Ordering { order }` | `{"order":["i","have","never","seen","this"]}` |
| matching | `Matching { matches }` | `{"matches":{"h":"hydrogen","o":"oxygen"}}` |
| hotspot | `Hotspot { x, y }` | `{"x":0.66,"y":0.57}` |
| labeling | `Labels { labels }` | `{"labels":{"nucleus":"l-nucleus"}}` |

## Numeric

```json
{
  "id": "q-numeric",
  "revision": 1,
  "type": "numeric",
  "prompt": "電圧6V、抵抗2Ωのとき、電流は何Aですか。有効数字2桁、単位Aを付けて答えてください。",
  "answer": {
    "value": 3,
    "unit": "A",
    "tolerance": 0.001,
    "significantFigures": 2,
    "acceptedUnits": { "mA": 0.001 },
    "requireUnit": true
  },
  "explanation": "I = V / R = 6V / 2Ω = 3A です。"
}
```

- `value` は数値または分数文字列（`"3/2"`）。
- `tolerance` は正解の単位で表した絶対許容誤差。省略時は `1e-9 × max(1, |正解|)`。
  単位変換がある場合は、値と同じ基準単位へ換算してから比較します。
- `significantFigures` は 1〜15。指定時は入力の表記から有効数字を数え、完全一致を要求。
  例: 2桁指定で `3m` は Near Miss、`3.0m` は正解。
- `acceptedUnits` は「単位記号 → 正解の単位への倍率」。ここに載っている単位だけが
  正解に使えます。`3000mA` は `mA: 0.001` があるときだけ `3A` と同値になり Correct。
- `requireUnit` が true で単位がない場合は Near Miss（`missing_unit`）。
- 分数は `3/2`、`-3/4`、指数は `3e2` を受け付けます。`3/0`、`--3`、`3..0`、`NaN`、
  `inf` は Near Miss（`format_error`）で、正解にはなりません。
- 数値の同値判定は文字列一致ではなく、単位変換後の値と許容誤差で行います。
- `3A` と `3.0A` は有効数字指定がなければ同じ値として正解です。
- 正解にない単位でも、同じ次元で換算値が一致する場合は Near Miss
  （`equivalent_expression`）。次元が違う場合は Near Miss（`wrong_unit`）。

### 単位換算

単位変換は「正解の単位」と「入力の単位」を共通の基準単位へそろえて比較します。

```text
input_base    = input_value  × input_unit_factor
expected_base = expected_value × expected_unit_factor
```

- `known_unit()` は次元ごとの基準単位への倍率だけを持ちます（`cm` → m、`mA` → A）。
  診断と、`acceptedUnits` にない同次元単位の比較に使います。
- `acceptedUnits` の倍率は**正解の単位**に対する倍率です。基準単位へは
  `input_value × accepted_factor × expected_unit_factor` で換算します。
- `acceptedUnits` に明示された単位で換算値が一致 → **Correct**。
- `acceptedUnits` にない同次元の単位で換算値が一致 → **Near Miss
  （`equivalent_expression`）**。正解にはしません。
- 次元が異なる単位 → Correct にはせず Near Miss（`wrong_unit`）。数値まで違う場合は
  Incorrect。
- 正解の単位が `known_unit()` にない場合、共通の基準単位がないため正解の単位空間で
  比較します。この場合も `acceptedUnits` は正解の単位への倍率として機能します。

例（正解 `3 cm`）:

| 入力 | 結果 |
|---|---|
| `3cm` | Correct |
| `3m` | Incorrect（3 m は 3 cm ではない） |
| `0.03m` | Near Miss（`equivalent_expression`） |
| `30mm` | Near Miss（`equivalent_expression`） |
| `3kg` | Correct にはならない（`wrong_unit`） |

### 有効数字

- 仮数部だけを数え、指数部（`e`/`E` 以降）の数字は含めません。
- 正負の符号は含めません。
- 先頭の 0 は含めません。小数点以下の末尾の 0 は含めます。
- `3e2` は 1 桁、`3.0e2` は 2 桁、`3.00e-2` は 3 桁、`0.0030` は 2 桁です。
- 整数末尾の 0 は書かれたとおりに数えます（`300` は 3 桁）。曖昧さを避ける場合は
  指数表記を使います（`3e2` は 1 桁）。
- すべて 0 の仮数部は、小数点以下の 0 の個数を有効数字とします
  （`0` と `0.0` は 1 桁、`0.00` は 2 桁）。
- 値の一致判定と有効数字の判定は分離しています。値が一致し、有効数字だけが違う場合に
  Near Miss（`significant_figures`）を返します。
- 分数の有効数字は分子と分母のうち桁数が多い方を使います（`3.0/2.00` は 3 桁）。

## Ordering

```json
{
  "id": "q-ordering",
  "revision": 1,
  "type": "ordering",
  "prompt": "次の語を並べ替えてください。",
  "options": [
    { "id": "i", "content": "I" },
    { "id": "have", "content": "have" },
    { "id": "never", "content": "never" }
  ],
  "initialOrder": ["never", "i", "have"],
  "answer": { "orders": [["i", "have", "never"]] },
  "explanation": "I have never ... の語順です。"
}
```

- `options` は 2〜12個、ID は一意。
- `initialOrder` は省略可能。省略時は `options` の順で表示。全 ID を1回ずつ指定。
- `orders` は許される正解順序の配列。1〜20個。各順序は全 ID を1回ずつ含む。
- 採点は完全一致。部分一致は診断情報（一致した位置の数）として返し、正解にはしない。
- 保存するのは ID 配列のみ。
- 選択肢の `content` は Text / Markdown / Image / Math / Code をそのまま描画します。
  画像・数式・長文でも行が崩れないよう、共通コンテンツレンダラーをコンパクト表示で
  使います。

## Matching

```json
{
  "id": "q-matching",
  "revision": 1,
  "type": "matching",
  "prompt": "元素記号と元素名を対応付けてください。",
  "matching": {
    "left": [
      { "id": "h", "content": "H" },
      { "id": "o", "content": "O" }
    ],
    "right": [
      { "id": "hydrogen", "content": "水素" },
      { "id": "oxygen", "content": "酸素" }
    ]
  },
  "answer": {
    "pairs": [
      { "left": "h", "right": "hydrogen" },
      { "left": "o", "right": "oxygen" }
    ],
    "alternatives": []
  },
  "explanation": "H=水素、O=酸素です。"
}
```

- 左右は 2〜12個で同数。左右を含めた全 ID が一意。
- `content` は文字列または Content ブロック。画像・数式ブロックも記述できます。
- `pairs` は左右の全項目を1回ずつ使う全単射。
- `alternatives` に別解の全単射を最大20個まで指定できます。
- 採点は完全一致。一致した組の数を診断情報として返す。
- 同じ右項目への重複割当ては UI で置換、データ検証では拒否。
- 左右の項目は Content をそのまま描画し、選択中は `●`、割当て済みは `✓` と
  対応する相手を併記します。ID ベースの保存形式は変わりません。

## Hotspot

```json
{
  "id": "q-hotspot",
  "revision": 1,
  "type": "hotspot",
  "prompt": "地図の中で関東地方を選んでください。",
  "hotspot": {
    "asset": "japan-map",
    "regions": [
      {
        "id": "kanto",
        "alt": "関東",
        "shape": {
          "type": "polygon",
          "points": [[0.625, 0.5125], [0.727, 0.5625], [0.711, 0.7]]
        }
      }
    ]
  },
  "answer": { "regionIds": ["kanto"] },
  "explanation": "関東地方は本州の東側です。"
}
```

- 座標は画像の左上を (0,0)、右下を (1,1) とする正規化座標。ピクセル座標は保存しない。
- 図形は `circle`（`cx`,`cy`,`r`）、`rect`（`x`,`y`,`w`,`h`）、`polygon`（3〜100点）。
  すべて 0〜1 の範囲、面積 0 の多角形は拒否。
- `regionIds` は正解として受け付ける領域。複数指定した場合は、そのどれかを選べば正解。
- 画像は `assets` の ID を参照。
- クリック位置は実際のポインター座標から画像表示領域に対する相対位置を求め、
  0〜1 に正規化して Attempt に保存します。20×20 などの固定グリッドは使いません。
  画像を `contain` で表示したときの余白（レターボックス）は画像外として扱い、
  回答にしません。座標は GPUI の論理ピクセルで計算するため、Retina/HiDPI でも
  同じ正規化値になります。
- キーボード／スクリーンリーダー向けに領域名ボタンを表示。ボタンは領域内の代表点を選びます。
  代表点は円・矩形の中心、多角形は面積重心→走査線上の最も広い内部区間の順に求め、
  最後に `shape.contains(x, y)` で内部にあることを検証します。内部点を証明できない
  図形ではボタンを出さず、座標を捏造しません。
- 領域が重なる場合、採点はクリック位置を含む領域の集合だけを見ます。配列順は結果に
  影響せず、正解領域のどれかを含めば Correct です。正解でない領域だけを含む位置は
  Incorrect です。

## Labeling

```json
{
  "id": "q-labeling",
  "revision": 1,
  "type": "labeling",
  "prompt": "植物細胞の図にラベルを配置してください。",
  "labeling": {
    "asset": "plant-cell",
    "targets": [
      { "id": "nucleus", "x": 0.422, "y": 0.45, "alt": "中央の丸い構造" }
    ],
    "labels": [
      { "id": "l-nucleus", "content": "核" }
    ]
  },
  "answer": { "pairs": { "nucleus": "l-nucleus" } },
  "explanation": "核は中央の丸い構造です。"
}
```

- `targets` は配置先。位置は正規化座標、`alt` は必須。
- `labels` は Content。画像・数式も使えます。
- 配置先とラベルは 1〜50個で同数。ID はそれぞれ一意。
- `pairs` は全配置先を1回ずつ使う全単射。`alternatives` で別解を最大20個。
- 採点は完全一致。一致した配置数を診断情報として返す。
- 同じラベルの二重配置は UI で移動、データ検証では拒否。
- ラベルは Content をそのまま描画します。ドラッグ＆ドロップと、ラベルを選んでから
  配置先をクリックする操作の両方を維持し、配置済みは `✓` で示します。画像上の
  配置先にもコンパクト表示し、キーボード向けの配置先ボタンには読み上げ用ラベルを
  付けます。保存形式は ID のままです。

## Content内の画像

Markdown / Math / Code / Image は `Content` のブロックとして混在できます。画像は
HTTPS URL と教材同梱アセットのどちらかを、型として明示して指定します。文字列の
接頭辞や拡張子で判別しません。

```json
{
  "type": "image",
  "asset": "cell-diagram",
  "alt": "動物細胞の模式図",
  "caption": "図1 細胞のつくり",
  "fit": "contain"
}
```

```json
{
  "type": "image",
  "url": "https://example.org/diagram.png",
  "alt": "外部の図",
  "fit": "cover"
}
```

- `asset` と `url` は**どちらか一方だけ**。両方または両方なしは JSON 検証で拒否します。
- `asset` は `assets` に存在する ID でなければなりません。存在しない ID はインポート時に
  拒否します。`asset` は `schemaVersion: 2` でのみ使用できます。
- `url` は既存 v1 互換の HTTPS URL です。Workbook 2 は外部取得を行わないため、表示は
  オフラインのプレースホルダになります。教材画像には `asset` を使ってください。
- `alt` は必須です。空文字は拒否します。`caption` は任意、`fit` は `contain`（既定）と
  `cover` に対応します。
- 同梱画像はインポート時に SQLite の `assets` へ保存され、学習時はファイルパスや
  ネットワークを参照しません。バックアップ／復元にも含まれます。
- 共通レンダラーが問題文・選択肢・Ordering・Matching・Labeling・解説・Knowledge Card の
  すべてで同じ Content を描画します。画像はアスペクト比を維持し、`contain` / `cover` を
  尊重します。デコード結果はワークブック ID とアセット ID 単位でキャッシュします。
- 画像の読み込みに失敗した場合は、alt を含むテキストのフォールバックを表示します。
  正常に表示できる画像を alt だけで置き換えることはしません。

## 画像アセット

```json
"assets": [
  {
    "id": "plant-cell",
    "alt": "植物細胞の模式図",
    "mediaType": "image/png",
    "data": "iVBORw0KGgo..."
  },
  {
    "id": "japan-map",
    "alt": "日本の地方を色分けした地図",
    "mediaType": "image/svg+xml",
    "file": "images/japan-regions.svg"
  }
]
```

- `file` は教材 JSON からの相対パス。`..`、絶対パス、ドライブ文字、空要素を拒否。
  インポート時に canonicalize し、教材フォルダの外へ出る symlink を拒否。
- `data` は base64。`file` と `data` はどちらか一方だけ。
- `mediaType` は `image/png`、`image/jpeg`、`image/svg+xml` のいずれか。
  SVGはスクリプト・外部参照・`<foreignObject>`・`<use>`・`<image>`・`style`・
  イベントハンドラ・未知の要素を含まない静的SVGだけを受け付けます。GIF/WebPは拒否。
- 1ファイル最大 5MB、合計 50MB、最大 50個。PNG/JPEGはヘッダ、SVGはルート要素から
  実サイズを検証します。デコード後のメモリを抑えるため、1画像あたり最大 2,400万画素
  （例: 6000×4000）を上限とします。
- インポート時に画像バイトを SQLite の `assets` テーブルへ保存する。学習時はファイル
  パスを読まない。バックアップ／復元にも画像が含まれる。
- 既存の Content の HTTPS 画像ブロックは従来どおり変更しない（外部取得はしない）。

## Near Miss と再回答

採点結果は次の4状態です。

| 状態 | 意味 | FSRS |
|---|---|---|
| `correct` | 正解 | 自動評価の対象 |
| `near_miss` | おしい。形式・単位・有効数字など特定できる差分 | **確定しない** |
| `incorrect` | 不正解 | Again |
| `manual` | 自動採点不能 | 手動評価 |

Near Miss の理由は構造化して `reasons` に保存します。

- `significant_figures`
- `missing_unit`
- `wrong_unit`
- `equivalent_expression`
- `incomplete_answer`
- `format_error`

`minor_typo` は将来用に定義していますが、誤判定を避けるため今回の採点では使いません。

Near Miss のとき:

1. `sessions.assist_json` に履歴（回答、理由、時刻）を追記する。
2. Attempt も FSRS 状態も書かない。
3. Feedback には一般化した補助メッセージだけを表示し、正解・解説を表示しない。
4. 「もう一度」で `retry_answer` を呼び、Question に戻る。入力中の回答は保持する。
5. 再回答で正解した場合、`hint` または Near Miss 履歴があるため自動評価は **Hard**。
6. 最終 Attempt の `assist_json` に履歴を保存する。`attempts.correct` は最終回答の正誤。
7. Easy を自動付与しない。不正解は Again。

## 制限

- Hotspot のクリックは画像表示領域に対する正規化座標で保存します。ピクセル座標は
  保存しません。画像外（`contain` の余白）のクリックは回答になりません。
- 複合単位（`m/s`、`kg·m/s²`）は未対応。単一の単位記号のみ。
- 温度のオフセット変換（°C ↔ K）は未対応。
- 自然言語の自由記述に対する意味的な自動採点は行わない。short_text は従来の正規化
  一致のまま。
- Hotspot の多角形は凹形・U字形・細長い形でも判定でき、領域名ボタンは走査線で
  内部点を求めます。NaN/Infinity、範囲外の座標、3点未満、面積 0 の図形は拒否します。
  内部点を証明できない図形では領域名ボタンを表示しません。
- 重複領域は許容します。採点はクリック位置を含む正解領域の有無だけを見るため、
  `regions` の配列順で正誤は変わりません。
- 画像の内容（何が描かれているか）の自動検証はしない。PNG/JPEG/静的SVGとしての安全性と
  サイズだけを検証する。
- Content の `image` は `url`（HTTPS・外部取得なし）と `asset`（教材同梱）の両方に対応
  します。教材同梱画像は Content、Hotspot、Labeling のどこからでも同じ `assets` ID を
  参照します。外部 URL の取得は行いません。
- 拡張サンプルの地図は「地球地図日本（国土地理院）」を加工したものです。
  `assets/japan-regions.NOTICE.md` を参照してください。
