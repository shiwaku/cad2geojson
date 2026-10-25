# 検証データ

このディレクトリは `README.md` と `synthetic/` 以外を追跡しない。第三者の図面は取得手順だけを書き、ファイルは各自で取得する。

## 合成データ（`synthetic/`）

位置合わせの答えが分かっている図面。`tools/gen_testdata` で生成し、リポジトリに入れている（MIT）。CI のテスト（`crates/cli/tests/synthetic.rs`）が `parts` と `georef` を回し、どの形式から読んでも変換が答えと一致し、同じ図形が出ることを確かめる。再配布できない図面（ビッグシップなど）で確かめた、形式・文字コードの違いや図面のノイズも、ここに同じ形のものを作って置き換える。

```bash
cargo run -p cad2geojson-gen-testdata   # testdata/synthetic を書き直す
```

| ファイル | 中身 |
|---|---|
| `plan_r12.dxf` | R12（AC1009）。文字は cp932 で `$DWGCODEPAGE` なし（Jw_cad が書き出す DXF と同じ形）。改行は CRLF |
| `plan_r2010.dxf` | R2010（AC1024）。文字は UTF-8。acadrust で書いた |
| `plan_r2018.dwg` | R2018（AC1032）の DWG。acadrust で書いた（DXF を経ずに dwg2geo で読む経路の確認用。CAD ソフトが書いた DWG は Schependomlaan で確かめる） |
| `plan_utf8_bom.dxf` | R12 で `$DWGCODEPAGE` は `ANSI_932` なのに、文字は UTF-8 で先頭に BOM がある。cp932 の DXF をテキストエディタで UTF-8 にして保存し直したもの、記録を残したまま UTF-8 で書く変換器（dwg2dxf など）と同じ形 |
| `plan_stray.dxf` | `plan_r2010.dxf` に、本体から 500 km 離れた線を 1 本足したもの。原点に残った点や壊れた座標のように、図面の範囲を膨らませる図形（`parts` の提案が変わらないことを確かめる） |
| `parts.json` | 図枠（レイヤ `図枠`、載せない）・2 階・1 階（`follow: 2F`） |
| `gcp.csv` | 2 階の対応点 2 点・検証点 2 点、1 階の検証点 3 点 |

- 1 枚の図面に 1 階・2 階・図枠を並べた（実在の建物の全体平面図によくある構成）。単位は mm。階の名前の文字を含めて、1 階は y ≈ 9,500〜34,500、2 階は y = 59,500〜84,500 にあり、間に図形の無い帯がある（部分の自動の提案 #14 の検証にも使う）
- 柱（半径 400 の円）は 6 × 4 = 24 本で両階に共通。通りの間隔をそろえていないので、ずらした位置で一致する柱は少ない（2 番目の候補は 8 本）。1 階だけに半径 300 の円が 3 つ、2 階だけに半径 250 の円が 2 つある
- 1 階は 2 階に対して −0.2° 回して描いた。正しい変換は、2 階が回転 35°・平行移動 (−7000, −38000) m、1 階が回転 35.2°（作業用の座標系は EPSG:6677）。対応点・検証点の地図側の座標はこの変換から計算した
- レイヤ名と文字は日本語（`柱`・`壁`・`文字`・`図枠`、「エントランスホール」など）

## 建築図面

### 米子コンベンションセンター（ビッグシップ）施設図面（`real/bigship/`）

実在する建物の平面図。施設の利用者向けに公開されている DXF。

- 取得元: <https://www.bigship.or.jp/bigship/organizer/3/>
- 利用条件: 図面のページには記載が無い。サイトの「[当サイトについて](https://www.bigship.or.jp/bigship/organizer/22/)」に、掲載情報（文章・写真・イラストなど）の著作権は米子コンベンションセンター（指定管理者: 公益財団法人とっとりコンベンションビューロー）が有するとあり、転載・二次利用を認める記述は無い（2026-10-12 確認）。**手元での検証にだけ使い、リポジトリにも公開デモにも入れない。変換結果を公開するには施設の許可が要る**

```bash
mkdir -p testdata/real/bigship && cd testdata/real/bigship
B=https://www.bigship.or.jp
curl -o 12f-heimenhall.dxf  $B/user/filer_public/43/96/4396965d-8084-4a77-8b97-d4cc0e72bebe/12f-heimenhall.dxf
curl -o entoransurobi.dxf   $B/user/filer_public/3f/f3/3ff35c89-ffde-48cb-a391-79e907938f21/entoransurobi.dxf
curl -o stage_100_1.dxf     $B/user/filer_public/7f/2b/7f2b1574-5a67-4072-8271-dabfaa857516/100_1.dxf
```

| ファイル | 図面 | `$ACADVER` | `$DWGCODEPAGE` | `$INSUNITS` | 特徴 |
|---|---|---|---|---|---|
| `12f-heimenhall.dxf` | 全体平面図 1F（客席）・2F | AC1009（R12） | なし | なし | レイヤ名が `_0-0_0` 形式（Jw_cad から書き出したと思われる）。単位情報なし。範囲 0〜168000 × 0〜118800 |
| `entoransurobi.dxf` | エントランスロビー（ホール棟 1F） | AC1015（2000） | ANSI_932 | 4（mm） | 日本語は cp932。MTEXT・POLYLINE あり |
| `stage_100_1.dxf` | 多目的ホール舞台平面図 1/100 | AC1024（2010） | ANSI_932 | 4（mm） | R2010 なので文字列は UTF-8 |

3 ファイルで DXF の世代・文字コード・単位の有無がすべて違う。

### Autodesk の AutoCAD サンプル図面（`real/autodesk/`）

本物の AutoCAD が書き出した DWG。建築系のサンプル（Architectural Example、`home_floor_plan.dwg` など）を使う。

- 取得元: <https://autodesk.com/autocad-samples>、<https://autodesk.com/autocadlt-samples>（ブラウザで手動ダウンロード）
- 利用条件: Autodesk のサンプルとしての提供。**手元での検証にだけ使い、リポジトリにも公開デモにも入れない**

### Schependomlaan（`real/schependomlaan/`）

オランダ・ナイメーヘンの集合住宅（4 階建て、10 戸、2015〜2016 年施工）の BIM のデータセット。実在の建物で、CAD ソフトが書き出した DWG と IFC が両方あり、**再配布できる**。公開デモにも使える（出典を明記する）。

- 取得元: <https://github.com/openBIMstandards/Archive-DataSetSchependomlaan>（現在は <https://github.com/buildingSMART/Sample-Test-Files/tree/master/IFC%202x3/Schependomlaan> に移っている）
- 利用条件: LICENSE.MD に「(C) original owners. This work is licensed under the Creative Commons Attribution 4.0 International License.」（CC BY 4.0）
- 全体は約 3 GB（点群・ドローン画像を含む）。使うファイルだけを取り出す

```bash
R=https://raw.githubusercontent.com/openBIMstandards/Archive-DataSetSchependomlaan/HEAD
mkdir -p testdata/real/schependomlaan/{dwg,ifc} && cd testdata/real/schependomlaan
D="Coordination%20model%20and%20subcontractors%20models/BIMsight%20Projectdata1/ProjectData"
for f in BG Dak Fund. V1 V2 V3; do for k in "Rio+MV+Hwa" "Water+Gas+CV" Sparingen; do
  curl -sfL -o "dwg/SONSBEEK-$f $k.dwg" "$R/$D/SONSBEEK-$f%20${k//+/%2B}.dwg"
done; done   # 存在しない組み合わせ（BG・Fund. の Sparingen）は失敗してよい
curl -sfL -o "ifc/IFC Schependomlaan.ifc" "$R/Design%20model%20IFC/IFC%20Schependomlaan.ifc"
```

| ファイル | 中身 | 特徴 |
|---|---|---|
| `dwg/SONSBEEK-*.dwg`（16 本） | 設備業者の図面。基礎（Fund.）・1 階（BG）・2〜4 階（V1〜V3）・屋根（Dak）ごとに、排水（Rio+MV+Hwa）・給水ガス暖房（Water+Gas+CV）・開口（Sparingen） | AC1024（R2010）。単位は m。全ファイルが同じ原点で、通り芯（レイヤ `New_A Stramien`）が共通。**建物の外形（壁）は描かれていない**（建築の下図が入っていない）。文字はオランダ語 |
| `ifc/IFC Schependomlaan.ifc` | 設計モデル（Archicad、IFC2X3、49 MB） | DWG と同じ原点。IfcSite の緯度経度は RD の原点（アメルスフォールト）のままで、位置の情報として使えない |

位置合わせの基準（どちらも再配布できる）:

- 建物の外形: BAG（建物台帳）の pand `0268100000084949`（CC0）。PDOK の OGC API（`https://api.pdok.nl/kadaster/bag/ogc/v2/collections/pand/items`）で取得
- 航空写真: PDOK の Luchtfoto（CC BY 4.0）。WMS `https://service.pdok.nl/hwh/luchtfotorgb/wms/v1_0`、レイヤ `Actueel_orthoHR`

確認した結果（2026-10-12）:

- DWG 16 本をすべて読めた（1 本あたり 511〜6,961 図形）。読み飛ばしたのは長さ 0 の線と OLE 埋め込みだけ
- DWG に外形が無いので、IFC の 1 階の床・壁から外形を作り、BAG の外形に当てはめた（回転 21° で一意、IoU 0.93）。角 20 組のうち 4 組を対応点、16 組を検証点にして、1 階の DWG を RD New（`--work-crs EPSG:28992 --units m`）で位置合わせした
- 回転 21.24°、対応点の残差 RMS 0.29 m、検証点の RMS 0.30 m・最大 0.40 m。残差はどれも建物の外向きで、BAG（外装の外側）と IFC の外形（構造壁）の差（レンガの外装、約 0.3 m）と合う
- 同じ記録を `apply` で残り 15 本に適用できた。航空写真に重ねて、通り芯と外形が建物に合うことを目で確かめた

#### 2 つの階を 1 枚に並べる（部分分け・`follow` の確認）

階ごとに別のファイルなので、そのままでは部分分けと `follow` を確かめられない。`tools/stack_sheets` で、1 階（BG）はそのまま、2 階（V1）は 0.2° 回して x 方向に 50 m ずらして 1 枚の DWG にする。本物の CAD が書いた図形のまま、答えが分かっている複数階の図面になる。対応点（上の 1 階の角）は元の DWG の座標で書き、ツールが並べた図面の座標に直す（V1 も原点が同じなので同じ角を検証点に使える）。

```bash
D=testdata/real/schependomlaan/dwg
cargo run --release -p cad2geojson-stack-sheets -- -o sheet.dwg --gcp gcp_src.csv \
  "BG=$D/SONSBEEK-BG Rio+MV+Hwa.dwg" "V1=$D/SONSBEEK-V1 Rio+MV+Hwa.dwg:0.2:50:0"
# sheet.dwg.truth.json（答え）と sheet.dwg.gcp.csv（並べた図面の座標の対応点）もできる
cad2geojson parts sheet.dwg -o parts.json
cad2geojson georef sheet.dwg --gcp sheet.dwg.gcp.csv --work-crs EPSG:28992 --units m --parts parts.json -o sheet.geojson
```

確認した結果（2026-10-25）:

- 並べた DWG の図形の数は、種類ごとに元の 2 本の合計と一致した（8,744 = 3,973 + 4,771）。V1 の寸法 16 個とブロック `COMPLEXOBJECT_0〜7` は展開し、通り芯の記号のブロック `NSY101`（40 個）は BG と同じ中身なのでそのまま残した
- `parts` は 1 階・2 階を分け、各階の凡例（`CV` `GAS` `WW` `KW` `MV`、14 図形）を図形の少ない塊として外した。「共通する円 72 個（2 番目の平行移動は 12 個）」と知らせた
- `follow`（V1 を BG に合わせる）の変換は答えと一致した（回転 21.0410°、平行移動の差 0）。V1 の検証点の RMS 0.305 m は BG の位置合わせの誤差（BAG と IFC の外形の差）と同じ
- 合成の図面では出ない癖が 2 つあった。共通する円 72 個の中身は、半径ほぼ 0（5×10⁻¹¹ m）の円 40 個と半径 0.03 m の円 32 個で、同じ位置に重なった円が多く、異なる組は 32 しかなかった。図面をそのまま複製したので半径 0 の円も一致したが、別々に描いた階では一致しない

## 測量図面（座標付き）

### BODIK に登録された自治体の基本図・地形図 DXF（CC BY）

[BODIK ODCS](https://odcs.bodik.jp/)（カタログ本体は <https://data.bodik.jp/>）に、自治体の基本図・地形図が CC BY の DXF で登録されている。公開デモ・CI のテストにも使える（出典を明記する）。

| データ | 形式 | ライセンス |
|---|---|---|
| [北九州市 基本図CADデータ（1:2500）](https://data.bodik.jp/dataset/aa61f3ed-f345-4329-a07c-b598652d9696) | DXF（図郭ごと、一括 ZIP あり） | CC BY |
| [北九州市 基本図CADデータ（1:10000）](https://data.bodik.jp/dataset/995ec2f1-567b-47cc-83a3-a8f9ec967016) | DXF | CC BY |
| [鹿児島市基本図（DXF・レベル2500）](https://data.bodik.jp/dataset/24e54921-8a24-4794-a48f-da356eb34f03) | DXF（図郭ごと） | CC BY 4.0 |
| [三田市地形図データ](https://data.bodik.jp/dataset/130089ee-5b5a-418a-8d86-a4e8942a76fa) | DXF（ZIP 約 90 MB） | CC BY 2.1 |
| [三重県道路台帳附図](https://data.bodik.jp/dataset/0e9d09c7-ec42-4eb1-a857-c46a844861c8) | DXF（建設事務所ごとの ZIP） | CC BY 4.0 |
| [宮崎市現況図](https://data.bodik.jp/dataset/fabf23fa-f0f2-4917-a1d7-10a400a71821)、[松浦市地形図](https://data.bodik.jp/dataset/00e5d6d5-756e-4ae8-a146-c6fa539098b3) | SFC | CC BY 4.0（SFC 対応は Phase 2） |

建築図面（平面図）の DXF / DWG は見つからなかった（「北九州市 開発許可データ」は中身が PDF）。

### 大阪市地形図 DXF（`real/osaka/`）

- 取得元: <https://www.geospatial.jp/ckan/dataset/r07-_dm-dxf-aprx-pdf>（「04_地形図DXFデータ」約 300 MB）
- 利用条件: 公共測量成果（大阪市・道路管理センター）。GIS の背景図に使うなどの複製には測量法第 43 条・第 44 条の承認が必要な場合がある。**手元での変換検証にだけ使う**

### 神奈川県 3 次元点群の等高線 DXF（`real/kanagawa/`）

- 取得元: <https://www.geospatial.jp/ckan/dataset/kanagawa-2024-pointcloud>（「等高線データ(DXF)」）
- 利用条件: CC BY 4.0。公開デモ・CI のテストに使ってよい（出典を明記する）
