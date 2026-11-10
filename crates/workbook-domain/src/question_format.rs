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
/// Guard against a highly compressed image that would decode into an
/// unreasonable amount of RGBA memory. Educational diagrams do not need more.
pub const MAX_IMAGE_PIXELS: u64 = 24_000_000;

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
/// external references, stylesheets, or embedded images). PNG/JPEG files must
/// decode completely; a plausible header alone is not enough.
pub fn inspect_image(
    bytes: &[u8],
    media_type: &str,
    path: &str,
) -> Result<(u32, u32), ValidationError> {
    let (width, height) = match media_type {
        "image/png" => inspect_raster(bytes, image::ImageFormat::Png, path)?,
        "image/jpeg" => inspect_raster(bytes, image::ImageFormat::Jpeg, path)?,
        "image/svg+xml" => {
            return inspect_svg(bytes, path);
        }
        _ => {
            return Err(ValidationError::new(
                path,
                "画像の形式は PNG / JPEG / SVG のいずれかに対応しています",
            ));
        }
    };
    validate_image_dimensions(width, height, path)?;
    Ok((width, height))
}

fn validate_image_dimensions(width: u32, height: u32, path: &str) -> Result<(), ValidationError> {
    if width == 0 || height == 0 || width > 20_000 || height > 20_000 {
        return Err(ValidationError::new(path, "画像サイズが不正です"));
    }
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(ValidationError::new(
            path,
            format!(
                "画像の画素数が上限を超えています（最大{}万画素）",
                MAX_IMAGE_PIXELS / 10_000
            ),
        ));
    }
    Ok(())
}

/// Decodes PNG/JPEG completely and returns the real dimensions.
///
/// The format is guessed from the bytes and compared with the declared media
/// type, so a PNG renamed to `.jpg` (or a JPEG served as `image/png`) is
/// rejected. Dimensions are read before the pixel buffer is allocated, and the
/// decoder receives hard allocation limits so a decompression bomb cannot
/// exhaust memory. A truncated file, a bad CRC, or an invalid segment makes the
/// decoder fail; callers never receive a header-only "valid" result.
fn inspect_raster(
    bytes: &[u8],
    expected: image::ImageFormat,
    path: &str,
) -> Result<(u32, u32), ValidationError> {
    if bytes.is_empty() {
        return Err(ValidationError::new(path, "画像データが空です"));
    }
    let guessed = image::guess_format(bytes).map_err(|_| {
        ValidationError::new(
            path,
            "画像データを読み取れません。PNG/JPEGの完全なファイルを指定してください",
        )
    })?;
    if guessed != expected {
        return Err(ValidationError::new(
            path,
            format!(
                "mediaType と画像データの形式が一致しません（宣言: {}、実際: {}）",
                media_label(expected),
                media_label(guessed)
            ),
        ));
    }
    if expected == image::ImageFormat::Jpeg && !jpeg_has_eoi(bytes) {
        return Err(ValidationError::new(
            path,
            "JPEGが途中で切れています（終端のEOIマーカーがありません）",
        ));
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    // RGBA output for the largest allowed image, with headroom for the
    // decoder's own row buffers.
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4 + 16 * 1024 * 1024);
    let mut header_reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), expected);
    header_reader.limits(limits.clone());
    let (width, height) = header_reader.into_dimensions().map_err(|error| {
        ValidationError::new(
            path,
            format!("画像データが壊れているか途中で切れています: {error}"),
        )
    })?;
    validate_image_dimensions(width, height, path)?;

    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), expected);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| {
        ValidationError::new(
            path,
            format!("画像データをデコードできません（破損または未対応の形式）: {error}"),
        )
    })?;
    if image.width() != width || image.height() != height {
        return Err(ValidationError::new(
            path,
            "画像のヘッダーとデコード結果のサイズが一致しません",
        ));
    }
    Ok((width, height))
}

/// JPEG decoders may accept a stream that ends before the EOI marker once all
/// scan data is present. Treat that as a truncated file: every complete JPEG we
/// accept ends with `FF D9` (optionally followed by ASCII whitespace/NUL).
fn jpeg_has_eoi(bytes: &[u8]) -> bool {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], 0x00 | b'\n' | b'\r' | b' ' | b'\t') {
        end -= 1;
    }
    end >= 2 && bytes[end - 2] == 0xFF && bytes[end - 1] == 0xD9
}

fn media_label(format: image::ImageFormat) -> &'static str {
    match format {
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Jpeg => "JPEG",
        _ => "その他",
    }
}

const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
const MAX_SVG_NODES: u32 = 10_000;
const MAX_SVG_ELEMENTS: usize = 4_000;
const MAX_SVG_DEPTH: usize = 64;

/// Elements that the static diagram subset may use. Names are matched exactly:
/// XML is case-sensitive, so `<SCRIPT>` or `<foreignobject>` cannot slip through
/// a case-insensitive allowlist.
const SAFE_SVG_ELEMENTS: &[&str] = &[
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
    "clipPath",
    "linearGradient",
    "radialGradient",
    "stop",
];

/// Attributes that only carry geometry, paint, or accessibility information.
/// `style`, `class`, `href`, `xlink:href`, and every event handler are absent on
/// purpose: there is no way to express script, CSS, or a remote reference.
const SAFE_SVG_ATTRIBUTES: &[&str] = &[
    "id",
    "role",
    "aria-label",
    "aria-hidden",
    "x",
    "y",
    "width",
    "height",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "fr",
    "d",
    "points",
    "transform",
    "gradientTransform",
    "gradientUnits",
    "spreadMethod",
    "clipPathUnits",
    "viewBox",
    "preserveAspectRatio",
    "version",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "color",
    "display",
    "visibility",
    "clip-path",
    "offset",
    "stop-color",
    "stop-opacity",
];

/// Local names that must never appear on an element, even if a future edit
/// accidentally adds one to the attribute allowlist.
const FORBIDDEN_SVG_ATTRIBUTES: &[&str] = &[
    "href",
    "src",
    "srcset",
    "action",
    "formaction",
    "poster",
    "data",
    "cite",
    "background",
    "longdesc",
    "usemap",
    "style",
];

/// Safe, static SVG subset used for bundled diagrams. Validation works on the
/// parsed XML tree, not on a substring scan: whitespace around `=`, entity
/// encodings, namespace prefixes, and attribute order cannot bypass it.
fn inspect_svg(bytes: &[u8], path: &str) -> Result<(u32, u32), ValidationError> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(ValidationError::new(path, "SVGは2MB以下にしてください"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ValidationError::new(path, "SVGはUTF-8で保存してください"))?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_SVG_NODES,
        ..Default::default()
    };
    let document = roxmltree::Document::parse_with_options(text, options).map_err(|error| {
        ValidationError::new(
            path,
            format!("SVGのXML構文が不正です（DTD・外部エンティティは使用できません）: {error}"),
        )
    })?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(ValidationError::new(
            path,
            "SVGのルート要素は <svg> である必要があります",
        ));
    }
    if root
        .tag_name()
        .namespace()
        .is_some_and(|ns| ns != SVG_NAMESPACE)
    {
        return Err(ValidationError::new(
            path,
            "SVGに許可されない名前空間が含まれています",
        ));
    }

    let mut elements = 0usize;
    let mut stack = vec![(root, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_SVG_DEPTH {
            return Err(ValidationError::new(
                path,
                format!("SVGのネストが深すぎます（最大{MAX_SVG_DEPTH}階層）"),
            ));
        }
        validate_svg_element(node, path)?;
        elements += 1;
        if elements > MAX_SVG_ELEMENTS {
            return Err(ValidationError::new(
                path,
                format!("SVGの要素数が多すぎます（最大{MAX_SVG_ELEMENTS}個）"),
            ));
        }
        for child in node.children() {
            match child.node_type() {
                roxmltree::NodeType::Element => stack.push((child, depth + 1)),
                roxmltree::NodeType::PI => {
                    return Err(ValidationError::new(
                        path,
                        "SVGに処理命令（processing instruction）を含めることはできません",
                    ));
                }
                roxmltree::NodeType::Text | roxmltree::NodeType::Comment => {}
                roxmltree::NodeType::Root => {}
            }
        }
    }

    let width = svg_dimension(root, "width");
    let height = svg_dimension(root, "height");
    let (width, height) = match (width, height) {
        (Some(width), Some(height)) => (width, height),
        _ => {
            let view_box = root
                .attribute("viewBox")
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
    let (width, height) = (width.round() as u32, height.round() as u32);
    validate_image_dimensions(width, height, path)?;
    Ok((width, height))
}

fn validate_svg_element(node: roxmltree::Node<'_, '_>, path: &str) -> Result<(), ValidationError> {
    let name = node.tag_name().name();
    if !SAFE_SVG_ELEMENTS.contains(&name) {
        return Err(ValidationError::new(
            path,
            format!("SVGに許可されない要素が含まれています: <{name}>"),
        ));
    }
    if node
        .tag_name()
        .namespace()
        .is_some_and(|ns| ns != SVG_NAMESPACE)
    {
        return Err(ValidationError::new(
            path,
            format!("SVGに許可されない名前空間が含まれています: <{name}>"),
        ));
    }
    for namespace in node.namespaces() {
        let uri = namespace.uri();
        if namespace.name().is_none() && uri == SVG_NAMESPACE {
            continue;
        }
        return Err(ValidationError::new(
            path,
            format!("SVGに許可されない名前空間宣言が含まれています: {uri}"),
        ));
    }
    for attribute in node.attributes() {
        let local = attribute.name();
        let lower = local.to_ascii_lowercase();
        if attribute.namespace().is_some() {
            return Err(ValidationError::new(
                path,
                format!("SVGに名前空間付き属性を含めることはできません: {local}"),
            ));
        }
        if FORBIDDEN_SVG_ATTRIBUTES.contains(&lower.as_str()) {
            return Err(ValidationError::new(
                path,
                format!("SVGに許可されない属性が含まれています: {local}"),
            ));
        }
        if lower.starts_with("on") && lower.len() > 2 {
            return Err(ValidationError::new(
                path,
                format!("SVGにイベントハンドラ属性を含めることはできません: {local}"),
            ));
        }
        if !SAFE_SVG_ATTRIBUTES.contains(&local) {
            return Err(ValidationError::new(
                path,
                format!("SVGに許可されない属性が含まれています: {local}"),
            ));
        }
        validate_svg_attribute_value(attribute.value(), path, local)?;
    }
    Ok(())
}

fn validate_svg_attribute_value(
    value: &str,
    path: &str,
    attribute: &str,
) -> Result<(), ValidationError> {
    // XML entities are decoded before this point, so `java&#115;cript:` is
    // caught here as well. Whitespace and control characters are removed to
    // stop `java\nscript:` style evasions.
    let normalized: String = value
        .chars()
        .filter(|ch| !ch.is_whitespace() && !ch.is_control())
        .flat_map(char::to_lowercase)
        .collect();
    for scheme in [
        "javascript:",
        "vbscript:",
        "data:",
        "file:",
        "http:",
        "https:",
        "ftp:",
    ] {
        if normalized.contains(scheme) {
            return Err(ValidationError::new(
                path,
                format!("SVGの属性に外部参照・スクリプトURLは使用できません: {attribute}"),
            ));
        }
    }
    if normalized.contains("//") || value.contains('<') || value.contains('>') {
        return Err(ValidationError::new(
            path,
            format!("SVGの属性に外部参照・マークアップは使用できません: {attribute}"),
        ));
    }
    // `url(...)` is only accepted for an internal fragment such as
    // `url(#gradient)`, which cannot reach the network or the filesystem.
    let mut rest = normalized.as_str();
    while let Some(index) = rest.find("url(") {
        let after = &rest[index + 4..];
        let Some(close) = after.find(')') else {
            return Err(ValidationError::new(
                path,
                format!("SVGの属性に不正な url() があります: {attribute}"),
            ));
        };
        let target = after[..close].trim();
        let Some(id) = target.strip_prefix('#') else {
            return Err(ValidationError::new(
                path,
                format!("SVGの属性に外部を参照する url() は使用できません: {attribute}"),
            ));
        };
        if id.is_empty()
            || !id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
        {
            return Err(ValidationError::new(
                path,
                format!("SVGの属性に不正な url() があります: {attribute}"),
            ));
        }
        rest = &after[close + 1..];
    }
    Ok(())
}

fn svg_dimension(node: roxmltree::Node<'_, '_>, name: &str) -> Option<f64> {
    let value = node.attribute(name)?;
    value
        .trim()
        .strip_suffix("px")
        .unwrap_or(value.trim())
        .trim()
        .parse::<f64>()
        .ok()
}

/// Standard base64 with padding. Used by JSON export so a workbook becomes a
/// single portable file again instead of referring to local asset paths.
pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let first = group[0];
        let second = group.get(1).copied().unwrap_or(0);
        let third = group.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        if group.len() > 1 {
            output.push(TABLE[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if group.len() > 2 {
            output.push(TABLE[(third & 0b11_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
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

/// Global conversion table into a shared base unit per dimension. It is used
/// only for diagnostics and for comparing an equivalent unit the workbook did
/// not explicitly accept. A workbook must opt in to a unit through
/// `acceptedUnits` before an answer using that unit can count as correct.
///
/// The factor converts the unit into the dimension's base unit (`cm` → m,
/// `mA` → A). `acceptedUnits` factors are relative to the expected unit, so
/// grading combines them with the expected unit's base factor.
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

/// Counts significant figures from the mantissa only.
///
/// Rules:
/// - The exponent (`e`/`E` and everything after it) never contributes digits.
/// - A leading sign never contributes.
/// - Leading zeros are not significant.
/// - Trailing zeros written after a decimal point are significant.
/// - Integer trailing zeros are counted as written (`300` → 3). Use exponent
///   notation (`3e2` → 1) when the trailing zeros are placeholders.
/// - An all-zero mantissa keeps the zeros written after the decimal point:
///   `0` and `0.0` → 1, `0.00` → 2.
fn count_significant_figures(literal: &str) -> u32 {
    let mantissa = literal
        .split(['e', 'E'])
        .next()
        .unwrap_or(literal)
        .trim()
        .trim_start_matches(['+', '-']);
    let digits: String = mantissa.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let first_non_zero = digits.find(|ch| ch != '0');
    match first_non_zero {
        Some(index) => (digits.len() - index) as u32,
        None => {
            let decimals = mantissa
                .split_once('.')
                .map(|(_, fraction)| fraction.chars().filter(|ch| ch.is_ascii_digit()).count())
                .unwrap_or(0);
            decimals.max(1) as u32
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

    /// A proven interior point for keyboard and screen-reader alternatives.
    /// Circles and rectangles use their centre; polygons try the area centroid
    /// and then a scanline through the widest interior interval. The result is
    /// always checked with [`Self::contains`], and `None` means no point could
    /// be proven rather than an invented coordinate.
    pub fn representative_point(&self) -> Option<(f64, f64)> {
        match self {
            HotspotShape::Circle { cx, cy, r } => {
                if !cx.is_finite()
                    || !cy.is_finite()
                    || !r.is_finite()
                    || *r <= 0.0
                    || !(0.0..=1.0).contains(cx)
                    || !(0.0..=1.0).contains(cy)
                {
                    return None;
                }
                let point = (*cx, *cy);
                self.contains(point.0, point.1).then_some(point)
            }
            HotspotShape::Rect { x, y, w, h } => {
                if !x.is_finite()
                    || !y.is_finite()
                    || !w.is_finite()
                    || !h.is_finite()
                    || *w <= 0.0
                    || *h <= 0.0
                    || *x < 0.0
                    || *y < 0.0
                    || x + w > 1.000_001
                    || y + h > 1.000_001
                {
                    return None;
                }
                let point = (x + w / 2.0, y + h / 2.0);
                if !(0.0..=1.0).contains(&point.0) || !(0.0..=1.0).contains(&point.1) {
                    return None;
                }
                self.contains(point.0, point.1).then_some(point)
            }
            HotspotShape::Polygon { points } => polygon_interior_point(points),
        }
    }
}

fn polygon_interior_point(points: &[[f64; 2]]) -> Option<(f64, f64)> {
    if points.len() < 3
        || points.iter().any(|point| {
            !point[0].is_finite()
                || !point[1].is_finite()
                || !(0.0..=1.0).contains(&point[0])
                || !(0.0..=1.0).contains(&point[1])
        })
    {
        return None;
    }
    if polygon_area(points).abs() < 1e-12 {
        return None;
    }

    let mut candidates = Vec::new();
    if let Some(centroid) = polygon_centroid(points) {
        candidates.push(centroid);
    }
    let count = points.len() as f64;
    candidates.push((
        points.iter().map(|point| point[0]).sum::<f64>() / count,
        points.iter().map(|point| point[1]).sum::<f64>() / count,
    ));

    let mut heights: Vec<f64> = points.iter().map(|point| point[1]).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    heights.dedup();
    for pair in heights.windows(2) {
        let y = (pair[0] + pair[1]) / 2.0;
        if let Some(point) = widest_interval_midpoint(points, y) {
            candidates.push(point);
        }
    }

    candidates.into_iter().find(|(x, y)| {
        point_in_unit_square(*x, *y)
            && HotspotShape::Polygon {
                points: points.to_vec(),
            }
            .contains(*x, *y)
    })
}

fn point_in_unit_square(x: f64, y: f64) -> bool {
    x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
}

fn polygon_centroid(points: &[[f64; 2]]) -> Option<(f64, f64)> {
    let mut twice_area = 0.0;
    let mut x_sum = 0.0;
    let mut y_sum = 0.0;
    let mut j = points.len() - 1;
    for (i, point) in points.iter().enumerate() {
        let cross = points[j][0] * point[1] - point[0] * points[j][1];
        twice_area += cross;
        x_sum += (points[j][0] + point[0]) * cross;
        y_sum += (points[j][1] + point[1]) * cross;
        j = i;
    }
    if twice_area.abs() < 1e-12 {
        return None;
    }
    let x = x_sum / (3.0 * twice_area);
    let y = y_sum / (3.0 * twice_area);
    point_in_unit_square(x, y).then_some((x, y))
}

fn widest_interval_midpoint(points: &[[f64; 2]], y: f64) -> Option<(f64, f64)> {
    let mut crossings = Vec::new();
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = (points[i][0], points[i][1]);
        let (xj, yj) = (points[j][0], points[j][1]);
        if (yi > y) != (yj > y) {
            let x = xj + (y - yj) * (xi - xj) / (yi - yj);
            if x.is_finite() {
                crossings.push(x);
            }
        }
        j = i;
    }
    crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    crossings
        .chunks_exact(2)
        .filter_map(|pair| {
            let width = pair[1] - pair[0];
            (width > 0.0).then_some((width, (pair[0] + pair[1]) / 2.0))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, x)| (x, y))
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

pub(crate) fn validate_asset_id(value: &str, path: &str) -> Result<(), ValidationError> {
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
    fn significant_figures_exclude_the_exponent_and_sign() {
        for (text, expected) in [
            ("3", 1),
            ("3.0", 2),
            ("3.00", 3),
            ("0.0030", 2),
            ("3e2", 1),
            ("3.0e2", 2),
            ("3.00e-2", 3),
            ("+3.0E+2", 2),
            ("-0.0030", 2),
        ] {
            assert_eq!(
                parse_numeric_answer(text).unwrap().significant_figures,
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn significant_figures_define_zero_and_trailing_zero_rules() {
        // All-zero mantissas keep the zeros written after the decimal point.
        assert_eq!(parse_numeric_answer("0").unwrap().significant_figures, 1);
        assert_eq!(parse_numeric_answer("0.0").unwrap().significant_figures, 1);
        assert_eq!(parse_numeric_answer("0.00").unwrap().significant_figures, 2);
        // Integer trailing zeros are counted as written; use exponent notation
        // when a value such as 300 should be one significant figure.
        assert_eq!(parse_numeric_answer("100").unwrap().significant_figures, 3);
        assert_eq!(parse_numeric_answer("1e2").unwrap().significant_figures, 1);
        // Fractions keep the more precise side, as before.
        assert_eq!(parse_numeric_answer("3/2").unwrap().significant_figures, 1);
        assert_eq!(
            parse_numeric_answer("3.0/2.00")
                .unwrap()
                .significant_figures,
            3
        );
    }

    #[test]
    fn exponent_notation_normalizes_value_and_unit() {
        let parsed = parse_numeric_answer("3.00e-2m").unwrap();
        assert!((parsed.value - 0.03).abs() < 1e-12);
        assert_eq!(parsed.significant_figures, 3);
        assert_eq!(parsed.unit.as_deref(), Some("m"));
        assert!(!parsed.fraction);

        let parsed = parse_numeric_answer("1e2/2e1").unwrap();
        assert!((parsed.value - 5.0).abs() < 1e-12);
        assert_eq!(parsed.significant_figures, 1);
        assert!(parsed.fraction);
    }

    fn inspect_png(bytes: &[u8]) -> Result<(u32, u32), ValidationError> {
        inspect_image(bytes, "image/png", "asset")
    }

    fn encode_png(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        let pixels = vec![128u8; (width * height * 3) as usize];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn encode_jpeg(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        let pixels = vec![128u8; (width * height * 3) as usize];
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(12 + data.len());
        chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        chunk
    }

    /// A structurally valid PNG header that declares huge dimensions without
    /// carrying pixel data. The dimension check must run before decoding.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        bytes.extend(png_chunk(b"IHDR", &ihdr));
        // An empty zlib stream: enough for the header reader to accept the
        // file structure without allocating the declared pixel buffer.
        bytes.extend(png_chunk(
            b"IDAT",
            &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        ));
        bytes.extend(png_chunk(b"IEND", &[]));
        bytes
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn images_with_an_excessive_pixel_count_are_rejected_before_decoding() {
        let bytes = png_header(6_000, 5_000);
        let error = inspect_png(&bytes).unwrap_err();
        assert!(error.message.contains("画素数"), "{error}");
    }

    #[test]
    fn valid_png_and_jpeg_decode_and_report_their_real_size() {
        assert_eq!(inspect_png(&encode_png(4, 3)).unwrap(), (4, 3));
        assert_eq!(
            inspect_image(&encode_jpeg(4, 3), "image/jpeg", "asset").unwrap(),
            (4, 3)
        );
    }

    #[test]
    fn header_only_and_truncated_png_are_rejected() {
        let png = encode_png(4, 3);
        let header_only = png[..24].to_vec();
        assert!(inspect_png(&header_only).is_err());

        let without_iend = &png[..png.len() - 12];
        assert!(inspect_png(without_iend).is_err());

        let truncated = &png[..png.len() / 2];
        assert!(inspect_png(truncated).is_err());
    }

    #[test]
    fn corrupt_png_crc_and_chunks_are_rejected() {
        let png = encode_png(4, 3);
        let mut bad_crc = png.clone();
        bad_crc[29] ^= 0xFF; // first byte of the IHDR CRC
        assert!(inspect_png(&bad_crc).is_err());

        let mut bad_chunk = png.clone();
        let idat = find_bytes(&bad_chunk, b"IDAT").unwrap();
        bad_chunk[idat] = b'X';
        assert!(inspect_png(&bad_chunk).is_err());
    }

    #[test]
    fn truncated_or_malformed_jpeg_is_rejected() {
        let jpeg = encode_jpeg(16, 16);
        assert!(inspect_image(&jpeg[..jpeg.len() * 2 / 3], "image/jpeg", "asset").is_err());
        // Removing only the EOI marker still leaves an incomplete file.
        assert!(inspect_image(&jpeg[..jpeg.len() - 2], "image/jpeg", "asset").is_err());

        let mut bad_segment = jpeg.clone();
        if bad_segment.len() > 6 && bad_segment[2] == 0xFF {
            bad_segment[4] = 0;
            bad_segment[5] = 1;
        }
        assert!(inspect_image(&bad_segment, "image/jpeg", "asset").is_err());
    }

    #[test]
    fn declared_media_type_must_match_the_bytes() {
        let png = encode_png(4, 3);
        let error = inspect_image(&png, "image/jpeg", "asset").unwrap_err();
        assert!(error.message.contains("一致しません"), "{error}");

        let jpeg = encode_jpeg(4, 3);
        let error = inspect_image(&jpeg, "image/png", "asset").unwrap_err();
        assert!(error.message.contains("一致しません"), "{error}");

        assert!(inspect_png(b"not an image").is_err());
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
            r##"<svg width="10" height="10"><iframe src="https://example.com"/></svg>"##,
            r##"<svg width="10" height="10"><object data="https://example.com"/></svg>"##,
            r##"<svg width="10" height="10"><embed src="https://example.com"/></svg>"##,
            r##"<svg width="10" height="10"><rect onload="alert(1)"/></svg>"##,
            r##"<svg width="10" height="10"><rect style="fill:red"/></svg>"##,
            r##"<svg width="10" height="10"><style>rect{fill:red}</style></svg>"##,
            r##"<svg width="10" height="10"><rect fill="url(http://x)"/></svg>"##,
            r##"<svg width="10" height="10"><foreignObject/></svg>"##,
            r##"<svg width="10" height="10"><animate/></svg>"##,
            r##"<?xml version="1.0"?><!DOCTYPE svg><svg width="10" height="10"/>"##,
            r##"<!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><svg width="10" height="10"/>"##,
        ] {
            assert!(inspect(svg).is_err(), "should reject: {svg}");
        }
    }

    #[test]
    fn svg_href_obfuscation_cannot_bypass_structural_validation() {
        for svg in [
            r##"<svg width="10" height="10"><rect href = "https://example.invalid/a.svg"/></svg>"##,
            "<svg width=\"10\" height=\"10\"><rect\nhref\n=\n\"https://example.invalid/a.svg\"/></svg>",
            r##"<svg width="10" height="10"><rect HREF="https://example.invalid/a.svg"/></svg>"##,
            r##"<svg width="10" height="10" xmlns:xlink="http://www.w3.org/1999/xlink"><rect xlink:href="https://example.invalid/a.svg"/></svg>"##,
            r##"<svg width="10" height="10"><rect onload = "alert(1)"/></svg>"##,
            r##"<svg width="10" height="10"><rect ONLOAD = "alert(1)"/></svg>"##,
            r##"<svg width="10" height="10"><rect fill="java&#115;cript:alert(1)"/></svg>"##,
            r##"<svg width="10" height="10"><rect fill="url( https://example.invalid/a.svg )"/></svg>"##,
        ] {
            assert!(inspect(svg).is_err(), "should reject: {svg}");
        }
    }

    #[test]
    fn svg_rejects_invalid_namespaces_syntax_and_disallowed_attributes() {
        for svg in [
            r##"<svg xmlns="http://example.invalid/svg" width="10" height="10"/>"##,
            r##"<svg width="10" height="10"><rect class="x"/></svg>"##,
            r##"<svg width="10" height="10"><rect filter="none"/></svg>"##,
            r##"<svg width="10" height="10"><rect/><g></svg>"##,
            r##"<html width="10" height="10"/>"##,
        ] {
            assert!(inspect(svg).is_err(), "should reject: {svg}");
        }
    }

    #[test]
    fn svg_rejects_excessive_size_and_nesting() {
        assert!(inspect(&" ".repeat(MAX_SVG_BYTES + 1)).is_err());

        let mut deep = String::from(r##"<svg width="10" height="10">"##);
        for _ in 0..=MAX_SVG_DEPTH {
            deep.push_str("<g>");
        }
        for _ in 0..=MAX_SVG_DEPTH {
            deep.push_str("</g>");
        }
        deep.push_str("</svg>");
        let error = inspect(&deep).unwrap_err();
        assert!(error.message.contains("ネスト"), "{error}");
    }

    #[test]
    fn base64_encoding_round_trips_arbitrary_bytes() {
        for bytes in [
            Vec::new(),
            vec![0u8],
            vec![0, 1],
            vec![0, 1, 2],
            (0..=255u8).collect::<Vec<_>>(),
        ] {
            let encoded = encode_base64(&bytes);
            assert_eq!(decode_base64(&encoded).unwrap(), bytes);
        }
    }

    #[test]
    fn svg_allows_only_static_shapes_and_gradients() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" role="img">
          <title>t</title><desc>d</desc><g fill="#fff"><path d="M0 0L1 1"/><rect width="1" height="1"/>
          <circle cx="1" cy="1" r="1"/><ellipse cx="1" cy="1" rx="1" ry="1"/>
          <polygon points="0,0 1,1 0,1"/><polyline points="0,0 1,1"/><line x1="0" y1="0" x2="1" y2="1"/></g>
          <defs><linearGradient id="lg"><stop offset="0" stop-color="#fff"/></linearGradient>
          <radialGradient id="rg"><stop offset="1" stop-color="#000"/></radialGradient>
          <clipPath id="cp"><rect width="1" height="1"/></clipPath></defs>
          <rect width="1" height="1" fill="url(#lg)" clip-path="url(#cp)"/></svg>"##;
        assert_eq!(inspect(svg).unwrap(), (10, 10));
    }
}
