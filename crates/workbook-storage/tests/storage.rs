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
    let committed = service
        .commit_rating(
            &evaluated.session_id,
            &uuid::Uuid::new_v4().to_string(),
            workbook_domain::Rating::Good,
            evaluated.revision,
        )
        .unwrap();
    service
        .finish(&committed.session_id, committed.revision)
        .unwrap();
    let next = service.next_wake_at().unwrap().expect("future due");
    assert!(next > clock.now());
}
