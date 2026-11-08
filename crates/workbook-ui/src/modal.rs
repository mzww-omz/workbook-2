use gpui_kit::component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, div, hsla, px};

use crate::{Modal, WorkbookApp};

impl WorkbookApp {
    pub(crate) fn render_modal(&self, modal: Modal, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (title, body, actions): (String, AnyElement, AnyElement) = match modal {
            Modal::Finish => (
                "学習を終了しますか？".to_string(),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("確定済みの回答は保存されています。")
                    .child("1分・10分の短期カードは期限を保ったまま残り、次回の復習で再開できます。")
                    .into_any_element(),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("modal-finish-cancel")
                            .label("続ける")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("modal-finish-confirm")
                            .label("終了する")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish_session(window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
            Modal::DeleteWorkbook { id, title } => (
                "教材を削除しますか？".to_string(),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("「{title}」を削除します。"))
                    .child("関連するFSRS状態とセッション項目も削除されます。削除前に安全用バックアップを自動作成します。")
                    .child("同じJSONの別コピーは削除されません。")
                    .into_any_element(),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("modal-delete-cancel")
                            .label("キャンセル")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("modal-delete-confirm")
                            .label("削除する")
                            .danger()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_delete(id.clone(), window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
            Modal::ImportJson {
                workbook,
                raw,
                source,
            } => {
                let note_count = workbook.notes.len();
                let question_count = workbook.question_count();
                (
                    "教材をインポート".to_string(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(20.0)).child(workbook.title.clone()))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(theme.muted_foreground)
                                .child(if workbook.description.is_empty() {
                                    "説明なし".to_string()
                                } else {
                                    workbook.description.clone()
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "Note {note_count} · Question {question_count} · schemaVersion 1"
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child(source.clone()),
                        )
                        .child("独立したローカルコピーとして追加します。既存教材は上書きしません。")
                        .into_any_element(),
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("modal-import-cancel")
                                .label("キャンセル")
                                .secondary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.modal = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("modal-import-confirm")
                                .label("インポート")
                                .primary()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_import(raw.clone(), window, cx);
                                })),
                        )
                        .into_any_element(),
                )
            }
            Modal::Restore { path, name } => (
                "バックアップから復元".to_string(),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("{name} の内容に完全に置き換えます。"))
                    .child("現在の教材・進捗・設定は置き換わります。復元前の安全用バックアップを自動作成します。")
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(path.display().to_string()),
                    )
                    .into_any_element(),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("modal-restore-cancel")
                            .label("キャンセル")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("modal-restore-confirm")
                            .label("置き換える")
                            .danger()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_restore(path.clone(), window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
            Modal::Legacy { path, preview } => {
                let mut warnings = div().flex().flex_col().gap_1();
                for warning in &preview.warnings {
                    warnings = warnings.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(format!("・{warning}")),
                    );
                }
                (
                    "旧Workbookから移行".to_string(),
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(format!(
                            "旧v{} · 教材{} · Note{} · 問題{} · FSRS状態{} · 回答{}",
                            preview.schema_version,
                            preview.workbook_count,
                            preview.note_count,
                            preview.question_count,
                            preview.review_state_count,
                            preview.attempt_count
                        ))
                        .child(if preview.active_session {
                            "中断中のセッションも移行します。"
                        } else {
                            "旧データは読み取り専用で検査し、変換後のみ新アプリDBへ追加します。"
                        })
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child(path.display().to_string()),
                        )
                        .when(!preview.warnings.is_empty(), |column| {
                            column.child(warnings)
                        })
                        .into_any_element(),
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("modal-legacy-cancel")
                                .label("キャンセル")
                                .secondary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.modal = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("modal-legacy-confirm")
                                .label("移行する")
                                .primary()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_legacy(path.clone(), window, cx);
                                })),
                        )
                        .into_any_element(),
                )
            }
            Modal::ResetHistory => (
                "学習履歴をリセット".to_string(),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("FSRS状態・回答・セッションを削除します。教材と設定は残ります。")
                    .child("リセット前の完全バックアップを自動作成します。")
                    .into_any_element(),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("modal-reset-cancel")
                            .label("キャンセル")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("modal-reset-confirm")
                            .label("リセットする")
                            .danger()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_reset(window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.45))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .w(px(560.0))
                    .max_w(px(640.0))
                    .p_6()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .child(div().text_size(px(22.0)).child(title))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .line_height(px(22.0))
                            .text_color(theme.foreground)
                            .child(body),
                    )
                    .child(div().flex().justify_end().child(actions)),
            )
            .into_any_element()
    }
}
