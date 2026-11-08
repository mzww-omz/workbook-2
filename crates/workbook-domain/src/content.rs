use std::collections::BTreeSet;

use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use url::Url;

pub const MAX_CONTENT_CHARS: usize = 100_000;
pub const MAX_BLOCKS: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Contain,
    Cover,
}

/// Where an image block gets its pixels from.
///
/// The two variants are explicit in the data model instead of being inferred
/// from a string prefix. `url` keeps the schema-v1 HTTPS form; `asset` refers
/// to an image bundled with the workbook and stored in SQLite at import time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSource {
    Url(String),
    Asset(String),
}

impl ImageSource {
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            Self::Asset(_) => None,
        }
    }

    pub fn asset(&self) -> Option<&str> {
        match self {
            Self::Asset(id) => Some(id),
            Self::Url(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageBlock {
    pub source: ImageSource,
    pub alt: String,
    pub caption: Option<String>,
    pub fit: ImageFit,
}

impl ImageBlock {
    pub fn url(url: impl Into<String>, alt: impl Into<String>) -> Self {
        Self {
            source: ImageSource::Url(url.into()),
            alt: alt.into(),
            caption: None,
            fit: ImageFit::Contain,
        }
    }

    pub fn asset(asset: impl Into<String>, alt: impl Into<String>) -> Self {
        Self {
            source: ImageSource::Asset(asset.into()),
            alt: alt.into(),
            caption: None,
            fit: ImageFit::Contain,
        }
    }

    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    pub fn with_fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }
}

impl Serialize for ImageBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(4))?;
        match &self.source {
            ImageSource::Url(url) => map.serialize_entry("url", url)?,
            ImageSource::Asset(asset) => map.serialize_entry("asset", asset)?,
        }
        map.serialize_entry("alt", &self.alt)?;
        if let Some(caption) = &self.caption {
            map.serialize_entry("caption", caption)?;
        }
        map.serialize_entry("fit", &self.fit)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for ImageBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawImage {
            #[serde(default)]
            url: Option<String>,
            #[serde(default)]
            asset: Option<String>,
            alt: String,
            #[serde(default)]
            caption: Option<String>,
            #[serde(default)]
            fit: ImageFit,
        }

        let raw = RawImage::deserialize(deserializer)?;
        let source = match (raw.url, raw.asset) {
            (Some(url), None) => ImageSource::Url(url),
            (None, Some(asset)) => ImageSource::Asset(asset),
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "image ブロックの url と asset はどちらか一方だけを指定してください",
                ));
            }
            (None, None) => {
                return Err(D::Error::custom(
                    "image ブロックには url または asset が必要です",
                ));
            }
        };
        Ok(Self {
            source,
            alt: raw.alt,
            caption: raw.caption,
            fit: raw.fit,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentBlock {
    Markdown {
        text: String,
    },
    Image(ImageBlock),
    Math {
        text: String,
    },
    Code {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{path}: {message}")]
pub struct ContentError {
    pub path: String,
    pub message: String,
}

impl ContentError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl Content {
    /// Structural validation. Image asset references are checked for syntax
    /// here; [`Self::validate_with_assets`] additionally verifies the schema
    /// version and that the referenced asset exists in the workbook.
    pub fn validate(&self, path: &str) -> Result<(), ContentError> {
        self.validate_inner(path, None)
    }

    pub fn validate_with_assets(
        &self,
        path: &str,
        schema_version: u32,
        asset_ids: &BTreeSet<String>,
    ) -> Result<(), ContentError> {
        self.validate_inner(path, Some((schema_version, asset_ids)))
    }

    fn validate_inner(
        &self,
        path: &str,
        assets: Option<(u32, &BTreeSet<String>)>,
    ) -> Result<(), ContentError> {
        match self {
            Content::Text(text) => validate_text(text, path),
            Content::Blocks(blocks) => {
                if blocks.is_empty() {
                    return Err(ContentError::new(path, "内容を空にできません"));
                }
                if blocks.len() > MAX_BLOCKS {
                    return Err(ContentError::new(
                        path,
                        format!("ブロックは{MAX_BLOCKS}個以下にしてください"),
                    ));
                }
                for (i, block) in blocks.iter().enumerate() {
                    block.validate(&format!("{path}[{i}]"), assets)?;
                }
                Ok(())
            }
        }
    }

    /// Collects every bundled asset ID referenced by this content.
    pub fn referenced_asset_ids(&self, output: &mut BTreeSet<String>) {
        if let Content::Blocks(blocks) = self {
            for block in blocks {
                if let ContentBlock::Image(ImageBlock {
                    source: ImageSource::Asset(asset),
                    ..
                }) = block
                {
                    output.insert(asset.clone());
                }
            }
        }
    }

    pub fn plain_text(&self) -> String {
        match self {
            Content::Text(text) => markdown_to_plain(text),
            Content::Blocks(blocks) => blocks
                .iter()
                .map(ContentBlock::plain_text)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(text) => Some(text),
            Content::Blocks(_) => None,
        }
    }
}

impl ContentBlock {
    fn validate(
        &self,
        path: &str,
        assets: Option<(u32, &BTreeSet<String>)>,
    ) -> Result<(), ContentError> {
        match self {
            ContentBlock::Markdown { text } | ContentBlock::Math { text } => {
                validate_text(text, path)
            }
            ContentBlock::Image(image) => {
                if image.alt.trim().is_empty() {
                    return Err(ContentError::new(
                        format!("{path}.alt"),
                        "代替テキストが必要です",
                    ));
                }
                if image.alt.chars().count() > MAX_CONTENT_CHARS {
                    return Err(ContentError::new(
                        format!("{path}.alt"),
                        "代替テキストが長すぎます",
                    ));
                }
                if let Some(caption) = &image.caption
                    && caption.chars().count() > MAX_CONTENT_CHARS
                {
                    return Err(ContentError::new(
                        format!("{path}.caption"),
                        "説明が長すぎます",
                    ));
                }
                match &image.source {
                    ImageSource::Url(url) => validate_https_url(url, &format!("{path}.url")),
                    ImageSource::Asset(asset) => {
                        crate::question_format::validate_asset_id(asset, &format!("{path}.asset"))
                            .map_err(|error| ContentError::new(error.path, error.message))?;
                        if let Some((schema_version, asset_ids)) = assets {
                            if schema_version < 2 {
                                return Err(ContentError::new(
                                    format!("{path}.asset"),
                                    "教材同梱画像は schemaVersion 2 でのみ使用できます",
                                ));
                            }
                            if !asset_ids.contains(asset) {
                                return Err(ContentError::new(
                                    format!("{path}.asset"),
                                    "assets に存在しない画像IDです",
                                ));
                            }
                        }
                        Ok(())
                    }
                }
            }
            ContentBlock::Code { text, language } => {
                validate_text(text, path)?;
                if let Some(language) = language
                    && language.chars().count() > 40
                {
                    return Err(ContentError::new(
                        format!("{path}.language"),
                        "言語名は40文字以下にしてください",
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn plain_text(&self) -> String {
        match self {
            ContentBlock::Markdown { text } => markdown_to_plain(text),
            ContentBlock::Image(image) => match &image.caption {
                Some(caption) => format!("{}\n{caption}", image.alt),
                None => image.alt.clone(),
            },
            ContentBlock::Math { text } => text.clone(),
            ContentBlock::Code { text, .. } => text.clone(),
        }
    }
}

fn validate_text(text: &str, path: &str) -> Result<(), ContentError> {
    if text.trim().is_empty() {
        return Err(ContentError::new(path, "内容を空にできません"));
    }
    if text.chars().count() > MAX_CONTENT_CHARS {
        return Err(ContentError::new(
            path,
            format!("内容は{MAX_CONTENT_CHARS}文字以下にしてください"),
        ));
    }
    Ok(())
}

pub fn validate_https_url(value: &str, path: &str) -> Result<(), ContentError> {
    let parsed =
        Url::parse(value).map_err(|_| ContentError::new(path, "HTTPS URLを指定してください"))?;
    if parsed.scheme() != "https" {
        return Err(ContentError::new(path, "HTTPS URLを指定してください"));
    }
    if parsed.host_str().unwrap_or_default().is_empty() {
        return Err(ContentError::new(path, "URLにホスト名がありません"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ContentError::new(path, "認証情報を含むURLは使用できません"));
    }
    Ok(())
}

/// Small, deliberately conservative Markdown-to-text conversion used for
/// previews and labels. The UI renders block structure separately.
pub fn markdown_to_plain(markdown: &str) -> String {
    let mut output = String::with_capacity(markdown.len());
    let mut in_code = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            output.push_str(line);
            output.push('\n');
            continue;
        }
        let mut line = trimmed.to_string();
        if line.starts_with('#') {
            line = line.trim_start_matches('#').trim_start().to_string();
        }
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            line = format!("• {rest}");
        }
        line = line
            .replace("**", "")
            .replace("__", "")
            .replace('`', "")
            .replace("~~", "");
        output.push_str(&line);
        output.push('\n');
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_urls_must_be_https_without_credentials() {
        assert!(validate_https_url("https://example.com/a.png", "p").is_ok());
        assert!(validate_https_url("http://example.com/a.png", "p").is_err());
        assert!(validate_https_url("https://user:pass@example.com/a.png", "p").is_err());
        assert!(validate_https_url("javascript:alert(1)", "p").is_err());
    }

    #[test]
    fn markdown_preview_drops_decorative_markers() {
        assert_eq!(
            markdown_to_plain("## 見出し\n**強い** `code`"),
            "見出し\n強い code"
        );
    }

    #[test]
    fn image_blocks_distinguish_url_and_asset_sources() {
        let url: ContentBlock =
            serde_json::from_str(r#"{"type":"image","url":"https://example.com/a.png","alt":"A"}"#)
                .unwrap();
        let ContentBlock::Image(url) = url else {
            panic!("image block expected");
        };
        assert_eq!(url.source.asset(), None);
        assert_eq!(url.source.url(), Some("https://example.com/a.png"));

        let asset: ContentBlock =
            serde_json::from_str(r#"{"type":"image","asset":"cell-diagram","alt":"細胞"}"#)
                .unwrap();
        let ContentBlock::Image(asset) = asset else {
            panic!("image block expected");
        };
        assert_eq!(asset.source.asset(), Some("cell-diagram"));
        assert_eq!(asset.source.url(), None);

        let serialized = serde_json::to_value(ContentBlock::Image(asset)).unwrap();
        assert_eq!(serialized["asset"], "cell-diagram");
        assert!(serialized.get("url").is_none());
    }

    #[test]
    fn image_blocks_reject_both_or_neither_source() {
        for raw in [
            r#"{"type":"image","url":"https://example.com/a.png","asset":"x","alt":"A"}"#,
            r#"{"type":"image","alt":"A"}"#,
        ] {
            assert!(serde_json::from_str::<ContentBlock>(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn asset_references_require_schema_two_and_an_existing_asset() {
        let content: Content =
            serde_json::from_str(r#"[{"type":"image","asset":"cell","alt":"細胞"}]"#).unwrap();
        let mut assets = BTreeSet::new();
        assets.insert("cell".to_string());

        content.validate("content").unwrap();
        content.validate_with_assets("content", 2, &assets).unwrap();
        let error = content
            .validate_with_assets("content", 1, &assets)
            .unwrap_err();
        assert!(error.message.contains("schemaVersion 2"), "{error}");

        let error = content
            .validate_with_assets("content", 2, &BTreeSet::new())
            .unwrap_err();
        assert!(error.message.contains("存在しない"), "{error}");
    }

    #[test]
    fn referenced_asset_ids_include_nested_blocks_only() {
        let content: Content = serde_json::from_str(
            r#"[{"type":"image","asset":"a","alt":"A"},{"type":"markdown","text":"x"},
                {"type":"image","url":"https://example.com/b.png","alt":"B"}]"#,
        )
        .unwrap();
        let mut ids = BTreeSet::new();
        content.referenced_asset_ids(&mut ids);
        assert_eq!(ids, BTreeSet::from(["a".to_string()]));
    }
}
