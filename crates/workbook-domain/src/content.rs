use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentBlock {
    Markdown {
        text: String,
    },
    Image {
        url: String,
        alt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        #[serde(default)]
        fit: ImageFit,
    },
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
    pub fn validate(&self, path: &str) -> Result<(), ContentError> {
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
                    block.validate(&format!("{path}[{i}]"))?;
                }
                Ok(())
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
    fn validate(&self, path: &str) -> Result<(), ContentError> {
        match self {
            ContentBlock::Markdown { text } | ContentBlock::Math { text } => {
                validate_text(text, path)
            }
            ContentBlock::Image {
                url, alt, caption, ..
            } => {
                if alt.trim().is_empty() {
                    return Err(ContentError::new(
                        format!("{path}.alt"),
                        "代替テキストが必要です",
                    ));
                }
                if alt.chars().count() > MAX_CONTENT_CHARS {
                    return Err(ContentError::new(
                        format!("{path}.alt"),
                        "代替テキストが長すぎます",
                    ));
                }
                if let Some(caption) = caption
                    && caption.chars().count() > MAX_CONTENT_CHARS
                {
                    return Err(ContentError::new(
                        format!("{path}.caption"),
                        "説明が長すぎます",
                    ));
                }
                validate_https_url(url, &format!("{path}.url"))
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
            ContentBlock::Image { alt, caption, .. } => match caption {
                Some(caption) => format!("{alt}\n{caption}"),
                None => alt.clone(),
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
}
