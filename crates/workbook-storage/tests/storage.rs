use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::tempdir;
use workbook_domain::StudyCalendar;
use workbook_scheduler::{Clock, FixedClock};
use workbook_service::{SAMPLE_WORKBOOK_JSON, WorkbookService};
use workbook_storage::Storage;

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
    workbook_storage::schema::SCHEMA_SQL
        .replace("  rating_source TEXT NOT NULL DEFAULT 'manual',\n", "")
        .replace("  assist_json TEXT,\n", "")
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
