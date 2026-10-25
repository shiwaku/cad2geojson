# cad2geojson

CAD 図面（DWG / DXF）を、Web 地図（MapLibre GL JS）に**正しい位置で**重ねられる GeoJSON にするためのツール。

> **状態: 開発中（Phase 1 の CLI が完成。次は Phase 2 のブラウザ版）。** DWG / DXF を読み、部分の候補を出し、部分に分けて位置合わせし、WGS84 の GeoJSON にする CLI ができた。設計は [docs/design.md](docs/design.md)、dwg2geo の評価は [docs/eval-dwg2geo.md](docs/eval-dwg2geo.md)、調べた関連ツールは [docs/related-tools.md](docs/related-tools.md) にある。

## 方針

DWG を読んで図形を取り出す部分は、既存の OSS [dwg2geo](https://github.com/milkway/dwg2geo)（MIT、内部で [acadrust](https://github.com/hakanaktt/acadrust) を使う）に任せる。cad2geojson は、dwg2geo に無い **「地図に正しく載せる」部分**を作る。

| cad2geojson が作るもの | 内容 |
|---|---|
| 図面を部分に分ける | 1 枚に並んだ 1 階・2 階・図枠などを分け、部分ごとに位置合わせする。階どうしは共通する柱（円）で相対位置を決める |
| 位置合わせ | 縮尺を単位から固定した剛体変換（既定）と相似変換。対応点はどの CRS で受け取っても作業用の投影座標系（平面直角座標系。海外の図面は UTM・RD New）で解く。検証点の誤差も出す |
| 座標系変換 | 純 Rust の proj4rs で WGS84 へ。ブラウザでも動く |
| 文字の属性の補正 | 文字の回転角・文字高を位置合わせに合わせて直す |
| 対応点を探す | 地図と図面を並べて点を打つ画面、図形の中心の自動抽出 |
| DXF | acadrust で読み、dwg2geo と同じ取り出し方で変換する。日本の DXF（Jw_cad 由来の R12 など）の文字コードを判定する |

上流（dwg2geo・acadrust）の不足は、上流の公開 API だけを使って cad2geojson の中で補う。

```
DWG ───────────────────┬─ dwg2geo ─→ 図面座標の GeoJSON ─→ 部分に分ける ─→ 位置合わせ ─→ WGS84 へ ─→ GeoJSON
DXF ─ acadrust → DWG ──┘                                                       ↑
                                                                          対応点を探す
```

- **CLI**（Rust）: dwg2geo と acadrust を crate として使う
- **ブラウザ版**（Vite + MapLibre）: dwg2geo の npm（WASM）で変換し、cad2geojson の位置合わせを WASM で動かす。図面をブラウザの外に送らない

## 使い方

リポジトリの合成の図面（[testdata/synthetic](testdata/synthetic)。答えの分かっている 1 階・2 階・表題欄の図面）で、下の手順をそのまま試せる（`plan.dxf` を `testdata/synthetic/plan_r12.dxf` などに、`gcp.csv`・`parts.json` を同じディレクトリのものにする）。

```sh
# 1. 図面座標の GeoJSON にして、対応点の図面側の座標を QGIS などで拾う
cad2geojson convert plan.dxf -o plan.local.geojson

# 2. 部分（1 階・2 階・図枠など）の候補を出し、名前と follow を書き足す
cad2geojson parts plan.dxf -o parts.json

# 3. 対応点で位置合わせし、WGS84 の GeoJSON にする（入力は DWG / DXF / 図面座標の GeoJSON）
cad2geojson georef plan.dxf \
  --gcp gcp.csv --work-crs EPSG:6677 --units mm \
  --parts parts.json -o plan.geojson
```

`gcp.csv`（`role` は `control`（既定）か `check`。検証点は変換を解くのに使わず、誤差だけを出す）:

```csv
part,cad_x,cad_y,map_x,map_y,role
2F,15000.000000,62000.000000,-7023.274458,-37940.608927,control
2F,50100.000000,84000.000000,-7007.140903,-37902.455049,control
2F,29100.000000,69500.000000,-7016.026238,-37926.377859,check
1F,21348.132608,25777.924822,-7026.029155,-37925.691097,check
```

`parts` は、図形の無い帯（既定は図面の範囲の長辺の 3%。`--min-gap` で図面の単位で指定）で図面を再帰的に分け、上から順に `part1`, `part2`, … の候補にする。用紙の隅にある長方形（長い水平・垂直線で囲まれた領域）は表題欄とみなして `frame`（`place: false`）にし、ほぼすべての図形を囲む用紙の枠の線はどの部分にも入れない。図形の少ない塊（全体の 1% 未満）は `small1`, … として載せない。本体から遠く離れた少数の図形（原点に残った点や壊れた座標など。図形の中心の四分位の幅の 3 倍より遠く、全体の 1% 以下）は、図面の範囲を膨らませないよう先に外し、`stray1`, … として載せない。各候補の図形の数と中の文字（大きい順）を表示するので、それを見て名前を付ける。載せる候補どうしに共通する円があれば、`follow` で合わせられることを知らせる。合成の図面では、表題欄・図枠の外の注記・2 階・1 階に分かれ、2 階と 1 階が共通の円 24 個を持つと知らせる。

`parts.json`（図形は外接矩形の中心で、先に書いた部分から振り分ける。`place: false` の部分は出力しない）:

```json
{"parts": [
  {"name": "frame", "layers": ["図枠"], "place": false},
  {"name": "2F", "bbox": [5000, 56000, 60000, 90000]},
  {"name": "1F", "bbox": [5000, 8500, 60000, 40000], "follow": "2F"}
]}
```

`follow` を書いた部分は、対応点ではなく、相手の部分と共通する円（柱など）で位置を決める。地面の目印が写真に写らない 1 階を、屋根と対応させた 2 階に合わせるときに使う。同じ半径の円の組から平行移動を仮定して最も多く一致するものを採り、一致した組で回転と平行移動を解き直す。一致した組数・残差・2 番目に多く一致した平行移動の組数を表示し、2 番目が一致数の半分を超えたら警告する。`follow` の部分の対応点は `check` としてだけ渡せる。

| オプション | 内容 |
|---|---|
| `--work-crs` | 変換を解く投影座標系（必須。系は自動で選ばない）。平面直角座標系（EPSG:6669〜6687 / 2443〜2461）、UTM（EPSG:326xx / 327xx / 258xx / 6688〜6692 / 3097〜3101）、RD New（EPSG:28992） |
| `--gcp-crs` | 対応点の地図側の CRS（既定は `--work-crs`）。経緯度（EPSG:4326）で渡しても作業用の座標系で解く |
| `--units` | 図面の単位（`mm` など）。縮尺をこれに固定し、回転と平行移動だけを解く |
| `--free-scale` | 縮尺も解く（相似変換）。単位が分からないとき |
| `--record` | 位置合わせの記録の出力先（既定は `<出力>.georef.json`） |
| `--dxf-encoding` | DXF の文字コード（`ANSI_932` など DXF の表記）。既定では中身のバイトで決める。記録が無い R12 などは中身が cp932 として妥当なら `ANSI_932`、R2004 以前で記録があっても中身が UTF-8 なら UTF-8 で読む（変換器やテキストエディタが記録を残したまま UTF-8 で書くことがある）。先頭の BOM は除く |

- 部分ごとの回転・縮尺・平行移動、対応点と検証点の誤差、縮尺を固定しなかった場合の縮尺を表示し、記録（JSON）に書き出す
- `cad2geojson apply plan.dxf --record plan.geojson.georef.json -o plan.geojson` で、記録から同じ変換を再現する
- 出力は WGS84 の GeoJSON（`.geojsonl` / `.ndjson` なら 1 行 1 図形）。各図形に `part` 属性を付ける
- 文字は `text_rotation_deg` を真北基準（子午線収差も補正）・反時計回りの度に、`text_height` を m に直す。元の値は `cad_text_rotation_deg` / `cad_text_height` に残す。MapLibre の `text-rotate` は時計回りなので、符号を反転して使う

## ビルド

```sh
cargo build --release   # target/release/cad2geojson
```

Windows の Git Bash では `/usr/bin/link` が MSVC の `link.exe` より先に見つかるので、cargo は PowerShell から実行する。
## DWG の仕様は公開されていない

DWG は Autodesk の独自バイナリ形式で、仕様は公開されていない。公式に読み書きできるのは Autodesk 製品と、同社の有償 SDK（RealDWG）のライセンス製品である。OSS が DWG を読めるのは、Open Design Alliance（ODA）が解析結果を「Open Design Specification for .dwg files」として無料公開しているなど、リバースエンジニアリングの蓄積があるためだ。一方 DXF は Autodesk が仕様（DXF Reference）を公開している。

- DWG は**完全に読める保証がない**。新しい形式（AC1032）や珍しいエンティティは欠ける可能性がある
- **実際の CAD ソフトが書き出した DWG** で検証する
- 直接読めない DWG は、ODA File Converter（無料）で DXF に変換してから読む経路を案内する
- 「DWG」の名称は Autodesk との商標問題の経緯があるため、「DWG ファイルを読み込める」という説明的な表現にとどめ、公式対応を連想させる書き方はしない

## デモ

<https://shiwaku.github.io/cad2geojson/> に、オランダの集合住宅 Schependomlaan（CC BY 4.0）の設備図 16 枚を位置合わせして航空写真に重ねた地図がある。1 階の図面を建物の外形（BAG）の角で位置合わせし、同じ記録を残り 15 枚に適用した。対応点・検証点とその残差も表示する。

`demo/build.sh` が DWG の取得から変換までを行い、GitHub Actions（`.github/workflows/pages.yml`）が main への push ごとに作り直す。手元では `cargo build --release -p cad2geojson && demo/build.sh` で `demo/dist` にでき、`python3 -m http.server -d demo/dist` などで開ける。

## 検証データ

取得元・取得手順・利用条件は [testdata/README.md](testdata/README.md) に書く。第三者の図面は同梱しない。

- **合成の図面**（リポジトリに入れる、CI で使う）: 位置合わせの答えが分かっている図面を `tools/gen_testdata` で作る。1 枚に 1 階・2 階・表題欄が並び、共通の柱がある。同じ図面を R12（cp932、記録なし）・R2010（UTF-8）・R2018 の DWG・記録と違う UTF-8（BOM 付き）の DXF で書き、本体から遠く離れた線を足したものも作る。どれから読んでも同じ変換・同じ図形になることを確かめる
- **実在の建物**（公開できる）: Schependomlaan（オランダの集合住宅、CC BY 4.0）。CAD ソフトが書き出した DWG 16 本を RD New で位置合わせし、BAG の建物と重ねる
- **手元だけの検証**（公開しない）: 利用条件で再配布が認められていない図面でも確かめている。結果は数字だけを [docs/design.md](docs/design.md) に書く

## 背景

2024 年 4 月に、顧客提供の DWG（平面直角座標系 9 系、mm 単位、AC1032）を次の手順で GIS データ化した。

1. AutoCAD で DWG → DXF（AutoCAD 2010 形式。新しい形式では日本語が文字化けした）
2. `ogr2ogr` で DXF → GeoJSON（`DXF_FEATURE_LIMIT_PER_BLOCK=-1`）
3. geopandas で座標を 0.001 倍し、EPSG:6677 を付与

市販 CAD を使わず、DWG / DXF から地図に正しく載るデータまでを 1 本で通すのがこのツールの狙い。

## ライセンス

MIT。依存する [dwg2geo](https://github.com/milkway/dwg2geo) は MIT、[acadrust](https://github.com/hakanaktt/acadrust) は MPL-2.0。
