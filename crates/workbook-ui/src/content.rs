use std::cell::RefCell;
use std::sync::Arc;

use gpui_kit::component::{Theme, text::TextView};
use gpui_kit::{
    AnyElement, Image, ImageFormat, ImageSource, IntoElement, ObjectFit, ParentElement, Styled,
    StyledImage, div, img, px,
};
use workbook_domain::{Content, ContentBlock, ImageBlock, ImageFit, ImageSource as BlockSource};
use workbook_service::AssetView;

use crate::image_cache::{ImageCache, ImageKey, decoded_bytes};

/// Text metrics used by the shared content renderer. Question bodies use the
/// normal scale; answer options use the compact scale so rich options still fit
/// in a list row.
#[derive(Clone, Copy)]
pub(crate) struct ContentScale {
    pub text_size: f32,
    pub line_height: f32,
    pub code_size: f32,
    pub code_line_height: f32,
    pub math_size: f32,
    /// Compact rows (answer options, matching/labeling cells) use smaller
    /// scripts and inline-style limits so a math expression stays inside the
    /// row.
    pub compact: bool,
    /// Compact option rows bound tall image placeholders so a list row stays
    /// readable. Body content leaves the placeholder unbounded.
    pub image_max_height: Option<f32>,
}

impl ContentScale {
    pub(crate) const BODY: Self = Self {
        text_size: 17.0,
        line_height: 27.0,
        code_size: 13.0,
        code_line_height: 20.0,
        math_size: 16.0,
        compact: false,
        image_max_height: None,
    };

    pub(crate) const OPTION: Self = Self {
        text_size: 15.0,
        line_height: 23.0,
        code_size: 12.0,
        code_line_height: 18.0,
        math_size: 14.0,
        compact: true,
        image_max_height: Some(120.0),
    };
}

/// Bundled images available to the current study view, plus the process-wide
/// decoded-image cache. Content blocks look images up by asset ID; they never
/// read SQLite or the filesystem from the renderer.
pub(crate) struct ContentImages<'a> {
    assets: &'a [AssetView],
    cache: &'a RefCell<ImageCache>,
    workbook_id: &'a str,
}

impl<'a> ContentImages<'a> {
    pub(crate) fn new(
        assets: &'a [AssetView],
        cache: &'a RefCell<ImageCache>,
        workbook_id: &'a str,
    ) -> Self {
        Self {
            assets,
            cache,
            workbook_id,
        }
    }

    pub(crate) fn asset(&self, id: &str) -> Option<&'a AssetView> {
        self.assets.iter().find(|asset| asset.id == id)
    }

    /// Returns the decoded image for this workbook asset, reusing the LRU
    /// cache entry. The key includes a content fingerprint, so a re-imported
    /// asset with the same ID but new bytes is treated as a different image.
    pub(crate) fn image(&self, asset: &AssetView) -> Arc<Image> {
        let key = ImageKey::new(self.workbook_id, &asset.id, asset.fingerprint);
        let estimate = decoded_bytes(asset.width, asset.height);
        self.cache
            .borrow_mut()
            .get_or_insert_with(key, estimate, || {
                let format = match asset.media_type.as_str() {
                    "image/jpeg" => ImageFormat::Jpeg,
                    "image/svg+xml" => ImageFormat::Svg,
                    _ => ImageFormat::Png,
                };
                Arc::new(Image::from_bytes(format, asset.bytes.as_ref().clone()))
            })
    }

    /// Shared image element used by rich content and by Hotspot/Labeling.
    pub(crate) fn render_asset_div(
        &self,
        asset: &AssetView,
        fit: ImageFit,
        max_width: f32,
        max_height: f32,
    ) -> gpui_kit::Div {
        let (width, height) =
            crate::ui_logic::image_box(asset.width, asset.height, fit, max_width, max_height);
        let image = self.image(asset);
        let alt = asset.alt.clone();
        let object_fit = match fit {
            ImageFit::Contain => ObjectFit::Contain,
            ImageFit::Cover => ObjectFit::Cover,
        };
        div()
            .relative()
            .w(px(width))
            .h(px(height))
            .max_w_full()
            .overflow_hidden()
            .child(
                img(ImageSource::Image(image))
                    .w_full()
                    .h_full()
                    .object_fit(object_fit)
                    .with_fallback(move || {
                        div()
                            .flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .p_4()
                            .text_size(px(13.0))
                            .child(format!("画像を表示できません: {alt}"))
                            .into_any_element()
                    }),
            )
    }

    pub(crate) fn render_asset(
        &self,
        asset: &AssetView,
        fit: ImageFit,
        max_width: f32,
        max_height: f32,
    ) -> AnyElement {
        self.render_asset_div(asset, fit, max_width, max_height)
            .into_any_element()
    }
}

pub(crate) fn render_content(
    prefix: &str,
    content: &Content,
    theme: &Theme,
    images: &ContentImages<'_>,
) -> AnyElement {
    render_content_scaled(prefix, content, theme, ContentScale::BODY, images)
}

/// Compact variant for Ordering, Matching, Labeling, and choice rows. It
/// reuses the same block renderer as prompts, explanations, and hints; question
/// formats never get their own copy of the content logic.
pub(crate) fn render_content_compact(
    prefix: &str,
    content: &Content,
    theme: &Theme,
    images: &ContentImages<'_>,
) -> AnyElement {
    render_content_scaled(prefix, content, theme, ContentScale::OPTION, images)
}

fn render_content_scaled(
    prefix: &str,
    content: &Content,
    theme: &Theme,
    scale: ContentScale,
    images: &ContentImages<'_>,
) -> AnyElement {
    match content {
        Content::Text(text) => render_text(prefix, text, theme, scale),
        Content::Blocks(blocks) => {
            let mut column = div().flex().flex_col().gap_2().min_w(px(0.0));
            for (index, block) in blocks.iter().enumerate() {
                column = column.child(render_block(
                    &format!("{prefix}-{index}"),
                    block,
                    theme,
                    scale,
                    images,
                ));
            }
            column.into_any_element()
        }
    }
}

/// Renders text, splitting inline math (`$...$` and `\(...\)`) out of the
/// Markdown run. Text without math keeps the original single `TextView` so
/// heading/list/paragraph layout is unchanged.
fn render_text(prefix: &str, text: &str, theme: &Theme, scale: ContentScale) -> AnyElement {
    let segments = crate::math::split_inline_math(text);
    let has_math = segments
        .iter()
        .any(|segment| matches!(segment, crate::math::InlineSegment::Math(_)));
    if !has_math {
        return markdown(prefix, text, scale);
    }
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .min_w(px(0.0))
        .max_w_full();
    for (index, segment) in segments.iter().enumerate() {
        match segment {
            crate::math::InlineSegment::Text(text) => {
                row = row.child(div().flex_shrink_1().min_w(px(0.0)).child(markdown(
                    &format!("{prefix}-text-{index}"),
                    text,
                    scale,
                )));
            }
            crate::math::InlineSegment::Math(text) => {
                row = row.child(crate::math::render_math(
                    text,
                    theme,
                    scale.math_size,
                    crate::math::MathMode::Inline,
                ));
            }
        }
    }
    row.into_any_element()
}

fn markdown(prefix: &str, text: &str, scale: ContentScale) -> AnyElement {
    TextView::markdown(format!("{prefix}-markdown"), text.to_string())
        .text_size(px(scale.text_size))
        .line_height(px(scale.line_height))
        .into_any_element()
}

fn render_block(
    prefix: &str,
    block: &ContentBlock,
    theme: &Theme,
    scale: ContentScale,
    images: &ContentImages<'_>,
) -> AnyElement {
    match block {
        ContentBlock::Markdown { text } => render_text(prefix, text, theme, scale),
        ContentBlock::Code { text, language } => {
            let label = language.clone().unwrap_or_else(|| "code".to_string());
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    TextView::markdown(format!("{prefix}-code"), format!("```\n{text}\n```"))
                        .text_size(px(scale.code_size))
                        .line_height(px(scale.code_line_height)),
                )
                .into_any_element()
        }
        ContentBlock::Math { text } => div()
            .p_2()
            .bg(theme.secondary)
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .max_w_full()
            .child(crate::math::render_math(
                text,
                theme,
                scale.math_size,
                if scale.compact {
                    crate::math::MathMode::Inline
                } else {
                    crate::math::MathMode::Display
                },
            ))
            .into_any_element(),
        ContentBlock::Image(image) => render_image(prefix, image, theme, scale, images),
    }
}

fn render_image(
    prefix: &str,
    image: &ImageBlock,
    theme: &Theme,
    scale: ContentScale,
    images: &ContentImages<'_>,
) -> AnyElement {
    let mut box_ = div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .max_w_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme.muted_foreground)
                .child(match image.fit {
                    ImageFit::Contain => "画像 · contain",
                    ImageFit::Cover => "画像 · cover",
                }),
        );
    match &image.source {
        BlockSource::Asset(id) => match images.asset(id) {
            Some(asset) => {
                let max_height = scale.image_max_height.unwrap_or(480.0);
                box_ = box_.child(images.render_asset(asset, image.fit, 640.0, max_height));
            }
            None => {
                box_ = box_.child(
                    div()
                        .p_2()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("画像を表示できません（asset={id}）: {}", image.alt)),
                );
            }
        },
        BlockSource::Url(url) => {
            box_ = box_.child(
                div()
                    .p_2()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(format!("外部画像は表示しません（HTTPS）: {url}")),
            );
        }
    }
    box_ = box_.child(
        div()
            .text_size(px(11.0))
            .text_color(theme.muted_foreground)
            .child(image.alt.clone()),
    );
    if let Some(caption) = &image.caption {
        box_ = box_.child(
            div()
                .text_size(px(12.0))
                .text_color(theme.muted_foreground)
                .child(caption.clone()),
        );
    }
    let _ = prefix;
    box_.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_scale_shrinks_math_and_bounds_images() {
        let body = ContentScale::BODY;
        let option = ContentScale::OPTION;
        assert!(body.math_size > option.math_size);
        assert!(!body.compact);
        assert!(option.compact);
        assert_eq!(body.image_max_height, None);
        assert!(option.image_max_height.is_some());
    }
}
