#!/usr/bin/env python3
"""Regenerates assets/sample-v2.workbook.json with its bundled images.

The cell diagram is drawn deterministically as PNG. The Japan map is the
committed assets/japan-regions.svg, generated from Global Map Japan by
scripts/generate-japan-map.py. No external assets are downloaded at runtime.
"""

import base64
import io
import json
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "sample-v2.workbook.json"


def png_bytes(image):
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=True)
    return base64.b64encode(buffer.getvalue()).decode("ascii")


def plant_cell():
    image = Image.new("RGB", (640, 400), "#f7f8f9")
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((20, 20, 620, 380), radius=36, fill="#e9efd9", outline="#17191B", width=4)
    draw.rounded_rectangle((42, 42, 598, 358), radius=28, fill="#f4f7ea", outline="#626A70", width=2)
    draw.ellipse((215, 125, 325, 235), fill="#c9d9ea", outline="#17191B", width=3)
    draw.ellipse((225, 135, 245, 155), fill="#6f86a6")
    draw.ellipse((385, 92, 565, 300), fill="#dbe8f2", outline="#626A70", width=2)
    for box in [(88, 84, 156, 122), (96, 292, 164, 330), (456, 64, 524, 102), (486, 318, 554, 356)]:
        draw.ellipse(box, fill="#94b86e", outline="#17191B", width=2)
    return image


def content(text):
    return text


def note(note_id, title, card, questions):
    return {"id": note_id, "title": title, "knowledgeCard": card, "questions": questions}


def question(question_id, kind, prompt, explanation, **extra):
    payload = {
        "id": question_id,
        "revision": 1,
        "type": kind,
        "prompt": prompt,
        "explanation": explanation,
    }
    payload.update(extra)
    return payload


def build():
    map_asset = base64.b64encode(
        (ROOT / "assets" / "japan-regions.svg").read_bytes()
    ).decode("ascii")
    map_regions = json.loads(
        (ROOT / "assets" / "japan-regions.json").read_text(encoding="utf-8")
    )["regions"]
    cell_asset = png_bytes(plant_cell())

    workbook = {
        "format": "workbook",
        "schemaVersion": 2,
        "id": "workbook-expanded-sample",
        "revision": 1,
        "title": "拡張問題形式サンプル",
        "description": "数値・並べ替え・対応付け・画像領域・ラベル付けを、実際に回答して確認できます。",
        "language": "ja",
        "tags": ["サンプル", "5教科"],
        "assets": [
            {
                "id": "japan-map",
                "alt": "日本の地方を色分けした地図",
                "mediaType": "image/svg+xml",
                "data": map_asset,
            },
            {
                "id": "plant-cell",
                "alt": "植物細胞の模式図。核・細胞壁・葉緑体・液胞が描かれている",
                "mediaType": "image/png",
                "data": cell_asset,
            },
        ],
        "notes": [
            note(
                "math-current",
                "オームの法則",
                "## V = RI\n電圧 V、電流 I、抵抗 R の関係は **V = RI** です。単位は V、A、Ω を使います。",
                [
                    question(
                        "q-numeric",
                        "numeric",
                        "電圧6V、抵抗2Ωのとき、電流は何Aですか。有効数字2桁、単位Aを付けて答えてください。",
                        "I = V / R = 6V / 2Ω = 3A です。3.0A と書くと有効数字2桁になります。",
                        answer={
                            "value": 3,
                            "unit": "A",
                            "tolerance": 0.001,
                            "significantFigures": 2,
                            "acceptedUnits": {"mA": 0.001},
                            "requireUnit": True,
                        },
                        hint="6 ÷ 2 を計算し、小数第1位まで書きます。",
                    )
                ],
            ),
            note(
                "english-order",
                "現在完了の語順",
                "## have + 過去分詞\n現在完了は **have/has + 過去分詞** の語順です。never は have と過去分詞の間に置きます。",
                [
                    question(
                        "q-ordering",
                        "ordering",
                        "次の語を並べ替えて「私はこれを一度も見たことがありません」という英文にしてください。",
                        "I have never seen this. が正しい語順です。",
                        options=[
                            {"id": "i", "content": "I"},
                            {"id": "have", "content": "have"},
                            {"id": "never", "content": "never"},
                            {"id": "seen", "content": "seen"},
                            {"id": "this", "content": "this"},
                        ],
                        initialOrder=["this", "never", "i", "seen", "have"],
                        answer={"orders": [["i", "have", "never", "seen", "this"]]},
                        hint="主語の次は助動詞 have です。",
                    )
                ],
            ),
            note(
                "history-order",
                "幕府の順序",
                "## 政治の中心\n鎌倉幕府 → 室町幕府 → 江戸幕府 → 明治維新 の順に進みます。",
                [
                    question(
                        "q-history",
                        "ordering",
                        "次の出来事を古い順に並べ替えてください。",
                        "鎌倉幕府（1192年ごろ）、室町幕府（1336年）、江戸幕府（1603年）、明治維新（1868年）の順です。",
                        options=[
                            {"id": "kamakura", "content": "鎌倉幕府の成立"},
                            {"id": "muromachi", "content": "室町幕府の成立"},
                            {"id": "edo", "content": "江戸幕府の成立"},
                            {"id": "meiji", "content": "明治維新"},
                        ],
                        initialOrder=["edo", "kamakura", "meiji", "muromachi"],
                        answer={
                            "orders": [
                                ["kamakura", "muromachi", "edo", "meiji"],
                            ]
                        },
                        hint="武士の政権が続いた順に考えます。",
                    )
                ],
            ),
            note(
                "science-match",
                "元素記号",
                "## 元素記号\nH は水素、O は酸素、Na はナトリウム、Cl は塩素です。",
                [
                    question(
                        "q-matching",
                        "matching",
                        "元素記号と元素名を対応付けてください。",
                        "H=水素、O=酸素、Na=ナトリウム、Cl=塩素です。",
                        matching={
                            "left": [
                                {"id": "h", "content": "H"},
                                {"id": "o", "content": "O"},
                                {"id": "na", "content": "Na"},
                                {"id": "cl", "content": "Cl"},
                            ],
                            "right": [
                                {"id": "hydrogen", "content": "水素"},
                                {"id": "oxygen", "content": "酸素"},
                                {"id": "sodium", "content": "ナトリウム"},
                                {"id": "chlorine", "content": "塩素"},
                            ],
                        },
                        answer={
                            "pairs": [
                                {"left": "h", "right": "hydrogen"},
                                {"left": "o", "right": "oxygen"},
                                {"left": "na", "right": "sodium"},
                                {"left": "cl", "right": "chlorine"},
                            ]
                        },
                        hint="Na はラテン語の natrium に由来します。",
                    )
                ],
            ),
            note(
                "social-map",
                "日本の地方",
                "## 地方区分\n日本は北海道、東北、関東、中部、近畿、中国、四国、九州などの地方に分かれます。\n\n地図: 地球地図日本（国土地理院）を加工。",
                [
                    question(
                        "q-hotspot",
                        "hotspot",
                        "地図の中で関東地方を選んでください。",
                        "関東地方は本州の東側、東京・神奈川・埼玉・千葉などを含む地域です。",
                        hotspot={
                            "asset": "japan-map",
                            "regions": [
                                {
                                    "id": "hokkaido",
                                    "alt": "北海道",
                                    "shape": {
                                        "type": "polygon",
                                        "points": map_regions["hokkaido"],
                                    },
                                },
                                {
                                    "id": "kanto",
                                    "alt": "関東",
                                    "shape": {
                                        "type": "polygon",
                                        "points": map_regions["kanto"],
                                    },
                                },
                                {
                                    "id": "kansai",
                                    "alt": "関西",
                                    "shape": {
                                        "type": "polygon",
                                        "points": map_regions["kansai"],
                                    },
                                },
                                {
                                    "id": "kyushu",
                                    "alt": "九州",
                                    "shape": {
                                        "type": "polygon",
                                        "points": map_regions["kyushu"],
                                    },
                                },
                            ],
                        },
                        answer={"regionIds": ["kanto"]},
                        hint="本州の東側、太平洋に面した地域です。地図は地球地図日本（国土地理院）を加工しています。",
                    )
                ],
            ),
            note(
                "science-cell",
                "植物細胞",
                "## 植物細胞のつくり\n植物細胞には細胞壁、細胞膜、核、液胞、葉緑体があります。",
                [
                    question(
                        "q-labeling",
                        "labeling",
                        "植物細胞の図に、核・細胞壁・葉緑体・液胞のラベルを配置してください。",
                        "核は中央の丸い構造、細胞壁は外側の厚い境界、葉緑体は緑の楕円、液胞は大きな水色の部分です。",
                        labeling={
                            "asset": "plant-cell",
                            "targets": [
                                {"id": "nucleus", "x": 0.422, "y": 0.45, "alt": "中央の丸い構造"},
                                {"id": "wall", "x": 0.075, "y": 0.5, "alt": "外側の厚い境界"},
                                {"id": "chloroplast", "x": 0.19, "y": 0.257, "alt": "緑の楕円"},
                                {"id": "vacuole", "x": 0.742, "y": 0.49, "alt": "大きな水色の部分"},
                            ],
                            "labels": [
                                {"id": "l-nucleus", "content": "核"},
                                {"id": "l-wall", "content": "細胞壁"},
                                {"id": "l-chloroplast", "content": "葉緑体"},
                                {"id": "l-vacuole", "content": "液胞"},
                            ],
                        },
                        answer={
                            "pairs": {
                                "nucleus": "l-nucleus",
                                "wall": "l-wall",
                                "chloroplast": "l-chloroplast",
                                "vacuole": "l-vacuole",
                            }
                        },
                        hint="外側から内側へ、細胞壁 → 細胞膜 → 核・液胞の順に見ます。",
                    )
                ],
            ),
        ],
    }
    OUT.write_text(json.dumps(workbook, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    build()
