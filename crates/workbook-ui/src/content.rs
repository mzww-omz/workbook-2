use gpui_kit::component::{Theme, text::TextView};
use gpui_kit::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use workbook_domain::{Content, ContentBlock, ImageFit};

pub(crate) fn render_content(prefix: &str, content: &Content, theme: &Theme) -> AnyElement {
    match content {
        Content::Text(text) => markdown(prefix, text, theme),
        Content::Blocks(blocks) => {
            let mut column = div().flex().flex_col().gap_4();
            for (index, block) in blocks.iter().enumerate() {
                column = column.child(render_block(&format!("{prefix}-{index}"), block, theme));
            }
            column.into_any_element()
        }
    }
}

fn markdown(prefix: &str, text: &str, _theme: &Theme) -> AnyElement {
    TextView::markdown(format!("{prefix}-markdown"), text.to_string())
        .text_size(px(17.0))
        .line_height(px(27.0))
        .into_any_element()
}

fn render_block(prefix: &str, block: &ContentBlock, theme: &Theme) -> AnyElement {
    match block {
        ContentBlock::Markdown { text } => markdown(prefix, text, theme),
        ContentBlock::Code { text, language } => {
            let label = language.clone().unwrap_or_else(|| "code".to_string());
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    TextView::markdown(format!("{prefix}-code"), format!("```\n{text}\n```"))
                        .text_size(px(13.0))
                        .line_height(px(20.0)),
                )
                .into_any_element()
        }
        ContentBlock::Math { text } => div()
            .p_3()
            .bg(theme.secondary)
            .font_family(theme.mono_font_family.clone())
            .text_size(px(14.0))
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
                .p_4()
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
                .child(div().text_size(px(14.0)).child(alt.clone()));
            if let Some(caption) = caption {
                box_ = box_.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(caption.clone()),
                );
            }
            box_.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(url.clone()),
            )
            .into_any_element()
        }
    }
}
