use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use thiserror::Error;
use workbook_domain::{
    Answer, AssetContent, AssetRef, AssistState, CardState, Content, Exam, ExamCatalogQuestion,
    ExamDraft, ExamScope, Goal, GoalDraft, GoalSource, GradeResult, GradeStatus, MAX_JSON_BYTES,
    MAX_TOTAL_ASSET_BYTES, MockOptions, Mode, NearMissReason, NearMissRecord, Phase,
    PracticeOptions, PracticeStrategy, Question, QuestionChange, QuestionType, Rating,
    RatingSource, SessionItemStatus, SessionStatus, Settings, StudyCalendar, ThemeSetting,
    TopicCatalogEntry, Workbook, answer_is_blank, automatic_rating, classify_question_change,
    encode_base64, expand_topic_ids, grade_answer_detailed, inspect_image, parse_workbook,
    resolve_scope, resolved_revision,
};
use workbook_import::{LegacyPlan, LegacyPreview};
use workbook_scheduler::{
    BudgetCategory, BudgetQuestion, Clock, ExamPlan, PlanCard, PlanInput, PlanQuestion,
    PlanWarning, QueueChoice, QueueInput, QueueKind, Scheduler, SchedulerSettings,
    estimated_seconds, exam_deadline, plan_exam, select_next, select_within_budget,
};
use workbook_storage::{
    AttemptRow, BudgetPool, CommitWrite, CorrectionWrite, ExamPracticeStats, MockAnswerRow,
    MockResultRow, SessionBundle, SessionCandidate, SessionCounts, SessionItemRow, SessionRow,
    SessionWorkbookRow, Storage, StorageError, counts_for_workbook,
};

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("{0}")]
    Storage(#[from] StorageError),
    #[error("{0}")]
    Validation(#[from] workbook_domain::ValidationError),
    #[error("{0}")]
    Import(#[from] workbook_import::ImportError),
    #[error("{0}")]
    Scheduler(#[from] workbook_scheduler::SchedulerError),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("操作できません: {0}")]
    Invalid(String),
    #[error("競合しました: {0}")]
    Conflict(String),
    #[error("見つかりません: {0}")]
    NotFound(String),
    #[error("学習内容が変わる問題が{}件あります。保存内容を確認してください", .0.len())]
    ResetRequired(Vec<QuestionRevisionPlan>),
}

pub type Result<T> = std::result::Result<T, ServiceError>;

pub const SAMPLE_WORKBOOK_JSON: &str = include_str!("../../../assets/sample.workbook.json");
pub const SAMPLE_WORKBOOK_V2_JSON: &str = include_str!("../../../assets/sample-v2.workbook.json");

#[derive(Clone, Debug)]
pub struct BookSummary {
    pub local_id: String,
    pub source_id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub total: usize,
    pub learned: usize,
    pub due: usize,
    pub new_available: usize,
    /// Learning/Relearning cards due within the Learn Ahead window. They are
    /// actionable when there are no normal due candidates.
    pub short_term_pending: usize,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct BookToday {
    pub local_id: String,
    pub title: String,
    pub due: usize,
    pub new_available: usize,
    pub short_term_pending: usize,
}

#[derive(Clone, Debug)]
pub struct TodaySummary {
    pub due: usize,
    pub new_available: usize,
    pub new_limit: u32,
    pub new_remaining_today: usize,
    pub short_term_pending: usize,
    pub next_due_at: Option<DateTime<Utc>>,
    pub answers: usize,
    pub correct: usize,
    pub graded: usize,
    pub elapsed_ms: i64,
    pub books: Vec<BookToday>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub books: Vec<BookSummary>,
    pub settings: Settings,
    pub active: Option<StudyView>,
    pub today: TodaySummary,
    /// Lightweight progress for active goals and exams, used by Today.
    pub goals: Vec<GoalBrief>,
    pub data_path: String,
    pub timezone: String,
}

#[derive(Clone, Debug, Default)]
pub struct StudyProgress {
    pub position: usize,
    pub total: usize,
    pub answered: usize,
    pub skipped: usize,
}

/// A time budget is a target, not a countdown. `reached` only tells the UI
/// that the selected amount of study is done; the current question is never
/// discarded and nothing is force-submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionBudgetView {
    pub target_seconds: u32,
    pub used_ms: i64,
    pub remaining_ms: i64,
    pub reached: bool,
}

#[derive(Clone, Debug)]
pub struct KnowledgeIntro {
    pub note_title: String,
    pub content: Content,
}

#[derive(Clone, Debug)]
pub struct FeedbackView {
    pub attempt_id: Option<String>,
    pub committed: bool,
    pub status: GradeStatus,
    pub correct: Option<bool>,
    pub unknown: bool,
    pub hint: bool,
    pub assisted: bool,
    pub retry_count: usize,
    pub near_miss_reasons: Vec<NearMissReason>,
    pub near_miss_messages: Vec<&'static str>,
    pub diagnostic: Option<workbook_domain::GradeDiagnostic>,
    pub rating: Option<Rating>,
    pub rating_source: Option<RatingSource>,
    /// The rating the automatic policy would choose. `None` means the answer
    /// cannot be scored automatically and the learner must choose a rating.
    pub suggested_rating: Option<Rating>,
    pub explanation: Content,
    pub answer_summary: String,
    pub given_answer: String,
    pub elapsed_ms: i64,
}

#[derive(Clone, Debug)]
pub struct AssetView {
    pub id: String,
    pub media_type: String,
    pub alt: String,
    pub width: u32,
    pub height: u32,
    /// Content fingerprint (bytes plus dimensions and media type). The UI
    /// image cache keys on it so re-importing an asset with the same ID but
    /// new content cannot reuse the old decoded texture.
    pub fingerprint: u64,
    pub bytes: Arc<Vec<u8>>,
}

/// Stable, allocation-free fingerprint of an embedded asset. FNV-1a is enough
/// for cache identity: a collision would only mean reusing an image whose
/// bytes and metadata are already effectively identical.
pub fn asset_fingerprint(media_type: &str, width: u32, height: u32, bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in media_type
        .bytes()
        .chain(width.to_le_bytes())
        .chain(height.to_le_bytes())
        .chain(bytes.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResultStats {
    pub questions: usize,
    pub answered: usize,
    pub attempts: usize,
    pub short_term: usize,
    pub skipped: usize,
    pub first_pass_graded: usize,
    pub first_pass_correct: usize,
    pub elapsed_ms: i64,
}

#[derive(Clone, Debug)]
pub struct WorkbookResultView {
    pub workbook_id: String,
    pub title: String,
    pub stats: ResultStats,
    pub next_workbook_title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OverallResultView {
    pub stats: ResultStats,
    pub workbooks: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct StudyView {
    pub session_id: String,
    pub mode: Mode,
    pub phase: Phase,
    pub revision: u64,
    pub workbook_id: Option<String>,
    pub workbook_title: Option<String>,
    pub note_id: Option<String>,
    pub note_title: Option<String>,
    pub item_kind: String,
    pub question: Option<Question>,
    pub assets: Vec<AssetView>,
    pub intro: Option<KnowledgeIntro>,
    pub draft: Option<Answer>,
    pub hint: Option<Content>,
    pub hint_visible: bool,
    pub revealed: bool,
    pub feedback: Option<FeedbackView>,
    pub progress: StudyProgress,
    pub workbook_result: Option<WorkbookResultView>,
    pub overall_result: Option<OverallResultView>,
    pub exam_id: Option<String>,
    pub exam_title: Option<String>,
    pub goal_id: Option<String>,
    pub goal_title: Option<String>,
    pub budget: Option<SessionBudgetView>,
    pub mock: Option<MockSessionView>,
    pub mock_result: Option<MockResultView>,
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub session_id: String,
    pub mode: Mode,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub workbook_titles: Vec<String>,
    pub stats: ResultStats,
}

#[derive(Clone, Debug)]
pub struct MockHistoryEntry {
    pub session_id: String,
    pub submitted_at: DateTime<Utc>,
    pub total: u32,
    pub correct: u32,
    pub incorrect: u32,
    pub unanswered: u32,
    pub ungraded: u32,
    pub duration_ms: i64,
    /// `correct / (correct + incorrect)`, or `None` when nothing could be
    /// graded automatically. Unanswered and ungraded items are never treated
    /// as wrong for this denominator.
    pub score_percent: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct ExamSummary {
    pub exam: Exam,
    pub total: usize,
    pub studied: usize,
    pub unstudied: usize,
    pub due: usize,
    /// Questions with a lapse, difficulty >= 7, or at least one incorrect
    /// answer. A transparent heuristic shared with Practice.
    pub weak: usize,
    /// Calendar days until the target date. Negative values are clamped to 0
    /// in `plan`; `expired` carries the actual state.
    pub days_remaining: Option<i64>,
    pub expired: bool,
    /// `studied / total`, or `None` when the scope is empty.
    pub coverage: Option<f64>,
    /// Today's deadline-aware selection. Recomputed on every read; never cached.
    pub plan: ExamPlan,
    pub practice: ExamPracticeStats,
    pub mock_history: Vec<MockHistoryEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockItemStatus {
    Correct,
    Incorrect,
    Unanswered,
    Ungraded,
}

impl MockItemStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Correct => "正解",
            Self::Incorrect => "不正解",
            Self::Unanswered => "未回答",
            Self::Ungraded => "採点なし",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MockItemResult {
    pub ordinal: u32,
    pub workbook_title: String,
    pub note_title: String,
    pub prompt: String,
    pub status: MockItemStatus,
    pub given_answer: String,
    pub answer_summary: String,
    pub explanation: Content,
}

#[derive(Clone, Debug)]
pub struct MockResultView {
    pub session_id: String,
    pub exam_id: String,
    pub exam_title: String,
    pub submitted_at: DateTime<Utc>,
    pub total: u32,
    pub correct: u32,
    pub incorrect: u32,
    pub unanswered: u32,
    pub ungraded: u32,
    pub duration_ms: i64,
    pub score_percent: Option<f64>,
    pub items: Vec<MockItemResult>,
}

#[derive(Clone, Debug)]
pub struct MockSessionView {
    pub total: usize,
    pub answered: usize,
    pub remaining_ms: Option<i64>,
    pub submitted: bool,
}

#[derive(Clone, Debug)]
pub struct TopicSummary {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    /// Human-readable path such as `数学 / 一次関数`.
    pub path: String,
    pub description: String,
    pub question_count: usize,
    pub studied: usize,
    pub due: usize,
    pub weak: usize,
}

#[derive(Clone, Debug)]
pub struct ScopeCount {
    pub id: String,
    pub title: String,
    pub total: usize,
    pub studied: usize,
    pub due: usize,
    pub weak: usize,
}

/// Measured counts for the currently selected scope. These are observations
/// from stored questions, FSRS states, and attempts; no mastery estimate is
/// invented here.
#[derive(Clone, Debug, Default)]
pub struct ScopePreview {
    pub total: usize,
    pub studied: usize,
    pub unstudied: usize,
    pub due: usize,
    pub weak: usize,
    pub workbooks: Vec<ScopeCount>,
    pub topics: Vec<ScopeCount>,
}

impl ScopePreview {
    pub fn coverage(&self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some(self.studied as f64 / self.total as f64)
        }
    }
}

#[derive(Clone, Debug)]
pub struct GoalBrief {
    pub id: String,
    pub title: String,
    pub source: GoalSource,
    pub target_date: Option<chrono::NaiveDate>,
    pub days_remaining: Option<i64>,
    pub expired: bool,
    pub total: usize,
    pub studied: usize,
    pub due: usize,
    pub coverage: Option<f64>,
}

/// Topic-level measured statistics for a goal. These are counts and observed
/// answer outcomes, not a knowledge-tracing or mastery score.
#[derive(Clone, Debug)]
pub struct GoalTopicStat {
    pub topic_id: String,
    pub title: String,
    pub total: usize,
    pub studied: usize,
    pub unstudied: usize,
    pub due: usize,
    pub weak: usize,
    pub attempts: usize,
    pub first_pass_graded: usize,
    pub first_pass_correct: usize,
    pub first_pass_accuracy: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct SavedWorkbook {
    pub local_id: String,
    pub workbook: Workbook,
    /// Questions whose study content changed during this save. Their previous
    /// FSRS state was reset; attempts are preserved.
    pub reset_questions: Vec<QuestionRevisionPlan>,
}

/// A question edit that changes what the learner is asked to remember.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRevisionPlan {
    pub note_id: String,
    pub question_id: String,
    pub prompt: String,
    pub previous_revision: i64,
    pub next_revision: i64,
}

#[derive(Clone, Debug)]
pub struct WorkbookIssue {
    pub code: String,
    pub severity: String,
    pub path: String,
    pub message: String,
    pub suggestion: Option<String>,
}

impl WorkbookIssue {
    pub fn is_error(&self) -> bool {
        self.severity == "error"
    }
}

#[derive(Clone, Debug)]
pub struct GoalSummary {
    pub goal: Goal,
    pub source: GoalSource,
    pub total: usize,
    pub studied: usize,
    pub unstudied: usize,
    pub due: usize,
    pub weak: usize,
    pub days_remaining: Option<i64>,
    pub expired: bool,
    pub coverage: Option<f64>,
    pub plan: ExamPlan,
    pub topics: Vec<GoalTopicStat>,
    pub first_pass_graded: usize,
    pub first_pass_correct: usize,
    pub first_pass_accuracy: Option<f64>,
}

pub fn mock_score_percent(correct: u32, incorrect: u32) -> Option<f64> {
    let graded = correct + incorrect;
    if graded == 0 {
        None
    } else {
        Some(correct as f64 / graded as f64 * 100.0)
    }
}

pub struct WorkbookService {
    storage: Mutex<Storage>,
    clock: Arc<dyn Clock>,
}

impl WorkbookService {
    pub fn open(data_dir: &std::path::Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let storage = Storage::open(data_dir)?;
        let service = Self {
            storage: Mutex::new(storage),
            clock,
        };
        service.recover_active()?;
        Ok(service)
    }

    #[cfg(test)]
    pub fn in_memory(clock: Arc<dyn Clock>) -> Result<Self> {
        Ok(Self {
            storage: Mutex::new(Storage::open_in_memory()?),
            clock,
        })
    }

    fn storage(&self) -> Result<MutexGuard<'_, Storage>> {
        self.storage
            .lock()
            .map_err(|_| ServiceError::Conflict("データベースのロックに失敗しました".to_string()))
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn data_path(&self) -> String {
        match self
            .storage()
            .ok()
            .and_then(|storage| storage.path().map(ToOwned::to_owned))
        {
            Some(path) => path.display().to_string(),
            None => "(memory)".to_string(),
        }
    }

    pub fn integrity(&self) -> Result<String> {
        Ok(self.storage()?.integrity()?)
    }

    pub fn settings(&self) -> Result<Settings> {
        Ok(self.storage()?.settings()?)
    }

    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        settings
            .validate()
            .map_err(|error| ServiceError::Invalid(error.0))?;
        self.storage()?.save_settings(&settings)?;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let storage = self.storage()?;
        self.snapshot_inner(&storage)
    }

    fn snapshot_inner(&self, storage: &Storage) -> Result<Snapshot> {
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let books = self.book_summaries(storage, &calendar, now)?;
        let active = match storage.active_session()? {
            Some(bundle) => Some(self.view_for(storage, &bundle, &settings, &calendar, now)?),
            None => None,
        };
        let due = storage.review_candidates(None, now)?.len();
        let new_available = books.iter().map(|book| book.new_available).sum();
        let short_term_pending = storage
            .upcoming_short_term(None, now, now + Duration::minutes(20))?
            .len();
        let introduced_today = storage.new_questions_introduced_today(&calendar, now)?;
        let new_remaining_today = (settings.new_limit as usize).saturating_sub(introduced_today);
        let next_due_at = storage.next_due_at(now)?;
        let today_day = calendar.learning_day(now);
        let goals = goal_briefs(storage, &calendar, now)?;
        let recent = storage.attempts_since(now - Duration::hours(48))?;
        let mut answers = 0;
        let mut correct = 0;
        let mut graded = 0;
        let mut elapsed_ms = 0;
        for attempt in recent {
            if calendar.learning_day(attempt.answered_at) == today_day {
                answers += 1;
                elapsed_ms += attempt.elapsed_ms;
                if let Some(is_correct) = attempt.correct {
                    graded += 1;
                    if is_correct {
                        correct += 1;
                    }
                }
            }
        }
        let today = TodaySummary {
            due,
            new_available,
            new_limit: settings.new_limit,
            new_remaining_today,
            short_term_pending,
            next_due_at,
            answers,
            correct,
            graded,
            elapsed_ms,
            books: books
                .iter()
                .map(|book| BookToday {
                    local_id: book.local_id.clone(),
                    title: book.title.clone(),
                    due: book.due,
                    new_available: book.new_available,
                    short_term_pending: book.short_term_pending,
                })
                .collect(),
        };
        Ok(Snapshot {
            books,
            settings,
            active,
            today,
            goals,
            data_path: storage
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "(memory)".to_string()),
            timezone: calendar.timezone_name().to_string(),
        })
    }

    fn book_summaries(
        &self,
        storage: &Storage,
        calendar: &StudyCalendar,
        now: DateTime<Utc>,
    ) -> Result<Vec<BookSummary>> {
        let _ = calendar;
        let mut output = Vec::new();
        for record in storage.workbooks()? {
            let workbook = parse_workbook(&record.content_json)?;
            let due = storage
                .review_candidates(Some(&record.local_id), now)?
                .len();
            let short_term_pending = storage
                .upcoming_short_term(Some(&record.local_id), now, now + Duration::minutes(20))?
                .len();
            output.push(BookSummary {
                local_id: record.local_id.clone(),
                source_id: record.source_id,
                title: workbook.title.clone(),
                description: workbook.description.clone(),
                tags: workbook.tags.clone(),
                total: workbook.question_count(),
                learned: storage.learned_count(&record.local_id)?,
                due,
                new_available: storage.new_question_count(&record.local_id)?,
                short_term_pending,
                last_used_at: record.last_used_at,
            });
        }
        output.sort_by(|a, b| {
            b.last_used_at
                .cmp(&a.last_used_at)
                .then(a.title.cmp(&b.title))
        });
        Ok(output)
    }

    pub fn workbook(&self, local_id: &str) -> Result<Workbook> {
        let storage = self.storage()?;
        let mut workbook = storage.workbook(local_id)?;
        hydrate_workbook_topics(&storage, local_id, &mut workbook)?;
        Ok(workbook)
    }

    pub fn preview_workbook(&self, raw: &str) -> Result<Workbook> {
        Ok(parse_workbook(raw)?)
    }

    pub fn preview_workbook_file(&self, path: &std::path::Path) -> Result<Workbook> {
        let (workbook, _assets) = workbook_import::load_workbook_file(path)?;
        Ok(workbook)
    }

    /// Imports a validated workbook. Validation (including image decoding) runs
    /// before the safety backup, so a broken workbook never causes a useless
    /// backup or a partially changed database. The backup itself is produced
    /// with the SQLite backup API and must succeed before the import starts.
    pub fn import_workbook_json(&self, raw: &str) -> Result<String> {
        let workbook = parse_workbook(raw)?;
        let assets = workbook.embedded_assets()?;
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-import")?;
        let now = self.clock.now();
        Ok(storage.import_workbook(&workbook, &assets, now)?)
    }

    pub fn import_workbook_file(&self, path: &std::path::Path) -> Result<String> {
        let (workbook, assets) = workbook_import::load_workbook_file(path)?;
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-import")?;
        let now = self.clock.now();
        Ok(storage.import_workbook(&workbook, &assets, now)?)
    }

    fn write_action_backup(&self, storage: &Storage, prefix: &str) -> Result<()> {
        let directory = storage.data_dir().join("backups");
        storage.backup_to_unique(&directory, prefix)?;
        Ok(())
    }

    pub fn delete_workbook(&self, local_id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-delete")?;
        storage.delete_workbook(local_id)?;
        Ok(())
    }

    // ---- authoring ------------------------------------------------------

    /// Reports question edits that would reset FSRS state for an existing
    /// workbook. The UI shows this before saving. A new question, a note move,
    /// an explanation edit, or a topic change produces no reset.
    pub fn preview_workbook_save(
        &self,
        local_id: &str,
        workbook: &Workbook,
    ) -> Result<Vec<QuestionRevisionPlan>> {
        workbook.validate()?;
        let storage = self.storage()?;
        let stored = storage.workbook(local_id)?;
        Ok(plan_question_revisions(&stored, workbook))
    }

    /// Saves a manually authored workbook. Existing workbooks keep their
    /// `local_id` and every unchanged Note/Question ID, so FSRS states and
    /// attempts survive note moves, explanation, hint, asset, and topic edits.
    /// A safety backup is made before replacing an existing workbook.
    ///
    /// A change to the study content of a question starts a new revision and
    /// resets that question's FSRS state. Callers must pass `confirm_reset`
    /// only after showing `preview_workbook_save` to the user.
    pub fn save_workbook(
        &self,
        local_id: Option<&str>,
        workbook: &Workbook,
        confirm_reset: bool,
    ) -> Result<SavedWorkbook> {
        workbook.validate()?;
        let mut storage = self.storage()?;
        let mut workbook = workbook.clone();
        let assets = resolve_assets_for_save(&storage, local_id, &workbook)?;
        let now = self.clock.now();
        let Some(local_id) = local_id else {
            let new_id = storage.import_workbook(&workbook, &assets, now)?;
            return Ok(SavedWorkbook {
                local_id: new_id,
                workbook,
                reset_questions: Vec::new(),
            });
        };
        if storage.has_active_session_for_workbook(local_id)? {
            return Err(ServiceError::Conflict(
                "この教材の学習中は編集できません。先にSessionを終了してください".to_string(),
            ));
        }
        let stored = storage.workbook(local_id)?;
        let resets = plan_question_revisions(&stored, &workbook);
        if !resets.is_empty() && !confirm_reset {
            return Err(ServiceError::ResetRequired(resets));
        }
        apply_question_revisions(&stored, &mut workbook, &resets);
        workbook.revision = stored.revision.saturating_add(1).max(workbook.revision);
        let reset_ids: BTreeSet<String> =
            resets.iter().map(|plan| plan.question_id.clone()).collect();
        self.write_action_backup(&storage, "pre-edit")?;
        storage.update_workbook_content(local_id, &workbook, &assets, &reset_ids, now)?;
        Ok(SavedWorkbook {
            local_id: local_id.to_string(),
            workbook,
            reset_questions: resets,
        })
    }

    /// Creates an independent copy. Question IDs inside the copy are unchanged,
    /// but the new local workbook ID means FSRS state and attempts are separate.
    pub fn copy_workbook(&self, local_id: &str) -> Result<String> {
        let mut storage = self.storage()?;
        let mut workbook = storage.workbook(local_id)?;
        hydrate_workbook_topics(&storage, local_id, &mut workbook)?;
        let assets = storage.assets_for_workbook(local_id)?;
        workbook.title = format!("{}（コピー）", workbook.title);
        workbook.revision = 1;
        let now = self.clock.now();
        Ok(storage.import_workbook(&workbook, &assets, now)?)
    }

    /// Exports a portable workbook JSON: topic paths are restored and every
    /// image asset is embedded as base64, so the file does not depend on this
    /// machine's asset paths.
    pub fn export_workbook_json(&self, local_id: &str) -> Result<String> {
        let storage = self.storage()?;
        let mut workbook = storage.workbook(local_id)?;
        hydrate_workbook_topics(&storage, local_id, &mut workbook)?;
        let stored_assets = storage.assets_for_workbook(local_id)?;
        workbook.assets = stored_assets
            .into_iter()
            .map(|asset| AssetRef {
                id: asset.id,
                alt: asset.alt,
                media_type: asset.media_type,
                file: None,
                data: Some(encode_base64(&asset.bytes)),
            })
            .collect();
        let raw = serde_json::to_string_pretty(&workbook)?;
        if raw.len() > MAX_JSON_BYTES {
            return Err(ServiceError::Invalid(format!(
                "書き出すJSONが{}MBを超えます。画像アセットを減らすか、小さい画像にしてください",
                MAX_JSON_BYTES / (1024 * 1024)
            )));
        }
        Ok(raw)
    }

    pub fn workbook_assets(&self, local_id: &str) -> Result<Vec<AssetView>> {
        let storage = self.storage()?;
        Ok(storage
            .assets_for_workbook(local_id)?
            .into_iter()
            .map(|asset| {
                let fingerprint =
                    asset_fingerprint(&asset.media_type, asset.width, asset.height, &asset.bytes);
                AssetView {
                    id: asset.id,
                    media_type: asset.media_type,
                    alt: asset.alt,
                    width: asset.width,
                    height: asset.height,
                    fingerprint,
                    bytes: Arc::new(asset.bytes),
                }
            })
            .collect())
    }

    pub fn validate_workbook_model(&self, workbook: &Workbook) -> Result<()> {
        workbook.validate()?;
        Ok(())
    }

    pub fn lint_workbook_json(&self, raw: &str) -> Vec<WorkbookIssue> {
        let report = workbook_lint::lint_raw(raw, "workbook.json", None);
        report
            .diagnostics
            .into_iter()
            .map(|diagnostic| WorkbookIssue {
                code: diagnostic.code.to_string(),
                severity: match diagnostic.severity {
                    workbook_lint::Severity::Error => "error",
                    workbook_lint::Severity::Warning => "warning",
                    workbook_lint::Severity::Info => "info",
                }
                .to_string(),
                path: diagnostic.path,
                message: diagnostic.message,
                suggestion: diagnostic.suggestion,
            })
            .collect()
    }

    // ---- topics and scope -----------------------------------------------

    pub fn topics(&self) -> Result<Vec<TopicSummary>> {
        let storage = self.storage()?;
        let now = self.clock.now();
        topic_summaries(&storage, now)
    }

    pub fn create_topic(
        &self,
        name: &str,
        parent_id: Option<&str>,
        description: &str,
    ) -> Result<String> {
        let topic = workbook_domain::Topic {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.trim().to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            description: description.trim().to_string(),
        };
        topic.validate()?;
        let mut storage = self.storage()?;
        storage.insert_topic(&topic, self.clock.now())?;
        Ok(topic.id)
    }

    pub fn update_topic(
        &self,
        id: &str,
        name: &str,
        parent_id: Option<&str>,
        description: &str,
    ) -> Result<()> {
        let topic = workbook_domain::Topic {
            id: id.to_string(),
            name: name.trim().to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            description: description.trim().to_string(),
        };
        topic.validate()?;
        let mut storage = self.storage()?;
        storage.update_topic(&topic, self.clock.now())?;
        Ok(())
    }

    pub fn delete_topic(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-delete-topic")?;
        storage.delete_topic(id)?;
        Ok(())
    }

    pub fn set_question_topics(
        &self,
        workbook_id: &str,
        question_id: &str,
        topic_ids: &[String],
    ) -> Result<()> {
        let mut storage = self.storage()?;
        storage.set_question_topics(workbook_id, question_id, topic_ids)?;
        Ok(())
    }

    // ---- goals ----------------------------------------------------------

    pub fn goals(&self, include_archived: bool) -> Result<Vec<GoalSummary>> {
        let storage = self.storage()?;
        let catalog = exam_catalog(&storage)?;
        let now = self.clock.now();
        let context = exam_plan_context(&storage, now)?;
        let first: FirstAttemptMap = storage
            .first_attempt_results()?
            .into_iter()
            .map(|(workbook_id, question_id, correct)| ((workbook_id, question_id), correct))
            .collect();
        let mut output = Vec::new();
        for goal in storage.goals(include_archived)? {
            output.push(goal_summary_from(
                &storage, goal, &catalog, &context, &first, now,
            )?);
        }
        for exam in storage.exams(include_archived)? {
            output.push(goal_summary_from(
                &storage,
                Goal::from_exam(&exam),
                &catalog,
                &context,
                &first,
                now,
            )?);
        }
        output.sort_by(|a, b| {
            a.goal
                .is_archived()
                .cmp(&b.goal.is_archived())
                .then(
                    a.goal
                        .target_date
                        .cmp(&b.goal.target_date)
                        .then(a.goal.title.cmp(&b.goal.title)),
                )
                .then(a.goal.id.cmp(&b.goal.id))
        });
        Ok(output)
    }

    pub fn goal(&self, id: &str) -> Result<Goal> {
        let storage = self.storage()?;
        goal_from(&storage, id)
    }

    pub fn goal_summary(&self, id: &str) -> Result<GoalSummary> {
        let storage = self.storage()?;
        let catalog = exam_catalog(&storage)?;
        let now = self.clock.now();
        let context = exam_plan_context(&storage, now)?;
        let first: FirstAttemptMap = storage
            .first_attempt_results()?
            .into_iter()
            .map(|(workbook_id, question_id, correct)| ((workbook_id, question_id), correct))
            .collect();
        let goal = goal_from(&storage, id)?;
        goal_summary_from(&storage, goal, &catalog, &context, &first, now)
    }

    pub fn create_goal(&self, draft: GoalDraft) -> Result<String> {
        draft.validate()?;
        let goal = draft.into_goal(self.clock.now());
        let mut storage = self.storage()?;
        storage.insert_goal(&goal)?;
        Ok(goal.id)
    }

    pub fn update_goal(&self, id: &str, draft: GoalDraft) -> Result<()> {
        draft.validate()?;
        let mut storage = self.storage()?;
        let mut goal = storage.goal(id)?;
        if goal.exam_id.is_some() {
            return Err(ServiceError::Invalid(
                "試験は試験画面から編集してください".to_string(),
            ));
        }
        draft.apply_to(&mut goal, self.clock.now());
        storage.update_goal(&goal)?;
        Ok(())
    }

    pub fn archive_goal(&self, id: &str, archived: bool) -> Result<()> {
        let mut storage = self.storage()?;
        match storage.goal(id) {
            Ok(_) => storage.archive_goal(id, archived, self.clock.now())?,
            Err(StorageError::NotFound(_)) => {
                storage.archive_exam(id, archived, self.clock.now())?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn delete_goal(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        match storage.goal(id) {
            Ok(_) => {
                self.write_action_backup(&storage, "pre-delete-goal")?;
                storage.delete_goal(id)?;
            }
            Err(StorageError::NotFound(_)) => {
                self.write_action_backup(&storage, "pre-delete-exam")?;
                storage.delete_exam(id)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn start_goal_plan(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        let goal = goal_from(&storage, id)?;
        if let Some(exam_id) = goal.exam_id.clone() {
            drop(storage);
            return self.start_exam_plan(&exam_id);
        }
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let context = exam_plan_context(&storage, now)?;
        let plan = build_goal_plan(&storage, &goal, &catalog, &context, now)?;
        let candidates: Vec<SessionCandidate> = plan
            .selected
            .iter()
            .map(|item| SessionCandidate {
                workbook_id: item.workbook_id.clone(),
                note_id: item.note_id.clone(),
                question_id: item.question_id.clone(),
                due_at: None,
            })
            .collect();
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                plan.warnings
                    .first()
                    .map(PlanWarning::message)
                    .unwrap_or_else(|| "この目標で今日の学習を作れませんでした".to_string()),
            ));
        }
        let (mut bundle, _) = build_session_bundle(Mode::Plan, &candidates, now, None, None, None)?;
        bundle.session.goal_id = Some(goal.id.clone());
        persist_started_bundle(&mut storage, bundle, now)
    }

    pub fn start_goal_review(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        let goal = goal_from(&storage, id)?;
        if let Some(exam_id) = goal.exam_id.clone() {
            drop(storage);
            return self.start_exam_review(&exam_id);
        }
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, &goal.scope, &catalog)?;
        let context = exam_plan_context(&storage, now)?;
        let mut candidates = scope_due_candidates(&resolved, &context, now)?;
        if candidates.is_empty() {
            candidates = scope_learn_ahead_candidates(&resolved, &context, now)?;
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "この目標に期限が来た復習はありません".to_string(),
            ));
        }
        let (mut bundle, _) =
            build_session_bundle(Mode::Review, &candidates, now, None, None, None)?;
        bundle.session.goal_id = Some(goal.id.clone());
        persist_started_bundle(&mut storage, bundle, now)
    }

    pub fn start_goal_practice(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        let goal = goal_from(&storage, id)?;
        if let Some(exam_id) = goal.exam_id.clone() {
            drop(storage);
            return self.start_exam_practice(
                &exam_id,
                &PracticeOptions {
                    strategy: PracticeStrategy::All,
                    question_limit: None,
                },
            );
        }
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, &goal.scope, &catalog)?;
        if resolved.is_empty() {
            return Err(ServiceError::Invalid(
                "この目標に対象の問題がありません".to_string(),
            ));
        }
        let candidates: Vec<SessionCandidate> = resolved
            .iter()
            .map(|question| SessionCandidate {
                workbook_id: question.workbook_id.clone(),
                note_id: question.note_id.clone(),
                question_id: question.question_id.clone(),
                due_at: None,
            })
            .collect();
        let (mut bundle, _) =
            build_session_bundle(Mode::Practice, &candidates, now, None, None, None)?;
        bundle.session.goal_id = Some(goal.id.clone());
        persist_started_bundle(&mut storage, bundle, now)
    }

    pub fn scope_preview(&self, scope: &ExamScope) -> Result<ScopePreview> {
        let storage = self.storage()?;
        scope_preview_inner(&storage, scope, self.clock.now())
    }

    /// Plans a cross-workbook scope with the existing deadline-aware planner.
    /// No target date means "today's useful set", not an exam deadline.
    pub fn start_scope_plan(&self, scope: &ExamScope, budget_seconds: Option<u32>) -> Result<()> {
        if budget_seconds == Some(0) {
            return Err(ServiceError::Invalid(
                "学習時間は1分以上を指定してください".to_string(),
            ));
        }
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, scope, &catalog)?;
        if resolved.is_empty() {
            return Err(ServiceError::Invalid(
                "この範囲に対象の問題がありません".to_string(),
            ));
        }
        let context = exam_plan_context(&storage, now)?;
        let questions = plan_questions_for(&resolved, &context, None, now);
        let minutes = budget_seconds
            .map(|seconds| seconds.div_ceil(60).max(1))
            .unwrap_or(30);
        let plan = plan_exam(PlanInput {
            now,
            calendar: &context.calendar,
            target_date: None,
            daily_minutes: minutes,
            new_limit_remaining: context.new_limit_remaining,
            studied_seconds_today: 0,
            questions: &questions,
        });
        let candidates: Vec<SessionCandidate> = plan
            .selected
            .iter()
            .map(|item| SessionCandidate {
                workbook_id: item.workbook_id.clone(),
                note_id: item.note_id.clone(),
                question_id: item.question_id.clone(),
                due_at: None,
            })
            .collect();
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                plan.warnings
                    .first()
                    .map(PlanWarning::message)
                    .unwrap_or_else(|| "この範囲で今日の学習を作れませんでした".to_string()),
            ));
        }
        let (bundle, _) =
            build_session_bundle(Mode::Plan, &candidates, now, None, None, budget_seconds)?;
        persist_started_bundle(&mut storage, bundle, now)
    }

    /// Starts a review limited to a scope. If nothing is due, Learn Ahead
    /// cards inside the scope may be used, preserving the 20-minute and 4 a.m.
    /// learning-day rules.
    pub fn start_scope_review(&self, scope: &ExamScope, budget_seconds: Option<u32>) -> Result<()> {
        if budget_seconds == Some(0) {
            return Err(ServiceError::Invalid(
                "学習時間は1分以上を指定してください".to_string(),
            ));
        }
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, scope, &catalog)?;
        if resolved.is_empty() {
            return Err(ServiceError::Invalid(
                "この範囲に対象の問題がありません".to_string(),
            ));
        }
        let context = exam_plan_context(&storage, now)?;
        let mut candidates = scope_due_candidates(&resolved, &context, now)?;
        if candidates.is_empty() {
            candidates = scope_learn_ahead_candidates(&resolved, &context, now)?;
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "この範囲に期限が来た復習はありません".to_string(),
            ));
        }
        if let Some(budget) = budget_seconds {
            let estimates = BudgetEstimates::load(&storage)?;
            let questions: Vec<BudgetQuestion> = candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    budget_question(
                        &estimates,
                        candidate.workbook_id.clone(),
                        candidate.note_id.clone(),
                        candidate.question_id.clone(),
                        BudgetPool::DueReview,
                        candidate.due_at,
                        index as u32,
                    )
                })
                .collect();
            candidates = select_within_budget(&questions, budget)
                .selected
                .into_iter()
                .map(session_candidate)
                .collect();
        }
        let (bundle, _) =
            build_session_bundle(Mode::Review, &candidates, now, None, None, budget_seconds)?;
        persist_started_bundle(&mut storage, bundle, now)
    }

    pub fn start_scope_practice(
        &self,
        scope: &ExamScope,
        budget_seconds: Option<u32>,
        limit: Option<u32>,
    ) -> Result<()> {
        if budget_seconds == Some(0) {
            return Err(ServiceError::Invalid(
                "学習時間は1分以上を指定してください".to_string(),
            ));
        }
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let now = self.clock.now();
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, scope, &catalog)?;
        if resolved.is_empty() {
            return Err(ServiceError::Invalid(
                "この範囲に対象の問題がありません".to_string(),
            ));
        }
        let mut candidates: Vec<SessionCandidate> = resolved
            .iter()
            .map(|question| SessionCandidate {
                workbook_id: question.workbook_id.clone(),
                note_id: question.note_id.clone(),
                question_id: question.question_id.clone(),
                due_at: None,
            })
            .collect();
        if let Some(limit) = limit {
            candidates.truncate(limit as usize);
        }
        if let Some(budget) = budget_seconds {
            let estimates = BudgetEstimates::load(&storage)?;
            let questions: Vec<BudgetQuestion> = candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    budget_question(
                        &estimates,
                        candidate.workbook_id.clone(),
                        candidate.note_id.clone(),
                        candidate.question_id.clone(),
                        BudgetPool::DueReview,
                        candidate.due_at,
                        index as u32,
                    )
                })
                .collect();
            candidates = select_within_budget(&questions, budget)
                .selected
                .into_iter()
                .map(session_candidate)
                .collect();
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "この範囲に対象の問題がありません".to_string(),
            ));
        }
        let (bundle, _) =
            build_session_bundle(Mode::Practice, &candidates, now, None, None, budget_seconds)?;
        persist_started_bundle(&mut storage, bundle, now)
    }

    // ---- exams ----------------------------------------------------------

    pub fn exams(&self, include_archived: bool) -> Result<Vec<ExamSummary>> {
        let storage = self.storage()?;
        let records = storage.exams(include_archived)?;
        let catalog = exam_catalog(&storage)?;
        let now = self.clock.now();
        let context = exam_plan_context(&storage, now)?;
        records
            .into_iter()
            .map(|exam| exam_summary_from(&storage, exam, &catalog, &context, now, 8))
            .collect()
    }

    pub fn exam(&self, id: &str) -> Result<Exam> {
        Ok(self.storage()?.exam(id)?)
    }

    pub fn exam_summary(&self, id: &str) -> Result<ExamSummary> {
        let storage = self.storage()?;
        let exam = storage.exam(id)?;
        let catalog = exam_catalog(&storage)?;
        let now = self.clock.now();
        let context = exam_plan_context(&storage, now)?;
        exam_summary_from(&storage, exam, &catalog, &context, now, 50)
    }

    pub fn exam_catalog(&self) -> Result<Vec<ExamCatalogQuestion>> {
        let storage = self.storage()?;
        exam_catalog(&storage)
    }

    pub fn preview_exam_scope(&self, scope: &ExamScope) -> Result<Vec<ExamCatalogQuestion>> {
        let storage = self.storage()?;
        let catalog = exam_catalog(&storage)?;
        resolve_scope_for(&storage, scope, &catalog)
    }

    pub fn create_exam(&self, draft: ExamDraft) -> Result<String> {
        draft.validate()?;
        let exam = draft.into_exam(self.clock.now());
        let mut storage = self.storage()?;
        storage.insert_exam(&exam)?;
        Ok(exam.id)
    }

    pub fn update_exam(&self, id: &str, draft: ExamDraft) -> Result<()> {
        draft.validate()?;
        let mut storage = self.storage()?;
        let mut exam = storage.exam(id)?;
        draft.apply_to(&mut exam, self.clock.now());
        storage.update_exam(&exam)?;
        Ok(())
    }

    pub fn archive_exam(&self, id: &str, archived: bool) -> Result<()> {
        let mut storage = self.storage()?;
        storage.archive_exam(id, archived, self.clock.now())?;
        Ok(())
    }

    pub fn delete_exam(&self, id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-delete-exam")?;
        storage.delete_exam(id)?;
        Ok(())
    }

    pub fn start_exam_review(&self, exam_id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let exam = storage.exam(exam_id)?;
        let catalog = exam_catalog(&storage)?;
        let scope: BTreeSet<(String, String)> = resolve_scope_for(&storage, &exam.scope, &catalog)?
            .into_iter()
            .map(|question| (question.workbook_id, question.question_id))
            .collect();
        let now = self.clock.now();
        let mut candidates: Vec<SessionCandidate> = storage
            .review_candidates(None, now)?
            .into_iter()
            .filter(|candidate| {
                scope.contains(&(candidate.workbook_id.clone(), candidate.question_id.clone()))
            })
            .collect();
        if candidates.is_empty() {
            candidates = storage
                .upcoming_short_term(None, now, now + Duration::minutes(20))?
                .into_iter()
                .filter(|candidate| {
                    scope.contains(&(candidate.workbook_id.clone(), candidate.question_id.clone()))
                })
                .collect();
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "この試験の範囲に期限が来た問題はありません".to_string(),
            ));
        }
        self.create_exam_bundle(&mut storage, Mode::Review, exam_id, &candidates, None, now)
    }

    pub fn start_exam_plan(&self, exam_id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let exam = storage.exam(exam_id)?;
        let catalog = exam_catalog(&storage)?;
        let now = self.clock.now();
        let context = exam_plan_context(&storage, now)?;
        let plan = build_exam_plan(&storage, &exam, &catalog, &context, now)?;
        if plan.selected.is_empty() {
            let message = plan
                .warnings
                .first()
                .map(|warning| warning.message())
                .unwrap_or_else(|| "今日の学習計画が空です".to_string());
            return Err(ServiceError::Invalid(message));
        }
        let candidates: Vec<SessionCandidate> = plan
            .selected
            .iter()
            .map(|item| SessionCandidate {
                workbook_id: item.workbook_id.clone(),
                note_id: item.note_id.clone(),
                question_id: item.question_id.clone(),
                due_at: None,
            })
            .collect();
        self.create_exam_bundle(&mut storage, Mode::Plan, exam_id, &candidates, None, now)
    }

    pub fn start_exam_practice(&self, exam_id: &str, options: &PracticeOptions) -> Result<()> {
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let exam = storage.exam(exam_id)?;
        let catalog = exam_catalog(&storage)?;
        let resolved = resolve_scope_for(&storage, &exam.scope, &catalog)?;
        if resolved.is_empty() {
            return Err(ServiceError::Invalid(
                "この試験の範囲に問題がありません".to_string(),
            ));
        }
        let now = self.clock.now();
        let states: BTreeMap<(String, String), CardState> = storage
            .review_states()?
            .into_iter()
            .map(|(workbook_id, question_id, card)| ((workbook_id, question_id), card.state))
            .collect();
        let attempt_stats: BTreeMap<(String, String), (usize, usize)> = storage
            .question_attempt_stats()?
            .into_iter()
            .map(|(workbook_id, question_id, stat)| {
                ((workbook_id, question_id), (stat.attempts, stat.incorrect))
            })
            .collect();
        let mut candidates: Vec<SessionCandidate> = resolved
            .into_iter()
            .filter(|question| {
                let key = (question.workbook_id.clone(), question.question_id.clone());
                match options.strategy {
                    PracticeStrategy::All | PracticeStrategy::Random => true,
                    PracticeStrategy::Unstudied => !states.contains_key(&key),
                    PracticeStrategy::PreviouslyIncorrect => attempt_stats
                        .get(&key)
                        .is_some_and(|(_, incorrect)| *incorrect > 0),
                    PracticeStrategy::Weak => {
                        attempt_stats
                            .get(&key)
                            .is_some_and(|(_, incorrect)| *incorrect > 0)
                            || storage
                                .review_state(&question.workbook_id, &question.question_id)
                                .ok()
                                .flatten()
                                .is_some_and(|card| card.lapses > 0 || card.difficulty >= 7.0)
                    }
                }
            })
            .map(|question| SessionCandidate {
                workbook_id: question.workbook_id,
                note_id: question.note_id,
                question_id: question.question_id,
                due_at: None,
            })
            .collect();
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "条件に合う練習問題がありません".to_string(),
            ));
        }
        if options.strategy == PracticeStrategy::Random {
            shuffle_candidates(&mut candidates, seed_from(now, exam_id, "practice"));
        }
        if let Some(limit) = options.question_limit {
            candidates.truncate(limit as usize);
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "条件に合う練習問題がありません".to_string(),
            ));
        }
        self.create_exam_bundle(
            &mut storage,
            Mode::Practice,
            exam_id,
            &candidates,
            None,
            now,
        )
    }

    pub fn start_exam_mock(&self, exam_id: &str, options: &MockOptions) -> Result<()> {
        let mut storage = self.storage()?;
        ensure_no_active_session(&storage)?;
        let exam = storage.exam(exam_id)?;
        let catalog = exam_catalog(&storage)?;
        let mut candidates: Vec<SessionCandidate> =
            resolve_scope_for(&storage, &exam.scope, &catalog)?
                .into_iter()
                .map(|question| SessionCandidate {
                    workbook_id: question.workbook_id,
                    note_id: question.note_id,
                    question_id: question.question_id,
                    due_at: None,
                })
                .collect();
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "この試験の範囲に問題がありません".to_string(),
            ));
        }
        let now = self.clock.now();
        if options.order == workbook_domain::MockOrder::Random {
            shuffle_candidates(&mut candidates, seed_from(now, exam_id, "mock"));
        }
        if let Some(limit) = options.question_limit {
            candidates.truncate(limit as usize);
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "模試の問題を選択できませんでした".to_string(),
            ));
        }
        self.create_exam_bundle(
            &mut storage,
            Mode::Mock,
            exam_id,
            &candidates,
            options.time_limit_seconds,
            now,
        )
    }

    fn create_exam_bundle(
        &self,
        storage: &mut Storage,
        mode: Mode,
        exam_id: &str,
        candidates: &[SessionCandidate],
        time_limit_seconds: Option<u32>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let (mut bundle, _first_note) = build_session_bundle(
            mode,
            candidates,
            now,
            Some(exam_id.to_string()),
            time_limit_seconds,
            None,
        )?;
        let first_item = bundle
            .items
            .first()
            .ok_or_else(|| ServiceError::Invalid("問題を選択できませんでした".to_string()))?
            .clone();
        bundle.session.current_workbook_id = Some(first_item.workbook_id.clone());
        bundle.session.current_item_id = Some(first_item.id.clone());
        bundle.session.cursor_kind = "initial".to_string();
        // Exam sessions normally skip the knowledge-card intro: they are an
        // assessment or review context. A Plan session is also allowed to
        // introduce unstudied notes, so it shows the intro once per note.
        let intro = mode == Mode::Plan
            && !storage.note_introduced(&first_item.workbook_id, &first_item.note_id)?;
        bundle.session.phase = if intro { Phase::Intro } else { Phase::Question };
        bundle.session.question_started_at = Some(now);
        bundle.session.revision = 0;
        storage.create_session(&bundle)?;
        let mut workbooks = BTreeSet::new();
        for item in &bundle.items {
            workbooks.insert(item.workbook_id.clone());
        }
        for workbook in workbooks {
            storage.touch_workbook(&workbook, now)?;
        }
        Ok(())
    }

    /// Grades one mock answer, stores it separately from FSRS, then advances.
    /// Unsupported question types are stored as ungraded (`correct = NULL`).
    pub fn mock_answer(
        &self,
        session_id: &str,
        answer: Option<&Answer>,
        expected_revision: u64,
    ) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        if bundle.session.mode != Mode::Mock {
            return Err(ServiceError::Invalid("模試ではありません".to_string()));
        }
        check_revision(&bundle.session, expected_revision)?;
        if !matches!(bundle.session.phase, Phase::Question) {
            return Err(ServiceError::Invalid("いまは回答できません".to_string()));
        }
        if mock_deadline(&bundle.session).is_some_and(|deadline| now >= deadline) {
            return Err(ServiceError::Invalid(
                "制限時間が終了しました。提出してください".to_string(),
            ));
        }
        let item = current_item(&bundle)?.clone();
        let workbook = storage.workbook(&item.workbook_id)?;
        let question = workbook
            .find_question(&item.note_id, &item.question_id)
            .map(|(_, question)| question.clone())
            .ok_or_else(|| ServiceError::NotFound("問題".to_string()))?;
        if answer_is_blank(answer) {
            return Err(ServiceError::Invalid("回答を入力してください".to_string()));
        }
        let correct = match grade_answer_detailed(&question, answer).map(|result| result.status) {
            Some(GradeStatus::Correct) => Some(true),
            Some(GradeStatus::Incorrect | GradeStatus::NearMiss) => Some(false),
            Some(GradeStatus::Manual) | None => None,
        };
        record_time(&mut bundle.session, now);
        let answer_json = answer
            .map(serde_json::to_string)
            .transpose()?
            .unwrap_or_else(|| "null".to_string());
        bundle.session.draft_json = Some(answer_json.clone());
        bundle.session.revision += 1;
        let mock_answer = MockAnswerRow {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            item_id: item.id.clone(),
            workbook_id: item.workbook_id.clone(),
            note_id: item.note_id.clone(),
            question_id: item.question_id.clone(),
            answered_at: now,
            correct,
            answer_json,
            created_at: now,
        };
        storage.record_mock_answer(&mock_answer, &bundle.session)?;
        if let Some(row) = bundle.items.iter_mut().find(|row| row.id == item.id) {
            row.status = SessionItemStatus::Answered;
        }
        self.advance(&mut storage, &mut bundle, now, &settings, &calendar)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn submit_mock(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let mut bundle = storage.session_bundle(session_id)?;
        if bundle.session.mode != Mode::Mock {
            return Err(ServiceError::Invalid("模試ではありません".to_string()));
        }
        let now = self.clock.now();
        if bundle.session.status == SessionStatus::Ended {
            return self.view_for(&storage, &bundle, &settings, &calendar, now);
        }
        check_revision(&bundle.session, expected_revision)?;
        ensure_active(&bundle)?;
        let exam_id = bundle
            .session
            .exam_id
            .clone()
            .ok_or_else(|| ServiceError::Invalid("試験が指定されていません".to_string()))?;
        let answers = storage.mock_answers_for_session(session_id)?;
        let total = bundle.items.len() as u32;
        let correct = answers
            .iter()
            .filter(|answer| answer.correct == Some(true))
            .count() as u32;
        let incorrect = answers
            .iter()
            .filter(|answer| answer.correct == Some(false))
            .count() as u32;
        let ungraded = answers
            .iter()
            .filter(|answer| answer.correct.is_none())
            .count() as u32;
        let unanswered = total.saturating_sub(answers.len() as u32);
        let submitted_at = now;
        let duration_ms = (submitted_at - bundle.session.started_at)
            .num_milliseconds()
            .max(0);
        let result = MockResultRow {
            session_id: session_id.to_string(),
            exam_id,
            submitted_at,
            total,
            correct,
            incorrect,
            unanswered,
            ungraded,
            duration_ms,
        };
        bundle.session.status = SessionStatus::Ended;
        bundle.session.ended_at = Some(submitted_at);
        bundle.session.phase = Phase::OverallResult;
        bundle.session.current_item_id = None;
        bundle.session.revision += 1;
        storage.finish_mock(&result, &bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    // ---- study session --------------------------------------------------

    pub fn start_session(&self, mode: Mode, workbook_id: Option<&str>) -> Result<()> {
        self.start_session_with_budget(mode, workbook_id, None)
    }

    /// Starts a session whose item list is chosen to fit `budget_seconds`.
    ///
    /// The budget is a selection target only. FSRS `due_at` values are read
    /// and never rewritten; unselected questions stay due. The returned
    /// session exposes its target so Study can show progress, but it never
    /// force-finishes an answer.
    pub fn start_session_with_budget(
        &self,
        mode: Mode,
        workbook_id: Option<&str>,
        budget_seconds: Option<u32>,
    ) -> Result<()> {
        if budget_seconds == Some(0) {
            return Err(ServiceError::Invalid(
                "学習時間は1分以上を指定してください".to_string(),
            ));
        }
        let mut storage = self.storage()?;
        if storage.active_session()?.is_some() {
            return Err(ServiceError::Conflict(
                "中断中のセッションを再開するか、終了してください".to_string(),
            ));
        }
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let introduced = storage.new_questions_introduced_today(&calendar, now)?;
        let new_remaining = (settings.new_limit as usize).saturating_sub(introduced);
        let estimates = if budget_seconds.is_some() {
            Some(BudgetEstimates::load(&storage)?)
        } else {
            None
        };
        let mut effective_mode = mode;
        let mut candidates = match mode {
            Mode::New => {
                let workbook_id = workbook_id
                    .ok_or_else(|| ServiceError::Invalid("Workbookを選んでください".to_string()))?;
                if new_remaining == 0 {
                    return Err(ServiceError::Invalid(
                        "本日の新規問題の上限に達しました".to_string(),
                    ));
                }
                storage.new_candidates(workbook_id, new_remaining)?
            }
            Mode::Review => {
                if budget_seconds.is_some() {
                    // A time-budgeted review deliberately mixes due reviews
                    // with new content up to the daily limit, matching the
                    // Today priority: short-term, review, then new.
                    let budget_candidates =
                        storage.budget_candidates(workbook_id, now, new_remaining)?;
                    let has_normal = budget_candidates
                        .iter()
                        .any(|candidate| candidate.pool != BudgetPool::LearnAhead);
                    let mut questions = Vec::new();
                    for candidate in budget_candidates {
                        if candidate.pool == BudgetPool::LearnAhead
                            && (has_normal
                                || !candidate
                                    .due_at
                                    .is_some_and(|due| calendar.learn_ahead_allowed(now, due)))
                        {
                            continue;
                        }
                        questions.push(budget_question(
                            estimates.as_ref().expect("budget estimates exist"),
                            candidate.workbook_id,
                            candidate.note_id,
                            candidate.question_id,
                            candidate.pool,
                            candidate.due_at,
                            questions.len() as u32,
                        ));
                    }
                    let selection = select_within_budget(&questions, budget_seconds.unwrap_or(0));
                    if !selection.is_empty() {
                        effective_mode = if selection
                            .selected
                            .iter()
                            .any(|question| question.category == BudgetCategory::New)
                        {
                            Mode::Plan
                        } else {
                            Mode::Review
                        };
                    }
                    selection
                        .selected
                        .into_iter()
                        .map(session_candidate)
                        .collect()
                } else {
                    let due = storage.review_candidates(workbook_id, now)?;
                    if due.is_empty() {
                        storage.upcoming_short_term(
                            workbook_id,
                            now,
                            now + Duration::minutes(20),
                        )?
                    } else {
                        due
                    }
                }
            }
            Mode::Practice => {
                let workbook_id = workbook_id
                    .ok_or_else(|| ServiceError::Invalid("Workbookを選んでください".to_string()))?;
                let candidates = storage.practice_candidates(workbook_id)?;
                if budget_seconds.is_some() {
                    let questions: Vec<BudgetQuestion> = candidates
                        .iter()
                        .enumerate()
                        .map(|(index, candidate)| {
                            budget_question(
                                estimates.as_ref().expect("budget estimates exist"),
                                candidate.workbook_id.clone(),
                                candidate.note_id.clone(),
                                candidate.question_id.clone(),
                                BudgetPool::DueReview,
                                candidate.due_at,
                                index as u32,
                            )
                        })
                        .collect();
                    select_within_budget(&questions, budget_seconds.unwrap_or(0))
                        .selected
                        .into_iter()
                        .map(session_candidate)
                        .collect()
                } else {
                    candidates
                }
            }
            Mode::Plan => {
                return Err(ServiceError::Invalid(
                    "計画学習は試験画面から開始してください".to_string(),
                ));
            }
            Mode::Mock => {
                return Err(ServiceError::Invalid(
                    "模試は試験画面から開始してください".to_string(),
                ));
            }
        };
        if budget_seconds.is_some() && mode == Mode::New {
            let questions: Vec<BudgetQuestion> = candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    budget_question(
                        estimates.as_ref().expect("budget estimates exist"),
                        candidate.workbook_id.clone(),
                        candidate.note_id.clone(),
                        candidate.question_id.clone(),
                        BudgetPool::New,
                        candidate.due_at,
                        index as u32,
                    )
                })
                .collect();
            candidates = select_within_budget(&questions, budget_seconds.unwrap_or(0))
                .selected
                .into_iter()
                .map(session_candidate)
                .collect();
        }
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "対象の問題はありません（関連問題は翌学習日まで延期される場合があります）"
                    .to_string(),
            ));
        }
        let (bundle, first_note) =
            build_session_bundle(effective_mode, &candidates, now, None, None, budget_seconds)?;
        let first_item = bundle
            .items
            .first()
            .ok_or_else(|| ServiceError::Invalid("問題を選択できませんでした".to_string()))?
            .clone();
        let mut session = bundle.session;
        session.current_workbook_id = Some(first_item.workbook_id.clone());
        session.current_item_id = Some(first_item.id.clone());
        session.cursor_kind = "initial".to_string();
        session.phase = if matches!(effective_mode, Mode::New | Mode::Plan)
            && !storage.note_introduced(&first_item.workbook_id, &first_note)?
        {
            Phase::Intro
        } else {
            Phase::Question
        };
        session.question_started_at = Some(now);
        session.question_elapsed_ms = 0;
        session.revision = 0;
        let bundle = SessionBundle { session, ..bundle };
        storage.create_session(&bundle)?;
        let mut workbooks = std::collections::BTreeSet::new();
        for item in &bundle.items {
            workbooks.insert(item.workbook_id.clone());
        }
        for workbook in workbooks {
            storage.touch_workbook(&workbook, now)?;
        }
        Ok(())
    }

    pub fn next_wake_at(&self) -> Result<Option<DateTime<Utc>>> {
        let storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let next_due = storage.next_due_at(now)?;
        let mock_deadline = storage
            .active_session()?
            .and_then(|bundle| mock_deadline(&bundle.session));
        // Refresh at the next 4 a.m. boundary even when no card is due, so
        // Today's learning-day counts and date caption switch over.
        let day_start = Some(calendar.next_learning_day_start(now));
        Ok([next_due, day_start, mock_deadline]
            .into_iter()
            .flatten()
            .min())
    }

    pub fn active_view(&self) -> Result<Option<StudyView>> {
        let storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        match storage.active_session()? {
            Some(bundle) => Ok(Some(
                self.view_for(&storage, &bundle, &settings, &calendar, now)?,
            )),
            None => Ok(None),
        }
    }

    pub fn session_view(&self, session_id: &str) -> Result<StudyView> {
        let storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let bundle = storage.session_bundle(session_id)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn begin_question(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Intro {
            return Err(ServiceError::Invalid(
                "知識カードの画面ではありません".to_string(),
            ));
        }
        bundle.session.phase = Phase::Question;
        bundle.session.question_started_at = Some(now);
        bundle.session.question_elapsed_ms = 0;
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn save_draft(
        &self,
        session_id: &str,
        answer: Option<&Answer>,
        hint: bool,
        revealed: bool,
        expected_revision: u64,
    ) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if !matches!(bundle.session.phase, Phase::Intro | Phase::Question) {
            return Err(ServiceError::Invalid(
                "いまは回答を編集できません".to_string(),
            ));
        }
        record_time(&mut bundle.session, now);
        bundle.session.draft_json = answer
            .map(serde_json::to_value)
            .transpose()?
            .map(|value| value.to_string());
        bundle.session.hint |= hint;
        bundle.session.revealed |= revealed;
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn evaluate_answer(
        &self,
        session_id: &str,
        answer: Option<&Answer>,
        unknown: bool,
        self_correct: Option<bool>,
        hint: bool,
        expected_revision: u64,
    ) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Question {
            return Err(ServiceError::Invalid(
                "問題を表示しているときだけ回答できます".to_string(),
            ));
        }
        let item = current_item(&bundle)?.clone();
        let workbook = storage.workbook(&item.workbook_id)?;
        let question = workbook
            .find_question(&item.note_id, &item.question_id)
            .map(|(_, question)| question.clone())
            .ok_or_else(|| ServiceError::NotFound("問題".to_string()))?;
        if question.kind == workbook_domain::QuestionType::Flashcard
            && !unknown
            && (self_correct.is_none() || !bundle.session.revealed)
        {
            return Err(ServiceError::Invalid(
                "解答を表示して自己評価してください".to_string(),
            ));
        }
        if !unknown && question.kind != QuestionType::Flashcard && answer_is_blank(answer) {
            return Err(ServiceError::Invalid("回答を入力してください".to_string()));
        }
        let grade = if unknown {
            GradeResult::incorrect()
        } else if question.kind == QuestionType::Flashcard {
            match self_correct {
                Some(true) => GradeResult::correct(),
                Some(false) => GradeResult::incorrect(),
                None => GradeResult::manual(),
            }
        } else {
            grade_answer_detailed(&question, answer).unwrap_or_else(GradeResult::manual)
        };
        record_time(&mut bundle.session, now);
        bundle.session.draft_json = answer
            .map(serde_json::to_value)
            .transpose()?
            .map(|value| value.to_string());
        bundle.session.hint |= hint;
        let mut assist = AssistState::from_json(bundle.session.assist_json.as_deref());
        bundle.session.pending_unknown = unknown;
        bundle.session.pending_at = Some(now);
        bundle.session.phase = Phase::Feedback;
        bundle.session.revision += 1;

        if grade.status == GradeStatus::NearMiss {
            // A near miss is not an answer. Keep it as assistance evidence and
            // stop before any FSRS state or attempt is written.
            assist.history.push(NearMissRecord {
                answer: answer.cloned(),
                result: grade.clone(),
                answered_at: now,
            });
            assist.pending = Some(grade.clone());
            assist.final_grade = None;
            bundle.session.pending_correct = None;
            bundle.session.assist_json = Some(serde_json::to_string(&assist)?);
            storage.update_session(&bundle.session)?;
            return self.view_for(&storage, &bundle, &settings, &calendar, now);
        }

        assist.pending = None;
        assist.final_grade = Some(grade.clone());
        bundle.session.assist_json = Some(serde_json::to_string(&assist)?);
        bundle.session.pending_correct = match grade.status {
            GradeStatus::Correct => Some(true),
            GradeStatus::Incorrect => Some(false),
            GradeStatus::Manual => None,
            GradeStatus::NearMiss => unreachable!(),
        };

        // Automatic rating commits in the same service call. This keeps the
        // answer, FSRS state, attempt, and session cursor crash-safe: either
        // the whole answer is saved or none of it is.
        let assisted = bundle.session.hint || assist.assisted();
        let automatic = if settings.automatic_rating {
            automatic_rating_for(&question, bundle.session.pending_correct, assisted, unknown)
        } else {
            None
        };
        match automatic {
            Some(rating) => self.commit_pending(
                &mut storage,
                &mut bundle,
                &uuid::Uuid::new_v4().to_string(),
                rating,
                RatingSource::Automatic,
                now,
                &settings,
                &calendar,
            )?,
            None => storage.update_session(&bundle.session)?,
        }
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    /// Returns to the question after a near miss. The draft answer is kept so
    /// the learner can fix it, and the near-miss history stays for the final
    /// attempt and for backup/restore.
    pub fn retry_answer(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Feedback {
            return Err(ServiceError::Invalid(
                "再回答できる画面ではありません".to_string(),
            ));
        }
        let mut assist = AssistState::from_json(bundle.session.assist_json.as_deref());
        if assist.pending.is_none() {
            return Err(ServiceError::Invalid(
                "再回答が必要な回答ではありません".to_string(),
            ));
        }
        assist.pending = None;
        assist.final_grade = None;
        bundle.session.assist_json = Some(serde_json::to_string(&assist)?);
        bundle.session.pending_correct = None;
        bundle.session.pending_unknown = false;
        bundle.session.pending_at = None;
        bundle.session.phase = Phase::Question;
        bundle.session.question_started_at = Some(now);
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn commit_rating(
        &self,
        session_id: &str,
        attempt_id: &str,
        rating: Rating,
        expected_revision: u64,
    ) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        if let Some(existing) = storage.attempt(attempt_id)? {
            if existing.session_id != session_id {
                return Err(ServiceError::Conflict(
                    "Attempt IDが別のセッションで使われています".to_string(),
                ));
            }
            return self.view_for(&storage, &bundle, &settings, &calendar, now);
        }
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Feedback {
            return Err(ServiceError::Invalid(
                "回答を確認してから評価してください".to_string(),
            ));
        }
        let assist = AssistState::from_json(bundle.session.assist_json.as_deref());
        if assist.pending.is_some() {
            return Err(ServiceError::Invalid(
                "おしい回答です。もう一度回答してください".to_string(),
            ));
        }
        self.commit_pending(
            &mut storage,
            &mut bundle,
            attempt_id,
            rating,
            RatingSource::Manual,
            now,
            &settings,
            &calendar,
        )?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_pending(
        &self,
        storage: &mut Storage,
        bundle: &mut SessionBundle,
        attempt_id: &str,
        rating: Rating,
        rating_source: RatingSource,
        now: DateTime<Utc>,
        settings: &Settings,
        calendar: &StudyCalendar,
    ) -> Result<()> {
        if bundle.session.phase != Phase::Feedback {
            return Err(ServiceError::Invalid(
                "回答を確認してから評価してください".to_string(),
            ));
        }
        let pending_at = bundle
            .session
            .pending_at
            .ok_or_else(|| ServiceError::Invalid("回答時刻が記録されていません".to_string()))?;
        let item = current_item(bundle)?.clone();
        let before = storage.review_state(&item.workbook_id, &item.question_id)?;
        let card = if matches!(bundle.session.mode, Mode::Practice | Mode::Mock) {
            None
        } else {
            Some(
                Scheduler::new(SchedulerSettings {
                    desired_retention: settings.retention,
                    ..SchedulerSettings::default()
                })?
                .review(before.as_ref(), rating, pending_at)?,
            )
        };
        let cursor_short_term = bundle.session.cursor_kind == "short_term";
        // An initial item can still be a Learning/Relearning card carried over
        // from an earlier session. Count the attempt as short-term from the
        // saved card state, while item completion depends on the cursor kind.
        let short_term = cursor_short_term
            || before
                .as_ref()
                .is_some_and(|card| card.state.is_short_term());
        let answer_json = bundle
            .session
            .draft_json
            .clone()
            .unwrap_or_else(|| "null".to_string());
        let assist_json = bundle.session.assist_json.clone();
        let before_json = before.as_ref().map(serde_json::to_string).transpose()?;
        let after_json = card.as_ref().map(serde_json::to_string).transpose()?;
        let question_revision = storage.question_revision(&item.workbook_id, &item.question_id)?;
        let attempt = AttemptRow {
            id: attempt_id.to_string(),
            session_id: bundle.session.id.clone(),
            workbook_id: item.workbook_id.clone(),
            note_id: item.note_id.clone(),
            question_id: item.question_id.clone(),
            answered_at: pending_at,
            correct: bundle.session.pending_correct,
            unknown: bundle.session.pending_unknown,
            hint: bundle.session.hint,
            elapsed_ms: bundle.session.question_elapsed_ms,
            rating: rating.value(),
            rating_source,
            question_revision,
            answer_json,
            before_state_json: before_json,
            after_state_json: after_json,
            state_revision: card.as_ref().map(|card| card.revision).unwrap_or(0),
            short_term,
            assist_json,
            created_at: now,
        };
        let bury_until = if matches!(bundle.session.mode, Mode::Review | Mode::Plan) {
            Some(calendar.next_learning_day_start(pending_at))
        } else {
            None
        };
        bundle.session.pending_correct = None;
        bundle.session.pending_unknown = false;
        bundle.session.pending_at = None;
        bundle.session.assist_json = None;
        bundle.session.revision += 1;
        let committed_attempt = attempt.clone();
        storage.commit_attempt(CommitWrite {
            attempt,
            session: bundle.session.clone(),
            card,
            item_id: Some(item.id.clone()),
            mark_answered: !cursor_short_term,
            introduce_note: !matches!(bundle.session.mode, Mode::Practice | Mode::Mock),
            bury_until,
        })?;
        bundle.attempts.push(committed_attempt);
        if !cursor_short_term
            && let Some(row) = bundle.items.iter_mut().find(|row| row.id == item.id)
        {
            row.status = SessionItemStatus::Answered;
        }
        Ok(())
    }

    pub fn correct_attempt(
        &self,
        session_id: &str,
        attempt_id: &str,
        correct: Option<bool>,
        rating: Option<Rating>,
        expected_revision: u64,
    ) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Feedback {
            return Err(ServiceError::Invalid(
                "いまの回答だけ訂正できます".to_string(),
            ));
        }
        let item = current_item(&bundle)?.clone();
        let current = storage
            .current_attempt(session_id, &item.question_id)?
            .ok_or_else(|| ServiceError::NotFound("回答".to_string()))?;
        if current.id != attempt_id {
            return Err(ServiceError::Conflict(
                "現在の回答だけ訂正できます".to_string(),
            ));
        }
        let mut new_correct = current.correct;
        let mut new_unknown = current.unknown;
        let mut new_rating = current.rating;
        if let Some(correct) = correct {
            new_correct = Some(correct);
            new_unknown = false;
            let assisted =
                current.hint || AssistState::from_json(current.assist_json.as_deref()).assisted();
            if let Some(suggested) = automatic_rating(Some(correct), assisted, false) {
                new_rating = suggested.value();
            }
        }
        if let Some(rating) = rating {
            new_rating = rating.value();
        }
        if correct.is_none() && rating.is_none() {
            return Err(ServiceError::Invalid("訂正内容がありません".to_string()));
        }
        let card = if matches!(bundle.session.mode, Mode::Practice | Mode::Mock) {
            None
        } else {
            let expected_revision = current.state_revision;
            let state = storage
                .review_state(&item.workbook_id, &item.question_id)?
                .ok_or_else(|| ServiceError::NotFound("FSRS状態".to_string()))?;
            if state.revision != expected_revision {
                return Err(ServiceError::Conflict(
                    "回答後に状態が更新されています".to_string(),
                ));
            }
            let before: Option<workbook_domain::StoredCard> = current
                .before_state_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let card = Scheduler::new(SchedulerSettings {
                desired_retention: settings.retention,
                ..SchedulerSettings::default()
            })?
            .review(
                before.as_ref(),
                Rating::from_value(new_rating).unwrap(),
                current.answered_at,
            )?;
            Some(card)
        };
        let mut attempt = current.clone();
        let mut assist = AssistState::from_json(attempt.assist_json.as_deref());
        if let Some(grade) = assist.final_grade.as_mut() {
            grade.status = match new_correct {
                Some(true) => GradeStatus::Correct,
                Some(false) => GradeStatus::Incorrect,
                None => GradeStatus::Manual,
            };
            if grade.status == GradeStatus::Correct {
                grade.diagnostic = None;
                grade.reasons.clear();
            }
            attempt.assist_json = Some(serde_json::to_string(&assist)?);
        }
        attempt.correct = new_correct;
        attempt.unknown = new_unknown;
        attempt.rating = new_rating;
        attempt.rating_source = RatingSource::Manual;
        attempt.after_state_json = card.as_ref().map(serde_json::to_string).transpose()?;
        attempt.state_revision = card.as_ref().map(|card| card.revision).unwrap_or(0);
        bundle.session.revision += 1;
        let corrected = attempt.clone();
        storage.correct_attempt(CorrectionWrite {
            attempt,
            session: bundle.session.clone(),
            card,
        })?;
        if let Some(row) = bundle
            .attempts
            .iter_mut()
            .find(|row| row.id == corrected.id)
        {
            *row = corrected;
        }
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn next(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::Feedback {
            return Err(ServiceError::Invalid(
                "回答を確定してから次へ進んでください".to_string(),
            ));
        }
        if bundle.session.pending_correct.is_some() {
            return Err(ServiceError::Invalid("評価を確定してください".to_string()));
        }
        let item = current_item(&bundle)?.clone();
        if storage
            .current_attempt(session_id, &item.question_id)?
            .is_none()
        {
            return Err(ServiceError::Invalid("評価を確定してください".to_string()));
        }
        self.advance(&mut storage, &mut bundle, now, &settings, &calendar)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn skip(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if !matches!(bundle.session.phase, Phase::Intro | Phase::Question) {
            return Err(ServiceError::Invalid(
                "いまはスキップできません".to_string(),
            ));
        }
        let item = current_item(&bundle)?.clone();
        storage.set_item_status(&item.id, SessionItemStatus::Skipped)?;
        if let Some(row) = bundle.items.iter_mut().find(|row| row.id == item.id) {
            row.status = SessionItemStatus::Skipped;
        }
        self.advance(&mut storage, &mut bundle, now, &settings, &calendar)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    fn advance(
        &self,
        storage: &mut Storage,
        bundle: &mut SessionBundle,
        now: DateTime<Utc>,
        settings: &Settings,
        calendar: &StudyCalendar,
    ) -> Result<()> {
        let workbook_id = bundle
            .session
            .current_workbook_id
            .clone()
            .ok_or_else(|| ServiceError::Invalid("Workbookが選択されていません".to_string()))?;
        let is_exam = bundle.session.exam_id.is_some();
        let mut choice: Option<(String, QueueChoice)> = None;
        let workbook_ids: Vec<String> = if is_exam {
            bundle
                .workbooks
                .iter()
                .map(|workbook| workbook.workbook_id.clone())
                .collect()
        } else {
            vec![workbook_id.clone()]
        };
        for candidate_workbook in workbook_ids {
            let queue = storage.session_queue(&bundle.session.id, &candidate_workbook)?;
            if let Some(selected) = select_next(QueueInput {
                mode: bundle.session.mode,
                workbook_id: &candidate_workbook,
                questions: &queue,
                now,
                calendar,
            }) {
                choice = Some((candidate_workbook, selected));
                break;
            }
        }
        match choice {
            Some((selected_workbook, choice)) => {
                let item = bundle
                    .items
                    .iter()
                    .find(|item| item.id == choice.item_id)
                    .cloned()
                    .ok_or_else(|| ServiceError::NotFound("セッション項目".to_string()))?;
                let intro = matches!(bundle.session.mode, Mode::New | Mode::Plan)
                    && choice.kind == QueueKind::Initial
                    && !storage.note_introduced(&item.workbook_id, &item.note_id)?;
                bundle.session.current_workbook_id = Some(selected_workbook);
                bundle.session.current_item_id = Some(item.id);
                bundle.session.cursor_kind = match choice.kind {
                    QueueKind::Initial => "initial",
                    QueueKind::ShortTerm => "short_term",
                }
                .to_string();
                bundle.session.phase = if intro { Phase::Intro } else { Phase::Question };
                bundle.session.draft_json = None;
                bundle.session.hint = false;
                bundle.session.revealed = false;
                bundle.session.pending_correct = None;
                bundle.session.pending_unknown = false;
                bundle.session.pending_at = None;
                bundle.session.assist_json = None;
                bundle.session.question_started_at = Some(now);
                bundle.session.question_elapsed_ms = 0;
            }
            None if is_exam && bundle.session.mode == Mode::Mock => {
                bundle.session.phase = Phase::WorkbookResult;
                bundle.session.current_item_id = None;
                bundle.session.current_workbook_id = Some(workbook_id);
            }
            None if is_exam => {
                bundle.session.phase = Phase::OverallResult;
                bundle.session.current_item_id = None;
                bundle.session.status = SessionStatus::Ended;
                bundle.session.ended_at = Some(now);
            }
            None => {
                bundle.session.phase = Phase::WorkbookResult;
                bundle.session.current_item_id = None;
                bundle.session.current_workbook_id = Some(workbook_id);
            }
        }
        let _ = settings;
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        Ok(())
    }

    pub fn continue_session(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        if bundle.session.phase != Phase::WorkbookResult {
            return Err(ServiceError::Invalid(
                "教材の結果画面ではありません".to_string(),
            ));
        }
        let current = bundle.session.current_workbook_id.clone();
        if let Some(current) = current.as_ref() {
            storage.set_workbook_ack(session_id, current, true)?;
            if let Some(row) = bundle
                .workbooks
                .iter_mut()
                .find(|row| row.workbook_id == *current)
            {
                row.result_ack = true;
            }
        }
        let mut next_workbook = None;
        let pending_ids: Vec<String> = bundle
            .workbooks
            .iter()
            .filter(|workbook| !workbook.result_ack)
            .map(|workbook| workbook.workbook_id.clone())
            .collect();
        for workbook_id in pending_ids {
            let queue = storage.session_queue(session_id, &workbook_id)?;
            let choice = select_next(QueueInput {
                mode: bundle.session.mode,
                workbook_id: &workbook_id,
                questions: &queue,
                now,
                calendar: &calendar,
            });
            if let Some(choice) = choice {
                next_workbook = Some((workbook_id, choice));
                break;
            }
            storage.set_workbook_ack(session_id, &workbook_id, true)?;
            if let Some(row) = bundle
                .workbooks
                .iter_mut()
                .find(|row| row.workbook_id == workbook_id)
            {
                row.result_ack = true;
            }
        }
        if let Some((workbook_id, choice)) = next_workbook {
            let item = bundle
                .items
                .iter()
                .find(|item| item.id == choice.item_id)
                .cloned()
                .ok_or_else(|| ServiceError::NotFound("セッション項目".to_string()))?;
            let intro = matches!(bundle.session.mode, Mode::New | Mode::Plan)
                && choice.kind == QueueKind::Initial
                && !storage.note_introduced(&item.workbook_id, &item.note_id)?;
            bundle.session.current_workbook_id = Some(workbook_id);
            bundle.session.current_item_id = Some(item.id);
            bundle.session.cursor_kind = match choice.kind {
                QueueKind::Initial => "initial",
                QueueKind::ShortTerm => "short_term",
            }
            .to_string();
            bundle.session.phase = if intro { Phase::Intro } else { Phase::Question };
            bundle.session.draft_json = None;
            bundle.session.hint = false;
            bundle.session.revealed = false;
            bundle.session.pending_correct = None;
            bundle.session.pending_unknown = false;
            bundle.session.pending_at = None;
            bundle.session.assist_json = None;
            bundle.session.question_started_at = Some(now);
            bundle.session.question_elapsed_ms = 0;
        } else {
            bundle.session.phase = Phase::OverallResult;
            bundle.session.current_item_id = None;
            // Reaching the overall result is a completed session, not an
            // early exit. Record it so a restart does not offer to resume an
            // already-finished summary.
            bundle.session.status = SessionStatus::Ended;
            bundle.session.ended_at = Some(now);
        }
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    pub fn finish(&self, session_id: &str, expected_revision: u64) -> Result<StudyView> {
        let mut storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        let now = self.clock.now();
        let mut bundle = storage.session_bundle(session_id)?;
        if bundle.session.mode == Mode::Mock {
            drop(storage);
            return self.submit_mock(session_id, expected_revision);
        }
        ensure_active(&bundle)?;
        check_revision(&bundle.session, expected_revision)?;
        bundle.session.status = SessionStatus::Ended;
        bundle.session.ended_at = Some(now);
        bundle.session.phase = Phase::OverallResult;
        bundle.session.current_item_id = None;
        bundle.session.pending_correct = None;
        bundle.session.pending_unknown = false;
        bundle.session.pending_at = None;
        bundle.session.assist_json = None;
        bundle.session.revision += 1;
        storage.update_session(&bundle.session)?;
        self.view_for(&storage, &bundle, &settings, &calendar, now)
    }

    // ---- view construction ---------------------------------------------

    fn view_for(
        &self,
        storage: &Storage,
        bundle: &SessionBundle,
        settings: &Settings,
        calendar: &StudyCalendar,
        now: DateTime<Utc>,
    ) -> Result<StudyView> {
        let _ = calendar;
        let _ = settings;
        let session = &bundle.session;
        let workbook_id = session.current_workbook_id.clone();
        let workbook = workbook_id
            .as_deref()
            .map(|id| storage.workbook(id))
            .transpose()?;
        let item = session
            .current_item_id
            .as_ref()
            .and_then(|id| bundle.items.iter().find(|item| &item.id == id));
        let note = workbook.as_ref().and_then(|workbook| {
            item.and_then(|item| workbook.notes.iter().find(|note| note.id == item.note_id))
        });
        let question = workbook.as_ref().and_then(|workbook| {
            item.and_then(|item| {
                workbook
                    .find_question(&item.note_id, &item.question_id)
                    .map(|(_, question)| question.clone())
            })
        });
        let mut assets = Vec::new();
        if let (Some(workbook_id), Some(question)) = (workbook_id.as_deref(), question.as_ref()) {
            let mut ids = question.referenced_asset_ids();
            if let Some(note) = note {
                note.knowledge_card.referenced_asset_ids(&mut ids);
            }
            for id in ids {
                if let Some(asset) = storage.asset(workbook_id, &id)? {
                    let fingerprint = asset_fingerprint(
                        &asset.media_type,
                        asset.width,
                        asset.height,
                        &asset.bytes,
                    );
                    assets.push(AssetView {
                        id: asset.id,
                        media_type: asset.media_type,
                        alt: asset.alt,
                        width: asset.width,
                        height: asset.height,
                        fingerprint,
                        bytes: Arc::new(asset.bytes),
                    });
                }
            }
        }
        let intro = if session.phase == Phase::Intro {
            note.map(|note| KnowledgeIntro {
                note_title: note.title.clone(),
                content: note.knowledge_card.clone(),
            })
        } else {
            None
        };
        let draft = session
            .draft_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let feedback = if session.phase == Phase::Feedback {
            match question.as_ref() {
                Some(question) => self.feedback_for(bundle, session, item, question)?,
                None => None,
            }
        } else {
            None
        };
        let mut progress = StudyProgress::default();
        if session.exam_id.is_some()
            || session.time_budget_seconds.is_some()
            || bundle.workbooks.len() > 1
        {
            progress = progress_for_exam(bundle, item);
        } else if let Some(workbook_id) = workbook_id.as_deref() {
            progress = progress_for(bundle, workbook_id, item);
        }
        let workbook_result = if session.phase == Phase::WorkbookResult {
            workbook_id.as_deref().map(|workbook_id| {
                let title = workbook
                    .as_ref()
                    .map(|workbook| workbook.title.clone())
                    .unwrap_or_default();
                let stats = stats_for(storage, bundle, Some(workbook_id));
                let next = bundle
                    .workbooks
                    .iter()
                    .filter(|row| !row.result_ack && row.workbook_id != workbook_id)
                    .filter_map(|row| storage.workbook(&row.workbook_id).ok())
                    .map(|workbook| workbook.title)
                    .next();
                WorkbookResultView {
                    workbook_id: workbook_id.to_string(),
                    title,
                    stats,
                    next_workbook_title: next,
                }
            })
        } else {
            None
        };
        let overall_result = if session.phase == Phase::OverallResult {
            let workbooks = bundle
                .workbooks
                .iter()
                .filter_map(|row| storage.workbook(&row.workbook_id).ok())
                .map(|workbook| workbook.title)
                .collect();
            Some(OverallResultView {
                stats: stats_for(storage, bundle, None),
                workbooks,
            })
        } else {
            None
        };
        let exam = session
            .exam_id
            .as_deref()
            .map(|id| storage.exam(id))
            .transpose()?;
        let goal = session
            .goal_id
            .as_deref()
            .map(|id| storage.goal(id))
            .transpose()
            .ok()
            .flatten();
        let mock = if session.mode == Mode::Mock {
            let answered = bundle
                .items
                .iter()
                .filter(|item| item.status == SessionItemStatus::Answered)
                .count();
            let remaining_ms =
                mock_deadline(session).map(|deadline| (deadline - now).num_milliseconds().max(0));
            Some(MockSessionView {
                total: bundle.items.len(),
                answered,
                remaining_ms,
                submitted: session.status == SessionStatus::Ended,
            })
        } else {
            None
        };
        let mock_result = if session.mode == Mode::Mock && session.phase == Phase::OverallResult {
            storage
                .mock_result(&session.id)?
                .map(|result| build_mock_result_view(storage, bundle, &result))
                .transpose()?
        } else {
            None
        };
        let budget = session.time_budget_seconds.map(|target_seconds| {
            let target_ms = i64::from(target_seconds) * 1_000;
            let used_ms = bundle
                .attempts
                .iter()
                .map(|attempt| attempt.elapsed_ms.max(0))
                .sum::<i64>();
            SessionBudgetView {
                target_seconds,
                used_ms,
                remaining_ms: target_ms - used_ms,
                reached: used_ms >= target_ms,
            }
        });
        Ok(StudyView {
            session_id: session.id.clone(),
            mode: session.mode,
            phase: session.phase,
            revision: session.revision,
            workbook_id,
            workbook_title: workbook.as_ref().map(|workbook| workbook.title.clone()),
            note_id: item.map(|item| item.note_id.clone()),
            note_title: note.map(|note| note.title.clone()),
            item_kind: session.cursor_kind.clone(),
            question: question.clone(),
            assets,
            intro,
            draft,
            hint: question.as_ref().and_then(|question| question.hint.clone()),
            hint_visible: session.hint,
            revealed: session.revealed,
            feedback,
            progress,
            workbook_result,
            overall_result,
            exam_id: session.exam_id.clone(),
            exam_title: exam.map(|exam| exam.title),
            goal_id: session.goal_id.clone(),
            goal_title: goal.map(|goal| goal.title),
            budget,
            mock,
            mock_result,
        })
    }

    fn feedback_for(
        &self,
        bundle: &SessionBundle,
        session: &SessionRow,
        item: Option<&SessionItemRow>,
        question: &Question,
    ) -> Result<Option<FeedbackView>> {
        let Some(item) = item else {
            return Ok(None);
        };
        let answer = session
            .draft_json
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let assist = AssistState::from_json(session.assist_json.as_deref());
        if let Some(pending) = assist.pending.as_ref() {
            return Ok(Some(FeedbackView {
                attempt_id: None,
                committed: false,
                status: GradeStatus::NearMiss,
                correct: None,
                unknown: false,
                hint: session.hint,
                assisted: true,
                retry_count: assist.retry_count(),
                near_miss_reasons: pending.reasons.clone(),
                near_miss_messages: pending.messages(),
                diagnostic: pending.diagnostic.clone(),
                rating: None,
                rating_source: None,
                suggested_rating: None,
                explanation: question.explanation.clone(),
                answer_summary: question.answer_summary(),
                given_answer: format_given_answer(question, answer.as_ref()),
                elapsed_ms: session.question_elapsed_ms,
            }));
        }
        if let Some(correct) = session.pending_correct {
            let assisted = session.hint || assist.assisted();
            let suggested =
                automatic_rating_for(question, Some(correct), assisted, session.pending_unknown);
            return Ok(Some(FeedbackView {
                attempt_id: None,
                committed: false,
                status: if correct {
                    GradeStatus::Correct
                } else {
                    GradeStatus::Incorrect
                },
                correct: Some(correct),
                unknown: session.pending_unknown,
                hint: session.hint,
                assisted,
                retry_count: assist.retry_count(),
                near_miss_reasons: Vec::new(),
                near_miss_messages: Vec::new(),
                diagnostic: None,
                rating: None,
                rating_source: None,
                suggested_rating: suggested,
                explanation: question.explanation.clone(),
                answer_summary: question.answer_summary(),
                given_answer: format_given_answer(question, answer.as_ref()),
                elapsed_ms: session.question_elapsed_ms,
            }));
        }
        let attempt = bundle
            .attempts
            .iter()
            .filter(|attempt| {
                attempt.question_id == item.question_id && attempt.workbook_id == item.workbook_id
            })
            .max_by_key(|attempt| (attempt.answered_at, attempt.created_at));
        let Some(attempt) = attempt else {
            return Ok(None);
        };
        let answer = serde_json::from_str::<Value>(&attempt.answer_json).ok();
        let assist = AssistState::from_json(attempt.assist_json.as_deref());
        let assisted = attempt.hint || assist.assisted();
        let status = assist
            .final_grade
            .as_ref()
            .map(|grade| grade.status)
            .unwrap_or(match attempt.correct {
                Some(true) => GradeStatus::Correct,
                Some(false) => GradeStatus::Incorrect,
                None => GradeStatus::Manual,
            });
        Ok(Some(FeedbackView {
            attempt_id: Some(attempt.id.clone()),
            committed: true,
            status,
            correct: attempt.correct,
            unknown: attempt.unknown,
            hint: attempt.hint,
            assisted,
            retry_count: assist.retry_count(),
            near_miss_reasons: Vec::new(),
            near_miss_messages: Vec::new(),
            diagnostic: assist
                .final_grade
                .as_ref()
                .and_then(|grade| grade.diagnostic.clone()),
            rating: Rating::from_value(attempt.rating),
            rating_source: Some(attempt.rating_source),
            suggested_rating: automatic_rating_for(
                question,
                attempt.correct,
                assisted,
                attempt.unknown,
            ),
            explanation: question.explanation.clone(),
            answer_summary: question.answer_summary(),
            given_answer: format_given_answer(question, answer.as_ref()),
            elapsed_ms: attempt.elapsed_ms,
        }))
    }

    // ---- history --------------------------------------------------------

    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let storage = self.storage()?;
        let mut titles = std::collections::BTreeMap::new();
        for record in storage.workbooks()? {
            titles.insert(record.local_id, parse_workbook(&record.content_json)?.title);
        }
        let bundles = storage.history(limit)?;
        Ok(bundles
            .into_iter()
            .map(|bundle| HistoryEntry {
                session_id: bundle.session.id.clone(),
                mode: bundle.session.mode,
                started_at: bundle.session.started_at,
                ended_at: bundle.session.ended_at,
                workbook_titles: bundle
                    .workbooks
                    .iter()
                    .filter_map(|row| titles.get(&row.workbook_id).cloned())
                    .collect(),
                stats: stats_from_counts(counts_for_workbook(&bundle, None), &bundle, None),
            })
            .collect())
    }

    // ---- migration, backup, restore ------------------------------------

    pub fn legacy_preview(path: &std::path::Path) -> Result<LegacyPreview> {
        Ok(workbook_import::inspect(path)?)
    }

    pub fn import_legacy(&self, path: &std::path::Path) -> Result<LegacyPreview> {
        let plan: LegacyPlan = workbook_import::read_legacy(path)?;
        let preview = workbook_import::preview_of(&plan);
        let mut storage = self.storage()?;
        self.write_action_backup(&storage, "pre-import")?;
        storage.apply_legacy_plan(&plan)?;
        drop(storage);
        self.recover_active()?;
        Ok(preview)
    }

    pub fn backup_to(&self, path: &std::path::Path) -> Result<()> {
        Ok(self.storage()?.backup_to(path)?)
    }

    pub fn daily_backup(&self) -> Result<Option<std::path::PathBuf>> {
        let storage = self.storage()?;
        let settings = storage.settings()?;
        let calendar =
            StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
        Ok(storage.daily_backup(&calendar, self.clock.now(), 7)?)
    }

    pub fn backup_files(&self) -> Result<Vec<std::path::PathBuf>> {
        Ok(self.storage()?.backup_files()?)
    }

    pub fn validate_backup(path: &std::path::Path) -> Result<()> {
        Ok(Storage::validate_backup(path)?)
    }

    pub fn restore_from(&self, path: &std::path::Path) -> Result<()> {
        self.storage()?.restore_from(path)?;
        self.recover_active()
    }

    pub fn reset_history(&self) -> Result<()> {
        let mut storage = self.storage()?;
        if storage.active_session()?.is_some() {
            return Err(ServiceError::Conflict(
                "学習中のセッションを終了してからリセットしてください".to_string(),
            ));
        }
        self.write_action_backup(&storage, "pre-reset")?;
        storage.reset_history()?;
        Ok(())
    }

    fn recover_active(&self) -> Result<()> {
        let mut storage = self.storage()?;
        let Some(mut bundle) = storage.active_session()? else {
            return Ok(());
        };
        if matches!(bundle.session.phase, Phase::Intro | Phase::Question) {
            bundle.session.question_started_at = Some(self.clock.now());
            storage.update_session(&bundle.session)?;
        }
        Ok(())
    }
}

fn exam_catalog(storage: &Storage) -> Result<Vec<ExamCatalogQuestion>> {
    let mut topics_by_question: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (workbook_id, question_id, topic_id) in storage.question_topics()? {
        topics_by_question
            .entry((workbook_id, question_id))
            .or_default()
            .push(topic_id);
    }
    let mut catalog = Vec::new();
    for record in storage.workbooks()? {
        let workbook = parse_workbook(&record.content_json)?;
        for note in &workbook.notes {
            for question in &note.questions {
                let key = (record.local_id.clone(), question.id.clone());
                let mut topic_ids = topics_by_question.get(&key).cloned().unwrap_or_default();
                topic_ids.sort();
                topic_ids.dedup();
                catalog.push(ExamCatalogQuestion {
                    workbook_id: record.local_id.clone(),
                    workbook_title: workbook.title.clone(),
                    note_id: note.id.clone(),
                    question_id: question.id.clone(),
                    question_prompt: question.prompt_text(),
                    question_kind: question.kind.as_str().to_string(),
                    workbook_tags: workbook.tags.clone(),
                    topic_ids,
                });
            }
        }
    }
    catalog.sort_by(|a, b| {
        a.workbook_title
            .cmp(&b.workbook_title)
            .then(a.note_id.cmp(&b.note_id))
            .then(a.question_id.cmp(&b.question_id))
    });
    Ok(catalog)
}

/// Expands selected topics to their descendants before exact-ID resolution.
/// A scope therefore follows a hierarchy without freezing the descendant list
/// at save time: new child topics are included on the next resolution.
fn scope_with_topic_descendants(scope: &ExamScope, topics: &[workbook_domain::Topic]) -> ExamScope {
    let mut expanded = scope.clone();
    for group in &mut expanded.groups {
        let ids = expand_topic_ids(topics, &group.topics);
        group.topics = ids.into_iter().collect();
    }
    let excluded = expand_topic_ids(topics, &expanded.exclude_topics);
    expanded.exclude_topics = excluded.into_iter().collect();
    expanded
}

fn resolve_scope_for(
    storage: &Storage,
    scope: &ExamScope,
    catalog: &[ExamCatalogQuestion],
) -> Result<Vec<ExamCatalogQuestion>> {
    let topics = storage.topics()?;
    let expanded = scope_with_topic_descendants(scope, &topics);
    Ok(resolve_scope(&expanded, catalog))
}

type FirstAttemptMap = BTreeMap<(String, String), Option<bool>>;

fn goal_from(storage: &Storage, id: &str) -> Result<Goal> {
    match storage.goal(id) {
        Ok(goal) => Ok(goal),
        Err(StorageError::NotFound(_)) => {
            let exam = storage.exam(id)?;
            Ok(Goal::from_exam(&exam))
        }
        Err(error) => Err(error.into()),
    }
}

fn goal_briefs(
    storage: &Storage,
    calendar: &StudyCalendar,
    now: DateTime<Utc>,
) -> Result<Vec<GoalBrief>> {
    let catalog = exam_catalog(storage)?;
    let states: BTreeMap<(String, String), workbook_domain::StoredCard> = storage
        .review_states()?
        .into_iter()
        .map(|(workbook_id, question_id, card)| ((workbook_id, question_id), card))
        .collect();
    let buried = storage.buried_questions(now)?;
    let mut goals: Vec<(Goal, GoalSource)> = storage
        .goals(false)?
        .into_iter()
        .map(|goal| (goal, GoalSource::Goal))
        .collect();
    goals.extend(
        storage
            .exams(false)?
            .into_iter()
            .map(|exam| (Goal::from_exam(&exam), GoalSource::Exam)),
    );
    let mut output = Vec::new();
    for (goal, source) in goals {
        let resolved = resolve_scope_for(storage, &goal.scope, &catalog)?;
        let mut total = 0usize;
        let mut studied = 0usize;
        let mut due = 0usize;
        for question in &resolved {
            let key = (question.workbook_id.clone(), question.question_id.clone());
            let card = states.get(&key);
            total += 1;
            studied += usize::from(card.is_some());
            due += usize::from(
                card.is_some_and(|card| question_is_due(card, buried.contains(&key), now)),
            );
        }
        let today = calendar.learning_day(now);
        let days_remaining = goal.target_date.map(|target| (target - today).num_days());
        output.push(GoalBrief {
            id: goal.id.clone(),
            title: goal.title.clone(),
            source,
            target_date: goal.target_date,
            days_remaining,
            expired: days_remaining.is_some_and(|days| days < 0),
            total,
            studied,
            due,
            coverage: if total == 0 {
                None
            } else {
                Some(studied as f64 / total as f64)
            },
        });
    }
    output.sort_by(|a, b| {
        a.expired
            .cmp(&b.expired)
            .then(a.days_remaining.cmp(&b.days_remaining))
            .then(a.title.cmp(&b.title))
    });
    Ok(output)
}

fn build_goal_plan(
    storage: &Storage,
    goal: &Goal,
    catalog: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    now: DateTime<Utc>,
) -> Result<ExamPlan> {
    let deadline = goal
        .target_date
        .map(|date| exam_deadline(&context.calendar, date));
    let today_start = context
        .calendar
        .day_start(context.calendar.learning_day(now));
    let studied_seconds_today =
        u32::try_from(storage.goal_studied_ms_since(&goal.id, today_start)?.max(0) / 1_000)
            .unwrap_or(u32::MAX);
    let questions = plan_questions_for(
        &resolve_scope_for(storage, &goal.scope, catalog)?,
        context,
        deadline,
        now,
    );
    Ok(plan_exam(PlanInput {
        now,
        calendar: &context.calendar,
        target_date: goal.target_date,
        daily_minutes: goal.daily_minutes,
        new_limit_remaining: context.new_limit_remaining,
        studied_seconds_today,
        questions: &questions,
    }))
}

fn first_pass_counts(resolved: &[ExamCatalogQuestion], first: &FirstAttemptMap) -> (usize, usize) {
    let mut graded = 0usize;
    let mut correct = 0usize;
    for question in resolved {
        let key = (question.workbook_id.clone(), question.question_id.clone());
        if let Some(Some(is_correct)) = first.get(&key) {
            graded += 1;
            correct += usize::from(*is_correct);
        }
    }
    (graded, correct)
}

fn goal_topic_stats(
    storage: &Storage,
    resolved: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    first: &FirstAttemptMap,
    now: DateTime<Utc>,
) -> Result<Vec<GoalTopicStat>> {
    let topics = storage.topics()?;
    let mut output = Vec::new();
    for topic in &topics {
        let effective = expand_topic_ids(&topics, [&topic.id]);
        let mut stat = GoalTopicStat {
            topic_id: topic.id.clone(),
            title: workbook_domain::topic_path(&topics, &topic.id)
                .map(|path| path.join(" / "))
                .unwrap_or_else(|| topic.name.clone()),
            total: 0,
            studied: 0,
            unstudied: 0,
            due: 0,
            weak: 0,
            attempts: 0,
            first_pass_graded: 0,
            first_pass_correct: 0,
            first_pass_accuracy: None,
        };
        for question in resolved {
            if !question.topic_ids.iter().any(|id| effective.contains(id)) {
                continue;
            }
            let key = (question.workbook_id.clone(), question.question_id.clone());
            let card = context.states.get(&key);
            let attempt = context.stats.get(&key);
            stat.total += 1;
            stat.studied += usize::from(card.is_some());
            stat.due += usize::from(
                card.is_some_and(|card| question_is_due(card, context.buried.contains(&key), now)),
            );
            stat.weak += usize::from(question_is_weak(card, attempt));
            stat.attempts += attempt.map(|value| value.attempts).unwrap_or(0);
            if let Some(Some(is_correct)) = first.get(&key) {
                stat.first_pass_graded += 1;
                stat.first_pass_correct += usize::from(*is_correct);
            }
        }
        if stat.total == 0 {
            continue;
        }
        stat.unstudied = stat.total.saturating_sub(stat.studied);
        stat.first_pass_accuracy = if stat.first_pass_graded == 0 {
            None
        } else {
            Some(stat.first_pass_correct as f64 / stat.first_pass_graded as f64)
        };
        output.push(stat);
    }
    output.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(output)
}

fn goal_summary_from(
    storage: &Storage,
    goal: Goal,
    catalog: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    first: &FirstAttemptMap,
    now: DateTime<Utc>,
) -> Result<GoalSummary> {
    let plan = if let Some(exam_id) = goal.exam_id.as_deref() {
        let exam = storage.exam(exam_id)?;
        build_exam_plan(storage, &exam, catalog, context, now)?
    } else {
        build_goal_plan(storage, &goal, catalog, context, now)?
    };
    let resolved = resolve_scope_for(storage, &goal.scope, catalog)?;
    let topics = goal_topic_stats(storage, &resolved, context, first, now)?;
    let (first_pass_graded, first_pass_correct) = first_pass_counts(&resolved, first);
    let coverage = plan.coverage.clone();
    Ok(GoalSummary {
        source: goal.source(),
        total: coverage.total,
        studied: coverage.studied,
        unstudied: coverage.unstudied,
        due: coverage.due,
        weak: coverage.weak,
        days_remaining: plan.days_remaining,
        expired: plan.expired,
        coverage: if coverage.total == 0 {
            None
        } else {
            Some(coverage.studied as f64 / coverage.total as f64)
        },
        first_pass_graded,
        first_pass_correct,
        first_pass_accuracy: if first_pass_graded == 0 {
            None
        } else {
            Some(first_pass_correct as f64 / first_pass_graded as f64)
        },
        plan,
        topics,
        goal,
    })
}

fn question_is_due(card: &workbook_domain::StoredCard, buried: bool, now: DateTime<Utc>) -> bool {
    card.due_at <= now && !(card.state == CardState::Review && buried)
}

fn question_is_weak(
    card: Option<&workbook_domain::StoredCard>,
    stat: Option<&workbook_storage::QuestionAttemptStat>,
) -> bool {
    card.is_some_and(|card| card.lapses > 0 || card.difficulty >= 7.0)
        || stat.is_some_and(|stat| stat.incorrect > 0)
}

fn topic_summaries(storage: &Storage, now: DateTime<Utc>) -> Result<Vec<TopicSummary>> {
    let catalog = exam_catalog(storage)?;
    let topics = storage.topics()?;
    let context = exam_plan_context(storage, now)?;
    let mut output = Vec::new();
    for topic in &topics {
        let effective = expand_topic_ids(&topics, [&topic.id]);
        let mut summary = TopicSummary {
            id: topic.id.clone(),
            name: topic.name.clone(),
            parent_id: topic.parent_id.clone(),
            path: workbook_domain::topic_path(&topics, &topic.id)
                .map(|path| path.join(" / "))
                .unwrap_or_else(|| topic.name.clone()),
            description: topic.description.clone(),
            question_count: 0,
            studied: 0,
            due: 0,
            weak: 0,
        };
        for question in &catalog {
            if !question.topic_ids.iter().any(|id| effective.contains(id)) {
                continue;
            }
            let key = (question.workbook_id.clone(), question.question_id.clone());
            let card = context.states.get(&key);
            let stat = context.stats.get(&key);
            summary.question_count += 1;
            if card.is_some() {
                summary.studied += 1;
            }
            if card.is_some_and(|card| question_is_due(card, context.buried.contains(&key), now)) {
                summary.due += 1;
            }
            if question_is_weak(card, stat) {
                summary.weak += 1;
            }
        }
        output.push(summary);
    }
    output.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

fn scope_preview_inner(
    storage: &Storage,
    scope: &ExamScope,
    now: DateTime<Utc>,
) -> Result<ScopePreview> {
    let catalog = exam_catalog(storage)?;
    let resolved = resolve_scope_for(storage, scope, &catalog)?;
    let topics = storage.topics()?;
    let context = exam_plan_context(storage, now)?;
    let mut preview = ScopePreview::default();
    let mut workbook_counts: BTreeMap<String, ScopeCount> = BTreeMap::new();
    for question in &resolved {
        let key = (question.workbook_id.clone(), question.question_id.clone());
        let card = context.states.get(&key);
        let stat = context.stats.get(&key);
        let due =
            card.is_some_and(|card| question_is_due(card, context.buried.contains(&key), now));
        let weak = question_is_weak(card, stat);
        preview.total += 1;
        if card.is_some() {
            preview.studied += 1;
        }
        if due {
            preview.due += 1;
        }
        if weak {
            preview.weak += 1;
        }
        let entry = workbook_counts
            .entry(question.workbook_id.clone())
            .or_insert_with(|| ScopeCount {
                id: question.workbook_id.clone(),
                title: question.workbook_title.clone(),
                total: 0,
                studied: 0,
                due: 0,
                weak: 0,
            });
        entry.total += 1;
        entry.studied += usize::from(card.is_some());
        entry.due += usize::from(due);
        entry.weak += usize::from(weak);
    }
    preview.unstudied = preview.total.saturating_sub(preview.studied);
    preview.workbooks = workbook_counts.into_values().collect();

    let mut topic_counts = Vec::new();
    for topic in &topics {
        let effective = expand_topic_ids(&topics, [&topic.id]);
        let mut count = ScopeCount {
            id: topic.id.clone(),
            title: workbook_domain::topic_path(&topics, &topic.id)
                .map(|path| path.join(" / "))
                .unwrap_or_else(|| topic.name.clone()),
            total: 0,
            studied: 0,
            due: 0,
            weak: 0,
        };
        for question in &resolved {
            if !question.topic_ids.iter().any(|id| effective.contains(id)) {
                continue;
            }
            let key = (question.workbook_id.clone(), question.question_id.clone());
            let card = context.states.get(&key);
            let stat = context.stats.get(&key);
            count.total += 1;
            count.studied += usize::from(card.is_some());
            count.due += usize::from(
                card.is_some_and(|card| question_is_due(card, context.buried.contains(&key), now)),
            );
            count.weak += usize::from(question_is_weak(card, stat));
        }
        if count.total > 0 {
            topic_counts.push(count);
        }
    }
    preview.topics = topic_counts;
    Ok(preview)
}

fn scope_due_candidates(
    resolved: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    now: DateTime<Utc>,
) -> Result<Vec<SessionCandidate>> {
    let mut rows = Vec::new();
    for (ordinal, question) in resolved.iter().enumerate() {
        let key = (question.workbook_id.clone(), question.question_id.clone());
        let Some(card) = context.states.get(&key) else {
            continue;
        };
        if card.due_at > now {
            continue;
        }
        if card.state == CardState::Review && context.buried.contains(&key) {
            continue;
        }
        rows.push((
            card.due_at,
            ordinal,
            SessionCandidate {
                workbook_id: question.workbook_id.clone(),
                note_id: question.note_id.clone(),
                question_id: question.question_id.clone(),
                due_at: Some(card.due_at),
            },
        ));
    }
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.question_id.cmp(&b.2.question_id))
    });
    let mut seen_notes = BTreeSet::new();
    let mut seen_questions = BTreeSet::new();
    let mut output = Vec::new();
    for (_, _, candidate) in rows {
        let note_key = (candidate.workbook_id.clone(), candidate.note_id.clone());
        let question_key = (candidate.workbook_id.clone(), candidate.question_id.clone());
        if seen_questions.contains(&question_key) || !seen_notes.insert(note_key) {
            continue;
        }
        seen_questions.insert(question_key);
        output.push(candidate);
    }
    Ok(output)
}

fn scope_learn_ahead_candidates(
    resolved: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    now: DateTime<Utc>,
) -> Result<Vec<SessionCandidate>> {
    let until = now + Duration::minutes(20);
    let mut rows = Vec::new();
    for (ordinal, question) in resolved.iter().enumerate() {
        let key = (question.workbook_id.clone(), question.question_id.clone());
        let Some(card) = context.states.get(&key) else {
            continue;
        };
        if !card.state.is_short_term() {
            continue;
        }
        if card.due_at <= now || card.due_at > until {
            continue;
        }
        if !context.calendar.learn_ahead_allowed(now, card.due_at) {
            continue;
        }
        rows.push((
            card.due_at,
            ordinal,
            SessionCandidate {
                workbook_id: question.workbook_id.clone(),
                note_id: question.note_id.clone(),
                question_id: question.question_id.clone(),
                due_at: Some(card.due_at),
            },
        ));
    }
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.question_id.cmp(&b.2.question_id))
    });
    let mut seen_notes = BTreeSet::new();
    let mut output = Vec::new();
    for (_, _, candidate) in rows {
        if seen_notes.insert((candidate.workbook_id.clone(), candidate.note_id.clone())) {
            output.push(candidate);
        }
    }
    Ok(output)
}

fn persist_started_bundle(
    storage: &mut Storage,
    mut bundle: SessionBundle,
    now: DateTime<Utc>,
) -> Result<()> {
    let first_item = bundle
        .items
        .first()
        .ok_or_else(|| ServiceError::Invalid("問題を選択できませんでした".to_string()))?
        .clone();
    let workbooks: BTreeSet<String> = bundle
        .items
        .iter()
        .map(|item| item.workbook_id.clone())
        .collect();
    bundle.session.current_workbook_id = Some(first_item.workbook_id.clone());
    bundle.session.current_item_id = Some(first_item.id.clone());
    bundle.session.cursor_kind = "initial".to_string();
    bundle.session.phase = if matches!(bundle.session.mode, Mode::New | Mode::Plan)
        && !storage.note_introduced(&first_item.workbook_id, &first_item.note_id)?
    {
        Phase::Intro
    } else {
        Phase::Question
    };
    bundle.session.question_started_at = Some(now);
    bundle.session.question_elapsed_ms = 0;
    bundle.session.revision = 0;
    storage.create_session(&bundle)?;
    for workbook in workbooks {
        storage.touch_workbook(&workbook, now)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ExamPlanContext {
    states: BTreeMap<(String, String), workbook_domain::StoredCard>,
    stats: BTreeMap<(String, String), workbook_storage::QuestionAttemptStat>,
    buried: BTreeSet<(String, String)>,
    calendar: StudyCalendar,
    scheduler: Scheduler,
    new_limit_remaining: usize,
}

fn exam_plan_context(storage: &Storage, now: DateTime<Utc>) -> Result<ExamPlanContext> {
    let settings = storage.settings()?;
    let calendar =
        StudyCalendar::new(&settings.effective_timezone()).map_err(ServiceError::Invalid)?;
    let scheduler = Scheduler::new(SchedulerSettings {
        desired_retention: settings.retention,
        ..SchedulerSettings::default()
    })?;
    let introduced = storage.new_questions_introduced_today(&calendar, now)?;
    let states = storage
        .review_states()?
        .into_iter()
        .map(|(workbook_id, question_id, card)| ((workbook_id, question_id), card))
        .collect();
    let stats = storage
        .question_attempt_stats()?
        .into_iter()
        .map(|(workbook_id, question_id, stat)| ((workbook_id, question_id), stat))
        .collect();
    Ok(ExamPlanContext {
        states,
        stats,
        buried: storage.buried_questions(now)?,
        calendar,
        scheduler,
        new_limit_remaining: (settings.new_limit as usize).saturating_sub(introduced),
    })
}

fn build_exam_plan(
    storage: &Storage,
    exam: &Exam,
    catalog: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    now: DateTime<Utc>,
) -> Result<ExamPlan> {
    let deadline = exam
        .target_date
        .map(|date| exam_deadline(&context.calendar, date));
    let today_start = context
        .calendar
        .day_start(context.calendar.learning_day(now));
    let studied_seconds_today =
        u32::try_from(storage.exam_studied_ms_since(&exam.id, today_start)?.max(0) / 1_000)
            .unwrap_or(u32::MAX);
    let questions = plan_questions_for(
        &resolve_scope_for(storage, &exam.scope, catalog)?,
        context,
        deadline,
        now,
    );
    Ok(plan_exam(PlanInput {
        now,
        calendar: &context.calendar,
        target_date: exam.target_date,
        daily_minutes: exam.daily_minutes,
        new_limit_remaining: context.new_limit_remaining,
        studied_seconds_today,
        questions: &questions,
    }))
}

fn plan_questions_for(
    questions: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    deadline: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Vec<PlanQuestion> {
    questions
        .iter()
        .map(|question| {
            let key = (question.workbook_id.clone(), question.question_id.clone());
            let card = context.states.get(&key);
            let stat = context.stats.get(&key);
            let attempts = stat.map(|stat| stat.attempts as u32).unwrap_or(0);
            let incorrect = stat.map(|stat| stat.incorrect as u32).unwrap_or(0);
            let elapsed_ms = stat.map(|stat| stat.elapsed_ms).unwrap_or(0);
            let estimated = estimated_seconds(&question.question_kind, attempts, elapsed_ms);
            let exam_recall = match (card, deadline) {
                (Some(card), Some(deadline)) => {
                    Some(context.scheduler.retrievability_at(card, now, deadline))
                }
                _ => None,
            };
            PlanQuestion {
                workbook_id: question.workbook_id.clone(),
                note_id: question.note_id.clone(),
                question_id: question.question_id.clone(),
                kind: question.question_kind.clone(),
                card: card.map(|card| PlanCard {
                    due_at: card.due_at,
                    last_review_at: card.last_review_at,
                    lapses: card.lapses,
                    difficulty: card.difficulty,
                    reps: card.reps,
                }),
                attempts,
                incorrect,
                estimated_seconds: estimated,
                exam_recall,
            }
        })
        .collect()
}

fn exam_summary_from(
    storage: &Storage,
    exam: Exam,
    catalog: &[ExamCatalogQuestion],
    context: &ExamPlanContext,
    now: DateTime<Utc>,
    mock_limit: usize,
) -> Result<ExamSummary> {
    let plan = build_exam_plan(storage, &exam, catalog, context, now)?;
    let coverage = plan.coverage.clone();
    let practice = storage.exam_practice_stats(&exam.id)?;
    let mock_history = storage
        .mock_results(&exam.id, mock_limit)?
        .into_iter()
        .map(|result| MockHistoryEntry {
            session_id: result.session_id,
            submitted_at: result.submitted_at,
            total: result.total,
            correct: result.correct,
            incorrect: result.incorrect,
            unanswered: result.unanswered,
            ungraded: result.ungraded,
            duration_ms: result.duration_ms,
            score_percent: mock_score_percent(result.correct, result.incorrect),
        })
        .collect();
    Ok(ExamSummary {
        total: coverage.total,
        studied: coverage.studied,
        unstudied: coverage.unstudied,
        due: coverage.due,
        weak: coverage.weak,
        days_remaining: plan.days_remaining.map(|days| days.max(0)),
        expired: plan.expired,
        coverage: if coverage.total == 0 {
            None
        } else {
            Some(coverage.studied as f64 / coverage.total as f64)
        },
        plan,
        practice,
        mock_history,
        exam,
    })
}

fn build_mock_result_view(
    storage: &Storage,
    bundle: &SessionBundle,
    result: &MockResultRow,
) -> Result<MockResultView> {
    let exam_title = storage
        .exam(&result.exam_id)
        .map(|exam| exam.title)
        .unwrap_or_else(|_| "試験".to_string());
    let answers: BTreeMap<String, MockAnswerRow> = storage
        .mock_answers_for_session(&result.session_id)?
        .into_iter()
        .map(|answer| (answer.item_id.clone(), answer))
        .collect();
    let mut workbook_cache: BTreeMap<String, Workbook> = BTreeMap::new();
    let mut items = Vec::new();
    for item in &bundle.items {
        let workbook = match workbook_cache.get(&item.workbook_id) {
            Some(workbook) => workbook.clone(),
            None => {
                let workbook = storage.workbook(&item.workbook_id)?;
                workbook_cache.insert(item.workbook_id.clone(), workbook.clone());
                workbook
            }
        };
        let note_title = workbook
            .notes
            .iter()
            .find(|note| note.id == item.note_id)
            .map(|note| note.title.clone())
            .unwrap_or_default();
        let question = workbook
            .find_question(&item.note_id, &item.question_id)
            .map(|(_, question)| question.clone());
        let answer = answers.get(&item.id);
        let status = match answer.and_then(|answer| answer.correct) {
            Some(true) => MockItemStatus::Correct,
            Some(false) => MockItemStatus::Incorrect,
            None if answer.is_some() => MockItemStatus::Ungraded,
            None => MockItemStatus::Unanswered,
        };
        let given = answer
            .and_then(|answer| serde_json::from_str::<Value>(&answer.answer_json).ok())
            .filter(|value| !value.is_null());
        items.push(MockItemResult {
            ordinal: item.ordinal,
            workbook_title: workbook.title.clone(),
            note_title,
            prompt: question
                .as_ref()
                .map(Question::prompt_text)
                .unwrap_or_default(),
            status,
            given_answer: question
                .as_ref()
                .map(|question| format_given_answer(question, given.as_ref()))
                .unwrap_or_else(|| "（未回答）".to_string()),
            answer_summary: question
                .as_ref()
                .map(Question::answer_summary)
                .unwrap_or_default(),
            explanation: question
                .as_ref()
                .map(|question| question.explanation.clone())
                .unwrap_or_else(|| Content::Text(String::new())),
        });
    }
    Ok(MockResultView {
        session_id: result.session_id.clone(),
        exam_id: result.exam_id.clone(),
        exam_title,
        submitted_at: result.submitted_at,
        total: result.total,
        correct: result.correct,
        incorrect: result.incorrect,
        unanswered: result.unanswered,
        ungraded: result.ungraded,
        duration_ms: result.duration_ms,
        score_percent: mock_score_percent(result.correct, result.incorrect),
        items,
    })
}

fn mock_deadline(session: &SessionRow) -> Option<DateTime<Utc>> {
    if session.mode != Mode::Mock {
        return None;
    }
    session
        .time_limit_seconds
        .map(|seconds| session.started_at + Duration::seconds(i64::from(seconds)))
}

fn ensure_no_active_session(storage: &Storage) -> Result<()> {
    if storage.active_session()?.is_some() {
        return Err(ServiceError::Conflict(
            "中断中のセッションを再開するか、終了してください".to_string(),
        ));
    }
    Ok(())
}

fn seed_from(now: DateTime<Utc>, exam_id: &str, mode: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in now
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .to_le_bytes()
        .into_iter()
        .chain(exam_id.bytes())
        .chain(mode.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn shuffle_candidates(candidates: &mut [SessionCandidate], seed: u64) {
    if candidates.len() < 2 {
        return;
    }
    let mut state = seed | 1;
    for index in (1..candidates.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let swap = (state % (index as u64 + 1)) as usize;
        candidates.swap(index, swap);
    }
}

fn plan_question_revisions(stored: &Workbook, next: &Workbook) -> Vec<QuestionRevisionPlan> {
    // Match by question ID alone: a question may have moved to another note,
    // and that must not hide a change to its answer or prompt.
    let previous: BTreeMap<&str, &Question> = stored
        .all_questions()
        .map(|(_, question)| (question.id.as_str(), question))
        .collect();
    let mut plans = Vec::new();
    for note in &next.notes {
        for question in &note.questions {
            let Some(old) = previous.get(question.id.as_str()) else {
                continue;
            };
            if classify_question_change(old, question) != QuestionChange::StudyContent {
                continue;
            }
            let prompt = question.prompt_text();
            let prompt = if prompt.chars().count() > 80 {
                format!("{}…", prompt.chars().take(80).collect::<String>())
            } else {
                prompt
            };
            plans.push(QuestionRevisionPlan {
                note_id: note.id.clone(),
                question_id: question.id.clone(),
                prompt,
                previous_revision: old.revision,
                next_revision: resolved_revision(old, question, QuestionChange::StudyContent),
            });
        }
    }
    plans
}

/// Applies the planned revisions and keeps display-only edits on the existing
/// revision. Questions that moved notes are matched by ID, not by note.
fn apply_question_revisions(
    stored: &Workbook,
    next: &mut Workbook,
    plans: &[QuestionRevisionPlan],
) {
    let mut previous: BTreeMap<String, i64> = BTreeMap::new();
    for (_, question) in stored.all_questions() {
        previous.insert(question.id.clone(), question.revision);
    }
    let planned: BTreeMap<&str, i64> = plans
        .iter()
        .map(|plan| (plan.question_id.as_str(), plan.next_revision))
        .collect();
    for note in &mut next.notes {
        for question in &mut note.questions {
            let Some(old_revision) = previous.get(&question.id).copied() else {
                continue;
            };
            question.revision = planned
                .get(question.id.as_str())
                .copied()
                .unwrap_or(old_revision);
        }
    }
}

fn hydrate_workbook_topics(
    storage: &Storage,
    local_id: &str,
    workbook: &mut Workbook,
) -> Result<()> {
    let topics = storage.topics()?;
    let mut assignments: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (workbook_id, question_id, topic_id) in storage.question_topics()? {
        if workbook_id != local_id {
            continue;
        }
        if let Some(path) = workbook_domain::topic_path(&topics, &topic_id) {
            assignments
                .entry((workbook_id, question_id))
                .or_default()
                .push(path.join("/"));
        }
    }
    for note in &mut workbook.notes {
        for question in &mut note.questions {
            question.topics = assignments
                .get(&(local_id.to_string(), question.id.clone()))
                .cloned()
                .unwrap_or_default();
            question.topics.sort();
            question.topics.dedup();
        }
    }
    workbook.topic_catalog = topic_catalog_entries(storage, &topics, local_id)?;
    Ok(())
}

/// Exports the stable topic IDs used by this workbook, including ancestors.
/// A re-import after a topic rename resolves these IDs instead of creating a
/// second topic from the old display name.
fn topic_catalog_entries(
    storage: &Storage,
    topics: &[workbook_domain::Topic],
    local_id: &str,
) -> Result<Vec<TopicCatalogEntry>> {
    let by_id: BTreeMap<&str, &workbook_domain::Topic> = topics
        .iter()
        .map(|topic| (topic.id.as_str(), topic))
        .collect();
    let mut used = BTreeSet::new();
    for (workbook_id, _question_id, topic_id) in storage.question_topics()? {
        if workbook_id != local_id {
            continue;
        }
        let mut current = Some(topic_id.as_str());
        while let Some(id) = current {
            if !used.insert(id.to_string()) {
                break;
            }
            current = by_id.get(id).and_then(|topic| topic.parent_id.as_deref());
        }
    }
    let mut entries = used
        .into_iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|topic| (*topic).clone()))
        .map(|topic| TopicCatalogEntry {
            id: topic.id,
            name: topic.name,
            parent_id: topic.parent_id,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        workbook_domain::topic_path(topics, &a.id)
            .unwrap_or_default()
            .cmp(&workbook_domain::topic_path(topics, &b.id).unwrap_or_default())
            .then(a.id.cmp(&b.id))
    });
    Ok(entries)
}

fn resolve_assets_for_save(
    storage: &Storage,
    local_id: Option<&str>,
    workbook: &Workbook,
) -> Result<Vec<AssetContent>> {
    let mut output = Vec::new();
    let mut total_bytes = 0usize;
    for asset in &workbook.assets {
        let (bytes, width, height) = if let Some(bytes) = asset.embedded_bytes()? {
            let (width, height) =
                inspect_image(&bytes, &asset.media_type, &format!("assets.{}", asset.id))?;
            (bytes, width, height)
        } else if let Some(local_id) = local_id {
            let stored = storage.asset(local_id, &asset.id)?.ok_or_else(|| {
                ServiceError::Invalid(format!(
                    "assets.{}: 既存の画像を読み込めません。画像を再指定してください",
                    asset.id
                ))
            })?;
            if stored.media_type != asset.media_type {
                return Err(ServiceError::Invalid(format!(
                    "assets.{}.mediaType: 既存画像の形式と一致しません",
                    asset.id
                )));
            }
            (stored.bytes, stored.width, stored.height)
        } else {
            return Err(ServiceError::Invalid(format!(
                "assets.{}: file 参照の画像は教材ファイルからインポートしてください",
                asset.id
            )));
        };
        total_bytes += bytes.len();
        if total_bytes > MAX_TOTAL_ASSET_BYTES {
            return Err(ServiceError::Invalid(
                "画像アセットの合計サイズが上限を超えています".to_string(),
            ));
        }
        output.push(AssetContent {
            id: asset.id.clone(),
            alt: asset.alt.clone(),
            media_type: asset.media_type.clone(),
            width,
            height,
            bytes,
        });
    }
    Ok(output)
}

struct BudgetEstimates {
    kinds: BTreeMap<(String, String), String>,
    stats: BTreeMap<(String, String), (u32, i64)>,
}

impl BudgetEstimates {
    fn load(storage: &Storage) -> Result<Self> {
        let mut kinds = BTreeMap::new();
        for question in exam_catalog(storage)? {
            kinds.insert(
                (question.workbook_id, question.question_id),
                question.question_kind,
            );
        }
        let mut stats = BTreeMap::new();
        for (workbook_id, question_id, stat) in storage.question_attempt_stats()? {
            stats.insert(
                (workbook_id, question_id),
                (stat.attempts as u32, stat.elapsed_ms),
            );
        }
        Ok(Self { kinds, stats })
    }

    fn estimate(&self, workbook_id: &str, question_id: &str) -> f64 {
        let kind = self
            .kinds
            .get(&(workbook_id.to_string(), question_id.to_string()))
            .map(String::as_str)
            .unwrap_or("single_choice");
        let (attempts, elapsed_ms) = self
            .stats
            .get(&(workbook_id.to_string(), question_id.to_string()))
            .copied()
            .unwrap_or((0, 0));
        estimated_seconds(kind, attempts, elapsed_ms)
    }
}

fn budget_category(pool: BudgetPool) -> BudgetCategory {
    match pool {
        BudgetPool::ShortTermDue => BudgetCategory::ShortTermDue,
        BudgetPool::DueReview => BudgetCategory::DueReview,
        BudgetPool::New => BudgetCategory::New,
        BudgetPool::LearnAhead => BudgetCategory::LearnAhead,
    }
}

fn budget_question(
    estimates: &BudgetEstimates,
    workbook_id: String,
    note_id: String,
    question_id: String,
    pool: BudgetPool,
    due_at: Option<DateTime<Utc>>,
    ordinal: u32,
) -> BudgetQuestion {
    let estimated_seconds = estimates.estimate(&workbook_id, &question_id);
    let kind = estimates
        .kinds
        .get(&(workbook_id.clone(), question_id.clone()))
        .cloned()
        .unwrap_or_else(|| "single_choice".to_string());
    BudgetQuestion {
        workbook_id,
        note_id,
        question_id,
        kind,
        category: budget_category(pool),
        due_at,
        ordinal,
        estimated_seconds,
    }
}

fn session_candidate(question: BudgetQuestion) -> SessionCandidate {
    SessionCandidate {
        workbook_id: question.workbook_id,
        note_id: question.note_id,
        question_id: question.question_id,
        due_at: question.due_at,
    }
}

fn build_session_bundle(
    mode: Mode,
    candidates: &[SessionCandidate],
    now: DateTime<Utc>,
    exam_id: Option<String>,
    time_limit_seconds: Option<u32>,
    time_budget_seconds: Option<u32>,
) -> Result<(SessionBundle, String)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let started_at = now;
    let mut items = Vec::new();
    let mut workbooks: Vec<SessionWorkbookRow> = Vec::new();
    let mut seen_workbooks = std::collections::BTreeSet::new();
    for (ordinal, candidate) in candidates.iter().enumerate() {
        let item = SessionItemRow {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            ordinal: ordinal as u32,
            workbook_id: candidate.workbook_id.clone(),
            note_id: candidate.note_id.clone(),
            question_id: candidate.question_id.clone(),
            status: SessionItemStatus::Pending,
        };
        if seen_workbooks.insert(candidate.workbook_id.clone()) {
            workbooks.push(SessionWorkbookRow {
                session_id: session_id.clone(),
                workbook_id: candidate.workbook_id.clone(),
                ordinal: workbooks.len() as u32,
                result_ack: false,
            });
        }
        items.push(item);
    }
    let first_note = items
        .first()
        .map(|item| item.note_id.clone())
        .ok_or_else(|| ServiceError::Invalid("対象の問題がありません".to_string()))?;
    let session = SessionRow {
        id: session_id,
        mode,
        status: SessionStatus::Active,
        started_at,
        ended_at: None,
        current_workbook_id: items.first().map(|item| item.workbook_id.clone()),
        current_item_id: items.first().map(|item| item.id.clone()),
        phase: Phase::Question,
        draft_json: None,
        hint: false,
        revealed: false,
        pending_correct: None,
        pending_unknown: false,
        pending_at: None,
        question_started_at: Some(now),
        question_elapsed_ms: 0,
        cursor_kind: "initial".to_string(),
        assist_json: None,
        revision: 0,
        exam_id,
        goal_id: None,
        time_limit_seconds,
        time_budget_seconds,
    };
    Ok((
        SessionBundle {
            session,
            items,
            workbooks,
            attempts: Vec::new(),
        },
        first_note,
    ))
}

fn ensure_active(bundle: &SessionBundle) -> Result<()> {
    if bundle.session.status != SessionStatus::Active {
        return Err(ServiceError::Invalid("終了したセッションです".to_string()));
    }
    Ok(())
}

/// The domain policy is pure; the service only adds the question-type rule.
/// Subjective flashcards fall back to the manual rating flow unless the
/// learner explicitly says "わからない", which is always `Again`.
fn automatic_rating_for(
    question: &Question,
    correct: Option<bool>,
    hint: bool,
    unknown: bool,
) -> Option<Rating> {
    if question.kind == QuestionType::Flashcard && !unknown {
        None
    } else {
        automatic_rating(correct, hint, unknown)
    }
}

fn check_revision(session: &SessionRow, expected: u64) -> Result<()> {
    if session.revision != expected {
        return Err(ServiceError::Conflict(
            "画面の状態が更新されています。再読み込みしてください".to_string(),
        ));
    }
    Ok(())
}

fn current_item(bundle: &SessionBundle) -> Result<&SessionItemRow> {
    let id = bundle
        .session
        .current_item_id
        .as_ref()
        .ok_or_else(|| ServiceError::Invalid("現在の問題がありません".to_string()))?;
    bundle
        .items
        .iter()
        .find(|item| &item.id == id)
        .ok_or_else(|| ServiceError::NotFound("セッション項目".to_string()))
}

fn record_time(session: &mut SessionRow, now: DateTime<Utc>) {
    if matches!(session.phase, Phase::Intro | Phase::Question) {
        if let Some(started) = session.question_started_at {
            let elapsed = (now - started).num_milliseconds().max(0);
            session.question_elapsed_ms += elapsed;
        }
        session.question_started_at = Some(now);
    }
}

fn progress_for_exam(bundle: &SessionBundle, item: Option<&SessionItemRow>) -> StudyProgress {
    let answered = bundle
        .items
        .iter()
        .filter(|item| item.status == SessionItemStatus::Answered)
        .count();
    StudyProgress {
        position: item
            .map(|item| item.ordinal as usize + 1)
            .unwrap_or(answered),
        total: bundle.items.len(),
        answered,
        skipped: bundle
            .items
            .iter()
            .filter(|item| item.status == SessionItemStatus::Skipped)
            .count(),
    }
}

fn progress_for(
    bundle: &SessionBundle,
    workbook_id: &str,
    item: Option<&SessionItemRow>,
) -> StudyProgress {
    let items: Vec<_> = bundle
        .items
        .iter()
        .filter(|item| item.workbook_id == workbook_id)
        .collect();
    let answered = items
        .iter()
        .filter(|item| item.status == SessionItemStatus::Answered)
        .count();
    StudyProgress {
        position: item
            .filter(|item| item.workbook_id == workbook_id)
            .map(|item| item.ordinal as usize + 1)
            .unwrap_or(answered),
        total: items.len(),
        answered,
        skipped: items
            .iter()
            .filter(|item| item.status == SessionItemStatus::Skipped)
            .count(),
    }
}

fn stats_for(storage: &Storage, bundle: &SessionBundle, workbook_id: Option<&str>) -> ResultStats {
    let _ = storage;
    let counts = counts_for_workbook(bundle, workbook_id);
    stats_from_counts(counts, bundle, workbook_id)
}

fn stats_from_counts(
    counts: SessionCounts,
    bundle: &SessionBundle,
    workbook_id: Option<&str>,
) -> ResultStats {
    let elapsed_ms = bundle
        .attempts
        .iter()
        .filter(|attempt| workbook_id.is_none_or(|id| attempt.workbook_id == id))
        .map(|attempt| attempt.elapsed_ms)
        .sum();
    ResultStats {
        questions: counts.initial_questions,
        answered: counts.distinct_answered,
        attempts: counts.attempts,
        short_term: counts.short_term_answers,
        skipped: counts.skipped,
        first_pass_graded: counts.first_pass_graded,
        first_pass_correct: counts.first_pass_correct,
        elapsed_ms,
    }
}

fn format_given_answer(question: &Question, answer: Option<&Value>) -> String {
    let Some(answer) = answer else {
        return "（未回答）".to_string();
    };
    if answer.is_null() {
        return "（未回答）".to_string();
    }
    match question.kind {
        workbook_domain::QuestionType::SingleChoice => {
            let Some(id) = answer.as_str() else {
                return answer.to_string();
            };
            question
                .options
                .as_ref()
                .and_then(|options| {
                    options
                        .iter()
                        .find(|option| option.id == id)
                        .map(|option| option.content.plain_text())
                })
                .unwrap_or_else(|| id.to_string())
        }
        workbook_domain::QuestionType::MultipleChoice => {
            let Some(ids) = answer.as_array() else {
                return answer.to_string();
            };
            ids.iter()
                .filter_map(Value::as_str)
                .map(|id| {
                    question
                        .options
                        .as_ref()
                        .and_then(|options| {
                            options
                                .iter()
                                .find(|option| option.id == id)
                                .map(|option| option.content.plain_text())
                        })
                        .unwrap_or_else(|| id.to_string())
                })
                .collect::<Vec<_>>()
                .join(" / ")
        }
        workbook_domain::QuestionType::TrueFalse => match answer.as_bool() {
            Some(true) => "○ 正しい".to_string(),
            Some(false) => "× まちがい".to_string(),
            None => answer.to_string(),
        },
        workbook_domain::QuestionType::ShortText => answer
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| answer.to_string()),
        workbook_domain::QuestionType::Cloze => answer
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(key, value)| format!("{key}: {}", value.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_else(|| answer.to_string()),
        workbook_domain::QuestionType::Flashcard => {
            if answer.as_bool() == Some(true) {
                "思い出せた".to_string()
            } else if answer.as_bool() == Some(false) {
                "思い出せなかった".to_string()
            } else {
                "（自己評価）".to_string()
            }
        }
        workbook_domain::QuestionType::Numeric => answer
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| answer.to_string()),
        workbook_domain::QuestionType::Ordering => answer
            .get("order")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| {
                        question
                            .options
                            .as_ref()
                            .and_then(|options| {
                                options
                                    .iter()
                                    .find(|option| option.id == id)
                                    .map(|option| option.content.plain_text())
                            })
                            .unwrap_or_else(|| id.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(" → ")
            })
            .unwrap_or_else(|| answer.to_string()),
        workbook_domain::QuestionType::Matching => answer
            .get("matches")
            .and_then(Value::as_object)
            .map(|matches| {
                matches
                    .iter()
                    .map(|(left, right)| {
                        let left_text = question
                            .matching
                            .as_ref()
                            .and_then(|matching| {
                                matching
                                    .left
                                    .iter()
                                    .find(|item| &item.id == left)
                                    .map(|item| item.content.plain_text())
                            })
                            .unwrap_or_else(|| left.clone());
                        let right_id = right.as_str().unwrap_or_default();
                        let right_text = question
                            .matching
                            .as_ref()
                            .and_then(|matching| {
                                matching
                                    .right
                                    .iter()
                                    .find(|item| item.id == right_id)
                                    .map(|item| item.content.plain_text())
                            })
                            .unwrap_or_else(|| right_id.to_string());
                        format!("{left_text} → {right_text}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| answer.to_string()),
        workbook_domain::QuestionType::Hotspot => {
            let x = answer.get("x").and_then(Value::as_f64);
            let y = answer.get("y").and_then(Value::as_f64);
            match (x, y) {
                (Some(x), Some(y)) => {
                    let region = question.hotspot.as_ref().and_then(|hotspot| {
                        hotspot
                            .regions
                            .iter()
                            .find(|region| region.shape.contains(x, y))
                            .map(|region| region.alt.clone())
                    });
                    match region {
                        Some(alt) => alt,
                        None => format!("画像内 {:.0}% / {:.0}%", x * 100.0, y * 100.0),
                    }
                }
                _ => "（未選択）".to_string(),
            }
        }
        workbook_domain::QuestionType::Labeling => answer
            .get("labels")
            .and_then(Value::as_object)
            .map(|labels| {
                labels
                    .iter()
                    .map(|(target, label)| {
                        let target_text = question
                            .labeling
                            .as_ref()
                            .and_then(|labeling| {
                                labeling
                                    .targets
                                    .iter()
                                    .find(|item| &item.id == target)
                                    .map(|item| item.alt.clone())
                            })
                            .unwrap_or_else(|| target.clone());
                        let label_id = label.as_str().unwrap_or_default();
                        let label_text = question
                            .labeling
                            .as_ref()
                            .and_then(|labeling| {
                                labeling
                                    .labels
                                    .iter()
                                    .find(|item| item.id == label_id)
                                    .map(|item| item.content.plain_text())
                            })
                            .unwrap_or_else(|| label_id.to_string());
                        format!("{target_text}: {label_text}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| answer.to_string()),
    }
}

pub fn theme_from_settings(settings: &Settings) -> ThemeSetting {
    settings.theme
}

pub fn is_short_term(state: CardState) -> bool {
    state.is_short_term()
}

#[cfg(test)]
mod tests {
    use super::asset_fingerprint;

    #[test]
    fn asset_fingerprint_changes_when_the_content_changes() {
        let first = asset_fingerprint("image/svg+xml", 800, 300, b"<svg/>");
        let same = asset_fingerprint("image/svg+xml", 800, 300, b"<svg/>");
        let changed = asset_fingerprint("image/svg+xml", 800, 300, b"<svg>2</svg>");
        let resized = asset_fingerprint("image/svg+xml", 801, 300, b"<svg/>");
        assert_eq!(first, same);
        assert_ne!(first, changed);
        assert_ne!(first, resized);
    }
}
