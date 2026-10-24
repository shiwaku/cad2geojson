use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cad2geojson_core::crs::Crs;
use cad2geojson_core::geojson::{self, Summary};
use cad2geojson_core::georef::{self, Gcp, Record, Request, Role, ScaleMode};
use cad2geojson_core::part::Part;
use cad2geojson_core::relate;
use cad2geojson_core::suggest::{self, Kind};
use clap::{Args, Parser, Subcommand};
use serde_json::Value;

/// CAD 図面（DWG / DXF、または図面座標の GeoJSON）を、地図に正しい位置で載る GeoJSON（WGS84）にする。
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 対応点から部分ごとの変換を解き、GeoJSON に適用する
    Georef(GeorefArgs),
    /// 位置合わせの記録（JSON）をそのまま適用する
    Apply(ApplyArgs),
    /// DWG / DXF を図面座標の GeoJSON にする（対応点の図面側の座標を拾うため）
    Convert(ConvertArgs),
    /// 部分の候補（parts.json）を提案する。図形の無い帯で分け、用紙の隅の表題欄を載せない部分にする
    Parts(PartsArgs),
}

/// 入力の図面。
#[derive(Args)]
struct Input {
    /// DWG / DXF（ASCII）、または図面座標の GeoJSON（dwg2geo の出力など）
    input: PathBuf,
    /// DXF の文字コード（DXF の表記。例: ANSI_932）。既定は、記録が無い R12 などで中身から判定する
    #[arg(long)]
    dxf_encoding: Option<String>,
}

#[derive(Args)]
struct GeorefArgs {
    #[command(flatten)]
    input: Input,
    /// 対応点の CSV: part,cad_x,cad_y,map_x,map_y[,role]（role = control | check）
    #[arg(long)]
    gcp: PathBuf,
    /// 対応点の地図側の CRS（既定は --work-crs）
    #[arg(long)]
    gcp_crs: Option<Crs>,
    /// 変換を解く投影座標系（平面直角座標系。例: EPSG:6673）
    #[arg(long)]
    work_crs: Crs,
    /// 部分の定義（JSON）。無ければ図面全体を 1 つの部分 "drawing" とする
    #[arg(long)]
    parts: Option<PathBuf>,
    /// 図面の単位（mm / cm / m、または 1 単位が何 m か）。縮尺をこれに固定する
    #[arg(long, conflicts_with = "free_scale")]
    units: Option<String>,
    /// 縮尺も解く（相似変換）。単位が分からないときに使う
    #[arg(long)]
    free_scale: bool,
    /// 出力（.geojson、または 1 行 1 図形の .geojsonl / .ndjson）
    #[arg(short, long)]
    output: PathBuf,
    /// 位置合わせの記録の出力先（既定は <output>.georef.json）
    #[arg(long)]
    record: Option<PathBuf>,
}

#[derive(Args)]
struct ApplyArgs {
    #[command(flatten)]
    input: Input,
    /// 位置合わせの記録
    #[arg(long)]
    record: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Args)]
struct ConvertArgs {
    #[command(flatten)]
    input: Input,
    /// 出力（図面座標の GeoJSON）
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Args)]
struct PartsArgs {
    #[command(flatten)]
    input: Input,
    /// 分ける境目とみなす、図形の無い帯の最小の幅（図面の単位）。既定は図面の範囲の長辺の 3%
    #[arg(long)]
    min_gap: Option<f64>,
    /// 出力（parts.json）。無ければ標準出力
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Georef(a) => run_georef(a),
        Command::Apply(a) => run_apply(a),
        Command::Convert(a) => run_convert(a),
        Command::Parts(a) => run_parts(a),
    }
}

fn run_georef(a: GeorefArgs) -> Result<()> {
    let scale = match (&a.units, a.free_scale) {
        (Some(u), _) => ScaleMode::Fixed(parse_units(u)?),
        (None, true) => ScaleMode::Free,
        (None, false) => {
            bail!("specify --units (e.g. mm) to fix the scale, or --free-scale to solve it")
        }
    };
    let parts: Vec<Part> = match &a.parts {
        Some(p) => {
            #[derive(serde::Deserialize)]
            struct PartsFile {
                parts: Vec<Part>,
            }
            let s = read(p)?;
            serde_json::from_str::<PartsFile>(&s)
                .with_context(|| format!("invalid parts file {}", p.display()))?
                .parts
        }
        None => vec![],
    };
    let gj = load_geojson(&a.input)?;
    let mut relations = BTreeMap::new();
    if parts.iter().any(|p| p.place && p.follow.is_some()) {
        let circles = relate::circles_by_part(&gj, &parts)?;
        for (i, p) in parts.iter().enumerate() {
            let Some(to) = p.follow.as_ref().filter(|_| p.place) else {
                continue;
            };
            let Some(j) = parts.iter().position(|q| &q.name == to) else {
                bail!("part '{}' follows unknown part '{to}'", p.name);
            };
            let rel = relate::relate(&circles[i], &circles[j], to)
                .with_context(|| format!("cannot place part '{}' by '{to}'", p.name))?;
            relations.insert(p.name.clone(), rel);
        }
    }
    let req = Request {
        parts,
        gcps: read_gcps(&a.gcp)?,
        gcp_crs: a.gcp_crs.unwrap_or(a.work_crs),
        work_crs: a.work_crs,
        scale,
        relations,
    };
    let record = georef::solve(&req)?;
    print_record(&record);

    let record_path = a
        .record
        .unwrap_or_else(|| with_suffix(&a.output, ".georef.json"));
    fs::write(&record_path, serde_json::to_string_pretty(&record)? + "\n")
        .with_context(|| format!("cannot write {}", record_path.display()))?;
    eprintln!("record: {}", record_path.display());
    convert(&gj, &record, &a.output)
}

fn run_apply(a: ApplyArgs) -> Result<()> {
    let record = Record::from_json(&read(&a.record)?)?;
    convert(&load_geojson(&a.input)?, &record, &a.output)
}

fn run_convert(a: ConvertArgs) -> Result<()> {
    let s = load_input(&a.input)?;
    fs::write(&a.output, s + "\n")
        .with_context(|| format!("cannot write {}", a.output.display()))?;
    eprintln!("wrote {}", a.output.display());
    Ok(())
}

fn run_parts(a: PartsArgs) -> Result<()> {
    let gj = load_geojson(&a.input)?;
    let opts = suggest::Options { min_gap: a.min_gap };
    let s = suggest::suggest(&gj, &opts)?;
    eprintln!("min gap: {} (drawing units)", s.min_gap);
    for b in &s.borders {
        eprintln!("sheet border {b:?}: its lines are left out of all parts");
    }
    for c in &s.candidates {
        let kind = match c.kind {
            Kind::TitleBlock => "title block, not placed",
            Kind::Cluster => "placed",
            Kind::Small => "few features, not placed",
        };
        let texts: Vec<&str> = c.texts.iter().take(5).map(String::as_str).collect();
        eprintln!(
            "{}: {} features ({kind}), bbox {:?}",
            c.part.name,
            c.features,
            c.part.bbox.unwrap_or_default()
        );
        if !texts.is_empty() {
            eprintln!("  texts: {}", texts.join(" / "));
        }
    }
    let parts: Vec<Part> = s.candidates.iter().map(|c| c.part.clone()).collect();
    print_follow_hints(&gj, &parts)?;
    // 1 行に 1 部分（手で直しやすいように）
    let lines = parts
        .iter()
        .map(|p| serde_json::to_string(p).map(|s| format!("    {s}")))
        .collect::<Result<Vec<_>, _>>()?;
    let out = format!(
        "{{
  \"parts\": [
{}
  ]
}}
",
        lines.join(
            ",
"
        )
    );
    match &a.output {
        Some(p) => {
            fs::write(p, out).with_context(|| format!("cannot write {}", p.display()))?;
            eprintln!("wrote {}", p.display());
        }
        None => print!("{out}"),
    }
    eprintln!("rename the parts, and add \"follow\" where a part should be placed by another");
    Ok(())
}

/// 載せる候補どうしで共通する円があれば、`follow` で合わせられることを知らせる。
fn print_follow_hints(gj: &Value, parts: &[Part]) -> Result<()> {
    let circles = relate::circles_by_part(gj, parts)?;
    for (i, p) in parts.iter().enumerate().filter(|(_, p)| p.place) {
        for (j, q) in parts
            .iter()
            .enumerate()
            .skip(i + 1)
            .filter(|(_, q)| q.place)
        {
            let Ok(r) = relate::relate(&circles[j], &circles[i], &p.name) else {
                continue;
            };
            if r.runner_up * 2 <= r.pairs.len() {
                eprintln!(
                    "hint: {} and {} share {} circles (next best offset {}); one can follow the other",
                    q.name,
                    p.name,
                    r.pairs.len(),
                    r.runner_up
                );
            }
        }
    }
    Ok(())
}

/// 入力を図面座標の GeoJSON（文字列）にする。GeoJSON ならそのまま返す。
fn load_input(input: &Input) -> Result<String> {
    let p = &input.input;
    let bytes = fs::read(p).with_context(|| format!("cannot read {}", p.display()))?;
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{') {
        return String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", p.display()));
    }
    let opts = cad2geojson_read::Options {
        dxf_encoding: input.dxf_encoding.clone(),
        polygonize_closed: false,
    };
    let d = cad2geojson_read::read(&bytes, &opts)
        .with_context(|| format!("cannot read {}", p.display()))?;
    let fmt = match d.format {
        cad2geojson_read::Format::Dwg => "DWG".to_string(),
        cad2geojson_read::Format::Dxf => format!("DXF {}", d.dxf_version.as_deref().unwrap_or("?")),
    };
    eprintln!("read {} ({fmt}): {} features", p.display(), d.feature_count);
    for (kind, reason, n) in &d.skipped {
        eprintln!("  skipped {n} {kind}: {reason}");
    }
    for w in &d.warnings {
        eprintln!("  warning: {w}");
    }
    Ok(d.geojson)
}

fn load_geojson(input: &Input) -> Result<Value> {
    serde_json::from_str(&load_input(input)?)
        .with_context(|| format!("invalid JSON {}", input.input.display()))
}

fn convert(gj: &Value, record: &Record, output: &Path) -> Result<()> {
    let (out, summary) = geojson::apply(gj, record)?;
    write_geojson(&out, output)?;
    print_summary(&summary, output);
    Ok(())
}

fn read(p: &Path) -> Result<String> {
    fs::read_to_string(p).with_context(|| format!("cannot read {}", p.display()))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn parse_units(u: &str) -> Result<f64> {
    Ok(match u.to_ascii_lowercase().as_str() {
        "mm" => 0.001,
        "cm" => 0.01,
        "m" => 1.0,
        "km" => 1000.0,
        s => match s.parse::<f64>() {
            Ok(v) if v > 0.0 => v,
            _ => bail!("invalid --units '{u}' (mm, cm, m, km or meters per drawing unit)"),
        },
    })
}

/// part,cad_x,cad_y,map_x,map_y[,role]。`#` で始まる行と、数値でない見出し行は読み飛ばす。
fn read_gcps(p: &Path) -> Result<Vec<Gcp>> {
    let s = read(p)?;
    let mut out = vec![];
    for (i, line) in s.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let nums: Result<Vec<f64>, _> =
            f.iter().skip(1).take(4).map(|v| v.parse::<f64>()).collect();
        let nums = match nums {
            Ok(n) if n.len() == 4 => n,
            // 見出し行（1 行目で cad_x が数値でない）
            _ if i == 0 && f.get(1).is_none_or(|v| v.parse::<f64>().is_err()) => continue,
            _ => bail!(
                "{}:{}: expected part,cad_x,cad_y,map_x,map_y[,role]",
                p.display(),
                i + 1
            ),
        };
        let role = match f.get(5).copied().unwrap_or("") {
            "" | "control" => Role::Control,
            "check" => Role::Check,
            r => bail!(
                "{}:{}: unknown role '{r}' (control | check)",
                p.display(),
                i + 1
            ),
        };
        out.push(Gcp {
            part: f[0].to_string(),
            cad: [nums[0], nums[1]],
            map: [nums[2], nums[3]],
            role,
        });
    }
    Ok(out)
}

fn write_geojson(v: &Value, p: &Path) -> Result<()> {
    let w = fs::File::create(p).with_context(|| format!("cannot write {}", p.display()))?;
    let mut w = BufWriter::new(w);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "geojsonl" || ext == "ndjson" {
        for f in v["features"].as_array().into_iter().flatten() {
            serde_json::to_writer(&mut w, f)?;
            w.write_all(b"\n")?;
        }
    } else {
        serde_json::to_writer(&mut w, v)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    Ok(())
}

fn print_record(r: &Record) {
    eprintln!(
        "work CRS: {}  (control points given in {})",
        r.work_crs, r.gcp_crs
    );
    for p in &r.parts {
        let Some(t) = p.transform else {
            eprintln!("part {}: not placed", p.part.name);
            continue;
        };
        eprintln!(
            "part {}: rotation {:.4} deg, scale {}, translation ({:.3}, {:.3})",
            p.part.name, t.rotation_deg, t.scale, t.translation[0], t.translation[1]
        );
        if let Some(r) = &p.relation {
            eprintln!(
                "  placed by part {}: {} common circles, rotation {:+.4} deg, residual median {:.1} / max {:.1} (drawing units)",
                r.to,
                r.pairs.len(),
                r.transform.rotation_deg,
                r.residual_median,
                r.residual_max
            );
            eprintln!("  next best offset matched {} circles", r.runner_up);
            if r.runner_up * 2 > r.pairs.len() {
                eprintln!(
                    "  warning: another offset matches almost as many circles; check the result"
                );
            }
        }
        if let Some(fs) = p.free_scale {
            eprintln!(
                "  scale solved freely would be {:.7} ({:+.2}% vs fixed)",
                fs,
                (fs / t.scale - 1.0) * 100.0
            );
        }
        for (label, pts, stats) in [
            ("control", &p.control, p.control_error),
            ("check", &p.check, p.check_error),
        ] {
            for (i, q) in pts.iter().enumerate() {
                eprintln!(
                    "  {label} {}: residual {:.3} m (dx {:+.3}, dy {:+.3})",
                    i + 1,
                    q.residual[0].hypot(q.residual[1]),
                    q.residual[0],
                    q.residual[1]
                );
            }
            if let Some(s) = stats {
                eprintln!("  {label} RMS {:.3} m, max {:.3} m", s.rms, s.max);
            }
        }
    }
}

fn print_summary(s: &Summary, out: &Path) {
    let parts: Vec<String> = s.written.iter().map(|(n, c)| format!("{n} {c}")).collect();
    eprintln!("wrote {}: {}", out.display(), parts.join(", "));
    if s.not_placed + s.unassigned + s.no_geometry > 0 {
        eprintln!(
            "  skipped: {} in parts not placed, {} outside all parts, {} without geometry",
            s.not_placed, s.unassigned, s.no_geometry
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(parse_units("mm").unwrap(), 0.001);
        assert_eq!(parse_units("0.3048").unwrap(), 0.3048);
        assert!(parse_units("feet").is_err());
        assert!(parse_units("-1").is_err());
    }

    #[test]
    fn gcp_csv() {
        let dir = std::env::temp_dir().join(format!("cad2geojson-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gcp.csv");
        fs::write(
            &p,
            "part,cad_x,cad_y,map_x,map_y,role\n# comment\n2F,1,2,3,4\n2F,5,6,7,8,check\n",
        )
        .unwrap();
        let g = read_gcps(&p).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].role, Role::Check);
        assert_eq!(g[0].cad, [1.0, 2.0]);
        fs::write(&p, "2F,1,2,3\n").unwrap();
        assert!(read_gcps(&p).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
