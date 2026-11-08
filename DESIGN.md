# Design

## Direction

**THESIS:** 学習を「読む → 思い出す → 確認する」の一本道にする。ダッシュボードのタイルを増やさない。

**OWN-WORLD:** Metro / Swiss Style。大きな左揃えの文字、水平線、少ない色、Cyanアクセント、角丸ほぼなし。macOSの標準タイトルバーとウィンドウ操作は変更しない。

**STORY:** 起動するとToday。期限と短期再学習が見え、開始するとサイドバーが消え、1問に集中する。回答後は正誤・解説・評価を確認して次へ。教材ごとの結果で止まり、明示操作で次へ進む。

## Layout

- 通常画面: 232pxの左ナビ + 最大1120pxの本文、横56px
- Study: サイドバーなし、最大760px、上バーに終了・進捗・今日はここまで
- 角丸は2px、ダイアログのみ6px。影はダイアログだけ
- 背景はLight `#F7F8F9` / Dark `#0B0B0B`
- テキストはLight `#17191B` / Dark `#FFFFFF`
- muted `#626A70` / `#AEB4B9`、border `#DFE3E6` / `#383838`
- accent `#006D91` / `#1BA1E2`
- 正解 `#147B4C` / `#46BE7B`、不正解 `#C13741` / `#F56B6B`

## Type

System UI。見出しは 48px / Light、Todayの数値は 52px、問題文は 28px、本文 16–17px、補助 11–12px。日本語のOSフォールバックを維持する。

## Components

- 主操作: Cyan塗り
- 補助: 枠付き
- 第三: テキスト/ghost
- 学習の選択肢: 全幅ボタン、選択時にCyan
- 画像問題: 画像は最大640×380に収め、アスペクト比を維持。Hotspotはクリック位置、Labelingは正規化座標の配置先。キーボード代替として領域名／配置先ボタンを併設
- 並べ替え: ドラッグと ↑↓ ボタン。対応付け・ラベル付け: 項目選択→対応先選択、およびドラッグ
- Feedback: 左3pxの正誤バー + 「正解 / おしい / 不正解」の文字。色だけに依存しない。Near Missでは正解・解説を出さず、一般化した補助メッセージと「もう一度」だけを表示
- 評価: 自動評価ONでは「自動評価: Good」を控えめに表示し、「評価を変更」を押したときだけ `1 Again / 2 Hard / 3 Good / 4 Easy` を出す。自動評価OFFまたは自動採点不可のときは評価ボタンを必須にする
- 次へ: Feedbackの主操作。解説を読む時間を確保し、自動で進めない
- ダミーボタン、未実装の遷移、内部数値の大量表示はしない

## States

- Today: 期限到来、短期再学習待ち、完了、新規のみ、教材なし
- Library: 検索0件、教材0件
- Study: intro / question / feedback（未確定・Near Miss・確定） / workbook result / overall
- Dialog: finish / delete / import preview / restore / legacy preview / reset
- Error: 画面内の通知バー。操作を失わない

## Keyboard / Focus

`⌘1/2/3`, `⌘,`, `⌘O`, `⌘F`, `⌘Return`, `Esc`。Feedback中は入力フォーカスがないとき `1–4`。テキスト入力中は数字評価を発火しない。Study開始時は入力欄または本文ルートへフォーカス。

## Motion

装飾アニメーションは追加しない。状態変化は即時。スクロールと入力カーソル以外の動きを最小化。

## Screenshots

`docs/screenshots/` に Today / Library / Study / Feedback / Result / History / Settings / 短期再学習待ち を実機取得。
