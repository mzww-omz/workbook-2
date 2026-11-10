use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use gpui_kit::component::{
    ActiveTheme, Disableable,
    button::{Button, ButtonVariants},
    input::Input,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, div, px, relative,
};
use workbook_domain::{
    Content, GradeStatus, Mode, Phase, PracticeStrategy, Question, QuestionType, Rating,
    RatingSource, ThemeSetting,
};
use workbook_service::{AssetView, MockItemStatus, MockResultView};

use crate::{NoticeKind, Route, WorkbookApp};

struct DragGhost {
    label: String,
}

impl Render for DragGhost {
    fn render(
        &mut self,
        _window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .px(px(12.0))
            .py(px(8.0))
            .border_1()
            .border_color(theme.accent)
            .bg(theme.background)
            .text_size(px(14.0))
            .child(self.label.clone())
    }
}

#[derive(Clone)]
struct OrderDrag {
    id: String,
    label: String,
}

#[derive(Clone)]
struct LabelDrag {
    id: String,
    label: String,
}

/// Shared row layout for rich answer options. The content itself is rendered by
/// the one common content renderer; this only adds selection markers and the
/// paired item shown in Matching.
fn rich_option_body(
    prefix: &str,
    leading: Option<&str>,
    content: &Content,
    trailing: Option<(&str, &Content)>,
    theme: &gpui_kit::component::Theme,
    images: &crate::content::ContentImages<'_>,
) -> AnyElement {
    let mut row = div().flex().items_center().gap_2().flex_1().min_w(px(0.0));
    if let Some(leading) = leading {
        row = row.child(
            div()
                .flex_none()
                .text_size(px(12.0))
                .text_color(theme.muted_foreground)
                .child(leading.to_string()),
        );
    }
    row = row.child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(
        crate::content::render_content_compact(prefix, content, theme, images),
    ));
    if let Some((arrow, trailing_content)) = trailing {
        row = row
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(arrow.to_string()),
            )
            .child(
                div()
                    .max_w(relative(0.5))
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(crate::content::render_content_compact(
                        &format!("{prefix}-trailing"),
                        trailing_content,
                        theme,
                        images,
                    )),
            );
    }
    row.into_any_element()
}

fn format_duration(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}秒")
    } else {
        format!("{}分{:02}秒", seconds / 60, seconds % 60)
    }
}

fn format_percent(correct: usize, graded: usize) -> String {
    if graded == 0 {
        "—".to_string()
    } else {
        format!("{:.0}%", correct as f64 / graded as f64 * 100.0)
    }
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::New => "新規",
        Mode::Review => "復習",
        Mode::Practice => "練習",
        Mode::Plan => "計画",
        Mode::Mock => "模試",
    }
}

fn format_datetime(value: DateTime<Utc>, timezone: &str) -> String {
    let tz = timezone.parse::<Tz>().unwrap_or(Tz::UTC);
    value
        .with_timezone(&tz)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

impl WorkbookApp {
    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(232.0))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.background)
            .px(px(24.0))
            .py(px(28.0));

        sidebar = sidebar
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::LIGHT)
                            .child("Workbook 2"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child("LOCAL · FSRS-7"),
                    ),
            )
            .child(div().h(px(36.0)));

        sidebar = sidebar
            .child(self.nav_item("nav-today", "今日", "⌘1", Route::Today, cx))
            .child(self.nav_item("nav-library", "教材", "⌘2", Route::Library, cx))
            .child(self.nav_item("nav-history", "履歴", "⌘3", Route::History, cx))
            .child(self.nav_item("nav-exams", "試験", "⌘4", Route::Exams, cx))
            .child(self.nav_item("nav-settings", "設定", "⌘,", Route::Settings, cx));

        if let Some(active) = self.snapshot.active.as_ref()
            && self.route != Route::Study
        {
            {
                let title = active
                    .workbook_title
                    .clone()
                    .unwrap_or_else(|| "学習中".to_string());
                sidebar = sidebar.child(
                    div()
                        .mt(px(28.0))
                        .p_3()
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child("中断中のセッション"),
                        )
                        .child(div().text_size(px(14.0)).child(title))
                        .child(
                            Button::new("sidebar-resume")
                                .label("再開する")
                                .primary()
                                .w_full()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.resume(window, cx);
                                })),
                        ),
                );
            }
        }

        sidebar = sidebar.child(div().flex_1());
        sidebar = sidebar
            .child(
                Button::new("sidebar-import")
                    .label("JSONを読み込む")
                    .secondary()
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.import_json_dialog(window, cx);
                    })),
            )
            .child(
                div()
                    .mt(px(10.0))
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("⌘O · オフライン"),
            );
        sidebar.into_any_element()
    }

    fn nav_item(
        &self,
        id: &'static str,
        label: &'static str,
        shortcut: &'static str,
        route: Route,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let active = self.route == route;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .py(px(10.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .border_l_3()
            .border_color(if active {
                theme.accent
            } else {
                theme.transparent
            })
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.go(route, window, cx);
            }))
            .child(
                div()
                    .text_size(px(15.0))
                    .text_color(if active {
                        theme.accent
                    } else {
                        theme.foreground
                    })
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(shortcut),
            )
            .into_any_element()
    }

    /// Bundled assets and the shared decode cache for the current study view.
    fn content_images(&self) -> crate::content::ContentImages<'_> {
        let assets = self
            .study
            .as_ref()
            .map(|view| view.assets.as_slice())
            .unwrap_or(&[]);
        let workbook_id = self
            .study
            .as_ref()
            .and_then(|view| view.workbook_id.as_deref())
            .unwrap_or("");
        crate::content::ContentImages::new(assets, &self.image_cache, workbook_id)
    }

    pub(crate) fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut container = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(1120.0))
            .mx_auto()
            .px(px(56.0))
            .py(px(44.0))
            .gap_6();

        if let Some(notice) = self.notice.clone() {
            container = container.child(self.render_notice(notice, cx));
        }

        let page = match self.route {
            Route::Today => self.render_today(cx),
            Route::Library => self.render_library(cx),
            Route::BookDetail => self.render_book_detail(cx),
            Route::History => self.render_history(cx),
            Route::Settings => self.render_settings(cx),
            Route::Exams => self.render_exams(cx),
            Route::ExamDetail => self.render_exam_detail(cx),
            Route::ExamEditor => self.render_exam_editor(cx),
            Route::Study => self.render_study(cx),
        };
        container.child(page).into_any_element()
    }

    pub(crate) fn render_notice(
        &self,
        notice: crate::Notice,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (color, background) = match notice.kind {
            NoticeKind::Info => (theme.accent, theme.accent.opacity(0.08)),
            NoticeKind::Success => (theme.success, theme.success.opacity(0.10)),
            NoticeKind::Error => (theme.danger, theme.danger.opacity(0.10)),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p_3()
            .border_l_3()
            .border_color(color)
            .bg(background)
            .child(div().text_size(px(14.0)).child(notice.text))
            .child(
                Button::new("notice-close")
                    .label("閉じる")
                    .text()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_notice(cx);
                    })),
            )
            .into_any_element()
    }

    fn page_header(
        &self,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(48.0))
                    .font_weight(FontWeight::LIGHT)
                    .line_height(px(54.0))
                    .child(title.into()),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.muted_foreground)
                    .child(subtitle.into()),
            )
            .into_any_element()
    }

    fn render_today(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let today = &self.snapshot.today;
        let mut page = div().flex().flex_col().gap_7();
        page = page.child(self.page_header(
            "今日",
            format!("{} · 学習日は午前4時に切り替わります", self.today_date()),
            cx,
        ));

        if let Some(active) = self.snapshot.active.as_ref() {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_4()
                    .border_1()
                    .border_color(theme.accent)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child("中断中のセッション"),
                            )
                            .child(
                                div().text_size(px(18.0)).child(
                                    active
                                        .workbook_title
                                        .clone()
                                        .unwrap_or_else(|| "学習".to_string()),
                                ),
                            ),
                    )
                    .child(
                        Button::new("today-resume")
                            .label("再開する")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.resume(window, cx);
                            })),
                    ),
            );
        }

        // The single primary action of Today. It deliberately prefers resuming
        // (handled above), then due reviews and short-term repeats, then new
        // content. Everything else in the screen is secondary information.
        let new_book = if today.new_remaining_today > 0 {
            self.snapshot
                .books
                .iter()
                .find(|book| book.new_available > 0)
                .map(|book| book.local_id.clone())
        } else {
            None
        };
        let (primary_label, primary_caption, primary_action): (
            &'static str,
            String,
            Option<(Mode, Option<String>)>,
        ) = if today.due > 0 || today.short_term_pending > 0 {
            (
                "今日の学習を始める",
                if today.short_term_pending > 0 {
                    format!(
                        "復習 {} 問と短期再学習 {} 問が待っています",
                        today.due, today.short_term_pending
                    )
                } else {
                    format!("期限が来た復習は {} 問です", today.due)
                },
                Some((Mode::Review, None)),
            )
        } else if let Some(book) = new_book.clone() {
            (
                "今日の学習を始める",
                format!(
                    "今日の復習は完了しています。新規を {} 問まで学習できます",
                    today.new_remaining_today
                ),
                Some((Mode::New, Some(book))),
            )
        } else {
            (
                "今日の学習を始める",
                "今日の復習は完了しています。範囲を選んで練習できます".to_string(),
                Some((Mode::Practice, None)),
            )
        };
        page = page.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_6()
                .py(px(18.0))
                .border_t_1()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div().flex().flex_col().gap_1().child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme.muted_foreground)
                            .child(primary_caption),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child("時間"),
                        )
                        .children(
                            [
                                ("3分", Some(3 * 60)),
                                ("5分", Some(5 * 60)),
                                ("10分", Some(10 * 60)),
                                ("20分", Some(20 * 60)),
                                ("制限なし", None),
                            ]
                            .into_iter()
                            .map(|(label, value)| {
                                let selected = self.budget_seconds == value;
                                Button::new(format!("today-budget-{label}"))
                                    .label(label)
                                    .when(selected, |button| button.primary())
                                    .when(!selected, |button| button.ghost())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.budget_seconds = value;
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            }),
                        )
                        .child(
                            Button::new("today-start-primary")
                                .label(primary_label)
                                .primary()
                                .disabled(primary_action.is_none())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let Some((mode, book)) = primary_action.clone() else {
                                        return;
                                    };
                                    if mode == Mode::Practice && book.is_none() {
                                        this.go(Route::Library, window, cx);
                                        return;
                                    }
                                    this.start_with_budget(mode, book, window, cx);
                                })),
                        ),
                ),
        );

        page = page.child(
            div()
                .flex()
                .flex_row()
                .gap_4()
                .items_stretch()
                .child(self.big_stat(
                    "復習",
                    today.due,
                    if today.short_term_pending > 0 {
                        format!(
                            "期限到来 {} · 短期再学習 {}（20分以内）",
                            today.due, today.short_term_pending
                        )
                    } else {
                        "期限が来た問題".to_string()
                    },
                    cx,
                ))
                .child(self.big_stat(
                    "新規",
                    today.new_available.min(today.new_remaining_today),
                    format!(
                        "未学習 {} · 本日の残り {}",
                        today.new_available, today.new_remaining_today
                    ),
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .justify_between()
                        .p_5()
                        .border_1()
                        .border_color(theme.border)
                        .flex_1()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.muted_foreground)
                                        .child("今日の回答"),
                                )
                                .child(div().text_size(px(24.0)).child(format!(
                                    "{} 回 · 正答 {} / {}",
                                    today.answers, today.correct, today.graded
                                )))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(theme.muted_foreground)
                                        .child(format!(
                                            "学習時間 {}",
                                            format_duration(today.elapsed_ms)
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(theme.muted_foreground)
                                        .child(if today.due > 0 {
                                            "期限到来中".to_string()
                                        } else {
                                            match today.next_due_at {
                                                Some(next) => format!(
                                                    "次の期限 {}",
                                                    format_datetime(next, &self.snapshot.timezone)
                                                ),
                                                None => "次の期限 なし".to_string(),
                                            }
                                        }),
                                ),
                        )
                        .child(
                            div().flex().gap_2().child(
                                Button::new("today-review")
                                    .label(if today.due == 0 && today.short_term_pending > 0 {
                                        "短期再学習を始める"
                                    } else {
                                        "今日の復習を始める"
                                    })
                                    .primary()
                                    .disabled(today.due == 0 && today.short_term_pending == 0)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start(Mode::Review, None, window, cx);
                                    })),
                            ),
                        ),
                ),
        );

        if today.due == 0 && today.short_term_pending > 0 {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_4()
                    .border_l_3()
                    .border_color(theme.accent)
                    .bg(theme.accent.opacity(0.08))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_size(px(15.0)).child("短期再学習が待っています"))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child("1分・10分ステップのカードです。Learn Aheadの範囲内で今すぐ再開できます。"),
                            ),
                    )
                    .child(
                        Button::new("today-shortterm-start")
                            .label("今すぐ再開")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start(Mode::Review, None, window, cx);
                            })),
                    ),
            );
        } else if today.due == 0 && today.short_term_pending == 0 {
            page = page.child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.muted_foreground)
                    .child(if today.new_available > 0 {
                        "今日の復習は完了しました。新規学習を始められます。"
                    } else {
                        "今日の復習は完了しました。"
                    }),
            );
        }
        page = page.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.muted_foreground)
                .child(format!(
                    "新規は1日{}問まで · 設定で変更できます（本日 {}問学習済み）",
                    today.new_limit,
                    today.new_limit as usize - today.new_remaining_today
                )),
        );

        if self.snapshot.books.is_empty() {
            page = page.child(self.empty_state(
                "まだ教材がありません",
                "JSON教材を読み込むか、同梱のサンプルから始めてください。",
                cx,
            ));
            page = page.child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        Button::new("today-import")
                            .label("JSONを読み込む")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_json_dialog(window, cx);
                            })),
                    )
                    .child(
                        Button::new("today-sample")
                            .label("サンプルで試す")
                            .secondary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_sample(window, cx);
                            })),
                    )
                    .child(
                        Button::new("today-sample-v2")
                            .label("拡張サンプル（5形式）")
                            .secondary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_expanded_sample(window, cx);
                            })),
                    ),
            );
            return page.into_any_element();
        }

        page = page.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.muted_foreground)
                .child("教材"),
        );
        let mut list = div().flex().flex_col();
        for book in &self.snapshot.books {
            let book_id = book.local_id.clone();
            let new_id = book.local_id.clone();
            let practice_id = book.local_id.clone();
            let review_id = book.local_id.clone();
            let title = book.title.clone();
            let due = book.due;
            let new_available = book.new_available;
            let short_term_pending = book.short_term_pending;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py(px(16.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .child(div().text_size(px(18.0)).child(title))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "{}問 · 学習済み {} · 復習 {} · 短期 {} · 新規 {}",
                                        book.total,
                                        book.learned,
                                        due,
                                        short_term_pending,
                                        new_available
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new(format!("today-review-{book_id}"))
                                    .label(if due == 0 && short_term_pending > 0 {
                                        "短期"
                                    } else {
                                        "復習"
                                    })
                                    .secondary()
                                    .disabled(due == 0 && short_term_pending == 0)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(
                                            Mode::Review,
                                            Some(review_id.clone()),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(format!("today-new-{new_id}"))
                                    .label("新規")
                                    .primary()
                                    .disabled(new_available == 0 || today.new_remaining_today == 0)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(Mode::New, Some(new_id.clone()), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("today-practice-{practice_id}"))
                                    .label("練習")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(
                                            Mode::Practice,
                                            Some(practice_id.clone()),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(format!("today-open-{book_id}"))
                                    .label("開く")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_book(book_id.clone(), cx);
                                    })),
                            ),
                    ),
            );
        }
        page.child(list).into_any_element()
    }

    fn big_stat(
        &self,
        label: &'static str,
        value: usize,
        caption: impl Into<SharedString>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .p_5()
            .border_1()
            .border_color(theme.border)
            .flex_1()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(52.0))
                    .font_weight(FontWeight::LIGHT)
                    .line_height(px(58.0))
                    .child(value.to_string()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(caption.into()),
            )
            .into_any_element()
    }

    fn empty_state(
        &self,
        title: &'static str,
        body: &'static str,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_6()
            .border_1()
            .border_color(theme.border)
            .child(div().text_size(px(20.0)).child(title))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.muted_foreground)
                    .child(body),
            )
            .into_any_element()
    }

    fn render_library(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let needle = self.search.to_lowercase();
        let books: Vec<_> = self
            .snapshot
            .books
            .iter()
            .filter(|book| {
                needle.is_empty()
                    || book.title.to_lowercase().contains(&needle)
                    || book.description.to_lowercase().contains(&needle)
                    || book
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();

        let mut page = div().flex().flex_col().gap_6();
        page = page.child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(self.page_header(
                    "教材",
                    "同じJSONを読み込んでも、進捗は独立したコピーになります。",
                    cx,
                ))
                .child(
                    Button::new("library-import")
                        .label("JSONを読み込む")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.import_json_dialog(window, cx);
                        })),
                ),
        );
        page = page.child(
            div().w_full().child(
                Input::new(&self.search_input)
                    .id("library-search")
                    .w_full()
                    .cleanable(true),
            ),
        );

        if books.is_empty() {
            page = page.child(if self.snapshot.books.is_empty() {
                self.empty_state(
                    "教材がありません",
                    "JSON教材を読み込むか、サンプルを追加してください。",
                    cx,
                )
            } else {
                self.empty_state("該当する教材がありません", "検索語を変えてください。", cx)
            });
            if self.snapshot.books.is_empty() {
                page = page.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("library-sample")
                                .label("サンプルで試す")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_sample(window, cx);
                                })),
                        )
                        .child(
                            Button::new("library-sample-v2")
                                .label("拡張サンプル（5形式）")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.import_expanded_sample(window, cx);
                                })),
                        ),
                );
            }
            return page.into_any_element();
        }

        let mut list = div().flex().flex_col();
        for book in &books {
            let id_open = book.local_id.clone();
            let id_new = book.local_id.clone();
            let id_review = book.local_id.clone();
            let id_practice = book.local_id.clone();
            let last_used = book
                .last_used_at
                .map(|value| format_datetime(value, &self.snapshot.timezone))
                .unwrap_or_else(|| "未使用".to_string());
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .py(px(20.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_6()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .child(div().text_size(px(22.0)).child(book.title.clone()))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(theme.muted_foreground)
                                            .child(if book.description.is_empty() {
                                                "説明なし".to_string()
                                            } else {
                                                book.description.clone()
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme.muted_foreground)
                                            .child(format!(
                                                "{}問 · 学習済み {} · 復習 {} · 短期 {} · 新規 {} · 最終使用 {}",
                                                book.total,
                                                book.learned,
                                                book.due,
                                                book.short_term_pending,
                                                book.new_available,
                                                last_used
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(format!("library-open-{}", book.local_id))
                                    .label("開く")
                                    .secondary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_book(id_open.clone(), cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new(format!("library-new-{}", book.local_id))
                                    .label("新規")
                                    .primary()
                                    .disabled(
                                        book.new_available == 0
                                            || self.snapshot.today.new_remaining_today == 0,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(Mode::New, Some(id_new.clone()), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("library-review-{}", book.local_id))
                                    .label("復習")
                                    .secondary()
                                    .disabled(book.due == 0 && book.short_term_pending == 0)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(Mode::Review, Some(id_review.clone()), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("library-practice-{}", book.local_id))
                                    .label("練習")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start(Mode::Practice, Some(id_practice.clone()), window, cx);
                                    })),
                            )
                            .when(!book.tags.is_empty(), |row| {
                                row.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_size(px(11.0))
                                        .text_color(theme.muted_foreground)
                                        .child(book.tags.join(" · ")),
                                )
                            }),
                    ),
            );
        }
        page.child(list).into_any_element()
    }

    fn render_book_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(summary) = self.selected_summary() else {
            return self.empty_state("教材が見つかりません", "一覧へ戻ってください。", cx);
        };
        let Some(workbook) = self.selected_workbook.as_ref() else {
            return self.empty_state("教材を読み込めません", "一覧へ戻ってください。", cx);
        };
        let theme = cx.theme().clone();
        let id_new = summary.local_id.clone();
        let id_review = summary.local_id.clone();
        let id_practice = summary.local_id.clone();
        let id_delete = summary.local_id.clone();
        let title = summary.title.clone();

        let mut page = div().flex().flex_col().gap_6();
        page = page
            .child(
                Button::new("detail-back")
                    .label("← 教材一覧")
                    .text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.go(Route::Library, window, cx);
                    })),
            )
            .child(self.page_header(title.clone(), workbook.description.clone(), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("detail-new")
                            .label("新規学習")
                            .primary()
                            .disabled(
                                summary.new_available == 0
                                    || self.snapshot.today.new_remaining_today == 0,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start(Mode::New, Some(id_new.clone()), window, cx);
                            })),
                    )
                    .child(
                        Button::new("detail-review")
                            .label("復習")
                            .secondary()
                            .disabled(summary.due == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start(Mode::Review, Some(id_review.clone()), window, cx);
                            })),
                    )
                    .child(
                        Button::new("detail-practice")
                            .label("練習")
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start(Mode::Practice, Some(id_practice.clone()), window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("detail-delete")
                            .label("削除")
                            .danger()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.modal = Some(crate::Modal::DeleteWorkbook {
                                    id: id_delete.clone(),
                                    title: title.clone(),
                                });
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_6()
                    .py(px(14.0))
                    .border_t_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.detail_stat("問題", summary.total.to_string(), cx))
                    .child(self.detail_stat("学習済み", summary.learned.to_string(), cx))
                    .child(self.detail_stat("復習", summary.due.to_string(), cx))
                    .child(self.detail_stat(
                        "短期再学習",
                        summary.short_term_pending.to_string(),
                        cx,
                    ))
                    .child(self.detail_stat("新規", summary.new_available.to_string(), cx))
                    .child(self.detail_stat("言語", workbook.language.clone(), cx)),
            );

        let mut notes = div().flex().flex_col();
        for (index, note) in workbook.notes.iter().enumerate() {
            notes =
                notes.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py(px(16.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(theme.muted_foreground)
                                                .child(format!("{:02}", index + 1)),
                                        )
                                        .child(div().text_size(px(17.0)).child(note.title.clone())),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{}問", note.questions.len())),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(theme.muted_foreground)
                                .line_height(px(20.0))
                                .child(note.knowledge_card.plain_text()),
                        )
                        .child(div().flex().flex_wrap().gap_2().children(
                            note.questions.iter().map(|question| {
                                div()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_size(px(10.0))
                                    .text_color(theme.muted_foreground)
                                    .child(type_label(question.kind))
                                    .into_any_element()
                            }),
                        )),
                );
        }
        page.child(notes).into_any_element()
    }

    fn detail_stat(&self, label: &'static str, value: String, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(div().text_size(px(20.0)).child(value))
            .into_any_element()
    }

    fn render_exams(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let active: Vec<_> = self
            .exams
            .iter()
            .filter(|summary| !summary.exam.is_archived())
            .cloned()
            .collect();
        let archived: Vec<_> = self
            .exams
            .iter()
            .filter(|summary| summary.exam.is_archived())
            .cloned()
            .collect();
        let mut page = div().flex().flex_col().gap_6();
        page = page.child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(self.page_header(
                    "試験",
                    "範囲は保存された条件から毎回解決されます。問題をコピーしません。",
                    cx,
                ))
                .child(
                    Button::new("exam-new")
                        .label("新しい試験")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_exam_editor(None, window, cx);
                        })),
                ),
        );
        if active.is_empty() && archived.is_empty() {
            return page
                .child(self.empty_state(
                    "試験がありません",
                    "教材・タグ・個別問題を組み合わせて、学習範囲を作成できます。",
                    cx,
                ))
                .into_any_element();
        }

        let mut list = div().flex().flex_col();
        for summary in &active {
            list = list.child(self.exam_row(summary, cx));
        }
        page = page.child(list);
        if !archived.is_empty() {
            page = page.child(
                div()
                    .mt(px(16.0))
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("アーカイブ"),
            );
            let mut archive_list = div().flex().flex_col();
            for summary in &archived {
                archive_list = archive_list.child(self.exam_row(summary, cx));
            }
            page = page.child(archive_list);
        }
        page.into_any_element()
    }

    fn exam_row(
        &self,
        summary: &workbook_service::ExamSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let id_open = summary.exam.id.clone();
        let id_archive = summary.exam.id.clone();
        let archived = summary.exam.is_archived();
        let title = summary.exam.title.clone();
        let target = exam_target_line(summary);
        let coverage = summary
            .coverage
            .map(|value| format!("{:.0}%", value * 100.0))
            .unwrap_or_else(|| "—".to_string());
        div()
            .flex()
            .flex_col()
            .gap_3()
            .py(px(18.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_3()
                                    .child(div().text_size(px(20.0)).child(title))
                                    .when(archived, |row| {
                                        row.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(theme.muted_foreground)
                                                .child("アーカイブ済み"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "範囲 {} · 学習済み {} · 期限 {} · 苦手 {} · 今日 {}問 · カバレッジ {}",
                                        summary.total,
                                        summary.studied,
                                        summary.due,
                                        summary.weak,
                                        summary.plan.coverage.selected,
                                        coverage
                                    )),
                            )
                            .when_some(target, |column, target| {
                                column.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.muted_foreground)
                                        .child(target),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new(format!("exam-open-{}", summary.exam.id))
                                    .label("開く")
                                    .secondary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_exam(id_open.clone(), cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("exam-archive-{}", summary.exam.id))
                                    .label(if archived {
                                        "再開"
                                    } else {
                                        "アーカイブ"
                                    })
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_exam_archive(id_archive.clone(), !archived, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_exam_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(summary) = self.selected_exam_summary().cloned() else {
            return self.empty_state("試験が見つかりません", "一覧へ戻ってください。", cx);
        };
        let theme = cx.theme().clone();
        let exam = &summary.exam;
        let id_edit = exam.id.clone();
        let id_archive = exam.id.clone();
        let id_delete = exam.id.clone();
        let delete_title = exam.title.clone();
        let archived = exam.is_archived();
        let coverage = summary
            .coverage
            .map(|value| format!("{:.0}%", value * 100.0))
            .unwrap_or_else(|| "—".to_string());
        let practice_rate = if summary.practice.graded == 0 {
            "—".to_string()
        } else {
            format!(
                "{:.0}%",
                summary.practice.correct as f64 / summary.practice.graded as f64 * 100.0
            )
        };
        let remaining_label = if summary.expired {
            "期限切れ".to_string()
        } else {
            summary
                .days_remaining
                .map(|days| format!("{days}日"))
                .unwrap_or_else(|| "—".to_string())
        };
        let planned_minutes = summary.plan.planned_seconds / 60;
        let planned_label = format!(
            "{}問 / 約{}分",
            summary.plan.coverage.selected, planned_minutes
        );
        let mut page = div().flex().flex_col().gap_6();
        page = page
            .child(
                Button::new("exam-detail-back")
                    .label("← 試験一覧")
                    .text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.go(Route::Exams, window, cx);
                    })),
            )
            .child(self.page_header(
                exam.title.clone(),
                exam.description.clone().unwrap_or_else(|| {
                    exam_target_line(&summary)
                        .unwrap_or_else(|| "試験の説明はありません".to_string())
                }),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("exam-start-plan")
                            .label("今日の学習を始める")
                            .primary()
                            .disabled(archived)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_exam_plan(window, cx);
                            })),
                    )
                    .child(
                        Button::new("exam-start-review")
                            .label("復習を始める")
                            .secondary()
                            .disabled(archived)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_exam_review(window, cx);
                            })),
                    )
                    .child(
                        Button::new("exam-start-practice")
                            .label("練習を始める")
                            .secondary()
                            .disabled(archived)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_exam_practice(window, cx);
                            })),
                    )
                    .child(
                        Button::new("exam-start-mock")
                            .label("模試を始める")
                            .secondary()
                            .disabled(archived)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_exam_mock(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("exam-edit")
                            .label("編集")
                            .ghost()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_exam_editor(Some(id_edit.clone()), window, cx);
                            })),
                    )
                    .child(
                        Button::new("exam-archive")
                            .label(if archived {
                                "再開"
                            } else {
                                "アーカイブ"
                            })
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_exam_archive(id_archive.clone(), !archived, cx);
                            })),
                    )
                    .child(Button::new("exam-delete").label("削除").danger().on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.modal = Some(crate::Modal::DeleteExam {
                                id: id_delete.clone(),
                                title: delete_title.clone(),
                            });
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .py(px(14.0))
                    .border_t_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.detail_stat("範囲", summary.total.to_string(), cx))
                    .child(self.detail_stat("学習済み", summary.studied.to_string(), cx))
                    .child(self.detail_stat("未学習", summary.unstudied.to_string(), cx))
                    .child(self.detail_stat("期限到来", summary.due.to_string(), cx))
                    .child(self.detail_stat("苦手", summary.weak.to_string(), cx))
                    .child(self.detail_stat("カバレッジ", coverage, cx))
                    .child(self.detail_stat("残り", remaining_label, cx))
                    .child(self.detail_stat("今日の計画", planned_label, cx))
                    .child(self.detail_stat(
                        "練習 正答",
                        format!("{} / {}", summary.practice.correct, summary.practice.graded),
                        cx,
                    ))
                    .child(self.detail_stat("練習 正答率", practice_rate, cx)),
            );

        page = page.child(section_heading("学習条件", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(exam_scope_summary(exam, &self.snapshot.books)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "条件は保存時点の一覧ではなく、開始時に毎回解決されます。現在の範囲は {} 問です。",
                            summary.total
                        )),
                ),
        );

        if !archived {
            page = page.child(section_heading("今日の計画", cx)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_size(px(12.0)).child(format!(
                        "1日{}分のうち、今日は約{}分（{}問）を予定しています。",
                        summary.plan.daily_minutes,
                        summary.plan.planned_seconds / 60,
                        summary.plan.coverage.selected
                    )))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "未学習 {} / 復習期限 {} / 苦手 {} / 試験前の確認 {}",
                                summary.plan.coverage.selected_new,
                                summary.plan.coverage.selected_due,
                                summary.plan.coverage.selected_weak,
                                summary.plan.coverage.selected_early
                            )),
                    )
                    .when(!summary.plan.allocation.is_empty(), |section| {
                        let fields = summary
                            .plan
                            .allocation
                            .iter()
                            .map(|allocation| {
                                let title = self
                                    .snapshot
                                    .books
                                    .iter()
                                    .find(|book| book.local_id == allocation.workbook_id)
                                    .map(|book| book.title.clone())
                                    .unwrap_or_else(|| allocation.workbook_id.clone());
                                format!("{title} {}問", allocation.questions)
                            })
                            .collect::<Vec<_>>()
                            .join(" / ");
                        section.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child(fields),
                        )
                    })
                    .children(summary.plan.warnings.iter().map(|warning| {
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(warning.message())
                    })),
            );
        }

        if !archived {
            page = page.child(section_heading("練習", cx)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(
                                [
                                    PracticeStrategy::All,
                                    PracticeStrategy::Random,
                                    PracticeStrategy::Unstudied,
                                    PracticeStrategy::PreviouslyIncorrect,
                                    PracticeStrategy::Weak,
                                ]
                                .into_iter()
                                .map(|strategy| {
                                    let selected = self.practice_strategy == strategy;
                                    Button::new(format!("practice-strategy-{}", strategy.as_str()))
                                        .label(strategy.label())
                                        .when(selected, |button| button.primary())
                                        .when(!selected, |button| button.secondary())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.practice_strategy = strategy;
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(label("問題数", cx))
                                    .child(
                                        Input::new(&self.practice_limit_input)
                                            .id("practice-limit")
                                            .w(px(180.0)),
                                    ),
                            )
                            .child(
                                Button::new("practice-start")
                                    .label("練習を開始")
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_exam_practice(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child("練習はFSRSの期限・安定度・難易度を変更しません。繰り返し実行できます。"),
                    ),
            );

            page = page.child(section_heading("模試", cx)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(label("問題数", cx))
                                    .child(
                                        Input::new(&self.mock_limit_input)
                                            .id("mock-limit")
                                            .w(px(160.0)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(label("制限時間（分）", cx))
                                    .child(
                                        Input::new(&self.mock_minutes_input)
                                            .id("mock-minutes")
                                            .w(px(160.0)),
                                    ),
                            )
                            .child(
                                Button::new("mock-order")
                                    .label(if self.mock_random {
                                        "順序: ランダム"
                                    } else {
                                        "順序: 固定"
                                    })
                                    .secondary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mock_random = !this.mock_random;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("mock-start")
                                    .label("模試を開始")
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_exam_mock(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child("出題は開始時に固定されます。FSRSは変更しません。未回答と採点不能は正解率の分母から除外します。"),
                    ),
            );
        }

        page = page.child(section_heading("模試履歴", cx));
        if summary.mock_history.is_empty() {
            page = page.child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.muted_foreground)
                    .child("まだ模試の記録はありません。"),
            );
        } else {
            let mut rows = div().flex().flex_col();
            for entry in &summary.mock_history {
                let score = entry
                    .score_percent
                    .map(|value| format!("{value:.0}%"))
                    .unwrap_or_else(|| "採点なし".to_string());
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py(px(12.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(14.0)).child(format_datetime(
                                    entry.submitted_at,
                                    &self.snapshot.timezone,
                                )))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(theme.muted_foreground)
                                        .child(format!(
                                            "{}問 · 正解 {} · 不正解 {} · 未回答 {} · 採点なし {}",
                                            entry.total,
                                            entry.correct,
                                            entry.incorrect,
                                            entry.unanswered,
                                            entry.ungraded
                                        )),
                                ),
                        )
                        .child(div().text_size(px(18.0)).child(score)),
                );
            }
            page = page.child(rows);
        }

        page.into_any_element()
    }

    fn render_exam_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.exam_editor.as_ref() else {
            return self.empty_state("編集画面がありません", "試験一覧へ戻ってください。", cx);
        };
        let theme = cx.theme().clone();
        let title = if editor.exam_id.is_some() {
            "試験を編集"
        } else {
            "新しい試験"
        };
        let is_new = editor.exam_id.is_none();
        let preview = editor.preview_count;
        let preview_error = editor.preview_error.clone();
        let groups = editor.groups.clone();
        let include = editor.include.clone();
        let exclude = editor.exclude.clone();
        let exclude_tags = editor.exclude_tags.clone();
        let exclude_workbooks = editor.exclude_workbooks.clone();
        let catalog = editor.catalog.clone();
        let search = editor
            .question_search
            .read(cx)
            .value()
            .to_string()
            .to_lowercase();
        let workbooks = self.snapshot.books.clone();
        let mut all_tags: BTreeSet<String> = workbooks
            .iter()
            .flat_map(|book| book.tags.iter().cloned())
            .collect();
        for group in &groups {
            all_tags.extend(group.tags.iter().cloned());
        }
        all_tags.extend(exclude_tags.iter().cloned());

        let mut page = div().flex().flex_col().gap_6();
        page = page
            .child(
                Button::new("exam-editor-back")
                    .label("← 戻る")
                    .text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(id) = this
                            .exam_editor
                            .as_ref()
                            .and_then(|editor| editor.exam_id.clone())
                        {
                            this.open_exam(id, cx);
                        } else {
                            this.go(Route::Exams, window, cx);
                        }
                    })),
            )
            .child(self.page_header(
                title,
                format!("範囲 {preview} 問 · タイトルと範囲は必須です"),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("試験名", cx))
                            .child(Input::new(&editor.title).id("exam-title").w_full()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("説明（任意）", cx))
                            .child(
                                Input::new(&editor.description)
                                    .id("exam-description")
                                    .w_full(),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .child(label("目標日（任意）", cx))
                                    .child(
                                        Input::new(&editor.target_date)
                                            .id("exam-target-date")
                                            .w_full(),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .child(label("目標スコア（任意）", cx))
                                    .child(
                                        Input::new(&editor.target_score)
                                            .id("exam-target-score")
                                            .w_full(),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .child(label("1日の学習時間（分）", cx))
                                    .child(
                                        Input::new(&editor.daily_minutes)
                                            .id("exam-daily-minutes")
                                            .w_full(),
                                    ),
                            ),
                    ),
            );

        page = page.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_heading("含める条件（グループはOR）", cx))
                .child(
                    Button::new("exam-add-group")
                        .label("グループを追加")
                        .secondary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(editor) = this.exam_editor.as_mut() {
                                editor.groups.push(Default::default());
                            }
                            cx.notify();
                        })),
                ),
        );

        for (index, group) in groups.iter().enumerate() {
            let mut group_box = div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .border_1()
                .border_color(theme.border);
            group_box = group_box.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "グループ {} · 条件内はAND / 同じ種類の複数選択はOR",
                                index + 1
                            )),
                    )
                    .when(groups.len() > 1, |row| {
                        row.child(
                            Button::new(format!("exam-group-remove-{index}"))
                                .label("削除")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = this.exam_editor.as_mut()
                                        && index < editor.groups.len()
                                    {
                                        editor.groups.remove(index);
                                    }
                                    this.refresh_exam_preview(cx);
                                })),
                        )
                    }),
            );
            let mut workbook_row = div().flex().flex_wrap().gap_2();
            for book in &workbooks {
                let workbook_id = book.local_id.clone();
                let selected = group.workbook_ids.contains(&workbook_id);
                workbook_row = workbook_row.child(
                    Button::new(format!("exam-group-{index}-wb-{workbook_id}"))
                        .label(book.title.clone())
                        .when(selected, |button| button.primary())
                        .when(!selected, |button| button.secondary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = this.exam_editor.as_mut()
                                && let Some(group) = editor.groups.get_mut(index)
                            {
                                if selected {
                                    group.workbook_ids.remove(&workbook_id);
                                } else {
                                    group.workbook_ids.insert(workbook_id.clone());
                                }
                            }
                            this.refresh_exam_preview(cx);
                        })),
                );
            }
            for workbook_id in group
                .workbook_ids
                .iter()
                .filter(|id| !workbooks.iter().any(|book| &book.local_id == *id))
            {
                let workbook_id = workbook_id.clone();
                workbook_row = workbook_row.child(
                    Button::new(format!("exam-group-{index}-missing-{workbook_id}"))
                        .label(format!("参照切れ {}", short_id(&workbook_id)))
                        .danger()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = this.exam_editor.as_mut()
                                && let Some(group) = editor.groups.get_mut(index)
                            {
                                group.workbook_ids.remove(&workbook_id);
                            }
                            this.refresh_exam_preview(cx);
                        })),
                );
            }
            group_box = group_box.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground)
                            .child("Workbook（未選択なら全て）"),
                    )
                    .child(workbook_row),
            );
            let mut tag_row = div().flex().flex_wrap().gap_2();
            for tag in &all_tags {
                let tag = tag.clone();
                let selected = group.tags.contains(&tag);
                tag_row = tag_row.child(
                    Button::new(format!("exam-group-{index}-tag-{tag}"))
                        .label(tag.clone())
                        .when(selected, |button| button.primary())
                        .when(!selected, |button| button.secondary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = this.exam_editor.as_mut()
                                && let Some(group) = editor.groups.get_mut(index)
                            {
                                if selected {
                                    group.tags.remove(&tag);
                                } else {
                                    group.tags.insert(tag.clone());
                                }
                            }
                            this.refresh_exam_preview(cx);
                        })),
                );
            }
            group_box = group_box.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground)
                            .child("Workbookタグ（未選択なら全て）"),
                    )
                    .child(tag_row),
            );
            page = page.child(group_box);
        }

        page = page.child(section_heading("除外", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child("除外タグ（含める条件の後に適用）"),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(all_tags.iter().map(|tag| {
                            let tag = tag.clone();
                            let selected = exclude_tags.contains(&tag);
                            Button::new(format!("exam-exclude-tag-{tag}"))
                                .label(tag.clone())
                                .when(selected, |button| button.danger())
                                .when(!selected, |button| button.ghost())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(editor) = this.exam_editor.as_mut() {
                                        if selected {
                                            editor.exclude_tags.remove(&tag);
                                        } else {
                                            editor.exclude_tags.insert(tag.clone());
                                        }
                                    }
                                    this.refresh_exam_preview(cx);
                                }))
                                .into_any_element()
                        })),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child("除外Workbook"),
                )
                .child(
                    div().flex().flex_wrap().gap_2().children(
                        workbooks
                            .iter()
                            .map(|book| {
                                let workbook_id = book.local_id.clone();
                                let selected = exclude_workbooks.contains(&workbook_id);
                                Button::new(format!("exam-exclude-wb-{workbook_id}"))
                                    .label(book.title.clone())
                                    .when(selected, |button| button.danger())
                                    .when(!selected, |button| button.ghost())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(editor) = this.exam_editor.as_mut() {
                                            if selected {
                                                editor.exclude_workbooks.remove(&workbook_id);
                                            } else {
                                                editor
                                                    .exclude_workbooks
                                                    .insert(workbook_id.clone());
                                            }
                                        }
                                        this.refresh_exam_preview(cx);
                                    }))
                                    .into_any_element()
                            })
                            .chain(
                                exclude_workbooks
                                    .iter()
                                    .filter(|id| {
                                        !workbooks.iter().any(|book| &book.local_id == *id)
                                    })
                                    .map(|workbook_id| {
                                        let workbook_id = workbook_id.clone();
                                        let label = format!("参照切れ {}", short_id(&workbook_id));
                                        Button::new(format!("exam-exclude-missing-{workbook_id}"))
                                            .label(label)
                                            .danger()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(editor) = this.exam_editor.as_mut() {
                                                    editor.exclude_workbooks.remove(&workbook_id);
                                                }
                                                this.refresh_exam_preview(cx);
                                            }))
                                            .into_any_element()
                                    }),
                            ),
                    ),
                ),
        );

        page = page.child(section_heading("個別の問題", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .border_1()
                .border_color(theme.border)
                .child(
                    Input::new(&editor.question_search)
                        .id("exam-question-search")
                        .w_full(),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child("個別に含める / 除外する（グループ条件より後に適用）"),
                )
                .child({
                    let mut rows = div().flex().flex_col();
                    let mut shown = 0;
                    for question in &catalog {
                        if !search.is_empty()
                            && !question.question_prompt.to_lowercase().contains(&search)
                            && !question.workbook_title.to_lowercase().contains(&search)
                        {
                            continue;
                        }
                        let key = crate::QuestionKey::new(
                            question.workbook_id.clone(),
                            question.question_id.clone(),
                        );
                        let included = include.contains(&key);
                        let excluded = exclude.contains(&key);
                        let include_key = key.clone();
                        let exclude_key = key.clone();
                        rows = rows.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .py(px(8.0))
                                .border_b_1()
                                .border_color(theme.border)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .flex_1()
                                        .child(
                                            div().text_size(px(13.0)).child(truncate_text(
                                                &question.question_prompt,
                                                90,
                                            )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(theme.muted_foreground)
                                                .child(format!(
                                                    "{} · {}",
                                                    question.workbook_title, question.question_id
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            Button::new(format!(
                                                "exam-include-{}-{}",
                                                question.workbook_id, question.question_id
                                            ))
                                            .label("含める")
                                            .when(included, |button| button.primary())
                                            .when(!included, |button| button.secondary())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(editor) = this.exam_editor.as_mut() {
                                                    if included {
                                                        editor.include.remove(&include_key);
                                                    } else {
                                                        editor.include.insert(include_key.clone());
                                                        editor.exclude.remove(&include_key);
                                                    }
                                                }
                                                this.refresh_exam_preview(cx);
                                            })),
                                        )
                                        .child(
                                            Button::new(format!(
                                                "exam-exclude-{}-{}",
                                                question.workbook_id, question.question_id
                                            ))
                                            .label("除外")
                                            .when(excluded, |button| button.danger())
                                            .when(!excluded, |button| button.ghost())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(editor) = this.exam_editor.as_mut() {
                                                    if excluded {
                                                        editor.exclude.remove(&exclude_key);
                                                    } else {
                                                        editor.exclude.insert(exclude_key.clone());
                                                        editor.include.remove(&exclude_key);
                                                    }
                                                }
                                                this.refresh_exam_preview(cx);
                                            })),
                                        ),
                                ),
                        );
                        shown += 1;
                    }
                    if shown == 0 {
                        rows = rows.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.muted_foreground)
                                .child("該当する問題がありません。"),
                        );
                    }
                    rows
                }),
        );

        page = page.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(18.0))
                                .child(format!("範囲プレビュー: {preview} 問")),
                        )
                        .when_some(preview_error, |column, error| {
                            column.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.danger)
                                    .child(error),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("exam-editor-cancel")
                                .label("キャンセル")
                                .secondary()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !is_new
                                        && let Some(id) = this
                                            .exam_editor
                                            .as_ref()
                                            .and_then(|editor| editor.exam_id.clone())
                                    {
                                        this.open_exam(id, cx);
                                        return;
                                    }
                                    this.go(Route::Exams, window, cx);
                                })),
                        )
                        .child(
                            Button::new("exam-editor-save")
                                .label("保存")
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_exam(window, cx);
                                })),
                        ),
                ),
        );
        page.into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let entries = &self.history;
        let mut page = div().flex().flex_col().gap_6();
        page = page.child(self.page_header(
            "履歴",
            "終了したセッションの集計です。短期再学習と初回正答を分けて表示します。",
            cx,
        ));
        if entries.is_empty() {
            return page
                .child(self.empty_state(
                    "まだ履歴がありません",
                    "学習セッションを終了すると、ここに集計が残ります。",
                    cx,
                ))
                .into_any_element();
        }
        let mut list = div().flex().flex_col();
        for entry in entries {
            let rate = format_percent(
                entry.stats.first_pass_correct,
                entry.stats.first_pass_graded,
            );
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .py(px(18.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_size(px(18.0))
                                            .child(entry.workbook_titles.join(" / ")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.muted_foreground)
                                            .child(mode_label(entry.mode)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format_datetime(
                                        entry.started_at,
                                        &self.snapshot.timezone,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_5()
                            .child(history_stat("問題", entry.stats.questions.to_string(), cx))
                            .child(history_stat(
                                "回答した問題",
                                entry.stats.answered.to_string(),
                                cx,
                            ))
                            .child(history_stat("総回答", entry.stats.attempts.to_string(), cx))
                            .child(history_stat(
                                "短期再学習",
                                entry.stats.short_term.to_string(),
                                cx,
                            ))
                            .child(history_stat("初回正答率", rate, cx))
                            .child(history_stat(
                                "スキップ",
                                entry.stats.skipped.to_string(),
                                cx,
                            ))
                            .child(history_stat(
                                "学習時間",
                                format_duration(entry.stats.elapsed_ms),
                                cx,
                            )),
                    ),
            );
        }
        page.child(list).into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let settings = &self.snapshot.settings;
        let mut page = div().flex().flex_col().gap_7();
        page = page.child(self.page_header(
            "設定",
            "学習アルゴリズムとローカルデータを管理します。変更は既存カードの期限を一括変更しません。",
            cx,
        ));

        page = page.child(section_heading("学習", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .p_5()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .flex_1()
                                .child(label("目標定着率 (0.70–0.97)", cx))
                                .child(
                                    Input::new(&self.settings_retention).id("settings-retention").w(px(180.0)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .flex_1()
                                .child(label("1セッションの新規問題数", cx))
                                .child(
                                    Input::new(&self.settings_new_limit).id("settings-new-limit").w(px(180.0)),
                                ),
                        )
                        .child(
                            Button::new("settings-apply")
                                .label("適用")
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_settings(window, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_size(px(15.0)).child("自動評価"))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "回答の正誤からFSRS評価を自動で決めます。Easyは自動判定せず、必要なときだけ評価を変更できます。",
                                        ),
                                ),
                        )
                        .child(
                            Button::new("settings-automatic")
                                .label(if settings.automatic_rating { "ON" } else { "OFF" })
                                .when(settings.automatic_rating, |button| button.primary())
                                .when(!settings.automatic_rating, |button| button.secondary())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let enabled = !this.snapshot.settings.automatic_rating;
                                    this.set_automatic_rating(enabled, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(
                            "FSRS-7 · 学習 1分 → 10分 · 再学習 10分 · Learn Ahead 20分 · 学習日 午前4時",
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("タイムゾーン: {}", self.snapshot.timezone)),
                ),
        );

        page = page.child(section_heading("表示", cx)).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(theme_button(
                    "theme-system",
                    "System",
                    settings.theme == ThemeSetting::System,
                    ThemeSetting::System,
                    cx,
                ))
                .child(theme_button(
                    "theme-light",
                    "Light",
                    settings.theme == ThemeSetting::Light,
                    ThemeSetting::Light,
                    cx,
                ))
                .child(theme_button(
                    "theme-dark",
                    "Dark",
                    settings.theme == ThemeSetting::Dark,
                    ThemeSetting::Dark,
                    cx,
                )),
        );

        page = page.child(section_heading("データ", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child("保存先"),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .child(self.snapshot.data_path.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("settings-backup")
                                .label("完全バックアップ")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.backup_dialog(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-restore")
                                .label("バックアップから復元")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.restore_dialog(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-legacy")
                                .label("旧Workbookから移行")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.legacy_dialog(window, cx);
                                })),
                        )
                        .child(
                            Button::new("settings-folder")
                                .label("Finderで開く")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_data_folder(cx);
                                })),
                        )
                        .child(
                            Button::new("settings-integrity")
                                .label("整合性チェック")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.check_integrity(cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child("バックアップは教材・履歴・セッション・設定をすべて含みます。個人データとして扱ってください。"),
                ),
        );

        page = page.child(section_heading("危険な操作", cx)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .p_5()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_size(px(15.0)).child("学習履歴をリセット"))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.muted_foreground)
                                .child("教材は残し、FSRS状態・回答・セッションだけを削除します。"),
                        ),
                )
                .child(
                    Button::new("settings-reset")
                        .label("リセット")
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.modal = Some(crate::Modal::ResetHistory);
                            cx.notify();
                        })),
                ),
        );

        page = page.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.muted_foreground)
                .child(format!(
                    "Workbook 2 v{} · オフライン · WebViewなし · GPUI",
                    env!("CARGO_PKG_VERSION")
                )),
        );
        page.into_any_element()
    }

    pub(crate) fn render_study(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.study.as_ref() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("学習セッションがありません")
                .into_any_element();
        };
        let theme = cx.theme().clone();
        let mut page = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(self.study_top_bar(view, cx))
            .child(
                div()
                    .id("study-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .max_w(px(760.0))
                            .mx_auto()
                            .px(px(32.0))
                            .py(px(40.0))
                            .gap_6()
                            .child(match view.phase {
                                Phase::Intro => self.render_intro(view, cx),
                                Phase::Question => self.render_question(view, cx),
                                Phase::Feedback => self.render_feedback(view, cx),
                                Phase::WorkbookResult => self.render_workbook_result(view, cx),
                                Phase::OverallResult => self.render_overall_result(view, cx),
                            }),
                    ),
            );
        if let Some(notice) = self.notice.clone() {
            page = page.child(div().px(px(32.0)).child(self.render_notice(notice, cx)));
        }
        page.into_any_element()
    }

    fn study_top_bar(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let active = self
            .snapshot
            .active
            .as_ref()
            .is_some_and(|active| active.session_id == view.session_id);
        let progress = if view.progress.total == 0 {
            String::new()
        } else {
            format!(
                "{} / {}",
                view.progress.position.min(view.progress.total),
                view.progress.total
            )
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(28.0))
            .py(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(if active {
                Button::new("study-exit")
                    .label("終了")
                    .text()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.modal = Some(crate::Modal::Finish);
                        cx.notify();
                    }))
                    .into_any_element()
            } else {
                Button::new("study-close")
                    .label("Todayへ")
                    .text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.study = None;
                        this.go(Route::Today, window, cx);
                    }))
                    .into_any_element()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .when_some(view.exam_title.clone(), |row, title| {
                        row.child(div().text_size(px(13.0)).child(title))
                    })
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(mode_label(view.mode)),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme.muted_foreground)
                            .child(progress),
                    )
                    .when_some(view.budget, |row, budget| {
                        row.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(if budget.reached {
                                    theme.accent
                                } else {
                                    theme.muted_foreground
                                })
                                .child(if budget.reached {
                                    format!(
                                        "目標 {}分・回答 {}（目安に到達）",
                                        budget.target_seconds / 60,
                                        format_duration(budget.used_ms)
                                    )
                                } else {
                                    format!(
                                        "目標 {}分・残り目安 {}",
                                        budget.target_seconds / 60,
                                        format_duration(budget.remaining_ms.max(0))
                                    )
                                }),
                        )
                    })
                    .when_some(
                        view.mock
                            .as_ref()
                            .filter(|mock| !mock.submitted)
                            .and_then(|mock| mock.remaining_ms),
                        |row, remaining| {
                            row.child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(if remaining < 60_000 {
                                        theme.danger
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(format!("残り {}", format_duration(remaining))),
                            )
                        },
                    )
                    .when_some(
                        view.mock.as_ref().filter(|mock| !mock.submitted),
                        |row, mock| {
                            row.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} / {} 回答", mock.answered, mock.total)),
                            )
                        },
                    ),
            )
            .child(if active {
                div()
                    .flex()
                    .gap_2()
                    .when(
                        view.mode == Mode::Mock && view.phase != Phase::OverallResult,
                        |row| {
                            row.child(
                                Button::new("study-mock-submit")
                                    .label("提出")
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_mock_session(window, cx);
                                    })),
                            )
                        },
                    )
                    .child(
                        Button::new("study-finish")
                            .label(if view.mode == Mode::Mock {
                                "模試を終了"
                            } else {
                                "今日はここまで"
                            })
                            .text()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = Some(crate::Modal::Finish);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .into_any_element()
    }

    fn render_intro(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(intro) = view.intro.as_ref() else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("はじめに · このNoteは一度だけ説明を表示します"),
            )
            .child(
                div()
                    .text_size(px(28.0))
                    .font_weight(FontWeight::LIGHT)
                    .child(intro.note_title.clone()),
            )
            .child(crate::content::render_content(
                "intro-card",
                &intro.content,
                &theme,
                &self.content_images(),
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("intro-begin")
                            .label("問題へ")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_question(window, cx);
                            })),
                    )
                    .child(
                        Button::new("intro-skip")
                            .label("この問題をスキップ")
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.skip_question(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_question(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(question) = view.question.as_ref() else {
            return div().into_any_element();
        };
        let mut page = div().flex().flex_col().gap_5();
        if let Some(note) = view.note_title.as_ref() {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child(note.clone()),
                    )
                    .when(view.item_kind == "short_term", |row| {
                        row.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme.accent)
                                .child("短期再学習"),
                        )
                    }),
            );
        }
        let statement = if question.kind == QuestionType::Flashcard {
            question.front.as_ref()
        } else {
            question.prompt.as_ref()
        };
        page = page.child(match statement {
            Some(Content::Text(text)) => div()
                .text_size(px(28.0))
                .font_weight(FontWeight::LIGHT)
                .line_height(px(36.0))
                .child(text.clone())
                .into_any_element(),
            Some(content) => crate::content::render_content(
                "question-prompt",
                content,
                &theme,
                &self.content_images(),
            ),
            None => div().into_any_element(),
        });

        if question.kind == QuestionType::Flashcard {
            if view.revealed {
                if let Some(back) = question.back.as_ref() {
                    page = page.child(crate::content::render_content(
                        "flash-back",
                        back,
                        &theme,
                        &self.content_images(),
                    ));
                }
                if view.mode == Mode::Mock {
                    page = page.child(
                        Button::new("mock-flash-next")
                            .label("次へ（採点なし）")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_mock_question(window, cx);
                            })),
                    );
                } else {
                    page = page.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("flash-yes")
                                    .label("思い出せた")
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.self_correct = Some(true);
                                        this.submit_answer(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("flash-no")
                                    .label("思い出せなかった")
                                    .danger()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.self_correct = Some(false);
                                        this.submit_answer(window, cx);
                                    })),
                            ),
                    );
                }
            } else {
                page = page.child(
                    Button::new("flash-reveal")
                        .label("解答を見る")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.reveal_flashcard(window, cx);
                        })),
                );
            }
        } else {
            page = page.child(self.render_answer_input(question, cx));
        }

        if let Some(hint) = question.hint.as_ref()
            && view.mode != Mode::Mock
        {
            if view.hint_visible {
                page = page.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .bg(theme.secondary)
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme.muted_foreground)
                                .child("ヒント"),
                        )
                        .child(crate::content::render_content(
                            "hint",
                            hint,
                            &theme,
                            &self.content_images(),
                        )),
                );
            } else {
                page = page.child(
                    Button::new("hint-show")
                        .label("ヒントを見る")
                        .secondary()
                        .w(px(160.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_hint(window, cx);
                        })),
                );
            }
        }

        if question.kind != QuestionType::Flashcard || view.revealed {
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .when(view.mode != Mode::Mock, |row| {
                        row.child(
                            Button::new("question-unknown")
                                .label("わからない")
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.mark_unknown(window, cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("question-skip")
                                    .label(if view.mode == Mode::Mock {
                                        "スキップ（未回答）"
                                    } else {
                                        "スキップ"
                                    })
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.skip_question(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("question-submit")
                                    .label(if view.mode == Mode::Mock {
                                        "回答して次へ  ⌘↩"
                                    } else {
                                        "回答する  ⌘↩"
                                    })
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_answer(window, cx);
                                    })),
                            ),
                    ),
            );
        }

        page.into_any_element()
    }

    fn render_answer_input(&self, question: &Question, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        match question.kind {
            QuestionType::SingleChoice => {
                let options = question.options.clone().unwrap_or_default();
                let mut list = div().flex().flex_col().gap_2();
                for option in options {
                    let id = option.id.clone();
                    let selected = self.single_choice.as_deref() == Some(option.id.as_str());
                    let body = rich_option_body(
                        &format!("choice-body-{}", option.id),
                        None,
                        &option.content,
                        None,
                        &theme,
                        &self.content_images(),
                    );
                    list = list.child(
                        Button::new(format!("choice-{}", option.id))
                            .child(body)
                            .when(selected, |button| button.primary())
                            .when(!selected, |button| button.secondary())
                            .w_full()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.single_choice = Some(id.clone());
                                this.persist_draft(window, cx);
                                cx.notify();
                            })),
                    );
                }
                list.into_any_element()
            }
            QuestionType::MultipleChoice => {
                let options = question.options.clone().unwrap_or_default();
                let mut list = div().flex().flex_col().gap_2();
                for option in options {
                    let id = option.id.clone();
                    let selected = self.multiple_choice.contains(&option.id);
                    let body = rich_option_body(
                        &format!("multi-body-{}", option.id),
                        Some(if selected { "✓" } else { "　" }),
                        &option.content,
                        None,
                        &theme,
                        &self.content_images(),
                    );
                    list = list.child(
                        Button::new(format!("multi-{}", option.id))
                            .child(body)
                            .when(selected, |button| button.primary())
                            .when(!selected, |button| button.secondary())
                            .w_full()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.multiple_choice.remove(&id) {
                                    this.multiple_choice.insert(id.clone());
                                }
                                this.persist_draft(window, cx);
                                cx.notify();
                            })),
                    );
                }
                list.into_any_element()
            }
            QuestionType::TrueFalse => div()
                .flex()
                .gap_3()
                .child(
                    Button::new("tf-true")
                        .label("○ 正しい")
                        .when(self.true_false == Some(true), |button| button.primary())
                        .when(self.true_false != Some(true), |button| button.secondary())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.true_false = Some(true);
                            this.persist_draft(window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("tf-false")
                        .label("× まちがい")
                        .when(self.true_false == Some(false), |button| button.primary())
                        .when(self.true_false != Some(false), |button| button.secondary())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.true_false = Some(false);
                            this.persist_draft(window, cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
            QuestionType::ShortText => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child("回答"),
                )
                .child(
                    Input::new(&self.short_input)
                        .id("short-answer")
                        .w_full()
                        .h(px(48.0)),
                )
                .into_any_element(),
            QuestionType::Cloze => {
                let theme = cx.theme().clone();
                let mut list = div().flex().flex_col().gap_3();
                for (id, input) in &self.cloze_inputs {
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(120.0))
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("［{id}］")),
                            )
                            .child(
                                Input::new(input)
                                    .id(format!("cloze-{id}"))
                                    .w_full()
                                    .h(px(44.0)),
                            ),
                    );
                }
                list.into_any_element()
            }
            QuestionType::Numeric => {
                let theme = cx.theme().clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground)
                            .child("数値と単位（分数も使えます）"),
                    )
                    .child(
                        Input::new(&self.numeric_input)
                            .id("numeric-answer")
                            .w_full()
                            .h(px(48.0)),
                    )
                    .into_any_element()
            }
            QuestionType::Ordering => self.render_ordering(question, cx),
            QuestionType::Matching => self.render_matching(question, cx),
            QuestionType::Hotspot => self.render_hotspot(question, cx),
            QuestionType::Labeling => self.render_labeling(question, cx),
            QuestionType::Flashcard => div().into_any_element(),
        }
    }

    fn render_ordering(&self, question: &Question, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let contents: BTreeMap<&str, &Content> = question
            .options
            .as_ref()
            .map(|options| {
                options
                    .iter()
                    .map(|option| (option.id.as_str(), &option.content))
                    .collect()
            })
            .unwrap_or_default();
        let mut list = div().flex().flex_col().gap_2();
        for (index, id) in self.ordering.iter().enumerate() {
            let id = id.clone();
            let content = contents.get(id.as_str()).copied();
            let accessible = content
                .map(Content::plain_text)
                .unwrap_or_else(|| id.clone());
            let target_index = index;
            let up_id = id.clone();
            let down_id = id.clone();
            let drag_id = id.clone();
            let drag_label = accessible.clone();
            let up_label = format!("{accessible} を上へ");
            let down_label = format!("{accessible} を下へ");
            let row = div()
                .id(SharedString::from(format!("order-row-{id}")))
                .flex()
                .items_center()
                .gap_2()
                .p_2()
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .w(px(24.0))
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("{}", index + 1)),
                )
                .child(match content {
                    Some(content) => rich_option_body(
                        &format!("order-{id}"),
                        None,
                        content,
                        None,
                        &theme,
                        &self.content_images(),
                    ),
                    None => div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(15.0))
                        .child(id.clone())
                        .into_any_element(),
                })
                .child(
                    Button::new(format!("order-up-{id}"))
                        .label("↑")
                        .accessibility_label(up_label)
                        .ghost()
                        .disabled(index == 0)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.move_ordering(&up_id, target_index.saturating_sub(1));
                            this.persist_draft(window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(format!("order-down-{id}"))
                        .label("↓")
                        .accessibility_label(down_label)
                        .ghost()
                        .disabled(index + 1 >= self.ordering.len())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.move_ordering(&down_id, target_index + 1);
                            this.persist_draft(window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground)
                        .child("ドラッグ"),
                )
                .on_drag(
                    OrderDrag {
                        id: drag_id,
                        label: drag_label,
                    },
                    |drag, _, _, cx| {
                        let label = drag.label.clone();
                        cx.new(|_| DragGhost { label })
                    },
                )
                .on_drop::<OrderDrag>(cx.listener(move |this, drag: &OrderDrag, window, cx| {
                    this.move_ordering(&drag.id, target_index);
                    this.persist_draft(window, cx);
                    cx.notify();
                }));
            list = list.child(row);
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("ドラッグ、または ↑↓ ボタンで並べ替えます"),
            )
            .child(list)
            .into_any_element()
    }

    fn move_ordering(&mut self, id: &str, target: usize) {
        crate::ui_logic::move_order(&mut self.ordering, id, target);
    }

    fn render_matching(&self, question: &Question, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(matching) = question.matching.as_ref() else {
            return div().into_any_element();
        };
        let mut left_list = div().flex().flex_col().gap_2();
        for item in &matching.left {
            let id = item.id.clone();
            let selected = self.matching_left.as_deref() == Some(item.id.as_str());
            let assigned = self.matching.get(&item.id).and_then(|right| {
                matching
                    .right
                    .iter()
                    .find(|candidate| &candidate.id == right)
            });
            let accessible = match assigned {
                Some(right) => format!(
                    "{} → {}",
                    item.content.plain_text(),
                    right.content.plain_text()
                ),
                None => item.content.plain_text(),
            };
            let body = rich_option_body(
                &format!("match-left-body-{}", item.id),
                Some(if selected { "●" } else { "　" }),
                &item.content,
                assigned.map(|right| ("→", &right.content)),
                &theme,
                &self.content_images(),
            );
            left_list = left_list.child(
                Button::new(format!("match-left-{}", item.id))
                    .child(body)
                    .accessibility_label(accessible)
                    .h_auto()
                    .items_start()
                    .py(px(6.0))
                    .when(selected, |button| button.primary())
                    .when(!selected, |button| button.secondary())
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.matching_left = if this.matching_left.as_deref() == Some(id.as_str()) {
                            None
                        } else {
                            Some(id.clone())
                        };
                        cx.notify();
                    })),
            );
        }
        let mut right_list = div().flex().flex_col().gap_2();
        for item in &matching.right {
            let right_id = item.id.clone();
            let assigned_left = self
                .matching
                .iter()
                .find(|(_, right)| **right == item.id)
                .and_then(|(left, _)| matching.left.iter().find(|candidate| &candidate.id == left));
            let assigned = assigned_left.is_some();
            let accessible = match assigned_left {
                Some(left) => format!(
                    "{} → {}",
                    left.content.plain_text(),
                    item.content.plain_text()
                ),
                None => item.content.plain_text(),
            };
            let body = rich_option_body(
                &format!("match-right-body-{}", item.id),
                Some(if assigned { "✓" } else { "　" }),
                &item.content,
                assigned_left.map(|left| ("←", &left.content)),
                &theme,
                &self.content_images(),
            );
            right_list = right_list.child(
                Button::new(format!("match-right-{}", item.id))
                    .child(body)
                    .accessibility_label(accessible)
                    .h_auto()
                    .items_start()
                    .py(px(6.0))
                    .when(assigned, |button| button.primary())
                    .when(!assigned, |button| button.secondary())
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.assign_matching(&right_id);
                        this.persist_draft(window, cx);
                        cx.notify();
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("左の項目を選んでから、対応する右の項目を選びます"),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(div().flex().flex_col().flex_1().gap_2().child(left_list))
                    .child(div().flex().flex_col().flex_1().gap_2().child(right_list)),
            )
            .child(
                Button::new("matching-clear")
                    .label("対応をクリア")
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.matching.clear();
                        this.matching_left = None;
                        this.persist_draft(window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn assign_matching(&mut self, right: &str) {
        let Some(left) = self.matching_left.take() else {
            return;
        };
        crate::ui_logic::assign_pair(&mut self.matching, &left, right);
    }

    fn render_hotspot(&self, question: &Question, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(hotspot) = question.hotspot.as_ref() else {
            return div().into_any_element();
        };
        let asset = self
            .study
            .as_ref()
            .and_then(|view| view.assets.iter().find(|asset| asset.id == hotspot.asset))
            .cloned();
        let mut column = div().flex().flex_col().gap_3();
        if let Some(asset) = asset {
            let asset_width = asset.width;
            let asset_height = asset.height;
            let bounds_slot = self.hotspot_image_bounds.clone();
            let mut image = self
                .render_question_image(&asset, cx)
                .on_children_prepainted(move |children, _, _| {
                    if let Some(bounds) = children.first() {
                        bounds_slot.set(Some((
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        )));
                    }
                })
                .id("hotspot-image");
            image = image.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // Use the real pointer position. GPUI reports it in logical
                // pixels, so Retina/HiDPI scaling is already applied.
                let Some(position) = event.mouse_position() else {
                    return;
                };
                let Some((left, top, width, height)) = this.hotspot_image_bounds.get() else {
                    return;
                };
                let Some(point) = crate::ui_logic::normalize_image_point(
                    f32::from(position.x) - left,
                    f32::from(position.y) - top,
                    width,
                    height,
                    asset_width,
                    asset_height,
                ) else {
                    // A click in the letterbox margin is not an answer.
                    return;
                };
                this.hotspot_point = Some(point);
                this.persist_draft(window, cx);
                cx.notify();
            }));
            let marker_position = self.hotspot_point.and_then(|(x, y)| {
                let (width, height) = self
                    .hotspot_image_bounds
                    .get()
                    .map(|(_, _, width, height)| (width, height))
                    .unwrap_or_else(|| {
                        crate::ui_logic::fit_image(asset.width, asset.height, 640.0, 380.0)
                    });
                crate::ui_logic::image_point_to_container(
                    x,
                    y,
                    width,
                    height,
                    asset.width,
                    asset.height,
                )
            });
            column = column.child(match marker_position {
                Some((x, y)) => image.child(
                    div()
                        .absolute()
                        .left(px(x - 7.0))
                        .top(px(y - 7.0))
                        .w(px(14.0))
                        .h(px(14.0))
                        .bg(theme.accent),
                ),
                None => image,
            });
            column = column.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(asset.alt.clone()),
            );
        } else {
            column = column.child(
                div()
                    .p_3()
                    .border_1()
                    .border_color(theme.danger)
                    .text_size(px(13.0))
                    .child("画像を読み込めません。「領域名で選ぶ」から選択してください。"),
            );
        }
        if let Some((x, y)) = self.hotspot_point {
            column = column.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(format!("選択位置: {:.1}% / {:.1}%", x * 100.0, y * 100.0)),
            );
        }
        let mut regions = div().flex().flex_wrap().gap_2();
        for region in &hotspot.regions {
            let Some(point) = crate::ui_logic::region_center(&region.shape) else {
                continue;
            };
            let selected = self
                .hotspot_point
                .is_some_and(|(x, y)| region.shape.contains(x, y));
            regions = regions.child(
                Button::new(format!("hotspot-{}", region.id))
                    .label(region.alt.clone())
                    .when(selected, |button| button.primary())
                    .when(!selected, |button| button.secondary())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.hotspot_point = Some(point);
                        this.hotspot_list_open = false;
                        this.persist_draft(window, cx);
                        cx.notify();
                    })),
            );
        }
        column = column
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("画像をクリックするか、キーボードで領域名から選べます"),
            )
            .child(
                Button::new("hotspot-region-toggle")
                    .label(if self.hotspot_list_open {
                        "領域名を閉じる"
                    } else {
                        "領域名で選ぶ"
                    })
                    .secondary()
                    .w(px(200.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.hotspot_list_open = !this.hotspot_list_open;
                        cx.notify();
                    })),
            );
        if self.hotspot_list_open {
            column = column.child(regions);
        }
        column.into_any_element()
    }

    fn render_labeling(&self, question: &Question, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(labeling) = question.labeling.as_ref() else {
            return div().into_any_element();
        };
        let asset = self
            .study
            .as_ref()
            .and_then(|view| view.assets.iter().find(|asset| asset.id == labeling.asset))
            .cloned();
        let label_contents: BTreeMap<&str, &Content> = labeling
            .labels
            .iter()
            .map(|label| (label.id.as_str(), &label.content))
            .collect();
        let mut column = div().flex().flex_col().gap_3();
        if let Some(asset) = asset {
            let entity = cx.entity();
            let bounds_slot = self.labeling_image_bounds.clone();
            let mut canvas_box = self
                .render_question_image(&asset, cx)
                .on_children_prepainted(move |children, _, cx| {
                    if let Some(bounds) = children.first() {
                        let next = (
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.y),
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        );
                        let changed = bounds_slot.get().is_none_or(|(_, _, width, height)| {
                            (width - next.2).abs() > 0.5 || (height - next.3).abs() > 0.5
                        });
                        bounds_slot.set(Some(next));
                        if changed {
                            let entity = entity.clone();
                            cx.defer(move |cx| cx.notify(entity.entity_id()));
                        }
                    }
                });
            // Use the measured `contain` box. The fallback matches the requested
            // layout for the first frame; the prepaint listener re-renders once
            // the real window width is known.
            let (container_width, container_height) = self
                .labeling_image_bounds
                .get()
                .map(|(_, _, width, height)| (width, height))
                .unwrap_or_else(|| {
                    crate::ui_logic::fit_image(asset.width, asset.height, 640.0, 380.0)
                });
            for target in &labeling.targets {
                let Some((target_x, target_y)) = crate::ui_logic::image_point_to_container(
                    target.x,
                    target.y,
                    container_width,
                    container_height,
                    asset.width,
                    asset.height,
                ) else {
                    continue;
                };
                let assigned = self
                    .labeling
                    .get(&target.id)
                    .and_then(|label| label_contents.get(label.as_str()).copied());
                let target_id = target.id.clone();
                let target_id_drop = target.id.clone();
                let target_button = div()
                    .id(SharedString::from(format!("label-target-{}", target.id)))
                    .absolute()
                    .left(px(target_x - 30.0))
                    .top(px(target_y - 18.0))
                    .min_w(px(60.0))
                    .h(px(36.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .bg(if assigned.is_some() {
                        theme.accent.opacity(0.35)
                    } else {
                        theme.accent.opacity(0.16)
                    })
                    .border_1()
                    .border_color(theme.accent)
                    .text_size(px(12.0))
                    .child(match assigned {
                        Some(content) => div()
                            .max_w(px(120.0))
                            .max_h(px(32.0))
                            .overflow_hidden()
                            .child(crate::content::render_content_compact(
                                &format!("label-target-content-{}", target.id),
                                content,
                                &theme,
                                &self.content_images(),
                            ))
                            .into_any_element(),
                        None => div().text_size(px(12.0)).child("＋").into_any_element(),
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(label) = this.labeling_label.take() {
                            this.assign_label(&target_id, &label);
                            this.persist_draft(window, cx);
                            cx.notify();
                        }
                    }))
                    .on_drop::<LabelDrag>(cx.listener(
                        move |this, drag: &LabelDrag, window, cx| {
                            this.assign_label(&target_id_drop, &drag.id);
                            this.labeling_label = None;
                            this.persist_draft(window, cx);
                            cx.notify();
                        },
                    ));
                canvas_box = canvas_box.child(target_button);
            }
            column = column.child(canvas_box);
            column = column.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(asset.alt.clone()),
            );
        } else {
            column = column.child(
                div()
                    .p_3()
                    .border_1()
                    .border_color(theme.danger)
                    .text_size(px(13.0))
                    .child("画像を読み込めません。ラベルを選んでから配置先を選んでください。"),
            );
        }
        let mut chips = div().flex().flex_wrap().gap_2();
        for label in &labeling.labels {
            let click_id = label.id.clone();
            let drag_id = label.id.clone();
            let text = label.content.plain_text();
            let selected = self.labeling_label.as_deref() == Some(label.id.as_str());
            let used = self.labeling.values().any(|value| value == &label.id);
            let body = rich_option_body(
                &format!("label-body-{}", label.id),
                Some(if used { "✓" } else { "　" }),
                &label.content,
                None,
                &theme,
                &self.content_images(),
            );
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("label-drag-{}", label.id)))
                    .on_drag(
                        LabelDrag {
                            id: drag_id,
                            label: text.clone(),
                        },
                        |drag, _, _, cx| {
                            let label = drag.label.clone();
                            cx.new(|_| DragGhost { label })
                        },
                    )
                    .child(
                        Button::new(format!("label-{}", label.id))
                            .child(body)
                            .accessibility_label(text)
                            .h_auto()
                            .items_start()
                            .py(px(6.0))
                            .max_w(relative(0.48))
                            .when(selected, |button| button.primary())
                            .when(!selected, |button| button.secondary())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.labeling_label =
                                    if this.labeling_label.as_deref() == Some(click_id.as_str()) {
                                        None
                                    } else {
                                        Some(click_id.clone())
                                    };
                                cx.notify();
                            })),
                    ),
            );
        }
        let mut target_buttons = div().flex().flex_wrap().gap_1();
        for target in &labeling.targets {
            let target_id = target.id.clone();
            let assigned = self
                .labeling
                .get(&target.id)
                .and_then(|label| label_contents.get(label.as_str()).copied());
            let accessible = match assigned {
                Some(content) => format!("{}: {}", target.alt, content.plain_text()),
                None => format!("{}: 未配置", target.alt),
            };
            let body = div()
                .flex()
                .items_center()
                .gap_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .child(format!("{}: ", target.alt)),
                )
                .child(match assigned {
                    Some(content) => div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(crate::content::render_content_compact(
                            &format!("label-target-alt-content-{}", target.id),
                            content,
                            &theme,
                            &self.content_images(),
                        ))
                        .into_any_element(),
                    None => div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child("未配置")
                        .into_any_element(),
                })
                .into_any_element();
            target_buttons = target_buttons.child(
                Button::new(SharedString::from(format!("label-target-alt-{target_id}")))
                    .child(body)
                    .accessibility_label(accessible)
                    .h_auto()
                    .items_start()
                    .py(px(6.0))
                    .secondary()
                    .w(relative(0.48))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(label) = this.labeling_label.take() {
                            this.assign_label(&target_id, &label);
                            this.persist_draft(window, cx);
                            cx.notify();
                        }
                    })),
            );
        }
        column
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("ラベルを選んで配置先をクリックするか、ラベルをドラッグします"),
            )
            .child(chips)
            .child(target_buttons)
            .into_any_element()
    }

    fn assign_label(&mut self, target: &str, label: &str) {
        crate::ui_logic::assign_pair(&mut self.labeling, target, label);
    }

    fn render_question_image(&self, asset: &AssetView, _cx: &Context<Self>) -> gpui_kit::Div {
        self.content_images().render_asset_div(
            asset,
            crate::ui_logic::annotation_image_fit(),
            640.0,
            380.0,
        )
    }

    fn render_feedback(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(feedback) = view.feedback.as_ref() else {
            return div().into_any_element();
        };
        let (color, label) = match feedback.status {
            GradeStatus::Correct => (theme.success, "正解"),
            GradeStatus::Incorrect => (theme.danger, "不正解"),
            GradeStatus::NearMiss => (theme.accent, "おしい"),
            GradeStatus::Manual => (theme.accent, "自己評価"),
        };
        let mut page = div().flex().flex_col().gap_5();
        if let Some(question) = view.question.as_ref() {
            page = page.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(if question.kind == QuestionType::Flashcard {
                        question
                            .front
                            .as_ref()
                            .map(Content::plain_text)
                            .unwrap_or_default()
                    } else {
                        question.prompt_text()
                    }),
            );
        }
        let status_row = div()
            .flex()
            .items_center()
            .gap_3()
            .p_4()
            .border_l_3()
            .border_color(color)
            .bg(color.opacity(0.08))
            .child(div().text_size(px(18.0)).child(label))
            .when(feedback.unknown, |row| {
                row.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child("「わからない」として回答"),
                )
            })
            .when(feedback.hint, |row| {
                row.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child("ヒント使用"),
                )
            })
            .when(feedback.assisted && !feedback.hint, |row| {
                row.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("再回答 {}回", feedback.retry_count)),
                )
            });
        page = page.child(status_row);

        let given = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme.muted_foreground)
                    .child("あなたの回答"),
            )
            .child(
                div()
                    .text_size(px(15.0))
                    .child(feedback.given_answer.clone()),
            );

        if feedback.status == GradeStatus::NearMiss {
            page = page.child(given);
            for message in &feedback.near_miss_messages {
                page = page.child(
                    div()
                        .p_4()
                        .border_1()
                        .border_color(theme.accent)
                        .text_size(px(15.0))
                        .child(message.to_string()),
                );
            }
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground)
                            .child("この回答はまだ確定していません。"),
                    )
                    .child(
                        Button::new("feedback-retry")
                            .label("もう一度  ⌘↩")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.retry_question(window, cx);
                            })),
                    ),
            );
            return page.into_any_element();
        }

        page = page.child(given);
        if let Some(diagnostic) = feedback.diagnostic.as_ref() {
            page = page.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "一致: {} / {}",
                        diagnostic.matched, diagnostic.total
                    )),
            );
        }
        page = page
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground)
                            .child("正解"),
                    )
                    .child(
                        div()
                            .text_size(px(15.0))
                            .child(feedback.answer_summary.clone()),
                    ),
            )
            .child(crate::content::render_content(
                "feedback-explanation",
                &feedback.explanation,
                &theme,
                &self.content_images(),
            ));

        if feedback.committed {
            let rating = feedback.rating.unwrap_or(Rating::Good);
            let automatic = feedback.rating_source == Some(RatingSource::Automatic);
            page = page.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "{}: {}",
                                        if automatic { "自動評価" } else { "評価" },
                                        rating.label()
                                    )),
                            )
                            .when(feedback.assisted, |row| {
                                row.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(theme.muted_foreground)
                                        .child("補助あり（自動評価はHard上限）"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("feedback-correct")
                                    .label("正誤を訂正")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_correct(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("feedback-rating-toggle")
                                    .label(if self.rating_picker_open {
                                        "評価を閉じる"
                                    } else {
                                        "評価を変更"
                                    })
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_rating_picker(cx);
                                    })),
                            ),
                    ),
            );
            if self.rating_picker_open {
                page = page.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(self.rating_button("rating-again", Rating::Again, feedback, cx))
                        .child(self.rating_button("rating-hard", Rating::Hard, feedback, cx))
                        .child(self.rating_button("rating-good", Rating::Good, feedback, cx))
                        .child(self.rating_button("rating-easy", Rating::Easy, feedback, cx)),
                );
            }
            page = page.child(
                div().flex().justify_end().items_center().child(
                    Button::new("feedback-next")
                        .label("次へ  ⌘↩")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.next_question(window, cx);
                        })),
                ),
            );
        } else {
            page = page.child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child(if feedback.suggested_rating.is_some() {
                        "評価を選ぶとFSRS状態が確定します。"
                    } else {
                        "この問題は自動評価できません。評価を選んでください。"
                    }),
            );
            page = page.child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.rating_button("rating-again", Rating::Again, feedback, cx))
                    .child(self.rating_button("rating-hard", Rating::Hard, feedback, cx))
                    .child(self.rating_button("rating-good", Rating::Good, feedback, cx))
                    .child(self.rating_button("rating-easy", Rating::Easy, feedback, cx)),
            );
        }

        page.into_any_element()
    }

    fn rating_button(
        &self,
        id: &'static str,
        rating: Rating,
        feedback: &workbook_service::FeedbackView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = if feedback.committed {
            feedback.rating == Some(rating)
        } else {
            feedback.suggested_rating == Some(rating)
        };
        Button::new(id)
            .label(format!(
                "{} {}  [{}]",
                rating.value(),
                rating.japanese_label(),
                rating.label()
            ))
            .when(active, |button| button.primary())
            .when(!active, |button| button.secondary())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.rate_question(rating, window, cx);
            }))
            .into_any_element()
    }

    fn render_mock_waiting(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let mock = view.mock.as_ref();
        let total = mock.map(|mock| mock.total).unwrap_or(0);
        let answered = mock.map(|mock| mock.answered).unwrap_or(0);
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("模試"),
            )
            .child(
                div()
                    .text_size(px(32.0))
                    .font_weight(FontWeight::LIGHT)
                    .child("提出の準備ができました"),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{total}問中 {answered}問に回答しました。未回答は不正解ではなく「未回答」として集計されます。"
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("mock-submit-all")
                            .label("模試を提出する")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_mock_session(window, cx);
                            })),
                    )
                    .child(
                        Button::new("mock-review-today")
                            .label("今日へ戻る")
                            .secondary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.study = None;
                                this.go(Route::Today, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_mock_result(&self, result: &MockResultView, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let score = result
            .score_percent
            .map(|value| format!("{value:.0}%"))
            .unwrap_or_else(|| "採点なし".to_string());
        let exam_id = result.exam_id.clone();
        let mut page = div().flex().flex_col().gap_5();
        page = page
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("模試の結果"),
            )
            .child(
                div()
                    .text_size(px(32.0))
                    .font_weight(FontWeight::LIGHT)
                    .child(result.exam_title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(format_datetime(
                        result.submitted_at,
                        &self.snapshot.timezone,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .p_5()
                    .border_1()
                    .border_color(theme.border)
                    .child(history_stat("得点", score, cx))
                    .child(history_stat("正解", result.correct.to_string(), cx))
                    .child(history_stat("不正解", result.incorrect.to_string(), cx))
                    .child(history_stat("未回答", result.unanswered.to_string(), cx))
                    .child(history_stat("採点なし", result.ungraded.to_string(), cx))
                    .child(history_stat("問題", result.total.to_string(), cx))
                    .child(history_stat(
                        "所要時間",
                        format_duration(result.duration_ms),
                        cx,
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child(
                        "得点は 正解 ÷ (正解 + 不正解) です。未回答と採点不能は分母に含めません。",
                    ),
            );

        let mut list = div().flex().flex_col();
        for item in &result.items {
            let color = match item.status {
                MockItemStatus::Correct => theme.success,
                MockItemStatus::Incorrect => theme.danger,
                MockItemStatus::Unanswered => theme.muted_foreground,
                MockItemStatus::Ungraded => theme.accent,
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .py(px(14.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(theme.muted_foreground)
                                            .child(format!(
                                                "{:02} · {} · {}",
                                                item.ordinal + 1,
                                                item.workbook_title,
                                                item.note_title
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.0))
                                            .child(truncate_text(&item.prompt, 140)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(color)
                                    .child(item.status.label()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground)
                            .child(format!("回答: {}", item.given_answer))
                            .child(format!("正答: {}", item.answer_summary)),
                    )
                    .when(!item.explanation.plain_text().is_empty(), |column| {
                        column.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.muted_foreground)
                                .child(item.explanation.plain_text()),
                        )
                    }),
            );
        }
        page = page.child(list).child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("mock-result-exam")
                        .label("試験へ戻る")
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_exam(exam_id.clone(), cx);
                        })),
                )
                .child(
                    Button::new("mock-result-today")
                        .label("今日へ")
                        .secondary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.study = None;
                            this.go(Route::Today, window, cx);
                        })),
                ),
        );
        page.into_any_element()
    }

    fn render_workbook_result(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if view.mode == Mode::Mock {
            return self.render_mock_waiting(view, cx);
        }
        let theme = cx.theme().clone();
        let Some(result) = view.workbook_result.as_ref() else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("教材の結果"),
            )
            .child(
                div()
                    .text_size(px(32.0))
                    .font_weight(FontWeight::LIGHT)
                    .child(result.title.clone()),
            )
            .child(self.result_stats(result.stats, cx))
            .when_some(result.next_workbook_title.clone(), |page, next| {
                page.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_4()
                        .bg(theme.secondary)
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted_foreground)
                                .child("次の教材"),
                        )
                        .child(div().text_size(px(16.0)).child(next)),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Button::new("result-finish")
                            .label("ここで終了")
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.modal = Some(crate::Modal::Finish);
                                cx.notify();
                            })),
                    )
                    .child(if result.next_workbook_title.is_some() {
                        Button::new("result-continue")
                            .label("次の教材へ")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.continue_workbook(window, cx);
                            }))
                            .into_any_element()
                    } else {
                        Button::new("result-continue-end")
                            .label("セッションを終える")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.continue_workbook(window, cx);
                            }))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn render_overall_result(
        &self,
        view: &workbook_service::StudyView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(result) = view.mock_result.as_ref() {
            return self.render_mock_result(result, cx);
        }
        let theme = cx.theme().clone();
        let Some(result) = view.overall_result.as_ref() else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground)
                    .child("セッションの結果"),
            )
            .child(
                div()
                    .text_size(px(36.0))
                    .font_weight(FontWeight::LIGHT)
                    .child("おつかれさまでした"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.muted_foreground)
                    .child(result.workbooks.join(" / ")),
            )
            .child(self.result_stats(result.stats, cx))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("overall-today")
                            .label("今日へ戻る")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(cx);
                                this.study = None;
                                this.go(Route::Today, window, cx);
                            })),
                    )
                    .child(
                        Button::new("overall-history")
                            .label("履歴を見る")
                            .secondary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.study = None;
                                this.go(Route::History, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn result_stats(&self, stats: workbook_service::ResultStats, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_wrap()
            .gap_6()
            .p_5()
            .border_1()
            .border_color(theme.border)
            .child(history_stat("問題", stats.questions.to_string(), cx))
            .child(history_stat("回答した問題", stats.answered.to_string(), cx))
            .child(history_stat("総回答", stats.attempts.to_string(), cx))
            .child(history_stat("短期再学習", stats.short_term.to_string(), cx))
            .child(history_stat(
                "初回正答率",
                format_percent(stats.first_pass_correct, stats.first_pass_graded),
                cx,
            ))
            .child(history_stat("スキップ", stats.skipped.to_string(), cx))
            .child(history_stat(
                "学習時間",
                format_duration(stats.elapsed_ms),
                cx,
            ))
            .into_any_element()
    }
}

fn exam_target_line(summary: &workbook_service::ExamSummary) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(date) = summary.exam.target_date {
        let remaining = if summary.expired {
            "期限切れ".to_string()
        } else {
            summary
                .days_remaining
                .map(|days| format!("あと{days}日"))
                .unwrap_or_default()
        };
        parts.push(format!("目標日 {date}（{remaining}）"));
    }
    if let Some(score) = summary.exam.target_score {
        parts.push(format!("目標 {score}"));
    }
    parts.push(format!("1日 {}分", summary.exam.daily_minutes));
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn exam_scope_summary(
    exam: &workbook_domain::Exam,
    books: &[workbook_service::BookSummary],
) -> String {
    let workbook_label = |id: &str| {
        books
            .iter()
            .find(|book| book.local_id == id)
            .map(|book| book.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut parts = Vec::new();
    for (index, group) in exam.scope.groups.iter().enumerate() {
        let workbooks = if group.workbook_ids.is_empty() {
            "すべての教材".to_string()
        } else {
            group
                .workbook_ids
                .iter()
                .map(|id| workbook_label(id))
                .collect::<Vec<_>>()
                .join(" または ")
        };
        let tags = if group.tags.is_empty() {
            "すべてのタグ".to_string()
        } else {
            group.tags.join(" または ")
        };
        parts.push(format!("{}: ({workbooks}) AND ({tags})", index + 1));
    }
    if !exam.scope.include_questions.is_empty() {
        parts.push(format!(
            "個別指定 {} 問",
            exam.scope.include_questions.len()
        ));
    }
    let mut text = parts.join(" OR ");
    if text.is_empty() {
        text = "条件なし".to_string();
    }
    let mut excludes = Vec::new();
    if !exam.scope.exclude_questions.is_empty() {
        excludes.push(format!("個別 {} 問", exam.scope.exclude_questions.len()));
    }
    if !exam.scope.exclude_workbooks.is_empty() {
        excludes.push(format!(
            "Workbook {}",
            exam.scope
                .exclude_workbooks
                .iter()
                .map(|id| workbook_label(id))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    if !exam.scope.exclude_tags.is_empty() {
        excludes.push(format!("タグ {}", exam.scope.exclude_tags.join(" / ")));
    }
    if !excludes.is_empty() {
        text.push_str(&format!(" · 除外: {}", excludes.join(" / ")));
    }
    text
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn truncate_text(text: &str, limit: usize) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= limit {
        cleaned
    } else {
        let mut truncated: String = cleaned.chars().take(limit).collect();
        truncated.push('…');
        truncated
    }
}

fn type_label(kind: QuestionType) -> &'static str {
    match kind {
        QuestionType::SingleChoice => "単一選択",
        QuestionType::MultipleChoice => "複数選択",
        QuestionType::TrueFalse => "正誤",
        QuestionType::ShortText => "記述",
        QuestionType::Cloze => "穴埋め",
        QuestionType::Flashcard => "フラッシュカード",
        QuestionType::Numeric => "数値入力",
        QuestionType::Ordering => "並べ替え",
        QuestionType::Matching => "対応付け",
        QuestionType::Hotspot => "領域選択",
        QuestionType::Labeling => "ラベル付け",
    }
}

fn label(text: &'static str, cx: &Context<WorkbookApp>) -> AnyElement {
    div()
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn section_heading(text: &'static str, cx: &Context<WorkbookApp>) -> AnyElement {
    div()
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn history_stat(label: &'static str, value: String, cx: &Context<WorkbookApp>) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(div().text_size(px(17.0)).child(value))
        .into_any_element()
}

fn theme_button(
    id: &'static str,
    text: &'static str,
    active: bool,
    theme: ThemeSetting,
    cx: &mut Context<WorkbookApp>,
) -> AnyElement {
    Button::new(id)
        .label(text)
        .when(active, |button| button.primary())
        .when(!active, |button| button.secondary())
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_theme(theme, window, cx);
        }))
        .into_any_element()
}
