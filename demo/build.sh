#!/usr/bin/env bash
# GitHub Pages の地図デモを作る。Schependomlaan（CC BY 4.0）の DWG を取得し、1 階の DWG を対応点で
# 位置合わせして、同じ記録を残りの DWG に適用する。
#
#   cargo build --release -p cad2geojson && demo/build.sh [出力先（既定は demo/dist）]
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-$ROOT/demo/dist}
CLI=${CAD2GEOJSON:-$ROOT/target/release/cad2geojson}
[ -x "$CLI" ] || CLI=$CLI.exe
PY=${PYTHON:-python3}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$OUT/data" "$WORK/dwg"
cp "$ROOT/demo/index.html" "$OUT/"

# testdata/README.md の Schependomlaan と同じ取得元
R=https://raw.githubusercontent.com/openBIMstandards/Archive-DataSetSchependomlaan/HEAD
D="Coordination%20model%20and%20subcontractors%20models/BIMsight%20Projectdata1/ProjectData"
for f in Fund. BG V1 V2 V3 Dak; do
  for k in "Rio+MV+Hwa" "Water+Gas+CV" Sparingen; do
    # 基礎（Fund.）と 1 階（BG）の Sparingen は無い
    curl -sfL -o "$WORK/dwg/$f $k.dwg" "$R/$D/SONSBEEK-$f%20${k//+/%2B}.dwg" || continue
  done
done

name() { echo "$1" | sed 's/\.dwg$//; s/[ .+]\{1,\}/_/g; s/_$//'; }

# 1 階の排水の図面で位置合わせし、同じ記録を残りに適用する（全ファイルの原点が同じ）
GCP=$ROOT/demo/schependomlaan/gcp.csv
"$CLI" georef "$WORK/dwg/BG Rio+MV+Hwa.dwg" --gcp "$GCP" --work-crs EPSG:28992 --units m \
  -o "$WORK/BG_Rio_MV_Hwa.geojson" --record "$WORK/georef.json"
for p in "$WORK"/dwg/*.dwg; do
  n=$(name "$(basename "$p")")
  [ -f "$WORK/$n.geojson" ] || "$CLI" apply "$p" --record "$WORK/georef.json" -o "$WORK/$n.geojson"
done

# 対応点・検証点: 図面側は記録で、地図側（RD New）は回転 0・平行移動 0 の記録で経緯度にする
"$PY" "$ROOT/demo/prepare.py" gcp "$WORK/georef.json" "$WORK"
"$CLI" apply "$WORK/gcp_cad.geojson" --record "$WORK/georef.json" -o "$WORK/gcp_cad.wgs84.geojson"
"$CLI" apply "$WORK/gcp_map.geojson" --record "$WORK/identity.json" -o "$WORK/gcp_map.wgs84.geojson"

# 建物の外形（BAG、CC0）
curl -sfL -o "$WORK/bag.geojson" \
  "https://api.pdok.nl/kadaster/bag/ogc/v2/collections/pand/items?identificatie=0268100000084949&f=json"

"$PY" "$ROOT/demo/prepare.py" site "$WORK" "$OUT/data"
echo "wrote $OUT"
