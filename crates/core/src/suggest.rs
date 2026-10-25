//! 部分の候補を図面から提案する。どれを使うか、名前と `follow` は利用者が決める。
//!
//! 1. 長い水平・垂直の線分でできた長方形を探す
//! 2. ほぼすべての図形を囲む長方形は用紙の枠とみなし、その線は候補に入れない
//! 3. 用紙（枠が無ければ図面全体の範囲）の隅にある長方形は表題欄とみなし、載せない候補にする
//! 4. 残りの図形を、図形の無い帯（すき間）で再帰的に分ける（XY-cut）
//!
//! その前に、本体から遠く離れた少数の図形（原点に残った点、壊れた座標など）を外す。図面の範囲を
//! 膨らませ、すき間の既定の幅と長方形の辺の長さの基準を狂わせるため。外した図形は消さず、
//! 載せない候補にする。

use serde_json::Value;

use crate::Error;
use crate::geojson::{bbox, xy};
use crate::part::Part;

/// 提案の指定。
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// 分ける境目とみなす、図形の無い帯の最小の幅（図面の単位）。`None` なら図面の範囲の長辺の 3%
    pub min_gap: Option<f64>,
}

/// 既定の `min_gap`（図面の範囲の長辺に対する比）。
const MIN_GAP_RATIO: f64 = 0.03;
/// 長方形の辺とみなす線分の最小の長さ（図面の範囲の長辺に対する比）。
const MIN_SIDE_RATIO: f64 = 0.02;
/// 端点が一致するとみなす差（図面の範囲の長辺に対する比）。
const TOL_RATIO: f64 = 1e-3;
/// 用紙の枠とみなす、囲む図形の割合。
const BORDER_SHARE: f64 = 0.95;
/// 載せる候補にする最少の図形の割合。これより少ない塊は載せない候補にする（はみ出した注記など）。
const MIN_SHARE: f64 = 0.01;
/// 本体から遠い図形とみなす距離（図形の中心の四分位の幅に対する比）。
const STRAY_FACTOR: f64 = 3.0;
/// 遠い図形として外す最大の割合。これを超えて遠くにあるなら、そちらも図面の一部とみなす（図面が並ぶ配置など）。
const STRAY_MAX_SHARE: f64 = 0.01;
/// 四分位の統計を使う最少の図形の数。
const STRAY_MIN_FEATURES: usize = 20;

/// 部分の候補。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub part: Part,
    /// 候補に入る図形の数
    pub features: usize,
    /// 候補の中の文字（文字高の大きい順）。部分の名前を決める手がかり
    pub texts: Vec<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 用紙の隅の長方形（表題欄など）
    TitleBlock,
    /// すき間で分けた塊
    Cluster,
    /// 図形の少ない塊
    Small,
    /// 本体から遠く離れた図形
    Stray,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub candidates: Vec<Candidate>,
    /// 用紙の枠とみなした長方形（候補に入れない）
    pub borders: Vec<[f64; 4]>,
    pub min_gap: f64,
}

struct Feature {
    bbox: [f64; 4],
    text: Option<(f64, String)>,
}

pub fn suggest(input: &Value, opts: &Options) -> Result<Suggestion, Error> {
    let features = input
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::GeoJson("input is not a FeatureCollection".into()))?;
    let mut fs = vec![];
    let mut geoms = vec![];
    for f in features {
        let Some(geom) = f.get("geometry").filter(|g| !g.is_null()) else {
            continue;
        };
        let Some(b) = bbox(geom) else { continue };
        let props = f.get("properties");
        let text = props
            .and_then(|p| p.get("text"))
            .and_then(Value::as_str)
            .map(|t| {
                let h = props
                    .and_then(|p| p.get("text_height"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                (h, t.to_string())
            });
        let curve = props
            .and_then(|p| p.get("entity_type"))
            .and_then(Value::as_str)
            .is_some_and(|t| matches!(t, "CIRCLE" | "ARC" | "ELLIPSE" | "SPLINE"));
        geoms.push((!curve).then_some(geom));
        fs.push(Feature { bbox: b, text });
    }
    if fs.is_empty() {
        return Err(Error::GeoJson("drawing has no features".into()));
    }
    let centers: Vec<[f64; 2]> = fs.iter().map(|f| center(f.bbox)).collect();
    let strays = strays(&centers);
    let body: Vec<usize> = (0..fs.len()).filter(|i| !strays.contains(i)).collect();
    let mut segments = vec![];
    for &i in &body {
        if let Some(geom) = geoms[i] {
            push_segments(geom, &mut segments)?;
        }
    }
    let extent = body
        .iter()
        .map(|&i| fs[i].bbox)
        .reduce(union)
        .expect("strays are a minority");
    let size = (extent[2] - extent[0]).max(extent[3] - extent[1]).max(1e-9);
    let tol = size * TOL_RATIO;
    let min_gap = opts.min_gap.unwrap_or(size * MIN_GAP_RATIO);

    let rects = rectangles(&segments, size * MIN_SIDE_RATIO, tol);
    let share = |r: &[f64; 4]| {
        body.iter().filter(|&&i| inside(r, centers[i], tol)).count() as f64 / body.len() as f64
    };
    let (borders, others): (Vec<[f64; 4]>, Vec<[f64; 4]>) =
        rects.into_iter().partition(|r| share(r) >= BORDER_SHARE);
    // 用紙の内側。枠が二重なら内側の枠
    let sheet = borders
        .iter()
        .copied()
        .min_by(|a, b| area(a).total_cmp(&area(b)))
        .unwrap_or(extent);
    let mut titles: Vec<[f64; 4]> = others
        .into_iter()
        .filter(|r| in_corner(r, &sheet, tol) && area(r) < 0.5 * area(&sheet))
        .collect();
    // 隅の長方形の中を区切った長方形は、外側の長方形にまとめる
    let all = titles.clone();
    titles.retain(|r| !all.iter().any(|o| o != r && contains(o, r, tol)));

    let on_border = |b: &[f64; 4]| borders.iter().any(|r| on_edge(b, r, tol));
    let mut rest = vec![];
    let mut title_members = vec![vec![]; titles.len()];
    for &i in &body {
        let f = &fs[i];
        if let Some(t) = titles.iter().position(|r| inside(r, centers[i], tol)) {
            title_members[t].push(i);
        } else if !on_border(&f.bbox) {
            rest.push(i);
        }
    }

    let mut candidates = vec![];
    for (k, (r, members)) in titles.iter().zip(&title_members).enumerate() {
        let name = if k == 0 {
            "frame".to_string()
        } else {
            format!("frame{}", k + 1)
        };
        candidates.push(candidate(
            name,
            pad(*r, tol),
            members,
            &fs,
            false,
            Kind::TitleBlock,
        ));
    }
    let mut clusters = xy_cut(rest, &fs, min_gap);
    // 上から下、左から右の順
    clusters.sort_by(|a, b| {
        let (ba, bb) = (members_bbox(a, &fs), members_bbox(b, &fs));
        bb[3].total_cmp(&ba[3]).then(ba[0].total_cmp(&bb[0]))
    });
    let (mut n_part, mut n_small) = (0, 0);
    for m in &clusters {
        let small = (m.len() as f64) < MIN_SHARE * fs.len() as f64;
        let name = if small {
            n_small += 1;
            format!("small{n_small}")
        } else {
            n_part += 1;
            format!("part{n_part}")
        };
        // すき間の半分未満だけ広げる。隣の塊の範囲とは重ならない
        let b = pad(members_bbox(m, &fs), min_gap / 4.0);
        let kind = if small { Kind::Small } else { Kind::Cluster };
        candidates.push(candidate(name, b, m, &fs, !small, kind));
    }
    for (k, &i) in strays.iter().enumerate() {
        let b = pad(fs[i].bbox, tol);
        candidates.push(candidate(
            format!("stray{}", k + 1),
            b,
            &[i],
            &fs,
            false,
            Kind::Stray,
        ));
    }
    Ok(Suggestion {
        candidates,
        borders,
        min_gap,
    })
}

/// 本体から遠く離れた図形。中心の中央値から、四分位の幅の [`STRAY_FACTOR`] 倍より遠いもの。
/// 平均や最大を使うと遠い図形そのものに引きずられるので、中央値と四分位を使う。
fn strays(centers: &[[f64; 2]]) -> Vec<usize> {
    if centers.len() < STRAY_MIN_FEATURES {
        return vec![];
    }
    let quartiles = |axis: usize| {
        let mut v: Vec<f64> = centers.iter().map(|c| c[axis]).collect();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        (v[n / 4], v[n / 2], v[3 * n / 4])
    };
    let (x1, mx, x3) = quartiles(0);
    let (y1, my, y3) = quartiles(1);
    let limit = (x3 - x1).max(y3 - y1).max(1e-9) * STRAY_FACTOR;
    let far: Vec<usize> = (0..centers.len())
        .filter(|&i| (centers[i][0] - mx).abs() > limit || (centers[i][1] - my).abs() > limit)
        .collect();
    let max = ((centers.len() as f64 * STRAY_MAX_SHARE) as usize).max(1);
    if far.len() <= max { far } else { vec![] }
}

fn candidate(
    name: String,
    bbox: [f64; 4],
    members: &[usize],
    fs: &[Feature],
    place: bool,
    kind: Kind,
) -> Candidate {
    let mut texts: Vec<&(f64, String)> = members
        .iter()
        .filter_map(|&i| fs[i].text.as_ref())
        .collect();
    texts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = vec![];
    for (_, t) in texts {
        if !seen.contains(t) {
            seen.push(t.clone());
        }
    }
    Candidate {
        part: Part {
            name,
            bbox: Some(bbox.map(round)),
            layers: vec![],
            place,
            follow: None,
        },
        features: members.len(),
        texts: seen,
        kind,
    }
}

/// 折れ線と多角形の外周の、隣り合う頂点の組。
fn push_segments(geom: &Value, out: &mut Vec<[[f64; 2]; 2]>) -> Result<(), Error> {
    let coords = geom.get("coordinates");
    let lines: Vec<&Value> = match geom.get("type").and_then(Value::as_str) {
        Some("LineString") => coords.into_iter().collect(),
        Some("MultiLineString") | Some("Polygon") => coords
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => vec![],
    };
    for l in lines {
        let pts = l
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| xy(c.as_array().map(Vec::as_slice).unwrap_or(&[])).map(|(x, y)| [x, y]))
            .collect::<Result<Vec<_>, _>>()?;
        out.extend(pts.windows(2).map(|w| [w[0], w[1]]));
    }
    Ok(())
}

/// 水平の線分 (y, x0, x1) と垂直の線分 (x, y0, y1) から、4 辺がそろう長方形を探す。
fn rectangles(segments: &[[[f64; 2]; 2]], min_side: f64, tol: f64) -> Vec<[f64; 4]> {
    let mut hs = vec![];
    let mut vs = vec![];
    for [a, b] in segments {
        let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        if dy <= tol && dx >= min_side {
            hs.push(((a[1] + b[1]) / 2.0, a[0].min(b[0]), a[0].max(b[0])));
        } else if dx <= tol && dy >= min_side {
            vs.push(((a[0] + b[0]) / 2.0, a[1].min(b[1]), a[1].max(b[1])));
        }
    }
    let has_v = |x: f64, y0: f64, y1: f64| {
        vs.iter()
            .any(|v| (v.0 - x).abs() <= tol && v.1 <= y0 + tol && v.2 >= y1 - tol)
    };
    let mut out: Vec<[f64; 4]> = vec![];
    for (i, h1) in hs.iter().enumerate() {
        for h2 in &hs[i + 1..] {
            let (lo, hi) = if h1.0 < h2.0 { (h1, h2) } else { (h2, h1) };
            if hi.0 - lo.0 < min_side || (lo.1 - hi.1).abs() > tol || (lo.2 - hi.2).abs() > tol {
                continue;
            }
            let r = [lo.1.min(hi.1), lo.0, lo.2.max(hi.2), hi.0];
            if has_v(r[0], r[1], r[3])
                && has_v(r[2], r[1], r[3])
                && !out.iter().any(|o| same(o, &r, tol))
            {
                out.push(r);
            }
        }
    }
    out
}

/// 図形の外接矩形を、図形の無い帯で再帰的に分ける。先に y で、次に x で分ける。
fn xy_cut(items: Vec<usize>, fs: &[Feature], min_gap: f64) -> Vec<Vec<usize>> {
    if items.is_empty() {
        return vec![];
    }
    for axis in [1, 0] {
        let groups = split(&items, fs, axis, min_gap);
        if groups.len() > 1 {
            return groups
                .into_iter()
                .flat_map(|g| xy_cut(g, fs, min_gap))
                .collect();
        }
    }
    vec![items]
}

fn split(items: &[usize], fs: &[Feature], axis: usize, min_gap: f64) -> Vec<Vec<usize>> {
    let mut sorted = items.to_vec();
    sorted.sort_by(|&a, &b| fs[a].bbox[axis].total_cmp(&fs[b].bbox[axis]));
    let mut groups: Vec<Vec<usize>> = vec![];
    let mut reach = f64::NEG_INFINITY;
    for i in sorted {
        let b = fs[i].bbox;
        if groups.is_empty() || b[axis] - reach >= min_gap {
            groups.push(vec![]);
        }
        groups.last_mut().unwrap().push(i);
        reach = reach.max(b[axis + 2]);
    }
    groups
}

fn members_bbox(m: &[usize], fs: &[Feature]) -> [f64; 4] {
    m.iter()
        .map(|&i| fs[i].bbox)
        .reduce(union)
        .unwrap_or([0.0; 4])
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn center(b: [f64; 4]) -> [f64; 2] {
    [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
}

fn area(r: &[f64; 4]) -> f64 {
    (r[2] - r[0]) * (r[3] - r[1])
}

fn pad(r: [f64; 4], d: f64) -> [f64; 4] {
    [r[0] - d, r[1] - d, r[2] + d, r[3] + d]
}

fn inside(r: &[f64; 4], p: [f64; 2], tol: f64) -> bool {
    r[0] - tol <= p[0] && p[0] <= r[2] + tol && r[1] - tol <= p[1] && p[1] <= r[3] + tol
}

fn contains(outer: &[f64; 4], inner: &[f64; 4], tol: f64) -> bool {
    inside(outer, [inner[0], inner[1]], tol) && inside(outer, [inner[2], inner[3]], tol)
}

fn same(a: &[f64; 4], b: &[f64; 4], tol: f64) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

/// 隣り合う 2 辺が用紙の辺に重なるか。
fn in_corner(r: &[f64; 4], sheet: &[f64; 4], tol: f64) -> bool {
    let left = (r[0] - sheet[0]).abs() <= tol;
    let bottom = (r[1] - sheet[1]).abs() <= tol;
    let right = (r[2] - sheet[2]).abs() <= tol;
    let top = (r[3] - sheet[3]).abs() <= tol;
    (left || right) && (bottom || top)
}

/// 外接矩形が長方形のどれかの辺の上にあるか（枠の線そのもの）。
fn on_edge(b: &[f64; 4], r: &[f64; 4], tol: f64) -> bool {
    let horizontal = |y: f64| {
        (b[1] - y).abs() <= tol
            && (b[3] - y).abs() <= tol
            && b[0] >= r[0] - tol
            && b[2] <= r[2] + tol
    };
    let vertical = |x: f64| {
        (b[0] - x).abs() <= tol
            && (b[2] - x).abs() <= tol
            && b[1] >= r[1] - tol
            && b[3] <= r[3] + tol
    };
    horizontal(r[1]) || horizontal(r[3]) || vertical(r[0]) || vertical(r[2])
}

/// 部分の範囲は図面の単位の整数に丸める（読みやすさのため。広げた分より十分小さい）。
fn round(v: f64) -> f64 {
    v.round()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(a: [f64; 2], b: [f64; 2]) -> Value {
        json!({"type": "Feature", "properties": {"entity_type": "LINE"},
               "geometry": {"type": "LineString", "coordinates": [a, b]}})
    }

    fn rect(out: &mut Vec<Value>, a: [f64; 2], b: [f64; 2]) {
        out.push(line([a[0], a[1]], [b[0], a[1]]));
        out.push(line([b[0], a[1]], [b[0], b[1]]));
        out.push(line([b[0], b[1]], [a[0], b[1]]));
        out.push(line([a[0], b[1]], [a[0], a[1]]));
    }

    fn text(at: [f64; 2], h: f64, t: &str) -> Value {
        json!({"type": "Feature", "properties": {"entity_type": "TEXT", "text": t, "text_height": h},
               "geometry": {"type": "Point", "coordinates": at}})
    }

    /// 2 つの塊（下と上）を用紙の枠で囲み、右下の隅に表題欄を置く。
    fn sheet() -> Value {
        let mut fs = vec![];
        rect(&mut fs, [0.0, 0.0], [1000.0, 1400.0]);
        rect(&mut fs, [700.0, 0.0], [1000.0, 100.0]);
        fs.push(line([700.0, 50.0], [1000.0, 50.0]));
        fs.push(text([710.0, 60.0], 20.0, "表題"));
        for (y0, label) in [(200.0, "1F"), (800.0, "2F")] {
            for k in 0..60 {
                let y = y0 + (k % 10) as f64 * 40.0;
                let x = 100.0 + (k / 10) as f64 * 80.0;
                fs.push(line([x, y], [x + 70.0, y]));
            }
            fs.push(text([100.0, y0 - 30.0], 15.0, label));
            fs.push(text([150.0, y0 + 10.0], 5.0, "室"));
        }
        // 2F の右に離れた小さな注記
        fs.push(text([900.0, 1300.0], 5.0, "注"));
        json!({"type": "FeatureCollection", "features": fs})
    }

    #[test]
    fn finds_title_block_and_clusters() {
        let s = suggest(&sheet(), &Options::default()).unwrap();
        assert_eq!(s.borders, vec![[0.0, 0.0, 1000.0, 1400.0]]);
        let names: Vec<&str> = s.candidates.iter().map(|c| c.part.name.as_str()).collect();
        assert_eq!(names, ["frame", "small1", "part1", "part2"]);
        let c = &s.candidates;
        assert_eq!(c[0].kind, Kind::TitleBlock);
        assert!(!c[0].part.place);
        assert_eq!(c[0].texts, ["表題"]);
        assert_eq!(c[2].texts, ["2F", "室"]);
        assert_eq!(c[3].texts, ["1F", "室"]);
        assert!(c[2].part.place && c[3].part.place && !c[1].part.place);
        // 用紙の枠の線はどの候補にも入らない
        let n: usize = c.iter().map(|c| c.features).sum();
        assert_eq!(n, 6 + 2 * 62 + 1);
    }

    #[test]
    fn min_gap_controls_splitting() {
        let opts = Options {
            min_gap: Some(500.0),
        };
        let s = suggest(&sheet(), &opts).unwrap();
        let parts: Vec<&str> = s
            .candidates
            .iter()
            .filter(|c| c.kind == Kind::Cluster)
            .map(|c| c.part.name.as_str())
            .collect();
        assert_eq!(parts, ["part1"]);
    }

    /// 遠く離れた図形があっても、範囲とすき間の幅が膨らまず、同じ候補になる。
    #[test]
    fn stray_features_do_not_inflate_the_extent() {
        let mut v = sheet();
        let fs = v["features"].as_array_mut().unwrap();
        // 図形は 137 なので、外せるのは 1 つ（1%）
        fs.push(line([1e9, 1e9], [1e9 + 1.0, 1e9]));
        let s = suggest(&v, &Options::default()).unwrap();
        let base = suggest(&sheet(), &Options::default()).unwrap();
        assert_eq!(s.min_gap, base.min_gap);
        assert_eq!(s.borders, base.borders);
        let names: Vec<&str> = s.candidates.iter().map(|c| c.part.name.as_str()).collect();
        assert_eq!(names, ["frame", "small1", "part1", "part2", "stray1"]);
        assert_eq!(s.candidates[..4], base.candidates[..]);
        assert!(
            s.candidates[4..]
                .iter()
                .all(|c| c.kind == Kind::Stray && !c.part.place)
        );
    }

    /// 遠くにある図形が多ければ、図面の一部とみなして外さない。
    #[test]
    fn many_far_features_are_kept() {
        let mut v = sheet();
        let fs = v["features"].as_array_mut().unwrap();
        for k in 0..10 {
            fs.push(line(
                [1e6 + k as f64 * 10.0, 0.0],
                [1e6 + k as f64 * 10.0, 5.0],
            ));
        }
        let s = suggest(&v, &Options::default()).unwrap();
        assert!(s.candidates.iter().all(|c| c.kind != Kind::Stray));
    }

    #[test]
    fn title_block_without_border_in_extent_corner() {
        let mut fs = vec![];
        rect(&mut fs, [0.0, 900.0], [300.0, 1000.0]);
        fs.push(text([10.0, 950.0], 20.0, "表題"));
        for k in 0..20 {
            let x = 400.0 + k as f64 * 20.0;
            fs.push(line([x, 0.0], [x, 980.0]));
        }
        let s = suggest(
            &json!({"type": "FeatureCollection", "features": fs}),
            &Options::default(),
        )
        .unwrap();
        assert!(s.borders.is_empty());
        assert_eq!(s.candidates[0].part.name, "frame");
        assert_eq!(s.candidates[0].features, 5);
        assert_eq!(s.candidates.len(), 2);
    }
}
