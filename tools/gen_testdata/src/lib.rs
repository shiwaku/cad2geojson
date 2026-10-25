//! 位置合わせの答えが分かっている合成の図面を作る。
//!
//! 1 枚の図面に 1 階・2 階・図枠を並べる（実在の建物の全体平面図によくある構成）。両階は同じ建物なので
//! 柱（円）の位置が共通で、1 階は 2 階に対して少し回転して描かれている。2 階の図面座標から作業用の
//! 投影座標系への変換（[`TRUTH`]）を決めておき、対応点・検証点の地図側の座標をそこから計算する。
//!
//! 書き出すもの:
//!
//! - `plan_r12.dxf`: R12（AC1009）、文字は cp932、`$DWGCODEPAGE` なし（Jw_cad が書き出す DXF と同じ形）
//! - `plan_r2010.dxf`: R2010（AC1024）、文字は UTF-8（acadrust で書く）
//! - `plan_r2018.dwg`: R2018（AC1032）の DWG（acadrust で書く）
//! - `plan_utf8_bom.dxf`: R12 で `$DWGCODEPAGE` は `ANSI_932` なのに、文字は UTF-8 で先頭に BOM がある
//!   （cp932 の DXF をテキストエディタで UTF-8 にして保存し直したもの、記録を残したまま UTF-8 で書く変換器）
//! - `plan_stray.dxf`: R2010 に、本体から 500 km 離れた線を 1 本足したもの（原点や壊れた座標に残った図形）
//! - `parts.json`: 図枠（載せない）・2 階・1 階（2 階に合わせる）
//! - `gcp.csv`: 2 階の対応点 2 点と検証点 2 点、1 階の検証点 3 点

use std::fmt::Write as _;
use std::path::Path;

use acadrust::entities::{Circle, EntityType, Line, Text};
use acadrust::types::{DxfVersion, Vector3};
use acadrust::{CadDocument, DwgWriter, DxfWriter, Layer};

/// 作業用の投影座標系（平面直角座標系 IX 系、JGD2011）。
pub const WORK_CRS: &str = "EPSG:6677";
/// 図面の単位（mm）。
pub const UNITS_M: f64 = 0.001;

/// 2 階の図面座標 → 作業用の投影座標系（m）の変換。
pub const TRUTH: Similarity = Similarity {
    rotation_deg: 35.0,
    translation: [-7000.0, -38000.0],
};

/// 1 階を 2 階に対して回して描いた角度（図面上、反時計回り）。
pub const FLOOR1_SKEW_DEG: f64 = -0.2;

/// 柱の通り（建物の座標、mm）。間隔をそろえないのは、ずらした位置でも多くの柱が重なるのを避けるため。
pub const GRID_X: [f64; 6] = [0.0, 6300.0, 14100.0, 21000.0, 29400.0, 35100.0];
pub const GRID_Y: [f64; 4] = [0.0, 7500.0, 13800.0, 22000.0];
pub const COLUMN_RADIUS: f64 = 400.0;

/// 各階の建物の原点を置く図面座標。
const ORIGIN_1F: [f64; 2] = [15000.0, 12000.0];
const ORIGIN_2F: [f64; 2] = [15000.0, 62000.0];

/// 縮尺を除いた相似変換（回転と平行移動）。図面座標は [`UNITS_M`] で m にしてから回す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Similarity {
    pub rotation_deg: f64,
    pub translation: [f64; 2],
}

fn rotate(p: [f64; 2], deg: f64) -> [f64; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

/// 階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Floor {
    F1,
    F2,
}

impl Floor {
    pub fn name(self) -> &'static str {
        match self {
            Floor::F1 => "1F",
            Floor::F2 => "2F",
        }
    }

    /// 建物の座標 → この階の図面座標。
    pub fn to_drawing(self, q: [f64; 2]) -> [f64; 2] {
        let (o, r) = match self {
            Floor::F1 => (ORIGIN_1F, rotate(q, FLOOR1_SKEW_DEG)),
            Floor::F2 => (ORIGIN_2F, q),
        };
        [o[0] + r[0], o[1] + r[1]]
    }

    fn skew_deg(self) -> f64 {
        match self {
            Floor::F1 => FLOOR1_SKEW_DEG,
            Floor::F2 => 0.0,
        }
    }

    /// この階の図面座標 → 作業用の投影座標系（m）の、正しい変換。
    pub fn truth(self) -> Similarity {
        // 図面座標 p → 建物の座標 q = R(−skew)(p − o) → 2 階の図面座標 q + o2 → TRUTH
        let o = match self {
            Floor::F1 => ORIGIN_1F,
            Floor::F2 => ORIGIN_2F,
        };
        let rot = TRUTH.rotation_deg - self.skew_deg();
        let back = rotate(o, -self.skew_deg());
        let shift = rotate(
            [
                (ORIGIN_2F[0] - back[0]) * UNITS_M,
                (ORIGIN_2F[1] - back[1]) * UNITS_M,
            ],
            TRUTH.rotation_deg,
        );
        Similarity {
            rotation_deg: rot,
            translation: [
                TRUTH.translation[0] + shift[0],
                TRUTH.translation[1] + shift[1],
            ],
        }
    }
}

impl Similarity {
    /// 図面座標を作業用の投影座標系（m）にする。
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let r = rotate([p[0] * UNITS_M, p[1] * UNITS_M], self.rotation_deg);
        [r[0] + self.translation[0], r[1] + self.translation[1]]
    }
}

/// 建物の座標で書いた柱の位置を、地図（作業用の投影座標系、m）の位置にする。両階で同じ。
pub fn map_of(q: [f64; 2]) -> [f64; 2] {
    TRUTH.apply(Floor::F2.to_drawing(q))
}

/// 図形。座標は図面座標。
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line([f64; 2], [f64; 2]),
    Circle([f64; 2], f64),
    Text {
        at: [f64; 2],
        height: f64,
        rotation_deg: f64,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub layer: &'static str,
    pub shape: Shape,
}

pub const LAYER_COLUMN: &str = "柱";
pub const LAYER_WALL: &str = "壁";
pub const LAYER_TEXT: &str = "文字";
pub const LAYER_FRAME: &str = "図枠";
/// レイヤと色。
pub const LAYERS: [(&str, i16); 4] = [
    (LAYER_COLUMN, 1),
    (LAYER_WALL, 7),
    (LAYER_TEXT, 3),
    (LAYER_FRAME, 8),
];

struct Builder {
    floor: Floor,
    out: Vec<Entity>,
}

impl Builder {
    fn line(&mut self, a: [f64; 2], b: [f64; 2]) {
        let shape = Shape::Line(self.floor.to_drawing(a), self.floor.to_drawing(b));
        self.out.push(Entity {
            layer: LAYER_WALL,
            shape,
        });
    }

    fn rect(&mut self, a: [f64; 2], b: [f64; 2]) {
        self.line([a[0], a[1]], [b[0], a[1]]);
        self.line([b[0], a[1]], [b[0], b[1]]);
        self.line([b[0], b[1]], [a[0], b[1]]);
        self.line([a[0], b[1]], [a[0], a[1]]);
    }

    fn circle(&mut self, c: [f64; 2], r: f64) {
        let shape = Shape::Circle(self.floor.to_drawing(c), r);
        self.out.push(Entity {
            layer: LAYER_COLUMN,
            shape,
        });
    }

    fn text(&mut self, at: [f64; 2], height: f64, rotation_deg: f64, value: &str) {
        let shape = Shape::Text {
            at: self.floor.to_drawing(at),
            height,
            rotation_deg: rotation_deg + self.floor.skew_deg(),
            value: value.into(),
        };
        self.out.push(Entity {
            layer: LAYER_TEXT,
            shape,
        });
    }
}

fn floor(floor: Floor) -> Vec<Entity> {
    let mut b = Builder { floor, out: vec![] };
    let (x_max, y_max) = (GRID_X[5], GRID_Y[3]);
    // 外壁と階段（両階で共通）
    b.rect([-500.0, -500.0], [x_max + 500.0, y_max + 500.0]);
    b.rect([GRID_X[4], 0.0], [x_max, GRID_Y[1]]);
    for &x in &GRID_X {
        for &y in &GRID_Y {
            b.circle([x, y], COLUMN_RADIUS);
        }
    }
    b.text([31000.0, 3000.0], 500.0, 0.0, "階段");
    match floor {
        Floor::F1 => {
            b.line([GRID_X[2], -500.0], [GRID_X[2], y_max + 500.0]);
            b.line([GRID_X[2], GRID_Y[2]], [x_max + 500.0, GRID_Y[2]]);
            b.line([GRID_X[4], GRID_Y[2]], [GRID_X[4], y_max + 500.0]);
            // 庇の柱（1 階だけ、半径も違う）
            for y in [3000.0, 9000.0, 15000.0] {
                b.circle([-2500.0, y], 300.0);
            }
            b.text([2000.0, 10000.0], 500.0, 0.0, "エントランスホール");
            b.text([20000.0, 5000.0], 500.0, 0.0, "展示室");
            b.text([20000.0, 18000.0], 500.0, 0.0, "事務室");
            b.text([31000.0, 18000.0], 500.0, 0.0, "倉庫");
            b.text([0.0, -2500.0], 1000.0, 0.0, "１階 平面図");
        }
        Floor::F2 => {
            b.line([GRID_X[3], -500.0], [GRID_X[3], y_max + 500.0]);
            b.line([-500.0, GRID_Y[1]], [GRID_X[3], GRID_Y[1]]);
            // 天窓（2 階だけ）
            for x in [10000.0, 17500.0] {
                b.circle([x, 11000.0], 250.0);
            }
            b.text([3000.0, 3000.0], 500.0, 0.0, "会議室Ａ");
            b.text([3000.0, 15000.0], 500.0, 0.0, "会議室Ｂ");
            b.text([25000.0, 15000.0], 500.0, 0.0, "ラウンジ");
            b.text([24000.0, 2000.0], 500.0, 90.0, "吹抜");
            b.text([0.0, -2500.0], 1000.0, 0.0, "２階 平面図");
        }
    }
    b.out
}

/// 図枠と表題欄（図面座標そのまま）。
fn frame() -> Vec<Entity> {
    let mut out = vec![];
    let mut rect = |a: [f64; 2], b: [f64; 2]| {
        for (p, q) in [
            ([a[0], a[1]], [b[0], a[1]]),
            ([b[0], a[1]], [b[0], b[1]]),
            ([b[0], b[1]], [a[0], b[1]]),
            ([a[0], b[1]], [a[0], a[1]]),
        ] {
            out.push(Entity {
                layer: LAYER_FRAME,
                shape: Shape::Line(p, q),
            });
        }
    };
    rect([0.0, 0.0], [65000.0, 100000.0]);
    rect([1000.0, 1000.0], [64000.0, 99000.0]);
    rect([40000.0, 1000.0], [64000.0, 9000.0]);
    out.push(Entity {
        layer: LAYER_FRAME,
        shape: Shape::Line([40000.0, 5000.0], [64000.0, 5000.0]),
    });
    for (at, height, value) in [
        ([41000.0, 6500.0], 1500.0, "合成テスト建物"),
        ([41000.0, 2500.0], 1200.0, "１階・２階 平面図"),
        ([56000.0, 2500.0], 800.0, "縮尺 1/200"),
        ([2000.0, 97000.0], 800.0, "cad2geojson 検証用の生成データ"),
    ] {
        out.push(Entity {
            layer: LAYER_FRAME,
            shape: Shape::Text {
                at,
                height,
                rotation_deg: 0.0,
                value: value.into(),
            },
        });
    }
    out
}

/// 図面の全図形（2 階・1 階・図枠の順）。
pub fn entities() -> Vec<Entity> {
    let mut out = floor(Floor::F2);
    out.extend(floor(Floor::F1));
    out.extend(frame());
    out
}

/// 本体から遠く離れた線（図面座標、mm）。
pub const STRAY_AT: [f64; 2] = [5.0e8, 5.0e8];

/// [`entities`] に、本体から遠く離れた線を 1 本足したもの。
pub fn entities_with_stray() -> Vec<Entity> {
    let mut out = entities();
    out.push(Entity {
        layer: LAYER_WALL,
        shape: Shape::Line(STRAY_AT, [STRAY_AT[0] + 1000.0, STRAY_AT[1]]),
    });
    out
}

/// 部分の定義。図枠をレイヤで先に取り、残りを範囲で 2 階・1 階に分ける。
pub fn parts_json() -> String {
    r#"{
  "parts": [
    { "name": "frame", "layers": ["図枠"], "place": false },
    { "name": "2F", "bbox": [5000, 56000, 60000, 90000] },
    { "name": "1F", "bbox": [5000, 8500, 60000, 40000], "follow": "2F" }
  ]
}
"#
    .to_string()
}

/// 対応点と検証点。2 階は対応点 2 点・検証点 2 点、1 階は検証点 3 点（位置は 2 階との関係で決まる）。
pub fn gcp_csv() -> String {
    let mut s = String::from("part,cad_x,cad_y,map_x,map_y,role\n");
    let g = |i: usize, j: usize| [GRID_X[i], GRID_Y[j]];
    let points = [
        (Floor::F2, g(0, 0), "control"),
        (Floor::F2, g(5, 3), "control"),
        (Floor::F2, g(2, 1), "check"),
        (Floor::F2, g(4, 3), "check"),
        (Floor::F1, g(1, 2), "check"),
        (Floor::F1, g(5, 0), "check"),
        (Floor::F1, g(3, 3), "check"),
    ];
    for (floor, q, role) in points {
        let c = floor.to_drawing(q);
        let m = map_of(q);
        writeln!(
            s,
            "{},{:.6},{:.6},{:.6},{:.6},{role}",
            floor.name(),
            c[0],
            c[1],
            m[0],
            m[1]
        )
        .unwrap();
    }
    s
}

/// R12 の DXF。Jw_cad と同じく、文字は cp932 で `$DWGCODEPAGE` を書かない。改行は CRLF。
pub fn dxf_r12() -> Vec<u8> {
    let s = r12_text(&entities(), None);
    let (bytes, _, unmappable) = encoding_rs::SHIFT_JIS.encode(&s);
    assert!(!unmappable, "text not representable in cp932");
    bytes.into_owned()
}

/// R12 の DXF。`$DWGCODEPAGE` は `ANSI_932` なのに、文字は UTF-8 で先頭に BOM がある。
pub fn dxf_utf8_bom() -> Vec<u8> {
    let mut out = b"\xEF\xBB\xBF".to_vec();
    out.extend(r12_text(&entities(), Some("ANSI_932")).into_bytes());
    out
}

/// R12 の DXF の中身（文字コードにする前）。
fn r12_text(es: &[Entity], code_page: Option<&str>) -> String {
    let mut s = String::new();
    let mut g = |code: i32, v: &str| {
        write!(s, "{code:>3}\r\n{v}\r\n").unwrap();
    };
    let (min, max) = extents(es);
    g(0, "SECTION");
    g(2, "HEADER");
    g(9, "$ACADVER");
    g(1, "AC1009");
    if let Some(c) = code_page {
        g(9, "$DWGCODEPAGE");
        g(3, c);
    }
    g(9, "$INSBASE");
    g(10, "0");
    g(20, "0");
    g(30, "0");
    g(9, "$EXTMIN");
    g(10, &num(min[0]));
    g(20, &num(min[1]));
    g(9, "$EXTMAX");
    g(10, &num(max[0]));
    g(20, &num(max[1]));
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "TABLES");
    g(0, "TABLE");
    g(2, "LTYPE");
    g(70, "1");
    g(0, "LTYPE");
    g(2, "CONTINUOUS");
    g(70, "64");
    g(3, "実線");
    g(72, "65");
    g(73, "0");
    g(40, "0.0");
    g(0, "ENDTAB");
    g(0, "TABLE");
    g(2, "LAYER");
    g(70, &LAYERS.len().to_string());
    for (name, color) in LAYERS {
        g(0, "LAYER");
        g(2, name);
        g(70, "0");
        g(62, &color.to_string());
        g(6, "CONTINUOUS");
    }
    g(0, "ENDTAB");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "ENTITIES");
    for e in es {
        match &e.shape {
            Shape::Line(a, b) => {
                g(0, "LINE");
                g(8, e.layer);
                g(10, &num(a[0]));
                g(20, &num(a[1]));
                g(11, &num(b[0]));
                g(21, &num(b[1]));
            }
            Shape::Circle(c, r) => {
                g(0, "CIRCLE");
                g(8, e.layer);
                g(10, &num(c[0]));
                g(20, &num(c[1]));
                g(40, &num(*r));
            }
            Shape::Text {
                at,
                height,
                rotation_deg,
                value,
            } => {
                g(0, "TEXT");
                g(8, e.layer);
                g(10, &num(at[0]));
                g(20, &num(at[1]));
                g(40, &num(*height));
                g(41, "1");
                g(50, &num(*rotation_deg));
                g(1, value);
            }
        }
    }
    g(0, "ENDSEC");
    g(0, "EOF");
    s
}

/// R2010 の DXF（acadrust で書く。文字は UTF-8）。
pub fn dxf_r2010() -> Vec<u8> {
    write_dxf(&document(&entities(), DxfVersion::AC1024))
}

/// R2010 の DXF に、本体から遠く離れた線を 1 本足したもの。
pub fn dxf_stray() -> Vec<u8> {
    write_dxf(&document(&entities_with_stray(), DxfVersion::AC1024))
}

/// R2018 の DWG（acadrust で書く）。
pub fn dwg_r2018() -> Vec<u8> {
    DwgWriter::write_to_vec(&document(&entities(), DxfVersion::AC1032))
        .expect("DWG written to memory")
}

fn write_dxf(doc: &CadDocument) -> Vec<u8> {
    DxfWriter::new(doc)
        .write_to_vec()
        .expect("DXF written to memory")
}

fn document(es: &[Entity], version: DxfVersion) -> CadDocument {
    let mut doc = CadDocument::with_version(version);
    for (name, color) in LAYERS {
        let mut l = Layer::new(name);
        l.color = acadrust::types::Color::from_index(color);
        doc.layers.add(l).expect("unique layer names");
    }
    let v = |p: [f64; 2]| Vector3::new(p[0], p[1], 0.0);
    for e in es.iter().cloned() {
        let mut ent = match e.shape {
            Shape::Line(a, b) => EntityType::Line(Line::from_points(v(a), v(b))),
            Shape::Circle(c, r) => EntityType::Circle(Circle::from_center_radius(v(c), r)),
            Shape::Text {
                at,
                height,
                rotation_deg,
                value,
            } => {
                let mut t = Text::with_value(value, v(at));
                t.height = height;
                t.rotation = rotation_deg.to_radians();
                EntityType::Text(t)
            }
        };
        ent.common_mut().layer = e.layer.to_string();
        doc.add_entity(ent).expect("entity accepted");
    }
    doc
}

/// 書き出すファイル（名前, 中身）。
pub fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("plan_r12.dxf", dxf_r12()),
        ("plan_r2010.dxf", dxf_r2010()),
        ("plan_r2018.dwg", dwg_r2018()),
        ("plan_utf8_bom.dxf", dxf_utf8_bom()),
        ("plan_stray.dxf", dxf_stray()),
        ("parts.json", parts_json().into_bytes()),
        ("gcp.csv", gcp_csv().into_bytes()),
    ]
}

pub fn write_all(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in files() {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// 数値の表記。整数なら小数点を付けず、そうでなければ μm（1e-6 mm）までで丸める。
fn num(v: f64) -> String {
    let r = (v * 1e6).round() / 1e6;
    let s = format!("{r:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn extents(es: &[Entity]) -> ([f64; 2], [f64; 2]) {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    let mut add = |p: [f64; 2], r: f64| {
        for k in 0..2 {
            min[k] = min[k].min(p[k] - r);
            max[k] = max[k].max(p[k] + r);
        }
    };
    for e in es {
        match &e.shape {
            Shape::Line(a, b) => {
                add(*a, 0.0);
                add(*b, 0.0);
            }
            Shape::Circle(c, r) => add(*c, *r),
            Shape::Text { at, .. } => add(*at, 0.0),
        }
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_share_the_map_position_of_columns() {
        for q in [[0.0, 0.0], [GRID_X[5], GRID_Y[3]], [GRID_X[2], GRID_Y[1]]] {
            let m = map_of(q);
            for f in [Floor::F1, Floor::F2] {
                let p = f.truth().apply(f.to_drawing(q));
                assert!((p[0] - m[0]).abs() < 1e-9 && (p[1] - m[1]).abs() < 1e-9);
            }
        }
        assert_eq!(Floor::F1.truth().rotation_deg, 35.2);
    }

    #[test]
    fn numbers() {
        assert_eq!(num(15000.0), "15000");
        assert_eq!(num(-0.0000001), "0");
        assert_eq!(num(1.25), "1.25");
    }

    /// リポジトリの testdata/synthetic が生成器と食い違っていないか。食い違ったら
    /// `cargo run -p cad2geojson-gen-testdata` で書き直す。
    #[test]
    fn committed_files_are_current() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/synthetic");
        for (name, bytes) in files() {
            let committed = std::fs::read(dir.join(name)).unwrap();
            assert!(
                committed == bytes,
                "testdata/synthetic/{name} is stale; run `cargo run -p cad2geojson-gen-testdata`"
            );
        }
    }

    #[test]
    fn r12_is_cp932_without_code_page() {
        let b = dxf_r12();
        assert!(std::str::from_utf8(&b).is_err());
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("AC1009") && !s.contains("$DWGCODEPAGE"));
    }

    #[test]
    fn utf8_bom_declares_cp932() {
        let b = dxf_utf8_bom();
        let s = std::str::from_utf8(b.strip_prefix(b"\xEF\xBB\xBF").unwrap()).unwrap();
        assert!(
            s.contains("$DWGCODEPAGE\r\n  3\r\nANSI_932\r\n") && s.contains("エントランスホール")
        );
    }

    /// DWG を書き直しても同じバイト列になる（committed_files_are_current の前提）。
    #[test]
    fn dwg_is_deterministic() {
        assert!(dwg_r2018() == dwg_r2018());
    }
}
