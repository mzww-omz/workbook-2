use std::sync::Arc;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use tempfile::TempDir;
use uuid::Uuid;
use workbook_domain::{Answer, Mode, Phase, Rating};
use workbook_scheduler::{Clock, FixedClock};
use workbook_service::{SAMPLE_WORKBOOK_JSON, ServiceError, StudyView, WorkbookService};

struct Harness {
    _dir: TempDir,
    clock: Arc<FixedClock>,
    service: Option<WorkbookService>,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ));
        let service = WorkbookService::open(dir.path(), clock.clone()).unwrap();
        let mut settings = service.settings().unwrap();
        settings.timezone = "UTC".to_string();
        service.update_settings(settings).unwrap();
        Self {
            _dir: dir,
            clock,
            service: Some(service),
        }
    }

    fn service(&self) -> &WorkbookService {
        self.service.as_ref().unwrap()
    }

    fn reopen(&mut self) {
        let clock = self.clock.clone();
        let dir = self._dir.path().to_path_buf();
        drop(self.service.take());
        self.service = Some(WorkbookService::open(&dir, clock).unwrap());
    }

    fn import(&self, raw: &str) -> String {
        self.service().import_workbook_json(raw).unwrap()
    }

    fn start(&self, mode: Mode, book: Option<&str>) -> StudyView {
        self.service().start_session(mode, book).unwrap();
        self.service().active_view().unwrap().unwrap()
    }

    fn begin(&self, view: &StudyView) -> StudyView {
        if view.phase == Phase::Intro {
            self.service()
                .begin_question(&view.session_id, view.revision)
                .unwrap()
        } else {
            view.clone()
        }
    }

    fn evaluate(&self, view: &StudyView, answer: Option<Answer>) -> StudyView {
        self.service()
            .evaluate_answer(
                &view.session_id,
                answer.as_ref(),
                false,
                None,
                false,
                view.revision,
            )
            .unwrap()
    }

    fn commit(&self, view: &StudyView, rating: Rating) -> StudyView {
        self.service()
            .commit_rating(
                &view.session_id,
                &Uuid::new_v4().to_string(),
                rating,
                view.revision,
            )
            .unwrap()
    }

    fn answer(&self, view: &StudyView, answer: Option<Answer>, rating: Rating) -> StudyView {
        let view = self.begin(view);
        let evaluated = self.evaluate(&view, answer);
        assert!(!evaluated.feedback.as_ref().unwrap().committed);
        self.commit(&evaluated, rating)
    }

    fn next(&self, view: &StudyView) -> StudyView {
        self.service()
            .next(&view.session_id, view.revision)
            .unwrap()
    }

    fn finish(&self, view: &StudyView) {
        self.service()
            .finish(&view.session_id, view.revision)
            .unwrap();
    }

    fn history(&self) -> Vec<workbook_service::HistoryEntry> {
        self.service().history(20).unwrap()
    }
}

fn one_question(id: &str) -> String {
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Tiny {id}","description":"","language":"ja","tags":[],
          "notes":[{{"id":"n-{id}","title":"Note","knowledgeCard":"card","questions":[
            {{"id":"q-{id}","revision":1,"type":"true_false","prompt":"true?",
              "answer":true,"explanation":"yes"}}
          ]}}]
        }}"#
    )
}

fn sibling_workbook(id: &str) -> String {
    format!(
        r#"{{
          "format":"workbook","schemaVersion":1,"id":"{id}","revision":1,
          "title":"Sibling","description":"","language":"ja","tags":[],
          "notes":[{{"id":"n","title":"Note","knowledgeCard":"card","questions":[
            {{"id":"q1","revision":1,"type":"true_false","prompt":"one?","answer":true,"explanation":"yes"}},
            {{"id":"q2","revision":1,"type":"true_false","prompt":"two?","answer":true,"explanation":"yes"}}
          ]}}]
        }}"#
    )
}

#[test]
fn new_session_shows_intro_once_and_saves_answer() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let view = h.start(Mode::New, Some(&book));
    assert_eq!(view.phase, Phase::Intro);
    assert_eq!(view.progress.total, 6);
    let view = h.begin(&view);
    assert_eq!(view.phase, Phase::Question);
    let view = h.evaluate(&view, Some(Answer::Text("b".to_string())));
    let feedback = view.feedback.as_ref().unwrap();
    assert_eq!(feedback.correct, Some(true));
    assert_eq!(feedback.suggested_rating, Rating::Good);
    let view = h.commit(&view, Rating::Good);
    assert_eq!(view.phase, Phase::Feedback);
    assert!(view.feedback.as_ref().unwrap().committed);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Intro);
    let view = h.begin(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.progress.position, 2);
    h.finish(&view);
    assert_eq!(h.history().len(), 1);
}

#[test]
fn new_limit_counts_distinct_questions_and_skip_does_not_write_fsrs() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let mut settings = h.service().settings().unwrap();
    settings.new_limit = 2;
    h.service().update_settings(settings).unwrap();
    let view = h.start(Mode::New, Some(&book));
    assert_eq!(view.progress.total, 2);
    let skipped = h.service().skip(&view.session_id, view.revision).unwrap();
    h.finish(&skipped);
    // The skipped question was not introduced and remains available.
    let again = h.start(Mode::New, Some(&book));
    assert_eq!(again.progress.total, 2);
    h.finish(&again);
}

#[test]
fn one_question_session_reissues_immediately_and_keeps_each_attempt() {
    let h = Harness::new();
    let book = h.import(&one_question("one"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Again);
    let view = h.next(&view);
    assert_eq!(view.item_kind, "short_term");
    h.finish(&view);
    let history = h.history();
    let stats = history[0].stats;
    assert_eq!(stats.questions, 1);
    assert_eq!(stats.answered, 1);
    assert_eq!(stats.attempts, 2);
    assert_eq!(stats.short_term, 1);
    assert_eq!(stats.first_pass_correct, 1);
}

#[test]
fn duplicate_attempt_id_is_idempotent_and_corrections_do_not_add_attempts() {
    let h = Harness::new();
    let book = h.import(&one_question("idem"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(false)));
    assert_eq!(evaluated.feedback.as_ref().unwrap().correct, Some(false));
    let attempt_id = Uuid::new_v4().to_string();
    let committed = h
        .service()
        .commit_rating(
            &evaluated.session_id,
            &attempt_id,
            Rating::Again,
            evaluated.revision,
        )
        .unwrap();
    let again = h
        .service()
        .commit_rating(
            &evaluated.session_id,
            &attempt_id,
            Rating::Again,
            committed.revision,
        )
        .unwrap();
    assert_eq!(
        again.feedback.as_ref().unwrap().attempt_id.as_deref(),
        Some(attempt_id.as_str())
    );
    let corrected = h
        .service()
        .correct_attempt(
            &again.session_id,
            &attempt_id,
            Some(true),
            None,
            again.revision,
        )
        .unwrap();
    assert_eq!(corrected.feedback.as_ref().unwrap().correct, Some(true));
    assert_eq!(
        corrected.feedback.as_ref().unwrap().rating,
        Some(Rating::Good)
    );
    let corrected = h
        .service()
        .correct_attempt(
            &corrected.session_id,
            &attempt_id,
            None,
            Some(Rating::Hard),
            corrected.revision,
        )
        .unwrap();
    assert_eq!(
        corrected.feedback.as_ref().unwrap().rating,
        Some(Rating::Hard)
    );
    h.finish(&corrected);
    let stats = h.history()[0].stats;
    assert_eq!(stats.attempts, 1);
    assert_eq!(stats.first_pass_correct, 1);
}

#[test]
fn stale_revision_is_rejected() {
    let h = Harness::new();
    let book = h.import(&one_question("stale"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.begin(&view);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    let error = h
        .service()
        .evaluate_answer(
            &evaluated.session_id,
            Some(&Answer::Bool(true)),
            false,
            None,
            false,
            view.revision,
        )
        .unwrap_err();
    assert!(matches!(error, ServiceError::Conflict(_)));
}

#[test]
fn sibling_questions_are_suppressed_in_review_but_same_question_repeats() {
    let h = Harness::new();
    let book = h.import(&sibling_workbook("sib"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&view);
    h.clock.advance(chrono::Duration::minutes(11));
    let review = h.start(Mode::Review, Some(&book));
    assert_eq!(review.progress.total, 1);
    let review = h.answer(&review, Some(Answer::Bool(false)), Rating::Again);
    let repeated = h.next(&review);
    assert_eq!(repeated.item_kind, "short_term");
    assert_eq!(repeated.progress.total, 1);
    h.finish(&repeated);
}

#[test]
fn multi_workbook_review_stops_at_each_result_and_requires_continue() {
    let h = Harness::new();
    let a = h.import(&one_question("a"));
    let b = h.import(&one_question("b"));
    for book in [&a, &b] {
        let view = h.start(Mode::New, Some(book));
        let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
        let view = h.next(&view);
        let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
        let view = h.next(&view);
        assert_eq!(view.phase, Phase::WorkbookResult);
        h.finish(&view);
    }
    h.clock.advance(chrono::Duration::days(11));
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 2);
    assert_eq!(
        snapshot
            .today
            .books
            .iter()
            .filter(|book| book.due == 1)
            .count(),
        2
    );
    let view = h.start(Mode::Review, None);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    assert!(
        view.workbook_result
            .as_ref()
            .unwrap()
            .next_workbook_title
            .is_some()
    );
    let view = h
        .service()
        .continue_session(&view.session_id, view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::Question);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    assert!(
        view.workbook_result
            .as_ref()
            .unwrap()
            .next_workbook_title
            .is_none()
    );
    let view = h
        .service()
        .continue_session(&view.session_id, view.revision)
        .unwrap();
    assert_eq!(view.phase, Phase::OverallResult);
    assert!(h.service().snapshot().unwrap().active.is_none());
}

#[test]
fn restart_recovery_preserves_feedback_and_excludes_closed_time() {
    let mut h = Harness::new();
    let book = h.import(&one_question("restart"));
    let view = h.start(Mode::New, Some(&book));
    let _view = h.begin(&view);
    h.clock.advance(chrono::Duration::hours(1));
    h.reopen();
    let view = h.service().active_view().unwrap().unwrap();
    assert_eq!(view.phase, Phase::Question);
    let evaluated = h.evaluate(&view, Some(Answer::Bool(true)));
    assert_eq!(evaluated.feedback.as_ref().unwrap().elapsed_ms, 0);
    h.clock.advance(chrono::Duration::seconds(2));
    h.reopen();
    let feedback = h.service().active_view().unwrap().unwrap();
    assert_eq!(feedback.phase, Phase::Feedback);
    assert!(!feedback.feedback.as_ref().unwrap().committed);
    let committed = h.commit(&feedback, Rating::Good);
    h.finish(&committed);
    assert_eq!(h.history()[0].stats.attempts, 1);
}

#[test]
fn learn_ahead_does_not_cross_the_four_am_boundary() {
    let h = Harness::new();
    h.clock
        .set(Utc.with_ymd_and_hms(2026, 1, 1, 3, 55, 0).unwrap());
    let book = h.import(&one_question("boundary"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    h.finish(&view);
}

#[test]
fn independent_copies_and_safe_delete() {
    let h = Harness::new();
    let a = h.import(&one_question("copy"));
    let b = h.import(&one_question("copy"));
    assert_ne!(a, b);
    h.service().delete_workbook(&a).unwrap();
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].local_id, b);
}

#[test]
fn backup_restore_and_corrupt_backup_rejection() {
    let h = Harness::new();
    h.import(&one_question("backup"));
    let backup = h._dir.path().join("manual.sqlite");
    h.service().backup_to(&backup).unwrap();
    h.import(&one_question("later"));
    assert_eq!(h.service().snapshot().unwrap().books.len(), 2);
    h.service().restore_from(&backup).unwrap();
    assert_eq!(h.service().snapshot().unwrap().books.len(), 1);
    let corrupt = h._dir.path().join("corrupt.sqlite");
    std::fs::write(&corrupt, b"not a database").unwrap();
    assert!(h.service().restore_from(&corrupt).is_err());
    assert_eq!(h.service().snapshot().unwrap().books.len(), 1);
}

fn create_legacy_db(path: &std::path::Path) -> String {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            r#"
            PRAGMA application_id=1463962443;
            PRAGMA user_version=1;
            CREATE TABLE workbooks(id TEXT PRIMARY KEY, content TEXT NOT NULL, imported_at TEXT NOT NULL, last_used TEXT);
            CREATE TABLE notes(workbook_id TEXT NOT NULL, id TEXT NOT NULL, introduced INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(workbook_id,id));
            CREATE TABLE questions(workbook_id TEXT NOT NULL, note_id TEXT NOT NULL, id TEXT NOT NULL, buried_until TEXT, PRIMARY KEY(workbook_id,id));
            CREATE TABLE review_states(workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, card TEXT NOT NULL, due TEXT NOT NULL, PRIMARY KEY(workbook_id,question_id));
            CREATE TABLE sessions(id TEXT PRIMARY KEY, data TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1);
            CREATE TABLE review_logs(session_id TEXT NOT NULL, workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(session_id,workbook_id,question_id));
            CREATE TABLE practice_attempts(session_id TEXT NOT NULL, workbook_id TEXT NOT NULL, question_id TEXT NOT NULL, outcome TEXT NOT NULL, PRIMARY KEY(session_id,workbook_id,question_id));
            CREATE TABLE settings(id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);
            "#,
        )
        .unwrap();
    let workbook = workbook_domain::parse_workbook(SAMPLE_WORKBOOK_JSON).unwrap();
    let legacy_id = "legacy-book";
    connection
        .execute(
            "INSERT INTO workbooks VALUES(?1,?2,?3,NULL)",
            rusqlite::params![legacy_id, SAMPLE_WORKBOOK_JSON, "2025-12-01T00:00:00.000Z"],
        )
        .unwrap();
    for note in &workbook.notes {
        connection
            .execute(
                "INSERT INTO notes(workbook_id,id,introduced) VALUES(?1,?2,1)",
                rusqlite::params![legacy_id, note.id],
            )
            .unwrap();
        for question in &note.questions {
            connection
                .execute(
                    "INSERT INTO questions(workbook_id,note_id,id,buried_until) VALUES(?1,?2,?3,NULL)",
                    rusqlite::params![legacy_id, note.id, question.id],
                )
                .unwrap();
        }
    }
    let card = serde_json::json!({
        "stability": 5.0, "stabilityFast": 20.0, "difficulty": 4.0, "state": 2,
        "dueAt": "2025-12-31T00:00:00.000Z", "lastReviewAt": "2025-12-20T00:00:00.000Z",
        "cardId": "legacy-card", "reps": 3, "lapses": 0, "scheduledDays": 11.0,
        "learningStep": 0, "scheduleStatus": "review"
    });
    connection
        .execute(
            "INSERT INTO review_states VALUES(?1,'q1',?2,'2025-12-31T00:00:00.000Z')",
            rusqlite::params![legacy_id, card.to_string()],
        )
        .unwrap();
    let outcome = serde_json::json!({
        "correct": true, "unknown": false, "hint": false, "rating": 3,
        "answer": "b", "elapsedMs": 1234, "at": "2025-12-20T00:00:00.000Z",
        "before": null
    });
    let mut outcomes = serde_json::Map::new();
    outcomes.insert(format!("{legacy_id}/q1"), outcome.clone());
    let mut session = serde_json::json!({
        "id":"legacy-session","mode":"new",
        "queue":[{"workbookId":legacy_id,"noteId":"promise","questionId":"q1"}],
        "index":1,"phase":"workbookResult","draft":null,"hint":false,"revealed":false,
        "startedAt":"2025-12-20T00:00:00.000Z","questionStartedAt":"2025-12-20T00:00:00.000Z",
        "questionElapsedMs":1234,"paused":false,
        "outcomes":{},"skipped":[],
        "endedAt":"2025-12-20T00:10:00.000Z"
    });
    session["outcomes"] = serde_json::Value::Object(outcomes);
    connection
        .execute(
            "INSERT INTO sessions VALUES('legacy-session',?,0)",
            rusqlite::params![session.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO review_logs VALUES('legacy-session',?1,'q1',?2)",
            rusqlite::params![legacy_id, outcome.to_string()],
        )
        .unwrap();
    let active = serde_json::json!({
        "id":"legacy-active","mode":"review",
        "queue":[{"workbookId":legacy_id,"noteId":"async","questionId":"q2"}],
        "index":0,"phase":"question","draft":null,"hint":false,"revealed":false,
        "startedAt":"2025-12-31T00:00:00.000Z","questionStartedAt":"2025-12-31T00:00:00.000Z",
        "questionElapsedMs":0,"paused":false,"outcomes":{},"skipped":[],"endedAt":null
    });
    connection
        .execute(
            "INSERT INTO sessions VALUES('legacy-active',?,1)",
            rusqlite::params![active.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO settings VALUES(1,?)",
            rusqlite::params![
                serde_json::json!({
                    "theme":"system","newLimit":10,"automaticRating":true,
                    "retention":0.9,"externalImages":true,"autoBackup":true
                })
                .to_string()
            ],
        )
        .unwrap();
    workbook.title
}

#[test]
fn legacy_v1_migration_preserves_cards_and_single_attempt_history() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.sqlite");
    let title = create_legacy_db(&legacy);
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path().join("new-data").as_path(), clock).unwrap();
    let preview = WorkbookService::legacy_preview(&legacy).unwrap();
    assert_eq!(preview.workbook_count, 1);
    assert_eq!(preview.review_state_count, 1);
    assert_eq!(preview.attempt_count, 1);
    assert!(preview.active_session);
    let imported = service.import_legacy(&legacy).unwrap();
    assert_eq!(imported.attempt_count, 1);
    let snapshot = service.snapshot().unwrap();
    assert_eq!(snapshot.books.len(), 1);
    assert_eq!(snapshot.books[0].title, title);
    assert_eq!(snapshot.books[0].learned, 1);
    assert_eq!(snapshot.books[0].due, 1);
    let history = service.history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].stats.attempts, 1);
    assert_eq!(history[0].stats.first_pass_correct, 1);
    let active = service
        .active_view()
        .unwrap()
        .expect("migrated active session");
    assert_eq!(active.phase, Phase::Question);
    assert_eq!(active.note_title.as_deref(), Some("async関数"));
}

#[test]
fn legacy_invalid_database_is_rejected_without_touching_new_data() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.sqlite");
    let connection = Connection::open(&bad).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=42; PRAGMA user_version=1; CREATE TABLE workbooks(id TEXT);",
        )
        .unwrap();
    drop(connection);
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let service = WorkbookService::open(dir.path().join("new-data").as_path(), clock).unwrap();
    service
        .import_workbook_json(&one_question("existing"))
        .unwrap();
    assert!(service.import_legacy(&bad).is_err());
    assert_eq!(service.snapshot().unwrap().books.len(), 1);
}

#[test]
fn scenario_a_new_again_learn_ahead_good_transitions_to_review() {
    let h = Harness::new();
    let book = h.import(&one_question("scenario-a"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::Question);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.item_kind, "short_term");
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    assert_eq!(view.phase, Phase::WorkbookResult);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.books[0].learned, 1);
    assert_eq!(snapshot.books[0].due, 0);
    let stats = h.history()[0].stats;
    assert_eq!(stats.attempts, 3);
    assert_eq!(stats.short_term, 2);
    assert_eq!(stats.first_pass_graded, 1);
    h.clock.advance(chrono::Duration::days(11));
    assert_eq!(h.service().snapshot().unwrap().books[0].due, 1);
}

#[test]
fn scenario_b_review_again_relearning_then_new_review_due() {
    let h = Harness::new();
    let book = h.import(&one_question("scenario-b"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    h.finish(&view);
    h.clock.advance(chrono::Duration::days(11));

    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 1);
    let review = h.start(Mode::Review, None);
    assert_eq!(review.phase, Phase::Question);
    let review = h.answer(&review, Some(Answer::Bool(false)), Rating::Again);
    let review = h.next(&review);
    assert_eq!(review.item_kind, "short_term");
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&review);
    assert_eq!(h.service().snapshot().unwrap().today.due, 0);
    h.clock.advance(chrono::Duration::days(2));
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
}

#[test]
fn scenario_c_restart_uses_saved_due_without_duplicate_attempts() {
    let mut h = Harness::new();
    let book = h.import(&one_question("scenario-c"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    let view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    let view = h.next(&view);
    h.finish(&view);
    h.reopen();
    h.clock.advance(chrono::Duration::days(11));
    assert_eq!(h.service().snapshot().unwrap().today.due, 1);
    let review = h.start(Mode::Review, None);
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    h.finish(&review);
    let attempts: usize = h.history().iter().map(|entry| entry.stats.attempts).sum();
    assert_eq!(attempts, 3);
    h.reopen();
    let attempts: usize = h.history().iter().map(|entry| entry.stats.attempts).sum();
    assert_eq!(attempts, 3);
    assert_eq!(h.service().snapshot().unwrap().today.due, 0);
}

#[test]
fn today_reports_upcoming_short_term_instead_of_completion() {
    let h = Harness::new();
    let book = h.import(&one_question("upcoming"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.due, 0);
    assert_eq!(snapshot.today.short_term_pending, 1);
    assert!(snapshot.today.next_due_at.is_some());
    let review = h.start(Mode::Review, None);
    assert_eq!(review.phase, Phase::Question);
    assert_eq!(review.item_kind, "initial");
    let review = h.answer(&review, Some(Answer::Bool(true)), Rating::Good);
    let next = h.next(&review);
    assert_eq!(next.item_kind, "short_term");
    h.finish(&next);
    let review_stats = h
        .history()
        .into_iter()
        .find(|entry| entry.mode == Mode::Review)
        .unwrap()
        .stats;
    assert_eq!(review_stats.attempts, 1);
    assert_eq!(review_stats.short_term, 1);
}

#[test]
fn next_wake_at_tracks_the_next_saved_due() {
    let h = Harness::new();
    let book = h.import(&one_question("wake"));
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Bool(false)), Rating::Again);
    h.finish(&view);
    let next = h.service().next_wake_at().unwrap().expect("next due");
    assert!(next > h.clock.now());
    assert!(next <= h.clock.now() + chrono::Duration::minutes(1));
}

#[test]
fn restart_after_commit_before_next_does_not_duplicate_attempt() {
    let mut h = Harness::new();
    let book = h.import(&one_question("commit-restart"));
    let view = h.start(Mode::New, Some(&book));
    let _view = h.answer(&view, Some(Answer::Bool(true)), Rating::Good);
    // Crash with the committed feedback still on screen.
    h.reopen();
    let feedback = h.service().active_view().unwrap().unwrap();
    assert_eq!(feedback.phase, Phase::Feedback);
    let feedback_view = feedback.feedback.as_ref().unwrap();
    assert!(feedback_view.committed);
    assert!(feedback_view.attempt_id.is_some());
    h.finish(&feedback);
    assert_eq!(h.history()[0].stats.attempts, 1);
}

#[test]
fn next_wake_also_schedules_the_four_am_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 3, 55, 0).unwrap(),
    ));
    let service = WorkbookService::open(dir.path(), clock).unwrap();
    let mut settings = service.settings().unwrap();
    settings.timezone = "UTC".to_string();
    service.update_settings(settings).unwrap();
    let next = service.next_wake_at().unwrap().unwrap();
    assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 4, 0, 0).unwrap());
}

#[test]
fn daily_new_limit_counts_commits_and_resets_after_four_am() {
    let h = Harness::new();
    let book = h.import(SAMPLE_WORKBOOK_JSON);
    let mut settings = h.service().settings().unwrap();
    settings.new_limit = 1;
    h.service().update_settings(settings).unwrap();
    let view = h.start(Mode::New, Some(&book));
    let view = h.answer(&view, Some(Answer::Text("b".to_string())), Rating::Good);
    h.finish(&view);
    let snapshot = h.service().snapshot().unwrap();
    assert_eq!(snapshot.today.new_remaining_today, 0);
    let error = h
        .service()
        .start_session(Mode::New, Some(&book))
        .unwrap_err();
    assert!(matches!(error, ServiceError::Invalid(_)));
    h.clock.advance(chrono::Duration::hours(5));
    assert_eq!(h.service().snapshot().unwrap().today.new_remaining_today, 1);
    h.service().start_session(Mode::New, Some(&book)).unwrap();
}
