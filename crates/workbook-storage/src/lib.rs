use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;
use workbook_domain::{
    AssetContent, AssistState, CardState, Exam, ExamScope, Mode, Phase, RatingSource,
    SessionItemStatus, SessionStatus, Settings, StoredCard, StudyCalendar, ThemeSetting, Topic,
    Workbook, normalize_topic_name, parse_topic_path, parse_workbook,
};
use workbook_import::{
    LegacyAttempt, LegacyPlan, LegacySession, LegacySessionItem, LegacySessionWorkbook,
};

pub mod schema;

pub const APP_ID: i64 = schema::APPLICATION_ID;
pub const SCHEMA_VERSION: i64 = schema::SCHEMA_VERSION;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("ファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLiteエラー: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSONエラー: {0}")]
    Json(#[from] serde_json::Error),
    #[error("データが不正です: {0}")]
    Validation(String),
    #[error("見つかりません: {0}")]
    NotFound(String),
    #[error("競合しました: {0}")]
    Conflict(String),
    #[error("対応していないデータです: {0}")]
    Unsupported(String),
}

impl From<workbook_domain::ValidationError> for StorageError {
    fn from(value: workbook_domain::ValidationError) -> Self {
        Self::Validation(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Clone, Debug)]
pub struct WorkbookRecord {
    pub local_id: String,
    pub source_id: String,
    pub source_revision: i64,
    pub title: String,
    pub content_json: String,
    pub imported_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct AssetData {
    pub id: String,
    pub media_type: String,
    pub alt: String,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SessionCandidate {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub due_at: Option<DateTime<Utc>>,
}

/// Pool a time-budget candidate came from. The service maps these to the
/// scheduler's priority order; storage does not rank them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetPool {
    ShortTermDue,
    DueReview,
    New,
    LearnAhead,
}

#[derive(Clone, Debug)]
pub struct BudgetCandidate {
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub due_at: Option<DateTime<Utc>>,
    pub pool: BudgetPool,
}

#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: String,
    pub mode: Mode,
    pub status: SessionStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub current_workbook_id: Option<String>,
    pub current_item_id: Option<String>,
    pub phase: Phase,
    pub draft_json: Option<String>,
    pub hint: bool,
    pub revealed: bool,
    pub pending_correct: Option<bool>,
    pub pending_unknown: bool,
    pub pending_at: Option<DateTime<Utc>>,
    pub question_started_at: Option<DateTime<Utc>>,
    pub question_elapsed_ms: i64,
    pub cursor_kind: String,
    pub assist_json: Option<String>,
    pub revision: u64,
    pub exam_id: Option<String>,
    pub time_limit_seconds: Option<u32>,
    /// Selection target for time-budgeted sessions. `None` means no target.
    /// It is never a hard deadline.
    pub time_budget_seconds: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct MockAnswerRow {
    pub id: String,
    pub session_id: String,
    pub item_id: String,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub answered_at: DateTime<Utc>,
    pub correct: Option<bool>,
    pub answer_json: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct MockResultRow {
    pub session_id: String,
    pub exam_id: String,
    pub submitted_at: DateTime<Utc>,
    pub total: u32,
    pub correct: u32,
    pub incorrect: u32,
    pub unanswered: u32,
    pub ungraded: u32,
    pub duration_ms: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExamPracticeStats {
    pub attempts: usize,
    pub graded: usize,
    pub correct: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuestionAttemptStat {
    pub attempts: usize,
    pub incorrect: usize,
    /// Total recorded answer time across all attempts.
    pub elapsed_ms: i64,
}

#[derive(Clone, Debug)]
pub struct SessionItemRow {
    pub id: String,
    pub session_id: String,
    pub ordinal: u32,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub status: SessionItemStatus,
}

#[derive(Clone, Debug)]
pub struct SessionWorkbookRow {
    pub session_id: String,
    pub workbook_id: String,
    pub ordinal: u32,
    pub result_ack: bool,
}

#[derive(Clone, Debug)]
pub struct AttemptRow {
    pub id: String,
    pub session_id: String,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub answered_at: DateTime<Utc>,
    pub correct: Option<bool>,
    pub unknown: bool,
    pub hint: bool,
    pub elapsed_ms: i64,
    pub rating: u8,
    pub rating_source: RatingSource,
    pub answer_json: String,
    pub before_state_json: Option<String>,
    pub after_state_json: Option<String>,
    pub state_revision: u64,
    pub short_term: bool,
    pub assist_json: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct SessionBundle {
    pub session: SessionRow,
    pub items: Vec<SessionItemRow>,
    pub workbooks: Vec<SessionWorkbookRow>,
    pub attempts: Vec<AttemptRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionCounts {
    pub initial_questions: usize,
    pub distinct_answered: usize,
    pub attempts: usize,
    pub short_term_answers: usize,
    pub skipped: usize,
    pub first_pass_graded: usize,
    pub first_pass_correct: usize,
}

pub struct CommitWrite {
    pub attempt: AttemptRow,
    pub session: SessionRow,
    pub card: Option<StoredCard>,
    pub item_id: Option<String>,
    pub mark_answered: bool,
    pub introduce_note: bool,
    pub bury_until: Option<DateTime<Utc>>,
}

pub struct CorrectionWrite {
    pub attempt: AttemptRow,
    pub session: SessionRow,
    pub card: Option<StoredCard>,
}

pub struct Storage {
    conn: Option<Connection>,
    path: Option<PathBuf>,
    data_dir: PathBuf,
}

impl Storage {
    pub fn open(directory: &Path) -> Result<Self> {
        ensure_private_dir(directory)?;
        let backups = directory.join("backups");
        ensure_private_dir(&backups)?;
        let path = directory.join("workbook.sqlite");
        let existed = path.exists();
        let mut connection = open_connection(&path, true)?;
        {
            let application_id = pragma_i64(&connection, "application_id")?;
            let version = pragma_i64(&connection, "user_version")?;
            let tables = table_names(&connection)?;
            if application_id != 0 && application_id != APP_ID {
                return Err(StorageError::Unsupported(format!(
                    "Workbook 2のデータベースではありません (application_id={application_id})"
                )));
            }
            if version > SCHEMA_VERSION {
                return Err(StorageError::Unsupported(format!(
                    "このデータは新しいWorkbook 2で作成されています (user_version={version})"
                )));
            }
            if version == 0 {
                if !tables.is_empty() {
                    return Err(StorageError::Unsupported(
                        "既存の別データベースをWorkbook 2として開くことはできません".to_string(),
                    ));
                }
                connection.execute_batch(schema::SCHEMA_SQL)?;
                connection.execute_batch(&format!(
                    "PRAGMA application_id={APP_ID}; PRAGMA user_version={SCHEMA_VERSION};"
                ))?;
                connection.execute(
                    "INSERT OR REPLACE INTO meta(key,value) VALUES('created_at',?1)",
                    [format_ts(Utc::now())],
                )?;
                connection.execute(
                    "INSERT OR REPLACE INTO meta(key,value) VALUES('app','workbook-2')",
                    [],
                )?;
            } else {
                if application_id != APP_ID {
                    return Err(StorageError::Unsupported(
                        "Workbook 2のapplication_idがありません".to_string(),
                    ));
                }
                let expected: BTreeSet<&str> =
                    schema::expected_tables(version).iter().copied().collect();
                let actual: BTreeSet<&str> = tables.iter().map(String::as_str).collect();
                if expected != actual {
                    return Err(StorageError::Unsupported(
                        "Workbook 2のスキーマが一致しません".to_string(),
                    ));
                }
                if version < SCHEMA_VERSION {
                    migrate_connection(&mut connection, &backups, version)?;
                }
            }
            if existed {
                let integrity: String =
                    connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
                if integrity != "ok" {
                    return Err(StorageError::Unsupported(format!(
                        "データベース整合性チェックに失敗しました: {integrity}"
                    )));
                }
            }
        }
        set_private_file(&path)?;
        Ok(Self {
            conn: Some(connection),
            path: Some(path),
            data_dir: directory.to_path_buf(),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        connection.execute_batch(schema::SCHEMA_SQL)?;
        connection.execute_batch(&format!(
            "PRAGMA application_id={APP_ID}; PRAGMA user_version={SCHEMA_VERSION};"
        ))?;
        Ok(Self {
            conn: Some(connection),
            path: None,
            data_dir: PathBuf::from("."),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn connection(&self) -> Result<&Connection> {
        self.conn
            .as_ref()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))
    }

    pub fn integrity(&self) -> Result<String> {
        let connection = self.connection()?;
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let fk: i64 =
            connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if integrity == "ok" && fk == 0 {
            Ok("ok".to_string())
        } else {
            Ok(format!(
                "integrity={integrity}, foreign_key_violations={fk}"
            ))
        }
    }

    // ---- settings -------------------------------------------------------

    pub fn settings(&self) -> Result<Settings> {
        let raw: Option<String> = self
            .connection()?
            .query_row("SELECT data FROM settings WHERE id=1", [], |row| row.get(0))
            .optional()?;
        match raw {
            Some(raw) => Settings::from_json(&raw)
                .map_err(|error| StorageError::Validation(error.to_string())),
            None => Ok(Settings::default()),
        }
    }

    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        settings
            .validate()
            .map_err(|error| StorageError::Validation(error.0))?;
        self.connection()?.execute(
            "INSERT INTO settings(id,data) VALUES(1,?1)
             ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            [settings.to_json()],
        )?;
        Ok(())
    }

    // ---- workbooks ------------------------------------------------------

    pub fn workbooks(&self) -> Result<Vec<WorkbookRecord>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT local_id,source_id,source_revision,title,content_json,imported_at,last_used_at
             FROM workbooks ORDER BY COALESCE(last_used_at,imported_at) DESC, local_id",
        )?;
        let rows = statement.query_map([], map_workbook_record)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn workbook_record(&self, local_id: &str) -> Result<WorkbookRecord> {
        self.connection()?
            .query_row(
                "SELECT local_id,source_id,source_revision,title,content_json,imported_at,last_used_at
                 FROM workbooks WHERE local_id=?1",
                [local_id],
                map_workbook_record,
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("Workbook {local_id}")))
    }

    pub fn workbook(&self, local_id: &str) -> Result<Workbook> {
        let record = self.workbook_record(local_id)?;
        parse_workbook(&record.content_json)
            .map_err(|error| StorageError::Validation(error.to_string()))
    }

    pub fn import_workbook(
        &mut self,
        workbook: &Workbook,
        assets: &[AssetContent],
        now: DateTime<Utc>,
    ) -> Result<String> {
        let local_id = Uuid::new_v4().to_string();
        self.import_workbook_with_id(workbook, assets, now, &local_id)?;
        Ok(local_id)
    }

    fn import_workbook_with_id(
        &mut self,
        workbook: &Workbook,
        assets: &[AssetContent],
        now: DateTime<Utc>,
        local_id: &str,
    ) -> Result<()> {
        workbook.validate()?;
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO workbooks(local_id,source_id,source_revision,title,content_json,imported_at,last_used_at)
             VALUES(?1,?2,?3,?4,?5,?6,NULL)",
            params![
                local_id,
                workbook.id,
                workbook.revision,
                workbook.title,
                workbook.to_json(),
                format_ts(now)
            ],
        )?;
        for note in &workbook.notes {
            transaction.execute(
                "INSERT INTO notes(workbook_id,note_id,introduced,introduced_at) VALUES(?1,?2,0,NULL)",
                params![local_id, note.id],
            )?;
            for question in &note.questions {
                transaction.execute(
                    "INSERT INTO questions(workbook_id,question_id,note_id,buried_until) VALUES(?1,?2,?3,NULL)",
                    params![local_id, question.id, note.id],
                )?;
                for path in &question.topics {
                    let mut parent: Option<String> = None;
                    for name in parse_topic_path(path)? {
                        let topic_id = ensure_topic(&transaction, &name, parent.as_deref(), now)?;
                        parent = Some(topic_id);
                    }
                    if let Some(topic_id) = parent {
                        transaction.execute(
                            "INSERT OR IGNORE INTO question_topics(workbook_id,question_id,topic_id)
                             VALUES(?1,?2,?3)",
                            params![local_id, question.id, topic_id],
                        )?;
                    }
                }
            }
        }
        for asset in assets {
            transaction.execute(
                "INSERT INTO assets(workbook_id,asset_id,media_type,alt,width,height,bytes)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    local_id,
                    asset.id,
                    asset.media_type,
                    asset.alt,
                    asset.width,
                    asset.height,
                    asset.bytes
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn asset(&self, workbook_id: &str, asset_id: &str) -> Result<Option<AssetData>> {
        let row = self
            .connection()?
            .query_row(
                "SELECT asset_id,media_type,alt,width,height,bytes
                 FROM assets WHERE workbook_id=?1 AND asset_id=?2",
                params![workbook_id, asset_id],
                |row| {
                    Ok(AssetData {
                        id: row.get(0)?,
                        media_type: row.get(1)?,
                        alt: row.get(2)?,
                        width: row.get::<_, i64>(3)? as u32,
                        height: row.get::<_, i64>(4)? as u32,
                        bytes: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn asset_ids(&self, workbook_id: &str) -> Result<Vec<String>> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT asset_id FROM assets WHERE workbook_id=?1")?;
        let rows = statement.query_map([workbook_id], |row| row.get::<_, String>(0))?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn delete_workbook(&mut self, local_id: &str) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let active: Option<String> = connection
            .query_row(
                "SELECT s.id FROM sessions s
                 JOIN session_items i ON i.session_id=s.id
                 WHERE s.status='active' AND i.workbook_id=?1 LIMIT 1",
                [local_id],
                |row| row.get(0),
            )
            .optional()?;
        if active.is_some() {
            return Err(StorageError::Conflict(
                "学習中の教材は削除できません".to_string(),
            ));
        }
        let changed = connection.execute("DELETE FROM workbooks WHERE local_id=?1", [local_id])?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("Workbook {local_id}")));
        }
        Ok(())
    }

    pub fn touch_workbook(&mut self, local_id: &str, now: DateTime<Utc>) -> Result<()> {
        self.connection()?.execute(
            "UPDATE workbooks SET last_used_at=?1 WHERE local_id=?2",
            params![format_ts(now), local_id],
        )?;
        Ok(())
    }

    // ---- topics ---------------------------------------------------------

    pub fn topics(&self) -> Result<Vec<Topic>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,name,parent_id,description FROM topics
             ORDER BY IFNULL(parent_id,''), name_key, id",
        )?;
        let rows = statement.query_map([], map_topic)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn topic(&self, id: &str) -> Result<Topic> {
        self.connection()?
            .query_row(
                "SELECT id,name,parent_id,description FROM topics WHERE id=?1",
                [id],
                map_topic,
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("Topic {id}")))
    }

    /// All question-to-topic assignments as `(workbook_id, question_id,
    /// topic_id)`. The catalog uses this to resolve topic scopes without a
    /// query per question.
    pub fn question_topics(&self) -> Result<Vec<(String, String, String)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT workbook_id,question_id,topic_id FROM question_topics
             ORDER BY workbook_id,question_id,topic_id",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn insert_topic(&mut self, topic: &Topic, now: DateTime<Utc>) -> Result<()> {
        topic.validate()?;
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        if let Some(parent) = topic.parent_id.as_deref() {
            ensure_topic_parent(connection, parent, None)?;
        }
        let name_key = normalize_topic_name(&topic.name);
        ensure_no_sibling(connection, topic.parent_id.as_deref(), &name_key, None)?;
        connection.execute(
            "INSERT INTO topics(id,name,name_key,parent_id,description,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                topic.id,
                topic.name,
                name_key,
                topic.parent_id,
                topic.description,
                format_ts(now),
                format_ts(now)
            ],
        )?;
        Ok(())
    }

    pub fn update_topic(&mut self, topic: &Topic, now: DateTime<Utc>) -> Result<()> {
        topic.validate()?;
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let exists: i64 = connection.query_row(
            "SELECT COUNT(*) FROM topics WHERE id=?1",
            [&topic.id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(StorageError::NotFound(format!("Topic {}", topic.id)));
        }
        if let Some(parent) = topic.parent_id.as_deref() {
            ensure_topic_parent(connection, parent, Some(&topic.id))?;
        }
        let name_key = normalize_topic_name(&topic.name);
        ensure_no_sibling(
            connection,
            topic.parent_id.as_deref(),
            &name_key,
            Some(&topic.id),
        )?;
        connection.execute(
            "UPDATE topics SET name=?2,name_key=?3,parent_id=?4,description=?5,updated_at=?6
             WHERE id=?1",
            params![
                topic.id,
                topic.name,
                name_key,
                topic.parent_id,
                topic.description,
                format_ts(now)
            ],
        )?;
        Ok(())
    }

    pub fn delete_topic(&mut self, id: &str) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let children: i64 = connection.query_row(
            "SELECT COUNT(*) FROM topics WHERE parent_id=?1",
            [id],
            |row| row.get(0),
        )?;
        if children > 0 {
            return Err(StorageError::Conflict(
                "子Topicがあるため削除できません。先に子Topicを移動してください".to_string(),
            ));
        }
        let changed = connection.execute("DELETE FROM topics WHERE id=?1", [id])?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("Topic {id}")));
        }
        Ok(())
    }

    /// Replaces all topic assignments for one question. Used by the authoring
    /// editor; the workbook JSON is updated by the service in the same
    /// operation so export and import stay consistent.
    pub fn set_question_topics(
        &mut self,
        workbook_id: &str,
        question_id: &str,
        topic_ids: &[String],
    ) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        let question_exists: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM questions WHERE workbook_id=?1 AND question_id=?2",
            params![workbook_id, question_id],
            |row| row.get(0),
        )?;
        if question_exists == 0 {
            return Err(StorageError::NotFound(format!(
                "Question {workbook_id}/{question_id}"
            )));
        }
        for topic_id in topic_ids {
            let topic_exists: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM topics WHERE id=?1",
                [topic_id],
                |row| row.get(0),
            )?;
            if topic_exists == 0 {
                return Err(StorageError::NotFound(format!("Topic {topic_id}")));
            }
        }
        transaction.execute(
            "DELETE FROM question_topics WHERE workbook_id=?1 AND question_id=?2",
            params![workbook_id, question_id],
        )?;
        for topic_id in topic_ids {
            transaction.execute(
                "INSERT OR IGNORE INTO question_topics(workbook_id,question_id,topic_id)
                 VALUES(?1,?2,?3)",
                params![workbook_id, question_id, topic_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    // ---- exams ----------------------------------------------------------

    pub fn exams(&self, include_archived: bool) -> Result<Vec<Exam>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,title,description,target_date,target_score,daily_minutes,scope_json,
                    created_at,updated_at,archived_at
             FROM exams
             WHERE ?1=1 OR archived_at IS NULL
             ORDER BY archived_at IS NOT NULL,
                      COALESCE(target_date,'9999-12-31'),
                      updated_at DESC, title, id",
        )?;
        let rows = statement.query_map([i64::from(include_archived)], map_exam)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn exam(&self, id: &str) -> Result<Exam> {
        self.connection()?
            .query_row(
                "SELECT id,title,description,target_date,target_score,daily_minutes,scope_json,
                        created_at,updated_at,archived_at
                 FROM exams WHERE id=?1",
                [id],
                map_exam,
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("Exam {id}")))
    }

    pub fn insert_exam(&mut self, exam: &Exam) -> Result<()> {
        let scope_json = serde_json::to_string(&exam.scope)?;
        self.connection()?.execute(
            "INSERT INTO exams(id,title,description,target_date,target_score,daily_minutes,
                    scope_json,created_at,updated_at,archived_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                exam.id,
                exam.title,
                exam.description,
                exam.target_date.map(|date| date.to_string()),
                exam.target_score,
                exam.daily_minutes,
                scope_json,
                format_ts(exam.created_at),
                format_ts(exam.updated_at),
                exam.archived_at.map(format_ts),
            ],
        )?;
        Ok(())
    }

    pub fn update_exam(&mut self, exam: &Exam) -> Result<()> {
        let scope_json = serde_json::to_string(&exam.scope)?;
        let changed = self.connection()?.execute(
            "UPDATE exams SET title=?1,description=?2,target_date=?3,target_score=?4,
                    daily_minutes=?5,scope_json=?6,updated_at=?7,archived_at=?8 WHERE id=?9",
            params![
                exam.title,
                exam.description,
                exam.target_date.map(|date| date.to_string()),
                exam.target_score,
                exam.daily_minutes,
                scope_json,
                format_ts(exam.updated_at),
                exam.archived_at.map(format_ts),
                exam.id,
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("Exam {}", exam.id)));
        }
        Ok(())
    }

    pub fn archive_exam(&mut self, id: &str, archived: bool, now: DateTime<Utc>) -> Result<()> {
        let changed = self.connection()?.execute(
            "UPDATE exams SET archived_at=?1,updated_at=?2 WHERE id=?3",
            params![
                if archived { Some(format_ts(now)) } else { None },
                format_ts(now),
                id
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("Exam {id}")));
        }
        Ok(())
    }

    pub fn delete_exam(&mut self, id: &str) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let active: Option<String> = connection
            .query_row(
                "SELECT id FROM sessions WHERE status='active' AND exam_id=?1 LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if active.is_some() {
            return Err(StorageError::Conflict(
                "この試験の学習中は削除できません".to_string(),
            ));
        }
        let transaction = connection.transaction()?;
        // Mock answers/results are assessment-local. Deleting the exam removes
        // those sessions; ordinary Review/Practice sessions keep their attempts
        // and become unlinked through ON DELETE SET NULL.
        transaction.execute(
            "DELETE FROM sessions WHERE exam_id=?1 AND mode='mock'",
            [id],
        )?;
        let changed = transaction.execute("DELETE FROM exams WHERE id=?1", [id])?;
        if changed == 0 {
            return Err(StorageError::NotFound(format!("Exam {id}")));
        }
        transaction.commit()?;
        Ok(())
    }

    /// Attempt counts by question, used by Practice selection strategies.
    /// Only deterministic attempt history is counted; practice answers are
    /// already stored in `attempts` without touching FSRS.
    pub fn question_attempt_stats(&self) -> Result<Vec<(String, String, QuestionAttemptStat)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT workbook_id,question_id,COUNT(*),
                    COALESCE(SUM(CASE WHEN correct=0 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(elapsed_ms),0)
             FROM attempts GROUP BY workbook_id,question_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                QuestionAttemptStat {
                    attempts: row.get::<_, i64>(2)? as usize,
                    incorrect: row.get::<_, i64>(3)? as usize,
                    elapsed_ms: row.get::<_, i64>(4)?,
                },
            ))
        })?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn exam_practice_stats(&self, exam_id: &str) -> Result<ExamPracticeStats> {
        let connection = self.connection()?;
        let (attempts, graded, correct): (i64, i64, i64) = connection.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN a.correct IS NOT NULL THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN a.correct=1 THEN 1 ELSE 0 END),0)
             FROM attempts a JOIN sessions s ON s.id=a.session_id
             WHERE s.exam_id=?1 AND s.mode='practice'",
            [exam_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(ExamPracticeStats {
            attempts: attempts as usize,
            graded: graded as usize,
            correct: correct as usize,
        })
    }

    /// Total time spent answering this exam's non-mock sessions since `since`.
    /// The planner subtracts it from today's budget so finishing a session and
    /// starting another does not silently grant a second full daily allowance.
    pub fn exam_studied_ms_since(&self, exam_id: &str, since: DateTime<Utc>) -> Result<i64> {
        let connection = self.connection()?;
        let total: i64 = connection.query_row(
            "SELECT COALESCE(SUM(a.elapsed_ms),0)
             FROM attempts a JOIN sessions s ON s.id=a.session_id
             WHERE s.exam_id=?1 AND s.mode<>'mock' AND a.answered_at>=?2",
            params![exam_id, format_ts(since)],
            |row| row.get(0),
        )?;
        Ok(total)
    }

    pub fn mock_answers_for_session(&self, session_id: &str) -> Result<Vec<MockAnswerRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,session_id,item_id,workbook_id,note_id,question_id,answered_at,
                    correct,answer_json,created_at
             FROM mock_answers WHERE session_id=?1 ORDER BY answered_at, rowid",
        )?;
        let rows = statement.query_map([session_id], map_mock_answer)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn record_mock_answer(
        &mut self,
        answer: &MockAnswerRow,
        session: &SessionRow,
    ) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO mock_answers(id,session_id,item_id,workbook_id,note_id,question_id,
                    answered_at,correct,answer_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                answer.id,
                answer.session_id,
                answer.item_id,
                answer.workbook_id,
                answer.note_id,
                answer.question_id,
                format_ts(answer.answered_at),
                answer.correct.map(i64::from),
                answer.answer_json,
                format_ts(answer.created_at),
            ],
        )?;
        transaction.execute(
            "UPDATE session_items SET status='answered' WHERE id=?1",
            [&answer.item_id],
        )?;
        update_session_row(&transaction, session)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn finish_mock(&mut self, result: &MockResultRow, session: &SessionRow) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO mock_results(session_id,exam_id,submitted_at,total,correct,incorrect,
                    unanswered,ungraded,duration_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(session_id) DO UPDATE SET
               submitted_at=excluded.submitted_at,total=excluded.total,correct=excluded.correct,
               incorrect=excluded.incorrect,unanswered=excluded.unanswered,
               ungraded=excluded.ungraded,duration_ms=excluded.duration_ms",
            params![
                result.session_id,
                result.exam_id,
                format_ts(result.submitted_at),
                result.total as i64,
                result.correct as i64,
                result.incorrect as i64,
                result.unanswered as i64,
                result.ungraded as i64,
                result.duration_ms,
            ],
        )?;
        update_session_row(&transaction, session)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn mock_result(&self, session_id: &str) -> Result<Option<MockResultRow>> {
        self.connection()?
            .query_row(
                "SELECT session_id,exam_id,submitted_at,total,correct,incorrect,unanswered,
                        ungraded,duration_ms
                 FROM mock_results WHERE session_id=?1",
                [session_id],
                map_mock_result,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn mock_results(&self, exam_id: &str, limit: usize) -> Result<Vec<MockResultRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT session_id,exam_id,submitted_at,total,correct,incorrect,unanswered,
                    ungraded,duration_ms
             FROM mock_results WHERE exam_id=?1 ORDER BY submitted_at DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![exam_id, limit as i64], map_mock_result)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    // ---- notes / review states -----------------------------------------

    pub fn note_introduced(&self, workbook_id: &str, note_id: &str) -> Result<bool> {
        let value: Option<i64> = self
            .connection()?
            .query_row(
                "SELECT introduced FROM notes WHERE workbook_id=?1 AND note_id=?2",
                params![workbook_id, note_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or(0) != 0)
    }

    pub fn review_state(&self, workbook_id: &str, question_id: &str) -> Result<Option<StoredCard>> {
        let raw: Option<CardColumns> = self
            .connection()?
            .query_row(
                "SELECT state,stability,stability_fast,difficulty,due_at,last_review_at,reps,lapses,
                        scheduled_days,learning_step,algorithm,policy_version,revision
                 FROM review_states WHERE workbook_id=?1 AND question_id=?2",
                params![workbook_id, question_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                    ))
                },
            )
            .optional()?;
        raw.map(map_stored_card).transpose()
    }

    pub fn review_states(&self) -> Result<Vec<(String, String, StoredCard)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT workbook_id,question_id,state,stability,stability_fast,difficulty,due_at,
                    last_review_at,reps,lapses,scheduled_days,learning_step,algorithm,policy_version,revision
             FROM review_states",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                (
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, f64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, i64>(14)?,
                ),
            ))
        })?;
        let mut output = Vec::new();
        for row in rows {
            let (workbook_id, question_id, card) = row?;
            output.push((workbook_id, question_id, map_stored_card(card)?));
        }
        Ok(output)
    }

    // ---- candidates -----------------------------------------------------

    pub fn new_candidates(&self, workbook_id: &str, limit: usize) -> Result<Vec<SessionCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT q.workbook_id,q.note_id,q.question_id,NULL
             FROM questions q
             LEFT JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.question_id IS NULL
             ORDER BY q.rowid LIMIT ?2",
        )?;
        let rows = statement.query_map(params![workbook_id, limit as i64], map_candidate)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn new_question_count(&self, workbook_id: &str) -> Result<usize> {
        let count: i64 = self.connection()?.query_row(
            "SELECT COUNT(*) FROM questions q
             LEFT JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.question_id IS NULL",
            [workbook_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn learned_count(&self, workbook_id: &str) -> Result<usize> {
        let count: i64 = self.connection()?.query_row(
            "SELECT COUNT(*) FROM review_states WHERE workbook_id=?1",
            [workbook_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Review candidates combine due Learning/Relearning cards (all of them)
    /// with due Review cards (one per note, oldest first). Buried sibling
    /// reviews are omitted until the next learning day.
    pub fn review_candidates(
        &self,
        workbook_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Vec<SessionCandidate>> {
        let connection = self.connection()?;
        let now = format_ts(now);
        let mut output = Vec::new();
        let mut seen_notes = BTreeSet::new();
        let mut seen_questions = BTreeSet::new();

        let sql_short = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state IN (1,3) AND rs.due_at<=?2
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state IN (1,3) AND rs.due_at<=?1
             ORDER BY rs.due_at,q.rowid"
        };
        {
            let mut statement = connection.prepare(sql_short)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(params![workbook_id, now], map_candidate)?
            } else {
                statement.query_map([now.clone()], map_candidate)?
            };
            for row in rows {
                let candidate = row?;
                let note_key = (candidate.workbook_id.clone(), candidate.note_id.clone());
                if !seen_notes.insert(note_key) {
                    continue;
                }
                seen_questions
                    .insert((candidate.workbook_id.clone(), candidate.question_id.clone()));
                output.push(candidate);
            }
        }

        let sql_review = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state=2 AND rs.due_at<=?2
               AND (q.buried_until IS NULL OR q.buried_until<=?2)
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state=2 AND rs.due_at<=?1
               AND (q.buried_until IS NULL OR q.buried_until<=?1)
             ORDER BY rs.due_at,q.rowid"
        };
        {
            let mut statement = connection.prepare(sql_review)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(params![workbook_id, now], map_candidate)?
            } else {
                statement.query_map([now.clone()], map_candidate)?
            };
            for row in rows {
                let candidate = row?;
                if seen_questions
                    .contains(&(candidate.workbook_id.clone(), candidate.question_id.clone()))
                {
                    continue;
                }
                let note_key = (candidate.workbook_id.clone(), candidate.note_id.clone());
                if !seen_notes.insert(note_key) {
                    continue;
                }
                output.push(candidate);
            }
        }
        output.sort_by(|a, b| {
            a.due_at
                .cmp(&b.due_at)
                .then(a.workbook_id.cmp(&b.workbook_id))
                .then(a.note_id.cmp(&b.note_id))
                .then(a.question_id.cmp(&b.question_id))
        });
        Ok(output)
    }

    /// Candidates for a time-budgeted session. Unlike `review_candidates`,
    /// this also exposes whether a card is a due short-term repeat, a due
    /// review, new content, or a Learn Ahead repeat, so the scheduler can keep
    /// the documented priority. `new_limit` is the remaining daily allowance.
    pub fn budget_candidates(
        &self,
        workbook_id: Option<&str>,
        now: DateTime<Utc>,
        new_limit: usize,
    ) -> Result<Vec<BudgetCandidate>> {
        let connection = self.connection()?;
        let now_text = format_ts(now);
        let learn_ahead_until = format_ts(now + chrono::Duration::minutes(20));
        let mut output = Vec::new();
        let mut seen_questions = BTreeSet::new();
        let mut seen_notes = BTreeSet::new();

        let short_sql = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state IN (1,3) AND rs.due_at<=?2
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state IN (1,3) AND rs.due_at<=?1
             ORDER BY rs.due_at,q.rowid"
        };
        {
            let mut statement = connection.prepare(short_sql)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(params![workbook_id, now_text], map_budget_candidate)?
            } else {
                statement.query_map([now_text.clone()], map_budget_candidate)?
            };
            for row in rows {
                let (workbook_id, note_id, question_id, due_at) = row?;
                let due_at = due_at.as_deref().map(parse_ts).transpose()?;
                let key = (workbook_id.clone(), question_id.clone());
                let note_key = (workbook_id.clone(), note_id.clone());
                if !seen_questions.insert(key) || !seen_notes.insert(note_key) {
                    continue;
                }
                output.push(BudgetCandidate {
                    workbook_id,
                    note_id,
                    question_id,
                    due_at,
                    pool: BudgetPool::ShortTermDue,
                });
            }
        }

        let review_sql = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state=2 AND rs.due_at<=?2
               AND (q.buried_until IS NULL OR q.buried_until<=?2)
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state=2 AND rs.due_at<=?1
               AND (q.buried_until IS NULL OR q.buried_until<=?1)
             ORDER BY rs.due_at,q.rowid"
        };
        {
            let mut statement = connection.prepare(review_sql)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(params![workbook_id, now_text], map_budget_candidate)?
            } else {
                statement.query_map([now_text.clone()], map_budget_candidate)?
            };
            for row in rows {
                let (workbook_id, note_id, question_id, due_at) = row?;
                let due_at = due_at.as_deref().map(parse_ts).transpose()?;
                let key = (workbook_id.clone(), question_id.clone());
                let note_key = (workbook_id.clone(), note_id.clone());
                if seen_questions.contains(&key) || !seen_notes.insert(note_key) {
                    continue;
                }
                seen_questions.insert(key);
                output.push(BudgetCandidate {
                    workbook_id,
                    note_id,
                    question_id,
                    due_at,
                    pool: BudgetPool::DueReview,
                });
            }
        }

        if new_limit > 0 {
            let new_sql = if workbook_id.is_some() {
                "SELECT q.workbook_id,q.note_id,q.question_id,NULL
                 FROM questions q LEFT JOIN review_states rs
                   ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
                 WHERE q.workbook_id=?1 AND rs.question_id IS NULL
                 ORDER BY q.rowid LIMIT ?2"
            } else {
                "SELECT q.workbook_id,q.note_id,q.question_id,NULL
                 FROM questions q LEFT JOIN review_states rs
                   ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
                 WHERE rs.question_id IS NULL
                 ORDER BY q.rowid LIMIT ?1"
            };
            let mut statement = connection.prepare(new_sql)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(params![workbook_id, new_limit as i64], map_budget_candidate)?
            } else {
                statement.query_map([new_limit as i64], map_budget_candidate)?
            };
            for row in rows {
                let (workbook_id, note_id, question_id, _) = row?;
                if !seen_questions.insert((workbook_id.clone(), question_id.clone())) {
                    continue;
                }
                output.push(BudgetCandidate {
                    workbook_id,
                    note_id,
                    question_id,
                    due_at: None,
                    pool: BudgetPool::New,
                });
            }
        }

        let ahead_sql = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state IN (1,3)
               AND rs.due_at>?2 AND rs.due_at<=?3
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state IN (1,3)
               AND rs.due_at>?1 AND rs.due_at<=?2
             ORDER BY rs.due_at,q.rowid"
        };
        {
            let mut statement = connection.prepare(ahead_sql)?;
            let rows = if let Some(workbook_id) = workbook_id {
                statement.query_map(
                    params![workbook_id, now_text, learn_ahead_until],
                    map_budget_candidate,
                )?
            } else {
                statement.query_map(params![now_text, learn_ahead_until], map_budget_candidate)?
            };
            for row in rows {
                let (workbook_id, note_id, question_id, due_at) = row?;
                let due_at = due_at.as_deref().map(parse_ts).transpose()?;
                let key = (workbook_id.clone(), question_id.clone());
                let note_key = (workbook_id.clone(), note_id.clone());
                if seen_questions.contains(&key) || !seen_notes.insert(note_key) {
                    continue;
                }
                seen_questions.insert(key);
                output.push(BudgetCandidate {
                    workbook_id,
                    note_id,
                    question_id,
                    due_at,
                    pool: BudgetPool::LearnAhead,
                });
            }
        }

        Ok(output)
    }

    /// Short-term cards whose due time is later than `after` but within
    /// `before`. Used by Today to avoid claiming completion while a 1m/10m
    /// repeat is still pending, and by Review session start when there are no
    /// normal due candidates (Learn Ahead).
    pub fn upcoming_short_term(
        &self,
        workbook_id: Option<&str>,
        after: DateTime<Utc>,
        before: DateTime<Utc>,
    ) -> Result<Vec<SessionCandidate>> {
        let connection = self.connection()?;
        let sql = if workbook_id.is_some() {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE q.workbook_id=?1 AND rs.state IN (1,3)
               AND rs.due_at>?2 AND rs.due_at<=?3
             ORDER BY rs.due_at,q.rowid"
        } else {
            "SELECT q.workbook_id,q.note_id,q.question_id,rs.due_at
             FROM questions q JOIN review_states rs
               ON rs.workbook_id=q.workbook_id AND rs.question_id=q.question_id
             WHERE rs.state IN (1,3)
               AND rs.due_at>?1 AND rs.due_at<=?2
             ORDER BY rs.due_at,q.rowid"
        };
        let mut statement = connection.prepare(sql)?;
        let rows = if let Some(workbook_id) = workbook_id {
            statement.query_map(
                params![workbook_id, format_ts(after), format_ts(before)],
                map_candidate,
            )?
        } else {
            statement.query_map(params![format_ts(after), format_ts(before)], map_candidate)?
        };
        let mut output = Vec::new();
        let mut seen_notes = BTreeSet::new();
        for row in rows {
            let candidate = row?;
            if seen_notes.insert((candidate.workbook_id.clone(), candidate.note_id.clone())) {
                output.push(candidate);
            }
        }
        Ok(output)
    }

    /// Sibling reviews postponed to the next learning day. Scope-aware
    /// candidate selection uses this so a buried question is never selected.
    pub fn buried_questions(&self, now: DateTime<Utc>) -> Result<BTreeSet<(String, String)>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT workbook_id,question_id FROM questions
             WHERE buried_until IS NOT NULL AND buried_until>?1",
        )?;
        let rows = statement.query_map([format_ts(now)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut output = BTreeSet::new();
        for row in rows {
            output.insert(row?);
        }
        Ok(output)
    }

    /// The earliest saved due time, used to schedule the UI refresh timer at
    /// the next real deadline instead of scanning SQLite on every render.
    pub fn next_due_at(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
        let raw: Option<String> = self.connection()?.query_row(
            "SELECT MIN(due_at) FROM review_states WHERE due_at > ?1",
            [format_ts(after)],
            |row| row.get(0),
        )?;
        raw.as_deref().map(parse_ts).transpose()
    }

    /// Distinct questions that received their first New-session commit on the
    /// current learning day. Used to enforce the daily new-question limit
    /// without counting skips or practice attempts.
    pub fn new_questions_introduced_today(
        &self,
        calendar: &StudyCalendar,
        now: DateTime<Utc>,
    ) -> Result<usize> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT a.workbook_id, a.question_id, a.answered_at
             FROM attempts a JOIN sessions s ON s.id=a.session_id
             WHERE s.mode IN ('new','plan') AND a.answered_at>=?1
               AND a.before_state_json IS NULL",
        )?;
        let since = format_ts(now - chrono::Duration::hours(48));
        let rows = statement.query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let today = calendar.learning_day(now);
        let mut questions = BTreeSet::new();
        for row in rows {
            let (workbook_id, question_id, answered_at) = row?;
            if calendar.learning_day(parse_ts(&answered_at)?) == today {
                questions.insert((workbook_id, question_id));
            }
        }
        Ok(questions.len())
    }

    pub fn practice_candidates(&self, workbook_id: &str) -> Result<Vec<SessionCandidate>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT workbook_id,note_id,question_id,NULL FROM questions
             WHERE workbook_id=?1 ORDER BY rowid",
        )?;
        let rows = statement.query_map([workbook_id], map_candidate)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    // ---- sessions -------------------------------------------------------

    pub fn active_session(&self) -> Result<Option<SessionBundle>> {
        let id: Option<String> = self
            .connection()?
            .query_row(
                "SELECT id FROM sessions WHERE status='active' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.session_bundle(&id)).transpose()
    }

    pub fn session_bundle(&self, session_id: &str) -> Result<SessionBundle> {
        let connection = self.connection()?;
        let session = connection
            .query_row(
                "SELECT id,mode,status,started_at,ended_at,current_workbook_id,current_item_id,phase,
                        draft_json,hint,revealed,pending_correct,pending_unknown,pending_at,
                        question_started_at,question_elapsed_ms,cursor_kind,assist_json,revision,
                        exam_id,time_limit_seconds,time_budget_seconds
                 FROM sessions WHERE id=?1",
                [session_id],
                map_session,
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound(format!("Session {session_id}")))?;
        let mut statement = connection.prepare(
            "SELECT id,session_id,ordinal,workbook_id,note_id,question_id,status
             FROM session_items WHERE session_id=?1 ORDER BY ordinal",
        )?;
        let rows = statement.query_map([session_id], map_session_item)?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        let mut statement = connection.prepare(
            "SELECT session_id,workbook_id,ordinal,result_ack FROM session_workbooks
             WHERE session_id=?1 ORDER BY ordinal",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok(SessionWorkbookRow {
                session_id: row.get(0)?,
                workbook_id: row.get(1)?,
                ordinal: row.get(2)?,
                result_ack: row.get::<_, i64>(3)? != 0,
            })
        })?;
        let mut workbooks = Vec::new();
        for row in rows {
            workbooks.push(row?);
        }
        let attempts = self.attempts_for_session(session_id)?;
        Ok(SessionBundle {
            session,
            items,
            workbooks,
            attempts,
        })
    }

    pub fn create_session(&mut self, bundle: &SessionBundle) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        insert_session(&transaction, &bundle.session)?;
        for item in &bundle.items {
            insert_session_item(&transaction, item)?;
        }
        for workbook in &bundle.workbooks {
            insert_session_workbook(&transaction, workbook)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn update_session(&mut self, session: &SessionRow) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        update_session_row(connection, session)
    }

    pub fn set_item_status(&mut self, item_id: &str, status: SessionItemStatus) -> Result<()> {
        self.connection()?.execute(
            "UPDATE session_items SET status=?1 WHERE id=?2",
            params![status.as_str(), item_id],
        )?;
        Ok(())
    }

    pub fn set_workbook_ack(
        &mut self,
        session_id: &str,
        workbook_id: &str,
        ack: bool,
    ) -> Result<()> {
        self.connection()?.execute(
            "UPDATE session_workbooks SET result_ack=?1 WHERE session_id=?2 AND workbook_id=?3",
            params![i64::from(ack), session_id, workbook_id],
        )?;
        Ok(())
    }

    pub fn session_queue(
        &self,
        session_id: &str,
        workbook_id: &str,
    ) -> Result<Vec<workbook_scheduler::QueueQuestion>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT si.id,si.workbook_id,si.note_id,si.question_id,si.ordinal,si.status,
                    rs.state,rs.due_at
             FROM session_items si
             LEFT JOIN review_states rs
               ON rs.workbook_id=si.workbook_id AND rs.question_id=si.question_id
             WHERE si.session_id=?1 AND si.workbook_id=?2
             ORDER BY si.ordinal",
        )?;
        let rows = statement.query_map(params![session_id, workbook_id], |row| {
            let status: String = row.get(5)?;
            let state: Option<i64> = row.get(6)?;
            let due: Option<String> = row.get(7)?;
            Ok(workbook_scheduler::QueueQuestion {
                item_id: row.get(0)?,
                workbook_id: row.get(1)?,
                note_id: row.get(2)?,
                question_id: row.get(3)?,
                ordinal: row.get::<_, i64>(4)? as u32,
                status: SessionItemStatus::parse(&status).unwrap_or(SessionItemStatus::Pending),
                state: state
                    .and_then(|value| u8::try_from(value).ok())
                    .and_then(CardState::from_value),
                due_at: due
                    .as_deref()
                    .map(parse_ts)
                    .transpose()
                    .map_err(to_sql_error)?,
            })
        })?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn attempts_for_session(&self, session_id: &str) -> Result<Vec<AttemptRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,session_id,workbook_id,note_id,question_id,answered_at,correct,unknown,hint,
                    elapsed_ms,rating,rating_source,answer_json,before_state_json,after_state_json,
                    state_revision,short_term,assist_json,created_at
             FROM attempts WHERE session_id=?1 ORDER BY answered_at, created_at, rowid",
        )?;
        let rows = statement.query_map([session_id], map_attempt)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn attempt(&self, attempt_id: &str) -> Result<Option<AttemptRow>> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id,session_id,workbook_id,note_id,question_id,answered_at,correct,unknown,hint,
                        elapsed_ms,rating,rating_source,answer_json,before_state_json,after_state_json,
                        state_revision,short_term,assist_json,created_at
                 FROM attempts WHERE id=?1",
                [attempt_id],
                map_attempt,
            )
            .optional()?;
        Ok(row)
    }

    pub fn current_attempt(
        &self,
        session_id: &str,
        question_id: &str,
    ) -> Result<Option<AttemptRow>> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id,session_id,workbook_id,note_id,question_id,answered_at,correct,unknown,hint,
                        elapsed_ms,rating,rating_source,answer_json,before_state_json,after_state_json,
                        state_revision,short_term,assist_json,created_at
                 FROM attempts WHERE session_id=?1 AND question_id=?2
                 ORDER BY answered_at DESC, created_at DESC, rowid DESC LIMIT 1",
                params![session_id, question_id],
                map_attempt,
            )
            .optional()?;
        Ok(row)
    }

    pub fn attempts_since(&self, since: DateTime<Utc>) -> Result<Vec<AttemptRow>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id,session_id,workbook_id,note_id,question_id,answered_at,correct,unknown,hint,
                    elapsed_ms,rating,rating_source,answer_json,before_state_json,after_state_json,
                    state_revision,short_term,assist_json,created_at
             FROM attempts WHERE answered_at>=?1 ORDER BY answered_at",
        )?;
        let rows = statement.query_map([format_ts(since)], map_attempt)?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row?);
        }
        Ok(output)
    }

    pub fn history(&self, limit: usize) -> Result<Vec<SessionBundle>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id FROM sessions WHERE status='ended' AND mode<>'mock'
             ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        ids.into_iter().map(|id| self.session_bundle(&id)).collect()
    }

    pub fn session_counts(&self, bundle: &SessionBundle) -> SessionCounts {
        counts_for_workbook(bundle, None)
    }

    // ---- commit / correction -------------------------------------------

    pub fn commit_attempt(&mut self, write: CommitWrite) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        if let Some(card) = write.card.as_ref() {
            upsert_review_state(
                &transaction,
                &write.attempt.workbook_id,
                &write.attempt.question_id,
                card,
            )?;
        }
        insert_attempt(&transaction, &write.attempt)?;
        update_session_row(&transaction, &write.session)?;
        if write.mark_answered
            && let Some(item_id) = write.item_id.as_ref()
        {
            transaction.execute(
                "UPDATE session_items SET status='answered' WHERE id=?1",
                [item_id],
            )?;
        }
        if write.introduce_note {
            transaction.execute(
                "UPDATE notes SET introduced=1, introduced_at=COALESCE(introduced_at,?1)
                 WHERE workbook_id=?2 AND note_id=?3",
                params![
                    format_ts(write.attempt.answered_at),
                    write.attempt.workbook_id,
                    write.attempt.note_id
                ],
            )?;
        }
        if let Some(until) = write.bury_until {
            transaction.execute(
                "UPDATE questions SET buried_until=?1
                 WHERE workbook_id=?2 AND note_id=?3 AND question_id<>?4",
                params![
                    format_ts(until),
                    write.attempt.workbook_id,
                    write.attempt.note_id,
                    write.attempt.question_id
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn correct_attempt(&mut self, write: CorrectionWrite) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        if let Some(card) = write.card.as_ref() {
            upsert_review_state(
                &transaction,
                &write.attempt.workbook_id,
                &write.attempt.question_id,
                card,
            )?;
        }
        transaction.execute(
            "UPDATE attempts SET correct=?1,unknown=?2,rating=?3,rating_source=?4,after_state_json=?5,
                    state_revision=?6,assist_json=?7 WHERE id=?8",
            params![
                write.attempt.correct.map(i64::from),
                i64::from(write.attempt.unknown),
                write.attempt.rating,
                write.attempt.rating_source.as_str(),
                write.attempt.after_state_json,
                write.attempt.state_revision as i64,
                write.attempt.assist_json,
                write.attempt.id
            ],
        )?;
        update_session_row(&transaction, &write.session)?;
        transaction.commit()?;
        Ok(())
    }

    // ---- backups --------------------------------------------------------

    pub fn backup_to(&self, destination: &Path) -> Result<()> {
        let connection = self.connection()?;
        if let Some(live) = self.path.as_ref() {
            let destination_abs = normalize(destination);
            for candidate in [
                live.clone(),
                PathBuf::from(format!("{}-wal", live.display())),
                PathBuf::from(format!("{}-shm", live.display())),
            ] {
                if normalize(&candidate) == destination_abs {
                    return Err(StorageError::Conflict(
                        "使用中のデータベースには書き込めません".to_string(),
                    ));
                }
            }
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = destination.with_extension(format!("{}.partial", Uuid::new_v4()));
        if temp.exists() {
            fs::remove_file(&temp)?;
        }
        let result = (|| -> Result<()> {
            connection.backup("main", &temp, None::<fn(rusqlite::backup::Progress)>)?;
            set_private_file(&temp)?;
            sync_file(&temp)?;
            fs::rename(&temp, destination)?;
            if let Some(parent) = destination.parent() {
                sync_dir(parent);
            }
            Ok(())
        })();
        if temp.exists() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    /// Creates a uniquely named safety backup and refuses to reuse an existing
    /// path. The empty reservation file is created with `create_new`, so two
    /// imports at the same millisecond cannot overwrite one another.
    pub fn backup_to_unique(&self, directory: &Path, prefix: &str) -> Result<PathBuf> {
        ensure_private_dir(directory)?;
        for _ in 0..32 {
            let stamp = Utc::now().timestamp_millis();
            let unique = Uuid::new_v4().simple().to_string();
            let path = directory.join(format!("{prefix}-{stamp}-{}.sqlite", &unique[..12]));
            let reservation = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            drop(reservation);
            match self.backup_to(&path) {
                Ok(()) => return Ok(path),
                Err(error) => {
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
            }
        }
        Err(StorageError::Conflict(
            "一意なバックアップ名を作成できませんでした".to_string(),
        ))
    }

    pub fn daily_backup(
        &self,
        calendar: &StudyCalendar,
        now: DateTime<Utc>,
        retain: usize,
    ) -> Result<Option<PathBuf>> {
        let settings = self.settings()?;
        if !settings.auto_backup {
            return Ok(None);
        }
        let directory = self.data_dir.join("backups");
        ensure_private_dir(&directory)?;
        let day = calendar.learning_day(now);
        let path = directory.join(format!("daily-{day}.sqlite"));
        if !path.exists() {
            self.backup_to(&path)?;
        }
        let mut files = self.backup_files()?;
        if files.len() > retain {
            let remove = files.len() - retain;
            files.sort();
            for old in files.into_iter().take(remove) {
                let _ = fs::remove_file(old);
            }
        }
        Ok(Some(path))
    }

    pub fn backup_files(&self) -> Result<Vec<PathBuf>> {
        let directory = self.data_dir.join("backups");
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut output = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("daily-") && name.ends_with(".sqlite") {
                output.push(entry.path());
            }
        }
        output.sort();
        Ok(output)
    }

    pub fn validate_backup(path: &Path) -> Result<()> {
        if !path.is_file() {
            return Err(StorageError::NotFound(path.display().to_string()));
        }
        if fs::metadata(path)?.len() > 500 * 1024 * 1024 {
            return Err(StorageError::Unsupported(
                "バックアップが500MBを超えています".to_string(),
            ));
        }
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        let application_id = pragma_i64(&connection, "application_id")?;
        if application_id != APP_ID {
            return Err(StorageError::Unsupported(
                "Workbook 2のバックアップではありません".to_string(),
            ));
        }
        let version = pragma_i64(&connection, "user_version")?;
        if !(1..=SCHEMA_VERSION).contains(&version) {
            return Err(StorageError::Unsupported(format!(
                "バックアップのバージョンに対応していません: {version}"
            )));
        }
        let tables = table_names(&connection)?;
        let expected: BTreeSet<&str> = schema::expected_tables(version).iter().copied().collect();
        let actual: BTreeSet<&str> = tables.iter().map(String::as_str).collect();
        if expected != actual {
            return Err(StorageError::Unsupported(
                "バックアップのスキーマが一致しません".to_string(),
            ));
        }
        let triggers: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('trigger','view')",
            [],
            |row| row.get(0),
        )?;
        if triggers != 0 {
            return Err(StorageError::Unsupported(
                "バックアップに想定外のトリガーまたはビューがあります".to_string(),
            ));
        }
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(StorageError::Unsupported(format!(
                "バックアップの整合性チェックに失敗しました: {integrity}"
            )));
        }
        let fk: i64 =
            connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if fk != 0 {
            return Err(StorageError::Unsupported(
                "バックアップの外部キーに不整合があります".to_string(),
            ));
        }
        let mut statement = connection.prepare("SELECT local_id,content_json FROM workbooks")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (local_id, content_json) = row?;
            let workbook = parse_workbook(&content_json)
                .map_err(|error| StorageError::Validation(error.to_string()))?;
            if version >= 3 {
                let mut asset_statement =
                    connection.prepare("SELECT asset_id FROM assets WHERE workbook_id=?1")?;
                let stored: BTreeSet<String> = asset_statement
                    .query_map([&local_id], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<_, _>>()?;
                let required: BTreeSet<String> = workbook
                    .assets
                    .iter()
                    .map(|asset| asset.id.clone())
                    .collect();
                if stored != required {
                    return Err(StorageError::Validation(format!(
                        "教材 {local_id} の画像アセットが一致しません"
                    )));
                }
            }
        }
        if version >= 3 {
            let mut statement =
                connection.prepare("SELECT media_type,width,height,bytes FROM assets")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?;
            for row in rows {
                let (media_type, width, height, bytes) = row?;
                if bytes.len() > workbook_domain::MAX_ASSET_BYTES {
                    return Err(StorageError::Validation(
                        "画像アセットがサイズ上限を超えています".to_string(),
                    ));
                }
                let (actual_width, actual_height) =
                    workbook_domain::inspect_image(&bytes, &media_type, "assets")
                        .map_err(|error| StorageError::Validation(error.to_string()))?;
                if actual_width != width as u32 || actual_height != height as u32 {
                    return Err(StorageError::Validation(
                        "画像アセットのサイズ情報が一致しません".to_string(),
                    ));
                }
            }
        }
        let mut statement = connection.prepare("SELECT data FROM settings")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            Settings::from_json(&row?)
                .map_err(|error| StorageError::Validation(error.to_string()))?;
        }
        if version >= 3 {
            let mut statement = connection.prepare(
                "SELECT assist_json FROM attempts WHERE assist_json IS NOT NULL
                 UNION ALL
                 SELECT assist_json FROM sessions WHERE assist_json IS NOT NULL",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                serde_json::from_str::<AssistState>(&row?)
                    .map_err(|error| StorageError::Validation(format!("assist_json: {error}")))?;
            }
        }
        if version >= 4 {
            let sql = if version >= 5 {
                "SELECT id,title,description,target_date,target_score,daily_minutes,scope_json,
                        created_at,updated_at,archived_at FROM exams"
            } else {
                // v4 backups predate `daily_minutes`; validate them with the
                // same default that the forward migration will apply.
                "SELECT id,title,description,target_date,target_score,30,scope_json,
                        created_at,updated_at,archived_at FROM exams"
            };
            let mut statement = connection.prepare(sql)?;
            let rows = statement.query_map([], map_exam)?;
            for row in rows {
                let exam = row.map_err(|error| StorageError::Validation(error.to_string()))?;
                exam.validate()
                    .map_err(|error| StorageError::Validation(error.to_string()))?;
            }
            let mut statement = connection.prepare(
                "SELECT session_id,exam_id,submitted_at,total,correct,incorrect,unanswered,
                        ungraded,duration_ms FROM mock_results",
            )?;
            let rows = statement.query_map([], map_mock_result)?;
            for row in rows {
                let result = row.map_err(|error| StorageError::Validation(error.to_string()))?;
                if result.correct + result.incorrect + result.ungraded > result.total {
                    return Err(StorageError::Validation(
                        "模試結果の件数が不正です".to_string(),
                    ));
                }
            }
        }
        let mut statement = connection.prepare(
            "SELECT state,stability,stability_fast,difficulty,due_at,last_review_at,reps,lapses,
                    scheduled_days,learning_step,algorithm,policy_version,revision FROM review_states",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, i64>(12)?,
            ))
        })?;
        for row in rows {
            map_stored_card(row?)?;
        }
        Ok(())
    }

    pub fn restore_from(&mut self, source: &Path) -> Result<()> {
        let live = self
            .path
            .clone()
            .ok_or_else(|| StorageError::Conflict("メモリDBは復元できません".to_string()))?;
        if normalize(source) == normalize(&live) {
            return Err(StorageError::Conflict(
                "使用中のファイルからは復元できません".to_string(),
            ));
        }
        Self::validate_backup(source)?;
        let directory = self.data_dir.join("backups");
        ensure_private_dir(&directory)?;
        self.backup_to_unique(&directory, "pre-restore")?;
        let old = live.with_extension("pre-restore-old.sqlite");
        let staging = live.with_extension(format!("restore-{}.sqlite", Uuid::new_v4()));
        fs::copy(source, &staging)?;
        set_private_file(&staging)?;
        if let Some(connection) = self.conn.take() {
            connection
                .close()
                .map_err(|(_, error)| StorageError::Sqlite(error))?;
        }
        if old.exists() {
            fs::remove_file(&old)?;
        }
        let outcome = (|| -> Result<()> {
            fs::rename(&live, &old)?;
            for suffix in ["-wal", "-shm"] {
                let sidecar = PathBuf::from(format!("{}{suffix}", live.display()));
                if sidecar.exists() {
                    fs::remove_file(sidecar)?;
                }
            }
            fs::rename(&staging, &live)?;
            let mut connection = open_connection(&live, false)?;
            let version = pragma_i64(&connection, "user_version")?;
            if version < SCHEMA_VERSION {
                migrate_connection(&mut connection, &directory, version)?;
            }
            self.conn = Some(connection);
            let integrity = self.integrity()?;
            if integrity != "ok" {
                return Err(StorageError::Unsupported(format!(
                    "復元後の整合性チェックに失敗しました: {integrity}"
                )));
            }
            Ok(())
        })();
        match outcome {
            Ok(()) => {
                let _ = fs::remove_file(&old);
                Ok(())
            }
            Err(error) => {
                if let Some(connection) = self.conn.take() {
                    let _ = connection.close();
                }
                let _ = fs::remove_file(&live);
                if old.exists() {
                    let _ = fs::rename(&old, &live);
                }
                self.conn = Some(open_connection(&live, false)?);
                Err(error)
            }
        }
    }

    // ---- legacy migration ----------------------------------------------

    pub fn apply_legacy_plan(&mut self, plan: &LegacyPlan) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let active: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sessions WHERE status='active'",
            [],
            |row| row.get(0),
        )?;
        if active > 0
            && plan
                .sessions
                .iter()
                .any(|session| session.status == SessionStatus::Active)
        {
            return Err(StorageError::Conflict(
                "学習中のセッションがあるため、旧データのアクティブセッションを移行できません。先に現在のセッションを終了してください。".to_string(),
            ));
        }
        let transaction = connection.transaction()?;
        for book in &plan.workbooks {
            transaction.execute(
                "INSERT INTO workbooks(local_id,source_id,source_revision,title,content_json,imported_at,last_used_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    book.local_id,
                    book.source_id,
                    book.source_revision,
                    book.workbook.title,
                    book.workbook.to_json(),
                    format_ts(book.imported_at),
                    book.last_used_at.map(format_ts)
                ],
            )?;
            for note in &book.notes {
                transaction.execute(
                    "INSERT INTO notes(workbook_id,note_id,introduced,introduced_at)
                     VALUES(?1,?2,?3,?4)",
                    params![
                        book.local_id,
                        note.note_id,
                        i64::from(note.introduced),
                        note.introduced.then(|| format_ts(book.imported_at))
                    ],
                )?;
            }
            for question in &book.questions {
                transaction.execute(
                    "INSERT INTO questions(workbook_id,question_id,note_id,buried_until)
                     VALUES(?1,?2,?3,?4)",
                    params![
                        book.local_id,
                        question.question_id,
                        question.note_id,
                        question.buried_until.map(format_ts)
                    ],
                )?;
            }
            for card in &book.cards {
                upsert_review_state(&transaction, &book.local_id, &card.question_id, &card.card)?;
            }
        }
        for session in &plan.sessions {
            insert_legacy_session(&transaction, session)?;
        }
        transaction.execute(
            "INSERT INTO settings(id,data) VALUES(1,?1)
             ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            [plan.settings.to_json()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn reset_history(&mut self) -> Result<()> {
        let connection = self
            .conn
            .as_mut()
            .ok_or_else(|| StorageError::Conflict("データベースが閉じています".to_string()))?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM attempts", [])?;
        transaction.execute("DELETE FROM mock_answers", [])?;
        transaction.execute("DELETE FROM mock_results", [])?;
        transaction.execute("DELETE FROM session_items", [])?;
        transaction.execute("DELETE FROM session_workbooks", [])?;
        transaction.execute("DELETE FROM sessions", [])?;
        transaction.execute("DELETE FROM review_states", [])?;
        transaction.execute("UPDATE notes SET introduced=0, introduced_at=NULL", [])?;
        transaction.execute("UPDATE questions SET buried_until=NULL", [])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn close(mut self) -> Result<()> {
        if let Some(connection) = self.conn.take() {
            connection
                .close()
                .map_err(|(_, error)| StorageError::Sqlite(error))?;
        }
        Ok(())
    }
}

fn insert_legacy_session(
    transaction: &rusqlite::Transaction<'_>,
    session: &LegacySession,
) -> Result<()> {
    let row = SessionRow {
        id: session.id.clone(),
        mode: session.mode,
        status: session.status,
        started_at: session.started_at,
        ended_at: session.ended_at,
        current_workbook_id: session.current_workbook_id.clone(),
        current_item_id: session.current_item_id.clone(),
        phase: session.phase,
        draft_json: session.draft_json.clone(),
        hint: session.hint,
        revealed: session.revealed,
        pending_correct: session.pending_correct,
        pending_unknown: session.pending_unknown,
        pending_at: session.pending_at,
        question_started_at: session.question_started_at,
        question_elapsed_ms: session.question_elapsed_ms,
        cursor_kind: session.cursor_kind.clone(),
        assist_json: None,
        revision: session.revision,
        exam_id: None,
        time_limit_seconds: None,
        time_budget_seconds: None,
    };
    insert_session(transaction, &row)?;
    for item in &session.items {
        insert_session_item(transaction, &item_to_row(&session.id, item))?;
    }
    for workbook in &session.workbooks {
        insert_session_workbook(transaction, &workbook_to_row(&session.id, workbook))?;
    }
    for attempt in &session.attempts {
        insert_attempt(transaction, &legacy_attempt_to_row(&session.id, attempt))?;
    }
    Ok(())
}

fn item_to_row(session_id: &str, item: &LegacySessionItem) -> SessionItemRow {
    SessionItemRow {
        id: item.id.clone(),
        session_id: session_id.to_string(),
        ordinal: item.ordinal,
        workbook_id: item.workbook_id.clone(),
        note_id: item.note_id.clone(),
        question_id: item.question_id.clone(),
        status: item.status,
    }
}

fn workbook_to_row(session_id: &str, workbook: &LegacySessionWorkbook) -> SessionWorkbookRow {
    SessionWorkbookRow {
        session_id: session_id.to_string(),
        workbook_id: workbook.workbook_id.clone(),
        ordinal: workbook.ordinal,
        result_ack: workbook.result_ack,
    }
}

fn legacy_attempt_to_row(session_id: &str, attempt: &LegacyAttempt) -> AttemptRow {
    AttemptRow {
        id: attempt.id.clone(),
        session_id: session_id.to_string(),
        workbook_id: attempt.workbook_id.clone(),
        note_id: attempt.note_id.clone(),
        question_id: attempt.question_id.clone(),
        answered_at: attempt.answered_at,
        correct: attempt.correct,
        unknown: attempt.unknown,
        hint: attempt.hint,
        elapsed_ms: attempt.elapsed_ms,
        rating: attempt.rating,
        rating_source: RatingSource::Manual,
        answer_json: attempt.answer_json.clone(),
        before_state_json: attempt.before_state_json.clone(),
        after_state_json: attempt.after_state_json.clone(),
        state_revision: attempt.state_revision,
        short_term: attempt.short_term,
        assist_json: None,
        created_at: attempt.created_at,
    }
}

fn insert_session(transaction: &rusqlite::Transaction<'_>, session: &SessionRow) -> Result<()> {
    transaction.execute(
        "INSERT INTO sessions(id,mode,status,started_at,ended_at,current_workbook_id,current_item_id,
                phase,draft_json,hint,revealed,pending_correct,pending_unknown,pending_at,
                question_started_at,question_elapsed_ms,cursor_kind,assist_json,revision,
                exam_id,time_limit_seconds,time_budget_seconds)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
        params![
            session.id,
            session.mode.as_str(),
            match session.status {
                SessionStatus::Active => "active",
                SessionStatus::Ended => "ended",
            },
            format_ts(session.started_at),
            session.ended_at.map(format_ts),
            session.current_workbook_id,
            session.current_item_id,
            session.phase.as_str(),
            session.draft_json,
            i64::from(session.hint),
            i64::from(session.revealed),
            session.pending_correct.map(i64::from),
            i64::from(session.pending_unknown),
            session.pending_at.map(format_ts),
            session.question_started_at.map(format_ts),
            session.question_elapsed_ms,
            session.cursor_kind,
            session.assist_json,
            session.revision as i64,
            session.exam_id,
            session.time_limit_seconds.map(i64::from),
            session.time_budget_seconds.map(i64::from)
        ],
    )?;
    Ok(())
}

fn update_session_row(connection: &Connection, session: &SessionRow) -> Result<()> {
    connection.execute(
        "UPDATE sessions SET mode=?1,status=?2,started_at=?3,ended_at=?4,current_workbook_id=?5,
                current_item_id=?6,phase=?7,draft_json=?8,hint=?9,revealed=?10,pending_correct=?11,
                pending_unknown=?12,pending_at=?13,question_started_at=?14,question_elapsed_ms=?15,
                cursor_kind=?16,assist_json=?17,revision=?18,exam_id=?19,time_limit_seconds=?20,
                time_budget_seconds=?21
                WHERE id=?22",
        params![
            session.mode.as_str(),
            match session.status {
                SessionStatus::Active => "active",
                SessionStatus::Ended => "ended",
            },
            format_ts(session.started_at),
            session.ended_at.map(format_ts),
            session.current_workbook_id,
            session.current_item_id,
            session.phase.as_str(),
            session.draft_json,
            i64::from(session.hint),
            i64::from(session.revealed),
            session.pending_correct.map(i64::from),
            i64::from(session.pending_unknown),
            session.pending_at.map(format_ts),
            session.question_started_at.map(format_ts),
            session.question_elapsed_ms,
            session.cursor_kind,
            session.assist_json,
            session.revision as i64,
            session.exam_id,
            session.time_limit_seconds.map(i64::from),
            session.time_budget_seconds.map(i64::from),
            session.id
        ],
    )?;
    Ok(())
}

fn insert_session_item(
    transaction: &rusqlite::Transaction<'_>,
    item: &SessionItemRow,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO session_items(id,session_id,ordinal,workbook_id,note_id,question_id,status)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            item.id,
            item.session_id,
            item.ordinal,
            item.workbook_id,
            item.note_id,
            item.question_id,
            item.status.as_str()
        ],
    )?;
    Ok(())
}

fn insert_session_workbook(
    transaction: &rusqlite::Transaction<'_>,
    workbook: &SessionWorkbookRow,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO session_workbooks(session_id,workbook_id,ordinal,result_ack)
         VALUES(?1,?2,?3,?4)",
        params![
            workbook.session_id,
            workbook.workbook_id,
            workbook.ordinal,
            i64::from(workbook.result_ack)
        ],
    )?;
    Ok(())
}

fn insert_attempt(transaction: &rusqlite::Transaction<'_>, attempt: &AttemptRow) -> Result<()> {
    transaction.execute(
        "INSERT INTO attempts(id,session_id,workbook_id,note_id,question_id,answered_at,correct,
                unknown,hint,elapsed_ms,rating,rating_source,answer_json,before_state_json,
                after_state_json,state_revision,short_term,assist_json,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![
            attempt.id,
            attempt.session_id,
            attempt.workbook_id,
            attempt.note_id,
            attempt.question_id,
            format_ts(attempt.answered_at),
            attempt.correct.map(i64::from),
            i64::from(attempt.unknown),
            i64::from(attempt.hint),
            attempt.elapsed_ms,
            attempt.rating,
            attempt.rating_source.as_str(),
            attempt.answer_json,
            attempt.before_state_json,
            attempt.after_state_json,
            attempt.state_revision as i64,
            i64::from(attempt.short_term),
            attempt.assist_json,
            format_ts(attempt.created_at)
        ],
    )?;
    Ok(())
}

fn upsert_review_state(
    transaction: &rusqlite::Transaction<'_>,
    workbook_id: &str,
    question_id: &str,
    card: &StoredCard,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO review_states(workbook_id,question_id,state,stability,stability_fast,difficulty,
                due_at,last_review_at,reps,lapses,scheduled_days,learning_step,algorithm,
                policy_version,revision)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(workbook_id,question_id) DO UPDATE SET
                state=excluded.state,stability=excluded.stability,
                stability_fast=excluded.stability_fast,difficulty=excluded.difficulty,
                due_at=excluded.due_at,last_review_at=excluded.last_review_at,
                reps=excluded.reps,lapses=excluded.lapses,
                scheduled_days=excluded.scheduled_days,learning_step=excluded.learning_step,
                algorithm=excluded.algorithm,policy_version=excluded.policy_version,
                revision=excluded.revision",
        params![
            workbook_id,
            question_id,
            i64::from(card.state.value()),
            f64::from(card.stability),
            f64::from(card.stability_fast),
            f64::from(card.difficulty),
            format_ts(card.due_at),
            card.last_review_at.map(format_ts),
            i64::from(card.reps),
            i64::from(card.lapses),
            card.scheduled_days,
            i64::from(card.learning_step),
            card.algorithm,
            card.policy_version,
            card.revision as i64
        ],
    )?;
    Ok(())
}

pub fn counts_for_workbook(bundle: &SessionBundle, workbook_id: Option<&str>) -> SessionCounts {
    let in_scope = |workbook: &str| workbook_id.is_none_or(|wanted| workbook == wanted);
    let items: Vec<_> = bundle
        .items
        .iter()
        .filter(|item| in_scope(&item.workbook_id))
        .collect();
    let attempts: Vec<_> = bundle
        .attempts
        .iter()
        .filter(|attempt| in_scope(&attempt.workbook_id))
        .collect();
    let initial_questions = items.len();
    let skipped = items
        .iter()
        .filter(|item| item.status == SessionItemStatus::Skipped)
        .count();
    let distinct_answered = attempts
        .iter()
        .map(|attempt| attempt.question_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let short_term_answers = attempts.iter().filter(|attempt| attempt.short_term).count();
    let initial_attempts: Vec<_> = attempts
        .iter()
        .filter(|attempt| !attempt.short_term)
        .collect();
    let first_pass_graded = initial_attempts
        .iter()
        .filter(|attempt| attempt.correct.is_some())
        .count();
    let first_pass_correct = initial_attempts
        .iter()
        .filter(|attempt| attempt.correct == Some(true))
        .count();
    SessionCounts {
        initial_questions,
        distinct_answered,
        attempts: attempts.len(),
        short_term_answers,
        skipped,
        first_pass_graded,
        first_pass_correct,
    }
}

fn map_workbook_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkbookRecord> {
    Ok(WorkbookRecord {
        local_id: row.get(0)?,
        source_id: row.get(1)?,
        source_revision: row.get(2)?,
        title: row.get(3)?,
        content_json: row.get(4)?,
        imported_at: parse_ts(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
        last_used_at: row
            .get::<_, Option<String>>(6)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
    })
}

fn map_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionCandidate> {
    Ok(SessionCandidate {
        workbook_id: row.get(0)?,
        note_id: row.get(1)?,
        question_id: row.get(2)?,
        due_at: row
            .get::<_, Option<String>>(3)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
    })
}

type BudgetCandidateColumns = (String, String, String, Option<String>);

fn map_budget_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<BudgetCandidateColumns> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

type CardColumns = (
    i64,
    f64,
    f64,
    f64,
    String,
    Option<String>,
    i64,
    i64,
    f64,
    i64,
    String,
    String,
    i64,
);

fn map_stored_card(row: CardColumns) -> Result<StoredCard> {
    let (
        state,
        stability,
        stability_fast,
        difficulty,
        due_at,
        last_review_at,
        reps,
        lapses,
        scheduled_days,
        learning_step,
        algorithm,
        policy_version,
        revision,
    ) = row;
    let state = u8::try_from(state)
        .ok()
        .and_then(CardState::from_value)
        .ok_or_else(|| StorageError::Validation(format!("FSRS状態が不正です: {state}")))?;
    Ok(StoredCard {
        stability: stability as f32,
        stability_fast: stability_fast as f32,
        difficulty: difficulty as f32,
        state,
        due_at: parse_ts(&due_at)?,
        last_review_at: last_review_at.as_deref().map(parse_ts).transpose()?,
        reps: u32::try_from(reps)
            .map_err(|_| StorageError::Validation("repsが不正です".to_string()))?,
        lapses: u32::try_from(lapses)
            .map_err(|_| StorageError::Validation("lapsesが不正です".to_string()))?,
        scheduled_days,
        learning_step: u32::try_from(learning_step)
            .map_err(|_| StorageError::Validation("learning_stepが不正です".to_string()))?,
        algorithm,
        policy_version,
        revision: u64::try_from(revision)
            .map_err(|_| StorageError::Validation("revisionが不正です".to_string()))?,
    })
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let mode: String = row.get(1)?;
    let status: String = row.get(2)?;
    let phase: String = row.get(7)?;
    Ok(SessionRow {
        id: row.get(0)?,
        mode: Mode::parse(&mode).unwrap_or(Mode::New),
        status: match status.as_str() {
            "ended" => SessionStatus::Ended,
            _ => SessionStatus::Active,
        },
        started_at: parse_ts(&row.get::<_, String>(3)?).map_err(to_sql_error)?,
        ended_at: row
            .get::<_, Option<String>>(4)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
        current_workbook_id: row.get(5)?,
        current_item_id: row.get(6)?,
        phase: Phase::parse(&phase).unwrap_or(Phase::Question),
        draft_json: row.get(8)?,
        hint: row.get::<_, i64>(9)? != 0,
        revealed: row.get::<_, i64>(10)? != 0,
        pending_correct: row.get::<_, Option<i64>>(11)?.map(|value| value != 0),
        pending_unknown: row.get::<_, i64>(12)? != 0,
        pending_at: row
            .get::<_, Option<String>>(13)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
        question_started_at: row
            .get::<_, Option<String>>(14)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
        question_elapsed_ms: row.get(15)?,
        cursor_kind: row.get(16)?,
        assist_json: row.get(17)?,
        revision: row.get::<_, i64>(18)? as u64,
        exam_id: row.get(19)?,
        time_limit_seconds: row
            .get::<_, Option<i64>>(20)?
            .and_then(|value| u32::try_from(value).ok()),
        time_budget_seconds: row
            .get::<_, Option<i64>>(21)?
            .and_then(|value| u32::try_from(value).ok()),
    })
}

fn map_topic(row: &rusqlite::Row<'_>) -> rusqlite::Result<Topic> {
    Ok(Topic {
        id: row.get(0)?,
        name: row.get(1)?,
        parent_id: row.get(2)?,
        description: row.get(3)?,
    })
}

/// Resolves one authored path segment inside an import transaction, creating
/// the topic when it does not exist under that parent. Returning the existing
/// id is what makes topics shared across workbooks without being merged by
/// name alone.
fn ensure_topic(
    transaction: &rusqlite::Transaction<'_>,
    name: &str,
    parent_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<String> {
    let name_key = normalize_topic_name(name);
    let existing: Option<String> = transaction
        .query_row(
            "SELECT id FROM topics
             WHERE IFNULL(parent_id,'')=IFNULL(?1,'') AND name_key=?2",
            params![parent_id, name_key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO topics(id,name,name_key,parent_id,description,created_at,updated_at)
         VALUES(?1,?2,?3,?4,'',?5,?6)",
        params![
            id,
            name,
            name_key,
            parent_id,
            format_ts(now),
            format_ts(now)
        ],
    )?;
    Ok(id)
}

fn ensure_topic_parent(
    connection: &Connection,
    parent_id: &str,
    updating: Option<&str>,
) -> Result<()> {
    let mut current = Some(parent_id.to_string());
    let mut depth = 0usize;
    while let Some(id) = current {
        depth += 1;
        if depth > workbook_domain::MAX_TOPIC_DEPTH {
            return Err(StorageError::Validation(
                "Topicの階層が深すぎます".to_string(),
            ));
        }
        if updating == Some(id.as_str()) {
            return Err(StorageError::Validation(
                "子Topicを親にはできません".to_string(),
            ));
        }
        current = match connection
            .query_row("SELECT parent_id FROM topics WHERE id=?1", [&id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
        {
            Some(parent) => parent,
            None => {
                return Err(StorageError::NotFound(format!("Topic {id}")));
            }
        };
    }
    Ok(())
}

fn ensure_no_sibling(
    connection: &Connection,
    parent_id: Option<&str>,
    name_key: &str,
    excluding: Option<&str>,
) -> Result<()> {
    let existing: Option<String> = connection
        .query_row(
            "SELECT id FROM topics
             WHERE IFNULL(parent_id,'')=IFNULL(?1,'') AND name_key=?2
               AND (?3 IS NULL OR id<>?3)",
            params![parent_id, name_key, excluding],
            |row| row.get(0),
        )
        .optional()?;
    if existing.is_some() {
        return Err(StorageError::Conflict(
            "同じ親の下に同名のTopicがあります".to_string(),
        ));
    }
    Ok(())
}

fn map_exam(row: &rusqlite::Row<'_>) -> rusqlite::Result<Exam> {
    let target_date: Option<String> = row.get(3)?;
    let scope_json: String = row.get(6)?;
    Ok(Exam {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        target_date: target_date
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|_| {
                to_sql_error(StorageError::Validation(
                    "target_dateが不正です".to_string(),
                ))
            })?,
        target_score: row.get(4)?,
        daily_minutes: row.get::<_, i64>(5)?.clamp(0, 1_440) as u32,
        scope: serde_json::from_str::<ExamScope>(&scope_json)
            .map_err(StorageError::from)
            .map_err(to_sql_error)?,
        created_at: parse_ts(&row.get::<_, String>(7)?).map_err(to_sql_error)?,
        updated_at: parse_ts(&row.get::<_, String>(8)?).map_err(to_sql_error)?,
        archived_at: row
            .get::<_, Option<String>>(9)?
            .as_deref()
            .map(parse_ts)
            .transpose()
            .map_err(to_sql_error)?,
    })
}

fn map_mock_answer(row: &rusqlite::Row<'_>) -> rusqlite::Result<MockAnswerRow> {
    Ok(MockAnswerRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        item_id: row.get(2)?,
        workbook_id: row.get(3)?,
        note_id: row.get(4)?,
        question_id: row.get(5)?,
        answered_at: parse_ts(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
        correct: row.get::<_, Option<i64>>(7)?.map(|value| value != 0),
        answer_json: row.get(8)?,
        created_at: parse_ts(&row.get::<_, String>(9)?).map_err(to_sql_error)?,
    })
}

fn map_mock_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<MockResultRow> {
    Ok(MockResultRow {
        session_id: row.get(0)?,
        exam_id: row.get(1)?,
        submitted_at: parse_ts(&row.get::<_, String>(2)?).map_err(to_sql_error)?,
        total: row.get::<_, i64>(3)? as u32,
        correct: row.get::<_, i64>(4)? as u32,
        incorrect: row.get::<_, i64>(5)? as u32,
        unanswered: row.get::<_, i64>(6)? as u32,
        ungraded: row.get::<_, i64>(7)? as u32,
        duration_ms: row.get(8)?,
    })
}

fn map_session_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionItemRow> {
    let status: String = row.get(6)?;
    Ok(SessionItemRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        ordinal: row.get::<_, i64>(2)? as u32,
        workbook_id: row.get(3)?,
        note_id: row.get(4)?,
        question_id: row.get(5)?,
        status: SessionItemStatus::parse(&status).unwrap_or(SessionItemStatus::Pending),
    })
}

fn map_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttemptRow> {
    Ok(AttemptRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        workbook_id: row.get(2)?,
        note_id: row.get(3)?,
        question_id: row.get(4)?,
        answered_at: parse_ts(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
        correct: row.get::<_, Option<i64>>(6)?.map(|value| value != 0),
        unknown: row.get::<_, i64>(7)? != 0,
        hint: row.get::<_, i64>(8)? != 0,
        elapsed_ms: row.get(9)?,
        rating: row.get::<_, i64>(10)? as u8,
        rating_source: RatingSource::parse(&row.get::<_, String>(11)?)
            .unwrap_or(RatingSource::Manual),
        answer_json: row.get(12)?,
        before_state_json: row.get(13)?,
        after_state_json: row.get(14)?,
        state_revision: row.get::<_, i64>(15)? as u64,
        short_term: row.get::<_, i64>(16)? != 0,
        assist_json: row.get(17)?,
        created_at: parse_ts(&row.get::<_, String>(18)?).map_err(to_sql_error)?,
    })
}

fn open_connection(path: &Path, wal: bool) -> Result<Connection> {
    let connection = Connection::open(path)?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    set_private_file(path)?;
    let mut pragmas = String::from(
        "PRAGMA trusted_schema=OFF;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         PRAGMA synchronous=FULL;",
    );
    if wal {
        pragmas.push_str(" PRAGMA journal_mode=WAL;");
    }
    connection.execute_batch(&pragmas)?;
    Ok(connection)
}

/// Applies forward-only schema migrations. A backup is written with the
/// SQLite backup API before the first change so a failed upgrade can be
/// inspected or restored without touching the live file directly.
fn migrate_connection(connection: &mut Connection, backups: &Path, from: i64) -> Result<()> {
    if from >= SCHEMA_VERSION {
        return Ok(());
    }
    ensure_private_dir(backups)?;
    let backup = backups.join(format!(
        "pre-migration-v{from}-{}-{}.sqlite",
        Utc::now().timestamp_millis(),
        &Uuid::new_v4().simple().to_string()[..12]
    ));
    let temp = backup.with_extension(format!("{}.partial", Uuid::new_v4()));
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let result = (|| -> Result<()> {
        connection.backup("main", &temp, None::<fn(rusqlite::backup::Progress)>)?;
        set_private_file(&temp)?;
        sync_file(&temp)?;
        fs::rename(&temp, &backup)?;
        sync_dir(backups);
        Ok(())
    })();
    if temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    result?;

    let transaction = connection.transaction()?;
    if from < 2 {
        transaction.execute_batch(
            "ALTER TABLE attempts ADD COLUMN rating_source TEXT NOT NULL DEFAULT 'manual';",
        )?;
    }
    if from < 3 {
        transaction.execute_batch(
            "ALTER TABLE attempts ADD COLUMN assist_json TEXT;
             ALTER TABLE sessions ADD COLUMN assist_json TEXT;
             CREATE TABLE assets(
               workbook_id TEXT NOT NULL REFERENCES workbooks(local_id) ON DELETE CASCADE,
               asset_id TEXT NOT NULL,
               media_type TEXT NOT NULL,
               alt TEXT NOT NULL,
               width INTEGER NOT NULL,
               height INTEGER NOT NULL,
               bytes BLOB NOT NULL,
               PRIMARY KEY(workbook_id, asset_id)
             );",
        )?;
    }
    if from < 4 {
        transaction.execute_batch(
            "CREATE TABLE exams(
               id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               description TEXT,
               target_date TEXT,
               target_score REAL,
               scope_json TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               archived_at TEXT
             );
             CREATE INDEX exams_archive ON exams(archived_at, target_date);
             ALTER TABLE sessions ADD COLUMN exam_id TEXT REFERENCES exams(id) ON DELETE SET NULL;
             ALTER TABLE sessions ADD COLUMN time_limit_seconds INTEGER;
             CREATE TABLE mock_answers(
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               item_id TEXT NOT NULL UNIQUE REFERENCES session_items(id) ON DELETE CASCADE,
               workbook_id TEXT NOT NULL,
               note_id TEXT NOT NULL,
               question_id TEXT NOT NULL,
               answered_at TEXT NOT NULL,
               correct INTEGER,
               answer_json TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE INDEX mock_answers_session ON mock_answers(session_id, answered_at);
             CREATE TABLE mock_results(
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               exam_id TEXT NOT NULL REFERENCES exams(id) ON DELETE CASCADE,
               submitted_at TEXT NOT NULL,
               total INTEGER NOT NULL,
               correct INTEGER NOT NULL,
               incorrect INTEGER NOT NULL,
               unanswered INTEGER NOT NULL,
               ungraded INTEGER NOT NULL,
               duration_ms INTEGER NOT NULL
             );
             CREATE INDEX mock_results_exam ON mock_results(exam_id, submitted_at DESC);",
        )?;
    }
    if from < 5 {
        transaction.execute_batch(
            "ALTER TABLE exams ADD COLUMN daily_minutes INTEGER NOT NULL DEFAULT 30;",
        )?;
    }
    if from < 6 {
        transaction
            .execute_batch("ALTER TABLE sessions ADD COLUMN time_budget_seconds INTEGER;")?;
    }
    if from < 7 {
        transaction.execute_batch(
            "CREATE TABLE topics(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               name_key TEXT NOT NULL,
               parent_id TEXT REFERENCES topics(id) ON DELETE RESTRICT,
               description TEXT NOT NULL DEFAULT '',
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE UNIQUE INDEX topics_parent_name ON topics(IFNULL(parent_id,''), name_key);
             CREATE INDEX topics_parent ON topics(parent_id);
             CREATE TABLE question_topics(
               workbook_id TEXT NOT NULL,
               question_id TEXT NOT NULL,
               topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
               PRIMARY KEY(workbook_id, question_id, topic_id),
               FOREIGN KEY(workbook_id, question_id)
                 REFERENCES questions(workbook_id, question_id) ON DELETE CASCADE
             );
             CREATE INDEX question_topics_topic
               ON question_topics(topic_id, workbook_id, question_id);",
        )?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn pragma_i64(connection: &Connection, name: &str) -> Result<i64> {
    Ok(connection.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?)
}

fn table_names(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut output = Vec::new();
    for row in rows {
        output.push(row?);
    }
    Ok(output)
}

pub fn format_ts(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_ts(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| StorageError::Validation(format!("日時が不正です: {value}")))
}

fn to_sql_error(error: StorageError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.write_all(&[])?;
    file.sync_all()?;
    Ok(())
}

fn sync_dir(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

fn normalize(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// Default settings are intentionally imported here so a newly opened DB has
// no hidden state; the service persists settings on first change.
#[allow(dead_code)]
fn default_settings() -> Settings {
    Settings {
        theme: ThemeSetting::System,
        ..Settings::default()
    }
}
