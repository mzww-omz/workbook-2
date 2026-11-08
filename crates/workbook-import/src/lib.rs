//! Read-only inspection and migration planning for the legacy Electron
//! Workbook 0.1.0 database (SQLite application_id 1463962443, user_version 1).
//!
//! Nothing here writes to the legacy database. The source is copied into a
//! private staging directory together with its WAL/SHM sidecars, opened
//! read-only, verified, and converted into a `LegacyPlan` that the storage
//! layer applies in one transaction.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;
use thiserror::Error;
use uuid::Uuid;
use workbook_domain::{
    AssetContent, MAX_JSON_BYTES, Mode, Phase, SessionItemStatus, SessionStatus, Settings,
    StoredCard, ThemeSetting, Workbook, inspect_image, parse_workbook,
};
use workbook_scheduler::{ALGORITHM, POLICY_VERSION};

pub const LEGACY_APPLICATION_ID: i64 = 1_463_962_443;
pub const MAX_LEGACY_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("旧Workbookデータを開けません: {0}")]
    Io(#[from] std::io::Error),
    #[error("旧WorkbookのSQLiteを読めません: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("旧Workbookの内容が不正です: {0}")]
    Invalid(String),
    #[error("教材JSONが不正です: {0}")]
    Workbook(String),
    #[error("JSONの読み取りに失敗しました: {0}")]
    Json(#[from] serde_json::Error),
}

/// Reads a workbook JSON file and resolves bundled image assets relative to
/// its directory. External file paths must stay inside the workbook folder
/// after canonicalization; this blocks `..`, absolute paths, and symlink
/// escapes. Embedded base64 assets are decoded here.
pub fn load_workbook_file(path: &Path) -> Result<(Workbook, Vec<AssetContent>), ImportError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_JSON_BYTES as u64 {
        return Err(ImportError::Workbook(format!(
            "JSONは{}MB以下にしてください",
            MAX_JSON_BYTES / (1024 * 1024)
        )));
    }
    let raw = fs::read_to_string(path)?;
    let workbook =
        parse_workbook(&raw).map_err(|error| ImportError::Workbook(error.to_string()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let base_canonical = base.canonicalize()?;
    let mut assets = Vec::new();
    let mut total_bytes = 0usize;
    for asset in &workbook.assets {
        let bytes = if let Some(bytes) = asset
            .embedded_bytes()
            .map_err(|error| ImportError::Workbook(format!("assets.{}: {error}", asset.id)))?
        {
            bytes
        } else {
            let file = asset.file.as_deref().ok_or_else(|| {
                ImportError::Workbook(format!("assets.{}: 画像パスがありません", asset.id))
            })?;
            let candidate = base.join(file);
            let canonical = candidate.canonicalize().map_err(|_| {
                ImportError::Workbook(format!("assets.{}: 画像が見つかりません", asset.id))
            })?;
            if !canonical.starts_with(&base_canonical) {
                return Err(ImportError::Workbook(format!(
                    "assets.{}: 教材フォルダの外を参照しています",
                    asset.id
                )));
            }
            let metadata = fs::metadata(&canonical)?;
            if metadata.len() > workbook_domain::MAX_ASSET_BYTES as u64 {
                return Err(ImportError::Workbook(format!(
                    "assets.{}: 画像は{}MB以下にしてください",
                    asset.id,
                    workbook_domain::MAX_ASSET_BYTES / (1024 * 1024)
                )));
            }
            fs::read(&canonical)?
        };
        let (width, height) =
            inspect_image(&bytes, &asset.media_type, &format!("assets.{}", asset.id))
                .map_err(|error| ImportError::Workbook(error.to_string()))?;
        total_bytes += bytes.len();
        if total_bytes > workbook_domain::MAX_TOTAL_ASSET_BYTES {
            return Err(ImportError::Workbook(
                "画像アセットの合計サイズが上限を超えています".to_string(),
            ));
        }
        assets.push(AssetContent {
            id: asset.id.clone(),
            alt: asset.alt.clone(),
            media_type: asset.media_type.clone(),
            width,
            height,
            bytes,
        });
    }
    Ok((workbook, assets))
}

#[derive(Clone, Debug)]
pub struct LegacyPreview {
    pub schema_version: i64,
    pub workbook_count: usize,
    pub note_count: usize,
    pub question_count: usize,
    pub review_state_count: usize,
    pub attempt_count: usize,
    pub session_count: usize,
    pub active_session: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LegacyPlan {
    pub schema_version: i64,
    pub settings: Settings,
    pub workbooks: Vec<LegacyWorkbook>,
    pub sessions: Vec<LegacySession>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LegacyWorkbook {
    pub local_id: String,
    pub source_id: String,
    pub source_revision: i64,
    pub workbook: Workbook,
    pub imported_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub notes: Vec<LegacyNote>,
    pub questions: Vec<LegacyQuestion>,
    pub cards: Vec<LegacyCardRow>,
}

#[derive(Clone, Debug)]
pub struct LegacyNote {
    pub note_id: String,
    pub introduced: bool,
}

#[derive(Clone, Debug)]
pub struct LegacyQuestion {
    pub note_id: String,
    pub question_id: String,
    pub buried_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct LegacyCardRow {
    pub question_id: String,
    pub card: StoredCard,
}

#[derive(Clone, Debug)]
pub struct LegacySession {
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
    pub revision: u64,
    pub items: Vec<LegacySessionItem>,
    pub workbooks: Vec<LegacySessionWorkbook>,
    pub attempts: Vec<LegacyAttempt>,
}

#[derive(Clone, Debug)]
pub struct LegacySessionItem {
    pub id: String,
    pub ordinal: u32,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub status: SessionItemStatus,
}

#[derive(Clone, Debug)]
pub struct LegacySessionWorkbook {
    pub workbook_id: String,
    pub ordinal: u32,
    pub result_ack: bool,
}

#[derive(Clone, Debug)]
pub struct LegacyAttempt {
    pub id: String,
    pub workbook_id: String,
    pub note_id: String,
    pub question_id: String,
    pub answered_at: DateTime<Utc>,
    pub correct: Option<bool>,
    pub unknown: bool,
    pub hint: bool,
    pub elapsed_ms: i64,
    pub rating: u8,
    pub answer_json: String,
    pub before_state_json: Option<String>,
    pub after_state_json: Option<String>,
    pub state_revision: u64,
    pub short_term: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacySettings {
    theme: String,
    new_limit: u32,
    automatic_rating: bool,
    retention: f32,
    external_images: bool,
    auto_backup: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCard {
    stability: f32,
    #[serde(default)]
    stability_fast: Option<f32>,
    difficulty: f32,
    state: u8,
    due_at: String,
    last_review_at: Option<String>,
    #[serde(default)]
    reps: u32,
    #[serde(default)]
    lapses: u32,
    #[serde(default)]
    scheduled_days: f64,
    #[serde(default)]
    learning_step: u32,
    #[serde(default)]
    #[allow(dead_code)]
    card_id: Option<String>,
    #[serde(default)]
    schedule_status: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyOutcome {
    correct: Option<bool>,
    unknown: bool,
    hint: bool,
    rating: u8,
    answer: Value,
    elapsed_ms: i64,
    at: String,
    before: Option<String>,
}

pub fn inspect(path: &Path) -> Result<LegacyPreview, ImportError> {
    let plan = read_legacy(path)?;
    Ok(preview_of(&plan))
}

pub fn preview_of(plan: &LegacyPlan) -> LegacyPreview {
    LegacyPreview {
        schema_version: plan.schema_version,
        workbook_count: plan.workbooks.len(),
        note_count: plan.workbooks.iter().map(|book| book.notes.len()).sum(),
        question_count: plan.workbooks.iter().map(|book| book.questions.len()).sum(),
        review_state_count: plan.workbooks.iter().map(|book| book.cards.len()).sum(),
        attempt_count: plan
            .sessions
            .iter()
            .map(|session| session.attempts.len())
            .sum(),
        session_count: plan.sessions.len(),
        active_session: plan
            .sessions
            .iter()
            .any(|session| session.status == SessionStatus::Active),
        warnings: plan.warnings.clone(),
    }
}

pub fn read_legacy(path: &Path) -> Result<LegacyPlan, ImportError> {
    let staging = stage_source(path)?;
    let source = staging.path().join("workbook.sqlite");
    let connection = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    validate_legacy_database(&connection)?;

    let schema_version = pragma_i64(&connection, "user_version")?;
    let settings = read_settings(&connection)?;
    let mut warnings = Vec::new();

    let mut workbooks = Vec::new();
    let mut id_map: BTreeMap<String, String> = BTreeMap::new();
    let mut question_notes: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut workbook_revisions: BTreeMap<String, i64> = BTreeMap::new();

    {
        let mut statement = connection
            .prepare("SELECT id, content, imported_at, last_used FROM workbooks ORDER BY rowid")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (source_id, content, imported_at, last_used) = row?;
            let workbook = parse_workbook(&content)
                .map_err(|error| ImportError::Workbook(format!("{source_id}: {error}")))?;
            let local_id = Uuid::new_v4().to_string();
            id_map.insert(source_id.clone(), local_id.clone());
            workbook_revisions.insert(source_id.clone(), workbook.revision);
            for (note, question) in workbook.all_questions() {
                question_notes.insert((source_id.clone(), question.id.clone()), note.id.clone());
            }
            workbooks.push(LegacyWorkbook {
                local_id,
                source_id,
                source_revision: workbook.revision,
                notes: Vec::new(),
                questions: Vec::new(),
                cards: Vec::new(),
                workbook,
                imported_at: parse_required_datetime(&imported_at, "workbooks.imported_at")?,
                last_used_at: last_used
                    .as_deref()
                    .map(|value| parse_required_datetime(value, "workbooks.last_used"))
                    .transpose()?,
            });
        }
    }

    attach_notes(&connection, &mut workbooks, &id_map)?;
    attach_questions(&connection, &mut workbooks, &id_map)?;
    attach_cards(&connection, &mut workbooks, &id_map, &mut warnings)?;
    let sessions = read_sessions(
        &connection,
        &id_map,
        &question_notes,
        &workbook_revisions,
        &mut warnings,
    )?;

    let mut active = 0;
    for session in &sessions {
        if session.status == SessionStatus::Active {
            active += 1;
        }
    }
    if active > 1 {
        return Err(ImportError::Invalid(
            "旧データに複数のアクティブセッションがあります".to_string(),
        ));
    }
    if !sessions.is_empty() {
        warnings.push(
            "旧v1の履歴は1セッション・1問題につき1件のため、捏造せず1件として移行しました"
                .to_string(),
        );
    }

    Ok(LegacyPlan {
        schema_version,
        settings,
        workbooks,
        sessions,
        warnings,
    })
}

fn stage_source(path: &Path) -> Result<TempDir, ImportError> {
    let source = if path.is_dir() {
        path.join("workbook.sqlite")
    } else {
        path.to_path_buf()
    };
    if !source.is_file() {
        return Err(ImportError::Invalid(format!(
            "旧WorkbookのDBが見つかりません: {}",
            source.display()
        )));
    }
    let metadata = fs::metadata(&source)?;
    if metadata.len() > MAX_LEGACY_BYTES {
        return Err(ImportError::Invalid(
            "旧DBが500MBを超えています".to_string(),
        ));
    }
    let temp = tempfile::Builder::new()
        .prefix("workbook-legacy-")
        .tempdir()?;
    let destination = temp.path().join("workbook.sqlite");
    fs::copy(&source, &destination)?;
    set_private_file(&destination)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", source.display()));
        if sidecar.is_file() {
            let dest = PathBuf::from(format!("{}{suffix}", destination.display()));
            fs::copy(&sidecar, &dest)?;
            set_private_file(&dest)?;
        }
    }
    Ok(temp)
}

fn validate_legacy_database(connection: &Connection) -> Result<(), ImportError> {
    let application_id = pragma_i64(connection, "application_id")?;
    if application_id != LEGACY_APPLICATION_ID {
        return Err(ImportError::Invalid(format!(
            "旧WorkbookのDBではありません (application_id={application_id})"
        )));
    }
    let version = pragma_i64(connection, "user_version")?;
    if version != 1 {
        return Err(ImportError::Invalid(format!(
            "対応していない旧DBバージョンです: user_version={version} (対応: 1)"
        )));
    }
    let required = [
        "workbooks",
        "notes",
        "questions",
        "review_states",
        "sessions",
        "review_logs",
        "practice_attempts",
        "settings",
    ];
    for table in required {
        let exists: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(ImportError::Invalid(format!(
                "旧DBに {table} テーブルがありません"
            )));
        }
    }
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(ImportError::Invalid(format!(
            "旧DBの整合性チェックに失敗しました: {integrity}"
        )));
    }
    let fk_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if fk_count != 0 {
        return Err(ImportError::Invalid(
            "旧DBの外部キーに不整合があります".to_string(),
        ));
    }
    Ok(())
}

fn read_settings(connection: &Connection) -> Result<Settings, ImportError> {
    let raw: Option<String> = connection
        .query_row("SELECT data FROM settings WHERE id=1", [], |row| row.get(0))
        .optional()?;
    let Some(raw) = raw else {
        return Ok(Settings::default());
    };
    let legacy: LegacySettings = serde_json::from_str(&raw)?;
    let theme = match legacy.theme.as_str() {
        "system" => ThemeSetting::System,
        "light" => ThemeSetting::Light,
        "dark" => ThemeSetting::Dark,
        other => {
            return Err(ImportError::Invalid(format!("不明なテーマです: {other}")));
        }
    };
    let settings = Settings {
        theme,
        new_limit: legacy.new_limit,
        automatic_rating: legacy.automatic_rating,
        retention: legacy.retention,
        external_images: legacy.external_images,
        auto_backup: legacy.auto_backup,
        timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
    };
    settings
        .validate()
        .map_err(|error| ImportError::Invalid(error.0))?;
    Ok(settings)
}

fn attach_notes(
    connection: &Connection,
    workbooks: &mut [LegacyWorkbook],
    id_map: &BTreeMap<String, String>,
) -> Result<(), ImportError> {
    let mut statement =
        connection.prepare("SELECT workbook_id, id, introduced FROM notes ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (source_workbook, note_id, introduced) = row?;
        let local = id_map.get(&source_workbook).ok_or_else(|| {
            ImportError::Invalid(format!(
                "notes が未知のWorkbookを参照しています: {source_workbook}"
            ))
        })?;
        let book = workbooks
            .iter_mut()
            .find(|book| &book.local_id == local)
            .ok_or_else(|| {
                ImportError::Invalid("notes のWorkbook対応が壊れています".to_string())
            })?;
        if !book.workbook.notes.iter().any(|note| note.id == note_id) {
            return Err(ImportError::Invalid(format!(
                "notes が教材内に存在しないNoteを参照しています: {note_id}"
            )));
        }
        book.notes.push(LegacyNote {
            note_id,
            introduced: introduced != 0,
        });
    }
    Ok(())
}

fn attach_questions(
    connection: &Connection,
    workbooks: &mut [LegacyWorkbook],
    id_map: &BTreeMap<String, String>,
) -> Result<(), ImportError> {
    let mut statement = connection
        .prepare("SELECT workbook_id, note_id, id, buried_until FROM questions ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (source_workbook, note_id, question_id, buried_until) = row?;
        let local = id_map.get(&source_workbook).ok_or_else(|| {
            ImportError::Invalid(format!(
                "questions が未知のWorkbookを参照しています: {source_workbook}"
            ))
        })?;
        let book = workbooks
            .iter_mut()
            .find(|book| &book.local_id == local)
            .ok_or_else(|| {
                ImportError::Invalid("questions のWorkbook対応が壊れています".to_string())
            })?;
        if book
            .workbook
            .find_question(&note_id, &question_id)
            .is_none()
        {
            return Err(ImportError::Invalid(format!(
                "questions が教材内に存在しない問題を参照しています: {question_id}"
            )));
        }
        book.questions.push(LegacyQuestion {
            note_id,
            question_id,
            buried_until: buried_until
                .as_deref()
                .map(|value| parse_required_datetime(value, "questions.buried_until"))
                .transpose()?,
        });
    }
    Ok(())
}

fn attach_cards(
    connection: &Connection,
    workbooks: &mut [LegacyWorkbook],
    id_map: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<(), ImportError> {
    let mut statement =
        connection.prepare("SELECT workbook_id, question_id, card FROM review_states")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (source_workbook, question_id, raw) = row?;
        let local = id_map.get(&source_workbook).ok_or_else(|| {
            ImportError::Invalid(format!(
                "review_states が未知のWorkbookを参照しています: {source_workbook}"
            ))
        })?;
        let book = workbooks
            .iter_mut()
            .find(|book| &book.local_id == local)
            .ok_or_else(|| {
                ImportError::Invalid("review_states のWorkbook対応が壊れています".to_string())
            })?;
        if book
            .workbook
            .all_questions()
            .all(|(_, question)| question.id != question_id)
        {
            return Err(ImportError::Invalid(format!(
                "review_states が存在しない問題を参照しています: {question_id}"
            )));
        }
        let legacy: LegacyCard = serde_json::from_str(&raw)?;
        if legacy.state > 3 {
            return Err(ImportError::Invalid(format!(
                "FSRS状態が不正です: {question_id} state={}",
                legacy.state
            )));
        }
        if legacy.schedule_status.as_deref() == Some("suspended") {
            warnings.push(format!(
                "停止(suspended)カード {question_id} はv2では通常カードとして移行しました"
            ));
        }
        let card = StoredCard {
            stability: legacy.stability,
            stability_fast: legacy.stability_fast.unwrap_or(legacy.stability),
            difficulty: legacy.difficulty,
            state: workbook_domain::CardState::from_value(legacy.state)
                .ok_or_else(|| ImportError::Invalid("FSRS状態が不正です".to_string()))?,
            due_at: parse_required_datetime(&legacy.due_at, "review_states.card.dueAt")?,
            last_review_at: legacy
                .last_review_at
                .as_deref()
                .map(|value| parse_required_datetime(value, "review_states.card.lastReviewAt"))
                .transpose()?,
            reps: legacy.reps,
            lapses: legacy.lapses,
            scheduled_days: legacy.scheduled_days,
            learning_step: legacy.learning_step,
            algorithm: ALGORITHM.to_string(),
            policy_version: format!("legacy-ts-fsrs-6.0.0/{POLICY_VERSION}"),
            revision: 1,
        };
        book.cards.push(LegacyCardRow { question_id, card });
    }
    Ok(())
}

fn read_sessions(
    connection: &Connection,
    id_map: &BTreeMap<String, String>,
    question_notes: &BTreeMap<(String, String), String>,
    workbook_revisions: &BTreeMap<String, i64>,
    warnings: &mut Vec<String>,
) -> Result<Vec<LegacySession>, ImportError> {
    let mut attempts_by_session: BTreeMap<String, Vec<LegacyAttempt>> = BTreeMap::new();
    read_attempts(
        connection,
        "review_logs",
        id_map,
        question_notes,
        &mut attempts_by_session,
    )?;
    read_attempts(
        connection,
        "practice_attempts",
        id_map,
        question_notes,
        &mut attempts_by_session,
    )?;

    let mut sessions = Vec::new();
    let mut statement =
        connection.prepare("SELECT id, data, active FROM sessions ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (old_id, data, active) = row?;
        let value: Value = serde_json::from_str(&data)?;
        let ended_at = value
            .get("endedAt")
            .and_then(Value::as_str)
            .map(|value| parse_required_datetime(value, "sessions.endedAt"))
            .transpose()?;
        let is_active = active != 0;
        if is_active != ended_at.is_none() {
            return Err(ImportError::Invalid(format!(
                "セッション {old_id} の終了状態が一致しません"
            )));
        }
        let mode = value
            .get("mode")
            .and_then(Value::as_str)
            .and_then(Mode::parse)
            .ok_or_else(|| ImportError::Invalid(format!("セッション {old_id} のmodeが不正です")))?;
        let phase = value
            .get("phase")
            .and_then(Value::as_str)
            .and_then(Phase::parse)
            .ok_or_else(|| {
                ImportError::Invalid(format!("セッション {old_id} のphaseが不正です"))
            })?;
        let started_at = parse_required_datetime(
            value
                .get("startedAt")
                .and_then(Value::as_str)
                .ok_or_else(|| ImportError::Invalid("startedAtがありません".to_string()))?,
            "sessions.startedAt",
        )?;
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let queue = value
            .get("queue")
            .and_then(Value::as_array)
            .ok_or_else(|| ImportError::Invalid("queueがありません".to_string()))?;
        let skipped: BTreeSet<(String, String)> = value
            .get("skipped")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(split_outcome_key)
                    .collect()
            })
            .unwrap_or_default();
        let outcome_keys: BTreeSet<(String, String)> = value
            .get("outcomes")
            .and_then(Value::as_object)
            .map(|outcomes| {
                outcomes
                    .keys()
                    .filter_map(|key| split_outcome_key(key))
                    .collect()
            })
            .unwrap_or_default();

        let mut items = Vec::new();
        let mut workbooks = Vec::new();
        let mut workbook_order: BTreeMap<String, u32> = BTreeMap::new();
        let mut old_workbook_ids = BTreeSet::new();
        for (ordinal, item) in queue.iter().enumerate() {
            let old_workbook = item
                .get("workbookId")
                .and_then(Value::as_str)
                .ok_or_else(|| ImportError::Invalid("queue.workbookIdがありません".to_string()))?;
            let note_id = item
                .get("noteId")
                .and_then(Value::as_str)
                .ok_or_else(|| ImportError::Invalid("queue.noteIdがありません".to_string()))?;
            let question_id = item
                .get("questionId")
                .and_then(Value::as_str)
                .ok_or_else(|| ImportError::Invalid("queue.questionIdがありません".to_string()))?;
            let workbook_id = id_map.get(old_workbook).ok_or_else(|| {
                ImportError::Invalid(format!(
                    "セッション {old_id} が未知のWorkbookを参照しています: {old_workbook}"
                ))
            })?;
            if !question_notes.contains_key(&(old_workbook.to_string(), question_id.to_string())) {
                return Err(ImportError::Invalid(format!(
                    "セッション {old_id} が存在しない問題を参照しています: {question_id}"
                )));
            }
            if !old_workbook_ids.insert(old_workbook.to_string()) {
                // already present
            } else {
                let next_ordinal = workbook_order.len() as u32;
                workbook_order.insert(old_workbook.to_string(), next_ordinal);
                workbooks.push(LegacySessionWorkbook {
                    workbook_id: workbook_id.clone(),
                    ordinal: next_ordinal,
                    result_ack: ended_at.is_some(),
                });
            }
            let key = (old_workbook.to_string(), question_id.to_string());
            let status = if skipped.contains(&key) {
                SessionItemStatus::Skipped
            } else if outcome_keys.contains(&key)
                || attempts_by_session.get(&old_id).is_some_and(|attempts| {
                    attempts
                        .iter()
                        .any(|attempt| attempt.question_id == question_id)
                })
                || ordinal < index
            {
                SessionItemStatus::Answered
            } else {
                SessionItemStatus::Pending
            };
            items.push(LegacySessionItem {
                id: Uuid::new_v4().to_string(),
                ordinal: ordinal as u32,
                workbook_id: workbook_id.clone(),
                note_id: note_id.to_string(),
                question_id: question_id.to_string(),
                status,
            });
        }
        if index > items.len() {
            return Err(ImportError::Invalid(format!(
                "セッション {old_id} のindexがqueue長を超えています"
            )));
        }
        let current_item_id = if index < items.len() {
            Some(items[index].id.clone())
        } else {
            None
        };
        let current_workbook_id = if index < items.len() {
            Some(items[index].workbook_id.clone())
        } else {
            None
        };
        let draft_json = value
            .get("draft")
            .filter(|value| !value.is_null())
            .map(serde_json::to_string)
            .transpose()?;
        let hint = value.get("hint").and_then(Value::as_bool).unwrap_or(false);
        let revealed = value
            .get("revealed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let question_started_at = value
            .get("questionStartedAt")
            .and_then(Value::as_str)
            .map(|value| parse_required_datetime(value, "sessions.questionStartedAt"))
            .transpose()?;
        let question_elapsed_ms = value
            .get("questionElapsedMs")
            .and_then(Value::as_i64)
            .unwrap_or(0);

        if is_active && let Some(current) = current_workbook_id.as_ref() {
            for workbook in &mut workbooks {
                if workbook.workbook_id != *current {
                    workbook.result_ack = true;
                }
            }
        }

        let attempts = attempts_by_session.remove(&old_id).unwrap_or_default();
        for attempt in &attempts {
            if !items.iter().any(|item| {
                item.workbook_id == attempt.workbook_id && item.question_id == attempt.question_id
            }) {
                return Err(ImportError::Invalid(format!(
                    "セッション {old_id} の回答がqueueに存在しない問題を参照しています"
                )));
            }
        }
        let _ = workbook_revisions;
        sessions.push(LegacySession {
            id: Uuid::new_v4().to_string(),
            mode,
            status: if is_active {
                SessionStatus::Active
            } else {
                SessionStatus::Ended
            },
            started_at,
            ended_at,
            current_workbook_id,
            current_item_id,
            phase,
            draft_json,
            hint,
            revealed,
            pending_correct: None,
            pending_unknown: false,
            pending_at: None,
            question_started_at,
            question_elapsed_ms,
            cursor_kind: "initial".to_string(),
            revision: 0,
            items,
            workbooks,
            attempts,
        });
    }

    if !attempts_by_session.is_empty() {
        warnings.push(format!(
            "{}件の旧回答はセッションが見つからないため移行しませんでした",
            attempts_by_session.values().map(Vec::len).sum::<usize>()
        ));
    }
    Ok(sessions)
}

fn read_attempts(
    connection: &Connection,
    table: &str,
    id_map: &BTreeMap<String, String>,
    question_notes: &BTreeMap<(String, String), String>,
    output: &mut BTreeMap<String, Vec<LegacyAttempt>>,
) -> Result<(), ImportError> {
    let sql =
        format!("SELECT session_id, workbook_id, question_id, outcome FROM {table} ORDER BY rowid");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (session_id, old_workbook, question_id, raw) = row?;
        let workbook_id = id_map.get(&old_workbook).ok_or_else(|| {
            ImportError::Invalid(format!(
                "{table} が未知のWorkbookを参照しています: {old_workbook}"
            ))
        })?;
        let note_id = question_notes
            .get(&(old_workbook, question_id.clone()))
            .ok_or_else(|| {
                ImportError::Invalid(format!(
                    "{table} が存在しない問題を参照しています: {question_id}"
                ))
            })?;
        let outcome: LegacyOutcome = serde_json::from_str(&raw)?;
        if !(1..=4).contains(&outcome.rating) {
            return Err(ImportError::Invalid(format!(
                "{table} の評価が不正です: {}",
                outcome.rating
            )));
        }
        let answered_at = parse_required_datetime(&outcome.at, &format!("{table}.outcome.at"))?;
        output.entry(session_id).or_default().push(LegacyAttempt {
            id: Uuid::new_v4().to_string(),
            workbook_id: workbook_id.clone(),
            note_id: note_id.clone(),
            question_id,
            answered_at,
            correct: outcome.correct,
            unknown: outcome.unknown,
            hint: outcome.hint,
            elapsed_ms: outcome.elapsed_ms.max(0),
            rating: outcome.rating,
            answer_json: serde_json::to_string(&outcome.answer)?,
            before_state_json: outcome.before,
            after_state_json: None,
            state_revision: 0,
            short_term: false,
            created_at: answered_at,
        });
    }
    Ok(())
}

fn split_outcome_key(value: &str) -> Option<(String, String)> {
    let (workbook, question) = value.split_once('/')?;
    Some((workbook.to_string(), question.to_string()))
}

fn pragma_i64(connection: &Connection, name: &str) -> Result<i64, ImportError> {
    Ok(connection.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?)
}

fn parse_required_datetime(value: &str, path: &str) -> Result<DateTime<Utc>, ImportError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ImportError::Invalid(format!("{path} の日時が不正です: {value}")))
}

fn set_private_file(path: &Path) -> Result<(), ImportError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_keys_split_on_the_last_slash_component() {
        assert_eq!(
            split_outcome_key("a/b/question"),
            Some(("a".to_string(), "b/question".to_string()))
        );
        assert_eq!(split_outcome_key("invalid"), None);
    }

    const PNG_2X2: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd,
        0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x0e, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0x0f, 0x06, 0x0c, 0x10, 0x0a, 0x00, 0x53, 0xba, 0x0b, 0xf5, 0x98, 0xac, 0x20, 0x67, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn file_workbook(asset_path: &str) -> String {
        format!(
            r#"{{"format":"workbook","schemaVersion":2,"id":"w","revision":1,
            "title":"T","description":"","language":"ja","tags":[],
            "assets":[{{"id":"img","alt":"図","mediaType":"image/png","file":"{asset_path}"}}],
            "notes":[{{"id":"n","title":"N","knowledgeCard":"c","questions":[
              {{"id":"q","revision":1,"type":"hotspot","prompt":"p","explanation":"e",
                "hotspot":{{"asset":"img","regions":[{{"id":"r","alt":"R",
                  "shape":{{"type":"circle","cx":0.5,"cy":0.5,"r":0.1}}}}]}},
                "answer":{{"regionIds":["r"]}}}}]}}]}}"#
        )
    }

    #[test]
    fn file_assets_are_resolved_relative_to_the_workbook() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("figure.png"), PNG_2X2).unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(&path, file_workbook("figure.png")).unwrap();
        let (workbook, assets) = load_workbook_file(&path).unwrap();
        assert_eq!(workbook.assets.len(), 1);
        assert_eq!(assets.len(), 1);
        assert_eq!((assets[0].width, assets[0].height), (2, 2));
        assert_eq!(assets[0].bytes, PNG_2X2);
    }

    #[test]
    #[cfg(unix)]
    fn file_assets_cannot_escape_the_workbook_directory() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.png"), PNG_2X2).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.png"),
            dir.path().join("link.png"),
        )
        .unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(&path, file_workbook("link.png")).unwrap();
        let error = load_workbook_file(&path).unwrap_err();
        assert!(error.to_string().contains("教材フォルダの外"));
    }

    #[test]
    fn a_header_only_png_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("figure.png"), &PNG_2X2[..24]).unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(&path, file_workbook("figure.png")).unwrap();
        let error = load_workbook_file(&path).unwrap_err();
        assert!(error.to_string().contains("画像"), "{error}");
    }

    #[test]
    fn a_truncated_png_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("figure.png"),
            &PNG_2X2[..PNG_2X2.len() - 12],
        )
        .unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(&path, file_workbook("figure.png")).unwrap();
        assert!(load_workbook_file(&path).is_err());
    }

    #[test]
    fn a_file_whose_bytes_disagree_with_the_media_type_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // The extension and mediaType agree (JPEG), but the bytes are a PNG.
        std::fs::write(dir.path().join("figure.jpg"), PNG_2X2).unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(
            &path,
            file_workbook("figure.jpg").replace("image/png", "image/jpeg"),
        )
        .unwrap();
        let error = load_workbook_file(&path).unwrap_err();
        assert!(error.to_string().contains("形式が一致しません"), "{error}");
    }

    #[test]
    fn an_oversized_asset_file_is_rejected_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("figure.png"),
            vec![0u8; workbook_domain::MAX_ASSET_BYTES + 1],
        )
        .unwrap();
        let path = dir.path().join("book.json");
        std::fs::write(&path, file_workbook("figure.png")).unwrap();
        let error = load_workbook_file(&path).unwrap_err();
        assert!(error.to_string().contains("5MB"), "{error}");
    }

    #[test]
    fn an_embedded_header_only_png_is_rejected() {
        let raw = r#"{
          "format":"workbook","schemaVersion":2,"id":"w","revision":1,
          "title":"T","description":"","language":"ja","tags":[],
          "assets":[{"id":"img","alt":"図の説明","mediaType":"image/png",
            "data":"iVBORw0KGgoAAAANSUhEUgAAAAIAAAAC"}],
          "notes":[{"id":"n","title":"N","knowledgeCard":"c","questions":[
            {"id":"q","revision":1,"type":"true_false","prompt":"p","answer":true,"explanation":"e"}
          ]}]
        }"#;
        let workbook = parse_workbook(raw).unwrap();
        let error = workbook.embedded_assets().unwrap_err();
        assert!(error.message.contains("画像"), "{error}");
    }
}
