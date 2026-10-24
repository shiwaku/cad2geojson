//! 図面座標の GeoJSON（dwg2geo などが出したもの）に位置合わせを適用し、WGS84 の GeoJSON にする。

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::Error;
use crate::crs::{Crs, Transformer};
use crate::fit::{Similarity, normalize_deg};
use crate::georef::Record;
use crate::part;

/// 経緯度の小数桁。1e-9 度 ≒ 0.1 mm。
const DECIMALS: i32 = 9;

/// 適用の結果の件数。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    /// 部分ごとの出力件数（記録の部分の順）
    pub written: Vec<(String, usize)>,
    /// 載せない部分に入った件数
    pub not_placed: usize,
    /// どの部分にも入らなかった件数
    pub unassigned: usize,
    /// ジオメトリが無い件数
    pub no_geometry: usize,
}

/// `input`（FeatureCollection）に記録の変換を適用する。
///
/// - 図形は外接矩形の中心で部分に振り分け、`part` 属性を付ける
/// - 載せない部分・どの部分にも入らない図形は出力しない
/// - 文字の `text_rotation_deg` を真北基準・反時計回りの度に、`text_height` を m に直す。元の値は
///   `cad_text_rotation_deg` / `cad_text_height` に残す
pub fn apply(input: &Value, record: &Record) -> Result<(Value, Summary), Error> {
    let features = input
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::GeoJson("input is not a FeatureCollection".into()))?;
    let work = record.work_crs()?;
    let to_wgs = Transformer::new(work, Crs::WGS84)?;
    let parts: Vec<_> = record.parts.iter().map(|p| p.part.clone()).collect();

    let mut summary = Summary {
        written: record
            .parts
            .iter()
            .map(|p| (p.part.name.clone(), 0))
            .collect(),
        ..Default::default()
    };
    let mut out = Vec::with_capacity(features.len());
    for f in features {
        let Some(geom) = f.get("geometry").filter(|g| !g.is_null()) else {
            summary.no_geometry += 1;
            continue;
        };
        let props = f.get("properties").and_then(Value::as_object);
        let layer = props.and_then(|p| p.get("layer")).and_then(Value::as_str);
        let Some(center) = bbox_center(geom) else {
            summary.no_geometry += 1;
            continue;
        };
        let Some(i) = part::assign(&parts, center, layer) else {
            summary.unassigned += 1;
            continue;
        };
        let pg = &record.parts[i];
        let Some(t) = pg.transform.filter(|_| pg.part.place) else {
            summary.not_placed += 1;
            continue;
        };

        let mut g = geom.clone();
        map_coords(&mut g, &mut |c| transform_position(c, &t, &to_wgs))?;

        let mut p = props.cloned().unwrap_or_default();
        p.insert("part".into(), json!(pg.part.name));
        fix_text(&mut p, &t, &g, work, &to_wgs)?;

        let mut nf = Map::new();
        nf.insert("type".into(), json!("Feature"));
        if let Some(id) = f.get("id") {
            nf.insert("id".into(), id.clone());
        }
        nf.insert("geometry".into(), g);
        nf.insert("properties".into(), Value::Object(p));
        out.push(Value::Object(nf));
        summary.written[i].1 += 1;
    }

    let fc = json!({
        "type": "FeatureCollection",
        "cad2geojson": {
            "work_crs": record.work_crs,
            "parts": record.parts.iter().filter(|p| p.part.place).map(|p| &p.part.name).collect::<Vec<_>>(),
        },
        "features": out,
    });
    Ok((fc, summary))
}

fn transform_position(c: &mut [Value], t: &Similarity, to_wgs: &Transformer) -> Result<(), Error> {
    let (x, y) = xy(c)?;
    let (wx, wy) = t.apply(x, y);
    let (lon, lat) = to_wgs.transform(wx, wy)?;
    c[0] = json!(round(lon));
    c[1] = json!(round(lat));
    // Z は図面の単位なので縮尺だけ掛ける
    if let Some(z) = c.get(2).and_then(Value::as_f64) {
        c[2] = json!((z * t.scale * 1000.0).round() / 1000.0);
    }
    Ok(())
}

fn round(v: f64) -> f64 {
    let k = 10f64.powi(DECIMALS);
    (v * k).round() / k
}

pub(crate) fn xy(c: &[Value]) -> Result<(f64, f64), Error> {
    match (
        c.first().and_then(Value::as_f64),
        c.get(1).and_then(Value::as_f64),
    ) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(Error::GeoJson("invalid position".into())),
    }
}

/// 位置（[x, y, ...]）ごとに `f` を呼ぶ。
fn map_coords(
    geom: &mut Value,
    f: &mut dyn FnMut(&mut [Value]) -> Result<(), Error>,
) -> Result<(), Error> {
    if let Some(gs) = geom.get_mut("geometries").and_then(Value::as_array_mut) {
        for g in gs {
            map_coords(g, f)?;
        }
        return Ok(());
    }
    match geom.get_mut("coordinates") {
        Some(c) => walk(c, f),
        None => Err(Error::GeoJson("geometry without coordinates".into())),
    }
}

fn walk(v: &mut Value, f: &mut dyn FnMut(&mut [Value]) -> Result<(), Error>) -> Result<(), Error> {
    let Some(arr) = v.as_array_mut() else {
        return Err(Error::GeoJson("invalid coordinates".into()));
    };
    if arr.first().is_some_and(Value::is_number) {
        return f(arr);
    }
    for a in arr {
        walk(a, f)?;
    }
    Ok(())
}

pub(crate) fn bbox_center(geom: &Value) -> Option<[f64; 2]> {
    bbox(geom).map(|b| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0])
}

/// 外接矩形 [xmin, ymin, xmax, ymax]。
pub(crate) fn bbox(geom: &Value) -> Option<[f64; 4]> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for [x, y] in points(geom)? {
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    b[0].is_finite().then_some(b)
}

/// 位置の並び。折れ線は頂点の順に並ぶ。
pub(crate) fn points(geom: &Value) -> Option<Vec<[f64; 2]>> {
    let mut out = vec![];
    let mut g = geom.clone();
    map_coords(&mut g, &mut |c| {
        let (x, y) = xy(c)?;
        out.push([x, y]);
        Ok(())
    })
    .ok()?;
    Some(out)
}

/// 文字の回転角と文字高を、位置合わせに合わせて直す。
fn fix_text(
    p: &mut Map<String, Value>,
    t: &Similarity,
    geom_wgs: &Value,
    work: Crs,
    to_wgs: &Transformer,
) -> Result<(), Error> {
    if let Some(h) = p.get("text_height").and_then(Value::as_f64) {
        p.insert("cad_text_height".into(), json!(h));
        p.insert("text_height".into(), json!(h * t.scale));
    }
    if let Some(r) = p.get("text_rotation_deg").and_then(Value::as_f64) {
        // 作業用の投影座標系の X 軸（グリッド東）が、真東から何度回っているか（子午線収差）
        let conv = match geom_wgs.get("coordinates").and_then(Value::as_array) {
            Some(c) if geom_wgs.get("type") == Some(&json!("Point")) => {
                let (lon, lat) = xy(c)?;
                grid_east_angle(lon, lat, work, to_wgs)?
            }
            _ => 0.0,
        };
        p.insert("cad_text_rotation_deg".into(), json!(r));
        p.insert(
            "text_rotation_deg".into(),
            json!(normalize_deg(r + t.rotation_deg + conv)),
        );
    }
    Ok(())
}

fn grid_east_angle(lon: f64, lat: f64, work: Crs, to_wgs: &Transformer) -> Result<f64, Error> {
    let (x, y) = Transformer::new(Crs::WGS84, work)?.transform(lon, lat)?;
    let (lon1, lat1) = to_wgs.transform(x + 10.0, y)?;
    // 東西と南北の 1 度の長さの比（卯酉線と子午線の曲率半径の比 N/M = 1 + e'²cos²φ、GRS80）
    let c = lat.to_radians().cos();
    let de = (lon1 - lon) * c * (1.0 + 0.006_739_496_775_48 * c * c);
    let dn = lat1 - lat;
    Ok(dn.atan2(de).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::georef::{Gcp, Request, Role, ScaleMode, solve};
    use crate::part::Part;

    fn record() -> Record {
        // 図面 (0,0) → 5 系 (-90000, -63000)、(1000, 0) → 北へ 1 m（回転 90°）
        let gcps = vec![
            Gcp {
                part: "A".into(),
                cad: [0.0, 0.0],
                map: [-90000.0, -63000.0],
                role: Role::Control,
            },
            Gcp {
                part: "A".into(),
                cad: [1000.0, 0.0],
                map: [-90000.0, -62999.0],
                role: Role::Control,
            },
        ];
        let parts = vec![
            Part {
                name: "A".into(),
                bbox: Some([-5000.0, -5000.0, 5000.0, 5000.0]),
                layers: vec![],
                place: true,
                follow: None,
            },
            Part {
                name: "frame".into(),
                bbox: Some([10000.0, 0.0, 20000.0, 10000.0]),
                layers: vec![],
                place: false,
                follow: None,
            },
        ];
        let c = Crs::from_epsg(6673).unwrap();
        solve(&Request {
            parts,
            gcps,
            gcp_crs: c,
            work_crs: c,
            scale: ScaleMode::Fixed(0.001),
            relations: Default::default(),
        })
        .unwrap()
    }

    #[test]
    fn applies_parts_and_text() {
        let input = json!({
            "type": "FeatureCollection",
            "dwg2geo": {"coordinate_status": "local-unreferenced"},
            "features": [
                {"type": "Feature", "id": "0x1", "geometry": {"type": "LineString", "coordinates": [[0, 0], [1000, 0]]},
                 "properties": {"layer": "W"}},
                {"type": "Feature", "id": "0x2", "geometry": {"type": "Point", "coordinates": [0, 0]},
                 "properties": {"text": "あ", "text_height": 500.0, "text_rotation_deg": 0.0}},
                {"type": "Feature", "geometry": {"type": "Point", "coordinates": [15000, 5000]}, "properties": {}},
                {"type": "Feature", "geometry": {"type": "Point", "coordinates": [99999, 0]}, "properties": {}},
                {"type": "Feature", "geometry": null, "properties": {}}
            ]
        });
        let (out, s) = apply(&input, &record()).unwrap();
        assert_eq!(s.written, vec![("A".into(), 2), ("frame".into(), 0)]);
        assert_eq!((s.not_placed, s.unassigned, s.no_geometry), (1, 1, 1));
        assert!(out.get("dwg2geo").is_none());

        let fs = out["features"].as_array().unwrap();
        let line = fs[0]["geometry"]["coordinates"].as_array().unwrap();
        let (lon0, lat0) = (line[0][0].as_f64().unwrap(), line[0][1].as_f64().unwrap());
        let (lon1, lat1) = (line[1][0].as_f64().unwrap(), line[1][1].as_f64().unwrap());
        // 子午線収差の分だけ経度もわずかに変わる（1 m で約 1 cm）
        assert!((lon0 - lon1).abs() < 1e-6 && lat1 > lat0);
        assert_eq!(fs[0]["properties"]["part"], "A");
        assert_eq!(fs[0]["id"], "0x1");

        let p = &fs[1]["properties"];
        assert_eq!(p["cad_text_height"], 500.0);
        assert!((p["text_height"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        // 回転 90° に、子午線収差を足す。5 系の原点より西なので、グリッド東は真東より
        // 反時計回りに回っている（PROJ の測地線で 0.5746°）
        let rot = p["text_rotation_deg"].as_f64().unwrap();
        assert!((rot - 90.5746).abs() < 1e-3, "{rot}");
    }

    #[test]
    fn grid_east_is_true_east_on_central_meridian() {
        let c = Crs::from_epsg(6673).unwrap();
        let to = Transformer::new(c, Crs::WGS84).unwrap();
        let a = grid_east_angle(134.0 + 20.0 / 60.0, 35.0, c, &to).unwrap();
        assert!(a.abs() < 1e-4, "{a}");
    }
}
