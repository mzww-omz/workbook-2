//! Additional question formats introduced in workbook schema version 2.
//!
//! These types describe authoring data (what the workbook declares). Learner
//! answers stay in [`crate::answer::Answer`] so every attempt can be replayed
//! exactly. Validation and grading live next to these definitions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::content::Content;
use crate::workbook::ValidationError;

pub const MAX_ASSET_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_TOTAL_ASSET_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_ASSETS: usize = 50;
pub const MAX_ORDERING_ITEMS: usize = 12;

// ---------------------------------------------------------------------------
// Bundled image assets
// ---------------------------------------------------------------------------

/// An image shipped with a workbook. Exactly one of `file` (a relative path
/// next to the workbook JSON) or `data` (base64) is present. Files are copied
/// into SQLite at import time; the learner's data directory never trusts a
/// path from the workbook.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetRef {
    pub id: String,
    pub alt: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Decoded asset bytes plus intrinsic size. Produced by the importer and
/// stored in SQLite so backups include the image data.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetContent {
    pub id: String,
    pub alt: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

impl AssetRef {
    pub fn validate(&self, path: &str) -> Result<(), ValidationError> {
        validate_asset_id(&self.id, &format!("{path}.id"))?;
        if self.alt.trim().is_empty() || self.alt.chars().count() > 2_000 {
            return Err(ValidationError::new(
                format!("{path}.alt"),
                "代替テキストを1〜2000文字で指定してください",
            ));
        }
        match (&self.file, &self.data) {
            (Some(file), None) => {
                validate_relative_asset_path(file, &format!("{path}.file"))?;
                let lower = file.to_lowercase();
                let extension = if lower.ends_with(".png") {
                    "image/png"
                } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                    "image/jpeg"
                } else if lower.ends_with(".svg") {
                    "image/svg+xml"
                } else {
                    ""
                };
                if extension != self.media_type {
                    return Err(ValidationError::new(
                        format!("{path}.mediaType"),
                        "拡張子とmediaTypeが一致しません",
                    ));
                }
            }
            (None, Some(_)) => {}
            _ => {
                return Err(ValidationError::new(
                    path.to_string(),
                    "file または data のどちらか一方を指定してください",
                ));
            }
        }
        if !matches!(
            self.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/svg+xml"
        ) {
            return Err(ValidationError::new(
                format!("{path}.mediaType"),
                "image/png, image/jpeg, image/svg+xml のいずれかを指定してください",
            ));
        }
        Ok(())
    }

    /// Decodes the base64 payload for an embedded asset. External file assets
    /// return `None`; the import layer resolves those from disk.
    pub fn embedded_bytes(&self) -> Result<Option<Vec<u8>>, ValidationError> {
        let Some(data) = self.data.as_ref() else {
            return Ok(None);
        };
        let bytes =
            decode_base64(data).map_err(|message| ValidationError::new("asset.data", message))?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(ValidationError::new(
                "asset.data",
                format!(
                    "画像は{}MB以下にしてください",
                    MAX_ASSET_BYTES / (1024 * 1024)
                ),
            ));
        }
        Ok(Some(bytes))
    }
}

pub fn validate_relative_asset_path(value: &str, path: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.chars().count() > 1_000 {
        return Err(ValidationError::new(path, "画像パスを指定してください"));
    }
    if value.starts_with('/') || value.starts_with('\\') || value.contains(':') {
        return Err(ValidationError::new(path, "相対パスを指定してください"));
    }
    for component in value.split(['/', '\\']) {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ValidationError::new(
                path,
                "安全でないパス要素が含まれています",
            ));
        }
        if component.len() > 200 {
            return Err(ValidationError::new(path, "パスが長すぎます"));
        }
    }
    let lower = value.to_lowercase();
    if !(lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".svg"))
    {
        return Err(ValidationError::new(
            path,
            "PNG, JPEG, SVG のいずれかを指定してください",
        ));
    }
    Ok(())
}

/// Verifies that bytes match the declared media type and returns the intrinsic
/// size. SVG is accepted only through a strict static subset (no scripts,
/// external references, stylesheets, or embedded images).
pub fn inspect_image(
    bytes: &[u8],
    media_type: &str,
    path: &str,
) -> Result<(u32, u32), ValidationError> {
    let (width, height) = match media_type {
        "image/png" => png_size(bytes),
        "image/jpeg" => jpeg_size(bytes),
        "image/svg+xml" => {
            return inspect_svg(bytes, path);
        }
        _ => None,
    }
    .ok_or_else(|| ValidationError::new(path, "画像データを読み取れません（PNG/JPEGのみ対応）"))?;
    if width == 0 || height == 0 || width > 20_000 || height > 20_000 {
        return Err(ValidationError::new(path, "画像サイズが不正です"));
    }
    Ok((width, height))
}

/// Safe, static SVG subset used for bundled diagrams. The renderer never sees
/// scripts, external URLs, `<use>`/`<image>` references, or CSS.
fn inspect_svg(bytes: &[u8], path: &str) -> Result<(u32, u32), ValidationError> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(ValidationError::new(path, "SVGは2MB以下にしてください"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ValidationError::new(path, "SVGはUTF-8で保存してください"))?;
    let lower = text.to_lowercase();
    for forbidden in [
        "<script",
        "<foreignobject",
        "<iframe",
        "<object",
        "<embed",
        "<image",
        "<use",
        "<a ",
        "<style",
        "javascript:",
        "url(",
        "@import",
        "<!doctype",
        "<!entity",
        "href=",
        "xlink:href",
        "style=",
    ] {
        if lower.contains(forbidden) {
            return Err(ValidationError::new(
                path,
                format!("SVGに許可されない要素が含まれています: {forbidden}"),
            ));
        }
    }
    if has_event_handler(&lower) {
        return Err(ValidationError::new(
            path,
            "SVGにイベントハンドラを含めることはできません",
        ));
    }
    let allowed = [
        "svg",
        "g",
        "path",
        "rect",
        "circle",
        "ellipse",
        "polygon",
        "polyline",
        "line",
        "title",
        "desc",
        "defs",
        "clippath",
        "lineargradient",
        "radialgradient",
        "stop",
    ];
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if rest.starts_with('!') || rest.starts_with('?') || rest.starts_with('/') {
            // Comments, declarations, and closing tags are fine; the matching
            // opening tag was already checked.
            if rest.starts_with('/') {
                rest = &rest[1..];
            } else {
                continue;
            }
        }
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == ':')
            .collect();
        if name.is_empty() {
            continue;
        }
        let name = name.to_lowercase();
        if !allowed.contains(&name.as_str()) {
            return Err(ValidationError::new(
                path,
                format!("SVGに許可されない要素が含まれています: <{name}>"),
            ));
        }
    }
    let root = text
        .find("<svg")
        .map(|index| &text[index..])
        .ok_or_else(|| ValidationError::new(path, "SVGのルート要素がありません"))?;
    let tag_end = root
        .find('>')
        .ok_or_else(|| ValidationError::new(path, "SVGのルート要素が壊れています"))?;
    let tag = &root[..tag_end];
    let width = svg_dimension(tag, "width");
    let height = svg_dimension(tag, "height");
    let (width, height) = match (width, height) {
        (Some(width), Some(height)) => (width, height),
        _ => {
            let view_box = svg_attribute(tag, "viewbox")
                .ok_or_else(|| ValidationError::new(path, "SVGにサイズがありません"))?;
            let values: Vec<f64> = view_box
                .split(|ch: char| ch.is_whitespace() || ch == ',')
                .filter_map(|value| value.parse::<f64>().ok())
                .collect();
            if values.len() != 4 {
                return Err(ValidationError::new(path, "SVGのviewBoxが不正です"));
            }
            (values[2], values[3])
        }
    };
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > 20_000.0
        || height > 20_000.0
    {
        return Err(ValidationError::new(path, "SVGのサイズが不正です"));
    }
    Ok((width.round() as u32, height.round() as u32))
}

fn svg_dimension(tag: &str, name: &str) -> Option<f64> {
    let value = svg_attribute(tag, name)?;
    value
        .trim()
        .trim_end_matches("px")
        .trim()
        .parse::<f64>()
        .ok()
}

fn svg_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_lowercase();
    let needle = format!("{name}=");
    let mut from = 0;
    while let Some(relative) = lower[from..].find(&needle) {
        let start = from + relative;
        let boundary = start == 0
            || lower[..start]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_whitespace());
        if boundary {
            let after = &tag[start + needle.len()..];
            let quote = after.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let rest = &after[1..];
            let end = rest.find(quote)?;
            return Some(&rest[..end]);
        }
        from = start + needle.len();
    }
    None
}

fn has_event_handler(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == b'o' && bytes[index + 1] == b'n' {
            let mut cursor = index + 2;
            while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
                cursor += 1;
            }
            let mut spaces = cursor;
            while spaces < bytes.len() && bytes[spaces].is_ascii_whitespace() {
                spaces += 1;
            }
            if cursor > index + 2 && spaces < bytes.len() && bytes[spaces] == b'=' {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if marker == 0xD8 || marker == 0xD9 {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
            return Some((width, height));
        }
        index += 2 + length;
    }
    None
}

fn decode_base64(value: &str) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = value
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if cleaned.len() > MAX_ASSET_BYTES * 2 {
        return Err("埋め込み画像が大きすぎます".to_string());
    }
    if !cleaned.len().is_multiple_of(4) {
        return Err("base64 の長さが不正です".to_string());
    }
    let mut output = Vec::with_capacity(cleaned.len() / 4 * 3);
    let mut chunk = [0u8; 4];
    for group in cleaned.chunks_exact(4) {
        let mut padding = 0;
        for (index, byte) in group.iter().enumerate() {
            chunk[index] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    padding += 1;
                    0
                }
                _ => return Err("base64 に不正な文字があります".to_string()),
            };
        }
        if padding > 2 {
            return Err("base64 のパディングが不正です".to_string());
        }
        output.push((chunk[0] << 2) | (chunk[1] >> 4));
        if padding < 2 {
            output.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        if padding < 1 {
            output.push((chunk[2] << 6) | chunk[3]);
        }
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Numeric
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumericLiteral {
    Number(f64),
    Fraction(String),
}

impl NumericLiteral {
    pub fn value(&self) -> Result<f64, ValidationError> {
        match self {
            NumericLiteral::Number(value) if value.is_finite() => Ok(*value),
            NumericLiteral::Number(_) => Err(ValidationError::new(
                "answer.value",
                "有限の数値を指定してください",
            )),
            NumericLiteral::Fraction(text) => parse_rational(text).ok_or_else(|| {
                ValidationError::new("answer.value", "数値または分数を指定してください")
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NumericAnswerSpec {
    pub value: NumericLiteral,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significant_figures: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accepted_units: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_unit: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl NumericAnswerSpec {
    pub fn validate(&self, path: &str) -> Result<(), ValidationError> {
        self.value
            .value()
            .map_err(|error| ValidationError::new(format!("{path}.value"), error.message))?;
        if let Some(tolerance) = self.tolerance
            && (!tolerance.is_finite() || tolerance < 0.0)
        {
            return Err(ValidationError::new(
                format!("{path}.tolerance"),
                "0以上の有限値を指定してください",
            ));
        }
        if let Some(sig) = self.significant_figures
            && !(1..=15).contains(&sig)
        {
            return Err(ValidationError::new(
                format!("{path}.significantFigures"),
                "1〜15を指定してください",
            ));
        }
        if let Some(unit) = self.unit.as_ref() {
            validate_unit_text(unit, &format!("{path}.unit"))?;
        } else if self.require_unit || !self.accepted_units.is_empty() {
            return Err(ValidationError::new(
                format!("{path}.unit"),
                "単位が必要です",
            ));
        }
        for (unit, factor) in &self.accepted_units {
            validate_unit_text(unit, &format!("{path}.acceptedUnits.{unit}"))?;
            if !factor.is_finite() || *factor <= 0.0 {
                return Err(ValidationError::new(
                    format!("{path}.acceptedUnits.{unit}"),
                    "正の有限値を指定してください",
                ));
            }
        }
        Ok(())
    }
}

fn validate_unit_text(value: &str, path: &str) -> Result<(), ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 20 {
        return Err(ValidationError::new(
            path,
            "単位を1〜20文字で指定してください",
        ));
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return Err(ValidationError::new(path, "単位に空白は使えません"));
    }
    Ok(())
}

pub fn normalize_unit(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .trim()
        .replace(['μ', 'µ'], "u")
        .to_string()
}

use unicode_normalization::UnicodeNormalization;

/// Global conversion table used only for diagnostics. A workbook must opt in
/// to a unit through `acceptedUnits` before it counts as correct.
pub fn known_unit(unit: &str) -> Option<(&'static str, f64)> {
    Some(match normalize_unit(unit).as_str() {
        "m" => ("length", 1.0),
        "cm" => ("length", 0.01),
        "mm" => ("length", 0.001),
        "km" => ("length", 1000.0),
        "kg" => ("mass", 1.0),
        "g" => ("mass", 0.001),
        "mg" => ("mass", 0.000_001),
        "s" => ("time", 1.0),
        "min" => ("time", 60.0),
        "h" => ("time", 3600.0),
        "A" => ("current", 1.0),
        "mA" => ("current", 0.001),
        "uA" => ("current", 0.000_001),
        "V" => ("voltage", 1.0),
        "mV" => ("voltage", 0.001),
        "kV" => ("voltage", 1000.0),
        "Ω" | "ohm" => ("resistance", 1.0),
        "kΩ" | "kohm" => ("resistance", 1000.0),
        "L" => ("volume", 1.0),
        "mL" => ("volume", 0.001),
        "dL" => ("volume", 0.1),
        "Hz" => ("frequency", 1.0),
        "kHz" => ("frequency", 1000.0),
        "N" => ("force", 1.0),
        "J" => ("energy", 1.0),
        "W" => ("power", 1.0),
        "Pa" => ("pressure", 1.0),
        "mol" => ("amount", 1.0),
        _ => return None,
    })
}

/// Parses `3`, `-3.5`, `3e2`, `3/2`, `-3/4`, optionally followed by a unit.
pub struct ParsedNumeric {
    pub value: f64,
    pub unit: Option<String>,
    pub significant_figures: u32,
    pub fraction: bool,
}

pub fn parse_numeric_answer(value: &str) -> Result<ParsedNumeric, String> {
    let normalized: String = value.nfkc().collect();
    let text = normalized.trim();
    if text.is_empty() {
        return Err("空です".to_string());
    }
    if text.chars().count() > 200 {
        return Err("数値・単位が長すぎます".to_string());
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let (value, significant_figures, fraction) = parse_number_at(bytes, &mut index, text)?;
    let remainder = text[index..].trim();
    let unit = if remainder.is_empty() {
        None
    } else {
        if remainder
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_ascii_digit() || ch == '.' || ch == '/')
        {
            return Err("数値の書き方を確認してください".to_string());
        }
        if remainder.chars().count() > 20 {
            return Err("単位が長すぎます".to_string());
        }
        Some(remainder.to_string())
    };
    Ok(ParsedNumeric {
        value,
        unit,
        significant_figures,
        fraction,
    })
}

fn parse_number_at(
    bytes: &[u8],
    index: &mut usize,
    text: &str,
) -> Result<(f64, u32, bool), String> {
    let start = *index;
    let mut cursor = *index;
    if cursor < bytes.len() && (bytes[cursor] == b'+' || bytes[cursor] == b'-') {
        cursor += 1;
    }
    let numerator_start = cursor;
    cursor = scan_decimal(bytes, cursor)?;
    if cursor == numerator_start {
        return Err("数値が見つかりません".to_string());
    }
    let numerator_text = &text[numerator_start..cursor];
    if cursor < bytes.len() && bytes[cursor] == b'/' {
        cursor += 1;
        let denominator_start = cursor;
        if cursor < bytes.len() && (bytes[cursor] == b'+' || bytes[cursor] == b'-') {
            cursor += 1;
        }
        cursor = scan_decimal(bytes, cursor)?;
        if cursor == denominator_start {
            return Err("分数の分母がありません".to_string());
        }
        let denominator_text = &text[denominator_start..cursor];
        let numerator = parse_decimal(numerator_text)?;
        let denominator = parse_decimal(denominator_text)?;
        if denominator == 0.0 {
            return Err("0で割ることはできません".to_string());
        }
        let sign = if bytes[start] == b'-' { -1.0 } else { 1.0 };
        *index = cursor;
        let significant = count_significant_figures(numerator_text)
            .max(count_significant_figures(denominator_text));
        return Ok((sign * (numerator / denominator), significant, true));
    }
    let number = parse_decimal(&text[start..cursor])?;
    *index = cursor;
    Ok((number, count_significant_figures(numerator_text), false))
}

fn scan_decimal(bytes: &[u8], mut cursor: usize) -> Result<usize, String> {
    let mut digits = 0;
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        cursor += 1;
        digits += 1;
    }
    if cursor < bytes.len() && bytes[cursor] == b'.' {
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return Ok(cursor);
    }
    if cursor < bytes.len() && (bytes[cursor] == b'e' || bytes[cursor] == b'E') {
        let exponent_start = cursor;
        let mut after = cursor + 1;
        if after < bytes.len() && (bytes[after] == b'+' || bytes[after] == b'-') {
            after += 1;
        }
        let exponent_digits = after;
        while after < bytes.len() && bytes[after].is_ascii_digit() {
            after += 1;
        }
        if after == exponent_digits {
            return Err("指数の書き方を確認してください".to_string());
        }
        cursor = after;
        let _ = exponent_start;
    }
    Ok(cursor)
}

fn parse_decimal(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "数値を読み取れません".to_string())
}

fn parse_rational(value: &str) -> Option<f64> {
    let normalized: String = value.nfkc().collect();
    let text = normalized.trim();
    if text.is_empty() {
        return None;
    }
    let (numerator, denominator) = match text.split_once('/') {
        Some((numerator, denominator)) => {
            let numerator = numerator.trim().parse::<f64>().ok()?;
            let denominator = denominator.trim().parse::<f64>().ok()?;
            if denominator == 0.0 {
                return None;
            }
            (numerator, denominator)
        }
        None => (text.parse::<f64>().ok()?, 1.0),
    };
    let value = numerator / denominator;
    value.is_finite().then_some(value)
}

fn count_significant_figures(digits: &str) -> u32 {
    let mut cleaned = String::new();
    for ch in digits.chars() {
        if ch.is_ascii_digit() {
            cleaned.push(ch);
        }
    }
    let first_non_zero = cleaned.find(|ch| ch != '0');
    match first_non_zero {
        Some(index) => (cleaned.len() - index) as u32,
        None => {
            if digits.contains('.') {
                digits
                    .chars()
                    .rev()
                    .take_while(|ch| *ch == '0')
                    .count()
                    .max(1) as u32
            } else {
                1
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ordering / Matching / Hotspot / Labeling authoring data
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchItem {
    pub id: String,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchingSpec {
    pub left: Vec<MatchItem>,
    pub right: Vec<MatchItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HotspotSpec {
    pub asset: String,
    pub regions: Vec<HotspotRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HotspotRegion {
    pub id: String,
    pub alt: String,
    pub shape: HotspotShape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HotspotShape {
    Circle { cx: f64, cy: f64, r: f64 },
    Rect { x: f64, y: f64, w: f64, h: f64 },
    Polygon { points: Vec<[f64; 2]> },
}

impl HotspotShape {
    pub fn validate(&self, path: &str) -> Result<(), ValidationError> {
        match self {
            HotspotShape::Circle { cx, cy, r } => {
                unit_interval(*cx, &format!("{path}.cx"))?;
                unit_interval(*cy, &format!("{path}.cy"))?;
                if !r.is_finite() || *r <= 0.0 || *r > 1.0 {
                    return Err(ValidationError::new(
                        format!("{path}.r"),
                        "0より大きく1以下にしてください",
                    ));
                }
            }
            HotspotShape::Rect { x, y, w, h } => {
                unit_interval(*x, &format!("{path}.x"))?;
                unit_interval(*y, &format!("{path}.y"))?;
                if !w.is_finite() || *w <= 0.0 || *w > 1.0 {
                    return Err(ValidationError::new(
                        format!("{path}.w"),
                        "0より大きく1以下にしてください",
                    ));
                }
                if !h.is_finite() || *h <= 0.0 || *h > 1.0 {
                    return Err(ValidationError::new(
                        format!("{path}.h"),
                        "0より大きく1以下にしてください",
                    ));
                }
                if x + w > 1.000_001 || y + h > 1.000_001 {
                    return Err(ValidationError::new(
                        path,
                        "領域が画像の外にはみ出しています",
                    ));
                }
            }
            HotspotShape::Polygon { points } => {
                if !(3..=100).contains(&points.len()) {
                    return Err(ValidationError::new(
                        format!("{path}.points"),
                        "3〜100個の頂点を指定してください",
                    ));
                }
                for (index, point) in points.iter().enumerate() {
                    unit_interval(point[0], &format!("{path}.points[{index}][0]"))?;
                    unit_interval(point[1], &format!("{path}.points[{index}][1]"))?;
                }
                if polygon_area(points).abs() < 1e-9 {
                    return Err(ValidationError::new(
                        format!("{path}.points"),
                        "面積が0の図形は使用できません",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        match self {
            HotspotShape::Circle { cx, cy, r } => {
                let dx = x - cx;
                let dy = y - cy;
                dx * dx + dy * dy <= r * r
            }
            HotspotShape::Rect { x: rx, y: ry, w, h } => {
                x >= *rx && x <= rx + w && y >= *ry && y <= ry + h
            }
            HotspotShape::Polygon { points } => {
                if points.len() < 3 {
                    return false;
                }
                let mut inside = false;
                let mut j = points.len() - 1;
                for i in 0..points.len() {
                    let (xi, yi) = (points[i][0], points[i][1]);
                    let (xj, yj) = (points[j][0], points[j][1]);
                    if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
                        inside = !inside;
                    }
                    j = i;
                }
                inside
            }
        }
    }
}

fn unit_interval(value: f64, path: &str) -> Result<(), ValidationError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ValidationError::new(
            path,
            "0〜1の正規化座標で指定してください",
        ));
    }
    Ok(())
}

fn polygon_area(points: &[[f64; 2]]) -> f64 {
    let mut area = 0.0;
    let mut j = points.len() - 1;
    for (i, point) in points.iter().enumerate() {
        area += (points[j][0] * point[1]) - (point[0] * points[j][1]);
        j = i;
    }
    area / 2.0
}

fn validate_asset_id(value: &str, path: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() || value.chars().count() > 160 {
        return Err(ValidationError::new(
            path,
            "IDを1〜160文字で指定してください",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LabelingSpec {
    pub asset: String,
    pub targets: Vec<LabelTarget>,
    pub labels: Vec<LabelOption>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LabelTarget {
    pub id: String,
    /// Position of the drop target, normalized to the image (0..1).
    pub x: f64,
    pub y: f64,
    pub alt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LabelOption {
    pub id: String,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderingAnswerSpec {
    pub orders: Vec<Vec<String>>,
}

impl OrderingAnswerSpec {
    pub fn validate(
        &self,
        path: &str,
        option_ids: &BTreeSet<String>,
    ) -> Result<(), ValidationError> {
        if self.orders.is_empty() || self.orders.len() > 20 {
            return Err(ValidationError::new(
                format!("{path}.orders"),
                "1〜20個の正解順序を指定してください",
            ));
        }
        for (index, order) in self.orders.iter().enumerate() {
            let order_path = format!("{path}.orders[{index}]");
            if order.len() != option_ids.len() {
                return Err(ValidationError::new(
                    &order_path,
                    "すべての選択肢を1回ずつ含めてください",
                ));
            }
            let mut seen = BTreeSet::new();
            for id in order {
                if !option_ids.contains(id) {
                    return Err(ValidationError::new(
                        &order_path,
                        format!("存在しないIDです: {id}"),
                    ));
                }
                if !seen.insert(id) {
                    return Err(ValidationError::new(
                        &order_path,
                        format!("IDが重複しています: {id}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchPair {
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchingAnswerSpec {
    pub pairs: Vec<MatchPair>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Vec<MatchPair>>,
}

impl MatchingAnswerSpec {
    pub fn validate(
        &self,
        path: &str,
        left: &BTreeSet<String>,
        right: &BTreeSet<String>,
    ) -> Result<(), ValidationError> {
        validate_assignment(&self.pairs, &format!("{path}.pairs"), left, right)?;
        if self.alternatives.len() > 20 {
            return Err(ValidationError::new(
                format!("{path}.alternatives"),
                "代替解答は20個以下にしてください",
            ));
        }
        for (index, pairs) in self.alternatives.iter().enumerate() {
            validate_assignment(pairs, &format!("{path}.alternatives[{index}]"), left, right)?;
        }
        Ok(())
    }

    pub fn accepted(&self) -> impl Iterator<Item = &Vec<MatchPair>> {
        std::iter::once(&self.pairs).chain(self.alternatives.iter())
    }
}

fn validate_assignment(
    pairs: &[MatchPair],
    path: &str,
    left: &BTreeSet<String>,
    right: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    if pairs.len() != left.len() || left.len() != right.len() {
        return Err(ValidationError::new(
            path,
            "左右のすべての項目を1回ずつ対応付けてください",
        ));
    }
    let mut seen_left = BTreeSet::new();
    let mut seen_right = BTreeSet::new();
    for pair in pairs {
        if !left.contains(&pair.left) {
            return Err(ValidationError::new(
                path,
                format!("存在しない左項目です: {}", pair.left),
            ));
        }
        if !right.contains(&pair.right) {
            return Err(ValidationError::new(
                path,
                format!("存在しない右項目です: {}", pair.right),
            ));
        }
        if !seen_left.insert(&pair.left) {
            return Err(ValidationError::new(
                path,
                format!("左項目が重複しています: {}", pair.left),
            ));
        }
        if !seen_right.insert(&pair.right) {
            return Err(ValidationError::new(
                path,
                format!("右項目が重複しています: {}", pair.right),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HotspotAnswerSpec {
    pub region_ids: Vec<String>,
}

impl HotspotAnswerSpec {
    pub fn validate(&self, path: &str, regions: &BTreeSet<String>) -> Result<(), ValidationError> {
        if self.region_ids.is_empty() || self.region_ids.len() > regions.len() {
            return Err(ValidationError::new(
                format!("{path}.regionIds"),
                "1個以上の正解領域を指定してください",
            ));
        }
        let mut seen = BTreeSet::new();
        for id in &self.region_ids {
            if !regions.contains(id) {
                return Err(ValidationError::new(
                    format!("{path}.regionIds"),
                    format!("存在しない領域IDです: {id}"),
                ));
            }
            if !seen.insert(id) {
                return Err(ValidationError::new(
                    format!("{path}.regionIds"),
                    format!("領域IDが重複しています: {id}"),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LabelingAnswerSpec {
    pub pairs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<BTreeMap<String, String>>,
}

impl LabelingAnswerSpec {
    pub fn validate(
        &self,
        path: &str,
        targets: &BTreeSet<String>,
        labels: &BTreeSet<String>,
    ) -> Result<(), ValidationError> {
        validate_label_map(&self.pairs, &format!("{path}.pairs"), targets, labels)?;
        if self.alternatives.len() > 20 {
            return Err(ValidationError::new(
                format!("{path}.alternatives"),
                "代替解答は20個以下にしてください",
            ));
        }
        for (index, map) in self.alternatives.iter().enumerate() {
            validate_label_map(
                map,
                &format!("{path}.alternatives[{index}]"),
                targets,
                labels,
            )?;
        }
        Ok(())
    }

    pub fn accepted(&self) -> impl Iterator<Item = &BTreeMap<String, String>> {
        std::iter::once(&self.pairs).chain(self.alternatives.iter())
    }
}

fn validate_label_map(
    map: &BTreeMap<String, String>,
    path: &str,
    targets: &BTreeSet<String>,
    labels: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    if map.len() != targets.len() || targets.len() != labels.len() {
        return Err(ValidationError::new(
            path,
            "すべてのラベルを1回ずつ配置してください",
        ));
    }
    let mut seen_labels = BTreeSet::new();
    for (target, label) in map {
        if !targets.contains(target) {
            return Err(ValidationError::new(
                path,
                format!("存在しない配置先です: {target}"),
            ));
        }
        if !labels.contains(label) {
            return Err(ValidationError::new(
                path,
                format!("存在しないラベルです: {label}"),
            ));
        }
        if !seen_labels.insert(label) {
            return Err(ValidationError::new(
                path,
                format!("同じラベルが複数回使われています: {label}"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspect(svg: &str) -> Result<(u32, u32), ValidationError> {
        inspect_image(svg.as_bytes(), "image/svg+xml", "asset")
    }

    #[test]
    fn svg_size_comes_from_width_height_or_viewbox() {
        assert_eq!(
            inspect(r##"<svg width="640" height="420"><path d="M0 0"/></svg>"##).unwrap(),
            (640, 420)
        );
        assert_eq!(
            inspect(r##"<svg viewBox="0 0 320 200"><rect width="1" height="1"/></svg>"##).unwrap(),
            (320, 200)
        );
    }

    #[test]
    fn svg_rejects_scripts_external_references_and_event_handlers() {
        for svg in [
            r##"<svg width="10" height="10"><script>alert(1)</script></svg>"##,
            r##"<svg width="10" height="10"><image href="https://example.com/a.png"/></svg>"##,
            r##"<svg width="10" height="10"><use href="#x"/></svg>"##,
            r##"<svg width="10" height="10"><rect onload="alert(1)"/></svg>"##,
            r##"<svg width="10" height="10"><rect style="fill:red"/></svg>"##,
            r##"<svg width="10" height="10"><rect fill="url(http://x)"/></svg>"##,
            r##"<svg width="10" height="10"><foreignObject/></svg>"##,
            r##"<svg width="10" height="10"><animate/></svg>"##,
            r##"<?xml version="1.0"?><!DOCTYPE svg><svg width="10" height="10"/>"##,
        ] {
            assert!(inspect(svg).is_err(), "should reject: {svg}");
        }
    }

    #[test]
    fn svg_allows_only_static_shapes_and_gradients() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
          <title>t</title><g fill="#fff"><path d="M0 0L1 1"/><rect width="1" height="1"/>
          <circle cx="1" cy="1" r="1"/><polygon points="0,0 1,1 0,1"/></g></svg>"##;
        assert_eq!(inspect(svg).unwrap(), (10, 10));
    }
}
