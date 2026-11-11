//! GPUI views for Workbook 2.
//!
//! All persistence and scheduling goes through `workbook-service`; this crate
//! only owns transient view state (focus, draft widgets, dialogs).

mod content;
mod image_cache;
mod math;
mod modal;
mod ui_logic;
mod views;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono_tz::Tz;
use gpui_kit::component::{
    ActiveTheme, Theme, ThemeMode,
    input::{InputEvent, InputState},
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable as _, Hsla, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Subscription, Window, WindowAppearance,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use workbook_domain::{
    Answer, ChoiceOption, Content, ExamDraft, ExamScope, ExamScopeGroup, MockOptions, MockOrder,
    Mode, Note, Phase, PracticeOptions, PracticeStrategy, Question, QuestionRef, QuestionType,
    Rating, Settings, ThemeSetting, Workbook, answer_is_blank,
};
use workbook_import::LegacyPreview;
use workbook_service::{
    BookSummary, ExamSummary, GoalSummary, Snapshot, StudyView, TodaySummary, WorkbookIssue,
    WorkbookService,
};

use crate::image_cache::ImageCache;

/// Window-space rectangle `(x, y, width, height)` in logical pixels.
pub(crate) type HotspotImageBounds = (f32, f32, f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Today,
    Library,
    BookDetail,
    History,
    Settings,
    Exams,
    ExamDetail,
    ExamEditor,
    Scope,
    GoalDetail,
    Editor,
    Study,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub(crate) struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

#[derive(Clone)]
pub(crate) enum Modal {
    Finish,
    DeleteWorkbook {
        id: String,
        title: String,
    },
    ImportJson {
        workbook: Workbook,
        path: PathBuf,
        source: String,
        issues: Vec<String>,
    },
    Restore {
        path: PathBuf,
        name: String,
    },
    Legacy {
        path: PathBuf,
        preview: LegacyPreview,
    },
    ResetHistory,
    DeleteExam {
        id: String,
        title: String,
    },
    DeleteTopic {
        id: String,
        name: String,
    },
    DeleteGoal {
        id: String,
        title: String,
    },
    ResetFsrs {
        questions: Vec<workbook_service::QuestionRevisionPlan>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScopeGroupDraft {
    pub workbook_ids: BTreeSet<String>,
    pub tags: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct QuestionKey {
    pub workbook_id: String,
    pub question_id: String,
}

impl QuestionKey {
    pub(crate) fn new(workbook_id: impl Into<String>, question_id: impl Into<String>) -> Self {
        Self {
            workbook_id: workbook_id.into(),
            question_id: question_id.into(),
        }
    }
}

pub(crate) struct ExamEditorState {
    pub exam_id: Option<String>,
    pub title: Entity<InputState>,
    pub description: Entity<InputState>,
    pub target_date: Entity<InputState>,
    pub target_score: Entity<InputState>,
    pub daily_minutes: Entity<InputState>,
    pub groups: Vec<ScopeGroupDraft>,
    pub include: BTreeSet<QuestionKey>,
    pub exclude: BTreeSet<QuestionKey>,
    pub exclude_tags: BTreeSet<String>,
    pub exclude_workbooks: BTreeSet<String>,
    pub catalog: Vec<workbook_domain::ExamCatalogQuestion>,
    pub preview_count: usize,
    pub preview_error: Option<String>,
    pub question_search: Entity<InputState>,
}

/// Transient state for the cross-workbook scope screen. The saved scope lives
/// only in exams/goals; this draft is rebuilt from the service on demand.
pub(crate) struct ScopeState {
    pub selected_workbooks: BTreeSet<String>,
    pub selected_topics: BTreeSet<String>,
    pub include: BTreeSet<QuestionKey>,
    pub exclude: BTreeSet<QuestionKey>,
    pub exclude_workbooks: BTreeSet<String>,
    pub exclude_topics: BTreeSet<String>,
    pub match_all: bool,
    pub parent_topic: Option<String>,
    pub editing_topic: Option<String>,
    pub catalog: Vec<workbook_domain::ExamCatalogQuestion>,
    pub topics: Vec<workbook_service::TopicSummary>,
    pub preview: Option<workbook_service::ScopePreview>,
    pub error: Option<String>,
}

impl ScopeState {
    fn new(
        catalog: Vec<workbook_domain::ExamCatalogQuestion>,
        topics: Vec<workbook_service::TopicSummary>,
    ) -> Self {
        Self {
            selected_workbooks: BTreeSet::new(),
            selected_topics: BTreeSet::new(),
            include: BTreeSet::new(),
            exclude: BTreeSet::new(),
            exclude_workbooks: BTreeSet::new(),
            exclude_topics: BTreeSet::new(),
            match_all: false,
            parent_topic: None,
            editing_topic: None,
            catalog,
            topics,
            preview: None,
            error: None,
        }
    }

    pub(crate) fn scope(&self) -> ExamScope {
        let workbook_ids: Vec<String> = self.selected_workbooks.iter().cloned().collect();
        let topic_ids: Vec<String> = self.selected_topics.iter().cloned().collect();
        let mut groups = Vec::new();
        match (workbook_ids.is_empty(), topic_ids.is_empty()) {
            (false, false) if self.match_all => groups.push(ExamScopeGroup {
                workbook_ids,
                tags: Vec::new(),
                topics: topic_ids,
            }),
            (false, false) => {
                groups.push(ExamScopeGroup {
                    workbook_ids,
                    tags: Vec::new(),
                    topics: Vec::new(),
                });
                groups.push(ExamScopeGroup {
                    workbook_ids: Vec::new(),
                    tags: Vec::new(),
                    topics: topic_ids,
                });
            }
            (false, true) => groups.push(ExamScopeGroup {
                workbook_ids,
                tags: Vec::new(),
                topics: Vec::new(),
            }),
            (true, false) => groups.push(ExamScopeGroup {
                workbook_ids: Vec::new(),
                tags: Vec::new(),
                topics: topic_ids,
            }),
            (true, true) => {}
        }
        ExamScope {
            groups,
            include_questions: self
                .include
                .iter()
                .map(|key| QuestionRef::new(key.workbook_id.clone(), key.question_id.clone()))
                .collect(),
            exclude_questions: self
                .exclude
                .iter()
                .map(|key| QuestionRef::new(key.workbook_id.clone(), key.question_id.clone()))
                .collect(),
            exclude_workbooks: self.exclude_workbooks.iter().cloned().collect(),
            exclude_tags: Vec::new(),
            exclude_topics: self.exclude_topics.iter().cloned().collect(),
        }
    }
}

pub(crate) struct WorkbookEditorState {
    pub local_id: Option<String>,
    pub workbook: Workbook,
    pub note_index: usize,
    pub question_index: usize,
    pub assets: Vec<workbook_service::AssetView>,
    pub syncing: bool,
    pub error: Option<String>,
    pub issues: Vec<WorkbookIssue>,
    pub title: Entity<InputState>,
    pub description: Entity<InputState>,
    pub tags: Entity<InputState>,
    pub note_title: Entity<InputState>,
    pub note_card: Entity<InputState>,
    pub prompt: Entity<InputState>,
    pub hint: Entity<InputState>,
    pub explanation: Entity<InputState>,
    pub front: Entity<InputState>,
    pub back: Entity<InputState>,
    pub answer: Entity<InputState>,
    pub topics: Entity<InputState>,
    pub advanced: Entity<InputState>,
    pub option_inputs: Vec<Entity<InputState>>,
    pub _subscriptions: Vec<Subscription>,
}

pub struct WorkbookApp {
    pub(crate) service: Arc<WorkbookService>,
    pub(crate) route: Route,
    pub(crate) snapshot: Snapshot,
    pub(crate) history: Vec<workbook_service::HistoryEntry>,
    pub(crate) study: Option<StudyView>,
    pub(crate) selected_book: Option<String>,
    pub(crate) selected_workbook: Option<Workbook>,
    pub(crate) exams: Vec<ExamSummary>,
    pub(crate) goals: Vec<GoalSummary>,
    pub(crate) selected_goal: Option<String>,
    pub(crate) selected_exam: Option<String>,
    pub(crate) exam_editor: Option<ExamEditorState>,
    pub(crate) scope_editor: Option<ScopeState>,
    pub(crate) editor: Option<WorkbookEditorState>,
    pub(crate) scope_search_text: String,
    pub(crate) scope_search: Entity<InputState>,
    pub(crate) scope_topic_name: Entity<InputState>,
    pub(crate) scope_subscription: Option<Subscription>,
    pub(crate) goal_title_input: Entity<InputState>,
    pub(crate) goal_date_input: Entity<InputState>,
    pub(crate) goal_minutes_input: Entity<InputState>,
    pub(crate) practice_strategy: PracticeStrategy,
    pub(crate) budget_seconds: Option<u32>,
    pub(crate) practice_limit_input: Entity<InputState>,
    pub(crate) mock_limit_input: Entity<InputState>,
    pub(crate) mock_minutes_input: Entity<InputState>,
    pub(crate) mock_random: bool,
    pub(crate) search: String,
    pub(crate) search_input: Entity<InputState>,
    pub(crate) short_input: Entity<InputState>,
    pub(crate) cloze_inputs: Vec<(String, Entity<InputState>)>,
    pub(crate) answer_subscriptions: Vec<Subscription>,
    pub(crate) root_focus: FocusHandle,
    pub(crate) single_choice: Option<String>,
    pub(crate) multiple_choice: BTreeSet<String>,
    pub(crate) true_false: Option<bool>,
    pub(crate) self_correct: Option<bool>,
    pub(crate) numeric_input: Entity<InputState>,
    pub(crate) ordering: Vec<String>,
    pub(crate) matching: BTreeMap<String, String>,
    pub(crate) matching_left: Option<String>,
    pub(crate) hotspot_point: Option<(f64, f64)>,
    /// Last measured bounds of the hotspot image element in window logical
    /// pixels: `(x, y, width, height)`. Refreshed on every prepaint so clicks
    /// do not depend on scroll position or window placement.
    pub(crate) hotspot_image_bounds: Rc<Cell<Option<HotspotImageBounds>>>,
    /// Measured bounds of the Labeling image element. Targets are placed with
    /// the same `contain` mapping as Hotspot clicks, so letterboxing and window
    /// resizing cannot shift a label away from its image coordinate.
    pub(crate) labeling_image_bounds: Rc<Cell<Option<HotspotImageBounds>>>,
    pub(crate) hotspot_list_open: bool,
    pub(crate) labeling: BTreeMap<String, String>,
    pub(crate) labeling_label: Option<String>,
    pub(crate) image_cache: Rc<RefCell<ImageCache>>,
    pub(crate) rating_picker_open: bool,
    pub(crate) notice: Option<Notice>,
    pub(crate) modal: Option<Modal>,
    /// Set only by the reset-confirmation dialog. It lets `save_editor` retry
    /// the same save after the user has seen which questions lose FSRS state.
    pub(crate) fsrs_reset_confirmed: bool,
    pub(crate) settings_retention: Entity<InputState>,
    pub(crate) settings_new_limit: Entity<InputState>,
    pub(crate) appearance_subscription: Option<Subscription>,
    pub(crate) activation_subscription: Option<Subscription>,
    _tick_task: gpui_kit::Task<()>,
}

impl WorkbookApp {
    pub fn new(service: Arc<WorkbookService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let snapshot = service.snapshot().unwrap_or_else(|_error| Snapshot {
            books: Vec::new(),
            settings: Settings::default(),
            active: None,
            today: TodaySummary {
                due: 0,
                new_available: 0,
                new_limit: 10,
                new_remaining_today: 0,
                short_term_pending: 0,
                next_due_at: None,
                answers: 0,
                correct: 0,
                graded: 0,
                elapsed_ms: 0,
                books: Vec::new(),
            },
            goals: Vec::new(),
            data_path: String::new(),
            timezone: "UTC".to_string(),
        });
        let forced_route = std::env::var("WORKBOOK2_ROUTE").ok();
        let route = if snapshot.active.is_some() {
            Route::Study
        } else {
            match forced_route.as_deref() {
                Some("library") => Route::Library,
                Some("history") => Route::History,
                Some("settings") => Route::Settings,
                _ => Route::Today,
            }
        };
        let study = snapshot.active.clone();
        let history = service.history(200).unwrap_or_default();
        let root_focus = cx.focus_handle();
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("教材を検索"));
        let short_input = cx.new(|cx| InputState::new(window, cx).placeholder("回答を入力"));
        let numeric_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("数値と単位を入力（例: 1.5m）"));
        let retention = format!("{:.2}", snapshot.settings.retention);
        let new_limit = snapshot.settings.new_limit.to_string();
        let settings_retention =
            cx.new(|cx| InputState::new(window, cx).default_value(retention.clone()));
        let settings_new_limit =
            cx.new(|cx| InputState::new(window, cx).default_value(new_limit.clone()));
        let practice_limit_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("問題数（空=すべて）"));
        let mock_limit_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("問題数（空=すべて）"));
        let mock_minutes_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("制限時間（分・空=なし）"));
        let scope_search = cx.new(|cx| InputState::new(window, cx).placeholder("問題文・IDで検索"));
        let scope_topic_name = cx.new(|cx| InputState::new(window, cx).placeholder("Topic名"));
        let goal_title_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Goal名（例: 一次関数を固める）"));
        let goal_date_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("期限 YYYY-MM-DD（空=なし）"));
        let goal_minutes_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("30")
                .placeholder("1日の学習時間（分）")
        });

        let mut app = Self {
            service,
            route,
            snapshot,
            history,
            study,
            selected_book: None,
            selected_workbook: None,
            exams: Vec::new(),
            goals: Vec::new(),
            selected_goal: None,
            selected_exam: None,
            exam_editor: None,
            scope_editor: None,
            editor: None,
            scope_search_text: String::new(),
            scope_search,
            scope_topic_name,
            scope_subscription: None,
            goal_title_input,
            goal_date_input,
            goal_minutes_input,
            practice_strategy: PracticeStrategy::All,
            budget_seconds: Some(10 * 60),
            practice_limit_input,
            mock_limit_input,
            mock_minutes_input,
            mock_random: false,
            search: String::new(),
            search_input,
            short_input,
            cloze_inputs: Vec::new(),
            answer_subscriptions: Vec::new(),
            root_focus,
            single_choice: None,
            multiple_choice: BTreeSet::new(),
            true_false: None,
            self_correct: None,
            numeric_input,
            ordering: Vec::new(),
            matching: BTreeMap::new(),
            matching_left: None,
            hotspot_point: None,
            hotspot_list_open: false,
            labeling: BTreeMap::new(),
            labeling_label: None,
            image_cache: Rc::new(RefCell::new(ImageCache::default())),
            hotspot_image_bounds: Rc::new(Cell::new(None)),
            labeling_image_bounds: Rc::new(Cell::new(None)),
            rating_picker_open: false,
            notice: None,
            modal: None,
            fsrs_reset_confirmed: false,
            settings_retention,
            settings_new_limit,
            appearance_subscription: None,
            activation_subscription: None,
            _tick_task: gpui_kit::Task::ready(()),
        };

        let search = app.search_input.clone();
        let subscription = cx.subscribe_in(&search, window, |this, state, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search = state.read(cx).value().to_string();
                cx.notify();
            }
        });
        let _ = subscription;

        let scope_search = app.scope_search.clone();
        let scope_subscription =
            cx.subscribe_in(&scope_search, window, |this, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.scope_search_text = state.read(cx).value().to_string();
                    cx.notify();
                }
            });
        app.scope_subscription = Some(scope_subscription);

        let short = app.short_input.clone();
        let draft_subscription = cx.subscribe_in(&short, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.persist_draft(window, cx);
            }
        });
        app.answer_subscriptions.push(draft_subscription);

        let numeric = app.numeric_input.clone();
        let numeric_subscription =
            cx.subscribe_in(&numeric, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.persist_draft(window, cx);
                }
            });
        app.answer_subscriptions.push(numeric_subscription);

        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.snapshot.settings.theme == ThemeSetting::System {
                this.apply_theme(window, cx);
            }
        });
        app.appearance_subscription = Some(appearance);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh(cx);
            }
        });
        app.activation_subscription = Some(activation);
        let tick_service = app.service.clone();
        let tick = cx.spawn_in(window, async move |this, cx| {
            loop {
                let wait = next_tick_delay(&tick_service);
                cx.background_executor().timer(wait).await;
                let result = this.update_in(cx, |this, window, cx| {
                    if window.is_window_active()
                        && (!(this.route == Route::Study && this.input_focused(window, cx))
                            || this.mock_expired())
                    {
                        this.refresh(cx);
                    }
                });
                if result.is_err() {
                    return;
                }
            }
        });
        app._tick_task = tick;
        app.apply_theme(window, cx);
        app.prepare_question(window, cx);
        app
    }

    pub(crate) fn notice_info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            text: text.into(),
        });
    }

    pub(crate) fn notice_success(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Success,
            text: text.into(),
        });
    }

    pub(crate) fn notice_error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            text: text.into(),
        });
    }

    /// Frees decoded textures that [`ImageCache`] evicted during rendering.
    /// The renderer cannot call `App::remove_asset`, so question preparation
    /// and command handlers drain the queue here.
    pub(crate) fn drain_image_removals(&mut self, cx: &mut Context<Self>) {
        let removed = self.image_cache.borrow_mut().take_removals();
        for image in removed {
            image.remove_asset(cx);
        }
    }

    /// Drops cached images for one workbook after delete or re-import.
    pub(crate) fn invalidate_workbook_images(&mut self, workbook_id: &str, cx: &mut Context<Self>) {
        self.image_cache
            .borrow_mut()
            .invalidate_workbook(workbook_id);
        self.drain_image_removals(cx);
    }

    /// Drops every cached image after a restore or a bulk import.
    pub(crate) fn clear_image_cache(&mut self, cx: &mut Context<Self>) {
        self.image_cache.borrow_mut().clear();
        self.drain_image_removals(cx);
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.service.snapshot() {
            Ok(snapshot) => {
                let had_active = snapshot.active.is_some();
                self.snapshot = snapshot;
                if let Some(active) = self.snapshot.active.clone() {
                    self.study = Some(active);
                } else if self.route != Route::Study {
                    self.study = None;
                } else if !had_active && self.study.is_none() {
                    self.route = Route::Today;
                }
                if let Some(book) = self.selected_book.clone() {
                    self.selected_workbook = self.service.workbook(&book).ok();
                }
                if self.route == Route::History {
                    self.history = self.service.history(200).unwrap_or_default();
                }
                if matches!(
                    self.route,
                    Route::Exams | Route::ExamDetail | Route::GoalDetail
                ) {
                    self.exams = self.service.exams(true).unwrap_or_default();
                    self.goals = self.service.goals(true).unwrap_or_default();
                }
                if let Some(active) = self.snapshot.active.clone()
                    && active.mode == Mode::Mock
                    && active.phase != Phase::OverallResult
                    && active
                        .mock
                        .as_ref()
                        .and_then(|mock| mock.remaining_ms)
                        .is_some_and(|remaining| remaining <= 0)
                    && let Ok(updated) = self
                        .service
                        .submit_mock(&active.session_id, active.revision)
                {
                    self.study = Some(updated);
                    self.route = Route::Study;
                }
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn go(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if self.route == Route::Study && self.study_is_active() && route != Route::Study {
            self.notice_info("学習を終了してから移動してください");
            window.focus(&self.root_focus, cx);
            cx.notify();
            return;
        }
        if route == Route::Scope {
            self.open_scope(window, cx);
            return;
        }
        self.route = route;
        if matches!(route, Route::Today | Route::Library) {
            self.refresh(cx);
        }
        if route == Route::History {
            self.history = self.service.history(200).unwrap_or_default();
        }
        if matches!(route, Route::Exams | Route::ExamDetail | Route::GoalDetail) {
            self.exams = self.service.exams(true).unwrap_or_default();
            self.goals = self.service.goals(true).unwrap_or_default();
        }
        if route == Route::Settings {
            self.sync_settings_inputs(window, cx);
        }
        self.notice = None;
        window.focus(&self.root_focus, cx);
        cx.notify();
    }

    pub(crate) fn study_is_active(&self) -> bool {
        self.study.as_ref().is_some_and(|view| {
            self.snapshot
                .active
                .as_ref()
                .is_some_and(|active| active.session_id == view.session_id)
        })
    }

    pub(crate) fn mock_expired(&self) -> bool {
        self.study.as_ref().is_some_and(|view| {
            view.mode == Mode::Mock
                && view.phase != Phase::OverallResult
                && view
                    .mock
                    .as_ref()
                    .and_then(|mock| mock.remaining_ms)
                    .is_some_and(|remaining| remaining <= 0)
        })
    }

    pub(crate) fn resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.service.active_view() {
            Ok(Some(active)) => {
                self.study = Some(active);
                self.route = Route::Study;
                self.refresh(cx);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Ok(None) => {
                self.refresh(cx);
                self.notice_info("再開できるセッションがありません");
            }
            Err(error) => self.notice_error(error.to_string()),
        }
    }

    pub(crate) fn start(
        &mut self,
        mode: Mode,
        book: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_session_with_budget(mode, book, None, window, cx);
    }

    /// Starts a session using the learner's selected time budget. `None` means
    /// no limit. The budget only shapes the item list; it never interrupts an
    /// answer.
    pub(crate) fn start_with_budget(
        &mut self,
        mode: Mode,
        book: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let budget = self.budget_seconds;
        self.start_session_with_budget(mode, book, budget, window, cx);
    }

    fn start_session_with_budget(
        &mut self,
        mode: Mode,
        book: Option<String>,
        budget: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self
            .service
            .start_session_with_budget(mode, book.as_deref(), budget)
        {
            Ok(()) => {
                self.refresh(cx);
                if let Some(active) = self.snapshot.active.clone() {
                    self.study = Some(active);
                    self.route = Route::Study;
                    self.notice = None;
                    self.prepare_question(window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn open_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let catalog = match self.service.exam_catalog() {
            Ok(catalog) => catalog,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let topics = match self.service.topics() {
            Ok(topics) => topics,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        match self.scope_editor.as_mut() {
            Some(state) => {
                state.catalog = catalog;
                state.topics = topics;
            }
            None => self.scope_editor = Some(ScopeState::new(catalog, topics)),
        }
        self.route = Route::Scope;
        self.notice = None;
        self.refresh_scope_preview(cx);
        window.focus(&self.root_focus, cx);
    }

    pub(crate) fn refresh_scope_preview(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.scope_editor.as_mut() else {
            return;
        };
        let scope = state.scope();
        match self.service.scope_preview(&scope) {
            Ok(preview) => {
                state.preview = Some(preview);
                state.error = None;
            }
            Err(error) => {
                state.preview = None;
                state.error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn reload_scope_data(&mut self, cx: &mut Context<Self>) {
        let catalog = match self.service.exam_catalog() {
            Ok(catalog) => catalog,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let topics = match self.service.topics() {
            Ok(topics) => topics,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        if let Some(state) = self.scope_editor.as_mut() {
            state.catalog = catalog;
            state.topics = topics;
            // Drop selections that no longer exist.
            let workbook_ids: BTreeSet<String> = state
                .catalog
                .iter()
                .map(|q| q.workbook_id.clone())
                .collect();
            let topic_ids: BTreeSet<String> =
                state.topics.iter().map(|topic| topic.id.clone()).collect();
            state
                .selected_workbooks
                .retain(|id| workbook_ids.contains(id));
            state.selected_topics.retain(|id| topic_ids.contains(id));
            state.exclude_topics.retain(|id| topic_ids.contains(id));
            if state
                .parent_topic
                .as_ref()
                .is_some_and(|id| !topic_ids.contains(id))
            {
                state.parent_topic = None;
            }
        }
        self.refresh_scope_preview(cx);
    }

    pub(crate) fn save_scope_topic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.scope_topic_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            if let Some(state) = self.scope_editor.as_mut() {
                state.error = Some("Topic名を入力してください".to_string());
            }
            cx.notify();
            return;
        }
        let (parent, editing) = match self.scope_editor.as_ref() {
            Some(state) => (state.parent_topic.clone(), state.editing_topic.clone()),
            None => (None, None),
        };
        let result = if let Some(id) = editing.as_deref() {
            self.service.update_topic(id, &name, parent.as_deref(), "")
        } else {
            self.service
                .create_topic(&name, parent.as_deref(), "")
                .map(|_| ())
        };
        match result {
            Ok(()) => {
                self.scope_topic_name
                    .update(cx, |state, cx| state.set_value("", window, cx));
                if let Some(state) = self.scope_editor.as_mut() {
                    state.editing_topic = None;
                }
                self.notice_success(if editing.is_some() {
                    "Topicを更新しました"
                } else {
                    "Topicを作成しました"
                });
                self.reload_scope_data(cx);
            }
            Err(error) => {
                if let Some(state) = self.scope_editor.as_mut() {
                    state.error = Some(error.to_string());
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn confirm_delete_topic(&mut self, id: String, cx: &mut Context<Self>) {
        match self.service.delete_topic(&id) {
            Ok(()) => {
                self.modal = None;
                self.notice_success("Topicを削除しました");
                self.reload_scope_data(cx);
            }
            Err(error) => {
                self.modal = None;
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn start_scope(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.scope_editor.as_ref() else {
            return;
        };
        let scope = state.scope();
        let budget = self.budget_seconds;
        let result = match mode {
            Mode::Review => self.service.start_scope_review(&scope, budget),
            Mode::Plan => self.service.start_scope_plan(&scope, budget),
            Mode::Practice => self.service.start_scope_practice(&scope, budget, None),
            Mode::New | Mode::Mock => Err(workbook_service::ServiceError::Invalid(
                "この範囲では復習・計画・練習を開始できます".to_string(),
            )),
        };
        match result {
            Ok(()) => {
                self.refresh(cx);
                if let Some(active) = self.snapshot.active.clone() {
                    self.study = Some(active);
                    self.route = Route::Study;
                    self.notice = None;
                    self.prepare_question(window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn open_editor_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workbook = new_editor_workbook();
        self.open_editor_model(None, workbook, Vec::new(), window, cx);
    }

    pub(crate) fn open_editor_existing(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workbook = match self.service.workbook(&id) {
            Ok(workbook) => workbook,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let assets = self.service.workbook_assets(&id).unwrap_or_default();
        self.open_editor_model(Some(id), workbook, assets, window, cx);
    }

    fn open_editor_model(
        &mut self,
        local_id: Option<String>,
        workbook: Workbook,
        assets: Vec<workbook_service::AssetView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title_value = workbook.title.clone();
        let description_value = workbook.description.clone();
        let tags_value = workbook.tags.join(", ");
        let note_title_value = workbook
            .notes
            .first()
            .map(|note| note.title.clone())
            .unwrap_or_default();
        let note_card_value = workbook
            .notes
            .first()
            .map(|note| content_editor_text(&note.knowledge_card))
            .unwrap_or_default();
        let question = workbook
            .notes
            .first()
            .and_then(|note| note.questions.first());
        let prompt_value = question
            .and_then(|question| question.prompt.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let hint_value = question
            .and_then(|question| question.hint.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let explanation_value = question
            .map(|question| content_editor_text(&question.explanation))
            .unwrap_or_default();
        let front_value = question
            .and_then(|question| question.front.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let back_value = question
            .and_then(|question| question.back.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let answer_value = question.map(editor_answer_text).unwrap_or_default();
        let topics_value = question
            .map(|question| question.topics.join(", "))
            .unwrap_or_default();
        let advanced_value = question
            .map(|question| serde_json::to_string_pretty(question).unwrap_or_default())
            .unwrap_or_default();
        let option_values: Vec<String> = (0..6)
            .map(|index| {
                question
                    .and_then(|question| question.options.as_ref()?.get(index))
                    .map(|option| content_editor_text(&option.content))
                    .unwrap_or_default()
            })
            .collect();

        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_value));
        let description = cx.new(|cx| InputState::new(window, cx).default_value(description_value));
        let tags = cx.new(|cx| InputState::new(window, cx).default_value(tags_value));
        let note_title = cx.new(|cx| InputState::new(window, cx).default_value(note_title_value));
        let note_card = cx.new(|cx| InputState::new(window, cx).default_value(note_card_value));
        let prompt = cx.new(|cx| InputState::new(window, cx).default_value(prompt_value));
        let hint = cx.new(|cx| InputState::new(window, cx).default_value(hint_value));
        let explanation = cx.new(|cx| InputState::new(window, cx).default_value(explanation_value));
        let front = cx.new(|cx| InputState::new(window, cx).default_value(front_value));
        let back = cx.new(|cx| InputState::new(window, cx).default_value(back_value));
        let answer = cx.new(|cx| InputState::new(window, cx).default_value(answer_value));
        let topics = cx.new(|cx| InputState::new(window, cx).default_value(topics_value));
        let advanced = cx.new(|cx| InputState::new(window, cx).default_value(advanced_value));
        let option_inputs = option_values
            .into_iter()
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)))
            .collect::<Vec<_>>();

        let mut state = WorkbookEditorState {
            local_id,
            workbook,
            note_index: 0,
            question_index: 0,
            assets,
            syncing: false,
            error: None,
            issues: Vec::new(),
            title,
            description,
            tags,
            note_title,
            note_card,
            prompt,
            hint,
            explanation,
            front,
            back,
            answer,
            topics,
            advanced,
            option_inputs,
            _subscriptions: Vec::new(),
        };
        let mut subscriptions = Vec::new();
        let inputs: Vec<Entity<InputState>> = vec![
            state.title.clone(),
            state.description.clone(),
            state.tags.clone(),
            state.note_title.clone(),
            state.note_card.clone(),
            state.prompt.clone(),
            state.hint.clone(),
            state.explanation.clone(),
            state.front.clone(),
            state.back.clone(),
            state.answer.clone(),
            state.topics.clone(),
        ];
        for input in inputs
            .into_iter()
            .chain(state.option_inputs.iter().cloned())
        {
            subscriptions.push(cx.subscribe_in(&input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_editor_fields(cx);
                }
            }));
        }
        state._subscriptions = subscriptions;
        self.editor = Some(state);
        self.route = Route::Editor;
        self.notice = None;
        self.sync_editor_inputs(window, cx);
        window.focus(&self.root_focus, cx);
        cx.notify();
    }

    pub(crate) fn sync_editor_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.syncing = true;
        let workbook_title = editor.workbook.title.clone();
        let workbook_description = editor.workbook.description.clone();
        let workbook_tags = editor.workbook.tags.join(", ");
        let note = editor.workbook.notes.get(editor.note_index);
        let note_title = note.map(|note| note.title.clone()).unwrap_or_default();
        let note_card = note
            .map(|note| content_editor_text(&note.knowledge_card))
            .unwrap_or_default();
        let question = note.and_then(|note| note.questions.get(editor.question_index));
        let prompt = question
            .and_then(|question| question.prompt.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let hint = question
            .and_then(|question| question.hint.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let explanation = question
            .map(|question| content_editor_text(&question.explanation))
            .unwrap_or_default();
        let front = question
            .and_then(|question| question.front.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let back = question
            .and_then(|question| question.back.as_ref())
            .map(content_editor_text)
            .unwrap_or_default();
        let answer = question.map(editor_answer_text).unwrap_or_default();
        let topics = question
            .map(|question| question.topics.join(", "))
            .unwrap_or_default();
        let advanced = question
            .map(|question| serde_json::to_string_pretty(question).unwrap_or_default())
            .unwrap_or_default();
        let option_values: Vec<String> = (0..6)
            .map(|index| {
                question
                    .and_then(|question| question.options.as_ref()?.get(index))
                    .map(|option| content_editor_text(&option.content))
                    .unwrap_or_default()
            })
            .collect();
        let handles = vec![
            (editor.title.clone(), workbook_title),
            (editor.description.clone(), workbook_description),
            (editor.tags.clone(), workbook_tags),
            (editor.note_title.clone(), note_title),
            (editor.note_card.clone(), note_card),
            (editor.prompt.clone(), prompt),
            (editor.hint.clone(), hint),
            (editor.explanation.clone(), explanation),
            (editor.front.clone(), front),
            (editor.back.clone(), back),
            (editor.answer.clone(), answer),
            (editor.topics.clone(), topics),
            (editor.advanced.clone(), advanced),
        ];
        for (input, value) in handles {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        for (input, value) in editor.option_inputs.iter().cloned().zip(option_values) {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        editor.syncing = false;
    }

    pub(crate) fn apply_editor_fields(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.syncing {
            return;
        }
        let title = editor.title.read(cx).value().to_string();
        let description = editor.description.read(cx).value().to_string();
        let tags = editor.tags.read(cx).value().to_string();
        let note_title = editor.note_title.read(cx).value().to_string();
        let note_card = editor.note_card.read(cx).value().to_string();
        let prompt = editor.prompt.read(cx).value().to_string();
        let hint = editor.hint.read(cx).value().to_string();
        let explanation = editor.explanation.read(cx).value().to_string();
        let front = editor.front.read(cx).value().to_string();
        let back = editor.back.read(cx).value().to_string();
        let answer = editor.answer.read(cx).value().to_string();
        let topics = editor.topics.read(cx).value().to_string();
        let option_values: Vec<String> = editor
            .option_inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect();

        editor.workbook.title = title;
        editor.workbook.description = description;
        editor.workbook.tags = tags
            .split([',', '、'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let note_index = editor.note_index;
        let question_index = editor.question_index;
        let Some(note) = editor.workbook.notes.get_mut(note_index) else {
            return;
        };
        note.title = note_title;
        if let Err(error) = set_editor_content(&mut note.knowledge_card, &note_card) {
            editor.error = Some(error);
            return;
        }
        let Some(question) = note.questions.get_mut(question_index) else {
            return;
        };
        // Required contents keep an empty Text so Domain validation can point
        // at the exact field instead of silently accepting an absent value.
        if question.kind == QuestionType::Flashcard {
            question.prompt = None;
            if let Err(error) = set_editor_optional_content(&mut question.front, &front, true) {
                editor.error = Some(error);
                return;
            }
            if let Err(error) = set_editor_optional_content(&mut question.back, &back, true) {
                editor.error = Some(error);
                return;
            }
        } else {
            if let Err(error) = set_editor_optional_content(&mut question.prompt, &prompt, true) {
                editor.error = Some(error);
                return;
            }
            question.front = None;
            question.back = None;
        }
        if let Err(error) = set_editor_optional_content(&mut question.hint, &hint, false) {
            editor.error = Some(error);
            return;
        }
        if let Err(error) = set_editor_content(&mut question.explanation, &explanation) {
            editor.error = Some(error);
            return;
        }
        if let Some(options) = question.options.as_mut() {
            let count = options.len();
            for (index, value) in option_values.iter().take(count).enumerate() {
                if let Some(option) = options.get_mut(index)
                    && let Err(error) = set_editor_content(&mut option.content, value)
                {
                    editor.error = Some(error);
                    return;
                }
            }
        }
        question.topics = topics
            .split([',', '、'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if let Err(error) = set_editor_answer(question, &answer) {
            editor.error = Some(error);
            return;
        }
        editor.error = None;
    }

    pub(crate) fn select_editor_note(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.editor.as_mut() {
            editor.note_index = index;
            editor.question_index = 0;
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn select_editor_question(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.editor.as_mut() {
            editor.question_index = index;
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn set_editor_kind(
        &mut self,
        kind: QuestionType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_fields(cx);
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let note_index = editor.note_index;
        let question_index = editor.question_index;
        let first_asset = editor.assets.first().map(|asset| asset.id.clone());
        if let Some(question) = editor
            .workbook
            .notes
            .get_mut(note_index)
            .and_then(|note| note.questions.get_mut(question_index))
        {
            switch_editor_question_kind(question, kind, first_asset.as_deref());
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn add_editor_asset(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("画像", &["png", "jpg", "jpeg", "svg"])
            .pick_file()
        else {
            return;
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notice_error(format!("画像を読み込めません: {error}"));
                cx.notify();
                return;
            }
        };
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let media_type = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "svg" => "image/svg+xml",
            _ => {
                self.notice_error("PNG / JPEG / SVG のみ追加できます");
                cx.notify();
                return;
            }
        };
        let (width, height) = match workbook_domain::inspect_image(&bytes, media_type, "assets") {
            Ok(size) => size,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let id = format!("asset-{}", uuid::Uuid::new_v4().simple());
        let alt = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("画像")
            .to_string();
        let data = workbook_domain::encode_base64(&bytes);
        let fingerprint = workbook_service::asset_fingerprint(media_type, width, height, &bytes);
        if let Some(editor) = self.editor.as_mut() {
            editor.workbook.assets.push(workbook_domain::AssetRef {
                id: id.clone(),
                alt: alt.clone(),
                media_type: media_type.to_string(),
                file: None,
                data: Some(data),
            });
            editor.assets.push(workbook_service::AssetView {
                id,
                media_type: media_type.to_string(),
                alt,
                width,
                height,
                fingerprint,
                bytes: Arc::new(bytes),
            });
        }
        self.notice_success("画像を追加しました。Content JSON の asset で参照できます");
        cx.notify();
    }

    pub(crate) fn add_editor_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_fields(cx);
        let workbook_id = self
            .editor
            .as_ref()
            .map(|editor| editor.workbook.id.clone())
            .unwrap_or_default();
        if let Some(editor) = self.editor.as_mut() {
            let index = editor.workbook.notes.len();
            let question = new_editor_question(
                "single_choice",
                &format!("{workbook_id}-q-{}", uuid::Uuid::new_v4().simple()),
                editor.assets.first().map(|asset| asset.id.as_str()),
            );
            editor.workbook.notes.push(Note {
                id: format!("note-{}", uuid::Uuid::new_v4().simple()),
                title: format!("ノート{}", index + 1),
                knowledge_card: Content::Text("この単元の知識カード".to_string()),
                questions: vec![question],
            });
            editor.note_index = index;
            editor.question_index = 0;
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn add_editor_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_fields(cx);
        let workbook_id = self
            .editor
            .as_ref()
            .map(|editor| editor.workbook.id.clone())
            .unwrap_or_default();
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let first_asset = editor.assets.first().map(|asset| asset.id.clone());
            if let Some(note) = editor.workbook.notes.get_mut(note_index) {
                let index = note.questions.len();
                note.questions.push(new_editor_question(
                    "single_choice",
                    &format!("{workbook_id}-q-{}", uuid::Uuid::new_v4().simple()),
                    first_asset.as_deref(),
                ));
                editor.question_index = index;
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn remove_editor_note(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_fields(cx);
        if let Some(editor) = self.editor.as_mut() {
            if editor.workbook.notes.len() <= 1 || index >= editor.workbook.notes.len() {
                editor.error = Some("Noteは1つ以上必要です".to_string());
                cx.notify();
                return;
            }
            editor.workbook.notes.remove(index);
            editor.note_index = editor.note_index.min(editor.workbook.notes.len() - 1);
            editor.question_index = 0;
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn remove_editor_question(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_fields(cx);
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            if let Some(note) = editor.workbook.notes.get_mut(note_index) {
                if note.questions.len() <= 1 || index >= note.questions.len() {
                    editor.error = Some("NoteにはQuestionが1つ以上必要です".to_string());
                    cx.notify();
                    return;
                }
                note.questions.remove(index);
                editor.question_index = editor.question_index.min(note.questions.len() - 1);
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn add_editor_option(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_fields(cx);
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let question_index = editor.question_index;
            if let Some(question) = editor
                .workbook
                .notes
                .get_mut(note_index)
                .and_then(|note| note.questions.get_mut(question_index))
            {
                let options = question.options.get_or_insert_with(Vec::new);
                if options.len() >= 6 {
                    editor.error = Some("選択肢は6個までです".to_string());
                } else {
                    let id = option_id(options.len());
                    options.push(ChoiceOption {
                        id,
                        content: Content::Text(format!("選択肢 {}", options.len() + 1)),
                    });
                    refresh_ordering_answer(question);
                }
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn remove_editor_option(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_fields(cx);
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let question_index = editor.question_index;
            if let Some(question) = editor
                .workbook
                .notes
                .get_mut(note_index)
                .and_then(|note| note.questions.get_mut(question_index))
            {
                let current_single = question.single_choice_answer().map(ToOwned::to_owned);
                if let Some(options) = question.options.as_mut() {
                    if options.len() <= 2 {
                        editor.error = Some("選択肢は2個以上必要です".to_string());
                    } else {
                        let removed = options.remove(index);
                        if current_single.as_deref() == Some(removed.id.as_str()) {
                            question.answer = options
                                .first()
                                .map(|option| serde_json::json!({"optionId": option.id}));
                        }
                        if let Some(values) = question
                            .answer
                            .as_mut()
                            .and_then(|answer| answer.get_mut("optionIds"))
                            .and_then(|value| value.as_array_mut())
                        {
                            values.retain(|value| value.as_str() != Some(removed.id.as_str()));
                            if values.is_empty()
                                && let Some(first) = options.first()
                            {
                                values.push(serde_json::Value::String(first.id.clone()));
                            }
                        }
                        refresh_ordering_answer(question);
                    }
                }
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn move_editor_option(
        &mut self,
        index: usize,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_fields(cx);
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let question_index = editor.question_index;
            if let Some(question) = editor
                .workbook
                .notes
                .get_mut(note_index)
                .and_then(|note| note.questions.get_mut(question_index))
                && let Some(options) = question.options.as_mut()
            {
                let target = index as isize + delta;
                if target >= 0 && (target as usize) < options.len() {
                    options.swap(index, target as usize);
                    refresh_ordering_answer(question);
                }
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn set_editor_option_answer(
        &mut self,
        id: String,
        multiple: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let question_index = editor.question_index;
            if let Some(question) = editor
                .workbook
                .notes
                .get_mut(note_index)
                .and_then(|note| note.questions.get_mut(question_index))
            {
                if multiple {
                    let mut values = question.multiple_choice_answers();
                    if let Some(position) = values.iter().position(|value| value == &id) {
                        if values.len() > 1 {
                            values.remove(position);
                        }
                    } else {
                        values.push(id);
                    }
                    question.answer = Some(serde_json::json!({"optionIds": values}));
                } else {
                    question.answer = Some(serde_json::json!({"optionId": id}));
                }
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn set_editor_true_false(
        &mut self,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.editor.as_mut() {
            let note_index = editor.note_index;
            let question_index = editor.question_index;
            if let Some(question) = editor
                .workbook
                .notes
                .get_mut(note_index)
                .and_then(|note| note.questions.get_mut(question_index))
            {
                question.answer = Some(serde_json::Value::Bool(value));
            }
        }
        self.sync_editor_inputs(window, cx);
        cx.notify();
    }

    pub(crate) fn apply_editor_advanced(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_fields(cx);
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let raw = editor.advanced.read(cx).value().to_string();
        let question: Question = match serde_json::from_str(&raw) {
            Ok(question) => question,
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(format!("問題JSONが不正です: {error}"));
                }
                cx.notify();
                return;
            }
        };
        let mut model = editor.workbook.clone();
        let note_index = editor.note_index;
        let question_index = editor.question_index;
        if let Some(slot) = model
            .notes
            .get_mut(note_index)
            .and_then(|note| note.questions.get_mut(question_index))
        {
            *slot = question;
        }
        match self.service.validate_workbook_model(&model) {
            Ok(()) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.workbook = model;
                    editor.error = None;
                }
                self.sync_editor_inputs(window, cx);
                self.notice_success("問題JSONを適用しました");
            }
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn validate_editor(&mut self, cx: &mut Context<Self>) {
        self.apply_editor_fields(cx);
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.error.is_some())
        {
            cx.notify();
            return;
        }
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let workbook = editor.workbook.clone();
        let domain = self.service.validate_workbook_model(&workbook).err();
        let issues = self.service.lint_workbook_json(&workbook.to_json());
        if let Some(editor) = self.editor.as_mut() {
            editor.error = domain.map(|error| error.to_string());
            editor.issues = issues;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.error.is_none())
        {
            self.notice_success("検証に成功しました");
        }
        cx.notify();
    }

    pub(crate) fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.apply_editor_fields(cx);
        // Consume the confirmation at the start. An early validation return
        // must not leave the next unrelated save silently confirmed.
        let confirm_reset = self.fsrs_reset_confirmed;
        self.fsrs_reset_confirmed = false;
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.error.is_some())
        {
            cx.notify();
            return false;
        }
        let Some(editor) = self.editor.as_ref() else {
            return false;
        };
        let workbook = editor.workbook.clone();
        let local_id = editor.local_id.clone();
        if let Err(error) = self.service.validate_workbook_model(&workbook) {
            if let Some(editor) = self.editor.as_mut() {
                editor.error = Some(error.to_string());
                editor.issues = Vec::new();
            }
            cx.notify();
            return false;
        }
        let issues = self.service.lint_workbook_json(&workbook.to_json());
        let blocking = issues.iter().any(WorkbookIssue::is_error);
        if let Some(editor) = self.editor.as_mut() {
            editor.issues = issues;
        }
        if blocking {
            if let Some(editor) = self.editor.as_mut() {
                editor.error =
                    Some("Linterがエラーを検出しました。項目を修正してください".to_string());
            }
            cx.notify();
            return false;
        }
        if let Some(id) = local_id.as_deref()
            && !confirm_reset
        {
            match self.service.preview_workbook_save(id, &workbook) {
                Ok(resets) if !resets.is_empty() => {
                    self.modal = Some(Modal::ResetFsrs { questions: resets });
                    cx.notify();
                    return false;
                }
                Ok(_) => {}
                Err(error) => {
                    if let Some(editor) = self.editor.as_mut() {
                        editor.error = Some(error.to_string());
                    }
                    cx.notify();
                    return false;
                }
            }
        }
        match self
            .service
            .save_workbook(local_id.as_deref(), &workbook, confirm_reset)
        {
            Ok(saved) => {
                let changed = local_id.as_deref() != Some(saved.local_id.as_str());
                let assets = self
                    .service
                    .workbook_assets(&saved.local_id)
                    .unwrap_or_default();
                if let Some(editor) = self.editor.as_mut() {
                    editor.local_id = Some(saved.local_id.clone());
                    editor.workbook = saved.workbook;
                    editor.assets = assets;
                    editor.error = None;
                }
                self.selected_book = Some(saved.local_id.clone());
                self.selected_workbook = self.service.workbook(&saved.local_id).ok();
                if changed {
                    self.clear_image_cache(cx);
                } else {
                    self.invalidate_workbook_images(&saved.local_id, cx);
                }
                self.refresh(cx);
                self.notice_success("教材を保存しました");
                self.sync_editor_inputs(window, cx);
                cx.notify();
                true
            }
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(error.to_string());
                }
                cx.notify();
                false
            }
        }
    }

    pub(crate) fn confirm_fsrs_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fsrs_reset_confirmed = true;
        self.modal = None;
        if !self.save_editor(window, cx) {
            self.fsrs_reset_confirmed = false;
        }
        cx.notify();
    }

    pub(crate) fn export_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_editor(window, cx) {
            return;
        }
        let Some(local_id) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.local_id.clone())
        else {
            return;
        };
        let raw = match self.service.export_workbook_json(&local_id) {
            Ok(raw) => raw,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let title = self
            .editor
            .as_ref()
            .map(|editor| editor.workbook.title.clone())
            .unwrap_or_else(|| "workbook".to_string());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{title}.workbook.json"))
            .add_filter("Workbook JSON", &["json"])
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, raw) {
            Ok(()) => self.notice_success(format!("書き出しました: {}", path.display())),
            Err(error) => self.notice_error(format!("書き出しに失敗しました: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn copy_selected_workbook(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_book.clone() else {
            return;
        };
        match self.service.copy_workbook(&id) {
            Ok(copy) => {
                self.selected_book = Some(copy);
                self.selected_workbook = self
                    .service
                    .workbook(&self.selected_book.clone().unwrap())
                    .ok();
                self.refresh(cx);
                self.notice_success("教材をコピーしました");
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn open_book(&mut self, id: String, cx: &mut Context<Self>) {
        match self.service.workbook(&id) {
            Ok(workbook) => {
                self.selected_book = Some(id);
                self.selected_workbook = Some(workbook);
                self.route = Route::BookDetail;
                self.notice = None;
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn open_exam(&mut self, id: String, cx: &mut Context<Self>) {
        match self.service.exams(true) {
            Ok(exams) => {
                self.exams = exams;
                self.selected_exam = Some(id);
                self.route = Route::ExamDetail;
                self.notice = None;
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn selected_exam_summary(&self) -> Option<&ExamSummary> {
        self.selected_exam
            .as_ref()
            .and_then(|id| self.exams.iter().find(|summary| &summary.exam.id == id))
    }

    pub(crate) fn selected_goal_summary(&self) -> Option<&GoalSummary> {
        self.selected_goal
            .as_ref()
            .and_then(|id| self.goals.iter().find(|summary| &summary.goal.id == id))
    }

    pub(crate) fn open_goal(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.goals = self.service.goals(true).unwrap_or_default();
        let Some(summary) = self
            .goals
            .iter()
            .find(|summary| summary.goal.id == id)
            .cloned()
        else {
            self.notice_error("目標が見つかりません");
            cx.notify();
            return;
        };
        self.selected_goal = Some(id);
        let title = summary.goal.title.clone();
        let date = summary
            .goal
            .target_date
            .map(|date| date.to_string())
            .unwrap_or_default();
        let minutes = summary.goal.daily_minutes.to_string();
        self.goal_title_input
            .update(cx, |state, cx| state.set_value(title, window, cx));
        self.goal_date_input
            .update(cx, |state, cx| state.set_value(date, window, cx));
        self.goal_minutes_input
            .update(cx, |state, cx| state.set_value(minutes, window, cx));
        self.route = Route::GoalDetail;
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn create_goal_from_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.scope_editor.as_ref() else {
            self.notice_error("範囲を選択してください");
            cx.notify();
            return;
        };
        let scope = state.scope();
        if !scope.has_inclusion_rules() {
            self.notice_error("Workbook・Topic・個別問題のいずれかを含めてください");
            cx.notify();
            return;
        }
        let title = self.goal_title_input.read(cx).value().trim().to_string();
        let date_text = self.goal_date_input.read(cx).value().trim().to_string();
        let target_date = if date_text.is_empty() {
            None
        } else {
            match chrono::NaiveDate::parse_from_str(&date_text, "%Y-%m-%d") {
                Ok(date) => Some(date),
                Err(_) => {
                    self.notice_error("期限は YYYY-MM-DD で入力してください");
                    cx.notify();
                    return;
                }
            }
        };
        let daily_minutes = self
            .goal_minutes_input
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(30);
        let draft = workbook_domain::GoalDraft {
            title,
            description: None,
            target_date,
            target_score: None,
            daily_minutes,
            scope,
        };
        match self.service.create_goal(draft) {
            Ok(id) => {
                self.notice_success("Goalを作成しました");
                self.goals = self.service.goals(true).unwrap_or_default();
                self.open_goal(id, window, cx);
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn save_goal_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_goal.clone() else {
            return;
        };
        let Some(summary) = self.selected_goal_summary().cloned() else {
            return;
        };
        let title = self.goal_title_input.read(cx).value().trim().to_string();
        let date_text = self.goal_date_input.read(cx).value().trim().to_string();
        let target_date = if date_text.is_empty() {
            None
        } else {
            match chrono::NaiveDate::parse_from_str(&date_text, "%Y-%m-%d") {
                Ok(date) => Some(date),
                Err(_) => {
                    self.notice_error("期限は YYYY-MM-DD で入力してください");
                    cx.notify();
                    return;
                }
            }
        };
        let daily_minutes = self
            .goal_minutes_input
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .unwrap_or(30);
        let draft = workbook_domain::GoalDraft {
            title,
            description: summary.goal.description.clone(),
            target_date,
            target_score: summary.goal.target_score,
            daily_minutes,
            scope: summary.goal.scope.clone(),
        };
        match self.service.update_goal(&id, draft) {
            Ok(()) => {
                self.notice_success("Goalを更新しました");
                self.goals = self.service.goals(true).unwrap_or_default();
                self.open_goal(id, window, cx);
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn set_goal_archive(&mut self, archived: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected_goal.clone() else {
            return;
        };
        self.toggle_goal_archive(id, archived, cx);
    }

    pub(crate) fn toggle_goal_archive(
        &mut self,
        id: String,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        match self.service.archive_goal(&id, archived) {
            Ok(()) => {
                self.notice_success(if archived {
                    "Goalをアーカイブしました"
                } else {
                    "Goalを再開しました"
                });
                self.goals = self.service.goals(true).unwrap_or_default();
                self.exams = self.service.exams(true).unwrap_or_default();
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn confirm_delete_goal(&mut self, id: String, cx: &mut Context<Self>) {
        match self.service.delete_goal(&id) {
            Ok(()) => {
                self.modal = None;
                self.selected_goal = None;
                self.goals = self.service.goals(true).unwrap_or_default();
                self.route = Route::Exams;
                self.notice_success("Goalを削除しました");
                cx.notify();
            }
            Err(error) => {
                self.modal = None;
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn start_goal(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_goal.clone() else {
            return;
        };
        let result = match mode {
            Mode::Plan => self.service.start_goal_plan(&id),
            Mode::Review => self.service.start_goal_review(&id),
            Mode::Practice => self.service.start_goal_practice(&id),
            Mode::New | Mode::Mock => Err(workbook_service::ServiceError::Invalid(
                "Goalからは計画・復習・練習を開始できます".to_string(),
            )),
        };
        match result {
            Ok(()) => {
                self.refresh(cx);
                if let Some(active) = self.snapshot.active.clone() {
                    self.study = Some(active);
                    self.route = Route::Study;
                    self.notice = None;
                    self.prepare_question(window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn open_exam_editor(
        &mut self,
        exam_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let catalog = match self.service.exam_catalog() {
            Ok(catalog) => catalog,
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let existing = match exam_id.as_deref() {
            Some(id) => match self.service.exam(id) {
                Ok(exam) => Some(exam),
                Err(error) => {
                    self.notice_error(error.to_string());
                    cx.notify();
                    return;
                }
            },
            None => None,
        };
        let title = existing
            .as_ref()
            .map(|exam| exam.title.clone())
            .unwrap_or_default();
        let description = existing
            .as_ref()
            .and_then(|exam| exam.description.clone())
            .unwrap_or_default();
        let target_date = existing
            .as_ref()
            .and_then(|exam| exam.target_date)
            .map(|date| date.to_string())
            .unwrap_or_default();
        let target_score = existing
            .as_ref()
            .and_then(|exam| exam.target_score)
            .map(|score| format!("{score}"))
            .unwrap_or_default();
        let daily_minutes = existing
            .as_ref()
            .map(|exam| exam.daily_minutes)
            .unwrap_or_else(workbook_domain::exam::default_daily_minutes)
            .to_string();
        let (groups, include, exclude, exclude_tags, exclude_workbooks) = existing
            .as_ref()
            .map(|exam| {
                (
                    exam.scope
                        .groups
                        .iter()
                        .map(|group| ScopeGroupDraft {
                            workbook_ids: group.workbook_ids.iter().cloned().collect(),
                            tags: group.tags.iter().cloned().collect(),
                        })
                        .collect::<Vec<_>>(),
                    exam.scope
                        .include_questions
                        .iter()
                        .map(|reference| {
                            QuestionKey::new(
                                reference.workbook_id.clone(),
                                reference.question_id.clone(),
                            )
                        })
                        .collect::<BTreeSet<_>>(),
                    exam.scope
                        .exclude_questions
                        .iter()
                        .map(|reference| {
                            QuestionKey::new(
                                reference.workbook_id.clone(),
                                reference.question_id.clone(),
                            )
                        })
                        .collect::<BTreeSet<_>>(),
                    exam.scope.exclude_tags.iter().cloned().collect(),
                    exam.scope.exclude_workbooks.iter().cloned().collect(),
                )
            })
            .unwrap_or_else(|| {
                (
                    vec![ScopeGroupDraft::default()],
                    BTreeSet::new(),
                    BTreeSet::new(),
                    BTreeSet::new(),
                    BTreeSet::new(),
                )
            });
        let title_input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let description_input = cx.new(|cx| InputState::new(window, cx).default_value(description));
        let target_date_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("YYYY-MM-DD（任意）")
                .default_value(target_date)
        });
        let target_score_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("目標スコア（任意）")
                .default_value(target_score)
        });
        let daily_minutes_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1日の学習時間（分）")
                .default_value(daily_minutes)
        });
        let question_search = cx.new(|cx| InputState::new(window, cx).placeholder("問題を検索"));
        self.exam_editor = Some(ExamEditorState {
            exam_id,
            title: title_input,
            description: description_input,
            target_date: target_date_input,
            target_score: target_score_input,
            daily_minutes: daily_minutes_input,
            groups,
            include,
            exclude,
            exclude_tags,
            exclude_workbooks,
            catalog,
            preview_count: 0,
            preview_error: None,
            question_search,
        });
        self.refresh_exam_preview(cx);
        self.route = Route::ExamEditor;
        self.notice = None;
        cx.notify();
    }

    fn editor_scope(&self) -> Option<ExamScope> {
        let editor = self.exam_editor.as_ref()?;
        Some(ExamScope {
            groups: editor
                .groups
                .iter()
                .filter(|group| !group.workbook_ids.is_empty() || !group.tags.is_empty())
                .map(|group| ExamScopeGroup {
                    workbook_ids: group.workbook_ids.iter().cloned().collect(),
                    tags: group.tags.iter().cloned().collect(),
                    topics: Vec::new(),
                })
                .collect(),
            include_questions: editor
                .include
                .iter()
                .map(|key| QuestionRef::new(key.workbook_id.clone(), key.question_id.clone()))
                .collect(),
            exclude_questions: editor
                .exclude
                .iter()
                .map(|key| QuestionRef::new(key.workbook_id.clone(), key.question_id.clone()))
                .collect(),
            exclude_workbooks: editor.exclude_workbooks.iter().cloned().collect(),
            exclude_tags: editor.exclude_tags.iter().cloned().collect(),
            exclude_topics: Vec::new(),
        })
    }

    pub(crate) fn refresh_exam_preview(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.editor_scope() else {
            return;
        };
        match self.service.preview_exam_scope(&scope) {
            Ok(questions) => {
                if let Some(editor) = self.exam_editor.as_mut() {
                    editor.preview_count = questions.len();
                    editor.preview_error = None;
                }
            }
            Err(error) => {
                if let Some(editor) = self.exam_editor.as_mut() {
                    editor.preview_error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn save_exam(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.exam_editor.as_ref() else {
            return;
        };
        let title = editor.title.read(cx).value().to_string();
        let description = editor.description.read(cx).value().to_string();
        let date_text = editor.target_date.read(cx).value().to_string();
        let score_text = editor.target_score.read(cx).value().to_string();
        let minutes_text = editor.daily_minutes.read(cx).value().to_string();
        let target_date = if date_text.trim().is_empty() {
            None
        } else {
            match chrono::NaiveDate::parse_from_str(date_text.trim(), "%Y-%m-%d") {
                Ok(date) => Some(date),
                Err(_) => {
                    self.notice_error("目標日は YYYY-MM-DD 形式で入力してください");
                    window.focus(&self.root_focus, cx);
                    cx.notify();
                    return;
                }
            }
        };
        let target_score = if score_text.trim().is_empty() {
            None
        } else {
            match score_text.trim().parse::<f64>() {
                Ok(score) if score.is_finite() && score >= 0.0 => Some(score),
                _ => {
                    self.notice_error("目標スコアは0以上の数値で入力してください");
                    window.focus(&self.root_focus, cx);
                    cx.notify();
                    return;
                }
            }
        };
        let daily_minutes = if minutes_text.trim().is_empty() {
            workbook_domain::exam::default_daily_minutes()
        } else {
            match minutes_text.trim().parse::<u32>() {
                Ok(minutes) if minutes <= 1_440 => minutes,
                _ => {
                    self.notice_error("1日の学習時間は0〜1440分で入力してください");
                    window.focus(&self.root_focus, cx);
                    cx.notify();
                    return;
                }
            }
        };
        let Some(scope) = self.editor_scope() else {
            return;
        };
        let description = description.trim();
        let draft = ExamDraft {
            title,
            description: (!description.is_empty()).then(|| description.to_string()),
            target_date,
            target_score,
            daily_minutes,
            scope,
        };
        let exam_id = editor.exam_id.clone();
        let result = match exam_id.as_deref() {
            Some(id) => self.service.update_exam(id, draft).map(|_| id.to_string()),
            None => self.service.create_exam(draft),
        };
        match result {
            Ok(id) => {
                self.open_exam(id, cx);
                self.notice_success("試験を保存しました");
            }
            Err(error) => {
                self.notice_error(error.to_string());
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
        }
    }

    fn enter_started_study(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(cx);
        if let Some(active) = self.snapshot.active.clone() {
            self.study = Some(active);
            self.route = Route::Study;
            self.notice = None;
            self.prepare_question(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn start_exam_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_exam.clone() else {
            return;
        };
        match self.service.start_exam_plan(&id) {
            Ok(()) => self.enter_started_study(window, cx),
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn start_exam_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_exam.clone() else {
            return;
        };
        match self.service.start_exam_review(&id) {
            Ok(()) => self.enter_started_study(window, cx),
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn start_exam_practice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_exam.clone() else {
            return;
        };
        let limit_text = self.practice_limit_input.read(cx).value().to_string();
        let question_limit = match parse_optional_u32(&limit_text) {
            Ok(value) => value,
            Err(()) => {
                self.notice_error("問題数は0以上の整数で入力してください");
                cx.notify();
                return;
            }
        };
        let options = PracticeOptions {
            strategy: self.practice_strategy,
            question_limit,
        };
        match self.service.start_exam_practice(&id, &options) {
            Ok(()) => self.enter_started_study(window, cx),
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn start_exam_mock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_exam.clone() else {
            return;
        };
        let limit_text = self.mock_limit_input.read(cx).value().to_string();
        let minutes_text = self.mock_minutes_input.read(cx).value().to_string();
        let question_limit = match parse_optional_u32(&limit_text) {
            Ok(value) => value,
            Err(()) => {
                self.notice_error("問題数は0以上の整数で入力してください");
                cx.notify();
                return;
            }
        };
        let minutes = match parse_optional_u32(&minutes_text) {
            Ok(value) => value,
            Err(()) => {
                self.notice_error("制限時間は0以上の整数で入力してください");
                cx.notify();
                return;
            }
        };
        let options = MockOptions {
            question_limit,
            time_limit_seconds: minutes.map(|minutes| minutes.saturating_mul(60)),
            order: if self.mock_random {
                MockOrder::Random
            } else {
                MockOrder::Fixed
            },
        };
        match self.service.start_exam_mock(&id, &options) {
            Ok(()) => self.enter_started_study(window, cx),
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn submit_mock_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        if view.phase == Phase::WorkbookResult {
            return self.submit_mock_session(window, cx);
        }
        let Some(question) = view.question.as_ref() else {
            return;
        };
        if question.kind == QuestionType::Flashcard && !view.revealed {
            self.reveal_flashcard(window, cx);
            return;
        }
        let answer = if question.kind == QuestionType::Flashcard {
            Some(Answer::Bool(true))
        } else {
            self.current_answer(question, cx)
        };
        if answer_is_blank(answer.as_ref()) {
            self.notice_info("回答を入力してください");
            cx.notify();
            return;
        }
        match self
            .service
            .mock_answer(&view.session_id, answer.as_ref(), view.revision)
        {
            Ok(updated) => {
                self.study = Some(updated);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn submit_mock_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.submit_mock(&view.session_id, view.revision) {
            Ok(updated) => {
                self.study = Some(updated);
                self.modal = None;
                self.refresh(cx);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn toggle_exam_archive(
        &mut self,
        id: String,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        match self.service.archive_exam(&id, archived) {
            Ok(()) => {
                self.notice_success(if archived {
                    "試験をアーカイブしました"
                } else {
                    "試験を再開しました"
                });
                self.exams = self.service.exams(true).unwrap_or_default();
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn confirm_delete_exam(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.delete_exam(&id) {
            Ok(()) => {
                self.modal = None;
                self.exams = self.service.exams(true).unwrap_or_default();
                self.selected_exam = None;
                self.notice_success("試験を削除しました");
                self.go(Route::Exams, window, cx);
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn begin_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.begin_question(&view.session_id, view.revision) {
            Ok(view) => {
                self.study = Some(view);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    fn current_answer(&self, question: &Question, cx: &App) -> Option<Answer> {
        match question.kind {
            QuestionType::SingleChoice => self.single_choice.clone().map(Answer::Text),
            QuestionType::MultipleChoice => {
                if self.multiple_choice.is_empty() {
                    None
                } else {
                    Some(Answer::Choices(
                        self.multiple_choice.iter().cloned().collect(),
                    ))
                }
            }
            QuestionType::TrueFalse => self.true_false.map(Answer::Bool),
            QuestionType::ShortText => {
                Some(Answer::Text(self.short_input.read(cx).value().to_string()))
            }
            QuestionType::Cloze => {
                let mut values = std::collections::BTreeMap::new();
                for (id, input) in &self.cloze_inputs {
                    values.insert(id.clone(), input.read(cx).value().to_string());
                }
                Some(Answer::Blanks(values))
            }
            QuestionType::Numeric => Some(Answer::Text(
                self.numeric_input.read(cx).value().to_string(),
            )),
            QuestionType::Ordering => Some(Answer::Ordering {
                order: self.ordering.clone(),
            }),
            QuestionType::Matching => Some(Answer::Matching {
                matches: self.matching.clone(),
            }),
            QuestionType::Hotspot => self.hotspot_point.map(|(x, y)| Answer::Hotspot { x, y }),
            QuestionType::Labeling => Some(Answer::Labels {
                labels: self.labeling.clone(),
            }),
            QuestionType::Flashcard => self.self_correct.map(Answer::Bool),
        }
    }

    pub(crate) fn persist_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        if view.phase != Phase::Question {
            return;
        }
        let Some(question) = view.question.as_ref() else {
            return;
        };
        let answer = self.current_answer(question, cx);
        match self.service.save_draft(
            &view.session_id,
            answer.as_ref(),
            view.hint_visible,
            view.revealed,
            view.revision,
        ) {
            Ok(updated) => {
                self.study = Some(updated);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn save_hint(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        let Some(question) = view.question.as_ref() else {
            return;
        };
        let answer = self.current_answer(question, cx);
        match self.service.save_draft(
            &view.session_id,
            answer.as_ref(),
            true,
            view.revealed,
            view.revision,
        ) {
            Ok(updated) => {
                self.study = Some(updated);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn reveal_flashcard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.save_draft(
            &view.session_id,
            None,
            view.hint_visible,
            true,
            view.revision,
        ) {
            Ok(updated) => {
                self.study = Some(updated);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn submit_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        if view.mode == Mode::Mock {
            return self.submit_mock_question(window, cx);
        }
        match view.phase {
            Phase::Intro => self.begin_question(window, cx),
            Phase::Feedback => {
                if view.feedback.as_ref().is_some_and(|feedback| {
                    feedback.status == workbook_domain::GradeStatus::NearMiss
                }) {
                    self.retry_question(window, cx);
                    return;
                }
                if view
                    .feedback
                    .as_ref()
                    .is_some_and(|feedback| !feedback.committed)
                {
                    let Some(suggested) = view
                        .feedback
                        .as_ref()
                        .and_then(|feedback| feedback.suggested_rating)
                    else {
                        self.rating_picker_open = true;
                        self.notice_info("この問題は自動評価できません。評価を選んでください");
                        cx.notify();
                        return;
                    };
                    self.rate_question(suggested, window, cx);
                } else {
                    self.next_question(window, cx);
                }
            }
            Phase::Question => {
                let Some(question) = view.question.clone() else {
                    return;
                };
                if question.kind == QuestionType::Flashcard {
                    if !view.revealed {
                        self.reveal_flashcard(window, cx);
                        return;
                    }
                    let Some(correct) = self.self_correct else {
                        self.notice_info("「思い出せた」か「思い出せなかった」を選んでください");
                        cx.notify();
                        return;
                    };
                    let answer = Some(Answer::Bool(correct));
                    return self.evaluate(view, answer, Some(correct), false, window, cx);
                }
                let answer = self.current_answer(&question, cx);
                if answer.is_none()
                    && matches!(
                        question.kind,
                        QuestionType::SingleChoice
                            | QuestionType::MultipleChoice
                            | QuestionType::TrueFalse
                    )
                {
                    self.notice_info("回答を選んでください");
                    cx.notify();
                    return;
                }
                if answer_is_blank(answer.as_ref()) {
                    self.notice_info("回答を入力してください");
                    cx.notify();
                    return;
                }
                self.evaluate(view, answer, None, false, window, cx);
            }
            Phase::WorkbookResult | Phase::OverallResult => {}
        }
    }

    fn evaluate(
        &mut self,
        view: StudyView,
        answer: Option<Answer>,
        self_correct: Option<bool>,
        unknown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.evaluate_answer(
            &view.session_id,
            answer.as_ref(),
            unknown,
            self_correct,
            view.hint_visible,
            view.revision,
        ) {
            Ok(updated) => {
                self.rating_picker_open = updated.feedback.as_ref().is_some_and(|feedback| {
                    !feedback.committed && feedback.suggested_rating.is_none()
                });
                self.study = Some(updated);
                self.refresh(cx);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn retry_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.retry_answer(&view.session_id, view.revision) {
            Ok(updated) => {
                self.rating_picker_open = false;
                self.study = Some(updated);
                self.focus_answer(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn mark_unknown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        if view.phase != Phase::Question || view.question.is_none() {
            return;
        }
        self.evaluate(view, None, None, true, window, cx);
    }

    pub(crate) fn toggle_rating_picker(&mut self, cx: &mut Context<Self>) {
        self.rating_picker_open = !self.rating_picker_open;
        cx.notify();
    }

    pub(crate) fn rate_question(
        &mut self,
        rating: Rating,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.study.clone() else {
            return;
        };
        let Some(feedback) = view.feedback.as_ref() else {
            return;
        };
        if feedback.status == workbook_domain::GradeStatus::NearMiss {
            return;
        }
        let result = if feedback.committed {
            match feedback.attempt_id.as_deref() {
                Some(attempt_id) => self.service.correct_attempt(
                    &view.session_id,
                    attempt_id,
                    None,
                    Some(rating),
                    view.revision,
                ),
                None => return,
            }
        } else {
            self.service.commit_rating(
                &view.session_id,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
        };
        match result {
            Ok(updated) => {
                self.rating_picker_open = false;
                self.study = Some(updated);
                self.refresh(cx);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn toggle_correct(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        let Some(feedback) = view.feedback.as_ref() else {
            return;
        };
        let (Some(attempt_id), Some(correct)) = (feedback.attempt_id.as_ref(), feedback.correct)
        else {
            return;
        };
        match self.service.correct_attempt(
            &view.session_id,
            attempt_id,
            Some(!correct),
            None,
            view.revision,
        ) {
            Ok(updated) => {
                self.study = Some(updated);
                self.refresh(cx);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn next_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.next(&view.session_id, view.revision) {
            Ok(updated) => {
                self.study = Some(updated);
                self.refresh(cx);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn skip_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.skip(&view.session_id, view.revision) {
            Ok(updated) => {
                self.study = Some(updated);
                self.refresh(cx);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn continue_workbook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self
            .service
            .continue_session(&view.session_id, view.revision)
        {
            Ok(updated) => {
                self.study = Some(updated);
                self.refresh(cx);
                self.prepare_question(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn finish_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.clone() else {
            return;
        };
        match self.service.finish(&view.session_id, view.revision) {
            Ok(updated) => {
                self.study = Some(updated);
                self.modal = None;
                self.refresh(cx);
                window.focus(&self.root_focus, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn prepare_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.single_choice = None;
        self.multiple_choice.clear();
        self.true_false = None;
        self.self_correct = None;
        self.ordering.clear();
        self.matching.clear();
        self.matching_left = None;
        self.hotspot_point = None;
        self.hotspot_image_bounds.set(None);
        self.hotspot_list_open = false;
        self.labeling.clear();
        self.labeling_label = None;
        self.labeling_image_bounds.set(None);
        // Images are intentionally not cleared here: the LRU cache is keyed by
        // workbook, asset and content fingerprint, so switching questions
        // reuses decoded textures. Only the next question's asset bytes are
        // re-read from SQLite.
        self.image_cache.borrow_mut().trim();
        self.drain_image_removals(cx);
        self.rating_picker_open = false;
        self.answer_subscriptions.clear();
        self.answer_subscriptions.push(cx.subscribe_in(
            &self.short_input.clone(),
            window,
            |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.persist_draft(window, cx);
                }
            },
        ));
        self.answer_subscriptions.push(cx.subscribe_in(
            &self.numeric_input.clone(),
            window,
            |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.persist_draft(window, cx);
                }
            },
        ));
        self.cloze_inputs.clear();
        let Some(view) = self.study.clone() else {
            window.focus(&self.root_focus, cx);
            return;
        };
        if view.phase != Phase::Question {
            window.focus(&self.root_focus, cx);
            return;
        }
        let Some(question) = view.question.clone() else {
            window.focus(&self.root_focus, cx);
            return;
        };
        match question.kind {
            QuestionType::SingleChoice => {
                if let Some(Answer::Text(value)) = view.draft.clone() {
                    self.single_choice = Some(value);
                }
            }
            QuestionType::MultipleChoice => {
                if let Some(Answer::Choices(values)) = view.draft.clone() {
                    self.multiple_choice = values.into_iter().collect();
                }
            }
            QuestionType::TrueFalse => {
                if let Some(Answer::Bool(value)) = view.draft.clone() {
                    self.true_false = Some(value);
                }
            }
            QuestionType::ShortText => {
                let value = match view.draft.clone() {
                    Some(Answer::Text(value)) => value,
                    _ => String::new(),
                };
                let input = self.short_input.clone();
                input.update(cx, |state, cx| state.set_value(value, window, cx));
            }
            QuestionType::Cloze => {
                let blanks = question.cloze_blanks();
                let draft = match view.draft.clone() {
                    Some(Answer::Blanks(values)) => values,
                    _ => Default::default(),
                };
                for id in blanks.keys() {
                    let value = draft.get(id).cloned().unwrap_or_default();
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(SharedString::from(format!("［{id}］")))
                            .default_value(value)
                    });
                    let subscription =
                        cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                            if matches!(event, InputEvent::Change) {
                                this.persist_draft(window, cx);
                            }
                        });
                    self.answer_subscriptions.push(subscription);
                    self.cloze_inputs.push((id.clone(), input));
                }
            }
            QuestionType::Flashcard => {
                self.self_correct = if view.revealed {
                    match view.draft.clone() {
                        Some(Answer::Bool(value)) => Some(value),
                        _ => None,
                    }
                } else {
                    None
                };
            }
            QuestionType::Numeric => {
                let value = match view.draft.clone() {
                    Some(Answer::Text(value)) => value,
                    _ => String::new(),
                };
                let input = self.numeric_input.clone();
                input.update(cx, |state, cx| state.set_value(value, window, cx));
            }
            QuestionType::Ordering => {
                let from_draft = match view.draft.clone() {
                    Some(Answer::Ordering { order }) => Some(order),
                    _ => None,
                };
                self.ordering = from_draft.unwrap_or_else(|| question.display_order());
            }
            QuestionType::Matching => {
                if let Some(Answer::Matching { matches }) = view.draft.clone() {
                    self.matching = matches;
                }
            }
            QuestionType::Hotspot => {
                if let Some(Answer::Hotspot { x, y }) = view.draft.clone() {
                    self.hotspot_point = Some((x, y));
                }
            }
            QuestionType::Labeling => {
                if let Some(Answer::Labels { labels }) = view.draft.clone() {
                    self.labeling = labels;
                }
            }
        }
        self.focus_answer(window, cx);
    }

    fn focus_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.study.as_ref() else {
            window.focus(&self.root_focus, cx);
            return;
        };
        if view.phase != Phase::Question {
            window.focus(&self.root_focus, cx);
            return;
        }
        match view.question.as_ref().map(|question| question.kind) {
            Some(QuestionType::ShortText) => {
                let input = self.short_input.clone();
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            Some(QuestionType::Numeric) => {
                let input = self.numeric_input.clone();
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            Some(QuestionType::Cloze) => {
                if let Some((_, input)) = self.cloze_inputs.first() {
                    let input = input.clone();
                    input.update(cx, |state, cx| state.focus(window, cx));
                } else {
                    window.focus(&self.root_focus, cx);
                }
            }
            _ => window.focus(&self.root_focus, cx),
        }
    }

    fn input_focused(&self, window: &Window, cx: &App) -> bool {
        let mut focused = self
            .short_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            || self
                .numeric_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .search_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .settings_retention
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .settings_new_limit
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .scope_search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .scope_topic_name
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .goal_title_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .goal_date_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .goal_minutes_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        focused |= self
            .practice_limit_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            || self
                .mock_limit_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            || self
                .mock_minutes_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        if let Some(editor) = self.exam_editor.as_ref() {
            focused |= editor.title.read(cx).focus_handle(cx).is_focused(window)
                || editor
                    .description
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                || editor
                    .target_date
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                || editor
                    .target_score
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                || editor
                    .question_search
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
        }
        focused |= self
            .cloze_inputs
            .iter()
            .any(|(_, input)| input.read(cx).focus_handle(cx).is_focused(window));
        focused
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let platform = event.keystroke.modifiers.platform;
        if key == "escape" {
            if self.modal.is_some() {
                self.modal = None;
            } else if self.route == Route::Study && self.study_is_active() {
                self.modal = Some(Modal::Finish);
            } else {
                self.notice = None;
            }
            cx.notify();
            return;
        }
        if platform {
            match key {
                "1" => {
                    self.go(Route::Today, window, cx);
                    return;
                }
                "2" => {
                    self.go(Route::Library, window, cx);
                    return;
                }
                "3" => {
                    self.go(Route::History, window, cx);
                    return;
                }
                "4" => {
                    self.go(Route::Exams, window, cx);
                    return;
                }
                "5" => {
                    self.go(Route::Scope, window, cx);
                    return;
                }
                "," => {
                    self.go(Route::Settings, window, cx);
                    return;
                }
                "o" => {
                    self.import_json_dialog(window, cx);
                    return;
                }
                "f" => {
                    self.go(Route::Library, window, cx);
                    if self.route == Route::Library {
                        self.search_input
                            .update(cx, |state, cx| state.focus(window, cx));
                    }
                    cx.notify();
                    return;
                }
                "enter" | "return" => {
                    self.submit_answer(window, cx);
                    return;
                }
                _ => {}
            }
        }
        if self.route == Route::Study
            && !self.input_focused(window, cx)
            && let Some(rating) = match key {
                "1" => Some(Rating::Again),
                "2" => Some(Rating::Hard),
                "3" => Some(Rating::Good),
                "4" => Some(Rating::Easy),
                _ => None,
            }
            && self.study.as_ref().is_some_and(|view| {
                view.phase == Phase::Feedback
                    && view.feedback.as_ref().is_some_and(|feedback| {
                        feedback.status != workbook_domain::GradeStatus::NearMiss
                            && (!feedback.committed || self.rating_picker_open)
                    })
            })
        {
            self.rate_question(rating, window, cx);
        }
    }

    pub(crate) fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = match self.snapshot.settings.theme {
            ThemeSetting::Light => ThemeMode::Light,
            ThemeSetting::Dark => ThemeMode::Dark,
            ThemeSetting::System => match window.appearance() {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
                _ => ThemeMode::Light,
            },
        };
        Theme::change(mode, None, cx);
        let dark = mode == ThemeMode::Dark;
        Theme::update(cx, |theme| {
            theme.radius = px(2.0);
            theme.radius_lg = px(6.0);
            theme.shadow = false;
            theme.font_size = px(16.0);
            theme.mono_font_size = px(13.0);
            if dark {
                theme.background = rgb(0x0B0B0B).into();
                theme.foreground = rgb(0xFFFFFF).into();
                theme.muted_foreground = rgb(0xAEB4B9).into();
                theme.border = rgb(0x383838).into();
                let accent = Hsla::from(rgb(0x1BA1E2));
                theme.accent = accent;
                theme.accent_foreground = rgb(0x0B0B0B).into();
                theme.primary = accent;
                theme.primary_hover = rgb(0x2AAEEA).into();
                theme.primary_active = rgb(0x0F8BC7).into();
                theme.primary_foreground = rgb(0x0B0B0B).into();
                theme.button_primary = accent;
                theme.button_primary_hover = rgb(0x2AAEEA).into();
                theme.button_primary_active = rgb(0x0F8BC7).into();
                theme.button_primary_foreground = rgb(0x0B0B0B).into();
                theme.link = accent;
                theme.ring = accent;
            } else {
                theme.background = rgb(0xF7F8F9).into();
                theme.foreground = rgb(0x17191B).into();
                theme.muted_foreground = rgb(0x626A70).into();
                theme.border = rgb(0xDFE3E6).into();
                let accent = Hsla::from(rgb(0x006D91));
                theme.accent = accent;
                theme.accent_foreground = rgb(0xFFFFFF).into();
                theme.primary = accent;
                theme.primary_hover = rgb(0x005A78).into();
                theme.primary_active = rgb(0x004A63).into();
                theme.primary_foreground = rgb(0xFFFFFF).into();
                theme.button_primary = accent;
                theme.button_primary_hover = rgb(0x005A78).into();
                theme.button_primary_active = rgb(0x004A63).into();
                theme.button_primary_foreground = rgb(0xFFFFFF).into();
                theme.link = accent;
                theme.ring = accent;
            }
        });
        cx.notify();
    }

    pub(crate) fn sync_settings_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let retention = format!("{:.2}", self.snapshot.settings.retention);
        let new_limit = self.snapshot.settings.new_limit.to_string();
        self.settings_retention
            .update(cx, |state, cx| state.set_value(retention, window, cx));
        self.settings_new_limit
            .update(cx, |state, cx| state.set_value(new_limit, window, cx));
    }

    pub(crate) fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let retention = self
            .settings_retention
            .read(cx)
            .value()
            .to_string()
            .parse::<f32>();
        let new_limit = self
            .settings_new_limit
            .read(cx)
            .value()
            .to_string()
            .parse::<u32>();
        let (Ok(retention), Ok(new_limit)) = (retention, new_limit) else {
            self.notice_error("定着率と新規問題数を数値で入力してください");
            cx.notify();
            return;
        };
        let mut settings = self.snapshot.settings.clone();
        settings.retention = retention;
        settings.new_limit = new_limit;
        match self.service.update_settings(settings) {
            Ok(()) => {
                self.refresh(cx);
                self.apply_theme(window, cx);
                self.sync_settings_inputs(window, cx);
                self.notice_success("設定を保存しました");
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn set_theme(
        &mut self,
        theme: ThemeSetting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut settings = self.snapshot.settings.clone();
        settings.theme = theme;
        match self.service.update_settings(settings) {
            Ok(()) => {
                self.refresh(cx);
                self.apply_theme(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn set_automatic_rating(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let mut settings = self.snapshot.settings.clone();
        settings.automatic_rating = enabled;
        match self.service.update_settings(settings) {
            Ok(()) => {
                self.refresh(cx);
                self.notice_success(if enabled {
                    "自動評価を有効にしました"
                } else {
                    "自動評価を無効にしました"
                });
                cx.notify();
            }
            Err(error) => {
                self.notice_error(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn import_json_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Workbook JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        match self.service.preview_workbook_file(&path) {
            Ok(workbook) => {
                let issues = std::fs::read_to_string(&path)
                    .map(|raw| {
                        self.service
                            .lint_workbook_json(&raw)
                            .into_iter()
                            .filter(|issue| issue.severity != "info")
                            .map(|issue| {
                                format!(
                                    "{} {} · {}: {}",
                                    issue.severity, issue.code, issue.path, issue.message
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.modal = Some(Modal::ImportJson {
                    workbook,
                    path: path.clone(),
                    source: path.display().to_string(),
                    issues,
                });
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn confirm_import(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.import_workbook_file(&path) {
            Ok(_) => {
                self.clear_image_cache(cx);
                self.modal = None;
                self.refresh(cx);
                self.notice_success("教材をインポートしました");
                window.focus(&self.root_focus, cx);
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn import_sample(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        match self
            .service
            .import_workbook_json(workbook_service::SAMPLE_WORKBOOK_JSON)
        {
            Ok(_) => {
                self.clear_image_cache(cx);
                self.refresh(cx);
                self.notice_success("サンプル教材を追加しました");
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn import_expanded_sample(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        match self
            .service
            .import_workbook_json(workbook_service::SAMPLE_WORKBOOK_V2_JSON)
        {
            Ok(_) => {
                self.clear_image_cache(cx);
                self.refresh(cx);
                self.notice_success("拡張問題形式のサンプル教材を追加しました");
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn confirm_delete(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.delete_workbook(&id) {
            Ok(()) => {
                self.invalidate_workbook_images(&id, cx);
                self.modal = None;
                if self.selected_book.as_deref() == Some(id.as_str()) {
                    self.selected_book = None;
                    self.selected_workbook = None;
                    self.route = Route::Library;
                }
                self.refresh(cx);
                self.notice_success("教材を削除しました（削除前のバックアップを保存済み）");
                window.focus(&self.root_focus, cx);
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn backup_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("Workbook2-backup.sqlite")
            .add_filter("Workbook 2 backup", &["sqlite"])
            .save_file()
        else {
            return;
        };
        match self.service.backup_to(&path) {
            Ok(()) => {
                self.notice_success(format!("バックアップを保存しました: {}", path.display()))
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        window.focus(&self.root_focus, cx);
        cx.notify();
    }

    pub(crate) fn restore_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Workbook 2 backup", &["sqlite"])
            .pick_file()
        else {
            return;
        };
        match WorkbookService::validate_backup(&path) {
            Ok(()) => {
                self.modal = Some(Modal::Restore {
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string()),
                    path,
                });
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn confirm_restore(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.restore_from(&path) {
            Ok(()) => {
                self.clear_image_cache(cx);
                self.modal = None;
                self.route = Route::Today;
                self.study = None;
                self.selected_book = None;
                self.selected_workbook = None;
                self.refresh(cx);
                self.notice_success("バックアップから復元しました");
                window.focus(&self.root_focus, cx);
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn legacy_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("旧Workbook DB", &["sqlite", "db"])
            .pick_file()
        else {
            return;
        };
        match WorkbookService::legacy_preview(&path) {
            Ok(preview) => {
                self.modal = Some(Modal::Legacy { path, preview });
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn confirm_legacy(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.service.import_legacy(&path) {
            Ok(preview) => {
                self.clear_image_cache(cx);
                self.modal = None;
                self.refresh(cx);
                let warnings = if preview.warnings.is_empty() {
                    String::new()
                } else {
                    format!("（{}件の注意）", preview.warnings.len())
                };
                self.notice_success(format!(
                    "旧データを移行しました: 教材{} / 問題{} / 回答{}{warnings}",
                    preview.workbook_count, preview.question_count, preview.attempt_count
                ));
                window.focus(&self.root_focus, cx);
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.service.reset_history() {
            Ok(()) => {
                self.modal = None;
                self.refresh(cx);
                self.notice_success("学習履歴をリセットしました（教材は残っています）");
                window.focus(&self.root_focus, cx);
            }
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn open_data_folder(&mut self, cx: &mut Context<Self>) {
        let path = self.snapshot.data_path.clone();
        let result = if path.is_empty() || path == "(memory)" {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "データフォルダがありません",
            ))
        } else {
            std::process::Command::new("open")
                .arg(&path)
                .spawn()
                .map(|_| ())
        };
        match result {
            Ok(()) => self.notice_info("データフォルダを開きました"),
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn check_integrity(&mut self, cx: &mut Context<Self>) {
        match self.service.integrity() {
            Ok(status) if status == "ok" => self.notice_success("整合性チェック: ok"),
            Ok(status) => self.notice_error(format!("整合性チェック: {status}")),
            Err(error) => self.notice_error(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn clear_notice(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn today_date(&self) -> String {
        let tz = self.snapshot.timezone.parse::<Tz>().unwrap_or(Tz::UTC);
        self.service
            .now()
            .with_timezone(&tz)
            .format("%Y年%-m月%-d日")
            .to_string()
    }

    pub(crate) fn selected_summary(&self) -> Option<&BookSummary> {
        self.selected_book
            .as_ref()
            .and_then(|id| self.snapshot.books.iter().find(|book| &book.local_id == id))
    }
}

impl Render for WorkbookApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut root = div()
            .id("workbook-app-root")
            .relative()
            .track_focus(&self.root_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground);

        let body = if self.route == Route::Study {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.render_study(cx))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_row()
                .size_full()
                .child(self.render_sidebar(cx))
                .child(
                    div()
                        .id("content-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .h_full()
                        .overflow_y_scroll()
                        .child(self.render_content(cx)),
                )
                .into_any_element()
        };
        root = root.child(body);

        if let Some(modal) = self.modal.clone() {
            root = root.child(self.render_modal(modal, cx));
        }
        root
    }
}

fn new_editor_workbook() -> Workbook {
    let workbook_id = uuid::Uuid::new_v4().to_string();
    Workbook {
        format: "workbook".to_string(),
        schema_version: 2,
        id: workbook_id.clone(),
        revision: 1,
        title: "新しい教材".to_string(),
        description: String::new(),
        language: "ja".to_string(),
        tags: Vec::new(),
        assets: Vec::new(),
        topic_catalog: Vec::new(),
        notes: vec![Note {
            id: format!("note-{}", uuid::Uuid::new_v4().simple()),
            title: "ノート1".to_string(),
            knowledge_card: Content::Text("この単元で説明する内容".to_string()),
            questions: vec![new_editor_question(
                "single_choice",
                &format!("{workbook_id}-q-{}", uuid::Uuid::new_v4().simple()),
                None,
            )],
        }],
    }
}

fn new_editor_question(kind: &str, id: &str, _first_asset: Option<&str>) -> Question {
    let kind = kind
        .parse::<QuestionType>()
        .unwrap_or(QuestionType::SingleChoice);
    Question {
        id: id.to_string(),
        revision: 1,
        kind,
        prompt: Some(Content::Text("問題文".to_string())),
        hint: None,
        explanation: Content::Text("解説".to_string()),
        options: None,
        answer: None,
        front: None,
        back: None,
        initial_order: None,
        matching: None,
        hotspot: None,
        labeling: None,
        topics: Vec::new(),
    }
}

fn option_content(index: usize) -> Content {
    Content::Text(format!("選択肢 {}", index + 1))
}

fn option_id(index: usize) -> String {
    ["a", "b", "c", "d", "e", "f"]
        .get(index)
        .map(|value| (*value).to_string())
        .unwrap_or_else(|| format!("o{index}"))
}

fn switch_editor_question_kind(question: &mut Question, kind: QuestionType, asset: Option<&str>) {
    let prompt = question.prompt.clone();
    let hint = question.hint.clone();
    let explanation = question.explanation.clone();
    let topics = question.topics.clone();
    let front = question.front.clone();
    let back = question.back.clone();
    question.kind = kind;
    question.options = None;
    question.answer = None;
    question.initial_order = None;
    question.matching = None;
    question.hotspot = None;
    question.labeling = None;
    question.front = None;
    question.back = None;
    if prompt.is_some() {
        question.prompt = prompt;
    }
    question.hint = hint;
    question.explanation = explanation;
    question.topics = topics;
    if kind != QuestionType::Flashcard && question.prompt.is_none() {
        question.prompt = Some(Content::Text("問題文".to_string()));
    }
    match kind {
        QuestionType::SingleChoice => {
            let options: Vec<ChoiceOption> = (0..2)
                .map(|index| ChoiceOption {
                    id: option_id(index),
                    content: option_content(index),
                })
                .collect();
            question.answer = Some(serde_json::json!({"optionId": options[0].id.clone()}));
            question.options = Some(options);
        }
        QuestionType::TrueFalse => {
            question.answer = Some(serde_json::Value::Bool(true));
            question.prompt = question
                .prompt
                .clone()
                .or_else(|| Some(Content::Text("正しいか、まちがいか".to_string())));
        }
        QuestionType::MultipleChoice => {
            let options: Vec<ChoiceOption> = (0..2)
                .map(|index| ChoiceOption {
                    id: option_id(index),
                    content: option_content(index),
                })
                .collect();
            question.answer = Some(serde_json::json!({"optionIds": [options[0].id.clone()]}));
            question.options = Some(options);
        }
        QuestionType::ShortText => {
            question.answer = Some(serde_json::json!({"accepted": ["正解"]}));
        }
        QuestionType::Cloze => {
            question.prompt = Some(Content::Text("空欄は {{blank:1}} です".to_string()));
            question.answer = Some(serde_json::json!({"blanks": {"1": ["正解"]}}));
        }
        QuestionType::Flashcard => {
            question.front = front.or_else(|| Some(Content::Text("表面".to_string())));
            question.back = back.or_else(|| Some(Content::Text("裏面".to_string())));
            question.prompt = None;
        }
        QuestionType::Numeric => {
            question.answer = Some(serde_json::json!({"value": 0}));
        }
        QuestionType::Ordering => {
            let options: Vec<ChoiceOption> = (0..3)
                .map(|index| ChoiceOption {
                    id: option_id(index),
                    content: option_content(index),
                })
                .collect();
            question.answer = Some(serde_json::json!({
                "orders": [options.iter().map(|option| option.id.clone()).collect::<Vec<_>>()]
            }));
            question.options = Some(options);
        }
        QuestionType::Matching => {
            question.matching = Some(
                serde_json::from_value(serde_json::json!({
                    "left": [
                        {"id":"l1","content":"左1"},
                        {"id":"l2","content":"左2"}
                    ],
                    "right": [
                        {"id":"r1","content":"右1"},
                        {"id":"r2","content":"右2"}
                    ]
                }))
                .expect("matching template"),
            );
            question.answer = Some(serde_json::json!({
                "pairs": [{"left":"l1","right":"r1"},{"left":"l2","right":"r2"}],
                "alternatives": []
            }));
        }
        QuestionType::Hotspot => {
            let asset = asset.unwrap_or("").to_string();
            question.hotspot = Some(
                serde_json::from_value(serde_json::json!({
                    "asset": asset,
                    "regions": [
                        {"id":"r1","alt":"領域1","shape":{"type":"rect","x":0.1,"y":0.1,"w":0.3,"h":0.3}},
                        {"id":"r2","alt":"領域2","shape":{"type":"rect","x":0.5,"y":0.5,"w":0.2,"h":0.2}}
                    ]
                }))
                .expect("hotspot template"),
            );
            question.answer = Some(serde_json::json!({"regionIds": ["r1"]}));
        }
        QuestionType::Labeling => {
            let asset = asset.unwrap_or("").to_string();
            question.labeling = Some(
                serde_json::from_value(serde_json::json!({
                    "asset": asset,
                    "targets": [
                        {"id":"t1","x":0.25,"y":0.25,"alt":"配置先1"},
                        {"id":"t2","x":0.75,"y":0.75,"alt":"配置先2"}
                    ],
                    "labels": [
                        {"id":"lab1","content":"ラベル1"},
                        {"id":"lab2","content":"ラベル2"}
                    ]
                }))
                .expect("labeling template"),
            );
            question.answer = Some(serde_json::json!({
                "pairs": {"t1":"lab1","t2":"lab2"},
                "alternatives": []
            }));
        }
    }
}

fn content_editor_text(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn parse_editor_content(raw: &str) -> std::result::Result<Content, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str::<Content>(trimmed)
            .map_err(|error| format!("ContentのJSONが不正です: {error}"))
    } else {
        Ok(Content::Text(raw.to_string()))
    }
}

fn set_editor_content(slot: &mut Content, raw: &str) -> std::result::Result<(), String> {
    let content = parse_editor_content(raw)?;
    *slot = content;
    Ok(())
}

fn set_editor_optional_content(
    slot: &mut Option<Content>,
    raw: &str,
    required: bool,
) -> std::result::Result<(), String> {
    if raw.trim().is_empty() {
        if required {
            *slot = Some(Content::Text(String::new()));
        } else {
            *slot = None;
        }
        return Ok(());
    }
    *slot = Some(parse_editor_content(raw)?);
    Ok(())
}

fn editor_answer_text(question: &Question) -> String {
    match question.kind {
        QuestionType::ShortText => question.short_text_answers().join(" | "),
        QuestionType::Cloze => question
            .cloze_blanks()
            .into_iter()
            .map(|(id, values)| format!("{id}: {}", values.join(" | ")))
            .collect::<Vec<_>>()
            .join("; "),
        QuestionType::Numeric => match question.numeric_spec() {
            Some(spec) => {
                let value = match &spec.value {
                    workbook_domain::NumericLiteral::Number(value) => {
                        if (value - value.round()).abs() < 1e-9 {
                            format!("{}", value.round() as i64)
                        } else {
                            value.to_string()
                        }
                    }
                    workbook_domain::NumericLiteral::Fraction(text) => text.clone(),
                };
                let unit = spec.unit.unwrap_or_default();
                let sig = spec
                    .significant_figures
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                format!("{value}|{unit}|{sig}")
            }
            None => String::new(),
        },
        _ => String::new(),
    }
}

fn set_editor_answer(question: &mut Question, raw: &str) -> std::result::Result<(), String> {
    match question.kind {
        QuestionType::ShortText => {
            let accepted: Vec<String> = raw
                .split('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            question.answer = Some(serde_json::json!({"accepted": accepted}));
        }
        QuestionType::Cloze => {
            let mut blanks = serde_json::Map::new();
            for part in raw
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let Some((id, values)) = part.split_once(':') else {
                    return Err(
                        "穴埋めの正解は「1: 答え | 別解」の形式で入力してください".to_string()
                    );
                };
                let values: Vec<String> = values
                    .split('|')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                blanks.insert(
                    id.trim().to_string(),
                    serde_json::Value::Array(
                        values.into_iter().map(serde_json::Value::String).collect(),
                    ),
                );
            }
            question.answer = Some(serde_json::json!({"blanks": blanks}));
        }
        QuestionType::Numeric => {
            let parts: Vec<&str> = raw.split('|').map(str::trim).collect();
            let value_text = parts.first().copied().unwrap_or("");
            if value_text.is_empty() {
                question.answer = Some(serde_json::json!({"value": 0}));
                return Ok(());
            }
            let value = if let Ok(number) = value_text.parse::<f64>() {
                serde_json::Value::from(number)
            } else {
                serde_json::Value::String(value_text.to_string())
            };
            let unit = parts.get(1).copied().unwrap_or("");
            let sig = parts.get(2).copied().unwrap_or("");
            let mut spec = serde_json::Map::new();
            spec.insert("value".to_string(), value);
            if !unit.is_empty() {
                spec.insert(
                    "unit".to_string(),
                    serde_json::Value::String(unit.to_string()),
                );
            }
            if let Ok(sig) = sig.parse::<u32>() {
                spec.insert(
                    "significantFigures".to_string(),
                    serde_json::Value::from(sig),
                );
            }
            question.answer = Some(serde_json::Value::Object(spec));
        }
        _ => {}
    }
    Ok(())
}

fn refresh_ordering_answer(question: &mut Question) {
    if question.kind != QuestionType::Ordering {
        return;
    }
    let ids: Vec<String> = question
        .options
        .as_ref()
        .map(|options| options.iter().map(|option| option.id.clone()).collect())
        .unwrap_or_default();
    question.answer = Some(serde_json::json!({"orders": [ids]}));
}

fn parse_optional_u32(value: &str) -> std::result::Result<Option<u32>, ()> {
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        value.parse::<u32>().map(Some).map_err(|_| ())
    }
}

fn next_tick_delay(service: &WorkbookService) -> std::time::Duration {
    let now = chrono::Utc::now();
    let wait = service
        .next_wake_at()
        .ok()
        .flatten()
        .map(|due| {
            if due <= now {
                std::time::Duration::from_secs(1)
            } else {
                (due - now)
                    .to_std()
                    .unwrap_or_else(|_| std::time::Duration::from_secs(30))
            }
        })
        .unwrap_or_else(|| std::time::Duration::from_secs(30));
    wait.max(std::time::Duration::from_secs(1))
        .min(std::time::Duration::from_secs(600))
}

pub fn open_window(service: Arc<WorkbookService>, cx: &mut gpui_kit::App) -> anyhow::Result<()> {
    let bounds = gpui_kit::Bounds::centered(None, size(px(1120.0), px(780.0)), cx);
    gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(720.0), px(560.0))),
            titlebar: Some(gpui_kit::TitlebarOptions {
                title: Some("Workbook 2".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        cx,
        move |window, cx| {
            let service = service.clone();
            cx.new(|cx| WorkbookApp::new(service, window, cx))
        },
    )?;
    Ok(())
}
