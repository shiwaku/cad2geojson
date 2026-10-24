//! cad2geojson の中核。図面座標の GeoJSON を部分に分け、部分ごとに位置合わせし、WGS84 にする。
//! ファイル I/O は持たない（CLI と WASM で共有する）。

pub mod crs;
pub mod fit;
pub mod geojson;
pub mod georef;
pub mod part;
pub mod relate;
pub mod suggest;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "unsupported CRS: {0} (supported: EPSG:4326, 6668, 4612, 3857, 6669-6687, 2443-2461,          UTM 32601-32660 / 32701-32760 / 25828-25838 / 6688-6692 / 3097-3101, 28992)"
    )]
    UnsupportedCrs(String),
    #[error(
        "{0} cannot be used as the work CRS; use a projected CRS (Japan Plane Rectangular EPSG:6669-6687, UTM or RD New)"
    )]
    NotWorkCrs(String),
    #[error("projection error: {0}")]
    Projection(String),
    #[error("at least {need} control points are required, got {got}")]
    TooFewPoints { need: usize, got: usize },
    #[error("part '{part}' needs at least 2 control points, got {got}")]
    PartTooFewPoints { part: String, got: usize },
    #[error("control points are degenerate (coincident)")]
    DegeneratePoints,
    #[error("control point refers to unknown part '{0}'")]
    UnknownPart(String),
    #[error("invalid georef record: {0}")]
    Record(String),
    #[error("cannot relate parts: {0}")]
    Relation(String),
    #[error("invalid GeoJSON: {0}")]
    GeoJson(String),
}
