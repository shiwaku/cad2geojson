//! 複数の図面を、決めた回転・平行移動で 1 枚に並べる。
//!
//! 階ごとに別のファイルで原点がそろった図面（Schependomlaan など）から、1 枚に複数の階が並んだ図面を
//! 作り、部分分け（`parts`）と `follow` を実データで確かめるのに使う。最初の図面はそのまま置き、2 つ目
//! 以降は原点の周りに（反時計回りに）回してから平行移動する。答え（各図面の図面座標 → 並べた図面の
//! 図面座標）は `<出力>.truth.json` に書く。元の図面の座標で書いた対応点を渡すと、図面側の座標を
//! 並べた図面の座標に直した `<出力>.gcp.csv` も書く。
//!
//! ```sh
//! cargo run -p cad2geojson-stack-sheets -- -o sheet.dwg --gcp gcp.csv BG=bg.dwg V1=v1.dwg:0.2:50:0
//! ```
//!
//! 2 つ目以降の図面の図形は、最初の図面の文書に足す。
//!
//! - ブロック参照は、最初の図面に同じ名前・同じ中身のブロックがあればそのまま残し、無ければ展開する
//!   （acadrust に文書をまたいでブロックを写す API が無いため）。展開した円は楕円になる
//! - 寸法は匿名ブロック（`*D1` など）の名前が図面どうしでぶつかるので、ブロックの中身に展開する
//! - OLE 埋め込みは写さない（dwg2geo も読み飛ばす）
//! - 最初の図面に無いレイヤ・線種・文字スタイル・寸法スタイルは写す

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use acadrust::types::{Handle, Transform, Vector3};
use acadrust::{CadDocument, DwgReader, DwgWriter, DxfReader, DxfWriter, EntityType};
use anyhow::{Context, Result, bail};
use clap::Parser;

/// 複数の図面を、決めた回転・平行移動で 1 枚に並べる。
#[derive(Parser)]
struct Cli {
    /// 出力（.dwg、または .dxf）
    #[arg(short, long)]
    output: PathBuf,
    /// 元の図面の座標で書いた対応点の CSV（part は図面の名前）。図面側の座標を直して <出力>.gcp.csv に書く
    #[arg(long)]
    gcp: Option<PathBuf>,
    /// 並べる図面: [名前=]パス[:回転(度):dx:dy]。最初の図面はそのまま置く。名前の既定はファイル名
    #[arg(required = true)]
    drawings: Vec<String>,
}

/// 並べる図面と、その図面座標 → 並べた図面の図面座標の変換。
#[derive(Debug, PartialEq)]
struct Placement {
    name: String,
    path: PathBuf,
    rotation_deg: f64,
    translation: [f64; 2],
}

impl Placement {
    fn parse(s: &str) -> Result<Placement> {
        let (name, rest) = match s.split_once('=') {
            Some((n, r)) if !n.is_empty() && !n.contains(['/', '\\']) => (Some(n), r),
            _ => (None, s),
        };
        // パスに `:` が入る（C:\...）ので、後ろの 3 つがすべて数なら変換とみなす
        let fields: Vec<&str> = rest.rsplitn(4, ':').collect();
        let nums: Option<Vec<f64>> = (fields.len() == 4)
            .then(|| fields[..3].iter().map(|v| v.parse().ok()).collect())
            .flatten();
        let (path, rotation_deg, translation) = match nums {
            Some(n) => (fields[3], n[2], [n[1], n[0]]),
            None => (rest, 0.0, [0.0, 0.0]),
        };
        let path = PathBuf::from(path);
        let name = match name {
            Some(n) => n.to_string(),
            None => path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .with_context(|| format!("no file name in '{s}'"))?,
        };
        Ok(Placement {
            name,
            path,
            rotation_deg,
            translation,
        })
    }

    fn is_identity(&self) -> bool {
        self.rotation_deg == 0.0 && self.translation == [0.0, 0.0]
    }

    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        [
            c * p[0] - s * p[1] + self.translation[0],
            s * p[0] + c * p[1] + self.translation[1],
        ]
    }

    fn transform(&self) -> Transform {
        Transform::from_rotation(Vector3::new(0.0, 0.0, 1.0), self.rotation_deg.to_radians()).then(
            &Transform::from_translation(Vector3::new(
                self.translation[0],
                self.translation[1],
                0.0,
            )),
        )
    }
}

/// 写した図形の数と、写さなかった・展開した図形の数。
#[derive(Default)]
struct Stats {
    copied: usize,
    exploded: BTreeMap<String, usize>,
    skipped: BTreeMap<String, usize>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let placements = cli
        .drawings
        .iter()
        .map(|s| Placement::parse(s))
        .collect::<Result<Vec<_>>>()?;
    let (base, others) = placements.split_first().expect("required by clap");
    if !base.is_identity() {
        bail!("the first drawing is placed as is; give it no rotation or offset");
    }
    let mut doc = read(&base.path)?;
    for p in others {
        let src = read(&p.path)?;
        let stats = append(&mut doc, &src, &p.transform());
        eprintln!(
            "{}: copied {} entities, exploded {:?}, skipped {:?}",
            p.name, stats.copied, stats.exploded, stats.skipped
        );
    }
    let bytes = match cli.output.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("dxf") => DxfWriter::new(&doc).write_to_vec()?,
        _ => DwgWriter::write_to_vec(&doc)?,
    };
    std::fs::write(&cli.output, bytes)?;
    eprintln!("wrote {}", cli.output.display());
    let truth = sibling(&cli.output, "truth.json");
    std::fs::write(&truth, truth_json(&placements))?;
    eprintln!("wrote {}", truth.display());
    if let Some(gcp) = &cli.gcp {
        let text = std::fs::read_to_string(gcp)
            .with_context(|| format!("cannot read {}", gcp.display()))?;
        let out = sibling(&cli.output, "gcp.csv");
        std::fs::write(&out, place_gcp(&text, &placements)?)?;
        eprintln!("wrote {}", out.display());
    }
    Ok(())
}

fn sibling(out: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{suffix}", out.display()))
}

fn truth_json(placements: &[Placement]) -> String {
    let rows: Vec<String> = placements
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "file": p.path.file_name().map(|n| n.to_string_lossy()),
                "rotation_deg": p.rotation_deg,
                "translation": p.translation,
            })
            .to_string()
        })
        .collect();
    format!(
        "{{\n  \"note\": \"drawing coordinates are rotated counterclockwise about the origin, then translated\",\n  \"placements\": [\n    {}\n  ]\n}}\n",
        rows.join(",\n    ")
    )
}

/// 対応点の CSV の図面側の座標（cad_x, cad_y）を、part の名前の図面の変換で並べた図面の座標にする。
/// コメント（`#`）と空行はそのまま残す。
fn place_gcp(text: &str, placements: &[Placement]) -> Result<String> {
    let mut out = String::new();
    let mut lines = text.lines();
    let header = lines.next().context("empty GCP file")?;
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    let col = |name: &str| {
        cols.iter()
            .position(|c| *c == name)
            .with_context(|| format!("GCP file has no column '{name}'"))
    };
    let (ip, ix, iy) = (col("part")?, col("cad_x")?, col("cad_y")?);
    writeln!(out, "{header}")?;
    for (n, line) in lines.enumerate() {
        let lineno = n + 2;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            writeln!(out, "{line}")?;
            continue;
        }
        let mut f: Vec<String> = line.split(',').map(|v| v.trim().to_string()).collect();
        if f.len() < cols.len() {
            bail!("line {lineno}: expected {} fields", cols.len());
        }
        let p = placements
            .iter()
            .find(|p| p.name == f[ip])
            .with_context(|| format!("line {lineno}: no drawing named '{}'", f[ip]))?;
        let xy = |i: usize| -> Result<f64> {
            f[i].parse()
                .with_context(|| format!("line {lineno}: not a number '{}'", f[i]))
        };
        let q = p.apply([xy(ix)?, xy(iy)?]);
        f[ix] = format!("{:.6}", q[0]);
        f[iy] = format!("{:.6}", q[1]);
        writeln!(out, "{}", f.join(","))?;
    }
    Ok(out)
}

fn read(p: &Path) -> Result<CadDocument> {
    let is_dxf = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("dxf"));
    let doc = if is_dxf {
        DxfReader::from_file(p).and_then(|r| r.read())
    } else {
        DwgReader::from_file(p).and_then(|mut r| r.read())
    };
    doc.with_context(|| format!("cannot read {}", p.display()))
}

/// `src` のモデル空間の図形を `t` で動かして `doc` に足す。
fn append(doc: &mut CadDocument, src: &CadDocument, t: &Transform) -> Stats {
    copy_tables(doc, src);
    let mut stats = Stats::default();
    let mut queue: Vec<EntityType> = src.model_space_entities().cloned().collect();
    while let Some(e) = queue.pop() {
        match &e {
            EntityType::Insert(i) if !same_block(doc, src, &i.block_name) => {
                *stats
                    .exploded
                    .entry(format!("INSERT {}", i.block_name))
                    .or_default() += 1;
                queue.extend(i.explode_from_document(src));
            }
            EntityType::Dimension(d) => {
                let name = d.base().block_name.clone();
                let layer = e.common().layer.clone();
                *stats.exploded.entry("DIMENSION".into()).or_default() += 1;
                // 寸法のブロックの中身はワールド座標で描かれている
                queue.extend(src.entities_in_block(&name).cloned().map(|mut c| {
                    if c.common().layer == "0" {
                        c.common_mut().layer = layer.clone();
                    }
                    c
                }));
            }
            EntityType::Ole2Frame(_) => {
                *stats.skipped.entry("OLE2FRAME".into()).or_default() += 1;
            }
            _ => {
                let mut e = e;
                detach(&mut e);
                e.apply_transform(t);
                doc.add_entity(e).expect("entity accepted");
                stats.copied += 1;
            }
        }
    }
    stats
}

/// 元の文書のハンドルへの参照を外す。足した先の文書で振り直される。
fn detach(e: &mut EntityType) {
    let c = e.common_mut();
    c.handle = Handle::NULL;
    c.owner_handle = Handle::NULL;
    c.layer_handle = None;
    c.linetype_handle = None;
    c.reactors.clear();
    c.xdictionary_handle = None;
    c.extended_data = Default::default();
    // プロキシの表示用の図形は動かせないので捨てる
    c.graphic_data = None;
}

/// 両方の文書に同じ名前のブロックがあり、中身が同じか。
fn same_block(doc: &CadDocument, src: &CadDocument, name: &str) -> bool {
    let sig = |d: &CadDocument| -> Option<Vec<String>> {
        d.block_records.get(name)?;
        Some(
            d.entities_in_block(name)
                .cloned()
                .map(|mut e| {
                    detach(&mut e);
                    e.common_mut().raw_record = None;
                    format!("{e:?}")
                })
                .collect(),
        )
    };
    matches!((sig(doc), sig(src)), (Some(a), Some(b)) if a == b)
}

fn copy_tables(doc: &mut CadDocument, src: &CadDocument) {
    macro_rules! copy {
        ($table:ident) => {
            for entry in src.$table.iter() {
                if !doc.$table.contains(&entry.name) {
                    let mut entry = entry.clone();
                    entry.handle = Handle::NULL;
                    let _ = doc.$table.add(entry);
                }
            }
        };
    }
    copy!(layers);
    copy!(line_types);
    copy!(text_styles);
    copy!(dim_styles);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_placements() {
        let p = Placement::parse(r"V1=C:\data\v1.dwg:0.2:50:-3").unwrap();
        assert_eq!(p.name, "V1");
        assert_eq!(p.path, PathBuf::from(r"C:\data\v1.dwg"));
        assert_eq!((p.rotation_deg, p.translation), (0.2, [50.0, -3.0]));
        let p = Placement::parse(r"C:\data\SONSBEEK-BG Rio+MV+Hwa.dwg").unwrap();
        assert_eq!(p.name, "SONSBEEK-BG Rio+MV+Hwa");
        assert!(p.is_identity());
        // 数が 3 つそろわなければパスの一部とみなす
        let p = Placement::parse("a=b.dwg:1:2").unwrap();
        assert!(p.is_identity() && p.path == Path::new("b.dwg:1:2"));
    }

    #[test]
    fn gcp_follows_the_drawing_transform() {
        let ps = [
            Placement::parse("BG=bg.dwg").unwrap(),
            Placement::parse("V1=v1.dwg:90:50:0").unwrap(),
        ];
        let csv = "part,cad_x,cad_y,map_x,map_y,role\n# comment\nBG,1,2,10,20,control\nV1,1,2,10,20,check\n";
        assert_eq!(
            place_gcp(csv, &ps).unwrap(),
            "part,cad_x,cad_y,map_x,map_y,role\n# comment\nBG,1.000000,2.000000,10,20,control\nV1,48.000000,1.000000,10,20,check\n"
        );
        assert!(place_gcp("part,cad_x,cad_y\nX,1,2\n", &ps).is_err());
    }

    /// acadrust の変換と、対応点に使う変換が同じ向き。
    #[test]
    fn transforms_agree() {
        let p = Placement::parse("V1=v1.dwg:30:5:-7").unwrap();
        let a = p.apply([3.0, 4.0]);
        let b = p.transform().apply(Vector3::new(3.0, 4.0, 0.0));
        assert!((a[0] - b.x).abs() < 1e-12 && (a[1] - b.y).abs() < 1e-12);
    }
}
