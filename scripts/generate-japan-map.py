#!/usr/bin/env python3
"""Builds assets/japan-regions.svg and assets/japan-regions.json.

Source: 地球地図日本 (Global Map Japan, Geospatial Information Authority of
Japan), converted to GeoJSON by dataofjapan/land. Non-commercial use requires
attribution; the generated SVG keeps the attribution in a comment and the
README/sample description points to it.

Usage:
  python3 scripts/generate-japan-map.py /path/to/japan.geojson
"""

import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SVG_OUT = ROOT / "assets" / "japan-regions.svg"
GEO_OUT = ROOT / "assets" / "japan-regions.json"

WIDTH = 340
HEIGHT = 420
PADDING = 14

REGIONS = {
    1: ("hokkaido", "北海道", "#d9e8ef"),
    2: ("tohoku", "東北", "#cfe3d8"),
    3: ("tohoku", "東北", "#cfe3d8"),
    4: ("tohoku", "東北", "#cfe3d8"),
    5: ("tohoku", "東北", "#cfe3d8"),
    6: ("tohoku", "東北", "#cfe3d8"),
    7: ("tohoku", "東北", "#cfe3d8"),
    8: ("kanto", "関東", "#f2d8a8"),
    9: ("kanto", "関東", "#f2d8a8"),
    10: ("kanto", "関東", "#f2d8a8"),
    11: ("kanto", "関東", "#f2d8a8"),
    12: ("kanto", "関東", "#f2d8a8"),
    13: ("kanto", "関東", "#f2d8a8"),
    14: ("kanto", "関東", "#f2d8a8"),
    15: ("chubu", "中部", "#e3e8cc"),
    16: ("chubu", "中部", "#e3e8cc"),
    17: ("chubu", "中部", "#e3e8cc"),
    18: ("chubu", "中部", "#e3e8cc"),
    19: ("chubu", "中部", "#e3e8cc"),
    20: ("chubu", "中部", "#e3e8cc"),
    21: ("chubu", "中部", "#e3e8cc"),
    22: ("chubu", "中部", "#e3e8cc"),
    23: ("chubu", "中部", "#e3e8cc"),
    24: ("kansai", "関西", "#e7d6e8"),
    25: ("kansai", "関西", "#e7d6e8"),
    26: ("kansai", "関西", "#e7d6e8"),
    27: ("kansai", "関西", "#e7d6e8"),
    28: ("kansai", "関西", "#e7d6e8"),
    29: ("kansai", "関西", "#e7d6e8"),
    30: ("kansai", "関西", "#e7d6e8"),
    31: ("chugoku", "中国", "#ead5d0"),
    32: ("chugoku", "中国", "#ead5d0"),
    33: ("chugoku", "中国", "#ead5d0"),
    34: ("chugoku", "中国", "#ead5d0"),
    35: ("chugoku", "中国", "#ead5d0"),
    36: ("shikoku", "四国", "#dde6c8"),
    37: ("shikoku", "四国", "#dde6c8"),
    38: ("shikoku", "四国", "#dde6c8"),
    39: ("shikoku", "四国", "#dde6c8"),
    40: ("kyushu", "九州", "#f0dcc4"),
    41: ("kyushu", "九州", "#f0dcc4"),
    42: ("kyushu", "九州", "#f0dcc4"),
    43: ("kyushu", "九州", "#f0dcc4"),
    44: ("kyushu", "九州", "#f0dcc4"),
    45: ("kyushu", "九州", "#f0dcc4"),
    46: ("kyushu", "九州", "#f0dcc4"),
}
# 47 (Okinawa) is omitted to keep the main-island map readable. The remote
# islands are outside the sample question's scope.

ATTRIBUTION = "地図: 地球地図日本（国土地理院）を加工"


def project(lon, lat):
    # Web-Mercator-like y; x is also radians so both axes share units.
    lat = max(-85.0, min(85.0, lat))
    y = math.log(math.tan(math.pi / 4.0 + math.radians(lat) / 2.0))
    return math.radians(lon), y


def rdp(points, epsilon):
    if len(points) < 3:
        return points
    first = points[0]
    last = points[-1]
    dx = last[0] - first[0]
    dy = last[1] - first[1]
    norm = math.hypot(dx, dy)
    index = -1
    max_distance = 0.0
    for i in range(1, len(points) - 1):
        px, py = points[i]
        if norm == 0.0:
            distance = math.hypot(px - first[0], py - first[1])
        else:
            distance = abs(dy * px - dx * py + last[0] * first[1] - last[1] * first[0]) / norm
        if distance > max_distance:
            index = i
            max_distance = distance
    if max_distance > epsilon:
        left = rdp(points[: index + 1], epsilon)
        right = rdp(points[index:], epsilon)
        return left[:-1] + right
    return [first, last]


def ring_area(points):
    area = 0.0
    previous = points[-1]
    for point in points:
        area += previous[0] * point[1] - point[0] * previous[1]
        previous = point
    return area / 2.0


def convex_hull(points):
    points = sorted(set(points))
    if len(points) <= 1:
        return points

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lower = []
    for point in points:
        while len(lower) >= 2 and cross(lower[-2], lower[-1], point) <= 0:
            lower.pop()
        lower.append(point)
    upper = []
    for point in reversed(points):
        while len(upper) >= 2 and cross(upper[-2], upper[-1], point) <= 0:
            upper.pop()
        upper.append(point)
    return lower[:-1] + upper[:-1]


def main():
    source = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("/tmp/candidate.json")
    data = json.loads(source.read_text(encoding="utf-8"))
    features = [
        feature
        for feature in data["features"]
        if feature["properties"].get("id") in REGIONS
    ]
    if not features:
        raise SystemExit("no Japanese prefectures found")

    # Project first, then fit the projected bounding box into the viewBox.
    projected = []
    for feature in features:
        geometry = feature["geometry"]
        polygons = (
            [geometry["coordinates"]]
            if geometry["type"] == "Polygon"
            else geometry["coordinates"]
        )
        rings = []
        for polygon in polygons:
            for ring in polygon:
                if any(
                    not (128.0 <= lon <= 146.0 and 30.5 <= lat <= 46.0)
                    for lon, lat in ring
                ):
                    # Remote islands (Ogasawara, Okinawa, Amami) would stretch
                    # the bbox and make the main islands unreadably small.
                    continue
                rings.append([project(lon, lat) for lon, lat in ring])
        projected.append((feature, rings))

    xs = [x for _, rings in projected for ring in rings for x, _ in ring]
    ys = [y for _, rings in projected for ring in rings for _, y in ring]
    min_x, max_x = min(xs), max(xs)
    min_y, max_y = min(ys), max(ys)
    scale = min(
        (WIDTH - 2 * PADDING) / (max_x - min_x),
        (HEIGHT - 2 * PADDING) / (max_y - min_y),
    )
    offset_x = (WIDTH - (max_x - min_x) * scale) / 2.0
    offset_y = (HEIGHT - (max_y - min_y) * scale) / 2.0

    def to_svg(point):
        x, y = point
        return (
            offset_x + (x - min_x) * scale,
            HEIGHT - (offset_y + (y - min_y) * scale),
        )

    paths = []
    region_points = {}
    for feature, rings in projected:
        region_id, _, color = REGIONS[feature["properties"]["id"]]
        region_points.setdefault(region_id, [])
        subpaths = []
        for ring in rings:
            points = [to_svg(point) for point in ring]
            if len(points) < 3:
                continue
            if abs(ring_area(points)) < 2.0:
                continue
            points = rdp(points, 0.55)
            if len(points) < 3:
                continue
            region_points[region_id].extend(points)
            coordinates = " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
            subpaths.append(f"M{coordinates}Z")
        if subpaths:
            paths.append(
                f'<path fill="{color}" stroke="#ffffff" stroke-width="0.7" d="{"".join(subpaths)}"/>'
            )

    hulls = {}
    for region_id, points in region_points.items():
        hull = convex_hull([(round(x, 1), round(y, 1)) for x, y in points])
        hull = rdp(hull + [hull[0]], 1.5)[:-1]
        if len(hull) < 3:
            continue
        hulls[region_id] = [
            [round(x / WIDTH, 4), round(y / HEIGHT, 4)] for x, y in hull
        ]

    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}" role="img">',
        f"<!-- {ATTRIBUTION} -->",
        "<title>日本地図（地方色分け）</title>",
        f'<rect width="{WIDTH}" height="{HEIGHT}" fill="#f7f8f9"/>',
        "<g>",
        *paths,
        "</g>",
        "</svg>",
    ]
    SVG_OUT.write_text("\n".join(svg) + "\n", encoding="utf-8")
    GEO_OUT.write_text(
        json.dumps(
            {
                "attribution": ATTRIBUTION,
                "regions": hulls,
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {SVG_OUT} ({SVG_OUT.stat().st_size} bytes)")
    print(f"wrote {GEO_OUT} ({GEO_OUT.stat().st_size} bytes)")
    for region_id, hull in hulls.items():
        print(region_id, len(hull), "points")


if __name__ == "__main__":
    main()
