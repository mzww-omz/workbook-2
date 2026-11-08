use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use thiserror::Error;
use workbook_domain::{
    Answer, AssistState, CardState, Content, GradeResult, GradeStatus, Mode, NearMissReason,
    NearMissRecord, Phase, Question, QuestionType, Rating, RatingSource, SessionItemStatus,
    SessionStatus, Settings, StudyCalendar, ThemeSetting, Workbook, answer_is_blank,
    automatic_rating, grade_answer_detailed, parse_workbook,
};
use workbook_import::{LegacyPlan, LegacyPreview};
use workbook_scheduler::{Clock, QueueInput, QueueKind, Scheduler, SchedulerSettings, select_next};
use workbook_storage::{
    AttemptRow, CommitWrite, CorrectionWrite, SessionBundle, SessionCandidate, SessionCounts,
    SessionItemRow, SessionRow, SessionWorkbookRow, Storage, StorageError, counts_for_workbook,
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
    pub bytes: Arc<Vec<u8>>,
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
        Ok(self.storage()?.workbook(local_id)?)
    }

    pub fn preview_workbook(&self, raw: &str) -> Result<Workbook> {
        Ok(parse_workbook(raw)?)
    }

    pub fn preview_workbook_file(&self, path: &std::path::Path) -> Result<Workbook> {
        let (workbook, _assets) = workbook_import::load_workbook_file(path)?;
        Ok(workbook)
    }

    pub fn import_workbook_json(&self, raw: &str) -> Result<String> {
        let workbook = parse_workbook(raw)?;
        let assets = workbook.embedded_assets()?;
        let mut storage = self.storage()?;
        let now = self.clock.now();
        Ok(storage.import_workbook(&workbook, &assets, now)?)
    }

    pub fn import_workbook_file(&self, path: &std::path::Path) -> Result<String> {
        let (workbook, assets) = workbook_import::load_workbook_file(path)?;
        let mut storage = self.storage()?;
        let now = self.clock.now();
        Ok(storage.import_workbook(&workbook, &assets, now)?)
    }

    pub fn delete_workbook(&self, local_id: &str) -> Result<()> {
        let mut storage = self.storage()?;
        let backup = storage.data_dir().join("backups").join(format!(
            "pre-delete-{}.sqlite",
            Utc::now().timestamp_millis()
        ));
        storage.backup_to(&backup)?;
        storage.delete_workbook(local_id)?;
        Ok(())
    }

    // ---- study session --------------------------------------------------

    pub fn start_session(&self, mode: Mode, workbook_id: Option<&str>) -> Result<()> {
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
        let candidates = match mode {
            Mode::New => {
                let workbook_id = workbook_id
                    .ok_or_else(|| ServiceError::Invalid("Workbookを選んでください".to_string()))?;
                let introduced = storage.new_questions_introduced_today(&calendar, now)?;
                let remaining = (settings.new_limit as usize).saturating_sub(introduced);
                if remaining == 0 {
                    return Err(ServiceError::Invalid(
                        "本日の新規問題の上限に達しました".to_string(),
                    ));
                }
                storage.new_candidates(workbook_id, remaining)?
            }
            Mode::Review => {
                let due = storage.review_candidates(workbook_id, now)?;
                if due.is_empty() {
                    storage.upcoming_short_term(workbook_id, now, now + Duration::minutes(20))?
                } else {
                    due
                }
            }
            Mode::Practice => {
                let workbook_id = workbook_id
                    .ok_or_else(|| ServiceError::Invalid("Workbookを選んでください".to_string()))?;
                storage.practice_candidates(workbook_id)?
            }
        };
        if candidates.is_empty() {
            return Err(ServiceError::Invalid(
                "対象の問題はありません（関連問題は翌学習日まで延期される場合があります）"
                    .to_string(),
            ));
        }
        let (bundle, first_note) = build_session_bundle(mode, &candidates, now)?;
        let first_item = bundle
            .items
            .first()
            .ok_or_else(|| ServiceError::Invalid("問題を選択できませんでした".to_string()))?
            .clone();
        let mut session = bundle.session;
        session.current_workbook_id = Some(first_item.workbook_id.clone());
        session.current_item_id = Some(first_item.id.clone());
        session.cursor_kind = "initial".to_string();
        session.phase = if mode == Mode::New
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
        // Refresh at the next 4 a.m. boundary even when no card is due, so
        // Today's learning-day counts and date caption switch over.
        let day_start = Some(calendar.next_learning_day_start(now));
        Ok(match (next_due, day_start) {
            (Some(due), Some(start)) => Some(due.min(start)),
            (due, start) => due.or(start),
        })
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
        let card = if bundle.session.mode == Mode::Practice {
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
            answer_json,
            before_state_json: before_json,
            after_state_json: after_json,
            state_revision: card.as_ref().map(|card| card.revision).unwrap_or(0),
            short_term,
            assist_json,
            created_at: now,
        };
        let bury_until = if bundle.session.mode == Mode::Review {
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
            introduce_note: bundle.session.mode != Mode::Practice,
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
        let card = if bundle.session.mode == Mode::Practice {
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
        let queue = storage.session_queue(&bundle.session.id, &workbook_id)?;
        let choice = select_next(QueueInput {
            mode: bundle.session.mode,
            workbook_id: &workbook_id,
            questions: &queue,
            now,
            calendar,
        });
        match choice {
            Some(choice) => {
                let item = bundle
                    .items
                    .iter()
                    .find(|item| item.id == choice.item_id)
                    .cloned()
                    .ok_or_else(|| ServiceError::NotFound("セッション項目".to_string()))?;
                let intro = bundle.session.mode == Mode::New
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
            let intro = bundle.session.mode == Mode::New
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
        _now: DateTime<Utc>,
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
                    assets.push(AssetView {
                        id: asset.id,
                        media_type: asset.media_type,
                        alt: asset.alt,
                        width: asset.width,
                        height: asset.height,
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
        if let Some(workbook_id) = workbook_id.as_deref() {
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
        let backup = storage.data_dir().join("backups").join(format!(
            "pre-import-{}.sqlite",
            Utc::now().timestamp_millis()
        ));
        storage.backup_to(&backup)?;
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
        let backup = storage.data_dir().join("backups").join(format!(
            "pre-reset-{}.sqlite",
            Utc::now().timestamp_millis()
        ));
        storage.backup_to(&backup)?;
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

fn build_session_bundle(
    mode: Mode,
    candidates: &[SessionCandidate],
    now: DateTime<Utc>,
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
