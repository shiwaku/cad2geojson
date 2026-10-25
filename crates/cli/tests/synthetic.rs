//! 答えが分かっている合成の図面（testdata/synthetic、tools/gen_testdata で生成）で、
//! 部分分け・`follow`・位置合わせを CLI ごと確かめる。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use cad2geojson_core::georef::Record;
use cad2geojson_gen_testdata as gen_data;
use gen_data::Floor;
use serde_json::Value;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/synthetic")
        .join(name)
}

/// 呼ぶたびに別の出力先。テストは並行して走るので分ける。
fn out_dir(test: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "cad2geojson-synthetic-{}-{test}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `georef` を回し、記録と出力の図形を返す。
fn georef(dxf: &str, parts: &Path, out: &Path) -> (Record, Vec<Value>) {
    let stem = dxf.replace('.', "_");
    let output = out.join(format!("{stem}.geojson"));
    let record = out.join(format!("{stem}.georef.json"));
    let r = Command::new(env!("CARGO_BIN_EXE_cad2geojson"))
        .arg("georef")
        .arg(data(dxf))
        .arg("--gcp")
        .arg(data("gcp.csv"))
        .arg("--parts")
        .arg(parts)
        .args(["--work-crs", gen_data::WORK_CRS, "--units", "mm", "-o"])
        .arg(&output)
        .arg("--record")
        .arg(&record)
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "georef {dxf} failed:\n{}",
        String::from_utf8_lossy(&r.stderr)
    );
    let rec = Record::from_json(&std::fs::read_to_string(&record).unwrap()).unwrap();
    let gj: Value = serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    (rec, gj["features"].as_array().unwrap().clone())
}

fn check_record(rec: &Record) {
    let part = |n: &str| rec.parts.iter().find(|p| p.part.name == n).unwrap();
    assert!(part("frame").transform.is_none());
    for floor in [Floor::F2, Floor::F1] {
        let p = part(floor.name());
        let t = p.transform.unwrap();
        let want = floor.truth();
        assert!((t.rotation_deg - want.rotation_deg).abs() < 1e-6, "{t:?}");
        assert!((t.scale - gen_data::UNITS_M).abs() < 1e-12);
        for k in 0..2 {
            assert!(
                (t.translation[k] - want.translation[k]).abs() < 1e-4,
                "{t:?}"
            );
        }
        assert!(p.check_error.unwrap().max < 1e-3);
    }
    let rel = part("1F").relation.as_ref().unwrap();
    assert_eq!(rel.to, "2F");
    assert_eq!(
        rel.pairs.len(),
        gen_data::GRID_X.len() * gen_data::GRID_Y.len()
    );
    assert!(rel.runner_up * 2 < rel.pairs.len());
    assert!(rel.residual_max < 1.0);
}

fn texts(features: &[Value]) -> BTreeSet<(String, String)> {
    features
        .iter()
        .filter_map(|f| {
            let p = &f["properties"];
            Some((p["part"].as_str()?.into(), p["text"].as_str()?.into()))
        })
        .collect()
}

/// 出力の図形が同じか。図形の順も含めて比べる。ハンドルは書き出すたびに変わるので除き、
/// 線種の名前は大文字・小文字を区別しない（R12 は `CONTINUOUS`、acadrust は `Continuous` と書く）。
/// 数値は、R12 の DXF が μm・μ度で丸めて書く分の差を許す。
fn same_features(a: &[Value], b: &[Value]) -> Result<(), String> {
    if a.len() != b.len() {
        return Err(format!("{} vs {} features", a.len(), b.len()));
    }
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let mut x = x.clone();
        let mut y = y.clone();
        for f in [&mut x, &mut y] {
            let p = f["properties"].as_object_mut().unwrap();
            p.remove("handle");
            if let Some(Value::String(l)) = p.get_mut("linetype") {
                *l = l.to_lowercase();
            }
        }
        if !close(&x, &y) {
            return Err(format!("feature {i}: {x} vs {y}"));
        }
    }
    Ok(())
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 1e-6
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| close(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| close(v, w)))
        }
        _ => a == b,
    }
}

/// DXF（R12 cp932・R2010 UTF-8・記録と違う UTF-8 で BOM 付き）と DWG（R2018）のどれから読んでも、
/// 同じ変換になり、同じ図形が出る。
#[test]
fn every_format_places_both_floors_identically() {
    let out = out_dir("formats");
    let inputs = [
        "plan_r12.dxf",
        "plan_r2010.dxf",
        "plan_r2018.dwg",
        "plan_utf8_bom.dxf",
    ];
    let results: Vec<_> = inputs
        .iter()
        .map(|i| georef(i, &data("parts.json"), &out))
        .collect();
    for (rec, _) in &results {
        check_record(rec);
    }

    // 図枠は載せない。文字は文字コードによらず同じに読める
    let f12 = &results[0].1;
    let t = texts(f12);
    assert!(t.contains(&("1F".into(), "エントランスホール".into())));
    assert!(t.contains(&("2F".into(), "会議室Ａ".into())));
    assert!(t.iter().all(|(_, s)| !s.contains("合成テスト建物")));
    assert!(f12.iter().all(|f| f["properties"]["layer"] != "図枠"));
    for (input, (_, f)) in inputs.iter().zip(&results).skip(1) {
        if let Err(e) = same_features(f12, f) {
            panic!("{input} differs from plan_r12.dxf: {e}");
        }
    }

    std::fs::remove_dir_all(&out).unwrap();
}

/// 記録と違う文字コードを UTF-8 と判定したことを知らせる。
#[test]
fn mislabeled_code_page_is_reported() {
    let r = Command::new(env!("CARGO_BIN_EXE_cad2geojson"))
        .arg("parts")
        .arg(data("plan_utf8_bom.dxf"))
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains("declares code page ANSI_932 but its text is UTF-8"),
        "{stderr}"
    );
    assert!(stderr.contains("byte order mark"), "{stderr}");
}

/// `parts` の提案が、表題欄・2 階・1 階に分かれ、名前と `follow` を書き足すだけで位置合わせできる。
#[test]
fn suggested_parts_reproduce_the_georef() {
    suggest_and_georef("plan_r2010.dxf", &[]);
}

/// 本体から遠く離れた線があっても、同じ候補に分かれ（遠い線は載せない候補になる）、位置合わせできる。
#[test]
fn stray_line_does_not_change_the_suggestion() {
    let base = suggest_and_georef("plan_r2010.dxf", &[]);
    let with_stray = suggest_and_georef("plan_stray.dxf", &["stray1"]);
    assert_eq!(with_stray[..base.len()], base[..]);
    let stray = &with_stray[base.len()];
    assert_eq!(stray["place"], false);
    let b: Vec<f64> = stray["bbox"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert!(b[0] <= gen_data::STRAY_AT[0] && gen_data::STRAY_AT[1] <= b[3]);
}

/// 提案を名前と `follow` だけ書き換えて `georef` を回し、提案された候補を返す。
/// `extra` は表題欄・注記・2 つの階のあとに続く候補の名前。
fn suggest_and_georef(input: &str, extra: &[&str]) -> Vec<Value> {
    let out = out_dir(&format!("suggest-{}", input.replace('.', "_")));
    let suggested = out.join("parts.json");
    let r = Command::new(env!("CARGO_BIN_EXE_cad2geojson"))
        .arg("parts")
        .arg(data(input))
        .arg("-o")
        .arg(&suggested)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains("part2 and part1 share 24 circles"),
        "{stderr}"
    );

    let v: Value = serde_json::from_str(&std::fs::read_to_string(&suggested).unwrap()).unwrap();
    let parts = v["parts"].as_array().unwrap();
    let names: Vec<&str> = parts.iter().map(|p| p["name"].as_str().unwrap()).collect();
    // small1 は図枠の左上の注記（表題欄の外）
    let mut want = vec!["frame", "small1", "part1", "part2"];
    want.extend(extra);
    assert_eq!(names, want);
    assert_eq!(parts[0]["place"], false);
    assert_eq!(parts[1]["place"], false);
    // 表題欄・2 階・1 階の範囲が、それぞれの図形を囲む
    let bbox = |i: usize| -> Vec<f64> {
        parts[i]["bbox"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    };
    let within =
        |b: &[f64], p: [f64; 2]| b[0] <= p[0] && p[0] <= b[2] && b[1] <= p[1] && p[1] <= b[3];
    assert!(within(&bbox(0), [41000.0, 6500.0]));
    let corner = [gen_data::GRID_X[5], gen_data::GRID_Y[3]];
    for (i, floor) in [(2, Floor::F2), (3, Floor::F1)] {
        assert!(within(&bbox(i), floor.to_drawing([0.0, 0.0])));
        assert!(within(&bbox(i), floor.to_drawing(corner)));
    }

    let renamed = std::fs::read_to_string(&suggested)
        .unwrap()
        .replace(r#""part1""#, r#""2F""#)
        .replace(r#""part2""#, r#""1F","follow":"2F""#);
    std::fs::write(&suggested, renamed).unwrap();
    let (rec, features) = georef(input, &suggested, &out);
    check_record(&rec);
    let (_, want) = georef("plan_r2010.dxf", &data("parts.json"), &out);
    same_features(&want, &features).unwrap();

    std::fs::remove_dir_all(&out).unwrap();
    parts.clone()
}
