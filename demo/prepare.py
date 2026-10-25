"""地図デモのデータを整える（demo/build.sh から呼ぶ）。標準ライブラリだけを使う。

  prepare.py gcp <記録> <作業ディレクトリ>   対応点・検証点の GeoJSON（図面座標・RD New）と、回転 0 の記録を書く
  prepare.py site <作業ディレクトリ> <出力>   図面の GeoJSON を軽くし、目録（index.json）を書く
"""

import copy
import json
import re
import sys
from pathlib import Path

FLOORS = ["Fund", "BG", "V1", "V2", "V3", "Dak"]
KINDS = ["Rio_MV_Hwa", "Water_Gas_CV", "Sparingen"]
# 図面の属性のうち、表示に使うもの
KEEP = ["layer", "color_rgb", "text", "text_height", "text_rotation_deg", "text_attachment"]


def load(p):
    return json.loads(Path(p).read_text(encoding="utf-8"))


def dump(obj, p):
    Path(p).write_text(json.dumps(obj, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")


def points(rows, key):
    return {
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "properties": {"id": i, "role": role, "residual_m": round(res, 3)},
                "geometry": {"type": "Point", "coordinates": coord},
            }
            for i, (role, coord, res) in enumerate((r[0], r[1][key], r[2]) for r in rows)
        ],
    }


def gcp(record_path, work):
    record = load(record_path)
    part = record["parts"][0]
    rows = [
        (role, g, (g["residual"][0] ** 2 + g["residual"][1] ** 2) ** 0.5)
        for role in ("control", "check")
        for g in part.get(role, [])
    ]
    dump(points(rows, "cad"), Path(work) / "gcp_cad.geojson")
    dump(points(rows, "map_work"), Path(work) / "gcp_map.geojson")
    # 作業用の座標系の座標をそのまま経緯度にするための記録
    identity = copy.deepcopy(record)
    identity["parts"][0]["transform"] = {"scale": 1.0, "rotation_deg": 0.0, "translation": [0.0, 0.0]}
    dump(identity, Path(work) / "identity.json")


def rounded(c):
    if isinstance(c[0], (int, float)):
        return [round(v, 8) for v in c]
    return [rounded(x) for x in c]


def slim(fc):
    out = []
    for f in fc["features"]:
        p, g = f.get("properties") or {}, f.get("geometry")
        if not g or p.get("layer") == "DEFPOINTS" or (g["type"] == "Point" and not p.get("text")):
            continue
        q = {k: p[k] for k in KEEP if k in p}
        for k in ("text_height", "text_rotation_deg"):
            if k in q:
                q[k] = round(q[k], 3)
        out.append({"type": "Feature", "properties": q,
                    "geometry": {"type": g["type"], "coordinates": rounded(g["coordinates"])}})
    return {"type": "FeatureCollection", "features": out}


def site(work, out):
    work, out = Path(work), Path(out)
    drawings = []
    for p in sorted(work.glob("*.geojson")):
        m = re.fullmatch(r"(Fund|BG|V\d|Dak)_(.+)", p.stem)
        if not m or m.group(2) not in KINDS:
            continue
        fc = slim(load(p))
        dump(fc, out / f"{p.stem}.geojson")
        drawings.append({"id": p.stem, "floor": m.group(1), "kind": m.group(2),
                         "features": len(fc["features"])})
    drawings.sort(key=lambda d: (FLOORS.index(d["floor"]), KINDS.index(d["kind"])))

    # 対応点・検証点: 図面側の点と、地図側への線（残差）
    cad = load(work / "gcp_cad.wgs84.geojson")["features"]
    mp = {f["properties"]["id"]: f for f in load(work / "gcp_map.wgs84.geojson")["features"]}
    features = []
    for f in cad:
        q = {k: f["properties"][k] for k in ("id", "role", "residual_m")}
        a, b = f["geometry"]["coordinates"], mp[q["id"]]["geometry"]["coordinates"]
        features.append({"type": "Feature", "properties": q, "geometry": {"type": "Point", "coordinates": rounded(a)}})
        features.append({"type": "Feature", "properties": {**q, "residual": True},
                         "geometry": {"type": "LineString", "coordinates": [rounded(a), rounded(b)]}})
    dump({"type": "FeatureCollection", "features": features}, out / "gcp.geojson")

    bag = load(work / "bag.geojson")
    dump({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": f["geometry"],
         "properties": {k: f["properties"].get(k) for k in ("identificatie", "bouwjaar")}}
        for f in bag["features"]]}, out / "bag.geojson")

    part = load(work / "georef.json")["parts"][0]
    dump({
        "drawings": drawings,
        "georef": {
            "work_crs": "EPSG:28992",
            "rotation_deg": part["transform"]["rotation_deg"],
            "control": len(part.get("control", [])),
            "check": len(part.get("check", [])),
            "control_error": part.get("control_error"),
            "check_error": part.get("check_error"),
        },
    }, out / "index.json")
    print(f"{len(drawings)} drawings, {sum(d['features'] for d in drawings)} features")


if __name__ == "__main__":
    {"gcp": gcp, "site": site}[sys.argv[1]](*sys.argv[2:])
