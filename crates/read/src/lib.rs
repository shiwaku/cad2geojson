//! DWG / DXF を図面座標の GeoJSON にする。
//!
//! - DWG: dwg2geo の `convert_bytes` にそのまま渡す
//! - DXF: acadrust で読み、DWG（メモリ上）に書き出してから dwg2geo に渡す。dwg2geo は DXF を
//!   読めないため。図形の取り出し方は DWG と同じになる
//!
//! 文字コードは、記録（`$DWGCODEPAGE`）だけでなく中身のバイトを見て決める。
//!
//! - 記録が無い R12 などの DXF は、中身が cp932 として妥当なら `ANSI_932` で読む（Jw_cad が書き出す DXF）
//! - R2004 以前で記録があっても、中身が UTF-8 なら UTF-8 で読む。dwg2dxf などの変換器や、UTF-8 で
//!   保存し直したテキストエディタが、記録を元のまま残すことがある
//! - 先頭の UTF-8 の BOM は除く

use std::io::Cursor;

use acadrust::{DwgWriter, DxfReader, DxfReaderConfiguration, DxfVersion};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unrecognized drawing format (expected DWG or ASCII DXF)")]
    UnknownFormat,
    #[error("binary DXF is not supported; save it as ASCII DXF")]
    BinaryDxf,
    #[error("cannot read DXF: {0}")]
    Dxf(String),
    #[error("cannot convert DXF to DWG in memory: {0}")]
    DxfToDwg(String),
    #[error("cannot convert drawing: {0}")]
    Convert(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Dwg,
    Dxf,
}

/// 読み込みの指定。
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// DXF の文字コード（`ANSI_932` など DXF の表記）。`None` なら判定する
    pub dxf_encoding: Option<String>,
    /// 閉じた線をポリゴンにする
    pub polygonize_closed: bool,
}

/// 読み込みの結果。
#[derive(Debug, Clone)]
pub struct Drawing {
    pub format: Format,
    /// 図面座標の GeoJSON（FeatureCollection の文字列）
    pub geojson: String,
    pub feature_count: usize,
    /// DXF の版（`$ACADVER`）
    pub dxf_version: Option<String>,
    /// DXF を読んだ文字コード。ファイルの記録に従ったときは `None`
    pub dxf_encoding: Option<String>,
    /// 変換しなかった図形（種類, 理由, 件数）
    pub skipped: Vec<(String, String, usize)>,
    pub warnings: Vec<String>,
}

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

pub fn detect(bytes: &[u8]) -> Result<Format, Error> {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err(Error::BinaryDxf);
    }
    if bytes.len() >= 6 && bytes.starts_with(b"AC1") && bytes[3..6].iter().all(u8::is_ascii_digit) {
        return Ok(Format::Dwg);
    }
    // ASCII DXF はグループコード 0 と SECTION（またはコメント 999）で始まる
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    let mut lines = head.lines().map(str::trim);
    match (lines.next(), lines.next()) {
        (Some("0"), Some("SECTION")) | (Some("999"), _) => Ok(Format::Dxf),
        _ => Err(Error::UnknownFormat),
    }
}

pub fn read(bytes: &[u8], opts: &Options) -> Result<Drawing, Error> {
    match detect(bytes)? {
        Format::Dwg => convert_dwg(bytes, opts, Format::Dwg, None, None, vec![]),
        Format::Dxf => read_dxf(bytes, opts),
    }
}

fn read_dxf(bytes: &[u8], opts: &Options) -> Result<Drawing, Error> {
    let mut warnings = vec![];
    let bytes = match bytes.strip_prefix(UTF8_BOM) {
        Some(b) => {
            warnings.push("DXF starts with a UTF-8 byte order mark; ignored".to_string());
            b
        }
        None => bytes,
    };
    let header = DxfHeader::scan(bytes);
    // 記録と違う UTF-8 で読むときは、acadrust が記録に従わないよう記録を書き換えた中身を渡す
    let mut relabeled = None;
    let encoding = match &opts.dxf_encoding {
        Some(e) => Some(e.clone()),
        None if header.needs_guess() && looks_like_cp932(bytes) => {
            warnings.push(
                "DXF has no code page; text decoded as ANSI_932 (override with --dxf-encoding)"
                    .to_string(),
            );
            Some("ANSI_932".to_string())
        }
        None if header.mislabeled_utf8(bytes) => {
            let declared = header.codepage.as_deref().unwrap_or_default();
            warnings.push(format!(
                "DXF declares code page {declared} but its text is UTF-8; decoded as UTF-8 \
                 (override with --dxf-encoding)"
            ));
            relabeled = Some(relabel_code_page(bytes));
            Some("UTF-8".to_string())
        }
        None => None,
    };
    let cfg = DxfReaderConfiguration {
        failsafe: false,
        default_encoding: encoding.clone().filter(|e| e != "UTF-8"),
    };
    let source = relabeled.unwrap_or_else(|| bytes.to_vec());
    let mut doc = DxfReader::from_reader(Cursor::new(source))
        .map_err(|e| Error::Dxf(e.to_string()))?
        .with_configuration(cfg)
        .read()
        .map_err(|e| Error::Dxf(e.to_string()))?;
    if let Some(declared) = header
        .codepage
        .as_ref()
        .filter(|_| encoding.as_deref() == Some("UTF-8"))
    {
        // DWG に書くときの文字コードは記録どおりに戻す（読んだ文字は UTF-8 から正しく復元済み）
        doc.header.code_page = declared.clone();
    }
    // acadrust は文書の版のまま DWG を書く。R12 以前の DWG の書き出しは図形の参照に厳しく、
    // Jw_cad の DXF で失敗するので、R2000 として書く
    if matches!(
        doc.version,
        DxfVersion::Unknown | DxfVersion::AC1009 | DxfVersion::AC1012 | DxfVersion::AC1014
    ) {
        doc.version = DxfVersion::AC1015;
    }
    let dwg = DwgWriter::write_to_vec(&doc).map_err(|e| Error::DxfToDwg(e.to_string()))?;
    convert_dwg(&dwg, opts, Format::Dxf, header.version, encoding, warnings)
}

fn convert_dwg(
    bytes: &[u8],
    opts: &Options,
    format: Format,
    dxf_version: Option<String>,
    dxf_encoding: Option<String>,
    mut warnings: Vec<String>,
) -> Result<Drawing, Error> {
    let r = dwg2geo::backend::native::convert_bytes(bytes, opts.polygonize_closed, None)
        .map_err(|e| Error::Convert(format!("{e:#}")))?;
    warnings.extend(r.warnings);
    Ok(Drawing {
        format,
        geojson: r.geojson,
        feature_count: r.feature_count,
        dxf_version,
        dxf_encoding,
        skipped: r
            .skipped
            .into_iter()
            .map(|s| (s.entity_type, s.reason, s.count))
            .collect(),
        warnings,
    })
}

/// DXF のヘッダから、文字コードの判定に要る値だけを拾う。
#[derive(Debug, Default, PartialEq)]
struct DxfHeader {
    version: Option<String>,
    codepage: Option<String>,
}

impl DxfHeader {
    fn scan(bytes: &[u8]) -> DxfHeader {
        let mut h = DxfHeader::default();
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(1 << 20)]);
        let mut lines = text.lines().map(str::trim);
        while let Some(l) = lines.next() {
            match l {
                "$ACADVER" | "$DWGCODEPAGE" => {
                    lines.next(); // グループコード
                    let v = lines.next().map(str::to_string);
                    if l == "$ACADVER" {
                        h.version = v
                    } else {
                        h.codepage = v
                    }
                }
                "ENDSEC" => break,
                _ => {}
            }
        }
        h
    }

    /// R2007（AC1021）以降。文字は UTF-8 と決まっている
    fn is_utf8_version(&self) -> bool {
        self.version.as_deref().is_some_and(|v| v >= "AC1021")
    }

    /// 文字コードを推測する必要があるか
    fn needs_guess(&self) -> bool {
        !self.is_utf8_version() && self.codepage.as_deref().is_none_or(|c| c.is_empty())
    }

    /// 文字コードの記録があるのに、中身が UTF-8 か。ASCII だけなら区別が要らないので false
    fn mislabeled_utf8(&self, bytes: &[u8]) -> bool {
        !self.is_utf8_version()
            && self.codepage.as_deref().is_some_and(|c| {
                !c.is_empty() && !c.eq_ignore_ascii_case("UTF-8") && !c.eq_ignore_ascii_case("UTF8")
            })
            && !bytes.is_ascii()
            && std::str::from_utf8(bytes).is_ok()
    }
}

/// ヘッダの `$DWGCODEPAGE` の値を `UTF-8` に書き換える。acadrust は記録された文字コードを
/// 読み込みの指定より優先し、`UTF-8` という記録は知らないので、そのまま UTF-8 で読む。
fn relabel_code_page(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).expect("checked to be UTF-8");
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n');
    while let Some(l) = lines.next() {
        out.push_str(l);
        match l.trim() {
            "$DWGCODEPAGE" => {
                if let (Some(code), Some(value)) = (lines.next(), lines.next()) {
                    out.push_str(code);
                    out.push_str("UTF-8");
                    out.push_str(&value[value.trim_end_matches(['\r', '\n']).len()..]);
                }
            }
            "ENDSEC" => {
                out.extend(lines);
                break;
            }
            _ => {}
        }
    }
    out.into_bytes()
}

/// ASCII 以外のバイトがすべて Shift_JIS（cp932）の文字として妥当で、2 バイト文字を含むか。
/// UTF-8 として妥当なら cp932 とはみなさない。
fn looks_like_cp932(bytes: &[u8]) -> bool {
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let mut i = 0;
    let mut double = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x00..=0x7F | 0xA1..=0xDF => i += 1,
            0x81..=0x9F | 0xE0..=0xFC => {
                match bytes.get(i + 1) {
                    Some(0x40..=0x7E | 0x80..=0xFC) => double += 1,
                    _ => return false,
                }
                i += 2;
            }
            _ => return false,
        }
    }
    double > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_format() {
        assert_eq!(detect(b"AC1032\0\0\0").unwrap(), Format::Dwg);
        assert_eq!(
            detect(b"  0\r\nSECTION\r\n  2\r\nHEADER\r\n").unwrap(),
            Format::Dxf
        );
        assert_eq!(
            detect(b"999\nmade by x\n  0\nSECTION\n").unwrap(),
            Format::Dxf
        );
        assert_eq!(
            detect(b"\xEF\xBB\xBF  0\r\nSECTION\r\n").unwrap(),
            Format::Dxf
        );
        assert!(matches!(
            detect(b"AutoCAD Binary DXF\r\n\x1a\0"),
            Err(Error::BinaryDxf)
        ));
        assert!(matches!(
            detect(b"{\"type\":\"FeatureCollection\"}"),
            Err(Error::UnknownFormat)
        ));
    }

    #[test]
    fn header_scan() {
        let r12 = b"  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1009\n  0\nENDSEC\n";
        let h = DxfHeader::scan(r12);
        assert_eq!(h.version.as_deref(), Some("AC1009"));
        assert!(h.needs_guess());
        let r2000 = b"  9\n$ACADVER\n  1\nAC1015\n  9\n$DWGCODEPAGE\n  3\nANSI_932\n  0\nENDSEC\n";
        assert!(!DxfHeader::scan(r2000).needs_guess());
        let r2010 = b"  9\n$ACADVER\n  1\nAC1024\n  0\nENDSEC\n";
        assert!(!DxfHeader::scan(r2010).needs_guess());
    }

    #[test]
    fn mislabeled_code_page() {
        let r2000 = |text: &str| {
            format!(
                "  9\r\n$ACADVER\r\n  1\r\nAC1015\r\n  9\r\n$DWGCODEPAGE\r\n  3\r\nANSI_932\r\n  0\r\nENDSEC\r\n  1\r\n{text}\r\n"
            )
        };
        let utf8 = r2000("電話").into_bytes();
        assert!(DxfHeader::scan(&utf8).mislabeled_utf8(&utf8));
        let relabeled = String::from_utf8(relabel_code_page(&utf8)).unwrap();
        assert!(relabeled.contains("$DWGCODEPAGE\r\n  3\r\nUTF-8\r\n  0\r\nENDSEC"));
        assert!(relabeled.ends_with("電話\r\n"));
        // 中身も cp932 なら記録どおり
        let sjis = encoding_rs::SHIFT_JIS.encode(&r2000("電話")).0.into_owned();
        assert!(!DxfHeader::scan(&sjis).mislabeled_utf8(&sjis));
        // ASCII だけなら区別しない
        let ascii = r2000("TEL").into_bytes();
        assert!(!DxfHeader::scan(&ascii).mislabeled_utf8(&ascii));
    }

    #[test]
    fn cp932_guess() {
        // 「電話」を cp932 で
        assert!(looks_like_cp932(b"TEXT\n\x93\x64\x98\x62\n"));
        // 半角カナだけ（2 バイト文字なし）は判定しない
        assert!(!looks_like_cp932(b"\xB1\xB2"));
        // UTF-8 の「電話」
        assert!(!looks_like_cp932("電話".as_bytes()));
        // Latin-1 の é（0xE9）の後に ASCII の改行: 2 バイト目が範囲外
        assert!(!looks_like_cp932(b"caf\xE9\n"));
        assert!(!looks_like_cp932(b"plain ascii"));
    }
}
