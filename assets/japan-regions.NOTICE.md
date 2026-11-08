# 拡張サンプル地図の出典

`assets/japan-regions.svg` と `assets/japan-regions.json` は、国土地理院の
「地球地図日本」を dataofjapan/land が GeoJSON に変換したデータを加工して
作成しています。

- 出典: 地球地図日本（国土地理院）
- 元データ: https://github.com/dataofjapan/land
- 加工: `scripts/generate-japan-map.py` による投影・簡略化・地方色分け・輪郭抽出
- ライセンス: 配布元の条件に従います。非営利利用では出典明記、営利利用では
  出典明記と著作権者への利用報告が必要です。

アプリ本体のコードは MIT ライセンスですが、このサンプル地図データは上記の
条件が適用されます。サンプルを再配布する場合はこの NOTICE を同梱してください。
