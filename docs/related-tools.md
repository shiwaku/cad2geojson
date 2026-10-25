# 関連ツールの調査メモ

cad2geojson を作る過程で調べたツールと、cad2geojson との関係。

| ツール | 種類 | DWG | DXF | cad2geojson との関係 |
|---|---|---|---|---|
| [dwg2geo](https://github.com/milkway/dwg2geo)（MIT） | DWG → GeoJSON の変換器 | 読む | 読まない | **土台**。DWG の図形の取り出しを任せる（[評価](eval-dwg2geo.md)） |
| [acadrust](https://github.com/hakanaktt/acadrust)（MPL-2.0） | DWG / DXF の読み書きライブラリ（Rust） | 読み書き | 読み書き | **DXF の読み込みに使う**（DXF → メモリ上の DWG → dwg2geo） |
| GDAL（OGR DXF ドライバ） | 地理空間データの変換 | 読まない | 読む | 比較に使用。R12（文字コードの記録なし）で GeoJSON が壊れ、R2010 で文字化けした（[評価](eval-dwg2geo.md)） |
| ODA File Converter（無料、ODA） | DWG / DXF の版の変換 | 読み書き | 読み書き | 検証用の DWG を作るのに使用。読めない DWG を DXF に変換する経路として案内する |
| LibreDWG | DWG の読み書きライブラリ（C） | 読み書き | 読み書き | dwg2geo の外部バックエンドが使う。cad2geojson では使わない |
| [GeoLibre](https://github.com/opengeos/GeoLibre)（MIT） | ブラウザ・デスクトップ（Tauri）・モバイルの GIS | 一覧に記載（ドロップでは受けない） | 読む（DuckDB-WASM の `ST_Read`） | **Phase 2 の参考**。対応点の画面と残差の表、WASM での文字コードの直し方（下記） |
| [EchoCad](https://github.com/EchoPrime-Studio/echocad)（GPL-2.0） | QGIS プラグイン | 有料版のみ | 読む（GDAL） | 考え方だけ参考にする。文字コードの判定、遠く離れた図形、読み落としの報告（下記） |
| [02CadGis](https://gitlab.com/geophilo1/zero2cadgis)（GPL-2.0-or-later） | QGIS プラグイン | 読む（ODA） | 読む（GDAL） | 考え方だけ参考にする。座標系の推定、変換の記録（下記） |
| [Vernier](https://github.com/ryxlabs/vernier)（GPL-2.0） | QGIS プラグイン（ベクター編集ツール集） | 読む（ODA で DXF に変換） | 読み書き（GDAL・ezdxf） | 関係は薄い（下記） |
| [SolveCraft](https://github.com/bherbruck/solvecraft)（MIT / Apache-2.0） | 3D パラメトリック CAD（Rust、Fusion 風） | **非対応** | 板金の展開図の書き出しのみ | **関係なし**（下記） |

## SolveCraft（2026-10-12 確認）

Rust 製のオープンソースの 3D パラメトリック CAD（[fabscene の記事](https://fabscene.com/new/make/solvecraft-open-source-parametric-3d-cad-rust-fusion-76-parts-mcp/)）。

- 読み込み: STEP（AP203 / AP214 / AP242）、IGES、3MF・STL・OBJ（メッシュとして）、独自形式（.solvecraft、JSON）
- 書き出し: STEP（AP242）、IGES 5.3、3MF、STL、OBJ、独自形式
- DXF: 板金の展開図（線・円弧・円の外形と曲げ表）の書き出しのみ。DXF の読み込みは無い
- DWG: README・記事とも言及なし

STEP を中心とした 3D 形状の CAD で、2D の建築・土木図面（DWG / DXF）を扱う道具ではない。cad2geojson の入力の作り手にも、検証用の図面の出どころにもならない。

## QGIS プラグインと GeoLibre（2026-10-25 確認）

cad2geojson と同じく DWG / DXF を GIS に持ち込むツール。どれも図面の位置合わせ（対応点で図面座標を地図に合わせる）はせず、座標系は利用者が選ぶか、図面がすでに測量座標で描かれている前提で推定する。部分分けや部分どうしの関係も無い。

EchoCad・02CadGis・Vernier は GPL なので、cad2geojson（MIT）にはコードを取り込まず、考え方だけを参考にする。GeoLibre は MIT なので、出典を書けば取り込める。

**検証データ**: 4 つとも実在の図面をリポジトリに入れていない。テストの図面はコードで作る（GeoLibre の `tests/cad-encoding.test.ts` は `$ACADVER`・`$DWGCODEPAGE`・文字列を変えた小さな DXF をテストの中で組み立てる）。cad2geojson の合成の図面（`tools/gen_testdata`）と同じ考え方で、公開できない図面で確かめたことは、合成の図面の種類を増やして置き換える。

### GeoLibre

[opengeos](https://github.com/opengeos) の GIS（★8,100、2026-10 も更新中）。Web 版・Tauri v2 のデスクトップ版・Android / iOS・Jupyter で動く。

- CAD: DuckDB-WASM の `ST_Read`（中身は GDAL）で DXF を読む。座標系は「CAD には座標系が無いので EPSG を手で選ぶ」（[対応形式](https://geolibre.app/tutorials/supported-formats-en/)）。図面の位置合わせは無い
- 文字コード（`apps/geolibre-desktop/src/lib/cad-encoding.ts`）: WASM の GDAL には iconv が無く、ファイルの各バイトが Latin-1 の 1 文字になる。GeoJSON にしたあとで文字列を元のバイトに戻し、`$DWGCODEPAGE`（`ANSI_932` → `shift_jis` など）で読み直す。R2007（AC1021）以降は UTF-8。ヘッダーはグループコードの組で読み（MTEXT の中の `$DWGCODEPAGE` という文字に騙されない）、先頭の BOM を除く。Phase 2 で GDAL の WASM を使うなら同じ問題に当たる（cad2geojson は acadrust で読むので当たらない）
- 位置合わせ（`georeference.ts`・`GeoreferencerDialog.tsx`）: ラスター画像用。画素と経緯度の対応点 3 点以上でアフィン変換を最小二乗で求め、点ごとの残差（m）と RMS を表に出す。対応点は CSV でも読める。Phase 2 の「対応点を打つ画面、残差の表示」の手本になる。ただし経緯度に直接アフィンを当てるので、cad2geojson の作業用の投影座標系での相似変換・検証点とは違う

### EchoCad（v0.2.8、EchoPrime Studio）

DXF を、レイヤ・色・線種・線幅・ハッチ・文字を保って QGIS に読み込む。無料版は DXF のみ（GDAL）、DWG は有料版。

- 文字コード（`dxfenc.py`）: ヘッダーではなくバイトで判定する。dwg2dxf は中身を UTF-8 で書きながら `$DWGCODEPAGE` に元の DWG の地域の文字コード（`ANSI_1252`・`ANSI_949` など）を残すことがある。逆に、記録も中身も本当に CP949 の図面もある。ファイル全体が UTF-8 として読めるかで決める
- 遠く離れた図形（`outliers.py`）: 中央値と四分位の幅から、本体の幅の 100 倍より遠い点を壊れた座標として捨てる（GDAL のハッチの読み取りが 10¹³ ほどの座標を出すことがある）。それとは別に、本体の幅の 3 倍より遠い点を最初の表示範囲から外す（原点に正当な点が 1 つある図面など。データは消さない）。外れる点が全体の 1% を超えるときは、図面が 2 枚並んでいるとみなして外さない。cad2geojson の `parts` は図面の範囲の長辺の 3% をすき間の既定にしているので、遠い図形が 1 つあるだけで範囲が膨らむ
- 読み落としの報告（`census.py`・`importreport.py`）: 図面の ENTITIES をファイルから直接数え（モデル空間のみ）、出力の図形のハンドルと突き合わせて、読めなかった図形をハンドル単位で出し、理由（プロキシ、未対応の種類、無限線、外部参照、非表示のレイヤ、中身の無いブロックなど）を付ける

### 02CadGis（v4.1.2、Yusuf Eminoglu）

DWG / DXF（GDAL と ODA）のほか、トルコの Netcad（.ncz）・KML・GML・DGN・GDB などを GeoPackage にする。

- 座標系の推定（`core/crs_detect.py`）: 図面にある投影法の文字列で測地系（TUREF / ED50）を決め、東距の桁数で軸の形（TM か、帯番号を東距に含む Gauss-Krüger か）を決める。結果に確からしさ（high / medium / none）と理由を付けて画面に出す。測量座標で描かれた日本の土木図面なら、座標の範囲から平面直角座標系の系を候補に出す、という形で応用できる
- 変換の記録（`core/conversion_receipt.py`）: 入力・出力・座標系・レイヤごとの図形の数・警告を、コピーできる文字で出す。cad2geojson の位置合わせの記録 JSON に近い
- 線の面化（`core/cad_polygonizer.py`）: 都市計画図の区画の境界線（閉じていない線）を交点で分けてから面にする。閉じたポリラインは二重にならないよう除く

### Vernier（v0.10.0、RyxLabs）

ベクター編集ツール集の一部として、DXF / DWG の読み込みとラベル付き DXF の書き出しを持つ。DWG は ODA File Converter で DXF にし、ogr2ogr で GeoPackage にして、ezdxf で読んだレイヤの色から QML を作る。位置合わせ・文字コードの扱いは無く、cad2geojson の参考になる点は少ない。
