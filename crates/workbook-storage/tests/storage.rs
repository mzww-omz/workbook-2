use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::tempdir;
use workbook_domain::StudyCalendar;
use workbook_scheduler::{Clock, FixedClock};
use workbook_service::{SAMPLE_WORKBOOK_JSON, SAMPLE_WORKBOOK_V2_JSON, WorkbookService};
use workbook_storage::Storage;

const TOPIC_SCHEMA: &str = r#"CREATE TABLE topics(
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
CREATE INDEX question_topics_topic ON question_topics(topic_id, workbook_id, question_id);

"#;

#[test]
fn version_one_data_survives_the_v4_migration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    create_v1_database(&path);
    let content = r#"{
      "format":"workbook","schemaVersion":1,"id":"legacy","revision":1,
      "title":"Legacy","description":"","language":"ja","tags":["old"],
      "notes":[{"id":"n","title":"Note","knowledgeCard":"card","questions":[
        {"id":"q","revision":1,"type":"true_false","prompt":"p","answer":true,"explanation":"e"}
      ]}]
    }"#;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                r#"
                INSERT INTO workbooks(local_id,source_id,source_revision,title,content_json,imported_at,last_used_at)
                VALUES('w1','legacy',1,'Legacy','{content}','2026-01-01T00:00:00.000Z',NULL);
                INSERT INTO notes(workbook_id,note_id,introduced,introduced_at)
                VALUES('w1','n',1,'2026-01-01T00:00:00.000Z');
                INSERT INTO questions(workbook_id,question_id,note_id,buried_until)
                VALUES('w1','q','n',NULL);
                INSERT INTO review_states(workbook_id,question_id,state,stability,stability_fast,
                  difficulty,due_at,last_review_at,reps,lapses,scheduled_days,learning_step,
                  algorithm,policy_version,revision)
                VALUES('w1','q',2,5.0,5.0,5.0,'2026-02-01T00:00:00.000Z',
                  '2026-01-01T00:00:00.000Z',2,0,30.0,0,'FSRS-7','workbook2-step-v1',1);
                "#
            ))
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.workbooks().unwrap().len(), 1);
    assert_eq!(storage.learned_count("w1").unwrap(), 1);
    assert!(storage.review_state("w1", "q").unwrap().is_some());
    assert!(storage.exams(true).unwrap().is_empty());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('exam_id','time_limit_seconds')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 2);
}

const GOAL_SCHEMA: &str = r#"CREATE TABLE goals(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT,
  target_date TEXT,
  target_score REAL,
  daily_minutes INTEGER NOT NULL DEFAULT 30,
  scope_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT
);
CREATE INDEX goals_archive ON goals(archived_at, target_date);

"#;

const GOAL_ID_COLUMN: &str = ",\n  goal_id TEXT REFERENCES goals(id) ON DELETE SET NULL";

fn schema_without_topics_and_goals() -> String {
    workbook_storage::schema::SCHEMA_SQL
        .replace(TOPIC_SCHEMA, "")
        .replace(GOAL_SCHEMA, "")
        .replace(GOAL_ID_COLUMN, "")
}

fn schema_without_goals() -> String {
    workbook_storage::schema::SCHEMA_SQL
        .replace(GOAL_SCHEMA, "")
        .replace(GOAL_ID_COLUMN, "")
}

fn v3_schema_sql() -> String {
    schema_without_topics_and_goals()
        .replace(
            "  revision INTEGER NOT NULL DEFAULT 0,\n  exam_id TEXT REFERENCES exams(id) ON DELETE SET NULL,\n  time_limit_seconds INTEGER,\n  time_budget_seconds INTEGER\n",
            "  revision INTEGER NOT NULL DEFAULT 0\n",
        )
        .replace(
            r#"CREATE TABLE exams(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT,
  target_date TEXT,
  target_score REAL,
  daily_minutes INTEGER NOT NULL DEFAULT 30,
  scope_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT
);
CREATE INDEX exams_archive ON exams(archived_at, target_date);

"#,
            "",
        )
        .replace(
            r#"CREATE TABLE mock_answers(
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

"#,
            "",
        )
        .replace(
            r#"CREATE TABLE mock_results(
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
CREATE INDEX mock_results_exam ON mock_results(exam_id, submitted_at DESC);

"#,
            "",
        )
}

fn create_v3_database(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(&v3_schema_sql()).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=1463962418; PRAGMA user_version=3;
             INSERT INTO meta(key,value) VALUES('app','workbook-2');",
        )
        .unwrap();
}

#[test]
fn version_three_data_survives_the_v4_migration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    create_v3_database(&path);
    let content = r#"{
      "format":"workbook","schemaVersion":1,"id":"v3","revision":1,
      "title":"V3","description":"","language":"ja","tags":["v3"],
      "notes":[{"id":"n","title":"Note","knowledgeCard":"card","questions":[
        {"id":"q","revision":1,"type":"true_false","prompt":"p","answer":true,"explanation":"e"}
      ]}]
    }"#;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                r#"
                INSERT INTO workbooks(local_id,source_id,source_revision,title,content_json,imported_at,last_used_at)
                VALUES('w3','v3',1,'V3','{content}','2026-01-01T00:00:00.000Z',NULL);
                INSERT INTO notes(workbook_id,note_id,introduced,introduced_at)
                VALUES('w3','n',1,'2026-01-01T00:00:00.000Z');
                INSERT INTO questions(workbook_id,question_id,note_id,buried_until)
                VALUES('w3','q','n',NULL);
                INSERT INTO review_states(workbook_id,question_id,state,stability,stability_fast,
                  difficulty,due_at,last_review_at,reps,lapses,scheduled_days,learning_step,
                  algorithm,policy_version,revision)
                VALUES('w3','q',2,5.0,5.0,5.0,'2026-02-01T00:00:00.000Z',
                  '2026-01-01T00:00:00.000Z',2,0,30.0,0,'FSRS-7','workbook2-step-v1',1);
                "#
            ))
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.workbooks().unwrap().len(), 1);
    assert_eq!(storage.learned_count("w3").unwrap(), 1);
    assert!(storage.exams(true).unwrap().is_empty());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
               AND name IN ('exams','mock_answers','mock_results')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 3);
}

#[test]
fn version_five_database_gains_time_budget_without_losing_sessions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&schema_without_topics_and_goals())
            .unwrap();
        connection
            .execute_batch(
                "ALTER TABLE sessions DROP COLUMN time_budget_seconds;
                 PRAGMA application_id=1463962418;
                 PRAGMA user_version=5;
                 INSERT INTO meta(key,value) VALUES('app','workbook-2');
                 INSERT INTO sessions(id,mode,status,started_at,phase)
                   VALUES('s1','review','ended','2026-01-01T00:00:00.000Z','overall_result');",
            )
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.session_bundle("s1").unwrap().session.id, "s1");
    drop(storage);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='time_budget_seconds'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(column, 1);
}

#[test]
fn version_seven_database_gains_goals_without_touching_exams_or_sessions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(&schema_without_goals()).unwrap();
        connection
            .execute_batch(
                "PRAGMA application_id=1463962418;
                 PRAGMA user_version=7;
                 INSERT INTO meta(key,value) VALUES('app','workbook-2');
                 INSERT INTO sessions(id,mode,status,started_at,phase)
                   VALUES('s1','review','ended','2026-01-01T00:00:00.000Z','overall_result');",
            )
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert!(storage.goals(true).unwrap().is_empty());
    assert!(storage.exams(true).unwrap().is_empty());
    assert_eq!(storage.session_bundle("s1").unwrap().session.id, "s1");
    drop(storage);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='goal_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 1);
}

#[test]
fn version_six_database_gains_topics_without_losing_learning_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&schema_without_topics_and_goals())
            .unwrap();
        connection
            .execute_batch(
                "PRAGMA application_id=1463962418;
                 PRAGMA user_version=6;
                 INSERT INTO meta(key,value) VALUES('app','workbook-2');
                 INSERT INTO sessions(id,mode,status,started_at,phase)
                   VALUES('s1','review','ended','2026-01-01T00:00:00.000Z','overall_result');",
            )
            .unwrap();
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert!(storage.topics().unwrap().is_empty());
    assert!(storage.question_topics().unwrap().is_empty());
    assert_eq!(storage.session_bundle("s1").unwrap().session.id, "s1");
    drop(storage);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
               AND name IN ('topics','question_topics')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 2);
}

#[test]
fn daily_backups_keep_seven_generations_and_private_permissions() {
    let dir = tempdir().unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let calendar = StudyCalendar::new("UTC").unwrap();
    for day in 1..=9 {
        let now = Utc.with_ymd_and_hms(2026, 1, day, 12, 0, 0).unwrap();
        storage.daily_backup(&calendar, now, 7).unwrap();
    }
    let files = storage.backup_files().unwrap();
    assert_eq!(files.len(), 7);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in &files {
            let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
    storage.close().unwrap();
}

fn v1_schema_sql() -> String {
    schema_without_topics_and_goals()
        .replace("  rating_source TEXT NOT NULL DEFAULT 'manual',\n", "")
        .replace("  assist_json TEXT,\n", "")
        .replace(
            "  revision INTEGER NOT NULL DEFAULT 0,\n  exam_id TEXT REFERENCES exams(id) ON DELETE SET NULL,\n  time_limit_seconds INTEGER,\n  time_budget_seconds INTEGER\n",
            "  revision INTEGER NOT NULL DEFAULT 0\n",
        )
        .replace(
            r#"CREATE TABLE exams(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT,
  target_date TEXT,
  target_score REAL,
  daily_minutes INTEGER NOT NULL DEFAULT 30,
  scope_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT
);
CREATE INDEX exams_archive ON exams(archived_at, target_date);

"#,
            "",
        )
        .replace(
            r#"CREATE TABLE mock_answers(
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

"#,
            "",
        )
        .replace(
            r#"CREATE TABLE mock_results(
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
CREATE INDEX mock_results_exam ON mock_results(exam_id, submitted_at DESC);

"#,
            "",
        )
        .replace(
            "CREATE TABLE assets(\n  workbook_id TEXT NOT NULL REFERENCES workbooks(local_id) ON DELETE CASCADE,\n  asset_id TEXT NOT NULL,\n  media_type TEXT NOT NULL,\n  alt TEXT NOT NULL,\n  width INTEGER NOT NULL,\n  height INTEGER NOT NULL,\n  bytes BLOB NOT NULL,\n  PRIMARY KEY(workbook_id, asset_id)\n);\n\n",
            "",
        )
}

fn create_v1_database(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(&v1_schema_sql()).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=1463962418; PRAGMA user_version=1;
             INSERT INTO meta(key,value) VALUES('app','workbook-2');",
        )
        .unwrap();
}

fn schema_version_and_rating_source(path: &std::path::Path) -> (i64, i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('attempts') WHERE name='rating_source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    (version, column)
}

#[test]
fn version_one_database_migrates_in_place_with_a_safety_backup() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    create_v1_database(&path);
    let storage = Storage::open(dir.path()).unwrap();
    drop(storage);
    let (version, column) = schema_version_and_rating_source(&path);
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    assert_eq!(column, 1);
    let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("pre-migration-v1-"))
        .collect();
    assert_eq!(backups.len(), 1);
}

#[test]
fn version_one_backup_can_be_restored_and_is_upgraded() {
    let source = tempdir().unwrap();
    let old = source.path().join("old.sqlite");
    create_v1_database(&old);
    let dir = tempdir().unwrap();
    let mut storage = Storage::open(dir.path()).unwrap();
    storage.restore_from(&old).unwrap();
    drop(storage);
    let (version, column) = schema_version_and_rating_source(&dir.path().join("workbook.sqlite"));
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    assert_eq!(column, 1);
}

#[test]
fn next_due_at_only_returns_future_deadlines() {
    let dir = tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
    let book = service.import_workbook_json(SAMPLE_WORKBOOK_JSON).unwrap();
    service
        .start_session(workbook_domain::Mode::New, Some(&book))
        .unwrap();
    let view = service.active_view().unwrap().unwrap();
    let view = if view.phase == workbook_domain::Phase::Intro {
        service
            .begin_question(&view.session_id, view.revision)
            .unwrap()
    } else {
        view
    };
    let evaluated = service
        .evaluate_answer(
            &view.session_id,
            Some(&workbook_domain::Answer::Text("b".to_string())),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    assert!(evaluated.feedback.as_ref().unwrap().committed);
    service
        .finish(&evaluated.session_id, evaluated.revision)
        .unwrap();
    let next = service.next_wake_at().unwrap().expect("future due");
    assert!(next > clock.now());
}

#[test]
fn version_four_backup_without_daily_minutes_is_validated_and_upgraded() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&schema_without_topics_and_goals())
            .unwrap();
        connection
            .execute_batch(
                "ALTER TABLE exams DROP COLUMN daily_minutes;
                 ALTER TABLE sessions DROP COLUMN time_budget_seconds;
                 PRAGMA application_id=1463962418;
                 PRAGMA user_version=4;
                 INSERT INTO meta(key,value) VALUES('app','workbook-2');",
            )
            .unwrap();
    }
    // A v4 backup is still readable before the forward migration adds the
    // column, and opening it applies the same default as new exams.
    Storage::validate_backup(&path).unwrap();
    let storage = Storage::open(dir.path()).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    storage.close().unwrap();
}

// ---- real v5 schema migration -------------------------------------------

const REAL_V5_SCHEMA: &str = include_str!("fixtures/schema_v5.sql");

fn create_real_v5_database(path: &std::path::Path) -> (String, Vec<u8>) {
    use rusqlite::params;
    let workbook = workbook_domain::parse_workbook(SAMPLE_WORKBOOK_V2_JSON).unwrap();
    let assets = workbook.embedded_assets().unwrap();
    let question = &workbook.notes[0].questions[0];
    let note = &workbook.notes[0];
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(REAL_V5_SCHEMA).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=1463962418;
             PRAGMA user_version=5;
             INSERT INTO meta(key,value) VALUES('app','workbook-2');",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workbooks(local_id,source_id,source_revision,title,content_json,
                    imported_at,last_used_at)
             VALUES('v5-book',?1,1,?2,?3,'2026-01-01T00:00:00.000Z',NULL)",
            params![workbook.id, workbook.title, workbook.to_json()],
        )
        .unwrap();
    for note in &workbook.notes {
        connection
            .execute(
                "INSERT INTO notes(workbook_id,note_id,introduced,introduced_at)
                 VALUES('v5-book',?1,1,'2026-01-01T00:00:00.000Z')",
                [&note.id],
            )
            .unwrap();
        for question in &note.questions {
            connection
                .execute(
                    "INSERT INTO questions(workbook_id,question_id,note_id,buried_until)
                     VALUES('v5-book',?1,?2,NULL)",
                    params![question.id, note.id],
                )
                .unwrap();
        }
    }
    for asset in &assets {
        connection
            .execute(
                "INSERT INTO assets(workbook_id,asset_id,media_type,alt,width,height,bytes)
                 VALUES('v5-book',?1,?2,?3,?4,?5,?6)",
                params![
                    asset.id,
                    asset.media_type,
                    asset.alt,
                    asset.width,
                    asset.height,
                    asset.bytes
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO review_states(workbook_id,question_id,state,stability,stability_fast,
                    difficulty,due_at,last_review_at,reps,lapses,scheduled_days,learning_step,
                    algorithm,policy_version,revision)
             VALUES('v5-book',?1,2,5.5,5.5,4.5,'2026-02-01T00:00:00.000Z',
                    '2026-01-01T00:00:00.000Z',3,1,30.0,0,'FSRS-7','workbook2-step-v1',4)",
            [&question.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO exams(id,title,description,target_date,target_score,daily_minutes,
                    scope_json,created_at,updated_at,archived_at)
             VALUES('exam-v5','V5試験',NULL,'2026-03-01',80.0,45,
                    '{\"groups\":[{\"workbookIds\":[\"v5-book\"]}]}',
                    '2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',NULL)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions(id,mode,status,started_at,ended_at,phase,exam_id)
             VALUES('s-v5','review','ended','2026-01-01T00:00:00.000Z',
                    '2026-01-01T00:05:00.000Z','overall_result','exam-v5')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO attempts(id,session_id,workbook_id,note_id,question_id,answered_at,
                    correct,unknown,hint,elapsed_ms,rating,rating_source,answer_json,
                    before_state_json,after_state_json,state_revision,short_term,assist_json,
                    created_at)
             VALUES('a-v5','s-v5','v5-book',?1,?2,'2026-01-01T00:01:00.000Z',1,0,0,12000,3,
                    'automatic','{\"value\":true}',NULL,NULL,3,0,NULL,
                    '2026-01-01T00:01:00.000Z')",
            params![note.id, question.id],
        )
        .unwrap();
    (question.id.clone(), assets[0].bytes.clone())
}

fn v5_migration_backups(directory: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(directory.join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("pre-migration-v5-") && name.ends_with(".sqlite"))
        .collect()
}

#[test]
fn a_real_v5_database_migrates_with_all_learning_data_and_assets_intact() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    let (question_id, asset_bytes) = create_real_v5_database(&path);
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 5);
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(storage.workbooks().unwrap().len(), 1);
    assert_eq!(storage.workbooks().unwrap()[0].local_id, "v5-book");
    let card = storage
        .review_state("v5-book", &question_id)
        .unwrap()
        .expect("FSRS state is preserved");
    assert!((card.stability - 5.5).abs() < 1e-4);
    assert_eq!(card.reps, 3);
    assert_eq!(card.lapses, 1);
    let attempts = storage.attempts_for_session("s-v5").unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].question_id, question_id);
    assert_eq!(attempts[0].question_revision, 1);
    assert_eq!(
        attempts[0].rating_source,
        workbook_domain::RatingSource::Automatic
    );
    let exams = storage.exams(true).unwrap();
    assert_eq!(exams.len(), 1);
    assert_eq!(exams[0].daily_minutes, 45);
    let session = storage.session_bundle("s-v5").unwrap().session;
    assert_eq!(session.exam_id.as_deref(), Some("exam-v5"));
    let assets = storage.assets_for_workbook("v5-book").unwrap();
    assert!(!assets.is_empty());
    assert_eq!(assets[0].bytes, asset_bytes);
    assert_eq!(storage.integrity().unwrap(), "ok");
    let questions: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM questions WHERE workbook_id='v5-book' AND revision=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        questions,
        storage.workbook("v5-book").unwrap().question_count() as i64
    );
    let fk: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    let version: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
    let backups = v5_migration_backups(dir.path());
    assert_eq!(backups.len(), 1, "backups after first open: {backups:?}");
    let backup = dir.path().join("backups").join(&backups[0]);
    Storage::validate_backup(&backup).unwrap();
    drop(storage);

    // Reopening an already migrated file is a no-op and creates no second
    // migration backup.
    let storage = Storage::open(dir.path()).unwrap();
    let backups = v5_migration_backups(dir.path());
    assert_eq!(backups.len(), 1, "backups after reopen: {backups:?}");
    assert!(
        storage
            .review_state("v5-book", &question_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_v5_file_with_an_incompatible_column_is_rejected_without_partial_migration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("workbook.sqlite");
    let (question_id, _) = create_real_v5_database(&path);
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        // Simulate a partially upgraded file: a v8 column exists while the
        // version still says 5. The v8 step must fail, and the whole
        // transaction must roll back the v6/v7 steps that ran before it.
        connection
            .execute_batch(
                "ALTER TABLE sessions ADD COLUMN goal_id TEXT REFERENCES goals(id) ON DELETE SET NULL;",
            )
            .unwrap();
    }
    let error = match Storage::open(dir.path()) {
        Ok(_) => panic!("a partially migrated database must not open"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("SQLite") || error.to_string().contains("goal_id"));
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5);
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
               AND name IN ('topics','question_topics','goals')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    let time_budget: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='time_budget_seconds'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(time_budget, 0);
    let card: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM review_states WHERE question_id=?1",
            [&question_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(card, 1);
}

#[test]
fn a_non_database_file_is_rejected() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("workbook.sqlite"), b"not a sqlite database").unwrap();
    assert!(Storage::open(dir.path()).is_err());
}

#[test]
fn a_real_v5_backup_can_be_restored_and_is_migrated() {
    let source = tempdir().unwrap();
    let old = source.path().join("old.sqlite");
    let (question_id, _) = create_real_v5_database(&old);
    let target = tempdir().unwrap();
    let mut storage = Storage::open(target.path()).unwrap();
    storage.restore_from(&old).unwrap();
    let card = storage
        .review_state("v5-book", &question_id)
        .unwrap()
        .expect("restored FSRS state");
    assert_eq!(card.reps, 3);
    assert_eq!(storage.attempts_for_session("s-v5").unwrap().len(), 1);
    assert_eq!(storage.exams(true).unwrap().len(), 1);
    assert_eq!(storage.integrity().unwrap(), "ok");
    drop(storage);
    let version: i64 = rusqlite::Connection::open(target.path().join("workbook.sqlite"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, workbook_storage::SCHEMA_VERSION);
}
