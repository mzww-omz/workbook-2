use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use tempfile::TempDir;
use workbook_domain::{Mode, Phase};
use workbook_scheduler::FixedClock;
use workbook_service::WorkbookService;

struct Harness {
    _dir: TempDir,
    clock: Arc<FixedClock>,
    service: WorkbookService,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap(),
        ));
        let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
        let mut settings = service.settings().unwrap();
        settings.timezone = "UTC".to_string();
        settings.automatic_rating = true;
        settings.new_limit = 100;
        service.update_settings(settings).unwrap();
        Self {
            _dir: dir,
            clock,
            service,
        }
    }

    fn import(&self, questions: usize) -> String {
        let notes = serde_json::json!([{
            "id": "n1",
            "title": "時間予算",
            "knowledgeCard": "短い時間でも、復習は賢く。",
            "questions": (0..questions).map(|index| serde_json::json!({
                "id": format!("q{index}"),
                "revision": 1,
                "type": "true_false",
                "prompt": format!("問 {index}"),
                "answer": true,
                "explanation": "解説"
            })).collect::<Vec<_>>()
        }]);
        let workbook = serde_json::json!({
            "format": "workbook",
            "schemaVersion": 1,
            "id": "budget-book",
            "revision": 1,
            "title": "時間予算の教材",
            "description": "",
            "language": "ja",
            "tags": [],
            "notes": notes
        });
        self.service
            .import_workbook_json(&workbook.to_string())
            .unwrap()
    }

    fn active(&self) -> workbook_service::StudyView {
        self.service.active_view().unwrap().unwrap()
    }

    fn review_state_count(&self) -> i64 {
        let connection =
            rusqlite::Connection::open(self._dir.path().join("workbook.sqlite")).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM review_states", [], |row| row.get(0))
            .unwrap()
    }
}

#[test]
fn a_three_minute_session_selects_what_fits_and_keeps_the_rest_new() {
    let harness = Harness::new();
    let book = harness.import(40);
    harness
        .service
        .start_session_with_budget(Mode::New, Some(&book), Some(180))
        .unwrap();
    let view = harness.active();
    assert_eq!(view.mode, Mode::New);
    let budget = view.budget.expect("budget is recorded");
    assert_eq!(budget.target_seconds, 180);
    assert_eq!(budget.used_ms, 0);
    assert!(!budget.reached);
    // 20 seconds per true/false question: nine questions fit in 180 seconds.
    assert_eq!(view.progress.total, 9);
    // The session item list is fixed at start; the remaining 31 new questions
    // were not silently marked studied.
    assert!(harness.review_state_count() == 0);
}

#[test]
fn a_twenty_minute_session_is_larger_and_still_bounded_by_the_new_limit() {
    let harness = Harness::new();
    let book = harness.import(40);
    harness
        .service
        .start_session_with_budget(Mode::New, Some(&book), Some(1_200))
        .unwrap();
    let view = harness.active();
    assert_eq!(view.budget.unwrap().target_seconds, 1_200);
    assert_eq!(view.progress.total, 40);
}

#[test]
fn a_zero_second_budget_is_rejected_and_no_session_is_created() {
    let harness = Harness::new();
    let book = harness.import(4);
    let error = harness
        .service
        .start_session_with_budget(Mode::New, Some(&book), Some(0))
        .unwrap_err();
    assert!(error.to_string().contains("1分以上"));
    assert!(harness.service.active_view().unwrap().is_none());
}

#[test]
fn reaching_the_budget_never_discards_an_answer_in_progress() {
    let harness = Harness::new();
    let book = harness.import(10);
    harness
        .service
        .start_session_with_budget(Mode::New, Some(&book), Some(60))
        .unwrap();
    let view = harness.active();
    let view = if view.phase == Phase::Intro {
        harness
            .service
            .begin_question(&view.session_id, view.revision)
            .unwrap()
    } else {
        view
    };
    // Far beyond the 60 second target. The target is a selection budget, not a
    // hard timer; the answer must still commit.
    harness.clock.advance(Duration::minutes(45));
    let evaluated = harness
        .service
        .evaluate_answer(
            &view.session_id,
            Some(&workbook_domain::Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap();
    assert!(evaluated.feedback.as_ref().is_some_and(|f| f.committed));
    let view = harness.active();
    assert!(view.budget.unwrap().used_ms > 0);
}

#[test]
fn a_time_budgeted_review_mixes_new_content_and_reports_plan_progress() {
    let harness = Harness::new();
    let book = harness.import(20);
    harness
        .service
        .start_session_with_budget(Mode::Review, Some(&book), Some(600))
        .unwrap();
    let view = harness.active();
    // There is no due review yet, but the time budget may introduce new
    // content. The session uses the plan path so each note gets its intro and
    // the daily new limit is counted.
    assert_eq!(view.mode, Mode::Plan);
    assert!(view.progress.total > 0);
    assert_eq!(view.budget.unwrap().target_seconds, 600);
}

#[test]
fn practice_with_a_budget_does_not_touch_fsrs_state() {
    let harness = Harness::new();
    let book = harness.import(20);
    harness
        .service
        .start_session_with_budget(Mode::Practice, Some(&book), Some(120))
        .unwrap();
    let view = harness.active();
    assert_eq!(view.mode, Mode::Practice);
    assert!(view.progress.total <= 6);
    assert_eq!(harness.review_state_count(), 0);
}

#[test]
fn reopening_keeps_the_budget_target_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
    let mut settings = service.settings().unwrap();
    settings.timezone = "UTC".to_string();
    settings.new_limit = 100;
    service.update_settings(settings).unwrap();
    let workbook = serde_json::json!({
        "format": "workbook", "schemaVersion": 1, "id": "reopen", "revision": 1,
        "title": "再開", "description": "", "language": "ja", "tags": [],
        "notes": [{
            "id": "n", "title": "n", "knowledgeCard": "c",
            "questions": (0..5).map(|index| serde_json::json!({
                "id": format!("q{index}"), "revision": 1, "type": "true_false",
                "prompt": "p", "answer": true, "explanation": "e"
            })).collect::<Vec<_>>()
        }]
    });
    let book = service.import_workbook_json(&workbook.to_string()).unwrap();
    service
        .start_session_with_budget(Mode::New, Some(&book), Some(300))
        .unwrap();
    let session_id = service.active_view().unwrap().unwrap().session_id;
    drop(service);
    let reopened = WorkbookService::open(dir.path(), clock).unwrap();
    let view = reopened.active_view().unwrap().unwrap();
    assert_eq!(view.session_id, session_id);
    assert_eq!(view.budget.unwrap().target_seconds, 300);
}
