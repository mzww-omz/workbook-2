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
    Answer, ExamDraft, ExamScope, ExamScopeGroup, MockOptions, MockOrder, Mode, Phase,
    PracticeOptions, PracticeStrategy, Question, QuestionRef, QuestionType, Rating, Settings,
    ThemeSetting, Workbook, answer_is_blank,
};
use workbook_import::LegacyPreview;
use workbook_service::{
    BookSummary, ExamSummary, Snapshot, StudyView, TodaySummary, WorkbookService,
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

pub struct WorkbookApp {
    pub(crate) service: Arc<WorkbookService>,
    pub(crate) route: Route,
    pub(crate) snapshot: Snapshot,
    pub(crate) history: Vec<workbook_service::HistoryEntry>,
    pub(crate) study: Option<StudyView>,
    pub(crate) selected_book: Option<String>,
    pub(crate) selected_workbook: Option<Workbook>,
    pub(crate) exams: Vec<ExamSummary>,
    pub(crate) selected_exam: Option<String>,
    pub(crate) exam_editor: Option<ExamEditorState>,
    pub(crate) scope_editor: Option<ScopeState>,
    pub(crate) scope_search_text: String,
    pub(crate) scope_search: Entity<InputState>,
    pub(crate) scope_topic_name: Entity<InputState>,
    pub(crate) scope_subscription: Option<Subscription>,
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

        let mut app = Self {
            service,
            route,
            snapshot,
            history,
            study,
            selected_book: None,
            selected_workbook: None,
            exams: Vec::new(),
            selected_exam: None,
            exam_editor: None,
            scope_editor: None,
            scope_search_text: String::new(),
            scope_search,
            scope_topic_name,
            scope_subscription: None,
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
                if matches!(self.route, Route::Exams | Route::ExamDetail) {
                    self.exams = self.service.exams(true).unwrap_or_default();
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
        if matches!(route, Route::Exams | Route::ExamDetail) {
            self.exams = self.service.exams(true).unwrap_or_default();
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
                self.modal = Some(Modal::ImportJson {
                    workbook,
                    path: path.clone(),
                    source: path.display().to_string(),
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
