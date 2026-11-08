use gpui_kit::component::{Theme, text::TextView};
use gpui_kit::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use workbook_domain::{Content, ContentBlock, ImageFit};

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
        math_size: 14.0,
        image_max_height: None,
    };

    pub(crate) const OPTION: Self = Self {
        text_size: 15.0,
        line_height: 23.0,
        code_size: 12.0,
        code_line_height: 18.0,
        math_size: 13.0,
        image_max_height: Some(120.0),
    };
}

pub(crate) fn render_content(prefix: &str, content: &Content, theme: &Theme) -> AnyElement {
    render_content_scaled(prefix, content, theme, ContentScale::BODY)
}

/// Compact variant for Ordering, Matching, and Labeling rows. It reuses the
/// same block renderer as prompts, explanations, and hints; question formats
/// never get their own copy of the content logic.
pub(crate) fn render_content_compact(prefix: &str, content: &Content, theme: &Theme) -> AnyElement {
    render_content_scaled(prefix, content, theme, ContentScale::OPTION)
}

fn render_content_scaled(
    prefix: &str,
    content: &Content,
    theme: &Theme,
    scale: ContentScale,
) -> AnyElement {
    match content {
        Content::Text(text) => markdown(prefix, text, scale),
        Content::Blocks(blocks) => {
            let mut column = div().flex().flex_col().gap_2().min_w(px(0.0));
            for (index, block) in blocks.iter().enumerate() {
                column = column.child(render_block(
                    &format!("{prefix}-{index}"),
                    block,
                    theme,
                    scale,
                ));
            }
            column.into_any_element()
        }
    }
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
) -> AnyElement {
    match block {
        ContentBlock::Markdown { text } => markdown(prefix, text, scale),
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
            .font_family(theme.mono_font_family.clone())
            .text_size(px(scale.math_size))
            .overflow_hidden()
            .child(text.clone())
            .into_any_element(),
        ContentBlock::Image {
            url,
            alt,
            caption,
            fit,
        } => {
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
                        .child(match fit {
                            ImageFit::Contain => "画像 · contain · オフライン表示",
                            ImageFit::Cover => "画像 · cover · オフライン表示",
                        }),
                )
                .child(div().text_size(px(scale.text_size)).child(alt.clone()));
            if let Some(caption) = caption {
                box_ = box_.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(caption.clone()),
                );
            }
            if let Some(max_height) = scale.image_max_height {
                box_ = box_.max_h(px(max_height)).overflow_hidden();
            }
            box_.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .max_w_full()
                    .overflow_hidden()
                    .child(url.clone()),
            )
            .into_any_element()
        }
    }
}
