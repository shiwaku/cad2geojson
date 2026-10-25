//! 部分ごとの位置合わせと、その記録（JSON）。
//!
//! 記録を入力に渡せば同じ変換を再現できる（対応点を解き直さない）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::crs::{Crs, Transformer};
use crate::fit::{self, ErrorStats, Model, Pair, Similarity};
use crate::part::Part;
use crate::relate::Relation;

pub const RECORD_VERSION: u32 = 1;

/// 対応点の役割。検証点は変換を解くのに使わず、誤差を測るだけに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Control,
    Check,
}

/// 対応点。`map` は `gcp_crs` の座標（経緯度なら x = 経度）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gcp {
    pub part: String,
    pub cad: [f64; 2],
    pub map: [f64; 2],
    #[serde(default)]
    pub role: Role,
}

/// 縮尺の扱い。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScaleMode {
    /// 単位から決めた縮尺（図面の 1 単位が何 m か）に固定する
    Fixed(f64),
    /// 相似変換で縮尺も解く
    Free,
}

/// 位置合わせの入力。
#[derive(Debug, Clone)]
pub struct Request {
    pub parts: Vec<Part>,
    pub gcps: Vec<Gcp>,
    pub gcp_crs: Crs,
    pub work_crs: Crs,
    pub scale: ScaleMode,
    /// `follow` を持つ部分の、相手の部分への関係（部分の名前ごと）
    pub relations: BTreeMap<String, Relation>,
}

/// 対応点 1 つの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointResult {
    pub cad: [f64; 2],
    pub map: [f64; 2],
    /// 作業用の投影座標系での地図側の座標
    pub map_work: [f64; 2],
    /// 作業用の投影座標系での誤差（地図側 − 変換した図面側、m）
    pub residual: [f64; 2],
}

/// 部分 1 つの位置合わせの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartGeoref {
    #[serde(flatten)]
    pub part: Part,
    /// 載せない部分と、ほかの部分に合わせた部分には無い
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// ほかの部分に合わせた部分の、相手の部分への関係。`transform` はこれと相手の変換を合成したもの
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<Relation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Similarity>,
    /// 縮尺を固定したとき、相似変換で解いた場合の縮尺（単位の誤りや対応点の偏りに気づくため）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control: Vec<PointResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check: Vec<PointResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_error: Option<ErrorStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_error: Option<ErrorStats>,
}

/// 位置合わせの記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub cad2geojson_georef: u32,
    /// 作業用の投影座標系（"EPSG:6673" など）
    pub work_crs: String,
    pub gcp_crs: String,
    pub parts: Vec<PartGeoref>,
}

impl Record {
    pub fn work_crs(&self) -> Result<Crs, Error> {
        self.work_crs.parse()
    }

    pub fn from_json(s: &str) -> Result<Record, Error> {
        let r: Record = serde_json::from_str(s).map_err(|e| Error::Record(e.to_string()))?;
        if r.cad2geojson_georef != RECORD_VERSION {
            return Err(Error::Record(format!(
                "unsupported record version {}",
                r.cad2geojson_georef
            )));
        }
        for p in &r.parts {
            if p.part.place && p.transform.is_none() {
                return Err(Error::Record(format!(
                    "part '{}' has no transform",
                    p.part.name
                )));
            }
        }
        r.work_crs()?;
        Ok(r)
    }
}

/// 部分ごとに対応点から変換を解く。`follow` を持つ部分は、相手の部分の変換に関係を合成して決める。
pub fn solve(req: &Request) -> Result<Record, Error> {
    if !req.work_crs.is_work_crs() {
        return Err(Error::NotWorkCrs(req.work_crs.to_string()));
    }
    let parts = if req.parts.is_empty() {
        vec![Part::whole()]
    } else {
        req.parts.clone()
    };
    for g in &req.gcps {
        if !parts.iter().any(|p| p.name == g.part) {
            return Err(Error::UnknownPart(g.part.clone()));
        }
    }
    for p in parts.iter().filter(|p| p.place) {
        let Some(to) = &p.follow else { continue };
        if !parts
            .iter()
            .any(|q| &q.name == to && q.place && q.name != p.name)
        {
            return Err(Error::Relation(format!(
                "part '{}' follows '{to}', which is not another placed part",
                p.name
            )));
        }
        if req
            .gcps
            .iter()
            .any(|g| g.part == p.name && g.role == Role::Control)
        {
            return Err(Error::Relation(format!(
                "part '{}' follows '{to}'; give its points with role check",
                p.name
            )));
        }
        if !req.relations.contains_key(&p.name) {
            return Err(Error::Relation(format!(
                "no relation from part '{}' to '{to}'",
                p.name
            )));
        }
    }
    let to_work = Transformer::new(req.gcp_crs, req.work_crs)?;

    let mut out: Vec<Option<PartGeoref>> = vec![None; parts.len()];
    // 相手が決まった部分から順に決める（合わせる先がさらに合わせていてもよい）
    while out.iter().any(Option::is_none) {
        let mut progressed = false;
        for (i, part) in parts.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            let base = match &part.follow {
                Some(to) if part.place => {
                    let j = parts.iter().position(|q| &q.name == to).unwrap();
                    match out[j].as_ref().and_then(|q| q.transform) {
                        Some(t) => Some(t),
                        None => continue,
                    }
                }
                _ => None,
            };
            out[i] = Some(solve_part(part, req, &to_work, base)?);
            progressed = true;
        }
        if !progressed {
            return Err(Error::Relation("parts follow each other in a cycle".into()));
        }
    }
    Ok(Record {
        cad2geojson_georef: RECORD_VERSION,
        work_crs: req.work_crs.to_string(),
        gcp_crs: req.gcp_crs.to_string(),
        parts: out.into_iter().map(Option::unwrap).collect(),
    })
}

/// 部分 1 つを解く。`base` は合わせる先の部分の変換（`follow` のとき）。
fn solve_part(
    part: &Part,
    req: &Request,
    to_work: &Transformer,
    base: Option<Similarity>,
) -> Result<PartGeoref, Error> {
    let mut r = PartGeoref {
        part: part.clone(),
        model: None,
        relation: None,
        transform: None,
        free_scale: None,
        control: vec![],
        check: vec![],
        control_error: None,
        check_error: None,
    };
    if !part.place {
        return Ok(r);
    }
    for g in req.gcps.iter().filter(|g| g.part == part.name) {
        let (x, y) = to_work.transform(g.map[0], g.map[1])?;
        let pr = PointResult {
            cad: g.cad,
            map: g.map,
            map_work: [x, y],
            residual: [0.0, 0.0],
        };
        match g.role {
            Role::Control => r.control.push(pr),
            Role::Check => r.check.push(pr),
        }
    }
    let t = match base {
        Some(base) => {
            let rel = req.relations[&part.name].clone();
            let t = rel.transform.then(&base);
            r.relation = Some(rel);
            t
        }
        None => {
            let pairs: Vec<Pair> = r
                .control
                .iter()
                .map(|p| Pair {
                    cad: p.cad,
                    map: p.map_work,
                })
                .collect();
            if pairs.len() < 2 {
                return Err(Error::PartTooFewPoints {
                    part: part.name.clone(),
                    got: pairs.len(),
                });
            }
            let model = match req.scale {
                ScaleMode::Fixed(s) => Model::Rigid { scale: s },
                ScaleMode::Free => Model::Similarity,
            };
            r.model = Some(model);
            if let ScaleMode::Fixed(_) = req.scale {
                r.free_scale = Some(fit::solve(&pairs, Model::Similarity)?.scale);
            }
            fit::solve(&pairs, model)?
        }
    };
    let fill = |pts: &mut Vec<PointResult>| {
        for p in pts.iter_mut() {
            let (x, y) = t.apply(p.cad[0], p.cad[1]);
            p.residual = [p.map_work[0] - x, p.map_work[1] - y];
        }
        ErrorStats::of(&pts.iter().map(|p| p.residual).collect::<Vec<_>>())
    };
    r.control_error = fill(&mut r.control);
    r.check_error = fill(&mut r.check);
    r.transform = Some(t);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 既知の変換で作った対応点から、その変換を求め直す。回転は大きく負（−123°）、図面は mm、
    /// 地図は平面直角座標系で原点から遠い（建物の全体平面図を航空写真に合わせるときと同じ形）。
    #[test]
    fn recovers_a_known_transform() {
        let truth = Similarity {
            scale: 0.001,
            rotation_deg: -123.11,
            translation: [-90739.918, -63702.478],
        };
        let gcp = |cad: [f64; 2], role| {
            let (x, y) = truth.apply(cad[0], cad[1]);
            Gcp {
                part: "2F".into(),
                cad,
                map: [x, y],
                role,
            }
        };
        let req = Request {
            parts: vec![Part {
                name: "2F".into(),
                bbox: None,
                layers: vec![],
                place: true,
                follow: None,
            }],
            gcps: vec![
                gcp([39255.061, -65843.088], Role::Control),
                gcp([117327.534, -2802.344], Role::Control),
                gcp([80000.0, -30000.0], Role::Check),
            ],
            gcp_crs: Crs::from_epsg(6673).unwrap(),
            work_crs: Crs::from_epsg(6673).unwrap(),
            scale: ScaleMode::Fixed(0.001),
            relations: BTreeMap::new(),
        };
        let r = solve(&req).unwrap();
        let t = r.parts[0].transform.unwrap();
        assert!(
            (t.rotation_deg - truth.rotation_deg).abs() < 1e-9,
            "{}",
            t.rotation_deg
        );
        for k in 0..2 {
            assert!(
                (t.translation[k] - truth.translation[k]).abs() < 1e-6,
                "{t:?}"
            );
        }
        assert!(r.parts[0].control_error.unwrap().max < 1e-6);
        assert!(r.parts[0].check_error.unwrap().max < 1e-6);
        let fs = r.parts[0].free_scale.unwrap();
        assert!((fs - truth.scale).abs() < 1e-12, "{fs}");

        let back = Record::from_json(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn geographic_gcps_are_solved_in_work_crs() {
        // 図面の 1 単位 = 1 mm、回転 0、平面直角座標系 5 系の (-90000, -63000) を原点に置く図面
        let work = Crs::from_epsg(6673).unwrap();
        let back = Transformer::new(work, Crs::WGS84).unwrap();
        let cad = [
            [0.0, 0.0],
            [200_000.0, 0.0],
            [0.0, 150_000.0],
            [180_000.0, 120_000.0],
        ];
        let gcps: Vec<Gcp> = cad
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (lon, lat) = back
                    .transform(-90000.0 + c[0] / 1000.0, -63000.0 + c[1] / 1000.0)
                    .unwrap();
                Gcp {
                    part: "drawing".into(),
                    cad: *c,
                    map: [lon, lat],
                    role: if i == 3 { Role::Check } else { Role::Control },
                }
            })
            .collect();
        let req = Request {
            parts: vec![],
            gcps,
            gcp_crs: Crs::WGS84,
            work_crs: work,
            scale: ScaleMode::Fixed(0.001),
            relations: BTreeMap::new(),
        };
        let r = solve(&req).unwrap();
        let p = &r.parts[0];
        let t = p.transform.unwrap();
        assert!(t.rotation_deg.abs() < 1e-7);
        assert!(
            (t.translation[0] + 90000.0).abs() < 1e-4 && (t.translation[1] + 63000.0).abs() < 1e-4
        );
        assert!(p.check_error.unwrap().max < 1e-4);
    }

    #[test]
    fn rejects_bad_requests() {
        let base = Request {
            parts: vec![],
            gcps: vec![],
            gcp_crs: Crs::WGS84,
            work_crs: Crs::WGS84,
            scale: ScaleMode::Free,
            relations: BTreeMap::new(),
        };
        assert!(matches!(solve(&base), Err(Error::NotWorkCrs(_))));
        let req = Request {
            work_crs: Crs::from_epsg(6673).unwrap(),
            ..base.clone()
        };
        assert!(matches!(solve(&req), Err(Error::PartTooFewPoints { .. })));
        let req = Request {
            work_crs: Crs::from_epsg(6673).unwrap(),
            gcps: vec![Gcp {
                part: "1F".into(),
                cad: [0.0, 0.0],
                map: [0.0, 0.0],
                role: Role::Control,
            }],
            ..base
        };
        assert!(matches!(solve(&req), Err(Error::UnknownPart(_))));
    }

    #[test]
    fn follower_is_composed_with_its_target() {
        let c = Crs::from_epsg(6673).unwrap();
        // 2F: 図面 (0,0)→(-90000,-63000)、回転 90°
        let gcps = vec![
            Gcp {
                part: "2F".into(),
                cad: [0.0, 0.0],
                map: [-90000.0, -63000.0],
                role: Role::Control,
            },
            Gcp {
                part: "2F".into(),
                cad: [1000.0, 0.0],
                map: [-90000.0, -62999.0],
                role: Role::Control,
            },
            // 1F の (0, 100000) は 2F の (0, 0) と同じ場所 → 地図の (-90000, -63000)
            Gcp {
                part: "1F".into(),
                cad: [0.0, 100000.0],
                map: [-90000.0, -63000.0],
                role: Role::Check,
            },
        ];
        let rel = Relation {
            to: "2F".into(),
            transform: Similarity {
                scale: 1.0,
                rotation_deg: 0.0,
                translation: [0.0, -100000.0],
            },
            residual_median: 0.0,
            residual_max: 0.0,
            runner_up: 0,
            pairs: vec![],
        };
        let parts = vec![
            Part {
                name: "1F".into(),
                bbox: Some([-1e9, 50000.0, 1e9, 1e9]),
                follow: Some("2F".into()),
                ..Part::whole()
            },
            Part {
                name: "2F".into(),
                ..Part::whole()
            },
        ];
        let req = Request {
            parts,
            gcps,
            gcp_crs: c,
            work_crs: c,
            scale: ScaleMode::Fixed(0.001),
            relations: BTreeMap::from([("1F".to_string(), rel)]),
        };
        let r = solve(&req).unwrap();
        let f1 = &r.parts[0];
        assert!(f1.model.is_none() && f1.relation.is_some());
        let t = f1.transform.unwrap();
        assert!((t.rotation_deg - 90.0).abs() < 1e-9);
        assert!(f1.check_error.unwrap().max < 1e-6);
        let back = Record::from_json(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);

        // 合わせる部分に対応点（control）は与えられない
        let mut bad = req.clone();
        bad.gcps[2].role = Role::Control;
        assert!(matches!(solve(&bad), Err(Error::Relation(_))));
        // 関係が無い
        let mut bad = req.clone();
        bad.relations.clear();
        assert!(matches!(solve(&bad), Err(Error::Relation(_))));
        // 互いに合わせている
        let mut bad = req;
        bad.parts[1].follow = Some("1F".into());
        bad.gcps.clear();
        bad.relations
            .insert("2F".into(), bad.relations["1F"].clone());
        assert!(matches!(solve(&bad), Err(Error::Relation(_))));
    }
}
